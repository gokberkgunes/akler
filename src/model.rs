type AppResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const CORPUS_DIR: &str = "corpora";

const LAYOUT_DIR: &str = "layouts";

const WEIGHTS_FILE: &str = "optimizer-weights.conf";

const SEARCH_FILE: &str = "optimizer-search.conf";

const MODEL_VERSION: &str = "ortholinear-custom-14-jump-label-case";

const DEFAULT_CORPUS: &str = "corpus-reddit.json";

// Pinky, ring, middle, index: retained from the last agreed implementation.
// This is a modeling assumption, not a universal anatomical measurement.
const LENGTH_RANK: [i8; 4] = [0, 1, 3, 2];

const N_METRICS: usize = 44;

const N_WEIGHTS: usize = N_METRICS + 8;

const USAGE_WEIGHT: usize = N_METRICS + 4;

const SFB: usize = 0;

const SFS: usize = 1;

const SKB: usize = 2;

const SKS: usize = 3;

const FSB: usize = 4;

const HSB: usize = 5;

const FSS: usize = 6;

const HSS: usize = 7;

const LSB: usize = 8;

const LSS: usize = 9;

const DNFJB: usize = 10;

const DNFJS: usize = 11;

const REDIR: usize = 12;

const WRED: usize = 13;

const WISH: usize = 14;

const SRAF: usize = 15;

const ROLL: usize = 16;

const DAFJB: usize = 17;

const CAFJB: usize = 18;

const DAFJS: usize = 19;

const CAFJS: usize = 20;

const ALT: usize = 21;

const CNFJB: usize = 22;

const CNFJS: usize = 23;

const TRAVEL: usize = 24;

const VTRAVEL: usize = 25;

const LTRAVEL: usize = 26;

const SFTRAVEL: usize = 27;

// Directional roll metrics carry the reward; ROLL is their display total.
const IN2: usize = 28;

const OUT2: usize = 29;

const IN3: usize = 30;

const OUT3: usize = 31;

const INROLL: usize = 32;

const OUTROLL: usize = 33;

const INSRAF: usize = 34;

const OUTSRAF: usize = 35;

const DAHJB: usize = 36;

const CAHJB: usize = 37;

const DAHJS: usize = 38;

const CAHJS: usize = 39;

const DNHJB: usize = 40;

const CNHJB: usize = 41;

const DNHJS: usize = 42;

const CNHJS: usize = 43;

const USAGE: usize = N_METRICS;

const OFF: usize = USAGE + 10;

const SRAF_DEN: usize = OFF + 8;

const RHYTHM_DEN: usize = SRAF_DEN + 1;

const RAW_SRAF: usize = RHYTHM_DEN + 1;

const RAW_ALT: usize = RAW_SRAF + 1;

const ROLL_DEN: usize = RAW_ALT + 1;

const N_RAW: usize = ROLL_DEN + 1;

const METRIC_NAMES: [&str; N_METRICS] = [
    "SFB", "SFS", "SKB", "SKS", "FSB", "HSB", "FSS", "HSS", "LSB", "LSS", "dnFJB", "dnFJS", "RED",
    "WRED", "WISH", "SRAF", "ROLL", "daFJB", "caFJB", "daFJS", "caFJS", "ALT", "cnFJB", "cnFJS",
    "TRAVEL", "VTRAVEL", "LTRAVEL", "SFTRAVEL", "IN2", "OUT2", "IN3", "OUT3", "INROLL", "OUTROLL",
    "INSRAF", "OUTSRAF", "daHJB", "caHJB", "daHJS", "caHJS", "dnHJB", "cnHJB", "dnHJS", "cnHJS",
];

// Adjacent discordant jump subsets and combined rhythm totals are display-only.
const WEIGHT_NAMES: [&str; N_WEIGHTS] = [
    "sfb",
    "sfs",
    "skb",
    "sks",
    "fsb",
    "hsb",
    "fss",
    "hss",
    "lsb",
    "lss",
    "dnfjb",
    "dnfjs",
    "red",
    "wred",
    "wish",
    "sraf_reward",
    "roll_reward",
    "dafjb",
    "cafjb",
    "dafjs",
    "cafjs",
    "alt_reward",
    "cnfjb",
    "cnfjs",
    "travel",
    "vtravel",
    "ltravel",
    "sftravel",
    "in2",
    "out2",
    "in3",
    "out3",
    "inroll",
    "outroll",
    "insraf_reward",
    "outsraf_reward",
    "dahjb",
    "cahjb",
    "dahjs",
    "cahjs",
    "dnhjb",
    "cnhjb",
    "dnhjs",
    "cnhjs",
    "off_pinky",
    "off_ring",
    "off_middle",
    "off_index",
    "usage_pinky",
    "usage_ring",
    "usage_middle",
    "usage_index",
];

fn weight_display_name(i: usize) -> &'static str {
    if matches!(
        i,
        DAFJB
            | CAFJB
            | DAFJS
            | CAFJS
            | DNFJB
            | CNFJB
            | DNFJS
            | CNFJS
            | DAHJB
            | CAHJB
            | DAHJS
            | CAHJS
            | DNHJB
            | CNHJB
            | DNHJS
            | CNHJS
    ) {
        METRIC_NAMES[i]
    } else {
        WEIGHT_NAMES[i]
    }
}

const FINGER_NAMES: [&str; 10] = ["LP", "LR", "LM", "LI", "RI", "RM", "RR", "RP", "LT", "RT"];

const METRIC_HELP: [&str; N_METRICS] = [
    "Different keys on the same physical finger. Same-key repeats excluded.",
    "Different keys on the same finger with one intervening press.",
    "Same physical key twice in a row.",
    "Same physical key at both skipgram endpoints.",
    "Adjacent-finger discordant two-row jump. Display-only alias of daFJB; not an extra penalty.",
    "Adjacent-finger discordant one-row jump. Display-only alias of daHJB; not an extra penalty.",
    "Adjacent-finger discordant two-row skipgram. Display-only alias of daFJS; not an extra penalty.",
    "Adjacent-finger discordant one-row skipgram. Display-only alias of daHJS; not an extra penalty.",
    "Same hand, different fingers; horizontal span exceeds finger separation.",
    "Lateral-stretch rule applied to skipgram endpoints.",
    "Discordant two-row change between non-adjacent fingers, disjoint from daFJB.",
    "Discordant two-row skipgram between non-adjacent fingers, disjoint from daFJS.",
    "Same-hand, no-thumb trigram whose finger direction reverses.",
    "Redirect with no index finger. Subset of RED, with an extra penalty.",
    "Redirect with an index and a ring/pinky. Subset of RED.",
    "Clean same-row adjacent-finger bigram. Display total of INSRAF + OUTSRAF; same hand, no thumbs, and no blocked movement.",
    "Display total of INROLL + OUTROLL. No repeated fingers. Thumb inclusion and movement filters follow [rolls] in akler.conf.",
    "Discordant full jump bigram between adjacent fingers.",
    "Concordant full jump bigram between adjacent fingers.",
    "Discordant full jump skipgram between adjacent fingers.",
    "Concordant full jump skipgram between adjacent fingers.",
    "Hand alternation: LRL or RLR. Repeated keys/fingers, jumps and stretches still count. No thumbs; percentage of all no-thumb trigrams.",
    "Concordant two-row change between non-adjacent fingers, disjoint from caFJB.",
    "Concordant two-row skipgram between non-adjacent fingers, disjoint from caFJS.",
    "Main-finger distance from home, key units per 100 supported presses. Not a physical trajectory.",
    "Vertical distance from home, key units per 100 supported presses.",
    "Lateral distance from home, key units per 100 supported presses.",
    "Consecutive same-finger distance, key units per 100 supported bigrams.",
    "Inward two-finger roll within a mixed-hand trigram (LLR/RRL/LRR/RLL). Uses [rolls] settings. Display-only part of ROLL.",
    "Outward two-finger roll within a mixed-hand trigram (LLR/RRL/LRR/RLL). Uses [rolls] settings. Display-only part of ROLL.",
    "Three distinct fingers on one hand moving strictly inward. Uses [rolls] settings. Display-only part of ROLL.",
    "Three distinct fingers on one hand moving strictly outward. Uses [rolls] settings. Display-only part of ROLL.",
    "All inward rolls: IN2 + IN3. Percentage of eligible trigrams under [rolls]; independently rewarded.",
    "All outward rolls: OUT2 + OUT3. Percentage of eligible trigrams under [rolls]; independently rewarded.",
    "Clean inward same-row adjacent-finger bigram. Subset of SRAF; independently rewarded.",
    "Clean outward same-row adjacent-finger bigram. Subset of SRAF; independently rewarded.",
    "Discordant one-row jump bigram between adjacent fingers.",
    "Concordant one-row jump bigram between adjacent fingers.",
    "Discordant one-row jump skipgram between adjacent fingers.",
    "Concordant one-row jump skipgram between adjacent fingers.",
    "Discordant one-row jump bigram between nonadjacent fingers.",
    "Concordant one-row jump bigram between nonadjacent fingers.",
    "Discordant one-row jump skipgram between nonadjacent fingers.",
    "Concordant one-row jump skipgram between nonadjacent fingers.",
];

fn physical_metric(m: usize) -> bool {
    matches!(m, TRAVEL | VTRAVEL | LTRAVEL | SFTRAVEL)
}

fn metric_unit(m: usize) -> &'static str {
    if physical_metric(m) {
        "u/100"
    } else {
        "%"
    }
}

fn bit(m: usize) -> u128 {
    1u128 << m
}

fn is_skip(m: usize) -> bool {
    matches!(
        m,
        SFS | SKS | FSS | HSS | LSS | DNFJS | DAFJS | CAFJS | CNFJS | DAHJS | CAHJS | DNHJS | CNHJS
    )
}

fn is_rhythm(m: usize) -> bool {
    matches!(
        m,
        REDIR | WRED | WISH | ROLL | ALT | IN2 | OUT2 | IN3 | OUT3 | INROLL | OUTROLL
    )
}

fn higher_better(m: usize) -> bool {
    matches!(
        m,
        SRAF | ROLL | ALT | IN2 | OUT2 | IN3 | OUT3 | INROLL | OUTROLL | INSRAF | OUTSRAF
    )
}

fn aggregate(m: usize) -> bool {
    matches!(
        m,
        FSB | FSS | HSB | HSS | SRAF | ROLL | IN2 | OUT2 | IN3 | OUT3
    )
}

fn raw_positive(m: usize) -> Option<usize> {
    match m {
        SRAF => Some(RAW_SRAF),
        _ => None,
    }
}

fn pct(n: f64, d: f64) -> f64 {
    if d > 0.0 {
        100.0 * n / d
    } else {
        0.0
    }
}

fn blank(b: u8) -> bool {
    b < 32
}

fn display_symbol(b: u8) -> char {
    if blank(b) {
        '·'
    } else if b == b' ' {
        '␣'
    } else {
        b as char
    }
}

