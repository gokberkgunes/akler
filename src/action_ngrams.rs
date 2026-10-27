//! Cached n-gram evaluation. No raw corpus or ordered-text sidecar is opened here.
//! Each weighted context contributes only its counting anchor: the final
//! character normally, or a centered position with future context for chords.
//! Mapping and magic history remain bounded by the selected table order.
use crate::action_keys as ak;

use crate::*;

use crate::action_profile::{clock, elapsed, Profile};

pub(crate) type Table = BTreeMap<Slots, f64>;

/// A count table kept as its ascending entries until something reads it as a
/// map. Optimizer evaluations only total it (`entries`), in the order the map
/// would iterate, so building the map is skipped without changing any sum.
#[derive(Clone, Default)]
pub(crate) struct LazyTable {
    sorted: Vec<(Slots, f64)>,
    table: std::sync::OnceLock<Table>,
}

impl LazyTable {
    /// Entries in ascending key order, without building the map.
    pub(crate) fn entries(&self) -> impl Iterator<Item = (&Slots, f64)> + '_ {
        let (table, sorted) = match self.table.get() {
            Some(table) => (Some(table.iter()), None),
            None => (None, Some(self.sorted.iter())),
        };
        table
            .into_iter()
            .flatten()
            .map(|(key, &value)| (key, value))
            .chain(
                sorted
                    .into_iter()
                    .flatten()
                    .map(|(key, value)| (key, *value)),
            )
    }
}

impl From<Table> for LazyTable {
    fn from(table: Table) -> Self {
        Self {
            sorted: Vec::new(),
            table: table.into(),
        }
    }
}

impl std::ops::Deref for LazyTable {
    type Target = Table;

    fn deref(&self) -> &Table {
        self.table
            .get_or_init(|| self.sorted.iter().copied().collect())
    }
}

impl std::ops::DerefMut for LazyTable {
    fn deref_mut(&mut self) -> &mut Table {
        let _ = &**self;
        self.sorted = Vec::new();
        self.table.get_mut().unwrap()
    }
}

impl<'a> IntoIterator for &'a LazyTable {
    type Item = (&'a Slots, &'a f64);
    type IntoIter = std::collections::btree_map::Iter<'a, Slots, f64>;

    fn into_iter(self) -> Self::IntoIter {
        (**self).iter()
    }
}

impl PartialEq for LazyTable {
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}

impl PartialEq<Table> for LazyTable {
    fn eq(&self, other: &Table) -> bool {
        **self == *other
    }
}

impl std::fmt::Debug for LazyTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        (**self).fmt(f)
    }
}

/// A physical pattern's one to three slots, stored inline. It behaves as
/// the slice of its slots: compared, ordered, hashed and printed as one, so
/// a table keyed by it orders exactly as by `Vec<usize>`.
#[derive(Clone, Copy)]
pub(crate) struct Slots {
    len: u8,
    ids: [usize; 3],
}

impl Slots {
    pub(crate) fn new(slots: &[usize]) -> Self {
        let mut ids = [0; 3];
        ids[..slots.len()].copy_from_slice(slots);
        Self {
            len: slots.len() as u8,
            ids,
        }
    }
}

impl std::ops::Deref for Slots {
    type Target = [usize];

    fn deref(&self) -> &[usize] {
        &self.ids[..usize::from(self.len)]
    }
}

impl std::borrow::Borrow<[usize]> for Slots {
    fn borrow(&self) -> &[usize] {
        self
    }
}

impl PartialEq for Slots {
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}

impl Eq for Slots {}

impl PartialOrd for Slots {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Slots {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (**self).cmp(&**other)
    }
}

impl std::hash::Hash for Slots {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (**self).hash(state);
    }
}

impl std::fmt::Debug for Slots {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        (**self).fmt(f)
    }
}

impl From<Vec<usize>> for Slots {
    fn from(slots: Vec<usize>) -> Self {
        Self::new(&slots)
    }
}
type SourceGram = [u8; 5];
type TableBytes = BTreeMap<SourceGram, f64>;
// Original normalized counts in compact, sorted order. Keeping these avoids
// reparsing corpus JSON when the first chord layout needs boundary counts.
type ChordSource = [Vec<(SourceGram, f64)>; 5];

fn compact_chord_source(tables: &[TableBytes; 5]) -> ChordSource {
    std::array::from_fn(|order| {
        tables[order]
            .iter()
            .map(|(gram, frequency)| (*gram, *frequency))
            .collect()
    })
}

const NO_GROUP: u32 = u32::MAX;

/// Static contexts whose tails are typed by the same bindings, so they share
/// one tail under every arrangement.
#[derive(Clone, Debug)]
struct TailGroup {
    /// The bindings typing the tail, `UNTYPED` for none.
    bindings: [u8; 3],
    tail: Tail,
    /// The frequency of members whose fingers are their keys' own.
    default_weight: f64,
    /// Members with contextual fingers, and all members, ascending.
    contextual: Vec<u32>,
    members: Vec<u32>,
}

/// `default_finger_tail` on per-slot fingers.
fn default_fingers_of(default_fingers: &[usize], tail: Tail, fingers: Tail) -> bool {
    tail.iter().zip(&fingers).all(|(&slot, &finger)| {
        slot == GAP || finger != GAP && default_fingers[usize::from(slot)] == usize::from(finger)
    })
}

/// A finger tail after its keys moved without any rule's position taking
/// part: a moved key is matched by no rule and takes its new slot's finger.
fn moved_fingers(fingers: Tail, old: Tail, new: Tail, default_fingers: &[usize]) -> Tail {
    let mut moved = fingers;
    for ((finger, &slot), old) in moved.iter_mut().zip(&new).zip(old) {
        if slot != old {
            *finger = if slot == GAP {
                GAP
            } else {
                default_fingers[usize::from(slot)] as u8
            };
        }
    }
    moved
}

/// A cheap hasher for integer keys (Fx-style multiply-rotate).
#[derive(Default, Clone, Copy)]
struct FastHasher(u64);

impl std::hash::Hasher for FastHasher {
    fn finish(&self) -> u64 {
        // The multiply mixes upward only; fold its high half into the low
        // bits the table indexes by.
        self.0 ^ self.0 >> 32
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.write_u64(u64::from(byte));
        }
    }

    fn write_u64(&mut self, value: u64) {
        self.0 = (self.0.rotate_left(5) ^ value).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
}

type FastHash = std::hash::BuildHasherDefault<FastHasher>;

/// Up to five bytes as one key; tables keyed by it hold one gram length.
fn pack_gram(bytes: &[u8]) -> u64 {
    let mut packed = [0; 8];
    packed[..bytes.len()].copy_from_slice(bytes);
    u64::from_le_bytes(packed)
}

fn gram_slice(bytes: &[u8]) -> SourceGram {
    let mut gram = [0; 5];
    gram[..bytes.len()].copy_from_slice(bytes);
    gram
}

/// A physical slot sequence together with the fingers actually used for that
/// occurrence.  The ordinary physical tables remain slot-only because they
/// feed physical-key reports; contextual alternate
/// fingerings must not merge there by label or emitted text.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct FingerPattern {
    pub(crate) slots: Vec<usize>,
    pub(crate) fingers: Vec<usize>,
}

pub(crate) type FingerTable = BTreeMap<FingerPattern, f64>;

// Bound the cubic allocation. Unusually large programmatic layouts keep the
// ordinary sparse tables; each dense bin records presence separately so a
// zero-weight occurrence still produces a report entry.
const DENSE_COUNT_SLOTS: usize = 64;

enum PhysicalHistogram {
    Dense {
        slots: usize,
        width: usize,
        bins: DenseBins,
        touched: Vec<u32>,
    },
    Sparse(Table),
}

/// Zeroed dense bins. Evaluations run thousands of times per search, so the
/// buffers are recycled instead of allocated and cleared whole; `into_table`
/// resets only the bins it touched before returning them.
#[derive(Default)]
struct DenseBins {
    values: Vec<f64>,
    seen: Vec<bool>,
}

/// The recycled buffers, shared by all threads: fitters evaluate on
/// short-lived worker threads, whose own pools would always start empty.
static DENSE_BINS: std::sync::Mutex<Vec<DenseBins>> = std::sync::Mutex::new(Vec::new());
/// Buffers the pool keeps; each evaluation in flight holds four.
const DENSE_BINS_KEPT: usize = 256;

impl DenseBins {
    /// A zeroed buffer of `len` bins. Pooled buffers are reused only at their
    /// own size: growing a smaller one would zero the new bins every time.
    fn take(len: usize) -> Self {
        let reused = {
            let mut pool = DENSE_BINS.lock().unwrap();
            pool.iter()
                .position(|bins| bins.values.len() == len)
                .map(|index| pool.swap_remove(index))
        };
        reused.unwrap_or_else(|| Self {
            values: vec![0.0; len],
            seen: vec![false; len],
        })
    }

    fn give_back(self) {
        let mut pool = DENSE_BINS.lock().unwrap();
        if pool.len() < DENSE_BINS_KEPT {
            pool.push(self);
        }
    }
}

// Copies come from the pool too, so `into_table` returning them keeps it
// bounded. Pooled buffers are zeroed, so only the touched bins are copied.
impl Clone for PhysicalHistogram {
    fn clone(&self) -> Self {
        match self {
            Self::Dense {
                slots,
                width,
                bins,
                touched,
            } => {
                let mut copy = DenseBins::take(bins.values.len());
                for &bin in touched {
                    let bin = bin as usize;
                    copy.values[bin] = bins.values[bin];
                    copy.seen[bin] = true;
                }
                Self::Dense {
                    slots: *slots,
                    width: *width,
                    bins: copy,
                    touched: touched.clone(),
                }
            }
            Self::Sparse(table) => Self::Sparse(table.clone()),
        }
    }
}

impl PhysicalHistogram {
    fn new(slots: usize, width: usize, dense: bool) -> Self {
        if dense && slots <= DENSE_COUNT_SLOTS {
            Self::Dense {
                slots,
                width,
                bins: DenseBins::take(slots.pow(width as u32)),
                touched: Vec::new(),
            }
        } else {
            Self::Sparse(Table::new())
        }
    }

    /// Sort the touched bins, as `into_table` would.
    fn sort_touched(&mut self) {
        if let Self::Dense { touched, .. } = self {
            touched.sort_unstable();
        }
    }

    fn add(&mut self, keys: &[usize], frequency: f64) {
        match self {
            Self::Dense {
                slots,
                bins,
                touched,
                ..
            } => {
                let index = keys.iter().fold(0, |index, &key| index * *slots + key);
                bins.values[index] += frequency;
                if !bins.seen[index] {
                    bins.seen[index] = true;
                    touched.push(index as u32);
                }
            }
            Self::Sparse(table) => *table.entry(Slots::new(keys)).or_default() += frequency,
        }
    }

    /// The counted patterns. One whose frequencies cancel to zero counts
    /// nothing and is left out, so a count updated window by window builds
    /// the same table as a fresh one.
    fn into_table(self) -> LazyTable {
        match self {
            Self::Sparse(mut table) => {
                table.retain(|_, frequency| *frequency != 0.0);
                table.into()
            }
            Self::Dense {
                slots,
                width,
                mut bins,
                mut touched,
            } => {
                // Ascending bin order is ascending key order, which keeps
                // the map's bulk build cheap. Recorded counts keep theirs
                // sorted (`sort_touched`), so an update appends only a short
                // run to merge.
                touched.sort();
                let mut sorted = Vec::with_capacity(touched.len());
                sorted.extend(touched.iter().filter_map(|&bin| {
                    let bin = bin as usize;
                    let value = std::mem::take(&mut bins.values[bin]);
                    bins.seen[bin] = false;
                    if value == 0.0 {
                        return None;
                    }
                    let mut index = bin;
                    let mut keys = [0; 3];
                    for key in keys[..width].iter_mut().rev() {
                        *key = index % slots;
                        index /= slots;
                    }
                    Some((Slots::new(&keys[..width]), value))
                }));
                bins.give_back();
                LazyTable {
                    sorted,
                    table: std::sync::OnceLock::new(),
                }
            }
        }
    }
}

// Slots and fingers pack into one key, slots first, so ascending keys are
// ascending (slots, fingers): the order of the table this builds.
#[derive(Clone, Default)]
struct FingerHistogram(NetTable);

impl FingerHistogram {
    fn add(&mut self, slots: &[usize], fingers: &[usize], frequency: f64) {
        let mut key = 0u64;
        for index in 0..3 {
            key = key << 8 | slots.get(index).map_or(0, |&slot| slot as u64);
        }
        for index in 0..3 {
            key = key << 8 | fingers.get(index).map_or(0, |&finger| finger as u64);
        }
        self.0.add(key, frequency);
    }

    /// As `PhysicalHistogram::into_table`, without zero patterns.
    fn into_table(mut self, width: usize) -> FingerTable {
        let mut entries: Vec<_> = self.0.drain().filter(|&(_, f)| f != 0.0).collect();
        entries.sort_unstable_by_key(|&(key, _)| key);
        let byte = |key: u64, index: usize| (key >> (40 - 8 * index)) as u8 as usize;
        entries
            .into_iter()
            .map(|(key, frequency)| {
                (
                    FingerPattern {
                        slots: (0..width).map(|index| byte(key, index)).collect(),
                        fingers: (0..width).map(|index| byte(key, 3 + index)).collect(),
                    },
                    frequency,
                )
            })
            .collect()
    }
}

#[derive(Clone, Debug)]
pub struct Counts {
    pub tables: [LazyTable; 3],
    pub skip: LazyTable,
    pub(crate) finger_tables: Option<[FingerTable; 3]>,
    pub(crate) finger_skip: Option<FingerTable>,
    pub characters: f64,
    pub presses: f64,
    pub action_presses: f64,
    pub ignored_characters: f64,
    pub order: usize,
    /// The evaluated corpus's `speed_populations`.
    pub(crate) speed_populations: [f64; 2],
}

impl Counts {
    fn new(order: usize) -> Self {
        Self {
            tables: std::array::from_fn(|_| LazyTable::default()),
            skip: LazyTable::default(),
            finger_tables: None,
            finger_skip: None,
            characters: 0.0,
            presses: 0.0,
            action_presses: 0.0,
            ignored_characters: 0.0,
            order,
            speed_populations: [0.0; 2],
        }
    }

    fn report_json(&self, layout: &ak::Layout) -> String {
        let mut out = format!(
            "{{\n  \"kind\": \"physical-keystroke-ngram-estimate\",\n  \"context_order\": {},\n  \"policy\": \"safe magic before literal; action collisions with the other recent finger use local effort; following same-finger literals and alternative actions use two-press local effort; then greedy local effort within each cached context\",\n  \"keys\": [",
            self.order
        );
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

fn read_ngram_table<'a>(
    p: &mut JsonParser<'a>,
    width: usize,
    table: Option<&mut TableBytes>,
    stop: &AtomicBool,
    progress: &AtomicU64,
) -> AppResult<()> {
    p.ws();
    p.expect(b'{')?;
    p.ws();
    let mut seen = std::collections::HashSet::<_, FastHash>::default();
    let mut entries = Vec::new();
    if p.byte() == Some(b'}') {
        p.p += 1;
        return Ok(());
    }
    loop {
        if stop.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        p.ws();
        let name = p.string_cow()?;
        p.ws();
        p.expect(b':')?;
        let count = match p.value(0)? {
            Json::Number(f) if f.is_finite() && f >= 0.0 => f,
            _ => return Err("n-gram count must be finite and non-negative".into()),
        };
        let mut key = [0; 5];
        let mut chars = name.chars();
        for byte in &mut key[..width] {
            let Some(c) = chars.next() else {
                return Err(format!("invalid {width}-gram {name:?}").into());
            };
            // Non-ASCII/control characters retain one position as a gap.
            *byte = if c.is_ascii() && !c.is_ascii_control() {
                c.to_ascii_lowercase() as u8
            } else {
                0
            };
        }
        if chars.next().is_some() {
            return Err(format!("invalid {width}-gram {name:?}").into());
        }
        if table.is_some() {
            if !seen.insert(name.clone()) {
                return Err(format!("duplicate n-gram {name:?}").into());
            }
            entries.push((key, count));
        }
        progress.store((p.p * 90 / p.text.len().max(1)) as u64, Ordering::Relaxed);
        p.ws();
        if p.byte() == Some(b'}') {
            p.p += 1;
            break;
        }
        p.expect(b',')?;
    }
    if let Some(table) = table {
        // Each corpus order starts with an empty table. Stable sorting keeps
        // the source addition order for uppercase and Unicode-gap collisions.
        debug_assert!(table.is_empty());
        entries.sort_by_key(|(key, _)| *key);
        let mut merged: Vec<([u8; 5], f64)> = Vec::with_capacity(entries.len());
        for (key, count) in entries {
            if let Some((previous, total)) = merged.last_mut() {
                if *previous == key {
                    *total += count;
                    continue;
                }
            }
            // Keep the original 0.0 + first-count operation, including signed zero.
            merged.push((key, 0.0 + count));
        }
        *table = merged.into_iter().collect();
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct Window {
    bytes: [u8; 5],
    len: u8,
    target: u8,
    left_open: bool,
    weight: f64,
}

impl Window {
    fn len(&self) -> usize {
        self.len as usize
    }

    fn target(&self) -> usize {
        self.target as usize
    }

    fn end(&self) -> usize {
        self.target() + 1
    }

    fn text(&self) -> &[u8] {
        &self.bytes[..self.len()]
    }
}

fn weighted_contexts(
    tables: &[TableBytes; 5],
    order: usize,
    stop: &AtomicBool,
    collect: bool,
) -> AppResult<Vec<Window>> {
    let mut windows = Vec::new();
    for n in 1..=order {
        // Each suffix's total over its stored left extensions, summed in the
        // extensions' table order, sorted like the shorter table.
        let mut extended: Vec<(SourceGram, f64)> = Vec::new();
        if n < order {
            let mut suffixes = Vec::with_capacity(tables[n].len());
            for (gram, f) in &tables[n] {
                if stop.load(Ordering::Relaxed) {
                    return Err("cancelled".into());
                }
                suffixes.push((gram_slice(&gram[1..=n]), *f));
            }
            suffixes.sort_by_key(|(gram, _)| gram_order(gram));
            for (gram, f) in suffixes {
                match extended.last_mut() {
                    Some((previous, total)) if *previous == gram => *total += f,
                    _ => extended.push((gram, 0.0 + f)),
                }
            }
        }

        // Only occurrences with no stored left extension belong to the
        // shorter context. The rest are counted in a longer context.
        let mut next = 0;
        let mut missing = false;
        for (gram, f) in &tables[n - 1] {
            if stop.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            while next < extended.len() && extended[next].0 < *gram {
                missing |= extended[next].1 > 1e-8;
                next += 1;
            }
            let covered = if next < extended.len() && extended[next].0 == *gram {
                next += 1;
                extended[next - 1].1
            } else {
                0.0
            };
            let tol = 1e-8 * f.abs().max(1.0);
            if covered > *f + tol {
                return Err("N-gram orders have inconsistent frequencies; rebuild them together (raw counts, not separately normalized percentages).".into());
            }
            let weight = (*f - covered).max(0.0);
            if collect && weight > 0.0 {
                let mut bytes = [0; 5];
                bytes[..n].copy_from_slice(&gram[..n]);
                windows.push(Window {
                    bytes,
                    len: n as u8,
                    target: (n - 1) as u8,
                    left_open: false,
                    weight,
                });
            }
        }
        if missing || extended[next..].iter().any(|(_, f)| *f > 1e-8) {
            return Err(
                "N-gram table contains a suffix missing from its lower order; rebuild the corpus."
                    .into(),
            );
        }
    }
    Ok(windows)
}

/// A gram as one integer with the same order as the gram itself.
fn gram_order(gram: &SourceGram) -> u64 {
    let [a, b, c, d, e] = *gram;
    u64::from_be_bytes([0, 0, 0, a, b, c, d, e])
}

fn limit_context_tables(
    tables: &mut [TableBytes; 5],
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
        // Rebuilt from sorted entries rather than removed one by one. Grams
        // compare as `gram_order` integers, which order them identically.
        let suffixes: Vec<u64> = lower[i - 1].keys().map(gram_order).collect();
        let mut entries: Vec<(SourceGram, f64)> = std::mem::take(table)
            .into_iter()
            .filter(|(gram, _)| {
                suffixes
                    .binary_search(&gram_order(&gram_slice(&gram[1..=i])))
                    .is_ok()
            })
            .collect();
        if let Some(cap) = caps[i] {
            if entries.len() > cap {
                let selected =
                    top_ngram_keys(entries.iter().map(|(gram, f)| (gram_order(gram), *f)), cap);
                let mut selected = selected.into_iter().peekable();
                entries.retain(|(gram, _)| selected.next_if_eq(&gram_order(gram)).is_some());
            }
        }
        *table = entries.into_iter().collect();

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

/// Whether every window weight is a whole number, small enough that all sums
/// of weights and of their chord shares (multiples of one half, at most
/// `CHORD_TAILS` per window) are exact in any order.
fn exact_sums_of(windows: &[Window]) -> bool {
    let mut total = 0.0;
    windows.iter().all(|window| {
        total += window.weight;
        window.weight == window.weight.trunc()
    }) && total < (1_u64 << 44) as f64
}

/// Window indices, ascending, by each byte and each pair of adjacent bytes
/// of their text, every window once under each.
#[derive(Debug)]
struct WindowIndex {
    bytes: Box<[Vec<u32>]>,
    /// Windows by pair `first << 8 | second` are
    /// `pair_ids[pair_starts[pair]..pair_starts[pair + 1]]`.
    pair_starts: Box<[u32]>,
    pair_ids: Box<[u32]>,
}

impl WindowIndex {
    fn new(windows: &[Window]) -> Self {
        let mut bytes = vec![Vec::new(); 256];
        let mut pair_starts = vec![0u32; (1 << 16) + 1];
        // A window's distinct adjacent pairs.
        let pairs = |window: &Window| {
            let text = window.text();
            let mut pairs = [0usize; 4];
            let mut len = 0;
            for at in 1..text.len() {
                let pair = usize::from(text[at - 1]) << 8 | usize::from(text[at]);
                if !pairs[..len].contains(&pair) {
                    pairs[len] = pair;
                    len += 1;
                }
            }
            (pairs, len)
        };
        for (index, window) in windows.iter().enumerate() {
            for (at, &byte) in window.text().iter().enumerate() {
                if !window.text()[..at].contains(&byte) {
                    bytes[usize::from(byte)].push(index as u32);
                }
            }
            let (pairs, len) = pairs(window);
            for &pair in &pairs[..len] {
                pair_starts[pair + 1] += 1;
            }
        }
        for pair in 0..1 << 16 {
            pair_starts[pair + 1] += pair_starts[pair];
        }
        let mut next = pair_starts.clone();
        let mut pair_ids = vec![0; pair_starts[1 << 16] as usize];
        for (index, window) in windows.iter().enumerate() {
            let (pairs, len) = pairs(window);
            for &pair in &pairs[..len] {
                pair_ids[next[pair] as usize] = index as u32;
                next[pair] += 1;
            }
        }
        Self {
            bytes: bytes.into(),
            pair_starts: pair_starts.into(),
            pair_ids: pair_ids.into(),
        }
    }

    fn pair(&self, [first, second]: [u8; 2]) -> &[u32] {
        let pair = usize::from(first) << 8 | usize::from(second);
        &self.pair_ids[self.pair_starts[pair] as usize..self.pair_starts[pair + 1] as usize]
    }
}

type Postings = Arc<std::sync::OnceLock<WindowIndex>>;

#[derive(Clone, Debug)]
pub struct NgramCorpus {
    pub name: String,
    pub(crate) path: PathBuf,
    pub(crate) include_spacegrams: bool,
    pub order: usize,
    pub warnings: Vec<String>,
    windows: Arc<[Window]>,
    /// `exact_sums_of(windows)`.
    exact_sums: bool,
    // Retain compact original counts only when caps remove context. Chord
    // boundaries cannot be reconstructed from left-suffix backoff alone.
    chord_source: Option<Arc<ChordSource>>,
    /// Windows by the bytes and pairs of their text, built on first use.
    postings: Postings,
    /// The chord windows, their `exact_sums_of` and postings.
    chord_windows: Arc<std::sync::OnceLock<ak::Result<(Arc<[Window]>, bool, Postings)>>>,
    /// `rule_candidates`, built on first use.
    rule_candidates: Arc<std::sync::OnceLock<Vec<(bool, u8, u8, f64)>>>,
    /// Whole-corpus bigram and skipgram masses, measured as ordinary corpora
    /// measure them, for Fspeed normalization. Zero without stored tables.
    pub(crate) speed_populations: [f64; 2],
}

/// Mana2 stats for action and magic layouts. Fspeed is normalized by the
/// whole-corpus populations, as for ordinary layouts and on akl.gg; other
/// stats use the mapped totals. Missing populations fall back to them.
pub(crate) fn mana_stats(
    raw: &mana2_metrics::RawStats,
    totals: [f64; 4],
    speed_populations: [f64; 2],
) -> mana2_metrics::Stats {
    let population = |index: usize| {
        let stored = speed_populations[index - 1];
        if stored > 0.0 {
            stored
        } else {
            totals[index]
        }
    };
    mana2_metrics::stats_with_speed_totals(
        raw,
        totals,
        [totals[0], population(1), population(2), totals[3]],
    )
}

impl NgramCorpus {
    /// Source-text contexts anchored at each window's final counted character.
    /// Short boundary windows contribute only the history they actually retain.
    pub(crate) fn rule_candidates(&self) -> &[(bool, u8, u8, f64)] {
        self.rule_candidates
            .get_or_init(|| self.count_rule_candidates())
    }

    fn count_rule_candidates(&self) -> Vec<(bool, u8, u8, f64)> {
        let mut totals = BTreeMap::new();
        for window in self.windows.iter() {
            let text = window.text();
            let n = text.len();
            if n < 2 {
                continue;
            }
            let target = text[n - 1];
            if !(32..=126).contains(&target) {
                continue;
            }
            for (skip, distance) in [(false, 2), (true, 3)] {
                if n >= distance {
                    let context = text[n - distance];
                    if (32..=126).contains(&context) {
                        *totals.entry((skip, context, target)).or_insert(0.0) += window.weight;
                    }
                }
            }
        }
        totals
            .into_iter()
            .map(|((skip, context, target), frequency)| (skip, context, target, frequency))
            .collect()
    }

    /// Centred contexts: up to two characters before the counted one, for
    /// trigram statistics, and one after, to recognize a two-key combo that
    /// starts on it. Missing left/right extensions are corpus boundaries.
    pub(crate) fn chord_contexts(&self) -> ak::Result<Self> {
        if self.order < 4 {
            return Err("Combo chord analysis requires cached n-grams through order 4; rebuild the corpus through order 4 or disable combos.".into());
        }
        let windows = self
            .chord_windows
            .get_or_init(|| {
                self.build_chord_windows().map(|windows| {
                    (
                        windows.clone(),
                        exact_sums_of(&windows),
                        Postings::default(),
                    )
                })
            })
            .clone()?;
        let mut result = self.clone();
        (result.windows, result.exact_sums, result.postings) = windows;
        result.rule_candidates = Arc::default();
        Ok(result)
    }

    fn context_tables(&self) -> [TableBytes; 5] {
        let mut tables: [TableBytes; 5] = std::array::from_fn(|_| BTreeMap::new());
        for window in self.windows.iter() {
            for n in 1..=window.len() {
                *tables[n - 1]
                    .entry(gram_slice(&window.text()[window.len() - n..]))
                    .or_default() += window.weight;
            }
        }
        tables
    }

    /// Centred chord contexts with exact counts. Under n-gram caps, a context
    /// the capped tables did not keep gives its count to a shorter one, first
    /// without its following character (a combo starting on the counted one
    /// then goes unseen), then without its earliest one (its start becomes
    /// left-open), so every counted character keeps its full frequency.
    fn build_chord_windows(&self) -> ak::Result<Arc<[Window]>> {
        // Contexts the capped tables kept, by length.
        let retained = self.chord_source.as_ref().map(|_| {
            let mut retained: [std::collections::HashSet<u64>; 5] = Default::default();
            for window in self.windows.iter() {
                for n in 1..=window.len() {
                    retained[n - 1].insert(pack_gram(&window.text()[window.len() - n..]));
                }
            }
            retained
        });
        let uncapped;
        let source = if let Some(source) = &self.chord_source {
            source.as_ref()
        } else {
            uncapped = compact_chord_source(&self.context_tables());
            &uncapped
        };
        // Two characters before the counted one feed trigram statistics; one
        // after shows whether a two-key combo starts on it. A third before,
        // when the corpus has it, settles whether the first of those two
        // finishes a combo (`ChordPlan::movement` drops an unsettled one).
        let left = if self.order >= 5 { 3 } else { 2 };
        let right = 1;
        // Masses by packed gram; each sums in source order, as a sorted map
        // would.
        let mut left_mass: [NetTable; 5] = Default::default();
        let mut right_mass: [NetTable; 5] = Default::default();
        let mut both_mass: [NetTable; 5] = Default::default();
        for n in 1..=left + right {
            for (gram, f) in &source[n] {
                left_mass[n - 1].add(pack_gram(&gram[1..=n]), *f);
                right_mass[n - 1].add(pack_gram(&gram[..n]), *f);
            }
            if n < left + right {
                for (gram, f) in &source[n + 1] {
                    both_mass[n - 1].add(pack_gram(&gram[1..=n]), *f);
                }
            }
        }
        // (length, counted position, left-open, bytes) -> count; every key
        // occurs once, so the map is built from the sorted entries.
        let mut entries: Vec<((u8, u8, bool, SourceGram), f64)> = Vec::new();
        for l in 0..=left {
            for r in 0..=right {
                let n = l + r + 1;
                for (bytes, f) in &source[n - 1] {
                    let packed = pack_gram(&bytes[..n]);
                    let weight =
                        f - if l < left {
                            left_mass[n - 1].get(packed).unwrap_or(0.0)
                        } else {
                            0.0
                        } - if r < right {
                            right_mass[n - 1].get(packed).unwrap_or(0.0)
                        } else {
                            0.0
                        } + if l < left && r < right {
                            both_mass[n - 1].get(packed).unwrap_or(0.0)
                        } else {
                            0.0
                        };
                    if weight < -1e-8 * f.abs().max(1.0) {
                        return Err("Combo chords require consistent left/right n-gram contexts; rebuild the corpus with all n-gram limits set to all.".into());
                    }
                    // Left-open hides a first character a combo begun earlier
                    // may cover; three characters before the counted one keep
                    // it out of every statistic, so none is hidden then.
                    let left_open = l == left && l < 3;
                    if weight > 0.0 {
                        entries.push(((n as u8, l as u8, left_open, *bytes), weight));
                    }
                }
            }
        }
        // Per length, counts by packed (counted position, left-open, bytes),
        // whose numeric order is the tuple order.
        let pack = |target: u8, left_open: bool, bytes: &SourceGram| {
            bytes.iter().fold(
                u64::from(target) << 1 | u64::from(left_open),
                |key, &byte| key << 8 | u64::from(byte),
            )
        };
        let unpack = |key: u64| {
            let mut bytes = [0; 5];
            for (index, byte) in bytes.iter_mut().enumerate() {
                *byte = (key >> (32 - 8 * index)) as u8;
            }
            ((key >> 41) as u8, key >> 40 & 1 != 0, bytes)
        };
        let mut levels: Vec<HashMap<u64, f64, FastHash>> =
            (0..=left + right + 1).map(|_| HashMap::default()).collect();
        for ((n, target, left_open, bytes), weight) in entries {
            levels[usize::from(n)].insert(pack(target, left_open, &bytes), weight);
        }
        if let Some(tables) = &retained {
            for n in (2..=left + right + 1).rev() {
                // Capped contexts give their counts to shorter ones, in key
                // order, so each shorter count sums in a fixed order.
                let mut dropped: Vec<(u64, f64)> = levels[n]
                    .iter()
                    .filter(|&(&key, _)| !tables[n - 1].contains(&pack_gram(&unpack(key).2[..n])))
                    .map(|(&key, &weight)| (key, weight))
                    .collect();
                dropped.sort_unstable_by_key(|&(key, _)| key);
                for (key, weight) in dropped {
                    levels[n].remove(&key);
                    let (target, left_open, bytes) = unpack(key);
                    let target = usize::from(target);
                    let shorter = if target + 1 < n {
                        pack(target as u8, left_open, &gram_slice(&bytes[..n - 1]))
                    } else {
                        pack(target as u8 - 1, true, &gram_slice(&bytes[1..n]))
                    };
                    *levels[n - 1].entry(shorter).or_default() += weight;
                }
            }
        }
        let mut windows = Vec::new();
        for (n, level) in levels.into_iter().enumerate() {
            let mut level: Vec<_> = level.into_iter().collect();
            level.sort_unstable_by_key(|&(key, _)| key);
            windows.extend(level.into_iter().map(|(key, weight)| {
                let (target, left_open, bytes) = unpack(key);
                Window {
                    bytes,
                    len: n as u8,
                    target,
                    left_open,
                    weight,
                }
            }));
        }
        Ok(windows.into())
    }

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
        let mut tables: [TableBytes; 5] = std::array::from_fn(|_| BTreeMap::new());
        let mut skipgrams: Option<TableBytes> = None;
        let mut got = [false; 5];
        let mut fields = std::collections::HashSet::new();
        loop {
            if stop.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            p.ws();
            if p.byte() == Some(b'}') {
                p.p += 1;
                break;
            }
            let name = p.string_cow()?;
            if !fields.insert(name.clone()) {
                return Err(format!("duplicate corpus field {name}").into());
            }
            p.ws();
            p.expect(b':')?;
            let order = match name.as_ref() {
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
                read_ngram_table(&mut p, n, Some(&mut tables[n - 1]), stop, progress)?;
                if stop.load(Ordering::Relaxed) {
                    return Err("cancelled".into());
                }
            } else if name.as_ref() == "skipgrams" {
                read_ngram_table(
                    &mut p,
                    2,
                    Some(skipgrams.insert(BTreeMap::new())),
                    stop,
                    progress,
                )?;
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
        // Spaces separate contexts. Removing only entries that contain a
        // literal space preserves post-space shorter contexts without joining
        // text across the boundary.
        if !limits.include_spacegrams {
            for table in &mut tables {
                table.retain(|gram, _| !gram.contains(&b' '));
            }
        }
        // Ordinary corpora use the stored skip table, or filtered trigram
        // endpoints when it is absent or spacegrams are disabled; neither is
        // capped. Measure both populations before caps remove entries.
        let skip_mass = match &skipgrams {
            Some(skip) if limits.include_spacegrams => skip.values().sum::<f64>(),
            _ => tables[2].values().sum::<f64>(),
        };
        let speed_populations = [tables[1].values().sum::<f64>(), skip_mass];
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
        let chord_source = limited.then(|| Arc::new(compact_chord_source(&tables)));
        if limited {
            warnings = limit_context_tables(&mut tables, order, caps, stop)?;
            windows = weighted_contexts(&tables, order, stop, true)?;
            warnings.push("Removed contexts use retained shorter suffixes without chords; chord analysis omits removed contexts. Action history and physical metrics are approximate.".into());
        }
        if windows.is_empty() {
            return Err("no usable cached n-grams".into());
        }
        if !limits.include_spacegrams {
            warnings.push("Spacegrams disabled by configuration.".into());
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
            path: path.to_owned(),
            include_spacegrams: limits.include_spacegrams,
            order,
            warnings,
            exact_sums: exact_sums_of(&windows),
            windows: windows.into(),
            postings: Postings::default(),
            chord_source,
            chord_windows: Arc::new(std::sync::OnceLock::new()),
            rule_candidates: Arc::default(),
            speed_populations,
        })
    }

    #[cfg(test)]
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
        self.evaluate_with_fingers(
            layout,
            stop,
            progress,
            crate::alt_fingers::AltPolicy::layout(true),
            effort,
        )
    }

    #[cfg(test)]
    pub(crate) fn evaluate_with_fingers<F>(
        &self,
        layout: &ak::Layout,
        stop: &AtomicBool,
        progress: &AtomicU64,
        alt_fingers: crate::alt_fingers::AltPolicy,
        effort: F,
    ) -> ak::Result<Counts>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        self.evaluate_with_policy(layout, stop, progress, alt_fingers, true, effort)
    }

    pub(crate) fn evaluate_with_policy<F>(
        &self,
        layout: &ak::Layout,
        stop: &AtomicBool,
        progress: &AtomicU64,
        alt_fingers: crate::alt_fingers::AltPolicy,
        combos: impl Into<crate::combos::Policy>,
        effort: F,
    ) -> ak::Result<Counts>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        self.evaluate_policy_impl(layout, stop, progress, alt_fingers, combos, false, effort)
    }

    /// Opt in only for a pure effort function whose values and two-press sums
    /// are finite. The generic entry point preserves every callback invocation.
    pub(crate) fn evaluate_with_finite_policy<F>(
        &self,
        layout: &ak::Layout,
        stop: &AtomicBool,
        progress: &AtomicU64,
        alt_fingers: crate::alt_fingers::AltPolicy,
        combos: impl Into<crate::combos::Policy>,
        effort: F,
    ) -> ak::Result<Counts>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        self.evaluate_policy_impl(layout, stop, progress, alt_fingers, combos, true, effort)
    }

