//! Mana2's built-in stat definitions and configurable progressive score.
//!
//! `add_gram` accepts already-mapped physical slot IDs. It intentionally does
//! not use AKLER's ergonomic flags: Mana2 has its own coordinate comparisons,
//! finger ordering, stretch formula, scissors table, redirect, and roll rules.
//! `kind` follows `Gram.kind`: 0 monogram, 1 bigram, 2 skipgram, 3 trigram.
//! The attached finger-speed counters implement the Genkey-derived companion
//! metric shown beside Mana2 statistics by akl.gg; finger speed is not a native
//! Mana2 Go statistic. Scoring is deliberately limited to values visible in
//! that screenshot table, with a separately editable weight for every value.

use crate::Key;
use std::ops::{Add, AddAssign, Sub, SubAssign};

const N_RAW_STATS: usize = 48;
pub(crate) const N_STATS: usize = 68;

/// The first 48 IDs preserve Mana2's raw counter order. The final 20 expose
/// the per-finger Fspeed and Weighted columns shown by akl.gg.
pub(crate) const STAT_IDS: [&str; N_STATS] = [
    "finger-usage-LP",
    "finger-usage-LR",
    "finger-usage-LM",
    "finger-usage-LI",
    "finger-usage-LT",
    "finger-usage-RT",
    "finger-usage-RI",
    "finger-usage-RM",
    "finger-usage-RR",
    "finger-usage-RP",
    "offpinky",
    "pinkyringcurl",
    "sfb",
    "sfbw",
    "skb",
    "lsb",
    "vsb",
    "sfs",
    "sfsw",
    "sks",
    "lss",
    "vss",
    "alt",
    "altnothumbs",
    "altsfs",
    "altsfsnothumbs",
    "redirect",
    "redirectnothumbs",
    "redirectsfs",
    "redirectsfsnothumbs",
    "redirectweak",
    "redirectweaknothumbs",
    "redirectsfsweak",
    "redirectsfsweaknothumbs",
    "roll",
    "rollnothumbs",
    "inroll2",
    "inroll2nothumbs",
    "outroll2",
    "outroll2nothumbs",
    "inroll3",
    "inroll3nothumbs",
    "outroll3",
    "outroll3nothumbs",
    "goodroll",
    "goodrollnothumbs",
    // Genkey-derived companion values shown in akl.gg's Mana2 view.
    "fspeed",
    "weighted_speed",
    "fspeed-LP",
    "fspeed-LR",
    "fspeed-LM",
    "fspeed-LI",
    "fspeed-LT",
    "fspeed-RT",
    "fspeed-RI",
    "fspeed-RM",
    "fspeed-RR",
    "fspeed-RP",
    "weighted-speed-LP",
    "weighted-speed-LR",
    "weighted-speed-LM",
    "weighted-speed-LI",
    "weighted-speed-LT",
    "weighted-speed-RT",
    "weighted-speed-RI",
    "weighted-speed-RM",
    "weighted-speed-RR",
    "weighted-speed-RP",
];

/// Statistics configurable in `mana` and `aklgg` modes: akl.gg's Mana2 table
/// plus native Mana2 stats such as `offpinky` and `pinkyringcurl`.
pub(crate) const WEIGHTABLE_STATS: &[usize] = &[
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, // per-finger usage
    10, 11, // off-home pinky presses, pinky/ring curl
    12, 13, 14, 15, 16, // bigram Same Finger, weighted SFB, Same Key, Stretch, Scissor
    17, 18, 19, 20, 21, // skipgram Same Finger, weighted SFS, Same Key, Stretch, Scissor
    22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, // alternation/redirect
    34, 35, 36, 37, 38, 39, 40, 41, 42, 43, // rolls
    44, 45, // good rolls
    46, 47, // total Fspeed and Weighted
    48, 49, 50, 51, 52, 53, 54, 55, 56, 57, // per-finger Fspeed
    58, 59, 60, 61, 62, 63, 64, 65, 66, 67, // per-finger Weighted
];

pub(crate) fn is_weightable(index: usize) -> bool {
    WEIGHTABLE_STATS.contains(&index)
}

const MONOGRAM_STATS: usize = 12;
const BIGRAM_START: usize = MONOGRAM_STATS;
const SKIPGRAM_START: usize = BIGRAM_START + 5;
const TRIGRAM_START: usize = SKIPGRAM_START + 5;

const M_OFFPINKY: usize = 10;
const M_PINKYRINGCURL: usize = 11;

const B_SFB: usize = BIGRAM_START;
const B_SFBW: usize = BIGRAM_START + 1;
const B_SKB: usize = BIGRAM_START + 2;
const B_LSB: usize = BIGRAM_START + 3;
const B_VSB: usize = BIGRAM_START + 4;

const S_SFS: usize = SKIPGRAM_START;
const S_SFSW: usize = SKIPGRAM_START + 1;
const S_SKS: usize = SKIPGRAM_START + 2;
const S_LSS: usize = SKIPGRAM_START + 3;
const S_VSS: usize = SKIPGRAM_START + 4;

const T_ALT: usize = TRIGRAM_START;
const T_ALT_NO_THUMBS: usize = TRIGRAM_START + 1;
const T_ALT_SFS: usize = TRIGRAM_START + 2;
const T_ALT_SFS_NO_THUMBS: usize = TRIGRAM_START + 3;
const T_REDIRECT: usize = TRIGRAM_START + 4;
const T_REDIRECT_NO_THUMBS: usize = TRIGRAM_START + 5;
const T_REDIRECT_SFS: usize = TRIGRAM_START + 6;
const T_REDIRECT_SFS_NO_THUMBS: usize = TRIGRAM_START + 7;
const T_REDIRECT_WEAK: usize = TRIGRAM_START + 8;
const T_REDIRECT_WEAK_NO_THUMBS: usize = TRIGRAM_START + 9;
const T_REDIRECT_SFS_WEAK: usize = TRIGRAM_START + 10;
const T_REDIRECT_SFS_WEAK_NO_THUMBS: usize = TRIGRAM_START + 11;
const T_ROLL: usize = TRIGRAM_START + 12;
const T_ROLL_NO_THUMBS: usize = TRIGRAM_START + 13;
const T_INROLL2: usize = TRIGRAM_START + 14;
const T_INROLL2_NO_THUMBS: usize = TRIGRAM_START + 15;
const T_OUTROLL2: usize = TRIGRAM_START + 16;
const T_OUTROLL2_NO_THUMBS: usize = TRIGRAM_START + 17;
const T_INROLL3: usize = TRIGRAM_START + 18;
const T_INROLL3_NO_THUMBS: usize = TRIGRAM_START + 19;
const T_OUTROLL3: usize = TRIGRAM_START + 20;
const T_OUTROLL3_NO_THUMBS: usize = TRIGRAM_START + 21;
const T_GOODROLL: usize = TRIGRAM_START + 22;
const T_GOODROLL_NO_THUMBS: usize = TRIGRAM_START + 23;
const TOTAL_FSPEED: usize = 46;
const TOTAL_WEIGHTED_SPEED: usize = 47;
const FINGER_FSPEED_START: usize = 48;
const FINGER_WEIGHTED_SPEED_START: usize = 58;

// Fspeed uses AKLER's LP,LR,LM,LI,RI,RM,RR,RP,LT,RT order internally. The
// screenshot table and STAT_IDS use Mana2's LP,LR,LM,LI,LT,RT,RI,RM,RR,RP.
const MANA_TO_AKLER_FINGER: [usize; 10] = [0, 1, 2, 3, 8, 9, 4, 5, 6, 7];

#[derive(Clone, Copy, Debug)]
pub(crate) struct RawStats(
    pub(crate) [f64; N_RAW_STATS],
    pub(crate) crate::finger_speed::Raw,
);

impl Default for RawStats {
    fn default() -> Self {
        Self([0.0; N_RAW_STATS], crate::finger_speed::Raw::default())
    }
}

impl AddAssign for RawStats {
    fn add_assign(&mut self, rhs: Self) {
        for (left, right) in self.0.iter_mut().zip(rhs.0) {
            *left += right;
        }
        self.1 += rhs.1;
    }
}

impl Add for RawStats {
    type Output = Self;

    fn add(mut self, rhs: Self) -> Self::Output {
        self += rhs;
        self
    }
}

impl SubAssign for RawStats {
    fn sub_assign(&mut self, rhs: Self) {
        for (left, right) in self.0.iter_mut().zip(rhs.0) {
            *left -= right;
        }
        self.1 -= rhs.1;
    }
}

impl Sub for RawStats {
    type Output = Self;

    fn sub(mut self, rhs: Self) -> Self::Output {
        self -= rhs;
        self
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Stats {
    pub(crate) values: [f64; N_STATS],
}

impl Stats {
    #[cfg(test)]
    pub(crate) fn get(&self, id: &str) -> Option<f64> {
        STAT_IDS
            .iter()
            .position(|candidate| *candidate == id)
            .map(|index| self.values[index])
    }
}

/// Add one weighted physical n-gram to the stat sums. `signed_frequency` can
/// be negative for incremental swap proposals; totals are normalized separately
/// by [`stats`]. Invalid kind/length pairs indicate an internal call-site bug.
pub(crate) fn add_gram(
    raw: &mut RawStats,
    keys: &[Key],
    kind: usize,
    slot_ids: &[usize],
    signed_frequency: f64,
) {
    let expected_len = match kind {
        0 => 1,
        1 | 2 => 2,
        3 => 3,
        _ => panic!("invalid Mana2 n-gram kind {kind}"),
    };
    assert_eq!(slot_ids.len(), expected_len, "invalid Mana2 n-gram length");
    if signed_frequency == 0.0 {
        return;
    }

    match kind {
        0 => add_monogram(
            raw,
            PhysicalKey::from_key(keys[slot_ids[0]]),
            signed_frequency,
        ),
        1 | 2 => {
            let a = PhysicalKey::from_key(keys[slot_ids[0]]);
            let b = PhysicalKey::from_key(keys[slot_ids[1]]);
            add_pair(raw, &a, &b, signed_frequency, kind == 2);
            crate::finger_speed::accumulate(
                &mut raw.1,
                &keys[slot_ids[0]],
                &keys[slot_ids[1]],
                signed_frequency,
                kind == 2,
            );
        }
        3 => {
            let a = PhysicalKey::from_key(keys[slot_ids[0]]);
            let b = PhysicalKey::from_key(keys[slot_ids[1]]);
            let c = PhysicalKey::from_key(keys[slot_ids[2]]);
            add_trigram(raw, &a, &b, &c, signed_frequency);
        }
        _ => unreachable!(),
    }
}

/// Geometry-only Mana2 contributions for one fixed physical keyboard.
///
/// Optimizer swaps move bindings, not physical slots, so the monogram, pair,
/// and triple classifications below never change during a search. Compiling
/// them once avoids repeating Mana2's coordinate calculations for every
/// remapped corpus context. Frequencies remain outside this cache and are
/// applied in the same field order as [`add_gram`].
#[derive(Clone)]
pub(crate) struct GeometryContributions {
    unary: Vec<u16>,
    pair: Vec<PairContribution>,
    triple: Vec<u32>,
    max_abs: RawStats,
    n: usize,
    active: Option<ActiveContributions>,
}

/// Raw counters which can affect the configured objective.
///
/// Candidate trials may leave every other counter at its committed value. A
/// winning trial must still be replayed through the complete contribution path
/// before it is committed or displayed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ActiveStats {
    raw: u64,
    speed_fingers: u16,
}

impl ActiveStats {
    #[cfg(test)]
    pub(crate) fn from_weights(weights: &Weights) -> Self {
        let mut raw = 0;
        let mut speed_fingers = 0;
        for (index, schedule) in weights.0.iter().enumerate() {
            let active = schedule.iter().step_by(2).any(|slope| *slope != 0.0);
            if !active {
                continue;
            }
            if matches!(index, TOTAL_FSPEED | TOTAL_WEIGHTED_SPEED) {
                speed_fingers = (1 << 10) - 1;
            } else if (FINGER_FSPEED_START..FINGER_WEIGHTED_SPEED_START).contains(&index) {
                let mana_finger = index - FINGER_FSPEED_START;
                speed_fingers |= 1 << MANA_TO_AKLER_FINGER[mana_finger];
            } else if (FINGER_WEIGHTED_SPEED_START..N_STATS).contains(&index) {
                let mana_finger = index - FINGER_WEIGHTED_SPEED_START;
                speed_fingers |= 1 << MANA_TO_AKLER_FINGER[mana_finger];
            } else if index < TOTAL_FSPEED {
                raw |= 1_u64 << index;
            }
        }
        Self { raw, speed_fingers }
    }