fn clean_text(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

fn short(s: &str, width: usize) -> String {
    clean_text(s).chars().take(width).collect()
}

fn label(path: &Path, prefix: &str) -> String {
    let s = path.file_stem().and_then(|s| s.to_str()).unwrap_or("?");
    clean_text(s.strip_prefix(prefix).unwrap_or(s))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Key {
    row: i8,
    col: i8,
    row_offset: i16,
    column_offset: i16,
    finger: usize,
    rank: i8,
    hand: i8,
    main: bool,
}

impl Key {
    fn horizontal_delta(self, other: Self) -> f64 {
        (self.col - other.col) as f64
            + (i32::from(self.row_offset) - i32::from(other.row_offset)) as f64 / 1000.0
    }

    fn vertical_delta(self, other: Self) -> f64 {
        (self.row - other.row) as f64
            + (i32::from(self.column_offset) - i32::from(other.column_offset)) as f64 / 1000.0
    }

    fn length(self) -> i8 {
        if self.main {
            LENGTH_RANK[self.rank as usize]
        } else {
            -1
        }
    }

    fn index(self) -> bool {
        self.main && self.rank == 3
    }

    fn weak(self) -> bool {
        self.main && self.rank <= 1
    }

    #[cfg(test)]
    fn home(self) -> bool {
        self.main && self.row == 1 && matches!(self.col, 0 | 1 | 2 | 3 | 6 | 7 | 8 | 9)
    }
}

fn main_key_at(row: usize, col: i8) -> Key {
    let (finger, rank, hand) = match col {
        -1 | 0 => (0, 0, 0),
        1 => (1, 1, 0),
        2 => (2, 2, 0),
        3 | 4 => (3, 3, 0),
        5 | 6 => (4, 3, 1),
        7 => (5, 2, 1),
        8 => (6, 1, 1),
        9 | 10 | 11 => (7, 0, 1),
        _ => unreachable!(),
    };
    Key {
        row: row as i8,
        col,
        row_offset: 0,
        column_offset: 0,
        finger,
        rank,
        hand,
        main: true,
    }
}

fn main_key(row: usize, col: usize) -> Key {
    main_key_at(row, col as i8)
}

fn thumb_key(slot: usize) -> Key {
    Key {
        row: 3,
        col: slot as i8,
        row_offset: 0,
        column_offset: 0,
        finger: 8 + slot,
        rank: -1,
        hand: slot as i8,
        main: false,
    }
}

fn same_hand(a: Key, b: Key) -> bool {
    a.main && b.main && a.hand == b.hand
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowMotion {
    None,
    Discordant,
    Concordant,
}

fn row_motion(a: Key, b: Key) -> RowMotion {
    if !same_hand(a, b) || a.finger == b.finger || a.row == b.row {
        return RowMotion::None;
    }
    let (upper, lower) = if a.row < b.row { (a, b) } else { (b, a) };
    if upper.length() < lower.length() {
        RowMotion::Discordant
    } else if upper.length() > lower.length() {
        RowMotion::Concordant
    } else {
        RowMotion::None
    }
}

fn scissor_kind(a: Key, b: Key) -> u8 {
    if !same_hand(a, b) || (a.rank - b.rank).abs() != 1 {
        return 0;
    }
    match (a.row - b.row).abs() {
        2 if row_motion(a, b) == RowMotion::Discordant => 2,
        1 if row_motion(a, b) == RowMotion::Discordant => 1,
        _ => 0,
    }
}

fn is_lateral_stretch(a: Key, b: Key) -> bool {
    let df = (a.rank - b.rank).abs();
    same_hand(a, b) && df >= 1 && a.horizontal_delta(b).abs() >= (df + 1) as f64
}

fn is_diagonal_stretch(a: Key, b: Key) -> bool {
    (a.row - b.row).abs() == 2
        && (a.rank - b.rank).abs() != 1
        && row_motion(a, b) == RowMotion::Discordant
}

fn is_concordant_jump(a: Key, b: Key) -> bool {
    (a.row - b.row).abs() == 2
        && (a.rank - b.rank).abs() != 1
        && row_motion(a, b) == RowMotion::Concordant
}

fn sraf_shape(a: Key, b: Key) -> bool {
    same_hand(a, b) && a.row == b.row && (a.rank - b.rank).abs() == 1
}

fn is_redirect(a: Key, b: Key, c: Key) -> bool {
    if !(same_hand(a, b) && same_hand(b, c)) {
        return false;
    }
    let (d1, d2) = (b.rank - a.rank, c.rank - b.rank);
    d1 != 0 && d2 != 0 && d1.signum() != d2.signum()
}

/// Classification policy compiled into `Geometry`: `[rolls]` and `[alts]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RollSettings {
    include_thumbs: bool,
    include_scissors: bool,
    include_stretches: bool,
    /// `[alts]` position-based alternate fingers; see `alt_fingers::AltRules`.
    alts: Option<&'static crate::alt_fingers::AltRules>,
}

impl Default for RollSettings {
    fn default() -> Self {
        Self {
            include_thumbs: false,
            include_scissors: true,
            include_stretches: true,
            alts: None,
        }
    }
}

fn roll_settings_label(rolls: RollSettings) -> String {
    let state = |include| if include { "on" } else { "off" };
    format!(
        "Rolls: thumbs {} · scissors {} · stretches {}",
        state(rolls.include_thumbs),
        state(rolls.include_scissors),
        state(rolls.include_stretches),
    )
}

fn roll_metric(metric: usize) -> bool {
    matches!(metric, ROLL | INROLL | OUTROLL | IN2 | OUT2 | IN3 | OUT3)
}

// Rank increases from pinky toward index on either hand. Basic rolls depend
// only on physical fingers and hands, never on row or distance penalties.
#[cfg(test)]
fn roll_kind(a: Key, b: Key, c: Key) -> Option<usize> {
    roll_kind_with_settings(a, b, c, RollSettings::default())
}

fn roll_kind_with_settings(a: Key, b: Key, c: Key, settings: RollSettings) -> Option<usize> {
    if (!settings.include_thumbs && !(a.main && b.main && c.main))
        || a.finger == b.finger
        || b.finger == c.finger
        || a.finger == c.finger
    {
        return None;
    }

    // Thumb rank is -1 elsewhere; for roll direction it is inward of index.
    let rank = |key: Key| if key.main { key.rank } else { 4 };
    if a.hand != c.hand {
        let (from, to) = if a.hand == b.hand { (a, b) } else { (b, c) };
        return Some(if rank(from) < rank(to) { IN2 } else { OUT2 });
    }

    if a.hand == b.hand {
        if rank(a) < rank(b) && rank(b) < rank(c) {
            return Some(IN3);
        }
        if rank(a) > rank(b) && rank(b) > rank(c) {
            return Some(OUT3);
        }
    }

    None
}

fn alternation_shape(a: Key, b: Key, c: Key) -> bool {
    a.main && b.main && c.main && a.hand == c.hand && a.hand != b.hand
}

#[derive(Clone, Copy, Debug, Default)]
struct PairFlags {
    bi: u128,
    sk: u128,
    main: bool,
}

// Local movement events veto SRAF credit. ALT uses hand order alone.
// Finger load is an aggregate penalty, not a local movement veto.
const BAD_BI: u128 = (1 << SFB)
    | (1 << SKB)
    | (1 << FSB)
    | (1 << DAFJB)
    | (1 << CAFJB)
    | (1 << HSB)
    | (1 << DAHJB)
    | (1 << CAHJB)
    | (1 << LSB)
    | (1 << DNFJB)
    | (1 << CNFJB)
    | (1 << DNHJB)
    | (1 << CNHJB);

fn pair_flags(a: Key, b: Key, same_key: bool) -> PairFlags {
    let mut f = PairFlags {
        main: a.main && b.main,
        ..PairFlags::default()
    };
    if same_key {
        f.bi |= bit(SKB);
        f.sk |= bit(SKS);
    } else if a.finger == b.finger {
        f.bi |= bit(SFB);
        f.sk |= bit(SFS);
    }
    match scissor_kind(a, b) {
        1 => {
            f.bi |= bit(HSB);
            f.sk |= bit(HSS);
        }
        2 => {
            f.bi |= bit(FSB);
            f.sk |= bit(FSS);
        }
        _ => {}
    }
    match ((a.row - b.row).abs(), row_motion(a, b)) {
        (2, RowMotion::Discordant) if (a.rank - b.rank).abs() == 1 => {
            f.bi |= bit(DAFJB);
            f.sk |= bit(DAFJS);
        }
        (2, RowMotion::Concordant) if (a.rank - b.rank).abs() == 1 => {
            f.bi |= bit(CAFJB);
            f.sk |= bit(CAFJS);
        }
        (1, RowMotion::Discordant) if (a.rank - b.rank).abs() == 1 => {
            f.bi |= bit(DAHJB);
            f.sk |= bit(DAHJS);
        }
        (1, RowMotion::Concordant) if (a.rank - b.rank).abs() == 1 => {
            f.bi |= bit(CAHJB);
            f.sk |= bit(CAHJS);
        }
        (1, RowMotion::Discordant) => {
            f.bi |= bit(DNHJB);
            f.sk |= bit(DNHJS);
        }
        (1, RowMotion::Concordant) => {
            f.bi |= bit(CNHJB);
            f.sk |= bit(CNHJS);
        }
        _ => {}
    }
    if is_lateral_stretch(a, b) {
        f.bi |= bit(LSB);
        f.sk |= bit(LSS);
    }
    if is_diagonal_stretch(a, b) {
        f.bi |= bit(DNFJB);
        f.sk |= bit(DNFJS);
    }
    if is_concordant_jump(a, b) {
        f.bi |= bit(CNFJB);
        f.sk |= bit(CNFJS);
    }
    if sraf_shape(a, b) {
        f.bi |= bit(RAW_SRAF);
        if f.bi & BAD_BI == 0 {
            f.bi |= bit(SRAF) | bit(if a.rank < b.rank { INSRAF } else { OUTSRAF });
        }
    }
    f
}

#[derive(Clone, Copy, Debug, Default)]
struct TriFlags {
    bits: u128,
    main: bool,
}

const _: () = assert!(N_RAW <= 128);

/// Raw counters a trigram adds to: its flags plus `RHYTHM_DEN` for an
/// all-main-key trigram.
fn tri_raw_bits(flags: TriFlags) -> u128 {
    flags.bits | if flags.main { bit(RHYTHM_DEN) } else { 0 }
}

/// The cubic trigram table stores 4-byte masks over only the counters some
/// trigram on this board sets, so it stays cache-resident during search.
#[derive(Clone)]
struct TriTable {
    masks: Vec<u32>,
    /// Raw counter of each compact bit.
    counters: Vec<usize>,
}

impl TriTable {
    fn new(raw: &[u128]) -> Self {
        let used = raw.iter().fold(0, |all, bits| all | bits);
        let counters: Vec<usize> = (0..128).filter(|&bit| used >> bit & 1 != 0).collect();
        assert!(counters.len() <= 32, "trigram flags exceed the compact table");
        let masks = raw
            .iter()
            .map(|&bits| {
                counters
                    .iter()
                    .enumerate()
                    .filter(|(_, &counter)| bits >> counter & 1 != 0)
                    .fold(0, |mask, (compact, _)| mask | 1 << compact)
            })
            .collect();
        Self { masks, counters }
    }

    /// The raw counter bits of entry `index`.
    fn raw(&self, index: usize) -> u128 {
        let mut mask = self.masks[index];
        let mut bits = 0;
        while mask != 0 {
            bits |= 1 << self.counters[mask.trailing_zeros() as usize];
            mask &= mask - 1;
        }
        bits
    }

    /// Move `f` from entry `from`'s counters to entry `to`'s; counters set in
    /// both are untouched.
    #[inline(always)]
    fn move_counters(&self, raw: &mut Raw, from: usize, to: usize, f: f64) {
        let signed = [-f, f];
        let to = self.masks[to];
        let mut changed = self.masks[from] ^ to;
        while changed != 0 {
            let bit = changed.trailing_zeros() as usize;
            raw.0[self.counters[bit]] += signed[(to >> bit) as usize & 1];
            changed &= changed - 1;
        }
    }
}

fn trigram_penalties(a: Key, b: Key, c: Key) -> u128 {
    if !(a.main && b.main && c.main) {
        return 0;
    }
    let red = is_redirect(a, b, c);
    let weak = red && !(a.index() || b.index() || c.index());
    let wish = red && !weak && (a.weak() || b.weak() || c.weak());
    let mut bits = 0;
    if red {
        bits |= bit(REDIR);
    }
    if weak {
        bits |= bit(WRED);
    }
    if wish {
        bits |= bit(WISH);
    }
    bits
}

#[cfg(test)]
fn tri_flags(a: Key, b: Key, c: Key) -> TriFlags {
    tri_flags_with_settings(a, b, c, RollSettings::default())
}

fn tri_flags_with_settings(a: Key, b: Key, c: Key, settings: RollSettings) -> TriFlags {
    let main = a.main && b.main && c.main;
    let mut bits = if settings.include_thumbs || main {
        bit(ROLL_DEN)
    } else {
        0
    };
    let ab = pair_flags(a, b, a.row == b.row && a.col == b.col);
    let bc = pair_flags(b, c, b.row == c.row && b.col == c.col);

    if let Some(kind) = roll_kind_with_settings(a, b, c, settings) {
        // Filter consecutive pairs only. FSB and HSB identify adjacent
        // discordant jumps. Skip AC is not a veto.
        let pairs = ab.bi | bc.bi;
        let blocked_scissors = !settings.include_scissors && pairs & (bit(FSB) | bit(HSB)) != 0;
        let blocked_stretches = !settings.include_stretches && pairs & bit(LSB) != 0;
        if !blocked_scissors && !blocked_stretches {
            let direction = if matches!(kind, IN2 | IN3) {
                INROLL
            } else {
                OUTROLL
            };
            bits |= bit(kind) | bit(direction) | bit(ROLL);
        }
    }

    // Thumb inclusion is specific to rolls. Other rhythm metrics, including
    // ALT, retain their non-thumb population and denominator.
    if !main {
        return TriFlags { bits, main };
    }
    bits |= trigram_penalties(a, b, c);
    if alternation_shape(a, b, c) {
        bits |= bit(ALT) | bit(RAW_ALT);
    }

    TriFlags { bits, main }
}

#[cfg(test)]
fn is_sraf(a: Key, b: Key) -> bool {
    pair_flags(a, b, false).bi & bit(SRAF) != 0
}

#[cfg(test)]
fn is_roll(a: Key, b: Key, c: Key) -> bool {
    tri_flags(a, b, c).bits & bit(ROLL) != 0
}

#[cfg(test)]
fn is_alternation(a: Key, b: Key, c: Key) -> bool {
    tri_flags(a, b, c).bits & bit(ALT) != 0
}

#[cfg(test)]
fn home_travel(k: Key) -> [f64; 3] {
    if !k.main {
        return [0.0; 3];
    }
    let homes = [0, 1, 2, 3, 6, 7, 8, 9];
    let y = (k.row - 1).abs() as f64;
    let x = ((k.col - homes[k.finger]) as f64 + k.row_offset as f64 / 1000.0).abs();
    [x.hypot(y), y, x]
}

const HOME_COLUMNS: [i8; 8] = [0, 1, 2, 3, 6, 7, 8, 9];

#[derive(Clone, Copy)]
struct HomeAnchors {
    slots: [Option<Key>; 8],
    columns: [i8; 8],
}

impl HomeAnchors {
    fn new(keys: &[Key]) -> Self {
        let shift: [i8; 2] = std::array::from_fn(|hand| {
            let mut counts = BTreeMap::new();
            let mut seen = BTreeSet::new();
            for key in keys
                .iter()
                .filter(|key| key.main && key.row == 1 && key.hand == hand as i8)
            {
                if seen.insert((key.finger, key.col)) {
                    *counts
                        .entry(key.col - HOME_COLUMNS[key.finger])
                        .or_insert(0usize) += 1;
                }
            }
            let mut best = (0i8, 0usize);
            for (candidate, count) in counts {
                if count > best.1
                    || count == best.1 && (candidate.abs(), candidate) < (best.0.abs(), best.0)
                {
                    best = (candidate, count);
                }
            }
            best.0
        });
        let columns =
            std::array::from_fn(|finger| HOME_COLUMNS[finger] + shift[usize::from(finger >= 4)]);
        let slots = std::array::from_fn(|finger| {
            let target = columns[finger];
            keys.iter()
                .copied()
                .filter(|key| key.main && key.row == 1 && key.finger == finger)
                .min_by_key(|key| ((key.col - target).abs(), key.col))
        });
        Self { slots, columns }
    }

    fn anchor(self, k: Key, keys: &[Key]) -> Key {
        if let Some(home) = self.slots[k.finger] {
            return home;
        }
        let column = self.columns[k.finger];
        let row_offset = keys
            .iter()
            .find(|key| key.main && key.row == 1)
            .map_or(0, |key| key.row_offset);
        let column_offset = keys
            .iter()
            .find(|key| key.main && key.row == 1 && key.col == column)
            .map_or(0, |key| key.column_offset);
        Key {
            row: 1,
            col: column,
            row_offset,
            column_offset,
            ..k
        }
    }

    fn contains(self, k: Key) -> bool {
        k.main && self.slots[k.finger].is_some_and(|home| home.row == k.row && home.col == k.col)
    }
}

fn home_slot_mask(keys: &[Key]) -> Vec<bool> {
    let anchors = HomeAnchors::new(keys);
    keys.iter()
        .copied()
        .map(|key| anchors.contains(key))
        .collect()
}

fn home_travel_for(k: Key, keys: &[Key]) -> [f64; 3] {
    if !k.main {
        return [0.0; 3];
    }
    let home = HomeAnchors::new(keys).anchor(k, keys);
    let x = k.horizontal_delta(home).abs();
    let y = k.vertical_delta(home).abs();
    [x.hypot(y), y, x]
}

#[derive(Clone)]
struct Geometry {
    rolls: RollSettings,
    keys: Vec<Key>,
    pair: Vec<PairFlags>,
    tri: TriTable,
    n: usize,
    /// Each key's distance from home (total, vertical, lateral).
    home: Vec<[f64; 3]>,
    is_home: Vec<bool>,
    sf_distance: Vec<f64>,
    /// `[alts]` rules for these keys.
    alts: crate::alt_fingers::Resolver,
    /// Mana2 classifications for these keys, built on first use and shared
    /// by clones. Only searches that maintain Mana2 counters need them.
    mana2: Arc<std::sync::OnceLock<mana2_metrics::GeometryContributions>>,
}

impl Geometry {
    fn new(keys: Vec<Key>) -> Self {
        Self::with_rolls(keys, RollSettings::default())
    }

    /// Tables for action evaluation, which applies `[alts]` rules per context
    /// window instead (see `alt_fingers::AltPolicy`).
    fn with_rolls(keys: Vec<Key>, rolls: RollSettings) -> Self {
        Self::build(keys, rolls, crate::alt_fingers::Resolver::none())
    }

    /// Tables for the ordinary model, with `[alts]` rules applied within each
    /// pair and triple: ordinary grams carry no context beyond themselves.
    fn with_alts(keys: Vec<Key>, rolls: RollSettings) -> Self {
        let alts = rolls
            .alts
            .map_or_else(crate::alt_fingers::Resolver::none, |rules| rules.resolver(&keys));
        Self::build(keys, rolls, alts)
    }

    fn build(keys: Vec<Key>, rolls: RollSettings, alts: crate::alt_fingers::Resolver) -> Self {
        let n = keys.len();
        let mut pair = vec![PairFlags::default(); n * n];
        let mut tri = vec![0; n * n * n];
        let mut sf_distance = vec![0.0; n * n];
        for a in 0..n {
            for b in 0..n {
                let [x, y] = alts.keys(&keys, [a, b]);
                pair[a * n + b] = pair_flags(x, y, a == b);
                if x.main && y.main && x.finger == y.finger {
                    sf_distance[a * n + b] = x.vertical_delta(y).hypot(x.horizontal_delta(y));
                }
                for c in 0..n {
                    let [x, y, z] = alts.keys(&keys, [a, b, c]);
                    tri[(a * n + b) * n + c] =
                        tri_raw_bits(tri_flags_with_settings(x, y, z, rolls));
                }
            }
        }
        let tri = TriTable::new(&tri);
        let anchors = HomeAnchors::new(&keys);
        let home = keys
            .iter()
            .copied()
            .map(|key| {
                if !key.main {
                    return [0.0; 3];
                }
                let anchor = anchors.anchor(key, &keys);
                let x = key.horizontal_delta(anchor).abs();
                let y = key.vertical_delta(anchor).abs();
                [x.hypot(y), y, x]
            })
            .collect();
        let is_home = keys
            .iter()
            .copied()
            .map(|key| anchors.contains(key))
            .collect();
        Self {
            rolls,
            keys,
            pair,
            tri,
            n,
            home,
            is_home,
            sf_distance,
            alts,
            mana2: Arc::new(std::sync::OnceLock::new()),
        }
    }

    fn mana2(&self) -> &mana2_metrics::GeometryContributions {
        self.mana2
            .get_or_init(|| mana2_metrics::GeometryContributions::new(&self.keys, &self.alts))
    }
}

#[derive(Clone, Debug)]
struct Board {
    name: String,
    path: PathBuf,
    symbols: Vec<u8>,
    keys: Vec<Key>,
    row_stagger: crate::action_keys::RowStagger,
}

fn parse_token(token: &str, empty: &mut u8) -> AppResult<u8> {
    if let Some(character) = token.strip_prefix("char:") {
        if character.len() == 1 && character.as_bytes()[0].is_ascii_graphic() {
            return Ok(character.as_bytes()[0]);
        }
        return Err("char: requires one printable ASCII character".into());
    }
    if token.eq_ignore_ascii_case("blank") || matches!(token, "~" | "·") {
        *empty += 1;
        if *empty >= 32 {
            return Err("too many blank slots".into());
        }
        return Ok(*empty);
    }
    if token.eq_ignore_ascii_case("space") || (token == "␠" || token == "␣") {
        return Ok(b' ');
    }
    let bytes = token.as_bytes();
    if bytes.len() != 1 || !bytes[0].is_ascii_graphic() {
        return Err(
            format!("key {token:?}: expected one printable ASCII character, ~, or space").into(),
        );
    }
    Ok(bytes[0].to_ascii_lowercase())
}

fn parse_row(line: &str, row: usize, empty: &mut u8) -> AppResult<(Vec<(u8, i8)>, (usize, usize))> {
    let tokens: Vec<_> = line.split_whitespace().collect();
    // Legacy 20-character rows with two spaces between hands. Do not trim
    // leading whitespace: it encodes an empty leftmost physical slot.
    const SLOTS: [usize; 10] = [0, 2, 4, 6, 8, 11, 13, 15, 17, 19];
    let bytes = line.as_bytes();
    if tokens.len() < 10
        && !line.contains('|')
        && bytes.len() <= 20
        && bytes.iter().all(|b| b.is_ascii())
        && bytes
            .iter()
            .enumerate()
            .all(|(i, b)| SLOTS.contains(&i) || *b == b' ')
    {
        let mut result = Vec::new();
        for (col, &p) in SLOTS.iter().enumerate() {
            let b = bytes.get(p).copied().unwrap_or(b' ');
            result.push((
                if b == b' ' {
                    parse_token("~", empty)?
                } else {
                    parse_token(&(b as char).to_string(), empty)?
                },
                col as i8,
            ));
        }
        return Ok((result, (5, 5)));
    }
    let separator = tokens.iter().position(|t| *t == "|");
    let (left, right) = if let Some(i) = separator {
        if tokens.iter().filter(|t| **t == "|").count() != 1 {
            return Err(format!("row {} has more than one hand separator", row + 1).into());
        }
        (&tokens[..i], &tokens[i + 1..])
    } else {
        let n = match tokens.len() {
            10 => 5,
            12 => 6,
            _ => {
                return Err(format!(
                    "row {}: use 10 or 12 slots, or separate an 11-slot row as 6|5 or 5|6",
                    row + 1
                )
                .into())
            }
        };
        (&tokens[..n], &tokens[n..])
    };
    if !(5..=6).contains(&left.len()) || !(5..=6).contains(&right.len()) {
        return Err(format!(
            "row {}: each hand needs five or six single-character slots",
            row + 1
        )
        .into());
    }
    let mut result = Vec::new();
    let start = if left.len() == 6 { -1 } else { 0 };
    for (i, t) in left.iter().enumerate() {
        result.push((parse_token(t, empty)?, start + i as i8));
    }
    for (i, t) in right.iter().enumerate() {
        result.push((parse_token(t, empty)?, 5 + i as i8));
    }
    Ok((result, (left.len(), right.len())))
}

fn board_from_text(text: &str, path: &Path) -> AppResult<Board> {
    if crate::layout_io::is_json_layout(text) {
        return board_from_json(text, path);
    }
    board_from_dat(text, path)
}

fn board_from_json(text: &str, path: &Path) -> AppResult<Board> {
    use crate::action_keys::{Binding, Layout};

    let layout = Layout::parse(text, path)?;
    let mut symbols = Vec::with_capacity(layout.slots.len());
    let mut keys = Vec::with_capacity(layout.slots.len());
    let mut empty = 0;
    let mut seen = BTreeSet::new();
    for slot in &layout.slots {
        let symbol = match &slot.binding {
            Binding::Empty => parse_token("~", &mut empty)?,
            Binding::Text(bytes) if bytes.len() == 1 => bytes[0],
            _ => return Err("action layout requires the magic evaluator".into()),
        };
        if !blank(symbol) && !seen.insert(symbol) {
            return Err(format!("duplicate key {:?}", symbol as char).into());
        }
        symbols.push(symbol);
        keys.push(Key {
            row: slot.row,
            col: slot.col,
            row_offset: slot.row_offset,
            column_offset: slot.column_offset,
            finger: slot.finger,
            rank: slot.rank,
            hand: slot.hand,
            main: slot.main,
        });
    }
    Ok(Board {
        name: layout.name,
        path: layout.path,
        symbols,
        keys,
        row_stagger: layout.row_stagger,
    })
}

fn board_from_dat(text: &str, path: &Path) -> AppResult<Board> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines: Vec<_> = text
        .lines()
        .map(|s| s.strip_suffix('\r').unwrap_or(s))
        .collect();
    // Ignore genuinely empty edge lines only. A line of spaces is a valid row.
    while lines.first() == Some(&"") {
        lines.remove(0);
    }
    while lines.last() == Some(&"") {
        lines.pop();
    }
    if lines.len() < 3 {
        return Err("layout needs three physical rows".into());
    }
    let mut empty = 0u8;
    let mut symbols = Vec::new();
    let mut keys = Vec::new();
    let absolute = crate::action_keys::absolute_columns(&lines[3..])?;
    let mut geometry = crate::action_keys::GeometrySettings::default();
    for r in 0..3 {
        let (row, _) = parse_row(lines[r], r, &mut empty)?;
        for (index, (ch, col)) in row.into_iter().enumerate() {
            let col = if absolute { index as i8 } else { col };
            symbols.push(ch);
            keys.push(main_key_at(r, col));
        }
    }
    let mut thumbs: [Vec<u8>; 2] = [Vec::new(), Vec::new()];
    let mut absent = [false; 2];
    let mut stagger_mode = None;
    let mut thumb_columns = None;
    for line in &lines[3..] {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("thumb-columns:") {
            if thumb_columns.is_some() {
                return Err("thumb-columns is defined twice".into());
            }
            let columns = value
                .split_whitespace()
                .map(|value| {
                    value
                        .parse::<i8>()
                        .map_err(|_| "thumb-columns needs integer columns in -128..127")
                })
                .collect::<Result<Vec<_>, _>>()?;
            thumb_columns = Some(columns);
            continue;
        }
        if trimmed.starts_with("col-layout:") || geometry.parse_line(trimmed)? {
            continue;
        }
        if let Some(mode) = crate::action_keys::parse_row_stagger(trimmed)? {
            if stagger_mode.replace(mode).is_some() {
                return Err("row-stagger is defined twice".into());
            }
            continue;
        }
        let named = trimmed
            .get(..7)
            .is_some_and(|s| s.eq_ignore_ascii_case("thumbs:"));
        let content = if named { &trimmed[7..] } else { trimmed };
        let tokens: Vec<_> = content.split_whitespace().collect();
        if named && tokens.contains(&"|") {
            if tokens.iter().filter(|&&token| token == "|").count() != 1
                || thumbs.iter().any(|group| !group.is_empty())
            {
                return Err("invalid or repeated thumb hand divider".into());
            }
            let split = tokens.iter().position(|&token| token == "|").unwrap();
            for (hand, group) in [&tokens[..split], &tokens[split + 1..]].iter().enumerate() {
                if group.len() == 1 && group[0] == "none" {
                    absent[hand] = true;
                } else {
                    if group.contains(&"none") {
                        return Err("none cannot be combined with a thumb key".into());
                    }
                    for token in *group {
                        thumbs[hand].push(parse_token(token, &mut empty)?);
                    }
                }
            }
            continue;
        }
        if tokens.is_empty() || tokens.len() > 2 {
            return Err("thumb line needs one key, or explicit left and right keys".into());
        }
        for (i, &token) in tokens.iter().enumerate() {
            // A bare, single thumb keeps its visual hand: columns 0..=10 are
            // left and columns 11 onward are right.
            let hand = if named || tokens.len() == 2 {
                i
            } else {
                usize::from(line.as_bytes().iter().take_while(|&&b| b == b' ').count() > 10)
            };
            if !thumbs[hand].is_empty() || absent[hand] {
                return Err(format!(
                    "{} thumb is defined twice",
                    if hand == 0 { "left" } else { "right" }
                )
                .into());
            }
            let token = if token.len() > 1 {
                token.strip_suffix(',').unwrap_or(token)
            } else {
                token
            };
            if token == "none" {
                absent[hand] = true;
            } else {
                thumbs[hand].push(parse_token(token, &mut empty)?);
            }
        }
    }
    if !symbols.contains(&b' ')
        && !thumbs.iter().flatten().any(|&ch| ch == b' ')
        && !absent.contains(&true)
    {
        match (thumbs[0].is_empty(), thumbs[1].is_empty()) {
            (true, true) => thumbs[0].push(b' '),
            (false, true) => thumbs[1].push(b' '),
            (true, false) => thumbs[0].push(b' '),
            _ => {}
        }
    }
    let mut thumb_col = 0usize;
    for (hand, group) in thumbs.into_iter().enumerate() {
        if hand == 1 && thumb_col == 0 {
            thumb_col = 1;
        }
        for ch in group {
            symbols.push(ch);
            let mut key = thumb_key(hand);
            key.col = i8::try_from(thumb_col).map_err(|_| "too many thumb keys")?;
            keys.push(key);
            thumb_col += 1;
        }
    }
    if let Some(columns) = thumb_columns {
        let thumbs: Vec<_> = keys.iter_mut().filter(|key| !key.main).collect();
        if columns.len() != thumbs.len() {
            return Err(
                "thumb-columns needs one column for every thumb slot, left hand then right hand"
                    .into(),
            );
        }
        for (key, col) in thumbs.into_iter().zip(columns) {
            key.col = col;
        }
    }
    if let Some(mode) = stagger_mode {
        for key in keys.iter_mut().filter(|key| key.main) {
            key.row_offset = mode.offset(key.row);
            key.finger = mode.finger(key.row, key.col, key.finger);
            if key.hand == 0 {
                key.rank = key.finger as i8;
            }
        }
    }
    let shape = std::array::from_fn(|row| {
        keys.iter()
            .filter(|key| key.main && key.row == row as i8)
            .count()
    });
    let minimum = keys
        .iter()
        .filter(|key| key.main)
        .map(|key| key.col)
        .min()
        .unwrap_or(0);
    let maximum = keys
        .iter()
        .filter(|key| key.main)
        .map(|key| key.col)
        .max()
        .unwrap_or(0);
    geometry.validate(shape, minimum, maximum)?;
    let mut columns = [0; 3];
    for key in keys.iter_mut().filter(|key| key.main) {
        let row = key.row as usize;
        if let Some(offset) = geometry.row_offset(row) {
            key.row_offset = offset;
        }
        if let Some(offset) = geometry.column_offset(key.col, minimum) {
            key.column_offset = offset;
        }
        if let Some(finger) = geometry.finger(row, columns[row]) {
            key.finger = finger;
            (key.rank, key.hand) = crate::action_keys::finger_geometry(finger);
        }
        columns[row] += 1;
    }

    let mut seen = BTreeSet::new();
    for &ch in &symbols {
        if !blank(ch) && !seen.insert(ch) {
            return Err(format!("duplicate key {:?}", ch as char).into());
        }
    }
    Ok(Board {
        name: label(path, ""),
        path: path.to_owned(),
        symbols,
        keys,
        row_stagger: stagger_mode.unwrap_or_default(),
    })
}

fn load_board(path: &Path) -> AppResult<Board> {
    let mut timing = crate::load_profile::LoadProfile::new("ordinary layout read/parse");
    let text = fs::read_to_string(path)?;
    timing.mark("Layout read");
    let result =
        board_from_text(&text, path).map_err(|e| format!("{}: {e}", path.display()).into());
    timing.mark("Layout parse");
    result
}

fn board_text(board: &Board, symbols: &[u8]) -> String {
    let token = |b| {
        if blank(b) {
            "~".to_string()
        } else if b == b' ' {
            "space".to_string()
        } else if matches!(b, b'~' | b'|' | b'@') || b.is_ascii_uppercase() {
            format!("char:{}", b as char)
        } else {
            (b as char).to_string()
        }
    };
    let mut out = String::new();
    for r in 0..3 {
        let mut row: Vec<_> = board
            .keys
            .iter()
            .enumerate()
            .filter(|(_, k)| k.main && k.row == r)
            .collect();
        row.sort_by_key(|(_, k)| k.col);
        let mut line = String::new();
        let mut first = true;
        let width = row.len();
        let absolute = board.keys.iter().any(|key| key.main && key.col == 11);
        let separator = if absolute && width == 12 { 6 } else { 5 };
        for (i, k) in row {
            if !first {
                if k.col == separator {
                    line.push_str(" | ");
                } else {
                    line.push(' ');
                }
            }
            first = false;
            line.push_str(&token(symbols[i]));
        }
        out.push_str(&line);
        out.push('\n');
    }
    out.push_str(&board.row_stagger.text());
    out.push_str(&crate::action_keys::geometry_text(
        board.keys.iter().filter(|key| key.main).map(|key| {
            (
                key.row,
                key.col,
                key.row_offset,
                key.column_offset,
                key.finger,
            )
        }),
        board.row_stagger,
    ));
    let thumb_columns: Vec<_> = board
        .keys
        .iter()
        .filter(|key| !key.main)
        .map(|key| key.col)
        .collect();
    let default_columns: Vec<_> = board
        .keys
        .iter()
        .filter(|key| !key.main)
        .enumerate()
        .map(|(index, key)| {
            if index == 0 && key.hand == 1 {
                1
            } else {
                index as i8
            }
        })
        .collect();
    if thumb_columns != default_columns {
        out.push_str(&format!(
            "thumb-columns: {}\n",
            thumb_columns
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }
    let mut thumbs: Vec<_> = board
        .keys
        .iter()
        .enumerate()
        .filter(|(_, k)| !k.main)
        .collect();
    thumbs.sort_by_key(|(_, k)| k.hand);
    let is_space = |(i, _): &(usize, &Key)| symbols[*i] == b' ';
    if thumbs.iter().filter(|(_, key)| key.hand == 0).count() > 1
        || thumbs.iter().filter(|(_, key)| key.hand == 1).count() > 1
    {
        let group = |hand| {
            let keys: Vec<_> = thumbs
                .iter()
                .filter(|(_, key)| key.hand == hand)
                .map(|(i, _)| token(symbols[*i]))
                .collect();
            if keys.is_empty() {
                "none".into()
            } else {
                keys.join(" ")
            }
        };
        out.push_str(&format!("thumbs: {} | {}\n", group(0), group(1)));
        return out;
    }
    let bare = match thumbs.as_slice() {
        [s] if s.1.hand == 1 || !is_space(s) => Some(*s),
        [a, b] if is_space(a) ^ is_space(b) => Some(if is_space(a) { *b } else { *a }),
        _ => None,
    };
    if thumbs.is_empty() || thumbs.len() == 1 && !is_space(&thumbs[0]) {
        out.push_str("thumbs:");
        for hand in 0..2 {
            out.push(' ');
            if let Some((i, _)) = thumbs.iter().find(|(_, key)| key.hand == hand) {
                out.push_str(&token(symbols[*i]));
            } else {
                out.push_str("none");
            }
        }
        out.push('\n');
    } else if let Some((i, k)) = bare {
        if k.hand == 1 {
            out.push_str("            ");
        }
        out.push_str(&token(symbols[i]));
        out.push('\n');
    } else if !thumbs.is_empty() {
        out.push_str("thumbs:");
        for (i, _) in thumbs {
            out.push(' ');
            out.push_str(&token(symbols[i]));
        }
        out.push('\n');
    }
    out
}

/// Default locks, the same for every design: keys off the main block
/// (thumbs, Space) stay where they are.
fn default_locks(board: &Board) -> Vec<bool> {
    board.keys.iter().map(|key| !key.main).collect()
}

/// The `H` preset: the default locks plus the home row.
fn home_locks(board: &Board) -> Vec<bool> {
    let homes = home_slot_mask(&board.keys);
    board
        .keys
        .iter()
        .zip(homes)
        .map(|(key, home)| home || !key.main)
        .collect()
}

fn timestamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn atomic_write(path: &Path, text: &str, backup: bool) -> AppResult<()> {
    if backup && path.exists() {
        let old = path.with_file_name(format!(
            "{}.{}.bak",
            path.file_name().unwrap_or_default().to_string_lossy(),
            timestamp()
        ));
        fs::copy(path, old)?;
    }
    let tmp = path.with_file_name(format!(".akler-{}-{}.tmp", crate::process_tag(), timestamp()));
    let result = (|| -> AppResult<()> {
        let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        if let Ok(metadata) = fs::metadata(path) {
            crate::copy_permissions(&f, metadata.permissions())?;
        }
        f.write_all(text.as_bytes())?;
        crate::sync_file(&f)?;
        fs::rename(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn board_as_action_layout(
    board: &Board,
    symbols: &[u8],
) -> crate::action_keys::Result<crate::action_keys::Layout> {
    crate::action_keys::Layout::parse_dat(&board_text(board, symbols), &board.path)
}

fn save_new_layout(board: &Board, symbols: &[u8]) -> AppResult<PathBuf> {
    let dir = board.path.parent().unwrap_or(Path::new(LAYOUT_DIR));
    let stem = board
        .path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("layout");
    let dat = board_text(board, symbols);
    let jsonc = crate::layout_export::jsonc_text(&board_as_action_layout(board, symbols)?)?;
    for n in 1..10000 {
        let path = dir.join(format!("{stem}-optimized-{n:03}.dat"));
        match crate::layout_export::save_pair(&path, &dat, &jsonc) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
    }
    Err("no unused result filename".into())
}

fn discover(dir: &str, extension: &str, prefix: &str) -> AppResult<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(dir).map_err(|error| format!("{dir}/: {error}"))? {
        let path = entry?.path();
        if path.is_file()
            && path.extension().and_then(|value| value.to_str()) == Some(extension)
            && path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("")
                .starts_with(prefix)
        {
            paths.push(path);
        }
    }

    paths.sort();
    if paths.is_empty() {
        return Err(format!("no {prefix}*.{extension} files in {dir}/").into());
    }

    Ok(paths)
}

// A complete JSON grammar rather than searching for a field-name substring.
// Escapes (including surrogate pairs), duplicate fields and finite numbers
// are validated. Nothing in this parser constructs n-grams across dropped text.
#[derive(Clone, Debug)]
enum Json {
    Object(BTreeMap<String, Json>),
    Array(Vec<Json>),
    String(String),
    Number(f64),
    Bool(bool),
    Null,
}

struct JsonParser<'a> {
    text: &'a str,
    p: usize,
}

impl<'a> JsonParser<'a> {
    fn ws(&mut self) {
        while self.p < self.text.len()
            && matches!(self.text.as_bytes()[self.p], b' ' | b'\n' | b'\r' | b'\t')
        {
            self.p += 1;
        }
    }

    fn byte(&self) -> Option<u8> {
        self.text.as_bytes().get(self.p).copied()
    }

    fn expect(&mut self, b: u8) -> AppResult<()> {
        if self.byte() == Some(b) {
            self.p += 1;
            Ok(())
        } else {
            Err(format!("expected {:?} at byte {}", b as char, self.p).into())
        }
    }

    fn hex4(&mut self) -> AppResult<u32> {
        let end = self.p + 4;
        let h = self
            .text
            .as_bytes()
            .get(self.p..end)
            .ok_or("truncated Unicode escape")?;
        if !h.iter().all(|b| b.is_ascii_hexdigit()) {
            return Err("invalid Unicode escape".into());
        }
        let n = u32::from_str_radix(std::str::from_utf8(h)?, 16)?;
        self.p = end;
        Ok(n)
    }

    fn string(&mut self) -> AppResult<String> {
        Ok(self.string_cow()?.into_owned())
    }

    // Corpus keys usually have no escapes. Borrow that exact UTF-8 slice
    // instead of allocating and appending one character at a time. Callers
    // retaining a key can still request an owned String via string().
    fn string_cow(&mut self) -> AppResult<std::borrow::Cow<'a, str>> {
        let start = self.p;
        if self.byte() == Some(b'"') {
            for (offset, byte) in self.text.as_bytes()[start + 1..].iter().enumerate() {
                match byte {
                    b'"' => {
                        let end = start + 1 + offset;
                        self.p = end + 1;
                        return Ok(std::borrow::Cow::Borrowed(&self.text[start + 1..end]));
                    }
                    b'\\' | 0..=31 => break,
                    _ => {}
                }
            }
        }
        self.string_decoded().map(std::borrow::Cow::Owned)
    }

    fn string_decoded(&mut self) -> AppResult<String> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            match self.byte().ok_or("unterminated JSON string")? {
                b'"' => {
                    self.p += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.p += 1;
                    let b = self.byte().ok_or("unterminated escape")?;
                    self.p += 1;
                    match b {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let mut cp = self.hex4()?;
                            if (0xd800..=0xdbff).contains(&cp) {
                                self.expect(b'\\')?;
                                self.expect(b'u')?;
                                let low = self.hex4()?;
                                if !(0xdc00..=0xdfff).contains(&low) {
                                    return Err("invalid Unicode surrogate pair".into());
                                }
                                cp = 0x10000 + ((cp - 0xd800) << 10) + (low - 0xdc00);
                            }
                            out.push(char::from_u32(cp).ok_or("invalid Unicode scalar")?);
                        }
                        _ => return Err("invalid JSON escape".into()),
                    }
                }
                0..=31 => return Err("unescaped control character in JSON".into()),
                _ => {
                    let ch = self.text[self.p..].chars().next().ok_or("invalid string")?;
                    out.push(ch);
                    self.p += ch.len_utf8();
                }
            }
        }
    }

    fn value(&mut self, depth: usize) -> AppResult<Json> {
        if depth > 64 {
            return Err("JSON nesting exceeds 64".into());
        }
        self.ws();
        match self.byte().ok_or("unexpected end of JSON")? {
            b'{' => {
                self.p += 1;
                self.ws();
                let mut out = BTreeMap::new();
                if self.byte() == Some(b'}') {
                    self.p += 1;
                    return Ok(Json::Object(out));
                }
                loop {
                    self.ws();
                    let k = self.string()?;
                    self.ws();
                    self.expect(b':')?;
                    let v = self.value(depth + 1)?;
                    if out.insert(k.clone(), v).is_some() {
                        return Err(format!("duplicate JSON key {k:?}").into());
                    }
                    self.ws();
                    if self.byte() == Some(b'}') {
                        self.p += 1;
                        break;
                    }
                    self.expect(b',')?;
                }
                Ok(Json::Object(out))
            }
            b'[' => {
                self.p += 1;
                self.ws();
                let mut out = Vec::new();
                if self.byte() == Some(b']') {
                    self.p += 1;
                    return Ok(Json::Array(out));
                }
                loop {
                    out.push(self.value(depth + 1)?);
                    self.ws();
                    if self.byte() == Some(b']') {
                        self.p += 1;
                        break;
                    }
                    self.expect(b',')?;
                }
                Ok(Json::Array(out))
            }
            b'"' => Ok(Json::String(self.string()?)),
            b't' | b'f' | b'n' => {
                for (word, value) in [
                    ("true", Json::Bool(true)),
                    ("false", Json::Bool(false)),
                    ("null", Json::Null),
                ] {
                    if self.text[self.p..].starts_with(word) {
                        self.p += word.len();
                        return Ok(value);
                    }
                }
                Err("invalid JSON literal".into())
            }
            b'-' | b'0'..=b'9' => {
                let start = self.p;
                if self.byte() == Some(b'-') {
                    self.p += 1;
                }
                if self.byte() == Some(b'0') {
                    self.p += 1;
                } else {
                    let p = self.p;
                    while self.byte().map(|b| b.is_ascii_digit()).unwrap_or(false) {
                        self.p += 1;
                    }
                    if self.p == p {
                        return Err("invalid number".into());
                    }
                }
                if self.byte() == Some(b'.') {
                    self.p += 1;
                    let p = self.p;
                    while self.byte().map(|b| b.is_ascii_digit()).unwrap_or(false) {
                        self.p += 1;
                    }
                    if self.p == p {
                        return Err("invalid fraction".into());
                    }
                }
                if matches!(self.byte(), Some(b'e') | Some(b'E')) {
                    self.p += 1;
                    if matches!(self.byte(), Some(b'+') | Some(b'-')) {
                        self.p += 1;
                    }
                    let p = self.p;
                    while self.byte().map(|b| b.is_ascii_digit()).unwrap_or(false) {
                        self.p += 1;
                    }
                    if self.p == p {
                        return Err("invalid exponent".into());
                    }
                }
                let n: f64 = self.text[start..self.p].parse()?;
                if !n.is_finite() {
                    return Err("non-finite JSON number".into());
                }
                Ok(Json::Number(n))
            }
            _ => Err(format!("unexpected JSON byte at {}", self.p).into()),
        }
    }
}

fn parse_json(text: &str) -> AppResult<Json> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut p = JsonParser { text, p: 0 };
    let j = p.value(0)?;
    p.ws();
    if p.p != text.len() {
        return Err("trailing JSON data".into());
    }
    Ok(j)
}

