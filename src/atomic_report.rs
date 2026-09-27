//! On-demand weighted physical-pattern reports. Normal evaluation and search
//! never call this module. The existing evaluator owns all n-gram mapping.

use crate::action_ngrams as ng;
use crate::atomic_analysis::{self, AtomicKeyboard};
use crate::atomic_metrics::{classify, Finger, Query, SelectedPress, SlotId};
use crate::*;

type PhysicalTable = BTreeMap<Vec<SlotId>, f64>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Population {
    Bigrams,
    Trigrams,
    Skip1,
}

impl Population {
    fn parse(value: &str) -> AppResult<Self> {
        match value {
            "bigrams" => Ok(Self::Bigrams),
            "trigrams" => Ok(Self::Trigrams),
            "skip1" => Ok(Self::Skip1),
            _ => Err(
                format!("unsupported --patterns {value:?}; use bigrams, trigrams, or skip1").into(),
            ),
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Bigrams => "bigrams",
            Self::Trigrams => "trigrams",
            Self::Skip1 => "skip1",
        }
    }

    pub(crate) fn next(self) -> Self {
        match self {
            Self::Bigrams => Self::Trigrams,
            Self::Trigrams => Self::Skip1,
            Self::Skip1 => Self::Bigrams,
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Bigrams => 0,
            Self::Trigrams => 1,
            Self::Skip1 => 2,
        }
    }

    fn corpus_kind(self) -> usize {
        match self {
            Self::Bigrams => 1,
            Self::Skip1 => 2,
            Self::Trigrams => 3,
        }
    }

