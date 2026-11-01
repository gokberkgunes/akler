fn magic_designer_main(term: &mut Terminal) -> AppResult<()> {
    let Some(path) = select_source_path(term, true)? else {
        return Ok(());
    };
    let Some(data) = load_ranking_tui(term, &path)? else {
        return Ok(());
    };
    let order = rank_layout_choices(&data.rows, "");
    let names: Vec<_> = order
        .iter()
        .map(|&i| data.rows[i].name().to_owned())
        .collect();
    if let Some(index) = search_menu(term, "Magic designer: layout", &names)? {
        magic_design_open(term, &data, order[index])?;
    }
    Ok(())
}

fn magic_root_label(layout: &action_keys::Layout, index: usize) -> String {
    let slot = &layout.slots[index];
    format!(
        "#{} {} · {} · row {} col {}",
        index + 1,
        slot.label,
        FINGER_NAMES.get(slot.finger).copied().unwrap_or("?"),
        slot.row + 1,
        slot.col + 1
    )
}

/// Open `selected` in the action optimizer on its Magic tab.
fn magic_design_open(term: &mut Terminal, data: &RankingData, selected: usize) -> AppResult<()> {
    if data.mana2 {
        return info_page(
            term,
            "Magic design",
            &["Open an ordinary detailed ranking to design magic rules.".into()],
        );
    }
    let Some(row) = data.rows.get(selected) else {
        return Ok(());
    };
    let (original, existing_corpus) = match row {
        RankRow::Plain { model, .. } => (
            board_as_action_layout(&model.board, &model.board.symbols)?,
            None,
        ),
        RankRow::Action { layout, corpus, .. } => (layout.clone(), Some(Arc::clone(corpus))),
    };
    let corpus = match existing_corpus {
        Some(corpus) => (*corpus).clone(),
        None => {
            let path = data.path.clone();
            let limits = data.ngrams;
            let Some(corpus) = action_ui::ngram_job(
                term,
                move |stop, progress| {
                    let raw = fs::read_to_string(&path).map_err(|error| error.to_string())?;
                    action_ngrams::NgramCorpus::from_text_progress_with_limits(
                        &raw, &path, limits, stop, progress,
                    )
                },
            )?
            else {
                return Ok(());
            };
            corpus
        }
    };
    action_ui::action_optimizer_magic(term, original, corpus)
}

/// The optimizer's Magic tab, kept across visits in one optimizer session:
/// its own design settings, the last search's candidates and what they
/// replace.
pub(crate) struct MagicTab {
    /// Finger groups (`MAGIC_FINGER_GROUPS`) that may hold the magic key.
    groups: [bool; 5],
    passes: usize,
    shortlist: usize,
    retain: usize,
    archive: Option<crate::magic_design::DesignArchive>,
    /// The layout searched, before a magic key it added and with its
    /// adaptive swaps; the searched layout's locks, magic key, and swaps.
    seed: Option<MagicSeed>,
    /// The selected row: a setting, then the candidates.
    selected: usize,
    /// The page's scroll and the first scrolled ranker column, kept while
    /// other tabs are open.
    scroll: usize,
    first_column: usize,
}

struct MagicSeed {
    before: action_keys::Layout,
    before_locks: Vec<bool>,
    locks: Vec<bool>,
    root: usize,
    swaps: Vec<crate::swap_fit::Swap>,
}

/// What a Magic tab search starts from: the optimizer's layout without its
/// adaptive swaps (`swaps`), with a magic key added on an outer key when it
/// has none, its locks, the keys the magic key may take and the locked keys
/// no letter leaves.
struct MagicPlan {
    seed: action_keys::Layout,
    root: usize,
    locks: Vec<bool>,
    allowed: Vec<usize>,
    locked_slots: Vec<usize>,
    swaps: Vec<crate::swap_fit::Swap>,
}

/// The `MagicPlan` for `layout`: a locked magic key keeps its key; an
/// unlocked one may take any unlocked key the finger `groups` and `magic`
/// settings allow.
fn magic_search_plan(
    layout: &action_keys::Layout,
    locks: &[bool],
    magic_root: Option<usize>,
    groups: &[bool; 5],
    magic: &MagicSettings,
) -> Result<MagicPlan, String> {
    let swaps = crate::swap_fit::swaps_of(layout);
    let base = crate::swap_fit::split(layout).0;
    // A layout's own magic key, even one of several the optimizer does not
    // search, before a new one.
    let existing = magic_root.or_else(|| {
        base.slots.iter().position(|slot| {
            matches!(slot.binding, action_keys::Binding::Named(_))
                && crate::layout_io::dedicated_magic_label(&slot.label)
        })
    });
    let (seed, root, seed_locks) = match existing {
        Some(root) => (base, root, locks.to_vec()),
        None => crate::magic_optimizer::inject_default(&base, locks)
            .map_err(|error| format!("Cannot add a magic key: {error}"))?,
    };
    let keys = action_ui::physical_keys(&seed);
    let allowed: Vec<usize> = if seed_locks.get(root).copied().unwrap_or(false) {
        vec![root]
    } else {
        crate::magic_design::placement_roots(&seed)
            .into_iter()
            .filter(|&slot| {
                (slot == root || !seed_locks[slot])
                    && magic.allows(keys[slot])
                    && MAGIC_FINGER_GROUPS
                        .iter()
                        .zip(groups)
                        .any(|((_, fingers), &on)| on && fingers.contains(&seed.slots[slot].finger))
            })
            .collect()
    };
    if allowed.is_empty() {
        return Err("No key may hold the magic key: unblock a finger (p) or unlock a key.".into());
    }
    let locked_slots = (0..seed_locks.len())
        .filter(|&slot| {
            seed_locks[slot]
                && matches!(
                    seed.slots[slot].binding,
                    action_keys::Binding::Text(_) | action_keys::Binding::Empty
                )
        })
        .collect();
    Ok(MagicPlan { seed, root, locks: seed_locks, allowed, locked_slots, swaps })
}

