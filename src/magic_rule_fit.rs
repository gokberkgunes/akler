//! Bounded conditional fitting for a fixed ordinary magic key.
//!
//! Proposal screening uses the existing physical pair evaluator.  The actual
//! choice is made from complete metric deltas over only the source windows a
//! tentative dictionary can affect.  Callers must still run the returned
//! layouts through the canonical evaluator and rule-pruning policy.

use crate::*;
use crate::{action_keys as ak, action_rule_delta, action_summary as summary};
use crate::{magic_design::Rule, magic_design::Settings};

const PAIRED_REPAIRS: usize = 6;
const EPSILON: f64 = 1e-10;

#[derive(Default)]
struct Budget {
    trial_limit: usize,
    pass_limit: usize,
    trials: usize,
    mapped_windows: usize,
    total_windows: usize,
}

impl Budget {
    /// Enough trials for a greedy walk from an empty dictionary to `max_rules`
    /// plus a stalled round: each round measures removals, the shortlist and
    /// paired repairs. A fixed 64 once stopped five-rule fits after three.
    /// Mapping work allows a fifth of a corpus pass per trial on average.
    fn new(settings: &Settings, total_windows: usize, mapped_windows: usize) -> Self {
        let trial_limit =
            (settings.max_rules + 2) * (settings.max_rules + settings.shortlist + PAIRED_REPAIRS);
        Self {
            trial_limit,
            pass_limit: trial_limit.div_ceil(5),
            total_windows,
            mapped_windows,
            ..Self::default()
        }
    }

    fn available(&self) -> bool {
        self.trials < self.trial_limit
            && (self.total_windows == 0
                || self.mapped_windows < self.total_windows.saturating_mul(self.pass_limit))
    }

    fn record(&mut self, work: action_rule_delta::Work) {
        self.trials += 1;
        self.mapped_windows = self.mapped_windows.saturating_add(work.mapped_windows);
        self.total_windows = self.total_windows.max(work.total_windows);
    }
}

#[derive(Clone)]
struct RankedRule {
    rule: Rule,
    direct_gain: f64,
}

struct SeedResult {
    terminal: ak::Layout,
    terminal_score: f64,
    terminal_feasible: bool,
    feasible: Option<(ak::Layout, f64)>,
}

fn install_rules(
    layout: &mut ak::Layout,
    action_name: &str,
    fallback: &ak::Emission,
    rules: &BTreeMap<Vec<u8>, ak::Emission>,
) {
    layout.actions.insert(
        action_name.into(),
        ak::Action::Rules {
            basis: ak::Basis::Text,
            rules: rules.clone(),
            fallback: fallback.clone(),
        },
    );
}

fn delta_feasible(
    metrics: &Metrics,
    ignored: f64,
    coverage_limit: f64,
    baseline: &summary::Summary,
    settings: &Settings,
) -> bool {
    ignored <= coverage_limit + EPSILON
        && [
            (SFB, settings.sfb_limit),
            (SFS, settings.sfs_limit),
            (TRAVEL, settings.travel_limit),
            (SFTRAVEL, settings.sftravel_limit),
        ]
        .into_iter()
        .all(|(metric, limit)| {
            limit.is_none_or(|delta| {
                metrics.v[metric] <= baseline.metrics.v[metric] + delta + EPSILON
            })
        })
}

