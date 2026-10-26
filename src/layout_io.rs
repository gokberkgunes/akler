//! Content-based JSON/JSONC layout import. Runtime actions still use the DAT model.

use crate::action_keys::{self as ak, Layout};
use crate::{Json, JsonParser};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

fn without_comments(text: &str) -> ak::Result<String> {
    let mut bytes = text.trim_start_matches('\u{feff}').as_bytes().to_vec();
    let mut i = 0;
    let mut quoted = false;

    while i < bytes.len() {
        if quoted {
            if bytes[i] == b'\\' {
                i += 2;
                continue;
            }
            if bytes[i] == b'"' {
                quoted = false;
            }
            i += 1;
            continue;
        }
        if bytes[i] == b'"' {
            quoted = true;
            i += 1;
            continue;
        }
        if bytes.get(i..i + 2) == Some(b"//") {
            while i < bytes.len() && !matches!(bytes[i], b'\n' | b'\r') {
                bytes[i] = b' ';
                i += 1;
            }
            continue;
        }
        if bytes.get(i..i + 2) == Some(b"/*") {
            let start = i;
            bytes[i] = b' ';
            bytes[i + 1] = b' ';
            i += 2;

            while i + 1 < bytes.len() && &bytes[i..i + 2] != b"*/" {
                if !matches!(bytes[i], b'\n' | b'\r') {
                    bytes[i] = b' ';
                }
                i += 1;
            }
            if i + 1 == bytes.len() || i == bytes.len() {
                return Err(format!("unterminated JSONC block comment at byte {start}"));
            }
            bytes[i] = b' ';
            bytes[i + 1] = b' ';
            i += 2;
            continue;
        }
        i += 1;
    }

    String::from_utf8(bytes).map_err(|error| error.to_string())
}

/// Detect an object header, not the filename extension or a literal `{` DAT key.
pub(crate) fn is_json_layout(text: &str) -> bool {
    let Ok(clean) = without_comments(text) else {
        return text
            .trim_start_matches('\u{feff}')
            .trim_start()
            .starts_with('{');
    };
    let clean = clean.trim_start();
    let Some(rest) = clean.strip_prefix('{') else {
        return false;
    };
    let rest = rest.trim_start();
    if rest.is_empty() || rest.starts_with('}') {
        return true;
    }
    if !rest.starts_with('"') {
        return false;
    }
    let mut parser = JsonParser { text: rest, p: 0 };
    parser.string().is_ok() && rest[parser.p..].trim_start().starts_with(':')
}

/// JSON comments and trailing commas are accepted only outside quoted strings.
pub(crate) fn parse_jsonc(text: &str) -> ak::Result<Json> {
    let mut bytes = without_comments(text)?.into_bytes();
    let mut i = 0;
    let mut quoted = false;

    while i < bytes.len() {
        if quoted {
            if bytes[i] == b'\\' {
                i += 2;
                continue;
            }
            if bytes[i] == b'"' {
                quoted = false;
            }
        } else if bytes[i] == b'"' {
            quoted = true;
        } else if bytes[i] == b',' {
            let mut next = i + 1;
            while bytes.get(next).is_some_and(u8::is_ascii_whitespace) {
                next += 1;
            }
            let previous = bytes[..i].iter().rfind(|byte| !byte.is_ascii_whitespace());
            let follows_value =
                previous.is_some_and(|byte| !matches!(byte, b'[' | b'{' | b',' | b':'));
            if follows_value && matches!(bytes.get(next), Some(b']' | b'}')) {
                bytes[i] = b' ';
            }
        }
        i += 1;
    }

    let clean = String::from_utf8(bytes).map_err(|error| error.to_string())?;
    crate::parse_json(&clean).map_err(|error| error.to_string())
}

fn object<'a>(value: &'a Json, field: &str) -> ak::Result<&'a BTreeMap<String, Json>> {
    match value {
        Json::Object(value) => Ok(value),
        _ => Err(format!("{field}: expected an object")),
    }
}

fn array<'a>(value: &'a Json, field: &str) -> ak::Result<&'a [Json]> {
    match value {
        Json::Array(value) => Ok(value),
        _ => Err(format!("{field}: expected an array")),
    }
}

fn string<'a>(value: &'a Json, field: &str) -> ak::Result<&'a str> {
    match value {
        Json::String(value) => Ok(value),
        _ => Err(format!("{field}: expected a string")),
    }
}

fn required<'a>(map: &'a BTreeMap<String, Json>, field: &str) -> ak::Result<&'a Json> {
    map.get(field)
        .ok_or_else(|| format!("missing JSON layout field {field:?}"))
}

fn empty_feature(map: &BTreeMap<String, Json>, field: &str) -> ak::Result<()> {
    let empty = match map.get(field) {
        None | Some(Json::Null) => true,
        Some(Json::Array(values)) => values.is_empty(),
        Some(Json::Object(values)) => values.is_empty(),
        _ => false,
    };
    if empty {
        Ok(())
    } else {
        Err(format!(
            "JSON layout {field} is not supported; omit it or leave it empty"
        ))
    }
}

fn bool_field(map: &BTreeMap<String, Json>, field: &str, default: bool) -> ak::Result<bool> {
    match map.get(field) {
        None => Ok(default),
        Some(Json::Bool(value)) => Ok(*value),
        _ => Err(format!("board.{field}: expected true or false")),
    }
}

fn integer_field(map: &BTreeMap<String, Json>, field: &str, context: &str) -> ak::Result<usize> {
    match map.get(field) {
        Some(Json::Number(value)) if value.is_finite() && *value >= 0.0 && value.fract() == 0.0 => {
            Ok(*value as usize)
        }
        Some(_) => Err(format!("{context}.{field}: expected a nonnegative integer")),
        None => Err(format!("{context}: missing field {field:?}")),
    }
}

fn key_token(token: &str) -> ak::Result<String> {
    if let Some(value) = token.strip_prefix("char:") {
        if value.len() == 1 && value.as_bytes()[0].is_ascii_graphic() {
            return Ok(token.into());
        }
        return Err("char: requires one printable non-space ASCII character".into());
    }
    if token == "space" || (token == "␠" || token == "␣") {
        return Ok(" ".into());
    }
    if matches!(token, "skip" | "blank" | "~") {
        return Ok(String::new());
    }
    let mut chars = token.chars();
    let Some(ch) = chars.next() else {
        return Ok(String::new());
    };
    if chars.next().is_some() || ch.is_control() || ch.is_whitespace() {
        return Err(format!("JSON key {token:?}: expected one character, space, or skip; tap-holds and directional keys are not supported"));
    }
    // This is a physical label, not corpus text: y and Y can be separate
    // adaptive actions. Text emissions are normalized when imported below.
    Ok(ch.to_string())
}

pub(crate) fn dedicated_magic_label(label: &str) -> bool {
    matches!(label, "@" | "*") || (!label.is_ascii() && label.chars().count() == 1)
}

fn dat_key(label: &str, actions: &BTreeMap<String, String>) -> String {
    if let Some(name) = actions.get(label) {
        format!("@{name}")
    } else if label.is_empty() {
        "~".into()
    } else if label == " " {
        "space".into()
    } else if label == "|" {
        "char:|".into()
    } else {
        label.into()
    }
}

#[derive(Clone)]
enum RuleOutput {
    Text(Vec<u8>),
    CallKey(String),
}

struct ImportedRules {
    basis: &'static str,
    rules: BTreeMap<Vec<u8>, RuleOutput>,
    fallback: String,
}

fn physical_key(value: &Json, field: &str, labels: &BTreeSet<String>) -> ak::Result<String> {
    let key = key_token(string(value, field)?)?;
    if key.is_empty() || !labels.contains(&key) {
        return Err(format!("{field} refers to missing physical key {key:?}"));
    }
    Ok(key)
}

fn fallback_text(value: Option<&Json>, default: &str, field: &str) -> ak::Result<String> {
    let fallback = match value {
        None | Some(Json::Null) => return Ok(default.into()),
        Some(value) => string(value, field)?,
    };
    match fallback {
        "none" => Ok("none".into()),
        "repeat" | "repeat-output" | "repeat-previous-output" | "repeat-action" => {
            Ok(fallback.into())
        }
        "inactive" => Ok("@inactive".into()),
        text if text.len() == 1 && text.as_bytes()[0].is_ascii_graphic() => {
            Ok(ak::quote(text.as_bytes()))
        }
        _ => Err(format!(
            "{field}: expected none, a repeat action, inactive, or one printable ASCII character"
        )),
    }
}

