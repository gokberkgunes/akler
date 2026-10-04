// Compact, cell-based terminal UI. Rendering and mouse hitboxes share the
// same rectangles; wheel scrolling and resizing cannot desynchronize them.
const PANEL_WIDTH: usize = 108;

const DISPLAY_DECIMALS: usize = 2;

const DETAIL_DECIMALS: usize = 4;

const DISPLAY_EPSILON: f64 = 1e-9;

const FG: u8 = 7;

const MUTED: u8 = 8;

const BORDER: u8 = 8;

const BLUE: u8 = 4;

const CYAN: u8 = 6;

const GREEN: u8 = 2;

const RED: u8 = 1;

const YELLOW: u8 = 3;

// Soft accents make neighboring metric families easy to scan without
// competing with the red/green change colors used for their values.
const GROUP_TINTS: [u8; 6] = [72, 73, 74, 75, 76, 77];

fn group_tint(group: usize) -> u8 {
    GROUP_TINTS[group % GROUP_TINTS.len()]
}

// Dedicated grayscale IDs, separate from the existing score gradient (16..=64).
fn finger_tint(finger: usize) -> u8 {
    [70, 71, 70, 71, 71, 70, 71, 70, 70, 71][finger.min(9)]
}

fn stagger_cells(offset: i16, minimum: i16, step: usize) -> usize {
    (((i32::from(offset) - i32::from(minimum)).max(0) as usize * step) + 500) / 1000
}

fn saved_layout_message(path: &Path) -> String {
    format!(
        "saved {} and {}",
        path.with_extension("dat").display(),
        path.with_extension("jsonc").display(),
    )
}

fn ansi_color(color: u8) -> String {
    if matches!(color, 70 | 71) {
        let value = if color == 70 { 155 } else { 230 };
        return format!("\x1b[38;2;{value};{value};{value}m");
    }
    if (72..=77).contains(&color) {
        let (r, g, b) = [
            (170, 200, 225),
            (190, 215, 190),
            (220, 205, 170),
            (205, 185, 220),
            (175, 215, 215),
            (220, 185, 195),
        ][usize::from(color - 72)];
        return format!("\x1b[38;2;{r};{g};{b}m");
    }
    if color >= 16 {
        let t = (color - 16).min(48) as f64 / 48.0;
        let (r, g, b) = if t <= 0.5 {
            let u = t * 2.0;
            (255.0, 64.0 + u * 160.0, 64.0 *(1.0 - u))
        } else {
            let u = (t - 0.5) * 2.0;
            (255.0 *(1.0 - u), 224.0 + u * 6.0, u * 96.0)
        };
        return format!("\x1b[38;2;{};{};{}m", r.round() as u8, g.round() as u8, b.round() as u8);
    }
    match color {
        0 => "\x1b[30m",
        1 => "\x1b[31m",
        2 => "\x1b[32m",
        3 => "\x1b[33m",
        4 => "\x1b[34m",
        5 => "\x1b[35m",
        6 => "\x1b[36m",
        8 => "\x1b[90m",
        _ => "\x1b[39m"
    }.into()
}

fn ansi_background(background: Option<(u8, u8, u8)>) -> String {
    match background {
        Some((r, g, b)) => format!("\x1b[48;2;{r};{g};{b}m"),
        None => "\x1b[49m".into(),
    }
}

fn gradient(t: f64) -> u8 {
    16 +(if t.is_finite() {
        t.clamp(0.0, 1.0)
    } else {
        0.5
    }
        * 48.0).round() as u8
}

#[derive(Clone,Copy,PartialEq,Eq)]
struct Cell {
    ch: char,
    color: u8,
    background: Option<(u8, u8, u8)>,
    bold: bool,
}

#[derive(Clone,Copy,Debug)]
struct Rect {
    x: usize,
    y: usize,
    w: usize,
    h: usize
}

impl Rect {
    fn contains(self, x: usize, y: usize) -> bool {
        x >= self.x && x<self.x + self.w && y >= self.y && y<self.y + self.h
    }
}

#[derive(Clone,Copy,Debug)]
enum Action {
    Key(usize),
    Metric(usize),
    Weight(usize),
    Mana2Weight(usize),
    SimpleWeight(usize),
    SimpleSpeedSetting(usize),
    Setting(usize),
    Item(usize),
    Command(char),
}

struct Canvas {
    w: usize,
    h: usize,
    cells: Vec<Cell>,
    hits: Vec<(Rect, Action)>
}

impl Canvas {
    fn new(w: usize, h: usize) -> Self {
        let w = w.min(PANEL_WIDTH);
        Self {
            w,
            h,
            cells: vec![Cell {
                ch: ' ',
                color: FG,
                background: None,
                bold: false,
            }; w * h],
            hits: Vec::new()
        }
    }

    fn optimizer(w: usize, h: usize) -> Self {
        Self::new(w, h)
    }

    fn ranking(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            cells: vec![Cell {
                ch: ' ',
                color: FG,
                background: None,
                bold: false,
            }; w * h],
            hits: Vec::new()
        }
    }

    fn put(&mut self, x: usize, y: usize, ch: char, color: u8) {
        if x >= self.w {
            return;
        }
        if y >= self.h {
            self.cells.resize((y + 1) * self.w, Cell {
                ch: ' ',
                color: FG,
                background: None,
                bold: false,
            });
            self.h = y + 1;
        }
        let cell = &mut self.cells[y * self.w + x];
        cell.ch = ch;
        cell.color = color;
    }

    fn text(&mut self, x: usize, y: usize, s: &str, color: u8) {
        for (i, ch) in clean_text(s).chars().enumerate() {
            self.put(x + i, y, ch, color);
        }
    }

    fn right(&mut self, x: usize, y: usize, w: usize, s: &str, color: u8) {
        let text = if s.chars().count()>w {
            "…"
        } else {
            s
        };
        self.text(x + w.saturating_sub(text.chars().count()), y, text, color);
    }

    fn center(&mut self, x: usize, y: usize, w: usize, s: &str, color: u8) {
        let s = short(s, w);
        self.text(x + w.saturating_sub(s.chars().count()) / 2, y, &s, color);
    }

    fn line(&mut self, x: usize, y: usize, w: usize, color: u8) {
        for i in 0..w {
            self.put(x + i, y, '─', color);
        }
    }

    fn row_style(&mut self, r: Rect, background: Option<(u8, u8, u8)>, bold: bool) {
        for y in r.y..(r.y + r.h).min(self.h) {
            for x in r.x..(r.x + r.w).min(self.w) {
                let cell = &mut self.cells[y * self.w + x];
                cell.background = background;
                cell.bold = bold;
            }
        }
    }

    fn boxed(&mut self, r: Rect, color: u8) {
        if r.w<2 || r.h<2 {
            return;
        }
        self.line(r.x, r.y, r.w, color);
        self.line(r.x, r.y + r.h - 1, r.w, color);
        for y in r.y + 1..r.y + r.h - 1 {
            self.put(r.x, y, '│', color);
            self.put(r.x + r.w - 1, y, '│', color);
        }
        for (x, y, c) in[
            (r.x, r.y, '┌'),
            (r.x + r.w - 1, r.y, '┐'),
            (r.x, r.y + r.h - 1, '└'),
            (r.x + r.w - 1, r.y + r.h - 1, '┘')
        ] {
            self.put(x, y, c, color);
        }
    }

    fn hit(&mut self, r: Rect, a: Action) {
        self.hits.push((r, a));
    }

    fn action(&self, x: usize, y: usize) -> Option<Action> {
        self.hits.iter().rev().find(|(r, _) | r.contains(x, y)).map(|(_, a)|*a)
    }

    fn button(&mut self, x: usize, y: usize, text: &str, cmd: char) {
        let s = format!("[{text}]");
        self.text(x, y, &s, CYAN);
        self.hit(Rect {
            x,
            y,
            w: s.chars().count(),
            h: 1
        }, Action::Command(cmd));
    }
}

#[derive(Clone,Debug,PartialEq,Eq)]
enum Event {
    Tick,
    Char(char),
    Enter,
    Escape,
    Quit,
    Backspace,
    Clear,
    Redo,
    Tab,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Mouse {
        x: usize,
        y: usize,
        button: u8,
        release: bool,
        motion: bool
    },
    Wheel(i32),
    Paste(String)
}

#[derive(Default)]
struct Decoder {
    pending: Vec<u8>,
    discard_paste: bool
}

impl Decoder {
    fn push(&mut self, b: &[u8]) {
        self.pending.extend_from_slice(b);
        if self.pending.len()>65536 {
            if self.pending.starts_with(b"\x1b[200~") || self.discard_paste {
                // Never turn an oversized paste into command keystrokes.
                self.discard_paste = true;
                if !self.pending.windows(6).any(|w| w == b"\x1b[201~") {
                    let tail = self.pending.split_off(self.pending.len() - 5);
                    self.pending = tail;
                }
            } else {
                self.pending.clear();
            }
        }
    }

    fn next(&mut self, timed_out: bool) -> Option<Event> {
        if self.discard_paste {
            if let Some(end) = self.pending.windows(6).position(|w| w == b"\x1b[201~") {
                self.pending.drain(..end + 6);
                self.discard_paste = false;
                return Some(Event::Paste(String::new()));
            }
            return None;
        }
        if self.pending.is_empty() {
            return None;
        }
        let b = self.pending[0];
        if b != 27 {
            self.pending.remove(0);
            return Some(match b {
                3 | 4 => Event::Quit,
                13 | 10 => Event::Enter,
                127 | 8 => Event::Backspace,
                21 => Event::Clear,
                18 => Event::Redo,
                9 => Event::Tab,
                32..=126 => Event::Char(b as char),
                _ => Event::Tick,
            });
        }
        if self.pending.len() == 1 {
            if timed_out {
                self.pending.clear();
                return Some(Event::Escape);
            }
            return None;
        }
        if self.pending[1] != b'[' && self.pending[1] != b'O' {
            self.pending.remove(0);
            return Some(Event::Escape);
        }
        if self.pending.starts_with(b"\x1b[200~") {
            let end = self.pending.windows(6).position(|w| w == b"\x1b[201~");
            if let Some(end) = end {
                let s = String::from_utf8_lossy(&self.pending[6..end]).to_string();
                self.pending.drain(..end + 6);
                return Some(Event::Paste(s));
            }
            return None;
        }
        let end = (2..self.pending.len()).find(|&i |(0x40..=0x7e).contains(&self.pending[i]));
        let Some(end) = end else {
            if timed_out {
                self.pending.clear();
                return Some(Event::Tick);
            }
            return None;
        };
        let seq: Vec<u8> = self.pending.drain(..=end).collect();
        let last = seq[seq.len() - 1];
        if seq.starts_with(b"\x1b[<") {
            let text = std::str::from_utf8(&seq[3..seq.len() - 1]).ok()?;
            let v: Vec<usize> = text.split(';').filter_map(|s| s.parse().ok()).collect();
            if v.len() != 3 {
                return Some(Event::Tick);
            }
            let cb = v[0] as u8;
            if cb&64 != 0 {
                return Some(Event::Wheel(if cb&1 == 0 {
                    -3
                } else {
                    3
                }));
            }
            return Some(Event::Mouse {
                x: v[1].saturating_sub(1),
                y: v[2].saturating_sub(1),
                button: cb&3,
                release: last == b'm',
                motion: cb&32 != 0
            });
        }
        Some(match last {
            b'A' => Event::Up,
            b'B' => Event::Down,
            b'C' => Event::Right,
            b'D' => Event::Left,
            b'H' => Event::Home,
            b'F' => Event::End,
            b'~' => match &seq[2..seq.len() - 1] {
                b"5" => Event::PageUp,
                b"6" => Event::PageDown,
                b"1" | b"7" => Event::Home,
                b"4" | b"8" => Event::End,
                _ => Event::Tick
            },
            _ => Event::Tick,
        })
    }
}

struct Terminal {
    tty: File,
    saved: String,
    decoder: Decoder,
    size:(usize, usize),
    size_at: Instant,
    previous: Vec<Cell>,
    offset_x: usize,
    precise: bool,
    quitting: bool,
    restart_requested: bool,
    _reports: crate::profile_output::TerminalReports,
}

impl Terminal {
    fn open() -> AppResult<Self> {
        let tty = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
        let out = Command::new("stty").arg("-g").stdin(tty.try_clone()?).output()?;
        if !out.status.success() {
            return Err("cannot read terminal state from /dev/tty".into());
        }
        let saved = String::from_utf8(out.stdout)?.trim().to_string();
        let reports = crate::profile_output::TerminalReports::begin();
        let status = Command::new("stty").args(["raw", "-echo", "min", "0", "time", "1"]).stdin(tty.try_clone()?).status()?;
        if !status.success() {
            return Err("cannot enter terminal raw mode".into());
        }
        let mut t = Self {
            tty,
            saved,
            decoder: Decoder::default(),
            size:(80, 24),
            size_at: Instant::now() - Duration::from_secs(2),
            previous: Vec::new(),
            offset_x: 0,
            precise: false,
            quitting: false,
            restart_requested: false,
            _reports: reports,
        };
        // Alternate screen, SGR drag events, bracketed paste, cursor off, wrap off.
        t.tty.write_all(b"\x1b[?1049h\x1b[?1002h\x1b[?1006h\x1b[?2004h\x1b[?25l\x1b[?7l")?;
        t.refresh_size()?;
        Ok(t)
    }

    fn refresh_size(&mut self) -> AppResult<()> {
        if self.size_at.elapsed()<Duration::from_millis(500) {
            return Ok(());
        }
        self.size_at = Instant::now();
        let out = Command::new("stty").arg("size").stdin(self.tty.try_clone()?).output()?;
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            let v: Vec<usize> = s.split_whitespace().filter_map(|v| v.parse().ok()).collect();
            if v.len() == 2 && v[0]>0 && v[1]>0 {
                let size = (v[1].min(1000), v[0].min(500));
                if size != self.size {
                    self.previous.clear();
                    self.size = size;
                }
            }
        }
        Ok(())
    }

    fn request_restart(&mut self) -> AppResult<()> {
        restart_program_path()?;
        self.restart_requested = true;
        self.quitting = true;
        Ok(())
    }

    fn width(&self) -> usize {
        self.size.0.clamp(64, PANEL_WIDTH)
    }

    fn decimals(&self) -> usize {
        if self.precise {
            DETAIL_DECIMALS
        } else {
            DISPLAY_DECIMALS
        }
    }

    fn event(&mut self) -> AppResult<Event> {
        if self.quitting {
            return Ok(Event::Quit);
        }
        if let Some(e) = self.decoder.next(false) {
            return Ok(self.visible_event(e));
        }
        let mut buf = [0; 4096];
        let n = match self.tty.read(&mut buf) {
            Ok(n) => n,
            Err(e)if e.kind() == io::ErrorKind::Interrupted => 0,
            Err(e) => return Err(e.into())
        };
        if n>0 {
            self.decoder.push(&buf[..n]);
        }
        self.refresh_size()?;
        let event = self.decoder.next(n == 0).unwrap_or(Event::Tick);
        Ok(self.visible_event(event))
    }

    fn visible_event(&mut self, e: Event) -> Event {
        if e == Event::Quit {
            self.quitting = true;
        }
        if (self.size.0<64 || self.size.1<12) && !matches!(e, Event::Escape | Event::Quit | Event::Char('q')) {
            Event::Tick
        } else {
            e
        }
    }

    fn present(&mut self, c: &Canvas, scroll: usize) -> AppResult<()> {
        self.refresh_size()?;
        let (w, h) = self.size;
        let scroll = scroll.min(c.h.saturating_sub(h));
        self.offset_x = 0;
        let mut view = vec![Cell {
            ch: ' ',
            color: FG,
            background: None,
            bold: false,
        }; w * h];
        for y in 0..h {
            if y + scroll >= c.h {
                break;
            }
            for x in 0..c.w.min(w) {
                view[y * w + x + self.offset_x] = c.cells[(y + scroll) * c.w + x];
            }
        }
        if w<64 || h<12 {
            view.fill(Cell {
                ch: ' ',
                color: FG,
                background: None,
                bold: false,
            });
            for (i, ch) in "Resize terminal to at least 64 columns x 12 rows. Esc to back out.".chars().take(w).enumerate() {
                view[i] = Cell {
                    ch,
                    color: YELLOW,
                    background: None,
                    bold: false,
                };
            }
        }
        let mut out = String::new();
        if self.previous.len() != view.len() {
            out.push_str("\x1b[2J");
            self.previous.clear();
        }
        for y in 0..h {
            if self.previous.len() == view.len() && self.previous[y * w..(y + 1) * w] == view[y * w..(y + 1) * w] {
                continue;
            }
            out.push_str(&format!("\x1b[{};1H", y + 1));
            let mut color = 255;
            let mut background = None;
            let mut bold = None;
            for cell in &view[y * w..(y + 1) * w] {
                if bold != Some(cell.bold) {
                    out.push_str(if cell.bold { "\x1b[1m" } else { "\x1b[22m" });
                    bold = Some(cell.bold);
                }
                if background != Some(cell.background) {
                    out.push_str(&ansi_background(cell.background));
                    background = Some(cell.background);
                }
                let shown = cell.color;
                if color != shown {
                    out.push_str(&ansi_color(shown));
                    color = shown;
                }
                out.push(cell.ch);
            }
        }
        out.push_str("\x1b[0m");
        self.tty.write_all(out.as_bytes())?;
        self.tty.flush()?;
        self.previous = view;
        Ok(())
    }

    fn hit(&self, c: &Canvas, x: usize, y: usize, scroll: usize) -> Option<Action> {
        if x<self.offset_x {
            return None;
        }
        c.action(x - self.offset_x, y + scroll.min(c.h.saturating_sub(self.size.1)))
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // The reports guard is dropped after this body restores the terminal.
        let _ = self.tty.write_all(b"\x1b[0m\x1b[?1002l\x1b[?1000l\x1b[?1006l\x1b[?2004l\x1b[?25h\x1b[?7h\x1b[?1049l");
        let _ = self.tty.flush();
        if let Ok(f) = self.tty.try_clone() {
            let _ = Command::new("stty").arg(&self.saved).stdin(f).status();
        }
    }
}

