//! Text-producing keyboard actions. This module never equates output bytes with physical presses.
use std::collections::{BTreeMap, HashMap};

use std::fs::{self, File, OpenOptions};

use std::io::{Read, Write};

use std::path::{Path, PathBuf};

use std::sync::atomic::{AtomicBool, Ordering};

pub const DEFAULT_STATE_LIMIT: usize = 500_000;

pub const SEQUENCE_HEADER: &[u8] = b"LAYOUTER-SEQUENCES-1\n";

pub type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Binding {
    Empty,
    Text(Vec<u8>),
    Named(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Basis {
    Text,
    Press,
    SkipPress,
    Output,
    SkipOutput,
    Remembered,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Emission {
    Text(Vec<u8>),
    Call(String),
    None,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Text(Vec<u8>),
    Rules {
        basis: Basis,
        rules: BTreeMap<Vec<u8>, Emission>,
        fallback: Emission,
    },
    RepeatOutput,
    RepeatAction,
    Inactive,
}

#[derive(Clone, Debug)]
pub struct Layout {
    pub name: String,
    pub path: PathBuf,
    pub slots: Vec<Slot>,
    pub actions: BTreeMap<String, Action>,
    pub left_outer: bool,
    pub right_outer: bool,
    pub(crate) row_stagger: RowStagger,
    pub(crate) action_mode: bool,
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

fn literal(text: &str) -> Result<Vec<u8>> {
    let text = text.trim();
    if text.len() < 2 || !text.starts_with('"') || !text.ends_with('"') {
        return err(format!("expected a quoted string, got {text:?}"));
    }
    let mut out = Vec::new();
    let mut bytes = text.as_bytes()[1..text.len() - 1].iter().copied();
    while let Some(b) = bytes.next() {
        if b != b'\\' {
            if b == b'"' || b < 32 {
                return err("invalid quoted string");
            }
            out.push(b);
            continue;
        }
        match bytes.next().ok_or("unfinished escape")? {
            b'"' => out.push(b'"'),
            b'\\' => out.push(b'\\'),
            b'/' => out.push(b'/'),
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'u' => {
                let mut n = 0u32;
                for _ in 0..4 {
                    let d = bytes.next().ok_or("unfinished Unicode escape")?;
                    n = n * 16 + (d as char).to_digit(16).ok_or("invalid Unicode escape")?;
                }
                let ch = char::from_u32(n).ok_or("invalid Unicode scalar")?;
                let mut buf = [0u8; 4];
                out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            }
            _ => return err("unsupported string escape"),
        }
    }
    Ok(out)
}

fn action_name(text: &str) -> String {
    if text == "@" {
        text.into()
    } else {
        text.strip_prefix('@').unwrap_or(text).to_ascii_lowercase()
    }
}

fn assignment(text: &str) -> Result<(&str, &str)> {
    let (mut quoted, mut escaped) = (false, false);
    for (i, ch) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if quoted && ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '"' {
            quoted = !quoted;
        }
        if ch == '=' && !quoted {
            return Ok((text[..i].trim(), text[i + 1..].trim()));
        }
    }
    err("expected '=' outside a quoted string")
}

fn emission(text: &str) -> Result<Emission> {
    let s = text.trim();
    if s == "none" {
        return Ok(Emission::None);
    }
    if let Some(name) = s.strip_prefix('@') {
        return Ok(Emission::Call(name.to_ascii_lowercase()));
    }
    if matches!(s, "repeat" | "repeat-output" | "repeat-action" | "again") {
        return Ok(Emission::Call(s.into()));
    }
    Ok(Emission::Text(ascii(literal(s)?, "action output")?))
}

fn action(text: &str) -> Result<Action> {
    let s = text.trim();
    if let Some(rest) = s.strip_prefix("text ") {
        return Ok(Action::Text(ascii(literal(rest)?, "macro")?));
    }
    let basis = match s {
        "magic" => Some(Basis::Text),
        "press-magic" => Some(Basis::Press),
        "skip-magic" => Some(Basis::SkipPress),
        "output-magic" => Some(Basis::Output),
        "skip-output-magic" => Some(Basis::SkipOutput),
        "alternate" | "alt-repeat" => Some(Basis::Remembered),
        _ => None,
    };
    if let Some(basis) = basis {
        return Ok(Action::Rules {
            basis,
            rules: BTreeMap::new(),
            fallback: Emission::None,
        });
    }
    match s {
        "repeat" | "repeat-output" => Ok(Action::RepeatOutput),
        "again" | "repeat-action" => Ok(Action::RepeatAction),
        "inactive" => Ok(Action::Inactive),
        _ => err(format!("unknown action kind {s:?}")),
    }
}

/// Fixed row-stagger geometry with fingering defined by physical QWERTY slots.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum RowStagger {
    #[default]
    Off,
    Standard,
    Anglemod,
    Nokwts,
    Meteorite,
}

impl RowStagger {
    pub(crate) fn offset(self, row: i8) -> i16 {
        if self == Self::Off {
            return 0;
        }
        match row {
            0 => -250,
            2 => 500,
            _ => 0,
        }
    }

    pub(crate) fn finger(self, row: i8, col: i8, original: usize) -> usize {
        // Extra outer columns, thumbs, and the right hand retain their fingering.
        if !(0..=4).contains(&col) || !(0..=2).contains(&row) {
            return original;
        }
        match (self, row, col) {
            (Self::Anglemod | Self::Meteorite, 2, 0) => 1, // QWERTY Z
            (Self::Anglemod | Self::Meteorite, 2, 1) => 2, // QWERTY X
            (Self::Anglemod | Self::Nokwts, 2, 2) => 3,    // QWERTY C
            (Self::Nokwts, 0, 3) => 2,                     // QWERTY R
            _ => original,
        }
    }

    pub(crate) fn text(self) -> String {
        let name = match self {
            Self::Off => return String::new(),
            Self::Standard => "standard",
            Self::Anglemod => "anglemod",
            Self::Nokwts => "nokwts",
            Self::Meteorite => "meteorite",
        };
        format!("row-stagger: {name}\n")
    }
}

pub(crate) fn parse_row_stagger(line: &str) -> Result<Option<RowStagger>> {
    let Some(rest) = line.strip_prefix("row-stagger:") else {
        return Ok(None);
    };
    let mode = match rest.trim().to_ascii_lowercase().as_str() {
        "" | "standard" | "standart" => RowStagger::Standard,
        "anglemod" => RowStagger::Anglemod,
        "nokwts" => RowStagger::Nokwts,
        "meteorite" => RowStagger::Meteorite,
        "off" => RowStagger::Off,
        _ => return err("row-stagger: expected standard (or standart), anglemod, nokwts, meteorite, or off; use row-offsets: for numeric geometry"),
    };
    Ok(Some(mode))
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

pub(crate) fn absolute_columns(lines: &[&str]) -> Result<bool> {
    let mut found = None;
    for line in lines {
        if let Some(value) = line.trim().strip_prefix("col-layout:") {
            if found.is_some() {
                return err("col-layout is defined twice");
            }
            found = Some(match value.trim() {
                "absolute" => true,
                "standard" => false,
                _ => return err("col-layout: expected absolute or standard"),
            });
        }
    }
    Ok(found.unwrap_or(false))
}

#[derive(Default)]
pub(crate) struct GeometrySettings {
    rows: Option<[i16; 3]>,
    columns: Option<Vec<i16>>,
    fingers: Option<Vec<Vec<usize>>>,
}

impl GeometrySettings {
    pub(crate) fn parse_line(&mut self, line: &str) -> Result<bool> {
        if let Some(rest) = line.strip_prefix("row-offsets:") {
            let values: Vec<_> = rest
                .split_whitespace()
                .map(geometry_offset)
                .collect::<Result<_>>()?;
            let values: [i16; 3] = values
                .try_into()
                .map_err(|_| "row-offsets needs three values".to_string())?;
            if self.rows.replace(values).is_some() {
                return err("row-offsets is defined twice");
            }
        } else if let Some(rest) = line.strip_prefix("column-offsets:") {
            let values = rest
                .split_whitespace()
                .map(geometry_offset)
                .collect::<Result<Vec<_>>>()?;
            if self.columns.replace(values).is_some() {
                return err("column-offsets is defined twice");
            }
        } else if let Some(rest) = line.strip_prefix("fingermap:") {
            let mut rows = Vec::new();
            for row in rest.split('/') {
                let mut fingers = Vec::new();
                for name in row.split_whitespace() {
                    let finger = match name.to_ascii_uppercase().as_str() {
                        "LP" => 0,
                        "LR" => 1,
                        "LM" => 2,
                        "LI" => 3,
                        "RI" => 4,
                        "RM" => 5,
                        "RR" => 6,
                        "RP" => 7,
                        _ => return err("fingermap uses LP LR LM LI RI RM RR RP; separate its three rows with /"),
                    };
                    fingers.push(finger);
                }
                rows.push(fingers);
            }
            if self.fingers.replace(rows).is_some() {
                return err("fingermap is defined twice");
            }
        } else {
            return Ok(false);
        }
        Ok(true)
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
}

pub(crate) fn geometry_text(
    keys: impl Iterator<Item = (i8, i8, i16, i16, usize)>,
    mode: RowStagger,
) -> String {
    let keys: Vec<_> = keys.collect();
    let mut out = String::new();
    if keys.iter().any(|key| key.1 == 11) {
        out.push_str("col-layout: absolute\n");
    }

    if keys.iter().any(|key| key.2 != mode.offset(key.0)) {
        out.push_str("row-offsets:");
        for row in 0..3 {
            let offset = keys.iter().find(|key| key.0 == row).map_or(0, |key| key.2);
            out.push_str(&format!(" {:.3}", offset as f64 / 1000.0));
        }
        out.push('\n');
    }
    if keys.iter().any(|key| key.3 != 0) {
        out.push_str("column-offsets:");
        let minimum = keys.iter().map(|key| key.1).min().unwrap_or(0);
        let maximum = keys.iter().map(|key| key.1).max().unwrap_or(0);
        for column in minimum..=maximum {
            let offset = keys
                .iter()
                .find(|key| key.1 == column)
                .map_or(0, |key| key.3);
            out.push_str(&format!(" {:.3}", offset as f64 / 1000.0));
        }
        out.push('\n');
    }

    if keys
        .iter()
        .any(|key| key.4 != mode.finger(key.0, key.1, finger(key.1).0))
    {
        const NAMES: [&str; 8] = ["LP", "LR", "LM", "LI", "RI", "RM", "RR", "RP"];
        out.push_str("fingermap:");
        for row in 0..3 {
            if row != 0 {
                out.push_str(" /");
            }
            for key in keys.iter().filter(|key| key.0 == row) {
                out.push(' ');
                out.push_str(NAMES[key.4]);
            }
        }
        out.push('\n');
    }
    out
}

fn binding(token: &str) -> Result<(Binding, String)> {
    let s = token.trim();
    if matches!(s, "~" | "blank" | "·") {
        return Ok((Binding::Empty, "·".into()));
    }
    if s.eq_ignore_ascii_case("space") || s == "␠" {
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
    let name = name.to_ascii_lowercase();
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
        9 | 10 | 11 => (7, 0, 1),
        _ => unreachable!(),
    }
}

fn make_slot(token: &str, row: i8, col: i8) -> Result<Slot> {
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

fn main_rows(lines: &[&str], absolute: bool) -> Result<(Vec<Slot>, bool, bool)> {
    let mut out = Vec::new();
    let mut left_outer = false;
    let mut right_outer = false;
    for (r, line) in lines.iter().take(3).enumerate() {
        let mut tokens: Vec<String> = line.split_whitespace().map(str::to_string).collect();
        if tokens.len() < 10 && !line.contains('|') {
            const AT: [usize; 10] = [0, 2, 4, 6, 8, 11, 13, 15, 17, 19];
            let bytes = line.as_bytes();
            if bytes.len() <= 20
                && bytes
                    .iter()
                    .enumerate()
                    .all(|(i, b)| AT.contains(&i) || *b == b' ')
            {
                tokens = AT
                    .iter()
                    .map(|&i| match bytes.get(i).copied().unwrap_or(b' ') {
                        b' ' => "blank".into(),
                        b => (b as char).to_string(),
                    })
                    .collect();
            }
        }
        let sep = tokens.iter().position(|t| t == "|");
        let (left, right) = if let Some(s) = sep {
            if tokens.iter().filter(|t| t.as_str() == "|").count() != 1 {
                return err("one hand separator is allowed per row");
            }
            (tokens[..s].to_vec(), tokens[s + 1..].to_vec())
        } else {
            let n = match tokens.len() {
                10 => 5,
                12 => 6,
                _ => {
                    return err(format!(
                        "row {}: use 10 or 12 slots, or separate an 11-slot row as 6|5 or 5|6",
                        r + 1
                    ))
                }
            };
            (tokens[..n].to_vec(), tokens[n..].to_vec())
        };
        if !(5..=6).contains(&left.len()) || !(5..=6).contains(&right.len()) {
            return err("each hand must have five or six columns");
        }
        left_outer |= left.len() == 6;
        right_outer |= right.len() == 6;
        let start = if left.len() == 6 && !absolute { -1 } else { 0 };
        for (j, t) in left.iter().enumerate() {
            out.push(make_slot(t, r as i8, start + j as i8)?);
        }
        for (j, t) in right.iter().enumerate() {
            let col = if absolute { left.len() + j } else { 5 + j };
            out.push(make_slot(t, r as i8, col as i8)?);
        }
    }
    Ok((out, left_outer, right_outer))
}

fn compact_mapping(line: &str, slots: &[Slot]) -> Result<Option<(u8, Vec<u8>, Vec<u8>)>> {
    // `i@ i'` describes the complete before/after text. The action itself
    // emits only the suffix (`'`); the already typed `i` is not emitted twice.
    let fields: Vec<_> = line.split_whitespace().collect();
    if fields.len() != 2 {
        return Ok(None);
    }
    let left = fields[0].as_bytes();
    let right = fields[1].as_bytes();
    if left.len() < 2 || !left.iter().chain(right).all(|b| b.is_ascii_graphic()) {
        return Ok(None);
    }
    let marker = *left.last().unwrap();
    if marker.is_ascii_alphanumeric() {
        return Ok(None);
    }
    if !slots
        .iter()
        .any(|s| s.binding == Binding::Text(vec![marker.to_ascii_lowercase()]))
    {
        return Ok(None);
    }
    let context: Vec<_> = left[..left.len() - 1]
        .iter()
        .map(|b| b.to_ascii_lowercase())
        .collect();
    let output: Vec<_> = right.iter().map(|b| b.to_ascii_lowercase()).collect();
    if !output.starts_with(&context) {
        return err(format!(
            "compact mapping {} must keep its context at the start of {}",
            fields[0], fields[1]
        ));
    }
    let emitted = output[context.len()..].to_vec();
    if emitted.is_empty() {
        return err(format!("compact mapping {} emits no new text", fields[0]));
    }
    Ok(Some((marker.to_ascii_lowercase(), context, emitted)))
}

/// Compact tables are syntax sugar for the existing text-context actions.
/// No evaluator or optimizer-specific rule representation is introduced.
struct CompactTable {
    rules: BTreeMap<Vec<u8>, Emission>,
    fallback: Emission,
}

fn compact_key(token: &str, adaptive: bool) -> Result<String> {
    if token.chars().count() == 1
        && !token.is_ascii()
        && !token.chars().any(char::is_control)
        && !adaptive
    {
        return Ok(token.into());
    }

    let bytes = token.as_bytes();
    if bytes.len() != 1 || !bytes[0].is_ascii_graphic() || matches!(bytes[0], b'~' | b'|') {
        return err("compact key must be one printable ASCII key (except ~ or |)");
    }
    if adaptive && bytes[0] == b'@' {
        return err("@ is reserved for magic; adaptive/swap keys must have literal fallbacks");
    }
    if adaptive && matches!(bytes[0], b'=' | b'"') {
        return err(
            "= and double quote require an explicitly named action rather than a compact swap",
        );
    }
    if !adaptive && bytes[0].is_ascii_alphanumeric() {
        return err("use adaptive for an ordinary letter/digit key");
    }
    Ok(token.to_ascii_lowercase())
}

fn compact_pair(token: &str) -> Result<(u8, u8)> {
    let bytes = token.as_bytes();
    if bytes.len() != 2 || !bytes.iter().all(u8::is_ascii_graphic) {
        return err(format!(
            "{token:?}: expected exactly two printable ASCII characters, e.g. hr or u'"
        ));
    }
    Ok((bytes[0].to_ascii_lowercase(), bytes[1].to_ascii_lowercase()))
}

fn insert_compact_rule(
    tables: &mut BTreeMap<String, CompactTable>,
    key: &str,
    context: Vec<u8>,
    output: u8,
    fallback: Emission,
) -> Result<()> {
    let table = tables.entry(key.into()).or_insert_with(|| CompactTable {
        rules: BTreeMap::new(),
        fallback: fallback.clone(),
    });
    if table.fallback != fallback {
        return err(format!(
            "cannot mix magic and adaptive definitions for {key}"
        ));
    }
    if table
        .rules
        .insert(context, Emission::Text(vec![output]))
        .is_some()
    {
        return err(format!("duplicate compact context for {key}"));
    }
    Ok(())
}

fn compact_table_line(line: &str, tables: &mut BTreeMap<String, CompactTable>) -> Result<bool> {
    let fields: Vec<_> = line.split_whitespace().collect();
    let Some(&kind) = fields.first() else {
        return Ok(false);
    };
    let marker = crate::layout_io::dedicated_magic_label(kind);
    if !marker && !matches!(kind, "magic" | "adaptive" | "swap") {
        return Ok(false);
    }
    // A lone symbol can still be an ordinary legacy thumb declaration.
    if marker && fields.len() == 1 {
        return Ok(false);
    }

    if kind == "swap" {
        if fields.len() < 3 {
            return err("swap requires a text context and at least one key pair: swap h nr");
        }
        // Preserve the original one-context/multiple-pair syntax. Otherwise
        // accept alternating context/pair tokens, as in `swap h nr y ,u`.
        let legacy = fields[2..].iter().all(|token| token.len() == 2);
        if !legacy && (fields.len() - 1) % 2 != 0 {
            return err("swap needs context/pair groups, e.g. swap h nr y ,u; each pair has exactly two characters");
        }
        let groups: Vec<_> = if legacy {
            fields[2..].iter().map(|pair| (fields[1], *pair)).collect()
        } else {
            fields[1..]
                .chunks_exact(2)
                .map(|group| (group[0], group[1]))
                .collect()
        };
        for (context, token) in groups {
            let context = ascii(context.to_ascii_lowercase().into_bytes(), "swap context")?;
            let (a, b) = compact_pair(token)?;
            if a == b {
                return err("a swap needs two different keys");
            }
            for (key, output) in [(a, b), (b, a)] {
                let name = compact_key(&(key as char).to_string(), true)?;
                insert_compact_rule(
                    tables,
                    &name,
                    context.clone(),
                    output,
                    Emission::Text(vec![key]),
                )?;
            }
        }
        return Ok(true);
    }
    let adaptive = kind == "adaptive";
    let (key, pairs) = if marker {
        (kind.to_string(), &fields[1..])
    } else {
        let token = fields
            .get(1)
            .ok_or("magic/adaptive requires a physical key")?;
        (compact_key(token, adaptive)?, &fields[2..])
    };
    if pairs.is_empty() {
        return err("compact table requires at least one context/output pair");
    }
    let fallback = if adaptive {
        Emission::Text(key.as_bytes().to_vec())
    } else {
        Emission::Call("repeat-output".into())
    };
    for pair in pairs {
        let (context, output) = compact_pair(pair)?;
        insert_compact_rule(tables, &key, vec![context], output, fallback.clone())?;
    }
    Ok(true)
}

fn install_compact_tables(
    slots: &mut [Slot],
    actions: &mut BTreeMap<String, Action>,
    tables: BTreeMap<String, CompactTable>,
) -> Result<()> {
    for (key, table) in tables {
        // @ alone installs the historical implicit repeat action. It may be
        // refined by a compact table, but explicit definitions must not be lost.
        if actions.contains_key(&key)
            && !(key == "@" && matches!(actions.get(&key), Some(Action::RepeatOutput)))
        {
            return err(format!(
                "{key}: cannot mix compact and existing action definitions"
            ));
        }
        let mut found = false;
        for slot in slots.iter_mut() {
            let matches = match &slot.binding {
                Binding::Text(bytes) => bytes.as_slice() == key.as_bytes(),
                Binding::Named(name) => name == &key,
                Binding::Empty => false,
            };
            if matches && slot.label != "char:@" {
                slot.binding = Binding::Named(key.clone());
                slot.label = key.clone();
                found = true;
            }
        }
        if !found {
            return err(format!(
                "compact table refers to missing physical key {key}"
            ));
        }
        actions.insert(
            key,
            Action::Rules {
                basis: Basis::Text,
                rules: table.rules,
                fallback: table.fallback,
            },
        );
    }
    Ok(())
}

/// Only use short serialization when it exactly represents the action.
fn short_table_text(name: &str, action: &Action) -> Option<String> {
    let Action::Rules {
        basis: Basis::Text,
        rules,
        fallback,
    } = action
    else {
        return None;
    };
    if rules.is_empty() {
        return None;
    }
    let adaptive =
        compact_key(name, true).is_ok() && *fallback == Emission::Text(name.as_bytes().to_vec());
    let expected_fallback = Emission::Call("repeat-output".into());
    let magic = compact_key(name, false).is_ok() && *fallback == expected_fallback;
    if !adaptive && !magic {
        return None;
    }
    // Longer adaptive contexts are serialized with the existing explicit map
    // grammar, which preserves the context and action name exactly.
    let mut pairs = Vec::new();
    for (context, emission) in rules {
        let Emission::Text(output) = emission else {
            return None;
        };
        if context.len() != 1
            || output.len() != 1
            || !context[0].is_ascii_graphic()
            || !output[0].is_ascii_graphic()
            || context[0].is_ascii_uppercase()
            || output[0].is_ascii_uppercase()
        {
            return None;
        }
        pairs.push(format!("{}{}", context[0] as char, output[0] as char));
    }
    Some(format!(
        "{} {}\n",
        if magic {
            format!("magic {name}")
        } else {
            format!("adaptive {name}")
        },
        pairs.join(" ")
    ))
}

fn set_thumb(thumbs: &mut [Option<String>; 2], hand: usize, token: &str) -> Result<()> {
    if hand > 1 {
        return err("thumb hand must be left or right");
    }
    if thumbs[hand].is_some() {
        return err(format!(
            "{} thumb is defined twice",
            if hand == 0 { "left" } else { "right" }
        ));
    }
    let token = if token.len() > 1 && token != "@," {
        token.strip_suffix(',').unwrap_or(token)
    } else {
        token
    };
    thumbs[hand] = Some(token.to_string());
    Ok(())
}

fn action_slot_token(name: &str) -> String {
    if name == "@" || name.chars().count() == 1 && !name.is_ascii() {
        name.into()
    } else {
        format!("@{name}")
    }
}

fn compact_action_text(name: &str, action: &Action) -> Option<String> {
    if name.len() != 1
        || !name.as_bytes()[0].is_ascii_graphic()
        || name.as_bytes()[0].is_ascii_alphanumeric()
    {
        return None;
    }
    if name == "@" && matches!(action, Action::RepeatOutput) {
        return Some(String::new());
    }
    let Action::Rules {
        basis: Basis::Text,
        rules,
        fallback,
    } = action
    else {
        return None;
    };
    let expected = if name == "@" {
        Emission::Call("repeat-output".into())
    } else {
        Emission::None
    };
    if fallback != &expected {
        return None;
    }
    let marker = name.as_bytes()[0];
    let mut out = String::new();
    for (context, emission) in rules {
        let Emission::Text(suffix) = emission else {
            return None;
        };
        if context.is_empty() || !context.iter().chain(suffix).all(|b| b.is_ascii_graphic()) {
            return None;
        }
        let mut left = context.clone();
        left.push(marker);
        let mut right = context.clone();
        right.extend_from_slice(suffix);
        out.push_str(&format!(
            "{} {}\n",
            String::from_utf8_lossy(&left),
            String::from_utf8_lossy(&right)
        ));
    }
    Some(out)
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
        if crate::layout_io::is_json_layout(text) {
            crate::layout_io::parse_json_layout(text, path)
        } else {
            Self::parse_dat(text, path)
        }
    }

    pub(crate) fn parse_dat(text: &str, path: &Path) -> Result<Self> {
        let mut layout = Self::parse_dat_preserving_fallbacks(text, path)?;
        layout.normalize_magic_fallbacks();
        Ok(layout)
    }

    pub(crate) fn parse_dat_preserving_fallbacks(text: &str, path: &Path) -> Result<Self> {
        let text = text.trim_start_matches('\u{feff}');
        let mut lines: Vec<&str> = text.lines().map(|s| s.trim_end_matches('\r')).collect();
        while lines.first() == Some(&"") {
            lines.remove(0);
        }
        while lines.last() == Some(&"") {
            lines.pop();
        }
        if lines.len() < 3 {
            return err("layout needs three main rows");
        }
        let absolute = absolute_columns(&lines[3..])?;
        let (mut slots, mut left_outer, mut right_outer) = main_rows(&lines, absolute)?;
        let mut geometry = GeometrySettings::default();
        let mut actions = BTreeMap::new();
        for (k, v) in [
            ("repeat", Action::RepeatOutput),
            ("repeat-output", Action::RepeatOutput),
            ("repeat-action", Action::RepeatAction),
            ("again", Action::RepeatAction),
        ] {
            actions.insert(k.into(), v);
        }
        let mut definitions = Vec::new();
        let mut mappings = Vec::new();
        let mut compact = Vec::new();
        let mut thumbs: [Option<String>; 2] = [None, None];
        let mut action_mode = false;
        let mut short_tables = BTreeMap::new();
        let mut stagger_mode = None;
        let mut key_labels = None;
        for (i, line) in lines.iter().enumerate().skip(3) {
            let s = line.trim();
            if s.is_empty() || s.starts_with('#') {
                continue;
            }
            let fail = |e: String| format!("{}:{}: {e}", path.display(), i + 1);
            if let Some(labels) = s.strip_prefix("key-labels:") {
                if key_labels.is_some() {
                    return err(fail("key-labels is defined twice".into()));
                }
                let crate::Json::Array(values) =
                    crate::parse_json(labels.trim()).map_err(|e| fail(e.to_string()))?
                else {
                    return err(fail("key-labels needs a JSON array of strings".into()));
                };
                let mut labels = Vec::new();
                for value in values {
                    let crate::Json::String(label) = value else {
                        return err(fail("key-labels entries must be strings".into()));
                    };
                    if label.is_empty() || label.chars().any(char::is_control) {
                        return err(fail(
                            "key-labels entries must be nonempty printable strings".into(),
                        ));
                    }
                    labels.push(label);
                }
                key_labels = Some(labels);
                continue;
            }
            if s.starts_with("col-layout:") || geometry.parse_line(s).map_err(&fail)? {
                continue;
            }
            if let Some(mode) = parse_row_stagger(s).map_err(&fail)? {
                if stagger_mode.replace(mode).is_some() {
                    return err(fail("row-stagger is defined twice".into()));
                }
                continue;
            }
            if compact_table_line(s, &mut short_tables).map_err(&fail)? {
                action_mode = true;
            } else if let Some(rest) = s.strip_prefix("action ") {
                action_mode = true;
                let (name, value) = assignment(rest).map_err(&fail)?;
                let name = action_name(name);
                if definitions.contains(&name) {
                    return err(fail(format!("duplicate action {name}")));
                }
                definitions.push(name.clone());
                actions.insert(name, action(value).map_err(&fail)?);
            } else if let Some(rest) = s.strip_prefix("map ") {
                action_mode = true;
                let split = rest
                    .find(char::is_whitespace)
                    .ok_or_else(|| fail("map needs a name and context".into()))?;
                let name = action_name(&rest[..split]);
                let (context, value) = assignment(&rest[split..]).map_err(&fail)?;
                mappings.push((
                    name,
                    Some(literal(context).map_err(&fail)?),
                    emission(value).map_err(&fail)?,
                ));
            } else if let Some(rest) = s.strip_prefix("fallback ") {
                action_mode = true;
                let (name, value) = assignment(rest).map_err(&fail)?;
                mappings.push((action_name(name), None, emission(value).map_err(&fail)?));
            } else if s.starts_with("outer-left:") || s.starts_with("outer-right:") {
                action_mode = true;
                let left = s.starts_with("outer-left:");
                let (_, content) = s.split_once(':').unwrap();
                let ts: Vec<_> = content.split_whitespace().collect();
                if ts.len() != 3 {
                    return err(fail("outer column needs top/home/bottom tokens".into()));
                }
                if if left { left_outer } else { right_outer } {
                    return err(fail("outer column defined twice".into()));
                }
                for (r, t) in ts.iter().enumerate() {
                    slots.push(make_slot(t, r as i8, if left { -1 } else { 10 }).map_err(&fail)?);
                }
                if left {
                    left_outer = true
                } else {
                    right_outer = true
                }
            } else if let Some(rule) = compact_mapping(s, &slots).map_err(&fail)? {
                action_mode = true;
                compact.push(rule);
            } else {
                let named = s
                    .get(..7)
                    .is_some_and(|v| v.eq_ignore_ascii_case("thumbs:"));
                let content = if named { &s[7..] } else { s };
                let ts: Vec<_> = content.split_whitespace().collect();
                if ts.is_empty() || ts.len() > 2 {
                    return err(fail(
                        "thumb line needs one key, or explicit left and right keys".into(),
                    ));
                }
                if named || ts.len() == 2 {
                    for (hand, t) in ts.iter().enumerate() {
                        set_thumb(&mut thumbs, hand, t).map_err(&fail)?;
                    }
                } else {
                    // In the legacy text grid columns 0..=10 are left-thumb
                    // positions; columns 11 and later are right-thumb positions.
                    let indent = line.as_bytes().iter().take_while(|&&b| b == b' ').count();
                    set_thumb(&mut thumbs, usize::from(indent > 10), ts[0]).map_err(&fail)?;
                }
            }
        }
        let mut compact_rules: BTreeMap<u8, BTreeMap<Vec<u8>, Emission>> = BTreeMap::new();
        for (marker, context, output) in compact {
            if compact_rules
                .entry(marker)
                .or_default()
                .insert(context, Emission::Text(output))
                .is_some()
            {
                return err(format!("duplicate compact mapping for {}", marker as char));
            }
        }
        if slots
            .iter()
            .any(|s| s.binding == Binding::Text(vec![b'@']) && s.label != "char:@")
        {
            compact_rules.entry(b'@').or_default();
        }
        for (marker, rules) in compact_rules {
            let name = (marker as char).to_string();
            let action = if marker == b'@' && rules.is_empty() {
                Action::RepeatOutput
            } else {
                Action::Rules {
                    basis: Basis::Text,
                    rules,
                    fallback: if marker == b'@' {
                        Emission::Call("repeat-output".into())
                    } else {
                        Emission::None
                    },
                }
            };
            if definitions.contains(&name) {
                if let Action::Rules { rules, .. } = &action {
                    if !rules.is_empty() {
                        return err(format!(
                            "{name}: cannot mix compact and explicit action definitions"
                        ));
                    }
                }
            } else {
                actions.insert(name.clone(), action);
            }
            for slot in &mut slots {
                if slot.binding == Binding::Text(vec![marker]) && slot.label != "char:@" {
                    slot.binding = Binding::Named(name.clone());
                    slot.label = name.clone();
                }
            }
        }
        let has_space = slots.iter().any(|s| s.binding == Binding::Text(vec![b' ']))
            || thumbs
                .iter()
                .flatten()
                .any(|t| t.eq_ignore_ascii_case("space") || t == "␠");
        let explicit_absence = thumbs.iter().flatten().any(|token| token == "none");
        if !has_space && !explicit_absence {
            match (thumbs[0].is_some(), thumbs[1].is_some()) {
                (false, false) => thumbs[0] = Some("space".into()),
                (true, false) => thumbs[1] = Some("space".into()),
                (false, true) => thumbs[0] = Some("space".into()),
                _ => {}
            }
        }
        slots.sort_by_key(|s| (s.row, s.col));
        for (hand, t) in thumbs.iter().enumerate() {
            if let Some(t) = t {
                if t == "none" {
                    continue;
                }
                let (binding, label) = binding(t)?;
                slots.push(Slot {
                    binding,
                    label,
                    row: 3,
                    col: hand as i8,
                    row_offset: 0,
                    column_offset: 0,
                    finger: 8 + hand,
                    rank: -1,
                    hand: hand as i8,
                    main: false,
                });
            }
        }
        if let Some(mode) = stagger_mode {
            for slot in slots.iter_mut().filter(|slot| slot.main) {
                slot.row_offset = mode.offset(slot.row);
                slot.finger = mode.finger(slot.row, slot.col, slot.finger);
                if slot.hand == 0 {
                    slot.rank = slot.finger as i8;
                }
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

        for key in short_tables.keys() {
            if definitions.contains(key) {
                return err(format!(
                    "{key}: cannot mix compact and explicit action definitions"
                ));
            }
        }
        install_compact_tables(&mut slots, &mut actions, short_tables)?;
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
            if let Binding::Named(name) = &slot.binding {
                if compact_key(name, true).is_ok() {
                    if let Some(Action::Rules {
                        basis: Basis::Text,
                        fallback: Emission::Text(bytes),
                        ..
                    }) = actions.get(name)
                    {
                        if bytes.as_slice() == name.as_bytes() {
                            slot.label = name.clone();
                        }
                    }
                }
            }
        }
        if let Some(labels) = key_labels {
            if labels.len() != slots.len() {
                return err(
                    "key-labels must contain one label for every physical slot, including thumbs",
                );
            }
            for (slot, label) in slots.iter_mut().zip(labels) {
                slot.label = label;
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
            row_stagger: stagger_mode.unwrap_or_default(),
            action_mode,
        })
    }

    /// A physical text-magic key repeats remembered output when no rule matches.
    /// Adaptive keys are the exception: their own literal output remains fallback.
    pub(crate) fn normalize_magic_fallbacks(&mut self) {
        let mut repeat = "repeat-output".to_string();
        if !matches!(self.actions.get(&repeat), Some(Action::RepeatOutput)) {
            for index in 0.. {
                let name = format!("layouter-repeat-output-{index}");
                if !self.actions.contains_key(&name)
                    || matches!(self.actions.get(&name), Some(Action::RepeatOutput))
                {
                    self.actions.insert(name.clone(), Action::RepeatOutput);
                    repeat = name;
                    break;
                }
            }
        }
        for slot in &self.slots {
            let Binding::Named(name) = &slot.binding else {
                continue;
            };
            if let Some(Action::Rules {
                basis: Basis::Text,
                fallback,
                ..
            }) = self.actions.get_mut(name)
            {
                let dedicated = crate::layout_io::dedicated_magic_label(&slot.label)
                    || crate::layout_io::dedicated_magic_label(name);
                let adaptive = !dedicated
                    && matches!(fallback, Emission::Text(bytes)
                    if bytes.len() == 1 && (bytes.as_slice() == slot.label.as_bytes() || bytes.as_slice() == name.as_bytes()));
                if !adaptive {
                    *fallback = Emission::Call(repeat.clone());
                }
            }
        }
    }

    pub fn extended(&self) -> bool {
        self.action_mode
    }

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

    pub fn text(&self) -> String {
        let token = |s: &Slot| match &s.binding {
            Binding::Empty => "~".into(),
            Binding::Named(n) => {
                if self.actions.get(n).is_some_and(|a| {
                    short_table_text(n, a).is_some() || compact_action_text(n, a).is_some()
                }) {
                    n.clone()
                } else {
                    action_slot_token(n)
                }
            }
            Binding::Text(v) => {
                if v == b" " {
                    "space".into()
                } else if v == b"~"
                    || v == b"|"
                    || v == b"@"
                    || v.len() == 1 && v[0].is_ascii_uppercase()
                {
                    format!("char:{}", v[0] as char)
                } else {
                    String::from_utf8_lossy(v).into_owned()
                }
            }
        };
        let mut out = String::new();
        let absolute = self.slots.iter().any(|slot| slot.main && slot.col == 11);
        for row in 0..3 {
            let mut first = true;
            let mut line = String::new();
            let width = self
                .slots
                .iter()
                .filter(|slot| slot.main && slot.row == row)
                .count();
            let separator = if absolute && width == 12 { 6 } else { 5 };
            for s in self.slots.iter().filter(|s| s.main && s.row == row) {
                if !first {
                    if s.col == separator {
                        line.push_str(" | ");
                    } else {
                        line.push(' ');
                    }
                }
                first = false;
                line.push_str(&token(s));
            }
            out.push_str(&line);
            out.push('\n');
        }
        out.push_str(&self.row_stagger.text());
        out.push_str(&geometry_text(
            self.slots
                .iter()
                .filter(|s| s.main)
                .map(|s| (s.row, s.col, s.row_offset, s.column_offset, s.finger)),
            self.row_stagger,
        ));
        let thumbs: Vec<_> = self.slots.iter().filter(|s| !s.main).collect();
        let is_space = |s: &Slot| s.binding == Binding::Text(vec![b' ']);
        let bare = match thumbs.as_slice() {
            [s] if s.hand == 1 || !is_space(s) => Some(*s),
            [a, b] if is_space(a) ^ is_space(b) => Some(if is_space(a) { *b } else { *a }),
            _ => None,
        };
        if thumbs.is_empty() || thumbs.len() == 1 && !is_space(thumbs[0]) {
            out.push_str("thumbs:");
            for hand in 0..2 {
                out.push(' ');
                if let Some(slot) = thumbs.iter().find(|slot| slot.hand == hand) {
                    out.push_str(&token(slot));
                } else {
                    out.push_str("none");
                }
            }
            out.push('\n');
        } else if let Some(s) = bare {
            if s.hand == 1 {
                out.push_str("            ");
            }
            out.push_str(&token(s));
            out.push('\n');
        } else if !thumbs.is_empty() {
            out.push_str("thumbs:");
            for s in thumbs {
                out.push(' ');
                out.push_str(&token(s));
            }
            out.push('\n');
        }
        if self.slots.iter().any(|slot| {
            matches!(&slot.binding, Binding::Named(name) if slot.label != *name && slot.label != action_slot_token(name))
        }) {
            let labels: Vec<_> = self.slots.iter().map(|slot| crate::json_quote(&slot.label)).collect();
            out.push_str(&format!("key-labels: [{}]\n", labels.join(", ")));
        }
        let mut needed = std::collections::BTreeSet::new();
        fn gather(
            name: &str,
            defs: &BTreeMap<String, Action>,
            needed: &mut std::collections::BTreeSet<String>,
        ) {
            if !needed.insert(name.into()) {
                return;
            }
            if let Some(Action::Rules {
                rules, fallback, ..
            }) = defs.get(name)
            {
                for e in rules.values().chain(std::iter::once(fallback)) {
                    if let Emission::Call(n) = e {
                        gather(n, defs, needed);
                    }
                }
            }
        }
        for slot in &self.slots {
            if let Binding::Named(n) = &slot.binding {
                gather(n, &self.actions, &mut needed);
            }
        }
        for (name, a) in self
            .actions
            .iter()
            .filter(|(name, _)| needed.contains(*name))
        {
            let builtin = match name.as_str() {
                "repeat" | "repeat-output" => matches!(a, Action::RepeatOutput),
                "repeat-action" | "again" => matches!(a, Action::RepeatAction),
                _ => false,
            };
            if builtin
                && !self
                    .slots
                    .iter()
                    .any(|s| matches!(&s.binding, Binding::Named(n) if n == name))
            {
                continue;
            }
            if self
                .slots
                .iter()
                .any(|s| matches!(&s.binding, Binding::Named(n) if n == name))
            {
                if let Some(text) = short_table_text(name, a) {
                    out.push('\n');
                    out.push_str(&text);
                    continue;
                }
            }
            if let Some(text) = compact_action_text(name, a) {
                if !text.is_empty() {
                    out.push('\n');
                    out.push_str(&text);
                }
                continue;
            }
            let kind = match a {
                Action::Text(v) => format!("text {}", quote(v)),
                Action::RepeatOutput => "repeat-output".into(),
                Action::RepeatAction => "repeat-action".into(),
                Action::Inactive => "inactive".into(),
                Action::Rules { basis, .. } => match basis {
                    Basis::Text => "magic",
                    Basis::Press => "press-magic",
                    Basis::SkipPress => "skip-magic",
                    Basis::Output => "output-magic",
                    Basis::SkipOutput => "skip-output-magic",
                    Basis::Remembered => "alternate",
                }
                .into(),
            };
            out.push_str(&format!("\naction {name} = {kind}\n"));
            if let Action::Rules {
                rules, fallback, ..
            } = a
            {
                for (c, e) in rules {
                    out.push_str(&format!("map {name} {} = {}\n", quote(c), emission_text(e)));
                }
                if *fallback != Emission::None {
                    out.push_str(&format!("fallback {name} = {}\n", emission_text(fallback)));
                }
            }
        }
        out
    }

    pub fn save_new(&self) -> Result<PathBuf> {
        let dir = self.path.parent().unwrap_or(Path::new("layouts"));
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let dat = self.text();
        let jsonc = crate::layout_export::jsonc_text(self)?;
        for i in 1..10000 {
            let path = dir.join(format!("{}-actions-{i:03}.dat", self.name));
            match crate::layout_export::save_pair(&path, &dat, &jsonc) {
                Ok(()) => return Ok(path),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return err(e.to_string()),
            }
        }
        err("no unused save filename")
    }
}

fn emission_text(e: &Emission) -> String {
    match e {
        Emission::None => "none".into(),
        Emission::Text(v) => quote(v),
        Emission::Call(n) => format!("@{n}"),
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

fn context<'a>(basis: Basis, l: &'a Layout, mem: &'a Memory, prefix: &'a [u8]) -> &'a [u8] {
    match basis {
        Basis::Text => prefix,
        Basis::Remembered => &mem.remembered_output,
        Basis::Output => &mem.last_output,
        Basis::SkipOutput => &mem.previous_output,
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
        Action::Inactive => None,
        Action::Text(v) => Some(Resolved {
            output: v.clone(),
            remember: true,
            reason: format!("macro @{name}"),
        }),
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
/// character uses one physical key; choose the smallest immediate effort,
/// preferring a literal key on ties. This is not whole-text pathfinding.
pub(crate) struct WindowMapper<'a> {
    layout: &'a Layout,
    literals: [Vec<usize>; 256],
    actions: Vec<usize>,
    previous: [u8; 5],
    previous_len: usize,
    memories: [Memory; 6],
    starts: [usize; 6],
    keys: [Option<usize>; 5],
}

impl<'a> WindowMapper<'a> {
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
                Action::Text(text) => {
                    if text.len() != 1 {
                        return err(format!("Action {name} emits multiple characters; cached n-gram mode requires one character per press."));
                    }
                }
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
        }
    }

    pub(crate) fn map<F>(&mut self, text: &[u8], effort: &F) -> Result<[Option<usize>; 5]>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        if text.is_empty() || text.len() > 5 {
            return err("cached context must contain 1 to 5 characters");
        }
        let common = text
            .iter()
            .zip(&self.previous[..self.previous_len])
            .take_while(|(a, b)| a == b)
            .count();
        for at in common..text.len() {
            let mem = &self.memories[at];
            let start = self.starts[at];
            let mut best: Option<(usize, Resolved, f64)> = None;
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
                if best.as_ref().is_some_and(|(_, _, old)| cost >= *old) {
                    continue;
                }
                best = Some((key, r, cost));
            }
            if let Some((key, r, _)) = best {
                let next = advance(mem, key, &r);
                self.keys[at] = Some(key);
                self.memories[at + 1] = next;
                self.starts[at + 1] = start;
            } else {
                self.keys[at] = None;
                self.memories[at + 1] = Memory::default();
                self.starts[at + 1] = at + 1;
            }
        }
        self.previous[..text.len()].copy_from_slice(text);
        self.previous_len = text.len();
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
    decode_impl(layout, text, limit, cancel, effort, false)
}

/// Corpus policy: type the longest reachable prefix; when no path can proceed,
/// skip one byte and restart with empty typing history. Never delete and join text.
fn decode_corpus<F>(
    layout: &Layout,
    text: &[u8],
    limit: usize,
    cancel: &AtomicBool,
    effort: F,
) -> Result<Vec<Step>>
where
    F: Fn(Option<usize>, Option<usize>, usize) -> f64,
{
    decode_impl(layout, text, limit, cancel, effort, true)
}

fn decode_impl<F>(
    layout: &Layout,
    text: &[u8],
    limit: usize,
    cancel: &AtomicBool,
    effort: F,
    skip_missing: bool,
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
    let mut furthest = 0;
    let mut context_start = 0;
    for at in 0..text.len() {
        if cancel.load(Ordering::Relaxed) {
            return err("cancelled");
        }
        let frontier = std::mem::take(&mut frontiers[at]);
        let best = if skip_missing {
            frontier.values().copied().min_by(|&a, &b| {
                arena[a]
                    .cost
                    .presses
                    .cmp(&arena[b].cost.presses)
                    .then_with(|| arena[a].cost.effort.total_cmp(&arena[b].cost.effort))
            })
        } else {
            None
        };
        for (_, node_id) in frontier {
            let old = arena[node_id].clone();
            for key in 0..layout.slots.len() {
                let Some(resolved) = resolve_slot(
                    key,
                    layout,
                    &old.memory,
                    &text[context_start..at],
                    &mut Vec::new(),
                ) else {
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
                furthest = furthest.max(end);
            }
        }
        if skip_missing && furthest == at {
            if let Some(from) = best {
                if limit > 0 && arena.len() >= limit {
                    return err(format!("typing exceeded {limit} states at byte {at}; increase the state limit or use shorter true sequences"));
                }
                let cost = arena[from].cost;
                let i = arena.len();
                arena.push(Node {
                    memory: Memory::default(),
                    cost,
                    from: Some(from),
                    step: None,
                });
                frontiers[at + 1].insert(Memory::default(), i);
                context_start = at + 1;
                furthest = at + 1;
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

#[derive(Clone, Default, Debug)]
pub struct Counts {
    pub tables: [BTreeMap<Vec<usize>, u64>; 5],
    pub skip: BTreeMap<Vec<usize>, u64>,
    pub characters: u64,
    pub presses: u64,
    pub action_presses: u64,
    pub ignored_characters: u64,
}

impl Counts {
    pub fn add(&mut self, steps: &[Step], weight: u64, layout: &Layout) -> Result<()> {
        let mut run_start = 0;
        for (i, step) in steps.iter().enumerate() {
            if i > 0 && steps[i - 1].end != step.start {
                run_start = i;
            }
            self.presses = self.presses.checked_add(weight).ok_or("count overflow")?;
            self.characters = self
                .characters
                .checked_add(
                    (step.output.len() as u64)
                        .checked_mul(weight)
                        .ok_or("count overflow")?,
                )
                .ok_or("count overflow")?;
            if matches!(layout.slots[step.key].binding, Binding::Named(_)) {
                self.action_presses = self
                    .action_presses
                    .checked_add(weight)
                    .ok_or("count overflow")?;
            }
            for n in 1..=5.min(i + 1 - run_start) {
                let k = steps[i + 1 - n..=i].iter().map(|s| s.key).collect();
                let e = self.tables[n - 1].entry(k).or_default();
                *e = e.checked_add(weight).ok_or("count overflow")?;
            }
            if i - run_start >= 2 {
                let e = self
                    .skip
                    .entry(vec![steps[i - 2].key, step.key])
                    .or_default();
                *e = e.checked_add(weight).ok_or("count overflow")?;
            }
        }
        Ok(())
    }

    fn report_json(&self, layout: &Layout) -> String {
        let mut out = String::from("{\n  \"kind\": \"physical-keystrokes\",\n  \"policy\": \"minimum presses, then local effort\",\n  \"keys\": [");
        for (i, s) in layout.slots.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&crate::json_quote(&s.label));
        }
        out.push_str("],\n  \"ngrams\": [\n");
        for (n, t) in self.tables.iter().enumerate() {
            if n > 0 {
                out.push_str(",\n");
            }
            out.push_str("    [");
            for (j, (keys, count)) in t.iter().enumerate() {
                if j > 0 {
                    out.push(',');
                }
                out.push_str(&format!("[{:?},{}]", keys, count));
            }
            out.push(']');
        }
        out.push_str(&format!(
            "\n  ],\n  \"presses\": {},\n  \"characters\": {},\n  \"ignored_characters\": {}\n}}\n",
            self.presses, self.characters, self.ignored_characters
        ));
        out
    }

    pub fn write_report(&self, layout: &Layout, path: &Path) -> Result<()> {
        let out = self.report_json(layout);
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|e| e.to_string())?;
        f.write_all(out.as_bytes()).map_err(|e| e.to_string())
    }
}

/// ASCII normalization matching the plain default: collapsed spaces, hard control/non-ASCII boundaries.
/// Bytes crossing I/O buffers remain in the same sequence.
pub fn normalize(reader: &mut dyn Read, writer: &mut dyn Write) -> Result<()> {
    let (mut have, mut space) = (false, false);
    let mut buf = [0u8; 65536];
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        for &b in &buf[..n] {
            match b {
                b' ' => {
                    space = have;
                }
                32..=126 => {
                    if space {
                        writer.write_all(b" ").map_err(|e| e.to_string())?;
                    }
                    writer
                        .write_all(&[b.to_ascii_lowercase()])
                        .map_err(|e| e.to_string())?;
                    have = true;
                    space = false;
                }
                _ => {
                    if have {
                        writer.write_all(b"\n").map_err(|e| e.to_string())?;
                    }
                    have = false;
                    space = false;
                }
            }
        }
    }
    if have {
        writer.write_all(b"\n").map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct TextCorpus {
    pub name: String,
    pub sequences: Vec<(Vec<u8>, u64)>,
}

impl TextCorpus {
    pub fn from_bytes(name: &str, bytes: &[u8]) -> Result<Self> {
        let mut normalized = Vec::new();
        normalize(&mut &bytes[..], &mut normalized)?;
        Self::from_normalized(name, &normalized)
    }

    fn from_normalized(name: &str, bytes: &[u8]) -> Result<Self> {
        let mut map: BTreeMap<Vec<u8>, u64> = BTreeMap::new();
        for s in bytes.split(|b| *b == b'\n') {
            if s.is_empty() {
                continue;
            }
            let n = map.entry(s.to_vec()).or_default();
            *n = n.checked_add(1).ok_or("sequence count overflow")?;
        }
        if map.is_empty() {
            return err("no usable text sequences");
        }
        Ok(Self {
            name: name.into(),
            sequences: map.into_iter().collect(),
        })
    }

    pub fn load(path: &Path) -> Result<Self> {
        let name = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .trim_start_matches("corpus-")
            .to_string();
        if path.extension().is_some_and(|s| s == "seq") {
            let b = fs::read(path).map_err(|e| e.to_string())?;
            let body = b
                .strip_prefix(SEQUENCE_HEADER)
                .ok_or("unrecognized ordered-text sidecar")?;
            return Self::from_normalized(&name, body);
        }
        if path.extension().is_some_and(|s| s == "txt") {
            if path.with_extension("config.json").exists() {
                return err("custom normalization is not guessed: provide an explicitly normalized .seq for action layouts");
            }
            let b = fs::read(path).map_err(|e| e.to_string())?;
            return Self::from_bytes(&name, &b);
        }
        let raw = Path::new("corpus/raw").join(format!("{name}.txt"));
        let nearby = path.with_extension("txt");
        // Raw text wins so edits cannot leave action statistics silently stale.
        for p in [raw, nearby] {
            if p.exists() {
                let config = p.with_extension("config.json");
                if config.exists() {
                    return err(format!("{} uses a custom normalizer; supply already normalized .seq text for actions instead of silently applying ASCII defaults", p.display()));
                }
                return Self::load(&p);
            }
        }
        let seq = path.with_extension("seq");
        if seq.exists() {
            return Self::load(&seq);
        }
        err(format!("{} contains frequencies, not ordered typing history. Add corpus/raw/{name}.txt or a matching .seq sidecar; five-gram tables alone cannot reconstruct arbitrary actions.", path.display()))
    }

    pub fn evaluate<F>(
        &self,
        layout: &Layout,
        limit: usize,
        cancel: &AtomicBool,
        effort: F,
    ) -> Result<Counts>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        let mut counts = Counts::default();
        for (text, weight) in &self.sequences {
            if cancel.load(Ordering::Relaxed) {
                return err("cancelled");
            }
            let steps = decode_corpus(layout, text, limit, cancel, &effort)?;
            let retained: usize = steps.iter().map(|s| s.output.len()).sum();
            let ignored = ((text.len() - retained) as u64)
                .checked_mul(*weight)
                .ok_or("count overflow")?;
            counts.ignored_characters = counts
                .ignored_characters
                .checked_add(ignored)
                .ok_or("count overflow")?;
            counts.add(&steps, *weight, layout)?;
        }
        Ok(counts)
    }
}

pub fn write_sidecar(raw: &Path, json: &Path) -> Result<PathBuf> {
    if raw.with_extension("config.json").exists() {
        return err("custom-normalized corpora need an explicitly normalized sequence source for action decoding");
    }
    let path = json.with_extension("seq");
    let tmp = path.with_extension(format!("seq.{}.tmp", std::process::id()));
    let mut output = std::io::BufWriter::new(File::create(&tmp).map_err(|e| e.to_string())?);
    output
        .write_all(SEQUENCE_HEADER)
        .map_err(|e| e.to_string())?;
    let result = normalize(
        &mut File::open(raw).map_err(|e| e.to_string())?,
        &mut output,
    )
    .and_then(|_| output.flush().map_err(|e| e.to_string()));
    drop(output);
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Count text n-grams directly; useful for checking the project's parallel generator.
pub fn text_ngrams(bytes: &[u8]) -> [HashMap<Vec<u8>, u64>; 5] {
    let mut out = std::array::from_fn(|_| HashMap::new());
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
    pub(crate) const SHORTHAND: &str = concat!(
        "~ q w e r t | y u i o p\n",
        "~ a s d f g | h j k l ;\n",
        "~ z x c v b | n m , @ /\n",
        "thumbs: space\n",
        "i@ i'\nr@ rk\na@ aa\n",
    );

    pub(crate) const COMPACT: &str = concat!(
        "f d l w v | q p o u ,\n",
        "s t h y g | z n a e i\n",
        "x k m c j | * b ' ; .\n",
        "thumbs: r space\n",
        "* 'r ay hr ik jo ke rl u' ye\n",
        "swap h nr\n",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    const BASE: &str =
        "q w e r t  y u i o p\na s d f g  h j k l ;\nz x c v b  n m , . /\nthumbs: space\n";
    fn make(extra: &str) -> Layout {
        Layout::parse(&format!("{BASE}{extra}"), Path::new("sample.dat")).unwrap()
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
    fn physical_report_preserves_unicode_and_escaped_key_labels() {
        let mut layout = make("outer-left: ~ ◇ ~\n◇ hr\n");
        let steps = trace_keys(&layout, &[key(&layout, "h"), key(&layout, "◇")]).unwrap();
        let mut counts = Counts::default();
        counts.add(&steps, 1, &layout).unwrap();

        // Programmatic labels must be valid JSON too, even when they contain
        // characters that are not accepted in a .dat key token.
        layout.slots[0].label = "\"\\\n\t".into();
        let report = counts.report_json(&layout);
        let crate::Json::Object(root) = crate::parse_json(&report).unwrap() else {
            panic!("expected report object");
        };
        let Some(crate::Json::Array(labels)) = root.get("keys") else {
            panic!("expected key labels");
        };

        assert_eq!(labels.len(), layout.slots.len());
        for (value, slot) in labels.iter().zip(&layout.slots) {
            let crate::Json::String(label) = value else {
                panic!("expected string label");
            };
            assert_eq!(label, &slot.label);
        }
        assert!(report.contains("\"◇\""));
    }

    #[test]
    fn compact_swap_matches_explicit_actions_and_preserves_uppercase_on_save() {
        let short = make("swap h nr\n");
        let rows = BASE.replacen("n ", "@n ", 1).replacen("r ", "@r ", 1);
        let definitions = concat!(
            "action n = magic\nmap n \"h\" = \"r\"\nfallback n = \"n\"\n",
            "action r = magic\nmap r \"h\" = \"n\"\nfallback r = \"r\"\n",
        );
        let explicit =
            Layout::parse(&format!("{rows}{definitions}"), Path::new("explicit.dat")).unwrap();
        assert_eq!(short.slots, explicit.slots);
        assert_eq!(short.actions, explicit.actions);

        let upper = Layout::parse(
            &format!(
                "{}action n = magic\nmap n \"h\" = \"R\"\nfallback n = \"n\"\n",
                BASE.replacen("n ", "@n ", 1)
            ),
            Path::new("uppercase.dat"),
        )
        .unwrap();
        let round = Layout::parse(&upper.text(), Path::new("round.dat")).unwrap();
        assert_eq!(typed(&round, &["h", "n"]), b"hR");
    }

    #[test]
    fn star_pairs_are_unquoted_context_output_tokens() {
        // Keep exact-output assertions independent of the editable sample layout.
        let source = concat!(
            "f d l w v | q p o u ,\n",
            "s t h y g | z n a e i\n",
            "x k m c j | * b ' ; .\n",
            "thumbs: r space\n",
            "* 'r ay hr ik jo ke rl u' ye\n",
            "swap h nr\n",
        );
        let layout = Layout::parse(source, Path::new("compact.dat")).unwrap();
        for (context, output) in [
            ("'", "r"),
            ("a", "y"),
            ("h", "r"),
            ("i", "k"),
            ("j", "o"),
            ("k", "e"),
            ("r", "l"),
            ("u", "'"),
            ("y", "e"),
        ] {
            assert_eq!(
                typed(&layout, &[context, "*"]),
                format!("{context}{output}").as_bytes()
            );
        }
        assert!(trace_keys(&layout, &[key(&layout, "*")]).is_err());
        // Unlisted contexts repeat remembered output; only missing memory fails.
        assert_eq!(typed(&layout, &["q", "*"]), b"qq");
        assert_eq!(typed(&layout, &["q", "*", "*"]), b"qqq");

        let round = Layout::parse(&layout.text(), Path::new("round.dat")).unwrap();
        assert_eq!(layout.slots, round.slots);
        assert_eq!(layout.actions, round.actions);
        for names in [
            ["h", "n"],
            ["h", "r"],
            ["h", "*"],
            ["q", "n"],
            ["q", "r"],
            ["q", "*"],
        ] {
            assert_eq!(typed(&layout, &names), typed(&round, &names));
        }
    }

    #[test]
    fn grouped_punctuation_swaps_match_separate_rules_and_round_trip() {
        let rows = concat!(
            "b f ◇ p w  ' . u o y\n",
            "n s t d g  k r e a i\n",
            "q v c m x  l h , z j\n",
            "thumbs: space space\n",
            "◇ 'r ay hr ik jo ke rl u' ye\n",
        );
        let grouped = Layout::parse(
            &format!("{rows}swap ' lf j nu u eo y ,u\n"),
            Path::new("grouped.dat"),
        )
        .unwrap();
        let separate = Layout::parse(
            &format!("{rows}swap ' lf\nswap j nu\nswap u eo\nswap y ,u\n"),
            Path::new("separate.dat"),
        )
        .unwrap();
        assert_eq!(grouped.slots, separate.slots);
        assert_eq!(grouped.actions, separate.actions);
        for (keys, output) in [
            (["'", "l"], "'f"),
            (["'", "f"], "'l"),
            (["j", "n"], "ju"),
            (["j", "u"], "jn"),
            (["u", "e"], "uo"),
            (["u", "o"], "ue"),
            (["y", ","], "yu"),
            (["y", "u"], "y,"),
            (["b", ","], "b,"),
        ] {
            assert_eq!(typed(&grouped, &keys), output.as_bytes());
        }
        let round = Layout::parse(&grouped.text(), Path::new("round.dat")).unwrap();
        assert_eq!(grouped.slots, round.slots);
        assert_eq!(grouped.actions, round.actions);
        for invalid in ["swap y ,/u", "swap y ,u j", "swap y ,,", "swap y ,u y ,u"] {
            assert!(Layout::parse(&format!("{rows}{invalid}\n"), Path::new("bad.dat")).is_err());
        }
        // Keep the previous one-context/multiple-pair interpretation.
        let old = make("swap h nr qe\n");
        assert_eq!(typed(&old, &["h", "q"]), b"he");
        assert_eq!(typed(&old, &["h", "n"]), b"hr");

        let long = make("swap th ,u\n");
        let saved = Layout::parse(&long.text(), Path::new("long.dat")).unwrap();
        assert_eq!(long.slots, saved.slots);
        assert_eq!(long.actions, saved.actions);
        assert_eq!(typed(&saved, &["t", "h", ","]), b"thu");
    }

    #[test]
    fn row_stagger_survives_action_swaps_and_save() {
        let mut layout = make("outer-left: ~ @ ~\n@ hr\nrow-stagger: standard\n");
        let before: Vec<_> = layout.slots.iter().map(|s| s.row_offset).collect();
        let action = key(&layout, "@");
        let q = key(&layout, "q");
        layout.swap(action, q);
        assert_eq!(
            before,
            layout
                .slots
                .iter()
                .map(|s| s.row_offset)
                .collect::<Vec<_>>()
        );
        let round = Layout::parse(&layout.text(), Path::new("stagger.dat")).unwrap();
        assert_eq!(layout.slots, round.slots);
        for bad in ["0 1 0", "0 NaN 0", "-3 0 0", "0.0001 0 0", "0 0"] {
            assert!(parse_row_stagger(&format!("row-stagger: {bad}")).is_err());
        }
    }

    #[test]
    fn adaptive_fallback_swap_direction_and_physical_root() {
        let layout = make("swap h nr\n");
        assert_eq!(typed(&layout, &["h", "n"]), b"hr");
        assert_eq!(typed(&layout, &["h", "r"]), b"hn");
        assert_eq!(typed(&layout, &["q", "n", "r"]), b"qnr");
        let n = key(&layout, "n");
        let steps = trace_keys(&layout, &[key(&layout, "h"), n]).unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[1].key, n);
        assert_eq!(steps[1].output, b"r");
        let one_way = make("adaptive n hr\n");
        assert_eq!(typed(&one_way, &["h", "n"]), b"hr");
        assert_eq!(typed(&one_way, &["h", "r"]), b"hr");
        assert_eq!(typed(&one_way, &["n"]), b"n");
    }
    #[test]
    fn concise_rules_can_be_split_over_lines_and_keep_repeat_fallback() {
        let layout = make("outer-left: ~ @ ~\n@ ay\nmagic @ hr\n");
        assert_eq!(typed(&layout, &["a", "@"]), b"ay");
        assert_eq!(typed(&layout, &["q", "@"]), b"qq");
        let round = Layout::parse(&layout.text(), Path::new("round.dat")).unwrap();
        assert_eq!(typed(&round, &["h", "@"]), b"hr");
    }
    #[test]
    fn compact_validation_rejects_ambiguous_or_missing_definitions() {
        for extra in [
            "adaptive n h\n",
            "adaptive n hrr\n",
            "adaptive n hr hr\n",
            "swap h nn\n",
            "swap h nr rn\n",
            "swap h nr\nadaptive n hy\n",
            "adaptive @ hr\n",
            "magic n hr\n",
            "* hr\n",
            "adaptive ? hr\n",
            "adaptive n hr\naction n = inactive\n",
        ] {
            assert!(
                Layout::parse(&format!("{BASE}{extra}"), Path::new("bad.dat")).is_err(),
                "{extra}"
            );
        }
    }
    #[test]
    fn long_context_swap_round_trip_and_candidate_binding_movement() {
        let mut layout = make("swap th nr\n");
        assert_eq!(typed(&layout, &["t", "h", "n"]), b"thr");
        assert_eq!(typed(&layout, &["h", "n"]), b"hn");
        let n = key(&layout, "n");
        let q = key(&layout, "q");
        layout.swap(n, q);
        let steps = trace_keys(&layout, &[key(&layout, "t"), key(&layout, "h"), q]).unwrap();
        assert_eq!(steps[2].key, q);
        assert_eq!(steps[2].output, b"r");
        let round = Layout::parse(&layout.text(), Path::new("round.dat")).unwrap();
        assert_eq!(round.slots, layout.slots);
        assert_eq!(typed(&round, &["t", "h", "n"]), b"thr");
    }
    #[test]
    fn legacy_plain_preserved() {
        let l = make("");
        assert_eq!(l.slots.len(), 31);
        assert!(!l.extended());
        assert_eq!(typed(&l, &["q", "a", "z"]), b"qaz");
    }
    #[test]
    fn outer_pinky_is_same_finger() {
        let l = make("outer-left: ~ ! ~\n");
        let a = &l.slots[key(&l, "a")];
        let outer = &l.slots[key(&l, "!")];
        assert_eq!(a.finger, outer.finger);
        assert_eq!(outer.col, -1);
        assert!(l.home(key(&l, "a")));
        assert!(!l.home(key(&l, "!")));
    }
    #[test]
    fn both_outer_geometry_round_trip() {
        let l = make("outer-left: ~ @rep ~\nouter-right: ~ @rep ~\naction rep = repeat-output\n");
        let again = Layout::parse(&l.text(), Path::new("again.dat")).unwrap();
        assert_eq!(l.slots, again.slots);
    }
    #[test]
    fn plain_save_does_not_add_action_directives() {
        let l = make("");
        assert!(!l.text().contains("action "));
    }
    #[test]
    fn inner_tilde_is_literal_only_when_explicit() {
        let l = Layout::parse(&BASE.replacen("q ", "char:~ ", 1), Path::new("x.dat")).unwrap();
        assert_eq!(l.slots[0].binding, Binding::Text(vec![b'~']));
    }
    #[test]
    fn magic_uses_longest_text_suffix() {
        let l = make("outer-left: ~ @m ~\naction m = magic\nmap m \"q\" = \"u\"\nmap m \"qu\" = \"e\"\nmap m \"u\" = \"a\"\n");
        assert_eq!(typed(&l, &["q", "m", "m"]), b"que");
    }
    #[test]
    fn macro_repeat_repeats_whole_output() {
        let l = make(
            "outer-left: @macro @rep ~\naction macro = text \"the\"\naction rep = repeat-output\n",
        );
        assert_eq!(typed(&l, &["macro", "rep", "rep"]), b"thethethe");
    }
    #[test]
    fn repeat_at_start_emits_nothing() {
        let l = make("outer-left: ~ @rep ~\naction rep = repeat-output\n");
        assert!(trace_keys(&l, &[key(&l, "rep")]).is_err());
    }
    #[test]
    fn repeat_output_and_action_differ() {
        let l = make("outer-left: @m @rep @again\naction m = magic\nmap m \"q\" = \"u\"\nmap m \"qu\" = \"e\"\naction rep = repeat-output\naction again = repeat-action\n");
        assert_eq!(typed(&l, &["q", "m", "rep"]), b"quu");
        assert_eq!(typed(&l, &["q", "m", "again"]), b"que");
    }
    #[test]
    fn fallback_repeat_keeps_remembered_action() {
        let l = make("outer-left: @macro @m @rep\naction macro = text \"th\"\naction m = magic\nfallback m = repeat-output\naction rep = repeat-output\n");
        assert_eq!(typed(&l, &["macro", "m", "rep"]), b"ththth");
    }
    #[test]
    fn alternative_updates_repeat_snapshot() {
        let l = make("outer-left: @alt @rep ~\naction alt = alternate\nmap alt \"e\" = \"u\"\nmap alt \"u\" = \"e\"\naction rep = repeat-output\n");
        assert_eq!(typed(&l, &["e", "alt", "alt"]), b"eue");
        assert_eq!(typed(&l, &["e", "alt", "rep"]), b"euu");
    }
    #[test]
    fn skip_magic_uses_second_last_physical_key() {
        let l = make("outer-left: ~ @sk ~\naction sk = skip-magic\nmap sk \"q\" = \"u\"\nmap sk \"x\" = \"a\"\n");
        assert_eq!(typed(&l, &["q", "x", "sk"]), b"qxu");
    }
    #[test]
    fn physical_history_includes_repeater() {
        let l = make("outer-left: @rep @sk ~\naction rep = repeat-output\naction sk = skip-magic\nmap sk \"q\" = \"u\"\n");
        assert_eq!(typed(&l, &["q", "rep", "sk"]), b"qqu");
    }
    #[test]
    fn magic_can_call_skip_magic() {
        let l = make("outer-left: @m @sk ~\naction m = magic\nmap m \"q\" = @sk\naction sk = skip-magic\nmap sk \"r\" = \"v\"\n");
        assert_eq!(typed(&l, &["r", "q", "m"]), b"rqv");
    }
    #[test]
    fn magic_context_spans_spaces() {
        let l = make("outer-left: ~ @m ~\naction m = magic\nmap m \"in \" = \"the\"\n");
        assert_eq!(typed(&l, &["i", "n", "␠", "m"]), b"in the");
    }
    #[test]
    fn static_cross_call_cycle_is_rejected() {
        assert!(Layout::parse(&format!("{BASE}outer-left: @m @s ~\naction m = magic\nfallback m = @s\naction s = skip-magic\nfallback s = @m\n"), Path::new("x")).is_err());
    }
    #[test]
    fn dynamic_repeat_cycle_does_not_recurse_forever() {
        let l = make("outer-left: @m @again ~\naction m = magic\nmap m \"q\" = \"u\"\nmap m \"u\" = repeat-action\naction again = repeat-action\n");
        assert!(trace_keys(&l, &[key(&l, "q"), key(&l, "m"), key(&l, "again")]).is_err());
    }
    #[test]
    fn no_empty_macro() {
        assert!(
            Layout::parse(&format!("{BASE}action empty = text \"\"\n"), Path::new("x")).is_err()
        );
    }
    #[test]
    fn unsupported_named_actions_are_not_guessed() {
        assert!(Layout::parse(&format!("{BASE}outer-left: ~ @layer ~\n"), Path::new("x")).is_err());
    }
    #[test]
    fn multi_character_main_key_is_rejected() {
        assert!(Layout::parse(
            &BASE.replace("a s d f g", "capslock s d f g"),
            Path::new("x")
        )
        .is_err());
    }
    #[test]
    fn compact_magic_and_repeat_fallback() {
        let l = Layout::parse("~ q w e r t | y u i o p\n~ a s d f g | h j k l ;\n~ z x c v b | n m , @ /\nthumbs: space\n\ni@ i'\nr@ rk\na@ aa\n", Path::new("magic-shorthand.dat")).unwrap();
        assert!(l.extended());
        assert_eq!(typed(&l, &["i", "@"]), b"i'");
        assert_eq!(typed(&l, &["r", "@"]), b"rk");
        assert_eq!(typed(&l, &["a", "@"]), b"aa");
        assert_eq!(typed(&l, &["q", "@"]), b"qq");
        let round = Layout::parse(&l.text(), Path::new("round.dat")).unwrap();
        assert_eq!(typed(&round, &["i", "@"]), b"i'");
    }
    #[test]
    fn indented_single_thumb_selects_hand() {
        let base = BASE
            .replace("q w e r t", "q w e [ t")
            .replace("thumbs: space", "");
        for indent in [12, 13] {
            let l = Layout::parse(
                &format!("{base}{}r\n", " ".repeat(indent)),
                Path::new("right.dat"),
            )
            .unwrap();
            assert_eq!(l.slots[key(&l, "r")].hand, 1);
            assert_eq!(l.slots[key(&l, "␠")].hand, 0);
            assert_eq!(l.text().lines().nth(3), Some("            r"));
        }
        let l = Layout::parse(&format!("{base}          r\n"), Path::new("left.dat")).unwrap();
        assert_eq!(l.slots[key(&l, "r")].hand, 0);
        assert_eq!(l.slots[key(&l, "␠")].hand, 1);
        assert_eq!(l.text().lines().nth(3), Some("r"));
    }
    #[test]
    fn shortest_typing_chooses_macro() {
        let l = make("outer-left: ~ @th ~\naction th = text \"the\"\n");
        let s = decode(&l, b"thethe", 100000, &AtomicBool::new(false), |_, _, _| {
            0.0
        })
        .unwrap();
        assert_eq!(s.len(), 2);
        assert!(s.iter().all(|s| s.output == b"the"));
    }
    #[test]
    fn decoder_never_slices_macro_output() {
        let l = make("outer-left: ~ @macro ~\naction macro = text \"the\"\n");
        let s = decode(&l, b"then", 100000, &AtomicBool::new(false), |_, _, _| 0.0).unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(
            s.iter().flat_map(|s| s.output.clone()).collect::<Vec<_>>(),
            b"then"
        );
    }
    #[test]
    fn exact_decoder_state_limit_errors() {
        let l = make("");
        assert!(decode(&l, b"abc", 1, &AtomicBool::new(false), |_, _, _| 0.0).is_err());
    }
    #[test]
    fn cancellation() {
        let l = make("");
        assert!(decode(&l, b"abc", 1000, &AtomicBool::new(true), |_, _, _| 0.0).is_err());
    }
    #[test]
    fn keystrokes_and_emitted_characters_are_not_ngrams_of_each_other() {
        let l = make("outer-left: ~ @macro ~\naction macro = text \"the\"\n");
        let steps = trace_keys(&l, &vec![key(&l, "macro"); 5]).unwrap();
        let mut c = Counts::default();
        c.add(&steps, 2, &l).unwrap();
        assert_eq!(c.characters, 30);
        assert_eq!(c.presses, 10);
        assert_eq!(c.tables[4].values().sum::<u64>(), 2);
        assert_eq!(c.skip.values().sum::<u64>(), 6);
    }
    #[test]
    fn normalized_boundaries_and_spaces() {
        let mut out = Vec::new();
        normalize(&mut &b"  A  B\r\nC\tD  "[..], &mut out).unwrap();
        assert_eq!(out, b"a b\nc\nd\n");
    }
    #[test]
    fn fivegrams_do_not_cross_sequences() {
        let t = text_ngrams(b"abcde\nfghij\n");
        assert_eq!(t[4].len(), 2);
        assert!(!t[4].contains_key(b"bcdef".as_slice()));
    }
    #[test]
    fn quoted_equals_context() {
        let l = make("outer-left: ~ @m ~\naction m = magic\nmap m \"a = \" = \"b\"\n");
        assert!(matches!(l.actions.get("m"), Some(Action::Rules { .. })));
    }
    #[test]
    fn exact_duplicate_rules_rejected() {
        assert!(Layout::parse(
            &format!("{BASE}action m = magic\nmap m \"a\" = \"b\"\nmap m \"a\" = \"c\"\n"),
            Path::new("x")
        )
        .is_err());
    }
    #[test]
    fn press_rule_distinguishes_magic_key_from_output() {
        let l = make("outer-left: @macro @next ~\naction macro = text \"xy\"\naction next = press-magic\nmap next \"macro\" = \"z\"\n");
        assert_eq!(typed(&l, &["macro", "next"]), b"xyz");
    }
    #[test]
    fn plain_corpus_counts_have_correct_total() {
        let l = make("");
        let c = TextCorpus::from_bytes("x", b"abcde\nabcde\n").unwrap();
        let counts = c
            .evaluate(&l, 10000, &AtomicBool::new(false), |_, _, _| 0.0)
            .unwrap();
        assert_eq!(counts.tables[0].values().sum::<u64>(), 10);
        assert_eq!(counts.tables[4].values().sum::<u64>(), 2);
    }
    #[test]
    fn corpus_ignores_missing_characters_without_joining_runs() {
        let l = make("outer-left: ~ @rep ~\naction rep = repeat-output\n");
        let c = TextCorpus::from_bytes("x", b"abc!def\nabc!def\n!!!\n").unwrap();
        let counts = c
            .evaluate(&l, 10000, &AtomicBool::new(false), |_, _, _| 0.0)
            .unwrap();
        assert_eq!(counts.characters, 12);
        assert_eq!(counts.ignored_characters, 5);
        assert_eq!(counts.presses, 12);
        assert_eq!(counts.tables[1].values().sum::<u64>(), 8);
        assert_eq!(counts.tables[2].values().sum::<u64>(), 4);
        assert!(counts.tables[3].is_empty());
        assert_eq!(counts.skip.values().sum::<u64>(), 4);
    }
    #[test]
    fn missing_characters_reset_magic_text_context() {
        let l = make("outer-left: ~ @m ~\naction m = magic\nmap m \"a\" = \"bc\"\n");
        let c = TextCorpus::from_bytes("x", b"a!bc").unwrap();
        let counts = c
            .evaluate(&l, 10000, &AtomicBool::new(false), |_, _, _| 0.0)
            .unwrap();
        assert_eq!(counts.presses, 3);
        assert_eq!(counts.action_presses, 0);
    }
    #[test]
    fn magic_only_punctuation_is_preserved() {
        let l = make("outer-left: ~ @m ~\naction m = magic\nmap m \"i\" = \"'\"\n");
        let c = TextCorpus::from_bytes("x", b"i'!i'").unwrap();
        let counts = c
            .evaluate(&l, 10000, &AtomicBool::new(false), |_, _, _| 0.0)
            .unwrap();
        assert_eq!(counts.characters, 4);
        assert_eq!(counts.action_presses, 2);
        assert_eq!(counts.tables[1].values().sum::<u64>(), 2);
    }
    #[test]
    fn entirely_unsupported_corpus_is_empty() {
        let c = TextCorpus::from_bytes("x", b"!!!").unwrap();
        let counts = c
            .evaluate(&make(""), 10000, &AtomicBool::new(false), |_, _, _| 0.0)
            .unwrap();
        assert_eq!(counts.presses, 0);
        assert_eq!(counts.ignored_characters, 3);
        assert!(counts.tables.iter().all(|t| t.is_empty()));
    }
    #[test]
    fn corpus_still_reports_decoder_limits() {
        let c = TextCorpus::from_bytes("x", b"!abc!").unwrap();
        assert!(c
            .evaluate(&make(""), 1, &AtomicBool::new(false), |_, _, _| 0.0)
            .unwrap_err()
            .contains("exceeded"));
    }
    #[test]
    fn corpus_handles_contextually_unavailable_punctuation() {
        let l = Layout::parse(
            crate::action_keys::test_layouts::SHORTHAND,
            Path::new("sample.dat"),
        )
        .unwrap();
        let c = TextCorpus::from_bytes("x", b"' it's so delicious.").unwrap();
        let counts = c
            .evaluate(&l, 100000, &AtomicBool::new(false), |_, _, _| 0.0)
            .unwrap();
        assert_eq!(counts.ignored_characters, 3);
        // Two apostrophes and the period.
        assert_eq!(counts.characters, 17);
        let steps =
            decode_corpus(&l, b"it'a", 100000, &AtomicBool::new(false), |_, _, _| 0.0).unwrap();
        assert_eq!(
            steps.iter().map(|s| s.start).collect::<Vec<_>>(),
            vec![0, 1, 3]
        );
        let mut counts = Counts::default();
        counts.add(&steps, 1, &l).unwrap();
        assert_eq!(counts.tables[1].values().sum::<u64>(), 1);
        assert!(counts.tables[2].is_empty());
        assert!(counts.skip.is_empty());
    }
    #[test]
    fn corpus_does_not_truncate_macros_or_ignore_valid_longer_paths() {
        let l = make("outer-left: ~ @macro ~\naction macro = text \"a!b\"\n");
        let steps =
            decode_corpus(&l, b"a!bc", 10000, &AtomicBool::new(false), |_, _, _| 0.0).unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].output, b"a!b");
        assert_eq!(steps[1].start, 3);
        let c = TextCorpus::from_bytes("x", b"a!c").unwrap();
        let counts = c
            .evaluate(&l, 10000, &AtomicBool::new(false), |_, _, _| 0.0)
            .unwrap();
        assert_eq!(counts.characters, 2);
        assert_eq!(counts.ignored_characters, 1);
        assert!(counts.tables[1].is_empty());
    }
    #[test]
    fn corpus_boundaries_reset_effort_and_repeat_history() {
        let l = make("outer-left: ~ @rep ~\naction rep = repeat-output\n");
        let steps = decode_corpus(
            &l,
            b"a!a",
            10000,
            &AtomicBool::new(false),
            |previous, last, _| {
                assert!(previous.is_none());
                assert!(last.is_none());
                0.0
            },
        )
        .unwrap();
        assert!(steps.iter().all(|s| s.reason == "literal"));
        assert_eq!(steps.len(), 2);
    }
    #[test]
    fn strict_text_trace_still_rejects_missing_output() {
        assert!(decode(
            &make(""),
            b"abc!def",
            10000,
            &AtomicBool::new(false),
            |_, _, _| 0.0
        )
        .is_err());
    }
    #[test]
    fn validated_permutations_match_fresh_mapper_for_all_swaps() {
        let source = format!("{}\nouter-right: @sk @again ~\naction sk = skip-magic\nmap sk \"q\" = \"u\"\naction again = repeat-action\n",
            crate::action_keys::test_layouts::SHORTHAND);
        let mut layout = Layout::parse(&source, Path::new("cached.dat")).unwrap();
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
    fn mapper_still_rejects_invalid_initial_definitions() {
        let macro_layout = make("outer-left: ~ @macro ~\naction macro = text \"ab\"\n");
        assert!(WindowMapper::new(&macro_layout, 5).is_err());
        let long_rule = make("outer-left: ~ @m ~\naction m = magic\nmap m \"abc\" = \"d\"\n");
        assert!(WindowMapper::new(&long_rule, 3).is_err());
        assert!(WindowMapper::new(&long_rule, 4).is_ok());
    }
    #[test]
    fn swaps_move_owned_bindings_without_changing_geometry_or_definitions() {
        let mut layout = make("outer-left: ~ @rep ~\naction rep = repeat-output\n");
        let original = layout.clone();
        let a = key(&layout, "a");
        let b = key(&layout, "rep");
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

#[cfg(test)]
mod format_policy_tests {
    use super::*;

    const ROWS: &str = "q w e r t | y u i o p\na s d f g | h j k l ;\nz x c v b | n m , . /\n";

    #[test]
    fn missing_thumbs_are_distinct_from_blank_physical_slots() {
        for (thumbs, count, right) in [
            ("none none", 30, false),
            ("none x", 31, true),
            ("~ x", 32, true),
        ] {
            let layout =
                Layout::parse(&format!("{ROWS}thumbs: {thumbs}\n"), Path::new("inline")).unwrap();
            assert_eq!(layout.slots.len(), count);
            assert_eq!(
                layout.slots.iter().any(|slot| !slot.main && slot.hand == 1),
                right
            );
            let round = Layout::parse(&layout.text(), Path::new("round")).unwrap();
            assert_eq!(layout.slots, round.slots);
        }
    }

    #[test]
    fn magic_repeat_fallback_preserves_explicit_none_and_adaptive_literals() {
        let source = format!("{ROWS}thumbs: @magic space\naction magic = magic\nmap magic \"q\" = none\nfallback magic = none\nswap h nr\n");
        let layout = Layout::parse(&source, Path::new("inline")).unwrap();
        match &layout.actions["magic"] {
            Action::Rules {
                rules, fallback, ..
            } => {
                assert_eq!(rules[b"q".as_slice()], Emission::None);
                assert_eq!(*fallback, Emission::Call("repeat-output".into()));
            }
            _ => panic!("expected text rules"),
        }
        for name in ["n", "r"] {
            match &layout.actions[name] {
                Action::Rules { fallback, .. } => {
                    assert_eq!(*fallback, Emission::Text(name.as_bytes().to_vec()))
                }
                _ => panic!("expected adaptive rules"),
            }
        }
        let round = Layout::parse(&layout.text(), Path::new("round")).unwrap();
        assert_eq!(layout.actions, round.actions);
        assert_eq!(layout.slots, round.slots);
    }

    #[test]
    fn unicode_magic_and_reserved_at_round_trip_without_filename_detection() {
        for marker in ["★", "◇", "@"] {
            let source = format!("{ROWS}thumbs: @{marker} space\naction {marker} = magic\nmap {marker} \"h\" = \"r\"\nfallback {marker} = none\n");
            let layout = Layout::parse(&source, Path::new("seconds")).unwrap();
            let round = Layout::parse(&layout.text(), Path::new("seconds.json")).unwrap();
            assert_eq!(layout.slots, round.slots);
            assert_eq!(layout.actions, round.actions);
        }
    }

    #[test]
    fn overwritten_repeat_builtin_cannot_create_a_normalization_cycle() {
        let source = format!("{ROWS}thumbs: @repeat-output space\naction repeat-output = magic\n");
        let layout = Layout::parse(&source, Path::new("inline")).unwrap();
        let Action::Rules {
            fallback: Emission::Call(name),
            ..
        } = &layout.actions["repeat-output"]
        else {
            panic!("expected compiled repeat fallback");
        };
        assert_ne!(name, "repeat-output");
        assert_eq!(layout.actions[name], Action::RepeatOutput);
        for name in layout.actions.keys() {
            check_calls(name, &layout.actions, &mut Vec::new()).unwrap();
        }
    }

    #[test]
    fn escaped_literal_at_and_uppercase_never_become_magic_or_lowercase() {
        let source = ROWS.replacen("q w", "char:@ char:W", 1);
        let layout = Layout::parse(&source, Path::new("inline")).unwrap();
        assert_eq!(layout.slots[0].binding, Binding::Text(vec![b'@']));
        assert_eq!(layout.slots[1].binding, Binding::Text(vec![b'W']));
        assert!(!layout.extended());
        let round = Layout::parse(&layout.text(), Path::new("round")).unwrap();
        assert_eq!(layout.slots, round.slots);
    }

    #[test]
    fn reachable_overridden_builtin_is_serialized() {
        let source = format!("{ROWS}thumbs: @magic space\naction repeat = text \"x\"\naction magic = magic\nmap magic \"q\" = @repeat\n");
        let layout = Layout::parse(&source, Path::new("inline")).unwrap();
        let round = Layout::parse(&layout.text(), Path::new("round")).unwrap();
        assert_eq!(round.actions["repeat"], Action::Text(b"x".to_vec()));
        assert_eq!(round.actions["magic"], layout.actions["magic"]);
    }
}