#[derive(Clone)]
struct Source {
    name: String,
    path: PathBuf,
    pub(crate) include_spacegrams: bool,
    tables: [Vec<(String, f64)>; 4],
    masses: [f64; 4],
    warnings: Vec<String>,
    fingerprint: u64,
}

impl Source {
    fn from_text(text: &str, path: &Path) -> AppResult<Self> {
        load_frequency_source(text, path)
    }

    fn from_text_with_limits(text: &str, path: &Path, limits: NgramLimits) -> AppResult<Self> {
        load_frequency_source_with_limits(text, path, limits)
    }

    fn load(path: &Path) -> AppResult<Self> {
        let mut timing = crate::load_profile::LoadProfile::new("ordinary corpus load");
        let resolved = if corpus_is_raw(path)? {
            ensure_corpus(path, false, None)?
        } else {
            path.to_owned()
        };
        timing.mark("Resolve/rebuild text cache if needed");
        let text = fs::read_to_string(&resolved)?;
        timing.mark("Corpus read");
        let limits = load_app_config()?.ngrams;
        let result = Self::from_text_with_limits(&text, &resolved, limits)
            .map_err(|e| format!("{}: {e}", resolved.display()).into());
        timing.mark("JSON parse and frequency tables");
        result
    }
}

/// One corpus n-gram over layout symbol ids. Kept at 16 bytes: the trial
/// loop reads affected grams scattered across the whole corpus.
#[derive(Clone, Copy, Debug)]
struct Gram {
    symbols: [u16; 3],
    length: u8,
    order_kind: u8,
    f: f64,
}

