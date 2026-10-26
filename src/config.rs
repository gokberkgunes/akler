const APP_CONFIG_FILE: &str = "akler.conf";
const LEGACY_CONFIG_FILE: &str = "layouter.conf";
const RANK_HIDDEN_LAYOUTS_FILE: &str = "ranker-hidden.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NgramLimits {
    trigrams: Option<usize>,
    tetragrams: Option<usize>,
    pentagrams: Option<usize>,
    include_spacegrams: bool,
}

impl Default for NgramLimits {
    fn default() -> Self {
        Self {
            trigrams: None,
            tetragrams: None,
            pentagrams: None,
            include_spacegrams: true,
        }
    }
}

impl NgramLimits {
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct RankFilters {
    hide_magic: bool,
    hide_non_thumb: bool,
}

#[derive(Clone, Debug)]
pub struct MagicSettings {
    pub allowed_fingers: [bool; 10],
    pub allowed_keys: Option<BTreeSet<(usize, usize)>>,
    /// Rules fitted by the normal optimizer's magic search; at most five use
    /// the fast conditional fitter.
    pub max_rules: usize,
    /// Screened rule proposals given exact trials in each fitting round.
    pub shortlist: usize,
}

impl Default for MagicSettings {
    fn default() -> Self {
        Self {
            allowed_fingers: [
                false, false, true, true, true, true, false, false, true, true,
            ],
            allowed_keys: None,
            max_rules: 5,
            shortlist: 24,
        }
    }
}

impl MagicSettings {
    fn allows(&self, key: Key) -> bool {
        match &self.allowed_keys {
            Some(keys) => keys.contains(&(key.row as usize, key.col as usize)),
            None => self
                .allowed_fingers
                .get(key.finger)
                .copied()
                .unwrap_or(false),
        }
    }
}

#[derive(Clone, Debug)]
struct AppConfig {
    weights: Weights,
    search: SearchSettings,
    rank_columns: [bool; RANK_COUNT],
    rank_filters: RankFilters,
    rank_hidden_layouts: BTreeSet<PathBuf>,
    ngrams: NgramLimits,
    magic: MagicSettings,
    combos: crate::combo_fit::Settings,
    adaptive: crate::swap_fit::Settings,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            weights: Weights::default(),
            search: SearchSettings::default(),
            rank_columns: default_rank_columns(),
            rank_filters: RankFilters::default(),
            rank_hidden_layouts: BTreeSet::new(),
            ngrams: NgramLimits::default(),
            magic: MagicSettings::default(),
            combos: crate::combo_fit::Settings {
                max_combos: 0,
                ..Default::default()
            },
            adaptive: crate::swap_fit::Settings {
                max_swaps: 0,
                ..Default::default()
            },
        }
    }
}

// Configuration comments begin at an unquoted '#'. This matters for persisted
// layout paths: a '#' inside a quoted path is part of the path.
fn config_comment_index(line: &str) -> Option<usize> {
    let mut quoted = false;
    let mut escaped = false;
    for (index, ch) in line.char_indices() {
        if escaped {
            escaped = false;
        } else if quoted && ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            quoted = !quoted;
        } else if ch == '#' && !quoted {
            return Some(index);
        }
    }
    None
}

fn config_content(line: &str) -> &str {
    &line[..config_comment_index(line).unwrap_or(line.len())]
}

fn canonical_config_key<'a>(section: &str, key: &'a str) -> &'a str {
    match (section, key) {
        ("search", "max_sfb") => "max_sfb_increase",
        ("search", "max_sfs") => "max_sfs_increase",
        ("search", "metrics") => "mode",
        ("search", "max_action_anneal_steps") => "magic_anneal_steps",
        ("rolls", "include_stetches") => "include_stretches",
        _ => key,
    }
}

