//! Contextual alternate fingerings. Rules name keys, by the letter on them or
//! by position, while the existing mapper supplies the actual physical
//! presses. No keys move.
//! Null ngram entries match one character; null fingers retain the usual finger.
//! Longer rules win, then rules with more literal characters, then later rules.
//! A rule does not apply where a key its `_` matches already uses, next to it,
//! the finger the rule assigns.
//! Rules never cross an unmapped character or reset. Only retained context is
//! known: this is an ngram estimate, not a simulation with unlimited lookahead.

use crate::{action_keys, Json, Key};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Rule {
    pub(crate) ngram: Vec<Option<u8>>,
    pub(crate) fingers: Vec<Option<usize>>,
}

const FINGERS: [&str; 10] = ["LP", "LR", "LM", "LI", "RI", "RM", "RR", "RP", "LT", "RT"];

pub(crate) fn parse_json(value: &Json) -> Result<Vec<Rule>, String> {
    let Json::Array(values) = value else {
        return Err("alts must be an array".into());
    };
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let error = |message: &str| format!("alts[{}]: {message}", index);
            let Json::Object(fields) = value else {
                return Err(error("expected an object"));
            };
            if fields
                .keys()
                .any(|key| !matches!(key.as_str(), "ngram" | "fingers"))
            {
                return Err(error("only ngram and fingers are supported"));
            }
            let Some(Json::Array(chars)) = fields.get("ngram") else {
                return Err(error("ngram must be an array"));
            };
            let Some(Json::Array(fingers)) = fields.get("fingers") else {
                return Err(error("fingers must be an array"));
            };
            if !matches!(chars.len(), 2 | 3) || fingers.len() != chars.len() {
                return Err(error("ngram and fingers must have the same length, 2 or 3"));
            }
            let ngram = chars
                .iter()
                .map(|value| match value {
                    Json::Null => Ok(None),
                    Json::String(text)
                        if text.len() == 1
                            && (text.as_bytes()[0].is_ascii_graphic() || text == " ") =>
                    {
                        Ok(Some(text.as_bytes()[0].to_ascii_lowercase()))
                    }
                    _ => Err(error(
                        "ngram entries must be one printable ASCII character, space, or null",
                    )),
                })
                .collect::<Result<Vec<_>, _>>()?;
            let fingers = fingers
                .iter()
                .map(|value| match value {
                    Json::Null => Ok(None),
                    Json::String(name) => FINGERS
                        .iter()
                        .position(|finger| name.eq_ignore_ascii_case(finger))
                        .map(Some)
                        .ok_or_else(|| error("unknown finger; use LP LR LM LI LT RI RM RR RP RT")),
                    _ => Err(error("finger entries must be finger names or null")),
                })
                .collect::<Result<Vec<_>, _>>()?;
            if ngram.iter().all(Option::is_none) || fingers.iter().all(Option::is_none) {
                return Err(error(
                    "a rule needs a literal character and an assigned finger",
                ));
            }
            Ok(Rule { ngram, fingers })
        })
        .collect()
}

pub(crate) fn resolve(
    rules: &[Rule],
    text: &[u8],
    mapped: &[Option<usize>],
    default_fingers: &[usize],
) -> Vec<Option<usize>> {
    assert_eq!(text.len(), mapped.len());
    let mut result: Vec<_> = mapped
        .iter()
        .map(|slot| slot.map(|id| default_fingers[id]))
        .collect();
    apply(rules, text, mapped, &mut result);
    result
}

/// Overwrite `result`, which starts as each key's own finger, with the
/// fingers of matching rules: longer rules win, then more named keys, then
/// later rules. Windows hold at most eight keys.
fn apply(rules: &[Rule], text: &[u8], mapped: &[Option<usize>], result: &mut [Option<usize>]) {
    let mut priority = [None; 8];
    assert!(text.len() <= priority.len());
    for (index, rule) in rules.iter().enumerate() {
        apply_rule(index, rule, None, text, mapped, result, &mut priority);
    }
}

type Priority = Option<(usize, usize, usize)>;