impl Gram {
    fn new(ids: [usize; 3], len: usize, kind: usize, f: f64) -> Self {
        debug_assert!(ids.iter().all(|&id| id <= u16::MAX as usize) && len <= 3 && kind <= 3);
        Self {
            symbols: ids.map(|id| id as u16),
            length: len as u8,
            order_kind: kind as u8,
            f,
        }
    }

    #[inline(always)]
    fn id(&self, j: usize) -> usize {
        self.symbols[j] as usize
    }

    #[inline(always)]
    fn ids(&self) -> [usize; 3] {
        self.symbols.map(usize::from)
    }

    #[inline(always)]
    fn len(&self) -> usize {
        self.length as usize
    }

    /// 0 monogram, 1 bigram, 2 skipgram, 3 trigram.
    #[inline(always)]
    fn kind(&self) -> usize {
        self.order_kind as usize
    }
}

#[derive(Clone)]
struct Corpus {
    name: String,
    source_path: PathBuf,
    include_spacegrams: bool,
    grams: Vec<Gram>,
    /// Indices into `grams` of the grams containing each symbol, ascending.
    by_symbol: Vec<Vec<u32>>,
    totals: [f64; 4],
    coverage: [f64; 4],
    warnings: Vec<String>,
    fingerprint: u64,
}

