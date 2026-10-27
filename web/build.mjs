// Build the static site in web/dist: akler.wasm, the page, its vendored
// runtime (Wasmer SDK, xterm.js) and the bundled data directory.
import { execFileSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const web = dirname(fileURLToPath(import.meta.url));
const root = join(web, "..");
const dist = join(web, "dist");
const data = join(dist, "data");
const modules = join(web, "node_modules");

const run = (command, args, options = {}) =>
  execFileSync(command, args, { cwd: root, stdio: ["ignore", "inherit", "inherit"], ...options });

if (!existsSync(modules)) throw new Error("run `npm install` in web/ first");

console.log("Building akler.wasm and the native helper…");
run("cargo", ["build", "--release", "--target", "wasm32-wasip1-threads"]);
run("cargo", ["build", "--release"]);

rmSync(dist, { recursive: true, force: true });
mkdirSync(data, { recursive: true });
cpSync(join(root, "target/wasm32-wasip1-threads/release/akler.wasm"), join(dist, "akler.wasm"));
for (const file of ["index.html", "main.js", "style.css"]) {
  cpSync(join(web, "src", file), join(dist, file));
}
// Only font files, so nothing else dropped in the folder is published.
cpSync(join(web, "src/fonts"), join(dist, "fonts"), {
  recursive: true,
  filter: (path) => statSync(path).isDirectory() || /\.(otf|ttf|woff2?)$/.test(path),
});
cpSync(join(modules, "coi-serviceworker/coi-serviceworker.min.js"), join(dist, "coi-serviceworker.js"));

// The runtime is copied as published: it loads its workers and wasm
// relative to its own files, which a bundler would break.
const sdk = join(modules, "@wasmer/sdk");
cpSync(join(sdk, "dist"), join(dist, "vendor/wasmer-sdk/dist"), {
  recursive: true,
  filter: (path) => !path.endsWith(".map") && !path.endsWith(".d.ts"),
});
cpSync(join(sdk, "pkg"), join(dist, "vendor/wasmer-sdk/pkg"), { recursive: true });
mkdirSync(join(dist, "vendor/xterm"), { recursive: true });
cpSync(join(modules, "@xterm/xterm/lib/xterm.mjs"), join(dist, "vendor/xterm/xterm.mjs"));
cpSync(join(modules, "@xterm/xterm/css/xterm.css"), join(dist, "vendor/xterm/xterm.css"));
cpSync(join(modules, "@xterm/addon-fit/lib/addon-fit.mjs"), join(dist, "vendor/xterm/addon-fit.mjs"));
cpSync(join(modules, "@xterm/addon-webgl/lib/addon-webgl.mjs"), join(dist, "vendor/xterm/addon-webgl.mjs"));

// Data: layouts without backups or reports, the served corpora, the scissor
// test's word list, and akler.conf using one worker per core.
const skip = (name) =>
  name.startsWith(".") || name.endsWith(".bak") || name.endsWith(".run.txt") || name.endsWith(".csv");
// Only layouts git tracks: local files and optimizer output stay private.
const tracked = execFileSync("git", ["ls-files", "-z", "layouts"], { cwd: root, encoding: "utf8" })
  .split("\0")
  .filter((path) => path && !path.split("/").some(skip));
for (const path of tracked) {
  mkdirSync(dirname(join(data, path)), { recursive: true });
  cpSync(join(root, path), join(data, path));
}
// Only the corpora listed here are served, to keep the first load small.
const CORPORA = ["corpus-reddit.json"];
mkdirSync(join(data, "corpus/processed"), { recursive: true });
for (const name of CORPORA) {
  cpSync(join(root, "corpus/processed", name), join(data, "corpus/processed", name));
}
const words = execFileSync(join(root, "target/release/akler"), ["scissor-words"], { cwd: root });
writeFileSync(join(data, "corpus/scissor-words.txt"), words);
const config = readFileSync(join(root, "akler.conf"), "utf8").replace(/^workers = \d+$/m, "workers = 0");
writeFileSync(join(data, "akler.conf"), config);

const manifest = [];
const list = (directory) => {
  for (const name of readdirSync(directory)) {
    const path = join(directory, name);
    if (statSync(path).isDirectory()) list(path);
    else if (name !== "manifest.json") manifest.push({ path: relative(data, path), size: statSync(path).size });
  }
};
list(data);
writeFileSync(join(data, "manifest.json"), JSON.stringify(manifest));

// Static hosts that read _headers (Netlify, Cloudflare Pages) isolate the
// page, which threads require.
writeFileSync(
  join(dist, "_headers"),
  "/*\n  Cross-Origin-Opener-Policy: same-origin\n  Cross-Origin-Embedder-Policy: require-corp\n",
);
// GitHub Pages would otherwise run Jekyll, which drops files starting with _.
writeFileSync(join(dist, ".nojekyll"), "");
// A custom domain for GitHub Pages, one line in web/CNAME such as akler.example.com.
if (existsSync(join(web, "CNAME"))) cpSync(join(web, "CNAME"), join(dist, "CNAME"));

const size = (directory) =>
  readdirSync(directory).reduce((sum, name) => {
    const path = join(directory, name);
    return sum + (statSync(path).isDirectory() ? size(path) : statSync(path).size);
  }, 0);
console.log(`web/dist: ${(size(dist) / 1e6).toFixed(1)} MB, ${manifest.length} data files`);
