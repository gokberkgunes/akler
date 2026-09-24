type AppResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const CORPUS_DIR: &str = "corpora";

const LAYOUT_DIR: &str = "layouts";

const WEIGHTS_FILE: &str = "optimizer-weights.conf";

const SEARCH_FILE: &str = "optimizer-search.conf";

const MODEL_VERSION: &str = "ortholinear-custom-6-directional-rolls";

const DEFAULT_CORPUS: &str = "corpus-reddit.json";

// Pinky, ring, middle, index: retained from the last agreed implementation.
// This is a modeling assumption, not a universal anatomical measurement.
const LENGTH_RANK: [i8; 4] = [0, 1, 3, 2];

const N_METRICS: usize = 33;

const N_WEIGHTS: usize = N_METRICS + 4;

const SFB: usize = 0;

const SFS: usize = 1;

const FSB: usize = 2;

const HSB: usize = 3;

const FSS: usize = 4;

const HSS: usize = 5;

const LSB: usize = 6;

const LSS: usize = 7;

const DSB: usize = 8;

const DSS: usize = 9;

const REDIR: usize = 10;

const WRED: usize = 11;

const WISH: usize = 12;

const OSF: usize = 13;

const SRAF: usize = 14;

const ROLL: usize = 15;

const DFSB: usize = 16;

const CFSB: usize = 17;

const DFSS: usize = 18;

const CFSS: usize = 19;

const ALT: usize = 20;

const CSB: usize = 21;

const CSS: usize = 22;

const TRAVEL: usize = 23;

const VTRAVEL: usize = 24;

const LTRAVEL: usize = 25;

const SFTRAVEL: usize = 26;

// Roll breakdowns are display-only; ROLL carries their combined reward.
const IN2: usize = 27;

const OUT2: usize = 28;

const IN3: usize = 29;

const OUT3: usize = 30;

const INROLL: usize = 31;

const OUTROLL: usize = 32;

const USAGE: usize = N_METRICS;

const OFF: usize = USAGE + 10;

const SRAF_DEN: usize = OFF + 8;

const RHYTHM_DEN: usize = SRAF_DEN + 1;

const RAW_SRAF: usize = RHYTHM_DEN + 1;

const RAW_ROLL: usize = RAW_SRAF + 1;

const RAW_ALT: usize = RAW_ROLL + 1;

const ROW1_BI: usize = RAW_ALT + 1;

const ROW1_SK: usize = ROW1_BI + 1;

const ROW2_BI: usize = ROW1_SK + 1;

const ROW2_SK: usize = ROW2_BI + 1;

const SIMPLE_ROLL: usize = ROW2_SK + 1;

const ROLL_DEN: usize = SIMPLE_ROLL + 1;

const N_RAW: usize = ROLL_DEN + 1;

const SIMPLE_NAMES: [&str; 7] = ["SFB", "SFS", "Lat. stretch", "1-row change", "2-row change", "SRAF", "ROLL"];

const SIMPLE_KEYS: [&str; 7] = ["sfb", "sfs", "lateral", "row1", "row2", "sraf_reward", "roll_reward"];

const DEFAULT_SIMPLE: [f64; 7] = [12.0, 1.5, 2.0, 0.75, 2.0, 0.25, 0.25];

const METRIC_NAMES: [&str; N_METRICS] = [
    "SFB", "SFS", "FSB", "HSB", "FSS", "HSS", "LSB", "LSS",
    "DSB", "DSS", "RED", "WRED", "WISH", "OSF", "SRAF", "ROLL",
    "DFSB", "CFSB", "DFSS", "CFSS", "ALT", "CSB", "CSS",
    "TRAVEL", "VTRAVEL", "LTRAVEL", "SFTRAVEL",
    "IN2", "OUT2", "IN3", "OUT3", "INROLL", "OUTROLL",
];

// FSB/FSS and roll breakdowns are display-only. Weight D/C scissors and total ROLL.
const WEIGHT_NAMES: [&str; N_WEIGHTS] = [
    "sfb", "sfs", "fsb", "hsb", "fss", "hss", "lsb", "lss", "dsb", "dss",
    "red", "wred", "wish", "osf", "sraf_reward", "roll_reward",
    "dfsb", "cfsb", "dfss", "cfss", "alt_reward", "csb", "css",
    "travel", "vtravel", "ltravel", "sftravel",
    "in2", "out2", "in3", "out3", "inroll", "outroll",
    "off_pinky", "off_ring", "off_middle", "off_index",
];

const FINGER_NAMES: [&str; 10] = ["LP", "LR", "LM", "LI", "RI", "RM", "RR", "RP", "LT", "RT"];

const METRIC_HELP: [&str; N_METRICS] = [
    "Different keys on the same physical finger. Same-key repeats excluded.",
    "Same-finger skipgram; endpoints are separated by one corpus character.",
    "Full scissors: adjacent different fingers, two rows apart. FSB = DFSB + CFSB. Display total, not an extra penalty.",
    "Adjacent fingers; shorter finger above longer finger; one row apart. Unchanged discordant half-scissor rule.",
    "Full-scissor skipgrams: FSS = DFSS + CFSS. Display total, not an extra penalty.",
    "Unchanged discordant half-scissor rule applied to skipgram endpoints.",
    "Same hand, different fingers; horizontal span exceeds finger separation.",
    "Lateral-stretch rule applied to skipgram endpoints.",
    "Discordant two-row change between non-adjacent fingers; excludes DFSB.",
    "Discordant two-row skipgram between non-adjacent fingers; excludes DFSS.",
    "Same-hand, no-thumb trigram whose finger direction reverses.",
    "Redirect with no index finger. Subset of RED, with an extra penalty.",
    "Redirect with an index and a ring/pinky. Subset of RED.",
    "No-thumb trigram repeats a finger, including same-key repeats; excludes WRED.",
    "Clean same-row adjacent-finger bigram. Same hand, no thumbs; lateral stretches receive no credit.",
    "Total directional rolls: IN2 + OUT2 + IN3 + OUT3. No repeated fingers. Thumb inclusion and movement filters follow [rolls] in layouter.conf.",
    "Discordant full-scissor bigram: adjacent fingers, shorter above longer, two rows apart.",
    "Concordant full-scissor bigram: adjacent fingers, longer above shorter, two rows apart. Still penalized.",
    "Discordant full-scissor skipgram; the same endpoint geometry as DFSB.",
    "Concordant full-scissor skipgram; the same endpoint geometry as CFSB.",
    "Clean hand alternation: LRL or RLR. The returning AC pair must use different fingers without a scissor, stretch or two-row change. No thumbs.",
    "Concordant two-row change between non-adjacent fingers; excludes CFSB.",
    "Concordant two-row skipgram between non-adjacent fingers; excludes CFSS.",
    "Main-finger distance from home, key units per 100 supported presses. Not a physical trajectory.",
    "Vertical distance from home, key units per 100 supported presses.",
    "Lateral distance from home, key units per 100 supported presses.",
    "Consecutive same-finger distance, key units per 100 supported bigrams.",
    "Inward two-finger roll within a mixed-hand trigram (LLR/RRL/LRR/RLL). Uses [rolls] settings. Display-only part of ROLL.",
    "Outward two-finger roll within a mixed-hand trigram (LLR/RRL/LRR/RLL). Uses [rolls] settings. Display-only part of ROLL.",
    "Three distinct fingers on one hand moving strictly inward. Uses [rolls] settings. Display-only part of ROLL.",
    "Three distinct fingers on one hand moving strictly outward. Uses [rolls] settings. Display-only part of ROLL.",
    "All inward rolls: IN2 + IN3. Percentage of eligible trigrams under [rolls]; display-only part of ROLL.",
    "All outward rolls: OUT2 + OUT3. Percentage of eligible trigrams under [rolls]; display-only part of ROLL.",
];