    fn raw_mask(self, start: usize, width: usize) -> u64 {
        debug_assert!(start + width <= N_RAW_STATS);
        (self.raw >> start) & ((1_u64 << width) - 1)
    }

    /// Zero products can be omitted only after the caller's finite-projection
    /// guard and this check. Under round-to-nearest, finite additions cannot
    /// produce negative zero from an accumulator that is not negative zero.
    pub(crate) fn allows_sparse_zeros(self, raw: &RawStats) -> bool {
        let negative_zero = (-0.0_f64).to_bits();
        let mut fields = self.raw;
        while fields != 0 {
            let index = fields.trailing_zeros() as usize;
            if raw.0[index].to_bits() == negative_zero {
                return false;
            }
            fields &= fields - 1;
        }
        let mut fingers = self.speed_fingers;
        while fingers != 0 {
            let finger = fingers.trailing_zeros() as usize;
            if raw.1.bigrams[finger].to_bits() == negative_zero
                || raw.1.skipgrams[finger].to_bits() == negative_zero
            {
                return false;
            }
            fingers &= fingers - 1;
        }
        true
    }
}

#[derive(Clone, Copy)]
struct PairContribution {
    stats: [f64; 5],
    speed_finger: u8,
    speed: f64,
}

/// Compact objective-specific tables. Zero-valued active pair fields remain
/// present: adding their product is observable for signed zero and nonfinite
/// frequencies, so compilation only removes fields outside the objective.
#[derive(Clone)]
struct ActiveContributions {
    stats: ActiveStats,
    unary: Vec<u16>,
    triple: Vec<u32>,
    bigram: ActivePairs,
    skipgram: ActivePairs,
    speed_fingers: Vec<u8>,
    speed: Vec<f64>,
    sparse_pair: Vec<SparsePairContribution>,
    boolean_fields: u64,
}

#[derive(Clone)]
struct SparsePairContribution {
    stats: [f64; 5],
    bigram_fields: u8,
    skipgram_fields: u8,
    nonboolean_bigram_fields: u8,
    nonboolean_skipgram_fields: u8,
    speed_finger: u8,
    speed: f64,
}

/// Candidate-local integer replacement. Pair coefficients remain in the small
/// geometry table; only resolved pair indices and boolean differences are
/// memoized. The operation belongs to the geometry and objective that compiled
/// it and requires the same projection guards as integer replacement.
#[derive(Clone, Copy, Debug)]
pub(crate) struct IntegerReplacement {
    old_only: u64,
    new_only: u64,
    pairs: [u16; 4],
}

/// One ordered continuous update compiled for an immutable direct-tail plan.
#[derive(Clone, Copy, Debug)]
pub(crate) struct NumericUpdate {
    coefficient: f64,
    field: u8,
    subtract: bool,
}

impl NumericUpdate {
    #[inline(always)]
    pub(crate) fn apply_all(raw: &mut RawStats, updates: &[Self], frequency: f64) {
        if frequency == 0.0 {
            return;
        }
        for update in updates {
            let frequency = if update.subtract {
                -frequency
            } else {
                frequency
            };
            let field = update.field as usize;
            if field < N_RAW_STATS {
                raw.0[field] += update.coefficient * frequency;
            } else if field < N_RAW_STATS + 10 {
                raw.1.bigrams[field - N_RAW_STATS] += frequency * update.coefficient;
            } else {
                raw.1.skipgrams[field - N_RAW_STATS - 10] += frequency * update.coefficient;
            }
        }
    }
}

impl IntegerReplacement {
    #[inline(always)]
    pub(crate) fn is_empty(self) -> bool {
        self.old_only == 0 && self.new_only == 0 && self.pairs == [u16::MAX; 4]
    }

    #[inline(always)]
    pub(crate) fn has_boolean(self) -> bool {
        self.old_only | self.new_only != 0
    }

    #[inline(always)]
    pub(crate) fn has_nonboolean(self) -> bool {
        self.pairs != [u16::MAX; 4]
    }

    /// Certified integer frequencies may be summed before these disjoint
    /// counters are updated: the proposal budget keeps every integer exact.
    #[inline(always)]
    pub(crate) fn apply_boolean(self, raw: &mut RawStats, frequency: f64) {
        if frequency == 0.0 {
            return;
        }
        add_mask(&mut raw.0, self.old_only, 0, -frequency);
        add_mask(&mut raw.0, self.new_only, 0, frequency);
    }
}

impl SparsePairContribution {
    #[inline(always)]
    fn add(&self, raw: &mut RawStats, skip: bool, frequency: f64) {
        let fields = if skip {
            self.skipgram_fields
        } else {
            self.bigram_fields
        };
        self.add_fields(raw, fields, skip, frequency);
    }

    #[inline(always)]
    fn add_nonboolean(&self, raw: &mut RawStats, skip: bool, frequency: f64) {
        let fields = if skip {
            self.nonboolean_skipgram_fields
        } else {
            self.nonboolean_bigram_fields
        };
        self.add_fields(raw, fields, skip, frequency);
    }

    #[inline(always)]
    fn add_fields(&self, raw: &mut RawStats, mut fields: u8, skip: bool, frequency: f64) {
        let start = if skip { SKIPGRAM_START } else { BIGRAM_START };
        while fields != 0 {
            let offset = fields.trailing_zeros() as usize;
            raw.0[start + offset] += self.stats[offset] * frequency;
            fields &= fields - 1;
        }
        if self.speed_finger != u8::MAX {
            let speed = if skip {
                &mut raw.1.skipgrams
            } else {
                &mut raw.1.bigrams
            };
            speed[self.speed_finger as usize] += frequency * self.speed;
        }
    }
}

#[derive(Clone)]
struct ActivePairs {
    offsets: Vec<usize>,
    values: Vec<f64>,
}

impl ActiveContributions {
    #[inline(always)]
    fn tail_boolean_fields(&self, tail: [u8; 3], n: usize) -> u64 {
        let [a, b, c] = tail;
        if c == u8::MAX {
            return 0;
        }
        let c = c as usize;
        let mut fields = self.unary[c] as u64;
        if b != u8::MAX {
            let b = b as usize;
            let pair = &self.sparse_pair[b * n + c];
            fields |= (pair.bigram_fields as u64) << BIGRAM_START;
            if a != u8::MAX {
                let a = a as usize;
                let pair = &self.sparse_pair[a * n + c];
                fields |= (pair.skipgram_fields as u64) << SKIPGRAM_START;
                fields |= (self.triple[(a * n + b) * n + c] as u64) << TRIGRAM_START;
            }
        }
        fields & self.boolean_fields
    }

    #[inline(always)]
    fn add_tail_nonboolean(&self, raw: &mut RawStats, tail: [u8; 3], n: usize, frequency: f64) {
        let [a, b, c] = tail;
        if c == u8::MAX || b == u8::MAX {
            return;
        }
        let c = c as usize;
        self.sparse_pair[b as usize * n + c].add_nonboolean(raw, false, frequency);
        if a != u8::MAX {
            self.sparse_pair[a as usize * n + c].add_nonboolean(raw, true, frequency);
        }
    }

    #[cfg(test)]
    #[inline(always)]
    fn tail_nonboolean_is_empty(&self, tail: [u8; 3], n: usize) -> bool {
        let [a, b, c] = tail;
        if c == u8::MAX || b == u8::MAX {
            return true;
        }
        let c = c as usize;
        let bigram = &self.sparse_pair[b as usize * n + c];
        if bigram.nonboolean_bigram_fields != 0 || bigram.speed_finger != u8::MAX {
            return false;
        }
        if a != u8::MAX {
            let skipgram = &self.sparse_pair[a as usize * n + c];
            if skipgram.nonboolean_skipgram_fields != 0 || skipgram.speed_finger != u8::MAX {
                return false;
            }
        }
        true
    }

    #[inline(always)]
    fn add_unary(&self, raw: &mut RawStats, c: usize, frequency: f64) {
        if self.stats.raw_mask(0, MONOGRAM_STATS) != 0 {
            add_mask(&mut raw.0, self.unary[c] as u64, 0, frequency);
        }
    }

    #[inline(always)]
    fn add_pair(&self, raw: &mut RawStats, pair: usize, skip: bool, frequency: f64) {
        if skip {
            self.skipgram.add(raw, pair, SKIPGRAM_START, frequency);
        } else {
            self.bigram.add(raw, pair, BIGRAM_START, frequency);
        }
        if self.stats.speed_fingers != 0 {
            let finger = self.speed_fingers[pair];
            if finger != u8::MAX {
                let speed = if skip {
                    &mut raw.1.skipgrams
                } else {
                    &mut raw.1.bigrams
                };
                speed[finger as usize] += frequency * self.speed[pair];
            }
        }
    }
}

impl ActivePairs {
    #[cfg(test)]
    fn new(pair: &[PairContribution], active: ActiveStats, start: usize) -> Self {
        let offsets: Vec<_> = (0..5)
            .filter(|offset| active.raw & (1 << (start + offset)) != 0)
            .collect();
        let mut values = Vec::with_capacity(pair.len() * offsets.len());
        for contribution in pair {
            for &offset in &offsets {
                values.push(contribution.stats[offset]);
            }
        }
        Self { offsets, values }
    }

    #[inline(always)]
    fn add(&self, raw: &mut RawStats, pair: usize, start: usize, frequency: f64) {
        let begin = pair * self.offsets.len();
        for (index, &offset) in self.offsets.iter().enumerate() {
            raw.0[start + offset] += self.values[begin + index] * frequency;
        }
    }
}

impl GeometryContributions {
    /// `alts` applies `[alts]` rules within each pair and triple.
    pub(crate) fn new(keys: &[Key], alts: &crate::alt_fingers::Resolver) -> Self {
        assert!(MONOGRAM_STATS <= u16::BITS as usize);
        assert!(N_RAW_STATS - TRIGRAM_START <= u32::BITS as usize);
        let n = keys.len();
        let mut max_abs = RawStats::default();
        let mut unary = Vec::with_capacity(n);
        for slot in 0..n {
            let mut raw = RawStats::default();
            add_gram(&mut raw, keys, 0, &[slot], 1.0);
            let mut mask = 0;
            for (index, value) in raw.0[..MONOGRAM_STATS].iter().enumerate() {
                assert!(matches!(*value, 0.0 | 100.0));
                if *value != 0.0 {
                    mask |= 1 << index;
                }
                max_abs.0[index] = finite_max(max_abs.0[index], value.abs());
            }
            unary.push(mask);
        }

        let mut pair = Vec::with_capacity(n * n);
        for a in 0..n {
            for b in 0..n {
                let mut raw = RawStats::default();
                let local = alts.keys(keys, [a, b]);
                add_gram(&mut raw, &local, 1, &[0, 1], 1.0);
                let speed_finger = if local[0].finger == local[1].finger {
                    local[0].finger as u8
                } else {
                    u8::MAX
                };
                let speed = if speed_finger == u8::MAX {
                    0.0
                } else {
                    raw.1.bigrams[speed_finger as usize]
                };
                debug_assert!(raw
                    .1
                    .bigrams
                    .iter()
                    .enumerate()
                    .all(|(finger, value)| finger == speed_finger as usize || *value == 0.0));
                pair.push(PairContribution {
                    stats: std::array::from_fn(|offset| raw.0[BIGRAM_START + offset]),
                    speed_finger: speed_finger as u8,
                    speed,
                });
                for offset in 0..5 {
                    let value = raw.0[BIGRAM_START + offset].abs();
                    max_abs.0[BIGRAM_START + offset] =
                        finite_max(max_abs.0[BIGRAM_START + offset], value);
                    max_abs.0[SKIPGRAM_START + offset] =
                        finite_max(max_abs.0[SKIPGRAM_START + offset], value);
                }
                if speed_finger != u8::MAX {
                    let finger = speed_finger as usize;
                    max_abs.1.bigrams[finger] = finite_max(max_abs.1.bigrams[finger], speed.abs());
                    max_abs.1.skipgrams[finger] =
                        finite_max(max_abs.1.skipgrams[finger], speed.abs());
                }
            }
        }

        let mut triple = Vec::with_capacity(n * n * n);
        for a in 0..n {
            for b in 0..n {
                for c in 0..n {
                    let mut raw = RawStats::default();
                    let local = alts.keys(keys, [a, b, c]);
                    add_gram(&mut raw, &local, 3, &[0, 1, 2], 1.0);
                    let mut mask = 0;
                    for (offset, value) in raw.0[TRIGRAM_START..N_RAW_STATS].iter().enumerate() {
                        assert!(matches!(*value, 0.0 | 100.0));
                        if *value != 0.0 {
                            mask |= 1 << offset;
                        }
                        max_abs.0[TRIGRAM_START + offset] =
                            finite_max(max_abs.0[TRIGRAM_START + offset], value.abs());
                    }
                    triple.push(mask);
                }
            }
        }
        Self {
            unary,
            pair,
            triple,
            max_abs,
            n,
            active: None,
        }
    }

