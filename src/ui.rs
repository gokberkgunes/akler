// Compact, cell-based terminal UI. Rendering and mouse hitboxes share the
// same rectangles; wheel scrolling and resizing cannot desynchronize them.
const PANEL_WIDTH: usize = 108;
/// Rows above every screen, holding the Back button.
const TOP_BAR: usize = 3;
const BACK_BUTTON: [&str; TOP_BAR] = ["┌──────────┐", "│ ◀━━ Back │", "└──────────┘"];

const DISPLAY_DECIMALS: usize = 2;

const DETAIL_DECIMALS: usize = 4;

const DISPLAY_EPSILON: f64 = 1e-9;

const FG: u8 = 7;

const MUTED: u8 = 8;

const BORDER: u8 = 9;

/// The dimmed screen behind a popup: a dark grey no theme recolours.
const DIM: u8 = 10;

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
    if path
        .extension()
        .is_some_and(|extension| extension == "jsonc")
    {
        format!("saved {}", path.display())
    } else {
        format!(
            "saved {} and {}",
            path.display(),
            path.with_extension("jsonc").display()
        )
    }
}

fn ansi_color(color: u8) -> String {
    // In a browser the page's theme sets the palette, so muted text, the
    // finger greys and the group tints use palette slots it can recolour.
    #[cfg(target_os = "wasi")]
    if let Some(code) = match color {
        MUTED | 70 => Some(37),
        71 => Some(39),
        72..=77 => Some([94, 92, 93, 95, 96, 91][usize::from(color - 72)]),
        _ => None,
    } {
        return format!("\x1b[{code}m");
    }
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
            (255.0, 64.0 + u * 160.0, 64.0 * (1.0 - u))
        } else {
            let u = (t - 0.5) * 2.0;
            (255.0 * (1.0 - u), 224.0 + u * 6.0, u * 96.0)
        };
        return format!(
            "\x1b[38;2;{};{};{}m",
            r.round() as u8,
            g.round() as u8,
            b.round() as u8
        );
    }
    match color {
        0 => "\x1b[30m",
        1 => "\x1b[31m",
        2 => "\x1b[32m",
        3 => "\x1b[33m",
        4 => "\x1b[34m",
        5 => "\x1b[35m",
        6 => "\x1b[36m",
        MUTED | BORDER => "\x1b[90m",
        DIM => "\x1b[38;5;240m",
        _ => "\x1b[39m",
    }
    .into()
}

// Ranker row backgrounds: the darker and lighter stripes and the selected
// row. In a browser they are written as 256-colour palette entries, which
// each page theme sets (web/src/main.js).
const STRIPE_DARK: (u8, u8, u8) = (18, 18, 18);
const STRIPE_LIGHT: (u8, u8, u8) = (30, 30, 30);
const ROW_SELECTED: (u8, u8, u8) = (36, 36, 36);

fn ansi_background(background: Option<(u8, u8, u8)>) -> String {
    #[cfg(target_os = "wasi")]
    if let Some(index) = match background {
        Some(STRIPE_DARK) => Some(233),
        Some(STRIPE_LIGHT) => Some(235),
        Some(ROW_SELECTED) => Some(236),
        _ => None,
    } {
        return format!("\x1b[48;5;{index}m");
    }
    match background {
        Some((r, g, b)) => format!("\x1b[48;2;{r};{g};{b}m"),
        None => "\x1b[49m".into(),
    }
}