/// `apply` for one rule, at `start` alone or at every start.
fn apply_rule(
    index: usize,
    rule: &Rule,
    start: Option<usize>,
    text: &[u8],
    mapped: &[Option<usize>],
    result: &mut [Option<usize>],
    priority: &mut [Priority; 8],
) {
    let len = rule.ngram.len();
    let weight = (
        len,
        rule.ngram.iter().filter(|entry| entry.is_some()).count(),
        index,
    );
    if len > text.len() {
        return;
    }
    let starts = match start {
        Some(start) if start + len <= text.len() => start..start + 1,
        Some(_) => return,
        None => 0..text.len() - len + 1,
    };
    for start in starts {
        if (0..len).any(|offset| {
            mapped[start + offset].is_none()
                || rule.ngram[offset]
                    .is_some_and(|ch| ch != text[start + offset].to_ascii_lowercase())
        }) {
            continue;
        }
        // A finger the key a `_` stands for already uses next to it is no
        // alternative: "e_␣ (LT RT)" leaves Space on its own finger in "er␣"
        // when r is on RT, instead of making a same-finger pair.
        let clashes = (0..len).any(|offset| {
            rule.ngram[offset].is_none()
                && [offset.wrapping_sub(1), offset + 1].into_iter().any(|next| {
                    next < len
                        && rule.fingers[next].is_some()
                        && rule.fingers[next] == result[start + offset]
                })
        });
        if clashes {
            continue;
        }
        for (offset, &finger) in rule.fingers.iter().enumerate() {
            let position = start + offset;
            if let Some(finger) = finger {
                if priority[position].map_or(true, |previous| weight >= previous) {
                    result[position] = Some(finger);
                    priority[position] = Some(weight);
                }
            }
        }
    }
}

/// `[alts]`: rules like a layout's own `alts`, but naming key positions by
/// the QWERTY key in the same place instead of characters, so they stay put
/// while optimizers move letters. Rows are top, home and bottom; columns 1-10.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AltRules {
    pub(crate) rowstag: Vec<Rule>,
    pub(crate) colstag: Vec<Rule>,
}

const QWERTY: [&[u8; 10]; 3] = [b"qwertyuiop", b"asdfghjkl;", b"zxcvbnm,./"];

/// Names each main position by the letter `keys` put there, for display.
pub(crate) fn position_names<'a>(
    keys: impl Iterator<Item = (i8, i8, String)> + 'a,
) -> impl Fn(u8) -> String + 'a {
    let mut names = vec![String::from("·"); 31];
    for (row, col, name) in keys {
        let code = position_code(row, col);
        if code != 0 && !name.is_empty() {
            names[usize::from(code)] = name;
        }
    }
    move |code| names[usize::from(code)].clone()
}

/// A slot's position code: `position_code` on the main block, and 128 up for
/// thumb keys by thumb column, so a layout's own rules can pin thumb keys.
/// Matching lowercases ASCII, so thumb codes stay above it.
fn slot_code(slot: &action_keys::Slot) -> u8 {
    if slot.main {
        position_code(slot.row, slot.col)
    } else {
        u8::try_from(160 + i16::from(slot.col))
            .ok()
            .filter(|&code| code >= 128)
            .unwrap_or(0)
    }
}

/// Positions are encoded as bytes 1-30 so `resolve` can match them as text.
fn position_code(row: i8, col: i8) -> u8 {
    if (0..3).contains(&row) && (0..10).contains(&col) {
        1 + row as u8 * 10 + col as u8
    } else {
        0
    }
}

impl AltRules {
    /// Groups of patterns followed by their fingers, as in stray's notes:
    /// `rt r_t (LM LI) gt (LI LM)`. A pattern has two or three QWERTY
    /// position names or `_` for any key; its fingers cover the named
    /// positions in order, `.` keeping a position's normal finger.
    pub(crate) fn parse(text: &str) -> Result<Vec<Rule>, String> {
        let key = |byte| {
            (0..3).find_map(|row| {
                let col = QWERTY[row].iter().position(|&key| key == byte)?;
                Some(position_code(row as i8, col as i8))
            })
        };
        Self::parse_with(text, key, "use QWERTY key names or _", &FINGERS[..8])
    }

