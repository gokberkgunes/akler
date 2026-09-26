const RANK_SCORE: usize = N_METRICS;

const RANK_COVERAGE: usize = N_METRICS + 1;

const RANK_COUNT: usize = N_METRICS + 2;

const RANK_Y: usize = 3;

const RANK_DATA: usize = 6;

const RANK_ORDER: [usize; RANK_COUNT] = [
    RANK_SCORE, SFB, SKB, SFS, SKS, TRAVEL, SFTRAVEL, FSB, HSB, FSS, HSS, LSB, LSS, REDIR, WRED, WISH, SRAF, ROLL,
    INROLL, OUTROLL, IN2, OUT2, IN3, OUT3,
    DFSB, CFSB, DFSS, CFSS, ALT, DSB, CSB, DSS, CSS, VTRAVEL, LTRAVEL, RANK_COVERAGE,
];

fn rank_name(m: usize) -> &'static str {
    if m == RANK_SCORE {
        "SCORE"
    } else if m == RANK_COVERAGE {
        "COVERAGE"
    } else {
        METRIC_NAMES[m]
    }
}

fn rank_positive(m: usize) -> bool {
    m == RANK_COVERAGE ||(m<N_METRICS && higher_better(m))
}

enum RankRow {
    Plain {
        model: Model,
        corpus: Corpus,
        raw: Raw,
        metrics: Metrics,
        score: f64,
    },
    Action {
        layout: action_keys::Layout,
        summary: action_summary::Summary,
        corpus: Arc<action_ngrams::NgramCorpus>,
        details: Option<Arc<action_ui::Evaluated>>,
    },
}

impl RankRow {
    fn name(&self) -> &str {
        match self {
            Self::Plain { model, .. } => &model.board.name,
            Self::Action { layout, .. } => &layout.name,
        }
    }

    fn path(&self) -> &Path {
        match self {
            Self::Plain { model, .. } => &model.board.path,
            Self::Action { layout, .. } => &layout.path,
        }
    }

    fn totals(&self) -> &[f64; 4] {
        match self {
            Self::Plain { corpus, .. } => &corpus.totals,
            Self::Action { summary, .. } => &summary.totals,
        }
    }

    fn raw(&self) -> &Raw {
        match self {
            Self::Plain { raw, .. } => raw,
            Self::Action { summary, .. } => &summary.raw,
        }
    }

    fn metrics(&self) -> &Metrics {
        match self {
            Self::Plain { metrics, .. } => metrics,
            Self::Action { summary, .. } => &summary.metrics,
        }
    }

    fn score(&self) -> f64 {
        match self {
            Self::Plain { score, .. } => *score,
            Self::Action { summary, .. } => summary.score,
        }
    }
}

fn rank_value(row: &RankRow, m: usize) -> Option<f64> {
    if m == RANK_SCORE {
        Some(row.score())
    } else if m == RANK_COVERAGE {
        // The action adapter's coverage is over decoded physical grams, not the
        // source text bigrams measured by ordinary coverage. Do not label it 100%.
        match row {
            RankRow::Plain { corpus, .. } => Some(corpus.coverage[1]),
            RankRow::Action { .. } => None,
        }
    } else if denominator_totals(m, row.raw(), row.totals()) > 0.0 {
        Some(row.metrics().v[m])
    } else {
        None
    }
}

fn plain_rank_row(board: Board, source: &Source, weights: &Weights) -> AppResult<RankRow> {
    let model = Model::with_rolls(board, weights.rolls());
    let corpus = model.corpus(source)?;
    let raw = full_raw(&model.original, &corpus, &model.geometry);
    let metrics = metrics(&raw, &corpus);
    let score = breakdown(&metrics, weights).net;

    Ok(RankRow::Plain {
        model,
        corpus,
        raw,
        metrics,
        score,
    })
}

enum RankLayout {
    Plain(Board),
    Action(action_keys::Layout),
}

fn parse_rank_layout(text: &str, path: &Path) -> AppResult<RankLayout> {
    match action_keys::Layout::parse(text, path) {
        Ok(layout) if layout.extended() => Ok(RankLayout::Action(layout)),
        Ok(_) => Ok(RankLayout::Plain(board_from_text(text, path)?)),
        Err(action_error) => match board_from_text(text, path) {
            Ok(board) => Ok(RankLayout::Plain(board)),
            Err(_) => Err(action_error.into()),
        },
    }
}

// Prepared once for a ranking load. An all-magic collection never prepares
// ordinary tables. A mixed collection shares one file read between its parsers.
struct RankCorpora {
    plain: Option<Result<Source, String>>,
    action: Option<action_keys::Result<Arc<action_ngrams::NgramCorpus>>>,
}

impl RankCorpora {
    fn prepare(
        layouts: &[(PathBuf, RankLayout)],
        text: &str,
        path: &Path,
        limits: NgramLimits,
        stop: &AtomicBool,
    ) -> Self {
        let plain = layouts.iter().any(|(_, layout)| matches!(layout, RankLayout::Plain(_)))
            .then(|| Source::from_text_with_limits(text, path, limits).map_err(|error| error.to_string()));
        let action = layouts.iter().any(|(_, layout)| matches!(layout, RankLayout::Action(_)))
            .then(|| action_ngrams::NgramCorpus::from_text_progress_with_limits(
                text, path, limits, stop, &AtomicU64::new(0),
            ).map(Arc::new));

        Self { plain, action }
    }

    fn warnings(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        if let Some(Ok(source)) = &self.plain {
            warnings.extend(source.warnings.iter().cloned());
        }
        if let Some(Ok(corpus)) = &self.action {
            for warning in &corpus.warnings {
                if !warnings.contains(warning) {
                    warnings.push(warning.clone());
                }
            }
        }
        warnings
    }
}

