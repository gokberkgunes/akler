//! Action-aware entry points. Plain layouts continue to use the original evaluator/search.
use crate::action_keys as ak;

use crate::action_ngrams as ng;

use crate::*;

#[derive(Clone)]
pub(crate) struct Evaluated {
    pub(crate) model: Model,
    pub(crate) corpus: Corpus,
    pub(crate) raw: Raw,
    pub(crate) metrics: Metrics,
    pub(crate) score: f64,
    counts: ng::Counts,
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
                if i == j {
                    bi[i * n + j] += 2.0;
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
    let counts = c.evaluate(l, stop, progress, |a, b, k| effort.get(a, b, k))?;
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
        path: PathBuf::new(),
        tables,
        masses,
        warnings: std::iter::once(format!("{}-gram bounded-context magic estimate", c.order))
            .chain(c.warnings.iter().cloned())
            .collect(),
        fingerprint: 0,
    };
    let corpus = model.corpus(&source).map_err(|e| e.to_string())?;
    timing.mark("Physical count adapter and metric corpus");
    let raw = full_raw(&model.original, &corpus, &model.geometry);
    let metrics = metrics(&raw, &corpus);
    let score = breakdown(&metrics, w).net;
    timing.mark("Raw metrics and score");
    Ok(Evaluated {
        model,
        corpus,
        raw,
        metrics,
        score,
        counts,
    })
}

// Keep terminal input live while loading or evaluating cached contexts.
pub(crate) fn ngram_job<T, F>(
    term: &mut Terminal,
    title: &str,
    corpus_name: &str,
    layout_name: &str,
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
            let mut canvas = Canvas::new(term.width(), 8);
            header(
                &mut canvas,
                title,
                corpus_name,
                layout_name,
                "Esc / q / Ctrl+C cancel",
            );
            canvas.text(
                2,
                4,
                &format!("{}%", progress.load(Ordering::Relaxed)),
                CYAN,
            );
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
    let layout = l.clone();
    let corpus = c.clone();
    let weights = *w;
    ngram_job(
        term,
        &format!("{}-gram magic estimate", c.order),
        &c.name,
        &l.name,
        move |stop, progress| {
            evaluate_progress(&layout, &corpus, &weights, stop, progress).map(Arc::new)
        },
    )
}

