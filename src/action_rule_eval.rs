//! Scoped mapping reuse for ordinary fixed-root, single-byte text-rule edits.
//! Counts are always replayed in corpus order through the normal accumulator.
use crate::action_fast::{KeyState, Mapper, Program};
use crate::action_keys::{self as ak, Action, Basis, Binding, Emission};
use crate::action_ngrams as ng;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

type Mapping = [Option<usize>; 5];

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Work {
    pub(crate) mapped_windows: usize,
    #[cfg(test)]
    pub(crate) total_windows: usize,
}

pub(crate) struct Cache {
    // Cloning a corpus aliases its immutable source windows.
    corpus: ng::NgramCorpus,
    baseline: ak::Layout,
    mapped: Arc<[Mapping]>,
    policy: (crate::alt_fingers::AltPolicy, bool, bool),
}

fn ordinary_rules(action: &Action, layout: &ak::Layout) -> bool {
    let Action::Rules {
        basis: Basis::Text,
        rules,
        fallback,
    } = action
    else {
        return false;
    };
    let Emission::Call(name) = fallback else {
        return false;
    };
    matches!(layout.actions.get(name), Some(Action::RepeatOutput))
        && rules.iter().all(|(context, emission)| {
            context.len() == 1 && matches!(emission, Emission::Text(text) if text.len() == 1)
        })
}

fn eligible(
    layout: &ak::Layout,
    alt_fingers: crate::alt_fingers::AltPolicy,
    combos: bool,
    finite: bool,
) -> bool {
    finite
        && !alt_fingers.active(layout)
        && !(combos && !layout.combos.is_empty())
        && layout.slots.iter().all(|slot| match &slot.binding {
            Binding::Named(name) => match layout.actions.get(name) {
                Some(Action::Inactive | Action::RepeatOutput) => true,
                Some(Action::Text(text)) => text.len() == 1,
                Some(action) => ordinary_rules(action, layout),
                None => false,
            },
            _ => true,
        })
}

impl Cache {
    pub(crate) fn new<F>(
        corpus: &ng::NgramCorpus,
        layout: &ak::Layout,
        stop: &AtomicBool,
        progress: &AtomicU64,
        alt_fingers: crate::alt_fingers::AltPolicy,
        combos: bool,
        finite: bool,
        effort: F,
    ) -> ak::Result<Option<(Self, ng::Counts)>>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        if stop.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        if !eligible(layout, alt_fingers, combos, finite) {
            return Ok(None);
        }
        let Ok(program) = Program::new(layout, corpus.order) else {
            return Ok(None);
        };
        let state = KeyState::new(&program);
        let mut mapper = Mapper::new(&program, &state);
        mapper.enable_transition_memo();
        let mut mapped = Vec::with_capacity(corpus.rule_window_count());
        let counts = corpus.evaluate_rule_mappings(layout, stop, progress, |_, text| {
            let keys = mapper.map_finite_profiled::<false, F>(
                text,
                &effort,
                &mut crate::action_profile::MappingCounts::default(),
            )?;
            mapped.push(keys);
            Ok(keys)
        })?;
        Ok(Some((
            Self {
                corpus: corpus.clone(),
                baseline: layout.clone(),
                mapped: mapped.into(),
                policy: (alt_fingers, combos, finite),
            },
            counts,
        )))
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.mapped.len()
    }

    pub(crate) fn evaluate<F>(
        &self,
        layout: &ak::Layout,
        stop: &AtomicBool,
        progress: &AtomicU64,
        alt_fingers: crate::alt_fingers::AltPolicy,
        combos: bool,
        finite: bool,
        effort: F,
    ) -> ak::Result<Option<(ng::Counts, Work)>>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        if stop.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let base = &self.baseline;
        if self.policy != (alt_fingers, combos, finite)
            || !eligible(layout, alt_fingers, combos, finite)
            || layout.slots != base.slots
            || layout.row_stagger != base.row_stagger
            || layout.left_outer != base.left_outer
            || layout.right_outer != base.right_outer
            || layout.action_mode != base.action_mode
            || layout.actions.len() != base.actions.len()
        {
            return Ok(None);
        }
        let mut changed = [false; 256];
        for (name, old) in &base.actions {
            let Some(new) = layout.actions.get(name) else {
                return Ok(None);
            };
            if old == new {
                continue;
            }
            if !ordinary_rules(old, base) || !ordinary_rules(new, layout) {
                return Ok(None);
            }
            let (
                Action::Rules {
                    rules: before,
                    fallback: old_fallback,
                    ..
                },
                Action::Rules {
                    rules: after,
                    fallback: new_fallback,
                    ..
                },
            ) = (old, new)
            else {
                unreachable!()
            };
            if old_fallback != new_fallback {
                return Ok(None);
            }
            for context in before.keys().chain(after.keys()) {
                if before.get(context) != after.get(context) {
                    changed[context[0] as usize] = true;
                }
            }
        }
        let Ok(program) = Program::new(layout, self.corpus.order) else {
            return Ok(None);
        };
        let state = KeyState::new(&program);
        let mut mapper = Mapper::new(&program, &state);
        mapper.enable_transition_memo();
        let mut work = Work {
            mapped_windows: 0,
            #[cfg(test)]
            total_windows: self.len(),
        };
        let counts =
            self.corpus
                .evaluate_rule_mappings(layout, stop, progress, |index, text| {
                    // Include losing alternatives, all history positions, gaps, and
                    // lookahead. A terminal winner alone cannot prove independence.
                    if text.iter().any(|byte| changed[*byte as usize]) {
                        work.mapped_windows += 1;
                        mapper.map_finite_profiled::<false, F>(
                            text,
                            &effort,
                            &mut crate::action_profile::MappingCounts::default(),
                        )
                    } else {
                        Ok(self.mapped[index])
                    }
                })?;
        Ok(Some((counts, work)))
    }
}