#[allow(clippy::too_many_arguments)]
fn screen(
    layout: &ak::Layout,
    root: usize,
    proposals: &[Rule],
    shortlist: usize,
    prepared: &summary::Prepared<'_>,
    stop: &AtomicBool,
    progress: &AtomicU64,
) -> ak::Result<Vec<RankedRule>> {
    let (existing, fallback) =
        crate::magic_design::original(layout, root).ok_or("Unsupported magic fallback.")?;
    let ak::Binding::Named(action_name) = &layout.slots[root].binding else {
        return Err("Magic root moved during rule screening.".into());
    };
    let mut scratch = layout.clone();
    let mut ranked = Vec::new();
    prepared.prepare_pair_costs(layout, stop, progress)?;
    let nonnegative = prepared.pair_movement_costs_nonnegative();
    for proposal in proposals {
        if stop.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let context = vec![proposal.context];
        let mut rules = existing.clone();
        rules.remove(&context);
        install_rules(&mut scratch, action_name, &fallback, &rules);
        let pair = [proposal.context, proposal.output];
        let without: f64 = prepared
            .pair_movement_costs(&scratch, pair, stop, progress)?
            .into_iter()
            .sum();
        // Most pairs carry no movement penalty; a rule cannot improve them.
        if nonnegative && without <= EPSILON {
            continue;
        }
        rules.insert(context, ak::Emission::Text(vec![proposal.output]));
        install_rules(&mut scratch, action_name, &fallback, &rules);
        let with: f64 = prepared
            .pair_movement_costs(&scratch, pair, stop, progress)?
            .into_iter()
            .sum();
        let direct_gain = without - with;
        if direct_gain.is_finite() && direct_gain > EPSILON {
            ranked.push(RankedRule {
                rule: proposal.clone(),
                direct_gain,
            });
        }
    }
    ranked.sort_by(|a, b| {
        (b.direct_gain * b.rule.frequency)
            .total_cmp(&(a.direct_gain * a.rule.frequency))
            .then_with(|| b.direct_gain.total_cmp(&a.direct_gain))
            .then_with(|| (a.rule.context, a.rule.output).cmp(&(b.rule.context, b.rule.output)))
    });

    // Give every promising context one place before alternate outputs consume
    // the bounded conditional-evaluation budget.
    let mut contexts = std::collections::BTreeSet::new();
    let mut primary = Vec::new();
    let mut alternatives = Vec::new();
    for candidate in ranked {
        if contexts.insert(candidate.rule.context) {
            primary.push(candidate);
        } else {
            alternatives.push(candidate);
        }
    }
    primary.extend(alternatives);
    primary.truncate(shortlist);
    Ok(primary)
}

