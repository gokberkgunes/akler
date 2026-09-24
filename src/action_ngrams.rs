//! Cached n-gram evaluation. No raw corpus or ordered-text sidecar is opened here.
//! Each weighted context contributes only its final physical event, avoiding
//! overlap double-counting. Magic history is bounded by the selected table order.
use crate::action_keys as ak;

use crate::*;

use crate::action_profile::{clock, elapsed, Profile};

type Table = BTreeMap<Vec<usize>, f64>;

#[derive(Clone, Debug)]
pub struct Counts {
    pub tables: [Table; 3],
    pub skip: Table,
    pub characters: f64,
    pub presses: f64,
    pub action_presses: f64,
    pub ignored_characters: f64,
    pub order: usize,
}

impl Counts {
    fn new(order: usize) -> Self {
        Self {
            tables: std::array::from_fn(|_| Table::new()),
            skip: Table::new(),
            characters: 0.0,
            presses: 0.0,
            action_presses: 0.0,
            ignored_characters: 0.0,
            order,
        }
    }

    fn report_json(&self, layout: &ak::Layout) -> String {
        let mut out = format!("{{\n  \"kind\": \"physical-keystroke-ngram-estimate\",\n  \"context_order\": {},\n  \"policy\": \"greedy local effort within each cached context; literal wins ties\",\n  \"keys\": [", self.order);
        for (i, slot) in layout.slots.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&crate::json_quote(&slot.label));
        }
        out.push_str("],\n  \"ngrams\": [");
        for (i, table) in self.tables.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push('[');
            for (j, (keys, count)) in table.iter().enumerate() {
                if j > 0 {
                    out.push(',');
                }
                out.push_str(&format!("[{:?},{}]", keys, count));
            }
            out.push(']');
        }
        out.push_str("],\n  \"skipgrams\": [");
        for (i, (keys, count)) in self.skip.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&format!("[{:?},{}]", keys, count));
        }
        out.push_str(&format!(
            "],\n  \"presses\": {},\n  \"characters\": {},\n  \"ignored_characters\": {}\n}}\n",
            self.presses, self.characters, self.ignored_characters
        ));
        out
    }

    pub fn write_report(&self, layout: &ak::Layout, path: &Path) -> ak::Result<()> {
        let out = self.report_json(layout);
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|e| e.to_string())?;
        f.write_all(out.as_bytes()).map_err(|e| e.to_string())
    }
}

fn read_ngram_table(
    p: &mut JsonParser<'_>,
    width: usize,
    keep: bool,
    stop: &AtomicBool,
    progress: &AtomicU64,
) -> AppResult<Vec<(String, f64)>> {
    p.ws();
    p.expect(b'{')?;
    p.ws();
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    if p.byte() == Some(b'}') {
        p.p += 1;
        return Ok(out);
    }
    loop {
        if stop.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        p.ws();
        let name = p.string()?;
        p.ws();
        p.expect(b':')?;
        let count = match p.value(0)? {
            Json::Number(f) if f.is_finite() && f >= 0.0 => f,
            _ => return Err("n-gram count must be finite and non-negative".into()),
        };
        if name.chars().count() != width {
            return Err(format!("invalid {width}-gram {name:?}").into());
        }
        if keep {
            if !seen.insert(name.clone()) {
                return Err(format!("duplicate n-gram {name:?}").into());
            }
            out.push((name, count));
        }
        progress.store((p.p * 90 / p.text.len().max(1)) as u64, Ordering::Relaxed);
        p.ws();
        if p.byte() == Some(b'}') {
            p.p += 1;
            break;
        }
        p.expect(b',')?;
    }
    Ok(out)
}

#[derive(Clone, Debug)]
struct Window {
    bytes: [u8; 5],
    len: usize,
    weight: f64,
}

impl Window {
    fn text(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

fn weighted_contexts(
    tables: &[BTreeMap<Vec<u8>, f64>; 5],
    order: usize,
    stop: &AtomicBool,
    collect: bool,
) -> AppResult<Vec<Window>> {
    let mut windows = Vec::new();
    for n in 1..=order {
        let mut extended: BTreeMap<Vec<u8>, f64> = BTreeMap::new();
        if n < order {
            for (gram, f) in &tables[n] {
                if stop.load(Ordering::Relaxed) {
                    return Err("cancelled".into());
                }
                *extended.entry(gram[1..].to_vec()).or_default() += f;
            }
        }

        // Only occurrences with no stored left extension belong to the
        // shorter context. The rest are counted in a longer context.
        for (gram, f) in &tables[n - 1] {
            if stop.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            let covered = extended.remove(gram).unwrap_or(0.0);
            let tol = 1e-8 * f.abs().max(1.0);
            if covered > *f + tol {
                return Err("N-gram orders have inconsistent frequencies; rebuild them together (raw counts, not separately normalized percentages).".into());
            }
            let weight = (*f - covered).max(0.0);
            if collect && weight > 0.0 {
                let mut bytes = [0; 5];
                bytes[..n].copy_from_slice(gram);
                windows.push(Window {
                    bytes,
                    len: n,
                    weight,
                });
            }
        }
        if extended.values().any(|f| *f > 1e-8) {
            return Err(
                "N-gram table contains a suffix missing from its lower order; rebuild the corpus."
                    .into(),
            );
        }
    }
    Ok(windows)
}

fn limit_context_tables(
    tables: &mut [BTreeMap<Vec<u8>, f64>; 5],
    order: usize,
    caps: [Option<usize>; 5],
    stop: &AtomicBool,
) -> AppResult<Vec<String>> {
    let mut warnings = Vec::new();
    for i in 2..order {
        if stop.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let original = tables[i].len();
        let original_mass = tables[i].values().sum();

        // A retained context needs its suffix in the preceding table. Without
        // this restriction, subtracting extension counts could create negative
        // residuals or references to absent lower-order contexts.
        let (lower, current) = tables.split_at_mut(i);
        let table = &mut current[0];
        table.retain(|gram, _| lower[i - 1].contains_key(&gram[1..]));
        if let Some(cap) = caps[i] {
            if table.len() > cap {
                let selected =
                    top_ngram_keys(table.iter().map(|(gram, f)| (gram.clone(), *f)), cap);
                table.retain(|gram, _| selected.contains(gram));
            }
        }

        if table.len() != original {
            warnings.push(ngram_limit_warning(
                i + 1,
                table.len(),
                original,
                table.values().sum(),
                original_mass,
            ));
        }
    }
    Ok(warnings)
}

#[derive(Clone, Debug)]
pub struct NgramCorpus {
    pub name: String,
    pub order: usize,
    pub warnings: Vec<String>,
    windows: Arc<[Window]>,
}

impl NgramCorpus {
    pub fn load(path: &Path) -> ak::Result<Self> {
        Self::load_progress(path, &AtomicBool::new(false), &AtomicU64::new(0))
    }

    pub fn load_progress(path: &Path, stop: &AtomicBool, progress: &AtomicU64) -> ak::Result<Self> {
        if path.extension().and_then(|x| x.to_str()) != Some("json") {
            return Err("Magic evaluation uses cached .json n-grams. Build/select a corpus in Corpora first.".into());
        }
        let mut timing = crate::load_profile::LoadProfile::new("action corpus load");
        let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
        timing.mark("Corpus read");
        let limits = load_app_config().map_err(|e| e.to_string())?.ngrams;
        let result = Self::parse(&text, path, stop, progress, limits).map_err(|e| e.to_string());
        timing.mark("Corpus parse/preparation (nested report)");
        result
    }

    pub fn from_text(text: &str, path: &Path) -> ak::Result<Self> {
        Self::from_text_with_limits(text, path, NgramLimits::default())
    }

    pub fn from_text_with_limits(text: &str, path: &Path, limits: NgramLimits) -> ak::Result<Self> {
        Self::parse(
            text,
            path,
            &AtomicBool::new(false),
            &AtomicU64::new(0),
            limits,
        )
        .map_err(|e| e.to_string())
    }

    pub(crate) fn from_text_progress_with_limits(
        text: &str,
        path: &Path,
        limits: NgramLimits,
        stop: &AtomicBool,
        progress: &AtomicU64,
    ) -> ak::Result<Self> {
        Self::parse(text, path, stop, progress, limits).map_err(|e| e.to_string())
    }

    fn parse(
        text: &str,
        path: &Path,
        stop: &AtomicBool,
        progress: &AtomicU64,
        limits: NgramLimits,
    ) -> AppResult<Self> {
        let mut timing = crate::load_profile::LoadProfile::new("action corpus preparation");
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut p = JsonParser { text, p: 0 };
        p.ws();
        p.expect(b'{')?;
        let mut tables: [BTreeMap<Vec<u8>, f64>; 5] = std::array::from_fn(|_| BTreeMap::new());
        let mut got = [false; 5];
        let mut fields = BTreeSet::new();
        loop {
            if stop.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            p.ws();
            if p.byte() == Some(b'}') {
                p.p += 1;
                break;
            }
            let name = p.string()?;
            if !fields.insert(name.clone()) {
                return Err(format!("duplicate corpus field {name}").into());
            }
            p.ws();
            p.expect(b':')?;
            let order = match name.as_str() {
                "letters" | "monograms" | "unigrams" => Some(1),
                "bigrams" => Some(2),
                "trigrams" => Some(3),
                "fourgrams" | "quadrigrams" | "quadgrams" | "tetragrams" => Some(4),
                "fivegrams" => Some(5),
                _ => None,
            };
            if let Some(n) = order {
                if got[n - 1] {
                    return Err(
                        format!("multiple {n}-gram tables; keep one table per order").into(),
                    );
                }
                got[n - 1] = true;
                for (s, f) in read_ngram_table(&mut p, n, true, stop, progress)? {
                    if stop.load(Ordering::Relaxed) {
                        return Err("cancelled".into());
                    }
                    if !f.is_finite() {
                        return Err("non-finite n-gram frequency".into());
                    }
                    // Non-ASCII/control characters retain one position as a gap.
                    let key = s
                        .chars()
                        .map(|c| {
                            if c.is_ascii() && !c.is_ascii_control() {
                                c.to_ascii_lowercase() as u8
                            } else {
                                0
                            }
                        })
                        .collect();
                    *tables[n - 1].entry(key).or_default() += f;
                }
            } else if name == "skipgrams" {
                read_ngram_table(&mut p, 2, false, stop, progress)?;
            } else {
                p.value(0)?;
            }
            p.ws();
            if p.byte() == Some(b'}') {
                p.p += 1;
                break;
            }
            p.expect(b',')?;
        }
        p.ws();
        if p.p != text.len() {
            return Err("trailing corpus JSON data".into());
        }
        if !got[..3].iter().all(|x| *x) {
            return Err("Magic metrics require letters, bigrams and trigrams; rebuild through order 3, 4 or 5 in Corpora.".into());
        }
        timing.mark("JSON parse, normalize and merge n-grams");
        let order = got.iter().rposition(|v| *v).unwrap() + 1;
        if !got[..order].iter().all(|x| *x) {
            return Err(
                "Missing intermediate n-gram table; rebuild the corpus through the selected order."
                    .into(),
            );
        }
        if tables.iter().any(|t| !t.values().sum::<f64>().is_finite()) {
            return Err("n-gram frequency sum overflow".into());
        }
        let caps = [
            None,
            None,
            limits.trigrams,
            limits.tetragrams,
            limits.pentagrams,
        ];
        if caps.iter().any(|cap| *cap == Some(0)) {
            return Err("n-gram limits must be positive or all".into());
        }
        let limited = (2..order).any(|i| caps[i].is_some_and(|cap| tables[i].len() > cap));

        // Validate the complete source even when limits will remove entries.
        // With all/unreached limits, retain the original arithmetic and order.
        let mut windows = weighted_contexts(&tables, order, stop, !limited)?;
        let mut warnings = Vec::new();
        if limited {
            warnings = limit_context_tables(&mut tables, order, caps, stop)?;
            windows = weighted_contexts(&tables, order, stop, true)?;
            warnings.push("Removed contexts use retained shorter suffixes; action history and physical metrics are approximate.".into());
        }
        if windows.is_empty() {
            return Err("no usable cached n-grams".into());
        }
        drop(tables);
        timing.mark("Validate orders and construct weighted contexts");
        windows.sort_by(|a, b| a.text().cmp(b.text()));
        if stop.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        progress.store(100, Ordering::Relaxed);
        timing.mark("Sort contexts");
        Ok(Self {
            name: corpus_name(path),
            order,
            warnings,
            windows: windows.into(),
        })
    }

    pub fn evaluate<F>(
        &self,
        layout: &ak::Layout,
        stop: &AtomicBool,
        progress: &AtomicU64,
        effort: F,
    ) -> ak::Result<Counts>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        let mut timing = crate::load_profile::LoadProfile::new("detailed physical mapping");
        let program = match crate::action_fast::Program::new(layout, self.order) {
            Ok(program) => program,
            Err(_) => {
                // Preserve reference validation errors and support layouts outside
                // the numeric mapper's capacity. Parsed keyboards fit its limit.
                let mut mapper = ak::WindowMapper::new(layout, self.order)?;
                timing.mark("Validate and initialize reference mapper fallback");
                return self.evaluate_mapped(layout, stop, progress, &mut timing, |text| {
                    mapper.map(text, &effort)
                });
            }
        };
        let state = crate::action_fast::KeyState::new(&program);
        let mut mapper = crate::action_fast::Mapper::new(&program, &state);
        timing.mark("Validate and compile numeric mapper");

        self.evaluate_mapped(layout, stop, progress, &mut timing, |text| {
            mapper.map(text, &effort)
        })
    }

    #[cfg(test)]
    pub(crate) fn evaluate_reference<F>(
        &self,
        layout: &ak::Layout,
        stop: &AtomicBool,
        progress: &AtomicU64,
        effort: F,
    ) -> ak::Result<Counts>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        let mut timing = crate::load_profile::LoadProfile::new("reference physical mapping");
        let mut mapper = ak::WindowMapper::new(layout, self.order)?;
        timing.mark("Validate and initialize reference mapper");

        self.evaluate_mapped(layout, stop, progress, &mut timing, |text| {
            mapper.map(text, &effort)
        })
    }