fn scroll_event(e: &Event, scroll: &mut usize, total: usize, height: usize) -> bool {
    let delta = match e {
        Event::Wheel(n)=>*n,
        Event::PageUp=>-(height as i32 - 2).max(1),
        Event::PageDown => (height as i32 - 2).max(1),
        _ => return false
    };
    * scroll = ( * scroll as i32 + delta).max(0) as usize;
    * scroll = ( * scroll).min(total.saturating_sub(height));
    true
}

fn action_press(term: &Terminal, c: &Canvas, e: &Event, scroll: usize) -> Option<Action> {
    if let Event::Mouse {
        x,
        y,
        button: 0,
        release: false,
        motion: false
    } = e {
        term.hit(c, * x, * y, scroll)
    } else {
        None
    }
}

fn direction(e: &Event) -> Option<(i32, i32)> {
    match e {
        Event::Left | Event::Char('h') => Some((0, -1)),
        Event::Right | Event::Char('l') => Some((0, 1)),
        Event::Up | Event::Char('k') => Some((-1, 0)),
        Event::Down | Event::Char('j') => Some((1, 0)),
        _ => None
    }
}

fn move_cursor(cursor: usize, dr: i32, dc: i32, keys: &[Key]) -> usize {
    let Some(&current) = keys.get(cursor) else {
        return 0;
    };
    let mut candidates = Vec::new();
    if dc != 0 {
        for (i, k) in keys.iter().enumerate() {
            if k.main != current.main {
                continue;
            }
            if current.main && k.row == current.row &&(k.col - current.col).signum() == dc as i8 {
                candidates.push((i,(k.col - current.col).abs()));
            } else if !current.main &&(k.col - current.col).signum() == dc as i8 {
                candidates.push((i,(k.col - current.col).abs()));
            }
        }
    } else if current.main {
        let target = current.row + dr as i8;
        if (0..=2).contains(&target) {
            for (i, k) in keys.iter().enumerate() {
                if k.main && k.row == target {
                    candidates.push((i,(k.col - current.col).abs()));
                }
            }
        } else if target>2 {
            for (i, k) in keys.iter().enumerate() {
                if !k.main && k.hand == current.hand {
                    candidates.push((i, (k.col - current.col).abs()));
                }
            }
        }
    } else if dr<0 {
        let target = if current.hand == 0 {
            4
        } else {
            5
        };
        for (i, k) in keys.iter().enumerate() {
            if k.main && k.row == 2 && k.hand == current.hand {
                candidates.push((i,(k.col - target).abs()));
            }
        }
    }
    candidates.into_iter().min_by_key(|v| v.1).map(|v| v.0).unwrap_or(cursor)
}

fn change_color(m: usize, before: f64, after: f64) -> u8 {
    let d = if higher_better(m) {
        after - before
    } else {
        before - after
    };
    if d>DISPLAY_EPSILON {
        GREEN
    } else if d< -DISPLAY_EPSILON {
        RED
    } else {
        FG
    }
}

fn delta_color(m: usize, before: f64, after: f64) -> u8 {
    if (after - before).abs() <= DISPLAY_EPSILON {
        MUTED
    } else {
        change_color(m, before, after)
    }
}

fn header(c: &mut Canvas, title: &str, corpus: &str, layout: &str, controls: &str) {
    c.text(0, 0, title, FG);
    let context = [corpus, layout].into_iter().filter(|text| !text.is_empty())
        .collect::<Vec<_>>().join(" · ");
    let x = title.chars().count() + 2;
    c.text(
        x,
        0,
        &short(&context, c.w.saturating_sub(x)),
        CYAN
    );
    c.text(0, 1, &short(controls, c.w), MUTED);
}

fn right_main_column(keys: &[Key], occupied: impl Fn(usize) -> bool) -> i8 {
    let min = keys
        .iter()
        .filter(|key| key.main)
        .map(|key| key.col)
        .min()
        .unwrap_or(0);
    let max = keys
        .iter()
        .filter(|key| key.main)
        .map(|key| key.col)
        .max()
        .unwrap_or(9);
    let default = min + if max - min + 1 == 12 { 6 } else { 5 };
    let left = keys
        .iter()
        .enumerate()
        .filter(|(i, key)| key.main && key.hand == 0 && occupied(*i))
        .map(|(_, key)| key.col)
        .max();
    let right = keys
        .iter()
        .enumerate()
        .filter(|(i, key)| key.main && key.hand == 1 && occupied(*i))
        .map(|(_, key)| key.col)
        .min();
    match (left, right) {
        (Some(left), Some(right)) if left < right => right,
        _ => default,
    }
}

fn keyboard(
    c: &mut Canvas,
    y: usize,
    model: &Model,
    arr: &[usize],
    baseline: &[usize],
    locks: Option<&[bool]>,
    cursor: Option<usize>,
    highlight: &[usize]
) -> usize {
    let min = model.board.keys.iter().filter(|k| k.main).map(|k| k.col).min().unwrap_or(0);
    let max = model.board.keys.iter().filter(|k| k.main).map(|k| k.col).max().unwrap_or(9);
    let cols = (max - min + 1) as usize;
    let right_col = right_main_column(&model.board.keys, |i| !blank(model.board.symbols[i]));
    let min_offset = model.board.keys.iter().filter(|k| k.main).map(|k| k.row_offset).min().unwrap_or(0).min(0);
    let max_offset = model.board.keys.iter().filter(|k| k.main).map(|k| k.row_offset).max().unwrap_or(0).max(0);
    let min_column_offset = model.board.keys.iter().filter(|k| k.main).map(|k| k.column_offset).min().unwrap_or(0).min(0);
    let max_column_offset = model.board.keys.iter().filter(|k| k.main).map(|k| k.column_offset).max().unwrap_or(0).max(0);
    let column_height = stagger_cells(max_column_offset, min_column_offset, 3);
    let gap = 3;
    let fits=|kw: usize | cols *(kw + 1) - 1 + gap + stagger_cells(max_offset, min_offset, kw + 1) <= c.w;
    let kw: usize = if fits(7) {
        7
    } else if fits(5) {
        5
    } else {
        4
    };
    let step = kw + 1;
    let width = step * cols - 1 + gap + stagger_cells(max_offset, min_offset, step);
    let x = c.w.saturating_sub(width) / 2;
    let left_thumbs = model.board.keys.iter().filter(|key| !key.main && key.hand == 0).count();
    let left_start = (x + width / 2).saturating_sub(kw + 2 + left_thumbs.saturating_sub(1) * (kw + 3));
    let right_start = x + width / 2 + 1;
    let mut thumb_index = [0; 2];
    for i in 0..arr.len() {
        let key = model.board.keys[i];
        let (kx, ky) = if key.main {
            (
                x + (key.col - min) as usize * step
                    + stagger_cells(key.row_offset, min_offset, step)
                    + usize::from(key.col >= right_col) * gap,
                y + key.row as usize * 3 + stagger_cells(key.column_offset, min_column_offset, 3),
            )
        } else {
            let hand = key.hand as usize;
            let start = if hand == 0 { left_start } else { right_start };
            let position = (start + thumb_index[hand] * (kw + 3), y + 9 + column_height);
            thumb_index[hand] += 1;
            position
        };
        let locked = locks.map(|v| v[i]).unwrap_or(false);
        let border = if locked {
            YELLOW
        } else {
            finger_tint(key.finger)
        };
        let color = if arr[i] != baseline[i] {
            BLUE
        } else if locked {
            YELLOW
        } else {
            finger_tint(key.finger)
        };
        let r = Rect {
            x: kx,
            y: ky,
            w: kw,
            h: 3
        };
        c.boxed(r, border);
        let symbol = model.canonical[arr[i]];
        let label = if symbol == b' ' { "SP".to_owned() } else { display_symbol(symbol).to_string() };
        c.center(kx + 1, ky + 1, kw - 2, &label, color);
        // Keyboard/click selection is indicated without changing ANSI color.
        if highlight.contains(&i) || cursor == Some(i) {
            c.put(kx + 1, ky + 1, '›', FG);
        }
        c.hit(r, Action::Key(i));
    }
    y + column_height + if model.board.keys.iter().any(| k|!k.main) {
        12
    } else {
        9
    }
}

fn fixed(value: f64, decimals: usize) -> String {
    if !value.is_finite() {
        return "n/a".into();
    }
    let rounded = format!("{:.*}", decimals, value);
    if rounded.starts_with('-') && rounded[1..].chars().all(|c| c == '0' || c == '.') {
        rounded[1..].to_string()
    } else {
        rounded
    }
}

fn number(value: f64, decimals: usize) -> String {
    let text = fixed(value, decimals);
    if value>DISPLAY_EPSILON && text == fixed(0.0, decimals) {
        format!("<{}", fixed(10.0f64.powi(-(decimals as i32)), decimals))
    } else {
        text
    }
}

fn delta_text(before: f64, after: f64, decimals: usize) -> String {
    let d = (after - before).abs();
    if d <= DISPLAY_EPSILON {
        "—".into()
    } else {
        number(d, decimals)
    }
}

fn fmt2(value: f64) -> String {
    number(value, DISPLAY_DECIMALS)
}

fn config_number(value: f64) -> String {
    if value != 0.0 && value.abs()<0.01 {
        let decimals = (1.0 - value.abs().log10().floor()).max(2.0).min(12.0) as usize;
        fixed(value, decimals)
    } else {
        fixed(value, DISPLAY_DECIMALS)
    }
}

static OPT_GROUP_SAME: [usize; 4] = [SFB, SFS, SKB, SKS];

static OPT_GROUP_FULL: [usize; 4] = [DAFJB, DAFJS, CAFJB, CAFJS];

static OPT_GROUP_HALF: [usize; 4] = [DAHJB, DAHJS, CAHJB, CAHJS];

static OPT_GROUP_HALF_NONADJ: [usize; 4] = [DNHJB, DNHJS, CNHJB, CNHJS];

static OPT_GROUP_ADJACENT: [usize; 4] = [FSB, FSS, HSB, HSS];

static OPT_GROUP_STRETCH: [usize; 2] = [LSB, LSS];

static OPT_GROUP_NONADJ: [usize; 4] = [DNFJB, DNFJS, CNFJB, CNFJS];

static OPT_GROUP_RHYTHM: [usize; 3] = [REDIR, WRED, WISH];

static OPT_GROUP_PREF: [usize; 3] = [SRAF, ROLL, ALT];

static OPT_GROUP_SRAF_DIRECTIONS: [usize; 2] = [INSRAF, OUTSRAF];

static OPT_GROUP_ROLL_TOTALS: [usize; 2] = [INROLL, OUTROLL];

static OPT_GROUP_ROLL_TYPES: [usize; 4] = [IN2, OUT2, IN3, OUT3];

static OPT_GROUP_TRAVEL: [usize; 4] = [TRAVEL, VTRAVEL, LTRAVEL, SFTRAVEL];

static OPT_METRIC_GROUPS: [(&str, &[usize]); 13] = [
    ("Same finger", &OPT_GROUP_SAME),
    ("Adjacent full", &OPT_GROUP_FULL),
    ("Adjacent half", &OPT_GROUP_HALF),
    ("Nonadj. half", &OPT_GROUP_HALF_NONADJ),
    ("Adjacent D", &OPT_GROUP_ADJACENT),
    ("Stretch", &OPT_GROUP_STRETCH),
    ("Nonadj. full", &OPT_GROUP_NONADJ),
    ("Rhythm", &OPT_GROUP_RHYTHM),
    ("Preferences", &OPT_GROUP_PREF),
    ("SRAF directions", &OPT_GROUP_SRAF_DIRECTIONS),
    ("Roll directions", &OPT_GROUP_ROLL_TOTALS),
    ("Roll types", &OPT_GROUP_ROLL_TYPES),
    ("Travel u/100", &OPT_GROUP_TRAVEL),
];

// Display families are independent of the optimizer's weight controls.
// Keep the adjacent discordant aliases (FSB/FSS and HSB/HSS) in their families.
static TABLE_FULL_JUMPS: [usize; 10] = [DAFJB, DAFJS, CAFJB, CAFJS, DNFJB, DNFJS, CNFJB, CNFJS, FSB, FSS];

static TABLE_HALF_JUMPS: [usize; 10] = [DAHJB, DAHJS, CAHJB, CAHJS, DNHJB, DNHJS, CNHJB, CNHJS, HSB, HSS];

static EDITOR_HAND_PATTERNS: [usize; 6] = [REDIR, WRED, WISH, ALT, SRAF, ROLL];

static TABLE_HAND_PATTERNS: [usize; 14] = [
    REDIR, WRED, WISH, ALT, SRAF, ROLL, INSRAF, OUTSRAF, INROLL, OUTROLL, IN2, OUT2, IN3, OUT3,
];

static TABLE_GROUPS: [(&str, &[usize]); 6] = [
    ("Same finger", &OPT_GROUP_SAME),
    ("Full jumps", &TABLE_FULL_JUMPS),
    ("Half jumps", &TABLE_HALF_JUMPS),
    ("Stretch", &OPT_GROUP_STRETCH),
    ("Hand patterns", &TABLE_HAND_PATTERNS),
    ("Travel u/100", &OPT_GROUP_TRAVEL),
];

// Editors favor a compact overview. Directional SRAF and roll totals remain
// available in the detailed reports and optimizer views.
static EDITOR_TABLE_GROUPS: [(&str, &[usize]); 6] = [
    ("Same finger", &OPT_GROUP_SAME),
    ("Full jumps", &TABLE_FULL_JUMPS),
    ("Half jumps", &TABLE_HALF_JUMPS),
    ("Stretch", &OPT_GROUP_STRETCH),
    ("Hand patterns", &EDITOR_HAND_PATTERNS),
    ("Travel u/100", &OPT_GROUP_TRAVEL),
];

struct MetricGrid {
    x: usize,
    width: usize,
    label: usize,
    cell: usize,
    cols: usize,
    name: usize,
    value: usize,
    delta: usize,
    rows: usize
}

impl MetricGrid {
    fn new(
        w: usize,
        values: &[String],
        deltas: &[String],
        decimals: usize,
        groups: &[(&str, &[usize])],
        left_aligned: bool
    ) -> Self {
        let value = values.iter().map(|s| s.chars().count()).max().unwrap_or(0).max(decimals + 4);
        let delta = deltas.iter().map(|s| s.chars().count()).max().unwrap_or(0).max(decimals + 3);
        // Size both label columns from the text actually rendered. Long group
        // names must leave the separator intact, including in detailed views.
        let label = groups.iter().map(|(name, _)|name.chars().count() + 2).max().unwrap_or(0)
            .max(if left_aligned { 14 } else { 13 });
        let name = groups.iter().flat_map(|(_, items)|items.iter())
            .map(|&m|metric_short_name(m).chars().count()).max().unwrap_or(7).max(7);
        let cell = 1 + name + 1 + value + 2 + delta + 1;
        let fit = w.saturating_sub(label + 2) /(cell + 1);
        // Keep bigrams on the left and their skipgrams on the right, even
        // when the terminal could fit additional columns.
        let cols = if fit >= 2 {
            2
        } else {
            1
        };
        let rows = groups.iter().map(|(_, g) |(g.len() + cols - 1) / cols).sum::<usize>()
            + groups.len().saturating_sub(1);
        let width = label + 2 + cols *(cell + 1);
        Self {
            x: if left_aligned && width<w { 1 } else { w.saturating_sub(width) / 2 },
            width,
            label,
            cell,
            cols,
            name,
            value,
            delta,
            rows
        }
    }
}

