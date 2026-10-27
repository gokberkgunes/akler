// Runs akler.wasm in a Wasmer sandbox and shows it in xterm.js. The program's
// data directory is copied into the sandbox at start; files akler writes under
// SAVED are kept in IndexedDB and restored on the next visit.
import { Wasmer } from "@wasmer/sdk";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";

const HOME = "/workspace/akler";
/** Paths, relative to HOME, whose changes are saved in this browser. */
const SAVED = ["akler.conf", "ranker-hidden.json", "layouts", "scissors", "optimized"];
const SAVE_EVERY_MS = 3000;

/** Command-line arguments from `?args=...`, e.g. `?args=bench layouts/night.jsonc corpus-reddit`. */
const commandArgs = (new URLSearchParams(location.search).get("args") ?? "").split(" ").filter(Boolean);

const status = document.getElementById("status");
const setStatus = (text) => (status.textContent = text);

// ---- Themes -------------------------------------------------------------

/** xterm's default colours 16–255 (6×6×6 cube, then greys). */
const hex = (r, g, b) => `#${[r, g, b].map((v) => v.toString(16).padStart(2, "0")).join("")}`;
const LEVELS = [0, 95, 135, 175, 215, 255];
const EXTENDED = Array.from({ length: 240 }, (_, n) => {
  if (n >= 216) return hex(...Array(3).fill(8 + 10 * (n - 216)));
  return hex(LEVELS[Math.floor(n / 36)], LEVELS[Math.floor(n / 6) % 6], LEVELS[n % 6]);
});
/** The 256-colour palette with some entries replaced, keyed by colour index. */
const extended = (overrides) => EXTENDED.map((color, n) => overrides[n + 16] ?? color);

/** Colour themes, the first being the default. akler draws its main text in
 *  the terminal palette, so these set its colours too. */
const THEMES = {
  dark: {
    label: "Dark",
    terminal: {
      background: "#000000",
      foreground: "#ffffff",
      black: "#000000",
      red: "#ff5f5f",
      green: "#5fd75f",
      yellow: "#ffd75f",
      blue: "#5fafff",
      magenta: "#d787ff",
      cyan: "#00d7d7",
      // Muted text and finger greys.
      white: "#ffffff",
      // Borders.
      brightBlack: "#8a8a8a",
      // Metric group labels.
      brightRed: "#ff8787",
      brightGreen: "#87ff87",
      brightYellow: "#ffff87",
      brightBlue: "#87d7ff",
      brightMagenta: "#ffafff",
      brightCyan: "#5fffff",
      brightWhite: "#ffffff",
      // Ranker rows (STRIPE_DARK, STRIPE_LIGHT and ROW_SELECTED in ui.rs):
      // a lighter and a darker stripe every four rows, as in a terminal.
      extendedAnsi: extended({ 233: "#121212", 235: "#1e1e1e", 236: "#242424" }),
    },
  },
  light: {
    label: "Light",
    terminal: {
      background: "#141414",
      foreground: "#ffffff",
      black: "#141414",
      red: "#f07178",
      green: "#a6d189",
      yellow: "#f0c674",
      blue: "#6cb6ff",
      magenta: "#d2a8ff",
      cyan: "#66d9e8",
      white: "#ffffff",
      brightBlack: "#7a7a7a",
      brightRed: "#ff9aa2",
      brightGreen: "#c0e8a0",
      brightYellow: "#ffd98e",
      brightBlue: "#9fd0ff",
      brightMagenta: "#e2c2ff",
      brightCyan: "#9eeaf2",
      brightWhite: "#ffffff",
      // Ranker rows: every other row a step lighter.
      extendedAnsi: extended({ 233: "#1c1c1c", 235: "#1c1c1c", 236: "#2e2e2e" }),
    },
  },
};
const THEME_KEY = "akler-theme";
let themeName = (() => {
  try {
    const saved = localStorage.getItem(THEME_KEY);
    return saved in THEMES ? saved : Object.keys(THEMES)[0];
  } catch {
    return Object.keys(THEMES)[0];
  }
})();
document.documentElement.dataset.theme = themeName;

// ---- Fonts --------------------------------------------------------------

/** Terminal fonts, the first being the default. Gohu is a bitmap font, sized
 *  in `pixels` so each of its pixels covers whole screen pixels. */