    /// Compile objective dependencies once before candidate evaluation.
    #[cfg(test)]
    pub(crate) fn prepare_active(&mut self, active: ActiveStats) {
        if self
            .active
            .as_ref()
            .is_some_and(|cache| cache.stats == active)
        {
            return;
        }
        let unary_mask = active.raw_mask(0, MONOGRAM_STATS) as u16;
        let triple_mask = active.raw_mask(TRIGRAM_START, N_RAW_STATS - TRIGRAM_START) as u32;
        let mut boolean_fields = unary_mask as u64 | ((triple_mask as u64) << TRIGRAM_START);
        for offset in 0..5 {
            if self
                .pair
                .iter()
                .all(|pair| matches!(pair.stats[offset], 0.0 | 100.0))
            {
                boolean_fields |= active.raw
                    & ((1 << (BIGRAM_START + offset)) | (1 << (SKIPGRAM_START + offset)));
            }
        }
        let speed_fingers = self
            .pair
            .iter()
            .map(|pair| {
                if pair.speed_finger != u8::MAX
                    && active.speed_fingers & (1 << pair.speed_finger) != 0
                {
                    pair.speed_finger
                } else {
                    u8::MAX
                }
            })
            .collect();
        let cache = ActiveContributions {
            stats: active,
            unary: self.unary.iter().map(|mask| mask & unary_mask).collect(),
            triple: self.triple.iter().map(|mask| mask & triple_mask).collect(),
            bigram: ActivePairs::new(&self.pair, active, BIGRAM_START),
            skipgram: ActivePairs::new(&self.pair, active, SKIPGRAM_START),
            speed_fingers,
            speed: self.pair.iter().map(|pair| pair.speed).collect(),
            sparse_pair: self
                .pair
                .iter()
                .map(|pair| {
                    let mut nonzero = 0_u8;
                    for (offset, &value) in pair.stats.iter().enumerate() {
                        if value != 0.0 {
                            nonzero |= 1 << offset;
                        }
                    }
                    let speed_finger = if pair.speed_finger != u8::MAX
                        && active.speed_fingers & (1 << pair.speed_finger) != 0
                        && pair.speed != 0.0
                    {
                        pair.speed_finger
                    } else {
                        u8::MAX
                    };
                    SparsePairContribution {
                        stats: pair.stats,
                        bigram_fields: nonzero & active.raw_mask(BIGRAM_START, 5) as u8,
                        skipgram_fields: nonzero & active.raw_mask(SKIPGRAM_START, 5) as u8,
                        nonboolean_bigram_fields: nonzero
                            & active.raw_mask(BIGRAM_START, 5) as u8
                            & !((boolean_fields >> BIGRAM_START) as u8),
                        nonboolean_skipgram_fields: nonzero
                            & active.raw_mask(SKIPGRAM_START, 5) as u8
                            & !((boolean_fields >> SKIPGRAM_START) as u8),
                        speed_finger,
                        speed: pair.speed,
                    }
                })
                .collect(),
            boolean_fields,
        };
        self.active = Some(cache);
    }

    /// Whether shared 100-valued contributions can be canceled before updates.
    /// The caller must also establish that every corpus frequency is an integer
    /// and apply the finite-projection and sparse-zero guards. The integer
    /// budget keeps each intermediate integer sum exactly representable.
    pub(crate) fn active_integer_is_exact(
        &self,
        raw: &RawStats,
        exact_frequency_mass: u64,
        active: ActiveStats,
    ) -> bool {
        let Some(cache) = self.active.as_ref().filter(|cache| cache.stats == active) else {
            return false;
        };
        const EXACT_INTEGER_LIMIT: u128 = 1_u128 << 53;
        let mut fields = cache.boolean_fields;
        while fields != 0 {
            let index = fields.trailing_zeros() as usize;
            let value = raw.0[index];
            if !value.is_finite()
                || value.fract() != 0.0
                || value.to_bits() == (-0.0_f64).to_bits()
                || value.abs() > EXACT_INTEGER_LIMIT as f64
                || value.abs() as u128 + 200 * exact_frequency_mass as u128 > EXACT_INTEGER_LIMIT
            {
                return false;
            }
            fields &= fields - 1;
        }
        true
    }

    /// Replace integer-frequency tails after `active_integer_is_exact` and
    /// the sparse-projection guards have succeeded for the proposal's baseline.
    #[inline(always)]
    pub(crate) fn replace_tail_active_integer(
        &self,
        raw: &mut RawStats,
        old: [u8; 3],
        new: [u8; 3],
        frequency: f64,
        active: ActiveStats,
    ) {
        if frequency == 0.0 {
            return;
        }
        if let Some(cache) = self.active.as_ref().filter(|cache| cache.stats == active) {
            let old_fields = cache.tail_boolean_fields(old, self.n);
            let new_fields = cache.tail_boolean_fields(new, self.n);
            add_mask(&mut raw.0, old_fields & !new_fields, 0, -frequency);
            add_mask(&mut raw.0, new_fields & !old_fields, 0, frequency);
            cache.add_tail_nonboolean(raw, old, self.n, -frequency);
            cache.add_tail_nonboolean(raw, new, self.n, frequency);
            return;
        }
        self.add_tail_active_sparse(raw, old, -frequency, active);
        self.add_tail_active_sparse(raw, new, frequency, active);
    }

    /// Whether a certified integer replacement leaves every raw counter alone.
    /// Callers must separately establish unchanged denominator membership and
    /// the integer/sparse projection guards before omitting numeric updates.
    #[cfg(test)]
    #[inline(always)]
    pub(crate) fn active_integer_replacement_is_empty(
        &self,
        old: [u8; 3],
        new: [u8; 3],
        active: ActiveStats,
    ) -> bool {
        let Some(cache) = self.active.as_ref().filter(|cache| cache.stats == active) else {
            return false;
        };
        cache.tail_nonboolean_is_empty(old, self.n)
            && cache.tail_nonboolean_is_empty(new, self.n)
            && cache.tail_boolean_fields(old, self.n) == cache.tail_boolean_fields(new, self.n)
    }

    /// Resolve a candidate's fixed old/new tails once for repeated frequencies.
    pub(crate) fn compile_active_integer_replacement(
        &self,
        old: [u8; 3],
        new: [u8; 3],
        active: ActiveStats,
    ) -> Option<IntegerReplacement> {
        let cache = self.active.as_ref().filter(|cache| cache.stats == active)?;
        // Packed physical tails reserve u8::MAX for missing slots. This also
        // leaves u16::MAX available as the missing pair sentinel.
        if self.n > u8::MAX as usize {
            return None;
        }
        let old_fields = cache.tail_boolean_fields(old, self.n);
        let new_fields = cache.tail_boolean_fields(new, self.n);
        let mut pairs = [u16::MAX; 4];
        for (tail, begin) in [(old, 0), (new, 2)] {
            let [a, b, c] = tail;
            if c == u8::MAX || b == u8::MAX {
                continue;
            }
            let bigram = b as usize * self.n + c as usize;
            let pair = &cache.sparse_pair[bigram];
            if pair.nonboolean_bigram_fields != 0 || pair.speed_finger != u8::MAX {
                pairs[begin] = bigram as u16;
            }
            if a != u8::MAX {
                let skipgram = a as usize * self.n + c as usize;
                let pair = &cache.sparse_pair[skipgram];
                if pair.nonboolean_skipgram_fields != 0 || pair.speed_finger != u8::MAX {
                    pairs[begin + 1] = skipgram as u16;
                }
            }
        }
        Some(IntegerReplacement {
            old_only: old_fields & !new_fields,
            new_only: new_fields & !old_fields,
            pairs,
        })
    }

    /// Apply an operation compiled by this geometry for its current objective.
    #[cfg(test)]
    #[inline(always)]
    pub(crate) fn apply_active_integer_replacement(
        &self,
        raw: &mut RawStats,
        operation: &IntegerReplacement,
        frequency: f64,
    ) {
        operation.apply_boolean(raw, frequency);
        self.apply_active_integer_replacement_nonboolean(raw, operation, frequency);
    }

    /// Append the same ordered continuous updates used by integer replacement.
    /// The resulting schedule belongs to this prepared geometry and objective.
    pub(crate) fn compile_integer_numeric_updates(
        &self,
        operation: &IntegerReplacement,
        updates: &mut Vec<NumericUpdate>,
    ) {
        let cache = self
            .active
            .as_ref()
            .expect("integer objective must be prepared");
        for (position, &pair_id) in operation.pairs.iter().enumerate() {
            if pair_id == u16::MAX {
                continue;
            }
            let pair = &cache.sparse_pair[pair_id as usize];
            let skip = position % 2 != 0;
            let subtract = position < 2;
            let start = if skip { SKIPGRAM_START } else { BIGRAM_START };
            let mut fields = if skip {
                pair.nonboolean_skipgram_fields
            } else {
                pair.nonboolean_bigram_fields
            };
            while fields != 0 {
                let offset = fields.trailing_zeros() as usize;
                fields &= fields - 1;
                updates.push(NumericUpdate {
                    coefficient: pair.stats[offset],
                    field: (start + offset) as u8,
                    subtract,
                });
            }
            if pair.speed_finger != u8::MAX {
                updates.push(NumericUpdate {
                    coefficient: pair.speed,
                    field: (N_RAW_STATS + if skip { 10 } else { 0 }) as u8 + pair.speed_finger,
                    subtract,
                });
            }
        }
    }

    /// Continuous counters retain their per-context removal/addition order.
    #[inline(always)]
    pub(crate) fn apply_active_integer_replacement_nonboolean(
        &self,
        raw: &mut RawStats,
        operation: &IntegerReplacement,
        frequency: f64,
    ) {
        if frequency == 0.0 {
            return;
        }
        let cache = self
            .active
            .as_ref()
            .expect("integer objective must be prepared");
        let [old_bigram, old_skipgram, new_bigram, new_skipgram] = operation.pairs;
        if old_bigram != u16::MAX {
            cache.sparse_pair[old_bigram as usize].add_nonboolean(raw, false, -frequency);
        }
        if old_skipgram != u16::MAX {
            cache.sparse_pair[old_skipgram as usize].add_nonboolean(raw, true, -frequency);
        }
        if new_bigram != u16::MAX {
            cache.sparse_pair[new_bigram as usize].add_nonboolean(raw, false, frequency);
        }
        if new_skipgram != u16::MAX {
            cache.sparse_pair[new_skipgram as usize].add_nonboolean(raw, true, frequency);
        }
    }