fn grouped_metric_cards(
    c: &mut Canvas,
    y: usize,
    before: &Metrics,
    after: &Metrics,
    raw: &Raw,
    corpus: &Corpus,
    decimals: usize
) -> usize {
    grouped_metric_totals(c, y, before, after, raw, &corpus.totals, decimals)
}

fn editor_metric_cards(
    c: &mut Canvas,
    y: usize,
    before: &Metrics,
    after: &Metrics,
    raw: &Raw,
    corpus: &Corpus,
    decimals: usize
) -> usize {
    grouped_metric_table(c, y, before, after, raw, &corpus.totals, decimals, &EDITOR_TABLE_GROUPS, true, true)
}

fn grouped_metric_totals(
    c: &mut Canvas,
    y: usize,
    before: &Metrics,
    after: &Metrics,
    raw: &Raw,
    totals: &[f64; 4],
    decimals: usize,
) -> usize {
    grouped_metric_table(c, y, before, after, raw, totals, decimals, &TABLE_GROUPS, false, true)
}

fn grouped_metric_table(
    c: &mut Canvas,
    y: usize,
    before: &Metrics,
    after: &Metrics,
    raw: &Raw,
    totals: &[f64; 4],
    decimals: usize,
    groups: &[(&str, &[usize])],
    left_aligned: bool,
    clickable: bool
) -> usize {
    let values: Vec<String> = (0..N_METRICS).map(|m| if denominator_totals(m, raw, totals)>0.0 {
        metric_value_text(m, after.v[m], decimals)
    } else {
        "n/a".into()
    }).collect();
    let deltas: Vec<String> = (0..N_METRICS).map(|m| if denominator_totals(m, raw, totals)>0.0 {
        delta_text(before.v[m], after.v[m], decimals)
    } else {
        "—".into()
    }).collect();
    let grid = MetricGrid::new(c.w, &values, &deltas, decimals, groups, left_aligned);
    let h = grid.rows + 2;
    c.boxed(Rect {
        x: grid.x,
        y,
        w: grid.width,
        h
    }, BORDER);
    for col in 0..grid.cols {
        let xx = grid.x + 1 + grid.label + col *(grid.cell + 1);
        c.put(xx, y, '┬', BORDER);
        c.put(xx, y + h - 1, '┴', BORDER);
        for yy in y + 1..y + h - 1 {
            c.put(xx, yy, '│', BORDER);
        }
    }
    let mut row = 0;
    for (group_index, &(group, items)) in groups.iter().enumerate() {
        if group_index > 0 {
            let yy = y + 1 + row;
            c.line(grid.x, yy, grid.width, BORDER);
            c.put(grid.x, yy, '├', BORDER);
            c.put(grid.x + grid.width - 1, yy, '┤', BORDER);
            for col in 0..grid.cols {
                let xx = grid.x + 1 + grid.label + col *(grid.cell + 1);
                c.put(xx, yy, '┼', BORDER);
            }
            row += 1;
        }
        let tint = group_tint(group_index);
        c.text(grid.x + 2, y + 1 + row, &short(group, grid.label.saturating_sub(2)), tint);
        for (i, &m) in items.iter().enumerate() {
            let xx = grid.x + 2 + grid.label +(i % grid.cols) *(grid.cell + 1);
            let yy = y + 1 + row + i / grid.cols;
            let vx = xx + 1 + grid.name + 1;
            c.text(xx + 1, yy, &short(metric_short_name(m), grid.name), tint);
            c.right(vx, yy, grid.value, &values[m], if values[m] == "n/a" {
                MUTED
            } else {
                change_color(m, before.v[m], after.v[m])
            });
            c.right(vx + grid.value + 2, yy, grid.delta, &deltas[m], delta_color(m, before.v[m], after.v[m]));
            if clickable {
                c.hit(Rect {
                    x: xx,
                    y: yy,
                    w: grid.cell,
                    h: 1
                }, Action::Metric(m));
            }
        }
        row +=(items.len() + grid.cols - 1) / grid.cols;
    }
    y + h
}

fn weight_label(i: usize) -> &'static str {
    if i<N_METRICS {
        METRIC_NAMES[i]
    } else {
        ["PINKY", "RING", "MIDDLE", "INDEX"][(i - N_METRICS) % 4]
    }
}

fn grouped_weight_rows(c: &mut Canvas, mut y: usize, w: &Weights) -> usize {
    c.text(0, y, &format!("Weights · {}", roll_settings_label(w.rolls())), FG);
    y += 1;
    let groups: [(&str, &[usize]); 10] = [
        ("Same finger", &OPT_GROUP_SAME),
        ("Adjacent full", &OPT_GROUP_FULL),
        ("Half jumps", &OPT_GROUP_HALF),
        ("Stretch", &OPT_GROUP_STRETCH),
        ("Nonadj. full", &OPT_GROUP_NONADJ),
        ("Rhythm", &OPT_GROUP_RHYTHM),
        ("Preferences", &OPT_GROUP_PREF),
        ("Travel", &OPT_GROUP_TRAVEL),
        ("Off-home", &[N_METRICS, N_METRICS + 1, N_METRICS + 2, N_METRICS + 3]),
        (
            "Finger usage",
            &[N_METRICS + 4, N_METRICS + 5, N_METRICS + 6, N_METRICS + 7],
        ),
    ];
    for (group_index, (group, items)) in groups.into_iter().enumerate() {
        let tint = group_tint(group_index);
        c.text(0, y, group, tint);
        let mut x = 16usize;
        for &i in items {
            let name = weight_label(i);
            let value = config_number(w.0[i]);
            let width = (name.len() + 1 + value.len() + 2).max(12);
            if x + width>c.w && x>16 {
                y += 1;
                x = 16;
            }
            c.text(x, y, name, tint);
            c.text(x + name.len() + 1, y, &value, CYAN);
            c.hit(Rect {
                x,
                y,
                w: width.min(c.w.saturating_sub(x)),
                h: 1
            }, Action::Weight(i));
            x += width;
        }
        y += 1;
    }
    y
}

fn mana2_weight_rows(c: &mut Canvas, mut y: usize, weights: &mana2_metrics::Weights) -> usize {
    c.text(0, y, "Mana2 weights · click to edit; s saves", FG);
    y += 1;
    c.text(
        0,
        y,
        &short(
            "[weight, boundary, ..., weight] · negative penalizes; positive rewards",
            c.w,
        ),
        MUTED,
    );
    y += 1;
    let groups = [
        ("Finger usage", 0..10),
        ("Pinky", 10..12),
        ("Bigrams", 12..17),
        ("Skipgrams", 17..22),
        ("Alternations", 22..26),
        ("Redirects", 26..34),
        ("Rolls", 34..mana2_metrics::N_STATS),
    ];
    let left = if c.w >= 64 { 16 } else { 0 };
    for (group_index, (group, indices)) in groups.into_iter().enumerate() {
        let tint = group_tint(group_index);
        c.text(0, y, group, tint);
        if left == 0 {
            y += 1;
        }
        let mut x = left;
        for index in indices {
            let name = mana2_metrics::STAT_IDS[index];
            let value = weights.schedule_text(index);
            let width = name.len() + 1 + value.len() + 2;
            if x > left && x + width > c.w {
                y += 1;
                x = left;
            }
            let available = c.w.saturating_sub(x);
            c.text(x, y, &short(name, available), tint);
            if available > name.len() + 1 {
                c.text(
                    x + name.len() + 1,
                    y,
                    &short(&value, available - name.len() - 1),
                    CYAN,
                );
            }
            c.hit(
                Rect {
                    x,
                    y,
                    w: width.min(available),
                    h: 1,
                },
                Action::Mana2Weight(index),
            );
            x += width;
        }
        y += 1;
    }
    y
}

fn simple_weight_rows(c: &mut Canvas, mut y: usize, weights: &simple_metrics::Weights) -> usize {
    c.text(0, y, "Simple weights · click to edit; s saves", FG);
    y += 1;
    c.text(
        0,
        y,
        "Positive penalizes; negative rewards · lower score is better",
        MUTED,
    );
    y += 1;
    let left = if c.w >= 64 { 16 } else { 0 };
    c.text(0, y, "Metrics", group_tint(0));
    if left == 0 {
        y += 1;
    }
    let mut x = left;
    for index in 0..simple_metrics::N_STATS {
        let name = simple_metrics::STAT_IDS[index];
        let value = config_number(weights.get(index));
        let width = name.len() + 1 + value.len() + 2;
        if x > left && x + width > c.w {
            y += 1;
            x = left;
        }
        let available = c.w.saturating_sub(x);
        c.text(x, y, &short(name, available), group_tint(0));
        if available > name.len() + 1 {
            c.text(
                x + name.len() + 1,
                y,
                &short(&value, available - name.len() - 1),
                CYAN,
            );
        }
        c.hit(
            Rect {
                x,
                y,
                w: width.min(available),
                h: 1,
            },
            Action::SimpleWeight(index),
        );
        x += width;
    }
    y += 2;
    c.text(0, y, "Speed settings", group_tint(1));
    if left == 0 {
        y += 1;
    }
    let mut x = left;
    for (index, name) in simple_metrics::SPEED_SETTING_IDS.iter().enumerate() {
        let value = config_number(weights.get_speed_setting(index));
        let width = name.len() + 1 + value.len() + 2;
        if x > left && x + width > c.w {
            y += 1;
            x = left;
        }
        let available = c.w.saturating_sub(x);
        c.text(x, y, &short(name, available), group_tint(1));
        if available > name.len() + 1 {
            c.text(
                x + name.len() + 1,
                y,
                &short(&value, available - name.len() - 1),
                CYAN,
            );
        }
        c.hit(
            Rect {
                x,
                y,
                w: width.min(available),
                h: 1,
            },
            Action::SimpleSpeedSetting(index),
        );
        x += width;
    }
    y + 1
}

fn simple_stats_rows(
    c: &mut Canvas,
    mut y: usize,
    before: Option<&mana2_metrics::Stats>,
    after: &mana2_metrics::Stats,
    weights: &simple_metrics::Weights,
    decimals: usize,
) -> usize {
    let before_values = before.map(|stats| simple_metrics::values(stats, weights));
    let after_values = simple_metrics::values(after, weights);
    let columns = if c.w >= 80 { 2 } else { 1 };
    let column_width = c.w / columns;
    for index in 0..simple_metrics::N_STATS {
        // Mana2's stretch and scissor terms are geometry ratings; the other
        // Simple terms are normalized frequencies in percentage points.
        let suffix = if (2..=5).contains(&index) || index >= 16 {
            ""
        } else {
            "%"
        };
        let text = if let Some(values) = before_values.as_ref() {
            format!(
                "{} {}{} → {}{}",
                simple_metrics::LABELS[index],
                number(values[index], decimals),
                suffix,
                number(after_values[index], decimals),
                suffix,
            )
        } else {
            format!(
                "{} {}{}",
                simple_metrics::LABELS[index],
                number(after_values[index], decimals),
                suffix,
            )
        };
        let x = index % columns * column_width;
        let row = y + index / columns;
        c.text(x, row, &short(&text, column_width.saturating_sub(1)), FG);
    }
    y += (simple_metrics::N_STATS + columns - 1) / columns;
    y
}

fn simple_speed_table(
    c: &mut Canvas,
    mut y: usize,
    before: Option<&mana2_metrics::Stats>,
    after: &mana2_metrics::Stats,
    weights: &simple_metrics::Weights,
    decimals: usize,
) -> usize {
    const LEFT: [usize; 5] = [0, 1, 2, 3, 8];
    const RIGHT: [usize; 5] = [7, 6, 5, 4, 9];
    const USAGE_IDS: [&str; 10] = [
        "finger-usage-LP",
        "finger-usage-LR",
        "finger-usage-LM",
        "finger-usage-LI",
        "finger-usage-RI",
        "finger-usage-RM",
        "finger-usage-RR",
        "finger-usage-RP",
        "finger-usage-LT",
        "finger-usage-RT",
    ];

    let before_speeds = before.map(|stats| simple_metrics::speeds(stats, weights));
    let after_speeds = simple_metrics::speeds(after, weights);
    let usage = |stats: &mana2_metrics::Stats, index: usize| {
        stats
            .get(USAGE_IDS[index])
            .expect("Simple finger usage stat is defined")
    };
    let value = |before: Option<f64>, after: f64| {
        if let Some(before) = before {
            format!("{}→{}", number(before, decimals), number(after, decimals))
        } else {
            number(after, decimals)
        }
    };
    let row = |indices: &[usize], label: &str| {
        let after_usage: f64 = indices.iter().map(|&i| usage(after, i)).sum();
        let after_fspeed: f64 = indices.iter().map(|&i| after_speeds.per_finger[i]).sum();
        let after_weighted: f64 = indices.iter().map(|&i| after_speeds.weighted[i]).sum();
        let before_usage = before.map(|stats| indices.iter().map(|&i| usage(stats, i)).sum());
        let before_fspeed = before_speeds
            .as_ref()
            .map(|speeds| indices.iter().map(|&i| speeds.per_finger[i]).sum::<f64>());
        let before_weighted = before_speeds
            .as_ref()
            .map(|speeds| indices.iter().map(|&i| speeds.weighted[i]).sum::<f64>());
        (
            label.to_string(),
            value(before_usage, after_usage),
            value(before_fspeed, after_fspeed),
            value(before_weighted, after_weighted),
        )
    };
    let finger_row = |index: usize| row(&[index], FINGER_NAMES[index]);

    let mut left = LEFT.map(finger_row).to_vec();
    let mut right = RIGHT.map(finger_row).to_vec();
    left.push(row(&LEFT, "Left"));
    right.push(row(&RIGHT, "Right"));
    let both = row(&(0..10).collect::<Vec<_>>(), "Both");

    let before_width = if before.is_some() { 13 } else { 8 };
    let side_width = 7 + before_width * 3;
    let render_header = |c: &mut Canvas, x: usize, y: usize| {
        c.text(x, y, "Finger", MUTED);
        c.right(x + 7, y, before_width - 1, "Usage %", MUTED);
        c.right(x + 7 + before_width, y, before_width - 1, "Fspeed", MUTED);
        c.right(
            x + 7 + before_width * 2,
            y,
            before_width - 1,
            "Weighted",
            MUTED,
        );
    };
    let render_row = |c: &mut Canvas,
                      x: usize,
                      y: usize,
                      entry: &(String, String, String, String),
                      color: u8| {
        c.text(x, y, &entry.0, color);
        c.right(x + 7, y, before_width - 1, &entry.1, FG);
        c.right(x + 7 + before_width, y, before_width - 1, &entry.2, FG);
        c.right(x + 7 + before_width * 2, y, before_width - 1, &entry.3, FG);
    };

    c.text(
        0,
        y,
        "Finger speed · available bigrams + skip1",
        group_tint(1),
    );
    y += 1;
    if c.w >= side_width * 2 + 3 {
        render_header(c, 0, y);
        render_header(c, side_width + 3, y);
        y += 1;
        for row_index in 0..left.len() {
            let left_color = if row_index < 5 {
                finger_tint(LEFT[row_index])
            } else {
                MUTED
            };
            let right_color = if row_index < 5 {
                finger_tint(RIGHT[row_index])
            } else {
                MUTED
            };
            render_row(c, 0, y, &left[row_index], left_color);
            render_row(c, side_width + 3, y, &right[row_index], right_color);
            y += 1;
        }
    } else {
        render_header(c, 0, y);
        y += 1;
        for &index in LEFT.iter().chain(RIGHT.iter()) {
            let entry = finger_row(index);
            render_row(c, 0, y, &entry, finger_tint(index));
            y += 1;
        }
        render_row(c, 0, y, &row(&LEFT, "Left"), MUTED);
        y += 1;
        render_row(c, 0, y, &row(&RIGHT, "Right"), MUTED);
        y += 1;
    }
    render_row(c, 0, y, &both, CYAN);
    y + 1
}

fn simple_score_line(
    c: &mut Canvas,
    y: usize,
    before: Option<f64>,
    after: f64,
    objective: Option<f64>,
    decimals: usize,
) -> usize {
    let score = if let Some(before) = before {
        format!(
            "Simple score {} → {} (lower is better)",
            number(before, decimals),
            number(after, decimals),
        )
    } else {
        format!("Simple score {} (lower is better)", number(after, decimals))
    };
    c.text(0, y, &short(&score, c.w), CYAN);
    if let Some(objective) = objective {
        c.text(
            0,
            y + 1,
            &short(
                &format!(
                    "Training objective {} (weighted corpus mixture; lower is better)",
                    number(objective, decimals),
                ),
                c.w,
            ),
            MUTED,
        );
        y + 2
    } else {
        y + 1
    }
}

