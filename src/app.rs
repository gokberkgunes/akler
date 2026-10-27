fn emit_evaluation(board: Board, source: Source) -> AppResult<()> {
    let weights = load_weights(Path::new(WEIGHTS_FILE))?;
    let model = Model::with_rolls(board, weights.rolls());
    let corpus = model.corpus(&source)?;
    let raw = full_raw_with_mana2(&model.original, &corpus, &model.geometry, true);
    let m = metrics(&raw, &corpus);
    let b = breakdown(&m, &weights);
    let mana = mana_stats_for_corpus(&raw, &corpus);
    let stdout = io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    writeln!(
        out,
        "{{\n  \"Model\": {},\n  \"Layout\": {},\n  \"Corpus\": {},\n  \"Metrics\": {{",
        json_quote(MODEL_VERSION),
        json_quote(&model.board.name),
        json_quote(&corpus.name)
    )?;
    for (i, name) in METRIC_NAMES.iter().enumerate() {
        writeln!(
            out,
            "    {}: {:.12}{}",
            json_quote(name),
            m.v[i],
            if i + 1 == N_METRICS { "" } else { "," }
        )?;
    }
    writeln!(out, "  }},\n  \"Usage\": {:?},\n  \"Off\": {:?},\n  \"Totals\": {:?},\n  \"Coverage\": {:?},\n  \"NoThumbPair\": {},\n  \"NoThumbTriple\": {},\n  \"Score\": {{\"Penalties\": {}, \"Rewards\": {}, \"Score\": {}}},\n  \"Warnings\": [", m.usage, m.off, corpus.totals, corpus.coverage, raw.0[SRAF_DEN], raw.0[RHYTHM_DEN], b.penalty, b.bonus, b.net)?;
    for (i, line) in corpus.warnings.iter().enumerate() {
        writeln!(
            out,
            "    {}{}",
            json_quote(line),
            if i + 1 == corpus.warnings.len() {
                ""
            } else {
                ","
            }
        )?;
    }
    // Mana2 counters in akl.gg units: percentages, ratings and Fspeed values.
    writeln!(out, "  ],\n  \"Mana\": {{")?;
    for (i, name) in mana2_metrics::STAT_IDS.iter().enumerate() {
        writeln!(
            out,
            "    {}: {:.12}{}",
            json_quote(name),
            mana.values[i],
            if i + 1 == mana2_metrics::N_STATS { "" } else { "," }
        )?;
    }
    writeln!(out, "  }}\n}}")?;
    out.flush()?;
    Ok(())
}

fn corpus_order_name(order: &str) -> AppResult<(usize, Vec<&'static str>)> {
    match order {
        "1" => Ok((1, vec!["letters", "monograms", "unigrams"])),
        "2" => Ok((2, vec!["bigrams"])),
        "3" => Ok((3, vec!["trigrams"])),
        "4" => Ok((
            4,
            vec!["fourgrams", "quadrigrams", "quadgrams", "tetragrams"],
        )),
        "5" => Ok((5, vec!["fivegrams"])),
        "skip" => Ok((2, vec!["skipgrams"])),
        _ => Err("order must be 1, 2, 3, 4, 5 or skip".into()),
    }
}

fn top_ngrams(path: &Path, order: &str, limit: usize) -> AppResult<Vec<(String, f64)>> {
    let path = if corpus_is_raw(path)? {
        let needed = order.parse::<usize>().unwrap_or(3).max(DEFAULT_ORDER);
        ensure_corpus(path, false, Some(needed))?
    } else {
        path.to_owned()
    };
    let (width, names) = corpus_order_name(order)?;
    let text = fs::read_to_string(&path)?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let mut p = JsonParser { text, p: 0 };
    p.ws();
    p.expect(b'{')?;
    let mut table = None;
    loop {
        p.ws();
        if p.byte() == Some(b'}') {
            break;
        }
        let key = p.string()?;
        p.ws();
        p.expect(b':')?;
        if names.contains(&key.as_str()) {
            table = Some(read_frequency_table(&mut p, width, true)?);
        } else if let Ok((w, _)) = corpus_order_name(match key.as_str() {
            "letters" | "monograms" | "unigrams" => "1",
            "bigrams" => "2",
            "trigrams" => "3",
            "fourgrams" | "quadgrams" | "quadrigrams" | "tetragrams" => "4",
            "fivegrams" => "5",
            "skipgrams" => "skip",
            _ => "?",
        }) {
            read_frequency_table(&mut p, w, false)?;
        } else {
            p.value(0)?;
        }
        p.ws();
        if p.byte() == Some(b'}') {
            break;
        }
        p.expect(b',')?;
    }
    let mut rows = table.ok_or_else(|| {
        format!(
            "{order}-gram table is absent; rebuild from raw text with --order {}",
            width.max(3)
        )
    })?;
    rows.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    rows.truncate(limit);
    Ok(rows)
}

