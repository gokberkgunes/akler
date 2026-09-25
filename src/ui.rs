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
    color: u8
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
    Setting(usize),
    Item(usize),
    SimpleMetric(usize),
    SimpleWeight(usize),
    Command(char)
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
                color: FG
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
                color: FG
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
                color: FG
            });
            self.h = y + 1;
        }
        self.cells[y * self.w + x] = Cell {
            ch,
            color
        };
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
            color: FG
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
                color: FG
            });
            for (i, ch) in "Resize terminal to at least 64 columns x 12 rows. Esc to back out.".chars().take(w).enumerate() {
                view[i] = Cell {
                    ch,
                    color: YELLOW
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
            for cell in &view[y * w..(y + 1) * w] {
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
            } else if !current.main &&(k.hand - current.hand).signum() == dc as i8 {
                candidates.push((i,(k.hand - current.hand).abs()));
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
                    candidates.push((i, 0));
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
    c.text(
        title.len() + 2,
        0,
        &short(&format!("{corpus}  ·  {layout}"), c.w.saturating_sub(title.len() + 2)),
        CYAN
    );
    c.text(0, 1, &short(controls, c.w), MUTED);
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
    for i in 0..arr.len() {
        let key = model.board.keys[i];
        let (kx, ky) = if key.main {
            (
                x + (key.col - min) as usize * step
                    + stagger_cells(key.row_offset, min_offset, step)
                    + usize::from(key.hand == 1) * gap,
                y + key.row as usize * 3 + stagger_cells(key.column_offset, min_column_offset, 3),
            )
        } else {
            (x + width / 2 - kw - 2 + key.hand as usize *(kw + 3), y + 9 + column_height)
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
        c.center(kx + 1, ky + 1, kw - 2, &display_symbol(model.canonical[arr[i]]).to_string(), color);
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

static OPT_GROUP_SAME: [usize; 2] = [SFB, SFS];

static OPT_GROUP_FULL: [usize; 4] = [DFSB, CFSB, DFSS, CFSS];

static OPT_GROUP_HALF: [usize; 2] = [HSB, HSS];

static OPT_GROUP_TOTAL: [usize; 2] = [FSB, FSS];

static OPT_GROUP_STRETCH: [usize; 2] = [LSB, LSS];

static OPT_GROUP_ROW: [usize; 4] = [DSB, CSB, DSS, CSS];

static OPT_GROUP_RHYTHM: [usize; 4] = [REDIR, WRED, WISH, OSF];

static OPT_GROUP_PREF: [usize; 3] = [SRAF, ROLL, ALT];

static OPT_GROUP_ROLL_TOTALS: [usize; 2] = [INROLL, OUTROLL];

static OPT_GROUP_ROLL_TYPES: [usize; 4] = [IN2, OUT2, IN3, OUT3];

static OPT_GROUP_TRAVEL: [usize; 4] = [TRAVEL, VTRAVEL, LTRAVEL, SFTRAVEL];

static OPT_METRIC_GROUPS: [(&str, &[usize]); 11] = [
    ("Same finger", &OPT_GROUP_SAME),
    ("Full scissors", &OPT_GROUP_FULL),
    ("Half scissors", &OPT_GROUP_HALF),
    ("Full totals", &OPT_GROUP_TOTAL),
    ("Stretch", &OPT_GROUP_STRETCH),
    ("Other 2-row", &OPT_GROUP_ROW),
    ("Rhythm", &OPT_GROUP_RHYTHM),
    ("Preferences", &OPT_GROUP_PREF),
    ("Roll totals", &OPT_GROUP_ROLL_TOTALS),
    ("Roll types", &OPT_GROUP_ROLL_TYPES),
    ("Travel u/100", &OPT_GROUP_TRAVEL),
];

static TABLE_GROUPS: [(&str, &[usize]); 11] = [
    ("Same finger", &OPT_GROUP_SAME),
    ("Full", &OPT_GROUP_FULL),
    ("Half", &OPT_GROUP_HALF),
    ("Full totals", &OPT_GROUP_TOTAL),
    ("Stretch", &OPT_GROUP_STRETCH),
    ("Other 2-row", &OPT_GROUP_ROW),
    ("Rhythm", &OPT_GROUP_RHYTHM),
    ("Preferences", &OPT_GROUP_PREF),
    ("Roll totals", &OPT_GROUP_ROLL_TOTALS),
    ("Roll types", &OPT_GROUP_ROLL_TYPES),
    ("Travel u/100", &OPT_GROUP_TRAVEL),
];

struct MetricGrid {
    x: usize,
    width: usize,
    label: usize,
    cell: usize,
    cols: usize,
    value: usize,
    delta: usize,
    rows: usize
}

impl MetricGrid {
    fn new(w: usize, values: &[String], deltas: &[String], decimals: usize) -> Self {
        let value = values.iter().map(|s| s.chars().count()).max().unwrap_or(0).max(decimals + 4);
        let delta = deltas.iter().map(|s| s.chars().count()).max().unwrap_or(0).max(decimals + 3);
        let label = 13;
        // left/right padding around the longest group name
        let cell = 1 + 7 + 1 + value + 2 + delta + 1;
        let fit = w.saturating_sub(label + 2) /(cell + 1);
        let cols = if fit >= 4 {
            4
        } else if fit >= 2 {
            2
        } else {
            1
        };
        let rows = TABLE_GROUPS.iter().map(|(_, g) |(g.len() + cols - 1) / cols).sum();
        let width = label + 2 + cols *(cell + 1);
        Self {
            x: w.saturating_sub(width) / 2,
            width,
            label,
            cell,
            cols,
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

fn grouped_metric_totals(
    c: &mut Canvas,
    y: usize,
    before: &Metrics,
    after: &Metrics,
    raw: &Raw,
    totals: &[f64; 4],
    decimals: usize,
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
    let grid = MetricGrid::new(c.w, &values, &deltas, decimals);
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
    for &(group, items) in TABLE_GROUPS.iter() {
        c.text(grid.x + 2, y + 1 + row, group, MUTED);
        for (i, &m) in items.iter().enumerate() {
            let xx = grid.x + 2 + grid.label +(i % grid.cols) *(grid.cell + 1);
            let yy = y + 1 + row + i / grid.cols;
            let vx = xx + 9;
            c.text(xx + 1, yy, metric_short_name(m), FG);
            c.right(vx, yy, grid.value, &values[m], if values[m] == "n/a" {
                MUTED
            } else {
                change_color(m, before.v[m], after.v[m])
            });
            c.right(vx + grid.value + 2, yy, grid.delta, &deltas[m], delta_color(m, before.v[m], after.v[m]));
            c.hit(Rect {
                x: xx,
                y: yy,
                w: grid.cell,
                h: 1
            }, Action::Metric(m));
        }
        row +=(items.len() + grid.cols - 1) / grid.cols;
    }
    y + h
}

fn weight_label(i: usize) -> &'static str {
    if i<N_METRICS {
        METRIC_NAMES[i]
    } else {
        ["PINKY", "RING", "MIDDLE", "INDEX"][i - N_METRICS]
    }
}

fn grouped_weight_rows(c: &mut Canvas, mut y: usize, w: &Weights) -> usize {
    c.text(0, y, &format!("Weights · {}", roll_settings_label(w.rolls())), FG);
    y += 1;
    let groups: [(&str, &[usize]); 9] = [
        ("Same finger", &OPT_GROUP_SAME),
        ("Full scissors", &OPT_GROUP_FULL),
        ("Half scissors", &OPT_GROUP_HALF),
        ("Stretch", &OPT_GROUP_STRETCH),
        ("Other 2-row", &OPT_GROUP_ROW),
        ("Rhythm", &OPT_GROUP_RHYTHM),
        ("Preferences", &OPT_GROUP_PREF),
        ("Travel", &OPT_GROUP_TRAVEL),
        ("Off-home", &[N_METRICS, N_METRICS + 1, N_METRICS + 2, N_METRICS + 3]),
    ];
    for (group, items) in groups {
        c.text(0, y, group, MUTED);
        let mut x = 16usize;
        for &i in items {
            let name = weight_label(i);
            let value = config_number(w.0[i]);
            let width = (name.len() + 1 + value.len() + 2).max(12);
            if x + width>c.w && x>16 {
                y += 1;
                x = 16;
            }
            c.text(x, y, name, FG);
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
        let controls = if raw_positive(m).is_some() {
            "x view | d sort | b pair | ? help | q back"
        } else {
            "d sort | b pair | ? help | q back"
        };
        header(&mut c, &title, &corpus.name, &model.board.name, controls);
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
            Event::Char('?') => info_page(
                term,
                METRIC_NAMES[m],
                &[ METRIC_HELP[m].into(), "D/C describe finger placement, not keystroke order. Finger-length order is a model assumption.".into(), "For SRAF/ALT: x cycles clean, raw and rejected. Raw = clean + rejected, with the SAME denominator.".into(), "The veto is structural, even for a zero-weight penalty. SRAF checks its pair; ALT checks AB, BC and skip AC. Roll filters follow [rolls] in akler.conf.".into(), "Roll thumb inclusion follows [rolls]; other preference metrics exclude thumbs. Excluded presses are never spliced out.".into(), "Only stored pairs/trigrams are known: no claim about four-key or longer sequences.".into(),]
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
                WEIGHT_NAMES[i],
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
            "FSB/FSS totals are not weighted twice. Other 2-row changes exclude adjacent full scissors; lateral stretch stays independent.".into(),
            "SRAF/ALT reward clean movement; ROLL follows [rolls] and rewards the four types once. Roll breakdowns are display-only.".into()
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
                WEIGHT_NAMES[i],
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
    _optimizer_ui: bool
) -> Canvas {
    let r0 = full_raw(baseline, corpus, &model.geometry);
    let r1 = full_raw(arr, corpus, &model.geometry);
    let b = metrics(&r0, corpus);
    let a = metrics(&r1, corpus);
    let mut c = Canvas::new(term.width(), 96);
    header(&mut c, title, &corpus.name, &model.board.name, controls);
    let mut y = keyboard(&mut c, 2, model, arr, baseline, locks, cursor, &[]);
    y = score_panel(&mut c, y, &breakdown(&b, w), &breakdown(&a, w), ensemble, term.decimals());
    y = grouped_metric_cards(&mut c, y, &b, &a, &r1, corpus, term.decimals());
    y = finger_table(&mut c, y + 1, &b, &a, term.decimals());
    c.text(0, y, &short(status, c.w), CYAN);
    c.h = y + 2;
    c
}

fn edit_single_weight(term: &mut Terminal, w: &mut Weights, i: usize) -> AppResult<()> {
    if aggregate(i) {
        return Err("Display-only metrics have no separate weight; edit D/C scissors or the combined ROLL weight.".into());
    }
    let hint = if i<N_METRICS {
        METRIC_HELP[i]
    } else {
        "Extra penalty per percentage point of off-home use by this finger pair."
    };
    if let Some(v) = input_box(term, &format!("Weight: {}", WEIGHT_NAMES[i]), hint, &format!("{}", w.0[i]))? {
        w.0[i] = finite_nonnegative(&v)?;
    }
    Ok(())
}

fn editor(term: &mut Terminal, board: Board, source: &Source) -> AppResult<()> {
    let mut timing = crate::load_profile::LoadProfile::new("ordinary editor initialization");
    let w = load_weights(Path::new(WEIGHTS_FILE))?;
    let model = Model::with_rolls(board, w.rolls());
    timing.mark("Weights and geometry");
    let c = model.corpus(source)?;
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
        }, "Space swap | u undo | U redo | r reset | s save | . digits | ? help | q back", &model, &arr, &baseline, &c, &w, None, if keyboard_mode {
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
            Event::Char('v') => validation_view(term, &model, &baseline, &arr)?,
            Event::Char('i') => show_corpus_info(term, &c)?,
            Event::Char('a') => objective_view(term, &model, &baseline, &arr, &c, &w)?,
            Event::Char('?') => info_page(
                term,
                "Editor controls",
                &["Click two keys or drag one onto another to swap.".into(), "Arrows/hjkl move; Space selects/swaps. u/U undo/redo.".into(), "Click a metric for before/after contributors. a audits the objective; . toggles 2/4 decimals.".into(), "s saves both .dat and .jsonc after confirmation; S saves a new pair. — means unchanged; <0.01 is a nonzero amount below display precision.".into(), "Moved letters are blue. Green/red deltas compare with the loaded/saved baseline.".into()]
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

fn simple_metric_table(c: &mut Canvas, y: usize, before: &Metrics, after: &Metrics, dp: usize) -> usize {
    let ids: [[Option<usize>; 3]; 3] = [
        [Some(0), Some(1), None],
        [Some(2), Some(3), Some(4)],
        [Some(5), Some(6), None]
    ];
    let groups = ["Same finger", "Movement", "Preferences"];
    let labels = ["SFB", "SFS", "LAT", "ROW1", "ROW2", "SRAF", "ROLL"];
    let label = 13;
    let value = (dp + 4).max(after.simple.iter().map(|v| format!("{}%", number( * v, dp)).len()).max().unwrap_or(0));
    let delta = (dp + 3).max((0..7).map(|i| delta_text(before.simple[i], after.simple[i], dp).chars().count()).max().unwrap_or(0));
    let cell = 1 + 4 + 1 + value + 2 + delta + 1;
    let cols = ((c.w.saturating_sub(label + 2)) /(cell + 1)).clamp(1, 3);
    let rows = ids.iter().map(|r|(r.iter().flatten().count() + cols - 1) / cols).sum:: <usize>();
    let width = label + 2 + cols *(cell + 1);
    let x = c.w.saturating_sub(width) / 2;
    let h = rows + 2;
    c.boxed(Rect {
        x,
        y,
        w: width,
        h
    }, BORDER);
    for j in 0..cols {
        let xx = x + 1 + label + j *(cell + 1);
        c.put(xx, y, '┬', BORDER);
        c.put(xx, y + h - 1, '┴', BORDER);
        for yy in y + 1..y + h - 1 {
            c.put(xx, yy, '│', BORDER);
        }
    }
    let mut row = 0;
    for (i, items) in ids.iter().enumerate() {
        c.text(x + 2, y + 1 + row, groups[i], MUTED);
        let mut n = 0;
        for &id in items.iter().flatten() {
            let xx = x + 2 + label +(n % cols) *(cell + 1);
            let yy = y + 1 + row + n / cols;
            let m = if id >= 5 {
                ROLL
            } else {
                SFB
            };
            c.text(xx + 1, yy, labels[id], FG);
            c.right(
                xx + 6,
                yy,
                value,
                &format!("{}%", number(after.simple[id], dp)),
                change_color(m, before.simple[id], after.simple[id])
            );
            c.right(
                xx + 6 + value + 2,
                yy,
                delta,
                &delta_text(before.simple[id], after.simple[id], dp),
                delta_color(m, before.simple[id], after.simple[id])
            );
            c.hit(Rect {
                x: xx,
                y: yy,
                w: cell,
                h: 1
            }, Action::SimpleMetric(id));
            n += 1;
        }
        row +=(n + cols - 1) / cols;
    }
    y + h
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
    let den=|r: &Raw | match id {
        0 => co.totals[1],
        1 => co.totals[2],
        2..=4 => co.totals[1] + co.totals[2],
        5 => r.0[SRAF_DEN],
        _ => r.0[ROLL_DEN]
    };
    let mass=|r: &Raw | match id {
        0 => r.0[SFB],
        1 => r.0[SFS],
        2 => r.0[LSB] + r.0[LSS],
        3 => r.0[ROW1_BI] + r.0[ROW1_SK],
        4 => r.0[ROW2_BI] + r.0[ROW2_SK],
        5 => r.0[SRAF],
        _ => r.0[SIMPLE_ROLL]
    };
    let mut out = Vec::new();
    for g in &co.grams {
        let mut a = Raw::default();
        let mut b = Raw::default();
        add_gram(&mut a, g, &p0, &model.geometry, 1.0);
        add_gram(&mut b, g, &p1, &model.geometry, 1.0);
        let old = pct(mass(&a).max(0.0), den(&r0));
        let new = pct(mass(&b).max(0.0), den(&r1));
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

fn simple_contributor_view(
    term: &mut Terminal,
    id: usize,
    model: &Model,
    before: &[usize],
    after: &[usize],
    co: &Corpus
) -> AppResult<()> {
    let mut by_delta = false;
    let mut scroll = 0;
    loop {
        let mut rows = simple_contributors(id, model, before, after, co);
        if by_delta {
            rows.sort_by(|a, b|(b.after - b.before).abs().total_cmp(&(a.after - a.before).abs()));
        }
        let mut c = Canvas::new(term.width(), 64);
        header(&mut c, SIMPLE_NAMES[id], &co.name, &model.board.name, "d sort | q back");
        let mut y = keyboard(&mut c, 2, model, after, before, None, None, &[]) + 1;
        let total0: f64 = rows.iter().map(|r| r.before).sum();
        let total1: f64 = rows.iter().map(|r| r.after).sum();
        c.text(0, y, &format!("{total0:.4}% → {total1:.4}%"), FG);
        y += 2;
        for (x, t) in[(0, "Bind"),(10, "Before"),(21, "After"),(32, "Change")] {
            c.text(x, y, t, MUTED);
        }
        y += 1;
        let max = rows.iter().map(|r| r.before.max(r.after)).fold(1e-12, f64::max);
        let px = 43;
        let pw = c.w.saturating_sub(px + 1);
        for (i, r) in rows.iter().take(16).enumerate() {
            let yy = y + i;
            let m = if id >= 5 {
                ROLL
            } else {
                SFB
            };
            let col = change_color(m, r.before, r.after);
            c.text(0, yy, &r.gram, FG);
            c.right(8, yy, 10, &number(r.before, 4), MUTED);
            c.right(19, yy, 10, &number(r.after, 4), col);
            c.right(30, yy, 10, &delta_text(r.before, r.after, 4), delta_color(m, r.before, r.after));
            if pw>0 {
                c.line(px, yy, pw, BORDER);
                let a = (r.before / max *(pw - 1) as f64).round() as usize;
                let b = (r.after / max *(pw - 1) as f64).round() as usize;
                if r.after>0.0 {
                    c.line(px, yy, b + 1, col);
                }
                if r.before>0.0 {
                    c.put(px + a, yy, '○', MUTED);
                }
                if r.after>0.0 {
                    c.put(px + b, yy, '●', col);
                }
            }
        }
        let n = rows.len().min(16);
        y += n + 1;
        c.text(0, y, "Other", MUTED);
        c.right(
            8,
            y,
            10,
            &number((total0 - rows.iter().take(n).map(|r| r.before).sum:: <f64>()).max(0.0), 4),
            MUTED
        );
        c.right(
            19,
            y,
            10,
            &number((total1 - rows.iter().take(n).map(|r| r.after).sum:: <f64>()).max(0.0), 4),
            MUTED
        );
        c.h = y + 2;
        term.present(&c, scroll)?;
        let e = term.event()?;
        if scroll_event(&e, &mut scroll, c.h, term.size.1) {
            continue;
        }
        match e {
            Event::Char('d') => by_delta=!by_delta,
            Event::Char('q') | Event::Quit | Event::Escape => return Ok(()),
            _ => {
            }
        }
    }
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
    arr: &[usize],
    locks: &mut Vec<bool>
) -> AppResult<()> {
    if let Some(i) = menu(term, "", &["Refine".into(), "Random".into(), "Evolve".into()])? {
        let old = s.design.clone();
        s.design = ["refine", "random", "evolve"][i].into();
        if old == "refine" && s.design != "refine" {
            * locks = arr.iter().map(|&id | m.canonical[id] == b' ').collect();
        } else if old != "refine" && s.design == "refine" {
            * locks = default_locks(&m.board);
        }
    }
    Ok(())
}

fn choose_metrics(term: &mut Terminal, s: &mut SearchSettings) -> AppResult<()> {
    if let Some(i) = menu(term, "", &["Simple".into(), "Detailed".into()])? {
        s.mode = ["simple", "detailed"][i].into();
        s.preset = "custom".into();
    }
    Ok(())
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
    term: &Terminal,
    model: &Model,
    source: &Source,
    arr: &[usize],
    locks: &[bool],
    w: &Weights,
    s: &SearchSettings,
    status: &str
) -> Canvas {
    let mut c = Canvas::optimizer(term.width(), 96);
    header(
        &mut c,
        "Optimizer",
        &source.name,
        &model.board.name,
        "Space run | s save | r reload | p preset | q back"
    );
    let mut y = keyboard(&mut c, 3, model, arr, &model.original, Some(locks), None, &[]) + 1;
    c.text(
        0,
        y,
        &format!("Locked {}   Free {}", locks.iter().filter(| && v | v).count(), locks.iter().filter(| && v|!v).count()),
        MUTED
    );
    y += 2;
    if s.mode == "simple" {
        for (i, name) in SIMPLE_NAMES.iter().enumerate() {
            let x = (i % 3) *(c.w / 3);
            let yy = y + i / 3;
            let v = config_number(s.simple[i]);
            c.text(x, yy, name, FG);
            c.text(x + name.len() + 1, yy, &v, CYAN);
            c.hit(Rect {
                x,
                y: yy,
                w: c.w / 3 - 1,
                h: 1
            }, Action::SimpleWeight(i));
        }
        y += 4;
    } else {
        y = grouped_weight_rows(&mut c, y, w) + 1;
    }
    let labels = settings_labels(s);
    let cols = if c.w >= 80 {
        4
    } else {
        2
    };
    let width = c.w / cols;
    for (i,(name, value)) in labels.iter().enumerate() {
        let x = i % cols * width;
        let yy = y + i / cols * 2;
        c.text(x, yy, name, MUTED);
        c.text(x, yy + 1, &short(value, width - 1), CYAN);
        c.hit(Rect {
            x,
            y: yy,
            w: width - 1,
            h: 2
        }, Action::Setting(i));
    }
    y +=(labels.len() + cols - 1) / cols * 2 + 1;
    let mix = if s.mix.values().any(| v|*v>0.0) {
        s.mix.iter().filter(|(_, v)|**v>0.0).map(|(k, v) | format!("{k}:{v}")).collect:: <Vec<_>>().join(" ")
    } else {
        source.name.clone()
    };
    c.text(0, y, &short(&mix, c.w), MUTED);
    c.hit(Rect {
        x: 0,
        y,
        w: c.w,
        h: 1
    }, Action::Command('x'));
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
    s: &mut SearchSettings
) -> AppResult<bool> {
    let mut scroll = 0;
    let mut status = String::new();
    loop {
        let c = optimizer_setup_frame(term, model, source, arr, locks, w, s, &status);
        term.present(&c, scroll)?;
        let mut e = term.event()?;
        if scroll_event(&e, &mut scroll, c.h, term.size.1) {
            continue;
        }
        if let Some(action) = action_press(term, &c, &e, scroll) {
            match action {
                Action::Key(i) => {
                    if model.canonical[arr[i]] != b' ' {
                        locks[i]=!locks[i];
                    }
                    continue;
                },
                Action::Weight(i) => {
                    if let Err(err) = edit_single_weight(term, w, i) {
                        status = err.to_string();
                    }
                    continue;
                },
                Action::SimpleWeight(i) => {
                    if let Some(v) = input_box(term, SIMPLE_NAMES[i], "", &s.simple[i].to_string())? {
                        match finite_nonnegative(&v) {
                            Ok(n) => {
                                s.simple[i] = n;
                                s.preset = "custom".into();
                            },
                            Err(err) => status = err.to_string()
                        }
                    }
                    continue;
                },
                Action::Setting(i) => {
                    let res = match i {
                        14 => choose_design(term, s, model, arr, locks),
                        16 => choose_metrics(term, s),
                        17 => choose_preset(term, s),
                        _ => edit_setting(term, s, i)
                    };
                    if let Err(err) = res {
                        status = err.to_string();
                    }
                    continue;
                },
                Action::Command(ch) => e = Event::Char(ch),
                _ => {
                }
            }
        }
        match e {
            Event::Char(' ') => return Ok(true),
            Event::Escape | Event::Quit | Event::Char('q') => return Ok(false),
            Event::Char('H')=>*locks = default_locks(&model.board),
            Event::Char('U')=>*locks = arr.iter().map(|&i | model.canonical[i] == b' ').collect(),
            Event::Char('L') => locks.fill(true),
            Event::Char('o')=>*arr = model.original.clone(),
            Event::Char('n') => s.seed = new_seed(),
            Event::Char('g') => choose_design(term, s, model, arr, locks)?,
            Event::Char('m') => choose_metrics(term, s)?,
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
            Event::Char('?') => info_page(
                term,
                "Optimizer",
                &["Space run; s save configuration; r reload; d defaults; o original.".into(), "g design; m simple/detailed; p preset; n new seed; x corpus mixture.".into(), "H home locks; U unlock except Space; L lock all. Mouse toggles locks.".into(), "Limits are increases relative to the original on each training corpus.".into(), "Travel limits use u/100; SFB/SFS use percentage points. none disables.".into()]
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
    status: &str
) -> Canvas {
    let co=&p.corpora[0];
    let r0 = full_raw(&p.model.original, co, &p.model.geometry);
    let r1 = full_raw(arr, co, &p.model.geometry);
    let a = metrics(&r0, co);
    let b = metrics(&r1, co);
    let mut cv = Canvas::new(term.width(), 96);
    header(&mut cv, title, &co.name, &p.model.board.name, controls);
    let mut y = keyboard(&mut cv, 2, &p.model, arr, &p.model.original, Some(locks), None, &[]) + 1;
    let mix = if p.corpora.len()>1 {
        Some((State::new(p.model.original.clone(), p).score, State::new(arr.to_vec(), p).score))
    } else {
        None
    };
    y = score_panel(&mut cv, y, &p.breakdown(&r0, co), &p.breakdown(&r1, co), mix, term.decimals());
    if p.settings.mode == "simple" {
        y = simple_metric_table(&mut cv, y, &a, &b, term.decimals()) + 1;
        cv.text(
            0,
            y,
            &format!("Travel {} → {} u/100   SF {} → {} u/100", fmt2(a.v[TRAVEL]), fmt2(b.v[TRAVEL]), fmt2(a.v[SFTRAVEL]), fmt2(b.v[SFTRAVEL])),
            MUTED
        );
        y += 2;
    } else {
        y = grouped_metric_cards(&mut cv, y, &a, &b, &r1, co, term.decimals());
        y = finger_table(&mut cv, y + 1, &a, &b, term.decimals());
    }
    cv.text(0, y, &short(status, cv.w), CYAN);
    cv.h = y + 2;
    cv
}

fn search_live(
    term: &mut Terminal,
    p: &Problem,
    arr: &[usize],
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
                "p pause | q back",
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
    progress: &Progress
) -> AppResult<PathBuf> {
    let symbols = p.model.symbols(arr);
    let mut b = p.model.board.clone();
    if p.settings.design != "refine" {
        let parent = b.path.parent().unwrap_or(Path::new(LAYOUT_DIR));
        b.path = parent.join(format!("{}-{}.dat", b.name, p.settings.design));
    }
    let path = save_new_layout(&b, &symbols)?;

    let mut report = format!(
        "model = {MODEL_VERSION}\nseed = 0x{:x}\ntrials = {}\nseconds = {:.6}\n\n[weights]\n{}\n[rolls]\n{}\n[search]\n{}\n[original]\n{}\n[start]\n{}\n[result]\n{}\n[locks]\n{:?}\n",
        progress.seed,
        progress.evaluations,
        progress.elapsed,
        weights_text(&p.weights),
        rolls_config_text(p.weights.rolls()),
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
                c.name,
                c.fingerprint,
                p.shares[i]
            ));
        for j in 0..N_METRICS {
            report.push_str(&format!("{} = {:.9} -> {:.9}\n", METRIC_NAMES[j], old.v[j], new.v[j]));
        }
        for j in 0..7 {
            report.push_str(&format!(
                    "simple_{} = {:.9} -> {:.9}\n",
                    SIMPLE_KEYS[j],
                    old.simple[j],
                    new.simple[j]
                ));
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
            },
            Err(e)if e.kind() == io::ErrorKind::AlreadyExists => {
            },
            Err(e) => return Err(e.into())
        }
    }
    Err("no unused batch filename".into())
}

fn simple_objective_audit(term: &mut Terminal, p: &Problem, arr: &[usize]) -> AppResult<()> {
    if p.settings.mode != "simple" {
        return problem_objective_view(term, p, arr);
    }
    let mut lines = vec!["Term                Weight        Before          After".into()];
    let mut old = [0.0; 7];
    let mut new = [0.0; 7];
    for (i, c) in p.corpora.iter().enumerate() {
        let a = metrics(&full_raw(&p.model.original, c, &p.model.geometry), c);
        let b = metrics(&full_raw(arr, c, &p.model.geometry), c);
        for j in 0..7 {
            let scale = p.shares[i] * p.settings.simple[j] * if j >= 5 {
                -1.0
            } else {
                1.0
            };
            old[j] += a.simple[j] * scale;
            new[j] += b.simple[j] * scale;
        }
    }
    for j in 0..7 {
        lines.push(format!(
                "{:<18} {:8.3} {:13.4} {:13.4}",
                SIMPLE_NAMES[j],
                p.settings.simple[j],
                old[j],
                new[j]
            ));
    }
    lines.push(format!(
            "Objective                       {:13.4} {:13.4}",
            old.iter().sum:: <f64>(),
            new.iter().sum:: <f64>()
        ));
    info_page(term, "Objective", &lines)
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
        locks = arr.iter().map(|&i | model.canonical[i] == b' ').collect();
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
        let mut problem = Problem::new(model.clone(), &sources, 0, weights, settings.clone())?;
        'runs: loop {
            let start = arr.clone();
            let result = match search_live(term, &problem, &arr, &locks)? {
                Some(r) => r,
                None => return Ok(())
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
                    "r setup | Space refine | b compare | [ ] | s save | S batch | q back",
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
                        Action::SimpleMetric(m) => simple_contributor_view(
                            term,
                            m,
                            &model,
                            &model.original,
                            &arr,
                            &problem.corpora[0]
                        )?,
                        _ => {
                        }
                    }
                }
                match e {
                    Event::Escape | Event::Quit | Event::Char('q') => return Ok(()),
                    Event::Char('.') => term.precise=!term.precise,
                    Event::Char('r') => {
                        arr = model.original.clone();
                        break 'runs;
                    },
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
                    Event::Char('a') => simple_objective_audit(term, &problem, &arr)?,
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
    fn changed_row_stagger_is_a_different_saved_layout() {
        let original = board("thumbs: space\n");
        let staggered = board("thumbs: space\nrow-stagger: standard\n");
        assert_eq!(original.symbols, staggered.symbols);
        assert!(!same_layout_for_save(&original, &original.symbols, &staggered));
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
        assert!(!same_layout_for_save(&standard, &standard.symbols, &reassigned));
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
        let text = format!("{ROWS}thumbs: space\nrow-stagger: standard\n")
            .replacen("q w", "~ blank", 1);
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
