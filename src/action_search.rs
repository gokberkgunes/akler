//! Search policy for action layouts. Candidate mapping remains incremental;
//! only compact permutations and numeric totals are retained as search states.
use crate::action_keys as ak;
use crate::action_ngrams as ng;
use crate::action_profile::{clock, elapsed, Profile};
use crate::action_ui::{physical_keys, LocalEffort};
use crate::*;

pub(crate) fn arrangement(seed: &ak::Layout, arr: &[usize]) -> ak::Layout {
    assert!(valid_arrangement(arr, seed.slots.len()));
    let mut layout = seed.clone();
    for (slot, &id) in layout.slots.iter_mut().zip(arr) {
        slot.binding.clone_from(&seed.slots[id].binding);
        slot.label.clone_from(&seed.slots[id].label);
    }
    layout
}

fn same_geometry(a: &ak::Layout, b: &ak::Layout) -> bool {
    a.slots.len() == b.slots.len()
        && a.left_outer == b.left_outer
        && a.right_outer == b.right_outer
        && a.slots.iter().zip(&b.slots).all(|(a, b)| {
            (
                a.row,
                a.col,
                a.row_offset,
                a.column_offset,
                a.finger,
                a.rank,
                a.hand,
                a.main,
            ) == (
                b.row,
                b.col,
                b.row_offset,
                b.column_offset,
                b.finger,
                b.rank,
                b.hand,
                b.main,
            )
        })
}

fn permutation(seed: &ak::Layout, layout: &ak::Layout) -> Option<Vec<usize>> {
    if !same_geometry(seed, layout) || seed.actions != layout.actions {
        return None;
    }
    let mut used = vec![false; seed.slots.len()];
    let mut arr = Vec::with_capacity(seed.slots.len());
    for slot in &layout.slots {
        let id = seed.slots.iter().enumerate().position(|(id, other)| {
            !used[id] && slot.binding == other.binding && slot.label == other.label
        })?;
        used[id] = true;
        arr.push(id);
    }
    Some(arr)
}

fn same_visible(seed: &ak::Layout, a: &[usize], b: &[usize]) -> bool {
    a.iter().zip(b).all(|(&a, &b)| {
        seed.slots[a].binding == seed.slots[b].binding && seed.slots[a].label == seed.slots[b].label
    })
}

fn is_letter(slot: &ak::Slot) -> bool {
    slot.label.len() == 1 && slot.label.as_bytes()[0].is_ascii_lowercase()
}

fn letter_distance(seed: &ak::Layout, a: &[usize], b: &[usize]) -> usize {
    // Binding identities stay stable when actions move. Labels identify the
    // physical letter keys, including adaptive letter bindings.
    let pa = positions(a);
    let pb = positions(b);
    seed.slots
        .iter()
        .enumerate()
        .filter(|(id, slot)| is_letter(slot) && pa[*id] != pb[*id])
        .count()
}

fn diverse(
    seed: &ak::Layout,
    pool: &[Candidate],
    limit: usize,
    distance: usize,
    generation: bool,
) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    for candidate in pool {
        if out.iter().any(|other| {
            if generation {
                letter_distance(seed, &other.arr, &candidate.arr) < distance.max(1)
            } else {
                same_visible(seed, &other.arr, &candidate.arr)
            }
        }) {
            continue;
        }
        out.push(candidate.clone());
        if out.len() == limit {
            break;
        }
    }
    out
}

fn objective(metrics: &Metrics, weights: &Weights, settings: &SearchSettings) -> f64 {
    if settings.mode == "simple" {
        simple_breakdown(metrics, &settings.simple).net
    } else {
        breakdown(metrics, weights).net
    }
}

fn violation(metrics: &Metrics, baseline: &Metrics, settings: &SearchSettings) -> f64 {
    let mut amount = 0.0;
    for (metric, limit) in [
        (SFB, settings.sfb_limit),
        (SFS, settings.sfs_limit),
        (TRAVEL, settings.travel_limit),
        (SFTRAVEL, settings.sftravel_limit),
    ] {
        if let Some(increase) = limit {
            let cap = baseline.v[metric] + increase;
            let excess = metrics.v[metric] - cap;
            if excess > 1e-10 {
                amount += (excess / cap.abs().max(1.0)).powi(2);
            }
        }
    }
    amount
}

struct Problem<'a> {
    seed: &'a ak::Layout,
    weights: &'a Weights,
    settings: &'a SearchSettings,
    shares: Vec<f64>,
    baseline: Vec<Metrics>,
    effort: LocalEffort,
    caches: Vec<ng::Incremental>,
    proposals: Vec<ng::Proposal>,
}

#[derive(Clone)]
struct State {
    arr: Vec<usize>,
    numeric: Vec<ng::NumericState>,
    score: f64,
    violation: f64,
}