const FONTS = {
  serious: { label: "Serious Shanns", family: "Serious Shanns", size: 15 },
  gohu: { label: "Gohu", family: "GohuFont", pixels: 14 },
  comic: { label: "Comic Mono", family: "Comic Mono", size: 14 },
};
// Symbols a font lacks, such as ␣ in all three, come from these.
const FALLBACK = '"JetBrains Mono", "DejaVu Sans Mono", Menlo, Consolas, monospace';
const FONT_KEY = "akler-font";

const remembered = (() => {
  try {
    return localStorage.getItem(FONT_KEY);
  } catch {
    return null;
  }
})();
let fontName = remembered in FONTS ? remembered : Object.keys(FONTS)[0];

/** The CSS size: a bitmap font scales by whole screen pixels, 1× up to 125%
 *  display scaling and 2× from 150%. */
const fontSize = ({ size, pixels }) =>
  pixels ? (pixels * Math.max(1, Math.round(devicePixelRatio))) / devicePixelRatio : size;

// xterm.js measures its cells from the font, so it must be loaded first.
const loadFont = (font) =>
  Promise.all(
    ["", "bold "].map((weight) =>
      document.fonts.load(`${weight}${fontSize(font)}px "${font.family}"`),
    ),
  ).catch((error) => console.warn(error));
await loadFont(FONTS[fontName]);

const terminal = new Terminal({
  fontFamily: `"${FONTS[fontName].family}", ${FALLBACK}`,
  fontSize: fontSize(FONTS[fontName]),
  // A command's plain output ends lines with \n alone; the TUI positions its cursor.
  convertEol: commandArgs.length > 0,
  scrollback: commandArgs.length > 0 ? 5000 : 0,
  cursorBlink: false,
  allowProposedApi: true,
  theme: THEMES[themeName].terminal,
});
// Exposed for scripted checks of what the terminal shows.
globalThis.aklerTerminal = terminal;
const fit = new FitAddon();
terminal.loadAddon(fit);
terminal.open(document.getElementById("terminal"));
// WebGL draws box-drawing characters itself, without font gaps.
try {
  const webgl = new WebglAddon();
  webgl.onContextLoss(() => webgl.dispose());
  terminal.loadAddon(webgl);
} catch (error) {
  console.warn("WebGL renderer unavailable; using the DOM renderer", error);
}
fit.fit();
window.addEventListener("resize", () => fit.fit());
// Ctrl+L belongs to the browser (the address bar), not to akler.
terminal.attachCustomKeyEventHandler(
  (event) => !(event.ctrlKey && !event.altKey && !event.metaKey && event.key.toLowerCase() === "l"),
);

// The theme button shows the current theme: a moon for dark, a sun for light.
const ICONS = {
  dark: '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M20 14.5A8 8 0 0 1 9.5 4 8 8 0 1 0 20 14.5z"/></svg>',
  light:
    '<svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="4.5"/>' +
    '<path d="M12 1.5v3M12 19.5v3M1.5 12h3M19.5 12h3M4.6 4.6l2.1 2.1M17.3 17.3l2.1 2.1M4.6 19.4l2.1-2.1M17.3 6.7l2.1-2.1"/></svg>',
};
const themeButton = document.getElementById("theme");
const otherTheme = () => Object.keys(THEMES).find((name) => name !== themeName);
const showTheme = () => {
  themeButton.innerHTML = ICONS[themeName];
  themeButton.title = `${THEMES[themeName].label} theme; click for ${THEMES[otherTheme()].label.toLowerCase()}`;
  themeButton.setAttribute("aria-label", themeButton.title);
};
showTheme();
themeButton.addEventListener("click", () => {
  themeName = otherTheme();
  document.documentElement.dataset.theme = themeName;
  terminal.options.theme = THEMES[themeName].terminal;
  showTheme();
  terminal.focus();
  try {
    localStorage.setItem(THEME_KEY, themeName);
  } catch {}
});

const fontPicker = document.getElementById("font");
for (const [name, { label }] of Object.entries(FONTS)) fontPicker.add(new Option(label, name));
fontPicker.value = fontName;
async function applyFont() {
  const font = FONTS[fontName];
  await loadFont(font);
  terminal.options.fontFamily = `"${font.family}", ${FALLBACK}`;
  terminal.options.fontSize = fontSize(font);
  // The new cell size changes the grid; akler hears of it through onResize.
  fit.fit();
}
fontPicker.addEventListener("change", async () => {
  fontName = fontPicker.value;
  await applyFont();
  terminal.focus();
  try {
    localStorage.setItem(FONT_KEY, fontName);
  } catch {}
});
// Browser zoom and moving to another screen change the pixel ratio.
const watchPixelRatio = () =>
  matchMedia(`(resolution: ${devicePixelRatio}dppx)`).addEventListener(
    "change",
    () => {
      applyFont();
      watchPixelRatio();
    },
    { once: true },
  );
