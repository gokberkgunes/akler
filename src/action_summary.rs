//! Ranking totals without constructing the detailed physical histogram adapter.
use crate::action_keys as ak;

use crate::action_ngrams as ng;

use crate::*;

/// Weighted adjacent-pair penalties measured by `pair_movement_costs`.
const PAIR_MOVEMENT_METRICS: [usize; 12] = [
    SFB, SKB, LSB, DAFJB, CAFJB, DNFJB, CNFJB, DAHJB, CAHJB, DNHJB, CNHJB, SFTRAVEL,
];

/// Per-pair values before objective weighting: weighted AKLER penalties in
/// normal mode, the pair's Mana2 stats in physical modes.
type PlainPairCosts = BTreeMap<(Option<usize>, Option<usize>, (bool, bool, bool)), Arc<[f64]>>;

/// A pair, its pair corpus's windows' pressed keys and the layout policy.
type MappedPairKey = ([u8; 2], Vec<[u8; 5]>, (bool, bool, bool));

/// Mapped pair costs a `PhysicalTables` keeps before it starts over.
const MAPPED_PAIR_COSTS: usize = 1 << 16;

/// A layout's compiled numeric mapper, for mapping pair corpora.
/// Compiled layouts `mapped_pair_key` keeps (`Prepared::pair_programs`).
const PAIR_PROGRAMS: usize = 4;
/// One-pair corpora `pair_corpus` keeps: all 95 × 95 printable pairs.
const PAIR_CORPORA: usize = 95 * 95;

struct PairProgram {
    bindings: Vec<ak::Binding>,
    actions: BTreeMap<String, ak::Action>,
    program: crate::action_fast::Program,
    state: crate::action_fast::KeyState,
}

/// Return the only slots typing `pair` when its cost can depend on nothing
/// but those two physical keys (`None` for an untypeable byte): each byte has
/// at most one single-byte text binding,
/// alternate fingers are inactive, and no action can emit either
/// byte while typing the pair (no rule for the first byte, no fallback or
/// repeat producing either byte).
fn is_physical_mode(mode: &str) -> bool {
    mode == "aklgg"
}

fn plain_pair_slots(
    layout: &ak::Layout,
    weights: &Weights,
    pair: [u8; 2],
) -> Option<(Option<usize>, Option<usize>)> {
    // Pair corpora are mapped without combos (see `pair_movement_costs_mapped`),
    // so a layout's combos never change these costs.
    if pair[0] == pair[1] || weights.alts().active(layout) {
        return None;
    }
    let mut found = [None, None];
    for (index, slot) in layout.slots.iter().enumerate() {
        match &slot.binding {
            ak::Binding::Empty => {}
            ak::Binding::Text(text) if text.len() == 1 => {
                for (byte, found) in pair.iter().zip(&mut found) {
                    if text[0] == *byte {
                        if found.is_some() {
                            return None;
                        }
                        *found = Some(index);
                    }
                }
            }
            ak::Binding::Named(name) => match layout.actions.get(name)? {
                ak::Action::Inactive | ak::Action::RepeatOutput => {}
                ak::Action::Rules {
                    basis: ak::Basis::Text,
                    rules,
                    fallback,
                } => {
                    let fallback_safe = match fallback {
                        ak::Emission::Text(text) => !text.iter().any(|b| pair.contains(b)),
                        ak::Emission::Call(call) => {
                            matches!(layout.actions.get(call), Some(ak::Action::RepeatOutput))
                        }
                        ak::Emission::None => true,
                    };
                    if !fallback_safe
                        || rules.keys().any(|context| context.last() == Some(&pair[0]))
                    {
                        return None;
                    }
                }
                _ => return None,
            },
            _ => return None,
        }
    }
    Some((found[0], found[1]))
}

#[derive(Clone)]
pub(crate) struct Summary {
    pub(crate) raw: Raw,
    pub(crate) totals: [f64; 4],
    pub(crate) metrics: Metrics,
    pub(crate) score: f64,
    pub(crate) counts: Arc<ng::Counts>,
}

/// Immutable physical tables for one board and objective snapshot. Bindings and
/// action rules are deliberately excluded: every evaluator rebuilds its mapper.
pub(crate) struct PhysicalTables {
    slots: Vec<ak::Slot>,
    row_stagger: ak::RowStagger,
    pair_corpora: std::sync::Mutex<BTreeMap<[u8; 2], Arc<ng::NgramCorpus>>>,
    /// Pair costs keyed by the two pressed slots and layout policy, valid only
    /// when no action can type either character (see `plain_pair_slots`).
    slot_pair_costs: std::sync::Mutex<PlainPairCosts>,
    /// Pair costs of layouts actions can type a pair on, keyed by the keys
    /// its pair corpus presses (`Prepared::mapped_pair_key`): the counts, and
    /// so the costs, follow from those keys alone.
    mapped_pair_costs: std::sync::Mutex<HashMap<MappedPairKey, Arc<[f64]>>>,
    weight_bits: [u64; N_WEIGHTS],
    rolls: RollSettings,
    /// Scoring mode and, for mana/aklgg, its physical weights.
    pub(crate) mode: String,
    physical_weights: Option<mana2_metrics::Weights>,
    pub(crate) effort: crate::action_ui::LocalEffort,
    finite_effort: bool,
    pub(crate) geometry: Arc<Geometry>,
}

impl PhysicalTables {
    pub(crate) fn new(layout: &ak::Layout, weights: &Weights) -> Arc<Self> {
        Self::with_mode(layout, weights, "normal")
    }

    /// Tables for one search mode; physical modes score Mana2 stats.
    pub(crate) fn with_mode(layout: &ak::Layout, weights: &Weights, mode: &str) -> Arc<Self> {
        let effort = crate::action_ui::LocalEffort::new(layout, weights);
        let physical = is_physical_mode(mode);
        Arc::new(Self {
            mode: mode.into(),
            physical_weights: physical.then(|| weights.2.clone()),
            slots: layout.slots.clone(),
            row_stagger: layout.row_stagger,
            pair_corpora: std::sync::Mutex::new(BTreeMap::new()),
            slot_pair_costs: std::sync::Mutex::new(BTreeMap::new()),
            mapped_pair_costs: std::sync::Mutex::new(HashMap::new()),
            weight_bits: weights.0.map(f64::to_bits),
            rolls: weights.rolls(),
            finite_effort: effort.all_finite(),
            effort,
            geometry: Arc::new(Geometry::with_rolls(
                crate::action_ui::physical_keys(layout),
                weights.rolls(),
            )),
        })
    }

    pub(crate) fn matches(&self, layout: &ak::Layout, weights: &Weights) -> bool {
        layout.row_stagger == self.row_stagger
            && weights.0.map(f64::to_bits) == self.weight_bits
            && self
                .physical_weights
                .as_ref()
                .is_none_or(|physical| physical == &weights.2)
            && weights.rolls() == self.rolls
            && layout.slots.len() == self.slots.len()
            && layout
                .slots
                .iter()
                .zip(&self.slots)
                .all(|(slot, original)| same_physical_slot(slot, original))
    }
}

/// Equal slots but for their bindings and labels, compared field by field so
/// that no slot is cloned.
fn same_physical_slot(a: &ak::Slot, b: &ak::Slot) -> bool {
    let ak::Slot {
        binding: _,
        label: _,
        row,
        col,
        row_offset,
        column_offset,
        finger,
        rank,
        hand,
        main,
    } = a;
    (
        *row,
        *col,
        *row_offset,
        *column_offset,
        *finger,
        *rank,
        *hand,
        *main,
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
}

// Compare JSON structurally, including floating point bit patterns, without
// relying on a hash or changing the application's general JSON representation.
fn same_json(a: &Json, b: &Json) -> bool {
    match (a, b) {
        (Json::Object(a), Json::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .zip(b)
                    .all(|((ak, av), (bk, bv))| ak == bk && same_json(av, bv))
        }
        (Json::Array(a), Json::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same_json(a, b))
        }
        (Json::String(a), Json::String(b)) => a == b,
        (Json::Number(a), Json::Number(b)) => a.to_bits() == b.to_bits(),
        (Json::Bool(a), Json::Bool(b)) => a == b,
        (Json::Null, Json::Null) => true,
        _ => false,
    }
}