impl Problem<'_> {
    fn current(&self, arr: Vec<usize>) -> State {
        let mut score = 0.0;
        let mut excess = 0.0;
        for (i, cache) in self.caches.iter().enumerate() {
            let metrics = cache.metrics();
            score += self.shares[i] * objective(&metrics, self.weights, self.settings);
            if self.shares[i] != 0.0 {
                excess += violation(&metrics, &self.baseline[i], self.settings);
            }
        }
        State {
            arr,
            numeric: self
                .caches
                .iter()
                .map(ng::Incremental::numeric_state)
                .collect(),
            score,
            violation: excess,
        }
    }

    fn restore(&mut self, state: &State, control: &Control) -> ak::Result<()> {
        for (cache, numeric) in self.caches.iter_mut().zip(&state.numeric) {
            cache.restore(&state.arr, Some(numeric), &control.cancel, |a, b, key| {
                self.effort.get(a, b, key)
            })?;
        }
        Ok(())
    }

    fn fresh(&mut self, arr: Vec<usize>, control: &Control) -> ak::Result<State> {
        for cache in &mut self.caches {
            cache.restore(&arr, None, &control.cancel, |a, b, key| {
                self.effort.get(a, b, key)
            })?;
        }
        Ok(self.current(arr))
    }

    fn trial<const PROFILE: bool, const DETAIL: bool>(
        &mut self,
        movement: Move,
        control: &Control,
        profile: &mut Profile,
    ) -> ak::Result<(f64, f64)> {
        let previous_max = profile.affected_max;
        let previous_scans = profile.full_scans;
        for (cache, proposal) in self.caches.iter_mut().zip(&mut self.proposals) {
            cache.propose_move_profiled::<PROFILE, DETAIL, _>(
                movement,
                &control.cancel,
                |a, b, key| self.effort.get(a, b, key),
                proposal,
                profile,
            )?;
        }

        if PROFILE {
            let affected: u64 = self
                .proposals
                .iter()
                .map(|proposal| proposal.examined as u64)
                .sum();
            profile.affected_max = previous_max.max(affected);
            profile.full_scans = previous_scans
                + u64::from(self.proposals.iter().any(|proposal| proposal.all_contexts));
        }

        let mut score = 0.0;
        let mut excess = 0.0;
        for (i, proposal) in self.proposals.iter().enumerate() {
            let score_start = clock::<PROFILE>();
            let metrics = proposal.metrics();
            score += self.shares[i] * objective(&metrics, self.weights, self.settings);
            if PROFILE {
                profile.score += elapsed::<PROFILE>(score_start);
            }

            let check_start = clock::<PROFILE>();
            if self.shares[i] != 0.0 {
                excess += violation(&metrics, &self.baseline[i], self.settings);
            }
            if PROFILE {
                profile.checks += elapsed::<PROFILE>(check_start);
            }
        }
        if PROFILE {
            profile.candidates += 1;
        }
        Ok((score, excess))
    }

    fn commit<const PROFILE: bool>(
        &mut self,
        state: &mut State,
        movement: Move,
        profile: &mut Profile,
    ) {
        for (cache, proposal) in self.caches.iter_mut().zip(&mut self.proposals) {
            cache.commit_profiled::<PROFILE>(proposal, profile);
        }
        state.arr.swap(movement.slots[0], movement.slots[1]);
        if movement.len == 3 {
            state.arr.swap(movement.slots[1], movement.slots[2]);
        }
        // Commit may perform the existing 128-commit rebase. Read the resulting
        // score/totals so search snapshots match the committed cache exactly.
        let next = self.current(state.arr.clone());
        *state = next;
    }
}

struct Runtime<'a> {
    started: Instant,
    last_frame: Instant,
    progress: Progress,
    control: &'a Control,
    callback: &'a mut dyn FnMut(Snapshot, Metrics, Raw, [f64; 4]),
    archive: Vec<Candidate>,
    pool: Vec<Candidate>,
    best: State,
    baseline: Metrics,
    has_best: bool,
    known: Option<Vec<Vec<usize>>>,
    template: Vec<usize>,
}