    /// `parse` for letters instead of positions, as stray writes a layout's
    /// own rules in its notes: `sg s_g (LM LI) · gs g_s (LI LM)`. `␣` names
    /// Space, and thumb keys may take `LT` or `RT`: `e␣ (LT RT)`.
    pub(crate) fn parse_letters(text: &str) -> Result<Vec<Rule>, String> {
        let key = |byte: u8| (byte.is_ascii_graphic() || byte == b' ').then_some(byte);
        Self::parse_with(
            &text.replace('·', " "),
            key,
            "use one character per key, ␣ or _",
            &FINGERS,
        )
    }

    /// `parse` with `key` naming each pattern character; `hint` explains a
    /// character it rejects. `fingers` are the names a list may use.
    fn parse_with(
        text: &str,
        key: impl Fn(u8) -> Option<u8>,
        hint: &str,
        fingers: &[&str],
    ) -> Result<Vec<Rule>, String> {
        let fingers_allowed = fingers;
        let mut rules = Vec::new();
        let mut pending: Vec<Vec<Option<u8>>> = Vec::new();
        let spaced = text.replace('(', " ( ").replace(')', " ) ");
        let mut tokens = spaced.split_whitespace();
        while let Some(token) = tokens.next() {
            if token != "(" {
                let pattern = token
                    .chars()
                    .map(|ch| match ch {
                        '_' => Some(None),
                        '␣' | '␠' => key(b' ').map(Some),
                        ch if ch.is_ascii() => key((ch as u8).to_ascii_lowercase()).map(Some),
                        _ => None,
                    }
                    .ok_or_else(|| format!("{token}: {hint}")))
                    .collect::<Result<Vec<_>, _>>()?;
                if !matches!(pattern.len(), 2 | 3) || pattern.iter().all(Option::is_none) {
                    return Err(format!("{token}: a pattern names two or three keys"));
                }
                pending.push(pattern);
                continue;
            }
            let mut fingers = Vec::new();
            for token in tokens.by_ref() {
                if token == ")" {
                    break;
                }
                fingers.push(match token {
                    "." => None,
                    _ => Some(
                        fingers_allowed
                            .iter()
                            .position(|name| name.eq_ignore_ascii_case(token))
                            .ok_or_else(|| format!("{token}: use {} or .", fingers_allowed.join(" ")))?,
                    ),
                });
            }
            if pending.is_empty() || fingers.iter().all(Option::is_none) {
                return Err("each (fingers) list follows patterns and assigns a finger".into());
            }
            for ngram in pending.drain(..) {
                if ngram.iter().flatten().count() != fingers.len() {
                    return Err("give one finger per named key, e.g. r_t (LM LI)".into());
                }
                let mut assigned = fingers.iter();
                let fingers = ngram
                    .iter()
                    .map(|entry| entry.and_then(|_| *assigned.next().unwrap()))
                    .collect();
                rules.push(Rule { ngram, fingers });
            }
        }
        if !pending.is_empty() {
            return Err("patterns at the end need a (fingers) list".into());
        }
        Ok(rules)
    }

    /// The `parse` form of `rules`; consecutive rules with the same fingers
    /// share one group.
    pub(crate) fn text(rules: &[Rule]) -> String {
        Self::text_with(rules, |code| {
            let index = usize::from(code - 1);
            (QWERTY[index / 10][index % 10] as char).to_string()
        })
    }

    /// A layout's own letter rules in the `parse` form.
    pub(crate) fn letters(rules: &[Rule]) -> String {
        Self::text_with(rules, |letter| match letter {
            b' ' => "␣".into(),
            _ => char::from(letter).to_string(),
        })
    }

