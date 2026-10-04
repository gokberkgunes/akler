//! Linear, editable scoring over a small subset of Mana2 statistics.
//!
//! Positive weights are penalties and negative weights are rewards. Some terms
//! intentionally overlap: for example, an inward roll contributes to `roll`,
//! `inroll`, and its directional length term when those weights are nonzero.
//! Finger speed uses the documented available-pair model in `finger_speed`.

use crate::mana2_metrics::Stats;

pub(crate) const N_STATS: usize = 18;

pub(crate) const STAT_IDS: [&str; N_STATS] = [
    "sfb",
    "sfs",
    "stretch_bigrams",
    "stretch_skipgrams",
    "scissor_bigrams",
    "scissor_skipgrams",
    "alternation",
    "redirect",
    "weak_redirect",
    "roll",
    "inroll",
    "outroll",
    "inroll2",
    "inroll3",
    "outroll2",
    "outroll3",
    "fspeed",
    "weighted_speed",
];

pub(crate) const LABELS: [&str; N_STATS] = [
    "SFB",
    "SFS",
    "Stretch bigrams",
    "Stretch skipgrams",
    "Scissor bigrams",
    "Scissor skipgrams",
    "Alternation",
    "Redirect",
    "Weak redirect",
    "Roll",
    "Inward roll",
    "Outward roll",
    "Inward roll 2",
    "Inward roll 3",
    "Outward roll 2",
    "Outward roll 3",
    "Fspeed",
    "Weighted speed",
];

const SFB: usize = 0;
const SFS: usize = 1;
const STRETCH_BIGRAMS: usize = 2;
const STRETCH_SKIPGRAMS: usize = 3;
const SCISSOR_BIGRAMS: usize = 4;
const SCISSOR_SKIPGRAMS: usize = 5;
const ALTERNATION: usize = 6;
const REDIRECT: usize = 7;
const WEAK_REDIRECT: usize = 8;
const ROLL: usize = 9;
const INROLL: usize = 10;
const OUTROLL: usize = 11;
const INROLL2: usize = 12;
const INROLL3: usize = 13;
const OUTROLL2: usize = 14;
const OUTROLL3: usize = 15;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Weights([f64; N_STATS], crate::finger_speed::Settings);

impl Default for Weights {
    fn default() -> Self {
        Self(
            [
                12.0, // sfb
                1.0,  // sfs
                1.0,  // stretch_bigrams
                0.25, // stretch_skipgrams
                1.0,  // scissor_bigrams
                0.25, // scissor_skipgrams
                0.0,  // alternation
                1.0,  // redirect
                2.0,  // weak_redirect
                -0.1, // roll
                0.0,  // inroll
                0.0,  // outroll
                0.0,  // inroll2
                0.0,  // inroll3
                0.0,  // outroll2
                0.0,  // outroll3
                0.0,  // fspeed: available as an optional additional cost
                1.0,  // weighted_speed: accounts for finger strength
            ],
            crate::finger_speed::Settings::default(),
        )
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
            if let Some(index) = SPEED_SETTING_IDS.iter().position(|id| *id == key) {
                let value = value
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| format!("line {}: {key} must be a number", line_index + 1))?;
                result
                    .set_speed_setting(index, value)
                    .map_err(|error| format!("line {}: {error}", line_index + 1))?;
                continue;
            }
            let index = STAT_IDS
                .iter()
                .position(|id| id.eq_ignore_ascii_case(&key))
                .ok_or_else(|| format!("line {}: unknown Simple stat {key}", line_index + 1))?;
            let value = value
                .trim()
                .parse::<f64>()
                .map_err(|_| format!("line {}: {key} must be a number", line_index + 1))?;
            validate_value(&key, value)
                .map_err(|error| format!("line {}: {error}", line_index + 1))?;
            result.0[index] = value;
        }
        Ok(result)
    }

    pub(crate) fn config_text(&self) -> String {
        let mut text = String::new();
        for (id, value) in STAT_IDS.iter().zip(self.0) {
            text.push_str(&format!("{id} = {value}\n"));
        }
        for (index, id) in SPEED_SETTING_IDS.iter().enumerate() {
            text.push_str(&format!("{id} = {}\n", self.get_speed_setting(index)));
        }
        text
    }

    pub(crate) fn speed_settings(&self) -> &crate::finger_speed::Settings {
        &self.1
    }

    pub(crate) fn get_speed_setting(&self, index: usize) -> f64 {
        if index == 0 {
            self.1.skip_ratio
        } else {
            self.1.strengths[index - 1]
        }
    }

    pub(crate) fn set_speed_setting(&mut self, index: usize, value: f64) -> crate::AppResult<bool> {
        let id = SPEED_SETTING_IDS
            .get(index)
            .ok_or("unknown speed setting")?;
        if !value.is_finite()
            || value > 1_000_000.0
            || (index == 0 && value < 0.0)
            || (index != 0 && value <= 0.0)
        {
            return Err(format!(
                "{id} must be finite, {} and at most 1000000",
                if index == 0 {
                    "nonnegative"
                } else {
                    "positive"
                }
            )
            .into());
        }
        let setting = if index == 0 {
            &mut self.1.skip_ratio
        } else {
            &mut self.1.strengths[index - 1]
        };
        let changed = *setting != value;
        *setting = value;
        Ok(changed)
    }

    pub(crate) fn get(&self, index: usize) -> f64 {
        self.0[index]
    }

    pub(crate) fn set(&mut self, index: usize, value: f64) -> crate::AppResult<bool> {
        let id = STAT_IDS.get(index).ok_or("unknown Simple weight")?;
        validate_value(id, value)?;
        let changed = self.0[index] != value;
        self.0[index] = value;
        Ok(changed)
    }
}