impl Runtime<'_> {
    fn stop(&self, settings: &SearchSettings) -> bool {
        self.control.cancel.load(Ordering::Relaxed)
            || settings.seconds > 0.0 && self.started.elapsed().as_secs_f64() >= settings.seconds
    }

    fn checkpoint(&self, settings: &SearchSettings) -> bool {
        while self.control.pause.load(Ordering::Relaxed) && !self.stop(settings) {
            thread::sleep(Duration::from_millis(15));
        }
        self.stop(settings)
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            best: Candidate {
                arr: self.best.arr.clone(),
                score: self.best.score,
            },
            progress: self.progress.clone(),
            archive: self.archive.clone(),
            has_best: self.has_best,
        }
    }

    fn frame(&mut self, force: bool) {
        if force || self.last_frame.elapsed() >= Duration::from_millis(100) {
            self.progress.elapsed = self.started.elapsed().as_secs_f64();
            self.last_frame = Instant::now();
            let snapshot = self.snapshot();
            let baseline = self.baseline.clone();
            let (raw, totals) = self.best.numeric[0].raw_totals();
            (self.callback)(snapshot, baseline, raw.clone(), totals);
        }
    }

    fn keep(&mut self, state: &State, problem: &Problem<'_>) {
        if !state.score.is_finite() || state.violation != 0.0 {
            return;
        }
        if let Some(known) = &self.known {
            if letter_distance(problem.seed, &self.template, &state.arr)
                < problem.settings.diversity
                || known
                    .iter()
                    .any(|arr| same_visible(problem.seed, arr, &state.arr))
            {
                return;
            }
        }
        if !self.has_best || state.score < self.best.score - 1e-10 {
            self.best.clone_from(state);
            self.has_best = true;
        }
        if self
            .pool
            .iter()
            .any(|candidate| same_visible(problem.seed, &candidate.arr, &state.arr))
        {
            return;
        }
        self.pool.push(Candidate {
            arr: state.arr.clone(),
            score: state.score,
        });
        self.pool
            .sort_by(|a, b| a.score.total_cmp(&b.score).then_with(|| a.arr.cmp(&b.arr)));
        self.pool
            .truncate(256usize.max(problem.settings.archive * 16));
        self.archive = diverse(
            problem.seed,
            &self.pool,
            problem.settings.archive,
            problem.settings.diversity,
            self.known.is_some(),
        );
    }

    fn accept(&mut self, state: &State, problem: &Problem<'_>) {
        self.progress.accepted += 1;
        if !self.has_best || state.score < self.best.score - 1e-10 {
            self.keep(state, problem);
        }
    }
}

// This helper owns the profiling boundary, so mixtures still count one move as
// one candidate. Context operation counts include every corpus evaluated.
fn trial<const PROFILE: bool, const DETAIL: bool>(
    problem: &mut Problem<'_>,
    state: &State,
    movement: Move,
    runtime: &mut Runtime<'_>,
    profile: &mut Profile,
) -> ak::Result<(f64, f64)> {
    let start = clock::<PROFILE>();
    let mut named = false;
    if PROFILE {
        profile.attempted += 1;
        named = movement.slots[..movement.len].iter().any(|&slot| {
            matches!(
                problem.seed.slots[state.arr[slot]].binding,
                ak::Binding::Named(_)
            )
        });
        if named {
            profile.named += 1;
        } else if movement.slots[..movement.len].iter().all(|&slot| {
            matches!(
                problem.seed.slots[state.arr[slot]].binding,
                ak::Binding::Text(_)
            )
        }) {
            profile.literals += 1;
        } else {
            profile.other_candidates += 1;
        }
    }
    let result = problem.trial::<PROFILE, DETAIL>(movement, runtime.control, profile);
    if PROFILE && named {
        profile.named_time += elapsed::<PROFILE>(start);
    }
    if result.is_ok() {
        runtime.progress.evaluations += 1;
    }
    result
}

fn sweep<const PROFILE: bool, const DETAIL: bool>(
    problem: &mut Problem<'_>,
    current: &mut State,
    free: &[usize],
    runtime: &mut Runtime<'_>,
    profile: &mut Profile,
) -> ak::Result<()> {
    let mut good = Vec::with_capacity(free.len() * free.len() / 2);
    for pass in 0..problem.settings.passes {
        if runtime.checkpoint(problem.settings) {
            break;
        }
        runtime.progress.pass = pass + 1;
        runtime.progress.phase = "sweep".into();
        good.clear();
        for i in 0..free.len() {
            for j in i + 1..free.len() {
                if runtime.checkpoint(problem.settings) {
                    return Ok(());
                }
                let movement = Move::pair(free[i], free[j]);
                let (score, excess) =
                    trial::<PROFILE, DETAIL>(problem, current, movement, runtime, profile)?;
                if score < current.score - 1e-10 && excess == 0.0 {
                    good.push((score - current.score, movement));
                }
                // This first pass ranks proposals; it never commits them.
                if PROFILE {
                    profile.rejected += 1;
                }
                runtime.frame(false);
            }
        }
        if good.is_empty() {
            break;
        }
        good.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut changed = false;
        for (_, movement) in &good {
            if runtime.checkpoint(problem.settings) {
                return Ok(());
            }
            let (score, excess) =
                trial::<PROFILE, DETAIL>(problem, current, *movement, runtime, profile)?;
            if score < current.score - 1e-10 && excess == 0.0 {
                problem.commit::<PROFILE>(current, *movement, profile);
                if PROFILE {
                    profile.accepted += 1;
                }
                runtime.accept(current, problem);
                changed = true;
            } else if PROFILE {
                profile.rejected += 1;
            }
        }
        runtime.frame(false);
        if !changed {
            break;
        }
    }
    Ok(())
}

