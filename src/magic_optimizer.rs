//! Joint ordinary-binding and magic-rule search for the normal optimizer.
use crate::*;
use crate::{
    action_keys as ak, action_ngrams as ng, action_search, action_summary as summary,
    magic_design as design,
};

const JOINT_ANNEAL_CHUNK: usize = 2_000;
const MAX_JOINT_EPOCHS: usize = 8;
// Repair seeds refitted on the injected start. Over six seeds, the third and
// fourth never improved a final result but cost about 15% of run time.
const JOINT_ARCHIVE_SEEDS: usize = 2;

#[derive(Clone)]
struct JointResult {
    layout: ak::Layout,
    score: f64,
    raw: Raw,
    totals: [f64; 4],
    snapshot: Snapshot,
}

fn normalized_shares(corpora: &[(ng::NgramCorpus, f64)]) -> ak::Result<Vec<f64>> {
    if corpora.is_empty() {
        return Err("Magic optimization requires a training corpus.".into());
    }
    let total: f64 = corpora.iter().map(|(_, share)| *share).sum();
    if corpora
        .iter()
        .any(|(_, share)| !share.is_finite() || *share < 0.0)
        || !total.is_finite()
        || total <= 0.0
    {
        return Err("Training shares must have a finite positive sum.".into());
    }
    Ok(corpora.iter().map(|(_, share)| *share / total).collect())
}

fn within_limits(metrics: &Metrics, baseline: &Metrics, settings: &SearchSettings) -> bool {
    [
        (SFB, settings.sfb_limit),
        (SFS, settings.sfs_limit),
        (TRAVEL, settings.travel_limit),
        (SFTRAVEL, settings.sftravel_limit),
    ]
    .into_iter()
    .all(|(metric, limit)| {
        limit.is_none_or(|increase| metrics.v[metric] <= baseline.v[metric] + increase + 1e-10)
    })
}

/// Why a report fails the caps or coverage measured against `baseline`.
fn limit_failure(
    report: &summary::Summary,
    baseline: &summary::Summary,
    settings: &SearchSettings,
) -> Option<String> {
    let (metrics, base) = (&report.metrics, &baseline.metrics);
    for (metric, name, key, limit) in [
        (SFB, "SFB", "max_sfb_increase", settings.sfb_limit),
        (SFS, "SFS", "max_sfs_increase", settings.sfs_limit),
        (TRAVEL, "TRAVEL", "max_travel_increase", settings.travel_limit),
        (SFTRAVEL, "SFTRAVEL", "max_sftravel_increase", settings.sftravel_limit),
    ] {
        if let Some(increase) = limit {
            if metrics.v[metric] > base.v[metric] + increase + 1e-10 {
                return Some(format!(
                    "{name} is {:.3} against the original's {:.3} + {increase} ({key})",
                    metrics.v[metric], base.v[metric]
                ));
            }
        }
    }
    let lost = report.counts.ignored_characters - baseline.counts.ignored_characters;
    (lost > 1e-10).then(|| {
        format!("it cannot type {lost:.0} corpus characters the original types")
    })
}

fn identity_snapshot(mut snapshot: Snapshot, slots: usize, score: f64) -> Snapshot {
    let candidate = Candidate {
        arr: (0..slots).collect(),
        score,
    };
    snapshot.best = candidate.clone();
    snapshot.archive = vec![candidate];
    snapshot.has_best = true;
    snapshot
}

fn evaluate_joint(
    layout: &ak::Layout,
    corpora: &[(ng::NgramCorpus, f64)],
    shares: &[f64],
    weights: &Weights,
    baselines: &[summary::Summary],
    settings: &SearchSettings,
    stop: &AtomicBool,
    known: Option<(usize, &summary::Summary)>,
    mut scorer: Option<&mut ExactScorer<'_>>,
) -> ak::Result<Option<(f64, Vec<summary::Summary>)>> {
    if !design::rule_counts_satisfied(layout) {
        return Ok(None);
    }
    let progress = AtomicU64::new(0);
    let mut score = 0.0;
    let mut reports = Vec::with_capacity(corpora.len());
    for (index, (((corpus, _), share), baseline)) in
        corpora.iter().zip(shares).zip(baselines).enumerate()
    {
        let report = if known.is_some_and(|(known_index, _)| known_index == index) {
            known.unwrap().1.clone()
        } else if let Some(scorer) = scorer.as_deref_mut() {
            scorer.evaluate(index, layout, stop)?
        } else {
            summary::evaluate_with_mode(layout, corpus, weights, stop, &progress, &settings.mode)?
        };
        // Match the ordinary optimizer: a corpus with no share is available
        // for display, but it neither contributes to the objective nor gates
        // a candidate through limits or coverage.
        if *share != 0.0 && limit_failure(&report, baseline, settings).is_some() {
            return Ok(None);
        }
        score += share * report.score;
        reports.push(report);
    }
    Ok(score.is_finite().then_some((score, reports)))
}

fn better(candidate: &JointResult, current: &JointResult) -> bool {
    candidate.score < current.score - 1e-10
        || ((candidate.score - current.score).abs() <= 1e-10
            && layout_tie_key(&candidate.layout) < layout_tie_key(&current.layout))
}

fn layout_tie_key(layout: &ak::Layout) -> String {
    // Slot order and BTreeMap action order make this stable. This path is used
    // only for exact score ties, so the allocation is outside the hot loop.
    format!("{}\n{:?}", layout.text(), layout.actions)
}

fn emit(
    callback: &mut dyn FnMut(ak::Layout, Snapshot, Metrics, Raw, [f64; 4]),
    value: &JointResult,
    baseline: &summary::Summary,
) {
    let snapshot = identity_snapshot(
        value.snapshot.clone(),
        value.layout.slots.len(),
        value.score,
    );
    callback(
        value.layout.clone(),
        snapshot,
        baseline.metrics.clone(),
        value.raw.clone(),
        value.totals,
    );
}

fn rule_settings(
    root: usize,
    settings: &SearchSettings,
    magic: &crate::MagicSettings,
) -> design::Settings {
    design::Settings {
        roots: vec![root],
        locked_slots: vec![root],
        max_rules: magic.max_rules,
        shortlist: magic.shortlist,
        sfb_limit: settings.sfb_limit,
        sfs_limit: settings.sfs_limit,
        travel_limit: settings.travel_limit,
        sftravel_limit: settings.sftravel_limit,
    }
}

/// The layout's existing magic key: the single slot bound to a rule action
/// whose dictionary the rule fitter supports. Such layouts can refit their
/// rules without injecting a new key.
pub(crate) fn existing_root(layout: &ak::Layout) -> Option<usize> {
    // Adaptive swap keys also hold rules, but they are letters, not roots.
    let mut roots = layout.slots.iter().enumerate().filter(|&(index, slot)| {
        matches!(&slot.binding, ak::Binding::Named(name)
            if matches!(layout.actions.get(name), Some(ak::Action::Rules { .. })))
            && !crate::swap_fit::is_adaptive_key(layout, index)
    });
    let (root, _) = roots.next()?;
    (roots.next().is_none() && design::original(layout, root).is_some()).then_some(root)
}

fn locate_root(layout: &ak::Layout, action_name: &str) -> ak::Result<usize> {
    let mut roots = layout.slots.iter().enumerate().filter_map(|(index, slot)| {
        matches!(&slot.binding, ak::Binding::Named(name) if name == action_name).then_some(index)
    });
    let root = roots
        .next()
        .ok_or_else(|| format!("Magic action {action_name:?} has no physical key."))?;
    if roots.next().is_some() {
        return Err(format!(
            "Magic action {action_name:?} has more than one physical key."
        ));
    }
    Ok(root)
}

fn wait_if_paused(control: &Control, deadline: Option<Instant>) -> bool {
    while control.pause.load(Ordering::Relaxed)
        && !control.cancel.load(Ordering::Relaxed)
        && !expired(deadline)
    {
        thread::sleep(Duration::from_millis(15));
    }
    control.cancel.load(Ordering::Relaxed) || expired(deadline)
}

fn chunk_sizes(steps: usize, hybrid: bool) -> Vec<usize> {
    if !hybrid {
        return vec![steps];
    }
    let chunks = steps
        .div_ceil(JOINT_ANNEAL_CHUNK)
        .clamp(1, MAX_JOINT_EPOCHS);
    let base = steps / chunks;
    let extra = steps % chunks;
    (0..chunks)
        .map(|chunk| base + usize::from(chunk < extra))
        .collect()
}

fn at_temperature(settings: &SearchSettings, completed: usize) -> f64 {
    if settings.anneal_steps <= 1 {
        return settings.cooling_end;
    }
    let fraction = completed.min(settings.anneal_steps) as f64 / settings.anneal_steps as f64;
    settings.temperature * (settings.cooling_end / settings.temperature).powf(fraction)
}

/// Exact mixture scores along one restart chain. On each corpus a candidate
/// remaps only the windows it changes from the previous one scored there;
/// the scores are a fresh evaluation's (`Prepared::evaluate_exact_from`).
struct ExactScorer<'a> {
    prepared: Vec<summary::Prepared<'a>>,
    previous: Vec<Option<ng::Mappings>>,
}

impl<'a> ExactScorer<'a> {
    fn new(
        seed: &ak::Layout,
        corpora: &'a [(ng::NgramCorpus, f64)],
        weights: &'a Weights,
        physical: &Arc<summary::PhysicalTables>,
    ) -> ak::Result<Self> {
        let prepared = corpora
            .iter()
            .map(|(corpus, _)| {
                summary::Prepared::with_physical(seed, corpus, weights, Arc::clone(physical))
            })
            .collect::<ak::Result<Vec<_>>>()?;
        Ok(Self {
            previous: prepared.iter().map(|_| None).collect(),
            prepared,
        })
    }

