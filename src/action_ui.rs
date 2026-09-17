//! Action-aware entry points. Plain layouts continue to use the original evaluator/search.
use crate::action_keys as ak;
use crate::action_ngrams as ng;
use crate::*;

#[derive(Clone)]
struct Evaluated {
    model: Model,
    corpus: Corpus,
    raw: Raw,
    metrics: Metrics,
    score: f64,
    counts: ng::Counts,
}
fn physical_keys(l: &ak::Layout) -> Vec<Key> {
    l.slots
        .iter()
        .map(|s| {
            let mut k = main_key(0, 0);
            k.row = s.row as _;
            k.col = s.col as _;
            k.finger = s.finger as _;
            k.rank = s.rank as _;
            k.hand = s.hand as _;
            k.main = s.main;
            k
        })
        .collect()
}
struct LocalEffort {
    uni: Vec<f64>,
    bi: Vec<f64>,
    sk: Vec<f64>,
    n: usize,
}
impl LocalEffort {
    fn new(l: &ak::Layout, w: &Weights) -> Self {
        let keys = physical_keys(l);
        let n = keys.len();
        let mut uni = vec![0.0; n];
        let mut bi = vec![0.0; n * n];
        let mut sk = vec![0.0; n * n];
        for i in 0..n {
            if keys[i].main {
                let h = [0.0, 1.0, 2.0, 3.0, 6.0, 7.0, 8.0, 9.0][keys[i].finger];
                uni[i] = 0.02 * (keys[i].col as f64 - h).hypot(keys[i].row as f64 - 1.0);
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
    fn get(&self, a: Option<usize>, b: Option<usize>, next: usize) -> f64 {
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
fn evaluate_progress(
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
    let model = Model::new(board);
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
        warnings: vec![format!("{}-gram bounded-context magic estimate", c.order)],
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
fn ngram_job<T, F>(
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
    let owned = path.to_owned();
    ngram_job(
        term,
        "Loading cached n-grams",
        &corpus_name(path),
        "",
        move |stop, progress| ng::NgramCorpus::load_progress(&owned, stop, progress),
    )
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
    let cols = 10 + usize::from(l.left_outer) + usize::from(l.right_outer);
    let gap = 3;
    let fits = |kw: usize| cols * (kw + 1) - 1 + gap <= c.w;
    let kw: usize = if fits(7) {
        7
    } else if fits(5) {
        5
    } else {
        4
    };
    let step = kw + 1;
    let width = cols * step - 1 + gap;
    let x = c.w.saturating_sub(width) / 2;
    let min = if l.left_outer { -1 } else { 0 };
    for (i, s) in l.slots.iter().enumerate() {
        let (kx, ky) = if s.main {
            (
                x + (s.col - min) as usize * step + if s.hand == 1 { gap } else { 0 },
                y + s.row as usize * 3,
            )
        } else {
            (x + width / 2 - kw - 2 + (s.finger - 8) * (kw + 3), y + 9)
        };
        let locked = locks.is_some_and(|v| v[i]);
        let color = if s.binding != original.slots[i].binding {
            BLUE
        } else if locked {
            YELLOW
        } else {
            FG
        };
        let r = Rect {
            x: kx,
            y: ky,
            w: kw,
            h: 3,
        };
        c.boxed(r, if locked { YELLOW } else { BORDER });
        let label = short(&s.label, kw - 2);
        c.center(kx + 1, ky + 1, kw - 2, &label, color);
        if selected == Some(i) {
            c.put(kx + 1, ky + 1, '›', FG);
        }
        c.hit(r, Action::Key(i));
    }
    y + 12
}
fn action_controls(optimize: bool) -> &'static str {
    if optimize {
        "Space run | U unlock | w weights | s save copy | . digits | q back"
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
    header(
        &mut c,
        &format!("{title} ({}-gram estimate)", a.counts.order),
        &a.corpus.name,
        &l.name,
        action_controls(locks.is_some()),
    );
    let mut y = action_keyboard(&mut c, 3, l, base, locks, selected) + 1;
    y = score_panel(
        &mut c,
        y,
        &breakdown(&b.metrics, w),
        &breakdown(&a.metrics, w),
        None,
        term.decimals(),
    );
    c.text(
        0,
        y,
        &short(
            &format!(
                "Physical presses {}   ignored {}",
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
fn action_detail_frame(
    width: usize,
    m: usize,
    l: &ak::Layout,
    corpus: &str,
    rows: &[(String, f64, f64)],
    sort_change: bool,
) -> Canvas {
    let mut c = Canvas::new(width, rows.len() + 7);
    header(
        &mut c,
        METRIC_NAMES[m],
        corpus,
        &l.name,
        "d sort | ? help | PgUp/PgDn scroll | q back",
    );
    c.text(
        0,
        3,
        &short(
            &format!(
                "{} contributions · {} · before = loaded baseline",
                metric_unit(m),
                if sort_change {
                    "absolute change"
                } else {
                    "after contribution"
                }
            ),
            width,
        ),
        MUTED,
    );
    let name_w = width.saturating_sub(33).clamp(8, 32);
    let x = name_w + 1;
    c.text(0, 4, "Physical keys", MUTED);
    c.right(x, 4, 9, "Before", MUTED);
    c.right(x + 11, 4, 9, "After", MUTED);
    c.right(x + 22, 4, 9, "Change", MUTED);
    c.line(0, 5, width, BORDER);
    for (i, (name, b, a)) in rows.iter().enumerate() {
        let y = i + 6;
        c.text(0, y, &short(name, name_w), FG);
        c.right(x, y, 9, &number(*b, DETAIL_DECIMALS), MUTED);
        c.right(
            x + 11,
            y,
            9,
            &number(*a, DETAIL_DECIMALS),
            change_color(m, *b, *a),
        );
        c.right(
            x + 22,
            y,
            9,
            &delta_text(*b, *a, DETAIL_DECIMALS),
            delta_color(m, *b, *a),
        );
    }
    if rows.is_empty() {
        c.text(0, 6, "No contributing physical sequences", MUTED);
    }
    c
}
fn action_contributors(
    term: &mut Terminal,
    m: usize,
    l: &ak::Layout,
    before: &Evaluated,
    after: &Evaluated,
) -> AppResult<()> {
    let mut rows = action_contributor_rows(m, l, before, after);
    let mut scroll = 0;
    let mut sort_change = false;
    loop {
        let c = action_detail_frame(term.width(), m, l, &after.corpus.name, &rows, sort_change);
        term.present(&c, scroll)?;
        let e = term.event()?;
        if scroll_event(&e, &mut scroll, c.h, term.size.1) {
            continue;
        }
        match e {
            Event::Escape|Event::Quit|Event::Char('q')|Event::Enter=>return Ok(()),
            Event::Char('d')=>{
                sort_change=!sort_change;
                rows.sort_by(|a,b|if sort_change{(b.2-b.1).abs().total_cmp(&(a.2-a.1).abs()).then_with(||a.0.cmp(&b.0))}
                    else{b.2.total_cmp(&a.2).then_with(||a.0.cmp(&b.0))});scroll=0;
            },
            Event::Char('?')=>info_page(term,METRIC_NAMES[m],&[METRIC_HELP[m].into(),
                "These are physical slots pressed, including action keys; they are not emitted text.".into(),
                "Before is the loaded layout under the selected corpus and weights. After is the current layout.".into(),
                "Both-zero rows are omitted. All remaining rows are scrollable; — means unchanged.".into()])?,
            _=>{},
        }
    }
}
fn choose_corpus(term: &mut Terminal) -> AppResult<Option<PathBuf>> {
    let mut paths = corpus_paths()?;
    paths.retain(|p| p.extension().is_some_and(|v| v == "json"));
    paths.sort();
    let names: Vec<_> = paths.iter().map(|p| p.display().to_string()).collect();
    Ok(menu(term, "Cached n-grams", &names)?.map(|i| paths[i].clone()))
}
pub(crate) fn action_editor(
    term: &mut Terminal,
    mut l: ak::Layout,
    mut corpus: ng::NgramCorpus,
    optimize: bool,
) -> AppResult<()> {
    let mut timing =
        crate::load_profile::LoadProfile::new("action editor/optimizer initialization");
    let original = l.clone();
    let mut w = load_weights(Path::new(WEIGHTS_FILE))?;
    let Some(mut baseline) = evaluate_tui(term, &original, &corpus, &w)? else {
        return Ok(());
    };
    timing.mark("Baseline evaluation (nested report)");
    let mut current = baseline.clone();
    let mut locks: Vec<bool> = (0..l.slots.len())
        .map(|i| {
            l.home(i) || !l.slots[i].main || matches!(l.slots[i].binding, ak::Binding::Named(_))
        })
        .collect();
    let mut undo: Vec<ak::Layout> = Vec::new();
    let (mut selected, mut drag) = (None, None);
    let mut scroll = 0;
    let mut status = String::new();
    timing.mark("Shared current state, locks and editor state");
    let mut opening = Some(timing);
    loop {
        let c = action_frame(
            term,
            if optimize {
                "Action optimizer"
            } else {
                "Action editor"
            },
            &l,
            &original,
            &baseline,
            &current,
            &w,
            if optimize { Some(&locks) } else { None },
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
            Event::Escape if selected.is_some()||drag.is_some()=>{selected=None;drag=None;},
            Event::Escape|Event::Quit|Event::Char('q')=>return Ok(()),
            Event::Char('.')=>term.precise=!term.precise,
            Event::Char('?')=>info_page(term,"Action controls",&[action_controls(optimize).into(),
                "t traces text; p traces physical key presses; c changes corpus; w edits weights.".into(),
                "Click metrics for physical contributions. Action keys retain their names. Save creates a new copy.".into()])?,
            Event::Char('t')=>trace_view(term,&l,&w,false)?,Event::Char('p')=>trace_view(term,&l,&w,true)?,
            Event::Char('s')=>status=match l.save_new(){Ok(p)=>format!("saved {}",p.display()),Err(e)=>e},
            Event::Char('r')=>{undo.push(l.clone());l=original.clone();current=baseline.clone();},
            Event::Char('u') if !optimize=>if let Some(old)=undo.last(){
                if let Some(value)=evaluate_tui(term,old,&corpus,&w)?{l=undo.pop().unwrap();current=value;}
            },
            Event::Char('U') if optimize=>{for(i,lock)in locks.iter_mut().enumerate(){*lock=l.space(i);}},
            Event::Char('c')=>if let Some(p)=choose_corpus(term)?{
                match load_corpus_tui(term,&p) {
                    Ok(Some(next))=>{
                        if let Some(b)=evaluate_tui(term,&original,&next,&w)? {
                            if let Some(a)=evaluate_tui(term,&l,&next,&w)? {corpus=next;baseline=b;current=a;status.clear();}
                        }
                    },Ok(None)=>{},Err(e)=>status=e.to_string(),
                }
            },
            Event::Char('w')=>{
                let ids:Vec<_>=(0..N_WEIGHTS).filter(|&i|!aggregate(i)).collect();
                let names:Vec<_>=ids.iter().map(|&i|WEIGHT_NAMES[i].to_string()).collect();
                if let Some(i)=menu(term,"",&names)?{
                    let mut next_w=w;edit_single_weight(term,&mut next_w,ids[i])?;
                    if let Some(b)=evaluate_tui(term,&original,&corpus,&next_w)? {
                        if let Some(a)=evaluate_tui(term,&l,&corpus,&next_w)? {w=next_w;baseline=b;current=a;}
                    }
                }
            }
            Event::Char(' ') if optimize=>{
                let settings=load_search_settings(Path::new(SEARCH_FILE))?;
                let caps=(settings.sfb_limit,settings.sfs_limit);
                let result=action_search(term,&l,&original,&corpus,&w,&locks,&baseline,caps)?;
                if let Some(best)=result{
                    if let Some(value)=evaluate_tui(term,&best,&corpus,&w)?{undo.push(l.clone());l=best;current=value;status="search finished".into();}
                }
            }
            Event::Mouse{x,y,button:0,release:false,motion:false}=>{
                match term.hit(&c,x,y,scroll){
                    Some(Action::Key(i)) if optimize=>{if !l.space(i){locks[i]=!locks[i];}},
                    Some(Action::Key(i))=>drag=Some(i),
                    Some(Action::Metric(m))=>action_contributors(term,m,&l,&baseline,&current)?,_=>{}
                }
            }
            Event::Mouse{x,y,button:0,release:true,..} if !optimize=>{
                if let Some(from)=drag.take(){if let Some(Action::Key(to))=term.hit(&c,x,y,scroll){
                    if from!=to{swap=Some((from,to));selected=None;}else if let Some(a)=selected.take(){if a!=to{swap=Some((a,to));}}else{selected=Some(to);}
                }}
            }
            _=>{}
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
// One sender preserves accepted-swap order; UI updates need only two indices.
enum SearchMsg {
    Progress(u64, f64),
    Update(usize, usize, u64, f64),
    Done(ak::Layout, u64),
    Error(String),
}
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
        Geometry::new(physical_keys(&best)),
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

fn action_search(
    term: &mut Terminal,
    start: &ak::Layout,
    original: &ak::Layout,
    corpus: &ng::NgramCorpus,
    w: &Weights,
    locks: &[bool],
    baseline: &Evaluated,
    caps: (Option<f64>, Option<f64>),
) -> AppResult<Option<ak::Layout>> {
    let profile_level = match std::env::var("LAYOUTER_PROFILE").as_deref() {
        Ok("1") => 1,
        Ok("2") => 2,
        _ => 0,
    };
    let initial = start.clone();
    let text = corpus.clone();
    let weights = *w;
    let locked = locks.to_vec();
    let limits = (
        baseline.metrics.v[SFB] + caps.0.unwrap_or(f64::INFINITY),
        baseline.metrics.v[SFS] + caps.1.unwrap_or(f64::INFINITY),
    );
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancel);
    let (tx, rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let mut profile = crate::action_profile::Profile::default();
        let started = if profile_level != 0 {
            Some(Instant::now())
        } else {
            None
        };
        // Dispatch once, outside all hot loops. The normal specialization has
        // no profiling clock reads or operation-counter updates.
        let result = match profile_level {
            1 => run_action_search::<true, false>(
                initial,
                &text,
                &weights,
                &locked,
                limits,
                &worker_cancel,
                &tx,
                &mut profile,
            ),
            2 => run_action_search::<true, true>(
                initial,
                &text,
                &weights,
                &locked,
                limits,
                &worker_cancel,
                &tx,
                &mut profile,
            ),
            _ => run_action_search::<false, false>(
                initial,
                &text,
                &weights,
                &locked,
                limits,
                &worker_cancel,
                &tx,
                &mut profile,
            ),
        };
        if let Some(started) = started {
            profile.total = started.elapsed();
            let status = if worker_cancel.load(Ordering::Relaxed) {
                "cancelled/partial"
            } else if result.is_err() {
                "error/partial"
            } else {
                "completed"
            };
            // One report per search. Redirect stderr to retain it outside the TUI.
            let _ = profile.report(&mut io::stderr().lock(), status, profile_level == 2);
        }
        match result {
            Ok((best, n)) => {
                let _ = tx.send(SearchMsg::Done(best, n));
            }
            Err(e) => {
                let _ = tx.send(SearchMsg::Error(e));
            }
        }
    });
    let mut best = start.clone();
    let mut status = "building incremental context cache".to_string();
    let mut leave = false;
    let mut error = None;
    let run = (|| -> AppResult<()> {
        loop {
            let mut done = false;
            while let Ok(m) = rx.try_recv() {
                match m {
                    SearchMsg::Progress(n, s) => {
                        status = format!("{n} trials   score {s:.4}");
                    }
                    SearchMsg::Update(a, b, n, s) => {
                        best.swap(a, b);
                        status = format!("{n} trials   score {s:.4}");
                    }
                    SearchMsg::Done(l, n) => {
                        best = l;
                        status = format!("{n} trials");
                        done = true;
                    }
                    SearchMsg::Error(e) => {
                        error = Some(e);
                        done = true;
                    }
                }
            }
            let mut c = Canvas::new(term.width(), 22);
            header(&mut c, "Action search", &corpus.name, &start.name, "q back");
            let y = action_keyboard(&mut c, 3, &best, original, Some(locks), None);
            c.text(0, y + 1, &status, CYAN);
            c.h = y + 3;
            term.present(&c, 0)?;
            if done {
                break;
            }
            match term.event()? {
                Event::Quit | Event::Escape | Event::Char('q') => {
                    leave = true;
                    cancel.store(true, Ordering::Relaxed);
                    break;
                }
                _ => {}
            }
            if handle.is_finished() {
                match rx.try_recv() {
                    Ok(SearchMsg::Progress(n, s)) => {
                        status = format!("{n} trials   score {s:.4}");
                    }
                    Ok(SearchMsg::Done(l, _)) => {
                        best = l;
                        break;
                    }
                    Ok(SearchMsg::Error(e)) => {
                        error = Some(e);
                        break;
                    }
                    Ok(SearchMsg::Update(a, b, n, s)) => {
                        best.swap(a, b);
                        status = format!("{n} trials   score {s:.4}");
                    }
                    Err(_) => {
                        error = Some("worker ended without a result".into());
                        break;
                    }
                }
            }
        }
        Ok(())
    })();
    cancel.store(true, Ordering::Relaxed);
    let joined = handle.join();
    run?;
    if joined.is_err() {
        return Err("action search worker panicked".into());
    }
    if leave {
        return Ok(None);
    }
    if let Some(e) = error {
        return Err(e.into());
    }
    Ok(Some(best))
}

fn print_evaluation(l: &ak::Layout, corpus: &ng::NgramCorpus) -> AppResult<Evaluated> {
    let w = load_weights(Path::new(WEIGHTS_FILE))?;
    let v = evaluate(l, corpus, &w, &AtomicBool::new(false))
        .map_err(|e| format!("action corpus: {e}"))?;
    println!(
        "{}  {}  {}-gram estimate",
        l.name, corpus.name, corpus.order
    );
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
fn paths_for_corpus() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in ["corpus/processed", "corpora"] {
        if let Ok(items) = fs::read_dir(dir) {
            for e in items.flatten() {
                let p = e.path();
                if p.extension()
                    .is_some_and(|s| s == "txt" || s == "seq" || s == "json")
                {
                    out.push(p);
                }
            }
        }
    }
    if Path::new("corpus-reddit.json").exists() {
        out.insert(0, PathBuf::from("corpus-reddit.json"));
    }
    out
}
fn corpus_arg(args: &[String], i: usize) -> AppResult<PathBuf> {
    if let Some(s) = args.get(i) {
        let p = PathBuf::from(s);
        if p.exists() {
            return Ok(p);
        }
        for p in paths_for_corpus() {
            if p.file_stem()
                .is_some_and(|v| v.to_string_lossy().trim_start_matches("corpus-") == s)
            {
                return Ok(p);
            }
        }
        return Err(format!("corpus {s:?} not found").into());
    }
    for p in paths_for_corpus() {
        if p.file_stem()
            .is_some_and(|s| s == "reddit" || s == "corpus-reddit")
        {
            return Ok(p);
        }
    }
    Err("select a cached corpus JSON".into())
}
fn action_ranker(term: &mut Terminal, corpus: &ng::NgramCorpus) -> AppResult<()> {
    let weights = load_weights(Path::new(WEIGHTS_FILE))?;
    let mut rows = Vec::new();
    for p in discover(LAYOUT_DIR, "dat", "")? {
        match ak::Layout::load(&p) {
            Ok(l) => match evaluate_tui(term, &l, corpus, &weights) {
                Ok(Some(v)) => rows.push((l, v)),
                Ok(None) => return Ok(()),
                Err(e) => eprintln!("{}: {e}", p.display()),
            },
            Err(e) => eprintln!("{}: {e}", p.display()),
        }
    }
    if rows.is_empty() {
        return Err("no decodable layouts".into());
    }
    let mut metric = 0usize;
    let mut ascending = true;
    let mut hidden_rows = BTreeSet::new();
    let mut hidden_cols = BTreeSet::new();
    let mut undo: Vec<(bool, usize)> = Vec::new();
    let mut first = 0usize;
    let mut top = 0usize;
    loop {
        let mut order: Vec<_> = (0..rows.len())
            .filter(|i| !hidden_rows.contains(i))
            .collect();
        order.sort_by(|&a, &b| {
            let o = rows[a].1.metrics.v[metric].total_cmp(&rows[b].1.metrics.v[metric]);
            if ascending {
                o
            } else {
                o.reverse()
            }
        });
        let visible: Vec<_> = (0..N_METRICS)
            .filter(|m| !hidden_cols.contains(m))
            .collect();
        let width = term.size.0.max(64);
        let name = 24usize;
        let cw = 10usize;
        let capacity = (width - name - 2) / cw;
        first = first.min(visible.len().saturating_sub(capacity));
        let cols: Vec<_> = visible.iter().skip(first).take(capacity).copied().collect();
        let height = term.size.1.saturating_sub(7).max(1);
        top = top.min(order.len().saturating_sub(height));
        let shown = order.len().saturating_sub(top).min(height);
        let mut c = Canvas::ranking(width, shown + 8);
        header(
            &mut c,
            "Action ranker",
            &corpus.name,
            "",
            "u restore | H all | q back",
        );
        let tablew = name + 2 + cols.len() * cw;
        c.boxed(
            Rect {
                x: 0,
                y: 3,
                w: tablew,
                h: shown + 3,
            },
            BORDER,
        );
        c.text(2, 4, "Layout", FG);
        for (j, &m) in cols.iter().enumerate() {
            let x = name + 1 + j * cw;
            c.center(
                x,
                4,
                cw,
                METRIC_NAMES[m],
                if m == metric { YELLOW } else { FG },
            );
            c.hit(
                Rect {
                    x,
                    y: 4,
                    w: cw,
                    h: 1,
                },
                Action::Metric(m),
            );
        }
        for (r, &i) in order.iter().skip(top).take(shown).enumerate() {
            let y = 5 + r;
            c.text(2, y, &short(&rows[i].0.name, name - 3), FG);
            c.hit(
                Rect {
                    x: 1,
                    y,
                    w: name - 1,
                    h: 1,
                },
                Action::Item(i),
            );
            for (j, &m) in cols.iter().enumerate() {
                let x = name + 1 + j * cw;
                let lo = order
                    .iter()
                    .map(|&i| rows[i].1.metrics.v[m])
                    .fold(f64::INFINITY, f64::min);
                let hi = order
                    .iter()
                    .map(|&i| rows[i].1.metrics.v[m])
                    .fold(f64::NEG_INFINITY, f64::max);
                let mut t = if hi > lo {
                    (rows[i].1.metrics.v[m] - lo) / (hi - lo)
                } else {
                    0.5
                };
                if higher_better(m) {
                    t = 1.0 - t;
                }
                c.right(
                    x,
                    y,
                    cw - 1,
                    &number(rows[i].1.metrics.v[m], term.decimals()),
                    if t < 0.33 {
                        GREEN
                    } else if t > 0.67 {
                        RED
                    } else {
                        YELLOW
                    },
                );
                c.hit(Rect { x, y, w: cw, h: 1 }, Action::Metric(m));
            }
        }
        c.h = shown + 7;
        term.present(&c, 0)?;
        let e = term.event()?;
        if let Event::Mouse {
            x,
            y,
            button: 1,
            release: false,
            motion: false,
        } = e
        {
            match term.hit(&c, x, y, 0) {
                Some(Action::Item(i)) => {
                    hidden_rows.insert(i);
                    undo.push((true, i));
                }
                Some(Action::Metric(m)) => {
                    hidden_cols.insert(m);
                    undo.push((false, m));
                }
                _ => {}
            }
            continue;
        }
        if let Some(a) = action_press(term, &c, &e, 0) {
            match a {
                Action::Metric(m) => {
                    if m == metric {
                        ascending = !ascending
                    } else {
                        metric = m;
                        ascending = !higher_better(m);
                    }
                }
                Action::Item(i) => action_editor(term, rows[i].0.clone(), corpus.clone(), false)?,
                _ => {}
            }
        }
        match e {
            Event::Quit | Event::Escape | Event::Char('q') => return Ok(()),
            Event::Left => first = first.saturating_sub(1),
            Event::Right => first += 1,
            Event::Wheel(d) => top = (top as i64 + d as i64).max(0) as usize,
            Event::Char('u') => {
                if let Some((row, i)) = undo.pop() {
                    if row {
                        hidden_rows.remove(&i);
                    } else {
                        hidden_cols.remove(&i);
                    }
                }
            }
            Event::Char('H') => {
                hidden_rows.clear();
                hidden_cols.clear();
                undo.clear();
            }
            Event::Char('.') => term.precise = !term.precise,
            _ => {}
        }
    }
}

pub fn dispatch(args: &[String]) -> Option<AppResult<()>> {
    if std::env::var_os("LAYOUTER_ACTION_PLAIN_CHILD").is_some() {
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
            _ => return None,
        }
    }
    Some((|| -> AppResult<()> {
        if command == "ranker" {
            let path = corpus_arg(args, 1)?;
            let mut term = Terminal::open()?;
            let Some(corpus) = load_corpus_tui(&mut term, &path)? else {
                return Ok(());
            };
            return action_ranker(&mut term, &corpus);
        }
        let path = if let Some(p) = args.get(1) {
            PathBuf::from(p)
        } else {
            let mut t = Terminal::open()?;
            match choose_path(&mut t, LAYOUT_DIR, "dat", "", "")? {
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
            let c = action_detail_frame(width, SFB, &l, "test", &rows, false);
            let text = canvas_text(&c);
            assert!(text.contains("@ n 39"));
            assert!(text.contains("Before"));
            assert!(c.h >= 46); // All rows, not a fixed top-24 snapshot.
        }
        let empty = action_detail_frame(80, SFB, &l, "test", &[], false);
        assert!(canvas_text(&empty).contains("No contributing physical sequences"));
    }
    #[test]
    fn action_detail_rows_and_shared_baseline_keep_exact_evaluation() {
        let l = history_layout();
        let stop = AtomicBool::new(false);
        let w = Weights::default();
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
        let geometry_value = evaluate(&changed, &c, &Weights([0.0; N_WEIGHTS]), &stop).unwrap();
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

    const HISTORY_LAYOUT:&str="f d l w v | q p o u ,\ns t h y g | z n a e i\nx k m c j | @ b ' ; .\nthumbs: r space\n\nw@ wh\nn@ n'\na@ aa\n";
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
    fn action_n_and_action_p_are_physical_sfb_in_both_evaluators() {
        let l = history_layout();
        let m = history_key(&l, "@");
        let a = history_key(&l, "a");
        let w = Weights::default();
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
            include_str!("../examples/magic-shorthand.dat"),
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
        let base = include_str!("../examples/magic-shorthand.dat");
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
                    let b = (state >> 32) as usize % layout.slots.len();
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
            include_str!("../examples/magic-shorthand.dat"),
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
            include_str!("../examples/magic-shorthand.dat"),
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
            include_str!("../examples/magic-shorthand.dat"),
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
        let source=format!("{}outer-left: @m @again ~\naction m = magic\nmap m \"i\" = \"'\"\nmap m \"qi\" = \"!\"\nmap m \"aqi\" = none\nmap m \"raqi\" = \"'\"\nfallback m = @alias\naction alias = magic\nfallback alias = repeat-output\naction again = repeat-action\n",crate::action_fast::tests::fixtures()[0]);
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
            let b = (rng >> 32) as usize % layout.slots.len();
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