fn corpus_info(path: &Path) -> AppResult<Vec<String>> {
    let path = if corpus_is_raw(path)? {
        ensure_corpus(path, false, None)?
    } else {
        path.to_owned()
    };
    // Corpus inspection describes the stored cache, not an evaluation subset.
    let source = Source::from_text(&fs::read_to_string(&path)?, &path)?;
    let mut lines = vec![format!("{}  {}", source.name, path.display())];
    for (i, name) in ["letters", "bigrams", "skipgrams", "trigrams"]
        .iter()
        .enumerate()
    {
        lines.push(format!(
            "{name:<12} {:>9} rows   {:.0} events",
            source.tables[i].len(),
            source.masses[i]
        ));
    }
    let mut line = String::new();
    BufReader::new(File::open(&path)?)
        .take(8192)
        .read_line(&mut line)?;
    let prefix = line.trim().trim_end_matches(',');
    if let Ok(Json::Object(root)) = parse_json(&format!("{prefix}}}")) {
        if let Some(Json::Object(meta)) = root.get("source") {
            if let Some(Json::Number(n)) = meta.get("max_order") {
                lines.push(format!("maximum order  {n}"));
            }
            if let Some(Json::Array(rows)) = meta.get("rows") {
                for (i, name) in ["fourgrams", "fivegrams"].iter().enumerate() {
                    if let Some(Json::Number(n)) = rows.get(i + 3) {
                        if *n > 0.0 {
                            lines.push(format!("{name:<12} {:>9} rows", *n as u64));
                        }
                    }
                }
            }
        }
    }
    lines.extend(source.warnings);
    Ok(lines)
}

fn corpus_command(args: &[String]) -> AppResult<()> {
    corpus_dirs()?;
    let action = args.first().map(String::as_str).unwrap_or("list");
    match action {
        "list" => {
            for p in corpus_paths()? {
                println!("{:<24} {}", corpus_name(&p), p.display());
            }
        }
        "add" => {
            if args.len() < 3 || args.len() > 4 {
                return Err("corpus add NAME INPUT [CONFIG.json]".into());
            }
            let path = add_corpus(&args[1], Path::new(&args[2]), args.get(3).map(Path::new))?;
            println!("{}", path.display());
        }
        "build" => {
            let mut name = None;
            let mut order = None;
            let mut jobs = None;
            let mut i = 1;
            while i < args.len() {
                match args[i].as_str() {
                    "--order" => {
                        i += 1;
                        let n: usize = args.get(i).ok_or("--order needs a value")?.parse()?;
                        if !(3..=5).contains(&n) {
                            return Err("--order must be 3..5".into());
                        }
                        order = Some(n);
                    }
                    "--jobs" => {
                        i += 1;
                        let n = bounded_usize(args.get(i).ok_or("--jobs needs a value")?, 16)?;
                        jobs = Some(n);
                    }
                    value if !value.starts_with('-') && name.is_none() => {
                        name = Some(value.to_owned())
                    }
                    _ => return Err("corpus build [NAME] [--order 3|4|5] [--jobs N]".into()),
                }
                i += 1;
            }
            let mut paths = Vec::new();
            if let Some(n) = name {
                let p = corpus_by_name(&n)?;
                paths.push(raw_corpus_for(&p)?.ok_or(
                    "raw text is needed to rebuild n-grams; existing JSON remains usable",
                )?);
            } else {
                paths = raw_corpus_paths()?;
            }
            for p in paths {
                let (mut cfg, hash) = corpus_config(&corpus_config_path(&p))?;
                if let Some(n) = order {
                    cfg.order = n;
                }
                if let Some(n) = jobs {
                    cfg.jobs = n;
                }
                let start = Instant::now();
                let name = corpus_name(&p);
                let mut last = Instant::now() - Duration::from_secs(1);
                let output = rebuild_corpus(&p, &cfg, hash, &mut |n, total| {
                    if last.elapsed() > Duration::from_millis(250) {
                        eprint!("\r{name} {:3.0}%", pct(n as f64, total as f64));
                        last = Instant::now();
                    }
                })?;
                eprint!("\r\x1b[2K");
                println!(
                    "{}  {:.2}s",
                    output.display(),
                    start.elapsed().as_secs_f64()
                );
            }
        }
        "info" => {
            let path = corpus_by_name(args.get(1).ok_or("corpus info NAME")?)?;
            for line in corpus_info(&path)? {
                println!("{line}");
            }
        }
        "top" => {
            if args.len() < 2 || args.len() > 4 {
                return Err("corpus top NAME [1|2|3|4|5|skip] [COUNT]".into());
            }
            let path = corpus_by_name(&args[1])?;
            let order = args.get(2).map(String::as_str).unwrap_or("4");
            let n = if let Some(s) = args.get(3) {
                bounded_usize(s, 1_000_000)?
            } else {
                20
            };
            for (gram, count) in top_ngrams(&path, order, n)? {
                println!("{}\t{count}", json_quote(&gram));
            }
        }
        _ => return Err("corpus: list, add, build, info or top".into()),
    }
    Ok(())
}

fn select_source_path(term: &mut Terminal, default: bool) -> AppResult<Option<PathBuf>> {
    let paths = corpus_paths()?;
    let path = if default {
        default_corpus_path()?
    } else {
        None
    };
    let path = match path {
        Some(p) => Some(p),
        None => {
            if paths.is_empty() {
                info_page(term, "No corpus found", &empty_corpus_help())?;
                return Ok(None);
            }
            let names = paths.iter().map(|p| corpus_name(p)).collect::<Vec<_>>();
            menu(term, "Select corpus", &names)?.map(|i| paths[i].clone())
        }
    };
    Ok(path)
}