    fn evaluate(
        &mut self,
        corpus: usize,
        layout: &ak::Layout,
        stop: &AtomicBool,
    ) -> ak::Result<summary::Summary> {
        let (summary, mappings) = self.prepared[corpus].evaluate_exact_from(
            layout,
            self.previous[corpus].as_ref(),
            stop,
            &AtomicU64::new(0),
        )?;
        if mappings.is_some() {
            self.previous[corpus] = mappings;
        }
        Ok(summary)
    }
}

#[allow(clippy::too_many_arguments)]
fn exact_candidate(
    layout: ak::Layout,
    corpora: &[(ng::NgramCorpus, f64)],
    shares: &[f64],
    weights: &Weights,
    baselines: &[summary::Summary],
    settings: &SearchSettings,
    stop: &AtomicBool,
    progress: Progress,
    scorer: Option<&mut ExactScorer<'_>>,
) -> ak::Result<Option<JointResult>> {
    exact_candidate_reusing(
        layout, corpora, shares, weights, baselines, settings, stop, progress, None, scorer,
    )
}

#[allow(clippy::too_many_arguments)]
fn exact_candidate_reusing(
    layout: ak::Layout,
    corpora: &[(ng::NgramCorpus, f64)],
    shares: &[f64],
    weights: &Weights,
    baselines: &[summary::Summary],
    settings: &SearchSettings,
    stop: &AtomicBool,
    progress: Progress,
    known: Option<(usize, &summary::Summary)>,
    scorer: Option<&mut ExactScorer<'_>>,
) -> ak::Result<Option<JointResult>> {
    Ok(evaluate_joint(
        &layout, corpora, shares, weights, baselines, settings, stop, known, scorer,
    )?
    .map(|(score, reports)| JointResult {
        raw: reports[0].raw.clone(),
        totals: reports[0].totals,
        snapshot: Snapshot {
            best: Candidate {
                arr: (0..layout.slots.len()).collect(),
                score,
            },
            progress,
            archive: Vec::new(),
            has_best: true,
        },
        layout,
        score,
    }))
}

/// The start layout scored without the caps, for annealing to leave a start
/// that breaks them, and why it breaks them.
fn ungated_start(
    layout: &ak::Layout,
    corpora: &[(ng::NgramCorpus, f64)],
    shares: &[f64],
    weights: &Weights,
    baselines: &[summary::Summary],
    settings: &SearchSettings,
    stop: &AtomicBool,
    progress: Progress,
) -> ak::Result<(JointResult, String)> {
    let counter = AtomicU64::new(0);
    let (mut score, mut first, mut reason) = (0.0, None, None);
    for (((corpus, _), share), baseline) in corpora.iter().zip(shares).zip(baselines) {
        let report =
            summary::evaluate_with_mode(layout, corpus, weights, stop, &counter, &settings.mode)?;
        if *share != 0.0 {
            score += share * report.score;
            if reason.is_none() {
                reason = limit_failure(&report, baseline, settings);
            }
        }
        first.get_or_insert(report);
    }
    let first = first.ok_or("Optimization requires a corpus.")?;
    let start = JointResult {
        raw: first.raw.clone(),
        totals: first.totals,
        snapshot: Snapshot {
            best: Candidate {
                arr: (0..layout.slots.len()).collect(),
                score,
            },
            progress,
            archive: Vec::new(),
            has_best: true,
        },
        layout: layout.clone(),
        score,
    };
    Ok((start, reason.unwrap_or_else(|| "its score is not finite".into())))
}

/// The error for a run that found nothing within the caps.
fn infeasible(reason: &str) -> String {
    format!(
        "Nothing found within the caps. The start layout breaks one: {reason}. \
         Raise that cap or set it to none, give the run more time, or undo the \
         edit that caused it."
    )
}

/// Fits one worker has made, by input layout. A fit's result depends only on
/// the layout it starts from (the scorer's starting mappings only speed it
/// up), so a chunk ending where an earlier one did reuses the earlier fits.
#[derive(Default)]
struct FitMemo {
    combos: Vec<(ak::Layout, Option<(ak::Layout, Option<JointResult>)>)>,
    swaps: Vec<(ak::Layout, Option<(ak::Layout, Option<JointResult>)>)>,
    /// The candidates `fitted_candidates` reported, in order.
    candidates: Vec<(ak::Layout, Vec<JointResult>)>,
}

/// Fits each kind keeps; the oldest goes first.
const FIT_MEMO: usize = 32;

fn same_layout(a: &ak::Layout, b: &ak::Layout) -> bool {
    a.name == b.name
        && a.slots == b.slots
        && a.actions == b.actions
        && a.left_outer == b.left_outer
        && a.right_outer == b.right_outer
        && a.row_stagger == b.row_stagger
        && a.action_mode == b.action_mode
        && a.magic_rule_slots == b.magic_rule_slots
        && a.magic_refit_pending == b.magic_refit_pending
        && a.alt_fingers == b.alt_fingers
        && a.alt_spots == b.alt_spots
        && crate::action_rule_delta::same_json_slice(&a.combos, &b.combos)
}

impl FitMemo {
    fn find<'a, T>(entries: &'a [(ak::Layout, T)], layout: &ak::Layout) -> Option<&'a T> {
        entries
            .iter()
            .find(|(input, _)| same_layout(input, layout))
            .map(|(_, value)| value)
    }

    fn keep<T>(entries: &mut Vec<(ak::Layout, T)>, layout: &ak::Layout, value: T) {
        if entries.len() == FIT_MEMO {
            entries.remove(0);
        }
        entries.push((layout.clone(), value));
    }
}

/// A remembered fit with the progress a fresh fit would carry now.
fn refreshed(
    fit: &Option<(ak::Layout, Option<JointResult>)>,
    progress: &Progress,
) -> Option<(ak::Layout, Option<JointResult>)> {
    fit.clone().map(|(layout, candidate)| {
        (
            layout,
            candidate.map(|mut candidate| {
                candidate.snapshot.progress = progress.clone();
                candidate
            }),
        )
    })
}

/// Refit `layout`'s combos on the primary corpus, aiming within the caps,
/// then score the result on the whole training mixture: the refit layout, and
/// its candidate when within the caps. None when combo fitting is off.
#[allow(clippy::too_many_arguments)]
fn fitted_combos(
    layout: &ak::Layout,
    corpora: &[(ng::NgramCorpus, f64)],
    shares: &[f64],
    weights: &Weights,
    physical: &Arc<summary::PhysicalTables>,
    combos: &crate::combo_fit::Settings,
    baselines: &[summary::Summary],
    settings: &SearchSettings,
    stop: &AtomicBool,
    progress: Progress,
    threads: usize,
    scorer: Option<&mut ExactScorer<'_>>,
) -> ak::Result<Option<(ak::Layout, Option<JointResult>)>> {
    if combos.active_max(weights) == 0 {
        return Ok(None);
    }
    let Some(primary) = shares.iter().position(|&share| share != 0.0) else {
        return Ok(None);
    };
    let fitted = crate::combo_fit::fit(
        layout,
        &corpora[primary].0,
        weights,
        physical,
        combos,
        Some(action_search::Caps {
            baseline: &baselines[primary].metrics,
            settings,
        }),
        stop,
        &AtomicU64::new(0),
        threads,
        scorer
            .as_ref()
            .and_then(|scorer| scorer.previous[primary].as_ref()),
    )?;
    let candidate = exact_candidate(
        fitted.layout.clone(),
        corpora,
        shares,
        weights,
        baselines,
        settings,
        stop,
        progress,
        scorer,
    )?;
    Ok(Some((fitted.layout, candidate)))
}

/// Refit `layout`'s adaptive swaps on the primary corpus as `fitted_combos`
/// refits combos.
#[allow(clippy::too_many_arguments)]
fn fitted_swaps(
    layout: &ak::Layout,
    corpora: &[(ng::NgramCorpus, f64)],
    shares: &[f64],
    weights: &Weights,
    physical: &Arc<summary::PhysicalTables>,
    swaps: &crate::swap_fit::Settings,
    baselines: &[summary::Summary],
    settings: &SearchSettings,
    stop: &AtomicBool,
    progress: Progress,
    threads: usize,
    scorer: Option<&mut ExactScorer<'_>>,
) -> ak::Result<Option<(ak::Layout, Option<JointResult>)>> {
    if swaps.max_swaps == 0 {
        return Ok(None);
    }
    let Some(primary) = shares.iter().position(|&share| share != 0.0) else {
        return Ok(None);
    };
    let fitted = crate::swap_fit::fit(
        layout,
        &corpora[primary].0,
        weights,
        physical,
        swaps,
        Some(action_search::Caps {
            baseline: &baselines[primary].metrics,
            settings,
        }),
        stop,
        &AtomicU64::new(0),
        threads,
        scorer
            .as_ref()
            .and_then(|scorer| scorer.previous[primary].as_ref()),
    )?;
    let candidate = exact_candidate(
        fitted.layout.clone(),
        corpora,
        shares,
        weights,
        baselines,
        settings,
        stop,
        progress,
        scorer,
    )?;
    Ok(Some((fitted.layout, candidate)))
}