fn gradient(t: f64) -> u8 {
    16 + (if t.is_finite() {
        t.clamp(0.0, 1.0)
    } else {
        0.5
    } * 48.0)
        .round() as u8
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Cell {
    ch: char,
    color: u8,
    background: Option<(u8, u8, u8)>,
    bold: bool,
}

#[derive(Clone, Copy, Debug)]
struct Rect {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

impl Rect {
    fn contains(self, x: usize, y: usize) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

#[derive(Clone, Copy, Debug)]
enum Action {
    Key(usize),
    Metric(usize),
    Weight(usize),
    ManaWeight(usize),
    Setting(usize),
    Item(usize),
    Command(char),
}

struct Canvas {
    w: usize,
    h: usize,
    cells: Vec<Cell>,
    hits: Vec<(Rect, Action)>,
    /// Tabs for the top bar (active index, labels); row 0 is then theirs
    /// and is not shown.
    tabs: Option<(usize, Vec<String>)>,
}

impl Canvas {
    fn new(w: usize, h: usize) -> Self {
        let w = w.min(PANEL_WIDTH);
        Self {
            w,
            h,
            cells: vec![
                Cell {
                    ch: ' ',
                    color: FG,
                    background: None,
                    bold: false,
                };
                w * h
            ],
            hits: Vec::new(),
            tabs: None,
        }
    }

    fn optimizer(w: usize, h: usize) -> Self {
        Self::new(w, h)
    }

    fn ranking(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            cells: vec![
                Cell {
                    ch: ' ',
                    color: FG,
                    background: None,
                    bold: false,
                };
                w * h
            ],
            hits: Vec::new(),
            tabs: None,
        }
    }

    fn put(&mut self, x: usize, y: usize, ch: char, color: u8) {
        if x >= self.w {
            return;
        }
        if y >= self.h {
            self.cells.resize(
                (y + 1) * self.w,
                Cell {
                    ch: ' ',
                    color: FG,
                    background: None,
                    bold: false,
                },
            );
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
        let text = if s.chars().count() > w { "…" } else { s };
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
        if r.w < 2 || r.h < 2 {
            return;
        }
        self.line(r.x, r.y, r.w, color);
        self.line(r.x, r.y + r.h - 1, r.w, color);
        for y in r.y + 1..r.y + r.h - 1 {
            self.put(r.x, y, '│', color);
            self.put(r.x + r.w - 1, y, '│', color);
        }
        for (x, y, c) in [
            (r.x, r.y, '┌'),
            (r.x + r.w - 1, r.y, '┐'),
            (r.x, r.y + r.h - 1, '└'),
            (r.x + r.w - 1, r.y + r.h - 1, '┘'),
        ] {
            self.put(x, y, c, color);
        }
    }

    fn hit(&mut self, r: Rect, a: Action) {
        self.hits.push((r, a));
    }

    fn action(&self, x: usize, y: usize) -> Option<Action> {
        self.hits
            .iter()
            .rev()
            .find(|(r, _)| r.contains(x, y))
            .map(|(_, a)| *a)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
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
        motion: bool,
    },
    Wheel(i32),
    Paste(String),
    /// A host terminal's size, columns then rows (`CSI 8 ; rows ; cols t`).
    Resize(usize, usize),
}

#[derive(Default)]
struct Decoder {
    pending: Vec<u8>,
    discard_paste: bool,
}

impl Decoder {
    fn push(&mut self, b: &[u8]) {
        self.pending.extend_from_slice(b);
        if self.pending.len() > 65536 {
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
        let end = (2..self.pending.len()).find(|&i| (0x40..=0x7e).contains(&self.pending[i]));
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
            if cb & 64 != 0 {
                return Some(Event::Wheel(if cb & 1 == 0 { -3 } else { 3 }));
            }
            return Some(Event::Mouse {
                x: v[1].saturating_sub(1),
                y: v[2].saturating_sub(1),
                button: cb & 3,
                release: last == b'm',
                motion: cb & 32 != 0,
            });
        }
        // WASI hosts report their size on start and on every resize.
        if last == b't' && seq.starts_with(b"\x1b[8;") {
            let text = std::str::from_utf8(&seq[4..seq.len() - 1]).ok()?;
            let v: Vec<usize> = text.split(';').filter_map(|s| s.parse().ok()).collect();
            return Some(match v[..] {
                [rows, cols] if rows > 0 && cols > 0 => Event::Resize(cols, rows),
                _ => Event::Tick,
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
                _ => Event::Tick,
            },
            _ => Event::Tick,
        })
    }
}

/// A WASI host's terminal, such as xterm.js in a browser: already raw, with
/// keys on stdin and the screen on stdout. A thread reads stdin so that a
/// quiet keyboard still yields ticks, as `stty time 1` does natively.
#[cfg(target_os = "wasi")]
struct HostTerminal {
    keys: mpsc::Receiver<Vec<u8>>,
    pending: Vec<u8>,
}

#[cfg(target_os = "wasi")]
impl HostTerminal {
    fn new() -> Self {
        let (send, keys) = mpsc::channel();
        thread::spawn(move || {
            let mut buf = [0; 4096];
            while let Ok(n @ 1..) = io::stdin().read(&mut buf) {
                if send.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        Self {
            keys,
            pending: Vec::new(),
        }
    }
}

#[cfg(target_os = "wasi")]
impl Read for HostTerminal {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pending.is_empty() {
            match self.keys.recv_timeout(Duration::from_millis(100)) {
                Ok(bytes) => self.pending = bytes,
                Err(_) => return Ok(0),
            }
        }
        let n = buf.len().min(self.pending.len());
        buf[..n].copy_from_slice(&self.pending[..n]);
        self.pending.drain(..n);
        Ok(n)
    }
}

#[cfg(target_os = "wasi")]
impl Write for HostTerminal {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        io::stdout().write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        io::stdout().flush()
    }
}

#[cfg(not(target_os = "wasi"))]
type TerminalIo = File;

#[cfg(target_os = "wasi")]
type TerminalIo = HostTerminal;

struct Terminal {
    tty: TerminalIo,
    #[cfg_attr(target_os = "wasi", allow(dead_code))]
    saved: String,
    decoder: Decoder,
    /// The area screens draw in: the terminal less the top bar.
    size: (usize, usize),
    /// The whole terminal, columns then rows.
    screen: (usize, usize),
    #[cfg_attr(target_os = "wasi", allow(dead_code))]
    size_at: Instant,
    previous: Vec<Cell>,
    offset_x: usize,
    precise: bool,
    simple_stats: bool,
    optimizer_weights: bool,
    /// The optimizer's [4] Score tab.
    optimizer_score: bool,
    quitting: bool,
    /// Whether the top bar has the Back button; the home menu turns it off.
    show_back: bool,
    /// Click targets in the top bar, as last drawn: the Back button and tabs.
    bar_hits: Vec<(Rect, Action)>,
    /// Rows of the top bar in the last frame: TOP_BAR, or 0 on home.
    bar: usize,
    /// Canvas rows hidden above the screen: 1 when the tabs moved to the bar.
    skip: usize,
    _reports: crate::profile_output::TerminalReports,
}

impl Terminal {
    #[cfg(not(target_os = "wasi"))]
    fn open_io() -> AppResult<(TerminalIo, String)> {
        let tty = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
        let out = Command::new("stty")
            .arg("-g")
            .stdin(tty.try_clone()?)
            .output()?;
        if !out.status.success() {
            return Err("cannot read terminal state from /dev/tty".into());
        }
        let saved = String::from_utf8(out.stdout)?.trim().to_string();
        let status = Command::new("stty")
            .args(["raw", "-echo", "min", "0", "time", "1"])
            .stdin(tty.try_clone()?)
            .status()?;
        if !status.success() {
            return Err("cannot enter terminal raw mode".into());
        }
        Ok((tty, saved))
    }

    #[cfg(target_os = "wasi")]
    fn open_io() -> AppResult<(TerminalIo, String)> {
        Ok((HostTerminal::new(), String::new()))
    }

    fn open() -> AppResult<Self> {
        let reports = crate::profile_output::TerminalReports::begin();
        let (tty, saved) = Self::open_io()?;
        // Until a WASI host reports its size, take the conventional variables.
        let variable = |name: &str| std::env::var(name).ok().and_then(|v| v.parse().ok());
        let size = (
            variable("COLUMNS").unwrap_or(80usize).clamp(1, 1000),
            variable("LINES").unwrap_or(24usize).clamp(1, 500),
        );
        let mut t = Self {
            tty,
            saved,
            decoder: Decoder::default(),
            size: (size.0, size.1.saturating_sub(TOP_BAR)),
            screen: size,
            size_at: Instant::now() - Duration::from_secs(2),
            previous: Vec::new(),
            offset_x: 0,
            precise: false,
            simple_stats: false,
            optimizer_weights: false,
            optimizer_score: false,
            quitting: false,
            show_back: true,
            bar_hits: Vec::new(),
            bar: 0,
            skip: 0,
            _reports: reports,
        };
        // Alternate screen, SGR drag events, bracketed paste, cursor off, wrap off.
        t.tty
            .write_all(b"\x1b[?1049h\x1b[?1002h\x1b[?1006h\x1b[?2004h\x1b[?25l\x1b[?7l")?;
        t.refresh_size()?;
        Ok(t)
    }

    fn toggle_stats_view(&mut self) {
        self.simple_stats = !self.simple_stats;
    }

    fn stats_view_name(&self) -> &'static str {
        if self.simple_stats {
            "Simple"
        } else {
            "Detailed"
        }
    }

    #[cfg(target_os = "wasi")]
    fn refresh_size(&mut self) -> AppResult<()> {
        // The host reports its size as input (`Event::Resize`).
        Ok(())
    }

    #[cfg(not(target_os = "wasi"))]
    fn refresh_size(&mut self) -> AppResult<()> {
        if self.size_at.elapsed() < Duration::from_millis(500) {
            return Ok(());
        }
        self.size_at = Instant::now();
        let out = Command::new("stty")
            .arg("size")
            .stdin(self.tty.try_clone()?)
            .output()?;
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            let v: Vec<usize> = s
                .split_whitespace()
                .filter_map(|v| v.parse().ok())
                .collect();
            if v.len() == 2 && v[0] > 0 && v[1] > 0 {
                self.set_screen((v[1].min(1000), v[0].min(500)));
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
            Err(e) if e.kind() == io::ErrorKind::Interrupted => 0,
            Err(e) => return Err(e.into()),
        };
        if n > 0 {
            self.decoder.push(&buf[..n]);
        }
        self.refresh_size()?;
        let event = self.decoder.next(n == 0).unwrap_or(Event::Tick);
        Ok(self.visible_event(event))
    }

    fn set_screen(&mut self, screen: (usize, usize)) {
        if screen != self.screen {
            self.previous.clear();
            self.screen = screen;
            self.size = (screen.0, screen.1.saturating_sub(TOP_BAR));
        }
    }

    fn visible_event(&mut self, e: Event) -> Event {
        // Screens see coordinates below the top bar. A click on its Back
        // button is read as Esc, and on a tab as that tab's number.
        let e = match e {
            Event::Mouse { x, y, button, release, motion } if y < self.bar => {
                let pressed = (button == 0 && !release && !motion)
                    .then(|| self.bar_hits.iter().find(|(r, _)| r.contains(x, y)))
                    .flatten();
                return match pressed {
                    Some((_, Action::Command('\u{1b}'))) => Event::Escape,
                    Some((_, Action::Command(key))) => Event::Char(*key),
                    _ => Event::Tick,
                };
            }
            Event::Mouse { x, y, button, release, motion } => Event::Mouse {
                x,
                y: y - self.bar + self.skip,
                button,
                release,
                motion,
            },
            e => e,
        };
        if let Event::Resize(columns, rows) = e {
            self.set_screen((columns.min(1000), rows.min(500)));
            return Event::Tick;
        }
        // In a browser Ctrl-C only backs out, since quitting ends the page.
        #[cfg(target_os = "wasi")]
        let e = if e == Event::Quit { Event::Escape } else { e };
        if e == Event::Quit {
            self.quitting = true;
        }
        if (self.size.0 < 64 || self.size.1 < 12)
            && !matches!(e, Event::Escape | Event::Quit | Event::Char('q'))
        {
            Event::Tick
        } else {
            e
        }
    }

    fn present(&mut self, c: &Canvas, scroll: usize) -> AppResult<()> {
        self.refresh_size()?;
        let (w, h) = if self.show_back { self.size } else { self.screen };
        let scroll = scroll.min(c.h.saturating_sub(h));
        self.offset_x = 0;
        let mut view = vec![
            Cell {
                ch: ' ',
                color: FG,
                background: None,
                bold: false,
            };
            w * h
        ];
        // With tabs in the top bar, the canvas row that held them is skipped.
        let skip = if self.show_back { usize::from(c.tabs.is_some()) } else { 0 };
        // Some screens set h after drawing; never read past the cells.
        let rows = c.h.min(c.cells.len() / c.w.max(1));
        for y in 0..h {
            if y + scroll + skip >= rows {
                break;
            }
            for x in 0..c.w.min(w) {
                view[y * w + x + self.offset_x] = c.cells[(y + scroll + skip) * c.w + x];
            }
        }
        if w < 64 || h < 12 {
            view.fill(Cell {
                ch: ' ',
                color: FG,
                background: None,
                bold: false,
            });
            for (i, ch) in "Resize terminal to at least 64 columns x 12 rows. Esc to back out."
                .chars()
                .take(w)
                .enumerate()
            {
                view[i] = Cell {
                    ch,
                    color: YELLOW,
                    background: None,
                    bold: false,
                };
            }
        }
        // The top bar: the boxed Back button, then the screen's tabs as
        // connected boxes. The home menu has no bar.
        self.bar = if self.show_back { TOP_BAR } else { 0 };
        self.skip = skip;
        let mut bar = Canvas::new(w, self.bar);
        if self.show_back {
            for (row, line) in BACK_BUTTON.iter().enumerate() {
                bar.text(0, row, line, FG);
            }
            let back = BACK_BUTTON[0].chars().count();
            for x in 0..back.min(bar.w) {
                bar.cells[bar.w + x].bold = true;
            }
            bar.hit(Rect { x: 0, y: 0, w: back, h: TOP_BAR }, Action::Command('\u{1b}'));
            if let Some((active, labels)) = &c.tabs {
                tab_boxes(&mut bar, back + 1, 0, *active, labels);
            }
        }
        self.bar_hits = std::mem::take(&mut bar.hits);
        let mut frame = Vec::with_capacity(w * (h + self.bar));
        for y in 0..self.bar {
            frame.extend_from_slice(&bar.cells[y * bar.w..(y + 1) * bar.w]);
            frame.resize(
                (y + 1) * w,
                Cell {
                    ch: ' ',
                    color: FG,
                    background: None,
                    bold: false,
                },
            );
        }
        frame.extend(view);
        let (view, h) = (frame, h + self.bar);
        let mut out = String::new();
        if self.previous.len() != view.len() {
            out.push_str("\x1b[2J");
            self.previous.clear();
        }
        for y in 0..h {
            if self.previous.len() == view.len()
                && self.previous[y * w..(y + 1) * w] == view[y * w..(y + 1) * w]
            {
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
        if x < self.offset_x {
            return None;
        }
        c.action(
            x - self.offset_x,
            y + scroll.min(c.h.saturating_sub(self.size.1)),
        )
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // The reports guard is dropped after this body restores the terminal.
        let _ = self.tty.write_all(
            b"\x1b[0m\x1b[?1002l\x1b[?1000l\x1b[?1006l\x1b[?2004l\x1b[?25h\x1b[?7h\x1b[?1049l",
        );
        let _ = self.tty.flush();
        #[cfg(not(target_os = "wasi"))]
        if let Ok(f) = self.tty.try_clone() {
            let _ = Command::new("stty").arg(&self.saved).stdin(f).status();
        }
    }
}

fn scroll_event(e: &Event, scroll: &mut usize, total: usize, height: usize) -> bool {
    let delta = match e {
        Event::Wheel(n) => *n,
        Event::PageUp => -(height as i32 - 2).max(1),
        Event::PageDown => (height as i32 - 2).max(1),
        _ => return false,
    };
    *scroll = (*scroll as i32 + delta).max(0) as usize;
    *scroll = (*scroll).min(total.saturating_sub(height));
    true
}

fn action_press(term: &Terminal, c: &Canvas, e: &Event, scroll: usize) -> Option<Action> {
    if let Event::Mouse {
        x,
        y,
        button: 0,
        release: false,
        motion: false,
    } = e
    {
        term.hit(c, *x, *y, scroll)
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
        _ => None,
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
            if current.main && k.row == current.row && (k.col - current.col).signum() == dc as i8 {
                candidates.push((i, (k.col - current.col).abs()));
            } else if !current.main && (k.col - current.col).signum() == dc as i8 {
                candidates.push((i, (k.col - current.col).abs()));
            }
        }
    } else if current.main {
        let target = current.row + dr as i8;
        if (0..=2).contains(&target) {
            for (i, k) in keys.iter().enumerate() {
                if k.main && k.row == target {
                    candidates.push((i, (k.col - current.col).abs()));
                }
            }
        } else if target > 2 {
            for (i, k) in keys.iter().enumerate() {
                if !k.main && k.hand == current.hand {
                    candidates.push((i, (k.col - current.col).abs()));
                }
            }
        }
    } else if dr < 0 {
        let target = if current.hand == 0 { 4 } else { 5 };
        for (i, k) in keys.iter().enumerate() {
            if k.main && k.row == 2 && k.hand == current.hand {
                candidates.push((i, (k.col - target).abs()));
            }
        }
    }
    candidates
        .into_iter()
        .min_by_key(|v| v.1)
        .map(|v| v.0)
        .unwrap_or(cursor)
}

fn change_color(m: usize, before: f64, after: f64) -> u8 {
    let d = if higher_better(m) {
        after - before
    } else {
        before - after
    };
    if d > DISPLAY_EPSILON {
        GREEN
    } else if d < -DISPLAY_EPSILON {
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

// Retain actual percentages in Before/After, but compare the contributing
// masses on one scale so an unrelated population change cannot color a row.
fn contributor_comparison(before: f64, after: f64, before_den: f64, after_den: f64) -> (f64, f64) {
    let shared = before_den.max(after_den);
    if shared <= 0.0 {
        return (0.0, 0.0);
    }
    (before * (before_den / shared), after * (after_den / shared))
}

fn header(c: &mut Canvas, title: &str, corpus: &str, layout: &str, controls: &str) {
    c.text(0, 0, title, FG);
    let context = [corpus, layout]
        .into_iter()
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    let x = title.chars().count() + 2;
    c.text(x, 0, &short(&context, c.w.saturating_sub(x)), CYAN);
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
    highlight: &[usize],
) -> usize {
    let min = model
        .board
        .keys
        .iter()
        .filter(|k| k.main)
        .map(|k| k.col)
        .min()
        .unwrap_or(0);
    let max = model
        .board
        .keys
        .iter()
        .filter(|k| k.main)
        .map(|k| k.col)
        .max()
        .unwrap_or(9);
    let cols = (max - min + 1) as usize;
    let right_col = right_main_column(&model.board.keys, |i| !blank(model.board.symbols[i]));
    let min_offset = model
        .board
        .keys
        .iter()
        .filter(|k| k.main)
        .map(|k| k.row_offset)
        .min()
        .unwrap_or(0)
        .min(0);
    let max_offset = model
        .board
        .keys
        .iter()
        .filter(|k| k.main)
        .map(|k| k.row_offset)
        .max()
        .unwrap_or(0)
        .max(0);
    let min_column_offset = model
        .board
        .keys
        .iter()
        .filter(|k| k.main)
        .map(|k| k.column_offset)
        .min()
        .unwrap_or(0)
        .min(0);
    let max_column_offset = model
        .board
        .keys
        .iter()
        .filter(|k| k.main)
        .map(|k| k.column_offset)
        .max()
        .unwrap_or(0)
        .max(0);
    let column_height = stagger_cells(max_column_offset, min_column_offset, 3);
    let gap = 3;
    let fits = |kw: usize| {
        cols * (kw + 1) - 1 + gap + stagger_cells(max_offset, min_offset, kw + 1) <= c.w
    };
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
    let left_thumbs = model
        .board
        .keys
        .iter()
        .filter(|key| !key.main && key.hand == 0)
        .count();
    let left_start =
        (x + width / 2).saturating_sub(kw + 2 + left_thumbs.saturating_sub(1) * (kw + 3));
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
            h: 3,
        };
        c.boxed(r, border);
        let symbol = model.canonical[arr[i]];
        let label = if symbol == b' ' {
            "␣".to_owned()
        } else {
            display_symbol(symbol).to_string()
        };
        c.center(kx + 1, ky + 1, kw - 2, &label, color);
        // Keyboard/click selection is indicated without changing ANSI color.
        if highlight.contains(&i) || cursor == Some(i) {
            c.put(kx + 1, ky + 1, '›', FG);
        }
        c.hit(r, Action::Key(i));
    }
    y + column_height
        + if model.board.keys.iter().any(|k| !k.main) {
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
    if value > DISPLAY_EPSILON && text == fixed(0.0, decimals) {
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
    if value != 0.0 && value.abs() < 0.01 {
        let decimals = (1.0 - value.abs().log10().floor()).max(2.0).min(12.0) as usize;
        fixed(value, decimals)
    } else {
        fixed(value, DISPLAY_DECIMALS)
    }
}

static OPT_GROUP_SAME: [usize; 4] = [SFB, SFS, SKB, SKS];

static OPT_GROUP_STRETCH: [usize; 2] = [LSB, LSS];

static OPT_GROUP_TRAVEL: [usize; 4] = [TRAVEL, VTRAVEL, LTRAVEL, SFTRAVEL];

// Display families are independent of the optimizer's weight controls.
// Keep the adjacent discordant aliases (FSB/FSS and HSB/HSS) in their families.
static TABLE_FULL_JUMPS: [usize; 10] = [
    DAFJB, DAFJS, CAFJB, CAFJS, DNFJB, DNFJS, CNFJB, CNFJS, FSB, FSS,
];

static TABLE_HALF_JUMPS: [usize; 10] = [
    DAHJB, DAHJS, CAHJB, CAHJS, DNHJB, DNHJS, CNHJB, CNHJS, HSB, HSS,
];

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

// Display only: the existing adjacent discordant aliases retain their values.
// Concordant and nonadjacent jumps still contribute to the detailed score.
static SIMPLE_FULL_JUMPS: [usize; 2] = [FSB, FSS];
static SIMPLE_HALF_JUMPS: [usize; 2] = [HSB, HSS];
static SIMPLE_TABLE_GROUPS: [(&str, &[usize]); 6] = [
    ("Same finger", &OPT_GROUP_SAME),
    ("Full jumps", &SIMPLE_FULL_JUMPS),
    ("Half jumps", &SIMPLE_HALF_JUMPS),
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
    rows: usize,
}

impl MetricGrid {
    fn new(
        w: usize,
        values: &[String],
        deltas: &[String],
        decimals: usize,
        groups: &[(&str, &[usize])],
        left_aligned: bool,
    ) -> Self {
        let value = values
            .iter()
            .map(|s| s.chars().count())
            .max()
            .unwrap_or(0)
            .max(decimals + 4);
        let delta = deltas
            .iter()
            .map(|s| s.chars().count())
            .max()
            .unwrap_or(0)
            .max(decimals + 3);
        // Size both label columns from the text actually rendered. Long group
        // names must leave the separator intact, including in detailed views.
        let label = groups
            .iter()
            .map(|(name, _)| name.chars().count() + 2)
            .max()
            .unwrap_or(0)
            .max(if left_aligned { 14 } else { 13 });
        let name = groups
            .iter()
            .flat_map(|(_, items)| items.iter())
            .map(|&m| metric_short_name(m).chars().count())
            .max()
            .unwrap_or(7)
            .max(7);
        let cell = 1 + name + 1 + value + 2 + delta + 1;
        let fit = w.saturating_sub(label + 2) / (cell + 1);
        // Keep bigrams on the left and their skipgrams on the right, even
        // when the terminal could fit additional columns.
        let cols = if fit >= 2 { 2 } else { 1 };
        let rows = groups
            .iter()
            .map(|(_, g)| (g.len() + cols - 1) / cols)
            .sum::<usize>()
            + groups.len().saturating_sub(1);
        let width = label + 2 + cols * (cell + 1);
        Self {
            x: if left_aligned && width < w {
                1
            } else {
                w.saturating_sub(width) / 2
            },
            width,
            label,
            cell,
            cols,
            name,
            value,
            delta,
            rows,
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
    decimals: usize,
    simple: bool,
) -> usize {
    if simple {
        return grouped_metric_table(
            c,
            y,
            before,
            after,
            raw,
            &corpus.totals,
            decimals,
            &SIMPLE_TABLE_GROUPS,
            false,
            true,
        );
    }
    grouped_metric_totals(c, y, before, after, raw, &corpus.totals, decimals)
}

fn editor_metric_cards(
    c: &mut Canvas,
    y: usize,
    before: &Metrics,
    after: &Metrics,
    raw: &Raw,
    corpus: &Corpus,
    decimals: usize,
    simple: bool,
) -> usize {
    grouped_metric_table(
        c,
        y,
        before,
        after,
        raw,
        &corpus.totals,
        decimals,
        if simple {
            &SIMPLE_TABLE_GROUPS
        } else {
            &EDITOR_TABLE_GROUPS
        },
        true,
        true,
    )
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
    grouped_metric_table(
        c,
        y,
        before,
        after,
        raw,
        totals,
        decimals,
        &TABLE_GROUPS,
        false,
        true,
    )
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
    clickable: bool,
) -> usize {
    let values: Vec<String> = (0..N_METRICS)
        .map(|m| {
            if denominator_totals(m, raw, totals) > 0.0 {
                metric_value_text(m, after.v[m], decimals)
            } else {
                "n/a".into()
            }
        })
        .collect();
    let deltas: Vec<String> = (0..N_METRICS)
        .map(|m| {
            if denominator_totals(m, raw, totals) > 0.0 {
                delta_text(before.v[m], after.v[m], decimals)
            } else {
                "—".into()
            }
        })
        .collect();
    let colors: Vec<_> = (0..N_METRICS)
        .map(|m| {
            if values[m] == "n/a" {
                MUTED
            } else {
                change_color(m, before.v[m], after.v[m])
            }
        })
        .collect();
    let delta_colors: Vec<_> = (0..N_METRICS)
        .map(|m| delta_color(m, before.v[m], after.v[m]))
        .collect();
    grouped_value_table(
        c,
        y,
        &values,
        &deltas,
        &colors,
        &delta_colors,
        decimals,
        groups,
        left_aligned,
        clickable,
        false,
    )
}

// Shared borders, spacing, tints, and hitboxes for stats and editable weights.
fn grouped_value_table(
    c: &mut Canvas,
    y: usize,
    values: &[String],
    deltas: &[String],
    colors: &[u8],
    delta_colors: &[u8],
    decimals: usize,
    groups: &[(&str, &[usize])],
    left_aligned: bool,
    clickable: bool,
    weights: bool,
) -> usize {
    let grid = MetricGrid::new(c.w, &values, &deltas, decimals, groups, left_aligned);
    grouped_value_table_grid(
        c,
        y,
        values,
        deltas,
        colors,
        delta_colors,
        groups,
        clickable,
        weights,
        grid,
    )
}

fn grouped_value_table_grid(
    c: &mut Canvas,
    y: usize,
    values: &[String],
    deltas: &[String],
    colors: &[u8],
    delta_colors: &[u8],
    groups: &[(&str, &[usize])],
    clickable: bool,
    weights: bool,
    grid: MetricGrid,
) -> usize {
    let h = grid.rows + 2;
    c.boxed(
        Rect {
            x: grid.x,
            y,
            w: grid.width,
            h,
        },
        BORDER,
    );
    for col in 0..grid.cols {
        let xx = grid.x + 1 + grid.label + col * (grid.cell + 1);
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
                let xx = grid.x + 1 + grid.label + col * (grid.cell + 1);
                c.put(xx, yy, '┼', BORDER);
            }
            row += 1;
        }
        let tint = group_tint(group_index);
        c.text(
            grid.x + 2,
            y + 1 + row,
            &short(group, grid.label.saturating_sub(2)),
            tint,
        );
        for (i, &m) in items.iter().enumerate() {
            let xx = grid.x + 2 + grid.label + (i % grid.cols) * (grid.cell + 1);
            let yy = y + 1 + row + i / grid.cols;
            let vx = xx + 1 + grid.name + 1;
            c.text(xx + 1, yy, &short(metric_short_name(m), grid.name), tint);
            c.right(vx, yy, grid.value, &values[m], colors[m]);
            c.right(
                vx + grid.value + 2,
                yy,
                grid.delta,
                &deltas[m],
                delta_colors[m],
            );
            if clickable && (!weights || optimizer_metric_weight(m).is_some()) {
                c.hit(
                    Rect {
                        x: xx,
                        y: yy,
                        w: grid.cell,
                        h: 1,
                    },
                    if weights {
                        Action::Weight(optimizer_metric_weight(m).unwrap())
                    } else {
                        Action::Metric(m)
                    },
                );
            }
        }
        row += (items.len() + grid.cols - 1) / grid.cols;
    }
    y + h
}

// Use the same metric slots and grid for both optimizer panels. Aggregate
// metrics keep their place; their component weights are listed underneath.
fn optimizer_metric_cards(
    c: &mut Canvas,
    y: usize,
    before: &Metrics,
    after: &Metrics,
    raw: &Raw,
    corpus: &Corpus,
    w: &Weights,
    decimals: usize,
    simple: bool,
    weights: bool,
) -> usize {
    let groups = if simple {
        &SIMPLE_TABLE_GROUPS
    } else {
        &EDITOR_TABLE_GROUPS
    };
    let stats: Vec<String> = (0..N_METRICS)
        .map(|m| {
            if denominator_totals(m, raw, &corpus.totals) > 0.0 {
                metric_value_text(m, after.v[m], decimals)
            } else {
                "n/a".into()
            }
        })
        .collect();
    let changes: Vec<String> = (0..N_METRICS)
        .map(|m| {
            if denominator_totals(m, raw, &corpus.totals) > 0.0 {
                delta_text(before.v[m], after.v[m], decimals)
            } else {
                "—".into()
            }
        })
        .collect();
    let weight_values: Vec<String> = (0..N_WEIGHTS)
        .map(|m| {
            optimizer_metric_weight(m)
                .map(|id| config_number(w.0[id]))
                .unwrap_or_else(|| "—".into())
        })
        .collect();
    let mut sizing = stats.clone();
    sizing.extend(weight_values.iter().cloned());
    let grid = MetricGrid::new(c.w, &sizing, &changes, decimals, groups, true);
    let colors: Vec<_> = (0..N_METRICS)
        .map(|m| {
            if stats[m] == "n/a" {
                MUTED
            } else {
                change_color(m, before.v[m], after.v[m])
            }
        })
        .collect();
    let delta_colors: Vec<_> = (0..N_METRICS)
        .map(|m| delta_color(m, before.v[m], after.v[m]))
        .collect();
    if !weights {
        return grouped_value_table_grid(
            c,
            y,
            &stats,
            &changes,
            &colors,
            &delta_colors,
            groups,
            true,
            false,
            grid,
        );
    }
    let empty = vec![String::new(); N_WEIGHTS];
    let cyan = vec![CYAN; N_WEIGHTS];
    let y = grouped_value_table_grid(
        c,
        y,
        &weight_values,
        &empty,
        &cyan,
        &cyan,
        groups,
        true,
        true,
        grid,
    );
    let visible: Vec<_> = groups
        .iter()
        .flat_map(|(_, ids)| ids.iter())
        .filter_map(|&m| optimizer_metric_weight(m))
        .collect();
    let extra: Vec<_> = TABLE_GROUPS
        .iter()
        .map(|&(name, ids)| {
            (
                name,
                ids.iter()
                    .copied()
                    .filter(|&m| !aggregate(m) && !visible.contains(&m))
                    .collect::<Vec<_>>(),
            )
        })
        .filter(|(_, ids)| !ids.is_empty())
        .chain([
            ("Off-home", (N_METRICS..N_METRICS + 4).collect()),
            ("Finger usage", (N_METRICS + 4..N_WEIGHTS).collect()),
        ])
        .collect();
    let extra_groups: Vec<_> = extra
        .iter()
        .map(|(name, ids)| (*name, ids.as_slice()))
        .collect();
    c.text(
        0,
        y,
        "Additional weights · — means component weights below",
        MUTED,
    );
    grouped_value_table(
        c,
        y + 1,
        &weight_values,
        &empty,
        &cyan,
        &cyan,
        decimals,
        &extra_groups,
        true,
        true,
        true,
    )
}

fn optimizer_metric_weight(m: usize) -> Option<usize> {
    match m {
        FSB => Some(DAFJB),
        FSS => Some(DAFJS),
        HSB => Some(DAHJB),
        HSS => Some(DAHJS),
        _ if aggregate(m) => None,
        _ => Some(m),
    }
}

fn weight_label(i: usize) -> &'static str {
    if i < N_METRICS {
        METRIC_NAMES[i]
    } else {
        ["PINKY", "RING", "MIDDLE", "INDEX"][(i - N_METRICS) % 4]
    }
}

pub(crate) fn physical_weight_rows(
    c: &mut Canvas,
    mut y: usize,
    weights: &mana2_metrics::Weights,
) -> usize {
    c.text(0, y, "AKL.GG weights · click to edit", FG);
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
        ("Rolls and speed", 34..mana2_metrics::N_STATS),
    ];
    let left = if c.w >= 64 { 16 } else { 0 };
    for (group_index, (group, indices)) in groups.into_iter().enumerate() {
        let tint = group_tint(group_index);
        c.text(0, y, group, tint);
        if left == 0 {
            y += 1;
        }
        let mut x = left;
        for index in indices.filter(|index| mana2_metrics::is_weightable(*index)) {
            let name = mana2_metrics::STAT_IDS[index];
            let value = weights.schedule_text(index);
            let width = (name.len() + value.len() + 3).max(12);
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
                Action::ManaWeight(index),
            );
            x += width;
        }
        y += 1;
    }
    y
}

fn aklgg_index(id: &str) -> usize {
    mana2_metrics::STAT_IDS
        .iter()
        .position(|candidate| *candidate == id)
        .expect("AKL.GG table stat must exist")
}

fn aklgg_change_color(id: &str, before: f64, after: f64) -> u8 {
    let higher_is_better = matches!(
        id,
        "alt"
            | "altnothumbs"
            | "roll"
            | "rollnothumbs"
            | "inroll2"
            | "inroll2nothumbs"
            | "outroll2"
            | "outroll2nothumbs"
            | "inroll3"
            | "inroll3nothumbs"
            | "outroll3"
            | "outroll3nothumbs"
    );
    if (after - before).abs() <= DISPLAY_EPSILON {
        FG
    } else if (higher_is_better && after > before) || (!higher_is_better && after < before) {
        GREEN
    } else {
        RED
    }
}

fn aklgg_number(stats: &mana2_metrics::Stats, id: &str, decimals: usize) -> String {
    let index = aklgg_index(id);
    let value = number(stats.values[index], decimals);
    if id.starts_with("finger-usage-")
        || matches!(id, "sfb" | "skb" | "sfs" | "sks")
        || (22..=45).contains(&index)
    {
        format!("{value}%")
    } else {
        value
    }
}

fn aklgg_stat_cell(
    c: &mut Canvas,
    x: usize,
    y: usize,
    width: usize,
    id: &str,
    before: &mana2_metrics::Stats,
    after: &mana2_metrics::Stats,
    decimals: usize,
) {
    let index = aklgg_index(id);
    c.text(
        x,
        y,
        &short(&aklgg_number(after, id, decimals), width),
        aklgg_change_color(id, before.values[index], after.values[index]),
    );
}

/// Compact comparison of the physical statistics exposed by AKL.GG's Mana2
/// view. Values are current-layout values; their color compares them with the
/// supplied baseline. Usage is deliberately neutral because moving load from
/// one finger to another is not intrinsically an improvement.
pub(crate) fn aklgg_stat_table(
    c: &mut Canvas,
    mut y: usize,
    before: &mana2_metrics::Stats,
    after: &mana2_metrics::Stats,
    decimals: usize,
) -> usize {
    let value_w = decimals + 6;
    let bigram_x = 18;
    let skipgram_x = bigram_x + value_w + 2;
    c.text(0, y, "AKL.GG patterns", CYAN);
    c.text(bigram_x, y, "BIGRAM", MUTED);
    c.text(skipgram_x, y, "SKIPGRAM", MUTED);
    y += 1;
    for (label, bigram, skipgram) in [
        ("Same Finger", "sfb", "sfs"),
        ("Same Key", "skb", "sks"),
        ("Stretch", "lsb", "lss"),
        ("Scissor", "vsb", "vss"),
    ] {
        c.text(0, y, label, FG);
        aklgg_stat_cell(c, bigram_x, y, value_w, bigram, before, after, decimals);
        aklgg_stat_cell(c, skipgram_x, y, value_w, skipgram, before, after, decimals);
        y += 1;
    }

    y += 1;
    let all_x = 18;
    let no_thumb_x = all_x + value_w + 2;
    c.text(0, y, "Three-key patterns", CYAN);
    c.text(all_x, y, "ALL", MUTED);
    c.text(no_thumb_x, y, "NO THUMBS", MUTED);
    y += 1;
    for (label, all, no_thumbs) in [
        ("Alternation", "alt", "altnothumbs"),
        ("Alt & SFS", "altsfs", "altsfsnothumbs"),
        ("Redirect total", "redirect", "redirectnothumbs"),
        ("Red & SFS", "redirectsfs", "redirectsfsnothumbs"),
        ("Weak redirect", "redirectweak", "redirectweaknothumbs"),
        (
            "Weak & SFS",
            "redirectsfsweak",
            "redirectsfsweaknothumbs",
        ),
        ("Roll total", "roll", "rollnothumbs"),
        ("Inroll2", "inroll2", "inroll2nothumbs"),
        ("Outroll2", "outroll2", "outroll2nothumbs"),
        ("Inroll3", "inroll3", "inroll3nothumbs"),
        ("Outroll3", "outroll3", "outroll3nothumbs"),
    ] {
        c.text(0, y, label, FG);
        aklgg_stat_cell(c, all_x, y, value_w, all, before, after, decimals);
        aklgg_stat_cell(c, no_thumb_x, y, value_w, no_thumbs, before, after, decimals);
        y += 1;
    }

    y += 1;
    let label_w = 8;
    let col_w = decimals + 6;
    let left_x = label_w;
    let right_x = left_x + col_w * 3 + 3;
    c.text(0, y, "Finger speed", CYAN);
    c.text(left_x, y, "Left", MUTED);
    c.text(right_x, y, "Right", MUTED);
    y += 1;
    for side_x in [left_x, right_x] {
        c.text(side_x, y, "Usage", MUTED);
        c.text(side_x + col_w, y, "Fspeed", MUTED);
        c.text(side_x + col_w * 2, y, "Weighted", MUTED);
    }
    y += 1;
    let finger_rows = [
        ("pinky", "LP", "RP"),
        ("ring", "LR", "RR"),
        ("middle", "LM", "RM"),
        ("index", "LI", "RI"),
        ("thumb", "LT", "RT"),
    ];
    for (label, left, right) in finger_rows {
        c.text(0, y, label, FG);
        for (side_x, suffix) in [(left_x, left), (right_x, right)] {
            let usage = format!("finger-usage-{suffix}");
            let fspeed = format!("fspeed-{suffix}");
            let weighted = format!("weighted-speed-{suffix}");
            c.text(
                side_x,
                y,
                &short(&aklgg_number(after, &usage, decimals), col_w),
                FG,
            );
            aklgg_stat_cell(c, side_x + col_w, y, col_w, &fspeed, before, after, decimals);
            aklgg_stat_cell(
                c,
                side_x + col_w * 2,
                y,
                col_w,
                &weighted,
                before,
                after,
                decimals,
            );
        }
        y += 1;
    }

    let side_total = |suffixes: [&str; 5], prefix: &str, stats: &mana2_metrics::Stats| {
        suffixes
            .iter()
            .map(|suffix| stats.values[aklgg_index(&format!("{prefix}-{suffix}"))])
            .sum::<f64>()
    };
    for (side_x, suffixes) in [
        (left_x, ["LP", "LR", "LM", "LI", "LT"]),
        (right_x, ["RP", "RR", "RM", "RI", "RT"]),
    ] {
        for (column, prefix) in [
            (0, "finger-usage"),
            (1, "fspeed"),
            (2, "weighted-speed"),
        ] {
            let current = side_total(suffixes, prefix, after);
            let baseline = side_total(suffixes, prefix, before);
            let color = if column == 0 {
                FG
            } else if current < baseline - DISPLAY_EPSILON {
                GREEN
            } else if current > baseline + DISPLAY_EPSILON {
                RED
            } else {
                FG
            };
            let value = number(current, decimals);
            let value = if column == 0 {
                format!("{value}%")
            } else {
                value
            };
            c.text(
                side_x + column * col_w,
                y,
                &short(&value, col_w),
                color,
            );
        }
    }
    c.text(0, y, "total", FG);
    y += 1;
    c.text(0, y, "both", FG);
    let usage_total = side_total(["LP", "LR", "LM", "LI", "LT"], "finger-usage", after)
        + side_total(["RP", "RR", "RM", "RI", "RT"], "finger-usage", after);
    c.text(
        left_x,
        y,
        &format!("{}%", number(usage_total, decimals)),
        FG,
    );
    aklgg_stat_cell(c, left_x + col_w, y, col_w, "fspeed", before, after, decimals);
    aklgg_stat_cell(
        c,
        left_x + col_w * 2,
        y,
        col_w,
        "weighted_speed",
        before,
        after,
        decimals,
    );
    y + 1
}

fn typing_policy_row(c: &mut Canvas, y: usize, weights: &Weights) -> usize {
    let text = format!("Alternate fingers: {}", weights.alt_fingers_label());
    c.text(0, y, &short(&text, c.w), FG);
    c.hit(
        Rect {
            x: 0,
            y,
            w: text.len().min(c.w),
            h: 1,
        },
        Action::Command('f'),
    );
    y + 1
}

fn combo_policy_row(c: &mut Canvas, y: usize, weights: &Weights) -> usize {
    let value = if weights.4 { "enabled" } else { "disabled" };
    let text = format!("Combos: {value}");
    c.text(0, y, &short(&text, c.w), FG);
    c.hit(
        Rect {
            x: 0,
            y,
            w: text.chars().count().min(c.w),
            h: 1,
        },
        Action::Command('F'),
    );
    y + 1
}

pub(crate) fn spacegram_policy_row(c: &mut Canvas, y: usize, enabled: bool) -> usize {
    let (value, color) = if enabled {
        ("ON", GREEN)
    } else {
        ("OFF", MUTED)
    };
    let text = format!("Spacegrams: {value}");
    c.text(0, y, &short(&text, c.w), color);
    c.hit(
        Rect {
            x: 0,
            y,
            w: text.chars().count().min(c.w),
            h: 1,
        },
        Action::Command('z'),
    );
    y + 1
}

fn finger_table(
    c: &mut Canvas,
    y: usize,
    before: &Metrics,
    after: &Metrics,
    decimals: usize,
) -> usize {
    const ORDER: [usize; 10] = [0, 1, 2, 3, 8, 4, 5, 6, 7, 9];
    let label = 8usize;
    let mut fw = decimals + 4;
    for &i in &ORDER {
        fw = fw.max(number(after.usage[i], decimals).chars().count() + 2);
        if i < 8 {
            fw = fw.max(number(after.off[i], decimals).chars().count() + 2);
            fw = fw.max(
                delta_text(before.off[i], after.off[i], decimals)
                    .chars()
                    .count()
                    + 2,
            );
        }
    }
    let cols = if label + 2 + 10 * (fw + 1) <= c.w {
        10
    } else {
        5
    };
    let max_fw = c.w.saturating_sub(label + 2) / cols;
    fw = fw.min(max_fw.saturating_sub(1).max(3));
    let blocks = 10 / cols;
    let width = label + 2 + cols * (fw + 1);
    let x = c.w.saturating_sub(width) / 2;
    let h = 2 + blocks * 4;
    c.boxed(Rect { x, y, w: width, h }, BORDER);
    for col in 0..cols {
        let xx = x + 1 + label + col * (fw + 1);
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
            let i = ORDER[block * cols + col];
            let xx = x + 2 + label + col * (fw + 1);
            c.center(xx, yy, fw, FINGER_NAMES[i], MUTED);
            c.right(
                xx + 1,
                yy + 1,
                fw - 2,
                &number(after.usage[i], decimals),
                CYAN,
            );
            if i < 8 {
                c.right(
                    xx + 1,
                    yy + 2,
                    fw - 2,
                    &number(after.off[i], decimals),
                    change_color(0, before.off[i], after.off[i]),
                );
                c.right(
                    xx + 1,
                    yy + 3,
                    fw - 2,
                    &delta_text(before.off[i], after.off[i], decimals),
                    delta_color(0, before.off[i], after.off[i]),
                );
            } else {
                c.right(xx + 1, yy + 2, fw - 2, "-", MUTED);
                c.right(xx + 1, yy + 3, fw - 2, "-", MUTED);
            }
        }
    }
    y + h
}

fn score_panel(
    c: &mut Canvas,
    y: usize,
    before: &Breakdown,
    after: &Breakdown,
    ensemble: Option<(f64, f64)>,
    decimals: usize,
) -> usize {
    let mut items = vec![
        ("Penalties", before.penalty, after.penalty, false),
        ("− Rewards", before.bonus, after.bonus, true),
        ("= Score", before.net, after.net, false),
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
        if x > 0 && x + 5 + len > c.w {
            yy += 1;
            x = 0;
        }
        if x > 0 {
            c.text(x, yy, "  │  ", BORDER);
            x += 5;
        }
        let color = if (a - b).abs() <= DISPLAY_EPSILON {
            FG
        } else if (a < b) ^ credit {
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

/// One weight's part of the objective for the Score tab: its stat and points
/// at the start and now. Points are lower-is-better (rewards are negative)
/// and sum to the objective exactly.
pub(crate) struct ScoreRow {
    name: String,
    stat: (f64, f64),
    weight: String,
    points: (f64, f64),
}

/// Normal mode: `breakdown` per weight, with the stat each weight multiplies.
pub(crate) fn metric_score_rows(before: &Metrics, after: &Metrics, w: &Weights) -> Vec<ScoreRow> {
    let (start, now) = (breakdown(before, w), breakdown(after, w));
    let stat = |m: &Metrics, i: usize| {
        if i < N_METRICS {
            m.v[i]
        } else if i < USAGE_WEIGHT {
            m.off[i - N_METRICS] + m.off[7 - (i - N_METRICS)]
        } else {
            m.usage[i - USAGE_WEIGHT] + m.usage[7 - (i - USAGE_WEIGHT)]
        }
    };
    (0..N_WEIGHTS)
        .filter(|&i| start.contributions[i] != 0.0 || now.contributions[i] != 0.0)
        .map(|i| ScoreRow {
            name: weight_display_name(i).into(),
            stat: (stat(before, i), stat(after, i)),
            weight: config_number(w.0[i]),
            points: (start.contributions[i], now.contributions[i]),
        })
        .collect()
}

/// aklgg mode: each `[aklgg]` stat's weighted contribution, sign flipped to
/// the optimizer's lower-is-better convention.
pub(crate) fn aklgg_score_rows(
    before: &mana2_metrics::Stats,
    after: &mana2_metrics::Stats,
    w: &Weights,
) -> Vec<ScoreRow> {
    let start = mana2_metrics::score_contributions(before, &w.2);
    let now = mana2_metrics::score_contributions(after, &w.2);
    mana2_metrics::WEIGHTABLE_STATS
        .iter()
        .filter(|&&i| start[i] != 0.0 || now[i] != 0.0)
        .map(|&i| ScoreRow {
            name: mana2_metrics::STAT_IDS[i].into(),
            stat: (before.values[i], after.values[i]),
            weight: w.2.schedule_text(i),
            points: (-start[i], -now[i]),
        })
        .collect()
}

/// The optimizer's [4] Score tab: where the objective comes from, largest
/// parts first. Bars compare penalties with penalties and rewards with rewards.
pub(crate) fn score_frame(
    width: usize,
    mut rows: Vec<ScoreRow>,
    decimals: usize,
) -> Canvas {
    rows.sort_by(|a, b| {
        b.points.1.abs().total_cmp(&a.points.1.abs()).then_with(|| a.name.cmp(&b.name))
    });
    // Callers write their status and key lines on the last three rows.
    let mut c = Canvas::optimizer(width, rows.len() + 12);
    let total = |pick: fn(&ScoreRow) -> f64, penalty: bool| -> f64 {
        rows.iter().map(pick).filter(|&v| (v > 0.0) == penalty).sum()
    };
    let (penalty, credit) = (total(|r| r.points.1, true), -total(|r| r.points.1, false));
    let (start_penalty, start_credit) = (total(|r| r.points.0, true), -total(|r| r.points.0, false));
    let largest = rows.iter().map(|r| r.points.1.abs()).fold(0.0, f64::max);
    let mut y = 3;
    c.text(
        0,
        y,
        &short(
            "Points = stat × weight; lower is better, rewards are negative. Share: of all penalties (red) or all rewards (green).",
            c.w,
        ),
        MUTED,
    );
    y += 2;
    let columns = [(0, "Weight"), (18, "Stat start → now"), (42, "Weight"), (56, "Points start → now"), (80, "Δ"), (91, "Share")];
    for (x, label) in columns {
        if x < c.w {
            c.text(x, y, label, CYAN);
        }
    }
    y += 1;
    let bar_width = c.w.saturating_sub(91 + 8).min(30);
    for row in &rows {
        let (before, after) = row.points;
        let delta = after - before;
        let color = if delta.abs() <= DISPLAY_EPSILON {
            FG
        } else if delta < 0.0 {
            GREEN
        } else {
            RED
        };
        c.text(0, y, &short(&row.name, 17), FG);
        c.text(
            18,
            y,
            &format!("{} → {}", fixed(row.stat.0, decimals), fixed(row.stat.1, decimals)),
            MUTED,
        );
        c.text(42, y, &short(&row.weight, 13), MUTED);
        c.text(56, y, &format!("{} → ", fixed(before, decimals)), MUTED);
        c.text(56 + fixed(before, decimals).chars().count() + 3, y, &fixed(after, decimals), color);
        c.text(80, y, &format!("{:+.*}", decimals, delta), color);
        let side = if after > 0.0 { penalty } else { credit };
        let share = if side > 0.0 { after.abs() / side * 100.0 } else { 0.0 };
        let filled = if largest > 0.0 {
            (after.abs() / largest * bar_width as f64).round() as usize
        } else {
            0
        };
        let bar_color = if after > 0.0 { RED } else { GREEN };
        c.text(91, y, &"█".repeat(filled), bar_color);
        c.text(91 + bar_width + 1, y, &format!("{share:4.1}%"), MUTED);
        y += 1;
    }
    y += 1;
    let line = format!(
        "Penalties {} → {}   − Rewards {} → {}   = Score {} → {}",
        fixed(start_penalty, decimals),
        fixed(penalty, decimals),
        fixed(start_credit, decimals),
        fixed(credit, decimals),
        fixed(start_penalty - start_credit, decimals),
        fixed(penalty - credit, decimals),
    );
    c.text(0, y, &short(&line, c.w), FG);
    c
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
            if !text.is_empty() && text.chars().count() + 1 + word.chars().count() > width {
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
            c.text(
                0,
                i + 2,
                &short(line, c.w),
                if line.starts_with("ERROR") { RED } else { FG },
            );
        }
        term.present(&c, scroll)?;
        let e = term.event()?;
        if scroll_event(&e, &mut scroll, c.h, term.size.1) {
            continue;
        }
        if matches!(
            e,
            Event::Escape | Event::Quit | Event::Char('q') | Event::Enter
        ) {
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
            cell.color = DIM;
            cell.background = None;
            cell.bold = false;
            c.cells[y * w + x] = cell;
        }
    }
    let width = w.saturating_sub(4).max(4).min(80).min(w);
    let content_width = width.saturating_sub(4).max(1);
    let key_width = lines
        .iter()
        .filter_map(|line| line.split_once('\t'))
        .map(|(key, _)| key.chars().count() + 2)
        .max()
        .unwrap_or(0)
        .min(content_width / 2);
    let mut formatted = Vec::new();
    for line in lines {
        if let Some((key, description)) = line.split_once('\t') {
            if key.chars().count() + 2 > key_width {
                // Keep long key labels readable in narrow terminals.
                for text in wrap_lines(&[key.into()], content_width) {
                    let bold = text.chars().count();
                    formatted.push((text, bold));
                }
                for text in wrap_lines(&[description.into()], content_width) {
                    formatted.push((text, 0));
                }
            } else {
                for (index, text) in wrap_lines(
                    &[description.into()],
                    content_width.saturating_sub(key_width).max(1),
                )
                .into_iter()
                .enumerate()
                {
                    let label = if index == 0 { key } else { "" };
                    formatted.push((
                        format!("{label:width$}{text}", width = key_width),
                        label.chars().count(),
                    ));
                }
            }
        } else {
            formatted.extend(
                wrap_lines(&[line.clone()], content_width)
                    .into_iter()
                    .map(|text| (text, 0)),
            );
        }
    }
    let lines = formatted;
    let height = (lines.len() + 4).min(h.saturating_sub(2)).max(4).min(h);
    let capacity = height.saturating_sub(4);
    *scroll = (*scroll).min(lines.len().saturating_sub(capacity));
    let r = Rect {
        x: (w - width) / 2,
        y: (h - height) / 2,
        w: width,
        h: height,
    };
    for y in r.y..r.y + r.h {
        for x in r.x..r.x + r.w {
            c.cells[y * w + x] = Cell {
                ch: ' ',
                color: FG,
                background: None,
                bold: false,
            };
        }
    }
    c.boxed(r, BORDER);
    c.text(r.x + 2, r.y, &short(title, width.saturating_sub(4)), CYAN);
    for (index, (line, bold)) in lines.iter().skip(*scroll).take(capacity).enumerate() {
        let y = r.y + 2 + index;
        c.text(r.x + 2, y, &short(line, content_width), FG);
        // Keys in yellow, like the ranker's options.
        for x in (r.x + 2)..(r.x + 2 + (*bold).min(content_width)) {
            c.cells[y * w + x].bold = true;
            c.cells[y * w + x].color = YELLOW;
        }
    }
    if height >= 4 {
        c.text(
            r.x + 2,
            r.y + height - 2,
            &short("↑↓ scroll · Esc / ? close", width.saturating_sub(4)),
            MUTED,
        );
    }
    (c, capacity, lines.len())
}

/// Main help menus that end with the fat-finger combo state.
const COMBO_STATE_MENUS: [&str; 5] = [
    "Editor controls",
    "Optimizer",
    "Optimizer search",
    "Optimizer result",
    "Ranker help",
];

pub(crate) fn help_popup(
    term: &mut Terminal,
    base: &Canvas,
    title: &str,
    lines: &[String],
) -> AppResult<()> {
    let with_state;
    let lines = if COMBO_STATE_MENUS.contains(&title) {
        let on = load_app_config().is_ok_and(|config| config.combos.fatfingering);
        let mut all = lines.to_vec();
        all.push(format!("Combos\tfatfinger {}", if on { "on" } else { "off" }));
        with_state = all;
        &with_state[..]
    } else {
        lines
    };
    let mut scroll = 0usize;
    // Use the visible frame below the top bar, including the caller's
    // scrolling, as the backdrop.
    let (w, h) = term.size;
    let snapshot = if term.bar == TOP_BAR && term.previous.len() == w * (h + TOP_BAR) {
        Some(Canvas {
            w,
            h,
            cells: term.previous[w * TOP_BAR..].to_vec(),
            hits: Vec::new(),
            tabs: None,
        })
    } else {
        None
    };
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

fn input_box(
    term: &mut Terminal,
    title: &str,
    hint: &str,
    initial: &str,
) -> AppResult<Option<String>> {
    let mut text = initial.to_string();
    let mut replace = true;
    loop {
        let mut c = Canvas::new(term.width(), 8);
        c.text(0, 0, title, FG);
        let _ = hint;
        c.boxed(
            Rect {
                x: 0,
                y: 2,
                w: c.w,
                h: 3,
            },
            BORDER,
        );
        c.text(2, 3, &short(&format!("{text}▏"), c.w.saturating_sub(4)), FG);
        term.present(&c, 0)?;
        match term.event()? {
            Event::Enter => return Ok(Some(text.trim().to_string())),
            Event::Escape | Event::Quit => return Ok(None),
            Event::Clear => {
                text.clear();
                replace = false;
            }
            Event::Backspace => {
                if replace {
                    text.clear();
                    replace = false;
                } else {
                    text.pop();
                }
            }
            Event::Char(ch) => {
                if replace {
                    text.clear();
                    replace = false;
                }
                if text.len() < 256 {
                    text.push(ch);
                }
            }
            Event::Paste(s) => {
                if replace {
                    text.clear();
                    replace = false;
                }
                text.extend(
                    s.chars()
                        .filter(|c| !c.is_control())
                        .take(256 - text.len().min(256)),
                );
            }
            _ => {}
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
            let x = i * (verb.len().max(6) + 8);
            let selected = if i == 0 { accept } else { !accept };
            c.text(x, 4, if selected { "›" } else { " " }, FG);
            let text = format!("[{name}]");
            c.text(x + 2, 4, &text, FG);
            c.hit(
                Rect {
                    x,
                    y: 4,
                    w: text.len() + 2,
                    h: 1,
                },
                Action::Command(if i == 0 { 'y' } else { 'n' }),
            );
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
            Event::Tab => accept = !accept,
            _ => {}
        }
    }
}

/// Scores `text` as a fuzzy match for `query`: every query character must
/// appear in order. Runs and word starts score higher; returns the score and
/// the matched character positions.
fn fuzzy_match(query: &str, text: &str) -> Option<(i32, Vec<usize>)> {
    let query: Vec<char> = query.to_lowercase().chars().filter(|c| *c != ' ').collect();
    let text: Vec<char> = text.to_lowercase().chars().collect();
    let Some(&first) = query.first() else {
        return Some((0, Vec::new()));
    };
    let mut best: Option<(i32, Vec<usize>)> = None;
    // Matching greedily from each place the first character occurs finds a
    // later contiguous run that a single leftmost pass would miss.
    for start in (0..text.len()).filter(|&i| text[i] == first) {
        let mut positions = vec![start];
        for &q in &query[1..] {
            let from = positions[positions.len() - 1] + 1;
            match text[from..].iter().position(|&c| c == q) {
                Some(offset) => positions.push(from + offset),
                None => break,
            }
        }
        if positions.len() < query.len() {
            break;
        }
        let score = positions
            .iter()
            .enumerate()
            .map(|(n, &at)| {
                let boundary = at == 0 || !text[at - 1].is_alphanumeric();
                if n > 0 && positions[n - 1] + 1 == at {
                    8
                } else if boundary {
                    6
                } else {
                    1
                }
            })
            .sum::<i32>()
            - (positions[positions.len() - 1] - start) as i32 / 4;
        if best.as_ref().is_none_or(|(top, _)| score > *top) {
            best = Some((score, positions));
        }
    }
    best
}

fn menu(term: &mut Terminal, title: &str, items: &[String]) -> AppResult<Option<usize>> {
    menu_with(term, title, items, false)
}

/// A menu that opens with a fuzzy search: typing filters at once.
pub(crate) fn search_menu(
    term: &mut Terminal,
    title: &str,
    items: &[String],
) -> AppResult<Option<usize>> {
    menu_with(term, title, items, true)
}

fn menu_with(
    term: &mut Terminal,
    title: &str,
    items: &[String],
    search: bool,
) -> AppResult<Option<usize>> {
    if items.is_empty() {
        return Ok(None);
    }
    let mut selected = 0usize;
    let mut query = String::new();
    let mut scroll = 0usize;
    loop {
        let mut matches: Vec<(usize, i32, Vec<usize>)> = if search {
            items
                .iter()
                .enumerate()
                .filter_map(|(i, s)| fuzzy_match(&query, s).map(|(score, at)| (i, score, at)))
                .collect()
        } else {
            items
                .iter()
                .enumerate()
                .filter(|(_, s)| s.to_ascii_lowercase().contains(&query.to_ascii_lowercase()))
                .map(|(i, _)| (i, 0, Vec::new()))
                .collect()
        };
        matches.sort_by_key(|&(i, score, _)| (-score, i));
        let filtered: Vec<usize> = matches.iter().map(|(i, _, _)| *i).collect();
        if selected >= filtered.len() {
            selected = filtered.len().saturating_sub(1);
        }
        let top = if query.is_empty() && !search { 2 } else { 3 };
        let mut c = Canvas::new(term.width(), filtered.len().max(1) + top + 1);
        c.text(0, 0, title, FG);
        if search {
            c.text(0, 1, &short(&format!("> {query}▏"), c.w), BLUE);
            if filtered.is_empty() {
                c.text(3, top, "No matches", MUTED);
            }
        } else if !query.is_empty() {
            c.text(0, 1, &format!("/{}", query), BLUE);
        }
        for (row, &i) in filtered.iter().enumerate() {
            let yy = row + top;
            let active = row == selected;
            c.text(
                1,
                yy,
                if active { "›" } else { " " },
                if active { BLUE } else { MUTED },
            );
            c.text(
                3,
                yy,
                &short(&items[i], c.w.saturating_sub(4)),
                if active { BLUE } else { FG },
            );
            let width = c.w.saturating_sub(5);
            for &at in matches[row].2.iter().filter(|&&at| at < width) {
                if let Some(ch) = items[i].chars().nth(at) {
                    c.text(3 + at, yy, &ch.to_string(), YELLOW);
                }
            }
            c.hit(
                Rect {
                    x: 0,
                    y: yy,
                    w: c.w,
                    h: 1,
                },
                Action::Item(i),
            );
        }
        c.text(
            0,
            filtered.len().max(1) + top,
            if search {
                "Type to search · ↑↓ select · Enter open · Esc back"
            } else {
                "? help"
            },
            MUTED,
        );
        term.present(&c, scroll)?;
        let e = term.event()?;
        if scroll_event(&e, &mut scroll, c.h, term.size.1) {
            continue;
        }
        if let Some(Action::Item(i)) = action_press(term, &c, &e, scroll) {
            return Ok(Some(i));
        }
        if search {
            let typed = match &e {
                Event::Char(ch) => {
                    query.push(*ch);
                    true
                }
                Event::Paste(text) => {
                    query.extend(text.chars().filter(|c| !c.is_control()).take(80));
                    true
                }
                Event::Backspace => query.pop().is_some(),
                Event::Clear => !std::mem::take(&mut query).is_empty(),
                _ => false,
            };
            if typed {
                selected = 0;
                scroll = 0;
                continue;
            }
        }
        match e {
            Event::Enter => {
                if let Some(&i) = filtered.get(selected) {
                    return Ok(Some(i));
                }
            }
            Event::Escape | Event::Quit | Event::Char('q') => return Ok(None),
            Event::Char('?') => {
                let lines = [
                    "j/k or ↑/↓: select; Enter or click: open.".into(),
                    "/: filter; mouse wheel / PgUp / PgDn: scroll.".into(),
                    "q or Esc: back.".into(),
                ];
                help_popup(term, &c, title, &lines)?;
            }
            Event::Up | Event::Char('k') => selected = selected.saturating_sub(1),
            Event::Down | Event::Char('j') => {
                selected = (selected + 1).min(filtered.len().saturating_sub(1))
            }
            Event::Char('/') => {
                if let Some(s) = input_box(term, "Filter", "", &query)? {
                    query = s;
                    selected = 0;
                    scroll = 0;
                }
            }
            _ => {}
        }
        if selected + top < scroll {
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
    title: &str,
) -> AppResult<Option<PathBuf>> {
    let paths = if dir == CORPUS_DIR {
        corpus_paths()?
    } else if dir == LAYOUT_DIR {
        discover_layouts(Path::new(dir))?
    } else {
        discover(dir, ext, prefix)?
    };
    let names: Vec<_> = paths
        .iter()
        .map(|path| {
            if dir == LAYOUT_DIR {
                layout_file_label(path)
            } else {
                label(path, prefix)
            }
        })
        .collect();
    Ok(menu_with(term, title, &names, dir == LAYOUT_DIR)?.map(|i| paths[i].clone()))
}

fn show_corpus_info(term: &mut Terminal, c: &Corpus) -> AppResult<()> {
    let mut lines = vec![
        format!("Model: {MODEL_VERSION}"),
        format!(
            "Corpus: {}   content fingerprint {:016x}",
            c.name, c.fingerprint
        ),
        String::new(),
        "Coverage of STORED corpus tables (not the original raw text):".into(),
    ];
    for (i, name) in ["Letters", "Bigrams", "Skipgrams", "Trigrams"]
        .iter()
        .enumerate()
    {
        lines.push(format!(
            "{name:<12} {:8.4}%  denominator mass {:.6}",
            c.coverage[i], c.totals[i]
        ));
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
    color: u8,
) {
    if width == 0 {
        return;
    }
    let old = (before / maximum * (width - 1) as f64).round() as usize;
    let new = (after / maximum * (width - 1) as f64).round() as usize;
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

pub(crate) fn metric_detail_intro(c: &mut Canvas, mut y: usize, m: usize) -> usize {
    let summary = match m {
        SFB => "Two consecutive presses on different keys using the same finger.",
        SFS => "Different keys using the same finger, with one press between them.",
        SKB => "The same physical key pressed twice consecutively.",
        SKS => "The same physical key repeated with one press between repeats.",
        ROLL => {
            "Trigrams containing an inward or outward roll; includes two- and three-finger rolls."
        }
        _ => METRIC_HELP[m].split(". ").next().unwrap_or(METRIC_HELP[m]),
    };
    let description = format!("{}: {}", METRIC_NAMES[m], summary);
    for line in wrap_lines(&[description], c.w.max(1)) {
        c.text(0, y, &line, FG);
        y += 1;
    }
    y + 1
}

pub(crate) fn metric_detail_footer(
    c: &mut Canvas,
    mut y: usize,
    m: usize,
    sort_change: bool,
    merge: bool,
    all: Option<bool>,
    view: Option<&str>,
) -> usize {
    let mut controls = vec![format!(
        "d sort: {}",
        if sort_change { "change" } else { "value" }
    )];
    if !is_rhythm(m) {
        controls.push(format!(
            "b pairs: {}",
            if merge { "merged" } else { "separate" }
        ));
    }
    if let Some(all) = all {
        controls.push(format!("a rows: {}", if all { "all" } else { "top" }));
    }
    if let Some(view) = view {
        controls.push(format!("x view: {view}"));
    }
    controls.push("? help".into());
    controls.push("q back".into());
    for line in wrap_lines(&[controls.join(" · ")], c.w.max(1)) {
        c.text(0, y, &line, MUTED);
        y += 1;
    }
    y + 1
}

fn contributor_view(
    term: &mut Terminal,
    m: usize,
    model: &Model,
    before: &[usize],
    after: &[usize],
    corpus: &Corpus,
    _optimizer_ui: bool,
) -> AppResult<()> {
    let mut sort_change = false;
    let mut merge = false;
    let mut scroll = 0;
    let mut view = CreditView::Clean;
    loop {
        let mut rows = contributor_data_mode(m, before, after, corpus, model, merge, view);
        let raw0 = full_raw(before, corpus, &model.geometry);
        let raw1 = full_raw(after, corpus, &model.geometry);
        let d0 = denominator(m, &raw0, corpus);
        let d1 = denominator(m, &raw1, corpus);
        if sort_change {
            rows.sort_by(|a, b| {
                let (a0, a1) = contributor_comparison(a.before, a.after, d0, d1);
                let (b0, b1) = contributor_comparison(b.before, b.after, d0, d1);
                (b1 - b0)
                    .abs()
                    .total_cmp(&(a1 - a0).abs())
                    .then_with(|| a.gram.cmp(&b.gram))
            });
        }
        let v0 = pct(
            contribution_mass(m, &raw0, view),
            denominator(m, &raw0, corpus),
        );
        let v1 = pct(
            contribution_mass(m, &raw1, view),
            denominator(m, &raw1, corpus),
        );
        let dp = DETAIL_DECIMALS;
        let mut c = Canvas::new(term.width(), 64);
        let title = if roll_metric(m) {
            format!(
                "{} · {}",
                METRIC_NAMES[m],
                roll_settings_label(model.geometry.rolls)
            )
        } else if raw_positive(m).is_some() {
            format!("{} · {}", METRIC_NAMES[m], view.name())
        } else {
            METRIC_NAMES[m].to_string()
        };
        header(&mut c, &title, &corpus.name, &model.board.name, "");
        let mut y = keyboard(&mut c, 2, model, after, before, None, None, &[]) + 1;
        y = metric_detail_intro(&mut c, y, m);
        if (d0 - d1).abs() > DISPLAY_EPSILON {
            for line in wrap_lines(
                &[
                    "Change ignores denominator shifts; Before/After keep actual percentages."
                        .into(),
                ],
                c.w.max(1),
            ) {
                c.text(0, y, &line, MUTED);
                y += 1;
            }
            y += 1;
        }
        c.text(
            0,
            y,
            &format!("{} → {} {}", number(v0, dp), number(v1, dp), metric_unit(m)),
            FG,
        );
        c.text(
            32,
            y,
            &format!(
                "○ before  ● after   {}",
                if sort_change {
                    "change"
                } else {
                    "contribution"
                }
            ),
            MUTED,
        );
        y += 2;
        let show_reasons = raw_positive(m).is_some() && view == CreditView::Rejected;
        let plot_x = if show_reasons && c.w >= 90 { 62 } else { 43 };
        let plot_w = c.w.saturating_sub(plot_x + 1).max(1);
        c.text(0, y, if merge { "Pair" } else { "Keys" }, MUTED);
        c.right(9, y, 9, "Before", MUTED);
        c.right(20, y, 9, "After", MUTED);
        c.right(31, y, 9, "Change", MUTED);
        if plot_x > 43 {
            c.text(43, y, "Blocked by", MUTED);
        }
        let maximum = rows
            .iter()
            .map(|r| r.before.max(r.after))
            .fold(0.0, f64::max)
            .max(1e-12);
        c.text(plot_x, y, "0", MUTED);
        c.right(
            plot_x + 2,
            y,
            plot_w.saturating_sub(2),
            &format!("{} {}", number(maximum, dp), metric_unit(m)),
            MUTED,
        );
        y += 1;
        let n = rows.len().min(16);
        for (i, row) in rows.iter().take(n).enumerate() {
            let yy = y + i;
            let (compare_before, compare_after) =
                contributor_comparison(row.before, row.after, d0, d1);
            let cm = if view == CreditView::Rejected { SFB } else { m };
            let color = if view == CreditView::Raw {
                FG
            } else {
                change_color(cm, compare_before, compare_after)
            };
            c.text(0, yy, &short(&row.gram, 8), FG);
            c.right(9, yy, 9, &number(row.before, dp), MUTED);
            c.right(20, yy, 9, &number(row.after, dp), color);
            c.right(
                31,
                yy,
                9,
                &delta_text(compare_before, compare_after, dp),
                if view == CreditView::Raw {
                    MUTED
                } else {
                    delta_color(cm, compare_before, compare_after)
                },
            );
            if show_reasons {
                let bits = if row.after > 0.0 {
                    row.why_after
                } else {
                    row.why_before
                };
                if plot_x > 43 {
                    c.text(43, yy, &short(&blocker_names(bits), plot_x - 44), MUTED);
                }
                c.hit(
                    Rect {
                        x: 0,
                        y: yy,
                        w: c.w,
                        h: 1,
                    },
                    Action::Item(i),
                );
            }
            contribution_bar(
                &mut c, plot_x, yy, plot_w, row.before, row.after, maximum, color,
            );
        }
        let sum0: f64 = rows.iter().take(n).map(|r| r.before).sum();
        let sum1: f64 = rows.iter().take(n).map(|r| r.after).sum();
        let other0 = (v0 - sum0).max(0.0);
        let other1 = (v1 - sum1).max(0.0);
        let (compare_other0, compare_other1) = contributor_comparison(other0, other1, d0, d1);
        y += n + 1;
        c.text(0, y, "Other", MUTED);
        c.right(9, y, 9, &number(other0, dp), MUTED);
        c.right(20, y, 9, &number(other1, dp), MUTED);
        c.right(
            31,
            y,
            9,
            &delta_text(compare_other0, compare_other1, dp),
            if view == CreditView::Raw {
                MUTED
            } else {
                delta_color(
                    if view == CreditView::Rejected { SFB } else { m },
                    compare_other0,
                    compare_other1,
                )
            },
        );
        let detail_view = raw_positive(m).map(|_| view.name());
        c.h = metric_detail_footer(&mut c, y + 2, m, sort_change, merge, None, detail_view);
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
                        &[
                            format!("Before: {}", blocker_names(row.why_before)),
                            format!("After:  {}", blocker_names(row.why_after)),
                        ],
                    )?;
                }
            }
        }
        match e {
            Event::Escape | Event::Quit | Event::Char('q') => return Ok(()),
            Event::Char('d') => sort_change = !sort_change,
            Event::Char('b') => {
                if !is_rhythm(m) {
                    merge = !merge;
                }
            }
            Event::Char('x') => {
                if raw_positive(m).is_some() {
                    view = view.next();
                    scroll = 0;
                }
            }
            Event::Char('?') => {
                let mut lines = vec![
                    METRIC_HELP[m].into(),
                    "Before/After show actual percentages. Change and its colors compare pattern contributions using one shared denominator, so denominator-only shifts stay neutral.".into(),
                    "d\tSort by current contribution or size of the change.".into(),
                ];
                if !is_rhythm(m) {
                    lines.push(
                        "b\tCombine a pair with its reverse, or show them separately.".into(),
                    );
                }
                if raw_positive(m).is_some() {
                    lines.push("x\tShow counted, total, or excluded contributions (clean / raw / rejected).".into());
                }
                lines.push("q\tReturn to the editor.".into());
                lines.push(
                    "The top 16 contributions are shown; Other contains the remainder.".into(),
                );
                if roll_metric(m) {
                    lines.push(roll_settings_label(model.geometry.rolls));
                }
                help_popup(term, &c, METRIC_NAMES[m], &lines)?;
            }
            _ => {}
        }
    }
}

fn objective_view(
    term: &mut Terminal,
    model: &Model,
    before: &[usize],
    after: &[usize],
    c: &Corpus,
    w: &Weights,
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
            a.contributions[i] - b.contributions[i]
        ));
    }
    lines.extend([
            String::new(),
            format!("Penalties         {:.4} → {:.4}", b.penalty, a.penalty),
            format!("− Rewards         {:.4} → {:.4}", b.bonus, a.bonus),
            format!("= Score           {:.4} → {:.4}", b.net, a.net),
            String::new(),
            "These are model score units, not measured comfort or typing speed.".into(),
            "FSB/FSS duplicate daFJB/daFJS; HSB/HSS duplicate daHJB/daHJS. Adjacent and nonadjacent jump weights score disjoint finger distances. Lateral stretch stays independent.".into(),
            "IN/OUT SRAF and IN/OUT ROLL have separate rewards. Combined SRAF/ROLL and roll type columns are display totals.".into()
        ]);
    info_page(term, "Score audit — selected corpus", &lines)
}

fn problem_objective_view(term: &mut Terminal, p: &Problem, arr: &[usize]) -> AppResult<()> {
    let mut old = [0.0; N_WEIGHTS];
    let mut new = [0.0; N_WEIGHTS];
    let mut lines = vec!["Training corpus       Share     Net before       Net after".into()];
    for (i, c) in p.corpora.iter().enumerate() {
        let b = breakdown(
            &metrics(&full_raw(&p.model.original, c, &p.model.geometry), c),
            &p.weights,
        );
        let a = breakdown(
            &metrics(&full_raw(arr, c, &p.model.geometry), c),
            &p.weights,
        );
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
            new[i] - old[i]
        ));
    }
    lines.push(String::new());
    lines.push(format!(
        "Mixture net        {:.6} -> {:.6}",
        old.iter().sum::<f64>(),
        new.iter().sum::<f64>()
    ));
    lines.push(
        "This breakdown is the actual search objective, including all training shares.".into(),
    );
    lines.push("Negative terms are preference credits, not measured ergonomic benefits.".into());
    info_page(term, "Search objective audit", &lines)
}