fn raw_corpus_for(path: &Path) -> AppResult<Option<PathBuf>> {
    if corpus_is_raw(path)? {
        return Ok(Some(path.to_owned()));
    }

    if let Some(raw) = raw_corpus_paths()?
        .into_iter()
        .find(|raw| corpus_name(raw).eq_ignore_ascii_case(&corpus_name(path)))
    {
        return Ok(Some(raw));
    }
    let nearby = path.with_extension("txt");
    Ok(nearby.is_file().then_some(nearby))
}

fn corpus_detail_tui(term: &mut Terminal, path: &Path) -> AppResult<()> {
    let raw = raw_corpus_for(path)?;
    let mut status = String::new();
    let mut scroll = 0;
    let mut source = if let Some(raw) = &raw {
        let (_, hash) = corpus_config(&corpus_config_path(raw))?;
        let cache = cached_path(raw);
        if cache_current_at_least(raw, &cache, hash, 3)? {
            Some(Source::load(&cache)?)
        } else {
            None
        }
    } else {
        Some(Source::load(path)?)
    };
    loop {
        let name = source
            .as_ref()
            .map(|s| s.name.clone())
            .unwrap_or_else(|| corpus_name(path));
        let mut lines = if let Some(source) = &source {
            corpus_info(&source.path)?
        } else {
            vec!["No current cache. Choose the largest n-gram width to store.".into()]
        };
        if let Some(raw) = &raw {
            lines.insert(0, format!("raw          {}", raw.display()));
        } else {
            lines.push("Rebuild unavailable: no matching raw text file.".into());
        }
        let lines = wrap_lines(&lines, term.width());
        let mut c = Canvas::new(term.width(), lines.len() + 6);
        header(
            &mut c,
            "Corpora",
            &name,
            "",
            if raw.is_some() {
                "3 trigrams | 4 fourgrams | 5 fivegrams | q back"
            } else {
                "q back"
            },
        );
        for (i, line) in lines.iter().enumerate() {
            c.text(
                0,
                i + 3,
                line,
                if line.starts_with("Rebuild unavailable") {
                    YELLOW
                } else {
                    FG
                },
            );
        }
        c.text(0, lines.len() + 4, &short(&status, c.w), CYAN);
        c.h = lines.len() + 6;
        term.present(&c, scroll)?;
        let e = term.event()?;
        if scroll_event(&e, &mut scroll, c.h, term.size.1) {
            continue;
        }
        match e {
            Event::Escape | Event::Quit | Event::Char('q') => return Ok(()),
            Event::Char(ch @ ('3' | '4' | '5')) => {
                if let Some(raw) = &raw {
                    let order = ch.to_digit(10).unwrap() as usize;
                    let (mut cfg, hash) = corpus_config(&corpus_config_path(raw))?;
                    cfg.order = order;
                    match rebuild_source_tui(term, raw, cfg, hash) {
                        Ok(cache) => match Source::load(&cache) {
                            Ok(next) => {
                                source = Some(next);
                                status = format!("rebuilt through {order}-grams");
                            }
                            Err(e) => status = e.to_string(),
                        },
                        Err(e) => status = e.to_string(),
                    }
                    scroll = 0;
                } else {
                    status = "raw text is required to rebuild".into();
                }
            }
            _ => {}
        }
    }
}

fn empty_corpus_help() -> Vec<String> {
    vec![
        "Editor, Ranker, and Optimizer need a corpus to calculate metrics.".into(),
        "Open Corpora from the main menu and choose Import text corpus.".into(),
        "Or place a text file in corpus/raw/; any filename or extension is accepted.".into(),
        "From the command line: akler corpus add NAME /path/to/text".into(),
    ]
}

fn corpus_menu_items(paths: &[PathBuf]) -> Vec<String> {
    let mut names: Vec<_> = paths.iter().map(|path| corpus_name(path)).collect();
    names.push("Import text corpus...".into());
    names
}

fn import_corpus_tui(term: &mut Terminal) -> AppResult<()> {
    let Some(input) = input_box(
        term,
        "Import corpus: text file path (any extension)",
        "",
        "",
    )?
    else {
        return Ok(());
    };
    let input = PathBuf::from(input);
    if !input.is_file() {
        return Err("choose an existing text file".into());
    }

    let suggested = corpus_name(&input);
    let Some(name) = input_box(term, "Import corpus: name", "", &suggested)? else {
        return Ok(());
    };
    let raw = import_corpus_text(&name, &input, None)?;
    let (config, hash) = corpus_config(&corpus_config_path(&raw))?;
    rebuild_source_tui(term, &raw, config, hash)?;
    corpus_detail_tui(term, &raw)
}

fn corpus_tui(term: &mut Terminal) -> AppResult<()> {
    while !term.quitting {
        let paths = corpus_paths()?;
        let names = corpus_menu_items(&paths);
        let title = if paths.is_empty() {
            "Corpora — no corpus found"
        } else {
            "Corpora"
        };
        let i = match menu(term, title, &names)? {
            Some(i) => i,
            None => return Ok(()),
        };

        let result = if i == paths.len() {
            import_corpus_tui(term)
        } else {
            corpus_detail_tui(term, &paths[i])
        };
        if let Err(error) = result {
            if !term.quitting {
                info_page(term, "Corpus error", &[error.to_string()])?;
            }
        }
    }
    Ok(())
}

#[derive(Default)]
struct HomeState {
    tab: usize,
    selected: [usize; 3],
}

const HOME_TABS: [&str; 3] = ["1 Workbench", "2 Analysis", "3 Data"];

