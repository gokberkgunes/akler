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

fn bytes(value: &[u8]) -> Result<Json> {
    std::str::from_utf8(value)
        .map(string)
        .map_err(|_| "cannot export a non-UTF-8 layout value to JSONC".into())
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
                (0, _) => &[
                    "layout",
                    "fingermap",
                    "board",
                    "layers",
                    "magic",
                    "akler",
                ],
                (_, "layout") => &["fingers", "thumbs"],
                (_, "board") => &[
                    "isRowStaggered",
                    "mirrorLeftRowStagger",
                    "splitAngle",
                    "rowOrColumnStagger",
                ],
                (_, "magic") => &["magicKeys", "rules"],
                (_, "rule") => &["inputs", "output"],
                (_, "akler") => &[
                    "version",
                    "actions",
                    "labels",
                    "columnOrigins",
                    "rowOffsets",
                ],
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

/// Use ordinary input/output rules only when reimport reproduces every physical
/// binding and reachable action definition. Calls, explicit none, aliases, and
/// other histories keep their native representation instead of losing semantics.
fn simple_jsonc(
    root: &BTreeMap<String, Json>,
    layout: &Layout,
    needed: &std::collections::BTreeSet<String>,
) -> Option<String> {
    let mut rules = BTreeMap::new();
    for slot in &layout.slots {
        let Binding::Named(name) = &slot.binding else {
            continue;
        };
        if name != &slot.label || name.chars().count() != 1 {
            return None;
        }
        let Action::Rules {
            basis: Basis::Text,
            rules: entries,
            ..
        } = layout.actions.get(name)?
        else {
            return None;
        };
        for (context, emission) in entries {
            let Emission::Text(output) = emission else {
                return None;
            };
            if output.len() != 1 {
                return None;
            }
            let context = std::str::from_utf8(context).ok()?;
            let output = std::str::from_utf8(output).ok()?;
            rules.insert(format!("{context}{name}"), format!("{context}{output}"));
        }
    }

    let mut simple = root.clone();
    let Json::Object(keys) = simple.get_mut("layout")? else {
        return None;
    };
    for field in ["fingers", "thumbs"] {
        let Json::Array(values) = keys.get_mut(field)? else {
            return None;
        };
        for value in values {
            let Json::String(text) = value else {
                return None;
            };
            // Retain cosmetic row indentation and literal char:@ escapes.
            *text = text
                .split(' ')
                .map(|token| token.strip_prefix('@').unwrap_or(token))
                .collect::<Vec<_>>()
                .join(" ");
        }
    }
    let remove_extension = if let Some(Json::Object(native)) = simple.get_mut("akler") {
        native.remove("actions");
        native.remove("labels");
        native.len() == 1
    } else {
        false
    };
    if remove_extension {
        simple.remove("akler");
    }

    let rules = rules
        .into_iter()
        .map(|(input, output)| object([("inputs", string(input)), ("output", string(output))]))
        .collect();
    simple.insert(
        "magic".into(),
        object([("magicKeys", Json::Null), ("rules", Json::Array(rules))]),
    );

    let text = format!("{}\n", json_text(&Json::Object(simple), 0, ""));
    let restored = crate::layout_io::parse_json_layout(&text, &layout.path).ok()?;
    if restored.slots != layout.slots
        || restored.left_outer != layout.left_outer
        || restored.right_outer != layout.right_outer
        || restored.extended() != layout.extended()
        || needed
            .iter()
            .any(|name| restored.actions.get(name) != layout.actions.get(name))
    {
        return None;
    }
    Some(text)
}

fn emission_json(emission: &Emission) -> Result<Json> {
    Ok(match emission {
        Emission::None => Json::Null,
        Emission::Text(text) => object([("text", bytes(text)?)]),
        Emission::Call(name) => object([("call", string(name))]),
    })
}

fn action_json(action: &Action) -> Result<Json> {
    Ok(match action {
        Action::Text(text) => object([("kind", string("text")), ("text", bytes(text)?)]),
        Action::RepeatOutput => object([("kind", string("repeat-output"))]),
        Action::RepeatAction => object([("kind", string("repeat-action"))]),
        Action::Inactive => object([("kind", string("inactive"))]),
        Action::Rules {
            basis,
            rules,
            fallback,
        } => {
            let basis = match basis {
                Basis::Text => "text",
                Basis::Press => "press",
                Basis::SkipPress => "skip-press",
                Basis::Output => "output",
                Basis::SkipOutput => "skip-output",
                Basis::Remembered => "remembered",
            };
            let mut entries = BTreeMap::new();
            for (context, emission) in rules {
                let context = std::str::from_utf8(context)
                    .map_err(|_| "cannot export non-UTF-8 action context")?;
                entries.insert(context.into(), emission_json(emission)?);
            }
            object([
                ("kind", string("rules")),
                ("basis", string(basis)),
                ("rules", Json::Object(entries)),
                ("fallback", emission_json(fallback)?),
            ])
        }
    })
}

fn token(binding: &Binding) -> Result<String> {
    match binding {
        Binding::Empty => Ok("skip".into()),
        Binding::Named(name) => {
            validate_name(name)?;
            Ok(format!("@{name}"))
        }
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

/// The visible layout, fingermap, and board fields are the editable source of
/// positions. The extension stores only action semantics/labels and geometry
/// that the Mana fields cannot express (a second axis or legacy column origin).
pub(crate) fn jsonc_text(layout: &Layout) -> Result<String> {
    let mut layout = layout.clone();
    layout.normalize_magic_fallbacks();
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
    let mut origins = Vec::new();
    let mut columns = BTreeMap::new();
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
        let origin = slots[0].col;
        if !matches!(origin, -1 | 0)
            || slots
                .iter()
                .enumerate()
                .any(|(index, slot)| slot.col != origin + index as i8)
        {
            return Err(
                "JSONC export needs consecutive physical columns starting at -1 or 0".into(),
            );
        }
        origins.push(Json::Number(origin as f64));
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
                .map(|slot| token(&slot.binding))
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

    let mut thumbs = vec![string(""), string("")];
    let mut hands = [false; 2];
    for slot in layout.slots.iter().filter(|slot| !slot.main) {
        let hand = usize::try_from(slot.hand).map_err(|_| "invalid thumb hand")?;
        if hand > 1 || hands[hand] {
            return Err("JSONC export supports at most one thumb key per hand".into());
        }
        if slot.row_offset != 0
            || slot.column_offset != 0
            || slot.finger != 8 + hand
            || slot.rank != -1
        {
            return Err("JSONC cannot represent custom thumb geometry".into());
        }
        hands[hand] = true;
        thumbs[hand] = string(token(&slot.binding)?);
    }
    let column_offsets: Vec<_> = columns.values().copied().collect();
    let has_columns = column_offsets.iter().any(|offset| *offset != 0);
    let has_rows = row_offsets.iter().any(|offset| *offset != 0);
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
    let mut native = BTreeMap::new();
    native.insert("version".into(), Json::Number(1.0));
    if origins
        .iter()
        .any(|origin| matches!(origin, Json::Number(value) if *value != 0.0))
    {
        native.insert("columnOrigins".into(), Json::Array(origins));
    }
    if has_rows && has_columns {
        native.insert("rowOffsets".into(), offsets(row_offsets));
    }
    let mut actions = BTreeMap::new();
    let mut needed = std::collections::BTreeSet::new();
    let mut pending: Vec<_> = layout
        .slots
        .iter()
        .filter_map(|slot| match &slot.binding {
            Binding::Named(name) => Some(name.clone()),
            _ => None,
        })
        .collect();
    while let Some(name) = pending.pop() {
        if !needed.insert(name.clone()) {
            continue;
        }
        if let Some(Action::Rules {
            rules, fallback, ..
        }) = layout.actions.get(&name)
        {
            for emission in rules.values().chain(std::iter::once(fallback)) {
                if let Emission::Call(called) = emission {
                    pending.push(called.clone());
                }
            }
        }
    }
    for (name, action) in layout
        .actions
        .iter()
        .filter(|(name, _)| needed.contains(*name))
    {
        // Unused built-ins are supplied by the DAT runtime already.
        let builtin = match name.as_str() {
            "repeat" | "repeat-output" => Some(Action::RepeatOutput),
            "repeat-action" | "again" => Some(Action::RepeatAction),
            _ => None,
        };
        if builtin.as_ref() == Some(action)
            && !layout
                .slots
                .iter()
                .any(|slot| matches!(&slot.binding, Binding::Named(bound) if bound == name))
        {
            continue;
        }
        validate_name(name)?;
        actions.insert(name.clone(), action_json(action)?);
    }
    let mut labels = BTreeMap::new();
    for slot in &layout.slots {
        if let Binding::Named(name) = &slot.binding {
            match labels.insert(name.clone(), string(&slot.label)) {
                Some(Json::String(previous)) if previous != slot.label => {
                    return Err(format!("JSONC cannot represent different display labels for the same action @{name}"));
                }
                _ => {}
            }
        }
    }
    if !actions.is_empty() {
        native.insert("actions".into(), Json::Object(actions));
    }
    if !labels.is_empty() {
        native.insert("labels".into(), Json::Object(labels));
    }
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
    root.insert(
        "magic".into(),
        object([
            ("magicKeys", Json::Null),
            ("rules", Json::Array(Vec::new())),
        ]),
    );
    if native.len() > 1 {
        root.insert("akler".into(), Json::Object(native));
    }

    if let Some(text) = simple_jsonc(&root, &layout, &needed) {
        return Ok(text);
    }
    Ok(format!("{}\n", json_text(&Json::Object(root), 0, "")))
}

fn expect_object<'a>(value: &'a Json, field: &str) -> Result<&'a BTreeMap<String, Json>> {
    match value {
        Json::Object(value) => Ok(value),
        _ => Err(format!("{field}: expected an object")),
    }
}

fn expect_string<'a>(value: &'a Json, field: &str) -> Result<&'a str> {
    match value {
        Json::String(value) => Ok(value),
        _ => Err(format!("{field}: expected a string")),
    }
}