fn action_keyboard(
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
    for (i, s) in l.slots.iter().enumerate() {
        let (kx, ky) = if s.main {
            (
                x + (s.col - min) as usize * step
                    + stagger_cells(s.row_offset, min_offset, step)
                    + usize::from(s.hand == 1) * gap,
                y + s.row as usize * 3 + stagger_cells(s.column_offset, min_column_offset, 3),
            )
        } else {
            (
                x + width / 2 - kw - 2 + (s.finger - 8) * (kw + 3),
                y + 9 + column_height,
            )
        };
        let locked = locks.is_some_and(|v| v[i]);
        let color = if s.binding != original.slots[i].binding {
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
        let label = short(&s.label, kw - 2);
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

fn action_controls(optimize: bool) -> &'static str {
    if optimize {
        "Space run | Tab setup/results | U unlock | w weights | s save copy | q back"
    } else {
        "Click/drag swap | u undo | s save copy | r original | . digits | q back"
    }
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
) -> Canvas {
    let mut c = Canvas::new(term.width(), 150);
    let controls = if title == "Layout" {
        "Click metric for details | i corpus | t text trace | p key trace | . digits | q back"
    } else {
        action_controls(locks.is_some())
    };
    header(&mut c, title, &a.corpus.name, &l.name, controls);
    let mut y = action_keyboard(&mut c, 2, l, base, locks, selected) + 1;
    y = score_panel(
        &mut c,
        y,
        &breakdown(&b.metrics, w),
        &breakdown(&a.metrics, w),
        None,
        term.decimals(),
    );
    let limited = if a
        .corpus
        .warnings
        .iter()
        .any(|warning| warning.starts_with("Approximate"))
    {
        " · limited contexts (i info)"
    } else {
        ""
    };
    c.text(
        0,
        y,
        &short(
            &format!(
                "{}-gram estimate{limited}   Physical presses {}   ignored {}",
                a.counts.order,
                number(a.counts.presses, term.decimals()),
                number(a.counts.ignored_characters, term.decimals())
            ),
            c.w,
        ),
        MUTED,
    );
    y += 2;
    y = grouped_metric_cards(
        &mut c,
        y,
        &b.metrics,
        &a.metrics,
        &a.raw,
        &a.corpus,
        term.decimals(),
    );
    y = finger_table(&mut c, y + 1, &b.metrics, &a.metrics, term.decimals());
    c.text(0, y, &short(status, c.w), CYAN);
    c.h = y + 2;
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

fn action_contributor_rows(
    m: usize,
    l: &ak::Layout,
    before: &Evaluated,
    after: &Evaluated,
) -> Vec<(String, f64, f64)> {
    let mut rows = Vec::new();
    let unigram =
        METRIC_NAMES[m] == "TRAVEL" || METRIC_NAMES[m] == "VTRAVEL" || METRIC_NAMES[m] == "LTRAVEL";
    let count_kind = if unigram {
        0
    } else if is_rhythm(m) {
        2
    } else {
        1
    };
    let bt = if is_skip(m) {
        &before.counts.skip
    } else {
        &before.counts.tables[count_kind]
    };
    let at = if is_skip(m) {
        &after.counts.skip
    } else {
        &after.counts.tables[count_kind]
    };
    let d0 = denominator(m, &before.raw, &before.corpus);
    let d1 = denominator(m, &after.raw, &after.corpus);
    let grams: BTreeSet<_> = bt.keys().chain(at.keys()).cloned().collect();
    for ids in grams {
        let mut gram_ids = [0usize; 3];
        for (j, &key) in ids.iter().enumerate() {
            gram_ids[j] = after.model.original[key];
        }
        let gram = Gram {
            ids: gram_ids,
            len: ids.len(),
            kind: if unigram {
                0
            } else if is_rhythm(m) {
                3
            } else if is_skip(m) {
                2
            } else {
                1
            },
            f: 1.0,
        };
        let mut unit = Raw::default();
        let pos = positions(&after.model.original);
        add_gram(&mut unit, &gram, &pos, &after.model.geometry, 1.0);
        let mass = unit.0[m];
        if mass <= 0.0 {
            continue;
        }
        let a = pct(*bt.get(&ids).unwrap_or(&0.0) * mass, d0);
        let b = pct(*at.get(&ids).unwrap_or(&0.0) * mass, d1);
        if a == 0.0 && b == 0.0 {
            continue;
        }
        let label = ids
            .iter()
            .map(|&i| l.slots[i].label.as_str())
            .collect::<Vec<_>>()
            .join(if is_skip(m) { " _ " } else { " " });
        rows.push((label, a, b));
    }
    rows.sort_by(|a, b| b.2.total_cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
    rows
}

// Render the same settings and hit targets as the ordinary optimizer.
fn action_setup_frame(
    width: usize,
    layout: &ak::Layout,
    baseline: &ak::Layout,
    corpus: &ng::NgramCorpus,
    weights: &Weights,
    locks: &[bool],
    settings: &SearchSettings,
    status: &str,
) -> Canvas {
    let mut c = Canvas::optimizer(width, 96);
    header(
        &mut c,
        "Optimizer",
        &corpus.name,
        &layout.name,
        "Space run | s save | r reload | p preset | q back",
    );
    let mut y = action_keyboard(&mut c, 3, layout, baseline, Some(locks), None) + 1;
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

    if settings.mode == "simple" {
        for (i, name) in SIMPLE_NAMES.iter().enumerate() {
            let column_width = c.w / 3;
            let x = i % 3 * column_width;
            let row = y + i / 3;
            c.text(x, row, name, FG);
            c.text(
                x + name.len() + 1,
                row,
                &config_number(settings.simple[i]),
                CYAN,
            );
            c.hit(
                Rect {
                    x,
                    y: row,
                    w: column_width.saturating_sub(1),
                    h: 1,
                },
                Action::SimpleWeight(i),
            );
        }
        y += 4;
    } else {
        y = grouped_weight_rows(&mut c, y, weights) + 1;
    }

    let fields = settings_labels(settings);
    let columns = if c.w >= 80 { 4 } else { 2 };
    let column_width = c.w / columns;
    for (i, (label, value)) in fields.iter().enumerate() {
        let x = i % columns * column_width;
        let row = y + i / columns * 2;
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
            Action::Setting(i),
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
    if settings.mode == "simple" {
        y += 1;
        c.text(0, y, "Typing effort: detailed weights (w to edit)", MUTED);
    }
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
    sort_change: bool,
    all: bool,
    merge: bool,
) -> Canvas {
    let mut c = Canvas::new(width, 64);
    header(
        &mut c,
        title,
        corpus,
        &layout.name,
        "d sort | b pair | a all/top | ? help | q back",
    );
    let mut y = action_keyboard(&mut c, 2, layout, baseline, None, None) + 1;
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
        let color = change_color(m, *before, *after);
        c.text(0, row, &short(name, 8), FG);
        c.right(9, row, 9, &number(*before, DETAIL_DECIMALS), MUTED);
        c.right(20, row, 9, &number(*after, DETAIL_DECIMALS), color);
        c.right(
            31,
            row,
            9,
            &delta_text(*before, *after, DETAIL_DECIMALS),
            delta_color(m, *before, *after),
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
        y += 1;
        c.text(0, y, "Other", MUTED);
        c.right(9, y, 9, &number(before, DETAIL_DECIMALS), MUTED);
        c.right(20, y, 9, &number(after, DETAIL_DECIMALS), MUTED);
        c.right(
            31,
            y,
            9,
            &delta_text(before, after, DETAIL_DECIMALS),
            delta_color(m, before, after),
        );
    }
    c.h = y + 2;
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
                (b.2 - b.1).abs().total_cmp(&(a.2 - a.1).abs())
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
            Event::Char('?') => info_page(
                term,
                METRIC_NAMES[m],
                &[ METRIC_HELP[m].into(), roll_settings_label(after.model.geometry.rolls), "Keys are physical slots pressed, including action keys, labelled by the current layout; they are not emitted text.".into(), "Before is the loaded baseline; After is the current layout. Blue keys have moved.".into(), "Top 16 contributions are shown with the remainder in Other. a shows all rows; b combines reversed pairs.".into(), "SRAF/ALT show clean credit; roll thumb inclusion and movement filters follow [rolls]. Both-zero rows are omitted; — means unchanged.".into(),]
            )?,
            _ => {
            }
        }
    }
}

fn choose_corpus(term: &mut Terminal) -> AppResult<Option<PathBuf>> {
    select_source_path(term, false)
}

pub(crate) fn action_editor(
    term: &mut Terminal,
    mut l: ak::Layout,
    mut corpus: ng::NgramCorpus,
    optimize: bool,
) -> AppResult<()> {
    if optimize {
        return action_optimizer(term, l, corpus);
    }

    let mut timing =
        crate::load_profile::LoadProfile::new("action editor/optimizer initialization");
    let original = l.clone();
    let mut w = load_weights(Path::new(WEIGHTS_FILE))?;
    let Some(mut baseline) = evaluate_tui(term, &original, &corpus, &w)? else {
        return Ok(());
    };
    timing.mark("Baseline evaluation (nested report)");
    let mut current = baseline.clone();
    let mut undo: Vec<ak::Layout> = Vec::new();
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
            &baseline,
            &current,
            &w,
            None,
            selected.or(drag),
            &status,
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
            },
            Event::Escape | Event::Quit | Event::Char('q') => return Ok(()),
            Event::Char('.') => term.precise=!term.precise,
            Event::Char('?') => info_page(
                term,
                "Action controls",
                &[action_controls(false).into(), "i shows corpus/limit notes; t traces text; p traces physical key presses; c changes corpus; w edits weights.".into(), "Click metrics for physical contributions. Save creates a new copy; r restores the original layout.".into()]
            )?,
            Event::Char('i') => show_corpus_info(term, &current.corpus)?,
            Event::Char('t') => trace_view(term, &l, &w, false)?,
            Event::Char('p') => trace_view(term, &l, &w, true)?,
            Event::Char('s') => status = match l.save_new() {
                Ok(p) => saved_layout_message(&p),
                Err(e) => e
            },
            Event::Char('r') => {
                undo.push(l.clone());
                l = original.clone();
                current = baseline.clone();
            },
            Event::Char('u') => if let Some(old) = undo.last() {
                if let Some(value) = evaluate_tui(term, old, &corpus, &w)? {
                    l = undo.pop().unwrap();
                    current = value;
                }
            },
            Event::Char('c') => if let Some(p) = choose_corpus(term)? {
                match load_corpus_tui(term, &p) {
                    Ok(Some(next)) => {
                        if let Some(b) = evaluate_tui(term, &original, &next, &w)? {
                            if let Some(a) = evaluate_tui(term, &l, &next, &w)? {
                                corpus = next;
                                baseline = b;
                                current = a;
                                status.clear();
                            }
                        }
                    },
                    Ok(None) => {
                    },
                    Err(e) => status = e.to_string(),
                }
            },
            Event::Char('w') => {
                let ids: Vec<_> = (0..N_WEIGHTS).filter(|&i|!aggregate(i)).collect();
                let names: Vec<_> = ids.iter().map(|&i | WEIGHT_NAMES[i].to_string()).collect();
                if let Some(i) = menu(term, "", &names)? {
                    let mut next_w = w;
                    edit_single_weight(term, &mut next_w, ids[i])?;
                    if let Some(b) = evaluate_tui(term, &original, &corpus, &next_w)? {
                        if let Some(a) = evaluate_tui(term, &l, &corpus, &next_w)? {
                            w = next_w;
                            baseline = b;
                            current = a;
                        }
                    }
                }
            }
            Event::Mouse {
                x,
                y,
                button: 0,
                release: false,
                motion: false
            } => {
                match term.hit(&c, x, y, scroll) {
                    Some(Action::Key(i)) => drag = Some(i),
                    Some(Action::Metric(m)) => action_contributors(term, m, &l, &original, &baseline, &current)?,
                    Some(Action::Weight(i)) => {
                        let mut next_w = w;
                        edit_single_weight(term, &mut next_w, i)?;
                        if let Some(b) = evaluate_tui(term, &original, &corpus, &next_w)? {
                            if let Some(a) = evaluate_tui(term, &l, &corpus, &next_w)? {
                                w = next_w;
                                baseline = b;
                                current = a;
                            }
                        }
                    }
                    _ => {
                    }
                }
            }
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
            _ => {
            }
        }
        if let Some((a, b)) = swap {
            if l.space(a) || l.space(b) {
                continue;
            }
            let mut next = l.clone();
            next.swap(a, b);
            match evaluate_tui(term, &next, &corpus, &w) {
                Ok(Some(v)) => {
                    undo.push(l);
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

fn action_simple_rows(
    metric: usize,
    layout: &ak::Layout,
    before: &Evaluated,
    after: &Evaluated,
) -> Vec<(String, f64, f64)> {
    let mut physical = BTreeMap::<(Vec<usize>, bool), (f64, f64)>::new();
    for (side, evaluation) in [before, after].into_iter().enumerate() {
        let model = &evaluation.model;
        let positions = positions(&model.original);
        let denominator = match metric {
            0 => evaluation.corpus.totals[1],
            1 => evaluation.corpus.totals[2],
            2..=4 => evaluation.corpus.totals[1] + evaluation.corpus.totals[2],
            5 => evaluation.raw.0[SRAF_DEN],
            _ => evaluation.raw.0[ROLL_DEN],
        };
        for gram in &evaluation.corpus.grams {
            let mut contribution = Raw::default();
            add_gram(&mut contribution, gram, &positions, &model.geometry, 1.0);
            let mass = match metric {
                0 => contribution.0[SFB],
                1 => contribution.0[SFS],
                2 => contribution.0[LSB] + contribution.0[LSS],
                3 => contribution.0[ROW1_BI] + contribution.0[ROW1_SK],
                4 => contribution.0[ROW2_BI] + contribution.0[ROW2_SK],
                5 => contribution.0[SRAF],
                _ => contribution.0[SIMPLE_ROLL],
            };
            let value = pct(mass.max(0.0), denominator);
            if value == 0.0 {
                continue;
            }
            let keys: Vec<_> = gram.ids[..gram.len]
                .iter()
                .map(|&id| positions[id])
                .collect();
            let values = physical.entry((keys, gram.kind == 2)).or_default();
            if side == 0 {
                values.0 += value;
            } else {
                values.1 += value;
            }
        }
    }

    let mut rows: Vec<_> = physical
        .into_iter()
        .map(|((keys, skip), (before, after))| {
            let label = keys
                .iter()
                .map(|&key| layout.slots[key].label.as_str())
                .collect::<Vec<_>>()
                .join(if skip { " _ " } else { " " });
            (label, before, after)
        })
        .collect();
    rows.sort_by(|a, b| b.2.total_cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
    rows
}

fn action_simple_contributors(
    term: &mut Terminal,
    metric: usize,
    layout: &ak::Layout,
    original: &ak::Layout,
    before: &Evaluated,
    after: &Evaluated,
) -> AppResult<()> {
    let source = action_simple_rows(metric, layout, before, after);
    // These IDs supply the common percentage unit and improvement colors;
    // the contributions and totals above are the actual simple metrics.
    let color_metric = [SFB, SFS, LSB, DSB, DSB, SRAF, ROLL][metric];
    let mut sort_change = false;
    let mut all = false;
    let mut scroll = 0;
    loop {
        let mut rows = source.clone();
        if sort_change {
            rows.sort_by(|a, b| {
                (b.2 - b.1)
                    .abs()
                    .total_cmp(&(a.2 - a.1).abs())
                    .then_with(|| a.0.cmp(&b.0))
            });
        }
        let mut frame = action_detail_frame_named(
            term.width(),
            color_metric,
            SIMPLE_NAMES[metric],
            layout,
            original,
            &after.corpus.name,
            &rows,
            (before.metrics.simple[metric], after.metrics.simple[metric]),
            sort_change,
            all,
            false,
        );
        // Simple contributions combine metric classes, so reversed-pair
        // aggregation is intentionally not offered on this screen.
        for x in 0..frame.w {
            frame.cells[frame.w + x] = Cell { ch: ' ', color: FG };
        }
        frame.text(0, 1, "d sort | a all/top | ? help | q back", MUTED);
        term.present(&frame, scroll)?;
        let event = term.event()?;
        if scroll_event(&event, &mut scroll, frame.h, term.size.1) {
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
            Event::Char('?') => info_page(term, SIMPLE_NAMES[metric], &[
                "Keys are actual physical slots, including winning action keys, not emitted text.".into(),
                "The selected corpus is shown. The optimizer may use a weighted corpus mixture.".into(),
                "d sorts by absolute change; a switches all/top 16 contributions.".into(),
            ])?,
            _ => {}
        }
    }
}

fn action_default_locks(layout: &ak::Layout, generation: bool) -> Vec<bool> {
    (0..layout.slots.len())
        .map(|i| {
            layout.space(i)
                || (!generation
                    && (layout.home(i)
                        || !layout.slots[i].main
                        || matches!(layout.slots[i].binding, ak::Binding::Named(_))))
        })
        .collect()
}

fn action_choose_design(
    term: &mut Terminal,
    layout: &ak::Layout,
    settings: &mut SearchSettings,
    locks: &mut Vec<bool>,
) -> AppResult<()> {
    if let Some(choice) = menu(
        term,
        "",
        &["Refine".into(), "Random".into(), "Evolve".into()],
    )? {
        let was_generation = settings.design != "refine";
        settings.design = ["refine", "random", "evolve"][choice].into();
        if was_generation != (settings.design != "refine") {
            *locks = action_default_locks(layout, settings.design != "refine");
        }
    }
    Ok(())
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
) -> AppResult<bool> {
    let mut scroll = 0;
    loop {
        let frame = action_setup_frame(
            term.width(),
            layout,
            original,
            corpus,
            weights,
            locks,
            settings,
            status,
        );
        term.present(&frame, scroll)?;
        let mut event = term.event()?;
        if scroll_event(&event, &mut scroll, frame.h, term.size.1) {
            continue;
        }
        if let Some(action) = action_press(term, &frame, &event, scroll) {
            match action {
                Action::Key(i) => {
                    if !layout.space(i) {
                        locks[i] = !locks[i];
                    }
                    continue;
                }
                Action::Weight(i) => {
                    if let Err(error) = edit_single_weight(term, weights, i) {
                        *status = error.to_string();
                    }
                    continue;
                }
                Action::SimpleWeight(i) => {
                    if let Some(value) =
                        input_box(term, SIMPLE_NAMES[i], "", &settings.simple[i].to_string())?
                    {
                        match finite_nonnegative(&value) {
                            Ok(value) => {
                                settings.simple[i] = value;
                                settings.preset = "custom".into();
                            }
                            Err(error) => *status = error.to_string(),
                        }
                    }
                    continue;
                }
                Action::Setting(i) => {
                    let result = match i {
                        14 => action_choose_design(term, layout, settings, locks),
                        16 => choose_metrics(term, settings),
                        17 => choose_preset(term, settings),
                        _ => edit_setting(term, settings, i),
                    };
                    if let Err(error) = result {
                        *status = error.to_string();
                    }
                    continue;
                }
                Action::Command(ch) => event = Event::Char(ch),
                _ => {}
            }
        }

        match event {
            Event::Char(' ') => {
                validate_search_settings(settings)?;
                return Ok(true);
            }
            Event::Escape | Event::Quit | Event::Char('q') => return Ok(false),
            Event::Char('H') => *locks = action_default_locks(layout, false),
            Event::Char('U') => *locks = action_default_locks(layout, true),
            Event::Char('L') => locks.fill(true),
            Event::Char('o') => *layout = original.clone(),
            Event::Char('n') => settings.seed = new_seed(),
            Event::Char('g') => action_choose_design(term, layout, settings, locks)?,
            Event::Char('m') => choose_metrics(term, settings)?,
            Event::Char('p') => choose_preset(term, settings)?,
            Event::Char('x') => edit_mix(term, settings)?,
            Event::Char('w') => {
                let ids: Vec<_> = (0..N_WEIGHTS).filter(|&i| !aggregate(i)).collect();
                let names: Vec<_> = ids.iter().map(|&i| WEIGHT_NAMES[i].to_string()).collect();
                if let Some(i) = menu(term, "Typing effort weights", &names)? {
                    edit_single_weight(term, weights, ids[i])?;
                }
            }
            Event::Char('d') => {
                *weights = Weights::default();
                *settings = SearchSettings::default();
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
                    *status = "reloaded configuration".into();
                }
                Err(error) => *status = error.to_string(),
            },
            Event::Char('i') => info_page(term, "Corpus", &corpus.warnings)?,
            Event::Char('?') => info_page(
                term,
                "Optimizer",
                &[
                    "Space run; s save configuration; r reload; d defaults; o original.".into(),
                    "g design; m simple/detailed; p preset; n new seed; x corpus mixture.".into(),
                    "H home/action locks; U unlock except Space; L lock all. Mouse toggles locks.".into(),
                    "Limits are increases relative to the original on each training corpus.".into(),
                    "Travel limits use u/100; SFB/SFS use percentage points. none disables.".into(),
                    "Detailed weights also select typing effort, including in simple mode; w edits them.".into(),
                ],
            )?,
            _ => {}
        }
    }
}

fn action_optimizer_search(
    term: &mut Terminal,
    original: &ak::Layout,
    seed: &ak::Layout,
    corpora: &[(ng::NgramCorpus, f64)],
    weights: &Weights,
    settings: &SearchSettings,
    locks: &[bool],
) -> AppResult<Option<Snapshot>> {
    let original_owned = original.clone();
    let seed_owned = seed.clone();
    let training = corpora.to_vec();
    let worker_weights = *weights;
    let worker_settings = settings.clone();
    let worker_locks = locks.to_vec();
    let control = Arc::new(Control::new());
    let worker_control = Arc::clone(&control);
    let (updates_tx, updates_rx) = mpsc::sync_channel(2);
    let (done_tx, done_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        let mut callback = |snapshot, baseline, raw, totals| {
            let _ = updates_tx.try_send((snapshot, baseline, raw, totals));
        };
        let result = crate::action_search::run(
            &original_owned,
            &seed_owned,
            &training,
            &worker_weights,
            &worker_settings,
            &worker_locks,
            &worker_control,
            &mut callback,
        );
        let _ = done_tx.send(result);
    });

    let mut snapshot: Option<(Snapshot, Metrics, Raw, [f64; 4])> = None;
    let mut scroll = 0;
    let result = (|| -> AppResult<Option<Snapshot>> {
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
                .map(|(value, _, _, _)| crate::action_search::arrangement(seed, &value.best.arr))
                .unwrap_or_else(|| seed.clone());
            let mut frame = Canvas::optimizer(term.width(), 96);
            header(
                &mut frame,
                "Optimizing",
                &corpora[0].0.name,
                &seed.name,
                "p pause | q back",
            );
            let mut y = action_keyboard(&mut frame, 3, &layout, original, Some(locks), None) + 1;
            if let Some((snapshot, baseline, raw, totals)) = &snapshot {
                let progress = &snapshot.progress;
                let phase = if control.pause.load(Ordering::Relaxed) {
                    "paused"
                } else {
                    &progress.phase
                };
                let current = metrics_totals(raw, totals);
                y = score_panel(
                    &mut frame,
                    y,
                    &action_score_breakdown(baseline, weights, settings),
                    &action_score_breakdown(&current, weights, settings),
                    None,
                    term.decimals(),
                );
                if snapshot.has_best {
                    frame.text(
                        0,
                        y,
                        &format!(
                            "Search objective: {} ({}, training mixture)",
                            number(snapshot.best.score, term.decimals()),
                            settings.mode,
                        ),
                        CYAN,
                    );
                    y += 2;
                }
                if settings.mode == "simple" {
                    y = simple_metric_table(&mut frame, y, baseline, &current, term.decimals()) + 1;
                    frame.text(
                        0,
                        y,
                        &format!(
                            "Travel {} → {} u/100   SF {} → {} u/100",
                            fmt2(baseline.v[TRAVEL]),
                            fmt2(current.v[TRAVEL]),
                            fmt2(baseline.v[SFTRAVEL]),
                            fmt2(current.v[SFTRAVEL]),
                        ),
                        MUTED,
                    );
                    y += 2;
                } else {
                    y = grouped_metric_totals(
                        &mut frame,
                        y,
                        baseline,
                        &current,
                        raw,
                        totals,
                        term.decimals(),
                    );
                    y = finger_table(&mut frame, y + 1, baseline, &current, term.decimals());
                }
                frame.text(
                    0,
                    y,
                    &format!(
                        "{}  {}/{}  {} trials  {} accepted  {:.1}s",
                        phase,
                        progress.restart,
                        settings.restarts,
                        progress.evaluations,
                        progress.accepted,
                        progress.elapsed,
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
    result
}

fn action_score_breakdown(
    metrics: &Metrics,
    weights: &Weights,
    settings: &SearchSettings,
) -> Breakdown {
    if settings.mode == "simple" {
        simple_breakdown(metrics, &settings.simple)
    } else {
        breakdown(metrics, weights)
    }
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
        "r setup | Space refine | b compare | [ ] | s save | S batch | q back",
    );
    let mut y = action_keyboard(&mut frame, 2, layout, original, Some(locks), None) + 1;
    y = score_panel(
        &mut frame,
        y,
        &action_score_breakdown(&before.metrics, weights, settings),
        &action_score_breakdown(&after.metrics, weights, settings),
        None,
        term.decimals(),
    );
    frame.text(
        0,
        y,
        &format!(
            "Search objective: {} ({}, training mixture) · {}-gram estimate",
            number(objective, term.decimals()),
            settings.mode,
            after.counts.order,
        ),
        CYAN,
    );
    y += 2;

    if settings.mode == "simple" {
        y = simple_metric_table(
            &mut frame,
            y,
            &before.metrics,
            &after.metrics,
            term.decimals(),
        ) + 1;
        frame.text(
            0,
            y,
            &format!(
                "Travel {} → {} u/100   SF {} → {} u/100",
                fmt2(before.metrics.v[TRAVEL]),
                fmt2(after.metrics.v[TRAVEL]),
                fmt2(before.metrics.v[SFTRAVEL]),
                fmt2(after.metrics.v[SFTRAVEL]),
            ),
            MUTED,
        );
        y += 2;
    } else {
        y = grouped_metric_cards(
            &mut frame,
            y,
            &before.metrics,
            &after.metrics,
            &after.raw,
            &after.corpus,
            term.decimals(),
        );
        y = finger_table(
            &mut frame,
            y + 1,
            &before.metrics,
            &after.metrics,
            term.decimals(),
        );
    }
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
) -> AppResult<PathBuf> {
    let path = layout
        .save_new()
        .map_err(|error| format!("layout save: {error}"))?;
    let mut report = format!(
        "model = {MODEL_VERSION}\nseed = 0x{:x}\ntrials = {}\nseconds = {:.6}\nobjective = {:.17}\n\n[weights]\n{}\n[rolls]\n{}\n[search]\n{}\n[original]\n{}\n[start]\n{}\n[result]\n{}\n[locks]\n{:?}\n",
        progress.seed, progress.evaluations, progress.elapsed, candidate.score,
        weights_text(weights), rolls_config_text(weights.rolls()), search_settings_text(settings),
        original.text(), start.text(), layout.text(),
        locks.iter().enumerate().filter_map(|(i, &locked)| locked.then_some(i)).collect::<Vec<_>>(),
    );
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
    let mut weights = config.weights;
    let mut settings = config.search;
    let mut layout = original.clone();
    let mut locks = action_default_locks(&layout, settings.design != "refine");
    let mut status = String::new();

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
        )? {
            return Ok(());
        }
        let Some(training) = crate::session::load_action_training(term, &corpus, &settings)? else {
            continue;
        };
        let mut before: Option<Arc<Evaluated>> = None;
        'runs: loop {
            let start = layout.clone();
            let Some(result) = action_optimizer_search(
                term, &original, &start, &training, &weights, &settings, &locks,
            )?
            else {
                return Ok(());
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
                layout = crate::action_search::arrangement(&start, &candidate.arr);
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
                        Action::SimpleMetric(metric) => action_simple_contributors(
                            term, metric, &layout, &original, baseline, current,
                        )?,
                        _ => {}
                    }
                }

                match event {
                    Event::Escape | Event::Quit | Event::Char('q') => return Ok(()),
                    Event::Char('.') => term.precise = !term.precise,
                    Event::Char('r') => {
                        layout = original.clone();
                        break 'runs;
                    }
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
                                    "{}  objective {}",
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
                            &start,
                            &weights,
                            &settings,
                            &locks,
                            &training,
                            candidate,
                            &result.progress,
                        ) {
                            Ok(path) => saved_layout_message(&path),
                            Err(error) => error.to_string(),
                        };
                    }
                    Event::Char('S') => {
                        let mut saved = 0;
                        for candidate in &result.archive {
                            let candidate_layout =
                                crate::action_search::arrangement(&start, &candidate.arr);
                            match save_action_result(
                                &candidate_layout,
                                &original,
                                &start,
                                &weights,
                                &settings,
                                &locks,
                                &training,
                                candidate,
                                &result.progress,
                            ) {
                                Ok(_) => saved += 1,
                                Err(error) => {
                                    status = format!(
                                        "saved {saved} candidates (.dat + .jsonc); {error}"
                                    );
                                    break;
                                }
                            }
                        }
                        if saved == result.archive.len() {
                            status =
                                format!("saved {saved} candidates (.dat + .jsonc) and run records");
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
enum SearchMsg {
    Progress(u64, f64),
    Update(usize, usize, u64, f64),
    Done(ak::Layout, u64),
    Error(String),
}

#[cfg(test)]
fn run_action_search<const PROFILE: bool, const DETAIL: bool>(
    initial: ak::Layout,
    text: &ng::NgramCorpus,
    weights: &Weights,
    locked: &[bool],
    limits: (f64, f64),
    cancel: &AtomicBool,
    tx: &mpsc::Sender<SearchMsg>,
    profile: &mut crate::action_profile::Profile,
) -> ak::Result<(ak::Layout, u64)> {
    use crate::action_profile::{clock, elapsed};
    let setup_start = clock::<PROFILE>();
    let mut best = initial;
    let effort = LocalEffort::new(&best, weights);
    let cache_result = ng::Incremental::new(
        text,
        &best,
        Geometry::with_rolls(physical_keys(&best), weights.rolls()),
        cancel,
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
            if locked[a] || best.space(a) {
                continue;
            }
            for b in a + 1..best.slots.len() {
                if locked[b] || best.space(b) {
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
                    let _ = tx.send(SearchMsg::Update(a, b, trials, score));
                } else {
                    if PROFILE {
                        profile.rejected += 1;
                    }
                    if trials % 16 == 0 {
                        let _ = tx.send(SearchMsg::Progress(trials, score));
                    }
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
    for warning in &corpus.warnings {
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
    Ok(v)
}

fn corpus_arg(args: &[String], i: usize) -> AppResult<PathBuf> {
    match args.get(i) {
        Some(name) => corpus_by_name(name),
        None => default_corpus_path()?.ok_or_else(|| "select a corpus".into()),
    }
}

pub(crate) fn inspect_ranked(
    term: &mut Terminal,
    layout: &ak::Layout,
    evaluation: &Evaluated,
    weights: &Weights,
) -> AppResult<()> {
    let mut scroll = 0;
    loop {
        let canvas = action_frame(
            term, "Layout", layout, layout, evaluation, evaluation, weights, None, None, "",
        );
        term.present(&canvas, scroll)?;
        let event = term.event()?;
        if scroll_event(&event, &mut scroll, canvas.h, term.size.1) {
            continue;
        }
        if let Some(Action::Metric(metric)) = action_press(term, &canvas, &event, scroll) {
            action_contributors(term, metric, layout, layout, evaluation, evaluation)?;
        }
        match event {
            Event::Escape | Event::Quit | Event::Char('q') => return Ok(()),
            Event::Char('.') => term.precise = !term.precise,
            Event::Char('i') => show_corpus_info(term, &evaluation.corpus)?,
            Event::Char('t') => trace_view(term, layout, weights, false)?,
            Event::Char('p') => trace_view(term, layout, weights, true)?,
            _ => {}
        }
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
    fn ranked_action_details_keep_root_physical_key_names() {
        let layout = history_layout();
        let corpus = history_corpus(b"aan");
        // Zero metric weights isolate the intended action win over the
        // hardcoded same-key repeat cost without relying on saved defaults.
        let weights = Weights::new([0.0; N_WEIGHTS]);
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
            false,
            true,
            false,
        );
        assert!(canvas_text(&canvas).contains("@ n"));
    }

    #[test]
    fn action_view_helpers_preserve_physical_names_selection_and_all_rows() {
        assert!(action_controls(true).contains("Space run"));
        assert!(!action_controls(false).contains("Space run"));
        assert!(action_controls(false).contains("save copy"));
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
        let empty =
            action_detail_frame(80, SFB, &l, &l, "test", &[], (0.0, 0.0), false, true, false);
        assert!(canvas_text(&empty).contains("No contributing physical sequences"));
    }
    #[test]
    fn magic_setup_uses_normal_weight_grid_and_shared_search_settings() {
        let layout = history_layout();
        let corpus = history_corpus(b"aan");
        let weights = Weights::default();
        let settings = SearchSettings::default();
        let locks = vec![false; layout.slots.len()];
        for width in [40, 80, 120] {
            let canvas = action_setup_frame(
                width, &layout, &layout, &corpus, &weights, &locks, &settings, "ready",
            );
            let text = canvas_text(&canvas);
            for label in [
                "Optimizer",
                "Locked",
                "Free",
                "Weights",
                "SFB",
                "SFS",
                "ready",
            ] {
                assert!(text.contains(label), "missing {label} at width {width}");
            }
            assert!(text.contains("Restarts"));
            assert!(text.contains("Temp start"));
            assert!(text.contains("Travel Δ max"));
            assert!(text.contains("hybrid"));
            for setting in 0..settings_labels(&settings).len() {
                assert!(canvas.hits.iter().any(|(_, action)| matches!(
                    action, Action::Setting(index) if *index == setting
                )));
            }
            assert!(canvas
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
    }
    #[test]
    fn magic_simple_setup_exposes_weights_and_all_shared_controls() {
        let layout = history_layout();
        let corpus = history_corpus(b"aan");
        let weights = Weights::default();
        let mut settings = SearchSettings::default();
        settings.mode = "simple".into();
        settings.hybrid = false;
        settings.mix.insert("history".into(), 2.0);
        let locks = action_default_locks(&layout, false);
        let canvas = action_setup_frame(
            100, &layout, &layout, &corpus, &weights, &locks, &settings, "ready",
        );
        let text = canvas_text(&canvas);
        assert!(text.contains("sweep"));
        assert!(text.contains("history:2"));
        for i in 0..7 {
            assert!(canvas.hits.iter().any(|(_, action)| matches!(
                action, Action::SimpleWeight(index) if *index == i
            )));
        }
        assert!(canvas
            .hits
            .iter()
            .any(|(_, action)| matches!(action, Action::Command('x'))));
        assert!(!canvas
            .hits
            .iter()
            .any(|(_, action)| matches!(action, Action::Weight(_))));
    }

    #[test]
    fn action_default_locks_preserve_refine_safety_and_allow_generation() {
        let layout = history_layout();
        let refine = action_default_locks(&layout, false);
        let generation = action_default_locks(&layout, true);
        let action = history_key(&layout, "@");
        assert!(refine[action]);
        assert!(!generation[action]);
        for i in 0..layout.slots.len() {
            assert_eq!(generation[i], layout.space(i));
            if layout.space(i) || layout.home(i) || !layout.slots[i].main {
                assert!(refine[i]);
            }
        }
    }

    #[test]
    fn optimizer_objective_mode_does_not_change_the_typing_effort_policy() {
        let layout = history_layout();
        let corpus = history_corpus(b"aanwhnaap");
        let weights = Weights::default();
        let evaluation = evaluate(&layout, &corpus, &weights, &AtomicBool::new(false)).unwrap();
        let mut settings = SearchSettings::default();
        assert_eq!(
            action_score_breakdown(&evaluation.metrics, &weights, &settings)
                .net
                .to_bits(),
            evaluation.score.to_bits(),
        );
        let effort_before = LocalEffort::new(&layout, &weights);
        settings.mode = "simple".into();
        settings.simple = [3.0, 7.0, 1.0, 2.0, 5.0, 0.5, 0.8];
        assert_eq!(
            action_score_breakdown(&evaluation.metrics, &weights, &settings)
                .net
                .to_bits(),
            simple_breakdown(&evaluation.metrics, &settings.simple)
                .net
                .to_bits(),
        );
        let effort_after = LocalEffort::new(&layout, &weights);
        assert_eq!(effort_before.uni, effort_after.uni);
        assert_eq!(effort_before.bi, effort_after.bi);
        assert_eq!(effort_before.sk, effort_after.sk);
    }

    #[test]
    fn simple_action_details_use_physical_labels_and_exact_simple_components() {
        let layout = history_layout();
        let corpus = history_corpus(b"aanwaapnwhn");
        let weights = Weights::new([0.0; N_WEIGHTS]);
        let evaluation = evaluate(&layout, &corpus, &weights, &AtomicBool::new(false)).unwrap();
        for metric in 0..7 {
            let rows = action_simple_rows(metric, &layout, &evaluation, &evaluation);
            let total: f64 = rows.iter().map(|row| row.2).sum();
            assert!((total - evaluation.metrics.simple[metric]).abs() < 1e-9);
            assert!(rows.iter().all(|row| row.1.to_bits() == row.2.to_bits()));
        }
        let rows = action_simple_rows(0, &layout, &evaluation, &evaluation);
        assert!(rows.iter().any(|row| row.0 == "@ n" && row.2 > 0.0));
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
            false,
            false,
            false,
        );
        let text = canvas_text(&canvas);
        for label in [
            "Before", "After", "Change", "Other", "@ n 15", "○", "●", "2.0000", "4.0000",
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
        let w = Weights::new([0.0; N_WEIGHTS]);
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
        for label in ["Penalty", "Credit", "Objective"] {
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
        // A matching action loses a tie to the literal; mismatch/no-output
        // attempts do not create extra presses. ! is a real history boundary.
        assert_physical_mapping(&l, b"aan", &[Some(a), Some(a), Some(n)], false);
        assert_physical_mapping(&l, b"a!an", &[Some(a), None, Some(a), Some(n)], true);
        assert_physical_mapping(
            &l,
            b"aaaaa",
            &[Some(a), Some(m), Some(m), Some(m), Some(m)],
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
    fn default_effort_can_prefer_literal_repeat_over_action() {
        let l = history_layout();
        let a = history_key(&l, "a");
        let action = history_key(&l, "@");
        let weights = Weights::default();
        let effort = LocalEffort::new(&l, &weights);
        assert!(effort.get(None, Some(a), a) < effort.get(None, Some(a), action));

        let stop = AtomicBool::new(false);
        let detailed = evaluate(&l, &history_corpus(b"aan"), &weights, &stop).unwrap();
        let n = history_key(&l, "n");
        assert_eq!(detailed.counts.tables[2].get(&vec![a, a, n]), Some(&1.0));
        assert_eq!(detailed.counts.action_presses, 0.0);
    }

    #[test]
    fn action_n_and_action_p_are_physical_sfb_in_both_evaluators() {
        let l = history_layout();
        let m = history_key(&l, "@");
        let a = history_key(&l, "a");
        // Metric classification is independent of weights. Make the repeat
        // action win so this test measures physical history, not default policy.
        let w = Weights::new([0.0; N_WEIGHTS]);
        let effort = LocalEffort::new(&l, &w);
        assert!(effort.get(None, Some(a), m) < effort.get(None, Some(a), a));

        let stop = AtomicBool::new(false);
        for (text, label) in [(b"aan".as_slice(), "n"), (b"aap".as_slice(), "p")] {
            let k = history_key(&l, label);
            let corpus = history_corpus(text);
            let detailed = evaluate(&l, &corpus, &w, &stop).unwrap();
            assert_eq!(detailed.counts.tables[2].get(&vec![a, m, k]), Some(&1.0));
            assert_eq!(detailed.counts.tables[1].get(&vec![m, k]), Some(&1.0));
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
                    let simple = action_simple_rows(6, &layout, &full, &full);
                    let total: f64 = simple.iter().map(|(_, _, after)| after).sum();
                    assert!((total - full.metrics.simple[6]).abs() < 1e-9);

                    let effort = LocalEffort::new(&layout, &weights);
                    let mut cache = ng::Incremental::new(
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
            let mut cache = ng::Incremental::new(
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
            assert_eq!(x.counts.tables[2].get(&ids.to_vec()), Some(&1.0));
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
        for (a, b) in actual.0.simple.iter().zip(expected.metrics.simple) {
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
                    if matches!(layout.slots[a].binding, ak::Binding::Named(_))
                        || matches!(layout.slots[b].binding, ak::Binding::Named(_))
                    {
                        assert!(proposal.all_contexts);
                    }
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
        let mut cache = ng::Incremental::new(
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
        let (tx, _rx) = mpsc::channel();
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
            &tx,
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
            &tx,
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
            &tx,
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
                        assert_eq!(
                            actual.metrics.simple.map(f64::to_bits),
                            expected.metrics.simple.map(f64::to_bits)
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
