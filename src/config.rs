const APP_CONFIG_FILE: &str = "akler.conf";
const LEGACY_CONFIG_FILE: &str = "layouter.conf";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct NgramLimits {
    trigrams: Option<usize>,
    tetragrams: Option<usize>,
    pentagrams: Option<usize>,
}

impl NgramLimits {
    fn for_order(self, order: usize) -> Option<usize> {
        match order {
            3 => self.trigrams,
            4 => self.tetragrams,
            5 => self.pentagrams,
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
struct AppConfig {
    weights: Weights,
    search: SearchSettings,
    rank_columns: [bool; RANK_COUNT],
    ngrams: NgramLimits,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            weights: Weights::default(),
            search: SearchSettings::default(),
            rank_columns: default_rank_columns(),
            ngrams: NgramLimits::default(),
        }
    }
}

fn canonical_config_key<'a>(section: &str, key: &'a str) -> &'a str {
    match (section, key) {
        ("search", "max_sfb") => "max_sfb_increase",
        ("search", "max_sfs") => "max_sfs_increase",
        ("search", "metrics") => "mode",
        ("rolls", "include_stetches") => "include_stretches",
        _ => key,
    }
}

fn config_sections(text: &str) -> AppResult<BTreeMap<String, String>> {
    let mut sections = BTreeMap::<String, String>::new();
    let mut section = String::new();
    let mut seen = BTreeSet::new();

    for (index, line) in text.lines().enumerate() {
        let content = line.split('#').next().unwrap_or("").trim();
        if content.is_empty() {
            continue;
        }

        if let Some(name) = content.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            section = name.trim().to_ascii_lowercase();
            if !["weights", "search", "ranker", "ngrams", "rolls"].contains(&section.as_str()) {
                return Err(format!("line {}: unknown section [{section}]", index + 1).into());
            }
            if sections.insert(section.clone(), String::new()).is_some() {
                return Err(format!("line {}: duplicate section [{section}]", index + 1).into());
            }
            continue;
        }

        if section.is_empty() {
            return Err(format!("line {}: setting must follow a [section]", index + 1).into());
        }
        let (key, value) = content
            .split_once('=')
            .ok_or_else(|| format!("line {}: expected name = value", index + 1))?;
        let key = key.trim().to_ascii_lowercase();
        if key.is_empty() || value.trim().is_empty() {
            return Err(format!("line {}: setting name and value cannot be empty", index + 1).into());
        }
        let canonical = canonical_config_key(&section, &key);
        if !seen.insert((section.clone(), canonical.to_string())) {
            return Err(format!("line {}: duplicate setting {section}.{canonical}", index + 1).into());
        }
        let body = sections.get_mut(&section).expect("section inserted above");
        body.push_str(content);
        body.push('\n');
    }

    Ok(sections)
}

fn parse_ngram_limit(value: &str) -> AppResult<Option<usize>> {
    if value.eq_ignore_ascii_case("all") {
        return Ok(None);
    }
    let count: usize = value
        .parse()
        .map_err(|_| "expected all or a positive n-gram count")?;
    if count == 0 {
        return Err("expected all or a positive n-gram count".into());
    }
    Ok(Some(count))
}

