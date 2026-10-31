//! Greedy rule design using the existing physical mapper and detailed scorer.
use crate::*;
use crate::{action_keys as ak, action_ngrams as ng, action_summary as summary};

/// One rule per supported previous-character context. The designer currently
/// accepts printable ASCII except uppercase letters: 94 graphic bytes - 26.
pub(crate) const MAX_RULES: usize = 68;

#[derive(Clone, Debug)]
pub(crate) struct Settings {
    pub roots: Vec<usize>,
    pub locked_slots: Vec<usize>,
    pub max_rules: usize,
    pub shortlist: usize,
    pub sfb_limit: Option<f64>,
    pub sfs_limit: Option<f64>,
    pub travel_limit: Option<f64>,
    pub sftravel_limit: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlacementMode {
    ExtraBothPinkies,
    Existing,
}

#[derive(Clone, Debug)]
pub(crate) struct IterativeSettings {
    pub rule_settings: Settings,
    /// Zero uses the host's available parallelism, matching the main optimizer.
    pub workers: usize,
    pub passes: usize,
    pub archive: usize,
    pub placement: PlacementMode,
}

impl Default for IterativeSettings {
    fn default() -> Self {
        Self {
            rule_settings: Settings {
                roots: Vec::new(),
                max_rules: 5,
                shortlist: 24,
                ..Settings::default()
            },
            workers: 0,
            passes: 3,
            archive: 8,
            placement: PlacementMode::ExtraBothPinkies,
        }
    }
}

pub(crate) struct Candidate {
    pub layout: ak::Layout,
    pub score: f64,
    pub rules_off_score: Option<f64>,
    pub rules_off_note: Option<String>,
    pub raw: Raw,
    pub totals: [f64; 4],
    pub metrics: Metrics,
    pub root: usize,
    pub rules: Vec<Rule>,
    pub pass: usize,
}

pub(crate) struct DesignArchive {
    pub baseline: ak::Layout,
    pub before: summary::Summary,
    pub candidates: Vec<Candidate>,
    pub candidate_count: usize,
    pub warnings: Vec<String>,
    pub settings: IterativeSettings,
    pub source_order: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            roots: vec![0],
            locked_slots: Vec::new(),
            max_rules: 5,
            shortlist: 64,
            sfb_limit: Some(0.0),
            sfs_limit: Some(0.0),
            travel_limit: None,
            sftravel_limit: None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Rule {
    pub skip: bool,
    pub context: u8,
    pub output: u8,
    pub frequency: f64,
}

/// The exact effect of one rule in the context of all other rules currently
/// attached to the same physical root. Positive values mean the rule improves
/// the corresponding score or contribution.
#[derive(Clone, Debug)]
pub(crate) struct RuleEffect {
    pub rule: Rule,
    pub score_saved: f64,
    pub penalty_saved: f64,
    pub contribution_saved: [f64; N_WEIGHTS],
    pub metrics_without: [f64; N_METRICS],
    pub metrics_with: [f64; N_METRICS],
    pub direct_pair_saved: f64,
    pub direct_contribution_saved: [f64; N_METRICS],
}

/// Rules that repeat their one-letter context under a repeat-output fallback,
/// such as `e → e`. They type what the fallback would type after a literal
/// key, so fixed-root fitting sets them aside rather than counting them.
pub(crate) fn plain_repeats(layout: &ak::Layout, root: usize) -> BTreeMap<Vec<u8>, ak::Emission> {
    let ak::Binding::Named(name) = &layout.slots[root].binding else {
        return BTreeMap::new();
    };
    let Some(ak::Action::Rules {
        basis: ak::Basis::Text,
        rules,
        fallback: ak::Emission::Call(fallback),
    }) = layout.actions.get(name)
    else {
        return BTreeMap::new();
    };
    if !matches!(layout.actions.get(fallback), Some(ak::Action::RepeatOutput)) {
        return BTreeMap::new();
    }
    rules
        .iter()
        .filter(|(context, emission)| {
            context.len() == 1 && matches!(emission, ak::Emission::Text(text) if text == *context)
        })
        .map(|(context, emission)| (context.clone(), emission.clone()))
        .collect()
}

/// Rules of the root's dictionary excluding `plain_repeats`.
pub(crate) fn genuine_rule_count(layout: &ak::Layout, root: usize) -> usize {
    let repeats = plain_repeats(layout, root);
    match &layout.slots[root].binding {
        ak::Binding::Named(name) => match layout.actions.get(name) {
            Some(ak::Action::Rules { rules, .. }) => rules
                .keys()
                .filter(|context| !repeats.contains_key(*context))
                .count(),
            _ => 0,
        },
        _ => 0,
    }
}

/// Refresh requests may keep rule slots empty during setup, but an optimized
/// result must refill every requested slot with a genuine rule.
pub(crate) fn rule_counts_satisfied(layout: &ak::Layout) -> bool {
    layout.magic_refit_pending.is_empty()
        && layout.magic_rule_slots.iter().all(|(name, slots)| {
            layout.slots.iter().enumerate().any(|(root, slot)| {
                matches!(&slot.binding, ak::Binding::Named(bound) if bound == name)
                    && genuine_rule_count(layout, root) == slots.len()
                    && original(layout, root).is_some_and(|(rules, _)| {
                        slots.iter().all(|context| rules.contains_key(context))
                    })
            })
        })
}

/// The layout with the root's `plain_repeats` removed: the dictionary that
/// fitting and support checks work on.
fn without_plain_repeats(layout: &ak::Layout, root: usize) -> ak::Layout {
    let repeats = plain_repeats(layout, root);
    let mut genuine = layout.clone();
    edit_rules(&mut genuine, root, |rules| {
        rules.retain(|context, _| !repeats.contains_key(context))
    });
    genuine
}

/// Edit the root's rule dictionary in place.
fn edit_rules(
    layout: &mut ak::Layout,
    root: usize,
    edit: impl FnOnce(&mut BTreeMap<Vec<u8>, ak::Emission>),
) {
    if let ak::Binding::Named(name) = &layout.slots[root].binding {
        if let Some(ak::Action::Rules { rules, .. }) = layout.actions.get_mut(name) {
            edit(rules);
        }
    }
}

pub(crate) fn original(
    layout: &ak::Layout,
    root: usize,
) -> Option<(BTreeMap<Vec<u8>, ak::Emission>, ak::Emission)> {
    let literal = |text: &[u8]| text.len() == 1 && text[0].is_ascii_graphic();
    match &layout.slots[root].binding {
        ak::Binding::Empty => Some((BTreeMap::new(), ak::Emission::Call("repeat-output".into()))),
        ak::Binding::Text(text) if literal(text) => {
            Some((BTreeMap::new(), ak::Emission::Text(text.clone())))
        }
        ak::Binding::Named(name) => match layout.actions.get(name) {
            Some(ak::Action::Rules {
                basis: ak::Basis::Text,
                rules,
                fallback,
            }) => {
                let repeat_fallback = matches!(fallback, ak::Emission::Call(n)
                    if matches!(layout.actions.get(n), Some(ak::Action::RepeatOutput)));
                // Plain repeats such as Space → Space type what a repeat
                // fallback would, so their output need not be a literal.
                let supported = rules.iter().all(|(context, e)| {
                    matches!(e, ak::Emission::Text(t)
                        if literal(t) || (repeat_fallback && context.len() == 1 && t == context))
                });
                (supported
                    && (repeat_fallback || matches!(fallback, ak::Emission::Text(t) if literal(t))))
                .then(|| (rules.clone(), fallback.clone()))
            }
            _ => None,
        },
        _ => None,
    }
}

pub(crate) fn eligible_roots(layout: &ak::Layout) -> Vec<usize> {
    (0..layout.slots.len())
        .filter(|&root| original(layout, root).is_some())
        .collect()
}

pub(crate) fn placement_roots(layout: &ak::Layout) -> Vec<usize> {
    if layout.slots.iter().enumerate().any(|(i, s)| {
        matches!(s.binding, ak::Binding::Named(_))
            && crate::layout_io::dedicated_magic_label(&s.label)
            && original(layout, i).is_some()
    }) {
        (0..layout.slots.len())
            .filter(|&i| {
                matches!(&layout.slots[i].binding,ak::Binding::Text(t) if t.len()==1)
                    || matches!(layout.slots[i].binding, ak::Binding::Empty)
                    || (matches!(layout.slots[i].binding, ak::Binding::Named(_))
                        && crate::layout_io::dedicated_magic_label(&layout.slots[i].label)
                        && original(layout, i).is_some())
            })
            .collect()
    } else {
        eligible_roots(layout)
    }
}

#[cfg(test)]
fn with_rules(layout: &ak::Layout, root: usize, rules: &[Rule]) -> ak::Result<ak::Layout> {
    let mut result = layout.clone();
    let (mut ordinary, fallback) = original(layout, root)
        .ok_or("This physical key cannot preserve its fallback in the supported magic format.")?;
    let action_name = |glyph: u8| match glyph {
        b'@' => "magic".into(),
        b'$' => "skip".into(),
        _ => (glyph as char).to_string(),
    };
    let available = |glyph: u8| {
        let label = (glyph as char).to_string();
        !layout
            .slots
            .iter()
            .enumerate()
            .any(|(i, slot)| i != root && slot.label == label)
            && !layout.actions.contains_key(&action_name(glyph))
    };
    let old_label = layout.slots[root].label.as_bytes();
    let glyph =
        if old_label.len() == 1 && old_label[0].is_ascii_graphic() && available(old_label[0]) {
            old_label[0]
        } else {
            std::iter::once(b'@')
                .chain(33..=126)
                .filter(|&g| g != b'$')
                .find(|&g| available(g))
                .ok_or("No unused printable label is available for magic design.")?
        };
    for rule in rules {
        if rule.skip || ordinary.contains_key(&vec![rule.context]) {
            return Err("Magic design cannot overwrite an existing context.".into());
        }
        ordinary.insert(vec![rule.context], ak::Emission::Text(vec![rule.output]));
    }
    if matches!(&fallback, ak::Emission::Call(n) if n == "repeat-output") {
        match result.actions.get("repeat-output") {
            Some(ak::Action::RepeatOutput) => {}
            Some(_) => {
                return Err(
                    "The repeat-output action name is already used by another action.".into(),
                );
            }
            None => {
                result
                    .actions
                    .insert("repeat-output".into(), ak::Action::RepeatOutput);
            }
        }
    }
    let name = action_name(glyph);
    result.actions.insert(
        name.clone(),
        ak::Action::Rules {
            basis: ak::Basis::Text,
            rules: ordinary,
            fallback,
        },
    );
    result.slots[root].binding = ak::Binding::Named(name);
    result.slots[root].label = (glyph as char).to_string();
    result.action_mode = true;
    crate::layout_export::jsonc_text(&result)?;
    Ok(result)
}

fn feasible(
    candidate: &summary::Summary,
    baseline: &summary::Summary,
    settings: &Settings,
) -> bool {
    if candidate.counts.ignored_characters > baseline.counts.ignored_characters + 1e-10 {
        return false;
    }
    [
        (SFB, settings.sfb_limit),
        (SFS, settings.sfs_limit),
        (TRAVEL, settings.travel_limit),
        (SFTRAVEL, settings.sftravel_limit),
    ]
    .into_iter()
    .all(|(metric, limit)| {
        limit.is_none_or(|delta| {
            candidate.metrics.v[metric] <= baseline.metrics.v[metric] + delta + 1e-10
        })
    })
}

/// `feasible` with every cap and the coverage limit loosened by `relative`.
fn feasible_within(
    candidate: &summary::Summary,
    baseline: &summary::Summary,
    settings: &Settings,
    relative: f64,
) -> bool {
    let loose = |limit: f64| limit + relative * (1.0 + limit.abs());
    candidate.counts.ignored_characters <= loose(baseline.counts.ignored_characters + 1e-10)
        && [
            (SFB, settings.sfb_limit),
            (SFS, settings.sfs_limit),
            (TRAVEL, settings.travel_limit),
            (SFTRAVEL, settings.sftravel_limit),
        ]
        .into_iter()
        .all(|(metric, limit)| {
            limit.is_none_or(|delta| {
                candidate.metrics.v[metric] <= loose(baseline.metrics.v[metric] + delta + 1e-10)
            })
        })
}

fn feasible_metrics(candidate: &Metrics, baseline: &summary::Summary, settings: &Settings) -> bool {
    [
        (SFB, settings.sfb_limit),
        (SFS, settings.sfs_limit),
        (TRAVEL, settings.travel_limit),
        (SFTRAVEL, settings.sftravel_limit),
    ]
    .into_iter()
    .all(|(metric, limit)| {
        limit.is_none_or(|delta| candidate.v[metric] <= baseline.metrics.v[metric] + delta + 1e-10)
    })
}

/// Make a deliberately small ordinary-layout adjustment around a fitted magic
/// key. The mapper is built once and every pair is scored incrementally. One
/// best binding swap is committed per sweep, so this remains bounded even when
/// the magic designer considers every possible root.
#[cfg(test)]
fn quick_optimize_bindings(
    layout: &ak::Layout,
    root: usize,
    corpus: &ng::NgramCorpus,
    weights: &Weights,
    baseline: &summary::Summary,
    settings: &Settings,
    stop: &AtomicBool,
) -> ak::Result<(ak::Layout, usize)> {
    let physical = summary::PhysicalTables::new(layout, weights);
    quick_optimize_bindings_with_physical(
        layout, root, corpus, weights, baseline, settings, stop, &physical,
    )
}

fn quick_optimize_bindings_with_physical(
    layout: &ak::Layout,
    root: usize,
    corpus: &ng::NgramCorpus,
    weights: &Weights,
    baseline: &summary::Summary,
    settings: &Settings,
    stop: &AtomicBool,
    physical: &Arc<summary::PhysicalTables>,
) -> ak::Result<(ak::Layout, usize)> {
    if !physical.matches(layout, weights) {
        return Err("shared action tables require unchanged physical slots and weights".into());
    }

    const SWEEPS: usize = 2;

    let effort = &physical.effort;
    let geometry = Arc::clone(&physical.geometry);
    let mut cache = ng::Incremental::new_with_shared_geometry(
        corpus,
        layout,
        geometry,
        stop,
        weights.alts(),
        weights.combo_policy(),
        |a, b, key| effort.get(a, b, key),
    )?;
    // The selected magic root and other named actions stay on their physical
    // slots. Ordinary text and empty bindings can move throughout the board,
    // including thumb keys and the available outer columns.
    let free: Vec<_> = layout
        .slots
        .iter()
        .enumerate()
        .filter_map(|(slot, value)| {
            (slot != root
                && !settings.locked_slots.contains(&slot)
                && matches!(value.binding, ak::Binding::Text(_) | ak::Binding::Empty))
            .then_some(slot)
        })
        .collect();
    let mut optimized = layout.clone();
    let mut current_score = cache.score(weights).score;
    let mut attempted = 0usize;
    let mut proposal = ng::Proposal::new();
    for _ in 0..SWEEPS {
        let mut winner: Option<(f64, usize, usize)> = None;
        for left in 0..free.len() {
            for right in left + 1..free.len() {
                if stop.load(Ordering::Relaxed) {
                    return Err("cancelled".into());
                }
                attempted += 1;
                cache.propose_swap(
                    free[left],
                    free[right],
                    stop,
                    |a, b, key| effort.get(a, b, key),
                    &mut proposal,
                )?;
                let score = proposal.score(weights).score;
                if score < current_score - 1e-10
                    && feasible_metrics(&proposal.metrics(), baseline, settings)
                    && winner
                        .as_ref()
                        .is_none_or(|(best, _, _)| score < *best - 1e-10)
                {
                    winner = Some((score, free[left], free[right]));
                }
            }
        }
        let Some((_, left, right)) = winner else {
            break;
        };
        // Recreate the winning proposal because the reusable scratch proposal
        // now contains the final pair inspected above.
        cache.propose_swap(
            left,
            right,
            stop,
            |a, b, key| effort.get(a, b, key),
            &mut proposal,
        )?;
        current_score = proposal.score(weights).score;
        cache.commit(&mut proposal);
        optimized.swap(left, right);
    }
    Ok((optimized, attempted))
}

const JOINT_POLISH_SEEDS: usize = 8;
const JOINT_POLISH_ROUNDS: usize = 3;
const JOINT_SWAP_SHORTLIST: usize = 10;
const JOINT_SEQUENCE_STARTS: usize = 3;
const JOINT_SEQUENCE_SHORTLIST: usize = 4;
const ROOT_RELOCATION_SHORTLIST: usize = 4;
const ROOT_RELOCATION_FINALISTS: usize = 2;

#[derive(Clone, Copy)]
struct JointSwap {
    adjacent_mass: f64,
    optimistic_score: f64,
    fixed_score: f64,
    left: usize,
    right: usize,
}

#[derive(Clone, Copy)]
struct RootMove {
    adjacent_mass: f64,
    optimistic_score: f64,
    fixed_score: f64,
    destination: usize,
}

/// Estimate how much of a swap's newly-created adjacent movement penalty a
/// refitted ordinary magic rule could conceivably remove. This deliberately
/// ignores rewards and non-adjacent statistics: it only widens the shortlist;
/// exact rule fitting, pruning, caps, and the complete score still decide
/// whether the swap is accepted.
fn recoverable_adjacent_penalty(
    incumbent: &Metrics,
    candidate: &Metrics,
    weights: &Weights,
) -> f64 {
    [
        SFB, LSB, DAFJB, CAFJB, DNFJB, CNFJB, DAHJB, CAHJB, DNHJB, CNHJB, SFTRAVEL,
    ]
    .into_iter()
    .map(|metric| {
        let weight = weights.0[metric].max(0.0);
        ((candidate.v[metric] - incumbent.v[metric]) * weight).max(0.0)
    })
    .sum()
}

/// Heuristically compare a bounded dictionary allocation with the dictionary
/// already included in the fixed score, at the same candidate bindings. Pair
/// costs omit corpus interactions; exact fitting and acceptance remain decisive.
#[allow(clippy::too_many_arguments)]
fn capacity_recoverable_adjacent_penalty(
    layout: &ak::Layout,
    root: usize,
    proposals: &[Rule],
    prepared: &summary::Prepared<'_>,
    settings: &Settings,
    adjacent_mass: f64,
    weights: &Weights,
    stop: &AtomicBool,
    progress: &AtomicU64,
) -> ak::Result<f64> {
    if adjacent_mass <= 0.0 {
        return Ok(0.0);
    }
    let Some((rules, fallback)) = original(layout, root) else {
        return Ok(0.0);
    };
    let ak::Binding::Named(action_name) = &layout.slots[root].binding else {
        return Ok(0.0);
    };
    let metrics = [
        SFB, LSB, DAFJB, CAFJB, DNFJB, CNFJB, DAHJB, CAHJB, DNHJB, CNHJB, SFTRAVEL,
    ];
    // Literal positions provide only a cheap proposal priority. The normal
    // physical mapper below resolves alternate fingers, actions, and combos.
    let keys = crate::action_ui::physical_keys(layout);
    let mut literals: BTreeMap<u8, Vec<usize>> = BTreeMap::new();
    for (slot, value) in layout.slots.iter().enumerate() {
        if let ak::Binding::Text(text) = &value.binding {
            if text.len() == 1 {
                literals.entry(text[0]).or_default().push(slot);
            }
        }
    }
    let mut ranked: Vec<_> = proposals
        .iter()
        .filter(|rule| !rule.skip)
        .map(|rule| {
            let mut potential = 0.0_f64;
            if let (Some(contexts), Some(outputs)) =
                (literals.get(&rule.context), literals.get(&rule.output))
            {
                for &a in contexts {
                    for &b in outputs {
                        let before = pair_flags(keys[a], keys[b], a == b);
                        let after = pair_flags(keys[a], keys[root], a == root);
                        let gain: f64 = metrics
                            .iter()
                            .filter(|&&metric| metric != SFTRAVEL)
                            .map(|&metric| {
                                let old = if before.bi & bit(metric) != 0 {
                                    1.0
                                } else {
                                    0.0
                                };
                                let new = if after.bi & bit(metric) != 0 {
                                    1.0
                                } else {
                                    0.0
                                };
                                (old - new) * weights.0[metric]
                            })
                            .sum();
                        potential = potential.max(gain);
                    }
                }
            }
            (potential.max(0.0) * rule.frequency, rule)
        })
        .collect();
    ranked.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then_with(|| b.1.frequency.total_cmp(&a.1.frequency))
            .then_with(|| (a.1.context, a.1.output).cmp(&(b.1.context, b.1.output)))
    });
    let mut selected: BTreeMap<(u8, u8), &Rule> = ranked
        .into_iter()
        .take(settings.shortlist.clamp(24, 48))
        .map(|(_, rule)| ((rule.context, rule.output), rule))
        .collect();
    // Existing allocations must remain visible, including the benefit lost
    // when a replacement consumes their context or the rule budget.
    for rule in proposals.iter().filter(|rule| !rule.skip) {
        if rules.get(&vec![rule.context]) == Some(&ak::Emission::Text(vec![rule.output])) {
            selected.insert((rule.context, rule.output), rule);
        }
    }
    let mut contexts: BTreeMap<u8, f64> = BTreeMap::new();
    let mut existing_benefit = 0.0;
    for rule in selected.into_values() {
        let mut without = layout.clone();
        let mut trial_rules = rules.clone();
        trial_rules.remove(&vec![rule.context]);
        without.actions.insert(
            action_name.clone(),
            ak::Action::Rules {
                basis: ak::Basis::Text,
                rules: trial_rules.clone(),
                fallback: fallback.clone(),
            },
        );
        let mut with = without.clone();
        trial_rules.insert(vec![rule.context], ak::Emission::Text(vec![rule.output]));
        with.actions.insert(
            action_name.clone(),
            ak::Action::Rules {
                basis: ak::Basis::Text,
                rules: trial_rules,
                fallback: fallback.clone(),
            },
        );
        let pair = [rule.context, rule.output];
        let before =
            prepared.pair_movement_costs_binding_variation(&without, pair, stop, progress)?;
        let after = prepared.pair_movement_costs_binding_variation(&with, pair, stop, progress)?;
        let gain = metrics
            .iter()
            .map(|&metric| before[metric] - after[metric])
            .sum::<f64>()
            * rule.frequency;
        if !gain.is_finite() {
            continue;
        }
        if rules.get(&vec![rule.context]) == Some(&ak::Emission::Text(vec![rule.output])) {
            existing_benefit += gain;
        }
        contexts
            .entry(rule.context)
            .and_modify(|best| *best = best.max(gain))
            .or_insert(gain.max(0.0));
    }
    let mut gains: Vec<_> = contexts.into_values().collect();
    gains.sort_by(|a, b| b.total_cmp(a));
    let fitted_benefit = gains.into_iter().take(settings.max_rules).sum::<f64>();
    Ok((fitted_benefit - existing_benefit).max(0.0) * 100.0 / adjacent_mass)
}