    fn selected_positions(self) -> &'static [usize] {
        match self {
            Self::Bigrams => &[0, 1],
            Self::Trigrams => &[0, 1, 2],
            Self::Skip1 => &[0, 2],
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ReportRow {
    pub(crate) slots: Vec<SlotId>,
    pub(crate) frequency: f64,
    pub(crate) stats: Vec<&'static str>,
}

#[derive(Clone, Debug)]
pub(crate) struct AtomicReport {
    pub(crate) layout: String,
    pub(crate) corpus: String,
    pub(crate) population: Population,
    pub(crate) query: String,
    pub(crate) source: String,
    pub(crate) warnings: Vec<String>,
    pub(crate) population_frequency: f64,
    pub(crate) matching_frequency: f64,
    pub(crate) rows: Vec<ReportRow>,
    pub(crate) keys: Vec<crate::atomic_metrics::PhysicalKey>,
}

/// Aggregate by physical slot sequence. This is deliberately separate from
/// classification and rendering, so a future UI can reuse the same report.
fn analyze(
    keyboard: &AtomicKeyboard,
    layout: &str,
    corpus: &str,
    population: Population,
    query_text: Option<&str>,
    source: &str,
    warnings: &[String],
    counts: &PhysicalTable,
    rolls: RollSettings,
) -> AppResult<AtomicReport> {
    let query = query_text.map(Query::parse).transpose()?;
    let population_frequency: f64 = counts.values().copied().sum();
    if !population_frequency.is_finite() {
        return Err("atomic population frequency is not finite".into());
    }
    let mut rows = Vec::new();
    let mut matching_frequency = 0.0;
    let positions = population.selected_positions();
    for (slots, &frequency) in counts {
        if slots.len() != positions.len() {
            return Err("atomic source supplied a pattern with the wrong length".into());
        }
        let selected: Vec<_> = slots
            .iter()
            .zip(positions)
            .map(|(&slot_id, &original_position)| SelectedPress {
                slot_id,
                original_position,
            })
            .collect();
        let pattern = classify(&keyboard.keys, &selected, Some(&keyboard.pairs))?;
        let matches = match &query {
            Some(query) => query.matches(&pattern)?,
            None => true,
        };
        if matches {
            matching_frequency += frequency;
            let bits = keyboard.metric_bits(slots, population == Population::Skip1, rolls)?;
            let stats = METRIC_NAMES
                .iter()
                .enumerate()
                .filter(|(index, _)| bits & bit(*index) != 0)
                .map(|(_, &name)| name)
                .collect();
            rows.push(ReportRow {
                slots: slots.clone(),
                frequency,
                stats,
            });
        }
    }
    if !matching_frequency.is_finite() {
        return Err("atomic matching frequency is not finite".into());
    }
    rows.sort_by(|a, b| {
        b.frequency
            .total_cmp(&a.frequency)
            .then_with(|| a.slots.cmp(&b.slots))
    });
    Ok(AtomicReport {
        layout: layout.to_owned(),
        corpus: corpus.to_owned(),
        population,
        query: query_text.unwrap_or("all patterns").to_owned(),
        source: source.to_owned(),
        warnings: warnings.to_vec(),
        population_frequency,
        matching_frequency,
        rows,
        keys: keyboard.keys.clone(),
    })
}

/// A single layout/corpus snapshot. Preparing it performs physical mapping once;
/// filtering and redraws never re-run either evaluator.
pub(crate) struct PreparedAtomic {
    keyboard: AtomicKeyboard,
    layout: String,
    corpus: String,
    source: [String; 3],
    warnings: Vec<String>,
    tables: [Option<PhysicalTable>; 3],
    rolls: RollSettings,
}

impl PreparedAtomic {
    pub(crate) fn available(&self, population: Population) -> bool {
        self.tables[population.index()].is_some()
    }

    #[cfg(test)]
    pub(crate) fn clear_geometry_for_test(&mut self) {
        self.keyboard.pairs = Default::default();
    }

    pub(crate) fn report(
        &self,
        population: Population,
        query: Option<&str>,
    ) -> AppResult<AtomicReport> {
        let index = population.index();
        let counts = self.tables[index].as_ref().ok_or_else(|| {
            format!(
                "{} population is unavailable in this corpus",
                population.name()
            )
        })?;
        analyze(
            &self.keyboard,
            &self.layout,
            &self.corpus,
            population,
            query,
            &self.source[index],
            &self.warnings,
            counts,
            self.rolls,
        )
    }
}

pub(crate) fn prepare_ordinary(board: Board, source: Source) -> AppResult<PreparedAtomic> {
    let keyboard = atomic_analysis::from_board(&board)?;
    let layout_name = board.name.clone();
    let model = Model::new(board);
    let rolls = load_weights(Path::new(WEIGHTS_FILE))?.rolls();
    let corpus = model.corpus(&source)?;
    let physical_positions = positions(&model.original);
    let mut tables: [Option<PhysicalTable>; 3] = std::array::from_fn(|index| {
        let population = [Population::Bigrams, Population::Trigrams, Population::Skip1][index];
        source.available[population.corpus_kind()].then(PhysicalTable::new)
    });
    for gram in &corpus.grams {
        let index = match gram.kind {
            1 => 0,
            3 => 1,
            2 => 2,
            _ => continue,
        };
        if let Some(counts) = &mut tables[index] {
            let slots: Vec<_> = gram.ids[..gram.len]
                .iter()
                .map(|&id| physical_positions[id] as SlotId)
                .collect();
            *counts.entry(slots).or_default() += gram.f;
        }
    }
    Ok(PreparedAtomic {
        keyboard, layout: layout_name, corpus: corpus.name,
        source: [
            "Ordinary evaluator: supported stored n-grams mapped to physical slots.".into(),
            "Ordinary evaluator: supported stored n-grams mapped to physical slots.".into(),
            "Ordinary evaluator: stored skip endpoints, or source-derived trigram endpoints when noted below; the middle press is not retained in this table.".into(),
        ],
        warnings: corpus.warnings, tables, rolls,
    })
}

pub(crate) fn prepare_action(
    layout: action_keys::Layout,
    corpus: ng::NgramCorpus,
) -> AppResult<PreparedAtomic> {
    let keyboard = atomic_analysis::from_action_layout(&layout)?;
    let weights = load_weights(Path::new(WEIGHTS_FILE))?;
    let effort = action_ui::LocalEffort::new(&layout, &weights);
    let counted = corpus.evaluate(
        &layout,
        &AtomicBool::new(false),
        &AtomicU64::new(0),
        |a, b, key| effort.get(a, b, key),
    )?;
    let source_note = format!("Magic evaluator: weighted physical winners from cached contexts through order {}; retained/mapped population.", corpus.order);
    let raw_tables = [&counted.tables[1], &counted.tables[2], &counted.skip];
    let mut tables = [None, None, None];
    for (index, raw) in raw_tables.into_iter().enumerate() {
        let mut counts = PhysicalTable::new();
        for (slots, &frequency) in raw {
            let ids = slots
                .iter()
                .map(|&id| SlotId::try_from(id))
                .collect::<Result<Vec<_>, _>>()?;
            *counts.entry(ids).or_default() += frequency;
        }
        tables[index] = Some(counts);
    }
    Ok(PreparedAtomic {
        keyboard,
        layout: layout.name,
        corpus: corpus.name,
        source: std::array::from_fn(|_| source_note.clone()),
        warnings: corpus.warnings,
        tables,
        rolls: weights.rolls(),
    })
}

pub(crate) fn ordinary_counts(
    board: Board,
    source: Source,
    population: Population,
    query: Option<&str>,
) -> AppResult<AtomicReport> {
    prepare_ordinary(board, source)?.report(population, query)
}

pub(crate) fn action_counts(
    layout: action_keys::Layout,
    corpus: ng::NgramCorpus,
    population: Population,
    query: Option<&str>,
) -> AppResult<AtomicReport> {
    prepare_action(layout, corpus)?.report(population, query)
}

fn printable(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if ch.is_control() {
            out.push_str(&format!("\\u{{{:04x}}}", ch as u32));
        } else {
            out.push(ch);
        }
    }
    out
}

fn sanitized_label(label: &str) -> String {
    if label == " " {
        return "␠".into();
    }
    printable(label)
}

fn finger_name(finger: Finger) -> &'static str {
    match finger {
        Finger::LeftPinky => "LP",
        Finger::LeftRing => "LR",
        Finger::LeftMiddle => "LM",
        Finger::LeftIndex => "LI",
        Finger::LeftThumb => "LT",
        Finger::RightIndex => "RI",
        Finger::RightMiddle => "RM",
        Finger::RightRing => "RR",
        Finger::RightPinky => "RP",
        Finger::RightThumb => "RT",
    }
}

