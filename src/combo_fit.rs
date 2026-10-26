//! Fit a layout's combos: up to `max_combos` two-key combos, each typing a
//! costly bigram on a better key pair (see `combos`):
//!
//! - two keys on neighbouring fingers of one hand in the same row, typing any
//!   bigram other than their own letters;
//! - with `[combos] fatfingering`, two up-down or left-right neighbours on one
//!   finger, typing any bigram.
//!
//! Every combo is pressed as one chord (see `combos`); the screening cost
//! below still estimates a non-neighbour pair by its two presses.
//!
//! Candidates are screened by bigram frequency times the typing effort the
//! key pair saves (`LocalEffort`), and the best `shortlist` get exact
//! evaluations. Each round tries removing every combo and adding each
//! shortlisted one (exchanging it for the least useful combo when the set is
//! full), keeps the best improvement, and stops when no trial improves the
//! score. The fitter manages every combo in the layout: one whose keys no
//! longer have an allowed shape, after a search moved them, is dropped.

use crate::*;
use crate::{action_keys as ak, action_ngrams as ng, action_summary as summary};
use std::sync::atomic::AtomicUsize;

/// Most combos one fit may keep.
pub(crate) const MAX_COMBOS: usize = 32;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Settings {
    pub(crate) max_combos: usize,
    pub(crate) shortlist: usize,
    /// `[combos] fatfingering`, kept for saving the configuration; scoring and
    /// fitting read it from `Weights` (`weights.5`).
    pub(crate) fatfingering: bool,
    /// `[combos] exact`: fit exactly `max_combos` combos (as many as there
    /// are candidates), even where fewer would score better.
    pub(crate) exact: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            max_combos: 4,
            shortlist: 12,
            fatfingering: false,
            exact: false,
        }
    }
}

impl Settings {
    /// Combos to fit: `max_combos` while combos are scored (`[typing]
    /// combos`), else none.
    pub(crate) fn active_max(&self, weights: &Weights) -> usize {
        if weights.4 {
            self.max_combos
        } else {
            0
        }
    }
}

/// A fitted combo: two physical slots, pressed in order, typing `output`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Chord {
    pub(crate) first: usize,
    pub(crate) second: usize,
    pub(crate) output: [u8; 2],
}

impl Chord {
    fn keys(self) -> [usize; 2] {
        let mut keys = [self.first, self.second];
        keys.sort_unstable();
        keys
    }
}

pub(crate) struct Fit {
    pub(crate) layout: ak::Layout,
    pub(crate) summary: summary::Summary,
    pub(crate) chords: Vec<Chord>,
    pub(crate) evaluations: usize,
}

/// Whether two keys may form a combo: same-hand neighbouring fingers on one
/// row, or a fat-finger pair when those are scored.
pub(crate) fn allowed_pair(a: &ak::Slot, b: &ak::Slot, fatfinger: bool) -> bool {
    let neighbours =
        a.main && b.main && a.hand == b.hand && a.row == b.row && a.finger.abs_diff(b.finger) == 1;
    neighbours || (fatfinger && crate::combos::fat_finger_pair(a, b))
}

/// Slots a combo may use: main keys with a unique, nonempty label.
fn combo_slots(layout: &ak::Layout) -> Vec<usize> {
    (0..layout.slots.len())
        .filter(|&slot| {
            let key = &layout.slots[slot];
            key.main
                && !key.label.is_empty()
                && !matches!(key.binding, ak::Binding::Empty)
                && layout
                    .slots
                    .iter()
                    .filter(|other| other.label == key.label)
                    .count()
                    == 1
        })
        .collect()
}

fn chord_json(layout: &ak::Layout, chord: Chord) -> Json {
    Json::Object(BTreeMap::from([
        (
            "keys".to_string(),
            Json::Array(vec![
                Json::String(layout.slots[chord.first].label.clone()),
                Json::String(layout.slots[chord.second].label.clone()),
            ]),
        ),
        (
            "output".to_string(),
            Json::String(chord.output.iter().map(|&byte| byte as char).collect()),
        ),
    ]))
}