fn validate_value(id: &str, value: f64) -> Result<(), String> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(format!("{id} weight must be finite"))
    }
}

pub(crate) fn score_contributions(stats: &Stats, weights: &Weights) -> [f64; N_STATS] {
    let values = values(stats, weights);
    std::array::from_fn(|index| values[index] * weights.0[index])
}

pub(crate) const SPEED_SETTING_IDS: [&str; 11] = [
    "speed_skip_ratio",
    "speed_strength_lp",
    "speed_strength_lr",
    "speed_strength_lm",
    "speed_strength_li",
    "speed_strength_ri",
    "speed_strength_rm",
    "speed_strength_rr",
    "speed_strength_rp",
    "speed_strength_lt",
    "speed_strength_rt",
];

pub(crate) fn speeds(stats: &Stats, weights: &Weights) -> crate::finger_speed::Stats {
    crate::finger_speed::calculate(&stats.speed, &weights.1)
}

pub(crate) fn values(stats: &Stats, weights: &Weights) -> [f64; N_STATS] {
    let speed = speeds(stats, weights);
    [
        stat(stats, "sfb"),
        stat(stats, "sfs"),
        stat(stats, "lsb"),
        stat(stats, "lss"),
        stat(stats, "vsb"),
        stat(stats, "vss"),
        stat(stats, "alt"),
        stat(stats, "redirect"),
        stat(stats, "redirectweak"),
        stat(stats, "roll"),
        stat(stats, "inroll2") + stat(stats, "inroll3"),
        stat(stats, "outroll2") + stat(stats, "outroll3"),
        stat(stats, "inroll2"),
        stat(stats, "inroll3"),
        stat(stats, "outroll2"),
        stat(stats, "outroll3"),
        speed.per_finger.iter().sum(),
        speed.weighted.iter().sum(),
    ]
}

pub(crate) fn score(stats: &Stats, weights: &Weights) -> f64 {
    score_contributions(stats, weights).into_iter().sum()
}

fn stat(stats: &Stats, id: &str) -> f64 {
    stats
        .get(id)
        .unwrap_or_else(|| unreachable!("missing Mana2 stat {id}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mana2_metrics;

    fn stats(entries: &[(&str, f64)]) -> Stats {
        let mut values = [0.0; mana2_metrics::N_STATS];
        for (id, value) in entries {
            let index = mana2_metrics::STAT_IDS
                .iter()
                .position(|candidate| candidate == id)
                .unwrap();
            values[index] = *value;
        }
        Stats {
            values,
            speed: crate::finger_speed::Normalized::default(),
        }
    }

    #[test]
    fn scalar_weights_parse_and_round_trip() {
        let parsed =
            Weights::from_text("SFB = 7.5 # scalar penalty\nroll = -0.4\ninroll3 = 1.25\n")
                .unwrap();
        assert_eq!(parsed.get(SFB), 7.5);
        assert_eq!(parsed.get(ROLL), -0.4);
        assert_eq!(parsed.get(INROLL3), 1.25);
        assert_eq!(Weights::from_text(&parsed.config_text()).unwrap(), parsed);

        assert!(Weights::from_text("sfb = NaN\n").is_err());
        assert!(Weights::from_text("sfb = 1\nsfb = 2\n").is_err());
        assert!(Weights::from_text("unknown = 1\n").is_err());
        let mut speed_weights = Weights::from_text(
            "fspeed = 2\nweighted_speed = 3\nspeed_skip_ratio = 0.5\nspeed_strength_lp = 2\n",
        )
        .unwrap();
        assert_eq!(
            Weights::from_text(&speed_weights.config_text()).unwrap(),
            speed_weights
        );
        let previous = speed_weights.clone();
        for (index, bad) in [(0, -1.0), (1, 0.0), (1, f64::NAN), (1, f64::INFINITY)] {
            assert!(speed_weights.set_speed_setting(index, bad).is_err());
            assert_eq!(speed_weights, previous);
        }
        let mut speed_stats = stats(&[]);
        speed_stats.speed.bigrams[0] = 2.0;
        speed_stats.speed.skipgrams[0] = 4.0;
        let terms = score_contributions(&speed_stats, &speed_weights);
        assert_eq!(terms[16], 8.0);
        assert_eq!(terms[17], 6.0);
    }

    #[test]
    fn linear_terms_overlap_explicitly() {
        let stats = stats(&[
            ("redirect", 3.0),
            ("redirectweak", 2.0),
            ("roll", 8.0),
            ("inroll2", 2.0),
            ("inroll3", 1.0),
            ("outroll2", 4.0),
            ("outroll3", 1.0),
        ]);
        let mut weights = Weights([0.0; N_STATS], crate::finger_speed::Settings::default());
        weights.set(REDIRECT, 1.0).unwrap();
        weights.set(WEAK_REDIRECT, 2.0).unwrap();
        weights.set(ROLL, -0.1).unwrap();
        weights.set(INROLL, 3.0).unwrap();
        weights.set(OUTROLL, 4.0).unwrap();
        weights.set(INROLL2, 5.0).unwrap();

        let contributions = score_contributions(&stats, &weights);
        assert_eq!(contributions[REDIRECT], 3.0);
        assert_eq!(contributions[WEAK_REDIRECT], 4.0);
        assert_eq!(contributions[ROLL], -0.8);
        assert_eq!(contributions[INROLL], 9.0);
        assert_eq!(contributions[OUTROLL], 20.0);
        assert_eq!(contributions[INROLL2], 10.0);
        assert_eq!(score(&stats, &weights), 45.2);
    }
}