fn parse_app_config(text: &str) -> AppResult<AppConfig> {
    let mut config = AppConfig::default();
    let mut rolls = RollSettings::default();

    for (section, body) in config_sections(text)? {
        let result = (|| -> AppResult<()> {
            match section.as_str() {
                "weights" => config.weights = weights_from_text(&body)?,
                "search" => config.search = search_from_text(&body)?,
                "ranker" => {
                    for (key, value) in config_lines(&body)? {
                        match key.as_str() {
                            "columns" => config.rank_columns = parse_rank_columns(&value)?,
                            _ => return Err(format!("unknown setting {key}").into()),
                        }
                    }
                }
                "rolls" => {
                    for (key, value) in config_lines(&body)? {
                        let include = match value.to_ascii_lowercase().as_str() {
                            "true" => true,
                            "false" => false,
                            _ => return Err(format!("{key} must be true or false").into()),
                        };
                        match canonical_config_key("rolls", &key) {
                            "include_thumbs" => rolls.include_thumbs = include,
                            "include_scissors" => rolls.include_scissors = include,
                            "include_stretches" => rolls.include_stretches = include,
                            _ => return Err(format!("unknown setting {key}").into()),
                        }
                    }
                }
                "ngrams" => {
                    for (key, value) in config_lines(&body)? {
                        match key.as_str() {
                            "trigrams" => config.ngrams.trigrams = parse_ngram_limit(&value)?,
                            "tetragrams" => config.ngrams.tetragrams = parse_ngram_limit(&value)?,
                            "pentagrams" => config.ngrams.pentagrams = parse_ngram_limit(&value)?,
                            _ => return Err(format!("unknown setting {key}").into()),
                        }
                    }
                }
                _ => unreachable!("config_sections validates section names"),
            }
            Ok(())
        })();
        result.map_err(|error| format!("[{section}]: {error}"))?;
    }

    config.weights = config.weights.with_rolls(rolls);
    Ok(config)
}

fn read_optional_config(path: &Path) -> AppResult<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}: {error}", path.display()).into()),
    }
}

fn load_legacy_config() -> AppResult<AppConfig> {
    let mut config = AppConfig::default();
    if let Some(text) = read_optional_config(Path::new(WEIGHTS_FILE))? {
        config.weights = weights_from_text(&text)
            .map_err(|error| format!("{WEIGHTS_FILE}: {error}"))?;
    }
    if let Some(text) = read_optional_config(Path::new(SEARCH_FILE))? {
        config.search = search_from_text(&text)
            .map_err(|error| format!("{SEARCH_FILE}: {error}"))?;
    }
    if let Some(text) = read_optional_config(Path::new("ranker-columns.conf"))? {
        config.rank_columns = parse_rank_columns(&text)
            .map_err(|error| format!("ranker-columns.conf: {error}"))?;
    }
    Ok(config)
}

fn load_app_config() -> AppResult<AppConfig> {
    match read_active_config()? {
        Some((name, text)) => parse_app_config(&text)
            .map_err(|error| format!("{name}: {error}").into()),
        None => load_legacy_config(),
    }
}

fn read_active_config() -> AppResult<Option<(&'static str, String)>> {
    if let Some(text) = read_optional_config(Path::new(APP_CONFIG_FILE))? {
        return Ok(Some((APP_CONFIG_FILE, text)));
    }
    Ok(read_optional_config(Path::new(LEGACY_CONFIG_FILE))?
        .map(|text| (LEGACY_CONFIG_FILE, text)))
}

fn rank_config_text(hidden: &[bool; RANK_COUNT]) -> String {
    let names: Vec<_> = RANK_ORDER
        .iter()
        .copied()
        .filter(|&metric| !hidden[metric])
        .map(rank_name)
        .collect();
    format!("columns = {}\n", names.join(" "))
}

fn rolls_config_text(rolls: RollSettings) -> String {
    format!(
        "include_thumbs = {}\ninclude_scissors = {}\ninclude_stretches = {}\n",
        rolls.include_thumbs,
        rolls.include_scissors,
        rolls.include_stretches,
    )
}

fn app_config_text(config: &AppConfig) -> String {
    let limit = |value: Option<usize>| value.map(|v| v.to_string()).unwrap_or_else(|| "all".into());
    format!(
        "# akler configuration; see doc/USAGE.md.\n\
         # Missing settings use built-in defaults. Limits other than all are approximate.\n\n\
         [weights]\n{}\n[rolls]\n{}\n[search]\n{}\n[ranker]\n{}\n\
         [ngrams]\ntrigrams = {}\ntetragrams = {}\npentagrams = {}\n",
        weights_text(&config.weights),
        rolls_config_text(config.weights.rolls()),
        search_settings_text(&config.search),
        rank_config_text(&config.rank_columns),
        limit(config.ngrams.trigrams),
        limit(config.ngrams.tetragrams),
        limit(config.ngrams.pentagrams),
    )
}