fn import_wildcard_magic(
    magic: &BTreeMap<String, Json>,
    labels: &BTreeSet<String>,
    action_names: &mut BTreeMap<String, String>,
    action_defs: &mut BTreeMap<String, ImportedRules>,
) -> ak::Result<()> {
    let keys = string(required(magic, "keys")?, "magic.keys")?;
    let wildcard = string(required(magic, "wildcards")?, "magic.wildcards")?;
    if wildcard.len() != 1 || !wildcard.as_bytes()[0].is_ascii_graphic() {
        return Err("magic.wildcards must be one printable ASCII character".into());
    }
    let mut key_labels = BTreeSet::new();
    for key in keys.chars() {
        let label = key.to_string();
        if !labels.contains(&label) {
            return Err(format!(
                "magic.keys refers to missing physical key {label:?}"
            ));
        }
        if !key_labels.insert(label) {
            return Err(format!("magic.keys contains duplicate key {key:?}"));
        }
    }
    if key_labels.is_empty() {
        return Err("magic.keys must name at least one physical key".into());
    }
    if wildcard == "_" || key_labels.contains(wildcard) {
        return Err("magic.wildcards cannot be '_' or a magic key".into());
    }
    let rule_values = magic
        .get("rules")
        .filter(|value| !matches!(value, Json::Null));
    let rules = if let Some(value) = rule_values {
        array(value, "magic.rules")?
    } else {
        return Err("magic.rules must contain a wildcard rule for each magic key".into());
    };
    let mut definitions: BTreeMap<String, ImportedRules> = BTreeMap::new();
    let mut wildcard_positions = BTreeMap::new();
    let mut explicit = Vec::new();
    for (position, value) in rules.iter().enumerate() {
        let rule = object(value, "magic rule")?;
        let input = string(required(rule, "inputs")?, "magic rule inputs")?.to_string();
        let label = key_labels
            .iter()
            .find(|label| input.ends_with(label.as_str()))
            .ok_or_else(|| format!("magic rule {input:?} must end with a key from magic.keys"))?
            .clone();
        let prefix = input.strip_suffix(&label).unwrap();
        let text_wildcard = prefix == wildcard;
        let skip_wildcard = prefix == format!("{wildcard}_");
        if text_wildcard || skip_wildcard {
            if rule.contains_key("call") {
                return Err(format!("magic wildcard rule {input:?} needs output"));
            }
            let output = string(required(rule, "output")?, "magic wildcard output")?;
            let fallback = if text_wildcard {
                let emitted = output.strip_prefix(wildcard).ok_or_else(|| {
                    format!("magic wildcard rule {input:?} must preserve {wildcard:?}")
                })?;
                if emitted == wildcard {
                    "repeat-output".into()
                } else if emitted.len() == 1 && emitted.as_bytes()[0].is_ascii_graphic() {
                    ak::quote(&[emitted.as_bytes()[0].to_ascii_lowercase()])
                } else {
                    return Err(format!(
                        "magic wildcard rule {input:?} must repeat the wildcard or append one printable ASCII character"
                    ));
                }
            } else {
                let expected = format!("{wildcard}_{wildcard}");
                if output != expected {
                    return Err(format!(
                        "magic wildcard rule {input:?} must output {expected:?}"
                    ));
                }
                "repeat-previous-output".into()
            };
            definitions.insert(
                label.clone(),
                ImportedRules {
                    basis: if text_wildcard { "magic" } else { "skip-magic" },
                    rules: BTreeMap::new(),
                    fallback,
                },
            );
            wildcard_positions.insert(label, position);
        } else {
            explicit.push((position, label, input, rule));
        }
    }
    for label in &key_labels {
        if !definitions.contains_key(label) {
            return Err(format!("magic key {label:?} needs a wildcard rule"));
        }
    }
    for (position, label, input, rule) in explicit {
        if position < wildcard_positions[&label] {
            continue;
        }
        let definition = definitions.get_mut(&label).unwrap();
        let prefix = input.strip_suffix(&label).unwrap();
        let context = if definition.basis == "skip-magic" {
            let context = prefix.strip_suffix('_').ok_or_else(|| {
                format!("magic skip rule {input:?} must use '_' for its skipped key")
            })?;
            if context.len() != 1 || !context.as_bytes()[0].is_ascii_graphic() {
                return Err(format!(
                    "magic skip rule {input:?} needs one printable key before '_'"
                ));
            }
            if !labels.contains(context) {
                return Err(format!(
                    "magic skip rule {input:?} refers to missing physical key {context:?}"
                ));
            }
            context.to_string()
        } else {
            if prefix.is_empty() || !prefix.bytes().all(|byte| (32..=126).contains(&byte)) {
                return Err(format!(
                    "magic rule {input:?} needs printable text before its key"
                ));
            }
            prefix.to_ascii_lowercase()
        };
        let output = match (rule.get("output"), rule.get("call")) {
            (Some(value), None) => {
                let output = string(value, "magic rule output")?;
                let output = if definition.basis == "skip-magic" {
                    output.to_string()
                } else {
                    output.to_ascii_lowercase()
                };
                let expected = if definition.basis == "skip-magic" {
                    format!("{context}_")
                } else {
                    context.to_string()
                };
                let emitted = output.strip_prefix(&expected).ok_or_else(|| {
                    format!("magic rule {input:?}: output must preserve {expected:?}")
                })?;
                if emitted.len() != 1 || !emitted.as_bytes()[0].is_ascii_graphic() {
                    return Err(format!(
                        "magic rule {input:?} must append one printable ASCII character"
                    ));
                }
                RuleOutput::Text(emitted.to_ascii_lowercase().into_bytes())
            }
            (None, Some(value)) if definition.basis == "skip-magic" => {
                RuleOutput::CallKey(physical_key(value, "magic rule call", labels)?)
            }
            _ => {
                return Err(format!(
                    "magic rule {input:?} needs exactly one valid output or call"
                ))
            }
        };
        definition.rules.insert(context.as_bytes().to_vec(), output);
    }
    for (label, definition) in definitions {
        let name = if label == "@" {
            "magic".to_string()
        } else if label == "$" {
            "skip".to_string()
        } else {
            label.clone()
        };
        action_names.insert(label, name.clone());
        action_defs.insert(name, definition);
    }
    Ok(())
}