fn repair<const PROFILE: bool, const DETAIL: bool>(
    problem: &mut Problem<'_>,
    current: &mut State,
    free: &[usize],
    runtime: &mut Runtime<'_>,
    profile: &mut Profile,
) -> ak::Result<bool> {
    if current.violation == 0.0 {
        return Ok(true);
    }
    runtime.progress.phase = "repair".into();
    for _ in 0..problem.settings.passes.min(32) {
        let mut best = current.violation;
        let mut chosen = None;
        for i in 0..free.len() {
            for j in i + 1..free.len() {
                if runtime.checkpoint(problem.settings) {
                    return Ok(false);
                }
                let movement = Move::pair(free[i], free[j]);
                let (_, excess) =
                    trial::<PROFILE, DETAIL>(problem, current, movement, runtime, profile)?;
                if excess < best {
                    best = excess;
                    chosen = Some(movement);
                }
                if PROFILE {
                    profile.rejected += 1;
                }
                runtime.frame(false);
            }
        }
        let Some(movement) = chosen else {
            break;
        };
        trial::<PROFILE, DETAIL>(problem, current, movement, runtime, profile)?;
        problem.commit::<PROFILE>(current, movement, profile);
        if PROFILE {
            profile.accepted += 1;
        }
        runtime.progress.accepted += 1;
        if current.violation == 0.0 {
            return Ok(true);
        }
    }
    Ok(false)
}

fn search<const PROFILE: bool, const DETAIL: bool>(
    problem: &mut Problem<'_>,
    initial: State,
    locked: &[bool],
    control: &Control,
    callback: &mut dyn FnMut(Snapshot, Metrics, Raw, [f64; 4]),
    profile: &mut Profile,
    known: Vec<Vec<usize>>,
) -> ak::Result<Snapshot> {
    let generation = problem.settings.design != "refine";
    if !generation && initial.violation != 0.0 {
        return Err("starting arrangement violates active limits".into());
    }
    let free: Vec<_> = (0..locked.len())
        .filter(|&slot| !locked[slot] && !problem.seed.space(slot))
        .collect();
    let started = Instant::now();
    let mut runtime = Runtime {
        started,
        last_frame: started,
        progress: Progress {
            seed: problem.settings.seed,
            ..Progress::default()
        },
        control,
        callback,
        archive: Vec::new(),
        pool: Vec::new(),
        best: initial.clone(),
        baseline: problem.baseline[0].clone(),
        has_best: false,
        known: generation.then_some(known),
        template: initial.arr.clone(),
    };
    if !generation {
        runtime.keep(&initial, problem);
    }
    let letter_count = free
        .iter()
        .filter(|&&slot| is_letter(&problem.seed.slots[slot]))
        .count();
    let can_search = free.len() >= 2 && (!generation || problem.settings.diversity <= letter_count);
    let mut rng = Rng::new(problem.settings.seed);
    let mut current = initial.clone();

    let outcome = (|| -> ak::Result<()> {
        for restart in 0..problem.settings.restarts {
            if !can_search || runtime.checkpoint(problem.settings) {
                break;
            }
            runtime.progress.restart = restart + 1;
            runtime.progress.pass = 0;
            if generation {
                let parents = diverse(
                    problem.seed,
                    &runtime.pool,
                    64,
                    problem.settings.diversity.max(2),
                    true,
                );
                let mut arr = initial.arr.clone();
                if problem.settings.design == "evolve" && parents.len() >= 2 && rng.unit() >= 0.30 {
                    let x = rng.index(parents.len());
                    let mut y = rng.index(parents.len() - 1);
                    if y >= x {
                        y += 1;
                    }
                    arr = crossover(&parents[x].arr, &parents[y].arr, &free, &mut rng);
                    let kicks = 2 + rng.index((free.len() / 5).max(1));
                    for _ in 0..kicks {
                        let movement = rng.movement(&free, 0.20);
                        arr.swap(movement.slots[0], movement.slots[1]);
                        if movement.len == 3 {
                            arr.swap(movement.slots[1], movement.slots[2]);
                        }
                    }
                } else {
                    shuffle(&mut arr, &free, &mut rng);
                }
                current = problem.fresh(arr, control)?;
                if !repair::<PROFILE, DETAIL>(problem, &mut current, &free, &mut runtime, profile)?
                {
                    continue;
                }
            } else {
                current.clone_from(if restart == 0 {
                    &initial
                } else {
                    &runtime.best
                });
                if restart > 0 {
                    problem.restore(&current, control)?;
                    for _ in 0..48 {
                        if runtime.checkpoint(problem.settings) {
                            break;
                        }
                        let movement = rng.movement(&free, 0.25);
                        let (_, excess) = trial::<PROFILE, DETAIL>(
                            problem,
                            &current,
                            movement,
                            &mut runtime,
                            profile,
                        )?;
                        if excess == 0.0 {
                            problem.commit::<PROFILE>(&mut current, movement, profile);
                            if PROFILE {
                                profile.accepted += 1;
                            }
                        } else if PROFILE {
                            profile.rejected += 1;
                        }
                    }
                }
            }

            if problem.settings.hybrid {
                runtime.progress.phase = "anneal".into();
                let mut local = current.clone();
                for step in 0..problem.settings.anneal_steps {
                    if runtime.checkpoint(problem.settings) {
                        break;
                    }
                    let fraction =
                        step as f64 / problem.settings.anneal_steps.saturating_sub(1).max(1) as f64;
                    let temperature = problem.settings.temperature
                        * (problem.settings.cooling_end / problem.settings.temperature)
                            .powf(fraction);
                    let movement = rng.movement(&free, problem.settings.cycle_probability);
                    let (score, excess) = trial::<PROFILE, DETAIL>(
                        problem,
                        &current,
                        movement,
                        &mut runtime,
                        profile,
                    )?;
                    let delta = score - current.score;
                    if excess == 0.0 && (delta < 0.0 || rng.unit() < (-delta / temperature).exp()) {
                        problem.commit::<PROFILE>(&mut current, movement, profile);
                        if PROFILE {
                            profile.accepted += 1;
                        }
                        runtime.accept(&current, problem);
                        if current.score < local.score {
                            local.clone_from(&current);
                        }
                    } else if PROFILE {
                        profile.rejected += 1;
                    }
                    runtime.frame(false);
                }
                current = local;
                if runtime.stop(problem.settings) {
                    runtime.keep(&current, problem);
                    runtime.frame(true);
                    break;
                }
                problem.restore(&current, control)?;
            }
            sweep::<PROFILE, DETAIL>(problem, &mut current, &free, &mut runtime, profile)?;
            runtime.keep(&current, problem);
            runtime.frame(true);
        }
        Ok(())
    })();
    if let Err(error) = outcome {
        if !control.cancel.load(Ordering::Relaxed) {
            return Err(error);
        }
    }

    for candidate in &runtime.archive {
        if !valid_arrangement(&candidate.arr, locked.len()) {
            return Err("generated action permutation is invalid".into());
        }
        for (slot, &locked) in locked.iter().enumerate() {
            if (locked || problem.seed.space(slot)) && candidate.arr[slot] != initial.arr[slot] {
                return Err("locked action-layout key moved".into());
            }
        }
    }
    runtime.progress.phase = if control.cancel.load(Ordering::Relaxed) {
        "stopped"
    } else if problem.settings.seconds > 0.0
        && started.elapsed().as_secs_f64() >= problem.settings.seconds
    {
        "time limit"
    } else {
        "finished"
    }
    .into();
    runtime.progress.elapsed = started.elapsed().as_secs_f64();
    Ok(runtime.snapshot())
}