fn mana_stats_for(arr: &[usize], corpus: &Corpus, model: &Model) -> mana2_metrics::Stats {
    let raw = full_raw_with_mana2(arr, corpus, &model.geometry, true);
    mana_stats_for_corpus(&raw, corpus)
}

fn mana_objective_view(term: &mut Terminal, p: &Problem, arr: &[usize]) -> AppResult<()> {
    let mut before_terms = [0.0; mana2_metrics::N_STATS];
    let mut after_terms = [0.0; mana2_metrics::N_STATS];
    let mode = p.settings.mode.as_str();
    let physical_weights = &p.weights.2;
    let title = if mode == "aklgg" { "AKL.GG" } else { "Mana" };
    let score = |stats: &mana2_metrics::Stats| mana2_metrics::score(stats, physical_weights);
    let contributions = |stats: &mana2_metrics::Stats| {
        mana2_metrics::score_contributions(stats, physical_weights)
    };
    let mut lines = vec![format!("{title} objective · lower is better.")];
    for (corpus_index, corpus) in p.corpora.iter().enumerate() {
        let before = mana_stats_for(&p.model.original, corpus, &p.model);
        let after = mana_stats_for(arr, corpus, &p.model);
        lines.push(format!(
            "{} (share {:.3}): {:.4} → {:.4}",
            corpus.name,
            p.shares[corpus_index],
            -score(&before),
            -score(&after)
        ));
        for (index, value) in contributions(&before)
            .into_iter()
            .enumerate()
        {
            before_terms[index] -= p.shares[corpus_index] * value;
        }
        for (index, value) in contributions(&after)
            .into_iter()
            .enumerate()
        {
            after_terms[index] -= p.shares[corpus_index] * value;
        }
    }
    lines.push(String::new());
    lines.push("Stat                         Before       After      Change".into());
    for &index in mana2_metrics::WEIGHTABLE_STATS {
        let before = before_terms[index];
        let after = after_terms[index];
        if before.abs() <= DISPLAY_EPSILON && after.abs() <= DISPLAY_EPSILON {
            continue;
        }
        lines.push(format!(
            "{:<26} {:10.4}  {:10.4}  {:+10.4}",
            mana2_metrics::STAT_IDS[index],
            before,
            after,
            after - before
        ));
    }
    info_page(term, &format!("{title} objective audit"), &lines)
}