// Update one section without discarding comments or touching other settings.
// Keys removed from the section retain their inline comment as a comment line.
fn replace_config_section(text: &str, section: &str, replacement: &str) -> AppResult<String> {
    let entries = config_lines(replacement)?;
    let values: BTreeMap<_, _> = entries.iter().cloned().collect();
    let mut remaining: BTreeSet<_> = values.keys().cloned().collect();
    let mut output = String::new();
    let mut active = false;
    let mut found = false;

    let append_remaining = |output: &mut String, remaining: &mut BTreeSet<String>| {
        for (key, value) in &entries {
            if remaining.remove(key) {
                output.push_str(&format!("{key} = {value}\n"));
            }
        }
    };

    for line in text.lines() {
        let content = line.split('#').next().unwrap_or("").trim();
        if let Some(name) = content.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            if active {
                append_remaining(&mut output, &mut remaining);
            }
            active = name.trim().eq_ignore_ascii_case(section);
            found |= active;
        }

        if active {
            if let Some((key, _)) = content.split_once('=') {
                let key = key.trim().to_ascii_lowercase();
                let canonical = canonical_config_key(section, &key);
                let comment = line.find('#').map(|index| &line[index..]);
                if let Some(value) = values.get(canonical) {
                    remaining.remove(canonical);
                    let prefix = line.split_once('=').expect("assignment checked above").0;
                    output.push_str(prefix);
                    output.push_str("= ");
                    output.push_str(value);
                    if let Some(comment) = comment {
                        output.push(' ');
                        output.push_str(comment);
                    }
                    output.push('\n');
                } else if let Some(comment) = comment {
                    output.push_str(comment);
                    output.push('\n');
                }
                continue;
            }
        }

        output.push_str(line);
        output.push('\n');
    }

    if !found {
        if !output.is_empty() && !output.ends_with("\n\n") {
            output.push('\n');
        }
        output.push_str(&format!("[{section}]\n"));
    }
    append_remaining(&mut output, &mut remaining);
    Ok(output)
}

fn save_config_sections(sections: &[(&str, String)]) -> AppResult<()> {
    let (source, mut text) = match read_active_config()? {
        Some((name, text)) => (name, text),
        None => (APP_CONFIG_FILE, app_config_text(&load_legacy_config()?)),
    };
    parse_app_config(&text).map_err(|error| format!("{source}: {error}"))?;

    for (section, replacement) in sections {
        text = replace_config_section(&text, section, replacement)?;
    }
    parse_app_config(&text).map_err(|error| format!("{APP_CONFIG_FILE}: {error}"))?;
    atomic_write(Path::new(APP_CONFIG_FILE), &text, false)
}

fn save_optimizer_settings(weights: &Weights, search: &SearchSettings) -> AppResult<()> {
    validate_search_settings(search)?;
    save_config_sections(&[
        ("weights", weights_text(weights)),
        ("rolls", rolls_config_text(weights.rolls())),
        ("search", search_settings_text(search)),
    ])
}

fn save_rank_columns(hidden: &[bool; RANK_COUNT]) -> AppResult<()> {
    save_config_sections(&[("ranker", rank_config_text(hidden))])
}

#[cfg(test)]
mod config_tests {
    use super::*;

    #[test]
    fn unified_defaults_round_trip_without_changing_settings() {
        let defaults = AppConfig::default();
        let parsed = parse_app_config(&app_config_text(&defaults)).unwrap();
        assert_eq!(parsed.weights.0, defaults.weights.0);
        assert_eq!(
            search_settings_text(&parsed.search),
            search_settings_text(&defaults.search)
        );
        assert_eq!(parsed.rank_columns, defaults.rank_columns);
        assert_eq!(parsed.ngrams, defaults.ngrams);
    }