fn run_profiled<const PROFILE: bool, const DETAIL: bool>(
    original: &ak::Layout,
    seed: &ak::Layout,
    corpora: &[(ng::NgramCorpus, f64)],
    weights: &Weights,
    settings: &SearchSettings,
    locked: &[bool],
    control: &Control,
    callback: &mut dyn FnMut(Snapshot, Metrics, Raw, [f64; 4]),
    profile: &mut Profile,
) -> ak::Result<Snapshot> {
    validate_search_settings(settings).map_err(|error| error.to_string())?;
    if locked.len() != seed.slots.len() || corpora.is_empty() {
        return Err("invalid action search lock mask or empty training corpus list".into());
    }
    let baseline_arr = permutation(seed, original)
        .ok_or("baseline layout is not a permutation of the search layout")?;
    let shares: Vec<_> = corpora.iter().map(|(_, share)| *share).collect();
    let total: f64 = shares.iter().sum();
    if shares
        .iter()
        .any(|share| !share.is_finite() || *share < 0.0)
        || !total.is_finite()
        || total <= 0.0
    {
        return Err("training shares must have a finite positive sum".into());
    }
    let shares: Vec<_> = shares.into_iter().map(|share| share / total).collect();
    let start = clock::<PROFILE>();
    let effort = LocalEffort::new(seed, weights);
    let geometry = Geometry::with_rolls(physical_keys(seed), weights.rolls());
    let identity: Vec<_> = (0..seed.slots.len()).collect();
    let mut caches = Vec::with_capacity(corpora.len());
    let mut baseline = Vec::with_capacity(corpora.len());
    for (corpus, _) in corpora {
        let mut cache = ng::Incremental::new(
            corpus,
            seed,
            geometry.clone(),
            &control.cancel,
            |a, b, key| effort.get(a, b, key),
        )?;
        if baseline_arr != identity {
            let saved = cache.numeric_state();
            cache.restore(&baseline_arr, None, &control.cancel, |a, b, key| {
                effort.get(a, b, key)
            })?;
            baseline.push(cache.metrics());
            cache.restore(&identity, Some(&saved), &control.cancel, |a, b, key| {
                effort.get(a, b, key)
            })?;
        } else {
            baseline.push(cache.metrics());
        }
        caches.push(cache);
    }
    if PROFILE {
        profile.setup += elapsed::<PROFILE>(start);
        profile.cache_size = caches
            .iter()
            .map(|cache| cache.context_count() as u64)
            .sum();
    }
    let mut problem = Problem {
        seed,
        weights,
        settings,
        shares,
        baseline,
        effort,
        proposals: (0..caches.len()).map(|_| ng::Proposal::new()).collect(),
        caches,
    };
    let initial = problem.current(identity);
    let generation = settings.design != "refine";
    let mut known = vec![initial.arr.clone()];
    if generation {
        if let Ok(paths) = discover_layouts(Path::new(LAYOUT_DIR)) {
            for path in paths {
                if let Ok(text) = fs::read_to_string(&path) {
                    if let Ok(layout) = ak::Layout::parse(&text, &path) {
                        if let Some(arr) = permutation(problem.seed, &layout) {
                            known.push(arr);
                        }
                    }
                }
            }
        }
    }
    search::<PROFILE, DETAIL>(
        &mut problem,
        initial,
        locked,
        control,
        callback,
        profile,
        known,
    )
}