#[allow(clippy::too_many_arguments)]
fn fitted_candidates(
    layout: &ak::Layout,
    action_name: &str,
    locks: &[bool],
    corpora: &[(ng::NgramCorpus, f64)],
    shares: &[f64],
    fitters: &[(usize, design::FixedRootFitter<'_>)],
    weights: &Weights,
    settings: &SearchSettings,
    baselines: &[summary::Summary],
    control: &Control,
    workers: usize,
    include_repairs: bool,
    progress: &Progress,
    deadline: Option<Instant>,
    on_update: &mut dyn FnMut(FitUpdate),
) -> ak::Result<()> {
    let root = locate_root(layout, action_name)?;
    let mut seeds = vec![layout.clone()];
    let counter = AtomicU64::new(0);
    if include_repairs {
        on_update(FitUpdate::Phase("Preparing magic-rule repair candidates".into()));
        if wait_if_paused(control, deadline) {
            return Err("cancelled".into());
        }
        if let Some(primary) = corpora
            .iter()
            .zip(shares)
            .find(|(_, share)| **share != 0.0)
            .map(|((corpus, _), _)| corpus)
        {
            let mut repair = design::fixed_root_repair_seeds(
                layout,
                root,
                locks,
                primary,
                weights,
                &settings.mode,
                &control.cancel,
                &counter,
            )?;
            repair.truncate(JOINT_ARCHIVE_SEEDS);
            seeds.extend(repair);
        }
    }
    let mut unique = Vec::with_capacity(seeds.len());
    for seed in seeds {
        if !unique
            .iter()
            .any(|known: &ak::Layout| known.slots == seed.slots && known.actions == seed.actions)
        {
            unique.push(seed);
        }
    }
    let total = unique.len() * fitters.len();
    let mut completed = 0usize;
    let mut emitted: Vec<(Vec<ak::Slot>, BTreeMap<String, ak::Action>)> = Vec::new();
    for seed in unique {
        let seed_root = locate_root(&seed, action_name)?;
        for (corpus_index, fitter) in fitters {
            if wait_if_paused(control, deadline) {
                return Err("cancelled".into());
            }
            on_update(FitUpdate::Phase(format!(
                "Fitting magic rules {}/{}",
                completed + 1,
                total
            )));
            let (fitted, fitted_summary) =
                fitter.refit(&seed, seed_root, workers, &control.cancel, &counter)?;
            completed += 1;
            if wait_if_paused(control, deadline) {
                return Err("cancelled".into());
            }
            if let Some(candidate) = exact_candidate_reusing(
                fitted,
                corpora,
                shares,
                weights,
                baselines,
                settings,
                &control.cancel,
                progress.clone(),
                Some((*corpus_index, &fitted_summary)),
                None,
            )? {
                if !emitted.iter().any(|(slots, actions)| {
                    *slots == candidate.layout.slots && *actions == candidate.layout.actions
                }) {
                    emitted.push((
                        candidate.layout.slots.clone(),
                        candidate.layout.actions.clone(),
                    ));
                    on_update(FitUpdate::Candidate(candidate));
                }
            }
        }
    }
    Ok(())
}

enum FitUpdate {
    Phase(String),
    Candidate(JointResult),
}

enum JointMessage {
    Live {
        layout: ak::Layout,
        snapshot: Snapshot,
        raw: Raw,
        totals: [f64; 4],
        /// Within every cap, so its score is exact and it may be a result;
        /// otherwise it is only shown (annealing charges `SOFT_CAP_POINTS`).
        within_caps: bool,
    },
    Phase(Progress),
    Exact(JointResult),
    Done(ak::Result<Option<JointResult>>),
    Panicked,
}

fn expired(deadline: Option<Instant>) -> bool {
    deadline.is_some_and(|value| Instant::now() >= value)
}

/// What finished restarts leave for later rounds: results `evolve` crosses,
/// and round 0's best score after every chunk, which later restarts must keep
/// up with (`thresholds`). Each worker's restart i reads only rounds before i,
/// once every worker has finished them, so nothing depends on timing.
struct RoundPool {
    /// Restarts each worker runs.
    rounds: Vec<usize>,
    state: std::sync::Mutex<RoundState>,
    ready: std::sync::Condvar,
}

struct RoundState {
    /// Rounds each worker has finished (`usize::MAX` once it stops).
    done: Vec<usize>,
    /// Results with their rounds, for `evolve`.
    parents: Vec<(usize, Candidate)>,
    /// Round 0 chains' best score after each chunk.
    paths: Vec<Vec<f64>>,
}

impl RoundPool {
    fn new(rounds: Vec<usize>) -> Self {
        Self {
            state: std::sync::Mutex::new(RoundState {
                done: vec![0; rounds.len()],
                parents: Vec::new(),
                paths: Vec::new(),
            }),
            rounds,
            ready: std::sync::Condvar::new(),
        }
    }

    /// The state once every worker has finished the rounds before `round`
    /// (or the run is stopping).
    fn after(
        &self,
        round: usize,
        control: &Control,
        deadline: Option<Instant>,
    ) -> std::sync::MutexGuard<'_, RoundState> {
        let mut state = self.state.lock().unwrap();
        while !state
            .done
            .iter()
            .zip(&self.rounds)
            .all(|(&done, &total)| done >= round.min(total))
            && !control.cancel.load(Ordering::Relaxed)
            && !expired(deadline)
        {
            state = self
                .ready
                .wait_timeout(state, Duration::from_millis(50))
                .unwrap()
                .0;
        }
        state
    }

    /// Results of the rounds before `round`, best first.
    fn parents(
        &self,
        round: usize,
        control: &Control,
        deadline: Option<Instant>,
    ) -> Vec<Candidate> {
        let state = self.after(round, control, deadline);
        let mut parents: Vec<_> = state
            .parents
            .iter()
            .filter(|(of, _)| *of < round)
            .map(|(_, candidate)| candidate.clone())
            .collect();
        parents.sort_by(|a, b| a.score.total_cmp(&b.score).then_with(|| a.arr.cmp(&b.arr)));
        parents
    }

    /// Per chunk, the worst best-so-far score among round 0's top quarter (at
    /// least three chains) there. A later chain behind all of them after a
    /// chunk is abandoned: on finch with 64 restarts, results were the same
    /// in about 24% less time. None without three round 0 chains.
    fn thresholds(&self, control: &Control, deadline: Option<Instant>) -> Option<Vec<f64>> {
        let state = self.after(1, control, deadline);
        let mut paths = state.paths.clone();
        if paths.len() < 3 || paths.iter().any(Vec::is_empty) {
            return None;
        }
        let order = |a: &Vec<f64>, b: &Vec<f64>| {
            a.last()
                .unwrap()
                .total_cmp(b.last().unwrap())
                .then_with(|| {
                    a.iter()
                        .zip(b)
                        .map(|(x, y)| x.total_cmp(y))
                        .find(|order| order.is_ne())
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        };
        paths.sort_by(order);
        let top = &paths[..(paths.len() / 4).max(3)];
        let chunks = top.iter().map(Vec::len).min().unwrap_or(0);
        Some(
            (0..chunks)
                .map(|chunk| {
                    top.iter()
                        .map(|path| path[chunk])
                        .fold(f64::NEG_INFINITY, f64::max)
                })
                .collect(),
        )
    }

    /// Finish `worker`'s next round with its result (for `evolve`) and its
    /// best score after each chunk.
    fn publish(&self, worker: usize, result: Option<Candidate>, path: Vec<f64>) {
        let mut state = self.state.lock().unwrap();
        let round = state.done[worker];
        if let Some(candidate) = result {
            state.parents.push((round, candidate));
        }
        if round == 0 {
            state.paths.push(path);
        }
        state.done[worker] += 1;
        self.ready.notify_all();
    }

    fn finish(&self, worker: usize) {
        self.state.lock().unwrap().done[worker] = usize::MAX;
        self.ready.notify_all();
    }
}

/// Marks a worker finished however its restarts end, so no other waits on it.
struct Finished<'a>(&'a RoundPool, usize);

impl Drop for Finished<'_> {
    fn drop(&mut self) {
        self.0.finish(self.1);
    }
}

fn merge_progress(
    progress: &mut Progress,
    evaluations: &mut [u64],
    accepted: &mut [u64],
    started: Instant,
    seed: u64,
) {
    let restart = progress
        .restart
        .saturating_sub(1)
        .min(evaluations.len() - 1);
    evaluations[restart] = evaluations[restart].max(progress.evaluations);
    accepted[restart] = accepted[restart].max(progress.accepted);
    progress.evaluations = evaluations.iter().copied().sum();
    progress.accepted = accepted.iter().copied().sum();
    progress.restart = progress.restart.max(
        evaluations
            .iter()
            .rposition(|&value| value != 0)
            .map_or(0, |index| index + 1),
    );
    progress.elapsed = started.elapsed().as_secs_f64();
    progress.seed = seed;
}

/// `layout` with no more combos and swaps than the fits allow: above a limit
/// the least useful are dropped, on the primary corpus; nothing is added.
#[allow(clippy::too_many_arguments)]
fn within_piece_limits(
    layout: &ak::Layout,
    corpora: &[(ng::NgramCorpus, f64)],
    shares: &[f64],
    weights: &Weights,
    physical: &Arc<summary::PhysicalTables>,
    combos: &crate::combo_fit::Settings,
    swaps: &crate::swap_fit::Settings,
    stop: &AtomicBool,
) -> ak::Result<ak::Layout> {
    let Some(primary) = shares.iter().position(|&share| share != 0.0) else {
        return Ok(layout.clone());
    };
    let corpus = &corpora[primary].0;
    let mut layout = layout.clone();
    let max_combos = combos.active_max(weights);
    if max_combos > 0 && layout.combos.len() > max_combos {
        let trim = crate::combo_fit::Settings {
            shortlist: 0,
            exact: false,
            ..combos.clone()
        };
        layout = crate::combo_fit::fit(
            &layout,
            corpus,
            weights,
            physical,
            &trim,
            None,
            stop,
            &AtomicU64::new(0),
            1,
            None,
        )?
        .layout;
    }
    if swaps.max_swaps > 0 && crate::swap_fit::split(&layout).1.len() > swaps.max_swaps {
        let trim = crate::swap_fit::Settings {
            shortlist: 0,
            exact: false,
            ..swaps.clone()
        };
        layout = crate::swap_fit::fit(
            &layout,
            corpus,
            weights,
            physical,
            &trim,
            None,
            stop,
            &AtomicU64::new(0),
            1,
            None,
        )?
        .layout;
    }
    Ok(layout)
}

/// Random combo or swap changes a chain tries after each chunk's fits.
const DICTIONARY_MOVES: usize = 4;

/// One random change to `layout`'s combos or adaptive swaps (whichever the
/// run fits), on the primary corpus; None when neither can change.
fn dictionary_neighbor(
    layout: &ak::Layout,
    corpora: &[(ng::NgramCorpus, f64)],
    shares: &[f64],
    weights: &Weights,
    combos: &crate::combo_fit::Settings,
    swaps: &crate::swap_fit::Settings,
    pick: &mut dyn FnMut(usize) -> usize,
) -> Option<ak::Layout> {
    let corpus = &corpora[shares.iter().position(|&share| share != 0.0)?].0;
    let fit_combos = combos.active_max(weights) > 0;
    let fit_swaps = swaps.max_swaps > 0;
    let combos_first = match (fit_combos, fit_swaps) {
        (false, false) => return None,
        (true, true) => pick(2) == 0,
        (only_combos, _) => only_combos,
    };
    // The other kind when the first cannot change.
    for combo in [combos_first, !combos_first] {
        let proposed = if combo {
            fit_combos
                .then(|| crate::combo_fit::neighbor(layout, corpus, weights, combos, pick))
                .flatten()
        } else {
            fit_swaps
                .then(|| crate::swap_fit::neighbor(layout, corpus, swaps, pick))
                .flatten()
        };
        if proposed.is_some() {
            return proposed;
        }
    }
    None
}

/// Whether `candidate` beats a chain's best so far, if any.
fn improves(candidate: &JointResult, best: &Option<JointResult>) -> bool {
    best.as_ref().is_none_or(|best| better(candidate, best))
}

#[allow(clippy::too_many_arguments)]
fn run_restart_chain(
    initial: &JointResult,
    // False when `initial` breaks the caps: it is then only a place to start.
    initial_feasible: bool,
    action_name: Option<&str>,
    locks: &[bool],
    corpora: &[(ng::NgramCorpus, f64)],
    shares: &[f64],
    fitters: &[(usize, design::FixedRootFitter<'_>)],
    combos: &crate::combo_fit::Settings,
    swaps: &crate::swap_fit::Settings,
    fit_physical: &Arc<summary::PhysicalTables>,
    weights: &Weights,
    settings: &SearchSettings,
    baselines: &[summary::Summary],
    baseline_metrics: &[Metrics],
    control: &Control,
    deadline: Option<Instant>,
    restart: usize,
    parents: &[Candidate],
    thresholds: Option<&[f64]>,
    tx: &mpsc::Sender<JointMessage>,
    memo: &mut FitMemo,
) -> ak::Result<(Option<JointResult>, Vec<f64>)> {
    let chunks = chunk_sizes(settings.anneal_steps, settings.hybrid);
    // The chain's best score after each chunk (`RoundPool::thresholds`).
    let mut path = Vec::with_capacity(chunks.len());
    let mut completed_steps = 0usize;
    let mut total_evaluations = 0u64;
    let mut total_accepted = 0u64;
    let mut current = initial.layout.clone();
    // A generated restart (random or evolve) follows its own arrangement:
    // measured against the start, its first chunk rarely wins and would be
    // dropped for the start, so every restart would refine the start.
    let mut best = (settings.design == "refine" && initial_feasible).then(|| initial.clone());
    let started = Instant::now();
    // Without one (other physical slots), candidates are evaluated afresh.
    let mut scorer = ExactScorer::new(&initial.layout, corpora, weights, fit_physical).ok();

    for (chunk, &steps) in chunks.iter().enumerate() {
        if wait_if_paused(control, deadline) {
            break;
        }
        let mut epoch = settings.clone();
        epoch.restarts = 1;
        epoch.anneal_steps = steps;
        epoch.temperature = at_temperature(settings, completed_steps);
        epoch.cooling_end = at_temperature(settings, completed_steps + steps);
        epoch.seed = crate::worker_seed(settings.seed, restart * MAX_JOINT_EPOCHS + chunk);
        // Generate only at the beginning of an independent restart. Later
        // chunks continue the arrangement selected by the preceding chunk.
        if chunk != 0 {
            epoch.design = "refine".into();
        }
        epoch.seconds = deadline.map_or(0.0, |end| {
            end.saturating_duration_since(Instant::now()).as_secs_f64()
        });
        if deadline.is_some() && epoch.seconds == 0.0 {
            break;
        }
        let run_sweeps = chunk + 1 == chunks.len();
        let offset_evaluations = total_evaluations;
        let offset_accepted = total_accepted;
        let fixed_dictionary = current.clone();
        let mut live = |mut snapshot: Snapshot, _baseline: Metrics, raw: Raw, totals: [f64; 4]| {
            if !snapshot.has_best {
                return;
            }
            let layout = action_search::arrangement(&fixed_dictionary, &snapshot.best.arr);
            snapshot.progress.restart = restart + 1;
            snapshot.progress.evaluations =
                offset_evaluations.saturating_add(snapshot.progress.evaluations);
            snapshot.progress.accepted = offset_accepted.saturating_add(snapshot.progress.accepted);
            snapshot.progress.elapsed = started.elapsed().as_secs_f64();
            let score = snapshot.best.score;
            snapshot = identity_snapshot(snapshot, layout.slots.len(), score);
            // Only the first corpus's counters arrive; a mixture with caps is
            // left to the exact candidates after each chunk.
            let capped = [
                settings.sfb_limit,
                settings.sfs_limit,
                settings.travel_limit,
                settings.sftravel_limit,
            ]
            .iter()
            .any(Option::is_some);
            let within_caps = !capped
                || corpora.len() == 1
                    && within_limits(
                        &metrics_totals(&raw, &totals),
                        &baseline_metrics[0],
                        settings,
                    );
            let _ = tx.send(JointMessage::Live {
                layout,
                snapshot,
                raw,
                totals,
                within_caps,
            });
        };
        let snapshot = action_search::run_joint_epoch(
            &current,
            corpora,
            weights,
            &epoch,
            locks,
            control,
            baseline_metrics,
            run_sweeps,
            if chunk == 0 { restart } else { 0 },
            if chunk == 0 { parents } else { &[] },
            &mut live,
        )?;
        total_evaluations = total_evaluations.saturating_add(snapshot.progress.evaluations);
        total_accepted = total_accepted.saturating_add(snapshot.progress.accepted);
        if snapshot.has_best {
            current = action_search::arrangement(&current, &snapshot.best.arr);
        }
        completed_steps += steps;

        let mut progress = snapshot.progress.clone();
        progress.restart = restart + 1;
        progress.evaluations = total_evaluations;
        progress.accepted = total_accepted;
        progress.elapsed = started.elapsed().as_secs_f64();
        progress.phase = "fitting rules".into();

        let annealed = exact_candidate(
            current.clone(),
            corpora,
            shares,
            weights,
            baselines,
            settings,
            &control.cancel,
            progress.clone(),
            scorer.as_mut(),
        )?;
        // Annealing may end above a cap (`SOFT_CAP_POINTS`): the fits below
        // then start from this arrangement to bring it back within the caps.
        let within_caps = annealed.is_some();
        if let Some(candidate) = annealed {
            if improves(&candidate, &best) {
                best = Some(candidate.clone());
            }
            let _ = tx.send(JointMessage::Exact(candidate));
        }
        let mut repaired: Option<JointResult> = None;
        // Refit after every annealing checkpoint from the arrangement alone;
        // exact mixture evaluation decides whether the refit is retained.
        let mut update = |update: FitUpdate| match update {
            FitUpdate::Phase(phase) => {
                let mut fitting = progress.clone();
                fitting.phase = phase;
                fitting.elapsed = started.elapsed().as_secs_f64();
                let _ = tx.send(JointMessage::Phase(fitting));
            }
            FitUpdate::Candidate(candidate) => {
                if !within_caps
                    && repaired
                        .as_ref()
                        .is_none_or(|known| better(&candidate, known))
                {
                    repaired = Some(candidate.clone());
                }
                if improves(&candidate, &best) {
                    best = Some(candidate.clone());
                }
                let _ = tx.send(JointMessage::Exact(candidate));
            }
        };
        if let Some(action_name) = action_name {
            if let Some(known) = FitMemo::find(&memo.candidates, &current) {
                for candidate in known {
                    let mut candidate = candidate.clone();
                    candidate.snapshot.progress = progress.clone();
                    update(FitUpdate::Candidate(candidate));
                }
            } else {
                let mut reported = Vec::new();
                fitted_candidates(
                    &current,
                    action_name,
                    locks,
                    corpora,
                    shares,
                    fitters,
                    weights,
                    settings,
                    baselines,
                    control,
                    1,
                    // Repair seeds pay off only on the injected start: at every
                    // checkpoint they matched or lost to no repairs given equal time.
                    false,
                    &progress,
                    deadline,
                    &mut |fit| {
                        if let FitUpdate::Candidate(candidate) = &fit {
                            reported.push(candidate.clone());
                        }
                        update(fit);
                    },
                )?;
                FitMemo::keep(&mut memo.candidates, &current, reported);
            }
        }
        // Refit combos on the best layout so far, rules included. The next
        // chunk's rule refit then sees these combos in place.
        // Refitting only after a restart's last chunk lost quality at equal
        // time: the next chunk anneals better with the refit pieces in place.
        // Above a cap, the arrangement (or its best rule repair) is refit
        // instead, so combos and swaps can bring it back within the caps.
        if !wait_if_paused(control, deadline) {
            let mut start = match (within_caps, repaired) {
                (true, _) => None,
                (false, Some(repaired)) => Some(repaired.layout),
                (false, None) => Some(current.clone()),
            };
            let mut fitting = progress.clone();
            fitting.phase = "fitting combos".into();
            let input = start
                .as_ref()
                .or(best.as_ref().map(|best| &best.layout))
                .unwrap_or(&current)
                .clone();
            let fit = match FitMemo::find(&memo.combos, &input) {
                Some(known) => refreshed(known, &fitting),
                None => {
                    let fit = fitted_combos(
                        &input,
                        corpora,
                        shares,
                        weights,
                        fit_physical,
                        combos,
                        baselines,
                        settings,
                        &control.cancel,
                        fitting,
                        1,
                        scorer.as_mut(),
                    )?;
                    FitMemo::keep(&mut memo.combos, &input, fit.clone());
                    fit
                }
            };
            if let Some((fitted, candidate)) = fit {
                if let Some(candidate) = candidate {
                    if improves(&candidate, &best) {
                        best = Some(candidate.clone());
                    }
                    let _ = tx.send(JointMessage::Exact(candidate));
                }
                if start.is_some() {
                    start = Some(fitted);
                }
            }
            let mut fitting = progress.clone();
            fitting.phase = "fitting swaps".into();
            let input = start
                .as_ref()
                .or(best.as_ref().map(|best| &best.layout))
                .unwrap_or(&current)
                .clone();
            let fit = match FitMemo::find(&memo.swaps, &input) {
                Some(known) => refreshed(known, &fitting),
                None => {
                    let fit = fitted_swaps(
                        &input,
                        corpora,
                        shares,
                        weights,
                        fit_physical,
                        swaps,
                        baselines,
                        settings,
                        &control.cancel,
                        fitting,
                        1,
                        scorer.as_mut(),
                    )?;
                    FitMemo::keep(&mut memo.swaps, &input, fit.clone());
                    fit
                }
            };
            if let Some((_, Some(candidate))) = fit {
                if improves(&candidate, &best) {
                    best = Some(candidate.clone());
                }
                let _ = tx.send(JointMessage::Exact(candidate));
            }
        }
        // Continue with the best arrangement and dictionary found in this
        // chain rather than resetting to the injection seed.
        if let Some(best) = &best {
            current = best.layout.clone();
        }
        // The fits above only ever improve. Also try a few random combo and
        // swap changes, kept by the annealing rule at this point's
        // temperature, so the next chunk can explore arrangements that pay
        // off only with other combos or swaps.
        if chunk + 1 < chunks.len() && !wait_if_paused(control, deadline) {
            if let Some(start) = best.as_ref() {
                let temperature = at_temperature(settings, completed_steps);
                let mut rng = crate::Rng::new(crate::worker_seed(
                    settings.seed ^ 0x5eed_d1c7,
                    restart * MAX_JOINT_EPOCHS + chunk,
                ));
                let mut held = start.clone();
                for _ in 0..DICTIONARY_MOVES {
                    let Some(proposed) = dictionary_neighbor(
                        &held.layout,
                        corpora,
                        shares,
                        weights,
                        combos,
                        swaps,
                        &mut |n| rng.index(n),
                    ) else {
                        break;
                    };
                    let Some(candidate) = exact_candidate(
                        proposed,
                        corpora,
                        shares,
                        weights,
                        baselines,
                        settings,
                        &control.cancel,
                        progress.clone(),
                        scorer.as_mut(),
                    )?
                    else {
                        continue;
                    };
                    let delta = candidate.score - held.score;
                    if delta < 0.0 || rng.unit() < (-delta / temperature.max(1e-12)).exp() {
                        if improves(&candidate, &best) {
                            best = Some(candidate.clone());
                            let _ = tx.send(JointMessage::Exact(candidate.clone()));
                        }
                        held = candidate;
                    }
                }
                current = held.layout;
            }
        }
        let score = best.as_ref().map_or(f64::INFINITY, |best| best.score);
        path.push(score);
        // Behind every top round 0 chain at this point: give up. The first
        // and last chunks are never cut.
        if chunk > 0
            && chunk + 1 < chunks.len()
            && thresholds
                .is_some_and(|limits| limits.get(chunk).is_some_and(|&limit| score > limit))
        {
            break;
        }
    }
    Ok((best, path))
}

/// Jointly optimize bindings, the physical position of one magic action and
/// its dictionary, and the layout's combos. A locked root remains fixed; an
/// unlocked root participates in ordinary swaps and is located again before
/// every rule refit. Without a root only bindings and combos are optimized;
/// with no active combo count the combos are left as they are. The
/// dedicated designer remains separate.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_controlled(
    original: &ak::Layout,
    seed: &ak::Layout,
    root: Option<usize>,
    locks: &[bool],
    corpora: &[(ng::NgramCorpus, f64)],
    weights: &Weights,
    settings: &SearchSettings,
    magic: &crate::MagicSettings,
    combos: &crate::combo_fit::Settings,
    swaps: &crate::swap_fit::Settings,
    control: &Control,
    callback: &mut dyn FnMut(ak::Layout, Snapshot, Metrics, Raw, [f64; 4]),
) -> ak::Result<(ak::Layout, Snapshot)> {
    run_controlled_with_status(
        original,
        seed,
        root,
        locks,
        corpora,
        weights,
        settings,
        magic,
        combos,
        swaps,
        control,
        callback,
        &mut |_| {},
    )
}

/// Status is independent of eligible candidates: refreshed rules can take
/// time to fit before a layout satisfies every cap and retained rule count.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_controlled_with_status(
    original: &ak::Layout,
    seed: &ak::Layout,
    root: Option<usize>,
    locks: &[bool],
    corpora: &[(ng::NgramCorpus, f64)],
    weights: &Weights,
    settings: &SearchSettings,
    magic: &crate::MagicSettings,
    combos: &crate::combo_fit::Settings,
    swaps: &crate::swap_fit::Settings,
    control: &Control,
    callback: &mut dyn FnMut(ak::Layout, Snapshot, Metrics, Raw, [f64; 4]),
    status: &mut dyn FnMut(&str),
) -> ak::Result<(ak::Layout, Snapshot)> {
    crate::validate_search_settings(settings).map_err(|error| error.to_string())?;
    let settings = &settings.for_actions();
    if locks.len() != seed.slots.len() {
        return Err("Optimization requires one lock per physical slot.".into());
    }
    let mut ready_seed = seed.clone();
    ready_seed.magic_refit_pending.clear();
    let seed = &ready_seed;
    let action_name = match root {
        Some(root) => {
            if root >= seed.slots.len() {
                return Err("Magic optimization requires a valid physical root.".into());
            }
            let ak::Binding::Named(action_name) = &seed.slots[root].binding else {
                return Err("Magic optimization requires a named action root.".into());
            };
            locate_root(seed, action_name)?;
            Some(action_name.clone())
        }
        None => None,
    };
    let shares = normalized_shares(corpora)?;
    // Combo fits share one set of physical tables: keys move, the board does not.
    status("Preparing physical geometry");
    let fit_physical = summary::PhysicalTables::with_mode(seed, weights, &settings.mode);
    // The combo and swap limits count every piece, the layout's own included:
    // start from, and measure the caps against, the layouts cut down to them.
    let limited = |layout: &ak::Layout| {
        within_piece_limits(
            layout,
            corpora,
            &shares,
            weights,
            &fit_physical,
            combos,
            swaps,
            &control.cancel,
        )
    };
    status("Checking combo and adaptive-swap limits");
    let original = &limited(original)?;
    let seed = &limited(seed)?;
    let started = Instant::now();
    let deadline =
        (settings.seconds > 0.0).then(|| started + Duration::from_secs_f64(settings.seconds));
    let progress_counter = AtomicU64::new(0);
    status("Evaluating the original layout");
    let mut baselines = Vec::with_capacity(corpora.len());
    for (corpus, _) in corpora {
        baselines.push(summary::evaluate_with_mode(
            original,
            corpus,
            weights,
            &control.cancel,
            &progress_counter,
            &settings.mode,
        )?);
    }
    let baseline_metrics: Vec<_> = baselines
        .iter()
        .map(|summary| summary.metrics.clone())
        .collect();
    let fit_settings = rule_settings(root.unwrap_or(0), settings, magic);
    let fitters: Vec<_> = corpora
        .iter()
        .zip(&shares)
        .zip(&baselines)
        .enumerate()
        .filter_map(|(index, (((corpus, _), share), baseline))| {
            (*share != 0.0 && root.is_some()).then(|| {
                (
                    index,
                    design::FixedRootFitter::new(
                        seed,
                        corpus,
                        weights,
                        &fit_settings,
                        baseline.clone(),
                        &settings.mode,
                    ),
                )
            })
        })
        .collect();
    let initial_progress = Progress {
        phase: "fitting rules".into(),
        seed: settings.seed,
        ..Progress::default()
    };

    status("Evaluating the starting layout");
    let mut best = exact_candidate(
        seed.clone(),
        corpora,
        &shares,
        weights,
        &baselines,
        settings,
        &control.cancel,
        initial_progress.clone(),
        None,
    )?;
    if let Some(value) = &best {
        emit(callback, value, &baselines[0]);
    }

    // Repair the injected seed before annealing. In particular, a displaced
    // letter may temporarily violate a cap even though a rule can repair it.
    if !wait_if_paused(control, deadline) {
        let mut fit_error = None;
        let mut update = |update: FitUpdate| match update {
            FitUpdate::Phase(phase) => {
                status(&phase);
                if let Some(value) = &best {
                    let mut shown = value.clone();
                    shown.snapshot.progress.phase = phase;
                    shown.snapshot.progress.elapsed = started.elapsed().as_secs_f64();
                    emit(callback, &shown, &baselines[0]);
                }
            }
            FitUpdate::Candidate(candidate) => {
                if best
                    .as_ref()
                    .is_none_or(|current| better(&candidate, current))
                {
                    best = Some(candidate);
                }
                if let Some(value) = &best {
                    emit(callback, value, &baselines[0]);
                }
            }
        };
        let fitted = match &action_name {
            Some(action_name) => fitted_candidates(
                seed,
                action_name,
                locks,
                corpora,
                &shares,
                &fitters,
                weights,
                settings,
                &baselines,
                control,
                crate::search_worker_count(settings),
                true,
                &initial_progress,
                deadline,
                &mut update,
            ),
            None => Ok(()),
        };
        match fitted {
            Ok(()) => {}
            Err(error) if error == "cancelled" && best.is_some() => {}
            Err(error) => fit_error = Some(error),
        }
        if let Some(error) = fit_error {
            return Err(error);
        }
        // Fit combos to the seed before annealing, starting from the best
        // seed so far.
        status("Fitting combos");
        let start = best.as_ref().map_or(seed, |value| &value.layout);
        if let Some((_, Some(candidate))) = fitted_combos(
            start,
            corpora,
            &shares,
            weights,
            &fit_physical,
            combos,
            &baselines,
            settings,
            &control.cancel,
            initial_progress.clone(),
            // Before annealing, every worker is free.
            crate::search_worker_count(settings),
            None,
        )? {
            if best
                .as_ref()
                .is_none_or(|current| better(&candidate, current))
            {
                best = Some(candidate);
            }
            if let Some(value) = &best {
                emit(callback, value, &baselines[0]);
            }
        }
        status("Fitting adaptive swaps");
        let start = best.as_ref().map_or(seed, |value| &value.layout);
        if let Some((_, Some(candidate))) = fitted_swaps(
            start,
            corpora,
            &shares,
            weights,
            &fit_physical,
            swaps,
            &baselines,
            settings,
            &control.cancel,
            initial_progress.clone(),
            // Before annealing, every worker is free.
            crate::search_worker_count(settings),
            None,
        )? {
            if best
                .as_ref()
                .is_none_or(|current| better(&candidate, current))
            {
                best = Some(candidate);
            }
            if let Some(value) = &best {
                emit(callback, value, &baselines[0]);
            }
        }
    }
    // A start that breaks the caps (an edit raised SFB, say) is annealed
    // anyway: annealing may pass above a cap and comes back under it.
    let mut infeasible_reason = None;
    let initial = match best.clone() {
        Some(initial) => initial,
        None if control.cancel.load(Ordering::Relaxed) => return Err("cancelled".into()),
        None => {
            let (start, reason) = ungated_start(
                seed,
                corpora,
                &shares,
                weights,
                &baselines,
                settings,
                &control.cancel,
                initial_progress.clone(),
            )?;
            if expired(deadline) {
                return Err(infeasible(&reason));
            }
            infeasible_reason = Some(reason);
            start
        }
    };
    let initial_feasible = infeasible_reason.is_none();
    if control.cancel.load(Ordering::Relaxed) || expired(deadline) {
        if let Some(reason) = &infeasible_reason {
            return Err(if control.cancel.load(Ordering::Relaxed) {
                "cancelled".into()
            } else {
                infeasible(reason)
            });
        }
        let mut snapshot = identity_snapshot(
            initial.snapshot.clone(),
            initial.layout.slots.len(),
            initial.score,
        );
        snapshot.progress.phase = if control.cancel.load(Ordering::Relaxed) {
            "stopped".into()
        } else {
            "time limit".into()
        };
        snapshot.progress.elapsed = started.elapsed().as_secs_f64();
        snapshot.progress.seed = settings.seed;
        return Ok((initial.layout, snapshot));
    }

    let worker_count = crate::search_worker_count(settings);
    let (tx, rx) = mpsc::channel();
    let mut fatal_error = None;
    let mut restart_evaluations = vec![0u64; settings.restarts];
    let mut restart_accepted = vec![0u64; settings.restarts];
    let evolve = settings.design == "evolve";
    status("Preparing binding searches");
    let pool = RoundPool::new(
        (0..worker_count)
            .map(|worker| {
                settings.restarts * (worker + 1) / worker_count
                    - settings.restarts * worker / worker_count
            })
            .collect(),
    );
    thread::scope(|scope| {
        let pool_ref = &pool;
        let shares_ref = &shares;
        let baselines_ref = &baselines;
        let baseline_metrics_ref = &baseline_metrics;
        let fitters_ref = &fitters;
        let fit_physical_ref = &fit_physical;
        let action_name_ref = action_name.as_deref();
        for worker in 0..worker_count {
            let tx = tx.clone();
            let first = settings.restarts * worker / worker_count;
            let end = settings.restarts * (worker + 1) / worker_count;
            let initial = initial.clone();
            scope.spawn(move || {
                let _finished = Finished(pool_ref, worker);
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let mut local: Option<JointResult> = None;
                    let (mut thresholds, mut fetched) = (None, false);
                    let mut memo = FitMemo::default();
                    for restart in first..end {
                        if control.cancel.load(Ordering::Relaxed) || expired(deadline) {
                            break;
                        }
                        let parents = if evolve {
                            pool_ref.parents(restart - first, control, deadline)
                        } else {
                            Vec::new()
                        };
                        if restart > first && !fetched {
                            thresholds = pool_ref.thresholds(control, deadline);
                            fetched = true;
                        }
                        let (result, path) = run_restart_chain(
                            &initial,
                            initial_feasible,
                            action_name_ref,
                            locks,
                            corpora,
                            shares_ref,
                            fitters_ref,
                            combos,
                            swaps,
                            fit_physical_ref,
                            weights,
                            settings,
                            baselines_ref,
                            baseline_metrics_ref,
                            control,
                            deadline,
                            restart,
                            &parents,
                            thresholds.as_deref(),
                            &tx,
                            &mut memo,
                        )?;
                        // Parents are letter arrangements of the start: a
                        // result's rules, combos and swaps are refit.
                        pool_ref.publish(
                            worker,
                            result.as_ref().filter(|_| evolve).and_then(|result| {
                                Some(Candidate {
                                    arr: action_search::letter_arrangement(
                                        &initial.layout,
                                        &result.layout,
                                    )?,
                                    score: result.score,
                                })
                            }),
                            path,
                        );
                        if let Some(candidate) = result {
                            if local.as_ref().is_none_or(|best| better(&candidate, best)) {
                                local = Some(candidate);
                            }
                        }
                    }
                    Ok(local)
                }));
                let message = match outcome {
                    Ok(result) => JointMessage::Done(result),
                    Err(_) => JointMessage::Panicked,
                };
                let _ = tx.send(message);
            });
        }
        drop(tx);
        let mut completed = 0usize;
        while completed < worker_count {
            let Ok(message) = rx.recv() else {
                break;
            };
            match message {
                JointMessage::Live {
                    layout,
                    mut snapshot,
                    raw,
                    totals,
                    within_caps,
                } => {
                    merge_progress(
                        &mut snapshot.progress,
                        &mut restart_evaluations,
                        &mut restart_accepted,
                        started,
                        settings.seed,
                    );
                    status(&snapshot.progress.phase);
                    let merged_progress = snapshot.progress.clone();
                    let candidate = JointResult {
                        layout,
                        score: snapshot.best.score,
                        raw,
                        totals,
                        snapshot,
                    };
                    if within_caps
                        && design::rule_counts_satisfied(&candidate.layout)
                        && best
                            .as_ref()
                            .is_none_or(|current| better(&candidate, current))
                    {
                        best = Some(candidate);
                    }
                    if let Some(value) = &best {
                        let mut value = value.clone();
                        value.snapshot.progress = merged_progress;
                        emit(callback, &value, &baselines[0]);
                    }
                }
                JointMessage::Phase(mut progress) => {
                    status(&progress.phase);
                    merge_progress(
                        &mut progress,
                        &mut restart_evaluations,
                        &mut restart_accepted,
                        started,
                        settings.seed,
                    );
                    if let Some(value) = &best {
                        let mut shown = value.clone();
                        shown.snapshot.progress = progress;
                        emit(callback, &shown, &baselines[0]);
                    }
                }
                JointMessage::Exact(mut candidate) => {
                    merge_progress(
                        &mut candidate.snapshot.progress,
                        &mut restart_evaluations,
                        &mut restart_accepted,
                        started,
                        settings.seed,
                    );
                    if best
                        .as_ref()
                        .is_none_or(|current| better(&candidate, current))
                    {
                        best = Some(candidate.clone());
                    }
                    let value = best.as_ref().unwrap();
                    let mut shown = value.clone();
                    shown.snapshot.progress = candidate.snapshot.progress;
                    emit(callback, &shown, &baselines[0]);
                }
                JointMessage::Done(Ok(candidate)) => {
                    if let Some(mut candidate) = candidate {
                        merge_progress(
                            &mut candidate.snapshot.progress,
                            &mut restart_evaluations,
                            &mut restart_accepted,
                            started,
                            settings.seed,
                        );
                        if best
                            .as_ref()
                            .is_none_or(|current| better(&candidate, current))
                        {
                            best = Some(candidate);
                        }
                    }
                    completed += 1;
                }
                JointMessage::Done(Err(error)) => {
                    if error != "cancelled" {
                        if fatal_error.is_none() {
                            fatal_error = Some(error);
                        }
                        control.cancel.store(true, Ordering::Relaxed);
                    }
                    completed += 1;
                }
                JointMessage::Panicked => {
                    if fatal_error.is_none() {
                        fatal_error = Some("Magic optimizer worker panicked.".into());
                    }
                    control.cancel.store(true, Ordering::Relaxed);
                    completed += 1;
                }
            }
        }
    });

    if let Some(error) = fatal_error {
        return Err(error);
    }

    let best = best.ok_or_else(|| match &infeasible_reason {
        Some(reason) => infeasible(reason),
        None => "Magic optimizer finished without a feasible candidate.".into(),
    })?;
    let mut snapshot =
        identity_snapshot(best.snapshot.clone(), best.layout.slots.len(), best.score);
    snapshot.progress.restart = restart_evaluations
        .iter()
        .rposition(|&count| count != 0)
        .map_or(0, |index| index + 1);
    snapshot.progress.evaluations = restart_evaluations.iter().copied().sum();
    snapshot.progress.accepted = restart_accepted.iter().copied().sum();
    snapshot.progress.seed = settings.seed;
    snapshot.progress.phase = if control.cancel.load(Ordering::Relaxed) {
        "stopped".into()
    } else if expired(deadline) {
        "time limit".into()
    } else {
        "finished".into()
    };
    snapshot.progress.elapsed = started.elapsed().as_secs_f64();
    emit(
        callback,
        &JointResult {
            snapshot: snapshot.clone(),
            ..best.clone()
        },
        &baselines[0],
    );
    Ok((best.layout, snapshot))
}