    fn evaluate_policy_impl<F>(
        &self,
        layout: &ak::Layout,
        stop: &AtomicBool,
        progress: &AtomicU64,
        alt_fingers: crate::alt_fingers::AltPolicy,
        combos: impl Into<crate::combos::Policy>,
        finite: bool,
        effort: F,
    ) -> ak::Result<Counts>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        let policy: crate::combos::Policy = combos.into();
        self.evaluate_reusing(
            layout,
            stop,
            progress,
            (alt_fingers, policy, finite),
            None,
            false,
            false,
            effort,
        )
        .map(|(counts, _)| counts)
    }

    /// `evaluate_with_(finite_)policy` with `flags` = (alt_fingers, combos,
    /// finite). Windows `base` proves unchanged reuse its mapped keys
    /// (see `Mappings::reuse`); `record` returns this layout's mappings.
    pub(crate) fn evaluate_reusing<F>(
        &self,
        layout: &ak::Layout,
        stop: &AtomicBool,
        progress: &AtomicU64,
        flags: (crate::alt_fingers::AltPolicy, crate::combos::Policy, bool),
        base: Option<&Mappings>,
        record: bool,
        delta: bool,
        effort: F,
    ) -> ak::Result<(Counts, Option<Mappings>)>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        self.evaluate_reusing_impl(
            layout, stop, progress, flags, base, record, delta, false, effort,
        )
        .map(|result| result.expect("a full evaluation always counts"))
    }

    /// Only the windows `layout` changes from `base`: each recorded count
    /// taken back and the new one added, as `evaluate_reusing`'s delta does,
    /// into otherwise empty counts. Their scalars and tables are differences.
    /// None where `base` cannot be reused.
    pub(crate) fn count_changes<F>(
        &self,
        layout: &ak::Layout,
        stop: &AtomicBool,
        progress: &AtomicU64,
        flags: (crate::alt_fingers::AltPolicy, crate::combos::Policy, bool),
        base: &Mappings,
        effort: F,
    ) -> ak::Result<Option<Counts>>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        Ok(self
            .evaluate_reusing_impl(
                layout,
                stop,
                progress,
                flags,
                Some(base),
                false,
                true,
                true,
                effort,
            )?
            .map(|(counts, _)| counts))
    }

    #[allow(clippy::too_many_arguments)]
    fn evaluate_reusing_impl<F>(
        &self,
        layout: &ak::Layout,
        stop: &AtomicBool,
        progress: &AtomicU64,
        flags: (crate::alt_fingers::AltPolicy, crate::combos::Policy, bool),
        base: Option<&Mappings>,
        record: bool,
        delta: bool,
        changes_only: bool,
        effort: F,
    ) -> ak::Result<Option<(Counts, Option<Mappings>)>>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        let (alt_fingers, policy, finite) = flags;
        let combos = if policy.enabled && !layout.combos.is_empty() {
            Some(crate::combos::Program::new(layout)?)
        } else {
            None
        };
        let chord_corpus = combos.as_ref().map(|_| self.chord_contexts()).transpose()?;
        let source = chord_corpus.as_ref().unwrap_or(self);
        let mut timing = crate::load_profile::LoadProfile::new("detailed physical mapping");
        let program = match crate::action_fast::Program::new(layout, self.order) {
            Ok(program) => program,
            Err(_) => {
                // Preserve reference validation errors and support layouts outside
                // the numeric mapper's capacity. Parsed keyboards fit its limit.
                let mut mapper = ak::WindowMapper::new(layout, self.order)?;
                timing.mark("Validate and initialize reference mapper fallback");
                let counts = source.evaluate_mapped(
                    layout,
                    stop,
                    progress,
                    &mut timing,
                    alt_fingers,
                    combos.as_ref(),
                    true,
                    |text, plan| match plan {
                        Some(plan) => mapper.map_chords(
                            text,
                            plan.forced(),
                            &plan.mask()[..text.len()],
                            &effort,
                        ),
                        None => mapper.map(text, &effort),
                    },
                )?;
                if changes_only {
                    return Ok(None);
                }
                return Ok(Some((counts, None)));
            }
        };
        let state = crate::action_fast::KeyState::new(&program);
        let mut mapper = crate::action_fast::Mapper::new(&program, &state);
        // Forced chord mappings bypass the memo; plain windows share it.
        if finite {
            mapper.enable_transition_memo();
        }
        timing.mark("Validate and compile numeric mapper");

        let reuse =
            base.and_then(|base| base.reuse(layout, combos.as_ref(), flags, source.windows.len()));
        let mut map_window = |text: &[u8], plan: Option<&crate::combos::ChordPlan>| {
            Ok::<_, String>(match plan {
                Some(plan) => {
                    mapper.map_chords(text, plan.forced(), &plan.mask()[..text.len()], &effort)?
                }
                None if finite => mapper.map_finite_profiled::<false, F>(
                    text,
                    &effort,
                    &mut crate::action_profile::MappingCounts::default(),
                )?,
                None => mapper.map(text, &effort)?,
            })
        };
        let plan_for = |program: Option<&crate::combos::Program>, window: &Window| {
            program
                .map(|program| program.plan_window(window.text(), window.left_open))
                .filter(|plan| !plan.is_empty())
        };
        // The recorded layout's counts, with only the windows this layout
        // changes taken back and counted again. With whole-number weights
        // (`exact_sums`) every sum is exact, so this is a fresh count's
        // result; otherwise sums differ from it in the last bits, and only
        // callers asking for `delta` get it.
        let delta = delta || changes_only || source.exact_sums();
        if let (Some(reuse), Some(base)) = (&reuse, base.filter(|_| delta)) {
            if let Some(accumulated) = &base.accumulated {
                let alts = alt_fingers.compile(layout);
                let default_fingers: Vec<_> = layout.slots.iter().map(|slot| slot.finger).collect();
                let mut accumulators = if changes_only {
                    // Dense bins come from a per-thread pool, and only
                    // touched ones are read back.
                    source.accumulators(layout, accumulated.finger_tables.is_some(), true)
                } else {
                    accumulated.clone()
                };
                let mut keys = record.then(|| base.keys.clone());
                // A window mapped as before still counts differently where
                // its counted press holds a key that became, or stopped
                // being, an action key (`Accumulators::add_window`).
                let named = |layout: &ak::Layout, slot: usize| {
                    matches!(layout.slots[slot].binding, ak::Binding::Named(_))
                };
                let renamed: Vec<bool> = (0..layout.slots.len())
                    .map(|slot| named(&base.layout, slot) != named(layout, slot))
                    .collect();
                // Rule and binding trials keep the base's combos, so a
                // window's take-back and re-add share one chord plan.
                let same_plans = base.combos == combos;
                let (presses, counted) = base.counted(&source.windows);
                // The recorded counted presses, where windows keep their keys.
                let mut pressed = record.then(|| presses.clone());
                let action_press = |layout: &ak::Layout, index: usize| {
                    presses[index]
                        .iter()
                        .any(|&key| key != GAP && named(layout, usize::from(key)))
                };
                let indices =
                    reuse.candidates(source.postings(), source.windows.len(), &renamed, counted);
                // Every window the loop below would count again is listed.
                #[cfg(test)]
                {
                    let mut listed = vec![false; source.windows.len()];
                    for &index in &indices {
                        listed[index as usize] = true;
                    }
                    for (index, window) in source.windows.iter().enumerate() {
                        assert!(
                            listed[index]
                                || !reuse.changes(window.text(), window.left_open)
                                    && action_press(&base.layout, index)
                                        == action_press(layout, index),
                            "window {index} changes outside the candidates"
                        );
                    }
                }
                for &index in &indices {
                    let index = index as usize;
                    let window = &source.windows[index];
                    if !reuse.changes(window.text(), window.left_open) {
                        // Its tables stay; only its action press may not.
                        match (
                            action_press(&base.layout, index),
                            action_press(layout, index),
                        ) {
                            (false, true) => accumulators.counts.action_presses += window.weight,
                            (true, false) => accumulators.counts.action_presses -= window.weight,
                            _ => {}
                        }
                        continue;
                    }
                    if stop.load(Ordering::Relaxed) {
                        return Err("cancelled".into());
                    }
                    let base_plan = plan_for(base.combos.as_ref(), window);
                    accumulators.add_window(
                        &base.layout,
                        window,
                        decode_keys(base.keys[index]),
                        base_plan.as_ref(),
                        alts.as_ref(),
                        &default_fingers,
                        -window.weight,
                    );
                    let plan = if same_plans {
                        base_plan
                    } else {
                        plan_for(combos.as_ref(), window)
                    };
                    let mapped = map_window(window.text(), plan.as_ref())?;
                    accumulators.add_window(
                        layout,
                        window,
                        mapped,
                        plan.as_ref(),
                        alts.as_ref(),
                        &default_fingers,
                        window.weight,
                    );
                    if let Some(recorded) = &mut keys {
                        match encode_keys(mapped) {
                            Some(encoded) => {
                                recorded[index] = encoded;
                                if let Some(pressed) = &mut pressed {
                                    pressed[index] = counted_press(plan.as_ref(), encoded, window);
                                }
                            }
                            None => keys = None,
                        }
                    }
                }
                progress.store(100, Ordering::Relaxed);
                let (counts, accumulators) = match &keys {
                    Some(_) => {
                        let accumulators = accumulators.recorded();
                        (accumulators.clone().finish(), accumulators)
                    }
                    None => return Ok(Some((accumulators.finish(), None))),
                };
                let mappings = keys.map(|keys| Mappings {
                    layout: layout.clone(),
                    combos,
                    flags,
                    keys,
                    accumulated: Some(accumulators),
                    pressed,
                    counted: std::sync::OnceLock::new(),
                });
                return Ok(Some((counts, mappings)));
            }
        }
        if changes_only {
            return Ok(None);
        }
        let mut keys = record.then(|| Vec::with_capacity(source.windows.len()));
        let mut index = 0;
        let accumulators = source.evaluate_accumulated(
            layout,
            stop,
            progress,
            alt_fingers,
            combos.as_ref(),
            true,
            |text, plan| {
                let window = &source.windows[index];
                let mapped = match &reuse {
                    Some(reuse) if !reuse.changes(text, window.left_open) => {
                        decode_keys(reuse.keys[index])
                    }
                    _ => map_window(text, plan)?,
                };
                index += 1;
                if let Some(recorded) = &mut keys {
                    match encode_keys(mapped) {
                        Some(encoded) => recorded.push(encoded),
                        None => keys = None,
                    }
                }
                Ok(mapped)
            },
        )?;
        progress.store(100, Ordering::Relaxed);
        timing.mark("Map contexts and build physical count tables");
        let (counts, accumulated) = match &keys {
            Some(_) if delta => {
                let accumulators = accumulators.recorded();
                (accumulators.clone().finish(), Some(accumulators))
            }
            _ => (accumulators.finish(), None),
        };
        let mappings = keys.map(|keys| Mappings {
            layout: layout.clone(),
            combos,
            flags,
            keys,
            accumulated,
            pressed: None,
            counted: std::sync::OnceLock::new(),
        });
        Ok(Some((counts, mappings)))
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
        self.evaluate_reference_with_policy(
            layout,
            stop,
            progress,
            crate::alt_fingers::AltPolicy::layout(true),
            false,
            effort,
        )
    }

    #[cfg(test)]
    pub(crate) fn evaluate_reference_with_policy<F>(
        &self,
        layout: &ak::Layout,
        stop: &AtomicBool,
        progress: &AtomicU64,
        alt_fingers: crate::alt_fingers::AltPolicy,
        combos: impl Into<crate::combos::Policy>,
        effort: F,
    ) -> ak::Result<Counts>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        let mut timing = crate::load_profile::LoadProfile::new("reference physical mapping");
        let mut mapper = ak::WindowMapper::new(layout, self.order)?;
        let policy: crate::combos::Policy = combos.into();
        let combos = if policy.enabled && !layout.combos.is_empty() {
            Some(crate::combos::Program::new(layout)?)
        } else {
            None
        };
        let chord_corpus = combos.as_ref().map(|_| self.chord_contexts()).transpose()?;
        let source = chord_corpus.as_ref().unwrap_or(self);
        timing.mark("Validate and initialize reference mapper");

        source.evaluate_mapped(
            layout,
            stop,
            progress,
            &mut timing,
            alt_fingers,
            combos.as_ref(),
            false,
            |text, plan| match plan {
                Some(plan) => {
                    mapper.map_chords(text, plan.forced(), &plan.mask()[..text.len()], &effort)
                }
                None => mapper.map(text, &effort),
            },
        )
    }

    /// Replay mappings through the canonical count accumulator. Rule proposal
    /// caches use the source index without exposing or copying corpus windows.
    pub(crate) fn evaluate_rule_mappings<F>(
        &self,
        layout: &ak::Layout,
        stop: &AtomicBool,
        progress: &AtomicU64,
        mut map: F,
    ) -> ak::Result<Counts>
    where
        F: FnMut(usize, &[u8]) -> ak::Result<[Option<usize>; 5]>,
    {
        let mut timing = crate::load_profile::LoadProfile::new("cached rule physical mapping");
        let mut index = 0;
        self.evaluate_mapped(
            layout,
            stop,
            progress,
            &mut timing,
            crate::alt_fingers::AltPolicy::layout(false),
            None,
            true,
            |text, _| {
                let mapped = map(index, text);
                index += 1;
                mapped
            },
        )
    }

    fn postings(&self) -> &WindowIndex {
        self.postings
            .get_or_init(|| WindowIndex::new(&self.windows))
    }

    /// Whether sums of this corpus's counts are exact in any order
    /// (`exact_sums_of`).
    /// Each window's keys, as `evaluate_with_(finite_)policy` maps them
    /// without combos or contextual fingers, under `program` with bindings
    /// `state` (`GAP` where none). These keys are all its counts depend on.
    pub(crate) fn plain_window_keys<F>(
        &self,
        program: &crate::action_fast::Program,
        state: &crate::action_fast::KeyState,
        finite: bool,
        effort: F,
    ) -> ak::Result<Vec<[u8; 5]>>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        let mut mapper = crate::action_fast::Mapper::new(program, state);
        self.windows
            .iter()
            .map(|window| {
                let mapped = if finite {
                    mapper.map_finite_profiled::<false, F>(
                        window.text(),
                        &effort,
                        &mut crate::action_profile::MappingCounts::default(),
                    )?
                } else {
                    mapper.map(window.text(), &effort)?
                };
                encode_keys(mapped).ok_or_else(|| "slot outside the key encoding".into())
            })
            .collect()
    }

    pub(crate) fn order(&self) -> usize {
        self.order
    }

    pub(crate) fn exact_sums(&self) -> bool {
        self.exact_sums
    }

    /// `exact_sums` of the windows that evaluating `layout` counts.
    pub(crate) fn exact_sums_for(
        &self,
        layout: &ak::Layout,
        policy: crate::combos::Policy,
    ) -> bool {
        if policy.enabled && !layout.combos.is_empty() {
            self.chord_contexts().is_ok_and(|chords| chords.exact_sums)
        } else {
            self.exact_sums
        }
    }

    pub(crate) fn rule_window_count(&self) -> usize {
        self.windows.len()
    }

    /// Copy the compact source windows used by action-rule delta evaluation.
    /// This deliberately exposes source text rather than mapped winners: a
    /// changed rule can turn a previously losing action into the winner.
    pub(crate) fn rule_delta_windows(&self, stop: &AtomicBool) -> ak::Result<Vec<RuleDeltaWindow>> {
        let mut result = Vec::with_capacity(self.windows.len());
        for (index, window) in self.windows.iter().enumerate() {
            if index % 1024 == 0 && stop.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            result.push(RuleDeltaWindow {
                bytes: window.bytes,
                len: window.len,
                target: window.target,
                weight: window.weight,
            });
        }
        Ok(result)
    }

    // Keep window traversal, histogram insertion, frequency accumulation and
    // cancellation/progress checks identical for numeric and reference mapping.
    fn evaluate_mapped<F>(
        &self,
        layout: &ak::Layout,
        stop: &AtomicBool,
        progress: &AtomicU64,
        timing: &mut crate::load_profile::LoadProfile,
        alt_fingers: crate::alt_fingers::AltPolicy,
        combos: Option<&crate::combos::Program>,
        dense_counts: bool,
        map: F,
    ) -> ak::Result<Counts>
    where
        F: FnMut(&[u8], Option<&crate::combos::ChordPlan>) -> ak::Result<[Option<usize>; 5]>,
    {
        let counts = self
            .evaluate_accumulated(
                layout,
                stop,
                progress,
                alt_fingers,
                combos,
                dense_counts,
                map,
            )?
            .finish();
        progress.store(100, Ordering::Relaxed);
        timing.mark("Map contexts and build physical count tables");
        Ok(counts)
    }

    /// `evaluate_mapped` before its tables are built, so a similar layout can
    /// start from these counts and add only the windows it changes.
    fn evaluate_accumulated<F>(
        &self,
        layout: &ak::Layout,
        stop: &AtomicBool,
        progress: &AtomicU64,
        alt_fingers: crate::alt_fingers::AltPolicy,
        combos: Option<&crate::combos::Program>,
        dense_counts: bool,
        mut map: F,
    ) -> ak::Result<Accumulators>
    where
        F: FnMut(&[u8], Option<&crate::combos::ChordPlan>) -> ak::Result<[Option<usize>; 5]>,
    {
        let alts = alt_fingers.compile(layout);
        let default_fingers: Vec<_> = layout.slots.iter().map(|slot| slot.finger).collect();
        let mut accumulators = self.accumulators(layout, alts.is_some(), dense_counts);
        for (i, window) in self.windows.iter().enumerate() {
            if stop.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            // Windows no combo touches take the plain (memoized) mapping.
            let plan = combos
                .map(|program| program.plan_window(window.text(), window.left_open))
                .filter(|plan| !plan.is_empty());
            let mapped = map(window.text(), plan.as_ref())?;
            accumulators.add_window(
                layout,
                window,
                mapped,
                plan.as_ref(),
                alts.as_ref(),
                &default_fingers,
                window.weight,
            );
            if i % 1024 == 0 {
                progress.store(
                    ((i + 1) * 100 / self.windows.len()) as u64,
                    Ordering::Relaxed,
                );
            }
        }
        Ok(accumulators)
    }

    fn accumulators(
        &self,
        layout: &ak::Layout,
        use_alt_fingers: bool,
        dense_counts: bool,
    ) -> Accumulators {
        let mut counts = Counts::new(self.order);
        counts.speed_populations = self.speed_populations;
        Accumulators {
            counts,
            tables: std::array::from_fn(|index| {
                PhysicalHistogram::new(layout.slots.len(), index + 1, dense_counts)
            }),
            skip: PhysicalHistogram::new(layout.slots.len(), 2, dense_counts),
            finger_tables: use_alt_fingers
                .then(|| std::array::from_fn(|_| FingerHistogram::default())),
            finger_skip: use_alt_fingers.then(FingerHistogram::default),
        }
    }
}

/// Count tables while windows are added; `finish` builds `Counts`.
#[derive(Clone)]
struct Accumulators {
    counts: Counts,
    tables: [PhysicalHistogram; 3],
    skip: PhysicalHistogram,
    finger_tables: Option<[FingerHistogram; 3]>,
    finger_skip: Option<FingerHistogram>,
}

impl Accumulators {
    /// Count one window mapped to `mapped` with `plan`, at `weight` (negative
    /// to take back an earlier count).
    #[allow(clippy::too_many_arguments)]
    fn add_window(
        &mut self,
        layout: &ak::Layout,
        window: &Window,
        mapped: [Option<usize>; 5],
        plan: Option<&crate::combos::ChordPlan>,
        alts: Option<&crate::alt_fingers::Compiled>,
        default_fingers: &[usize],
        weight: f64,
    ) {
        if let Some(plan) = plan {
            self.add_chord_window(layout, window, mapped, plan, alts, default_fingers, weight);
            return;
        }
        let Self {
            counts,
            tables,
            skip,
            finger_tables,
            finger_skip,
        } = self;
        let keys = mapped;
        if mapped[window.target()].is_some() {
            counts.characters += weight;
        } else {
            counts.ignored_characters += weight;
        }
        let end = window.end();
        let effective =
            alts.map(|alts| alts.resolve(window.text(), &keys[..window.len()], default_fingers));
        let last = window.target();
        if let Some(key) = keys[last] {
            counts.presses += weight;
            if matches!(layout.slots[key].binding, ak::Binding::Named(_)) {
                counts.action_presses += weight;
            }
            for n in 1..=3.min(end) {
                let tail = &keys[end - n..end];
                if tail.iter().any(Option::is_none) {
                    break;
                }
                let mut ids = [0; 3];
                for (id, slot) in ids.iter_mut().zip(tail) {
                    *id = slot.unwrap();
                }
                tables[n - 1].add(&ids[..n], weight);
                if let (Some(tables), Some(effective)) = (&mut *finger_tables, &effective) {
                    let start = end - n;
                    let mut fingers = [0; 3];
                    for (finger, resolved) in fingers.iter_mut().zip(&effective[start..end]) {
                        *finger = resolved.unwrap();
                    }
                    tables[n - 1].add(&ids[..n], &fingers[..n], weight);
                }
            }
            if end >= 3 && keys[last - 1].is_some() {
                if let Some(first) = keys[last - 2] {
                    skip.add(&[first, key], weight);
                    if let (Some(table), Some(effective)) = (&mut *finger_skip, &effective) {
                        table.add(
                            &[first, key],
                            &[effective[last - 2].unwrap(), effective[last].unwrap()],
                            weight,
                        );
                    }
                }
            }
        }
    }

    /// `add_window` for a context a combo plans: each combo is one chord
    /// press (`ChordGrams`).
    #[allow(clippy::too_many_arguments)]
    fn add_chord_window(
        &mut self,
        layout: &ak::Layout,
        window: &Window,
        mapped: [Option<usize>; 5],
        plan: &crate::combos::ChordPlan,
        alts: Option<&crate::alt_fingers::Compiled>,
        default_fingers: &[usize],
        weight: f64,
    ) {
        let target = window.target();
        if mapped[target].is_some() || plan.chorded(target) {
            self.counts.characters += weight;
        } else {
            self.counts.ignored_characters += weight;
        }
        let effective =
            alts.map(|alts| alts.resolve(window.text(), &mapped[..window.len()], default_fingers));
        let grams = chord_grams(plan, &mapped, effective.as_ref(), target);
        if grams.pressed().next().is_some() {
            self.counts.presses += weight;
            if grams
                .pressed()
                .any(|key| matches!(layout.slots[key].binding, ak::Binding::Named(_)))
            {
                self.counts.action_presses += weight;
            }
        }
        for (tail, fingers, sign) in grams.iter() {
            let weight = sign * weight;
            let [a, b, c] = tail.map(usize::from);
            let [fa, fb, fc] = fingers.map(usize::from);
            self.tables[0].add(&[c], weight);
            if let Some(tables) = &mut self.finger_tables {
                tables[0].add(&[c], &[fc], weight);
            }
            if tail[1] == GAP {
                continue;
            }
            self.tables[1].add(&[b, c], weight);
            if let Some(tables) = &mut self.finger_tables {
                tables[1].add(&[b, c], &[fb, fc], weight);
            }
            if tail[0] == GAP {
                continue;
            }
            self.tables[2].add(&[a, b, c], weight);
            self.skip.add(&[a, c], weight);
            if let Some(tables) = &mut self.finger_tables {
                tables[2].add(&[a, b, c], &[fa, fb, fc], weight);
            }
            if let Some(table) = &mut self.finger_skip {
                table.add(&[a, c], &[fa, fc], weight);
            }
        }
    }

    /// Keep for later updates: their tables then need only a short sort.
    fn recorded(mut self) -> Self {
        self.tables
            .iter_mut()
            .for_each(PhysicalHistogram::sort_touched);
        self.skip.sort_touched();
        self
    }

    fn finish(self) -> Counts {
        let mut counts = self.counts;
        counts.tables = self.tables.map(PhysicalHistogram::into_table);
        counts.skip = self.skip.into_table();
        counts.finger_tables = self.finger_tables.map(|tables| {
            let mut width = 0;
            tables.map(|table| {
                width += 1;
                table.into_table(width)
            })
        });
        counts.finger_skip = self.finger_skip.map(|table| table.into_table(2));
        counts
    }
}

// One compact terminal history per weighted context; metric contributions are
// linear in frequency, so rejected proposals never need to mutate this cache.
/// One evaluation's mapped keys per window, in corpus window order, so a
/// later evaluation of a similar layout re-maps only the windows it changes.
pub(crate) struct Mappings {
    layout: ak::Layout,
    combos: Option<crate::combos::Program>,
    flags: (crate::alt_fingers::AltPolicy, crate::combos::Policy, bool),
    keys: Vec<[u8; 5]>,
    /// The counts before their tables were built, so a later evaluation can
    /// count only the windows it changes.
    accumulated: Option<Accumulators>,
    /// `counted`'s presses when known from an earlier evaluation's.
    pressed: Option<Vec<[u8; 2]>>,
    /// Each window's counted press (up to a chord's two keys) and, by slot,
    /// the windows, ascending, whose counted press holds it (`counted`).
    counted: std::sync::OnceLock<(Vec<[u8; 2]>, Vec<Vec<u32>>)>,
}

/// A window's counted press under `plan` from its mapped `keys`.
fn counted_press(
    plan: Option<&crate::combos::ChordPlan>,
    keys: [u8; 5],
    window: &Window,
) -> [u8; 2] {
    match plan {
        Some(plan) => chord_grams(plan, &decode_keys(keys), None, window.target()).pressed,
        None => [keys[window.target()], GAP],
    }
}

/// What a layout changed relative to recorded `Mappings`: windows holding
/// none of `bytes` and none of `pairs` that no changed combo touches map
/// exactly as recorded.
struct Reuse<'a> {
    keys: &'a [[u8; 5]],
    bytes: [u64; 4],
    /// Adjacent characters whose second a changed text rule may now type,
    /// or no longer types, after the first; `firsts` holds their firsts.
    pairs: Vec<[u8; 2]>,
    firsts: [u64; 4],
    combos: Option<crate::combos::Program>,
}

impl Reuse<'_> {
    /// The windows, ascending, that `changes` can select or whose counted
    /// press holds a `renamed` key (`counted` lists each slot's windows):
    /// those holding one of `bytes` or `pairs` or a changed combo's output,
    /// and those whose action press may change.
    fn candidates(
        &self,
        index: &WindowIndex,
        windows: usize,
        renamed: &[bool],
        counted: &[Vec<u32>],
    ) -> Vec<u32> {
        let mut bytes = self.bytes;
        if let Some(combos) = &self.combos {
            for (bytes, outputs) in bytes.iter_mut().zip(combos.output_bytes()) {
                *bytes |= outputs;
            }
        }
        let mut marks = Vec::new();
        for byte in 0..256 {
            if bytes[byte / 64] & 1 << (byte % 64) != 0 {
                mark_contexts(&mut marks, windows, &index.bytes[byte]);
            }
        }
        for &pair in &self.pairs {
            mark_contexts(&mut marks, windows, index.pair(pair));
        }
        for (slot, _) in renamed.iter().enumerate().filter(|(_, &renamed)| renamed) {
            mark_contexts(&mut marks, windows, &counted[slot]);
        }
        let mut ids = Vec::new();
        if !marks.is_empty() {
            read_marks(&mut marks, &mut ids);
        }
        ids
    }

    fn changes(&self, text: &[u8], left_open: bool) -> bool {
        let has = |set: &[u64; 4], byte: u8| set[usize::from(byte) / 64] & 1 << (byte % 64) != 0;
        text.iter().any(|&byte| has(&self.bytes, byte))
            || !self.pairs.is_empty()
                && text.windows(2).any(|pair| {
                    has(&self.firsts, pair[0]) && self.pairs.contains(&[pair[0], pair[1]])
                })
            || self
                .combos
                .as_ref()
                .is_some_and(|combos| combos.touches(text, left_open))
    }
}

// Keys type exact bytes, as in `action_fast::Program::affected_bytes`.
fn add_byte(bytes: &mut [u64; 4], byte: u8) {
    bytes[usize::from(byte) / 64] |= 1 << (byte % 64);
}

/// Every byte an emission can type; None when a call makes that open-ended.
fn emission_bytes(emission: &ak::Emission, bytes: &mut [u64; 4]) -> Option<()> {
    match emission {
        ak::Emission::Text(text) => text.iter().for_each(|&byte| add_byte(bytes, byte)),
        ak::Emission::None => {}
        ak::Emission::Call(_) => return None,
    }
    Some(())
}

/// Every byte an action can type: a key bound to it can only matter to
/// windows holding one of them. None when it depends on history or calls.
fn action_bytes(action: Option<&ak::Action>, bytes: &mut [u64; 4]) -> Option<()> {
    match action {
        None | Some(ak::Action::Inactive) => {}
        Some(ak::Action::Text(text)) => text.iter().for_each(|&byte| add_byte(bytes, byte)),
        Some(ak::Action::Rules {
            basis: ak::Basis::Text,
            rules,
            fallback,
        }) => {
            for emission in rules.values().chain([fallback]) {
                emission_bytes(emission, bytes)?;
            }
        }
        Some(_) => return None,
    }
    Some(())
}

/// The rules of an adaptive key typing its own character wherever they
/// leave out the key pressed before (`ak::key_context`), as
/// `swap_fit::install` makes them.
fn adaptive_action<'a>(
    name: &str,
    action: &'a ak::Action,
) -> Option<&'a BTreeMap<Vec<u8>, ak::Emission>> {
    match action {
        ak::Action::Rules {
            basis: ak::Basis::Text,
            rules,
            fallback,
        } if ak::key_context(name, ak::Basis::Text, rules, fallback) => Some(rules),
        _ => None,
    }
}