    #[test]
    fn legacy_values_survive_unified_serialization() {
        let mut legacy = AppConfig::default();
        legacy.weights = weights_from_text("fsb = 9\nsfs = 2\n").unwrap();
        legacy.search = search_from_text("method = sweep\nseconds = 17\ncorpus.books = 2\n").unwrap();
        legacy.rank_columns = parse_rank_columns("SCORE SFB OSF").unwrap();

        let parsed = parse_app_config(&app_config_text(&legacy)).unwrap();
        assert_eq!(parsed.weights.0, legacy.weights.0);
        assert_eq!(parsed.weights.0[DFSB], 9.0);
        assert_eq!(parsed.weights.0[CFSB], 4.5);
        assert_eq!(
            search_settings_text(&parsed.search),
            search_settings_text(&legacy.search)
        );
        assert_eq!(parsed.rank_columns, legacy.rank_columns);
    }

    #[test]
    fn unified_config_changes_only_explicit_values() {
        let parsed = parse_app_config(
            "[weights]\nsfb = 7\n[search]\nseconds = 15\ncorpus.reddit = 2\n\
             [ranker]\ncolumns = SCORE SFB\n[ngrams]\n\
             trigrams = 2000\ntetragrams = 4000\npentagrams = all\n",
        )
        .unwrap();
        assert_eq!(parsed.weights.0[SFB], 7.0);
        assert_eq!(parsed.weights.0[SFS], Weights::default().0[SFS]);
        assert_eq!(parsed.search.seconds, 15.0);
        assert_eq!(parsed.search.mix.get("reddit"), Some(&2.0));
        assert_eq!(parsed.rank_columns, parse_rank_columns("SCORE SFB").unwrap());
        assert_eq!(parsed.ngrams.for_order(3), Some(2000));
        assert_eq!(parsed.ngrams.for_order(4), Some(4000));
        assert_eq!(parsed.ngrams.for_order(5), None);
        assert_eq!(parsed.ngrams.for_order(2), None);
    }

    #[test]
    fn unified_config_rejects_invalid_and_duplicate_settings() {
        for text in [
            "sfb = 1",
            "[typo]\nsfb = 1",
            "[weights]\nsfb = 1\nsfb = 2",
            "[weights]\n[weights]\n",
            "[search]\nmax_sfb = 1\nmax_sfb_increase = 2",
            "[weights]\nunknown = 1",
            "[weights]\nsfb = NaN",
            "[search]\nseconds = -1",
            "[ranker]\ncolumns = SCORE INVALID",
            "[ranker]\ncolumns = ",
            "[ngrams]\ntrigrams = 0",
            "[ngrams]\ntrigrams = -1",
            "[ngrams]\ntrigrams = 1.5",
            "[ngrams]\nbigrams = 2000",
        ] {
            assert!(parse_app_config(text).is_err(), "accepted {text:?}");
        }
    }

    #[test]
    fn saving_section_preserves_comments_and_unrelated_settings() {
        let text = "# user note\n[weights]\nsfb = 7 # tune later\n\n\
                    [search]\n# preserve this\nseconds = 9\nmax_sfb = 0.2 # old name\n\
                    corpus.old = 1 # old mixture note\n\n\
                    [ranker]\ncolumns = SCORE # concise\n\n\
                    [ngrams]\ntrigrams = 2000 # approximate\n";
        let mut search = SearchSettings::default();
        search.seconds = 12.0;
        let updated = replace_config_section(text, "search", &search_settings_text(&search)).unwrap();
        let parsed = parse_app_config(&updated).unwrap();
        assert_eq!(
            search_settings_text(&parsed.search),
            search_settings_text(&search)
        );
        assert_eq!(parsed.weights.0[SFB], 7.0);
        assert_eq!(parsed.ngrams.trigrams, Some(2000));
        for unchanged in [
            "# user note",
            "sfb = 7 # tune later",
            "# preserve this",
            "max_sfb = 0 # old name",
            "# old mixture note",
            "columns = SCORE # concise",
            "trigrams = 2000 # approximate",
        ] {
            assert!(updated.contains(unchanged), "lost {unchanged:?}");
        }
        assert!(!updated.contains("corpus.old ="));
    }

