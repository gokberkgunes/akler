//! Lossless JSONC layout exports and coordinated DAT/JSONC saves.
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::action_keys::{Action, Basis, Binding, Emission, Layout, Result};
use crate::Json;

/// Claim both result names before writing either. Only files created by this
/// call are removed on failure; an existing sibling or run record is retained.
pub(crate) fn save_pair(dat_path: &Path, dat: &str, jsonc: &str) -> io::Result<()> {
    let jsonc_path = dat_path.with_extension("jsonc");
    if dat_path == jsonc_path {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a DAT save path",
        ));
    }
    if dat_path
        .with_extension("run.txt")
        .symlink_metadata()
        .is_ok()
    {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "a run record already uses this result name",
        ));
    }
    let mut dat_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dat_path)?;
    let mut jsonc_file = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&jsonc_path)
    {
        Ok(file) => file,
        Err(error) => {
            drop(dat_file);
            let _ = fs::remove_file(dat_path);
            return Err(error);
        }
    };
    let result = (|| {
        dat_file.write_all(dat.as_bytes())?;
        jsonc_file.write_all(jsonc.as_bytes())?;
        dat_file.sync_all()?;
        jsonc_file.sync_all()
    })();
    drop(dat_file);
    drop(jsonc_file);
    if result.is_err() {
        let _ = fs::remove_file(dat_path);
        let _ = fs::remove_file(&jsonc_path);
    }
    result
}

static SAVE_SERIAL: AtomicU64 = AtomicU64::new(0);

fn temporary_file(path: &Path, suffix: &str) -> io::Result<(PathBuf, File)> {
    for _ in 0..1000 {
        let serial = SAVE_SERIAL.fetch_add(1, Ordering::Relaxed);
        let name = format!(
            ".akler-{}-{}-{serial}.{suffix}",
            std::process::id(),
            crate::timestamp()
        );
        let candidate = path.with_file_name(name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => return Ok((candidate, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no unused temporary save filename",
    ))
}

fn staged_file(path: &Path, text: &str) -> io::Result<PathBuf> {
    let (temporary, mut file) = temporary_file(path, "tmp")?;
    let result = (|| {
        if let Ok(metadata) = fs::metadata(path) {
            if !metadata.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "save target is not a file",
                ));
            }
            file.set_permissions(metadata.permissions())?;
        }
        file.write_all(text.as_bytes())?;
        file.sync_all()
    })();
    drop(file);
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(temporary)
}

fn backup_file(path: &Path) -> io::Result<Option<PathBuf>> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "save target is not a file",
            ))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    }
    let (backup, mut file) = temporary_file(path, "bak")?;
    let result = (|| {
        let mut source = File::open(path)?;
        io::copy(&mut source, &mut file)?;
        file.set_permissions(source.metadata()?.permissions())?;
        file.sync_all()
    })();
    drop(file);
    if let Err(error) = result {
        let _ = fs::remove_file(&backup);
        return Err(error);
    }
    Ok(Some(backup))
}

/// The editor's explicitly confirmed overwrite. Prepare and back up both
/// siblings first; a failed second replacement restores the first sibling.
pub(crate) fn replace_pair(
    dat_path: &Path,
    dat: &str,
    jsonc: &str,
    backup: bool,
) -> io::Result<()> {
    let jsonc_path = dat_path.with_extension("jsonc");
    if dat_path == jsonc_path {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a DAT save path",
        ));
    }
    let dat_temporary = staged_file(dat_path, dat)?;
    let jsonc_temporary = match staged_file(&jsonc_path, jsonc) {
        Ok(path) => path,
        Err(error) => {
            let _ = fs::remove_file(dat_temporary);
            return Err(error);
        }
    };
    let mut backups = Vec::new();
    for path in [dat_path, jsonc_path.as_path()] {
        match backup_file(path) {
            Ok(saved) => backups.push(saved),
            Err(error) => {
                let _ = fs::remove_file(&dat_temporary);
                let _ = fs::remove_file(&jsonc_temporary);
                for saved in backups.into_iter().flatten() {
                    let _ = fs::remove_file(saved);
                }
                return Err(error);
            }
        }
    }
    if let Err(error) = fs::rename(&dat_temporary, dat_path) {
        let _ = fs::remove_file(&dat_temporary);
        let _ = fs::remove_file(&jsonc_temporary);
        for saved in backups.into_iter().flatten() {
            let _ = fs::remove_file(saved);
        }
        return Err(error);
    }
    if let Err(error) = fs::rename(&jsonc_temporary, &jsonc_path) {
        let rollback = match &backups[0] {
            Some(saved) => fs::rename(saved, dat_path),
            None => fs::remove_file(dat_path),
        };
        let _ = fs::remove_file(&jsonc_temporary);
        if let Err(restore_error) = rollback {
            // Keep the backup for recovery if the filesystem also rejects rollback.
            return Err(io::Error::new(error.kind(), format!("JSONC save failed: {error}; DAT recovery failed: {restore_error}; backups: {backups:?}")));
        }
        for saved in backups.into_iter().flatten() {
            let _ = fs::remove_file(saved);
        }
        return Err(error);
    }
    if !backup {
        for saved in backups.into_iter().flatten() {
            let _ = fs::remove_file(saved);
        }
    }
    Ok(())
}