    /// `text` with each position named by `name`, e.g. the letter a layout
    /// puts there.
    pub(crate) fn text_with(rules: &[Rule], name: impl Fn(u8) -> String) -> String {
        let mut groups: Vec<(Vec<String>, String)> = Vec::new();
        for (pattern, fingers) in rules.iter().map(|rule| {
            let pattern: String = rule
                .ngram
                .iter()
                .map(|entry| match entry {
                    Some(code) => name(*code),
                    None => "_".into(),
                })
                .collect();
            let fingers: Vec<_> = rule
                .ngram
                .iter()
                .zip(&rule.fingers)
                .filter(|(entry, _)| entry.is_some())
                .map(|(_, finger)| finger.map_or(".", |finger| FINGERS[finger]))
                .collect();
            (pattern, fingers.join(" "))
        }) {
            match groups.last_mut() {
                Some((patterns, last)) if *last == fingers => patterns.push(pattern),
                _ => groups.push((vec![pattern], fingers)),
            }
        }
        groups
            .iter()
            .map(|(patterns, fingers)| format!("{} ({fingers})", patterns.join(" ")))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn is_empty(&self) -> bool {
        self.rowstag.is_empty() && self.colstag.is_empty()
    }

    /// A shared copy for `RollSettings`, which stays `Copy`; `None` without
    /// rules. Equal rule sets share one allocation.
    pub(crate) fn intern(self) -> Option<&'static AltRules> {
        static INTERNED: std::sync::Mutex<Vec<&'static AltRules>> =
            std::sync::Mutex::new(Vec::new());
        if self.is_empty() {
            return None;
        }
        let mut interned = INTERNED.lock().unwrap();
        if let Some(found) = interned.iter().find(|rules| ***rules == self) {
            return Some(*found);
        }
        let leaked: &'static AltRules = Box::leak(Box::new(self));
        interned.push(leaked);
        Some(leaked)
    }

    /// The rules for an action layout's board; see `for_keys`.
    pub(crate) fn for_layout(&self, layout: &action_keys::Layout) -> &[Rule] {
        let rows = |row: i8| {
            layout
                .slots
                .iter()
                .find(|s| s.main && s.row == row)
                .map(|s| s.row_offset)
        };
        let rowstag = (0..3).any(|row| rows(row).is_some_and(|offset| Some(offset) != rows(0)));
        if rowstag {
            &self.rowstag
        } else {
            &self.colstag
        }
    }

    /// The rules for `keys`: boards with row offsets use `rowstag`, all others
    /// (ortholinear included) `colstag`.
    pub(crate) fn for_keys(&self, keys: &[Key]) -> &[Rule] {
        let main = || keys.iter().filter(|key| key.main);
        let row_offset = |row: i8| main().find(|key| key.row == row).map(|key| key.row_offset);
        let rowstag = (0..3).any(|row| row_offset(row).is_some_and(|o| Some(o) != row_offset(0)));
        if rowstag {
            &self.rowstag
        } else {
            &self.colstag
        }
    }

    /// `for_keys` compiled for these keys.
    pub(crate) fn resolver(&'static self, keys: &[Key]) -> Resolver {
        let rules = self.for_keys(keys);
        let codes: Vec<u8> = keys
            .iter()
            .map(|key| {
                if key.main {
                    position_code(key.row, key.col)
                } else {
                    0
                }
            })
            .collect();
        let involved = codes
            .iter()
            .map(|code| *code != 0 && rules.iter().any(|rule| rule.ngram.contains(&Some(*code))))
            .collect();
        Resolver {
            rules,
            codes,
            involved,
        }
    }
}

/// Which alternate fingers an evaluation applies: `[typing] alt_fingers`, and
/// `[alts]` position rules, which replace layouts' own rules while set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct AltPolicy {
    pub(crate) enabled: bool,
    pub(crate) positions: Option<&'static AltRules>,
}

/// Ties a layout's own rules to the keys their letters are on now, so an
/// optimizer moving letters keeps the alternates on those keys. Rules naming
/// a letter no key types alone (or a key off the 3x10 block and thumbs) are
/// dropped.
pub(crate) fn pin(layout: &mut action_keys::Layout) {
    if layout.alt_spots {
        return;
    }
    layout.alt_fingers = spot_rules(layout);
    layout.alt_spots = true;
}

