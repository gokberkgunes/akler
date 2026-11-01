//! Text-producing keyboard actions. This module never equates output bytes with physical presses.
use std::collections::{BTreeMap, BTreeSet};

use std::fs;

use std::path::{Path, PathBuf};

use std::sync::atomic::{AtomicBool, Ordering};

pub const DEFAULT_STATE_LIMIT: usize = 500_000;

pub type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Binding {
    Empty,
    Text(Vec<u8>),
    Named(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Slot {
    pub binding: Binding,
    pub label: String,
    pub row: i8,
    pub col: i8,
    pub row_offset: i16,
    pub column_offset: i16,
    pub finger: usize,
    pub rank: i8,
    pub hand: i8,
    pub main: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Basis {
    Text,
    Press,
    SkipPress,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Emission {
    Text(Vec<u8>),
    Call(String),
    None,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Rules {
        basis: Basis,
        rules: BTreeMap<Vec<u8>, Emission>,
        fallback: Emission,
    },
    RepeatOutput,
    RepeatPreviousOutput,
    RepeatAction,
}

#[derive(Clone, Debug)]
pub struct Layout {
    pub name: String,
    pub path: PathBuf,
    pub slots: Vec<Slot>,
    pub actions: BTreeMap<String, Action>,
    pub left_outer: bool,
    pub right_outer: bool,
    pub(crate) action_mode: bool,
    /// Session-only contexts whose outputs refresh must refit in place.
    pub(crate) magic_rule_slots: BTreeMap<String, Vec<Vec<u8>>>,
    /// Presentation only: working outputs stay active until the refit starts.
    pub(crate) magic_refit_pending: BTreeSet<String>,
    pub(crate) alt_fingers: Vec<crate::alt_fingers::Rule>,
    /// `alt_fingers` name key positions instead of letters while an
    /// optimizer runs; see `alt_fingers::pin`.
    pub(crate) alt_spots: bool,
    pub(crate) combos: Vec<crate::Json>,
    pub(crate) source_warnings: Vec<String>,
}

fn err<T>(message: impl Into<String>) -> Result<T> {
    Err(message.into())
}

fn ascii(text: Vec<u8>, what: &str) -> Result<Vec<u8>> {
    if text.is_empty() || text.len() > 128 || !text.iter().all(|b| (32..=126).contains(b)) {
        return err(format!(
            "{what}: use 1–128 printable ASCII characters (Space is allowed)"
        ));
    }
    Ok(text)
}

pub fn quote(bytes: &[u8]) -> String {
    let mut out = String::from("\"");
    for &b in bytes {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            32..=126 => out.push(b as char),
            _ => out.push_str(&format!("\\u{:04x}", b)),
        }
    }
    out.push('"');
    out
}

// Single glyph identifiers describe physical bindings, so y and Y may emit
// the same normalized text through different rules. Longer action names keep
// their legacy case-insensitive spelling.
pub(crate) fn action_name(text: &str) -> String {
    let name = if text == "@" {
        text
    } else {
        text.strip_prefix('@').unwrap_or(text)
    };
    if name.chars().count() == 1 {
        name.to_string()
    } else {
        name.to_ascii_lowercase()
    }
}

/// Geometry is stored in thousandths of a key unit, without silent rounding.
pub(crate) fn geometry_offset(text: &str) -> Result<i16> {
    let value: f64 = text
        .parse()
        .map_err(|_| format!("invalid geometry offset {text:?}"))?;
    let scaled = value * 1000.0;
    if !value.is_finite() || value.abs() > 32.0 || (scaled - scaled.round()).abs() > 1e-9 {
        return err(
            "geometry offsets must be finite, within -32..32, and multiples of 0.001 key units",
        );
    }
    Ok(scaled.round() as i16)
}

pub(crate) fn finger_geometry(finger: usize) -> (i8, i8) {
    if finger < 4 {
        (finger as i8, 0)
    } else {
        ((7 - finger) as i8, 1)
    }
}

#[derive(Default)]
pub(crate) struct GeometrySettings {
    rows: Option<[i16; 3]>,
    columns: Option<Vec<i16>>,
    fingers: Option<Vec<Vec<usize>>>,
}

impl GeometrySettings {

    pub(crate) fn set_row_offsets(&mut self, rows: [i16; 3]) {
        self.rows = Some(rows);
    }

    pub(crate) fn set_column_offsets(&mut self, columns: Vec<i16>) {
        self.columns = Some(columns);
    }

    /// Fingers per main row, 0–3 left pinky to index, 4–7 right index to pinky.
    pub(crate) fn set_fingers(&mut self, rows: Vec<Vec<usize>>) {
        self.fingers = Some(rows);
    }

    pub(crate) fn validate(&self, shape: [usize; 3], minimum: i8, maximum: i8) -> Result<()> {
        if let Some(columns) = &self.columns {
            if columns.len() != (maximum - minimum + 1) as usize {
                return err(format!(
                    "column-offsets needs {} values for physical columns {minimum}..{maximum}",
                    maximum - minimum + 1
                ));
            }
        }
        if let Some(rows) = &self.fingers {
            if rows.len() != 3
                || rows
                    .iter()
                    .zip(shape)
                    .any(|(row, count)| row.len() != count)
            {
                return err("fingermap needs three rows matching the physical key counts");
            }
        }
        Ok(())
    }

    pub(crate) fn row_offset(&self, row: usize) -> Option<i16> {
        self.rows.map(|rows| rows[row])
    }

    pub(crate) fn column_offset(&self, column: i8, minimum: i8) -> Option<i16> {
        self.columns
            .as_ref()
            .map(|columns| columns[(column - minimum) as usize])
    }

    pub(crate) fn finger(&self, row: usize, column: usize) -> Option<usize> {
        self.fingers.as_ref().map(|rows| rows[row][column])
    }

    pub(crate) fn has_fingermap(&self) -> bool {
        self.fingers.is_some()
    }
}

fn binding(token: &str) -> Result<(Binding, String)> {
    let s = token.trim();
    if matches!(s, "~" | "blank" | "·") {
        return Ok((Binding::Empty, "·".into()));
    }
    if s.eq_ignore_ascii_case("space") || (s == "␠" || s == "␣") {
        return Ok((Binding::Text(vec![b' ']), "␠".into()));
    }
    // A display symbol for a named action, never a UTF-8 text emission.
    if s.chars().count() == 1 && !s.is_ascii() && !s.chars().any(char::is_control) {
        return Ok((Binding::Named(s.into()), s.into()));
    }
    if let Some(c) = s.strip_prefix("char:") {
        let b = ascii(c.as_bytes().to_vec(), "literal key")?;
        if b.len() != 1 {
            return err("char: requires one ASCII character");
        }
        let label = if c == "@" { "char:@" } else { c };
        return Ok((Binding::Text(b), label.into()));
    }
    if s.len() == 1 && s.as_bytes()[0].is_ascii_graphic() {
        return Ok((
            Binding::Text(vec![s.as_bytes()[0].to_ascii_lowercase()]),
            s.to_ascii_lowercase(),
        ));
    }
    let Some(name) = s.strip_prefix('@') else {
        return err(format!("key {s:?}: use one printable ASCII character, a single action symbol, ~, space, or an explicit @action"));
    };
    let name = action_name(name);
    if name.chars().count() == 1 && !name.is_ascii() && !name.chars().any(char::is_control) {
        return Ok((Binding::Named(name.clone()), name));
    }
    if name == "@" {
        return Ok((Binding::Named(name.clone()), name));
    }
    if name.len() == 1 && name.as_bytes()[0].is_ascii_punctuation() && name != "@" {
        return Ok((Binding::Named(name.clone()), name));
    }
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return err(format!("invalid action key {s:?}"));
    }
    Ok((Binding::Named(name.clone()), format!("@{name}")))
}

fn finger(col: i8) -> (usize, i8, i8) {
    match col {
        -1 | 0 => (0, 0, 0),
        1 => (1, 1, 0),
        2 => (2, 2, 0),
        3 | 4 => (3, 3, 0),
        5 | 6 => (4, 3, 1),
        7 => (5, 2, 1),
        8 => (6, 1, 1),
        9..=12 => (7, 0, 1),
        _ => unreachable!(),
    }
}

pub(crate) fn make_slot(token: &str, row: i8, col: i8) -> Result<Slot> {
    let (binding, label) = binding(token)?;
    let (finger, rank, hand) = finger(col);
    Ok(Slot {
        binding,
        label,
        row,
        col,
        row_offset: 0,
        column_offset: 0,
        finger,
        rank,
        hand,
        main: true,
    })
}

/// Add one main row's slots from its left- and right-hand key tokens.
/// Returns whether each hand has an outer column.
pub(crate) fn hand_row(
    out: &mut Vec<Slot>,
    row: i8,
    left: &[String],
    right: &[String],
    absolute: bool,
) -> Result<(bool, bool)> {
    let max_hand_width = if absolute && left.len() + right.len() == 13 {
        7
    } else {
        6
    };
    if !(5..=max_hand_width).contains(&left.len()) || !(5..=max_hand_width).contains(&right.len())
    {
        return err("each hand must have five or six columns (six or seven in an absolute 13-column row)");
    }
    let start = if left.len() == 6 && !absolute { -1 } else { 0 };
    for (j, t) in left.iter().enumerate() {
        out.push(make_slot(t, row, start + j as i8)?);
    }
    for (j, t) in right.iter().enumerate() {
        let col = if absolute { left.len() + j } else { 5 + j };
        out.push(make_slot(t, row, col as i8)?);
    }
    Ok((left.len() >= 6, right.len() >= 6))
}

/// What a layout file states, before `Layout::assemble` places the thumbs,
/// applies fingers and offsets, and installs the actions.
pub(crate) struct Parts {
    pub(crate) slots: Vec<Slot>,
    pub(crate) left_outer: bool,
    pub(crate) right_outer: bool,
    pub(crate) geometry: GeometrySettings,
    /// Key tokens per hand; `none` for a hand with no thumb keys.
    pub(crate) thumbs: [Vec<String>; 2],
    pub(crate) thumb_columns: Option<Vec<i8>>,
    pub(crate) actions: BTreeMap<String, Action>,
    /// Rules (`Some` context) and fallbacks (`None`) for defined actions.
    pub(crate) mappings: Vec<(String, Option<Vec<u8>>, Emission)>,
    pub(crate) action_mode: bool,
    pub(crate) alt_fingers: Option<Vec<crate::alt_fingers::Rule>>,
    pub(crate) combos: Option<Vec<crate::Json>>,
}

pub(crate) fn builtin_actions() -> BTreeMap<String, Action> {
    [
        ("repeat", Action::RepeatOutput),
        ("repeat-output", Action::RepeatOutput),
        ("repeat-previous-output", Action::RepeatPreviousOutput),
        ("repeat-action", Action::RepeatAction),
        ("again", Action::RepeatAction),
    ]
    .into_iter()
    .map(|(name, action)| (name.into(), action))
    .collect()
}

impl Layout {
    pub fn load(path: &Path) -> Result<Self> {
        let mut timing = crate::load_profile::LoadProfile::new("action layout read/parse");
        let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
        timing.mark("Layout read");
        let result = Self::parse(&text, path);
        timing.mark("Parse and definition validation");
        result
    }

    pub fn parse(text: &str, path: &Path) -> Result<Self> {
        if !crate::layout_io::is_json_layout(text) {
            return err("not a JSON or JSONC layout");
        }
        let mut layout = crate::layout_io::parse_json_layout(text, path)?;
        let unplaced = crate::alt_fingers::unplaced(&layout);
        layout.source_warnings.extend(unplaced);
        Ok(layout)
    }

    /// The layout `parts` describe: thumbs placed, fingers and offsets
    /// applied, actions installed and checked.
    pub(crate) fn assemble(parts: Parts, path: &Path) -> Result<Self> {
        let Parts {
            mut slots,
            left_outer,
            right_outer,
            geometry,
            mut thumbs,
            thumb_columns,
            mut actions,
            mappings,
            mut action_mode,
            alt_fingers,
            combos,
        } = parts;
        let has_space = slots.iter().any(|s| s.binding == Binding::Text(vec![b' ']))
            || thumbs
                .iter()
                .flatten()
                .any(|t| t.eq_ignore_ascii_case("space") || (t == "␠" || t == "␣"));
        let explicit_absence = thumbs.iter().flatten().any(|token| token == "none");
        if !has_space && !explicit_absence {
            match (thumbs[0].is_empty(), thumbs[1].is_empty()) {
                (true, true) => thumbs[0].push("space".into()),
                (false, true) => thumbs[1].push("space".into()),
                (true, false) => thumbs[0].push("space".into()),
                _ => {}
            }
        }
        slots.sort_by_key(|s| (s.row, s.col));
        let mut thumb_col = 0usize;
        for (hand, group) in thumbs.iter().enumerate() {
            if hand == 1 && thumb_col == 0 {
                thumb_col = 1;
            }
            for t in group {
                if t == "none" {
                    continue;
                }
                let (binding, label) = binding(t)?;
                slots.push(Slot {
                    binding,
                    label,
                    row: 3,
                    col: i8::try_from(thumb_col).map_err(|_| "too many thumb keys")?,
                    row_offset: 0,
                    column_offset: 0,
                    finger: 8 + hand,
                    rank: -1,
                    hand: hand as i8,
                    main: false,
                });
                thumb_col += 1;
            }
        }

        let shape = std::array::from_fn(|row| {
            slots
                .iter()
                .filter(|s| s.main && s.row == row as i8)
                .count()
        });
        let minimum = slots
            .iter()
            .filter(|s| s.main)
            .map(|s| s.col)
            .min()
            .unwrap_or(0);
        let maximum = slots
            .iter()
            .filter(|s| s.main)
            .map(|s| s.col)
            .max()
            .unwrap_or(0);
        if shape.iter().any(|&width| width == 13) && !geometry.has_fingermap() {
            return err("13-column layouts require an explicit fingermap");
        }
        geometry.validate(shape, minimum, maximum)?;
        let mut columns = [0; 3];
        for slot in slots.iter_mut().filter(|slot| slot.main) {
            let row = slot.row as usize;
            if let Some(offset) = geometry.row_offset(row) {
                slot.row_offset = offset;
            }
            if let Some(offset) = geometry.column_offset(slot.col, minimum) {
                slot.column_offset = offset;
            }
            if let Some(finger) = geometry.finger(row, columns[row]) {
                slot.finger = finger;
                (slot.rank, slot.hand) = finger_geometry(finger);
            }
            columns[row] += 1;
        }

        for (name, context, out) in mappings {
            let a = actions
                .get_mut(&name)
                .ok_or_else(|| format!("map/fallback refers to undefined action {name}"))?;
            match a {
                Action::Rules {
                    rules, fallback, ..
                } => {
                    if let Some(c) = context {
                        if c.is_empty() {
                            return err("empty context is not allowed; use fallback");
                        }
                        if rules.insert(c, out).is_some() {
                            return err(format!("duplicate rule in {name}"));
                        }
                    } else {
                        *fallback = out;
                    }
                }
                _ => return err(format!("{name} is not a mapping action")),
            }
        }
        for slot in &mut slots {
            if slot.binding == Binding::Text(vec![b'@']) && slot.label == "char:@" {
                slot.label = "@".into();
            }
        }
        if let Some(columns) = thumb_columns {
            let thumbs: Vec<_> = slots.iter_mut().filter(|slot| !slot.main).collect();
            if columns.len() != thumbs.len() {
                return err("thumb-columns needs one column for every thumb slot, left hand then right hand");
            }
            for (slot, col) in thumbs.into_iter().zip(columns) {
                slot.col = col;
            }
        }
        if slots.len() > 60 {
            return err("too many physical keys");
        }
        for slot in &slots {
            if let Binding::Named(n) = &slot.binding {
                if !actions.contains_key(n) {
                    return err(format!("undefined action @{n}"));
                }
            }
        }
        for a in actions.values() {
            if let Action::Rules {
                rules, fallback, ..
            } = a
            {
                for e in rules.values().chain(std::iter::once(fallback)) {
                    if let Emission::Call(n) = e {
                        if !actions.contains_key(n) {
                            return err(format!("undefined called action @{n}"));
                        }
                    }
                }
            }
        }
        // Explicit static cycles are errors. Dynamic repeat-action cycles are caught during resolution.
        for name in actions.keys() {
            check_calls(name, &actions, &mut Vec::new())?;
        }
        action_mode |= slots.iter().any(|s| matches!(s.binding, Binding::Named(_)));
        Ok(Self {
            name: path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            path: path.into(),
            slots,
            actions,
            left_outer,
            right_outer,
            action_mode,
            magic_rule_slots: BTreeMap::new(),
            magic_refit_pending: BTreeSet::new(),
            alt_fingers: alt_fingers.unwrap_or_default(),
            alt_spots: false,
            source_warnings: if combos.as_ref().is_some_and(|values| !values.is_empty()) {
                vec!["Combos are preserved; [typing] combos controls their scoring.".into()]
            } else {
                Vec::new()
            },
            combos: combos.unwrap_or_default(),
        })
    }

    pub fn extended(&self) -> bool {
        self.action_mode || !self.alt_fingers.is_empty() || !self.combos.is_empty()
    }

    #[cfg(test)]
    pub fn home(&self, i: usize) -> bool {
        let s = &self.slots[i];
        s.main && s.row == 1 && matches!(s.col, 0 | 1 | 2 | 3 | 6 | 7 | 8 | 9)
    }

    pub fn space(&self, i: usize) -> bool {
        self.slots[i].binding == Binding::Text(vec![b' '])
    }

    pub fn swap(&mut self, a: usize, b: usize) {
        if a == b {
            return;
        }
        let (lo, hi) = if a < b { (a, b) } else { (b, a) };
        let (left, right) = self.slots.split_at_mut(hi);
        std::mem::swap(&mut left[lo].binding, &mut right[0].binding);
        std::mem::swap(&mut left[lo].label, &mut right[0].label);
    }

    /// Reflect bindings on the existing board, including the thumb row.
    /// Geometry stays fixed; layout-owned alternate fingers change hands.
    pub(crate) fn mirror(&mut self) {
        let pinned = self.alt_spots;
        crate::alt_fingers::unpin(self);
        for row in 0..=3 {
            let mut slots: Vec<_> = self
                .slots
                .iter()
                .enumerate()
                .filter(|(_, slot)| slot.row == row)
                .map(|(index, _)| index)
                .collect();
            slots.sort_by_key(|&index| self.slots[index].col);
            for index in 0..slots.len() / 2 {
                self.swap(slots[index], slots[slots.len() - index - 1]);
            }
        }
        for rule in &mut self.alt_fingers {
            for finger in rule.fingers.iter_mut().flatten() {
                *finger = [7, 6, 5, 4, 3, 2, 1, 0, 9, 8][*finger];
            }
        }
        if pinned {
            crate::alt_fingers::pin(self);
        }
    }

    /// Whether each slot is a spare the optimizer adds (`with_spare_keys`):
    /// an empty key in an outer pinky column, or an empty thumb key.
    pub(crate) fn spare_slots(&self) -> Vec<bool> {
        let outer = |slot: &Slot| {
            let row: Vec<_> = self
                .slots
                .iter()
                .filter(|s| s.main && s.row == slot.row && s.hand == slot.hand)
                .map(|s| s.col)
                .collect();
            row.len() >= 6
                && Some(slot.col)
                    == if slot.hand == 0 {
                        row.iter().min().copied()
                    } else {
                        row.iter().max().copied()
                    }
        };
        self.slots
            .iter()
            .map(|slot| slot.binding == Binding::Empty && (!slot.main || outer(slot)))
            .collect()
    }

    /// This layout with empty keys added for the optimizer to show: an outer
    /// pinky column on each side that has none, and two thumb keys per hand.
    pub(crate) fn with_spare_keys(&self) -> Layout {
        let mut out = self.clone();
        for hand in 0..2i8 {
            let rows: Vec<Vec<&Slot>> = (0..3)
                .map(|row| {
                    self.slots
                        .iter()
                        .filter(|s| s.main && s.row == row && s.hand == hand)
                        .collect()
                })
                .collect();
            if rows.iter().any(|row| row.is_empty() || row.len() >= 6) {
                continue;
            }
            for row in rows {
                let edge = if hand == 0 {
                    row.iter().min_by_key(|s| s.col)
                } else {
                    row.iter().max_by_key(|s| s.col)
                };
                let mut slot = (**edge.unwrap()).clone();
                let Some(col) = slot.col.checked_add(if hand == 0 { -1 } else { 1 }) else {
                    continue;
                };
                slot.col = col;
                slot.finger = if hand == 0 { 0 } else { 7 };
                slot.rank = 0;
                slot.binding = Binding::Empty;
                slot.label = "·".into();
                out.slots.push(slot);
            }
            if hand == 0 {
                out.left_outer = true;
            } else {
                out.right_outer = true;
            }
        }
        for hand in 0..2i8 {
            let have = out.slots.iter().filter(|s| !s.main && s.hand == hand).count();
            for _ in have..2 {
                let cols = out.slots.iter().filter(|s| !s.main).map(|s| s.col);
                let col = if hand == 0 {
                    cols.min().map_or(0, |col| col.saturating_sub(1))
                } else {
                    cols.max().map_or(1, |col| col.saturating_add(1))
                };
                out.slots.push(Slot {
                    binding: Binding::Empty,
                    label: "·".into(),
                    row: 3,
                    col,
                    row_offset: 0,
                    column_offset: 0,
                    finger: 8 + hand as usize,
                    rank: -1,
                    hand,
                    main: false,
                });
            }
        }
        out.order_slots();
        out
    }

    /// Drops the spare keys `with_spare_keys` added that are still empty: a
    /// wholly empty outer column at -1 or 10, and empty thumbs.
    pub(crate) fn without_spare_keys(&self) -> Layout {
        let spare = self.spare_slots();
        let mut drop = vec![false; self.slots.len()];
        let mut dropped_outer = [false; 2];
        for hand in 0..2i8 {
            // Only the columns it adds: magic seeds shift columns to put
            // theirs at 0 and 11, and rows need those to be written back.
            let col = if hand == 0 { -1 } else { 10 };
            let outer: Vec<_> = (0..self.slots.len())
                .filter(|&i| {
                    spare[i]
                        && self.slots[i].main
                        && self.slots[i].hand == hand
                        && self.slots[i].col == col
                })
                .collect();
            if outer.len() == 3 {
                outer.iter().for_each(|&i| drop[i] = true);
                dropped_outer[hand as usize] = true;
            }
            for i in 0..self.slots.len() {
                if spare[i] && !self.slots[i].main && self.slots[i].hand == hand {
                    drop[i] = true;
                }
            }
        }
        let mut out = self.clone();
        let mut index = 0;
        out.slots.retain(|_| {
            index += 1;
            !drop[index - 1]
        });
        // A side keeps its outer column unless it was the one dropped. Keys
        // are on a side by position, as the parsers place them, not by hand:
        // a key may use a finger of the other hand.
        if dropped_outer[0] {
            out.left_outer = false;
        }
        if dropped_outer[1] {
            out.right_outer = false;
        }
        out.order_slots();
        out
    }

    /// Main keys by row and column, then left and right thumbs by column, as
    /// `text` writes them. Spare thumbs sit outside the others, so removing
    /// them leaves the original thumb columns.
    fn order_slots(&mut self) {
        self.slots
            .sort_by_key(|s| (!s.main, s.row, if s.main { 0 } else { s.hand }, s.col));
    }

    pub fn save_new(&self) -> Result<PathBuf> {
        let dir = self.path.parent().unwrap_or(Path::new("layouts"));
        crate::layout_export::save_numbered_jsonc(self, dir, &format!("{}-actions", self.name))
    }
}

fn check_calls(name: &str, defs: &BTreeMap<String, Action>, stack: &mut Vec<String>) -> Result<()> {
    if stack.iter().any(|n| n == name) {
        return err(format!(
            "recursive action calls: {} -> {name}",
            stack.join(" -> ")
        ));
    }
    stack.push(name.into());
    if let Some(Action::Rules {
        rules, fallback, ..
    }) = defs.get(name)
    {
        for e in rules.values().chain(std::iter::once(fallback)) {
            if let Emission::Call(n) = e {
                check_calls(n, defs, stack)?;
            }
        }
    }
    stack.pop();
    Ok(())
}

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct Memory {
    remembered_key: Option<usize>,
    remembered_output: Vec<u8>,
    last: Option<usize>,
    previous: Option<usize>,
    last_output: Vec<u8>,
    previous_output: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct Step {
    pub key: usize,
    pub output: Vec<u8>,
    pub start: usize,
    pub end: usize,
    pub reason: String,
}

#[derive(Clone, Debug)]
struct Resolved {
    output: Vec<u8>,
    remember: bool,
    reason: String,
}

/// Adaptive keys: a key named after the character it types, whose rules
/// each name one character of context. As on keyboards (and akl.gg), that
/// context is the key pressed before: "after r" means after the r key, even
/// where another rule made it type something else.
pub(crate) fn key_context(
    name: &str,
    basis: Basis,
    rules: &BTreeMap<Vec<u8>, Emission>,
    fallback: &Emission,
) -> bool {
    basis == Basis::Text
        && name.len() == 1
        && *fallback == Emission::Text(name.as_bytes().to_vec())
        && rules.keys().all(|context| context.len() == 1)
}

fn context<'a>(basis: Basis, l: &'a Layout, mem: &'a Memory, prefix: &'a [u8]) -> &'a [u8] {
    match basis {
        Basis::Text => prefix,
        Basis::Press => mem
            .last
            .map(|i| match &l.slots[i].binding {
                Binding::Named(n) => n.as_bytes(),
                _ => l.slots[i].label.as_bytes(),
            })
            .unwrap_or(b""),
        Basis::SkipPress => mem
            .previous
            .map(|i| match &l.slots[i].binding {
                Binding::Named(n) => n.as_bytes(),
                _ => l.slots[i].label.as_bytes(),
            })
            .unwrap_or(b""),
    }
}

fn resolve_emission(
    e: &Emission,
    l: &Layout,
    m: &Memory,
    prefix: &[u8],
    stack: &mut Vec<String>,
) -> Option<Resolved> {
    match e {
        Emission::None => None,
        Emission::Text(v) => Some(Resolved {
            output: v.clone(),
            remember: true,
            reason: "mapping".into(),
        }),
        Emission::Call(n) => resolve_named(n, l, m, prefix, stack),
    }
}

fn resolve_slot(
    i: usize,
    l: &Layout,
    m: &Memory,
    prefix: &[u8],
    stack: &mut Vec<String>,
) -> Option<Resolved> {
    match &l.slots[i].binding {
        Binding::Empty => None,
        Binding::Text(v) => Some(Resolved {
            output: v.clone(),
            remember: true,
            reason: "literal".into(),
        }),
        Binding::Named(n) => resolve_named(n, l, m, prefix, stack),
    }
}

fn resolve_named(
    name: &str,
    l: &Layout,
    m: &Memory,
    prefix: &[u8],
    stack: &mut Vec<String>,
) -> Option<Resolved> {
    if stack.len() >= 32 || stack.iter().any(|n| n == name) {
        return None;
    }
    stack.push(name.into());
    let result = match l.actions.get(name)? {
        Action::RepeatOutput => {
            if m.remembered_output.is_empty() {
                None
            } else {
                Some(Resolved {
                    output: m.remembered_output.clone(),
                    remember: false,
                    reason: "repeat output".into(),
                })
            }
        }
        Action::RepeatPreviousOutput => {
            if m.previous_output.is_empty() {
                None
            } else {
                Some(Resolved {
                    output: m.previous_output.clone(),
                    remember: true,
                    reason: "repeat previous output".into(),
                })
            }
        }
        Action::RepeatAction => m
            .remembered_key
            .and_then(|i| resolve_slot(i, l, m, prefix, stack))
            .map(|mut v| {
                v.remember = false;
                v.reason = format!("repeat action: {}", v.reason);
                v
            }),
        Action::Rules {
            basis,
            rules,
            fallback,
        } => {
            let basis = &if key_context(name, *basis, rules, fallback) {
                Basis::Press
            } else {
                *basis
            };
            let ctx = context(*basis, l, m, prefix);
            let selected = if *basis == Basis::Text {
                rules
                    .iter()
                    .filter(|(k, _)| ctx.ends_with(k))
                    .max_by_key(|(k, _)| k.len())
                    .map(|(_, e)| e)
            } else {
                rules.get(ctx).or_else(|| {
                    if matches!(basis, Basis::Press | Basis::SkipPress) {
                        rules.get(&[b"@".as_slice(), ctx].concat())
                    } else {
                        None
                    }
                })
            };
            resolve_emission(selected.unwrap_or(fallback), l, m, prefix, stack).map(|mut v| {
                v.reason = format!("@{name}: {}", v.reason);
                v
            })
        }
    };
    stack.pop();
    result
}

fn advance(m: &Memory, key: usize, r: &Resolved) -> Memory {
    let mut next = m.clone();
    next.previous = m.last;
    next.last = Some(key);
    next.previous_output = m.last_output.clone();
    next.last_output = r.output.clone();
    if r.remember {
        next.remembered_key = Some(key);
        next.remembered_output = r.output.clone();
    }
    next
}

#[derive(Clone, Copy, Debug, Default)]
struct Cost {
    presses: usize,
    effort: f64,
}

impl Cost {
    fn better(self, other: Self) -> bool {
        self.presses < other.presses
            || (self.presses == other.presses && self.effort.total_cmp(&other.effort).is_lt())
    }
}

#[derive(Clone, Debug)]
struct Node {
    memory: Memory,
    cost: Cost,
    from: Option<usize>,
    step: Option<Step>,
}

/// Fast, bounded-context mapping for cached n-gram statistics. Each output
/// character uses one physical key. Prefer a magic/repeat action over a literal
/// when its relevant finger is safe: the previous press for ordinary actions,
/// or the press two back for skip actions. If an action shares the other
/// recent finger, compare its local effort with the literal instead of forcing
/// the action. When every direct literal for the next character uses the
/// action's finger, compare the local cost through that press as well. Also
/// compare that two-press cost between alternative actions, including jumps
/// and stretches onto different fingers. Following actions are not projected.
/// This is not whole-text pathfinding.
pub(crate) struct WindowMapper<'a> {
    layout: &'a Layout,
    literals: [Vec<usize>; 256],
    actions: Vec<usize>,
    previous: [u8; 5],
    previous_len: usize,
    memories: [Memory; 6],
    starts: [usize; 6],
    keys: [Option<usize>; 5],
    may_look_ahead: [bool; 5],
}