fn evaluate_rank_layout(
    layout: RankLayout,
    corpora: &RankCorpora,
    weights: &Weights,
    stop: &AtomicBool,
) -> AppResult<RankRow> {
    match layout {
        RankLayout::Plain(board) => {
            let source = corpora.plain.as_ref().ok_or("ordinary corpus was not prepared")?
                .as_ref().map_err(|error| error.clone())?;
            plain_rank_row(board, source, weights)
        }
        RankLayout::Action(layout) => {
            let corpus = corpora.action.as_ref().ok_or("action corpus was not prepared")?
                .as_ref().map_err(|error| error.clone())?;
            let summary = action_summary::evaluate(
                &layout, corpus, weights, stop, &AtomicU64::new(0),
            ).map_err(|error| format!("action evaluation: {error}"))?;
            Ok(RankRow::Action {
                layout,
                summary,
                corpus: Arc::clone(corpus),
                details: None,
            })
        }
    }
}

struct RankingData {
    path: PathBuf,
    name: String,
    rows: Vec<RankRow>,
    errors: Vec<String>,
    warnings: Vec<String>,
    columns: [bool; RANK_COUNT],
    weights: Weights,
}

fn load_ranking_progress(
    path: &Path,
    stop: &AtomicBool,
    progress: &AtomicU64,
) -> AppResult<RankingData> {
    let mut timing = load_profile::LoadProfile::new("ranker initialization");
    let config = load_app_config()?;
    let mut layouts = Vec::new();
    let mut errors = Vec::new();

    for layout_path in discover_layouts(Path::new(LAYOUT_DIR))? {
        if stop.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let parsed = fs::read_to_string(&layout_path)
            .map_err(|error| error.to_string())
            .and_then(|text| parse_rank_layout(&text, &layout_path).map_err(|error| error.to_string()));
        match parsed {
            Ok(layout) => layouts.push((layout_path, layout)),
            Err(error) => errors.push(format!("{}: {error}", layout_path.display())),
        }
    }
    if layouts.is_empty() {
        return Err(format!("no valid layouts\n{}", errors.join("\n")).into());
    }
    timing.mark("Read and parse layout files");

    let text = fs::read_to_string(path)?;
    timing.mark("Read corpus once");
    let corpora = RankCorpora::prepare(&layouts, &text, path, config.ngrams, stop);
    drop(text);
    let warnings = corpora.warnings();
    timing.mark("Prepare required corpus evaluators");

    let total = layouts.len();
    let mut rows = Vec::with_capacity(total);
    for (index, (layout_path, layout)) in layouts.into_iter().enumerate() {
        if stop.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        match evaluate_rank_layout(layout, &corpora, &config.weights, stop) {
            Ok(row) => rows.push(row),
            Err(error) => errors.push(format!("{}: {error}", layout_path.display())),
        }
        // This is overall layout progress, never a per-layout reset to zero.
        progress.store(((index + 1) * 100 / total) as u64, Ordering::Relaxed);
    }
    if stop.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    if rows.is_empty() {
        return Err(format!("no valid layouts\n{}", errors.join("\n")).into());
    }
    timing.mark("Evaluate layouts once each");

    Ok(RankingData {
        path: path.to_owned(),
        name: corpus_name(path),
        rows,
        errors,
        warnings,
        columns: config.rank_columns,
        weights: config.weights,
    })
}

fn load_ranking_tui(term: &mut Terminal, path: &Path) -> AppResult<Option<RankingData>> {
    let owned = prepare_corpus_path_tui(term, path)?;
    action_ui::ngram_job(
        term,
        "Ranking layouts (overall progress)",
        &corpus_name(path),
        "",
        move |stop, progress| {
            load_ranking_progress(&owned, stop, progress).map_err(|error| error.to_string())
        },
    )
}

// A cancelled or failed first load leaves the row uncached. The cache belongs
// to one ranking snapshot: reload/corpus changes replace the complete row.
fn rank_details_with<F>(
    cached: &mut Option<Arc<action_ui::Evaluated>>,
    prepare: F,
) -> AppResult<Option<Arc<action_ui::Evaluated>>>
where
    F: FnOnce() -> AppResult<Option<Arc<action_ui::Evaluated>>>,
{
    if cached.is_none() {
        *cached = prepare()?;
    }
    Ok(cached.clone())
}

fn inspect_row(term: &mut Terminal, row: &mut RankRow, weights: &Weights) -> AppResult<()> {
    if let RankRow::Action { layout, corpus, details, .. } = row {
        let evaluation = rank_details_with(details, || {
            let snapshot_layout = layout.clone();
            let snapshot_corpus = Arc::clone(corpus);
            let snapshot_weights = *weights;
            action_ui::ngram_job(
                term,
                "Preparing layout details",
                &corpus.name,
                &layout.name,
                move |stop, progress| {
                    action_ui::evaluate_progress(
                        &snapshot_layout, &snapshot_corpus, &snapshot_weights, stop, progress,
                    ).map(Arc::new)
                },
            )
        })?;
        if let Some(evaluation) = evaluation {
            action_ui::inspect_ranked(term, layout, &evaluation, weights)?;
        }
        return Ok(());
    }

    let RankRow::Plain { model, corpus, .. } = row else {
        unreachable!();
    };
    let mut scroll = 0;
    loop {
        let canvas = dashboard(
            term, "Layout", "v validate | q back", model, &model.original,
            &model.original, corpus, weights, None, None, "", None, false,
        );
        term.present(&canvas, scroll)?;
        let event = term.event()?;
        if scroll_event(&event, &mut scroll, canvas.h, term.size.1) {
            continue;
        }
        if let Some(Action::Metric(metric)) = action_press(term, &canvas, &event, scroll) {
            contributor_view(term, metric, model, &model.original, &model.original, corpus, false)?;
        }
        match event {
            Event::Char('.') => term.precise = !term.precise,
            Event::Escape | Event::Quit | Event::Char('q') => return Ok(()),
            Event::Char('i') => show_corpus_info(term, corpus)?,
            Event::Char('v') => validation_view(term, model, &model.original, &model.original)?,
            _ => {}
        }
    }
}

#[derive(Clone,Debug)]
enum Hidden {
    Column(usize),
    Row(PathBuf)
}

struct RankColumns {
    hidden: [bool; RANK_COUNT],
    rows: BTreeSet<PathBuf>,
    history: Vec<Hidden>,
    first: usize
}

impl RankColumns {
    fn new() -> Self {
        Self {
            hidden: [false; RANK_COUNT],
            rows: BTreeSet::new(),
            history: Vec::new(),
            first: 0
        }
    }

