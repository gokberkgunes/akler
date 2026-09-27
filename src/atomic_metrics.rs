//! Experimental, standalone classification of selected physical key presses.
//!
//! These names are definitions for this module, not universal keyboard terminology.
//! Classification uses physical slot IDs, never labels. Input is already resolved to
//! physical presses: an action or magic winner supplies its root slot once, and
//! nested action calls supply no additional press. The caller chooses two or three
//! presses and supplies their positions in the original sequence.
//!
//! Query fields and types:
//! * `length`: integer (2 or 3).
//! * `start.*`, `end.*`, `position[N].*`: `key` (integer slot ID), `finger`
//!   (`left_pinky`, `left_ring`, `left_middle`, `left_index`, `left_thumb`,
//!   and corresponding `right_*`), `finger_type` (`pinky`, `ring`, `middle`,
//!   `index`, or `thumb`), `hand` (`left`/`right`), `row` (integer or
//!   `top`/`home`/`bottom`, meaning 0/1/2), `column` (integer), and `original`
//!   (integer original sequence position). `N` is 0, 1, or 2.
//! * `gap[N]`: integer omitted presses between selected positions; N is 0 or 1.
//! * `contains.thumb`, `hand.same`, `hand.alternating`, `redirect`, and
//!   `first.same_key`, `first.same_finger`, `last.same_key`,
//!   `last.same_finger`, `endpoints.same_key`, `endpoints.same_finger`: boolean.
//! * `hand.pattern`: `LL`, `LR`, `RL`, `RR`, or a three-letter L/R pattern.
//! * `row.direction`: `ascending`, `descending`, `level`, or `mixed`.
//!   `row.transitions`, `row.total_steps`, `row.net_delta`, and `row.delta[N]`
//!   are integers. N is 0 or 1; deltas follow selected-press order.
//! * `first.direction`, `last.direction`, `endpoints.direction`: `inward`,
//!   `outward`, `same_finger`, `cross_hand`, or `thumb_involving`.
//! * `roll.direction`: `inward`, `outward`, or `none` for the full sequence.
//! * `first.scissor`, `first.lateral_stretch`, `first.diagonal_stretch`,
//!   and corresponding `last.*` and `endpoints.*`: externally supplied booleans.
//!
//! Grammar: `field (= | !=) value (and field (= | !=) value)*`. Values are
//! integers, booleans, enum words, or quoted enum strings. No precedence or OR.
//! Compile with [`Query::parse`], then call [`Query::matches`] repeatedly.
//! A field applying only to triples (including position[2], gap[1], and
//! `last.*`) returns false on a pair for either operator. A missing external
//! scissor/stretch classification returns an unsupported-attribute error, never
//! false. `hand.pattern` is raw hand order and applies no ergonomic exclusions.

use std::collections::{HashMap, HashSet};
use std::fmt;

pub type SlotId = u32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Hand {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Finger {
    LeftPinky,
    LeftRing,
    LeftMiddle,
    LeftIndex,
    LeftThumb,
    RightPinky,
    RightRing,
    RightMiddle,
    RightIndex,
    RightThumb,
}

/// Hand-independent anatomical finger identity. This is explicit rather than
/// derived from the declaration or numeric order of [`Finger`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FingerType {
    Pinky,
    Ring,
    Middle,
    Index,
    Thumb,
}

impl Finger {
    pub fn hand(self) -> Hand {
        match self {
            Self::LeftPinky
            | Self::LeftRing
            | Self::LeftMiddle
            | Self::LeftIndex
            | Self::LeftThumb => Hand::Left,
            Self::RightPinky
            | Self::RightRing
            | Self::RightMiddle
            | Self::RightIndex
            | Self::RightThumb => Hand::Right,
        }
    }

    pub fn is_thumb(self) -> bool {
        matches!(self, Self::LeftThumb | Self::RightThumb)
    }

    pub fn finger_type(self) -> FingerType {
        match self {
            Self::LeftPinky | Self::RightPinky => FingerType::Pinky,
            Self::LeftRing | Self::RightRing => FingerType::Ring,
            Self::LeftMiddle | Self::RightMiddle => FingerType::Middle,
            Self::LeftIndex | Self::RightIndex => FingerType::Index,
            Self::LeftThumb | Self::RightThumb => FingerType::Thumb,
        }
    }