/// A candidate as the optimizer's layout and locks: its adaptive swaps
/// back, and the magic key's lock moved with it, as is the displaced
/// letter's.
fn magic_applied(
    entry: &crate::magic_design::Candidate,
    seed: &MagicSeed,
) -> (action_keys::Layout, Vec<bool>) {
    let mut locks = seed.locks.clone();
    if entry.root != seed.root && entry.root < locks.len() && seed.root < locks.len() {
        locks.swap(entry.root, seed.root);
    }
    let mut layout = entry.layout.clone();
    if !seed.swaps.is_empty() {
        layout = crate::swap_fit::install(&layout, &seed.swaps);
    }
    (layout, locks)
}

/// How the Magic tab was left.
pub(crate) enum MagicTabExit {
    /// Another tab's key, or `q` to leave the optimizer.
    Key(char),
    /// A candidate became the optimizer's layout; the message says so.
    Applied(String),
}

const MAGIC_SETTING_ROWS: usize = 4;

impl MagicTab {
    pub(crate) fn new() -> Self {
        let fingers = load_app_config()
            .map(|config| config.magic.allowed_fingers)
            .unwrap_or([true; 10]);
        Self {
            groups: std::array::from_fn(|group| {
                MAGIC_FINGER_GROUPS[group].1.iter().any(|&finger| fingers[finger])
            }),
            passes: 3,
            shortlist: 24,
            retain: 20,
            archive: None,
            seed: None,
            selected: 0,
            scroll: 0,
            first_column: 0,
        }
    }

    fn blocked(&self) -> String {
        let names: Vec<_> = MAGIC_FINGER_GROUPS
            .iter()
            .zip(self.groups)
            .filter(|(_, on)| !on)
            .map(|((name, _), _)| name.to_ascii_lowercase())
            .collect();
        if names.is_empty() {
            "none".into()
        } else {
            names.join(", ")
        }
    }
}