fn field<'a>(map: &'a BTreeMap<String, Json>, name: &str) -> Result<&'a Json> {
    map.get(name)
        .ok_or_else(|| format!("missing native action field {name:?}"))
}

fn allowed(map: &BTreeMap<String, Json>, names: &[&str], context: &str) -> Result<()> {
    if let Some(name) = map.keys().find(|name| !names.contains(&name.as_str())) {
        return Err(format!("{context}: unsupported field {name:?}"));
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name != name.to_ascii_lowercase()
        || (name.starts_with('@') && name != "@")
        || name
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace() || matches!(ch, '=' | '"'))
    {
        return Err(format!(
            "native action name {name:?} cannot be represented in DAT"
        ));
    }
    Ok(())
}

fn native_emission(value: &Json) -> Result<String> {
    if matches!(value, Json::Null) {
        return Ok("none".into());
    }
    let value = expect_object(value, "native action emission")?;
    allowed(value, &["text", "call"], "native action emission")?;
    if value.len() != 1 {
        return Err("native action emission needs exactly one of text or call".into());
    }
    if let Some(text) = value.get("text") {
        return Ok(json_quote(expect_string(text, "emission.text")?));
    }
    let name = expect_string(field(value, "call")?, "emission.call")?;
    validate_name(name)?;
    Ok(format!("@{name}"))
}

