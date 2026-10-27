//! Sparse scoring for ordinary, single-byte fixed-root magic-rule edits.
//!
//! This is a proposal scorer. It maps a fixed layout once, then remaps only
//! source windows containing a changed rule context. Accepted finalists still
//! go through the canonical ordered evaluator before entering user-visible
//! results.

use crate::action_fast::{KeyState, Mapper, Program};
use crate::action_keys::{self as ak, Action, Basis, Binding, Emission};
use crate::action_ngrams as ng;
use crate::action_summary::PhysicalTables;
use crate::{Json, Metrics, Weights};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

type Mapping = ng::RuleDeltaMapping;

static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

fn next_session() -> u64 {
    NEXT_SESSION.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Work {
    pub(crate) mapped_windows: usize,
    pub(crate) total_windows: usize,
}

/// Corpus text plus an inverted index over every retained source position.
/// It is independent of layouts and can be shared by fixed-root fitters built
/// for different binding arrangements.
pub(crate) struct SourceIndex {
    windows: Arc<[ng::RuleDeltaWindow]>,
    postings: [Arc<[u32]>; 256],
    order: usize,
    /// Windows come from the centered chord corpus that combos require.
    chords: bool,
    speed_populations: [f64; 2],
}

impl SourceIndex {
    pub(crate) fn supports(layout: &ak::Layout, root: usize, weights: &Weights) -> bool {
        root < layout.slots.len()
            && layout.slots.len() < u8::MAX as usize
            && !weights.alts().active(layout)
            && root_action(layout, root)
                .is_some_and(|(_, action)| ordinary_parts(action, layout).is_some())
            && safe_actions(layout)
    }

    /// `chords` indexes the centered chord corpus, which layouts with active
    /// combos are evaluated on.
    pub(crate) fn new(
        corpus: &ng::NgramCorpus,
        chords: bool,
        stop: &AtomicBool,
    ) -> ak::Result<Arc<Self>> {
        if stop.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let windows = if chords {
            corpus.chord_contexts()?.rule_delta_windows(stop)?
        } else {
            corpus.rule_delta_windows(stop)?
        };
        if windows.len() > u32::MAX as usize {
            return Err("rule delta source exceeds index capacity".into());
        }
        let mut postings: [Vec<u32>; 256] = std::array::from_fn(|_| Vec::new());
        for (index, window) in windows.iter().enumerate() {
            if index % 1024 == 0 && stop.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            let mut seen = [false; 256];
            for &byte in window.text() {
                if !seen[byte as usize] {
                    seen[byte as usize] = true;
                    postings[byte as usize].push(index as u32);
                }
            }
        }
        if stop.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        Ok(Arc::new(Self {
            windows: windows.into(),
            postings: postings.map(Into::into),
            order: corpus.order,
            chords,
            speed_populations: corpus.speed_populations,
        }))
    }

    pub(crate) fn chords(&self) -> bool {
        self.chords
    }

    pub(crate) fn len(&self) -> usize {
        self.windows.len()
    }

    fn affected(&self, changed: &[u8]) -> Vec<u32> {
        if changed.len() == 1 {
            return self.postings[changed[0] as usize].to_vec();
        }
        let capacity = changed
            .iter()
            .map(|byte| self.postings[*byte as usize].len())
            .sum();
        let mut affected = Vec::with_capacity(capacity);
        for byte in changed {
            affected.extend_from_slice(&self.postings[*byte as usize]);
        }
        affected.sort_unstable();
        affected.dedup();
        affected
    }
}

pub(crate) struct Trial {
    session: u64,
    generation: u64,
    layout: ak::Layout,
    aggregate: ng::RuleDeltaAggregate,
    changes: Vec<(u32, Mapping)>,
    metrics: Metrics,
    score: f64,
    work: Work,
}

impl Trial {
    pub(crate) fn score(&self) -> f64 {
        self.score
    }

    pub(crate) fn metrics(&self) -> &Metrics {
        &self.metrics
    }

    pub(crate) fn ignored_characters(&self) -> f64 {
        self.aggregate.ignored_characters()
    }

    pub(crate) fn usable(&self) -> bool {
        self.aggregate.usable()
    }

    pub(crate) fn work(&self) -> Work {
        self.work
    }
}

pub(crate) struct Delta {
    source: Arc<SourceIndex>,
    combos: Option<Arc<crate::combos::Program>>,
    layout: ak::Layout,
    root: usize,
    root_action: String,
    weights: Weights,
    physical: Arc<PhysicalTables>,
    mappings: Arc<[Mapping]>,
    aggregate: ng::RuleDeltaAggregate,
    metrics: Metrics,
    score: f64,
    session: u64,
    generation: u64,
    initial_work: Work,
}

impl Clone for Delta {
    fn clone(&self) -> Self {
        Self {
            source: Arc::clone(&self.source),
            combos: self.combos.clone(),
            layout: self.layout.clone(),
            root: self.root,
            root_action: self.root_action.clone(),
            weights: self.weights.clone(),
            physical: Arc::clone(&self.physical),
            mappings: Arc::clone(&self.mappings),
            aggregate: self.aggregate.clone(),
            metrics: self.metrics.clone(),
            score: self.score,
            session: next_session(),
            generation: self.generation,
            initial_work: self.initial_work,
        }
    }
}

impl Delta {
    pub(crate) fn new(
        source: Arc<SourceIndex>,
        layout: &ak::Layout,
        root: usize,
        weights: &Weights,
        physical: Arc<PhysicalTables>,
        stop: &AtomicBool,
    ) -> ak::Result<Option<Self>> {
        if stop.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        if !SourceIndex::supports(layout, root, weights)
            || !physical.matches(layout, weights)
            || !physical.effort.all_finite()
        {
            return Ok(None);
        }
        let Some((root_action, _)) = root_action(layout, root) else {
            return Ok(None);
        };
        if physical.geometry.keys.len() != layout.slots.len() {
            return Ok(None);
        }
        let combos = if weights.4 && !layout.combos.is_empty() {
            let Ok(program) = crate::combos::Program::new(layout) else {
                return Ok(None);
            };
            Some(Arc::new(program))
        } else {
            None
        };
        // A chord press spans several tails, which the per-window mappings
        // here cannot hold; the canonical evaluator scores layouts with combos.
        if combos.is_some() || source.chords {
            return Ok(None);
        }
        let Ok(program) = Program::new(layout, source.order) else {
            return Ok(None);
        };
        let state = KeyState::new(&program);
        let mut mapper = Mapper::new(&program, &state);
        mapper.enable_transition_memo();
        let mut aggregate = ng::RuleDeltaAggregate::new(
            Arc::clone(&physical.geometry),
            matches!(physical.mode.as_str(), "aklgg"),
            source.speed_populations,
        );
        let mut mappings = Vec::with_capacity(source.len());
        for (index, window) in source.windows.iter().copied().enumerate() {
            if index % 1024 == 0 && stop.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            let mapped = map_window(&mut mapper, window, &physical)?;
            aggregate.add_mapping(mapped, window);
            mappings.push(mapped);
        }
        if stop.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        if !aggregate.usable() {
            return Err("layout has no usable letters/bigrams in this corpus".into());
        }
        let metrics = aggregate.metrics();
        let score = aggregate.objective(&metrics, weights, &physical.mode);
        let work = Work {
            mapped_windows: source.len(),
            total_windows: source.len(),
        };
        Ok(Some(Self {
            source,
            combos,
            layout: layout.clone(),
            root,
            root_action: root_action.to_string(),
            weights: weights.clone(),
            physical,
            mappings: mappings.into(),
            aggregate,
            metrics,
            score,
            session: next_session(),
            generation: 0,
            initial_work: work,
        }))
    }

    pub(crate) fn score(&self) -> f64 {
        self.score
    }

    pub(crate) fn metrics(&self) -> &Metrics {
        &self.metrics
    }

    pub(crate) fn ignored_characters(&self) -> f64 {
        self.aggregate.ignored_characters()
    }

    pub(crate) fn initial_work(&self) -> Work {
        self.initial_work
    }

    pub(crate) fn evaluate(
        &self,
        layout: &ak::Layout,
        stop: &AtomicBool,
    ) -> ak::Result<Option<Trial>> {
        if stop.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let Some(changed) = self.changed_contexts(layout) else {
            return Ok(None);
        };
        let affected = self.source.affected(&changed);
        let mut aggregate = self.aggregate.clone();
        let mut changes = Vec::new();
        if !affected.is_empty() {
            let Ok(program) = Program::new(layout, self.source.order) else {
                return Ok(None);
            };
            let state = KeyState::new(&program);
            let mut mapper = Mapper::new(&program, &state);
            mapper.enable_transition_memo();
            for (position, &id) in affected.iter().enumerate() {
                if position % 256 == 0 && stop.load(Ordering::Relaxed) {
                    return Err("cancelled".into());
                }
                let window = self.source.windows[id as usize];
                let mapped = map_window(&mut mapper, window, &self.physical)?;
                let old = self.mappings[id as usize];
                if old != mapped {
                    aggregate.replace_mapping(old, mapped, window);
                    changes.push((id, mapped));
                }
            }
        }
        if stop.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let metrics = aggregate.metrics();
        let score = aggregate.objective(&metrics, &self.weights, &self.physical.mode);
        Ok(Some(Trial {
            session: self.session,
            generation: self.generation,
            layout: layout.clone(),
            aggregate,
            changes,
            metrics,
            score,
            work: Work {
                mapped_windows: affected.len(),
                total_windows: self.source.len(),
            },
        }))
    }

    pub(crate) fn commit(&mut self, trial: Trial) -> ak::Result<()> {
        if trial.session != self.session || trial.generation != self.generation {
            return Err("stale rule delta trial".into());
        }
        if !trial.aggregate.usable() {
            return Err("layout has no usable letters/bigrams in this corpus".into());
        }
        let next_generation = self
            .generation
            .checked_add(1)
            .ok_or("rule delta generation overflow")?;
        let mappings = Arc::make_mut(&mut self.mappings);
        for (id, mapped) in trial.changes {
            mappings[id as usize] = mapped;
        }
        self.layout = trial.layout;
        self.aggregate = trial.aggregate;
        self.metrics = trial.metrics;
        self.score = trial.score;
        self.generation = next_generation;
        Ok(())
    }

    fn changed_contexts(&self, layout: &ak::Layout) -> Option<Vec<u8>> {
        if layout.slots != self.layout.slots
            || layout.row_stagger != self.layout.row_stagger
            || layout.left_outer != self.layout.left_outer
            || layout.right_outer != self.layout.right_outer
            || layout.action_mode != self.layout.action_mode
            || layout.alt_fingers != self.layout.alt_fingers
            || !same_json_slice(&layout.combos, &self.layout.combos)
            || !SourceIndex::supports(layout, self.root, &self.weights)
        {
            return None;
        }
        if layout.actions.len() != self.layout.actions.len() {
            return None;
        }
        for (name, action) in &self.layout.actions {
            if name != &self.root_action && layout.actions.get(name) != Some(action) {
                return None;
            }
        }
        let (_, before) = root_action(&self.layout, self.root)?;
        let (name, after) = root_action(layout, self.root)?;
        if name != self.root_action {
            return None;
        }
        let (before_rules, before_fallback) = ordinary_parts(before, &self.layout)?;
        let (after_rules, after_fallback) = ordinary_parts(after, layout)?;
        if before_fallback != after_fallback {
            return None;
        }
        let mut changed = Vec::new();
        for context in before_rules.keys().chain(after_rules.keys()) {
            if before_rules.get(context) != after_rules.get(context) {
                changed.push(context[0]);
            }
        }
        changed.sort_unstable();
        changed.dedup();
        Some(changed)
    }
}

/// Map one source window as the canonical evaluator would: the finite fast
/// path (layouts with combos are not supported here).
fn map_window(
    mapper: &mut Mapper<'_>,
    window: ng::RuleDeltaWindow,
    physical: &PhysicalTables,
) -> ak::Result<Mapping> {
    let effort = |a, b, key| physical.effort.get(a, b, key);
    Ok(window.mapping(mapper.map_finite_profiled::<false, _>(
        window.text(),
        &effort,
        &mut crate::action_profile::MappingCounts::default(),
    )?))
}

fn root_action(layout: &ak::Layout, root: usize) -> Option<(&str, &Action)> {
    let Binding::Named(name) = &layout.slots.get(root)?.binding else {
        return None;
    };
    Some((name, layout.actions.get(name)?))
}

fn ordinary_parts<'a>(
    action: &'a Action,
    layout: &ak::Layout,
) -> Option<(&'a BTreeMap<Vec<u8>, Emission>, &'a Emission)> {
    let Action::Rules {
        basis: Basis::Text,
        rules,
        fallback,
    } = action
    else {
        return None;
    };
    let safe_fallback = match fallback {
        Emission::Call(name) => matches!(layout.actions.get(name), Some(Action::RepeatOutput)),
        Emission::Text(text) => text.len() == 1,
        _ => false,
    };
    if !safe_fallback
        || !rules.iter().all(|(context, emission)| {
            context.len() == 1 && matches!(emission, Emission::Text(text) if text.len() == 1)
        })
    {
        return None;
    }
    Some((rules, fallback))
}