impl<'a> WindowMapper<'a> {
    fn action_priority(&self, key: usize, memory: &Memory) -> Option<bool> {
        let Binding::Named(name) = &self.layout.slots[key].binding else {
            return None;
        };
        let Some(action) = self.layout.actions.get(name) else {
            return None;
        };
        let skip = match action {
            Action::Rules {
                basis: Basis::SkipPress,
                ..
            }
            | Action::RepeatPreviousOutput => true,
            Action::Rules { .. } | Action::RepeatOutput | Action::RepeatAction => false,
        };
        let finger = self.layout.slots[key].finger;
        let prior = if skip { memory.previous } else { memory.last };
        if prior.is_some_and(|old| self.layout.slots[old].finger == finger) {
            return Some(false);
        }
        let other = if skip { memory.last } else { memory.previous };
        if other.is_some_and(|old| self.layout.slots[old].finger == finger) {
            return None;
        }
        Some(true)
    }

    pub(crate) fn new(layout: &'a Layout, order: usize) -> Result<Self> {
        Self::validate(layout, order)?;
        Ok(Self::from_validated_permutation(layout))
    }

    pub(crate) fn validate(layout: &Layout, order: usize) -> Result<()> {
        let mut pending = Vec::new();
        let mut visited = Vec::new();
        for slot in &layout.slots {
            match &slot.binding {
                Binding::Text(text) => {
                    if text.len() != 1 {
                        return err("N-gram mode requires one output character per press; multi-character macros are supported only by explicit text tracing.");
                    }
                }
                Binding::Named(name) => pending.push(name.clone()),
                Binding::Empty => {}
            }
        }
        while let Some(name) = pending.pop() {
            if visited.contains(&name) {
                continue;
            }
            visited.push(name.clone());
            let action = layout
                .actions
                .get(&name)
                .ok_or_else(|| format!("unknown action {name}"))?;
            let mut emissions = Vec::new();
            match action {
                Action::Rules {
                    basis,
                    rules,
                    fallback,
                } => {
                    if *basis == Basis::Text && rules.keys().any(|k| k.len() >= order) {
                        return err(format!("Action {name} needs more context than this {order}-gram corpus; rebuild with a higher order or shorten the rule."));
                    }
                    emissions.extend(rules.values());
                    emissions.push(fallback);
                }
                _ => {}
            }
            for emission in emissions {
                match emission {
                    Emission::Text(text) if text.len() != 1 => return err(format!("Action {name} emits multiple characters; cached n-gram mode requires one character per press.")),
                    Emission::Call(target) => pending.push(target.clone()),
                    _ => {
                    },
                }
            }
        }
        Ok(())
    }
    /// Search-only constructor: `new` must have validated the starting layout
    /// at the same corpus order. Subsequent layouts may only permute bindings
    /// and labels; definitions and the binding multiset must remain unchanged.
    /// Positions are candidate-dependent, so rebuild these small indices and
    /// reset history, but do not walk/clone the immutable action graph again.
    pub(crate) fn from_validated_permutation(layout: &'a Layout) -> Self {
        let mut literals: [Vec<usize>; 256] = std::array::from_fn(|_| Vec::new());
        let mut actions = Vec::new();
        for (key, slot) in layout.slots.iter().enumerate() {
            match &slot.binding {
                Binding::Text(text) => literals[text[0] as usize].push(key),
                Binding::Named(_) => actions.push(key),
                Binding::Empty => {}
            }
        }
        Self {
            layout,
            literals,
            actions,
            previous: [0; 5],
            previous_len: 0,
            memories: std::array::from_fn(|_| Memory::default()),
            starts: [0; 6],
            keys: [None; 5],
            may_look_ahead: [false; 5],
        }
    }

