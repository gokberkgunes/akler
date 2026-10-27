const RANK_SCORE: usize = N_METRICS;

const RANK_COVERAGE: usize = N_METRICS + 1;

const RANK_FJB: usize = N_METRICS + 2;

const RANK_FJS: usize = N_METRICS + 3;

const BASE_RANK_COUNT: usize = N_METRICS + 4;

const RANK_MANA_START: usize = BASE_RANK_COUNT;

const RANK_COUNT: usize = BASE_RANK_COUNT + mana2_metrics::N_STATS;

const RANK_Y: usize = 4;

const RANK_DATA: usize = 7;

// The ordinary ranker only exposes AKLER's own metrics. Mana2 has a separate
// catalog and entry point below so its statistics cannot leak into the normal
// column picker or scoring mode.
const RANK_ORDER: [usize; BASE_RANK_COUNT - 8] = [
    RANK_SCORE,
    SFB,
    SKB,
    SFS,
    SKS,
    TRAVEL,
    SFTRAVEL,
    FSB,
    HSB,
    FSS,
    HSS,
    RANK_FJB,
    RANK_FJS,
    LSB,
    LSS,
    REDIR,
    WRED,
    WISH,
    SRAF,
    INSRAF,
    OUTSRAF,
    ROLL,
    INROLL,
    OUTROLL,
    IN2,
    OUT2,
    IN3,
    OUT3,
    DAHJB,
    CAHJB,
    DAHJS,
    CAHJS,
    ALT,
    DNHJB,
    CNHJB,
    DNHJS,
    CNHJS,
    VTRAVEL,
    LTRAVEL,
    RANK_COVERAGE,
];

fn rank_catalog(mana2: bool) -> Vec<usize> {
    if mana2 {
        std::iter::once(RANK_SCORE)
            .chain(
                mana2_metrics::WEIGHTABLE_STATS
                    .iter()
                    .map(|&index| RANK_MANA_START + index),
            )
            .collect()
    } else {
        RANK_ORDER.to_vec()
    }
}

fn rank_name(m: usize) -> &'static str {
    if m >= RANK_MANA_START {
        match mana2_metrics::STAT_IDS[m - RANK_MANA_START] {
            // These IDs overlap with akler's detailed column names. Prefix
            // them so both metrics remain selectable and serializable.
            "sfb" => "M2_SFB",
            "sfs" => "M2_SFS",
            "skb" => "M2_SKB",
            "sks" => "M2_SKS",
            "lsb" => "M2_LSB",
            "lss" => "M2_LSS",
            "alt" => "M2_ALT",
            "roll" => "M2_ROLL",
            id => id,
        }
    } else if m == RANK_SCORE {
        "SCORE"
    } else if m == RANK_COVERAGE {
        "COVERAGE"
    } else if m == RANK_FJB {
        "FJB"
    } else if m == RANK_FJS {
        "FJS"
    } else {
        METRIC_NAMES[m]
    }
}

fn displayed_rank_name(m: usize, mana2: bool) -> &'static str {
    if mana2 && m >= RANK_MANA_START {
        match mana2_metrics::STAT_IDS[m - RANK_MANA_START] {
            "sfb" => "SFB",
            "sfs" => "SFS",
            "lsb" => "StretchB",
            "lss" => "StretchS",
            "vsb" => "ScissorB",
            "vss" => "ScissorS",
            "alt" => "ALT",
            "redirect" => "RED",
            "redirectweak" => "WeakRED",
            "roll" => "ROLL",
            "inroll2" => "IN2",
            "outroll2" => "OUT2",
            "inroll3" => "IN3",
            "outroll3" => "OUT3",
            "fspeed" => "Fspeed",
            "weighted_speed" => "Weighted",
            id => id,
        }
    } else {
        rank_name(m)
    }
}

fn rank_positive(m: usize) -> bool {
    m == RANK_COVERAGE
        || (m < N_METRICS && higher_better(m))
        || (m >= RANK_MANA_START
            && mana2_higher_better(mana2_metrics::STAT_IDS[m - RANK_MANA_START]))
}

/// Conventional column ordering for the screenshot statistics. This controls
/// only ranker display direction; the configurable objective is scored
/// separately from normal AKLER's lower-is-better score.
fn mana2_higher_better(id: &str) -> bool {
    matches!(id, "inroll2" | "inroll3")
}

fn rank_percent(m: usize) -> bool {
    if m == RANK_COVERAGE || m == RANK_FJB || m == RANK_FJS {
        return true;
    }
    if m < RANK_MANA_START {
        return false;
    }
    let id = mana2_metrics::STAT_IDS[m - RANK_MANA_START];
    !matches!(
        id,
        "lsb" | "vsb" | "lss" | "vss" | "sfbw" | "sfsw" | "fspeed" | "weighted_speed"
    ) && !id.starts_with("fspeed-")
        && !id.starts_with("weighted-speed-")
}