watchPixelRatio();

// ---- Browser storage: path (relative to HOME) -> bytes -----------------

const db = new Promise((resolve, reject) => {
  const request = indexedDB.open("akler", 1);
  request.onupgradeneeded = () => request.result.createObjectStore("files");
  request.onsuccess = () => resolve(request.result);
  request.onerror = () => reject(request.error);
});

async function store(mode, action) {
  const transaction = (await db).transaction("files", mode);
  const request = action(transaction.objectStore("files"));
  return new Promise((resolve, reject) => {
    transaction.oncomplete = () => resolve(request?.result);
    transaction.onerror = () => reject(transaction.error);
  });
}

async function savedFiles() {
  const files = new Map();
  await store("readonly", (files_) => {
    const cursor = files_.openCursor();
    cursor.onsuccess = () => {
      const entry = cursor.result;
      if (entry) {
        files.set(entry.key, new Uint8Array(entry.value));
        entry.continue();
      }
    };
  });
  return files;
}

const save = (path, bytes) => store("readwrite", (files) => files.put(bytes, path));
const forget = (path) => store("readwrite", (files) => files.delete(path));
const forgetAll = () => store("readwrite", (files) => files.clear());

// ---- Bundled data ------------------------------------------------------

async function bundledFiles() {
  const manifest = await (await fetch("data/manifest.json")).json();
  const total = manifest.reduce((sum, file) => sum + file.size, 0);
  let loaded = 0;
  const files = new Map();
  await Promise.all(
    manifest.map(async ({ path }) => {
      const response = await fetch(`data/${path}`);
      if (!response.ok) throw new Error(`data/${path}: ${response.status}`);
      const bytes = new Uint8Array(await response.arrayBuffer());
      files.set(path, bytes);
      loaded += bytes.length;
      setStatus(`Loading layouts and corpora… ${Math.round((100 * loaded) / total)}%`);
    }),
  );
  return files;
}

const sameBytes = (a, b) => a.length === b.length && a.every((byte, index) => byte === b[index]);

// ---- Sandbox -----------------------------------------------------------

let sandbox;
let pkg;
let bundled;
/** What each saved path held when last compared, to find changes. */
const known = new Map();

async function writeFile(path, bytes) {
  const full = `${HOME}/${path}`;
  await sandbox.fs.mkdir(full.slice(0, full.lastIndexOf("/")), { recursive: true });
  await sandbox.fs.writeFile(full, bytes);
}

async function listFiles(path) {
  const full = `${HOME}/${path}`;
  let stat;
  try {
    stat = await sandbox.fs.stat(full);
  } catch {
    return [];
  }
  if (stat.kind === "file") return [path];
  const paths = [];
  for (const entry of await sandbox.fs.readDir(full)) {
    paths.push(...(await listFiles(`${path}/${entry.name}`)));
  }
  return paths;
}

/** Keep files that differ from the bundled copy; forget those that match it. */
async function persist() {
  if (!sandbox) return;
  for (const root of SAVED) {
    for (const path of await listFiles(root)) {
      let bytes;
      try {
        bytes = await sandbox.fs.readFile(`${HOME}/${path}`);
      } catch {
        continue;
      }
      const previous = known.get(path);
      if (previous && sameBytes(previous, bytes)) continue;
      known.set(path, bytes);
      const original = bundled.get(path);
      if (original && sameBytes(original, bytes)) await forget(path);
      else await save(path, bytes);
    }
  }
}

async function boot() {
  if (!globalThis.crossOriginIsolated) {
    setStatus("This page needs cross-origin isolation (COOP/COEP headers) to run threads.");
    terminal.write(
      "\r\nakler needs a cross-origin-isolated page for threads.\r\n" +
        "Serve it with Cross-Origin-Opener-Policy: same-origin and\r\n" +
        "Cross-Origin-Embedder-Policy: require-corp (see web/README.md).\r\n",
    );
    return;
  }
  setStatus("Loading akler…");
  const [wasm, data, saved] = await Promise.all([
    fetch("akler.wasm").then((response) => response.arrayBuffer()),
    bundledFiles(),
    savedFiles(),
  ]);
  bundled = data;
  const wasmer = new Wasmer();
  pkg = await wasmer.packages.create({
    modules: { akler: new Uint8Array(wasm) },
    commands: { akler: { module: "akler" } },
  });
  sandbox = await wasmer.sandboxes.create({ packages: [pkg] });
  setStatus("Copying files…");
  const files = new Map([...bundled, ...saved]);
  for (const [path, bytes] of files) {
    await writeFile(path, bytes);
    known.set(path, bytes);
  }
  setInterval(() => persist().catch((error) => console.error(error)), SAVE_EVERY_MS);
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "hidden") persist();
  });
  await run();
}