/// Deliberately create literal same-finger pairs that the fixed magic root
/// can repair. The physical mapper screens a small, context-diverse budget.
#[allow(clippy::too_many_arguments)]
fn targeted_repair_swaps(
    layout: &ak::Layout,
    root: usize,
    proposals: &[Rule],
    free: &[usize],
    prepared: &summary::Prepared<'_>,
    weights: &Weights,
    stop: &AtomicBool,
    progress: &AtomicU64,
) -> ak::Result<Vec<(usize, usize)>> {
    // Physical modes rank by their own slope-weighted pair cost instead.
    if !prepared.physical_mode() && weights.0[SFB] <= 0.0 {
        return Ok(Vec::new());
    }
    let Some((rules, fallback)) = original(layout, root) else {
        return Ok(Vec::new());
    };
    let ak::Binding::Named(action_name) = &layout.slots[root].binding else {
        return Ok(Vec::new());
    };
    let literal_slots = |byte| {
        layout
            .slots
            .iter()
            .enumerate()
            .filter_map(move |(index, slot)| {
                matches!(&slot.binding, ak::Binding::Text(text) if text.as_slice() == [byte])
                    .then_some(index)
            })
            .collect::<Vec<_>>()
    };
    let mut templates = Vec::new();
    for rule in proposals
        .iter()
        .filter(|rule| !rule.skip && rule.context != rule.output)
    {
        if stop.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        for context in literal_slots(rule.context) {
            if layout.slots[context].finger == layout.slots[root].finger {
                continue;
            }
            for output in literal_slots(rule.output)
                .into_iter()
                .filter(|slot| free.contains(slot))
            {
                // Only deliberate newly-created SFBs belong in this reserve.
                if layout.slots[output].finger == layout.slots[context].finger {
                    continue;
                }
                for &destination in free {
                    if destination == context
                        || destination == output
                        || layout.slots[destination].finger != layout.slots[context].finger
                    {
                        continue;
                    }
                    let (left, right) = (output.min(destination), output.max(destination));
                    templates.push((
                        rule.frequency * weights.0[SFB],
                        rule.context,
                        rule.output,
                        left,
                        right,
                        rule.frequency,
                    ));
                }
            }
        }
    }
    templates.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then_with(|| (a.1, a.2, a.3, a.4).cmp(&(b.1, b.2, b.3, b.4)))
    });
    templates.dedup_by(|a, b| (a.1, a.2, a.3, a.4) == (b.1, b.2, b.3, b.4));
    let mut contexts = std::collections::BTreeSet::new();
    let mut first = Vec::new();
    let mut alternates = Vec::new();
    for template in templates {
        if contexts.insert(template.1) {
            first.push(template);
        } else {
            alternates.push(template);
        }
    }
    let mut screened = Vec::new();
    for (_, context, output, left, right, frequency) in first.into_iter().chain(alternates).take(24)
    {
        let mut without = layout.clone();
        without.swap(left, right);
        let mut trial_rules = rules.clone();
        trial_rules.remove(&vec![context]);
        without.actions.insert(
            action_name.clone(),
            ak::Action::Rules {
                basis: ak::Basis::Text,
                rules: trial_rules.clone(),
                fallback: fallback.clone(),
            },
        );
        let mut with = without.clone();
        trial_rules.insert(vec![context], ak::Emission::Text(vec![output]));
        with.actions.insert(
            action_name.clone(),
            ak::Action::Rules {
                basis: ak::Basis::Text,
                rules: trial_rules,
                fallback: fallback.clone(),
            },
        );
        let before = prepared.pair_movement_costs_binding_variation(
            &without,
            [context, output],
            stop,
            progress,
        )?;
        let after = prepared.pair_movement_costs_binding_variation(
            &with,
            [context, output],
            stop,
            progress,
        )?;
        let gain: f64 = [
            SFB, LSB, DAFJB, CAFJB, DNFJB, CNFJB, DAHJB, CAHJB, DNHJB, CNHJB, SFTRAVEL,
        ]
        .into_iter()
        .map(|metric| before[metric] - after[metric])
        .sum();
        if before[SFB] - after[SFB] > 1e-10 && gain.is_finite() && gain > 1e-10 {
            let weighted_gain = gain * frequency;
            if weighted_gain.is_finite() && weighted_gain > 0.0 {
                screened.push((weighted_gain, context, left, right));
            }
        }
    }
    screened.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then_with(|| (a.1, a.2, a.3).cmp(&(b.1, b.2, b.3)))
    });
    let mut contexts = std::collections::BTreeSet::new();
    let mut endpoints = std::collections::BTreeSet::new();
    let mut selected = Vec::new();
    let mut alternates = Vec::new();
    for (_, context, left, right) in screened {
        if !endpoints.insert((left, right)) {
            continue;
        }
        if contexts.insert(context) {
            selected.push((left, right));
        } else {
            alternates.push((left, right));
        }
    }
    selected.extend(alternates);
    selected.truncate(6);
    Ok(selected)
}

fn joint_swap_order(a: &JointSwap, b: &JointSwap) -> std::cmp::Ordering {
    a.optimistic_score
        .total_cmp(&b.optimistic_score)
        .then_with(|| a.fixed_score.total_cmp(&b.fixed_score))
        .then_with(|| (a.left, a.right).cmp(&(b.left, b.right)))
}

fn root_move_order(a: &RootMove, b: &RootMove) -> std::cmp::Ordering {
    a.optimistic_score
        .total_cmp(&b.optimistic_score)
        .then_with(|| a.fixed_score.total_cmp(&b.fixed_score))
        .then_with(|| a.destination.cmp(&b.destination))
}

/// Jointly polish one fixed-root candidate. The fast placement phase judges
/// swaps with the currently fitted rules, which can reject a useful layout
/// change before rules get a chance to adapt. Here each round builds one
/// incremental scorer, keeps a bounded optimistic swap shortlist, then performs
/// exact rule refitting and pruning for those layouts. This is still a bounded
/// local heuristic rather than a global optimum search.
#[allow(clippy::too_many_arguments)]
fn joint_polish_candidate(
    seed: &ak::Layout,
    root: usize,
    proposals: &[Rule],
    corpus: &ng::NgramCorpus,
    weights: &Weights,
    baseline: &summary::Summary,
    settings: &Settings,
    stop: &AtomicBool,
    rule_workers: usize,
) -> ak::Result<(ak::Layout, summary::Summary, usize)> {
    let physical = summary::PhysicalTables::new(seed, weights);
    joint_polish_candidate_rounds(
        seed,
        root,
        proposals,
        corpus,
        weights,
        baseline,
        settings,
        stop,
        rule_workers,
        JOINT_POLISH_ROUNDS,
        &physical,
    )
}

#[allow(clippy::too_many_arguments)]
fn joint_polish_candidate_rounds(
    seed: &ak::Layout,
    root: usize,
    proposals: &[Rule],
    corpus: &ng::NgramCorpus,
    weights: &Weights,
    baseline: &summary::Summary,
    settings: &Settings,
    stop: &AtomicBool,
    rule_workers: usize,
    rounds: usize,

    physical: &Arc<summary::PhysicalTables>,
) -> ak::Result<(ak::Layout, summary::Summary, usize)> {
    let local_progress = AtomicU64::new(0);
    let mut current = seed.clone();
    let initial =
        summary::Prepared::with_physical(&current, corpus, weights, Arc::clone(physical))?;
    let mut current_summary = initial.evaluate(&current, stop, &local_progress)?;
    let mut attempts = 1usize;

    for _ in 0..rounds {
        if stop.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let effort = &physical.effort;
        let geometry = Arc::clone(&physical.geometry);
        let cache = ng::Incremental::new_with_shared_geometry(
            corpus,
            &current,
            geometry,
            stop,
            weights.alts(),
            weights.combo_policy(),
            |a, b, key| effort.get(a, b, key),
        )?;
        let incumbent_metrics = cache.metrics();
        let incumbent_score = cache.score(weights).score;
        let free: Vec<_> = current
            .slots
            .iter()
            .enumerate()
            .filter_map(|(slot, value)| {
                (slot != root
                    && !settings.locked_slots.contains(&slot)
                    && matches!(value.binding, ak::Binding::Text(_) | ak::Binding::Empty))
                .then_some(slot)
            })
            .collect();
        let mut shortlist = Vec::with_capacity(JOINT_SWAP_SHORTLIST + 1);
        let mut fixed_winner: Option<JointSwap> = None;
        let mut proposal = ng::Proposal::new();

        for left_index in 0..free.len() {
            for right_index in left_index + 1..free.len() {
                if stop.load(Ordering::Relaxed) {
                    return Err("cancelled".into());
                }
                attempts += 1;
                let left = free[left_index];
                let right = free[right_index];
                cache.propose_swap(
                    left,
                    right,
                    stop,
                    |a, b, key| effort.get(a, b, key),
                    &mut proposal,
                )?;
                let fixed_score = proposal.score(weights).score;
                let candidate_metrics = proposal.metrics();
                let swap = JointSwap {
                    adjacent_mass: proposal.adjacent_mass(),
                    optimistic_score: fixed_score
                        - recoverable_adjacent_penalty(
                            &incumbent_metrics,
                            &candidate_metrics,
                            weights,
                        ),
                    fixed_score,
                    left,
                    right,
                };
                shortlist.push(swap);
                shortlist.sort_by(joint_swap_order);
                shortlist.truncate(JOINT_SWAP_SHORTLIST * 2);

                if fixed_score < incumbent_score - 1e-10
                    && feasible_metrics(&candidate_metrics, baseline, settings)
                    && fixed_winner.as_ref().is_none_or(|winner| {
                        fixed_score < winner.fixed_score - 1e-10
                            || (fixed_score - winner.fixed_score).abs() <= 1e-10
                                && (left, right) < (winner.left, winner.right)
                    })
                {
                    fixed_winner = Some(swap);
                }
            }
        }

        let prepared =
            summary::Prepared::with_physical(&current, corpus, weights, Arc::clone(physical))?;
        for swap in &mut shortlist {
            let mut trial = current.clone();
            trial.swap(swap.left, swap.right);
            swap.optimistic_score = swap.fixed_score
                - capacity_recoverable_adjacent_penalty(
                    &trial,
                    root,
                    proposals,
                    &prepared,
                    settings,
                    swap.adjacent_mass,
                    weights,
                    stop,
                    &local_progress,
                )?;
        }
        shortlist.sort_by(joint_swap_order);
        shortlist.truncate(JOINT_SWAP_SHORTLIST);

        // Reserve one of the exact evaluations for the ordinary best
        // improving swap. The other entries may look temporarily worse with
        // fixed rules but promising after those rules are redesigned.
        if let Some(winner) = fixed_winner {
            if !shortlist
                .iter()
                .any(|swap| (swap.left, swap.right) == (winner.left, winner.right))
            {
                if shortlist.len() == JOINT_SWAP_SHORTLIST {
                    shortlist.pop();
                }
                shortlist.push(winner);
                shortlist.sort_by(joint_swap_order);
            }
        }

        // Screen second swaps from a few promising first swaps, even when
        // the intermediate layout worsens the score or violates a cap. Only
        // the final refitted and pruned design must satisfy the acceptance
        // policy. The fixed root and physical geometry are retained throughout.
        let mut sequences: Vec<(JointSwap, JointSwap)> = Vec::new();
        for first in shortlist.iter().take(JOINT_SEQUENCE_STARTS) {
            let mut intermediate = current.clone();
            intermediate.swap(first.left, first.right);
            let intermediate_effort = &physical.effort;
            let intermediate_geometry = Arc::clone(&physical.geometry);
            let intermediate_cache = ng::Incremental::new_with_shared_geometry(
                corpus,
                &intermediate,
                intermediate_geometry,
                stop,
                weights.alts(),
                weights.combo_policy(),
                |a, b, key| intermediate_effort.get(a, b, key),
            )?;
            let intermediate_metrics = intermediate_cache.metrics();
            for left_index in 0..free.len() {
                for right_index in left_index + 1..free.len() {
                    if stop.load(Ordering::Relaxed) {
                        return Err("cancelled".into());
                    }
                    let left = free[left_index];
                    let right = free[right_index];
                    if (left, right) == (first.left, first.right) {
                        continue;
                    }
                    // Disjoint swaps commute; keep only one ordering.
                    if left != first.left
                        && left != first.right
                        && right != first.left
                        && right != first.right
                        && (left, right) < (first.left, first.right)
                        && shortlist
                            .iter()
                            .take(JOINT_SEQUENCE_STARTS)
                            .any(|start| (start.left, start.right) == (left, right))
                    {
                        continue;
                    }
                    attempts += 1;
                    intermediate_cache.propose_swap(
                        left,
                        right,
                        stop,
                        |a, b, key| intermediate_effort.get(a, b, key),
                        &mut proposal,
                    )?;
                    let fixed_score = proposal.score(weights).score;
                    let second = JointSwap {
                        adjacent_mass: proposal.adjacent_mass(),
                        optimistic_score: fixed_score
                            - recoverable_adjacent_penalty(
                                &intermediate_metrics,
                                &proposal.metrics(),
                                weights,
                            ),
                        fixed_score,
                        left,
                        right,
                    };
                    sequences.push((*first, second));
                    sequences.sort_by(|(af, a), (bf, b)| {
                        joint_swap_order(a, b)
                            .then_with(|| (af.left, af.right).cmp(&(bf.left, bf.right)))
                    });
                    sequences.truncate(JOINT_SEQUENCE_SHORTLIST * 2);
                }
            }
        }

        for (first, second) in &mut sequences {
            let mut trial = current.clone();
            trial.swap(first.left, first.right);
            trial.swap(second.left, second.right);
            second.optimistic_score = second.fixed_score
                - capacity_recoverable_adjacent_penalty(
                    &trial,
                    root,
                    proposals,
                    &prepared,
                    settings,
                    second.adjacent_mass,
                    weights,
                    stop,
                    &local_progress,
                )?;
        }
        sequences.sort_by(|(af, a), (bf, b)| {
            joint_swap_order(a, b).then_with(|| (af.left, af.right).cmp(&(bf.left, bf.right)))
        });
        sequences.truncate(JOINT_SEQUENCE_SHORTLIST);

        let mut winner: Option<(ak::Layout, summary::Summary)> = None;
        let incumbent_acceptable_score = if feasible(&current_summary, baseline, settings)
            && original(&current, root)
                .is_some_and(|(rules, _)| !rules.is_empty() && rules.len() <= settings.max_rules)
        {
            current_summary.score
        } else {
            f64::INFINITY
        };
        // These exact refits have a separate budget: fixed-rule score must not
        // discard a swap whose purpose is to create a repairable literal pair.
        let mut targeted = Vec::new();
        for (left, right) in targeted_repair_swaps(
            &current,
            root,
            proposals,
            &free,
            &prepared,
            weights,
            stop,
            &local_progress,
        )? {
            cache.propose_swap(
                left,
                right,
                stop,
                |a, b, key| effort.get(a, b, key),
                &mut proposal,
            )?;
            attempts += 1;
            let fixed_score = proposal.score(weights).score;
            targeted.push(JointSwap {
                adjacent_mass: proposal.adjacent_mass(),
                optimistic_score: fixed_score,
                fixed_score,
                left,
                right,
            });
        }
        let trials = shortlist
            .into_iter()
            .chain(targeted)
            .map(|swap| (swap, None))
            .chain(
                sequences
                    .into_iter()
                    .map(|(first, second)| (first, Some(second))),
            );
        let mut evaluated_bindings = std::collections::HashSet::new();
        for (swap, second) in trials {
            if stop.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            let mut trial = current.clone();
            trial.swap(swap.left, swap.right);
            if let Some(second) = second {
                trial.swap(second.left, second.right);
            }
            let bindings: Vec<_> = trial
                .slots
                .iter()
                .map(|slot| slot.binding.clone())
                .collect();
            if !evaluated_bindings.insert(bindings) {
                continue;
            }
            let (pruned, evaluated, fit_attempts) = refit_rules(
                &trial,
                root,
                proposals,
                settings.max_rules,
                settings.shortlist,
                corpus,
                weights,
                stop,
                &local_progress,
                rule_workers,
                Some((baseline, settings)),
                physical,
            )?;
            attempts += fit_attempts;
            let best_score = winner
                .as_ref()
                .map_or(incumbent_acceptable_score, |(_, summary)| summary.score);
            if original(&pruned, root)
                .is_some_and(|(rules, _)| !rules.is_empty() && rules.len() <= settings.max_rules)
                && evaluated.counts.ignored_characters
                    <= current_summary.counts.ignored_characters + 1e-10
                && evaluated.score.is_finite()
                && evaluated.score < best_score - 1e-10
                && feasible(&evaluated, baseline, settings)
            {
                winner = Some((pruned, evaluated));
            }
        }

        let Some((layout, evaluated)) = winner else {
            break;
        };
        current = layout;
        current_summary = evaluated;
    }

    Ok((current, current_summary, attempts))
}