/// Whether mapping `layout` never weighs one key against another: plain keys
/// type distinct characters, and its only actions are adaptive keys whose
/// rules after each context exchange those keys' own characters, as
/// `swap_fit::install` makes them. A choice looks ahead to the keys typing
/// the next character, so only then does an adaptive change reach no further
/// than `key_rule_changes` finds.
fn choice_free(layout: &ak::Layout) -> bool {
    let mut owners = [0u8; 256];
    let mut exchanges: BTreeMap<u8, Vec<(u8, u8)>> = BTreeMap::new();
    for slot in &layout.slots {
        let own = match &slot.binding {
            ak::Binding::Empty => continue,
            ak::Binding::Text(text) if text.len() == 1 => text[0],
            ak::Binding::Named(name) => {
                let Some(rules) = adaptive_rules(&slot.binding, &layout.actions) else {
                    return false;
                };
                let own = name.as_bytes()[0];
                for (context, emission) in rules {
                    let ak::Emission::Text(text) = emission else {
                        return false;
                    };
                    if text.len() != 1 {
                        return false;
                    }
                    exchanges
                        .entry(context[0])
                        .or_default()
                        .push((own, text[0]));
                }
                own
            }
            _ => return false,
        };
        owners[usize::from(own)] += 1;
        if owners[usize::from(own)] > 1 {
            return false;
        }
    }
    exchanges.values().all(|moves| {
        let keys: BTreeSet<u8> = moves.iter().map(|&(key, _)| key).collect();
        let outputs: BTreeSet<u8> = moves.iter().map(|&(_, output)| output).collect();
        keys.len() == moves.len() && keys == outputs
    })
}

/// `adaptive_action` for the action `binding` names.
fn adaptive_rules<'a>(
    binding: &ak::Binding,
    actions: &'a BTreeMap<String, ak::Action>,
) -> Option<&'a BTreeMap<Vec<u8>, ak::Emission>> {
    let ak::Binding::Named(name) = binding else {
        return None;
    };
    adaptive_action(name, actions.get(name)?)
}

/// Record where an adaptive key typing `own` changes from `before` to
/// `after` rules. Right after its context key it types a rule's output, else
/// `own`, and mapping chooses among the keys that can type each character:
/// only where a byte the context key types is followed by the old or the new
/// output can a window's mapping change. An output or context key typing
/// open-ended text falls back to every window holding the context's bytes.
fn key_rule_changes(
    own: u8,
    before: &BTreeMap<Vec<u8>, ak::Emission>,
    after: &BTreeMap<Vec<u8>, ak::Emission>,
    layouts: [&ak::Layout; 2],
    bytes: &mut [u64; 4],
    pairs: &mut Vec<[u8; 2]>,
    firsts: &mut [u64; 4],
) -> Option<()> {
    let output = |emission: Option<&ak::Emission>| match emission {
        None => Some(own),
        Some(ak::Emission::Text(text)) if text.len() == 1 => Some(text[0]),
        _ => None,
    };
    for context in before.keys().chain(after.keys()) {
        if before.get(context) == after.get(context) {
            continue;
        }
        // The keys named by the context, as `Basis::Press` reads them.
        let mut typed = [0; 4];
        let mut known = true;
        for layout in layouts {
            for slot in &layout.slots {
                let named = match &slot.binding {
                    ak::Binding::Named(name) => name.as_bytes(),
                    _ => slot.label.as_bytes(),
                };
                if named == context.as_slice() {
                    known &= binding_bytes(&slot.binding, &layout.actions, &mut typed).is_some();
                }
            }
        }
        match [output(before.get(context)), output(after.get(context))] {
            [Some(old), Some(new)] if known => {
                for first in 0..=u8::MAX {
                    if typed[usize::from(first) / 64] & 1 << (first % 64) == 0 {
                        continue;
                    }
                    add_byte(firsts, first);
                    for output in [old, new] {
                        if !pairs.contains(&[first, output]) {
                            pairs.push([first, output]);
                        }
                    }
                }
            }
            _ => {
                context.iter().for_each(|&byte| add_byte(bytes, byte));
                for layout in layouts {
                    for slot in &layout.slots {
                        let named = match &slot.binding {
                            ak::Binding::Named(name) => name.as_bytes(),
                            _ => slot.label.as_bytes(),
                        };
                        if named == context.as_slice() {
                            binding_bytes(&slot.binding, &layout.actions, bytes)?;
                        }
                    }
                }
            }
        }
    }
    Some(())
}

fn binding_bytes(
    binding: &ak::Binding,
    actions: &BTreeMap<String, ak::Action>,
    bytes: &mut [u64; 4],
) -> Option<()> {
    match binding {
        ak::Binding::Empty => Some(()),
        ak::Binding::Text(text) => {
            text.iter().for_each(|&byte| add_byte(bytes, byte));
            Some(())
        }
        ak::Binding::Named(name) => action_bytes(actions.get(name), bytes),
    }
}

impl Mappings {
    /// The recorded layout's counts, when they were kept (`count_changes`
    /// differences add to them).
    pub(crate) fn counts(&self) -> Option<Counts> {
        Some(self.accumulated.clone()?.finish())
    }

    /// Each of `windows`' counted press, the keys whose bindings decide
    /// whether it is an action press (`Accumulators::add_window`), and by
    /// slot the windows, ascending, whose counted press holds that slot.
    fn counted(&self, windows: &[Window]) -> &(Vec<[u8; 2]>, Vec<Vec<u32>>) {
        self.counted.get_or_init(|| {
            let press = |(window, &keys): (&Window, &[u8; 5])| {
                let plan = self
                    .combos
                    .as_ref()
                    .map(|program| program.plan_window(window.text(), window.left_open))
                    .filter(|plan| !plan.is_empty());
                counted_press(plan.as_ref(), keys, window)
            };
            let presses = match &self.pressed {
                Some(pressed) => {
                    #[cfg(test)]
                    assert!(
                        windows
                            .iter()
                            .zip(&self.keys)
                            .map(press)
                            .eq(pressed.iter().copied()),
                        "derived counted presses differ from the mapped ones"
                    );
                    pressed.clone()
                }
                None => windows.iter().zip(&self.keys).map(press).collect(),
            };
            let mut by_slot = vec![Vec::new(); self.layout.slots.len()];
            for (index, pressed) in presses.iter().enumerate() {
                for (at, &key) in pressed.iter().enumerate() {
                    if key != GAP && (at == 0 || pressed[0] != key) {
                        by_slot[usize::from(key)].push(index as u32);
                    }
                }
            }
            (presses, by_slot)
        })
    }