pub(crate) fn percent(value: f64, denominator: f64) -> String {
    if denominator == 0.0 {
        "n/a".into()
    } else {
        let value = 100.0 * value / denominator;
        if value > 0.0 && value < 0.005 {
            return "<0.01%".into();
        }
        let rounded = format!("{value:.2}");
        format!("{}%", rounded.trim_end_matches('0').trim_end_matches('.'))
    }
}

fn pad_right(text: &str, width: usize) -> String {
    format!(
        "{text}{}",
        " ".repeat(width.saturating_sub(text.chars().count()))
    )
}

fn pad_left(text: &str, width: usize) -> String {
    format!(
        "{}{text}",
        " ".repeat(width.saturating_sub(text.chars().count()))
    )
}

/// The CLI, editor, and export use the same aligned five-column table.
/// Display rounding never changes frequencies, matching, or row order.
pub(crate) fn table_lines(report: &AtomicReport) -> (String, Vec<String>) {
    let labels: Vec<_> = report
        .keys
        .iter()
        .map(|key| sanitized_label(key.label.as_deref().unwrap_or("")))
        .collect();
    let mut multiplicity = BTreeMap::<&str, usize>::new();
    for label in &labels {
        *multiplicity.entry(label.as_str()).or_default() += 1;
    }
    let separator = if report.population == Population::Skip1 {
        " _ "
    } else {
        " → "
    };
    let mut cells = Vec::with_capacity(report.rows.len());
    let mut widths = [
        "Physical keys".chars().count(),
        "Fingers".len(),
        "Rows".len(),
        1,
        "Stats".len(),
    ];
    for row in &report.rows {
        let mut key_names = Vec::new();
        let mut fingers = Vec::new();
        let mut rows = Vec::new();
        for &slot in &row.slots {
            let key = &report.keys[slot as usize];
            let label = &labels[slot as usize];
            key_names.push(if label.is_empty() || multiplicity[label.as_str()] > 1 {
                format!("{}[#{slot}]", if label.is_empty() { "key" } else { label })
            } else {
                label.clone()
            });
            fingers.push(finger_name(key.finger).to_string());
            rows.push((i64::from(key.row) + 1).to_string());
        }
        let values = [
            key_names.join(separator),
            fingers.join(separator),
            rows.join(separator),
            percent(row.frequency, report.population_frequency),
            if row.stats.is_empty() {
                "—".into()
            } else {
                row.stats.join(", ")
            },
        ];
        for (width, value) in widths.iter_mut().zip(&values) {
            *width = (*width).max(value.chars().count());
        }
        cells.push(values);
    }
    let line = |values: [&str; 5]| {
        format!(
            "{} | {} | {} | {} | {}",
            pad_right(values[0], widths[0]),
            pad_right(values[1], widths[1]),
            pad_right(values[2], widths[2]),
            pad_left(values[3], widths[3]),
            pad_right(values[4], widths[4]),
        )
    };
    let header = line(["Physical keys", "Fingers", "Rows", "%", "Stats"]);
    let rows = cells
        .iter()
        .map(|values| line(values.each_ref().map(String::as_str)))
        .collect();
    (header, rows)
}