    pub(crate) fn map<F>(&mut self, text: &[u8], effort: &F) -> Result<[Option<usize>; 5]>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        self.map_impl(text, None, None, effort)
    }

    /// Map forced combo roots while treating every marked source position as
    /// part of one chord event. Returned roots retain all chord-key identities
    /// for usage accounting, while physical movement history is cleared.
    pub(crate) fn map_chords<F>(
        &mut self,
        text: &[u8],
        forced: &[Option<usize>],
        chords: &[bool],
        effort: &F,
    ) -> Result<[Option<usize>; 5]>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        if forced.len() != text.len() || chords.len() != text.len() {
            return err("forced roots and chord mask must match cached context length");
        }
        if forced
            .iter()
            .flatten()
            .any(|&key| key >= self.layout.slots.len())
        {
            return err("forced root is not a physical slot");
        }
        if chords
            .iter()
            .zip(forced)
            .any(|(&chord, root)| chord && root.is_none())
        {
            return err("chord position requires a forced physical root");
        }
        self.map_impl(text, Some(forced), Some(chords), effort)
    }

    fn map_impl<F>(
        &mut self,
        text: &[u8],
        forced: Option<&[Option<usize>]>,
        chords: Option<&[bool]>,
        effort: &F,
    ) -> Result<[Option<usize>; 5]>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        if text.is_empty() || text.len() > 5 {
            return err("cached context must contain 1 to 5 characters");
        }
        let common = forced.map_or_else(
            || {
                text.iter()
                    .zip(&self.previous[..self.previous_len])
                    .take_while(|(a, b)| a == b)
                    .count()
            },
            |_| 0,
        );
        let common = if common > 0
            && (common < text.len() || common < self.previous_len)
            && self.may_look_ahead[common - 1]
        {
            common - 1
        } else {
            common
        };
        for at in common..text.len() {
            let mem = &self.memories[at];
            let start = self.starts[at];
            let mut best: Option<(usize, Resolved, f64)> = None;
            let mut may_look_ahead = false;
            if let Some(key) = forced.and_then(|roots| roots[at]) {
                best = Some((
                    key,
                    Resolved {
                        output: vec![text[at]],
                        remember: true,
                        reason: "forced physical root".into(),
                    },
                    0.0,
                ));
            } else {
                for &key in self.literals[text[at] as usize]
                    .iter()
                    .chain(self.actions.iter())
                {
                    let Some(r) =
                        resolve_slot(key, self.layout, mem, &text[start..at], &mut Vec::new())
                    else {
                        continue;
                    };
                    if r.output.as_slice() != &text[at..at + 1] {
                        continue;
                    }
                    let cost = effort(mem.previous, mem.last, key);
                    if !cost.is_finite() {
                        return err("non-finite typing effort");
                    }
                    if let Some((old_key, _, old_cost)) = &best {
                        let literal =
                            matches!(self.layout.slots[*old_key].binding, Binding::Text(_));
                        let priority = literal.then(|| self.action_priority(key, mem)).flatten();
                        may_look_ahead |= priority != Some(false);
                        let next_is_chord =
                            chords.is_some_and(|mask| mask.get(at + 1) == Some(&true));
                        let projected =
                            if priority != Some(false) && at + 1 < text.len() && !next_is_chord {
                                let forced_next;
                                let next =
                                    if let Some(slot) = forced.and_then(|roots| roots[at + 1]) {
                                        forced_next = [slot];
                                        &forced_next[..]
                                    } else {
                                        &self.literals[text[at + 1] as usize]
                                    };
                                if (!literal
                                    || next.iter().all(|&n| {
                                        self.layout.slots[n].finger == self.layout.slots[key].finger
                                    }))
                                    && !next.is_empty()
                                {
                                    let mut nonfinite = false;
                                    let mut next_cost = |current| {
                                        next.iter()
                                            .map(|&n| {
                                                let value = effort(mem.last, Some(current), n);
                                                nonfinite |= !value.is_finite();
                                                value
                                            })
                                            .fold(f64::INFINITY, f64::min)
                                    };
                                    let action_total = cost + next_cost(key);
                                    let literal_total = *old_cost + next_cost(*old_key);
                                    if nonfinite
                                        || !action_total.is_finite()
                                        || !literal_total.is_finite()
                                    {
                                        return err("non-finite typing effort");
                                    }
                                    Some(
                                        action_total < literal_total
                                            || (priority == Some(true)
                                                && action_total == literal_total),
                                    )
                                } else {
                                    None
                                }
                            } else {
                                None
                            };
                        if priority == Some(false)
                            || match projected {
                                Some(wins) => !wins,
                                None => priority != Some(true) && cost >= *old_cost,
                            }
                        {
                            continue;
                        }
                    }
                    best = Some((key, r, cost));
                }
            }
            if let Some((key, r, _)) = best {
                let mut next = advance(mem, key, &r);
                if chords.is_some_and(|mask| mask[at]) {
                    next.previous = None;
                    next.last = None;
                    next.remembered_key = None;
                }
                self.keys[at] = Some(key);
                self.memories[at + 1] = next;
                self.starts[at + 1] = start;
            } else {
                self.keys[at] = None;
                self.memories[at + 1] = Memory::default();
                self.starts[at + 1] = at + 1;
            }
            self.may_look_ahead[at] = may_look_ahead;
        }
        self.previous[..text.len()].copy_from_slice(text);
        self.previous_len = if forced.is_some() { 0 } else { text.len() };
        Ok(self.keys)
    }
}

