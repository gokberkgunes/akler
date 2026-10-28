//! Physical combo plans parsed from a layout's preserved combo definitions.
//!
//! A combo types its output on its own keys: `a+b → je` types "je" by pressing
//! the a and b keys at once, as keyboard firmware does (and akl.gg scores):
//! one chord press. Both keys add usage; movement to and from the chord counts
//! once for each of its keys, and there is none between them
//! (`action_ngrams::ChordGrams`). Chords follow their keys as they move.

use crate::{action_keys, Json};

/// How a layout's combos are scored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Policy {
    /// `[typing] combos`: type combo outputs on their keys at all.
    pub(crate) enabled: bool,
    /// `[combos] fatfingering`: the combo fitter may also chord two
    /// neighbours on one finger.
    pub(crate) fatfinger: bool,
}

impl Policy {
    pub(crate) const OFF: Self = Self {
        enabled: false,
        fatfinger: false,
    };
    /// Combos, fitted on same-finger neighbours too.
    #[cfg(test)]
    pub(crate) const ALL: Self = Self {
        enabled: true,
        fatfinger: true,
    };
}

/// `true` enables combos, fitted without same-finger neighbours, the
/// configuration default.
impl From<bool> for Policy {
    fn from(enabled: bool) -> Self {
        Self {
            enabled,
            fatfinger: false,
        }
    }
}

/// Two keys one finger can press together: distinct main keys on the same
/// finger, one row or one column apart (up-down or left-right, no diagonals).
pub(crate) fn fat_finger_pair(a: &action_keys::Slot, b: &action_keys::Slot) -> bool {
    a.main && b.main && a.finger == b.finger && (a.row - b.row).abs() + (a.col - b.col).abs() == 1
}

#[derive(Clone, Debug, PartialEq)]
struct Combo {
    output: Vec<u8>,
    slots: Vec<usize>,
    /// Each participant types its own output character, as fitted combos do.
    own_output: bool,
}

/// Longest context `Program::plan_window` accepts: cached windows hold at
/// most five bytes (chord windows use four).
pub(crate) const PLAN_LEN: usize = 5;

/// One context's combos, planned once and reused for mapping and movement.
/// Every combo is one chord press of all its keys (as on keyboards and
/// akl.gg). Equivalent to `forced_at` and `chord_mask_at`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ChordPlan {
    len: u8,
    /// The key each position is pressed on when a combo types it.
    forced: [Option<usize>; PLAN_LEN],
    /// Positions a chord types.
    chord: [bool; PLAN_LEN],
    /// Where each chord's press begins.
    starts: [bool; PLAN_LEN],
    /// Leading positions a combo that began before a left-open context may
    /// cover; their movement is unknown.
    open_prefix: u8,
    /// Planned for alternate fingers alone (`Program::for_contexts`):
    /// `chord_grams` counts skipgrams one key later.
    ahead: bool,
}

impl ChordPlan {
    /// This plan, made with declared slots, with each forced key moved to
    /// `positions` (`Program::plan_window_at`).
    pub(crate) fn at(&self, positions: &[usize]) -> Self {
        let mut plan = *self;
        for forced in &mut plan.forced {
            *forced = forced.map(|slot| positions[slot]);
        }
        plan
    }

    /// The physical key each position is pressed on, if a combo types it.
    pub(crate) fn forced(&self) -> &[Option<usize>] {
        &self.forced[..usize::from(self.len)]
    }

    /// Whether this plan forces, chords and hides nothing: the window then
    /// maps exactly as without combos.
    pub(crate) fn is_empty(&self) -> bool {
        !self.ahead && self.open_prefix == 0 && self.forced.iter().all(Option::is_none)
    }

    /// Whether the plan is for alternate fingers alone, without combos.
    pub(crate) fn ahead(&self) -> bool {
        self.ahead
    }

    /// The planned context's length.
    pub(crate) fn len(&self) -> usize {
        usize::from(self.len)
    }

    /// Chord positions.
    pub(crate) fn mask(&self) -> [bool; PLAN_LEN] {
        self.chord
    }

    pub(crate) fn chorded(&self, index: usize) -> bool {
        self.chord[index]
    }

    /// Whether a chord's press begins at `index`.
    pub(crate) fn starts(&self, index: usize) -> bool {
        self.starts[index]
    }