fn finger_table(c: &mut Canvas, y: usize, before: &Metrics, after: &Metrics, decimals: usize) -> usize {
    let label = 8usize;
    let mut fw = decimals + 4;
    for i in 0..8 {
        for text in [
            number(after.usage[i], decimals),
            number(after.off[i], decimals),
            delta_text(before.off[i], after.off[i], decimals)
        ] {
            fw = fw.max(text.chars().count() + 2);
        }
    }
    let cols = if label + 2 + 8 *(fw + 1) <= c.w {
        8
    } else {
        4
    };
    let blocks = 8 / cols;
    let width = label + 2 + cols *(fw + 1);
    let x = c.w.saturating_sub(width) / 2;
    let h = 2 + blocks * 4;
    c.boxed(Rect {
        x,
        y,
        w: width,
        h
    }, BORDER);
    for col in 0..cols {
        let xx = x + 1 + label + col *(fw + 1);
        c.put(xx, y, '┬', BORDER);
        c.put(xx, y + h - 1, '┴', BORDER);
        for yy in y + 1..y + h - 1 {
            c.put(xx, yy, '│', BORDER);
        }
    }
    for block in 0..blocks {
        let yy = y + 1 + block * 4;
        c.text(x + 1, yy + 1, "usage %", MUTED);
        c.text(x + 1, yy + 2, "off %", MUTED);
        c.text(x + 1, yy + 3, "change", MUTED);
        for col in 0..cols {
            let i = block * cols + col;
            let xx = x + 2 + label + col *(fw + 1);
            c.center(xx, yy, fw, FINGER_NAMES[i], MUTED);
            c.right(xx + 1, yy + 1, fw - 2, &number(after.usage[i], decimals), CYAN);
            c.right(
                xx + 1,
                yy + 2,
                fw - 2,
                &number(after.off[i], decimals),
                change_color(0, before.off[i], after.off[i])
            );
            c.right(
                xx + 1,
                yy + 3,
                fw - 2,
                &delta_text(before.off[i], after.off[i], decimals),
                delta_color(0, before.off[i], after.off[i])
            );
        }
    }
    c.text(
        x,
        y + h,
        &format!("LT {}%   RT {}%", number(after.usage[8], decimals), number(after.usage[9], decimals)),
        MUTED
    );
    y + h + 1
}

fn score_panel(
    c: &mut Canvas,
    y: usize,
    before: &Breakdown,
    after: &Breakdown,
    ensemble: Option<(f64, f64)>,
    decimals: usize
) -> usize {
    let mut items = vec![
        ("Penalty", before.penalty, after.penalty, false),
        ("Credit", before.bonus, after.bonus, true),
        ("Objective", before.net, after.net, false)
    ];
    if let Some((b, a)) = ensemble {
        items.push(("Mix", b, a, false));
    }
    let mut x = 0;
    let mut yy = y;
    for (name, b, a, credit) in items {
        let left = format!("{name} {} → ", fixed(b, decimals));
        let right = fixed(a, decimals);
        let len = left.chars().count() + right.chars().count();
        if x>0 && x + 5 + len>c.w {
            yy += 1;
            x = 0;
        }
        if x>0 {
            c.text(x, yy, "  │  ", BORDER);
            x += 5;
        }
        let color = if (a - b).abs() <= DISPLAY_EPSILON {
            FG
        } else if (a<b)^credit {
            GREEN
        } else {
            RED
        };
        c.text(x, yy, &left, MUTED);
        x += left.chars().count();
        c.text(x, yy, &right, color);
        x += right.chars().count();
    }
    yy + 1
}

fn wrap_lines(lines: &[String], width: usize) -> Vec<String> {
    let mut result = Vec::new();
    for line in lines {
        if line.chars().count() <= width {
            result.push(line.clone());
            continue;
        }
        let mut text = String::new();
        for word in line.split_whitespace() {
            if !text.is_empty() && text.chars().count() + 1 + word.chars().count()>width {
                result.push(text);
                text = String::new();
            }
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(word);
        }
        if !text.is_empty() {
            result.push(text);
        }
    }
    result
}

fn info_page(term: &mut Terminal, title: &str, lines: &[String]) -> AppResult<()> {
    let mut scroll = 0;
    loop {
        let lines = wrap_lines(lines, term.width());
        let mut c = Canvas::new(term.width(), lines.len() + 3);
        c.text(0, 0, title, FG);
        for (i, line) in lines.iter().enumerate() {
            c.text(0, i + 2, &short(line, c.w), if line.starts_with("ERROR") {
                RED
            } else {
                FG
            });
        }
        term.present(&c, scroll)?;
        let e = term.event()?;
        if scroll_event(&e, &mut scroll, c.h, term.size.1) {
            continue;
        }
        if matches!(e, Event::Escape | Event::Quit | Event::Char('q') | Event::Enter) {
            return Ok(());
        }
    }
}

fn help_popup_frame(
    base: &Canvas,
    title: &str,
    lines: &[String],
    size: (usize, usize),
    scroll: &mut usize,
) -> (Canvas, usize, usize) {
    let (w, h) = (size.0.max(1), size.1.max(1));
    let mut c = Canvas::ranking(w, h);
    for y in 0..h.min(base.h) {
        for x in 0..w.min(base.w) {
            let mut cell = base.cells[y * base.w + x];
            cell.color = MUTED;
            cell.bold = false;
            c.cells[y * w + x] = cell;
        }
    }
    let width = w.saturating_sub(4).max(4).min(80).min(w);
    let lines = wrap_lines(lines, width.saturating_sub(4).max(1));
    let height = (lines.len() + 4).min(h.saturating_sub(2)).max(4).min(h);
    let capacity = height.saturating_sub(4);
    *scroll = (*scroll).min(lines.len().saturating_sub(capacity));
    let r = Rect { x: (w - width) / 2, y: (h - height) / 2, w: width, h: height };
    for y in r.y..r.y + r.h {
        for x in r.x..r.x + r.w {
            c.cells[y * w + x] = Cell {
                ch: ' ', color: FG, background: None, bold: false,
            };
        }
    }
    c.boxed(r, BORDER);
    c.text(r.x + 2, r.y, &short(title, width.saturating_sub(4)), CYAN);
    for (index, line) in lines.iter().skip(*scroll).take(capacity).enumerate() {
        c.text(r.x + 2, r.y + 2 + index, &short(line, width.saturating_sub(4)), FG);
    }
    if height >= 4 {
        c.text(r.x + 2, r.y + height - 2,
            &short("↑↓ scroll · Esc / ? close", width.saturating_sub(4)), MUTED);
    }
    (c, capacity, lines.len())
}

pub(crate) fn help_popup(term: &mut Terminal, base: &Canvas, title: &str, lines: &[String]) -> AppResult<()> {
    let mut scroll = 0usize;
    // Use the visible frame, including the caller's scrolling, as the backdrop.
    let snapshot = if term.previous.len() == term.size.0 * term.size.1 {
        Some(Canvas {
            w: term.size.0, h: term.size.1,
            cells: term.previous.clone(), hits: Vec::new(),
        })
    } else { None };
    let base = snapshot.as_ref().unwrap_or(base);
    loop {
        term.refresh_size()?;
        let (c, capacity, total) = help_popup_frame(base, title, lines, term.size, &mut scroll);
        term.present(&c, 0)?;
        match term.event()? {
            Event::Escape | Event::Quit | Event::Enter | Event::Char('q' | '?') => return Ok(()),
            Event::Up | Event::Char('k') => scroll = scroll.saturating_sub(1),
            Event::Down | Event::Char('j') => scroll = scroll.saturating_add(1),
            Event::PageUp => scroll = scroll.saturating_sub(capacity),
            Event::PageDown => scroll = scroll.saturating_add(capacity),
            Event::Wheel(delta) => scroll = (scroll as i64 + i64::from(delta)).max(0) as usize,
            Event::Home => scroll = 0,
            Event::End => scroll = total.saturating_sub(capacity),
            _ => {}
        }
    }
}

fn input_box(term: &mut Terminal, title: &str, hint: &str, initial: &str) -> AppResult<Option<String>> {
    let mut text = initial.to_string();
    let mut replace = true;
    loop {
        let mut c = Canvas::new(term.width(), 8);
        c.text(0, 0, title, FG);
        let _ = hint;
        c.boxed(Rect {
            x: 0,
            y: 2,
            w: c.w,
            h: 3
        }, BORDER);
        c.text(2, 3, &short(&format!("{text}▏"), c.w.saturating_sub(4)), FG);
        term.present(&c, 0)?;
        match term.event()? {
            Event::Enter => return Ok(Some(text.trim().to_string())),
            Event::Escape | Event::Quit => return Ok(None),
            Event::Clear => {
                text.clear();
                replace = false;
            },
            Event::Backspace => {
                if replace {
                    text.clear();
                    replace = false;
                } else {
                    text.pop();
                }
            },
            Event::Char(ch) => {
                if replace {
                    text.clear();
                    replace = false;
                }
                if text.len()<256 {
                    text.push(ch);
                }
            },
            Event::Paste(s) => {
                if replace {
                    text.clear();
                    replace = false;
                }
                text.extend(s.chars().filter(| c|!c.is_control()).take(256 - text.len().min(256)));
            },
            _ => {
            },
        }
    }
}

fn confirm_box(term: &mut Terminal, title: &str, subject: &str, verb: &str) -> AppResult<bool> {
    let mut accept = false;
    loop {
        let mut c = Canvas::new(term.width(), 7);
        c.text(0, 0, title, FG);
        c.text(0, 2, &short(subject, c.w), MUTED);
        for (i, name) in [verb, "Cancel"].iter().enumerate() {
            let x = i *(verb.len().max(6) + 8);
            let selected = if i == 0 {
                accept
            } else {
                !accept
            };
            c.text(x, 4, if selected {
                "›"
            } else {
                " "
            }, FG);
            let text = format!("[{name}]");
            c.text(x + 2, 4, &text, FG);
            c.hit(Rect {
                x,
                y: 4,
                w: text.len() + 2,
                h: 1
            }, Action::Command(if i == 0 {
                'y'
            } else {
                'n'
            }));
        }
        term.present(&c, 0)?;
        let e = term.event()?;
        if let Some(Action::Command(ch)) = action_press(term, &c, &e, 0) {
            return Ok(ch == 'y');
        }
        match e {
            Event::Enter => return Ok(accept),
            Event::Char('y') => return Ok(true),
            Event::Escape | Event::Quit | Event::Char('q') | Event::Char('n') => return Ok(false),
            Event::Left => accept = true,
            Event::Right => accept = false,
            Event::Tab => accept=!accept,
            _ => {
            }
        }
    }
}

fn menu(term: &mut Terminal, title: &str, items: &[String]) -> AppResult<Option<usize>> {
    if items.is_empty() {
        return Ok(None);
    }
    let mut selected = 0usize;
    let mut query = String::new();
    let mut scroll = 0usize;
    loop {
        let filtered: Vec<usize> = items.iter().enumerate().filter(|(_, s) | s.to_ascii_lowercase().contains(&query.to_ascii_lowercase())).map(|(i, _) | i).collect();
        if selected >= filtered.len() {
            selected = filtered.len().saturating_sub(1);
        }
        let top = if query.is_empty() {
            2
        } else {
            3
        };
        let mut c = Canvas::new(term.width(), filtered.len() + top + 1);
        c.text(0, 0, title, FG);
        if !query.is_empty() {
            c.text(0, 1, &format!("/{}", query), BLUE);
        }
        for (row, &i) in filtered.iter().enumerate() {
            let yy = row + top;
            let active = row == selected;
            c.text(1, yy, if active {
                "›"
            } else {
                " "
            }, if active {
                BLUE
            } else {
                MUTED
            });
            c.text(3, yy, &short(&items[i], c.w.saturating_sub(4)), if active {
                BLUE
            } else {
                FG
            });
            c.hit(Rect {
                x: 0,
                y: yy,
                w: c.w,
                h: 1
            }, Action::Item(i));
        }
        c.text(0, filtered.len() + top, "? help", MUTED);
        term.present(&c, scroll)?;
        let e = term.event()?;
        if scroll_event(&e, &mut scroll, c.h, term.size.1) {
            continue;
        }
        if let Some(Action::Item(i)) = action_press(term, &c, &e, scroll) {
            return Ok(Some(i));
        }
        match e {
            Event::Enter => if let Some(&i) = filtered.get(selected) {
                return Ok(Some(i));
            },
            Event::Escape | Event::Quit | Event::Char('q') => return Ok(None),
            Event::Char('R') if title == "akler" => {
                match term.request_restart() {
                    Ok(()) => return Ok(None),
                    Err(error) => help_popup(term, &c, "Reload program", &[error.to_string()])?,
                }
            },
            Event::Char('?') => {
                let mut lines = vec![
                    "j/k or ↑/↓: select; Enter or click: open.".into(),
                    "/: filter; mouse wheel / PgUp / PgDn: scroll.".into(),
                    "q or Esc: back.".into(),
                ];
                if title == "akler" {
                    lines.push("R or Reload program: restart into the rebuilt executable.".into());
                }
                help_popup(term, &c, title, &lines)?;
            },
            Event::Up | Event::Char('k') => selected = selected.saturating_sub(1),
            Event::Down | Event::Char('j') => selected = (selected + 1).min(filtered.len().saturating_sub(1)),
            Event::Char('/') => {
                if let Some(s) = input_box(term, "Filter", "", &query)? {
                    query = s;
                    selected = 0;
                    scroll = 0;
                }
            },
            _ => {
            }
        }
        if selected + top<scroll {
            scroll = selected + top;
        } else if selected + top >= scroll + term.size.1 {
            scroll = selected + top + 1 - term.size.1;
        }
    }
}

fn choose_path(
    term: &mut Terminal,
    dir: &str,
    ext: &str,
    prefix: &str,
    title: &str
) -> AppResult<Option<PathBuf>> {
    let paths = if dir == CORPUS_DIR {
        corpus_paths()?
    } else if dir == LAYOUT_DIR {
        discover_layouts(Path::new(dir))?
    } else {
        discover(dir, ext, prefix)?
    };
    let names: Vec<_> = paths.iter().map(|path| {
        if dir == LAYOUT_DIR { layout_file_label(path) } else { label(path, prefix) }
    }).collect();
    Ok(menu(term, title, &names)?.map(|i| paths[i].clone()))
}

fn show_corpus_info(term: &mut Terminal, c: &Corpus) -> AppResult<()> {
    let mut lines = vec![
        format!("Model: {MODEL_VERSION}"),
        format!("Corpus: {}   content fingerprint {:016x}", c.name, c.fingerprint),
        String::new(),
        "Coverage of STORED corpus tables (not the original raw text):".into()
    ];
    for (i, name) in["Letters", "Bigrams", "Skipgrams", "Trigrams"].iter().enumerate() {
        lines.push(format!("{name:<12} {:8.4}%  denominator mass {:.6}", c.coverage[i], c.totals[i]));
    }
    lines.extend([
            String::new(),
            "B/S and finger usage normalize over mapped symbols, including Space.".into(),
            "SRAF and rhythm normalize over mapped NON-THUMB n-grams; rejected candidates remain in the denominator.".into(),
            "Every layout is evaluated independently; no key-set intersection.".into(),
            "A low coverage layout is not directly comparable without checking omissions.".into(),
            String::new()
        ]);
    lines.extend(c.warnings.iter().cloned());
    info_page(term, "Corpus / normalization", &lines)
}

// Shared physical-contribution plot; no scoring or aggregation happens here.
fn contribution_bar(
    c: &mut Canvas,
    x: usize,
    y: usize,
    width: usize,
    before: f64,
    after: f64,
    maximum: f64,
    color: u8
) {
    if width == 0 {
        return;
    }
    let old = (before / maximum *(width - 1) as f64).round() as usize;
    let new = (after / maximum *(width - 1) as f64).round() as usize;
    c.line(x, y, width, BORDER);
    if after > 0.0 {
        c.line(x, y, new + 1, color);
    }
    if before > 0.0 {
        c.put(x + old, y, '○', MUTED);
    }
    if after > 0.0 {
        c.put(x + new, y, '●', color);
    }
}