#[allow(clippy::too_many_arguments)]
fn search_seed(
    seed: ak::Layout,
    root: usize,
    action_name: &str,
    fallback: &ak::Emission,
    ranked: &[RankedRule],
    max_rules: usize,
    mut delta: action_rule_delta::Delta,
    baseline: &summary::Summary,
    settings: &Settings,
    budget: &mut Budget,
    stop: &AtomicBool,
    _progress: &AtomicU64,
) -> ak::Result<SeedResult> {
    let mut current = seed;
    let mut current_score = delta.score();
    let coverage_limit = delta
        .ignored_characters()
        .min(baseline.counts.ignored_characters);
    let mut best_feasible = delta_feasible(
        delta.metrics(),
        delta.ignored_characters(),
        coverage_limit,
        baseline,
        settings,
    )
    .then(|| (current.clone(), current_score));
    // Keep cap-safe alternatives even if an uncapped trial wins this round.
    // Following only that winner can otherwise discard an already-found valid
    // improvement when a later round stalls outside the caps.
    let mut remember_feasible = |candidate: &ak::Layout, trial: &action_rule_delta::Trial| {
        if trial.usable()
            && trial.score().is_finite()
            && delta_feasible(
                trial.metrics(),
                trial.ignored_characters(),
                coverage_limit,
                baseline,
                settings,
            )
            && best_feasible
                .as_ref()
                .is_none_or(|(_, score)| trial.score() < *score - EPSILON)
        {
            best_feasible = Some((candidate.clone(), trial.score()));
        }
    };

    for _ in 0..max_rules.saturating_add(2) {
        if stop.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        if !budget.available() {
            break;
        }
        let (existing, _) = crate::magic_design::original(&current, root)
            .ok_or("Magic dictionary changed to an unsupported form.")?;

        // Measure removal conditionally. Besides allowing harmful rules to be
        // deleted, this identifies the cheapest contexts to exchange at full
        // capacity without testing every proposal against every old rule.
        let mut removals = Vec::new();
        let mut winner: Option<(ak::Layout, action_rule_delta::Trial)> = None;
        for context in existing.keys() {
            if !budget.available() {
                break;
            }
            let mut rules = existing.clone();
            rules.remove(context);
            let mut trial_layout = current.clone();
            install_rules(&mut trial_layout, action_name, fallback, &rules);
            let Some(trial) = delta.evaluate(&trial_layout, stop)? else {
                return Err("unsupported conditional magic rule trial".into());
            };
            budget.record(trial.work());
            remember_feasible(&trial_layout, &trial);
            removals.push((trial.score() - current_score, context.clone()));
            if trial.usable()
                && trial.score().is_finite()
                && trial.ignored_characters() <= coverage_limit + EPSILON
                && trial.score() < current_score - EPSILON
                && winner
                    .as_ref()
                    .is_none_or(|(_, best)| trial.score() < best.score() - EPSILON)
            {
                winner = Some((trial_layout, trial));
            }
        }
        removals.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

        for candidate in ranked {
            if !budget.available() {
                break;
            }
            let context = vec![candidate.rule.context];
            let mut rules = existing.clone();
            if !rules.contains_key(&context) && rules.len() == max_rules {
                let removed = removals
                    .iter()
                    .find(|(_, old)| **old != context)
                    .map(|(_, old)| old.clone());
                let Some(old) = removed else { continue };
                rules.remove(&old);
            }
            rules.insert(context, ak::Emission::Text(vec![candidate.rule.output]));
            if rules == existing || rules.len() > max_rules {
                continue;
            }
            let mut trial_layout = current.clone();
            install_rules(&mut trial_layout, action_name, fallback, &rules);
            let Some(trial) = delta.evaluate(&trial_layout, stop)? else {
                return Err("unsupported conditional magic rule trial".into());
            };
            budget.record(trial.work());
            remember_feasible(&trial_layout, &trial);
            if trial.usable()
                && trial.score().is_finite()
                && trial.ignored_characters() <= coverage_limit + EPSILON
                && trial.score() < current_score - EPSILON
                && winner
                    .as_ref()
                    .is_none_or(|(_, best)| trial.score() < best.score() - EPSILON)
            {
                winner = Some((trial_layout, trial));
            }
        }

        // A pair can be useful even when neither member wins alone. Keep this
        // escape hatch deliberately tiny and use the cheapest measured removals
        // when the dictionary is already full.
        if winner.is_none() && budget.available() {
            let mut distinct = std::collections::BTreeSet::new();
            let top: Vec<_> = ranked
                .iter()
                .filter(|candidate| distinct.insert(candidate.rule.context))
                .take(4)
                .collect();
            let mut attempted = 0;
            'pairs: for (index, first) in top.iter().enumerate() {
                for second in top.iter().skip(index + 1) {
                    if attempted == PAIRED_REPAIRS || !budget.available() {
                        break 'pairs;
                    }
                    let mut rules = existing.clone();
                    rules.insert(
                        vec![first.rule.context],
                        ak::Emission::Text(vec![first.rule.output]),
                    );
                    rules.insert(
                        vec![second.rule.context],
                        ak::Emission::Text(vec![second.rule.output]),
                    );
                    for (_, context) in &removals {
                        if rules.len() <= max_rules {
                            break;
                        }
                        if *context != vec![first.rule.context]
                            && *context != vec![second.rule.context]
                        {
                            rules.remove(context);
                        }
                    }
                    if rules == existing || rules.len() > max_rules {
                        continue;
                    }
                    attempted += 1;
                    let mut trial_layout = current.clone();
                    install_rules(&mut trial_layout, action_name, fallback, &rules);
                    let Some(trial) = delta.evaluate(&trial_layout, stop)? else {
                        return Err("unsupported conditional magic rule trial".into());
                    };
                    budget.record(trial.work());
                    remember_feasible(&trial_layout, &trial);
                    if trial.usable()
                        && trial.score().is_finite()
                        && trial.ignored_characters() <= coverage_limit + EPSILON
                        && trial.score() < current_score - EPSILON
                        && winner
                            .as_ref()
                            .is_none_or(|(_, best)| trial.score() < best.score() - EPSILON)
                    {
                        winner = Some((trial_layout, trial));
                    }
                }
            }
        }

        let Some((next, trial)) = winner else { break };
        let score = trial.score();
        delta.commit(trial)?;
        current = next;
        current_score = score;
    }

    Ok(SeedResult {
        terminal: current,
        terminal_score: current_score,
        terminal_feasible: delta_feasible(
            delta.metrics(),
            delta.ignored_characters(),
            coverage_limit,
            baseline,
            settings,
        ),
        feasible: best_feasible,
    })
}