    /// Leading positions whose press is unknown.
    pub(crate) fn open_prefix(&self) -> usize {
        usize::from(self.open_prefix)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Program {
    combos: Vec<Combo>,
    slot_count: usize,
    /// Bit sets of output first bytes, and of the later bytes a left-open
    /// window can begin with: a window holding neither has an empty plan.
    first_bytes: [u64; 4],
    open_bytes: [u64; 4],
    /// No combos: alternate fingers alone asked for centred contexts, and
    /// every context is planned (`ChordPlan::ahead`).
    ahead: bool,
}

fn has_byte(set: &[u64; 4], byte: u8) -> bool {
    set[usize::from(byte) / 64] & 1 << (usize::from(byte) % 64) != 0
}

// Byte loops: these outputs are two bytes, and slice equality calls memcmp.
fn same(output: &[u8], text: &[u8]) -> bool {
    output.len() == text.len() && output.iter().zip(text).all(|(a, b)| a == b)
}

impl Program {
    pub(crate) fn new(layout: &action_keys::Layout) -> Result<Self, String> {
        let mut combos = Vec::with_capacity(layout.combos.len());
        for (index, value) in layout.combos.iter().enumerate() {
            let fail = |message: &str| format!("combos[{index}]: {message}");
            let Json::Object(fields) = value else {
                return Err(fail("expected an object"));
            };
            if fields
                .keys()
                .any(|field| !matches!(field.as_str(), "keys" | "inputs" | "output"))
            {
                return Err(fail("only keys or inputs, and output, are supported"));
            }
            // `keys` lists one chord's labels. `inputs`, the form magic rules
            // use, lists chords as strings with one character per key; several
            // strings declare several chords with the same output.
            let chords: Vec<Vec<String>> = match (fields.get("keys"), fields.get("inputs")) {
                (Some(Json::Array(keys)), None) => vec![keys
                    .iter()
                    .map(|key| match key {
                        Json::String(label) => Ok(label.clone()),
                        _ => Err(fail("key labels must be strings")),
                    })
                    .collect::<Result<_, _>>()?],
                (None, Some(Json::Array(inputs))) if !inputs.is_empty() => inputs
                    .iter()
                    .map(|input| match input {
                        Json::String(text) => Ok(text.chars().map(String::from).collect()),
                        _ => Err(fail("inputs must be strings")),
                    })
                    .collect::<Result<_, _>>()?,
                (Some(_), None) => return Err(fail("keys must be an array")),
                (None, Some(_)) => return Err(fail("inputs must be a non-empty array")),
                _ => return Err(fail("expected either keys or inputs")),
            };
            let Some(Json::String(output)) = fields.get("output") else {
                return Err(fail("output must be a string"));
            };
            for keys in chords {
                if keys.len() != 2 {
                    return Err(fail("a combo chords exactly 2 keys"));
                }
                if output.len() != keys.len()
                    || !output
                        .as_bytes()
                        .iter()
                        .all(|byte| (32..=126).contains(byte))
                {
                    return Err(fail(
                        "output must contain one printable ASCII character per key",
                    ));
                }

                let mut slots = Vec::with_capacity(keys.len());
                for (key_index, label) in keys.iter().enumerate() {
                    if label.is_empty() {
                        return Err(fail("key labels must not be empty"));
                    }
                    let mut matches = layout
                        .slots
                        .iter()
                        .enumerate()
                        .filter(|(_, slot)| {
                            let label = if label == " "
                                || label == "␣"
                                || label.eq_ignore_ascii_case("space")
                            {
                                "␠"
                            } else {
                                label.as_str()
                            };
                            slot.label == label
                        })
                        .map(|(slot, _)| slot);
                    let Some(slot) = matches.next() else {
                        return Err(fail(&format!(
                            "keys[{key_index}] label {label:?} is not present in the layout"
                        )));
                    };
                    if matches.next().is_some() {
                        return Err(fail(&format!(
                            "keys[{key_index}] label {label:?} is ambiguous in the layout"
                        )));
                    }
                    if matches!(layout.slots[slot].binding, action_keys::Binding::Empty) {
                        return Err(fail(&format!(
                            "keys[{key_index}] label {label:?} is an empty physical slot"
                        )));
                    }
                    if slots.contains(&slot) {
                        return Err(fail("keys must refer to distinct physical slots"));
                    }
                    slots.push(slot);
                }
                let output: Vec<u8> = output
                    .as_bytes()
                    .iter()
                    .map(u8::to_ascii_lowercase)
                    .collect();
                let own_output = slots.iter().zip(&output).all(|(&slot, &byte)| {
                    matches!(&layout.slots[slot].binding,
                        action_keys::Binding::Text(text)
                            if text.len() == 1 && text[0].to_ascii_lowercase() == byte)
                });
                combos.push(Combo {
                    output,
                    slots,
                    own_output,
                });
            }
        }
        Ok(Self::assemble(combos, layout.slots.len()))
    }

    /// The program a layout's contexts are counted with: its combos while
    /// the policy types them, or none at all when only alternate fingers
    /// need the centred windows, whose key after the counted one rules like
    /// "␣e" read. Corpora below order 4 have no such windows.
    pub(crate) fn for_contexts(
        layout: &action_keys::Layout,
        policy: Policy,
        alts: crate::alt_fingers::AltPolicy,
        order: usize,
    ) -> Result<Option<Self>, String> {
        if policy.enabled && !layout.combos.is_empty() {
            Self::new(layout).map(Some)
        } else if order >= 4 && alts.active(layout) {
            Ok(Some(Self {
                ahead: true,
                ..Self::assemble(Vec::new(), layout.slots.len())
            }))
        } else {
            Ok(None)
        }
    }

    /// Whether alternate fingers alone asked for centred contexts, counted
    /// as `ChordPlan::ahead` describes.
    pub(crate) fn ahead(&self) -> bool {
        self.ahead
    }

    /// This program for an optimizer search: alternate fingers then count
    /// each key at its own context, which is several times faster and
    /// slightly less exact.
    pub(crate) fn for_search(self) -> Self {
        Self {
            ahead: false,
            ..self
        }
    }

    fn assemble(combos: Vec<Combo>, n: usize) -> Self {
        let mut first_bytes = [0u64; 4];
        let mut open_bytes = [0u64; 4];
        for combo in &combos {
            first_bytes[usize::from(combo.output[0]) / 64] |= 1 << (combo.output[0] % 64);
            for &byte in &combo.output[1..] {
                open_bytes[usize::from(byte) / 64] |= 1 << (byte % 64);
            }
        }
        Self {
            combos,
            slot_count: n,
            first_bytes,
            open_bytes,
            ahead: false,
        }
    }

    /// The combos only one of two programs over the same slots has, for
    /// `touches`; None when both have the same combos.
    pub(crate) fn changed(&self, other: &Program) -> Option<Program> {
        let only = |a: &Program, b: &Program| {
            a.combos
                .iter()
                .filter(|combo| !b.combos.contains(combo))
                .cloned()
                .collect::<Vec<_>>()
        };
        let mut combos = only(self, other);
        combos.extend(only(other, self));
        (!combos.is_empty()).then(|| Self::assemble(combos, self.slot_count))
    }

    /// Every byte some combo's output holds, in either case: `touches`
    /// finds no combo in a window holding none of them.
    pub(crate) fn output_bytes(&self) -> [u64; 4] {
        let mut bytes = [0; 4];
        for &byte in self.combos.iter().flat_map(|combo| &combo.output) {
            for byte in [byte.to_ascii_lowercase(), byte.to_ascii_uppercase()] {
                bytes[usize::from(byte) / 64] |= 1 << (byte % 64);
            }
        }
        bytes
    }

    /// Plan one context with every key at its declared slot.
    pub(crate) fn plan_window(&self, text: &[u8], left_open: bool) -> ChordPlan {
        self.plan_window_at(text, left_open, None)
    }

    /// Plan one context; `positions` maps each original slot ID to its
    /// current physical position, None meaning declared slots.
    pub(crate) fn plan_window_at(
        &self,
        text: &[u8],
        left_open: bool,
        positions: Option<&[usize]>,
    ) -> ChordPlan {
        assert!(
            text.len() <= PLAN_LEN,
            "chord plans cover at most five bytes"
        );
        let len = text.len();
        let mut lower = [0u8; PLAN_LEN];
        let mut starts = false;
        for (target, &byte) in lower.iter_mut().zip(text) {
            *target = byte.to_ascii_lowercase();
            starts |= has_byte(&self.first_bytes, *target);
        }
        let position = |slot: usize| positions.map_or(slot, |positions| positions[slot]);
        let mut plan = ChordPlan {
            len: len as u8,
            forced: [None; PLAN_LEN],
            chord: [false; PLAN_LEN],
            starts: [false; PLAN_LEN],
            open_prefix: 0,
            ahead: self.ahead,
        };
        let open = left_open
            && text
                .first()
                .is_some_and(|&byte| has_byte(&self.open_bytes, byte));
        if !starts && !open {
            return plan;
        }
        let mut start = 0;
        while start < len {
            // The longest matching combo wins; later ones break ties.
            let mut winner: Option<&Combo> = None;
            for combo in &self.combos {
                let end = start + combo.output.len();
                if end <= len
                    && same(&combo.output, &lower[start..end])
                    && winner.is_none_or(|previous| combo.output.len() >= previous.output.len())
                {
                    winner = Some(combo);
                }
            }
            let Some(combo) = winner else {
                start += 1;
                continue;
            };
            for (offset, &slot) in combo.slots.iter().enumerate() {
                plan.forced[start + offset] = Some(position(slot));
                plan.chord[start + offset] = true;
            }
            plan.starts[start] = true;
            start += combo.output.len();
        }
        if open {
            // A chord begun before this context may have taken its start.
            for combo in &self.combos {
                for prefix in 1..combo.output.len().min(len + 1) {
                    if same(
                        &combo.output[combo.output.len() - prefix..],
                        &text[..prefix],
                    ) {
                        plan.open_prefix = plan.open_prefix.max(prefix as u8);
                    }
                }
            }
        }
        plan
    }

    /// Whether any plan of this window can be non-empty, whatever the key
    /// positions: some output occurs in it, or (left open) some output's
    /// proper suffix begins it. Otherwise every plan forces, chords and
    /// hides nothing, and the window maps exactly as without combos.
    pub(crate) fn touches(&self, text: &[u8], left_open: bool) -> bool {
        if self.ahead {
            return true;
        }
        let mut buffer = [0u8; PLAN_LEN];
        let lower = &mut buffer[..text.len()];
        for (lower, byte) in lower.iter_mut().zip(text) {
            *lower = byte.to_ascii_lowercase();
        }
        // No output starts or (left open) ends inside: nothing to plan.
        if !lower.iter().any(|&byte| has_byte(&self.first_bytes, byte))
            && !(left_open
                && text
                    .first()
                    .is_some_and(|&byte| has_byte(&self.open_bytes, byte)))
        {
            return false;
        }
        let lower = &*lower;
        self.combos.iter().any(|combo| {
            let output = &combo.output[..];
            lower
                .windows(output.len())
                .any(|window| same(window, output))
                || left_open
                    && (1..output.len().min(text.len() + 1))
                        .any(|prefix| same(&output[output.len() - prefix..], &text[..prefix]))
        })
    }

    /// Contexts a move of these original slot IDs can change through combos,
    /// beyond those holding a moved key's own character (which every move
    /// remaps), as (bytes the context contains, first bytes of a left-open
    /// context). A combo typing its keys' own characters only changes
    /// contexts holding its output, which hold a moved participant's
    /// character, or ones that begin inside it; other combos change every
    /// context with an output byte.
    pub(crate) fn affected_by(&self, slots: &[usize]) -> ([u64; 4], [u64; 4]) {
        let mut bytes = [0u64; 4];
        let mut open = [0u64; 4];
        let add = |set: &mut [u64; 4], byte: u8| {
            set[usize::from(byte) / 64] |= 1 << (usize::from(byte) % 64);
        };
        for combo in &self.combos {
            if !combo.slots.iter().any(|slot| slots.contains(slot)) {
                continue;
            }
            if !combo.own_output {
                for &byte in &combo.output {
                    add(&mut bytes, byte.to_ascii_lowercase());
                    add(&mut bytes, byte.to_ascii_uppercase());
                }
                continue;
            }
            // `plan_window` matches an open prefix against the exact bytes.
            for &byte in &combo.output[1..] {
                add(&mut open, byte);
            }
        }
        (bytes, open)
    }
}

/// Straightforward reference planner, kept to check `plan_window_at`.
#[cfg(test)]
impl Program {
    /// Each position's winning combo and key, and whether that combo starts
    /// there.
    fn reference(
        &self,
        text: &[u8],
        position: impl Fn(usize) -> usize,
    ) -> Vec<Option<(&Combo, usize, bool)>> {
        let mut result = vec![None; text.len()];
        let mut start = 0;
        while start < text.len() {
            let mut winner: Option<&Combo> = None;
            for combo in &self.combos {
                let end = start + combo.output.len();
                if end <= text.len()
                    && combo
                        .output
                        .iter()
                        .zip(&text[start..end])
                        .all(|(expected, actual)| *expected == actual.to_ascii_lowercase())
                    && winner.map_or(true, |previous| combo.output.len() >= previous.output.len())
                {
                    winner = Some(combo);
                }
            }
            if let Some(combo) = winner {
                for (offset, &slot) in combo.slots.iter().enumerate() {
                    result[start + offset] = Some((combo, position(slot), offset == 0));
                }
                start += combo.output.len();
            } else {
                start += 1;
            }
        }
        result
    }

    pub(crate) fn forced(&self, text: &[u8]) -> Vec<Option<usize>> {
        self.forced_at(text, &(0..self.slot_count).collect::<Vec<_>>())
    }

    pub(crate) fn forced_at(&self, text: &[u8], positions: &[usize]) -> Vec<Option<usize>> {
        self.reference(text, |slot| positions[slot])
            .into_iter()
            .map(|entry| entry.map(|(_, key, _)| key))
            .collect()
    }

    pub(crate) fn chord_mask_at(&self, text: &[u8], positions: &[usize]) -> Vec<bool> {
        self.reference(text, |slot| positions[slot])
            .into_iter()
            .map(|entry| entry.is_some())
            .collect()
    }

    fn starts_at(&self, text: &[u8]) -> Vec<bool> {
        self.reference(text, |slot| slot)
            .into_iter()
            .map(|entry| entry.is_some_and(|(_, _, start)| start))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn corpus(text: &[u8], order: usize) -> crate::action_ngrams::NgramCorpus {
        let tables = action_keys::text_ngrams(text);
        let names = ["letters", "bigrams", "trigrams", "fourgrams", "fivegrams"];
        let mut json = String::from("{");
        for n in 0..order {
            if n > 0 {
                json.push(',');
            }
            json.push_str(&format!("\"{}\":{{", names[n]));
            for (i, (gram, frequency)) in tables[n].iter().enumerate() {
                if i > 0 {
                    json.push(',');
                }
                json.push_str(&format!("{}:{}", action_keys::quote(gram), frequency));
            }
            json.push('}');
        }
        json.push('}');
        crate::action_ngrams::NgramCorpus::from_text(&json, Path::new("combo-corpus.json")).unwrap()
    }

    fn slot(layout: &action_keys::Layout, label: &str) -> usize {
        layout
            .slots
            .iter()
            .position(|key| key.label == label)
            .unwrap()
    }

    fn corpus_for_boundary<F>(
        layout: &action_keys::Layout,
        stop: &std::sync::atomic::AtomicBool,
        progress: &std::sync::atomic::AtomicU64,
        effort: &F,
    ) -> crate::action_ngrams::Counts
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        corpus(b"waaev", 5)
            .evaluate_with_policy(
                layout,
                stop,
                progress,
                crate::alt_fingers::AltPolicy::layout(false),
                Policy::ALL,
                effort,
            )
            .unwrap()
    }

    #[test]
    fn skipgrams_after_a_combo_letter_keep_their_movement() {
        // "ph" is a combo; the h in "the" still starts its own skipgrams, and
        // a combo whose output never occurs changes nothing.
        let mut layout = action_keys::Layout::parse(
            "q w e r t  y u i o p\na s d f g  h j k l ;\nz x c v b  n m , . /\nthumbs: space\n",
            Path::new("combo.dat"),
        )
        .unwrap();
        let stop = std::sync::atomic::AtomicBool::new(false);
        let progress = std::sync::atomic::AtomicU64::new(0);
        let text = corpus(b"the phone then hot why ", 5);
        let effort = |_: Option<usize>, _: Option<usize>, _: usize| 0.0;
        let mut count = |output: Option<&str>| {
            layout.combos = output.map_or_else(Vec::new, |output| {
                let json = format!(r#"[{{"keys":["a","s"],"output":"{output}"}}]"#);
                let Json::Array(values) = crate::parse_json(&json).unwrap() else {
                    unreachable!()
                };
                values
            });
            text.evaluate_with_policy(
                &layout,
                &stop,
                &progress,
                crate::alt_fingers::AltPolicy::layout(false),
                Policy::from(true),
                &effort,
            )
            .unwrap()
        };
        let plain = count(None);
        let unused = count(Some("zq"));
        assert_eq!(unused.skip, plain.skip);
        assert_eq!(unused.tables, plain.tables);
        let ph = count(Some("ph"));
        let h = slot(&layout, "h");
        let from_h = |counts: &crate::action_ngrams::Counts| -> f64 {
            counts
                .skip
                .iter()
                .filter(|(keys, _)| keys[0] == h)
                .map(|(_, f)| f)
                .sum()
        };
        // Only "phone" loses an h press (chorded); "the", "then", "hot" and
        // "why" keep theirs.
        assert!(from_h(&ph) > 0.0);
        assert!(
            (from_h(&plain) - from_h(&ph) - 1.0).abs() < 1e-9,
            "{} {}",
            from_h(&plain),
            from_h(&ph)
        );
        // "ph" is one chord press of a and s: "phone" skips from both to n.
        let (a, s, n) = (slot(&layout, "a"), slot(&layout, "s"), slot(&layout, "n"));
        for key in [a, s] {
            assert_eq!(ph.skip.get(&[key, n][..]), Some(&1.0), "{key}");
        }
    }

    #[test]
    fn combos_are_one_chord_press() {
        let mut layout = action_keys::Layout::parse(
            "q w e r t  y u i o p\na s d f g  h j k l ;\nz x c v b  n m , . /\nthumbs: space\n",
            Path::new("combo.dat"),
        )
        .unwrap();
        let key = |label: &str| slot(&layout, label);
        let (e, d, k, c, r, f, g) = (
            key("e"),
            key("d"),
            key("k"),
            key("c"),
            key("r"),
            key("f"),
            key("g"),
        );
        // Fat-finger shapes only decide what the combo fitter may place.
        let slots = &layout.slots;
        assert!(fat_finger_pair(&slots[e], &slots[d]), "up-down");
        assert!(
            fat_finger_pair(&slots[f], &slots[g]),
            "left-right on the index"
        );
        assert!(!fat_finger_pair(&slots[r], &slots[g]), "diagonal");
        assert!(!fat_finger_pair(&slots[e], &slots[c]), "two rows apart");
        assert!(!fat_finger_pair(&slots[d], &slots[f]), "different fingers");

        // e above d types its own bigram; d and k, on two hands, type "je".
        let Json::Array(values) = crate::parse_json(
            r#"[{"keys":["e","d"],"output":"ed"},{"keys":["d","k"],"output":"je"}]"#,
        )
        .unwrap() else {
            unreachable!()
        };
        layout.combos = values;
        let identity: Vec<usize> = (0..layout.slots.len()).collect();
        let program = Program::new(&layout).unwrap();
        // Every combo is a chord on its keys, wherever they are.
        assert_eq!(program.forced(b"ed"), vec![Some(e), Some(d)]);
        assert_eq!(program.chord_mask_at(b"ed", &identity), vec![true, true]);
        assert_eq!(program.forced(b"je"), vec![Some(d), Some(k)]);
        assert_eq!(program.chord_mask_at(b"je", &identity), vec![true, true]);

        let stop = std::sync::atomic::AtomicBool::new(false);
        let progress = std::sync::atomic::AtomicU64::new(0);
        let text = corpus(b"red bed ed fed jet jest ", 5);
        let effort = |_: Option<usize>, _: Option<usize>, _: usize| 0.0;
        let count = |layout: &action_keys::Layout| {
            text.evaluate_with_policy(
                layout,
                &stop,
                &progress,
                crate::alt_fingers::AltPolicy::layout(false),
                Policy::from(true),
                &effort,
            )
            .unwrap()
            .tables
        };
        let mut plain = layout.clone();
        plain.combos.clear();
        let mut own = layout.clone();
        own.combos.truncate(1);
        let (chorded, plain) = (count(&own), count(&plain));
        // "ed" pressed at once: both keys still type, but e never moves to d.
        assert_eq!(chorded[0].get(&[e][..]), plain[0].get(&[e][..]));
        assert_eq!(chorded[0].get(&[d][..]), plain[0].get(&[d][..]));
        assert_eq!(plain[1].get(&[e, d][..]), Some(&4.0));
        assert_eq!(chorded[1].get(&[e, d][..]).copied().unwrap_or(0.0), 0.0);
        // "red": r moves to the chord, so to both e and d.
        assert_eq!(chorded[1].get(&[r, e][..]), Some(&1.0));
        assert_eq!(chorded[1].get(&[r, d][..]), Some(&1.0));
    }

    #[test]
    fn single_pass_plans_match_the_reference_functions() {
        let mut layout = action_keys::Layout::parse(
            "q w e r t  y u i o p\na s d f g  h j k l ;\nz x c v b  n m , . /\nthumbs: space\n",
            Path::new("combo.dat"),
        )
        .unwrap();
        // Overlapping outputs, a three-key chord, an uppercase-insensitive
        // match and a combo that only chords at some positions.
        let Json::Array(values) = crate::parse_json(
            r#"[
                {"keys":["e","d"],"output":"ed"},
                {"keys":["d","c"],"output":"de"},
                {"keys":["r","f"],"output":"rf"},
                {"keys":["a","q"],"output":"aq"}
            ]"#,
        )
        .unwrap() else {
            unreachable!()
        };
        layout.combos = values;
        let letters = b"edrfaqEDx";
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut next = |limit: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % limit as u64) as usize
        };
        let mut untouched = 0;
        {
            let program = Program::new(&layout).unwrap();
            let n = layout.slots.len();
            for _ in 0..4000 {
                let mut positions: Vec<usize> = (0..n).collect();
                for _ in 0..next(4) {
                    let (a, b) = (next(n), next(n));
                    positions.swap(a, b);
                }
                let len = 1 + next(PLAN_LEN);
                let text: Vec<u8> = (0..len).map(|_| letters[next(letters.len())]).collect();
                let left_open = next(2) == 0;
                let plan = program.plan_window_at(&text, left_open, Some(&positions));
                assert_eq!(plan.forced(), &program.forced_at(&text, &positions)[..]);
                assert_eq!(
                    &plan.mask()[..len],
                    &program.chord_mask_at(&text, &positions)[..]
                );
                let starts: Vec<bool> = (0..len).map(|at| plan.starts(at)).collect();
                assert_eq!(starts, program.starts_at(&text));
                // Windows no combo touches keep the plain mapping paths.
                if !program.touches(&text, left_open) {
                    untouched += 1;
                    assert!(plan.is_empty() && plan.mask() == [false; PLAN_LEN]);
                }
                let identity = program.plan_window(&text, left_open);
                assert_eq!(identity.forced(), &program.forced(&text)[..]);
            }
        }
        assert!(untouched > 500, "{untouched}");
    }