    #[test]
    fn saving_can_add_missing_sections_and_keep_aliases() {
        let original = "[search]\nmetrics = simple # alias\n";
        let with_ranker = replace_config_section(original, "ranker", "columns = SFB SFS\n").unwrap();
        let updated = replace_config_section(
            &with_ranker,
            "search",
            &search_settings_text(&SearchSettings::default()),
        )
        .unwrap();
        let parsed = parse_app_config(&updated).unwrap();
        assert_eq!(parsed.search.mode, "detailed");
        assert!(updated.contains("metrics = detailed # alias"));
        assert_eq!(parsed.rank_columns, parse_rank_columns("SFB SFS").unwrap());
    }

    #[test]
    fn saving_weights_preserves_legacy_scissor_cost_and_comment() {
        let original = "[weights]\nfsb = 9 # old scissors weight\nsfb = 6\n";
        let before = parse_app_config(original).unwrap();
        let updated = replace_config_section(original, "weights", &weights_text(&before.weights)).unwrap();
        let after = parse_app_config(&updated).unwrap();
        assert_eq!(before.weights.0, after.weights.0);
        assert!(updated.contains("# old scissors weight"));
        assert!(!config_lines(&config_sections(&updated).unwrap()["weights"])
            .unwrap()
            .iter()
            .any(|(key, _)| key == "fsb"));
    }
}

#[cfg(test)]
mod roll_config_tests {
    use super::*;

    #[test]
    fn roll_options_round_trip_and_preserve_defaults_and_aliases() {
        assert_eq!(parse_app_config("").unwrap().weights.rolls(), RollSettings::default());
        for include_thumbs in [false, true] {
            for include_scissors in [false, true] {
                for include_stretches in [false, true] {
                    let rolls = RollSettings {
                        include_thumbs,
                        include_scissors,
                        include_stretches,
                    };
                    let text = format!("[rolls]\n{}[weights]\nsfb = 7\n", rolls_config_text(rolls));
                    let parsed = parse_app_config(&text).unwrap();
                    assert_eq!(parsed.weights.rolls(), rolls);
                    assert_eq!(parsed.weights.0[SFB], 7.0);
                    let round = parse_app_config(&app_config_text(&parsed)).unwrap();
                    assert_eq!(round.weights.rolls(), rolls);
                    assert_eq!(round.weights.0, parsed.weights.0);
                }
            }
        }
        let alias = "[rolls]\ninclude_stetches = false # retain this comment\n";
        let before = parse_app_config(alias).unwrap();
        assert!(!before.weights.rolls().include_stretches);
        let replacement = rolls_config_text(RollSettings::default());
        let updated = replace_config_section(alias, "rolls", &replacement).unwrap();
        assert!(updated.contains("# retain this comment"));
        assert_eq!(parse_app_config(&updated).unwrap().weights.rolls(), RollSettings::default());
    }

    #[test]
    fn roll_options_reject_invalid_and_duplicate_keys() {
        for body in [
            "include_thumbs = yes",
            "include_scissors = 1",
            "include_stretches = no",
            "include_stretches = true\ninclude_stetches = false",
            "include_thumbs = true\ninclude_thumbs = false",
            "unknown = true",
            "mode = good",
        ] {
            assert!(parse_app_config(&format!("[rolls]\n{body}\n")).is_err(), "{body}");
        }
    }
}
