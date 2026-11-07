// Scissor test: rate how comfortable same-hand key pairs (or single keys)
// are, four to a page, on a 6-level scale. Keys are named by their QWERTY
// *position*, whatever layout the tester types on, so results from different
// people compare directly. Items are shuffled and each pair's typing order is
// random. Every session lands in scissors/ratings.jsonc, and leaving prints a
// short code another person can send back; "Enter a code" decodes it.

const SCISSOR_FILE: &str = "scissors/ratings.jsonc";

const SCISSOR_LEVELS: [&str; 6] = [
    "Very easy",
    "Comfortable",
    "Manageable",
    "Slight issue",
    "Uncomfortable",
    "Terrible",
];

/// One hand's 3×5 block, outer column first.
const SCISSOR_LEFT: [&str; 3] = ["qwert", "asdfg", "zxcvb"];
const SCISSOR_RIGHT: [&str; 3] = ["poiuy", ";lkjh", "/.,mn"];
const SCISSOR_FINGERS: [&str; 5] = ["pinky", "ring", "middle", "index", "index"];

/// Crockford base32: no I, L, O or U, so a code survives being retyped.
const SCISSOR_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

const SCISSOR_PER_PAGE: usize = 4;

#[derive(Clone, Debug, PartialEq)]
struct ScissorItem {
    /// The keys, sorted, upper case: "CS".
    id: String,
    keys: Vec<char>,
    hand: char,
    /// Rows apart: 1 half, 2 full; 0 for a single key.
    jump: usize,
    fingers: Vec<&'static str>,
}

impl ScissorItem {
    fn info(&self) -> String {
        let mut text = self.fingers.join("–");
        if self.jump == 2 {
            text.push_str("  full");
        }
        text
    }
}

fn scissor_block(hand: char) -> [&'static str; 3] {
    if hand == 'L' {
        SCISSOR_LEFT
    } else {
        SCISSOR_RIGHT
    }
}

/// Row and column of a key in its hand's block.
fn scissor_place(hand: char, key: char) -> Option<(usize, usize)> {
    scissor_block(hand)
        .iter()
        .enumerate()
        .find_map(|(row, keys)| keys.find(key).map(|col| (row, col)))
}

fn scissor_hands(hands: char) -> &'static [char] {
    match hands {
        'L' => &['L'],
        'R' => &['R'],
        _ => &['L', 'R'],
    }
}

/// The items of a test, in the fixed order its code uses. `set`: 'h' half
/// scissors, 'f' full, 'a' both, 'k' single keys.
fn scissor_items(hands: char, set: char) -> Vec<ScissorItem> {
    let mut items = Vec::new();
    for &hand in scissor_hands(hands) {
        let keys: Vec<char> = scissor_block(hand).concat().chars().collect();
        if set == 'k' {
            for &key in &keys {
                let (_, col) = scissor_place(hand, key).unwrap();
                items.push(ScissorItem {
                    id: key.to_ascii_uppercase().to_string(),
                    keys: vec![key],
                    hand,
                    jump: 0,
                    fingers: vec![SCISSOR_FINGERS[col]],
                });
            }
            continue;
        }
        for (i, &a) in keys.iter().enumerate() {
            for &b in &keys[i + 1..] {
                let (ra, ca) = scissor_place(hand, a).unwrap();
                let (rb, cb) = scissor_place(hand, b).unwrap();
                let jump = ra.abs_diff(rb);
                let wanted = match jump {
                    1 => set != 'f',
                    2 => set != 'h',
                    _ => false,
                };
                if !wanted || SCISSOR_FINGERS[ca] == SCISSOR_FINGERS[cb] {
                    continue;
                }
                let mut id: Vec<char> = [a, b].iter().map(|c| c.to_ascii_uppercase()).collect();
                id.sort_unstable();
                // Outer finger first: "pinky–middle".
                let fingers = vec![SCISSOR_FINGERS[ca.min(cb)], SCISSOR_FINGERS[ca.max(cb)]];
                items.push(ScissorItem {
                    id: id.into_iter().collect(),
                    keys: vec![a, b],
                    hand,
                    jump,
                    fingers,
                });
            }
        }
    }
    items
}

fn scissor_set_name(hands: char, set: char) -> String {
    let hand = match hands {
        'L' => "left hand",
        'R' => "right hand",
        _ => "both hands",
    };
    let what = match set {
        'h' => "half scissors",
        'f' => "full scissors",
        'a' => "half + full scissors",
        _ => "single keys",
    };
    format!("{what}, {hand}")
}