    #[test]
    fn parses_plans_remaps_and_rejects_ambiguous_labels() {
        let mut layout = action_keys::Layout::parse(
            "q w e r t  y u i o p\na s d f g  h j k l ;\nz x c v b  n m , . /\nthumbs: space\n",
            Path::new("combo.dat"),
        )
        .unwrap();
        let Json::Array(values) = crate::parse_json(
            r#"[
                {"keys":["a","u"],"output":"au"},
                {"keys":["u","p"],"output":"up"},
                {"keys":["s","w"],"output":"au"}
            ]"#,
        )
        .unwrap() else {
            unreachable!()
        };
        layout.combos = values;
        let program = Program::new(&layout).unwrap();
        let slot = |label: &str| {
            layout
                .slots
                .iter()
                .position(|key| key.label == label)
                .unwrap()
        };
        // Matching ignores case; the later of two equal outputs wins, and a
        // combo starts only where the previous one ended.
        assert_eq!(
            program.forced(b"AUPAUx"),
            vec![
                Some(slot("s")),
                Some(slot("w")),
                None,
                Some(slot("s")),
                Some(slot("w")),
                None,
            ]
        );
        assert_eq!(
            program.forced(b"xup"),
            vec![None, Some(slot("u")), Some(slot("p"))]
        );
        let positions: Vec<_> = (0..layout.slots.len()).rev().collect();
        assert_eq!(
            program.forced_at(b"au", &positions),
            vec![Some(positions[slot("s")]), Some(positions[slot("w")])]
        );