/// The chord a combo definition describes, if its two labels name slots.
fn parse_chord(layout: &ak::Layout, value: &Json) -> Option<Chord> {
    let Json::Object(fields) = value else {
        return None;
    };
    let labels: Vec<String> = match (fields.get("keys"), fields.get("inputs")) {
        (Some(Json::Array(keys)), None) => keys
            .iter()
            .map(|key| match key {
                Json::String(label) => Some(label.clone()),
                _ => None,
            })
            .collect::<Option<_>>()?,
        (None, Some(Json::Array(inputs))) if inputs.len() == 1 => match &inputs[0] {
            Json::String(text) => text.chars().map(String::from).collect(),
            _ => return None,
        },
        _ => return None,
    };
    let Some(Json::String(output)) = fields.get("output") else {
        return None;
    };
    let [first, second] = &labels[..] else {
        return None;
    };
    let &[a, b] = output.as_bytes() else {
        return None;
    };
    let find = |label: &str| {
        let mut matches = (0..layout.slots.len()).filter(|&slot| layout.slots[slot].label == label);
        let slot = matches.next()?;
        matches.next().is_none().then_some(slot)
    };
    Some(Chord {
        first: find(first)?,
        second: find(second)?,
        output: [a.to_ascii_lowercase(), b.to_ascii_lowercase()],
    })
}

/// Combos that type a costly bigram on a cheaper allowed key pair, best
/// first.
fn candidates(
    layout: &ak::Layout,
    corpus: &ng::NgramCorpus,
    effort: &crate::action_ui::LocalEffort,
    fatfinger: bool,
) -> Vec<(Chord, f64)> {
    const OUTPUTS: usize = 60;
    let mut masses: BTreeMap<[u8; 2], f64> = BTreeMap::new();
    for &(skip, context, target, mass) in corpus.rule_candidates() {
        if !skip && context != b' ' && target != b' ' {
            *masses
                .entry([context.to_ascii_lowercase(), target.to_ascii_lowercase()])
                .or_default() += mass;
        }
    }
    // The single main slot typing each character on its own.
    let mut slot_of: BTreeMap<u8, Option<usize>> = BTreeMap::new();
    for (slot, key) in layout.slots.iter().enumerate() {
        if let ak::Binding::Text(text) = &key.binding {
            if text.len() == 1 && key.main {
                slot_of
                    .entry(text[0].to_ascii_lowercase())
                    .and_modify(|known| *known = None)
                    .or_insert(Some(slot));
            }
        }
    }
    let cost = |a: usize, b: usize| effort.get(None, Some(a), b);
    let mut outputs: Vec<([u8; 2], f64, f64)> = masses
        .iter()
        .filter_map(|(&bigram, &mass)| {
            let (Some(&Some(a)), Some(&Some(b))) =
                (slot_of.get(&bigram[0]), slot_of.get(&bigram[1]))
            else {
                return None;
            };
            let own = cost(a, b);
            (own > 0.0).then_some((bigram, mass, own))
        })
        .collect();
    outputs.sort_by(|a, b| (b.1 * b.2).total_cmp(&(a.1 * a.2)).then(a.0.cmp(&b.0)));
    outputs.truncate(OUTPUTS);

    let slots = combo_slots(layout);
    let typed = |slot: usize| match &layout.slots[slot].binding {
        ak::Binding::Text(text) if text.len() == 1 => Some(text[0].to_ascii_lowercase()),
        _ => None,
    };
    let mut result = Vec::new();
    for (output, mass, own) in outputs {
        for &first in &slots {
            for &second in &slots {
                let (a, b) = (&layout.slots[first], &layout.slots[second]);
                if first == second || !allowed_pair(a, b, fatfinger) {
                    continue;
                }
                let fat = fatfinger && crate::combos::fat_finger_pair(a, b);
                // Combos on the bigram's own keys are left out.
                if !fat && [typed(first), typed(second)] == [Some(output[0]), Some(output[1])] {
                    continue;
                }
                let chord_cost = if fat { 0.0 } else { cost(first, second) };
                let gain = mass * (own - chord_cost);
                if gain > 0.0 {
                    result.push((
                        Chord {
                            first,
                            second,
                            output,
                        },
                        gain,
                    ));
                }
            }
        }
    }
    result.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    result
}