/// Translate only the nonstandard action definitions. The JSON importer still
/// owns all visible physical slots and geometry and validates the resulting DAT.
pub(crate) fn native_action_dat(
    root: &BTreeMap<String, Json>,
) -> Result<(String, BTreeMap<String, String>)> {
    if root.contains_key("akler") && root.contains_key("layouter") {
        return Err("layout cannot contain both akler and layouter extensions".into());
    }
    let Some(native) = root.get("akler").or_else(|| root.get("layouter")) else {
        return Ok((String::new(), BTreeMap::new()));
    };
    let native = expect_object(native, "akler")?;
    allowed(
        native,
        &[
            "version",
            "actions",
            "labels",
            "rowOffsets",
            "columnOrigins",
        ],
        "akler",
    )?;
    if !matches!(native.get("version"), Some(Json::Number(1.0))) {
        return Err("akler.version must be 1".into());
    }
    let mut dat = String::new();
    let mut names = std::collections::BTreeSet::new();
    if let Some(actions) = native.get("actions") {
        for (name, value) in expect_object(actions, "akler.actions")? {
            validate_name(name)?;
            names.insert(name.clone());
            let value = expect_object(value, "native action")?;
            let kind = expect_string(field(value, "kind")?, "action.kind")?;
            match kind {
                "text" => {
                    allowed(value, &["kind", "text"], "text action")?;
                    let text = expect_string(field(value, "text")?, "action.text")?;
                    dat.push_str(&format!("action @{name} = text {}\n", json_quote(text)));
                }
                "repeat-output" | "repeat-action" | "inactive" => {
                    allowed(value, &["kind"], "native action")?;
                    dat.push_str(&format!("action @{name} = {kind}\n"));
                }
                "rules" => {
                    allowed(
                        value,
                        &["kind", "basis", "rules", "fallback"],
                        "rules action",
                    )?;
                    let basis = expect_string(field(value, "basis")?, "action.basis")?;
                    let dat_basis = match basis {
                        "text" => "magic",
                        "press" => "press-magic",
                        "skip-press" => "skip-magic",
                        "output" => "output-magic",
                        "skip-output" => "skip-output-magic",
                        "remembered" => "alternate",
                        _ => return Err(format!("unknown native action basis {basis:?}")),
                    };
                    dat.push_str(&format!("action @{name} = {dat_basis}\n"));
                    for (context, emission) in
                        expect_object(field(value, "rules")?, "action.rules")?
                    {
                        if context.is_empty() {
                            return Err("native action rule contexts must not be empty".into());
                        }
                        dat.push_str(&format!(
                            "map @{name} {} = {}\n",
                            json_quote(context),
                            native_emission(emission)?
                        ));
                    }
                    dat.push_str(&format!(
                        "fallback @{name} = {}\n",
                        native_emission(field(value, "fallback")?)?
                    ));
                }
                _ => return Err(format!("unknown native action kind {kind:?}")),
            }
        }
    }
    let mut labels = BTreeMap::new();
    if let Some(value) = native.get("labels") {
        for (name, value) in expect_object(value, "akler.labels")? {
            if !names.contains(name) {
                return Err(format!("native label refers to undefined action @{name}"));
            }
            let label = expect_string(value, "native action label")?;
            if label.is_empty() || label.chars().any(char::is_control) {
                return Err("native action labels must be nonempty printable strings".into());
            }
            labels.insert(name.clone(), label.into());
        }
    }
    if let Some(value) = native.get("rowOffsets") {
        let Json::Array(values) = value else {
            return Err("akler.rowOffsets must contain three numeric values".into());
        };
        if values.len() != 3 {
            return Err("akler.rowOffsets must contain three numeric values".into());
        }
        let mut numbers = Vec::new();
        for value in values {
            let Json::Number(value) = value else {
                return Err("akler.rowOffsets must contain three numeric values".into());
            };
            numbers.push(value.to_string());
        }
        if let Some(Json::Object(board)) = root.get("board") {
            if matches!(board.get("isRowStaggered"), Some(Json::Bool(true))) {
                return Err(
                    "akler.rowOffsets is only for layouts whose board specifies column stagger"
                        .into(),
                );
            }
        }
        dat.push_str(&format!("row-offsets: {}\n", numbers.join(" ")));
    }
    Ok((dat, labels))
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
        assert!(!text.contains("\"akler\""));

        let fields = ["layout", "fingermap", "board", "layers", "magic"];
        let positions = fields.map(|field| text.find(&format!("    \"{field}\":")).unwrap());
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
        let restored = Layout::parse(&text, Path::new("round")).unwrap();
        assert_eq!(restored.slots, layout.slots);
        assert_eq!(restored.actions, layout.actions);
        assert_eq!(jsonc_text(&restored).unwrap(), text);
    }

    #[test]
    fn simple_magic_and_adaptive_rules_keep_exact_definitions_after_swaps() {
        for marker in ["@", "*", "◇"] {
            let rows = GRID.replace("q w e", &format!("q w {marker}"));
            let source = format!(
                "{rows}thumbs: space e\n{marker} hr a' zz\nswap j nu y ,u\nrow-offsets: 0 0.25 0.75\n"
            );
            let mut layout = Layout::parse(&source, Path::new("inline")).unwrap();
            for moved in [false, true] {
                if moved {
                    layout.swap(2, 19);
                    layout.swap(5, 25);
                }
                let text = jsonc_text(&layout).unwrap();
                assert!(!text.contains("\"akler\""), "{text}");
                assert!(!text.contains("\"actions\""));
                assert!(text.contains(&format!("\"inputs\": \"h{marker}\"")));
                assert!(text.contains("\"output\": \"hr\""));
                assert!(text.contains("\"inputs\": \"y,\""));
                assert!(text.contains(&format!("\"inputs\": \"z{marker}\"")));
                // Never synthesize explicit repeat rules from the implicit fallback.
                assert!(!text.contains(&format!("\"inputs\": \"q{marker}\"")));

                let restored = Layout::parse(&text, Path::new("round")).unwrap();
                assert_eq!(restored.slots, layout.slots);
                assert_eq!(restored.actions, layout.actions);
                let dat = Layout::parse(&layout.text(), Path::new("round.dat")).unwrap();
                assert_eq!(restored.slots, dat.slots);
                assert_eq!(restored.actions, dat.actions);
            }
        }
    }

    #[test]
    fn simple_rule_round_trip_preserves_physical_mapping_and_metric_bits() {
        use crate::{action_ngrams, action_ui, AtomicBool, AtomicU64, Weights};

        let source = format!(
            "{}thumbs: space e\n@ hr a' zz\nswap j nu y ,u\n",
            GRID.replace("q w e", "q w @"),
        );
        let layout = Layout::parse(&source, Path::new("inline")).unwrap();
        let text = jsonc_text(&layout).unwrap();
        assert!(!text.contains("\"akler\""));
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
    fn simple_export_does_not_lose_advanced_rules_or_case() {
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
            let text = jsonc_text(&layout).unwrap();
            assert!(text.contains("\"akler\""), "{text}");
            let restored = Layout::parse(&text, Path::new("round")).unwrap();
            assert_eq!(restored.slots, layout.slots);
            assert_eq!(restored.actions, layout.actions);
        }
    }

    #[test]
    fn literal_and_adaptive_keys_with_same_label_are_not_merged() {
        let source = format!("{}adaptive n hr\n", GRID.replace("q w e", "n w e"),);
        let mut layout = Layout::parse(&source, Path::new("inline")).unwrap();
        layout.slots[0].binding = Binding::Text(b"n".to_vec());
        let text = jsonc_text(&layout).unwrap();
        assert!(text.contains("\"akler\""));
        let restored = Layout::parse(&text, Path::new("round")).unwrap();
        assert_eq!(restored.slots, layout.slots);
        assert_eq!(restored.actions, layout.actions);
    }

    #[test]
    fn native_rules_calls_labels_and_transformed_bindings_round_trip() {
        let source = format!("{}action ◇ = magic\nmap ◇ \"q\" = \"u\"\nmap ◇ \"x\" = none\naction a = magic\nmap a \"i\" = \"o\"\nfallback a = \"a\"\naction press = press-magic\nmap press \"q\" = @macro\nfallback press = none\naction macro = text \"the\"\naction again-key = repeat-action\n", GRID.replace("q w e r t", "@◇ @a @press @again-key t").replace("a s d f g", "q s d f g"));
        let mut original = Layout::parse(&source, Path::new("source")).unwrap();
        original.swap(0, 10);
        original.swap(1, 20);
        let dat = original.text();
        let (original, restored) = round_trip(&dat);
        assert!(matches!(
            restored.actions.get("press"),
            Some(Action::Rules {
                basis: Basis::Press,
                fallback: Emission::None,
                ..
            })
        ));
        assert!(
            matches!(restored.actions.get("a"), Some(Action::Rules { fallback: Emission::Text(text), .. }) if text == b"a")
        );
        assert_eq!(
            original
                .slots
                .iter()
                .filter(|slot| slot.label == "◇")
                .count(),
            1
        );
    }

    #[test]
    fn both_stagger_axes_custom_fingers_and_legacy_columns_round_trip() {
        let source = "q w e r t y | u i o p [ ]\na s d f g h | j k l ; ' /\nz x c v b n | m , . = - \\\nrow-offsets: -0.25 0 0.5\ncolumn-offsets: 0 0 -0.2 -0.3 -0.1 0 0 -0.1 -0.3 -0.2 0 0\nfingermap: LP LP LR LM LI LI / LP LP LR LM LI LI / LP LP LR LM LI LI\n";
        // Supply complete custom finger rows separately to keep the geometry explicit.
        let source = source.replace("LP LP LR LM LI LI / LP LP LR LM LI LI / LP LP LR LM LI LI", "LP LP LR LM LI LI RI RI RM RR RP RP / LP LP LR LM LI LI RI RI RM RR RP RP / LP LP LR LM LI LI RI RI RM RR RP RP");
        let (original, restored) = round_trip(&source);
        assert_eq!(original.slots[0].col, -1);
        assert!(restored.slots.iter().any(|slot| slot.column_offset == -300));
        assert!(restored.slots.iter().any(|slot| slot.row_offset == 500));
    }

    #[test]
    fn visible_native_layout_positions_remain_editable() {
        let source = format!("{}action m = repeat-output\n", GRID.replace("q w", "@m w"));
        let original = Layout::parse(&source, Path::new("source")).unwrap();
        let jsonc = jsonc_text(&original).unwrap().replace("@m w e", "w @m e");
        let restored = Layout::parse(&jsonc, Path::new("edited")).unwrap();
        assert_eq!(restored.slots[0].binding, Binding::Text(vec![b'w']));
        assert_eq!(restored.slots[1].binding, Binding::Named("m".into()));
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