    fn visible(&self) -> Vec<usize> {
        RANK_ORDER.iter().copied().filter(|&m|!self.hidden[m]).collect()
    }

    fn hide(&mut self, m: usize) -> bool {
        if m >= RANK_COUNT || self.hidden[m] {
            return false;
        }
        if let Some(i) = self.visible().iter().position(|&id|id == m) {
            if i<self.first {
                self.first = self.first.saturating_sub(1);
            }
        }
        self.hidden[m] = true;
        self.history.push(Hidden::Column(m));
        true
    }

    fn hide_row(&mut self, path: PathBuf) -> bool {
        if !self.rows.insert(path.clone()) {
            return false;
        }
        self.history.push(Hidden::Row(path));
        true
    }

    fn undo(&mut self, capacity: usize) {
        if let Some(last) = self.history.pop() {
            match last {
                Hidden::Row(p) => {
                    self.rows.remove(&p);
                },
                Hidden::Column(m) => {
                    self.hidden[m] = false;
                    if let Some(i) = self.visible().iter().position(|&id|id == m) {
                        if i<self.first {
                            self.first = i;
                        } else if i >= self.first + capacity.max(1) {
                            self.first = i + 1 - capacity.max(1);
                        }
                    }
                }
            }
        }
    }

    fn restore(&mut self) {
        self.hidden.fill(false);
        self.rows.clear();
        self.history.clear();
        self.first = 0;
    }
}

const RANK_DEFAULT_COLUMNS: &[usize] = &[
    RANK_SCORE, SFB, SKB, SFS, SKS, TRAVEL, SFTRAVEL, FSB, HSB, FSS, HSS,
    LSB, LSS, REDIR, INROLL, OUTROLL, ALT, RANK_COVERAGE,
];

fn default_rank_columns() -> [bool; RANK_COUNT] {
    std::array::from_fn(|metric| !RANK_DEFAULT_COLUMNS.contains(&metric))
}

fn parse_rank_columns(text: &str) -> AppResult<[bool; RANK_COUNT]> {
    let mut hidden = [true; RANK_COUNT];
    for line in text.lines() {
        let content = line.split('#').next().unwrap_or("");
        for name in content.split_whitespace() {
            let metric = RANK_ORDER.iter().copied()
                .find(|&metric| rank_name(metric).eq_ignore_ascii_case(name))
                .ok_or_else(|| format!("unknown ranker column {name:?}"))?;
            hidden[metric] = false;
        }
    }
    if hidden.iter().all(|&value| value) {
        return Err("select at least one ranker column".into());
    }
    Ok(hidden)
}

fn rank_columns_text(hidden: &[bool; RANK_COUNT]) -> String {
    let names: Vec<_> = RANK_ORDER.iter().copied()
        .filter(|&metric| !hidden[metric])
        .map(rank_name)
        .collect();
    format!("# Visible ranker columns; edit here or press v in the ranker.\n{}\n", names.join(" "))
}

fn toggle_rank_column(hidden: &mut [bool; RANK_COUNT], metric: usize) {
    if hidden[metric] || hidden.iter().filter(|&&value| !value).count() > 1 {
        hidden[metric] = !hidden[metric];
    }
}

fn choose_rank_columns(
    term: &mut Terminal,
    current: &[bool; RANK_COUNT],
) -> AppResult<Option<[bool; RANK_COUNT]>> {
    let mut hidden = *current;
    let mut selected = 0;
    let mut scroll = 0;
    let mut status = String::new();
    loop {
        let mut canvas = Canvas::new(term.width(), RANK_COUNT + 6);
        header(
            &mut canvas, "Ranker columns", "", "",
            "Space toggle | d compact | a all | s save defaults | q cancel",
        );
        for (index, &metric) in RANK_ORDER.iter().enumerate() {
            canvas.text(
                1, index + 3,
                &format!("{} [{}] {}", if index == selected { '›' } else { ' ' },
                    if hidden[metric] { ' ' } else { 'x' }, rank_name(metric)),
                if index == selected { YELLOW } else { FG },
            );
            canvas.hit(Rect { x: 1, y: index + 3, w: 24, h: 1 }, Action::Item(index));
        }
        canvas.text(1, RANK_COUNT + 4, &short(&status, canvas.w.saturating_sub(2)), CYAN);
        canvas.h = RANK_COUNT + 6;
        term.present(&canvas, scroll)?;
        let event = term.event()?;
        if scroll_event(&event, &mut scroll, canvas.h, term.size.1) {
            continue;
        }
        if let Some(Action::Item(index)) = action_press(term, &canvas, &event, scroll) {
            selected = index;
            toggle_rank_column(&mut hidden, RANK_ORDER[selected]);
        }
        match event {
            Event::Escape | Event::Quit | Event::Char('q') => return Ok(None),
            Event::Up | Event::Char('k') => selected = selected.saturating_sub(1),
            Event::Down | Event::Char('j') => selected = (selected + 1).min(RANK_COUNT - 1),
            Event::Char(' ') | Event::Enter => toggle_rank_column(&mut hidden, RANK_ORDER[selected]),
            Event::Char('d') => hidden = default_rank_columns(),
            Event::Char('a') => hidden.fill(false),
            Event::Char('s') => {
                if hidden.iter().all(|&value| value) {
                    status = "Select at least one column".into();
                    continue;
                }
                match save_rank_columns(&hidden) {
                    Ok(()) => return Ok(Some(hidden)),
                    Err(error) => status = error.to_string(),
                }
            }
            _ => {}
        }
        let y = selected + 3;
        if y < scroll {
            scroll = y;
        } else if y >= scroll + term.size.1.max(1) {
            scroll = y + 1 - term.size.1.max(1);
        }
    }
}

struct RankGrid {
    width: usize,
    name: usize,
    cell: usize,
    metrics: Vec<usize>,
    first: usize,
    total: usize,
    capacity: usize
}

impl RankGrid {
    fn new(width: usize, name: usize, cell: usize, columns: &mut RankColumns) -> Self {
        let name = name.min(width.saturating_sub(cell + 3));
        let capacity = (width.saturating_sub(name + 2) /(cell + 1)).max(1);
        let visible = columns.visible();
        columns.first = columns.first.min(visible.len().saturating_sub(capacity));
        let metrics: Vec<_> = visible.iter().skip(columns.first).take(capacity).copied().collect();
        Self {
            width: name + 2 + metrics.len()*(cell + 1),
            name,
            cell,
            metrics,
            first: columns.first,
            total: visible.len(),
            capacity
        }
    }

