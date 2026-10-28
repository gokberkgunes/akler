//! Action-aware entry points. Plain layouts continue to use the original evaluator/search.
use crate::action_keys as ak;

use crate::action_ngrams as ng;

use crate::*;

#[derive(Clone)]
pub(crate) struct Evaluated {
    // Presentation belongs to this evaluated layout, not the current editor.
    labels: Vec<String>,
    pub(crate) model: Model,
    pub(crate) corpus: Corpus,
    pub(crate) raw: Raw,
    pub(crate) metrics: Metrics,
    pub(crate) score: f64,
    pub(crate) counts: Arc<ng::Counts>,
}

pub(crate) fn physical_keys(l: &ak::Layout) -> Vec<Key> {
    l.slots
        .iter()
        .map(|s| {
            let mut k = main_key(0, 0);
            k.row = s.row as _;
            k.col = s.col as _;
            k.row_offset = s.row_offset;
            k.column_offset = s.column_offset;
            k.finger = s.finger as _;
            k.rank = s.rank as _;
            k.hand = s.hand as _;
            k.main = s.main;
            k
        })
        .collect()
}

pub(crate) struct LocalEffort {
    uni: Vec<f64>,
    bi: Vec<f64>,
    sk: Vec<f64>,
    n: usize,
}

impl LocalEffort {
    pub(crate) fn new(l: &ak::Layout, w: &Weights) -> Self {
        let keys = physical_keys(l);
        let n = keys.len();
        let mut uni = vec![0.0; n];
        let mut bi = vec![0.0; n * n];
        let mut sk = vec![0.0; n * n];
        for i in 0..n {
            if keys[i].main {
                uni[i] = 0.02 * home_travel_for(keys[i], &keys)[0];
            }
            for j in 0..n {
                let flags = pair_flags(keys[i], keys[j], i == j);
                for m in 0..N_METRICS {
                    if !aggregate(m) && !higher_better(m) {
                        if flags.bi & bit(m) != 0 {
                            bi[i * n + j] += w.0[m];
                        }
                        if flags.sk & bit(m) != 0 {
                            sk[i * n + j] += w.0[m];
                        }
                    }
                }
            }
        }
        Self { uni, bi, sk, n }
    }

    pub(crate) fn get(&self, a: Option<usize>, b: Option<usize>, next: usize) -> f64 {
        self.uni[next]
            + b.map(|v| self.bi[v * self.n + next]).unwrap_or(0.0)
            + a.map(|v| self.sk[v * self.n + next]).unwrap_or(0.0)
    }

    /// The dedicated optimizer can omit effort evaluation for an unambiguous
    /// literal history only when every lookup and two-press projection is
    /// finite. Check once per search; retain the usual mapper if even an
    /// unused history could overflow.
    pub(crate) fn all_finite(&self) -> bool {
        for a in 0..=self.n {
            for b in 0..=self.n {
                for next in 0..self.n {
                    let value =
                        self.get((a < self.n).then_some(a), (b < self.n).then_some(b), next);
                    if !value.is_finite() || value.abs() > f64::MAX / 2.0 {
                        return false;
                    }
                }
            }
        }
        true
    }
}

fn evaluate(
    l: &ak::Layout,
    c: &ng::NgramCorpus,
    w: &Weights,
    stop: &AtomicBool,
) -> ak::Result<Evaluated> {
    evaluate_progress(l, c, w, stop, &AtomicU64::new(0))
}

pub(crate) fn evaluate_progress(
    l: &ak::Layout,
    c: &ng::NgramCorpus,
    w: &Weights,
    stop: &AtomicBool,
    progress: &AtomicU64,
) -> ak::Result<Evaluated> {
    let mut timing = crate::load_profile::LoadProfile::new("action evaluation");
    let effort = LocalEffort::new(l, w);
    timing.mark("Physical effort tables");
    let counts =
        c.evaluate_with_policy(l, stop, progress, w.alts(), w.combo_policy(), |a, b, k| {
            effort.get(a, b, k)
        })?;
    timing.mark("Detailed mapper and physical counts (nested report)");

    evaluate_counts(l, c, w, counts, &mut timing)
}

fn evaluate_counts(
    l: &ak::Layout,
    c: &ng::NgramCorpus,
    w: &Weights,
    counts: ng::Counts,
    timing: &mut crate::load_profile::LoadProfile,
) -> ak::Result<Evaluated> {
    evaluate_counts_shared(l, c, w, Arc::new(counts), timing)
}

fn evaluation_adapter(
    l: &ak::Layout,
    c: &ng::NgramCorpus,
    w: &Weights,
    counts: &ng::Counts,
    timing: &mut crate::load_profile::LoadProfile,
) -> ak::Result<(Model, Corpus)> {
    if counts.presses == 0.0 {
        return Err("no decoded keypresses".into());
    }
    let n = l.slots.len();
    let symbols: Vec<u8> = (33u8..=126)
        .filter(|b| !b.is_ascii_alphabetic())
        .take(n)
        .collect();
    if symbols.len() != n {
        return Err("too many keys for the existing metric adapter".into());
    }
    let mut board = board_from_text(
        "q w e r t  y u i o p\na s d f g  h j k l ;\nz x c v b  n m , . /\n",
        &l.path,
    )
    .map_err(|e| e.to_string())?;
    board.name = l.name.clone();
    board.symbols = symbols.clone();
    board.keys = physical_keys(l);
    board.row_stagger = l.row_stagger;
    let model = Model::with_rolls(board, w.rolls());
    timing.mark("Metric geometry and adapter board");
    let physical = [
        &counts.tables[0],
        &counts.tables[1],
        &counts.skip,
        &counts.tables[2],
    ];
    let tables: [Vec<(String, f64)>; 4] = std::array::from_fn(|i| {
        physical[i]
            .iter()
            .map(|(keys, f)| (keys.iter().map(|&k| symbols[k] as char).collect(), *f))
            .collect()
    });
    let masses = std::array::from_fn(|i| tables[i].iter().map(|(_, f)| *f).sum());
    let source = Source {
        name: c.name.clone(),
        path: c.path.clone(),
        include_spacegrams: c.include_spacegrams,
        tables,
        masses,
        warnings: std::iter::once(format!("{}-gram bounded-context magic estimate", c.order))
            .chain(l.source_warnings.iter().cloned())
            .chain((!l.alt_fingers.is_empty()).then(|| {
                if w.3 {
                    "Contextual alternate fingers affect physical statistics after root-key mapping; they do not choose magic winners.".to_string()
                } else {
                    "Contextual alternate-finger effects are disabled.".to_string()
                }
            }))
            .chain((!l.combos.is_empty()).then(|| {
                if w.4 {
                    "Combos enabled: each combo is one chord press of both keys, with movement to and from it once per key. Requires order-4 corpus tables; capped contexts fall back to shorter ones.".into()
                } else {
                    "Combos disabled: ordinary and magic typing is used.".into()
                }
            }))
            .chain(c.warnings.iter().cloned())
            .collect(),
        fingerprint: 0,
    };
    let corpus = model.corpus(&source).map_err(|e| e.to_string())?;
    timing.mark("Physical count adapter and metric corpus");
    Ok((model, corpus))
}

fn evaluate_counts_shared(
    l: &ak::Layout,
    c: &ng::NgramCorpus,
    w: &Weights,
    counts: Arc<ng::Counts>,
    timing: &mut crate::load_profile::LoadProfile,
) -> ak::Result<Evaluated> {
    let (model, corpus) = evaluation_adapter(l, c, w, &counts, timing)?;
    // Normal editing collects only AKLER counters.
    let (raw, totals) = ng::raw_from_counts(l, &counts, w, false);
    debug_assert_eq!(totals.map(f64::to_bits), corpus.totals.map(f64::to_bits));
    let metrics = metrics_totals(&raw, &totals);
    let score = breakdown(&metrics, w).net;
    timing.mark("Raw metrics and score");
    Ok(Evaluated {
        labels: l.slots.iter().map(|slot| slot.label.clone()).collect(),
        model,
        corpus,
        raw,
        metrics,
        score,
        counts,
    })
}

pub(crate) fn evaluate_ranked(
    l: &ak::Layout,
    c: &ng::NgramCorpus,
    w: &Weights,
    summary: &crate::action_summary::Summary,
) -> ak::Result<Evaluated> {
    let mut timing = crate::load_profile::LoadProfile::new("ranked action editor adapter");
    let (model, corpus) = evaluation_adapter(l, c, w, &summary.counts, &mut timing)?;
    debug_assert_eq!(
        corpus.totals.map(f64::to_bits),
        summary.totals.map(f64::to_bits)
    );
    let score = breakdown(&summary.metrics, w).net;
    timing.mark("Reuse ranked physical metrics and counts");
    Ok(Evaluated {
        labels: l.slots.iter().map(|slot| slot.label.clone()).collect(),
        model,
        corpus,
        raw: summary.raw.clone(),
        metrics: summary.metrics.clone(),
        score,
        counts: Arc::clone(&summary.counts),
    })
}

// Keep terminal input live while loading or evaluating cached contexts.
pub(crate) fn ngram_job<T, F>(
    term: &mut Terminal,
    job: F,
) -> AppResult<Option<T>>
where
    T: Send + 'static,
    F: FnOnce(&AtomicBool, &AtomicU64) -> ak::Result<T> + Send + 'static,
{
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancel);
    let progress = Arc::new(AtomicU64::new(0));
    let worker_progress = Arc::clone(&progress);
    let (tx, rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let _ = tx.send(job(&worker_cancel, &worker_progress));
    });
    let run = (|| -> AppResult<Option<T>> {
        loop {
            match rx.try_recv() {
                Ok(result) => return result.map(Some).map_err(|e| e.into()),
                Err(mpsc::TryRecvError::Disconnected) => return Err("n-gram worker stopped".into()),
                Err(mpsc::TryRecvError::Empty) => {}
            }
            let mut canvas = Canvas::new(term.width(), 4);
            progress_bar(&mut canvas, 1, progress.load(Ordering::Relaxed) as f64);
            term.present(&canvas, 0)?;
            if matches!(
                term.event()?,
                Event::Escape | Event::Quit | Event::Char('q')
            ) {
                return Ok(None);
            }
        }
    })();
    cancel.store(true, Ordering::Relaxed);
    if handle.join().is_err() {
        return Err("n-gram worker panicked".into());
    }
    run
}

pub(crate) fn load_corpus_tui(
    term: &mut Terminal,
    path: &Path,
) -> AppResult<Option<ng::NgramCorpus>> {
    crate::session::CorpusSession::new().action_tui(term, path)
}

fn evaluate_tui(
    term: &mut Terminal,
    l: &ak::Layout,
    c: &ng::NgramCorpus,
    w: &Weights,
) -> AppResult<Option<Arc<Evaluated>>> {
    evaluate_tui_shared(term, l, &Arc::new(c.clone()), w)
}

fn evaluate_tui_shared(
    term: &mut Terminal,
    l: &ak::Layout,
    c: &Arc<ng::NgramCorpus>,
    w: &Weights,
) -> AppResult<Option<Arc<Evaluated>>> {
    let layout = l.clone();
    let corpus = Arc::clone(c);
    let weights = w.clone();
    ngram_job(
        term,
        move |stop, progress| {
            evaluate_progress(&layout, &corpus, &weights, stop, progress).map(Arc::new)
        },
    )
}

/// One line naming a layout's magic rules, adaptive swaps and combos, if it
/// has any. Plain repeats such as `e→e` only restate the repeat fallback and
/// are left out.
pub(crate) fn rules_and_combos(layout: &ak::Layout) -> Option<String> {
    rules_line(layout, true)
}