/// The Magic tab of the action optimizer: searches magic-key positions,
/// rules and the letters around them on the optimizer's layout, with its
/// locks, corpus, weights, caps, workers and magic rule limit. A locked
/// magic key keeps its key; locked letters stay put. Applying a candidate
/// makes it the optimizer's layout, whose run then fits adaptive swaps and
/// combos as its settings say.
#[allow(clippy::too_many_arguments)]
pub(crate) fn optimizer_magic_tab(
    term: &mut Terminal,
    state: &mut MagicTab,
    layout: &mut action_keys::Layout,
    original: &action_keys::Layout,
    corpus: &action_ngrams::NgramCorpus,
    weights: &Weights,
    settings: &SearchSettings,
    max_rules: usize,
    locks: &mut Vec<bool>,
    magic_root: &mut Option<usize>,
    before_magic: &mut Option<(action_keys::Layout, Vec<bool>)>,
) -> AppResult<MagicTabExit> {
    use crate::magic_design::{IterativeSettings, PlacementMode, Settings};
    let mut status = String::new();
    let (rank_columns, rank_mana2) = configured_rank_columns(
        &load_app_config().unwrap_or_default(),
    );
    loop {
        let candidates = state
            .archive
            .as_ref()
            .map_or(&[][..], |archive| &archive.candidates[..]);
        state.selected = state
            .selected
            .min(MAGIC_SETTING_ROWS + candidates.len().saturating_sub(1));
        let chosen = state
            .selected
            .checked_sub(MAGIC_SETTING_ROWS)
            .and_then(|index| candidates.get(index));
        let mut c = Canvas::new(term.width(), 200);
        optimizer_tabs(&mut c, 4, &corpus.name, &layout.name, &settings.mode);
        let shown = chosen.map_or(&*layout, |candidate| &candidate.layout);
        let root = chosen.map(|candidate| candidate.root).or(*magic_root);
        let mut y = action_ui::action_keyboard(
            &mut c,
            2,
            shown,
            if shown.slots.len() == layout.slots.len() { &*layout } else { shown },
            (chosen.is_none() && locks.len() == layout.slots.len()).then_some(&locks[..]),
            root,
        ) + 1;
        let own_root = magic_root.or_else(|| {
            layout.slots.iter().position(|slot| {
                matches!(slot.binding, action_keys::Binding::Named(_))
                    && crate::layout_io::dedicated_magic_label(&slot.label)
            })
        });
        let magic_line = match own_root {
            Some(root) if locks.get(root).copied().unwrap_or(false) => format!(
                "Magic key {} is locked: the search finds the best layout around it. Click it to let it move.",
                layout.slots[root].label
            ),
            Some(root) => format!(
                "Magic key {}: the search tries each allowed key for it. Click it to lock it in place.",
                layout.slots[root].label
            ),
            None => "No magic key yet: the search adds one on an outer key and moves it to an allowed key.".into(),
        };
        if chosen.is_some() {
            c.text(0, y, "Candidate shown; select a setting row to edit locks.", MUTED);
        } else {
            c.text(0, y, &short(&magic_line, c.w), CYAN);
        }
        y += 2;
        let setting_rows = [
            format!("Finger blocks: {}", state.blocked()),
            format!("Passes: {}", state.passes),
            format!("Rule shortlist: {}", state.shortlist),
            format!("Positions shown: {}", state.retain),
        ];
        for (index, text) in setting_rows.iter().enumerate() {
            let row = Rect { x: 0, y, w: c.w, h: 1 };
            c.text(1, y, text, FG);
            if index == state.selected {
                c.row_style(row, Some((42, 42, 42)), false);
            }
            c.hit(row, Action::Setting(index));
            y += 1;
        }
        c.text(
            1,
            y,
            &short(
                &format!(
                    "Rules per key {} (Settings · Magic rules); caps, workers and weights as the optimizer's",
                    max_rules.clamp(1, crate::magic_design::MAX_RULES)
                ),
                c.w.saturating_sub(1),
            ),
            MUTED,
        );
        y += 2;
        if let Some(archive) = &state.archive {
            // The ranker's columns, as many as fit before the rules gain.
            const CELL: usize = 9;
            // SCORE stays; ← and → scroll the rest, as in the ranker.
            let fit = (c.w.saturating_sub(13 + 11) / CELL).max(2);
            let pinned = usize::from(rank_columns.first() == Some(&RANK_SCORE));
            let rest = &rank_columns[pinned..];
            let room = fit - pinned;
            state.first_column = state.first_column.min(rest.len().saturating_sub(room));
            let first_column = state.first_column;
            let metrics: Vec<usize> = rank_columns[..pinned]
                .iter()
                .chain(rest.iter().skip(first_column).take(room))
                .copied()
                .collect();
            let more = rest.len() > room;
            let gain_x = 13 + metrics.len() * CELL;
            let table_w = gain_x + 11;
            let speed = archive.before.counts.speed_populations;
            let value = |m, score, raw, totals, figures| {
                action_rank_value(m, score, raw, totals, figures, speed)
            };
            let before = &archive.before;
            let start: Vec<_> = metrics
                .iter()
                .map(|&m| value(m, before.score, &before.raw, &before.totals, &before.metrics))
                .collect();
            if more {
                let hint = format!(
                    "Ranker columns {}-{} of {}; ← → scrolls",
                    first_column + 1,
                    first_column + room,
                    rest.len()
                );
                c.text(2, y, &hint, MUTED);
                y += 1;
            }
            c.text(2, y, "Magic key", FG);
            for (j, &m) in metrics.iter().enumerate() {
                c.right(13 + j * CELL, y, CELL - 1, displayed_rank_name(m, rank_mana2), FG);
            }
            c.right(gain_x, y, 10, "Rules gain", FG);
            y += 1;
            c.text(2, y, "Start", MUTED);
            for (j, (&m, start)) in metrics.iter().zip(&start).enumerate() {
                let text = start.map_or_else(|| "n/a".into(), |v| rank_text(m, v, 3));
                c.right(13 + j * CELL, y, CELL - 1, &text, MUTED);
            }
            y += 1;
            for (index, entry) in candidates.iter().enumerate() {
                let row = MAGIC_SETTING_ROWS + index;
                c.text(0, y, if row == state.selected { "●" } else { " " }, CYAN);
                let key = &entry.layout.slots[entry.root];
                c.text(
                    2,
                    y,
                    &short(
                        &format!(
                            "{} r{}c{}",
                            FINGER_NAMES.get(key.finger).copied().unwrap_or("?"),
                            key.row + 1,
                            key.col + 1
                        ),
                        10,
                    ),
                    FG,
                );
                for (j, (&m, start)) in metrics.iter().zip(&start).enumerate() {
                    let found = value(m, entry.score, &entry.raw, &entry.totals, &entry.metrics);
                    let (text, color) = match (found, start) {
                        (None, _) => ("n/a".into(), MUTED),
                        (Some(v), Some(baseline)) if (v - baseline).abs() > 1e-9 => {
                            let better = (v < *baseline) == !rank_positive(m);
                            (rank_text(m, v, 3), if better { GREEN } else { RED })
                        }
                        (Some(v), _) => (rank_text(m, v, 3), FG),
                    };
                    c.right(13 + j * CELL, y, CELL - 1, &text, color);
                }
                match entry.rules_off_score {
                    Some(off) => c.text(
                        gain_x,
                        y,
                        &format!("{:+10.4}", off - entry.score),
                        if off > entry.score + 1e-9 { GREEN } else { FG },
                    ),
                    None => c.text(gain_x, y, "       n/a", MUTED),
                }
                let rect = Rect { x: 0, y, w: c.w.min(table_w), h: 1 };
                if row == state.selected {
                    c.row_style(rect, Some((42, 42, 42)), false);
                }
                c.hit(rect, Action::Item(index));
                y += 1;
            }
            if candidates.is_empty() {
                c.text(0, y, "No allowed position kept the layout within the caps.", MUTED);
                y += 1;
            }
            if let Some(entry) = chosen {
                y = magic_score_changes(
                    &mut c,
                    0,
                    y + 1,
                    &archive.before.metrics,
                    &entry.metrics,
                    weights,
                );
            }
        } else {
            c.text(
                0,
                y,
                "Space searches. The best layout for each magic-key position appears here; Enter applies one.",
                MUTED,
            );
            y += 1;
        }
        y += 1;
        c.text(
            0,
            y,
            &short(
                "Space search · j/k select · Enter edit / apply · click keys to lock (H/U/L) · c details · v rules · s/S save · o open · ? help",
                c.w,
            ),
            MUTED,
        );
        y += 1;
        c.text(0, y, &short(&status, c.w), FG);
        c.h = y + 1;
        c.cells.truncate(c.w * c.h);
        state.scroll = state.scroll.min(c.h.saturating_sub(term.size.1));
        term.present(&c, state.scroll)?;
        let mut event = term.event()?;
        if scroll_event(&event, &mut state.scroll, c.h, term.size.1) {
            continue;
        }
        if let Event::Mouse { x, y, button: 0, release: false, motion: false, .. } = event {
            match term.hit(&c, x, y, state.scroll) {
                Some(Action::Key(slot)) if chosen.is_none() && slot < locks.len() => {
                    locks[slot] = !locks[slot];
                    continue;
                }
                Some(Action::Setting(index)) => {
                    state.selected = index;
                    event = Event::Enter;
                }
                Some(Action::Item(index)) => {
                    state.selected = MAGIC_SETTING_ROWS + index;
                    continue;
                }
                Some(Action::Command(ch)) => event = Event::Char(ch),
                _ => {}
            }
        }
        match event {
            Event::Char(tab @ ('1'..='4')) => return Ok(MagicTabExit::Key(tab)),
            Event::Escape | Event::Char('q') => return Ok(MagicTabExit::Key('1')),
            Event::Quit => return Ok(MagicTabExit::Key('q')),
            Event::Char('j') | Event::Down => state.selected += 1,
            Event::Left => state.first_column = state.first_column.saturating_sub(1),
            Event::Right => state.first_column += 1,
            Event::Char('k') | Event::Up => state.selected = state.selected.saturating_sub(1),
            Event::Char('H') => {
                *locks = action_ui::optimizer_locks(layout, action_ui::action_home_locks(layout));
            }
            Event::Char('U') => locks.fill(false),
            Event::Char('L') => locks.fill(true),
            Event::Char('p') => magic_finger_groups(term, &mut state.groups)?,
            Event::Enter if state.selected == 0 => magic_finger_groups(term, &mut state.groups)?,
            Event::Enter if state.selected < MAGIC_SETTING_ROWS => {
                let (title, value, max) = match state.selected {
                    1 => ("Passes", &mut state.passes, 100),
                    2 => ("Rule shortlist", &mut state.shortlist, 256),
                    _ => ("Positions shown", &mut state.retain, 200),
                };
                if let Some(text) = input_box(
                    term,
                    title,
                    &format!("Integer from 1 to {max}"),
                    &value.to_string(),
                )? {
                    match text.parse::<usize>() {
                        Ok(n) if (1..=max).contains(&n) => *value = n,
                        _ => status = format!("{title}: enter an integer from 1 to {max}."),
                    }
                }
            }
            Event::Enter => {
                let index = state.selected - MAGIC_SETTING_ROWS;
                let (Some(archive), Some(seed)) = (&state.archive, &state.seed) else {
                    continue;
                };
                let Some(entry) = archive.candidates.get(index) else {
                    continue;
                };
                let (applied, applied_locks) = magic_applied(entry, seed);
                if before_magic.is_none() {
                    *before_magic = Some((seed.before.clone(), seed.before_locks.clone()));
                }
                *layout = applied;
                *locks = applied_locks;
                *magic_root = Some(entry.root);
                return Ok(MagicTabExit::Applied(format!(
                    "Magic candidate {} applied; Space runs the optimizer, which fits adaptive swaps and combos as Settings says.",
                    index + 1
                )));
            }
            Event::Char(' ') => {
                let plan = match magic_search_plan(
                    layout,
                    locks,
                    *magic_root,
                    &state.groups,
                    &load_app_config()?.magic,
                ) {
                    Ok(plan) => plan,
                    Err(error) => {
                        status = error;
                        continue;
                    }
                };
                let MagicPlan { seed, root, locks: seed_locks, allowed, locked_slots, swaps } = plan;
                let run = IterativeSettings {
                    rule_settings: Settings {
                        roots: allowed,
                        locked_slots,
                        max_rules: max_rules.clamp(1, crate::magic_design::MAX_RULES),
                        shortlist: state.shortlist,
                        sfb_limit: settings.sfb_limit,
                        sfs_limit: settings.sfs_limit,
                        travel_limit: settings.travel_limit,
                        sftravel_limit: settings.sftravel_limit,
                    },
                    passes: state.passes,
                    workers: settings.workers,
                    archive: state.retain,
                    placement: PlacementMode::Existing,
                };
                // Caps and the Start row measure the layout searched: its own
                // magic keys, without its adaptive swaps.
                let job_seed = seed.clone();
                let job_original = seed.clone();
                let job_corpus = corpus.clone();
                let job_weights = weights.clone();
                let Some(result) = action_ui::ngram_job(term, move |stop, progress| {
                    crate::magic_design::iterative_refine(
                        &job_seed,
                        &job_original,
                        &job_corpus,
                        &job_weights,
                        &run,
                        stop,
                        progress,
                    )
                })?
                else {
                    status = "Magic search cancelled.".into();
                    continue;
                };
                status = format!(
                    "{} evaluations; {} candidates.",
                    result.candidate_count,
                    result.candidates.len()
                );
                state.archive = Some(result);
                state.seed = Some(MagicSeed {
                    before: layout.clone(),
                    before_locks: locks.clone(),
                    locks: seed_locks,
                    root,
                    swaps,
                });
                state.selected = MAGIC_SETTING_ROWS;
            }
            Event::Char('s') => {
                if let Some(entry) = chosen {
                    let mut standalone = entry.layout.clone();
                    let stem: String = original
                        .name
                        .chars()
                        .map(|ch| if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') { ch } else { '_' })
                        .collect();
                    standalone.path = Path::new(LAYOUT_DIR).join(format!("{stem}-magic.jsonc"));
                    status = match crate::layout_export::save_jsonc_copy(&standalone) {
                        Ok(path) => format!("saved {}", path.display()),
                        Err(error) => format!("Save failed: {error}"),
                    };
                }
            }
            Event::Char('o') => {
                let Some(path) =
                    input_box(term, "Open magic candidates", "Saved folder", "layouts/magic/")?
                else {
                    continue;
                };
                let folder = std::path::PathBuf::from(path);
                let loaded = crate::magic_archive::load_baseline(&folder).and_then(|base| {
                    crate::magic_archive::load_candidates(&folder).map(|layouts| (base, layouts))
                });
                let (base, layouts) = match loaded {
                    Ok(loaded) => loaded,
                    Err(error) => {
                        status = format!("Open failed: {error}");
                        continue;
                    }
                };
                let Some(first) = layouts.first().cloned() else {
                    status = "Open failed: no candidates in that folder.".into();
                    continue;
                };
                let first_root = match magic_recovery_root(&first) {
                    Ok(root) => root,
                    Err(error) => {
                        status = format!("Open failed: {error}");
                        continue;
                    }
                };
                let job_corpus = corpus.clone();
                let job_weights = weights.clone();
                let Some(result) = action_ui::ngram_job(term, move |stop, progress| {
                    let before =
                        action_summary::evaluate(&base, &job_corpus, &job_weights, stop, progress)?;
                    let mut entries = Vec::new();
                    for layout in layouts {
                        let root = magic_recovery_root(&layout)?;
                        let summary = action_summary::evaluate(
                            &layout, &job_corpus, &job_weights, stop, progress,
                        )?;
                        let rules = magic_recovered_rules(&layout, root);
                        entries.push(crate::magic_design::Candidate {
                            layout,
                            score: summary.score,
                            rules_off_score: None,
                            rules_off_note: None,
                            raw: summary.raw,
                            totals: summary.totals,
                            metrics: summary.metrics,
                            root,
                            rules,
                            pass: 0,
                        });
                    }
                    entries.sort_by(|a, b| a.score.total_cmp(&b.score));
                    crate::magic_design::fill_rules_off_comparisons(
                        &mut entries,
                        &job_corpus,
                        &job_weights,
                        stop,
                        progress,
                    )?;
                    Ok(crate::magic_design::DesignArchive {
                        baseline: base,
                        before,
                        candidate_count: entries.len(),
                        candidates: entries,
                        warnings: Vec::new(),
                        settings: IterativeSettings::default(),
                        source_order: job_corpus.order,
                    })
                })?
                else {
                    continue;
                };
                state.seed = Some(MagicSeed {
                    before: layout.clone(),
                    before_locks: locks.clone(),
                    locks: action_ui::optimizer_locks(&first, action_ui::action_default_locks(&first)),
                    root: first_root,
                    swaps: Vec::new(),
                });
                state.archive = Some(result);
                state.selected = MAGIC_SETTING_ROWS;
                status = "Saved candidates rescored with this corpus and these weights.".into();
            }
            Event::Char('S') => {
                if let Some(archive) = &state.archive {
                    status = match crate::magic_archive::save(archive, &corpus.name, weights) {
                        Ok(path) => format!("Saved candidates: {}", path.display()),
                        Err(error) => format!("Save failed: {error}"),
                    };
                }
            }
            Event::Char('c') => {
                if let (Some(archive), Some(entry)) = (&state.archive, chosen) {
                    let mut detail = Canvas::new(term.width(), 100);
                    let mut end = grouped_metric_table(
                        &mut detail,
                        0,
                        &archive.before.metrics,
                        &entry.metrics,
                        &entry.raw,
                        &entry.totals,
                        term.decimals(),
                        &EDITOR_TABLE_GROUPS,
                        true,
                        false,
                    );
                    end = finger_table(
                        &mut detail,
                        end + 1,
                        &archive.before.metrics,
                        &entry.metrics,
                        term.decimals(),
                    );
                    end = magic_score_changes(
                        &mut detail,
                        0,
                        end + 1,
                        &archive.before.metrics,
                        &entry.metrics,
                        weights,
                    );
                    let lines: Vec<String> = (0..end)
                        .map(|row| {
                            detail.cells[row * detail.w..(row + 1) * detail.w]
                                .iter()
                                .map(|cell| cell.ch)
                                .collect::<String>()
                                .trim_end()
                                .to_owned()
                        })
                        .collect();
                    help_popup(term, &c, "Candidate details", &lines)?;
                }
            }
            Event::Char('v') => {
                if let Some(entry) = chosen {
                    let candidate = entry.layout.clone();
                    let root = entry.root;
                    let label = magic_root_label(&candidate, root);
                    let job_corpus = corpus.clone();
                    let job_weights = weights.clone();
                    let Some(effects) = action_ui::ngram_job(term, move |stop, progress| {
                        crate::magic_design::rule_effects(
                            &candidate, root, &job_corpus, &job_weights, stop, progress,
                        )
                    })?
                    else {
                        continue;
                    };
                    let mut lines = vec![label, String::new()];
                    for effect in effects {
                        lines.push(format!(
                            "After {:?}: types {:?}",
                            char::from(effect.rule.context),
                            char::from(effect.rule.output)
                        ));
                        if effect.direct_pair_saved > 1e-9 {
                            let repaired: Vec<_> = (0..N_METRICS)
                                .filter(|&i| effect.direct_contribution_saved[i] > 1e-9)
                                .map(|i| METRIC_NAMES[i])
                                .collect();
                            lines.push(format!(
                                "  Direct pair repair: {}.",
                                repaired.join(", ")
                            ));
                        } else {
                            lines.push(
                                "  No direct pair repair: excluded from new designs.".into(),
                            );
                        }
                        let penalty = if effect.penalty_saved > 1e-9 {
                            format!("Penalty reduced by {:.4} points", effect.penalty_saved)
                        } else if effect.penalty_saved < -1e-9 {
                            format!("Penalty increased by {:.4} points", -effect.penalty_saved)
                        } else {
                            "No penalty reduction".into()
                        };
                        let score = if effect.score_saved > 1e-9 {
                            format!("score improves by {:.4} points", effect.score_saved)
                        } else if effect.score_saved < -1e-9 {
                            format!("score worsens by {:.4} points", -effect.score_saved)
                        } else {
                            "no score change".into()
                        };
                        lines.push(format!("  {penalty}; {score}."));
                        let mut fixes: Vec<_> = (0..N_WEIGHTS)
                            .filter(|&i| {
                                (i >= N_METRICS || !higher_better(i))
                                    && effect.contribution_saved[i] > 1e-9
                            })
                            .collect();
                        fixes.sort_by(|&a, &b| {
                            effect.contribution_saved[b]
                                .total_cmp(&effect.contribution_saved[a])
                                .then(a.cmp(&b))
                        });
                        for i in fixes.into_iter().take(3) {
                            if i < N_METRICS {
                                lines.push(format!("  {}: {} without rule, {} with rule; {:.4} score points saved.", METRIC_NAMES[i], metric_value_text(i, effect.metrics_without[i], 4), metric_value_text(i, effect.metrics_with[i], 4), effect.contribution_saved[i]));
                            } else {
                                lines.push(format!(
                                    "  {}: {:.4} score points saved.",
                                    weight_display_name(i),
                                    effect.contribution_saved[i]
                                ));
                            }
                        }
                        lines.push(String::new());
                    }
                    if lines.len() == 2 {
                        lines.push("No explicit rules; this key uses its fallback.".into());
                    }
                    lines.push("Benefits compare this layout with that one rule removed.".into());
                    help_popup(term, &c, "Magic rules", &lines)?;
                }
            }
            Event::Char('?') => help_popup(
                term,
                &c,
                "Magic tab",
                &[
                    "Space\tFind the best layout for each allowed magic-key position".into(),
                    "j/k ↑/↓\tSelect a setting or candidate".into(),
                    "← / →\tScroll the table's ranker columns ([ranker] columns)".into(),
                    "Enter\tEdit the setting, or apply the candidate to the optimizer".into(),
                    "Click key\tLock or unlock it; a locked magic key keeps its key".into(),
                    "H / U / L\tDefault locks with the home row / unlock all / lock all".into(),
                    "p\tFinger blocks: blocked fingers on both hands cannot hold the magic key".into(),
                    "c / v\tSelected candidate's metrics / rules and their effects".into(),
                    "s / S\tSave the selected candidate to layouts/ / every candidate under layouts/magic/".into(),
                    "o\tOpen saved candidates, rescored with this corpus and these weights".into(),
                    "1–4\tOther optimizer tabs".into(),
                    "".into(),
                    "The search uses the optimizer's layout, locks, corpus, weights, caps,".into(),
                    "workers and Magic rules setting. Adaptive swaps are set aside while it".into(),
                    "runs and come back on the applied candidate; Space on another tab then".into(),
                    "runs the optimizer, which fits swaps and combos as Settings says.".into(),
                ],
            )?,
            _ => {}
        }
    }
}

const MAGIC_FINGER_GROUPS: [(&str, [usize; 2]); 5] = [
    ("Pinky", [0, 7]),
    ("Ring", [1, 6]),
    ("Middle", [2, 5]),
    ("Index", [3, 4]),
    ("Thumb", [8, 9]),
];

fn magic_recovery_root(layout: &action_keys::Layout) -> action_keys::Result<usize> {
    layout
        .slots
        .iter()
        .position(|slot| {
            slot.label == "*" && matches!(slot.binding, action_keys::Binding::Named(_))
        })
        .or_else(|| {
            crate::magic_design::eligible_roots(layout)
                .into_iter()
                .find(|&i| matches!(layout.slots[i].binding, action_keys::Binding::Named(_)))
        })
        .ok_or_else(|| "Recovered candidate has no supported magic key.".into())
}

fn magic_recovered_rules(
    layout: &action_keys::Layout,
    root: usize,
) -> Vec<crate::magic_design::Rule> {
    let mut result = Vec::new();
    let action_keys::Binding::Named(name) = &layout.slots[root].binding else {
        return result;
    };
    if let Some(action) = layout.actions.get(name) {
        if let action_keys::Action::Rules { basis, rules, .. } = action {
            let skip = *basis == action_keys::Basis::SkipPress;
            for (context, emission) in rules {
                if let action_keys::Emission::Text(output) = emission {
                    if context.len() == 1 && output.len() == 1 {
                        result.push(crate::magic_design::Rule {
                            skip,
                            context: context[0],
                            output: output[0],
                            frequency: 0.0,
                        });
                    }
                }
            }
        }
    }
    result
}

/// Toggle which finger groups may hold the magic key.
fn magic_finger_groups(term: &mut Terminal, enabled: &mut [bool; 5]) -> AppResult<()> {
    let mut selected = 0;
    loop {
        let mut c = Canvas::new(term.width(), 10);
        c.text(0, 0, "Magic finger blocks · both hands", FG);
        for (i, (name, _)) in MAGIC_FINGER_GROUPS.iter().enumerate() {
            c.text(
                1,
                i + 2,
                &format!("{name:<8}  Block: {}", if enabled[i] { "off" } else { "on" }),
                FG,
            );
            let row = Rect { x: 0, y: i + 2, w: c.w, h: 1 };
            if i == selected {
                c.row_style(row, Some((42, 42, 42)), false);
            }
            c.hit(row, Action::Item(i));
        }
        c.text(0, 8, "Space/Enter/click toggles · q returns", MUTED);
        term.present(&c, 0)?;
        match term.event()? {
            Event::Escape | Event::Quit | Event::Char('q') => return Ok(()),
            Event::Char('j') | Event::Down => selected = (selected + 1).min(4),
            Event::Char('k') | Event::Up => selected = selected.saturating_sub(1),
            Event::Char(' ')
            | Event::Enter
            | Event::Char('h')
            | Event::Char('l')
            | Event::Left
            | Event::Right => enabled[selected] = !enabled[selected],
            Event::Mouse { x, y, button: 0, release: false, motion: false, .. } => {
                if let Some(Action::Item(i)) = term.hit(&c, x, y, 0) {
                    selected = i;
                    enabled[i] = !enabled[i];
                }
            }
            _ => {}
        }
    }
}

fn magic_score_changes(
    c: &mut Canvas,
    x: usize,
    mut y: usize,
    before: &Metrics,
    after: &Metrics,
    weights: &Weights,
) -> usize {
    let before = breakdown(before, weights);
    let after = breakdown(after, weights);
    c.text(x, y, "Score changes", FG);
    y += 1;
    c.text(x, y, "               Before     After     Change", MUTED);
    y += 1;
    for (name, old, new) in [
        ("Penalties", before.penalty, after.penalty),
        ("− Rewards", before.bonus, after.bonus),
        ("= Score", before.net, after.net),
    ] {
        c.text(
            x,
            y,
            &format!("{name:<13} {old:>9.4} {new:>9.4} {:+10.4}", new - old),
            FG,
        );
        y += 1;
    }
    y += 1;
    let mut changes: Vec<_> = (0..N_WEIGHTS)
        .filter(|&i| (after.contributions[i] - before.contributions[i]).abs() > 1e-9)
        .collect();
    changes.sort_by(|&a, &b| {
        (after.contributions[b] - before.contributions[b])
            .abs()
            .total_cmp(&(after.contributions[a] - before.contributions[a]).abs())
            .then(a.cmp(&b))
    });
    for i in changes {
        let old = before.contributions[i];
        let new = after.contributions[i];
        let delta = new - old;
        c.text(
            x,
            y,
            &format!(
                "{:<13} {old:>9.4} {new:>9.4} {delta:+10.4}",
                weight_display_name(i)
            ),
            if delta < 0.0 { GREEN } else { RED },
        );
        y += 1;
    }
    y
}

#[cfg(test)]
mod magic_tab_tests {
    use super::*;

    fn layout() -> action_keys::Layout {
        action_keys::Layout::parse(
            &crate::action_keys::test_layouts::qwerty(&crate::action_keys::test_layouts::magic(&[
                ("ed", "er"),
                ("er", "ed"),
            ])),
            Path::new("magic-tab.jsonc"),
        )
        .unwrap()
    }

    fn slot_of(layout: &action_keys::Layout, letter: &[u8]) -> usize {
        layout
            .slots
            .iter()
            .position(|slot| slot.binding == action_keys::Binding::Text(letter.to_vec()))
            .unwrap()
    }

    #[test]
    fn a_search_keeps_locks_and_a_locked_magic_key_in_place() {
        let layout = layout().with_spare_keys();
        let mut locks = action_ui::optimizer_locks(&layout, action_ui::action_default_locks(&layout));
        let held = slot_of(&layout, b"s");
        locks[held] = true;
        let groups = [true; 5];
        let magic = MagicSettings::default();
        // Without a magic key, one is added unlocked: the search may move it
        // to any allowed key but locked ones (it starts on the first when
        // its outer key is not allowed), and locked letters stay put.
        let plan = magic_search_plan(&layout, &locks, None, &groups, &magic).unwrap();
        assert!(!plan.locks[plan.root]);
        assert!(plan.allowed.len() > 1);
        let seed_held = slot_of(&plan.seed, b"s");
        assert!(plan.locks[seed_held] && plan.locked_slots.contains(&seed_held));
        assert!(!plan.allowed.contains(&seed_held));
        // Its adaptive swap is set aside for the search.
        assert_eq!(plan.swaps.len(), 1);
        assert!(crate::swap_fit::swaps_of(&plan.seed).is_empty());

        // A locked magic key is the only place the search may put it.
        let mut locked = plan.locks.clone();
        locked[plan.root] = true;
        let fixed = magic_search_plan(&plan.seed, &locked, Some(plan.root), &groups, &magic).unwrap();
        assert_eq!(fixed.allowed, vec![plan.root]);
    }

    #[test]
    fn an_applied_candidate_moves_the_magic_keys_lock_and_restores_swaps() {
        let layout = layout().with_spare_keys();
        let locks = action_ui::optimizer_locks(&layout, action_ui::action_default_locks(&layout));
        let plan =
            magic_search_plan(&layout, &locks, None, &[true; 5], &MagicSettings::default()).unwrap();
        let mut seed_locks = plan.locks.clone();
        seed_locks[plan.root] = true;
        // The candidate moved the magic key onto f's key, and f onto its own.
        let f = slot_of(&plan.seed, b"f");
        let mut moved = plan.seed.clone();
        moved.swap(plan.root, f);
        let summary_metrics = metrics_totals(&Raw::default(), &[1.0; 4]);
        let entry = crate::magic_design::Candidate {
            layout: moved,
            score: 0.0,
            rules_off_score: None,
            rules_off_note: None,
            raw: Raw::default(),
            totals: [1.0; 4],
            metrics: summary_metrics,
            root: f,
            rules: Vec::new(),
            pass: 0,
        };
        let seed = MagicSeed {
            before: layout.clone(),
            before_locks: locks.clone(),
            locks: seed_locks,
            root: plan.root,
            swaps: plan.swaps.clone(),
        };
        let (applied, applied_locks) = magic_applied(&entry, &seed);
        assert!(applied_locks[f] && !applied_locks[plan.root]);
        assert_eq!(crate::swap_fit::swaps_of(&applied), plan.swaps);
    }
}