    // Keep window traversal, histogram insertion, frequency accumulation and
    // cancellation/progress checks identical for numeric and reference mapping.
    fn evaluate_mapped<F>(
        &self,
        layout: &ak::Layout,
        stop: &AtomicBool,
        progress: &AtomicU64,
        timing: &mut crate::load_profile::LoadProfile,
        mut map: F,
    ) -> ak::Result<Counts>
    where
        F: FnMut(&[u8]) -> ak::Result<[Option<usize>; 5]>,
    {
        let mut counts = Counts::new(self.order);
        for (i, window) in self.windows.iter().enumerate() {
            if stop.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            let keys = map(window.text())?;
            let last = window.len - 1;
            if let Some(key) = keys[last] {
                counts.characters += window.weight;
                counts.presses += window.weight;
                if matches!(layout.slots[key].binding, ak::Binding::Named(_)) {
                    counts.action_presses += window.weight;
                }
                for n in 1..=3.min(window.len) {
                    let tail = &keys[window.len - n..window.len];
                    if tail.iter().any(Option::is_none) {
                        break;
                    }
                    let ids = tail.iter().map(|x| x.unwrap()).collect();
                    *counts.tables[n - 1].entry(ids).or_default() += window.weight;
                }
                if window.len >= 3 && keys[last - 1].is_some() {
                    if let Some(first) = keys[last - 2] {
                        *counts.skip.entry(vec![first, key]).or_default() += window.weight;
                    }
                }
            } else {
                counts.ignored_characters += window.weight;
            }
            if i % 1024 == 0 {
                progress.store(
                    ((i + 1) * 100 / self.windows.len()) as u64,
                    Ordering::Relaxed,
                );
            }
        }
        progress.store(100, Ordering::Relaxed);
        timing.mark("Map contexts and build physical count tables");
        Ok(counts)
    }
}

// One compact terminal history per weighted context; metric contributions are
// linear in frequency, so rejected proposals never need to mutate this cache.
const GAP: u8 = u8::MAX;

type Tail = [u8; 3];

fn terminal(keys: [Option<usize>; 5], len: usize) -> Tail {
    let mut tail = [GAP; 3];
    for j in 0..len.min(3) {
        tail[2 - j] = keys[len - 1 - j].map(|k| k as u8).unwrap_or(GAP);
    }
    tail
}

fn contribute(
    raw: &mut Raw,
    totals: &mut [f64; 4],
    tail: Tail,
    f: f64,
    pos: &[usize],
    geometry: &Geometry,
) {
    let [a, b, c] = tail;
    if c == GAP {
        return;
    }
    let mut add = |kind: usize, ids: [usize; 3], len: usize| {
        totals[kind] += f;
        add_gram(raw, &Gram { ids, len, kind, f }, pos, geometry, 1.0);
    };
    add(0, [c as usize, 0, 0], 1);
    if b == GAP {
        return;
    }
    add(1, [b as usize, c as usize, 0], 2);
    if a == GAP {
        return;
    }
    add(2, [a as usize, c as usize, 0], 2);
    add(3, [a as usize, b as usize, c as usize], 3);
}

// Physical geometry never moves with bindings. Compile the disjoint event
// fields of a terminal history into one mask; retain floating travel values in
// Geometry. No frequency, binding, weight or candidate state lives here.
struct TailContributions {
    unary: Vec<u64>,
    pair: Vec<u64>,
    triple: Vec<u64>,
    n: usize,
}