    /// How `layout` differs from the recorded one, when every change is to
    /// bindings, text-basis actions or combos. A changed key matters only
    /// where it types one of its old or new bytes; a rule edit keeping its
    /// fallback only where its context occurs (`action_rule_eval` relies on
    /// the same); a combo only in windows it touches.
    fn reuse(
        &self,
        layout: &ak::Layout,
        combos: Option<&crate::combos::Program>,
        flags: (crate::alt_fingers::AltPolicy, crate::combos::Policy, bool),
        windows: usize,
    ) -> Option<Reuse<'_>> {
        let base = &self.layout;
        if flags != self.flags
            || windows != self.keys.len()
            || layout.slots.len() != base.slots.len()
            || (layout.left_outer, layout.right_outer, layout.action_mode)
                != (base.left_outer, base.right_outer, base.action_mode)
            || layout.row_stagger != base.row_stagger
            || flags.0.compile(layout) != flags.0.compile(base)
        {
            return None;
        }
        let mut bytes = [0; 4];
        let mut pairs = Vec::new();
        let mut firsts = [0; 4];
        let precise = choice_free(base) && choice_free(layout);
        for (old, new) in base.slots.iter().zip(&layout.slots) {
            if old == new {
                continue;
            }
            let mut moved = new.clone();
            moved.binding = old.binding.clone();
            if moved != *old {
                return None;
            }
            // A key turned adaptive, or plain again, keeping its own
            // character changes only where its rules apply.
            let own = |binding: &ak::Binding, actions| match binding {
                ak::Binding::Text(text) if text.len() == 1 => Some(text[0]),
                _ => adaptive_rules(binding, actions).map(|_| match binding {
                    ak::Binding::Named(name) => name.as_bytes()[0],
                    _ => unreachable!(),
                }),
            };
            let (before, after) = (
                adaptive_rules(&old.binding, &base.actions),
                adaptive_rules(&new.binding, &layout.actions),
            );
            match own(&old.binding, &base.actions) {
                Some(byte)
                    if precise
                        && (before.is_some() || after.is_some())
                        && own(&new.binding, &layout.actions) == Some(byte) =>
                {
                    let none = BTreeMap::new();
                    key_rule_changes(
                        byte,
                        before.unwrap_or(&none),
                        after.unwrap_or(&none),
                        [base, layout],
                        &mut bytes,
                        &mut pairs,
                        &mut firsts,
                    )?;
                }
                _ => {
                    binding_bytes(&old.binding, &base.actions, &mut bytes)?;
                    binding_bytes(&new.binding, &layout.actions, &mut bytes)?;
                }
            }
        }
        for name in base.actions.keys().chain(layout.actions.keys()) {
            let (old, new) = (base.actions.get(name), layout.actions.get(name));
            if old == new {
                continue;
            }
            match (old, new) {
                (
                    Some(ak::Action::Rules {
                        basis: ak::Basis::Text,
                        rules: before,
                        fallback: old_fallback,
                    }),
                    Some(ak::Action::Rules {
                        basis: ak::Basis::Text,
                        rules: after,
                        fallback: new_fallback,
                    }),
                ) if old_fallback == new_fallback => {
                    if precise
                        && ak::key_context(name, ak::Basis::Text, before, old_fallback)
                        && ak::key_context(name, ak::Basis::Text, after, new_fallback)
                    {
                        key_rule_changes(
                            name.as_bytes()[0],
                            before,
                            after,
                            [base, layout],
                            &mut bytes,
                            &mut pairs,
                            &mut firsts,
                        )?;
                        continue;
                    }
                    // An adaptive key's context is the key pressed before,
                    // which may have typed any of its characters.
                    let by_key = ak::key_context(name, ak::Basis::Text, before, old_fallback)
                        || ak::key_context(name, ak::Basis::Text, after, new_fallback);
                    // A text rule's key types its output only right after
                    // its context character. Mapping chooses among the keys
                    // that can type each character, so only where the
                    // context is followed by the old or the new output can
                    // a rule change alter a window's mapping.
                    let output = |emission: Option<&ak::Emission>,
                                  actions: &BTreeMap<_, _>,
                                  context: u8| {
                        match emission.unwrap_or(old_fallback) {
                            ak::Emission::Text(text) if text.len() == 1 => Some(text[0]),
                            ak::Emission::Call(name)
                                if matches!(actions.get(name), Some(ak::Action::RepeatOutput)) =>
                            {
                                Some(context)
                            }
                            _ => None,
                        }
                    };
                    for context in before.keys().chain(after.keys()) {
                        if before.get(context) != after.get(context) {
                            let outputs = (context.len() == 1 && !by_key)
                                .then(|| {
                                    Some([
                                        output(before.get(context), &base.actions, context[0])?,
                                        output(after.get(context), &layout.actions, context[0])?,
                                    ])
                                })
                                .flatten();
                            if let Some(outputs) = outputs {
                                add_byte(&mut firsts, context[0]);
                                for output in outputs {
                                    if !pairs.contains(&[context[0], output]) {
                                        pairs.push([context[0], output]);
                                    }
                                }
                                continue;
                            }
                            context.iter().for_each(|&byte| add_byte(&mut bytes, byte));
                            if by_key {
                                for (slots, actions) in [
                                    (&base.slots, &base.actions),
                                    (&layout.slots, &layout.actions),
                                ] {
                                    for slot in slots.iter() {
                                        let named = match &slot.binding {
                                            ak::Binding::Named(name) => name.as_bytes(),
                                            _ => slot.label.as_bytes(),
                                        };
                                        if named == context.as_slice() {
                                            binding_bytes(&slot.binding, actions, &mut bytes)?;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                // An adaptive key's action made or dropped, as
                // `swap_fit::install` does: absent, it has no rules.
                (None, Some(action)) | (Some(action), None)
                    if precise && adaptive_action(name, action).is_some() =>
                {
                    let none = BTreeMap::new();
                    let rules = adaptive_action(name, action).unwrap();
                    let (before, after) = if old.is_none() {
                        (&none, rules)
                    } else {
                        (rules, &none)
                    };
                    key_rule_changes(
                        name.as_bytes()[0],
                        before,
                        after,
                        [base, layout],
                        &mut bytes,
                        &mut pairs,
                        &mut firsts,
                    )?;
                }
                _ => {
                    action_bytes(old, &mut bytes)?;
                    action_bytes(new, &mut bytes)?;
                }
            }
        }
        let combos = match (&self.combos, combos) {
            (None, None) => None,
            (Some(old), Some(new)) => old.changed(new),
            _ => return None,
        };
        Some(Reuse {
            keys: &self.keys,
            bytes,
            pairs,
            firsts,
            combos,
        })
    }
}

fn encode_keys(mapped: [Option<usize>; 5]) -> Option<[u8; 5]> {
    let mut keys = [GAP; 5];
    for (key, slot) in keys.iter_mut().zip(mapped) {
        if let Some(slot) = slot {
            *key = u8::try_from(slot).ok().filter(|&slot| slot != GAP)?;
        }
    }
    Some(keys)
}

fn decode_keys(keys: [u8; 5]) -> [Option<usize>; 5] {
    keys.map(|key| (key != GAP).then_some(usize::from(key)))
}

const GAP: u8 = u8::MAX;

type Tail = [u8; 3];

/// Most tails `ChordGrams` holds: a chord in each of the last three presses.
const CHORD_TAILS: usize = 14;

/// A combo context's movement, every combo being one chord press of all its
/// keys (as on keyboards and akl.gg). For the last presses A, B, C (a chord's
/// keys, or one key or gap) unigrams count once per key of C, bigrams once per
/// pair from B to C, and skipgrams once per pair from A to C; trigrams count
/// once, split between a middle chord's keys. A tail contributes its unigram,
/// bigram and trigram (skipgram included), so these are the weighted tails
/// `Σ T(a,b,c) / |B| + (1 − |A|/|B|) Σ T(·,b,c) + (1 − |B|) Σ T(·,·,c)`.
/// There is no movement between a chord's keys; a chord's second character
/// is no press.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ChordGrams {
    tails: [(Tail, Tail, f64); CHORD_TAILS],
    len: u8,
    /// The counted press's keys, `GAP` where none.
    pressed: [u8; 2],
}

/// Equal tails in use and presses; unused entries are ignored.
impl PartialEq for ChordGrams {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len
            && self.pressed == other.pressed
            && self.tails[..usize::from(self.len)] == other.tails[..usize::from(other.len)]
    }
}

impl ChordGrams {
    /// These grams after `movement`, for a context whose presses all follow
    /// their keys (`Incremental::chord_direct`).
    fn moved(&self, movement: Move) -> Self {
        let mut grams = *self;
        for (tail, ..) in &mut grams.tails[..usize::from(grams.len)] {
            *tail = move_tail(*tail, movement);
        }
        let [a, b, _] = move_tail([grams.pressed[0], grams.pressed[1], GAP], movement);
        grams.pressed = [a, b];
        grams
    }

    /// Tails, their fingers (`GAP`s without contextual fingers) and signed
    /// weights.
    fn iter(&self) -> impl Iterator<Item = (Tail, Tail, f64)> + '_ {
        self.tails[..usize::from(self.len)].iter().copied()
    }

    fn pressed(&self) -> impl Iterator<Item = usize> + '_ {
        self.pressed
            .iter()
            .filter(|&&key| key != GAP)
            .map(|&key| usize::from(key))
    }
}

/// Add a combo context's chord tails at `weight`, as `apply_contextual` adds
/// one tail; `fingers` tracks contextual fingers.
#[allow(clippy::too_many_arguments)]
fn add_chord_grams(
    contributions: &TailContributions,
    raw: &mut Raw,
    totals: &mut [f64; 4],
    grams: &ChordGrams,
    weight: f64,
    fingers: bool,
    positions: &[usize],
    geometry: &Geometry,
) {
    for (tail, tail_fingers, sign) in grams.iter() {
        contributions.apply_contextual(
            raw,
            totals,
            tail,
            fingers.then_some(tail_fingers),
            sign * weight,
            positions,
            geometry,
        );
    }
}

/// `ChordGrams` of one window: `mapped` its keys (chord positions on their
/// keys), `fingers` each position's resolved finger.
fn chord_grams(
    plan: &crate::combos::ChordPlan,
    mapped: &[Option<usize>; 5],
    fingers: Option<&[Option<usize>; 5]>,
    target: usize,
) -> ChordGrams {
    let element = |at: usize| {
        let key = mapped[at].map_or(GAP, |key| key as u8);
        let finger = fingers
            .and_then(|fingers| fingers[at])
            .map_or(GAP, |finger| finger as u8);
        (key, finger)
    };
    // Presses through the counted character, each one or two (key, finger).
    let mut presses = [[(GAP, GAP); 2]; 5];
    let mut sizes = [1usize; 5];
    let mut count = 0;
    let mut counted = None;
    let mut at = 0;
    while at <= target {
        if at < plan.open_prefix() {
            at += 1;
            count += 1;
            continue;
        }
        if plan.starts(at) {
            if at + 1 == target {
                // The counted character is a chord's second: no press.
                counted = None;
                break;
            }
            presses[count] = [element(at), element(at + 1)];
            sizes[count] = 2;
            at += 2;
        } else {
            presses[count][0] = element(at);
            at += 1;
        }
        if at > target {
            counted = Some(count);
        }
        count += 1;
    }
    let mut grams = ChordGrams {
        tails: [([GAP; 3], [GAP; 3], 0.0); CHORD_TAILS],
        len: 0,
        pressed: [GAP; 2],
    };
    let Some(c) = counted else {
        return grams;
    };
    let gap = [(GAP, GAP)];
    let press = |index: Option<usize>| match index {
        Some(index) => &presses[index][..sizes[index]],
        None => &gap[..],
    };
    let (a, b, c) = (
        press(c.checked_sub(2)),
        press(c.checked_sub(1)),
        press(Some(c)),
    );
    for (pressed, &(key, _)) in grams.pressed.iter_mut().zip(c) {
        *pressed = key;
    }
    let mut push = |tail: Tail, fingers: Tail, weight: f64| {
        if weight != 0.0 && tail[2] != GAP {
            grams.tails[usize::from(grams.len)] = (tail, fingers, weight);
            grams.len += 1;
        }
    };
    let (sa, sb) = (a.len() as f64, b.len() as f64);
    for &(ka, fa) in a {
        for &(kb, fb) in b {
            for &(kc, fc) in c {
                push([ka, kb, kc], [fa, fb, fc], 1.0 / sb);
            }
        }
    }
    let (ma, mb) = (1.0 - sa / sb, 1.0 - sb);
    for &(kb, fb) in b {
        for &(kc, fc) in c {
            push([GAP, kb, kc], [GAP, fb, fc], ma);
        }
    }
    for &(kc, fc) in c {
        push([GAP, GAP, kc], [GAP, GAP, fc], mb);
    }
    grams
}
/// One rule-delta window's mapped movement tail, its chord participant usage
/// (`GAP`s without combos) and whether its target character went untyped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RuleDeltaMapping {
    tail: Tail,
    usage: Tail,
    ignored: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct RuleDeltaWindow {
    bytes: [u8; 5],
    len: u8,
    target: u8,
    pub(crate) weight: f64,
}

impl RuleDeltaWindow {
    pub(crate) fn mapping(&self, mapped: [Option<usize>; 5]) -> RuleDeltaMapping {
        let tail = terminal(mapped, self.end());
        RuleDeltaMapping {
            tail,
            usage: [GAP; 3],
            ignored: tail[2] == GAP,
        }
    }

    pub(crate) fn text(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }

    pub(crate) fn target(&self) -> usize {
        self.target as usize
    }

    pub(crate) fn end(&self) -> usize {
        self.target() + 1
    }
}

fn terminal(keys: [Option<usize>; 5], len: usize) -> Tail {
    let mut tail = [GAP; 3];
    for j in 0..len.min(3) {
        tail[2 - j] = keys[len - 1 - j].map(|k| k as u8).unwrap_or(GAP);
    }
    tail
}

fn finger_terminal_at(
    alts: &crate::alt_fingers::Compiled,
    text: &[u8],
    mapped: &[Option<usize>],
    default_fingers: &[usize],
    end: usize,
) -> Tail {
    let resolved = alts.resolve(text, mapped, default_fingers);
    let mut tail = [GAP; 3];
    for j in 0..end.min(3) {
        tail[2 - j] = resolved[end - 1 - j]
            .map(|finger| finger as u8)
            .unwrap_or(GAP);
    }
    tail
}

fn default_finger_tail(tail: Tail, fingers: Tail, geometry: &Geometry) -> bool {
    tail.into_iter().zip(fingers).all(|(slot, finger)| {
        slot == GAP || finger != GAP && geometry.keys[slot as usize].finger == finger as usize
    })
}

fn add_pattern_with_fingers(
    raw: &mut Raw,
    kind: usize,
    slots: &[usize],
    fingers: &[usize],
    f: f64,
    geometry: &Geometry,
) {
    debug_assert_eq!(slots.len(), fingers.len());
    let keys: Vec<_> = slots
        .iter()
        .zip(fingers)
        .map(|(&slot, &finger)| crate::alt_fingers::with_finger(geometry.keys[slot], finger))
        .collect();
    if let Some(mana) = &mut raw.1 {
        let ids: Vec<_> = (0..keys.len()).collect();
        mana2_metrics::add_gram(mana, &keys, kind, &ids, f);
    }
    match kind {
        0 => {
            let key = keys[0];
            raw.0[USAGE + key.finger] += f;
            if key.main && !HomeAnchors::new(&geometry.keys).contains(key) {
                raw.0[OFF + key.finger] += f;
            }
            let travel = home_travel_for(key, &geometry.keys);
            raw.0[TRAVEL] += travel[0] * f;
            raw.0[VTRAVEL] += travel[1] * f;
            raw.0[LTRAVEL] += travel[2] * f;
        }
        1 | 2 => {
            let flags = pair_flags(keys[0], keys[1], slots[0] == slots[1]);
            add_bits(raw, if kind == 1 { flags.bi } else { flags.sk }, f);
            if kind == 1 {
                if flags.main {
                    raw.0[SRAF_DEN] += f;
                }
                if keys[0].main && keys[1].main && keys[0].finger == keys[1].finger {
                    raw.0[SFTRAVEL] += keys[0]
                        .vertical_delta(keys[1])
                        .hypot(keys[0].horizontal_delta(keys[1]))
                        * f;
                }
            }
        }
        3 => {
            let flags = tri_flags_with_settings(keys[0], keys[1], keys[2], geometry.rolls);
            add_bits(raw, flags.bits, f);
            if flags.main {
                raw.0[RHYTHM_DEN] += f;
            }
        }
        _ => unreachable!(),
    }
}

/// Add one contextual pattern using the fingers selected for this occurrence.
/// Coordinates and physical slot identity remain those of the mapped slots.
fn contribute_with_fingers(
    raw: &mut Raw,
    totals: &mut [f64; 4],
    tail: Tail,
    fingers: Tail,
    f: f64,
    geometry: &Geometry,
) {
    let [a, b, c] = tail;
    let [fa, fb, fc] = fingers;
    if c == GAP || fc == GAP {
        return;
    }
    totals[0] += f;
    add_pattern_with_fingers(raw, 0, &[c as usize], &[fc as usize], f, geometry);
    if b == GAP || fb == GAP {
        return;
    }
    totals[1] += f;
    add_pattern_with_fingers(
        raw,
        1,
        &[b as usize, c as usize],
        &[fb as usize, fc as usize],
        f,
        geometry,
    );
    if a == GAP || fa == GAP {
        return;
    }
    totals[2] += f;
    add_pattern_with_fingers(
        raw,
        2,
        &[a as usize, c as usize],
        &[fa as usize, fc as usize],
        f,
        geometry,
    );
    totals[3] += f;
    add_pattern_with_fingers(
        raw,
        3,
        &[a as usize, b as usize, c as usize],
        &[fa as usize, fb as usize, fc as usize],
        f,
        geometry,
    );
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
        add_gram(raw, &Gram::new(ids, len, kind, f), pos, geometry, 1.0);
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

fn contribute_maybe_fingers(
    raw: &mut Raw,
    totals: &mut [f64; 4],
    tail: Tail,
    fingers: Option<Tail>,
    f: f64,
    positions: &[usize],
    geometry: &Geometry,
) {
    if let Some(fingers) = fingers.filter(|&value| !default_finger_tail(tail, value, geometry)) {
        contribute_with_fingers(raw, totals, tail, fingers, f, geometry);
    } else {
        contribute(raw, totals, tail, f, positions, geometry);
    }
}

/// Accumulate mapped physical counts using contextual fingers when present.
/// The slot-only path deliberately retains the established ordering and
/// arithmetic so layouts without active alternate fingerings remain exact.
pub(crate) fn raw_from_counts(
    layout: &ak::Layout,
    counts: &Counts,
    weights: &Weights,
    mana2: bool,
) -> (Raw, [f64; 4]) {
    let geometry = Geometry::with_rolls(crate::action_ui::physical_keys(layout), weights.rolls());
    raw_from_counts_with_geometry(layout, counts, mana2, &geometry)
}

/// Reuse physical geometry when only action rules change. The ordinary entry
/// point above deliberately keeps constructing its own geometry so existing
/// evaluator behavior and call sites remain unchanged.
pub(crate) fn raw_from_counts_with_geometry(
    layout: &ak::Layout,
    counts: &Counts,
    mana2: bool,
    geometry: &Geometry,
) -> (Raw, [f64; 4]) {
    let mut raw = if mana2 {
        Raw::with_mana2()
    } else {
        Raw::default()
    };
    let physical = [
        &counts.tables[0],
        &counts.tables[1],
        &counts.skip,
        &counts.tables[2],
    ];
    // Denominators are physical population masses and do not depend on how
    // contextual finger variants split one slot sequence.
    let totals = std::array::from_fn(|kind| physical[kind].entries().map(|(_, value)| value).sum());
    if let (Some(tables), Some(skip)) = (&counts.finger_tables, &counts.finger_skip) {
        let effective = [&tables[0], &tables[1], skip, &tables[2]];
        for (kind, table) in effective.into_iter().enumerate() {
            for (pattern, &frequency) in table {
                if frequency == 0.0 {
                    continue;
                }
                add_pattern_with_fingers(
                    &mut raw,
                    kind,
                    &pattern.slots,
                    &pattern.fingers,
                    frequency,
                    geometry,
                );
            }
        }
    } else {
        let positions: Vec<_> = (0..layout.slots.len()).collect();
        for (kind, table) in physical.into_iter().enumerate() {
            for (keys, frequency) in table.entries() {
                if frequency == 0.0 {
                    continue;
                }
                let mut ids = [0; 3];
                ids[..keys.len()].copy_from_slice(keys);
                add_gram(
                    &mut raw,
                    &Gram::new(ids, keys.len(), kind, frequency),
                    &positions,
                    geometry,
                    1.0,
                );
            }
        }
    }
    (raw, totals)
}

pub(crate) fn pattern_raw(geometry: &Geometry, kind: usize, pattern: &FingerPattern) -> Raw {
    let mut raw = Raw::default();
    add_pattern_with_fingers(
        &mut raw,
        kind,
        &pattern.slots,
        &pattern.fingers,
        1.0,
        geometry,
    );
    raw
}

/// Screening keys: a tail, its contextual fingers (`GAP`s for defaults) and
/// whether it is combo usage rather than movement.
#[inline]
fn pack_key(tail: Tail, fingers: Tail, usage: bool) -> u64 {
    u64::from(usage) << 48
        | u64::from(tail[0]) << 40
        | u64::from(tail[1]) << 32
        | u64::from(tail[2]) << 24
        | u64::from(fingers[0]) << 16
        | u64::from(fingers[1]) << 8
        | u64::from(fingers[2])
}

/// `raw += weight * unit`, field by field.
pub(crate) fn add_scaled(
    raw: &mut Raw,
    totals: &mut [f64; 4],
    unit: &(Raw, [f64; 4]),
    weight: f64,
) {
    let (unit_raw, unit_totals) = unit;
    for (total, unit) in totals.iter_mut().zip(unit_totals) {
        *total += unit * weight;
    }
    for (field, unit) in raw.0.iter_mut().zip(&unit_raw.0) {
        *field += unit * weight;
    }
    if let (Some(stats), Some(unit)) = (&mut raw.1, &unit_raw.1) {
        for (field, unit) in stats.0.iter_mut().zip(&unit.0) {
            *field += unit * weight;
        }
        for (field, unit) in stats.1.bigrams.iter_mut().zip(&unit.1.bigrams) {
            *field += unit * weight;
        }
        for (field, unit) in stats.1.skipgrams.iter_mut().zip(&unit.1.skipgrams) {
            *field += unit * weight;
        }
    }
}

fn usage_key(usage: Tail) -> u64 {
    pack_key(usage, [GAP; 3], true)
}

fn unpack_key(key: u64) -> (Tail, Option<Tail>, bool) {
    let byte = |shift: u32| (key >> shift) as u8;
    let fingers = [byte(16), byte(8), byte(0)];
    (
        [byte(40), byte(32), byte(24)],
        (fingers != [GAP; 3]).then_some(fingers),
        key >> 48 != 0,
    )
}

/// Marks `ids` in a bitmap over `contexts` context IDs.
/// Chord contexts carry the character after the counted one only to show a
/// combo starting there. Contexts no combo touches, whose counted character
/// one key alone types, map the same without it: those differing only in it
/// become one, as in the plain corpus. Alternate fingers can read it, so
/// callers merge only without them. Weights are whole counts, so sums stay
/// exact.
fn merge_unchorded(
    windows: &[Window],
    combos: &crate::combos::Program,
    keys: &crate::action_fast::KeyState,
) -> Vec<Window> {
    let mut merged: Vec<Window> = Vec::with_capacity(windows.len());
    let mut index: HashMap<([u8; 5], usize, bool), usize> = HashMap::new();
    for window in windows {
        if combos.touches(window.text(), window.left_open)
            || !keys.unique(window.text()[window.target()])
        {
            merged.push(window.clone());
            continue;
        }
        let mut bytes = [0; 5];
        bytes[..window.end()].copy_from_slice(&window.text()[..window.end()]);
        let key = (bytes, window.end(), window.left_open);
        match index.get(&key) {
            Some(&at) => merged[at].weight += window.weight,
            None => {
                index.insert(key, merged.len());
                merged.push(Window {
                    bytes,
                    len: window.end() as u8,
                    ..window.clone()
                });
            }
        }
    }
    merged
}

fn mark_contexts(marks: &mut Vec<u64>, contexts: usize, ids: &[u32]) {
    marks.resize((contexts + 63) / 64, 0);
    for &id in ids {
        marks[id as usize / 64] |= 1 << (id % 64);
    }
}

/// Replaces `ids` with the marked context IDs in ascending order, leaving
/// the bitmap all zero for its next use.
fn read_marks(marks: &mut [u64], ids: &mut Vec<u32>) {
    ids.clear();
    for (word, bits) in marks.iter_mut().enumerate() {
        let mut bits = std::mem::take(bits);
        while bits != 0 {
            ids.push((word * 64) as u32 + bits.trailing_zeros());
            bits &= bits - 1;
        }
    }
}

/// Net frequency per screening key, in first-insertion order, reusing its
/// storage across trials.
#[derive(Clone, Default)]
pub(crate) struct NetTable {
    slots: Vec<(u64, u32)>,
    entries: Vec<(u64, f64)>,
}

impl NetTable {
    const EMPTY: u64 = u64::MAX;

    #[inline(always)]
    fn add(&mut self, key: u64, weight: f64) {
        if self.entries.len() * 2 >= self.slots.len() {
            self.grow();
        }
        let mask = self.slots.len() - 1;
        let mut slot = (key.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 32) as usize & mask;
        loop {
            let (stored, index) = self.slots[slot];
            if stored == key {
                self.entries[index as usize].1 += weight;
                return;
            }
            if stored == Self::EMPTY {
                self.slots[slot] = (key, self.entries.len() as u32);
                self.entries.push((key, weight));
                return;
            }
            slot = (slot + 1) & mask;
        }
    }

    #[cold]
    fn grow(&mut self) {
        let size = (self.slots.len() * 2).max(4096);
        self.slots.clear();
        self.slots.resize(size, (Self::EMPTY, 0));
        let mask = size - 1;
        for (index, &(key, _)) in self.entries.iter().enumerate() {
            let mut slot = (key.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 32) as usize & mask;
            while self.slots[slot].0 != Self::EMPTY {
                slot = (slot + 1) & mask;
            }
            self.slots[slot] = (key, index as u32);
        }
    }

    fn get(&self, key: u64) -> Option<f64> {
        if self.slots.is_empty() {
            return None;
        }
        let mask = self.slots.len() - 1;
        let mut slot = (key.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 32) as usize & mask;
        loop {
            let (stored, index) = self.slots[slot];
            if stored == key {
                return Some(self.entries[index as usize].1);
            }
            if stored == Self::EMPTY {
                return None;
            }
            slot = (slot + 1) & mask;
        }
    }

    /// Every key's net frequency, emptying the table.
    fn drain(&mut self) -> impl Iterator<Item = (u64, f64)> + '_ {
        for &(key, _) in &self.entries {
            let mask = self.slots.len() - 1;
            let mut slot = (key.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 32) as usize & mask;
            while self.slots[slot].0 != key {
                slot = (slot + 1) & mask;
            }
            self.slots[slot].0 = Self::EMPTY;
        }
        self.entries.drain(..)
    }
}

fn add_tail_mana2_with_fingers(
    raw: &mut Raw,
    tail: Tail,
    fingers: Tail,
    frequency: f64,
    geometry: &Geometry,
) {
    let Some(mana) = &mut raw.1 else { return };
    let [a, b, c] = tail;
    let [fa, fb, fc] = fingers;
    if c == GAP || fc == GAP {
        return;
    }
    let c = crate::alt_fingers::with_finger(geometry.keys[c as usize], fc as usize);
    mana2_metrics::add_gram(mana, &[c], 0, &[0], frequency);
    if b == GAP || fb == GAP {
        return;
    }
    let b = crate::alt_fingers::with_finger(geometry.keys[b as usize], fb as usize);
    mana2_metrics::add_gram(mana, &[b, c], 1, &[0, 1], frequency);
    if a == GAP || fa == GAP {
        return;
    }
    let a = crate::alt_fingers::with_finger(geometry.keys[a as usize], fa as usize);
    let keys = [a, b, c];
    mana2_metrics::add_gram(mana, &keys, 2, &[0, 2], frequency);
    mana2_metrics::add_gram(mana, &keys, 3, &[0, 1, 2], frequency);
}

// Physical geometry never moves with bindings. Compile the disjoint event
// fields of a terminal history into one mask; retain floating travel values in
// Geometry. No frequency, binding, weight or candidate state lives here.
struct TailContributions {
    unary: Vec<u128>,
    pair: Vec<u128>,
    triple: Vec<u128>,
    n: usize,
    mana2: Option<mana2_metrics::GeometryContributions>,
    mana2_only: bool,
}

/// Numeric state for sparse action-rule proposals. It uses the same compiled
/// contribution tables as the normal incremental optimizer, but does not
/// construct physical histograms. Canonical finalist evaluation still does.
#[derive(Clone)]
pub(crate) struct RuleDeltaAggregate {
    raw: Raw,
    totals: [f64; 4],
    ignored_characters: f64,
    contributions: Arc<TailContributions>,
    geometry: Arc<Geometry>,
    positions: Arc<[usize]>,
    speed_populations: [f64; 2],
}

impl RuleDeltaAggregate {
    /// `mana2` also accumulates the physical stats used by mana/aklgg.
    pub(crate) fn new(geometry: Arc<Geometry>, mana2: bool, speed_populations: [f64; 2]) -> Self {
        let positions: Arc<[usize]> = (0..geometry.keys.len()).collect::<Vec<_>>().into();
        let mut contributions = TailContributions::new(&geometry);
        if mana2 {
            contributions.enable_mana2(&geometry);
        }
        Self {
            raw: if mana2 {
                Raw::with_mana2()
            } else {
                Raw::default()
            },
            totals: [0.0; 4],
            ignored_characters: 0.0,
            contributions: Arc::new(contributions),
            geometry,
            positions,
            speed_populations,
        }
    }

    pub(crate) fn add_mapping(&mut self, mapped: RuleDeltaMapping, window: RuleDeltaWindow) {
        self.contributions.apply_contextual(
            &mut self.raw,
            &mut self.totals,
            mapped.tail,
            None,
            window.weight,
            &self.positions,
            &self.geometry,
        );
        self.contributions.apply_combo_usage(
            &mut self.raw,
            &mut self.totals,
            mapped.usage,
            window.weight,
            &self.geometry,
        );
        if mapped.ignored {
            self.ignored_characters += window.weight;
        }
    }

    pub(crate) fn replace_mapping(
        &mut self,
        old: RuleDeltaMapping,
        new: RuleDeltaMapping,
        window: RuleDeltaWindow,
    ) {
        if old.tail != new.tail {
            self.contributions.replace_default(
                &mut self.raw,
                &mut self.totals,
                old.tail,
                new.tail,
                window.weight,
                &self.geometry,
            );
        }
        if old.usage != new.usage {
            self.contributions.apply_combo_usage(
                &mut self.raw,
                &mut self.totals,
                old.usage,
                -window.weight,
                &self.geometry,
            );
            self.contributions.apply_combo_usage(
                &mut self.raw,
                &mut self.totals,
                new.usage,
                window.weight,
                &self.geometry,
            );
        }
        let old_ignored = old.ignored;
        let new_ignored = new.ignored;
        if old_ignored != new_ignored {
            self.ignored_characters += if new_ignored {
                window.weight
            } else {
                -window.weight
            };
        }
    }

    #[cfg(test)]
    pub(crate) fn totals(&self) -> [f64; 4] {
        self.totals
    }

    pub(crate) fn metrics(&self) -> Metrics {
        metrics_totals(&self.raw, &self.totals)
    }

    /// Lower-is-better score in `mode`, as `action_summary` computes it.
    pub(crate) fn objective(&self, metrics: &Metrics, weights: &Weights, mode: &str) -> f64 {
        match &self.raw.1 {
            Some(raw) if mode == "aklgg" => {
                weights.physical_score(&mana_stats(raw, self.totals, self.speed_populations))
            }
            _ => breakdown(metrics, weights).net,
        }
    }

    pub(crate) fn ignored_characters(&self) -> f64 {
        self.ignored_characters
    }

    pub(crate) fn usable(&self) -> bool {
        self.totals[0] > 0.0 && self.totals[1] > 0.0
    }
}

impl TailContributions {
    fn new(g: &Geometry) -> Self {
        assert!(N_RAW <= 128);
        let travel = bit(TRAVEL) | bit(VTRAVEL) | bit(LTRAVEL) | bit(SFTRAVEL);
        // OR is exact only for disjoint fields. Fail at setup if future metric
        // definitions introduce multiplicity rather than silently losing it.
        let join = |left: u128, right: u128| {
            assert_eq!(left & right, 0, "overlapping tail contribution fields");
            assert_eq!((left | right) & travel, 0, "travel must remain numeric");
            left | right
        };
        let unary: Vec<u128> = g
            .keys
            .iter()
            .enumerate()
            .map(|(index, key)| {
                let off = if key.main && !g.is_home[index] {
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
                    let bits = g.tri.raw((a * g.n + b) * g.n + c);
                    triple.push(join(join(pair[b * g.n + c], g.pair[a * g.n + c].sk), bits));
                }
            }
        }
        Self {
            unary,
            pair,
            triple,
            n: g.n,
            mana2: None,
            mana2_only: false,
        }
    }

    fn enable_mana2(&mut self, g: &Geometry) {
        if self.mana2.is_none() {
            // Copy the geometry's cached classification instead of rebuilding it.
            self.mana2 = Some(g.mana2().clone());
        }
        self.mana2_only = false;
    }

    #[cfg(test)]
    fn enable_mana2_only(&mut self, g: &Geometry) {
        self.enable_mana2(g);
        self.mana2_only = true;
    }

    #[inline(always)]
    fn apply_mana2_raw(&self, raw: &mut mana2_metrics::RawStats, tail: Tail, f: f64) {
        let [a, b, c] = tail;
        if c == GAP {
            return;
        }
        let c = c as usize;
        let compiled = self
            .mana2
            .as_ref()
            .expect("Mana2 geometry cache must be enabled with Mana2 stats");
        if b == GAP {
            compiled.add_one(raw, c, f);
        } else if a == GAP {
            compiled.add_two(raw, b as usize, c, f);
        } else {
            compiled.add_three(raw, a as usize, b as usize, c, f);
        }
    }

    fn apply_mana2(&self, raw: &mut Raw, tail: Tail, f: f64) {
        if let Some(raw) = &mut raw.1 {
            self.apply_mana2_raw(raw, tail, f);
        }
    }

    #[inline(always)]
    fn apply_mana2_active(
        &self,
        raw: &mut mana2_metrics::RawStats,
        tail: Tail,
        f: f64,
        active: mana2_metrics::ActiveStats,
    ) {
        let [a, b, c] = tail;
        if c == GAP {
            return;
        }
        let compiled = self.mana2.as_ref().unwrap();
        if b == GAP {
            compiled.add_one_active(raw, c as usize, f, active);
        } else if a == GAP {
            compiled.add_two_active(raw, b as usize, c as usize, f, active);
        } else {
            compiled.add_three_active(raw, a as usize, b as usize, c as usize, f, active);
        }
    }

    #[inline]
    fn replace_active(
        &self,
        raw: &mut Raw,
        totals: &mut [f64; 4],
        old: Tail,
        new: Tail,
        f: f64,
        active: mana2_metrics::ActiveStats,
    ) {
        let mana2 = raw.1.as_mut().unwrap();
        Self::apply_mana2_totals(totals, old, -f);
        self.apply_mana2_active(mana2, old, -f, active);
        Self::apply_mana2_totals(totals, new, f);
        self.apply_mana2_active(mana2, new, f, active);
    }

    #[inline(always)]
    fn replace_active_sparse(
        &self,
        raw: &mut Raw,
        totals: &mut [f64; 4],
        old: Tail,
        new: Tail,
        f: f64,
        active: mana2_metrics::ActiveStats,
    ) {
        // Each denominator retains its old-removal/new-addition order.
        // Independent denominator fields can be updated together, allowing
        // vectorization of the overwhelmingly common complete-tail case.
        if old.iter().all(|&slot| slot != GAP) && new.iter().all(|&slot| slot != GAP) {
            for total in totals {
                *total -= f;
                *total += f;
            }
        } else {
            Self::apply_mana2_totals(totals, old, -f);
            Self::apply_mana2_totals(totals, new, f);
        }
        let raw = raw.1.as_mut().unwrap();
        let compiled = self.mana2.as_ref().unwrap();
        compiled.add_tail_active_sparse(raw, old, -f, active);
        compiled.add_tail_active_sparse(raw, new, f, active);
    }

    #[inline(always)]
    fn replace_integer_totals(totals: &mut [f64; 4], old: Tail, new: Tail, f: f64) {
        let membership = |[a, b, c]: Tail| {
            if c == GAP {
                0_u8
            } else if b == GAP {
                1
            } else if a == GAP {
                3
            } else {
                15
            }
        };
        let old_membership = membership(old);
        let new_membership = membership(new);
        // The integer budget proves that removing and adding a common
        // contribution would return each denominator to precisely its value.
        let mut remove = old_membership & !new_membership;
        while remove != 0 {
            let field = remove.trailing_zeros() as usize;
            totals[field] -= f;
            remove &= remove - 1;
        }
        let mut add = new_membership & !old_membership;
        while add != 0 {
            let field = add.trailing_zeros() as usize;
            totals[field] += f;
            add &= add - 1;
        }
    }

    #[inline(always)]
    fn replace_active_integer(
        &self,
        raw: &mut Raw,
        totals: &mut [f64; 4],
        old: Tail,
        new: Tail,
        f: f64,
        active: mana2_metrics::ActiveStats,
    ) {
        Self::replace_integer_totals(totals, old, new, f);
        self.mana2.as_ref().unwrap().replace_tail_active_integer(
            raw.1.as_mut().unwrap(),
            old,
            new,
            f,
            active,
        );
    }

    #[inline(always)]
    fn apply_mana2_totals(totals: &mut [f64; 4], tail: Tail, f: f64) {
        let [a, b, c] = tail;
        if c == GAP {
            return;
        }
        totals[0] += f;
        if b != GAP {
            totals[1] += f;
            if a != GAP {
                totals[2] += f;
                totals[3] += f;
            }
        }
    }

    /// Replace one default-finger history while retaining the established
    /// floating-point order: remove the old totals and raw contribution, then
    /// add the new totals and raw contribution.
    #[inline]
    fn replace_default(
        &self,
        raw: &mut Raw,
        totals: &mut [f64; 4],
        old: Tail,
        new: Tail,
        f: f64,
        geometry: &Geometry,
    ) {
        if !self.mana2_only {
            // Counting flags set by both tails would cancel; move only the
            // differing ones. Numeric fields are disjoint from the flags.
            let from = self.apply_numeric(raw, totals, old, -f, geometry);
            let to = self.apply_numeric(raw, totals, new, f, geometry);
            crate::move_bits(raw, from, to, f);
            self.apply_mana2(raw, old, -f);
            self.apply_mana2(raw, new, f);
            return;
        }
        let mana2 = raw.1.as_mut().expect("Mana2 totals must be enabled");
        Self::apply_mana2_totals(totals, old, -f);
        self.apply_mana2_raw(mana2, old, -f);
        Self::apply_mana2_totals(totals, new, f);
        self.apply_mana2_raw(mana2, new, f);
    }

    fn apply(&self, raw: &mut Raw, totals: &mut [f64; 4], tail: Tail, f: f64, g: &Geometry) {
        let [a, b, c] = tail;
        if c == GAP {
            return;
        }
        if self.mana2_only {
            // Keep this branch's arithmetic in the same order as the
            // historical generic update.
            totals[0] += f;
            if b != GAP {
                totals[1] += f;
                if a != GAP {
                    totals[2] += f;
                    totals[3] += f;
                }
            }
            self.apply_mana2(raw, tail, f);
            return;
        }
        let bits = self.apply_numeric(raw, totals, tail, f, g);
        add_bits(raw, bits, f);
        self.apply_mana2(raw, tail, f);
    }

    /// Add `tail`'s totals and numeric fields (travel) and return its counting
    /// flags, which the caller adds or moves. A gap tail contributes nothing.
    #[inline(always)]
    fn apply_numeric(
        &self,
        raw: &mut Raw,
        totals: &mut [f64; 4],
        tail: Tail,
        f: f64,
        g: &Geometry,
    ) -> u128 {
        let [a, b, c] = tail;
        if c == GAP {
            return 0;
        }
        let c = c as usize;
        totals[0] += f;
        // Preserve each field's arithmetic, including zero-valued travel terms.
        raw.0[TRAVEL] += g.home[c][0] * f;
        raw.0[VTRAVEL] += g.home[c][1] * f;
        raw.0[LTRAVEL] += g.home[c][2] * f;
        if b == GAP {
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
        }
    }

    fn apply_contextual(
        &self,
        raw: &mut Raw,
        totals: &mut [f64; 4],
        tail: Tail,
        fingers: Option<Tail>,
        f: f64,
        positions: &[usize],
        geometry: &Geometry,
    ) {
        let contextual = fingers.filter(|&value| !default_finger_tail(tail, value, geometry));
        if !self.mana2_only {
            contribute_maybe_fingers(raw, totals, tail, contextual, f, positions, geometry);
        } else if let Some(fingers) = contextual {
            let [a, b, c] = tail;
            if c == GAP {
                return;
            }
            totals[0] += f;
            if b != GAP {
                totals[1] += f;
                if a != GAP {
                    totals[2] += f;
                    totals[3] += f;
                }
            }
            add_tail_mana2_with_fingers(raw, tail, fingers, f, geometry);
        } else {
            self.apply(raw, totals, tail, f, geometry);
        }
    }

    fn apply_combo_usage(
        &self,
        raw: &mut Raw,
        totals: &mut [f64; 4],
        usage: Tail,
        frequency: f64,
        geometry: &Geometry,
    ) {
        if !self.mana2_only {
            contribute_combo_usage(raw, totals, usage, frequency, geometry);
            return;
        }
        let Some(mana) = &mut raw.1 else { return };
        for slot in usage.into_iter().take_while(|&slot| slot != GAP) {
            totals[0] += frequency;
            let key = geometry.keys[slot as usize];
            mana2_metrics::add_gram(mana, &[key], 0, &[0], frequency);
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct CandidateScore {
    #[cfg(test)]
    pub sfb: f64,
    #[cfg(test)]
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
    for i in 0..4 {
        let a = pct(raw.0[USAGE + i].max(0.0), totals[0]);
        let b = pct(raw.0[USAGE + 7 - i].max(0.0), totals[0]);
        let contribution = (a + b) * w.0[USAGE_WEIGHT + i];
        if contribution >= 0.0 {
            penalty += contribution;
        } else {
            bonus -= contribution;
        }
    }
    CandidateScore {
        #[cfg(test)]
        sfb: value(SFB),
        #[cfg(test)]
        sfs: value(SFS),
        score: penalty - bonus,
    }
}

// Reused across candidates. Only these numeric totals and tail patches are
// needed to accept a swap. No report, histogram or textual representation.
pub(crate) struct Proposal {
    raw: Raw,
    totals: [f64; 4],
    speed_populations: [f64; 2],
    changes: Vec<(usize, Tail)>,
    finger_changes: Vec<(usize, Tail)>,
    combo_usage_changes: Vec<(usize, Tail)>,
    affected: Vec<u32>,
    movement: Move,
    valid: bool,
    projected: bool,
    omitted_changes: bool,
    integer_plan_ids: Vec<u64>,
    integer_plans: Vec<mana2_metrics::IntegerReplacement>,
    direct_plan_ids: Vec<u32>,
    direct_plans: Vec<mana2_metrics::IntegerReplacement>,
    direct_plan_ranges: Vec<(u32, u32)>,
    direct_numeric_updates: Vec<mana2_metrics::NumericUpdate>,
    boolean_frequencies: Vec<f64>,
    numeric_context_bits: Vec<u64>,
    pub(crate) examined: usize,
    pub(crate) all_contexts: bool,
    /// Screened: fields come from net frequencies per distinct tail, not
    /// context by context, and differ in the last bits until `materialize`.
    aggregated: bool,
    net: NetTable,
    /// Screening's contextual-finger contributions at frequency one, for the
    /// geometry they were computed with.
    contextual: HashMap<u64, Box<(Raw, [f64; 4])>>,
    contextual_geometry: Arc<Geometry>,
    /// Screened tail groups that move and their new tail; `materialize`
    /// expands them into per-context changes.
    group_changes: Vec<(u32, Tail)>,
    group_marks: Vec<u64>,
    /// Scratch bitmap over context IDs for affected-context unions; all zero
    /// between uses.
    context_marks: Vec<u64>,
    /// The mapper memo tables of earlier trials.
    memos: crate::action_fast::MemoTables,
    /// Combo contexts' new chord tails, ascending.
    chord_changes: Vec<(usize, ChordGrams)>,
    /// Screened `chord_direct` contexts whose grams the move relabels,
    /// ascending; a replay adds them to the changes (`expand_chord_relabels`).
    chord_relabels: Vec<u32>,
}

impl Proposal {
    pub(crate) fn new() -> Self {
        Self {
            raw: Raw::default(),
            totals: [0.0; 4],
            speed_populations: [0.0; 2],
            changes: Vec::new(),
            finger_changes: Vec::new(),
            combo_usage_changes: Vec::new(),
            affected: Vec::new(),
            movement: Move::pair(0, 0),
            valid: false,
            projected: false,
            omitted_changes: false,
            integer_plan_ids: Vec::new(),
            integer_plans: Vec::new(),
            direct_plan_ids: Vec::new(),
            direct_plans: Vec::new(),
            direct_plan_ranges: Vec::new(),
            direct_numeric_updates: Vec::new(),
            boolean_frequencies: Vec::new(),
            numeric_context_bits: Vec::new(),
            aggregated: false,
            net: NetTable::default(),
            contextual: HashMap::new(),
            contextual_geometry: Arc::new(Geometry::new(Vec::new())),
            group_changes: Vec::new(),
            group_marks: Vec::new(),
            context_marks: Vec::new(),
            memos: Default::default(),
            chord_changes: Vec::new(),
            chord_relabels: Vec::new(),
            examined: 0,
            all_contexts: false,
        }
    }

    pub(crate) fn adjacent_mass(&self) -> f64 {
        assert!(self.valid);
        self.totals[1]
    }

    pub(crate) fn score(&self, w: &Weights) -> CandidateScore {
        assert!(self.valid);
        candidate_score(&self.raw, &self.totals, w)
    }
    pub(crate) fn metrics(&self) -> Metrics {
        metrics_totals(&self.raw, &self.totals)
    }

    pub(crate) fn physical_score(&self, weights: &Weights) -> f64 {
        let raw = self.raw.1.as_ref().expect("physical stats must be enabled");
        weights.physical_score(&mana_stats(raw, self.totals, self.speed_populations))
    }

    #[cfg(test)]
    pub(crate) fn mana2_score(&self, weights: &mana2_metrics::Weights) -> f64 {
        assert!(self.valid);
        let raw = self.raw.1.as_ref().expect("mana2 totals must be enabled");
        -mana2_metrics::score(
            &mana_stats(raw, self.totals, self.speed_populations),
            weights,
        )
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
        source_path: PathBuf::new(),
        include_spacegrams: true,
        grams: Vec::new(),
        by_symbol: Vec::new(),
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
}

#[derive(Clone, Copy)]
struct TrialContext {
    frequency: f64,
    tail_id: u32,
    tail: Tail,
    source_flags: u8,
}

struct DirectTailGroup {
    tail: Tail,
    tail_id: u32,
    frequency: f64,
    contexts: Vec<u32>,
}

impl TrialContext {
    #[inline(always)]
    fn direct(self) -> bool {
        self.source_flags & 1 != 0
    }

    #[cfg(test)]
    #[inline(always)]
    fn source_start(self) -> usize {
        (self.source_flags >> 1) as usize
    }

    fn packed_tail_id(tail: Tail, radix: usize) -> u32 {
        let slots = tail.map(|slot| {
            if slot == GAP {
                radix - 1
            } else {
                slot as usize
            }
        });
        ((slots[0] * radix + slots[1]) * radix + slots[2]) as u32
    }

    fn set_tail(&mut self, tail: Tail, radix: usize) {
        self.tail = tail;
        self.tail_id = Self::packed_tail_id(tail, radix);
    }
}

pub(crate) struct Incremental {
    corpus: NgramCorpus,
    tails: Vec<Tail>,
    finger_tails: Option<Vec<Tail>>,
    combo_usage: Vec<Tail>,
    postings: [Vec<u32>; 256],
    /// Left-open contexts by first byte, kept only with combos: a chord that
    /// began before such a context may cover its start.
    open_postings: [Vec<u32>; 256],
    direct_postings: [Vec<u32>; 256],
    ambiguous_postings: [Vec<u32>; 256],
    direct_contexts: Vec<bool>,
    /// Bindings typing each position of contexts no effort choice reaches
    /// (`action_fast::StaticBindings`): their tails only move with these
    /// bindings, so trials look them up and select them by binding.
    static_contexts: Vec<Option<[u8; 5]>>,
    /// Static contexts by the bindings typing their tails.
    binding_postings: Vec<Vec<u32>>,
    /// The contexts that are not static, ascending, kept with static ones.
    nonstatic: Vec<u32>,
    /// Combo contexts' chord tails; their own tails, fingers and usage stay
    /// gaps.
    chord_grams: Vec<Option<Box<ChordGrams>>>,
    /// Static and direct contexts by the bindings typing their tails, which
    /// fix the tail: screened trials move each group once
    /// (`enable_aggregation`).
    tail_groups: Vec<TailGroup>,
    /// Each context's tail group, `NO_GROUP` when it has none.
    group_of: Vec<u32>,
    /// Tail groups by the bindings typing their tails.
    group_postings: Vec<Vec<u32>>,
    /// `direct_postings` without the direct contexts tail groups move.
    grouped_postings: [Vec<u32>; 256],
    /// Contexts some combo plan can change (`combos::Program::touches`).
    /// All others map exactly as without combos and keep the plain paths.
    combo_contexts: Vec<bool>,
    /// Combo contexts mapped without weighing one key against another: which
    /// keys can type each press depends on bindings and text, not slots, so
    /// every move keeps the same presses, which follow their keys, and only
    /// relabels the grams' slots (`ChordGrams::moved`).
    chord_direct: Vec<bool>,
    /// Each combo context's plan with declared slots (`chord_plans` index
    /// by context).
    chord_plans: Vec<crate::combos::ChordPlan>,
    chord_plan_of: Vec<u32>,
    /// Combo contexts by the (declared) slots their chords press: moving a
    /// chord's key changes only these.
    chord_postings: Vec<Vec<u32>>,
    trial_contexts: Vec<TrialContext>,
    prepared_contexts: Vec<crate::action_fast::PreparedContext>,
    direct_groups: Vec<DirectTailGroup>,
    exact_integer_mass: Option<u64>,
    raw: Raw,
    totals: [f64; 4],
    geometry: Arc<Geometry>,
    positions: Vec<usize>,
    arrangement: Vec<usize>,
    commits: usize,
    contributions: TailContributions,
    program: crate::action_fast::Program,
    keys: crate::action_fast::KeyState,
    alt_fingers: crate::alt_fingers::Compiled,
    default_fingers: Vec<usize>,
    combos: Option<crate::combos::Program>,
    finite_effort: bool,
    /// Trials screen by net tail frequencies (`enable_aggregation`).
    aggregate: bool,
    active_mana2: Option<mana2_metrics::ActiveStats>,
    projection_mass: f64,
    projection_min_frequency: f64,
}

#[inline(always)]
fn move_tail(mut tail: Tail, movement: Move) -> Tail {
    let [a, b, c] = movement.slots.map(|slot| slot as u8);
    for slot in &mut tail {
        if *slot == a {
            *slot = b;
        } else if *slot == b {
            *slot = a;
        }
    }
    if movement.len == 3 {
        for slot in &mut tail {
            if *slot == b {
                *slot = c;
            } else if *slot == c {
                *slot = b;
            }
        }
    }
    tail
}

// A chord is one event in the physical sequence, but every participating key
// still contributes its own finger usage and distance from home. Movement to,
// within and from it is excluded by the GAPs in the separately cached tail.
fn contribute_combo_usage(
    raw: &mut Raw,
    totals: &mut [f64; 4],
    usage: Tail,
    frequency: f64,
    geometry: &Geometry,
) {
    if usage[0] == GAP {
        return;
    }
    for slot in usage.into_iter().take_while(|&slot| slot != GAP) {
        totals[0] += frequency;
        let slot = slot as usize;
        add_pattern_with_fingers(
            raw,
            0,
            &[slot],
            &[geometry.keys[slot].finger],
            frequency,
            geometry,
        );
    }
}

impl Incremental {
    /// Permit the mapper's finite-effort fast path during candidate trials.
    /// The caller promises that its effort callback is pure and returns a
    /// finite value for every physical key and reachable history, with finite
    /// sums of the two lookups used by the mapper. Bounding every lookup's
    /// absolute value by `f64::MAX / 2.0` satisfies both requirements. This
    /// contract must remain fixed for the lifetime of this cache.
    pub(crate) fn enable_finite_effort(&mut self) {
        self.finite_effort = true;
    }

    pub(crate) fn enable_mana2(&mut self) {
        self.active_mana2 = None;
        self.contributions.enable_mana2(&self.geometry);
        self.raw.1 = Some(mana2_metrics::RawStats::default());
        for (id, (tail, window)) in self
            .tails
            .iter()
            .zip(self.corpus.windows.iter())
            .enumerate()
        {
            if let Some(fingers) = self.finger_tails.as_ref().map(|tails| tails[id]) {
                add_tail_mana2_with_fingers(
                    &mut self.raw,
                    *tail,
                    fingers,
                    window.weight,
                    &self.geometry,
                );
            } else {
                self.contributions
                    .apply_mana2(&mut self.raw, *tail, window.weight);
            }
            for slot in self.combo_usage[id]
                .into_iter()
                .take_while(|&slot| slot != GAP)
            {
                let key = self.geometry.keys[slot as usize];
                mana2_metrics::add_gram(
                    self.raw.1.as_mut().unwrap(),
                    &[key],
                    0,
                    &[0],
                    window.weight,
                );
            }
            for (tail, fingers, sign) in self.chord_grams[id].iter().flat_map(|grams| grams.iter())
            {
                if self.finger_tails.is_some() {
                    add_tail_mana2_with_fingers(
                        &mut self.raw,
                        tail,
                        fingers,
                        sign * window.weight,
                        &self.geometry,
                    );
                } else {
                    self.contributions
                        .apply_mana2(&mut self.raw, tail, sign * window.weight);
                }
            }
        }
    }

    /// Enable the dedicated Mana2 objective. Candidate updates keep Mana2's
    /// raw fields and population totals current while deliberately leaving the
    /// unrelated AKLER raw fields stale. Callers in this mode must only read
    /// `mana2_score` (or the Mana2 half of `raw_totals`).
    #[cfg(test)]
    pub(crate) fn enable_mana2_only(&mut self) {
        self.enable_mana2();
        self.contributions.enable_mana2_only(&self.geometry);
    }

    /// Project rejected trials onto objective dependencies. Accepted proposals
    /// are replayed through the complete Mana2 path before committing.
    #[cfg(test)]
    pub(crate) fn enable_mana2_score_only(&mut self, weights: &mana2_metrics::Weights) {
        self.enable_mana2_only();
        if self.combos.is_none() && self.finger_tails.is_none() {
            self.projection_mass = self.corpus.windows.iter().map(|w| w.weight.abs()).sum();
            self.projection_min_frequency = self
                .corpus
                .windows
                .iter()
                .map(|w| w.weight)
                .filter(|&w| w > 0.0)
                .fold(f64::INFINITY, f64::min);
            if self
                .corpus
                .windows
                .iter()
                .all(|w| w.weight.is_finite() && w.weight >= 0.0)
            {
                self.trial_contexts = self
                    .corpus
                    .windows
                    .iter()
                    .enumerate()
                    .map(|(id, window)| TrialContext {
                        frequency: window.weight,
                        tail: self.tails[id],
                        // Literal multiplicity and action output dependencies
                        // remain fixed as bindings move between physical slots.
                        source_flags: (self.keys.terminal_dependency_start(
                            &self.program,
                            window.text(),
                            window.end(),
                        ) as u8)
                            << 1
                            | u8::from(self.direct_contexts[id]),
                        tail_id: TrialContext::packed_tail_id(
                            self.tails[id],
                            self.geometry.keys.len() + 1,
                        ),
                    })
                    .collect();
                // Check the budget using integers: f64 would round 2^53 + 1
                // back to 2^53 and could incorrectly certify exact updates.
                self.exact_integer_mass =
                    self.trial_contexts.iter().try_fold(0_u64, |sum, context| {
                        let f = context.frequency;
                        const LIMIT: u64 = 1 << 53;
                        if !f.is_finite() || f < 0.0 || f > LIMIT as f64 || f.fract() != 0.0 {
                            return None;
                        }
                        sum.checked_add(f as u64).filter(|&mass| mass <= LIMIT)
                    });
                self.prepared_contexts = self
                    .trial_contexts
                    .iter()
                    .zip(self.corpus.windows.iter())
                    .map(|(context, window)| {
                        let start = context.source_start();
                        crate::action_fast::PreparedContext::new(
                            &window.text()[start..],
                            window.end() - start,
                        )
                        .expect("validated cached source context")
                    })
                    .collect();
                let active = mana2_metrics::ActiveStats::from_weights(weights);
                self.contributions
                    .mana2
                    .as_mut()
                    .unwrap()
                    .prepare_active(active);
                self.active_mana2 = Some(active);
                self.rebuild_direct_groups();
            }
        }
    }

    /// Exact masses are layout-independent; physical tails are rebuilt after
    /// every committed permutation or checkpoint restore.
    fn rebuild_direct_groups(&mut self) {
        self.direct_groups.clear();
        if self.exact_integer_mass.is_none() {
            return;
        }
        let radix = self.geometry.keys.len() + 1;
        let mut indices = vec![u32::MAX; radix.pow(3)];
        for (id, context) in self.trial_contexts.iter().enumerate() {
            if !context.direct() {
                continue;
            }
            let index = &mut indices[context.tail_id as usize];
            if *index == u32::MAX {
                *index = self.direct_groups.len() as u32;
                self.direct_groups.push(DirectTailGroup {
                    tail: context.tail,
                    tail_id: context.tail_id,
                    frequency: 0.0,
                    contexts: Vec::new(),
                });
            }
            let group = &mut self.direct_groups[*index as usize];
            group.frequency += context.frequency;
            group.contexts.push(id as u32);
        }
    }

    pub(crate) fn physical_score(&self, weights: &Weights) -> f64 {
        let raw = self.raw.1.as_ref().expect("physical stats must be enabled");
        let populations = self.corpus.speed_populations;
        weights.physical_score(&mana_stats(raw, self.totals, populations))
    }

    #[cfg(test)]
    pub(crate) fn mana2_score(&self, weights: &mana2_metrics::Weights) -> f64 {
        let raw = self.raw.1.as_ref().expect("mana2 totals must be enabled");
        let populations = self.corpus.speed_populations;
        -mana2_metrics::score(&mana_stats(raw, self.totals, populations), weights)
    }

    #[cfg(test)]
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
        Self::new_with_fingers(
            corpus,
            layout,
            geometry,
            stop,
            crate::alt_fingers::AltPolicy::layout(true),
            effort,
        )
    }

    #[cfg(test)]
    pub(crate) fn new_with_fingers<F>(
        corpus: &NgramCorpus,
        layout: &ak::Layout,
        geometry: Geometry,
        stop: &AtomicBool,
        alt_fingers: crate::alt_fingers::AltPolicy,
        effort: F,
    ) -> ak::Result<Self>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        Self::new_with_policy(corpus, layout, geometry, stop, alt_fingers, true, effort)
    }

    pub(crate) fn new_with_policy<F>(
        corpus: &NgramCorpus,
        layout: &ak::Layout,
        geometry: Geometry,
        stop: &AtomicBool,
        alt_fingers: crate::alt_fingers::AltPolicy,
        combos: impl Into<crate::combos::Policy>,
        effort: F,
    ) -> ak::Result<Self>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        Self::new_with_shared_geometry(
            corpus,
            layout,
            Arc::new(geometry),
            stop,
            alt_fingers,
            combos,
            effort,
        )
    }

    /// Share immutable physical tables while rebuilding all rule and binding
    /// programs for this layout.
    pub(crate) fn new_with_shared_geometry<F>(
        corpus: &NgramCorpus,
        layout: &ak::Layout,
        geometry: Arc<Geometry>,
        stop: &AtomicBool,
        alt_fingers: crate::alt_fingers::AltPolicy,
        combos: impl Into<crate::combos::Policy>,
        effort: F,
    ) -> ak::Result<Self>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        let policy: crate::combos::Policy = combos.into();
        let combos = if policy.enabled && !layout.combos.is_empty() {
            Some(crate::combos::Program::new(layout)?)
        } else {
            None
        };
        let corpus = if combos.is_some() {
            corpus.chord_contexts()?
        } else {
            corpus.clone()
        };
        if layout.slots.len() >= GAP as usize || corpus.windows.len() > u32::MAX as usize {
            return Err("context cache exceeds index capacity".into());
        }
        let program = crate::action_fast::Program::new(layout, corpus.order)?;
        let keys = crate::action_fast::KeyState::new(&program);
        let contributions = TailContributions::new(&geometry);
        let compiled_alts = alt_fingers.compile(layout);
        let use_alt_fingers = compiled_alts.is_some();
        let mut corpus = corpus;
        if let Some(combos) = combos.as_ref().filter(|_| !use_alt_fingers) {
            corpus.windows = merge_unchorded(&corpus.windows, combos, &keys).into();
            corpus.rule_candidates = Arc::default();
        }
        let default_fingers: Vec<_> = layout.slots.iter().map(|slot| slot.finger).collect();
        let context_count = corpus.windows.len();
        let mut cache = Self {
            corpus,
            tails: Vec::with_capacity(context_count),
            finger_tails: use_alt_fingers.then(|| Vec::with_capacity(context_count)),
            combo_usage: Vec::with_capacity(context_count),
            postings: std::array::from_fn(|_| Vec::new()),
            open_postings: std::array::from_fn(|_| Vec::new()),
            direct_postings: std::array::from_fn(|_| Vec::new()),
            ambiguous_postings: std::array::from_fn(|_| Vec::new()),
            direct_contexts: Vec::with_capacity(context_count),
            static_contexts: Vec::with_capacity(context_count),
            binding_postings: Vec::new(),
            nonstatic: Vec::new(),
            chord_grams: Vec::with_capacity(context_count),
            tail_groups: Vec::new(),
            group_of: Vec::new(),
            group_postings: Vec::new(),
            grouped_postings: std::array::from_fn(|_| Vec::new()),
            combo_contexts: Vec::with_capacity(context_count),
            chord_direct: Vec::with_capacity(context_count),
            chord_plans: Vec::new(),
            chord_plan_of: Vec::new(),
            chord_postings: Vec::new(),
            trial_contexts: Vec::new(),
            prepared_contexts: Vec::new(),
            direct_groups: Vec::new(),
            exact_integer_mass: None,
            raw: Raw::default(),
            totals: [0.0; 4],
            geometry,
            positions: (0..layout.slots.len()).collect(),
            arrangement: (0..layout.slots.len()).collect(),
            commits: 0,
            program,
            keys,
            contributions,
            alt_fingers: compiled_alts.unwrap_or_default(),
            default_fingers,
            combos,
            finite_effort: false,
            aggregate: false,
            active_mana2: None,
            projection_mass: 0.0,
            projection_min_frequency: 0.0,
        };
        let mut mapper = crate::action_fast::Mapper::new(&cache.program, &cache.keys);
        // Contexts off the literal fast path whose every key is decided by the
        // previous character or key alone read it from static tables.
        let statics = crate::action_fast::StaticBindings::new(&cache.program);
        if statics.is_some() {
            cache.binding_postings = vec![Vec::new(); cache.positions.len()];
        }
        for (id, window) in cache.corpus.windows.iter().enumerate() {
            if stop.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            let combo_context = cache
                .combos
                .as_ref()
                .is_some_and(|combos| combos.touches(window.text(), window.left_open));
            let usage = [GAP; 3];
            // No press of this chord context weighed one key against another.
            let mut choice_free = false;
            let mapped = match &cache.combos {
                Some(combos) if combo_context => {
                    let text = window.text();
                    let plan = combos.plan_window(text, window.left_open);
                    cache.chord_plan_of.resize(id + 1, u32::MAX);
                    cache.chord_plan_of[id] = cache.chord_plans.len() as u32;
                    cache.chord_plans.push(plan);
                    cache
                        .chord_postings
                        .resize(cache.positions.len(), Vec::new());
                    let forced = plan.forced();
                    for (at, slot) in forced.iter().enumerate() {
                        if let Some(slot) = *slot {
                            if !forced[..at].contains(&Some(slot)) {
                                cache.chord_postings[slot].push(id as u32);
                            }
                        }
                    }
                    let mask = plan.mask();
                    // Forced roots bypass the mapper's caches: every press is
                    // counted.
                    let mut counts = crate::action_profile::MappingCounts::default();
                    let mapped = mapper.map_chords_profiled::<true, _>(
                        text,
                        plan.forced(),
                        &mask[..text.len()],
                        &effort,
                        &mut counts,
                    )?;
                    choice_free = counts.effort_comparisons == 0;
                    let fingers = cache.finger_tails.as_ref().map(|_| {
                        cache.alt_fingers.resolve(
                            text,
                            &mapped[..text.len()],
                            &cache.default_fingers,
                        )
                    });
                    let grams = chord_grams(&plan, &mapped, fingers.as_ref(), window.target());
                    add_chord_grams(
                        &cache.contributions,
                        &mut cache.raw,
                        &mut cache.totals,
                        &grams,
                        window.weight,
                        cache.finger_tails.is_some(),
                        &cache.positions,
                        &cache.geometry,
                    );
                    cache.chord_grams.push(Some(Box::new(grams)));
                    [None; 5]
                }
                _ => {
                    cache.chord_grams.push(None);
                    mapper.map(window.text(), &effort)?
                }
            };
            let tail = terminal(mapped, window.end());
            if let Some(finger_tails) = &mut cache.finger_tails {
                let fingers = finger_terminal_at(
                    &cache.alt_fingers,
                    window.text(),
                    &mapped[..window.len()],
                    &cache.default_fingers,
                    window.end(),
                );
                contribute_maybe_fingers(
                    &mut cache.raw,
                    &mut cache.totals,
                    tail,
                    Some(fingers),
                    window.weight,
                    &cache.positions,
                    &cache.geometry,
                );
                finger_tails.push(fingers);
            } else {
                contribute(
                    &mut cache.raw,
                    &mut cache.totals,
                    tail,
                    window.weight,
                    &cache.positions,
                    &cache.geometry,
                );
            }
            contribute_combo_usage(
                &mut cache.raw,
                &mut cache.totals,
                usage,
                window.weight,
                &cache.geometry,
            );
            cache.tails.push(tail);
            cache.combo_usage.push(usage);
            let direct = !use_alt_fingers
                && !combo_context
                && cache
                    .keys
                    .direct_terminal(window.text(), window.end())
                    .is_some();
            cache.direct_contexts.push(direct);
            cache.combo_contexts.push(combo_context);
            cache
                .chord_direct
                .push(!use_alt_fingers && combo_context && choice_free);
            let bindings = statics
                .as_ref()
                .filter(|_| !direct && !combo_context)
                .and_then(|statics| statics.bindings(window.text()));
            cache.static_contexts.push(bindings);
            if statics.is_some() && bindings.is_none() {
                cache.nonstatic.push(id as u32);
            }
            if combo_context && window.left_open {
                if let Some(&first) = window.text().first() {
                    cache.open_postings[first as usize].push(id as u32);
                }
            }
            for (j, &byte) in window.text().iter().enumerate() {
                if !window.text()[..j].contains(&byte) {
                    cache.postings[byte as usize].push(id as u32);
                }
            }
            // What each context's mapping reads: its tail, or its text up to
            // where the mapper can look ahead.
            let dependency = if direct {
                &window.text()[window.end().saturating_sub(3)..window.end()]
            } else if combo_context {
                // Its plan is cached and its presses run through the counted
                // character; the one after matters only to a mapper looking
                // ahead from an ambiguous counted character.
                if cache.keys.unique(window.text()[window.target()]) {
                    &window.text()[..window.end()]
                } else {
                    window.text()
                }
            } else {
                let start = cache.keys.terminal_dependency_start(
                    &cache.program,
                    window.text(),
                    window.end(),
                );
                // A chord context's following character starts no combo here;
                // only the mapper's look-ahead from an ambiguous target reads it.
                let stop = if cache.keys.unique(window.text()[window.target()]) {
                    window.end()
                } else {
                    window.len()
                };
                &window.text()[start..stop]
            };
            if let Some(bindings) = bindings {
                let tail = &bindings[window.end().saturating_sub(3)..window.end()];
                for (j, &binding) in tail.iter().enumerate() {
                    if binding != crate::action_fast::UNTYPED && !tail[..j].contains(&binding) {
                        cache.binding_postings[usize::from(binding)].push(id as u32);
                    }
                }
                continue;
            }
            for (j, &byte) in dependency.iter().enumerate() {
                if !dependency[..j].contains(&byte) {
                    cache.direct_postings[byte as usize].push(id as u32);
                    if !direct {
                        cache.ambiguous_postings[byte as usize].push(id as u32);
                    }
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
    // Fill the reusable sorted union from the literal or statically bounded
    // action outputs moved between these slots. History-dependent actions
    // return None and retain the conservative full-context scan.
    fn affected(
        &self,
        slots: &[usize],
        extra: [u64; 4],
        grouped: bool,
        ids: &mut Vec<u32>,
        marks: &mut Vec<u64>,
    ) -> bool {
        let direct =
            self.finite_effort && (self.finger_tails.is_none() || !self.alt_fingers.reaches(slots));
        if self.affected_by_bytes(slots, extra, direct, grouped, ids, marks) {
            return true;
        }
        // Static contexts are in the byte postings only as a whole; with
        // dependency postings, select them by the bindings that move.
        if direct && !grouped && !self.binding_postings.is_empty() {
            // Mark every listed context once and read them back in order.
            mark_contexts(marks, self.tails.len(), ids);
            for &slot in slots {
                mark_contexts(
                    marks,
                    self.tails.len(),
                    &self.binding_postings[self.keys.binding(slot)],
                );
            }
            read_marks(marks, ids);
        }
        false
    }

    fn affected_by_bytes(
        &self,
        slots: &[usize],
        extra: [u64; 4],
        direct: bool,
        grouped: bool,
        ids: &mut Vec<u32>,
        marks: &mut Vec<u64>,
    ) -> bool {
        ids.clear();
        let Some(mut bytes) = self.program.affected_bytes(&self.keys, slots) else {
            // A key typing what history decides (a repeat) leaves static
            // contexts to their bindings, which `affected` adds, or to their
            // tail groups; only the others need a look.
            if direct && !self.binding_postings.is_empty() {
                ids.extend_from_slice(&self.nonstatic);
                return false;
            }
            return true;
        };
        for (target, extra) in bytes.iter_mut().zip(extra) {
            *target |= extra;
        }
        // A context's mapping depends only on its dependency bytes. Alternate
        // fingers can also read earlier keys through position rules, so a move
        // touching a rule's position falls back to every byte.
        let postings = if direct && grouped {
            &self.grouped_postings
        } else if direct {
            &self.direct_postings
        } else {
            &self.postings
        };

        // Posting lists are sorted: one list is copied, two use a linear
        // two-pointer merge, and more are united through the context bitmap.
        let mut first: [&[u32]; 2] = [&[], &[]];
        let mut selected_len = 0;
        let mut reserve = 0usize;
        for (word, &bits) in bytes.iter().enumerate() {
            let mut bits = bits;
            while bits != 0 {
                let byte = word * 64 + bits.trailing_zeros() as usize;
                bits &= bits - 1;
                if selected_len < 2 {
                    first[selected_len] = &postings[byte];
                }
                selected_len += 1;
                reserve = reserve.saturating_add(postings[byte].len());
            }
        }

        if selected_len <= 1 {
            ids.extend_from_slice(first[0]);
            return false;
        }
        if selected_len == 2 {
            ids.reserve(reserve.min(self.tails.len()));
            let [left, right] = first;
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
            return false;
        }

        for (word, &bits) in bytes.iter().enumerate() {
            let mut bits = bits;
            while bits != 0 {
                let byte = word * 64 + bits.trailing_zeros() as usize;
                bits &= bits - 1;
                mark_contexts(marks, self.tails.len(), &postings[byte]);
            }
        }
        read_marks(marks, ids);
        false
    }

    // For a three-key cycle, merge the three original binding dependencies.
    // The established pair path above is retained without extra work.
    /// `grouped` leaves static contexts to their tail groups.
    fn affected_move(
        &self,
        movement: Move,
        grouped: bool,
        ids: &mut Vec<u32>,
        marks: &mut Vec<u64>,
    ) -> bool {
        let slots = &movement.slots[..movement.len];
        // Chords are recognized from text alone (their plans are cached), but
        // a chord's presses follow its participant keys: moving one changes
        // the contexts whose plans press it, and left-open contexts a combo
        // may have begun before (`combos::Program::affected_by`).
        let mut moved = [0; 3];
        for (original, &slot) in moved.iter_mut().zip(slots) {
            *original = self.arrangement[slot];
        }
        let moved = &moved[..slots.len()];
        let open = self
            .combos
            .as_ref()
            .map_or([0; 4], |combos| combos.affected_by(moved).1);
        if self.affected(slots, [0; 4], grouped, ids, marks) {
            return true;
        }
        let chords = moved
            .iter()
            .filter_map(|&slot| self.chord_postings.get(slot))
            .filter(|posting| !posting.is_empty());
        let opens = (0..256)
            .filter(|&byte| open[byte / 64] & 1 << (byte % 64) != 0)
            .map(|byte| &self.open_postings[byte])
            .filter(|posting| !posting.is_empty());
        let mut extra = chords.chain(opens).peekable();
        if extra.peek().is_some() {
            // United through the context bitmap, which reads back in order.
            mark_contexts(marks, self.tails.len(), ids);
            for posting in extra {
                mark_contexts(marks, self.tails.len(), posting);
            }
            read_marks(marks, ids);
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
        self.direct_groups.clear();
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
        let combo_positions = positions(&self.arrangement);
        let mut raw = if self.raw.1.is_some() {
            Raw::with_mana2()
        } else {
            Raw::default()
        };
        let mut totals = [0.0; 4];
        for (id, window) in self.corpus.windows.iter().enumerate() {
            if stop.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            let usage = [GAP; 3];
            let mapped = match &self.combos {
                Some(combos) if self.combo_contexts[id] => {
                    let text = window.text();
                    let plan =
                        self.chord_plans[self.chord_plan_of[id] as usize].at(&combo_positions);
                    let mask = plan.mask();
                    let mapped =
                        mapper.map_chords(text, plan.forced(), &mask[..text.len()], &effort)?;
                    let fingers = self.finger_tails.as_ref().map(|_| {
                        self.alt_fingers
                            .resolve(text, &mapped[..text.len()], &self.default_fingers)
                    });
                    let grams = chord_grams(&plan, &mapped, fingers.as_ref(), window.target());
                    if saved.is_none() {
                        add_chord_grams(
                            &self.contributions,
                            &mut raw,
                            &mut totals,
                            &grams,
                            window.weight,
                            self.finger_tails.is_some(),
                            &self.positions,
                            &self.geometry,
                        );
                    }
                    self.chord_grams[id] = Some(Box::new(grams));
                    [None; 5]
                }
                _ => mapper.map(window.text(), &effort)?,
            };
            let tail = terminal(mapped, window.end());
            self.tails[id] = tail;
            if let Some(context) = self.trial_contexts.get_mut(id) {
                context.set_tail(tail, self.geometry.keys.len() + 1);
            }
            self.combo_usage[id] = usage;
            let fingers = self.finger_tails.as_ref().map(|_| {
                finger_terminal_at(
                    &self.alt_fingers,
                    window.text(),
                    &mapped[..window.len()],
                    &self.default_fingers,
                    window.end(),
                )
            });
            if let (Some(tails), Some(fingers)) = (&mut self.finger_tails, fingers) {
                tails[id] = fingers;
            }
            if saved.is_none() {
                self.contributions.apply_contextual(
                    &mut raw,
                    &mut totals,
                    tail,
                    fingers,
                    window.weight,
                    &self.positions,
                    &self.geometry,
                );
                self.contributions.apply_combo_usage(
                    &mut raw,
                    &mut totals,
                    usage,
                    window.weight,
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
        self.rebuild_direct_groups();
        if !self.tail_groups.is_empty() {
            self.refresh_tail_groups();
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
        &self,
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
        &self,
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

    /// Dedicated score screening retains no patches in the finite projected
    /// path. The winner must be replayed through the normal proposal API.
    #[cfg(test)]
    pub(crate) fn propose_swap_score_profiled<const PROFILE: bool, const DETAIL: bool, F>(
        &self,
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
        self.propose_move_inner::<PROFILE, DETAIL, false, F>(
            Move::pair(a, b),
            stop,
            effort,
            proposal,
            profile,
        )
    }

    pub(crate) fn propose_move_profiled<const PROFILE: bool, const DETAIL: bool, F>(
        &self,
        movement: Move,
        stop: &AtomicBool,
        effort: F,
        proposal: &mut Proposal,
        profile: &mut Profile,
    ) -> ak::Result<()>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        self.propose_move_inner::<PROFILE, DETAIL, true, F>(
            movement, stop, effort, proposal, profile,
        )
    }

    fn propose_move_inner<const PROFILE: bool, const DETAIL: bool, const RECORD: bool, F>(
        &self,
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
        proposal.omitted_changes = false;
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
        proposal.finger_changes.clear();
        proposal.combo_usage_changes.clear();
        proposal.chord_changes.clear();
        proposal.chord_relabels.clear();
        proposal.raw.clone_from(&self.raw);
        proposal.totals = self.totals;
        proposal.speed_populations = self.corpus.speed_populations;
        let active = self.active_mana2.filter(|_| {
            self.contributions
                .mana2
                .as_ref()
                .unwrap()
                .projection_is_finite(
                    self.raw.1.as_ref().unwrap(),
                    self.totals,
                    self.projection_mass,
                    self.projection_min_frequency,
                )
                && self
                    .totals
                    .iter()
                    .all(|&t| t <= 0.0 || t >= self.projection_min_frequency / 2.0)
        });
        proposal.projected = active.is_some();
        proposal.movement = movement;
        let terminal_only = self.combos.is_none() && self.finger_tails.is_none();
        // Contexts no combo touches map as in a layout without combos.
        let plain_terminal = self.finger_tails.is_none();
        let terminal_active = active.filter(|_| self.finite_effort && terminal_only);
        // Screening sums each distinct tail's net frequency; `materialize`
        // replays an accepted proposal context by context.
        let aggregate = self.aggregate && active.is_none();
        proposal.aggregated = false;
        // Position rules match the same windows after a move no rule's
        // position takes part in: a tail key that moved is matched by none
        // and takes its new slot's finger, the others keep theirs. Static
        // contexts then move by tail group.
        let rules_unmoved = self.alt_fingers.positional()
            && !self.alt_fingers.reaches(&movement.slots[..movement.len]);
        let grouped = aggregate
            && self.finite_effort
            && !self.tail_groups.is_empty()
            && (self.finger_tails.is_none() || rules_unmoved);
        proposal.group_changes.clear();
        let sparse = terminal_active
            .is_some_and(|active| active.allows_sparse_zeros(self.raw.1.as_ref().unwrap()));
        let exact_integer = sparse
            && terminal_active.is_some_and(|active| {
                self.exact_integer_mass.is_some_and(|mass| {
                    self.totals.iter().all(|&total| {
                        total.is_finite()
                            && total.fract() == 0.0
                            && total.to_bits() != (-0.0_f64).to_bits()
                            && total.abs() <= (1_u64 << 53) as f64
                            && total.abs() as u128 + 2 * mass as u128 <= 1_u128 << 53
                    }) && self
                        .contributions
                        .mana2
                        .as_ref()
                        .unwrap()
                        .active_integer_is_exact(self.raw.1.as_ref().unwrap(), mass, active)
                })
            });
        let inert_base = self.geometry.keys.len() + 1;
        if !RECORD && exact_integer {
            proposal.integer_plan_ids.resize(inert_base.pow(3), 0);
            proposal.integer_plan_ids.fill(0);
            proposal.integer_plans.clear();
            proposal.boolean_frequencies.clear();
        }
        // Each slot's position after the move, for direct tails.
        let permutation: [u8; 256] =
            std::array::from_fn(|slot| move_tail([slot as u8, GAP, GAP], movement)[0]);
        let affected_start = clock::<PROFILE>();
        // Boolean fields are certified exact integers. Aggregate their direct
        // tails once; retain ascending context order for continuous fields.
        let grouped_direct = !RECORD && exact_integer && !self.direct_groups.is_empty();
        if grouped_direct {
            proposal.direct_plan_ids.resize(inert_base.pow(3), 0);
            proposal.direct_plan_ids.fill(0);
            proposal.direct_plans.clear();
            proposal.direct_plan_ranges.clear();
            proposal.direct_numeric_updates.clear();
            proposal
                .numeric_context_bits
                .resize((self.tails.len() + 63) / 64, 0);
            proposal.numeric_context_bits.fill(0);
            let active = terminal_active.unwrap();
            for group in &self.direct_groups {
                if stop.load(Ordering::Relaxed) {
                    return Err("cancelled".into());
                }
                let tail = group.tail.map(|slot| permutation[slot as usize]);
                if tail == group.tail {
                    continue;
                }
                let operation = self
                    .contributions
                    .mana2
                    .as_ref()
                    .unwrap()
                    .compile_active_integer_replacement(group.tail, tail, active)
                    .expect("prepared active contribution cache");
                operation.apply_boolean(proposal.raw.1.as_mut().unwrap(), group.frequency);
                if operation.has_nonboolean() {
                    let start = proposal.direct_numeric_updates.len() as u32;
                    self.contributions
                        .mana2
                        .as_ref()
                        .unwrap()
                        .compile_integer_numeric_updates(
                            &operation,
                            &mut proposal.direct_numeric_updates,
                        );
                    proposal
                        .direct_plan_ranges
                        .push((start, proposal.direct_numeric_updates.len() as u32));
                    proposal.direct_plans.push(operation);
                    proposal.direct_plan_ids[group.tail_id as usize] =
                        proposal.direct_plans.len() as u32;
                    for &id in &group.contexts {
                        proposal.numeric_context_bits[id as usize / 64] |= 1_u64 << (id % 64);
                    }
                } else if PROFILE {
                    profile.changed += group.contexts.len() as u64;
                    if operation.is_empty() {
                        profile.counter_skipped += group.contexts.len() as u64;
                    }
                }
            }
            if let Some(bytes) = self
                .program
                .affected_bytes(&self.keys, &movement.slots[..movement.len as usize])
            {
                for (word, &bits) in bytes.iter().enumerate() {
                    let mut bits = bits;
                    while bits != 0 {
                        let bit = bits.trailing_zeros() as usize;
                        bits &= bits - 1;
                        for &id in &self.ambiguous_postings[word * 64 + bit] {
                            proposal.numeric_context_bits[id as usize / 64] |= 1_u64 << (id % 64);
                        }
                    }
                }
            } else {
                for (id, context) in self.trial_contexts.iter().enumerate() {
                    if !context.direct() {
                        proposal.numeric_context_bits[id / 64] |= 1_u64 << (id % 64);
                    }
                }
                if PROFILE {
                    profile.full_scans += 1;
                }
            }
            proposal.affected.clear();
            for (word, &bits) in proposal.numeric_context_bits.iter().enumerate() {
                let mut bits = bits;
                while bits != 0 {
                    let bit = bits.trailing_zeros() as usize;
                    bits &= bits - 1;
                    proposal.affected.push((word * 64 + bit) as u32);
                }
            }
            proposal.all_contexts = false;
        } else {
            proposal.all_contexts = self.affected_move(
                movement,
                grouped,
                &mut proposal.affected,
                &mut proposal.context_marks,
            );
        }
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
        if RECORD || active.is_none() || !self.finite_effort {
            proposal.changes.reserve(proposal.examined);
        }
        // Each trial owns its small key masks and permutation. Corpus, tails,
        // geometry and contribution tables are shared by independent trials.
        let mut keys = self.keys.clone();
        keys.swap(a, b, &self.program);
        if movement.len == 3 {
            keys.swap(b, c, &self.program);
        }
        let combo_positions = self.combos.as_ref().map(|_| {
            let mut arrangement = self.arrangement.clone();
            arrangement.swap(a, b);
            if movement.len == 3 {
                arrangement.swap(b, c);
            }
            positions(&arrangement)
        });
        let contexts_start = clock::<PROFILE>();
        let result = (|| -> ak::Result<()> {
            let mut mapper = crate::action_fast::Mapper::new(&self.program, &keys);
            mapper.reuse_memos(&mut proposal.memos);
            // Finite effort makes each transition's winner a function of its
            // history, the same throughout this trial; chord contexts, mapped
            // with forced roots, bypass the memo.
            if self.finite_effort && terminal_active.is_none() {
                mapper.enable_transition_memo();
            }
            let binding_slots = keys.slots(self.positions.len());
            // This opt-in path has no contextual fingers or chords. Its compact
            // sidecar holds the current tail, frequency and tail cache ID.
            if let Some(active) = terminal_active {
                // Effort, geometry and bindings stay fixed throughout this trial.
                mapper.enable_transition_memo();
                mapper.enable_prepared_terminal_memo();
                proposal.omitted_changes = !RECORD;
                for index in 0..proposal.examined {
                    if stop.load(Ordering::Relaxed) {
                        return Err("cancelled".into());
                    }
                    let id = if proposal.all_contexts {
                        index
                    } else {
                        proposal.affected[index] as usize
                    };
                    let context = self.trial_contexts[id];
                    if grouped_direct && context.direct() {
                        // Group selection already certified a changed tail and
                        // applied its Boolean mass. These immutable plans are
                        // independent of the ambiguous tail replacement cache.
                        let plan_id = proposal.direct_plan_ids[context.tail_id as usize];
                        let (start, end) = proposal.direct_plan_ranges[plan_id as usize - 1];
                        let contribution_start = clock::<DETAIL>();
                        mana2_metrics::NumericUpdate::apply_all(
                            proposal.raw.1.as_mut().unwrap(),
                            &proposal.direct_numeric_updates[start as usize..end as usize],
                            context.frequency,
                        );
                        if DETAIL {
                            profile.contributions += elapsed::<DETAIL>(contribution_start);
                        }
                        if PROFILE {
                            profile.ops.literal_contexts += 1;
                            profile.mapped += 1;
                            profile.changed += 1;
                        }
                        continue;
                    }
                    let old = context.tail;
                    let mapping_start = clock::<DETAIL>();
                    let tail = if context.direct() {
                        if PROFILE {
                            profile.ops.literal_contexts += 1;
                        }
                        let cached = if !RECORD && exact_integer {
                            proposal.integer_plan_ids[context.tail_id as usize]
                        } else {
                            0
                        };
                        if cached & (1_u64 << 63) != 0 {
                            let signature = (cached >> 32) as u32;
                            [
                                signature as u8,
                                (signature >> 8) as u8,
                                (signature >> 16) as u8,
                            ]
                        } else {
                            old.map(|slot| permutation[slot as usize])
                        }
                    } else {
                        mapper.map_prepared_context_finite_profiled::<PROFILE, F>(
                            self.prepared_contexts[id],
                            &effort,
                            &mut profile.ops,
                        )?
                    };
                    if DETAIL {
                        profile.mapping += elapsed::<DETAIL>(mapping_start);
                    }
                    if PROFILE {
                        profile.mapped += 1;
                    }
                    if tail != old {
                        if !RECORD && exact_integer {
                            let cache_id = context.tail_id as usize;
                            let signature = u32::from(tail[0])
                                | (u32::from(tail[1]) << 8)
                                | (u32::from(tail[2]) << 16);
                            let entry = &mut proposal.integer_plan_ids[cache_id];
                            let mut plan_id = *entry as u32;
                            if plan_id == 0 || ((*entry >> 32) as u32 & 0x00ff_ffff) != signature {
                                let operation = self
                                    .contributions
                                    .mana2
                                    .as_ref()
                                    .unwrap()
                                    .compile_active_integer_replacement(old, tail, active)
                                    .expect("prepared active contribution cache");
                                if plan_id == 0 {
                                    proposal.integer_plans.push(operation);
                                    proposal.boolean_frequencies.push(0.0);
                                    plan_id = proposal.integer_plans.len() as u32;
                                } else {
                                    // A different ambiguous mapping may reuse
                                    // this tail's slot. Flush its certified
                                    // integer contribution before replacing it.
                                    let index = plan_id as usize - 1;
                                    proposal.integer_plans[index].apply_boolean(
                                        proposal.raw.1.as_mut().unwrap(),
                                        proposal.boolean_frequencies[index],
                                    );
                                    proposal.boolean_frequencies[index] = 0.0;
                                    proposal.integer_plans[plan_id as usize - 1] = operation;
                                }
                                *entry = (u64::from(signature) << 32) | u64::from(plan_id);
                            }
                            if context.direct() {
                                // This permutation is fixed for the trial; only
                                // a differing ambiguous mapping clears the flag.
                                *entry |= 1_u64 << 63;
                            }
                            let operation = &proposal.integer_plans[plan_id as usize - 1];
                            if !context.direct() {
                                TailContributions::replace_integer_totals(
                                    &mut proposal.totals,
                                    old,
                                    tail,
                                    context.frequency,
                                );
                            }
                            let contribution_start = clock::<DETAIL>();
                            if !operation.is_empty() {
                                if operation.has_boolean() {
                                    // Every frequency is nonnegative integral,
                                    // and each partial sum is <= checked mass.
                                    proposal.boolean_frequencies[plan_id as usize - 1] +=
                                        context.frequency;
                                }
                                if operation.has_nonboolean() {
                                    self.contributions
                                        .mana2
                                        .as_ref()
                                        .unwrap()
                                        .apply_active_integer_replacement_nonboolean(
                                            proposal.raw.1.as_mut().unwrap(),
                                            operation,
                                            context.frequency,
                                        );
                                }
                            } else if PROFILE {
                                profile.counter_skipped += 1;
                            }
                            if DETAIL {
                                profile.contributions += elapsed::<DETAIL>(contribution_start);
                            }
                            if PROFILE {
                                profile.changed += 1;
                            }
                            continue;
                        }
                        let contribution_start = clock::<DETAIL>();
                        if exact_integer {
                            self.contributions.replace_active_integer(
                                &mut proposal.raw,
                                &mut proposal.totals,
                                old,
                                tail,
                                context.frequency,
                                active,
                            );
                        } else if sparse {
                            self.contributions.replace_active_sparse(
                                &mut proposal.raw,
                                &mut proposal.totals,
                                old,
                                tail,
                                context.frequency,
                                active,
                            );
                        } else {
                            self.contributions.replace_active(
                                &mut proposal.raw,
                                &mut proposal.totals,
                                old,
                                tail,
                                context.frequency,
                                active,
                            );
                        }
                        if RECORD {
                            proposal.changes.push((id, tail));
                        }
                        if DETAIL {
                            profile.contributions += elapsed::<DETAIL>(contribution_start);
                        }
                        if PROFILE {
                            profile.changed += 1;
                        }
                    }
                }
                if !RECORD && exact_integer {
                    let contribution_start = clock::<DETAIL>();
                    for (operation, frequency) in proposal
                        .integer_plans
                        .iter()
                        .zip(proposal.boolean_frequencies.iter_mut())
                    {
                        operation.apply_boolean(proposal.raw.1.as_mut().unwrap(), *frequency);
                        *frequency = 0.0;
                    }
                    if DETAIL {
                        profile.contributions += elapsed::<DETAIL>(contribution_start);
                    }
                }
                mapper.return_memos(&mut proposal.memos);
                return Ok(());
            }
            for index in 0..proposal.examined {
                if stop.load(Ordering::Relaxed) {
                    return Err("cancelled".into());
                }
                let id = if proposal.all_contexts {
                    index
                } else {
                    proposal.affected[index] as usize
                };
                if grouped && self.direct_contexts[id] {
                    // Its tail group moves it.
                    continue;
                }
                let window = &self.corpus.windows[id];
                let mapping_start = clock::<DETAIL>();
                if let (Some(_), Some(positions)) = (&self.combos, &combo_positions) {
                    if self.combo_contexts[id] {
                        let remap = |mapper: &mut crate::action_fast::Mapper<'_>,
                                     ops: &mut crate::action_profile::MappingCounts|
                         -> ak::Result<ChordGrams> {
                            let text = window.text();
                            let plan =
                                self.chord_plans[self.chord_plan_of[id] as usize].at(positions);
                            let mask = plan.mask();
                            let mapped = mapper.map_chords_profiled::<PROFILE, F>(
                                text,
                                plan.forced(),
                                &mask[..text.len()],
                                &effort,
                                ops,
                            )?;
                            let fingers = self.finger_tails.as_ref().map(|_| {
                                self.alt_fingers.resolve(
                                    text,
                                    &mapped[..text.len()],
                                    &self.default_fingers,
                                )
                            });
                            Ok(chord_grams(
                                &plan,
                                &mapped,
                                fingers.as_ref(),
                                window.target(),
                            ))
                        };
                        if aggregate && self.finite_effort && self.chord_direct[id] {
                            // Screening moves just the tails holding a moved
                            // key; a replay relabels the whole grams.
                            let old = self.chord_grams[id].as_deref().unwrap();
                            #[cfg(test)]
                            assert_eq!(old.moved(movement), remap(&mut mapper, &mut profile.ops)?);
                            let contribution_start = clock::<DETAIL>();
                            let mut changed = old.pressed().any(|key| {
                                move_tail([key as u8, GAP, GAP], movement)[0] != key as u8
                            });
                            for (tail, _, multiple) in old.iter() {
                                let moved = move_tail(tail, movement);
                                if moved != tail {
                                    changed = true;
                                    let weight = multiple * window.weight;
                                    proposal.net.add(self.tail_key(tail, None), -weight);
                                    proposal.net.add(self.tail_key(moved, None), weight);
                                }
                            }
                            if changed {
                                proposal.chord_relabels.push(id as u32);
                            }
                            if DETAIL {
                                profile.contributions += elapsed::<DETAIL>(contribution_start);
                            }
                            if PROFILE {
                                profile.mapped += 1;
                                profile.changed += u64::from(changed);
                            }
                            continue;
                        }
                        let grams = if self.finite_effort && self.chord_direct[id] {
                            let grams = self.chord_grams[id].as_deref().unwrap().moved(movement);
                            #[cfg(test)]
                            assert_eq!(grams, remap(&mut mapper, &mut profile.ops)?);
                            grams
                        } else {
                            remap(&mut mapper, &mut profile.ops)?
                        };
                        if DETAIL {
                            profile.mapping += elapsed::<DETAIL>(mapping_start);
                        }
                        if PROFILE {
                            profile.mapped += 1;
                        }
                        let old = self.chord_grams[id].as_deref().unwrap();
                        if grams == *old {
                            continue;
                        }
                        let contribution_start = clock::<DETAIL>();
                        if aggregate {
                            let tracked = self.finger_tails.is_some();
                            for (grams, sign) in [(old, -1.0), (&grams, 1.0)] {
                                for (tail, fingers, multiple) in grams.iter() {
                                    proposal.net.add(
                                        self.tail_key(tail, tracked.then_some(fingers)),
                                        sign * multiple * window.weight,
                                    );
                                }
                            }
                        } else {
                            self.apply_change(
                                &mut proposal.raw,
                                &mut proposal.totals,
                                id,
                                [GAP; 3],
                                None,
                                None,
                                active,
                                Some(&grams),
                            );
                        }
                        if active.is_none() && self.finger_tails.is_some() {
                            proposal.finger_changes.push((id, [GAP; 3]));
                        }
                        proposal.changes.push((id, [GAP; 3]));
                        proposal.chord_changes.push((id, grams));
                        if DETAIL {
                            profile.contributions += elapsed::<DETAIL>(contribution_start);
                        }
                        if PROFILE {
                            profile.changed += 1;
                        }
                        continue;
                    }
                }
                let (tail, fingers, usage) = if self.finite_effort && self.direct_contexts[id] {
                    if PROFILE {
                        profile.ops.literal_contexts += 1;
                    }
                    (
                        self.tails[id].map(|slot| permutation[slot as usize]),
                        None,
                        [GAP; 3],
                    )
                } else if let Some(bindings) =
                    self.static_contexts[id].filter(|_| self.finite_effort)
                {
                    let mut mapped = [None; 5];
                    for (key, &binding) in mapped.iter_mut().zip(&bindings[..window.len()]) {
                        if binding != crate::action_fast::UNTYPED {
                            *key = Some(usize::from(binding_slots[usize::from(binding)]));
                        }
                    }
                    let tail = terminal(mapped, window.end());
                    let fingers = self.finger_tails.as_ref().map(|tails| {
                        if rules_unmoved {
                            let fingers = moved_fingers(
                                tails[id],
                                self.tails[id],
                                tail,
                                &self.default_fingers,
                            );
                            #[cfg(test)]
                            assert_eq!(
                                fingers,
                                finger_terminal_at(
                                    &self.alt_fingers,
                                    window.text(),
                                    &mapped[..window.len()],
                                    &self.default_fingers,
                                    window.end(),
                                )
                            );
                            fingers
                        } else {
                            finger_terminal_at(
                                &self.alt_fingers,
                                window.text(),
                                &mapped[..window.len()],
                                &self.default_fingers,
                                window.end(),
                            )
                        }
                    });
                    (tail, fingers, [GAP; 3])
                } else {
                    match (&self.combos, &combo_positions) {
                        _ if plain_terminal && self.finite_effort => (
                            mapper.map_terminal_finite_profiled::<PROFILE, F>(
                                window.text(),
                                window.end(),
                                &effort,
                                &mut profile.ops,
                            )?,
                            None,
                            [GAP; 3],
                        ),
                        _ if plain_terminal => (
                            mapper.map_terminal_profiled::<PROFILE, F>(
                                window.text(),
                                window.end(),
                                &effort,
                                &mut profile.ops,
                            )?,
                            None,
                            [GAP; 3],
                        ),
                        _ => {
                            let mapped = if self.finite_effort {
                                mapper.map_finite_profiled::<PROFILE, F>(
                                    window.text(),
                                    &effort,
                                    &mut profile.ops,
                                )?
                            } else {
                                mapper.map_profiled::<PROFILE, F>(
                                    window.text(),
                                    &effort,
                                    &mut profile.ops,
                                )?
                            };
                            let fingers = Some(finger_terminal_at(
                                &self.alt_fingers,
                                window.text(),
                                &mapped[..window.len()],
                                &self.default_fingers,
                                window.end(),
                            ));
                            (terminal(mapped, window.end()), fingers, [GAP; 3])
                        }
                    }
                };
                if DETAIL {
                    profile.mapping += elapsed::<DETAIL>(mapping_start);
                }
                if PROFILE {
                    profile.mapped += 1;
                }
                let fingers_changed =
                    fingers.is_some_and(|value| self.finger_tails.as_ref().unwrap()[id] != value);
                let usage_changed = usage != self.combo_usage[id];
                if tail != self.tails[id] || fingers_changed || usage_changed {
                    let contribution_start = clock::<DETAIL>();
                    if aggregate {
                        // Linear in the frequency: net each tail's frequency
                        // and apply every distinct tail once below.
                        let weight = window.weight;
                        let old_fingers = fingers.map(|_| self.finger_tails.as_ref().unwrap()[id]);
                        proposal
                            .net
                            .add(self.tail_key(self.tails[id], old_fingers), -weight);
                        proposal.net.add(self.tail_key(tail, fingers), weight);
                        if usage_changed {
                            proposal.net.add(usage_key(self.combo_usage[id]), -weight);
                            proposal.net.add(usage_key(usage), weight);
                        }
                    } else {
                        self.apply_change(
                            &mut proposal.raw,
                            &mut proposal.totals,
                            id,
                            tail,
                            fingers,
                            usage_changed.then_some(usage),
                            active,
                            None,
                        );
                    }
                    if let Some(fingers) = fingers.filter(|_| active.is_none()) {
                        proposal.finger_changes.push((id, fingers));
                    }
                    if usage_changed {
                        proposal.combo_usage_changes.push((id, usage));
                    }
                    proposal.changes.push((id, tail));
                    if DETAIL {
                        profile.contributions += elapsed::<DETAIL>(contribution_start);
                    }
                    if PROFILE {
                        profile.changed += 1;
                    }
                }
            }
            if grouped {
                let contribution_start = clock::<DETAIL>();
                // Every member of a group shares its tail; members with their
                // keys' own fingers move as one frequency.
                let mut marks = std::mem::take(&mut proposal.group_marks);
                marks.clear();
                marks.resize((self.tail_groups.len() + 63) / 64, 0);
                for &slot in &movement.slots[..movement.len] {
                    for &group in &self.group_postings[self.keys.binding(slot)] {
                        marks[group as usize / 64] |= 1 << (group % 64);
                    }
                }
                for (word, &bits) in marks.iter().enumerate() {
                    let mut bits = bits;
                    while bits != 0 {
                        let index = word * 64 + bits.trailing_zeros() as usize;
                        bits &= bits - 1;
                        let group = &self.tail_groups[index];
                        let mut tail = [GAP; 3];
                        for (slot, &binding) in tail.iter_mut().zip(&group.bindings) {
                            if binding != crate::action_fast::UNTYPED {
                                *slot = binding_slots[usize::from(binding)];
                            }
                        }
                        if tail == group.tail {
                            continue;
                        }
                        if group.default_weight != 0.0 {
                            proposal
                                .net
                                .add(self.tail_key(group.tail, None), -group.default_weight);
                            proposal
                                .net
                                .add(self.tail_key(tail, None), group.default_weight);
                        }
                        let tails = self.finger_tails.as_ref();
                        for &member in &group.contextual {
                            let id = member as usize;
                            let weight = self.corpus.windows[id].weight;
                            let old = tails.unwrap()[id];
                            let fingers =
                                moved_fingers(old, group.tail, tail, &self.default_fingers);
                            proposal
                                .net
                                .add(self.tail_key(group.tail, Some(old)), -weight);
                            proposal.net.add(self.tail_key(tail, Some(fingers)), weight);
                        }
                        proposal.group_changes.push((index as u32, tail));
                        if PROFILE {
                            profile.changed += group.members.len() as u64;
                        }
                    }
                }
                proposal.group_marks = marks;
                if DETAIL {
                    profile.contributions += elapsed::<DETAIL>(contribution_start);
                }
            }
            if aggregate {
                let contribution_start = clock::<DETAIL>();
                for (key, weight) in proposal.net.drain() {
                    if weight == 0.0 {
                        continue;
                    }
                    let (tail, fingers, usage) = unpack_key(key);
                    if usage {
                        self.contributions.apply_combo_usage(
                            &mut proposal.raw,
                            &mut proposal.totals,
                            tail,
                            weight,
                            &self.geometry,
                        );
                    } else if fingers.is_some() {
                        // Contextual fingers take the general per-gram path;
                        // screening scales its frequency-one result instead.
                        if !Arc::ptr_eq(&proposal.contextual_geometry, &self.geometry) {
                            proposal.contextual.clear();
                            proposal.contextual_geometry = Arc::clone(&self.geometry);
                        }
                        let unit = proposal.contextual.entry(key).or_insert_with(|| {
                            let mut raw = if proposal.raw.1.is_some() {
                                Raw::with_mana2()
                            } else {
                                Raw::default()
                            };
                            let mut totals = [0.0; 4];
                            self.contributions.apply_contextual(
                                &mut raw,
                                &mut totals,
                                tail,
                                fingers,
                                1.0,
                                &self.positions,
                                &self.geometry,
                            );
                            Box::new((raw, totals))
                        });
                        add_scaled(&mut proposal.raw, &mut proposal.totals, unit, weight);
                    } else {
                        // Default fingers: the compiled tables, as in
                        // `replace_default`.
                        self.contributions.apply(
                            &mut proposal.raw,
                            &mut proposal.totals,
                            tail,
                            weight,
                            &self.geometry,
                        );
                    }
                }
                proposal.aggregated = true;
                if DETAIL {
                    profile.contributions += elapsed::<DETAIL>(contribution_start);
                }
            }
            mapper.return_memos(&mut proposal.memos);
            Ok(())
        })();
        if PROFILE {
            profile.contexts += elapsed::<PROFILE>(contexts_start);
        }
        result?;
        if proposal.projected
            && proposal
                .totals
                .iter()
                .any(|&t| t > 0.0 && t < self.projection_min_frequency / 2.0)
        {
            if proposal.omitted_changes {
                // Numerical fallback needs full fields. Recompute one recorded
                // proposal rather than retaining patches for every trial.
                return self.propose_move_inner::<PROFILE, DETAIL, true, F>(
                    movement, stop, effort, proposal, profile,
                );
            }
            self.materialize(proposal);
        }
        if proposal.totals[0] <= 0.0 || proposal.totals[1] <= 0.0 {
            return Err("layout has no usable letters/bigrams in this corpus".into());
        }
        proposal.valid = true;
        Ok(())
    }

    /// Replace one context's contribution, exactly as trials without
    /// screening do: `usage` is its new combo usage when that changed.
    #[allow(clippy::too_many_arguments)]
    fn apply_change(
        &self,
        raw: &mut Raw,
        totals: &mut [f64; 4],
        id: usize,
        tail: Tail,
        fingers: Option<Tail>,
        usage: Option<Tail>,
        active: Option<mana2_metrics::ActiveStats>,
        grams: Option<&ChordGrams>,
    ) {
        let weight = self.corpus.windows[id].weight;
        if let Some(grams) = grams {
            // A combo context: its chord tails replace the recorded ones.
            let old = self.chord_grams[id].as_deref().unwrap();
            for (grams, sign) in [(old, -1.0), (grams, 1.0)] {
                add_chord_grams(
                    &self.contributions,
                    raw,
                    totals,
                    grams,
                    sign * weight,
                    self.finger_tails.is_some(),
                    &self.positions,
                    &self.geometry,
                );
            }
            return;
        }
        if let Some(active) = active {
            self.contributions
                .replace_active(raw, totals, self.tails[id], tail, weight, active);
        } else if let Some(fingers) = fingers {
            let old_fingers = self.finger_tails.as_ref().unwrap()[id];
            if default_finger_tail(self.tails[id], old_fingers, &self.geometry)
                && default_finger_tail(tail, fingers, &self.geometry)
            {
                self.contributions.replace_default(
                    raw,
                    totals,
                    self.tails[id],
                    tail,
                    weight,
                    &self.geometry,
                );
            } else {
                self.contributions.apply_contextual(
                    raw,
                    totals,
                    self.tails[id],
                    Some(old_fingers),
                    -weight,
                    &self.positions,
                    &self.geometry,
                );
                self.contributions.apply_contextual(
                    raw,
                    totals,
                    tail,
                    Some(fingers),
                    weight,
                    &self.positions,
                    &self.geometry,
                );
            }
        } else {
            self.contributions.replace_default(
                raw,
                totals,
                self.tails[id],
                tail,
                weight,
                &self.geometry,
            );
        }
        if let Some(usage) = usage {
            self.contributions.apply_combo_usage(
                raw,
                totals,
                self.combo_usage[id],
                -weight,
                &self.geometry,
            );
            self.contributions
                .apply_combo_usage(raw, totals, usage, weight, &self.geometry);
        }
    }

    /// A tail and its contextual fingers as a screening key; default
    /// fingers contribute exactly as no fingers.
    #[inline]
    fn tail_key(&self, tail: Tail, fingers: Option<Tail>) -> u64 {
        // `default_finger_tail` on the compact per-slot fingers.
        let contextual = fingers.filter(|fingers| {
            !tail.iter().zip(fingers).all(|(&slot, &finger)| {
                slot == GAP
                    || finger != GAP && self.default_fingers[slot as usize] == finger as usize
            })
        });
        pack_key(tail, contextual.unwrap_or([GAP; 3]), false)
    }

    /// Screen trials by each distinct tail's net frequency instead of every
    /// context's (`Proposal::aggregated`); commits replay them exactly.
    pub(crate) fn enable_aggregation(&mut self) {
        self.aggregate = true;
        self.build_tail_groups();
    }

    fn build_tail_groups(&mut self) {
        let mut index: HashMap<[u8; 3], u32> = HashMap::new();
        self.tail_groups.clear();
        self.group_of = vec![NO_GROUP; self.tails.len()];
        for (id, window) in self.corpus.windows.iter().enumerate() {
            let key = if let Some(bindings) = self.static_contexts[id] {
                let end = window.end();
                let mut key = [crate::action_fast::UNTYPED; 3];
                for j in 0..end.min(3) {
                    key[2 - j] = bindings[end - 1 - j];
                }
                key
            } else if self.direct_contexts[id] {
                // A direct tail is its characters' literal keys, whose
                // bindings move with them.
                self.tails[id].map(|slot| {
                    if slot == GAP {
                        crate::action_fast::UNTYPED
                    } else {
                        self.keys.binding(usize::from(slot)) as u8
                    }
                })
            } else {
                continue;
            };
            let groups = &mut self.tail_groups;
            let group = *index.entry(key).or_insert_with(|| {
                groups.push(TailGroup {
                    bindings: key,
                    tail: [GAP; 3],
                    default_weight: 0.0,
                    contextual: Vec::new(),
                    members: Vec::new(),
                });
                (groups.len() - 1) as u32
            });
            self.tail_groups[group as usize].members.push(id as u32);
            self.group_of[id] = group;
        }
        self.group_postings = vec![Vec::new(); self.positions.len()];
        for (group, tail_group) in self.tail_groups.iter().enumerate() {
            let key = tail_group.bindings;
            for (j, &binding) in key.iter().enumerate() {
                if binding != crate::action_fast::UNTYPED && !key[..j].contains(&binding) {
                    self.group_postings[usize::from(binding)].push(group as u32);
                }
            }
        }
        for (grouped, direct) in self.grouped_postings.iter_mut().zip(&self.direct_postings) {
            grouped.clear();
            grouped.extend(
                direct
                    .iter()
                    .filter(|&&id| !self.direct_contexts[id as usize]),
            );
        }
        self.refresh_tail_groups();
    }

    /// Recompute each group's tail and default-finger frequency from its
    /// members, as after a full remap.
    fn refresh_tail_groups(&mut self) {
        for group in &mut self.tail_groups {
            group.default_weight = 0.0;
            group.contextual.clear();
            group.tail = self.tails[group.members[0] as usize];
            for &member in &group.members {
                let id = member as usize;
                let contextual = self.finger_tails.as_ref().is_some_and(|tails| {
                    !default_fingers_of(&self.default_fingers, self.tails[id], tails[id])
                });
                if contextual {
                    group.contextual.push(member);
                } else {
                    group.default_weight += self.corpus.windows[id].weight;
                }
            }
        }
    }

    /// Recompute a screened proposal context by context, in the mapper's
    /// ascending order, so its fields match an unscreened trial bit for bit.
    pub(crate) fn exact(&self, proposal: &mut Proposal) {
        self.materialize(proposal);
    }

    /// Merge screened chord relabels into the proposal's per-context changes,
    /// ascending, as an unscreened trial records them.
    fn expand_chord_relabels(&self, proposal: &mut Proposal) {
        if proposal.chord_relabels.is_empty() {
            return;
        }
        let movement = proposal.movement;
        let changes = std::mem::take(&mut proposal.changes);
        let chords = std::mem::take(&mut proposal.chord_changes);
        let relabels = &proposal.chord_relabels;
        let mut merged = Vec::with_capacity(changes.len() + relabels.len());
        let mut ids = relabels.iter().map(|&id| id as usize).peekable();
        for change in changes {
            while let Some(id) = ids.next_if(|&id| id < change.0) {
                merged.push((id, [GAP; 3]));
            }
            merged.push(change);
        }
        merged.extend(ids.map(|id| (id, [GAP; 3])));
        let relabel = |id: usize| (id, self.chord_grams[id].as_deref().unwrap().moved(movement));
        let mut merged_chords = Vec::with_capacity(chords.len() + relabels.len());
        let mut ids = relabels.iter().map(|&id| id as usize).peekable();
        for chord in chords {
            while let Some(id) = ids.next_if(|&id| id < chord.0) {
                merged_chords.push(relabel(id));
            }
            merged_chords.push(chord);
        }
        merged_chords.extend(ids.map(relabel));
        proposal.changes = merged;
        proposal.chord_changes = merged_chords;
        proposal.chord_relabels.clear();
    }

    /// Merge moved tail groups' members into the proposal's per-context
    /// changes, ascending, as an unscreened trial records them.
    fn expand_group_changes(&self, proposal: &mut Proposal) {
        if proposal.group_changes.is_empty() {
            return;
        }
        let mut expanded: Vec<(usize, Tail, Option<Tail>)> = Vec::new();
        for &(group, tail) in &proposal.group_changes {
            let group = &self.tail_groups[group as usize];
            for &member in &group.members {
                let id = member as usize;
                let fingers = self
                    .finger_tails
                    .as_ref()
                    .map(|tails| moved_fingers(tails[id], group.tail, tail, &self.default_fingers));
                expanded.push((id, tail, fingers));
            }
        }
        proposal.group_changes.clear();
        let mut fingers = proposal.finger_changes.iter();
        expanded.extend(proposal.changes.iter().map(|&(id, tail)| {
            (
                id,
                tail,
                self.finger_tails
                    .as_ref()
                    .map(|_| fingers.next().unwrap().1),
            )
        }));
        expanded.sort_unstable_by_key(|&(id, ..)| id);
        proposal.changes.clear();
        proposal.finger_changes.clear();
        for (id, tail, fingers) in expanded {
            proposal.changes.push((id, tail));
            if let Some(fingers) = fingers {
                proposal.finger_changes.push((id, fingers));
            }
        }
    }

    /// Keep tail groups in step with committed per-context changes.
    fn update_tail_groups(&mut self, proposal: &Proposal) {
        if self.tail_groups.is_empty() {
            return;
        }
        for (index, &(id, tail)) in proposal.changes.iter().enumerate() {
            let group = self.group_of[id];
            if group == NO_GROUP {
                continue;
            }
            let group = &mut self.tail_groups[group as usize];
            group.tail = tail;
            let Some(tails) = &self.finger_tails else {
                continue;
            };
            let before = default_fingers_of(&self.default_fingers, self.tails[id], tails[id]);
            let after = default_fingers_of(
                &self.default_fingers,
                tail,
                proposal.finger_changes[index].1,
            );
            if before == after {
                continue;
            }
            let weight = self.corpus.windows[id].weight;
            let member = id as u32;
            if after {
                group.default_weight += weight;
                group.contextual.retain(|&other| other != member);
            } else {
                group.default_weight -= weight;
                let at = group.contextual.partition_point(|&other| other < member);
                group.contextual.insert(at, member);
            }
        }
    }

    fn materialize(&self, proposal: &mut Proposal) {
        assert!(
            !proposal.omitted_changes,
            "score-only proposal must be replayed before commit"
        );
        if proposal.aggregated {
            self.expand_chord_relabels(proposal);
            self.expand_group_changes(proposal);
            proposal.raw.clone_from(&self.raw);
            proposal.totals = self.totals;
            proposal.speed_populations = self.corpus.speed_populations;
            let mut fingers = proposal.finger_changes.iter();
            let mut usages = proposal.combo_usage_changes.iter().peekable();
            let mut chords = proposal.chord_changes.iter().peekable();
            for &(id, tail) in &proposal.changes {
                let usage = usages
                    .next_if(|&&(changed, _)| changed == id)
                    .map(|&(_, usage)| usage);
                let grams = chords
                    .next_if(|(changed, _)| *changed == id)
                    .map(|(_, grams)| grams);
                let fingers = self
                    .finger_tails
                    .as_ref()
                    .map(|_| fingers.next().expect("fingers for every change").1);
                self.apply_change(
                    &mut proposal.raw,
                    &mut proposal.totals,
                    id,
                    tail,
                    fingers,
                    usage,
                    None,
                    grams,
                );
            }
            proposal.aggregated = false;
            return;
        }
        if !proposal.projected {
            return;
        }
        proposal.raw.clone_from(&self.raw);
        proposal.totals = self.totals;
        proposal.speed_populations = self.corpus.speed_populations;
        // Changes retain the mapper's original ascending context order.
        for &(id, tail) in &proposal.changes {
            self.contributions.replace_default(
                &mut proposal.raw,
                &mut proposal.totals,
                self.tails[id],
                tail,
                self.corpus.windows[id].weight,
                &self.geometry,
            );
        }
        proposal.projected = false;
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
        self.materialize(proposal);
        self.update_tail_groups(proposal);
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
            if let Some(context) = self.trial_contexts.get_mut(id) {
                context.set_tail(tail, self.geometry.keys.len() + 1);
            }
        }
        if let Some(tails) = &mut self.finger_tails {
            for &(id, fingers) in &proposal.finger_changes {
                tails[id] = fingers;
            }
        }
        for &(id, usage) in &proposal.combo_usage_changes {
            self.combo_usage[id] = usage;
        }
        for (id, grams) in proposal.chord_changes.drain(..) {
            self.chord_grams[id] = Some(Box::new(grams));
        }
        self.rebuild_direct_groups();
        self.raw.clone_from(&proposal.raw);
        self.totals = proposal.totals;
        self.commits += 1;
        // Preserve the existing periodic rebase and its accumulation order.
        if self.commits % 128 == 0 {
            let rebase_start = clock::<PROFILE>();
            self.raw = if self.raw.1.is_some() {
                Raw::with_mana2()
            } else {
                Raw::default()
            };
            self.totals = [0.0; 4];
            for (id, (window, &tail)) in self.corpus.windows.iter().zip(&self.tails).enumerate() {
                self.contributions.apply_contextual(
                    &mut self.raw,
                    &mut self.totals,
                    tail,
                    self.finger_tails.as_ref().map(|tails| tails[id]),
                    window.weight,
                    &self.positions,
                    &self.geometry,
                );
                if let Some(grams) = &self.chord_grams[id] {
                    add_chord_grams(
                        &self.contributions,
                        &mut self.raw,
                        &mut self.totals,
                        grams,
                        window.weight,
                        self.finger_tails.is_some(),
                        &self.positions,
                        &self.geometry,
                    );
                }
                self.contributions.apply_combo_usage(
                    &mut self.raw,
                    &mut self.totals,
                    self.combo_usage[id],
                    window.weight,
                    &self.geometry,
                );
            }
            // Default-finger frequencies drift as members move in and out.
            if !self.tail_groups.is_empty() {
                self.refresh_tail_groups();
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

    fn assert_mana2_bits(
        actual: &Raw,
        expected: &Raw,
        actual_totals: &[f64; 4],
        expected_totals: &[f64; 4],
    ) {
        let actual = actual.1.as_ref().unwrap();
        let expected = expected.1.as_ref().unwrap();
        for (index, (actual, expected)) in actual.0.iter().zip(expected.0).enumerate() {
            assert_eq!(
                actual.to_bits(),
                expected.to_bits(),
                "Mana2 raw field {index}"
            );
        }
        for (name, actual, expected) in [
            ("bigram", &actual.1.bigrams, &expected.1.bigrams),
            ("skipgram", &actual.1.skipgrams, &expected.1.skipgrams),
        ] {
            for (finger, (actual, expected)) in actual.iter().zip(expected).enumerate() {
                assert_eq!(
                    actual.to_bits(),
                    expected.to_bits(),
                    "Mana2 {name} speed finger {finger}"
                );
            }
        }
        for index in 0..4 {
            assert_eq!(
                actual_totals[index].to_bits(),
                expected_totals[index].to_bits(),
                "Mana2 total {index}"
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
                            let mut expected = Raw(
                                std::array::from_fn(|i| if i % 2 == 0 { 1234.125 } else { -0.0 }),
                                None,
                            );
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
    fn contextual_fingers_change_metrics_and_incremental_swaps_match_fresh_evaluation() {
        let mut layout = plain();
        layout.alt_fingers = crate::alt_fingers::parse_json(
            &crate::parse_json(r#"[{"ngram":["q",null,"g"],"fingers":["LI",null,"LI"]}]"#).unwrap(),
        )
        .unwrap();
        // As the optimizers do, the rules stay on their keys while letters move.
        crate::alt_fingers::pin(&mut layout);
        let corpus = corpus(b"qag", 3);
        let stop = AtomicBool::new(false);
        let progress = AtomicU64::new(0);
        let enabled = corpus
            .evaluate_with_fingers(
                &layout,
                &stop,
                &progress,
                crate::alt_fingers::AltPolicy::layout(true),
                |_, _, _| 0.0,
            )
            .unwrap();
        let disabled = corpus
            .evaluate_with_fingers(
                &layout,
                &stop,
                &progress,
                crate::alt_fingers::AltPolicy::layout(false),
                |_, _, _| 0.0,
            )
            .unwrap();
        let weights = Weights::default();
        let (enabled_raw, _) = raw_from_counts(&layout, &enabled, &weights, false);
        let (disabled_raw, _) = raw_from_counts(&layout, &disabled, &weights, false);
        assert_eq!(enabled_raw.0[SFS], 1.0);
        assert_eq!(disabled_raw.0[SFS], 0.0);

        let geometry = Geometry::new(crate::action_ui::physical_keys(&layout));
        let mut cache = Incremental::new_with_fingers(
            &corpus,
            &layout,
            geometry.clone(),
            &stop,
            crate::alt_fingers::AltPolicy::layout(true),
            |_, _, _| 0.0,
        )
        .unwrap();
        assert_eq!(cache.raw.0[SFS], enabled_raw.0[SFS]);
        let q = key(&layout, "q");
        let p = key(&layout, "p");
        let mut proposal = Proposal::new();
        cache
            .propose_swap(q, p, &stop, |_, _, _| 0.0, &mut proposal)
            .unwrap();
        cache.commit(&mut proposal);
        let mut moved = layout.clone();
        moved.swap(q, p);
        let fresh = Incremental::new_with_fingers(
            &corpus,
            &moved,
            geometry,
            &stop,
            crate::alt_fingers::AltPolicy::layout(true),
            |_, _, _| 0.0,
        )
        .unwrap();
        assert_contribution_bits(&cache.raw, &fresh.raw, &cache.totals, &fresh.totals);
    }

    #[test]
    fn position_alternates_follow_keys_through_incremental_swaps() {
        // QWERTY f and g positions, whatever letters the swaps put there.
        let rules = crate::alt_fingers::AltRules::parse("fg f_g (LM LI) gf g_f (LI LM)").unwrap();
        let alts = crate::alt_fingers::AltPolicy {
            enabled: true,
            positions: crate::alt_fingers::AltRules {
                rowstag: rules.clone(),
                colstag: rules,
            }
            .intern(),
        };
        let layout = plain();
        let corpus = corpus(b"fg gf fag dfg tgf frg the fat gas jog hug fig sap", 4);
        let stop = AtomicBool::new(false);
        let geometry = Geometry::new(crate::action_ui::physical_keys(&layout));
        for finite in [false, true] {
            let mut cache = Incremental::new_with_fingers(
                &corpus,
                &layout,
                geometry.clone(),
                &stop,
                alts,
                |_, _, _| 0.0,
            )
            .unwrap();
            if finite {
                cache.enable_finite_effort();
            }
            let mut moved = layout.clone();
            for (a, b) in [("f", "s"), ("g", "h"), ("t", "a"), ("s", "g"), ("d", "t")] {
                let (a, b) = (key(&moved, a), key(&moved, b));
                let mut proposal = Proposal::new();
                cache
                    .propose_swap(a, b, &stop, |_, _, _| 0.0, &mut proposal)
                    .unwrap();
                cache.commit(&mut proposal);
                moved.swap(a, b);
                let fresh = Incremental::new_with_fingers(
                    &corpus,
                    &moved,
                    geometry.clone(),
                    &stop,
                    alts,
                    |_, _, _| 0.0,
                )
                .unwrap();
                // Repeated swaps round travel differently; counts stay exact.
                for field in 0..N_RAW {
                    let (a, b) = (cache.raw.0[field], fresh.raw.0[field]);
                    if [TRAVEL, VTRAVEL, LTRAVEL, SFTRAVEL].contains(&field) {
                        assert!(
                            (a - b).abs() <= 1e-12 * a.abs().max(1.0),
                            "raw field {field}"
                        );
                    } else {
                        assert_eq!(a.to_bits(), b.to_bits(), "raw field {field}");
                    }
                }
                assert_eq!(
                    cache.totals.map(f64::to_bits),
                    fresh.totals.map(f64::to_bits)
                );
            }
        }
        let without = Incremental::new_with_fingers(
            &corpus,
            &layout,
            geometry.clone(),
            &stop,
            crate::alt_fingers::AltPolicy::layout(true),
            |_, _, _| 0.0,
        )
        .unwrap();
        let with =
            Incremental::new_with_fingers(&corpus, &layout, geometry, &stop, alts, |_, _, _| 0.0)
                .unwrap();
        assert!(with.raw.0[SFB] < without.raw.0[SFB]);
    }

    #[test]
    fn combo_trials_remap_only_affected_contexts_and_match_fresh_evaluation() {
        // Participants differ from outputs: moving q changes every "aa" chord.
        // f and g together (fig, gift) take the position rules' fingers.
        let text =
            b"aa naun taa xq qp the sdf fun aun aa zu fig gift fog egg gas sag sig fin fit\n";
        check_combo_trials(
            r#"[
                {"keys":["q","p"],"output":"aa"},
                {"keys":["s","f"],"output":"un"}
            ]"#,
            crate::combos::Policy::ALL,
            text,
        );
        // Same-finger neighbours are chords like any other pair.
        check_combo_trials(
            r#"[
                {"keys":["e","d"],"output":"aa"},
                {"keys":["f","r"],"output":"un"}
            ]"#,
            crate::combos::Policy::ALL,
            text,
        );
        // Fitted-shape combos type their own keys' characters, so a move only
        // remaps contexts holding a moved key or starting inside an output.
        let own = b"red fred deed ed fr dc frd cd de rf dcd edd ffr\n";
        for policy in [
            crate::combos::Policy::from(true),
            crate::combos::Policy::ALL,
        ] {
            check_combo_trials(
                r#"[
                    {"keys":["e","d"],"output":"ed"},
                    {"keys":["f","r"],"output":"fr"},
                    {"keys":["d","c"],"output":"dc"}
                ]"#,
                policy,
                own,
            );
        }
    }

    #[test]
    fn combo_trials_with_magic_and_swaps_match_fresh_evaluation() {
        // Magic repeats and adaptive swaps make letters ambiguous, so the
        // mapper looks ahead: a chord context's following character matters.
        for layout in [
            crate::action_keys::test_layouts::COMPACT,
            crate::action_keys::test_layouts::SHORTHAND,
        ] {
            // Chord contexts no effort choice reaches still move by
            // relabelling, checked against a remap.
            assert!(check_combo_trials_on(
                ak::Layout::parse(layout, Path::new("magic.dat")).unwrap(),
                r#"[{"keys":["q","p"],"output":"th"},{"keys":["s","x"],"output":"ea"}]"#,
                crate::combos::Policy::from(true),
                b"the aa hr ik rl ye nr hnr thaa seat oak rkaa i' ja kea th fig gift gas sag\n",
            ));
        }
    }

    fn raw_bits(raw: &Raw) -> Vec<u64> {
        let mut bits: Vec<u64> = raw.0.iter().map(|value| value.to_bits()).collect();
        if let Some(stats) = &raw.1 {
            bits.extend(stats.0.iter().map(|value| value.to_bits()));
            bits.extend(stats.1.bigrams.iter().map(|value| value.to_bits()));
            bits.extend(stats.1.skipgrams.iter().map(|value| value.to_bits()));
        }
        bits
    }

    fn check_combo_trials(definitions: &str, policy: crate::combos::Policy, text: &[u8]) {
        // Plain letters move chord contexts by relabelling their grams, which
        // the trial loop checks against a remap.
        assert!(check_combo_trials_on(plain(), definitions, policy, text));
    }

    /// Whether some trial moved chord contexts by relabelling (`chord_direct`).
    fn check_combo_trials_on(
        mut layout: ak::Layout,
        definitions: &str,
        policy: crate::combos::Policy,
        text: &[u8],
    ) -> bool {
        let mut relabelled = false;
        let Json::Array(combos) = crate::parse_json(definitions).unwrap() else {
            unreachable!()
        };
        layout.combos = combos;
        let corpus = corpus(text, 5);
        let stop = AtomicBool::new(false);
        // Unequal efforts, so the mapper's look-ahead decides between a
        // repeated letter's own key and the repeat key.
        let effort = |a: Option<usize>, b: Option<usize>, key: usize| {
            let distance = |other: usize| (other as f64 - key as f64).abs();
            0.01 * key as f64
                + b.map_or(0.0, |b| if b == key { 1.0 } else { 0.1 * distance(b) })
                + a.map_or(0.0, |a| 0.05 * distance(a))
        };
        let geometry = Geometry::new(crate::action_ui::physical_keys(&layout));
        // Without alternate fingers, and with position rules on QWERTY f/g,
        // whose moves can reach outside a context's mapping dependencies.
        let rules = crate::alt_fingers::AltRules::parse("fg f_g (LM LI) gf g_f (LI LM)").unwrap();
        let positions = crate::alt_fingers::AltPolicy {
            enabled: true,
            positions: crate::alt_fingers::AltRules {
                rowstag: rules.clone(),
                colstag: rules,
            }
            .intern(),
        };
        // Finite effort trials remap only contexts whose dependencies a move
        // touches; both paths must match a fresh evaluation. Without Mana2
        // counters, screened finite trials move direct contexts by tail group.
        for (alts, finite, mana2) in [
            (crate::alt_fingers::AltPolicy::layout(false), false, false),
            (crate::alt_fingers::AltPolicy::layout(false), true, true),
            (crate::alt_fingers::AltPolicy::layout(false), true, false),
            (positions, false, false),
            (positions, true, true),
            (positions, true, false),
        ] {
            let fresh = |layout: &ak::Layout| {
                Incremental::new_with_policy(
                    &corpus,
                    layout,
                    geometry.clone(),
                    &stop,
                    alts,
                    policy,
                    effort,
                )
                .unwrap()
            };
            // Incremental updates subtract and re-add contributions, so they can
            // differ from a fresh sum in the last bits; a missed context could not.
            let assert_close = |raw: &Raw, totals: &[f64; 4], expected: &Incremental| {
                let close = |a: f64, b: f64| (a - b).abs() <= 1e-9 * (1.0 + b.abs());
                for (field, (a, b)) in raw.0.iter().zip(&expected.raw.0).enumerate() {
                    assert!(close(*a, *b), "raw field {field}: {a} vs {b}");
                }
                for (a, b) in totals.iter().zip(&expected.totals) {
                    assert!(close(*a, *b), "totals: {a} vs {b}");
                }
            };
            let mut cache = fresh(&layout);
            if finite {
                cache.enable_finite_effort();
                if mana2 {
                    cache.enable_mana2();
                }
            }
            // Screened trials sum each tail's net frequency; replayed, they match
            // the unscreened proposal bit for bit.
            let mut screened = fresh(&layout);
            if finite {
                screened.enable_finite_effort();
                if mana2 {
                    screened.enable_mana2();
                }
            }
            screened.enable_aggregation();
            for (id, &direct) in screened.direct_contexts.iter().enumerate() {
                assert!(!direct || screened.group_of[id] != NO_GROUP);
            }
            let mut screened_proposal = Proposal::new();
            // Every combo context is pressed as chords.
            assert!(cache
                .chord_grams
                .iter()
                .flatten()
                .any(|grams| grams.len > 0));
            let slots: Vec<_> = (0..layout.slots.len())
                .filter(|&slot| !matches!(layout.slots[slot].binding, ak::Binding::Empty))
                .collect();
            let mut proposal = Proposal::new();
            let mut partial = 0;
            for (i, &a) in slots.iter().enumerate() {
                for &b in &slots[i + 1..] {
                    cache
                        .propose_swap(a, b, &stop, effort, &mut proposal)
                        .unwrap();
                    partial += usize::from(!proposal.all_contexts);
                    let mut moved = layout.clone();
                    moved.swap(a, b);
                    assert_close(&proposal.raw, &proposal.totals, &fresh(&moved));
                    screened
                        .propose_swap(a, b, &stop, effort, &mut screened_proposal)
                        .unwrap();
                    assert_close(
                        &screened_proposal.raw,
                        &screened_proposal.totals,
                        &fresh(&moved),
                    );
                    screened.exact(&mut screened_proposal);
                    assert_eq!(
                        raw_bits(&screened_proposal.raw),
                        raw_bits(&proposal.raw),
                        "{a} {b}"
                    );
                    assert_eq!(
                        screened_proposal.totals.map(f64::to_bits),
                        proposal.totals.map(f64::to_bits)
                    );
                }
            }
            assert!(
                partial > 0,
                "combo swaps should no longer remap every context"
            );
            relabelled |= finite && cache.chord_direct.iter().any(|&direct| direct);

            // Committed chains, including three-key cycles, stay exact.
            let mut current = layout.clone();
            let (q, p, s, a, n, x) = (
                key(&layout, "q"),
                key(&layout, "p"),
                key(&layout, "s"),
                key(&layout, "a"),
                key(&layout, "n"),
                key(&layout, "x"),
            );
            let (f, g, i) = (key(&layout, "f"), key(&layout, "g"), key(&layout, "i"));
            for movement in [
                Move {
                    slots: [q, x, 0],
                    len: 2,
                },
                Move {
                    slots: [a, s, n],
                    len: 3,
                },
                Move {
                    slots: [p, a, 0],
                    len: 2,
                },
                Move {
                    slots: [x, n, q],
                    len: 3,
                },
                // Rule positions move: contextual fingers change, and with
                // them tail groups' members.
                Move {
                    slots: [f, s, 0],
                    len: 2,
                },
                Move {
                    slots: [g, n, a],
                    len: 3,
                },
                Move {
                    slots: [q, p, 0],
                    len: 2,
                },
                // "fi" now meets the rules: contexts take contextual fingers.
                Move {
                    slots: [g, i, 0],
                    len: 2,
                },
                Move {
                    slots: [q, x, 0],
                    len: 2,
                },
            ] {
                cache
                    .propose_move_profiled::<false, false, _>(
                        movement,
                        &stop,
                        effort,
                        &mut proposal,
                        &mut Profile::default(),
                    )
                    .unwrap();
                cache.commit(&mut proposal);
                // Screened commits replay exactly, groups and all.
                screened
                    .propose_move_profiled::<false, false, _>(
                        movement,
                        &stop,
                        effort,
                        &mut screened_proposal,
                        &mut Profile::default(),
                    )
                    .unwrap();
                screened.commit(&mut screened_proposal);
                assert_eq!(raw_bits(&screened.raw), raw_bits(&cache.raw));
                assert_eq!(screened.tails, cache.tails);
                assert_eq!(screened.finger_tails, cache.finger_tails);
                let (o, t, y) = (key(&layout, "o"), key(&layout, "t"), key(&layout, "y"));
                for (a, b) in [(q, s), (p, x), (n, a), (f, x), (i, o), (t, y), (i, t)] {
                    cache
                        .propose_swap(a, b, &stop, effort, &mut proposal)
                        .unwrap();
                    screened
                        .propose_swap(a, b, &stop, effort, &mut screened_proposal)
                        .unwrap();
                    // Screened scores follow the groups' bookkeeping.
                    let approximate = raw_bits(&screened_proposal.raw);
                    screened.exact(&mut screened_proposal);
                    assert_eq!(
                        raw_bits(&screened_proposal.raw),
                        raw_bits(&proposal.raw),
                        "after commits: {a} {b}"
                    );
                    for (approximate, exact) in approximate.iter().zip(raw_bits(&proposal.raw)) {
                        let (approximate, exact) =
                            (f64::from_bits(*approximate), f64::from_bits(exact));
                        assert!(
                            (approximate - exact).abs() <= 1e-9 * (1.0 + exact.abs()),
                            "{a} {b}"
                        );
                    }
                }
                current.swap(movement.slots[0], movement.slots[1]);
                if movement.len == 3 {
                    current.swap(movement.slots[1], movement.slots[2]);
                }
                assert_close(&cache.raw, &cache.totals, &fresh(&current));
            }
        }
        relabelled
    }

    #[test]
    fn action_winners_keep_current_slots_before_contextual_fingers_are_applied() {
        let mut layout = magic();
        // The magic key (on QWERTY .) types the second a of "aan". Rules
        // name keys, so a position rule gives it the right pinky.
        let rule = |text| crate::alt_fingers::AltPolicy {
            enabled: true,
            positions: crate::alt_fingers::AltRules {
                rowstag: crate::alt_fingers::AltRules::parse(text).unwrap(),
                colstag: crate::alt_fingers::AltRules::parse(text).unwrap(),
            }
            .intern(),
        };
        let corpus = corpus(b"aan", 3);
        let stop = AtomicBool::new(false);
        let progress = AtomicU64::new(0);
        let evaluate = |layout: &ak::Layout, enabled| {
            let action = action_key(layout, "@");
            corpus
                .evaluate_with_fingers(layout, &stop, &progress, enabled, |_, _, key| {
                    if key == action {
                        -100.0
                    } else {
                        0.0
                    }
                })
                .unwrap()
        };
        let original_action = action_key(&layout, "@");
        let original = evaluate(&layout, rule("a.n (. RP .)"));
        assert!(original.finger_tables.as_ref().unwrap()[2]
            .keys()
            .any(|pattern| pattern.slots[1] == original_action && pattern.fingers[1] == 7));
        assert!(
            evaluate(&layout, crate::alt_fingers::AltPolicy::layout(false))
                .finger_tables
                .is_none()
        );

        let other = key(&layout, "n");
        layout.swap(original_action, other);
        let moved_action = action_key(&layout, "@");
        // It is on QWERTY n now, and n on QWERTY . instead.
        let moved = evaluate(&layout, rule("an. (. RP .)"));
        assert_ne!(moved_action, original_action);
        let moved_patterns = &moved.finger_tables.as_ref().unwrap()[2];
        assert!(
            moved_patterns
                .keys()
                .any(|pattern| pattern.slots[1] == moved_action && pattern.fingers[1] == 7),
            "action {moved_action}; patterns {moved_patterns:?}"
        );
    }

    #[test]
    fn bounded_adaptive_action_swaps_match_fresh_mana2_mapping() {
        // Neon-style uppercase/lowercase adaptive actions can each emit only
        // `y` or `#`. Moving either root therefore needs those source postings,
        // rather than every retained corpus context.
        let source =
            format!("{BASE}outer-left: @Y ~ ~\nadaptive Y a# by cy\nadaptive y ay b# c#\n");
        let seed = ak::Layout::parse(&source, Path::new("adaptive-output-set.dat")).unwrap();
        let corpus = corpus(b"away byway cab yay #y abcde pqrst uvwxy", 5);
        let stop = AtomicBool::new(false);
        let geometry = Geometry::new(crate::action_ui::physical_keys(&seed));
        let make = |layout: &ak::Layout| {
            let mut cache =
                Incremental::new(&corpus, layout, geometry.clone(), &stop, |_, _, key| {
                    key as f64 * 0.01
                })
                .unwrap();
            // Dedicated Mana2 search proves this finite, side-effect-free
            // effort contract before enabling its terminal mapper fast path.
            cache.enable_finite_effort();
            cache.enable_mana2();
            cache
        };

        for (action, literal) in [("Y", "q"), ("y", "p")] {
            let cache = make(&seed);
            let a = action_key(&seed, action);
            let b = key(&seed, literal);
            let mut proposal = Proposal::new();
            cache
                .propose_swap(a, b, &stop, |_, _, key| key as f64 * 0.01, &mut proposal)
                .unwrap();
            assert!(!proposal.all_contexts);
            assert!(proposal.examined > 0);
            assert!(proposal.examined < cache.tails.len());

            let mut moved = seed.clone();
            moved.swap(a, b);
            let fresh = make(&moved);
            for (actual, expected) in proposal.raw.0.iter().zip(fresh.raw.0) {
                assert!((actual - expected).abs() < 1e-8);
            }
            for (actual, expected) in proposal.totals.iter().zip(fresh.totals) {
                assert!((actual - expected).abs() < 1e-8);
            }
            let actual = proposal.raw.1.as_ref().unwrap();
            let expected = fresh.raw.1.as_ref().unwrap();
            for (actual, expected) in actual.0.iter().zip(expected.0) {
                assert!((actual - expected).abs() < 1e-8);
            }
            for (actual, expected) in actual
                .1
                .bigrams
                .iter()
                .chain(&actual.1.skipgrams)
                .zip(expected.1.bigrams.iter().chain(&expected.1.skipgrams))
            {
                assert!((actual - expected).abs() < 1e-8);
            }
        }
    }

    #[test]
    fn mana2_only_candidates_rebases_and_restores_match_full_updates() {
        let mut layout = plain();
        let Json::Array(combos) = crate::parse_json(
            r#"[
                {"keys":["q","a"],"output":"aa"},
                {"keys":["e","d"],"output":"un"}
            ]"#,
        )
        .unwrap() else {
            unreachable!()
        };
        layout.combos = combos;
        layout.alt_fingers = crate::alt_fingers::parse_json(
            &crate::parse_json(
                r#"[
                    {"ngram":["q",null,"g"],"fingers":["LI",null,"LI"]},
                    {"ngram":["a","a"],"fingers":["LP","LP"]}
                ]"#,
            )
            .unwrap(),
        )
        .unwrap();
        let corpus = corpus(b"aa\naun\nqag\ntev\n", 5);
        let stop = AtomicBool::new(false);
        let geometry = Geometry::new(crate::action_ui::physical_keys(&layout));
        let make = |only| {
            let mut cache = Incremental::new_with_policy(
                &corpus,
                &layout,
                geometry.clone(),
                &stop,
                crate::alt_fingers::AltPolicy::layout(true),
                crate::combos::Policy::ALL,
                |_, _, _| 0.0,
            )
            .unwrap();
            if only {
                cache.enable_mana2_only();
            } else {
                cache.enable_mana2();
            }
            cache
        };
        let mut full = make(false);
        let mut only = make(true);
        only.enable_mana2_score_only(&mana2_metrics::Weights::default());
        assert!(only.active_mana2.is_none());
        assert!(full.finger_tails.is_some());
        assert!(full.chord_grams.iter().flatten().any(|grams| grams.len > 0));
        assert_mana2_bits(&only.raw, &full.raw, &only.totals, &full.totals);

        let full_saved = full.numeric_state();
        let only_saved = only.numeric_state();
        let original: Vec<_> = (0..layout.slots.len()).collect();
        let q = key(&layout, "q");
        let a = key(&layout, "a");
        let mut full_proposal = Proposal::new();
        let mut only_proposal = Proposal::new();
        full.propose_swap(q, a, &stop, |_, _, _| 0.0, &mut full_proposal)
            .unwrap();
        only.propose_swap(q, a, &stop, |_, _, _| 0.0, &mut only_proposal)
            .unwrap();
        assert_mana2_bits(
            &only_proposal.raw,
            &full_proposal.raw,
            &only_proposal.totals,
            &full_proposal.totals,
        );

        // Exercise the periodic full rebuild as well as incremental updates.
        full.commits = 127;
        only.commits = 127;
        full.commit(&mut full_proposal);
        only.commit(&mut only_proposal);
        assert_eq!(full.commits, 128);
        assert_eq!(only.commits, 128);
        assert_mana2_bits(&only.raw, &full.raw, &only.totals, &full.totals);

        full.restore(&original, Some(&full_saved), &stop, |_, _, _| 0.0)
            .unwrap();
        only.restore(&original, Some(&only_saved), &stop, |_, _, _| 0.0)
            .unwrap();
        assert_eq!(only.tails, full.tails);
        assert_eq!(only.finger_tails, full.finger_tails);
        assert_eq!(only.combo_usage, full.combo_usage);
        assert_mana2_bits(&only.raw, &full.raw, &only.totals, &full.totals);
    }

    #[test]
    fn three_key_moves_match_fresh_mapping_and_restore_numeric_checkpoints() {
        let seed = magic();
        let corpus = corpus(b"ii aa i' ai' qi! rrr rk aa zz abcdef qgnev", 5);
        let weights = Weights::default();
        let stop = AtomicBool::new(false);
        let effort = crate::action_ui::LocalEffort::new(&seed, &weights);
        let geometry = Geometry::new(crate::action_ui::physical_keys(&seed));
        let make = |layout: &ak::Layout| {
            let mut cache =
                Incremental::new(&corpus, layout, geometry.clone(), &stop, |a, b, key| {
                    effort.get(a, b, key)
                })
                .unwrap();
            cache.enable_mana2();
            cache
        };
        let mut cache = make(&seed);
        let original = cache.numeric_state();
        let original_tails = cache.tails.clone();
        let identity: Vec<_> = (0..seed.slots.len()).collect();
        let mut proposal = Proposal::new();
        let sparse_weights = |selected: &[(&str, &str)]| {
            let text = mana2_metrics::WEIGHTABLE_STATS
                .iter()
                .map(|&index| {
                    let name = mana2_metrics::STAT_IDS[index];
                    let value = selected
                        .iter()
                        .find(|(id, _)| *id == name)
                        .map_or("0", |(_, value)| *value);
                    format!("{name} = {value}")
                })
                .collect::<Vec<_>>()
                .join("\n");
            mana2_metrics::Weights::from_text(&text).unwrap()
        };
        let objectives = [
            mana2_metrics::Weights::default(),
            sparse_weights(&[("fspeed", "-1"), ("weighted_speed", "-1")]),
            sparse_weights(&[("fspeed-LI", "-1"), ("weighted-speed-LP", "-2")]),
        ];
        let assert_trial_contexts = |cache: &Incremental| {
            assert_eq!(std::mem::size_of::<TrialContext>(), 16);
            assert_eq!(cache.trial_contexts.len(), cache.tails.len());
            assert_eq!(cache.prepared_contexts.len(), cache.tails.len());
            let mut grouped = vec![false; cache.tails.len()];
            for group in &cache.direct_groups {
                assert!(group.contexts.windows(2).all(|ids| ids[0] < ids[1]));
                let mut frequency = 0.0;
                for &id in &group.contexts {
                    let id = id as usize;
                    assert!(!grouped[id]);
                    grouped[id] = true;
                    let context = cache.trial_contexts[id];
                    assert!(context.direct());
                    assert_eq!(context.tail, group.tail);
                    assert_eq!(context.tail_id, group.tail_id);
                    frequency += context.frequency;
                }
                assert_eq!(frequency.to_bits(), group.frequency.to_bits());
            }
            if cache.exact_integer_mass.is_some() {
                assert_eq!(grouped, cache.direct_contexts);
            }
            for (id, context) in cache.trial_contexts.iter().enumerate() {
                let window = &cache.corpus.windows[id];
                assert_eq!(context.frequency.to_bits(), window.weight.to_bits());
                assert_eq!(context.tail, cache.tails[id]);
                assert_eq!(context.direct(), cache.direct_contexts[id]);
                assert_eq!(
                    cache.prepared_contexts[id],
                    crate::action_fast::PreparedContext::new(
                        &window.text()[context.source_start()..],
                        window.end() - context.source_start(),
                    )
                    .unwrap()
                );
                assert_eq!(
                    context.source_start(),
                    cache.keys.terminal_dependency_start(
                        &cache.program,
                        window.text(),
                        window.end(),
                    )
                );
                assert_eq!(
                    context.tail_id,
                    TrialContext::packed_tail_id(cache.tails[id], cache.geometry.keys.len() + 1),
                );
            }
        };
        for (iteration, labels) in [["a", "i", "q"], ["@", "a", "r"], ["q", "@", "i"]]
            .into_iter()
            .enumerate()
        {
            let mut projected = make(&seed);
            projected.enable_mana2_score_only(&objectives[iteration]);
            projected.enable_finite_effort();
            assert!(projected.active_mana2.is_some());
            assert_trial_contexts(&projected);
            let projected_original = projected.numeric_state();
            let mut projected_proposal = Proposal::new();
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
            projected
                .propose_move_profiled::<false, false, _>(
                    movement,
                    &stop,
                    |a, b, key| effort.get(a, b, key),
                    &mut projected_proposal,
                    &mut Profile::default(),
                )
                .unwrap();
            assert!(projected_proposal.projected);
            let mut score_only = Proposal::new();
            projected
                .propose_move_inner::<false, false, false, _>(
                    movement,
                    &stop,
                    |a, b, key| effort.get(a, b, key),
                    &mut score_only,
                    &mut Profile::default(),
                )
                .unwrap();
            assert!(score_only.omitted_changes);
            assert!(score_only.changes.is_empty());
            assert_eq!(
                score_only.direct_plans.len(),
                score_only.direct_plan_ranges.len()
            );
            for &(start, end) in &score_only.direct_plan_ranges {
                assert!(start < end);
                assert!(end as usize <= score_only.direct_numeric_updates.len());
            }
            for &id in &score_only.affected {
                let context = projected.trial_contexts[id as usize];
                if context.direct() && !score_only.direct_plans.is_empty() {
                    let plan_id = score_only.direct_plan_ids[context.tail_id as usize];
                    assert!(plan_id > 0);
                    assert!(score_only.direct_plans[plan_id as usize - 1].has_nonboolean());
                }
            }
            assert_eq!(
                score_only.integer_plans.len(),
                score_only.boolean_frequencies.len()
            );
            assert!(score_only
                .boolean_frequencies
                .iter()
                .all(|frequency| *frequency == 0.0));
            assert_mana2_bits(
                &score_only.raw,
                &projected_proposal.raw,
                &score_only.totals,
                &projected_proposal.totals,
            );
            assert_eq!(
                score_only.mana2_score(&objectives[iteration]).to_bits(),
                projected_proposal
                    .mana2_score(&objectives[iteration])
                    .to_bits(),
            );
            assert_eq!(
                projected_proposal
                    .mana2_score(&objectives[iteration])
                    .to_bits(),
                proposal.mana2_score(&objectives[iteration]).to_bits()
            );
            assert_eq!(proposal.all_contexts, labels.contains(&"@"));
            let mut layout = seed.clone();
            layout.swap(slots[0], slots[1]);
            layout.swap(slots[1], slots[2]);
            let fresh = make(&layout);
            for (actual, expected) in proposal.metrics().v.iter().zip(fresh.metrics().v) {
                assert!((actual - expected).abs() < 1e-8);
            }
            assert!(
                (proposal.physical_score(&weights) - fresh.physical_score(&weights)).abs() < 1e-8
            );
            let speed = proposal.raw.1.as_ref().unwrap().1;
            let full_speed = fresh.raw.1.as_ref().unwrap().1;
            for (actual, expected) in speed
                .bigrams
                .iter()
                .chain(&speed.skipgrams)
                .zip(full_speed.bigrams.iter().chain(&full_speed.skipgrams))
            {
                assert!((actual - expected).abs() < 1e-8);
            }
            if iteration == 2 {
                cache.commits = 127;
                projected.commits = 127;
            }
            cache.commit(&mut proposal);
            projected.commit(&mut projected_proposal);
            assert_trial_contexts(&projected);
            assert!(!projected_proposal.projected);
            assert_mana2_bits(&projected.raw, &cache.raw, &projected.totals, &cache.totals);
            assert_eq!(
                projected.mana2_score(&objectives[iteration]).to_bits(),
                cache.mana2_score(&objectives[iteration]).to_bits()
            );
            projected
                .restore(&identity, Some(&projected_original), &stop, |a, b, key| {
                    effort.get(a, b, key)
                })
                .unwrap();
            assert_trial_contexts(&projected);
            assert_mana2_bits(
                &projected.raw,
                &projected_original.raw,
                &projected.totals,
                &projected_original.totals,
            );
            assert_eq!(cache.tails, fresh.tails);
            cache
                .restore(&identity, Some(&original), &stop, |a, b, key| {
                    effort.get(a, b, key)
                })
                .unwrap();
            assert_eq!(cache.tails, original_tails);
            assert_contribution_bits(&cache.raw, &original.raw, &cache.totals, &original.totals);
        }

        // Magic's fallback repeat can emit any byte, so its terminal contexts
        // depend on action history. Use literal keys to exercise prefix pruning.
        let literal_seed = plain();
        let literal_corpus = self::corpus(b"qgnev abcdef aa qyr", 5);
        let literal_effort = crate::action_ui::LocalEffort::new(&literal_seed, &weights);
        let literal_geometry = Geometry::new(crate::action_ui::physical_keys(&literal_seed));
        let make_literal = |layout: &ak::Layout| {
            let mut cache = Incremental::new(
                &literal_corpus,
                layout,
                literal_geometry.clone(),
                &stop,
                |a, b, key| literal_effort.get(a, b, key),
            )
            .unwrap();
            cache.enable_mana2();
            cache
        };
        let full = make_literal(&literal_seed);
        let mut projected = make_literal(&literal_seed);
        projected.enable_mana2_score_only(&objectives[0]);
        projected.enable_finite_effort();
        assert_trial_contexts(&projected);
        let movement = Move {
            slots: ["a", "i", "q"].map(|label| key(&literal_seed, label)),
            len: 3,
        };
        let mut full_proposal = Proposal::new();
        let mut projected_proposal = Proposal::new();
        for (cache, proposal) in [
            (&full, &mut full_proposal),
            (&projected, &mut projected_proposal),
        ] {
            cache
                .propose_move_profiled::<false, false, _>(
                    movement,
                    &stop,
                    |a, b, key| literal_effort.get(a, b, key),
                    proposal,
                    &mut Profile::default(),
                )
                .unwrap();
        }
        let prefix_only = literal_corpus
            .windows
            .iter()
            .position(|window| window.text() == b"qgnev" && window.end() == 5)
            .unwrap();
        assert!(projected_proposal.projected);
        assert!(projected.direct_contexts[prefix_only]);
        assert!(full_proposal.affected.contains(&(prefix_only as u32)));
        assert!(!projected_proposal.affected.contains(&(prefix_only as u32)));
        assert!(projected_proposal.examined < full_proposal.examined);
        assert_eq!(
            projected_proposal.mana2_score(&objectives[0]).to_bits(),
            full_proposal.mana2_score(&objectives[0]).to_bits()
        );

        // An alternating literal tail changes slots but contributes nothing to
        // this objective. Only unrecorded trials may omit its numeric work.
        let inert_weights = sparse_weights(&[("sfb", "-1"), ("sfs", "-1")]);
        let mut inert_cache = make_literal(&literal_seed);
        inert_cache.enable_mana2_score_only(&inert_weights);
        inert_cache.enable_finite_effort();
        assert_trial_contexts(&inert_cache);
        let [a, b] = ["q", "w"].map(|label| key(&literal_seed, label));
        let mut recorded = Proposal::new();
        let mut unrecorded = Proposal::new();
        inert_cache
            .propose_swap(
                a,
                b,
                &stop,
                |a, b, key| literal_effort.get(a, b, key),
                &mut recorded,
            )
            .unwrap();
        let mut profile = Profile::default();
        inert_cache
            .propose_swap_score_profiled::<true, false, _>(
                a,
                b,
                &stop,
                |a, b, key| literal_effort.get(a, b, key),
                &mut unrecorded,
                &mut profile,
            )
            .unwrap();
        assert!(profile.counter_skipped > 0);
        assert!(unrecorded.examined < recorded.examined);
        assert_eq!(profile.mapped, unrecorded.examined as u64);
        assert!(unrecorded.affected.windows(2).all(|ids| ids[0] < ids[1]));
        for &id in &unrecorded.affected {
            let context = inert_cache.trial_contexts[id as usize];
            if context.direct() {
                let plan_id = unrecorded.direct_plan_ids[context.tail_id as usize];
                assert!(plan_id > 0);
                assert!(unrecorded.direct_plans[plan_id as usize - 1].has_nonboolean());
            }
        }
        assert_eq!(
            recorded.mana2_score(&inert_weights).to_bits(),
            unrecorded.mana2_score(&inert_weights).to_bits()
        );
        assert_mana2_bits(
            &recorded.raw,
            &unrecorded.raw,
            &recorded.totals,
            &unrecorded.totals,
        );
        let mut reference = make_literal(&literal_seed);
        let mut full_replay = Proposal::new();
        reference
            .propose_swap(
                a,
                b,
                &stop,
                |a, b, key| literal_effort.get(a, b, key),
                &mut full_replay,
            )
            .unwrap();
        reference.commit(&mut full_replay);
        inert_cache.commit(&mut recorded);
        assert_trial_contexts(&inert_cache);
        assert_eq!(inert_cache.tails, reference.tails);
        assert_mana2_bits(
            &inert_cache.raw,
            &reference.raw,
            &inert_cache.totals,
            &reference.totals,
        );
    }

    #[test]
    fn restoring_checkpoint_restores_the_periodic_rebase_boundary() {
        let seed = magic();
        let corpus = corpus(b"aa ai' abc", 5);
        let stop = AtomicBool::new(false);
        let geometry = Geometry::new(crate::action_ui::physical_keys(&seed));
        let weights = Weights::default();
        let a = key(&seed, "a");
        let i = key(&seed, "i");
        let mut moved = seed.clone();
        moved.swap(a, i);
        for physical in [false, true] {
            let make = |layout: &ak::Layout| {
                let mut cache =
                    Incremental::new(&corpus, layout, geometry.clone(), &stop, |_, _, _| 0.0)
                        .unwrap();
                if physical {
                    cache.enable_mana2();
                }
                cache
            };
            let mut cache = make(&seed);
            let fresh = make(&moved);
            cache.commits = 127;
            let saved = cache.numeric_state();
            let identity: Vec<_> = (0..seed.slots.len()).collect();
            let mut proposal = Proposal::new();
            let mut profile = Profile::default();

            for expected_rebases in 1..=2 {
                cache
                    .propose_swap(a, i, &stop, |_, _, _| 0.0, &mut proposal)
                    .unwrap();
                cache.commit_profiled::<true>(&mut proposal, &mut profile);
                assert_eq!(cache.commits, 128);
                assert_eq!(profile.rebases, expected_rebases);
                assert_eq!(cache.raw.1.is_some(), physical);
                assert_contribution_bits(&cache.raw, &fresh.raw, &cache.totals, &fresh.totals);
                if physical {
                    assert_eq!(
                        cache.physical_score(&weights).to_bits(),
                        fresh.physical_score(&weights).to_bits(),
                    );
                    let speed = cache.raw.1.as_ref().unwrap().1;
                    let expected = fresh.raw.1.as_ref().unwrap().1;
                    assert_eq!(speed.bigrams, expected.bigrams);
                    assert_eq!(speed.skipgrams, expected.skipgrams);
                }
                cache
                    .restore(&identity, Some(&saved), &stop, |_, _, _| 0.0)
                    .unwrap();
                assert_eq!(cache.commits, 127);
            }
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

    fn action_key(l: &ak::Layout, name: &str) -> usize {
        l.slots
            .iter()
            .position(|slot| matches!(&slot.binding, ak::Binding::Named(bound) if bound == name))
            .unwrap()
    }
    #[test]
    fn reused_mappings_match_fresh_evaluations() {
        let text: &[u8] = b"the quick brown fox jumps over the lazy dog; i'd rather see \
            aardvarks drink quietly. she said: wow, those weird owls queue at dusk! \
            red shoes, odd seeds, quotes and quills; it's quite true.";
        let corpus = corpus(text, 5);
        let stop = AtomicBool::new(false);
        let progress = AtomicU64::new(0);
        let effort = |a: Option<usize>, b: Option<usize>, key: usize| {
            key as f64 * 0.25
                + a.map_or(0.5, |a| ((a * 7 + key * 3) % 5) as f64)
                + b.map_or(0.0, |b| ((b + key) % 3) as f64 * 0.1)
        };
        let base = magic();
        let magic_name = base
            .slots
            .iter()
            .find_map(|slot| match &slot.binding {
                ak::Binding::Named(name)
                    if matches!(base.actions.get(name), Some(ak::Action::Rules { .. })) =>
                {
                    Some(name.clone())
                }
                _ => None,
            })
            .unwrap();
        // Own-output, fat-finger and foreign-output combos.
        let combos: Vec<Json> = [
            r#"{"keys":["e","d"],"output":"ed"}"#,
            r#"{"keys":["t","h"],"output":"th"}"#,
            r#"{"keys":["w","e"],"output":"qu"}"#,
            r#"{"keys":["s","w"],"output":"ow"}"#,
        ]
        .iter()
        .map(|text| crate::parse_json(text).unwrap())
        .collect();
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = |limit: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % limit as u64) as usize
        };
        let letters = b"etaoinshrdwqu";
        let mutate = |layout: &ak::Layout, choice: usize, a: usize, b: usize| {
            let mut layout = layout.clone();
            match choice {
                0 => {
                    let combo = &combos[a % combos.len()];
                    match layout
                        .combos
                        .iter()
                        .position(|known| format!("{known:?}") == format!("{combo:?}"))
                    {
                        Some(index) => {
                            layout.combos.remove(index);
                        }
                        None => layout.combos.push(combo.clone()),
                    }
                }
                1 => {
                    let Some(ak::Action::Rules { rules, .. }) = layout.actions.get_mut(&magic_name)
                    else {
                        unreachable!()
                    };
                    let context = vec![letters[a % letters.len()]];
                    if rules.remove(&context).is_none() {
                        rules.insert(
                            context,
                            ak::Emission::Text(vec![letters[b % letters.len()]]),
                        );
                    }
                }
                _ => {
                    // Toggle an adaptive swap: after C the key types Y.
                    let x = letters[a % letters.len()];
                    let slot = layout
                        .slots
                        .iter()
                        .position(|slot| slot.label.as_bytes() == [x])
                        .unwrap();
                    let name = format!("swap-{}", x as char);
                    if matches!(layout.slots[slot].binding, ak::Binding::Named(_)) {
                        layout.slots[slot].binding = ak::Binding::Text(vec![x]);
                        layout.actions.remove(&name);
                    } else {
                        let c = letters[b % letters.len()];
                        let y = letters[(b + 1) % letters.len()];
                        layout.slots[slot].binding = ak::Binding::Named(name.clone());
                        layout.actions.insert(
                            name,
                            ak::Action::Rules {
                                basis: ak::Basis::Text,
                                rules: BTreeMap::from([(vec![c], ak::Emission::Text(vec![y]))]),
                                fallback: ak::Emission::Text(vec![x]),
                            },
                        );
                    }
                }
            }
            layout
        };
        let mut reused_windows = 0;
        let rules = crate::alt_fingers::AltRules::parse("ed e_d (LM LI) de d_e (LI LM)").unwrap();
        let positions = crate::alt_fingers::AltPolicy {
            enabled: true,
            positions: crate::alt_fingers::AltRules {
                rowstag: rules.clone(),
                colstag: rules,
            }
            .intern(),
        };
        let none = crate::alt_fingers::AltPolicy::layout(false);
        for (alts, policy, finite) in [
            (none, crate::combos::Policy::ALL, true),
            (none, crate::combos::Policy::from(true), true),
            (none, crate::combos::Policy::OFF, false),
            (positions, crate::combos::Policy::ALL, true),
        ] {
            let flags = (alts, policy, finite);
            let evaluate = |layout: &ak::Layout, base: Option<&Mappings>| {
                corpus
                    .evaluate_reusing(layout, &stop, &progress, flags, base, true, false, effort)
                    .unwrap()
            };
            // Fitters chain counts that only recount the changed windows.
            let delta = |layout: &ak::Layout, base: Option<&Mappings>| {
                corpus
                    .evaluate_reusing(layout, &stop, &progress, flags, base, true, true, effort)
                    .unwrap()
            };
            let mut layout = mutate(&base, 0, 0, 0);
            let (_, first) = evaluate(&layout, None);
            let first = first.unwrap();
            let mut previous = evaluate(&layout, None).1.unwrap();
            let mut chained = delta(&layout, None).1.unwrap();
            for _ in 0..60 {
                let (choice, a, b) = (next(3), next(64), next(64));
                layout = mutate(&layout, choice, a, b);
                let (fresh, mappings) = evaluate(&layout, None);
                let (counted, next_chained) = delta(&layout, Some(&chained));
                assert_counts_close(&counted, &fresh);
                chained = next_chained.unwrap();
                for known in [&previous, &first] {
                    let (reused, _) = evaluate(&layout, Some(known));
                    super::detailed_mapping_tests::assert_counts_bits(&reused, &fresh);
                    let combos = (policy.enabled && !layout.combos.is_empty())
                        .then(|| crate::combos::Program::new(&layout).unwrap());
                    let source = if combos.is_some() {
                        corpus.chord_contexts().unwrap()
                    } else {
                        corpus.clone()
                    };
                    if let Some(reuse) =
                        known.reuse(&layout, combos.as_ref(), flags, source.windows.len())
                    {
                        reused_windows += source
                            .windows
                            .iter()
                            .filter(|window| !reuse.changes(window.text(), window.left_open))
                            .count();
                    }
                }
                previous = mappings.unwrap();
            }
        }
        assert!(reused_windows > 1000, "{reused_windows}");
    }

    /// `assert_counts_bits` up to rounding, a missing entry counting as zero.
    fn assert_counts_close(actual: &Counts, expected: &Counts) {
        let close = |a: f64, b: f64| (a - b).abs() <= 1e-9 * (1.0 + b.abs());
        for (a, b) in [
            (&actual.tables[0], &expected.tables[0]),
            (&actual.tables[1], &expected.tables[1]),
            (&actual.tables[2], &expected.tables[2]),
            (&actual.skip, &expected.skip),
        ] {
            for key in a.keys().chain(b.keys()) {
                let (x, y) = (
                    a.get(key).copied().unwrap_or(0.0),
                    b.get(key).copied().unwrap_or(0.0),
                );
                assert!(close(x, y), "{key:?}: {x} vs {y}");
            }
        }
        for (x, y) in [
            (actual.characters, expected.characters),
            (actual.presses, expected.presses),
            (actual.action_presses, expected.action_presses),
            (actual.ignored_characters, expected.ignored_characters),
        ] {
            assert!(close(x, y), "{x} vs {y}");
        }
        let finger_tables = |counts: &Counts| {
            counts
                .finger_tables
                .iter()
                .flatten()
                .chain(&counts.finger_skip)
                .cloned()
                .collect::<Vec<_>>()
        };
        let (a, b) = (finger_tables(actual), finger_tables(expected));
        assert_eq!(a.len(), b.len());
        for (a, b) in a.iter().zip(&b) {
            for key in a.keys().chain(b.keys()) {
                let (x, y) = (
                    a.get(key).copied().unwrap_or(0.0),
                    b.get(key).copied().unwrap_or(0.0),
                );
                assert!(close(x, y), "{key:?}: {x} vs {y}");
            }
        }
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
                        *expected[n - 1]
                            .entry(crate::action_ngrams::Slots::new(&gram))
                            .or_default() += 1.0;
                    }
                }
                for gram in ids.windows(3) {
                    *skip.entry(Slots::new(&[gram[0], gram[2]])).or_default() += 1.0;
                }
            }
            assert_eq!(counts.tables, expected);
            assert_eq!(*counts.skip, skip);
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
            counts.tables[1].get(&[key(&l, "i"), key(&l, "@")][..]),
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
            counts.tables[1].get(&[key(&l, "a"), magic_key][..]),
            Some(&1.0)
        );
    }
    #[test]
    fn safe_magic_priority_respects_the_previous_finger() {
        let l = magic();
        let c = corpus(b"aa", 3);
        let magic_key = key(&l, "@");
        let counts = c
            .evaluate(
                &l,
                &AtomicBool::new(false),
                &AtomicU64::new(0),
                |_, _, next| if next == magic_key { 100.0 } else { 0.0 },
            )
            .unwrap();
        assert_eq!(counts.action_presses, 1.0);
        assert_eq!(
            counts.tables[1].get(&[key(&l, "a"), magic_key][..]),
            Some(&1.0)
        );

        let same_finger = ak::Layout::parse(
            "@m w e r t | y u i o p\na s d f g | h j k l ;\nz x c v b | n m , . /\nthumbs: space\naction m = magic\nfallback m = repeat-output\n",
            Path::new("same-finger-magic.dat"),
        )
        .unwrap();
        let same_finger_action = action_key(&same_finger, "m");
        let counts = c
            .evaluate(
                &same_finger,
                &AtomicBool::new(false),
                &AtomicU64::new(0),
                |_, _, next| {
                    if next == same_finger_action {
                        -100.0
                    } else {
                        0.0
                    }
                },
            )
            .unwrap();
        assert_eq!(counts.action_presses, 0.0);
        assert_eq!(
            counts.tables[1].get(&[key(&same_finger, "a"), key(&same_finger, "a")][..]),
            Some(&1.0)
        );
    }
    #[test]
    fn safe_skip_magic_priority_respects_the_two_back_finger() {
        let safe = ak::Layout::parse(
            "q w e r t | y u i o p\na s d f g | h j k l ;\nz x c v b | n m , @sk /\nthumbs: space\naction sk = skip-magic\nfallback sk = repeat-previous-output\n",
            Path::new("safe-skip-magic.dat"),
        )
        .unwrap();
        let c = corpus(b"aba", 3);
        let safe_action = action_key(&safe, "sk");
        let counts = c
            .evaluate(
                &safe,
                &AtomicBool::new(false),
                &AtomicU64::new(0),
                |_, _, next| if next == safe_action { 100.0 } else { 0.0 },
            )
            .unwrap();
        assert_eq!(counts.action_presses, 1.0);
        assert_eq!(
            counts.tables[2].get(&[key(&safe, "a"), key(&safe, "b"), safe_action,][..]),
            Some(&1.0)
        );

        let same_finger = ak::Layout::parse(
            "@sk w e r t | y u i o p\na s d f g | h j k l ;\nz x c v b | n m , . /\nthumbs: space\naction sk = skip-magic\nfallback sk = repeat-previous-output\n",
            Path::new("same-finger-skip-magic.dat"),
        )
        .unwrap();
        let same_finger_action = action_key(&same_finger, "sk");
        let counts = c
            .evaluate(
                &same_finger,
                &AtomicBool::new(false),
                &AtomicU64::new(0),
                |_, _, next| {
                    if next == same_finger_action {
                        -100.0
                    } else {
                        0.0
                    }
                },
            )
            .unwrap();
        assert_eq!(counts.action_presses, 0.0);
        assert_eq!(
            counts.tables[2].get(
                &[
                    key(&same_finger, "a"),
                    key(&same_finger, "b"),
                    key(&same_finger, "a"),
                ][..]
            ),
            Some(&1.0)
        );
    }

    #[test]
    fn adjacent_skip_action_sfb_uses_local_effort_instead_of_forced_priority() {
        let layout = ak::Layout::parse(
            "q w @sk r t | y u i o p\na s d f g | h j k l ;\nz x c v b | n m , . /\nthumbs: space\naction sk = skip-magic\nfallback sk = repeat-previous-output\n",
            Path::new("adjacent-skip-action.dat"),
        )
        .unwrap();
        let corpus = corpus(b"aca", 3);
        let a = key(&layout, "a");
        let c = key(&layout, "c");
        let action = action_key(&layout, "sk");
        assert_eq!(layout.slots[c].finger, layout.slots[action].finger);
        assert_ne!(layout.slots[a].finger, layout.slots[action].finger);

        let mut weights = Weights::new([0.0; N_WEIGHTS]);
        weights.0[SFB] = 12.0;
        let effort = crate::action_ui::LocalEffort::new(&layout, &weights);
        let literal = corpus
            .evaluate(
                &layout,
                &AtomicBool::new(false),
                &AtomicU64::new(0),
                |a, b, key| effort.get(a, b, key),
            )
            .unwrap();
        assert_eq!(literal.tables[2].get(&[a, c, a][..]), Some(&1.0));
        assert_eq!(literal.action_presses, 0.0);
        let mut reference = ak::WindowMapper::new(&layout, 3).unwrap();
        assert_eq!(
            reference
                .map(b"aca", &|a, b, key| effort.get(a, b, key))
                .unwrap()[2],
            Some(a)
        );

        weights.0[SFB] = 0.0;
        weights.0[SKS] = 12.0;
        let effort = crate::action_ui::LocalEffort::new(&layout, &weights);
        let action_wins = corpus
            .evaluate(
                &layout,
                &AtomicBool::new(false),
                &AtomicU64::new(0),
                |a, b, key| effort.get(a, b, key),
            )
            .unwrap();
        assert_eq!(action_wins.tables[2].get(&[a, c, action][..]), Some(&1.0));
        assert_eq!(action_wins.action_presses, 1.0);
        let mut reference = ak::WindowMapper::new(&layout, 3).unwrap();
        assert_eq!(
            reference
                .map(b"aca", &|a, b, key| effort.get(a, b, key))
                .unwrap()[2],
            Some(action)
        );
    }

    #[test]
    fn following_literal_effort_and_alternative_actions() {
        let layout = ak::Layout::parse(
            "q w e r @sk | y u i o p\na s d f g | h j k l ;\nz x c v b | n m , . /\nthumbs: space\naction sk = skip-magic\nfallback sk = repeat-previous-output\n",
            Path::new("following-skip-action.dat"),
        )
        .unwrap();
        let corpus = corpus(b"acav", 4);
        let a = key(&layout, "a");
        let v = key(&layout, "v");
        let action = action_key(&layout, "sk");
        assert_eq!(layout.slots[action].finger, layout.slots[v].finger);
        assert_ne!(layout.slots[a].finger, layout.slots[v].finger);

        let mut weights = Weights::new([0.0; N_WEIGHTS]);
        weights.0[SFB] = 12.0;
        let effort = crate::action_ui::LocalEffort::new(&layout, &weights);
        let counts = corpus
            .evaluate(
                &layout,
                &AtomicBool::new(false),
                &AtomicU64::new(0),
                |a, b, key| effort.get(a, b, key),
            )
            .unwrap();
        assert_eq!(
            counts.tables[2].get(&[key(&layout, "c"), a, v][..]),
            Some(&1.0)
        );
        let mut reference = ak::WindowMapper::new(&layout, 4).unwrap();
        assert_eq!(
            reference
                .map(b"acav", &|a, b, key| effort.get(a, b, key))
                .unwrap()[2],
            Some(a)
        );
        let reused = reference
            .map(b"acaq", &|a, b, key| effort.get(a, b, key))
            .unwrap();
        let mut fresh = ak::WindowMapper::new(&layout, 4).unwrap();
        assert_eq!(
            &reused[..4],
            &fresh
                .map(b"acaq", &|a, b, key| effort.get(a, b, key))
                .unwrap()[..4]
        );
        assert_eq!(reused[2], Some(action));

        let mut reference = ak::WindowMapper::new(&layout, 4).unwrap();
        assert_eq!(
            reference.map(b"acav", &|_, _, _| 0.0).unwrap()[2],
            Some(action)
        );

        // Alternative actions must include the following movement too, even
        // when that movement uses different fingers. The right action has
        // cheaper incoming effort, but a full jump onto the period key.
        let layout = ak::Layout::parse(
            "@y w e r t | q u i o @Y\na s d f g | h j k l ;\nz x c v b | n m , . /\nthumbs: space\naction y = magic\nfallback y = \"y\"\nmap y \"a\" = \"#\"\naction Y = magic\nfallback Y = \"y\"\n",
            Path::new("alternative-actions.dat"),
        )
        .unwrap();
        let (left, right) = (action_key(&layout, "y"), action_key(&layout, "Y"));
        let mut weights = Weights::new([0.0; N_WEIGHTS]);
        weights.0[DAHJB] = 1.0;
        weights.0[DAFJB] = 4.0;
        let effort = crate::action_ui::LocalEffort::new(&layout, &weights);
        assert!(
            effort.get(None, Some(key(&layout, "s")), right)
                < effort.get(None, Some(key(&layout, "s")), left)
        );
        let program = crate::action_fast::Program::new(&layout, 3).unwrap();
        let state = crate::action_fast::KeyState::new(&program);
        let mut fast = crate::action_fast::Mapper::new(&program, &state);
        let mut reference = ak::WindowMapper::new(&layout, 3).unwrap();
        // Changing or removing the following key must invalidate the cached
        // choice. Explicit rules still forbid alternatives with wrong output.
        for (text, winner) in [
            (b"sy.".as_slice(), left),
            (b"syq".as_slice(), right),
            (b"sy.".as_slice(), left),
            (b"sy".as_slice(), right),
            (b"ay.".as_slice(), right),
        ] {
            let expected = ak::WindowMapper::new(&layout, 3)
                .unwrap()
                .map(text, &|a, b, key| effort.get(a, b, key))
                .unwrap();
            assert_eq!(expected[1], Some(winner));
            let mapped = reference
                .map(text, &|a, b, key| effort.get(a, b, key))
                .unwrap();
            assert_eq!(&mapped[..text.len()], &expected[..text.len()]);
            let mapped = fast.map(text, &|a, b, key| effort.get(a, b, key)).unwrap();
            assert_eq!(&mapped[..text.len()], &expected[..text.len()]);
        }
        let counts = self::corpus(b"sy.", 3)
            .evaluate(
                &layout,
                &AtomicBool::new(false),
                &AtomicU64::new(0),
                |a, b, key| effort.get(a, b, key),
            )
            .unwrap();
        assert_eq!(
            counts.tables[2].get(&[key(&layout, "s"), left, key(&layout, ".")][..]),
            Some(&1.0)
        );
    }

    #[test]
    fn ordinary_action_skip_finger_collision_uses_local_effort() {
        let layout = ak::Layout::parse(
            "@m w e r t | y u i o p\na s d f g | h j k l ;\nz x c v b | n m , . /\nthumbs: space\naction m = magic\nfallback m = repeat-output\n",
            Path::new("ordinary-action-skip-collision.dat"),
        )
        .unwrap();
        let corpus = corpus(b"abb", 3);
        let a = key(&layout, "a");
        let b = key(&layout, "b");
        let action = action_key(&layout, "m");
        assert_eq!(layout.slots[a].finger, layout.slots[action].finger);
        assert_ne!(layout.slots[b].finger, layout.slots[action].finger);

        let mut weights = Weights::new([0.0; N_WEIGHTS]);
        weights.0[SFS] = 12.0;
        let effort = crate::action_ui::LocalEffort::new(&layout, &weights);
        let literal = corpus
            .evaluate(
                &layout,
                &AtomicBool::new(false),
                &AtomicU64::new(0),
                |a, b, key| effort.get(a, b, key),
            )
            .unwrap();
        assert_eq!(literal.tables[2].get(&[a, b, b][..]), Some(&1.0));
        assert_eq!(literal.action_presses, 0.0);
        let mut reference = ak::WindowMapper::new(&layout, 3).unwrap();
        assert_eq!(
            reference
                .map(b"abb", &|a, b, key| effort.get(a, b, key))
                .unwrap()[2],
            Some(b)
        );

        weights.0[SFS] = 0.0;
        weights.0[SKB] = 12.0;
        let effort = crate::action_ui::LocalEffort::new(&layout, &weights);
        let action_wins = corpus
            .evaluate(
                &layout,
                &AtomicBool::new(false),
                &AtomicU64::new(0),
                |a, b, key| effort.get(a, b, key),
            )
            .unwrap();
        assert_eq!(action_wins.tables[2].get(&[a, b, action][..]), Some(&1.0));
        assert_eq!(action_wins.action_presses, 1.0);
        let mut reference = ak::WindowMapper::new(&layout, 3).unwrap();
        assert_eq!(
            reference
                .map(b"abb", &|a, b, key| effort.get(a, b, key))
                .unwrap()[2],
            Some(action)
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
        let duplicate_gram = r#"{"letters":{"a":1,"\u0061":2},"bigrams":{},"trigrams":{}}"#;
        assert!(
            NgramCorpus::from_text(duplicate_gram, Path::new("bad.json"))
                .unwrap_err()
                .contains("duplicate n-gram")
        );
        let duplicate_field = r#"{"letters":{},"\u006cetters":{},"bigrams":{},"trigrams":{}}"#;
        assert!(
            NgramCorpus::from_text(duplicate_field, Path::new("bad.json"))
                .unwrap_err()
                .contains("duplicate corpus field")
        );

        // Different source names can normalize to one key. Their additions
        // retain JSON order, including gaps from Unicode and control keys.
        let stop = AtomicBool::new(false);
        let progress = AtomicU64::new(0);
        for (text, gap) in [
            (
                r#"{"é":9007199254740992,"\n":1,"\t":1,"A":2,"a":3,"z":-0.0}"#,
                9007199254740992.0_f64,
            ),
            (
                r#"{"\n":1,"\t":1,"é":9007199254740992,"a":3,"A":2,"z":0}"#,
                9007199254740994.0_f64,
            ),
        ] {
            let mut parser = JsonParser { text, p: 0 };
            let mut table = TableBytes::new();
            read_ngram_table(&mut parser, 1, Some(&mut table), &stop, &progress).unwrap();
            assert_eq!(table.len(), 3);
            assert_eq!(table[&[0; 5]].to_bits(), gap.to_bits());
            assert_eq!(table[&[b'a', 0, 0, 0, 0]].to_bits(), 5.0_f64.to_bits());
            assert_eq!(table[&[b'z', 0, 0, 0, 0]].to_bits(), 0.0_f64.to_bits());
        }
        for text in [r#"{"a":-1}"#, r#"{"a":1e400}"#, r#"{"aa":1}"#] {
            let mut parser = JsonParser { text, p: 0 };
            let mut table = TableBytes::new();
            assert!(read_ngram_table(&mut parser, 1, Some(&mut table), &stop, &progress).is_err());
        }
        let mut parser = JsonParser {
            text: r#"{"a":1}"#,
            p: 0,
        };
        assert_eq!(
            read_ngram_table(
                &mut parser,
                1,
                Some(&mut TableBytes::new()),
                &AtomicBool::new(true),
                &progress,
            )
            .unwrap_err()
            .to_string(),
            "cancelled"
        );
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
    fn candidate_score_matches_report_arithmetic_for_identical_totals() {
        let mut state = 41u64;
        for trial in 0..500 {
            let mut next = || {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                ((state >> 32) % 10000) as f64 * 0.03125
            };
            let raw = Raw(std::array::from_fn(|_| next() - 10.0), None);
            let totals = if trial % 7 == 0 {
                [0.0; 4]
            } else {
                std::array::from_fn(|_| next() + 1.0)
            };
            let weights = Weights::new(std::array::from_fn(|_| next() * 0.01 - 0.5));
            let corpus = Corpus {
                name: String::new(),
                source_path: PathBuf::new(),
                include_spacegrams: true,
                grams: Vec::new(),
                by_symbol: Vec::new(),
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
                    len: text.len() as u8,
                    target: (text.len() - 1) as u8,
                    left_open: false,
                    weight: [0.0, 0.1, 1.0 / 3.0, 7.0, 1_000_000.25][index % 5],
                }
            })
            .collect();

        NgramCorpus {
            name: "inline".into(),
            path: PathBuf::from("inline.json"),
            include_spacegrams: true,
            order,
            warnings: Vec::new(),
            exact_sums: exact_sums_of(&windows),
            windows: windows.into(),
            postings: Postings::default(),
            chord_source: None,
            chord_windows: Arc::new(std::sync::OnceLock::new()),
            rule_candidates: Arc::default(),
            speed_populations: [0.0; 2],
        }
    }

    pub(super) fn assert_counts_bits(actual: &Counts, expected: &Counts) {
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
        for (a, b) in [
            (
                actual.finger_tables.as_ref().map(|tables| &tables[0]),
                expected.finger_tables.as_ref().map(|tables| &tables[0]),
            ),
            (
                actual.finger_tables.as_ref().map(|tables| &tables[1]),
                expected.finger_tables.as_ref().map(|tables| &tables[1]),
            ),
            (
                actual.finger_tables.as_ref().map(|tables| &tables[2]),
                expected.finger_tables.as_ref().map(|tables| &tables[2]),
            ),
            (actual.finger_skip.as_ref(), expected.finger_skip.as_ref()),
        ] {
            assert_eq!(a.is_some(), b.is_some());
            if let (Some(a), Some(b)) = (a, b) {
                assert_eq!(a.len(), b.len());
                for ((a_key, a_count), (b_key, b_count)) in a.iter().zip(b) {
                    assert_eq!(a_key, b_key);
                    assert_eq!(
                        a_count.to_bits(),
                        b_count.to_bits(),
                        "finger pattern {a_key:?}"
                    );
                }
            }
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
                        let finite = corpus
                            .evaluate_with_finite_policy(
                                &layout,
                                &stop,
                                &actual_progress,
                                crate::alt_fingers::AltPolicy::layout(true),
                                true,
                                &effort,
                            )
                            .unwrap();
                        assert_counts_bits(&finite, &expected);
                        assert_eq!(actual_progress.load(Ordering::Relaxed), 100);
                        assert_eq!(reference_progress.load(Ordering::Relaxed), 100);
                    }
                }
            }
        }

        // Exercise contextual fingers, combo usage and centered chord anchors
        // under every policy combination. Chord boundaries require consistent
        // left/right counts, so derive these fractional tables from one text.
        let text_tables = ak::text_ngrams(b"waa quu aaquu! aqqaa quu\0aa qquuaau ");
        let names = ["letters", "bigrams", "trigrams", "fourgrams", "fivegrams"];
        let fields: Vec<_> = text_tables
            .iter()
            .enumerate()
            .map(|(index, table)| {
                let entries: Vec<_> = table
                    .iter()
                    .map(|(gram, frequency)| {
                        format!("{}:{}", ak::quote(gram), *frequency as f64 * 0.25)
                    })
                    .collect();
                format!("\"{}\":{{{}}}", names[index], entries.join(","))
            })
            .collect();
        let corpus = NgramCorpus::from_text(
            &format!("{{{}}}", fields.join(",")),
            Path::new("combo-inline.json"),
        )
        .unwrap();
        let mut layout = ak::Layout::parse(BASE, Path::new("inline.dat")).unwrap();
        let Json::Array(combos) = crate::parse_json(
            r#"[{"keys":["q","p"],"output":"aa"},{"keys":["s","d"],"output":"qu"}]"#,
        )
        .unwrap() else {
            unreachable!()
        };
        layout.combos = combos;
        layout.alt_fingers = crate::alt_fingers::parse_json(&crate::parse_json(
            r#"[{"ngram":["a","a"],"fingers":["LP","LP"]},{"ngram":["q",null,"u"],"fingers":["LR",null,"LR"]}]"#,
        ).unwrap()).unwrap();
        let stop = AtomicBool::new(false);
        let progress = AtomicU64::new(0);
        for alt_fingers in [false, true] {
            for combos in [false, true] {
                let effort = |_: Option<usize>, _: Option<usize>, key: usize| key as f64 * 0.01;
                let generic = corpus
                    .evaluate_with_policy(
                        &layout,
                        &stop,
                        &progress,
                        crate::alt_fingers::AltPolicy::layout(alt_fingers),
                        combos,
                        &effort,
                    )
                    .unwrap();
                let reference = corpus
                    .evaluate_reference_with_policy(
                        &layout,
                        &stop,
                        &progress,
                        crate::alt_fingers::AltPolicy::layout(alt_fingers),
                        combos,
                        &effort,
                    )
                    .unwrap();
                let finite = corpus
                    .evaluate_with_finite_policy(
                        &layout,
                        &stop,
                        &progress,
                        crate::alt_fingers::AltPolicy::layout(alt_fingers),
                        combos,
                        &effort,
                    )
                    .unwrap();
                assert_counts_bits(&generic, &reference);
                assert_counts_bits(&finite, &reference);
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
        while layout.slots.len() <= DENSE_COUNT_SLOTS {
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

    fn text_tables(text: &[u8]) -> [TableBytes; 5] {
        let counts = ak::text_ngrams(text);
        std::array::from_fn(|i| {
            counts[i]
                .iter()
                .map(|(gram, count)| (gram_slice(gram), *count as f64))
                .collect()
        })
    }

    fn cached_text(text: &[u8]) -> String {
        let names = ["letters", "bigrams", "trigrams", "fourgrams", "fivegrams"];
        let tables = text_tables(text);
        let fields: Vec<_> = tables
            .iter()
            .zip(names)
            .enumerate()
            .map(|(i, (table, name))| {
                let grams: Vec<_> = table
                    .iter()
                    .map(|(gram, count)| format!("{}:{count}", ak::quote(&gram[..=i])))
                    .collect();
                format!("\"{name}\":{{{}}}", grams.join(","))
            })
            .collect();
        format!("{{{}}}", fields.join(","))
    }

    fn window_bits(corpus: &NgramCorpus) -> Vec<([u8; 5], usize, usize, bool, u64)> {
        corpus
            .windows
            .iter()
            .map(|window| {
                (
                    window.bytes,
                    window.len(),
                    window.target(),
                    window.left_open,
                    window.weight.to_bits(),
                )
            })
            .collect()
    }

    // Preserve the former Vec-key construction as an independent bit-level
    // reference for the fixed-key, shared-mass chord path.
    fn reference_chord_windows(corpus: &NgramCorpus) -> Vec<Window> {
        let context_tables = || {
            let mut tables: [BTreeMap<Vec<u8>, f64>; 5] = std::array::from_fn(|_| BTreeMap::new());
            for window in corpus.windows.iter() {
                for n in 1..=window.len() {
                    *tables[n - 1]
                        .entry(window.text()[window.len() - n..].to_vec())
                        .or_default() += window.weight;
                }
            }
            tables
        };
        let retained = corpus.chord_source.as_ref().map(|_| context_tables());
        let uncapped;
        let source = if let Some(source) = &corpus.chord_source {
            source.as_ref()
        } else {
            let tables = context_tables();
            uncapped = std::array::from_fn(|i| {
                tables[i]
                    .iter()
                    .map(|(gram, frequency)| (gram_slice(gram), *frequency))
                    .collect()
            });
            &uncapped
        };
        let left = if corpus.order >= 5 { 3 } else { 2 };
        let right = 1;
        let mut counts: BTreeMap<(usize, usize, bool, Vec<u8>), f64> = BTreeMap::new();
        for l in 0..=left {
            for r in 0..=right {
                let n = l + r + 1;
                let mut left_mass = BTreeMap::<Vec<u8>, f64>::new();
                let mut right_mass = BTreeMap::<Vec<u8>, f64>::new();
                let mut both_mass = BTreeMap::<Vec<u8>, f64>::new();
                if l < left {
                    for (gram, f) in &source[n] {
                        *left_mass.entry(gram[1..=n].to_vec()).or_default() += f;
                    }
                }
                if r < right {
                    for (gram, f) in &source[n] {
                        *right_mass.entry(gram[..n].to_vec()).or_default() += f;
                    }
                }
                if l < left && r < right {
                    for (gram, f) in &source[n + 1] {
                        *both_mass.entry(gram[1..=n].to_vec()).or_default() += f;
                    }
                }
                for (bytes, f) in &source[n - 1] {
                    let gram = &bytes[..n];
                    let weight = f
                        - left_mass.get(gram).copied().unwrap_or(0.0)
                        - right_mass.get(gram).copied().unwrap_or(0.0)
                        + both_mass.get(gram).copied().unwrap_or(0.0);
                    if weight > 0.0 {
                        counts.insert((n, l, l == left && l < 3, gram.to_vec()), weight);
                    }
                }
            }
        }
        // Capped contexts give their counts to shorter ones: first without the
        // following character, then without the earliest one.
        if let Some(tables) = &retained {
            for n in (2..=left + right + 1).rev() {
                let dropped: Vec<_> = counts
                    .iter()
                    .filter(|((len, _, _, gram), _)| *len == n && !tables[n - 1].contains_key(gram))
                    .map(|(key, weight)| (key.clone(), *weight))
                    .collect();
                for ((len, target, left_open, gram), weight) in dropped {
                    counts.remove(&(len, target, left_open, gram.clone()));
                    let shorter = if target + 1 < len {
                        (len - 1, target, left_open, gram[..len - 1].to_vec())
                    } else {
                        (len - 1, target - 1, true, gram[1..].to_vec())
                    };
                    *counts.entry(shorter).or_default() += weight;
                }
            }
        }
        counts
            .into_iter()
            .map(|((len, target, left_open, gram), weight)| Window {
                bytes: gram_slice(&gram),
                len: len as u8,
                target: target as u8,
                left_open,
                weight,
            })
            .collect()
    }

    fn assert_chord_reference(corpus: &NgramCorpus) {
        let expected = reference_chord_windows(corpus);
        let actual = corpus.chord_contexts().unwrap();
        assert_eq!(
            window_bits(&actual),
            window_bits(&NgramCorpus {
                windows: expected.into(),
                ..corpus.clone()
            })
        );
    }

    #[test]
    fn unlimited_and_unreached_limits_preserve_context_bits_and_order() {
        let text = cached_text(b"abcabcabc\nxbcxbc\nybc\n");
        let original = NgramCorpus::from_text(&text, Path::new("inline.json")).unwrap();
        assert_chord_reference(&original);
        for limits in [
            NgramLimits::default(),
            NgramLimits {
                trigrams: Some(2000),
                tetragrams: Some(2000),
                pentagrams: Some(2000),
                include_spacegrams: true,
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
    fn disabling_spacegrams_removes_literal_space_contexts() {
        let text = cached_text(b"ab cd");
        let corpus = NgramCorpus::from_text_with_limits(
            &text,
            Path::new("spacegrams.json"),
            NgramLimits {
                include_spacegrams: false,
                ..NgramLimits::default()
            },
        )
        .unwrap();
        assert!(corpus
            .windows
            .iter()
            .all(|window| !window.text().contains(&b' ')));
        assert_eq!(corpus.path, Path::new("spacegrams.json"));
        assert!(!corpus.include_spacegrams);
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
                    assert!(tables[i - 1].contains_key(&gram_slice(&gram[1..=i])));
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
            .filter(|window| window.len() >= 3)
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
            include_spacegrams: true,
        };
        let full = NgramCorpus::from_text(&text, Path::new("inline.json")).unwrap();
        let limited =
            NgramCorpus::from_text_with_limits(&text, Path::new("inline.json"), limits).unwrap();
        assert_chord_reference(&limited);
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