    fn content_x(&self, col: usize) -> usize {
        2 + self.name + col*(self.cell + 1)
    }

    fn metric_at(&self, x: usize, y: usize, count: usize) -> Option<usize> {
        if y != RANK_Y + 1&&!(RANK_DATA..RANK_DATA + count).contains(&y) {
            return None;
        }
        self.metrics.iter().enumerate().find_map(|(j, &m)| {
            let xx = self.content_x(j);
            (x >= xx && x<xx + self.cell).then_some(m)
        })
    }

    fn layout_at(&self, x: usize, y: usize, count: usize) -> Option<usize> {
        if x >= 1 && x<1 + self.name &&(RANK_DATA..RANK_DATA + count).contains(&y) {
            Some(y - RANK_DATA)
        } else {
            None
        }
    }
}

#[derive(Clone,Copy)]
struct RankRange {
    lo: f64,
    hi: f64
}

fn rank_ranges(rows: &[RankRow], hidden: &BTreeSet<PathBuf>) -> [RankRange; RANK_COUNT] {
    let mut out = [RankRange {
        lo: f64::INFINITY,
        hi: f64::NEG_INFINITY
    }; RANK_COUNT];
    for row in rows {
        if hidden.contains(row.path()) {
            continue;
        }
        for (m, range) in out.iter_mut().enumerate() {
            if let Some(v) = rank_value(row, m) {
                if v.is_finite() {
                    range.lo = range.lo.min(v);
                    range.hi = range.hi.max(v);
                }
            }
        }
    }
    out
}

fn rank_color(m: usize, v: f64, range: RankRange) -> u8 {
    if !v.is_finite()||!range.lo.is_finite()||!range.hi.is_finite() {
        return MUTED;
    }
    let span = range.hi - range.lo;
    if span.abs() <= 1e-12*(1.0 + range.lo.abs().max(range.hi.abs())) {
        return gradient(0.5);
    }
    let t = ((v - range.lo) / span).clamp(0.0, 1.0);
    gradient(if rank_positive(m) {
        t
    } else {
        1.0 - t
    })
}

fn rank_order(rows: &[RankRow], filter: &str, metric: Option<usize>, ascending: bool, hidden: &BTreeSet<PathBuf>) -> Vec<usize> {
    let filter = filter.to_ascii_lowercase();
    let mut order: Vec<_> =(0..rows.len()).filter(|&i|!hidden.contains(rows[i].path()) && rows[i].name().to_ascii_lowercase().contains(&filter)).collect();
    order.sort_by(|&a, &b| {
        if let Some(m) = metric {
            let va = rank_value(&rows[a], m);
            let vb = rank_value(&rows[b], m);
            if va.is_none() != vb.is_none() {
                return va.is_none().cmp(&vb.is_none());
            }
        }
        let cmp = if let Some(m) = metric {
            rank_value(&rows[a], m).unwrap_or(0.0).total_cmp(&rank_value(&rows[b], m).unwrap_or(0.0))
        } else {
            rows[a].name().cmp(rows[b].name())
        };
        (if ascending {
            cmp
        } else {
            cmp.reverse()
        }).then_with(|| rows[a].name().cmp(rows[b].name()))
    });
    order
}

fn rank_viewport(top: &mut usize, selected: &mut usize, total: usize, capacity: usize, follow: bool) {
    let capacity = capacity.max(1);
    *selected = (*selected).min(total.saturating_sub(1));
    *top = (*top).min(total.saturating_sub(capacity));
    if follow {
        if *selected<*top {
            *top=*selected;
        } else if *selected >= top.saturating_add(capacity) {
            *top=*selected + 1 - capacity;
        }
    }
    else if total>0 {
        *selected = (*selected).clamp(*top,(top.saturating_add(capacity) - 1).min(total - 1));
    }
}

fn short_end(text: &str, width: usize) -> String {
    let cs: Vec<_> = clean_text(text).chars().collect();
    if cs.len() <= width {
        cs.into_iter().collect()
    } else if width <= 1 {
        "…".into()
    } else {
        format!("…{}", cs[cs.len() - width + 1..].iter().collect:: <String>())
    }
}

fn draw_rank_table(c: &mut Canvas, g: &RankGrid, rows: &[RankRow], order: &[usize], top: usize, selected: usize, shown: usize, metric: Option<usize>, ascending: bool, dp: usize, ranges: &[RankRange; RANK_COUNT]) -> usize {
    let bottom = RANK_DATA + shown.max(1);
    c.boxed(Rect {
        x: 0,
        y: RANK_Y,
        w: g.width,
        h: bottom - RANK_Y + 1
    }, BORDER);
    c.line(0, RANK_Y + 2, g.width, BORDER);
    c.put(0, RANK_Y + 2, '├', BORDER);
    c.put(g.width - 1, RANK_Y + 2, '┤', BORDER);
    let arrow = if ascending {
        "↑"
    } else {
        "↓"
    };
    c.text(2, RANK_Y + 1, &format!("Layout{}", if metric.is_none() {
        arrow
    } else {
        ""
    }), if metric.is_none() {
        YELLOW
    } else {
        FG
    });
    c.hit(Rect {
        x: 1,
        y: RANK_Y + 1,
        w: g.name,
        h: 1
    }, Action::Command('n'));
    for (j, &m) in g.metrics.iter().enumerate() {
        let x = g.content_x(j);
        c.put(x - 1, RANK_Y, '┬', BORDER);
        c.put(x - 1, bottom, '┴', BORDER);
        for y in RANK_Y + 1..bottom {
            c.put(x - 1, y, '│', BORDER);
        }
        c.put(x - 1, RANK_Y + 2, '┼', BORDER);
        c.center(x, RANK_Y + 1, g.cell, &format!("{}{}", rank_name(m), if metric == Some(m) {
            arrow
        } else {
            ""
        }), if metric == Some(m) {
            YELLOW
        } else {
            FG
        });
        c.hit(Rect {
            x,
            y: RANK_Y + 1,
            w: g.cell,
            h: 1
        }, Action::Metric(m));
    }
    let digits = order.len().to_string().len();
    for (offset, &i) in order.iter().skip(top).take(shown).enumerate() {
        let r = top + offset;
        let y = RANK_DATA + offset;
        let row=&rows[i];
        let prefix = format!("{}{:>digits$}. ", if r == selected {
            '›'
        } else {
            ' '
        }, r + 1);
        let name = short_end(row.name(), g.name.saturating_sub(prefix.chars().count()));
        c.text(1, y, &format!("{prefix}{name}"), FG);
        c.hit(Rect {
            x: 1,
            y,
            w: g.width - 2,
            h: 1
        }, Action::Item(r));
        for (j, &m) in g.metrics.iter().enumerate() {
            let value = rank_value(row, m);
            let text = match value {
                Some(v) => {
                    let mut s = number(v, dp);
                    if m == RANK_COVERAGE || m<N_METRICS&&!physical_metric(m) {
                        s.push('%');
                    }
                    s
                },
                None => "n/a".into()
            };
            c.right(g.content_x(j) + 1, y, g.cell - 2, &text, value.map(|v|rank_color(m, v, ranges[m])).unwrap_or(MUTED));
        }
    }
    if order.is_empty() {
        c.text(2, RANK_DATA, "—", MUTED);
    }
    bottom
}