/// Inject a dedicated magic action without discarding the selected binding.
/// Lock indices follow the original physical positions after column extension.
/// A locked selected binding also stays locked at its displacement destination.
pub(crate) fn inject(
    layout: &ak::Layout,
    slot: usize,
    locks: &[bool],
) -> ak::Result<(ak::Layout, usize, Vec<bool>)> {
    let selected = layout
        .slots
        .get(slot)
        .ok_or("Select a valid physical magic slot.")?;
    if matches!(selected.binding, ak::Binding::Named(_))
        && !(crate::layout_io::dedicated_magic_label(&selected.label)
            && design::eligible_roots(layout).contains(&slot))
    {
        return Err("Select an ordinary text, empty, or supported existing magic key.".into());
    }
    let (mut injected, seed_root) = design::both_outer_magic_seed(layout)?;
    let shift = i8::from(!layout.left_outer);
    let locate = |old: &ak::Slot| {
        let col = old
            .col
            .checked_add(shift)
            .ok_or("Physical column overflow.")?;
        injected
            .slots
            .iter()
            .position(|s| s.main == old.main && s.row == old.row && s.col == col)
            .ok_or_else(|| "An original physical slot was lost during magic injection.".to_string())
    };
    let root = locate(selected)?;
    for old in &layout.slots {
        if locate(old)? == seed_root && matches!(old.binding, ak::Binding::Text(_)) {
            return Err(
                "No empty key is available to seed magic without replacing an ordinary binding."
                    .into(),
            );
        }
    }
    let mut mapped_locks = vec![false; injected.slots.len()];
    for (index, old) in layout.slots.iter().enumerate() {
        mapped_locks[locate(old)?] = locks.get(index).copied().unwrap_or(false);
    }
    let selected_locked = locks.get(slot).copied().unwrap_or(false);
    if seed_root != root {
        if mapped_locks[seed_root] {
            return Err("The existing magic key is locked at another physical slot.".into());
        }
        // Find the displacement destination before touching either binding.
        let outer = injected
            .slots
            .iter()
            .filter(|s| s.main)
            .map(|s| s.col)
            .max()
            .ok_or("No physical rows.")?;
        let destination = injected
            .slots
            .iter()
            .enumerate()
            .filter(|(i, s)| {
                *i != root
                    && !mapped_locks[*i]
                    && s.main
                    && (s.col == 0 || s.col == outer)
                    && matches!(s.binding, ak::Binding::Empty)
            })
            .min_by_key(|(_, s)| (s.col != outer, s.row != 1, s.row != selected.row, s.row))
            .map(|(i, _)| i);
        let seed_is_outer = injected.slots[seed_root].main
            && (injected.slots[seed_root].col == 0 || injected.slots[seed_root].col == outer);
        if !seed_is_outer
            && !matches!(injected.slots[root].binding, ak::Binding::Empty)
            && destination.is_none()
        {
            return Err("No unlocked outer empty key can hold the displaced binding.".into());
        }
        injected.swap(seed_root, root);
        if !matches!(injected.slots[seed_root].binding, ak::Binding::Empty) {
            let displaced = if let Some(destination) = destination {
                injected.swap(seed_root, destination);
                destination
            } else {
                seed_root
            };
            mapped_locks[displaced] |= selected_locked;
        }
    }
    // Newly available outer keys are opt-in search positions. Keep the
    // displaced binding available, and lock the other empty pinky slots.
    let outer = injected
        .slots
        .iter()
        .filter(|slot| slot.main)
        .map(|slot| slot.col)
        .max();
    for (index, slot) in injected.slots.iter().enumerate() {
        if slot.main
            && (slot.col == 0 || Some(slot.col) == outer)
            && matches!(slot.binding, ak::Binding::Empty)
        {
            mapped_locks[index] = true;
        }
    }
    mapped_locks[root] = true;
    crate::layout_export::jsonc_text(&injected)?;
    Ok((injected, root, mapped_locks))
}

