//! Read-only presentation of layout rules shared by the editor screens.

use crate::{
    action_keys::{Action as KeyAction, Basis, Binding, Emission, Layout},
    AppResult, Canvas, Json, Terminal, Weights,
};

const FINGERS: [&str; 10] = ["LP", "LR", "LM", "LI", "RI", "RM", "RR", "RP", "LT", "RT"];

fn count(noun: &str, value: usize) -> String {
    format!("{value} {noun}{}", if value == 1 { "" } else { "s" })
}

fn character(ch: u8) -> String {
    if ch == b' ' {
        "␣".into()
    } else {
        (ch as char).to_string()
    }
}

fn alt_rule(rule: &crate::alt_fingers::Rule) -> String {
    let ngram = rule
        .ngram
        .iter()
        .map(|entry| entry.map_or_else(|| "_".into(), character))
        .collect::<Vec<_>>()
        .join(" ");
    let fingers = rule
        .fingers
        .iter()
        .map(|entry| {
            entry
                .and_then(|finger| FINGERS.get(finger).copied())
                .unwrap_or("keep")
        })
        .collect::<Vec<_>>()
        .join(" · ");
    format!("{ngram}: {fingers}")
}

fn visible_text(text: &str) -> String {
    if text == " " {
        "␣".into()
    } else {
        text.replace([' ', '␠'], "␣")
    }
}

fn combo(value: &Json) -> String {
    if let Json::Object(fields) = value {
        if fields.len() == 2 {
            if let (Some(Json::Array(keys)), Some(Json::String(output))) =
                (fields.get("keys"), fields.get("output"))
            {
                let keys = keys
                    .iter()
                    .map(|key| match key {
                        Json::String(text) => Some(visible_text(text)),
                        _ => None,
                    })
                    .collect::<Option<Vec<_>>>();
                if let Some(keys) = keys {
                    return format!("{} → {}", keys.join(" + "), visible_text(output));
                }
            }
        }
    }
    crate::alt_fingers::compact_json(value)
}

fn basis_name(basis: Basis) -> &'static str {
    match basis {
        Basis::Text => "text",
        Basis::Press => "press",
        Basis::SkipPress => "skip press",
        Basis::Output => "output",
        Basis::SkipOutput => "skip output",
        Basis::Remembered => "remembered output",
    }
}

fn emission(value: &Emission) -> String {
    match value {
        Emission::Text(text) => crate::action_keys::quote(text),
        Emission::Call(name) => format!("@{name}"),
        Emission::None => "none".into(),
    }
}

fn bound_slots(layout: &Layout, name: &str) -> String {
    let slots = layout
        .slots
        .iter()
        .enumerate()
        .filter_map(|(slot_id, slot)| match &slot.binding {
            Binding::Named(bound) if bound == name => Some(format!("{} #{slot_id}", slot.label)),
            _ => None,
        })
        .collect::<Vec<_>>();
    if slots.is_empty() {
        String::new()
    } else {
        format!(" [{}]", slots.join(", "))
    }
}

fn action_lines(layout: &Layout, name: &str, action: &KeyAction, out: &mut Vec<String>) {
    let bound = bound_slots(layout, name);
    match action {
        KeyAction::Text(text) => out.push(format!(
            "{name}{bound}: text {}",
            crate::action_keys::quote(text)
        )),
        KeyAction::RepeatOutput => out.push(format!("{name}{bound}: repeat output")),
        KeyAction::RepeatPreviousOutput => {
            out.push(format!("{name}{bound}: repeat previous output"))
        }
        KeyAction::RepeatAction => out.push(format!("{name}{bound}: repeat action")),
        KeyAction::Inactive => out.push(format!("{name}{bound}: inactive")),
        KeyAction::Rules {
            basis,
            rules,
            fallback,
        } => {
            out.push(format!(
                "{name}{bound}: rules by {} · {} · fallback {}",
                basis_name(*basis),
                count("rule", rules.len()),
                emission(fallback),
            ));
            for (input, output) in rules {
                out.push(format!(
                    "  {} → {}",
                    crate::action_keys::quote(input),
                    emission(output)
                ));
            }
        }
    }
}

pub(crate) fn lines(layout: &Layout, weights: &Weights) -> Vec<String> {
    let mut out = vec![format!("Layout: {}", layout.name), String::new()];

    out.push(format!(
        "Alternate fingers: {} · {}",
        weights.alt_fingers_label(),
        count("rule", layout.alt_fingers.len())
    ));
    if layout.alt_fingers.is_empty() {
        out.push("  No alternate-finger rules are defined.".into());
    } else {
        out.push("  _ matches one character; keep uses the normal fingermap finger.".into());
        out.extend(
            layout
                .alt_fingers
                .iter()
                .map(alt_rule)
                .map(|line| format!("  {line}")),
        );
    }

    out.push(String::new());
    out.push(format!(
        "Combos: {} · {}",
        if weights.4 { "enabled" } else { "disabled" },
        count("definition", layout.combos.len())
    ));
    if layout.combos.is_empty() {
        out.push("  No combos are defined.".into());
    } else {
        out.push(
            "  One chord press; all keys add usage, and touching movement is excluded.".into(),
        );
        out.extend(
            layout
                .combos
                .iter()
                .map(combo)
                .map(|line| format!("  {line}")),
        );
    }

    out.push(String::new());
    out.push(format!(
        "Actions / magic: {}",
        count("definition", layout.actions.len())
    ));
    if layout.actions.is_empty() {
        out.push("  No actions are defined.".into());
    } else {
        for (name, action) in &layout.actions {
            action_lines(layout, name, action, &mut out);
        }
    }
    out
}

pub(crate) fn show(
    term: &mut Terminal,
    frame: &Canvas,
    layout: &Layout,
    weights: &Weights,
) -> AppResult<()> {
    crate::help_popup(term, frame, "Layout rules", &lines(layout, weights))
}