fn import_layout_with_actions(
    root: &BTreeMap<String, Json>,
    path: &Path,
    mut action_names: BTreeMap<String, String>,
    mut action_defs: BTreeMap<String, ImportedRules>,
    allow_unequal_rows: bool,
) -> ak::Result<Layout> {
    for field in root.keys() {
        if !matches!(
            field.as_str(),
            "layout" | "fingermap" | "board" | "layers" | "magic" | "skip" | "combos" | "alts"
        ) {
            return Err(format!("unsupported JSON layout field {field:?}"));
        }
    }
    empty_feature(root, "layers")?;

    let layout = object(required(root, "layout")?, "layout")?;
    empty_feature(layout, "combos")?;
    let fingers = array(required(layout, "fingers")?, "layout.fingers")?;
    if fingers.len() != 3 {
        return Err("layout.fingers must contain exactly three rows".into());
    }
    let mut rows = Vec::new();
    for (row, value) in fingers.iter().enumerate() {
        let tokens = string(value, "layout.fingers row")?
            .split_whitespace()
            .map(key_token)
            .collect::<ak::Result<Vec<_>>>()?;
        if !(10..=13).contains(&tokens.len()) {
            return Err(format!(
                "layout.fingers row {} must have 10 through 13 keys (use skip for empty slots)",
                row + 1
            ));
        }
        rows.push(tokens);
    }
    if !allow_unequal_rows && rows.iter().any(|row| row.len() != rows[0].len()) {
        return Err(format!(
            "layout.fingers rows have {}, {}, and {} physical slots; add skip so all rows have the same width",
            rows[0].len(), rows[1].len(), rows[2].len()
        ));
    }

    if rows.iter().any(|row| row.len() == 13) && !root.contains_key("fingermap") {
        return Err("13-column layouts require an explicit fingermap".into());
    }

    let mut thumbs = [Vec::new(), Vec::new()];
    if let Some(value) = layout.get("thumbs") {
        let values = array(value, "layout.thumbs")?;
        if values.len() > 2 {
            return Err("layout.thumbs supports at most two hand strings".into());
        }
        thumbs = [Vec::new(), Vec::new()];
        for (hand, value) in values.iter().enumerate() {
            let group = string(value, "layout.thumbs hand")?;
            let tokens: Vec<_> = group.split_whitespace().collect();
            if tokens == ["none"] {
                thumbs[hand].push("none".into());
            } else {
                if tokens.iter().any(|token| *token == "none") {
                    return Err("layout.thumbs none cannot be combined with a thumb key".into());
                }
                thumbs[hand] = tokens
                    .into_iter()
                    .map(key_token)
                    .collect::<ak::Result<Vec<_>>>()?;
            }
        }
    }
    let has_space = rows
        .iter()
        .flatten()
        .chain(thumbs.iter().flatten())
        .any(|label| label == " ");
    let explicit_absence = thumbs.iter().flatten().any(|label| label == "none");
    if !has_space && !explicit_absence {
        match (thumbs[0].is_empty(), thumbs[1].is_empty()) {
            (true, _) => thumbs[0].push(" ".into()),
            (false, true) => thumbs[1].push(" ".into()),
            (false, false) => {}
        }
    }

    let finger_names = ["LP", "LR", "LM", "LI", "LT", "RT", "RI", "RM", "RR", "RP"];
    let mut fingermap = Vec::new();
    if let Some(value) = root.get("fingermap") {
        let maps = array(value, "fingermap")?;
        if maps.len() != 3 {
            return Err("fingermap must contain exactly three rows".into());
        }
        for (row, value) in maps.iter().enumerate() {
            let tokens: Vec<_> = string(value, "fingermap row")?.split_whitespace().collect();
            if tokens.len() != rows[row].len() {
                return Err(format!(
                    "fingermap row {} must match its layout row width",
                    row + 1
                ));
            }
            let names = tokens.iter().map(|token| {
                let finger = token.parse::<usize>().map_err(|_| "fingermap must use Mana finger IDs 0 through 9".to_string())?;
                if matches!(finger, 4 | 5) {
                    return Err("thumb finger IDs 4 and 5 belong to layout.thumbs, not the main fingermap".into());
                }
                finger_names.get(finger).copied().ok_or_else(|| "fingermap must use Mana finger IDs 0 through 9".to_string())
            }).collect::<ak::Result<Vec<_>>>()?;
            fingermap.push(names.join(" "));
        }
    }

    let labels: BTreeSet<String> = rows
        .iter()
        .flatten()
        .chain(thumbs.iter().flatten())
        .cloned()
        .collect();
    if let Some(magic) = root
        .get("magic")
        .filter(|value| !matches!(value, Json::Null))
    {
        let magic = object(magic, "magic")?;
        empty_feature(magic, "magicKeys")?;
        empty_feature(magic, "combos")?;
        if magic.contains_key("wildcards") {
            if magic.contains_key("key") || magic.contains_key("fallback") {
                return Err(
                    "magic.keys and magic.wildcards cannot be combined with key or fallback".into(),
                );
            }
            import_wildcard_magic(magic, &labels, &mut action_names, &mut action_defs)?;
        } else {
            let explicit_key = magic
                .get("key")
                .map(|value| physical_key(value, "magic.key", &labels))
                .transpose()?;
            let mut grouped: BTreeMap<String, BTreeMap<Vec<u8>, RuleOutput>> = BTreeMap::new();
            if let Some(key) = &explicit_key {
                grouped.entry(key.clone()).or_default();
            }
            if let Some(value) = magic
                .get("rules")
                .filter(|value| !matches!(value, Json::Null))
            {
                for rule in array(value, "magic.rules")? {
                    let rule = object(rule, "magic rule")?;
                    let input = string(required(rule, "inputs")?, "magic rule inputs")?;
                    let output = string(required(rule, "output")?, "magic rule output")?
                        .to_ascii_lowercase();
                    let (last, ch) = input
                        .char_indices()
                        .last()
                        .ok_or("magic rule inputs cannot be empty")?;
                    let label = ch.to_string();
                    if explicit_key.as_ref().is_some_and(|key| key != &label) {
                        return Err(format!(
                            "magic rule {input:?} must end with magic.key {:?}",
                            explicit_key.as_ref().unwrap()
                        ));
                    }
                    if !labels.contains(&label) {
                        return Err(format!(
                            "magic rule refers to missing physical key {label:?}"
                        ));
                    }
                    // Only the suffix names a physical key. Its preceding
                    // context and emitted character follow corpus normalization.
                    let context = input[..last].to_ascii_lowercase();
                    if context.is_empty() || !context.bytes().all(|byte| (32..=126).contains(&byte))
                    {
                        return Err("magic rule must have a nonempty printable ASCII text context before its final physical key".into());
                    }
                    let Some(emitted) = output.strip_prefix(context.as_str()) else {
                        return Err(format!(
                        "magic rule {input:?}: output must preserve its text context {context:?}"
                    ));
                    };
                    if emitted.len() != 1
                        || !emitted.as_bytes()[0].is_ascii()
                        || !(32..=126).contains(&emitted.as_bytes()[0])
                    {
                        return Err(format!("magic rule {input:?}: output must append exactly one printable ASCII character"));
                    }
                    // Mana's loader replaces earlier rules with the same input.
                    grouped.entry(label).or_default().insert(
                        context.as_bytes().to_vec(),
                        RuleOutput::Text(emitted.to_ascii_lowercase().into_bytes()),
                    );
                }
            }
            for (index, (label, rules)) in grouped.into_iter().enumerate() {
                let name = if explicit_key.is_some() {
                    "magic".to_string()
                } else if matches!(label.as_str(), " " | "=" | "\"") {
                    format!("json-magic-{index}")
                } else {
                    label.clone()
                };
                if let Some(previous) = action_names.insert(label.clone(), name.clone()) {
                    return Err(format!(
                    "physical key {label:?} refers to both action {previous:?} and action {name:?}"
                ));
                }
                let default = if dedicated_magic_label(&label) {
                    "repeat-output".to_string()
                } else {
                    ak::quote(label.to_ascii_lowercase().as_bytes())
                };
                let fallback = fallback_text(magic.get("fallback"), &default, "magic.fallback")?;
                action_defs.insert(
                    name,
                    ImportedRules {
                        basis: "magic",
                        rules,
                        fallback,
                    },
                );
            }
        }
    }

    if let Some(skip) = root
        .get("skip")
        .filter(|value| !matches!(value, Json::Null))
    {
        let skip = object(skip, "skip")?;
        let key = physical_key(required(skip, "key")?, "skip.key", &labels)?;
        if let Some(previous) = action_names.insert(key.clone(), "skip".into()) {
            return Err(format!(
                "physical key {key:?} refers to both action {previous:?} and action \"skip\""
            ));
        }
        let mut rules = BTreeMap::new();
        if let Some(value) = skip
            .get("rules")
            .filter(|value| !matches!(value, Json::Null))
        {
            for value in array(value, "skip.rules")? {
                let rule = object(value, "skip rule")?;
                let input = string(required(rule, "inputs")?, "skip rule inputs")?;
                let Some(prefix) = input.strip_suffix(&key) else {
                    return Err(format!(
                        "skip rule {input:?} must end with skip.key {key:?}"
                    ));
                };
                let Some(context) = prefix.strip_suffix('_') else {
                    return Err(format!(
                        "skip rule {input:?} must use '_' for its one skipped key"
                    ));
                };
                if context.len() != 1 || !context.as_bytes()[0].is_ascii_graphic() {
                    return Err(format!(
                        "skip rule {input:?} needs one printable physical key before '_'"
                    ));
                }
                if !labels.contains(context) {
                    return Err(format!(
                        "skip rule refers to missing physical key {context:?}"
                    ));
                }
                let output = match (rule.get("output"), rule.get("call")) {
                    (Some(output), None) => {
                        let output = string(output, "skip rule output")?;
                        let expected = format!("{context}_");
                        let Some(emitted) = output.strip_prefix(&expected) else {
                            return Err(format!(
                                "skip rule {input:?}: output must preserve {expected:?}"
                            ));
                        };
                        if emitted.len() != 1
                            || !emitted.as_bytes()[0].is_ascii()
                            || !(32..=126).contains(&emitted.as_bytes()[0])
                        {
                            return Err(format!(
                                "skip rule {input:?}: output must append exactly one printable ASCII character"
                            ));
                        }
                        RuleOutput::Text(emitted.to_ascii_lowercase().into_bytes())
                    }
                    (None, Some(call)) => {
                        let call = physical_key(call, "skip rule call", &labels)?;
                        RuleOutput::CallKey(call)
                    }
                    _ => return Err("skip rule needs exactly one of output or call".into()),
                };
                rules.insert(context.as_bytes().to_vec(), output);
            }
        }
        let fallback = fallback_text(
            skip.get("fallback"),
            "repeat-previous-output",
            "skip.fallback",
        )?;
        action_defs.insert(
            "skip".into(),
            ImportedRules {
                basis: "skip-magic",
                rules,
                fallback,
            },
        );
    }

    let mut dat = String::new();
    for (row_index, row) in rows.iter().enumerate() {
        let default_split = if row.len() >= 12 {
            row.len().div_ceil(2)
        } else {
            5
        };
        let split = fingermap
            .get(row_index)
            .and_then(|map| {
                let fingers: Vec<_> = map.split_whitespace().collect();
                fingers.iter().position(|finger| finger.starts_with('R'))
            })
            .filter(|&split| {
                matches!(row.len(), 11 | 13)
                    && (5..=7).contains(&split)
                    && (5..=7).contains(&(row.len() - split))
            })
            .unwrap_or(default_split);
        let left = row[..split]
            .iter()
            .map(|label| dat_key(label, &action_names))
            .collect::<Vec<_>>()
            .join(" ");
        let right = row[split..]
            .iter()
            .map(|label| dat_key(label, &action_names))
            .collect::<Vec<_>>()
            .join(" ");
        dat.push_str(&format!("{left} | {right}\n"));
    }
    let thumb_tokens: Vec<_> = thumbs
        .iter()
        .map(|group| {
            if group.is_empty() {
                "none".into()
            } else {
                group
                    .iter()
                    .map(|label| dat_key(label, &action_names))
                    .collect::<Vec<_>>()
                    .join(" ")
            }
        })
        .collect();
    dat.push_str(&format!(
        "thumbs: {} | {}\n",
        thumb_tokens[0], thumb_tokens[1]
    ));
    dat.push_str("col-layout: absolute\n");
    if !fingermap.is_empty() {
        dat.push_str(&format!("fingermap: {}\n", fingermap.join(" / ")));
    }
    if let Some(board) = root.get("board") {
        let board = object(board, "board")?;
        if bool_field(board, "mirrorLeftRowStagger", false)? {
            return Err("board.mirrorLeftRowStagger=true is not supported".into());
        }
        match board.get("splitAngle") {
            None | Some(Json::Number(0.0)) => {}
            Some(Json::Number(_)) => return Err("nonzero board.splitAngle is not supported".into()),
            _ => return Err("board.splitAngle must be a number".into()),
        }
        let row_staggered = bool_field(board, "isRowStaggered", false)?;
        let column_staggered = bool_field(board, "isColumnStaggered", false)?;
        if row_staggered && column_staggered {
            return Err(
                "board.isRowStaggered and board.isColumnStaggered cannot both be true".into(),
            );
        }
        if row_staggered || column_staggered {
            let value = board.get("rowOrColumnStagger").ok_or_else(|| {
                "board.rowOrColumnStagger is required when row or column stagger is enabled"
                    .to_string()
            })?;
            let values = array(value, "board.rowOrColumnStagger")?;
            let expected = if row_staggered {
                3
            } else {
                rows.iter().map(Vec::len).max().unwrap()
            };
            if values.len() != expected {
                return Err(format!(
                    "board.rowOrColumnStagger needs {expected} values for this geometry"
                ));
            }
            let numbers = values
                .iter()
                .map(|value| match value {
                    Json::Number(number) => Ok(number.to_string()),
                    _ => Err("board.rowOrColumnStagger values must be numbers".to_string()),
                })
                .collect::<ak::Result<Vec<_>>>()?;
            let directive = if row_staggered {
                "row-offsets"
            } else {
                "column-offsets"
            };
            dat.push_str(&format!("{directive}: {}\n", numbers.join(" ")));
        }
        if let Some(value) = board.get("thumbColumns") {
            let values = array(value, "board.thumbColumns")?;
            let thumb_count = thumbs
                .iter()
                .flatten()
                .filter(|label| label.as_str() != "none")
                .count();
            if values.len() != thumb_count {
                return Err(format!(
                    "board.thumbColumns needs {thumb_count} values for this geometry"
                ));
            }
            let columns = values
                .iter()
                .map(|value| match value {
                    Json::Number(number)
                        if number.is_finite()
                            && number.fract() == 0.0
                            && (i8::MIN as f64..=i8::MAX as f64).contains(number) =>
                    {
                        Ok((*number as i8).to_string())
                    }
                    _ => Err("board.thumbColumns values must be integers in -128..127".into()),
                })
                .collect::<ak::Result<Vec<_>>>()?;
            dat.push_str(&format!("thumb-columns: {}\n", columns.join(" ")));
        }
    }

    for (name, definition) in &action_defs {
        dat.push_str(&format!("action {name} = {}\n", definition.basis));
        dat.push_str(&format!("fallback {name} = {}\n", definition.fallback));
        for (context, output) in &definition.rules {
            let context = if definition.basis == "skip-magic" {
                let label = std::str::from_utf8(context).unwrap();
                action_names
                    .get(label)
                    .map_or_else(|| context.clone(), |name| name.as_bytes().to_vec())
            } else {
                context.clone()
            };
            let output = match output {
                RuleOutput::Text(output) => ak::quote(output),
                RuleOutput::CallKey(label) => {
                    let name = action_names.get(label).ok_or_else(|| {
                        format!("called physical key {label:?} is not a magic or skip key")
                    })?;
                    format!("@{name}")
                }
            };
            dat.push_str(&format!("map {name} {} = {output}\n", ak::quote(&context),));
        }
    }

    let mut parsed = Layout::parse_dat_preserving_fallbacks(&dat, path)?;
    for slot in &mut parsed.slots {
        if let ak::Binding::Named(name) = &slot.binding {
            if let Some((label, _)) = action_names.iter().find(|(_, value)| *value == name) {
                slot.label = label.clone();
            }
        }
    }
    if let Some(value) = root
        .get("alts")
        .filter(|value| !matches!(value, Json::Null))
    {
        parsed.alt_fingers = crate::alt_fingers::parse_json(value)?;
    }
    if let Some(value) = root
        .get("combos")
        .filter(|value| !matches!(value, Json::Null))
    {
        parsed.combos = array(value, "combos")?.to_vec();
        if !parsed.combos.is_empty() {
            parsed
                .source_warnings
                .push("Combos are preserved; [typing] combos controls their scoring.".into());
        }
    }
    Ok(parsed)
}