/// Full, width-independent text export. Summary frequencies display six
/// decimals; percentages display up to two. Calculations use unrounded f64.
pub(crate) fn render_text(report: &AtomicReport) -> String {
    let mut out =
        format!(
        "Atomic physical patterns\nLayout: {}\nCorpus: {}\nPopulation: {}\nQuery: {}\nSource: {}\n",
        printable(&report.layout), printable(&report.corpus), report.population.name(),
        printable(&report.query), printable(&report.source)
    );
    if report.warnings.is_empty() {
        out.push_str("Source limits/warnings: none reported\n");
    } else {
        out.push_str("Source limits/warnings:\n");
        for warning in &report.warnings {
            out.push_str(&format!("- {}\n", printable(warning)));
        }
    }
    out.push_str(&format!(
        "Population frequency: {:.6}\nMatching frequency: {:.6}\nMatch percentage: {}\n",
        report.population_frequency,
        report.matching_frequency,
        percent(report.matching_frequency, report.population_frequency)
    ));
    let (header, rows) = table_lines(report);
    out.push_str(&format!("\n{header}\n"));
    for row in rows {
        out.push_str(&row);
        out.push('\n');
    }
    out
}

const USAGE: &str =
    "atomic LAYOUT CORPUS --patterns bigrams|trigrams|skip1 [--query QUERY] [--output PATH]";

pub(crate) fn write_new_report(path: &Path, text: &str) -> AppResult<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(text.as_bytes())?;
    Ok(())
}