const HOME_ITEMS: [&[(usize, &str, &str)]; 2] = [
    &[
        (0, "Editor", "Inspect and edit one layout"),
        (1, "Ranker", "Compare layouts on a corpus"),
        (2, "Optimizer", "Search for a better layout"),
    ],
    &[
        (5, "Weakness report", "Find the layout's largest problems"),
        (6, "Magic designer", "Design and compare magic keys"),
        (8, "Scissor test", "Type words and rate scissor pairs"),
    ],
];

const HOME_DATA_ITEMS: [(usize, &str, &str); 1] = [
    (3, "Corpora", "Inspect, import, and rebuild corpora"),
];

fn home_items(tab: usize) -> &'static [(usize, &'static str, &'static str)] {
    match tab {
        0 => HOME_ITEMS[0],
        1 => HOME_ITEMS[1],
        _ => &HOME_DATA_ITEMS,
    }
}

/// The package version and the git commit this binary was built from, with
/// "+modified" when src/ differed from it (see build.rs).
pub(crate) const BUILD: &str = env!("AKLER_BUILD");

// A browser page has nowhere to return to, so its home menu stays open.
#[cfg(not(target_os = "wasi"))]
const HOME_FOOTER: &str = "? Help   q Quit";
#[cfg(target_os = "wasi")]
const HOME_FOOTER: &str = "? Help";

fn home_menu(term: &mut Terminal, state: &mut HomeState) -> AppResult<Option<usize>> {
    loop {
        term.refresh_size()?;
        state.tab = state.tab.min(HOME_TABS.len() - 1);
        let items = home_items(state.tab);
        state.selected[state.tab] = state.selected[state.tab].min(items.len() - 1);

        // Home has no top bar, so it gets the whole screen.
        let height = term.screen.1.max(1);
        let mut canvas = Canvas::new(term.width(), height);
        let tabs: Vec<String> = HOME_TABS.iter().map(|label| label.to_string()).collect();
        tab_boxes(&mut canvas, 0, 0, state.tab, &tabs);
        // The items as one box of rows: name, then description.
        let first_row = 4;
        let footer = height.saturating_sub(1);
        let capacity = footer.saturating_sub(first_row + 1).div_ceil(2).max(1);
        let start = state.selected[state.tab].saturating_sub(capacity - 1);
        let shown: Vec<_> = items.iter().skip(start).take(capacity).collect();
        let name_w = shown.iter().map(|item| item.1.chars().count()).max().unwrap_or(0) + 2;
        let text_w = shown
            .iter()
            .map(|item| item.2.chars().count())
            .max()
            .unwrap_or(0)
            .min(canvas.w.saturating_sub(name_w + 5))
            + 2;
        let rule = |left: &str, middle: &str, right: &str| {
            format!("{left}{}{middle}{}{right}", "─".repeat(name_w), "─".repeat(text_w))
        };
        for (row, (action, name, description)) in shown.iter().enumerate() {
            let y = first_row + 2 * row;
            if y + 2 > footer {
                break;
            }
            let edge = if row == 0 { rule("┌", "┬", "┐") } else { rule("├", "┼", "┤") };
            canvas.text(0, y, &edge, BORDER);
            let selected = row + start == state.selected[state.tab];
            canvas.text(0, y + 1, "│", BORDER);
            canvas.text(2, y + 1, name, if selected { FG } else { MUTED });
            canvas.text(name_w + 1, y + 1, "│", BORDER);
            canvas.text(name_w + 3, y + 1, &short(description, text_w - 2), MUTED);
            canvas.text(name_w + text_w + 2, y + 1, "│", BORDER);
            if selected {
                let row = Rect { x: 1, y: y + 1, w: name_w, h: 1 };
                canvas.row_style(row, Some((58, 58, 58)), true);
                canvas.row_style(Rect { x: name_w + 2, w: text_w, ..row }, Some((58, 58, 58)), false);
            }
            canvas.hit(
                Rect {
                    x: 0,
                    y,
                    w: name_w + text_w + 3,
                    h: 2,
                },
                Action::Item(*action),
            );
            if row + 1 == shown.len() || y + 4 > footer {
                canvas.text(0, y + 2, &rule("└", "┴", "┘"), BORDER);
            }
        }

        if height > 1 {
            canvas.text(2, footer, HOME_FOOTER, MUTED);
            let build = format!("akler {BUILD}");
            let x = canvas.w.saturating_sub(build.chars().count() + 2);
            if x > HOME_FOOTER.len() + 4 {
                canvas.text(x, footer, &build, MUTED);
            }
        }
        // Home is the top level: nothing to go back to.
        term.show_back = false;
        term.present(&canvas, 0)?;
        term.show_back = true;

        let mut event = term.event()?;
        if let Some(action) = action_press(term, &canvas, &event, 0) {
            match action {
                Action::Command(ch) => event = Event::Char(ch),
                Action::Item(action) => return Ok(Some(action)),
                _ => {}
            }
        }
        match event {
            Event::Char(ch @ ('1'..='3')) => state.tab = (ch as usize) - ('1' as usize),
            Event::Left | Event::Char('h') => {
                state.tab = state.tab.checked_sub(1).unwrap_or(HOME_TABS.len() - 1)
            }
            Event::Right | Event::Char('l') => state.tab = (state.tab + 1) % HOME_TABS.len(),
            Event::Up | Event::Char('k') => {
                let selected = &mut state.selected[state.tab];
                *selected = selected.checked_sub(1).unwrap_or(items.len() - 1);
            }
            Event::Down | Event::Char('j') => {
                let selected = &mut state.selected[state.tab];
                *selected = (*selected + 1) % items.len();
            }
            Event::Enter => return Ok(Some(items[state.selected[state.tab]].0)),
            Event::Char('?') => help_popup(
                term,
                &canvas,
                "Home",
                &[
                    "1 2 3\tSwitch section".into(),
                    "h l / ← →\tSwitch section".into(),
                    "j k / ↑ ↓\tSelect".into(),
                    "Enter\tOpen".into(),
                    #[cfg(not(target_os = "wasi"))]
                    "q\tQuit".into(),
                ],
            )?,
            #[cfg(not(target_os = "wasi"))]
            Event::Escape | Event::Quit | Event::Char('q') => return Ok(None),
            _ => {}
        }
    }
}