pub(crate) struct Prepared<'a> {
    corpus: &'a ng::NgramCorpus,
    weights: &'a Weights,
    slots: Vec<ak::Slot>,
    physical: Arc<PhysicalTables>,
    alt_fingers: Vec<crate::alt_fingers::Rule>,
    combos: Vec<Json>,
    layout_policy: (bool, bool, bool),
    rule_cache: std::sync::Mutex<Option<Arc<crate::action_rule_eval::Cache>>>,
    /// The first fully mapped layout's mappings, reused by later `evaluate`
    /// calls the rule cache cannot serve (combos, adaptive keys).
    mappings: std::sync::Mutex<Option<Arc<ng::Mappings>>>,
    #[cfg(test)]
    rule_work: std::sync::Mutex<Option<crate::action_rule_eval::Work>>,
    // FIFO, scoped to this fixed corpus, objective, bindings and metadata.
    summaries:
        std::sync::Mutex<std::collections::VecDeque<(BTreeMap<String, ak::Action>, Summary)>>,
    /// Physical modes: local objective slopes per Mana2 stat, taken from the
    /// first layout whose pair costs are requested.
    pair_slopes: std::sync::OnceLock<[f64; mana2_metrics::N_STATS]>,
    /// The layouts `mapped_pair_key` last compiled, most recent first. Rule
    /// screening alternates the current dictionary with one-rule variants,
    /// so the current one stays compiled.
    pair_programs: std::sync::Mutex<std::collections::VecDeque<Arc<PairProgram>>>,
    /// `mappings`' raw counters, totals and counts, for `screen`.
    screen_base: std::sync::OnceLock<Option<(Raw, [f64; 4], ng::Counts)>>,
}

impl<'a> Prepared<'a> {
    pub(crate) fn new(
        layout: &ak::Layout,
        corpus: &'a ng::NgramCorpus,
        weights: &'a Weights,
    ) -> Self {
        Self::with_physical(
            layout,
            corpus,
            weights,
            PhysicalTables::new(layout, weights),
        )
        .expect("fresh physical tables match their source layout")
    }

    pub(crate) fn with_physical(
        layout: &ak::Layout,
        corpus: &'a ng::NgramCorpus,
        weights: &'a Weights,
        physical: Arc<PhysicalTables>,
    ) -> ak::Result<Self> {
        if !physical.matches(layout, weights) {
            return Err(
                "prepared action summary requires unchanged physical slots and weights".into(),
            );
        }
        Ok(Self {
            corpus,
            weights,
            slots: layout.slots.clone(),
            physical,
            alt_fingers: layout.alt_fingers.clone(),
            combos: layout.combos.clone(),
            layout_policy: (layout.left_outer, layout.right_outer, layout.action_mode),
            summaries: std::sync::Mutex::new(std::collections::VecDeque::new()),
            rule_cache: std::sync::Mutex::new(None),
            mappings: std::sync::Mutex::new(None),
            #[cfg(test)]
            rule_work: std::sync::Mutex::new(None),
            pair_slopes: std::sync::OnceLock::new(),
            pair_programs: std::sync::Mutex::new(std::collections::VecDeque::new()),
            screen_base: std::sync::OnceLock::new(),
        })
    }

    fn validate(
        &self,
        layout: &ak::Layout,
        stop: &AtomicBool,
        binding_variation: bool,
    ) -> ak::Result<()> {
        let same_geometry = self.physical.matches(layout, self.weights);
        if !same_geometry || (!binding_variation && layout.slots != self.slots) {
            return Err(
                "prepared action summary requires unchanged physical slots and bindings".into(),
            );
        }
        if stop.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        Ok(())
    }

    fn memo_metadata_matches(&self, layout: &ak::Layout) -> bool {
        layout.alt_fingers == self.alt_fingers
            && (layout.left_outer, layout.right_outer, layout.action_mode) == self.layout_policy
            && layout.combos.len() == self.combos.len()
            && layout
                .combos
                .iter()
                .zip(&self.combos)
                .all(|(a, b)| same_json(a, b))
    }

    fn map_counts(
        &self,
        layout: &ak::Layout,
        corpus: &ng::NgramCorpus,
        stop: &AtomicBool,
        progress: &AtomicU64,
        binding_variation: bool,
    ) -> ak::Result<Arc<ng::Counts>> {
        let policy = self.weights.combo_policy();
        self.map_counts_with_combos(layout, corpus, stop, progress, binding_variation, policy)
    }

    fn map_counts_with_combos(
        &self,
        layout: &ak::Layout,
        corpus: &ng::NgramCorpus,
        stop: &AtomicBool,
        progress: &AtomicU64,
        binding_variation: bool,
        combos: crate::combos::Policy,
    ) -> ak::Result<Arc<ng::Counts>> {
        self.validate(layout, stop, binding_variation)?;
        Ok(Arc::new(if self.physical.finite_effort {
            corpus.evaluate_with_finite_policy(
                layout,
                stop,
                progress,
                self.weights.alts(),
                combos,
                |a, b, key| self.physical.effort.get(a, b, key),
            )?
        } else {
            corpus.evaluate_with_policy(
                layout,
                stop,
                progress,
                self.weights.alts(),
                combos,
                |a, b, key| self.physical.effort.get(a, b, key),
            )?
        }))
    }

    /// `map_counts` from `base`'s mappings where they still hold, returning
    /// this layout's mappings when `record` is set.
    fn map_counts_reusing(
        &self,
        layout: &ak::Layout,
        stop: &AtomicBool,
        progress: &AtomicU64,
        binding_variation: bool,
        base: Option<&ng::Mappings>,
        record: bool,
        delta: bool,
    ) -> ak::Result<(Arc<ng::Counts>, Option<ng::Mappings>)> {
        self.validate(layout, stop, binding_variation)?;
        let flags = (
            self.weights.alts(),
            self.weights.combo_policy(),
            self.physical.finite_effort,
        );
        let (counts, mappings) = self.corpus.evaluate_reusing(
            layout,
            stop,
            progress,
            flags,
            base,
            record,
            delta,
            |a, b, key| self.physical.effort.get(a, b, key),
        )?;
        Ok((Arc::new(counts), mappings))
    }

    /// `evaluate_bindings` reusing `base`'s mappings, also returning this
    /// layout's for the next call. Fitters pass their current layout's; only
    /// the windows a change touches are counted again, so scores can differ
    /// from a fresh evaluation in the last bits.
    pub(crate) fn evaluate_from(
        &self,
        layout: &ak::Layout,
        base: Option<&ng::Mappings>,
        stop: &AtomicBool,
        progress: &AtomicU64,
    ) -> ak::Result<(Summary, Option<ng::Mappings>)> {
        let (counts, mappings) =
            self.map_counts_reusing(layout, stop, progress, true, base, true, true)?;
        let summary = from_counts_with_geometry(
            layout,
            counts,
            self.weights,
            &self.physical.mode,
            &self.physical.geometry,
        )?;
        Ok((summary, mappings))
    }

    /// `evaluate_from(layout, None)`'s result, mapping again only the windows
    /// that changed from `start` where whole-number weights make the counts
    /// the same.
    pub(crate) fn evaluate_fresh(
        &self,
        layout: &ak::Layout,
        start: Option<&ng::Mappings>,
        stop: &AtomicBool,
        progress: &AtomicU64,
    ) -> ak::Result<(Summary, Option<ng::Mappings>)> {
        let exact = self
            .corpus
            .exact_sums_for(layout, self.weights.combo_policy(), self.weights.alts());
        self.evaluate_from(layout, start.filter(|_| exact), stop, progress)
    }