/// Exact dynamic programming for the documented lexicographic typing policy.
/// Limits return an error; no beam pruning or truncated answer is passed off as exact.
pub fn decode<F>(
    layout: &Layout,
    text: &[u8],
    limit: usize,
    cancel: &AtomicBool,
    effort: F,
) -> Result<Vec<Step>>
where
    F: Fn(Option<usize>, Option<usize>, usize) -> f64,
{
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let mut arena = vec![Node {
        memory: Memory::default(),
        cost: Cost::default(),
        from: None,
        step: None,
    }];
    let mut frontiers: Vec<BTreeMap<Memory, usize>> =
        (0..=text.len()).map(|_| BTreeMap::new()).collect();
    frontiers[0].insert(Memory::default(), 0);
    for at in 0..text.len() {
        if cancel.load(Ordering::Relaxed) {
            return err("cancelled");
        }
        let frontier = std::mem::take(&mut frontiers[at]);
        for (_, node_id) in frontier {
            let old = arena[node_id].clone();
            for key in 0..layout.slots.len() {
                let Some(resolved) =
                    resolve_slot(key, layout, &old.memory, &text[..at], &mut Vec::new())
                else {
                    continue;
                };
                if resolved.output.is_empty() || !text[at..].starts_with(&resolved.output) {
                    continue;
                }
                let end = at + resolved.output.len();
                let memory = advance(&old.memory, key, &resolved);
                let e = effort(old.memory.previous, old.memory.last, key);
                if !e.is_finite() {
                    return err("non-finite typing effort");
                }
                let cost = Cost {
                    presses: old.cost.presses + 1,
                    effort: old.cost.effort + e,
                };
                if frontiers[end]
                    .get(&memory)
                    .is_some_and(|&i| !cost.better(arena[i].cost))
                {
                    continue;
                }
                if limit > 0 && arena.len() >= limit {
                    return err(format!("typing exceeded {limit} states at byte {at}; increase the state limit or use shorter true sequences"));
                }
                let i = arena.len();
                arena.push(Node {
                    memory: memory.clone(),
                    cost,
                    from: Some(node_id),
                    step: Some(Step {
                        key,
                        output: resolved.output,
                        start: at,
                        end,
                        reason: resolved.reason,
                    }),
                });
                frontiers[end].insert(memory, i);
            }
        }
    }
    let mut last = frontiers[text.len()]
        .values()
        .copied()
        .min_by(|&a, &b| {
            arena[a]
                .cost
                .presses
                .cmp(&arena[b].cost.presses)
                .then_with(|| arena[a].cost.effort.total_cmp(&arena[b].cost.effort))
        })
        .ok_or_else(|| {
            format!(
                "layout cannot type sequence {}",
                quote(&text[..text.len().min(100)])
            )
        })?;
    let mut steps = Vec::new();
    while let Some(from) = arena[last].from {
        if let Some(step) = &arena[last].step {
            steps.push(step.clone());
        }
        last = from;
    }
    steps.reverse();
    Ok(steps)
}