/// A layout's own rules naming the keys their letters are on now.
fn spot_rules(layout: &action_keys::Layout) -> Vec<Rule> {
    if layout.alt_spots {
        return layout.alt_fingers.clone();
    }
    let code = |letter: u8| {
        layout
            .slots
            .iter()
            .find(|slot| slot_letter(layout, slot) == Some(letter))
            .map(slot_code)
            .filter(|&code| code != 0)
    };
    map_rules(&layout.alt_fingers, code)
}

/// Undoes `pin`, naming each key by the letter on it now.
pub(crate) fn unpin(layout: &mut action_keys::Layout) {
    if !layout.alt_spots {
        return;
    }
    let letter = |code: u8| {
        layout
            .slots
            .iter()
            .find(|slot| slot_code(slot) == code)
            .and_then(|slot| slot_letter(layout, slot))
    };
    layout.alt_fingers = map_rules(&layout.alt_fingers, letter);
    layout.alt_spots = false;
}

/// The letter naming a slot in rules: its text, an adaptive key's fallback,
/// or else a magic key's one-character label (`*`).
fn slot_letter(layout: &action_keys::Layout, slot: &action_keys::Slot) -> Option<u8> {
    use action_keys::{Action, Binding, Emission};
    let text = match &slot.binding {
        Binding::Text(text) => text,
        Binding::Named(name) => match layout.actions.get(name) {
            Some(
                Action::Text(text)
                | Action::Rules {
                    fallback: Emission::Text(text),
                    ..
                },
            ) => text,
            _ => slot.label.as_bytes(),
        },
        Binding::Empty => return None,
    };
    (text.len() == 1 && (text[0].is_ascii_graphic() || text[0] == b' '))
        .then(|| text[0].to_ascii_lowercase())
}

fn map_rules(rules: &[Rule], map: impl Fn(u8) -> Option<u8>) -> Vec<Rule> {
    rules
        .iter()
        .filter_map(|rule| {
            let ngram = rule
                .ngram
                .iter()
                .map(|entry| entry.map_or(Some(None), |key| map(key).map(Some)))
                .collect::<Option<Vec<_>>>()?;
            Some(Rule {
                ngram,
                fingers: rule.fingers.clone(),
            })
        })
        .collect()
}

impl AltPolicy {
    /// Layouts' own rules only.
    pub(crate) fn layout(enabled: bool) -> Self {
        Self {
            enabled,
            positions: None,
        }
    }

    /// The rules this layout's evaluation applies, or `None` when none can
    /// change a finger. Rules carry each slot's position code and match the
    /// pressed keys; layouts' own rules name the keys their letters are on.
    pub(crate) fn compile(self, layout: &action_keys::Layout) -> Option<Compiled> {
        if !self.enabled {
            return None;
        }
        let codes = || layout.slots.iter().map(slot_code).collect();
        // A layout's own rules win over `[alts]`. Each letter names the key
        // it is on, as in the optimizers: another key typing that letter (a
        // magic key, or an adaptive swap) keeps its own finger.
        if !layout.alt_fingers.is_empty() {
            let rules = spot_rules(layout);
            return (!rules.is_empty()).then(|| Compiled::new(rules, Some(codes())));
        }
        let rules = self.positions?.for_layout(layout);
        (!rules.is_empty()).then(|| Compiled::new(rules.to_vec(), Some(codes())))
    }

    pub(crate) fn active(self, layout: &action_keys::Layout) -> bool {
        self.compile(layout).is_some()
    }
}

/// Alternate-finger rules ready for one layout's context windows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Compiled {
    rules: Vec<Rule>,
    /// Position code per slot for `[alts]` rules; `None` matches the text.
    codes: Option<Vec<u8>>,
    /// Characters or position codes some rule names: a window without one
    /// keeps every key's own finger.
    named: [u64; 4],
    /// Rules by their first key, when every rule names one: a window's
    /// matches can then only start where such a key is.
    by_first: Option<Vec<Vec<u16>>>,
}