fn physical_metric(m: usize) -> bool {
    matches!(m, TRAVEL|VTRAVEL|LTRAVEL|SFTRAVEL)
}

fn metric_unit(m: usize) -> &'static str {
    if physical_metric(m) {
        "u/100"
    } else {
        "%"
    }
}

fn bit(m: usize) -> u64 {
    1u64 << m
}

fn is_skip(m: usize) -> bool {
    matches!(m, SFS|FSS|HSS|LSS|DSS|DFSS|CFSS|CSS)
}

fn is_rhythm(m: usize) -> bool {
    matches!(
        m,
        REDIR | WRED | WISH | OSF | ROLL | ALT | IN2 | OUT2 | IN3 | OUT3 | INROLL | OUTROLL
    )
}

fn higher_better(m: usize) -> bool {
    matches!(m, SRAF | ROLL | ALT | IN2 | OUT2 | IN3 | OUT3 | INROLL | OUTROLL)
}

fn aggregate(m: usize) -> bool {
    matches!(m, FSB | FSS | IN2 | OUT2 | IN3 | OUT3 | INROLL | OUTROLL)
}

fn raw_positive(m: usize) -> Option<usize> {
    match m {
        SRAF => Some(RAW_SRAF),
        ALT => Some(RAW_ALT),
        _ => None
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
        '␠'
    } else {
        b as char
    }
}

fn clean_text(s: &str) -> String {
    s.chars().map(|c| if c.is_control() {
        '?'
    } else {
        c
    }).collect()
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
    main: bool
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

    fn home(self) -> bool {
        self.main && self.row == 1 && matches!(self.col, 0|1|2|3|6|7|8|9)
    }
}

fn main_key_at(row: usize, col: i8) -> Key {
    let (finger, rank, hand) = match col {
        -1|0 => (0, 0, 0),
        1 => (1, 1, 0),
        2 => (2, 2, 0),
        3|4 => (3, 3, 0),
        5|6 => (4, 3, 1),
        7 => (5, 2, 1),
        8 => (6, 1, 1),
        9|10|11 => (7, 0, 1),
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
        main: true
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
        main: false
    }
}

