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

/// `layout`'s letter placement as an arrangement of `seed`, matching keys by
/// what they type alone, whatever adaptive rules, combos or magic rules either
/// has: an adaptive key types its fallback letter like a plain one. None when
/// the two hold different keys.
pub(crate) fn letter_arrangement(seed: &ak::Layout, layout: &ak::Layout) -> Option<Vec<usize>> {
    fn identity<'a>(
        layout: &'a ak::Layout,
        slot: &'a ak::Slot,
    ) -> Result<&'a [u8], &'a ak::Binding> {
        match &slot.binding {
            ak::Binding::Text(text) => Ok(text),
            ak::Binding::Named(name) => match layout.actions.get(name) {
                Some(ak::Action::Rules {
                    basis: ak::Basis::Text,
                    fallback: ak::Emission::Text(text),
                    ..
                }) if name.as_bytes() == text.as_slice() => Ok(text),
                _ => Err(&slot.binding),
            },
            ak::Binding::Empty => Err(&slot.binding),
        }
    }
    if !same_geometry(seed, layout) {
        return None;
    }
    let mut used = vec![false; seed.slots.len()];
    let mut arr = Vec::with_capacity(seed.slots.len());
    for slot in &layout.slots {
        let key = identity(layout, slot);
        let id = (0..seed.slots.len())
            .position(|id| !used[id] && identity(seed, &seed.slots[id]) == key)?;
        used[id] = true;
        arr.push(id);
    }
    Some(arr)
}

/// The saved layouts results keep their distance from. Every epoch of every
/// worker reads them, so they are parsed once per process and again only
/// after a file in the layout folders is added, removed or changed.
fn saved_layouts() -> Arc<[ak::Layout]> {
    type Stamp = Vec<(PathBuf, u64, Option<std::time::SystemTime>)>;
    static CACHE: std::sync::Mutex<Option<(Stamp, Arc<[ak::Layout]>)>> =
        std::sync::Mutex::new(None);
    let directory = Path::new(LAYOUT_DIR);
    let mut stamp = Stamp::new();
    for folder in [directory.to_path_buf(), directory.join("optimized")] {
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let meta = entry.metadata().ok();
            stamp.push((
                entry.path(),
                meta.as_ref().map_or(0, |meta| meta.len()),
                meta.and_then(|meta| meta.modified().ok()),
            ));
        }
    }
    stamp.sort();
    // Workers starting together wait for one parse instead of repeating it.
    let mut cache = CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((cached, layouts)) = cache.as_ref() {
        if *cached == stamp {
            return layouts.clone();
        }
    }
    let mut layouts = Vec::new();
    if let Ok(paths) = discover_layouts(directory) {
        for path in paths {
            if let Ok(text) = fs::read_to_string(&path) {
                if let Ok(layout) = ak::Layout::parse(&text, &path) {
                    layouts.push(layout);
                }
            }
        }
    }
    let layouts: Arc<[ak::Layout]> = layouts.into();
    *cache = Some((stamp, layouts.clone()));
    layouts
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

/// What a trial's score and cap checks read: a projected screened trial
/// carries only its capped metrics (`ng::Proposal::screened`).
fn scoring_metrics(proposal: &ng::Proposal) -> Metrics {
    match proposal.screened() {
        Some((_, capped)) => {
            let mut v = [0.0; N_METRICS];
            for (metric, value) in [SFB, SFS, TRAVEL, SFTRAVEL].into_iter().zip(capped) {
                v[metric] = value;
            }
            Metrics {
                v,
                usage: [0.0; 10],
                off: [0.0; 8],
                corner: 0.0,
            }
        }
        None => proposal.metrics(),
    }
}

fn objective(metrics: &Metrics, weights: &Weights, _settings: &SearchSettings) -> f64 {
    breakdown(metrics, weights).net
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

/// Points a joint epoch charges per unit above a cap (a percentage point of
/// SFB or SFS, a travel unit). Between epochs the rule, combo and swap fits can
/// still bring the layout back under, and only results within the caps after
/// them are kept, so annealing may pass through layouts above a cap.
const SOFT_CAP_POINTS: f64 = 20.0;

/// How far `metrics` go above `baseline` plus each active cap, summed in the
/// caps' own units; zero within every cap.
pub(crate) fn overshoot(metrics: &Metrics, baseline: &Metrics, settings: &SearchSettings) -> f64 {
    let mut amount = 0.0;
    for (metric, limit) in [
        (SFB, settings.sfb_limit),
        (SFS, settings.sfs_limit),
        (TRAVEL, settings.travel_limit),
        (SFTRAVEL, settings.sftravel_limit),
    ] {
        if let Some(increase) = limit {
            let excess = metrics.v[metric] - (baseline.v[metric] + increase);
            if excess > 1e-10 {
                amount += excess;
            }
        }
    }
    amount
}

/// A fitter's view of the search caps: candidates closer to within them win
/// first, then lower scores.
#[derive(Clone, Copy)]
pub(crate) struct Caps<'a> {
    pub(crate) baseline: &'a Metrics,
    pub(crate) settings: &'a SearchSettings,
}

impl Caps<'_> {
    /// Whether `a` beats `b`: less overshoot, or as much and a score lower by
    /// more than `margin`. Without caps only the scores decide.
    pub(crate) fn beats(
        caps: Option<Self>,
        a: &crate::action_summary::Summary,
        b: &crate::action_summary::Summary,
        margin: f64,
    ) -> bool {
        let over = |summary: &crate::action_summary::Summary| {
            caps.map_or(0.0, |caps| {
                overshoot(&summary.metrics, caps.baseline, caps.settings)
            })
        };
        let (a_over, b_over) = (over(a), over(b));
        if a_over != b_over {
            return a_over < b_over;
        }
        a.score < b.score - margin
    }

    pub(crate) fn overshoot(caps: Option<Self>, summary: &crate::action_summary::Summary) -> f64 {
        caps.map_or(0.0, |caps| {
            overshoot(&summary.metrics, caps.baseline, caps.settings)
        })
    }

    /// `overshoot` of a screened summary; None when a capped metric is so
    /// close to its cap that the exact summary may count it differently.
    fn screened_overshoot(
        caps: Option<Self>,
        summary: &crate::action_summary::Summary,
    ) -> Option<f64> {
        if let Some(caps) = caps {
            let settings = caps.settings;
            for (metric, limit) in [
                (SFB, settings.sfb_limit),
                (SFS, settings.sfs_limit),
                (TRAVEL, settings.travel_limit),
                (SFTRAVEL, settings.sftravel_limit),
            ] {
                if let Some(increase) = limit {
                    let excess = summary.metrics.v[metric] - (caps.baseline.v[metric] + increase);
                    if near(excess, 1e-10) {
                        return None;
                    }
                }
            }
        }
        Some(Self::overshoot(caps, summary))
    }

    /// `beats` on the exact summaries, judged from a screened `a` (and `b`
    /// when `b_screened`); None when they are too close to tell.
    pub(crate) fn screened_beats(
        caps: Option<Self>,
        a: &crate::action_summary::Summary,
        b: &crate::action_summary::Summary,
        b_screened: bool,
        margin: f64,
    ) -> Option<bool> {
        let a_over = Self::screened_overshoot(caps, a)?;
        let b_over = if b_screened {
            Self::screened_overshoot(caps, b)?
        } else {
            Self::overshoot(caps, b)
        };
        // Overshoot without any capped metric over is exactly zero.
        if a_over != 0.0 || b_over != 0.0 {
            return (!near(a_over, b_over)).then_some(a_over < b_over);
        }
        let threshold = b.score - margin;
        (!near(a.score, threshold)).then_some(a.score < threshold)
    }
}