fn contributor_view(
    term: &mut Terminal,
    m: usize,
    model: &Model,
    before: &[usize],
    after: &[usize],
    corpus: &Corpus,
    _optimizer_ui: bool
) -> AppResult<()> {
    let mut sort_change = false;
    let mut merge = false;
    let mut scroll = 0;
    let mut view = CreditView::Clean;
    loop {
        let mut rows = contributor_data_mode(m, before, after, corpus, model, merge, view);
        if sort_change {
            rows.sort_by(
                |a,
                b|(b.after - b.before).abs().total_cmp(&(a.after - a.before).abs()).then_with(|| a.gram.cmp(&b.gram))
            );
        }
        let raw0 = full_raw(before, corpus, &model.geometry);
        let raw1 = full_raw(after, corpus, &model.geometry);
        let v0 = pct(contribution_mass(m, &raw0, view), denominator(m, &raw0, corpus));
        let v1 = pct(contribution_mass(m, &raw1, view), denominator(m, &raw1, corpus));
        let dp = DETAIL_DECIMALS;
        let mut c = Canvas::new(term.width(), 64);
        let title = if roll_metric(m) {
            format!("{} · {}", METRIC_NAMES[m], roll_settings_label(model.geometry.rolls))
        } else if raw_positive(m).is_some() {
            format!("{} · {}", METRIC_NAMES[m], view.name())
        } else {
            METRIC_NAMES[m].to_string()
        };
        let _controls = if raw_positive(m).is_some() {
            "x view | d sort | b pair | ? help | q back"
        } else {
            "d sort | b pair | ? help | q back"
        };
        header(&mut c, &title, &corpus.name, &model.board.name, "? help");
        let mut y = keyboard(&mut c, 2, model, after, before, None, None, &[]) + 1;
        c.text(0, y, &format!("{} → {} {}", number(v0, dp), number(v1, dp), metric_unit(m)), FG);
        c.text(32, y, &format!("○ before  ● after   {}", if sort_change {
            "change"
        } else {
            "contribution"
        }), MUTED);
        y += 2;
        let show_reasons = raw_positive(m).is_some() && view == CreditView::Rejected;
        let plot_x = if show_reasons && c.w >= 90 {
            62
        } else {
            43
        };
        let plot_w = c.w.saturating_sub(plot_x + 1).max(1);
        c.text(0, y, if merge {
            "Pair"
        } else {
            "Keys"
        }, MUTED);
        c.right(9, y, 9, "Before", MUTED);
        c.right(20, y, 9, "After", MUTED);
        c.right(31, y, 9, "Change", MUTED);
        if plot_x>43 {
            c.text(43, y, "Blocked by", MUTED);
        }
        let maximum = rows.iter().map(|r| r.before.max(r.after)).fold(0.0, f64::max).max(1e-12);
        c.text(plot_x, y, "0", MUTED);
        c.right(
            plot_x + 2,
            y,
            plot_w.saturating_sub(2),
            &format!("{} {}", number(maximum, dp), metric_unit(m)),
            MUTED
        );
        y += 1;
        let n = rows.len().min(16);
        for (i, row) in rows.iter().take(n).enumerate() {
            let yy = y + i;
            let cm = if view == CreditView::Rejected {
                SFB
            } else {
                m
            };
            let color = if view == CreditView::Raw {
                FG
            } else {
                change_color(cm, row.before, row.after)
            };
            c.text(0, yy, &short(&row.gram, 8), FG);
            c.right(9, yy, 9, &number(row.before, dp), MUTED);
            c.right(20, yy, 9, &number(row.after, dp), color);
            c.right(31, yy, 9, &delta_text(row.before, row.after, dp), if view == CreditView::Raw {
                MUTED
            } else {
                delta_color(cm, row.before, row.after)
            });
            if show_reasons {
                let bits = if row.after>0.0 {
                    row.why_after
                } else {
                    row.why_before
                };
                if plot_x>43 {
                    c.text(43, yy, &short(&blocker_names(bits), plot_x - 44), MUTED);
                }
                c.hit(Rect {
                    x: 0,
                    y: yy,
                    w: c.w,
                    h: 1
                }, Action::Item(i));
            }
            contribution_bar(&mut c, plot_x, yy, plot_w, row.before, row.after, maximum, color);
        }
        let sum0: f64 = rows.iter().take(n).map(|r| r.before).sum();
        let sum1: f64 = rows.iter().take(n).map(|r| r.after).sum();
        let other0 = (v0 - sum0).max(0.0);
        let other1 = (v1 - sum1).max(0.0);
        y += n + 1;
        c.text(0, y, "Other", MUTED);
        c.right(9, y, 9, &number(other0, dp), MUTED);
        c.right(20, y, 9, &number(other1, dp), MUTED);
        c.right(31, y, 9, &delta_text(other0, other1, dp), if view == CreditView::Raw {
            MUTED
        } else {
            delta_color(if view == CreditView::Rejected {
                SFB
            } else {
                m
            }, other0, other1)
        });
        c.h = y + 2;
        term.present(&c, scroll)?;
        let e = term.event()?;
        if scroll_event(&e, &mut scroll, c.h, term.size.1) {
            continue;
        }
        if show_reasons {
            if let Some(Action::Item(i)) = action_press(term, &c, &e, scroll) {
                if let Some(row) = rows.get(i) {
                    info_page(
                        term,
                        &row.gram,
                        &[ format!("Before: {}", blocker_names(row.why_before)), format!("After:  {}", blocker_names(row.why_after)),]
                    )?;
                }
            }
        }
        match e {
            Event::Escape | Event::Quit | Event::Char('q') => return Ok(()),
            Event::Char('d') => sort_change=!sort_change,
            Event::Char('b') => if !is_rhythm(m) {
                merge=!merge;
            },
            Event::Char('x') => if raw_positive(m).is_some() {
                view = view.next();
                scroll = 0;
            },
            Event::Char('?') => help_popup(
                term,
                &c,
                METRIC_NAMES[m],
                &[_controls.into(), METRIC_HELP[m].into(), "D/C describe finger placement, not keystroke order. Finger-length order is a model assumption.".into(), "For SRAF: x cycles clean, raw and rejected. Raw = clean + rejected, with the SAME denominator.".into(), "SRAF checks its pair; the veto is structural, even for a zero-weight penalty. ALT counts all non-thumb LRL/RLR triples. Roll filters follow [rolls] in akler.conf.".into(), "Roll thumb inclusion follows [rolls]; other preference metrics exclude thumbs. Excluded presses are never spliced out.".into(), "Only stored pairs/trigrams are known: no claim about four-key or longer sequences.".into(),]
            )?,
            _ => {
            }
        }
    }
}

fn objective_view(
    term: &mut Terminal,
    model: &Model,
    before: &[usize],
    after: &[usize],
    c: &Corpus,
    w: &Weights
) -> AppResult<()> {
    let b = breakdown(&metrics(&full_raw(before, c, &model.geometry), c), w);
    let a = breakdown(&metrics(&full_raw(after, c, &model.geometry), c), w);
    let mut lines = vec!["Term              Weight       Before        After        Change".into()];
    for i in 0..N_WEIGHTS {
        if aggregate(i) {
            continue;
        }
        lines.push(format!(
                "{:<16} {:8.3}  {:11.4}  {:11.4}  {:11.4}",
                weight_display_name(i),
                w.0[i],
                b.contributions[i],
                a.contributions[i],
                a.contributions[i]-b.contributions[i]
            ));
    }
    lines.extend([
            String::new(),
            format!("Penalty          {:.4} → {:.4}", b.penalty, a.penalty),
            format!("Preference credit {:.4} → {:.4}", b.bonus, a.bonus),
            format!("Net objective     {:.4} → {:.4}", b.net, a.net),
            String::new(),
            "These are model score units, not measured comfort or typing speed.".into(),
            "FSB/FSS duplicate daFJB/daFJS; HSB/HSS duplicate daHJB/daHJS. Adjacent and nonadjacent jump weights score disjoint finger distances. Lateral stretch stays independent.".into(),
            "IN/OUT SRAF and IN/OUT ROLL have separate rewards. Combined SRAF/ROLL and roll type columns are display totals.".into()
        ]);
    info_page(term, "Objective audit — selected corpus", &lines)
}

fn problem_objective_view(term: &mut Terminal, p: &Problem, arr: &[usize]) -> AppResult<()> {
    let mut old = [0.0; N_WEIGHTS];
    let mut new = [0.0; N_WEIGHTS];
    let mut lines = vec!["Training corpus       Share     Net before       Net after".into()];
    for (i, c) in p.corpora.iter().enumerate() {
        let b = breakdown(&metrics(&full_raw(&p.model.original, c, &p.model.geometry), c), &p.weights);
        let a = breakdown(&metrics(&full_raw(arr, c, &p.model.geometry), c), &p.weights);
        lines.push(format!(
                "{:<20} {:5.3}     {:10.4}      {:10.4}",
                short(&c.name, 20),
                p.shares[i],
                b.net,
                a.net
            ));
        for j in 0..N_WEIGHTS {
            old[j] += p.shares[i] * b.contributions[j];
            new[j] += p.shares[i] * a.contributions[j];
        }
    }
    lines.push(String::new());
    lines.push("Term                Before         After        Change".into());
    for i in 0..N_WEIGHTS {
        if aggregate(i) {
            continue;
        }
        lines.push(format!(
                "{:<16} {:11.4}   {:11.4}   {:11.4}",
                weight_display_name(i),
                old[i],
                new[i],
                new[i]-old[i]
            ));
    }
    lines.push(String::new());
    lines.push(format!(
            "Mixture net        {:.6} -> {:.6}",
            old.iter().sum:: <f64>(),
            new.iter().sum:: <f64>()
        ));
    lines.push("This breakdown is the actual search objective, including all training shares.".into());
    lines.push("Negative terms are preference credits, not measured ergonomic benefits.".into());
    info_page(term, "Search objective audit", &lines)
}

fn mana2_stats_for(arr: &[usize], corpus: &Corpus, model: &Model) -> mana2_metrics::Stats {
    let raw = full_raw_with_mana2(arr, corpus, &model.geometry, true);
    mana2_metrics::stats(raw.1.as_ref().unwrap(), corpus.totals)
}

fn mana2_objective_view(term: &mut Terminal, p: &Problem, arr: &[usize]) -> AppResult<()> {
    let mut old = [0.0; mana2_metrics::N_STATS];
    let mut new = [0.0; mana2_metrics::N_STATS];
    let mut lines = vec!["Mana2 score is maximized; AKLER minimizes its negative.".into()];
    for (i, corpus) in p.corpora.iter().enumerate() {
        let before = mana2_stats_for(&p.model.original, corpus, &p.model);
        let after = mana2_stats_for(arr, corpus, &p.model);
        lines.push(format!(
            "{} (share {:.3}): score {:.4} → {:.4}",
            corpus.name,
            p.shares[i],
            mana2_metrics::score(&before, &p.weights.2),
            mana2_metrics::score(&after, &p.weights.2)
        ));
        for (index, value) in mana2_metrics::score_contributions(&before, &p.weights.2).into_iter().enumerate() {
            old[index] += p.shares[i] * value;
        }
        for (index, value) in mana2_metrics::score_contributions(&after, &p.weights.2).into_iter().enumerate() {
            new[index] += p.shares[i] * value;
        }
    }
    lines.push(String::new());
    lines.push(format!("Training objective {:.4} → {:.4}", -old.iter().sum::<f64>(), -new.iter().sum::<f64>()));
    lines.push(String::new());
    lines.push("Mana2 stat              Before term      After term".into());
    for (index, id) in mana2_metrics::STAT_IDS.iter().enumerate() {
        lines.push(format!("{id:<24} {:+11.4}     {:+11.4}", old[index], new[index]));
    }
    info_page(term, "Mana2 objective audit", &lines)
}

fn simple_objective_view(term: &mut Terminal, p: &Problem, arr: &[usize]) -> AppResult<()> {
    let mut old_values = [0.0; simple_metrics::N_STATS];
    let mut new_values = [0.0; simple_metrics::N_STATS];
    let mut old_terms = [0.0; simple_metrics::N_STATS];
    let mut new_terms = [0.0; simple_metrics::N_STATS];
    let mut lines = vec![
        "Simple score is minimized; positive weights penalize and negative weights reward.".into(),
    ];
    for (i, corpus) in p.corpora.iter().enumerate() {
        let before = mana2_stats_for(&p.model.original, corpus, &p.model);
        let after = mana2_stats_for(arr, corpus, &p.model);
        lines.push(format!(
            "{} (share {:.3}): score {:.4} → {:.4}",
            corpus.name,
            p.shares[i],
            simple_metrics::score(&before, &p.weights.3),
            simple_metrics::score(&after, &p.weights.3),
        ));
        let before_values = simple_metrics::values(&before, &p.weights.3);
        let after_values = simple_metrics::values(&after, &p.weights.3);
        let before_terms = simple_metrics::score_contributions(&before, &p.weights.3);
        let after_terms = simple_metrics::score_contributions(&after, &p.weights.3);
        for index in 0..simple_metrics::N_STATS {
            old_values[index] += p.shares[i] * before_values[index];
            new_values[index] += p.shares[i] * after_values[index];
            old_terms[index] += p.shares[i] * before_terms[index];
            new_terms[index] += p.shares[i] * after_terms[index];
        }
    }
    lines.push(String::new());
    lines.push(format!(
        "Training objective {:.4} → {:.4}",
        old_terms.iter().sum::<f64>(),
        new_terms.iter().sum::<f64>(),
    ));
    lines.push(String::new());
    lines.push(
        "Simple stat              Weight   Before value  After value   Before term   After term"
            .into(),
    );
    for index in 0..simple_metrics::N_STATS {
        lines.push(format!(
            "{:<24} {:>7.3}  {:>12.4}  {:>11.4}  {:+12.4}  {:+11.4}",
            simple_metrics::LABELS[index],
            p.weights.3.get(index),
            old_values[index],
            new_values[index],
            old_terms[index],
            new_terms[index],
        ));
    }
    info_page(term, "Simple objective audit", &lines)
}

fn validation_view(term: &mut Terminal, model: &Model, before: &[usize], after: &[usize]) -> AppResult<()> {
    let paths = corpus_paths()?;
    let mut lines = vec!["Corpus              SFB before → after    SFS before → after    Bi coverage".into()];
    for path in paths {
        if term.quitting {
            return Ok(());
        }
        match load_source_tui(term, &path).and_then(|s| model.corpus(&s)) {
            Ok(c) => {
                let a = metrics(&full_raw(before, &c, &model.geometry), &c);
                let b = metrics(&full_raw(after, &c, &model.geometry), &c);
                lines.push(format!(
                        "{:<19} {:6.3} → {:6.3}     {:6.3} → {:6.3}        {:6.2}%",
                        short(&c.name, 19),
                        a.v[0],
                        b.v[0],
                        a.v[1],
                        b.v[1],
                        c.coverage[1]
                    ));
            },
            Err(e) => lines.push(format!("ERROR {}: {e}", label(&path, "corpus-"))),
        }
    }
    lines.extend([
            String::new(),
            "Same n-gram definitions, but each corpus has its own frequencies.".into(),
            "Validation never changes the result or silently changes training weights.".into()
        ]);
    info_page(term, "Cross-corpus validation", &lines)
}

fn dashboard(
    term: &Terminal,
    title: &str,
    controls: &str,
    model: &Model,
    arr: &[usize],
    baseline: &[usize],
    corpus: &Corpus,
    w: &Weights,
    locks: Option<&[bool]>,
    cursor: Option<usize>,
    status: &str,
    ensemble: Option<(f64, f64)>,
    optimizer_ui: bool
) -> Canvas {
    let r0 = full_raw(baseline, corpus, &model.geometry);
    let r1 = full_raw(arr, corpus, &model.geometry);
    let b = metrics(&r0, corpus);
    let a = metrics(&r1, corpus);
    let mut c = Canvas::new(term.width(), 96);
    header(&mut c, title, &corpus.name, &model.board.name, if optimizer_ui { controls } else { "" });
    let mut y = keyboard(&mut c, 2, model, arr, baseline, locks, cursor, &[]);
    if optimizer_ui {
        y = grouped_metric_cards(&mut c, y, &b, &a, &r1, corpus, term.decimals());
        y = finger_table(&mut c, y + 1, &b, &a, term.decimals());
        y = score_panel(&mut c, y + 1, &breakdown(&b, w), &breakdown(&a, w), ensemble, term.decimals());
    } else {
        y = editor_metric_cards(&mut c, y, &b, &a, &r1, corpus, term.decimals());
        y = finger_table(&mut c, y + 1, &b, &a, term.decimals());
        y = score_panel(&mut c, y + 1, &breakdown(&b, w), &breakdown(&a, w), ensemble, term.decimals());
    }
    c.text(0, y, &short(status, c.w), CYAN);
    if optimizer_ui {
        c.h = y + 2;
    } else {
        c.text(0, y + 1, "? help", MUTED);
        c.h = y + 3;
    }
    c
}

fn edit_single_weight(term: &mut Terminal, w: &mut Weights, i: usize) -> AppResult<()> {
    if aggregate(i) {
        return Err("Display-only totals have no separate weight; edit their directional or D/C components.".into());
    }
    let hint = if i<N_METRICS {
        METRIC_HELP[i]
    } else if i<N_METRICS + 4 {
        "Extra penalty per percentage point of off-home use by this finger pair."
    } else {
        "Extra penalty per percentage point of total use by this finger pair."
    };
    if let Some(v) = input_box(term, &format!("Weight: {}", weight_display_name(i)), hint, &format!("{}", w.0[i]))? {
        w.0[i] = finite_nonnegative(&v)?;
    }
    Ok(())
}