/// Whether two combos cannot coexist: one key pair has one chord, and one
/// output has one combo.
fn conflicts(a: Chord, b: Chord) -> bool {
    a.keys() == b.keys() || a.output == b.output
}

fn with_chords(layout: &ak::Layout, chords: &[Chord]) -> ak::Layout {
    let mut result = layout.clone();
    result.combos = chords
        .iter()
        .map(|&chord| chord_json(layout, chord))
        .collect();
    result
}

/// One random change to `layout`'s combos for the joint search to try at
/// annealing temperature: drop one, add a shortlisted candidate (up to the
/// limit), or exchange one for a candidate; `exact` only exchanges. `pick(n)`
/// draws an index below `n`. None when nothing can change.
pub(crate) fn neighbor(
    layout: &ak::Layout,
    corpus: &ng::NgramCorpus,
    weights: &Weights,
    settings: &Settings,
    pick: &mut dyn FnMut(usize) -> usize,
) -> Option<ak::Layout> {
    let fatfinger = weights.5;
    let mut chords: Vec<Chord> = layout
        .combos
        .iter()
        .filter_map(|value| parse_chord(layout, value))
        .filter(|chord| {
            allowed_pair(
                &layout.slots[chord.first],
                &layout.slots[chord.second],
                fatfinger,
            )
        })
        .collect();
    let effort = crate::action_ui::LocalEffort::new(layout, weights);
    let shortlist: Vec<Chord> = candidates(layout, corpus, &effort, fatfinger)
        .into_iter()
        .map(|(chord, _)| chord)
        .filter(|chord| !chords.contains(chord))
        .take(settings.shortlist)
        .collect();
    let mut kinds = Vec::new();
    if !settings.exact && !chords.is_empty() {
        kinds.push(0);
    }
    if !settings.exact && chords.len() < settings.max_combos && !shortlist.is_empty() {
        kinds.push(1);
    }
    if !chords.is_empty() && !shortlist.is_empty() {
        kinds.push(2);
    }
    if kinds.is_empty() {
        return None;
    }
    let kind = kinds[pick(kinds.len())];
    if kind != 1 {
        chords.remove(pick(chords.len()));
    }
    if kind != 0 {
        let open: Vec<Chord> = shortlist
            .into_iter()
            .filter(|&candidate| !chords.iter().any(|&chord| conflicts(chord, candidate)))
            .collect();
        if open.is_empty() {
            return None;
        }
        chords.push(open[pick(open.len())]);
    }
    chords.sort_unstable();
    Some(with_chords(layout, &chords))
}

/// `evaluate` on each of a fitting round's independent `trials`, on up to
/// `threads` threads. Results come back in trial order, so every decision
/// made from them is the serial one.
pub(crate) fn evaluate_in_order<T: Sync, R: Send>(
    trials: &[T],
    threads: usize,
    evaluate: impl Fn(&T) -> ak::Result<R> + Sync,
) -> ak::Result<Vec<R>> {
    let threads = threads.clamp(1, trials.len().max(1));
    if threads == 1 {
        return trials.iter().map(&evaluate).collect();
    }
    let next = AtomicUsize::new(0);
    let results: Vec<std::sync::Mutex<Option<ak::Result<R>>>> =
        trials.iter().map(|_| std::sync::Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| loop {
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(trial) = trials.get(index) else {
                    break;
                };
                *results[index].lock().unwrap() = Some(evaluate(trial));
            });
        }
    });
    results
        .into_iter()
        .map(|result| result.into_inner().unwrap().unwrap())
        .collect()
}

#[cfg(test)]
thread_local! {
    /// Whether `screen_in_order` screens, and how many trials it spared.
    pub(crate) static SCREENING: std::cell::Cell<(bool, usize)> =
        const { std::cell::Cell::new((true, 0)) };
}