impl TailContributions {
    fn new(g: &Geometry) -> Self {
        assert!(N_RAW <= 64);
        let travel = bit(TRAVEL) | bit(VTRAVEL) | bit(LTRAVEL) | bit(SFTRAVEL);
        // OR is exact only for disjoint fields. Fail at setup if future metric
        // definitions introduce multiplicity rather than silently losing it.
        let join = |left: u64, right: u64| {
            assert_eq!(left & right, 0, "overlapping tail contribution fields");
            assert_eq!((left | right) & travel, 0, "travel must remain numeric");
            left | right
        };
        let unary: Vec<u64> = g
            .keys
            .iter()
            .map(|key| {
                let off = if key.main && !key.home() {
                    bit(OFF + key.finger)
                } else {
                    0
                };
                join(bit(USAGE + key.finger), off)
            })
            .collect();
        let mut pair = Vec::with_capacity(g.n * g.n);
        for b in 0..g.n {
            for c in 0..g.n {
                let flags = g.pair[b * g.n + c];
                let bits = join(flags.bi, if flags.main { bit(SRAF_DEN) } else { 0 });
                pair.push(join(unary[c], bits));
            }
        }
        let mut triple = Vec::with_capacity(g.n * g.n * g.n);
        for a in 0..g.n {
            for b in 0..g.n {
                for c in 0..g.n {
                    let flags = g.tri[(a * g.n + b) * g.n + c];
                    let bits = join(flags.bits, if flags.main { bit(RHYTHM_DEN) } else { 0 });
                    triple.push(join(join(pair[b * g.n + c], g.pair[a * g.n + c].sk), bits));
                }
            }
        }
        Self {
            unary,
            pair,
            triple,
            n: g.n,
        }
    }

    fn apply(&self, raw: &mut Raw, totals: &mut [f64; 4], tail: Tail, f: f64, g: &Geometry) {
        let [a, b, c] = tail;
        if c == GAP {
            return;
        }
        let c = c as usize;
        totals[0] += f;
        // Preserve each field's arithmetic, including zero-valued travel terms.
        raw.0[TRAVEL] += g.home[c][0] * f;
        raw.0[VTRAVEL] += g.home[c][1] * f;
        raw.0[LTRAVEL] += g.home[c][2] * f;
        let bits = if b == GAP {
            self.unary[c]
        } else {
            let b = b as usize;
            let bc = b * self.n + c;
            totals[1] += f;
            raw.0[SFTRAVEL] += g.sf_distance[bc] * f;
            if a == GAP {
                self.pair[bc]
            } else {
                totals[2] += f;
                totals[3] += f;
                self.triple[a as usize * self.n * self.n + bc]
            }
        };
        add_bits(raw, bits, f);
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct CandidateScore {
    pub sfb: f64,
    pub sfs: f64,
    pub score: f64,
}

fn candidate_score(raw: &Raw, totals: &[f64; 4], w: &Weights) -> CandidateScore {
    // Keep breakdown's contribution and summation order, including separate
    // penalty/bonus sums. Do not reassociate floating-point arithmetic.
    let value = |m: usize| pct(raw.0[m].max(0.0), denominator_totals(m, raw, totals));
    let (mut penalty, mut bonus) = (0.0, 0.0);
    for m in 0..N_METRICS {
        let contribution = if aggregate(m) {
            0.0
        } else {
            value(m) * w.0[m] * if higher_better(m) { -1.0 } else { 1.0 }
        };
        if contribution >= 0.0 {
            penalty += contribution;
        } else {
            bonus -= contribution;
        }
    }
    for i in 0..4 {
        let a = pct(raw.0[OFF + i].max(0.0), totals[0]);
        let b = pct(raw.0[OFF + 7 - i].max(0.0), totals[0]);
        let contribution = (a + b) * w.0[N_METRICS + i];
        if contribution >= 0.0 {
            penalty += contribution;
        } else {
            bonus -= contribution;
        }
    }
    CandidateScore {
        sfb: value(SFB),
        sfs: value(SFS),
        score: penalty - bonus,
    }
}

// Reused across candidates. Only these numeric totals and tail patches are
// needed to accept a swap. No report, histogram or textual representation.
pub(crate) struct Proposal {
    raw: Raw,
    totals: [f64; 4],
    changes: Vec<(usize, Tail)>,
    affected: Vec<u32>,
    movement: Move,
    valid: bool,
    pub(crate) examined: usize,
    pub(crate) all_contexts: bool,
}

impl Proposal {
    pub(crate) fn new() -> Self {
        Self {
            raw: Raw::default(),
            totals: [0.0; 4],
            changes: Vec::new(),
            affected: Vec::new(),
            movement: Move::pair(0, 0),
            valid: false,
            examined: 0,
            all_contexts: false,
        }
    }

    pub(crate) fn score(&self, w: &Weights) -> CandidateScore {
        assert!(self.valid);
        candidate_score(&self.raw, &self.totals, w)
    }
    pub(crate) fn metrics(&self) -> Metrics {
        metrics_totals(&self.raw, &self.totals)
    }

    #[cfg(test)]
    pub(crate) fn summary(&self, w: &Weights) -> (Metrics, f64) {
        summary(&self.raw, self.totals, w)
    }
}

#[cfg(test)]
fn summary(raw: &Raw, totals: [f64; 4], w: &Weights) -> (Metrics, f64) {
    let corpus = Corpus {
        name: String::new(),
        grams: Vec::new(),
        by_symbol: Vec::new(),
        uni: Vec::new(),
        totals,
        coverage: [0.0; 4],
        warnings: Vec::new(),
        fingerprint: 0,
    };
    let m = metrics(raw, &corpus);
    let score = candidate_score(raw, &totals, w).score;
    (m, score)
}

// A checkpoint keeps only numeric totals. Tails are remapped when restoring a
// restart/local optimum, never cloned per candidate or archive entry.
#[derive(Clone)]
pub(crate) struct NumericState {
    raw: Raw,
    totals: [f64; 4],
    commits: usize,
}

impl NumericState {
    pub(crate) fn raw_totals(&self) -> (&Raw, [f64; 4]) {
        (&self.raw, self.totals)
    }

    pub(crate) fn metrics(&self) -> Metrics {
        metrics_totals(&self.raw, &self.totals)
    }
}

pub(crate) struct Incremental {
    corpus: NgramCorpus,
    tails: Vec<Tail>,
    postings: [Vec<u32>; 256],
    raw: Raw,
    totals: [f64; 4],
    geometry: Geometry,
    positions: Vec<usize>,
    arrangement: Vec<usize>,
    commits: usize,
    contributions: TailContributions,
    program: crate::action_fast::Program,
    keys: crate::action_fast::KeyState,
}

impl Incremental {
    pub(crate) fn new<F>(
        corpus: &NgramCorpus,
        layout: &ak::Layout,
        geometry: Geometry,
        stop: &AtomicBool,
        effort: F,
    ) -> ak::Result<Self>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        if layout.slots.len() >= GAP as usize || corpus.windows.len() > u32::MAX as usize {
            return Err("context cache exceeds index capacity".into());
        }
        let program = crate::action_fast::Program::new(layout, corpus.order)?;
        let keys = crate::action_fast::KeyState::new(&program);
        let contributions = TailContributions::new(&geometry);
        let mut cache = Self {
            corpus: corpus.clone(),
            tails: Vec::with_capacity(corpus.windows.len()),
            postings: std::array::from_fn(|_| Vec::new()),
            raw: Raw::default(),
            totals: [0.0; 4],
            geometry,
            positions: (0..layout.slots.len()).collect(),
            arrangement: (0..layout.slots.len()).collect(),
            commits: 0,
            program,
            keys,
            contributions,
        };
        let mut mapper = crate::action_fast::Mapper::new(&cache.program, &cache.keys);
        for (id, window) in corpus.windows.iter().enumerate() {
            if stop.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            let tail = terminal(mapper.map(window.text(), &effort)?, window.len);
            contribute(
                &mut cache.raw,
                &mut cache.totals,
                tail,
                window.weight,
                &cache.positions,
                &cache.geometry,
            );
            cache.tails.push(tail);
            for (j, &byte) in window.text().iter().enumerate() {
                if !window.text()[..j].contains(&byte) {
                    cache.postings[byte as usize].push(id as u32);
                }
            }
        }
        if cache.totals[0] <= 0.0 || cache.totals[1] <= 0.0 {
            return Err("layout has no usable letters/bigrams in this corpus".into());
        }
        drop(mapper);
        // Release compiled-state borrows before returning the owner.
        Ok(cache)
    }
    // Fill the reusable sorted union. Named-action moves can affect every
    // context, including ones that did not previously select that action.
    fn affected(&self, a: usize, b: usize, ids: &mut Vec<u32>) -> bool {
        ids.clear();
        let Some(bytes) = self.program.affected_bytes(&self.keys, a, b) else {
            return true;
        };
        let left: &[u32] = match bytes[0] {
            Some(byte) => self.postings[byte as usize].as_slice(),
            None => &[],
        };
        let right: &[u32] = match bytes[1] {
            Some(byte) => self.postings[byte as usize].as_slice(),
            None => &[],
        };
        ids.reserve(left.len() + right.len());
        let (mut i, mut j) = (0, 0);
        while i < left.len() && j < right.len() {
            if left[i] < right[j] {
                ids.push(left[i]);
                i += 1;
            } else if left[i] > right[j] {
                ids.push(right[j]);
                j += 1;
            } else {
                ids.push(left[i]);
                i += 1;
                j += 1;
            }
        }
        ids.extend_from_slice(&left[i..]);
        ids.extend_from_slice(&right[j..]);
        false
    }

    // For a three-key cycle, merge the three original binding dependencies.
    // The established pair path above is retained without extra work.
    fn affected_move(&self, movement: Move, ids: &mut Vec<u32>) -> bool {
        if movement.len == 2 {
            return self.affected(movement.slots[0], movement.slots[1], ids);
        }

        ids.clear();
        let [a, b, c] = movement.slots;
        let Some(ab) = self.program.affected_bytes(&self.keys, a, b) else {
            return true;
        };
        let Some(ac) = self.program.affected_bytes(&self.keys, a, c) else {
            return true;
        };
        let bytes = [ab[0], ab[1], ac[1]];
        let lists: [&[u32]; 3] = std::array::from_fn(|i| {
            bytes[i].map_or(&[][..], |byte| self.postings[byte as usize].as_slice())
        });
        ids.reserve(lists.iter().map(|list| list.len()).sum());
        let mut offsets = [0; 3];
        loop {
            let next = (0..3)
                .filter_map(|i| lists[i].get(offsets[i]).copied())
                .min();
            let Some(next) = next else {
                break;
            };
            ids.push(next);
            for i in 0..3 {
                if lists[i].get(offsets[i]) == Some(&next) {
                    offsets[i] += 1;
                }
            }
        }
        false
    }

    pub(crate) fn numeric_state(&self) -> NumericState {
        NumericState {
            raw: self.raw.clone(),
            totals: self.totals,
            commits: self.commits,
        }
    }

    // Restores outside the candidate loop. Program, geometry, effort tables and
    // posting lists stay allocated; only the permutation and mapped tails change.
    pub(crate) fn restore<F>(
        &mut self,
        arrangement: &[usize],
        saved: Option<&NumericState>,
        stop: &AtomicBool,
        effort: F,
    ) -> ak::Result<()>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        if !valid_arrangement(arrangement, self.arrangement.len()) {
            return Err("invalid action search permutation".into());
        }
        for slot in 0..arrangement.len() {
            if self.arrangement[slot] != arrangement[slot] {
                let other = self
                    .arrangement
                    .iter()
                    .position(|&id| id == arrangement[slot])
                    .unwrap();
                self.keys.swap(slot, other, &self.program);
                self.arrangement.swap(slot, other);
            }
        }

        let mut mapper = crate::action_fast::Mapper::new(&self.program, &self.keys);
        let mut raw = Raw::default();
        let mut totals = [0.0; 4];
        for (id, window) in self.corpus.windows.iter().enumerate() {
            if stop.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            let tail = terminal(mapper.map(window.text(), &effort)?, window.len);
            self.tails[id] = tail;
            if saved.is_none() {
                contribute(
                    &mut raw,
                    &mut totals,
                    tail,
                    window.weight,
                    &self.positions,
                    &self.geometry,
                );
            }
        }
        drop(mapper);

        if let Some(saved) = saved {
            self.raw.clone_from(&saved.raw);
            self.totals = saved.totals;
            self.commits = saved.commits;
        } else {
            self.raw = raw;
            self.totals = totals;
            self.commits = 0;
        }
        if self.totals[0] <= 0.0 || self.totals[1] <= 0.0 {
            return Err("layout has no usable letters/bigrams in this corpus".into());
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn physical_test_state(&self) -> (&Raw, [f64; 4], &[[u8; 3]]) {
        (&self.raw, self.totals, &self.tails)
    }

    pub(crate) fn context_count(&self) -> usize {
        self.tails.len()
    }

    pub(crate) fn propose_swap<F>(
        &mut self,
        a: usize,
        b: usize,
        stop: &AtomicBool,
        effort: F,
        proposal: &mut Proposal,
    ) -> ak::Result<()>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        self.propose_swap_profiled::<false, false, F>(
            a,
            b,
            stop,
            effort,
            proposal,
            &mut Profile::default(),
        )
    }

    pub(crate) fn propose_swap_profiled<const PROFILE: bool, const DETAIL: bool, F>(
        &mut self,
        a: usize,
        b: usize,
        stop: &AtomicBool,
        effort: F,
        proposal: &mut Proposal,
        profile: &mut Profile,
    ) -> ak::Result<()>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        self.propose_move_profiled::<PROFILE, DETAIL, F>(
            Move::pair(a, b),
            stop,
            effort,
            proposal,
            profile,
        )
    }