fn edit_mana2_weight(term: &mut Terminal, weights: &mut Weights, index: usize) -> AppResult<bool> {
    let title = format!(
        "Mana2 {}: [weight, boundary, ..., weight]",
        mana2_metrics::STAT_IDS[index]
    );
    let initial = weights.2.schedule_text(index);
    match input_box(term, &title, "", &initial)? {
        Some(value) => weights.2.set_schedule_text(index, &value),
        None => Ok(false),
    }
}

fn edit_simple_weight(term: &mut Terminal, weights: &mut Weights, index: usize) -> AppResult<bool> {
    let id = simple_metrics::STAT_IDS
        .get(index)
        .ok_or("unknown Simple weight")?;
    let title = format!("Simple weight: {id}");
    let initial = weights.3.get(index).to_string();
    match input_box(
        term,
        &title,
        "Positive penalizes; negative rewards.",
        &initial,
    )? {
        Some(value) => {
            let value = value
                .trim()
                .parse::<f64>()
                .map_err(|_| format!("{id} weight must be a number"))?;
            weights.3.set(index, value)
        }
        None => Ok(false),
    }
}

fn edit_simple_speed_setting(
    term: &mut Terminal,
    weights: &mut Weights,
    index: usize,
) -> AppResult<bool> {
    let id = simple_metrics::SPEED_SETTING_IDS
        .get(index)
        .ok_or("unknown Simple speed setting")?;
    let title = format!("Simple speed setting: {id}");
    let initial = weights.3.get_speed_setting(index).to_string();
    match input_box(
        term,
        &title,
        "Skip ratio is nonnegative; finger strengths are positive.",
        &initial,
    )? {
        Some(value) => {
            let value = value
                .trim()
                .parse::<f64>()
                .map_err(|_| format!("{id} must be a number"))?;
            weights.3.set_speed_setting(index, value)
        }
        None => Ok(false),
    }
}

fn editor(term: &mut Terminal, board: Board, source: &Source) -> AppResult<()> {
    let mut timing = crate::load_profile::LoadProfile::new("ordinary editor initialization");
    let w = load_weights(Path::new(WEIGHTS_FILE))?;
    let model = Model::with_rolls(board, w.rolls());
    timing.mark("Weights and geometry");
    let mut c = model.corpus(source)?;
    timing.mark("Metric corpus and postings");

    let mut arr = model.original.clone();
    let mut baseline = arr.clone();
    let mut undo: Vec<Vec<usize>>=Vec::new();
    let mut redo: Vec<Vec<usize>>=Vec::new();
    let mut cursor = 0;
    let mut keyboard_mode = false;
    let mut selected = None;
    let mut drag = None;
    let mut scroll = 0;
    let mut status = String::new();
    timing.mark("Editor state");
    let mut opening = Some(timing);
    loop {
        let highlights = selected.into_iter().chain(drag).collect:: <Vec<_>>();
        let mut frame = dashboard(term, if arr == baseline {
            "Layout editor"
        } else {
            "Layout editor *"
        }, "Click stat for details | Space swap | u undo | U redo | r reset | s save | c corpus | . digits | ? help | q back", &model, &arr, &baseline, &c, &w, None, if keyboard_mode {
            Some(cursor)
        } else {
            None
        }, &status, None, false);
        if !highlights.is_empty() {
            keyboard(&mut frame, 2, &model, &arr, &baseline, None, if keyboard_mode {
                Some(cursor)
            } else {
                None
            }, &highlights);
        }
        term.present(&frame, scroll)?;
        if let Some(mut timing) = opening.take() {
            timing.mark("First dashboard metrics, rendering and presentation");
        }
        let e = term.event()?;
        if scroll_event(&e, &mut scroll, frame.h, term.size.1) {
            continue;
        }
        if let Some((dr, dc)) = direction(&e) {
            cursor = move_cursor(cursor, dr, dc, &model.board.keys);
            keyboard_mode = true;
            continue;
        }
        let mut swap = None;
        match e.clone() {
            Event::Char(' ') | Event::Enter => {
                keyboard_mode = true;
                if let Some(a) = selected {
                    if a != cursor {
                        swap = Some((a, cursor));
                    }
                    selected = None;
                } else {
                    selected = Some(cursor);
                }
            },
            Event::Mouse {
                x,
                y,
                button: 0,
                release: false,
                motion: false
            } => {
                keyboard_mode = false;
                match term.hit(&frame, x, y, scroll) {
                    Some(Action::Key(i)) => {
                        drag = Some(i);
                        cursor = i;
                    },
                    Some(Action::Metric(m)) => {
                        contributor_view(term, m, &model, &baseline, &arr, &c, false)?;
                    },
                    _ => {
                    }
                }
            },
            Event::Mouse {
                x,
                y,
                button: 0,
                release: false,
                motion: true
            } => {
                if drag.is_some() {
                    if let Some(Action::Key(i)) = term.hit(&frame, x, y, scroll) {
                        cursor = i;
                    }
                }
            },
            Event::Mouse {
                x,
                y,
                button: 0,
                release: true,
                ..
            } => {
                if let Some(from) = drag.take() {
                    if let Some(Action::Key(to)) = term.hit(&frame, x, y, scroll) {
                        cursor = to;
                        if from != to {
                            swap = Some((from, to));
                            selected = None;
                        } else if let Some(a) = selected {
                            if a != to {
                                swap = Some((a, to));
                            }
                            selected = None;
                        } else {
                            selected = Some(to);
                        }
                    }
                }
            },
            Event::Char('.') => {
                term.precise=!term.precise;
            },
            Event::Char('u') => {
                if let Some(a) = undo.pop() {
                    redo.push(arr);
                    arr = a;
                    selected = None;
                    status = "undo".into();
                }
            },
            Event::Char('U') => {
                if let Some(a) = redo.pop() {
                    undo.push(arr);
                    arr = a;
                    selected = None;
                    status = "redo".into();
                }
            },
            Event::Char('r') => {
                undo.push(arr);
                arr = model.original.clone();
                redo.clear();
                selected = None;
                status = "reset to loaded layout".into();
            },
            Event::Char('s') => {
                let path = model.board.path.with_extension("dat");
                let targets = format!("{} and {}", path.display(), path.with_extension("jsonc").display());
                if confirm_box(term, "Save layout", &targets, "Save")? {
                    let symbols = model.symbols(&arr);
                    let dat = board_text(&model.board, &symbols);
                    let jsonc = crate::layout_export::jsonc_text(&board_as_action_layout(&model.board, &symbols)?)?;
                    crate::layout_export::replace_pair(&path, &dat, &jsonc, true)?;
                    baseline = arr.clone();
                    status = format!("{}; comparison baseline updated", saved_layout_message(&path));
                }
            },
            Event::Char('S') => {
                let p = save_new_layout(&model.board, &model.symbols(&arr))?;
                status = saved_layout_message(&p);
            },
            Event::Char('c') => {
                if let Some(path) = select_source_path(term, false)? {
                    let next = crate::session::CorpusSession::new()
                        .plain_tui(term, &path)
                        .and_then(|source| source.map(|source| model.corpus(&source)).transpose());
                    match next {
                        Ok(Some(corpus)) => {
                            c = corpus;
                            selected = None;
                            drag = None;
                            status = format!("Corpus changed to {}", c.name);
                        },
                        Ok(None) => {},
                        Err(error) => status = format!("Corpus change failed: {error}"),
                    }
                }
            },
            Event::Char('v') => validation_view(term, &model, &baseline, &arr)?,
            Event::Char('i') => show_corpus_info(term, &c)?,
            Event::Char('a') => objective_view(term, &model, &baseline, &arr, &c, &w)?,
            Event::Char('?') => help_popup(
                term,
                &frame,
                "Editor controls",
                &["Click a stat for its contributing patterns.".into(), "Click two keys or drag one onto another to swap.".into(), "Arrows/hjkl move; Space selects/swaps. u/U undo/redo.".into(), "c changes corpus and keeps your edits and undo history.".into(), "a audits the objective; . toggles 2/4 decimals.".into(), "s saves both .dat and .jsonc after confirmation; S saves a new pair. — means unchanged; <0.01 is a nonzero amount below display precision.".into(), "Moved letters are blue. Green/red deltas compare with the loaded/saved baseline.".into()]
            )?,
            Event::Escape if selected.is_some() || drag.is_some() => {
                selected = None;
                drag = None;
            },
            Event::Escape | Event::Char('q') | Event::Quit => {
                return Ok(());
            },
            _ => {
            }
        }
        if let Some((a, b)) = swap {
            // Keep Space out of the main grid; explicit main-grid space layouts
            // are readable, but accidental dragging cannot change its assignment.
            if model.canonical[arr[a]] == b' ' || model.canonical[arr[b]] == b' ' {
                status = "Space stays fixed".into();
            } else {
                undo.push(arr.clone());
                redo.clear();
                arr.swap(a, b);
                status = format!(
                    "{} ↔ {}",
                    display_symbol(model.canonical[arr[a]]),
                    display_symbol(model.canonical[arr[b]])
                );
            }
        }
    }
}

fn metric_short_name(m: usize) -> &'static str {
    match m {
        TRAVEL => "TOTAL",
        VTRAVEL => "VERT",
        LTRAVEL => "LAT",
        SFTRAVEL => "SF",
        _ => METRIC_NAMES[m]
    }
}

fn simple_denominator(id: usize, raw: &Raw, totals: &[f64; 4]) -> f64 {
    match id {
        0 => totals[1],
        1 => totals[2],
        2..=4 => totals[1] + totals[2],
        5 | 6 => raw.0[SRAF_DEN],
        _ => raw.0[ROLL_DEN],
    }
}

fn simple_mass(id: usize, raw: &Raw) -> f64 {
    match id {
        0 => raw.0[SFB],
        1 => raw.0[SFS],
        2 => raw.0[LSB] + raw.0[LSS],
        3 => raw.0[ROW1_BI] + raw.0[ROW1_SK],
        4 => raw.0[ROW2_BI] + raw.0[ROW2_SK],
        5 => raw.0[INSRAF],
        6 => raw.0[OUTSRAF],
        7 => raw.0[INROLL],
        _ => raw.0[OUTROLL],
    }
}

fn simple_contributors(
    id: usize,
    model: &Model,
    before: &[usize],
    after: &[usize],
    co: &Corpus
) -> Vec<Contributor> {
    let p0 = positions(before);
    let p1 = positions(after);
    let r0 = full_raw(before, co, &model.geometry);
    let r1 = full_raw(after, co, &model.geometry);
    let den=|r: &Raw | simple_denominator(id, r, &co.totals);
    let mut out = Vec::new();
    for g in &co.grams {
        let mut a = Raw::default();
        let mut b = Raw::default();
        add_gram(&mut a, g, &p0, &model.geometry, 1.0);
        add_gram(&mut b, g, &p1, &model.geometry, 1.0);
        let old = pct(simple_mass(id, &a).max(0.0), den(&r0));
        let new = pct(simple_mass(id, &b).max(0.0), den(&r1));
        if old != 0.0 || new != 0.0 {
            out.push(Contributor {
                gram: gram_name(g, &model.canonical),
                ids: g.ids[..g.len].to_vec(),
                before: old,
                after: new,
                why_before: 0,
                why_after: 0
            });
        }
    }
    out.sort_by(|a, b| b.after.total_cmp(&a.after).then_with(|| a.gram.cmp(&b.gram)));
    out
}

fn settings_labels(s: &SearchSettings) -> Vec<(String, String)> {
    let lim=|v: Option<f64>|v.map(config_number).unwrap_or("none".into());
    vec![("Method".into(), if s.hybrid {
        "hybrid"
    } else {
        "sweep"
    }.into()),("Restarts".into(), s.restarts.to_string()),("Steps".into(), s.anneal_steps.to_string()),("Seconds".into(), fmt2(s.seconds)),("SFB Δ max".into(), lim(s.sfb_limit)),("SFS Δ max".into(), lim(s.sfs_limit)),("Travel Δ max".into(), lim(s.travel_limit)),("SF travel Δ max".into(), lim(s.sftravel_limit)),("Seed".into(), format!("{:x}", s.seed)),("Passes".into(), s.passes.to_string()),("Temp start".into(), config_number(s.temperature)),("Temp end".into(), config_number(s.cooling_end)),("3-key share".into(), fmt2(s.cycle_probability)),("Candidates".into(), s.archive.to_string()),("Design".into(), s.design.clone()),("Min letters".into(), s.diversity.to_string()),("Metrics".into(), s.mode.clone()),("Preset".into(), s.preset.clone())]
}

fn edit_setting(term: &mut Terminal, s: &mut SearchSettings, i: usize) -> AppResult<()> {
    let mut next = s.clone();
    if i == 0 {
        next.hybrid=!next.hybrid;
        validate_search_settings(&next)?;
        * s = next;
        return Ok(());
    }
    let (name, value) = settings_labels(s).get(i).cloned().ok_or("invalid setting")?;
    let initial = match i {
        4 => s.sfb_limit,
        5 => s.sfs_limit,
        6 => s.travel_limit,
        7 => s.sftravel_limit,
        _ => None
    };
    let initial = if (4..=7).contains(&i) {
        initial.map(|n| n.to_string()).unwrap_or("none".into())
    } else {
        value
    };
    if let Some(v) = input_box(term, &name, "", &initial)? {
        match i {
            1 => next.restarts = bounded_usize(&v, 100000)?,
            2 => next.anneal_steps = bounded_usize(&v, 10000000)?,
            3 => next.seconds = finite_nonnegative(&v)?,
            4 => next.sfb_limit = optional_limit(&v)?,
            5 => next.sfs_limit = optional_limit(&v)?,
            6 => next.travel_limit = optional_limit(&v)?,
            7 => next.sftravel_limit = optional_limit(&v)?,
            8 => next.seed = u64::from_str_radix(v.trim_start_matches("0x"), 16)?,
            9 => next.passes = bounded_usize(&v, 100000)?,
            10 => next.temperature = finite_nonnegative(&v)?,
            11 => next.cooling_end = finite_nonnegative(&v)?,
            12 => next.cycle_probability = finite_nonnegative(&v)?,
            13 => next.archive = bounded_usize(&v, 100)?,
            15 => next.diversity = v.parse()?,
            _ => {
            }
        }
        if (4..=7).contains(&i) {
            next.preset = "custom".into();
        }
        validate_search_settings(&next)?;
        * s = next;
    }
    Ok(())
}

fn choose_design(
    term: &mut Terminal,
    s: &mut SearchSettings,
    m: &Model,
    _arr: &[usize],
    locks: &mut Vec<bool>
) -> AppResult<()> {
    if let Some(i) = menu(term, "", &["Refine".into(), "Random".into(), "Evolve".into()])? {
        let old = s.design.clone();
        s.design = ["refine", "random", "evolve"][i].into();
        if old == "refine" && s.design != "refine" {
            locks.fill(false);
        } else if old != "refine" && s.design == "refine" {
            * locks = default_locks(&m.board);
        }
    }
    Ok(())
}

fn cycle_metrics(s: &mut SearchSettings) {
    s.mode = match s.mode.as_str() {
        "detailed" => "mana2",
        "mana2" => "simple",
        _ => "detailed",
    }
    .into();
    s.preset = "custom".into();
}

fn choose_preset(term: &mut Terminal, s: &mut SearchSettings) -> AppResult<()> {
    let names = ["Balanced", "Strict", "Low travel", "Explore", "Custom"];
    if let Some(i) = menu(term, "", &names.iter().map(|x| x.to_string()).collect:: <Vec<_>>())? {
        apply_preset(s, ["balanced", "strict", "low-travel", "explore", "custom"][i]);
    }
    Ok(())
}

fn edit_mix(term: &mut Terminal, s: &mut SearchSettings) -> AppResult<()> {
    let paths = corpus_paths()?;
    let names: Vec<_> = paths.iter().map(|p| corpus_name(p)).collect();
    if let Some(i) = menu(term, "", &names)? {
        let key = names[i].to_ascii_lowercase();
        let old = s.mix.get(&key).copied().unwrap_or(0.0);
        if let Some(v) = input_box(term, &names[i], "", &old.to_string())? {
            s.mix.insert(key, finite_nonnegative(&v)?);
        }
    }
    Ok(())
}

