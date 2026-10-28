//! Lossless JSONC layout exports and coordinated DAT/JSONC saves.
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::action_keys::{Action, Basis, Binding, Emission, Layout, Result};
use crate::Json;

/// Save optimizer output separately from editable source layouts. Claim a
/// fresh name without overwriting a layout or its existing run record.
pub(crate) fn save_optimized(layout: &Layout) -> Result<PathBuf> {
    // The optimizer's spare keys are saved only when they hold something.
    let layout = &layout.without_spare_keys();
    let parent = layout
        .path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("layouts"));
    let dir = if parent.file_name().is_some_and(|name| name == "optimized") {
        parent.to_owned()
    } else {
        parent.join("optimized")
    };
    let stem = layout
        .path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("layout");
    save_numbered_jsonc(layout, &dir, &format!("{stem}-optimized"))
}

/// Save a JSONC copy beside the supplied path without overwriting existing files.
pub(crate) fn save_jsonc_copy(layout: &Layout) -> Result<PathBuf> {
    let dir = layout
        .path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("layouts"));
    let stem = layout
        .path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("layout");
    save_numbered_jsonc(layout, dir, stem)
}

fn save_numbered_jsonc(layout: &Layout, dir: &Path, stem: &str) -> Result<PathBuf> {
    let text = jsonc_text(layout)?;
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    for n in 1..10000 {
        let path = dir.join(format!("{stem}-{n:03}.jsonc"));
        if path.with_extension("run.txt").symlink_metadata().is_ok() {
            continue;
        }
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        };
        let result = file
            .write_all(text.as_bytes())
            .and_then(|_| crate::sync_file(&file));
        drop(file);
        if let Err(error) = result {
            let _ = fs::remove_file(&path);
            return Err(error.to_string());
        }
        return Ok(path);
    }
    Err("no unused result filename".into())
}

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
        crate::sync_file(&dat_file)?;
        crate::sync_file(&jsonc_file)
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
            crate::process_tag(),
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
            crate::copy_permissions(&file, metadata.permissions())?;
        }
        file.write_all(text.as_bytes())?;
        crate::sync_file(&file)
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
        crate::copy_permissions(&file, source.metadata()?.permissions())?;
        crate::sync_file(&file)
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
                    .map(|value| format!("{padding}{}", json_text(value, depth + 1, item_field)))
                    .collect();
                format!("[\n{}\n{}]", items.join(",\n"), "    ".repeat(depth))
            }
        }
        Json::Object(values) => {
            let order: &[&str] = match (depth, field) {
                (0, _) => &["layout", "fingermap", "board", "layers", "magic"],
                (_, "layout") => &["fingers", "thumbs"],
                (_, "board") => &[
                    "isRowStaggered",
                    "isColumnStaggered",
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
                        "{padding}{}: {}",
                        json_quote(name),
                        json_text(value, depth + 1, name)
                    )
                })
                .collect();
            format!("{{\n{}\n{}}}", lines.join(",\n"), "    ".repeat(depth))
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

fn wildcard_marker(layout: &Layout) -> Result<char> {
    let mut used = [false; 128];
    used[b'_' as usize] = true;
    let mut mark = |bytes: &[u8]| {
        for &byte in bytes {
            if byte.is_ascii_graphic() {
                used[byte as usize] = true;
            }
        }
    };
    let mark_emission = |emission: &Emission, mark: &mut dyn FnMut(&[u8])| {
        if let Emission::Text(text) = emission {
            mark(text);
        }
    };
    for slot in &layout.slots {
        mark(slot.label.as_bytes());
        if let Binding::Text(text) = &slot.binding {
            mark(text);
        }
    }
    for action in layout.actions.values() {
        match action {
            Action::Text(text) => mark(text),
            Action::Rules {
                rules, fallback, ..
            } => {
                for (context, emission) in rules {
                    mark(context);
                    mark_emission(emission, &mut mark);
                }
                mark_emission(fallback, &mut mark);
            }
            _ => {}
        }
    }
    drop(mark);
    b"*%#~^+?!&"
        .iter()
        .copied()
        .chain(33..=126)
        .find(|byte| !used[*byte as usize])
        .map(char::from)
        .ok_or_else(|| "JSONC needs one unused printable ASCII wildcard marker".into())
}