/// Reconsider the physical magic location after the ordinary bindings and
/// rules have settled. Screening uses one incremental mapper with the current
/// rules. Only a small deterministic shortlist receives an exact rule refit
/// and prune, which keeps this final sweep bounded.
#[allow(clippy::too_many_arguments)]
fn relocate_polished_root(
    layout: &ak::Layout,
    root: usize,
    current_summary: summary::Summary,
    proposals: &[Rule],
    corpus: &ng::NgramCorpus,
    weights: &Weights,
    baseline: &summary::Summary,
    settings: &Settings,
    stop: &AtomicBool,
    rule_workers: usize,

    physical: &Arc<summary::PhysicalTables>,
) -> ak::Result<(ak::Layout, usize, summary::Summary, usize, bool)> {
    if !physical.matches(layout, weights) {
        return Err("shared action tables require unchanged physical slots and weights".into());
    }

    let allowed: Vec<_> = layout
        .slots
        .iter()
        .enumerate()
        .filter_map(|(destination, slot)| {
            (destination != root
                && (settings.roots.is_empty() || settings.roots.contains(&destination))
                && (matches!(&slot.binding, ak::Binding::Text(text) if text.len() == 1)
                    || matches!(slot.binding, ak::Binding::Empty)))
            .then_some(destination)
        })
        .collect();
    if allowed.is_empty() {
        return Ok((layout.clone(), root, current_summary, 0, false));
    }

    let effort = &physical.effort;
    let geometry = Arc::clone(&physical.geometry);
    let cache = ng::Incremental::new_with_shared_geometry(
        corpus,
        layout,
        geometry,
        stop,
        weights.alts(),
        weights.combo_policy(),
        |a, b, key| effort.get(a, b, key),
    )?;
    let incumbent_metrics = cache.metrics();
    let mut proposal = ng::Proposal::new();
    let mut shortlist = Vec::with_capacity(ROOT_RELOCATION_SHORTLIST + 1);
    let mut attempts = 0usize;
    for destination in allowed {
        if stop.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        attempts += 1;
        cache.propose_swap(
            root,
            destination,
            stop,
            |a, b, key| effort.get(a, b, key),
            &mut proposal,
        )?;
        let fixed_score = proposal.score(weights).score;
        let candidate_metrics = proposal.metrics();
        shortlist.push(RootMove {
            adjacent_mass: proposal.adjacent_mass(),
            optimistic_score: fixed_score
                - recoverable_adjacent_penalty(&incumbent_metrics, &candidate_metrics, weights),
            fixed_score,
            destination,
        });
        shortlist.sort_by(root_move_order);
        shortlist.truncate(ROOT_RELOCATION_SHORTLIST * 2);
    }

    let prepared = summary::Prepared::with_physical(layout, corpus, weights, Arc::clone(physical))?;
    let screening_progress = AtomicU64::new(0);
    for movement in &mut shortlist {
        let mut trial = layout.clone();
        trial.swap(root, movement.destination);
        movement.optimistic_score = movement.fixed_score
            - capacity_recoverable_adjacent_penalty(
                &trial,
                movement.destination,
                proposals,
                &prepared,
                settings,
                movement.adjacent_mass,
                weights,
                stop,
                &screening_progress,
            )?;
    }
    shortlist.sort_by(root_move_order);
    shortlist.truncate(ROOT_RELOCATION_SHORTLIST);

    let current_is_allowed =
        settings.roots.is_empty() || settings.roots.iter().any(|&candidate| candidate == root);
    let mut candidates: Vec<(ak::Layout, usize, summary::Summary)> = Vec::new();
    for movement in shortlist {
        if stop.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let mut trial = layout.clone();
        trial.swap(root, movement.destination);
        if crate::layout_export::jsonc_text(&trial).is_err() {
            continue;
        }
        let local_progress = AtomicU64::new(0);
        let (pruned, evaluated, fit_attempts) = refit_rules(
            &trial,
            movement.destination,
            proposals,
            settings.max_rules,
            settings.shortlist,
            corpus,
            weights,
            stop,
            &local_progress,
            rule_workers,
            Some((baseline, settings)),
            physical,
        )?;
        attempts += fit_attempts;
        // Preserve finite relocated seeds before enforcing final caps or score
        // improvement: one joint round can repair their letters and rules.
        if evaluated.score.is_finite()
            && evaluated.counts.ignored_characters
                <= current_summary
                    .counts
                    .ignored_characters
                    .min(baseline.counts.ignored_characters)
                    + 1e-10
        {
            candidates.push((pruned, movement.destination, evaluated));
        }
    }
    candidates.sort_by(|a, b| a.2.score.total_cmp(&b.2.score).then_with(|| a.1.cmp(&b.1)));
    let finalists: Vec<_> = candidates
        .iter()
        .take(ROOT_RELOCATION_FINALISTS)
        .cloned()
        .collect();
    for (seed, destination, _) in finalists {
        let (polished, evaluated, polish_attempts) = joint_polish_candidate_rounds(
            &seed,
            destination,
            proposals,
            corpus,
            weights,
            baseline,
            settings,
            stop,
            rule_workers,
            1,
            physical,
        )?;
        attempts += polish_attempts;
        candidates.push((polished, destination, evaluated));
    }

    let mut winner: Option<(ak::Layout, usize, summary::Summary)> = None;
    for (candidate, destination, evaluated) in candidates {
        if original(&candidate, destination)
            .is_none_or(|(rules, _)| rules.is_empty() || rules.len() > settings.max_rules)
            || !evaluated.score.is_finite()
            || evaluated.counts.ignored_characters
                > current_summary.counts.ignored_characters + 1e-10
            || !feasible(&evaluated, baseline, settings)
        {
            continue;
        }
        let best_score = winner.as_ref().map_or(
            if current_is_allowed {
                current_summary.score
            } else {
                f64::INFINITY
            },
            |(_, _, summary)| summary.score,
        );
        if evaluated.score < best_score - 1e-10 {
            winner = Some((candidate, destination, evaluated));
        }
    }

    if let Some((layout, root, evaluated)) = winner {
        Ok((layout, root, evaluated, attempts, true))
    } else {
        Ok((layout.clone(), root, current_summary, attempts, false))
    }
}

/// Final per-seed search: settle letters/rules, reconsider the physical root,
/// then give a winning relocation one more joint letter/rule polish.
#[allow(clippy::too_many_arguments)]
fn polish_and_relocate_candidate(
    seed: &ak::Layout,
    root: usize,
    proposals: &[Rule],
    corpus: &ng::NgramCorpus,
    weights: &Weights,
    baseline: &summary::Summary,
    settings: &Settings,
    stop: &AtomicBool,
    rule_workers: usize,
) -> ak::Result<(ak::Layout, usize, summary::Summary, usize)> {
    let physical = summary::PhysicalTables::new(seed, weights);
    let physical = &physical;
    let (polished, polished_summary, mut attempts) = joint_polish_candidate_rounds(
        seed,
        root,
        proposals,
        corpus,
        weights,
        baseline,
        settings,
        stop,
        rule_workers,
        JOINT_POLISH_ROUNDS,
        physical,
    )?;
    let (relocated, relocated_root, relocated_summary, relocation_attempts, moved) =
        relocate_polished_root(
            &polished,
            root,
            polished_summary,
            proposals,
            corpus,
            weights,
            baseline,
            settings,
            stop,
            rule_workers,
            physical,
        )?;
    attempts += relocation_attempts;
    if !moved {
        return Ok((relocated, relocated_root, relocated_summary, attempts));
    }

    let (repolished, repolished_summary, repolish_attempts) = joint_polish_candidate_rounds(
        &relocated,
        relocated_root,
        proposals,
        corpus,
        weights,
        baseline,
        settings,
        stop,
        rule_workers,
        JOINT_POLISH_ROUNDS,
        physical,
    )?;
    attempts += repolish_attempts;
    if repolished_summary.score < relocated_summary.score - 1e-10
        && feasible(&repolished_summary, baseline, settings)
    {
        Ok((repolished, relocated_root, repolished_summary, attempts))
    } else {
        Ok((relocated, relocated_root, relocated_summary, attempts))
    }
}

fn refit_rules(
    layout: &ak::Layout,
    root: usize,
    proposals: &[Rule],
    max_rules: usize,
    shortlist: usize,
    corpus: &ng::NgramCorpus,
    weights: &Weights,
    stop: &AtomicBool,
    progress: &AtomicU64,
    workers: usize,
    acceptance: Option<(&summary::Summary, &Settings)>,

    physical: &Arc<summary::PhysicalTables>,
) -> ak::Result<(ak::Layout, summary::Summary, usize)> {
    // Explore both incumbent and empty dictionaries, keeping their evaluated
    // summaries so branch selection cannot hide a feasible dictionary.
    let prepared = summary::Prepared::with_physical(layout, corpus, weights, Arc::clone(physical))?;
    let seed_summary = prepared.evaluate(layout, stop, progress)?;
    let reference_ignored =
        acceptance.map_or(seed_summary.counts.ignored_characters, |(baseline, _)| {
            seed_summary
                .counts
                .ignored_characters
                .min(baseline.counts.ignored_characters)
        });
    let acceptable = |candidate_layout: &ak::Layout, candidate: &summary::Summary| {
        original(candidate_layout, root).is_some_and(|(rules, _)| rules.len() <= max_rules)
            && candidate.score.is_finite()
            && candidate.counts.ignored_characters <= reference_ignored + 1e-10
            && acceptance.is_none_or(|(baseline, settings)| feasible(candidate, baseline, settings))
    };
    let (incumbent, incumbent_summary, mut attempted) = refit_rules_from_seed(
        layout,
        root,
        proposals,
        max_rules,
        shortlist,
        stop,
        progress,
        workers,
        &prepared,
        weights,
        seed_summary.clone(),
        reference_ignored,
        acceptance,
    )?;
    attempted += 1;
    let mut best = (incumbent, incumbent_summary);
    let (initial_rules, _) = original(layout, root).ok_or("Unsupported magic fallback.")?;
    if initial_rules.is_empty() {
        return Ok((best.0, best.1, attempted));
    }
    let mut empty = layout.clone();
    let ak::Binding::Named(action_name) = &empty.slots[root].binding else {
        return Err("Magic root moved during refinement.".into());
    };
    let action_name = action_name.clone();
    let Some(ak::Action::Rules { rules, .. }) = empty.actions.get_mut(&action_name) else {
        return Err("Magic root action changed during refinement.".into());
    };
    rules.clear();
    let empty_summary = prepared.evaluate(&empty, stop, progress)?;
    attempted += 1;
    let (fresh, fresh_summary, fresh_attempts) = refit_rules_from_seed(
        &empty,
        root,
        proposals,
        max_rules,
        shortlist,
        stop,
        progress,
        workers,
        &prepared,
        weights,
        empty_summary.clone(),
        reference_ignored,
        acceptance,
    )?;
    attempted += fresh_attempts;
    let fresh_acceptable = acceptable(&fresh, &fresh_summary);
    let incumbent_acceptable = acceptable(&best.0, &best.1);
    if (fresh_acceptable && !incumbent_acceptable)
        || (fresh_acceptable == incumbent_acceptable && fresh_summary.score < best.1.score - 1e-10)
    {
        best = (fresh, fresh_summary);
    }
    Ok((best.0, best.1, attempted))
}

/// Refit the ordinary text rules attached to one fixed physical magic key.
///
/// This is the narrow bridge used by the normal optimizer: binding search
/// remains in `action_search`, while rule fitting reuses the designer's exact
/// incumbent/empty-dictionary branches, pruning, coverage checks and caps.
/// Neither the physical root nor any slot binding is changed here.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn refit_fixed_root(
    layout: &ak::Layout,
    baseline: &ak::Layout,
    root: usize,
    corpus: &ng::NgramCorpus,
    weights: &Weights,
    rule_settings: &Settings,
    workers: usize,
    stop: &AtomicBool,
    progress: &AtomicU64,
) -> ak::Result<(ak::Layout, summary::Summary)> {
    let baseline_summary = summary::evaluate(baseline, corpus, weights, stop, progress)?;
    FixedRootFitter::new(
        layout,
        corpus,
        weights,
        rule_settings,
        baseline_summary,
        "normal",
    )
    .refit(layout, root, workers, stop, progress)
}

/// Reusable, corpus-specific setup for repeated fixed-root rule fits. Joint
/// optimization changes bindings and rules many times while the corpus,
/// physical board, weights and acceptance baseline stay fixed; rebuilding
/// those inputs at every annealing checkpoint is pure overhead.
pub(crate) struct FixedRootFitter<'a> {
    corpus: &'a ng::NgramCorpus,
    weights: &'a Weights,
    settings: Settings,
    proposals: Vec<Rule>,
    physical: Arc<summary::PhysicalTables>,
    baseline: summary::Summary,
    source_index: std::sync::Mutex<Option<Arc<crate::action_rule_delta::SourceIndex>>>,
}

impl<'a> FixedRootFitter<'a> {
    pub(crate) fn new(
        layout: &ak::Layout,
        corpus: &'a ng::NgramCorpus,
        weights: &'a Weights,
        settings: &Settings,
        baseline: summary::Summary,
        mode: &str,
    ) -> Self {
        Self {
            corpus,
            weights,
            settings: settings.clone(),
            proposals: ordinary_rule_proposals(corpus),
            physical: summary::PhysicalTables::with_mode(layout, weights, mode),
            baseline,
            source_index: std::sync::Mutex::new(None),
        }
    }

    pub(crate) fn refit(
        &self,
        layout: &ak::Layout,
        root: usize,
        workers: usize,
        stop: &AtomicBool,
        progress: &AtomicU64,
    ) -> ak::Result<(ak::Layout, summary::Summary)> {
        if root >= layout.slots.len() {
            return Err("Select a supported fixed magic key.".into());
        }
        if original(&without_plain_repeats(layout, root), root).is_none() {
            return Err("Select a supported fixed magic key.".into());
        }
        if self.settings.shortlist == 0 || !(1..=MAX_RULES).contains(&self.settings.max_rules) {
            return Err(format!(
                "Magic rule fitting requires a positive shortlist and 1–{MAX_RULES} rules."
            ));
        }
        if stop.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }

        let original_slots = layout.slots.clone();
        let prepared = summary::Prepared::with_physical(
            layout,
            self.corpus,
            self.weights,
            Arc::clone(&self.physical),
        )?;
        if let ak::Binding::Named(name) = &layout.slots[root].binding {
            if let Some(slots) = layout.magic_rule_slots.get(name) {
                return self.refit_outputs(layout, root, slots, &prepared, stop, progress);
            }
        }
        // A hand-made dictionary larger than `max_rules` is first trimmed to
        // it. Rules that are the only way to type a character stay, so the
        // limit can still exceed `max_rules`.
        let trimmed;
        let layout = if genuine_rule_count(layout, root) > self.settings.max_rules {
            trimmed = trim_rules_to(
                layout,
                root,
                self.settings.max_rules,
                self.weights,
                stop,
                progress,
                &prepared,
            )?;
            &trimmed
        } else {
            layout
        };
        // Fit the genuine rules, setting plain repeats aside (they may be
        // non-literal, such as Space); they are restored before every exact
        // evaluation below.
        let repeats = plain_repeats(layout, root);
        let genuine = without_plain_repeats(layout, root);
        let limit = self
            .settings
            .max_rules
            .max(genuine_rule_count(layout, root))
            .min(MAX_RULES);
        let settings = Settings {
            max_rules: limit,
            ..self.settings.clone()
        };
        let restore = |mut candidate: ak::Layout| {
            edit_rules(&mut candidate, root, |rules| {
                for (context, emission) in &repeats {
                    rules
                        .entry(context.clone())
                        .or_insert_with(|| emission.clone());
                }
            });
            candidate
        };
        let fitted = if genuine_rule_count(layout, root) <= limit
            && crate::action_rule_delta::SourceIndex::supports(&genuine, root, self.weights)
        {
            let chords = self.weights.4 && !layout.combos.is_empty();
            let existing = self
                .source_index
                .lock()
                .unwrap()
                .clone()
                .filter(|source| source.chords() == chords);
            let source = if let Some(source) = existing {
                source
            } else {
                let source = crate::action_rule_delta::SourceIndex::new(self.corpus, chords, stop)?;
                let mut cached = self.source_index.lock().unwrap();
                // Keep a concurrently built index of the same kind; replace one
                // built for the other combo policy.
                match cached.as_ref() {
                    Some(existing) if existing.chords() == chords => Arc::clone(existing),
                    _ => Arc::clone(cached.insert(source)),
                }
            };
            crate::magic_rule_fit::fit(
                &genuine,
                root,
                &self.proposals,
                source,
                self.weights,
                &settings,
                &prepared,
                Arc::clone(&self.physical),
                &self.baseline,
                stop,
                progress,
            )?
            .map(|candidates| candidates.into_iter().map(restore).collect::<Vec<_>>())
        } else {
            None
        };
        let (fitted, evaluated) = if let Some(candidates) = fitted {
            let seed = prepared.evaluate(layout, stop, progress)?;
            let coverage_limit = seed
                .counts
                .ignored_characters
                .min(self.baseline.counts.ignored_characters);
            let acceptable = |candidate: &ak::Layout, evaluated: &summary::Summary| {
                genuine_rule_count(candidate, root) <= limit
                    && evaluated.score.is_finite()
                    && evaluated.counts.ignored_characters <= coverage_limit + 1e-10
                    && feasible(evaluated, &self.baseline, &self.settings)
            };
            let (checked_seed, checked_summary, _) = prune_rules_with_summary(
                layout,
                root,
                self.weights,
                stop,
                progress,
                &prepared,
                seed,
            )?;
            let mut best_any = (checked_seed, checked_summary);
            let mut best_feasible = acceptable(&best_any.0, &best_any.1).then(|| best_any.clone());
            let mut checked = vec![layout.actions.clone()];
            if best_any.0.actions != layout.actions {
                checked.push(best_any.0.actions.clone());
            }
            for candidate in candidates {
                if checked.iter().any(|actions| actions == &candidate.actions) {
                    continue;
                }
                checked.push(candidate.actions.clone());
                if stop.load(Ordering::Relaxed) {
                    return Err("cancelled".into());
                }
                let evaluated = prepared.evaluate(&candidate, stop, progress)?;
                let (candidate, evaluated, _) = prune_rules_with_summary(
                    &candidate,
                    root,
                    self.weights,
                    stop,
                    progress,
                    &prepared,
                    evaluated,
                )?;
                if evaluated.score.is_finite() && evaluated.score < best_any.1.score - 1e-10 {
                    best_any = (candidate.clone(), evaluated.clone());
                }
                if acceptable(&candidate, &evaluated)
                    && best_feasible
                        .as_ref()
                        .is_none_or(|best| evaluated.score < best.1.score - 1e-10)
                {
                    best_feasible = Some((candidate, evaluated));
                }
            }
            best_feasible.unwrap_or(best_any)
        } else {
            let (fitted, evaluated, _) = refit_rules(
                layout,
                root,
                &self.proposals,
                limit,
                self.settings.shortlist,
                self.corpus,
                self.weights,
                stop,
                progress,
                workers,
                Some((&self.baseline, &self.settings)),
                &self.physical,
            )?;
            (fitted, evaluated)
        };
        if fitted.slots != original_slots {
            return Err("Fixed-root magic rule fitting moved a physical binding.".into());
        }
        Ok((fitted, evaluated))
    }

    /// Refit outputs in the retained contexts. The existing complete dictionary
    /// is a feasible starting point, so refresh never strands an empty root.
    fn refit_outputs(
        &self,
        layout: &ak::Layout,
        root: usize,
        slots: &[Vec<u8>],
        prepared: &summary::Prepared<'_>,
        stop: &AtomicBool,
        progress: &AtomicU64,
    ) -> ak::Result<(ak::Layout, summary::Summary)> {
        let mut fitted = layout.clone();
        let ak::Binding::Named(name) = &layout.slots[root].binding else {
            return Err("Magic root has no action".into());
        };
        fitted.magic_refit_pending.remove(name);
        let (mut current, mut mappings) = prepared.evaluate_fresh(&fitted, None, stop, progress)?;
        let rank = |value: &summary::Summary| {
            let coverage = (value.counts.ignored_characters
                - self.baseline.counts.ignored_characters)
                .max(0.0);
            let excess: f64 = [
                (SFB, self.settings.sfb_limit),
                (SFS, self.settings.sfs_limit),
                (TRAVEL, self.settings.travel_limit),
                (SFTRAVEL, self.settings.sftravel_limit),
            ]
            .into_iter()
            .filter_map(|(metric, cap)| {
                cap.map(|cap| {
                    (value.metrics.v[metric] - self.baseline.metrics.v[metric] - cap).max(0.0)
                })
            })
            .sum();
            (coverage, excess, value.score)
        };
        for _ in 0..2 {
            let mut changed = false;
            for context in slots {
                if stop.load(Ordering::Relaxed) {
                    return Err("cancelled".into());
                }
                let Some(&byte) = context.first().filter(|_| context.len() == 1) else {
                    continue;
                };
                let existing = original(&fitted, root)
                    .ok_or("Unsupported magic fallback")?
                    .0;
                let mut outputs: Vec<_> = self
                    .proposals
                    .iter()
                    .filter(|rule| rule.context == byte && rule.output != byte)
                    .map(|rule| (rule.frequency, rule.output))
                    .collect();
                outputs.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
                outputs.dedup_by_key(|entry| entry.1);
                let mut best: Option<(ak::Layout, summary::Summary, Option<ng::Mappings>)> = None;
                let mut best_rank = rank(&current);
                for (_, output) in outputs.into_iter().take(self.settings.shortlist.max(1)) {
                    let emission = ak::Emission::Text(vec![output]);
                    if existing.get(context) == Some(&emission) {
                        continue;
                    }
                    let mut trial = fitted.clone();
                    edit_rules(&mut trial, root, |rules| {
                        rules.insert(context.clone(), emission);
                    });
                    let (evaluated, trial_mappings) =
                        prepared.evaluate_from(&trial, mappings.as_ref(), stop, progress)?;
                    if !evaluated.score.is_finite() {
                        continue;
                    }
                    let candidate_rank = rank(&evaluated);
                    if candidate_rank
                        .0
                        .total_cmp(&best_rank.0)
                        .then(candidate_rank.1.total_cmp(&best_rank.1))
                        .then(candidate_rank.2.total_cmp(&best_rank.2))
                        .is_lt()
                    {
                        best_rank = candidate_rank;
                        best = Some((trial, evaluated, trial_mappings));
                    }
                }
                if let Some(best) = best {
                    (fitted, current, mappings) = best;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        if !rule_counts_satisfied(&fitted) {
            return Err("Magic refresh did not preserve its rule slots".into());
        }
        Ok((fitted, current))
    }
}

fn ordinary_rule_proposals(corpus: &ng::NgramCorpus) -> Vec<Rule> {
    corpus
        .rule_candidates()
        .iter()
        .copied()
        .filter(|(skip, context, output, _)| {
            !skip
                && context.is_ascii_graphic()
                && !context.is_ascii_uppercase()
                && output.is_ascii_graphic()
                && !output.is_ascii_uppercase()
        })
        .map(|(skip, context, output, frequency)| Rule {
            skip,
            context,
            output,
            frequency,
        })
        .collect()
}

/// Return a tiny set of locked-root binding swaps that deliberately create a
/// literal SFB which a redesigned magic rule can repair. These seeds cover the
/// main blind spot of a fixed-dictionary annealing epoch; callers still refit
/// and rank every returned layout with the exact full objective.
#[allow(clippy::too_many_arguments)]
pub(crate) fn fixed_root_repair_seeds(
    layout: &ak::Layout,
    root: usize,
    locked: &[bool],
    corpus: &ng::NgramCorpus,
    weights: &Weights,
    mode: &str,
    stop: &AtomicBool,
    progress: &AtomicU64,
) -> ak::Result<Vec<ak::Layout>> {
    if locked.len() != layout.slots.len() {
        return Err("Magic repair seed lock mask does not match the layout.".into());
    }
    if root >= layout.slots.len() || original(layout, root).is_none() {
        return Err("Select a supported fixed magic key.".into());
    }
    let free: Vec<_> = layout
        .slots
        .iter()
        .enumerate()
        .filter_map(|(slot, value)| {
            (slot != root
                && !locked[slot]
                && matches!(value.binding, ak::Binding::Text(_) | ak::Binding::Empty))
            .then_some(slot)
        })
        .collect();
    if free.len() < 2 {
        return Ok(Vec::new());
    }
    let prepared = summary::Prepared::with_physical(
        layout,
        corpus,
        weights,
        summary::PhysicalTables::with_mode(layout, weights, mode),
    )?;
    let swaps = targeted_repair_swaps(
        layout,
        root,
        &ordinary_rule_proposals(corpus),
        &free,
        &prepared,
        weights,
        stop,
        progress,
    )?;
    Ok(swaps
        .into_iter()
        .map(|(left, right)| {
            let mut seed = layout.clone();
            seed.swap(left, right);
            seed
        })
        .collect())
}

#[allow(clippy::too_many_arguments)]
fn refit_rules_from_seed(
    layout: &ak::Layout,
    root: usize,
    proposals: &[Rule],
    max_rules: usize,
    shortlist: usize,
    stop: &AtomicBool,
    progress: &AtomicU64,
    workers: usize,
    prepared: &summary::Prepared<'_>,
    weights: &Weights,
    mut fitted_summary: summary::Summary,
    coverage_limit: f64,
    acceptance: Option<(&summary::Summary, &Settings)>,
) -> ak::Result<(ak::Layout, summary::Summary, usize)> {
    let mut fitted = layout.clone();
    let seed_summary = fitted_summary.clone();
    let acceptable = |candidate: &ak::Layout, evaluated: &summary::Summary| {
        evaluated.score.is_finite()
            && evaluated.counts.ignored_characters <= coverage_limit + 1e-10
            && original(candidate, root).is_some_and(|(rules, _)| rules.len() <= max_rules)
            && acceptance.is_none_or(|(baseline, settings)| feasible(evaluated, baseline, settings))
    };
    // Four raw feasible states are a bounded finalist heuristic; their scores
    // are not lower bounds on the scores obtained after strict pruning.
    let mut finalists: Vec<(ak::Layout, summary::Summary)> = Vec::new();
    let ak::Binding::Named(action_name) = &fitted.slots[root].binding else {
        return Err("Magic root moved during refinement.".into());
    };
    let action_name = action_name.clone();
    let screen = |current: &ak::Layout| -> ak::Result<Vec<Rule>> {
        let (current_rules, current_fallback) =
            original(current, root).ok_or("Unsupported magic fallback.")?;
        // Frequency alone promotes common, already comfortable pairs. Screen all
        // eligible proposals on this exact physical root, then spend full-corpus
        // evaluations on the rules with the largest direct weighted movement fix.
        let mut screened = Vec::new();
        let mut scratch = current.clone();
        scratch.actions.insert(
            action_name.clone(),
            ak::Action::Rules {
                basis: ak::Basis::Text,
                rules: current_rules,
                fallback: current_fallback,
            },
        );
        prepared.prepare_pair_costs(current, stop, progress)?;
        let nonnegative = prepared.pair_movement_costs_nonnegative();
        // Each proposal edits the scratch dictionary in place, without its
        // context and then with the rule, and puts back what it replaced.
        fn rules_of<'a>(
            scratch: &'a mut ak::Layout,
            name: &str,
        ) -> &'a mut BTreeMap<Vec<u8>, ak::Emission> {
            match scratch.actions.get_mut(name) {
                Some(ak::Action::Rules { rules, .. }) => rules,
                _ => unreachable!("the scratch action was installed above"),
            }
        }
        for rule in proposals {
            if stop.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            let context = vec![rule.context];
            let previous = rules_of(&mut scratch, &action_name).remove(&context);
            let restore = |scratch: &mut ak::Layout, previous: Option<ak::Emission>| {
                match previous {
                    Some(emission) => rules_of(scratch, &action_name).insert(context.clone(), emission),
                    None => rules_of(scratch, &action_name).remove(&context),
                };
            };
            let pair = [rule.context, rule.output];
            let without_cost: f64 = prepared
                .pair_movement_costs(&scratch, pair, stop, progress)?
                .into_iter()
                .sum();
            // Most pairs carry no movement penalty; a rule cannot improve them.
            if nonnegative && without_cost <= 1e-9 {
                restore(&mut scratch, previous);
                continue;
            }
            rules_of(&mut scratch, &action_name)
                .insert(context.clone(), ak::Emission::Text(vec![rule.output]));
            let with_cost: f64 = prepared
                .pair_movement_costs(&scratch, pair, stop, progress)?
                .into_iter()
                .sum();
            restore(&mut scratch, previous);
            let gain = without_cost - with_cost;
            if gain.is_finite() && gain > 1e-9 {
                screened.push((gain * rule.frequency, gain, rule.clone()));
            }
        }
        screened.sort_by(|a, b| {
            b.0.total_cmp(&a.0)
                .then_with(|| b.1.total_cmp(&a.1))
                .then_with(|| (a.2.context, a.2.output).cmp(&(b.2.context, b.2.output)))
        });
        // Keep the best output for distinct previous-character contexts before
        // admitting alternate outputs for a context. Otherwise several variants
        // of one frequent context can consume the whole shortlist and prevent a
        // useful rule for another context from ever reaching full evaluation.
        let mut contexts = std::collections::BTreeSet::new();
        let mut primary = Vec::new();
        let mut alternatives = Vec::new();
        for screened_rule in screened {
            if contexts.insert(screened_rule.2.context) {
                primary.push(screened_rule);
            } else {
                alternatives.push(screened_rule);
            }
        }
        primary.extend(alternatives);
        primary.truncate(shortlist);
        Ok(primary
            .into_iter()
            .map(|(_, _, rule)| rule)
            .collect::<Vec<_>>())
    };
    let mut candidates = screen(&fitted)?;
    let mut attempted = proposals.len() * 2;
    let mut refreshed = false;
    // A strictly improving greedy walk can never benefit from reevaluating a
    // rule state it already rejected at an equal or higher incumbent score.
    // This also removes duplicate replacements produced by proposals sharing
    // a context.
    let mut seen = std::collections::BTreeSet::new();
    let state = |rules: &BTreeMap<Vec<u8>, ak::Emission>| {
        rules
            .iter()
            .map(|(context, emission)| {
                let ak::Emission::Text(output) = emission else {
                    unreachable!("original() accepts only literal rule outputs")
                };
                (context.clone(), output.clone())
            })
            .collect::<Vec<_>>()
    };
    seen.insert(state(
        &original(&fitted, root)
            .ok_or("Unsupported magic fallback.")?
            .0,
    ));
    // Replacement and deletion allow a moved key to shed rules which became
    // harmful after either root placement or ordinary binding refinement.
    for _ in 0..(max_rules * 2 + 2) {
        let (existing, fallback) = original(&fitted, root).ok_or("Unsupported magic fallback.")?;
        let mut edits = Vec::new();
        for context in existing.keys() {
            let mut rules = existing.clone();
            rules.remove(context);
            if seen.insert(state(&rules)) {
                edits.push(rules);
            }
        }
        for rule in &candidates {
            let mut rules = existing.clone();
            rules.insert(vec![rule.context], ak::Emission::Text(vec![rule.output]));
            if rules.len() <= max_rules && rules != existing && seen.insert(state(&rules)) {
                edits.push(rules);
            }
        }
        let ak::Binding::Named(name) = &fitted.slots[root].binding else {
            return Err("Magic root moved during refinement.".into());
        };
        let install = |trial: &mut ak::Layout, rules: &BTreeMap<Vec<u8>, ak::Emission>| {
            trial.actions.insert(
                name.clone(),
                ak::Action::Rules {
                    basis: ak::Basis::Text,
                    rules: rules.clone(),
                    fallback: fallback.clone(),
                },
            );
        };
        let evaluate = |trial: &mut ak::Layout, rules: &BTreeMap<Vec<u8>, ak::Emission>| {
            if stop.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            // Replace the complete edited action from this candidate's immutable
            // dictionary; scratch never accumulates edits from earlier candidates.
            install(trial, rules);
            prepared.evaluate(trial, stop, progress)
        };
        let screen_trial = |trial: &mut ak::Layout, rules: &BTreeMap<Vec<u8>, ak::Emission>| {
            if stop.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            install(trial, rules);
            prepared.screen(trial, stop, progress)
        };
        let mut evaluate_edits = |edits: Vec<BTreeMap<Vec<u8>, ak::Emission>>| -> ak::Result<Option<(ak::Layout, summary::Summary)>> {
            if edits.is_empty() {
                return Ok(None);
            }
            let worker_count = workers.max(1).min(edits.len());
            let mut winner: Option<(ak::Layout, summary::Summary)> = None;
            // With `screened` (`Prepared::screen`), only whether the exact
            // summary could be kept: every threshold within the screening
            // margin counts as passed, and nothing changes.
            let mut consider = |index: usize, evaluated: summary::Summary, screened: bool| -> bool {
                if screened && !evaluated.score.is_finite() {
                    return true;
                }
                let margin = |value: f64| if screened { 1e-9 * (1.0 + value.abs()) } else { 0.0 };
                let best = winner.as_ref().map_or(fitted_summary.score, |value| value.1.score);
                let improves = evaluated.score.is_finite()
                    && evaluated.score < best - 1e-10 + margin(best);
                let eligible = evaluated.score.is_finite()
                    && evaluated.counts.ignored_characters
                        <= coverage_limit + 1e-10 + margin(coverage_limit)
                    && edits[index].len() <= max_rules
                    && acceptance.is_none_or(|(baseline, settings)| {
                        if screened {
                            feasible_within(&evaluated, baseline, settings, 1e-9)
                        } else {
                            feasible(&evaluated, baseline, settings)
                        }
                    });
                // Stable sorting puts equal new scores after existing finalists.
                let retain = eligible && (finalists.len() < 4 || {
                    let last = finalists.last().unwrap().1.score;
                    if screened {
                        evaluated.score < last + margin(last)
                    } else {
                        evaluated.score.total_cmp(&last).is_lt()
                    }
                });
                if screened {
                    return retain || improves;
                }
                if retain || improves {
                    let mut trial = fitted.clone();
                    install(&mut trial, &edits[index]);
                    debug_assert_eq!(eligible, acceptable(&trial, &evaluated));
                    if retain {
                        finalists.push((trial.clone(), evaluated.clone()));
                        finalists.sort_by(|a, b| a.1.score.total_cmp(&b.1.score));
                        finalists.truncate(4);
                    }
                    if improves {
                        winner = Some((trial, evaluated));
                    }
                }
                false
            };
            if worker_count == 1 {
                let mut scratch = fitted.clone();
                for (index, rules) in edits.iter().enumerate() {
                    // Trials no screened score could keep need no exact one.
                    if let Some(screened) = screen_trial(&mut scratch, rules)? {
                        if !consider(index, screened, true) {
                            continue;
                        }
                    }
                    let evaluated = evaluate(&mut scratch, rules)?;
                    consider(index, evaluated, false);
                }
            } else {
                std::thread::scope(|scope| -> ak::Result<()> {
                    let edits = &edits;
                    let mut handles = Vec::with_capacity(worker_count);
                    let mut receivers = Vec::with_capacity(worker_count);
                    for worker in 0..worker_count {
                        let evaluate = &evaluate;
                        let mut scratch = fitted.clone();
                        let (sender, receiver) = std::sync::mpsc::sync_channel(0);
                        receivers.push(receiver);
                        handles.push(scope.spawn(move || {
                            for index in (worker..edits.len()).step_by(worker_count) {
                                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    evaluate(&mut scratch, &edits[index])
                                })).unwrap_or_else(|_| Err("Magic rule evaluation worker panicked.".into()));
                                if sender.send((index, result)).is_err() {
                                    return;
                                }
                            }
                        }));
                    }
                    let mut first_error = None;
                    // Drain in original proposal order, including after errors,
                    // before joining workers blocked on zero-capacity channels.
                    for expected in 0..edits.len() {
                        match receivers[expected % worker_count].recv() {
                            Ok((index, result)) => {
                                debug_assert_eq!(index, expected);
                                match result {
                                    Ok(evaluated) if first_error.is_none() => {
                                        consider(index, evaluated, false);
                                    }
                                    Ok(_) => {},
                                    Err(error) if first_error.is_none() => first_error = Some(error),
                                    Err(_) => {},
                                }
                            },
                            Err(_) if first_error.is_none() => first_error = Some("Magic rule evaluation worker stopped unexpectedly.".into()),
                            Err(_) => {},
                        }
                    }
                    for handle in handles {
                        if handle.join().is_err() && first_error.is_none() {
                            first_error = Some("Magic rule evaluation worker panicked.".into());
                        }
                    }
                    first_error.map_or(Ok(()), Err)
                })?;
            }
            Ok(winner)
        };
        attempted += edits.len();
        let mut winner = evaluate_edits(edits)?;
        // At capacity, an independently harmful deletion can block a useful
        // new context. Only widen a stalled single-edit neighborhood, keeping
        // the dictionary full throughout each atomic exchange. The screened
        // shortlist bounds this batch to existing.len() * proposals.len().
        if winner.is_none() && existing.len() == max_rules && max_rules > 0 {
            let mut exchanges = Vec::new();
            for context in existing.keys() {
                for rule in &candidates {
                    let added_context = vec![rule.context];
                    if existing.contains_key(&added_context) {
                        continue;
                    }
                    let mut rules = existing.clone();
                    rules.remove(context);
                    rules.insert(added_context, ak::Emission::Text(vec![rule.output]));
                    if seen.insert(state(&rules)) {
                        exchanges.push(rules);
                    }
                }
            }
            attempted += exchanges.len();
            winner = evaluate_edits(exchanges)?;
        }
        if winner.is_none() && !refreshed {
            candidates = screen(&fitted)?;
            attempted += proposals.len() * 2;
            refreshed = true;
            continue;
        }
        // Distinct proposed contexts bound the coordinated neighborhood. Two
        // replacements/additions can repair interactions missed by single edits;
        // at capacity, remove only the minimum number of old contexts together.
        if winner.is_none() {
            let mut contexts = std::collections::BTreeSet::new();
            let top: Vec<_> = candidates
                .iter()
                .filter(|rule| contexts.insert(rule.context))
                .take(6)
                .collect();
            let mut pair_variants = Vec::new();
            for (index, first) in top.iter().enumerate() {
                for second in top.iter().skip(index + 1) {
                    let mut rules = existing.clone();
                    rules.insert(vec![first.context], ak::Emission::Text(vec![first.output]));
                    rules.insert(
                        vec![second.context],
                        ak::Emission::Text(vec![second.output]),
                    );
                    let excess = rules.len().saturating_sub(max_rules);
                    let removable: Vec<_> = existing
                        .keys()
                        .filter(|context| {
                            **context != vec![first.context] && **context != vec![second.context]
                        })
                        .cloned()
                        .collect();
                    let mut variants = Vec::new();
                    if excess == 0 {
                        variants.push(rules);
                    } else if excess == 1 {
                        for context in &removable {
                            let mut exchanged = rules.clone();
                            exchanged.remove(context);
                            variants.push(exchanged);
                        }
                    } else if excess == 2 {
                        for (i, context) in removable.iter().enumerate() {
                            for other in removable.iter().skip(i + 1) {
                                let mut exchanged = rules.clone();
                                exchanged.remove(context);
                                exchanged.remove(other);
                                variants.push(exchanged);
                            }
                        }
                    }
                    pair_variants.push(variants.into_iter());
                }
            }
            let mut paired = Vec::new();
            // Visit each context pair before taking another capacity exchange
            // for a pair, so one pair cannot consume the exact-evaluation budget.
            'rounds: loop {
                let mut remaining = false;
                for variants in &mut pair_variants {
                    if let Some(rules) = variants.next() {
                        remaining = true;
                        if rules != existing
                            && rules.len() <= max_rules
                            && seen.insert(state(&rules))
                        {
                            paired.push(rules);
                            if paired.len() == 16 {
                                break 'rounds;
                            }
                        }
                    }
                }
                if !remaining {
                    break;
                }
            }
            attempted += paired.len();
            winner = evaluate_edits(paired)?;
        }
        let Some((trial, evaluated)) = winner else {
            break;
        };
        fitted = trial;
        fitted_summary = evaluated;
    }
    // Always include seed and terminal states, even if their raw forms fail
    // caps: pruning can restore feasibility. Deduplication avoids pruning the
    // same dictionary twice when a finalist is also the greedy endpoint.
    finalists.push((layout.clone(), seed_summary));
    finalists.push((fitted.clone(), fitted_summary));
    let mut pruned_states = std::collections::BTreeSet::new();
    let mut best_feasible: Option<(ak::Layout, summary::Summary)> = None;
    let mut terminal: Option<(ak::Layout, summary::Summary)> = None;
    for (candidate, evaluated) in finalists {
        let rules = original(&candidate, root)
            .ok_or("Unsupported magic fallback.")?
            .0;
        if !pruned_states.insert(state(&rules)) {
            continue;
        }
        let (pruned, evaluated, attempts) = prune_rules_with_summary(
            &candidate, root, weights, stop, progress, prepared, evaluated,
        )?;
        attempted += attempts;
        if acceptable(&pruned, &evaluated)
            && best_feasible
                .as_ref()
                .is_none_or(|best| evaluated.score < best.1.score - 1e-10)
        {
            best_feasible = Some((pruned.clone(), evaluated.clone()));
        }
        if terminal
            .as_ref()
            .is_none_or(|best| evaluated.score < best.1.score - 1e-10)
        {
            terminal = Some((pruned, evaluated));
        }
    }
    let (layout, evaluated) = best_feasible
        .or(terminal)
        .ok_or("Magic rule fitting produced no evaluated state.")?;
    Ok((layout, evaluated, attempted))
}