pub(crate) fn command(args: &[String]) -> AppResult<()> {
    if args.len() < 4 {
        return Err(USAGE.into());
    }
    let layout_path = Path::new(&args[0]);
    let mut population = None;
    let mut query = None;
    let mut output = None;
    let mut i = 2;
    while i < args.len() {
        let option = args[i].as_str();
        let value = args
            .get(i + 1)
            .ok_or_else(|| format!("{option} requires a value; {USAGE}"))?;
        match option {
            "--patterns" if population.is_none() => population = Some(Population::parse(value)?),
            "--query" if query.is_none() => query = Some(value.as_str()),
            "--output" if output.is_none() => output = Some(Path::new(value)),
            "--patterns" | "--query" | "--output" => {
                return Err(format!("duplicate option {option}").into())
            }
            _ => return Err(format!("unknown option {option}; {USAGE}").into()),
        }
        i += 2;
    }
    let population = population.ok_or_else(|| format!("--patterns is required; {USAGE}"))?;
    let corpus_path = corpus_by_name(&args[1])?;
    let text = fs::read_to_string(layout_path)?;
    let layout = parse_open_layout(&text, layout_path)?;
    let report = match layout {
        OpenLayout::Plain(board) => {
            ordinary_counts(board, Source::load(&corpus_path)?, population, query)?
        }
        OpenLayout::Action(layout) => {
            let cached = if corpus_is_raw(&corpus_path)? {
                ensure_corpus(&corpus_path, false, Some(3))?
            } else {
                corpus_path
            };
            action_counts(layout, ng::NgramCorpus::load(&cached)?, population, query)?
        }
    };
    let rendered = render_text(&report);
    if let Some(path) = output {
        write_new_report(path, &rendered)?;
    } else {
        io::stdout().lock().write_all(rendered.as_bytes())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action_keys as ak;

    const ROWS: &str =
        "q w e r t | y u i o p\na s d f g | h j k l ;\nz x c v b | n m , . /\nthumbs: space\n";

    fn inline_corpus(text: &[u8], scale: f64) -> String {
        let tables = ak::text_ngrams(text);
        let names = ["letters", "bigrams", "trigrams", "fourgrams", "fivegrams"];
        let mut out = String::from("{");
        for order in 0..3 {
            if order > 0 {
                out.push(',');
            }
            out.push_str(&format!("\"{}\":{{", names[order]));
            for (i, (gram, frequency)) in tables[order].iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&format!(
                    "{}:{}",
                    ak::quote(gram),
                    *frequency as f64 * scale
                ));
            }
            out.push('}');
        }
        out.push('}');
        out
    }

    fn plain_board() -> Board {
        board_from_text(ROWS, Path::new("ordinary.dat")).unwrap()
    }

    fn magic_layout() -> ak::Layout {
        let rows = ROWS.replacen("w e", "@magic e", 1);
        ak::Layout::parse(
            &format!("{rows}action magic = magic\nfallback magic = repeat-output\n"),
            Path::new("magic.dat"),
        )
        .unwrap()
    }

    fn source(text: &[u8], scale: f64) -> Source {
        Source::from_text(&inline_corpus(text, scale), Path::new("inline.json")).unwrap()
    }

    fn action_corpus(text: &[u8], scale: f64) -> ng::NgramCorpus {
        ng::NgramCorpus::from_text(&inline_corpus(text, scale), Path::new("inline.json")).unwrap()
    }

    #[test]
    fn stats_column_uses_existing_pair_skip_and_triple_flags() {
        let keyboard = atomic_analysis::from_board(&plain_board()).unwrap();
        let report_for = |population, slots: Vec<SlotId>| {
            analyze(
                &keyboard,
                "board",
                "corpus",
                population,
                None,
                "inline",
                &[],
                &BTreeMap::from([(slots, 0.5)]),
                RollSettings::default(),
            )
            .unwrap()
        };
        let stats = |population, slots| report_for(population, slots).rows[0].stats.clone();
        assert!(stats(Population::Bigrams, vec![0, 10]).contains(&"SFB"));
        assert!(stats(Population::Bigrams, vec![0, 0]).contains(&"SKB"));
        let inward = stats(Population::Bigrams, vec![0, 1]);
        assert!(inward.contains(&"SRAF") && inward.contains(&"INSRAF"));
        let outward = stats(Population::Bigrams, vec![1, 0]);
        assert!(outward.contains(&"SRAF") && outward.contains(&"OUTSRAF"));
        for (slots, full, directional) in
            [(vec![0, 21], "FSB", "DFSB"), (vec![1, 20], "FSB", "CFSB")]
        {
            let names = stats(Population::Bigrams, slots.clone());
            assert!(
                names.contains(&full) && names.contains(&directional),
                "{names:?}"
            );
            let names = stats(Population::Skip1, slots);
            assert!(names.contains(&"FSS"));
            assert!(names.contains(&if directional == "DFSB" {
                "DFSS"
            } else {
                "CFSS"
            }));
        }
        assert!(stats(Population::Skip1, vec![0, 10]).contains(&"SFS"));
        assert!(stats(Population::Skip1, vec![0, 0]).contains(&"SKS"));
        let weak = stats(Population::Trigrams, vec![0, 2, 1]);
        assert!(weak.contains(&"RED") && weak.contains(&"WRED"), "{weak:?}");
        let wish = stats(Population::Trigrams, vec![0, 3, 1]);
        assert!(wish.contains(&"RED") && wish.contains(&"WISH"), "{wish:?}");
        let text = render_text(&report_for(Population::Bigrams, vec![0, 21]));
        assert!(text.contains("% | Stats"));
        assert!(text.contains("FSB") && text.contains("DFSB"));
        let top_to_bottom = report_for(Population::Bigrams, vec![0, 21]);
        let (_, lines) = table_lines(&top_to_bottom);
        assert_eq!(lines[0].split(" | ").nth(2).unwrap().trim(), "1 → 3");
        assert_eq!(top_to_bottom.keys[0].row, 0);
    }

    #[test]
    fn ordinary_populations_use_separate_weighted_tables_and_fixed_denominators() {
        let source = source(b"qswq", 0.5);
        let board = plain_board();
        let expected = [
            (Population::Bigrams, 1.5, 3),
            (Population::Trigrams, 1.0, 2),
            (Population::Skip1, 1.0, 2),
        ];
        for (population, total, distinct) in expected {
            let all = ordinary_counts(board.clone(), source.clone(), population, None).unwrap();
            assert_eq!(all.population_frequency, total);
            assert_eq!(all.matching_frequency, total);
            assert_eq!(all.rows.len(), distinct);
            let filtered = ordinary_counts(
                board.clone(),
                source.clone(),
                population,
                Some("start.key = 0"),
            )
            .unwrap();
            assert_eq!(filtered.population_frequency, total);
            assert!(filtered.matching_frequency < total);
        }
        let skipped = ordinary_counts(
            board.clone(),
            source.clone(),
            Population::Skip1,
            Some("gap[0] = 1 and start.key = 0 and end.key = 1"),
        )
        .unwrap();
        assert_eq!(skipped.matching_frequency, 0.5);
        assert_eq!(skipped.rows[0].slots, vec![0, 1]);
        // Existing mapped ordinary totals remain the source of these populations.
        let model = Model::new(board);
        let corpus = model.corpus(&source).unwrap();
        assert_eq!(corpus.totals[1], 1.5);
        assert_eq!(corpus.totals[2], 1.0);
        assert_eq!(corpus.totals[3], 1.0);
        let mut swapped = plain_board();
        swapped.symbols.swap(0, 1);
        let moved = ordinary_counts(swapped, source, Population::Bigrams, None).unwrap();
        assert_eq!(moved.population_frequency, 1.5);
        assert!(moved.rows.iter().any(|row| row.slots == [1, 11]));
    }

    #[test]
    fn magic_populations_use_winning_root_slots_after_a_swap() {
        let mut layout = magic_layout();
        let corpus = action_corpus(b"aaa", 0.5);
        let action_slot = layout
            .slots
            .iter()
            .position(|s| s.label == "@magic")
            .unwrap();
        let first =
            action_counts(layout.clone(), corpus.clone(), Population::Bigrams, None).unwrap();
        assert!(first
            .rows
            .iter()
            .any(|row| row.slots.contains(&(action_slot as SlotId))));
        for (population, expected) in [
            (Population::Bigrams, 1.0),
            (Population::Trigrams, 0.5),
            (Population::Skip1, 0.5),
        ] {
            let report = action_counts(layout.clone(), corpus.clone(), population, None).unwrap();
            assert_eq!(report.population_frequency, expected);
            assert_eq!(report.matching_frequency, expected);
        }
        layout.swap(action_slot, 9);
        let moved =
            action_counts(layout.clone(), corpus.clone(), Population::Bigrams, None).unwrap();
        assert!(moved.rows.iter().any(|row| row.slots.contains(&9)));
        assert_eq!(moved.keys[9].label.as_deref(), Some("@magic"));
        assert_eq!(moved.keys[9].finger, Finger::RightPinky);
        let physical = atomic_analysis::from_action_layout(&layout).unwrap();
        let weights = load_weights(Path::new(WEIGHTS_FILE)).unwrap();
        for row in &moved.rows {
            let expected = physical
                .metric_bits(&row.slots, false, weights.rolls())
                .unwrap();
            for (index, &name) in METRIC_NAMES.iter().enumerate() {
                assert_eq!(row.stats.contains(&name), expected & bit(index) != 0);
            }
        }
        // The same evaluator with the same policy still produces the table
        // used by this report, without the report changing evaluator state.
        let effort = action_ui::LocalEffort::new(&layout, &weights);
        let direct = corpus
            .evaluate(
                &layout,
                &AtomicBool::new(false),
                &AtomicU64::new(0),
                |a, b, key| effort.get(a, b, key),
            )
            .unwrap();
        let direct_total: f64 = direct.tables[1].values().sum();
        assert_eq!(moved.population_frequency, direct_total);
        let with_gap = action_corpus(b"a!a", 1.0);
        let no_join = action_counts(layout, with_gap, Population::Skip1, None).unwrap();
        assert_eq!(no_join.population_frequency, 0.0);
    }

    #[test]
    fn empty_available_population_is_na_and_unavailable_one_errors() {
        let available = Source::from_text(
            r#"{"letters":{"q":1,"w":1},"bigrams":{"qw":1},"trigrams":{}}"#,
            Path::new("empty.json"),
        )
        .unwrap();
        let empty = ordinary_counts(plain_board(), available, Population::Trigrams, None).unwrap();
        assert_eq!(empty.population_frequency, 0.0);
        assert_eq!(empty.matching_frequency, 0.0);
        assert!(empty.rows.is_empty());
        assert!(render_text(&empty).contains("Match percentage: n/a"));
        let unavailable = Source::from_text(
            r#"{"letters":{"q":1,"w":1},"bigrams":{"qw":1}}"#,
            Path::new("missing.json"),
        )
        .unwrap();
        assert!(!unavailable.available[2]);
        assert!(!unavailable.available[3]);
        assert!(ordinary_counts(
            plain_board(),
            unavailable.clone(),
            Population::Trigrams,
            None
        )
        .unwrap_err()
        .to_string()
        .contains("unavailable"));
        assert!(
            ordinary_counts(plain_board(), unavailable, Population::Skip1, None)
                .unwrap_err()
                .to_string()
                .contains("unavailable")
        );
    }

    #[test]
    fn ordinary_skip_report_keeps_existing_endpoint_only_semantics() {
        let source = Source::from_text(
            r#"{"letters":{"a":4,"q":3,"w":2,"r":1,"!":1},
                "bigrams":{"a!":1,"!q":1,"aw":2,"wq":1},
                "skipgrams":{"aq":2,"ar":1},
                "trigrams":{"a!q":1,"awq":1,"awr":1}}"#,
            Path::new("skip-source.json"),
        )
        .unwrap();
        let report = ordinary_counts(
            plain_board(),
            source,
            Population::Skip1,
            Some("start.key = 10 and end.key = 0"),
        )
        .unwrap();
        assert_eq!(report.population_frequency, 3.0);
        assert_eq!(report.matching_frequency, 2.0);
        assert_eq!(report.rows[0].slots, [10, 0]);
        assert!(report.source.contains("middle press is not retained"));
    }

    #[test]
    fn capped_source_warning_describes_the_retained_denominator() {
        let source = Source::from_text_with_limits(
            &inline_corpus(b"qswq", 1.0),
            Path::new("capped.json"),
            NgramLimits {
                trigrams: Some(1),
                tetragrams: None,
                pentagrams: None,
            },
        )
        .unwrap();
        let report = ordinary_counts(plain_board(), source, Population::Trigrams, None).unwrap();
        assert_eq!(report.population_frequency, 1.0);
        assert_eq!(report.rows.len(), 1);
        let text = render_text(&report);
        assert!(text.contains("Approximate 3-grams: 1/2 sequences retained"));
        assert!(text.contains("Population frequency: 1.000000"));
    }

    #[test]
    fn query_errors_keep_positions_and_missing_geometry_is_not_false() {
        let board = plain_board();
        let keyboard = atomic_analysis::from_board(&board).unwrap();
        let mut counts = PhysicalTable::new();
        counts.insert(vec![0, 1], 0.25);
        let error = analyze(
            &keyboard,
            "board",
            "corpus",
            Population::Bigrams,
            Some("length = 2 and first.unknown = true"),
            "source",
            &[],
            &counts,
            RollSettings::default(),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("query byte 15"), "{error}");
        let mut missing = atomic_analysis::from_board(&board).unwrap();
        missing.pairs = Default::default();
        let error = analyze(
            &missing,
            "board",
            "corpus",
            Population::Bigrams,
            Some("first.scissor = false"),
            "source",
            &[],
            &counts,
            RollSettings::default(),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("UnsupportedAttribute"), "{error}");
    }

    #[test]
    fn deterministic_export_keeps_all_rows_and_disambiguates_duplicate_labels() {
        assert_eq!(sanitized_label("space"), "space");
        assert_eq!(sanitized_label(" "), "␠");
        let mut keyboard = atomic_analysis::from_board(&plain_board()).unwrap();
        keyboard.keys[0].label = Some("duplicate".into());
        keyboard.keys[1].label = Some("duplicate".into());
        keyboard.keys[2].label = Some("\u{1b}[31m".into());
        let mut counts = PhysicalTable::new();
        counts.insert(vec![1, 0], 0.25);
        counts.insert(vec![0, 1], 0.25);
        counts.insert(vec![2, 0], 0.125);
        let report = analyze(
            &keyboard,
            "board",
            "corpus",
            Population::Bigrams,
            None,
            "retained source",
            &["capped".into()],
            &counts,
            RollSettings::default(),
        )
        .unwrap();
        assert_eq!(report.population_frequency, 0.625);
        assert_eq!(
            report.rows.iter().map(|row| &row.slots).collect::<Vec<_>>(),
            vec![&vec![0, 1], &vec![1, 0], &vec![2, 0]]
        );
        let text = render_text(&report);
        assert!(text.contains("duplicate[#0] → duplicate[#1]"));
        assert!(text.contains("duplicate[#1] → duplicate[#0]"));
        assert_eq!(
            text.lines()
                .filter(|line| line.contains('→') && line.contains(" | "))
                .count(),
            3
        );
        assert!(!text.contains('\u{1b}'));
        assert!(text.contains("\\u{001b}"));
        assert!(text.contains("Population frequency: 0.625000"));
        assert!(text.contains("Match percentage: 100%"));
        let (header, rows) = table_lines(&report);
        assert_eq!(header.split(" | ").count(), 5);
        assert!(header.trim_end().ends_with("Stats"));
        assert!(!header.contains("Frequency"));
        let widths = header
            .split(" | ")
            .map(|cell| cell.chars().count())
            .collect::<Vec<_>>();
        for row in &rows {
            assert_eq!(
                row.split(" | ")
                    .map(|cell| cell.chars().count())
                    .collect::<Vec<_>>(),
                widths
            );
        }
        assert!(rows
            .iter()
            .any(|row| row.split(" | ").nth(3).unwrap().trim() == "40%"));
        assert!(rows
            .iter()
            .any(|row| row.split(" | ").nth(3).unwrap().trim() == "20%"));
        let output_path = std::env::temp_dir().join(format!(
            "akler-atomic-report-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        write_new_report(&output_path, &text).unwrap();
        assert_eq!(fs::read_to_string(&output_path).unwrap(), text);
        assert!(write_new_report(&output_path, "replacement").is_err());
        fs::remove_file(output_path).unwrap();
        let none = analyze(
            &atomic_analysis::from_board(&plain_board()).unwrap(),
            "board",
            "corpus",
            Population::Bigrams,
            Some("start.key = 99"),
            "source",
            &[],
            &BTreeMap::from([(vec![0, 1], 0.25)]),
            RollSettings::default(),
        )
        .unwrap();
        assert_eq!(none.population_frequency, 0.25);
        assert_eq!(none.matching_frequency, 0.0);
        assert!(none.rows.is_empty());
        assert!(render_text(&none).contains("Match percentage: 0%"));
    }

    #[test]
    fn percentage_display_has_at_most_two_decimals() {
        assert_eq!(percent(1.0, 1.0), "100%");
        assert_eq!(percent(1.0, 8.0), "12.5%");
        assert_eq!(percent(1.0, 3.0), "33.33%");
        assert_eq!(percent(0.000001, 1.0), "<0.01%");
        assert_eq!(percent(0.0, 0.0), "n/a");
    }

    #[test]
    fn cli_rejects_invalid_options() {
        let args = |tail: &[&str]| {
            ["missing.dat", "missing-corpus"]
                .into_iter()
                .chain(tail.iter().copied())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        assert!(command(&args(&["--patterns", "unknown"]))
            .unwrap_err()
            .to_string()
            .contains("unsupported --patterns"));
        assert!(command(&args(&["--bogus", "x"]))
            .unwrap_err()
            .to_string()
            .contains("unknown option"));
        assert!(
            command(&args(&["--patterns", "bigrams", "--patterns", "skip1"]))
                .unwrap_err()
                .to_string()
                .contains("duplicate option")
        );
    }
}