        let stop = std::sync::atomic::AtomicBool::new(false);
        let progress = std::sync::atomic::AtomicU64::new(0);
        let effort = |_: Option<usize>, _: Option<usize>, _: usize| 0.0;
        let ordinary = corpus(b"au", 5)
            .evaluate_with_policy(
                &layout,
                &stop,
                &progress,
                crate::alt_fingers::AltPolicy::layout(false),
                Policy::ALL,
                &effort,
            )
            .unwrap();
        assert_eq!(ordinary.presses, 1.0);
        assert_eq!(ordinary.characters, 2.0);
        assert_eq!(ordinary.tables[0].get(&[slot("s")][..]), Some(&1.0));
        assert_eq!(ordinary.tables[0].get(&[slot("w")][..]), Some(&1.0));
        assert!(ordinary.tables[1].is_empty());
        let short_source_error = corpus(b"au", 3)
            .evaluate_with_policy(
                &layout,
                &stop,
                &progress,
                crate::alt_fingers::AltPolicy::layout(false),
                Policy::ALL,
                &effort,
            )
            .unwrap_err();
        assert!(short_source_error.contains("through order 4"));

        // Capped contexts fall back to shorter ones instead of disappearing.
        // No source file is reread.
        let capped = crate::action_ngrams::NgramCorpus::from_text_with_limits(
            r#"{"letters":{"a":1,"u":1,"e":2,"x":1,"q":1},
                "bigrams":{"au":1,"ue":1,"ex":1,"xq":1},
                "trigrams":{"aue":1,"exq":1},"fourgrams":{},"fivegrams":{}}"#,
            Path::new("nonexistent-inline-source.json"),
            crate::NgramLimits {
                trigrams: Some(1),
                ..crate::NgramLimits::default()
            },
        )
        .unwrap()
        .evaluate_with_policy(
            &layout,
            &stop,
            &progress,
            crate::alt_fingers::AltPolicy::layout(false),
            Policy::ALL,
            &effort,
        )
        .unwrap();
        // Every character keeps its count: "au" is one chord, and x and q,
        // whose capped trigram is gone, fall back to their bigrams.
        assert_eq!(capped.presses, 5.0);
        assert_eq!(capped.characters, 6.0);
        assert_eq!(capped.tables[0].values().sum::<f64>(), 6.0);
        assert_eq!(capped.tables[0].get(&[slot("e")][..]), Some(&2.0));
        // The chord moves to e from both its keys.
        assert_eq!(capped.tables[1].len(), 4);
        for chorded in ["s", "w"] {
            assert_eq!(
                capped.tables[1].get(&[slot(chorded), slot("e")][..]),
                Some(&1.0)
            );
        }
        assert_eq!(
            capped.tables[1].get(&[slot("e"), slot("x")][..]),
            Some(&1.0)
        );
        assert_eq!(
            capped.tables[1].get(&[slot("x"), slot("q")][..]),
            Some(&1.0)
        );
        assert!(capped.tables[2].is_empty());

        let Json::Array(three) =
            crate::parse_json(r#"[{"keys":["a","s","d"],"output":"asd"}]"#).unwrap()
        else {
            unreachable!()
        };
        let mut triple = layout.clone();
        triple.combos = three;
        let error = Program::new(&triple).unwrap_err();
        assert!(error.contains("exactly 2 keys"), "{error}");

        layout.slots[1].label = "a".into();
        let error = Program::new(&layout).unwrap_err();
        assert!(error.contains("label \"a\" is ambiguous"), "{error}");
    }

    #[test]
    fn chords_count_once_preserve_usage_and_match_swapped_incremental_results() {
        let mut layout = action_keys::Layout::parse(
            crate::action_keys::test_layouts::SHORTHAND,
            Path::new("combo-magic.dat"),
        )
        .unwrap();
        let Json::Array(values) = crate::parse_json(
            r#"[
                {"keys":["q","a"],"output":"aa"},
                {"keys":["e","d"],"output":"un"}
            ]"#,
        )
        .unwrap() else {
            unreachable!()
        };
        layout.combos = values;
        layout.alt_fingers = crate::alt_fingers::parse_json(
            &crate::parse_json(
                r#"[
                    {"ngram":["a","a"],"fingers":["LP","LP"]},
                    {"ngram":["a",null,"n"],"fingers":["LR",null,"LR"]}
                ]"#,
            )
            .unwrap(),
        )
        .unwrap();

        let combo = Program::new(&layout).unwrap();
        let numeric_program = crate::action_fast::Program::new(&layout, 3).unwrap();
        let numeric_keys = crate::action_fast::KeyState::new(&numeric_program);
        let mut numeric = crate::action_fast::Mapper::new(&numeric_program, &numeric_keys);
        let mut reference = action_keys::WindowMapper::new(&layout, 3).unwrap();
        let effort = |_: Option<usize>, _: Option<usize>, _: usize| 0.0;

        let chords = |roots: &Vec<Option<usize>>| -> Vec<bool> {
            roots.iter().map(Option::is_some).collect()
        };
        // Start with an ordinary magic mapping, then force identical text onto
        // combo roots. Repeating both calls checks that forced contexts neither
        // reuse nor poison prefix/action history in either mapper.
        for (text, forced) in [
            (b"aa".as_slice(), None),
            (b"aa".as_slice(), Some(combo.forced(b"aa"))),
            (b"aun".as_slice(), Some(combo.forced(b"aun"))),
            (b"aa".as_slice(), None),
        ] {
            let fast = match &forced {
                Some(roots) => numeric.map_chords(text, roots, &chords(roots), &effort),
                None => numeric.map(text, &effort),
            };
            let slow = match &forced {
                Some(roots) => reference.map_chords(text, roots, &chords(roots), &effort),
                None => reference.map(text, &effort),
            };
            assert_eq!(fast.unwrap(), slow.unwrap());
        }

        let corpus = corpus(b"aa\naun\n", 5);
        let stop = std::sync::atomic::AtomicBool::new(false);
        let progress = std::sync::atomic::AtomicU64::new(0);
        let evaluate = |layout: &action_keys::Layout, alt_fingers, combos| {
            corpus
                .evaluate_with_policy(layout, &stop, &progress, alt_fingers, combos, &effort)
                .unwrap()
        };
        let enabled = evaluate(
            &layout,
            crate::alt_fingers::AltPolicy::layout(true),
            Policy::ALL,
        );

        let without_combos = evaluate(
            &layout,
            crate::alt_fingers::AltPolicy::layout(true),
            Policy::OFF,
        );
        let without_alts = evaluate(
            &layout,
            crate::alt_fingers::AltPolicy::layout(false),
            Policy::ALL,
        );
        let (q, a) = (slot(&layout, "q"), slot(&layout, "a"));
        let (e, d) = (slot(&layout, "e"), slot(&layout, "d"));

        // "aa" and "un" are one press each; the a between them is its own,
        // and moves to both keys of the "un" chord.
        assert_eq!(enabled.presses, 3.0);
        assert_eq!(enabled.characters, 5.0);
        assert_eq!(enabled.tables[0].values().sum::<f64>(), 5.0);
        for (key, count) in [(q, 1.0), (a, 2.0), (e, 1.0), (d, 1.0)] {
            assert_eq!(enabled.tables[0].get(&[key][..]), Some(&count));
        }
        assert_eq!(enabled.tables[1].len(), 2);
        for key in [e, d] {
            assert_eq!(enabled.tables[1].get(&[a, key][..]), Some(&1.0));
        }
        assert!(enabled.tables[2].is_empty());
        assert!(enabled.skip.is_empty());
        assert_eq!(without_combos.presses, 5.0);
        assert_eq!(enabled.characters, without_combos.characters);
        assert_eq!(enabled.tables, without_alts.tables);
        assert!(without_alts.finger_tables.is_none());
        let finger_unigrams = &enabled.finger_tables.as_ref().unwrap()[0];
        assert!(finger_unigrams.keys().any(|pattern| {
            pattern.slots == [q] && pattern.fingers == [layout.slots[q].finger]
        }));

        // "waaev": w, the "aa" chord, e and v. Moves into and out of the
        // chord count once per chord key; a skip across it counts once, and
        // its trigram once in halves.
        let boundary_counts = corpus_for_boundary(&layout, &stop, &progress, &effort);
        let (w, v) = (slot(&layout, "w"), slot(&layout, "v"));
        assert_eq!(boundary_counts.presses, 4.0);
        assert_eq!(boundary_counts.characters, 5.0);
        let bigrams = &boundary_counts.tables[1];
        assert_eq!(bigrams.len(), 5);
        for pair in [[w, q], [w, a], [q, e], [a, e], [e, v]] {
            assert_eq!(bigrams.get(&pair[..]), Some(&1.0), "{pair:?}");
        }
        let trigrams = &boundary_counts.tables[2];
        assert_eq!(trigrams.get(&[w, q, e][..]), Some(&0.5));
        assert_eq!(trigrams.get(&[w, a, e][..]), Some(&0.5));
        assert_eq!(trigrams.get(&[q, e, v][..]), Some(&1.0));
        assert_eq!(trigrams.get(&[a, e, v][..]), Some(&1.0));
        for pair in [[w, e], [q, v], [a, v]] {
            assert_eq!(boundary_counts.skip.get(&pair[..]), Some(&1.0), "{pair:?}");
        }
        assert_eq!(boundary_counts.skip.values().sum::<f64>(), 3.0);

        let fractional = crate::action_ngrams::NgramCorpus::from_text(
            r#"{"letters":{"a":0.5},"bigrams":{"aa":0.25},"trigrams":{},"fourgrams":{},"fivegrams":{}}"#,
            Path::new("fractional.json"),
        )
        .unwrap()
        .evaluate_with_policy(&layout, &stop, &progress, crate::alt_fingers::AltPolicy::layout(false), Policy::ALL, &effort)
        .unwrap();
        assert_eq!(fractional.presses, 0.25);
        assert_eq!(fractional.characters, 0.5);
        assert_eq!(fractional.tables[0].values().sum::<f64>(), 0.5);
        assert!(fractional.tables[1].is_empty());

        // Combo keys follow moved bindings while their physical geometry stays
        // at the destination slots, matching editor/optimizer swap semantics.
        layout.swap(q, a);
        let moved = evaluate(
            &layout,
            crate::alt_fingers::AltPolicy::layout(true),
            Policy::ALL,
        );
        assert_eq!(moved.tables[0].get(&[a][..]), Some(&1.0));
        assert_eq!(moved.tables[0].get(&[q][..]), Some(&2.0));
        for key in [e, d] {
            assert_eq!(moved.tables[1].get(&[q, key][..]), Some(&1.0));
        }

        // The optimizer's incremental combo remapping must produce the same
        // physical state as rebuilding the moved layout, then restore its
        // original checkpoint and mapped tails exactly.
        layout.swap(q, a);
        let corpus = self::corpus(b"aa\naun\ntev\n", 5);
        let geometry = crate::Geometry::new(crate::action_ui::physical_keys(&layout));
        let mut cache = crate::action_ngrams::Incremental::new_with_policy(
            &corpus,
            &layout,
            geometry.clone(),
            &stop,
            crate::alt_fingers::AltPolicy::layout(true),
            Policy::ALL,
            &effort,
        )
        .unwrap();
        cache.enable_mana2();
        let baseline = cache.numeric_state();
        assert_eq!(baseline.raw_totals().1, [8.0, 4.0, 1.0, 1.0]);
        let baseline_tails = cache.physical_test_state().2.to_vec();
        let baseline_raw = format!("{:?}", baseline.raw_totals().0);
        let baseline_totals = baseline.raw_totals().1.map(f64::to_bits);
        let identity: Vec<_> = (0..layout.slots.len()).collect();
        let mut proposal = crate::action_ngrams::Proposal::new();
        cache
            .propose_swap(q, a, &stop, &effort, &mut proposal)
            .unwrap();
        // Combo swaps remap only contexts holding moved bytes or the outputs
        // of chords whose participants moved, not every context.
        assert!(!proposal.all_contexts);
        cache.commit(&mut proposal);

        let mut swapped = layout.clone();
        swapped.swap(q, a);
        let mut fresh = crate::action_ngrams::Incremental::new_with_policy(
            &corpus,
            &swapped,
            geometry,
            &stop,
            crate::alt_fingers::AltPolicy::layout(true),
            Policy::ALL,
            &effort,
        )
        .unwrap();
        fresh.enable_mana2();
        let (actual_raw, actual_totals, actual_tails) = cache.physical_test_state();
        let (fresh_raw, fresh_totals, fresh_tails) = fresh.physical_test_state();
        assert_eq!(actual_tails, fresh_tails);
        assert_eq!(format!("{actual_raw:?}"), format!("{fresh_raw:?}"));
        assert_eq!(
            actual_totals.map(f64::to_bits),
            fresh_totals.map(f64::to_bits)
        );

        cache
            .restore(&identity, Some(&baseline), &stop, &effort)
            .unwrap();
        let (restored_raw, restored_totals, restored_tails) = cache.physical_test_state();
        assert_eq!(restored_tails, baseline_tails);
        assert_eq!(format!("{restored_raw:?}"), baseline_raw);
        assert_eq!(restored_totals.map(f64::to_bits), baseline_totals);
    }
}
