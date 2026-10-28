// Peer diagnostics use the same evaluated snapshot as the ordinary ranker.
fn weakness_open(term: &mut Terminal) -> AppResult<()> {
    let Some(path) = select_source_path(term, true)? else {
        return Ok(());
    };
    let Some(data) = load_ranking_tui(term, &path)? else {
        return Ok(());
    };
    if let Some(selected) = weakness_choose(term, &data)? {
        weakness_loaded(term, &data, selected)?;
    }
    Ok(())
}

fn weakness_choose(term: &mut Terminal, data: &RankingData) -> AppResult<Option<usize>> {
    let order = rank_layout_choices(&data.rows, "");
    let labels = order
        .iter()
        .map(|&i| data.rows[i].name().to_owned())
        .collect::<Vec<_>>();
    Ok(search_menu(term, "Weakness report: layout", &labels)?.map(|i| order[i]))
}

fn weakness_bound_action(
    layout: &action_keys::Layout,
    name: &str,
    visiting: &mut BTreeSet<String>,
    flags: &mut (bool, bool, bool),
) -> String {
    if !visiting.insert(name.to_owned()) {
        return "cycle".into();
    }
    let mut emission = |value: &action_keys::Emission| match value {
        action_keys::Emission::Call(child) => weakness_bound_action(layout, child, visiting, flags),
        other => format!("{other:?}"),
    };
    let fingerprint = match layout.actions.get(name) {
        Some(action_keys::Action::Rules {
            basis,
            rules,
            fallback,
        }) => {
            // Drop the closure before updating the borrowed flags.
            let fallback_binding = emission(fallback);
            let branches = rules
                .iter()
                .map(|(history, value)| format!("{history:?}:{}", emission(value)))
                .collect::<Vec<_>>();
            flags.0 = true;
            flags.1 |= matches!(fallback, action_keys::Emission::Text(_));
            format!("Rules:{basis:?}:{branches:?}:{fallback_binding}")
        }
        Some(
            action_keys::Action::RepeatOutput
            | action_keys::Action::RepeatPreviousOutput
            | action_keys::Action::RepeatAction,
        ) => {
            flags.2 = true;
            format!("{:?}", layout.actions.get(name))
        }
        Some(action_keys::Action::Text(text)) if text.len() == 1 => format!("{}", text[0]),
        other => format!("{other:?}"),
    };
    visiting.remove(name);
    fingerprint
}

type WeaknessSlotGeometry = (i8, i8, i16, i16, usize, i8, i8, bool);

#[derive(PartialEq, Eq)]
struct WeaknessCohort {
    thumb: bool,
    contextual: bool,
    adaptive: bool,
    repeat: bool,
    alts: bool,
    combos: bool,
    geometry: Vec<WeaknessSlotGeometry>,
}

impl WeaknessCohort {
    fn features_match(&self, other: &Self) -> bool {
        (
            self.thumb,
            self.contextual,
            self.adaptive,
            self.repeat,
            self.alts,
            self.combos,
        ) == (
            other.thumb,
            other.contextual,
            other.adaptive,
            other.repeat,
            other.alts,
            other.combos,
        )
    }
    fn geometry_kind(&self) -> usize {
        let rows: BTreeSet<_> = self
            .geometry
            .iter()
            .filter(|slot| slot.7)
            .map(|slot| slot.2)
            .collect();
        let columns: BTreeSet<_> = self
            .geometry
            .iter()
            .filter(|slot| slot.7)
            .map(|slot| slot.3)
            .collect();
        let row = rows.len() > 1;
        let column = columns.len() > 1;
        match (row, column) {
            (false, false) => 2,
            (true, false) => 3,
            (false, true) => 4,
            (true, true) => 5,
        }
    }
}

const WEAKNESS_FAMILIES: [&str; 7] = [
    "Similar features",
    "All families",
    "No thumb alpha",
    "Thumb alpha",
    "Magic",
    "Magic + thumb alpha",
    "Adaptive",
];
const WEAKNESS_GEOMETRIES: [&str; 6] = [
    "Same geometry",
    "Any geometry",
    "Ortholinear",
    "Row stagger",
    "Column stagger",
    "Mixed stagger",
];

#[derive(Clone, Copy, Default)]
struct WeaknessBoardFilter {
    family: usize,
    geometry: usize,
}

impl WeaknessBoardFilter {
    fn matches(&self, candidate: &WeaknessCohort, subject: &WeaknessCohort) -> bool {
        let magic = candidate.contextual || candidate.repeat;
        let family = match self.family {
            0 => candidate.features_match(subject),
            1 => true,
            2 => !candidate.thumb,
            3 => candidate.thumb,
            4 => magic,
            5 => magic && candidate.thumb,
            6 => candidate.adaptive,
            _ => false,
        };
        let geometry = match self.geometry {
            0 => candidate.geometry == subject.geometry,
            1 => true,
            kind => candidate.geometry_kind() == kind,
        };
        family && geometry
    }
}

