// Scissor test: the user types real words on their own keyboard and rates
// how much they dislike each same-hand, cross-row finger pair. Ratings go to
// scissors/<layout>.csv, with each pair's geometry and Mana2 rating, for
// fitting a scissor formula. Mana2's rating is never shown during the test.

const SCISSOR_DIR: &str = "scissors";

const SCISSOR_LEVELS: [&str; 5] = [
    "fine",
    "slightly awkward",
    "awkward",
    "uncomfortable",
    "awful",
];

/// Words typed per pair, and how many of the corpus text's most frequent
/// words are searched for each pair.
const SCISSOR_WORDS: usize = 3;
const SCISSOR_VOCABULARY: usize = 60_000;
const SCISSOR_WORDS_PER_PAIR: usize = 30;

const SCISSOR_HEADER: &str = "layout,pass,first_label,second_label,first_slot,second_slot,hand,first_finger,second_finger,first_row,first_col,second_row,second_col,first_inner,second_inner,first_x,first_y,second_x,second_y,dx,dy,direction,lower_finger,mana2,words,pair_ms,typical_ms,attempts,errors,rating,level,time";

struct ScissorPair {
    first: usize,
    second: usize,
    /// Words whose typing presses `first` then `second`, most frequent first.
    words: Vec<String>,
    /// Where the next prompt starts in `words`.
    next: usize,
}

struct ScissorPrompt {
    text: Vec<u8>,
    /// Per occurrence of the pair: the text index of the last byte `first`
    /// types and of the first byte `second` types.
    targets: Vec<(usize, usize)>,
    /// The same for every other press pair within a word, for the user's
    /// typical speed.
    others: Vec<(usize, usize)>,
}

#[derive(Default)]
struct ScissorTyping {
    typed: Vec<u8>,
    /// When each text position was last typed, and whether that was the
    /// first try with nothing erased.
    times: Vec<Option<Instant>>,
    first_try: Vec<bool>,
    errors: usize,
}

impl ScissorTyping {
    fn new(len: usize) -> Self {
        Self {
            typed: Vec::new(),
            times: vec![None; len],
            first_try: vec![true; len],
            errors: 0,
        }
    }

    fn done(&self, text: &[u8]) -> bool {
        self.typed == text
    }

    fn push(&mut self, text: &[u8], byte: u8) {
        let at = self.typed.len();
        if at >= text.len() {
            return;
        }
        if byte != text[at] {
            self.errors += 1;
            self.first_try[at] = false;
        }
        self.typed.push(byte);
        self.times[at] = Some(Instant::now());
    }

    fn erase(&mut self) {
        if self.typed.pop().is_some() {
            self.first_try[self.typed.len()] = false;
        }
    }

    /// Median milliseconds between the presses of `pairs` typed on the first
    /// try.
    fn median_ms(&self, pairs: &[(usize, usize)]) -> Option<f64> {
        let mut gaps: Vec<f64> = pairs
            .iter()
            .filter(|&&(a, b)| self.first_try[a] && self.first_try[b])
            .filter_map(|&(a, b)| Some(self.times[b]?.duration_since(self.times[a]?)))
            .map(|gap| gap.as_secs_f64() * 1000.0)
            .collect();
        if gaps.is_empty() {
            return None;
        }
        gaps.sort_by(f64::total_cmp);
        let middle = gaps.len() / 2;
        Some(if gaps.len() % 2 == 1 {
            gaps[middle]
        } else {
            (gaps[middle - 1] + gaps[middle]) / 2.0
        })
    }
}

/// The saved ratings for one layout: each line with the pair it rates.
struct ScissorLog {
    path: PathBuf,
    lines: Vec<((usize, usize), String)>,
}

impl ScissorLog {
    fn load(path: PathBuf) -> AppResult<Self> {
        let mut lines = Vec::new();
        if path.is_file() {
            for line in fs::read_to_string(&path)?.lines().skip(1) {
                let fields = scissor_csv_fields(line);
                let slot = |index: usize| fields.get(index).and_then(|field| field.parse().ok());
                if let (Some(first), Some(second)) = (slot(4), slot(5)) {
                    lines.push(((first, second), line.to_owned()));
                }
            }
        }
        Ok(Self { path, lines })
    }