pub fn trace_keys(layout: &Layout, keys: &[usize]) -> Result<Vec<Step>> {
    let mut prefix = Vec::new();
    let mut mem = Memory::default();
    let mut steps = Vec::new();
    for &key in keys {
        if key >= layout.slots.len() {
            return err("physical key index outside layout");
        }
        let r = resolve_slot(key, layout, &mem, &prefix, &mut Vec::new())
            .ok_or_else(|| format!("{} emits nothing in this context", layout.slots[key].label))?;
        let start = prefix.len();
        mem = advance(&mem, key, &r);
        prefix.extend_from_slice(&r.output);
        steps.push(Step {
            key,
            output: r.output,
            start,
            end: prefix.len(),
            reason: r.reason,
        });
    }
    Ok(steps)
}

/// Count text n-grams directly; useful for checking the project's parallel generator.
#[cfg(test)]
pub fn text_ngrams(bytes: &[u8]) -> [std::collections::HashMap<Vec<u8>, u64>; 5] {
    let mut out = std::array::from_fn(|_| std::collections::HashMap::new());
    for s in bytes.split(|b| *b == b'\n') {
        for n in 1..=5 {
            for w in s.windows(n) {
                *out[n - 1].entry(w.to_vec()).or_default() += 1;
            }
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod test_layouts {
    /// QWERTY's three rows, for `jsonc`.
    pub(crate) const QWERTY: [&str; 3] = [
        "q w e r t y u i o p",
        "a s d f g h j k l ;",
        "z x c v b n m , . /",
    ];

    /// QWERTY with Space on the left thumb.
    pub(crate) const QWERTY_JSONC: &str = r#"{"layout": {"fingers": ["q w e r t y u i o p", "a s d f g h j k l ;", "z x c v b n m , . /"], "thumbs": ["space", ""]}}"#;

    /// A JSONC layout: three rows of keys (`skip` for an empty slot), the
    /// left and right thumbs, and further top-level fields as written in
    /// the file (`"magic": ...`, `"fingermap": ...`).
    pub(crate) fn jsonc(rows: [&str; 3], thumbs: [&str; 2], fields: &str) -> String {
        let fields = if fields.is_empty() {
            String::new()
        } else {
            format!(",\n{fields}")
        };
        format!(
            "{{\"layout\": {{\"fingers\": [{:?}, {:?}, {:?}], \"thumbs\": [{:?}, {:?}]}}{fields}}}",
            rows[0], rows[1], rows[2], thumbs[0], thumbs[1]
        )
    }

    /// QWERTY with Space on the left thumb and these further fields.
    pub(crate) fn qwerty(fields: &str) -> String {
        jsonc(QWERTY, ["space", ""], fields)
    }

    /// A `magic` field with these (inputs, output) rules.
    pub(crate) fn magic(rules: &[(&str, &str)]) -> String {
        let rules: Vec<_> = rules
            .iter()
            .map(|(inputs, output)| format!("{{\"inputs\": {inputs:?}, \"output\": {output:?}}}"))
            .collect();
        format!("\"magic\": {{\"rules\": [{}]}}", rules.join(", "))
    }

    /// QWERTY with a magic key `@` in place of n, typing r after h, and
    /// further magic rules.
    pub(crate) fn magic_on_n(rules: &[(&str, &str)]) -> String {
        let mut all = vec![("h@", "hr")];
        all.extend_from_slice(rules);
        jsonc(
            ["q w e r t y u i o p", "a s d f g h j k l ;", "z x c v b @ m , . /"],
            ["space", ""],
            &magic(&all),
        )
    }

    /// Fingers for 11-key rows with an outer column on the left.
    pub(crate) const LEFT_OUTER_FINGERS: &str = r#""fingermap": ["0 0 1 2 3 3 6 6 7 8 9", "0 0 1 2 3 3 6 6 7 8 9", "0 0 1 2 3 3 6 6 7 8 9"]"#;

    /// QWERTY with an empty outer column on the left and a magic key `@`
    /// in place of `.`.
    pub(crate) fn shorthand() -> String {
        jsonc(
            [
                "skip q w e r t y u i o p",
                "skip a s d f g h j k l ;",
                "skip z x c v b n m , @ /",
            ],
            ["space", ""],
            &format!(
                "{LEFT_OUTER_FINGERS},\n{}",
                magic(&[("i@", "i'"), ("r@", "rk"), ("a@", "aa")])
            ),
        )
    }

    /// `shorthand` with an outer column on the right holding a skip key
    /// `$` that types u two presses after q.
    pub(crate) fn shorthand_with_skip() -> String {
        jsonc(
            [
                "skip q w e r t y u i o p skip",
                "skip a s d f g h j k l ; $",
                "skip z x c v b n m , @ / skip",
            ],
            ["space", ""],
            &format!(
                "\"fingermap\": [\"0 0 1 2 3 3 6 6 7 8 9 9\", \"0 0 1 2 3 3 6 6 7 8 9 9\", \"0 0 1 2 3 3 6 6 7 8 9 9\"],\n{},\n\"skip\": {{\"key\": \"$\", \"rules\": [{{\"inputs\": \"q_$\", \"output\": \"q_u\"}}]}}",
                magic(&[("i@", "i'"), ("r@", "rk"), ("a@", "aa")])
            ),
        )
    }

    /// A layout with a magic key `*` and a swap of n and r after h.
    pub(crate) fn compact() -> String {
        jsonc(
            [
                "f d l w v q p o u ,",
                "s t h y g z n a e i",
                "x k m c j * b ' ; .",
            ],
            ["r", "space"],
            &magic(&[
                ("'*", "'r"),
                ("a*", "ay"),
                ("h*", "hr"),
                ("i*", "ik"),
                ("j*", "jo"),
                ("k*", "ke"),
                ("r*", "rl"),
                ("u*", "u'"),
                ("y*", "ye"),
                ("hn", "hr"),
                ("hr", "hn"),
            ]),
        )
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    /// QWERTY with `key` on the home row of an outer column on the left,
    /// and further JSONC fields.
    fn make(key: &str, fields: &str) -> Layout {
        let rows = [
            "skip q w e r t y u i o p".to_string(),
            format!("{key} a s d f g h j k l ;"),
            "skip z x c v b n m , . /".to_string(),
        ];
        let fields = format!("{},\n{fields}", test_layouts::LEFT_OUTER_FINGERS);
        let source = test_layouts::jsonc([&rows[0], &rows[1], &rows[2]], ["space", ""], &fields);
        Layout::parse(&source, Path::new("sample.jsonc")).unwrap()
    }

    fn key(l: &Layout, s: &str) -> usize {
        l.slots
            .iter()
            .position(|x| x.label == s || x.label.trim_start_matches('@') == s)
            .unwrap()
    }

    fn typed(l: &Layout, names: &[&str]) -> Vec<u8> {
        trace_keys(l, &names.iter().map(|s| key(l, s)).collect::<Vec<_>>())
            .unwrap_or_else(|error| panic!("physical keys {names:?}: {error}"))
            .into_iter()
            .flat_map(|step| step.output)
            .collect()
    }

    #[test]
    fn magic_uses_longest_text_suffix() {
        let l = make("@", &test_layouts::magic(&[("q@", "qu"), ("qu@", "que"), ("u@", "ua")]));
        assert_eq!(typed(&l, &["q", "@", "@"]), b"que");
    }
    #[test]
    fn skip_magic_uses_second_last_physical_key() {
        let l = make(
            "$",
            r#""skip": {"key": "$", "rules": [{"inputs": "q_$", "output": "q_u"}, {"inputs": "x_$", "output": "x_a"}]}"#,
        );
        assert_eq!(typed(&l, &["q", "x", "$"]), b"qxu");
    }
    #[test]
    fn decoder_honors_state_limit_and_cancellation() {
        let l = make("skip", "");
        for (case, state_limit, cancelled) in
            [("state limit", 1, false), ("cancellation", 1000, true)]
        {
            assert!(
                decode(
                    &l,
                    b"abc",
                    state_limit,
                    &AtomicBool::new(cancelled),
                    |_, _, _| 0.0
                )
                .is_err(),
                "ignored {case}"
            );
        }
    }
    #[test]
    fn validated_permutations_match_fresh_mapper_for_all_swaps() {
        let source = test_layouts::shorthand_with_skip();
        let mut layout = Layout::parse(&source, Path::new("cached.jsonc")).unwrap();
        for order in 3..=5 {
            // The production cache validates once at the start of each search.
            WindowMapper::new(&layout, order).unwrap();
            for a in 0..layout.slots.len() {
                for b in a..layout.slots.len() {
                    layout.swap(a, b);
                    {
                        let mut fresh = WindowMapper::new(&layout, order).unwrap();
                        let mut cached = WindowMapper::from_validated_permutation(&layout);
                        // Unsorted contexts exercise history resets as well as reuse.
                        for text in [
                            b"i'i'a".as_slice(),
                            b"aa!aa",
                            b"qxuqu",
                            b"rkrka",
                            b"qqqqu",
                            b"i",
                            b"i'a",
                        ] {
                            let text = &text[..text.len().min(order)];
                            let effort =
                                |previous: Option<usize>, last: Option<usize>, key: usize| {
                                    key as f64 * 0.01
                                        + last.map_or(0.0, |k| if k == key { 2.0 } else { 0.0 })
                                        + previous.map_or(0.0, |k| if k == key { 1.0 } else { 0.0 })
                                };
                            let expected = fresh.map(text, &effort).unwrap();
                            let actual = cached.map(text, &effort).unwrap();
                            assert_eq!(&actual[..text.len()], &expected[..text.len()]);
                        }
                    }
                    // End immutable mapper borrows before restoring the layout.
                    layout.swap(a, b);
                }
            }
        }
    }
    #[test]
    fn swaps_move_owned_bindings_without_changing_geometry_or_definitions() {
        // A magic key without rules repeats the last output.
        let mut layout = make("@", r#""magic": {"key": "@"}"#);
        let original = layout.clone();
        let a = key(&layout, "a");
        let b = key(&layout, "@");
        let label_ptr = layout.slots[a].label.as_ptr();
        let text_ptr = match &layout.slots[a].binding {
            Binding::Text(v) => v.as_ptr(),
            _ => unreachable!(),
        };
        layout.swap(b, a);
        // Reverse-index path also uses disjoint mutable slices.
        assert_eq!(layout.slots[b].label.as_ptr(), label_ptr);
        match &layout.slots[b].binding {
            Binding::Text(v) => assert_eq!(v.as_ptr(), text_ptr),
            _ => panic!("literal lost"),
        }
        assert_eq!(layout.actions, original.actions);
        for (i, slot) in layout.slots.iter().enumerate() {
            let old = &original.slots[i];
            assert_eq!(
                (
                    slot.row,
                    slot.col,
                    slot.finger,
                    slot.rank,
                    slot.hand,
                    slot.main
                ),
                (old.row, old.col, old.finger, old.rank, old.hand, old.main)
            );
        }
        layout.swap(a, b);
        layout.swap(a, a);
        assert_eq!(layout.slots, original.slots);
    }
}