fn config_sections(text: &str) -> AppResult<BTreeMap<String, String>> {
    let mut sections = BTreeMap::<String, String>::new();
    let mut section = String::new();
    let mut seen = BTreeSet::new();

    for (index, line) in text.lines().enumerate() {
        let content = config_content(line).trim();
        if content.is_empty() {
            continue;
        }

        if let Some(name) = content.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            section = name.trim().to_ascii_lowercase();
            if ![
                "weights", "aklgg", "search", "ranker", "ngrams", "rolls", "typing", "alts",
                "magic", "combos", "adaptive",
            ]
            .contains(&section.as_str())
            {
                if section == "mana" {
                    return Err(format!(
                        "line {}: [mana] was removed; move its weights to [aklgg]",
                        index + 1
                    )
                    .into());
                }
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
        if key.is_empty()
            || (value.trim().is_empty() && !(section == "magic" && key == "allowed_keys"))
        {
            return Err(
                format!("line {}: setting name and value cannot be empty", index + 1).into(),
            );
        }
        let canonical = canonical_config_key(&section, &key);
        if !seen.insert((section.clone(), canonical.to_string())) {
            return Err(format!(
                "line {}: duplicate setting {section}.{canonical}",
                index + 1
            )
            .into());
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

fn parse_rank_hidden_layouts(value: &str) -> AppResult<BTreeSet<PathBuf>> {
    let Json::Array(values) = parse_json(value)
        .map_err(|error| format!("hidden_layouts must be a JSON list of quoted paths: {error}"))?
    else {
        return Err("hidden_layouts must be a JSON list of quoted paths".into());
    };
    values
        .into_iter()
        .map(|value| match value {
            Json::String(path) if !path.is_empty() => Ok(PathBuf::from(path)),
            Json::String(_) => Err("hidden_layouts: paths cannot be empty".into()),
            _ => Err("hidden_layouts: every path must be a quoted string".into()),
        })
        .collect()
}

fn rank_hidden_layouts_text(hidden_layouts: &BTreeSet<PathBuf>) -> AppResult<String> {
    let paths = hidden_layouts
        .iter()
        .map(|path| quoted_rank_path(path))
        .collect::<AppResult<Vec<_>>>()?;
    Ok(format!("[{}]\n", paths.join(", ")))
}

fn read_rank_hidden_layouts() -> AppResult<Option<BTreeSet<PathBuf>>> {
    let Some(text) = read_optional_config(Path::new(RANK_HIDDEN_LAYOUTS_FILE))? else {
        return Ok(None);
    };
    parse_rank_hidden_layouts(&text)
        .map(Some)
        .map_err(|error| format!("{RANK_HIDDEN_LAYOUTS_FILE}: {error}").into())
}

fn ranker_config_lines(text: &str) -> AppResult<Vec<(String, String)>> {
    let mut result = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = config_content(line).trim();
        if line.is_empty() {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("line {}: expected name = value", index + 1))?;
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim().to_string();
        if key.is_empty() || value.is_empty() {
            return Err(
                format!("line {}: setting name and value cannot be empty", index + 1).into(),
            );
        }
        result.push((key, value));
    }
    Ok(result)
}

fn rank_filters_from_text(text: &str) -> AppResult<RankFilters> {
    let mut filters = RankFilters::default();
    let mut in_ranker = false;
    let mut seen = BTreeSet::new();

    for (index, line) in text.lines().enumerate() {
        let content = config_content(line).trim();
        if let Some(name) = content.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            in_ranker = name.trim().eq_ignore_ascii_case("ranker");
            continue;
        }
        if !in_ranker || content.is_empty() {
            continue;
        }
        let Some((key, value)) = content.split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        if !["hide_magic", "hide_non_thumb"].contains(&key.as_str()) {
            continue;
        }
        if !seen.insert(key.clone()) {
            return Err(format!("line {}: duplicate setting ranker.{key}", index + 1).into());
        }
        let enabled = match value.trim().to_ascii_lowercase().as_str() {
            "true" => true,
            "false" => false,
            _ => return Err(format!("line {}: {key} must be true or false", index + 1).into()),
        };
        match key.as_str() {
            "hide_magic" => filters.hide_magic = enabled,
            "hide_non_thumb" => filters.hide_non_thumb = enabled,
            _ => unreachable!("filter keys checked above"),
        }
    }
    Ok(filters)
}

fn parse_app_config(text: &str) -> AppResult<AppConfig> {
    let mut config = AppConfig::default();
    let mut rolls = RollSettings::default();
    let mut aklgg = mana2_metrics::Weights::default();
    let mut alt_fingers = Weights::default().3;
    let mut alt_rules = crate::alt_fingers::AltRules::default();
    let mut combos = Weights::default().4;

    for (section, body) in config_sections(text)? {
        let result = (|| -> AppResult<()> {
            match section.as_str() {
                "weights" => config.weights = weights_from_text(&body)?,
                "aklgg" => aklgg = mana2_metrics::Weights::from_text(&body)?,
                "search" => config.search = search_from_text(&body)?,
                "magic" => {
                    for (key, value) in config_lines(&body)? {
                        match key.as_str() {
                            "allowed_fingers" => {
                                config.magic.allowed_fingers = [false; 10];
                                for name in value.split_whitespace() {
                                    if name.eq_ignore_ascii_case("none") {
                                        continue;
                                    }
                                    let finger = FINGER_NAMES.iter().position(|finger| finger.eq_ignore_ascii_case(name))
                                        .ok_or_else(|| format!("unknown finger {name}; use LP LR LM LI RI RM RR RP LT RT"))?;
                                    config.magic.allowed_fingers[finger] = true;
                                }
                            }
                            "allowed_keys" => {
                                config.magic.allowed_keys = if value.is_empty()
                                    || value.eq_ignore_ascii_case("all")
                                {
                                    None
                                } else {
                                    let mut keys = BTreeSet::new();
                                    for position in value.split_whitespace() {
                                        let (row, col) =
                                            position.split_once(':').ok_or_else(|| {
                                                format!("invalid key {position}; use row:column")
                                            })?;
                                        let row: usize = row.parse().map_err(|_| {
                                            format!(
                                                "invalid key {position}; use positive row:column"
                                            )
                                        })?;
                                        let col: usize = col.parse().map_err(|_| {
                                            format!(
                                                "invalid key {position}; use positive row:column"
                                            )
                                        })?;
                                        if row == 0 || col == 0 {
                                            return Err(format!("invalid key {position}; rows and columns start at 1").into());
                                        }
                                        keys.insert((row - 1, col - 1));
                                    }
                                    Some(keys)
                                };
                            }
                            "max_rules" => {
                                config.magic.max_rules = value
                                    .parse()
                                    .ok()
                                    .filter(|rules| {
                                        (1..=crate::magic_design::MAX_RULES).contains(rules)
                                    })
                                    .ok_or_else(|| {
                                        format!(
                                            "max_rules must be 1-{}",
                                            crate::magic_design::MAX_RULES
                                        )
                                    })?;
                            }
                            "shortlist" => {
                                config.magic.shortlist = value
                                    .parse()
                                    .ok()
                                    .filter(|count| *count > 0)
                                    .ok_or("shortlist must be a positive integer")?;
                            }
                            _ => return Err(format!("unknown setting {key}").into()),
                        }
                    }
                }
                "combos" => {
                    for (key, value) in config_lines(&body)? {
                        let count = |limit: usize| {
                            value
                                .parse::<usize>()
                                .ok()
                                .filter(|count| *count <= limit)
                                .ok_or_else(|| format!("{key} must be 0-{limit}"))
                        };
                        match key.as_str() {
                            "max" => config.combos.max_combos = count(crate::combo_fit::MAX_COMBOS)?,
                            "fatfingering" => {
                                config.combos.fatfingering = match value.to_ascii_lowercase().as_str()
                                {
                                    "true" => true,
                                    "false" => false,
                                    _ => return Err(format!("{key} must be true or false").into()),
                                }
                            }
                            "shortlist" => {
                                config.combos.shortlist = count(1000)?.max(1);
                            }
                            "exact" => {
                                config.combos.exact = match value.to_ascii_lowercase().as_str() {
                                    "true" => true,
                                    "false" => false,
                                    _ => return Err(format!("{key} must be true or false").into()),
                                }
                            }
                            _ => return Err(format!("unknown setting {key}").into()),
                        }
                    }
                }
                "adaptive" => {
                    for (key, value) in config_lines(&body)? {
                        let count = |limit: usize| {
                            value
                                .parse::<usize>()
                                .ok()
                                .filter(|count| *count <= limit)
                                .ok_or_else(|| format!("{key} must be 0-{limit}"))
                        };
                        match key.as_str() {
                            "max" => config.adaptive.max_swaps = count(crate::swap_fit::MAX_SWAPS)?,
                            "shortlist" => config.adaptive.shortlist = count(1000)?.max(1),
                            "exact" => {
                                config.adaptive.exact = match value.to_ascii_lowercase().as_str() {
                                    "true" => true,
                                    "false" => false,
                                    _ => return Err(format!("{key} must be true or false").into()),
                                }
                            }
                            _ => return Err(format!("unknown setting {key}").into()),
                        }
                    }
                }
                "ranker" => {
                    for (key, value) in ranker_config_lines(&body)? {
                        match key.as_str() {
                            "columns" => config.rank_columns = parse_rank_columns(&value)?,
                            "hide_magic" => {
                                config.rank_filters.hide_magic = match value
                                    .to_ascii_lowercase()
                                    .as_str()
                                {
                                    "true" => true,
                                    "false" => false,
                                    _ => return Err(format!("{key} must be true or false").into()),
                                }
                            }
                            "hide_non_thumb" => {
                                config.rank_filters.hide_non_thumb = match value
                                    .to_ascii_lowercase()
                                    .as_str()
                                {
                                    "true" => true,
                                    "false" => false,
                                    _ => return Err(format!("{key} must be true or false").into()),
                                }
                            }
                            "hidden_layouts" => {
                                config.rank_hidden_layouts = parse_rank_hidden_layouts(&value)?
                            }
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
                "typing" => {
                    for (key, value) in config_lines(&body)? {
                        let enabled = match value.to_ascii_lowercase().as_str() {
                            "true" => true,
                            "false" => false,
                            _ => return Err(format!("{key} must be true or false").into()),
                        };
                        match key.as_str() {
                            "alt_fingers" => alt_fingers = enabled,
                            "combos" => combos = enabled,
                            _ => return Err(format!("unknown setting {key}").into()),
                        }
                    }
                }
                "alts" => {
                    for (key, value) in config_lines(&body)? {
                        let rules = match key.as_str() {
                            "rowstag" => &mut alt_rules.rowstag,
                            "colstag" => &mut alt_rules.colstag,
                            _ => return Err(format!("unknown setting {key}").into()),
                        };
                        *rules = crate::alt_fingers::AltRules::parse(&value)
                            .map_err(|error| format!("{key}: {error}"))?;
                    }
                }
                "ngrams" => {
                    for (key, value) in config_lines(&body)? {
                        match key.as_str() {
                            "trigrams" => config.ngrams.trigrams = parse_ngram_limit(&value)?,
                            "tetragrams" => config.ngrams.tetragrams = parse_ngram_limit(&value)?,
                            "pentagrams" => config.ngrams.pentagrams = parse_ngram_limit(&value)?,
                            "include_spacegrams" => {
                                config.ngrams.include_spacegrams = match value
                                    .to_ascii_lowercase()
                                    .as_str()
                                {
                                    "true" => true,
                                    "false" => false,
                                    _ => return Err(format!("{key} must be true or false").into()),
                                }
                            }
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

    rolls.alts = alt_rules.intern();
    config.weights = config.weights.with_rolls(rolls);
    config.weights.2 = aklgg;
    config.weights.3 = alt_fingers;
    config.weights.4 = combos;
    config.weights.5 = config.combos.fatfingering;
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
        config.weights =
            weights_from_text(&text).map_err(|error| format!("{WEIGHTS_FILE}: {error}"))?;
    }
    if let Some(text) = read_optional_config(Path::new(SEARCH_FILE))? {
        config.search =
            search_from_text(&text).map_err(|error| format!("{SEARCH_FILE}: {error}"))?;
    }
    if let Some(text) = read_optional_config(Path::new("ranker-columns.conf"))? {
        config.rank_columns =
            parse_rank_columns(&text).map_err(|error| format!("ranker-columns.conf: {error}"))?;
    }
    Ok(config)
}

fn load_app_config() -> AppResult<AppConfig> {
    let mut config = match read_active_config()? {
        Some((name, text)) => {
            parse_app_config(&text).map_err(|error| format!("{name}: {error}").into())
        }
        None => load_legacy_config(),
    }?;
    if let Some(hidden_layouts) = read_rank_hidden_layouts()? {
        config.rank_hidden_layouts = hidden_layouts;
    }
    Ok(config)
}

fn load_rank_hidden_preferences() -> AppResult<BTreeSet<PathBuf>> {
    if let Some(hidden) = read_rank_hidden_layouts()? {
        return Ok(hidden);
    }
    let Some((_, text)) = read_active_config()? else {
        return Ok(BTreeSet::new());
    };
    let sections = config_sections(&text)?;
    if let Some(body) = sections.get("ranker") {
        for (key, value) in ranker_config_lines(body)? {
            if key == "hidden_layouts" {
                return parse_rank_hidden_layouts(&value);
            }
        }
    }
    Ok(BTreeSet::new())
}

fn load_rank_filters() -> AppResult<RankFilters> {
    let Some((name, text)) = read_active_config()? else {
        return Ok(RankFilters::default());
    };
    rank_filters_from_text(&text).map_err(|error| format!("{name}: [ranker]: {error}").into())
}

/// A configuration file read instead of `akler.conf` for the rest of this
/// process (`akler bench --config FILE`). It is never written.
static CONFIG_OVERRIDE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

fn use_config_file(path: &Path) -> AppResult<()> {
    fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    CONFIG_OVERRIDE
        .set(path.to_owned())
        .map_err(|_| "the configuration file was already chosen".into())
}

fn read_active_config() -> AppResult<Option<(&'static str, String)>> {
    if let Some(path) = CONFIG_OVERRIDE.get() {
        return Ok(Some(("--config file", fs::read_to_string(path)?)));
    }
    if let Some(text) = read_optional_config(Path::new(APP_CONFIG_FILE))? {
        return Ok(Some((APP_CONFIG_FILE, text)));
    }
    Ok(read_optional_config(Path::new(LEGACY_CONFIG_FILE))?.map(|text| (LEGACY_CONFIG_FILE, text)))
}

fn quoted_rank_path(path: &Path) -> AppResult<String> {
    let path = path
        .to_str()
        .ok_or_else(|| format!("cannot save non-UTF-8 layout path {}", path.display()))?;
    Ok(json_quote(path))
}

fn rank_config_text(hidden: &[bool; RANK_COUNT], filters: RankFilters) -> String {
    let names: Vec<_> = RANK_ORDER
        .iter()
        .copied()
        .filter(|&metric| !hidden[metric])
        .map(rank_name)
        .collect();
    format!(
        "columns = {}\nhide_magic = {}\nhide_non_thumb = {}\n",
        names.join(" "),
        filters.hide_magic,
        filters.hide_non_thumb,
    )
}

fn rolls_config_text(rolls: RollSettings) -> String {
    format!(
        "include_thumbs = {}\ninclude_scissors = {}\ninclude_stretches = {}\n",
        rolls.include_thumbs, rolls.include_scissors, rolls.include_stretches,
    )
}

fn alts_config_text(weights: &Weights) -> String {
    let rules = weights.1.alts.cloned().unwrap_or_default();
    let mut text = String::new();
    for (family, rules) in [("rowstag", &rules.rowstag), ("colstag", &rules.colstag)] {
        if !rules.is_empty() {
            let rules = crate::alt_fingers::AltRules::text(rules);
            text.push_str(&format!("{family} = {rules}\n"));
        }
    }
    text
}

fn typing_config_text(weights: &Weights) -> String {
    format!("alt_fingers = {}\ncombos = {}\n", weights.3, weights.4)
}

fn adaptive_config_text(adaptive: &crate::swap_fit::Settings) -> String {
    format!(
        "max = {}\nexact = {}\nshortlist = {}\n",
        adaptive.max_swaps, adaptive.exact, adaptive.shortlist
    )
}

fn combos_config_text(combos: &crate::combo_fit::Settings) -> String {
    format!(
        "fatfingering = {}\nmax = {}\nexact = {}\nshortlist = {}\n",
        combos.fatfingering, combos.max_combos, combos.exact, combos.shortlist
    )
}

fn magic_config_text(magic: &MagicSettings) -> String {
    let fingers = FINGER_NAMES
        .iter()
        .enumerate()
        .filter(|(index, _)| magic.allowed_fingers[*index])
        .map(|(_, name)| *name)
        .collect::<Vec<_>>()
        .join(" ");
    let fingers = if fingers.is_empty() { "none" } else { &fingers };
    let keys = magic
        .allowed_keys
        .as_ref()
        .map(|keys| {
            keys.iter()
                .map(|(row, col)| format!("{}:{}", row + 1, col + 1))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_else(|| "all".into());
    format!(
        "allowed_fingers = {fingers}\nallowed_keys = {keys}\nmax_rules = {}\nshortlist = {}\n",
        magic.max_rules, magic.shortlist
    )
}

fn save_spacegram_setting(enabled: bool) -> AppResult<()> {
    let mut limits = load_app_config()?.ngrams;
    limits.include_spacegrams = enabled;
    let cap = |value: Option<usize>| value.map(|v| v.to_string()).unwrap_or_else(|| "all".into());
    let text = format!(
        "include_spacegrams = {}\ntrigrams = {}\ntetragrams = {}\npentagrams = {}\n",
        limits.include_spacegrams,
        cap(limits.trigrams),
        cap(limits.tetragrams),
        cap(limits.pentagrams)
    );
    save_config_sections(&[("ngrams", text)])
}

fn app_config_text(config: &AppConfig) -> String {
    let limit = |value: Option<usize>| value.map(|v| v.to_string()).unwrap_or_else(|| "all".into());
    format!(
        "# akler configuration; see doc/USAGE.md.\n\
         # Missing settings use built-in defaults. Limits other than all are approximate.\n\n\
         [weights]\n{}\n[aklgg]\n{}\n[rolls]\n{}\n[typing]\n{}\n[alts]\n{}\n[search]\n{}\n[magic]\n{}\n[combos]\n{}\n[adaptive]\n{}\n[ranker]\n{}\n\
         [ngrams]\ntrigrams = {}\ntetragrams = {}\npentagrams = {}\ninclude_spacegrams = {}\n",
        weights_text(&config.weights),
        config.weights.2.config_text(),
        rolls_config_text(config.weights.rolls()),
        typing_config_text(&config.weights),
        alts_config_text(&config.weights),
        search_settings_text(&config.search),
        magic_config_text(&config.magic),
        combos_config_text(&config.combos),
        adaptive_config_text(&config.adaptive),
        rank_config_text(&config.rank_columns, config.rank_filters),
        limit(config.ngrams.trigrams),
        limit(config.ngrams.tetragrams),
        limit(config.ngrams.pentagrams),
        config.ngrams.include_spacegrams,
    )
}

// Update one section without discarding comments or touching other settings.
// Keys removed from the section retain their inline comment as a comment line.
fn replace_config_section(text: &str, section: &str, replacement: &str) -> AppResult<String> {
    let entries = if section.eq_ignore_ascii_case("ranker") {
        ranker_config_lines(replacement)?
    } else {
        config_lines(replacement)?
    };
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
        let content = config_content(line).trim();
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
                let comment = config_comment_index(line).map(|index| &line[index..]);
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
    if CONFIG_OVERRIDE.get().is_some() {
        return Err("settings are not saved while --config overrides akler.conf".into());
    }
    let (source, mut text) = match read_active_config()? {
        Some((name, text)) => (name, text),
        None => (APP_CONFIG_FILE, app_config_text(&load_legacy_config()?)),
    };
    let config = parse_app_config(&text).map_err(|error| format!("{source}: {error}"))?;

    // Move the legacy in-config list only when no separate state file exists.
    // Once that file exists it is authoritative, so stale legacy values can
    // never replace newer hidden-row state.
    if read_rank_hidden_layouts()?.is_none() {
        atomic_write(
            Path::new(RANK_HIDDEN_LAYOUTS_FILE),
            &rank_hidden_layouts_text(&config.rank_hidden_layouts)?,
            false,
        )?;
    }

    for (section, replacement) in sections {
        text = replace_config_section(&text, section, replacement)?;
    }
    // Any config save completes migration by removing the obsolete key.
    let updated = parse_app_config(&text).map_err(|error| format!("{APP_CONFIG_FILE}: {error}"))?;
    text = replace_config_section(
        &text,
        "ranker",
        &rank_config_text(&updated.rank_columns, updated.rank_filters),
    )?;
    parse_app_config(&text).map_err(|error| format!("{APP_CONFIG_FILE}: {error}"))?;
    atomic_write(Path::new(APP_CONFIG_FILE), &text, false)
}

fn save_optimizer_settings(weights: &Weights, search: &SearchSettings) -> AppResult<()> {
    validate_search_settings(search)?;
    save_config_sections(&[
        ("weights", weights_text(weights)),
        ("aklgg", weights.2.config_text()),
        ("rolls", rolls_config_text(weights.rolls())),
        ("typing", typing_config_text(weights)),
        ("search", search_settings_text(search)),
    ])
}

fn rank_settings_for_save() -> AppResult<(String, [bool; RANK_COUNT], RankFilters)> {
    let text = read_active_config()?
        .map(|(_, text)| text)
        .unwrap_or_else(|| app_config_text(&AppConfig::default()));
    let sections = config_sections(&text)?;
    let mut columns = default_rank_columns();
    if let Some(body) = sections.get("ranker") {
        for (key, value) in ranker_config_lines(body)? {
            if key == "columns" {
                columns = parse_rank_columns(&value)?;
            }
        }
    }
    let filters = rank_filters_from_text(&text)?;
    Ok((text, columns, filters))
}

fn rank_replacement_for_save(
    text: &str,
    columns: &[bool; RANK_COUNT],
    filters: RankFilters,
) -> AppResult<String> {
    let mut replacement = rank_config_text(columns, filters);
    // Keep legacy hidden rows until a separate state file has been saved.
    if let Some(body) = config_sections(text)?.get("ranker") {
        for (key, value) in ranker_config_lines(body)? {
            if key == "hidden_layouts" {
                replacement.push_str(&format!("hidden_layouts = {value}\n"));
            }
        }
    }
    Ok(replacement)
}

fn save_rank_columns(hidden: &[bool; RANK_COUNT]) -> AppResult<()> {
    let (text, _, filters) = rank_settings_for_save()?;
    let replacement = rank_replacement_for_save(&text, hidden, filters)?;
    let text = replace_config_section(&text, "ranker", &replacement)?;
    atomic_write(Path::new(APP_CONFIG_FILE), &text, false)
}

fn save_rank_filters(filters: RankFilters) -> AppResult<()> {
    let (text, columns, _) = rank_settings_for_save()?;
    let replacement = rank_replacement_for_save(&text, &columns, filters)?;
    let text = replace_config_section(&text, "ranker", &replacement)?;
    atomic_write(Path::new(APP_CONFIG_FILE), &text, false)
}

fn save_rank_selection(
    columns: Option<&[bool; RANK_COUNT]>,
    filters: RankFilters,
    hidden: &BTreeSet<PathBuf>,
) -> AppResult<()> {
    let previous = read_optional_config(Path::new(APP_CONFIG_FILE))?;
    let (text, saved_columns, _) = rank_settings_for_save()?;
    let text = replace_config_section(
        &text,
        "ranker",
        &rank_config_text(columns.unwrap_or(&saved_columns), filters),
    )?;
    let hidden_text = rank_hidden_layouts_text(hidden)?;
    atomic_write(Path::new(APP_CONFIG_FILE), &text, false)?;
    if let Err(error) = atomic_write(Path::new(RANK_HIDDEN_LAYOUTS_FILE), &hidden_text, false) {
        let rollback = match previous {
            Some(text) => atomic_write(Path::new(APP_CONFIG_FILE), &text, false),
            None => fs::remove_file(APP_CONFIG_FILE).map_err(|error| error.into()),
        };
        return match rollback {
            Ok(()) => Err(error),
            Err(rollback) => {
                Err(format!("{error}; configuration rollback failed: {rollback}").into())
            }
        };
    }
    Ok(())
}

fn save_rank_hidden_layouts(hidden_layouts: &BTreeSet<PathBuf>) -> AppResult<()> {
    atomic_write(
        Path::new(RANK_HIDDEN_LAYOUTS_FILE),
        &rank_hidden_layouts_text(hidden_layouts)?,
        false,
    )
}

#[cfg(test)]
mod config_tests {
    use super::*;

    #[test]
    fn unified_defaults_round_trip_without_changing_settings() {
        let defaults = AppConfig::default();
        let parsed = parse_app_config(&app_config_text(&defaults)).unwrap();
        assert_eq!(parsed.weights.0, defaults.weights.0);
        assert_eq!(parsed.weights.2, defaults.weights.2);
        assert_eq!(parsed.weights.3, defaults.weights.3);
        assert_eq!(parsed.combos, defaults.combos);
        assert!(!defaults.combos.fatfingering && !defaults.weights.5);
        let combos =
            parse_app_config("[combos]\nfatfingering = true\nmax = 3\nshortlist = 20\n").unwrap();
        assert_eq!((combos.combos.max_combos, combos.combos.shortlist), (3, 20));
        assert!(combos.combos.fatfingering && combos.weights.5);
        assert_eq!(combos.combos.active_max(&combos.weights), 3);
        // Combo fitting follows [typing] combos, not fat-fingering.
        let plain = parse_app_config("[combos]\nmax = 3\n").unwrap();
        assert_eq!(plain.combos.active_max(&plain.weights), 3);
        let off = parse_app_config("[typing]\ncombos = false\n[combos]\nmax = 3\n").unwrap();
        assert_eq!(off.combos.active_max(&off.weights), 0);
        assert!(parse_app_config("[combos]\nfatfingering = maybe").is_err());
        assert_eq!(parsed.adaptive, defaults.adaptive);
        let adaptive = parse_app_config("[adaptive]\nmax = 6\nshortlist = 8\n").unwrap();
        assert_eq!((adaptive.adaptive.max_swaps, adaptive.adaptive.shortlist), (6, 8));
        assert_eq!(
            parse_app_config(&app_config_text(&adaptive)).unwrap().adaptive,
            adaptive.adaptive
        );
        for text in ["[adaptive]\nmax = 33", "[adaptive]\nunknown = 1"] {
            assert!(parse_app_config(text).is_err(), "accepted {text:?}");
        }
        let saved = parse_app_config(&app_config_text(&combos)).unwrap();
        assert_eq!(saved.combos, combos.combos);
        for text in ["[combos]\nmax = 33", "[combos]\nmax = -1", "[combos]\nunknown = 1"] {
            assert!(parse_app_config(text).is_err(), "accepted {text:?}");
        }
        assert_eq!(
            search_settings_text(&parsed.search),
            search_settings_text(&defaults.search)
        );
        assert_eq!(parsed.rank_columns, defaults.rank_columns);
        assert_eq!(parsed.rank_filters, defaults.rank_filters);
        assert_eq!(parsed.rank_hidden_layouts, defaults.rank_hidden_layouts);
        assert_eq!(parsed.ngrams, defaults.ngrams);
    }

    #[test]
    fn magic_rule_limits_default_parse_and_round_trip() {
        let defaults = AppConfig::default();
        assert_eq!(
            (defaults.magic.max_rules, defaults.magic.shortlist),
            (5, 24)
        );
        let mut config = defaults.clone();
        config.magic.max_rules = 3;
        config.magic.shortlist = 40;
        let parsed = parse_app_config(&app_config_text(&config)).unwrap();
        assert_eq!((parsed.magic.max_rules, parsed.magic.shortlist), (3, 40));
        assert!(parse_app_config("[magic]\nmax_rules = 0\n").is_err());
        assert!(parse_app_config("[magic]\nmax_rules = 69\n").is_err());
        assert!(parse_app_config("[magic]\nshortlist = 0\n").is_err());
    }

    #[test]
    fn legacy_values_survive_config_and_hidden_state_serialization() {
        let mut legacy = AppConfig::default();
        legacy.weights = weights_from_text("fsb = 9\nsfs = 2\n").unwrap();
        legacy.search =
            search_from_text("method = sweep\nseconds = 17\ncorpus.books = 2\n").unwrap();
        legacy.rank_columns = parse_rank_columns("SCORE SFB SKB SKS").unwrap();
        legacy.rank_filters = RankFilters {
            hide_magic: true,
            hide_non_thumb: true,
        };
        legacy.rank_hidden_layouts.extend([
            PathBuf::from("layouts/a b.jsonc"),
            PathBuf::from("layouts/#quoted\\\".dat"),
        ]);

        let parsed = parse_app_config(&app_config_text(&legacy)).unwrap();
        assert_eq!(parsed.weights.0, legacy.weights.0);
        assert_eq!(parsed.weights.0[DAFJB], 9.0);
        assert_eq!(parsed.weights.0[CAFJB], 4.5);
        assert_eq!(
            search_settings_text(&parsed.search),
            search_settings_text(&legacy.search)
        );
        assert_eq!(parsed.rank_columns, legacy.rank_columns);
        assert_eq!(parsed.rank_filters, legacy.rank_filters);
        assert!(parsed.rank_hidden_layouts.is_empty());
        assert!(!app_config_text(&legacy).contains("hidden_layouts"));
        assert_eq!(
            parse_rank_hidden_layouts(
                &rank_hidden_layouts_text(&legacy.rank_hidden_layouts).unwrap()
            )
            .unwrap(),
            legacy.rank_hidden_layouts
        );
    }

    #[test]
    fn unified_config_changes_only_explicit_values() {
        let parsed = parse_app_config(
            "[weights]\nsfb = 7\n[search]\nseconds = 15\ncorpus.reddit = 2\n\
             [ranker]\ncolumns = SCORE SFB\nhide_magic = true\nhide_non_thumb = true\n\
             hidden_layouts = [\"layouts/a b.dat\", \"layouts/c.dat\"]\n[ngrams]\n\
             trigrams = 2000\ntetragrams = 4000\npentagrams = all\n[typing]\nalt_fingers = false\ncombos = false\n",
        )
        .unwrap();
        assert_eq!(parsed.weights.0[SFB], 7.0);
        assert_eq!(parsed.weights.0[SFS], Weights::default().0[SFS]);
        assert_eq!(parsed.search.seconds, 15.0);
        assert_eq!(parsed.search.mix.get("reddit"), Some(&2.0));
        assert_eq!(
            parsed.rank_columns,
            parse_rank_columns("SCORE SFB").unwrap()
        );
        assert_eq!(
            parsed.rank_filters,
            RankFilters {
                hide_magic: true,
                hide_non_thumb: true,
            }
        );
        assert_eq!(
            parsed.rank_hidden_layouts,
            [
                PathBuf::from("layouts/a b.dat"),
                PathBuf::from("layouts/c.dat")
            ]
            .into_iter()
            .collect()
        );
        assert_eq!(parsed.ngrams.trigrams, Some(2000));
        assert_eq!(parsed.ngrams.tetragrams, Some(4000));
        assert_eq!(parsed.ngrams.pentagrams, None);
        assert!(!parsed.weights.3);
        assert!(!parsed.weights.4);
        let restored = parse_app_config(&app_config_text(&parsed)).unwrap();
        assert!(!restored.weights.3 && !restored.weights.4);
        assert_eq!(restored.rank_filters, parsed.rank_filters);

        let isolated = rank_filters_from_text(
            "[weights]\nthis is intentionally invalid\n\
             [ranker]\ncolumns = INVALID\nhide_magic = true\nhide_non_thumb = false\n",
        )
        .unwrap();
        assert_eq!(
            isolated,
            RankFilters {
                hide_magic: true,
                hide_non_thumb: false,
            }
        );
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
            "[ranker]\nhide_magic = maybe",
            "[ranker]\nhide_non_thumb = 1",
            "[ranker]\nhidden_layouts = layouts/a.dat",
            "[ranker]\nhidden_layouts = [\"unterminated]",
            "[ranker]\nhidden_layouts = [\"a\" \"b\"]",
            "[ranker]\nhidden_layouts = [\"bad\\q\"]",
            "[ngrams]\ntrigrams = 0",
            "[ngrams]\ntrigrams = -1",
            "[ngrams]\ntrigrams = 1.5",
            "[ngrams]\nbigrams = 2000",
            "[mana]\nunknown = [1]",
            "[mana]\nsfb = []",
            "[mana]\nsfb = [-4, 0.5]",
            "[mana]\nsfb = [-4, 0, -13]",
            "[mana]\nsfb = [-4, 1, -13, 0.5, -26]",
            "[mana]\nsfb = [-4, inf, -13]",
            "[mana]\nsfb = [-4, 0.5, NaN]",
            "[aklgg]\nunknown = 1",
            "[typing]\nalt_fingers = maybe",
            "[typing]\nunknown = true",
        ] {
            assert!(parse_app_config(text).is_err(), "accepted {text:?}");
        }
    }

    #[test]
    fn aklgg_weights_round_trip_and_removed_sections_are_rejected() {
        for text in ["[mana]\nsfb = -4", "[mana2]\nsfb = -4", "[simple]\nsfb = 12"] {
            assert!(parse_app_config(text).is_err(), "accepted {text}");
        }
        let error = parse_app_config("[mana]\nsfb = -4").unwrap_err().to_string();
        assert!(error.contains("move its weights to [aklgg]"), "{error}");

        let parsed =
            parse_app_config("[aklgg]\nsfb = -19\ninroll3 = 0.7\nlsb = [-2, 0.25, -9]\n")
                .unwrap();
        let sfb = mana2_metrics::STAT_IDS
            .iter()
            .position(|id| *id == "sfb")
            .unwrap();
        let lsb = mana2_metrics::STAT_IDS
            .iter()
            .position(|id| *id == "lsb")
            .unwrap();
        assert_eq!(parsed.weights.2.schedule_text(sfb), "-19");
        assert_eq!(parsed.weights.2.schedule_text(lsb), "[-2, 0.25, -9]");

        let text = app_config_text(&AppConfig::default());
        assert!(!text.contains("[mana]"));
        assert!(text.contains("[aklgg]"));
        assert!(!text.contains("[mana2]"));
        assert!(!text.contains("[simple]"));
        let roundtrip = parse_app_config(&app_config_text(&parsed)).unwrap();
        assert_eq!(roundtrip.weights.2, parsed.weights.2);
        let sections = config_sections(&app_config_text(&parsed)).unwrap();
        let aklgg = sections.get("aklgg").unwrap();
        assert!(aklgg.contains("weighted_speed ="));
        assert!(aklgg.contains("fspeed-LP ="));
        assert!(aklgg.contains("offpinky ="));
    }

    #[test]
    fn saving_section_preserves_comments_and_unrelated_settings() {
        let text = "# user note\n[weights]\nsfb = 7 # tune later\n\n\
                    [search]\n# preserve this\nseconds = 9\nmax_sfb = 0.2 # old name\n\
                    corpus.old = 1 # old mixture note\n\n\
                    [ranker]\ncolumns = SCORE # concise\n\
                    hide_magic = false # keep magic note\n\
                    hide_non_thumb = true # keep thumb note\n\n\
                    [ngrams]\ntrigrams = 2000 # approximate\n";
        let mut search = SearchSettings::default();
        search.seconds = 12.0;
        let updated =
            replace_config_section(text, "search", &search_settings_text(&search)).unwrap();
        let updated = replace_config_section(
            &updated,
            "ranker",
            &rank_config_text(
                &parse_rank_columns("SCORE").unwrap(),
                RankFilters {
                    hide_magic: true,
                    hide_non_thumb: false,
                },
            ),
        )
        .unwrap();
        let parsed = parse_app_config(&updated).unwrap();
        assert_eq!(
            search_settings_text(&parsed.search),
            search_settings_text(&search)
        );
        assert_eq!(parsed.weights.0[SFB], 7.0);
        assert_eq!(parsed.ngrams.trigrams, Some(2000));
        assert_eq!(
            parsed.rank_filters,
            RankFilters {
                hide_magic: true,
                hide_non_thumb: false,
            }
        );
        for unchanged in [
            "# user note",
            "sfb = 7 # tune later",
            "# preserve this",
            "max_sfb = 0 # old name",
            "# old mixture note",
            "columns = SCORE # concise",
            "hide_magic = true # keep magic note",
            "hide_non_thumb = false # keep thumb note",
            "trigrams = 2000 # approximate",
        ] {
            assert!(updated.contains(unchanged), "lost {unchanged:?}");
        }
        assert!(!updated.contains("corpus.old ="));
    }

    #[test]
    fn saving_can_add_missing_sections_and_keep_aliases() {
        let original = "[search]\nmetrics = detailed # legacy alias\n";
        let with_ranker = replace_config_section(
            original,
            "ranker",
            &rank_config_text(
                &parse_rank_columns("SFB SFS").unwrap(),
                RankFilters::default(),
            ),
        )
        .unwrap();
        let updated = replace_config_section(
            &with_ranker,
            "search",
            &search_settings_text(&SearchSettings::default()),
        )
        .unwrap();
        let parsed = parse_app_config(&updated).unwrap();
        assert_eq!(parsed.search.mode, "normal");
        assert!(updated.contains("metrics = normal # legacy alias"));
        assert_eq!(parsed.rank_columns, parse_rank_columns("SFB SFS").unwrap());
        assert_eq!(parsed.rank_filters, RankFilters::default());
        assert!(updated.contains("hide_magic = false"));
        assert!(updated.contains("hide_non_thumb = false"));
    }

    #[test]
    fn saving_weights_preserves_legacy_scissor_cost_and_comment() {
        let original = "[weights]\nfsb = 9 # old scissors weight\nsfb = 6\n";
        let before = parse_app_config(original).unwrap();
        let updated =
            replace_config_section(original, "weights", &weights_text(&before.weights)).unwrap();
        let after = parse_app_config(&updated).unwrap();
        assert_eq!(before.weights.0, after.weights.0);
        assert!(updated.contains("# old scissors weight"));
        assert!(
            !config_lines(&config_sections(&updated).unwrap()["weights"])
                .unwrap()
                .iter()
                .any(|(key, _)| key == "fsb")
        );
    }
}

#[cfg(test)]
mod roll_config_tests {
    use super::*;

    #[test]
    fn spacegram_option_defaults_true_and_round_trips() {
        assert!(parse_app_config("").unwrap().ngrams.include_spacegrams);
        let parsed = parse_app_config("[ngrams]\ninclude_spacegrams = false\n").unwrap();
        assert!(!parsed.ngrams.include_spacegrams);
        assert!(
            !parse_app_config(&app_config_text(&parsed))
                .unwrap()
                .ngrams
                .include_spacegrams
        );
        assert!(
            parse_app_config("[ngrams]\ninclude_spacegrams = true\n")
                .unwrap()
                .ngrams
                .include_spacegrams
        );
    }

    #[test]
    fn roll_options_round_trip_and_preserve_defaults_and_aliases() {
        assert_eq!(
            parse_app_config("").unwrap().weights.rolls(),
            RollSettings::default()
        );
        for include_thumbs in [false, true] {
            for include_scissors in [false, true] {
                for include_stretches in [false, true] {
                    let rolls = RollSettings {
                        include_thumbs,
                        include_scissors,
                        include_stretches,
                        alts: None,
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
        assert_eq!(
            parse_app_config(&updated).unwrap().weights.rolls(),
            RollSettings::default()
        );
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
            assert!(
                parse_app_config(&format!("[rolls]\n{body}\n")).is_err(),
                "{body}"
            );
        }
    }
}