    fn save(&self) -> AppResult<()> {
        if let Some(directory) = self.path.parent() {
            fs::create_dir_all(directory)?;
        }
        let mut text = format!("{SCISSOR_HEADER}\n");
        for (_, line) in &self.lines {
            text.push_str(line);
            text.push('\n');
        }
        fs::write(&self.path, text)?;
        Ok(())
    }

    fn count(&self, pair: (usize, usize)) -> usize {
        self.lines.iter().filter(|(rated, _)| *rated == pair).count()
    }
}

fn scissor_csv(field: &str) -> String {
    if field.contains([',', '"', '\n']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_owned()
    }
}

fn scissor_csv_fields(line: &str) -> Vec<String> {
    let mut fields = vec![String::new()];
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' if quoted && chars.peek() == Some(&'"') => {
                chars.next();
                fields.last_mut().unwrap().push('"');
            }
            '"' => quoted = !quoted,
            ',' if !quoted => fields.push(String::new()),
            _ => fields.last_mut().unwrap().push(ch),
        }
    }
    fields
}

fn scissor_thumb(layout: &action_keys::Layout, slot: usize) -> bool {
    layout.slots[slot].finger >= 8
}

fn scissor_left(layout: &action_keys::Layout, slot: usize) -> bool {
    layout.slots[slot].finger < 4
}

/// An index key with another index key of its row between it and the
/// middle finger.
fn scissor_inner(layout: &action_keys::Layout, slot: usize) -> bool {
    let key = &layout.slots[slot];
    if !matches!(key.finger, 3 | 4) {
        return false;
    }
    layout.slots.iter().any(|other| {
        other.finger == key.finger
            && other.row == key.row
            && if key.finger == 3 {
                other.col < key.col
            } else {
                other.col > key.col
            }
    })
}

/// Same-hand pairs of different non-thumb fingers at different heights, in
/// both orders.
fn scissor_pairs(layout: &action_keys::Layout) -> Vec<(usize, usize)> {
    let keys = crate::action_ui::physical_keys(layout);
    let mut pairs = Vec::new();
    for first in 0..keys.len() {
        for second in 0..keys.len() {
            if first == second
                || scissor_thumb(layout, first)
                || scissor_thumb(layout, second)
                || scissor_left(layout, first) != scissor_left(layout, second)
                || layout.slots[first].finger == layout.slots[second].finger
                || mana2_metrics::position(keys[first]).1 == mana2_metrics::position(keys[second]).1
            {
                continue;
            }
            pairs.push((first, second));
        }
    }
    pairs
}

/// A ready word list, most frequent first, one per line, used instead of raw
/// text when present. Web builds ship it (`akler scissor-words`).
const SCISSOR_WORD_LIST: &str = "corpus/scissor-words.txt";

/// The word list or raw text words come from, and its words.
fn scissor_source() -> AppResult<(PathBuf, Vec<Vec<u8>>)> {
    let list = Path::new(SCISSOR_WORD_LIST);
    if list.is_file() {
        let text = fs::read(list).map_err(|error| format!("{}: {error}", list.display()))?;
        let words = text
            .split(|&byte| byte == b'\n')
            .filter(|word| !word.is_empty())
            .map(<[u8]>::to_vec)
            .collect();
        return Ok((list.to_owned(), words));
    }
    let path = scissor_text_path()?;
    let words = scissor_vocabulary(&path)?;
    Ok((path, words))
}

/// `akler scissor-words`: print the raw text's word list for `SCISSOR_WORD_LIST`.
fn scissor_words_command() -> AppResult<()> {
    let words = scissor_vocabulary(&scissor_text_path()?)?;
    let mut out = BufWriter::new(io::stdout().lock());
    for word in words {
        out.write_all(&word)?;
        out.write_all(b"\n")?;
    }
    out.flush()?;
    Ok(())
}

/// The default corpus's raw text, or any raw text.
fn scissor_text_path() -> AppResult<PathBuf> {
    let preferred = Path::new(RAW_CORPUS_DIR).join("reddit.txt");
    if preferred.is_file() {
        return Ok(preferred);
    }
    let mut paths: Vec<_> = fs::read_dir(RAW_CORPUS_DIR)
        .map_err(|error| format!("{RAW_CORPUS_DIR}: {error}"))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_file())
        .collect();
    paths.sort();
    paths
        .into_iter()
        .next()
        .ok_or_else(|| format!("the scissor test needs words: add a text file to {RAW_CORPUS_DIR}").into())
}