fn ranking(term: &mut Terminal, path: &Path) -> AppResult<()> {
    let Some(mut data) = load_ranking_tui(term, path)? else {
        return Ok(());
    };
    let mut metric = Some(RANK_SCORE);
    let mut ascending = true;
    let mut columns = RankColumns::new();
    columns.hidden = data.columns;
    let (mut selected, mut top) = (0, 0);
    let mut filter = String::new();
    let mut status = String::new();

    loop {
        let rows = &data.rows;
        let errors = &data.errors;
        let order = rank_order(rows, &filter, metric, ascending, &columns.rows);
        let ranges = rank_ranges(rows, &columns.rows);
        let width = term.size.0.max(64);
        let height = term.size.1.max(12);
        let capacity = height.saturating_sub(RANK_DATA + 3).max(1);
        rank_viewport(&mut top, &mut selected, order.len(), capacity, false);
        let shown = order.len().saturating_sub(top).min(capacity);
        let name_width = rows.iter().map(|r|r.name().chars().count()).max().unwrap_or(12).saturating_add(order.len().to_string().len() + 3).clamp(18, 30);
        let mut cw = 10usize;
        for row in rows {
            for m in 0..RANK_COUNT {
                if let Some(v) = rank_value(row, m) {
                    cw = cw.max(number(v, term.decimals()).chars().count() + 3);
                }
            }
        }
        let grid = RankGrid::new(width, name_width, cw, &mut columns);
        let mut c = Canvas::ranking(width, height);
        header(&mut c, "Ranker", &data.name, "", "v columns | c corpus | r reload | u restore | H all | q back");
        let bottom = draw_rank_table(&mut c, &grid, rows, &order, top, selected, shown, metric, ascending, term.decimals(), &ranges);
        let mut footer = format!("{} layouts · travel u/100", order.len());
        if !errors.is_empty() {
            footer.push_str(&format!(" · {} failed (e errors)", errors.len()));
        }
        if data.warnings.iter().any(|warning| warning.starts_with("Approximate")) {
            footer.push_str(" · limited n-grams (i info)");
        }
        let hidden = columns.hidden.iter().filter(|&&x|x).count();
        if hidden>0 {
            footer.push_str(&format!(" · {hidden} columns hidden"));
        }
        if !columns.rows.is_empty() {
            footer.push_str(&format!(" · {} rows hidden", columns.rows.len()));
        }
        if grid.total>grid.metrics.len() {
            footer.push_str(&format!(" · columns {}–{}/{}", grid.first + 1, grid.first + grid.metrics.len(), grid.total));
        }
        c.text(0, bottom + 1, &short(&footer, width), MUTED);
        let selected_name = order.get(selected).map(|&i|rows[i].name()).unwrap_or("");
        c.text(0, bottom + 2, &short(if status.is_empty() {
            selected_name
        } else {
            &status
        }, width), CYAN);
        c.h = bottom + 3;
        term.present(&c, 0)?;
        let mut e = term.event()?;
        if let Event::Mouse {
            x,
            y,
            button: 1,
            release: false,
            motion: false
        } = e {
            if let Some(offset) = grid.layout_at(x, y, shown) {
                if let Some(&i) = order.get(top + offset) {
                    columns.hide_row(rows[i].path().to_owned());
                }
                status.clear();
                continue;
            }
            if let Some(m) = grid.metric_at(x, y, shown) {
                columns.hide(m);
                status.clear();
                continue;
            }
        }
        let delta = match e {
            Event::Wheel(n) => Some(n),
            Event::PageUp => Some(-(capacity as i32)),
            Event::PageDown => Some(capacity as i32),
            _ => None
        };
        if let Some(d) = delta {
            top = (top as i64 + d as i64).max(0) as usize;
            rank_viewport(&mut top, &mut selected, order.len(), capacity, false);
            continue;
        }
        if let Some(a) = action_press(term, &c, &e, 0) {
            match a {
                Action::Metric(m) => {
                    if metric == Some(m) {
                        ascending=!ascending;
                    } else {
                        metric = Some(m);
                        ascending=!rank_positive(m);
                    }
                    top = 0;
                    selected = 0;
                    continue;
                },
                Action::Item(r) => {
                    if selected == r {
                        if let Some(&i) = order.get(r) {
                            inspect_row(term, &mut data.rows[i], &data.weights)?;
                        }
                    }
                    selected = r;
                    continue;
                },
                Action::Command(ch) => e = Event::Char(ch),
                _ => {
                }
            }
        }
        match e {
            Event::Escape|Event::Quit|Event::Char('q') => return Ok(()),
            Event::Enter => if let Some(&i) = order.get(selected) {
                inspect_row(term, &mut data.rows[i], &data.weights)?;
            },
            Event::Up|Event::Char('k') => {
                selected = selected.saturating_sub(1);
                rank_viewport(&mut top, &mut selected, order.len(), capacity, true);
            },
            Event::Down|Event::Char('j') => {
                selected = (selected + 1).min(order.len().saturating_sub(1));
                rank_viewport(&mut top, &mut selected, order.len(), capacity, true);
            },
            Event::Left|Event::Char('h') => columns.first = columns.first.saturating_sub(1),
            Event::Right|Event::Char('l') => columns.first = columns.first.saturating_add(1),
            Event::Tab => columns.first = columns.first.saturating_add(grid.capacity),
            Event::Home => columns.first = 0,
            Event::End => columns.first = grid.total,
            Event::Char('u') => columns.undo(grid.capacity),
            Event::Char('H') => columns.restore(),
            Event::Char('v') => {
                if let Some(hidden) = choose_rank_columns(term, &columns.hidden)? {
                    columns.hidden = hidden;
                    columns.first = 0;
                    columns.history.retain(|item| matches!(item, Hidden::Row(_)));
                    status = format!("Saved default columns to {APP_CONFIG_FILE}");
                }
            }
            Event::Char('.') => term.precise=!term.precise,
            Event::Char('n') => {
                if metric.is_none() {
                    ascending=!ascending;
                } else {
                    metric = None;
                    ascending = true;
                }
                top = 0;
                selected = 0;
            },
            Event::Char('/') => if let Some(q) = input_box(term, "", "", &filter)? {
                filter = q;
                top = 0;
                selected = 0;
            },
            Event::Char('e') => info_page(term, "Errors", &errors)?,
            Event::Char('i') => info_page(term, "Corpus / n-gram limits", &data.warnings)?,
            Event::Char('c') => if let Some(path) = select_source_path(term, false)? {
                match load_ranking_tui(term, &path) {
                    Ok(Some(next)) => {
                        data = next;
                        columns.hidden = data.columns;
                        columns.first = 0;
                        columns.history.retain(|item| matches!(item, Hidden::Row(_)));
                        status.clear();
                        top = 0;
                        selected = 0;
                    }
                    Ok(None) => {}
                    Err(error) => status = error.to_string(),
                }
            },
            Event::Char('r') => {
                let path = corpus_by_name(&data.name).unwrap_or_else(|_| data.path.clone());
                match load_ranking_tui(term, &path) {
                    Ok(Some(next)) => {
                        data = next;
                        columns.hidden = data.columns;
                        columns.first = 0;
                        columns.history.retain(|item| matches!(item, Hidden::Row(_)));
                        status.clear();
                    }
                    Ok(None) => {}
                    Err(error) => status = error.to_string(),
                }
            },
            Event::Char('?') => info_page(term, "Ranker", &[
                    "Left-click header sorts. Middle-click a metric hides its column; middle-click a layout name hides its row.".into(),
                    "u restores the last hidden item; H restores all for this session. Left/right scroll columns; wheel scrolls layouts.".into(),
                    "v chooses columns; Space toggles; s saves defaults to akler.conf. d chooses compact defaults; a selects all.".into(),
                    "Action layouts use their bounded n-gram evaluator. Their COVERAGE is n/a; open a row for physical presses and ignored text.".into(),
                    "Colors use min/max of every non-hidden layout; green is preferable. Different key sets still require coverage checks.".into(),
                    "SCORE uses saved detailed weights on this corpus. Candidate comparison uses its actual training objective.".into(),
                    "Enter opens the selected layout; . changes precision; / filters names; e shows errors; i shows corpus/limit notes.".into()
                ])?,
            _ => {
            }
        }
    }
}