pub(crate) fn run(
    original: &ak::Layout,
    seed: &ak::Layout,
    corpora: &[(ng::NgramCorpus, f64)],
    weights: &Weights,
    settings: &SearchSettings,
    locked: &[bool],
    control: &Control,
    callback: &mut dyn FnMut(Snapshot, Metrics, Raw, [f64; 4]),
) -> ak::Result<Snapshot> {
    let level = match std::env::var("AKLER_PROFILE")
        .or_else(|_| std::env::var("LAYOUTER_PROFILE"))
        .as_deref() {
        Ok("1") => 1,
        Ok("2") => 2,
        _ => 0,
    };
    let mut profile = Profile::default();
    let started = (level != 0).then(Instant::now);
    let result = match level {
        1 => run_profiled::<true, false>(
            original,
            seed,
            corpora,
            weights,
            settings,
            locked,
            control,
            callback,
            &mut profile,
        ),
        2 => run_profiled::<true, true>(
            original,
            seed,
            corpora,
            weights,
            settings,
            locked,
            control,
            callback,
            &mut profile,
        ),
        _ => run_profiled::<false, false>(
            original,
            seed,
            corpora,
            weights,
            settings,
            locked,
            control,
            callback,
            &mut profile,
        ),
    };
    if let Some(started) = started {
        profile.total = started.elapsed();
        let status = if control.cancel.load(Ordering::Relaxed) {
            "cancelled/partial"
        } else if result.is_err() {
            "error/partial"
        } else {
            "completed"
        };
        let mut report = Vec::new();
        let _ = profile.report(&mut report, status, level == 2);
        let _ = writeln!(report, "Mixtures: context counters sum all corpora; candidates count search moves. Ranked sweep probes count as rejected until re-evaluated and committed. Restart kicks count as commits; UI accepted counts follow the ordinary search convention. Restoring a restart/local optimum remaps tails outside candidate counters (other time). Cap timing is separate; anneal/improvement decisions and archive work are other time.");
        crate::profile_output::write_report(&report);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> ak::Layout {
        ak::Layout::parse(
            "q w e r t  y u i o p\na s d f g  h j k l ;\nz x c v b  @ m , . /\nthumbs: space\n@ hr\n",
            Path::new("inline-action-search.dat"),
        ).unwrap()
    }

    fn corpus(name: &str, text: &[u8]) -> ng::NgramCorpus {
        let tables = ak::text_ngrams(text);
        let names = ["letters", "bigrams", "trigrams", "fourgrams", "fivegrams"];
        let fields: Vec<_> = tables
            .iter()
            .zip(names)
            .map(|(table, name)| {
                let entries: Vec<_> = table
                    .iter()
                    .map(|(gram, count)| format!("{}:{count}", ak::quote(gram)))
                    .collect();
                format!("\"{name}\":{{{}}}", entries.join(","))
            })
            .collect();
        ng::NgramCorpus::from_text(&format!("{{{}}}", fields.join(",")), Path::new(name)).unwrap()
    }

    fn settings() -> SearchSettings {
        SearchSettings {
            restarts: 2,
            passes: 1,
            anneal_steps: 12,
            seconds: 0.0,
            cycle_probability: 1.0,
            sfb_limit: None,
            sfs_limit: None,
            travel_limit: None,
            sftravel_limit: None,
            archive: 4,
            diversity: 0,
            ..SearchSettings::default()
        }
    }

    fn locks(seed: &ak::Layout) -> Vec<bool> {
        seed.slots
            .iter()
            .map(|slot| !["a", "q", "@", "r"].contains(&slot.label.as_str()))
            .collect()
    }

    fn quiet(_: Snapshot, _: Metrics, _: Raw, _: [f64; 4]) {}

    fn assert_same(a: &Snapshot, b: &Snapshot) {
        assert_eq!(a.has_best, b.has_best);
        assert_eq!(a.best.arr, b.best.arr);
        assert_eq!(a.best.score.to_bits(), b.best.score.to_bits());
        assert_eq!(a.progress.evaluations, b.progress.evaluations);
        assert_eq!(a.progress.accepted, b.progress.accepted);
        assert_eq!(a.archive.len(), b.archive.len());
        for (a, b) in a.archive.iter().zip(&b.archive) {
            assert_eq!(a.arr, b.arr);
            assert_eq!(a.score.to_bits(), b.score.to_bits());
        }
    }

    #[test]
    fn hybrid_sweep_seed_profile_and_locks_are_deterministic() {
        let seed = layout();
        let weights = Weights::default();
        let corpora = vec![(corpus("one.json", b"aa aq hr rh aqr qra! aa hr"), 1.0)];
        let locked = locks(&seed);
        for hybrid in [false, true] {
            let settings = SearchSettings {
                hybrid,
                ..settings()
            };
            let mut off = Profile::default();
            let a = run_profiled::<false, false>(
                &seed,
                &seed,
                &corpora,
                &weights,
                &settings,
                &locked,
                &Control::new(),
                &mut quiet,
                &mut off,
            )
            .unwrap();
            let mut profiled = Profile::default();
            let b = run_profiled::<true, true>(
                &seed,
                &seed,
                &corpora,
                &weights,
                &settings,
                &locked,
                &Control::new(),
                &mut quiet,
                &mut profiled,
            )
            .unwrap();
            assert_same(&a, &b);
            assert!(a.progress.evaluations > 0);
            assert_eq!(profiled.candidates, a.progress.evaluations);
            assert_eq!(profiled.candidates, profiled.accepted + profiled.rejected);
            assert_eq!(off.candidates, 0);
            for candidate in &a.archive {
                assert!(valid_arrangement(&candidate.arr, seed.slots.len()));
                for (slot, &locked) in locked.iter().enumerate() {
                    if locked || seed.space(slot) {
                        assert_eq!(candidate.arr[slot], slot);
                    }
                }
            }
        }
    }

    #[test]
    fn simple_objective_and_normalized_mixture_use_numeric_totals() {
        let seed = layout();
        let weights = Weights::default();
        let corpora = vec![
            (corpus("first.json", b"aa aq hr rh aqr qra"), 1.0),
            (corpus("second.json", b"hhh rrr ar ar qr"), 3.0),
        ];
        let locked = vec![true; seed.slots.len()];
        for mode in ["detailed", "simple"] {
            let settings = SearchSettings {
                mode: mode.into(),
                ..settings()
            };
            let effort = LocalEffort::new(&seed, &weights);
            let expected: f64 = corpora
                .iter()
                .map(|(corpus, share)| {
                    let cache = ng::Incremental::new(
                        corpus,
                        &seed,
                        Geometry::new(physical_keys(&seed)),
                        &AtomicBool::new(false),
                        |a, b, key| effort.get(a, b, key),
                    )
                    .unwrap();
                    (share / 4.0) * objective(&cache.metrics(), &weights, &settings)
                })
                .sum();
            let result = run_profiled::<false, false>(
                &seed,
                &seed,
                &corpora,
                &weights,
                &settings,
                &locked,
                &Control::new(),
                &mut quiet,
                &mut Profile::default(),
            )
            .unwrap();
            assert_eq!(result.best.score.to_bits(), expected.to_bits());
            assert_eq!(result.progress.evaluations, 0);
        }
    }

    #[test]
    fn all_four_caps_use_original_per_corpus_and_skip_zero_share() {
        let mut baseline = metrics_totals(&Raw::default(), &[1.0; 4]);
        for (metric, limit_field) in [(SFB, 0), (SFS, 1), (TRAVEL, 2), (SFTRAVEL, 3)] {
            baseline.v[metric] = 1.0;
            let mut current = baseline.clone();
            current.v[metric] = 2.0;
            let mut settings = settings();
            match limit_field {
                0 => settings.sfb_limit = Some(0.0),
                1 => settings.sfs_limit = Some(0.0),
                2 => settings.travel_limit = Some(0.0),
                _ => settings.sftravel_limit = Some(0.0),
            }
            assert_eq!(violation(&baseline, &baseline, &settings), 0.0);
            assert!(violation(&current, &baseline, &settings) > 0.0);
            current.v[metric] = 1.0 + 0.5e-10;
            assert_eq!(violation(&current, &baseline, &settings), 0.0);
        }

        let seed = layout();
        let weights = Weights::default();
        let selected = corpus("selected.json", b"aa aq hr rh aqr qra");
        let training = corpus("training.json", b"hhh rrr ar ar qr");
        let locked = vec![true; seed.slots.len()];
        let settings = settings();
        let with_zero = run_profiled::<false, false>(
            &seed,
            &seed,
            &[(selected, 0.0), (training.clone(), 1.0)],
            &weights,
            &settings,
            &locked,
            &Control::new(),
            &mut quiet,
            &mut Profile::default(),
        )
        .unwrap();
        let only_training = run_profiled::<false, false>(
            &seed,
            &seed,
            &[(training, 1.0)],
            &weights,
            &settings,
            &locked,
            &Control::new(),
            &mut quiet,
            &mut Profile::default(),
        )
        .unwrap();
        assert_eq!(
            with_zero.best.score.to_bits(),
            only_training.best.score.to_bits()
        );
    }

    #[test]
    fn cancellation_time_limit_and_no_free_slots_finish_cleanly() {
        let seed = layout();
        let weights = Weights::default();
        let corpora = vec![(corpus("one.json", b"aa aq hr rh aqr qra"), 1.0)];
        let control = Control::new();
        control.cancel.store(true, Ordering::Relaxed);
        let error = run_profiled::<false, false>(
            &seed,
            &seed,
            &corpora,
            &weights,
            &settings(),
            &locks(&seed),
            &control,
            &mut quiet,
            &mut Profile::default(),
        )
        .err()
        .unwrap();
        assert_eq!(error, "cancelled");

        let tiny_budget = SearchSettings {
            seconds: f64::MIN_POSITIVE,
            ..settings()
        };
        let paused = Control::new();
        paused.pause.store(true, Ordering::Relaxed);
        let result = run_profiled::<false, false>(
            &seed,
            &seed,
            &corpora,
            &weights,
            &tiny_budget,
            &locks(&seed),
            &paused,
            &mut quiet,
            &mut Profile::default(),
        )
        .unwrap();
        assert_eq!(result.progress.phase, "time limit");
        assert_eq!(result.progress.evaluations, 0);
        assert!(result.has_best);

        let result = run_profiled::<false, false>(
            &seed,
            &seed,
            &corpora,
            &weights,
            &settings(),
            &vec![true; seed.slots.len()],
            &Control::new(),
            &mut quiet,
            &mut Profile::default(),
        )
        .unwrap();
        assert_eq!(result.progress.phase, "finished");
        assert_eq!(result.progress.evaluations, 0);
    }

    #[test]
    fn cancellation_after_a_frame_keeps_the_valid_best_and_numeric_display() {
        let seed = layout();
        let weights = Weights::default();
        let corpora = vec![(corpus("one.json", b"aa aq hr rh aqr qra"), 1.0)];
        let control = Control::new();
        let settings = settings();
        let mut frames = 0;
        let mut callback = |snapshot: Snapshot, baseline: Metrics, raw: Raw, totals: [f64; 4]| {
            frames += 1;
            assert!(snapshot.has_best);
            let metrics = metrics_totals(&raw, &totals);
            assert_eq!(
                snapshot.best.score.to_bits(),
                objective(&metrics, &weights, &settings).to_bits()
            );
            assert!(baseline.v[SFB].is_finite());
            control.cancel.store(true, Ordering::Relaxed);
        };
        let result = run_profiled::<false, false>(
            &seed,
            &seed,
            &corpora,
            &weights,
            &settings,
            &locks(&seed),
            &control,
            &mut callback,
            &mut Profile::default(),
        )
        .unwrap();
        assert!(frames > 0);
        assert_eq!(result.progress.phase, "stopped");
        assert!(result.has_best);
    }

    #[test]
    fn generated_archives_obey_diversity_without_external_layout_fixtures() {
        let seed = layout();
        let weights = Weights::default();
        let corpus = corpus("inline.json", b"aa aq hr rh aqr qra");
        for design in ["random", "evolve"] {
            let settings = SearchSettings {
                design: design.into(),
                diversity: 1,
                restarts: 4,
                ..settings()
            };
            let effort = LocalEffort::new(&seed, &weights);
            let cache = ng::Incremental::new(
                &corpus,
                &seed,
                Geometry::new(physical_keys(&seed)),
                &AtomicBool::new(false),
                |a, b, key| effort.get(a, b, key),
            )
            .unwrap();
            let baseline = cache.metrics();
            let mut problem = Problem {
                seed: &seed,
                weights: &weights,
                settings: &settings,
                shares: vec![1.0],
                baseline: vec![baseline],
                effort,
                caches: vec![cache],
                proposals: vec![ng::Proposal::new()],
            };
            let identity: Vec<_> = (0..seed.slots.len()).collect();
            let initial = problem.current(identity.clone());
            let result = search::<false, false>(
                &mut problem,
                initial,
                &locks(&seed),
                &Control::new(),
                &mut quiet,
                &mut Profile::default(),
                vec![identity.clone()],
            )
            .unwrap();
            assert!(result.archive.len() <= settings.archive);
            for (i, candidate) in result.archive.iter().enumerate() {
                assert!(letter_distance(&seed, &identity, &candidate.arr) >= 1);
                for other in &result.archive[..i] {
                    assert!(letter_distance(&seed, &other.arr, &candidate.arr) >= 1);
                }
            }
        }
    }

    #[test]
    fn arrangements_preserve_geometry_and_action_definitions() {
        let seed = layout();
        let mut arr: Vec<_> = (0..seed.slots.len()).collect();
        arr.swap(0, 25);
        let moved = arrangement(&seed, &arr);
        assert!(same_geometry(&seed, &moved));
        assert_eq!(seed.actions, moved.actions);
        assert_eq!(permutation(&seed, &moved), Some(arr));
        let mut altered = moved;
        altered.slots[0].finger = 3;
        assert!(permutation(&seed, &altered).is_none());
    }
}