fn effects_with_prepared(
    layout: &ak::Layout,
    root: usize,
    with_rules: &summary::Summary,
    prepared: &summary::Prepared<'_>,
    weights: &Weights,
    stop: &AtomicBool,
    progress: &AtomicU64,
) -> ak::Result<Vec<RuleEffect>> {
    let mut effects = Vec::new();
    visit_effects_with_prepared(
        layout,
        root,
        with_rules,
        prepared,
        weights,
        stop,
        progress,
        |effect, _| effects.push(effect),
    )?;
    Ok(effects)
}

fn visit_effects_with_prepared(
    layout: &ak::Layout,
    root: usize,
    with_rules: &summary::Summary,
    prepared: &summary::Prepared<'_>,
    weights: &Weights,
    stop: &AtomicBool,
    progress: &AtomicU64,
    mut visit: impl FnMut(RuleEffect, summary::Summary),
) -> ak::Result<()> {
    visit_effects_screened(
        layout,
        root,
        with_rules,
        prepared,
        weights,
        stop,
        progress,
        None,
        |effect, without| {
            if let Some(without) = without {
                visit(effect, without);
            }
        },
    )
}

/// `visit_effects_with_prepared`, screening each removal first
/// (`Prepared::screen`): where `skip` holds for the screened effect and
/// summary, the exact evaluation is spared and `visit` gets None.
#[allow(clippy::too_many_arguments)]
fn visit_effects_screened(
    layout: &ak::Layout,
    root: usize,
    with_rules: &summary::Summary,
    prepared: &summary::Prepared<'_>,
    weights: &Weights,
    stop: &AtomicBool,
    progress: &AtomicU64,
    skip: Option<&dyn Fn(&RuleEffect, &summary::Summary) -> bool>,
    mut visit: impl FnMut(RuleEffect, Option<summary::Summary>),
) -> ak::Result<()> {
    let (rules, _) = original(layout, root).ok_or("Unsupported magic fallback.")?;
    let ak::Binding::Named(name) = &layout.slots[root].binding else {
        return Err("Magic root moved during rule analysis.".into());
    };
    let with_breakdown = breakdown(&with_rules.metrics, weights);
    for (context, emission) in rules {
        if stop.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let ak::Emission::Text(output) = emission else {
            continue;
        };
        if context.len() != 1 || output.len() != 1 {
            continue;
        }
        let mut without_layout = layout.clone();
        let Some(ak::Action::Rules { rules, .. }) = without_layout.actions.get_mut(name) else {
            return Err("Magic root action changed during rule analysis.".into());
        };
        rules.remove(&context);
        let effect_of = |without: &summary::Summary| -> ak::Result<RuleEffect> {
            let without_breakdown = breakdown(&without.metrics, weights);
            let pair = [context[0], output[0]];
            let direct_with = prepared.pair_movement_costs(layout, pair, stop, progress)?;
            let direct_without =
                prepared.pair_movement_costs(&without_layout, pair, stop, progress)?;
            let direct_contribution_saved: [f64; N_METRICS] =
                std::array::from_fn(|i| direct_without[i] - direct_with[i]);
            Ok(RuleEffect {
                rule: Rule {
                    skip: false,
                    context: context[0],
                    output: output[0],
                    frequency: 0.0,
                },
                direct_pair_saved: direct_contribution_saved.iter().sum(),
                direct_contribution_saved,
                metrics_without: without.metrics.v,
                metrics_with: with_rules.metrics.v,
                score_saved: without.score - with_rules.score,
                // Mana/aklgg scores have no AKLER penalty split; use the net score.
                penalty_saved: if prepared.physical_mode() {
                    without.score - with_rules.score
                } else {
                    without_breakdown.penalty - with_breakdown.penalty
                },
                contribution_saved: std::array::from_fn(|metric| {
                    without_breakdown.contributions[metric] - with_breakdown.contributions[metric]
                }),
            })
        };
        if let Some(skip) = skip {
            if let Some(screened) = prepared.screen(&without_layout, stop, progress)? {
                let effect = effect_of(&screened)?;
                if skip(&effect, &screened) {
                    visit(effect, None);
                    continue;
                }
            }
        }
        let without = prepared.evaluate(&without_layout, stop, progress)?;
        visit(effect_of(&without)?, Some(without));
    }
    Ok(())
}

/// Measure each current literal rule by removing it while leaving the physical
/// root, fallback, other rules, and all bindings untouched.
pub(crate) fn rule_effects(
    layout: &ak::Layout,
    root: usize,
    corpus: &ng::NgramCorpus,
    weights: &Weights,
    stop: &AtomicBool,
    progress: &AtomicU64,
) -> ak::Result<Vec<RuleEffect>> {
    let prepared = summary::Prepared::new(layout, corpus, weights);
    let with_rules = prepared.evaluate(layout, stop, progress)?;
    effects_with_prepared(
        layout,
        root,
        &with_rules,
        &prepared,
        weights,
        stop,
        progress,
    )
}

/// Remove rules which merely gain a reward, are unused, or make the full
/// weighted score worse. Effects are recomputed after every deletion because
/// action rules can change the context seen by later presses.
#[cfg(test)]
fn prune_rules(
    layout: &ak::Layout,
    root: usize,
    corpus: &ng::NgramCorpus,
    weights: &Weights,
    stop: &AtomicBool,
    progress: &AtomicU64,
) -> ak::Result<(ak::Layout, summary::Summary, usize)> {
    let prepared = summary::Prepared::new(layout, corpus, weights);
    prune_rules_with_prepared(layout, root, weights, stop, progress, &prepared)
}

#[cfg(test)]
fn prune_rules_with_prepared(
    layout: &ak::Layout,
    root: usize,
    weights: &Weights,
    stop: &AtomicBool,
    progress: &AtomicU64,
    prepared: &summary::Prepared<'_>,
) -> ak::Result<(ak::Layout, summary::Summary, usize)> {
    let evaluated = prepared.evaluate(layout, stop, progress)?;
    let (pruned, evaluated, attempts) =
        prune_rules_with_summary(layout, root, weights, stop, progress, prepared, evaluated)?;
    Ok((pruned, evaluated, attempts + 1))
}

fn prune_rules_with_summary(
    layout: &ak::Layout,
    root: usize,
    weights: &Weights,
    stop: &AtomicBool,
    progress: &AtomicU64,
    prepared: &summary::Prepared<'_>,
    mut evaluated: summary::Summary,
) -> ak::Result<(ak::Layout, summary::Summary, usize)> {
    const EPSILON: f64 = 1e-9;
    let mut pruned = layout.clone();
    let mut attempts = 0;
    // Plain repeats (`e → e` under a repeat-output fallback) type what the
    // fallback would: removing one changes no count, so the loop below would
    // remove each, at zero cost and in any order, after a full round of
    // evaluations apiece. Remove them all at once instead, once a single
    // evaluation confirms the result is unchanged to the bit.
    if let ak::Binding::Named(name) = &pruned.slots[root].binding {
        let name = name.clone();
        let mut without = pruned.clone();
        if let Some(ak::Action::Rules {
            rules, fallback, ..
        }) = without.actions.get_mut(&name)
        {
            let repeat_fallback = matches!(fallback, ak::Emission::Call(called)
                if matches!(layout.actions.get(called.as_str()), Some(ak::Action::RepeatOutput)));
            let before = rules.len();
            if repeat_fallback {
                rules.retain(|context, emission| {
                    !(context.len() == 1
                        && matches!(emission, ak::Emission::Text(text) if text == context))
                });
            }
            if rules.len() != before {
                attempts += 1;
                let checked = prepared.evaluate(&without, stop, progress)?;
                let same = checked.score.to_bits() == evaluated.score.to_bits()
                    && checked.counts.ignored_characters.to_bits()
                        == evaluated.counts.ignored_characters.to_bits()
                    && checked.metrics.v.map(f64::to_bits) == evaluated.metrics.v.map(f64::to_bits);
                if same {
                    pruned = without;
                    evaluated = checked;
                }
            }
        }
    }
    loop {
        let mut selected: Option<(RuleEffect, summary::Summary)> = None;
        let ignored = evaluated.counts.ignored_characters;
        // A removal whose screened effect is clearly not removable needs no
        // exact evaluation: screened sums differ from exact ones only in the
        // last bits, and anything near a threshold is evaluated exactly.
        let with_penalty = breakdown(&evaluated.metrics, weights).penalty;
        let clearly_kept = |effect: &RuleEffect, screened: &summary::Summary| {
            let margin = |a: f64, b: f64| 1e-9 * (1.0 + a.abs() + b.abs());
            if screened.counts.ignored_characters > ignored + EPSILON + margin(ignored, 0.0) {
                return true;
            }
            let (penalty, base_penalty) = if prepared.physical_mode() {
                (screened.score, evaluated.score)
            } else {
                (breakdown(&screened.metrics, weights).penalty, with_penalty)
            };
            effect.direct_pair_saved.is_finite()
                && effect.direct_pair_saved > EPSILON
                && effect.score_saved.is_finite()
                && effect.score_saved > EPSILON + margin(screened.score, evaluated.score)
                && effect.penalty_saved.is_finite()
                && effect.penalty_saved > EPSILON + margin(penalty, base_penalty)
        };
        visit_effects_screened(
            &pruned,
            root,
            &evaluated,
            &prepared,
            weights,
            stop,
            progress,
            Some(&clearly_kept),
            |effect, without| {
                attempts += 1;
                // Screened out: not removable, as an exact summary would show.
                let Some(without) = without else {
                    return;
                };
                // A rule that is the only way to type a character stays:
                // untyped characters lower the score, but callers reject
                // dictionaries that leave more characters untyped.
                if without.counts.ignored_characters > ignored + EPSILON {
                    return;
                }
                let removable = !effect.direct_pair_saved.is_finite()
                    || effect.direct_pair_saved <= EPSILON
                    || !effect.score_saved.is_finite()
                    || !effect.penalty_saved.is_finite()
                    || effect.score_saved <= EPSILON
                    || effect.penalty_saved <= EPSILON;
                if !removable {
                    return;
                }
                let replace = selected.as_ref().is_none_or(|(current, _)| {
                    effect
                        .penalty_saved
                        .total_cmp(&current.penalty_saved)
                        .then_with(|| effect.score_saved.total_cmp(&current.score_saved))
                        .then_with(|| {
                            (effect.rule.context, effect.rule.output)
                                .cmp(&(current.rule.context, current.rule.output))
                        })
                        .is_lt()
                });
                if replace {
                    selected = Some((effect, without));
                }
            },
        )?;
        let Some((remove, without)) = selected else {
            break;
        };
        let ak::Binding::Named(name) = &pruned.slots[root].binding else {
            return Err("Magic root moved during rule pruning.".into());
        };
        let name = name.clone();
        let Some(ak::Action::Rules { rules, .. }) = pruned.actions.get_mut(&name) else {
            return Err("Magic root action changed during rule pruning.".into());
        };
        rules.remove(&vec![remove.rule.context]);
        // This exact layout was already evaluated while measuring the effect.
        evaluated = without;
    }
    Ok((pruned, evaluated, attempts))
}

/// Remove genuine rules from `layout`'s magic key until at most `target`
/// remain, each time the one whose removal scores best. Plain repeats and
/// rules that are the only way to type a character are kept.
fn trim_rules_to(
    layout: &ak::Layout,
    root: usize,
    target: usize,
    weights: &Weights,
    stop: &AtomicBool,
    progress: &AtomicU64,
    prepared: &summary::Prepared<'_>,
) -> ak::Result<ak::Layout> {
    const EPSILON: f64 = 1e-9;
    let repeats = plain_repeats(layout, root);
    let mut trimmed = layout.clone();
    let mut evaluated = prepared.evaluate(&trimmed, stop, progress)?;
    while genuine_rule_count(&trimmed, root) > target {
        let ignored = evaluated.counts.ignored_characters;
        let mut selected: Option<(RuleEffect, summary::Summary)> = None;
        visit_effects_with_prepared(
            &trimmed,
            root,
            &evaluated,
            prepared,
            weights,
            stop,
            progress,
            |effect, without| {
                if repeats.contains_key(&vec![effect.rule.context])
                    || without.counts.ignored_characters > ignored + EPSILON
                {
                    return;
                }
                let replace = selected.as_ref().is_none_or(|(current, best)| {
                    without
                        .score
                        .total_cmp(&best.score)
                        .then_with(|| effect.rule.context.cmp(&current.rule.context))
                        .is_lt()
                });
                if replace {
                    selected = Some((effect, without));
                }
            },
        )?;
        let Some((remove, without)) = selected else {
            break;
        };
        edit_rules(&mut trimmed, root, |rules| {
            rules.remove(&vec![remove.rule.context]);
        });
        evaluated = without;
    }
    Ok(trimmed)
}

struct PlacementTrial {
    layout: ak::Layout,
    root: usize,
    summary: summary::Summary,
    attempts: usize,
}

#[derive(Clone)]
struct PolishSeed {
    layout: ak::Layout,
    root: usize,
    score: f64,
    pass: usize,
}

fn consider_polish_seed(seeds: &mut Vec<PolishSeed>, candidate: PolishSeed) {
    if let Some(existing) = seeds.iter_mut().find(|seed| {
        seed.layout.slots == candidate.layout.slots
            && seed.layout.actions == candidate.layout.actions
    }) {
        if candidate.score < existing.score - 1e-10
            || (candidate.score - existing.score).abs() <= 1e-10 && candidate.pass < existing.pass
        {
            *existing = candidate;
        }
    } else {
        seeds.push(candidate);
    }
    select_diverse_seeds(seeds, JOINT_POLISH_SEEDS);
}

// A bounded frontier keeps alternate roots and binding arrangements alive.
const PASS_BEAM_WIDTH: usize = 8;

fn consider_pass_seed(seeds: &mut Vec<PolishSeed>, candidate: PolishSeed) {
    if let Some(existing) = seeds.iter_mut().find(|seed| {
        seed.root == candidate.root
            && seed.layout.slots == candidate.layout.slots
            && seed.layout.actions == candidate.layout.actions
    }) {
        if candidate.score < existing.score {
            *existing = candidate;
        }
    } else {
        seeds.push(candidate);
    }
    select_diverse_seeds(seeds, PASS_BEAM_WIDTH);
}

fn select_diverse_seeds(seeds: &mut Vec<PolishSeed>, limit: usize) {
    seeds.sort_by(|a, b| {
        a.score
            .total_cmp(&b.score)
            .then_with(|| a.root.cmp(&b.root))
            .then_with(|| a.pass.cmp(&b.pass))
    });
    if seeds.len() <= limit {
        return;
    }
    let mut pool = std::mem::take(seeds);
    // Preserve the best score, then reward new roots and the smallest binding
    // distance to an already selected design. Score rank limits that reward
    // so a very weak candidate cannot displace all competitive alternatives.
    // Selection is online and order-sensitive: bounded storage may discard
    // a design before later parents provide a better diversity combination.
    seeds.push(pool.remove(0));
    while seeds.len() < limit && !pool.is_empty() {
        let utility = |index: usize, seed: &PolishSeed| {
            let new_root = !seeds.iter().any(|chosen| chosen.root == seed.root);
            let distance = seeds
                .iter()
                .map(|chosen| {
                    seed.layout
                        .slots
                        .iter()
                        .zip(&chosen.layout.slots)
                        .filter(|(a, b)| a.binding != b.binding)
                        .count()
                })
                .min()
                .unwrap_or(0) as f64
                / seed.layout.slots.len().max(1) as f64;
            (if new_root { 2.0 } else { 0.0 }) + distance - index as f64 * 0.02
        };
        let index = (0..pool.len())
            .max_by(|&a, &b| {
                utility(a, &pool[a])
                    .total_cmp(&utility(b, &pool[b]))
                    .then_with(|| b.cmp(&a))
            })
            .unwrap();
        seeds.push(pool.remove(index));
    }
    seeds.sort_by(|a, b| {
        a.score
            .total_cmp(&b.score)
            .then_with(|| a.root.cmp(&b.root))
            .then_with(|| a.pass.cmp(&b.pass))
    });
}

fn configured_worker_count(settings: &IterativeSettings) -> usize {
    if std::env::var("AKLER_PROFILE")
        .or_else(|_| std::env::var("LAYOUTER_PROFILE"))
        .is_ok_and(|level| matches!(level.as_str(), "1" | "2"))
    {
        return 1;
    }
    let requested = if settings.workers == 0 {
        crate::available_threads()
    } else {
        settings.workers
    };
    requested.max(1).min(256)
}

fn iterative_worker_count(settings: &IterativeSettings, jobs: usize) -> usize {
    configured_worker_count(settings).min(jobs.max(1))
}