fn import_layout(root: &BTreeMap<String, Json>, path: &Path) -> ak::Result<Layout> {
    import_layout_with_actions(root, path, BTreeMap::new(), BTreeMap::new(), false)
}

#[derive(Clone)]
struct SparkKey {
    label: String,
    row: usize,
    col: usize,
    finger: usize,
}

fn spark_finger(value: &Json, field: &str) -> ak::Result<usize> {
    let name = string(value, field)?;
    ["LP", "LR", "LM", "LI", "LT", "RT", "RI", "RM", "RR", "RP"]
        .iter()
        .position(|candidate| name.eq_ignore_ascii_case(candidate))
        .ok_or_else(|| format!("{field}: unknown finger {name:?}"))
}

fn import_spark(root: &BTreeMap<String, Json>, path: &Path) -> ak::Result<Layout> {
    let format = string(required(root, "format")?, "format")?;
    if format != "spark/1" {
        return Err(format!("unsupported Spark layout format {format:?}"));
    }
    let source_name = string(required(root, "name")?, "name")?;
    if source_name.trim().is_empty() || source_name.chars().any(char::is_control) {
        return Err("name: expected nonempty text without control characters".into());
    }
    let payload = object(required(root, "payload")?, "payload")?;
    for field in payload.keys() {
        if !matches!(field.as_str(), "keys" | "magic" | "alts" | "combos") {
            return Err(format!("unsupported spark/1 payload field {field:?}"));
        }
    }

    let mut keys = Vec::new();
    let mut coordinates = BTreeSet::new();
    for (index, value) in array(required(payload, "keys")?, "payload.keys")?
        .iter()
        .enumerate()
    {
        let field = format!("payload.keys[{index}]");
        let key = object(value, &field)?;
        for name in key.keys() {
            if !matches!(name.as_str(), "char" | "row" | "col" | "finger") {
                return Err(format!("{field}: unsupported field {name:?}"));
            }
        }
        let label = key_token(string(required(key, "char")?, &format!("{field}.char"))?)?;
        if label.is_empty() {
            return Err(format!("{field}.char: a Spark key cannot be empty"));
        }
        let row = integer_field(key, "row", &field)?;
        let col = integer_field(key, "col", &field)?;
        if row > 3 {
            return Err(format!(
                "{field}.row: only main rows 0..2 and thumb row 3 are supported"
            ));
        }
        if col > 11 {
            return Err(format!("{field}.col: only columns 0..11 are supported"));
        }
        let finger = spark_finger(required(key, "finger")?, &format!("{field}.finger"))?;
        if row == 3 && !matches!(finger, 4 | 5) {
            return Err(format!("{field}: row 3 requires LT or RT"));
        }
        if row < 3 && matches!(finger, 4 | 5) {
            return Err(format!("{field}: LT and RT belong on thumb row 3"));
        }
        if !coordinates.insert((row, col)) {
            return Err(format!(
                "payload.keys contains duplicate position [{row}, {col}]"
            ));
        }
        keys.push(SparkKey {
            label,
            row,
            col,
            finger,
        });
    }
    if keys.is_empty() {
        return Err("payload.keys must contain physical keys".into());
    }

    let main_width = keys
        .iter()
        .filter(|key| key.row < 3)
        .map(|key| key.col + 1)
        .max()
        .unwrap_or(0);
    if !(10..=12).contains(&main_width) {
        return Err(format!(
            "spark/1 main rows need 10, 11, or 12 columns; found {main_width}"
        ));
    }
    let default_finger = |col: usize| -> usize {
        match col {
            0 => 0,
            1 => 1,
            2 => 2,
            3 | 4 => 3,
            5 | 6 => 6,
            7 => 7,
            8 => 8,
            _ => 9,
        }
    };
    let mut rows = Vec::new();
    let mut fingermap = Vec::new();
    for row in 0..3 {
        let mut labels = Vec::new();
        let mut fingers = Vec::new();
        for col in 0..main_width {
            if let Some(key) = keys.iter().find(|key| key.row == row && key.col == col) {
                labels.push(if key.label == " " {
                    "space".into()
                } else {
                    key.label.clone()
                });
                // Spark and Mana use the same LP..LI, LT, RT, RI..RP IDs.
                fingers.push(key.finger.to_string());
            } else {
                labels.push("skip".into());
                fingers.push(default_finger(col).to_string());
            }
        }
        rows.push(Json::String(labels.join(" ")));
        fingermap.push(Json::String(fingers.join(" ")));
    }

    let mut spark_thumbs: [Vec<SparkKey>; 2] = [Vec::new(), Vec::new()];
    for key in keys.iter().filter(|key| key.row == 3) {
        spark_thumbs[key.finger - 4].push(key.clone());
    }
    for group in &mut spark_thumbs {
        group.sort_by_key(|key| key.col);
    }
    let thumb_text = spark_thumbs
        .iter()
        .map(|group| {
            Json::String(
                group
                    .iter()
                    .map(|key| {
                        if key.label == " " {
                            "space".to_string()
                        } else {
                            key.label.clone()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
            )
        })
        .collect();

    let mut action_names = BTreeMap::new();
    let mut action_defs = BTreeMap::new();
    if let Some(value) = payload
        .get("magic")
        .filter(|value| !matches!(value, Json::Null))
    {
        let magic = object(value, "payload.magic")?;
        for field in magic.keys() {
            if field != "magic_keys" {
                return Err(format!("payload.magic: unsupported field {field:?}"));
            }
        }
        if let Some(value) = magic.get("magic_keys") {
            for (index, value) in array(value, "payload.magic.magic_keys")?.iter().enumerate() {
                let field = format!("payload.magic.magic_keys[{index}]");
                let magic_key = object(value, &field)?;
                for name in magic_key.keys() {
                    if !matches!(name.as_str(), "key" | "default" | "rules") {
                        return Err(format!("{field}: unsupported field {name:?}"));
                    }
                }
                let label = key_token(string(
                    required(magic_key, "key")?,
                    &format!("{field}.key"),
                )?)?;
                if keys.iter().filter(|key| key.label == label).count() != 1 {
                    return Err(format!(
                        "{field}.key must identify exactly one physical key"
                    ));
                }
                let default = object(required(magic_key, "default")?, &format!("{field}.default"))?;
                if default
                    .keys()
                    .any(|name| !matches!(name.as_str(), "kind" | "char"))
                {
                    return Err(format!("{field}.default: only kind and char are supported"));
                }
                if string(required(default, "kind")?, &format!("{field}.default.kind"))? != "char" {
                    return Err(format!("{field}.default.kind: only \"char\" is supported"));
                }
                let output = string(required(default, "char")?, &format!("{field}.default.char"))?;
                if output.len() != 1 || !output.as_bytes()[0].is_ascii_graphic() {
                    return Err(format!(
                        "{field}.default.char: expected one printable ASCII character"
                    ));
                }
                if let Some(rules) = magic_key.get("rules") {
                    if !array(rules, &format!("{field}.rules"))?.is_empty() {
                        return Err(format!(
                            "{field}.rules: Spark magic rules are not supported yet"
                        ));
                    }
                }
                let name = match label.as_str() {
                    "@" => "magic".into(),
                    "$" => "skip".into(),
                    _ => label.clone(),
                };
                if action_names.insert(label, name.clone()).is_some() {
                    return Err(format!("{field}: duplicate magic key"));
                }
                action_defs.insert(
                    name,
                    ImportedRules {
                        basis: "magic",
                        rules: BTreeMap::new(),
                        fallback: ak::quote(&[output.as_bytes()[0].to_ascii_lowercase()]),
                    },
                );
            }
        }
    }

    let mut converted = BTreeMap::new();
    converted.insert(
        "layout".into(),
        Json::Object(BTreeMap::from([
            ("fingers".into(), Json::Array(rows)),
            ("thumbs".into(), Json::Array(thumb_text)),
        ])),
    );
    converted.insert("fingermap".into(), Json::Array(fingermap));
    if let Some(value) = payload.get("alts") {
        converted.insert("alts".into(), value.clone());
    }
    if let Some(value) = payload.get("combos") {
        converted.insert("combos".into(), value.clone());
    }

    let mut parsed =
        import_layout_with_actions(&converted, path, action_names, action_defs, false)?;
    for hand in 0..2 {
        let parsed_thumbs: Vec<_> = parsed
            .slots
            .iter_mut()
            .filter(|slot| !slot.main && slot.hand == hand as i8)
            .collect();
        // The parser keeps each hand's declared order. An implicit Space is
        // inserted only on the other, empty hand.
        if !spark_thumbs[hand].is_empty() && parsed_thumbs.len() != spark_thumbs[hand].len() {
            return Err("Spark thumb geometry could not be preserved".into());
        }
        for (slot, source) in parsed_thumbs.into_iter().zip(&spark_thumbs[hand]) {
            slot.col = source.col as i8;
        }
    }
    parsed.name = source_name.to_string();
    parsed.source_warnings.push(
        "spark/1 does not provide physical stagger coordinates; imported with zero offsets.".into(),
    );
    Ok(parsed)
}

pub(crate) fn parse_json_layout(text: &str, path: &Path) -> ak::Result<Layout> {
    let parsed = parse_jsonc(text).map_err(|error| format!("{}: {error}", path.display()))?;
    let root = object(&parsed, "layout document")?;
    let result = if root.contains_key("format") || root.contains_key("payload") {
        import_spark(root, path)
    } else {
        import_layout(root, path)
    };
    let mut layout = result.map_err(|error| format!("{}: {error}", path.display()))?;
    if layout.alt_fingers.is_empty() {
        if let Some(notes) = alt_finger_notes(text) {
            match crate::alt_fingers::AltRules::parse_letters(&notes) {
                Ok(rules) => layout.alt_fingers = rules,
                Err(error) => layout
                    .source_warnings
                    .push(format!("alt fingers notes ignored: {error}")),
            }
        }
    }
    Ok(layout)
}

/// The rules under a `// alt fingers` heading in the leading comments, as
/// stray's exports write them; used when the file has no `alts` array, which
/// some tools drop when they save.
fn alt_finger_notes(text: &str) -> Option<String> {
    let mut lines = text
        .lines()
        .map(str::trim)
        .take_while(|line| line.starts_with("//"))
        .map(|line| line.trim_start_matches('/').trim());
    lines.find(|line| line.eq_ignore_ascii_case("alt fingers"))?;
    let notes: Vec<_> = lines.take_while(|line| !line.is_empty()).collect();
    (!notes.is_empty()).then(|| notes.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ak::{Action, Binding, Emission};

    fn example(marker: &str, rules: &str, board: &str) -> String {
        format!(
            r#"{{
                "layout": {{
                    "fingers": ["q w {marker} r t y u i o p", "a s d f g h j k l ;", "z x c v b n m , . /",],
                    "thumbs": ["space", "e"],
                }},
                "fingermap": ["0 1 2 3 3 6 6 7 8 9", "0 1 2 3 3 6 6 7 8 9", "0 1 2 3 3 6 6 7 8 9"],
                "board": {board},
                "layers": null,
                "magic": {{"magicKeys": [], "rules": [{rules}]}},
            }}"#
        )
    }

    fn row_board() -> &'static str {
        r#"{"isRowStaggered":true,"mirrorLeftRowStagger":false,"splitAngle":0,"rowOrColumnStagger":[0,0.25,0.75]}"#
    }

    fn parse(text: &str) -> Layout {
        Layout::parse(text, Path::new("layout-without-extension")).unwrap()
    }

    fn key(layout: &Layout, label: &str) -> usize {
        layout
            .slots
            .iter()
            .position(|slot| slot.label == label)
            .unwrap()
    }

    fn spark_example() -> String {
        let rows = vec![
            vec![
                ("у", 0, "LP"),
                ("l", 1, "LR"),
                ("d", 2, "LM"),
                ("p", 3, "LI"),
                ("x", 4, "LI"),
                ("j", 5, "RI"),
                (",", 6, "RI"),
                ("u", 7, "RM"),
                ("o", 8, "RR"),
                ("y", 9, "RP"),
                (";", 10, "RP"),
            ],
            vec![
                ("t", 0, "LP"),
                ("r", 1, "LR"),
                ("n", 2, "LM"),
                ("c", 3, "LI"),
                ("k", 4, "LI"),
                ("f", 5, "RI"),
                ("h", 6, "RI"),
                ("a", 7, "RM"),
                ("i", 8, "RR"),
                ("e", 9, "RP"),
                ("-", 10, "RP"),
            ],
            vec![
                ("v", 0, "LP"),
                ("q", 1, "LM"),
                ("g", 2, "LI"),
                ("w", 3, "LI"),
                ("'", 4, "LI"),
                ("b", 5, "RI"),
                ("m", 6, "RI"),
                ("z", 7, "RM"),
                (".", 8, "RP"),
                ("/", 9, "RP"),
            ],
        ];
        let mut keys = Vec::new();
        // Reverse order verifies that coordinates, rather than array order,
        // define physical slots.
        for (row, values) in rows.iter().enumerate().rev() {
            for (label, col, finger) in values.iter().rev() {
                keys.push(format!(
                    r#"{{"char":{},"col":{col},"finger":"{finger}","row":{row}}}"#,
                    crate::json_quote(label)
                ));
            }
        }
        keys.push(r#"{"char":"s","col":6,"finger":"RT","row":3}"#.into());
        format!(
            r#"{{
                "name":"slate",
                "format":"spark/1",
                "payload":{{
                    "keys":[{}],
                    "magic":{{"magic_keys":[{{"key":"у","default":{{"kind":"char","char":"y"}},"rules":[]}}]}},
                    "alts":[{{"ngram":["p",null,"g"],"fingers":["LM",null,"LI"]}}],
                    "combos":[{{"keys":["a","u"],"output":"au"}}]
                }}
            }}"#,
            keys.join(",")
        )
    }

    #[test]
    fn spark_wrapper_preserves_slots_magic_alts_and_metadata() {
        let layout = Layout::parse(&spark_example(), Path::new("download.json")).unwrap();
        assert_eq!(layout.name, "slate");
        assert_eq!(layout.alt_fingers.len(), 1);
        assert_eq!(layout.combos.len(), 1);
        assert!(layout
            .source_warnings
            .iter()
            .any(|warning| warning.contains("zero offsets")));
        assert!(layout
            .source_warnings
            .iter()
            .any(|warning| warning.contains("Combos are preserved")));

        let ordered = |row| {
            let mut slots: Vec<_> = layout
                .slots
                .iter()
                .filter(|slot| slot.main && slot.row == row)
                .collect();
            slots.sort_by_key(|slot| slot.col);
            slots
                .into_iter()
                .map(|slot| slot.label.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ordered(0),
            ["у", "l", "d", "p", "x", "j", ",", "u", "o", "y", ";"]
        );
        assert_eq!(ordered(2).last(), Some(&"·"));
        assert_eq!(layout.slots.iter().filter(|slot| slot.main).count(), 33);
        assert_eq!(layout.slots[key(&layout, "q")].finger, 2);
        assert_eq!(layout.slots[key(&layout, "j")].finger, 4);
        assert_eq!(layout.slots[key(&layout, "u")].finger, 5);
        assert_eq!(layout.slots[key(&layout, "o")].finger, 6);
        assert_eq!(layout.slots[key(&layout, ".")].finger, 7);
        assert!(layout
            .slots
            .iter()
            .filter(|slot| slot.main)
            .all(|slot| slot.row_offset == 0 && slot.column_offset == 0));

        let action = key(&layout, "у");
        assert!(matches!(layout.slots[action].binding, Binding::Named(_)));
        assert_eq!(ak::trace_keys(&layout, &[action]).unwrap()[0].output, b"y");
        let right_thumb = layout
            .slots
            .iter()
            .find(|slot| !slot.main && slot.label == "s")
            .unwrap();
        assert_eq!(
            (right_thumb.hand, right_thumb.finger, right_thumb.col),
            (1, 9, 6)
        );
        assert!(layout.slots.iter().any(|slot| {
            !slot.main
                && slot.hand == 0
                && matches!(&slot.binding, Binding::Text(text) if text == b" ")
        }));

        let jsonc = crate::layout_export::jsonc_text(&layout).unwrap();
        for (path, text) in [("slate.dat", layout.text()), ("slate.jsonc", jsonc)] {
            let restored = Layout::parse(&text, Path::new(path)).unwrap();
            assert_eq!(restored.slots, layout.slots, "{path}");
            assert_eq!(restored.alt_fingers, layout.alt_fingers, "{path}");
            assert_eq!(
                crate::alt_fingers::compact_json(&Json::Array(restored.combos.clone())),
                crate::alt_fingers::compact_json(&Json::Array(layout.combos.clone())),
                "{path}"
            );
            let action = key(&restored, "у");
            assert_eq!(
                ak::trace_keys(&restored, &[action]).unwrap()[0].output,
                b"y",
                "{path}"
            );
        }
        for marker in ["@", "$"] {
            let source = spark_example().replace('у', marker);
            let layout = Layout::parse(&source, Path::new("spark.json")).unwrap();
            let jsonc = crate::layout_export::jsonc_text(&layout).unwrap();
            let restored = Layout::parse(&jsonc, Path::new("round.jsonc")).unwrap();
            assert_eq!(restored.slots, layout.slots);
            assert_eq!(restored.actions, layout.actions);
        }
    }

    #[test]
    fn spark_magic_rule_effects_fail_explicitly() {
        let text = spark_example().replace(r#""rules":[]"#, r#""rules":[{"when":"a","char":"b"}]"#);
        let error = Layout::parse(&text, Path::new("spark.json")).unwrap_err();
        assert!(
            error.contains("Spark magic rules are not supported yet"),
            "{error}"
        );
    }

    #[test]
    fn alt_finger_notes_stand_in_for_a_dropped_alts_array() {
        let body = r#"{"layout":{"fingers":["q w e r t y u i o p","a s d f g h j k l ;","z x c v b n m , . /"]}}"#;
        let notes = "// Notes.\n//\n// alt fingers\n//   dg d_g (LM LI) · gd g_d (LI LM)\n//\n";
        let layout = parse_json_layout(&format!("{notes}{body}"), Path::new("x.jsonc")).unwrap();
        let expected = crate::alt_fingers::AltRules::parse_letters("dg d_g (LM LI) gd g_d (LI LM)");
        assert_eq!(layout.alt_fingers, expected.unwrap());
        assert_eq!(layout.alt_fingers.len(), 4);
        // An alts array wins; no notes means no rules.
        let alts = r#"{"alts":[{"ngram":["s","g"],"fingers":["LM","LI"]}],"#;
        let with_array = format!("{notes}{alts}{}", &body[1..]);
        let layout = parse_json_layout(&with_array, Path::new("x.jsonc")).unwrap();
        assert_eq!(layout.alt_fingers.len(), 1);
        assert!(parse_json_layout(body, Path::new("x.jsonc"))
            .unwrap()
            .alt_fingers
            .is_empty());
    }

    #[test]
    fn mana_json_preserves_alts_and_combos() {
        let text = example("@", "", row_board()).replacen(
            r#""layers": null,"#,
            r#""layers": null,
               "alts":[{"ngram":["q",null,"e"],"fingers":["LM",null,"LI"]}],
               "combos":[{"keys":["a","s"],"output":"as"}],"#,
            1,
        );
        let layout = parse(&text);
        assert_eq!(layout.alt_fingers.len(), 1);
        assert_eq!(layout.combos.len(), 1);
        assert!(layout
            .source_warnings
            .iter()
            .any(|warning| warning.contains("Combos are preserved")));
    }

    #[test]
    fn unequal_rows_need_explicit_empty_slots() {
        let error = Layout::parse(
            r#"{"layout":{"fingers":[
                "q w e r t y u i o p",
                "a s d f g h j k l ; [ ]",
                "z x c v b n m , . /"
            ]}}"#,
            Path::new("unequal.jsonc"),
        )
        .unwrap_err();
        assert!(error.contains("10, 12, and 10 physical slots"), "{error}");
        assert!(error.contains("add skip"), "{error}");
        let padded = r#"{"layout":{"fingers":[
            "skip q w e r t y u i o p skip",
            "skip a s d f g h j k l ; skip",
            "skip z x c v b n m , . / skip"
        ]}}"#;
        assert!(Layout::parse(padded, Path::new("padded.jsonc")).is_ok());
    }

    #[test]
    fn format_detection_reads_content_and_keeps_literal_braces() {
        let text = example("@", "", row_board());
        assert!(is_json_layout(&format!(
            "\u{feff}// heading\n/* block */ {text}"
        )));
        assert!(!Layout::parse(&text, Path::new("looks-like-dat.dat"))
            .unwrap()
            .extended());
        let declared = text.replacen("\"magicKeys\": []", "\"key\": \"@\"", 1);
        assert!(Layout::parse(&declared, Path::new("declared.jsonc"))
            .unwrap()
            .extended());
        assert!(!is_json_layout(
            "{ w e r t y u i o p\na s d f g h j k l ;\nz x c v b n m , . /\n"
        ));
        assert!(!is_json_layout(
            "{ \" e r t y u i o p\na s d f g h j k l ;\nz x c v b n m , . /\n"
        ));
    }

    #[test]
    fn jsonc_normalization_preserves_string_comments_escapes_and_commas() {
        let Json::Object(value) =
            parse_jsonc(r#"/* head */ {"text":"// /* ,] ,} \\\"", "values":[1,2,],}// tail"#)
                .unwrap()
        else {
            panic!("object expected");
        };
        let Json::String(text) = &value["text"] else {
            panic!("string expected");
        };
        assert!(text.starts_with("// /* ,] ,}"));
        assert!(matches!(&value["values"], Json::Array(values) if values.len() == 2));
        assert!(parse_jsonc("{/* unclosed").is_err());
        assert!(parse_jsonc(r#"{"x":1,"x":2}"#).is_err());
        assert!(parse_jsonc(r#"{"x":[1,,]}"#).is_err());
        assert!(parse_jsonc("[,]").is_err());
        assert!(parse_jsonc("{,}").is_err());
    }

    #[test]
    fn unicode_magic_and_ascii_markers_repeat_unlisted_contexts() {
        for marker in ["◇", "★", "@", "*"] {
            let rules = format!(r#"{{"inputs":"a{marker}","output":"ay"}}"#);
            let layout = parse(&example(marker, &rules, row_board()));
            let magic = key(&layout, marker);
            let steps = ak::trace_keys(&layout, &[key(&layout, "a"), magic]).unwrap();
            assert_eq!(steps[1].key, magic);
            assert_eq!(steps[1].output, b"y");

            let steps = ak::trace_keys(&layout, &[key(&layout, "q"), magic, magic]).unwrap();
            assert_eq!(steps[1].output, b"q");
            assert_eq!(steps[2].output, b"q");
            assert!(ak::trace_keys(&layout, &[magic]).is_err());
        }
    }

    #[test]
    fn adaptive_comma_and_literal_space_contexts_are_not_trimmed() {
        let rules = r#"{"inputs":"u,","output":"ui"},{"inputs":"ui","output":"u,"},{"inputs":" @","output":"  "}"#;
        let layout = parse(&example("@", rules, row_board()));
        let comma = key(&layout, ",");
        let after_u = ak::trace_keys(&layout, &[key(&layout, "u"), comma]).unwrap();
        let after_q = ak::trace_keys(&layout, &[key(&layout, "q"), comma]).unwrap();
        assert_eq!(after_u[1].output, b"i");
        assert_eq!(after_q[1].output, b",");

        let Binding::Named(name) = &layout.slots[key(&layout, "@")].binding else {
            panic!("action expected");
        };
        let Action::Rules {
            rules, fallback, ..
        } = &layout.actions[name]
        else {
            panic!("rules expected");
        };
        assert_eq!(
            rules.get(b" ".as_slice()),
            Some(&Emission::Text(b" ".to_vec()))
        );
        assert_eq!(fallback, &Emission::Call("repeat-output".into()));
    }

    #[test]
    fn duplicate_input_uses_last_rule_and_longest_context_still_wins() {
        let rules = r#"{"inputs":"q@","output":"qu"},{"inputs":"q@","output":"qi"},{"inputs":"aq@","output":"aqy"}"#;
        let layout = parse(&example("@", rules, row_board()));
        let magic = key(&layout, "@");
        let short = ak::trace_keys(&layout, &[key(&layout, "q"), magic]).unwrap();
        let long = ak::trace_keys(&layout, &[key(&layout, "a"), key(&layout, "q"), magic]).unwrap();
        assert_eq!(short[1].output, b"i");
        assert_eq!(long[2].output, b"y");
    }

    #[test]
    fn geometry_preserves_fingers_and_decimal_offsets() {
        let layout = parse(&example("@", "", row_board()));
        assert_eq!(layout.slots.iter().filter(|slot| slot.main).count(), 30);
        let home = &layout.slots[key(&layout, ";")];
        assert_eq!((home.row, home.col, home.finger), (1, 9, 7));
        assert_eq!((home.row_offset, home.column_offset), (250, 0));
        assert_eq!(layout.slots[key(&layout, "z")].row_offset, 750);
        assert_eq!(layout.slots[key(&layout, "y")].finger, 4);

        let board = r#"{"isColumnStaggered":true,"rowOrColumnStagger":[0,-0.3,-0.4,-0.3,-0.2,-0.2,-0.3,-0.4,-0.3,0]}"#;
        let columns = parse(&example("@", "", board));
        assert_eq!(columns.slots[key(&columns, "w")].column_offset, -300);
        assert_eq!(columns.slots[key(&columns, "w")].row_offset, 0);
        assert_eq!(columns.slots[key(&columns, "s")].column_offset, -300);
    }

    #[test]
    fn stagger_offsets_need_an_explicit_direction() {
        let flat = parse(&example(
            "@",
            "",
            r#"{"isRowStaggered":false,"rowOrColumnStagger":[0,0,5]}"#,
        ));
        assert!(flat
            .slots
            .iter()
            .filter(|slot| slot.main)
            .all(|slot| slot.row_offset == 0 && slot.column_offset == 0));

        let both = example(
            "@",
            "",
            r#"{"isRowStaggered":true,"isColumnStaggered":true,"rowOrColumnStagger":[0,0,0]}"#,
        );
        let error = Layout::parse(&both, Path::new("both.jsonc")).unwrap_err();
        assert!(error.contains("cannot both be true"), "{error}");

        let missing = example("@", "", r#"{"isColumnStaggered":true}"#);
        let error = Layout::parse(&missing, Path::new("missing.jsonc")).unwrap_err();
        assert!(error.contains("rowOrColumnStagger is required"), "{error}");
    }

    #[test]
    fn absent_thumbs_and_literal_marker_escapes_preserve_physical_slots() {
        let base = example("char:*", "", row_board());
        for (thumbs, expected) in [
            (r#"["space"]"#, 1),
            (r#"["","e"]"#, 2),
            ("[]", 1),
            (r#"["skip","e"]"#, 2),
            (r#"["none","e"]"#, 1),
            (r#"["none"]"#, 0),
        ] {
            let layout = parse(&base.replace(r#"["space", "e"]"#, thumbs));
            assert_eq!(
                layout.slots.iter().filter(|slot| !slot.main).count(),
                expected
            );
            assert_eq!(
                layout.slots[key(&layout, "*")].binding,
                Binding::Text(b"*".to_vec())
            );
            assert!(!layout.extended());
        }

        let right_space = parse(&base.replace(r#"["space", "e"]"#, r#"["e",""]"#));
        let space = right_space
            .slots
            .iter()
            .find(|slot| matches!(&slot.binding, Binding::Text(text) if text == b" "))
            .unwrap();
        assert!(!space.main);
        assert_eq!(space.hand, 1);

        let main_space_source = example("space", "", row_board());
        for text in [
            main_space_source.replace(r#""thumbs": ["space", "e"],"#, ""),
            main_space_source.replace(r#"["space", "e"]"#, "[]"),
            main_space_source.replace(r#"["space", "e"]"#, r#"["","e"]"#),
        ] {
            let main_space = parse(&text);
            assert_eq!(
                main_space
                    .slots
                    .iter()
                    .filter(|slot| matches!(&slot.binding, Binding::Text(text) if text == b" "))
                    .count(),
                1
            );
            assert!(
                main_space
                    .slots
                    .iter()
                    .find(|slot| matches!(&slot.binding, Binding::Text(text) if text == b" "))
                    .unwrap()
                    .main
            );
        }

        let magic = parse(
            &example("char:*", r#"{"inputs":"a*","output":"ay"}"#, row_board())
                .replace(r#"["space", "e"]"#, r#"["","*"]"#),
        );
        let magic_thumb = magic
            .slots
            .iter()
            .find(|slot| !slot.main && slot.label == "*")
            .unwrap();
        assert_eq!(magic_thumb.hand, 1);
        assert!(matches!(magic_thumb.binding, Binding::Named(_)));
        assert!(magic.slots.iter().any(|slot| {
            !slot.main
                && slot.hand == 0
                && matches!(&slot.binding, Binding::Text(text) if text == b" ")
        }));

        let mixed_none = base.replace(r#"["space", "e"]"#, r#"["none e",""]"#);
        let error = Layout::parse(&mixed_none, Path::new("mixed-none.jsonc")).unwrap_err();
        assert!(error.contains("none cannot be combined"), "{error}");
    }

    #[test]
    fn multiple_thumb_keys_load_and_round_trip() {
        let source = r#"{
            "layout": {
                "fingers": [", u o c z k d g b j skip", "i e a $ # m t s n h q", ". ; / w ' x v f p y skip"],
                "thumbs": ["", "l r"]
            }
        }"#;
        let layout = parse(source);
        let thumbs: Vec<_> = layout.slots.iter().filter(|slot| !slot.main).collect();
        assert_eq!(thumbs.len(), 3);
        assert_eq!(
            (thumbs[0].label.as_str(), thumbs[0].hand, thumbs[0].finger),
            ("␠", 0, 8)
        );
        assert_eq!(
            (thumbs[1].label.as_str(), thumbs[1].hand, thumbs[1].finger),
            ("l", 1, 9)
        );
        assert_eq!(
            (thumbs[2].label.as_str(), thumbs[2].hand, thumbs[2].finger),
            ("r", 1, 9)
        );
        assert_ne!(thumbs[1].col, thumbs[2].col);

        let dat = layout.text();
        assert!(dat.contains("thumbs: space | l r\n"));
        let from_dat = parse(&dat);
        assert_eq!(from_dat.slots, layout.slots);

        let jsonc = crate::layout_export::jsonc_text(&layout).unwrap();
        assert!(jsonc.contains("\"thumbs\": [\"space\", \"l r\"]"));
        let from_jsonc = parse(&jsonc);
        assert_eq!(from_jsonc.slots, layout.slots);

        let both = parse(&source.replace("[\"\", \"l r\"]", "[\"l r\", \"space =\"]"));
        let thumbs: Vec<_> = both.slots.iter().filter(|slot| !slot.main).collect();
        assert_eq!(thumbs.len(), 4);
        assert_eq!(
            thumbs.iter().map(|slot| slot.hand).collect::<Vec<_>>(),
            [0, 0, 1, 1]
        );
        assert_eq!(parse(&both.text()).slots, both.slots);
    }

    #[test]
    fn afterburner_combines_magic_and_skip_rules() {
        let layout = parse(include_str!("../layouts/afterburner.jsonc"));
        let physical_row = |row| {
            let mut slots: Vec<_> = layout
                .slots
                .iter()
                .filter(|slot| slot.main && slot.row == row)
                .collect();
            slots.sort_by_key(|slot| slot.col);
            slots
                .into_iter()
                .map(|slot| slot.label.as_str())
                .collect::<Vec<_>>()
        };
        for row in 0..3 {
            assert_eq!(physical_row(row).len(), 11);
        }
        assert_eq!(physical_row(0).last(), Some(&"·"));
        assert_eq!(physical_row(2).last(), Some(&"·"));
        let thumbs: Vec<_> = layout
            .slots
            .iter()
            .filter(|slot| !slot.main)
            .map(|slot| (slot.label.as_str(), slot.hand))
            .collect();
        assert_eq!(thumbs, [("␠", 0), ("l", 1), ("r", 1)]);

        let magic = key(&layout, "#");
        let skip = key(&layout, "$");
        assert_eq!(layout.slots[magic].binding, Binding::Named("#".into()));
        assert_eq!(layout.slots[skip].binding, Binding::Named("skip".into()));

        let Action::Rules {
            basis: ak::Basis::Text,
            rules,
            fallback: Emission::Call(fallback),
        } = &layout.actions["#"]
        else {
            panic!("text magic action expected");
        };
        assert_eq!(rules[b"a".as_slice()], Emission::Text(b"o".to_vec()));
        assert_eq!(fallback, "repeat-output");

        let Action::Rules {
            basis: ak::Basis::SkipPress,
            rules,
            ..
        } = &layout.actions["skip"]
        else {
            panic!("skip magic action expected");
        };
        assert_eq!(rules[b"#".as_slice()], Emission::Call("#".into()));

        let typed = |labels: &[&str]| {
            ak::trace_keys(
                &layout,
                &labels
                    .iter()
                    .map(|label| key(&layout, label))
                    .collect::<Vec<_>>(),
            )
            .unwrap()
            .into_iter()
            .flat_map(|step| step.output)
            .collect::<Vec<_>>()
        };
        assert_eq!(typed(&["a", "l", "#"]), b"all");
        assert_eq!(typed(&["a", "#"]), b"ao");
        assert_eq!(typed(&["a", "#", "#"]), b"aoo");
        assert_eq!(typed(&["c", "#"]), b"cc");
        assert_eq!(typed(&["a", "c", "$"]), b"aco");
        assert_eq!(typed(&["r", "u", "$", "#"]), b"rull");
        assert_eq!(typed(&["i", "s", "#", "u", "$"]), b"issue");
        assert_eq!(typed(&["c", "u", "$"]), b"cuc");
    }

    #[test]
    fn later_wildcard_replaces_an_earlier_exact_rule() {
        let text = include_str!("../layouts/afterburner.jsonc").replace(
            "{\"inputs\": \"*#\", \"output\": \"**\"},\n      {\"inputs\": \"a#\", \"output\": \"ao\"}",
            "{\"inputs\": \"a#\", \"output\": \"ao\"},\n      {\"inputs\": \"*#\", \"output\": \"**\"}",
        );
        let layout = parse(&text);
        let steps = ak::trace_keys(&layout, &[key(&layout, "a"), key(&layout, "#")]).unwrap();
        assert_eq!(
            steps
                .into_iter()
                .flat_map(|step| step.output)
                .collect::<Vec<_>>(),
            b"aa"
        );
    }

    #[test]
    fn missing_skip_context_names_the_absent_key() {
        let text = include_str!("../layouts/afterburner.jsonc").replace(
            "{\"inputs\": \"#_$\", \"call\": \"#\"}",
            "{\"inputs\": \"-_$\", \"output\": \"-_i\"},\n      {\"inputs\": \"#_$\", \"call\": \"#\"}",
        );
        let error = Layout::parse(&text, Path::new("missing-key.jsonc")).unwrap_err();
        assert!(error.contains("missing physical key \"-\""), "{error}");
    }

    #[test]
    fn twelve_column_rows_keep_absolute_coordinates_and_custom_fingers() {
        let text = r#"{
            "layout": {
                "fingers": ["q w e r t y u i o p [ ]", "a s d f g h j k l ; skip skip", "z x c v b n m , . / skip skip"],
                "thumbs": ["space"]
            },
            "fingermap": ["0 1 2 3 3 6 6 7 8 9 9 9", "0 1 2 3 3 6 6 7 8 9 9 9", "0 1 2 3 3 6 6 7 8 9 9 9"],
            "board": {"isColumnStaggered": true, "rowOrColumnStagger": [0,0,0,0,0,0,0,0,0,0,0.2,0.3]}
        }"#;
        let layout = parse(text);
        assert!(!layout.extended());
        assert_eq!(layout.slots[key(&layout, "q")].col, 0);
        assert_eq!(layout.slots[key(&layout, "a")].col, 0);
        let last = &layout.slots[key(&layout, "]")];
        assert_eq!((last.col, last.column_offset, last.finger), (11, 300, 7));
    }

    #[test]
    fn unsupported_features_and_non_prefix_rules_fail_clearly() {
        let base = example("@", "", row_board());
        for text in [
            base.replace("\"layers\": null", "\"layers\": {\"one\":{}}"),
            base.replace("\"magicKeys\": []", "\"magicKeys\": [[0,0]]"),
            base.replace("\"splitAngle\":0", "\"splitAngle\":15"),
            base.replace(
                "\"mirrorLeftRowStagger\":false",
                "\"mirrorLeftRowStagger\":true",
            ),
            base.replace("0.25", "0.2501"),
            example("@", r#"{"inputs":"q@","output":"uy"}"#, row_board()),
            example("@", r#"{"inputs":"q@","output":"qzz"}"#, row_board()),
        ] {
            assert!(Layout::parse(&text, Path::new("test")).is_err());
        }
    }

    #[test]
    fn unknown_top_level_fields_fail_clearly() {
        let text = example("@", "", row_board()).replacen(
            "\"layers\": null,",
            "\"layers\": null, \"unknown\": {},",
            1,
        );
        let error = Layout::parse(&text, Path::new("unknown.jsonc")).unwrap_err();
        assert!(
            error.contains("unsupported JSON layout field \"unknown\""),
            "{error}"
        );
    }
}