fn same_hand(a: Key, b: Key) -> bool {
    a.main && b.main && a.hand == b.hand
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowMotion {
    None,
    Discordant,
    Concordant
}

fn row_motion(a: Key, b: Key) -> RowMotion {
    if !same_hand(a, b) || a.finger == b.finger || a.row == b.row {
        return RowMotion::None;
    }
    let (upper, lower) = if a.row<b.row {
        (a, b)
    } else {
        (b, a)
    };
    if upper.length()<lower.length() {
        RowMotion::Discordant
    }
    else if upper.length()>lower.length() {
        RowMotion::Concordant
    }
    else {
        RowMotion::None
    }
}

fn scissor_kind(a: Key, b: Key) -> u8 {
    if !same_hand(a, b) ||(a.rank - b.rank).abs() != 1 {
        return 0;
    }
    match (a.row - b.row).abs() {
        2 => 2,
        // Both orientations; the children preserve the D/C distinction.
        1 if row_motion(a, b) == RowMotion::Discordant => 1,
        _ => 0,
    }
}

fn is_lateral_stretch(a: Key, b: Key) -> bool {
    let df = (a.rank - b.rank).abs();
    same_hand(a, b) && df >= 1 &&a.horizontal_delta(b).abs() >= (df + 1) as f64
}

fn is_diagonal_stretch(a: Key, b: Key) -> bool {
    (a.row - b.row).abs() == 2 &&(a.rank - b.rank).abs() != 1 && row_motion(a, b) == RowMotion::Discordant
}

fn is_concordant_jump(a: Key, b: Key) -> bool {
    (a.row - b.row).abs() == 2 &&(a.rank - b.rank).abs() != 1 && row_motion(a, b) == RowMotion::Concordant
}

fn sraf_shape(a: Key, b: Key) -> bool {
    same_hand(a, b) && a.row == b.row &&(a.rank - b.rank).abs() == 1
}

fn is_redirect(a: Key, b: Key, c: Key) -> bool {
    if !(same_hand(a, b) && same_hand(b, c)) {
        return false;
    }
    let (d1, d2) = (b.rank - a.rank, c.rank - b.rank);
    d1 != 0 && d2 != 0 && d1.signum() != d2.signum()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RollSettings {
    include_thumbs: bool,
    include_scissors: bool,
    include_stretches: bool,
}

impl Default for RollSettings {
    fn default() -> Self {
        Self {
            include_thumbs: false,
            include_scissors: true,
            include_stretches: true,
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
        let (from, to) = if a.hand == b.hand {
            (a, b)
        } else {
            (b, c)
        };
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
    bi: u64,
    sk: u64,
    main: bool
}

// Local movement events veto SRAF and ALT credit, but not basic rolls.
// Off-home usage is an aggregate load penalty, not a local movement veto.
const BAD_BI: u64 = (1<<SFB)|(1<<FSB)|(1<<DFSB)|(1<<CFSB)|(1<<HSB)|(1<<LSB)|(1<<DSB)|(1<<CSB);

const BAD_SK: u64 = (1<<SFS)|(1<<FSS)|(1<<DFSS)|(1<<CFSS)|(1<<HSS)|(1<<LSS)|(1<<DSS)|(1<<CSS);

const BAD_TRI: u64 = (1<<REDIR)|(1<<WRED)|(1<<WISH)|(1<<OSF);

fn pair_flags(a: Key, b: Key, same_key: bool) -> PairFlags {
    let mut f = PairFlags {
        main: a.main && b.main,
        ..PairFlags::default()
    };
    if !same_key && a.finger == b.finger {
        f.bi|=bit(SFB);
        f.sk|=bit(SFS);
    }
    match scissor_kind(a, b) {
        1 => {
            f.bi|=bit(HSB);
            f.sk|=bit(HSS);
        },
        2 => {
            f.bi|=bit(FSB);
            f.sk|=bit(FSS);
            match row_motion(a, b) {
                RowMotion::Discordant => {
                    f.bi|=bit(DFSB);
                    f.sk|=bit(DFSS);
                },
                RowMotion::Concordant => {
                    f.bi|=bit(CFSB);
                    f.sk|=bit(CFSS);
                },
                RowMotion::None => {
                },
            }
        },
        _ => {
        },
    }
    if is_lateral_stretch(a, b) {
        f.bi|=bit(LSB);
        f.sk|=bit(LSS);
    }
    if is_diagonal_stretch(a, b) {
        f.bi|=bit(DSB);
        f.sk|=bit(DSS);
    }
    if is_concordant_jump(a, b) {
        f.bi|=bit(CSB);
        f.sk|=bit(CSS);
    }
    if same_hand(a, b) && a.finger != b.finger {
        match (a.row - b.row).abs() {
            1 => {
                f.bi|=bit(ROW1_BI);
                f.sk|=bit(ROW1_SK);
            },
            2 => {
                f.bi|=bit(ROW2_BI);
                f.sk|=bit(ROW2_SK);
            },
            _ => {
            }
        }
    }
    if sraf_shape(a, b) {
        f.bi|=bit(RAW_SRAF);
        if f.bi&BAD_BI == 0 {
            f.bi|=bit(SRAF);
        }
    }
    f
}

#[derive(Clone, Copy, Debug, Default)]
struct TriFlags {
    bits: u64,
    main: bool
}

fn trigram_penalties(a: Key, b: Key, c: Key) -> u64 {
    if !(a.main && b.main && c.main) {
        return 0;
    }
    let red = is_redirect(a, b, c);
    let weak = red && !(a.index() || b.index() || c.index());
    let wish = red && !weak &&(a.weak() || b.weak() || c.weak());
    let repeated = a.finger == b.finger || b.finger == c.finger || a.finger == c.finger;
    let mut bits = 0;
    if red {
        bits|=bit(REDIR);
    }
    if weak {
        bits|=bit(WRED);
    }
    if wish {
        bits|=bit(WISH);
    }
    if repeated && !weak {
        bits|=bit(OSF);
    }
    bits
}

fn triple_blockers(a: Key, b: Key, c: Key, ab: PairFlags, bc: PairFlags, ac: PairFlags) -> u64 {
    (ab.bi&BAD_BI)|(bc.bi&BAD_BI)|(ac.sk&BAD_SK)|(trigram_penalties(a, b, c)&BAD_TRI)
}

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
        // Filter consecutive pairs only. FSB covers both full-scissor directions;
        // HSB is the existing half-scissor definition. Skip AC is not a veto.
        let pairs = ab.bi | bc.bi;
        let blocked_scissors = !settings.include_scissors && pairs & (bit(FSB) | bit(HSB)) != 0;
        let blocked_stretches = !settings.include_stretches && pairs & bit(LSB) != 0;
        if !blocked_scissors && !blocked_stretches {
            let direction = if matches!(kind, IN2 | IN3) {
                INROLL
            } else {
                OUTROLL
            };
            bits |= bit(kind) | bit(direction) | bit(ROLL) | bit(RAW_ROLL) | bit(SIMPLE_ROLL);
        }
    }

    // Thumb inclusion is specific to rolls. Preserve other rhythm metrics and
    // their non-thumb denominator, including ALT's existing skip-pair veto.
    if !main {
        return TriFlags { bits, main };
    }
    bits |= trigram_penalties(a, b, c);
    let ac = pair_flags(a, c, a.row == c.row && a.col == c.col);
    if alternation_shape(a, b, c) {
        bits |= bit(RAW_ALT);
        if triple_blockers(a, b, c, ab, bc, ac) == 0 {
            bits |= bit(ALT);
        }
    }

    TriFlags { bits, main }
}

fn is_sraf(a: Key, b: Key) -> bool {
    pair_flags(a, b, false).bi&bit(SRAF) != 0
}

fn is_roll(a: Key, b: Key, c: Key) -> bool {
    tri_flags(a, b, c).bits&bit(ROLL) != 0
}

fn is_alternation(a: Key, b: Key, c: Key) -> bool {
    tri_flags(a, b, c).bits&bit(ALT) != 0
}

fn home_travel(k: Key) -> [f64; 3] {
    if !k.main {
        return [0.0; 3];
    }
    let homes = [0, 1, 2, 3, 6, 7, 8, 9];
    let y = (k.row - 1).abs() as f64;
    let x = ((k.col - homes[k.finger]) as f64 + k.row_offset as f64 / 1000.0).abs();
    [x.hypot(y), y, x]
}

fn home_travel_for(k: Key, keys: &[Key]) -> [f64; 3] {
    if !k.main {
        return [0.0; 3];
    }
    let column = [0, 1, 2, 3, 6, 7, 8, 9][k.finger];
    let home = keys.iter().find(|home| {
        home.main && home.row == 1 && home.finger == k.finger && home.col == column
    }).or_else(|| keys.iter().find(|home| {
        home.main && home.row == 1 && home.finger == k.finger
    })).or_else(|| keys.iter().find(|home| {
        home.main && home.row == 1 && home.col == column
    }));
    // If the canonical home slot is also absent, reconstruct its coordinate
    // from the row/column offsets. Uniform translations must still cancel.
    let fallback = Key {
        row: 1,
        col: column,
        row_offset: keys.iter().find(|key| key.main && key.row == 1).map_or(0, |key| key.row_offset),
        column_offset: keys.iter().find(|key| key.main && key.col == column).map_or(0, |key| key.column_offset),
        ..k
    };
    let home = home.copied().unwrap_or(fallback);
    let x = k.horizontal_delta(home).abs();
    let y = k.vertical_delta(home).abs();
    [x.hypot(y), y, x]
}

#[derive(Clone)]
struct Geometry {
    rolls: RollSettings,
    keys: Vec<Key>,
    pair: Vec<PairFlags>,
    tri: Vec<TriFlags>,
    n: usize,
    home: Vec<[f64; 3]>,
    sf_distance: Vec<f64>
}

impl Geometry {
    fn new(keys: Vec<Key>) -> Self {
        Self::with_rolls(keys, RollSettings::default())
    }

    fn with_rolls(keys: Vec<Key>, rolls: RollSettings) -> Self {
        let n = keys.len();
        let mut pair = vec![PairFlags::default(); n*n];
        let mut tri = vec![TriFlags::default(); n*n*n];
        for a in 0..n {
            for b in 0..n {
                pair[a*n + b] = pair_flags(keys[a], keys[b], a == b);
                for c in 0..n {
                    tri[(a*n + b)*n + c] = tri_flags_with_settings(keys[a], keys[b], keys[c], rolls);
                }
            }
        }
        let home = keys.iter().copied().map(|key| home_travel_for(key, &keys)).collect();
        let mut sf_distance = vec![0.0; n*n];
        for a in 0..n {
            for b in 0..n {
                let (x, y) = (keys[a], keys[b]);
                if x.main && y.main && x.finger == y.finger {
                    sf_distance[a*n + b] = x.vertical_delta(y).hypot(x.horizontal_delta(y));
                }
            }
        }
        Self {
            rolls,
            keys,
            pair,
            tri,
            n,
            home,
            sf_distance
        }
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
    if token.eq_ignore_ascii_case("blank") || matches!(token, "~"|"·") {
        *empty += 1;
        if *empty >= 32 {
            return Err("too many blank slots".into());
        }
        return Ok(*empty);
    }
    if token.eq_ignore_ascii_case("space") || token == "␠" {
        return Ok(b' ');
    }
    let bytes = token.as_bytes();
    if bytes.len() != 1 || !bytes[0].is_ascii_graphic() {
        return Err(format!("key {token:?}: expected one printable ASCII character, ~, or space").into());
    }
    Ok(bytes[0].to_ascii_lowercase())
}

fn parse_row(line: &str, row: usize, empty: &mut u8) -> AppResult<(Vec<(u8, i8)>,(usize, usize))> {
    let tokens: Vec<_> = line.split_whitespace().collect();
    // Legacy 20-character rows with two spaces between hands. Do not trim
    // leading whitespace: it encodes an empty leftmost physical slot.
    const SLOTS: [usize; 10] = [0, 2, 4, 6, 8, 11, 13, 15, 17, 19];
    let bytes = line.as_bytes();
    if tokens.len()<10 && !line.contains('|') && bytes.len() <= 20 && bytes.iter().all(|b| b.is_ascii()) &&
    bytes.iter().enumerate().all(|(i, b)| SLOTS.contains(&i) || *b == b' ') {
        let mut result = Vec::new();
        for (col, &p) in SLOTS.iter().enumerate() {
            let b = bytes.get(p).copied().unwrap_or(b' ');
            result.push((if b == b' ' {
                parse_token("~", empty)?
            } else {
                parse_token(&(b as char).to_string(), empty)?
            }, col as i8));
        }
        return Ok((result,(5, 5)));
    }
    let separator = tokens.iter().position(|t|*t == "|");
    let (left, right) = if let Some(i) = separator {
        if tokens.iter().filter(|t|**t == "|").count() != 1 {
            return Err(format!("row {} has more than one hand separator", row + 1).into());
        }
        (&tokens[..i], &tokens[i + 1..])
    } else {
        let n = match tokens.len() {
            10 => 5,
            12 => 6,
            _ => return Err(format!("row {}: use 10 or 12 slots, or separate an 11-slot row as 6|5 or 5|6", row + 1).into())
        };
        (&tokens[..n], &tokens[n..])
    };
    if !(5..=6).contains(&left.len())||!(5..=6).contains(&right.len()) {
        return Err(format!("row {}: each hand needs five or six single-character slots", row + 1).into());
    }
    let mut result = Vec::new();
    let start = if left.len() == 6 {
        -1
    } else {
        0
    };
    for (i, t) in left.iter().enumerate() {
        result.push((parse_token(t, empty)?, start + i as i8));
    }
    for (i, t) in right.iter().enumerate() {
        result.push((parse_token(t, empty)?, 5 + i as i8));
    }
    Ok((result,(left.len(), right.len())))
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
    let mut lines: Vec<_> = text.lines().map(|s| s.strip_suffix('\r').unwrap_or(s)).collect();
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
    let mut thumbs: [Option<u8>; 2] = [None, None];
    let mut absent = [false; 2];
    let mut stagger_mode = None;
    for line in &lines[3..] {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
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
        let named = trimmed.get(..7).is_some_and(|s|s.eq_ignore_ascii_case("thumbs:"));
        let content = if named {
            &trimmed[7..]
        } else {
            trimmed
        };
        let tokens: Vec<_> = content.split_whitespace().collect();
        if tokens.is_empty() || tokens.len()>2 {
            return Err("thumb line needs one key, or explicit left and right keys".into());
        }
        for (i, &token) in tokens.iter().enumerate() {
            // A bare, single thumb keeps its visual hand: columns 0..=10 are
            // left and columns 11 onward are right.
            let hand = if named || tokens.len() == 2 {
                i
            } else {
                usize::from(line.as_bytes().iter().take_while(|&&b|b == b' ').count()>10)
            };
            if thumbs[hand].is_some() || absent[hand] {
                return Err(format!("{} thumb is defined twice", if hand == 0 {
                    "left"
                } else {
                    "right"
                }).into());
            }
            let token = if token.len()>1 {
                token.strip_suffix(',').unwrap_or(token)
            } else {
                token
            };
            if token == "none" {
                absent[hand] = true;
            } else {
                thumbs[hand] = Some(parse_token(token, &mut empty)?);
            }
        }
    }
    if !symbols.contains(&b' ') && !thumbs.contains(&Some(b' ')) && !absent.contains(&true) {
        match (thumbs[0].is_some(), thumbs[1].is_some()) {
            (false, false) => thumbs[0] = Some(b' '),
            (true, false) => thumbs[1] = Some(b' '),
            (false, true) => thumbs[0] = Some(b' '),
            _ => {
            }
        }
    }
    for (hand, ch) in thumbs.into_iter().enumerate() {
        if let Some(ch) = ch {
            symbols.push(ch);
            keys.push(thumb_key(hand));
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
    let shape = std::array::from_fn(|row| keys.iter().filter(|key| key.main && key.row == row as i8).count());
    let minimum = keys.iter().filter(|key| key.main).map(|key| key.col).min().unwrap_or(0);
    let maximum = keys.iter().filter(|key| key.main).map(|key| key.col).max().unwrap_or(0);
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
    let result = board_from_text(&text, path).map_err(|e| format!("{}: {e}", path.display()).into());
    timing.mark("Layout parse");
    result
}

fn board_text(board: &Board, symbols: &[u8]) -> String {
    let token = |b| if blank(b) {
        "~".to_string()
    } else if b == b' ' {
        "space".to_string()
    } else if matches!(b, b'~' | b'|' | b'@') || b.is_ascii_uppercase() {
        format!("char:{}", b as char)
    } else {
        (b as char).to_string()
    };
    let mut out = String::new();
    for r in 0..3 {
        let mut row: Vec<_> = board.keys.iter().enumerate().filter(|(_, k)|k.main && k.row == r).collect();
        row.sort_by_key(|(_, k)|k.col);
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
    out.push_str(&crate::action_keys::geometry_text(board.keys.iter().filter(|key| key.main).map(|key| {
        (key.row, key.col, key.row_offset, key.column_offset, key.finger)
    }), board.row_stagger));
    let mut thumbs: Vec<_> = board.keys.iter().enumerate().filter(|(_, k)|!k.main).collect();
    thumbs.sort_by_key(|(_, k)|k.hand);
    let is_space=|(i, _): &(usize, &Key)|symbols[*i] == b' ';
    let bare = match thumbs.as_slice() {
        [s] if s.1.hand == 1||!is_space(s) => Some(*s),
        [a, b] if is_space(a)^is_space(b) => Some(if is_space(a) {
            *b
        } else {
            *a
        }),
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
    }
    else if !thumbs.is_empty() {
        out.push_str("thumbs:");
        for (i, _) in thumbs {
            out.push(' ');
            out.push_str(&token(symbols[i]));
        }
        out.push('\n');
    }
    out
}

fn default_locks(board: &Board) -> Vec<bool> {
    board.keys.iter().map(|k| k.home() || !k.main).collect()
}

fn timestamp() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos()
}

fn atomic_write(path: &Path, text: &str, backup: bool) -> AppResult<()> {
    if backup && path.exists() {
        let old = path.with_file_name(format!("{}.{}.bak", path.file_name().unwrap_or_default().to_string_lossy(), timestamp()));
        fs::copy(path, old)?;
    }
    let tmp = path.with_file_name(format!(".layouter-{}-{}.tmp", std::process::id(), timestamp()));
    let result = (|| -> AppResult<()> {
        let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        if let Ok(metadata) = fs::metadata(path) {
            f.set_permissions(metadata.permissions())?;
        }
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        fs::rename(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn board_as_action_layout(board: &Board, symbols: &[u8]) -> crate::action_keys::Result<crate::action_keys::Layout> {
    crate::action_keys::Layout::parse_dat(&board_text(board, symbols), &board.path)
}

fn save_new_layout(board: &Board, symbols: &[u8]) -> AppResult<PathBuf> {
    let dir = board.path.parent().unwrap_or(Path::new(LAYOUT_DIR));
    let stem = board.path.file_stem().and_then(|s| s.to_str()).unwrap_or("layout");
    let dat = board_text(board, symbols);
    let jsonc = crate::layout_export::jsonc_text(&board_as_action_layout(board, symbols)?)?;
    for n in 1..10000 {
        let path = dir.join(format!("{stem}-optimized-{n:03}.dat"));
        match crate::layout_export::save_pair(&path, &dat, &jsonc) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {},
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
            && path.file_name().and_then(|value| value.to_str()).unwrap_or("").starts_with(prefix)
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
#[derive(Clone,Debug)]
enum Json {
    Object(BTreeMap<String, Json>),
    Array(Vec<Json>),
    String(String),
    Number(f64),
    Bool(bool),
    Null
}

struct JsonParser<'a> {
    text: &'a str,
    p: usize
}

impl<'a> JsonParser<'a> {
    fn ws(&mut self) {
        while self.p<self.text.len() && matches!(self.text.as_bytes()[self.p], b' '|b'\n'|b'\r'|b'\t') {
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
        let h = self.text.as_bytes().get(self.p..end).ok_or("truncated Unicode escape")?;
        if !h.iter().all(|b| b.is_ascii_hexdigit()) {
            return Err("invalid Unicode escape".into());
        }
        let n = u32::from_str_radix(std::str::from_utf8(h)?, 16)?;
        self.p = end;
        Ok(n)
    }

    fn string(&mut self) -> AppResult<String> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            match self.byte().ok_or("unterminated JSON string")? {
                b'"' => {
                    self.p += 1;
                    return Ok(out);
                },
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
                                cp = 0x10000 +((cp - 0xd800)<<10) +(low - 0xdc00);
                            }
                            out.push(char::from_u32(cp).ok_or("invalid Unicode scalar")?);
                        },
                        _ => return Err("invalid JSON escape".into()),
                    }
                },
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
        if depth>64 {
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
            },
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
            },
            b'"' => Ok(Json::String(self.string()?)),
            b't'|b'f'|b'n' => {
                for (word, value) in [("true", Json::Bool(true)),("false", Json::Bool(false)),("null", Json::Null)] {
                    if self.text[self.p..].starts_with(word) {
                        self.p += word.len();
                        return Ok(value);
                    }
                }
                Err("invalid JSON literal".into())
            },
            b'-'|b'0'..=b'9' => {
                let start = self.p;
                if self.byte() == Some(b'-') {
                    self.p += 1;
                }
                if self.byte() == Some(b'0') {
                    self.p += 1;
                }
                else {
                    let p = self.p;
                    while self.byte().map(|b|b.is_ascii_digit()).unwrap_or(false) {
                        self.p += 1;
                    }
                    if self.p == p {
                        return Err("invalid number".into());
                    }
                }
                if self.byte() == Some(b'.') {
                    self.p += 1;
                    let p = self.p;
                    while self.byte().map(|b|b.is_ascii_digit()).unwrap_or(false) {
                        self.p += 1;
                    }
                    if self.p == p {
                        return Err("invalid fraction".into());
                    }
                }
                if matches!(self.byte(), Some(b'e')|Some(b'E')) {
                    self.p += 1;
                    if matches!(self.byte(), Some(b'+')|Some(b'-')) {
                        self.p += 1;
                    }
                    let p = self.p;
                    while self.byte().map(|b|b.is_ascii_digit()).unwrap_or(false) {
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
            },
            _ => Err(format!("unexpected JSON byte at {}", self.p).into()),
        }
    }
}

fn parse_json(text: &str) -> AppResult<Json> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut p = JsonParser {
        text,
        p: 0
    };
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
        let result = Self::from_text_with_limits(&text, &resolved, limits).map_err(|e|format!("{}: {e}", resolved.display()).into());
        timing.mark("JSON parse and frequency tables");
        result
    }
}

#[derive(Clone,Copy,Debug)]
struct Gram {
    ids: [usize; 3],
    len: usize,
    kind: usize,
    f: f64
}

#[derive(Clone)]
struct Corpus {
    name: String,
    grams: Vec<Gram>,
    by_symbol: Vec<Vec<usize>>,
    uni: Vec<f64>,
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
    fn new(board: Board) -> Self {
        Self::with_rolls(board, RollSettings::default())
    }

    fn with_rolls(board: Board, rolls: RollSettings) -> Self {
        let mut canonical = board.symbols.clone();
        canonical.sort_unstable();
        let original = board.symbols.iter().map(|s|canonical.iter().position(|c|c == s).unwrap()).collect();
        let geometry = Geometry::with_rolls(board.keys.clone(), rolls);
        Self {
            board,
            canonical,
            original,
            geometry
        }
    }

    fn symbols(&self, arr: &[usize]) -> Vec<u8> {
        arr.iter().map(|&id|self.canonical[id]).collect()
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
        let mut totals = [0.0; 4];
        let mut grams = Vec::new();
        let mut uni = vec![0.0; n];
        for kind in 0..4 {
            let mut merged: BTreeMap<Vec<usize>, f64> = BTreeMap::new();
            for (s, f) in &source.tables[kind] {
                if !s.is_ascii() {
                    continue;
                }
                let mapped: Vec<usize> = s.bytes().map(|b|ids[b as usize]).collect();
                if mapped.iter().any(|&i|i == usize::MAX) {
                    continue;
                }
                *merged.entry(mapped).or_default() += f;
            }
            for (v, f) in merged {
                if f == 0.0 {
                    continue;
                }
                totals[kind] += f;
                if kind == 0 {
                    uni[v[0]] += f;
                }
                let mut out = [0; 3];
                for (i, &id) in v.iter().enumerate() {
                    out[i] = id;
                }
                grams.push(Gram {
                    ids: out,
                    len: v.len(),
                    kind,
                    f
                });
            }
        }
        if totals[0] <= 0.0 || totals[1] <= 0.0 {
            return Err("layout has no usable letters/bigrams in this corpus".into());
        }
        let mut by_symbol = vec![Vec::new(); n];
        for (i, g) in grams.iter().enumerate() {
            for j in 0..g.len {
                if !g.ids[..j].contains(&g.ids[j]) {
                    by_symbol[g.ids[j]].push(i);
                }
            }
        }
        Ok(Corpus {
            name: source.name.clone(),
            grams,
            by_symbol,
            uni,
            totals,
            coverage: std::array::from_fn(|i|pct(totals[i], source.masses[i])),
            warnings: source.warnings.clone(),
            fingerprint: source.fingerprint
        })
    }
}

#[derive(Clone,Debug)]
struct Raw([f64; N_RAW]);

impl Default for Raw {
    fn default() -> Self {
        Self([0.0; N_RAW])
    }
}

fn positions(arr: &[usize]) -> Vec<usize> {
    let mut p = vec![0; arr.len()];
    for (i, &id) in arr.iter().enumerate() {
        p[id] = i;
    }
    p
}

fn add_bits(raw: &mut Raw, mut bits: u64, f: f64) {
    while bits != 0 {
        let bit = bits.trailing_zeros() as usize;
        raw.0[bit] += f;
        bits &= bits - 1;
    }
}

fn add_gram(raw: &mut Raw, g: &Gram, pos: &[usize], geometry: &Geometry, sign: f64) {
    let f = g.f*sign;
    let a = pos[g.ids[0]];
    match g.kind {
        0 => {
            let key = geometry.keys[a];
            raw.0[USAGE + key.finger] += f;
            if key.main && !key.home() {
                raw.0[OFF + key.finger] += f;
            }
            raw.0[TRAVEL] += geometry.home[a][0]*f;
            raw.0[VTRAVEL] += geometry.home[a][1]*f;
            raw.0[LTRAVEL] += geometry.home[a][2]*f;
        },
        1|2 => {
            let b = pos[g.ids[1]];
            let flags = geometry.pair[a*geometry.n + b];
            add_bits(raw, if g.kind == 1 {
                flags.bi
            } else {
                flags.sk
            }, f);
            if g.kind == 1 {
                if flags.main {
                    raw.0[SRAF_DEN] += f;
                }
                raw.0[SFTRAVEL] += geometry.sf_distance[a*geometry.n + b]*f;
            }
        },
        _ => {
            let b = pos[g.ids[1]];
            let c = pos[g.ids[2]];
            let flags = geometry.tri[(a*geometry.n + b)*geometry.n + c];
            add_bits(raw, flags.bits, f);
            if flags.main {
                raw.0[RHYTHM_DEN] += f;
            }
        },
    }
}

fn full_raw(arr: &[usize], c: &Corpus, g: &Geometry) -> Raw {
    let pos = positions(arr);
    let mut raw = Raw::default();
    for gram in &c.grams {
        add_gram(&mut raw, gram, &pos, g, 1.0);
    }
    raw
}

#[derive(Clone,Debug)]
struct Metrics {
    v: [f64; N_METRICS],
    usage: [f64; 10],
    off: [f64; 8],
    simple: [f64; 7]
}

fn denominator(m: usize, raw: &Raw, c: &Corpus) -> f64 {
    denominator_totals(m, raw, &c.totals)
}

fn denominator_totals(m: usize, raw: &Raw, totals: &[f64; 4]) -> f64 {
    if matches!(m, TRAVEL|VTRAVEL|LTRAVEL) {
        totals[0]
    } else if m == SRAF {
        raw.0[SRAF_DEN].max(0.0)
    } else if roll_metric(m) {
        raw.0[ROLL_DEN].max(0.0)
    } else if is_rhythm(m) {
        raw.0[RHYTHM_DEN].max(0.0)
    }
    else if is_skip(m) {
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
        v: std::array::from_fn(|m| {
            pct(raw.0[m].max(0.0), denominator_totals(m, raw, totals))
        }),
        usage: std::array::from_fn(|i| pct(raw.0[USAGE + i].max(0.0), totals[0])),
        off: std::array::from_fn(|i| pct(raw.0[OFF + i].max(0.0), totals[0])),
        simple: [
            pct(raw.0[SFB].max(0.0), totals[1]),
            pct(raw.0[SFS].max(0.0), totals[2]),
            pct((raw.0[LSB] + raw.0[LSS]).max(0.0), totals[1] + totals[2]),
            pct((raw.0[ROW1_BI] + raw.0[ROW1_SK]).max(0.0), totals[1] + totals[2]),
            pct((raw.0[ROW2_BI] + raw.0[ROW2_SK]).max(0.0), totals[1] + totals[2]),
            pct(raw.0[SRAF].max(0.0), raw.0[SRAF_DEN]),
            pct(raw.0[SIMPLE_ROLL].max(0.0), raw.0[ROLL_DEN]),
        ],
    }
}

// Keep roll policy beside the weights so evaluations and worker threads use
// the same immutable objective snapshot, without global configuration reads.
#[derive(Clone, Copy, Debug)]
struct Weights([f64; N_WEIGHTS], RollSettings);

impl Weights {
    fn new(values: [f64; N_WEIGHTS]) -> Self {
        Self(values, RollSettings::default())
    }

    fn rolls(&self) -> RollSettings {
        self.1
    }

    fn with_rolls(mut self, rolls: RollSettings) -> Self {
        self.1 = rolls;
        self
    }
}

impl Default for Weights {
    fn default() -> Self {
        Self::new([
                12.0, 1.5, 0.0, 1.0, 0.0, 0.4, 3.0, 0.75, 1.0, 0.25,
                0.75, 2.5, 1.25, 0.5, 0.25, 0.25,
                4.0, 2.0, 1.5, 0.75, 0.05, 0.50, 0.15,
                0.02, 0.0, 0.0, 0.10,
                0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
                0.60, 0.20, 0.05, 0.0,
            ])
    }
}

#[derive(Clone,Debug)]
struct Breakdown {
    penalty: f64,
    bonus: f64,
    net: f64,
    contributions: [f64; N_WEIGHTS]
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
            b.contributions[i] = m.v[i]*w.0[i]*if higher_better(i) {
                -1.0
            } else {
                1.0
            };
        }
    }
    for i in 0..4 {
        b.contributions[N_METRICS + i] = (m.off[i] + m.off[7 - i])*w.0[N_METRICS + i];
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

fn finite_nonnegative(s: &str) -> AppResult<f64> {
    let n: f64 = s.trim().parse()?;
    if !n.is_finite() || n<0.0 || n>1.0e6 {
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
        let (k, v) = l.split_once('=').ok_or_else(|| format!("line {}: expected name = value", i + 1))?;
        let k = k.trim().to_ascii_lowercase();
        if !seen.insert(k.clone()) {
            return Err(format!("duplicate setting {k}").into());
        }
        result.push((k, v.trim().to_string()));
    }
    Ok(result)
}

fn weights_from_text(text: &str) -> AppResult<Weights> {
    let entries: BTreeMap<String, String> = config_lines(text)?.into_iter().collect();
    let mut w = Weights::default();
    for (k, v) in &entries {
        let i = WEIGHT_NAMES.iter().position(|s|*s == k.as_str()).ok_or_else(|| format!("unknown weight {k}"))?;
        let value = finite_nonnegative(v)?;
        if !aggregate(i) {
            w.0[i] = value;
        }
    }
    // An old fsb/fss weight described only discordant full scissors. Preserve
    // that cost; seed the new concordant component at half the old value.
    // Explicit new keys win regardless of line order. Never weight the total.
    for (legacy, d, c) in [("fsb", DFSB, CFSB),("fss", DFSS, CFSS)] {
        if let Some(v) = entries.get(legacy) {
            let value = finite_nonnegative(v)?;
            if !entries.contains_key(WEIGHT_NAMES[d]) {
                w.0[d] = value;
            }
            if !entries.contains_key(WEIGHT_NAMES[c]) {
                w.0[c] = value*0.5;
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
    let mut s = String::from("# Percentages x weights; preferences are subtracted.\n# FSB/FSS are display totals. Other 2-row changes exclude full scissors.\n");
    for i in 0..N_WEIGHTS {
        if !aggregate(i) {
            s.push_str(&format!("{} = {}\n", WEIGHT_NAMES[i], w.0[i]));
        }
    }
    s
}

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
enum CreditView {
    Clean,
    Raw,
    Rejected
}

impl CreditView {
    fn next(self) -> Self {
        match self {
            Self::Clean => Self::Raw,
            Self::Raw => Self::Rejected,
            Self::Rejected => Self::Clean
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::Raw => "raw",
            Self::Rejected => "rejected"
        }
    }
}

fn contribution_mass(m: usize, raw: &Raw, view: CreditView) -> f64 {
    match raw_positive(m) {
        Some(r) => match view {
            CreditView::Raw => raw.0[r].max(0.0),
            CreditView::Rejected => (raw.0[r]-raw.0[m]).max(0.0),
            _ => raw.0[m].max(0.0)
        },
        None => raw.0[m].max(0.0),
    }
}

fn blocker_names(bits: u64) -> String {
    let mut names = Vec::new();
    // Show the D/C children instead of repeating the aggregate FSB/FSS label.
    for m in [SFB, SFS, DFSB, CFSB, DFSS, CFSS, HSB, HSS, LSB, LSS, DSB, DSS, CSB, CSS, REDIR, WRED, WISH, OSF] {
        if bits&bit(m) != 0 {
            names.push(METRIC_NAMES[m]);
        }
    }
    names.join("+")
}

fn reward_blockers(g: &Gram, pos: &[usize], geometry: &Geometry) -> u64 {
    let a = pos[g.ids[0]];
    let b = pos[g.ids[1]];
    if g.len == 2 {
        return geometry.pair[a*geometry.n + b].bi&BAD_BI;
    }
    let c = pos[g.ids[2]];
    triple_blockers(geometry.keys[a], geometry.keys[b], geometry.keys[c],
        geometry.pair[a*geometry.n + b], geometry.pair[b*geometry.n + c], geometry.pair[a*geometry.n + c])
}

#[derive(Clone,Debug)]
struct Contributor {
    gram: String,
    ids: Vec<usize>,
    before: f64,
    after: f64,
    why_before: u64,
    why_after: u64
}

fn gram_name(g: &Gram, canonical: &[u8]) -> String {
    let chars: Vec<String> = g.ids[..g.len].iter().map(|&id|display_symbol(canonical[id]).to_string()).collect();
    chars.join(if g.kind == 2 {
        "_"
    } else {
        ""
    })
}

fn contributor_data_mode(m: usize, before: &[usize], after: &[usize], c: &Corpus, model: &Model, merge_reverse: bool, view: CreditView) -> Vec<Contributor> {
    let p0 = positions(before);
    let p1 = positions(after);
    let r0 = full_raw(before, c, &model.geometry);
    let r1 = full_raw(after, c, &model.geometry);
    let d0 = denominator(m, &r0, c);
    let d1 = denominator(m, &r1, c);
    let mut map: BTreeMap<String, Contributor> = BTreeMap::new();
    for g in &c.grams {
        if g.kind != if matches!(m, TRAVEL|VTRAVEL|LTRAVEL) {
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
        if merge_reverse && g.len == 2 {
            let mut reverse=*g;
            reverse.ids.swap(0, 1);
            let rev = gram_name(&reverse, &model.canonical);
            if rev<key {
                key = rev;
            }
        }
        let item = map.entry(key.clone()).or_insert_with(|| Contributor {
            gram: key,
            ids: g.ids[..g.len].to_vec(),
            before: 0.0,
            after: 0.0,
            why_before: 0,
            why_after: 0
        });
        item.before += v0;
        item.after += v1;
        if view == CreditView::Rejected && raw_positive(m).is_some() {
            if v0>0.0 {
                item.why_before|=reward_blockers(g, &p0, &model.geometry);
            }
            if v1>0.0 {
                item.why_after|=reward_blockers(g, &p1, &model.geometry);
            }
        }
    }
    let mut result: Vec<_> = map.into_values().collect();
    result.sort_by(|a, b|b.after.total_cmp(&a.after).then_with(|| a.gram.cmp(&b.gram)));
    result
}

fn contributor_data(m: usize, before: &[usize], after: &[usize], c: &Corpus, model: &Model, merge_reverse: bool) -> Vec<Contributor> {
    contributor_data_mode(m, before, after, c, model, merge_reverse, CreditView::Clean)
}

fn simple_breakdown(m: &Metrics, w: &[f64; 7]) -> Breakdown {
    let mut b = Breakdown::default();
    for i in 0..7 {
        let v = m.simple[i]*w[i];
        if i >= 5 {
            b.bonus += v;
        } else {
            b.penalty += v;
        }
    }
    b.net = b.penalty - b.bonus;
    b
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

    const ROWS: &str = "q w e r t | y u i o p\na s d f g | h j k l ;\nz x c v b | n m , . /\nthumbs: space\n";

    fn parse(extra: &str) -> Board {
        board_from_text(&format!("{ROWS}{extra}"), Path::new("inline")).unwrap()
    }

    #[test]
    fn old_coordinates_keep_their_exact_distances() {
        for extra in ["", "row-stagger: standard\n", "row-stagger: anglemod\n", "row-stagger: nokwts\n", "row-stagger: meteorite\n"] {
            let board = parse(extra);
            let geometry = Geometry::new(board.keys.clone());
            for (index, key) in board.keys.iter().copied().enumerate() {
                let expected = home_travel(key);
                for axis in 0..3 {
                    assert_eq!(geometry.home[index][axis].to_bits(), expected[axis].to_bits());
                }
            }
            for (a, first) in board.keys.iter().enumerate() {
                for (b, second) in board.keys.iter().enumerate() {
                    if first.main && second.main && first.finger == second.finger {
                        let expected = ((first.row - second.row) as f64).hypot(first.horizontal_delta(*second));
                        assert_eq!(geometry.sf_distance[a * geometry.n + b].to_bits(), expected.to_bits());
                    }
                }
            }
        }
    }

    #[test]
    fn uniform_row_translation_cancels_from_travel_and_pair_distances() {
        let first = Geometry::new(parse("row-offsets: 0 0.25 0.75\n").keys);
        let shifted = Geometry::new(parse("row-offsets: -0.25 0 0.5\n").keys);
        for (a, b) in first.home.iter().flatten().zip(shifted.home.iter().flatten()) {
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
    fn column_offsets_use_actual_home_and_cross_column_same_finger_distance() {
        let board = parse("column-offsets: 0 -0.3 -0.4 -0.3 -0.2 -0.2 -0.3 -0.4 -0.3 0\n");
        let geometry = Geometry::new(board.keys.clone());
        // QWERTY W/S share their physical column, so vertical translation cancels.
        assert_eq!(geometry.home[1][0].to_bits(), 1.0f64.to_bits());
        assert_eq!(geometry.home[11][0].to_bits(), 0.0f64.to_bits());
        // R/T use the same index finger but have different column heights.
        assert_eq!(geometry.sf_distance[3 * geometry.n + 4].to_bits(), 0.1f64.hypot(1.0).to_bits());
        assert_eq!(geometry.home[4][0].to_bits(), 1.0f64.hypot(0.9).to_bits());

        let plain = parse("");
        for (a, b) in [(0, 21), (1, 22), (3, 20), (20, 3)] {
            assert_eq!(scissor_kind(board.keys[a], board.keys[b]), scissor_kind(plain.keys[a], plain.keys[b]));
            assert_eq!(row_motion(board.keys[a], board.keys[b]), row_motion(plain.keys[a], plain.keys[b]));
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
        let round = board_from_text(&board_text(&board, &board.symbols), Path::new("round")).unwrap();
        assert_eq!(round.keys, board.keys);
        assert_eq!(round.symbols, board.symbols);
        let round = crate::action_keys::Layout::parse(&action.text(), Path::new("round")).unwrap();
        assert_eq!(round.slots, action.slots);
    }

    #[test]
    fn geometry_validation_is_shared_and_subtraction_cannot_overflow() {
        for extra in ["row-offsets: NaN 0 0\n", "row-offsets: 0 0.0001 0\n", "row-offsets: 33 0 0\n", "row-offsets: 0 0\n", "column-offsets: 0 0\n", "fingermap: LP / LP / LP\n"] {
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