    // Explicit anatomical order. Enum declaration order is irrelevant.
    fn inward_rank(self) -> Option<u8> {
        match self {
            Self::LeftPinky | Self::RightPinky => Some(0),
            Self::LeftRing | Self::RightRing => Some(1),
            Self::LeftMiddle | Self::RightMiddle => Some(2),
            Self::LeftIndex | Self::RightIndex => Some(3),
            Self::LeftThumb | Self::RightThumb => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PhysicalKey {
    pub slot_id: SlotId,
    pub finger: Finger,
    pub hand: Hand,
    pub row: i32,
    pub column: i32,
    pub x: f64,
    pub y: f64,
    pub label: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectedPress {
    pub slot_id: SlotId,
    pub original_position: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PairGeometry {
    /// `None` means this attribute was not supplied, not `false`.
    pub scissor: Option<bool>,
    pub lateral_stretch: Option<bool>,
    pub diagonal_stretch: Option<bool>,
}

/// Optional, ordered classifications supplied by an external geometry policy.
/// The existing application's geometry is private and tied to its model, so
/// this module does not copy its thresholds. Supply all queried pairs here.
#[derive(Clone, Debug, Default)]
pub struct PairClassifications {
    pairs: HashMap<(SlotId, SlotId), PairGeometry>,
}

impl PairClassifications {
    pub fn insert(&mut self, from: SlotId, to: SlotId, geometry: PairGeometry) {
        self.pairs.insert((from, to), geometry);
    }

    fn get(&self, from: SlotId, to: SlotId) -> Option<PairGeometry> {
        self.pairs.get(&(from, to)).copied()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClassifyError {
    InvalidLength(usize),
    DuplicateSlot(SlotId),
    MissingSlot(SlotId),
    FingerHandMismatch(SlotId),
    NonFiniteCoordinate(SlotId),
    InvalidSourcePositions,
    ArithmeticOverflow,
}

impl fmt::Display for ClassifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for ClassifyError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FingerDirection {
    Inward,
    Outward,
    SameFinger,
    CrossHand,
    ThumbInvolving,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RollDirection {
    Inward,
    Outward,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RowDirection {
    Ascending,
    Descending,
    Level,
    Mixed,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PairRelationship {
    pub same_key: bool,
    pub same_finger: bool,
    pub direction: FingerDirection,
    pub physical_distance: f64,
    pub geometry: Option<PairGeometry>,
}

// f64 does not implement Eq; a relationship is a measured classification.
#[derive(Clone, Debug, PartialEq)]
pub struct Pattern {
    pub presses: Vec<SelectedPress>,
    pub keys: Vec<PhysicalKey>,
    pub row_deltas: Vec<i32>,
    pub row_direction: RowDirection,
    pub row_transitions: u8,
    pub row_total_steps: u32,
    pub row_net_delta: i32,
    pub first: PairRelationship,
    pub last: Option<PairRelationship>,
    pub endpoints: PairRelationship,
    pub roll_direction: RollDirection,
    pub redirect: bool,
    pub hand_pattern: String,
    pub hand_same: bool,
    pub hand_alternating: bool,
    pub contains_thumb: bool,
}

fn pair(
    a: &PhysicalKey,
    b: &PhysicalKey,
    supplied: Option<&PairClassifications>,
) -> PairRelationship {
    let direction = if a.finger.is_thumb() || b.finger.is_thumb() {
        FingerDirection::ThumbInvolving
    } else if a.hand != b.hand {
        FingerDirection::CrossHand
    } else if a.finger == b.finger {
        FingerDirection::SameFinger
    } else if a.finger.inward_rank() < b.finger.inward_rank() {
        FingerDirection::Inward
    } else {
        FingerDirection::Outward
    };
    PairRelationship {
        same_key: a.slot_id == b.slot_id,
        same_finger: a.finger == b.finger,
        direction,
        physical_distance: (b.x - a.x).hypot(b.y - a.y),
        geometry: supplied.and_then(|g| g.get(a.slot_id, b.slot_id)),
    }
}

/// Classifies exactly two or three already-resolved physical presses.
pub fn classify(
    keyboard: &[PhysicalKey],
    presses: &[SelectedPress],
    geometry: Option<&PairClassifications>,
) -> Result<Pattern, ClassifyError> {
    if !matches!(presses.len(), 2 | 3) {
        return Err(ClassifyError::InvalidLength(presses.len()));
    }
    if presses
        .windows(2)
        .any(|w| w[0].original_position >= w[1].original_position)
    {
        return Err(ClassifyError::InvalidSourcePositions);
    }
    if presses
        .iter()
        .any(|p| i64::try_from(p.original_position).is_err())
    {
        return Err(ClassifyError::ArithmeticOverflow);
    }
    let mut known = HashMap::new();
    let mut ids = HashSet::new();
    for key in keyboard {
        if !ids.insert(key.slot_id) {
            return Err(ClassifyError::DuplicateSlot(key.slot_id));
        }
        if key.finger.hand() != key.hand {
            return Err(ClassifyError::FingerHandMismatch(key.slot_id));
        }
        if !key.x.is_finite() || !key.y.is_finite() {
            return Err(ClassifyError::NonFiniteCoordinate(key.slot_id));
        }
        known.insert(key.slot_id, key);
    }
    let keys: Vec<PhysicalKey> = presses
        .iter()
        .map(|p| {
            known
                .get(&p.slot_id)
                .map(|k| (*k).clone())
                .ok_or(ClassifyError::MissingSlot(p.slot_id))
        })
        .collect::<Result<_, _>>()?;
    let mut row_deltas = Vec::with_capacity(keys.len() - 1);
    let mut row_total_steps = 0_u32;
    for w in keys.windows(2) {
        let delta = w[1]
            .row
            .checked_sub(w[0].row)
            .ok_or(ClassifyError::ArithmeticOverflow)?;
        row_total_steps = row_total_steps
            .checked_add(delta.unsigned_abs())
            .ok_or(ClassifyError::ArithmeticOverflow)?;
        row_deltas.push(delta);
    }
    let row_net_delta = keys[keys.len() - 1]
        .row
        .checked_sub(keys[0].row)
        .ok_or(ClassifyError::ArithmeticOverflow)?;
    let positive = row_deltas.iter().any(|&d| d > 0);
    let negative = row_deltas.iter().any(|&d| d < 0);
    let row_direction = match (positive, negative) {
        (true, true) => RowDirection::Mixed,
        (true, false) => RowDirection::Descending,
        (false, true) => RowDirection::Ascending,
        (false, false) => RowDirection::Level,
    };
    let first = pair(&keys[0], &keys[1], geometry);
    let last = (keys.len() == 3).then(|| pair(&keys[1], &keys[2], geometry));
    let endpoints = pair(&keys[0], &keys[keys.len() - 1], geometry);
    let roll_direction = match (first.direction, last.map(|p| p.direction)) {
        (FingerDirection::Inward, None | Some(FingerDirection::Inward)) => RollDirection::Inward,
        (FingerDirection::Outward, None | Some(FingerDirection::Outward)) => RollDirection::Outward,
        _ => RollDirection::None,
    };
    let redirect = matches!(
        (first.direction, last.map(|p| p.direction)),
        (FingerDirection::Inward, Some(FingerDirection::Outward))
            | (FingerDirection::Outward, Some(FingerDirection::Inward))
    ) && keys
        .iter()
        .all(|k| k.hand == keys[0].hand && !k.finger.is_thumb());
    let hand_pattern: String = keys
        .iter()
        .map(|k| if k.hand == Hand::Left { 'L' } else { 'R' })
        .collect();
    Ok(Pattern {
        presses: presses.to_vec(),
        keys,
        row_transitions: row_deltas.iter().filter(|&&d| d != 0).count() as u8,
        row_total_steps,
        row_net_delta,
        row_deltas,
        row_direction,
        first,
        last,
        endpoints,
        roll_direction,
        redirect,
        hand_pattern,
        hand_same: presses
            .iter()
            .all(|p| known[&p.slot_id].hand == known[&presses[0].slot_id].hand),
        hand_alternating: presses
            .windows(2)
            .all(|w| known[&w[0].slot_id].hand != known[&w[1].slot_id].hand),
        contains_thumb: presses.iter().any(|p| known[&p.slot_id].finger.is_thumb()),
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueryErrorKind {
    Syntax(String),
    UnsupportedAttribute(String),
    InvalidValue { field: String, value: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryError {
    /// Byte offset in the query source.
    pub position: usize,
    pub kind: QueryErrorKind,
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "query byte {}: {:?}", self.position, self.kind)
    }
}

impl std::error::Error for QueryError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Location {
    Start,
    End,
    Position(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LocationAttribute {
    Key,
    Finger,
    FingerType,
    Hand,
    Row,
    Column,
    Original,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PairLocation {
    First,
    Last,
    Endpoints,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PairAttribute {
    SameKey,
    SameFinger,
    Direction,
    Scissor,
    LateralStretch,
    DiagonalStretch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    Length,
    Location(Location, LocationAttribute),
    Gap(usize),
    ContainsThumb,
    HandSame,
    HandAlternating,
    HandPattern,
    Redirect,
    RowDirection,
    RowTransitions,
    RowTotalSteps,
    RowNetDelta,
    RowDelta(usize),
    Pair(PairLocation, PairAttribute),
    RollDirection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ValueType {
    Integer,
    Boolean,
    Finger,
    FingerType,
    Hand,
    Row,
    HandPattern,
    RowDirection,
    FingerDirection,
    RollDirection,
}

impl Field {
    fn value_type(self) -> ValueType {
        match self {
            Self::Length
            | Self::Gap(_)
            | Self::RowTransitions
            | Self::RowTotalSteps
            | Self::RowNetDelta
            | Self::RowDelta(_)
            | Self::Location(
                _,
                LocationAttribute::Key | LocationAttribute::Column | LocationAttribute::Original,
            ) => ValueType::Integer,
            Self::Location(_, LocationAttribute::Row) => ValueType::Row,
            Self::Location(_, LocationAttribute::Finger) => ValueType::Finger,
            Self::Location(_, LocationAttribute::FingerType) => ValueType::FingerType,
            Self::Location(_, LocationAttribute::Hand) => ValueType::Hand,
            Self::HandPattern => ValueType::HandPattern,
            Self::RowDirection => ValueType::RowDirection,
            Self::Pair(_, PairAttribute::Direction) => ValueType::FingerDirection,
            Self::RollDirection => ValueType::RollDirection,
            _ => ValueType::Boolean,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum AttributeValue {
    Integer(i64),
    Boolean(bool),
    Finger(Finger),
    FingerType(FingerType),
    Hand(Hand),
    Pattern(String),
    RowDirection(RowDirection),
    FingerDirection(FingerDirection),
    RollDirection(RollDirection),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Operator {
    Equal,
    NotEqual,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Predicate {
    field: Field,
    field_name: String,
    position: usize,
    operator: Operator,
    value: AttributeValue,
}

/// A compiled classifier field that can be reused for grouping or display.
/// It delegates to the same field lookup used by [`Query`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attribute {
    field: Field,
    name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    predicates: Vec<Predicate>,
}

fn bracket_index(text: &str, prefix: &str, suffix: &str, max: usize) -> Option<usize> {
    text.strip_prefix(prefix)?
        .strip_suffix(suffix)?
        .parse::<usize>()
        .ok()
        .filter(|&i| i <= max)
}

fn parse_field(name: &str, position: usize) -> Result<Field, QueryError> {
    let field = match name {
        "length" => Field::Length,
        "contains.thumb" => Field::ContainsThumb,
        "hand.same" => Field::HandSame,
        "hand.alternating" => Field::HandAlternating,
        "hand.pattern" => Field::HandPattern,
        "redirect" => Field::Redirect,
        "row.direction" => Field::RowDirection,
        "row.transitions" => Field::RowTransitions,
        "row.total_steps" => Field::RowTotalSteps,
        "row.net_delta" => Field::RowNetDelta,
        "roll.direction" => Field::RollDirection,
        _ => {
            if let Some(i) = bracket_index(name, "gap[", "]", 1) {
                return Ok(Field::Gap(i));
            }
            if let Some(i) = bracket_index(name, "row.delta[", "]", 1) {
                return Ok(Field::RowDelta(i));
            }
            let mut pieces = name.splitn(2, '.');
            let head = pieces.next().unwrap_or("");
            let tail = pieces.next().unwrap_or("");
            let loc = match head {
                "start" => Some(Location::Start),
                "end" => Some(Location::End),
                _ => bracket_index(head, "position[", "]", 2).map(Location::Position),
            };
            if let Some(loc) = loc {
                let attr = match tail {
                    "key" => LocationAttribute::Key,
                    "finger" => LocationAttribute::Finger,
                    "finger_type" => LocationAttribute::FingerType,
                    "hand" => LocationAttribute::Hand,
                    "row" => LocationAttribute::Row,
                    "column" => LocationAttribute::Column,
                    "original" => LocationAttribute::Original,
                    _ => return Err(unsupported(name, position)),
                };
                return Ok(Field::Location(loc, attr));
            }
            let pair = match head {
                "first" => Some(PairLocation::First),
                "last" => Some(PairLocation::Last),
                "endpoints" => Some(PairLocation::Endpoints),
                _ => None,
            };
            if let Some(pair) = pair {
                let attr = match tail {
                    "same_key" => PairAttribute::SameKey,
                    "same_finger" => PairAttribute::SameFinger,
                    "direction" => PairAttribute::Direction,
                    "scissor" => PairAttribute::Scissor,
                    "lateral_stretch" => PairAttribute::LateralStretch,
                    "diagonal_stretch" => PairAttribute::DiagonalStretch,
                    _ => return Err(unsupported(name, position)),
                };
                return Ok(Field::Pair(pair, attr));
            }
            return Err(unsupported(name, position));
        }
    };
    Ok(field)
}

fn unsupported(name: &str, position: usize) -> QueryError {
    QueryError {
        position,
        kind: QueryErrorKind::UnsupportedAttribute(name.to_owned()),
    }
}

fn parse_value(
    field: Field,
    name: &str,
    word: &str,
    position: usize,
) -> Result<AttributeValue, QueryError> {
    let bad = || QueryError {
        position,
        kind: QueryErrorKind::InvalidValue {
            field: name.to_owned(),
            value: word.to_owned(),
        },
    };
    let value = match field.value_type() {
        ValueType::Integer => {
            let number = word.parse::<i64>().map_err(|_| bad())?;
            if matches!(field, Field::Length) && !matches!(number, 2 | 3) {
                return Err(bad());
            }
            AttributeValue::Integer(number)
        }
        ValueType::Row => AttributeValue::Integer(match word {
            "top" => 0,
            "home" => 1,
            "bottom" => 2,
            _ => word.parse::<i64>().map_err(|_| bad())?,
        }),
        ValueType::Boolean => AttributeValue::Boolean(match word {
            "true" => true,
            "false" => false,
            _ => return Err(bad()),
        }),
        ValueType::Finger => AttributeValue::Finger(match word {
            "left_pinky" => Finger::LeftPinky,
            "left_ring" => Finger::LeftRing,
            "left_middle" => Finger::LeftMiddle,
            "left_index" => Finger::LeftIndex,
            "left_thumb" => Finger::LeftThumb,
            "right_pinky" => Finger::RightPinky,
            "right_ring" => Finger::RightRing,
            "right_middle" => Finger::RightMiddle,
            "right_index" => Finger::RightIndex,
            "right_thumb" => Finger::RightThumb,
            _ => return Err(bad()),
        }),
        ValueType::FingerType => AttributeValue::FingerType(match word {
            "pinky" => FingerType::Pinky,
            "ring" => FingerType::Ring,
            "middle" => FingerType::Middle,
            "index" => FingerType::Index,
            "thumb" => FingerType::Thumb,
            _ => return Err(bad()),
        }),
        ValueType::Hand => AttributeValue::Hand(match word {
            "left" => Hand::Left,
            "right" => Hand::Right,
            _ => return Err(bad()),
        }),
        ValueType::HandPattern => {
            if !(2..=3).contains(&word.len()) || !word.bytes().all(|b| matches!(b, b'L' | b'R')) {
                return Err(bad());
            }
            AttributeValue::Pattern(word.to_owned())
        }
        ValueType::RowDirection => AttributeValue::RowDirection(match word {
            "ascending" => RowDirection::Ascending,
            "descending" => RowDirection::Descending,
            "level" => RowDirection::Level,
            "mixed" => RowDirection::Mixed,
            _ => return Err(bad()),
        }),
        ValueType::FingerDirection => AttributeValue::FingerDirection(match word {
            "inward" => FingerDirection::Inward,
            "outward" => FingerDirection::Outward,
            "same_finger" => FingerDirection::SameFinger,
            "cross_hand" => FingerDirection::CrossHand,
            "thumb_involving" => FingerDirection::ThumbInvolving,
            _ => return Err(bad()),
        }),
        ValueType::RollDirection => AttributeValue::RollDirection(match word {
            "inward" => RollDirection::Inward,
            "outward" => RollDirection::Outward,
            "none" => RollDirection::None,
            _ => return Err(bad()),
        }),
    };
    Ok(value)
}

struct Parser<'a> {
    source: &'a str,
    offset: usize,
}

impl<'a> Parser<'a> {
    fn skip_space(&mut self) {
        while self
            .source
            .as_bytes()
            .get(self.offset)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.offset += 1;
        }
    }

    fn syntax(&self, at: usize, message: &str) -> QueryError {
        QueryError {
            position: at,
            kind: QueryErrorKind::Syntax(message.to_owned()),
        }
    }

    fn field(&mut self) -> Result<(String, usize), QueryError> {
        self.skip_space();
        let start = self.offset;
        while let Some(&byte) = self.source.as_bytes().get(self.offset) {
            if byte.is_ascii_whitespace() || matches!(byte, b'=' | b'!') {
                break;
            }
            if !byte.is_ascii_alphanumeric() && !matches!(byte, b'_' | b'.' | b'[' | b']') {
                return Err(self.syntax(self.offset, "invalid field character"));
            }
            self.offset += 1;
        }
        if self.offset == start {
            return Err(self.syntax(start, "expected field"));
        }
        Ok((self.source[start..self.offset].to_owned(), start))
    }

    fn operator(&mut self) -> Result<Operator, QueryError> {
        self.skip_space();
        let at = self.offset;
        let rest = &self.source[self.offset..];
        if rest.starts_with("!=") {
            self.offset += 2;
            Ok(Operator::NotEqual)
        } else if rest.starts_with('=') {
            self.offset += 1;
            Ok(Operator::Equal)
        } else {
            Err(self.syntax(at, "expected = or !="))
        }
    }

    fn value(&mut self) -> Result<(String, usize), QueryError> {
        self.skip_space();
        let start = self.offset;
        if self.source.as_bytes().get(self.offset) == Some(&b'"') {
            self.offset += 1;
            let mut result = String::new();
            loop {
                let Some(ch) = self.source[self.offset..].chars().next() else {
                    return Err(self.syntax(start, "unterminated quoted value"));
                };
                self.offset += ch.len_utf8();
                if ch == '"' {
                    break;
                }
                if ch == '\\' {
                    let Some(escaped) = self.source[self.offset..].chars().next() else {
                        return Err(self.syntax(self.offset, "unterminated escape"));
                    };
                    if !matches!(escaped, '"' | '\\') {
                        return Err(self.syntax(self.offset, "unsupported quoted-value escape"));
                    }
                    self.offset += escaped.len_utf8();
                    result.push(escaped);
                } else {
                    result.push(ch);
                }
            }
            Ok((result, start))
        } else {
            while let Some(&byte) = self.source.as_bytes().get(self.offset) {
                if byte.is_ascii_whitespace() {
                    break;
                }
                self.offset += 1;
            }
            if self.offset == start {
                return Err(self.syntax(start, "expected value"));
            }
            Ok((self.source[start..self.offset].to_owned(), start))
        }
    }
}

impl Attribute {
    /// Compiles one documented query field without an operator or value.
    pub fn parse(name: &str) -> Result<Self, QueryError> {
        Ok(Self {
            field: parse_field(name, 0)?,
            name: name.to_owned(),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Projects this field from a classified pattern. `None` means the field
    /// is supported but does not apply to this sequence length.
    pub fn value(&self, pattern: &Pattern) -> Result<Option<AttributeValue>, QueryError> {
        field_value(pattern, self.field, &self.name, 0)
    }

    /// Formats a projected value using query spelling. Location row values are
    /// shifted to the viewer's one-based row presentation.
    pub fn display_value(&self, value: &AttributeValue) -> String {
        if matches!(self.field, Field::Location(_, LocationAttribute::Row)) {
            if let AttributeValue::Integer(row) = value {
                return (row + 1).to_string();
            }
        }
        match value {
            AttributeValue::Integer(value) => value.to_string(),
            AttributeValue::Boolean(value) => value.to_string(),
            AttributeValue::Finger(value) => match value {
                Finger::LeftPinky => "left_pinky",
                Finger::LeftRing => "left_ring",
                Finger::LeftMiddle => "left_middle",
                Finger::LeftIndex => "left_index",
                Finger::LeftThumb => "left_thumb",
                Finger::RightPinky => "right_pinky",
                Finger::RightRing => "right_ring",
                Finger::RightMiddle => "right_middle",
                Finger::RightIndex => "right_index",
                Finger::RightThumb => "right_thumb",
            }
            .to_owned(),
            AttributeValue::FingerType(value) => match value {
                FingerType::Pinky => "pinky",
                FingerType::Ring => "ring",
                FingerType::Middle => "middle",
                FingerType::Index => "index",
                FingerType::Thumb => "thumb",
            }
            .to_owned(),
            AttributeValue::Hand(value) => match value {
                Hand::Left => "left",
                Hand::Right => "right",
            }
            .to_owned(),
            AttributeValue::Pattern(value) => value.clone(),
            AttributeValue::RowDirection(value) => match value {
                RowDirection::Ascending => "ascending",
                RowDirection::Descending => "descending",
                RowDirection::Level => "level",
                RowDirection::Mixed => "mixed",
            }
            .to_owned(),
            AttributeValue::FingerDirection(value) => match value {
                FingerDirection::Inward => "inward",
                FingerDirection::Outward => "outward",
                FingerDirection::SameFinger => "same_finger",
                FingerDirection::CrossHand => "cross_hand",
                FingerDirection::ThumbInvolving => "thumb_involving",
            }
            .to_owned(),
            AttributeValue::RollDirection(value) => match value {
                RollDirection::Inward => "inward",
                RollDirection::Outward => "outward",
                RollDirection::None => "none",
            }
            .to_owned(),
        }
    }
}

impl Query {
    pub fn parse(source: &str) -> Result<Self, QueryError> {
        let mut parser = Parser { source, offset: 0 };
        let mut predicates = Vec::new();
        loop {
            let (name, position) = parser.field()?;
            let field = parse_field(&name, position)?;
            let operator = parser.operator()?;
            let (word, value_position) = parser.value()?;
            let value = parse_value(field, &name, &word, value_position)?;
            predicates.push(Predicate {
                field,
                field_name: name,
                position,
                operator,
                value,
            });
            parser.skip_space();
            if parser.offset == source.len() {
                break;
            }
            let at = parser.offset;
            let (connector, _) = parser.field()?;
            if connector != "and" {
                return Err(parser.syntax(at, "expected and"));
            }
            if parser.offset == source.len() {
                return Err(parser.syntax(parser.offset, "expected predicate after and"));
            }
        }
        Ok(Self { predicates })
    }

    pub fn matches(&self, pattern: &Pattern) -> Result<bool, QueryError> {
        // Evaluate every predicate so missing geometry is an error even if a
        // preceding ordinary predicate would already make the result false.
        let mut matches = true;
        for p in &self.predicates {
            let actual = field_value(pattern, p.field, &p.field_name, p.position)?;
            let equal = actual.as_ref().is_some_and(|v| *v == p.value);
            matches &= actual.is_some()
                && match p.operator {
                    Operator::Equal => equal,
                    Operator::NotEqual => !equal,
                };
        }
        Ok(matches)
    }
}

fn field_value(
    pattern: &Pattern,
    field: Field,
    name: &str,
    position: usize,
) -> Result<Option<AttributeValue>, QueryError> {
    let integer = |n: i64| Some(AttributeValue::Integer(n));
    let boolean = |b: bool| Some(AttributeValue::Boolean(b));
    let value = match field {
        Field::Length => integer(pattern.keys.len() as i64),
        Field::Location(location, attr) => {
            let index = match location {
                Location::Start => 0,
                Location::End => pattern.keys.len() - 1,
                Location::Position(i) => i,
            };
            let Some(key) = pattern.keys.get(index) else {
                return Ok(None);
            };
            match attr {
                LocationAttribute::Key => integer(i64::from(key.slot_id)),
                LocationAttribute::Finger => Some(AttributeValue::Finger(key.finger)),
                LocationAttribute::FingerType => {
                    Some(AttributeValue::FingerType(key.finger.finger_type()))
                }
                LocationAttribute::Hand => Some(AttributeValue::Hand(key.hand)),
                LocationAttribute::Row => integer(i64::from(key.row)),
                LocationAttribute::Column => integer(i64::from(key.column)),
                LocationAttribute::Original => {
                    i64::try_from(pattern.presses[index].original_position)
                        .ok()
                        .map(AttributeValue::Integer)
                }
            }
        }
        Field::Gap(i) => pattern
            .presses
            .get(i + 1)
            .and_then(|next| {
                next.original_position
                    .checked_sub(pattern.presses[i].original_position + 1)
            })
            .and_then(|n| i64::try_from(n).ok())
            .map(AttributeValue::Integer),
        Field::ContainsThumb => boolean(pattern.contains_thumb),
        Field::HandSame => boolean(pattern.hand_same),
        Field::HandAlternating => boolean(pattern.hand_alternating),
        Field::HandPattern => Some(AttributeValue::Pattern(pattern.hand_pattern.clone())),
        Field::Redirect => {
            (pattern.keys.len() == 3).then_some(AttributeValue::Boolean(pattern.redirect))
        }
        Field::RowDirection => Some(AttributeValue::RowDirection(pattern.row_direction)),
        Field::RowTransitions => integer(i64::from(pattern.row_transitions)),
        Field::RowTotalSteps => integer(i64::from(pattern.row_total_steps)),
        Field::RowNetDelta => integer(i64::from(pattern.row_net_delta)),
        Field::RowDelta(i) => pattern
            .row_deltas
            .get(i)
            .map(|&n| AttributeValue::Integer(i64::from(n))),
        Field::RollDirection => Some(AttributeValue::RollDirection(pattern.roll_direction)),
        Field::Pair(which, attr) => {
            let pair = match which {
                PairLocation::First => Some(pattern.first),
                PairLocation::Last => pattern.last,
                PairLocation::Endpoints => Some(pattern.endpoints),
            };
            let Some(pair) = pair else { return Ok(None) };
            match attr {
                PairAttribute::SameKey => boolean(pair.same_key),
                PairAttribute::SameFinger => boolean(pair.same_finger),
                PairAttribute::Direction => Some(AttributeValue::FingerDirection(pair.direction)),
                PairAttribute::Scissor => boolean(
                    pair.geometry
                        .and_then(|g| g.scissor)
                        .ok_or_else(|| unsupported(name, position))?,
                ),
                PairAttribute::LateralStretch => boolean(
                    pair.geometry
                        .and_then(|g| g.lateral_stretch)
                        .ok_or_else(|| unsupported(name, position))?,
                ),
                PairAttribute::DiagonalStretch => boolean(
                    pair.geometry
                        .and_then(|g| g.diagonal_stretch)
                        .ok_or_else(|| unsupported(name, position))?,
                ),
            }
        }
    };
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qwerty() -> Vec<PhysicalKey> {
        let rows = ["qwertyuiop", "asdfghjkl;", "zxcvbnm,./"];
        let mut keys = Vec::new();
        for (row, labels) in rows.iter().enumerate() {
            for (column, label) in labels.chars().enumerate() {
                let (hand, finger) = match column {
                    0 => (Hand::Left, Finger::LeftPinky),
                    1 => (Hand::Left, Finger::LeftRing),
                    2 => (Hand::Left, Finger::LeftMiddle),
                    3 | 4 => (Hand::Left, Finger::LeftIndex),
                    5 | 6 => (Hand::Right, Finger::RightIndex),
                    7 => (Hand::Right, Finger::RightMiddle),
                    8 => (Hand::Right, Finger::RightRing),
                    9 => (Hand::Right, Finger::RightPinky),
                    _ => unreachable!(),
                };
                keys.push(PhysicalKey {
                    slot_id: (row * 10 + column) as SlotId,
                    finger,
                    hand,
                    row: row as i32,
                    column: column as i32,
                    x: column as f64,
                    y: row as f64,
                    label: Some(label.to_string()),
                });
            }
        }
        keys
    }

    fn with_thumbs(mut keys: Vec<PhysicalKey>) -> Vec<PhysicalKey> {
        keys.push(PhysicalKey {
            slot_id: 30,
            finger: Finger::LeftThumb,
            hand: Hand::Left,
            row: 3,
            column: 4,
            x: 4.0,
            y: 3.0,
            label: Some("left space".into()),
        });
        keys.push(PhysicalKey {
            slot_id: 31,
            finger: Finger::RightThumb,
            hand: Hand::Right,
            row: 3,
            column: 5,
            x: 5.0,
            y: 3.0,
            label: Some("right space".into()),
        });
        keys
    }

    fn slot(keys: &[PhysicalKey], label: &str) -> SlotId {
        keys.iter()
            .find(|k| k.label.as_deref() == Some(label))
            .unwrap()
            .slot_id
    }

    fn selected(keys: &[PhysicalKey], labels: &[&str], original: &[usize]) -> Vec<SelectedPress> {
        labels
            .iter()
            .zip(original)
            .map(|(label, &original_position)| SelectedPress {
                slot_id: slot(keys, label),
                original_position,
            })
            .collect()
    }

    fn pattern(labels: &[&str]) -> Pattern {
        let keys = qwerty();
        let positions: Vec<_> = (0..labels.len()).collect();
        classify(&keys, &selected(&keys, labels, &positions), None).unwrap()
    }

    fn matches(query: &str, labels: &[&str]) -> bool {
        Query::parse(query)
            .unwrap()
            .matches(&pattern(labels))
            .unwrap()
    }

    #[test]
    fn attributes_reuse_query_fields_and_group_mirrored_finger_types() {
        let start_type = Attribute::parse("start.finger_type").unwrap();
        let end_type = Attribute::parse("end.finger_type").unwrap();
        assert_eq!(start_type.name(), "start.finger_type");

        for labels in [["q", "w"], ["p", "o"]] {
            let p = pattern(&labels);
            let start = start_type.value(&p).unwrap().unwrap();
            let end = end_type.value(&p).unwrap().unwrap();
            assert_eq!(start, AttributeValue::FingerType(FingerType::Pinky));
            assert_eq!(end, AttributeValue::FingerType(FingerType::Ring));
            assert_eq!(start_type.display_value(&start), "pinky");
            assert_eq!(end_type.display_value(&end), "ring");
            assert!(
                Query::parse("start.finger_type = pinky and end.finger_type = ring")
                    .unwrap()
                    .matches(&p)
                    .unwrap()
            );
        }

        let p = pattern(&["q", "z"]);
        let start_row = Attribute::parse("start.row").unwrap();
        let end_row = Attribute::parse("end.row").unwrap();
        assert_eq!(
            start_row.display_value(&start_row.value(&p).unwrap().unwrap()),
            "1"
        );
        assert_eq!(
            end_row.display_value(&end_row.value(&p).unwrap().unwrap()),
            "3"
        );
        let finger = Attribute::parse("start.finger").unwrap();
        assert_eq!(
            finger.display_value(&finger.value(&p).unwrap().unwrap()),
            "left_pinky"
        );
        let direction = Attribute::parse("row.direction").unwrap();
        assert_eq!(
            direction.display_value(&direction.value(&p).unwrap().unwrap()),
            "descending"
        );
    }

    #[test]
    fn attribute_errors_preserve_query_applicability_and_geometry_policy() {
        assert_eq!(
            Attribute::parse("start.unknown").unwrap_err().kind,
            QueryErrorKind::UnsupportedAttribute("start.unknown".into())
        );
        let pair = pattern(&["q", "w"]);
        assert_eq!(
            Attribute::parse("last.direction")
                .unwrap()
                .value(&pair)
                .unwrap(),
            None
        );
        assert_eq!(
            Attribute::parse("first.scissor")
                .unwrap()
                .value(&pair)
                .unwrap_err()
                .kind,
            QueryErrorKind::UnsupportedAttribute("first.scissor".into())
        );
    }

    #[test]
    fn location_order_and_broad_pair_queries() {
        let query = "length = 2 and roll.direction = inward and row.direction = descending";
        for labels in [["q", "s"], ["w", "d"]] {
            assert!(matches(query, &labels), "{labels:?}");
        }
        for labels in [["q", "w"], ["s", "q"], ["q", "a"], ["q", "p"]] {
            assert!(!matches(query, &labels), "{labels:?}");
        }
        assert!(matches("start.finger = left_pinky and end.finger = left_ring and start.row = top and end.row = home", &["q", "s"]));
        assert!(matches(
            "start.key = 0 and end.key = 11 and start.column = 0 and end.column = 1",
            &["q", "s"]
        ));
        assert!(!matches("start.key = 0", &["s", "q"]));
        assert!(matches(
            "position[1].hand = left and position[1].row = home",
            &["q", "s"]
        ));
        assert!(matches(
            "start.finger != left_ring and end.key != 0",
            &["q", "s"]
        ));
        assert!(!matches("start.row = top", &["s", "q"]));
    }

    #[test]
    fn rows_keep_direction_steps_transitions_and_net_distinct() {
        let cases: &[(&[&str], RowDirection, u8, u32, i32, &[i32])] = &[
            (&["q", "a"], RowDirection::Descending, 1, 1, 1, &[1]),
            (&["a", "q"], RowDirection::Ascending, 1, 1, -1, &[-1]),
            (&["q", "z"], RowDirection::Descending, 1, 2, 2, &[2]),
            (&["q", "w"], RowDirection::Level, 0, 0, 0, &[0]),
            (&["q", "s", "c"], RowDirection::Descending, 2, 2, 2, &[1, 1]),
            (
                &["c", "s", "q"],
                RowDirection::Ascending,
                2,
                2,
                -2,
                &[-1, -1],
            ),
            (&["q", "s", "e"], RowDirection::Mixed, 2, 2, 0, &[1, -1]),
            (&["q", "w", "s"], RowDirection::Descending, 1, 1, 1, &[0, 1]),
        ];
        for &(labels, direction, transitions, steps, net, deltas) in cases {
            let p = pattern(labels);
            assert_eq!(p.row_direction, direction, "{labels:?}");
            assert_eq!(p.row_transitions, transitions);
            assert_eq!(p.row_total_steps, steps);
            assert_eq!(p.row_net_delta, net);
            assert_eq!(p.row_deltas, deltas);
        }
        assert!(matches("length = 2 and row.transitions = 1 and row.total_steps = 2 and row.net_delta = 2 and row.delta[0] = 2", &["q", "z"]));
        assert!(matches(
            "row.direction = mixed and row.delta[1] = -1",
            &["q", "s", "e"]
        ));
    }

    #[test]
    fn anatomical_finger_direction_and_roll_length() {
        for labels in [["q", "w"], ["p", "o"], ["q", "r"], ["o", "i"]] {
            assert_eq!(pattern(&labels).first.direction, FingerDirection::Inward);
        }
        for labels in [["w", "q"], ["o", "p"]] {
            assert_eq!(pattern(&labels).first.direction, FingerDirection::Outward);
        }
        assert_eq!(
            pattern(&["r", "t"]).first.direction,
            FingerDirection::SameFinger
        );
        assert_eq!(
            pattern(&["q", "p"]).first.direction,
            FingerDirection::CrossHand
        );
        for labels in [["q", "w", "e"], ["p", "o", "i"]] {
            assert_eq!(pattern(&labels).roll_direction, RollDirection::Inward);
        }
        assert_eq!(
            pattern(&["e", "w", "q"]).roll_direction,
            RollDirection::Outward
        );
        for labels in [["q", "w", "p"], ["q", "w", "w"]] {
            assert_eq!(pattern(&labels).roll_direction, RollDirection::None);
        }
        assert!(matches(
            "first.direction = inward and last.direction = cross_hand and roll.direction = none",
            &["q", "w", "p"]
        ));
    }

    #[test]
    fn redirect_and_endpoint_relationships_overlap() {
        let query = "length = 3 and redirect = true and endpoints.same_key = false";
        for labels in [["q", "e", "w"], ["q", "w", "a"]] {
            assert!(matches(query, &labels));
        }
        for labels in [["q", "w", "q"], ["q", "w", "e"]] {
            assert!(!matches(query, &labels));
        }
        assert!(pattern(&["e", "q", "w"]).redirect);
        assert!(!pattern(&["q", "w", "p"]).redirect);
        assert!(!pattern(&["q", "a", "w"]).redirect);
        assert!(matches(
            "redirect = true and endpoints.same_key = true",
            &["q", "w", "q"]
        ));
        assert!(matches(
            "redirect = true and endpoints.same_finger = true and endpoints.same_key = false",
            &["q", "w", "a"]
        ));
        assert!(matches(
            "first.same_key = true and first.same_finger = true",
            &["q", "q"]
        ));
        assert!(matches(
            "first.same_key = false and first.same_finger = true",
            &["q", "a"]
        ));
        assert!(matches(
            "first.same_key = false and first.same_finger = false",
            &["q", "w"]
        ));
        let repeated_finger = pattern(&["q", "a", "z"]);
        assert!(
            repeated_finger.first.same_finger
                && repeated_finger.last.unwrap().same_finger
                && repeated_finger.endpoints.same_finger
        );
    }

    #[test]
    fn hand_patterns_are_raw_and_include_both_hands() {
        let hands = [
            ("LL", &["q", "w"][..]),
            ("LR", &["q", "p"]),
            ("RL", &["p", "q"]),
            ("RR", &["p", "o"]),
            ("LLL", &["q", "w", "e"]),
            ("LLR", &["q", "w", "p"]),
            ("LRL", &["q", "p", "w"]),
            ("LRR", &["q", "p", "o"]),
            ("RLL", &["p", "q", "w"]),
            ("RLR", &["p", "q", "o"]),
            ("RRL", &["p", "o", "q"]),
            ("RRR", &["p", "o", "i"]),
        ];
        for (name, labels) in hands {
            let p = pattern(labels);
            assert_eq!(p.hand_pattern, name);
            assert_eq!(p.hand_same, name.bytes().all(|b| b == name.as_bytes()[0]));
            assert_eq!(
                p.hand_alternating,
                name.as_bytes().windows(2).all(|w| w[0] != w[1])
            );
            assert!(Query::parse(&format!("hand.pattern = \"{name}\""))
                .unwrap()
                .matches(&p)
                .unwrap());
        }
        let query = "length = 3 and hand.pattern = LLR and first.direction = inward";
        assert!(matches(query, &["q", "w", "p"]));
        assert!(!matches(query, &["w", "q", "p"]));
        assert!(!matches(query, &["q", "w", "e"]));
    }

    #[test]
    fn thumbs_are_preserved_without_roll_direction() {
        let keys = with_thumbs(qwerty());
        for (labels, expected) in [
            (["q", "left space"], "left_thumb"),
            (["right space", "p"], "right_thumb"),
        ] {
            let presses = selected(&keys, &labels, &[0, 1]);
            let p = classify(&keys, &presses, None).unwrap();
            assert!(p.contains_thumb);
            assert_eq!(p.first.direction, FingerDirection::ThumbInvolving);
            assert_eq!(p.roll_direction, RollDirection::None);
            assert!(
                Query::parse("contains.thumb = true and first.direction = thumb_involving")
                    .unwrap()
                    .matches(&p)
                    .unwrap()
            );
            assert!(
                Query::parse(&format!("end.finger = {expected}"))
                    .unwrap()
                    .matches(&p)
                    .unwrap()
                    || Query::parse(&format!("start.finger = {expected}"))
                        .unwrap()
                        .matches(&p)
                        .unwrap()
            );
        }
        let p = classify(
            &keys,
            &selected(&keys, &["q", "left space", "w"], &[0, 1, 2]),
            None,
        )
        .unwrap();
        assert!(!p.redirect);
    }

    #[test]
    fn skip_positions_and_triple_position_index() {
        let keys = qwerty();
        let adjacent = classify(&keys, &selected(&keys, &["q", "a"], &[0, 1]), None).unwrap();
        let skip_one = classify(&keys, &selected(&keys, &["q", "a"], &[0, 2]), None).unwrap();
        let skip_two = classify(&keys, &selected(&keys, &["q", "a"], &[0, 3]), None).unwrap();
        let query = Query::parse("length = 2 and gap[0] = 1 and endpoints.same_finger = true and position[1].original = 2").unwrap();
        assert!(!query.matches(&adjacent).unwrap());
        assert!(query.matches(&skip_one).unwrap());
        assert!(!query.matches(&skip_two).unwrap());
        let triple = classify(&keys, &selected(&keys, &["q", "s", "c"], &[0, 2, 5]), None).unwrap();
        assert!(Query::parse("length = 3 and position[1].row = home and position[1].original = 2 and gap[0] = 1 and gap[1] = 2").unwrap().matches(&triple).unwrap());
    }

    #[test]
    fn physical_stagger_does_not_change_logical_row() {
        let plain = qwerty();
        let mut staggered = plain.clone();
        staggered[11].y += 0.75; // s is still in logical home row.
        staggered[1].y += 0.25; // w is still in logical top row.
        let pair = selected(&plain, &["q", "s"], &[0, 1]);
        let a = classify(&plain, &pair, None).unwrap();
        let b = classify(&staggered, &pair, None).unwrap();
        assert_eq!(a.row_deltas, b.row_deltas);
        assert_eq!(a.row_direction, b.row_direction);
        assert_ne!(a.first.physical_distance, b.first.physical_distance);
        let same_row = selected(&plain, &["q", "w"], &[0, 1]);
        let c = classify(&staggered, &same_row, None).unwrap();
        assert_eq!(c.row_deltas, [0]);
        assert_ne!(staggered[0].y, staggered[1].y);
    }

    #[test]
    fn resolved_action_root_is_a_single_physical_press() {
        let keys = qwerty();
        let literal = classify(&keys, &selected(&keys, &["q", "w"], &[0, 1]), None).unwrap();
        let mut renamed = keys.clone();
        renamed[1].label = Some("magic action root".into());
        let action = classify(&renamed, &literal.presses, None).unwrap();
        assert_eq!(literal.first, action.first);
        assert_eq!(literal.roll_direction, action.roll_direction);
        assert_eq!(action.presses.len(), 2);
        assert!(Query::parse("start.key = 0 and end.key = 1")
            .unwrap()
            .matches(&action)
            .unwrap());
    }

    #[test]
    fn external_geometry_is_optional_and_can_overlap_rolls() {
        let keys = qwerty();
        let presses = selected(&keys, &["q", "s", "c"], &[0, 1, 2]);
        let missing = classify(&keys, &presses, None).unwrap();
        let query = Query::parse("first.scissor = true and first.lateral_stretch = true").unwrap();
        assert_eq!(
            query.matches(&missing).unwrap_err().kind,
            QueryErrorKind::UnsupportedAttribute("first.scissor".into())
        );
        let mut supplied = PairClassifications::default();
        supplied.insert(
            slot(&keys, "q"),
            slot(&keys, "s"),
            PairGeometry {
                scissor: Some(true),
                lateral_stretch: Some(true),
                diagonal_stretch: Some(false),
            },
        );
        let p = classify(&keys, &presses, Some(&supplied)).unwrap();
        assert_eq!(p.first.direction, FingerDirection::Inward);
        assert!(query.matches(&p).unwrap());
        assert!(Query::parse("first.diagonal_stretch = false")
            .unwrap()
            .matches(&p)
            .unwrap());
        assert!(matches!(
            Query::parse("last.scissor = false").unwrap().matches(&p),
            Err(QueryError {
                kind: QueryErrorKind::UnsupportedAttribute(_),
                ..
            })
        ));
        assert!(matches!(
            Query::parse("endpoints.scissor = false")
                .unwrap()
                .matches(&p),
            Err(QueryError {
                kind: QueryErrorKind::UnsupportedAttribute(_),
                ..
            })
        ));
        supplied.insert(
            slot(&keys, "q"),
            slot(&keys, "s"),
            PairGeometry {
                scissor: Some(false),
                lateral_stretch: None,
                diagonal_stretch: None,
            },
        );
        let partial = classify(&keys, &presses, Some(&supplied)).unwrap();
        assert!(Query::parse("first.scissor = false")
            .unwrap()
            .matches(&partial)
            .unwrap());
        assert_eq!(
            Query::parse("first.lateral_stretch = false")
                .unwrap()
                .matches(&partial)
                .unwrap_err()
                .kind,
            QueryErrorKind::UnsupportedAttribute("first.lateral_stretch".into())
        );
    }

    #[test]
    fn malformed_queries_and_pair_applicability() {
        let malformed = [
            "",
            "length",
            "length == 2",
            "length =",
            "length = two",
            "length = 4",
            "length = 2 or redirect = true",
            "length = 2 and",
            "position[3].row = home",
            "gap[2] = 1",
            "row.ascending = true",
            "roll.direction = sideways",
            "hand.pattern = LRX",
            "contains.thumb = perhaps",
            "start.row = \"unterminated",
            "length = 2 and $ = 1",
        ];
        for source in malformed {
            let error = Query::parse(source).unwrap_err();
            assert!(error.position <= source.len(), "{source}: {error}");
        }
        assert_eq!(
            Query::parse("unknown.field = true").unwrap_err().kind,
            QueryErrorKind::UnsupportedAttribute("unknown.field".into())
        );
        let pair = pattern(&["q", "w"]);
        for source in [
            "redirect = false",
            "redirect != true",
            "last.direction != inward",
            "position[2].row != home",
            "gap[1] != 0",
            "row.delta[1] != 0",
        ] {
            assert!(
                !Query::parse(source).unwrap().matches(&pair).unwrap(),
                "{source}"
            );
        }
        assert!(
            Query::parse("hand.pattern = \"LL\" and contains.thumb != true")
                .unwrap()
                .matches(&pair)
                .unwrap()
        );
    }

    #[test]
    fn invalid_inputs_are_rejected() {
        let keys = qwerty();
        assert_eq!(
            classify(&keys, &[], None).unwrap_err(),
            ClassifyError::InvalidLength(0)
        );
        assert_eq!(
            classify(&keys, &selected(&keys, &["q"], &[0]), None).unwrap_err(),
            ClassifyError::InvalidLength(1)
        );
        assert_eq!(
            classify(
                &keys,
                &selected(&keys, &["q", "w", "e", "r"], &[0, 1, 2, 3]),
                None
            )
            .unwrap_err(),
            ClassifyError::InvalidLength(4)
        );
        for positions in [[0, 0], [2, 1]] {
            assert_eq!(
                classify(&keys, &selected(&keys, &["q", "w"], &positions), None).unwrap_err(),
                ClassifyError::InvalidSourcePositions
            );
        }
        if usize::MAX > i64::MAX as usize {
            assert_eq!(
                classify(
                    &keys,
                    &selected(&keys, &["q", "w"], &[i64::MAX as usize, usize::MAX]),
                    None
                )
                .unwrap_err(),
                ClassifyError::ArithmeticOverflow
            );
        }
        assert_eq!(
            classify(
                &keys,
                &[
                    SelectedPress {
                        slot_id: 999,
                        original_position: 0
                    },
                    SelectedPress {
                        slot_id: 0,
                        original_position: 1
                    }
                ],
                None
            )
            .unwrap_err(),
            ClassifyError::MissingSlot(999)
        );
        let mut duplicate = keys.clone();
        duplicate.push(keys[0].clone());
        assert_eq!(
            classify(&duplicate, &selected(&keys, &["q", "w"], &[0, 1]), None).unwrap_err(),
            ClassifyError::DuplicateSlot(0)
        );
        let mut wrong_hand = keys.clone();
        wrong_hand[0].hand = Hand::Right;
        assert_eq!(
            classify(&wrong_hand, &selected(&keys, &["q", "w"], &[0, 1]), None).unwrap_err(),
            ClassifyError::FingerHandMismatch(0)
        );
        let mut bad_coordinate = keys.clone();
        bad_coordinate[0].x = f64::NAN;
        assert_eq!(
            classify(
                &bad_coordinate,
                &selected(&keys, &["q", "w"], &[0, 1]),
                None
            )
            .unwrap_err(),
            ClassifyError::NonFiniteCoordinate(0)
        );
        let mut extreme_rows = keys.clone();
        extreme_rows[0].row = i32::MIN;
        extreme_rows[1].row = i32::MAX;
        assert_eq!(
            classify(&extreme_rows, &selected(&keys, &["q", "w"], &[0, 1]), None).unwrap_err(),
            ClassifyError::ArithmeticOverflow
        );
    }
}