/// Produce at most two dictionaries for canonical validation: the best actual
/// conditional score and the best state which also obeyed the configured caps.
/// `None` means the delta evaluator cannot prove this layout safe; callers must
/// use the established full-evaluation fitter in that case.
#[allow(clippy::too_many_arguments)]
pub(crate) fn fit(
    layout: &ak::Layout,
    root: usize,
    proposals: &[Rule],
    source: Arc<action_rule_delta::SourceIndex>,
    weights: &Weights,
    settings: &Settings,
    prepared: &summary::Prepared<'_>,
    physical: Arc<summary::PhysicalTables>,
    baseline: &summary::Summary,
    stop: &AtomicBool,
    progress: &AtomicU64,
) -> ak::Result<Option<Vec<ak::Layout>>> {
    if !action_rule_delta::SourceIndex::supports(layout, root, weights) {
        return Ok(None);
    }
    let ak::Binding::Named(action_name) = &layout.slots[root].binding else {
        return Ok(None);
    };
    let action_name = action_name.clone();
    let (initial_rules, fallback) =
        crate::magic_design::original(layout, root).ok_or("Unsupported magic fallback.")?;
    let ranked = screen(
        layout,
        root,
        proposals,
        settings.shortlist,
        prepared,
        stop,
        progress,
    )?;
    if ranked.is_empty() && initial_rules.is_empty() {
        return Ok(Some(vec![layout.clone()]));
    }
    let Some(base) = action_rule_delta::Delta::new(source, layout, root, weights, physical, stop)?
    else {
        return Ok(None);
    };
    let mut current_budget = Budget::new(
        settings,
        base.initial_work().total_windows,
        base.initial_work().mapped_windows,
    );
    let current = search_seed(
        layout.clone(),
        root,
        &action_name,
        &fallback,
        &ranked,
        settings.max_rules,
        base.clone(),
        baseline,
        settings,
        &mut current_budget,
        stop,
        progress,
    )?;
    let mut results = vec![(
        current.terminal,
        current.terminal_score,
        current.terminal_feasible,
    )];
    if let Some(feasible) = current.feasible {
        results.push((feasible.0, feasible.1, true));
    }

    if !initial_rules.is_empty() {
        let mut fresh_budget = Budget::new(settings, base.initial_work().total_windows, 0);
        let mut empty = layout.clone();
        install_rules(&mut empty, &action_name, &fallback, &BTreeMap::new());
        let Some(empty_trial) = base.evaluate(&empty, stop)? else {
            return Ok(None);
        };
        fresh_budget.record(empty_trial.work());
        if empty_trial.usable() {
            let mut empty_delta = base;
            empty_delta.commit(empty_trial)?;
            let fresh = search_seed(
                empty,
                root,
                &action_name,
                &fallback,
                &ranked,
                settings.max_rules,
                empty_delta,
                baseline,
                settings,
                &mut fresh_budget,
                stop,
                progress,
            )?;
            results.push((
                fresh.terminal,
                fresh.terminal_score,
                fresh.terminal_feasible,
            ));
            if let Some(feasible) = fresh.feasible {
                results.push((feasible.0, feasible.1, true));
            }
        }
    }

    results.sort_by(|a, b| a.1.total_cmp(&b.1));
    results.dedup_by(|a, b| a.0.actions == b.0.actions);
    let mut layouts = Vec::new();
    if let Some((candidate, _, _)) = results.first() {
        layouts.push(candidate.clone());
    }
    if let Some((candidate, _, _)) = results.iter().find(|(candidate, _, feasible)| {
        *feasible
            && layouts
                .iter()
                .all(|known| known.actions != candidate.actions)
    }) {
        layouts.push(candidate.clone());
    }
    if layouts.is_empty() {
        layouts.push(layout.clone());
    }
    Ok(Some(layouts))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_allows_a_full_greedy_walk_from_an_empty_dictionary() {
        for (max_rules, shortlist) in [(5, 24), (1, 1), (5, 60)] {
            let settings = Settings {
                roots: Vec::new(),
                locked_slots: Vec::new(),
                max_rules,
                shortlist,
                sfb_limit: None,
                sfs_limit: None,
                travel_limit: None,
                sftravel_limit: None,
            };
            let total_windows = 8_000;
            let mut budget = Budget::new(&settings, total_windows, total_windows);
            // Every round measures existing removals and the full shortlist; the
            // final, stalled round also tries the paired repairs.
            for existing in 0..=max_rules {
                let trials = existing
                    + shortlist
                    + if existing == max_rules {
                        PAIRED_REPAIRS
                    } else {
                        0
                    };
                for _ in 0..trials {
                    assert!(budget.available(), "budget ended with {existing} rules");
                    budget.record(action_rule_delta::Work {
                        mapped_windows: total_windows / 6,
                        total_windows,
                    });
                }
            }
        }
    }
}