fn safe_actions(layout: &ak::Layout) -> bool {
    layout.slots.iter().all(|slot| match &slot.binding {
        Binding::Named(name) => match layout.actions.get(name) {
            Some(Action::Inactive | Action::RepeatOutput) => true,
            Some(Action::Text(text)) => text.len() == 1,
            Some(action) => ordinary_parts(action, layout).is_some(),
            None => false,
        },
        Binding::Text(text) => text.len() == 1,
        Binding::Empty => true,
    })
}

pub(crate) fn same_json_slice(left: &[Json], right: &[Json]) -> bool {
    left.len() == right.len() && left.iter().zip(right).all(|(a, b)| same_json(a, b))
}

fn same_json(left: &Json, right: &Json) -> bool {
    match (left, right) {
        (Json::Object(a), Json::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .zip(b)
                    .all(|((ak, av), (bk, bv))| ak == bk && same_json(av, bv))
        }
        (Json::Array(a), Json::Array(b)) => same_json_slice(a, b),
        (Json::String(a), Json::String(b)) => a == b,
        (Json::Number(a), Json::Number(b)) => a.to_bits() == b.to_bits(),
        (Json::Bool(a), Json::Bool(b)) => a == b,
        (Json::Null, Json::Null) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{action_summary as summary, alt_fingers, Path};

    #[test]
    fn physical_mode_rule_trials_match_canonical_scores() {
        let layout = ak::Layout::parse(
            "q w e r t  y u i o p\na s d f g  h j k l ;\nz x c v b  @ m , . /\nthumbs: space\n@ hr\n",
            Path::new("inline-physical-delta.dat"),
        )
        .unwrap();
        let root = layout.slots.iter().position(|s| s.label == "@").unwrap();
        let Binding::Named(name) = layout.slots[root].binding.clone() else {
            unreachable!()
        };
        let text = format!(
            "{}ded de ed un nu hr cede ",
            "the cat sat on a mat ".repeat(20)
        );
        let fields: Vec<_> = ak::text_ngrams(text.as_bytes())
            .iter()
            .zip(["letters", "bigrams", "trigrams", "fourgrams", "fivegrams"])
            .map(|(table, field)| {
                let entries: Vec<_> = table
                    .iter()
                    .map(|(gram, count)| format!("{}:{count}", ak::quote(gram)))
                    .collect();
                format!("\"{field}\":{{{}}}", entries.join(","))
            })
            .collect();
        let corpus = ng::NgramCorpus::from_text(
            &format!("{{{}}}", fields.join(",")),
            Path::new("inline-physical-delta-corpus"),
        )
        .unwrap();
        let weights = Weights::default();
        let stop = AtomicBool::new(false);
        let source = SourceIndex::new(&corpus, false, &stop).unwrap();
        for mode in ["aklgg"] {
            let physical = PhysicalTables::with_mode(&layout, &weights, mode);
            let mut delta = Delta::new(
                Arc::clone(&source),
                &layout,
                root,
                &weights,
                physical,
                &stop,
            )
            .unwrap()
            .unwrap();
            let mut current = layout.clone();
            for (context, output) in [(b'e', b'd'), (b'u', b'n'), (b'h', b'a')] {
                let mut proposed = current.clone();
                let Action::Rules { rules, .. } = proposed.actions.get_mut(&name).unwrap() else {
                    unreachable!()
                };
                rules.insert(vec![context], Emission::Text(vec![output]));
                let trial = delta.evaluate(&proposed, &stop).unwrap().unwrap();
                let expected = summary::evaluate_with_mode(
                    &proposed,
                    &corpus,
                    &weights,
                    &stop,
                    &AtomicU64::new(0),
                    mode,
                )
                .unwrap();
                assert!((trial.score() - expected.score).abs() < 1e-8, "{mode}");
                for (a, b) in trial.metrics.v.iter().zip(expected.metrics.v) {
                    assert!((a - b).abs() < 1e-8);
                }
                delta.commit(trial).unwrap();
                current = proposed;
            }
        }
    }

    #[test]
    fn sparse_rule_trials_match_canonical_counts_and_reject_stale_or_unsupported_states() {
        let mut layout = ak::Layout::parse(
            "q w e r t  y u i o p\na s d f g  h j k l ;\nz x c v b  @ m , . /\nthumbs: space\n@ hr\n",
            Path::new("inline-delta.dat"),
        ).unwrap();
        // A competing action can win even where the edited action previously lost.
        let competitor = layout.slots.iter().position(|s| s.label == "p").unwrap();
        layout
            .actions
            .insert("other".into(), Action::Text(vec![b'a']));
        layout.slots[competitor].binding = Binding::Named("other".into());
        let root = layout.slots.iter().position(|s| s.label == "@").unwrap();
        let Binding::Named(name) = layout.slots[root].binding.clone() else {
            unreachable!()
        };
        let text = format!(
            "{}za zaa zqa #az ha har hrr aa za",
            "qwe wqw er ui ".repeat(30)
        );
        let fields: Vec<_> = ak::text_ngrams(text.as_bytes())
            .iter()
            .zip(["letters", "bigrams", "trigrams", "fourgrams", "fivegrams"])
            .map(|(table, name)| {
                let entries: Vec<_> = table
                    .iter()
                    .map(|(gram, count)| format!("{}:{}", ak::quote(gram), *count as f64 * 0.25))
                    .collect();
                format!("\"{name}\":{{{}}}", entries.join(","))
            })
            .collect();
        let corpus = ng::NgramCorpus::from_text(
            &format!("{{{}}}", fields.join(",")),
            Path::new("inline-delta-corpus"),
        )
        .unwrap();
        let weights = Weights::default();
        let stop = AtomicBool::new(false);
        let source = SourceIndex::new(&corpus, false, &stop).unwrap();
        let physical = PhysicalTables::new(&layout, &weights);
        let mut delta = Delta::new(
            Arc::clone(&source),
            &layout,
            root,
            &weights,
            Arc::clone(&physical),
            &stop,
        )
        .unwrap()
        .unwrap();
        let check = |trial: &Trial, proposed: &ak::Layout| {
            let expected =
                summary::evaluate(proposed, &corpus, &weights, &stop, &AtomicU64::new(0)).unwrap();
            assert!((trial.score() - expected.score).abs() < 1e-8);
            for (a, b) in trial.metrics.v.iter().zip(expected.metrics.v) {
                assert!((a - b).abs() < 1e-8);
            }
            for (a, b) in trial.metrics.usage.iter().zip(expected.metrics.usage) {
                assert!((a - b).abs() < 1e-8);
            }
            for (a, b) in trial.metrics.off.iter().zip(expected.metrics.off) {
                assert!((a - b).abs() < 1e-8);
            }
            for (a, b) in trial.aggregate.totals().iter().zip(expected.totals) {
                assert!((a - b).abs() < 1e-8);
            }
            assert!((trial.ignored_characters() - expected.counts.ignored_characters).abs() < 1e-8);
        };
        let change = |input: &ak::Layout, context: u8, output: u8| {
            let mut changed = input.clone();
            let Action::Rules { rules, .. } = changed.actions.get_mut(&name).unwrap() else {
                unreachable!()
            };
            rules.insert(vec![context], Emission::Text(vec![output]));
            changed
        };
        let first = change(&layout, b'z', b'a');
        let trial = delta.evaluate(&first, &stop).unwrap().unwrap();
        check(&trial, &first);
        assert!(
            trial.work.mapped_windows > 0 && trial.work.mapped_windows < trial.work.total_windows
        );
        eprintln!(
            "sparse rule fixture: {} / {} windows remapped",
            trial.work.mapped_windows, trial.work.total_windows
        );
        let stale = delta
            .evaluate(&change(&layout, b'h', b'a'), &stop)
            .unwrap()
            .unwrap();
        let foreign = delta.clone().evaluate(&first, &stop).unwrap().unwrap();
        assert!(delta.commit(foreign).is_err());
        delta.commit(trial).unwrap();
        assert!(delta.commit(stale).is_err());
        let second = change(&first, b'h', b'a');
        let trial = delta.evaluate(&second, &stop).unwrap().unwrap();
        check(&trial, &second);
        delta.commit(trial).unwrap();
        let mut empty = second.clone();
        let Action::Rules { rules, .. } = empty.actions.get_mut(&name).unwrap() else {
            unreachable!()
        };
        rules.clear();
        let removal = delta.evaluate(&empty, &stop).unwrap().unwrap();
        check(&removal, &empty);
        let unchanged = delta.evaluate(&second, &stop).unwrap().unwrap();
        assert_eq!(unchanged.work.mapped_windows, 0);
        let mut moved = second.clone();
        moved.swap(0, root);
        assert!(delta.evaluate(&moved, &stop).unwrap().is_none());
        let mut geometry = second.clone();
        geometry.slots[0].row_offset += 1;
        assert!(delta.evaluate(&geometry, &stop).unwrap().is_none());
        assert!(Delta::new(
            Arc::clone(&source),
            &geometry,
            root,
            &weights,
            Arc::clone(&physical),
            &stop
        )
        .unwrap()
        .is_none());
        let mut unsupported = second.clone();
        let Action::Rules { basis, .. } = unsupported.actions.get_mut(&name).unwrap() else {
            unreachable!()
        };
        *basis = Basis::SkipPress;
        assert!(!SourceIndex::supports(&unsupported, root, &weights));
        let mut enabled = weights.clone();
        let mut alts = second.clone();
        alts.alt_fingers.push(alt_fingers::Rule {
            ngram: vec![Some(b'a'), Some(b'a')],
            fingers: vec![None, None],
        });
        enabled.3 = true;
        assert!(!SourceIndex::supports(&alts, root, &enabled));

        // Active combos are left to the canonical evaluator, with either
        // corpus.
        let mut chorded = layout.clone();
        let Json::Array(values) = crate::parse_json(
            r#"[{"keys":["x","c"],"output":"za"},{"keys":["q","w"],"output":"qw"}]"#,
        )
        .unwrap() else {
            unreachable!()
        };
        chorded.combos = values;
        assert!(weights.4 && SourceIndex::supports(&chorded, root, &weights));
        let chord_source = SourceIndex::new(&corpus, true, &stop).unwrap();
        assert!(chord_source.chords());
        let plain_delta = Delta::new(
            Arc::clone(&source),
            &chorded,
            root,
            &weights,
            Arc::clone(&physical),
            &stop,
        );
        assert!(plain_delta.unwrap().is_none());
        // Chord presses span several tails: the canonical evaluator scores
        // layouts with combos.
        let chord_delta = Delta::new(
            chord_source,
            &chorded,
            root,
            &weights,
            Arc::clone(&physical),
            &stop,
        );
        assert!(chord_delta.unwrap().is_none());

        let before = delta.score().to_bits();
        stop.store(true, Ordering::Relaxed);
        assert_eq!(delta.evaluate(&empty, &stop).err().unwrap(), "cancelled");
        assert!(SourceIndex::new(&corpus, false, &stop).is_err());
        assert_eq!(delta.score().to_bits(), before);
        stop.store(false, Ordering::Relaxed);
        assert!(delta.evaluate(&empty, &stop).unwrap().is_some());
        assert_eq!(std::mem::size_of::<Mapping>(), 7);
    }
}
