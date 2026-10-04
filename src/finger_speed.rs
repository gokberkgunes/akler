//! An available-pair finger-motion estimate for Simple mode.
//!
//! This is an explicit AKLER model, not a Mana2 statistic or measured WPM.
//! Each same-finger pair contributes its Euclidean physical distance times
//! frequency. Bigram and skip-1 sums are independently normalized per 100
//! available pairs. Fspeed = bigram motion + skip_ratio * skip-1 motion.
//! Weighted speed divides each finger's Fspeed by its configured strength.
//! Same-position repeats have zero distance. Thumbs follow the same rule.
//!
//! The general distance/frequency/strength idea follows Oxeylyzer's Fspeed,
//! but its full model includes further skip populations and other machinery:
//! https://github.com/o-x-e-y/oxeylyzer#fspeed
//! No missing skip-2/skip-3 populations are synthesized here.

use crate::Key;
use std::ops::{AddAssign, SubAssign};

/// AKLER finger order: LP, LR, LM, LI, RI, RM, RR, RP, LT, RT.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Raw {
    pub(crate) bigrams: [f64; 10],
    pub(crate) skipgrams: [f64; 10],
}

impl AddAssign for Raw {
    fn add_assign(&mut self, rhs: Self) {
        for i in 0..10 {
            self.bigrams[i] += rhs.bigrams[i];
            self.skipgrams[i] += rhs.skipgrams[i];
        }
    }
}

impl SubAssign for Raw {
    fn sub_assign(&mut self, rhs: Self) {
        for i in 0..10 {
            self.bigrams[i] -= rhs.bigrams[i];
            self.skipgrams[i] -= rhs.skipgrams[i];
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Normalized {
    pub(crate) bigrams: [f64; 10],
    pub(crate) skipgrams: [f64; 10],
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Settings {
    pub(crate) skip_ratio: f64,
    pub(crate) strengths: [f64; 10],
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            skip_ratio: 0.11,
            // Starting preferences, not measured personal finger capacities.
            strengths: [1.5, 3.6, 4.8, 5.5, 5.5, 4.8, 3.6, 1.5, 3.3, 3.3],
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Stats {
    pub(crate) per_finger: [f64; 10],
    pub(crate) weighted: [f64; 10],
}

pub(crate) fn accumulate(raw: &mut Raw, a: &Key, b: &Key, frequency: f64, skip: bool) {
    if a.finger != b.finger || frequency == 0.0 {
        return;
    }
    let x = |key: &Key| f64::from(key.col) + f64::from(key.row_offset) / 1000.0;
    let y = |key: &Key| f64::from(key.row) + f64::from(key.column_offset) / 1000.0;
    let distance = (x(a) - x(b)).hypot(y(a) - y(b));
    let sums = if skip {
        &mut raw.skipgrams
    } else {
        &mut raw.bigrams
    };
    sums[a.finger] += frequency * distance;
}

pub(crate) fn normalize(raw: &Raw, totals: [f64; 4]) -> Normalized {
    let average = |sum: f64, total: f64| {
        if total > 0.0 {
            100.0 * sum / total
        } else {
            0.0
        }
    };
    Normalized {
        bigrams: std::array::from_fn(|i| average(raw.bigrams[i], totals[1])),
        skipgrams: std::array::from_fn(|i| average(raw.skipgrams[i], totals[2])),
    }
}

pub(crate) fn calculate(data: &Normalized, settings: &Settings) -> Stats {
    let per_finger: [f64; 10] = std::array::from_fn(|i| {
        // Signed incremental deltas can leave a tiny negative rounding residue.
        (data.bigrams[i] + settings.skip_ratio * data.skipgrams[i]).max(0.0)
    });
    Stats {
        weighted: std::array::from_fn(|i| per_finger[i] / settings.strengths[i]),
        per_finger,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(finger: usize, col: i8, row: i8, stagger: i16) -> Key {
        Key {
            finger,
            col,
            row,
            row_offset: stagger,
            column_offset: 0,
            rank: 0,
            hand: i8::from(matches!(finger, 4..=7 | 9)),
            main: finger < 8,
        }
    }

    #[test]
    fn physical_motion_normalization_strength_and_signed_deltas() {
        let a = key(0, 0, 0, 0);
        let b = key(0, 0, 1, 0);
        let mut raw = Raw::default();
        accumulate(&mut raw, &a, &b, 0.5, false);
        accumulate(&mut raw, &b, &a, 1.0, true);
        accumulate(&mut raw, &a, &a, 20.0, false);
        accumulate(&mut raw, &a, &key(1, 0, 1, 0), 20.0, false);
        let data = normalize(&raw, [100.0, 2.0, 4.0, 1.0]);
        let settings = Settings {
            skip_ratio: 0.5,
            strengths: [2.0; 10],
        };
        let stats = calculate(&data, &settings);
        assert_eq!(stats.per_finger[0], 37.5);
        assert_eq!(stats.weighted[0], 18.75);
        assert_eq!(stats.per_finger[1..], [0.0; 9]);
        let saved = raw;
        accumulate(&mut raw, &a, &b, -0.5, false);
        accumulate(&mut raw, &a, &b, 0.5, false);
        assert_eq!(raw, saved);
        raw -= saved;
        assert_eq!(raw, Raw::default());
        raw += saved;
        assert_eq!(raw, saved);
        assert_eq!(normalize(&raw, [0.0; 4]), Normalized::default());
    }

    #[test]
    fn mirrored_fingers_stagger_and_thumbs_use_fixed_geometry() {
        let mut raw = Raw::default();
        for finger in [0, 7, 8, 9] {
            let a = key(finger, 0, 0, 0);
            let b = key(finger, 3, 4, 0);
            accumulate(&mut raw, &a, &b, 0.25, false);
            assert_eq!(raw.bigrams[finger], 1.25);
        }
        accumulate(&mut raw, &key(2, 0, 0, 0), &key(2, 0, 0, 500), 1.0, false);
        assert_eq!(raw.bigrams[2], 0.5);
        let mut upper = key(3, 0, 0, 0);
        upper.column_offset = 500;
        accumulate(&mut raw, &key(3, 0, 0, 0), &upper, 1.0, true);
        assert_eq!(raw.skipgrams[3], 0.5);
    }
}
