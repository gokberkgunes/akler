//! Cached n-gram evaluation. No raw corpus or ordered-text sidecar is opened here.
//! Each weighted context contributes only its final physical event, avoiding
//! overlap double-counting. Magic history is bounded by the selected table order.
use crate::action_keys as ak;
use crate::action_profile::{clock, elapsed, Profile};
use crate::*;

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
    pub fn write_report(&self, layout: &ak::Layout, path: &Path) -> ak::Result<()> {
        let mut out=format!("{{\n  \"kind\": \"physical-keystroke-ngram-estimate\",\n  \"context_order\": {},\n  \"policy\": \"greedy local effort within each cached context; literal wins ties\",\n  \"keys\": [",self.order);
        for (i, slot) in layout.slots.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&ak::quote(slot.label.as_bytes()));
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
#[derive(Clone, Debug)]
pub struct NgramCorpus {
    pub name: String,
    pub order: usize,
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
        let result = Self::parse(&text, path, stop, progress).map_err(|e| e.to_string());
        timing.mark("Corpus parse/preparation (nested report)");
        result
    }
    pub fn from_text(text: &str, path: &Path) -> ak::Result<Self> {
        Self::parse(text, path, &AtomicBool::new(false), &AtomicU64::new(0))
            .map_err(|e| e.to_string())
    }
    fn parse(text: &str, path: &Path, stop: &AtomicBool, progress: &AtomicU64) -> AppResult<Self> {
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
                if weight > 0.0 {
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
                return Err("N-gram table contains a suffix missing from its lower order; rebuild the corpus.".into());
            }
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
        let mut mapper = ak::WindowMapper::new(layout, self.order)?;
        timing.mark("Validate and initialize reference mapper");
        let mut counts = Counts::new(self.order);
        for (i, window) in self.windows.iter().enumerate() {
            if stop.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            let keys = mapper.map(window.text(), &effort)?;
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
    swap: (usize, usize),
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
            swap: (0, 0),
            valid: false,
            examined: 0,
            all_contexts: false,
        }
    }
    pub(crate) fn score(&self, w: &Weights) -> CandidateScore {
        assert!(self.valid);
        candidate_score(&self.raw, &self.totals, w)
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
pub(crate) struct Incremental {
    corpus: NgramCorpus,
    tails: Vec<Tail>,
    postings: [Vec<u32>; 256],
    raw: Raw,
    totals: [f64; 4],
    geometry: Geometry,
    positions: Vec<usize>,
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
        drop(mapper); // Release compiled-state borrows before returning the owner.
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
        proposal.valid = false;
        proposal.changes.clear();
        proposal.raw.clone_from(&self.raw);
        proposal.totals = self.totals;
        proposal.swap = (a, b);
        let affected_start = clock::<PROFILE>();
        proposal.all_contexts = self.affected(a, b, &mut proposal.affected);
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
        // Reuse masks and permutation; only two positions change. Always undo
        // the numeric trial even on mapping error/cancellation.
        self.keys.swap(a, b, &self.program);
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
        self.keys
            .swap(proposal.swap.0, proposal.swap.1, &self.program);
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
            include_str!("../examples/magic-shorthand.dat"),
            Path::new("magic.dat"),
        )
        .unwrap()
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
            let weights = Weights(std::array::from_fn(|_| next() * 0.01 - 0.5));
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