// ---- Process and terminal ----------------------------------------------

let akler;
const resizeSequence = () => `\x1b[8;${terminal.rows};${terminal.cols}t`;

terminal.onData((data) => akler?.stdin?.write(data));
terminal.onBinary((data) =>
  akler?.stdin?.write(Uint8Array.from(data, (char) => char.charCodeAt(0))),
);
terminal.onResize(() => akler?.stdin?.write(resizeSequence()));

async function run() {
  const threads = navigator.hardwareConcurrency || 4;
  setStatus(`Running · ${threads} threads · saved in this browser`);
  terminal.reset();
  akler = await sandbox
    .command(pkg, commandArgs, {
      cwd: HOME,
      env: {
        AKLER_HOME: HOME,
        AKLER_THREADS: String(threads),
        COLUMNS: String(terminal.cols),
        LINES: String(terminal.rows),
        TERM: "xterm-256color",
      },
    })
    .spawn({ stdin: "pipe", stdout: "pipe", stderr: "pipe" });
  await akler.stdin.write(resizeSequence());
  const pump = async (stream, sink) => {
    for await (const chunk of stream) sink(chunk);
  };
  const decoder = new TextDecoder();
  void pump(akler.stdout, (chunk) => terminal.write(chunk));
  void pump(akler.stderr, (chunk) => console.log(decoder.decode(chunk)));
  let ending;
  try {
    ending = `akler exited (status ${(await akler.wait()).exitCode}).`;
  } catch (error) {
    console.error(error);
    ending = `akler stopped: ${String(error.message ?? error).split("\n")[0]}`;
  }
  akler = undefined;
  await persist();
  // A command's output stays; the menu's exit clears the screen.
  if (commandArgs.length === 0) terminal.reset();
  terminal.write(`\r\n${ending} Press any key to start it again.`);
  setStatus("Stopped");
  const restart = terminal.onData(() => {
    restart.dispose();
    void run();
  });
}

// ---- Toolbar -----------------------------------------------------------

const uploadInput = document.getElementById("upload-input");
document.getElementById("upload").addEventListener("click", () => uploadInput.click());
uploadInput.addEventListener("change", async () => {
  if (!sandbox) return;
  const names = [];
  for (const file of uploadInput.files) {
    const path = `layouts/${file.name.replace(/[/\\]/g, "_")}`;
    const bytes = new Uint8Array(await file.arrayBuffer());
    await writeFile(path, bytes);
    await save(path, bytes);
    known.set(path, bytes);
    names.push(file.name);
  }
  uploadInput.value = "";
  setStatus(`Added ${names.join(", ")} to layouts/; reopen the layout list to see them.`);
});

const dialog = document.getElementById("files-dialog");
document.getElementById("files").addEventListener("click", async () => {
  if (!sandbox) return;
  await persist();
  const list = document.getElementById("files-list");
  list.replaceChildren();
  const paths = [];
  for (const root of SAVED) paths.push(...(await listFiles(root)));
  for (const path of paths.sort()) {
    const bytes = await sandbox.fs.readFile(`${HOME}/${path}`);
    const item = document.createElement("li");
    const link = document.createElement("a");
    link.textContent = path;
    link.download = path.split("/").pop();
    link.href = URL.createObjectURL(new Blob([bytes]));
    const size = document.createElement("span");
    size.className = "size";
    size.textContent = `${(bytes.length / 1024).toFixed(1)} KB${bundled.has(path) && sameBytes(bundled.get(path), bytes) ? "" : " · changed"}`;
    item.append(link, size);
    list.append(item);
  }
  dialog.showModal();
});

document.getElementById("reset").addEventListener("click", async () => {
  if (!confirm("Forget all layouts, settings and results saved in this browser?")) return;
  await forgetAll();
  location.reload();
});

boot().catch((error) => {
  console.error(error);
  setStatus(`Failed: ${error.message ?? error}`);
  terminal.write(`\r\nakler could not start: ${error.message ?? error}\r\n`);
});
