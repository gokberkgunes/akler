//! Stage 1 bridge from existing physical slots to experimental atomic metrics.
//! No corpus, scoring, optimizer, or action-resolution work happens here.

use crate::action_keys;
use crate::atomic_metrics::{Finger, Hand, PairClassifications, PairGeometry, PhysicalKey};
use crate::{
    bit, display_symbol, pair_flags, tri_flags_with_settings, Board, Key, RollSettings, DSB, FSB,
    HSB, LSB,
};

pub(crate) struct AtomicKeyboard {
    pub(crate) keys: Vec<PhysicalKey>,
    pub(crate) pairs: PairClassifications,
    metric_keys: Vec<Key>,
}

impl AtomicKeyboard {
    /// Reuse the evaluator's exact flags for the selected physical slots.
    pub(crate) fn metric_bits(
        &self,
        slots: &[u32],
        skip: bool,
        rolls: RollSettings,
    ) -> Result<u64, String> {
        let keys = slots
            .iter()
            .map(|&slot| {
                self.metric_keys
                    .get(slot as usize)
                    .copied()
                    .ok_or_else(|| format!("unknown physical slot {slot}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        match keys.as_slice() {
            [a, b] => {
                let flags = pair_flags(*a, *b, slots[0] == slots[1]);
                Ok(if skip { flags.sk } else { flags.bi })
            }
            [a, b, c] if !skip => Ok(tri_flags_with_settings(*a, *b, *c, rolls).bits),
            _ => Err("atomic stats require an adjacent pair, skip pair, or triple".into()),
        }
    }
}

fn finger(id: usize) -> Result<Finger, String> {
    match id {
        0 => Ok(Finger::LeftPinky),
        1 => Ok(Finger::LeftRing),
        2 => Ok(Finger::LeftMiddle),
        3 => Ok(Finger::LeftIndex),
        4 => Ok(Finger::RightIndex),
        5 => Ok(Finger::RightMiddle),
        6 => Ok(Finger::RightRing),
        7 => Ok(Finger::RightPinky),
        8 => Ok(Finger::LeftThumb),
        9 => Ok(Finger::RightThumb),
        _ => Err(format!("unknown physical finger {id}")),
    }
}

fn physical_key(index: usize, key: Key, label: String) -> Result<PhysicalKey, String> {
    let slot_id = u32::try_from(index).map_err(|_| "too many physical slots")?;
    let finger = finger(key.finger)?;
    let hand = match key.hand {
        0 => Hand::Left,
        1 => Hand::Right,
        other => return Err(format!("physical slot {index} has unknown hand {other}")),
    };
    if finger.hand() != hand || finger.is_thumb() == key.main {
        return Err(format!(
            "physical slot {index} has inconsistent finger/hand geometry"
        ));
    }
    Ok(PhysicalKey {
        slot_id,
        finger,
        hand,
        row: i32::from(key.row),
        column: i32::from(key.col),
        // These are exactly the model's coordinate components, including
        // row stagger and column stagger. They do not depend on key labels.
        x: f64::from(key.col) + f64::from(key.row_offset) / 1000.0,
        y: f64::from(key.row) + f64::from(key.column_offset) / 1000.0,
        label: Some(label),
    })
}

fn adapt(keys: &[Key], labels: impl IntoIterator<Item = String>) -> Result<AtomicKeyboard, String> {
    let labels: Vec<_> = labels.into_iter().collect();
    if keys.len() != labels.len() {
        return Err("physical slots and display labels have different lengths".into());
    }
    let physical = keys
        .iter()
        .copied()
        .zip(labels)
        .enumerate()
        .map(|(index, (key, label))| physical_key(index, key, label))
        .collect::<Result<Vec<_>, _>>()?;
    let mut pairs = PairClassifications::default();
    for (from, &a) in keys.iter().enumerate() {
        for (to, &b) in keys.iter().enumerate() {
            let flags = pair_flags(a, b, from == to).bi;
            // The existing pair flags are the source of truth. HSB is its
            // discordant half-scissor; FSB is its full-scissor aggregate.
            // LSB and DSB are respectively its lateral and discordant
            // diagonal stretch flags. No threshold is reproduced here.
            pairs.insert(
                physical[from].slot_id,
                physical[to].slot_id,
                PairGeometry {
                    scissor: Some(flags & (bit(HSB) | bit(FSB)) != 0),
                    lateral_stretch: Some(flags & bit(LSB) != 0),
                    diagonal_stretch: Some(flags & bit(DSB) != 0),
                },
            );
        }
    }
    Ok(AtomicKeyboard {
        keys: physical,
        pairs,
        metric_keys: keys.to_vec(),
    })
}

/// Preserve ordinary board slot indices, including blank slots and thumbs.
pub(crate) fn from_board(board: &Board) -> Result<AtomicKeyboard, String> {
    adapt(
        &board.keys,
        board
            .symbols
            .iter()
            .map(|&symbol| display_symbol(symbol).to_string()),
    )
}

/// Preserve action slot indices and physical geometry regardless of bindings.
/// This deliberately reads each slot's label, not its emitted text or actions.
pub(crate) fn from_action_layout(layout: &action_keys::Layout) -> Result<AtomicKeyboard, String> {
    let keys = crate::action_ui::physical_keys(layout);
    adapt(&keys, layout.slots.iter().map(|slot| slot.label.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atomic_metrics::{classify, Query, SelectedPress};
    use crate::{board_from_text, is_diagonal_stretch, is_lateral_stretch, scissor_kind};
    use std::path::Path;

    const ROWS: &str = "q w e r t | y u i o p\na s d f g | h j k l ;\nz x c v b | n m , . /\n";

    fn board(extra: &str) -> Board {
        board_from_text(&format!("{ROWS}{extra}"), Path::new("fixture.dat")).unwrap()
    }

    fn action(extra: &str) -> action_keys::Layout {
        let rows = ROWS.replacen("q w", "@magic w", 1);
        action_keys::Layout::parse(
            &format!("{rows}thumbs: space | char:?\naction magic = magic\nfallback magic = repeat-output\n{extra}"),
            Path::new("fixture-magic.dat"),
        )
        .unwrap()
    }

    fn press(id: usize, position: usize) -> SelectedPress {
        SelectedPress {
            slot_id: id as u32,
            original_position: position,
        }
    }

    fn check(atomic: &AtomicKeyboard, a: usize, b: usize, query: &str) -> bool {
        let pattern = classify(
            &atomic.keys,
            &[press(a, 0), press(b, 1)],
            Some(&atomic.pairs),
        )
        .unwrap();
        Query::parse(query).unwrap().matches(&pattern).unwrap()
    }

    #[test]
    fn ordinary_slots_keep_ids_coordinates_labels_and_both_thumbs() {
        let source = board("thumbs: space | char:?\n");
        let atomic = from_board(&source).unwrap();
        assert_eq!(atomic.keys.len(), source.keys.len());
        for (index, key) in atomic.keys.iter().enumerate() {
            let original = source.keys[index];
            assert_eq!(key.slot_id, index as u32);
            assert_eq!(key.row, i32::from(original.row));
            assert_eq!(key.column, i32::from(original.col));
            assert_eq!(
                key.x,
                f64::from(original.col) + f64::from(original.row_offset) / 1000.0
            );
            assert_eq!(
                key.y,
                f64::from(original.row) + f64::from(original.column_offset) / 1000.0
            );
            let expected_label = display_symbol(source.symbols[index]).to_string();
            assert_eq!(key.label.as_deref(), Some(expected_label.as_str()));
        }
        let left = &atomic.keys[30];
        let right = &atomic.keys[31];
        assert_eq!(
            (left.finger, left.hand, left.row),
            (Finger::LeftThumb, Hand::Left, 3)
        );
        assert_eq!(
            (right.finger, right.hand, right.row),
            (Finger::RightThumb, Hand::Right, 3)
        );
        assert!(check(
            &atomic,
            30,
            31,
            "contains.thumb = true and hand.pattern = LR and first.direction = thumb_involving"
        ));
    }

    #[test]
    fn custom_fingermap_and_stagger_come_from_parsed_physical_keys() {
        let map = "fingermap: LR LR LM LI LI RI RI RM RR RP / LP LR LM LI LI RI RI RM RR RP / LP LR LM LI LI RI RI RM RR RP\n";
        let source = board(&format!("{map}row-offsets: 0 0.25 0.75\ncolumn-offsets: 0 0.1 0.2 0.3 0.4 0.5 0.6 0.7 0.8 0.9\nthumbs: space\n"));
        let atomic = from_board(&source).unwrap();
        assert_eq!(atomic.keys[0].finger, Finger::LeftRing);
        assert_eq!(atomic.keys[1].finger, Finger::LeftRing);
        assert!(check(
            &atomic,
            0,
            1,
            "first.same_finger = true and roll.direction = none"
        ));
        assert_eq!(atomic.keys[10].row, 1);
        assert_eq!(atomic.keys[10].x, 0.25);
        assert_eq!(atomic.keys[11].y, 1.1);
        assert!(check(
            &atomic,
            0,
            10,
            "row.direction = descending and row.delta[0] = 1"
        ));
        assert!(check(&atomic, 0, 1, "row.direction = level"));
        assert_ne!(atomic.keys[0].y, atomic.keys[1].y);
    }

    #[test]
    fn geometry_mappings_are_exact_existing_pair_flags() {
        let source = board("thumbs: space | char:?\n");
        let atomic = from_board(&source).unwrap();
        // q->s: existing discordant half-scissor. a->t: lateral stretch.
        // q->v: existing discordant diagonal stretch. Thumb and cross-hand
        // pairs have known false values rather than missing classifications.
        assert_eq!(scissor_kind(source.keys[0], source.keys[11]), 1);
        assert!(check(
            &atomic,
            0,
            11,
            "first.scissor = true and first.direction = inward"
        ));
        assert!(is_lateral_stretch(source.keys[10], source.keys[4]));
        assert!(check(&atomic, 10, 4, "first.lateral_stretch = true"));
        assert!(is_diagonal_stretch(source.keys[0], source.keys[23]));
        assert!(check(&atomic, 0, 23, "first.diagonal_stretch = true"));
        assert!(check(&atomic, 0, 9, "first.scissor = false and first.lateral_stretch = false and first.diagonal_stretch = false"));
        assert!(check(&atomic, 30, 31, "first.scissor = false and first.lateral_stretch = false and first.diagonal_stretch = false"));
    }

    #[test]
    fn magic_slots_keep_geometry_through_moved_bindings_and_duplicate_labels() {
        let mut source = action("row-offsets: 0 0.25 0.75\n");
        let before = from_action_layout(&source).unwrap();
        assert_eq!(before.keys[0].label.as_deref(), Some("@magic"));
        assert_eq!(
            (before.keys[0].finger, before.keys[0].hand),
            (Finger::LeftPinky, Hand::Left)
        );
        assert_eq!(before.keys[10].x, 0.25);
        assert_eq!(before.keys[30].finger, Finger::LeftThumb);
        assert_eq!(before.keys[31].finger, Finger::RightThumb);
        source.swap(0, 9);
        let moved = from_action_layout(&source).unwrap();
        assert_eq!(moved.keys[9].label.as_deref(), Some("@magic"));
        assert_eq!(
            (moved.keys[9].finger, moved.keys[9].hand, moved.keys[9].x),
            (Finger::RightPinky, Hand::Right, 9.0)
        );
        assert_eq!(moved.keys[0].label.as_deref(), Some("p"));
        for (a, b) in before.keys.iter().zip(&moved.keys) {
            assert_eq!(
                (a.slot_id, a.finger, a.hand, a.row, a.column, a.x, a.y),
                (b.slot_id, b.finger, b.hand, b.row, b.column, b.x, b.y)
            );
        }
        assert!(check(
            &moved,
            9,
            8,
            "start.key = 9 and start.finger = right_pinky and first.direction = inward"
        ));
        // Labels are presentation metadata: duplicate them without merging slots.
        source.slots[0].label = "duplicate".into();
        source.slots[9].label = "duplicate".into();
        let duplicate = from_action_layout(&source).unwrap();
        assert_eq!(duplicate.keys[0].label, duplicate.keys[9].label);
        assert_ne!(duplicate.keys[0].slot_id, duplicate.keys[9].slot_id);
        assert!(check(
            &duplicate,
            0,
            9,
            "start.key = 0 and end.key = 9 and hand.pattern = LR"
        ));
    }

    #[test]
    fn jsonc_magic_layout_uses_its_physical_fingermap_and_stagger() {
        let text = r#"{
            "layout": {
                "fingers": [
                    "@ w e r t y u i o p",
                    "a s d f g h j k l ;",
                    "z x c v b n m , . /"
                ],
                "thumbs": ["space", ""]
            },
            "fingermap": [
                "1 1 2 3 3 6 6 7 8 9",
                "0 1 2 3 3 6 6 7 8 9",
                "0 1 2 3 3 6 6 7 8 9"
            ],
            "board": {
                "isRowStaggered": true,
                "rowOrColumnStagger": [0, 0.25, 0.75]
            },
            "magic": {
                "keys": "@",
                "wildcards": "*",
                "rules": [{"inputs": "*@", "output": "**"}]
            }
        }"#;
        let layout = action_keys::Layout::parse(text, Path::new("fixture.jsonc")).unwrap();
        let atomic = from_action_layout(&layout).unwrap();
        assert_eq!(atomic.keys[0].label.as_deref(), Some("@"));
        assert_eq!(atomic.keys[0].finger, Finger::LeftRing);
        assert_eq!(atomic.keys[10].x, 0.25);
        assert_eq!(atomic.keys[20].x, 0.75);
        assert!(check(&atomic, 0, 1, "first.same_finger = true"));
    }
}