/// Lowercase words of the text, most frequent first.
fn scissor_vocabulary(path: &Path) -> AppResult<Vec<Vec<u8>>> {
    let text = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut counts: HashMap<&[u8], u32> = HashMap::new();
    // Words keep one trailing punctuation mark, so punctuation keys are
    // typed in context too.
    for token in text.split(|byte| byte.is_ascii_whitespace()) {
        let token = token
            .strip_prefix(b"\"")
            .or_else(|| token.strip_prefix(b"("))
            .unwrap_or(token);
        let token = token
            .strip_suffix(b"\"")
            .or_else(|| token.strip_suffix(b")"))
            .unwrap_or(token);
        let word = token
            .strip_suffix(b",")
            .or_else(|| token.strip_suffix(b"."))
            .or_else(|| token.strip_suffix(b";"))
            .unwrap_or(token);
        if (2..=12).contains(&word.len())
            && word.iter().all(|byte| byte.is_ascii_lowercase() || *byte == b'\'')
            && word[0] != b'\''
            && word[word.len() - 1] != b'\''
        {
            *counts.entry(token).or_default() += 1;
        }
    }
    let mut words: Vec<_> = counts.into_iter().filter(|&(_, count)| count >= 3).collect();
    words.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    Ok(words
        .into_iter()
        .take(SCISSOR_VOCABULARY)
        .map(|(word, _)| word.to_vec())
        .collect())
}

/// How `layout` types `text`, as akler's decoder presses it.
fn scissor_steps(
    layout: &action_keys::Layout,
    effort: &crate::action_ui::LocalEffort,
    text: &[u8],
) -> Option<Vec<action_keys::Step>> {
    let stop = AtomicBool::new(false);
    action_keys::decode(
        layout,
        text,
        action_keys::DEFAULT_STATE_LIMIT,
        &stop,
        |a, b, key| effort.get(a, b, key),
    )
    .ok()
}