#[derive(Clone)]
struct Model {
    board: Board,
    canonical: Vec<u8>,
    original: Vec<usize>,
    geometry: Geometry,
}

impl Model {
    #[cfg(test)]
    fn new(board: Board) -> Self {
        Self::with_rolls(board, RollSettings::default())
    }

    fn with_rolls(board: Board, rolls: RollSettings) -> Self {
        let mut canonical = board.symbols.clone();
        canonical.sort_unstable();
        let original = board
            .symbols
            .iter()
            .map(|s| canonical.iter().position(|c| c == s).unwrap())
            .collect();
        let geometry = Geometry::with_alts(board.keys.clone(), rolls);
        Self {
            board,
            canonical,
            original,
            geometry,
        }
    }

    fn symbols(&self, arr: &[usize]) -> Vec<u8> {
        arr.iter().map(|&id| self.canonical[id]).collect()
    }

    fn corpus(&self, source: &Source) -> AppResult<Corpus> {
        let n = self.canonical.len();
        let mut ids = [usize::MAX; 128];
        for (i, &b) in self.canonical.iter().enumerate() {
            // Empty slots are physical placeholders, never tab/newline letters.
            if !blank(b) {
                ids[b as usize] = i;
                if b.is_ascii_alphabetic() {
                    ids[b.to_ascii_uppercase() as usize] = i;
                }
            }
        }
        if n > u16::MAX as usize {
            return Err("layout has too many symbols".into());
        }
        let mut totals = [0.0; 4];
        let mut grams = Vec::new();
        for kind in 0..4 {
            let mut merged: BTreeMap<Vec<usize>, f64> = BTreeMap::new();
            for (s, f) in &source.tables[kind] {
                if !s.is_ascii() {
                    continue;
                }
                let mapped: Vec<usize> = s.bytes().map(|b| ids[b as usize]).collect();
                if mapped.iter().any(|&i| i == usize::MAX) {
                    continue;
                }
                *merged.entry(mapped).or_default() += f;
            }
            for (v, f) in merged {
                if f == 0.0 {
                    continue;
                }
                totals[kind] += f;
                let mut out = [0; 3];
                for (i, &id) in v.iter().enumerate() {
                    out[i] = id;
                }
                grams.push(Gram::new(out, v.len(), kind, f));
            }
        }
        if totals[0] <= 0.0 || totals[1] <= 0.0 {
            return Err("layout has no usable letters/bigrams in this corpus".into());
        }
        let mut by_symbol = vec![Vec::new(); n];
        for (i, g) in grams.iter().enumerate() {
            let ids = g.ids();
            for j in 0..g.len() {
                if !ids[..j].contains(&ids[j]) {
                    by_symbol[ids[j]].push(u32::try_from(i).map_err(|_| "too many n-grams")?);
                }
            }
        }
        Ok(Corpus {
            name: source.name.clone(),
            source_path: source.path.clone(),
            include_spacegrams: source.include_spacegrams,
            grams,
            by_symbol,
            totals,
            coverage: std::array::from_fn(|i| pct(totals[i], source.masses[i])),
            warnings: source.warnings.clone(),
            fingerprint: source.fingerprint,
        })
    }
}