#[allow(clippy::too_many_arguments)]
fn evaluate_placement(
    current: &ak::Layout,
    root: usize,
    destination: usize,
    proposals: &[Rule],
    corpus: &ng::NgramCorpus,
    weights: &Weights,
    before: &summary::Summary,
    settings: &Settings,
    stop: &AtomicBool,
    rule_workers: usize,

    physical: &Arc<summary::PhysicalTables>,
) -> ak::Result<Option<PlacementTrial>> {
    let mut trial_layout = current.clone();
    trial_layout.swap(root, destination);
    if crate::layout_export::jsonc_text(&trial_layout).is_err() {
        return Ok(None);
    }
    let mut attempts = 1;
    let local_progress = AtomicU64::new(0);
    let trial_root = destination;
    let (fitted, mut evaluated, rule_attempts) = refit_rules(
        &trial_layout,
        trial_root,
        proposals,
        settings.max_rules,
        settings.shortlist,
        corpus,
        weights,
        stop,
        &local_progress,
        rule_workers,
        Some((before, settings)),
        physical,
    )?;
    attempts += rule_attempts;
    trial_layout = fitted;
    // Judge this root on a locally adjusted letter layout. The root and all
    // named/empty bindings remain fixed; geometry never moves.
    let (optimized, swap_attempts) = quick_optimize_bindings_with_physical(
        &trial_layout,
        trial_root,
        corpus,
        weights,
        before,
        settings,
        stop,
        physical,
    )?;
    attempts += swap_attempts;
    if optimized.slots != trial_layout.slots {
        trial_layout = optimized;
        // Ordinary binding changes alter which contexts a magic rule can
        // repair, so fit the rules once more before ranking or archiving.
        let (fitted, fitted_summary, rule_attempts) = refit_rules(
            &trial_layout,
            trial_root,
            proposals,
            settings.max_rules,
            settings.shortlist,
            corpus,
            weights,
            stop,
            &local_progress,
            rule_workers,
            Some((before, settings)),
            physical,
        )?;
        attempts += rule_attempts;
        trial_layout = fitted;
        evaluated = fitted_summary;
    }
    Ok(Some(PlacementTrial {
        layout: trial_layout,
        root: trial_root,
        summary: evaluated,
        attempts,
    }))
}