// Layout discovery inspects contents so uploaded files do not need a suffix.
// Invalid but recognizable layouts remain visible and report their parse errors
// when opened; corpus caches and optimizer reports do not become chooser rows.
fn recognizable_layout(text: &str) -> bool {
    if crate::layout_io::is_json_layout(text) {
        let fields = ["layout", "fingermap", "magic", "skip"];
        return match crate::layout_io::parse_jsonc(text) {
            Ok(Json::Object(object)) => fields.iter().any(|field| object.contains_key(*field)),
            Ok(_) => false,
            Err(_) => fields.iter().any(|field| {
                let quoted = format!("\"{field}\"");
                text.match_indices(&quoted)
                    .any(|(index, _)| text[index + quoted.len()..].trim_start().starts_with(':'))
            }),
        };
    }

    let first = text
        .trim_start_matches('\u{feff}')
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    let tokens: Vec<_> = first.split_whitespace().collect();
    let key_tokens = tokens
        .iter()
        .filter(|token| {
            token.chars().count() == 1
                || matches!(**token, "space" | "blank" | "empty" | "none")
                || token.starts_with('@')
                || token.starts_with("char:")
        })
        .count();
    (8..=13).contains(&tokens.len()) && key_tokens >= 8
}

fn same_discovered_layout(a: &action_keys::Layout, b: &action_keys::Layout) -> bool {
    a.slots == b.slots
        && a.actions == b.actions
        && a.left_outer == b.left_outer
        && a.right_outer == b.right_outer
        && a.extended() == b.extended()
}

fn layout_file_label(path: &Path) -> String {
    let label = path
        .strip_prefix(Path::new(LAYOUT_DIR))
        .unwrap_or_else(|_| Path::new(path.file_name().unwrap_or_default()));
    clean_text(&label.to_string_lossy())
}

fn layout_companion_priority(path: &Path) -> Option<u8> {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("dat") => Some(0),
        Some("json") => Some(1),
        Some("jsonc") => Some(2),
        _ => None,
    }
}

fn internal_layout_artifact(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name.starts_with(".akler-")
        || name.starts_with(".layouter-")
        || name
            .strip_suffix(".bak")
            .and_then(|name| name.rsplit_once('.'))
            .is_some_and(|(original, stamp)| {
                !original.is_empty()
                    && stamp.len() >= 16
                    && stamp.bytes().all(|byte| byte.is_ascii_digit())
            })
}

fn discover_layouts(directory: &Path) -> AppResult<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in
        fs::read_dir(directory).map_err(|error| format!("{}: {error}", directory.display()))?
    {
        paths.push(entry?.path());
    }
    let optimized = directory.join("optimized");
    if optimized.is_dir() {
        for entry in
            fs::read_dir(&optimized).map_err(|error| format!("{}: {error}", optimized.display()))?
        {
            paths.push(entry?.path());
        }
    }

    let mut candidates = Vec::new();
    for path in paths {
        if !path.is_file() || internal_layout_artifact(&path) {
            continue;
        }
        let bytes = fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        let layout = action_keys::Layout::parse(&text, &path).ok();
        if layout.is_some() || recognizable_layout(&text) {
            candidates.push((path, layout));
        }
    }
    // Generated companions share a stem. Prefer DAT only after confirming that
    // both files parse to the same complete layout; independent edits stay visible.
    candidates.sort_by(|(a, _), (b, _)| {
        layout_companion_priority(a)
            .unwrap_or(3)
            .cmp(&layout_companion_priority(b).unwrap_or(3))
            .then_with(|| a.cmp(b))
    });
    let mut layouts: Vec<(PathBuf, Option<action_keys::Layout>)> = Vec::new();
    for (path, layout) in candidates {
        let duplicate = layout.as_ref().is_some_and(|layout| {
            layout_companion_priority(&path).is_some()
                && layouts.iter().any(|(other_path, other)| {
                    path.file_stem() == other_path.file_stem()
                        && path.parent() == other_path.parent()
                        && path.extension() != other_path.extension()
                        && layout_companion_priority(other_path).is_some()
                        && other
                            .as_ref()
                            .is_some_and(|other| same_discovered_layout(layout, other))
                })
        });
        if !duplicate {
            layouts.push((path, layout));
        }
    }
    let mut paths: Vec<_> = layouts.into_iter().map(|(path, _)| path).collect();
    paths.sort();
    if paths.is_empty() {
        return Err(format!(
            "no DAT, JSON, or JSONC layout contents in {}",
            directory.display()
        )
        .into());
    }
    Ok(paths)
}