fn object(fields: impl IntoIterator<Item = (&'static str, Json)>) -> Json {
    Json::Object(
        fields
            .into_iter()
            .map(|(name, value)| (name.into(), value))
            .collect(),
    )
}

fn string(value: impl Into<String>) -> Json {
    Json::String(value.into())
}

pub(crate) fn json_quote(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch < ' ' => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn json_text(value: &Json, depth: usize, field: &str) -> String {
    match value {
        Json::String(value) => json_quote(value),
        Json::Number(value) => value.to_string(),
        Json::Bool(value) => value.to_string(),
        Json::Null => "null".into(),
        Json::Array(values) if values.is_empty() => "[]".into(),
        Json::Object(values) if values.is_empty() => "{}".into(),
        Json::Array(values) => {
            if !matches!(field, "fingers" | "fingermap" | "rules")
                && values
                    .iter()
                    .all(|value| !matches!(value, Json::Object(_) | Json::Array(_)))
            {
                let items: Vec<_> = values
                    .iter()
                    .map(|value| json_text(value, depth + 1, ""))
                    .collect();
                format!("[{}]", items.join(", "))
            } else {
                let padding = "    ".repeat(depth + 1);
                let item_field = if field == "rules" { "rule" } else { "" };
                let items: Vec<_> = values
                    .iter()
                    .map(|value| format!("{padding}{},\n", json_text(value, depth + 1, item_field)))
                    .collect();
                format!("[\n{}{}]", items.concat(), "    ".repeat(depth))
            }
        }
        Json::Object(values) => {
            let order: &[&str] = match (depth, field) {
                (0, _) => &["layout", "fingermap", "board", "layers", "magic"],
                (_, "layout") => &["fingers", "thumbs"],
                (_, "board") => &[
                    "isRowStaggered",
                    "mirrorLeftRowStagger",
                    "splitAngle",
                    "rowOrColumnStagger",
                ],
                (_, "magic") => &["keys", "wildcards", "rules"],
                (_, "rule") => &["inputs", "output", "call"],
                _ => &[],
            };
            let padding = "    ".repeat(depth + 1);
            let mut fields: Vec<_> = values.iter().collect();
            fields.sort_by_key(|(name, _)| {
                order
                    .iter()
                    .position(|field| *field == name.as_str())
                    .unwrap_or(order.len())
            });
            let lines: Vec<_> = fields
                .into_iter()
                .map(|(name, value)| {
                    format!(
                        "{padding}{}: {},\n",
                        json_quote(name),
                        json_text(value, depth + 1, name)
                    )
                })
                .collect();
            format!("{{\n{}{}}}", lines.concat(), "    ".repeat(depth))
        }
    }
}

fn printable_byte(value: &[u8], what: &str) -> Result<char> {
    if value.len() == 1 && (32..=126).contains(&value[0]) {
        Ok(value[0] as char)
    } else {
        Err(format!(
            "JSONC {what} must be one printable ASCII character"
        ))
    }
}

fn rule_json(
    input: String,
    emission: &Emission,
    prefix: &str,
    labels: &BTreeMap<String, String>,
) -> Result<Json> {
    match emission {
        Emission::Text(text) => Ok(object([
            ("inputs", string(input)),
            (
                "output",
                string(format!("{prefix}{}", printable_byte(text, "rule output")?)),
            ),
        ])),
        Emission::Call(name) => {
            let label = labels
                .get(name)
                .ok_or_else(|| format!("JSONC rule calls unrepresentable action @{name}"))?;
            Ok(object([("inputs", string(input)), ("call", string(label))]))
        }
        Emission::None => Err("JSONC action rules cannot represent a none output".into()),
    }
}

fn action_blocks(layout: &Layout) -> Result<(BTreeMap<String, String>, Option<Json>)> {
    let mut labels = BTreeMap::new();
    let mut label_counts = BTreeMap::new();
    for slot in &layout.slots {
        *label_counts.entry(slot.label.clone()).or_insert(0_usize) += 1;
        if let Binding::Named(name) = &slot.binding {
            if slot.label.chars().count() != 1 || !slot.label.chars().all(|ch| !ch.is_control()) {
                return Err(format!(
                    "JSONC action @{name} needs a one-character physical key label"
                ));
            }
            match labels.insert(name.clone(), slot.label.clone()) {
                Some(previous) if previous != slot.label => {
                    return Err(format!(
                        "JSONC cannot represent different physical labels for action @{name}"
                    ));
                }
                _ => {}
            }
        }
    }
    if let Some(label) = labels
        .values()
        .find(|label| label_counts.get(*label) != Some(&1))
    {
        return Err(format!(
            "JSONC action key {label:?} must occur exactly once"
        ));
    }

    let mut wildcard_rules = Vec::new();
    let mut explicit_rules = BTreeMap::new();
    let mut keys = String::new();
    for (name, label) in &labels {
        let action = layout
            .actions
            .get(name)
            .ok_or_else(|| format!("physical key refers to missing action @{name}"))?;
        let Action::Rules {
            basis,
            rules,
            fallback,
        } = action
        else {
            return Err(format!(
                "JSONC cannot represent bound action @{name}; only magic and skip-magic rules are supported"
            ));
        };
        let expected_name = match label.as_str() {
            "@" => "magic",
            "$" => "skip",
            _ => label,
        };
        if name != expected_name {
            return Err(format!(
                "JSONC action key {label:?} must use action name @{expected_name}, found @{name}"
            ));
        }
        keys.push_str(label);
        match basis {
            Basis::Text => {
                if !matches!(
                    fallback,
                    Emission::Call(fallback_name)
                        if matches!(layout.actions.get(fallback_name), Some(Action::RepeatOutput))
                ) {
                    return Err(format!(
                        "JSONC wildcard magic key {label:?} requires a repeat-output fallback"
                    ));
                }
                wildcard_rules.push(object([
                    ("inputs", string(format!("*{label}"))),
                    ("output", string("**")),
                ]));
                for (context, emission) in rules {
                    let context = std::str::from_utf8(context)
                        .map_err(|_| "JSONC magic contexts must be UTF-8")?;
                    if context.is_empty()
                        || !context.bytes().all(|byte| (32..=126).contains(&byte))
                        || context != context.to_ascii_lowercase()
                    {
                        return Err(
                            "JSONC magic contexts must be nonempty lowercase printable ASCII"
                                .into(),
                        );
                    }
                    if context == "*" {
                        return Err(
                            "JSONC magic context '*' is reserved as the wildcard marker".into()
                        );
                    }
                    if let Emission::Text(output) = emission {
                        let output = printable_byte(output, "magic rule output")?;
                        if output != output.to_ascii_lowercase() {
                            return Err(
                                "JSONC magic rule output must be lowercase printable ASCII".into(),
                            );
                        }
                    } else {
                        return Err("JSONC magic rules cannot call another action".into());
                    }
                    let input = format!("{context}{label}");
                    let rule = rule_json(input.clone(), emission, context, &labels)?;
                    if explicit_rules.insert(input.clone(), rule).is_some() {
                        return Err(format!("JSONC has duplicate magic input {input:?}"));
                    }
                }
            }
            Basis::SkipPress => {
                if !matches!(
                    fallback,
                    Emission::Call(fallback_name)
                        if matches!(
                            layout.actions.get(fallback_name),
                            Some(Action::RepeatPreviousOutput)
                        )
                ) {
                    return Err(format!(
                        "JSONC wildcard skip key {label:?} requires a repeat-previous-output fallback"
                    ));
                }
                wildcard_rules.push(object([
                    ("inputs", string(format!("*_{label}"))),
                    ("output", string("*_*")),
                ]));
                for (context, emission) in rules {
                    let context = if let Some(label) = labels.get(
                        std::str::from_utf8(context)
                            .map_err(|_| "JSONC skip contexts must be UTF-8")?,
                    ) {
                        label.clone()
                    } else {
                        printable_byte(context, "skip context")?.to_string()
                    };
                    if context == "*" {
                        return Err(
                            "JSONC skip context '*' is reserved as the wildcard marker".into()
                        );
                    }
                    let prefix = format!("{context}_");
                    let input = format!("{prefix}{label}");
                    let rule = rule_json(input.clone(), emission, &prefix, &labels)?;
                    if explicit_rules.insert(input.clone(), rule).is_some() {
                        return Err(format!("JSONC has duplicate magic input {input:?}"));
                    }
                }
            }
            _ => {
                return Err(format!(
                    "JSONC cannot represent action @{name} with {basis:?} history"
                ));
            }
        }
    }

    if labels.is_empty() {
        return Ok((labels, None));
    }
    wildcard_rules.extend(explicit_rules.into_values());
    Ok((
        labels,
        Some(object([
            ("keys", string(keys)),
            ("wildcards", string("*")),
            ("rules", Json::Array(wildcard_rules)),
        ])),
    ))
}

fn token(binding: &Binding, action_labels: &BTreeMap<String, String>) -> Result<String> {
    match binding {
        Binding::Empty => Ok("skip".into()),
        Binding::Named(name) => action_labels
            .get(name)
            .cloned()
            .ok_or_else(|| format!("JSONC cannot represent action @{name}")),
        Binding::Text(text) if text == b" " => Ok("space".into()),
        Binding::Text(text) if text.len() == 1 && text[0].is_ascii_graphic() => {
            let ch = text[0] as char;
            if matches!(ch, '~' | '|' | '*' | '@') || ch.is_ascii_uppercase() {
                Ok(format!("char:{ch}"))
            } else {
                Ok(ch.to_string())
            }
        }
        Binding::Text(_) => {
            Err("JSONC physical text keys must contain one printable ASCII character".into())
        }
    }
}

fn offsets(values: impl IntoIterator<Item = i16>) -> Json {
    Json::Array(
        values
            .into_iter()
            .map(|value| Json::Number(value as f64 / 1000.0))
            .collect(),
    )
}

/// Export the subset that the readable Mana-style JSONC fields preserve exactly.
pub(crate) fn jsonc_text(layout: &Layout) -> Result<String> {
    let (action_labels, magic) = action_blocks(layout)?;
    for slot in &layout.slots {
        let expected = match &slot.binding {
            Binding::Empty => Some("·".to_string()),
            Binding::Text(text) if text == b" " => Some("␠".to_string()),
            Binding::Text(text) => Some(
                std::str::from_utf8(text)
                    .map_err(|_| "non-UTF-8 physical key")?
                    .to_string(),
            ),
            Binding::Named(_) => None,
        };
        if expected.is_some_and(|expected| expected != slot.label) {
            return Err(format!(
                "JSONC cannot represent custom display label {:?} on a literal or empty key",
                slot.label
            ));
        }
    }
    let mut rows = Vec::new();
    let mut finger_rows = Vec::new();
    let mut row_offsets = Vec::new();
    let mut columns = BTreeMap::new();
    let mut row_width = None;
    // Internal finger IDs are left fingers 0..3, right fingers 4..7,
    // thumbs 8..9. Mana places the two thumbs between the hands.
    const MANA_FINGERS: [usize; 10] = [0, 1, 2, 3, 6, 7, 8, 9, 4, 5];
    for row in 0..3 {
        let mut slots: Vec<_> = layout
            .slots
            .iter()
            .filter(|slot| slot.main && slot.row == row)
            .collect();
        slots.sort_by_key(|slot| slot.col);
        if !(10..=12).contains(&slots.len()) {
            return Err(format!(
                "JSONC export needs 10, 11, or 12 physical slots in row {}",
                row + 1
            ));
        }
        if let Some(width) = row_width {
            if slots.len() != width {
                return Err("JSONC export needs equally wide rows; add empty physical slots for missing positions".into());
            }
        } else {
            row_width = Some(slots.len());
        }
        let origin = slots[0].col;
        if origin != 0
            || slots
                .iter()
                .enumerate()
                .any(|(index, slot)| slot.col != origin + index as i8)
        {
            return Err(format!(
                "JSONC cannot represent row {} column origin; use DAT for this geometry",
                row + 1
            ));
        }
        let row_offset = slots[0].row_offset;
        if slots.iter().any(|slot| slot.row_offset != row_offset) {
            return Err(
                "JSONC cannot represent different horizontal offsets within one row".into(),
            );
        }
        row_offsets.push(row_offset);
        let mut fingers = Vec::new();
        for slot in &slots {
            let finger = MANA_FINGERS
                .get(slot.finger)
                .ok_or("invalid physical finger ID")?;
            fingers.push(finger.to_string());
            if let Some(previous) = columns.insert(slot.col, slot.column_offset) {
                if previous != slot.column_offset {
                    return Err(
                        "JSONC cannot represent different vertical offsets within one column"
                            .into(),
                    );
                }
            }
        }
        rows.push(string(
            slots
                .iter()
                .map(|slot| token(&slot.binding, &action_labels))
                .collect::<Result<Vec<_>>>()?
                .join(" "),
        ));
        finger_rows.push(string(fingers.join(" ")));
    }

    // Indentation is a visual hint only; board offsets remain authoritative.
    let leftmost = row_offsets.iter().copied().min().unwrap_or(0);
    for row in 0..3 {
        let delta = i32::from(row_offsets[row]) - i32::from(leftmost);
        let cells = (delta as f64 / 500.0).round() as usize;
        for value in [&mut rows[row], &mut finger_rows[row]] {
            if let Json::String(text) = value {
                *text = format!("{}{text}", " ".repeat(cells));
            }
        }
    }

    let mut thumbs = [Vec::new(), Vec::new()];
    for slot in layout.slots.iter().filter(|slot| !slot.main) {
        let hand = usize::try_from(slot.hand).map_err(|_| "invalid thumb hand")?;
        if hand > 1 {
            return Err("invalid thumb hand".into());
        }
        if slot.row_offset != 0
            || slot.column_offset != 0
            || slot.finger != 8 + hand
            || slot.rank != -1
        {
            return Err("JSONC cannot represent custom thumb geometry".into());
        }
        thumbs[hand].push(token(&slot.binding, &action_labels)?);
    }
    let thumbs = thumbs
        .into_iter()
        .map(|group| string(group.join(" ")))
        .collect();
    let column_offsets: Vec<_> = columns.values().copied().collect();
    let has_columns = column_offsets.iter().any(|offset| *offset != 0);
    let has_rows = row_offsets.iter().any(|offset| *offset != 0);
    if has_columns && has_rows {
        return Err(
            "JSONC cannot represent row and column offsets together; use DAT for this geometry"
                .into(),
        );
    }
    let board = object([
        ("isRowStaggered", Json::Bool(!has_columns && has_rows)),
        ("mirrorLeftRowStagger", Json::Bool(false)),
        ("splitAngle", Json::Number(0.0)),
        (
            "rowOrColumnStagger",
            if !has_columns && has_rows {
                offsets(row_offsets.iter().copied())
            } else {
                offsets(column_offsets)
            },
        ),
    ]);
    let mut root = BTreeMap::new();
    root.insert(
        "layout".into(),
        object([
            ("fingers", Json::Array(rows)),
            ("thumbs", Json::Array(thumbs)),
        ]),
    );
    root.insert("fingermap".into(), Json::Array(finger_rows));
    root.insert("board".into(), board);
    root.insert("layers".into(), Json::Null);
    if let Some(magic) = magic {
        root.insert("magic".into(), magic);
    }
    let text = format!("{}\n", json_text(&Json::Object(root), 0, ""));
    let restored = crate::layout_io::parse_json_layout(&text, &layout.path)
        .map_err(|error| format!("JSONC export failed its reimport check: {error}"))?;
    if restored.slots != layout.slots
        || restored.actions != layout.actions
        || restored.left_outer != layout.left_outer
        || restored.right_outer != layout.right_outer
    {
        return Err("JSONC cannot preserve this layout exactly; save and use its DAT form".into());
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRID: &str = "q w e r t | y u i o p\na s d f g | h j k l ;\nz x c v b | n m , . /\n";

    fn round_trip(source: &str) -> (Layout, Layout) {
        let original = Layout::parse(source, Path::new("original.dat")).unwrap();
        // Pair saves intentionally retain the reachable definitions emitted by DAT.
        let original = Layout::parse(&original.text(), Path::new("canonical.dat")).unwrap();
        let jsonc = jsonc_text(&original).unwrap();
        let restored = Layout::parse(&jsonc, Path::new("seconds")).unwrap();
        assert_eq!(restored.slots, original.slots, "{jsonc}");
        assert_eq!(restored.actions, original.actions, "{jsonc}");
        assert_eq!(restored.extended(), original.extended());
        (original, restored)
    }

    #[test]
    fn ordinary_jsonc_round_trip_keeps_single_thumb_and_literal_punctuation() {
        let source = GRID.replace("q w e r t", "char:* char:~ char:| char:@ char:Q");
        let (original, restored) = round_trip(&source);
        assert_eq!(restored.slots.iter().filter(|slot| !slot.main).count(), 1);
        assert!(!original.extended());
        assert!(!restored.extended());
    }

    #[test]
    fn unequal_dat_rows_need_explicit_empty_slots_for_jsonc() {
        let source = GRID.replace("a s d f g | h j k l ;", "a s d f g | h j k l ; ?");
        let layout = Layout::parse(&source, Path::new("unequal.dat")).unwrap();
        let error = jsonc_text(&layout).unwrap_err();
        assert!(error.contains("equally wide rows"), "{error}");
    }

    #[test]
    fn jsonc_uses_handwritten_field_order_and_one_keyboard_row_per_line() {
        let source = format!("{GRID}thumbs: space\nrow-offsets: 0 0.25 0.75\n");
        let layout = Layout::parse(&source, Path::new("inline")).unwrap();
        let text = jsonc_text(&layout).unwrap();
        assert!(text.starts_with("{\n    \"layout\": {\n        \"fingers\": [\n"));
        for row in [
            "q w e r t y u i o p",
            " a s d f g h j k l ;",
            "  z x c v b n m , . /",
        ] {
            assert!(text.contains(&format!("            \"{row}\",\n")));
        }
        assert!(text.contains("        \"thumbs\": [\"space\", \"\"],\n"));
        assert!(text.contains("    \"fingermap\": [\n        \"0 1 2 3 3 6 6 7 8 9\",\n"));
        assert!(text.contains("        \"rowOrColumnStagger\": [0, 0.25, 0.75],\n"));

        let fields = ["layout", "fingermap", "board", "layers"];
        let positions = fields.map(|field| text.find(&format!("    \"{field}\":")).unwrap());
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(!text.contains("\"magic\":"));
        let restored = Layout::parse(&text, Path::new("round")).unwrap();
        assert_eq!(restored.slots, layout.slots);
        assert_eq!(restored.actions, layout.actions);
        assert_eq!(jsonc_text(&restored).unwrap(), text);
    }

    #[test]
    fn unified_magic_writes_wildcard_before_explicit_rules() {
        let original = Layout::parse(
            include_str!("../layouts/afterburner.jsonc"),
            Path::new("afterburner.jsonc"),
        )
        .unwrap();
        let text = jsonc_text(&original).unwrap();
        let wildcard = text.find("\"inputs\": \"*#\"").unwrap();
        let explicit = text.find("\"inputs\": \"a#\"").unwrap();
        assert!(wildcard < explicit, "{text}");
        assert!(text.contains("\"output\": \"**\""), "{text}");
    }

    #[test]
    fn simple_rule_round_trip_preserves_physical_mapping_and_metric_bits() {
        use crate::{action_ngrams, action_ui, AtomicBool, AtomicU64, Weights};

        let layout = Layout::parse(
            include_str!("../layouts/afterburner.jsonc"),
            Path::new("afterburner.jsonc"),
        )
        .unwrap();
        let text = jsonc_text(&layout).unwrap();
        let restored = Layout::parse(&text, Path::new("round")).unwrap();
        let tables = crate::action_keys::text_ngrams(b"hr hrr aa a' zz zzz ju jn yu y, q!qq");
        let names = ["letters", "bigrams", "trigrams", "fourgrams", "fivegrams"];
        let fields: Vec<_> = tables
            .iter()
            .zip(names)
            .map(|(table, name)| {
                let entries: Vec<_> = table
                    .iter()
                    .map(|(text, count)| format!("{}:{count}", crate::action_keys::quote(text)))
                    .collect();
                format!("{}:{{{}}}", json_quote(name), entries.join(","))
            })
            .collect();
        let corpus = action_ngrams::NgramCorpus::from_text(
            &format!("{{{}}}", fields.join(",")),
            Path::new("inline-corpus"),
        )
        .unwrap();
        let stop = AtomicBool::new(false);
        let progress = AtomicU64::new(0);
        let weights = Weights::default();
        let effort = action_ui::LocalEffort::new(&layout, &weights);
        let original_counts = corpus
            .evaluate(&layout, &stop, &progress, |a, b, k| effort.get(a, b, k))
            .unwrap();
        let restored_counts = corpus
            .evaluate(&restored, &stop, &progress, |a, b, k| effort.get(a, b, k))
            .unwrap();
        assert_eq!(restored_counts.tables, original_counts.tables);
        assert_eq!(restored_counts.skip, original_counts.skip);

        let original =
            action_ui::evaluate_progress(&layout, &corpus, &weights, &stop, &progress).unwrap();
        let round =
            action_ui::evaluate_progress(&restored, &corpus, &weights, &stop, &progress).unwrap();
        for (a, b) in round.raw.0.iter().zip(&original.raw.0) {
            assert_eq!(a.to_bits(), b.to_bits());
        }
        assert_eq!(round.score.to_bits(), original.score.to_bits());
    }

    #[test]
    fn unsupported_action_rules_fail_instead_of_exporting_lossily() {
        let rows = GRID.replace("q w e", "q w @*");
        for definitions in [
            "action * = magic\nmap * \"q\" = none\n",
            "action * = magic\nmap * \"q\" = \"U\"\n",
            "action * = magic\nmap * \"Q\" = \"u\"\n",
            "action * = magic\nmap * \"q\" = @leaf\naction leaf = text \"u\"\n",
            "action * = repeat-action\n",
            "action * = press-magic\nmap * \"q\" = \"u\"\n",
            "action * = magic\nmap * \"q\" = \"ui\"\n",
        ] {
            let layout =
                Layout::parse(&format!("{rows}{definitions}"), Path::new("inline")).unwrap();
            let error = jsonc_text(&layout).unwrap_err();
            assert!(error.contains("JSONC"), "{error}");
        }
    }

    #[test]
    fn literal_and_adaptive_keys_with_same_label_are_not_merged() {
        let source = format!("{}adaptive n hr\n", GRID.replace("q w e", "n w e"),);
        let mut layout = Layout::parse(&source, Path::new("inline")).unwrap();
        layout.slots[0].binding = Binding::Text(b"n".to_vec());
        assert!(jsonc_text(&layout)
            .unwrap_err()
            .contains("must occur exactly once"));
    }

    #[test]
    fn unsupported_action_graph_fails_instead_of_exporting_lossily() {
        let source = format!("{}action ◇ = magic\nmap ◇ \"q\" = \"u\"\nmap ◇ \"x\" = none\naction a = magic\nmap a \"i\" = \"o\"\nfallback a = \"a\"\naction press = press-magic\nmap press \"q\" = @macro\nfallback press = none\naction macro = text \"the\"\naction again-key = repeat-action\n", GRID.replace("q w e r t", "@◇ @a @press @again-key t").replace("a s d f g", "q s d f g"));
        let mut original = Layout::parse(&source, Path::new("source")).unwrap();
        original.swap(0, 10);
        original.swap(1, 20);
        let error = jsonc_text(&original).unwrap_err();
        assert!(error.contains("JSONC"), "{error}");
    }

    #[test]
    fn both_stagger_axes_fail_instead_of_exporting_lossily() {
        let source = "q w e r t y | u i o p [ ]\na s d f g h | j k l ; ' /\nz x c v b n | m , . = - \\\nrow-offsets: -0.25 0 0.5\ncolumn-offsets: 0 0 -0.2 -0.3 -0.1 0 0 -0.1 -0.3 -0.2 0 0\nfingermap: LP LP LR LM LI LI / LP LP LR LM LI LI / LP LP LR LM LI LI\n";
        // Supply complete custom finger rows separately to keep the geometry explicit.
        let source = source.replace("LP LP LR LM LI LI / LP LP LR LM LI LI / LP LP LR LM LI LI", "LP LP LR LM LI LI RI RI RM RR RP RP / LP LP LR LM LI LI RI RI RM RR RP RP / LP LP LR LM LI LI RI RI RM RR RP RP");
        let layout = Layout::parse(&source, Path::new("source")).unwrap();
        let error = jsonc_text(&layout).unwrap_err();
        assert!(error.contains("geometry"), "{error}");
    }

    #[test]
    fn standalone_repeat_action_fails_instead_of_exporting_lossily() {
        let source = format!("{}action m = repeat-output\n", GRID.replace("q w", "@m w"));
        let original = Layout::parse(&source, Path::new("source")).unwrap();
        assert!(jsonc_text(&original)
            .unwrap_err()
            .contains("one-character physical key label"));
    }

    #[test]
    fn skip_magic_round_trips_in_the_unified_magic_block() {
        let source = format!(
            "{}action skip = skip-magic\nfallback skip = repeat-previous-output\n",
            GRID.replace("q w", "@skip w")
        );
        let mut original = Layout::parse(&source, Path::new("source")).unwrap();
        original.slots[0].label = "$".into();
        let jsonc = jsonc_text(&original).unwrap();
        assert!(jsonc.contains("\"keys\": \"$\""), "{jsonc}");
        assert!(jsonc.contains("\"wildcards\": \"*\""), "{jsonc}");
        assert!(jsonc.contains("\"inputs\": \"*_$\""), "{jsonc}");
        assert!(jsonc.contains("\"output\": \"*_*\""), "{jsonc}");
        assert!(!jsonc.contains("\"skip\": {"), "{jsonc}");
        let restored = Layout::parse(&jsonc, Path::new("round.jsonc")).unwrap();
        assert_eq!(restored.actions, original.actions);
        assert_eq!(
            restored.actions["repeat-previous-output"],
            Action::RepeatPreviousOutput
        );
    }

    #[test]
    fn afterburner_export_uses_one_magic_block_with_both_keys() {
        let original = Layout::parse(
            include_str!("../layouts/afterburner.jsonc"),
            Path::new("afterburner.jsonc"),
        )
        .unwrap();
        let jsonc = jsonc_text(&original).unwrap();
        assert!(jsonc.contains("i e a $ # m t s n h q\""), "{jsonc}");
        assert!(jsonc.contains("\"thumbs\": [\"\", \"l r\"]"), "{jsonc}");
        assert!(
            jsonc.contains("\"keys\": \"#$\"") || jsonc.contains("\"keys\": \"$#\""),
            "{jsonc}"
        );
        assert!(jsonc.contains("\"wildcards\": \"*\""), "{jsonc}");
        assert!(jsonc.contains("\"inputs\": \"*#\""), "{jsonc}");
        assert!(jsonc.contains("\"output\": \"**\""), "{jsonc}");
        assert!(jsonc.contains("\"inputs\": \"a#\""), "{jsonc}");
        assert!(jsonc.contains("\"output\": \"ao\""), "{jsonc}");
        assert!(jsonc.contains("\"inputs\": \"*_$\""), "{jsonc}");
        assert!(jsonc.contains("\"output\": \"*_*\""), "{jsonc}");
        assert!(jsonc.contains("\"inputs\": \"#_$\""), "{jsonc}");
        assert!(jsonc.contains("\"call\": \"#\""), "{jsonc}");
        assert!(!jsonc.contains("\"skip\":"), "{jsonc}");

        let restored = Layout::parse(&jsonc, Path::new("round.jsonc")).unwrap();
        assert_eq!(restored.slots, original.slots, "{jsonc}");
        assert_eq!(restored.actions, original.actions, "{jsonc}");
    }

    #[test]
    fn unsupported_magic_fallbacks_fail_clearly() {
        let original =
            Layout::parse(include_str!("../layouts/opal.dat"), Path::new("opal.dat")).unwrap();
        let error = jsonc_text(&original).unwrap_err();
        assert!(error.contains("wildcard magic key"), "{error}");
    }

    #[test]
    fn unsupported_literal_labels_fail_without_a_lossy_export() {
        let mut layout = Layout::parse(GRID, Path::new("source")).unwrap();
        layout.slots[0].label = "custom q".into();
        assert!(jsonc_text(&layout)
            .unwrap_err()
            .contains("custom display label"));
    }

    fn test_directory() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "akler-pair-test-{}-{}",
            std::process::id(),
            crate::timestamp()
        ));
        fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn new_pair_preserves_existing_sibling_and_run_record() {
        let directory = test_directory();
        let path = directory.join("result.dat");
        let jsonc_path = path.with_extension("jsonc");
        fs::write(&jsonc_path, "original JSONC").unwrap();
        assert_eq!(
            save_pair(&path, "DAT", "JSONC").unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert!(!path.exists());
        assert_eq!(fs::read_to_string(&jsonc_path).unwrap(), "original JSONC");
        fs::remove_file(&jsonc_path).unwrap();
        fs::write(path.with_extension("run.txt"), "original record").unwrap();
        assert_eq!(
            save_pair(&path, "DAT", "JSONC").unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert!(!path.exists());
        assert!(!jsonc_path.exists());
        assert_eq!(
            fs::read_to_string(path.with_extension("run.txt")).unwrap(),
            "original record"
        );
        fs::remove_file(path.with_extension("run.txt")).unwrap();
        fs::write(&path, "original DAT").unwrap();
        assert_eq!(
            save_pair(&path, "DAT", "JSONC").unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "original DAT");
        assert!(!jsonc_path.exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn overwrite_prepares_both_siblings_before_changing_existing_dat() {
        let directory = test_directory();
        let path = directory.join("result.dat");
        fs::write(&path, "original DAT").unwrap();
        fs::create_dir(path.with_extension("jsonc")).unwrap();
        assert!(replace_pair(&path, "new DAT", "new JSONC", true).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "original DAT");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn new_and_confirmed_overwrite_save_both_files() {
        let directory = test_directory();
        let path = directory.join("result.dat");
        save_pair(&path, "DAT one", "JSONC one").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "DAT one");
        assert_eq!(
            fs::read_to_string(path.with_extension("jsonc")).unwrap(),
            "JSONC one"
        );
        replace_pair(&path, "DAT two", "JSONC two", false).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "DAT two");
        assert_eq!(
            fs::read_to_string(path.with_extension("jsonc")).unwrap(),
            "JSONC two"
        );
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 2);
        fs::remove_dir_all(directory).unwrap();
    }
}