struct FullContext {
    cache: ng::Incremental,
    proposal: ng::Proposal,
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
    /// Full-context counts for corpora whose alternate fingers require more
    /// context than the fast search. Only candidate moves that may be kept
    /// need to update them.
    full: Vec<Option<FullContext>>,
    full_pending: Option<Move>,
    /// Trials return screened scores (`Incremental::enable_aggregation`);
    /// decisions near a threshold call `exact` first.
    screening: bool,
    /// Joint epochs charge `SOFT_CAP_POINTS` above a cap instead of
    /// rejecting the move.
    soft_caps: bool,
}

/// Screened scores differ from exact ones in the last bits; any decision
/// this close to its threshold is made on the exact score.
const SCREEN_MARGIN: f64 = 1e-9;

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() <= SCREEN_MARGIN * (1.0 + a.abs().max(b.abs()))
}

#[derive(Clone)]
struct State {
    arr: Vec<usize>,
    numeric: Vec<ng::NumericState>,
    full_numeric: Vec<Option<ng::NumericState>>,
    score: f64,
    violation: f64,
}

impl Problem<'_> {
    fn current(&self, arr: Vec<usize>) -> State {
        let mut score = 0.0;
        let mut excess = 0.0;
        for (i, cache) in self.caches.iter().enumerate() {
            let metrics = cache.metrics();
            let checked = self.full[i]
                .as_ref()
                .filter(|_| self.has_caps())
                .map(|full| full.cache.metrics())
                .unwrap_or_else(|| metrics.clone());
            let value = if self.settings.uses_mana2_stats() {
                cache.physical_score(self.weights)
            } else {
                objective(&metrics, self.weights, self.settings)
            };
            score += self.shares[i] * value;
            if self.shares[i] != 0.0 {
                if self.soft_caps {
                    score += self.shares[i]
                        * SOFT_CAP_POINTS
                        * overshoot(&checked, &self.baseline[i], self.settings);
                } else {
                    excess += violation(&checked, &self.baseline[i], self.settings);
                }
            }
        }
        State {
            arr,
            numeric: self
                .caches
                .iter()
                .map(ng::Incremental::numeric_state)
                .collect(),
            full_numeric: self
                .full
                .iter()
                .map(|full| {
                    full.as_ref()
                        .filter(|_| self.has_caps())
                        .map(|full| full.cache.numeric_state())
                })
                .collect(),
            score,
            violation: excess,
        }
    }

    fn restore(&mut self, state: &State, control: &Control) -> ak::Result<()> {
        self.full_pending = None;
        let track_full = self.has_caps();
        for (i, (cache, numeric)) in self.caches.iter_mut().zip(&state.numeric).enumerate() {
            cache.restore(&state.arr, Some(numeric), &control.cancel, |a, b, key| {
                self.effort.get(a, b, key)
            })?;
            if let Some(full) = self.full[i].as_mut().filter(|_| track_full) {
                full.cache.restore(
                    &state.arr,
                    state.full_numeric[i].as_ref(),
                    &control.cancel,
                    |a, b, key| self.effort.get(a, b, key),
                )?;
            }
        }
        Ok(())
    }

    fn fresh(&mut self, arr: Vec<usize>, control: &Control) -> ak::Result<State> {
        self.full_pending = None;
        let track_full = self.has_caps();
        for (i, cache) in self.caches.iter_mut().enumerate() {
            cache.restore(&arr, None, &control.cancel, |a, b, key| {
                self.effort.get(a, b, key)
            })?;
            if let Some(full) = self.full[i].as_mut().filter(|_| track_full) {
                full.cache
                    .restore(&arr, None, &control.cancel, |a, b, key| {
                        self.effort.get(a, b, key)
                    })?;
            }
        }
        Ok(self.current(arr))
    }

    fn trial<const PROFILE: bool, const DETAIL: bool>(
        &mut self,
        movement: Move,
        control: &Control,
        profile: &mut Profile,
    ) -> ak::Result<(f64, f64)> {
        self.full_pending = None;
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

        // Joint searches include a soft cap penalty in every candidate's
        // score, so that penalty needs the full counts before ranking it.
        if self.soft_caps && self.has_caps() && self.has_full() {
            self.verify_trial::<PROFILE>(movement, control, profile, 0.0)?;
        }
        let mut result = self.proposal_scores::<PROFILE>(profile);
        if self.screening && self.near_cap() {
            result = self.exact();
        }
        if PROFILE {
            profile.candidates += 1;
        }
        Ok(result)
    }

    fn has_full(&self) -> bool {
        self.full.iter().any(Option::is_some)
    }

    fn has_caps(&self) -> bool {
        [
            self.settings.sfb_limit,
            self.settings.sfs_limit,
            self.settings.travel_limit,
            self.settings.sftravel_limit,
        ]
        .iter()
        .any(Option::is_some)
    }

    /// Recompute a candidate's full-context counters before it can be
    /// committed. Fast scores can still screen candidates that cannot win.
    fn verify_trial<const PROFILE: bool>(
        &mut self,
        movement: Move,
        control: &Control,
        profile: &mut Profile,
        fallback: f64,
    ) -> ak::Result<f64> {
        if !self.has_full() || !self.has_caps() {
            return Ok(fallback);
        }
        let started = clock::<PROFILE>();
        if let Some(pending) = self.full_pending {
            assert_eq!(pending.len, movement.len);
            assert_eq!(pending.slots, movement.slots);
        } else {
            for full in self.full.iter_mut().flatten() {
                full.cache.propose_move_profiled::<false, false, _>(
                    movement,
                    &control.cancel,
                    |a, b, key| self.effort.get(a, b, key),
                    &mut full.proposal,
                    &mut Profile::default(),
                )?;
                full.cache.exact(&mut full.proposal);
            }
            self.full_pending = Some(movement);
        }
        let mut excess = 0.0;
        if !self.soft_caps {
            for (i, proposal) in self.proposals.iter().enumerate() {
                if self.shares[i] == 0.0 {
                    continue;
                }
                let metrics = self.full[i]
                    .as_ref()
                    .map(|full| full.proposal.metrics())
                    .unwrap_or_else(|| scoring_metrics(proposal));
                excess += violation(&metrics, &self.baseline[i], self.settings);
            }
        }
        if PROFILE {
            profile.checks += elapsed::<PROFILE>(started);
        }
        Ok(excess)
    }

    /// Whether a screened proposal's capped metric is close enough to its cap
    /// that the last bits could decide whether it is above.
    fn near_cap(&self) -> bool {
        let limits = [
            (SFB, self.settings.sfb_limit),
            (SFS, self.settings.sfs_limit),
            (TRAVEL, self.settings.travel_limit),
            (SFTRAVEL, self.settings.sftravel_limit),
        ];
        if limits.iter().all(|(_, limit)| limit.is_none()) {
            return false;
        }
        self.proposals.iter().enumerate().any(|(i, proposal)| {
            if self.shares[i] == 0.0 {
                return false;
            }
            let metrics = self.full[i]
                .as_ref()
                .filter(|_| self.full_pending.is_some())
                .map(|full| full.proposal.metrics())
                .unwrap_or_else(|| scoring_metrics(proposal));
            limits.iter().any(|&(metric, limit)| {
                limit.is_some_and(|increase| {
                    near(
                        metrics.v[metric],
                        self.baseline[i].v[metric] + increase + 1e-10,
                    )
                })
            })
        })
    }

    /// The current proposals' exact score and excess: replays screened
    /// proposals context by context.
    fn exact(&mut self) -> (f64, f64) {
        for (cache, proposal) in self.caches.iter().zip(&mut self.proposals) {
            cache.exact(proposal);
        }
        self.proposal_scores::<false>(&mut Profile::default())
    }

    fn proposal_scores<const PROFILE: bool>(&self, profile: &mut Profile) -> (f64, f64) {
        let mut score = 0.0;
        let mut excess = 0.0;
        for (i, proposal) in self.proposals.iter().enumerate() {
            let score_start = clock::<PROFILE>();
            let metrics = scoring_metrics(proposal);
            let checked = self.full[i]
                .as_ref()
                .filter(|_| self.full_pending.is_some())
                .map(|full| full.proposal.metrics())
                .unwrap_or_else(|| metrics.clone());
            let value = if let Some((score, _)) = proposal.screened() {
                score
            } else if self.settings.uses_mana2_stats() {
                proposal.physical_score(self.weights)
            } else {
                objective(&metrics, self.weights, self.settings)
            };
            score += self.shares[i] * value;
            if PROFILE {
                profile.score += elapsed::<PROFILE>(score_start);
            }

            let check_start = clock::<PROFILE>();
            if self.shares[i] != 0.0 {
                if self.soft_caps {
                    score += self.shares[i]
                        * SOFT_CAP_POINTS
                        * overshoot(&checked, &self.baseline[i], self.settings);
                } else {
                    excess += violation(&checked, &self.baseline[i], self.settings);
                }
            }
            if PROFILE {
                profile.checks += elapsed::<PROFILE>(check_start);
            }
        }
        (score, excess)
    }

    fn commit<const PROFILE: bool>(
        &mut self,
        state: &mut State,
        movement: Move,
        profile: &mut Profile,
    ) {
        if self.has_full() && self.has_caps() {
            let pending = self
                .full_pending
                .expect("full-context move was not verified");
            assert_eq!(pending.len, movement.len);
            assert_eq!(pending.slots, movement.slots);
        }
        for (cache, proposal) in self.caches.iter_mut().zip(&mut self.proposals) {
            cache.commit_profiled::<PROFILE>(proposal, profile);
        }
        if self.has_caps() {
            for full in self.full.iter_mut().flatten() {
                full.cache
                    .commit_profiled::<false>(&mut full.proposal, &mut Profile::default());
            }
        }
        self.full_pending = None;
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

/// The longest a displayed best's full-context counts wait for a recount.
const FULL_DISPLAY_INTERVAL: Duration = Duration::from_secs(1);

struct Runtime<'a> {
    started: Instant,
    last_frame: Instant,
    /// When the displayed best's full-context counts were last made, and
    /// the counts.
    last_full: Instant,
    shown: Option<(Raw, [f64; 4])>,
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

    fn frame(&mut self, force: bool, problem: &mut Problem<'_>) -> ak::Result<()> {
        if force || self.last_frame.elapsed() >= Duration::from_millis(100) {
            self.progress.elapsed = self.started.elapsed().as_secs_f64();
            self.last_frame = Instant::now();
            let snapshot = self.snapshot();
            let baseline = self.baseline.clone();
            let mut stale = false;
            if self.best.full_numeric[0].is_none() {
                if let Some(full) = &mut problem.full[0] {
                    // With no caps the full evaluator is only needed for a
                    // displayed best, so don't update it for every move,
                    // and recount it at most once a second: until then the
                    // last counts stay shown. A final frame must still be
                    // available after cancellation.
                    if force
                        || self.shown.is_none()
                        || self.last_full.elapsed() >= FULL_DISPLAY_INTERVAL
                    {
                        full.cache.restore(
                            &self.best.arr,
                            None,
                            &AtomicBool::new(false),
                            |a, b, key| problem.effort.get(a, b, key),
                        )?;
                        self.best.full_numeric[0] = Some(full.cache.numeric_state());
                        self.last_full = Instant::now();
                    } else {
                        stale = true;
                    }
                }
            }
            let (raw, totals) = match &self.shown {
                Some((raw, totals)) if stale => (raw.clone(), *totals),
                _ => {
                    let numeric = self.best.full_numeric[0]
                        .as_ref()
                        .unwrap_or(&self.best.numeric[0]);
                    let (raw, totals) = numeric.raw_totals();
                    (raw.clone(), totals)
                }
            };
            self.shown = Some((raw.clone(), totals));
            (self.callback)(snapshot, baseline, raw, totals);
        }
        Ok(())
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
                let (mut score, mut excess) =
                    trial::<PROFILE, DETAIL>(problem, current, movement, runtime, profile)?;
                // Improvements are ranked by their exact gain.
                if problem.screening
                    && (score < current.score - 1e-10 || near(score, current.score - 1e-10))
                {
                    score = problem.exact().0;
                }
                if score < current.score - 1e-10 {
                    if problem.has_full() && problem.has_caps() && !problem.soft_caps {
                        excess = problem.verify_trial::<PROFILE>(
                            movement,
                            runtime.control,
                            profile,
                            excess,
                        )?;
                    }
                    if excess == 0.0 {
                        good.push((score - current.score, movement));
                    }
                }
                // This first pass ranks proposals; it never commits them.
                if PROFILE {
                    profile.rejected += 1;
                }
                runtime.frame(false, problem)?;
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
            let (mut score, excess) =
                trial::<PROFILE, DETAIL>(problem, current, *movement, runtime, profile)?;
            if problem.screening
                && (score < current.score - 1e-10 || near(score, current.score - 1e-10))
            {
                score = problem.exact().0;
            }
            let excess = if score < current.score - 1e-10 {
                problem.verify_trial::<PROFILE>(*movement, runtime.control, profile, excess)?
            } else {
                excess
            };
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
        runtime.frame(false, problem)?;
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
                let (_, mut excess) =
                    trial::<PROFILE, DETAIL>(problem, current, movement, runtime, profile)?;
                // Repair compares excesses themselves, so exactly.
                if problem.screening {
                    excess = problem.exact().1;
                }
                excess =
                    problem.verify_trial::<PROFILE>(movement, runtime.control, profile, excess)?;
                if excess < best {
                    best = excess;
                    chosen = Some(movement);
                }
                if PROFILE {
                    profile.rejected += 1;
                }
                runtime.frame(false, problem)?;
            }
        }
        let Some(movement) = chosen else {
            break;
        };
        let (_, excess) = trial::<PROFILE, DETAIL>(problem, current, movement, runtime, profile)?;
        problem.verify_trial::<PROFILE>(movement, runtime.control, profile, excess)?;
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
    started: Instant,
    restart_offset: usize,
    run_sweeps: bool,
    parents: &[Candidate],
) -> ak::Result<Snapshot> {
    let generation = problem.settings.design != "refine";
    if !generation && initial.violation != 0.0 {
        return Err("starting arrangement violates active limits".into());
    }
    let free: Vec<_> = (0..locked.len()).filter(|&slot| !locked[slot]).collect();
    let mut runtime = Runtime {
        started,
        last_frame: started,
        last_full: started,
        shown: None,
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
            let global_restart = restart_offset + restart;
            if !can_search || runtime.checkpoint(problem.settings) {
                break;
            }
            runtime.progress.restart = restart + 1;
            runtime.progress.pass = 0;
            if generation {
                // Earlier restarts' results (`parents`) breed with this
                // search's own when `evolve` crosses two of them.
                let mut pool = runtime.pool.clone();
                pool.extend_from_slice(parents);
                pool.sort_by(|a, b| a.score.total_cmp(&b.score).then_with(|| a.arr.cmp(&b.arr)));
                let parents = diverse(
                    problem.seed,
                    &pool,
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
                current.clone_from(if global_restart == 0 {
                    &initial
                } else {
                    &runtime.best
                });
                if global_restart > 0 {
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
                        let excess =
                            problem.verify_trial::<PROFILE>(movement, control, profile, excess)?;
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
                    let mut delta = score - current.score;
                    // A screened gain decides alone only when clearly positive;
                    // the acceptance draw happens exactly when the exact gain
                    // is not negative.
                    if problem.screening && (delta < 0.0 || near(score, current.score)) {
                        delta = problem.exact().0 - current.score;
                    }
                    let can_screen_limit = problem.has_full() || excess == 0.0;
                    let mut accept = |delta: &mut f64| {
                        let unit = rng.unit();
                        let mut threshold = (-*delta / temperature).exp();
                        if problem.screening && near(unit, threshold) {
                            *delta = problem.exact().0 - current.score;
                            threshold = (-*delta / temperature).exp();
                        }
                        unit < threshold
                    };
                    let accepted_by_score = can_screen_limit && (delta < 0.0 || accept(&mut delta));
                    let accepted = if accepted_by_score {
                        problem.verify_trial::<PROFILE>(movement, control, profile, excess)? == 0.0
                    } else {
                        false
                    };
                    if accepted {
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
                    runtime.frame(false, problem)?;
                }
                current = local;
                if runtime.stop(problem.settings) {
                    runtime.keep(&current, problem);
                    runtime.frame(true, problem)?;
                    break;
                }
                problem.restore(&current, control)?;
            }
            if run_sweeps {
                sweep::<PROFILE, DETAIL>(problem, &mut current, &free, &mut runtime, profile)?;
            }
            runtime.keep(&current, problem);
            runtime.frame(true, problem)?;
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
            if locked && candidate.arr[slot] != initial.arr[slot] {
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
    // Pair every returned best with an exact callback payload. Parallel
    // coordination relies on this final frame when cancellation lands between
    // the periodic frame interval and an accepted move.
    runtime.frame(true, problem)?;
    Ok(runtime.snapshot())
}

fn run_profiled_at<const PROFILE: bool, const DETAIL: bool>(
    original: &ak::Layout,
    seed: &ak::Layout,
    corpora: &[(ng::NgramCorpus, f64)],
    weights: &Weights,
    settings: &SearchSettings,
    locked: &[bool],
    control: &Control,
    callback: &mut dyn FnMut(Snapshot, Metrics, Raw, [f64; 4]),
    profile: &mut Profile,
    search_started: Option<Instant>,
    restart_offset: usize,
    external_baseline: Option<&[Metrics]>,
    run_sweeps: bool,
    parents: &[Candidate],
) -> ak::Result<Snapshot> {
    validate_search_settings(settings).map_err(|error| error.to_string())?;
    if locked.len() != seed.slots.len() || corpora.is_empty() {
        return Err("invalid action search lock mask or empty training corpus list".into());
    }
    if external_baseline.is_some_and(|baseline| baseline.len() != corpora.len()) {
        return Err("joint search baseline/corpus count mismatch".into());
    }
    let baseline_arr = if external_baseline.is_some() {
        (0..seed.slots.len()).collect()
    } else {
        permutation(seed, original)
            .ok_or("baseline layout is not a permutation of the search layout")?
    };
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
    // A finite effort table lets trials remap only the contexts whose
    // dependencies a move touches; see `Incremental::enable_finite_effort`.
    let finite_effort = effort.all_finite();
    // Use projected search scores, resolving numerical ties by replaying the
    // proposal. Full-context limits are checked separately before a commit.
    let screening = true;
    let geometry = std::sync::Arc::new(Geometry::with_rolls(physical_keys(seed), weights.rolls()));
    let identity: Vec<_> = (0..seed.slots.len()).collect();
    let mut caches = Vec::with_capacity(corpora.len());
    let mut full = Vec::with_capacity(corpora.len());
    let mut baseline = Vec::with_capacity(corpora.len());
    for (corpus_index, (corpus, _)) in corpora.iter().enumerate() {
        let mut cache = ng::Incremental::new_with_shared_geometry(
            corpus,
            seed,
            std::sync::Arc::clone(&geometry),
            &control.cancel,
            weights.alts(),
            weights.combo_policy(),
            |a, b, key| effort.get(a, b, key),
        )?;
        if finite_effort {
            cache.enable_finite_effort();
        }
        if screening {
            cache.enable_aggregation();
        }
        if settings.uses_mana2_stats() {
            cache.enable_mana2();
        } else if screening {
            cache.enable_score_projection(score_terms(weights));
        }
        let needs_full = crate::combos::Program::for_contexts(
            seed,
            weights.combo_policy(),
            weights.alts(),
            corpus.order(),
            |pair| corpus.has_pair(pair),
        )?
        .is_some_and(|program| program.ahead() || program.alt_contexts());
        let mut full_cache = if needs_full {
            let mut full = ng::Incremental::new_with_full_contexts(
                corpus,
                seed,
                std::sync::Arc::clone(&geometry),
                &control.cancel,
                weights.alts(),
                weights.combo_policy(),
                |a, b, key| effort.get(a, b, key),
            )?;
            if finite_effort {
                full.enable_finite_effort();
            }
            if screening {
                full.enable_aggregation();
            }
            if settings.uses_mana2_stats() {
                full.enable_mana2();
            }
            Some(full)
        } else {
            None
        };
        if let Some(metrics) = external_baseline {
            baseline.push(metrics[corpus_index].clone());
        } else if baseline_arr != identity {
            let saved = cache.numeric_state();
            let full_saved = full_cache.as_ref().map(ng::Incremental::numeric_state);
            cache.restore(&baseline_arr, None, &control.cancel, |a, b, key| {
                effort.get(a, b, key)
            })?;
            if let Some(full) = &mut full_cache {
                full.restore(&baseline_arr, None, &control.cancel, |a, b, key| {
                    effort.get(a, b, key)
                })?;
            }
            baseline.push(full_cache.as_ref().unwrap_or(&cache).metrics());
            cache.restore(&identity, Some(&saved), &control.cancel, |a, b, key| {
                effort.get(a, b, key)
            })?;
            if let Some(full) = &mut full_cache {
                full.restore(
                    &identity,
                    full_saved.as_ref(),
                    &control.cancel,
                    |a, b, key| effort.get(a, b, key),
                )?;
            }
        } else {
            baseline.push(full_cache.as_ref().unwrap_or(&cache).metrics());
        }
        caches.push(cache);
        full.push(full_cache.map(|cache| FullContext {
            cache,
            proposal: ng::Proposal::new(),
        }));
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
        full,
        full_pending: None,
        screening,
        soft_caps: external_baseline.is_some(),
    };
    let initial = problem.current(identity);
    let generation = settings.design != "refine";
    let mut known = vec![initial.arr.clone()];
    if generation {
        for layout in saved_layouts().iter() {
            if let Some(arr) = permutation(problem.seed, layout) {
                known.push(arr);
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
        search_started.unwrap_or_else(Instant::now),
        restart_offset,
        run_sweeps,
        parents,
    )
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
    run_profiled_at::<PROFILE, DETAIL>(
        original,
        seed,
        corpora,
        weights,
        settings,
        locked,
        control,
        callback,
        profile,
        None,
        0,
        None,
        true,
        &[],
    )
}

/// One normal search epoch with fixed external caps. The joint coordinator
/// owns dictionary changes and parallel restart chains; ordinary search is unchanged.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_joint_epoch(
    seed: &ak::Layout,
    corpora: &[(ng::NgramCorpus, f64)],
    weights: &Weights,
    settings: &SearchSettings,
    locked: &[bool],
    control: &Control,
    baseline: &[Metrics],
    run_sweeps: bool,
    restart_offset: usize,
    parents: &[Candidate],
    callback: &mut dyn FnMut(Snapshot, Metrics, Raw, [f64; 4]),
) -> ak::Result<Snapshot> {
    let level = crate::action_profile::level();
    let mut profile = Profile::default();
    let started = (level != 0).then(Instant::now);
    let result = match level {
        0 => run_profiled_at::<false, false>(
            seed,
            seed,
            corpora,
            weights,
            settings,
            locked,
            control,
            callback,
            &mut profile,
            None,
            restart_offset,
            Some(baseline),
            run_sweeps,
            parents,
        ),
        1 => run_profiled_at::<true, false>(
            seed,
            seed,
            corpora,
            weights,
            settings,
            locked,
            control,
            callback,
            &mut profile,
            None,
            restart_offset,
            Some(baseline),
            run_sweeps,
            parents,
        ),
        _ => run_profiled_at::<true, true>(
            seed,
            seed,
            corpora,
            weights,
            settings,
            locked,
            control,
            callback,
            &mut profile,
            None,
            restart_offset,
            Some(baseline),
            run_sweeps,
            parents,
        ),
    };
    if let Some(started) = started {
        profile.total = started.elapsed();
        let mut report = Vec::new();
        let _ = profile.report(&mut report, "joint epoch", level == 2);
        crate::profile_output::write_report(&report);
    }
    result
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
    let settings = &settings.for_actions();
    let level = match std::env::var("AKLER_PROFILE")
        .or_else(|_| std::env::var("LAYOUTER_PROFILE"))
        .as_deref()
    {
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

struct ParallelView {
    snapshot: Snapshot,
    report: Option<(Metrics, Raw, [f64; 4])>,
}

enum ParallelMessage {
    Update {
        worker: usize,
        snapshot: Snapshot,
        baseline: Metrics,
        raw: Raw,
        totals: [f64; 4],
    },
    Done {
        worker: usize,
        result: ak::Result<Snapshot>,
    },
    Panicked {
        worker: usize,
    },
}

fn merge_parallel_views(
    seed: &ak::Layout,
    settings: &SearchSettings,
    views: &[Option<ParallelView>],
    started: Instant,
    finished: bool,
) -> Option<(Snapshot, usize)> {
    let populated: Vec<_> = views
        .iter()
        .enumerate()
        .filter_map(|(worker, view)| view.as_ref().map(|view| (worker, view)))
        .collect();
    let &(winner, best_view) = populated.iter().min_by(|(a_worker, a), (b_worker, b)| {
        // A feasible best always outranks a placeholder initial state. Stable
        // arrangement and worker tie breaks keep aggregation deterministic.
        b.snapshot
            .has_best
            .cmp(&a.snapshot.has_best)
            .then_with(|| a.snapshot.best.score.total_cmp(&b.snapshot.best.score))
            .then_with(|| a.snapshot.best.arr.cmp(&b.snapshot.best.arr))
            .then_with(|| a_worker.cmp(b_worker))
    })?;

    let mut pool: Vec<_> = populated
        .iter()
        .flat_map(|(_, view)| view.snapshot.archive.iter().cloned())
        .collect();
    pool.sort_by(|a, b| a.score.total_cmp(&b.score).then_with(|| a.arr.cmp(&b.arr)));
    let archive = diverse(
        seed,
        &pool,
        settings.archive,
        settings.diversity,
        settings.design != "refine",
    );
    let mut progress = Progress {
        phase: if finished {
            "finished".into()
        } else {
            best_view.snapshot.progress.phase.clone()
        },
        restart: 0,
        pass: 0,
        evaluations: 0,
        accepted: 0,
        elapsed: started.elapsed().as_secs_f64(),
        seed: settings.seed,
    };
    for (_, view) in &populated {
        progress.restart = progress
            .restart
            .saturating_add(view.snapshot.progress.restart);
        progress.pass = progress.pass.max(view.snapshot.progress.pass);
        progress.evaluations = progress
            .evaluations
            .saturating_add(view.snapshot.progress.evaluations);
        progress.accepted = progress
            .accepted
            .saturating_add(view.snapshot.progress.accepted);
    }
    Some((
        Snapshot {
            best: best_view.snapshot.best.clone(),
            progress,
            archive,
            has_best: best_view.snapshot.has_best,
        },
        winner,
    ))
}

/// Run independent restart groups concurrently. Each worker executes the
/// existing serial search unchanged with its own caches and deterministic RNG
/// seed; this coordinator only partitions restarts and merges snapshots.
pub(crate) fn run_parallel(
    original: &ak::Layout,
    seed: &ak::Layout,
    corpora: &[(ng::NgramCorpus, f64)],
    weights: &Weights,
    settings: &SearchSettings,
    locked: &[bool],
    control: &Control,
    callback: &mut dyn FnMut(Snapshot, Metrics, Raw, [f64; 4]),
) -> ak::Result<Snapshot> {
    let settings = &settings.for_actions();
    let worker_count = crate::search_worker_count(settings);
    let profiling = std::env::var("AKLER_PROFILE")
        .or_else(|_| std::env::var("LAYOUTER_PROFILE"))
        .is_ok_and(|level| matches!(level.as_str(), "1" | "2"));
    if worker_count <= 1 || profiling {
        return run(
            original, seed, corpora, weights, settings, locked, control, callback,
        );
    }

    validate_search_settings(settings).map_err(|error| error.to_string())?;
    let started = Instant::now();
    let mut views: Vec<Option<ParallelView>> = (0..worker_count).map(|_| None).collect();
    let mut results: Vec<Option<ak::Result<Snapshot>>> = (0..worker_count).map(|_| None).collect();
    let mut panicked = vec![false; worker_count];
    let (tx, rx) = mpsc::channel();

    thread::scope(|scope| {
        for worker in 0..worker_count {
            let tx = tx.clone();
            let mut worker_settings = settings.clone();
            worker_settings.restarts = settings.restarts / worker_count
                + usize::from(worker < settings.restarts % worker_count);
            worker_settings.seed = crate::worker_seed(settings.seed, worker);
            let restart_offset = worker * (settings.restarts / worker_count)
                + worker.min(settings.restarts % worker_count);
            scope.spawn(move || {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let mut send_update = |snapshot, baseline, raw, totals| {
                        let _ = tx.send(ParallelMessage::Update {
                            worker,
                            snapshot,
                            baseline,
                            raw,
                            totals,
                        });
                    };
                    run_profiled_at::<false, false>(
                        original,
                        seed,
                        corpora,
                        weights,
                        &worker_settings,
                        locked,
                        control,
                        &mut send_update,
                        &mut Profile::default(),
                        Some(started),
                        restart_offset,
                        None,
                        true,
                        &[],
                    )
                }));
                let message = match outcome {
                    Ok(result) => ParallelMessage::Done { worker, result },
                    Err(_) => ParallelMessage::Panicked { worker },
                };
                let _ = tx.send(message);
            });
        }
        drop(tx);

        loop {
            let message = match rx.recv() {
                Ok(message) => message,
                Err(_) => break,
            };
            match message {
                ParallelMessage::Update {
                    worker,
                    snapshot,
                    baseline,
                    raw,
                    totals,
                } => {
                    views[worker] = Some(ParallelView {
                        snapshot,
                        report: Some((baseline, raw, totals)),
                    });
                    if let Some((merged, winner)) =
                        merge_parallel_views(seed, settings, &views, started, false)
                    {
                        if let Some((baseline, raw, totals)) =
                            views[winner].as_ref().and_then(|view| view.report.clone())
                        {
                            callback(merged, baseline, raw, totals);
                        }
                    }
                }
                ParallelMessage::Done { worker, result } => {
                    if let Ok(snapshot) = &result {
                        if let Some(view) = &mut views[worker] {
                            view.snapshot = snapshot.clone();
                        } else {
                            views[worker] = Some(ParallelView {
                                snapshot: snapshot.clone(),
                                report: None,
                            });
                        }
                    } else {
                        control.cancel.store(true, Ordering::Relaxed);
                    }
                    results[worker] = Some(result);
                }
                ParallelMessage::Panicked { worker } => {
                    panicked[worker] = true;
                    control.cancel.store(true, Ordering::Relaxed);
                }
            }
        }
    });

    if panicked.iter().any(|&value| value) {
        return Err("action search worker panicked".into());
    }
    let mut cancelled = false;
    let mut error = None;
    for result in results.into_iter().flatten() {
        if let Err(worker_error) = result {
            if worker_error == "cancelled" {
                cancelled = true;
            } else if error.is_none() {
                error = Some(worker_error);
            }
        }
    }
    if let Some(error) = error {
        return Err(error);
    }
    let Some((mut snapshot, winner)) = merge_parallel_views(seed, settings, &views, started, true)
    else {
        return Err(if cancelled {
            "cancelled".into()
        } else {
            "action search workers exited without a result".into()
        });
    };
    if control.cancel.load(Ordering::Relaxed) {
        snapshot.progress.phase = "stopped".into();
    } else if settings.seconds > 0.0 && started.elapsed().as_secs_f64() >= settings.seconds {
        snapshot.progress.phase = "time limit".into();
    }
    if let Some((baseline, raw, totals)) =
        views[winner].as_ref().and_then(|view| view.report.clone())
    {
        callback(snapshot.clone(), baseline, raw, totals);
    }
    Ok(snapshot)
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

    fn alternate_fixture(rule: &str, text: &[u8]) -> (ak::Layout, ng::NgramCorpus, Weights) {
        let seed = ak::Layout::parse(
            "q w e r t | y u i o p\na s d f g | h j k l ;\nz x c v b | n m , . /\nthumbs: space\n",
            Path::new("alternate-search.dat"),
        )
        .unwrap();
        let rules = crate::alt_fingers::AltRules::parse(rule).unwrap();
        let mut weights = Weights::new([0.0; N_WEIGHTS]);
        weights.1.alts = crate::alt_fingers::AltRules {
            rowstag: rules.clone(),
            colstag: rules,
        }
        .intern();
        (seed, corpus("alternate-search.json", text), weights)
    }

    fn full_metrics(seed: &ak::Layout, corpus: &ng::NgramCorpus, weights: &Weights) -> Metrics {
        crate::action_summary::evaluate_with_mode(
            seed,
            corpus,
            weights,
            &AtomicBool::new(false),
            &AtomicU64::new(0),
            "normal",
        )
        .unwrap()
        .metrics
    }

    #[test]
    fn alternate_finger_limits_and_joint_penalties_use_full_contexts() {
        let (mut seed, corpus, weights) = alternate_fixture("a_s (LP LM)", b"aqsvd aqsvd aqsvd");
        let q = seed
            .slots
            .iter()
            .position(|slot| slot.label == "q")
            .unwrap();
        let e = seed
            .slots
            .iter()
            .position(|slot| slot.label == "e")
            .unwrap();
        seed.swap(q, e);
        let settings = SearchSettings {
            sfs_limit: Some(0.0),
            ..settings()
        };
        let control = Control::new();
        let initial = full_metrics(&seed, &corpus, &weights);
        let mut changed = seed.clone();
        changed.swap(q, e);
        let changed_metrics = full_metrics(&changed, &corpus, &weights);
        assert!((initial.v[SFS] - 0.0).abs() < 1e-9);
        assert!((changed_metrics.v[SFS] - 20.0).abs() < 1e-9);

        for soft_caps in [false, true] {
            let effort = LocalEffort::new(&seed, &weights);
            let geometry =
                std::sync::Arc::new(Geometry::with_rolls(physical_keys(&seed), weights.rolls()));
            let make_cache = |full| {
                let constructor = if full {
                    ng::Incremental::new_with_full_contexts
                } else {
                    ng::Incremental::new_with_shared_geometry
                };
                constructor(
                    &corpus,
                    &seed,
                    geometry.clone(),
                    &control.cancel,
                    weights.alts(),
                    weights.combo_policy(),
                    |a, b, key| effort.get(a, b, key),
                )
                .unwrap()
            };
            let mut cache = make_cache(false);
            let mut full = make_cache(true);
            cache.enable_finite_effort();
            cache.enable_aggregation();
            cache.enable_score_projection(score_terms(&weights));
            full.enable_finite_effort();
            full.enable_aggregation();
            let mut problem = Problem {
                seed: &seed,
                weights: &weights,
                settings: &settings,
                shares: vec![1.0],
                baseline: vec![initial.clone()],
                effort,
                caches: vec![cache],
                proposals: vec![ng::Proposal::new()],
                full: vec![Some(FullContext {
                    cache: full,
                    proposal: ng::Proposal::new(),
                })],
                full_pending: None,
                screening: true,
                soft_caps,
            };
            let movement = Move::pair(q, e);
            let (score, fast_excess) = problem
                .trial::<false, false>(movement, &control, &mut Profile::default())
                .unwrap();
            assert_eq!(fast_excess, 0.0);
            let exact_excess = problem
                .verify_trial::<false>(movement, &control, &mut Profile::default(), fast_excess)
                .unwrap();
            if soft_caps {
                assert!((score - SOFT_CAP_POINTS * 20.0).abs() < 1e-8);
                assert_eq!(exact_excess, 0.0);
            } else {
                assert_eq!(score, 0.0);
                assert!(exact_excess > 0.0);
            }
        }
    }

    #[test]
    fn alternate_finger_limits_and_live_metrics_match_evaluation_with_16_workers() {
        let (mut seed, corpus, weights) = alternate_fixture("a_s (LP LM)", b"aqsvd aqsvd aqsvd");
        let q = seed
            .slots
            .iter()
            .position(|slot| slot.label == "q")
            .unwrap();
        let e = seed
            .slots
            .iter()
            .position(|slot| slot.label == "e")
            .unwrap();
        seed.swap(q, e);
        let mut locked = vec![true; seed.slots.len()];
        locked[q] = false;
        locked[e] = false;
        let settings = SearchSettings {
            workers: 16,
            restarts: 16,
            magic_anneal_steps: 1,
            sfs_limit: Some(0.0),
            cycle_probability: 0.0,
            ..settings()
        };
        let expected_baseline = full_metrics(&seed, &corpus, &weights);
        let mut frames = 0;
        let result = run_parallel(
            &seed,
            &seed,
            &[(corpus.clone(), 1.0)],
            &weights,
            &settings,
            &locked,
            &Control::new(),
            &mut |snapshot, baseline, raw, totals| {
                frames += 1;
                let expected =
                    full_metrics(&arrangement(&seed, &snapshot.best.arr), &corpus, &weights);
                let actual = metrics_totals(&raw, &totals);
                assert!((baseline.v[SFS] - expected_baseline.v[SFS]).abs() < 1e-9);
                for (actual, expected) in actual.v.iter().zip(expected.v) {
                    assert!((actual - expected).abs() < 1e-8 * (1.0 + expected.abs()));
                }
            },
        )
        .unwrap();
        assert!(frames > 0 && result.has_best);
        assert_eq!(result.progress.accepted, 0);
        for candidate in std::iter::once(&result.best).chain(&result.archive) {
            let metrics = full_metrics(&arrangement(&seed, &candidate.arr), &corpus, &weights);
            assert!(metrics.v[SFS] <= expected_baseline.v[SFS] + 1e-9);
        }
    }

    #[test]
    fn alternate_finger_live_metrics_use_full_contexts_without_limits() {
        let (seed, corpus, weights) = alternate_fixture("g_x (LM LR)", b"afgvx afgvx afgvx");
        let expected = full_metrics(&seed, &corpus, &weights);
        let mut observed = None;
        run(
            &seed,
            &seed,
            &[(corpus, 1.0)],
            &weights,
            &settings(),
            &vec![true; seed.slots.len()],
            &Control::new(),
            &mut |_, _, raw, totals| observed = Some(metrics_totals(&raw, &totals)),
        )
        .unwrap();
        assert!((expected.v[REDIR] - 100.0 / 3.0).abs() < 1e-8);
        assert!((observed.unwrap().v[REDIR] - expected.v[REDIR]).abs() < 1e-8);
    }

    #[test]
    fn uncapped_live_metrics_refresh_when_the_best_layout_changes() {
        let (mut seed, corpus, mut weights) =
            alternate_fixture("a_s (LP LM)", b"aqsvd aqsvd aqsvd");
        let q = seed
            .slots
            .iter()
            .position(|slot| slot.label == "q")
            .unwrap();
        let e = seed
            .slots
            .iter()
            .position(|slot| slot.label == "e")
            .unwrap();
        seed.swap(q, e);
        let effort = LocalEffort::new(&seed, &weights);
        let stop = AtomicBool::new(false);
        let cache = ng::Incremental::new_with_policy(
            &corpus,
            &seed,
            Geometry::with_rolls(physical_keys(&seed), weights.rolls()),
            &stop,
            weights.alts(),
            weights.combo_policy(),
            |a, b, key| effort.get(a, b, key),
        )
        .unwrap();
        let mut proposal = ng::Proposal::new();
        cache
            .propose_swap(
                q,
                e,
                &stop,
                |a, b, key| effort.get(a, b, key),
                &mut proposal,
            )
            .unwrap();
        let difference = proposal.metrics().v[TRAVEL] - cache.metrics().v[TRAVEL];
        assert!(difference.abs() > 1e-9);
        weights.0[TRAVEL] = -difference.signum();
        let mut locked = vec![true; seed.slots.len()];
        locked[q] = false;
        locked[e] = false;
        let identity: Vec<_> = (0..seed.slots.len()).collect();
        let mut changed = false;
        let result = run(
            &seed,
            &seed,
            &[(corpus.clone(), 1.0)],
            &weights,
            &SearchSettings {
                hybrid: false,
                restarts: 1,
                ..settings()
            },
            &locked,
            &Control::new(),
            &mut |snapshot, _, raw, totals| {
                changed |= snapshot.best.arr != identity;
                let expected =
                    full_metrics(&arrangement(&seed, &snapshot.best.arr), &corpus, &weights);
                let actual = metrics_totals(&raw, &totals);
                for (a, b) in actual.v.iter().zip(expected.v) {
                    assert!((a - b).abs() < 1e-8 * (1.0 + b.abs()));
                }
            },
        )
        .unwrap();
        assert!(changed && result.best.arr != identity);
    }

    #[test]
    fn limits_use_full_metrics_for_a_separate_original_layout() {
        let (original, corpus, weights) = alternate_fixture("a_s (LP LM)", b"aqsvd aqsvd aqsvd");
        let mut seed = original.clone();
        let q = seed
            .slots
            .iter()
            .position(|slot| slot.label == "q")
            .unwrap();
        let e = seed
            .slots
            .iter()
            .position(|slot| slot.label == "e")
            .unwrap();
        seed.swap(q, e);
        let expected = full_metrics(&original, &corpus, &weights);
        let mut baseline = None;
        run(
            &original,
            &seed,
            &[(corpus, 1.0)],
            &weights,
            &SearchSettings {
                sfs_limit: Some(0.0),
                ..settings()
            },
            &vec![true; seed.slots.len()],
            &Control::new(),
            &mut |_, metrics, _, _| baseline = Some(metrics),
        )
        .unwrap();
        assert!((expected.v[SFS] - 20.0).abs() < 1e-9);
        assert!((baseline.unwrap().v[SFS] - expected.v[SFS]).abs() < 1e-9);
    }

    #[test]
    fn bigram_lookahead_corrections_survive_incremental_moves_and_restore() {
        let (seed, corpus, weights) = alternate_fixture("g_x (LM LR)", b"afgvx afgvx fgvz");
        let effort = LocalEffort::new(&seed, &weights);
        let geometry =
            std::sync::Arc::new(Geometry::with_rolls(physical_keys(&seed), weights.rolls()));
        let stop = AtomicBool::new(false);
        let mut cache = ng::Incremental::new_with_full_contexts(
            &corpus,
            &seed,
            geometry,
            &stop,
            weights.alts(),
            weights.combo_policy(),
            |a, b, key| effort.get(a, b, key),
        )
        .unwrap();
        cache.enable_finite_effort();
        cache.enable_aggregation();
        cache.enable_mana2();
        let initial = cache.numeric_state();
        let mut arr: Vec<_> = (0..seed.slots.len()).collect();
        let slot = |name| {
            seed.slots
                .iter()
                .position(|slot| slot.label == name)
                .unwrap()
        };
        let moves = [
            Move::pair(slot("g"), slot("x")),
            Move::pair(slot("f"), slot("v")),
            Move {
                slots: [slot("x"), slot("e"), slot("g")],
                len: 3,
            },
            Move::pair(slot("f"), slot("x")),
        ];
        for movement in moves {
            let mut proposal = ng::Proposal::new();
            cache
                .propose_move_profiled::<false, false, _>(
                    movement,
                    &stop,
                    |a, b, key| effort.get(a, b, key),
                    &mut proposal,
                    &mut Profile::default(),
                )
                .unwrap();
            cache.exact(&mut proposal);
            arr.swap(movement.slots[0], movement.slots[1]);
            if movement.len == 3 {
                arr.swap(movement.slots[1], movement.slots[2]);
            }
            let layout = arrangement(&seed, &arr);
            let expected = full_metrics(&layout, &corpus, &weights);
            for (actual, expected) in proposal.metrics().v.iter().zip(expected.v) {
                assert!((actual - expected).abs() < 1e-8 * (1.0 + expected.abs()));
            }
            let physical = crate::action_summary::evaluate_with_mode(
                &layout,
                &corpus,
                &weights,
                &stop,
                &AtomicU64::new(0),
                "aklgg",
            )
            .unwrap()
            .score;
            assert!(
                (proposal.physical_score(&weights) - physical).abs()
                    < 1e-8 * (1.0 + physical.abs())
            );
            cache.commit(&mut proposal);
        }
        let identity: Vec<_> = (0..seed.slots.len()).collect();
        cache
            .restore(&identity, Some(&initial), &stop, |a, b, key| {
                effort.get(a, b, key)
            })
            .unwrap();
        for (actual, expected) in cache
            .metrics()
            .v
            .iter()
            .zip(full_metrics(&seed, &corpus, &weights).v)
        {
            assert!((actual - expected).abs() < 1e-8 * (1.0 + expected.abs()));
        }
    }

    #[test]
    fn full_context_incremental_moves_and_restore_match_fresh_evaluation() {
        let (seed, corpus, weights) = alternate_fixture("a_s (LP LM)", b"aqsvd aqsvd aqsvd");
        let effort = LocalEffort::new(&seed, &weights);
        let geometry =
            std::sync::Arc::new(Geometry::with_rolls(physical_keys(&seed), weights.rolls()));
        let stop = AtomicBool::new(false);
        let mut cache = ng::Incremental::new_with_full_contexts(
            &corpus,
            &seed,
            geometry,
            &stop,
            weights.alts(),
            weights.combo_policy(),
            |a, b, key| effort.get(a, b, key),
        )
        .unwrap();
        cache.enable_finite_effort();
        cache.enable_aggregation();
        cache.enable_mana2();
        let initial = cache.numeric_state();
        let mut arr: Vec<_> = (0..seed.slots.len()).collect();
        let q = seed
            .slots
            .iter()
            .position(|slot| slot.label == "q")
            .unwrap();
        let e = seed
            .slots
            .iter()
            .position(|slot| slot.label == "e")
            .unwrap();
        let compare = |actual: &Metrics, expected: &Metrics| {
            for (name, a, b) in [
                ("metric", actual.v.as_slice(), expected.v.as_slice()),
                ("usage", actual.usage.as_slice(), expected.usage.as_slice()),
                ("off", actual.off.as_slice(), expected.off.as_slice()),
            ] {
                for (index, (a, b)) in a.iter().zip(b).enumerate() {
                    assert!(
                        (a - b).abs() < 1e-8 * (1.0 + b.abs()),
                        "{name} {index}: {a} versus {b}"
                    );
                }
            }
        };
        let moves = [
            Move::pair(q, e),
            Move {
                slots: [q, 9, 12],
                len: 3,
            },
            Move::pair(12, 19),
            Move::pair(e, 7),
            Move::pair(q, e),
        ];
        for movement in moves {
            let mut proposal = ng::Proposal::new();
            cache
                .propose_move_profiled::<false, false, _>(
                    movement,
                    &stop,
                    |a, b, key| effort.get(a, b, key),
                    &mut proposal,
                    &mut Profile::default(),
                )
                .unwrap();
            cache.exact(&mut proposal);
            arr.swap(movement.slots[0], movement.slots[1]);
            if movement.len == 3 {
                arr.swap(movement.slots[1], movement.slots[2]);
            }
            let layout = arrangement(&seed, &arr);
            compare(
                &proposal.metrics(),
                &full_metrics(&layout, &corpus, &weights),
            );
            let physical = crate::action_summary::evaluate_with_mode(
                &layout,
                &corpus,
                &weights,
                &stop,
                &AtomicU64::new(0),
                "aklgg",
            )
            .unwrap()
            .score;
            assert!(
                (proposal.physical_score(&weights) - physical).abs()
                    < 1e-8 * (1.0 + physical.abs())
            );
            cache.commit(&mut proposal);
            compare(&cache.metrics(), &full_metrics(&layout, &corpus, &weights));
        }
        let identity: Vec<_> = (0..seed.slots.len()).collect();
        cache
            .restore(&identity, Some(&initial), &stop, |a, b, key| {
                effort.get(a, b, key)
            })
            .unwrap();
        compare(&cache.metrics(), &full_metrics(&seed, &corpus, &weights));
    }

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
                    if locked {
                        assert_eq!(candidate.arr[slot], slot);
                    }
                }
            }
        }
    }

    #[test]
    fn parallel_restart_groups_merge_exact_scores_progress_and_locks() {
        let seed = layout();
        let weights = Weights::default();
        let corpora = vec![(corpus("parallel.json", b"aa aq hr rh aqr qra! aa hr"), 1.0)];
        let locked = locks(&seed);
        let serial_settings = SearchSettings {
            workers: 1,
            restarts: 4,
            ..settings()
        };
        let serial = run_parallel(
            &seed,
            &seed,
            &corpora,
            &weights,
            &serial_settings,
            &locked,
            &Control::new(),
            &mut quiet,
        )
        .unwrap();
        assert!(serial.has_best);
        assert_eq!(serial.progress.restart, serial_settings.restarts);
        assert!(serial.progress.evaluations > 0);

        let settings = SearchSettings {
            workers: 2,
            restarts: 4,
            ..settings()
        };
        let mut reported = None;
        let mut callback = |snapshot: Snapshot, _: Metrics, raw: Raw, totals: [f64; 4]| {
            if snapshot.has_best {
                assert_eq!(
                    snapshot.best.score.to_bits(),
                    objective(&metrics_totals(&raw, &totals), &weights, &settings).to_bits()
                );
            }
            reported = Some(snapshot);
        };
        let result = run_parallel(
            &seed,
            &seed,
            &corpora,
            &weights,
            &settings,
            &locked,
            &Control::new(),
            &mut callback,
        )
        .unwrap();
        assert!(result.has_best);
        assert_eq!(result.progress.restart, settings.restarts);
        assert!(result.progress.evaluations > 0);
        let reported = reported.unwrap();
        assert_eq!(reported.best.arr, result.best.arr);
        assert_eq!(reported.best.score.to_bits(), result.best.score.to_bits());

        let effort = LocalEffort::new(&seed, &weights);
        let mut cache = ng::Incremental::new_with_policy(
            &corpora[0].0,
            &seed,
            Geometry::with_rolls(physical_keys(&seed), weights.rolls()),
            &AtomicBool::new(false),
            weights.alts(),
            weights.combo_policy(),
            |a, b, key| effort.get(a, b, key),
        )
        .unwrap();
        for candidate in std::iter::once(&result.best).chain(result.archive.iter()) {
            cache
                .restore(
                    &candidate.arr,
                    None,
                    &AtomicBool::new(false),
                    |a, b, key| effort.get(a, b, key),
                )
                .unwrap();
            assert_eq!(
                candidate.score.to_bits(),
                objective(&cache.metrics(), &weights, &settings).to_bits()
            );
            for (slot, &is_locked) in locked.iter().enumerate() {
                if is_locked {
                    assert_eq!(candidate.arr[slot], slot);
                }
            }
        }
        assert!(result.archive.len() <= settings.archive);
        for (i, candidate) in result.archive.iter().enumerate() {
            assert!(!result.archive[..i].iter().any(|other| same_visible(
                &seed,
                &other.arr,
                &candidate.arr
            )));
        }
    }

    #[test]
    fn configured_objectives_and_normalized_mixture_use_numeric_totals() {
        let seed = layout();
        let mut weights = Weights::default();
        weights.0[INSRAF] = 0.2;
        weights.0[OUTSRAF] = 0.7;
        weights.0[INROLL] = 0.3;
        weights.0[OUTROLL] = 0.9;
        let corpora = vec![
            (corpus("first.json", b"aa aq hr rh aqr qra"), 1.0),
            (corpus("second.json", b"hhh rrr ar ar qr"), 3.0),
        ];
        weights.2 = mana2_metrics::Weights::from_text("sfb = -23\n").unwrap();
        let locked = vec![true; seed.slots.len()];
        for mode in ["normal", "aklgg"] {
            let settings = SearchSettings {
                mode: mode.into(),
                ..settings()
            };
            let effort = LocalEffort::new(&seed, &weights);
            let expected: f64 = corpora
                .iter()
                .map(|(corpus, share)| {
                    let mut cache = ng::Incremental::new(
                        corpus,
                        &seed,
                        Geometry::new(physical_keys(&seed)),
                        &AtomicBool::new(false),
                        |a, b, key| effort.get(a, b, key),
                    )
                    .unwrap();
                    let score = if settings.uses_mana2_stats() {
                        cache.enable_mana2();
                        cache.physical_score(&weights)
                    } else {
                        objective(&cache.metrics(), &weights, &settings)
                    };
                    (share / 4.0) * score
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

        let mut parallel_settings = settings();
        parallel_settings.workers = 2;
        let error = run_parallel(
            &seed,
            &seed,
            &corpora,
            &weights,
            &parallel_settings,
            &locks(&seed),
            &control,
            &mut quiet,
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
                full: vec![None],
                full_pending: None,
                screening: false,
                soft_caps: false,
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
                Instant::now(),
                0,
                true,
                &[],
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

    #[test]
    fn letter_arrangements_see_through_refit_swaps_and_rules() {
        let seed = ak::Layout::parse(
            "q w e r t  y u i o p\na s d f g  h j k l ;\nz x c v b  @ m , . /\nthumbs: space\n@ hr\nswap h ar\n",
            Path::new("inline-letter-arrangement.dat"),
        )
        .unwrap();
        let slot = |layout: &ak::Layout, label: &str| {
            layout
                .slots
                .iter()
                .position(|slot| slot.label == label)
                .unwrap()
        };
        let mut arr: Vec<_> = (0..seed.slots.len()).collect();
        arr.swap(slot(&seed, "a"), slot(&seed, "e"));
        arr.swap(slot(&seed, "@"), slot(&seed, "q"));
        let moved = arrangement(&seed, &arr);
        // A refit drops the swap for another and changes the magic rules: the
        // keys type the same letters, so the arrangement is the same.
        let (bare, _) = crate::swap_fit::split(&moved);
        let swap = crate::swap_fit::Swap {
            context: b't',
            a: b'a',
            b: b'e',
        };
        let mut refit = crate::swap_fit::install(&bare, &[swap]);
        if let Some(ak::Action::Rules { rules, .. }) = refit.actions.get_mut("@") {
            rules.insert(b"i".to_vec(), ak::Emission::Text(b"o".to_vec()));
        }
        assert!(permutation(&seed, &refit).is_none());
        assert_eq!(letter_arrangement(&seed, &refit), Some(arr));
        let mut altered = refit;
        altered.slots[0].finger = 3;
        assert!(letter_arrangement(&seed, &altered).is_none());
    }
}