#[derive(Clone, Debug, PartialEq)]
struct ScissorSession {
    /// "20261107-153012" (UTC), unique per local session.
    session: String,
    /// "20261107": the day the test started.
    date: String,
    tester: String,
    board: String,
    /// 'L', 'R' or 'B'.
    hands: char,
    /// 'h', 'f', 'a' or 'k' (see `scissor_items`).
    set: char,
    /// "local", "code" or "akler_rate.py".
    source: String,
    blind: bool,
    /// Item ids in the order shown; empty for decoded codes.
    order: Vec<String>,
    /// The typing order shown per item: "SC" for item "CS".
    shown: BTreeMap<String, String>,
    /// Item id → level 1..=6.
    ratings: BTreeMap<String, u8>,
}

impl ScissorSession {
    fn items(&self) -> Vec<ScissorItem> {
        scissor_items(self.hands, self.set)
    }

    fn rated(&self) -> usize {
        self.items()
            .iter()
            .filter(|item| self.ratings.contains_key(&item.id))
            .count()
    }

    fn summary(&self) -> String {
        format!(
            "{}  {}  {}  {}  {}/{} rated{}",
            self.date,
            self.tester,
            self.board,
            scissor_set_name(self.hands, self.set),
            self.rated(),
            self.items().len(),
            match self.source.as_str() {
                "local" => String::new(),
                source => format!("  ({source})"),
            }
        )
    }
}

// ------------------------------------------------------------------ code ----

/// Lower case letters, digits and '_': what a code's name fields may hold.
fn scissor_name(text: &str, fallback: &str) -> String {
    let name: String = text
        .trim()
        .chars()
        .map(|c| if c.is_whitespace() || c == '-' { '_' } else { c.to_ascii_lowercase() })
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .take(20)
        .collect();
    if name.is_empty() {
        fallback.into()
    } else {
        name
    }
}

/// `digits` in base `from`, most significant first, rewritten in base `to`
/// with at least `width` digits.
fn scissor_rebase(digits: &[u32], from: u32, to: u32, width: usize) -> Vec<u32> {
    let mut number = digits.to_vec();
    let mut out = Vec::new();
    while number.iter().any(|&d| d != 0) {
        let mut rest = 0;
        for digit in number.iter_mut() {
            let value = rest * from + *digit;
            *digit = value / to;
            rest = value % to;
        }
        out.push(rest);
    }
    out.resize(out.len().max(width), 0);
    out.reverse();
    out
}

/// Base-32 digits a test with `count` items needs: enough for 7^count − 1.
fn scissor_payload_width(count: usize) -> usize {
    scissor_rebase(&vec![6; count], 7, 32, 0).len()
}

/// Two base-32 check characters over the rest of the code.
fn scissor_check(body: &str) -> String {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in body.to_ascii_lowercase().bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    let hash = (hash ^ (hash >> 16)) & 0x3ff;
    [hash >> 5, hash & 31]
        .iter()
        .map(|&d| SCISSOR_ALPHABET[d as usize] as char)
        .collect()
}

/// `DATE-BOARD-TESTER-SET-PAYLOAD`, e.g. `20261107-rowstag-gg-Lh-3M0…`.
/// The payload holds every item's level (0 unrated) in base 7, in the fixed
/// order of `scissor_items`, written in base 32 with two check characters.
fn scissor_code(session: &ScissorSession) -> String {
    let items = session.items();
    let digits: Vec<u32> = items
        .iter()
        .map(|item| session.ratings.get(&item.id).map_or(0, |&level| u32::from(level)))
        .collect();
    let width = scissor_payload_width(items.len());
    let payload: String = scissor_rebase(&digits, 7, 32, width)
        .iter()
        .map(|&d| SCISSOR_ALPHABET[d as usize] as char)
        .collect();
    let body = format!(
        "{}-{}-{}-{}{}-{payload}",
        session.date,
        scissor_name(&session.board, "board"),
        scissor_name(&session.tester, "anon"),
        session.hands,
        session.set,
    );
    let check = scissor_check(&body);
    format!("{body}{check}")
}