fn rules_line(layout: &ak::Layout, with_swaps: bool) -> Option<String> {
    let mut parts = Vec::new();
    for (slot, key) in layout.slots.iter().enumerate() {
        if !matches!(&key.binding, ak::Binding::Named(_))
            || crate::swap_fit::is_adaptive_key(layout, slot)
        {
            continue;
        }
        let Some((rules, _)) = crate::magic_design::original(layout, slot) else {
            continue;
        };
        let text: Vec<_> = rules
            .iter()
            .filter_map(|(context, emission)| match emission {
                ak::Emission::Text(output) if output != context => Some(format!(
                    "{}→{}",
                    String::from_utf8_lossy(context),
                    String::from_utf8_lossy(output)
                )),
                _ => None,
            })
            .collect();
        if !text.is_empty() {
            parts.push(format!("Magic {}: {}", key.label, text.join(" ")));
        }
    }
    let swaps: Vec<_> = crate::swap_fit::swaps_of(layout)
        .iter()
        .map(|swap| {
            format!(
                "{}:{}{}",
                swap.context as char, swap.a as char, swap.b as char
            )
        })
        .collect();
    if with_swaps && !swaps.is_empty() {
        parts.push(format!("Swaps: {}", swaps.join(" ")));
    }
    let combos: Vec<_> = layout
        .combos
        .iter()
        .filter_map(|value| {
            let Json::Object(fields) = value else {
                return None;
            };
            let Some(Json::String(output)) = fields.get("output") else {
                return None;
            };
            let keys: Vec<String> = match (fields.get("keys"), fields.get("inputs")) {
                (Some(Json::Array(keys)), _) => keys
                    .iter()
                    .filter_map(|key| match key {
                        Json::String(label) => Some(label.clone()),
                        _ => None,
                    })
                    .collect(),
                (None, Some(Json::Array(inputs))) => match inputs.first() {
                    Some(Json::String(text)) => text.chars().map(String::from).collect(),
                    _ => return None,
                },
                _ => return None,
            };
            Some(format!("{}={output}", keys.join("+")))
        })
        .collect();
    if !combos.is_empty() {
        parts.push(format!("Combos: {}", combos.join(" ")));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// Draw `rules_and_combos` at `y`, returning the next free row.
/// Under the optimizer's board: magic rules and combos, then the adaptive
/// swaps and alternate fingers as lines a click turns off (red) or on (green)
/// for this session. `stashed` holds the swaps while they are off; `fit` is
/// how many swaps a run may fit.
fn optimizer_toggle_rows(
    c: &mut Canvas,
    mut y: usize,
    l: &ak::Layout,
    w: &Weights,
    stashed: Option<&[crate::swap_fit::Swap]>,
    fit: usize,
    exact: bool,
) -> usize {
    let toggle = |c: &mut Canvas, y: usize, text: &str, on: bool, command: char| {
        c.text(0, y, &short(text, c.w), if on { GREEN } else { RED });
        let w = text.chars().count().min(c.w);
        c.hit(Rect { x: 0, y, w, h: 1 }, Action::Command(command));
    };
    if let Some(line) = rules_line(l, false) {
        c.text(0, y, &short(&line, c.w), CYAN);
        y += 1;
    }
    let swaps = stashed.map_or_else(|| crate::swap_fit::swaps_of(l), <[_]>::to_vec);
    if !swaps.is_empty() || fit > 0 || stashed.is_some() {
        let names: Vec<_> = swaps
            .iter()
            .map(|swap| {
                format!(
                    "{}:{}{}",
                    swap.context as char, swap.a as char, swap.b as char
                )
            })
            .collect();
        let names = if names.is_empty() {
            "none".into()
        } else {
            names.join(" ")
        };
        let text = match (stashed, fit) {
            (Some(_), _) => format!("Swaps off: {names} · none used or fitted"),
            (None, 0) => format!("Swaps: {names}"),
            (None, fit) => format!("Swaps: {names} · fit {} {fit}", count_bound(exact)),
        };
        toggle(c, y, &text, stashed.is_none(), 'a');
        y += 1;
    }
    let alts = alts_text(l, w);
    if let Some(alts) = alts {
        let text = alts_label(w.3, &alts);
        toggle(c, y, &text, w.3, 'f');
        y += 1;
    }
    y + 1
}

/// The alternate fingers `l` is scored with: its own rules, or else the
/// `[alts]` rules named by the letters on those keys.
fn alts_text(l: &ak::Layout, w: &Weights) -> Option<String> {
    if !l.alt_fingers.is_empty() {
        let mut own = l.clone();
        crate::alt_fingers::unpin(&mut own);
        return Some(format!(
            "{} (the layout's own)",
            crate::alt_fingers::AltRules::letters(&own.alt_fingers)
        ));
    }
    let rules = w.1.alts?.for_layout(l);
    let names = crate::alt_fingers::position_names(
        l.slots
            .iter()
            .filter(|s| s.main)
            .map(|s| (s.row, s.col, s.label.clone())),
    );
    (!rules.is_empty()).then(|| crate::alt_fingers::AltRules::text_with(rules, names))
}

/// The optimizer's alternate-finger switch: green while on, red while off.
pub(crate) fn alts_label(on: bool, source: &str) -> String {
    format!("Alts {}: {source}", if on { "on" } else { "off" })
}

fn rules_and_combos_row(canvas: &mut Canvas, y: usize, layout: &ak::Layout) -> usize {
    match rules_and_combos(layout) {
        Some(line) => {
            canvas.text(0, y, &short(&line, canvas.w), CYAN);
            y + 2
        }
        None => y,
    }
}

/// `rules_and_combos_row` plus the alternate fingers the run scores with,
/// for the running and result screens.
fn rules_and_alts_rows(canvas: &mut Canvas, y: usize, layout: &ak::Layout, w: &Weights) -> usize {
    let Some(alts) = alts_text(layout, w) else {
        return rules_and_combos_row(canvas, y, layout);
    };
    let y = match rules_and_combos(layout) {
        Some(line) => {
            canvas.text(0, y, &short(&line, canvas.w), CYAN);
            y + 1
        }
        None => y,
    };
    canvas.text(
        0,
        y,
        &short(&alts_label(w.3, &alts), canvas.w),
        if w.3 { GREEN } else { RED },
    );
    y + 2
}

pub(crate) fn action_keyboard(
    c: &mut Canvas,
    y: usize,
    l: &ak::Layout,
    original: &ak::Layout,
    locks: Option<&[bool]>,
    selected: Option<usize>,
) -> usize {
    let min = l
        .slots
        .iter()
        .filter(|slot| slot.main)
        .map(|slot| slot.col)
        .min()
        .unwrap_or(0);
    let max = l
        .slots
        .iter()
        .filter(|slot| slot.main)
        .map(|slot| slot.col)
        .max()
        .unwrap_or(9);
    let cols = (max - min + 1) as usize;
    let physical = physical_keys(l);
    let column_shift = i8::from(l.left_outer && !original.left_outer);
    let original_slot = |i: usize| {
        let slot = &l.slots[i];
        original.slots.iter().find(|old| {
            old.main == slot.main
                && old.row == slot.row
                && old.col.checked_add(column_shift) == Some(slot.col)
        })
    };
    let right_col = right_main_column(&physical, |i| {
        original_slot(i).is_some_and(|slot| slot.binding != ak::Binding::Empty)
    });
    let min_offset = l
        .slots
        .iter()
        .filter(|s| s.main)
        .map(|s| s.row_offset)
        .min()
        .unwrap_or(0)
        .min(0);
    let max_offset = l
        .slots
        .iter()
        .filter(|s| s.main)
        .map(|s| s.row_offset)
        .max()
        .unwrap_or(0)
        .max(0);
    let min_column_offset = l
        .slots
        .iter()
        .filter(|s| s.main)
        .map(|s| s.column_offset)
        .min()
        .unwrap_or(0)
        .min(0);
    let max_column_offset = l
        .slots
        .iter()
        .filter(|s| s.main)
        .map(|s| s.column_offset)
        .max()
        .unwrap_or(0)
        .max(0);
    let column_height = stagger_cells(max_column_offset, min_column_offset, 3);
    let gap = 3;
    let fits = |kw: usize| {
        cols * (kw + 1) - 1 + gap + stagger_cells(max_offset, min_offset, kw + 1) <= c.w
    };
    let kw: usize = if fits(7) {
        7
    } else if fits(5) {
        5
    } else {
        4
    };
    let step = kw + 1;
    let width = cols * step - 1 + gap + stagger_cells(max_offset, min_offset, step);
    let x = c.w.saturating_sub(width) / 2;
    let left_thumbs = l
        .slots
        .iter()
        .filter(|slot| !slot.main && slot.hand == 0)
        .count();
    let left_start =
        (x + width / 2).saturating_sub(kw + 2 + left_thumbs.saturating_sub(1) * (kw + 3));
    let right_start = x + width / 2 + 1;
    let mut thumb_index = [0; 2];
    for (i, s) in l.slots.iter().enumerate() {
        let (kx, ky) = if s.main {
            (
                x + (s.col - min) as usize * step
                    + stagger_cells(s.row_offset, min_offset, step)
                    + usize::from(s.col >= right_col) * gap,
                y + s.row as usize * 3 + stagger_cells(s.column_offset, min_column_offset, 3),
            )
        } else {
            let hand = s.hand as usize;
            let start = if hand == 0 { left_start } else { right_start };
            let position = (start + thumb_index[hand] * (kw + 3), y + 9 + column_height);
            thumb_index[hand] += 1;
            position
        };
        let locked = locks.is_some_and(|v| v.get(i).copied().unwrap_or(true));
        let color = if original_slot(i).is_none_or(|slot| s.binding != slot.binding) {
            BLUE
        } else if locked {
            YELLOW
        } else {
            finger_tint(s.finger)
        };
        let r = Rect {
            x: kx,
            y: ky,
            w: kw,
            h: 3,
        };
        c.boxed(
            r,
            if locked {
                YELLOW
            } else {
                finger_tint(s.finger)
            },
        );
        let label = if s.binding == ak::Binding::Text(vec![b' ']) {
            "␣".to_owned()
        } else {
            short(&s.label, kw - 2)
        };
        c.center(kx + 1, ky + 1, kw - 2, &label, color);
        if selected == Some(i) {
            c.put(kx + 1, ky + 1, '›', FG);
        }
        c.hit(r, Action::Key(i));
    }
    y + column_height
        + if l.slots.iter().any(|slot| !slot.main) {
            12
        } else {
            9
        }
}

fn action_mana_stats(
    layout: &ak::Layout,
    evaluation: &Evaluated,
    weights: &Weights,
) -> mana2_metrics::Stats {
    // The winning physical presses are already present in `counts`. Rebuild
    // only Mana's compact counters; changing a weight or display mode must not
    // run the action mapper again.
    let (raw, totals) = ng::raw_from_counts(layout, &evaluation.counts, weights, true);
    ng::mana_stats(
        raw.1
            .as_ref()
            .expect("Mana counters requested while collecting Mana statistics"),
        totals,
        evaluation.counts.speed_populations,
    )
}

fn action_frame(
    term: &Terminal,
    title: &str,
    l: &ak::Layout,
    base: &ak::Layout,
    b: &Evaluated,
    a: &Evaluated,
    w: &Weights,
    locks: Option<&[bool]>,
    selected: Option<usize>,
    status: &str,
    latest_comparison: Option<bool>,
    scoring_mode: Option<&str>,
    toggles: Option<(Option<&[crate::swap_fit::Swap]>, usize, bool)>,
) -> Canvas {
    let mut c = Canvas::new(term.width(), 150);
    let controls = match latest_comparison {
        Some(true) => format!(
            "m stats: {} | ? help | q back | compare: latest edit (d)",
            term.stats_view_name()
        ),
        Some(false) => format!(
            "m stats: {} | ? help | q back | compare: baseline (d)",
            term.stats_view_name()
        ),
        None if title == "Optimizer" => "Space run | m magic | ? help | q back".to_owned(),
        None => "? help | q back".to_owned(),
    };
    header(&mut c, title, &a.corpus.name, &l.name, "");
    let mut y = action_keyboard(&mut c, 2, l, base, locks, selected) + 1;
    y = match toggles {
        Some((stashed, fit, exact)) => optimizer_toggle_rows(&mut c, y, l, w, stashed, fit, exact),
        None => rules_and_combos_row(&mut c, y, l),
    };
    let limited = if a.corpus.warnings.iter().any(|warning| {
        warning.starts_with("Approximate")
            || warning.starts_with("Combos")
            || warning.starts_with("spark/1")
    }) {
        " · limited contexts (i info)"
    } else {
        ""
    };
    let aklgg_scoring = scoring_mode == Some("aklgg");
    if title == "Optimizer" && term.optimizer_weights {
        if aklgg_scoring {
            y = physical_weight_rows(&mut c, y, &w.2);
        } else {
            y = optimizer_metric_cards(
                &mut c,
                y,
                &b.metrics,
                &a.metrics,
                &a.raw,
                &a.corpus,
                w,
                term.decimals(),
                term.simple_stats,
                true,
            );
        }
    } else if title == "Optimizer" {
        if aklgg_scoring {
            let before = action_mana_stats(base, b, w);
            let after = action_mana_stats(l, a, w);
            y = aklgg_stat_table(&mut c, y, &before, &after, term.decimals());
        } else {
            y = optimizer_metric_cards(
                &mut c,
                y,
                &b.metrics,
                &a.metrics,
                &a.raw,
                &a.corpus,
                w,
                term.decimals(),
                term.simple_stats,
                false,
            );
        }
    } else if title == "Editor" {
        y = editor_metric_cards(
            &mut c,
            y,
            &b.metrics,
            &a.metrics,
            &a.raw,
            &a.corpus,
            term.decimals(),
            term.simple_stats,
        );
    }
    if title != "Editor" && title != "Optimizer" && !limited.is_empty() {
        c.text(0, y, "Source notes available (i info)", YELLOW);
        y += 2;
    }
    if title != "Editor" && title != "Optimizer" {
        y = grouped_metric_cards(
            &mut c,
            y,
            &b.metrics,
            &a.metrics,
            &a.raw,
            &a.corpus,
            term.decimals(),
            term.simple_stats,
        );
    }
    if !aklgg_scoring {
        y = finger_table(&mut c, y + 1, &b.metrics, &a.metrics, term.decimals());
    }
    if aklgg_scoring {
        let before = action_mana_stats(base, b, w);
        let after = action_mana_stats(l, a, w);
        y = score_panel(
            &mut c,
            y + 1,
            &mana_breakdown(&before, w),
            &mana_breakdown(&after, w),
            None,
            term.decimals(),
        );
        c.text(0, y, "Scoring: aklgg weights", MUTED);
        y += 1;
    } else {
        y = score_panel(
            &mut c,
            y + 1,
            &breakdown(&b.metrics, w),
            &breakdown(&a.metrics, w),
            None,
            term.decimals(),
        );
    }
    if (title == "Editor" && !l.alt_fingers.is_empty()) || title == "Optimizer" {
        y = typing_policy_row(&mut c, y, w);
    }
    if (title == "Editor" && !l.combos.is_empty()) || title == "Optimizer" {
        y = combo_policy_row(&mut c, y, w);
    }
    if title == "Editor" {
        y = spacegram_policy_row(&mut c, y, a.corpus.include_spacegrams);
    }
    c.text(0, y, &short(status, c.w), CYAN);
    c.text(0, y + 1, &short(&controls, c.w), MUTED);
    c.h = y + 3;
    c
}

fn trace_view(term: &mut Terminal, l: &ak::Layout, w: &Weights, physical: bool) -> AppResult<()> {
    let title = if physical { "Press labels" } else { "Text" };
    let Some(text) = input_box(term, title, "", "")? else {
        return Ok(());
    };
    let effort = LocalEffort::new(l, w);
    let result = if physical {
        let keys = text
            .split_whitespace()
            .map(|label| {
                l.slots
                    .iter()
                    .position(|s| s.label == label || s.label.trim_start_matches('@') == label)
                    .ok_or_else(|| format!("unknown key {label}"))
            })
            .collect::<ak::Result<Vec<_>>>();
        keys.and_then(|keys| ak::trace_keys(l, &keys))
    } else {
        let stop = AtomicBool::new(false);
        ak::decode(
            l,
            text.as_bytes(),
            ak::DEFAULT_STATE_LIMIT,
            &stop,
            |a, b, k| effort.get(a, b, k),
        )
    };
    let lines = match result {
        Err(e) => vec![e],
        Ok(steps) => {
            let mut lines = vec!["Key       Output            Text range     Resolution".into()];
            for s in steps {
                lines.push(format!(
                    "{:<9} {:<17} {:>4}..{:<5} {}",
                    l.slots[s.key].label,
                    ak::quote(&s.output),
                    s.start,
                    s.end,
                    s.reason
                ));
            }
            lines
        }
    };
    info_page(term, title, &lines)
}

type ContributorKeys = Vec<(String, Option<usize>)>;

fn contributor_keys(before: &Evaluated, after: &Evaluated) -> [ContributorKeys; 2] {
    let mut duplicates = BTreeSet::new();
    for evaluation in [before, after] {
        let mut seen = BTreeSet::new();
        for label in &evaluation.labels {
            if !seen.insert(label) {
                duplicates.insert(label.as_str());
            }
        }
    }
    [before, after].map(|evaluation| {
        evaluation
            .labels
            .iter()
            .enumerate()
            .map(|(slot, label)| {
                (
                    label.clone(),
                    duplicates.contains(label.as_str()).then_some(slot),
                )
            })
            .collect()
    })
}

fn contributor_name(keys: &ContributorKeys, skip: bool) -> String {
    keys.iter()
        .map(|(label, duplicate_slot)| {
            let mut name = if label.contains(char::is_whitespace)
                || label.contains('#')
                || label.contains('"')
            {
                ak::quote(label.as_bytes())
            } else {
                label.clone()
            };
            if let Some(slot) = duplicate_slot {
                name.push_str(&format!("#{slot}"));
            }
            name
        })
        .collect::<Vec<_>>()
        .join(if skip { " _ " } else { " " })
}

fn action_contributor_rows(
    m: usize,
    _l: &ak::Layout,
    before: &Evaluated,
    after: &Evaluated,
) -> Vec<(String, f64, f64)> {
    let mut contributions = BTreeMap::<(ContributorKeys, Vec<usize>), (f64, f64)>::new();
    let keys = contributor_keys(before, after);
    let unigram = matches!(METRIC_NAMES[m], "TRAVEL" | "VTRAVEL" | "LTRAVEL");
    let count_kind = if unigram {
        0
    } else if is_rhythm(m) {
        2
    } else {
        1
    };
    let pattern_kind = if unigram {
        0
    } else if is_rhythm(m) {
        3
    } else if is_skip(m) {
        2
    } else {
        1
    };
    let patterns = |evaluation: &Evaluated| -> ng::FingerTable {
        let effective = if is_skip(m) {
            evaluation.counts.finger_skip.as_ref()
        } else {
            evaluation
                .counts
                .finger_tables
                .as_ref()
                .map(|tables| &tables[count_kind])
        };
        if let Some(table) = effective {
            return table.clone();
        }
        let physical = if is_skip(m) {
            &evaluation.counts.skip
        } else {
            &evaluation.counts.tables[count_kind]
        };
        physical
            .iter()
            .map(|(slots, &frequency)| {
                (
                    ng::FingerPattern {
                        slots: slots.to_vec(),
                        fingers: slots
                            .iter()
                            .map(|&slot| evaluation.model.geometry.keys[slot].finger)
                            .collect(),
                    },
                    frequency,
                )
            })
            .collect()
    };
    for (side, evaluation) in [before, after].into_iter().enumerate() {
        let denominator = denominator(m, &evaluation.raw, &evaluation.corpus);
        for (pattern, frequency) in patterns(evaluation) {
            let unit = ng::pattern_raw(&evaluation.model.geometry, pattern_kind, &pattern);
            let mass = unit.0[m];
            if mass <= 0.0 {
                continue;
            }
            let value = pct(frequency * mass, denominator);
            if value == 0.0 {
                continue;
            }
            let names = pattern
                .slots
                .iter()
                .map(|&i| keys[side][i].clone())
                .collect();
            let alternate_fingers = if pattern
                .slots
                .iter()
                .zip(&pattern.fingers)
                .any(|(&slot, &finger)| evaluation.model.geometry.keys[slot].finger != finger)
            {
                pattern.fingers
            } else {
                Vec::new()
            };
            let values = contributions.entry((names, alternate_fingers)).or_default();
            if side == 0 {
                values.0 += value;
            } else {
                values.1 += value;
            }
        }
    }
    let mut rows: Vec<_> = contributions
        .into_iter()
        .map(|((keys, fingers), (before, after))| {
            let mut label = contributor_name(&keys, is_skip(m));
            if !fingers.is_empty() {
                label.push_str(" (");
                label.push_str(
                    &fingers
                        .iter()
                        .map(|&finger| FINGER_NAMES[finger])
                        .collect::<Vec<_>>()
                        .join(" "),
                );
                label.push(')');
            }
            (label, before, after)
        })
        .collect();
    rows.sort_by(|a, b| b.2.total_cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
    rows
}

// Search options live on a separate screen so the optimizer's main view can
// use the same compact statistics dashboard as the editor.
/// Settings-tab id of `[magic] max_rules`, shown while magic search is on.
const MAGIC_RULES_SETTING: usize = 18;
/// Settings-tab id of `[combos] max`; 0 leaves combos as they are.
const COMBOS_SETTING: usize = 19;
/// Settings-tab id of `[adaptive] max`; 0 leaves adaptive swaps as they are.
const SWAPS_SETTING: usize = 20;
/// Settings-tab ids of `[combos] exact` and `[adaptive] exact`.
const COMBOS_EXACT_SETTING: usize = 22;
const SWAPS_EXACT_SETTING: usize = 23;

/// Toggle `[combos] exact` (`combos`) or `[adaptive] exact` and save it.
fn toggle_exact(combos: bool, exact: &mut bool) -> AppResult<()> {
    let mut config = load_app_config()?;
    *exact = !*exact;
    if combos {
        config.combos.exact = *exact;
        save_config_sections(&[("combos", combos_config_text(&config.combos))])?;
    } else {
        config.adaptive.exact = *exact;
        save_config_sections(&[("adaptive", adaptive_config_text(&config.adaptive))])?;
    }
    Ok(())
}

/// Ask for a new adaptive swap count and save it to `[adaptive]`.
fn edit_swap_count(term: &mut Terminal, swaps: &mut usize) -> AppResult<()> {
    let limit = crate::swap_fit::MAX_SWAPS;
    let title = format!("Adaptive swaps to fit (0 off, up to {limit})");
    let Some(value) = input_box(term, &title, "", &swaps.to_string())? else {
        return Ok(());
    };
    let value = value
        .trim()
        .parse()
        .ok()
        .filter(|count| *count <= limit)
        .ok_or_else(|| format!("adaptive swaps must be 0-{limit}"))?;
    let mut config = load_app_config()?;
    config.adaptive.max_swaps = value;
    save_config_sections(&[("adaptive", adaptive_config_text(&config.adaptive))])?;
    *swaps = value;
    Ok(())
}

/// How many pieces a fit may add: "exactly" with `exact`, else "up to".
fn count_bound(exact: bool) -> &'static str {
    if exact {
        "exactly"
    } else {
        "up to"
    }
}

/// Status text for adaptive swap fitting.
fn swap_label(swaps: usize, exact: bool) -> String {
    if swaps == 0 {
        " · Swaps: off".into()
    } else {
        format!(" · Swaps: {} {swaps}", count_bound(exact))
    }
}

/// Status text for combo fitting; empty while `[typing] combos` is off.
fn combo_label(combos: Option<usize>, exact: bool) -> String {
    match combos {
        None => String::new(),
        Some(0) => " · Combos: off · Settings".into(),
        Some(count) => format!(" · Combos: {} {count}", count_bound(exact)),
    }
}

/// Ask for a new combo count and save it to `[combos]`; the optimizer reads
/// that section when it starts.
fn edit_combo_count(term: &mut Terminal, combos: &mut usize) -> AppResult<()> {
    let limit = crate::combo_fit::MAX_COMBOS;
    let title = format!("Combos to fit (0 off, up to {limit})");
    let Some(value) = input_box(term, &title, "", &combos.to_string())? else {
        return Ok(());
    };
    let value = value
        .trim()
        .parse()
        .ok()
        .filter(|count| *count <= limit)
        .ok_or_else(|| format!("combos must be 0-{limit}"))?;
    let mut config = load_app_config()?;
    config.combos.max_combos = value;
    save_config_sections(&[("combos", combos_config_text(&config.combos))])?;
    *combos = value;
    Ok(())
}

/// Ask for a new magic rule count and save it to `[magic]`; the magic search
/// reads that section when it starts.
fn edit_magic_rules(term: &mut Terminal, rules: &mut usize) -> AppResult<()> {
    let limit = crate::magic_design::MAX_RULES;
    let title = format!("Magic rules (1-{limit})");
    let Some(value) = input_box(term, &title, "", &rules.to_string())? else {
        return Ok(());
    };
    let value = value
        .trim()
        .parse()
        .ok()
        .filter(|count| (1..=limit).contains(count))
        .ok_or_else(|| format!("magic rules must be 1-{limit}"))?;
    let mut config = load_app_config()?;
    config.magic.max_rules = value;
    save_config_sections(&[("magic", magic_config_text(&config.magic))])?;
    *rules = value;
    Ok(())
}

fn action_setup_frame(
    width: usize,
    stats_view: &str,
    layout: &ak::Layout,
    baseline: &ak::Layout,
    corpus: &ng::NgramCorpus,
    weights: &Weights,
    locks: &[bool],
    settings: &SearchSettings,
    magic_rules: Option<usize>,
    combos: Option<usize>,
    swaps: usize,
    exact: (bool, bool),
    status: &str,
    selected: Option<usize>,
) -> Canvas {
    let mut c = Canvas::optimizer(width, 96);
    header(
        &mut c,
        "Search settings",
        &corpus.name,
        &layout.name,
        &format!("e done | Space run | w weights | v stats: {stats_view} | ? help"),
    );
    let mut y = action_keyboard(&mut c, 3, layout, baseline, Some(locks), selected) + 1;
    c.text(
        0,
        y,
        &format!(
            "Locked {}   Free {}",
            locks.iter().filter(|&&value| value).count(),
            locks.iter().filter(|&&value| !value).count(),
        ),
        MUTED,
    );
    y += 2;

    y = typing_policy_row(&mut c, y, weights);
    y = combo_policy_row(&mut c, y, weights) + 1;
    let mut fields = settings_labels(settings);
    if let Some(rules) = magic_rules {
        fields.push((MAGIC_RULES_SETTING, "Magic rules".into(), rules.to_string()));
    }
    let yes_no = |value: bool| if value { "yes" } else { "no" }.to_string();
    // Combos fit only while they are scored (`[typing] combos`, the row above).
    let combos = match combos {
        None => "off (typing)".to_string(),
        Some(0) => "off".to_string(),
        Some(count) => count.to_string(),
    };
    fields.push((COMBOS_SETTING, "Combos".into(), combos));
    fields.push((COMBOS_EXACT_SETTING, "Combos exact".into(), yes_no(exact.0)));
    let swaps = if swaps == 0 {
        "off".to_string()
    } else {
        swaps.to_string()
    };
    fields.push((SWAPS_SETTING, "Adaptive swaps".into(), swaps));
    fields.push((SWAPS_EXACT_SETTING, "Swaps exact".into(), yes_no(exact.1)));
    let columns = if c.w >= 80 { 4 } else { 2 };
    let column_width = c.w / columns;
    for (cell, (i, label, value)) in fields.iter().enumerate() {
        let x = cell % columns * column_width;
        let row = y + cell / columns * 2;
        c.text(x, row, &short(label, column_width.saturating_sub(1)), MUTED);
        c.text(
            x,
            row + 1,
            &short(value, column_width.saturating_sub(1)),
            CYAN,
        );
        c.hit(
            Rect {
                x,
                y: row,
                w: column_width.saturating_sub(1),
                h: 2,
            },
            Action::Setting(*i),
        );
    }
    y += (fields.len() + columns - 1) / columns * 2 + 1;

    let mixture = if settings.mix.values().any(|&share| share > 0.0) {
        settings
            .mix
            .iter()
            .filter(|(_, share)| **share > 0.0)
            .map(|(name, share)| format!("{name}:{share}"))
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        corpus.name.clone()
    };
    c.text(0, y, &short(&mixture, c.w), MUTED);
    c.hit(
        Rect {
            x: 0,
            y,
            w: c.w,
            h: 1,
        },
        Action::Command('x'),
    );
    y += 1;
    c.text(
        0,
        y,
        &format!("{}-gram magic estimate", corpus.order),
        MUTED,
    );
    if !corpus.warnings.is_empty() {
        y += 1;
        c.text(
            0,
            y,
            "Limited contexts · i for retained counts and frequency",
            YELLOW,
        );
    }
    y += 2;
    c.text(0, y, &short(status, c.w), CYAN);
    c.text(
        0,
        y + 1,
        &short("Space run · m magic · ? help · q back", c.w),
        MUTED,
    );
    c.h = y + 2;
    c
}

fn action_detail_frame(
    width: usize,
    m: usize,
    layout: &ak::Layout,
    baseline: &ak::Layout,
    corpus: &str,
    rows: &[(String, f64, f64)],
    totals: (f64, f64),
    denominators: (f64, f64),
    sort_change: bool,
    all: bool,
    merge: bool,
) -> Canvas {
    action_detail_frame_named(
        width,
        m,
        METRIC_NAMES[m],
        layout,
        baseline,
        corpus,
        rows,
        totals,
        denominators,
        sort_change,
        all,
        merge,
    )
}

fn action_detail_frame_named(
    width: usize,
    m: usize,
    title: &str,
    layout: &ak::Layout,
    baseline: &ak::Layout,
    corpus: &str,
    rows: &[(String, f64, f64)],
    totals: (f64, f64),
    denominators: (f64, f64),
    sort_change: bool,
    all: bool,
    merge: bool,
) -> Canvas {
    let mut c = Canvas::new(width, 64);
    header(&mut c, title, corpus, &layout.name, "");
    let mut y = action_keyboard(&mut c, 2, layout, baseline, None, None) + 1;
    y = metric_detail_intro(&mut c, y, m);
    if (denominators.0 - denominators.1).abs() > DISPLAY_EPSILON {
        for line in wrap_lines(
            &["Change ignores denominator shifts; Before/After keep actual percentages.".into()],
            c.w.max(1),
        ) {
            c.text(0, y, &line, MUTED);
            y += 1;
        }
        y += 1;
    }
    c.text(
        0,
        y,
        &format!(
            "{} → {} {}",
            number(totals.0, DETAIL_DECIMALS),
            number(totals.1, DETAIL_DECIMALS),
            metric_unit(m)
        ),
        FG,
    );
    c.text(
        32,
        y,
        if sort_change {
            "○ before  ● after   change"
        } else {
            "○ before  ● after   contribution"
        },
        MUTED,
    );
    y += 2;
    c.text(0, y, if merge { "Pair" } else { "Keys" }, MUTED);
    c.right(9, y, 9, "Before", MUTED);
    c.right(20, y, 9, "After", MUTED);
    c.right(31, y, 9, "Change", MUTED);
    let maximum = rows.iter().map(|r| r.1.max(r.2)).fold(1e-12, f64::max);
    let plot_width = c.w.saturating_sub(44);
    if plot_width > 0 {
        c.text(43, y, "0", MUTED);
        c.right(
            45,
            y,
            plot_width.saturating_sub(2),
            &format!("{} {}", number(maximum, DETAIL_DECIMALS), metric_unit(m)),
            MUTED,
        );
    }
    y += 1;
    let count = if all { rows.len() } else { rows.len().min(16) };
    for (i, (name, before, after)) in rows.iter().take(count).enumerate() {
        let row = y + i;
        let (compare_before, compare_after) =
            contributor_comparison(*before, *after, denominators.0, denominators.1);
        let color = change_color(m, compare_before, compare_after);
        c.text(0, row, &short(name, 8), FG);
        c.right(9, row, 9, &number(*before, DETAIL_DECIMALS), MUTED);
        c.right(20, row, 9, &number(*after, DETAIL_DECIMALS), color);
        c.right(
            31,
            row,
            9,
            &delta_text(compare_before, compare_after, DETAIL_DECIMALS),
            delta_color(m, compare_before, compare_after),
        );
        contribution_bar(&mut c, 43, row, plot_width, *before, *after, maximum, color);
    }
    y += count;
    if rows.is_empty() {
        c.text(0, y, "No contributing physical sequences", MUTED);
        y += 1;
    }
    if !all {
        let before = (totals.0 - rows.iter().take(count).map(|r| r.1).sum::<f64>()).max(0.0);
        let after = (totals.1 - rows.iter().take(count).map(|r| r.2).sum::<f64>()).max(0.0);
        let (compare_before, compare_after) =
            contributor_comparison(before, after, denominators.0, denominators.1);
        y += 1;
        c.text(0, y, "Other", MUTED);
        c.right(9, y, 9, &number(before, DETAIL_DECIMALS), MUTED);
        c.right(20, y, 9, &number(after, DETAIL_DECIMALS), MUTED);
        c.right(
            31,
            y,
            9,
            &delta_text(compare_before, compare_after, DETAIL_DECIMALS),
            delta_color(m, compare_before, compare_after),
        );
    }
    let footer_y = if all { y + 1 } else { y + 2 };
    c.h = metric_detail_footer(&mut c, footer_y, m, sort_change, merge, Some(all), None);
    c
}

fn action_contributors(
    term: &mut Terminal,
    m: usize,
    layout: &ak::Layout,
    baseline: &ak::Layout,
    before: &Evaluated,
    after: &Evaluated,
) -> AppResult<()> {
    let source = action_contributor_rows(m, layout, before, after);
    let totals = (before.metrics.v[m], after.metrics.v[m]);
    let denominators = (
        denominator(m, &before.raw, &before.corpus),
        denominator(m, &after.raw, &after.corpus),
    );
    let mut scroll = 0;
    let mut sort_change = false;
    let mut all = false;
    let mut merge = false;
    loop {
        let mut rows = source.clone();
        if merge {
            let mut pairs: BTreeMap<String, (f64, f64)> = BTreeMap::new();
            for (name, before, after) in rows {
                let mut keys: Vec<_> = name.split_whitespace().collect();
                let reverse: Vec<_> = keys.iter().rev().copied().collect();
                if reverse < keys {
                    keys = reverse;
                }
                let entry = pairs.entry(keys.join(" ")).or_default();
                entry.0 += before;
                entry.1 += after;
            }
            rows = pairs
                .into_iter()
                .map(|(name, (b, a))| (name, b, a))
                .collect();
        }
        rows.sort_by(|a, b| {
            let order = if sort_change {
                let (a0, a1) = contributor_comparison(a.1, a.2, denominators.0, denominators.1);
                let (b0, b1) = contributor_comparison(b.1, b.2, denominators.0, denominators.1);
                (b1 - b0).abs().total_cmp(&(a1 - a0).abs())
            } else {
                b.2.total_cmp(&a.2)
            };
            order.then_with(|| a.0.cmp(&b.0))
        });
        let c = action_detail_frame(
            term.width(),
            m,
            layout,
            baseline,
            &after.corpus.name,
            &rows,
            totals,
            denominators,
            sort_change,
            all,
            merge,
        );
        term.present(&c, scroll)?;
        let event = term.event()?;
        if scroll_event(&event, &mut scroll, c.h, term.size.1) {
            continue;
        }
        match event {
            Event::Escape | Event::Quit | Event::Char('q') | Event::Enter => return Ok(()),
            Event::Char('d') => {
                sort_change = !sort_change;
                scroll = 0;
            }
            Event::Char('a') => {
                all = !all;
                scroll = 0;
            }
            Event::Char('b') if !is_rhythm(m) => {
                merge = !merge;
                scroll = 0;
            }
            Event::Char('?') => {
                let mut help = vec![
                    METRIC_HELP[m].into(),
                    "d: Sort by contribution or absolute change.".into(),
                    "a: Show the top 16 contributions or all contributions.".into(),
                ];
                if !is_rhythm(m) {
                    help.push("b: Combine each physical sequence with its reverse.".into());
                }
                help.extend([
                    "q or Esc: Return to the editor.".into(),
                    roll_settings_label(after.model.geometry.rolls),
                    "Keys name the physical presses, including action keys, not emitted text. Before and After use their own layout's labels.".into(),
                    "Before is the active comparison layout; After is the current layout. Blue keys have moved.".into(),
                    "Top 16 contributions are shown with the remainder in Other.".into(),
                    "SRAF shows clean credit; ALT counts all non-thumb LRL/RLR triples. Roll thumb inclusion and movement filters follow [rolls]. Both-zero rows are omitted; — means unchanged.".into(),
                ]);
                help_popup(term, &c, METRIC_NAMES[m], &help)?;
            }
            _ => {}
        }
    }
}

fn choose_corpus(term: &mut Terminal) -> AppResult<Option<PathBuf>> {
    select_source_path(term, false)
}

pub(crate) fn action_editor(
    term: &mut Terminal,
    l: ak::Layout,
    corpus: ng::NgramCorpus,
    optimize: bool,
) -> AppResult<()> {
    if optimize {
        return action_optimizer(term, l, corpus);
    }

    let mut timing = crate::load_profile::LoadProfile::new("action editor initialization");
    let w = load_weights(Path::new(WEIGHTS_FILE))?;
    let corpus = Arc::new(corpus);
    let Some(baseline) = evaluate_tui_shared(term, &l, &corpus, &w)? else {
        return Ok(());
    };
    timing.mark("Baseline evaluation (nested report)");
    action_editor_core(term, l, corpus, w, baseline, timing)
}

pub(crate) fn action_editor_prepared(
    term: &mut Terminal,
    l: ak::Layout,
    corpus: Arc<ng::NgramCorpus>,
    w: Weights,
    baseline: Arc<Evaluated>,
) -> AppResult<()> {
    let mut timing = crate::load_profile::LoadProfile::new("prepared action editor initialization");
    timing.mark("Reuse ranked mapping and metrics");
    action_editor_core(term, l, corpus, w, baseline, timing)
}

fn toggle_editor_policy(
    term: &mut Terminal,
    original: &ak::Layout,
    layout: &ak::Layout,
    corpus: &Arc<ng::NgramCorpus>,
    weights: &Weights,
    combos: bool,
) -> AppResult<Option<(Weights, Arc<Evaluated>, Arc<Evaluated>)>> {
    let mut next = weights.clone();
    if combos {
        next.4 = !next.4;
    } else {
        next.3 = !next.3;
    }
    let Some(before) = evaluate_tui_shared(term, original, corpus, &next)? else {
        return Ok(None);
    };
    let Some(after) = evaluate_tui_shared(term, layout, corpus, &next)? else {
        return Ok(None);
    };
    save_config_sections(&[("typing", typing_config_text(&next))])?;
    Ok(Some((next, before, after)))
}

fn toggle_editor_spacegrams(
    term: &mut Terminal,
    original: &ak::Layout,
    layout: &ak::Layout,
    corpus: &Arc<ng::NgramCorpus>,
    weights: &Weights,
) -> AppResult<Option<(Arc<ng::NgramCorpus>, Arc<Evaluated>, Arc<Evaluated>)>> {
    let mut limits = load_app_config()?.ngrams;
    limits.include_spacegrams = !corpus.include_spacegrams;
    let enabled = limits.include_spacegrams;
    let path = corpus.path.clone();
    let raw = fs::read_to_string(&path)?;
    let Some(next) = ngram_job(
        term,
        move |stop, progress| {
            ng::NgramCorpus::from_text_progress_with_limits(&raw, &path, limits, stop, progress)
        },
    )?
    else {
        return Ok(None);
    };
    let next = Arc::new(next);
    let Some(before) = evaluate_tui_shared(term, original, &next, weights)? else {
        return Ok(None);
    };
    let Some(after) = evaluate_tui_shared(term, layout, &next, weights)? else {
        return Ok(None);
    };
    save_spacegram_setting(enabled)?;
    Ok(Some((next, before, after)))
}

fn action_editor_core(
    term: &mut Terminal,
    mut l: ak::Layout,
    mut corpus: Arc<ng::NgramCorpus>,
    mut w: Weights,
    mut baseline: Arc<Evaluated>,
    mut timing: crate::load_profile::LoadProfile,
) -> AppResult<()> {
    let original = l.clone();
    let mut current = baseline.clone();
    let mut previous = current.clone();
    let mut compare_latest = false;
    let mut undo: Vec<(ak::Layout, Option<Arc<Evaluated>>)> = Vec::new();
    let mut redo: Vec<(ak::Layout, Option<Arc<Evaluated>>)> = Vec::new();
    let (mut selected, mut drag) = (None, None);
    let mut scroll = 0;
    let mut status = String::new();
    timing.mark("Shared current state, locks and editor state");
    let mut opening = Some(timing);
    loop {
        let c = action_frame(
            term,
            "Editor",
            &l,
            &original,
            if compare_latest { &previous } else { &baseline },
            &current,
            &w,
            None,
            selected.or(drag),
            &status,
            Some(compare_latest),
            None,
            None,
        );
        term.present(&c, scroll)?;
        if let Some(mut timing) = opening.take() {
            timing.mark("First UI frame and terminal presentation");
        }
        let e = term.event()?;
        if scroll_event(&e, &mut scroll, c.h, term.size.1) {
            continue;
        }
        let mut swap = None;
        match e {
            Event::Escape if selected.is_some() || drag.is_some() => {
                selected = None;
                drag = None;
            }
            Event::Escape | Event::Quit | Event::Char('q') => return Ok(()),
            Event::Char('.') => term.precise = !term.precise,
            Event::Char('d') => compare_latest = !compare_latest,
            Event::Char('m') => {
                term.toggle_stats_view();
                scroll = 0;
            }
            Event::Char('?') => help_popup(
                term,
                &c,
                "Editor controls",
                &[
                    "Click a statistic\tShow its contributing physical patterns".into(),
                    "Click or drag keys\tSelect two keys to swap their bindings".into(),
                    "d\tToggle isolated changes: compare with the layout before your latest edit.".into(),
                    "m\tSwitch between Detailed jump statistics and Simple FSB/FSS/HSB/HSS statistics.".into(),
                    "u\tUndo the last edit.".into(),
                    "U\tRedo the last undone edit.".into(),
                    "r\tReset the layout to its loaded bindings".into(),
                    "s\tSave the current layout as a new copy".into(),
                    "c\tChoose a different corpus".into(),
                    "z\tInclude or exclude patterns with Space, such as e + Space or Space + t.".into(),
                    "f\tEnable or disable alternate fingers.".into(),
                    "F\tEnable or disable combos.".into(),
                    "w\tEdit a metric weight".into(),
                    "v\tShow layout rules".into(),
                    "i\tShow corpus and source-limit notes".into(),
                    "t\tShow how text is typed.".into(),
                    "p\tTrace physical key presses.".into(),
                    ".\tToggle display precision".into(),
                    "Blue keys\tBindings changed from the loaded layout".into(),
                    "Green means better; red means worse. Normally, compare with the loaded layout. Isolated changes show only the latest edit, including undo or reset.".into(),
                    "Changing corpus, typing settings, or weights starts a fresh latest-edit comparison.".into(),
                ],
            )?,
            Event::Char('i') => show_corpus_info(term, &current.corpus)?,
            Event::Char('v') => crate::layout_rules::show(term, &c, &l, &w)?,
            Event::Char('z') => match toggle_editor_spacegrams(term, &original, &l, &corpus, &w) {
                Ok(Some((next, b, a))) => {
                    corpus = next;
                    baseline = b;
                    previous = a.clone();
                    current = a;
                    invalidate_action_editor_history(&mut undo, &mut redo);
                    status = format!(
                        "Spacegrams {}",
                        if corpus.include_spacegrams {
                            "ON"
                        } else {
                            "OFF"
                        }
                    );
                }
                Ok(None) => {}
                Err(error) => status = format!("Spacegrams unchanged: {error}"),
            },
            Event::Char('t') => trace_view(term, &l, &w, false)?,
            Event::Char('p') => trace_view(term, &l, &w, true)?,
            Event::Char('s') => {
                status = match l.save_new() {
                    Ok(p) => saved_layout_message(&p),
                    Err(e) => e,
                }
            }
            Event::Char('r') => {
                undo.push((l, Some(current.clone())));
                redo.clear();
                previous = current;
                l = original.clone();
                current = baseline.clone();
            }
            Event::Char('u') => {
                if let Some((old, saved)) = undo.last().cloned() {
                    let value = match saved {
                        Some(value) => Some(value),
                        None => evaluate_tui_shared(term, &old, &corpus, &w)?,
                    };
                    if let Some(value) = value {
                        undo.pop();
                        redo.push((l, Some(current.clone())));
                        previous = current;
                        l = old;
                        current = value;
                        status.clear();
                    }
                }
            }
            Event::Char('U') => {
                if let Some((next, saved)) = redo.last().cloned() {
                    let value = match saved {
                        Some(value) => Some(value),
                        None => evaluate_tui_shared(term, &next, &corpus, &w)?,
                    };
                    if let Some(value) = value {
                        redo.pop();
                        undo.push((l, Some(current.clone())));
                        previous = current;
                        l = next;
                        current = value;
                        status.clear();
                    }
                }
            }
            Event::Char('c') => {
                if let Some(p) = choose_corpus(term)? {
                    match load_corpus_tui(term, &p) {
                        Ok(Some(next)) => {
                            let next = Arc::new(next);
                            if let Some(b) = evaluate_tui_shared(term, &original, &next, &w)? {
                                if let Some(a) = evaluate_tui_shared(term, &l, &next, &w)? {
                                    corpus = next;
                                    baseline = b;
                                    previous = a.clone();
                                    current = a;
                                    invalidate_action_editor_history(&mut undo, &mut redo);
                                    status.clear();
                                }
                            }
                        }
                        Ok(None) => {}
                        Err(e) => status = e.to_string(),
                    }
                }
            }
            Event::Char(ch @ ('f' | 'F'))
                if ch == 'f' && !l.alt_fingers.is_empty() || ch == 'F' && !l.combos.is_empty() =>
            {
                let label = if ch == 'F' {
                    "Combos"
                } else {
                    "Alternate fingers"
                };
                match toggle_editor_policy(term, &original, &l, &corpus, &w, ch == 'F') {
                    Ok(Some((next_w, b, a))) => {
                        w = next_w;
                        baseline = b;
                        previous = a.clone();
                        current = a;
                        invalidate_action_editor_history(&mut undo, &mut redo);
                        status = format!(
                            "{label} {}",
                            if if ch == 'F' { w.4 } else { w.3 } {
                                "enabled"
                            } else {
                                "disabled"
                            }
                        );
                    }
                    Ok(None) => {}
                    Err(error) => status = format!("{label} toggle failed: {error}"),
                }
            }
            Event::Char('w') => {
                let ids: Vec<_> = (0..N_WEIGHTS).filter(|&i| !aggregate(i)).collect();
                let names: Vec<_> = ids.iter().map(|&i| WEIGHT_NAMES[i].to_string()).collect();
                if let Some(i) = menu(term, "", &names)? {
                    let mut next_w = w.clone();
                    edit_single_weight(term, &mut next_w, ids[i])?;
                    if let Some(b) = evaluate_tui_shared(term, &original, &corpus, &next_w)? {
                        if let Some(a) = evaluate_tui_shared(term, &l, &corpus, &next_w)? {
                            w = next_w;
                            baseline = b;
                            previous = a.clone();
                            current = a;
                            invalidate_action_editor_history(&mut undo, &mut redo);
                        }
                    }
                }
            }
            Event::Mouse {
                x,
                y,
                button: 0,
                release: false,
                motion: false,
                ..
            } => match term.hit(&c, x, y, scroll) {
                Some(Action::Key(i)) => drag = Some(i),
                Some(Action::Command('v')) => crate::layout_rules::show(term, &c, &l, &w)?,
                Some(Action::Command('z')) => {
                    match toggle_editor_spacegrams(term, &original, &l, &corpus, &w) {
                        Ok(Some((next, b, a))) => {
                            corpus = next;
                            baseline = b;
                            previous = a.clone();
                            current = a;
                            invalidate_action_editor_history(&mut undo, &mut redo);
                            status = format!(
                                "Spacegrams {}",
                                if corpus.include_spacegrams {
                                    "ON"
                                } else {
                                    "OFF"
                                }
                            );
                        }
                        Ok(None) => {}
                        Err(error) => status = format!("Spacegrams unchanged: {error}"),
                    }
                }
                Some(Action::Command(ch @ ('f' | 'F'))) => {
                    let label = if ch == 'F' {
                        "Combos"
                    } else {
                        "Alternate fingers"
                    };
                    match toggle_editor_policy(term, &original, &l, &corpus, &w, ch == 'F') {
                        Ok(Some((next_w, b, a))) => {
                            w = next_w;
                            baseline = b;
                            previous = a.clone();
                            current = a;
                            invalidate_action_editor_history(&mut undo, &mut redo);
                            status = format!(
                                "{label} {}",
                                if if ch == 'F' { w.4 } else { w.3 } {
                                    "enabled"
                                } else {
                                    "disabled"
                                }
                            );
                        }
                        Ok(None) => {}
                        Err(error) => status = format!("{label} toggle failed: {error}"),
                    }
                }
                Some(Action::Metric(m)) => {
                    let comparison = if compare_latest { &previous } else { &baseline };
                    action_contributors(term, m, &l, &original, comparison, &current)?;
                }
                Some(Action::Weight(i)) => {
                    let mut next_w = w.clone();
                    edit_single_weight(term, &mut next_w, i)?;
                    if let Some(b) = evaluate_tui_shared(term, &original, &corpus, &next_w)? {
                        if let Some(a) = evaluate_tui_shared(term, &l, &corpus, &next_w)? {
                            w = next_w;
                            baseline = b;
                            previous = a.clone();
                            current = a;
                            invalidate_action_editor_history(&mut undo, &mut redo);
                        }
                    }
                }
                _ => {}
            },
            Event::Mouse {
                x,
                y,
                button: 0,
                release: true,
                ..
            } => {
                if let Some(from) = drag.take() {
                    if let Some(Action::Key(to)) = term.hit(&c, x, y, scroll) {
                        if from != to {
                            swap = Some((from, to));
                            selected = None;
                        } else if let Some(a) = selected.take() {
                            if a != to {
                                swap = Some((a, to));
                            }
                        } else {
                            selected = Some(to);
                        }
                    }
                }
            }
            _ => {}
        }
        if let Some((a, b)) = swap {
            if l.space(a) || l.space(b) {
                continue;
            }
            let mut next = l.clone();
            next.swap(a, b);
            match evaluate_tui_shared(term, &next, &corpus, &w) {
                Ok(Some(v)) => {
                    undo.push((l, Some(current.clone())));
                    redo.clear();
                    previous = current;
                    l = next;
                    current = v;
                    status.clear();
                }
                Ok(None) => {}
                Err(e) => status = e.to_string(),
            }
        }
    }
}

fn invalidate_action_editor_history(
    undo: &mut [(ak::Layout, Option<Arc<Evaluated>>)],
    redo: &mut [(ak::Layout, Option<Arc<Evaluated>>)],
) {
    for (_, evaluation) in undo.iter_mut().chain(redo.iter_mut()) {
        *evaluation = None;
    }
}

/// Default locks, the same for every design: keys off the main block
/// (thumbs, Space) stay where they are.
pub(crate) fn action_default_locks(layout: &ak::Layout) -> Vec<bool> {
    layout.slots.iter().map(|slot| !slot.main).collect()
}

/// The `H` preset: the default locks plus the home row and actions other than
/// adaptive keys.
pub(crate) fn action_home_locks(layout: &ak::Layout) -> Vec<bool> {
    let homes = home_slot_mask(&physical_keys(layout));
    // Adaptive swap keys move with their letters; other actions stay put.
    layout
        .slots
        .iter()
        .zip(homes)
        .enumerate()
        .map(|(index, (slot, home))| {
            home || !slot.main
                || matches!(slot.binding, ak::Binding::Named(_))
                    && !crate::swap_fit::is_adaptive_key(layout, index)
        })
        .collect()
}

fn action_choose_design(term: &mut Terminal, settings: &mut SearchSettings) -> AppResult<()> {
    if let Some(choice) = menu(
        term,
        "",
        &["Refine".into(), "Random".into(), "Evolve".into()],
    )? {
        settings.design = ["refine", "random", "evolve"][choice].into();
    }
    Ok(())
}

fn choose_optimizer_magic(term: &mut Terminal, layout: &ak::Layout) -> AppResult<Option<usize>> {
    let mut selected = layout.slots.iter().position(|slot| slot.main).unwrap_or(0);
    loop {
        let mut frame = Canvas::optimizer(term.width(), 32);
        header(
            &mut frame,
            "Choose magic key",
            "",
            &layout.name,
            "Click / hjkl / arrows select | Enter accept | Esc cancel",
        );
        action_keyboard(&mut frame, 3, layout, layout, None, Some(selected));
        term.present(&frame, 0)?;
        let event = term.event()?;
        if let Some(Action::Key(slot)) = action_press(term, &frame, &event, 0) {
            selected = slot;
        }
        match event {
            Event::Enter => return Ok(Some(selected)),
            Event::Escape | Event::Quit | Event::Char('q') => return Ok(None),
            Event::Left | Event::Char('h') | Event::Up | Event::Char('k') => {
                selected = (selected + layout.slots.len() - 1) % layout.slots.len();
            }
            Event::Right | Event::Char('l') | Event::Down | Event::Char('j') => {
                selected = (selected + 1) % layout.slots.len();
            }
            _ => {}
        }
    }
}

fn enable_optimizer_magic(
    term: &mut Terminal,
    layout: &mut ak::Layout,
    locks: &mut Vec<bool>,
) -> AppResult<Option<usize>> {
    let Some(slot) = choose_optimizer_magic(term, layout)? else {
        return Ok(None);
    };
    let (injected, root, fixed_locks) = crate::magic_optimizer::inject(layout, slot, locks)?;
    *layout = injected;
    *locks = fixed_locks;
    Ok(Some(root))
}

fn action_optimizer_setup(
    term: &mut Terminal,
    layout: &mut ak::Layout,
    original: &ak::Layout,
    corpus: &ng::NgramCorpus,
    weights: &mut Weights,
    settings: &mut SearchSettings,
    locks: &mut Vec<bool>,
    status: &mut String,
    baseline_cache: &mut Option<Arc<Evaluated>>,
    current_cache: &mut Option<Arc<Evaluated>>,
    magic_root: &mut Option<usize>,
    before_magic: &mut Option<(ak::Layout, Vec<bool>)>,
    stashed_swaps: &mut Option<Vec<crate::swap_fit::Swap>>,
    run_now: bool,
) -> AppResult<bool> {
    // The selected optimizer panel belongs to this setup session.
    term.optimizer_weights = false;
    term.optimizer_score = false;
    let mut scroll = 0;
    let mut settings_view = false;
    // Starts once the analyses below are ready, as if Space were pressed.
    let mut run_requested = run_now;
    let mut drag = None;
    let mut drag_hover = None;
    let (mut magic_rules, mut combos, mut swaps, mut combos_exact, mut swaps_exact) =
        load_app_config()
            .map(|config| {
                let combos = config.weights.4.then_some(config.combos.max_combos);
                (
                    config.magic.max_rules,
                    combos,
                    config.adaptive.max_swaps,
                    config.combos.exact,
                    config.adaptive.exact,
                )
            })
            .unwrap_or((MagicSettings::default().max_rules, None, 0, false, false));
    loop {
        if !settings_view && (baseline_cache.is_none() || current_cache.is_none()) {
            if baseline_cache.is_none() {
                match evaluate_tui(term, original, corpus, weights) {
                    Ok(Some(value)) => *baseline_cache = Some(value),
                    Ok(None) => {
                        *status =
                            "Optimizer analysis cancelled; change settings or try again".into();
                        settings_view = true;
                    }
                    Err(error) => {
                        *status = format!("Optimizer analysis failed: {error}");
                        settings_view = true;
                    }
                }
            }
            if !settings_view && current_cache.is_none() {
                if layout.slots == original.slots
                    && layout.actions == original.actions
                    && layout.row_stagger == original.row_stagger
                    && layout.left_outer == original.left_outer
                    && layout.right_outer == original.right_outer
                    && layout.action_mode == original.action_mode
                    && layout.alt_fingers == original.alt_fingers
                    && layout.combos.is_empty()
                    && original.combos.is_empty()
                {
                    *current_cache = baseline_cache.clone();
                } else {
                    match evaluate_tui(term, layout, corpus, weights) {
                        Ok(Some(value)) => *current_cache = Some(value),
                        Ok(None) => {
                            *status =
                                "Optimizer analysis cancelled; change settings or try again".into();
                            settings_view = true;
                        }
                        Err(error) => {
                            *status = format!("Optimizer analysis failed: {error}");
                            settings_view = true;
                        }
                    }
                }
            }
            if baseline_cache.is_some()
                && current_cache.is_some()
                && (status.starts_with("Optimizer analysis failed:")
                    || status.starts_with("Optimizer analysis cancelled;"))
            {
                status.clear();
            }
        }
        if run_requested {
            run_requested = false;
            if baseline_cache.is_some() && current_cache.is_some() {
                validate_search_settings(settings)?;
                return Ok(true);
            }
        }
        let mut frame = if settings_view {
            action_setup_frame(
                term.width(),
                term.stats_view_name(),
                layout,
                original,
                corpus,
                weights,
                locks,
                settings,
                magic_root.map(|_| magic_rules),
                combos,
                swaps,
                (combos_exact, swaps_exact),
                &format!(
                    "Magic rules: {}{}{}{}",
                    magic_root
                        .map(|root| format!("on · key {} · joint search", layout.slots[root].label))
                        .unwrap_or_else(|| "off · m enable".into()),
                    combo_label(combos, combos_exact),
                    swap_label(swaps, swaps_exact),
                    if status.is_empty() {
                        String::new()
                    } else {
                        format!(" · {status}")
                    }
                ),
                drag_hover,
            )
        } else if term.optimizer_score {
            let (before, after) = (
                baseline_cache.as_ref().unwrap(),
                current_cache.as_ref().unwrap(),
            );
            let rows = if settings.mode == "aklgg" {
                aklgg_score_rows(
                    &action_mana_stats(original, before, weights),
                    &action_mana_stats(layout, after, weights),
                    weights,
                )
            } else {
                metric_score_rows(&before.metrics, &after.metrics, weights)
            };
            let mut frame = score_frame(term.width(), rows, term.decimals());
            let keys = "1 overview · 2 settings · 3 weights · Space run · q back";
            let h = frame.h;
            frame.text(0, h - 3, &short(status, frame.w), CYAN);
            frame.text(0, h - 2, &short(keys, frame.w), MUTED);
            frame
        } else {
            let lock_status = format!(
                "Locked {} · Free {} · Magic rules: {}{}{}{}",
                locks.iter().filter(|&&value| value).count(),
                locks.iter().filter(|&&value| !value).count(),
                magic_root
                    .map(|root| format!("on · key {} · joint search", layout.slots[root].label))
                    .unwrap_or_else(|| "off · m enable".into()),
                combo_label(combos, combos_exact),
                swap_label(if stashed_swaps.is_some() { 0 } else { swaps }, swaps_exact),
                if status.is_empty() {
                    String::new()
                } else {
                    format!(" · {status}")
                }
            );
            action_frame(
                term,
                "Optimizer",
                layout,
                original,
                baseline_cache.as_ref().unwrap(),
                current_cache.as_ref().unwrap(),
                weights,
                Some(locks),
                drag_hover,
                &lock_status,
                None,
                Some(&settings.mode),
                Some((stashed_swaps.as_deref(), swaps, swaps_exact)),
            )
        };
        let active_tab = if settings_view {
            1
        } else if term.optimizer_score {
            3
        } else if term.optimizer_weights {
            2
        } else {
            0
        };
        optimizer_tabs(
            &mut frame,
            active_tab,
            &corpus.name,
            &layout.name,
            &settings.mode,
        );
        scroll = scroll.min(frame.h.saturating_sub(term.size.1));
        term.present(&frame, scroll)?;
        let mut event = term.event()?;
        if scroll_event(&event, &mut scroll, frame.h, term.size.1) {
            continue;
        }
        match event.clone() {
            Event::Escape if drag.is_some() => {
                drag = None;
                drag_hover = None;
                continue;
            }
            Event::Mouse {
                x,
                y,
                button: 0,
                release: false,
                motion: false,
                ..
            } => {
                if let Some(Action::Key(i)) = term.hit(&frame, x, y, scroll) {
                    drag = Some(i);
                    drag_hover = Some(i);
                    continue;
                }
            }
            Event::Mouse {
                x,
                y,
                button: 0,
                release: false,
                motion: true,
                ..
            } if drag.is_some() => {
                if let Some(Action::Key(i)) = term.hit(&frame, x, y, scroll) {
                    drag_hover = Some(i);
                }
                continue;
            }
            Event::Mouse {
                x,
                y,
                button: 0,
                release: true,
                ..
            } if drag.is_some() => {
                let from = drag.take().unwrap();
                let target = match term.hit(&frame, x, y, scroll) {
                    Some(Action::Key(i)) => Some(i),
                    _ => None,
                };
                drag_hover = None;
                if let Some(to) = target {
                    if from == to {
                        locks[to] = !locks[to];
                    } else {
                        layout.swap(from, to);
                        if let Some(root) = *magic_root {
                            *magic_root = Some(if root == from {
                                to
                            } else if root == to {
                                from
                            } else {
                                root
                            });
                        }
                        *current_cache = None;
                        *status = "Swapped bindings".into();
                    }
                }
                continue;
            }
            _ => {}
        }
        if let Some(action) = action_press(term, &frame, &event, scroll) {
            match action {
                Action::Key(i) => {
                    locks[i] = !locks[i];
                    continue;
                }
                Action::Weight(i) => {
                    if let Err(error) = edit_single_weight(term, weights, i) {
                        *status = error.to_string();
                    } else {
                        *baseline_cache = None;
                        *current_cache = None;
                    }
                    continue;
                }
                Action::ManaWeight(i) => {
                    match edit_physical_weight(term, weights, i) {
                        Ok(true) => {
                            settings.preset = "custom".into();
                            *status =
                                format!("Updated {} {}", settings.mode, mana2_metrics::STAT_IDS[i]);
                        }
                        Ok(false) => {}
                        Err(error) => *status = error.to_string(),
                    }
                    continue;
                }
                Action::Setting(i) => {
                    let previous_mode = settings.mode.clone();
                    let result = match i {
                        14 => action_choose_design(term, settings),
                        17 => choose_preset(term, settings),
                        MAGIC_RULES_SETTING => edit_magic_rules(term, &mut magic_rules),
                        COMBOS_SETTING => {
                            // Setting a count scores combos too.
                            if combos.is_none() {
                                weights.4 = true;
                                *baseline_cache = None;
                                *current_cache = None;
                            }
                            let count = combos.get_or_insert_with(|| {
                                load_app_config().map_or(0, |config| config.combos.max_combos)
                            });
                            edit_combo_count(term, count)
                        }
                        SWAPS_SETTING => edit_swap_count(term, &mut swaps),
                        COMBOS_EXACT_SETTING => toggle_exact(true, &mut combos_exact),
                        SWAPS_EXACT_SETTING => toggle_exact(false, &mut swaps_exact),
                        _ => edit_setting(term, settings, i),
                    };
                    if let Err(error) = result {
                        *status = error.to_string();
                    } else if settings.mode != previous_mode {
                        *status = format!("Scoring: {}", settings.mode);
                    }
                    continue;
                }
                Action::Metric(metric) if !settings_view => {
                    action_contributors(
                        term,
                        metric,
                        layout,
                        original,
                        baseline_cache.as_ref().unwrap(),
                        current_cache.as_ref().unwrap(),
                    )?;
                    continue;
                }
                Action::Command(ch) => event = Event::Char(ch),
                _ => {}
            }
        }

        // Handle shared tab shortcuts before panel-specific commands. This is
        // also the path used by mouse clicks on the tab bar.
        match event {
            Event::Char(tab @ '1'..='4') => {
                settings_view = tab == '2';
                term.optimizer_weights = tab == '3';
                term.optimizer_score = tab == '4';
                scroll = 0;
                continue;
            }
            _ => {}
        }

        match event {
            Event::Char(' ') => {
                if baseline_cache.is_none() || current_cache.is_none() {
                    // Refresh changed weights/policies even when the user
                    // starts directly from the settings panel.
                    settings_view = false;
                    term.optimizer_weights = false;
                    run_requested = true;
                    continue;
                }
                validate_search_settings(settings)?;
                term.optimizer_weights = false;
                term.optimizer_score = false;
                return Ok(true);
            }
            Event::Escape | Event::Char('q') if settings_view => {
                settings_view = false;
                term.optimizer_weights = false;
                scroll = 0;
            }
            Event::Escape | Event::Char('q') if term.optimizer_weights => {
                term.optimizer_weights = false;
                scroll = 0;
            }
            Event::Escape | Event::Char('q') if term.optimizer_score => {
                term.optimizer_score = false;
                scroll = 0;
            }
            Event::Escape | Event::Quit | Event::Char('q') => return Ok(false),
            Event::Char('e') => {
                settings_view = !settings_view;
                term.optimizer_weights = false;
                term.optimizer_score = false;
                scroll = 0;
            }
            Event::Char('.') => term.precise = !term.precise,
            Event::Char('m') | Event::Char('M') => {
                if magic_root.is_some() {
                    *magic_root = None;
                    if let Some((saved_layout, saved_locks)) = before_magic.take() {
                        *layout = saved_layout;
                        *locks = saved_locks;
                    } else {
                        *layout = original.clone();
                        *locks = action_default_locks(layout);
                    }
                    *current_cache = None;
                    if stashed_swaps.is_some() {
                        keep_swaps_off(layout, stashed_swaps, current_cache);
                    }
                    *status =
                        "Magic search off; restored the layout from before magic search.".into();
                } else {
                    let saved = (layout.clone(), locks.clone());
                    match enable_optimizer_magic(term, layout, locks) {
                        Ok(root) => {
                            *magic_root = root;
                            if root.is_some() {
                                *before_magic = Some(saved);
                                *current_cache = None;
                            }
                        }
                        Err(error) => *status = error.to_string(),
                    }
                }
            }
            Event::Char('H') => *locks = action_home_locks(layout),
            Event::Char('U') => locks.fill(false),
            Event::Char('L') => locks.fill(true),
            Event::Char('o') => {
                // Restores the arrangement only; the locks stay as set.
                *layout = original.clone();
                *magic_root = None;
                *before_magic = None;
                *current_cache = baseline_cache.clone();
                if stashed_swaps.is_some() {
                    keep_swaps_off(layout, stashed_swaps, current_cache);
                }
            }
            Event::Char('a') => {
                match stashed_swaps.take() {
                    Some(swaps) => {
                        *layout = crate::swap_fit::install(layout, &swaps);
                        *status = "Adaptive swaps on".into();
                    }
                    None => {
                        *stashed_swaps = Some(Vec::new());
                        keep_swaps_off(layout, stashed_swaps, current_cache);
                        *status = "Adaptive swaps off for this session: none used or fitted".into();
                    }
                }
                *current_cache = None;
            }
            Event::Char('n') => settings.seed = new_seed(),
            Event::Char('v') => {
                term.toggle_stats_view();
                scroll = 0;
            }
            Event::Char('t') => {
                cycle_weight_system(settings);
                *status = format!("{} scoring", settings.mode);
                scroll = 0;
            }
            Event::Char('g') => action_choose_design(term, settings)?,
            Event::Char('f') => {
                weights.3 = !weights.3;
                *baseline_cache = None;
                *current_cache = None;
                *status = format!(
                    "Alternate fingers {}; s saves configuration",
                    if weights.3 { "on" } else { "off" }
                );
            }
            Event::Char('F') => {
                weights.4 = !weights.4;
                combos = weights
                    .4
                    .then(|| load_app_config().map_or(0, |config| config.combos.max_combos));
                *baseline_cache = None;
                *current_cache = None;
                *status = format!(
                    "Combos {}; s saves configuration",
                    if weights.4 { "enabled" } else { "disabled" }
                );
            }
            Event::Char('p') => choose_preset(term, settings)?,
            Event::Char('x') => edit_mix(term, settings)?,
            Event::Char('w') => {
                let open = !term.optimizer_weights;
                term.optimizer_weights = open;
                term.optimizer_score = false;
                settings_view = false;
                scroll = 0;
            }
            Event::Enter if term.optimizer_weights => {
                if settings.uses_mana2_stats() {
                    if let Err(error) = choose_physical_weight(term, weights) {
                        *status = error.to_string();
                    }
                } else {
                    let before = weights_text(weights);
                    if let Err(error) = choose_optimizer_weight(term, weights) {
                        *status = error.to_string();
                    }
                    if weights_text(weights) != before {
                        *baseline_cache = None;
                        *current_cache = None;
                    }
                }
            }
            Event::Char('d') => {
                *weights = Weights::default();
                *settings = SearchSettings::default();
                *baseline_cache = None;
                *current_cache = None;
            }
            Event::Char('s') => {
                *status = match save_optimizer_settings(weights, settings) {
                    Ok(()) => "saved configuration".into(),
                    Err(error) => error.to_string(),
                };
            }
            Event::Char('r') => match load_app_config() {
                Ok(config) => {
                    *weights = config.weights;
                    *settings = config.search;
                    *baseline_cache = None;
                    *current_cache = None;
                    *status = "reloaded configuration".into();
                }
                Err(error) => *status = error.to_string(),
            },
            Event::Char('i') => {
                let warnings: Vec<_> = corpus
                    .warnings
                    .iter()
                    .chain(layout.source_warnings.iter())
                    .cloned()
                    .collect();
                info_page(term, "Corpus and layout", &warnings)?;
            }
            Event::Char('?') => help_popup(
                term,
                &frame,
                "Optimizer",
                &[
                    "Space\tRun the optimizer.".into(),
                    "m\tToggle magic-key injection and joint letter/rule search.".into(),
                    "1 / 2 / 3\tOverview / Settings / Weights.".into(),
                    "e / w\tOpen Settings / Weights; press again for Overview.".into(),
                    "Enter\tChoose a weight for the selected scoring system.".into(),
                    "v\tSwitch Simple / Detailed statistics; scoring stays unchanged.".into(),
                    "t\tSwitch normal / aklgg scoring.".into(),
                    "s\tSave weights and search settings.".into(),
                    "r\tReload weights and search settings from configuration.".into(),
                    "d\tRestore default weights and search settings.".into(),
                    "o\tReset the layout to its loaded bindings.".into(),
                    "Click a key\tLock or unlock its physical slot.".into(),
                    "Drag a key\tSwap its binding with another key.".into(),
                    "Click a stat\tShow its contributing patterns.".into(),
                    "H\tUse default locks.".into(),
                    "U\tUnlock all keys, including an active magic key.".into(),
                    "L\tLock all keys.".into(),
                    "f\tTurn alternate fingers off or on (or click the Alts line).".into(),
                    "a\tTurn adaptive swaps off or on for this session (or click the Swaps line)."
                        .into(),
                    "F\tEnable or disable combos.".into(),
                    "g\tChoose Refine, Random, or Evolve.".into(),
                    "p\tChoose a limit preset.".into(),
                    "n\tChoose a new random seed.".into(),
                    "x\tEdit the training corpus mixture.".into(),
                    ".\tSwitch between two and four decimal places.".into(),
                    "q\tClose settings, or return to the previous screen.".into(),
                    "Limits are increases relative to the original on each training corpus.".into(),
                    "Travel limits use u/100; SFB/SFS use percentage points. none disables.".into(),
                ],
            )?,
            _ => {}
        }
    }
}

/// Take `layout`'s adaptive swaps out into `stashed`, which marks them off.
/// Swaps already stashed stay; the layout's own replace them when it has any.
fn keep_swaps_off(
    layout: &mut ak::Layout,
    stashed: &mut Option<Vec<crate::swap_fit::Swap>>,
    current_cache: &mut Option<Arc<Evaluated>>,
) {
    let (base, swaps) = crate::swap_fit::split(layout);
    if !swaps.is_empty() {
        *layout = base;
        *stashed = Some(swaps);
        *current_cache = None;
    }
}

/// `arrangement` of a search result, with its alternate fingers named by
/// letter again (see `alt_fingers::pin`).
fn arranged(start: &ak::Layout, arr: &[usize]) -> ak::Layout {
    let mut layout = crate::action_search::arrangement(start, arr);
    crate::alt_fingers::unpin(&mut layout);
    layout
}

fn action_optimizer_search(
    term: &mut Terminal,
    original: &ak::Layout,
    seed: &mut ak::Layout,
    corpora: &[(ng::NgramCorpus, f64)],
    weights: &Weights,
    settings: &SearchSettings,
    locks: &[bool],
    magic_root: Option<usize>,
    swaps_off: bool,
) -> AppResult<Option<(Snapshot, ak::Layout)>> {
    // Own alternate fingers stay on their keys while letters move; every
    // layout leaving the search names them by letter again.
    let mut original_owned = original.clone();
    let mut seed_owned = seed.clone();
    crate::alt_fingers::pin(&mut original_owned);
    crate::alt_fingers::pin(&mut seed_owned);
    let training = corpora.to_vec();
    let worker_weights = weights.clone();
    let worker_settings = settings.clone();
    let worker_locks = locks.to_vec();
    let config = load_app_config()?;
    let worker_magic = config.magic;
    let worker_combos = config.combos;
    let mut worker_swaps = config.adaptive;
    if swaps_off {
        worker_swaps.max_swaps = 0;
    }
    let joint = magic_root.is_some()
        || worker_combos.active_max(&worker_weights) > 0
        || worker_swaps.max_swaps > 0;
    let control = Arc::new(Control::new());
    let worker_control = Arc::clone(&control);
    let (updates_tx, updates_rx) = mpsc::sync_channel(2);
    let (done_tx, done_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        let result = if joint {
            let mut callback = |mut layout, snapshot, baseline, raw, totals| {
                crate::alt_fingers::unpin(&mut layout);
                let _ = updates_tx.try_send((snapshot, baseline, raw, totals, layout));
            };
            crate::magic_optimizer::run_controlled(
                &original_owned,
                &seed_owned,
                magic_root,
                &worker_locks,
                &training,
                &worker_weights,
                &worker_settings,
                &worker_magic,
                &worker_combos,
                &worker_swaps,
                &worker_control,
                &mut callback,
            )
            .map(|(layout, snapshot)| (snapshot, layout))
        } else {
            let mut callback = |snapshot: Snapshot, baseline, raw, totals| {
                let layout = arranged(&seed_owned, &snapshot.best.arr);
                let _ = updates_tx.try_send((snapshot, baseline, raw, totals, layout));
            };
            crate::action_search::run_parallel(
                &original_owned,
                &seed_owned,
                &training,
                &worker_weights,
                &worker_settings,
                &worker_locks,
                &worker_control,
                &mut callback,
            )
            .map(|snapshot| (snapshot, seed_owned))
        };
        let _ = done_tx.send(result);
    });

    let mut snapshot: Option<(Snapshot, Metrics, Raw, [f64; 4], ak::Layout)> = None;
    let mut scroll = 0;
    let result = (|| -> AppResult<Option<(Snapshot, ak::Layout)>> {
        loop {
            while let Ok(update) = updates_rx.try_recv() {
                snapshot = Some(update);
            }
            match done_rx.try_recv() {
                Ok(Ok(value)) => return Ok(Some(value)),
                Ok(Err(error)) if error == "cancelled" => return Ok(None),
                Ok(Err(error)) => return Err(error.into()),
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err("action search worker exited without a result".into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }

            let layout = snapshot
                .as_ref()
                .map(|(_, _, _, _, layout)| layout.clone())
                .unwrap_or_else(|| seed.clone());
            let mut frame = Canvas::optimizer(term.width(), 96);
            header(
                &mut frame,
                "Optimizing",
                &corpora[0].0.name,
                &seed.name,
                &format!(
                    "p pause | m stats: {} | ? help | q setup",
                    term.stats_view_name()
                ),
            );
            let mut y = action_keyboard(&mut frame, 3, &layout, original, Some(locks), None) + 1;
            y = rules_and_alts_rows(&mut frame, y, &layout, weights);
            if let Some((snapshot, baseline, raw, totals, _)) = &snapshot {
                let progress = &snapshot.progress;
                let phase = if control.pause.load(Ordering::Relaxed) {
                    "paused"
                } else {
                    &progress.phase
                };
                let current = metrics_totals(raw, totals);
                let aklgg_scoring = settings.mode == "aklgg";
                let physical_stats = settings.uses_mana2_stats().then(|| {
                    // A training mixture has no single population; it keeps
                    // the mapped totals.
                    let populations = match corpora {
                        [(corpus, _)] => corpus.speed_populations,
                        _ => [0.0; 2],
                    };
                    ng::mana_stats(
                        raw.1
                            .as_ref()
                            .expect("physical search snapshot must contain Mana counters"),
                        *totals,
                        populations,
                    )
                });
                if aklgg_scoring {
                    // Search snapshots carry the current compact physical counters.
                    // Showing it in both columns avoids remapping the baseline merely
                    // for a live progress frame; setup and result views have both.
                    let stats = physical_stats.as_ref().unwrap();
                    y = aklgg_stat_table(&mut frame, y, stats, stats, term.decimals());
                } else {
                    y = grouped_metric_table(
                        &mut frame,
                        y,
                        baseline,
                        &current,
                        raw,
                        totals,
                        term.decimals(),
                        if term.simple_stats {
                            &SIMPLE_TABLE_GROUPS
                        } else {
                            &TABLE_GROUPS
                        },
                        false,
                        true,
                    );
                    y = finger_table(&mut frame, y + 1, baseline, &current, term.decimals());
                }
                if settings.uses_mana2_stats() {
                    let score = mana_breakdown(physical_stats.as_ref().unwrap(), weights);
                    frame.text(
                        0,
                        y + 1,
                        &format!(
                            "{} · Penalties {} − Rewards {} = Score {}",
                            "aklgg",
                            number(score.penalty, term.decimals()),
                            number(score.bonus, term.decimals()),
                            number(score.net, term.decimals()),
                        ),
                        CYAN,
                    );
                    y += 3;
                } else {
                    y = score_panel(
                        &mut frame,
                        y + 1,
                        &action_score_breakdown(baseline, weights),
                        &action_score_breakdown(&current, weights),
                        None,
                        term.decimals(),
                    );
                }
                if snapshot.has_best {
                    frame.text(
                        0,
                        y,
                        &format!(
                            "Search score: {} ({}, training mixture)",
                            number(snapshot.best.score, term.decimals()),
                            if settings.uses_mana2_stats() {
                                "aklgg"
                            } else {
                                "Normal"
                            },
                        ),
                        CYAN,
                    );
                    y += 2;
                }
                frame.text(
                    0,
                    y,
                    &format!(
                        "{}  {}/{}  {} {}  {} accepted  {:.1}s · {} workers",
                        phase,
                        progress.restart,
                        settings.restarts,
                        progress.evaluations,
                        if magic_root.is_some() {
                            "swap trials"
                        } else {
                            "trials"
                        },
                        progress.accepted,
                        progress.elapsed,
                        search_worker_count(settings),
                    ),
                    CYAN,
                );
            } else {
                frame.text(0, y, "Building incremental context caches", CYAN);
            }
            frame.h = y + 3;
            term.present(&frame, scroll)?;
            let event = term.event()?;
            if scroll_event(&event, &mut scroll, frame.h, term.size.1) {
                continue;
            }
            match event {
                Event::Char('p') => {
                    let paused = control.pause.load(Ordering::Relaxed);
                    control.pause.store(!paused, Ordering::Relaxed);
                }
                Event::Char('.') => term.precise = !term.precise,
                Event::Char('m') => {
                    term.toggle_stats_view();
                    scroll = 0;
                }
                Event::Char('?') => help_popup(
                    term,
                    &frame,
                    "Optimizer search",
                    &[
                        "p pauses or resumes the search; q returns to setup and keeps the best current changes in memory.".into(),
                        "The score, accepted count, trial count, and current best layout remain visible while searching.".into(),
                        "Use the setup screen to change weights, corpus mixture, locks, limits, and search settings.".into(),
                        "m switches the statistics display: Detailed jump breakdown or Simple FSB/FSS/HSB/HSS.".into(),
                        "Magic search refits rules between search sections; swap trials count binding proposals.".into(),
                    ],
                )?,
                Event::Escape | Event::Quit | Event::Char('q') => return Ok(None),
                _ => {}
            }
        }
    })();

    control.cancel.store(true, Ordering::Relaxed);
    control.pause.store(false, Ordering::Relaxed);
    if worker.join().is_err() {
        return Err("action search worker panicked".into());
    }
    if matches!(&result, Ok(None)) {
        // A final accepted candidate may arrive after the last screen update.
        // Recover the complete layout, including its dictionary, before leaving.
        if let Ok(Ok((finished, basis))) = done_rx.try_recv() {
            if finished.has_best {
                *seed = arranged(&basis, &finished.best.arr);
            }
        } else {
            while let Ok(update) = updates_rx.try_recv() {
                snapshot = Some(update);
            }
            if let Some((snapshot, _, _, _, layout)) = snapshot {
                if snapshot.has_best {
                    *seed = layout;
                }
            }
        }
    }
    result
}

fn action_score_breakdown(metrics: &Metrics, weights: &Weights) -> Breakdown {
    breakdown(metrics, weights)
}

fn action_result_frame(
    term: &Terminal,
    layout: &ak::Layout,
    original: &ak::Layout,
    before: &Evaluated,
    after: &Evaluated,
    weights: &Weights,
    settings: &SearchSettings,
    locks: &[bool],
    objective: f64,
    status: &str,
) -> Canvas {
    let mut frame = Canvas::optimizer(term.width(), 96);
    header(
        &mut frame,
        "Optimizer result",
        &after.corpus.name,
        &layout.name,
        &format!("m stats: {} | ? help | q setup", term.stats_view_name()),
    );
    let mut y = action_keyboard(&mut frame, 2, layout, original, Some(locks), None) + 1;
    y = rules_and_alts_rows(&mut frame, y, layout, weights);
    let aklgg_scoring = settings.mode == "aklgg";
    let physical_stats = settings.uses_mana2_stats().then(|| {
        (
            action_mana_stats(original, before, weights),
            action_mana_stats(layout, after, weights),
        )
    });
    if aklgg_scoring {
        let (before_stats, after_stats) = physical_stats.as_ref().unwrap();
        y = aklgg_stat_table(&mut frame, y, before_stats, after_stats, term.decimals());
    } else {
        y = grouped_metric_cards(
            &mut frame,
            y,
            &before.metrics,
            &after.metrics,
            &after.raw,
            &after.corpus,
            term.decimals(),
            term.simple_stats,
        );
        y = finger_table(
            &mut frame,
            y + 1,
            &before.metrics,
            &after.metrics,
            term.decimals(),
        );
    }
    if settings.uses_mana2_stats() {
        let (before_stats, after_stats) = physical_stats.as_ref().unwrap();
        y = score_panel(
            &mut frame,
            y + 1,
            &mana_breakdown(before_stats, weights),
            &mana_breakdown(after_stats, weights),
            None,
            term.decimals(),
        );
    } else {
        y = score_panel(
            &mut frame,
            y + 1,
            &action_score_breakdown(&before.metrics, weights),
            &action_score_breakdown(&after.metrics, weights),
            None,
            term.decimals(),
        );
    }
    frame.text(
        0,
        y,
        &format!(
            "Search score: {} ({}, training mixture) · {}-gram estimate",
            number(objective, term.decimals()),
            if settings.uses_mana2_stats() {
                "aklgg"
            } else {
                "normal"
            },
            after.counts.order,
        ),
        CYAN,
    );
    y += 2;
    frame.text(0, y, &short(status, frame.w), CYAN);
    frame.h = y + 2;
    frame
}

fn save_action_result(
    layout: &ak::Layout,
    original: &ak::Layout,
    start: &ak::Layout,
    weights: &Weights,
    settings: &SearchSettings,
    locks: &[bool],
    corpora: &[(ng::NgramCorpus, f64)],
    candidate: &Candidate,
    progress: &Progress,
    magic_enabled: bool,
) -> AppResult<PathBuf> {
    let path = crate::layout_export::save_optimized(layout)
        .map_err(|error| format!("layout save: {error}"))?;
    let selected_weights = if settings.uses_mana2_stats() {
        format!("[aklgg]\n{}\n", weights.2.config_text())
    } else {
        String::new()
    };
    let mut report = format!(
        "model = {MODEL_VERSION}\nseed = 0x{:x}\ntrials = {}\nseconds = {:.6}\nscore = {:.17}\n\n[weights]\n{}\n[rolls]\n{}\n{}[search]\n{}\n[original]\n{}\n[start]\n{}\n[result]\n{}\n[locks]\n{:?}\n",
        progress.seed,
        progress.evaluations,
        progress.elapsed,
        candidate.score,
        weights_text(weights),
        rolls_config_text(weights.rolls()),
        selected_weights,
        search_settings_text(settings),
        original.text(),
        start.text(),
        layout.text(),
        locks
            .iter()
            .enumerate()
            .filter_map(|(i, &locked)| locked.then_some(i))
            .collect::<Vec<_>>(),
    );
    if magic_enabled {
        let max_rules = load_app_config()
            .map(|config| config.magic.max_rules)
            .unwrap_or(MagicSettings::default().max_rules);
        report.push_str(&format!(
            "\n[magic_rules]\nenabled = true\nmax_rules = {max_rules}\nmode = {}\nalgorithm = controlled_joint_search\n",
            settings.mode
        ));
    }
    let total_share: f64 = corpora.iter().map(|(_, share)| *share).sum();
    for (corpus, share) in corpora {
        report.push_str(&format!(
            "\n[corpus {}]\nshare = {}\norder = {}\n",
            corpus.name,
            share / total_share,
            corpus.order,
        ));
        for warning in &corpus.warnings {
            report.push_str(&format!("# {warning}\n"));
        }
    }
    atomic_write(&path.with_extension("run.txt"), &report, false)?;
    Ok(path)
}

fn action_optimizer(
    term: &mut Terminal,
    original: ak::Layout,
    corpus: ng::NgramCorpus,
) -> AppResult<()> {
    let config = load_app_config()?;
    let weights = config.weights;
    let settings = config.search;
    let locks = action_default_locks(&original);
    action_optimizer_session(
        term, original, corpus, weights, settings, locks, false, false,
    )
    .map(|_| ())
}

/// The action optimizer for a converted plain layout, injecting a magic key
/// when `magic` is set and starting a run at once when `start` is set.
pub(crate) fn action_optimizer_from_plain(
    term: &mut Terminal,
    original: ak::Layout,
    corpus: ng::NgramCorpus,
    weights: Weights,
    settings: SearchSettings,
    locks: Vec<bool>,
    magic: bool,
    start: bool,
) -> AppResult<bool> {
    action_optimizer_session(
        term, original, corpus, weights, settings, locks, magic, start,
    )
}

fn action_optimizer_session(
    term: &mut Terminal,
    original: ak::Layout,
    corpus: ng::NgramCorpus,
    mut weights: Weights,
    mut settings: SearchSettings,
    mut locks: Vec<bool>,
    enable: bool,
    start: bool,
) -> AppResult<bool> {
    let mut layout = original.clone();
    let mut before_magic = None;
    let mut run_now = start;
    let mut magic_root = if enable {
        let saved = (layout.clone(), locks.clone());
        let Some(root) = enable_optimizer_magic(term, &mut layout, &mut locks)? else {
            return Ok(false);
        };
        before_magic = Some(saved);
        Some(root)
    } else {
        // A layout that already has a magic key refits its rules in place.
        // Turning magic search off keeps this layout rather than resetting it.
        let existing = crate::magic_optimizer::existing_root(&layout);
        if existing.is_some() {
            before_magic = Some((layout.clone(), locks.clone()));
        }
        existing
    };
    let mut status = String::new();
    // Setup redraws, lock changes, and search-option edits reuse these full
    // physical analyses. Layout or weight changes invalidate only what they
    // can affect.
    let mut setup_baseline: Option<Arc<Evaluated>> = None;
    let mut setup_current: Option<Arc<Evaluated>> = None;
    // Adaptive swaps taken out while they are off for this session.
    let mut stashed_swaps = None;

    loop {
        if !action_optimizer_setup(
            term,
            &mut layout,
            &original,
            &corpus,
            &mut weights,
            &mut settings,
            &mut locks,
            &mut status,
            &mut setup_baseline,
            &mut setup_current,
            &mut magic_root,
            &mut before_magic,
            &mut stashed_swaps,
            std::mem::take(&mut run_now),
        )? {
            return Ok(true);
        }
        let Some(training) = crate::session::load_action_training(term, &corpus, &settings)? else {
            continue;
        };
        let mut before = setup_baseline.clone();
        'runs: loop {
            let actual_run_start = layout.clone();
            let magic_binding = magic_root
                .and_then(|root| layout.slots.get(root))
                .map(|slot| slot.binding.clone());
            let searched = action_optimizer_search(
                term,
                &original,
                &mut layout,
                &training,
                &weights,
                &settings,
                &locks,
                magic_root,
                stashed_swaps.is_some(),
            );
            let (result, start) = match searched {
                Ok(Some(result)) => result,
                Ok(None) => {
                    if let Some(binding) = &magic_binding {
                        magic_root = layout
                            .slots
                            .iter()
                            .position(|slot| &slot.binding == binding);
                    }
                    setup_current = None;
                    status = "Search stopped; changes kept in memory. o resets the layout.".into();
                    break 'runs;
                }
                Err(error) => {
                    setup_current = None;
                    status = format!("Search failed: {error}");
                    break 'runs;
                }
            };
            if !result.has_best || result.archive.is_empty() {
                status = format!("{} · no eligible candidate", result.progress.phase);
                break 'runs;
            }

            // Full physical histograms are needed only for the result currently
            // inspected. Keep each requested result until leaving this archive.
            if before.is_none() {
                before = evaluate_tui(term, &original, &corpus, &weights)?;
                if before.is_none() {
                    break 'runs;
                }
            }
            let baseline = before.as_ref().unwrap();
            let mut details: Vec<Option<Arc<Evaluated>>> = vec![None; result.archive.len()];
            let mut choice = 0;
            let mut scroll = 0;
            status.clear();
            loop {
                let candidate = &result.archive[choice];
                layout = arranged(&start, &candidate.arr);
                if let Some(binding) = &magic_binding {
                    magic_root = layout
                        .slots
                        .iter()
                        .position(|slot| &slot.binding == binding);
                }
                if details[choice].is_none() {
                    details[choice] = evaluate_tui(term, &layout, &corpus, &weights)?;
                    if details[choice].is_none() {
                        break 'runs;
                    }
                }
                let current = details[choice].as_ref().unwrap();
                let summary = if status.is_empty() {
                    format!(
                        "{}/{}  {}  {} trials  {} accepted",
                        choice + 1,
                        result.archive.len(),
                        result.progress.phase,
                        result.progress.evaluations,
                        result.progress.accepted,
                    )
                } else {
                    status.clone()
                };
                let frame = action_result_frame(
                    term,
                    &layout,
                    &original,
                    baseline,
                    current,
                    &weights,
                    &settings,
                    &locks,
                    candidate.score,
                    &summary,
                );
                term.present(&frame, scroll)?;
                let event = term.event()?;
                if scroll_event(&event, &mut scroll, frame.h, term.size.1) {
                    continue;
                }
                if let Some(action) = action_press(term, &frame, &event, scroll) {
                    match action {
                        Action::Metric(metric) => action_contributors(
                            term, metric, &layout, &original, baseline, current,
                        )?,
                        _ => {}
                    }
                }

                match event {
                    Event::Escape | Event::Char('q') | Event::Char('r') => {
                        setup_current = Some(current.clone());
                        status = "Changes kept in memory. o resets the layout.".into();
                        break 'runs;
                    }
                    Event::Quit => return Ok(true),
                    Event::Char('.') => term.precise = !term.precise,
                    Event::Char('m') => {
                        term.toggle_stats_view();
                        scroll = 0;
                    }
                    Event::Char('v') if magic_root.is_some() => {
                        let root = magic_root.unwrap();
                        let mut rows = vec![format!("Fixed physical key: {}", layout.slots[root].label)];
                        if let ak::Binding::Named(name) = &layout.slots[root].binding {
                            if let Some(ak::Action::Rules { rules, .. }) = layout.actions.get(name) {
                                rows.extend(rules.iter().map(|(context, output)| {
                                    let output = match output {
                                        ak::Emission::Text(text) => format!("{:?}", String::from_utf8_lossy(text)),
                                        ak::Emission::Call(name) => format!("@{name}"),
                                        ak::Emission::None => "none".into(),
                                    };
                                    format!("After {:?} → {output}", String::from_utf8_lossy(context))
                                }));
                            }
                        }
                        help_popup(term, &frame, "Current magic rules", &rows)?;
                    }
                    Event::Char('?') => help_popup(
                        term,
                        &frame,
                        "Optimizer result",
                        &[
                            "Space refines this candidate; b compares archived candidates; [ and ] move between them.".into(),
                            "s saves this candidate; S saves the archive; r or q returns to setup and keeps the current layout in memory.".into(),
                            "Click a detailed metric for its physical pattern contributions. i shows corpus notes; t traces text; p traces physical key presses.".into(),
                            "m switches the statistics display: Detailed jump breakdown or Simple FSB/FSS/HSB/HSS.".into(),
                            "v shows current magic rules when magic optimization is enabled.".into(),
                            ". toggles display precision.".into(),
                        ],
                    )?,
                    Event::Char(' ') => {
                        settings.design = "refine".into();
                        settings.seed = settings.seed.wrapping_add(1);
                        continue 'runs;
                    }
                    Event::Char('[') => {
                        choice = (choice + result.archive.len() - 1) % result.archive.len();
                        status.clear();
                        scroll = 0;
                    }
                    Event::Char(']') => {
                        choice = (choice + 1) % result.archive.len();
                        status.clear();
                        scroll = 0;
                    }
                    Event::Char('b') => {
                        let names: Vec<_> = result
                            .archive
                            .iter()
                            .enumerate()
                            .map(|(i, candidate)| {
                                format!(
                                    "{}  score {}",
                                    i + 1,
                                    number(candidate.score, term.decimals())
                                )
                            })
                            .collect();
                        if let Some(index) = menu(term, "Candidates", &names)? {
                            choice = index;
                            status.clear();
                            scroll = 0;
                        }
                    }
                    Event::Char('s') => {
                        status = match save_action_result(
                            &layout,
                            &original,
                            &actual_run_start,
                            &weights,
                            &settings,
                            &locks,
                            &training,
                            candidate,
                            &result.progress,
                            magic_root.is_some(),
                        ) {
                            Ok(path) => saved_layout_message(&path),
                            Err(error) => error.to_string(),
                        };
                    }
                    Event::Char('S') => {
                        let mut saved = 0;
                        for candidate in &result.archive {
                            let candidate_layout =
                                arranged(&start, &candidate.arr);
                            match save_action_result(
                                &candidate_layout,
                                &original,
                                &actual_run_start,
                                &weights,
                                &settings,
                                &locks,
                                &training,
                                candidate,
                                &result.progress,
                                magic_root.is_some(),
                            ) {
                                Ok(_) => saved += 1,
                                Err(error) => {
                                    status = format!(
                                        "saved {saved} candidates (.jsonc); {error}"
                                    );
                                    break;
                                }
                            }
                        }
                        if saved == result.archive.len() {
                            status =
                                format!("saved {saved} candidates (.jsonc) and run records");
                        }
                    }
                    Event::Char('i') => show_corpus_info(term, &current.corpus)?,
                    Event::Char('t') => trace_view(term, &layout, &weights, false)?,
                    Event::Char('p') => trace_view(term, &layout, &weights, true)?,
                    _ => {}
                }
            }
        }
    }
}

// Retain the previous greedy runner only as a profiling/differential reference.
#[cfg(test)]
fn run_action_search<const PROFILE: bool, const DETAIL: bool>(
    initial: ak::Layout,
    text: &ng::NgramCorpus,
    weights: &Weights,
    locked: &[bool],
    limits: (f64, f64),
    cancel: &AtomicBool,
    profile: &mut crate::action_profile::Profile,
) -> ak::Result<(ak::Layout, u64)> {
    use crate::action_profile::{clock, elapsed};
    let setup_start = clock::<PROFILE>();
    let mut best = initial;
    let effort = LocalEffort::new(&best, weights);
    let cache_result = ng::Incremental::new_with_policy(
        text,
        &best,
        Geometry::with_rolls(physical_keys(&best), weights.rolls()),
        cancel,
        weights.alts(),
        weights.combo_policy(),
        |a, b, k| effort.get(a, b, k),
    );
    let mut cache = match cache_result {
        Ok(cache) => cache,
        Err(error) => {
            if PROFILE {
                profile.setup += elapsed::<PROFILE>(setup_start);
            }
            return Err(error);
        }
    };
    let mut score = cache.score(weights).score;
    let mut trials = 0;
    let mut proposal = ng::Proposal::new();
    if PROFILE {
        profile.cache_size = cache.context_count() as u64;
        profile.setup += elapsed::<PROFILE>(setup_start);
    }
    for _ in 0..40 {
        let mut improved = false;
        for a in 0..best.slots.len() {
            if locked[a] {
                continue;
            }
            for b in a + 1..best.slots.len() {
                if locked[b] {
                    continue;
                }
                if cancel.load(Ordering::Relaxed) {
                    return Ok((best, trials));
                }
                if best.slots[a].binding == best.slots[b].binding {
                    continue;
                }
                let candidate_start = clock::<PROFILE>();
                let mut named = false;
                if PROFILE {
                    profile.attempted += 1;
                    named = matches!(best.slots[a].binding, ak::Binding::Named(_))
                        || matches!(best.slots[b].binding, ak::Binding::Named(_));
                    if named {
                        profile.named += 1;
                    } else if matches!(best.slots[a].binding, ak::Binding::Text(_))
                        && matches!(best.slots[b].binding, ak::Binding::Text(_))
                    {
                        profile.literals += 1;
                    } else {
                        profile.other_candidates += 1;
                    }
                }
                let result = cache.propose_swap_profiled::<PROFILE, DETAIL, _>(
                    a,
                    b,
                    cancel,
                    |a, b, k| effort.get(a, b, k),
                    &mut proposal,
                    profile,
                );
                if let Err(error) = result {
                    if PROFILE && named {
                        profile.named_time += elapsed::<PROFILE>(candidate_start);
                    }
                    return Err(error);
                }
                let score_start = clock::<PROFILE>();
                let candidate = proposal.score(weights);
                trials += 1;
                if PROFILE {
                    profile.score += elapsed::<PROFILE>(score_start);
                    profile.candidates += 1;
                }
                let checks_start = clock::<PROFILE>();
                let accept = candidate.sfb <= limits.0 + 1e-10
                    && candidate.sfs <= limits.1 + 1e-10
                    && candidate.score < score - 1e-10;
                if PROFILE {
                    profile.checks += elapsed::<PROFILE>(checks_start);
                }
                if accept {
                    cache.commit_profiled::<PROFILE>(&mut proposal, profile);
                    best.swap(a, b);
                    let score_start = clock::<PROFILE>();
                    score = cache.score(weights).score;
                    improved = true;
                    if PROFILE {
                        profile.score += elapsed::<PROFILE>(score_start);
                        profile.accepted += 1;
                    }
                } else if PROFILE {
                    profile.rejected += 1;
                }
                if PROFILE && named {
                    profile.named_time += elapsed::<PROFILE>(candidate_start);
                }
            }
        }
        if !improved {
            break;
        }
    }
    Ok((best, trials))
}

fn print_evaluation(l: &ak::Layout, corpus: &ng::NgramCorpus) -> AppResult<Evaluated> {
    let w = load_weights(Path::new(WEIGHTS_FILE))?;
    let v = evaluate(l, corpus, &w, &AtomicBool::new(false))
        .map_err(|e| format!("action corpus: {e}"))?;
    println!(
        "{}  {}  {}-gram estimate",
        l.name, corpus.name, corpus.order
    );
    for warning in &v.corpus.warnings {
        println!("{warning}");
    }
    println!(
        "presses {}  characters {}  action presses {}  ignored {}",
        v.counts.presses, v.counts.characters, v.counts.action_presses, v.counts.ignored_characters
    );
    for m in 0..N_METRICS {
        println!("{:<12} {:.6}", METRIC_NAMES[m], v.metrics.v[m]);
    }
    println!("SCORE        {:.6}", v.score);
    // Mana2 / akl.gg statistics for the same mapping, normalized as the ranker
    // normalizes action layouts.
    let summary = crate::action_summary::evaluate_with_mode(
        l,
        corpus,
        &w,
        &AtomicBool::new(false),
        &AtomicU64::new(0),
        "aklgg",
    )
    .map_err(|e| format!("action corpus: {e}"))?;
    if let Some(raw) = summary.raw.1.as_ref() {
        let stats = ng::mana_stats(raw, summary.totals, summary.counts.speed_populations);
        for (id, value) in mana2_metrics::STAT_IDS.iter().zip(stats.values) {
            println!("mana.{id:<22} {value:.6}");
        }
    }
    Ok(v)
}

fn corpus_arg(args: &[String], i: usize) -> AppResult<PathBuf> {
    match args.get(i) {
        Some(name) => corpus_by_name(name),
        None => default_corpus_path()?.ok_or_else(|| "select a corpus".into()),
    }
}

pub fn dispatch(args: &[String]) -> Option<AppResult<()>> {
    if std::env::var_os("AKLER_ACTION_PLAIN_CHILD").is_some() {
        return None;
    }
    let explicit = args.first().is_some_and(|s| s == "magic" || s == "actions");
    let args = if explicit { &args[1..] } else { args };
    let command = args.first().map(String::as_str).unwrap_or("editor");
    let trace = matches!(command, "trace" | "trace-keys");
    let mut detected = None;
    if !explicit && !trace {
        if !matches!(command, "editor" | "optimizer" | "eval") {
            return None;
        }
        let Some(p) = args.get(1) else {
            return None;
        };
        match ak::Layout::load(Path::new(p)) {
            Ok(l) if l.extended() => {
                detected = Some(l);
            }
            Ok(_) => return None,
            Err(error) => {
                if load_board(Path::new(p)).is_ok() {
                    return None;
                }
                return Some(Err(format!("action layout: {error}").into()));
            }
        }
    }
    Some((|| -> AppResult<()> {
        if command == "ranker" {
            let path = corpus_arg(args, 1)?;
            let mut term = Terminal::open()?;
            return ranking(&mut term, &path);
        }
        let path = if let Some(p) = args.get(1) {
            PathBuf::from(p)
        } else {
            let mut t = Terminal::open()?;
            match choose_path(&mut t, LAYOUT_DIR, "", "", "Layouts")? {
                Some(p) => p,
                None => return Ok(()),
            }
        };
        let l = match detected.take() {
            Some(l) => l,
            None => ak::Layout::load(&path).map_err(|e| format!("action layout: {e}"))?,
        };
        if trace {
            let text = args
                .get(2)
                .ok_or("trace requires text or space-separated physical labels")?;
            let weights = load_weights(Path::new(WEIGHTS_FILE))?;
            let effort = LocalEffort::new(&l, &weights);
            let steps = if command == "trace-keys" {
                let keys = text
                    .split_whitespace()
                    .map(|label| {
                        l.slots
                            .iter()
                            .position(|s| {
                                s.label == label || s.label.trim_start_matches('@') == label
                            })
                            .ok_or_else(|| format!("unknown key {label}"))
                    })
                    .collect::<ak::Result<Vec<_>>>()
                    .map_err(|e| format!("trace: {e}"))?;
                ak::trace_keys(&l, &keys)
            } else {
                ak::decode(
                    &l,
                    text.as_bytes(),
                    ak::DEFAULT_STATE_LIMIT,
                    &AtomicBool::new(false),
                    |a, b, k| effort.get(a, b, k),
                )
            }
            .map_err(|e| format!("trace: {e}"))?;
            for s in steps {
                println!(
                    "{:>3} {:<14} {:<18} {:>4}..{:<4} {}",
                    s.key,
                    l.slots[s.key].label,
                    ak::quote(&s.output),
                    s.start,
                    s.end,
                    s.reason
                );
            }
            return Ok(());
        }
        let corpus_path = corpus_arg(args, 2)?;
        if matches!(command, "editor" | "optimizer") {
            let mut term = Terminal::open()?;
            let Some(corpus) = load_corpus_tui(&mut term, &corpus_path)? else {
                return Ok(());
            };
            return action_editor(&mut term, l, corpus, command == "optimizer");
        }
        let corpus_path = if corpus_is_raw(&corpus_path)? {
            ensure_corpus(&corpus_path, false, Some(3))?
        } else {
            corpus_path
        };
        let text =
            ng::NgramCorpus::load(&corpus_path).map_err(|e| format!("action corpus: {e}"))?;
        match command {
            "eval" => {
                let _ = print_evaluation(&l, &text)?;
                Ok(())
            }
            "report" => {
                let v = print_evaluation(&l, &text)?;
                let p = args.get(3).ok_or("report requires an output .json path")?;
                v.counts
                    .write_report(&l, Path::new(p))
                    .map_err(|e| e.into())
            }
            _ => Err(
                "actions: editor | optimizer | ranker | eval | report | trace | trace-keys".into(),
            ),
        }
    })())
}

#[cfg(test)]
mod ngram_integration_tests {
    use super::*;
    fn canvas_text(c: &Canvas) -> String {
        c.cells.iter().map(|cell| cell.ch).collect()
    }
    #[test]
    fn staggered_action_keyboard_preserves_tints_highlights_and_hitboxes() {
        let source = format!("{}row-stagger: standard\n", ak::test_layouts::COMPACT);
        let original = ak::Layout::parse(&source, Path::new("tints.dat")).unwrap();
        let mut layout = original.clone();
        layout.swap(0, 1);
        let mut locks = vec![false; layout.slots.len()];
        locks[2] = true;
        let mut canvas = Canvas::new(120, 20);
        action_keyboard(&mut canvas, 0, &layout, &original, Some(&locks), None);
        for (i, (rect, _)) in canvas.hits.iter().enumerate() {
            let expected = if i < 2 {
                BLUE
            } else if i == 2 {
                YELLOW
            } else {
                finger_tint(layout.slots[i].finger)
            };
            let center = &canvas.cells[(rect.y + 1) * canvas.w + rect.x..(rect.y + 2) * canvas.w];
            assert!(center
                .iter()
                .take(rect.w)
                .any(|cell| cell.color == expected));
        }
        assert_eq!(canvas.hits.len(), layout.slots.len());
        assert_eq!(canvas.hits[10].0.x - canvas.hits[0].0.x, 2);
        assert_eq!(canvas.hits[20].0.x - canvas.hits[0].0.x, 6);
    }

    #[test]
    fn numeric_column_stagger_has_matching_plain_and_action_hitboxes() {
        let source = "q w e r t | y u i o p\na s d f g | h j k l ;\nz x c v b | n m , . /\nthumbs: space\ncolumn-offsets: -0.5 0 0 0 0 0 0 0 0 0.5\n";
        let layout = ak::Layout::parse(source, Path::new("columns")).unwrap();
        let model = Model::new(board_from_text(source, Path::new("columns")).unwrap());
        let mut action_canvas = Canvas::new(120, 25);
        let mut plain_canvas = Canvas::new(120, 25);
        let action_bottom = action_keyboard(&mut action_canvas, 2, &layout, &layout, None, None);
        let plain_bottom = keyboard(
            &mut plain_canvas,
            2,
            &model,
            &model.original,
            &model.original,
            None,
            None,
            &[],
        );
        assert_eq!(action_bottom, 17);
        assert_eq!(plain_bottom, action_bottom);
        assert_eq!(action_canvas.hits[0].0.y, 2);
        assert_eq!(action_canvas.hits[9].0.y, 5);
        assert_eq!(action_canvas.hits.len(), plain_canvas.hits.len());
        for (index, ((actual, _), (plain, _))) in action_canvas
            .hits
            .iter()
            .zip(&plain_canvas.hits)
            .enumerate()
        {
            assert_eq!(
                (actual.x, actual.y, actual.w, actual.h),
                (plain.x, plain.y, plain.w, plain.h)
            );
            assert!(
                matches!(action_canvas.action(actual.x + 1, actual.y + 1), Some(Action::Key(key)) if key == index)
            );
        }
        let physical = physical_keys(&layout);
        assert_eq!(physical[0].column_offset, -500);
        assert_eq!(physical[9].column_offset, 500);
    }

    #[test]
    fn rightmost_empty_slots_keep_their_physical_column_and_hitbox() {
        let source = ", u o c z | k d g b j ~\n\
                      i e a $ # | m t s n h q\n\
                      . ; / w ' | x v f p y ~\n\
                      thumbs: space\n\
                      fingermap: LP LR LM LI LI RI RI RM RR RP LP / LP LR LM LI LI RI RI RM RR RP RP / LP LR LM LI LI RI RI RM RR RP LP\n\
                      col-layout: absolute\n";
        let layout = ak::Layout::parse(source, Path::new("empty-right.dat")).unwrap();
        let model = Model::new(board_from_text(source, Path::new("empty-right.dat")).unwrap());
        let mut action_canvas = Canvas::new(120, 25);
        let mut plain_canvas = Canvas::new(120, 25);
        action_keyboard(&mut action_canvas, 2, &layout, &layout, None, None);
        keyboard(
            &mut plain_canvas,
            2,
            &model,
            &model.original,
            &model.original,
            None,
            None,
            &[],
        );
        for row in [0, 2] {
            let prior = layout
                .slots
                .iter()
                .position(|s| s.main && s.row == row && s.col == 9)
                .unwrap();
            let empty = layout
                .slots
                .iter()
                .position(|s| s.main && s.row == row && s.col == 10)
                .unwrap();
            assert_eq!(layout.slots[empty].hand, 0);
            assert_eq!(layout.slots[prior].hand, 1);
            let actual = action_canvas.hits[empty].0;
            let before = action_canvas.hits[prior].0;
            assert!(actual.x >= before.x + before.w);
            assert_eq!(actual.x, plain_canvas.hits[empty].0.x);
            assert!(
                matches!(action_canvas.action(actual.x + 1, actual.y + 1), Some(Action::Key(i)) if i == empty)
            );
        }
    }

    #[test]
    fn ranked_action_details_keep_root_physical_key_names() {
        let layout = history_layout();
        let corpus = history_corpus(b"aan");
        // Make the action win over the literal same-key repeat without relying
        // on saved defaults or unrelated metric weights.
        let mut weights = Weights::new([0.0; N_WEIGHTS]);
        weights.0[SKB] = 2.0;
        let evaluation = evaluate(&layout, &corpus, &weights, &AtomicBool::new(false)).unwrap();
        let rows = action_contributor_rows(SFB, &layout, &evaluation, &evaluation);
        assert!(rows.iter().any(|(keys, before, after)| {
            keys == "@ n" && *before > 0.0 && before.to_bits() == after.to_bits()
        }));
        let canvas = action_detail_frame(
            100,
            SFB,
            &layout,
            &layout,
            &corpus.name,
            &rows,
            (evaluation.metrics.v[SFB], evaluation.metrics.v[SFB]),
            (1.0, 1.0),
            false,
            true,
            false,
        );
        assert!(canvas_text(&canvas).contains("@ n"));
    }

    #[test]
    fn action_view_helpers_preserve_physical_names_selection_and_all_rows() {
        let l = history_layout();
        let m = history_key(&l, "@");
        let mut c = Canvas::new(100, 20);
        let locks = vec![false; l.slots.len()];
        action_keyboard(&mut c, 3, &l, &l, Some(&locks), Some(m));
        assert!(canvas_text(&c).contains('›'));
        assert_eq!(c.hits.len(), l.slots.len());
        let rows: Vec<_> = (0..40).map(|i| (format!("@ n {i}"), 0.1, 0.2)).collect();
        for width in [40, 80, 120] {
            let c = action_detail_frame(
                width,
                SFB,
                &l,
                &l,
                "test",
                &rows,
                (4.0, 8.0),
                (1.0, 1.0),
                false,
                true,
                false,
            );
            let text = canvas_text(&c);
            assert!(text.contains("@ n 39"));
            assert!(text.contains("Before"));
            assert!(c.h >= 46);
            // All rows, not a fixed top-24 snapshot.
        }
        let empty = action_detail_frame(
            80,
            SFB,
            &l,
            &l,
            "test",
            &[],
            (0.0, 0.0),
            (1.0, 1.0),
            false,
            true,
            false,
        );
        assert!(canvas_text(&empty).contains("No contributing physical sequences"));
    }
    #[test]
    fn magic_setup_uses_shared_search_settings_without_inline_weights() {
        let layout = history_layout();
        let corpus = history_corpus(b"aan");
        let weights = Weights::default();
        let settings = SearchSettings::default();
        let locks = vec![false; layout.slots.len()];
        for width in [40, 80, 120] {
            let canvas = action_setup_frame(
                width,
                "Detailed",
                &layout,
                &layout,
                &corpus,
                &weights,
                &locks,
                &settings,
                None,
                None,
                0,
                (false, false),
                "ready",
                None,
            );
            let text = canvas_text(&canvas);
            for label in ["Search settings", "Locked", "Free", "ready"] {
                assert!(text.contains(label), "missing {label} at width {width}");
            }
            assert!(text.contains("Steps"));
            assert!(text.contains("Temp start"));
            assert!(text.contains("Travel Δ max"));
            assert!(!text.contains("Restarts") && !text.contains("Workers"));
            for (setting, _, _) in settings_labels(&settings) {
                assert!(canvas.hits.iter().any(|(_, action)| matches!(
                    action, Action::Setting(index) if *index == setting
                )));
            }
            assert!(!canvas
                .hits
                .iter()
                .any(|(_, action)| matches!(action, Action::Weight(SFB))));
            assert_eq!(
                canvas
                    .hits
                    .iter()
                    .filter(|(_, action)| matches!(action, Action::Key(_)))
                    .count(),
                layout.slots.len()
            );
        }

        let canvas = action_setup_frame(
            80,
            "Detailed",
            &layout,
            &layout,
            &corpus,
            &weights,
            &locks,
            &settings,
            None,
            None,
            0,
            (false, false),
            "ready",
            None,
        );
        assert!(canvas
            .hits
            .iter()
            .any(|(_, action)| matches!(action, Action::Setting(16))));
        assert!(!canvas_text(&canvas).contains("Magic rules"));
        let canvas = action_setup_frame(
            80,
            "Detailed",
            &layout,
            &layout,
            &corpus,
            &weights,
            &locks,
            &settings,
            Some(7),
            Some(4),
            4,
            (false, false),
            "ready",
            None,
        );
        assert!(canvas_text(&canvas).contains("Magic rules"));
        assert!(canvas
            .hits
            .iter()
            .any(|(_, action)| matches!(action, Action::Setting(MAGIC_RULES_SETTING))));
        assert!(canvas_text(&canvas).contains("Combos"));
        assert!(canvas
            .hits
            .iter()
            .any(|(_, action)| matches!(action, Action::Setting(COMBOS_SETTING))));
        assert!(canvas_text(&canvas).contains("Adaptive swaps"));
        assert!(canvas
            .hits
            .iter()
            .any(|(_, action)| matches!(action, Action::Setting(SWAPS_SETTING))));
        // Combo and swap counts and exactness show whether or not combos are
        // scored, at every width.
        for (width, combos) in [(80, None), (120, Some(3)), (40, Some(0))] {
            let canvas = action_setup_frame(
                width,
                "Detailed",
                &layout,
                &layout,
                &corpus,
                &weights,
                &locks,
                &settings,
                None,
                combos,
                3,
                (true, false),
                "ready",
                None,
            );
            let text = canvas_text(&canvas);
            for label in ["Combos exact", "Swaps exact", "Adaptive swaps", "yes"] {
                assert!(text.contains(label), "missing {label} at {width}:\n{text}");
            }
            for setting in [COMBOS_SETTING, COMBOS_EXACT_SETTING, SWAPS_EXACT_SETTING] {
                assert!(canvas
                    .hits
                    .iter()
                    .any(|(_, action)| matches!(action, Action::Setting(s) if *s == setting)));
            }
        }
    }
    #[test]
    fn result_line_names_genuine_rules_and_combos() {
        let mut layout = ak::Layout::parse(
            r#"{"layout": {"fingers": ["q w e r t y u i o p", "a s d f g h j k l *", "z x c v b n m , . /"],
                "thumbs": ["space", ""]},
              "fingermap": ["0 1 2 3 3 6 6 7 8 9", "0 1 2 3 3 6 6 7 8 9", "0 1 2 3 3 6 6 7 8 9"],
              "magic": {"keys": "*", "wildcards": "%", "rules": [
                  {"inputs": "%*", "output": "%%"},
                  {"inputs": "o*", "output": "ou"},
                  {"inputs": "e*", "output": "ee"}]}}"#,
            Path::new("result-line.jsonc"),
        )
        .unwrap();
        assert_eq!(rules_and_combos(&layout).as_deref(), Some("Magic *: o→u"));
        let Json::Array(combos) = crate::parse_json(
            r#"[{"keys":["e","d"],"output":"ed"},{"inputs":["rf"],"output":"rf"}]"#,
        )
        .unwrap() else {
            unreachable!()
        };
        layout.combos = combos;
        assert_eq!(
            rules_and_combos(&layout).as_deref(),
            Some("Magic *: o→u · Combos: e+d=ed r+f=rf")
        );
        layout.combos.clear();
        let plain = ak::Layout::parse(
            "q w e r t  y u i o p\na s d f g  h j k l ;\nz x c v b  n m , . /\n",
            Path::new("plain.dat"),
        )
        .unwrap();
        assert_eq!(rules_and_combos(&plain), None);
    }

    #[test]
    fn default_locks_keep_off_main_keys_and_home_locks_add_the_home_row() {
        let layout = history_layout();
        let home = action_home_locks(&layout);
        let default = action_default_locks(&layout);
        let action = history_key(&layout, "@");
        assert!(home[action]);
        assert!(!default[action]);
        let mut shifted = layout.clone();
        for slot in shifted.slots.iter_mut().filter(|slot| slot.main) {
            slot.col += 1;
        }
        shifted.left_outer = true;
        let mut outer = shifted
            .slots
            .iter()
            .find(|slot| slot.main && slot.row == 1 && slot.col == 1)
            .unwrap()
            .clone();
        outer.col = 0;
        outer.binding = ak::Binding::Empty;
        shifted.slots.push(outer);
        let shifted_locks = action_home_locks(&shifted);
        for (slot, locked) in shifted.slots.iter().zip(shifted_locks) {
            if slot.main && slot.row == 1 && !matches!(slot.binding, ak::Binding::Named(_)) {
                assert_eq!(locked, matches!(slot.col, 1 | 2 | 3 | 4 | 7 | 8 | 9 | 10));
            }
        }
        for i in 0..layout.slots.len() {
            assert_eq!(default[i], !layout.slots[i].main);
            if layout.space(i) || layout.home(i) || !layout.slots[i].main {
                assert!(home[i]);
            }
        }
    }

    #[test]
    fn magic_details_show_keyboard_bars_top_rows_and_remainder() {
        let baseline = history_layout();
        let mut layout = baseline.clone();
        let action = history_key(&layout, "@");
        let literal = history_key(&layout, "n");
        layout.swap(action, literal);
        let rows: Vec<_> = (0..20).map(|i| (format!("@ n {i}"), 0.1, 0.2)).collect();
        let canvas = action_detail_frame(
            108,
            SFB,
            &layout,
            &baseline,
            "test",
            &rows,
            (2.0, 4.0),
            (2.0, 1.0),
            false,
            false,
            false,
        );
        let text = canvas_text(&canvas);
        for label in [
            "SFB: Two consecutive presses on different keys using the same finger.",
            "Before",
            "After",
            "Change",
            "Other",
            "@ n 15",
            "○",
            "●",
            "2.0000",
            "4.0000",
            "d sort: value",
            "b pairs: separate",
            "a rows: top",
            "Change ignores denominator shifts",
            "—",
        ] {
            assert!(text.contains(label), "missing {label}");
        }
        assert!(!text.contains("@ n 16"));
        assert!(canvas
            .cells
            .iter()
            .any(|cell| cell.ch == '@' && cell.color == BLUE));
        assert_eq!(canvas.hits.len(), layout.slots.len());
    }
    #[test]
    fn action_detail_rows_and_shared_baseline_keep_exact_evaluation() {
        let l = history_layout();
        let stop = AtomicBool::new(false);
        // Exercise an actual action winner; default stretch penalties can make
        // the literal repeat cheaper than a -> @ in this geometry.
        let mut w = Weights::new([0.0; N_WEIGHTS]);
        w.0[SKB] = 2.0;
        let effort = LocalEffort::new(&l, &w);
        let a = history_key(&l, "a");
        let action = history_key(&l, "@");
        assert!(effort.get(None, Some(a), action) < effort.get(None, Some(a), a));

        let c = history_corpus(b"aan");
        let initial = Arc::new(evaluate(&l, &c, &w, &stop).unwrap());
        let current = initial.clone();
        assert!(Arc::ptr_eq(&initial, &current));
        let rows = action_contributor_rows(SFB, &l, &initial, &current);
        assert!(rows
            .iter()
            .any(|(name, b, a)| name == "@ n" && *b > 0.0 && b.to_bits() == a.to_bits()));
        assert!(rows.iter().all(|(_, b, a)| *b != 0.0 || *a != 0.0));
        let mut panel = Canvas::new(100, 8);
        score_panel(
            &mut panel,
            0,
            &breakdown(&initial.metrics, &w),
            &breakdown(&current.metrics, &w),
            None,
            4,
        );
        let panel_text = canvas_text(&panel);
        for label in ["Penalties", "Rewards", "Score"] {
            assert!(panel_text.contains(label));
        }
        let reopened = evaluate(&l, &c, &w, &stop).unwrap();
        for i in 0..N_RAW {
            assert_eq!(initial.raw.0[i].to_bits(), reopened.raw.0[i].to_bits());
        }
        let other = evaluate(&l, &history_corpus(b"aap"), &w, &stop).unwrap();
        assert!(action_contributor_rows(SFB, &l, &other, &other)
            .iter()
            .any(|(name, _, _)| name == "@ p"));
        // There is no derived-state cache: changed rules/geometry are evaluated afresh.
        let mut changed = l.clone();
        changed.actions.insert("@".into(), ak::Action::Inactive);
        let changed_value = evaluate(&changed, &c, &w, &stop).unwrap();
        assert_eq!(changed_value.counts.action_presses, 0.0);
        let m = history_key(&l, "@");
        changed = l.clone();
        changed.slots[m].finger = 0;
        changed.slots[m].hand = 0;
        changed.slots[m].rank = 0;
        let geometry_value =
            evaluate(&changed, &c, &Weights::new([0.0; N_WEIGHTS]), &stop).unwrap();
        assert_eq!(geometry_value.model.geometry.keys[m].finger, 0);
        assert_eq!(geometry_value.raw.0[SFB], 0.0);
        let plain_text = HISTORY_LAYOUT.replace("@ b", "! b");
        let plain = ak::Layout::parse(
            plain_text.split("\nw@ wh").next().unwrap(),
            Path::new("plain.dat"),
        )
        .unwrap();
        let value = evaluate(&plain, &c, &w, &stop).unwrap();
        assert_eq!(value.counts.action_presses, 0.0);

        // Before-only named pairs must survive a three-key binding cycle.
        let original = ak::Layout::parse(
            "y l d w z | q f u o /\nn r t s v | k b e a i\nx j m c g | p h , . ;\n",
            Path::new("jump-comparison.dat"),
        )
        .unwrap();
        let corpus = history_corpus(b"ho op po bo ob ok ko kb kb bh hk hk hk");
        let before = evaluate(&original, &corpus, &w, &stop).unwrap();
        let mut edited = original.clone();
        let k = history_key(&original, "k");
        let b = history_key(&original, "b");
        let h = history_key(&original, "h");
        edited.swap(k, b);
        edited.swap(b, h);
        let after = evaluate(&edited, &corpus, &w, &stop).unwrap();
        let rows = action_contributor_rows(DNFJB, &edited, &before, &after);
        assert!(rows
            .iter()
            .any(|(name, b, a)| name == "h o" && *b > 0.0 && *a == 0.0));
        assert!(rows
            .iter()
            .any(|(name, b, a)| name == "k o" && *b == 0.0 && *a > 0.0));
        let rows = action_contributor_rows(DNHJB, &edited, &before, &after);
        assert!(rows
            .iter()
            .any(|(name, b, a)| { name == "b o" && *b > 0.0 && b.to_bits() == a.to_bits() }));
        for metric in 0..N_METRICS {
            let rows = action_contributor_rows(metric, &edited, &before, &after);
            let sums = rows
                .iter()
                .fold((0.0, 0.0), |sums, row| (sums.0 + row.1, sums.1 + row.2));
            assert!((sums.0 - before.metrics.v[metric]).abs() < 1e-8);
            assert!((sums.1 - after.metrics.v[metric]).abs() < 1e-8);
        }
    }
    const HISTORY_LAYOUT: &str = "f d l w v | q p o u ,\ns t h y g | z n a e i\nx k m c j | @ b ' ; .\nthumbs: r space\n\nw@ wh\nn@ n'\na@ aa\n";
    fn history_layout() -> ak::Layout {
        ak::Layout::parse(HISTORY_LAYOUT, Path::new("history.dat")).unwrap()
    }

    fn history_key(l: &ak::Layout, label: &str) -> usize {
        l.slots.iter().position(|s| s.label == label).unwrap()
    }

    fn history_corpus(text: &[u8]) -> ng::NgramCorpus {
        let tables = ak::text_ngrams(text);
        let names = ["letters", "bigrams", "trigrams", "fourgrams", "fivegrams"];
        let fields = (0..5)
            .map(|n| {
                format!(
                    "{}:{{{}}}",
                    ak::quote(names[n].as_bytes()),
                    tables[n]
                        .iter()
                        .map(|(g, f)| format!("{}:{}", ak::quote(g), f))
                        .collect::<Vec<_>>()
                        .join(",")
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        ng::NgramCorpus::from_text(&format!("{{{fields}}}"), Path::new("history.json")).unwrap()
    }

    fn assert_physical_mapping(
        l: &ak::Layout,
        text: &[u8],
        expected: &[Option<usize>],
        prefer_action: bool,
    ) {
        let program = crate::action_fast::Program::new(l, 5).unwrap();
        let state = crate::action_fast::KeyState::new(&program);
        let mut numeric = crate::action_fast::Mapper::new(&program, &state);
        let mut reference = ak::WindowMapper::new(l, 5).unwrap();
        let effort = |_: Option<usize>, _: Option<usize>, k: usize| {
            if prefer_action && matches!(l.slots[k].binding, ak::Binding::Named(_)) {
                -1.0
            } else {
                0.0
            }
        };
        // Reuse every successively longer prefix, including resets.
        for len in 1..=text.len() {
            let a = numeric.map(&text[..len], &effort).unwrap();
            let b = reference.map(&text[..len], &effort).unwrap();
            assert_eq!(&a[..len], &expected[..len]);
            assert_eq!(&b[..len], &expected[..len]);
        }
    }
    #[test]
    fn physical_root_action_history_is_independent_of_emitted_byte() {
        let mut l = history_layout();
        let k = |s| history_key(&l, s);
        let (a, m, n, p, w, h) = (k("a"), k("@"), k("n"), k("p"), k("w"), k("h"));
        assert_physical_mapping(&l, b"aan", &[Some(a), Some(m), Some(n)], true);
        assert_physical_mapping(&l, b"aap", &[Some(a), Some(m), Some(p)], true);
        assert_physical_mapping(&l, b"whn", &[Some(w), Some(m), Some(n)], true);
        // Safe magic has priority over a literal even without an effort bonus;
        // mismatch/no-output attempts do not create extra presses. ! is a real
        // history boundary.
        assert_physical_mapping(&l, b"aan", &[Some(a), Some(m), Some(n)], false);
        assert_physical_mapping(&l, b"a!an", &[Some(a), None, Some(a), Some(n)], true);
        assert_physical_mapping(
            &l,
            b"aaaaa",
            &[Some(a), Some(m), Some(a), Some(m), Some(a)],
            true,
        );
        l.swap(m, h);
        assert_physical_mapping(&l, b"aan", &[Some(a), Some(h), Some(n)], true);
        assert_eq!(physical_keys(&l)[h].finger, l.slots[h].finger);
    }
    #[test]
    fn nested_semantic_calls_append_only_the_root_physical_press() {
        let mut l = history_layout();
        let m = history_key(&l, "@");
        let a = history_key(&l, "a");
        let n = history_key(&l, "n");
        l.actions
            .insert("leaf".into(), ak::Action::Text(vec![b'!']));
        l.actions.insert(
            "@".into(),
            ak::Action::Rules {
                basis: ak::Basis::Text,
                rules: BTreeMap::new(),
                fallback: ak::Emission::Call("leaf".into()),
            },
        );
        assert_physical_mapping(&l, b"!n", &[Some(m), Some(n)], true);
        l.actions.insert("@".into(), ak::Action::RepeatAction);
        assert_physical_mapping(&l, b"aan", &[Some(a), Some(m), Some(n)], true);
    }
    #[test]
    fn safe_magic_priority_overrides_lower_literal_effort() {
        let l = history_layout();
        let a = history_key(&l, "a");
        let action = history_key(&l, "@");
        let weights = Weights::default();
        let effort = LocalEffort::new(&l, &weights);
        assert!(effort.get(None, Some(a), a) < effort.get(None, Some(a), action));

        let stop = AtomicBool::new(false);
        let detailed = evaluate(&l, &history_corpus(b"aah"), &weights, &stop).unwrap();
        let h = history_key(&l, "h");
        assert_eq!(
            detailed.counts.tables[2].get(&[a, action, h][..]),
            Some(&1.0)
        );
        assert_eq!(detailed.counts.action_presses, 1.0);

        let following_collision = evaluate(&l, &history_corpus(b"aan"), &weights, &stop).unwrap();
        let n = history_key(&l, "n");
        assert_eq!(
            following_collision.counts.tables[2].get(&[a, a, n][..]),
            Some(&1.0)
        );
    }

    #[test]
    fn zero_skb_weight_adds_no_same_key_effort() {
        let l = history_layout();
        let a = history_key(&l, "a");
        let mut weights = Weights::new([0.0; N_WEIGHTS]);
        let effort = LocalEffort::new(&l, &weights);
        assert_eq!(effort.get(None, Some(a), a), effort.get(None, None, a));

        weights.0[SKB] = 2.0;
        let weighted = LocalEffort::new(&l, &weights);
        assert!(weighted.all_finite());
        let mut overflowing = LocalEffort::new(&l, &weights);
        overflowing.uni[0] = f64::MAX;
        overflowing.bi[0] = f64::MAX;
        assert!(!overflowing.all_finite());
        assert_eq!(
            weighted.get(None, Some(a), a),
            weighted.get(None, None, a) + 2.0,
        );
    }

    #[test]
    fn action_n_and_action_p_are_physical_sfb_in_both_evaluators() {
        let l = history_layout();
        let m = history_key(&l, "@");
        let a = history_key(&l, "a");
        // Metric classification is independent of weights. Make the repeat
        // action win so this test measures physical history, not default policy.
        let mut w = Weights::new([0.0; N_WEIGHTS]);
        w.0[SKB] = 2.0;
        let effort = LocalEffort::new(&l, &w);
        assert!(effort.get(None, Some(a), m) < effort.get(None, Some(a), a));

        let stop = AtomicBool::new(false);
        for (text, label) in [(b"aan".as_slice(), "n"), (b"aap".as_slice(), "p")] {
            let k = history_key(&l, label);
            let corpus = history_corpus(text);
            let detailed = evaluate(&l, &corpus, &w, &stop).unwrap();
            assert_eq!(detailed.counts.tables[2].get(&[a, m, k][..]), Some(&1.0));
            assert_eq!(detailed.counts.tables[1].get(&[m, k][..]), Some(&1.0));
            assert_eq!(detailed.raw.0[SFB], 1.0);
            let effort = LocalEffort::new(&l, &w);
            let cache = ng::Incremental::new(
                &corpus,
                &l,
                Geometry::new(physical_keys(&l)),
                &stop,
                |a, b, k| effort.get(a, b, k),
            )
            .unwrap();
            let (raw, totals, tails) = cache.physical_test_state();
            assert!(tails.contains(&[a as u8, m as u8, k as u8]));
            for i in 0..N_RAW {
                assert_eq!(raw.0[i].to_bits(), detailed.raw.0[i].to_bits(), "field {i}");
            }
            for i in 0..4 {
                assert_eq!(totals[i].to_bits(), detailed.corpus.totals[i].to_bits());
            }
        }
    }

    #[test]
    fn configurable_rolls_preserve_mapping_and_match_all_evaluation_paths() {
        let source = "q @ e r t | y u i o p\na s d f g | h j k l ;\nz x c v b | n m , . /\nthumbs: space\n@ a!\n";
        let mut layout = ak::Layout::parse(source, Path::new("inline.dat")).unwrap();
        let root = history_key(&layout, "@");
        layout
            .actions
            .insert("@".into(), ak::Action::Text(vec![b'!']));
        let corpus = history_corpus(b"z!j a!j a ! a!d d!a a!j z!j");
        let stop = AtomicBool::new(false);
        let baseline = evaluate(&layout, &corpus, &Weights::default(), &stop).unwrap();

        for include_thumbs in [false, true] {
            for include_scissors in [false, true] {
                for include_stretches in [false, true] {
                    let rolls = RollSettings {
                        include_thumbs,
                        include_scissors,
                        include_stretches,
                        alts: None,
                    };
                    let weights = Weights::default().with_rolls(rolls);
                    let full = evaluate(&layout, &corpus, &weights, &stop).unwrap();
                    assert_eq!(full.counts.tables, baseline.counts.tables);
                    assert_eq!(full.counts.skip, baseline.counts.skip);
                    for metric in 0..N_METRICS {
                        if !roll_metric(metric) {
                            assert_eq!(
                                full.metrics.v[metric].to_bits(),
                                baseline.metrics.v[metric].to_bits()
                            );
                        }
                    }
                    let summary = crate::action_summary::evaluate(
                        &layout,
                        &corpus,
                        &weights,
                        &stop,
                        &AtomicU64::new(0),
                    )
                    .unwrap();
                    assert_eq!(
                        summary.raw.0.map(f64::to_bits),
                        full.raw.0.map(f64::to_bits)
                    );
                    assert_eq!(summary.score.to_bits(), full.score.to_bits());
                    for metric in [ROLL, INROLL, OUTROLL, IN2, OUT2, IN3, OUT3] {
                        let rows = action_contributor_rows(metric, &layout, &full, &full);
                        let total: f64 = rows.iter().map(|(_, _, after)| after).sum();
                        assert!((total - full.metrics.v[metric]).abs() < 1e-9);
                    }

                    let effort = LocalEffort::new(&layout, &weights);
                    let cache = ng::Incremental::new(
                        &corpus,
                        &layout,
                        Geometry::with_rolls(physical_keys(&layout), rolls),
                        &stop,
                        |a, b, key| effort.get(a, b, key),
                    )
                    .unwrap();
                    assert_summary(cache.summary(&weights), &full);
                    let mut proposal = ng::Proposal::new();
                    for other in [0, 2, 14, 30] {
                        cache
                            .propose_swap(
                                root,
                                other,
                                &stop,
                                |a, b, key| effort.get(a, b, key),
                                &mut proposal,
                            )
                            .unwrap();
                        let mut moved = layout.clone();
                        moved.swap(root, other);
                        let expected = evaluate(&moved, &corpus, &weights, &stop).unwrap();
                        assert_summary(proposal.summary(&weights), &expected);
                    }
                }
            }
        }
    }

    #[test]
    fn directional_rolls_use_action_slots_and_match_incremental_swaps() {
        let source = "q @ e r t | y u i o p\na s d f g | h j k l ;\nz x c v b | n m , . /\nthumbs: space\n@ a!\n";
        let mut layout = ak::Layout::parse(source, Path::new("inline.dat")).unwrap();
        let root = history_key(&layout, "@");
        layout
            .actions
            .insert("@".into(), ak::Action::Text(vec![b'!']));
        let stop = AtomicBool::new(false);
        let weights = Weights::default();

        for (text, expected) in [
            (b"a!j".as_slice(), Some(IN2)),
            (b"!aj".as_slice(), Some(OUT2)),
            (b"a!d".as_slice(), Some(IN3)),
            (b"d!a".as_slice(), Some(OUT3)),
            (b"a !".as_slice(), None),
        ] {
            let corpus = history_corpus(text);
            let detailed = evaluate(&layout, &corpus, &weights, &stop).unwrap();
            assert_eq!(detailed.counts.action_presses, 1.0);
            for metric in [IN2, OUT2, IN3, OUT3] {
                let count = if expected == Some(metric) { 1.0 } else { 0.0 };
                assert_eq!(detailed.raw.0[metric], count);
            }
            if let Some(metric) = expected {
                assert_eq!(detailed.metrics.v[metric], 100.0);
                let rows = action_contributor_rows(metric, &layout, &detailed, &detailed);
                assert!(rows
                    .iter()
                    .any(|(keys, _, after)| keys.contains('@') && *after == 100.0));
            } else {
                assert_eq!(detailed.raw.0[RHYTHM_DEN], 0.0);
            }

            let effort = LocalEffort::new(&layout, &weights);
            let cache = ng::Incremental::new(
                &corpus,
                &layout,
                Geometry::new(physical_keys(&layout)),
                &stop,
                |a, b, key| effort.get(a, b, key),
            )
            .unwrap();
            let (raw, _, _) = cache.physical_test_state();
            assert_eq!(raw.0.map(f64::to_bits), detailed.raw.0.map(f64::to_bits));
            let mut proposal = ng::Proposal::new();
            for other in 0..layout.slots.len() {
                if other == root {
                    continue;
                }

                cache
                    .propose_swap(
                        root,
                        other,
                        &stop,
                        |a, b, key| effort.get(a, b, key),
                        &mut proposal,
                    )
                    .unwrap();
                let mut moved = layout.clone();
                moved.swap(root, other);
                let full = evaluate(&moved, &corpus, &weights, &stop).unwrap();
                assert_summary(proposal.summary(&weights), &full);
            }
        }
    }

    #[test]
    fn action_in_each_triple_position_matches_literal_physical_metrics() {
        let l = history_layout();
        let m = history_key(&l, "@");
        let n = history_key(&l, "n");
        let a = history_key(&l, "a");
        // Force ! to have exactly one physical producer, then replace its
        // binding with literal ! at the same slot for an independent control.
        let mut action = l.clone();
        action
            .actions
            .insert("@".into(), ak::Action::Text(vec![b'!']));
        let mut literal = action.clone();
        literal.slots[m].binding = ak::Binding::Text(vec![b'!']);
        let stop = AtomicBool::new(false);
        let w = Weights::default();
        for (text, ids) in [
            (b"!an".as_slice(), [m, a, n]),
            (b"n!a".as_slice(), [n, m, a]),
            (b"na!".as_slice(), [n, a, m]),
        ] {
            let corpus = history_corpus(text);
            let x = evaluate(&action, &corpus, &w, &stop).unwrap();
            let y = evaluate(&literal, &corpus, &w, &stop).unwrap();
            assert_eq!(x.counts.tables, y.counts.tables);
            assert_eq!(x.counts.skip, y.counts.skip);
            assert_eq!(x.counts.tables[2].get(&ids[..]), Some(&1.0));
            assert_eq!(x.counts.action_presses, 1.0);
            assert_eq!(y.counts.action_presses, 0.0);
            for i in 0..N_RAW {
                assert_eq!(x.raw.0[i].to_bits(), y.raw.0[i].to_bits(), "field {i}");
            }
            if ids[0] == m || ids[2] == m {
                assert_eq!(x.raw.0[SFS], 1.0);
            }
            let effort = LocalEffort::new(&action, &w);
            let cache = ng::Incremental::new(
                &corpus,
                &action,
                Geometry::new(physical_keys(&action)),
                &stop,
                |a, b, k| effort.get(a, b, k),
            )
            .unwrap();
            let (raw, _, tails) = cache.physical_test_state();
            assert!(tails.contains(&ids.map(|k| k as u8)));
            for i in 0..N_RAW {
                assert_eq!(raw.0[i].to_bits(), x.raw.0[i].to_bits());
            }
        }
    }
    #[test]
    fn metric_adapter_evaluates_magic_from_json_only() {
        let layout = ak::Layout::parse(
            crate::action_keys::test_layouts::SHORTHAND,
            Path::new("magic.dat"),
        )
        .unwrap();
        let corpus = ng::NgramCorpus::from_text(
            r#"{"letters":{"i":1,"'":1,"a":1},"bigrams":{"i'":1,"'a":1},"trigrams":{"i'a":1}}"#,
            Path::new("/no/raw/files/corpus.json"),
        )
        .unwrap();
        let value = evaluate(
            &layout,
            &corpus,
            &Weights::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(value.counts.presses, 3.0);
        assert_eq!(value.counts.action_presses, 1.0);
        assert_eq!(value.counts.ignored_characters, 0.0);
        assert!(value.score.is_finite());
        assert!(value.metrics.v.iter().all(|x| x.is_finite()));
    }

    fn delta_corpus(order: usize) -> ng::NgramCorpus {
        let tables =
            ak::text_ngrams(b"i' it's so delicious. abc!def aaaa qqu rkrk qxuqxu aa i'i' xyz\n");
        let names = ["letters", "bigrams", "trigrams", "fourgrams", "fivegrams"];
        let fields = (0..order)
            .map(|n| {
                format!(
                    "\"{}\":{{{}}}",
                    names[n],
                    tables[n]
                        .iter()
                        .map(|(g, f)| format!("{}:{}", ak::quote(g), *f as f64 * 0.375))
                        .collect::<Vec<_>>()
                        .join(",")
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        ng::NgramCorpus::from_text(&format!("{{{fields}}}"), Path::new("delta.json")).unwrap()
    }

    fn assert_summary(actual: (Metrics, f64), expected: &Evaluated) {
        let close = |a: f64, b: f64| {
            assert!(
                (a - b).abs() < 1e-8 * a.abs().max(b.abs()).max(1.0),
                "{a} != {b}"
            )
        };
        close(actual.1, expected.score);
        for (a, b) in actual.0.v.iter().zip(expected.metrics.v) {
            close(*a, b);
        }
        for (a, b) in actual.0.usage.iter().zip(expected.metrics.usage) {
            close(*a, b);
        }
        for (a, b) in actual.0.off.iter().zip(expected.metrics.off) {
            close(*a, b);
        }
    }
    #[test]
    fn incremental_proposals_match_full_evaluation_and_rejected_swaps_do_not_leak() {
        let base = crate::action_keys::test_layouts::SHORTHAND;
        let skip =
            format!("{base}\nouter-right: ~ @sk ~\naction sk = skip-magic\nmap sk \"q\" = \"u\"\n");
        let bases = crate::action_fast::tests::fixtures();
        for source in std::iter::once(base)
            .chain(std::iter::once(skip.as_str()))
            .chain(bases.iter().map(String::as_str))
        {
            for order in 3..=5 {
                let mut layout = ak::Layout::parse(source, Path::new("delta.dat")).unwrap();
                let corpus = delta_corpus(order);
                let w = Weights::default();
                let stop = AtomicBool::new(false);
                let effort = LocalEffort::new(&layout, &w);
                let mut cache = ng::Incremental::new(
                    &corpus,
                    &layout,
                    Geometry::new(physical_keys(&layout)),
                    &stop,
                    |a, b, k| effort.get(a, b, k),
                )
                .unwrap();
                assert_summary(
                    cache.summary(&w),
                    &evaluate(&layout, &corpus, &w, &stop).unwrap(),
                );
                let mut proposal = ng::Proposal::new();
                let mut state = 17u64;
                for trial in 0..200 {
                    state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                    let a = (state >> 32) as usize % layout.slots.len();
                    state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                    // Search proposals require distinct physical slots.
                    let mut b = (state >> 32) as usize % (layout.slots.len() - 1);
                    if b >= a {
                        b += 1;
                    }

                    cache
                        .propose_swap(a, b, &stop, |a, b, k| effort.get(a, b, k), &mut proposal)
                        .unwrap();
                    layout.swap(a, b);
                    let full = evaluate(&layout, &corpus, &w, &stop).unwrap();
                    assert_summary(proposal.summary(&w), &full);
                    let fast = proposal.score(&w);
                    let baseline = cache.score(&w);
                    assert_eq!(
                        fast.score < baseline.score - 1e-10,
                        full.score < baseline.score - 1e-10
                    );
                    assert_eq!(
                        fast.sfb <= baseline.sfb + 1e-10 && fast.sfs <= baseline.sfs + 1e-10,
                        full.metrics.v[SFB] <= baseline.sfb + 1e-10
                            && full.metrics.v[SFS] <= baseline.sfs + 1e-10
                    );
                    for margin in [-0.01, 0.01] {
                        let caps = (full.metrics.v[SFB] + margin, full.metrics.v[SFS] + margin);
                        assert_eq!(
                            fast.sfb <= caps.0 + 1e-10 && fast.sfs <= caps.1 + 1e-10,
                            full.metrics.v[SFB] <= caps.0 + 1e-10
                                && full.metrics.v[SFS] <= caps.1 + 1e-10
                        );
                        assert_eq!(
                            fast.score < full.score + margin - 1e-10,
                            full.score < full.score + margin - 1e-10
                        );
                    }
                    if trial % 3 != 0 {
                        cache.commit(&mut proposal);
                    } else {
                        layout.swap(a, b);
                    }
                    assert_summary(
                        cache.summary(&w),
                        &evaluate(&layout, &corpus, &w, &stop).unwrap(),
                    );
                }
                let before = cache.summary(&w).1;
                assert!(cache
                    .propose_swap(
                        0,
                        1,
                        &AtomicBool::new(true),
                        |a, b, k| effort.get(a, b, k),
                        &mut proposal
                    )
                    .is_err());
                assert_eq!(cache.summary(&w).1, before);
                // A mapping error must also restore the numeric permutation/masks.
                let a = layout
                    .slots
                    .iter()
                    .position(|s| s.binding == ak::Binding::Text(vec![b'a']))
                    .unwrap();
                let b = layout
                    .slots
                    .iter()
                    .position(|s| s.binding == ak::Binding::Text(vec![b'i']))
                    .unwrap();
                assert!(cache
                    .propose_swap(a, b, &stop, |_, _, _| f64::NAN, &mut proposal)
                    .is_err());
                cache
                    .propose_swap(a, b, &stop, |a, b, k| effort.get(a, b, k), &mut proposal)
                    .unwrap();
                layout.swap(a, b);
                assert_summary(
                    proposal.summary(&w),
                    &evaluate(&layout, &corpus, &w, &stop).unwrap(),
                );
            }
        }
    }
    #[test]
    fn absent_literal_swap_maps_no_contexts() {
        let layout = ak::Layout::parse(
            crate::action_keys::test_layouts::SHORTHAND,
            Path::new("delta.dat"),
        )
        .unwrap();
        let corpus = delta_corpus(5);
        let w = Weights::default();
        let stop = AtomicBool::new(false);
        let effort = LocalEffort::new(&layout, &w);
        let cache = ng::Incremental::new(
            &corpus,
            &layout,
            Geometry::new(physical_keys(&layout)),
            &stop,
            |a, b, k| effort.get(a, b, k),
        )
        .unwrap();
        let a = layout
            .slots
            .iter()
            .position(|s| s.binding == ak::Binding::Text(vec![b'm']))
            .unwrap();
        let b = layout
            .slots
            .iter()
            .position(|s| s.binding == ak::Binding::Text(vec![b'w']))
            .unwrap();
        let mut proposal = ng::Proposal::new();
        cache
            .propose_swap(a, b, &stop, |a, b, k| effort.get(a, b, k), &mut proposal)
            .unwrap();
        assert_eq!(proposal.examined, 0);
        let mut trial = layout.clone();
        trial.swap(a, b);
        assert_summary(
            proposal.summary(&w),
            &evaluate(&trial, &corpus, &w, &stop).unwrap(),
        );
    }
    #[test]
    fn profiling_preserves_proposals_commit_and_rebase() {
        let layout = ak::Layout::parse(
            crate::action_keys::test_layouts::SHORTHAND,
            Path::new("profile.dat"),
        )
        .unwrap();
        let corpus = delta_corpus(5);
        let w = Weights::default();
        let stop = AtomicBool::new(false);
        let effort = LocalEffort::new(&layout, &w);
        let make = || {
            ng::Incremental::new(
                &corpus,
                &layout,
                Geometry::new(physical_keys(&layout)),
                &stop,
                |a, b, k| effort.get(a, b, k),
            )
            .unwrap()
        };
        let mut off = make();
        let mut on = make();
        let mut a = ng::Proposal::new();
        let mut b = ng::Proposal::new();
        let mut profile = crate::action_profile::Profile::default();
        profile.cache_size = on.context_count() as u64;
        let magic = layout
            .slots
            .iter()
            .position(|s| matches!(s.binding, ak::Binding::Named(_)))
            .unwrap();
        let other = layout
            .slots
            .iter()
            .position(|s| s.binding == ak::Binding::Text(vec![b'a']))
            .unwrap();
        for _ in 0..128 {
            off.propose_swap(magic, other, &stop, |a, b, k| effort.get(a, b, k), &mut a)
                .unwrap();
            on.propose_swap_profiled::<true, true, _>(
                magic,
                other,
                &stop,
                |a, b, k| effort.get(a, b, k),
                &mut b,
                &mut profile,
            )
            .unwrap();
            assert_eq!(a.score(&w).score.to_bits(), b.score(&w).score.to_bits());
            assert_eq!(a.score(&w).sfb.to_bits(), b.score(&w).sfb.to_bits());
            assert_eq!(a.score(&w).sfs.to_bits(), b.score(&w).sfs.to_bits());
            off.commit(&mut a);
            on.commit_profiled::<true>(&mut b, &mut profile);
            assert_eq!(off.score(&w).score.to_bits(), on.score(&w).score.to_bits());
        }
        assert_eq!(profile.full_scans, 128);
        assert_eq!(profile.rebases, 1);
        assert_eq!(profile.affected_total, 128 * profile.cache_size);
        assert_eq!(profile.mapped, profile.affected_total);
        assert_eq!(profile.unaffected, 0);
        assert!(profile.ops.resolutions > 0);
        assert!(profile.mapping + profile.contributions <= profile.contexts);
    }
    #[test]
    fn profiling_levels_preserve_search_and_counter_partitions() {
        let layout = ak::Layout::parse(
            crate::action_keys::test_layouts::SHORTHAND,
            Path::new("profile-search.dat"),
        )
        .unwrap();
        let corpus = delta_corpus(5);
        let w = Weights::default();
        let stop = AtomicBool::new(false);
        let mut locked = vec![true; layout.slots.len()];
        for (i, s) in layout.slots.iter().enumerate() {
            if matches!(s.binding, ak::Binding::Named(_))
                || s.binding == ak::Binding::Text(vec![b'a'])
                || s.binding == ak::Binding::Text(vec![b'i'])
            {
                locked[i] = false;
            }
        }
        let limits = (f64::INFINITY, f64::INFINITY);
        let mut off = crate::action_profile::Profile::default();
        let mut coarse = crate::action_profile::Profile::default();
        let mut detail = crate::action_profile::Profile::default();
        let a = run_action_search::<false, false>(
            layout.clone(),
            &corpus,
            &w,
            &locked,
            limits,
            &stop,
            &mut off,
        )
        .unwrap();
        let b = run_action_search::<true, false>(
            layout.clone(),
            &corpus,
            &w,
            &locked,
            limits,
            &stop,
            &mut coarse,
        )
        .unwrap();
        let c = run_action_search::<true, true>(
            layout,
            &corpus,
            &w,
            &locked,
            limits,
            &stop,
            &mut detail,
        )
        .unwrap();
        assert_eq!(a.0.slots, b.0.slots);
        assert_eq!(a.0.slots, c.0.slots);
        assert_eq!(a.1, b.1);
        assert_eq!(a.1, c.1);
        assert_eq!(off.attempted + off.mapped + off.ops.resolutions, 0);
        assert_eq!(off.setup + off.contexts + off.commit, Duration::ZERO);
        for profile in [&coarse, &detail] {
            assert_eq!(profile.candidates, a.1);
            assert_eq!(profile.attempted, profile.candidates);
            assert_eq!(profile.accepted + profile.rejected, profile.candidates);
            assert_eq!(
                profile.named + profile.literals + profile.other_candidates,
                profile.attempted
            );
            assert_eq!(profile.named, profile.full_scans);
            assert_eq!(profile.mapped, profile.affected_total);
            assert_eq!(
                profile.affected_total + profile.unaffected,
                profile.attempted * profile.cache_size
            );
            assert!(profile.affected_max <= profile.cache_size);
        }
        assert_eq!(coarse.mapping + coarse.contributions, Duration::ZERO);
        assert_eq!(coarse.ops.resolutions, detail.ops.resolutions);
        assert_eq!(coarse.ops.suffix_checks, detail.ops.suffix_checks);
    }
    #[test]
    fn suffix_index_and_terminal_chains_match_full_candidate_scores() {
        let source = format!(
            "{}outer-left: @m @again ~\naction m = magic\nmap m \"i\" = \"'\"\nmap m \"qi\" = \"!\"\nmap m \"aqi\" = none\nmap m \"raqi\" = \"'\"\nmap m \"q\" = @alias\naction alias = magic\nfallback alias = repeat-output\naction again = repeat-action\n",
            crate::action_fast::tests::fixtures()[0]
        );
        // An explicit rule retains alias-chain coverage under forced repeat fallback.
        let mut layout = ak::Layout::parse(&source, Path::new("suffix-score.dat")).unwrap();
        let tables = ak::text_ngrams(b"raqi' aqi' qi! i' qqqqu raqiaqi !!q");
        let names = ["letters", "bigrams", "trigrams", "fourgrams", "fivegrams"];
        let fields = (0..5)
            .map(|n| {
                format!(
                    "\"{}\":{{{}}}",
                    names[n],
                    tables[n]
                        .iter()
                        .map(|(g, f)| format!("{}:{}", ak::quote(g), *f as f64 * 0.375))
                        .collect::<Vec<_>>()
                        .join(",")
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let corpus =
            ng::NgramCorpus::from_text(&format!("{{{fields}}}"), Path::new("suffix.json")).unwrap();
        let w = Weights::default();
        let stop = AtomicBool::new(false);
        let effort = LocalEffort::new(&layout, &w);
        let mut cache = ng::Incremental::new(
            &corpus,
            &layout,
            Geometry::new(physical_keys(&layout)),
            &stop,
            |a, b, k| effort.get(a, b, k),
        )
        .unwrap();
        let mut proposal = ng::Proposal::new();
        let mut rng = 73u64;
        for trial in 0..200 {
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
            let a = (rng >> 32) as usize % layout.slots.len();
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
            let mut b = (rng >> 32) as usize % (layout.slots.len() - 1);
            if b >= a {
                b += 1;
            }

            cache
                .propose_swap(a, b, &stop, |a, b, k| effort.get(a, b, k), &mut proposal)
                .unwrap();
            layout.swap(a, b);
            let full = evaluate(&layout, &corpus, &w, &stop).unwrap();
            assert_summary(proposal.summary(&w), &full);
            let candidate = proposal.score(&w);
            let previous = cache.score(&w);
            assert_eq!(
                candidate.score < previous.score - 1e-10,
                full.score < previous.score - 1e-10
            );
            assert_eq!(
                candidate.sfb <= previous.sfb + 1e-10 && candidate.sfs <= previous.sfs + 1e-10,
                full.metrics.v[SFB] <= previous.sfb + 1e-10
                    && full.metrics.v[SFS] <= previous.sfs + 1e-10
            );
            if trial % 3 != 0 {
                cache.commit(&mut proposal);
            } else {
                layout.swap(a, b);
            }
            assert_summary(
                cache.summary(&w),
                &evaluate(&layout, &corpus, &w, &stop).unwrap(),
            );
        }
    }
}

#[cfg(test)]
mod detailed_evaluation_tests {
    use super::*;

    fn inline_corpus(order: usize) -> ng::NgramCorpus {
        let tables = ak::text_ngrams(b"i'i'a quu qqqqu qxuqu hnr thqe h!nr qu\0qu aan aap y,u ");
        let names = ["letters", "bigrams", "trigrams", "fourgrams", "fivegrams"];
        let fields: Vec<_> = (0..order)
            .map(|index| {
                let entries: Vec<_> = tables[index]
                    .iter()
                    .map(|(gram, count)| format!("{}:{}", ak::quote(gram), *count as f64 * 0.125))
                    .collect();
                format!("\"{}\":{{{}}}", names[index], entries.join(","))
            })
            .collect();
        ng::NgramCorpus::from_text(
            &format!("{{{}}}", fields.join(",")),
            Path::new("inline.json"),
        )
        .unwrap()
    }

    #[test]
    fn detailed_numeric_evaluation_matches_reference_reports_bit_for_bit() {
        let stop = AtomicBool::new(false);
        let progress = AtomicU64::new(0);
        for source in crate::action_fast::tests::fixtures() {
            for order in 3..=5 {
                let corpus = inline_corpus(order);
                let mut layout = ak::Layout::parse(&source, Path::new("inline.dat")).unwrap();
                for swap in [None, Some((0, layout.slots.len() - 1))] {
                    if let Some((a, b)) = swap {
                        layout.swap(a, b);
                    }
                    for weights in [Weights::default(), Weights::new([0.0; N_WEIGHTS])] {
                        let actual =
                            evaluate_progress(&layout, &corpus, &weights, &stop, &progress)
                                .unwrap();
                        let effort = LocalEffort::new(&layout, &weights);
                        let counts = corpus
                            .evaluate_reference(&layout, &stop, &progress, |a, b, key| {
                                effort.get(a, b, key)
                            })
                            .unwrap();
                        let mut timing =
                            crate::load_profile::LoadProfile::new("reference report test");
                        let expected =
                            evaluate_counts(&layout, &corpus, &weights, counts, &mut timing)
                                .unwrap();

                        assert_eq!(
                            actual.raw.0.map(f64::to_bits),
                            expected.raw.0.map(f64::to_bits)
                        );
                        assert_eq!(
                            actual.corpus.totals.map(f64::to_bits),
                            expected.corpus.totals.map(f64::to_bits)
                        );
                        assert_eq!(
                            actual.metrics.v.map(f64::to_bits),
                            expected.metrics.v.map(f64::to_bits)
                        );
                        assert_eq!(
                            actual.metrics.usage.map(f64::to_bits),
                            expected.metrics.usage.map(f64::to_bits)
                        );
                        assert_eq!(
                            actual.metrics.off.map(f64::to_bits),
                            expected.metrics.off.map(f64::to_bits)
                        );
                        assert_eq!(actual.score.to_bits(), expected.score.to_bits());
                        for metric in 0..N_METRICS {
                            let actual_rows =
                                action_contributor_rows(metric, &layout, &actual, &actual);
                            let expected_rows =
                                action_contributor_rows(metric, &layout, &expected, &expected);
                            assert_eq!(actual_rows.len(), expected_rows.len());
                            for ((a_key, a_before, a_after), (b_key, b_before, b_after)) in
                                actual_rows.iter().zip(&expected_rows)
                            {
                                assert_eq!(a_key, b_key);
                                assert_eq!(a_before.to_bits(), b_before.to_bits());
                                assert_eq!(a_after.to_bits(), b_after.to_bits());
                            }
                        }
                    }
                }
            }
        }
    }
}