fn validation_view(
    term: &mut Terminal,
    model: &Model,
    before: &[usize],
    after: &[usize],
) -> AppResult<()> {
    let paths = corpus_paths()?;
    let mut lines =
        vec!["Corpus              SFB before → after    SFS before → after    Bi coverage".into()];
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
            }
            Err(e) => lines.push(format!("ERROR {}: {e}", label(&path, "corpus-"))),
        }
    }
    lines.extend([
        String::new(),
        "Same n-gram definitions, but each corpus has its own frequencies.".into(),
        "Validation never changes the result or silently changes training weights.".into(),
    ]);
    info_page(term, "Cross-corpus validation", &lines)
}

fn selected_breakdown(
    raw: &Raw,
    normal: &Metrics,
    corpus: &Corpus,
    weights: &Weights,
    mode: Option<&str>,
) -> Breakdown {
    if mode.is_some_and(|mode| mode == "aklgg") {
        mana_breakdown(&mana_stats_for_corpus(raw, corpus), weights)
    } else {
        breakdown(normal, weights)
    }
}

fn dashboard_from_raw(
    term: &Terminal,
    title: &str,
    controls: &str,
    model: &Model,
    arr: &[usize],
    baseline: &[usize],
    corpus: &Corpus,
    w: &Weights,
    baseline_raw: &Raw,
    current_raw: &Raw,
    baseline_metrics: &Metrics,
    current_metrics: &Metrics,
    locks: Option<&[bool]>,
    cursor: Option<usize>,
    status: &str,
    ensemble: Option<(f64, f64)>,
    optimizer_ui: bool,
    optimizer_mode: Option<&str>,
) -> Canvas {
    let mut c = Canvas::new(term.width(), 96);
    header(
        &mut c,
        title,
        &corpus.name,
        &model.board.name,
        if optimizer_ui { controls } else { "" },
    );
    let mut y = keyboard(&mut c, 2, model, arr, baseline, locks, cursor, &[]);
    // The optimizer's [alts] rules, named by the letters now at those spots;
    // green while on, a click turns them off.
    let alts = w.1.alts.filter(|_| title == "Optimizer");
    let rules = alts.map(|all| all.for_keys(&model.board.keys));
    if let Some(rules) = rules.filter(|rules| !rules.is_empty()) {
        let main = model.board.keys.iter().zip(arr).filter(|(key, _)| key.main);
        let names = crate::alt_fingers::position_names(main.map(|(key, &symbol)| {
            let symbol = model.canonical[symbol];
            let name = if symbol == b' ' {
                "␣".to_owned()
            } else {
                display_symbol(symbol).to_string()
            };
            (key.row, key.col, name)
        }));
        let source = crate::alt_fingers::AltRules::text_with(rules, names);
        let text = crate::action_ui::alts_label(w.3, &source);
        c.text(0, y + 1, &short(&text, c.w), if w.3 { GREEN } else { RED });
        let width = text.chars().count().min(c.w);
        c.hit(Rect { x: 0, y: y + 1, w: width, h: 1 }, Action::Command('f'));
        y += 3;
    }
    let aklgg = optimizer_mode == Some("aklgg");
    if optimizer_ui {
        if aklgg {
            y = aklgg_stat_table(
                &mut c,
                y,
                &mana_stats_for_corpus(baseline_raw, corpus),
                &mana_stats_for_corpus(current_raw, corpus),
                term.decimals(),
            );
        } else {
            y = grouped_metric_cards(
                &mut c,
                y,
                baseline_metrics,
                current_metrics,
                current_raw,
                corpus,
                term.decimals(),
                term.simple_stats,
            );
            y = finger_table(
                &mut c,
                y + 1,
                baseline_metrics,
                current_metrics,
                term.decimals(),
            );
        }
        y = score_panel(
            &mut c,
            y + 1,
            &selected_breakdown(baseline_raw, baseline_metrics, corpus, w, optimizer_mode),
            &selected_breakdown(current_raw, current_metrics, corpus, w, optimizer_mode),
            ensemble,
            term.decimals(),
        );
    } else if title == "Optimizer" && term.optimizer_weights {
        y = optimizer_metric_cards(
            &mut c,
            y,
            baseline_metrics,
            current_metrics,
            current_raw,
            corpus,
            w,
            term.decimals(),
            term.simple_stats,
            true,
        );
        y = finger_table(
            &mut c,
            y + 1,
            baseline_metrics,
            current_metrics,
            term.decimals(),
        );
        y = score_panel(
            &mut c,
            y + 1,
            &breakdown(baseline_metrics, w),
            &breakdown(current_metrics, w),
            ensemble,
            term.decimals(),
        );
    } else if title == "Optimizer" {
        if aklgg {
            y = aklgg_stat_table(
                &mut c,
                y,
                &mana_stats_for_corpus(baseline_raw, corpus),
                &mana_stats_for_corpus(current_raw, corpus),
                term.decimals(),
            );
        } else {
            y = optimizer_metric_cards(
                &mut c,
                y,
                baseline_metrics,
                current_metrics,
                current_raw,
                corpus,
                w,
                term.decimals(),
                term.simple_stats,
                false,
            );
            y = finger_table(
                &mut c,
                y + 1,
                baseline_metrics,
                current_metrics,
                term.decimals(),
            );
        }
        y = score_panel(
            &mut c,
            y + 1,
            &selected_breakdown(baseline_raw, baseline_metrics, corpus, w, optimizer_mode),
            &selected_breakdown(current_raw, current_metrics, corpus, w, optimizer_mode),
            ensemble,
            term.decimals(),
        );
        y = spacegram_policy_row(&mut c, y + 1, corpus.include_spacegrams);
    } else {
        y = editor_metric_cards(
            &mut c,
            y,
            baseline_metrics,
            current_metrics,
            current_raw,
            corpus,
            term.decimals(),
            term.simple_stats,
        );
        y = finger_table(
            &mut c,
            y + 1,
            baseline_metrics,
            current_metrics,
            term.decimals(),
        );
        y = score_panel(
            &mut c,
            y + 1,
            &breakdown(baseline_metrics, w),
            &breakdown(current_metrics, w),
            ensemble,
            term.decimals(),
        );
        y = spacegram_policy_row(&mut c, y + 1, corpus.include_spacegrams);
    }
    c.text(0, y, &short(status, c.w), CYAN);
    c.text(
        0,
        y + 1,
        &format!("m stats: {} · ? help", term.stats_view_name()),
        MUTED,
    );
    c.h = y + 3;
    c
}