fn weakness_tabs(canvas: &mut Canvas, active: usize) {
    let labels = if canvas.w >= 39 {
        [" [1] Report ", " [2] Design ", " [3] Board type "]
    } else if canvas.w >= 33 {
        [" [1] Report ", " [2] Design ", " [3] Board "]
    } else {
        [" [1] ", " [2] ", " [3] "]
    };
    workspace_tabs(canvas, active, &labels);
}

fn weakness_identity(row: &RankRow, weights: &Weights) -> (WeaknessCohort, String) {
    let mut geometry = Vec::new();
    let mut bindings = Vec::new();
    let (mut contextual, mut adaptive, mut repeat) = (false, false, false);
    let (mut alts, mut combos) = (false, false);
    match row {
        RankRow::Plain { model, .. } => {
            for (i, k) in model.geometry.keys.iter().enumerate() {
                let position = (
                    k.row,
                    k.col,
                    k.row_offset,
                    k.column_offset,
                    k.finger,
                    k.rank,
                    k.hand,
                    k.main,
                );
                geometry.push(position);
                bindings.push(format!(
                    "{position:?}:{}",
                    model.canonical[model.original[i]]
                ));
            }
        }
        RankRow::Action { layout, .. } => {
            alts = weights.alts().active(layout);
            combos = weights.4 && !layout.combos.is_empty();
            for slot in &layout.slots {
                let position = (
                    slot.row,
                    slot.col,
                    slot.row_offset,
                    slot.column_offset,
                    slot.finger,
                    slot.rank,
                    slot.hand,
                    slot.main,
                );
                geometry.push(position);
                let binding = match &slot.binding {
                    action_keys::Binding::Named(name) => {
                        let mut flags = (false, false, false);
                        let fingerprint =
                            weakness_bound_action(layout, name, &mut BTreeSet::new(), &mut flags);
                        contextual |= flags.0;
                        adaptive |= flags.1;
                        repeat |= flags.2;
                        fingerprint
                    }
                    action_keys::Binding::Text(text) if text.len() == 1 => format!("{}", text[0]),
                    other => format!("{other:?}"),
                };
                bindings.push(format!("{position:?}:{binding}"));
            }
            if alts {
                bindings.push(format!("alts:{:?}", layout.alt_fingers));
            }
            if combos {
                bindings.push(format!("combos:{:?}", layout.combos));
            }
        }
    }
    geometry.sort();
    bindings.sort();
    let cohort = WeaknessCohort {
        thumb: row.has_thumb_letter(),
        contextual,
        adaptive,
        repeat,
        alts,
        combos,
        geometry,
    };
    (cohort, bindings.join("|"))
}

struct WeaknessIssue {
    metric: usize,
    value: f64,
    median: f64,
    adverse_percentile: f64,
    peers: usize,
}

fn weakness_issues(
    data: &RankingData,
    selected: usize,
    filter: WeaknessBoardFilter,
) -> (Vec<WeaknessIssue>, usize) {
    let subject = &data.rows[selected];
    let (cohort, own_binding) = weakness_identity(subject, &data.weights);
    let mut seen = BTreeSet::new();
    seen.insert(own_binding);
    let peers = data
        .rows
        .iter()
        .filter(|row| {
            if row.path() == subject.path() {
                return false;
            }
            let (candidate, binding) = weakness_identity(row, &data.weights);
            filter.matches(&candidate, &cohort) && seen.insert(binding)
        })
        .collect::<Vec<_>>();
    let mut issues = Vec::new();
    for metric in 0..N_METRICS {
        if aggregate(metric) {
            continue;
        }
        let Some(value) = rank_value(subject, metric).filter(|v| v.is_finite()) else {
            continue;
        };
        let mut values = peers
            .iter()
            .filter_map(|row| rank_value(row, metric))
            .filter(|v| v.is_finite())
            .collect::<Vec<_>>();
        if values.is_empty() {
            continue;
        }
        values.sort_by(f64::total_cmp);
        let n = values.len();
        let median = if n % 2 == 0 {
            (values[n / 2 - 1] + values[n / 2]) / 2.0
        } else {
            values[n / 2]
        };
        if if higher_better(metric) {
            value >= median
        } else {
            value <= median
        } {
            continue;
        }
        let worse = values
            .iter()
            .map(|&peer| {
                if value == peer {
                    0.5
                } else if if higher_better(metric) {
                    value < peer
                } else {
                    value > peer
                } {
                    1.0
                } else {
                    0.0
                }
            })
            .sum::<f64>();
        issues.push(WeaknessIssue {
            metric,
            value,
            median,
            adverse_percentile: 100.0 * worse / n as f64,
            peers: n,
        });
    }
    issues.sort_by(|a, b| {
        b.adverse_percentile
            .total_cmp(&a.adverse_percentile)
            .then_with(|| a.metric.cmp(&b.metric))
    });
    (issues, peers.len())
}