    /// Whether projected candidate updates are guaranteed to stay finite.
    ///
    /// `absolute_frequency_mass` is the sum of absolute weights for every
    /// context a candidate may replace. A replacement removes one history and
    /// adds another, hence the factor of two. The conservative margin also
    /// keeps finger speed's later `800 * raw` normalization and ten-finger
    /// sums finite. Positive candidate denominators must be at least half the
    /// smallest positive corpus frequency; callers must fall back to complete
    /// updates if rounding produces a smaller positive denominator. Callers
    /// with contextual fingers or combo usage must use the complete path.
    pub(crate) fn projection_is_finite(
        &self,
        raw: &RawStats,
        totals: [f64; 4],
        absolute_frequency_mass: f64,
        minimum_positive_frequency: f64,
    ) -> bool {
        if !absolute_frequency_mass.is_finite()
            || absolute_frequency_mass < 0.0
            || !minimum_positive_frequency.is_finite()
            || minimum_positive_frequency <= 0.0
        {
            return false;
        }
        let minimum_denominator = minimum_positive_frequency / 2.0;
        let limit = (f64::MAX / 16384.0) * minimum_denominator.min(1.0);
        if limit == 0.0
            || totals
                .iter()
                .any(|&total| total > 0.0 && total < minimum_denominator)
        {
            return false;
        }
        let bounded = |base: f64, per_context: f64| {
            base.is_finite()
                && per_context.is_finite()
                && base.abs() < limit
                && (per_context == 0.0
                    || absolute_frequency_mass <= (limit - base.abs()) / (2.0 * per_context))
        };
        raw.0
            .iter()
            .zip(self.max_abs.0)
            .all(|(&base, maximum)| bounded(base, maximum))
            && raw
                .1
                .bigrams
                .iter()
                .zip(self.max_abs.1.bigrams)
                .all(|(&base, maximum)| bounded(base, maximum))
            && raw
                .1
                .skipgrams
                .iter()
                .zip(self.max_abs.1.skipgrams)
                .all(|(&base, maximum)| bounded(base, maximum))
            && totals.into_iter().all(|base| bounded(base, 1.0))
    }

    /// Add a one-, two-, or three-press terminal history. For three presses,
    /// the first/last pair is the skipgram, matching `add_tail_mana2`.
    #[cfg(test)]
    pub(crate) fn add_history(&self, raw: &mut RawStats, slots: &[usize], signed_frequency: f64) {
        assert!((1..=3).contains(&slots.len()));
        assert!(slots.iter().all(|slot| *slot < self.n));
        match slots {
            [c] => self.add_one(raw, *c, signed_frequency),
            [b, c] => self.add_two(raw, *b, *c, signed_frequency),
            [a, b, c] => self.add_three(raw, *a, *b, *c, signed_frequency),
            _ => unreachable!(),
        }
    }

    /// Cached equivalent of the free [`add_gram`] for one n-gram on this
    /// geometry: every field receives the same product in the same order, so
    /// results are bit-identical. The ordinary optimizer calls this for each
    /// affected gram instead of reclassifying the keys every trial.
    #[inline(always)]
    pub(crate) fn add_gram(
        &self,
        raw: &mut RawStats,
        kind: usize,
        slot_ids: &[usize],
        signed_frequency: f64,
    ) {
        if signed_frequency == 0.0 {
            return;
        }
        match kind {
            0 => add_mask(
                &mut raw.0,
                self.unary[slot_ids[0]] as u64,
                0,
                signed_frequency,
            ),
            1 | 2 => self.add_pair(raw, slot_ids[0], slot_ids[1], kind == 2, signed_frequency),
            3 => {
                let index = (slot_ids[0] * self.n + slot_ids[1]) * self.n + slot_ids[2];
                add_mask(
                    &mut raw.0,
                    self.triple[index] as u64,
                    TRIGRAM_START,
                    signed_frequency,
                );
            }
            _ => panic!("invalid Mana2 n-gram kind {kind}"),
        }
    }

    /// Move one n-gram from `old_ids` to `new_ids`. Counting fields that are
    /// set at both positions would cancel, so only the differing fields are
    /// touched; pairs carry weighted values and are moved in full.
    #[inline(always)]
    pub(crate) fn replace_gram(
        &self,
        raw: &mut RawStats,
        kind: usize,
        old_ids: &[usize],
        new_ids: &[usize],
        frequency: f64,
    ) {
        if frequency == 0.0 {
            return;
        }
        let (old, new, start) = match kind {
            0 => (
                self.unary[old_ids[0]] as u64,
                self.unary[new_ids[0]] as u64,
                0,
            ),
            1 | 2 => {
                let skip = kind == 2;
                self.add_pair(raw, old_ids[0], old_ids[1], skip, -frequency);
                self.add_pair(raw, new_ids[0], new_ids[1], skip, frequency);
                return;
            }
            3 => {
                let index = |ids: &[usize]| (ids[0] * self.n + ids[1]) * self.n + ids[2];
                (
                    self.triple[index(old_ids)] as u64,
                    self.triple[index(new_ids)] as u64,
                    TRIGRAM_START,
                )
            }
            _ => panic!("invalid Mana2 n-gram kind {kind}"),
        };
        move_mask(&mut raw.0, old, new, start, frequency);
    }

    /// Fixed-length forms used by the action optimizer's packed terminal
    /// histories. Keeping length validation outside the candidate loop lets
    /// these small geometry-only updates inline without constructing slices.
    #[inline(always)]
    pub(crate) fn add_one(&self, raw: &mut RawStats, c: usize, signed_frequency: f64) {
        debug_assert!(c < self.n);
        if signed_frequency != 0.0 {
            add_mask(&mut raw.0, self.unary[c] as u64, 0, signed_frequency);
        }
    }

    #[inline(always)]
    pub(crate) fn add_one_active(
        &self,
        raw: &mut RawStats,
        c: usize,
        signed_frequency: f64,
        active: ActiveStats,
    ) {
        debug_assert!(c < self.n);
        if signed_frequency != 0.0 {
            if let Some(cache) = self.active.as_ref().filter(|cache| cache.stats == active) {
                cache.add_unary(raw, c, signed_frequency);
                return;
            }
            self.add_unary_active(raw, c, signed_frequency, active);
        }
    }

    #[inline(always)]
    pub(crate) fn add_two(&self, raw: &mut RawStats, b: usize, c: usize, signed_frequency: f64) {
        debug_assert!(b < self.n && c < self.n);
        if signed_frequency == 0.0 {
            return;
        }
        add_mask(&mut raw.0, self.unary[c] as u64, 0, signed_frequency);
        self.add_pair(raw, b, c, false, signed_frequency);
    }

    #[inline(always)]
    pub(crate) fn add_two_active(
        &self,
        raw: &mut RawStats,
        b: usize,
        c: usize,
        signed_frequency: f64,
        active: ActiveStats,
    ) {
        debug_assert!(b < self.n && c < self.n);
        if signed_frequency == 0.0 {
            return;
        }
        if let Some(cache) = self.active.as_ref().filter(|cache| cache.stats == active) {
            cache.add_unary(raw, c, signed_frequency);
            cache.add_pair(raw, b * self.n + c, false, signed_frequency);
            return;
        }
        self.add_unary_active(raw, c, signed_frequency, active);
        self.add_pair_active(raw, b, c, false, signed_frequency, active);
    }

    #[inline(always)]
    pub(crate) fn add_three(
        &self,
        raw: &mut RawStats,
        a: usize,
        b: usize,
        c: usize,
        signed_frequency: f64,
    ) {
        debug_assert!(a < self.n && b < self.n && c < self.n);
        if signed_frequency == 0.0 {
            return;
        }
        add_mask(&mut raw.0, self.unary[c] as u64, 0, signed_frequency);
        self.add_pair(raw, b, c, false, signed_frequency);
        self.add_pair(raw, a, c, true, signed_frequency);
        let index = (a * self.n + b) * self.n + c;
        add_mask(
            &mut raw.0,
            self.triple[index] as u64,
            TRIGRAM_START,
            signed_frequency,
        );
    }

    #[inline(always)]
    pub(crate) fn add_three_active(
        &self,
        raw: &mut RawStats,
        a: usize,
        b: usize,
        c: usize,
        signed_frequency: f64,
        active: ActiveStats,
    ) {
        debug_assert!(a < self.n && b < self.n && c < self.n);
        if signed_frequency == 0.0 {
            return;
        }
        if let Some(cache) = self.active.as_ref().filter(|cache| cache.stats == active) {
            cache.add_unary(raw, c, signed_frequency);
            cache.add_pair(raw, b * self.n + c, false, signed_frequency);
            cache.add_pair(raw, a * self.n + c, true, signed_frequency);
            if cache
                .stats
                .raw_mask(TRIGRAM_START, N_RAW_STATS - TRIGRAM_START)
                != 0
            {
                let index = (a * self.n + b) * self.n + c;
                add_mask(
                    &mut raw.0,
                    cache.triple[index] as u64,
                    TRIGRAM_START,
                    signed_frequency,
                );
            }
            return;
        }
        self.add_unary_active(raw, c, signed_frequency, active);
        self.add_pair_active(raw, b, c, false, signed_frequency, active);
        self.add_pair_active(raw, a, c, true, signed_frequency, active);
        let fields = active.raw_mask(TRIGRAM_START, N_RAW_STATS - TRIGRAM_START);
        if fields != 0 {
            let index = (a * self.n + b) * self.n + c;
            add_mask(
                &mut raw.0,
                self.triple[index] as u64 & fields,
                TRIGRAM_START,
                signed_frequency,
            );
        }
    }

    /// Sparse variants require a finite projection and
    /// `active.allows_sparse_zeros(raw)` before the proposal starts.
    /// Packed tails are left-padded with `u8::MAX`.
    #[inline(always)]
    pub(crate) fn add_tail_active_sparse(
        &self,
        raw: &mut RawStats,
        tail: [u8; 3],
        frequency: f64,
        active: ActiveStats,
    ) {
        let [a, b, c] = tail;
        if c == u8::MAX || frequency == 0.0 {
            return;
        }
        if b == u8::MAX {
            self.add_one_active_sparse(raw, c as usize, frequency, active);
        } else if a == u8::MAX {
            self.add_two_active_sparse(raw, b as usize, c as usize, frequency, active);
        } else {
            self.add_three_active_sparse(
                raw, a as usize, b as usize, c as usize, frequency, active,
            );
        }
    }

    #[inline(always)]
    pub(crate) fn add_one_active_sparse(
        &self,
        raw: &mut RawStats,
        c: usize,
        frequency: f64,
        active: ActiveStats,
    ) {
        // Unary masks already omit absent contributions on the dense path.
        self.add_one_active(raw, c, frequency, active);
    }

    #[inline(always)]
    pub(crate) fn add_two_active_sparse(
        &self,
        raw: &mut RawStats,
        b: usize,
        c: usize,
        frequency: f64,
        active: ActiveStats,
    ) {
        debug_assert!(b < self.n && c < self.n);
        if frequency == 0.0 {
            return;
        }
        if frequency.is_finite() {
            if let Some(cache) = self.active.as_ref().filter(|cache| cache.stats == active) {
                cache.add_unary(raw, c, frequency);
                cache.sparse_pair[b * self.n + c].add(raw, false, frequency);
                return;
            }
        }
        self.add_two_active(raw, b, c, frequency, active);
    }

    #[inline(always)]
    pub(crate) fn add_three_active_sparse(
        &self,
        raw: &mut RawStats,
        a: usize,
        b: usize,
        c: usize,
        frequency: f64,
        active: ActiveStats,
    ) {
        debug_assert!(a < self.n && b < self.n && c < self.n);
        if frequency == 0.0 {
            return;
        }
        if frequency.is_finite() {
            if let Some(cache) = self.active.as_ref().filter(|cache| cache.stats == active) {
                cache.add_unary(raw, c, frequency);
                cache.sparse_pair[b * self.n + c].add(raw, false, frequency);
                cache.sparse_pair[a * self.n + c].add(raw, true, frequency);
                if cache
                    .stats
                    .raw_mask(TRIGRAM_START, N_RAW_STATS - TRIGRAM_START)
                    != 0
                {
                    let index = (a * self.n + b) * self.n + c;
                    add_mask(
                        &mut raw.0,
                        cache.triple[index] as u64,
                        TRIGRAM_START,
                        frequency,
                    );
                }
                return;
            }
        }
        self.add_three_active(raw, a, b, c, frequency, active);
    }

    #[inline(always)]
    fn add_unary_active(
        &self,
        raw: &mut RawStats,
        c: usize,
        signed_frequency: f64,
        active: ActiveStats,
    ) {
        let fields = active.raw_mask(0, MONOGRAM_STATS);
        if fields != 0 {
            add_mask(
                &mut raw.0,
                self.unary[c] as u64 & fields,
                0,
                signed_frequency,
            );
        }
    }