    /// `evaluate_from` with a fresh evaluation's result: windows `base`
    /// proves unchanged reuse their mapped keys, and only a corpus with exact
    /// sums counts just the changed windows again.
    pub(crate) fn evaluate_exact_from(
        &self,
        layout: &ak::Layout,
        base: Option<&ng::Mappings>,
        stop: &AtomicBool,
        progress: &AtomicU64,
    ) -> ak::Result<(Summary, Option<ng::Mappings>)> {
        let (counts, mappings) =
            self.map_counts_reusing(layout, stop, progress, true, base, true, false)?;
        let summary = from_counts_with_geometry(
            layout,
            counts,
            self.weights,
            &self.physical.mode,
            &self.physical.geometry,
        )?;
        Ok((summary, mappings))
    }

    /// `evaluate`'s summary of a fitter's trial, screened: the recorded
    /// mappings' counts plus just the windows `layout` changes, added to their
    /// raw counters, so sums differ from `evaluate`'s in the last bits. Its
    /// tables hold only the differences. A fitter keeps its decisions exact by
    /// evaluating every trial it may keep or that lands this close to a
    /// threshold. None where `evaluate` has no recorded mappings to reuse.
    pub(crate) fn screen(
        &self,
        layout: &ak::Layout,
        stop: &AtomicBool,
        progress: &AtomicU64,
    ) -> ak::Result<Option<Summary>> {
        self.validate(layout, stop, false)?;
        let Some(base) = self.mappings.lock().unwrap().clone() else {
            return Ok(None);
        };
        let mana2 = self.physical.mode == "aklgg";
        let screen_base = self.screen_base.get_or_init(|| {
            base.counts().map(|counts| {
                let (raw, totals) = ng::raw_from_counts_with_geometry(
                    layout,
                    &counts,
                    mana2,
                    &self.physical.geometry,
                );
                (raw, totals, counts)
            })
        });
        let Some((base_raw, base_totals, base_counts)) = screen_base else {
            return Ok(None);
        };
        self.screen_changes(
            layout,
            &base,
            base_raw,
            *base_totals,
            base_counts,
            stop,
            progress,
        )
    }