/// For each of `pairs`, the most frequent words that press it.
fn scissor_words(
    layout: &action_keys::Layout,
    effort: &crate::action_ui::LocalEffort,
    vocabulary: &[Vec<u8>],
    pairs: &[(usize, usize)],
) -> Vec<Vec<String>> {
    let threads = available_threads();
    let chunk = vocabulary.len().div_ceil(threads).max(1);
    // Each word's consecutive presses, in vocabulary order.
    let presses: Vec<Vec<(usize, usize)>> = thread::scope(|scope| {
        let workers: Vec<_> = vocabulary
            .chunks(chunk)
            .map(|words| {
                scope.spawn(move || {
                    words
                        .iter()
                        .map(|word| {
                            scissor_steps(layout, effort, word)
                                .map(|steps| {
                                    steps.windows(2).map(|step| (step[0].key, step[1].key)).collect()
                                })
                                .unwrap_or_default()
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().unwrap())
            .collect()
    });
    let index: HashMap<(usize, usize), usize> =
        pairs.iter().enumerate().map(|(index, &pair)| (pair, index)).collect();
    let mut words = vec![Vec::new(); pairs.len()];
    for (word, pressed) in vocabulary.iter().zip(&presses) {
        let mut seen = Vec::new();
        for pair in pressed {
            if let Some(&index) = index.get(pair) {
                if !seen.contains(&index) && words[index].len() < SCISSOR_WORDS_PER_PAIR {
                    seen.push(index);
                    words[index].push(String::from_utf8_lossy(word).into_owned());
                }
            }
        }
    }
    words
}

/// `text` with its pair occurrences located, or None when typing it never
/// presses the pair.
fn scissor_prompt(
    layout: &action_keys::Layout,
    effort: &crate::action_ui::LocalEffort,
    pair: (usize, usize),
    text: Vec<u8>,
) -> Option<ScissorPrompt> {
    let steps = scissor_steps(layout, effort, &text)?;
    let mut targets = Vec::new();
    let mut others = Vec::new();
    for step in steps.windows(2) {
        let (a, b) = (&step[0], &step[1]);
        if a.output.is_empty() || b.output.is_empty() || a.end != b.start {
            continue;
        }
        let gap = (a.end - 1, b.start);
        if (a.key, b.key) == pair {
            targets.push(gap);
        } else if !a.output.contains(&b' ') && !b.output.contains(&b' ') {
            others.push(gap);
        }
    }
    (!targets.is_empty()).then_some(ScissorPrompt {
        text,
        targets,
        others,
    })
}

/// The next words for `pair`, filled up to `SCISSOR_WORDS` with a drill of
/// its two keys' characters (all drill when no word presses it).
fn scissor_next_prompt(
    layout: &action_keys::Layout,
    effort: &crate::action_ui::LocalEffort,
    pair: &mut ScissorPair,
) -> Option<ScissorPrompt> {
    let keys = (pair.first, pair.second);
    // A key's own character, or its label's for an action key such as an
    // adaptive one; decoding the prompt checks it presses the pair.
    let typed = |slot: usize| match &layout.slots[slot].binding {
        action_keys::Binding::Text(text) if text.len() == 1 => Some(text[0]),
        _ => {
            let label = layout.slots[slot].label.as_bytes();
            (label.len() == 1).then(|| label[0])
        }
    };
    let drill = typed(pair.first).zip(typed(pair.second)).map(|(a, b)| vec![a, b]);
    let count = pair.words.len().min(SCISSOR_WORDS);
    let mut tokens: Vec<Vec<u8>> = (0..count)
        .map(|offset| pair.words[(pair.next + offset) % pair.words.len()].clone().into_bytes())
        .collect();
    if count > 0 {
        pair.next = (pair.next + count) % pair.words.len();
    }
    if let Some(drill) = &drill {
        tokens.resize(SCISSOR_WORDS, drill.clone());
    }
    scissor_prompt(layout, effort, keys, tokens.join(&b' '))
        .or_else(|| scissor_prompt(layout, effort, keys, vec![drill?; SCISSOR_WORDS].join(&b' ')))
}

fn scissor_row_name(row: i8) -> String {
    match row {
        0 => "top".into(),
        1 => "home".into(),
        2 => "bottom".into(),
        row => format!("row {}", row + 1),
    }
}

/// "left index (inner) top" for the user, and the finger's name.
fn scissor_key_name(layout: &action_keys::Layout, slot: usize) -> String {
    let key = &layout.slots[slot];
    let finger = ["pinky", "ring", "middle", "index", "index", "middle", "ring", "pinky"]
        .get(key.finger)
        .copied()
        .unwrap_or("thumb");
    format!(
        "{} {finger}{} {}",
        if scissor_left(layout, slot) { "left" } else { "right" },
        if scissor_inner(layout, slot) { " (inner)" } else { "" },
        scissor_row_name(key.row)
    )
}

#[allow(clippy::too_many_arguments)]
fn scissor_record(
    layout_name: &str,
    layout: &action_keys::Layout,
    pass: usize,
    pair: &ScissorPair,
    prompt: &ScissorPrompt,
    typing: &ScissorTyping,
    attempts: usize,
    rating: usize,
) -> String {
    let keys = crate::action_ui::physical_keys(layout);
    let (a, b) = (&layout.slots[pair.first], &layout.slots[pair.second]);
    let (ax, ay) = mana2_metrics::position(keys[pair.first]);
    let (bx, by) = mana2_metrics::position(keys[pair.second]);
    let lower = if by > ay { b } else { a };
    let ms = |value: Option<f64>| value.map(|value| format!("{value:.1}")).unwrap_or_default();
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_secs());
    [
        layout_name.to_owned(),
        pass.to_string(),
        a.label.clone(),
        b.label.clone(),
        pair.first.to_string(),
        pair.second.to_string(),
        if scissor_left(layout, pair.first) { "left" } else { "right" }.into(),
        FINGER_NAMES[a.finger].into(),
        FINGER_NAMES[b.finger].into(),
        a.row.to_string(),
        a.col.to_string(),
        b.row.to_string(),
        b.col.to_string(),
        scissor_inner(layout, pair.first).to_string(),
        scissor_inner(layout, pair.second).to_string(),
        ax.to_string(),
        ay.to_string(),
        bx.to_string(),
        by.to_string(),
        (bx - ax).to_string(),
        (by - ay).to_string(),
        if by > ay { "down" } else { "up" }.into(),
        FINGER_NAMES[lower.finger].into(),
        format!("{:.4}", mana2_metrics::scissor(keys[pair.first], keys[pair.second])),
        String::from_utf8_lossy(&prompt.text).into_owned(),
        ms(typing.median_ms(&prompt.targets)),
        ms(typing.median_ms(&prompt.others)),
        attempts.to_string(),
        typing.errors.to_string(),
        rating.to_string(),
        SCISSOR_LEVELS[rating].into(),
        time.to_string(),
    ]
    .iter()
    .map(|field| scissor_csv(field))
    .collect::<Vec<_>>()
    .join(",")
}

/// The usual keyboard, the pair's first key on yellow and its second on
/// blue, ending at the canvas's last row (or below `top` when it is short).
fn scissor_keyboard(
    canvas: &mut Canvas,
    top: usize,
    layout: &action_keys::Layout,
    pair: (usize, usize),
) -> usize {
    let mut probe = Canvas::new(canvas.w, 1);
    let height = crate::action_ui::action_keyboard(&mut probe, 0, layout, layout, None, None);
    let y = canvas.h.saturating_sub(height).max(top);
    let bottom = crate::action_ui::action_keyboard(canvas, y, layout, layout, None, None);
    for (slot, marker, color, background) in [
        (pair.0, '1', YELLOW, (110, 85, 0)),
        (pair.1, '2', BLUE, (0, 70, 120)),
    ] {
        let key = canvas
            .hits
            .iter()
            .find(|(_, action)| matches!(action, Action::Key(key) if *key == slot))
            .map(|(rect, _)| *rect);
        if let Some(rect) = key {
            canvas.boxed(rect, color);
            canvas.row_style(
                Rect {
                    x: rect.x + 1,
                    y: rect.y + 1,
                    w: rect.w.saturating_sub(2),
                    h: 1,
                },
                Some(background),
                true,
            );
            canvas.put(rect.x + 1, rect.y, marker, color);
        }
    }
    bottom
}

enum ScissorStep {
    Rated,
    Skip,
    Undo,
    Leave,
}

#[allow(clippy::too_many_arguments)]
fn scissor_trial(
    term: &mut Terminal,
    layout_name: &str,
    layout: &action_keys::Layout,
    effort: &crate::action_ui::LocalEffort,
    pair: &mut ScissorPair,
    progress: (usize, usize, usize),
    log: &mut ScissorLog,
    undoable: bool,
) -> AppResult<ScissorStep> {
    let (done, total, pass) = progress;
    let Some(mut prompt) = scissor_next_prompt(layout, effort, pair) else {
        return Ok(ScissorStep::Skip);
    };
    let mut typing = ScissorTyping::new(prompt.text.len());
    let mut attempts = 1;
    let first = &layout.slots[pair.first].label;
    let second = &layout.slots[pair.second].label;
    loop {
        let rating = typing.done(&prompt.text);
        let mut canvas = Canvas::new(term.width(), term.size.1.max(12));
        canvas.text(2, 0, &format!("Scissor test · {layout_name}"), CYAN);
        canvas.right(
            0,
            0,
            canvas.w.saturating_sub(2),
            &format!("pair {} of {total} · pass {pass}", done + 1),
            MUTED,
        );
        let mut x = 2;
        for (text, color) in [
            (first.as_str(), YELLOW),
            (" → ", FG),
            (second.as_str(), BLUE),
            ("   ", FG),
        ] {
            canvas.text(x, 2, text, color);
            x += text.chars().count();
        }
        canvas.text(
            x,
            2,
            &format!(
                "{} → {}",
                scissor_key_name(layout, pair.first),
                scissor_key_name(layout, pair.second)
            ),
            MUTED,
        );
        // The typing bar: typed text green (red where wrong), the pair's
        // characters in its keys' colours until typed, on a grey band.
        let bar = Rect {
            x: 2,
            y: 4,
            w: canvas.w.saturating_sub(4),
            h: 3,
        };
        canvas.row_style(bar, Some((44, 44, 44)), false);
        for (index, &byte) in prompt.text.iter().enumerate() {
            let x = bar.x + 3 + index;
            let typed = typing.typed.get(index);
            let shown = match (byte, typed) {
                (b' ', Some(&typed)) if typed != b' ' => typed as char,
                (b' ', _) => '·',
                _ => byte as char,
            };
            let color = match typed {
                Some(&typed) if typed == byte => GREEN,
                Some(_) => RED,
                None if byte == b' ' => MUTED,
                None if prompt.targets.iter().any(|&(a, _)| a == index) => YELLOW,
                None if prompt.targets.iter().any(|&(_, b)| b == index) => BLUE,
                None => FG,
            };
            canvas.put(x, bar.y + 1, shown, color);
        }
        if !rating {
            let cursor = Rect {
                x: bar.x + 3 + typing.typed.len(),
                y: bar.y + 1,
                w: 1,
                h: 1,
            };
            canvas.row_style(cursor, Some((110, 110, 110)), false);
        }
        if rating {
            canvas.text(2, 8, &format!("How much do you dislike {first} → {second}?"), FG);
            let mut x = 2;
            for (level, name) in SCISSOR_LEVELS.iter().enumerate() {
                canvas.text(x, 10, &level.to_string(), CYAN);
                canvas.text(x + 2, 10, name, FG);
                x += name.chars().count() + 5;
            }
            canvas.text(
                2,
                12,
                &format!(
                    "r type again   w other words   Tab skip{}   Esc save and leave",
                    if undoable { "   u undo last rating" } else { "" }
                ),
                MUTED,
            );
        } else {
            canvas.text(
                2,
                8,
                "Type the text on your keyboard. Backspace fixes mistakes.",
                MUTED,
            );
            canvas.text(2, 9, "Tab skip pair   Esc save and leave", MUTED);
        }
        scissor_keyboard(&mut canvas, 14, layout, (pair.first, pair.second));
        term.present(&canvas, 0)?;
        match term.event()? {
            Event::Quit | Event::Escape => return Ok(ScissorStep::Leave),
            Event::Tab => return Ok(ScissorStep::Skip),
            Event::Char(ch @ '0'..='4') if rating => {
                let rating = ch as usize - '0' as usize;
                let line = scissor_record(
                    layout_name, layout, pass, pair, &prompt, &typing, attempts, rating,
                );
                log.lines.push(((pair.first, pair.second), line));
                log.save()?;
                return Ok(ScissorStep::Rated);
            }
            Event::Char('r') if rating => {
                typing = ScissorTyping::new(prompt.text.len());
                attempts += 1;
            }
            Event::Char('w') if rating => {
                if let Some(next) = scissor_next_prompt(layout, effort, pair) {
                    prompt = next;
                }
                typing = ScissorTyping::new(prompt.text.len());
                attempts = 1;
            }
            Event::Char('u') if rating && undoable => return Ok(ScissorStep::Undo),
            Event::Backspace if !rating => typing.erase(),
            Event::Char(ch) if !rating && ch.is_ascii() => typing.push(&prompt.text, ch as u8),
            _ => {}
        }
    }
}

fn scissor_test(term: &mut Terminal) -> AppResult<()> {
    let paths = discover_layouts(Path::new(LAYOUT_DIR))?;
    let names: Vec<_> = paths.iter().map(|path| layout_file_label(path)).collect();
    let Some(index) = search_menu(term, "Scissor test: layout you type on", &names)? else {
        return Ok(());
    };
    let path = &paths[index];
    let layout = action_keys::Layout::parse(&fs::read_to_string(path)?, path)?;
    let layout_name = path
        .file_stem()
        .map_or_else(|| "layout".into(), |stem| stem.to_string_lossy().into_owned());

    let mut canvas = Canvas::new(term.width(), term.size.1.max(12));
    canvas.text(2, 1, "Finding words for each key pair …", MUTED);
    term.present(&canvas, 0)?;
    let (_, vocabulary) = scissor_source()?;
    let weights = load_weights(Path::new(WEIGHTS_FILE))?;
    let effort = crate::action_ui::LocalEffort::new(&layout, &weights);
    let keys = scissor_pairs(&layout);
    let words = scissor_words(&layout, &effort, &vocabulary, &keys);
    let mut pairs: Vec<ScissorPair> = keys
        .iter()
        .zip(words)
        .map(|(&(first, second), words)| ScissorPair {
            first,
            second,
            words,
            next: 0,
        })
        .collect();
    // Pairs no word or drill presses cannot be tested.
    let found = pairs.len();
    pairs.retain_mut(|pair| {
        let reachable = scissor_next_prompt(&layout, &effort, pair).is_some();
        pair.next = 0;
        reachable
    });
    let unreachable = found - pairs.len();
    if pairs.is_empty() {
        return info_page(term, "Scissor test", &["This layout has no scissor pairs to type.".into()]);
    }

    let file = Path::new(SCISSOR_DIR).join(format!("{layout_name}.csv"));
    let mut log = ScissorLog::load(file)?;
    // A pass rates every pair once; pairs rated least often come first.
    let least = pairs
        .iter()
        .map(|pair| log.count((pair.first, pair.second)))
        .min()
        .unwrap_or(0);
    let pass = least + 1;
    let mut queue: Vec<usize> = (0..pairs.len())
        .filter(|&index| log.count((pairs[index].first, pairs[index].second)) == least)
        .collect();
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(1, |time| time.as_nanos() as u64);
    let mut rng = Rng::new(seed);
    for index in (1..queue.len()).rev() {
        queue.swap(index, rng.index(index + 1));
    }
    let mut rated: Vec<usize> = Vec::new();
    let mut skipped = 0;
    let total = pairs.len();
    let done_before = total - queue.len();
    while let Some(&index) = queue.first() {
        let step = scissor_trial(
            term,
            &layout_name,
            &layout,
            &effort,
            &mut pairs[index],
            (done_before + rated.len(), total, pass),
            &mut log,
            !rated.is_empty(),
        )?;
        match step {
            ScissorStep::Rated => {
                rated.push(queue.remove(0));
            }
            ScissorStep::Skip => {
                queue.remove(0);
                skipped += 1;
            }
            ScissorStep::Undo => {
                let last = rated.pop().expect("undo follows a rating");
                log.lines.pop();
                log.save()?;
                queue.insert(0, last);
            }
            ScissorStep::Leave => break,
        }
        if term.quitting {
            break;
        }
    }
    if term.quitting {
        return Ok(());
    }
    let mut lines = vec![
        format!("Rated {} pairs this session; {skipped} skipped.", rated.len()),
        format!("Ratings are saved in {}.", log.path.display()),
    ];
    if unreachable > 0 {
        lines.push(format!("{unreachable} pairs could not be typed with words or a drill."));
    }
    if queue.is_empty() && skipped == 0 {
        lines.push(format!("Pass {pass} is complete; the next test starts pass {}.", pass + 1));
    } else {
        lines.push("The next test continues this pass with the pairs not yet rated.".into());
    }
    info_page(term, "Scissor test", &lines)
}

#[cfg(test)]
mod scissor_tests {
    use super::*;

    fn qwerty() -> action_keys::Layout {
        action_keys::Layout::parse(
            "q w e r t | y u i o p\na s d f g | h j k l ;\nz x c v b | n m , . /\nthumbs: space\n",
            Path::new("scissor-test.dat"),
        )
        .unwrap()
    }

    fn slot(layout: &action_keys::Layout, label: &str) -> usize {
        layout.slots.iter().position(|slot| slot.label == label).unwrap()
    }

    #[test]
    fn pairs_are_same_hand_cross_row_and_include_the_inner_index_column() {
        let layout = qwerty();
        let pairs = scissor_pairs(&layout);
        // Per hand: 15 keys, less same-finger and same-row pairs, both orders.
        assert_eq!(pairs.len(), 216);
        let has = |a: &str, b: &str| pairs.contains(&(slot(&layout, a), slot(&layout, b)));
        assert!(has("e", "x") && has("x", "e") && has("t", "x") && has("o", "m"));
        assert!(!has("e", "r"), "same row");
        assert!(!has("r", "b"), "same finger");
        assert!(!has("e", "m"), "other hand");
        assert!(scissor_inner(&layout, slot(&layout, "t")));
        assert!(scissor_inner(&layout, slot(&layout, "h")));
        assert!(!scissor_inner(&layout, slot(&layout, "f")));
        assert!(!scissor_inner(&layout, slot(&layout, "j")));
        assert_eq!(scissor_key_name(&layout, slot(&layout, "b")), "left index (inner) bottom");
    }

    #[test]
    fn prompts_press_their_pair_with_words_first_then_drills() {
        let layout = qwerty();
        let weights = Weights::default();
        let effort = crate::action_ui::LocalEffort::new(&layout, &weights);
        let pairs = scissor_pairs(&layout);
        let vocabulary: Vec<Vec<u8>> = ["excel", "next", "sexy", "text", "first", "worst"]
            .iter()
            .map(|word| word.as_bytes().to_vec())
            .collect();
        let words = scissor_words(&layout, &effort, &vocabulary, &pairs);
        let ex = pairs
            .iter()
            .position(|&pair| pair == (slot(&layout, "e"), slot(&layout, "x")))
            .unwrap();
        assert_eq!(words[ex], ["excel", "next", "sexy", "text"]);
        for (index, &(first, second)) in pairs.iter().enumerate() {
            let mut pair = ScissorPair {
                first,
                second,
                words: words[index].clone(),
                next: 0,
            };
            let prompt = scissor_next_prompt(&layout, &effort, &mut pair).unwrap();
            assert!(prompt.targets.len() >= SCISSOR_WORDS);
            let label = |slot: usize| layout.slots[slot].label.as_bytes()[0];
            for &(a, b) in &prompt.targets {
                assert_eq!((prompt.text[a], prompt.text[b]), (label(first), label(second)));
            }
            if words[index].is_empty() {
                let drill = [label(first), label(second)];
                assert_eq!(prompt.text, [&drill[..], b" ", &drill, b" ", &drill].concat());
            }
        }
        // Too few words are filled with the drill: "first" and "worst" press r, s.
        let rs = pairs
            .iter()
            .position(|&pair| pair == (slot(&layout, "r"), slot(&layout, "s")))
            .unwrap();
        let mut pair = ScissorPair {
            first: pairs[rs].0,
            second: pairs[rs].1,
            words: words[rs].clone(),
            next: 0,
        };
        let prompt = scissor_next_prompt(&layout, &effort, &mut pair).unwrap();
        assert_eq!(prompt.text, b"first worst rs");
        assert_eq!(prompt.targets, [(2, 3), (8, 9), (12, 13)]);
    }

    #[test]
    fn typing_times_only_first_tries_and_counts_errors() {
        let text = b"ab ab";
        let mut typing = ScissorTyping::new(text.len());
        for &byte in b"ab a" {
            typing.push(text, byte);
            std::thread::sleep(Duration::from_millis(2));
        }
        typing.push(text, b'x');
        assert!(!typing.done(text));
        typing.erase();
        typing.push(text, b'b');
        assert!(typing.done(text));
        assert_eq!(typing.errors, 1);
        // The second ab was corrected, so only the first is timed.
        let first = typing.median_ms(&[(0, 1)]).unwrap();
        assert!(first >= 2.0);
        assert_eq!(typing.median_ms(&[(0, 1), (3, 4)]), Some(first));
        assert_eq!(typing.median_ms(&[(3, 4)]), None);
    }

    #[test]
    fn records_match_the_header_and_survive_a_save() {
        let layout = qwerty();
        let weights = Weights::default();
        let effort = crate::action_ui::LocalEffort::new(&layout, &weights);
        let (first, second) = (slot(&layout, "o"), slot(&layout, ","));
        let mut pair = ScissorPair {
            first,
            second,
            words: Vec::new(),
            next: 0,
        };
        let prompt = scissor_next_prompt(&layout, &effort, &mut pair).unwrap();
        let typing = ScissorTyping::new(prompt.text.len());
        let line = scissor_record("q,\"w\"", &layout, 2, &pair, &prompt, &typing, 1, 3);
        let fields = scissor_csv_fields(&line);
        let header: Vec<_> = SCISSOR_HEADER.split(',').collect();
        assert_eq!(fields.len(), header.len());
        let field = |name: &str| &fields[header.iter().position(|&known| known == name).unwrap()];
        assert_eq!(field("layout"), "q,\"w\"");
        assert_eq!(field("second_label"), ",");
        assert_eq!(field("words"), "o, o, o,");
        assert_eq!(field("direction"), "down");
        assert_eq!(field("dy"), "2");
        assert_eq!(field("lower_finger"), "RM");
        assert_eq!(field("rating"), "3");
        assert_eq!(field("level"), "uncomfortable");
        let keys = crate::action_ui::physical_keys(&layout);
        assert_eq!(
            field("mana2"),
            &format!("{:.4}", mana2_metrics::scissor(keys[first], keys[second]))
        );

        let directory = std::env::temp_dir().join(format!("akler-scissors-{}", crate::process_tag()));
        let mut log = ScissorLog::load(directory.join("qwerty.csv")).unwrap();
        log.lines.push(((first, second), line.clone()));
        log.lines.push(((first, second), line));
        log.save().unwrap();
        let loaded = ScissorLog::load(directory.join("qwerty.csv")).unwrap();
        assert_eq!(loaded.count((first, second)), 2);
        assert_eq!(loaded.count((second, first)), 0);
        fs::remove_dir_all(directory).unwrap();
    }
}