fn edit_single_weight(term: &mut Terminal, w: &mut Weights, i: usize) -> AppResult<()> {
    if aggregate(i) {
        return Err("Display-only totals have no separate weight; edit their directional or D/C components.".into());
    }
    let hint = if i < N_METRICS {
        METRIC_HELP[i]
    } else if i < N_METRICS + 4 {
        "Extra penalty per percentage point of off-home use by this finger pair."
    } else {
        "Extra penalty per percentage point of total use by this finger pair."
    };
    if let Some(v) = input_box(
        term,
        &format!("Weight: {}", weight_display_name(i)),
        hint,
        &format!("{}", w.0[i]),
    )? {
        w.0[i] = finite_nonnegative(&v)?;
    }
    Ok(())
}

pub(crate) fn edit_physical_weight(
    term: &mut Terminal,
    weights: &mut Weights,
    index: usize,
) -> AppResult<bool> {
    let id = mana2_metrics::STAT_IDS
        .get(index)
        .ok_or("unknown physical-stat weight")?;
    let title = format!("AKL.GG {id}: [weight, boundary, ..., weight]");
    let initial = weights.2.schedule_text(index);
    match input_box(term, &title, "", &initial)? {
        Some(value) => weights.2.set_schedule_text(index, &value),
        None => Ok(false),
    }
}