/// The exact evaluation of each of a fitting round's `trials` that
/// `consider` could keep, None for the others, and with `cheapest` the trial
/// of least overshoot, then score (the first of equals). `screen` scores
/// every trial first. A trial is evaluated exactly unless its screened score
/// shows it cannot beat `current` or `best` by `margin`, or is no better than
/// an earlier trial certain to beat `current` (the best by then is at least as
/// good), and, for `cheapest`, that another trial is certainly cheaper.
/// Decisions are the serial exact ones.
#[allow(clippy::too_many_arguments)]
pub(crate) fn screen_in_order<T: Sync>(
    trials: &[T],
    threads: usize,
    caps: Option<crate::action_search::Caps<'_>>,
    current: &summary::Summary,
    best: Option<&summary::Summary>,
    margin: f64,
    cheapest: bool,
    screen: impl Fn(&T) -> ak::Result<Option<summary::Summary>> + Sync,
    evaluate: impl Fn(&T) -> ak::Result<(summary::Summary, Option<ng::Mappings>)> + Sync,
) -> ak::Result<(
    Vec<Option<(summary::Summary, Option<ng::Mappings>)>>,
    Option<usize>,
)> {
    use crate::action_search::Caps;
    let screened = evaluate_in_order(trials, threads, screen)?;
    let mut exact = vec![false; trials.len()];
    let mut winners: Vec<&summary::Summary> = Vec::new();
    for (index, trial) in screened.iter().enumerate() {
        let Some(trial) = trial else {
            exact[index] = true;
            continue;
        };
        let loses = |known: &summary::Summary, screened: bool, margin: f64| {
            Caps::screened_beats(caps, trial, known, screened, margin) == Some(false)
        };
        exact[index] = !(loses(current, false, margin)
            || best.is_some_and(|best| loses(best, false, margin))
            || winners.iter().any(|&winner| loses(winner, true, 0.0)));
        if Caps::screened_beats(caps, trial, current, false, margin) == Some(true) {
            winners.push(trial);
        }
    }
    let key = |summary: &summary::Summary| (Caps::overshoot(caps, summary), summary.score);
    let lower = |a: (f64, f64), b: (f64, f64)| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1));
    if cheapest {
        // The screened cheapest rules out every trial certainly dearer.
        let least = screened
            .iter()
            .flatten()
            .min_by(|a, b| lower(key(a), key(b)));
        for (index, trial) in screened.iter().enumerate() {
            if let (Some(trial), Some(least)) = (trial, least) {
                if Caps::screened_beats(caps, least, trial, true, 0.0) == Some(true) {
                    continue;
                }
            }
            exact[index] = true;
        }
    }
    #[cfg(test)]
    SCREENING.with(|state| {
        let (on, spared) = state.get();
        if !on {
            exact.fill(true);
        }
        state.set((on, spared + exact.iter().filter(|&&exact| !exact).count()));
    });
    let indices: Vec<usize> = (0..trials.len()).filter(|&index| exact[index]).collect();
    let evaluated = evaluate_in_order(&indices, threads, |&index| evaluate(&trials[index]))?;
    let mut results: Vec<Option<_>> = trials.iter().map(|_| None).collect();
    for (index, result) in indices.into_iter().zip(evaluated) {
        results[index] = Some(result);
    }
    let cheapest = cheapest
        .then(|| {
            results
                .iter()
                .enumerate()
                .filter_map(|(index, result)| Some((index, key(&result.as_ref()?.0))))
                .min_by(|a, b| lower(a.1, b.1).then(a.0.cmp(&b.0)))
                .map(|(index, _)| index)
        })
        .flatten();
    Ok((results, cheapest))
}