    pub(crate) fn propose_move_profiled<const PROFILE: bool, const DETAIL: bool, F>(
        &mut self,
        movement: Move,
        stop: &AtomicBool,
        effort: F,
        proposal: &mut Proposal,
        profile: &mut Profile,
    ) -> ak::Result<()>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        proposal.valid = false;
        let [a, b, c] = movement.slots;
        if !matches!(movement.len, 2 | 3)
            || a >= self.arrangement.len()
            || b >= self.arrangement.len()
            || a == b
            || movement.len == 3 && (c >= self.arrangement.len() || c == a || c == b)
        {
            return Err("invalid action search move".into());
        }

        proposal.changes.clear();
        proposal.raw.clone_from(&self.raw);
        proposal.totals = self.totals;
        proposal.movement = movement;
        let affected_start = clock::<PROFILE>();
        proposal.all_contexts = self.affected_move(movement, &mut proposal.affected);
        if PROFILE {
            profile.affected += elapsed::<PROFILE>(affected_start);
        }
        proposal.examined = if proposal.all_contexts {
            self.tails.len()
        } else {
            proposal.affected.len()
        };
        if PROFILE {
            profile.affected_total += proposal.examined as u64;
            profile.affected_max = profile.affected_max.max(proposal.examined as u64);
            profile.unaffected += (self.tails.len() - proposal.examined) as u64;
            if proposal.all_contexts {
                profile.full_scans += 1;
            }
        }
        if stop.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        proposal.changes.reserve(proposal.examined);
        // Reuse masks and permutation; only the moved positions change. Always
        // undo the numeric trial even on mapping error/cancellation.
        self.keys.swap(a, b, &self.program);
        if movement.len == 3 {
            self.keys.swap(b, c, &self.program);
        }
        let contexts_start = clock::<PROFILE>();
        let result = (|| -> ak::Result<()> {
            let mut mapper = crate::action_fast::Mapper::new(&self.program, &self.keys);
            for index in 0..proposal.examined {
                if stop.load(Ordering::Relaxed) {
                    return Err("cancelled".into());
                }
                let id = if proposal.all_contexts {
                    index
                } else {
                    proposal.affected[index] as usize
                };
                let window = &self.corpus.windows[id];
                let mapping_start = clock::<DETAIL>();
                let mapped =
                    mapper.map_profiled::<PROFILE, F>(window.text(), &effort, &mut profile.ops);
                if DETAIL {
                    profile.mapping += elapsed::<DETAIL>(mapping_start);
                }
                let tail = terminal(mapped?, window.len);
                if PROFILE {
                    profile.mapped += 1;
                }
                if tail != self.tails[id] {
                    let contribution_start = clock::<DETAIL>();
                    self.contributions.apply(
                        &mut proposal.raw,
                        &mut proposal.totals,
                        self.tails[id],
                        -window.weight,
                        &self.geometry,
                    );
                    self.contributions.apply(
                        &mut proposal.raw,
                        &mut proposal.totals,
                        tail,
                        window.weight,
                        &self.geometry,
                    );
                    proposal.changes.push((id, tail));
                    if DETAIL {
                        profile.contributions += elapsed::<DETAIL>(contribution_start);
                    }
                    if PROFILE {
                        profile.changed += 1;
                    }
                }
            }
            Ok(())
        })();
        if PROFILE {
            profile.contexts += elapsed::<PROFILE>(contexts_start);
        }
        if movement.len == 3 {
            self.keys.swap(b, c, &self.program);
        }
        self.keys.swap(a, b, &self.program);
        result?;
        if proposal.totals[0] <= 0.0 || proposal.totals[1] <= 0.0 {
            return Err("layout has no usable letters/bigrams in this corpus".into());
        }
        proposal.valid = true;
        Ok(())
    }

    pub(crate) fn commit(&mut self, proposal: &mut Proposal) {
        self.commit_profiled::<false>(proposal, &mut Profile::default());
    }

    pub(crate) fn commit_profiled<const PROFILE: bool>(
        &mut self,
        proposal: &mut Proposal,
        profile: &mut Profile,
    ) {
        let commit_start = clock::<PROFILE>();
        let mut rebase_time = Duration::ZERO;
        assert!(proposal.valid);
        proposal.valid = false;
        let [a, b, c] = proposal.movement.slots;
        self.keys.swap(a, b, &self.program);
        self.arrangement.swap(a, b);
        if proposal.movement.len == 3 {
            self.keys.swap(b, c, &self.program);
            self.arrangement.swap(b, c);
        }
        for &(id, tail) in &proposal.changes {
            self.tails[id] = tail;
        }
        self.raw.clone_from(&proposal.raw);
        self.totals = proposal.totals;
        self.commits += 1;
        // Preserve the existing periodic rebase and its accumulation order.
        if self.commits % 128 == 0 {
            let rebase_start = clock::<PROFILE>();
            self.raw = Raw::default();
            self.totals = [0.0; 4];
            for (window, &tail) in self.corpus.windows.iter().zip(&self.tails) {
                contribute(
                    &mut self.raw,
                    &mut self.totals,
                    tail,
                    window.weight,
                    &self.positions,
                    &self.geometry,
                );
            }
            if PROFILE {
                rebase_time = elapsed::<PROFILE>(rebase_start);
                profile.rebase += rebase_time;
                profile.rebases += 1;
            }
        }
        if PROFILE {
            profile.commit += elapsed::<PROFILE>(commit_start).saturating_sub(rebase_time);
        }
    }

    pub(crate) fn score(&self, w: &Weights) -> CandidateScore {
        candidate_score(&self.raw, &self.totals, w)
    }
    pub(crate) fn metrics(&self) -> Metrics {
        metrics_totals(&self.raw, &self.totals)
    }

    #[cfg(test)]
    pub(crate) fn summary(&self, w: &Weights) -> (Metrics, f64) {
        summary(&self.raw, self.totals, w)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn assert_contribution_bits(
        actual: &Raw,
        expected: &Raw,
        actual_totals: &[f64; 4],
        expected_totals: &[f64; 4],
    ) {
        for i in 0..N_RAW {
            assert_eq!(
                actual.0[i].to_bits(),
                expected.0[i].to_bits(),
                "raw field {i}"
            );
        }
        for i in 0..4 {
            assert_eq!(
                actual_totals[i].to_bits(),
                expected_totals[i].to_bits(),
                "total {i}"
            );
        }
    }

    fn contribution_geometry(wide: bool) -> Geometry {
        let mut keys = Vec::new();
        for row in 0..3 {
            if wide {
                for col in -1i8..=10 {
                    keys.push(main_key_at(row, col));
                }
            } else {
                for col in 0..10 {
                    keys.push(main_key(row, col));
                }
            }
        }
        keys.push(thumb_key(0));
        keys.push(thumb_key(1));
        Geometry::new(keys)
    }
    #[test]
    fn packed_contributions_match_original_for_every_tail() {
        for wide in [false, true] {
            let g = contribution_geometry(wide);
            let packed = TailContributions::new(&g);
            let pos: Vec<usize> = (0..g.n).collect();
            let ids: Vec<u8> = (0..g.n)
                .map(|k| k as u8)
                .chain(std::iter::once(GAP))
                .collect();
            // All key triples, repeats, thumbs, partial histories and internal
            // gaps. Nonzero seeds expose changes to floating accumulation.
            for &a in &ids {
                for &b in &ids {
                    for &c in &ids {
                        for f in [1.0, -1.0, 0.1, -0.1, 0.0, -0.0] {
                            let mut expected = Raw(std::array::from_fn(|i| {
                                if i % 2 == 0 {
                                    1234.125
                                } else {
                                    -0.0
                                }
                            }));
                            let mut actual = expected.clone();
                            let mut et = [17.25, 31.5, 2.75, 2.75];
                            let mut at = et;
                            contribute(&mut expected, &mut et, [a, b, c], f, &pos, &g);
                            packed.apply(&mut actual, &mut at, [a, b, c], f, &g);
                            assert_contribution_bits(&actual, &expected, &at, &et);
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn packed_tail_replacements_match_original_across_rebases() {
        // Exercise the 60-key table capacity independently of layout parsing.
        let keys = (0..58)
            .map(|i| main_key_at(i / 12, (i % 12) as i8 - 1))
            .chain([thumb_key(0), thumb_key(1)])
            .collect();
        let g = Geometry::new(keys);
        let packed = TailContributions::new(&g);
        let pos: Vec<usize> = (0..g.n).collect();
        let mut seed = 42u64;
        let mut next_tail = || -> Tail {
            std::array::from_fn(|_| {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                let id = ((seed >> 32) % (g.n as u64 + 1)) as usize;
                if id == g.n {
                    GAP
                } else {
                    id as u8
                }
            })
        };
        let mut tails: Vec<Tail> = (0..257).map(|_| next_tail()).collect();
        let weights: Vec<f64> = (0..tails.len()).map(|i| (i + 1) as f64 / 7.0).collect();
        let mut expected = Raw::default();
        let mut et = [0.0; 4];
        for (&tail, &f) in tails.iter().zip(&weights) {
            contribute(&mut expected, &mut et, tail, f, &pos, &g);
        }
        let mut actual = expected.clone();
        let mut at = et;
        for step in 0..4096 {
            let id = step % tails.len();
            let old = tails[id];
            let new = next_tail();
            let f = weights[id];
            contribute(&mut expected, &mut et, old, -f, &pos, &g);
            contribute(&mut expected, &mut et, new, f, &pos, &g);
            packed.apply(&mut actual, &mut at, old, -f, &g);
            packed.apply(&mut actual, &mut at, new, f, &g);
            tails[id] = new;
            assert_contribution_bits(&actual, &expected, &at, &et);
            if (step + 1) % 128 == 0 {
                // Production rebases retain the original contribution path.
                expected = Raw::default();
                et = [0.0; 4];
                for (&tail, &f) in tails.iter().zip(&weights) {
                    contribute(&mut expected, &mut et, tail, f, &pos, &g);
                }
                actual = expected.clone();
                at = et;
            }
        }
    }
    const BASE: &str =
        "q w e r t | y u i o p\na s d f g | h j k l ;\nz x c v b | n m , . /\nthumbs: space\n";
    fn plain() -> ak::Layout {
        ak::Layout::parse(BASE, Path::new("plain.dat")).unwrap()
    }

    fn magic() -> ak::Layout {
        ak::Layout::parse(
            crate::action_keys::test_layouts::SHORTHAND,
            Path::new("magic.dat"),
        )
        .unwrap()
    }

    #[test]
    fn three_key_moves_match_fresh_mapping_and_restore_numeric_checkpoints() {
        let seed = magic();
        let corpus = corpus(b"ii aa i' ai' qi! rrr rk aa zz abcdef", 5);
        let weights = Weights::default();
        let stop = AtomicBool::new(false);
        let effort = crate::action_ui::LocalEffort::new(&seed, &weights);
        let geometry = Geometry::new(crate::action_ui::physical_keys(&seed));
        let make = |layout: &ak::Layout| {
            Incremental::new(&corpus, layout, geometry.clone(), &stop, |a, b, key| {
                effort.get(a, b, key)
            })
            .unwrap()
        };
        let mut cache = make(&seed);
        let original = cache.numeric_state();
        let original_tails = cache.tails.clone();
        let identity: Vec<_> = (0..seed.slots.len()).collect();
        let mut proposal = Proposal::new();
        for labels in [["a", "i", "q"], ["@", "a", "r"], ["q", "@", "i"]] {
            let slots = labels.map(|label| key(&seed, label));
            let movement = Move { slots, len: 3 };
            cache
                .propose_move_profiled::<false, false, _>(
                    movement,
                    &stop,
                    |a, b, key| effort.get(a, b, key),
                    &mut proposal,
                    &mut Profile::default(),
                )
                .unwrap();
            assert_eq!(proposal.all_contexts, labels.contains(&"@"));
            let mut layout = seed.clone();
            layout.swap(slots[0], slots[1]);
            layout.swap(slots[1], slots[2]);
            let fresh = make(&layout);
            for (actual, expected) in proposal.metrics().v.iter().zip(fresh.metrics().v) {
                assert!((actual - expected).abs() < 1e-8);
            }
            cache.commit(&mut proposal);
            assert_eq!(cache.tails, fresh.tails);
            cache
                .restore(&identity, Some(&original), &stop, |a, b, key| {
                    effort.get(a, b, key)
                })
                .unwrap();
            assert_eq!(cache.tails, original_tails);
            assert_contribution_bits(&cache.raw, &original.raw, &cache.totals, &original.totals);
        }
    }

    #[test]
    fn restoring_checkpoint_restores_the_periodic_rebase_boundary() {
        let seed = magic();
        let corpus = corpus(b"aa ai' abc", 5);
        let stop = AtomicBool::new(false);
        let geometry = Geometry::new(crate::action_ui::physical_keys(&seed));
        let mut cache = Incremental::new(&corpus, &seed, geometry, &stop, |_, _, _| 0.0).unwrap();
        cache.commits = 127;
        let saved = cache.numeric_state();
        let identity: Vec<_> = (0..seed.slots.len()).collect();
        let mut proposal = Proposal::new();
        let mut profile = Profile::default();

        for expected_rebases in 1..=2 {
            cache
                .propose_swap(
                    key(&seed, "a"),
                    key(&seed, "i"),
                    &stop,
                    |_, _, _| 0.0,
                    &mut proposal,
                )
                .unwrap();
            cache.commit_profiled::<true>(&mut proposal, &mut profile);
            assert_eq!(cache.commits, 128);
            assert_eq!(profile.rebases, expected_rebases);
            cache
                .restore(&identity, Some(&saved), &stop, |_, _, _| 0.0)
                .unwrap();
            assert_eq!(cache.commits, 127);
        }
    }

    #[test]
    fn invalid_cycle_invalidates_proposal_and_mapping_error_undoes_keys() {
        let seed = magic();
        let corpus = corpus(b"aa ai' abc", 5);
        let stop = AtomicBool::new(false);
        let geometry = Geometry::new(crate::action_ui::physical_keys(&seed));
        let mut cache = Incremental::new(&corpus, &seed, geometry, &stop, |_, _, _| 0.0).unwrap();
        let mut proposal = Proposal::new();
        let a = key(&seed, "a");
        let i = key(&seed, "i");

        let original = cache.numeric_state();
        let original_tails = cache.tails.clone();
        let original_arrangement = cache.arrangement.clone();
        cache
            .propose_swap(a, i, &stop, |_, _, _| 0.0, &mut proposal)
            .unwrap();
        assert!(proposal.valid);
        assert!(cache
            .propose_swap(
                a,
                a,
                &stop,
                |_, _, _| panic!("a self-swap must be rejected before mapping"),
                &mut proposal,
            )
            .is_err());
        assert!(!proposal.valid);
        assert_eq!(cache.tails, original_tails);
        assert_eq!(cache.arrangement, original_arrangement);
        assert_eq!(cache.commits, original.commits);
        assert_contribution_bits(&cache.raw, &original.raw, &cache.totals, &original.totals);

        cache
            .propose_swap(a, i, &stop, |_, _, _| 0.0, &mut proposal)
            .unwrap();
        let invalid = Move {
            slots: [a, i, a],
            len: 3,
        };
        assert!(cache
            .propose_move_profiled::<false, false, _>(
                invalid,
                &stop,
                |_, _, _| 0.0,
                &mut proposal,
                &mut Profile::default(),
            )
            .is_err());
        assert!(!proposal.valid);

        let movement = Move {
            slots: [a, i, key(&seed, "@")],
            len: 3,
        };
        assert!(cache
            .propose_move_profiled::<false, false, _>(
                movement,
                &stop,
                |_, _, _| f64::NAN,
                &mut proposal,
                &mut Profile::default(),
            )
            .is_err());
        assert!(!proposal.valid);
        cache
            .propose_swap(a, i, &stop, |_, _, _| 0.0, &mut proposal)
            .unwrap();
        let mut fresh_layout = seed.clone();
        fresh_layout.swap(a, i);
        let fresh = Incremental::new(
            &corpus,
            &fresh_layout,
            Geometry::new(crate::action_ui::physical_keys(&seed)),
            &stop,
            |_, _, _| 0.0,
        )
        .unwrap();
        cache.commit(&mut proposal);
        assert_eq!(cache.tails, fresh.tails);
    }

    #[test]
    fn ngram_report_preserves_unicode_and_escaped_key_labels() {
        let text = format!("{BASE}outer-left: ~ ◇ ~\n◇ hr\n");
        let mut layout = ak::Layout::parse(&text, Path::new("inline.dat")).unwrap();
        let counts = evaluate(&corpus(b"hr", 3), &layout);

        layout.slots[0].label = "\"\\\n\t".into();
        let report = counts.report_json(&layout);
        let Json::Object(root) = parse_json(&report).unwrap() else {
            panic!("expected report object");
        };
        let Some(Json::Array(labels)) = root.get("keys") else {
            panic!("expected key labels");
        };

        assert_eq!(labels.len(), layout.slots.len());
        for (value, slot) in labels.iter().zip(&layout.slots) {
            let Json::String(label) = value else {
                panic!("expected string label");
            };
            assert_eq!(label, &slot.label);
        }
        assert!(report.contains("\"◇\""));
    }

    fn cache(text: &[u8], order: usize, weight: f64) -> String {
        let tables = ak::text_ngrams(text);
        let names = ["letters", "bigrams", "trigrams", "fourgrams", "fivegrams"];
        let mut out = String::from("{");
        for n in 0..order {
            if n > 0 {
                out.push(',');
            }
            out.push_str(&format!("\"{}\":{{", names[n]));
            for (i, (gram, f)) in tables[n].iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&format!("{}:{}", ak::quote(gram), *f as f64 * weight));
            }
            out.push('}');
        }
        out.push('}');
        out
    }

    fn corpus(text: &[u8], order: usize) -> NgramCorpus {
        NgramCorpus::from_text(&cache(text, order, 1.0), Path::new("cache.json")).unwrap()
    }

    fn evaluate(c: &NgramCorpus, l: &ak::Layout) -> Counts {
        c.evaluate(l, &AtomicBool::new(false), &AtomicU64::new(0), |_, _, _| {
            0.0
        })
        .unwrap()
    }

    fn key(l: &ak::Layout, label: &str) -> usize {
        l.slots.iter().position(|s| s.label == label).unwrap()
    }
    #[test]
    fn plain_counts_equal_direct_counts_for_every_order() {
        let l = plain();
        let text = b"a\nab\nabc\nabcd\nabcdeabcde\nabc!def\n";
        for order in 3..=5 {
            let counts = evaluate(&corpus(text, order), &l);
            let mut expected: [Table; 3] = std::array::from_fn(|_| Table::new());
            let mut skip = Table::new();
            for run in text.split(|b| *b == b'\n' || *b == b'!') {
                let ids: Vec<_> = run
                    .iter()
                    .map(|b| key(&l, &(*b as char).to_string()))
                    .collect();
                for n in 1..=3 {
                    for gram in ids.windows(n) {
                        *expected[n - 1].entry(gram.to_vec()).or_default() += 1.0;
                    }
                }
                for gram in ids.windows(3) {
                    *skip.entry(vec![gram[0], gram[2]]).or_default() += 1.0;
                }
            }
            assert_eq!(counts.tables, expected);
            assert_eq!(counts.skip, skip);
            assert_eq!(counts.ignored_characters, 1.0);
            assert_eq!(counts.presses, 26.0);
        }
    }
    #[test]
    fn overlapping_windows_do_not_multiply_frequencies() {
        let l = plain();
        let c = NgramCorpus::from_text(
            &cache(b"abcdef\n", 5, 1_000_000.0),
            Path::new("weighted.json"),
        )
        .unwrap();
        assert_eq!(c.windows.len(), 6);
        let counts = evaluate(&c, &l);
        assert_eq!(counts.characters, 6_000_000.0);
        assert_eq!(counts.tables[1].values().sum::<f64>(), 5_000_000.0);
        assert_eq!(counts.tables[2].values().sum::<f64>(), 4_000_000.0);
    }
    #[test]
    fn missing_characters_are_gaps_even_if_magic_can_sometimes_emit_them() {
        let l = magic();
        let counts = evaluate(&corpus(b"' it's so delicious.", 5), &l);
        assert_eq!(counts.characters, 17.0);
        assert_eq!(counts.ignored_characters, 3.0);
        let counts = evaluate(&corpus(b"it'a", 5), &l);
        assert_eq!(counts.tables[1].values().sum::<f64>(), 1.0);
        assert!(counts.tables[2].is_empty());
        assert!(counts.skip.is_empty());
    }
    #[test]
    fn magic_only_character_is_typed_with_its_rule() {
        let l = magic();
        let counts = evaluate(&corpus(b"i'!i'", 5), &l);
        assert_eq!(counts.action_presses, 2.0);
        assert_eq!(counts.presses, 4.0);
        assert_eq!(counts.ignored_characters, 1.0);
        assert_eq!(
            counts.tables[1].get(&vec![key(&l, "i"), key(&l, "@")]),
            Some(&2.0)
        );
        assert!(counts.skip.is_empty());
    }
    #[test]
    fn local_effort_can_choose_repeat_over_same_key() {
        let l = magic();
        let c = corpus(b"aa", 3);
        let magic_key = key(&l, "@");
        let counts = c
            .evaluate(
                &l,
                &AtomicBool::new(false),
                &AtomicU64::new(0),
                |_, last, next| if last == Some(next) { 10.0 } else { 0.0 },
            )
            .unwrap();
        assert_eq!(counts.action_presses, 1.0);
        assert_eq!(
            counts.tables[1].get(&vec![key(&l, "a"), magic_key]),
            Some(&1.0)
        );
    }
    #[test]
    fn prefix_reuse_matches_fresh_mapping() {
        let l = magic();
        let mut cached = ak::WindowMapper::new(&l, 5).unwrap();
        let alphabet = b"ai'!";
        for number in 0..1024 {
            let mut n = number;
            let mut text = [0; 5];
            for b in text.iter_mut().rev() {
                *b = alphabet[n % 4];
                n /= 4;
            }
            let mut fresh = ak::WindowMapper::new(&l, 5).unwrap();
            for len in [5, 3, 1, 4, 2] {
                let a = cached.map(&text[..len], &|_, _, _| 0.0).unwrap();
                let b = fresh.map(&text[..len], &|_, _, _| 0.0).unwrap();
                assert_eq!(&a[..len], &b[..len]);
            }
        }
    }
    #[test]
    fn cache_without_raw_or_seq_is_sufficient() {
        let c = NgramCorpus::from_text(
            &cache(b"i' abc", 4, 1.0),
            Path::new("/does-not-exist/corpus.json"),
        )
        .unwrap();
        assert!(evaluate(&c, &magic()).presses > 0.0);
        assert!(NgramCorpus::load(Path::new("corpus.txt"))
            .unwrap_err()
            .contains("cached"));
        assert!(NgramCorpus::load(Path::new("corpus.seq")).is_err());
    }
    #[test]
    fn inconsistent_orders_are_rejected() {
        let json = r#"{"letters":{"a":1},"bigrams":{"aa":2},"trigrams":{}}"#;
        assert!(NgramCorpus::from_text(json, Path::new("bad.json"))
            .unwrap_err()
            .contains("inconsistent"));
    }
    #[test]
    fn cancellation_is_honored() {
        let c = corpus(b"abc", 5);
        assert_eq!(
            c.evaluate(
                &plain(),
                &AtomicBool::new(true),
                &AtomicU64::new(0),
                |_, _, _| 0.0
            )
            .unwrap_err(),
            "cancelled"
        );
    }
    #[test]
    fn longer_rules_require_sufficient_context_and_macros_are_explicitly_rejected() {
        let l = ak::Layout::parse(
            &format!("{BASE}outer-left: ~ @m ~\naction m = magic\nmap m \"abc\" = \"d\"\n"),
            Path::new("long.dat"),
        )
        .unwrap();
        assert!(ak::WindowMapper::new(&l, 3).is_err());
        assert!(ak::WindowMapper::new(&l, 5).is_ok());
        let l = ak::Layout::parse(
            &format!("{BASE}outer-left: ~ @m ~\naction m = text \"the\"\n"),
            Path::new("macro.dat"),
        )
        .unwrap();
        assert!(ak::WindowMapper::new(&l, 5).is_err());
    }
    #[test]
    fn candidate_score_matches_report_arithmetic_for_identical_totals() {
        let mut state = 41u64;
        for trial in 0..500 {
            let mut next = || {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                ((state >> 32) % 10000) as f64 * 0.03125
            };
            let raw = Raw(std::array::from_fn(|_| next() - 10.0));
            let totals = if trial % 7 == 0 {
                [0.0; 4]
            } else {
                std::array::from_fn(|_| next() + 1.0)
            };
            let weights = Weights::new(std::array::from_fn(|_| next() * 0.01 - 0.5));
            let corpus = Corpus {
                name: String::new(),
                grams: Vec::new(),
                by_symbol: Vec::new(),
                uni: Vec::new(),
                totals,
                coverage: [0.0; 4],
                warnings: Vec::new(),
                fingerprint: 0,
            };
            let reference = metrics(&raw, &corpus);
            let fast = candidate_score(&raw, &totals, &weights);
            assert_eq!(
                fast.score.to_bits(),
                breakdown(&reference, &weights).net.to_bits()
            );
            assert_eq!(fast.sfb.to_bits(), reference.v[SFB].to_bits());
            assert_eq!(fast.sfs.to_bits(), reference.v[SFS].to_bits());
            // Preserve the existing cap and improvement comparisons even at boundaries.
            for cap in [fast.sfb, fast.sfb - 1e-10, fast.sfb + 1e-10] {
                assert_eq!(fast.sfb <= cap + 1e-10, reference.v[SFB] <= cap + 1e-10);
            }
        }
    }
}

#[cfg(test)]
mod detailed_mapping_tests {
    use super::*;
    use std::cell::RefCell;

    const BASE: &str =
        "q w e r t | y u i o p\na s d f g | h j k l ;\nz x c v b | n m , . /\nthumbs: space\n";

    fn contexts(order: usize) -> NgramCorpus {
        let mut texts = BTreeSet::new();
        for text in [
            b"qqqqu".as_slice(),
            b"qu!qu",
            b"q qu",
            b"i'i'a",
            b"hrhnr",
            b"thqeq",
            b"h!nr",
            b"aa\0aa",
            b"u'u'a",
            b"qquaa",
        ] {
            for len in 1..=text.len().min(order) {
                texts.insert(text[..len].to_vec());
            }
        }
        let mut seed = 29u64;
        let alphabet = b"quaei' !\0";
        for _ in 0..120 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let len = 1 + (seed >> 32) as usize % order;
            let mut text = Vec::new();
            for _ in 0..len {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                text.push(alphabet[(seed >> 32) as usize % alphabet.len()]);
            }
            texts.insert(text);
        }
        let windows: Vec<_> = texts
            .into_iter()
            .enumerate()
            .map(|(index, text)| {
                let mut bytes = [0; 5];
                bytes[..text.len()].copy_from_slice(&text);
                Window {
                    bytes,
                    len: text.len(),
                    weight: [0.1, 1.0 / 3.0, 7.0, 1_000_000.25][index % 4],
                }
            })
            .collect();

        NgramCorpus {
            name: "inline".into(),
            order,
            warnings: Vec::new(),
            windows: windows.into(),
        }
    }

    fn assert_counts_bits(actual: &Counts, expected: &Counts) {
        assert_eq!(actual.order, expected.order);
        for (a, b) in [
            (&actual.tables[0], &expected.tables[0]),
            (&actual.tables[1], &expected.tables[1]),
            (&actual.tables[2], &expected.tables[2]),
            (&actual.skip, &expected.skip),
        ] {
            assert_eq!(a.len(), b.len());
            for ((a_key, a_count), (b_key, b_count)) in a.iter().zip(b) {
                assert_eq!(a_key, b_key);
                assert_eq!(
                    a_count.to_bits(),
                    b_count.to_bits(),
                    "physical keys {a_key:?}"
                );
            }
        }
        for (a, b) in [
            (actual.characters, expected.characters),
            (actual.presses, expected.presses),
            (actual.action_presses, expected.action_presses),
            (actual.ignored_characters, expected.ignored_characters),
        ] {
            assert_eq!(a.to_bits(), b.to_bits());
        }
    }

    #[test]
    fn detailed_numeric_counts_and_effort_order_match_reference() {
        // Inline fixtures cover literals, repeat-output, repeat-action, every
        // rule basis, aliases, nested calls, explicit none, and longer suffixes.
        for source in crate::action_fast::tests::fixtures() {
            for order in 3..=5 {
                let corpus = contexts(order);
                let mut layout = ak::Layout::parse(&source, Path::new("inline.dat")).unwrap();
                for swap in [
                    None,
                    Some((0, 1)),
                    Some((2, layout.slots.len() - 1)),
                    Some((0, 20)),
                ] {
                    if let Some((a, b)) = swap {
                        layout.swap(a, b);
                    }
                    for policy in 0..3 {
                        let actual_calls = RefCell::new(Vec::new());
                        let reference_calls = RefCell::new(Vec::new());
                        let effort =
                            |previous: Option<usize>, last: Option<usize>, key: usize| match policy
                            {
                                0 => 0.0,
                                1 => {
                                    key as f64 * 0.01
                                        + last.map_or(0.0, |old| if old == key { 2.0 } else { 0.0 })
                                        + previous
                                            .map_or(0.0, |old| if old == key { 1.0 } else { 0.0 })
                                }
                                _ => {
                                    if matches!(layout.slots[key].binding, ak::Binding::Named(_)) {
                                        -1.0
                                    } else {
                                        0.0
                                    }
                                }
                            };
                        let actual_progress = AtomicU64::new(0);
                        let reference_progress = AtomicU64::new(0);
                        let stop = AtomicBool::new(false);
                        let actual = corpus
                            .evaluate(&layout, &stop, &actual_progress, |a, b, k| {
                                actual_calls.borrow_mut().push((a, b, k));
                                effort(a, b, k)
                            })
                            .unwrap();
                        let expected = corpus
                            .evaluate_reference(&layout, &stop, &reference_progress, |a, b, k| {
                                reference_calls.borrow_mut().push((a, b, k));
                                effort(a, b, k)
                            })
                            .unwrap();

                        assert_counts_bits(&actual, &expected);
                        assert_eq!(*actual_calls.borrow(), *reference_calls.borrow());
                        assert_eq!(actual_progress.load(Ordering::Relaxed), 100);
                        assert_eq!(reference_progress.load(Ordering::Relaxed), 100);
                    }
                }
            }
        }
    }

    #[test]
    fn detailed_numeric_errors_cancellation_and_capacity_match_reference() {
        let corpus = contexts(3);
        let stop = AtomicBool::new(false);
        let progress = AtomicU64::new(0);
        for suffix in [
            "outer-left: ~ @m ~\naction m = text \"ab\"\n",
            "outer-left: ~ @m ~\naction m = magic\nmap m \"abc\" = \"d\"\n",
        ] {
            let layout =
                ak::Layout::parse(&format!("{BASE}{suffix}"), Path::new("inline.dat")).unwrap();
            assert_eq!(
                corpus
                    .evaluate(&layout, &stop, &progress, |_, _, _| 0.0)
                    .unwrap_err(),
                corpus
                    .evaluate_reference(&layout, &stop, &progress, |_, _, _| 0.0)
                    .unwrap_err(),
            );
        }
        let mut layout = ak::Layout::parse(BASE, Path::new("inline.dat")).unwrap();
        for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                corpus
                    .evaluate(&layout, &stop, &progress, |_, _, _| invalid)
                    .unwrap_err(),
                corpus
                    .evaluate_reference(&layout, &stop, &progress, |_, _, _| invalid)
                    .unwrap_err(),
            );
        }
        let cancelled = AtomicBool::new(true);
        assert_eq!(
            corpus
                .evaluate(&layout, &cancelled, &progress, |_, _, _| 0.0)
                .unwrap_err(),
            corpus
                .evaluate_reference(&layout, &cancelled, &progress, |_, _, _| 0.0)
                .unwrap_err(),
        );

        // Preserve valid programmatically built layouts beyond the numeric cap.
        while layout.slots.len() <= 60 {
            layout.slots.push(layout.slots[0].clone());
        }
        let actual = corpus
            .evaluate(&layout, &stop, &progress, |_, _, key| -(key as f64))
            .unwrap();
        let expected = corpus
            .evaluate_reference(&layout, &stop, &progress, |_, _, key| -(key as f64))
            .unwrap();
        assert_counts_bits(&actual, &expected);
    }

    #[test]
    fn detailed_terminal_depth_limits_match_reference() {
        let corpus = contexts(5);
        let stop = AtomicBool::new(false);
        let progress = AtomicU64::new(0);
        for depth in [30, 31, 32, 33] {
            let mut source = format!("{BASE}outer-left: ~ @m0 ~\n");
            for id in 0..depth {
                if id + 1 == depth {
                    source.push_str(&format!("action m{id} = repeat-output\n"));
                } else {
                    // Preserve the bound root's delegation; text-magic helper
                    // actions remain unbound and keep their own fallbacks.
                    let basis = if id == 0 { "output-magic" } else { "magic" };
                    source.push_str(&format!(
                        "action m{id} = {basis}\nfallback m{id} = @m{}\n",
                        id + 1
                    ));
                }
            }
            let layout = ak::Layout::parse(&source, Path::new("inline.dat")).unwrap();
            let effort = |_: Option<usize>, _: Option<usize>, key: usize| {
                if matches!(layout.slots[key].binding, ak::Binding::Named(_)) {
                    -1.0
                } else {
                    0.0
                }
            };
            let actual = corpus.evaluate(&layout, &stop, &progress, &effort).unwrap();
            let expected = corpus
                .evaluate_reference(&layout, &stop, &progress, &effort)
                .unwrap();
            assert_counts_bits(&actual, &expected);
        }
    }
}

#[cfg(test)]
mod limit_tests {
    use super::*;

    fn text_tables(text: &[u8]) -> [BTreeMap<Vec<u8>, f64>; 5] {
        let counts = ak::text_ngrams(text);
        std::array::from_fn(|i| {
            counts[i]
                .iter()
                .map(|(gram, count)| (gram.clone(), *count as f64))
                .collect()
        })
    }

    fn cached_text(text: &[u8]) -> String {
        let names = ["letters", "bigrams", "trigrams", "fourgrams", "fivegrams"];
        let tables = text_tables(text);
        let fields: Vec<_> = tables
            .iter()
            .zip(names)
            .map(|(table, name)| {
                let grams: Vec<_> = table
                    .iter()
                    .map(|(gram, count)| format!("{}:{count}", ak::quote(gram)))
                    .collect();
                format!("\"{name}\":{{{}}}", grams.join(","))
            })
            .collect();
        format!("{{{}}}", fields.join(","))
    }

    fn window_bits(corpus: &NgramCorpus) -> Vec<([u8; 5], usize, u64)> {
        corpus
            .windows
            .iter()
            .map(|window| (window.bytes, window.len, window.weight.to_bits()))
            .collect()
    }

    #[test]
    fn unlimited_and_unreached_limits_preserve_context_bits_and_order() {
        let text = cached_text(b"abcabcabc\nxbcxbc\nybc\n");
        let original = NgramCorpus::from_text(&text, Path::new("inline.json")).unwrap();
        for limits in [
            NgramLimits::default(),
            NgramLimits {
                trigrams: Some(2000),
                tetragrams: Some(2000),
                pentagrams: Some(2000),
            },
        ] {
            let actual =
                NgramCorpus::from_text_with_limits(&text, Path::new("inline.json"), limits)
                    .unwrap();
            assert_eq!(window_bits(&actual), window_bits(&original));
            assert!(actual.warnings.is_empty());
            assert_eq!(actual.order, original.order);
        }
    }

    #[test]
    fn each_cap_is_a_hard_limit_and_higher_contexts_keep_their_suffix() {
        let original = text_tables(b"abcabcabc\nxbcxbc\nybc\n");
        let stop = AtomicBool::new(false);
        for caps in [
            [None, None, Some(1), None, None],
            [None, None, None, Some(1), None],
            [None, None, None, None, Some(1)],
            [None, None, Some(2), Some(1), Some(1)],
        ] {
            let mut tables = original.clone();
            let warnings = limit_context_tables(&mut tables, 5, caps, &stop).unwrap();
            assert_eq!(tables[..2], original[..2]);
            assert!(!warnings.is_empty());
            for i in 2..5 {
                if let Some(cap) = caps[i] {
                    assert!(tables[i].len() <= cap);
                }
                for gram in tables[i].keys() {
                    assert!(tables[i - 1].contains_key(&gram[1..]));
                }
            }
            let windows = weighted_contexts(&tables, 5, &stop, true).unwrap();
            let actual: f64 = windows.iter().map(|window| window.weight).sum();
            let expected: f64 = original[0].values().sum();
            assert_eq!(actual.to_bits(), expected.to_bits());
        }
    }

    #[test]
    fn ties_use_lexical_order_and_can_empty_higher_tables() {
        let text = cached_text(b"abc\nxbcde\n");
        let limits = NgramLimits {
            trigrams: Some(1),
            ..NgramLimits::default()
        };
        let corpus =
            NgramCorpus::from_text_with_limits(&text, Path::new("inline.json"), limits).unwrap();
        assert_eq!(corpus.order, 5);
        assert!(corpus
            .windows
            .iter()
            .filter(|window| window.len >= 3)
            .all(|window| window.text() == b"abc"));
        assert!(corpus
            .warnings
            .iter()
            .any(|warning| warning.contains("5-grams: 0/1")));
        assert_eq!(
            corpus
                .windows
                .iter()
                .map(|window| window.weight)
                .sum::<f64>(),
            8.0
        );
    }

    #[test]
    fn limited_contexts_preserve_literal_pairs_and_reset_at_gaps() {
        let text = cached_text(b"ab\tcdab\tcd\nabcdeabcde\n");
        let limits = NgramLimits {
            trigrams: Some(1),
            tetragrams: Some(1),
            pentagrams: Some(1),
        };
        let full = NgramCorpus::from_text(&text, Path::new("inline.json")).unwrap();
        let limited =
            NgramCorpus::from_text_with_limits(&text, Path::new("inline.json"), limits).unwrap();
        let layout = ak::Layout::parse(
            "q w e r t | y u i o p\na s d f g | h j k l ;\nz x c v b | n m , . /\nthumbs: space\n",
            Path::new("inline.dat"),
        )
        .unwrap();
        let stop = AtomicBool::new(false);
        let progress = AtomicU64::new(0);
        let effort = |_: Option<usize>, _: Option<usize>, _: usize| 0.0;
        let full_counts = full.evaluate(&layout, &stop, &progress, effort).unwrap();
        let limited_counts = limited.evaluate(&layout, &stop, &progress, effort).unwrap();

        assert_eq!(limited_counts.tables[..2], full_counts.tables[..2]);
        assert_eq!(limited_counts.ignored_characters, 2.0);
        assert_eq!(limited_counts.presses, full_counts.presses);
        assert_eq!(
            limited_counts.ignored_characters,
            full_counts.ignored_characters
        );
        assert!(limited
            .windows
            .iter()
            .any(|window| window.text().contains(&0)));
    }

    #[test]
    fn limits_do_not_hide_invalid_original_frequencies_or_missing_suffixes() {
        let limits = NgramLimits {
            trigrams: Some(1),
            ..NgramLimits::default()
        };
        let text = cached_text(b"abcabc\nxbc\n");
        let inconsistent = text.replace("\"xbc\":1", "\"xbc\":100");
        assert_ne!(text, inconsistent);
        let error =
            NgramCorpus::from_text_with_limits(&inconsistent, Path::new("inline.json"), limits)
                .unwrap_err();
        assert!(error.contains("inconsistent"));

        let missing = text.replace("\"xbc\":1", "\"xyz\":1");
        let error = NgramCorpus::from_text_with_limits(&missing, Path::new("inline.json"), limits)
            .unwrap_err();
        assert!(error.contains("suffix missing"));
    }
}
