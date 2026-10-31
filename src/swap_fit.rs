//! Fit a layout's adaptive swaps: up to `max_swaps` rules `swap C XY`, under
//! which the keys of X and Y exchange outputs right after the C key is
//! pressed (`doc/LAYOUTS.md`). A swap fixes a same-finger bigram "C X" by typing X on
//! Y's key, at the price of typing "C Y" on X's.
//!
//! Candidates are screened by the same-finger bigrams they remove, and the
//! best `shortlist` get exact evaluations. Each round tries removing every
//! fitted swap and adding each shortlisted one (trying every single exchange
//! when the set is full, or replacing conflicting swaps), keeps the best improvement, and stops
//! when no trial improves the score. Adaptive rules that are not mirrored
//! swap pairs are kept as they are and do not count toward the limit.

use crate::*;
use crate::{action_keys as ak, action_ngrams as ng, action_summary as summary};
use std::sync::atomic::AtomicUsize;

/// Most swaps one fit may keep.
pub(crate) const MAX_SWAPS: usize = 32;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Settings {
    pub(crate) max_swaps: usize,
    pub(crate) shortlist: usize,
    /// `[adaptive] exact`: fit exactly `max_swaps` swaps (as many as there
    /// are candidates), even where fewer would score better.
    pub(crate) exact: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            max_swaps: 4,
            shortlist: 12,
            exact: false,
        }
    }
}

/// `swap context ab`: after `context`, the keys of `a` and `b` exchange
/// outputs. `a < b`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Swap {
    pub(crate) context: u8,
    pub(crate) a: u8,
    pub(crate) b: u8,
}

impl Swap {
    fn new(context: u8, x: u8, y: u8) -> Self {
        Self {
            context,
            a: x.min(y),
            b: x.max(y),
        }
    }
}

pub(crate) struct Fit {
    pub(crate) layout: ak::Layout,
    pub(crate) summary: summary::Summary,
    pub(crate) swaps: Vec<Swap>,
    pub(crate) evaluations: usize,
}

/// Characters an adaptive key may type: printable ASCII except the reserved
/// `@ ~ | = "` and uppercase letters.
fn swappable(byte: u8) -> bool {
    byte.is_ascii_graphic()
        && !byte.is_ascii_uppercase()
        && !matches!(byte, b'@' | b'~' | b'|' | b'=' | b'"')
}

/// The character a slot types on its own: a one-character literal, or an
/// adaptive key's literal fallback.
fn own_character(layout: &ak::Layout, slot: usize) -> Option<u8> {
    match &layout.slots[slot].binding {
        ak::Binding::Text(text) if text.len() == 1 && swappable(text[0]) => Some(text[0]),
        ak::Binding::Named(name) => match layout.actions.get(name) {
            Some(ak::Action::Rules {
                basis: ak::Basis::Text,
                fallback: ak::Emission::Text(text),
                ..
            }) if text.len() == 1 && name.as_bytes() == text.as_slice() && swappable(text[0]) => {
                Some(text[0])
            }
            _ => None,
        },
        _ => None,
    }
}

/// The adaptive rules on the key typing `key`, as context -> output.
fn adaptive_rules(layout: &ak::Layout, key: u8) -> Option<&BTreeMap<Vec<u8>, ak::Emission>> {
    match layout.actions.get(&(key as char).to_string()) {
        Some(ak::Action::Rules {
            basis: ak::Basis::Text,
            rules,
            fallback: ak::Emission::Text(text),
        }) if text.as_slice() == [key] => Some(rules),
        _ => None,
    }
}