fn weakness_loaded(term: &mut Terminal, data: &RankingData, mut selected: usize) -> AppResult<()> {
    if data.mana2 {
        return info_page(
            term,
            "Weakness report",
            &["This report currently supports AKLER detailed analysis.".into()],
        );
    }
    let (mut page, mut issue_index, mut board_cursor) = (0usize, 0usize, 0usize);
    let mut board_filter = WeaknessBoardFilter::default();
    let (mut issues, mut peers) = weakness_issues(data, selected, board_filter);
    loop {
        let row = &data.rows[selected];
        issue_index = issue_index.min(issues.len().min(10).saturating_sub(1));
        let mut c = Canvas::ranking(term.size.0, term.size.1);
        weakness_tabs(&mut c, page);
        let footer = term.size.1.saturating_sub(3);
        let body_end = footer;
        if page == 2 {
            for (index, (label, value)) in [
                ("Family", WEAKNESS_FAMILIES[board_filter.family]),
                ("Geometry", WEAKNESS_GEOMETRIES[board_filter.geometry]),
            ]
            .iter()
            .enumerate()
            {
                let y = 2 + index;
                if y >= body_end {
                    break;
                }
                c.text(
                    1,
                    y,
                    &format!(
                        "{} {label:<10} {value}",
                        if board_cursor == index { "›" } else { " " }
                    ),
                    if board_cursor == index { CYAN } else { FG },
                );
                c.hit(
                    Rect {
                        x: 1,
                        y,
                        w: c.w.saturating_sub(2),
                        h: 1,
                    },
                    Action::Setting(index),
                );
            }
            if body_end > 5 {
                c.text(1, 5, "h/l change · Enter/Space next", MUTED);
            }
        } else if body_end < 4 {
            // Keep tabs and footer reachable in a short terminal.
        } else if peers == 0 {
            c.text(
                1,
                2,
                "No comparable peers. No ranks available for this selected board type.",
                YELLOW,
            );
        } else if issues.is_empty() {
            c.text(
                1,
                2,
                "No supported metrics are worse than the peer median.",
                YELLOW,
            );
        } else if page == 1 {
            let issue = &issues[issue_index];
            let lines = vec![
                format!("Selected issue: {}", METRIC_NAMES[issue.metric]),
                METRIC_HELP[issue.metric].to_owned(),
                "Investigate the contributing physical slots and corpus sequences in Editor."
                    .into(),
                "Check finger assignments, row changes, and the sequence's emitted text.".into(),
                "For contextual bindings, inspect matched history, fallback, and repeat behavior."
                    .into(),
                "Compare rule coverage and literal output before proposing a binding change."
                    .into(),
                "Press g to search magic rules and preview their measured effects.".into(),
            ];
            let lines = wrap_lines(&lines, c.w.saturating_sub(2));
            for (i, line) in lines.iter().take(body_end.saturating_sub(2)).enumerate() {
                c.text(1, 2 + i, line, FG);
            }
        } else {
            c.text(
                1,
                2,
                "Metric        Current     Peer median    Delta       Adverse%    N",
                MUTED,
            );
            let capacity = body_end.saturating_sub(3).max(1);
            let top = issue_index.saturating_sub(capacity - 1);
            for (i, issue) in issues.iter().take(10).enumerate().skip(top).take(capacity) {
                c.text(
                    1,
                    3 + i - top,
                    &format!(
                        "{} {:<10} {:>10.4} {:>13.4} {:>11.4} {:>10.1} {:>4}",
                        if i == issue_index { "›" } else { " " },
                        METRIC_NAMES[issue.metric],
                        issue.value,
                        issue.median,
                        issue.value - issue.median,
                        issue.adverse_percentile,
                        issue.peers
                    ),
                    if i == issue_index { CYAN } else { FG },
                );
            }
        }
        if term.size.1 >= 4 {
            c.text(
                1,
                footer,
                &short(
                    &format!(
                        "{} · {} · {} / {} · {} peers",
                        row.name(),
                        data.name,
                        WEAKNESS_FAMILIES[board_filter.family],
                        WEAKNESS_GEOMETRIES[board_filter.geometry],
                        peers
                    ),
                    c.w.saturating_sub(2),
                ),
                MUTED,
            );
            c.text(
                1,
                footer + 1,
                &short(
                    if page == 2 {
                        "1/2/3 tabs · j/k select · h/l change · q back"
                    } else {
                        "1/2/3 tabs · j/k issue · / layout · g magic · ? help · q back"
                    },
                    c.w.saturating_sub(2),
                ),
                MUTED,
            );
        }
        term.present(&c, 0)?;
        let mut event = term.event()?;
        if let Event::Mouse {
            x,
            y,
            button: 0,
            release: false,
            motion: false,
            ..
        } = event
        {
            if let Some(Action::Command(key)) = c.action(x, y) {
                event = Event::Char(key);
            }
        }
        let mut filter_changed = false;
        if let Event::Mouse {
            x,
            y,
            button: 0,
            release: false,
            motion: false,
            ..
        } = event
        {
            if let Some(Action::Setting(index)) = c.action(x, y) {
                board_cursor = index;
                if index == 0 {
                    board_filter.family = (board_filter.family + 1) % WEAKNESS_FAMILIES.len();
                } else {
                    board_filter.geometry = (board_filter.geometry + 1) % WEAKNESS_GEOMETRIES.len();
                }
                filter_changed = true;
            }
        }
        match event {
            Event::Escape | Event::Quit | Event::Char('q') => return Ok(()),
            Event::Tab => page = (page + 1) % 3,
            Event::Char('1') => page = 0,
            Event::Char('2') => page = 1,
            Event::Char('3') => page = 2,
            Event::Up | Event::Char('k') if page == 2 => {
                board_cursor = board_cursor.saturating_sub(1)
            }
            Event::Down | Event::Char('j') if page == 2 => board_cursor = (board_cursor + 1).min(1),
            Event::Left | Event::Right | Event::Char('h' | 'l' | ' ') | Event::Enter
                if page == 2 =>
            {
                let backwards = matches!(event, Event::Left | Event::Char('h'));
                let (value, length) = if board_cursor == 0 {
                    (&mut board_filter.family, WEAKNESS_FAMILIES.len())
                } else {
                    (&mut board_filter.geometry, WEAKNESS_GEOMETRIES.len())
                };
                *value = if backwards {
                    (*value + length - 1) % length
                } else {
                    (*value + 1) % length
                };
                filter_changed = true;
            }
            Event::Left | Event::Char('h') => page = (page + 2) % 3,
            Event::Right | Event::Char('l') => page = (page + 1) % 3,
            Event::Down | Event::Char('j') => {
                issue_index = (issue_index + 1).min(issues.len().min(10).saturating_sub(1))
            }
            Event::Up | Event::Char('k') => issue_index = issue_index.saturating_sub(1),
            Event::Char('g') => magic_design_open(term, data, selected)?,
            Event::Char('/') => {
                if let Some(next) = weakness_choose(term, data)? {
                    selected = next;
                    (issues, peers) = weakness_issues(data, selected, board_filter);
                    issue_index = 0;
                }
            }
            Event::Char('?') => {
                let mut lines = vec![
                    "3 Board type: choose the comparison family and physical geometry of peers. Your selected layout stays unchanged.".into(),
                    "Geometry uses relative offsets of main keys: ortholinear, row stagger, column stagger, or mixed. Thumb placement and uniform translations do not determine stagger type.".into(),
                    "Family and geometry filters affect peers only. Magic includes contextual/repeat keys and may overlap Adaptive.".into(),
                    "Similar features matches thumb letters, contextual/adaptive/repeat actions and active alt-finger/combo features. Same geometry also matches fingers and slots. Other choices explicitly broaden or change the peer group.".into(),
                    "Up to ten canonical metrics worse than the peer median are ordered by adverse peer percentile. Ties count halfway; raw delta is current minus peer median. Display totals are excluded.".into(),
                    "This is a corpus diagnostic, not evidence of comfort or typing speed. Small peer cohorts give coarse percentiles. Different symbol support may affect denominators.".into(),
                    "g opens magic-rule search with physical root selection, measured effects, and save-copy controls.".into(),
                    format!("Source: {} · spacegrams {} · evaluated layouts {}", data.path.display(), data.spacegrams, data.rows.len()),
                    format!("Source limits: {:#?}", data.ngrams),
                ];
                lines.extend(
                    data.warnings
                        .iter()
                        .map(|warning| format!("Warning: {warning}")),
                );
                lines.extend(data.errors.iter().map(|error| format!("ERROR: {error}")));
                help_popup(term, &c, "Weakness report help", &lines)?;
            }
            _ => {}
        }
        if filter_changed {
            (issues, peers) = weakness_issues(data, selected, board_filter);
            issue_index = 0;
        }
    }
}