#[cfg(test)]
mod tests {
    #[test]
    fn combos_only_runs_fit_chordable_combos_without_a_magic_key() {
        let layout = ak::Layout::parse(
            "q w e r t  y u i o p\na s d f g  h j k l ;\nz x c v b  n m , . /\nthumbs: space\n",
            Path::new("combo-joint.dat"),
        )
        .unwrap();
        let tables = ak::text_ngrams(b"red bed fed led wed ted ed fur cue sea tea fun gun art ");
        let names = ["letters", "bigrams", "trigrams", "fourgrams", "fivegrams"];
        let fields: Vec<_> = names
            .iter()
            .zip(&tables)
            .map(|(name, table)| {
                let entries: Vec<_> = table
                    .iter()
                    .map(|(gram, count)| format!("{}:{count}", ak::quote(gram)))
                    .collect();
                format!("\"{name}\":{{{}}}", entries.join(","))
            })
            .collect();
        let corpus = ng::NgramCorpus::from_text(
            &format!("{{{}}}", fields.join(",")),
            Path::new("combo-joint"),
        )
        .unwrap();
        let corpora = vec![(corpus, 1.0)];
        let mut locks = vec![true; layout.slots.len()];
        for label in ["e", "d", "r", "f", "u", "c"] {
            locks[layout
                .slots
                .iter()
                .position(|slot| slot.label == label)
                .unwrap()] = false;
        }
        let settings = SearchSettings {
            workers: 2,
            restarts: 2,
            passes: 1,
            anneal_steps: 6,
            seconds: 0.0,
            sfb_limit: None,
            sfs_limit: None,
            travel_limit: None,
            sftravel_limit: None,
            ..SearchSettings::default()
        };
        let mut weights = Weights::default();
        weights.5 = true;
        let combos = crate::combo_fit::Settings {
            max_combos: 1,
            fatfingering: true,
            ..Default::default()
        };
        let (result, snapshot) = run_controlled(
            &layout,
            &layout,
            None,
            &locks,
            &corpora,
            &weights,
            &settings,
            &crate::MagicSettings::default(),
            &combos,
            &crate::swap_fit::Settings {
                max_swaps: 0,
                ..Default::default()
            },
            &Control::new(),
            &mut |_, _, _, _, _| {},
        )
        .unwrap();
        assert_eq!(result.combos.len(), 1);
        // Keys may move (e can leave d's finger entirely); whichever combo is
        // returned must still chord at the final positions.
        let Json::Object(fields) = &result.combos[0] else {
            panic!("combo object")
        };
        let Some(Json::String(output)) = fields.get("output") else {
            panic!("combo output")
        };
        let chords = crate::combos::Program::new(&result).unwrap();
        assert!(chords.forced(output.as_bytes()).iter().all(Option::is_some));
        let stop = AtomicBool::new(false);
        let progress = AtomicU64::new(0);
        let exact = summary::evaluate(&result, &corpora[0].0, &weights, &stop, &progress).unwrap();
        assert!((snapshot.best.score - exact.score).abs() < 1e-8);
        let start = summary::evaluate(&layout, &corpora[0].0, &weights, &stop, &progress).unwrap();
        assert!(exact.score < start.score);
    }