enum RankRow {
    Plain {
        model: Model,
        corpus: Arc<Corpus>,
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
    fn has_magic(&self) -> bool {
        matches!(self, Self::Action { layout, .. } if layout.slots.iter().any(|slot| matches!(slot.binding, action_keys::Binding::Named(_))))
    }

    fn has_thumb_letter(&self) -> bool {
        let alpha = |text: &[u8]| text.len() == 1 && text[0].is_ascii_alphabetic();
        match self {
            Self::Plain { model, .. } => {
                model.geometry.keys.iter().enumerate().any(|(slot, key)| {
                    !key.main && model.canonical[model.original[slot]].is_ascii_alphabetic()
                })
            }
            Self::Action { layout, .. } => layout.slots.iter().any(|slot| {
                if slot.main {
                    return false;
                }
                match &slot.binding {
                    action_keys::Binding::Text(text) => alpha(text),
                    action_keys::Binding::Named(name) => match layout.actions.get(name) {
                        Some(action_keys::Action::Text(text)) => alpha(text),
                        Some(action_keys::Action::Rules {
                            fallback: action_keys::Emission::Text(text),
                            ..
                        }) => alpha(text),
                        _ => false,
                    },
                    action_keys::Binding::Empty => false,
                }
            }),
        }
    }

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
    if m >= RANK_MANA_START {
        let raw = row.raw().1.as_ref()?;
        let stats = mana2_rank_stats(row, raw);
        Some(stats.values[m - RANK_MANA_START])
    } else if m == RANK_SCORE {
        Some(row.score())
    } else if m == RANK_FJB || m == RANK_FJS {
        let component = if m == RANK_FJB {
            [DAFJB, CAFJB, DNFJB, CNFJB]
        } else {
            [DAFJS, CAFJS, DNFJS, CNFJS]
        };
        let denominator = denominator_totals(component[0], row.raw(), row.totals());
        (denominator > 0.0).then(|| {
            pct(
                component.iter().map(|&id| row.raw().0[id].max(0.0)).sum(),
                denominator,
            )
        })
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

fn mana2_rank_stats(row: &RankRow, raw: &mana2_metrics::RawStats) -> mana2_metrics::Stats {
    match row {
        RankRow::Plain { corpus, .. } => mana_stats_for_corpus(row.raw(), corpus),
        RankRow::Action { summary, .. } => {
            action_ngrams::mana_stats(raw, *row.totals(), summary.counts.speed_populations)
        }
    }
}

// Switching objectives reuses the existing physical mappings. Only missing
// counters are calculated; action winners and corpus preparation stay intact.
fn rescore_rank_rows(rows: &mut [RankRow], weights: &Weights, mode: &str) {
    let mana = mode == "aklgg";
    for row in rows {
        if mana && row.raw().1.is_none() {
            match row {
                RankRow::Plain { model, corpus, raw, .. } => {
                    *raw = full_raw_with_mana2(&model.original, corpus, &model.geometry, true);
                }
                RankRow::Action { layout, summary, .. } => {
                    summary.raw = action_ngrams::raw_from_counts(
                        layout, &summary.counts, weights, true,
                    ).0;
                }
            }
        }
        let score = if mana {
            weights.physical_score(&mana2_rank_stats(row, row.raw().1.as_ref().unwrap()))
        } else {
            breakdown(row.metrics(), weights).net
        };
        match row {
            RankRow::Plain { score: current, .. } => *current = score,
            RankRow::Action { summary, .. } => summary.score = score,
        }
    }
}

fn plain_rank_row(
    board: Board,
    source: &Source,
    cache: &std::cell::RefCell<BTreeMap<Vec<u8>, Arc<Corpus>>>,
    weights: &Weights,
    mode: &str,
) -> AppResult<RankRow> {
    let model = Model::with_rolls(board, weights.rolls());
    // Canonical IDs depend only on the exact sorted symbol vector, including
    // duplicate blank slots. Share the immutable mapped corpus across layouts
    // with those same IDs instead of rebuilding or cloning its gram tables.
    let cached = cache.borrow().get(&model.canonical).cloned();
    let corpus = match cached {
        Some(corpus) => corpus,
        None => {
            let corpus = Arc::new(model.corpus(source)?);
            cache
                .borrow_mut()
                .insert(model.canonical.clone(), Arc::clone(&corpus));
            corpus
        }
    };
    let mana2 = mode == "aklgg";
    let raw = full_raw_with_mana2(&model.original, &corpus, &model.geometry, mana2);
    let metrics = metrics(&raw, &corpus);
    let score = if mana2 {
        let stats = mana_stats_for_corpus(&raw, &corpus);
        weights.physical_score(&stats)
    } else {
        breakdown(&metrics, weights).net
    };

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
    plain_cache: std::cell::RefCell<BTreeMap<Vec<u8>, Arc<Corpus>>>,
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
        let plain = layouts
            .iter()
            .any(|(_, layout)| matches!(layout, RankLayout::Plain(_)))
            .then(|| {
                Source::from_text_with_limits(text, path, limits).map_err(|error| error.to_string())
            });
        let action = layouts
            .iter()
            .any(|(_, layout)| matches!(layout, RankLayout::Action(_)))
            .then(|| {
                action_ngrams::NgramCorpus::from_text_progress_with_limits(
                    text,
                    path,
                    limits,
                    stop,
                    &AtomicU64::new(0),
                )
                .map(Arc::new)
            });

        Self {
            plain,
            plain_cache: std::cell::RefCell::new(BTreeMap::new()),
            action,
        }
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

#[cfg(test)]
fn evaluate_rank_layout(
    layout: RankLayout,
    corpora: &RankCorpora,
    weights: &Weights,
    stop: &AtomicBool,
) -> AppResult<RankRow> {
    evaluate_rank_layout_with_mode(layout, corpora, weights, stop, "detailed")
}

fn evaluate_rank_layout_with_mode(
    layout: RankLayout,
    corpora: &RankCorpora,
    weights: &Weights,
    stop: &AtomicBool,
    mode: &str,
) -> AppResult<RankRow> {
    match layout {
        RankLayout::Plain(board) => {
            let source = corpora
                .plain
                .as_ref()
                .ok_or("ordinary corpus was not prepared")?
                .as_ref()
                .map_err(|error| error.clone())?;
            plain_rank_row(board, source, &corpora.plain_cache, weights, mode)
        }
        RankLayout::Action(layout) => {
            let corpus = corpora
                .action
                .as_ref()
                .ok_or("action corpus was not prepared")?
                .as_ref()
                .map_err(|error| error.clone())?;
            let summary = action_summary::evaluate_with_mode(
                &layout,
                corpus,
                weights,
                stop,
                &AtomicU64::new(0),
                mode,
            )
            .map_err(|error| format!("action evaluation: {error}"))?;
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
    hidden_layouts: BTreeSet<PathBuf>,
    filters: RankFilters,
    weights: Weights,
    mana2: bool,
    scoring: String,
    spacegrams: bool,
    ngrams: NgramLimits,
}

impl RankingData {
    fn physical_catalog(&self) -> bool {
        self.mana2 || self.scoring == "aklgg"
    }
}

#[derive(Clone)]
struct Mana2RankSettings {
    weights: mana2_metrics::Weights,
    ngrams: NgramLimits,
    typing: (bool, bool),
}

fn load_ranking_progress_for_mode(
    path: &Path,
    stop: &AtomicBool,
    progress: &AtomicU64,
    mana2: Option<Mana2RankSettings>,
    typing: Option<(bool, bool)>,
    spacegrams: Option<bool>,
) -> AppResult<RankingData> {
    let mut timing = load_profile::LoadProfile::new("ranker initialization");
    let is_mana2 = mana2.is_some();
    let mut config = if is_mana2 {
        AppConfig::default()
    } else {
        load_app_config()?
    };
    if is_mana2 {
        config.rank_filters = load_rank_filters()?;
    }
    if let Some(settings) = mana2 {
        config.weights.2 = settings.weights;
        config.weights.3 = settings.typing.0;
        config.weights.4 = settings.typing.1;
        config.ngrams = settings.ngrams;
    }
    if let Some((alt_fingers, combos)) = typing {
        config.weights.3 = alt_fingers;
        config.weights.4 = combos;
    }
    if let Some(include_spacegrams) = spacegrams {
        config.ngrams.include_spacegrams = include_spacegrams;
    }
    let mut layouts = Vec::new();
    let mut errors = Vec::new();

    for layout_path in discover_layouts(Path::new(LAYOUT_DIR))? {
        if stop.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let parsed = fs::read_to_string(&layout_path)
            .map_err(|error| error.to_string())
            .and_then(|text| {
                parse_rank_layout(&text, &layout_path).map_err(|error| error.to_string())
            });
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

    let scoring = if is_mana2 { "aklgg".into() } else { config.search.mode.clone() };
    let total = layouts.len();
    let mut rows = Vec::with_capacity(total);
    for (index, (layout_path, layout)) in layouts.into_iter().enumerate() {
        if stop.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        match evaluate_rank_layout_with_mode(layout, &corpora, &config.weights, stop, &scoring) {
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
        columns: if is_mana2 || scoring == "aklgg" {
            default_mana2_rank_columns()
        } else {
            config.rank_columns
        },
        hidden_layouts: if is_mana2 {
            load_rank_hidden_preferences()?
        } else {
            config.rank_hidden_layouts
        },
        filters: config.rank_filters,
        weights: config.weights,
        mana2: is_mana2,
        scoring,
        spacegrams: config.ngrams.include_spacegrams,
        ngrams: config.ngrams,
    })
}

fn persist_hidden_rows(rows: &BTreeSet<PathBuf>) -> Option<String> {
    save_rank_hidden_layouts(rows).err().map(|error| {
        format!(
            "Row visibility changed for this session only; could not save {RANK_HIDDEN_LAYOUTS_FILE}: {error}"
        )
    })
}

/// Save the visible columns, as the settings tab does; AKL.GG's catalog
/// keeps its defaults.
fn persist_hidden_columns(columns: &RankColumns) -> Option<String> {
    if columns.mana2 {
        return None;
    }
    save_rank_columns(&columns.hidden).err().map(|error| {
        format!(
            "Column visibility changed for this session only; could not save {APP_CONFIG_FILE}: {error}"
        )
    })
}

fn load_ranking_tui(term: &mut Terminal, path: &Path) -> AppResult<Option<RankingData>> {
    load_ranking_tui_for_settings(term, path, None, None, None)
}

fn load_ranking_tui_for_settings(
    term: &mut Terminal,
    path: &Path,
    mana2: Option<Mana2RankSettings>,
    typing: Option<(bool, bool)>,
    spacegrams: Option<bool>,
) -> AppResult<Option<RankingData>> {
    let owned = prepare_corpus_path_tui(term, path)?;
    action_ui::ngram_job(
        term,
        "Ranking layouts (overall progress)",
        &corpus_name(path),
        "",
        move |stop, progress| {
            load_ranking_progress_for_mode(&owned, stop, progress, mana2, typing, spacegrams)
                .map_err(|error| error.to_string())
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

fn open_ranked_editor(term: &mut Terminal, row: &mut RankRow, weights: &Weights) -> AppResult<()> {
    if let RankRow::Action {
        layout,
        summary,
        corpus,
        details,
        ..
    } = row
    {
        let evaluation = rank_details_with(details, || {
            action_ui::evaluate_ranked(layout, corpus, weights, summary)
                .map(|value| Some(Arc::new(value)))
                .map_err(|error| error.into())
        })?;
        if let Some(evaluation) = evaluation {
            action_ui::action_editor_prepared(
                term,
                layout.clone(),
                Arc::clone(corpus),
                weights.clone(),
                evaluation,
            )?;
        }
        return Ok(());
    }

    let RankRow::Plain {
        model, corpus, raw, ..
    } = row
    else {
        unreachable!();
    };
    editor_prepared(
        term,
        model.clone(),
        (**corpus).clone(),
        weights.clone(),
        raw.clone(),
    )
}

#[derive(Clone, Debug)]
enum Hidden {
    Column(usize),
    Row(PathBuf),
}

struct RankColumns {
    hidden: [bool; RANK_COUNT],
    rows: BTreeSet<PathBuf>,
    history: Vec<Hidden>,
    redo: Vec<Hidden>,
    first: usize,
    mana2: bool,
}

impl RankColumns {
    fn new(mana2: bool) -> Self {
        Self {
            hidden: [false; RANK_COUNT],
            rows: BTreeSet::new(),
            history: Vec::new(),
            redo: Vec::new(),
            first: 0,
            mana2,
        }
    }

    fn visible(&self) -> Vec<usize> {
        rank_catalog(self.mana2)
            .into_iter()
            .filter(|&m| !self.hidden[m])
            .collect()
    }

    fn hide(&mut self, m: usize) -> bool {
        if m >= RANK_COUNT || self.hidden[m] {
            return false;
        }
        if let Some(i) = self.visible().iter().position(|&id| id == m) {
            if i < self.first {
                self.first = self.first.saturating_sub(1);
            }
        }
        self.hidden[m] = true;
        self.history.push(Hidden::Column(m));
        self.redo.clear();
        true
    }

    fn hide_row(&mut self, path: PathBuf) -> bool {
        if !self.rows.insert(path.clone()) {
            return false;
        }
        self.history.push(Hidden::Row(path));
        self.redo.clear();
        true
    }

    fn undo(&mut self, capacity: usize) -> bool {
        if let Some(last) = self.history.pop() {
            let rows_changed = matches!(last, Hidden::Row(_));
            match &last {
                Hidden::Row(p) => {
                    self.rows.remove(p);
                }
                Hidden::Column(m) => {
                    self.hidden[*m] = false;
                    if let Some(i) = self.visible().iter().position(|&id| id == *m) {
                        if i < self.first {
                            self.first = i;
                        } else if i >= self.first + capacity.max(1) {
                            self.first = i + 1 - capacity.max(1);
                        }
                    }
                }
            }
            self.redo.push(last);
            rows_changed
        } else {
            false
        }
    }

    fn redo(&mut self) -> bool {
        if let Some(next) = self.redo.pop() {
            let rows_changed = matches!(next, Hidden::Row(_));
            match &next {
                Hidden::Row(p) => {
                    self.rows.insert(p.clone());
                }
                Hidden::Column(m) => {
                    if let Some(i) = self.visible().iter().position(|&id| id == *m) {
                        if i < self.first {
                            self.first = self.first.saturating_sub(1);
                        }
                    }
                    self.hidden[*m] = true;
                }
            }
            self.history.push(next);
            rows_changed
        } else {
            false
        }
    }

    /// Show every hidden row and the columns hidden from the grid.
    fn restore(&mut self) -> bool {
        let rows_changed = !self.rows.is_empty();
        for hidden in &self.history {
            if let Hidden::Column(m) = hidden {
                self.hidden[*m] = false;
            }
        }
        self.rows.clear();
        self.history.clear();
        self.redo.clear();
        self.first = 0;
        rows_changed
    }
}

const RANK_DEFAULT_COLUMNS: &[usize] = &[
    RANK_SCORE,
    SFB,
    SKB,
    SFS,
    SKS,
    TRAVEL,
    SFTRAVEL,
    FSB,
    HSB,
    FSS,
    HSS,
    RANK_FJB,
    RANK_FJS,
    LSB,
    LSS,
    REDIR,
    INSRAF,
    OUTSRAF,
    INROLL,
    OUTROLL,
    ALT,
    RANK_COVERAGE,
];

fn default_rank_columns() -> [bool; RANK_COUNT] {
    std::array::from_fn(|metric| !RANK_DEFAULT_COLUMNS.contains(&metric))
}

fn default_mana2_rank_columns() -> [bool; RANK_COUNT] {
    let wanted = [
        "sfb",
        "sfs",
        "skb",
        "sks",
        "lsb",
        "lss",
        "vsb",
        "vss",
        "alt",
        "redirect",
        "redirectweak",
        "roll",
        "inroll2",
        "outroll2",
        "inroll3",
        "outroll3",
        "fspeed",
        "weighted_speed",
    ];
    std::array::from_fn(|metric| {
        metric != RANK_SCORE
            && (metric < RANK_MANA_START
                || !wanted.contains(&mana2_metrics::STAT_IDS[metric - RANK_MANA_START]))
    })
}

fn parse_rank_columns(text: &str) -> AppResult<[bool; RANK_COUNT]> {
    let mut hidden = [true; RANK_COUNT];
    for line in text.lines() {
        let content = line.split('#').next().unwrap_or("");
        for name in content.split_whitespace() {
            let old_half = match name.to_ascii_uppercase().as_str() {
                "DHJB" => Some((DAHJB, DNHJB)),
                "CHJB" => Some((CAHJB, CNHJB)),
                "DHJS" => Some((DAHJS, DNHJS)),
                "CHJS" => Some((CAHJS, CNHJS)),
                _ => None,
            };
            if let Some((adjacent, nonadjacent)) = old_half {
                hidden[adjacent] = false;
                hidden[nonadjacent] = false;
                continue;
            }
            let canonical = match name.to_ascii_uppercase().as_str() {
                "DFSB" | "CFSB" | "DFJB" | "CFJB" | "DFAB" | "CFAB" | "DSB" | "CSB" | "DFNB"
                | "CFNB" | "DAFJB" | "CAFJB" | "DNFJB" | "CNFJB" => "FJB",
                "DFSS" | "CFSS" | "DFJS" | "CFJS" | "DFAS" | "CFAS" | "DSS" | "CSS" | "DFNS"
                | "CFNS" | "DAFJS" | "CAFJS" | "DNFJS" | "CNFJS" => "FJS",
                _ => name,
            };
            let metric = RANK_ORDER
                .iter()
                .copied()
                .find(|&metric| rank_name(metric).eq_ignore_ascii_case(canonical))
                .ok_or_else(|| format!("unknown ranker column {name:?}"))?;
            hidden[metric] = false;
        }
    }
    if RANK_ORDER.iter().all(|&id| hidden[id]) {
        return Err("select at least one ranker column".into());
    }
    Ok(hidden)
}

#[cfg(test)]
fn rank_columns_text(hidden: &[bool; RANK_COUNT]) -> String {
    let names: Vec<_> = RANK_ORDER
        .iter()
        .copied()
        .filter(|&metric| !hidden[metric])
        .map(rank_name)
        .collect();
    format!(
        "# Visible ranker columns; edit here or press v in the ranker.\n{}\n",
        names.join(" ")
    )
}

fn toggle_rank_column(hidden: &mut [bool; RANK_COUNT], metric: usize, mana2: bool) {
    if hidden[metric]
        || rank_catalog(mana2)
            .iter()
            .filter(|&&id| !hidden[id])
            .count()
            > 1
    {
        hidden[metric] = !hidden[metric];
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RankTab {
    Rankings,
    Layouts,
    Columns,
}

fn draw_rank_tabs(canvas: &mut Canvas, active: RankTab, y: usize) {
    let index = match active {
        RankTab::Rankings => 0,
        RankTab::Layouts => 1,
        RankTab::Columns => 2,
    };
    workspace_tabs_at(canvas, y, index,
        &[" [1] Rankings ", " [2] Layouts ", " [3] Columns "]);
}

#[derive(Clone)]
struct RankSelection {
    columns: [bool; RANK_COUNT],
    layouts: BTreeSet<PathBuf>,
    filters: RankFilters,
}

fn rank_layout_choices(rows: &[RankRow], filter: &str) -> Vec<usize> {
    let filter = filter.to_ascii_lowercase();
    let mut choices: Vec<_> = (0..rows.len())
        .filter(|&index| {
            rows[index].name().to_ascii_lowercase().contains(&filter)
                || rows[index]
                    .path()
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .contains(&filter)
        })
        .collect();
    choices.sort_by(|&a, &b| {
        rows[a]
            .name()
            .cmp(rows[b].name())
            .then_with(|| rows[a].path().cmp(rows[b].path()))
    });
    choices
}

fn toggle_rank_layout(hidden: &mut BTreeSet<PathBuf>, path: &Path) {
    if !hidden.remove(path) {
        hidden.insert(path.to_owned());
    }
}

fn rank_hidden_with_filters(
    rows: &[RankRow],
    manual: &BTreeSet<PathBuf>,
    filters: RankFilters,
) -> BTreeSet<PathBuf> {
    let mut hidden = manual.clone();
    hidden.extend(
        rows.iter()
            .filter(|row| {
                (filters.hide_magic && row.has_magic())
                    || (filters.hide_non_thumb && !row.has_thumb_letter())
            })
            .map(|row| row.path().to_owned()),
    );
    hidden
}

fn rank_filtered_order(
    rows: &[RankRow],
    search: &str,
    metric: Option<usize>,
    ascending: bool,
    manual: &BTreeSet<PathBuf>,
    filters: RankFilters,
) -> Vec<usize> {
    rank_order(
        rows,
        search,
        metric,
        ascending,
        &rank_hidden_with_filters(rows, manual, filters),
    )
}

fn choose_rank_settings(
    term: &mut Terminal,
    rows: &[RankRow],
    current: &RankSelection,
    selected_path: Option<&Path>,
    mana2: bool,
    initial_tab: RankTab,
) -> AppResult<Option<RankSelection>> {
    let mut draft = current.clone();
    let mut active = initial_tab;
    let mut filter = String::new();
    let initial = rank_layout_choices(rows, "");
    let mut layout_selected = selected_path
        .and_then(|path| initial.iter().position(|&index| rows[index].path() == path))
        .unwrap_or(0);
    let mut layout_scroll = (layout_selected + 6).saturating_sub(term.size.1.max(1));
    let mut column_selected = 0;
    let mut column_scroll = 0;
    let mut status = String::new();
    let order = rank_catalog(mana2);
    loop {
        let choices = rank_layout_choices(rows, &filter);
        layout_selected = layout_selected.min(choices.len().saturating_sub(1));
        let layout_capacity = term.size.1.saturating_sub(5).max(1);
        let column_capacity = term.size.1.saturating_sub(4).max(1);
        layout_scroll = layout_scroll.min(choices.len().saturating_sub(layout_capacity));
        column_scroll = column_scroll.min(order.len().saturating_sub(column_capacity));
        let mut canvas = match active {
            RankTab::Layouts => Canvas::new(term.width(), term.size.1.max(6)),
            RankTab::Columns | RankTab::Rankings => Canvas::new(term.width(), term.size.1.max(5)),
        };
        match active {
            RankTab::Layouts => {
                header(
                    &mut canvas,
                    "Ranker settings",
                    "",
                    "",
                    "Space toggle · / search · ? help · 1/q apply · Esc cancel",
                );
                draw_rank_tabs(&mut canvas, active, 2);
                let magic = format!(
                    "[m Magic hidden: {}]",
                    if draft.filters.hide_magic {
                        "ON"
                    } else {
                        "OFF"
                    }
                );
                let no_thumb = format!(
                    "[t No thumb letter hidden: {}]",
                    if draft.filters.hide_non_thumb {
                        "ON"
                    } else {
                        "OFF"
                    }
                );
                canvas.text(1, 3, &magic, CYAN);
                canvas.hit(
                    Rect {
                        x: 1,
                        y: 3,
                        w: magic.chars().count(),
                        h: 1,
                    },
                    Action::Setting(0),
                );
                let filter_x = magic.chars().count() + 2;
                canvas.text(filter_x, 3, &no_thumb, CYAN);
                canvas.hit(
                    Rect {
                        x: filter_x,
                        y: 3,
                        w: no_thumb.chars().count(),
                        h: 1,
                    },
                    Action::Setting(1),
                );
                let note = if status.is_empty() {
                    format!("Filter: {filter}")
                } else {
                    status.clone()
                };
                canvas.text(
                    1,
                    4,
                    &short(&note, canvas.w.saturating_sub(2)),
                    if status.is_empty() { MUTED } else { CYAN },
                );
                for index in layout_scroll..(layout_scroll + layout_capacity).min(choices.len()) {
                    let row = choices[index];
                    let row = &rows[row];
                    let category_filtered = (draft.filters.hide_magic && row.has_magic())
                        || (draft.filters.hide_non_thumb && !row.has_thumb_letter());
                    let text = format!(
                        "{} [{}] {}{}  ({})",
                        if index == layout_selected { '›' } else { ' ' },
                        if draft.layouts.contains(row.path()) {
                            ' '
                        } else {
                            'x'
                        },
                        if category_filtered { "[filtered] " } else { "" },
                        row.name(),
                        row.path().display(),
                    );
                    canvas.text(
                        1,
                        index - layout_scroll + 5,
                        &short(&text, canvas.w.saturating_sub(2)),
                        if category_filtered {
                            MUTED
                        } else if index == layout_selected {
                            YELLOW
                        } else {
                            FG
                        },
                    );
                    canvas.hit(
                        Rect {
                            x: 1,
                            y: index - layout_scroll + 5,
                            w: canvas.w.saturating_sub(2),
                            h: 1,
                        },
                        Action::Item(index),
                    );
                }
                if choices.is_empty() {
                    canvas.text(1, 5, "No layouts match the filter", MUTED);
                }
            }
            RankTab::Columns | RankTab::Rankings => {
                header(
                    &mut canvas,
                    "Ranker settings",
                    "",
                    "",
                    "Space toggle · d compact · a all · 1/q apply · Esc cancel",
                );
                draw_rank_tabs(&mut canvas, active, 2);
                for index in column_scroll..(column_scroll + column_capacity).min(order.len()) {
                    let metric = order[index];
                    canvas.text(
                        1,
                        index - column_scroll + 4,
                        &format!(
                            "{} [{}] {}",
                            if index == column_selected { '›' } else { ' ' },
                            if draft.columns[metric] { ' ' } else { 'x' },
                            displayed_rank_name(metric, mana2)
                        ),
                        if index == column_selected { YELLOW } else { FG },
                    );
                    canvas.hit(
                        Rect {
                            x: 1,
                            y: index - column_scroll + 4,
                            w: 24,
                            h: 1,
                        },
                        Action::Item(index),
                    );
                }
                canvas.text(1, 3, &short(&status, canvas.w.saturating_sub(2)), CYAN);
            }
        }
        let (scroll, total, capacity) = if active == RankTab::Layouts {
            (&mut layout_scroll, choices.len(), layout_capacity)
        } else {
            (&mut column_scroll, order.len(), column_capacity)
        };
        term.present(&canvas, 0)?;
        let mut event = term.event()?;
        if scroll_event(&event, scroll, total, capacity) {
            continue;
        }
        let pressed = action_press(term, &canvas, &event, 0);
        if let Some(Action::Command(key)) = pressed {
            event = Event::Char(key);
        } else if let Some(Action::Item(index)) = pressed {
            match active {
                RankTab::Layouts => {
                    layout_selected = index;
                    toggle_rank_layout(&mut draft.layouts, rows[choices[index]].path());
                }
                RankTab::Columns | RankTab::Rankings => {
                    column_selected = index;
                    toggle_rank_column(&mut draft.columns, order[index], mana2);
                }
            }
        } else if let Some(Action::Setting(setting)) = pressed {
            event = Event::Char(if setting == 0 { 'm' } else { 't' });
        }
        match event {
            Event::Escape | Event::Quit => return Ok(None),
            Event::Char('2') => active = RankTab::Layouts,
            Event::Char('3') => active = RankTab::Columns,
            Event::Char('1') | Event::Char('q') => {
                if order.iter().all(|&id| draft.columns[id]) {
                    status = "Select at least one column".into();
                    continue;
                }
                let saved = save_rank_selection(
                    (!mana2).then_some(&draft.columns),
                    draft.filters,
                    &draft.layouts,
                );
                match saved {
                    Ok(()) => return Ok(Some(draft)),
                    Err(error) => {
                        status = error.to_string();
                        continue;
                    }
                }
            }
            Event::Up | Event::Char('k') => match active {
                RankTab::Layouts => layout_selected = layout_selected.saturating_sub(1),
                RankTab::Columns | RankTab::Rankings => {
                    column_selected = column_selected.saturating_sub(1)
                }
            },
            Event::Down | Event::Char('j') => match active {
                RankTab::Layouts => {
                    layout_selected = (layout_selected + 1).min(choices.len().saturating_sub(1))
                }
                RankTab::Columns | RankTab::Rankings => {
                    column_selected = (column_selected + 1).min(order.len().saturating_sub(1))
                }
            },
            Event::Home => match active {
                RankTab::Layouts => layout_selected = 0,
                RankTab::Columns | RankTab::Rankings => column_selected = 0,
            },
            Event::End => match active {
                RankTab::Layouts => layout_selected = choices.len().saturating_sub(1),
                RankTab::Columns | RankTab::Rankings => {
                    column_selected = order.len().saturating_sub(1)
                }
            },
            Event::Char(' ') | Event::Enter => match active {
                RankTab::Layouts => {
                    if let Some(&index) = choices.get(layout_selected) {
                        toggle_rank_layout(&mut draft.layouts, rows[index].path());
                    }
                }
                RankTab::Columns | RankTab::Rankings => {
                    toggle_rank_column(&mut draft.columns, order[column_selected], mana2)
                }
            },
            Event::Char('a') if active == RankTab::Layouts => {
                for &index in &choices {
                    draft.layouts.remove(rows[index].path());
                }
            }
            Event::Char('n') if active == RankTab::Layouts => {
                for &index in &choices {
                    draft.layouts.insert(rows[index].path().to_owned());
                }
            }
            Event::Char('/') if active == RankTab::Layouts => {
                if let Some(value) = input_box(term, "Layout filter", "", &filter)? {
                    filter = value;
                    layout_selected = 0;
                    layout_scroll = 0;
                }
            }
            Event::Char('m') if active == RankTab::Layouts => {
                draft.filters.hide_magic = !draft.filters.hide_magic;
            }
            Event::Char('t') if active == RankTab::Layouts => {
                draft.filters.hide_non_thumb = !draft.filters.hide_non_thumb;
            }
            Event::Char('?') if active == RankTab::Layouts => help_popup(
                term,
                &canvas,
                "Choose layouts",
                &[
                    "j/k or arrows\tMove through layouts".into(),
                    "Space or Enter\tToggle the selected layout".into(),
                    "m\tToggle hiding layouts with a bound magic/action key".into(),
                    "t\tToggle hiding layouts with no alphabetic thumb key".into(),
                    "a\tShow all layouts matching the name filter".into(),
                    "n\tHide all layouts matching the name filter".into(),
                    "/\tFilter names and paths".into(),
                    "1 or q\tApply settings and return to rankings".into(),
                    "2 / 3\tSwitch between layouts and columns".into(),
                    "Esc\tCancel changes".into(),
                ],
            )?,
            Event::Char('d') if active == RankTab::Columns => {
                draft.columns = if mana2 {
                    default_mana2_rank_columns()
                } else {
                    default_rank_columns()
                };
            }
            Event::Char('a') if active == RankTab::Columns => {
                for &metric in &order {
                    draft.columns[metric] = false;
                }
            }
            _ => {}
        }
        let (selected, capacity, scroll) = if active == RankTab::Layouts {
            (layout_selected, layout_capacity, &mut layout_scroll)
        } else {
            (column_selected, column_capacity, &mut column_scroll)
        };
        if selected < *scroll {
            *scroll = selected;
        } else if selected >= *scroll + capacity {
            *scroll = selected + 1 - capacity;
        }
    }
}

struct RankGrid {
    width: usize,
    name: usize,
    cell: usize,
    metrics: Vec<usize>,
    total: usize,
    capacity: usize,
}

impl RankGrid {
    fn new(width: usize, name: usize, cell: usize, columns: &mut RankColumns) -> Self {
        let name = name.min(width.saturating_sub(cell + 3));
        let capacity = (width.saturating_sub(name + 2) / (cell + 1)).max(1);
        let visible = columns.visible();
        columns.first = columns.first.min(visible.len().saturating_sub(capacity));
        let metrics: Vec<_> = visible
            .iter()
            .skip(columns.first)
            .take(capacity)
            .copied()
            .collect();
        Self {
            width: name + 2 + metrics.len() * (cell + 1),
            name,
            cell,
            metrics,
            total: visible.len(),
            capacity,
        }
    }

    fn content_x(&self, col: usize) -> usize {
        2 + self.name + col * (self.cell + 1)
    }

    fn metric_at(&self, x: usize, y: usize, count: usize) -> Option<usize> {
        if y != RANK_Y + 1 && !(RANK_DATA..RANK_DATA + count).contains(&y) {
            return None;
        }
        self.metrics.iter().enumerate().find_map(|(j, &m)| {
            let xx = self.content_x(j);
            (x >= xx && x < xx + self.cell).then_some(m)
        })
    }

    fn layout_at(&self, x: usize, y: usize, count: usize) -> Option<usize> {
        if x >= 1 && x < 1 + self.name && (RANK_DATA..RANK_DATA + count).contains(&y) {
            Some(y - RANK_DATA)
        } else {
            None
        }
    }
}

#[derive(Clone, Copy)]
struct RankRange {
    lo: f64,
    hi: f64,
}

fn rank_ranges(rows: &[RankRow], hidden: &BTreeSet<PathBuf>) -> [RankRange; RANK_COUNT] {
    let mut out = [RankRange {
        lo: f64::INFINITY,
        hi: f64::NEG_INFINITY,
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

fn rank_positive_for(m: usize, aklgg: bool) -> bool {
    if aklgg && m >= RANK_MANA_START {
        let id = mana2_metrics::STAT_IDS[m - RANK_MANA_START];
        return id.starts_with("alt") || id.starts_with("roll")
            || id.starts_with("inroll") || id.starts_with("outroll");
    }
    rank_positive(m)
}

fn rank_color_for(m: usize, v: f64, range: RankRange, aklgg: bool) -> u8 {
    if !v.is_finite() || !range.lo.is_finite() || !range.hi.is_finite() {
        return MUTED;
    }
    let span = range.hi - range.lo;
    if span.abs() <= 1e-12 * (1.0 + range.lo.abs().max(range.hi.abs())) {
        return gradient(0.5);
    }
    let t = ((v - range.lo) / span).clamp(0.0, 1.0);
    gradient(if rank_positive_for(m, aklgg) { t } else { 1.0 - t })
}

fn rank_order(
    rows: &[RankRow],
    filter: &str,
    metric: Option<usize>,
    ascending: bool,
    hidden: &BTreeSet<PathBuf>,
) -> Vec<usize> {
    let filter = filter.to_ascii_lowercase();
    let mut order: Vec<_> = (0..rows.len())
        .filter(|&i| {
            !hidden.contains(rows[i].path())
                && rows[i].name().to_ascii_lowercase().contains(&filter)
        })
        .collect();
    order.sort_by(|&a, &b| {
        if let Some(m) = metric {
            let va = rank_value(&rows[a], m);
            let vb = rank_value(&rows[b], m);
            if va.is_none() != vb.is_none() {
                return va.is_none().cmp(&vb.is_none());
            }
        }
        let cmp = if let Some(m) = metric {
            rank_value(&rows[a], m)
                .unwrap_or(0.0)
                .total_cmp(&rank_value(&rows[b], m).unwrap_or(0.0))
        } else {
            rows[a].name().cmp(rows[b].name())
        };
        (if ascending { cmp } else { cmp.reverse() })
            .then_with(|| rows[a].name().cmp(rows[b].name()))
    });
    order
}

fn rank_viewport(
    top: &mut usize,
    selected: &mut usize,
    total: usize,
    capacity: usize,
    follow: bool,
) {
    let capacity = capacity.max(1);
    *selected = (*selected).min(total.saturating_sub(1));
    *top = (*top).min(total.saturating_sub(capacity));
    if follow {
        if *selected < *top {
            *top = *selected;
        } else if *selected >= top.saturating_add(capacity) {
            *top = *selected + 1 - capacity;
        }
    } else if total > 0 {
        *selected = (*selected).clamp(*top, (top.saturating_add(capacity) - 1).min(total - 1));
    }
}

fn short_end(text: &str, width: usize) -> String {
    let cs: Vec<_> = clean_text(text).chars().collect();
    if cs.len() <= width {
        cs.into_iter().collect()
    } else if width <= 1 {
        "…".into()
    } else {
        format!("…{}", cs[cs.len() - width + 1..].iter().collect::<String>())
    }
}

fn draw_rank_table(
    c: &mut Canvas,
    g: &RankGrid,
    rows: &[RankRow],
    order: &[usize],
    top: usize,
    selected: usize,
    shown: usize,
    metric: Option<usize>,
    ascending: bool,
    dp: usize,
    ranges: &[RankRange; RANK_COUNT],
    mana2: bool,
    aklgg: bool,
) -> usize {
    let bottom = RANK_DATA + shown.max(1);
    c.boxed(
        Rect {
            x: 0,
            y: RANK_Y,
            w: g.width,
            h: bottom - RANK_Y + 1,
        },
        BORDER,
    );
    c.line(0, RANK_Y + 2, g.width, BORDER);
    c.put(0, RANK_Y + 2, '├', BORDER);
    c.put(g.width - 1, RANK_Y + 2, '┤', BORDER);
    let arrow = if ascending { "↑" } else { "↓" };
    c.text(
        2,
        RANK_Y + 1,
        &format!("Layout{}", if metric.is_none() { arrow } else { "" }),
        if metric.is_none() { YELLOW } else { FG },
    );
    c.hit(
        Rect {
            x: 1,
            y: RANK_Y + 1,
            w: g.name,
            h: 1,
        },
        Action::Command('n'),
    );
    for (j, &m) in g.metrics.iter().enumerate() {
        let x = g.content_x(j);
        c.put(x - 1, RANK_Y, '┬', BORDER);
        c.put(x - 1, bottom, '┴', BORDER);
        for y in RANK_Y + 1..bottom {
            c.put(x - 1, y, '│', BORDER);
        }
        c.put(x - 1, RANK_Y + 2, '┼', BORDER);
        c.center(
            x,
            RANK_Y + 1,
            g.cell,
            &format!(
                "{}{}",
                displayed_rank_name(m, mana2),
                if metric == Some(m) { arrow } else { "" }
            ),
            if metric == Some(m) { YELLOW } else { FG },
        );
        c.hit(
            Rect {
                x,
                y: RANK_Y + 1,
                w: g.cell,
                h: 1,
            },
            Action::Metric(m),
        );
    }
    let digits = order.len().to_string().len();
    for (offset, &i) in order.iter().skip(top).take(shown).enumerate() {
        let r = top + offset;
        let y = RANK_DATA + offset;
        let row = &rows[i];
        let prefix = format!(
            "{}{:>digits$}. ",
            if r == selected { '›' } else { ' ' },
            r + 1
        );
        let name = short_end(row.name(), g.name.saturating_sub(prefix.chars().count()));
        c.text(1, y, &format!("{prefix}{name}"), FG);
        c.hit(
            Rect {
                x: 1,
                y,
                w: g.width - 2,
                h: 1,
            },
            Action::Item(r),
        );
        for (j, &m) in g.metrics.iter().enumerate() {
            let value = rank_value(row, m);
            let text = match value {
                Some(v) => {
                    let mut s = number(v, dp);
                    if rank_percent(m) || m < N_METRICS && !physical_metric(m) {
                        s.push('%');
                    }
                    s
                }
                None => "n/a".into(),
            };
            let color = value.map(|v| rank_color_for(m, v, ranges[m], aklgg)).unwrap_or(MUTED);
            c.right(g.content_x(j) + 1, y, g.cell - 2, &text, color);
        }
        // Keep the four-row stripe pattern stable while scrolling.
        let background = if r == selected {
            Some(ROW_SELECTED)
        } else {
            match r % 4 {
                1 => Some(STRIPE_LIGHT),
                3 => Some(STRIPE_DARK),
                _ => None,
            }
        };
        c.row_style(
            Rect {
                x: 1,
                y,
                w: g.width - 2,
                h: 1,
            },
            background,
            false,
        );
    }
    if order.is_empty() {
        c.text(2, RANK_DATA, "—", MUTED);
    }
    bottom
}

fn rank_editor_return_status(weights: &Weights, spacegrams: bool) -> String {
    match load_app_config() {
        Ok(config)
            if config.weights.3 != weights.3
                || config.weights.1 != weights.1
                || config.weights.4 != weights.4
                || config.ngrams.include_spacegrams != spacegrams =>
        {
            "Typing policy changed; showing the previous ranking snapshot. r reloads it.".into()
        }
        _ => "Returned from editor; r reloads saved layouts".into(),
    }
}

fn inspect_mana2_ranked(
    term: &mut Terminal,
    row: &RankRow,
    weights: &mana2_metrics::Weights,
    mode: &str,
) -> AppResult<()> {
    let raw = row
        .raw()
        .1
        .as_ref()
        .ok_or("Mana2 statistics were not collected for this layout")?;
    let stats = mana2_rank_stats(row, raw);
    let contributions = mana2_metrics::score_contributions(&stats, weights);
    let mut lines = vec![
        format!("Layout: {}", row.name()),
        if mode == "aklgg" { format!("aklgg score: {:.6} (lower is better)", row.score()) }
        else { format!("Mana2 score: {:.6} (higher is better)", -row.score()) },
    ];
    if let RankRow::Action { layout, .. } = row {
        lines.extend(action_ui::rules_and_combos(layout));
    }
    lines.extend([
        String::new(),
        format!("{:<26} {:>12} {:>12}", "Statistic", "Value", "Score"),
    ]);
    for &index in mana2_metrics::WEIGHTABLE_STATS {
        let id = mana2_metrics::STAT_IDS[index];
        let metric = RANK_MANA_START + index;
        let value = if rank_percent(metric) {
            format!("{:.4}%", stats.values[index])
        } else {
            format!("{:.6}", stats.values[index])
        };
        lines.push(format!(
            "{:<26} {:>12} {:>12.6}",
            id, value, if mode == "aklgg" { -contributions[index] } else { contributions[index] }
        ));
    }
    let speed = finger_speed::calculate(&stats.speed, &finger_speed::Settings::default());
    lines.extend([
        String::new(),
        "Finger detail".into(),
        format!(
            "{:<8} {:>10} {:>10} {:>10}",
            "Finger", "Usage", "Fspeed", "Weighted"
        ),
    ]);
    for (name, usage_index, speed_index) in [
        ("LP", 0, 0),
        ("LR", 1, 1),
        ("LM", 2, 2),
        ("LI", 3, 3),
        ("LT", 4, 8),
        ("RT", 5, 9),
        ("RI", 6, 4),
        ("RM", 7, 5),
        ("RR", 8, 6),
        ("RP", 9, 7),
    ] {
        lines.push(format!(
            "{name:<8} {:>9.3}% {:>10.3} {:>10.3}",
            stats.values[usage_index], speed.per_finger[speed_index], speed.weighted[speed_index],
        ));
    }
    info_page(term, if mode == "aklgg" { "aklgg layout statistics" } else { "Mana2 layout statistics" }, &lines)
}

fn ranking(term: &mut Terminal, path: &Path) -> AppResult<()> {
    let Some(mut data) = load_ranking_tui(term, path)? else {
        return Ok(());
    };
    ranking_loaded(term, &mut data)
}

fn ranking_loaded(term: &mut Terminal, data: &mut RankingData) -> AppResult<()> {
    let mut metric = Some(RANK_SCORE);
    let mut ascending = true;
    let mut columns = RankColumns::new(data.physical_catalog());
    columns.hidden = data.columns;
    columns.rows = data.hidden_layouts.clone();
    let (mut selected, mut top) = (0, 0);
    let mut filter = String::new();
    let mut status = String::new();

    loop {
        if columns.mana2 != data.physical_catalog() {
            columns.mana2 = data.physical_catalog();
            columns.hidden = data.columns;
            columns.first = 0;
            columns.history.retain(|item| matches!(item, Hidden::Row(_)));
            columns.redo.clear();
            metric = Some(RANK_SCORE);
            ascending = true;
        }
        let rows = &data.rows;
        let errors = &data.errors;
        let effective_hidden = rank_hidden_with_filters(rows, &columns.rows, data.filters);
        let order = rank_order(rows, &filter, metric, ascending, &effective_hidden);
        let ranges = rank_ranges(rows, &effective_hidden);
        let width = term.size.0.max(64);
        let height = term.size.1.max(12);
        let capacity = height.saturating_sub(RANK_DATA + 3).max(1);
        rank_viewport(&mut top, &mut selected, order.len(), capacity, false);
        let shown = order.len().saturating_sub(top).min(capacity);
        let name_width = rows
            .iter()
            .map(|r| r.name().chars().count())
            .max()
            .unwrap_or(12)
            .saturating_add(order.len().to_string().len() + 3)
            .clamp(18, 30);
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
        let title = if data.mana2 {
            "Mana2 ranker".to_string()
        } else {
            format!("Ranker · {} weights [s]", data.scoring)
        };
        let alt_fingers = format!(
            "Alternate fingers: {} [f]",
            data.weights.alt_fingers_label()
        );
        let combos = format!(
            "Combos: {} [F]",
            if data.weights.4 {
                "enabled"
            } else {
                "disabled"
            }
        );
        let spacegrams = format!(
            "Spacegrams: {} [z]",
            if data.spacegrams { "ON" } else { "OFF" }
        );
        let typing = format!("{alt_fingers} · {combos} · {spacegrams}");
        header(&mut c, &title, &data.name, "", &typing);
        if !data.mana2 {
            c.hit(Rect { x: 0, y: 0, w: title.chars().count(), h: 1 }, Action::Command('s'));
        }
        draw_rank_tabs(&mut c, RankTab::Rankings, 2);
        let filter_buttons = [
            ('m', "Hide magic", data.filters.hide_magic),
            ('t', "Hide no thumb letters", data.filters.hide_non_thumb),
        ];
        let mut filter_x = 0;
        for (key, label, enabled) in filter_buttons {
            let text = format!("{key} {label}: {}", if enabled { "ON" } else { "OFF" });
            c.text(filter_x, 3, &text, if enabled { GREEN } else { MUTED });
            c.hit(
                Rect {
                    x: filter_x,
                    y: 3,
                    w: text.chars().count(),
                    h: 1,
                },
                Action::Command(key),
            );
            filter_x += text.chars().count() + 3;
        }
        let corpus_x = title.chars().count() + 2;
        c.hit(
            Rect {
                x: corpus_x,
                y: 0,
                w: data
                    .name
                    .chars()
                    .count()
                    .min(width.saturating_sub(corpus_x)),
                h: 1,
            },
            Action::Command('C'),
        );
        c.hit(
            Rect {
                x: 0,
                y: 1,
                w: alt_fingers.chars().count().min(width),
                h: 1,
            },
            Action::Command('f'),
        );
        let combos_x = alt_fingers.chars().count() + 3;
        c.hit(
            Rect {
                x: combos_x,
                y: 1,
                w: combos.chars().count().min(width.saturating_sub(combos_x)),
                h: 1,
            },
            Action::Command('F'),
        );
        let spacegrams_x = combos_x + combos.chars().count() + 3;
        c.text(
            spacegrams_x,
            1,
            &short(&spacegrams, width.saturating_sub(spacegrams_x)),
            if data.spacegrams { GREEN } else { MUTED },
        );
        c.hit(
            Rect {
                x: spacegrams_x,
                y: 1,
                w: spacegrams
                    .chars()
                    .count()
                    .min(width.saturating_sub(spacegrams_x)),
                h: 1,
            },
            Action::Command('z'),
        );
        let bottom = draw_rank_table(
            &mut c,
            &grid,
            rows,
            &order,
            top,
            selected,
            shown,
            metric,
            ascending,
            term.decimals(),
            &ranges,
            data.physical_catalog(),
            data.scoring == "aklgg",
        );
        let mut footer = format!("{} layouts · a rules · ? help", order.len());
        if !errors.is_empty() {
            footer.push_str(&format!(" · {} errors", errors.len()));
        }
        if !data.warnings.is_empty() {
            footer.push_str(" · notes");
        }
        // The selected layout's magic rules and combos at a glance.
        let mut bottom = bottom;
        if let Some(RankRow::Action { layout, .. }) = order.get(selected).map(|&i| &rows[i]) {
            if let Some(line) = action_ui::rules_and_combos(layout) {
                c.text(0, bottom + 1, &short(&line, width), CYAN);
                bottom += 1;
            }
        }
        c.text(0, bottom + 1, &short(&footer, width), MUTED);
        c.text(0, bottom + 2, &short(&status, width), CYAN);
        c.h = bottom + 3;
        term.present(&c, 0)?;
        let mut e = term.event()?;
        if let Event::Mouse {
            x,
            y,
            button: 1,
            release: false,
            motion: false,
        } = e
        {
            if let Some(offset) = grid.layout_at(x, y, shown) {
                if let Some(&i) = order.get(top + offset) {
                    if columns.hide_row(rows[i].path().to_owned()) {
                        status = persist_hidden_rows(&columns.rows).unwrap_or_default();
                    }
                }
                continue;
            }
            if let Some(m) = grid.metric_at(x, y, shown) {
                if columns.hide(m) {
                    status = persist_hidden_columns(&columns).unwrap_or_default();
                }
                continue;
            }
        }
        let delta = match e {
            Event::Wheel(n) => Some(n),
            Event::PageUp => Some(-(capacity as i32)),
            Event::PageDown => Some(capacity as i32),
            _ => None,
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
                        ascending = !ascending;
                    } else {
                        metric = Some(m);
                        ascending = !rank_positive_for(m, data.scoring == "aklgg");
                    }
                    top = 0;
                    selected = 0;
                    continue;
                }
                Action::Item(r) => {
                    if selected == r {
                        if let Some(&i) = order.get(r) {
                            if data.physical_catalog() {
                                inspect_mana2_ranked(term, &data.rows[i], &data.weights.2, &data.scoring)?;
                            } else {
                                open_ranked_editor(term, &mut data.rows[i], &data.weights)?;
                                status = rank_editor_return_status(&data.weights, data.spacegrams);
                            }
                        }
                    }
                    selected = r;
                    continue;
                }
                Action::Command(ch) => e = Event::Char(ch),
                _ => {}
            }
        }
        match e {
            Event::Escape | Event::Quit | Event::Char('q') => return Ok(()),
            Event::Char('s') if !data.mana2 => {
                let mode = if data.scoring == "normal" { "aklgg" } else { "normal" };
                let saved = load_app_config().and_then(|mut config| {
                    config.search.mode = mode.into();
                    save_config_sections(&[("search", search_settings_text(&config.search))])
                });
                match saved {
                    Ok(()) => {
                        rescore_rank_rows(&mut data.rows, &data.weights, mode);
                        data.scoring = mode.into();
                        columns.mana2 = data.physical_catalog();
                        columns.hidden = if columns.mana2 { default_mana2_rank_columns() } else {
                            load_app_config().map(|config| config.rank_columns).unwrap_or_else(|_| default_rank_columns())
                        };
                        columns.history.retain(|item| matches!(item, Hidden::Row(_)));
                        columns.redo.clear();
                        columns.first = 0;
                        metric = Some(RANK_SCORE);
                        ascending = true;
                        top = 0;
                        selected = 0;
                        status = format!("{mode} weights · lower score is better");
                    }
                    Err(error) => status = format!("Weights unchanged: {error}"),
                }
            }
            Event::Char('R') => {
                let restart = term.request_ranker_restart(&data.path);
                match restart {
                    Ok(()) => return Ok(()),
                    Err(error) => status = error.to_string(),
                }
            }
            Event::Enter => {
                if let Some(&i) = order.get(selected) {
                    if data.physical_catalog() {
                        inspect_mana2_ranked(term, &data.rows[i], &data.weights.2, &data.scoring)?;
                    } else {
                        open_ranked_editor(term, &mut data.rows[i], &data.weights)?;
                        status = rank_editor_return_status(&data.weights, data.spacegrams);
                    }
                }
            }
            Event::Up | Event::Char('k') => {
                selected = selected.saturating_sub(1);
                rank_viewport(&mut top, &mut selected, order.len(), capacity, true);
            }
            Event::Down | Event::Char('j') => {
                selected = (selected + 1).min(order.len().saturating_sub(1));
                rank_viewport(&mut top, &mut selected, order.len(), capacity, true);
            }
            Event::Left | Event::Char('h') => columns.first = columns.first.saturating_sub(1),
            Event::Right | Event::Char('l') => columns.first = columns.first.saturating_add(1),
            Event::Tab => columns.first = columns.first.saturating_add(grid.capacity),
            Event::Home => columns.first = 0,
            Event::End => columns.first = grid.total,
            Event::Char('u' | 'H') | Event::Redo => {
                let hidden = columns.hidden;
                let rows_changed = match e {
                    Event::Char('u') => columns.undo(grid.capacity),
                    Event::Redo => columns.redo(),
                    _ => columns.restore(),
                };
                status.clear();
                if rows_changed {
                    status = persist_hidden_rows(&columns.rows).unwrap_or_default();
                }
                if columns.hidden != hidden {
                    if let Some(error) = persist_hidden_columns(&columns) {
                        status = error;
                    }
                }
            }
            Event::Char('1') => {}
            Event::Char(ch @ ('2' | '3' | 'v' | 'V')) => {
                let selected_path = order
                    .get(selected)
                    .map(|&index| rows[index].path().to_owned());
                let current = RankSelection {
                    columns: columns.hidden,
                    layouts: columns.rows.clone(),
                    filters: data.filters,
                };
                let tab = if matches!(ch, '2' | 'V') {
                    RankTab::Layouts
                } else {
                    RankTab::Columns
                };
                if let Some(next) = choose_rank_settings(
                    term,
                    rows,
                    &current,
                    selected_path.as_deref(),
                    data.physical_catalog(),
                    tab,
                )? {
                    columns.hidden = next.columns;
                    columns.rows = next.layouts;
                    data.filters = next.filters;
                    columns.first = 0;
                    columns.history.clear();
                    columns.redo.clear();
                    let next_order = rank_filtered_order(
                        rows,
                        &filter,
                        metric,
                        ascending,
                        &columns.rows,
                        data.filters,
                    );
                    selected = selected_path
                        .as_ref()
                        .and_then(|path| {
                            next_order
                                .iter()
                                .position(|&index| rows[index].path() == path)
                        })
                        .unwrap_or_else(|| selected.min(next_order.len().saturating_sub(1)));
                    rank_viewport(&mut top, &mut selected, next_order.len(), capacity, true);
                    status = "Saved view".into();
                }
            }
            Event::Char(ch @ ('m' | 't')) => {
                let mut filters = data.filters;
                if ch == 'm' {
                    filters.hide_magic = !filters.hide_magic;
                } else {
                    filters.hide_non_thumb = !filters.hide_non_thumb;
                }
                match save_rank_filters(filters) {
                    Ok(()) => {
                        data.filters = filters;
                        selected = 0;
                        top = 0;
                        status.clear();
                    }
                    Err(error) => status = format!("Filters unchanged: {error}"),
                }
            }
            Event::Char(ch @ ('f' | 'F')) => {
                let selected_path = order
                    .get(selected)
                    .map(|&index| rows[index].path().to_owned());
                let alternate = ch == 'f';
                let name = if alternate {
                    "Alternate fingers"
                } else {
                    "Combos"
                };
                let old = if alternate {
                    data.weights.3
                } else {
                    data.weights.4
                };
                let mut typing = (data.weights.3, data.weights.4);
                if alternate {
                    typing.0 = !typing.0;
                } else {
                    typing.1 = !typing.1;
                }
                let next_value = !old;
                let path = data.path.clone();
                let mana2 = data.mana2.then(|| Mana2RankSettings {
                    weights: data.weights.2.clone(),
                    ngrams: data.ngrams,
                    typing,
                });
                match load_ranking_tui_for_settings(term, &path, mana2, Some(typing), None) {
                    Ok(Some(next)) => {
                        let save =
                            save_config_sections(&[("typing", typing_config_text(&next.weights))]);
                        match save {
                            Ok(()) => {
                                let next_order = rank_filtered_order(
                                    &next.rows,
                                    &filter,
                                    metric,
                                    ascending,
                                    &columns.rows,
                                    next.filters,
                                );
                                selected = selected_path
                                    .as_ref()
                                    .and_then(|path| {
                                        next_order.iter().position(|&index| {
                                            next.rows[index].path() == path.as_path()
                                        })
                                    })
                                    .unwrap_or_else(|| {
                                        selected.min(next_order.len().saturating_sub(1))
                                    });
                                *data = next;
                                rank_viewport(
                                    &mut top,
                                    &mut selected,
                                    next_order.len(),
                                    capacity,
                                    true,
                                );
                                status = format!(
                                    "{name} {}{}",
                                    if next_value { "enabled" } else { "disabled" },
                                    "; saved to akler.conf"
                                );
                            }
                            Err(error) => {
                                status = format!("{name} unchanged; could not save: {error}");
                            }
                        }
                    }
                    Ok(None) => {
                        status = format!(
                            "{name} unchanged ({}; cancelled)",
                            if old { "enabled" } else { "disabled" }
                        );
                    }
                    Err(error) => {
                        status = format!("{name} unchanged: {error}");
                    }
                }
            }
            Event::Char('z') => {
                let selected_path = order
                    .get(selected)
                    .map(|&index| rows[index].path().to_owned());
                let old = data.spacegrams;
                let next_value = !old;
                let path = data.path.clone();
                let typing = (data.weights.3, data.weights.4);
                let mut ngrams = data.ngrams;
                ngrams.include_spacegrams = next_value;
                let mana2 = data.mana2.then(|| Mana2RankSettings {
                    weights: data.weights.2.clone(),
                    ngrams,
                    typing,
                });
                match load_ranking_tui_for_settings(
                    term,
                    &path,
                    mana2,
                    Some(typing),
                    Some(next_value),
                ) {
                    Ok(Some(next)) => match save_spacegram_setting(next.spacegrams) {
                        Ok(()) => {
                            let next_order = rank_filtered_order(
                                &next.rows,
                                &filter,
                                metric,
                                ascending,
                                &columns.rows,
                                next.filters,
                            );
                            selected = selected_path
                                .as_ref()
                                .and_then(|path| {
                                    next_order.iter().position(|&index| {
                                        next.rows[index].path() == path.as_path()
                                    })
                                })
                                .unwrap_or_else(|| {
                                    selected.min(next_order.len().saturating_sub(1))
                                });
                            *data = next;
                            rank_viewport(
                                &mut top,
                                &mut selected,
                                next_order.len(),
                                capacity,
                                true,
                            );
                            status = format!(
                                "Spacegrams {}{}",
                                if next_value { "enabled" } else { "disabled" },
                                "; saved to akler.conf"
                            );
                        }
                        Err(error) => {
                            status = format!("Spacegrams unchanged; could not save: {error}");
                        }
                    },
                    Ok(None) => {
                        status = format!(
                            "Spacegrams unchanged ({}; cancelled)",
                            if old { "enabled" } else { "disabled" }
                        );
                    }
                    Err(error) => {
                        status = format!("Spacegrams unchanged: {error}");
                    }
                }
            }
            Event::Char('w') => {
                if let Some(&index) = order.get(selected) {
                    weakness_loaded(term, data, index)?;
                }
            }
            Event::Char('a') => {
                if let Some(&index) = order.get(selected) {
                    match &rows[index] {
                        RankRow::Action { layout, .. } => {
                            crate::layout_rules::show(term, &c, layout, &data.weights)?;
                        }
                        RankRow::Plain { .. } => {
                            status =
                                "This layout has no alternate fingers, combos, or actions".into();
                        }
                    }
                }
            }
            Event::Char('.') => term.precise = !term.precise,
            Event::Char('n') => {
                if metric.is_none() {
                    ascending = !ascending;
                } else {
                    metric = None;
                    ascending = true;
                }
                top = 0;
                selected = 0;
            }
            Event::Char('/') => {
                if let Some(q) = input_box(term, "", "", &filter)? {
                    filter = q;
                    top = 0;
                    selected = 0;
                }
            }
            Event::Char('e') => info_page(term, "Errors", &errors)?,
            Event::Char('i') => info_page(term, "Corpus / n-gram limits", &data.warnings)?,
            Event::Char('c') | Event::Char('C') => {
                let path = if matches!(e, Event::Char('C')) {
                    let name = if data.name.eq_ignore_ascii_case("e200") {
                        "reddit"
                    } else {
                        "e200"
                    };
                    match corpus_by_name(name) {
                        Ok(path) => Some(path),
                        Err(error) => {
                            status = format!("Corpus unchanged: {error}");
                            continue;
                        }
                    }
                } else {
                    select_source_path(term, false)?
                };
                let Some(path) = path else { continue };
                let selected_path = order
                    .get(selected)
                    .map(|&index| rows[index].path().to_owned());
                let mana2 = data.mana2.then(|| Mana2RankSettings {
                    weights: data.weights.2.clone(),
                    ngrams: data.ngrams,
                    typing: (data.weights.3, data.weights.4),
                });
                match load_ranking_tui_for_settings(term, &path, mana2, None, None) {
                    Ok(Some(next)) => {
                        let next_order = rank_filtered_order(
                            &next.rows,
                            &filter,
                            metric,
                            ascending,
                            &columns.rows,
                            next.filters,
                        );
                        selected = selected_path
                            .as_ref()
                            .and_then(|path| {
                                next_order
                                    .iter()
                                    .position(|&index| next.rows[index].path() == path.as_path())
                            })
                            .unwrap_or_else(|| selected.min(next_order.len().saturating_sub(1)));
                        rank_viewport(&mut top, &mut selected, next_order.len(), capacity, true);
                        *data = next;
                        status.clear();
                    }
                    Ok(None) => {}
                    Err(error) => status = format!("Corpus unchanged: {error}"),
                }
            }
            Event::Char('r') => {
                let path = corpus_by_name(&data.name).unwrap_or_else(|_| data.path.clone());
                match load_ranking_tui_for_settings(term, &path, None, None, None) {
                    Ok(Some(next)) => {
                        *data = next;
                        columns.hidden = data.columns;
                        columns.rows = data.hidden_layouts.clone();
                        columns.first = 0;
                        columns
                            .history
                            .retain(|item| matches!(item, Hidden::Row(_)));
                        columns.redo.clear();
                        status.clear();
                    }
                    Ok(None) => {}
                    Err(error) => status = error.to_string(),
                }
            }
            Event::Char('?') => {
                let lines = vec![
                    "j/k · ↑/↓\tSelect layout".into(),
                    if data.physical_catalog() {
                        "Enter\tView physical statistics".into()
                    } else {
                        "Enter\tEdit selected layout".into()
                    },
                    "h/l · ←/→ · Tab\tScroll columns".into(),
                    "PgUp/PgDn\tScroll layouts".into(),
                    "n\tSort by name".into(),
                    "s\tSwitch normal / aklgg".into(),
                    "/\tFilter layouts".into(),
                    ".\tChange decimal precision".into(),
                    "u\tUndo hiding".into(),
                    "Ctrl+R\tRedo hiding".into(),
                    "H\tRestore manually hidden rows and columns".into(),
                    "1/2/3\tRankings / Layouts / Columns".into(),
                    "m\tToggle hiding magic".into(),
                    "t\tToggle hiding layouts without thumb letters".into(),
                    "f\tToggle alternate fingers".into(),
                    "F\tToggle combos".into(),
                    "z\tToggle spacegrams".into(),
                    "a\tView layout rules".into(),
                    "w\tWeakness report".into(),
                    "c\tChoose corpus".into(),
                    "r\tReload files".into(),
                    "R\tRestart rebuilt program".into(),
                    "e\tView layout errors".into(),
                    "i\tView corpus notes".into(),
                    "q\tBack".into(),
                ];
                help_popup(term, &c, "Ranker help", &lines)?;
            }
            _ => {}
        }
    }
}

fn candidate_view(
    term: &mut Terminal,
    p: &Problem,
    candidates: &[Candidate],
    selected: usize,
) -> AppResult<usize> {
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
            corpus: Arc::new(corpus),
            raw,
            metrics,
            score: item.score,
        });
    }
    let mut columns = RankColumns::new(false);
    let wanted = [
        RANK_SCORE, SFB, SFS, TRAVEL, SFTRAVEL, LSB, RANK_FJB, RANK_FJS, SRAF, ROLL,
    ];
    for m in 0..RANK_COUNT {
        columns.hidden[m] = !wanted.contains(&m);
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
        header(
            &mut c,
            "Candidates",
            &p.corpora[0].name,
            &p.model.board.name,
            "q back",
        );
        let bottom = draw_rank_table(
            &mut c,
            &g,
            &rows,
            &order,
            top,
            current,
            shown,
            metric,
            ascending,
            term.decimals(),
            &ranges,
            false,
            false,
        );
        if let Some(&i) = order.get(current) {
            let nearest = candidates
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, other)| letter_distance(&p.model, &other.arr, &candidates[i].arr))
                .min();
            if let Some(n) = nearest {
                c.text(
                    0,
                    bottom + 1,
                    &format!("Nearest candidate: {n} moved letters"),
                    MUTED,
                );
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
                }
                Action::Metric(m) => {
                    if metric == Some(m) {
                        ascending = !ascending;
                    } else {
                        metric = Some(m);
                        ascending = !rank_positive(m);
                    }
                    current = 0;
                    top = 0;
                }
                _ => {}
            }
        }
        match e {
            Event::Enter => {
                if let Some(&i) = order.get(current) {
                    chosen = i;
                }
                return Ok(chosen);
            }
            Event::Char('q') | Event::Escape | Event::Quit => return Ok(chosen),
            Event::Up => current = current.saturating_sub(1),
            Event::Down => current = (current + 1).min(order.len().saturating_sub(1)),
            Event::Left => columns.first = columns.first.saturating_sub(1),
            Event::Right => columns.first = columns.first.saturating_add(1),
            Event::Char('.') => term.precise = !term.precise,
            _ => {}
        }
    }
}

#[cfg(test)]
mod ranker_tests {
    use super::*;

    const NORMAL_LAYOUT: &str =
        "q w e r t  y u i o p\na s d f g  h j k l ;\nz x c v b  n m , . /\nthumbs: space\n";

    const ACTION_LAYOUT: &str = "@ w e r t  y u i o p\na s d f g  h j k l ;\nz x c v b  n m , . /\nthumbs: space space\n@ aa\n";

    const CORPUS: &str = r#"{"monograms":{"a":3,"b":2},"bigrams":{"aa":1,"ab":1,"bb":1,"ba":1},"trigrams":{"aab":1,"abb":1,"bba":1}}"#;

    #[test]
    fn restoring_shows_grid_hidden_columns_and_keeps_configured_ones() {
        let mut columns = RankColumns::new(false);
        columns.hidden = default_rank_columns();
        let configured = columns.hidden;
        assert!(!configured[SFB] && configured[DAFJB]);
        assert!(columns.hide(SFB));
        assert!(columns.hidden[SFB]);
        assert!(columns.hide_row(PathBuf::from("layouts/a.dat")));
        assert!(columns.restore());
        assert_eq!(columns.hidden, configured);
        assert!(columns.rows.is_empty());
        // Undone hides are already shown; restoring keeps them shown.
        assert!(columns.hide(SFS));
        assert!(!columns.undo(10));
        assert!(!columns.restore());
        assert_eq!(columns.hidden, configured);
    }

    #[test]
    fn ranker_defaults_and_saved_column_round_trip() {
        let hidden = default_rank_columns();
        for metric in [SFB, SKB, SFS, SKS] {
            assert!(!hidden[metric]);
        }
        assert!(!hidden[FSB]);
        assert!(!hidden[FSS]);
        assert!(!hidden[RANK_FJB]);
        assert!(!hidden[RANK_FJS]);
        for metric in [DAFJB, CAFJB, DAFJS, CAFJS, DNFJB, CNFJB, DNFJS, CNFJS] {
            assert!(hidden[metric]);
        }
        assert_eq!(
            parse_rank_columns(&rank_columns_text(&hidden)).unwrap(),
            hidden
        );

        let mut all = [true; RANK_COUNT];
        for &metric in &RANK_ORDER {
            all[metric] = false;
        }
        // Only selectable columns can be emitted by rank_columns_text; legacy
        // detailed full-jump source columns are accepted as aliases, not shown.
        let parsed_all = parse_rank_columns(&rank_columns_text(&all)).unwrap();
        for &metric in &RANK_ORDER {
            assert_eq!(
                parsed_all[metric],
                all[metric],
                "{metric}: {}",
                rank_name(metric)
            );
        }
        for metric in [DAFJB, CAFJB, DAFJS, CAFJS, DNFJB, CNFJB, DNFJS, CNFJS] {
            assert!(parsed_all[metric]);
        }
        assert_eq!(
            parse_rank_columns("DFSB CFSS").unwrap(),
            parse_rank_columns("FJB FJS").unwrap()
        );
        assert_eq!(
            parse_rank_columns("DFJB CFJS").unwrap(),
            parse_rank_columns("FJB FJS").unwrap()
        );
        assert_eq!(
            parse_rank_columns("DFAB CFAS").unwrap(),
            parse_rank_columns("FJB FJS").unwrap()
        );
        assert_eq!(
            parse_rank_columns("DSB CSS").unwrap(),
            parse_rank_columns("FJB FJS").unwrap()
        );
        assert_eq!(
            parse_rank_columns("DFNB CFNS").unwrap(),
            parse_rank_columns("FJB FJS").unwrap()
        );
        assert_eq!(
            parse_rank_columns("DHJB CHJS").unwrap(),
            parse_rank_columns("DAHJB DNHJB CAHJS CNHJS").unwrap()
        );

        let selected = parse_rank_columns("score sfb\n# note\nSFS COVERAGE").unwrap();
        assert!(!selected[RANK_SCORE]);
        assert!(!selected[SFB]);
        assert!(selected[FSB]);
        assert!(parse_rank_columns("SCORE TYPO").is_err());
        assert!(parse_rank_columns("# empty").is_err());

        let mut one = parse_rank_columns("SCORE").unwrap();
        toggle_rank_column(&mut one, RANK_SCORE, false);
        assert!(!one[RANK_SCORE]);
        toggle_rank_column(&mut one, SFB, false);
        toggle_rank_column(&mut one, RANK_SCORE, false);
        assert!(one[RANK_SCORE]);
        assert!(!one[SFB]);
    }

    #[test]
    fn mana2_rank_direction_follows_progressive_score_sign() {
        assert!(!rank_positive(RANK_SCORE));
        assert!(rank_positive(RANK_MANA_START + 36)); // inroll2
        assert!(rank_positive(RANK_MANA_START + 40)); // inroll3
        assert!(!rank_positive(RANK_MANA_START + 15)); // lsb
        assert!(!rank_positive(RANK_MANA_START + 37)); // inroll2nothumbs
        assert!(!rank_positive(RANK_MANA_START + 34)); // legacy Mana display direction
        assert!(rank_positive_for(RANK_MANA_START + 34, true)); // AKL.GG roll
        assert!(rank_positive_for(RANK_MANA_START + 23, true)); // AKL.GG no-thumb alt
        let catalog = rank_catalog(true);
        assert!(catalog.contains(&(RANK_MANA_START + 10))); // offpinky
        assert!(catalog.contains(&(RANK_MANA_START + 45))); // goodrollnothumbs
        assert!(!rank_percent(RANK_MANA_START + 13)); // sfbw is a rating
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
            plain_cache: std::cell::RefCell::new(BTreeMap::new()),
            action: Some(Ok(Arc::new(corpus.clone()))),
        };

        let plain = evaluate_rank_layout(
            parse_rank_layout(NORMAL_LAYOUT, Path::new("normal.dat")).unwrap(),
            &prepared,
            &weights,
            &stop,
        )
        .unwrap();
        let model = Model::new(board_from_text(NORMAL_LAYOUT, Path::new("normal.dat")).unwrap());
        let ordinary_corpus = model.corpus(&source).unwrap();
        let ordinary_raw = full_raw(&model.original, &ordinary_corpus, &model.geometry);
        let ordinary_metrics = metrics(&ordinary_raw, &ordinary_corpus);
        assert_eq!(
            plain.raw().0.map(f64::to_bits),
            ordinary_raw.0.map(f64::to_bits)
        );
        assert_eq!(
            plain.metrics().v.map(f64::to_bits),
            ordinary_metrics.v.map(f64::to_bits)
        );
        assert_eq!(
            plain.score().to_bits(),
            breakdown(&ordinary_metrics, &weights).net.to_bits()
        );
        assert_eq!(
            rank_value(&plain, RANK_COVERAGE),
            Some(ordinary_corpus.coverage[1])
        );

        let plain_corpus = |row: &RankRow| match row {
            RankRow::Plain { corpus, .. } => Arc::clone(corpus),
            _ => panic!("expected an ordinary row"),
        };
        let original_corpus = plain_corpus(&plain);
        let reordered = NORMAL_LAYOUT.replace("a s", "b s").replace("v b", "v a");
        let reused = evaluate_rank_layout(
            parse_rank_layout(&reordered, Path::new("reordered.dat")).unwrap(),
            &prepared,
            &weights,
            &stop,
        )
        .unwrap();
        assert!(Arc::ptr_eq(&original_corpus, &plain_corpus(&reused)));
        assert_eq!(prepared.plain_cache.borrow().len(), 1);
        let different = NORMAL_LAYOUT.replace("q w", "! w");
        let separate = evaluate_rank_layout(
            parse_rank_layout(&different, Path::new("different.dat")).unwrap(),
            &prepared,
            &weights,
            &stop,
        )
        .unwrap();
        assert!(!Arc::ptr_eq(&original_corpus, &plain_corpus(&separate)));
        assert_eq!(prepared.plain_cache.borrow().len(), 2);
        for (text, row) in [(&reordered, &reused), (&different, &separate)] {
            let model = Model::with_rolls(
                board_from_text(text, Path::new("reference.dat")).unwrap(),
                weights.rolls(),
            );
            let corpus = model.corpus(&source).unwrap();
            let raw = full_raw(&model.original, &corpus, &model.geometry);
            let expected = metrics(&raw, &corpus);
            assert_eq!(row.raw().0.map(f64::to_bits), raw.0.map(f64::to_bits));
            assert_eq!(
                row.metrics().v.map(f64::to_bits),
                expected.v.map(f64::to_bits)
            );
            assert_eq!(
                row.score().to_bits(),
                breakdown(&expected, &weights).net.to_bits()
            );
            let actual = plain_corpus(row);
            assert_eq!(
                actual.totals.map(f64::to_bits),
                corpus.totals.map(f64::to_bits)
            );
            assert_eq!(
                actual.coverage.map(f64::to_bits),
                corpus.coverage.map(f64::to_bits)
            );
            assert_eq!(actual.by_symbol, corpus.by_symbol);
        }
        // A reload owns a new cache even when its symbols and source match.
        // Corpus changes and parser settings likewise cannot reuse old grams.
        for (text, limits) in [
            (CORPUS.to_owned(), NgramLimits::default()),
            (CORPUS.replace(":1", ":0.5"), NgramLimits::default()),
            (
                CORPUS.to_owned(),
                NgramLimits {
                    trigrams: Some(1),
                    include_spacegrams: false,
                    ..NgramLimits::default()
                },
            ),
        ] {
            let fresh = RankCorpora::prepare(
                &[(
                    PathBuf::from("normal.dat"),
                    parse_rank_layout(NORMAL_LAYOUT, Path::new("normal.dat")).unwrap(),
                )],
                &text,
                path,
                limits,
                &stop,
            );
            assert!(fresh.plain_cache.borrow().is_empty());
            let row = evaluate_rank_layout(
                parse_rank_layout(NORMAL_LAYOUT, Path::new("normal.dat")).unwrap(),
                &fresh,
                &weights,
                &stop,
            )
            .unwrap();
            assert!(!Arc::ptr_eq(&original_corpus, &plain_corpus(&row)));
            let expected_corpus = model
                .corpus(fresh.plain.as_ref().unwrap().as_ref().unwrap())
                .unwrap();
            let expected_raw = full_raw(&model.original, &expected_corpus, &model.geometry);
            let expected_metrics = metrics(&expected_raw, &expected_corpus);
            assert_eq!(
                row.raw().0.map(f64::to_bits),
                expected_raw.0.map(f64::to_bits)
            );
            assert_eq!(
                row.score().to_bits(),
                breakdown(&expected_metrics, &weights).net.to_bits()
            );
        }

        let action = evaluate_rank_layout(
            parse_rank_layout(ACTION_LAYOUT, Path::new("magic.dat")).unwrap(),
            &prepared,
            &weights,
            &stop,
        )
        .unwrap();
        let layout = action_keys::Layout::parse(ACTION_LAYOUT, Path::new("magic.dat")).unwrap();
        let expected =
            action_ui::evaluate_progress(&layout, &corpus, &weights, &stop, &progress).unwrap();
        assert_eq!(
            action.raw().0.map(f64::to_bits),
            expected.raw.0.map(f64::to_bits)
        );
        assert_eq!(
            action.metrics().v.map(f64::to_bits),
            expected.metrics.v.map(f64::to_bits)
        );
        assert_eq!(action.score().to_bits(), expected.score.to_bits());
        assert_eq!(rank_value(&action, RANK_COVERAGE), None);
        assert!(matches!(&action, RankRow::Action { layout, .. }
            if layout.slots.iter().any(|slot| slot.label == "@")));

        for row in [&plain, &action] {
            for (column, parts) in [
                (RANK_FJB, [DAFJB, CAFJB, DNFJB, CNFJB]),
                (RANK_FJS, [DAFJS, CAFJS, DNFJS, CNFJS]),
            ] {
                let expected: f64 = parts.iter().map(|&metric| row.metrics().v[metric]).sum();
                assert!((rank_value(row, column).unwrap() - expected).abs() < 1e-9);
            }
        }

        let mut rows = vec![plain, action];
        let original_scores: Vec<_> = rows.iter().map(|row| row.score().to_bits()).collect();
        let original_counts = match &rows[1] {
            RankRow::Action { summary, .. } => Arc::clone(&summary.counts),
            _ => unreachable!(),
        };
        for mode in ["aklgg"] {
        rescore_rank_rows(&mut rows, &weights, mode);
        for row in &rows {
            let expected = weights.physical_score(
                &mana2_rank_stats(row, row.raw().1.as_ref().unwrap()),
            );
            assert_eq!(row.score().to_bits(), expected.to_bits());
        }
        if let RankRow::Action { summary, .. } = &rows[1] {
            assert!(Arc::ptr_eq(&original_counts, &summary.counts));
        }
        }
        rescore_rank_rows(&mut rows, &weights, "normal");
        assert_eq!(rows.iter().map(|row| row.score().to_bits()).collect::<Vec<_>>(), original_scores);
        let hidden = BTreeSet::new();
        let order = rank_order(&rows, "", Some(RANK_SCORE), true, &hidden);
        assert_eq!(order.len(), 2);
        assert_eq!(rank_order(&rows, "magic", None, true, &hidden), vec![1]);
        assert_eq!(
            rank_order(&rows, "", Some(RANK_COVERAGE), false, &hidden),
            vec![0, 1]
        );
        assert!(!rows[0].has_magic());
        assert!(rows[1].has_magic());
        assert!(rows.iter().all(|row| !row.has_thumb_letter()));
        let thumb_text = NORMAL_LAYOUT
            .replace("a s", "~ s")
            .replace("thumbs: space", "thumbs: a | space");
        let thumb = evaluate_rank_layout(
            parse_rank_layout(&thumb_text, Path::new("thumb.dat")).unwrap(),
            &prepared,
            &weights,
            &stop,
        )
        .unwrap();
        assert!(thumb.has_thumb_letter());
        let literal_at = evaluate_rank_layout(
            parse_rank_layout(
                &NORMAL_LAYOUT.replace("q w", "char:@ w"),
                Path::new("literal-at.dat"),
            )
            .unwrap(),
            &prepared,
            &weights,
            &stop,
        )
        .unwrap();
        assert!(!literal_at.has_magic());
        let mut rows = rows;
        rows.extend([thumb, literal_at]);
        let hidden = BTreeSet::from([
            PathBuf::from("missing-layout.dat"),
            rows[3].path().to_owned(),
        ]);
        let filters = RankFilters {
            hide_magic: true,
            hide_non_thumb: true,
        };
        assert_eq!(
            rank_filtered_order(&rows, "", None, true, &hidden, filters),
            vec![2]
        );
        assert_eq!(
            rank_filtered_order(&rows, "", None, true, &hidden, RankFilters::default()).len(),
            3
        );
        let magic_only = RankFilters {
            hide_magic: true,
            hide_non_thumb: false,
        };
        assert_eq!(
            rank_filtered_order(&rows, "", None, true, &hidden, magic_only).len(),
            2
        );
        assert_eq!(hidden.len(), 2);
        assert!(hidden.contains(Path::new("missing-layout.dat")));
        assert_eq!(
            parse_rank_hidden_layouts(&rank_hidden_layouts_text(&hidden).unwrap()).unwrap(),
            hidden
        );
    }

    #[test]
    fn ranker_prepares_only_required_corpus_engines() {
        let path = Path::new("inline.json");
        let stop = AtomicBool::new(false);
        let normal = || {
            (
                PathBuf::from("normal.dat"),
                parse_rank_layout(NORMAL_LAYOUT, Path::new("normal.dat")).unwrap(),
            )
        };
        let magic = || {
            (
                PathBuf::from("magic.dat"),
                parse_rank_layout(ACTION_LAYOUT, Path::new("magic.dat")).unwrap(),
            )
        };

        let action_only =
            RankCorpora::prepare(&[magic()], CORPUS, path, NgramLimits::default(), &stop);
        assert!(action_only.plain.is_none());
        assert!(action_only.action.as_ref().unwrap().is_ok());

        let ordinary =
            RankCorpora::prepare(&[normal()], CORPUS, path, NgramLimits::default(), &stop);
        assert!(ordinary.plain.as_ref().unwrap().is_ok());
        assert!(ordinary.action.is_none());

        let mixed = RankCorpora::prepare(
            &[normal(), magic()],
            CORPUS,
            path,
            NgramLimits::default(),
            &stop,
        );
        assert!(mixed.plain.as_ref().unwrap().is_ok());
        assert!(mixed.action.as_ref().unwrap().is_ok());

        let limited = RankCorpora::prepare(
            &[normal(), magic()],
            CORPUS,
            path,
            NgramLimits {
                trigrams: Some(1),
                ..NgramLimits::default()
            },
            &stop,
        );
        assert_eq!(
            limited.plain.as_ref().unwrap().as_ref().unwrap().tables[3].len(),
            1
        );
        assert!(!limited.warnings().is_empty());
        assert_eq!(
            mixed.plain.as_ref().unwrap().as_ref().unwrap().tables[3].len(),
            3
        );
    }

    #[test]
    fn ranker_does_not_hide_action_or_corpus_errors() {
        let weights = Weights::default();
        let stop = AtomicBool::new(false);
        let prepared = RankCorpora {
            plain: None,
            plain_cache: std::cell::RefCell::new(BTreeMap::new()),
            action: Some(Err("missing cached contexts".into())),
        };

        let missing = evaluate_rank_layout(
            parse_rank_layout(ACTION_LAYOUT, Path::new("magic.dat")).unwrap(),
            &prepared,
            &weights,
            &stop,
        );
        assert!(missing
            .err()
            .unwrap()
            .to_string()
            .contains("missing cached contexts"));

        let invalid = format!("{ACTION_LAYOUT}swap y ,/u\n");
        assert!(parse_rank_layout(&invalid, Path::new("invalid.dat")).is_err());
    }

    #[test]
    fn action_details_are_lazy_reused_and_retryable_after_cancellation_or_error() {
        let stop = AtomicBool::new(false);
        let weights = Weights::default();
        let prepared = RankCorpora::prepare(
            &[(
                PathBuf::from("magic.dat"),
                parse_rank_layout(ACTION_LAYOUT, Path::new("magic.dat")).unwrap(),
            )],
            CORPUS,
            Path::new("inline.json"),
            NgramLimits::default(),
            &stop,
        );
        let mut row = evaluate_rank_layout(
            parse_rank_layout(ACTION_LAYOUT, Path::new("magic.dat")).unwrap(),
            &prepared,
            &weights,
            &stop,
        )
        .unwrap();
        let RankRow::Action {
            layout,
            summary,
            corpus,
            details,
            ..
        } = &mut row
        else {
            panic!("expected an action row");
        };
        assert!(details.is_none());
        assert!(Arc::ptr_eq(
            corpus,
            prepared.action.as_ref().unwrap().as_ref().unwrap()
        ));

        assert!(rank_details_with(details, || Ok(None)).unwrap().is_none());
        let error = rank_details_with(details, || Err("interrupted detail load".into()))
            .err()
            .unwrap();
        assert_eq!(error.to_string(), "interrupted detail load");
        assert!(details.is_none());

        let first = rank_details_with(details, || {
            action_ui::evaluate_ranked(layout, corpus, &weights, summary)
                .map(|value| Some(Arc::new(value)))
                .map_err(|error| error.into())
        })
        .unwrap()
        .unwrap();
        assert!(Arc::ptr_eq(&first.counts, &summary.counts));
        assert_eq!(
            first.raw.0.map(f64::to_bits),
            summary.raw.0.map(f64::to_bits)
        );
        assert_eq!(
            first.metrics.v.map(f64::to_bits),
            summary.metrics.v.map(f64::to_bits)
        );
        let reopened =
            rank_details_with(details, || panic!("cached details must not be reevaluated"))
                .unwrap()
                .unwrap();
        assert!(Arc::ptr_eq(&first, &reopened));
    }

    #[test]
    fn action_ranking_reload_creates_independent_corpus_layout_and_limit_snapshots() {
        let stop = AtomicBool::new(false);
        let weights = Weights::default();
        let make_row = |source: &str, text: &str, limits: NgramLimits, weights: &Weights| {
            let layout = parse_rank_layout(source, Path::new("magic.dat")).unwrap();
            let prepared = RankCorpora::prepare(
                &[(PathBuf::from("magic.dat"), layout)],
                text,
                Path::new("inline.json"),
                limits,
                &stop,
            );
            evaluate_rank_layout(
                parse_rank_layout(source, Path::new("magic.dat")).unwrap(),
                &prepared,
                weights,
                &stop,
            )
            .unwrap()
        };
        let original = make_row(ACTION_LAYOUT, CORPUS, NgramLimits::default(), &weights);
        let original_score = original.score().to_bits();
        let mut changed_weights = weights.clone();
        changed_weights.0[SFB] += 7.0;
        let changed_rule = ACTION_LAYOUT.replace("@ aa", "@ ab");
        let staggered = format!("{ACTION_LAYOUT}row-stagger: anglemod\n");
        let changed_corpus = CORPUS.replace(":1", ":0.5");
        let replacements = [
            make_row(ACTION_LAYOUT, CORPUS, NgramLimits::default(), &weights),
            make_row(&changed_rule, CORPUS, NgramLimits::default(), &weights),
            make_row(&staggered, CORPUS, NgramLimits::default(), &weights),
            make_row(
                ACTION_LAYOUT,
                &changed_corpus,
                NgramLimits::default(),
                &weights,
            ),
            make_row(
                ACTION_LAYOUT,
                CORPUS,
                NgramLimits {
                    trigrams: Some(1),
                    ..NgramLimits::default()
                },
                &weights,
            ),
            make_row(
                ACTION_LAYOUT,
                CORPUS,
                NgramLimits::default(),
                &changed_weights,
            ),
        ];
        let RankRow::Action {
            corpus: old_corpus,
            details: old_details,
            ..
        } = &original
        else {
            panic!("expected an action row");
        };
        for replacement in &replacements {
            let RankRow::Action {
                corpus, details, ..
            } = replacement
            else {
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