#[derive(Clone, Debug)]
struct Raw([f64; N_RAW], Option<mana2_metrics::RawStats>);

impl Default for Raw {
    fn default() -> Self {
        Self([0.0; N_RAW], None)
    }
}

impl Raw {
    fn with_mana2() -> Self {
        Self([0.0; N_RAW], Some(mana2_metrics::RawStats::default()))
    }
}

fn positions(arr: &[usize]) -> Vec<usize> {
    let mut p = vec![0; arr.len()];
    for (i, &id) in arr.iter().enumerate() {
        p[id] = i;
    }
    p
}

/// Move `f` from the counters flagged in `from` to those flagged in `to`;
/// counters flagged in both are untouched.
#[inline(always)]
fn move_bits(raw: &mut Raw, from: u128, to: u128, f: f64) {
    let signed = [-f, f];
    for (half, offset) in [(0, 0), (64, 64)] {
        let to = (to >> half) as u64;
        let mut changed = (from >> half) as u64 ^ to;
        while changed != 0 {
            let bit = changed.trailing_zeros() as usize;
            raw.0[bit + offset] += signed[(to >> bit) as usize & 1];
            changed &= changed - 1;
        }
    }
}

fn add_bits(raw: &mut Raw, bits: u128, f: f64) {
    let mut low = bits as u64;
    while low != 0 {
        let bit = low.trailing_zeros() as usize;
        raw.0[bit] += f;
        low &= low - 1;
    }
    let mut high = (bits >> 64) as u64;
    while high != 0 {
        let bit = high.trailing_zeros() as usize + 64;
        raw.0[bit] += f;
        high &= high - 1;
    }
}

fn add_gram(raw: &mut Raw, g: &Gram, pos: &[usize], geometry: &Geometry, sign: f64) {
    let f = g.f * sign;
    let a = pos[g.id(0)];
    if let Some(mana) = &mut raw.1 {
        let ids = g.ids().map(|id| pos[id]);
        geometry.mana2().add_gram(mana, g.kind(), &ids[..g.len()], f);
    }
    match g.kind() {
        0 => {
            let key = geometry.keys[a];
            raw.0[USAGE + key.finger] += f;
            if key.main && !geometry.is_home[a] {
                raw.0[OFF + key.finger] += f;
            }
            raw.0[TRAVEL] += geometry.home[a][0] * f;
            raw.0[VTRAVEL] += geometry.home[a][1] * f;
            raw.0[LTRAVEL] += geometry.home[a][2] * f;
        }
        1 | 2 => {
            let b = pos[g.id(1)];
            let flags = geometry.pair[a * geometry.n + b];
            add_bits(raw, if g.kind() == 1 { flags.bi } else { flags.sk }, f);
            if g.kind() == 1 {
                if flags.main {
                    raw.0[SRAF_DEN] += f;
                }
                raw.0[SFTRAVEL] += geometry.sf_distance[a * geometry.n + b] * f;
            }
        }
        _ => {
            let b = pos[g.id(1)];
            let c = pos[g.id(2)];
            add_bits(raw, geometry.tri.raw((a * geometry.n + b) * geometry.n + c), f);
        }
    }
}

#[inline(always)]
/// Move only the Mana2 counters; see `Problem::normal_counters_unused`.
fn move_gram_physical(raw: &mut Raw, g: &Gram, old: &[usize], new: &[usize], geometry: &Geometry) {
    if let Some(mana) = &mut raw.1 {
        let from = g.ids().map(|id| old[id]);
        let to = g.ids().map(|id| new[id]);
        geometry.mana2().replace_gram(mana, g.kind(), &from[..g.len()], &to[..g.len()], g.f);
    }
}

/// Move one gram from positions `old` to `new`. Equivalent to subtracting
/// `add_gram` at `old` and adding it at `new`, except that counting flags set
/// at both positions are left alone instead of cancelling.
#[inline(always)]
fn move_gram(raw: &mut Raw, g: &Gram, old: &[usize], new: &[usize], geometry: &Geometry) {
    move_gram_physical(raw, g, old, new, geometry);
    let f = g.f;
    let n = geometry.n;
    let (a0, a1) = (old[g.id(0)], new[g.id(0)]);
    match g.kind() {
        0 => {
            for (a, f) in [(a0, -f), (a1, f)] {
                let key = geometry.keys[a];
                raw.0[USAGE + key.finger] += f;
                if key.main && !geometry.is_home[a] {
                    raw.0[OFF + key.finger] += f;
                }
                raw.0[TRAVEL] += geometry.home[a][0] * f;
                raw.0[VTRAVEL] += geometry.home[a][1] * f;
                raw.0[LTRAVEL] += geometry.home[a][2] * f;
            }
        }
        1 | 2 => {
            let (b0, b1) = (old[g.id(1)], new[g.id(1)]);
            let (from, to) = (geometry.pair[a0 * n + b0], geometry.pair[a1 * n + b1]);
            let (x, y) = if g.kind() == 1 { (from.bi, to.bi) } else { (from.sk, to.sk) };
            move_bits(raw, x, y, f);
            if g.kind() == 1 {
                if from.main != to.main {
                    raw.0[SRAF_DEN] += if to.main { f } else { -f };
                }
                raw.0[SFTRAVEL] -= geometry.sf_distance[a0 * n + b0] * f;
                raw.0[SFTRAVEL] += geometry.sf_distance[a1 * n + b1] * f;
            }
        }
        _ => {
            let from = (a0 * n + old[g.id(1)]) * n + old[g.id(2)];
            let to = (a1 * n + new[g.id(1)]) * n + new[g.id(2)];
            geometry.tri.move_counters(raw, from, to, f);
        }
    }
}

fn full_raw(arr: &[usize], c: &Corpus, g: &Geometry) -> Raw {
    full_raw_with_mana2(arr, c, g, false)
}

fn full_raw_with_mana2(arr: &[usize], c: &Corpus, g: &Geometry, mana2: bool) -> Raw {
    let pos = positions(arr);
    let mut raw = if mana2 {
        Raw::with_mana2()
    } else {
        Raw::default()
    };
    for gram in &c.grams {
        add_gram(&mut raw, gram, &pos, g, 1.0);
    }
    raw
}

#[derive(Clone, Debug)]
struct Metrics {
    v: [f64; N_METRICS],
    usage: [f64; 10],
    off: [f64; 8],
}

fn denominator(m: usize, raw: &Raw, c: &Corpus) -> f64 {
    denominator_totals(m, raw, &c.totals)
}

fn denominator_totals(m: usize, raw: &Raw, totals: &[f64; 4]) -> f64 {
    if matches!(m, TRAVEL | VTRAVEL | LTRAVEL) {
        totals[0]
    } else if matches!(m, SRAF | INSRAF | OUTSRAF) {
        raw.0[SRAF_DEN].max(0.0)
    } else if roll_metric(m) {
        raw.0[ROLL_DEN].max(0.0)
    } else if is_rhythm(m) {
        raw.0[RHYTHM_DEN].max(0.0)
    } else if is_skip(m) {
        totals[2]
    } else {
        totals[1]
    }
}

fn metrics(raw: &Raw, c: &Corpus) -> Metrics {
    metrics_totals(raw, &c.totals)
}