fn candidate_view(term: &mut Terminal, p: &Problem, candidates: &[Candidate], selected: usize) -> AppResult<usize> {
    if candidates.is_empty() {
        return Ok(selected);
    }
    let mut rows = Vec::new();
    for (i, item) in candidates.iter().enumerate() {
        let mut model = p.model.clone();
        model.original = item.arr.clone();
        model.board.name = format!("Candidate {:03}", i + 1);
        model.board.path = PathBuf::from(format!("candidate-{i}"));
        let corpus = p.corpora[0].clone();
        let raw = full_raw(&item.arr, &corpus, &model.geometry);
        let metrics = metrics(&raw, &corpus);
        rows.push(RankRow::Plain {
            model,
            corpus,
            raw,
            metrics,
            score: item.score
        });
    }
    let mut columns = RankColumns::new();
    let wanted = [RANK_SCORE, SFB, SFS, TRAVEL, SFTRAVEL, LSB, DSB, CSB, SRAF, ROLL];
    for m in 0..RANK_COUNT {
        columns.hidden[m]=!wanted.contains(&m);
    }
    let mut chosen = selected.min(rows.len() - 1);
    let (mut top, mut current) = (0, chosen);
    let mut metric = Some(RANK_SCORE);
    let mut ascending = true;
    loop {
        let order = rank_order(&rows, "", metric, ascending, &columns.rows);
        let ranges = rank_ranges(&rows, &columns.rows);
        let capacity = term.size.1.saturating_sub(RANK_DATA + 3).max(1);
        rank_viewport(&mut top, &mut current, order.len(), capacity, true);
        let shown = order.len().saturating_sub(top).min(capacity);
        let g = RankGrid::new(term.size.0.max(64), 18, 10, &mut columns);
        let mut c = Canvas::ranking(term.size.0.max(64), term.size.1.max(12));
        header(&mut c, "Candidates", &p.corpora[0].name, &p.model.board.name, "q back");
        let bottom = draw_rank_table(&mut c, &g, &rows, &order, top, current, shown, metric, ascending, term.decimals(), &ranges);
        if let Some(&i) = order.get(current) {
            let nearest = candidates.iter().enumerate().filter(|(j, _)|*j != i).map(|(_, other)|letter_distance(&p.model, &other.arr, &candidates[i].arr)).min();
            if let Some(n) = nearest {
                c.text(0, bottom + 1, &format!("Nearest candidate: {n} moved letters"), MUTED);
            }
        }
        c.h = bottom + 2;
        term.present(&c, 0)?;
        let e = term.event()?;
        if let Some(a) = action_press(term, &c, &e, 0) {
            match a {
                Action::Item(r) => {
                    if let Some(&i) = order.get(r) {
                        return Ok(i);
                    }
                },
                Action::Metric(m) => {
                    if metric == Some(m) {
                        ascending=!ascending;
                    } else {
                        metric = Some(m);
                        ascending=!rank_positive(m);
                    }
                    current = 0;
                    top = 0;
                },
                _ => {
                }
            }
        }
        match e {
            Event::Enter => {
                if let Some(&i) = order.get(current) {
                    chosen = i;
                }
                return Ok(chosen);
            },
            Event::Char('q')|Event::Escape|Event::Quit => return Ok(chosen),
            Event::Up => current = current.saturating_sub(1),
            Event::Down => current = (current + 1).min(order.len().saturating_sub(1)),
            Event::Left => columns.first = columns.first.saturating_sub(1),
            Event::Right => columns.first = columns.first.saturating_add(1),
            Event::Char('.') => term.precise=!term.precise,
            _ => {
            }
        }
    }
}