impl Compiled {
    fn new(rules: Vec<Rule>, codes: Option<Vec<u8>>) -> Self {
        let mut named = [0; 4];
        for &key in rules.iter().flat_map(|rule| &rule.ngram).flatten() {
            named[usize::from(key) / 64] |= 1 << (key % 64);
        }
        let by_first = rules.iter().all(|rule| rule.ngram[0].is_some()).then(|| {
            let mut by_first = vec![Vec::new(); 256];
            for (index, rule) in rules.iter().enumerate() {
                by_first[usize::from(rule.ngram[0].unwrap())].push(index as u16);
            }
            by_first
        });
        Self {
            rules,
            codes,
            named,
            by_first,
        }
    }

    /// Whether rules match key positions rather than characters.
    pub(crate) fn positional(&self) -> bool {
        self.codes.is_some()
    }

    /// Whether moving keys between these slots can change which rules match
    /// outside the keys a context's own mapping depends on: only position
    /// rules naming one of the slots can. Rules matching characters never do.
    pub(crate) fn reaches(&self, slots: &[usize]) -> bool {
        let Some(codes) = &self.codes else {
            return false;
        };
        slots.iter().any(|&slot| {
            let code = codes[slot];
            code != 0
                && self
                    .rules
                    .iter()
                    .any(|rule| rule.ngram.contains(&Some(code)))
        })
    }

    /// `resolve` over one window, matching position rules against the codes
    /// of its pressed keys and layouts' own rules against its characters.
    /// Windows hold at most five keys; later entries stay `None`.
    pub(crate) fn resolve(
        &self,
        text: &[u8],
        mapped: &[Option<usize>],
        default_fingers: &[usize],
    ) -> [Option<usize>; 5] {
        let len = mapped.len();
        assert!(len <= 5 && text.len() == len);
        let mut keys = [0u8; 5];
        let mut result = [None; 5];
        let mut named = false;
        for at in 0..len {
            result[at] = mapped[at].map(|id| default_fingers[id]);
            keys[at] = match &self.codes {
                None => text[at].to_ascii_lowercase(),
                Some(codes) => mapped[at].map_or(0, |slot| codes[slot]),
            };
            named |= mapped[at].is_some()
                && self.named[usize::from(keys[at]) / 64] & 1 << (keys[at] % 64) != 0;
        }
        if !named {
            return result;
        }
        // Candidate (rule, start) pairs, applied in `apply`'s order.
        let mut candidates = [(0u16, 0u8); 32];
        let mut count = 0;
        if let Some(by_first) = &self.by_first {
            for (start, &key) in keys[..len].iter().enumerate() {
                for &rule in &by_first[usize::from(key)] {
                    // Most windows with a rule's first key miss its others.
                    let ngram = &self.rules[usize::from(rule)].ngram;
                    if start + ngram.len() > len
                        || ngram.iter().enumerate().any(|(offset, entry)| {
                            mapped[start + offset].is_none()
                                || entry.is_some_and(|key| key != keys[start + offset])
                        })
                    {
                        continue;
                    }
                    if count == candidates.len() {
                        apply(&self.rules, &keys[..len], mapped, &mut result[..len]);
                        return result;
                    }
                    candidates[count] = (rule, start as u8);
                    count += 1;
                }
            }
            candidates[..count].sort_unstable();
            let mut priority = [None; 8];
            for &(rule, start) in &candidates[..count] {
                let index = usize::from(rule);
                apply_rule(
                    index,
                    &self.rules[index],
                    Some(usize::from(start)),
                    &keys[..len],
                    mapped,
                    &mut result[..len],
                    &mut priority,
                );
            }
        } else {
            apply(&self.rules, &keys[..len], mapped, &mut result[..len]);
        }
        result
    }
}

/// `[alts]` rules compiled for one board.
#[derive(Clone, Debug)]
pub(crate) struct Resolver {
    rules: &'static [Rule],
    codes: Vec<u8>,
    involved: Vec<bool>,
}

impl Resolver {
    pub(crate) fn none() -> Self {
        Self {
            rules: &[],
            codes: Vec::new(),
            involved: Vec::new(),
        }
    }