/// The layout's mirrored swap pairs, and the layout without them. Adaptive
/// keys left without rules become literal keys again.
pub(crate) fn split(layout: &ak::Layout) -> (ak::Layout, Vec<Swap>) {
    let mut swaps = Vec::new();
    for slot in 0..layout.slots.len() {
        let Some(x) = own_character(layout, slot) else {
            continue;
        };
        let Some(rules) = adaptive_rules(layout, x) else {
            continue;
        };
        for (context, emission) in rules {
            let (&[c], ak::Emission::Text(output)) = (context.as_slice(), emission) else {
                continue;
            };
            let &[y] = output.as_slice() else {
                continue;
            };
            let mirrored = adaptive_rules(layout, y)
                .is_some_and(|other| other.get(context) == Some(&ak::Emission::Text(vec![x])));
            if x < y && mirrored {
                swaps.push(Swap::new(c, x, y));
            }
        }
    }
    swaps.sort_unstable();
    let mut base = layout.clone();
    for swap in &swaps {
        for key in [swap.a, swap.b] {
            let name = (key as char).to_string();
            if let Some(ak::Action::Rules { rules, .. }) = base.actions.get_mut(&name) {
                rules.remove(&vec![swap.context]);
                if rules.is_empty() {
                    base.actions.remove(&name);
                    for slot in &mut base.slots {
                        if slot.binding == ak::Binding::Named(name.clone()) {
                            slot.binding = ak::Binding::Text(vec![key]);
                        }
                    }
                }
            }
        }
    }
    (base, swaps)
}

/// The layout's mirrored swap pairs.
pub(crate) fn swaps_of(layout: &ak::Layout) -> Vec<Swap> {
    split(layout).1
}

/// Whether `slot` is an adaptive key: a named action typing its own
/// character unless one of its rules applies.
pub(crate) fn is_adaptive_key(layout: &ak::Layout, slot: usize) -> bool {
    matches!(&layout.slots[slot].binding, ak::Binding::Named(_))
        && own_character(layout, slot).is_some()
}

/// `base` with `swaps` installed; keys gain adaptive actions as needed.
pub(crate) fn install(base: &ak::Layout, swaps: &[Swap]) -> ak::Layout {
    let mut layout = base.clone();
    for swap in swaps {
        for (key, output) in [(swap.a, swap.b), (swap.b, swap.a)] {
            let name = (key as char).to_string();
            for slot in &mut layout.slots {
                if slot.binding == ak::Binding::Text(vec![key]) {
                    slot.binding = ak::Binding::Named(name.clone());
                }
            }
            let action = layout
                .actions
                .entry(name)
                .or_insert_with(|| ak::Action::Rules {
                    basis: ak::Basis::Text,
                    rules: BTreeMap::new(),
                    fallback: ak::Emission::Text(vec![key]),
                });
            if let ak::Action::Rules { rules, .. } = action {
                rules.insert(vec![swap.context], ak::Emission::Text(vec![output]));
            }
        }
    }
    layout
}

/// Swaps that each move a frequent same-finger bigram "C X" onto another
/// finger, ranked by the bigram mass they move off minus the mass of "C Y"
/// they move on. At most `PARTNERS` partners Y are kept per bigram, so the
/// shortlist covers different bigrams rather than alternatives for one.
fn candidates(base: &ak::Layout, corpus: &ng::NgramCorpus) -> Vec<(Swap, f64)> {
    const PARTNERS: usize = 2;
    let mut masses: BTreeMap<[u8; 2], f64> = BTreeMap::new();
    for &(skip, context, target, mass) in corpus.rule_candidates() {
        if !skip {
            *masses
                .entry([context.to_ascii_lowercase(), target.to_ascii_lowercase()])
                .or_default() += mass;
        }
    }
    // The single main slot typing each character; ambiguous ones are skipped.
    let mut slot_of: BTreeMap<u8, Option<usize>> = BTreeMap::new();
    for (slot, key) in base.slots.iter().enumerate() {
        let byte = match &key.binding {
            ak::Binding::Text(text) if text.len() == 1 => text[0].to_ascii_lowercase(),
            _ => match own_character(base, slot) {
                Some(byte) => byte,
                None => continue,
            },
        };
        slot_of
            .entry(byte)
            .and_modify(|known| *known = None)
            .or_insert((key.main).then_some(slot));
    }
    let keys: Vec<(u8, usize)> = slot_of
        .iter()
        .filter_map(|(&byte, &slot)| Some((byte, slot?)))
        .filter(|&(byte, slot)| own_character(base, slot) == Some(byte))
        .collect();
    let finger = |slot: usize| base.slots[slot].finger;
    let mut result = Vec::new();
    for (&[c, x], &mass) in &masses {
        let (Some(&Some(c_slot)), Some(&(_, x_slot))) =
            (slot_of.get(&c), keys.iter().find(|&&(byte, _)| byte == x))
        else {
            continue;
        };
        if c == x || c_slot == x_slot || finger(c_slot) != finger(x_slot) {
            continue;
        }
        for &(y, y_slot) in &keys {
            if y == x || y == c || finger(y_slot) == finger(c_slot) {
                continue;
            }
            let gain = mass - masses.get(&[c, y]).copied().unwrap_or(0.0);
            if gain > 0.0 {
                result.push((Swap::new(c, x, y), gain, [c, x]));
            }
        }
    }
    result.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut partners: BTreeMap<[u8; 2], usize> = BTreeMap::new();
    let mut kept: Vec<(Swap, f64)> = Vec::new();
    for (swap, gain, bigram) in result {
        let count = partners.entry(bigram).or_default();
        if *count < PARTNERS && !kept.iter().any(|(known, _)| *known == swap) {
            *count += 1;
            kept.push((swap, gain));
        }
    }
    kept
}