fn metrics_totals(raw: &Raw, totals: &[f64; 4]) -> Metrics {
    Metrics {
        v: std::array::from_fn(|m| pct(raw.0[m].max(0.0), denominator_totals(m, raw, totals))),
        usage: std::array::from_fn(|i| pct(raw.0[USAGE + i].max(0.0), totals[0])),
        off: std::array::from_fn(|i| pct(raw.0[OFF + i].max(0.0), totals[0])),
    }
}

// Keep detailed weights, roll policy and AKL.GG schedules in one immutable
// objective snapshot for evaluations and worker threads.
#[derive(Clone, Debug)]
struct Weights(
    [f64; N_WEIGHTS],
    RollSettings,
    mana2_metrics::Weights,
    bool,
    bool,
    /// `[combos] fatfingering`: score fat-finger combos; see `combos::Policy`.
    bool,
);

impl Weights {
    fn new(values: [f64; N_WEIGHTS]) -> Self {
        Self(
            values,
            RollSettings::default(),
            mana2_metrics::Weights::default(),
            true,
            true,
            false,
        )
    }

    /// Geometry policy; `[alts]` maps apply only while `[typing] alt_fingers`
    /// is on.
    fn rolls(&self) -> RollSettings {
        RollSettings {
            alts: self.1.alts.filter(|_| self.3),
            ..self.1
        }
    }

    /// Alternate fingers for action evaluation: `[typing] alt_fingers`, with
    /// `[alts]` position rules replacing layouts' own rules while set.
    fn alts(&self) -> crate::alt_fingers::AltPolicy {
        crate::alt_fingers::AltPolicy {
            enabled: self.3,
            positions: self.1.alts,
        }
    }

    /// Alternate-finger state for headers and policy rows.
    fn alt_fingers_label(&self) -> &'static str {
        if !self.3 {
            "disabled"
        } else if self.1.alts.is_some() {
            "[alts] locations"
        } else {
            "enabled"
        }
    }

    /// How combos are scored: `[typing] combos` and `[combos] fatfingering`.
    fn combo_policy(&self) -> crate::combos::Policy {
        crate::combos::Policy {
            enabled: self.4,
            fatfinger: self.5,
        }
    }

    /// Lower-is-better AKL.GG score of `stats`.
    fn physical_score(&self, stats: &mana2_metrics::Stats) -> f64 {
        -mana2_metrics::score(stats, &self.2)
    }

    /// Local slopes of the lower-is-better physical score for each stat.
    fn physical_slopes(&self, stats: &mana2_metrics::Stats) -> [f64; mana2_metrics::N_STATS] {
        mana2_metrics::score_slopes(stats, &self.2).map(|slope| -slope)
    }

    fn with_rolls(mut self, rolls: RollSettings) -> Self {
        self.1 = rolls;
        self
    }
}

impl Default for Weights {
    fn default() -> Self {
        Self::new([
            12.0, 1.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 3.0, 0.75, 1.0, 0.25, 0.75, 2.5, 1.25, 0.0,
            0.0, 4.0, 2.0, 1.5, 0.75, 0.05, 0.50, 0.15, 0.02, 0.0, 0.0, 0.10, 0.0, 0.0, 0.0, 0.0,
            0.25, 0.25, 0.25, 0.25, 1.0, 0.5, 0.4, 0.2, 1.0, 0.5, 0.4, 0.2, 0.60, 0.20, 0.05, 0.0,
            0.06, 0.02, 0.005, 0.0,
        ])
    }
}

#[derive(Clone, Debug)]
struct Breakdown {
    penalty: f64,
    bonus: f64,
    net: f64,
    contributions: [f64; N_WEIGHTS],
}

impl Default for Breakdown {
    fn default() -> Self {
        Self {
            penalty: 0.0,
            bonus: 0.0,
            net: 0.0,
            contributions: [0.0; N_WEIGHTS],
        }
    }
}

fn breakdown(m: &Metrics, w: &Weights) -> Breakdown {
    let mut b = Breakdown::default();
    for i in 0..N_METRICS {
        if !aggregate(i) {
            b.contributions[i] = m.v[i] * w.0[i] * if higher_better(i) { -1.0 } else { 1.0 };
        }
    }
    for i in 0..4 {
        b.contributions[N_METRICS + i] = (m.off[i] + m.off[7 - i]) * w.0[N_METRICS + i];
    }
    for i in 0..4 {
        b.contributions[USAGE_WEIGHT + i] = (m.usage[i] + m.usage[7 - i]) * w.0[USAGE_WEIGHT + i];
    }
    for &v in &b.contributions {
        if v >= 0.0 {
            b.penalty += v;
        } else {
            b.bonus -= v;
        }
    }
    b.net = b.penalty - b.bonus;
    b
}

// AKL.GG weights use negative penalties and positive rewards. Present its
// objective in the same lower-is-better convention as the optimizer and ranker.
fn mana_breakdown(stats: &mana2_metrics::Stats, weights: &Weights) -> Breakdown {
    let mut result = Breakdown::default();
    let contributions = mana2_metrics::score_contributions(stats, &weights.2);
    for contribution in contributions {
        if contribution < 0.0 {
            result.penalty -= contribution;
        } else {
            result.bonus += contribution;
        }
    }
    result.net = weights.physical_score(stats);
    result
}

fn mana_stats_for_corpus(raw: &Raw, corpus: &Corpus) -> mana2_metrics::Stats {
    let source_totals = std::array::from_fn(|index| {
        if corpus.coverage[index] > 0.0 {
            corpus.totals[index] * 100.0 / corpus.coverage[index]
        } else {
            corpus.totals[index]
        }
    });
    mana2_metrics::stats_with_speed_totals(
        raw.1.as_ref().expect("mana counters must be enabled"),
        corpus.totals,
        source_totals,
    )
}

fn finite_nonnegative(s: &str) -> AppResult<f64> {
    let n: f64 = s.trim().parse()?;
    if !n.is_finite() || n < 0.0 || n > 1.0e6 {
        return Err("expected a finite number from 0 to 1000000".into());
    }
    Ok(n)
}

fn config_lines(text: &str) -> AppResult<Vec<(String, String)>> {
    let mut result = Vec::new();
    let mut seen = BTreeSet::new();
    for (i, line) in text.lines().enumerate() {
        let l = line.split('#').next().unwrap_or("").trim();
        if l.is_empty() {
            continue;
        }
        let (k, v) = l
            .split_once('=')
            .ok_or_else(|| format!("line {}: expected name = value", i + 1))?;
        let k = k.trim().to_ascii_lowercase();
        if !seen.insert(k.clone()) {
            return Err(format!("duplicate setting {k}").into());
        }
        result.push((k, v.trim().to_string()));
    }
    Ok(result)
}

fn canonical_weight_key(key: &str) -> &str {
    match key {
        "dfsb" => "dafjb",
        "cfsb" => "cafjb",
        "dfss" => "dafjs",
        "cfss" => "cafjs",
        "dfjb" => "dafjb",
        "cfjb" => "cafjb",
        "dfjs" => "dafjs",
        "cfjs" => "cafjs",
        "dfab" => "dafjb",
        "cfab" => "cafjb",
        "dfas" => "dafjs",
        "cfas" => "cafjs",
        "dsb" => "dnfjb",
        "dss" => "dnfjs",
        "csb" => "cnfjb",
        "css" => "cnfjs",
        "dfnb" => "dnfjb",
        "dfns" => "dnfjs",
        "cfnb" => "cnfjb",
        "cfns" => "cnfjs",
        _ => key,
    }
}

fn weights_from_text(text: &str) -> AppResult<Weights> {
    let entries: BTreeMap<String, String> = config_lines(text)?.into_iter().collect();
    let mut w = Weights::default();
    for (k, v) in &entries {
        if matches!(k.as_str(), "dhjb" | "chjb" | "dhjs" | "chjs") {
            finite_nonnegative(v)?;
            continue;
        }
        let name = canonical_weight_key(k);
        let i = WEIGHT_NAMES
            .iter()
            .position(|s| *s == name)
            .ok_or_else(|| format!("unknown weight {k}"))?;
        let value = finite_nonnegative(v)?;
        if !aggregate(i) && (k == name || !entries.contains_key(name)) {
            w.0[i] = value;
        }
    }
    // Legacy aggregate keys seed directional jump weights. Explicit direction
    // keys win regardless of line order; display subsets are never weighted.
    for (legacy, d, c) in [("fsb", DAFJB, CAFJB), ("fss", DAFJS, CAFJS)] {
        if let Some(v) = entries.get(legacy) {
            let value = finite_nonnegative(v)?;
            if !entries
                .keys()
                .any(|key| canonical_weight_key(key) == WEIGHT_NAMES[d])
            {
                w.0[d] = value;
            }
            if !entries
                .keys()
                .any(|key| canonical_weight_key(key) == WEIGHT_NAMES[c])
            {
                w.0[c] = value * 0.5;
            }
        }
    }
    // A saved broad half-jump weight applies to both finger distances.
    for (legacy, adjacent, nonadjacent) in [
        ("dhjb", DAHJB, DNHJB),
        ("chjb", CAHJB, CNHJB),
        ("dhjs", DAHJS, DNHJS),
        ("chjs", CAHJS, CNHJS),
    ] {
        if let Some(v) = entries.get(legacy) {
            let value = finite_nonnegative(v)?;
            for metric in [adjacent, nonadjacent] {
                if !entries.contains_key(WEIGHT_NAMES[metric]) {
                    w.0[metric] = value;
                }
            }
        }
    }
    // The old HSB/HSS settings seeded broad directional half-jump weights.
    for (legacy, d_adj, d_nonadj, c_adj, c_nonadj, old_d, old_c) in [
        ("hsb", DAHJB, DNHJB, CAHJB, CNHJB, "dhjb", "chjb"),
        ("hss", DAHJS, DNHJS, CAHJS, CNHJS, "dhjs", "chjs"),
    ] {
        if let Some(v) = entries.get(legacy) {
            let value = finite_nonnegative(v)?;
            for metric in [d_adj, d_nonadj] {
                if !entries.contains_key(WEIGHT_NAMES[metric]) && !entries.contains_key(old_d) {
                    w.0[metric] = value;
                }
            }
            for metric in [c_adj, c_nonadj] {
                if !entries.contains_key(WEIGHT_NAMES[metric]) && !entries.contains_key(old_c) {
                    w.0[metric] = value * 0.5;
                }
            }
        }
    }
    // Combined legacy rewards seed both directions. An explicit directional
    // setting always takes precedence, regardless of configuration order.
    for (legacy, inward, outward) in [
        ("sraf_reward", INSRAF, OUTSRAF),
        ("roll_reward", INROLL, OUTROLL),
    ] {
        if let Some(value) = entries.get(legacy) {
            let value = finite_nonnegative(value)?;
            if !entries.contains_key(WEIGHT_NAMES[inward]) {
                w.0[inward] = value;
            }
            if !entries.contains_key(WEIGHT_NAMES[outward]) {
                w.0[outward] = value;
            }
        }
    }
    Ok(w)
}

fn load_weights(path: &Path) -> AppResult<Weights> {
    if path == Path::new(WEIGHTS_FILE) {
        return Ok(load_app_config()?.weights);
    }
    if !path.exists() {
        return Ok(Weights::default());
    }
    weights_from_text(&fs::read_to_string(path)?)
}