fn optimizer_setup_frame(
    width: usize,
    model: &Model,
    source: &Source,
    arr: &[usize],
    locks: &[bool],
    w: &Weights,
    s: &SearchSettings,
    status: &str,
) -> Canvas {
    let mut c = Canvas::optimizer(width, 96);
    header(
        &mut c,
        "Optimizer",
        &source.name,
        &model.board.name,
        "? help",
    );
    let mut y = keyboard(
        &mut c,
        3,
        model,
        arr,
        &model.original,
        Some(locks),
        None,
        &[],
    ) + 1;
    c.text(
        0,
        y,
        &format!(
            "Locked {}   Free {}",
            locks.iter().filter(|&&v| v).count(),
            locks.iter().filter(|&&v| !v).count()
        ),
        MUTED,
    );
    y += 2;
    match s.mode.as_str() {
        "detailed" => y = grouped_weight_rows(&mut c, y, w) + 1,
        "mana2" => y = mana2_weight_rows(&mut c, y, &w.2) + 1,
        "simple" => y = simple_weight_rows(&mut c, y, &w.3) + 1,
        _ => unreachable!("validated search mode"),
    }
    let labels = settings_labels(s);
    let cols = if c.w >= 80 { 4 } else { 2 };
    let width = c.w / cols;
    for (i, (name, value)) in labels.iter().enumerate() {
        let x = i % cols * width;
        let yy = y + i / cols * 2;
        c.text(x, yy, name, MUTED);
        c.text(x, yy + 1, &short(value, width - 1), CYAN);
        c.hit(
            Rect {
                x,
                y: yy,
                w: width - 1,
                h: 2,
            },
            Action::Setting(i),
        );
    }
    y += (labels.len() + cols - 1) / cols * 2 + 1;
    let mix = if s.mix.values().any(|v| *v > 0.0) {
        s.mix
            .iter()
            .filter(|(_, v)| **v > 0.0)
            .map(|(k, v)| format!("{k}:{v}"))
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        source.name.clone()
    };
    c.text(0, y, &short(&mix, c.w), MUTED);
    c.hit(
        Rect {
            x: 0,
            y,
            w: c.w,
            h: 1,
        },
        Action::Command('x'),
    );
    y += 2;
    c.text(0, y, &short(status, c.w), CYAN);
    c.h = y + 2;
    c
}

fn optimizer_setup(
    term: &mut Terminal,
    model: &Model,
    source: &Source,
    arr: &mut Vec<usize>,
    locks: &mut Vec<bool>,
    w: &mut Weights,
    s: &mut SearchSettings,
) -> AppResult<bool> {
    let mut scroll = 0;
    let mut status = String::new();
    loop {
        let c = optimizer_setup_frame(term.width(), model, source, arr, locks, w, s, &status);
        scroll = scroll.min(c.h.saturating_sub(term.size.1));
        term.present(&c, scroll)?;
        let mut e = term.event()?;
        if scroll_event(&e, &mut scroll, c.h, term.size.1) {
            continue;
        }
        if let Some(action) = action_press(term, &c, &e, scroll) {
            match action {
                Action::Key(i) => {
                    locks[i] = !locks[i];
                    continue;
                }
                Action::Weight(i) => {
                    if let Err(err) = edit_single_weight(term, w, i) {
                        status = err.to_string();
                    }
                    continue;
                }
                Action::Mana2Weight(i) => {
                    match edit_mana2_weight(term, w, i) {
                        Ok(true) => {
                            s.preset = "custom".into();
                            status = format!("Updated Mana2 {}", mana2_metrics::STAT_IDS[i]);
                        }
                        Ok(false) => {}
                        Err(err) => status = err.to_string(),
                    }
                    continue;
                }
                Action::SimpleWeight(i) => {
                    match edit_simple_weight(term, w, i) {
                        Ok(true) => {
                            s.preset = "custom".into();
                            status = format!("Updated Simple {}", simple_metrics::STAT_IDS[i]);
                        }
                        Ok(false) => {}
                        Err(err) => status = err.to_string(),
                    }
                    continue;
                }
                Action::SimpleSpeedSetting(i) => {
                    match edit_simple_speed_setting(term, w, i) {
                        Ok(true) => {
                            s.preset = "custom".into();
                            status =
                                format!("Updated Simple {}", simple_metrics::SPEED_SETTING_IDS[i]);
                        }
                        Ok(false) => {}
                        Err(err) => status = err.to_string(),
                    }
                    continue;
                }
                Action::Setting(i) => {
                    let res = match i {
                        14 => choose_design(term, s, model, arr, locks),
                        16 => {
                            cycle_metrics(s);
                            Ok(())
                        }
                        17 => choose_preset(term, s),
                        _ => edit_setting(term, s, i),
                    };
                    if let Err(err) = res {
                        status = err.to_string();
                    }
                    continue;
                }
                Action::Command(ch) => e = Event::Char(ch),
                _ => {}
            }
        }
        match e {
            Event::Char(' ') => return Ok(true),
            Event::Escape | Event::Quit | Event::Char('q') => return Ok(false),
            Event::Char('H')=>*locks = default_locks(&model.board),
            Event::Char('U')=>locks.fill(false),
            Event::Char('L') => locks.fill(true),
            Event::Char('o')=>*arr = model.original.clone(),
            Event::Char('n') => s.seed = new_seed(),
            Event::Char('g') => choose_design(term, s, model, arr, locks)?,
            Event::Char('m') => cycle_metrics(s),
            Event::Char('p') => choose_preset(term, s)?,
            Event::Char('x') => edit_mix(term, s)?,
            Event::Char('d') => {
                * w = Weights::default();
                * s = SearchSettings::default();
            },
            Event::Char('s') => {
                let result = save_optimizer_settings(w, s);
                status = match result {
                    Ok(()) => "saved".into(),
                    Err(err) => err.to_string()
                };
            },
            Event::Char('r') => {
                let result = (|| -> AppResult<(Weights, SearchSettings)> {
                    let config = load_app_config()?;
                    Ok((config.weights, config.search))
                })();
                match result {
                    Ok((nw, ns)) => {
                        * w = nw;
                        * s = ns;
                    },
                    Err(err) => status = err.to_string()
                }
            },
            Event::Char('?') => help_popup(
                term,
                &c,
                "Optimizer",
                &["Space run; s save configuration; r reload; d defaults; o reset layout to original.".into(), "g design; m detailed/Mana2/Simple; p preset; n new seed; x corpus mixture.".into(), "Click a Mana2 schedule or Simple scalar weight to edit it. s saves the active weight sections.".into(), "H home/thumb locks; U unlock all; L lock all. Mouse toggles locks.".into(), "Limits are increases relative to the original on each training corpus.".into(), "Travel limits use u/100; SFB/SFS use percentage points. none disables.".into()]
            )?,
            _ => {
            }
        }
    }
}

fn load_training_sources(
    term: &mut Terminal,
    selected: &Source,
    settings: &SearchSettings
) -> AppResult<Vec<Source>> {
    let mut out = vec![selected.clone()];
    for path in corpus_paths()? {
        let name = corpus_name(&path).to_ascii_lowercase();
        if !name.eq_ignore_ascii_case(&selected.name) && settings.mix.get(&name).copied().unwrap_or(0.0)>0.0 {
            out.push(load_source_tui(term, &path)?);
        }
    }
    Ok(out)
}

fn optimizer_dashboard(
    term: &Terminal,
    p: &Problem,
    arr: &[usize],
    locks: &[bool],
    title: &str,
    controls: &str,
    status: &str,
) -> Canvas {
    let co = &p.corpora[0];
    let physical = p.settings.uses_mana2_stats();
    let r0 = full_raw_with_mana2(&p.model.original, co, &p.model.geometry, physical);
    let r1 = full_raw_with_mana2(arr, co, &p.model.geometry, physical);
    let a = metrics(&r0, co);
    let b = metrics(&r1, co);
    let mut cv = Canvas::new(term.width(), 96);
    header(&mut cv, title, &co.name, &p.model.board.name,
        if title == "Optimizer result" { "? help" } else { controls });
    let mut y = keyboard(
        &mut cv,
        2,
        &p.model,
        arr,
        &p.model.original,
        Some(locks),
        None,
        &[],
    ) + 1;
    let mix = if p.corpora.len() > 1 {
        Some((
            State::new(p.model.original.clone(), p).score,
            State::new(arr.to_vec(), p).score,
        ))
    } else {
        None
    };
    if p.settings.mode == "mana2" {
        let before = mana2_metrics::stats(r0.1.as_ref().unwrap(), co.totals);
        let after = mana2_metrics::stats(r1.1.as_ref().unwrap(), co.totals);
        for (label, id) in [
            ("SFBW", "sfbw"),
            ("SFSW", "sfsw"),
            ("Stretch", "lsb"),
            ("Scissor", "vsb"),
            ("Weak RED", "redirectweak"),
            ("Roll", "roll"),
        ] {
            let left = before.get(id).unwrap_or(0.0);
            let right = after.get(id).unwrap_or(0.0);
            let suffix = if matches!(id, "redirectweak" | "roll") {
                "%"
            } else {
                ""
            };
            cv.text(
                0,
                y,
                &short(
                    &format!("{label:12} {left:.2}{suffix} → {right:.2}{suffix}"),
                    cv.w,
                ),
                FG,
            );
            y += 1;
        }
        y += 1;
        cv.text(
            0,
            y,
            &short(
                &format!(
                    "Mana2 score {:.2} → {:.2} (higher is better)",
                    mana2_metrics::score(&before, &p.weights.2),
                    mana2_metrics::score(&after, &p.weights.2)
                ),
                cv.w,
            ),
            CYAN,
        );
        y += 1;
        if let Some((old, new)) = mix {
            cv.text(
                0,
                y,
                &short(
                    &format!(
                        "Training objective {:.2} → {:.2} (lower is better)",
                        old, new
                    ),
                    cv.w,
                ),
                MUTED,
            );
            y += 1;
        }
    } else if p.settings.mode == "simple" {
        let before = mana2_metrics::stats(r0.1.as_ref().unwrap(), co.totals);
        let after = mana2_metrics::stats(r1.1.as_ref().unwrap(), co.totals);
        y = simple_stats_rows(
            &mut cv,
            y,
            Some(&before),
            &after,
            &p.weights.3,
            term.decimals(),
        );
        y = simple_speed_table(
            &mut cv,
            y + 1,
            Some(&before),
            &after,
            &p.weights.3,
            term.decimals(),
        );
        y = simple_score_line(
            &mut cv,
            y,
            Some(simple_metrics::score(&before, &p.weights.3)),
            simple_metrics::score(&after, &p.weights.3),
            mix.map(|(_, new)| new),
            term.decimals(),
        );
    } else {
        y = grouped_metric_cards(&mut cv, y, &a, &b, &r1, co, term.decimals());
        y = finger_table(&mut cv, y + 1, &a, &b, term.decimals());
    }
    if p.settings.mode == "detailed" {
        y = score_panel(
            &mut cv,
            y + 1,
            &p.breakdown(&r0, co),
            &p.breakdown(&r1, co),
            mix,
            term.decimals(),
        );
    }
    cv.text(0, y, &short(status, cv.w), CYAN);
    cv.h = y + 2;
    cv
}

fn search_live(
    term: &mut Terminal,
    p: &Problem,
    arr: &mut Vec<usize>,
    locks: &[bool]
) -> AppResult<Option<Snapshot>> {
    let problem = p.clone();
    let start = arr.to_vec();
    let frozen_locks = locks.to_vec();
    let ctrl = Arc::new(Control::new());
    let worker_ctrl = ctrl.clone();
    let (tx, rx) = mpsc::sync_channel(2);
    let (done_tx, done_rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let mut callback=|s | {
            let _ = tx.try_send(s);
        };
        let result = run_search(
            &problem,
            &start,
            &frozen_locks,
            &worker_ctrl,
            &mut callback
        ).map_err(|e| e.to_string());
        let _ = done_tx.send(result);
    });
    let state = State::new(arr.to_vec(), p);
    let mut snap = Snapshot {
        best: Candidate {
            arr: arr.to_vec(),
            score: state.score
        },
        progress: Progress {
            phase: "starting".into(),
            seed: p.settings.seed,
            ..Progress::default()
        },
        archive: Vec::new(),
        has_best: false
    };
    let mut scroll = 0;
    let mut leave = false;
    let result = (|| -> AppResult<Option<Snapshot>> {
        loop {
            while let Ok(s) = rx.try_recv() {
                snap = s;
            }
            match done_rx.try_recv() {
                Ok(r) => return r.map(Some).map_err(|e| e.into()),
                Err(mpsc::TryRecvError::Disconnected) => return Err("search worker exited without a result".into()),
                Err(_) => {
                }
            }
            let pr=&snap.progress;
            let status = format!("{}  {}/{}  {} trials  {:.1}s", if ctrl.pause.load(Ordering::Relaxed) {
                "paused"
            } else {
                &pr.phase
            }, pr.restart, p.settings.restarts, pr.evaluations, pr.elapsed);
            let c = optimizer_dashboard(
                term,
                p,
                &snap.best.arr,
                locks,
                "Optimizing",
                "p pause | q setup (keep changes)",
                &status
            );
            term.present(&c, scroll)?;
            let e = term.event()?;
            if scroll_event(&e, &mut scroll, c.h, term.size.1) {
                continue;
            }
            match e {
                Event::Char('p') => {
                    let b = ctrl.pause.load(Ordering::Relaxed);
                    ctrl.pause.store(!b, Ordering::Relaxed);
                },
                Event::Char('.') => term.precise=!term.precise,
                Event::Escape | Event::Char('q') | Event::Quit => {
                    leave = true;
                    return Ok(None);
                },
                _ => {
                }
            }
        }
    })();
    ctrl.cancel.store(true, Ordering::Relaxed);
    ctrl.pause.store(false, Ordering::Relaxed);
    if handle.join().is_err() {
        return Err("search worker panicked".into());
    }
    if leave {
        if snap.has_best {
            *arr = snap.best.arr;
        }
        Ok(None)
    } else {
        result
    }
}

fn same_layout_for_save(board: &Board, symbols: &[u8], saved: &Board) -> bool {
    // Blank IDs only distinguish interchangeable empty slots. Matching symbols
    // alone does not establish physical layout identity.
    board.keys == saved.keys
        && board.row_stagger == saved.row_stagger
        && symbols.len() == board.keys.len()
        && symbols.len() == saved.symbols.len()
        && symbols
            .iter()
            .zip(&saved.symbols)
            .all(|(&a, &b)| a == b || (blank(a) && blank(b)))
}

fn save_result(
    p: &Problem,
    arr: &[usize],
    start: &[usize],
    locks: &[bool],
    progress: &Progress,
) -> AppResult<PathBuf> {
    let symbols = p.model.symbols(arr);
    let mut b = p.model.board.clone();
    if p.settings.design != "refine" {
        let parent = b.path.parent().unwrap_or(Path::new(LAYOUT_DIR));
        b.path = parent.join(format!("{}-{}.dat", b.name, p.settings.design));
    }
    let path = save_new_layout(&b, &symbols)?;
    let physical_weights = if p.settings.mode == "mana2" {
        format!("[mana2]\n{}\n", p.weights.2.config_text())
    } else if p.settings.mode == "simple" {
        format!("[simple]\n{}\n", p.weights.3.config_text())
    } else {
        String::new()
    };

    let mut report = format!(
        "model = {MODEL_VERSION}\nseed = 0x{:x}\ntrials = {}\nseconds = {:.6}\nobjective = {:.17}\n\n[weights]\n{}\n[rolls]\n{}\n{}[search]\n{}\n[original]\n{}\n[start]\n{}\n[result]\n{}\n[locks]\n{:?}\n",
        progress.seed,
        progress.evaluations,
        progress.elapsed,
        State::new(arr.to_vec(), p).score,
        weights_text(&p.weights),
        rolls_config_text(p.weights.rolls()),
        physical_weights,
        search_settings_text(&p.settings),
        board_text(&p.model.board, &p.model.board.symbols),
        board_text(&p.model.board, &p.model.symbols(start)),
        board_text(&p.model.board, &p.model.symbols(arr)),
        locks.iter().enumerate().filter(|(_, v)|**v).map(|(i, _) | i).collect:: <Vec<_>>()
    );
    for (i, c) in p.corpora.iter().enumerate() {
        let old = metrics(&full_raw(&p.model.original, c, &p.model.geometry), c);
        let new = metrics(&full_raw(arr, c, &p.model.geometry), c);
        report.push_str(&format!(
            "\n[corpus {}]\nfingerprint = {:016x}\nshare = {}\n",
            c.name, c.fingerprint, p.shares[i]
        ));
        for j in 0..N_METRICS {
            report.push_str(&format!(
                "{} = {:.9} -> {:.9}\n",
                METRIC_NAMES[j], old.v[j], new.v[j]
            ));
        }
        for j in 0..SIMPLE_KEYS.len() {
            report.push_str(&format!(
                "simple_{} = {:.9} -> {:.9}\n",
                SIMPLE_KEYS[j], old.simple[j], new.simple[j]
            ));
        }
        if p.settings.mode == "mana2" {
            let before = mana2_stats_for(&p.model.original, c, &p.model);
            let after = mana2_stats_for(arr, c, &p.model);
            report.push_str(&format!(
                "mana2_score = {:.9} -> {:.9}\n",
                mana2_metrics::score(&before, &p.weights.2),
                mana2_metrics::score(&after, &p.weights.2)
            ));
            for (index, id) in mana2_metrics::STAT_IDS.iter().enumerate() {
                report.push_str(&format!(
                    "mana2_{id} = {:.9} -> {:.9}\n",
                    before.values[index], after.values[index]
                ));
            }
        } else if p.settings.mode == "simple" {
            let before = mana2_stats_for(&p.model.original, c, &p.model);
            let after = mana2_stats_for(arr, c, &p.model);
            report.push_str(&format!(
                "simple_score = {:.9} -> {:.9}\n",
                simple_metrics::score(&before, &p.weights.3),
                simple_metrics::score(&after, &p.weights.3),
            ));
            let before_values = simple_metrics::values(&before, &p.weights.3);
            let after_values = simple_metrics::values(&after, &p.weights.3);
            for (index, id) in simple_metrics::STAT_IDS.iter().enumerate() {
                report.push_str(&format!(
                    "simple_metric_{id} = {:.9} -> {:.9}\n",
                    before_values[index], after_values[index],
                ));
            }
        }
    }
    atomic_write(&path.with_extension("run.txt"), &report, false)?;
    Ok(path)
}