/// Whether two swaps cannot coexist: the same context with a shared key
/// would give that key two outputs.
fn conflicts(a: Swap, b: Swap) -> bool {
    a.context == b.context && [a.a, a.b].iter().any(|key| [b.a, b.b].contains(key))
}

/// One random change to `layout`'s adaptive swaps for the joint search to
/// try at annealing temperature, as `combo_fit::neighbor` changes combos.
pub(crate) fn neighbor(
    layout: &ak::Layout,
    corpus: &ng::NgramCorpus,
    settings: &Settings,
    pick: &mut dyn FnMut(usize) -> usize,
) -> Option<ak::Layout> {
    let (base, mut swaps) = split(layout);
    let blocked = |swap: Swap| {
        [swap.a, swap.b].iter().any(|&key| {
            adaptive_rules(&base, key).is_some_and(|rules| rules.contains_key(&vec![swap.context]))
        })
    };
    let shortlist: Vec<Swap> = candidates(&base, corpus)
        .into_iter()
        .map(|(swap, _)| swap)
        .filter(|&swap| !blocked(swap) && !swaps.contains(&swap))
        .take(settings.shortlist)
        .collect();
    let mut kinds = Vec::new();
    if !settings.exact && !swaps.is_empty() {
        kinds.push(0);
    }
    if !settings.exact && swaps.len() < settings.max_swaps && !shortlist.is_empty() {
        kinds.push(1);
    }
    if !swaps.is_empty() && !shortlist.is_empty() {
        kinds.push(2);
    }
    if kinds.is_empty() {
        return None;
    }
    let kind = kinds[pick(kinds.len())];
    if kind != 1 {
        swaps.remove(pick(swaps.len()));
    }
    if kind != 0 {
        let open: Vec<Swap> = shortlist
            .into_iter()
            .filter(|&candidate| !swaps.iter().any(|&swap| conflicts(swap, candidate)))
            .collect();
        if open.is_empty() {
            return None;
        }
        swaps.push(open[pick(open.len())]);
    }
    swaps.sort_unstable();
    Some(install(&base, &swaps))
}