fn scissor_decode(code: &str) -> Result<ScissorSession, String> {
    let code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    let parts: Vec<&str> = code.split('-').collect();
    let [date, board, tester, set, payload] = parts[..] else {
        return Err("A code has five parts: date-board-tester-set-ratings.".into());
    };
    if date.len() != 8 || !date.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("'{date}' is not a date (YYYYMMDD)."));
    }
    let mut set_chars = set.chars();
    let (Some(hands), Some(kind), None) = (
        set_chars.next().map(|c| c.to_ascii_uppercase()),
        set_chars.next().map(|c| c.to_ascii_lowercase()),
        set_chars.next(),
    ) else {
        return Err(format!("'{set}' is not a test kind such as Lh."));
    };
    if !"LRB".contains(hands) || !"hfak".contains(kind) {
        return Err(format!("'{set}' is not a test kind such as Lh."));
    }
    let items = scissor_items(hands, kind);
    let width = scissor_payload_width(items.len());
    let normal: String = payload
        .chars()
        .map(|c| match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        })
        .collect();
    if normal.len() != width + 2 {
        return Err(format!(
            "The ratings part should have {} characters for {}; it has {}.",
            width + 2,
            scissor_set_name(hands, kind),
            normal.len()
        ));
    }
    let (digits, check) = normal.split_at(width);
    let body = format!("{date}-{board}-{tester}-{hands}{kind}-{digits}");
    if scissor_check(&body) != check {
        return Err("The code's check characters do not match: it was probably mistyped.".into());
    }
    let values: Vec<u32> = digits
        .bytes()
        .map(|b| SCISSOR_ALPHABET.iter().position(|&a| a == b).map(|d| d as u32))
        .collect::<Option<_>>()
        .ok_or("The ratings part has a character no code uses.")?;
    let levels = scissor_rebase(&values, 32, 7, items.len());
    if levels.len() != items.len() {
        return Err("The ratings part is too large for this test.".into());
    }
    let ratings = items
        .iter()
        .zip(levels)
        .filter(|(_, level)| *level > 0)
        .map(|(item, level)| (item.id.clone(), level as u8))
        .collect();
    Ok(ScissorSession {
        session: format!("code-{date}-{board}-{tester}-{hands}{kind}"),
        date: date.into(),
        tester: tester.to_ascii_lowercase(),
        board: board.to_ascii_lowercase(),
        hands,
        set: kind,
        source: "code".into(),
        blind: true,
        order: Vec::new(),
        shown: BTreeMap::new(),
        ratings,
    })
}

// --------------------------------------------------------------- storage ----

/// UTC "YYYYMMDD-HHMMSS" for `seconds` since 1970.
fn scissor_stamp(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}",
        rest / 3600,
        rest / 60 % 60,
        rest % 60
    )
}