/// Fit up to `settings.max_combos` combos to `layout`'s current bindings,
/// scored exactly with `physical`'s objective. Requires `[typing] combos`.
/// With `caps`, a set closer to within them wins before a lower score. The
/// limit counts every combo, the layout's own included. Each round's trials
/// run on up to `threads` threads; the result does not depend on it, nor on
/// `start`, any earlier layout's mappings for `corpus` and `physical`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn fit(
    layout: &ak::Layout,
    corpus: &ng::NgramCorpus,
    weights: &Weights,
    physical: &Arc<summary::PhysicalTables>,
    settings: &Settings,
    caps: Option<crate::action_search::Caps<'_>>,
    stop: &AtomicBool,
    progress: &AtomicU64,
    threads: usize,
    start: Option<&ng::Mappings>,
) -> ak::Result<Fit> {
    use crate::action_search::Caps;
    const EPSILON: f64 = 1e-9;
    if !weights.4 {
        return Err("Combo fitting requires [typing] combos enabled.".into());
    }
    let fatfinger = weights.5;
    let mut chords: Vec<Chord> = layout
        .combos
        .iter()
        .filter_map(|value| parse_chord(layout, value))
        .filter(|chord| {
            allowed_pair(
                &layout.slots[chord.first],
                &layout.slots[chord.second],
                fatfinger,
            )
        })
        .collect();
    let base = with_chords(layout, &[]);
    let prepared = summary::Prepared::with_physical(&base, corpus, weights, Arc::clone(physical))?;
    let evaluations = AtomicUsize::new(0);
    // Each trial re-maps only the windows it changes from the current layout.
    let exact = |chords: &[Chord], from: Option<&ng::Mappings>| {
        prepared.evaluate_from(&with_chords(layout, chords), from, stop, progress)
    };
    let evaluate = |chords: &[Chord], from: Option<&ng::Mappings>| {
        evaluations.fetch_add(1, Ordering::Relaxed);
        exact(chords, from)
    };
    let evaluate_all = |trials: &[Vec<Chord>], from: Option<&ng::Mappings>| {
        evaluate_in_order(trials, threads, |trial| evaluate(trial, from))
    };
    // Most trials cannot be kept; their screened scores show which.
    let screen_all = |trials: &[Vec<Chord>],
                      current: &summary::Summary,
                      best: Option<&summary::Summary>,
                      cheapest: bool,
                      from: Option<&ng::Mappings>| {
        screen_in_order(
            trials,
            threads,
            caps,
            current,
            best,
            EPSILON,
            cheapest,
            |trial| {
                evaluations.fetch_add(1, Ordering::Relaxed);
                match from {
                    Some(from) => prepared.screen_from(
                        &with_chords(layout, trial),
                        from,
                        current,
                        stop,
                        progress,
                    ),
                    None => Ok(None),
                }
            },
            |trial| exact(trial, from),
        )
    };
    // `start` holds an earlier layout's mappings for this corpus.
    evaluations.fetch_add(1, Ordering::Relaxed);
    let (mut current, mut mappings) =
        prepared.evaluate_fresh(&with_chords(layout, &chords), start, stop, progress)?;
    // A dictionary above the limit first loses its least useful combos.
    while chords.len() > settings.max_combos {
        let mut cheapest: Option<(usize, summary::Summary, Option<ng::Mappings>)> = None;
        let trials: Vec<Vec<Chord>> = (0..chords.len())
            .map(|index| {
                let mut trial = chords.clone();
                trial.remove(index);
                trial
            })
            .collect();
        let evaluated = evaluate_all(&trials, mappings.as_ref())?;
        for (index, (summary, trial_mappings)) in evaluated.into_iter().enumerate() {
            if cheapest
                .as_ref()
                .is_none_or(|(_, known, _)| Caps::beats(caps, &summary, known, 0.0))
            {
                cheapest = Some((index, summary, trial_mappings));
            }
        }
        let (index, summary, trial_mappings) =
            cheapest.expect("combos above the limit are not empty");
        chords.remove(index);
        current = summary;
        mappings = trial_mappings;
    }
    let effort = crate::action_ui::LocalEffort::new(layout, weights);
    // At most two alternatives per key pair and per output: each carries one
    // combo, so more alternatives would crowd out other pairs and bigrams.
    let mut per_pair: BTreeMap<[usize; 2], usize> = BTreeMap::new();
    let mut per_output: BTreeMap<[u8; 2], usize> = BTreeMap::new();
    let shortlist: Vec<Chord> = candidates(layout, corpus, &effort, fatfinger)
        .into_iter()
        .map(|(chord, _)| chord)
        .filter(|chord| {
            let pair = per_pair.entry(chord.keys()).or_default();
            let output = per_output.entry(chord.output).or_default();
            let keep = *pair < 2 && *output < 2;
            if keep {
                *pair += 1;
                *output += 1;
            }
            keep
        })
        .take(settings.shortlist)
        .collect();

    for _ in 0..settings.max_combos + 2 {
        type Best = Option<(Vec<Chord>, summary::Summary, Option<ng::Mappings>)>;
        let mut best: Best = None;
        let consider = |best: &mut Best,
                        trial: Vec<Chord>,
                        (summary, trial_mappings): (summary::Summary, _)| {
            if Caps::beats(caps, &summary, &current, EPSILON)
                && best
                    .as_ref()
                    .is_none_or(|(_, known, _)| Caps::beats(caps, &summary, known, EPSILON))
            {
                *best = Some((trial, summary, trial_mappings));
            }
        };
        // Removals also rank the fitted combos for exchanges.
        let trials: Vec<Vec<Chord>> = (0..chords.len())
            .map(|index| {
                let mut trial = chords.clone();
                trial.remove(index);
                trial
            })
            .collect();
        let (evaluated, cheapest) = screen_all(&trials, &current, None, true, mappings.as_ref())?;
        for (trial, evaluated) in trials.into_iter().zip(evaluated) {
            if let Some(evaluated) = evaluated {
                consider(&mut best, trial, evaluated);
            }
        }
        let mut trials = Vec::new();
        for &candidate in &shortlist {
            if chords.contains(&candidate) {
                continue;
            }
            let mut trial = chords.clone();
            if trial.len() >= settings.max_combos {
                let Some(cheapest) = cheapest else {
                    continue;
                };
                trial.remove(cheapest);
            }
            if trial.iter().any(|&chord| conflicts(chord, candidate)) {
                continue;
            }
            trial.push(candidate);
            trial.sort_unstable();
            trials.push(trial);
        }
        let best_summary = best.as_ref().map(|(_, known, _)| known);
        let (evaluated, _) = screen_all(&trials, &current, best_summary, false, mappings.as_ref())?;
        for (trial, evaluated) in trials.into_iter().zip(evaluated) {
            if let Some(evaluated) = evaluated {
                consider(&mut best, trial, evaluated);
            }
        }
        let Some((trial, summary, trial_mappings)) = best else {
            break;
        };
        chords = trial;
        current = summary;
        mappings = trial_mappings;
    }
    // `exact`: fill up to the count with the least costly candidates.
    while settings.exact && chords.len() < settings.max_combos {
        let mut best: Option<(Vec<Chord>, summary::Summary, Option<ng::Mappings>)> = None;
        let trials: Vec<Vec<Chord>> = shortlist
            .iter()
            .filter(|&&candidate| {
                !chords.contains(&candidate)
                    && !chords.iter().any(|&chord| conflicts(chord, candidate))
            })
            .map(|&candidate| {
                let mut trial = chords.clone();
                trial.push(candidate);
                trial.sort_unstable();
                trial
            })
            .collect();
        let evaluated = evaluate_all(&trials, mappings.as_ref())?;
        for (trial, (summary, trial_mappings)) in trials.into_iter().zip(evaluated) {
            if best
                .as_ref()
                .is_none_or(|(_, known, _)| Caps::beats(caps, &summary, known, 0.0))
            {
                best = Some((trial, summary, trial_mappings));
            }
        }
        let Some((trial, summary, trial_mappings)) = best else {
            break;
        };
        chords = trial;
        current = summary;
        mappings = trial_mappings;
    }
    chords.sort_unstable();
    Ok(Fit {
        layout: with_chords(layout, &chords),
        summary: current,
        chords,
        evaluations: evaluations.into_inner(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn corpus(text: &[u8]) -> ng::NgramCorpus {
        let tables = ak::text_ngrams(text);
        let names = ["letters", "bigrams", "trigrams", "fourgrams", "fivegrams"];
        let mut json = String::from("{");
        for (n, name) in names.iter().enumerate() {
            if n > 0 {
                json.push(',');
            }
            json.push_str(&format!("\"{name}\":{{"));
            for (i, (gram, frequency)) in tables[n].iter().enumerate() {
                if i > 0 {
                    json.push(',');
                }
                json.push_str(&format!("{}:{}", ak::quote(gram), frequency));
            }
            json.push('}');
        }
        json.push('}');
        ng::NgramCorpus::from_text(&json, Path::new("combo-fit.json")).unwrap()
    }

    fn qwerty() -> ak::Layout {
        ak::Layout::parse(
            "q w e r t  y u i o p\na s d f g  h j k l ;\nz x c v b  n m , . /\nthumbs: space\n",
            Path::new("combo-fit.dat"),
        )
        .unwrap()
    }

    fn combos(definitions: &str) -> Vec<Json> {
        let Json::Array(values) = crate::parse_json(definitions).unwrap() else {
            unreachable!()
        };
        values
    }

    fn run(
        layout: &ak::Layout,
        text: &ng::NgramCorpus,
        max_combos: usize,
        fatfinger: bool,
    ) -> (Fit, summary::Summary) {
        let stop = AtomicBool::new(false);
        let progress = AtomicU64::new(0);
        let mut weights = Weights::default();
        weights.5 = fatfinger;
        let physical = summary::PhysicalTables::with_mode(layout, &weights, "normal");
        let settings = Settings {
            max_combos,
            ..Default::default()
        };
        let before = summary::evaluate(layout, text, &weights, &stop, &progress).unwrap();
        let fit = fit(
            layout, text, &weights, &physical, &settings, None, &stop, &progress, 2, None,
        )
        .unwrap();
        (fit, before)
    }

    // e sits above d on qwerty's left middle finger.
    const SAME_FINGER: &[u8] = b"red bed fed led wed ted ed deed seed need deer sea fun ";

    #[test]
    fn fits_a_costly_bigram_on_a_neighbour_finger_pair() {
        let layout = qwerty();
        let text = corpus(SAME_FINGER);
        let (fit, before) = run(&layout, &text, 1, false);
        assert_eq!(fit.chords.len(), 1);
        let chord = fit.chords[0];
        let (a, b) = (&layout.slots[chord.first], &layout.slots[chord.second]);
        assert!(allowed_pair(a, b, false), "{chord:?}");
        assert!(!crate::combos::fat_finger_pair(a, b));
        assert!(matches!(&chord.output, b"ed" | b"de"), "{chord:?}");
        assert!(fit.summary.score < before.score);
        assert!(fit.summary.metrics.v[SFB] < before.metrics.v[SFB]);
        // The fitted layout types that bigram on the chosen keys.
        let program = crate::combos::Program::new(&fit.layout).unwrap();
        assert_eq!(
            program.forced(&chord.output),
            vec![Some(chord.first), Some(chord.second)]
        );
    }

    #[test]
    fn fat_finger_chords_are_fitted_only_when_enabled() {
        let layout = qwerty();
        let text = corpus(SAME_FINGER);
        let (fit, _) = run(&layout, &text, 2, true);
        assert!(fit.chords.iter().any(|chord| {
            crate::combos::fat_finger_pair(&layout.slots[chord.first], &layout.slots[chord.second])
        }));
        let (fit, _) = run(&layout, &text, 2, false);
        assert!(!fit.chords.iter().any(|chord| {
            crate::combos::fat_finger_pair(&layout.slots[chord.first], &layout.slots[chord.second])
        }));
    }

    #[test]
    fn drops_combos_without_an_allowed_shape_and_never_conflicts() {
        let mut layout = qwerty();
        // e and k are on two hands; s and d are neighbouring fingers.
        layout.combos = combos(
            r#"[
                {"keys":["e","k"],"output":"th"},
                {"keys":["s","d"],"output":"ed"}
            ]"#,
        );
        let text = corpus(SAME_FINGER);
        let (none, _) = run(&layout, &text, 0, false);
        assert!(none.chords.is_empty() && none.layout.combos.is_empty());
        let (kept, _) = run(&layout, &text, 4, false);
        assert!(!kept.chords.iter().any(|chord| {
            [chord.first, chord.second] == [label_slot(&layout, "e"), label_slot(&layout, "k")]
        }));
        for (index, &a) in kept.chords.iter().enumerate() {
            for &b in &kept.chords[index + 1..] {
                assert!(!conflicts(a, b), "{a:?} {b:?}");
            }
        }
    }

    #[test]
    fn screened_fits_and_start_mappings_match_exact_fits() {
        let stop = AtomicBool::new(false);
        let progress = AtomicU64::new(0);
        let weights = Weights::default();
        let layout = qwerty();
        let text = b"red bed fed led wed ted ed deed seed need deer sea fun the then they \
            ice nice dice vice cede kid kiss loss lol polo ";
        let mut moved = layout.clone();
        moved.swap(label_slot(&layout, "e"), label_slot(&layout, "i"));
        let screening = |on: bool| SCREENING.with(|state| state.set((on, 0)));
        let spared = || SCREENING.with(|state| state.get().1);
        let mut total_spared = 0;
        // Whole counts reuse start mappings; fractions start afresh.
        for scale in [1.0, 0.17] {
            let tables = ak::text_ngrams(text);
            let names = ["letters", "bigrams", "trigrams", "fourgrams", "fivegrams"];
            let fields: Vec<_> = (0..5)
                .map(|n| {
                    let entries: Vec<_> = tables[n]
                        .iter()
                        .map(|(gram, count)| {
                            format!("{}:{}", ak::quote(gram), *count as f64 * scale)
                        })
                        .collect();
                    format!("\"{}\":{{{}}}", names[n], entries.join(","))
                })
                .collect();
            let text = ng::NgramCorpus::from_text(
                &format!("{{{}}}", fields.join(",")),
                Path::new("screened-fit.json"),
            )
            .unwrap();
            for mode in ["normal", "aklgg"] {
                let physical = summary::PhysicalTables::with_mode(&layout, &weights, mode);
                let prepared = summary::Prepared::with_physical(
                    &moved,
                    &text,
                    &weights,
                    Arc::clone(&physical),
                )
                .unwrap();
                let (before, start) = prepared
                    .evaluate_from(&moved, None, &stop, &progress)
                    .unwrap();
                let mut capped = crate::SearchSettings::default();
                capped.sfb_limit = Some(0.0);
                capped.sfs_limit = Some(0.0);
                let caps = Some(crate::action_search::Caps {
                    baseline: &before.metrics,
                    settings: &capped,
                });
                for caps in [None, caps] {
                    let combos = |start: Option<&ng::Mappings>| {
                        let settings = Settings {
                            max_combos: 3,
                            ..Default::default()
                        };
                        let fit = fit(
                            &layout, &text, &weights, &physical, &settings, caps, &stop, &progress,
                            2, start,
                        )
                        .unwrap();
                        (
                            format!("{:?}", fit.chords),
                            fit.summary.score.to_bits(),
                            fit.evaluations,
                        )
                    };
                    let swaps = |start: Option<&ng::Mappings>| {
                        let settings = crate::swap_fit::Settings {
                            max_swaps: 3,
                            ..Default::default()
                        };
                        let fit = crate::swap_fit::fit(
                            &layout, &text, &weights, &physical, &settings, caps, &stop, &progress,
                            2, start,
                        )
                        .unwrap();
                        (
                            format!("{:?}", fit.swaps),
                            fit.summary.score.to_bits(),
                            fit.evaluations,
                        )
                    };
                    for (name, fitted) in [
                        ("combos", &combos as &dyn Fn(Option<&ng::Mappings>) -> _),
                        ("swaps", &swaps),
                    ] {
                        let context =
                            format!("{name}, scale {scale}, {mode}, caps {}", caps.is_some());
                        screening(false);
                        let reference = fitted(None);
                        assert_eq!(spared(), 0);
                        screening(true);
                        assert_eq!(fitted(None), reference, "{context}");
                        assert_eq!(fitted(start.as_ref()), reference, "{context}, start");
                        total_spared += spared();
                    }
                }
            }
        }
        assert!(total_spared > 0);
    }

    fn label_slot(layout: &ak::Layout, label: &str) -> usize {
        (0..layout.slots.len())
            .find(|&slot| layout.slots[slot].label == label)
            .unwrap()
    }
}