#[cfg(test)]
mod ranker_tests {
    use super::*;

    const NORMAL_LAYOUT: &str = "q w e r t  y u i o p\na s d f g  h j k l ;\nz x c v b  n m , . /\nthumbs: space\n";

    const ACTION_LAYOUT: &str = "@ w e r t  y u i o p\na s d f g  h j k l ;\nz x c v b  n m , . /\nthumbs: space space\n@ aa\n";

    const CORPUS: &str = r#"{"monograms":{"a":3,"b":2},"bigrams":{"aa":1,"ab":1,"bb":1,"ba":1},"trigrams":{"aab":1,"abb":1,"bba":1}}"#;

    #[test]
    fn ranker_defaults_and_saved_column_round_trip() {
        let hidden = default_rank_columns();
        for metric in [SFB, SKB, SFS, SKS] {
            assert!(!hidden[metric]);
        }
        assert!(!hidden[FSB]);
        assert!(!hidden[FSS]);
        for metric in [DFSB, CFSB, DFSS, CFSS, DSB, CSB, DSS, CSS] {
            assert!(hidden[metric]);
        }
        assert_eq!(parse_rank_columns(&rank_columns_text(&hidden)).unwrap(), hidden);

        let all = [false; RANK_COUNT];
        assert_eq!(parse_rank_columns(&rank_columns_text(&all)).unwrap(), all);

        let selected = parse_rank_columns("score sfb\n# note\nSFS COVERAGE").unwrap();
        assert!(!selected[RANK_SCORE]);
        assert!(!selected[SFB]);
        assert!(selected[FSB]);
        assert!(parse_rank_columns("SCORE TYPO").is_err());
        assert!(parse_rank_columns("# empty").is_err());

        let mut one = parse_rank_columns("SCORE").unwrap();
        toggle_rank_column(&mut one, RANK_SCORE);
        assert!(!one[RANK_SCORE]);
        toggle_rank_column(&mut one, SFB);
        toggle_rank_column(&mut one, RANK_SCORE);
        assert!(one[RANK_SCORE]);
        assert!(!one[SFB]);
    }

    #[test]
    fn mixed_ranker_preserves_both_evaluators_exactly() {
        let path = Path::new("inline.json");
        let source = Source::from_text(CORPUS, path).unwrap();
        let corpus = action_ngrams::NgramCorpus::from_text(CORPUS, path).unwrap();
        let weights = Weights::default();
        let stop = AtomicBool::new(false);
        let progress = AtomicU64::new(0);
        let prepared = RankCorpora {
            plain: Some(Ok(source.clone())),
            action: Some(Ok(Arc::new(corpus.clone()))),
        };

        let plain = evaluate_rank_layout(
            parse_rank_layout(NORMAL_LAYOUT, Path::new("normal.dat")).unwrap(),
            &prepared, &weights, &stop,
        ).unwrap();
        let model = Model::new(board_from_text(NORMAL_LAYOUT, Path::new("normal.dat")).unwrap());
        let ordinary_corpus = model.corpus(&source).unwrap();
        let ordinary_raw = full_raw(&model.original, &ordinary_corpus, &model.geometry);
        let ordinary_metrics = metrics(&ordinary_raw, &ordinary_corpus);
        assert_eq!(plain.raw().0.map(f64::to_bits), ordinary_raw.0.map(f64::to_bits));
        assert_eq!(plain.metrics().v.map(f64::to_bits), ordinary_metrics.v.map(f64::to_bits));
        assert_eq!(plain.score().to_bits(), breakdown(&ordinary_metrics, &weights).net.to_bits());
        assert_eq!(rank_value(&plain, RANK_COVERAGE), Some(ordinary_corpus.coverage[1]));

        let action = evaluate_rank_layout(
            parse_rank_layout(ACTION_LAYOUT, Path::new("magic.dat")).unwrap(),
            &prepared, &weights, &stop,
        ).unwrap();
        let layout = action_keys::Layout::parse(ACTION_LAYOUT, Path::new("magic.dat")).unwrap();
        let expected = action_ui::evaluate_progress(&layout, &corpus, &weights, &stop, &progress).unwrap();
        assert_eq!(action.raw().0.map(f64::to_bits), expected.raw.0.map(f64::to_bits));
        assert_eq!(action.metrics().v.map(f64::to_bits), expected.metrics.v.map(f64::to_bits));
        assert_eq!(action.score().to_bits(), expected.score.to_bits());
        assert_eq!(rank_value(&action, RANK_COVERAGE), None);
        assert!(matches!(&action, RankRow::Action { layout, .. }
            if layout.slots.iter().any(|slot| slot.label == "@")));

        let rows = vec![plain, action];
        let hidden = BTreeSet::new();
        let order = rank_order(&rows, "", Some(RANK_SCORE), true, &hidden);
        assert_eq!(order.len(), 2);
        assert_eq!(rank_order(&rows, "magic", None, true, &hidden), vec![1]);
        assert_eq!(rank_order(&rows, "", Some(RANK_COVERAGE), false, &hidden), vec![0, 1]);
    }