fn scissor_json_session(value: &Json) -> Option<ScissorSession> {
    let Json::Object(fields) = value else {
        return None;
    };
    let text = |name: &str| match fields.get(name) {
        Some(Json::String(text)) => Some(text.clone()),
        _ => None,
    };
    let letter = |name: &str| text(name).and_then(|t| t.chars().next());
    let hands = letter("hands")?;
    let set = letter("set")?;
    if !"LRB".contains(hands) || !"hfak".contains(set) {
        return None;
    }
    let order = match fields.get("order") {
        Some(Json::Array(ids)) => ids
            .iter()
            .filter_map(|id| match id {
                Json::String(id) => Some(id.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    let strings = |name: &str| -> BTreeMap<String, String> {
        match fields.get(name) {
            Some(Json::Object(map)) => map
                .iter()
                .filter_map(|(k, v)| match v {
                    Json::String(v) => Some((k.clone(), v.clone())),
                    _ => None,
                })
                .collect(),
            _ => BTreeMap::new(),
        }
    };
    let ratings = match fields.get("ratings") {
        Some(Json::Object(map)) => map
            .iter()
            .filter_map(|(k, v)| match v {
                Json::Number(n) if (1.0..=6.0).contains(n) && n.fract() == 0.0 => {
                    Some((k.clone(), *n as u8))
                }
                _ => None,
            })
            .collect(),
        _ => BTreeMap::new(),
    };
    Some(ScissorSession {
        session: text("session")?,
        date: text("date").unwrap_or_default(),
        tester: text("tester").unwrap_or_default(),
        board: text("board").unwrap_or_default(),
        hands,
        set,
        source: text("source").unwrap_or_else(|| "local".into()),
        blind: !matches!(fields.get("blind"), Some(Json::Bool(false))),
        order,
        shown: strings("shown"),
        ratings,
    })
}

fn scissor_load(path: &Path) -> AppResult<Vec<ScissorSession>> {
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let json = crate::layout_io::parse_jsonc(&fs::read_to_string(path)?)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let Json::Object(top) = json else {
        return Err(format!("{}: expected an object", path.display()).into());
    };
    Ok(match top.get("sessions") {
        Some(Json::Array(list)) => list.iter().filter_map(scissor_json_session).collect(),
        _ => Vec::new(),
    })
}

/// `entries` as JSON lines of at most ~90 characters, each indented.
fn scissor_json_rows(entries: &[String], indent: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for entry in entries {
        match lines.last_mut() {
            Some(line) if line.len() + entry.len() < 90 => {
                line.push_str(", ");
                line.push_str(entry);
            }
            _ => lines.push(format!("{indent}{entry}")),
        }
    }
    lines.join(",\n")
}

fn scissor_text(sessions: &[ScissorSession]) -> String {
    use crate::layout_export::json_quote as q;
    let mut out = String::from(
        "// akler scissor test ratings, one entry per session.\n\
         // Levels: 1 Very easy, 2 Comfortable, 3 Manageable, 4 Slight issue,\n\
         // 5 Uncomfortable, 6 Terrible. Keys are QWERTY positions.\n\
         // set: h half scissors, f full, a half + full, k single keys.\n\
         {\n  \"sessions\": [\n",
    );
    for (n, s) in sessions.iter().enumerate() {
        out.push_str("    {\n");
        for (name, value) in [
            ("session", q(&s.session)),
            ("date", q(&s.date)),
            ("tester", q(&s.tester)),
            ("board", q(&s.board)),
            ("hands", q(&s.hands.to_string())),
            ("set", q(&s.set.to_string())),
            ("source", q(&s.source)),
            ("blind", s.blind.to_string()),
            ("code", q(&scissor_code(s))),
        ] {
            out.push_str(&format!("      {}: {value},\n", q(name)));
        }
        let order: Vec<String> = s.order.iter().map(|id| q(id)).collect();
        let shown: Vec<String> = s.shown.iter().map(|(k, v)| format!("{}: {}", q(k), q(v))).collect();
        let ratings: Vec<String> = s.ratings.iter().map(|(k, v)| format!("{}: {v}", q(k))).collect();
        for (name, open, close, rows, last) in [
            ("order", '[', ']', order, false),
            ("shown", '{', '}', shown, false),
            ("ratings", '{', '}', ratings, true),
        ] {
            let comma = if last { "" } else { "," };
            if rows.is_empty() {
                out.push_str(&format!("      {}: {open}{close}{comma}\n", q(name)));
            } else {
                out.push_str(&format!(
                    "      {}: {open}\n{}\n      {close}{comma}\n",
                    q(name),
                    scissor_json_rows(&rows, "        ")
                ));
            }
        }
        out.push_str(if n + 1 < sessions.len() { "    },\n" } else { "    }\n" });
    }
    out.push_str("  ]\n}\n");
    out
}

fn scissor_save(path: &Path, sessions: &[ScissorSession]) -> AppResult<()> {
    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory)?;
    }
    atomic_write(path, &scissor_text(sessions))
}

/// Replace the stored session with the same id, or add it.
fn scissor_store(path: &Path, session: &ScissorSession) -> AppResult<()> {
    let mut sessions = scissor_load(path)?;
    match sessions.iter_mut().find(|s| s.session == session.session) {
        Some(stored) => *stored = session.clone(),
        None => sessions.push(session.clone()),
    }
    scissor_save(path, &sessions)
}

// -------------------------------------------------------------------- UI ----

/// Both hands' blocks, row-staggered, with the focused item's keys lit:
/// the first shown key yellow, the second blue. Returns the next free row.
fn scissor_mini_keyboard(c: &mut Canvas, y: usize, lit: &[char]) -> usize {
    let rows = ["qwertyuiop", "asdfghjkl;", "zxcvbnm,./"];
    let width = 10 * 4 + 2 + 2;
    let left = c.w.saturating_sub(width) / 2;
    for (r, keys) in rows.iter().enumerate() {
        let mut x = left + [0, 1, 2][r];
        for (i, key) in keys.chars().enumerate() {
            if i == 5 {
                x += 2;
            }
            let at = lit.iter().position(|&k| k == key);
            let (color, background) = match at {
                Some(0) => (YELLOW, Some((110, 85, 0))),
                Some(_) => (BLUE, Some((0, 70, 120))),
                None => (MUTED, None),
            };
            c.text(x, y + r, &format!("[{}]", key.to_ascii_uppercase()), color);
            if at.is_some() {
                c.row_style(Rect { x, y: y + r, w: 3, h: 1 }, background, true);
            }
            x += 4;
        }
    }
    y + rows.len()
}

struct ScissorForm {
    session: ScissorSession,
    /// Items in the order shown.
    items: Vec<ScissorItem>,
    /// Focused item, and the option highlighted in it.
    cur: usize,
    opt: usize,
}

impl ScissorForm {
    fn pages(&self) -> usize {
        (self.items.len() + SCISSOR_PER_PAGE - 1) / SCISSOR_PER_PAGE
    }

    fn page(&self) -> usize {
        self.cur / SCISSOR_PER_PAGE
    }

    fn rating(&self, i: usize) -> Option<u8> {
        self.session.ratings.get(&self.items[i].id).copied()
    }

    fn focus(&mut self, i: usize) {
        self.cur = i.min(self.items.len() - 1);
        self.opt = self.rating(self.cur).map_or(0, |level| usize::from(level) - 1);
    }

    fn shown(&self, i: usize) -> String {
        let item = &self.items[i];
        self.session
            .shown
            .get(&item.id)
            .cloned()
            .unwrap_or_else(|| item.id.clone())
    }

    fn unrated(&self) -> Option<usize> {
        (0..self.items.len()).find(|&i| self.rating(i).is_none())
    }
}

/// Draw one page: four cards in a 2×2 grid, the keyboard and the footer.
fn scissor_form_frame(c: &mut Canvas, form: &ScissorForm) {
    let s = &form.session;
    let title = format!(
        "Scissor test · {} · {}",
        s.board,
        scissor_set_name(s.hands, s.set)
    );
    let counter = format!(
        "page {}/{} · {}/{} rated",
        form.page() + 1,
        form.pages(),
        s.rated(),
        form.items.len()
    );
    let room = c.w.saturating_sub(counter.chars().count() + 6);
    c.text(2, 0, &short(&title, room), CYAN);
    c.right(0, 0, c.w.saturating_sub(2), &counter, MUTED);
    let note = if s.set == 'k' {
        "Fingers on home row. Press the key a few times from home, then rate it."
    } else {
        "Fingers on home row. Type the pair back and forth a few times, then rate it."
    };
    c.text(2, 1, &short(note, c.w.saturating_sub(4)), MUTED);

    // Tall cards list the six levels down; short screens get two columns,
    // 1-3 on the left and 4-6 on the right.
    let tall = c.h >= 26;
    let card_h = if tall { 8 } else { 5 };
    let card_w = ((c.w.saturating_sub(5)) / 2).min(44);
    let left = c.w.saturating_sub(card_w * 2 + 1) / 2;
    let top = 3;
    let first = form.page() * SCISSOR_PER_PAGE;
    for slot in 0..SCISSOR_PER_PAGE {
        let i = first + slot;
        if i >= form.items.len() {
            break;
        }
        let item = &form.items[i];
        let focused = i == form.cur;
        let rect = Rect {
            x: left + (slot % 2) * (card_w + 1),
            y: top + (slot / 2) * card_h,
            w: card_w,
            h: card_h,
        };
        c.boxed(rect, if focused { CYAN } else { BORDER });
        let head = format!(" {} ", form.shown(i));
        c.text(rect.x + 2, rect.y, &head, if focused { YELLOW } else { FG });
        if focused {
            c.row_style(Rect { x: rect.x + 2, y: rect.y, w: head.len(), h: 1 }, None, true);
        }
        let info = format!(" {} ", item.info());
        c.text(rect.x + 2 + head.len(), rect.y, &info, MUTED);
        c.hit(Rect { x: rect.x, y: rect.y, w: rect.w, h: 1 }, Action::Key(i));
        let chosen = form.rating(i);
        let inner = card_w.saturating_sub(4);
        for (k, name) in SCISSOR_LEVELS.iter().enumerate() {
            let (x, y, w) = if tall {
                (rect.x + 2, rect.y + 1 + k, inner)
            } else {
                let half = 16.min(inner / 2);
                if k < 3 {
                    (rect.x + 2, rect.y + 1 + k, half)
                } else {
                    (rect.x + 2 + half, rect.y + k - 2, inner - half)
                }
            };
            let picked = chosen == Some(k as u8 + 1);
            let mark = if picked { '◉' } else { '○' };
            let color = if picked { GREEN } else if focused { FG } else { MUTED };
            c.text(x, y, &short(&format!("{mark} {} {name}", k + 1), w), color);
            if focused && k == form.opt {
                c.row_style(Rect { x, y, w: w.saturating_sub(1), h: 1 }, Some((60, 60, 60)), true);
            }
            c.hit(Rect { x, y, w, h: 1 }, Action::Item(i * SCISSOR_LEVELS.len() + k));
        }
    }

    let kb = top + 2 * card_h + 1;
    let lit: Vec<char> = form.shown(form.cur).to_lowercase().chars().collect();
    let below = scissor_mini_keyboard(c, kb, &lit);

    // Page buttons and keys.
    let y = (below + usize::from(tall)).max(c.h.saturating_sub(2));
    let mut x = 2;
    for (label, key) in [("◀ Prev", 'p'), ("Next ▶", 'n'), ("Finish", 'f')] {
        let text = format!("[{label}]");
        let enabled = match key {
            'p' => form.page() > 0,
            'n' => form.page() + 1 < form.pages(),
            _ => true,
        };
        c.text(x, y, &text, if enabled { FG } else { MUTED });
        if enabled {
            c.hit(Rect { x, y, w: text.chars().count(), h: 1 }, Action::Command(key));
        }
        x += text.chars().count() + 2;
    }
    let done = form.unrated().is_none();
    c.text(
        x + 1,
        y,
        if done { "All rated: Finish (Esc) shows your code." } else { "" },
        GREEN,
    );
    c.text(
        2,
        y + 1,
        &short(
            "1-6 rate  ↑↓ level  ←→ card  PgUp/PgDn page  u unrated  x clear  Esc finish",
            c.w.saturating_sub(4),
        ),
        MUTED,
    );
}

/// Run the form until the tester finishes; ratings are saved as they change.
fn scissor_form(term: &mut Terminal, path: &Path, form: &mut ScissorForm) -> AppResult<()> {
    let per = SCISSOR_LEVELS.len();
    loop {
        let mut c = Canvas::new(term.width(), term.size.1.max(20));
        scissor_form_frame(&mut c, form);
        term.present(&c, 0)?;
        let e = term.event()?;
        let mut pick: Option<(usize, usize, bool)> = None;
        if let Some(action) = action_press(term, &c, &e, 0) {
            match action {
                Action::Item(n) => pick = Some((n / per, n % per, false)),
                Action::Key(i) => form.focus(i),
                Action::Command('p') => form.focus((form.page() - 1) * SCISSOR_PER_PAGE),
                Action::Command('n') => form.focus((form.page() + 1) * SCISSOR_PER_PAGE),
                Action::Command('f') => return Ok(()),
                _ => {}
            }
        }
        let last = form.items.len() - 1;
        match e {
            Event::Escape | Event::Quit | Event::Char('q') => return Ok(()),
            Event::Up | Event::Char('k') => form.opt = form.opt.saturating_sub(1),
            Event::Down | Event::Char('j') => form.opt = (form.opt + 1).min(per - 1),
            Event::Left | Event::Char('h') => form.focus(form.cur.saturating_sub(1)),
            Event::Right | Event::Char('l') | Event::Tab => form.focus(form.cur + 1),
            Event::PageDown | Event::Char('n') => {
                if form.page() + 1 < form.pages() {
                    form.focus((form.page() + 1) * SCISSOR_PER_PAGE);
                }
            }
            Event::PageUp | Event::Char('p') => {
                if form.page() > 0 {
                    form.focus((form.page() - 1) * SCISSOR_PER_PAGE);
                }
            }
            Event::Wheel(n) if n > 0 && form.page() + 1 < form.pages() => {
                form.focus((form.page() + 1) * SCISSOR_PER_PAGE);
            }
            Event::Wheel(n) if n < 0 && form.page() > 0 => {
                form.focus((form.page() - 1) * SCISSOR_PER_PAGE);
            }
            Event::Home => form.focus(0),
            Event::End => form.focus(last),
            Event::Char('u') => {
                if let Some(i) = form.unrated() {
                    form.focus(i);
                }
            }
            Event::Char('f') => return Ok(()),
            Event::Enter | Event::Char(' ') => pick = Some((form.cur, form.opt, true)),
            Event::Char(ch @ '1'..='6') => pick = Some((form.cur, ch as usize - '1' as usize, true)),
            Event::Char('x') | Event::Backspace => {
                let id = form.items[form.cur].id.clone();
                if form.session.ratings.remove(&id).is_some() {
                    scissor_store(path, &form.session)?;
                }
            }
            _ => {}
        }
        if let Some((i, k, advance)) = pick {
            let id = form.items[i].id.clone();
            form.session.ratings.insert(id, k as u8 + 1);
            scissor_store(path, &form.session)?;
            if advance && i < last {
                form.focus(i + 1);
            } else {
                form.cur = i;
                form.opt = k;
            }
        }
        if term.quitting {
            return Ok(());
        }
    }
}

/// A session's ratings in a table, with its code above; `c` copies the code.
fn scissor_view(term: &mut Terminal, title: &str, session: &ScissorSession, note: &str) -> AppResult<()> {
    let code = scissor_code(session);
    let _ = term.copy_to_clipboard(&code);
    let mut status = "Copied to the clipboard.".to_string();
    let items = session.items();
    let mut scroll = 0;
    loop {
        let col_w = 38;
        let cols = (term.width().min(PANEL_WIDTH).saturating_sub(4) / col_w).max(1);
        let rows = (items.len() + cols - 1) / cols;
        let mut c = Canvas::new(term.width(), rows + 12);
        c.text(2, 0, title, CYAN);
        c.text(2, 1, &short(&session.summary(), c.w.saturating_sub(4)), MUTED);
        let line = 3;
        c.text(2, line, "Code to send back:", FG);
        let box_w = (code.chars().count() + 4).min(c.w.saturating_sub(4));
        c.boxed(Rect { x: 2, y: line + 1, w: box_w, h: 3 }, CYAN);
        c.text(4, line + 2, &short(&code, box_w.saturating_sub(4)), YELLOW);
        c.row_style(Rect { x: 4, y: line + 2, w: code.len(), h: 1 }, None, true);
        c.text(2, line + 4, &short(&format!("{status}  {note}"), c.w.saturating_sub(4)), MUTED);
        let table = line + 6;
        for (n, item) in items.iter().enumerate() {
            let x = 2 + (n / rows) * col_w;
            let y = table + n % rows;
            let level = session.ratings.get(&item.id).copied();
            c.text(x, y, &item.id, FG);
            let mut info = item.fingers.join("–");
            if item.jump == 2 && session.set == 'a' {
                info.push_str(" F");
            }
            c.text(x + 4, y, &short(&info, 15), MUTED);
            let (text, color) = match level {
                Some(level) => (
                    format!("{level} {}", SCISSOR_LEVELS[usize::from(level) - 1]),
                    [GREEN, GREEN, FG, YELLOW, RED, RED][usize::from(level) - 1],
                ),
                None => ("–".into(), MUTED),
            };
            c.text(x + 20, y, &short(&text, col_w - 21), color);
        }
        let legend = if session.set == 'a' { "F = full scissor   " } else { "" };
        c.text(2, table + rows + 1, &format!("{legend}c copy code again   PgUp/PgDn scroll   Esc back"), MUTED);
        term.present(&c, scroll)?;
        let e = term.event()?;
        if scroll_event(&e, &mut scroll, c.h, term.size.1) {
            continue;
        }
        match e {
            Event::Escape | Event::Quit | Event::Char('q') | Event::Enter => return Ok(()),
            Event::Char('c') => {
                term.copy_to_clipboard(&code)?;
                status = "Copied again.".into();
            }
            _ => {}
        }
    }
}

fn scissor_new_session(term: &mut Terminal, path: &Path, keys_only: bool) -> AppResult<()> {
    let sessions = scissor_load(path)?;
    let last = sessions.iter().rev().find(|s| s.source == "local");
    let tester_default = last.map_or("", |s| s.tester.as_str()).to_string();
    let Some(tester) = input_box(term, "Your name (letters and digits; it goes into the code)", "", &tester_default)? else {
        return Ok(());
    };
    let board_default = last.map_or("rowstag", |s| s.board.as_str()).to_string();
    let Some(board) = input_box(term, "Keyboard you rate on (e.g. rowstag, ortho, corne)", "", &board_default)? else {
        return Ok(());
    };
    let hand_items = ["Left hand", "Right hand", "Both hands"].map(String::from);
    let Some(hand) = menu(term, "Which hand?", &hand_items)? else {
        return Ok(());
    };
    let hands = ['L', 'R', 'B'][hand];
    let set = if keys_only {
        'k'
    } else {
        let kinds = [
            "Half scissors (one row apart)",
            "Full scissors (two rows apart)",
            "Half and full scissors",
        ]
        .map(String::from);
        let Some(kind) = menu(term, "Which scissors?", &kinds)? else {
            return Ok(());
        };
        ['h', 'f', 'a'][kind]
    };

    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_secs());
    let stamp = scissor_stamp(seconds);
    let mut rng = Rng::new(timestamp() as u64 ^ 0x9e37_79b9_7f4a_7c15);
    let mut items = scissor_items(hands, set);
    for i in (1..items.len()).rev() {
        items.swap(i, rng.index(i + 1));
    }
    let mut shown = BTreeMap::new();
    for item in &items {
        let mut keys = item.keys.clone();
        if keys.len() == 2 && rng.index(2) == 1 {
            keys.swap(0, 1);
        }
        let text: String = keys.iter().map(|k| k.to_ascii_uppercase()).collect();
        shown.insert(item.id.clone(), text);
    }
    let session = ScissorSession {
        session: stamp.clone(),
        date: stamp[..8].into(),
        tester: scissor_name(&tester, "anon"),
        board: scissor_name(&board, "board"),
        hands,
        set,
        source: "local".into(),
        blind: true,
        order: items.iter().map(|item| item.id.clone()).collect(),
        shown,
        ratings: BTreeMap::new(),
    };
    scissor_store(path, &session)?;
    let mut form = ScissorForm { session, items, cur: 0, opt: 0 };
    scissor_form(term, path, &mut form)?;
    if term.quitting {
        return Ok(());
    }
    let left = form.items.len() - form.session.rated();
    let note = if left == 0 {
        "Thank you! Send this code back.".to_string()
    } else {
        format!("{left} items unrated; they show as blank in the code.")
    };
    scissor_view(term, "Scissor test: done", &form.session, &note)
}

fn scissor_enter_code(term: &mut Terminal, path: &Path) -> AppResult<()> {
    let Some(code) = input_box(term, "Paste or type a scissor test code", "", "")? else {
        return Ok(());
    };
    if code.is_empty() {
        return Ok(());
    }
    let session = match scissor_decode(&code) {
        Ok(session) => session,
        Err(error) => return info_page(term, "Scissor test: code not read", &[error]),
    };
    let sessions = scissor_load(path)?;
    let known = sessions
        .iter()
        .any(|s| s.source == "code" && scissor_code(s) == scissor_code(&session));
    let note = if known {
        "This code was imported before; nothing changed."
    } else {
        let mut session = session.clone();
        // Two different codes from one tester and day keep separate entries.
        let base = session.session.clone();
        let mut n = 2;
        while sessions.iter().any(|s| s.session == session.session) {
            session.session = format!("{base}-{n}");
            n += 1;
        }
        scissor_store(path, &session)?;
        "Saved to scissors/ratings.jsonc."
    };
    scissor_view(term, "Scissor test: decoded code", &session, note)
}

fn scissor_browse(term: &mut Terminal, path: &Path) -> AppResult<()> {
    loop {
        let sessions = scissor_load(path)?;
        if sessions.is_empty() {
            return info_page(term, "Scissor test", &["No sessions saved yet.".into()]);
        }
        let names: Vec<String> = sessions.iter().rev().map(ScissorSession::summary).collect();
        let Some(index) = search_menu(term, "Scissor test: saved sessions (newest first)", &names)? else {
            return Ok(());
        };
        let session = &sessions[sessions.len() - 1 - index];
        scissor_view(term, "Scissor test: session", session, "")?;
        if term.quitting {
            return Ok(());
        }
    }
}

fn scissor_test(term: &mut Terminal) -> AppResult<()> {
    let path = Path::new(SCISSOR_FILE);
    let items = [
        "Rate scissor pairs",
        "Rate single keys",
        "Enter a code someone sent you",
        "Saved sessions",
    ]
    .map(String::from);
    loop {
        let Some(choice) = menu(term, "Scissor test", &items)? else {
            return Ok(());
        };
        match choice {
            0 => scissor_new_session(term, path, false)?,
            1 => scissor_new_session(term, path, true)?,
            2 => scissor_enter_code(term, path)?,
            _ => scissor_browse(term, path)?,
        }
        if term.quitting {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod scissor_tests {
    use super::*;

    fn session(hands: char, set: char) -> ScissorSession {
        ScissorSession {
            session: "20261107-120000".into(),
            date: "20261107".into(),
            tester: "gg".into(),
            board: "rowstag".into(),
            hands,
            set,
            source: "local".into(),
            blind: true,
            order: Vec::new(),
            shown: BTreeMap::new(),
            ratings: BTreeMap::new(),
        }
    }

    #[test]
    fn item_sets_match_the_rating_script() {
        assert_eq!(scissor_items('L', 'h').len(), 36);
        assert_eq!(scissor_items('L', 'f').len(), 18);
        assert_eq!(scissor_items('B', 'a').len(), 108);
        assert_eq!(scissor_items('R', 'k').len(), 15);
        let half: Vec<_> = scissor_items('L', 'h').into_iter().map(|i| i.id).collect();
        for id in ["DR", "AB", "SZ", "CG", "AE"] {
            assert!(half.contains(&id.to_string()), "{id}");
        }
        // Same finger (the two index columns) is never a scissor.
        assert!(!half.contains(&"FT".to_string()) && !half.contains(&"GR".to_string()));
    }

    #[test]
    fn codes_round_trip_partial_and_full() {
        for (hands, set) in [('L', 'h'), ('B', 'a'), ('R', 'k'), ('L', 'f')] {
            let mut s = session(hands, set);
            for (n, item) in s.items().iter().enumerate() {
                if n % 5 != 3 {
                    s.ratings.insert(item.id.clone(), (n % 6) as u8 + 1);
                }
            }
            let code = scissor_code(&s);
            assert!(code.starts_with("20261107-rowstag-gg-"), "{code}");
            let back = scissor_decode(&code.to_lowercase()).unwrap();
            assert_eq!(back.ratings, s.ratings, "{code}");
            assert_eq!((back.hands, back.set), (hands, set));
        }
        let blank = session('L', 'h');
        assert!(scissor_decode(&scissor_code(&blank)).unwrap().ratings.is_empty());
    }

    #[test]
    fn mistyped_codes_are_caught() {
        let mut s = session('L', 'h');
        s.ratings.insert("DR".into(), 2);
        let code = scissor_code(&s);
        let mut chars: Vec<char> = code.chars().collect();
        let n = chars.len() - 5;
        chars[n] = if chars[n] == '7' { '8' } else { '7' };
        let typo: String = chars.into_iter().collect();
        assert!(scissor_decode(&typo).is_err());
        assert!(scissor_decode("20261107-rowstag-gg-Lh").is_err());
    }

    #[test]
    fn sessions_survive_a_save() {
        let path = std::env::temp_dir().join(format!("akler-scissor-{}.jsonc", timestamp()));
        let mut a = session('L', 'h');
        a.order = a.items().iter().map(|i| i.id.clone()).collect();
        a.shown.insert("CS".into(), "SC".into());
        a.ratings.insert("CS".into(), 5);
        let mut b = session('R', 'k');
        b.session = "code-x".into();
        b.source = "code".into();
        b.blind = false;
        scissor_save(&path, &[a.clone(), b.clone()]).unwrap();
        let loaded = scissor_load(&path).unwrap();
        let _ = fs::remove_file(&path);
        assert_eq!(loaded, vec![a, b]);
    }

    #[test]
    fn stamps_are_utc_dates() {
        assert_eq!(scissor_stamp(0), "19700101-000000");
        assert_eq!(scissor_stamp(1_793_620_800), "20261102-120000");
        assert_eq!(scissor_stamp(951_782_400), "20000229-000000");
    }
}