    /// Keys of one pair or trigram with matching rules applied exactly as a
    /// layout's own rules are, but only within this n-gram, so every evaluator
    /// compiles the same tables.
    pub(crate) fn keys<const N: usize>(&self, keys: &[Key], ids: [usize; N]) -> [Key; N] {
        let out = ids.map(|id| keys[id]);
        if !ids
            .iter()
            .any(|&id| self.involved.get(id).copied().unwrap_or(false))
        {
            return out;
        }
        let text = ids.map(|id| self.codes[id]);
        let mapped: [Option<usize>; N] = std::array::from_fn(Some);
        let defaults = out.map(|key| key.finger);
        let fingers = resolve(self.rules, &text, &mapped, &defaults);
        std::array::from_fn(|i| match fingers[i] {
            Some(finger) if finger != out[i].finger => with_finger(out[i], finger),
            _ => out[i],
        })
    }
}

pub(crate) fn with_finger(mut key: Key, finger: usize) -> Key {
    key.finger = finger;
    key.main = finger < 8;
    (key.rank, key.hand) = if finger < 8 {
        action_keys::finger_geometry(finger)
    } else {
        (-1, (finger - 8) as i8)
    };
    key
}

pub(crate) fn to_json(rules: &[Rule]) -> Json {
    Json::Array(
        rules
            .iter()
            .map(|rule| {
                Json::Object(BTreeMap::from([
                    (
                        "ngram".into(),
                        Json::Array(
                            rule.ngram
                                .iter()
                                .map(|ch| {
                                    ch.map_or(Json::Null, |ch| {
                                        Json::String((ch as char).to_string())
                                    })
                                })
                                .collect(),
                        ),
                    ),
                    (
                        "fingers".into(),
                        Json::Array(
                            rule.fingers
                                .iter()
                                .map(|finger| {
                                    finger.map_or(Json::Null, |finger| {
                                        Json::String(FINGERS[finger].into())
                                    })
                                })
                                .collect(),
                        ),
                    ),
                ]))
            })
            .collect(),
    )
}

pub(crate) fn json_text(rules: &[Rule]) -> String {
    compact_json(&to_json(rules))
}

// DAT directives occupy one line. Reuse the application's string escaping.
pub(crate) fn compact_json(value: &Json) -> String {
    match value {
        Json::Object(fields) => format!(
            "{{{}}}",
            fields
                .iter()
                .map(|(key, value)| format!("{}:{}", crate::json_quote(key), compact_json(value)))
                .collect::<Vec<_>>()
                .join(",")
        ),
        Json::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(compact_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Json::String(text) => crate::json_quote(text),
        Json::Number(value) => value.to_string(),
        Json::Bool(value) => value.to_string(),
        Json::Null => "null".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contextual_rules_preserve_slots_and_do_not_cross_gaps() {
        let rules = parse_json(
            &crate::parse_json(
                r#"[
            {"ngram":["p",null,"g"],"fingers":["LM",null,"LI"]},
            {"ngram":["p","a","g"],"fingers":["LR",null,"LI"]}
        ]"#,
            )
            .unwrap(),
        )
        .unwrap();
        let mapped = [Some(0), Some(1), Some(2)];
        let defaults = [3, 7, 3];
        assert_eq!(
            resolve(&rules, b"pag", &mapped, &defaults),
            [Some(1), Some(7), Some(3)]
        );
        assert_eq!(
            resolve(&rules, b"pxg", &mapped, &defaults),
            [Some(2), Some(7), Some(3)]
        );
        assert_eq!(
            resolve(&rules, b"pxg", &[Some(0), None, Some(2)], &defaults),
            [Some(3), None, Some(3)]
        );
        assert_eq!(
            resolve(&rules, b"pad", &mapped, &defaults),
            [Some(3), Some(7), Some(3)]
        );
        assert_eq!(
            parse_json(&crate::parse_json(&json_text(&rules)).unwrap()).unwrap(),
            rules
        );
        assert!(parse_json(
            &crate::parse_json(r#"[{"ngram":["p","g"],"fingers":["wat",null]}]"#).unwrap()
        )
        .is_err());
    }
}