pub(crate) fn choose_physical_weight(term: &mut Terminal, weights: &mut Weights) -> AppResult<bool> {
    let ids = mana2_metrics::WEIGHTABLE_STATS
        .iter()
        .copied()
        .collect::<Vec<_>>();
    let names = ids
        .iter()
        .map(|&index| {
            format!(
                "{}  {}",
                mana2_metrics::STAT_IDS[index],
                &weights.2.schedule_text(index)
            )
        })
        .collect::<Vec<_>>();
    match menu(term, "Choose an AKL.GG weight", &names)? {
        Some(choice) => edit_physical_weight(term, weights, ids[choice]),
        None => Ok(false),
    }
}

fn choose_optimizer_weight(term: &mut Terminal, weights: &mut Weights) -> AppResult<()> {
    let ids: Vec<_> = (0..N_WEIGHTS).filter(|&i| !aggregate(i)).collect();
    let names = ids
        .iter()
        .map(|&i| weight_display_name(i).to_string())
        .collect::<Vec<_>>();
    if let Some(i) = menu(term, "Choose a weight", &names)? {
        edit_single_weight(term, weights, ids[i])?;
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
    let raw = full_raw(&model.original, &c, &model.geometry);
    timing.mark("Initial metrics");
    editor_with_analysis(term, model, c, w, raw, Some(timing))
}

fn plain_corpus_with_spacegrams(
    model: &Model,
    corpus: &Corpus,
    enabled: bool,
) -> AppResult<Corpus> {
    let mut limits = load_app_config()?.ngrams;
    limits.include_spacegrams = enabled;
    let text = fs::read_to_string(&corpus.source_path)?;
    let source = Source::from_text_with_limits(&text, &corpus.source_path, limits)?;
    model.corpus(&source)
}

// Opens an ordinary editor from a ranker's already prepared evaluator state.
// The supplied raw counters describe model.original under corpus and weights.
fn editor_prepared(
    term: &mut Terminal,
    model: Model,
    corpus: Corpus,
    weights: Weights,
    raw: Raw,
) -> AppResult<()> {
    editor_with_analysis(term, model, corpus, weights, raw, None)
}

fn editor_with_analysis(
    term: &mut Terminal,
    model: Model,
    mut corpus: Corpus,
    weights: Weights,
    initial_raw: Raw,
    mut timing: Option<crate::load_profile::LoadProfile>,
) -> AppResult<()> {
    let w = weights;

    let mut arr = model.original.clone();
    let mut baseline = arr.clone();
    // Cursor movement, scrolling and redraws reuse these snapshots. A binding
    // edit invalidates current_*; saving changes only the comparison baseline;
    // changing corpus invalidates both.
    let mut current_raw = initial_raw.clone();
    let mut baseline_metrics = metrics(&initial_raw, &corpus);
    let mut current_metrics = baseline_metrics.clone();
    let mut previous_arr = arr.clone();
    let mut previous_metrics = current_metrics.clone();
    let mut isolated_changes = false;
    let mut undo: Vec<Vec<usize>> = Vec::new();
    let mut redo: Vec<Vec<usize>> = Vec::new();
    let mut cursor = 0;
    let mut keyboard_mode = false;
    let mut selected = None;
    let mut drag = None;
    let mut scroll = 0;
    let mut status = String::new();
    if let Some(timing) = timing.as_mut() {
        timing.mark("Editor state");
    }
    let mut opening = timing;
    loop {
        let highlights = selected.into_iter().chain(drag).collect::<Vec<_>>();
        let mut frame = dashboard_from_raw(
            term,
            if arr == baseline {
                "Layout editor"
            } else {
                "Layout editor *"
            },
            "? help · q back",
            &model,
            &arr,
            &baseline,
            &corpus,
            &w,
            &current_raw,
            &current_raw,
            if isolated_changes {
                &previous_metrics
            } else {
                &baseline_metrics
            },
            &current_metrics,
            None,
            if keyboard_mode { Some(cursor) } else { None },
            &status,
            None,
            false,
            None,
        );
        let comparison = if isolated_changes {
            "Latest edit"
        } else {
            "Loaded / saved layout"
        };
        frame.text(
            0,
            frame.h - 2,
            &short(
                &format!(
                    "Compare: {comparison} · m stats: {} · ? help · q back",
                    term.stats_view_name()
                ),
                frame.w,
            ),
            MUTED,
        );
        if !highlights.is_empty() {
            keyboard(
                &mut frame,
                2,
                &model,
                &arr,
                &baseline,
                None,
                if keyboard_mode { Some(cursor) } else { None },
                &highlights,
            );
        }
        term.present(&frame, scroll)?;
        if let Some(mut timing) = opening.take() {
            timing.mark("First dashboard metrics, rendering and presentation");
        }
        let mut e = term.event()?;
        if scroll_event(&e, &mut scroll, frame.h, term.size.1) {
            continue;
        }
        if let Some(Action::Command(command)) = action_press(term, &frame, &e, scroll) {
            e = Event::Char(command);
        }
        if let Some((dr, dc)) = direction(&e) {
            cursor = move_cursor(cursor, dr, dc, &model.board.keys);
            keyboard_mode = true;
            continue;
        }
        let mut swap = None;
        match e.clone() {
            Event::Char('m') => {
                term.toggle_stats_view();
                scroll = 0;
            },
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
                        contributor_view(term, m, &model, if isolated_changes { &previous_arr } else { &baseline }, &arr, &corpus, false)?;
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
            Event::Char('d') => {
                isolated_changes = !isolated_changes;
                status.clear();
            },
            Event::Char('.') => {
                term.precise=!term.precise;
            },
            Event::Char('u') => {
                if let Some(a) = undo.pop() {
                    let changed = arr != a;
                    if changed {
                        previous_arr = arr.clone();
                        previous_metrics = current_metrics.clone();
                    }
                    redo.push(arr);
                    arr = a;
                    if changed {
                        current_raw = full_raw(&arr, &corpus, &model.geometry);
                        current_metrics = metrics(&current_raw, &corpus);
                    }
                    selected = None;
                    status = "undo".into();
                }
            },
            Event::Char('U') => {
                if let Some(a) = redo.pop() {
                    let changed = arr != a;
                    if changed {
                        previous_arr = arr.clone();
                        previous_metrics = current_metrics.clone();
                    }
                    undo.push(arr);
                    arr = a;
                    if changed {
                        current_raw = full_raw(&arr, &corpus, &model.geometry);
                        current_metrics = metrics(&current_raw, &corpus);
                    }
                    selected = None;
                    status = "redo".into();
                }
            },
            Event::Char('r') => {
                let changed = arr != model.original;
                if changed {
                    previous_arr = arr.clone();
                    previous_metrics = current_metrics.clone();
                }
                undo.push(arr);
                arr = model.original.clone();
                if changed {
                    current_raw = full_raw(&arr, &corpus, &model.geometry);
                    current_metrics = metrics(&current_raw, &corpus);
                }
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
                    baseline_metrics = current_metrics.clone();
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
                        Ok(Some(next_corpus)) => {
                            corpus = next_corpus;
                            let baseline_raw = full_raw(&baseline, &corpus, &model.geometry);
                            baseline_metrics = metrics(&baseline_raw, &corpus);
                            current_raw = full_raw(&arr, &corpus, &model.geometry);
                            current_metrics = metrics(&current_raw, &corpus);
                            previous_metrics = metrics(&full_raw(&previous_arr, &corpus, &model.geometry), &corpus);
                            selected = None;
                            drag = None;
                            status = format!("Corpus changed to {}", corpus.name);
                        },
                        Ok(None) => {},
                        Err(error) => status = format!("Corpus change failed: {error}"),
                    }
                }
            },
            Event::Char('z') => {
                let enabled = !corpus.include_spacegrams;
                match plain_corpus_with_spacegrams(&model, &corpus, enabled) {
                    Ok(next_corpus) => match save_spacegram_setting(enabled) {
                        Ok(()) => {
                            corpus = next_corpus;
                            let baseline_raw = full_raw(&baseline, &corpus, &model.geometry);
                            baseline_metrics = metrics(&baseline_raw, &corpus);
                            current_raw = full_raw(&arr, &corpus, &model.geometry);
                            current_metrics = metrics(&current_raw, &corpus);
                            previous_metrics = metrics(&full_raw(&previous_arr, &corpus, &model.geometry), &corpus);
                            status = format!("Spacegrams {}", if enabled { "ON" } else { "OFF" });
                        },
                        Err(error) => status = format!("Spacegram setting was not changed: {error}"),
                    },
                    Err(error) => status = format!("Spacegram change failed: {error}"),
                }
            },
            Event::Char('v') => validation_view(term, &model, &baseline, &arr)?,
            Event::Char('i') => show_corpus_info(term, &corpus)?,
            Event::Char('a') => objective_view(term, &model, if isolated_changes { &previous_arr } else { &baseline }, &arr, &corpus, &w)?,
            Event::Char('?') => help_popup(
                term,
                &frame,
                "Editor controls",
                &[
                    "Click / drag\tSelect two keys, or drag one onto another, to swap them.".into(),
                    "hjkl / arrows\tMove between keys.".into(),
                    "Space / Enter\tSelect a key, then swap with the next selected key.".into(),
                    "d\tToggle isolated changes: compare stats with the layout before your latest edit.".into(),
                    "m\tSwitch Simple / Detailed stats. Simple shows FSB/FSS/HSB/HSS; scoring stays unchanged.".into(),
                    "u\tUndo the last edit.".into(),
                    "U\tRedo the last undone edit.".into(),
                    "r\tReset to the layout you opened.".into(),
                    "s\tSave this layout after confirmation.".into(),
                    "S\tSave a new copy.".into(),
                    "c\tChoose a corpus; keep your edits.".into(),
                    "z\tInclude or exclude patterns that contain Space, such as e + Space or Space + t.".into(),
                    "a\tShow how weights contribute to the score.".into(),
                    "v\tCheck the layout for problems.".into(),
                    "i\tShow corpus information.".into(),
                    ".\tSwitch between two and four decimal places.".into(),
                    "Esc\tCancel key selection.".into(),
                    "q\tReturn to the previous screen.".into(),
                    String::new(),
                    "Click a stat to see the key sequences behind it.".into(),
                    "Green means better; red means worse. By default, compare with the loaded or saved layout. Isolated changes compare with the latest edit, including undo or reset.".into(),
                    "Blue keys have moved since loading or saving. Saving does not reset the latest-edit comparison.".into(),
                ]
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
                let changed = arr[a] != arr[b];
                if changed {
                    previous_arr = arr.clone();
                    previous_metrics = current_metrics.clone();
                }
                arr.swap(a, b);
                if changed {
                    current_raw = full_raw(&arr, &corpus, &model.geometry);
                    current_metrics = metrics(&current_raw, &corpus);
                }
                status.clear();
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
        _ if m >= N_METRICS => weight_label(m),
        _ => METRIC_NAMES[m],
    }
}

/// Day-to-day search settings shown in the optimizer's settings tab, as
/// `(id, label, value)`. IDs are stable `Action::Setting` values and do not
/// depend on which rows are shown. Workers, restarts, seed, passes, end
/// temperature, 3-key share, candidates, min letters and method remain in
/// `akler.conf`; measurements showed they rarely matter.
fn settings_labels(s: &SearchSettings) -> Vec<(usize, String, String)> {
    let lim = |v: Option<f64>| v.map(config_number).unwrap_or("none".into());
    vec![
        (
            16,
            "Scoring".into(),
            s.mode.clone(),
        ),
        (14, "Design".into(), s.design.clone()),
        (2, "Steps".into(), s.anneal_steps.to_string()),
        // `magic_anneal_steps`: the magic and action optimizers' steps.
        (21, "Magic steps".into(), s.magic_anneal_steps.to_string()),
        (3, "Seconds".into(), fmt2(s.seconds)),
        (10, "Temp start".into(), temperature_label(s)),
        (17, "Preset".into(), s.preset.clone()),
        (4, "SFB Δ max".into(), lim(s.sfb_limit)),
        (5, "SFS Δ max".into(), lim(s.sfs_limit)),
        (6, "Travel Δ max".into(), lim(s.travel_limit)),
        (7, "SF travel Δ max".into(), lim(s.sftravel_limit)),
    ]
}

pub(crate) fn cycle_weight_system(s: &mut SearchSettings) {
    s.mode = if s.mode == "aklgg" { "normal" } else { "aklgg" }.into();
    s.preset = "custom".into();
}

fn edit_setting(term: &mut Terminal, s: &mut SearchSettings, i: usize) -> AppResult<()> {
    let mut next = s.clone();
    if i == 16 {
        cycle_weight_system(s);
        validate_search_settings(s)?;
        return Ok(());
    }
    let (_, name, value) = settings_labels(s)
        .into_iter()
        .find(|(id, _, _)| *id == i)
        .ok_or("invalid setting")?;
    let initial = match i {
        4 => s.sfb_limit,
        5 => s.sfs_limit,
        6 => s.travel_limit,
        7 => s.sftravel_limit,
        _ => None,
    };
    let initial = if (4..=7).contains(&i) {
        initial.map(|n| n.to_string()).unwrap_or("none".into())
    } else {
        value
    };
    if let Some(v) = input_box(term, &name, "", &initial)? {
        match i {
            2 => next.anneal_steps = bounded_usize(&v, 10000000)?,
            21 => next.magic_anneal_steps = bounded_usize(&v, 10000000)?,
            3 => next.seconds = finite_nonnegative(&v)?,
            4 => next.sfb_limit = optional_limit(&v)?,
            5 => next.sfs_limit = optional_limit(&v)?,
            6 => next.travel_limit = optional_limit(&v)?,
            7 => next.sftravel_limit = optional_limit(&v)?,
            10 => next.temperature = finite_nonnegative(&v)?,
            _ => {}
        }
        if (4..=7).contains(&i) {
            next.preset = "custom".into();
        }
        validate_search_settings(&next)?;
        *s = next;
    }
    Ok(())
}

fn choose_design(term: &mut Terminal, s: &mut SearchSettings) -> AppResult<()> {
    if let Some(i) = menu(
        term,
        "",
        &["Refine".into(), "Random".into(), "Evolve".into()],
    )? {
        s.design = ["refine", "random", "evolve"][i].into();
    }
    Ok(())
}

fn choose_preset(term: &mut Terminal, s: &mut SearchSettings) -> AppResult<()> {
    let names = ["Balanced", "Strict", "Low travel", "Explore", "Custom"];
    if let Some(i) = menu(
        term,
        "",
        &names.iter().map(|x| x.to_string()).collect::<Vec<_>>(),
    )? {
        apply_preset(
            s,
            ["balanced", "strict", "low-travel", "explore", "custom"][i],
        );
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
    selected: Option<usize>,
) -> Canvas {
    let mut c = Canvas::optimizer(width, 96);
    header(
        &mut c,
        "Search settings",
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
        selected,
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
    y = typing_policy_row(&mut c, y, w);
    y = combo_policy_row(&mut c, y, w) + 1;
    let labels = settings_labels(s);
    let cols = if c.w >= 80 { 4 } else { 2 };
    let width = c.w / cols;
    for (cell, (i, name, value)) in labels.iter().enumerate() {
        let x = cell % cols * width;
        let yy = y + cell / cols * 2;
        c.text(x, yy, name, MUTED);
        c.text(x, yy + 1, &short(value, width - 1), CYAN);
        c.hit(
            Rect {
                x,
                y: yy,
                w: width - 1,
                h: 2,
            },
            Action::Setting(*i),
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

fn optimizer_mana_weights_frame(
    width: usize,
    model: &Model,
    source: &Source,
    arr: &[usize],
    locks: &[bool],
    weights: &Weights,
    status: &str,
    selected: Option<usize>,
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
        selected,
        &[],
    ) + 1;
    y = physical_weight_rows(&mut c, y, &weights.2) + 1;
    c.text(0, y, &short(status, c.w), CYAN);
    c.h = y + 3;
    c
}

const OPTIMIZER_TABS: [&str; 4] =
    [" [1] Overview ", " [2] Settings ", " [3] Weights ", " [4] Score "];

pub(crate) fn optimizer_tabs(canvas: &mut Canvas, active: usize, corpus: &str, layout: &str, mode: &str) {
    workspace_tabs_at(canvas, active, &OPTIMIZER_TABS);
    for x in 0..canvas.w {
        canvas.put(x, 1, ' ', MUTED);
    }
    let scoring = format!("Scoring: {mode} [t]");
    canvas.text(0, 1, &short(&format!("{scoring} · {corpus} · {layout}"), canvas.w), CYAN);
    canvas.hit(Rect { x: 0, y: 1, w: scoring.chars().count().min(canvas.w), h: 1 }, Action::Command('t'));
}

/// Open the action optimizer on this arrangement, with a new magic key when
/// `magic` is set and without one otherwise (combo fitting, `[combos]`).
fn optimizer_actions(
    term: &mut Terminal,
    model: &Model,
    source: &Source,
    arr: &[usize],
    locks: &[bool],
    weights: &Weights,
    settings: &SearchSettings,
    magic: bool,
    start: bool,
) -> AppResult<bool> {
    let layout = board_as_action_layout(&model.board, &model.symbols(arr))?;
    let action_locks = layout
        .slots
        .iter()
        .map(|slot| {
            model
                .board
                .keys
                .iter()
                .position(|key| key.row == slot.row && key.col == slot.col)
                .map(|index| locks[index])
                .unwrap_or(true)
        })
        .collect();
    let path = source.path.clone();
    let mut limits = load_app_config()?.ngrams;
    limits.include_spacegrams = source.include_spacegrams;
    let Some(corpus) = action_ui::ngram_job(
        term,
        move |stop, progress| {
            let raw = fs::read_to_string(&path).map_err(|error| error.to_string())?;
            action_ngrams::NgramCorpus::from_text_progress_with_limits(
                &raw, &path, limits, stop, progress,
            )
        },
    )?
    else {
        return Ok(false);
    };
    action_ui::action_optimizer_from_plain(
        term,
        layout,
        corpus,
        weights.clone(),
        settings.clone(),
        action_locks,
        magic,
        start,
    )
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
    // Optimizer panels are local to this setup session. Do not inherit the
    // weights panel left open by another optimizer or layout.
    term.optimizer_weights = false;
    term.optimizer_score = false;
    let mut scroll = 0;
    let mut status = String::new();
    let mut show_options = false;
    let mut drag = None;
    let mut drag_hover = None;
    let mut preview_model = model.clone();
    preview_model.geometry = Geometry::with_alts(model.board.keys.clone(), w.rolls());
    let corpus = preview_model.corpus(source)?;
    let mut preview_arr = arr.clone();
    let mut preview_mana = s.mode.as_str() == "aklgg";
    let mut baseline_raw = full_raw_with_mana2(
        &model.original,
        &corpus,
        &preview_model.geometry,
        preview_mana,
    );
    let mut baseline_metrics = metrics(&baseline_raw, &corpus);
    let mut current_raw = full_raw_with_mana2(arr, &corpus, &preview_model.geometry, preview_mana);
    let mut current_metrics = metrics(&current_raw, &corpus);
    loop {
        let mana = s.mode.as_str() == "aklgg";
        if preview_model.geometry.rolls != w.rolls() || preview_mana != mana {
            preview_model.geometry = Geometry::with_alts(model.board.keys.clone(), w.rolls());
            preview_mana = mana;
            baseline_raw = full_raw_with_mana2(
                &model.original,
                &corpus,
                &preview_model.geometry,
                preview_mana,
            );
            baseline_metrics = metrics(&baseline_raw, &corpus);
            current_raw = full_raw_with_mana2(arr, &corpus, &preview_model.geometry, preview_mana);
            current_metrics = metrics(&current_raw, &corpus);
            preview_arr = arr.clone();
        } else if preview_arr != *arr {
            current_raw = full_raw_with_mana2(arr, &corpus, &preview_model.geometry, preview_mana);
            current_metrics = metrics(&current_raw, &corpus);
            preview_arr = arr.clone();
        }
        let mut c = if show_options {
            optimizer_setup_frame(
                term.width(),
                model,
                source,
                arr,
                locks,
                w,
                s,
                &status,
                drag_hover,
            )
        } else if term.optimizer_score {
            let rows = if mana {
                aklgg_score_rows(
                    &mana_stats_for_corpus(&baseline_raw, &corpus),
                    &mana_stats_for_corpus(&current_raw, &corpus),
                    w,
                )
            } else {
                metric_score_rows(&baseline_metrics, &current_metrics, w)
            };
            score_frame(term.width(), rows, term.decimals())
        } else if term.optimizer_weights && mana {
            optimizer_mana_weights_frame(
                term.width(),
                model,
                source,
                arr,
                locks,
                w,
                &status,
                drag_hover,
            )
        } else {
            dashboard_from_raw(
                term,
                "Optimizer",
                "",
                &preview_model,
                arr,
                &model.original,
                &corpus,
                w,
                &baseline_raw,
                &current_raw,
                &baseline_metrics,
                &current_metrics,
                Some(locks),
                drag_hover,
                &status,
                None,
                false,
                Some(&s.mode),
            )
        };
        let active_tab = if show_options {
            1
        } else if term.optimizer_score {
            3
        } else if term.optimizer_weights {
            2
        } else {
            0
        };
        optimizer_tabs(&mut c, active_tab, &source.name, &model.board.name, &s.mode);
        let footer = "Space run · m magic · s save · ? help · q back";
        if show_options {
            c.h += 1;
        }
        if !show_options {
            c.text(
                0,
                c.h - 3,
                &short(
                    &format!(
                        "Locked {} · Free {}   {}",
                        locks.iter().filter(|&&v| v).count(),
                        locks.iter().filter(|&&v| !v).count(),
                        status
                    ),
                    c.w,
                ),
                CYAN,
            );
        }
        c.text(0, c.h - 2, &short(&footer, c.w), MUTED);
        scroll = scroll.min(c.h.saturating_sub(term.size.1));
        term.present(&c, scroll)?;
        let mut e = term.event()?;
        if scroll_event(&e, &mut scroll, c.h, term.size.1) {
            continue;
        }
        match e.clone() {
            Event::Escape if drag.is_some() => {
                drag = None;
                drag_hover = None;
                continue;
            }
            Event::Mouse {
                x,
                y,
                button: 0,
                release: false,
                motion: false,
            } => {
                if let Some(Action::Key(i)) = term.hit(&c, x, y, scroll) {
                    drag = Some(i);
                    drag_hover = Some(i);
                    continue;
                }
            }
            Event::Mouse {
                x,
                y,
                button: 0,
                release: false,
                motion: true,
            } if drag.is_some() => {
                if let Some(Action::Key(i)) = term.hit(&c, x, y, scroll) {
                    drag_hover = Some(i);
                }
                continue;
            }
            Event::Mouse {
                x,
                y,
                button: 0,
                release: true,
                ..
            } if drag.is_some() => {
                let from = drag.take().unwrap();
                let target = match term.hit(&c, x, y, scroll) {
                    Some(Action::Key(i)) => Some(i),
                    _ => None,
                };
                drag_hover = None;
                if let Some(to) = target {
                    if from == to {
                        locks[to] = !locks[to];
                    } else {
                        arr.swap(from, to);
                        status = "Swapped bindings".into();
                    }
                }
                continue;
            }
            _ => {}
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
                Action::ManaWeight(i) => {
                    match edit_physical_weight(term, w, i) {
                        Ok(true) => {
                            s.preset = "custom".into();
                            status = format!("Updated {} {}", s.mode, mana2_metrics::STAT_IDS[i]);
                        }
                        Ok(false) => {}
                        Err(err) => status = err.to_string(),
                    }
                    continue;
                }
                Action::Metric(i) => {
                    contributor_view(term, i, &preview_model, &model.original, arr, &corpus, true)?;
                    continue;
                }
                Action::Setting(i) => {
                    let res = match i {
                        14 => choose_design(term, s),
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
            Event::Char(tab @ '1'..='4') => {
                show_options = tab == '2';
                term.optimizer_weights = tab == '3';
                term.optimizer_score = tab == '4';
                scroll = 0;
                continue;
            }
            _ => {}
        }
        match e {
            Event::Char(' ') => {
                term.optimizer_weights = false;
                term.optimizer_score = false;
                // With combo or swap fitting on, offer the action run that
                // fits them.
                let config = load_app_config()?;
                let combos = config.combos.active_max(&config.weights);
                let swaps = config.adaptive.max_swaps;
                if combos == 0 && swaps == 0 {
                    return Ok(true);
                }
                let extras: Vec<String> = [(combos, "combos"), (swaps, "adaptive swaps")]
                    .into_iter()
                    .filter(|(count, _)| *count > 0)
                    .map(|(count, name)| format!("up to {count} {name}"))
                    .collect();
                let choices = [
                    format!("Run with {}", extras.join(" and ")),
                    "Run without them".to_string(),
                ];
                match menu(term, "Optimize", &choices)? {
                    Some(0) => match optimizer_actions(term, model, source, arr, locks, w, s, false, true) {
                        Ok(true) => return Ok(false),
                        Ok(false) => {}
                        Err(error) => status = error.to_string(),
                    },
                    Some(_) => return Ok(true),
                    None => {}
                }
            }
            Event::Char('m') | Event::Char('M') | Event::Char('c') => {
                let magic = matches!(e, Event::Char('m') | Event::Char('M'));
                match optimizer_actions(term, model, source, arr, locks, w, s, magic, false) {
                    Ok(true) => return Ok(false),
                    Ok(false) => {},
                    Err(error) => status = error.to_string(),
                }
            },
            Event::Char('e') => {
                show_options = !show_options;
                term.optimizer_weights = false;
                term.optimizer_score = false;
                scroll = 0;
            },
            Event::Char('w') => {
                let open = !term.optimizer_weights;
                term.optimizer_weights = open;
                term.optimizer_score = false;
                show_options = false;
                scroll = 0;
            },
            Event::Enter if term.optimizer_weights => {
                let result = if s.mode.as_str() == "aklgg" {
                    choose_physical_weight(term, w).map(|_| ())
                } else {
                    choose_optimizer_weight(term, w)
                };
                if let Err(error) = result {
                    status = error.to_string();
                }
            },
            Event::Char('.') => term.precise = !term.precise,
            Event::Escape | Event::Char('q') if show_options => {
                show_options = false;
                term.optimizer_weights = false;
                scroll = 0;
            },
            Event::Char('v') => {
                term.toggle_stats_view();
                status = format!("{} stats display; scoring unchanged", term.stats_view_name());
                scroll = 0;
            },
            Event::Escape | Event::Char('q') if term.optimizer_weights => { term.optimizer_weights = false; scroll = 0; },
            Event::Escape | Event::Char('q') if term.optimizer_score => { term.optimizer_score = false; scroll = 0; },
            Event::Escape | Event::Quit | Event::Char('q') => return Ok(false),
            Event::Char('H')=>*locks = home_locks(&model.board),
            Event::Char('U')=>locks.fill(false),
            Event::Char('L') => locks.fill(true),
            Event::Char('o')=>*arr = model.original.clone(),
            Event::Char('n') => s.seed = new_seed(),
            Event::Char('g') => choose_design(term, s)?,
            Event::Char('t') => {
                cycle_weight_system(s);
                status = format!("{} weights", s.mode);
                scroll = 0;
            },
            Event::Char('p') => choose_preset(term, s)?,
            Event::Char('x') => edit_mix(term, s)?,
            Event::Char('f') => {
                w.3 = !w.3;
                status = format!(
                    "Alternate fingers {}",
                    if w.3 { "enabled" } else { "disabled" }
                );
            },
            Event::Char('F') => {
                w.4 = !w.4;
                status = format!("Combos {}; s saves configuration", if w.4 { "enabled" } else { "disabled" });
            },
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
                &[
                    "Click a key\tLock or unlock its physical slot.".into(),
                    "Drag a key\tSwap its binding with another key.".into(),
                    "Click a stat\tShow its contributing patterns.".into(),
                    "Space\tStart optimization; with combos ([combos] fatfingering) or adaptive swaps ([adaptive] max) on, choose that run or an ordinary one.".into(),
                    "m\tInject a magic key and open the action optimizer with this arrangement, locks, and settings.".into(),
                    "c\tOpen the action optimizer without a magic key (fits combos and adaptive swaps when on).".into(),
                    "1 / 2 / 3\tOverview / Settings / Weights.".into(),
                    "e / w\tOpen Settings / Weights; press again for Overview.".into(),
                    "Click weight\tEdit that weight.".into(),
                    "Enter\tChoose a weight when the weights table is open.".into(),
                    "v\tSwitch Simple / Detailed stats; scoring stays unchanged.".into(),
                    "t\tSwitch Normal / AKL.GG scoring weights.".into(),
                    "AKL.GG\tBigrams, skipgrams, and trigrams use their own retained populations; speed columns are numeric costs.".into(),
                    "s\tSave weights and search settings to configuration.".into(),
                    ".\tSwitch between two and four decimal places.".into(),
                    "r\tReload configuration.".into(),
                    "d\tRestore default weights and search settings.".into(),
                    "o\tReset the layout to its original arrangement.".into(),
                    "H\tLock home keys and thumbs.".into(),
                    "U\tUnlock all keys.".into(),
                    "L\tLock all keys.".into(),
                    "g\tChoose Refine, Random, or Evolve.".into(),
                    "p\tChoose a limit preset.".into(),
                    "n\tChoose a new random seed.".into(),
                    "x\tEdit the training corpus mixture.".into(),
                    "q\tClose settings, or return to the previous screen.".into(),
                    "Limits allow increases from the original layout; travel uses u/100, SFB/SFS use percentage points.".into(),
                ]
            )?,
            _ => {
            }
        }
    }
}

fn load_training_sources(
    term: &mut Terminal,
    selected: &Source,
    settings: &SearchSettings,
) -> AppResult<Vec<Source>> {
    let mut out = vec![selected.clone()];
    for path in corpus_paths()? {
        let name = corpus_name(&path).to_ascii_lowercase();
        if !name.eq_ignore_ascii_case(&selected.name)
            && settings.mix.get(&name).copied().unwrap_or(0.0) > 0.0
        {
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
    let mana = p.settings.uses_mana2_stats();
    let r0 = full_raw_with_mana2(&p.model.original, co, &p.model.geometry, mana);
    let r1 = full_raw_with_mana2(arr, co, &p.model.geometry, mana);
    let a = metrics(&r0, co);
    let b = metrics(&r1, co);
    let mut cv = Canvas::new(term.width(), 96);
    header(
        &mut cv,
        title,
        &co.name,
        &p.model.board.name,
        if title == "Optimizer result" {
            "? help"
        } else {
            controls
        },
    );
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
    if p.settings.mode == "aklgg" {
        y = aklgg_stat_table(
            &mut cv,
            y,
            &mana_stats_for_corpus(&r0, co),
            &mana_stats_for_corpus(&r1, co),
            term.decimals(),
        );
    } else {
        y = grouped_metric_cards(
            &mut cv,
            y,
            &a,
            &b,
            &r1,
            co,
            term.decimals(),
            term.simple_stats,
        );
        y = finger_table(&mut cv, y + 1, &a, &b, term.decimals());
    }
    y = score_panel(
        &mut cv,
        y + 1,
        &p.breakdown(&r0, co),
        &p.breakdown(&r1, co),
        mix,
        term.decimals(),
    );
    cv.text(
        0,
        y,
        if p.settings.mode == "aklgg" { "Weights: AKL.GG" } else { "Weights: Normal" },
        MUTED,
    );
    y += 1;
    cv.text(0, y, &short(status, cv.w), CYAN);
    cv.text(
        0,
        y + 1,
        &format!("m stats: {} · ? help", term.stats_view_name()),
        MUTED,
    );
    cv.h = y + 3;
    cv
}

fn search_live(
    term: &mut Terminal,
    p: &Problem,
    arr: &mut Vec<usize>,
    locks: &[bool],
) -> AppResult<Option<Snapshot>> {
    let problem = p.clone();
    let start = arr.to_vec();
    let frozen_locks = locks.to_vec();
    let ctrl = Arc::new(Control::new());
    let worker_ctrl = ctrl.clone();
    let (tx, rx) = mpsc::sync_channel(2);
    let (done_tx, done_rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let mut callback = |s| {
            let _ = tx.try_send(s);
        };
        let result =
            run_search_parallel(&problem, &start, &frozen_locks, &worker_ctrl, &mut callback)
                .map_err(|e| e.to_string());
        let _ = done_tx.send(result);
    });
    let state = State::new(arr.to_vec(), p);
    let mut snap = Snapshot {
        best: Candidate {
            arr: arr.to_vec(),
            score: state.score,
        },
        progress: Progress {
            phase: "starting".into(),
            seed: p.settings.seed,
            ..Progress::default()
        },
        archive: Vec::new(),
        has_best: false,
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
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err("search worker exited without a result".into())
                }
                Err(_) => {}
            }
            let pr = &snap.progress;
            let status = format!(
                "{}  {}/{}  {} trials  {:.1}s · {} workers",
                if ctrl.pause.load(Ordering::Relaxed) {
                    "paused"
                } else {
                    &pr.phase
                },
                pr.restart,
                p.settings.restarts,
                pr.evaluations,
                pr.elapsed,
                search_worker_count(&p.settings)
            );
            let c = optimizer_dashboard(
                term,
                p,
                &snap.best.arr,
                locks,
                "Optimizing",
                "p pause | q setup (keep changes)",
                &status,
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
                }
                Event::Char('m') => {
                    term.toggle_stats_view();
                    scroll = 0;
                }
                Event::Char('.') => term.precise = !term.precise,
                Event::Escape | Event::Char('q') | Event::Quit => {
                    leave = true;
                    return Ok(None);
                }
                _ => {}
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

fn save_result(
    p: &Problem,
    arr: &[usize],
    start: &[usize],
    locks: &[bool],
    progress: &Progress,
) -> AppResult<PathBuf> {
    let symbols = p.model.symbols(arr);
    let b = p.model.board.clone();
    let path = crate::layout_export::save_optimized(&board_as_action_layout(&b, &symbols)?)?;
    let mut report = format!(
        "model = {MODEL_VERSION}\nseed = 0x{:x}\ntrials = {}\nseconds = {:.6}\nscore = {:.17}\n\n[weights]\n{}\n[rolls]\n{}\n[aklgg]\n{}\n[search]\n{}\n[original]\n{}\n[start]\n{}\n[result]\n{}\n[locks]\n{:?}\n",
        progress.seed,
        progress.evaluations,
        progress.elapsed,
        State::new(arr.to_vec(), p).score,
        weights_text(&p.weights),
        rolls_config_text(p.weights.rolls()),
        p.weights.2.config_text(),
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
            for v in m.v {
                out.push_str(&format!(",{v:.9}"));
            }
            out.push('\n');
        }
    }
    for n in 1..10000 {
        let path = paths
            .first()
            .and_then(|path| path.parent())
            .unwrap_or(Path::new(LAYOUT_DIR))
            .join(format!("{}-batch-{n:03}.csv", p.model.board.name));
        match OpenOptions::new().create_new(true).write(true).open(&path) {
            Ok(mut f) => {
                f.write_all(out.as_bytes())?;
                crate::sync_file(&f)?;
                return Ok(path);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
    }
    Err("no unused batch filename".into())
}

fn objective_audit(term: &mut Terminal, p: &Problem, arr: &[usize]) -> AppResult<()> {
    if matches!(p.settings.mode.as_str(), "aklgg") {
        mana_objective_view(term, p, arr)
    } else {
        problem_objective_view(term, p, arr)
    }
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
    timing.mark("Weights, settings and locks");
    drop(timing);
    loop {
        if !optimizer_setup(
            term,
            &model,
            source,
            &mut arr,
            &mut locks,
            &mut weights,
            &mut settings,
        )? {
            return Ok(());
        }
        if model.geometry.rolls != weights.rolls() {
            model.geometry = Geometry::with_alts(model.board.keys.clone(), weights.rolls());
        }
        let sources = load_training_sources(term, source, &settings)?;
        let mut problem = Problem::new(
            model.clone(),
            &sources,
            0,
            weights.clone(),
            settings.clone(),
        )?;
        'runs: loop {
            let start = arr.clone();
            let result = match search_live(term, &problem, &mut arr, &locks)? {
                Some(r) => r,
                None => break 'runs,
            };
            if !result.has_best || result.archive.is_empty() {
                break 'runs;
            }
            let mut choice = 0;
            let mut scroll = 0;
            let mut status = String::new();
            loop {
                let cur = &result.archive[choice];
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
                            true,
                        )?,
                        _ => {}
                    }
                }
                match e {
                    Event::Escape | Event::Char('q') | Event::Char('r') => break 'runs,
                    Event::Quit => return Ok(()),
                    Event::Char('?') => help_popup(
                        term,
                        &frame,
                        "Optimizer result",
                        &[
                            "q or r: return to setup with this candidate; Space: refine again."
                                .into(),
                            "[ / ]: previous / next candidate; b: compare candidates.".into(),
                            "s: save a copy; S: save all candidates.".into(),
                            "m\tSwitch Simple / Detailed stats; scoring stays unchanged.".into(),
                            "a: score contributions; i: corpus notes; v: validation; .: precision."
                                .into(),
                        ],
                    )?,
                    Event::Char('.') => term.precise = !term.precise,
                    Event::Char('m') => {
                        term.toggle_stats_view();
                        scroll = 0;
                    }
                    Event::Char(' ') => {
                        problem.settings.design = "refine".into();
                        problem.settings.seed = problem.settings.seed.wrapping_add(1);
                        continue 'runs;
                    }
                    Event::Char('[') => {
                        choice = (choice + result.archive.len() - 1) % result.archive.len()
                    }
                    Event::Char(']') => choice = (choice + 1) % result.archive.len(),
                    Event::Char('b') => {
                        choice = candidate_view(term, &problem, &result.archive, choice)?
                    }
                    Event::Char('s') => {
                        status = match save_result(&problem, &arr, &start, &locks, &result.progress)
                        {
                            Ok(p) => saved_layout_message(&p),
                            Err(e) => e.to_string(),
                        }
                    }
                    Event::Char('S') => {
                        status = match save_batch(&problem, &result, &start, &locks) {
                            Ok(p) => format!(
                                "saved {} candidates (.jsonc); {}",
                                result.archive.len(),
                                p.display()
                            ),
                            Err(e) => e.to_string(),
                        }
                    }
                    Event::Char('a') => objective_audit(term, &problem, &arr)?,
                    Event::Char('v') => validation_view(term, &model, &model.original, &arr)?,
                    Event::Char('i') => show_corpus_info(term, &problem.corpora[0])?,
                    _ => {}
                }
            }
        }
    }
}

fn rebuild_source_tui(
    term: &mut Terminal,
    raw: &Path,
    cfg: CorpusConfig,
    hash: u64,
) -> AppResult<PathBuf> {
    let raw = raw.to_owned();
    let total = fs::metadata(&raw)?.len();
    let progress = Arc::new(AtomicU64::new(0));
    let worker_progress = progress.clone();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = cancel.clone();
    let (tx, rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let result = rebuild_corpus_control(
            &raw,
            &cfg,
            hash,
            &mut |n, _| {
                worker_progress.store(n, Ordering::Relaxed);
            },
            Some(&worker_cancel),
        )
        .map_err(|e| e.to_string());
        let _ = tx.send(result);
    });
    let result = (|| -> AppResult<PathBuf> {
        loop {
            match rx.try_recv() {
                Ok(v) => return v.map_err(|e| e.into()),
                Err(mpsc::TryRecvError::Disconnected) => return Err("corpus worker exited".into()),
                Err(_) => {}
            }
            let mut c = Canvas::new(term.width(), 4);
            let n = progress.load(Ordering::Relaxed);
            progress_bar(&mut c, 1, pct(n as f64, total as f64));
            term.present(&c, 0)?;
            if matches!(
                term.event()?,
                Event::Escape | Event::Quit | Event::Char('q')
            ) {
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
    fn normal_optimizer_exposes_only_akler_weights() {
        let source = Source::from_text(
            r#"{"letters":{"q":4,"w":2},"bigrams":{"qw":2},"trigrams":{"qwe":1}}"#,
            Path::new("inline.json"),
        )
        .unwrap();
        let model = Model::new(board("thumbs: space\n"));
        let canvas = optimizer_setup_frame(
            108,
            &model,
            &source,
            &model.original,
            &default_locks(&model.board),
            &Weights::default(),
            &SearchSettings::default(),
            "ready",
            None,
        );
        assert!(!canvas
            .hits
            .iter()
            .any(|(_, a)| matches!(a, Action::Weight(SFB))));
        let mut weights = Canvas::new(108, 96);
        let corpus = model.corpus(&source).unwrap();
        let raw = full_raw(&model.original, &corpus, &model.geometry);
        let metrics = metrics(&raw, &corpus);
        optimizer_metric_cards(
            &mut weights,
            0,
            &metrics,
            &metrics,
            &raw,
            &corpus,
            &Weights::default(),
            DISPLAY_DECIMALS,
            false,
            true,
        );
        for simple in [false, true] {
            for width in [60, 108, 180] {
                let mut w = Weights::default();
                w.0[CNFJB] = 2222.0;
                let mut stats = Canvas::new(width, 96);
                let mut panel = Canvas::new(width, 96);
                optimizer_metric_cards(
                    &mut stats,
                    3,
                    &metrics,
                    &metrics,
                    &raw,
                    &corpus,
                    &w,
                    DISPLAY_DECIMALS,
                    simple,
                    false,
                );
                optimizer_metric_cards(
                    &mut panel,
                    3,
                    &metrics,
                    &metrics,
                    &raw,
                    &corpus,
                    &w,
                    DISPLAY_DECIMALS,
                    simple,
                    true,
                );
                for (rect, action) in &stats.hits {
                    if let Action::Metric(m) = action {
                        if let Some(id) = optimizer_metric_weight(*m) {
                            assert!(panel.hits.iter().any(
                                |(r, a)| matches!(a, Action::Weight(i) if *i == id)
                                    && r.x == rect.x
                                    && r.y == rect.y
                                    && r.w == rect.w
                            ));
                        }
                    }
                }
            }
        }
        assert!(weights
            .hits
            .iter()
            .any(|(_, action)| matches!(action, Action::Weight(SFB))));
        for i in (0..N_WEIGHTS).filter(|&i| !aggregate(i)) {
            assert!(
                weights
                    .hits
                    .iter()
                    .any(|(_, action)| matches!(action, Action::Weight(id) if *id == i)),
                "missing editable weight {}",
                WEIGHT_NAMES[i]
            );
        }
        assert!(weights.cells.iter().any(|cell| cell.ch == '├'));
        assert!(canvas
            .hits
            .iter()
            .any(|(_, a)| matches!(a, Action::Setting(16))));
        for name in ["balanced", "strict", "low-travel", "explore"] {
            let mut settings = SearchSettings::default();
            apply_preset(&mut settings, name);
            assert_eq!(settings.mode, "normal");
        }

        let mut aklgg_weights = Canvas::new(108, 96);
        physical_weight_rows(&mut aklgg_weights, 0, &Weights::default().2);
        let editable = aklgg_weights
            .hits
            .iter()
            .filter_map(|(_, action)| match action {
                Action::ManaWeight(index) => Some(mana2_metrics::STAT_IDS[*index]),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(editable.contains(&"weighted_speed"));
        assert!(editable.contains(&"fspeed-LP"));
        assert!(editable.contains(&"offpinky"));

        let mut settings = SearchSettings::default();
        cycle_weight_system(&mut settings);
        assert_eq!(settings.mode, "aklgg");
        cycle_weight_system(&mut settings);
        assert_eq!(settings.mode, "normal");

        let stats = mana_stats_for_corpus(&full_raw_with_mana2(
            &model.original,
            &corpus,
            &model.geometry,
            true,
        ), &corpus);
        let mut aklgg = Canvas::new(108, 48);
        aklgg_stat_table(&mut aklgg, 0, &stats, &stats, DISPLAY_DECIMALS);
        let rendered = aklgg
            .cells
            .chunks(aklgg.w)
            .map(|row| row.iter().map(|cell| cell.ch).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        for label in ["AKL.GG patterns", "SKIPGRAM", "NO THUMBS", "Fspeed", "Weighted"] {
            assert!(rendered.contains(label), "missing {label} from AKL.GG table");
        }
        let mut tabs = Canvas::new(108, 4);
        optimizer_tabs(&mut tabs, 0, "reddit", "packet", "aklgg");
        let tabs = tabs
            .cells
            .iter()
            .map(|cell| cell.ch)
            .collect::<String>();
        assert!(tabs.contains("Scoring: aklgg [t]"));
    }
}

/// A loading bar with its percentage, nothing else.
pub(crate) fn progress_bar(c: &mut Canvas, y: usize, percent: f64) {
    let width = c.w.saturating_sub(10).min(60);
    let filled = ((percent.clamp(0.0, 100.0) / 100.0) * width as f64).round() as usize;
    c.text(2, y, &"━".repeat(filled), CYAN);
    c.text(2 + filled, y, &"─".repeat(width - filled), BORDER);
    c.text(3 + width, y, &format!("{percent:.0}%"), FG);
}

/// Tabs as connected boxes on rows y..y+3 from column x, the active one
/// highlighted; a click on a box presses its number.
fn tab_boxes(c: &mut Canvas, x: usize, y: usize, active: usize, labels: &[String]) {
    if labels.is_empty() || x >= c.w {
        return;
    }
    // Shorten the labels evenly when the boxes do not fit.
    let room = c.w.saturating_sub(x + 1 + 3 * labels.len());
    let natural: usize = labels.iter().map(|label| label.chars().count()).sum();
    let widths: Vec<usize> = labels
        .iter()
        .map(|label| {
            let n = label.chars().count();
            if natural <= room { n } else { (room / labels.len()).max(1).min(n) }
        })
        .collect();
    let mut left = x;
    for (index, (label, width)) in labels.iter().zip(&widths).enumerate() {
        let inner = width + 2;
        let first = index == 0;
        let last = index + 1 == labels.len();
        let line = "─".repeat(inner);
        c.text(left, y, &format!("{}{line}{}", if first { "┌" } else { "┬" }, if last { "┐" } else { "" }), BORDER);
        c.text(left, y + 1, "│", BORDER);
        let selected = index == active;
        c.text(left + 2, y + 1, &short(label, *width), if selected { FG } else { MUTED });
        if selected {
            c.row_style(Rect { x: left + 1, y: y + 1, w: inner, h: 1 }, Some((58, 58, 58)), true);
        }
        if last {
            c.text(left + inner + 1, y + 1, "│", BORDER);
        }
        c.text(left, y + 2, &format!("{}{line}{}", if first { "└" } else { "┴" }, if last { "┘" } else { "" }), BORDER);
        if index < 9 {
            c.hit(
                Rect { x: left, y, w: inner + 2, h: 3 },
                Action::Command(char::from(b'1' + index as u8)),
            );
        }
        left += inner + 1;
    }
}

/// "[1] Rankings" as "1 Rankings".
fn tab_label(label: &str) -> String {
    label.trim().replace(['[', ']'], "")
}

fn workspace_tabs(canvas: &mut Canvas, active: usize, labels: &[&str]) {
    workspace_tabs_at(canvas, active, labels);
}

/// Shared tabs across workspaces: they are drawn as boxes in the top bar,
/// and row 0 of the canvas, which used to hold them, is not shown.
fn workspace_tabs_at(canvas: &mut Canvas, active: usize, labels: &[&str]) {
    canvas.tabs = Some((active, labels.iter().map(|label| tab_label(label)).collect()));
}

#[cfg(test)]
mod fuzzy_match_tests {
    use super::fuzzy_match;

    #[test]
    fn matches_characters_in_order_and_ignores_case() {
        assert_eq!(fuzzy_match("FNC", "finch").map(|m| m.1), Some(vec![0, 2, 3]));
        assert!(fuzzy_match("cnf", "finch").is_none());
        assert!(fuzzy_match("", "anything").is_some());
    }

    #[test]
    fn ranks_runs_and_word_starts_above_scattered_letters() {
        let score = |query, text| fuzzy_match(query, text).unwrap().0;
        assert!(score("gr", "graphite") > score("gr", "magnetar"));
        assert!(score("nt", "night") < score("nt", "ntx"));
        // A later contiguous run beats the leftmost scattered match.
        assert_eq!(fuzzy_match("ast", "a yeast").map(|m| m.1), Some(vec![4, 5, 6]));
    }
}