    #[test]
    fn ranker_prepares_only_required_corpus_engines() {
        let path = Path::new("inline.json");
        let stop = AtomicBool::new(false);
        let normal = || (PathBuf::from("normal.dat"), parse_rank_layout(NORMAL_LAYOUT, Path::new("normal.dat")).unwrap());
        let magic = || (PathBuf::from("magic.dat"), parse_rank_layout(ACTION_LAYOUT, Path::new("magic.dat")).unwrap());

        let action_only = RankCorpora::prepare(&[magic()], CORPUS, path, NgramLimits::default(), &stop);
        assert!(action_only.plain.is_none());
        assert!(action_only.action.as_ref().unwrap().is_ok());

        let ordinary = RankCorpora::prepare(&[normal()], CORPUS, path, NgramLimits::default(), &stop);
        assert!(ordinary.plain.as_ref().unwrap().is_ok());
        assert!(ordinary.action.is_none());

        let mixed = RankCorpora::prepare(&[normal(), magic()], CORPUS, path, NgramLimits::default(), &stop);
        assert!(mixed.plain.as_ref().unwrap().is_ok());
        assert!(mixed.action.as_ref().unwrap().is_ok());

        let limited = RankCorpora::prepare(
            &[normal(), magic()], CORPUS, path,
            NgramLimits { trigrams: Some(1), ..NgramLimits::default() }, &stop,
        );
        assert_eq!(limited.plain.as_ref().unwrap().as_ref().unwrap().tables[3].len(), 1);
        assert!(!limited.warnings().is_empty());
        assert_eq!(mixed.plain.as_ref().unwrap().as_ref().unwrap().tables[3].len(), 3);
    }

    #[test]
    fn ranker_does_not_hide_action_or_corpus_errors() {
        let weights = Weights::default();
        let stop = AtomicBool::new(false);
        let prepared = RankCorpora {
            plain: None,
            action: Some(Err("missing cached contexts".into())),
        };

        let missing = evaluate_rank_layout(
            parse_rank_layout(ACTION_LAYOUT, Path::new("magic.dat")).unwrap(),
            &prepared, &weights, &stop,
        );
        assert!(missing.err().unwrap().to_string().contains("missing cached contexts"));

        let invalid = format!("{ACTION_LAYOUT}swap y ,/u\n");
        assert!(parse_rank_layout(&invalid, Path::new("invalid.dat")).is_err());
    }

    #[test]
    fn action_details_are_lazy_reused_and_retryable_after_cancellation_or_error() {
        let stop = AtomicBool::new(false);
        let weights = Weights::default();
        let prepared = RankCorpora::prepare(
            &[(PathBuf::from("magic.dat"), parse_rank_layout(ACTION_LAYOUT, Path::new("magic.dat")).unwrap())],
            CORPUS,
            Path::new("inline.json"),
            NgramLimits::default(),
            &stop,
        );
        let mut row = evaluate_rank_layout(
            parse_rank_layout(ACTION_LAYOUT, Path::new("magic.dat")).unwrap(),
            &prepared, &weights, &stop,
        ).unwrap();
        let RankRow::Action { layout, corpus, details, .. } = &mut row else {
            panic!("expected an action row");
        };
        assert!(details.is_none());
        assert!(Arc::ptr_eq(corpus, prepared.action.as_ref().unwrap().as_ref().unwrap()));

        assert!(rank_details_with(details, || Ok(None)).unwrap().is_none());
        let error = rank_details_with(details, || Err("interrupted detail load".into())).err().unwrap();
        assert_eq!(error.to_string(), "interrupted detail load");
        assert!(details.is_none());

        let first = rank_details_with(details, || {
            action_ui::evaluate_progress(layout, corpus, &weights, &stop, &AtomicU64::new(0))
                .map(|value| Some(Arc::new(value)))
                .map_err(|error| error.into())
        }).unwrap().unwrap();
        let reopened = rank_details_with(details, || panic!("cached details must not be reevaluated"))
            .unwrap().unwrap();
        assert!(Arc::ptr_eq(&first, &reopened));
    }

    #[test]
    fn action_ranking_reload_creates_independent_corpus_layout_and_limit_snapshots() {
        let stop = AtomicBool::new(false);
        let weights = Weights::default();
        let make_row = |source: &str, text: &str, limits: NgramLimits, weights: &Weights| {
            let layout = parse_rank_layout(source, Path::new("magic.dat")).unwrap();
            let prepared = RankCorpora::prepare(
                &[(PathBuf::from("magic.dat"), layout)], text,
                Path::new("inline.json"), limits, &stop,
            );
            evaluate_rank_layout(
                parse_rank_layout(source, Path::new("magic.dat")).unwrap(),
                &prepared, weights, &stop,
            ).unwrap()
        };
        let original = make_row(ACTION_LAYOUT, CORPUS, NgramLimits::default(), &weights);
        let original_score = original.score().to_bits();
        let mut changed_weights = weights;
        changed_weights.0[SFB] += 7.0;
        let changed_rule = ACTION_LAYOUT.replace("@ aa", "@ ab");
        let staggered = format!("{ACTION_LAYOUT}row-stagger: anglemod\n");
        let changed_corpus = CORPUS.replace(":1", ":0.5");
        let replacements = [
            make_row(ACTION_LAYOUT, CORPUS, NgramLimits::default(), &weights),
            make_row(&changed_rule, CORPUS, NgramLimits::default(), &weights),
            make_row(&staggered, CORPUS, NgramLimits::default(), &weights),
            make_row(ACTION_LAYOUT, &changed_corpus, NgramLimits::default(), &weights),
            make_row(ACTION_LAYOUT, CORPUS, NgramLimits { trigrams: Some(1), ..NgramLimits::default() }, &weights),
            make_row(ACTION_LAYOUT, CORPUS, NgramLimits::default(), &changed_weights),
        ];
        let RankRow::Action { corpus: old_corpus, details: old_details, .. } = &original else {
            panic!("expected an action row");
        };
        for replacement in &replacements {
            let RankRow::Action { corpus, details, .. } = replacement else {
                panic!("expected an action row");
            };
            assert!(!Arc::ptr_eq(old_corpus, corpus));
            assert!(details.is_none());
        }
        assert!(old_details.is_none());
        assert_eq!(original.score().to_bits(), original_score);
        assert_eq!(replacements[0].score().to_bits(), original_score);
    }
}