    #[inline(always)]
    fn add_pair(&self, raw: &mut RawStats, a: usize, b: usize, skip: bool, signed_frequency: f64) {
        let contribution = &self.pair[a * self.n + b];
        let start = if skip { SKIPGRAM_START } else { BIGRAM_START };
        for (offset, &value) in contribution.stats.iter().enumerate() {
            raw.0[start + offset] += value * signed_frequency;
        }
        if contribution.speed_finger != u8::MAX {
            let speed = if skip {
                &mut raw.1.skipgrams
            } else {
                &mut raw.1.bigrams
            };
            speed[contribution.speed_finger as usize] += signed_frequency * contribution.speed;
        }
    }

    #[inline(always)]
    fn add_pair_active(
        &self,
        raw: &mut RawStats,
        a: usize,
        b: usize,
        skip: bool,
        signed_frequency: f64,
        active: ActiveStats,
    ) {
        let start = if skip { SKIPGRAM_START } else { BIGRAM_START };
        let mut fields = active.raw_mask(start, 5);
        if fields == 0 && active.speed_fingers == 0 {
            return;
        }
        let contribution = &self.pair[a * self.n + b];
        // Iterate only objective dependencies, retaining ascending field order
        // and even zero-valued additions for exact floating-point behavior.
        while fields != 0 {
            let offset = fields.trailing_zeros() as usize;
            raw.0[start + offset] += contribution.stats[offset] * signed_frequency;
            fields &= fields - 1;
        }
        if contribution.speed_finger != u8::MAX
            && active.speed_fingers & (1 << contribution.speed_finger) != 0
        {
            let speed = if skip {
                &mut raw.1.skipgrams
            } else {
                &mut raw.1.bigrams
            };
            speed[contribution.speed_finger as usize] += signed_frequency * contribution.speed;
        }
    }
}

// `f64::max` discards NaN, which would make the projection safety bound
// underestimate nonfinite geometry. Preserve that condition for the guard.
fn finite_max(left: f64, right: f64) -> f64 {
    if left.is_finite() && right.is_finite() {
        left.max(right)
    } else {
        f64::INFINITY
    }
}

/// Move `100 * frequency` from the fields flagged in `from` to those flagged
/// in `to`; fields flagged in both are untouched.
#[inline(always)]
fn move_mask(raw: &mut [f64; N_RAW_STATS], from: u64, to: u64, start: usize, frequency: f64) {
    let contribution = 100.0 * frequency;
    let signed = [-contribution, contribution];
    let mut changed = from ^ to;
    while changed != 0 {
        let offset = changed.trailing_zeros() as usize;
        raw[start + offset] += signed[(to >> offset) as usize & 1];
        changed &= changed - 1;
    }
}

#[inline(always)]
fn add_mask(raw: &mut [f64; N_RAW_STATS], mut mask: u64, start: usize, signed_frequency: f64) {
    let contribution = 100.0 * signed_frequency;
    while mask != 0 {
        let offset = mask.trailing_zeros() as usize;
        raw[start + offset] += contribution;
        mask &= mask - 1;
    }
}

/// Convert accumulated weighted contributions into Mana2's per-category
/// averages. The four totals are monogram, bigram, skipgram, trigram masses.
pub(crate) fn stats(raw: &RawStats, totals: [f64; 4]) -> Stats {
    stats_with_speed_totals(raw, totals, totals)
}

/// Normalize native Mana2 statistics and the akl.gg companion finger-speed
/// values with their respective populations. Mana2 excludes unavailable
/// layout symbols from its native denominators, while akl.gg finger speed uses
/// the complete retained bigram and skipgram source populations.
pub(crate) fn stats_with_speed_totals(
    raw: &RawStats,
    totals: [f64; 4],
    speed_totals: [f64; 4],
) -> Stats {
    let speed = crate::finger_speed::normalize(&raw.1, speed_totals);
    let calculated_speed =
        crate::finger_speed::calculate(&speed, &crate::finger_speed::Settings::default());
    Stats {
        values: std::array::from_fn(|index| {
            if index == TOTAL_FSPEED {
                return calculated_speed.per_finger.iter().sum();
            }
            if index == TOTAL_WEIGHTED_SPEED {
                return calculated_speed.weighted.iter().sum();
            }
            if let Some(mana_finger) = index.checked_sub(FINGER_FSPEED_START) {
                if mana_finger < 10 {
                    return calculated_speed.per_finger[MANA_TO_AKLER_FINGER[mana_finger]];
                }
            }
            if let Some(mana_finger) = index.checked_sub(FINGER_WEIGHTED_SPEED_START) {
                if mana_finger < 10 {
                    return calculated_speed.weighted[MANA_TO_AKLER_FINGER[mana_finger]];
                }
            }
            let denominator = if index < BIGRAM_START {
                totals[0]
            } else if index < SKIPGRAM_START {
                totals[1]
            } else if index < TRIGRAM_START {
                totals[2]
            } else {
                totals[3]
            };
            if denominator > 0.0 {
                raw.0[index] / denominator
            } else {
                0.0
            }
        }),
    }
}

/// Progressive slope schedules for screenshot-visible statistics, indexed by
/// [`STAT_IDS`]. Each schedule alternates slope and upper boundary, ending with
/// a slope. Internal raw counters always contribute zero.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Weights([Vec<f64>; N_STATS]);

impl Default for Weights {
    /// akl.gg's defaults: one speed control, the all-finger Weighted total.
    fn default() -> Self {
        Self(std::array::from_fn(|index| {
            vec![match index {
                B_SFB => -4.0,
                S_SFS => -1.0,
                B_LSB | B_VSB | T_REDIRECT | T_REDIRECT_WEAK => -1.0,
                S_LSS | S_VSS => -0.5,
                TOTAL_WEIGHTED_SPEED => -1.0,
                _ => 0.0,
            }]
        }))
    }
}

impl Weights {
    pub(crate) fn from_text(text: &str) -> crate::AppResult<Self> {
        let mut result = Self::default();
        let mut seen = std::collections::BTreeSet::new();

        for (line_index, line) in text.lines().enumerate() {
            let content = line.split('#').next().unwrap_or("").trim();
            if content.is_empty() {
                continue;
            }
            let (key, value) = content
                .split_once('=')
                .ok_or_else(|| format!("line {}: expected name = value", line_index + 1))?;
            let key = key.trim().to_ascii_lowercase();
            if !seen.insert(key.clone()) {
                return Err(format!("line {}: duplicate setting {key}", line_index + 1).into());
            }
            let index = STAT_IDS
                .iter()
                .position(|id| id.eq_ignore_ascii_case(&key))
                .ok_or_else(|| format!("line {}: unknown AKL.GG stat {key}", line_index + 1))?;
            if !is_weightable(index) {
                return Err(format!(
                    "line {}: {key} is an internal counter, not a configurable AKL.GG stat",
                    line_index + 1
                )
                .into());
            }
            let value = value.trim();
            let values = parse_schedule(value)
                .map_err(|error| format!("line {}: {key} {error}", line_index + 1))?;
            validate_schedule(&key, &values)
                .map_err(|error| format!("line {}: {error}", line_index + 1))?;
            result.0[index] = values;
        }
        Ok(result)
    }

    pub(crate) fn schedule_text(&self, index: usize) -> String {
        let values: Vec<_> = self.0[index].iter().map(f64::to_string).collect();
        if values.len() == 1 {
            values[0].clone()
        } else {
            format!("[{}]", values.join(", "))
        }
    }

    // Validate before replacing so a bad edit leaves the previous schedule intact.
    pub(crate) fn set_schedule_text(&mut self, index: usize, text: &str) -> crate::AppResult<bool> {
        let id = STAT_IDS.get(index).ok_or("unknown AKL.GG weight")?;
        if !is_weightable(index) {
            return Err(format!("{id} is not a configurable AKL.GG stat").into());
        }
        let values = parse_schedule(text.trim()).map_err(|error| format!("{id} {error}"))?;
        validate_schedule(id, &values)?;
        let changed = self.0[index] != values;
        self.0[index] = values;
        Ok(changed)
    }

    pub(crate) fn config_text(&self) -> String {
        let mut text = String::new();
        for &index in WEIGHTABLE_STATS {
            let id = STAT_IDS[index];
            text.push_str(&format!("{id} = {}\n", self.schedule_text(index)));
        }
        text
    }
}

fn parse_schedule(value: &str) -> Result<Vec<f64>, String> {
    if value.starts_with('[') || value.ends_with(']') {
        let body = value
            .strip_prefix('[')
            .and_then(|value| value.strip_suffix(']'))
            .ok_or("schedule must use matching brackets")?;
        if body.trim().is_empty() {
            return Ok(Vec::new());
        }
        body.split(',')
            .map(|part| {
                part.trim()
                    .parse::<f64>()
                    .map_err(|_| "schedule contains an invalid number".to_string())
            })
            .collect()
    } else {
        Ok(vec![value
            .parse::<f64>()
            .map_err(|_| "schedule contains an invalid number")?])
    }
}

fn validate_schedule(id: &str, schedule: &[f64]) -> Result<(), String> {
    if schedule.is_empty() || schedule.len() % 2 == 0 {
        return Err(format!(
            "{id} schedule must contain an odd number of values (slope, boundary, ..., slope)"
        ));
    }
    for (index, value) in schedule.iter().enumerate() {
        if !value.is_finite() {
            return Err(format!("{id} schedule values must be finite"));
        }
        if index % 2 == 1 {
            if *value <= 0.0 {
                return Err(format!("{id} boundaries must be positive"));
            }
            if index >= 3 && *value <= schedule[index - 2] {
                return Err(format!("{id} boundaries must be strictly ascending"));
            }
        }
    }
    Ok(())
}

/// Screenshot-stat objective using Mana2's progressive schedule arithmetic.
/// AKLER maximizes this signed score; negative weights penalize a statistic.
/// This deliberately differs from Mana2's built-in `core.Score` objective.
pub(crate) fn score(stats: &Stats, weights: &Weights) -> f64 {
    score_contributions(stats, weights).into_iter().sum()
}

/// Return Mana2's independently weighted contribution for each stat.
pub(crate) fn score_contributions(stats: &Stats, weights: &Weights) -> [f64; N_STATS] {
    std::array::from_fn(|index| {
        if is_weightable(index) {
            score_stat(stats.values[index], &weights.0[index])
        } else {
            0.0
        }
    })
}

/// Local slope of each weighted stat's score at `stats`. Progressive
/// schedules are piecewise linear; a boundary uses its lower bracket.
pub(crate) fn score_slopes(stats: &Stats, weights: &Weights) -> [f64; N_STATS] {
    std::array::from_fn(|index| {
        if is_weightable(index) {
            stat_slope(stats.values[index], &weights.0[index])
        } else {
            0.0
        }
    })
}

fn stat_slope(frequency: f64, weight: &[f64]) -> f64 {
    for i in (0..weight.len().saturating_sub(2)).step_by(2) {
        if frequency <= weight[i + 1] {
            return weight[i];
        }
    }
    weight[weight.len() - 1]
}

fn score_stat(mut frequency: f64, weight: &[f64]) -> f64 {
    let mut score = 0.0;
    let mut floor = 0.0;
    for i in (0..weight.len().saturating_sub(2)).step_by(2) {
        let bracket_size = weight[i + 1] - floor;
        if frequency <= bracket_size {
            return score + frequency * weight[i];
        }
        frequency -= bracket_size;
        score += bracket_size * weight[i];
        floor = weight[i + 1];
    }
    score + frequency * weight[weight.len() - 1]
}

#[derive(Clone, Copy)]
struct PhysicalKey {
    finger: usize,
    x_milli: i32,
    y_milli: i32,
}

impl PhysicalKey {
    fn from_key(key: Key) -> Self {
        // AKLER order is LP,LR,LM,LI,RI,RM,RR,RP,LT,RT. Mana2's order is
        // LP,LR,LM,LI,LT,RT,RI,RM,RR,RP.
        let finger = match key.finger {
            0..=3 => key.finger,
            4 => 6,
            5 => 7,
            6 => 8,
            7 => 9,
            8 => 4,
            9 => 5,
            _ => unreachable!("invalid AKLER finger index {}", key.finger),
        };
        Self {
            finger,
            x_milli: i32::from(key.col) * 1000 + i32::from(key.row_offset),
            y_milli: i32::from(key.row) * 1000 + i32::from(key.column_offset),
        }
    }