    /// `screen` for a trial of bindings or combos from `base`, the mappings
    /// `evaluate_from` returned with `summary`.
    pub(crate) fn screen_from(
        &self,
        layout: &ak::Layout,
        base: &ng::Mappings,
        summary: &Summary,
        stop: &AtomicBool,
        progress: &AtomicU64,
    ) -> ak::Result<Option<Summary>> {
        self.validate(layout, stop, true)?;
        self.screen_changes(
            layout,
            base,
            &summary.raw,
            summary.totals,
            &summary.counts,
            stop,
            progress,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn screen_changes(
        &self,
        layout: &ak::Layout,
        base: &ng::Mappings,
        base_raw: &Raw,
        base_totals: [f64; 4],
        base_counts: &ng::Counts,
        stop: &AtomicBool,
        progress: &AtomicU64,
    ) -> ak::Result<Option<Summary>> {
        let mana2 = self.physical.mode == "aklgg";
        let flags = (
            self.weights.alts(),
            self.weights.combo_policy(),
            self.physical.finite_effort,
        );
        let Some(mut counts) =
            self.corpus
                .count_changes(layout, stop, progress, flags, base, |a, b, key| {
                    self.physical.effort.get(a, b, key)
                })?
        else {
            return Ok(None);
        };
        let changes =
            ng::raw_from_counts_with_geometry(layout, &counts, mana2, &self.physical.geometry);
        let (mut raw, mut totals) = (base_raw.clone(), base_totals);
        ng::add_scaled(&mut raw, &mut totals, &changes, 1.0);
        counts.characters += base_counts.characters;
        counts.presses += base_counts.presses;
        counts.action_presses += base_counts.action_presses;
        counts.ignored_characters += base_counts.ignored_characters;
        counts.speed_populations = base_counts.speed_populations;
        summary_from_raw(Arc::new(counts), raw, totals, self.weights, mana2).map(Some)
    }

    pub(crate) fn physical_mode(&self) -> bool {
        is_physical_mode(&self.physical.mode)
    }

    /// True when no pair movement cost can be negative. A pair whose cost
    /// without a rule is zero then cannot gain from one. Physical modes are
    /// decided once their slopes are known, so request a pair cost first.
    pub(crate) fn pair_movement_costs_nonnegative(&self) -> bool {
        if self.physical_mode() {
            return self
                .pair_slopes
                .get()
                .is_some_and(|slopes| slopes.iter().all(|&slope| slope >= 0.0));
        }
        PAIR_MOVEMENT_METRICS
            .iter()
            .all(|&metric| self.weights.0[metric] >= 0.0)
    }

    /// Fix physical-mode pair slopes at `layout` before screening, so the
    /// zero-cost test in `pair_movement_costs_nonnegative` can apply.
    pub(crate) fn prepare_pair_costs(
        &self,
        layout: &ak::Layout,
        stop: &AtomicBool,
        progress: &AtomicU64,
    ) -> ak::Result<()> {
        if self.physical_mode() {
            self.pair_slopes(layout, stop, progress, false)?;
        }
        Ok(())
    }

    /// Slopes of the lower-is-better physical score at `layout`'s stats.
    fn pair_slopes(
        &self,
        layout: &ak::Layout,
        stop: &AtomicBool,
        progress: &AtomicU64,
        binding_variation: bool,
    ) -> ak::Result<&[f64; mana2_metrics::N_STATS]> {
        if let Some(slopes) = self.pair_slopes.get() {
            return Ok(slopes);
        }
        let summary = if binding_variation {
            let counts = self.map_counts(layout, self.corpus, stop, progress, true)?;
            from_counts_with_geometry(
                layout,
                counts,
                self.weights,
                &self.physical.mode,
                &self.physical.geometry,
            )?
        } else {
            self.evaluate(layout, stop, progress)?
        };
        let stats = ng::mana_stats(
            summary.raw.1.as_ref().ok_or("physical stats are missing")?,
            summary.totals,
            summary.counts.speed_populations,
        );
        Ok(self
            .pair_slopes
            .get_or_init(|| self.weights.physical_slopes(&stats)))
    }

    /// Measure only the weighted adjacent-pair penalties for one source-text
    /// pair. In mana/aklgg modes the pair's Mana2 stats are weighted by the
    /// objective's local slopes (a linear estimate) and reported under SFB. The normal action mapper still chooses the winning physical
    /// presses, including alternate fingers and combos, while this prepared
    /// evaluator reuses its physical effort and geometry tables.
    pub(crate) fn pair_movement_costs(
        &self,
        layout: &ak::Layout,
        pair: [u8; 2],
        stop: &AtomicBool,
        progress: &AtomicU64,
    ) -> ak::Result<[f64; N_METRICS]> {
        if stop.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        self.pair_movement_costs_inner(layout, pair, stop, progress, false)
    }

    /// Reuse physical tables when only slot bindings and labels have moved.
    pub(crate) fn pair_movement_costs_binding_variation(
        &self,
        layout: &ak::Layout,
        pair: [u8; 2],
        stop: &AtomicBool,
        progress: &AtomicU64,
    ) -> ak::Result<[f64; N_METRICS]> {
        self.pair_movement_costs_inner(layout, pair, stop, progress, true)
    }

    fn pair_movement_costs_inner(
        &self,
        layout: &ak::Layout,
        pair: [u8; 2],
        stop: &AtomicBool,
        progress: &AtomicU64,
        binding_variation: bool,
    ) -> ak::Result<[f64; N_METRICS]> {
        let plain = plain_pair_slots(layout, self.weights, pair).map(|(first, second)| {
            (
                first,
                second,
                (layout.left_outer, layout.right_outer, layout.action_mode),
            )
        });
        let cached = match plain {
            Some(key) => {
                self.validate(layout, stop, binding_variation)?;
                self.physical
                    .slot_pair_costs
                    .lock()
                    .unwrap()
                    .get(&key)
                    .cloned()
            }
            None => None,
        };
        let mapped = match (plain, cached.is_some()) {
            (None, false) => self.mapped_pair_key(layout, pair, stop, binding_variation)?,
            _ => None,
        };
        let cached = cached.or_else(|| {
            let key = mapped.as_ref()?;
            self.physical
                .mapped_pair_costs
                .lock()
                .unwrap()
                .get(key)
                .cloned()
        });
        let values = match cached {
            Some(values) => values,
            None => {
                let values: Arc<[f64]> = self
                    .pair_movement_costs_mapped(layout, pair, stop, progress, binding_variation)?
                    .into();
                if let Some(key) = plain {
                    self.physical
                        .slot_pair_costs
                        .lock()
                        .unwrap()
                        .insert(key, Arc::clone(&values));
                }
                if let Some(key) = mapped {
                    let mut costs = self.physical.mapped_pair_costs.lock().unwrap();
                    if costs.len() == MAPPED_PAIR_COSTS {
                        costs.clear();
                    }
                    costs.insert(key, Arc::clone(&values));
                }
                values
            }
        };
        let mut costs = [0.0; N_METRICS];
        if self.physical_mode() {
            let slopes = self.pair_slopes(layout, stop, progress, binding_variation)?;
            costs[SFB] = slopes
                .iter()
                .zip(values.iter())
                .map(|(slope, value)| slope * value)
                .sum();
        } else {
            costs.copy_from_slice(&values);
        }
        Ok(costs)
    }

    /// The key `pair`'s costs on `layout` follow from: the keys each window
    /// of its pair corpus presses, mapped as `pair_movement_costs_mapped`
    /// maps them. None where contextual fingers apply or the numeric mapper
    /// cannot take the layout.
    fn mapped_pair_key(
        &self,
        layout: &ak::Layout,
        pair: [u8; 2],
        stop: &AtomicBool,
        binding_variation: bool,
    ) -> ak::Result<Option<MappedPairKey>> {
        if self.weights.alts().active(layout) {
            return Ok(None);
        }
        self.validate(layout, stop, binding_variation)?;
        let corpus = self.pair_corpus(pair)?;
        let mut compiled = self.pair_programs.lock().unwrap();
        let found = compiled.iter().position(|program| {
            program.actions == layout.actions
                && program
                    .bindings
                    .iter()
                    .zip(&layout.slots)
                    .all(|(binding, slot)| *binding == slot.binding)
        });
        let program = match found {
            Some(index) => {
                let program = compiled.remove(index).unwrap();
                compiled.push_front(Arc::clone(&program));
                program
            }
            None => {
                let Ok(program) = crate::action_fast::Program::new(layout, corpus.order()) else {
                    return Ok(None);
                };
                let state = crate::action_fast::KeyState::new(&program);
                let program = Arc::new(PairProgram {
                    bindings: layout
                        .slots
                        .iter()
                        .map(|slot| slot.binding.clone())
                        .collect(),
                    actions: layout.actions.clone(),
                    program,
                    state,
                });
                compiled.truncate(PAIR_PROGRAMS - 1);
                compiled.push_front(Arc::clone(&program));
                program
            }
        };
        drop(compiled);
        let keys = corpus.plain_window_keys(
            &program.program,
            &program.state,
            self.physical.finite_effort,
            |a, b, key| self.physical.effort.get(a, b, key),
        )?;
        Ok(Some((
            pair,
            keys,
            (layout.left_outer, layout.right_outer, layout.action_mode),
        )))
    }

    /// The one-pair corpus `pair_movement_costs_mapped` maps.
    fn pair_corpus(&self, pair: [u8; 2]) -> ak::Result<Arc<ng::NgramCorpus>> {
        Ok({
            let mut corpora = self.physical.pair_corpora.lock().unwrap();
            if let Some(corpus) = corpora.get(&pair) {
                Arc::clone(corpus)
            } else {
                let letters = if pair[0] == pair[1] {
                    format!("{}:2", ak::quote(&pair[..1]))
                } else {
                    format!("{}:1,{}:1", ak::quote(&pair[..1]), ak::quote(&pair[1..]))
                };
                let corpus = ng::NgramCorpus::from_text(
                    &format!(
                        "{{\"letters\":{{{letters}}},\"bigrams\":{{{}:1}},\"trigrams\":{{}}}}",
                        ak::quote(&pair)
                    ),
                    Path::new("magic-direct-pair.json"),
                )?;
                let corpus = Arc::new(corpus);
                // Bound search memory even for arbitrary source bytes. Hold
                // every printable pair: screening walks pairs in order, so a
                // smaller map would evict each new pair before its reuse.
                if corpora.len() == PAIR_CORPORA {
                    corpora.pop_first();
                }
                corpora.insert(pair, Arc::clone(&corpus));
                corpus
            }
        })
    }

    fn pair_movement_costs_mapped(
        &self,
        layout: &ak::Layout,
        pair: [u8; 2],
        stop: &AtomicBool,
        progress: &AtomicU64,
        binding_variation: bool,
    ) -> ak::Result<Vec<f64>> {
        let corpus = self.pair_corpus(pair)?;
        // A two-character pair corpus cannot hold the order-5 context chord
        // recognition needs, so this screening estimate ignores combos. Exact
        // evaluations of fitted dictionaries still apply them.
        let counts = self.map_counts_with_combos(
            layout,
            &corpus,
            stop,
            progress,
            binding_variation,
            crate::combos::Policy::OFF,
        )?;
        let physical = self.physical_mode();
        let (raw, totals) =
            ng::raw_from_counts_with_geometry(layout, &counts, physical, &self.physical.geometry);
        if let Some(stats) = raw.1.as_ref().filter(|_| physical) {
            return Ok(mana2_metrics::stats(stats, totals).values.to_vec());
        }
        let mut costs = vec![0.0; N_METRICS];
        for metric in PAIR_MOVEMENT_METRICS {
            costs[metric] = raw.0[metric] * self.weights.0[metric];
        }
        Ok(costs)
    }

    pub(crate) fn evaluate(
        &self,
        layout: &ak::Layout,
        stop: &AtomicBool,
        progress: &AtomicU64,
    ) -> ak::Result<Summary> {
        // Cancellation and identity checks must also run for cache hits.
        self.validate(layout, stop, false)?;
        let memoize = self.memo_metadata_matches(layout);
        if memoize {
            let summaries = self.summaries.lock().unwrap();
            if let Some((_, summary)) = summaries
                .iter()
                .find(|(actions, _)| *actions == layout.actions)
            {
                return Ok(summary.clone());
            }
        }
        // Mapping can be expensive; allow concurrent misses without holding
        // either cache lock. A rule cache keeps one immutable baseline per
        // Prepared, and reconstructs complete counts in canonical window order.
        #[cfg(test)]
        {
            *self.rule_work.lock().unwrap() = None;
        }
        let mut new_rule_cache = None;
        // With exact sums, recorded mappings recount only the windows a
        // layout changes, which the rule cache cannot.
        let cached_counts = if memoize && !self.corpus.exact_sums() {
            let baseline = self.rule_cache.lock().unwrap().clone();
            if let Some(baseline) = baseline {
                baseline
                    .evaluate(
                        layout,
                        stop,
                        progress,
                        self.weights.alts(),
                        self.weights.4,
                        self.physical.finite_effort,
                        |a, b, key| self.physical.effort.get(a, b, key),
                    )?
                    .map(|(counts, _work)| {
                        #[cfg(test)]
                        {
                            *self.rule_work.lock().unwrap() = Some(_work);
                        }
                        counts
                    })
            } else {
                crate::action_rule_eval::Cache::new(
                    self.corpus,
                    layout,
                    stop,
                    progress,
                    self.weights.alts(),
                    self.weights.4,
                    self.physical.finite_effort,
                    |a, b, key| self.physical.effort.get(a, b, key),
                )?
                .map(|(cache, counts)| {
                    #[cfg(test)]
                    {
                        *self.rule_work.lock().unwrap() = Some(crate::action_rule_eval::Work {
                            mapped_windows: cache.len(),
                            total_windows: cache.len(),
                        });
                    }
                    new_rule_cache = Some(Arc::new(cache));
                    counts
                })
            }
        } else {
            None
        };
        let summary = match cached_counts {
            Some(counts) => from_counts_with_geometry(
                layout,
                Arc::new(counts),
                self.weights,
                &self.physical.mode,
                &self.physical.geometry,
            )?,
            None => {
                let base = self.mappings.lock().unwrap().clone();
                let (counts, mappings) = self.map_counts_reusing(
                    layout,
                    stop,
                    progress,
                    false,
                    base.as_deref(),
                    base.is_none(),
                    false,
                )?;
                let summary = from_counts_with_geometry(
                    layout,
                    counts,
                    self.weights,
                    &self.physical.mode,
                    &self.physical.geometry,
                )?;
                if let Some(mappings) = mappings {
                    self.mappings
                        .lock()
                        .unwrap()
                        .get_or_insert(Arc::new(mappings));
                }
                summary
            }
        };
        if let Some(cache) = new_rule_cache {
            let mut baseline = self.rule_cache.lock().unwrap();
            if baseline.is_none() {
                *baseline = Some(cache);
            }
        }
        if memoize {
            let mut summaries = self.summaries.lock().unwrap();
            if !summaries
                .iter()
                .any(|(actions, _)| *actions == layout.actions)
            {
                if summaries.len() == 4 {
                    summaries.pop_front();
                }
                summaries.push_back((layout.actions.clone(), summary.clone()));
            }
        }
        Ok(summary)
    }
}

pub(crate) fn evaluate(
    layout: &ak::Layout,
    corpus: &ng::NgramCorpus,
    weights: &Weights,
    stop: &AtomicBool,
    progress: &AtomicU64,
) -> ak::Result<Summary> {
    evaluate_with_mode(layout, corpus, weights, stop, progress, "detailed")
}

pub(crate) fn evaluate_with_mode(
    layout: &ak::Layout,
    corpus: &ng::NgramCorpus,
    weights: &Weights,
    stop: &AtomicBool,
    progress: &AtomicU64,
    mode: &str,
) -> ak::Result<Summary> {
    evaluate_with_geometry(layout, corpus, weights, stop, progress, mode, None)
}

/// A ranker load can reuse its physical geometry across matching layouts.
pub(crate) fn evaluate_with_geometry(
    layout: &ak::Layout,
    corpus: &ng::NgramCorpus,
    weights: &Weights,
    stop: &AtomicBool,
    progress: &AtomicU64,
    mode: &str,
    geometry: Option<&Geometry>,
) -> ak::Result<Summary> {
    let mut timing = load_profile::LoadProfile::new("action ranking summary");
    let effort = action_ui::LocalEffort::new(layout, weights);
    timing.mark("Physical effort tables");

    let counts = Arc::new(if effort.all_finite() {
        corpus.evaluate_with_finite_policy(
            layout,
            stop,
            progress,
            weights.alts(),
            weights.combo_policy(),
            |a, b, key| effort.get(a, b, key),
        )?
    } else {
        corpus.evaluate_with_policy(
            layout,
            stop,
            progress,
            weights.alts(),
            weights.combo_policy(),
            |a, b, key| effort.get(a, b, key),
        )?
    });
    timing.mark("Physical mapping and transient histograms (nested report)");

    let owned_geometry;
    let geometry = match geometry {
        Some(geometry) => geometry,
        None => {
            owned_geometry =
                Geometry::with_rolls(crate::action_ui::physical_keys(layout), weights.rolls());
            &owned_geometry
        }
    };
    let summary = from_counts_with_geometry(layout, counts, weights, mode, geometry)?;
    timing.mark("Physical geometry, raw metrics and score");
    Ok(summary)
}

fn from_counts_with_geometry(
    layout: &ak::Layout,
    counts: Arc<ng::Counts>,
    weights: &Weights,
    mode: &str,
    geometry: &Geometry,
) -> ak::Result<Summary> {
    if counts.presses == 0.0 {
        return Err("no decoded keypresses".into());
    }

    let n = layout.slots.len();
    // Match the detailed evaluator's supported key count and error. Its ASCII
    // surrogate labels are assigned in increasing physical-slot order, so its
    // canonical IDs and original positions are both the identity permutation.
    let adapter_capacity = (33u8..=126)
        .filter(|byte| !byte.is_ascii_alphabetic())
        .count();
    if n > adapter_capacity {
        return Err("too many keys for the existing metric adapter".into());
    }

    let mana2 = mode == "aklgg";
    // Counts without contextual fingers retain the old physical-table path
    // byte for byte; active rules use the effective finger attached to each
    // distinct physical pattern.
    let (raw, totals) = ng::raw_from_counts_with_geometry(layout, &counts, mana2, geometry);
    summary_from_raw(counts, raw, totals, weights, mana2)
}

/// A summary from `counts`' raw counters and totals.
fn summary_from_raw(
    counts: Arc<ng::Counts>,
    raw: Raw,
    totals: [f64; 4],
    weights: &Weights,
    mana2: bool,
) -> ak::Result<Summary> {
    if counts.presses == 0.0 {
        return Err("no decoded keypresses".into());
    }
    if totals[0] <= 0.0 || totals[1] <= 0.0 {
        return Err("layout has no usable letters/bigrams in this corpus".into());
    }

    let metrics = metrics_totals(&raw, &totals);
    let score = if mana2 {
        let stats = ng::mana_stats(raw.1.as_ref().unwrap(), totals, counts.speed_populations);
        weights.physical_score(&stats)
    } else {
        breakdown(&metrics, weights).net
    };
    Ok(Summary {
        raw,
        totals,
        metrics,
        score,
        counts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus(order: usize) -> ng::NgramCorpus {
        // Decimal fractions exercise the adapter's exact summation order.
        scaled_corpus(order, 0.17)
    }

    fn scaled_corpus(order: usize, scale: f64) -> ng::NgramCorpus {
        let tables = ak::text_ngrams(b"i'i'a quu qqqqu qxuqu hnr thqe h!nr qu\0qu aan aap y,u ");
        let names = ["letters", "bigrams", "trigrams", "fourgrams", "fivegrams"];
        let fields: Vec<_> = (0..order)
            .map(|index| {
                let entries: Vec<_> = tables[index]
                    .iter()
                    .map(|(gram, count)| format!("{}:{}", ak::quote(gram), *count as f64 * scale))
                    .collect();
                format!("\"{}\":{{{}}}", names[index], entries.join(","))
            })
            .collect();
        ng::NgramCorpus::from_text(
            &format!("{{{}}}", fields.join(",")),
            Path::new("summary-inline.json"),
        )
        .unwrap()
    }

    #[test]
    fn screened_rule_trials_match_evaluated_ones() {
        let stop = AtomicBool::new(false);
        let progress = AtomicU64::new(0);
        let weights = Weights::default();
        let Json::Array(combos) = crate::parse_json(
            r#"[{"keys":["q","p"],"output":"th"},{"keys":["s","x"],"output":"qu"}]"#,
        )
        .unwrap() else {
            unreachable!()
        };
        for scale in [1.0, 0.17] {
            let corpus = scaled_corpus(5, scale);
            for mode in ["normal", "aklgg"] {
                let mut layout = ak::Layout::parse(
                    crate::action_keys::test_layouts::COMPACT,
                    Path::new("screen.dat"),
                )
                .unwrap();
                layout.combos = combos.clone();
                let physical = PhysicalTables::with_mode(&layout, &weights, mode);
                let prepared =
                    Prepared::with_physical(&layout, &corpus, &weights, physical).unwrap();
                prepared.evaluate(&layout, &stop, &progress).unwrap();
                let Some(ak::Action::Rules { rules, .. }) = layout.actions.get("*").cloned() else {
                    panic!("magic rules")
                };
                let mut screened = 0;
                // Rule edits as a fitter tries them: removals, changed and new
                // outputs, each from the recorded dictionary.
                let mut trials: Vec<BTreeMap<Vec<u8>, ak::Emission>> = rules
                    .keys()
                    .map(|context| {
                        let mut rules = rules.clone();
                        rules.remove(context);
                        rules
                    })
                    .collect();
                for (context, output) in [(b'q', b'u'), (b'a', b'n'), (b'h', b'q'), (b'i', b'a')] {
                    let mut edited = rules.clone();
                    edited.insert(vec![context], ak::Emission::Text(vec![output]));
                    trials.push(edited);
                }
                for trial_rules in trials {
                    let mut trial = layout.clone();
                    if let Some(ak::Action::Rules { rules, .. }) = trial.actions.get_mut("*") {
                        *rules = trial_rules;
                    }
                    let exact = prepared.evaluate(&trial, &stop, &progress).unwrap();
                    let Some(fast) = prepared.screen(&trial, &stop, &progress).unwrap() else {
                        continue;
                    };
                    screened += 1;
                    let close =
                        |a: f64, b: f64| (a - b).abs() <= 1e-12 * (1.0 + a.abs().max(b.abs()));
                    let context = format!("scale {scale}, {mode}");
                    assert!(
                        close(fast.score, exact.score),
                        "{context}: {} vs {}",
                        fast.score,
                        exact.score
                    );
                    for (a, b) in fast.metrics.v.iter().zip(&exact.metrics.v) {
                        assert!(close(*a, *b), "{context}: metric {a} vs {b}");
                    }
                    assert_eq!(
                        fast.counts.ignored_characters.to_bits(),
                        exact.counts.ignored_characters.to_bits(),
                        "{context}"
                    );
                }
                // Whole counts record mappings with their counts.
                assert_eq!(screened > 0, scale == 1.0, "scale {scale}, {mode}");
            }
        }
    }

    #[test]
    fn exact_evaluations_from_earlier_mappings_match_fresh_ones() {
        let stop = AtomicBool::new(false);
        let progress = AtomicU64::new(0);
        let weights = Weights::default();
        let combos = |definitions: &str| {
            let Json::Array(combos) = crate::parse_json(definitions).unwrap() else {
                unreachable!()
            };
            combos
        };
        // Whole counts take the delta path; fractions recount every window.
        for scale in [1.0, 0.17] {
            let corpus = scaled_corpus(5, scale);
            assert_eq!(corpus.chord_contexts().unwrap().exact_sums(), scale == 1.0);
            // With a magic key, and with only adaptive swaps, which mapping
            // never chooses between (`choice_free`).
            let plain = crate::action_keys::test_layouts::COMPACT
                .replace("| * b", "| ~ b")
                .replace("* 'r ay hr ik jo ke rl u' ye\n", "");
            for (source, mode) in [crate::action_keys::test_layouts::COMPACT, plain.as_str()]
                .into_iter()
                .flat_map(|source| ["normal", "aklgg"].map(|mode| (source, mode)))
            {
                let mut layout = ak::Layout::parse(source, Path::new("exact-from.dat")).unwrap();
                layout.combos = combos(
                    r#"[{"keys":["q","p"],"output":"th"},{"keys":["s","x"],"output":"qu"}]"#,
                );
                let physical = PhysicalTables::with_mode(&layout, &weights, mode);
                let prepared =
                    Prepared::with_physical(&layout, &corpus, &weights, physical).unwrap();
                let slot = |layout: &ak::Layout, label: &str| {
                    layout
                        .slots
                        .iter()
                        .position(|slot| slot.label == label)
                        .unwrap()
                };
                let mut previous = None;
                let swap = |context: u8, a: u8, b: u8| crate::swap_fit::Swap { context, a, b };
                // Combos change, letters move, the magic and adaptive keys
                // trade places with letters (their bindings become actions),
                // and adaptive swaps are installed, extended and removed.
                for step in 0..9 {
                    match step {
                        0 => {}
                        1 => layout.combos.truncate(1),
                        2 => layout.swap(slot(&layout, "t"), slot(&layout, "u")),
                        3 => {
                            let magic = layout.slots.iter().position(|slot| slot.label == "*");
                            layout.swap(magic.unwrap_or(slot(&layout, "o")), slot(&layout, "q"))
                        }
                        4 => layout.combos = combos(r#"[{"keys":["a","e"],"output":"qu"}]"#),
                        5 => layout.swap(slot(&layout, "n"), slot(&layout, "i")),
                        6 => layout = crate::swap_fit::install(&layout, &[swap(b'q', b'u', b'x')]),
                        7 => layout = crate::swap_fit::install(&layout, &[swap(b'a', b'a', b'p')]),
                        _ => {
                            let (plain, swaps) = crate::swap_fit::split(&layout);
                            assert!(swaps.len() > 1);
                            layout = crate::swap_fit::install(&plain, &swaps[1..]);
                        }
                    }
                    let (summary, mappings) = prepared
                        .evaluate_exact_from(&layout, previous.as_ref(), &stop, &progress)
                        .unwrap();
                    let fresh =
                        evaluate_with_mode(&layout, &corpus, &weights, &stop, &progress, mode)
                            .unwrap();
                    let context = format!("scale {scale}, {mode}, step {step}");
                    assert_eq!(summary.counts.tables, fresh.counts.tables, "{context}");
                    assert_eq!(summary.counts.skip, fresh.counts.skip, "{context}");
                    assert_eq!(
                        summary.counts.action_presses.to_bits(),
                        fresh.counts.action_presses.to_bits(),
                        "{context}"
                    );
                    assert_eq!(
                        summary.raw.0.map(f64::to_bits),
                        fresh.raw.0.map(f64::to_bits),
                        "{context}"
                    );
                    assert_eq!(
                        summary.metrics.v.map(f64::to_bits),
                        fresh.metrics.v.map(f64::to_bits),
                        "{context}"
                    );
                    assert_eq!(summary.score.to_bits(), fresh.score.to_bits(), "{context}");
                    previous = mappings;
                }
            }
        }
    }

    #[test]
    fn slot_pair_cache_matches_mapped_pair_costs_across_arrangements() {
        let stop = AtomicBool::new(false);
        let progress = AtomicU64::new(0);
        let corpus = corpus(3);
        let mut weights = Weights::default();
        weights.3 = true;
        let layout = ak::Layout::parse(
            "q w e r t  y u i o p\na s d f g  h j k l ;\nz x c v b  @ m , . /\nthumbs: space\n@ hr\n",
            Path::new("summary-slot-pairs.dat"),
        )
        .unwrap();
        let prepared = Prepared::new(&layout, &corpus, &weights);
        let bytes: Vec<u8> = b"qwertyuiopasdfghjkl;zxcvbnm,./ hr#".to_vec();
        let mut arrangements = vec![layout.clone()];
        for (left, right) in [(0, 25), (3, 14), (12, 7)] {
            let mut moved = arrangements.last().unwrap().clone();
            moved.swap(left, right);
            arrangements.push(moved);
        }
        let mut alternate = layout.clone();
        alternate.alt_fingers.push(crate::alt_fingers::Rule {
            ngram: vec![Some(b'e'), Some(b'd')],
            fingers: vec![Some(2), Some(3)],
        });
        arrangements.push(alternate);
        let mut cached = 0;
        // Two rounds: the second reads entries written while mapping others.
        for _ in 0..2 {
            for arrangement in &arrangements {
                for &first in &bytes {
                    for &second in &bytes {
                        let pair = [first, second];
                        cached +=
                            usize::from(plain_pair_slots(arrangement, &weights, pair).is_some());
                        let fast = prepared
                            .pair_movement_costs_binding_variation(
                                arrangement,
                                pair,
                                &stop,
                                &progress,
                            )
                            .unwrap();
                        let mapped = prepared
                            .pair_movement_costs_mapped(arrangement, pair, &stop, &progress, true)
                            .unwrap();
                        assert_eq!(
                            fast.map(f64::to_bits).to_vec(),
                            mapped
                                .iter()
                                .map(|value| value.to_bits())
                                .collect::<Vec<_>>(),
                            "{pair:?}"
                        );
                    }
                }
            }
        }
        assert!(cached > 1000);
        assert!(plain_pair_slots(&layout, &weights, *b"hr").is_none());
        assert!(plain_pair_slots(&arrangements[4], &weights, *b"ed").is_none());
        // With a repeating magic key and an adaptive swap, actions can type
        // most pairs: their costs are kept by the keys each pair presses, and
        // moves that press the same keys reuse them.
        let layout = ak::Layout::parse(
            crate::action_keys::test_layouts::COMPACT,
            Path::new("summary-mapped-pairs.dat"),
        )
        .unwrap();
        let prepared = Prepared::new(&layout, &corpus, &weights);
        let mut arrangements = vec![layout.clone()];
        for (left, right) in [(0, 25), (3, 14), (12, 7), (26, 4)] {
            let mut moved = arrangements.last().unwrap().clone();
            moved.swap(left, right);
            arrangements.push(moved);
        }
        let bytes: Vec<u8> = b"fdlwvqpou,sthygznaeixkmcjb';. r#".to_vec();
        let mut evaluated = 0;
        for _ in 0..2 {
            for arrangement in &arrangements {
                for &first in &bytes {
                    for &second in &bytes {
                        let pair = [first, second];
                        evaluated +=
                            usize::from(plain_pair_slots(arrangement, &weights, pair).is_none());
                        let fast = prepared
                            .pair_movement_costs_binding_variation(
                                arrangement,
                                pair,
                                &stop,
                                &progress,
                            )
                            .unwrap();
                        let mapped = prepared
                            .pair_movement_costs_mapped(arrangement, pair, &stop, &progress, true)
                            .unwrap();
                        assert_eq!(
                            fast.map(f64::to_bits).to_vec(),
                            mapped
                                .iter()
                                .map(|value| value.to_bits())
                                .collect::<Vec<_>>(),
                            "{pair:?}"
                        );
                    }
                }
            }
        }
        let kept = prepared.physical.mapped_pair_costs.lock().unwrap().len();
        assert!(kept > 0 && kept * 3 < evaluated, "{kept} of {evaluated}");
    }

    #[test]
    fn summary_matches_detailed_metrics_and_score_bit_for_bit() {
        let stop = AtomicBool::new(false);
        let progress = AtomicU64::new(0);
        for source in crate::action_fast::tests::fixtures() {
            for order in 3..=5 {
                let corpus = corpus(order);
                let mut layout =
                    ak::Layout::parse(&source, Path::new("summary-inline.dat")).unwrap();
                for swap in [None, Some((0, layout.slots.len() - 1)), Some((2, 17))] {
                    if let Some((a, b)) = swap {
                        layout.swap(a, b);
                    }
                    for weights in [Weights::default(), Weights::new([0.0; N_WEIGHTS])] {
                        let summary =
                            evaluate(&layout, &corpus, &weights, &stop, &progress).unwrap();
                        let detailed = action_ui::evaluate_progress(
                            &layout, &corpus, &weights, &stop, &progress,
                        )
                        .unwrap();
                        assert_eq!(summary.counts.tables, detailed.counts.tables);
                        assert_eq!(summary.counts.skip, detailed.counts.skip);

                        assert_eq!(
                            summary.raw.0.map(f64::to_bits),
                            detailed.raw.0.map(f64::to_bits)
                        );
                        assert_eq!(
                            summary.totals.map(f64::to_bits),
                            detailed.corpus.totals.map(f64::to_bits)
                        );
                        assert_eq!(
                            summary.metrics.v.map(f64::to_bits),
                            detailed.metrics.v.map(f64::to_bits)
                        );
                        assert_eq!(
                            summary.metrics.usage.map(f64::to_bits),
                            detailed.metrics.usage.map(f64::to_bits)
                        );
                        assert_eq!(
                            summary.metrics.off.map(f64::to_bits),
                            detailed.metrics.off.map(f64::to_bits)
                        );
                        assert_eq!(summary.score.to_bits(), detailed.score.to_bits());
                    }
                }
            }
        }
    }

    #[test]
    fn summary_preserves_cancellation_and_empty_corpus_errors() {
        let layout = ak::Layout::parse(
            &crate::action_fast::tests::fixtures()[0],
            Path::new("summary-inline.dat"),
        )
        .unwrap();
        let weights = Weights::default();
        let progress = AtomicU64::new(0);
        for text in [
            r#"{"letters":{"q":1},"bigrams":{},"trigrams":{}}"#,
            r#"{"letters":{"!":1},"bigrams":{},"trigrams":{}}"#,
        ] {
            let corpus =
                ng::NgramCorpus::from_text(text, Path::new("summary-inline.json")).unwrap();
            for cancelled in [false, true] {
                let stop = AtomicBool::new(cancelled);
                let summary = evaluate(&layout, &corpus, &weights, &stop, &progress)
                    .err()
                    .unwrap();
                let detailed =
                    action_ui::evaluate_progress(&layout, &corpus, &weights, &stop, &progress)
                        .err()
                        .unwrap();
                assert_eq!(summary, detailed);
            }
        }
    }

    #[test]
    fn physical_summary_uses_selected_scoring_weights() {
        let stop = AtomicBool::new(false);
        let progress = AtomicU64::new(0);
        let layout = ak::Layout::parse(
            &crate::action_fast::tests::fixtures()[0],
            Path::new("summary-physical.dat"),
        )
        .unwrap();
        let corpus = corpus(5);
        let mut weights = Weights::default();
        weights.2 = mana2_metrics::Weights::from_text("finger-usage-LP = [-7]\n").unwrap();
        for mode in ["aklgg"] {
            let summary =
                evaluate_with_mode(&layout, &corpus, &weights, &stop, &progress, mode).unwrap();
            let stats = ng::mana_stats(
                summary.raw.1.as_ref().unwrap(),
                summary.totals,
                summary.counts.speed_populations,
            );
            assert_eq!(
                summary.score.to_bits(),
                weights.physical_score(&stats).to_bits()
            );
            assert_ne!(summary.score, Weights::default().physical_score(&stats));
        }
    }

    #[test]
    fn prepared_rule_summary_matches_full_evaluation_and_rejects_slot_changes() {
        let stop = AtomicBool::new(false);
        let progress = AtomicU64::new(0);
        let corpus = corpus(5);
        let mut weights = Weights::default();
        weights.3 = true;
        weights.4 = true;
        let mut layout = ak::Layout::parse(
            &crate::action_fast::tests::fixtures()[1],
            Path::new("summary-prepared.dat"),
        )
        .unwrap();
        layout.alt_fingers.push(crate::alt_fingers::Rule {
            ngram: vec![Some(b'q'), Some(b'u')],
            fingers: vec![Some(0), Some(3)],
        });
        layout.combos.push(Json::Object(BTreeMap::from([
            (
                "keys".into(),
                Json::Array(vec![Json::String("a".into()), Json::String("u".into())]),
            ),
            ("output".into(), Json::String("au".into())),
        ])));
        let prepared = Prepared::new(&layout, &corpus, &weights);

        let action = layout
            .actions
            .values_mut()
            .find(|action| matches!(action, ak::Action::Rules { .. }))
            .unwrap();
        let ak::Action::Rules { rules, .. } = action else {
            unreachable!()
        };
        rules.insert(vec![b'x'], ak::Emission::Text(vec![b'z']));

        let full = evaluate(&layout, &corpus, &weights, &stop, &progress).unwrap();
        let reused = prepared.evaluate(&layout, &stop, &progress).unwrap();
        assert_eq!(full.counts.tables, reused.counts.tables);
        assert_eq!(full.counts.skip, reused.counts.skip);
        assert_eq!(full.counts.finger_tables, reused.counts.finger_tables);
        assert_eq!(full.counts.finger_skip, reused.counts.finger_skip);
        assert_eq!(full.raw.0.map(f64::to_bits), reused.raw.0.map(f64::to_bits));
        assert_eq!(
            full.totals.map(f64::to_bits),
            reused.totals.map(f64::to_bits)
        );
        assert_eq!(
            full.metrics.v.map(f64::to_bits),
            reused.metrics.v.map(f64::to_bits)
        );
        assert_eq!(full.score.to_bits(), reused.score.to_bits());

        let cached = prepared.evaluate(&layout, &stop, &progress).unwrap();
        assert!(Arc::ptr_eq(&reused.counts, &cached.counts));
        stop.store(true, Ordering::Relaxed);
        assert_eq!(
            prepared.evaluate(&layout, &stop, &progress).err().unwrap(),
            "cancelled"
        );
        stop.store(false, Ordering::Relaxed);

        let mut changed_metadata = layout.clone();
        changed_metadata.alt_fingers.clear();
        changed_metadata.combos.clear();
        let changed = prepared
            .evaluate(&changed_metadata, &stop, &progress)
            .unwrap();
        let changed_again = prepared
            .evaluate(&changed_metadata, &stop, &progress)
            .unwrap();
        let changed_full =
            evaluate(&changed_metadata, &corpus, &weights, &stop, &progress).unwrap();
        assert!(!Arc::ptr_eq(&changed.counts, &changed_again.counts));
        assert_eq!(
            changed.raw.0.map(f64::to_bits),
            changed_full.raw.0.map(f64::to_bits)
        );
        assert_eq!(changed.score.to_bits(), changed_full.score.to_bits());

        for index in 0..5 {
            let mut variant = layout.clone();
            variant
                .actions
                .insert(format!("memo-unused-{index}"), ak::Action::Inactive);
            prepared.evaluate(&variant, &stop, &progress).unwrap();
        }
        assert_eq!(prepared.summaries.lock().unwrap().len(), 4);
        let evicted = prepared.evaluate(&layout, &stop, &progress).unwrap();
        assert!(!Arc::ptr_eq(&reused.counts, &evicted.counts));

        // Eligible direct text rules exercise dirty-window mapping on the
        // fractional corpus above, including its unmapped reset character.
        let direct_source = format!(
            "{}outer-right: ~ @m ~\naction rep = repeat-output\naction m = magic\nmap m \"q\" = \"u\"\nfallback m = @rep\n",
            crate::action_fast::tests::fixtures()[0],
        );
        let direct = ak::Layout::parse(&direct_source, Path::new("summary-direct.dat")).unwrap();
        let direct_prepared = Prepared::new(&direct, &corpus, &weights);
        let assert_exact = |actual: &Summary, expected: &Summary| {
            assert_eq!(actual.counts.tables, expected.counts.tables);
            assert_eq!(actual.counts.skip, expected.counts.skip);
            assert_eq!(actual.counts.finger_tables, expected.counts.finger_tables);
            assert_eq!(actual.counts.finger_skip, expected.counts.finger_skip);
            assert_eq!(actual.counts.order, expected.counts.order);
            assert_eq!(
                [
                    actual.counts.characters,
                    actual.counts.presses,
                    actual.counts.action_presses,
                    actual.counts.ignored_characters
                ]
                .map(f64::to_bits),
                [
                    expected.counts.characters,
                    expected.counts.presses,
                    expected.counts.action_presses,
                    expected.counts.ignored_characters
                ]
                .map(f64::to_bits),
            );
            assert_eq!(
                actual.raw.0.map(f64::to_bits),
                expected.raw.0.map(f64::to_bits)
            );
            assert_eq!(
                actual.totals.map(f64::to_bits),
                expected.totals.map(f64::to_bits)
            );
            assert_eq!(
                actual.metrics.v.map(f64::to_bits),
                expected.metrics.v.map(f64::to_bits)
            );
            assert_eq!(
                actual.metrics.usage.map(f64::to_bits),
                expected.metrics.usage.map(f64::to_bits)
            );
            assert_eq!(
                actual.metrics.off.map(f64::to_bits),
                expected.metrics.off.map(f64::to_bits)
            );
            assert_eq!(actual.score.to_bits(), expected.score.to_bits());
        };
        let baseline = direct_prepared.evaluate(&direct, &stop, &progress).unwrap();
        assert_exact(
            &baseline,
            &evaluate(&direct, &corpus, &weights, &stop, &progress).unwrap(),
        );
        assert!(direct_prepared.rule_cache.lock().unwrap().is_some());
        let mut direct_trial = direct.clone();
        let ak::Action::Rules { rules, .. } = direct_trial.actions.get_mut("m").unwrap() else {
            unreachable!()
        };
        rules.insert(vec![b'q'], ak::Emission::Text(vec![b'n']));
        let trial_summary = direct_prepared
            .evaluate(&direct_trial, &stop, &progress)
            .unwrap();
        assert_exact(
            &trial_summary,
            &evaluate(&direct_trial, &corpus, &weights, &stop, &progress).unwrap(),
        );
        let work = direct_prepared.rule_work.lock().unwrap().unwrap();
        assert!(work.mapped_windows > 0);
        assert!(work.mapped_windows < work.total_windows);
        assert_ne!(baseline.counts.tables, trial_summary.counts.tables);

        let ak::Action::Rules { rules, .. } = direct_trial.actions.get_mut("m").unwrap() else {
            unreachable!()
        };
        rules.clear();
        let removed = direct_prepared
            .evaluate(&direct_trial, &stop, &progress)
            .unwrap();
        assert_exact(
            &removed,
            &evaluate(&direct_trial, &corpus, &weights, &stop, &progress).unwrap(),
        );
        stop.store(true, Ordering::Relaxed);
        assert_eq!(
            direct_prepared
                .evaluate(&direct_trial, &stop, &progress)
                .err()
                .unwrap(),
            "cancelled"
        );
        stop.store(false, Ordering::Relaxed);

        let ak::Action::Rules { basis, .. } = direct_trial.actions.get_mut("m").unwrap() else {
            unreachable!()
        };
        *basis = ak::Basis::Output;
        let unsupported = direct_prepared
            .evaluate(&direct_trial, &stop, &progress)
            .unwrap();
        assert_exact(
            &unsupported,
            &evaluate(&direct_trial, &corpus, &weights, &stop, &progress).unwrap(),
        );
        assert!(direct_prepared.rule_work.lock().unwrap().is_none());

        let mut changed_direct_metadata = direct.clone();
        changed_direct_metadata
            .alt_fingers
            .push(crate::alt_fingers::Rule {
                ngram: vec![Some(b'q'), Some(b'u')],
                fingers: vec![None, None],
            });
        let metadata_summary = direct_prepared
            .evaluate(&changed_direct_metadata, &stop, &progress)
            .unwrap();
        assert_exact(
            &metadata_summary,
            &evaluate(
                &changed_direct_metadata,
                &corpus,
                &weights,
                &stop,
                &progress,
            )
            .unwrap(),
        );
        assert!(direct_prepared.rule_work.lock().unwrap().is_none());

        layout.slots.swap(0, 1);
        assert_eq!(
            prepared.evaluate(&layout, &stop, &progress).err().unwrap(),
            "prepared action summary requires unchanged physical slots and bindings"
        );
    }
}