fn weights_text(w: &Weights) -> String {
    let mut s = String::from("# Percentages x weights; preferences are subtracted.\n# FSB/FSS and HSB/HSS duplicate adjacent discordant jumps. A and N jump weights score disjoint finger distances.\n");
    for i in 0..N_WEIGHTS {
        if !aggregate(i) {
            s.push_str(&format!("{} = {}\n", weight_display_name(i), w.0[i]));
        }
    }
    s
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CreditView {
    Clean,
    Raw,
    Rejected,
}

impl CreditView {
    fn next(self) -> Self {
        match self {
            Self::Clean => Self::Raw,
            Self::Raw => Self::Rejected,
            Self::Rejected => Self::Clean,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::Raw => "raw",
            Self::Rejected => "rejected",
        }
    }
}

fn contribution_mass(m: usize, raw: &Raw, view: CreditView) -> f64 {
    match raw_positive(m) {
        Some(r) => match view {
            CreditView::Raw => raw.0[r].max(0.0),
            CreditView::Rejected => (raw.0[r] - raw.0[m]).max(0.0),
            _ => raw.0[m].max(0.0),
        },
        None => raw.0[m].max(0.0),
    }
}

fn blocker_names(bits: u128) -> String {
    let mut names = Vec::new();
    // Show directional jumps instead of repeating their adjacent display subsets.
    for m in [
        SFB, SFS, SKB, SKS, DAFJB, CAFJB, DAFJS, CAFJS, DAHJB, CAHJB, DAHJS, CAHJS, LSB, LSS,
        DNFJB, DNFJS, CNFJB, CNFJS, DNHJB, CNHJB, DNHJS, CNHJS, REDIR, WRED, WISH,
    ] {
        if bits & bit(m) != 0 {
            names.push(METRIC_NAMES[m]);
        }
    }
    names.join("+")
}

fn reward_blockers(g: &Gram, pos: &[usize], geometry: &Geometry) -> u128 {
    let a = pos[g.id(0)];
    let b = pos[g.id(1)];
    // Only SRAF retains a raw/rejected contribution view.
    geometry.pair[a * geometry.n + b].bi & BAD_BI
}

#[derive(Clone, Debug)]
struct Contributor {
    gram: String,
    before: f64,
    after: f64,
    why_before: u128,
    why_after: u128,
}

fn gram_name(g: &Gram, canonical: &[u8]) -> String {
    let chars: Vec<String> = g.ids()[..g.len()]
        .iter()
        .map(|&id| display_symbol(canonical[id]).to_string())
        .collect();
    chars.join(if g.kind() == 2 { "_" } else { "" })
}

fn contributor_data_mode(
    m: usize,
    before: &[usize],
    after: &[usize],
    c: &Corpus,
    model: &Model,
    merge_reverse: bool,
    view: CreditView,
) -> Vec<Contributor> {
    let p0 = positions(before);
    let p1 = positions(after);
    let r0 = full_raw(before, c, &model.geometry);
    let r1 = full_raw(after, c, &model.geometry);
    let d0 = denominator(m, &r0, c);
    let d1 = denominator(m, &r1, c);
    let mut map: BTreeMap<String, Contributor> = BTreeMap::new();
    for g in &c.grams {
        if g.kind()
            != if matches!(m, TRAVEL | VTRAVEL | LTRAVEL) {
                0
            } else if is_rhythm(m) {
                3
            } else if is_skip(m) {
                2
            } else {
                1
            }
        {
            continue;
        }
        let mut a = Raw::default();
        let mut b = Raw::default();
        add_gram(&mut a, g, &p0, &model.geometry, 1.0);
        add_gram(&mut b, g, &p1, &model.geometry, 1.0);
        let v0 = pct(contribution_mass(m, &a, view), d0);
        let v1 = pct(contribution_mass(m, &b, view), d1);
        if v0 == 0.0 && v1 == 0.0 {
            continue;
        }
        let mut key = gram_name(g, &model.canonical);
        if merge_reverse && g.len() == 2 {
            let mut reverse = *g;
            reverse.symbols.swap(0, 1);
            let rev = gram_name(&reverse, &model.canonical);
            if rev < key {
                key = rev;
            }
        }
        let item = map.entry(key.clone()).or_insert_with(|| Contributor {
            gram: key,
            before: 0.0,
            after: 0.0,
            why_before: 0,
            why_after: 0,
        });
        item.before += v0;
        item.after += v1;
        if view == CreditView::Rejected && raw_positive(m).is_some() {
            if v0 > 0.0 {
                item.why_before |= reward_blockers(g, &p0, &model.geometry);
            }
            if v1 > 0.0 {
                item.why_after |= reward_blockers(g, &p1, &model.geometry);
            }
        }
    }
    let mut result: Vec<_> = map.into_values().collect();
    result.sort_by(|a, b| {
        b.after
            .total_cmp(&a.after)
            .then_with(|| a.gram.cmp(&b.gram))
    });
    result
}

fn metric_value_text(m: usize, v: f64, decimals: usize) -> String {
    if physical_metric(m) {
        number(v, decimals)
    } else {
        format!("{}%", number(v, decimals))
    }
}

#[cfg(test)]
mod numeric_geometry_tests {
    use super::*;

    const ROWS: &str =
        "q w e r t | y u i o p\na s d f g | h j k l ;\nz x c v b | n m , . /\nthumbs: space\n";

    fn parse(extra: &str) -> Board {
        board_from_text(&format!("{ROWS}{extra}"), Path::new("inline")).unwrap()
    }

    #[test]
    fn old_coordinates_keep_their_exact_distances() {
        for extra in [
            "",
            "row-stagger: standard\n",
            "row-stagger: anglemod\n",
            "row-stagger: nokwts\n",
            "row-stagger: meteorite\n",
        ] {
            let board = parse(extra);
            let geometry = Geometry::new(board.keys.clone());
            for (index, key) in board.keys.iter().copied().enumerate() {
                let expected = home_travel(key);
                for axis in 0..3 {
                    assert_eq!(
                        geometry.home[index][axis].to_bits(),
                        expected[axis].to_bits()
                    );
                }
            }
            for (a, first) in board.keys.iter().enumerate() {
                for (b, second) in board.keys.iter().enumerate() {
                    if first.main && second.main && first.finger == second.finger {
                        let expected = ((first.row - second.row) as f64)
                            .hypot(first.horizontal_delta(*second));
                        assert_eq!(
                            geometry.sf_distance[a * geometry.n + b].to_bits(),
                            expected.to_bits()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn uniform_row_translation_cancels_from_travel_and_pair_distances() {
        let first = Geometry::new(parse("row-offsets: 0 0.25 0.75\n").keys);
        let shifted = Geometry::new(parse("row-offsets: -0.25 0 0.5\n").keys);
        for (a, b) in first
            .home
            .iter()
            .flatten()
            .zip(shifted.home.iter().flatten())
        {
            assert_eq!(a.to_bits(), b.to_bits());
        }
        for (a, b) in first.sf_distance.iter().zip(&shifted.sf_distance) {
            assert_eq!(a.to_bits(), b.to_bits());
        }
        for (a, b) in first.pair.iter().zip(&shifted.pair) {
            assert_eq!((a.bi, a.sk), (b.bi, b.sk));
        }
    }

    #[test]
    fn home_anchors_follow_translated_ten_eleven_and_twelve_column_fingermaps() {
        let cases = [
            (
                "q w e r t | y u i o p\na s d f g | h j k l ;\nz x c v b | n m , . /\n",
                "LP LR LM LI LI RI RI RM RR RP",
            ),
            (
                "~ q w e r t | y u i o p\n~ a s d f g | h j k l ;\n~ z x c v b | n m , . /\n",
                "LP LP LR LM LI LI RI RI RM RR RP",
            ),
            (
                "~ q w e r t | y u i o p ~\n~ a s d f g | h j k l ; ~\n~ z x c v b | n m , . / ~\n",
                "LP LP LR LM LI LI RI RI RM RR RP RP",
            ),
        ];
        for (rows, fingers) in cases {
            let source = format!("{rows}fingermap: {fingers} / {fingers} / {fingers}\n");
            let standard = board_from_text(&source, Path::new("standard.dat")).unwrap();
            let absolute = board_from_text(
                &format!("{source}col-layout: absolute\n"),
                Path::new("absolute.dat"),
            )
            .unwrap();
            let first = Geometry::new(standard.keys);
            let shifted = Geometry::new(absolute.keys);
            assert_eq!(first.home.len(), shifted.home.len());
            for (a, b) in first
                .home
                .iter()
                .flatten()
                .zip(shifted.home.iter().flatten())
            {
                assert_eq!(a.to_bits(), b.to_bits());
            }
            assert_eq!(first.is_home, shifted.is_home);
        }

        let (rows, fingers) = cases[2];
        let absolute = board_from_text(
            &format!("{rows}fingermap: {fingers} / {fingers} / {fingers}\ncol-layout: absolute\n"),
            Path::new("absolute.dat"),
        )
        .unwrap();
        let homes = home_slot_mask(&absolute.keys);
        let at = |finger, col| {
            absolute
                .keys
                .iter()
                .position(|key| key.main && key.row == 1 && key.finger == finger && key.col == col)
                .unwrap()
        };
        assert!(!homes[at(0, 0)]);
        assert!(homes[at(0, 1)]);
        assert!(!homes[at(4, 6)]);
        assert!(homes[at(4, 7)]);
    }

    #[test]
    fn column_offsets_use_actual_home_and_cross_column_same_finger_distance() {
        let board = parse("column-offsets: 0 -0.3 -0.4 -0.3 -0.2 -0.2 -0.3 -0.4 -0.3 0\n");
        let geometry = Geometry::new(board.keys.clone());
        // QWERTY W/S share their physical column, so vertical translation cancels.
        assert_eq!(geometry.home[1][0].to_bits(), 1.0f64.to_bits());
        assert_eq!(geometry.home[11][0].to_bits(), 0.0f64.to_bits());
        // R/T use the same index finger but have different column heights.
        assert_eq!(
            geometry.sf_distance[3 * geometry.n + 4].to_bits(),
            0.1f64.hypot(1.0).to_bits()
        );
        assert_eq!(geometry.home[4][0].to_bits(), 1.0f64.hypot(0.9).to_bits());

        let plain = parse("");
        for (a, b) in [(0, 21), (1, 22), (3, 20), (20, 3)] {
            assert_eq!(
                scissor_kind(board.keys[a], board.keys[b]),
                scissor_kind(plain.keys[a], plain.keys[b])
            );
            assert_eq!(
                row_motion(board.keys[a], board.keys[b]),
                row_motion(plain.keys[a], plain.keys[b])
            );
        }
    }

    #[test]
    fn ragged_rows_custom_fingers_and_offsets_round_trip() {
        let rows = "q w e r t | y u i o p\na s d f g | h j k l ; ?\nz x c v b | n m , . /\nthumbs: none none\n";
        let source = format!("{rows}row-offsets: 0 0.25 0.75\nfingermap: LP LR LM LM LI RI RI RM RR RP / LP LR LM LI LI RI RI RM RR RP RP / LP LR LI LI LI RI RI RM RR RP\n");
        let board = board_from_text(&source, Path::new("inline")).unwrap();
        assert_eq!(board.keys.len(), 31);
        let action = crate::action_keys::Layout::parse(&source, Path::new("inline")).unwrap();
        assert_eq!(board.keys, crate::action_ui::physical_keys(&action));
        let round =
            board_from_text(&board_text(&board, &board.symbols), Path::new("round")).unwrap();
        assert_eq!(round.keys, board.keys);
        assert_eq!(round.symbols, board.symbols);
        let round = crate::action_keys::Layout::parse(&action.text(), Path::new("round")).unwrap();
        assert_eq!(round.slots, action.slots);
    }

    #[test]
    fn geometry_validation_is_shared_and_subtraction_cannot_overflow() {
        for extra in [
            "row-offsets: NaN 0 0\n",
            "row-offsets: 0 0.0001 0\n",
            "row-offsets: 33 0 0\n",
            "row-offsets: 0 0\n",
            "column-offsets: 0 0\n",
            "fingermap: LP / LP / LP\n",
        ] {
            let source = format!("{ROWS}{extra}");
            assert!(board_from_text(&source, Path::new("inline")).is_err());
            assert!(crate::action_keys::Layout::parse(&source, Path::new("inline")).is_err());
        }
        let mut first = main_key(0, 0);
        let mut second = first;
        first.row_offset = 32000;
        second.row_offset = -32000;
        first.column_offset = -32000;
        second.column_offset = 32000;
        assert_eq!(first.horizontal_delta(second), 64.0);
        assert_eq!(first.vertical_delta(second), -64.0);
    }
}