#[allow(clippy::too_many_arguments)]
fn evaluate_placements<F>(
    current: &ak::Layout,
    root: usize,
    destinations: &[usize],
    proposals: &[Rule],
    corpus: &ng::NgramCorpus,
    weights: &Weights,
    before: &summary::Summary,
    settings: &IterativeSettings,
    stop: &AtomicBool,
    progress: &AtomicU64,
    pass: usize,
    mut consume: F,
) -> ak::Result<()>
where
    F: FnMut(PlacementTrial) -> ak::Result<()>,
{
    let physical = summary::PhysicalTables::new(current, weights);
    let physical = &physical;
    let worker_budget = configured_worker_count(settings);
    let workers = iterative_worker_count(settings, destinations.len());
    let rule_workers = (worker_budget / workers).max(1);
    if workers == 1 {
        for (index, &destination) in destinations.iter().enumerate() {
            if stop.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            if let Some(result) = evaluate_placement(
                current,
                root,
                destination,
                proposals,
                corpus,
                weights,
                before,
                &settings.rule_settings,
                stop,
                rule_workers,
                physical,
            )? {
                consume(result)?;
            }
            let value = (((pass * 100 + (index + 1) * 100 / destinations.len().max(1))
                / settings.passes)
                * 70
                / 100)
                .min(69) as u64;
            progress.store(value, Ordering::Relaxed);
        }
        return Ok(());
    }

    // Each physical destination is independent within a pass. Workers pull
    // strided jobs from a zero-capacity channel. The coordinator receives in
    // destination order, preserving serial tie-breaks while retaining at most
    // one completed Summary per worker.
    let mut receivers = Vec::with_capacity(workers);
    let mut panicked = false;
    let mut first_error: Option<(usize, String)> = None;
    thread::scope(|scope| {
        let mut handles = Vec::with_capacity(workers);
        for worker in 0..workers {
            let (tx, rx) = mpsc::sync_channel(0);
            receivers.push(rx);
            handles.push(scope.spawn(move || {
                for index in (worker..destinations.len()).step_by(workers) {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let result = evaluate_placement(
                        current,
                        root,
                        destinations[index],
                        proposals,
                        corpus,
                        weights,
                        before,
                        &settings.rule_settings,
                        stop,
                        rule_workers,
                        physical,
                    );
                    let failed = result.is_err();
                    if tx.send((index, result)).is_err() || failed {
                        break;
                    }
                }
            }));
        }
        for expected in 0..destinations.len() {
            let worker = expected % workers;
            match receivers[worker].recv() {
                Ok((index, Ok(result))) => {
                    debug_assert_eq!(index, expected);
                    if first_error.is_none() {
                        if let Some(result) = result {
                            if let Err(error) = consume(result) {
                                first_error = Some((expected, error));
                            }
                        }
                    }
                    let value = (((pass * 100 + (expected + 1) * 100 / destinations.len().max(1))
                        / settings.passes)
                        * 70
                        / 100)
                        .min(69) as u64;
                    progress.store(value, Ordering::Relaxed);
                }
                Ok((index, Err(error))) => {
                    debug_assert_eq!(index, expected);
                    if first_error
                        .as_ref()
                        .is_none_or(|(prior, _)| expected < *prior)
                    {
                        first_error = Some((expected, error));
                    }
                }
                Err(_) => {
                    // This worker either stopped after reporting its first
                    // error/cancellation or panicked. Join below distinguishes
                    // the panic; the earliest reported error remains stable.
                }
            }
        }
        for handle in handles {
            if handle.join().is_err() {
                panicked = true;
            }
        }
    });
    if panicked {
        return Err("magic design worker panicked".into());
    }
    if let Some((_, error)) = first_error {
        return Err(error);
    }
    if stop.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn polish_candidates<F>(
    seeds: &[PolishSeed],
    proposals: &[Rule],
    corpus: &ng::NgramCorpus,
    weights: &Weights,
    baseline: &summary::Summary,
    settings: &IterativeSettings,
    stop: &AtomicBool,
    progress: &AtomicU64,
    progress_start: u64,
    progress_end: u64,
    joint_only: bool,
    mut consume: F,
) -> ak::Result<()>
where
    F: FnMut(&PolishSeed, ak::Layout, usize, summary::Summary, usize) -> ak::Result<()>,
{
    if seeds.is_empty() {
        progress.store(progress_end, Ordering::Relaxed);
        return Ok(());
    }
    let worker_budget = configured_worker_count(settings);
    let workers = iterative_worker_count(settings, seeds.len());
    let rule_workers = (worker_budget / workers).max(1);
    if workers == 1 {
        for (index, seed) in seeds.iter().enumerate() {
            let (layout, root, evaluated, attempts) = polish_candidate(
                &seed.layout,
                seed.root,
                proposals,
                corpus,
                weights,
                baseline,
                &settings.rule_settings,
                stop,
                rule_workers,
                joint_only,
            )?;
            consume(seed, layout, root, evaluated, attempts)?;
            progress.store(
                progress_start
                    + (index + 1) as u64 * (progress_end - progress_start) / seeds.len() as u64,
                Ordering::Relaxed,
            );
        }
        return Ok(());
    }

    // Roots are independent during final polishing. Divide the configured
    // budget between outer root workers and each root's parallel rule fitting,
    // then receive in seed order for deterministic archive tie-breaks.
    let mut receivers = Vec::with_capacity(workers);
    let mut panicked = false;
    let mut first_error: Option<(usize, String)> = None;
    thread::scope(|scope| {
        let mut handles = Vec::with_capacity(workers);
        for worker in 0..workers {
            let (sender, receiver) = mpsc::sync_channel(0);
            receivers.push(receiver);
            handles.push(scope.spawn(move || {
                for index in (worker..seeds.len()).step_by(workers) {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let seed = &seeds[index];
                    let result = polish_candidate(
                        &seed.layout,
                        seed.root,
                        proposals,
                        corpus,
                        weights,
                        baseline,
                        &settings.rule_settings,
                        stop,
                        rule_workers,
                        joint_only,
                    );
                    let failed = result.is_err();
                    if sender.send((index, result)).is_err() || failed {
                        break;
                    }
                }
            }));
        }
        for expected in 0..seeds.len() {
            let worker = expected % workers;
            match receivers[worker].recv() {
                Ok((index, Ok((layout, root, evaluated, attempts)))) => {
                    debug_assert_eq!(index, expected);
                    if first_error.is_none() {
                        if let Err(error) =
                            consume(&seeds[index], layout, root, evaluated, attempts)
                        {
                            first_error = Some((expected, error));
                        }
                    }
                    progress.store(
                        progress_start
                            + (expected + 1) as u64 * (progress_end - progress_start)
                                / seeds.len() as u64,
                        Ordering::Relaxed,
                    );
                }
                Ok((index, Err(error))) => {
                    debug_assert_eq!(index, expected);
                    if first_error
                        .as_ref()
                        .is_none_or(|(prior, _)| expected < *prior)
                    {
                        first_error = Some((expected, error));
                    }
                }
                Err(_) => {}
            }
        }
        for handle in handles {
            if handle.join().is_err() {
                panicked = true;
            }
        }
    });
    if panicked {
        return Err("magic joint-polish worker panicked".into());
    }
    if let Some((_, error)) = first_error {
        return Err(error);
    }
    if stop.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn polish_candidate(
    seed: &ak::Layout,
    root: usize,
    proposals: &[Rule],
    corpus: &ng::NgramCorpus,
    weights: &Weights,
    baseline: &summary::Summary,
    settings: &Settings,
    stop: &AtomicBool,
    rule_workers: usize,
    joint_only: bool,
) -> ak::Result<(ak::Layout, usize, summary::Summary, usize)> {
    if joint_only {
        let (layout, evaluated, attempts) = joint_polish_candidate(
            seed,
            root,
            proposals,
            corpus,
            weights,
            baseline,
            settings,
            stop,
            rule_workers,
        )?;
        Ok((layout, root, evaluated, attempts))
    } else {
        polish_and_relocate_candidate(
            seed,
            root,
            proposals,
            corpus,
            weights,
            baseline,
            settings,
            stop,
            rule_workers,
        )
    }
}

// Exact keys retain every field previously compared by same_design. Hashing
// only accelerates lookup; equality still checks the complete design.
#[derive(PartialEq, Eq, Hash)]
struct ExpandedKey {
    root: usize,
    slots: Vec<ak::Slot>,
    actions: std::collections::BTreeMap<String, ak::Action>,
}

impl ExpandedKey {
    fn placed(seed: &PolishSeed, destination: usize) -> Self {
        let mut slots = seed.layout.slots.clone();
        if seed.root != destination {
            let (lo, hi) = if seed.root < destination {
                (seed.root, destination)
            } else {
                (destination, seed.root)
            };
            let (left, right) = slots.split_at_mut(hi);
            std::mem::swap(&mut left[lo].binding, &mut right[0].binding);
            std::mem::swap(&mut left[lo].label, &mut right[0].label);
        }
        Self {
            root: destination,
            slots,
            actions: seed.layout.actions.clone(),
        }
    }
}

pub(crate) fn iterative_refine(
    layout: &ak::Layout,
    baseline: &ak::Layout,
    corpus: &ng::NgramCorpus,
    weights: &Weights,
    settings: &IterativeSettings,
    stop: &AtomicBool,
    progress: &AtomicU64,
) -> ak::Result<DesignArchive> {
    let opts = &settings.rule_settings;
    if settings.passes == 0
        || settings.archive == 0
        || opts.shortlist == 0
        || !(1..=MAX_RULES).contains(&opts.max_rules)
    {
        return Err(format!(
            "Magic search requires positive passes, archive and shortlist, and 1–{MAX_RULES} rules."
        ));
    }
    crate::layout_export::jsonc_text(layout)?;
    progress.store(0, Ordering::Relaxed);
    let local = AtomicU64::new(0);
    let before = summary::evaluate(baseline, corpus, weights, stop, &local)?;
    let (mut current, mut root) = match settings.placement {
        PlacementMode::ExtraBothPinkies => both_outer_magic_seed(layout)?,
        PlacementMode::Existing => {
            let root = opts
                .roots
                .first()
                .copied()
                .filter(|root| opts.locked_slots.contains(root))
                .or_else(|| {
                    layout
                        .slots
                        .iter()
                        .enumerate()
                        .find(|(i, s)| {
                            matches!(s.binding, ak::Binding::Named(_))
                                && crate::layout_io::dedicated_magic_label(&s.label)
                                && original(layout, *i).is_some()
                        })
                        .map(|(i, _)| i)
                })
                .or_else(|| opts.roots.first().copied())
                .or_else(|| eligible_roots(layout).first().copied())
                .ok_or("Select an existing magic key.")?;
            if root >= layout.slots.len() || original(layout, root).is_none() {
                return Err("Select a supported existing magic key.".into());
            }
            (dedicated_existing(layout, root)?, root)
        }
    };
    // Resolve defaults on the prepared physical board and carry this exact
    // selection through every placement, polishing, and archive stage.
    let mut effective_settings = settings.clone();
    if effective_settings.rule_settings.roots.is_empty() {
        effective_settings.rule_settings.roots = placement_roots(&current)
            .into_iter()
            .filter(|&slot| !matches!(current.slots[slot].finger, 0 | 1 | 6 | 7))
            .collect();
    }
    let settings = &effective_settings;
    let opts = &settings.rule_settings;
    if opts.roots.is_empty() {
        return Err("No supported physical keys allow magic placement.".into());
    }
    let supported = placement_roots(&current);
    if opts.roots.iter().any(|&i| !supported.contains(&i)) {
        return Err("Magic placement includes an unsupported physical key.".into());
    }
    if !opts.roots.contains(&root) {
        let destination = opts.roots[0];
        current.swap(root, destination);
        root = destination;
    }
    let current_summary = summary::evaluate(&current, corpus, weights, stop, &local)?;
    let mut polish_seeds = Vec::with_capacity(JOINT_POLISH_SEEDS);
    // Keep the initial fixed-root design as a fallback even when magic rules
    // are not independently useful yet. Joint fitting may need a layout move
    // before that root can earn a valid rule.
    if opts.roots.is_empty() || opts.roots.contains(&root) {
        consider_polish_seed(
            &mut polish_seeds,
            PolishSeed {
                layout: current.clone(),
                root,
                score: current_summary.score,
                pass: 0,
            },
        );
    }
    let mut proposals: Vec<Rule> = corpus
        .rule_candidates()
        .iter()
        .copied()
        .filter(|(skip, c, o, _)| {
            !skip
                && c.is_ascii_graphic()
                && !c.is_ascii_uppercase()
                && o.is_ascii_graphic()
                && !o.is_ascii_uppercase()
        })
        .map(|(skip, context, output, frequency)| Rule {
            skip,
            context,
            output,
            frequency,
        })
        .collect();
    proposals.sort_by(|a, b| {
        b.frequency
            .total_cmp(&a.frequency)
            .then_with(|| (a.context, a.output).cmp(&(b.context, b.output)))
    });
    let mut archive = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut count = 0;
    let mut beam = vec![PolishSeed {
        layout: current,
        root,
        score: current_summary.score,
        pass: 0,
    }];
    let mut expanded_states = std::collections::HashSet::new();
    for pass in 0..settings.passes {
        if stop.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let mut next_beam = beam.clone();
        // Expand parents sequentially. Placement workers and rule workers
        // already share the full configured budget inside each expansion.
        for (parent_index, parent) in beam.iter().enumerate() {
            let destinations: Vec<_> = (0..parent.layout.slots.len())
                .filter(|&i| {
                    (opts.roots.is_empty() || opts.roots.contains(&i))
                        && (i == parent.root
                            || matches!(&parent.layout.slots[i].binding, ak::Binding::Text(t) if t.len() == 1)
                            || matches!(&parent.layout.slots[i].binding, ak::Binding::Empty))
                })
                .filter(|&destination| {
                    expanded_states.insert(ExpandedKey::placed(parent, destination))
                })
                .collect();
            // Internal placement progress is local to this parent; publish
            // monotonic progress only after its expansion completes.
            let parent_progress = AtomicU64::new(0);
            evaluate_placements(
                &parent.layout,
                parent.root,
                &destinations,
                &proposals,
                corpus,
                weights,
                &before,
                settings,
                stop,
                &parent_progress,
                pass,
                |trial| {
                    count += trial.attempts;
                    if trial.summary.score.is_finite() {
                        let seed = PolishSeed {
                            layout: trial.layout.clone(),
                            root: trial.root,
                            score: trial.summary.score,
                            pass: pass + 1,
                        };
                        consider_polish_seed(&mut polish_seeds, seed.clone());
                        // Pass candidates obey the original baseline caps.
                        // Final polish retains its existing exploratory seeds.
                        if feasible(&trial.summary, &before, opts) {
                            consider_pass_seed(&mut next_beam, seed);
                        }
                    }
                    retain_candidate(
                        &trial.layout,
                        trial.root,
                        pass + 1,
                        &trial.summary,
                        &before,
                        opts,
                        &mut archive,
                        &mut seen,
                        settings.archive,
                    )
                },
            )?;
            progress.store(
                ((pass * beam.len() + parent_index + 1) * 70 / (settings.passes * beam.len()))
                    as u64,
                Ordering::Relaxed,
            );
        }
        let unchanged = beam.len() == next_beam.len()
            && beam.iter().zip(&next_beam).all(|(a, b)| {
                a.root == b.root
                    && a.layout.slots == b.layout.slots
                    && a.layout.actions == b.layout.actions
            });
        beam = next_beam;
        if unchanged {
            break;
        }
    }
    progress.store(70, Ordering::Relaxed);
    // The fast pass above is intentionally cheap and can reject a layout swap
    // before new rules adapt to it. Revisit only the strongest distinct designs
    // with a bounded joint layout/rule search. Seeds may be worse than the
    // original or exceed a cap, but only final valid improvements are retained
    // in the archive. Keep diverse actual designs without a hard per-root cap,
    // so fixed-root polishing can also explore several binding arrangements.
    let mut polished_states: std::collections::HashSet<_> = polish_seeds
        .iter()
        .map(|seed| ExpandedKey::placed(seed, seed.root))
        .collect();
    let mut feedback_seeds = Vec::new();
    polish_candidates(
        &polish_seeds,
        &proposals,
        corpus,
        weights,
        &before,
        settings,
        stop,
        progress,
        70,
        82,
        false,
        |_seed, polished, polished_root, evaluated, polish_attempts| {
            count += polish_attempts;
            let seed = PolishSeed {
                layout: polished.clone(),
                root: polished_root,
                score: evaluated.score,
                pass: settings.passes + 1,
            };
            polished_states.insert(ExpandedKey::placed(&seed, seed.root));
            if evaluated.score.is_finite()
                && (opts.roots.is_empty() || opts.roots.contains(&polished_root))
                && feasible(&evaluated, &before, opts)
            {
                consider_pass_seed(&mut feedback_seeds, seed);
                select_diverse_seeds(&mut feedback_seeds, 2);
            }
            retain_candidate(
                &polished,
                polished_root,
                settings.passes + 1,
                &evaluated,
                &before,
                opts,
                &mut archive,
                &mut seen,
                settings.archive,
            )
        },
    )?;
    // Feed strong fully polished boards back into placement exactly once.
    // The original baseline continues to define caps and coverage throughout.
    let mut feedback_endpoints = Vec::new();
    for (index, parent) in feedback_seeds.iter().enumerate() {
        let destinations: Vec<_> = (0..parent.layout.slots.len())
            .filter(|&destination| {
                (opts.roots.is_empty() || opts.roots.contains(&destination))
                    && (destination == parent.root || matches!(&parent.layout.slots[destination].binding, ak::Binding::Text(t) if t.len() == 1)
                        || matches!(&parent.layout.slots[destination].binding, ak::Binding::Empty))
            })
            .filter(|&destination| {
                expanded_states.insert(ExpandedKey::placed(parent, destination))
            })
            .collect();
        let local_progress = AtomicU64::new(0);
        evaluate_placements(
            &parent.layout,
            parent.root,
            &destinations,
            &proposals,
            corpus,
            weights,
            &before,
            settings,
            stop,
            &local_progress,
            0,
            |trial| {
                count += trial.attempts;
                if trial.summary.score.is_finite() && feasible(&trial.summary, &before, opts) {
                    let endpoint = PolishSeed {
                        layout: trial.layout.clone(),
                        root: trial.root,
                        score: trial.summary.score,
                        pass: settings.passes + 2,
                    };
                    if !polished_states.contains(&ExpandedKey::placed(&endpoint, endpoint.root)) {
                        consider_pass_seed(&mut feedback_endpoints, endpoint);
                        select_diverse_seeds(&mut feedback_endpoints, 2);
                    }
                }
                retain_candidate(
                    &trial.layout,
                    trial.root,
                    settings.passes + 2,
                    &trial.summary,
                    &before,
                    opts,
                    &mut archive,
                    &mut seen,
                    settings.archive,
                )
            },
        )?;
        progress.store(
            82 + (index + 1) as u64 * 8 / feedback_seeds.len() as u64,
            Ordering::Relaxed,
        );
    }
    progress.store(90, Ordering::Relaxed);
    polish_candidates(
        &feedback_endpoints,
        &proposals,
        corpus,
        weights,
        &before,
        settings,
        stop,
        progress,
        90,
        95,
        true,
        |_seed, polished, polished_root, evaluated, attempts| {
            count += attempts;
            retain_candidate(
                &polished,
                polished_root,
                settings.passes + 3,
                &evaluated,
                &before,
                opts,
                &mut archive,
                &mut seen,
                settings.archive,
            )
        },
    )?;
    progress.store(95, Ordering::Relaxed);
    let mut warnings = corpus.warnings.clone();
    warnings.extend(layout.source_warnings.iter().cloned());
    warnings.push(format!("Bounded local search carries up to eight diverse root/binding designs between passes, alternating root placement, previous-text rule edits, and two best-pair binding sweeps over at most {} passes, then jointly polishes up to {} diverse designs for {} rounds, then expands placements once from up to two feasible polished designs and jointly polishes up to two new endpoints; each root fully scores its top {} direct movement fixes, so no global optimum is guaranteed.", settings.passes, JOINT_POLISH_SEEDS, JOINT_POLISH_ROUNDS, opts.shortlist));
    warnings.push("Final rules must reduce both the full score and a weighted penalty when removed rules are tested individually.".into());
    let frequencies: BTreeMap<_, _> = corpus
        .rule_candidates()
        .iter()
        .copied()
        .map(|(skip, context, output, frequency)| ((skip, context, output), frequency))
        .collect();
    for candidate in &mut archive {
        for rule in &mut candidate.rules {
            rule.frequency = frequencies
                .get(&(rule.skip, rule.context, rule.output))
                .copied()
                .unwrap_or(0.0);
        }
    }
    fill_rules_off_comparisons(&mut archive, corpus, weights, stop, progress)?;
    progress.store(100, Ordering::Relaxed);
    Ok(DesignArchive {
        baseline: baseline.clone(),
        before,
        candidates: archive,
        candidate_count: count,
        warnings,
        settings: settings.clone(),
        source_order: corpus.order,
    })
}

/// Extend the designer board once, preserving every existing binding and action.
pub(crate) fn both_outer_magic_seed(layout: &ak::Layout) -> ak::Result<(ak::Layout, usize)> {
    if layout
        .actions
        .get("repeat-output")
        .is_some_and(|a| !matches!(a, ak::Action::RepeatOutput))
    {
        return Err("The repeat-output action name is already used by another action.".into());
    }
    let mut result = layout.clone();
    for left in [true, false] {
        if if left {
            result.left_outer
        } else {
            result.right_outer
        } {
            continue;
        }
        let outer = result
            .slots
            .iter()
            .filter(|s| s.main)
            .map(|s| s.col)
            .max()
            .ok_or("No physical rows.")?;
        if left {
            for slot in &mut result.slots {
                slot.col = slot.col.checked_add(1).ok_or("Physical column overflow.")?;
            }
        }
        let col = if left {
            0
        } else {
            outer.checked_add(1).ok_or("Physical column overflow.")?
        };
        for row in 0..3 {
            let mut slot = result
                .slots
                .iter()
                .filter(|s| s.main && s.row == row)
                .min_by_key(|s| {
                    if left {
                        i16::from(s.col)
                    } else {
                        -i16::from(s.col)
                    }
                })
                .cloned()
                .ok_or("Missing main row.")?;
            slot.col = col;
            slot.column_offset = 0;
            slot.finger = if left { 0 } else { 7 };
            slot.rank = 0;
            slot.hand = if left { 0 } else { 1 };
            slot.binding = ak::Binding::Empty;
            slot.label = "·".into();
            result.slots.push(slot);
        }
        if left {
            result.left_outer = true;
        } else {
            result.right_outer = true;
        }
    }
    result.slots.sort_by_key(|s| (!s.main, s.row, s.col));
    let existing = result
        .slots
        .iter()
        .enumerate()
        .find(|(i, s)| {
            matches!(s.binding, ak::Binding::Named(_))
                && crate::layout_io::dedicated_magic_label(&s.label)
                && original(&result, *i).is_some()
        })
        .map(|(i, _)| i);
    let root = existing
        .or_else(|| {
            result
                .slots
                .iter()
                .position(|s| s.main && s.row == 1 && matches!(s.binding, ak::Binding::Empty))
        })
        .or_else(|| eligible_roots(&result).first().copied())
        .ok_or("No supported physical key for the dedicated magic seed.")?;
    let result = dedicated_existing(&result, root)?;
    crate::layout_export::jsonc_text(&result)?;
    Ok((result, root))
}

#[cfg(test)]
pub(crate) fn extra_magic_seed(layout: &ak::Layout, left: bool) -> ak::Result<(ak::Layout, usize)> {
    if layout
        .actions
        .get("repeat-output")
        .is_some_and(|a| !matches!(a, ak::Action::RepeatOutput))
    {
        return Err("The repeat-output action name is already used by another action.".into());
    }
    if layout.slots.iter().any(|s| s.label == "*") || layout.actions.contains_key("*") {
        return Err(
            "The dedicated '*' label is already used; select the existing key instead.".into(),
        );
    }
    let mut result = layout.clone();
    let outer = result
        .slots
        .iter()
        .filter(|s| s.main)
        .map(|s| s.col)
        .max()
        .ok_or("No physical rows.")?;
    if left {
        for slot in &mut result.slots {
            slot.col = slot.col.checked_add(1).ok_or("Physical column overflow.")?;
        }
    }
    let col = if left {
        0
    } else {
        outer.checked_add(1).ok_or("Physical column overflow.")?
    };
    for row in 0..3 {
        let mut slot = result
            .slots
            .iter()
            .filter(|s| s.main && s.row == row)
            .min_by_key(|s| {
                if left {
                    i16::from(s.col)
                } else {
                    -i16::from(s.col)
                }
            })
            .cloned()
            .ok_or("Missing main row.")?;
        slot.col = col;
        slot.column_offset = 0;
        slot.finger = if left { 0 } else { 7 };
        slot.rank = 0;
        slot.hand = if left { 0 } else { 1 };
        slot.binding = ak::Binding::Empty;
        slot.label = "·".into();
        if row == 1 {
            slot.label = "*".into();
            slot.binding = ak::Binding::Named("*".into());
        }
        result.slots.push(slot);
    }
    result
        .actions
        .insert("repeat-output".into(), ak::Action::RepeatOutput);
    result.actions.insert(
        "*".into(),
        ak::Action::Rules {
            basis: ak::Basis::Text,
            rules: BTreeMap::new(),
            fallback: ak::Emission::Call("repeat-output".into()),
        },
    );
    if left {
        result.left_outer = true;
    } else {
        result.right_outer = true;
    }
    result.action_mode = true;
    result.slots.sort_by_key(|s| (!s.main, s.row, s.col));
    let root = result.slots.iter().position(|s| s.label == "*").unwrap();
    crate::layout_export::jsonc_text(&result)?;
    Ok((result, root))
}

pub(crate) fn prepare(layout: &ak::Layout, placement: PlacementMode) -> ak::Result<ak::Layout> {
    match placement {
        PlacementMode::ExtraBothPinkies => both_outer_magic_seed(layout).map(|x| x.0),
        PlacementMode::Existing => {
            let root = layout
                .slots
                .iter()
                .enumerate()
                .find(|(i, s)| {
                    matches!(s.binding, ak::Binding::Named(_))
                        && crate::layout_io::dedicated_magic_label(&s.label)
                        && original(layout, *i).is_some()
                })
                .map(|(i, _)| i)
                .or_else(|| eligible_roots(layout).first().copied())
                .ok_or("Select an existing magic key.")?;
            dedicated_existing(layout, root)
        }
    }
}

fn retain_candidate(
    layout: &ak::Layout,
    root: usize,
    pass: usize,
    evaluated: &summary::Summary,
    baseline: &summary::Summary,
    settings: &Settings,
    archive: &mut Vec<Candidate>,
    seen: &mut std::collections::BTreeSet<String>,
    limit: usize,
) -> ak::Result<()> {
    if (!settings.roots.is_empty() && !settings.roots.contains(&root))
        || !evaluated.score.is_finite()
        || evaluated.score >= baseline.score
        || !feasible(evaluated, baseline, settings)
    {
        return Ok(());
    }
    let (rules, _) = original(layout, root).ok_or("Unsupported magic candidate.")?;
    if rules.len() > settings.max_rules {
        return Ok(());
    }
    let text = crate::layout_export::jsonc_text(layout)?;
    if !seen.insert(text) {
        return Ok(());
    }
    let rules = rules
        .into_iter()
        .filter_map(|(context, emission)| match emission {
            ak::Emission::Text(output) if context.len() == 1 && output.len() == 1 => Some(Rule {
                skip: false,
                context: context[0],
                output: output[0],
                frequency: 0.0,
            }),
            _ => None,
        })
        .collect();
    archive.push(Candidate {
        layout: layout.clone(),
        score: evaluated.score,
        rules_off_score: None,
        rules_off_note: None,
        raw: evaluated.raw.clone(),
        totals: evaluated.totals,
        metrics: evaluated.metrics.clone(),
        root,
        rules,
        pass,
    });
    archive.sort_by(|a, b| {
        a.score
            .total_cmp(&b.score)
            .then_with(|| a.pass.cmp(&b.pass))
            .then_with(|| a.root.cmp(&b.root))
    });
    archive.truncate(limit);
    Ok(())
}

fn dedicated_existing(layout: &ak::Layout, root: usize) -> ak::Result<ak::Layout> {
    if matches!(layout.slots[root].binding, ak::Binding::Named(_))
        && crate::layout_io::dedicated_magic_label(&layout.slots[root].label)
    {
        if original(layout, root).is_none() {
            return Err("Unsupported magic fallback.".into());
        }
        return Ok(layout.clone());
    }
    if layout.slots.iter().any(|s| s.label == "*") || layout.actions.contains_key("*") {
        return Err("The '*' label is already used by another key.".into());
    }
    let (rules, fallback) = original(layout, root).ok_or("Unsupported magic fallback.")?;
    let mut result = layout.clone();
    if let ak::Binding::Named(name) = &layout.slots[root].binding {
        if !layout
            .slots
            .iter()
            .enumerate()
            .any(|(i, s)| i != root && s.binding == layout.slots[root].binding)
        {
            result.actions.remove(name);
        }
    }
    if matches!(&fallback,ak::Emission::Call(n) if n == "repeat-output") {
        result
            .actions
            .entry("repeat-output".into())
            .or_insert(ak::Action::RepeatOutput);
    }
    result.actions.insert(
        "*".into(),
        ak::Action::Rules {
            basis: ak::Basis::Text,
            rules,
            fallback,
        },
    );
    result.slots[root].binding = ak::Binding::Named("*".into());
    result.slots[root].label = "*".into();
    result.action_mode = true;
    crate::layout_export::jsonc_text(&result)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plain_repeats_are_set_aside_and_need_not_be_literal() {
        let mut layout = ak::Layout::parse(
            &crate::action_fast::tests::fixtures()[0],
            Path::new("magic.dat"),
        )
        .unwrap();
        let root = layout.slots.len() - 1;
        let name = "magic".to_string();
        layout.slots[root].binding = ak::Binding::Named(name.clone());
        layout
            .actions
            .insert("repeat-output".into(), ak::Action::RepeatOutput);
        let text = |t: &[u8]| ak::Emission::Text(t.to_vec());
        layout.actions.insert(
            name,
            ak::Action::Rules {
                basis: ak::Basis::Text,
                rules: [
                    (b"e".to_vec(), text(b"e")),
                    (b" ".to_vec(), text(b" ")),
                    (b"a".to_vec(), text(b"u")),
                ]
                .into(),
                fallback: ak::Emission::Call("repeat-output".into()),
            },
        );
        // Space repeating itself is supported even though Space is not literal.
        assert!(original(&layout, root).is_some());
        let repeats = plain_repeats(&layout, root);
        assert_eq!(
            repeats.keys().cloned().collect::<Vec<_>>(),
            [b" ".to_vec(), b"e".to_vec()]
        );
        assert_eq!(genuine_rule_count(&layout, root), 1);
        let (genuine, _) = original(&without_plain_repeats(&layout, root), root).unwrap();
        assert_eq!(genuine.keys().cloned().collect::<Vec<_>>(), [b"a".to_vec()]);
        // Any other non-literal output stays unsupported.
        edit_rules(&mut layout, root, |rules| {
            rules.insert(b"b".to_vec(), text(b" "));
        });
        assert!(original(&layout, root).is_none());
    }

    #[test]
    fn extra_magic_swaps_preserve_bindings_geometry_and_readable_rules() {
        let layout = ak::Layout::parse(
            &crate::action_fast::tests::fixtures()[0],
            Path::new("magic.dat"),
        )
        .unwrap();
        let (mut seed, root) = extra_magic_seed(&layout, true).unwrap();
        let converted = prepare(&layout, PlacementMode::Existing).unwrap();
        let converted_root = converted.slots.iter().position(|s| s.label == "*").unwrap();
        assert_eq!(layout.slots.len(), converted.slots.len());
        for (a, b) in layout.slots.iter().zip(&converted.slots) {
            assert_eq!(
                (a.row, a.col, a.finger, a.rank, a.hand),
                (b.row, b.col, b.finger, b.rank, b.hand)
            );
        }
        let (_, fallback) = original(&converted, converted_root).unwrap();
        let (_, original_fallback) = original(&layout, converted_root).unwrap();
        assert_eq!(fallback, original_fallback);
        let space = seed
            .slots
            .iter()
            .position(|s| s.binding == ak::Binding::Text(vec![b' ']))
            .unwrap();
        assert!(placement_roots(&seed).contains(&space));
        let unchanged = dedicated_existing(&seed, root).unwrap();
        assert_eq!(unchanged.slots, seed.slots);
        assert_eq!(unchanged.actions, seed.actions);
        let original_keys = crate::action_ui::physical_keys(&layout);
        let prepared_keys = crate::action_ui::physical_keys(&seed);
        let original_anchors = HomeAnchors::new(&original_keys);
        let prepared_anchors = HomeAnchors::new(&prepared_keys);
        for original in &layout.slots {
            let added = seed
                .slots
                .iter()
                .find(|s| {
                    s.main == original.main && s.row == original.row && s.col == original.col + 1
                })
                .unwrap();
            assert_eq!(original.binding, added.binding);
            assert_eq!(original.finger, added.finger);
            assert_eq!(original.col + 1, added.col);
        }
        for (a, original) in layout.slots.iter().enumerate() {
            let shifted = seed
                .slots
                .iter()
                .position(|s| {
                    s.main == original.main && s.row == original.row && s.col == original.col + 1
                })
                .unwrap();
            let old = original_keys[a];
            let new = prepared_keys[shifted];
            if old.main {
                let old_anchor = original_anchors.anchor(old, &original_keys);
                let new_anchor = prepared_anchors.anchor(new, &prepared_keys);
                assert_eq!(
                    old.horizontal_delta(old_anchor),
                    new.horizontal_delta(new_anchor)
                );
                assert_eq!(
                    old.vertical_delta(old_anchor),
                    new.vertical_delta(new_anchor)
                );
                assert_eq!(
                    original_anchors.contains(old),
                    prepared_anchors.contains(new)
                );
            }
            for (b, other) in layout.slots.iter().enumerate() {
                let moved = seed
                    .slots
                    .iter()
                    .position(|s| {
                        s.main == other.main && s.row == other.row && s.col == other.col + 1
                    })
                    .unwrap();
                assert_eq!(
                    old.horizontal_delta(original_keys[b]),
                    new.horizontal_delta(prepared_keys[moved])
                );
                assert_eq!(
                    old.vertical_delta(original_keys[b]),
                    new.vertical_delta(prepared_keys[moved])
                );
            }
        }
        let target = seed
            .slots
            .iter()
            .position(|s| s.binding == ak::Binding::Text(vec![b'q']))
            .unwrap();
        let physical: Vec<_> = seed
            .slots
            .iter()
            .map(|s| (s.row, s.col, s.finger, s.rank, s.hand))
            .collect();
        seed.swap(root, target);
        assert_eq!(seed.slots[root].binding, ak::Binding::Text(vec![b'q']));
        assert_eq!(seed.slots[target].label, "*");
        assert_eq!(
            physical,
            seed.slots
                .iter()
                .map(|s| (s.row, s.col, s.finger, s.rank, s.hand))
                .collect::<Vec<_>>()
        );
        let text = crate::layout_export::jsonc_text(&seed).unwrap();
        let restored =
            crate::layout_io::parse_json_layout(&text, Path::new("magic.jsonc")).unwrap();
        assert_eq!(restored.actions["*"], seed.actions["*"]);
        let wide = crate::layout_io::parse_json_layout(r#"{
            "layout":{"fingers":[
                "skip q w e r t y u i o p skip",
                "skip a s d f g h j k l ; skip",
                "skip z x c v b n m , . / skip"],"thumbs":["",""]},
            "fingermap":["0 0 1 2 3 3 6 6 7 8 9 9","0 0 1 2 3 3 6 6 7 8 9 9","0 0 1 2 3 3 6 6 7 8 9 9"]
        }"#,Path::new("wide.jsonc")).unwrap();
        let (mut wide_seed, wide_root) = extra_magic_seed(&wide, true).unwrap();
        assert_eq!(
            wide_seed
                .slots
                .iter()
                .filter(|s| s.main && s.row == 1)
                .count(),
            13
        );
        let ak::Binding::Named(name) = &wide_seed.slots[wide_root].binding else {
            panic!()
        };
        let name = name.clone();
        let ak::Action::Rules { rules, .. } = wide_seed.actions.get_mut(&name).unwrap() else {
            panic!()
        };
        rules.insert(vec![b'q'], ak::Emission::Text(vec![b'a']));
        let text = crate::layout_export::jsonc_text(&wide_seed).unwrap();
        let round =
            crate::layout_io::parse_json_layout(&text, Path::new("wide-magic.jsonc")).unwrap();
        assert_eq!(round.slots, wide_seed.slots);
        assert_eq!(round.actions, wide_seed.actions);
    }

    #[test]
    fn quick_refinement_moves_text_bindings_but_keeps_magic_root_and_geometry() {
        let layout = ak::Layout::parse(
            &crate::action_fast::tests::fixtures()[0],
            Path::new("magic.dat"),
        )
        .unwrap();
        let (seed, root) = extra_magic_seed(&layout, true).unwrap();
        let corpus = ng::NgramCorpus::from_text(
            r#"{"letters":{"q":40,"a":40,"s":2},"bigrams":{"qa":40,"aq":40,"as":2},"trigrams":{"qaq":20,"aqa":20}}"#,
            Path::new("magic.json"),
        )
        .unwrap();
        let mut values = [0.0; N_WEIGHTS];
        values[SFB] = 10.0;
        let weights = Weights::new(values);
        let stop = AtomicBool::new(false);
        let progress = AtomicU64::new(0);
        let baseline = summary::evaluate(&seed, &corpus, &weights, &stop, &progress).unwrap();
        let geometry: Vec<_> = seed
            .slots
            .iter()
            .map(|slot| {
                (
                    slot.row,
                    slot.col,
                    slot.row_offset,
                    slot.column_offset,
                    slot.finger,
                    slot.rank,
                    slot.hand,
                    slot.main,
                )
            })
            .collect();
        let root_binding = seed.slots[root].binding.clone();
        let (optimized, attempts) = quick_optimize_bindings(
            &seed,
            root,
            &corpus,
            &weights,
            &baseline,
            &Settings {
                sfb_limit: None,
                sfs_limit: None,
                ..Settings::default()
            },
            &stop,
        )
        .unwrap();
        let after = summary::evaluate(&optimized, &corpus, &weights, &stop, &progress).unwrap();
        let effort = crate::action_ui::LocalEffort::new(&optimized, &weights);
        let cache = ng::Incremental::new_with_policy(
            &corpus,
            &optimized,
            Geometry::with_rolls(crate::action_ui::physical_keys(&optimized), weights.rolls()),
            &stop,
            weights.alts(),
            weights.4,
            |a, b, key| effort.get(a, b, key),
        )
        .unwrap();

        assert!(attempts > 0);
        assert!(after.score < baseline.score);
        assert!((cache.score(&weights).score - after.score).abs() < 1e-9);
        assert_eq!(optimized.slots[root].binding, root_binding);
        assert!(seed
            .slots
            .iter()
            .zip(&optimized.slots)
            .enumerate()
            .any(|(slot, (before, after))| slot != root && before.binding != after.binding));
        assert_eq!(
            geometry,
            optimized
                .slots
                .iter()
                .map(|slot| {
                    (
                        slot.row,
                        slot.col,
                        slot.row_offset,
                        slot.column_offset,
                        slot.finger,
                        slot.rank,
                        slot.hand,
                        slot.main,
                    )
                })
                .collect::<Vec<_>>()
        );

        let optimized_slots = optimized.slots.clone();
        let (fitted, fitted_summary) = refit_fixed_root(
            &optimized,
            &seed,
            root,
            &corpus,
            &weights,
            &Settings {
                max_rules: 2,
                shortlist: 4,
                sfb_limit: None,
                sfs_limit: None,
                ..Settings::default()
            },
            1,
            &stop,
            &progress,
        )
        .unwrap();
        let exact = summary::evaluate(&fitted, &corpus, &weights, &stop, &progress).unwrap();
        assert_eq!(fitted.slots, optimized_slots);
        assert_eq!(fitted.slots[root].binding, root_binding);
        assert_eq!(fitted_summary.score.to_bits(), exact.score.to_bits());
        assert_eq!(
            fitted_summary.metrics.v.map(f64::to_bits),
            exact.metrics.v.map(f64::to_bits)
        );
    }

    #[test]
    fn iterative_archive_is_distinct_improving_and_exportable() {
        let layout = ak::Layout::parse(
            &crate::action_fast::tests::fixtures()[0],
            Path::new("magic.dat"),
        )
        .unwrap();
        let corpus = ng::NgramCorpus::from_text(
            r#"{"letters":{"q":3,"a":2},"bigrams":{"qa":2,"aq":2},"trigrams":{"qaq":2,"aqa":1}}"#,
            Path::new("magic.json"),
        )
        .unwrap();
        let mut values = [0.0; N_WEIGHTS];
        values[SFB] = 1.0;
        let settings = IterativeSettings {
            workers: 1,
            passes: 2,
            archive: 3,
            rule_settings: Settings {
                max_rules: 2,
                roots: Vec::new(),
                shortlist: 4,
                sfb_limit: None,
                sfs_limit: None,
                ..Settings::default()
            },
            ..IterativeSettings::default()
        };
        let result = iterative_refine(
            &layout,
            &layout,
            &corpus,
            &Weights::new(values),
            &settings,
            &AtomicBool::new(false),
            &AtomicU64::new(0),
        )
        .unwrap();
        assert!(!result.candidates.is_empty());
        assert!(result.candidate_count > 0);
        let mut distinct = std::collections::BTreeSet::new();
        for candidate in result.candidates {
            assert!(candidate.score < result.before.score);
            assert!(candidate.rules.len() <= settings.rule_settings.max_rules);
            assert!(candidate.rules.iter().all(|r| r.frequency > 0.0));
            let text = crate::layout_export::jsonc_text(&candidate.layout).unwrap();
            assert!(distinct.insert(text.clone()));
            let restored =
                crate::layout_io::parse_json_layout(&text, Path::new("magic.jsonc")).unwrap();
            let score = summary::evaluate(
                &restored,
                &corpus,
                &Weights::new(values),
                &AtomicBool::new(false),
                &AtomicU64::new(0),
            )
            .unwrap();
            assert_eq!(score.score.to_bits(), candidate.score.to_bits());
        }
    }

    #[test]
    fn parallel_placement_trials_match_serial_order_and_scores() {
        let layout = ak::Layout::parse(
            &crate::action_fast::tests::fixtures()[0],
            Path::new("magic.dat"),
        )
        .unwrap();
        let corpus = ng::NgramCorpus::from_text(
            r#"{"letters":{"q":3,"a":2},"bigrams":{"qa":2,"aq":2},"trigrams":{"qaq":2,"aqa":1}}"#,
            Path::new("magic.json"),
        )
        .unwrap();
        let mut values = [0.0; N_WEIGHTS];
        values[SFB] = 1.0;
        let mut settings = IterativeSettings {
            workers: 1,
            passes: 1,
            archive: 2,
            rule_settings: Settings {
                roots: vec![0, 1],
                max_rules: 1,
                shortlist: 1,
                sfb_limit: None,
                sfs_limit: None,
                ..Settings::default()
            },
            ..IterativeSettings::default()
        };
        let run = |settings: &IterativeSettings| {
            iterative_refine(
                &layout,
                &layout,
                &corpus,
                &Weights::new(values),
                settings,
                &AtomicBool::new(false),
                &AtomicU64::new(0),
            )
            .unwrap()
        };
        let serial = run(&settings);
        settings.workers = 2;
        assert_eq!(iterative_worker_count(&settings, 2), 2);
        let parallel = run(&settings);
        let signature = |archive: &DesignArchive| {
            archive
                .candidates
                .iter()
                .map(|candidate| {
                    (
                        candidate.score.to_bits(),
                        candidate.root,
                        candidate.pass,
                        crate::layout_export::jsonc_text(&candidate.layout).unwrap(),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert!(serial.candidate_count > 0);
        assert_eq!(parallel.candidate_count, serial.candidate_count);
        assert_eq!(signature(&parallel), signature(&serial));
    }

    #[test]
    fn candidate_contexts_count_boundaries_once() {
        let corpus = ng::NgramCorpus::from_text(
            r#"{"letters":{"a":1,"b":1,"c":1},"bigrams":{"ab":1,"bc":1},"trigrams":{"abc":1}}"#,
            Path::new("magic.json"),
        )
        .unwrap();
        let candidates = corpus.rule_candidates();
        assert!(candidates.contains(&(false, b'a', b'b', 1.0)));
        assert!(candidates.contains(&(false, b'b', b'c', 1.0)));
        assert!(candidates.contains(&(true, b'a', b'c', 1.0)));
        assert_eq!(candidates.len(), 3);
    }

    #[test]
    fn pruning_keeps_a_rule_that_is_the_only_way_to_type_a_character() {
        let layout = ak::Layout::parse(
            &crate::action_fast::tests::fixtures()[0],
            Path::new("magic.dat"),
        )
        .unwrap();
        assert!(!layout
            .slots
            .iter()
            .any(|slot| slot.binding == ak::Binding::Text(vec![b'~'])));
        let corpus = ng::NgramCorpus::from_text(
            r#"{"letters":{"q":40,"~":20,"a":20},"bigrams":{"q~":20,"~q":10,"qa":20,"aq":10},
                "trigrams":{"q~q":10,"qaq":10}}"#,
            Path::new("magic.json"),
        )
        .unwrap();
        let stop = AtomicBool::new(false);
        let progress = AtomicU64::new(0);
        // An off-home root makes every magic press cost travel, so dropping
        // the rule lowers the score by leaving "~" untyped.
        let root = layout
            .slots
            .iter()
            .position(|slot| slot.binding == ak::Binding::Text(vec![b'p']))
            .unwrap();
        assert!(layout.slots[root].main && !layout.home(root));
        let rule = Rule {
            skip: false,
            context: b'q',
            output: b'~',
            frequency: 20.0,
        };
        let designed = with_rules(&layout, root, std::slice::from_ref(&rule)).unwrap();
        // Without the rule the root types only its own letter: the original layout.
        let empty = layout.clone();
        let mut costs = [0.0; N_WEIGHTS];
        costs[TRAVEL] = 1.0;
        let weights = Weights::new(costs);
        let with = summary::evaluate(&designed, &corpus, &weights, &stop, &progress).unwrap();
        let without = summary::evaluate(&empty, &corpus, &weights, &stop, &progress).unwrap();
        assert!(without.score < with.score);
        assert!(without.counts.ignored_characters > with.counts.ignored_characters);

        let (kept, evaluated, _) =
            prune_rules(&designed, root, &corpus, &weights, &stop, &progress).unwrap();
        assert_eq!(original(&kept, root).unwrap().0.len(), 1);
        assert_eq!(
            evaluated.counts.ignored_characters,
            with.counts.ignored_characters
        );
    }

    #[test]
    fn trimming_to_max_rules_keeps_rules_that_are_the_only_way_to_type_a_character() {
        let layout = ak::Layout::parse(
            &crate::action_fast::tests::fixtures()[0],
            Path::new("magic.dat"),
        )
        .unwrap();
        let corpus = ng::NgramCorpus::from_text(
            r#"{"letters":{"q":40,"~":20,"a":20},"bigrams":{"q~":20,"~q":10,"qa":20,"aq":10},
                "trigrams":{"q~q":10,"qaq":10}}"#,
            Path::new("magic.json"),
        )
        .unwrap();
        let stop = AtomicBool::new(false);
        let progress = AtomicU64::new(0);
        let root = layout
            .slots
            .iter()
            .position(|slot| slot.binding == ak::Binding::Text(vec![b'p']))
            .unwrap();
        let rule = |context, output| Rule {
            skip: false,
            context,
            output,
            frequency: 1.0,
        };
        // "~" has no key, so q→~ is the only way to type it.
        let designed = with_rules(
            &layout,
            root,
            &[rule(b'q', b'~'), rule(b'a', b'q'), rule(b'z', b'a')],
        )
        .unwrap();
        let mut costs = [0.0; N_WEIGHTS];
        costs[TRAVEL] = 1.0;
        let weights = Weights::new(costs);
        let prepared = summary::Prepared::new(&designed, &corpus, &weights);
        let before = prepared.evaluate(&designed, &stop, &progress).unwrap();

        let one = trim_rules_to(&designed, root, 1, &weights, &stop, &progress, &prepared).unwrap();
        let rules = original(&one, root).unwrap().0;
        assert_eq!(
            rules.keys().cloned().collect::<Vec<_>>(),
            vec![b"q".to_vec()]
        );
        // Asking for none still keeps the only way to type "~".
        let none =
            trim_rules_to(&designed, root, 0, &weights, &stop, &progress, &prepared).unwrap();
        assert_eq!(original(&none, root).unwrap().0, rules);
        let after = prepared.evaluate(&none, &stop, &progress).unwrap();
        assert_eq!(
            after.counts.ignored_characters,
            before.counts.ignored_characters
        );
        // A dictionary already within the limit is unchanged.
        let all = trim_rules_to(&designed, root, 3, &weights, &stop, &progress, &prepared).unwrap();
        assert_eq!(all.actions, designed.actions);
    }

    #[test]
    fn final_rule_ablation_keeps_penalty_fixes_and_removes_reward_only_or_unused_rules() {
        let layout = ak::Layout::parse(
            &crate::action_fast::tests::fixtures()[0],
            Path::new("magic.dat"),
        )
        .unwrap();
        let corpus = ng::NgramCorpus::from_text(
            r#"{"letters":{"q":20,"a":20},"bigrams":{"qa":20,"aq":10},"trigrams":{"qaq":10}}"#,
            Path::new("magic.json"),
        )
        .unwrap();
        let stop = AtomicBool::new(false);
        let progress = AtomicU64::new(0);
        let slot = |glyph| {
            layout
                .slots
                .iter()
                .position(|slot| slot.binding == ak::Binding::Text(vec![glyph]))
                .unwrap()
        };
        let rule = Rule {
            skip: false,
            context: b'q',
            output: b'a',
            frequency: 20.0,
        };

        // q->a is an SFB. Routing a through the physical j key removes that
        // penalty, so the conditional ablation reports and retains the rule.
        let root = slot(b'j');
        let designed = with_rules(&layout, root, std::slice::from_ref(&rule)).unwrap();
        let mut costs = [0.0; N_WEIGHTS];
        costs[SFB] = 10.0;
        let weights = Weights::new(costs);
        let effects = rule_effects(&designed, root, &corpus, &weights, &stop, &progress).unwrap();
        assert_eq!(effects.len(), 1);
        assert!(effects[0].score_saved > 0.0);
        assert!(effects[0].penalty_saved > 0.0);
        assert!(effects[0].contribution_saved[SFB] > 0.0);
        let (kept, _, _) =
            prune_rules(&designed, root, &corpus, &weights, &stop, &progress).unwrap();
        assert_eq!(original(&kept, root).unwrap().0.len(), 1);

        // On j, the same rule earns only an alternation reward. It improves
        // net score but removes no weighted penalty, and an absent z context
        // has no effect at all; both are pruned at the fixed point.
        let root = slot(b'j');
        let unused = Rule {
            context: b'z',
            output: b'x',
            frequency: 1.0,
            ..rule.clone()
        };
        let designed = with_rules(&layout, root, &[rule, unused]).unwrap();
        let mut rewards = [0.0; N_WEIGHTS];
        rewards[ALT] = 10.0;
        let weights = Weights::new(rewards);
        let effects = rule_effects(&designed, root, &corpus, &weights, &stop, &progress).unwrap();
        let reward = effects
            .iter()
            .find(|effect| effect.rule.context == b'q')
            .unwrap();
        let unused = effects
            .iter()
            .find(|effect| effect.rule.context == b'z')
            .unwrap();
        assert!(reward.score_saved > 0.0);
        assert!(reward.penalty_saved.abs() < 1e-12);
        assert!(unused.score_saved.abs() < 1e-12);
        let (pruned, _, _) =
            prune_rules(&designed, root, &corpus, &weights, &stop, &progress).unwrap();
        assert!(original(&pruned, root).unwrap().0.is_empty());

        // The sparse fitter's seed and proposals must pass the same canonical
        // pruning policy; a reward-only starting dictionary cannot bypass it.
        let baseline = summary::evaluate(&designed, &corpus, &weights, &stop, &progress).unwrap();
        let fitter = FixedRootFitter::new(
            &designed,
            &corpus,
            &weights,
            &Settings::default(),
            baseline,
            "normal",
        );
        let (fitted, evaluated) = fitter.refit(&designed, root, 1, &stop, &progress).unwrap();
        assert!(fitter.source_index.lock().unwrap().is_some());
        assert!(original(&fitted, root).unwrap().0.is_empty());
        assert_eq!(fitted.slots, designed.slots);
        let canonical = summary::evaluate(&fitted, &corpus, &weights, &stop, &progress).unwrap();
        assert_eq!(evaluated.score.to_bits(), canonical.score.to_bits());

        // A comfortable i->n pair can still repair the *following* n->u
        // SFB on QWERTY. The reported percentage must remain distinct from
        // its weighted score contribution.
        let corpus = ng::NgramCorpus::from_text(
            r#"{"letters":{"i":1,"n":1,"u":1},"bigrams":{"in":1,"nu":1},"trigrams":{"inu":1}}"#,
            Path::new("following-pair.json"),
        )
        .unwrap();
        assert_ne!(
            layout.slots[slot(b'i')].finger,
            layout.slots[slot(b'n')].finger
        );
        assert_eq!(
            layout.slots[slot(b'n')].finger,
            layout.slots[slot(b'u')].finger
        );
        let root = slot(b'f');
        let designed = with_rules(
            &layout,
            root,
            &[Rule {
                skip: false,
                context: b'i',
                output: b'n',
                frequency: 1.0,
            }],
        )
        .unwrap();
        let weights = Weights::new(costs);
        let effects = rule_effects(&designed, root, &corpus, &weights, &stop, &progress).unwrap();
        assert_eq!(effects.len(), 1);
        assert!(effects[0].metrics_without[SFB] > effects[0].metrics_with[SFB]);
        let percentage_reduction = effects[0].metrics_without[SFB] - effects[0].metrics_with[SFB];
        assert!(
            (effects[0].contribution_saved[SFB] - percentage_reduction * weights.0[SFB]).abs()
                < 1e-9
        );
        let after = summary::evaluate(&designed, &corpus, &weights, &stop, &progress).unwrap();
        assert_eq!(
            after.counts.tables[1]
                .get(&[slot(b'n'), slot(b'u')][..])
                .copied()
                .unwrap_or(0.0),
            0.0
        );
        assert!(after.counts.tables[1]
            .get(&[root, slot(b'u')][..])
            .is_some_and(|&frequency| frequency > 0.0));
    }
}

/// Compare the same physical layout with only the selected action's rules
/// cleared. Its binding, fallback, other actions, geometry and weights remain
/// unchanged. Refuse a score comparison when the available population changes.
pub(crate) fn rules_off_comparison(
    layout: &ak::Layout,
    root: usize,
    corpus: &ng::NgramCorpus,
    weights: &Weights,
    stop: &AtomicBool,
    progress: &AtomicU64,
) -> ak::Result<(Option<f64>, Option<String>)> {
    let ak::Binding::Named(name) = &layout.slots[root].binding else {
        return Ok((None, Some("selected key is not an action".into())));
    };
    let mut control = layout.clone();
    let Some(ak::Action::Rules { rules, .. }) = control.actions.get_mut(name) else {
        return Ok((None, Some("selected action has no rule dictionary".into())));
    };
    rules.clear();
    let prepared = summary::Prepared::new(layout, corpus, weights);
    let on = prepared.evaluate(layout, stop, progress)?;
    let off = match prepared.evaluate(&control, stop, progress) {
        Ok(value) => value,
        Err(error) if error == "cancelled" => return Err(error),
        Err(error) => return Ok((None, Some(format!("rules off: {error}")))),
    };
    let same = |a: f64, b: f64| (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0);
    if !same(on.counts.ignored_characters, off.counts.ignored_characters)
        || !same(on.counts.characters, off.counts.characters)
        || !on
            .totals
            .into_iter()
            .zip(off.totals)
            .all(|(a, b)| same(a, b))
    {
        return Ok((
            None,
            Some("rules off changes corpus coverage or physical populations".into()),
        ));
    }
    if !off.score.is_finite() {
        return Ok((None, Some("rules off score is not finite".into())));
    }
    Ok((Some(off.score), None))
}

pub(crate) fn fill_rules_off_comparisons(
    candidates: &mut [Candidate],
    corpus: &ng::NgramCorpus,
    weights: &Weights,
    stop: &AtomicBool,
    progress: &AtomicU64,
) -> ak::Result<()> {
    let count = candidates.len().max(1);
    let local = AtomicU64::new(0);
    for (index, candidate) in candidates.iter_mut().enumerate() {
        if stop.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        (candidate.rules_off_score, candidate.rules_off_note) = rules_off_comparison(
            &candidate.layout,
            candidate.root,
            corpus,
            weights,
            stop,
            &local,
        )?;
        progress.store(95 + ((index + 1) * 5 / count) as u64, Ordering::Relaxed);
    }
    Ok(())
}