    #[test]
    fn existing_magic_keys_are_detected_for_in_place_refits() {
        let layout = |rules: &str, extra: &str, keys: &str| {
            ak::Layout::parse(
                &format!(
                    r#"{{"layout": {{"fingers": ["q w e r t y u i o p", "a s d f g h j k l *", "z x c v b n m , . {extra}"],
                        "thumbs": ["space", ""]}},
                      "fingermap": ["0 1 2 3 3 6 6 7 8 9", "0 1 2 3 3 6 6 7 8 9", "0 1 2 3 3 6 6 7 8 9"],
                      "magic": {{"keys": "{keys}", "wildcards": "%", "rules": [{rules}]}}}}"#
                ),
                Path::new("existing-magic.jsonc"),
            )
            .unwrap()
        };
        let single = layout(
            r#"{"inputs": "%*", "output": "%%"}, {"inputs": "e*", "output": "e."}"#,
            "/",
            "*",
        );
        let root = existing_root(&single).unwrap();
        assert_eq!(single.slots[root].label, "*");
        let ak::Binding::Named(name) = &single.slots[root].binding else {
            unreachable!()
        };
        assert_eq!(locate_root(&single, name).unwrap(), root);
        // Two magic keys are ambiguous; the optimizer cannot pick one to refit.
        let two = layout(
            r#"{"inputs": "%*", "output": "%%"}, {"inputs": "%#", "output": "%%"}"#,
            "#",
            "*#",
        );
        assert_eq!(existing_root(&two), None);
        let plain = ak::Layout::parse(
            "q w e r t y u i o p\na s d f g h j k l ;\nz x c v b n m , . /\n",
            Path::new("plain.dat"),
        )
        .unwrap();
        assert_eq!(existing_root(&plain), None);
        // Adaptive swap keys hold rules too, but the magic key stays the root,
        // and by default the swap keys move with their letters.
        let swapped = ak::Layout::parse(
            "q w e r t y u i o p\na s d f g h j k l *\nz x c v b n m , . /\n\
             * eo\nswap o ty\n",
            Path::new("swapped.dat"),
        )
        .unwrap();
        let root = existing_root(&swapped).unwrap();
        assert_eq!(swapped.slots[root].label, "*");
        let locks = crate::action_ui::action_home_locks(&swapped);
        for label in ["t", "y", "*"] {
            let slot = swapped
                .slots
                .iter()
                .position(|slot| slot.label == label)
                .unwrap();
            assert!(
                matches!(swapped.slots[slot].binding, ak::Binding::Named(_)),
                "{label}"
            );
            assert_eq!(locks[slot], label == "*", "{label}");
        }
    }

    use super::*;

    #[test]
    fn joint_search_keeps_rules_geometry_scores_and_cancelled_best_together() {
        let ordinary = ak::Layout::parse(
            "q w e r t  y u i o p\na s d f g  h j k l ;\nz x c v b  n m , . /\nthumbs: space\n",
            Path::new("inline-injection.dat"),
        )
        .unwrap();
        let (injected, injected_root, injected_locks) =
            inject(&ordinary, 3, &vec![false; ordinary.slots.len()]).unwrap();
        let right = injected
            .slots
            .iter()
            .filter(|slot| slot.main)
            .map(|slot| slot.col)
            .max()
            .unwrap();
        let displaced = injected
            .slots
            .iter()
            .position(|slot| slot.binding == ak::Binding::Text(vec![b'r']))
            .unwrap();
        assert_eq!(
            (injected.slots[displaced].col, injected.slots[displaced].row),
            (right, 1)
        );
        assert!(!injected_locks[displaced]);
        assert!(injected_locks[injected_root]);
        let empty_outer: Vec<_> = injected
            .slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| {
                slot.main
                    && (slot.col == 0 || slot.col == right)
                    && slot.binding == ak::Binding::Empty
            })
            .collect();
        assert_eq!(empty_outer.len(), 5);
        assert!(empty_outer.iter().all(|(index, _)| injected_locks[*index]));
        let mut layout = ak::Layout::parse(
            "q w e r t  y u i o p\na s d f g  h j k l ;\nz x c v b  @ m , . /\nthumbs: space\n@ hr\n",
            Path::new("inline-joint.dat"),
        ).unwrap();
        // Use the canonical action name produced by magic injection/JSONC.
        let action = layout.actions.remove("@").unwrap();
        layout.actions.insert("magic".into(), action);
        for slot in &mut layout.slots {
            if slot.binding == ak::Binding::Named("@".into()) {
                slot.binding = ak::Binding::Named("magic".into());
            }
        }
        let make_corpus = |text: &[u8]| {
            let tables = ak::text_ngrams(text);
            let fields: Vec<_> = tables
                .iter()
                .zip(["letters", "bigrams", "trigrams", "fourgrams", "fivegrams"])
                .map(|(table, name)| {
                    let entries: Vec<_> = table
                        .iter()
                        .map(|(gram, count)| format!("{}:{count}", ak::quote(gram)))
                        .collect();
                    format!("\"{name}\":{{{}}}", entries.join(","))
                })
                .collect();
            ng::NgramCorpus::from_text(
                &format!("{{{}}}", fields.join(",")),
                Path::new("inline-joint-corpus"),
            )
            .unwrap()
        };
        let corpora = vec![
            (make_corpus(b"qaq wqw hr hr qaq wqw"), 2.0),
            (make_corpus(b"hr wqw qaq hr"), 1.0),
        ];
        let root = layout
            .slots
            .iter()
            .position(|slot| matches!(slot.binding, ak::Binding::Named(_)))
            .unwrap();
        let action_name = match &layout.slots[root].binding {
            ak::Binding::Named(name) => name.clone(),
            _ => unreachable!(),
        };
        let mut locks = vec![true; layout.slots.len()];
        locks[0] = false;
        locks[1] = false;
        locks[root] = false;
        let mut moved = layout.clone();
        moved.swap(root, 0);
        assert_eq!(locate_root(&moved, &action_name).unwrap(), 0);
        let settings = SearchSettings {
            workers: 2,
            restarts: 2,
            passes: 1,
            anneal_steps: 6,
            seconds: 0.0,
            sfb_limit: None,
            sfs_limit: None,
            travel_limit: None,
            sftravel_limit: None,
            ..SearchSettings::default()
        };
        let weights = Weights::default();
        let stop = AtomicBool::new(false);
        let progress = AtomicU64::new(0);
        let control = Control::new();
        let mut previous_score = f64::INFINITY;
        let mut previous_trials = 0;
        let mut callbacks = 0;
        let mut check =
            |shown: ak::Layout, snapshot: Snapshot, _: Metrics, raw: Raw, totals: [f64; 4]| {
                callbacks += 1;
                assert!(snapshot.has_best);
                assert!(snapshot.best.score <= previous_score + 1e-8);
                assert!(snapshot.progress.evaluations >= previous_trials);
                previous_score = snapshot.best.score;
                previous_trials = snapshot.progress.evaluations;
                assert!(locate_root(&shown, &action_name).is_ok());
                for (index, locked) in locks.iter().enumerate() {
                    if *locked {
                        assert_eq!(shown.slots[index], layout.slots[index]);
                    }
                }
                let first =
                    summary::evaluate(&shown, &corpora[0].0, &weights, &stop, &progress).unwrap();
                let second =
                    summary::evaluate(&shown, &corpora[1].0, &weights, &stop, &progress).unwrap();
                let expected = (2.0 * first.score + second.score) / 3.0;
                assert!((snapshot.best.score - expected).abs() < 1e-8);
                assert_eq!(totals.map(f64::to_bits), first.totals.map(f64::to_bits));
                assert!(raw
                    .0
                    .iter()
                    .zip(first.raw.0)
                    .all(|(shown, exact)| (shown - exact).abs() < 1e-10));
            };
        let (result, snapshot) = run_controlled(
            &layout,
            &layout,
            Some(root),
            &locks,
            &corpora,
            &weights,
            &settings,
            &crate::MagicSettings::default(),
            &crate::combo_fit::Settings {
                max_combos: 0,
                ..Default::default()
            },
            &crate::swap_fit::Settings {
                max_swaps: 0,
                ..Default::default()
            },
            &control,
            &mut check,
        )
        .unwrap();
        assert!(callbacks > 2);
        assert!(snapshot.progress.evaluations >= 12);
        assert_eq!(
            snapshot.best.arr,
            (0..result.slots.len()).collect::<Vec<_>>()
        );
        assert_eq!(snapshot.progress.phase, "finished");
        let serialized = crate::layout_export::jsonc_text(&result).unwrap();
        let reloaded = ak::Layout::parse(&serialized, Path::new("inline-joint.jsonc")).unwrap();
        assert_eq!(reloaded.actions, result.actions);
        assert_eq!(reloaded.slots, result.slots);
        let cancelled = Control::new();
        let mut last = None;
        let result_root = locate_root(&result, &action_name).unwrap();
        let (kept, stopped) = run_controlled(
            &layout,
            &result,
            Some(result_root),
            &locks,
            &corpora,
            &weights,
            &settings,
            &crate::MagicSettings::default(),
            &crate::combo_fit::Settings {
                max_combos: 0,
                ..Default::default()
            },
            &crate::swap_fit::Settings {
                max_swaps: 0,
                ..Default::default()
            },
            &cancelled,
            &mut |shown, snapshot, _, _, _| {
                last = Some((shown, snapshot.best.score));
                cancelled.cancel.store(true, Ordering::Relaxed);
            },
        )
        .unwrap();
        let (shown, score) = last.unwrap();
        assert_eq!(kept.slots, shown.slots);
        assert_eq!(kept.actions, shown.actions);
        assert_eq!(stopped.best.score.to_bits(), score.to_bits());
        assert_eq!(stopped.progress.phase, "stopped");
        assert_eq!(chunk_sizes(20_003, true).iter().sum::<usize>(), 20_003);
    }

    #[test]
    fn joint_search_reports_exact_physical_mode_scores() {
        let mut layout = ak::Layout::parse(
            "q w e r t  y u i o p\na s d f g  h j k l ;\nz x c v b  @ m , . /\nthumbs: space\n@ hr\n",
            Path::new("inline-physical-joint.dat"),
        )
        .unwrap();
        let action = layout.actions.remove("@").unwrap();
        layout.actions.insert("magic".into(), action);
        for slot in &mut layout.slots {
            if slot.binding == ak::Binding::Named("@".into()) {
                slot.binding = ak::Binding::Named("magic".into());
            }
        }
        let tables = ak::text_ngrams(b"the cede ded un nu hr edu the deed red cede hr");
        let fields: Vec<_> = tables
            .iter()
            .zip(["letters", "bigrams", "trigrams", "fourgrams", "fivegrams"])
            .map(|(table, name)| {
                let entries: Vec<_> = table
                    .iter()
                    .map(|(gram, count)| format!("{}:{count}", ak::quote(gram)))
                    .collect();
                format!("\"{name}\":{{{}}}", entries.join(","))
            })
            .collect();
        let corpus = ng::NgramCorpus::from_text(
            &format!("{{{}}}", fields.join(",")),
            Path::new("inline-physical-joint-corpus"),
        )
        .unwrap();
        let corpora = vec![(corpus, 1.0)];
        let root = locate_root(&layout, "magic").unwrap();
        let mut locks = vec![true; layout.slots.len()];
        for slot in [0, 1, 2, 12] {
            locks[slot] = false;
        }
        let weights = Weights::default();
        let stop = AtomicBool::new(false);
        let progress = AtomicU64::new(0);
        for mode in ["aklgg"] {
            let settings = SearchSettings {
                workers: 2,
                restarts: 2,
                passes: 1,
                anneal_steps: 6,
                seconds: 0.0,
                mode: mode.into(),
                ..SearchSettings::default()
            };
            let mut callbacks = 0;
            let (result, snapshot) = run_controlled(
                &layout,
                &layout,
                Some(root),
                &locks,
                &corpora,
                &weights,
                &settings,
                &crate::MagicSettings::default(),
                &crate::combo_fit::Settings {
                    max_combos: 0,
                    ..Default::default()
                },
                &crate::swap_fit::Settings {
                    max_swaps: 0,
                    ..Default::default()
                },
                &Control::new(),
                &mut |shown, snapshot, _, _, _| {
                    callbacks += 1;
                    let exact = summary::evaluate_with_mode(
                        &shown,
                        &corpora[0].0,
                        &weights,
                        &stop,
                        &progress,
                        mode,
                    )
                    .unwrap();
                    assert!((snapshot.best.score - exact.score).abs() < 1e-8, "{mode}");
                },
            )
            .unwrap();
            assert!(callbacks > 1);
            let exact = summary::evaluate_with_mode(
                &result,
                &corpora[0].0,
                &weights,
                &stop,
                &progress,
                mode,
            )
            .unwrap();
            assert!((snapshot.best.score - exact.score).abs() < 1e-8);
            let normal =
                summary::evaluate(&result, &corpora[0].0, &weights, &stop, &progress).unwrap();
            assert_ne!(normal.score.to_bits(), exact.score.to_bits());
        }
    }
}