fn csv_field(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

fn save_batch(p: &Problem, res: &Snapshot, start: &[usize], locks: &[bool]) -> AppResult<PathBuf> {
    let mut paths = Vec::new();
    for c in &res.archive {
        paths.push(save_result(p, &c.arr, start, locks, &res.progress)?);
    }
    let mut out = String::from("layout,design,mode,preset,seed,corpus,fingerprint,share,objective");
    for name in METRIC_NAMES {
        out.push(',');
        out.push_str(name);
    }
    for name in SIMPLE_KEYS {
        out.push_str(&format!(",simple_{name}"));
    }
    if p.settings.mode == "mana2" {
        for id in mana2_metrics::STAT_IDS {
            out.push_str(&format!(",mana2_{id}"));
        }
    } else if p.settings.mode == "simple" {
        out.push_str(",simple_score");
        for id in simple_metrics::STAT_IDS {
            out.push_str(&format!(",simple_metric_{id}"));
        }
    }
    out.push('\n');
    for (j, candidate) in res.archive.iter().enumerate() {
        for (i, co) in p.corpora.iter().enumerate() {
            let m = metrics(&full_raw(&candidate.arr, co, &p.model.geometry), co);
            out.push_str(&format!(
                "{},{},{},{},0x{:x},{},{:016x},{},{:.9}",
                csv_field(&paths[j].display().to_string()),
                p.settings.design,
                p.settings.mode,
                p.settings.preset,
                p.settings.seed,
                csv_field(&co.name),
                co.fingerprint,
                p.shares[i],
                candidate.score
            ));
            for v in m.v.into_iter().chain(m.simple) {
                out.push_str(&format!(",{v:.9}"));
            }
            if p.settings.mode == "mana2" {
                let stats = mana2_stats_for(&candidate.arr, co, &p.model);
                for value in stats.values {
                    out.push_str(&format!(",{value:.9}"));
                }
            } else if p.settings.mode == "simple" {
                let stats = mana2_stats_for(&candidate.arr, co, &p.model);
                out.push_str(&format!(
                    ",{:.9}",
                    simple_metrics::score(&stats, &p.weights.3)
                ));
                for value in simple_metrics::values(&stats, &p.weights.3) {
                    out.push_str(&format!(",{value:.9}"));
                }
            }
            out.push('\n');
        }
    }
    for n in 1..10000 {
        let path = Path::new(LAYOUT_DIR).join(format!("{}-batch-{n:03}.csv", p.model.board.name));
        match OpenOptions::new().create_new(true).write(true).open(&path) {
            Ok(mut f) => {
                f.write_all(out.as_bytes())?;
                f.sync_all()?;
                return Ok(path);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
    }
    Err("no unused batch filename".into())
}

fn objective_audit(term: &mut Terminal, p: &Problem, arr: &[usize]) -> AppResult<()> {
    if p.settings.mode == "mana2" {
        return mana2_objective_view(term, p, arr);
    }
    if p.settings.mode == "simple" {
        return simple_objective_view(term, p, arr);
    }
    problem_objective_view(term, p, arr)
}

fn optimizer(term: &mut Terminal, board: Board, source: &Source) -> AppResult<()> {
    let mut timing = crate::load_profile::LoadProfile::new("ordinary optimizer setup");
    let config = load_app_config()?;
    let mut model = Model::with_rolls(board, config.weights.rolls());
    timing.mark("Configuration and geometry");
    let mut arr = model.original.clone();
    let mut locks = default_locks(&model.board);
    let mut weights = config.weights;
    let mut settings = config.search;
    if settings.design != "refine" {
        locks.fill(false);
    }
    timing.mark("Weights, settings and locks");
    drop(timing);
    loop {
        if !optimizer_setup(term, &model, source, &mut arr, &mut locks, &mut weights, &mut settings)? {
            return Ok(());
        }
        if model.geometry.rolls != weights.rolls() {
            model.geometry = Geometry::with_rolls(model.board.keys.clone(), weights.rolls());
        }
        let sources = load_training_sources(term, source, &settings)?;
        let mut problem = Problem::new(model.clone(), &sources, 0, weights.clone(), settings.clone())?;
        'runs: loop {
            let start = arr.clone();
            let result = match search_live(term, &problem, &mut arr, &locks)? {
                Some(r) => r,
                None => break 'runs
            };
            if !result.has_best || result.archive.is_empty() {
                break 'runs;
            }
            let mut choice = 0;
            let mut scroll = 0;
            let mut status = String::new();
            loop {
                let cur=&result.archive[choice];
                arr = cur.arr.clone();
                let text = if status.is_empty() {
                    format!(
                        "{}/{}  {}  {} trials · {} letters changed",
                        choice + 1,
                        result.archive.len(),
                        result.progress.phase,
                        result.progress.evaluations,
                        letter_distance(&model, &model.original, &arr)
                    )
                } else {
                    status.clone()
                };
                let frame = optimizer_dashboard(
                    term,
                    &problem,
                    &arr,
                    &locks,
                    "Optimizer result",
                    "r setup | Space refine | b compare | [ ] | a stats | s save | S batch | q setup",
                    &text
                );
                term.present(&frame, scroll)?;
                let e = term.event()?;
                if scroll_event(&e, &mut scroll, frame.h, term.size.1) {
                    continue;
                }
                if let Some(a) = action_press(term, &frame, &e, scroll) {
                    match a {
                        Action::Metric(m) => contributor_view(
                            term,
                            m,
                            &model,
                            &model.original,
                            &arr,
                            &problem.corpora[0],
                            true
                        )?,
                        _ => {
                        }
                    }
                }
                match e {
                    Event::Escape | Event::Char('q') | Event::Char('r') => break 'runs,
                    Event::Quit => return Ok(()),
                    Event::Char('?') => help_popup(term, &frame, "Optimizer result", &[
                        "q or r: return to setup with this candidate; Space: refine again.".into(),
                        "[ / ]: previous / next candidate; b: compare candidates.".into(),
                        "s: save a copy; S: save all candidates.".into(),
                        "a: score contributions; i: corpus notes; v: validation; .: precision.".into(),
                    ])?,
                    Event::Char('.') => term.precise=!term.precise,
                    Event::Char(' ') => {
                        problem.settings.design = "refine".into();
                        problem.settings.seed = problem.settings.seed.wrapping_add(1);
                        continue 'runs;
                    },
                    Event::Char('[') => choice = (choice + result.archive.len() - 1) % result.archive.len(),
                    Event::Char(']') => choice = (choice + 1) % result.archive.len(),
                    Event::Char('b') => choice = candidate_view(term, &problem, &result.archive, choice)?,
                    Event::Char('s') => status = match save_result(&problem, &arr, &start, &locks, &result.progress) {
                        Ok(p) => saved_layout_message(&p),
                        Err(e) => e.to_string()
                    },
                    Event::Char('S') => status = match save_batch(&problem, &result, &start, &locks) {
                        Ok(p) => format!("saved {} candidates (.dat + .jsonc); {}", result.archive.len(), p.display()),
                        Err(e) => e.to_string()
                    },
                    Event::Char('a') => objective_audit(term, &problem, &arr)?,
                    Event::Char('v') => validation_view(term, &model, &model.original, &arr)?,
                    Event::Char('i') => show_corpus_info(term, &problem.corpora[0])?,
                    _ => {
                    }
                }
            }
        }
    }
}

fn rebuild_source_tui(term: &mut Terminal, raw: &Path, cfg: CorpusConfig, hash: u64) -> AppResult<PathBuf> {
    let raw = raw.to_owned();
    let total = fs::metadata(&raw)?.len();
    let name = corpus_name(&raw);
    let progress = Arc::new(AtomicU64::new(0));
    let worker_progress = progress.clone();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = cancel.clone();
    let (tx, rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let result = rebuild_corpus_control(&raw, &cfg, hash, &mut |n, _| {
            worker_progress.store(n, Ordering::Relaxed);
        }, Some(&worker_cancel)).map_err(|e| e.to_string());
        let _ = tx.send(result);
    });
    let result = (|| -> AppResult<PathBuf> {
        loop {
            match rx.try_recv() {
                Ok(v) => return v.map_err(|e| e.into()),
                Err(mpsc::TryRecvError::Disconnected) => return Err("corpus worker exited".into()),
                Err(_) => {
                }
            }
            let mut c = Canvas::new(term.width(), 4);
            let n = progress.load(Ordering::Relaxed);
            c.text(0, 1, &format!("{name}  {:.0}%", pct(n as f64, total as f64)), CYAN);
            term.present(&c, 0)?;
            if matches!(term.event()?, Event::Escape | Event::Quit | Event::Char('q')) {
                return Err("corpus build cancelled".into());
            }
        }
    })();
    cancel.store(true, Ordering::Relaxed);
    if handle.join().is_err() {
        return Err("corpus worker panicked".into());
    }
    result
}

fn prepare_corpus_path_tui(term: &mut Terminal, path: &Path) -> AppResult<PathBuf> {
    if !corpus_is_raw(path)? {
        return Ok(path.to_owned());
    }
    let (cfg, hash) = corpus_config(&corpus_config_path(path))?;
    let cache = cached_path(path);
    if cache_current_at_least(path, &cache, hash, 3)? {
        return Ok(cache);
    }
    rebuild_source_tui(term, path, cfg, hash)
}

fn load_source_tui(term: &mut Terminal, path: &Path) -> AppResult<Source> {
    Source::load(&prepare_corpus_path_tui(term, path)?)
}

#[cfg(test)]
mod saved_layout_identity_tests {
    use super::*;

    const ROWS: &str = "q w e r t | y u i o p\na s d f g | h j k l ;\nz x c v b | n m , . /\n";

    fn board(extra: &str) -> Board {
        board_from_text(&format!("{ROWS}{extra}"), Path::new("inline.dat")).unwrap()
    }

    #[test]
    fn optimizer_metrics_cycle_both_directions_and_resets_preset() {
        let mut settings = SearchSettings::default();
        settings.mode = "detailed".into();
        settings.preset = "balanced".into();
        cycle_metrics(&mut settings);
        assert_eq!(settings.mode, "mana2");
        assert_eq!(settings.preset, "custom");

        settings.preset = "strict".into();
        cycle_metrics(&mut settings);
        assert_eq!(settings.mode, "simple");
        assert_eq!(settings.preset, "custom");

        settings.preset = "balanced".into();
        cycle_metrics(&mut settings);
        assert_eq!(settings.mode, "detailed");
        assert_eq!(settings.preset, "custom");

        let source = Source::from_text(
            r#"{"letters":{"q":4,"w":2},"bigrams":{"qw":2},"trigrams":{"qwe":1}}"#,
            Path::new("inline.json"),
        )
        .unwrap();
        let model = Model::new(board("thumbs: space\n"));
        let locks = default_locks(&model.board);
        let mut weights = Weights::default();
        let sfbw = mana2_metrics::STAT_IDS
            .iter()
            .position(|id| *id == "sfbw")
            .unwrap();
        weights.2.set_schedule_text(sfbw, "[-11, 1.5, -9]").unwrap();
        settings.mode = "mana2".into();
        for width in [64, 108] {
            let canvas = optimizer_setup_frame(
                width,
                &model,
                &source,
                &model.original,
                &locks,
                &weights,
                &settings,
                "ready",
            );
            let text: String = canvas.cells.iter().map(|cell| cell.ch).collect();
            assert!(text.contains("[-11, 1.5, -9]"));
            for id in 0..mana2_metrics::N_STATS {
                let hits: Vec<_> = canvas
                    .hits
                    .iter()
                    .filter(
                        |(_, action)| matches!(action, Action::Mana2Weight(index) if *index == id),
                    )
                    .collect();
                assert_eq!(hits.len(), 1);
                let rect = hits[0].0;
                assert!(rect.x + rect.w <= canvas.w);
                assert!(rect.y < canvas.h);
                assert!(
                    matches!(canvas.action(rect.x, rect.y), Some(Action::Mana2Weight(index)) if index == id)
                );
            }
            assert!(!canvas
                .hits
                .iter()
                .any(|(_, action)| matches!(action, Action::Weight(_))));
        }

        cycle_metrics(&mut settings);
        let canvas = optimizer_setup_frame(
            108,
            &model,
            &source,
            &model.original,
            &locks,
            &weights,
            &settings,
            "ready",
        );
        assert!(!canvas
            .hits
            .iter()
            .any(|(_, action)| matches!(action, Action::Mana2Weight(_))));
        assert!(canvas
            .hits
            .iter()
            .any(|(_, action)| matches!(action, Action::SimpleWeight(0))));
        assert!(!canvas
            .hits
            .iter()
            .any(|(_, action)| matches!(action, Action::Weight(_))));

        cycle_metrics(&mut settings);
        let canvas = optimizer_setup_frame(
            108,
            &model,
            &source,
            &model.original,
            &locks,
            &weights,
            &settings,
            "ready",
        );
        assert!(!canvas
            .hits
            .iter()
            .any(|(_, action)| matches!(action, Action::SimpleWeight(_))));
        assert!(canvas
            .hits
            .iter()
            .any(|(_, action)| matches!(action, Action::Weight(SFB))));
        assert_eq!(weights.2.schedule_text(sfbw), "[-11, 1.5, -9]");
    }

    #[test]
    fn changed_row_stagger_is_a_different_saved_layout() {
        let original = board("thumbs: space\n");
        let staggered = board("thumbs: space\nrow-stagger: standard\n");
        assert_eq!(original.symbols, staggered.symbols);
        assert!(!same_layout_for_save(
            &original,
            &original.symbols,
            &staggered
        ));
    }

    #[test]
    fn changed_finger_assignments_are_a_different_saved_layout() {
        let standard = board("thumbs: space\nrow-stagger: standard\n");
        let angle = board("thumbs: space\nrow-stagger: anglemod\n");
        assert_eq!(standard.symbols, angle.symbols);
        assert!(!same_layout_for_save(&standard, &standard.symbols, &angle));

        // Also check resolved geometry independently of the preset name.
        let mut reassigned = standard.clone();
        reassigned.keys[0].finger = 1;
        reassigned.keys[0].rank = 1;
        assert!(!same_layout_for_save(
            &standard,
            &standard.symbols,
            &reassigned
        ));
    }

    #[test]
    fn same_symbols_on_different_thumb_hands_are_distinct() {
        let left = board("thumbs: space\n");
        let right = board("            space\n");
        assert_eq!(left.symbols, right.symbols);
        assert!(!same_layout_for_save(&left, &left.symbols, &right));
    }

    #[test]
    fn equivalent_aliases_names_and_blank_ids_share_save_identity() {
        let text =
            format!("{ROWS}thumbs: space\nrow-stagger: standard\n").replacen("q w", "~ blank", 1);
        let original = board_from_text(&text, Path::new("original.dat")).unwrap();
        let alias = text
            .replacen("~ blank", "· ~", 1)
            .replace("standard", "standart")
            .replace("space", "␠");
        let mut saved = board_from_text(&alias, Path::new("renamed.dat")).unwrap();
        saved.symbols.swap(0, 1);

        assert!(same_layout_for_save(&original, &original.symbols, &saved));
    }

    #[test]
    fn save_identity_uses_candidate_bindings_and_checks_all_slots() {
        let original = board("thumbs: space\n");
        let mut candidate = original.symbols.clone();
        candidate.swap(0, 1);
        let mut saved = original.clone();
        saved.symbols = candidate.clone();

        assert!(same_layout_for_save(&original, &candidate, &saved));
        assert!(!same_layout_for_save(&original, &original.symbols, &saved));
        assert!(!same_layout_for_save(&original, &candidate[..30], &saved));

        saved.keys[0].col -= 1;
        assert!(!same_layout_for_save(&original, &candidate, &saved));
    }
}
