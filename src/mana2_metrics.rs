//! Mana2's built-in stat definitions and configurable progressive score.
//!
//! `add_gram` accepts already-mapped physical slot IDs. It intentionally does
//! not use AKLER's ergonomic flags: Mana2 has its own coordinate comparisons,
//! finger ordering, stretch formula, scissors table, redirect, and roll rules.
//! `kind` follows `Gram.kind`: 0 monogram, 1 bigram, 2 skipgram, 3 trigram.

use crate::Key;
use std::ops::{Add, AddAssign, Sub, SubAssign};

pub(crate) const N_STATS: usize = 46;

/// IDs are copied from Mana2's built-in stats (plus its active
/// `pinkyringcurl` example stat) and line up with `RawStats`/`Stats.values`.
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
];

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

#[derive(Clone, Copy, Debug)]
pub(crate) struct RawStats(
    pub(crate) [f64; N_STATS],
    pub(crate) crate::finger_speed::Raw,
);

impl Default for RawStats {
    fn default() -> Self {
        Self([0.0; N_STATS], crate::finger_speed::Raw::default())
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
    pub(crate) speed: crate::finger_speed::Normalized,
}

impl Stats {
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

/// Convert accumulated weighted contributions into Mana2's per-category
/// averages. The four totals are monogram, bigram, skipgram, trigram masses.
pub(crate) fn stats(raw: &RawStats, totals: [f64; 4]) -> Stats {
    Stats {
        speed: crate::finger_speed::normalize(&raw.1, totals),
        values: std::array::from_fn(|index| {
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

/// Progressive slope schedules for Mana2's statistics, in [`STAT_IDS`] order.
/// Each schedule alternates slope and upper boundary, ending with a slope.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Weights([Vec<f64>; N_STATS]);

impl Default for Weights {
    fn default() -> Self {
        Self(std::array::from_fn(|index| {
            weights(STAT_IDS[index]).to_vec()
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
                .ok_or_else(|| format!("line {}: unknown Mana2 stat {key}", line_index + 1))?;
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
        format!("[{}]", values.join(", "))
    }

    // Validate before replacing so a bad edit leaves the previous schedule intact.
    pub(crate) fn set_schedule_text(&mut self, index: usize, text: &str) -> crate::AppResult<bool> {
        let id = STAT_IDS.get(index).ok_or("unknown Mana2 weight")?;
        let values = parse_schedule(text.trim()).map_err(|error| format!("{id} {error}"))?;
        validate_schedule(id, &values)?;
        let changed = self.0[index] != values;
        self.0[index] = values;
        Ok(changed)
    }

    pub(crate) fn config_text(&self) -> String {
        let mut text = String::new();
        for (index, id) in STAT_IDS.iter().enumerate() {
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

/// Mana2's progressive score. The result is directly compatible with
/// `core.Score`; AKLER minimizes the negated score.
pub(crate) fn score(stats: &Stats, weights: &Weights) -> f64 {
    score_contributions(stats, weights).into_iter().sum()
}

/// Return Mana2's independently weighted contribution for each stat.
pub(crate) fn score_contributions(stats: &Stats, weights: &Weights) -> [f64; N_STATS] {
    std::array::from_fn(|index| score_stat(stats.values[index], &weights.0[index]))
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

fn weights(id: &str) -> &'static [f64] {
    match id {
        "finger-usage-LP" | "finger-usage-RP" => &[0.0, 13.0, -1.0],
        "finger-usage-LR" | "finger-usage-LM" | "finger-usage-LI" | "finger-usage-RI"
        | "finger-usage-RM" | "finger-usage-RR" => &[0.0, 13.0, -0.7],
        "finger-usage-LT" | "finger-usage-RT" => &[0.0],
        "offpinky" => &[-0.5],
        "pinkyringcurl" => &[-22.0],
        "sfb"
        | "sfs"
        | "alt"
        | "altsfs"
        | "roll"
        | "rollnothumbs"
        | "outroll2"
        | "outroll2nothumbs"
        | "outroll3"
        | "outroll3nothumbs"
        | "redirect"
        | "redirectsfs"
        | "redirectsfsnothumbs"
        | "altsfsnothumbs" => &[0.0],
        "sfbw" | "skb" => &[-4.0, 0.5, -13.0, 1.0, -26.0],
        "sfsw" | "sks" => &[-2.0, 2.0, -8.2],
        "lsb" => &[-1.0, 1.5, -2.0, 3.5, -4.0],
        "lss" => &[-0.4, 3.4, -1.2, 3.8, -2.5],
        "vsb" => &[-2.0, 1.7, -4.0],
        "vss" => &[-1.5, 3.0, -3.0, 3.5, -6.0],
        "altnothumbs" => &[-0.1],
        "goodroll" => &[0.0],
        "goodrollnothumbs" => &[0.3],
        "inroll2" | "inroll3" => &[0.1],
        "inroll2nothumbs" | "inroll3nothumbs" => &[-0.1],
        "redirectnothumbs" => &[-1.0],
        "redirectweak" => &[-10.0, 0.3, -20.0],
        "redirectweaknothumbs" => &[0.0],
        "redirectsfsweak" => &[-20.0],
        "redirectsfsweaknothumbs" => &[0.0],
        _ => unreachable!("missing Mana2 weights for stat id {id}"),
    }
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
            10.0
        );
        assert_eq!(
            contributions[STAT_IDS
                .iter()
                .position(|id| *id == "inroll2nothumbs")
                .unwrap()],
            0.0
        );
        assert_eq!(score(&stats, &weights), 10.0);
    }
}