enum OpenLayout {
    Plain(Board),
    Action(crate::action_keys::Layout),
}

fn parse_open_layout(text: &str, path: &Path) -> AppResult<OpenLayout> {
    match crate::action_keys::Layout::parse(text, path) {
        Ok(layout) if layout.extended() => Ok(OpenLayout::Action(layout)),
        Ok(_) => board_from_text(text, path).map(OpenLayout::Plain),
        Err(error) => board_from_text(text, path)
            .map(OpenLayout::Plain)
            .map_err(|_| format!("action layout: {error}").into()),
    }
}

fn tui_mode(
    term: &mut Terminal,
    mode: &str,
    board: Option<&Path>,
    corpus: Option<&str>,
) -> AppResult<()> {
    let corpus_path = if let Some(name) = corpus {
        corpus_by_name(name)?
    } else {
        match select_source_path(term, true)? {
            Some(path) => path,
            None => return Ok(()),
        }
    };
    if mode == "ranker" {
        return ranking(term, &corpus_path);
    }

    // One selected-corpus snapshot lives until this layout chooser is left.
    // Switching layout geometry/actions never invalidates corpus-only tables.
    let mut session = crate::session::CorpusSession::new();
    loop {
        if term.quitting {
            return Ok(());
        }
        let path = if let Some(path) = board {
            path.to_owned()
        } else {
            let paths = discover_layouts(Path::new(LAYOUT_DIR))?;
            let mut names: Vec<_> = paths.iter().map(|path| layout_file_label(path)).collect();
            names.push("Reload corpus snapshot".into());
            match search_menu(term, "Layouts", &names)? {
                Some(index) if index == paths.len() => {
                    session.invalidate();
                    continue;
                }
                Some(index) => paths[index].clone(),
                None => return Ok(()),
            }
        };

        let result = (|| -> AppResult<()> {
            let mut timing = crate::load_profile::LoadProfile::new("layout open dispatch");
            let text = fs::read_to_string(&path)?;
            let layout = parse_open_layout(&text, &path)?;
            timing.mark("Read layout once and detect evaluator");
            drop(timing);

            match layout {
                OpenLayout::Action(layout) => {
                    if let Some(corpus) = session.action_tui(term, &corpus_path)? {
                        crate::action_ui::action_editor(term, layout, corpus, mode == "optimizer")?;
                    }
                }
                OpenLayout::Plain(layout) => {
                    if let Some(source) = session.plain_tui(term, &corpus_path)? {
                        if mode == "optimizer" {
                            optimizer(term, layout, &source)?;
                        } else {
                            editor(term, layout, &source)?;
                        }
                    }
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            if !term.quitting {
                info_page(term, "Error", &[error.to_string()])?;
            }
        }
        if board.is_some() {
            return Ok(());
        }
    }
}

/// Gives a file the permissions of the one it replaces. WASI has no
/// permissions to set (the browser's file system answers "not implemented").
pub(crate) fn copy_permissions(file: &File, permissions: fs::Permissions) -> io::Result<()> {
    #[cfg(target_os = "wasi")]
    return {
        let _ = (file, permissions);
        Ok(())
    };
    #[cfg(not(target_os = "wasi"))]
    file.set_permissions(permissions)
}

/// Flushes a file to disk; the browser's file system lives in memory.
pub(crate) fn sync_file(file: &File) -> io::Result<()> {
    #[cfg(target_os = "wasi")]
    return {
        let _ = file;
        Ok(())
    };
    #[cfg(not(target_os = "wasi"))]
    file.sync_all()
}

/// The process id, for temporary file names. WASI has no process ids and
/// panics when asked, so the browser build uses 0; it runs alone anyway.
pub(crate) fn process_tag() -> u32 {
    #[cfg(target_os = "wasi")]
    return 0;
    #[cfg(not(target_os = "wasi"))]
    std::process::id()
}

/// Threads to use by default. WASI reports one, so web hosts pass their
/// core count as `AKLER_THREADS`.
fn available_threads() -> usize {
    std::env::var("AKLER_THREADS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|&threads| threads > 0)
        .unwrap_or_else(|| thread::available_parallelism().map_or(1, |threads| threads.get()))
}

fn run() -> AppResult<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(cmd) = args.first() {
        match cmd.as_str() {
            "--version" | "-V" | "version" => {
                println!("akler {BUILD}");
                return Ok(());
            }
            "--help" | "-h" | "help" => {
                println!("akler {BUILD}\n\nakler [editor | ranker | optimizer]\nakler editor|optimizer [LAYOUT] [CORPUS]\nakler ranker [CORPUS]\nakler eval LAYOUT [CORPUS]\nakler bench LAYOUT CORPUS [--seeds N] [--set key=value]... [--full-ngrams] [--rescore-full] [--unlocked] [--show] [--swaps N] [--combos K] [--config FILE]\nakler bench --ranker CORPUS\nakler corpus list|info NAME|add NAME INPUT [CONFIG.json]\nakler corpus build [NAME] [--order 3|4|5] [--jobs N]\nakler corpus top NAME [ORDER] [COUNT]\n\nDefault corpus: corpus-reddit.json. Layouts: layouts/ (DAT, JSON, or JSONC; filenames need no extension). Raw text: corpus/raw/ (any extension or none). Import text from the Corpora menu.");
                return Ok(());
            }
            "corpus" => return corpus_command(&args[1..]),
            "scissor-words" => return scissor_words_command(),
            "bench" => return crate::bench::command(&args[1..]),
            "import" => {
                let mut rest = vec!["add".to_string()];
                rest.extend_from_slice(&args[1..]);
                return corpus_command(&rest);
            }
            "eval" => {
                if args.len() < 2 || args.len() > 3 {
                    return Err("eval LAYOUT [CORPUS]".into());
                }
                let corpus = if let Some(name) = args.get(2) {
                    corpus_by_name(name)?
                } else {
                    default_corpus_path()?
                        .ok_or("corpus-reddit.json not found; provide a corpus path")?
                };
                return emit_evaluation(load_board(Path::new(&args[1]))?, Source::load(&corpus)?);
            }
            "editor" | "edit" | "optimizer" | "optimize" | "ranker" | "rank" => {
                let mode = match cmd.as_str() {
                    "edit" => "editor",
                    "optimize" => "optimizer",
                    "rank" => "ranker",
                    s => s,
                };
                if args.len() > if mode == "ranker" { 2 } else { 3 } {
                    return Err("too many arguments".into());
                }
                let mut term = Terminal::open()?;
                let result = if mode == "ranker" {
                    tui_mode(&mut term, mode, None, args.get(1).map(String::as_str))
                } else {
                    tui_mode(
                        &mut term,
                        mode,
                        args.get(1).map(Path::new),
                        args.get(2).map(String::as_str),
                    )
                };
                drop(term);
                return result;
            }
            _ => return Err(format!("unknown mode {cmd}; use akler --help").into()),
        }
    }
    let mut term = Terminal::open()?;
    let mut home = HomeState::default();
    while !term.quitting {
        let choice = match home_menu(&mut term, &mut home)? {
            Some(i) => i,
            None => break,
        };
        let result = match choice {
            3 => corpus_tui(&mut term),
            5 => weakness_open(&mut term),
            6 => magic_designer_main(&mut term),
            8 => scissor_test(&mut term),
            _ => tui_mode(
                &mut term,
                ["editor", "ranker", "optimizer"][choice],
                None,
                None,
            ),
        };
        if let Err(e) = result {
            if !term.quitting {
                info_page(&mut term, "Error", &[e.to_string()])?;
            }
        }
    }
    drop(term);
    Ok(())
}

fn main() {
    // Web hosts bundle the data directory under their own root.
    if let Some(home) = std::env::var_os("AKLER_HOME") {
        if let Err(error) = std::env::set_current_dir(&home) {
            eprintln!("cannot enter AKLER_HOME {}: {error}", Path::new(&home).display());
            std::process::exit(1);
        }
    }
    if let Some(outcome) = crate::action_ui::dispatch(&std::env::args().skip(1).collect::<Vec<_>>()) {
        if let Err(error) = outcome {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod corpus_menu_tests {
    use super::*;

    #[test]
    fn empty_corpora_still_offer_an_import_action() {
        let items = corpus_menu_items(&[]);
        assert_eq!(items, vec!["Import text corpus..."]);
    }

    #[test]
    fn import_action_keeps_existing_corpus_indices() {
        let paths = vec![
            PathBuf::from("corpus/raw/news"),
            PathBuf::from("corpus/processed/corpus-fiction.json"),
        ];
        let items = corpus_menu_items(&paths);
        assert_eq!(items, vec!["news", "fiction", "Import text corpus..."]);
        assert_eq!(items.len(), paths.len() + 1);
    }
}

#[cfg(test)]
mod layout_dispatch_tests {
    use super::*;

    const ROWS: &str =
        "q w e r t | y u i o p\na s d f g | h j k l ;\nz x c v b | n m , . /\nthumbs: space\n";

    #[test]
    fn normal_and_action_layouts_select_only_the_required_engine() {
        let path = Path::new("inline.dat");
        assert!(matches!(
            parse_open_layout(ROWS, path).unwrap(),
            OpenLayout::Plain(_)
        ));

        let magic = format!("{ROWS}outer-left: @magic ~ ~\naction magic = repeat-output\n");
        assert!(matches!(
            parse_open_layout(&magic, path).unwrap(),
            OpenLayout::Action(_)
        ));
        assert!(matches!(
            parse_open_layout(ROWS, path).unwrap(),
            OpenLayout::Plain(_)
        ));
    }

    #[test]
    fn layout_dispatch_does_not_retain_old_actions_or_geometry() {
        let path = Path::new("inline.dat");
        let OpenLayout::Plain(original) = parse_open_layout(ROWS, path).unwrap() else {
            panic!("expected ordinary layout");
        };
        let staggered = format!("{ROWS}row-stagger: anglemod\n");
        let OpenLayout::Plain(changed) = parse_open_layout(&staggered, path).unwrap() else {
            panic!("expected ordinary staggered layout");
        };
        assert_ne!(original.keys, changed.keys);
        assert!(parse_open_layout("not a layout", path).is_err());
    }

    const PLAIN_JSON: &str = r#"{
        // Filenames do not decide how a layout is read.
        "layout": {
            "fingers": [
                "q w e r t y u i o p",
                "a s d f g h j k l ;",
                "z x c v b n m , . /",
            ],
            "thumbs": ["space"],
        },
    }"#;

    #[test]
    fn jsonc_dispatch_uses_contents_with_any_filename() {
        for path in [Path::new("seconds"), Path::new("misleading.dat")] {
            assert!(matches!(
                parse_open_layout(PLAIN_JSON, path).unwrap(),
                OpenLayout::Plain(_)
            ));
            let literal_at = PLAIN_JSON.replace("l ;", "l @");
            assert!(matches!(
                parse_open_layout(&literal_at, path).unwrap(),
                OpenLayout::Plain(_)
            ));
            let magic = literal_at.replacen(
                "    }",
                "    }, \"magic\": {\"key\": \"@\", \"rules\": []}",
                1,
            );
            assert!(matches!(
                parse_open_layout(&magic, path).unwrap(),
                OpenLayout::Action(_)
            ));
        }
    }

    #[test]
    fn empty_legacy_magic_settings_do_not_turn_xanthor_at_into_an_action() {
        let jsonc = r#"{
            "layout": {"fingers": [
                "f m l p v x @ u o y",
                "s t r d g . n e a i",
                "z k q c w b h ; ' ,"
            ], "thumbs": ["", ""]},
            "magic": {"magicKeys": [], "rules": []}
        }"#;
        let path = Path::new("xanthor.jsonc");
        let OpenLayout::Plain(board) = parse_open_layout(jsonc, path).unwrap() else {
            panic!("empty magic settings must leave @ literal");
        };
        assert!(board.symbols.contains(&b'@'));

        let dat = ROWS.replace("l ;", "l @");
        assert!(matches!(
            parse_open_layout(&dat, Path::new("literal.dat")).unwrap(),
            OpenLayout::Plain(_)
        ));
        let declared = format!("{dat}action @ = repeat-output\n");
        let OpenLayout::Action(magic) =
            parse_open_layout(&declared, Path::new("magic.dat")).unwrap()
        else {
            panic!("declared @ action must use the action evaluator");
        };
        assert!(magic
            .slots
            .iter()
            .any(|slot| slot.binding == crate::action_keys::Binding::Named("@".into())));
        let saved = magic.text();
        assert!(saved.contains("action @ = repeat-output"));
        let round = crate::action_keys::Layout::parse(&saved, Path::new("round.dat")).unwrap();
        assert!(round
            .slots
            .iter()
            .any(|slot| slot.binding == crate::action_keys::Binding::Named("@".into())));
    }

    #[test]
    fn discovery_recognizes_invalid_layouts_without_absorbing_reports() {
        assert!(recognizable_layout("{\"layout\":"));
        assert!(recognizable_layout("q w e r t | y u i o p\nshort row\n"));
        assert!(!recognizable_layout(
            r#"{"letters":{"a":1},"bigrams":{"aa":1}}"#
        ));
        assert!(!recognizable_layout(
            "model = test\n[original]\nq w e r t | y u i o p\n"
        ));
        assert!(!recognizable_layout(
            "Read the manual before changing layout settings."
        ));
    }

    struct LayoutDirectory(PathBuf);

    impl Drop for LayoutDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn discovery_accepts_optimized_extensionless_and_distinct_companions() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = LayoutDirectory(
            std::env::temp_dir().join(format!("akler-discovery-{}-{unique}", crate::process_tag(),)),
        );
        fs::create_dir(&directory.0).unwrap();
        let dat = action_keys::Layout::parse(PLAIN_JSON, Path::new("inline"))
            .unwrap()
            .text();
        for (name, text) in [
            ("seconds", PLAIN_JSON),
            ("misleading.bin", dat.as_str()),
            ("saved.dat", dat.as_str()),
            ("saved.jsonc", PLAIN_JSON),
            ("different.dat", dat.as_str()),
            (".akler-123-456-0.bak", dat.as_str()),
            (".layouter-123-456-0.bak", dat.as_str()),
            ("saved.dat.1800000000000000000.bak", dat.as_str()),
            ("broken.dat", "{\"layout\":"),
            ("broken.txt", "q w e r t | y u i o p\nshort row\n"),
            ("corpus.json", "{\"letters\":{\"a\":1}}"),
            (
                "candidate.run.txt",
                "model = test\n[original]\nq w e r t | y u i o p\n",
            ),
        ] {
            fs::write(directory.0.join(name), text).unwrap();
        }
        fs::write(
            directory.0.join("different.jsonc"),
            PLAIN_JSON.replace("q w", "w q"),
        )
        .unwrap();
        let optimized = directory.0.join("optimized");
        fs::create_dir(&optimized).unwrap();
        fs::write(optimized.join("saved.jsonc"), PLAIN_JSON).unwrap();
        fs::write(optimized.join("paired.dat"), &dat).unwrap();
        fs::write(optimized.join("paired.jsonc"), PLAIN_JSON).unwrap();
        let nested = optimized.join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join("ignored.jsonc"), PLAIN_JSON).unwrap();
        let names: BTreeSet<_> = discover_layouts(&directory.0)
            .unwrap()
            .into_iter()
            .map(|path| {
                path.strip_prefix(&directory.0)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        let expected: BTreeSet<_> = [
            "seconds",
            "misleading.bin",
            "saved.dat",
            "different.dat",
            "different.jsonc",
            "broken.dat",
            "broken.txt",
            "optimized/saved.jsonc",
            "optimized/paired.dat",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        assert_eq!(names, expected);

        assert_eq!(
            layout_file_label(Path::new("layouts/optimized/saved.jsonc")),
            "optimized/saved.jsonc"
        );
    }
}