/// Fit up to `settings.max_swaps` swaps to `layout`'s current bindings,
/// scored exactly with `physical`'s objective.
/// The limit counts every swap, the layout's own included. With `caps`, a
/// set closer to within them wins before a lower score. `threads` and
/// `start` work as in `combo_fit::fit`.
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
    let (base, mut swaps) = split(layout);
    // Rules the base layout already has for a context block that key.
    let blocked = |swap: Swap| {
        [swap.a, swap.b].iter().any(|&key| {
            adaptive_rules(&base, key).is_some_and(|rules| rules.contains_key(&vec![swap.context]))
        })
    };
    let prepared = summary::Prepared::with_physical(layout, corpus, weights, Arc::clone(physical))?;
    let evaluations = AtomicUsize::new(0);
    // Each trial re-maps only the windows it changes from the current layout.
    let exact = |swaps: &[Swap], from: Option<&ng::Mappings>| {
        prepared.evaluate_from(&install(&base, swaps), from, stop, progress)
    };
    let evaluate = |swaps: &[Swap], from: Option<&ng::Mappings>| {
        evaluations.fetch_add(1, Ordering::Relaxed);
        exact(swaps, from)
    };
    let evaluate_all = |trials: &[Vec<Swap>], from: Option<&ng::Mappings>| {
        crate::combo_fit::evaluate_in_order(trials, threads, |trial| evaluate(trial, from))
    };
    // Most trials cannot be kept; their screened scores show which.
    let screen_all = |trials: &[Vec<Swap>],
                      current: &summary::Summary,
                      best: Option<&summary::Summary>,
                      cheapest: bool,
                      from: Option<&ng::Mappings>| {
        crate::combo_fit::screen_in_order(
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
                    Some(from) => {
                        prepared.screen_from(&install(&base, trial), from, current, stop, progress)
                    }
                    None => Ok(None),
                }
            },
            |trial| exact(trial, from),
        )
    };
    // `start` holds an earlier layout's mappings for this corpus.
    evaluations.fetch_add(1, Ordering::Relaxed);
    let (mut current, mut mappings) =
        prepared.evaluate_fresh(&install(&base, &swaps), start, stop, progress)?;
    // A dictionary above the limit first loses its least useful swaps.
    while swaps.len() > settings.max_swaps {
        let mut cheapest: Option<(usize, summary::Summary, Option<ng::Mappings>)> = None;
        let trials: Vec<Vec<Swap>> = (0..swaps.len())
            .map(|index| {
                let mut trial = swaps.clone();
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
            cheapest.expect("swaps above the limit are not empty");
        swaps.remove(index);
        current = summary;
        mappings = trial_mappings;
    }
    let shortlist: Vec<Swap> = candidates(&base, corpus)
        .into_iter()
        .map(|(swap, _)| swap)
        .filter(|&swap| !blocked(swap))
        .take(settings.shortlist)
        .collect();

    for _ in 0..settings.max_swaps + 2 {
        type Best = Option<(Vec<Swap>, summary::Summary, Option<ng::Mappings>)>;
        let mut best: Best = None;
        let consider = |best: &mut Best,
                        trial: Vec<Swap>,
                        (summary, trial_mappings): (summary::Summary, _)| {
            if Caps::beats(caps, &summary, &current, EPSILON)
                && best
                    .as_ref()
                    .is_none_or(|(_, known, _)| Caps::beats(caps, &summary, known, EPSILON))
            {
                *best = Some((trial, summary, trial_mappings));
            }
        };
        // Try removals independently; a useful swap may still be the right
        // one to replace when a candidate conflicts with it.
        let trials: Vec<Vec<Swap>> = (0..swaps.len())
            .map(|index| {
                let mut trial = swaps.clone();
                trial.remove(index);
                trial
            })
            .collect();
        let (evaluated, _) = screen_all(&trials, &current, None, false, mappings.as_ref())?;
        for (trial, evaluated) in trials.into_iter().zip(evaluated) {
            if let Some(evaluated) = evaluated {
                consider(&mut best, trial, evaluated);
            }
        }
        let mut trials = Vec::new();
        for &candidate in &shortlist {
            if swaps.contains(&candidate) {
                continue;
            }
            let conflicting = swaps.iter().any(|&swap| conflicts(swap, candidate));
            let removals: Vec<Option<usize>> = if conflicting || swaps.len() < settings.max_swaps {
                vec![None]
            } else {
                (0..swaps.len()).map(Some).collect()
            };
            for remove in removals {
                let mut trial = swaps.clone();
                if let Some(index) = remove {
                    trial.remove(index);
                }
                trial.retain(|&swap| !conflicts(swap, candidate));
                trial.push(candidate);
                trial.sort_unstable();
                trials.push(trial);
            }
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
        swaps = trial;
        current = summary;
        mappings = trial_mappings;
    }
    // `exact`: fill up to the count with the least costly candidates.
    while settings.exact && swaps.len() < settings.max_swaps {
        let mut best: Option<(Vec<Swap>, summary::Summary, Option<ng::Mappings>)> = None;
        let trials: Vec<Vec<Swap>> = shortlist
            .iter()
            .filter(|&&candidate| {
                !swaps.contains(&candidate) && !swaps.iter().any(|&swap| conflicts(swap, candidate))
            })
            .map(|&candidate| {
                let mut trial = swaps.clone();
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
        swaps = trial;
        current = summary;
        mappings = trial_mappings;
    }
    Ok(Fit {
        layout: install(&base, &swaps),
        summary: current,
        swaps,
        evaluations: evaluations.into_inner(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const QWERTY: &str =
        "q w e r t  y u i o p\na s d f g  h j k l ;\nz x c v b  n m , . /\nthumbs: space\n";

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
        ng::NgramCorpus::from_text(&json, Path::new("swap-fit.json")).unwrap()
    }

    fn fit_swaps(
        layout: &ak::Layout,
        text: &ng::NgramCorpus,
        max_swaps: usize,
    ) -> (Fit, summary::Summary) {
        let stop = AtomicBool::new(false);
        let progress = AtomicU64::new(0);
        let weights = Weights::default();
        let physical = summary::PhysicalTables::with_mode(layout, &weights, "normal");
        let settings = Settings {
            max_swaps,
            ..Default::default()
        };
        let before = summary::evaluate(layout, text, &weights, &stop, &progress).unwrap();
        let fit = fit(
            layout, text, &weights, &physical, &settings, None, &stop, &progress, 2, None,
        )
        .unwrap();
        (fit, before)
    }

    #[test]
    fn full_sets_can_replace_a_useful_conflicting_swap() {
        let base = ak::Layout::parse(QWERTY, Path::new("replacement.dat")).unwrap();
        let layout = install(
            &base,
            &[Swap::new(b'e', b'd', b'q'), Swap::new(b'u', b'j', b'p')],
        );
        let text = corpus(
            format!(
                "{}{}{}{}",
                "ed ".repeat(100),
                "eq ".repeat(20),
                "uj ".repeat(5),
                "up "
            )
            .as_bytes(),
        );
        let mut weights = Weights::new([0.0; N_WEIGHTS]);
        weights.0[SFB] = 1.0;
        let physical = summary::PhysicalTables::with_mode(&layout, &weights, "normal");
        let fitted = fit(
            &layout,
            &text,
            &weights,
            &physical,
            &Settings {
                max_swaps: 2,
                ..Default::default()
            },
            None,
            &AtomicBool::new(false),
            &AtomicU64::new(0),
            1,
            None,
        )
        .unwrap();
        // e:dq fixes many ed pairs, but creates SFBs on eq. Its replacement
        // must remove that useful swap, although u:jp is cheaper to remove.
        assert_eq!(fitted.summary.metrics.v[SFB], 0.0);
        assert!(!fitted.swaps.contains(&Swap::new(b'e', b'd', b'q')));
        assert_eq!(fitted.swaps.len(), 2);
    }

    #[test]
    fn capped_fits_never_leave_the_caps_for_a_better_score() {
        let layout = ak::Layout::parse(QWERTY, Path::new("swap-fit.dat")).unwrap();
        let text = corpus(b"red bed fed led wed ted ed deer seed need deed sea fun ");
        let stop = AtomicBool::new(false);
        let progress = AtomicU64::new(0);
        let weights = Weights::default();
        let physical = summary::PhysicalTables::with_mode(&layout, &weights, "normal");
        let before = summary::evaluate(&layout, &text, &weights, &stop, &progress).unwrap();
        let fitted = |caps| {
            fit(
                &layout,
                &text,
                &weights,
                &physical,
                &Settings::default(),
                caps,
                &stop,
                &progress,
                1,
                None,
            )
            .unwrap()
        };
        let free = fitted(None);
        // Cap every metric the free fit made worse at the layout's own value.
        let mut search = SearchSettings::default();
        let worse =
            |metric: usize| free.summary.metrics.v[metric] > before.metrics.v[metric] + 1e-10;
        search.sfb_limit = worse(SFB).then_some(0.0);
        search.sfs_limit = worse(SFS).then_some(0.0);
        search.travel_limit = worse(TRAVEL).then_some(0.0);
        search.sftravel_limit = worse(SFTRAVEL).then_some(0.0);
        assert!(
            [
                search.sfb_limit,
                search.sfs_limit,
                search.travel_limit,
                search.sftravel_limit
            ]
            .iter()
            .any(Option::is_some),
            "the free fit makes nothing worse"
        );
        let capped = fitted(Some(crate::action_search::Caps {
            baseline: &before.metrics,
            settings: &search,
        }));
        assert_eq!(
            crate::action_search::overshoot(&capped.summary.metrics, &before.metrics, &search),
            0.0
        );
        assert!(capped.summary.score <= before.score);
    }

    #[test]
    fn mirrored_swaps_are_recognized_and_reinstalled_exactly() {
        let source = format!("{QWERTY}swap h nr\nadaptive t ex\n");
        let layout = ak::Layout::parse(&source, Path::new("swaps.dat")).unwrap();
        // The one-way adaptive rule on t is not a swap and stays.
        assert_eq!(swaps_of(&layout), vec![Swap::new(b'h', b'n', b'r')]);
        let (base, swaps) = split(&layout);
        assert!(base.actions.contains_key("t") && !base.actions.contains_key("n"));
        let n = layout
            .slots
            .iter()
            .position(|slot| slot.label == "n")
            .unwrap();
        assert_eq!(base.slots[n].binding, ak::Binding::Text(b"n".to_vec()));
        let restored = install(&base, &swaps);
        assert_eq!(restored.actions, layout.actions);
        assert_eq!(restored.slots, layout.slots);
        assert!(is_adaptive_key(&layout, n));
    }

    #[test]
    fn fits_a_swap_for_a_frequent_same_finger_bigram() {
        let layout = ak::Layout::parse(QWERTY, Path::new("swap-fit.dat")).unwrap();
        // e sits above d on the left middle finger.
        let text = corpus(b"red bed fed led wed ted ed deer seed need deed sea fun ");
        let (fit, before) = fit_swaps(&layout, &text, 1);
        assert_eq!(fit.swaps.len(), 1);
        let swap = fit.swaps[0];
        assert!(matches!(swap.context, b'e' | b'd'), "{swap:?}");
        assert!(fit.summary.score < before.score);
        assert!(fit.summary.metrics.v[SFB] < before.metrics.v[SFB]);
        // The fitted layout saves and loads back with the same swap.
        let saved = crate::layout_export::jsonc_text(&fit.layout).unwrap();
        let loaded = ak::Layout::parse(&saved, Path::new("swap-fit.jsonc")).unwrap();
        assert_eq!(swaps_of(&loaded), fit.swaps);
    }

    #[test]
    fn fitted_swaps_never_give_a_key_two_outputs() {
        let layout = ak::Layout::parse(QWERTY, Path::new("swap-fit.dat")).unwrap();
        // Several same-finger bigrams after e and after u.
        let text = corpus(b"ed ec ed ec de dec uj um uy juy ed um uj fun sea ");
        // The screen offers swaps that would give one key two outputs.
        let offered: Vec<Swap> = candidates(&layout, &text)
            .into_iter()
            .map(|(swap, _)| swap)
            .collect();
        assert!(offered
            .iter()
            .enumerate()
            .any(|(index, &a)| { offered[index + 1..].iter().any(|&b| conflicts(a, b)) }));
        let (fit, _) = fit_swaps(&layout, &text, 4);
        assert!(!fit.swaps.is_empty());
        for (index, &a) in fit.swaps.iter().enumerate() {
            for &b in &fit.swaps[index + 1..] {
                assert!(!conflicts(a, b), "{a:?} {b:?}");
            }
        }
        assert_eq!(swaps_of(&fit.layout), fit.swaps);
    }

    #[test]
    fn swaps_conflict_only_on_a_shared_key_in_one_context() {
        let swap = |context: u8, x: u8, y: u8| Swap::new(context, x, y);
        assert!(conflicts(swap(b'e', b'd', b'q'), swap(b'e', b'c', b'q')));
        assert!(conflicts(swap(b'e', b'd', b'q'), swap(b'e', b'q', b'z')));
        assert!(!conflicts(swap(b'e', b'd', b'q'), swap(b'u', b'd', b'q')));
        assert!(!conflicts(swap(b'e', b'd', b'q'), swap(b'e', b'c', b'z')));
    }
}