    fn x(self) -> f64 {
        f64::from(self.x_milli) / 1000.0
    }

    fn y(self) -> f64 {
        f64::from(self.y_milli) / 1000.0
    }

    fn hand(self) -> usize {
        usize::from(self.finger >= 5)
    }

    fn is_left(self) -> bool {
        self.finger <= 4
    }

    fn is_thumb(self) -> bool {
        self.finger == 4 || self.finger == 5
    }

    fn same_position(self, other: Self) -> bool {
        self.x_milli == other.x_milli && self.y_milli == other.y_milli
    }
}

fn add_monogram(raw: &mut RawStats, key: PhysicalKey, frequency: f64) {
    // STAT_IDS uses Mana2's exact suffix order: LP,LR,LM,LI,LT,RT,RI,RM,RR,RP.
    raw.0[key.finger] += 100.0 * frequency;
    if key.y_milli != 1000 && matches!(key.finger, 0 | 9) {
        raw.0[M_OFFPINKY] += 100.0 * frequency;
    }
    if key.y_milli == 2000 && matches!(key.finger, 0 | 2 | 8 | 9) {
        raw.0[M_PINKYRINGCURL] += 100.0 * frequency;
    }
}

fn add_pair(raw: &mut RawStats, a: &PhysicalKey, b: &PhysicalKey, frequency: f64, skipgram: bool) {
    let (sfb, sfbw, skb, stretch, scissor) = pair_stats(a, b);
    let indices = if skipgram {
        [S_SFS, S_SFSW, S_SKS, S_LSS, S_VSS]
    } else {
        [B_SFB, B_SFBW, B_SKB, B_LSB, B_VSB]
    };
    raw.0[indices[0]] += sfb * frequency;
    raw.0[indices[1]] += sfbw * frequency;
    raw.0[indices[2]] += skb * frequency;
    raw.0[indices[3]] += stretch * frequency;
    raw.0[indices[4]] += scissor * frequency;
}

fn pair_stats(a: &PhysicalKey, b: &PhysicalKey) -> (f64, f64, f64, f64, f64) {
    let same_position = a.same_position(*b);
    let same_finger = a.finger == b.finger;
    let sfb = if same_finger && !same_position {
        100.0
    } else {
        0.0
    };
    let skb = if same_finger && same_position {
        100.0
    } else {
        0.0
    };
    let sfbw = weighted_sfb(a, b);
    let stretch = stretch_rating(a, b);
    let scissor = scissor_rating(a, b);
    (sfb, sfbw, skb, stretch, scissor)
}

fn weighted_sfb(a: &PhysicalKey, b: &PhysicalKey) -> f64 {
    if a.same_position(*b) || a.finger != b.finger {
        return 0.0;
    }
    let mirrored_finger = mirror(a.finger);
    let finger_penalty = match mirrored_finger {
        0 | 4 => 2.5,
        1 => 1.3,
        _ => 1.0,
    };
    let dx = a.x() - b.x();
    let dy = a.y() - b.y();
    let mut distance_penalty = (dx.powi(2) + dy.powi(2)).sqrt() - 0.15;
    if distance_penalty <= 1.0 {
        distance_penalty = 1.0;
    }
    100.0 * distance_penalty * finger_penalty
}

fn stretch_rating(a: &PhysicalKey, b: &PhysicalKey) -> f64 {
    if a.hand() != b.hand() || a.is_thumb() || b.is_thumb() || a.finger == b.finger {
        return 0.0;
    }
    let dx = (a.x() - b.x()).abs();
    let df = a.finger.abs_diff(b.finger) as f64;
    80.0 * (((((dx - 0.25).max(0.0)) / df).powi(2)) - 1.0).max(0.0)
}

/// Mana2's scissor rating for pressing `b` after `a`.
pub(crate) fn scissor(a: Key, b: Key) -> f64 {
    scissor_rating(&PhysicalKey::from_key(a), &PhysicalKey::from_key(b))
}

/// Where Mana2 measures `key`, in key units; a larger y is lower.
pub(crate) fn position(key: Key) -> (f64, f64) {
    let key = PhysicalKey::from_key(key);
    (key.x(), key.y())
}

fn scissor_rating(a: &PhysicalKey, b: &PhysicalKey) -> f64 {
    if a.is_thumb()
        || b.is_thumb()
        || a.hand() != b.hand()
        || a.finger == b.finger
        || a.y_milli == b.y_milli
    {
        return 0.0;
    }

    let f1 = mirror(a.finger);
    let f2 = mirror(b.finger);
    let (outer, inner, outer_y, inner_y) = if f1 > f2 {
        (f1, f2, a.y(), b.y())
    } else {
        (f2, f1, b.y(), a.y())
    };
    if outer > 3 || inner > 3 {
        return 0.0;
    }

    // This asymmetric table is Mana2's current core/stats.go table. The pair
    // is indexed by anatomical outer/inner fingers; [0] means index lower.
    let penalties = match (outer, inner) {
        (3, 2) => [0.0, 1.5],
        (3, 1) => [0.0, 1.0],
        (3, 0) => [0.0, 1.0],
        (2, 1) => [0.5, 1.0],
        (2, 0) => [1.75, 0.75],
        (1, 0) => [2.25, 0.5],
        _ => return 0.0,
    };
    // Mana2's [0] means index lower; larger physical y is lower on the board.
    let index = if outer_y > inner_y { 0 } else { 1 };
    let vertical_distance = ((outer_y - inner_y).abs() - 0.25).max(0.0).powf(2.5);
    let is_inroll = f1 < f2;
    let is_outroll = f1 > f2;
    let roll_factor = if (index == 0 && is_inroll) || (index == 1 && is_outroll) {
        2.0
    } else {
        1.0
    };
    penalties[index] * vertical_distance * roll_factor * 84.2
}

fn add_trigram(
    raw: &mut RawStats,
    a: &PhysicalKey,
    b: &PhysicalKey,
    c: &PhysicalKey,
    frequency: f64,
) {
    let has_thumb = a.is_thumb() || b.is_thumb() || c.is_thumb();
    let alt = a.hand() != b.hand() && b.hand() != c.hand();
    let alt_sfs = alt && a.finger == c.finger;

    let redirect = a.hand() == b.hand()
        && b.hand() == c.hand()
        && a.finger != b.finger
        && b.finger != c.finger
        && ((a.finger < b.finger) != (b.finger < c.finger));
    let redirect_sfs = redirect && a.finger == c.finger;
    let redirect_weak = redirect
        && ![a.finger, b.finger, c.finger]
            .into_iter()
            .any(|finger| matches!(finger, 3 | 6) || matches!(finger, 4 | 5));
    let redirect_sfs_weak = redirect_sfs && redirect_weak;

    let inroll2 = is_inroll2(a, b, c);
    let outroll2 = is_outroll2(a, b, c);
    let inroll3 = is_inroll3(a, b, c);
    let outroll3 = is_outroll3(a, b, c);
    let roll = inroll2 || outroll2 || inroll3 || outroll3;
    let goodroll = roll && scissor_rating(a, b) == 0.0 && scissor_rating(b, c) == 0.0;

    add_indicator(raw, T_ALT, alt, frequency);
    add_indicator(raw, T_ALT_NO_THUMBS, alt && !has_thumb, frequency);
    add_indicator(raw, T_ALT_SFS, alt_sfs, frequency);
    add_indicator(raw, T_ALT_SFS_NO_THUMBS, alt_sfs && !has_thumb, frequency);
    add_indicator(raw, T_REDIRECT, redirect, frequency);
    add_indicator(raw, T_REDIRECT_NO_THUMBS, redirect && !has_thumb, frequency);
    add_indicator(raw, T_REDIRECT_SFS, redirect_sfs, frequency);
    add_indicator(
        raw,
        T_REDIRECT_SFS_NO_THUMBS,
        redirect_sfs && !has_thumb,
        frequency,
    );
    add_indicator(raw, T_REDIRECT_WEAK, redirect_weak, frequency);
    add_indicator(
        raw,
        T_REDIRECT_WEAK_NO_THUMBS,
        redirect_weak && !has_thumb,
        frequency,
    );
    add_indicator(raw, T_REDIRECT_SFS_WEAK, redirect_sfs_weak, frequency);
    add_indicator(
        raw,
        T_REDIRECT_SFS_WEAK_NO_THUMBS,
        redirect_sfs_weak && !has_thumb,
        frequency,
    );
    add_indicator(raw, T_ROLL, roll, frequency);
    add_indicator(raw, T_ROLL_NO_THUMBS, roll && !has_thumb, frequency);
    add_indicator(raw, T_INROLL2, inroll2, frequency);
    add_indicator(raw, T_INROLL2_NO_THUMBS, inroll2 && !has_thumb, frequency);
    add_indicator(raw, T_OUTROLL2, outroll2, frequency);
    add_indicator(raw, T_OUTROLL2_NO_THUMBS, outroll2 && !has_thumb, frequency);
    add_indicator(raw, T_INROLL3, inroll3, frequency);
    add_indicator(raw, T_INROLL3_NO_THUMBS, inroll3 && !has_thumb, frequency);
    add_indicator(raw, T_OUTROLL3, outroll3, frequency);
    add_indicator(raw, T_OUTROLL3_NO_THUMBS, outroll3 && !has_thumb, frequency);
    add_indicator(raw, T_GOODROLL, goodroll, frequency);
    add_indicator(raw, T_GOODROLL_NO_THUMBS, goodroll && !has_thumb, frequency);
}

fn add_indicator(raw: &mut RawStats, index: usize, matches: bool, frequency: f64) {
    if matches {
        raw.0[index] += 100.0 * frequency;
    }
}

fn is_roll2(a: &PhysicalKey, b: &PhysicalKey, c: &PhysicalKey) -> bool {
    a.hand() != c.hand() && a.finger != b.finger && b.finger != c.finger
}

fn is_inroll2(a: &PhysicalKey, b: &PhysicalKey, c: &PhysicalKey) -> bool {
    if !is_roll2(a, b, c) {
        return false;
    }
    ((a.hand() == b.hand() && a.finger > b.finger) || (a.hand() != b.hand() && c.finger < b.finger))
        != b.is_left()
}

fn is_outroll2(a: &PhysicalKey, b: &PhysicalKey, c: &PhysicalKey) -> bool {
    if !is_roll2(a, b, c) {
        return false;
    }
    ((a.hand() == b.hand() && a.finger > b.finger) || (a.hand() != b.hand() && c.finger < b.finger))
        == b.is_left()
}

fn is_inroll3(a: &PhysicalKey, b: &PhysicalKey, c: &PhysicalKey) -> bool {
    a.hand() == b.hand()
        && b.hand() == c.hand()
        && a.finger != b.finger
        && b.finger != c.finger
        && a.finger != c.finger
        && mirror(a.finger) < mirror(b.finger)
        && mirror(b.finger) < mirror(c.finger)
}

fn is_outroll3(a: &PhysicalKey, b: &PhysicalKey, c: &PhysicalKey) -> bool {
    a.hand() == b.hand()
        && b.hand() == c.hand()
        && a.finger != b.finger
        && b.finger != c.finger
        && a.finger != c.finger
        && mirror(a.finger) > mirror(b.finger)
        && mirror(b.finger) > mirror(c.finger)
}

fn mirror(finger: usize) -> usize {
    if finger >= 5 {
        9 - finger
    } else {
        finger
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(finger: usize, col: i8, row: i8, row_offset: i16, column_offset: i16) -> Key {
        Key {
            row,
            col,
            row_offset,
            column_offset,
            finger,
            rank: 0,
            hand: 0,
            main: true,
        }
    }

    #[test]
    fn weighted_sfb_and_mana2_finger_order_match_physical_keys() {
        let keys = [
            key(0, 0, 1, 0, 0), // AKLER LP -> Mana2 LP
            key(0, 2, 1, 0, 0), // a second key on AKLER LP
            key(8, 4, 1, 0, 0), // AKLER LT -> Mana2 LT
        ];
        let mut raw = RawStats::default();
        add_gram(&mut raw, &keys, 0, &[0], 1.0);
        add_gram(&mut raw, &keys, 0, &[2], 1.0);
        add_gram(&mut raw, &keys, 1, &[0, 1], 0.2);

        let stats = stats(&raw, [2.0, 0.2, 0.0, 0.0]);
        assert_eq!(stats.get("finger-usage-LP"), Some(50.0));
        assert_eq!(stats.get("finger-usage-LT"), Some(50.0));
        // Two physical units apart gives (2 - .15) * the LP penalty 2.5.
        assert_eq!(stats.get("sfbw"), Some(462.5));
        assert!((stats.get("fspeed-LP").unwrap() - 4496.0).abs() < 1e-12);
        assert!((stats.get("weighted-speed-LP").unwrap() - 4496.0 / 1.5).abs() < 1e-12);
        assert_eq!(stats.get("fspeed-LT"), Some(0.0));

        let mut every_finger = RawStats::default();
        for finger in 0..10 {
            every_finger.1.bigrams[finger] = (finger + 1) as f64;
            every_finger.1.skipgrams[finger] = (finger + 2) as f64;
        }
        let stats = stats_with_speed_totals(&every_finger, [1.0; 4], [0.0, 800.0, 800.0, 0.0]);
        let fspeed_sum: f64 = (FINGER_FSPEED_START..FINGER_WEIGHTED_SPEED_START)
            .map(|index| stats.values[index])
            .sum();
        let weighted_sum: f64 = (FINGER_WEIGHTED_SPEED_START..N_STATS)
            .map(|index| stats.values[index])
            .sum();
        assert!((fspeed_sum - stats.values[TOTAL_FSPEED]).abs() < 1e-12);
        assert!((weighted_sum - stats.values[TOTAL_WEIGHTED_SPEED]).abs() < 1e-12);
    }

    #[test]
    fn stretch_and_scissor_use_mana2_physical_geometry() {
        let keys = [
            key(1, 0, 1, 0, 250), // inner finger: x=0, y=1.25
            key(3, 4, 0, 0, 0),   // outer finger: x=4, y=0
        ];
        let mut raw = RawStats::default();
        add_gram(&mut raw, &keys, 1, &[0, 1], 1.0);
        let stats = stats(&raw, [0.0, 1.0, 0.0, 0.0]);

        // 80 * (((4 - .25) / 2)^2 - 1) = 201.25.
        assert_eq!(stats.get("lsb"), Some(201.25));
        // A 1.25-row gap leaves unit excess; this (3,1) table entry is 1.0.
        assert_eq!(stats.get("vsb"), Some(84.2));
    }

    #[test]
    fn cached_single_grams_match_direct_classification_bits() {
        let keys = [
            key(0, 0, 0, 0, 0),
            key(0, 0, 2, 250, 0),
            key(1, 1, 1, 0, 200),
            key(3, 4, 0, 500, 0),
            key(4, 5, 2, 0, 0),
            key(6, 6, 1, 0, 0),
            key(8, 3, 1, 0, 0),
        ];
        let compiled = GeometryContributions::new(&keys, &crate::alt_fingers::Resolver::none());
        let mut expected = RawStats::default();
        let mut actual = RawStats::default();
        let n = keys.len();
        let mut step = 0;
        for frequency in [1.0, -1.0, 0.1, -0.3, 7.0, 0.0, -0.0] {
            for a in 0..n {
                for b in 0..n {
                    for c in 0..n {
                        let slots = [a, b, c];
                        let kind = step % 4;
                        step += 1;
                        let len = [1, 2, 2, 3][kind];
                        add_gram(&mut expected, &keys, kind, &slots[..len], frequency);
                        compiled.add_gram(&mut actual, kind, &slots[..len], frequency);
                    }
                }
            }
        }
        for (index, (actual, expected)) in actual.0.iter().zip(expected.0).enumerate() {
            assert_eq!(actual.to_bits(), expected.to_bits(), "raw field {index}");
        }
        assert_eq!(actual.1, expected.1);
    }

    #[test]
    fn compiled_geometry_preserves_direct_contribution_bits() {
        let keys = [
            key(0, 0, 0, 0, 0),
            key(0, 0, 2, 250, 0),
            key(1, 1, 1, 0, 200),
            key(3, 4, 0, 500, 0),
            key(4, 5, 2, 0, 0),
            key(8, 3, 1, 0, 0),
        ];
        let compiled = GeometryContributions::new(&keys, &crate::alt_fingers::Resolver::none());
        let seeded = || {
            let mut raw = RawStats::default();
            for (index, value) in raw.0.iter_mut().enumerate() {
                *value = if index % 2 == 0 { 17.25 } else { -0.0 };
            }
            for (index, value) in raw.1.bigrams.iter_mut().enumerate() {
                *value = if index % 2 == 0 { 3.5 } else { -0.0 };
            }
            for (index, value) in raw.1.skipgrams.iter_mut().enumerate() {
                *value = if index % 2 == 0 { -2.5 } else { -0.0 };
            }
            raw
        };
        let assert_same = |actual: &RawStats, expected: &RawStats| {
            for (index, (actual, expected)) in actual.0.iter().zip(expected.0).enumerate() {
                assert_eq!(actual.to_bits(), expected.to_bits(), "raw field {index}");
            }
            for (name, actual, expected) in [
                ("bigram", &actual.1.bigrams, &expected.1.bigrams),
                ("skipgram", &actual.1.skipgrams, &expected.1.skipgrams),
            ] {
                for (finger, (actual, expected)) in actual.iter().zip(expected).enumerate() {
                    assert_eq!(
                        actual.to_bits(),
                        expected.to_bits(),
                        "{name} finger {finger}"
                    );
                }
            }
        };

        for frequency in [1.0, -1.0, 0.1, -0.1, 0.0, -0.0] {
            for a in 0..keys.len() {
                let mut expected = seeded();
                let mut actual = expected;
                add_gram(&mut expected, &keys, 0, &[a], frequency);
                compiled.add_history(&mut actual, &[a], frequency);
                assert_same(&actual, &expected);
                for b in 0..keys.len() {
                    let mut expected = seeded();
                    let mut actual = expected;
                    add_gram(&mut expected, &keys, 0, &[b], frequency);
                    add_gram(&mut expected, &keys, 1, &[a, b], frequency);
                    compiled.add_history(&mut actual, &[a, b], frequency);
                    assert_same(&actual, &expected);
                    for c in 0..keys.len() {
                        let mut expected = seeded();
                        let mut actual = expected;
                        add_gram(&mut expected, &keys, 0, &[c], frequency);
                        add_gram(&mut expected, &keys, 1, &[b, c], frequency);
                        add_gram(&mut expected, &keys, 2, &[a, c], frequency);
                        add_gram(&mut expected, &keys, 3, &[a, b, c], frequency);
                        compiled.add_history(&mut actual, &[a, b, c], frequency);
                        assert_same(&actual, &expected);
                    }
                }
            }
        }

        let mut weights = Weights(std::array::from_fn(|_| vec![0.0]));
        weights.0[B_SFB] = vec![-4.0];
        weights.0[S_LSS] = vec![-0.5];
        weights.0[T_REDIRECT] = vec![-1.0];
        weights.0[FINGER_WEIGHTED_SPEED_START + 4] = vec![-1.0]; // Mana LT -> AKLER 8.
                                                                 // Zero slopes stay inactive even when the schedule has boundaries.
        weights.0[T_ROLL] = vec![0.0, 0.5, -0.0];
        let active = ActiveStats::from_weights(&weights);
        assert_eq!(active.raw & (1 << B_SFB), 1 << B_SFB);
        assert_eq!(active.raw & (1 << S_LSS), 1 << S_LSS);
        assert_eq!(active.raw & (1 << T_REDIRECT), 1 << T_REDIRECT);
        assert_eq!(active.raw & (1 << T_ROLL), 0);
        assert_eq!(active.speed_fingers, 1 << 8);

        let mut prepared = GeometryContributions::new(&keys, &crate::alt_fingers::Resolver::none());
        prepared.prepare_active(active);
        let mut sparse_seed = seeded();
        for value in sparse_seed
            .0
            .iter_mut()
            .chain(sparse_seed.1.bigrams.iter_mut())
            .chain(sparse_seed.1.skipgrams.iter_mut())
        {
            if *value == 0.0 {
                *value = 0.0;
            }
        }
        assert!(active.allows_sparse_zeros(&sparse_seed));
        let mut negative_zero = sparse_seed;
        negative_zero.0[B_SFB] = -0.0;
        assert!(!active.allows_sparse_zeros(&negative_zero));
        negative_zero = sparse_seed;
        negative_zero.1.bigrams[8] = -0.0;
        assert!(!active.allows_sparse_zeros(&negative_zero));
        negative_zero = sparse_seed;
        negative_zero.1.skipgrams[8] = -0.0;
        assert!(!active.allows_sparse_zeros(&negative_zero));
        negative_zero = sparse_seed;
        negative_zero.0[M_OFFPINKY] = -0.0;
        negative_zero.1.bigrams[0] = -0.0;
        assert!(active.allows_sparse_zeros(&negative_zero));

        // Sparse omission preserves complete counters for finite proposals
        // that start without negative zero, including cancellation and tiny
        // signed products. Nonfinite frequencies must use the dense fallback.
        for sparse_active in [active, ActiveStats::from_weights(&Weights::default())] {
            prepared.prepare_active(sparse_active);
            let integer_seed = RawStats::default();
            assert!(prepared.active_integer_is_exact(&integer_seed, 100, sparse_active));
            let mut boundary = integer_seed;
            boundary.0[B_SFB] = ((1_u64 << 53) - 200) as f64;
            assert!(prepared.active_integer_is_exact(&boundary, 1, sparse_active));
            boundary.0[B_SFB] = ((1_u64 << 53) - 199) as f64;
            assert!(!prepared.active_integer_is_exact(&boundary, 1, sparse_active));
            boundary.0[B_SFB] = 0.5;
            assert!(!prepared.active_integer_is_exact(&boundary, 1, sparse_active));
            boundary.0[B_SFB] = -0.0;
            assert!(!prepared.active_integer_is_exact(&boundary, 1, sparse_active));
            boundary.0[B_SFB] = f64::INFINITY;
            assert!(!prepared.active_integer_is_exact(&boundary, 1, sparse_active));
            for frequency in [
                1.0,
                -1.0,
                17.0,
                -17.0,
                0.1,
                -0.1,
                0.0,
                -0.0,
                1e-250,
                -1e-250,
                f64::INFINITY,
                f64::NEG_INFINITY,
            ] {
                for a in 0..keys.len() {
                    let mut expected = sparse_seed;
                    let mut actual = expected;
                    let mut fused = expected;
                    prepared.add_one_active(&mut expected, a, frequency, sparse_active);
                    prepared.add_one_active_sparse(&mut actual, a, frequency, sparse_active);
                    prepared.add_tail_active_sparse(
                        &mut fused,
                        [u8::MAX, u8::MAX, a as u8],
                        frequency,
                        sparse_active,
                    );
                    assert_same(&actual, &expected);
                    assert_same(&fused, &expected);
                    for b in 0..keys.len() {
                        let mut expected = sparse_seed;
                        let mut actual = expected;
                        let mut fused = expected;
                        prepared.add_two_active(&mut expected, a, b, frequency, sparse_active);
                        prepared.add_two_active_sparse(&mut actual, a, b, frequency, sparse_active);
                        prepared.add_tail_active_sparse(
                            &mut fused,
                            [u8::MAX, a as u8, b as u8],
                            frequency,
                            sparse_active,
                        );
                        assert_same(&actual, &expected);
                        assert_same(&fused, &expected);
                        for c in 0..keys.len() {
                            if frequency.is_finite() && frequency.fract() == 0.0 {
                                let old = [a as u8, b as u8, c as u8];
                                for new in [
                                    [b as u8, a as u8, c as u8],
                                    [u8::MAX, a as u8, c as u8],
                                    [u8::MAX, u8::MAX, c as u8],
                                    [u8::MAX; 3],
                                ] {
                                    let mut expected = integer_seed;
                                    let mut actual = expected;
                                    prepared.add_tail_active_sparse(
                                        &mut expected,
                                        old,
                                        -frequency,
                                        sparse_active,
                                    );
                                    prepared.add_tail_active_sparse(
                                        &mut expected,
                                        new,
                                        frequency,
                                        sparse_active,
                                    );
                                    prepared.replace_tail_active_integer(
                                        &mut actual,
                                        old,
                                        new,
                                        frequency,
                                        sparse_active,
                                    );
                                    assert_same(&actual, &expected);
                                    let operation = prepared
                                        .compile_active_integer_replacement(old, new, sparse_active)
                                        .unwrap();
                                    let mut memoized = integer_seed;
                                    prepared.apply_active_integer_replacement(
                                        &mut memoized,
                                        &operation,
                                        frequency,
                                    );
                                    assert_same(&memoized, &expected);
                                    let mut split = integer_seed;
                                    operation.apply_boolean(&mut split, frequency);
                                    prepared.apply_active_integer_replacement_nonboolean(
                                        &mut split, &operation, frequency,
                                    );
                                    assert_same(&split, &expected);
                                    let mut updates = Vec::new();
                                    prepared
                                        .compile_integer_numeric_updates(&operation, &mut updates);
                                    let mut scheduled = integer_seed;
                                    operation.apply_boolean(&mut scheduled, frequency);
                                    NumericUpdate::apply_all(&mut scheduled, &updates, frequency);
                                    assert_same(&scheduled, &expected);
                                    if frequency >= 0.0 {
                                        let mut repeated = integer_seed;
                                        let mut grouped = integer_seed;
                                        let mut summed_frequency = 0.0;
                                        for next_frequency in [0.0, 1.0, 2.0, 17.0] {
                                            prepared.apply_active_integer_replacement(
                                                &mut repeated,
                                                &operation,
                                                next_frequency,
                                            );
                                            prepared.apply_active_integer_replacement_nonboolean(
                                                &mut grouped,
                                                &operation,
                                                next_frequency,
                                            );
                                            summed_frequency += next_frequency;
                                        }
                                        assert!(prepared.active_integer_is_exact(
                                            &integer_seed,
                                            summed_frequency as u64,
                                            sparse_active
                                        ));
                                        operation.apply_boolean(&mut grouped, summed_frequency);
                                        assert_same(&grouped, &repeated);
                                    }
                                    assert_eq!(
                                        operation.is_empty(),
                                        prepared.active_integer_replacement_is_empty(
                                            old,
                                            new,
                                            sparse_active
                                        )
                                    );
                                    if prepared.active_integer_replacement_is_empty(
                                        old,
                                        new,
                                        sparse_active,
                                    ) {
                                        assert_same(&expected, &integer_seed);
                                    }
                                }
                            }
                            let mut expected = sparse_seed;
                            let mut actual = expected;
                            let mut fused = expected;
                            prepared.add_three_active(
                                &mut expected,
                                a,
                                b,
                                c,
                                frequency,
                                sparse_active,
                            );
                            prepared.add_three_active_sparse(
                                &mut actual,
                                a,
                                b,
                                c,
                                frequency,
                                sparse_active,
                            );
                            assert_same(&actual, &expected);
                            prepared.add_tail_active_sparse(
                                &mut fused,
                                [a as u8, b as u8, c as u8],
                                frequency,
                                sparse_active,
                            );
                            assert_same(&fused, &expected);
                            if frequency.is_finite() {
                                prepared.add_three_active(
                                    &mut expected,
                                    a,
                                    b,
                                    c,
                                    -frequency,
                                    sparse_active,
                                );
                                prepared.add_three_active_sparse(
                                    &mut actual,
                                    a,
                                    b,
                                    c,
                                    -frequency,
                                    sparse_active,
                                );
                                assert_same(&actual, &expected);
                                prepared.add_tail_active_sparse(
                                    &mut fused,
                                    [a as u8, b as u8, c as u8],
                                    -frequency,
                                    sparse_active,
                                );
                                assert_same(&fused, &expected);
                            }
                        }
                    }
                }
            }
        }
        prepared.prepare_active(active);
        for frequency in [1.0, -1.0, 0.1, -0.1, 0.0, -0.0] {
            for a in 0..keys.len() {
                let mut expected = seeded();
                let mut actual = expected;
                compiled.add_one_active(&mut expected, a, frequency, active);
                prepared.add_one_active(&mut actual, a, frequency, active);
                assert_same(&actual, &expected);
                for b in 0..keys.len() {
                    let mut expected = seeded();
                    let mut actual = expected;
                    compiled.add_two_active(&mut expected, a, b, frequency, active);
                    prepared.add_two_active(&mut actual, a, b, frequency, active);
                    assert_same(&actual, &expected);
                    for c in 0..keys.len() {
                        let mut expected = seeded();
                        let mut actual = expected;
                        compiled.add_three_active(&mut expected, a, b, c, frequency, active);
                        prepared.add_three_active(&mut actual, a, b, c, frequency, active);
                        assert_same(&actual, &expected);
                    }
                }
            }
        }

        let mut expected = seeded();
        let mut projected = expected;
        add_gram(&mut expected, &keys, 0, &[5], 0.25);
        add_gram(&mut expected, &keys, 1, &[2, 5], 0.25);
        add_gram(&mut expected, &keys, 2, &[5, 5], 0.25);
        add_gram(&mut expected, &keys, 3, &[5, 2, 5], 0.25);
        compiled.add_three_active(&mut projected, 5, 2, 5, 0.25, active);
        for index in 0..N_RAW_STATS {
            let wanted = if active.raw & (1 << index) != 0 {
                expected.0[index]
            } else {
                seeded().0[index]
            };
            assert_eq!(projected.0[index].to_bits(), wanted.to_bits());
        }
        for finger in 0..10 {
            let wanted_bigram = if finger == 8 {
                expected.1.bigrams[finger]
            } else {
                seeded().1.bigrams[finger]
            };
            let wanted_skipgram = if finger == 8 {
                expected.1.skipgrams[finger]
            } else {
                seeded().1.skipgrams[finger]
            };
            assert_eq!(
                projected.1.bigrams[finger].to_bits(),
                wanted_bigram.to_bits()
            );
            assert_eq!(
                projected.1.skipgrams[finger].to_bits(),
                wanted_skipgram.to_bits()
            );
        }
        let totals = [1.0; 4];
        assert_eq!(
            score(&stats(&projected, totals), &weights).to_bits(),
            score(&stats(&expected, totals), &weights).to_bits()
        );
        assert!(compiled.projection_is_finite(&seeded(), totals, 1.0e100, 1.0));
        assert!(!compiled.projection_is_finite(&seeded(), totals, f64::MAX, 1.0));
        let mut non_finite = seeded();
        non_finite.0[T_ROLL] = f64::INFINITY;
        assert!(!compiled.projection_is_finite(&non_finite, totals, 1.0, 1.0));
        assert!(!compiled.projection_is_finite(&seeded(), totals, 1.0, 1.0e-320));
        assert!(!compiled.projection_is_finite(&seeded(), totals, 1.0, f64::NAN));
        assert!(!compiled.projection_is_finite(&seeded(), [1.0e-10; 4], 1.0, 1.0));
        let mut overflow = seeded();
        overflow.1.bigrams[0] = f64::MAX / 800.0;
        assert!(!compiled.projection_is_finite(&overflow, totals, 0.0, 1.0));
        let mut aggregate_overflow = seeded();
        aggregate_overflow.1.bigrams = [f64::MAX / 4096.0; 10];
        aggregate_overflow.1.skipgrams = [f64::MAX / 4096.0; 10];
        assert!(!stats(&aggregate_overflow, totals).values[TOTAL_FSPEED].is_finite());
        assert!(!compiled.projection_is_finite(&aggregate_overflow, totals, 0.0, 1.0));
        assert_eq!(finite_max(0.0, f64::NAN), f64::INFINITY);

        // Every configurable field, including each weighted-only finger and
        // both speed totals, gets its complete dependency closure. A slope
        // after a zero first bracket must still activate the field.
        for &index in WEIGHTABLE_STATS {
            for schedule in [vec![-1.0], vec![0.0, 0.5, -2.0]] {
                let mut weights = Weights(std::array::from_fn(|_| vec![0.0]));
                weights.0[index] = schedule;
                let active = ActiveStats::from_weights(&weights);
                if matches!(index, TOTAL_FSPEED | TOTAL_WEIGHTED_SPEED) {
                    assert_eq!(active.speed_fingers, (1 << 10) - 1);
                    assert_eq!(active.raw, 0);
                }
                let mut complete = seeded();
                let mut projected = complete;
                let mut cached = complete;
                compiled.add_three(&mut complete, 0, 1, 0, 0.25);
                compiled.add_three_active(&mut projected, 0, 1, 0, 0.25, active);
                prepared.prepare_active(active);
                prepared.add_three_active(&mut cached, 0, 1, 0, 0.25, active);
                assert_same(&cached, &projected);
                assert_eq!(
                    score(&stats(&projected, totals), &weights).to_bits(),
                    score(&stats(&complete, totals), &weights).to_bits(),
                    "projection for {} with {:?}",
                    STAT_IDS[index],
                    weights.0[index]
                );
            }
        }
    }

    #[test]
    fn trigram_roll2_excludes_thumb_variant_and_reports_weighted_terms() {
        let keys = [
            key(0, 0, 1, 0, 0), // left pinky
            key(8, 1, 1, 0, 0), // left thumb
            key(7, 2, 1, 0, 0), // right pinky
        ];
        let mut raw = RawStats::default();
        add_gram(&mut raw, &keys, 3, &[0, 1, 2], 0.25);
        let stats = stats(&raw, [0.0, 0.0, 0.0, 0.25]);

        assert_eq!(stats.get("inroll2"), Some(100.0));
        assert_eq!(stats.get("inroll2nothumbs"), Some(0.0));
        let weights = Weights::default();
        let contributions = score_contributions(&stats, &weights);
        assert_eq!(
            contributions[STAT_IDS.iter().position(|id| *id == "inroll2").unwrap()],
            0.0
        );
        assert_eq!(
            contributions[STAT_IDS
                .iter()
                .position(|id| *id == "inroll2nothumbs")
                .unwrap()],
            0.0
        );
        assert_eq!(score(&stats, &weights), 0.0);

        let index = |id: &str| {
            STAT_IDS
                .iter()
                .position(|candidate| *candidate == id)
                .unwrap()
        };
        let mut values = [0.0; N_STATS];
        values[index("fspeed-LP")] = 2.0;
        values[index("weighted-speed-LP")] = 3.0;
        values[index("weighted-speed-LT")] = 5.0;
        let synthetic = Stats {
            values,
        };
        let mut weights = Weights::default();
        for &stat in WEIGHTABLE_STATS {
            weights.set_schedule_text(stat, "0").unwrap();
        }
        weights.set_schedule_text(index("fspeed-LP"), "-1").unwrap();
        assert_eq!(score(&synthetic, &weights), -2.0);
        weights.set_schedule_text(index("fspeed-LP"), "0").unwrap();
        weights
            .set_schedule_text(index("weighted-speed-LP"), "-1")
            .unwrap();
        assert_eq!(score(&synthetic, &weights), -3.0);
        weights
            .set_schedule_text(index("weighted-speed-LP"), "0")
            .unwrap();
        weights
            .set_schedule_text(index("weighted-speed-LT"), "-1")
            .unwrap();
        assert_eq!(score(&synthetic, &weights), -5.0);
        // Native pinky stats and per-finger speeds are weightable in both modes.
        for text in [
            "offpinky = -1",
            "pinkyringcurl = [-1, 2, -5]",
            "goodroll = -1",
            "fspeed-LP = -1",
            "weighted-speed-RI = -1",
            "fspeed = -1",
        ] {
            assert!(Weights::from_text(text).is_ok(), "{text}");
            assert!(Weights::from_text(text).is_ok(), "{text}");
        }

        let mut values = synthetic.values;
        values[index("weighted_speed")] = 11.0;
        let aklgg_stats = Stats {
            values,
        };
        // AKL.GG defaults weight only the all-finger total; the per-finger
        // values start at 0 and count once given a weight.
        let mut aklgg = Weights::default();
        assert_eq!(score(&aklgg_stats, &aklgg), -11.0);
        aklgg
            .set_schedule_text(index("weighted-speed-LT"), "-1")
            .unwrap();
        assert_eq!(
            score(&aklgg_stats, &aklgg),
            -11.0 - values[index("weighted-speed-LT")]
        );
        let text = aklgg.config_text();
        assert!(text.contains("weighted_speed = -1"));
        assert!(text.contains("fspeed-LP = 0"));
        assert!(text.contains("offpinky = 0"));
    }
}