fn action_blocks(layout: &Layout) -> Result<(BTreeMap<String, String>, Option<Json>)> {
    let mut labels = BTreeMap::new();
    let mut label_bindings = BTreeMap::new();
    for slot in &layout.slots {
        if let Some(previous) = label_bindings.insert(slot.label.clone(), &slot.binding) {
            if previous != &slot.binding {
                return Err(format!(
                    "JSONC cannot represent different bindings for physical label {:?}",
                    slot.label
                ));
            }
        }
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
    // Adaptive keys use their normalized literal label as the fallback.
    // Multiple physical slots may share one adaptive binding; ordinary
    // magic.rules restores each slot while keeping distinct glyphs separate.
    let adaptive = !labels.is_empty()
        && labels.iter().all(|(name, label)| {
            !crate::layout_io::dedicated_magic_label(label)
                && matches!(layout.actions.get(name), Some(Action::Rules {
                basis: Basis::Text,
                fallback: Emission::Text(text),
                ..
            }) if text == label.to_ascii_lowercase().as_bytes())
        });
    let wildcard = if adaptive || labels.is_empty() {
        '*'
    } else {
        wildcard_marker(layout)?
    };

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
                if !adaptive
                    && !matches!(fallback, Emission::Text(text) if text.len() == 1 && text[0].is_ascii_graphic())
                    && !matches!(
                        fallback,
                        Emission::Call(fallback_name)
                            if matches!(layout.actions.get(fallback_name), Some(Action::RepeatOutput))
                    )
                {
                    return Err(format!(
                        "JSONC wildcard magic key {label:?} requires a repeat-output or one-character literal fallback"
                    ));
                }
                if !adaptive {
                    wildcard_rules.push(object([
                        ("inputs", string(format!("{wildcard}{label}"))),
                        (
                            "output",
                            string(match fallback {
                                Emission::Text(text) => {
                                    format!("{wildcard}{}", printable_byte(text, "magic fallback")?)
                                }
                                _ => format!("{wildcard}{wildcard}"),
                            }),
                        ),
                    ]));
                }
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
                    if context.as_bytes() == [wildcard as u8] {
                        return Err(format!(
                            "JSONC magic context {wildcard:?} is reserved as the wildcard marker"
                        ));
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
                    ("inputs", string(format!("{wildcard}_{label}"))),
                    ("output", string(format!("{wildcard}_{wildcard}"))),
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
                    if context.as_bytes() == [wildcard as u8] {
                        return Err(format!(
                            "JSONC skip context {wildcard:?} is reserved as the wildcard marker"
                        ));
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
    let magic = if adaptive {
        object([("rules", Json::Array(wildcard_rules))])
    } else {
        object([
            ("keys", string(keys)),
            ("wildcards", string(wildcard.to_string())),
            ("rules", Json::Array(wildcard_rules)),
        ])
    };
    Ok((labels, Some(magic)))
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

/// `layout` with the actions of keys labelled `@` and `$` named `magic` and
/// `skip`, as JSONC reimports them. Some JSONC files load such a key's action
/// under its label instead; names are internal, so typing is unchanged.
fn canonical_action_names(layout: &Layout) -> Layout {
    let mut layout = layout.clone();
    for (label, expected) in [("@", "magic"), ("$", "skip")] {
        let Some(name) = layout.slots.iter().find_map(|slot| match &slot.binding {
            Binding::Named(name) if slot.label == label && name != expected => Some(name.clone()),
            _ => None,
        }) else {
            continue;
        };
        if layout.actions.contains_key(expected) {
            continue;
        }
        let Some(action) = layout.actions.remove(&name) else {
            continue;
        };
        layout.actions.insert(expected.to_string(), action);
        for slot in &mut layout.slots {
            if slot.binding == Binding::Named(name.clone()) {
                slot.binding = Binding::Named(expected.to_string());
            }
        }
        for action in layout.actions.values_mut() {
            if let Action::Rules {
                rules, fallback, ..
            } = action
            {
                for emission in rules.values_mut().chain([fallback]) {
                    if *emission == Emission::Call(name.clone()) {
                        *emission = Emission::Call(expected.to_string());
                    }
                }
            }
        }
    }
    layout
}

/// Export the subset that the readable Mana-style JSONC fields preserve exactly.
pub(crate) fn jsonc_text(layout: &Layout) -> Result<String> {
    let mut layout = canonical_action_names(layout);
    // JSONC rows start at column 0; a DAT outer column sits at -1.
    let origins: BTreeSet<_> = (0..3)
        .filter_map(|row| {
            layout
                .slots
                .iter()
                .filter(|slot| slot.main && slot.row == row)
                .map(|slot| slot.col)
                .min()
        })
        .collect();
    if let [origin] = origins.iter().copied().collect::<Vec<_>>()[..] {
        for slot in layout.slots.iter_mut().filter(|slot| slot.main) {
            slot.col -= origin;
        }
    }
    let layout = &layout;
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
        if !(10..=13).contains(&slots.len()) {
            return Err(format!(
                "JSONC export needs 10 through 13 physical slots in row {}",
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
    let has_space = layout
        .slots
        .iter()
        .any(|slot| matches!(&slot.binding, Binding::Text(text) if text.as_slice() == b" "));
    let thumbs = thumbs
        .into_iter()
        .map(|group| {
            if group.is_empty() && !has_space {
                string("none")
            } else {
                string(group.join(" "))
            }
        })
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
    let mut board = object([
        ("isRowStaggered", Json::Bool(!has_columns && has_rows)),
        ("isColumnStaggered", Json::Bool(has_columns)),
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
    let thumb_columns: Vec<_> = layout
        .slots
        .iter()
        .filter(|slot| !slot.main)
        .map(|slot| slot.col)
        .collect();
    let default_columns: Vec<_> = layout
        .slots
        .iter()
        .filter(|slot| !slot.main)
        .enumerate()
        .map(|(index, slot)| {
            if index == 0 && slot.hand == 1 {
                1
            } else {
                index as i8
            }
        })
        .collect();
    if thumb_columns != default_columns {
        if let Json::Object(fields) = &mut board {
            fields.insert(
                "thumbColumns".into(),
                Json::Array(
                    thumb_columns
                        .iter()
                        .map(|&col| Json::Number(f64::from(col)))
                        .collect(),
                ),
            );
        }
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
    if !layout.alt_fingers.is_empty() {
        root.insert(
            "alts".into(),
            crate::alt_fingers::to_json(&layout.alt_fingers),
        );
    }
    if !layout.combos.is_empty() {
        root.insert("combos".into(), Json::Array(layout.combos.clone()));
    }
    if let Some(magic) = magic {
        root.insert("magic".into(), magic);
    }
    let text = format!("{}\n", json_text(&Json::Object(root), 0, ""));
    let restored = crate::layout_io::parse_json_layout(&text, &layout.path)
        .map_err(|error| format!("JSONC export failed its reimport check: {error}"))?;
    if restored.slots != layout.slots
        || restored.alt_fingers != layout.alt_fingers
        || crate::alt_fingers::compact_json(&Json::Array(restored.combos.clone()))
            != crate::alt_fingers::compact_json(&Json::Array(layout.combos.clone()))
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
    fn magic_key_actions_named_after_their_label_still_export() {
        // DAT and some JSONC files load an `@` magic key's action as @@,
        // while JSONC reimports it as @magic.
        let layout = Layout::parse(
            "q w e r t | y u i o p\na s d f g | h j k l @\nz x c v b | n m , . /\ne@ eo\n",
            Path::new("magic.dat"),
        )
        .unwrap();
        let action = layout.actions["@"].clone();
        let slot = layout
            .slots
            .iter()
            .position(|slot| slot.label == "@")
            .unwrap();
        assert_eq!(layout.slots[slot].binding, Binding::Named("@".into()));
        let jsonc = jsonc_text(&layout).unwrap();
        let restored = Layout::parse(&jsonc, Path::new("restored.jsonc")).unwrap();
        assert_eq!(restored.slots[slot].binding, Binding::Named("magic".into()));
        assert_eq!(restored.actions["magic"], action);
    }

    #[test]
    fn ordinary_jsonc_round_trip_keeps_single_thumb_and_literal_punctuation() {
        let source = GRID.replace("q w e r t", "char:* char:~ char:| char:@ char:Q");
        let (original, restored) = round_trip(&source);
        assert_eq!(restored.slots.iter().filter(|slot| !slot.main).count(), 1);
        assert!(!original.extended());
        assert!(!restored.extended());
        for thumbs in ["none | none", "none | r", "r | none"] {
            let (_, restored) = round_trip(&format!("{GRID}thumbs: {thumbs}\n"));
            assert!(!restored.slots.iter().any(|slot| {
                matches!(&slot.binding, Binding::Text(text) if text.as_slice() == b" ")
            }));
        }
    }

    #[test]
    fn edited_lead_saves_chiral_actions_and_their_geometry() {
        let mut layout = Layout::parse(
            r##"{
                "layout": {
                    "fingers": [
                        "\\ y l d m v q f u o Y [",
                        "/ n r t s g k b e a i '",
                        "x j z w c skip p h , . ; skip"
                    ],
                    "thumbs": ["", ""]
                },
                "fingermap": [
                    "0 0 1 2 2 3 6 6 7 8 9 9",
                    "0 0 1 2 3 3 6 6 7 8 9 9",
                    "0 1 2 3 3 0 6 6 7 8 9 9"
                ],
                "board": {
                    "isRowStaggered": true,
                    "rowOrColumnStagger": [0, 0.25, 0.75]
                },
                "magic": {"rules": [
                    {"inputs": "hy", "output": "hy"},
                    {"inputs": "hY", "output": "h#"},
                    {"inputs": "dy", "output": "d#"},
                    {"inputs": "dY", "output": "dy"}
                ]}
            }"##,
            Path::new("lead.jsonc"),
        )
        .unwrap();
        let key = |label: &str| {
            layout
                .slots
                .iter()
                .position(|slot| slot.label == label)
                .unwrap()
        };
        let (y, right_y, h, b, d) = (key("y"), key("Y"), key("h"), key("b"), key("d"));
        assert_eq!(layout.slots[y].binding, Binding::Named("y".into()));
        assert_eq!(layout.slots[right_y].binding, Binding::Named("Y".into()));
        let output = |layout: &Layout, presses: &[usize]| {
            crate::action_keys::trace_keys(layout, presses)
                .unwrap()
                .into_iter()
                .flat_map(|step| step.output)
                .collect::<Vec<_>>()
        };
        assert_eq!(output(&layout, &[h, y]), b"hy");
        assert_eq!(output(&layout, &[h, right_y]), b"h#");
        assert_eq!(output(&layout, &[d, y]), b"d#");
        assert_eq!(output(&layout, &[d, right_y]), b"dy");
        // Both physical actions emit normalized text in unlisted contexts.
        assert_eq!(output(&layout, &[y]), b"y");
        assert_eq!(output(&layout, &[right_y]), b"y");
        layout.swap(h, b);
        let directory = test_directory();
        layout.path = directory.join("lead.jsonc");
        let saved = layout.save_new().unwrap();
        for path in [saved.clone(), saved.with_extension("jsonc")] {
            let restored = Layout::parse(&fs::read_to_string(&path).unwrap(), &path).unwrap();
            assert_eq!(restored.slots, layout.slots);
            assert_eq!(restored.actions, layout.actions);
            for previous in 0..layout.slots.len() {
                if matches!(layout.slots[previous].binding, Binding::Empty) {
                    continue;
                }
                for action in [y, right_y] {
                    let presses = [previous, action];
                    assert_eq!(output(&restored, &presses), output(&layout, &presses));
                }
            }
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn column_stagger_export_declares_its_mode_and_keeps_physical_offsets() {
        let source = format!("{GRID}column-offsets: 0 -0.3 -0.4 -0.3 -0.2 -0.2 -0.3 -0.4 -0.3 0\n");
        let original = Layout::parse(&source, Path::new("columns.dat")).unwrap();
        let jsonc = jsonc_text(&original).unwrap();
        assert!(jsonc.contains("\"isRowStaggered\": false"), "{jsonc}");
        assert!(jsonc.contains("\"isColumnStaggered\": true"), "{jsonc}");
        let restored = Layout::parse(&jsonc, Path::new("columns.jsonc")).unwrap();
        assert_eq!(restored.slots, original.slots);
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
        for row in ["q w e r t y u i o p", " a s d f g h j k l ;"] {
            assert!(text.contains(&format!("            \"{row}\",\n")));
        }
        assert!(text.contains("            \"  z x c v b n m , . /\"\n        ]"));
        assert!(text.contains("        \"thumbs\": [\"space\", \"\"]\n    }"));
        assert!(text.contains("    \"fingermap\": [\n        \"0 1 2 3 3 6 6 7 8 9\",\n"));
        assert!(text.contains("        \"rowOrColumnStagger\": [0, 0.25, 0.75]\n    }"));
        crate::parse_json(&text).unwrap();

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

        let mut chog = Layout::parse(
            r#"{
                "layout": {
                    "fingers": [
                        "q w e r t y u i o p",
                        "a s d f g h j k l ;",
                        "z x c v b n m , . /"
                    ],
                    "thumbs": ["space", "*"]
                },
                "magic": {
                    "magicKeys": [],
                    "rules": [{"inputs": "a*", "output": "aa"}]
                }
            }"#,
            Path::new("chog.jsonc"),
        )
        .unwrap();
        let key = |layout: &Layout, label: &str| {
            layout
                .slots
                .iter()
                .position(|slot| slot.label == label)
                .unwrap()
        };
        let (q, w) = (key(&chog, "q"), key(&chog, "w"));
        chog.swap(q, w);
        let jsonc = jsonc_text(&chog).unwrap();
        assert!(jsonc.contains(r#""wildcards": "%""#), "{jsonc}");
        assert!(jsonc.contains(r#""inputs": "%*""#), "{jsonc}");
        assert!(jsonc.contains(r#""output": "%%""#), "{jsonc}");

        let directory = test_directory();
        let dat_path = directory.join("chog.dat");
        save_pair(&dat_path, &chog.text(), &jsonc).unwrap();
        for path in [dat_path.clone(), dat_path.with_extension("jsonc")] {
            let restored = Layout::parse(&fs::read_to_string(&path).unwrap(), &path).unwrap();
            assert_eq!(restored.slots, chog.slots);
            assert_eq!(restored.actions, chog.actions);
            assert_eq!(restored.text(), chog.text());
            let presses = [key(&restored, "a"), key(&restored, "*")];
            let output = crate::action_keys::trace_keys(&restored, &presses)
                .unwrap()
                .into_iter()
                .flat_map(|step| step.output)
                .collect::<Vec<_>>();
            assert_eq!(output, b"aa");
        }
        fs::remove_dir_all(directory).unwrap();
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
            .contains("different bindings for physical label"));
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
        assert!(
            jsonc.contains("\"thumbs\": [\"space\", \"l r\"]"),
            "{jsonc}"
        );
        assert!(
            jsonc.contains("\"keys\": \"#$\"") || jsonc.contains("\"keys\": \"$#\""),
            "{jsonc}"
        );
        assert!(jsonc.contains("\"wildcards\": \"*\""), "{jsonc}");
        assert!(jsonc.contains("\"inputs\": \"*#\""), "{jsonc}");
        assert!(jsonc.contains("\"output\": \"**\""), "{jsonc}");
        assert!(jsonc.contains("\"inputs\": \"a#\""), "{jsonc}");
        assert!(
            jsonc.find("\"inputs\": \"*#\"").unwrap() < jsonc.find("\"inputs\": \"a#\"").unwrap(),
            "{jsonc}"
        );
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
    fn literal_magic_defaults_round_trip_and_multichar_defaults_fail() {
        let mut layout = Layout::parse(GRID, Path::new("inline.dat")).unwrap();
        layout.slots[0].binding = Binding::Named("◇".into());
        layout.slots[0].label = "◇".into();
        layout.actions.insert(
            "◇".into(),
            Action::Rules {
                basis: Basis::Text,
                rules: BTreeMap::new(),
                fallback: Emission::Text(b"x".to_vec()),
            },
        );
        let jsonc = jsonc_text(&layout).unwrap();
        let restored = Layout::parse(&jsonc, Path::new("inline.jsonc")).unwrap();
        assert_eq!(restored.slots, layout.slots);
        assert_eq!(restored.actions, layout.actions);
        if let Action::Rules { fallback, .. } = layout.actions.get_mut("◇").unwrap() {
            *fallback = Emission::Text(b"xy".to_vec());
        }
        assert!(jsonc_text(&layout)
            .unwrap_err()
            .contains("one-character literal fallback"));
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
            crate::process_tag(),
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

        let mut layout = Layout::parse(GRID, Path::new("source.dat")).unwrap();
        layout.path = directory.join("source.dat");
        let first = save_optimized(&layout).unwrap();
        let first_text = fs::read_to_string(&first).unwrap();
        assert_eq!(first.parent(), Some(directory.join("optimized").as_path()));
        assert_eq!(
            first.extension().and_then(|value| value.to_str()),
            Some("jsonc")
        );
        assert!(!first.with_extension("dat").exists());
        crate::parse_json(&first_text).unwrap();
        let restored = Layout::parse(&first_text, &first).unwrap();
        assert_eq!(restored.slots, layout.slots);
        assert_eq!(restored.actions, layout.actions);

        let second = save_optimized(&layout).unwrap();
        assert_ne!(second, first);
        assert_eq!(fs::read_to_string(&first).unwrap(), first_text);
        let second_text = fs::read_to_string(&second).unwrap();
        crate::parse_json(&second_text).unwrap();
        let restored = Layout::parse(&second_text, &second).unwrap();
        assert_eq!(restored.slots, layout.slots);
        assert_eq!(restored.actions, layout.actions);
        assert!(!second.with_extension("dat").exists());

        layout.path = first;
        let third = save_optimized(&layout).unwrap();
        assert_eq!(third.parent(), second.parent());
        assert!(!third.parent().unwrap().join("optimized").exists());
        let third_text = fs::read_to_string(&third).unwrap();
        crate::parse_json(&third_text).unwrap();
        let restored = Layout::parse(&third_text, &third).unwrap();
        assert_eq!(restored.slots, layout.slots);
        assert_eq!(restored.actions, layout.actions);
        assert!(!third.with_extension("dat").exists());
        fs::remove_dir_all(directory).unwrap();
    }
}
