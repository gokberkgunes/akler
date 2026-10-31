// Run the same akler commands natively and as WebAssembly (under the Wasmer
// CLI) and report any difference in their output: every layout's evaluation
// on the served corpus, and short fixed-seed optimizer benches. Wall times
// and trials/s are ignored; scores and trial counts must match.
import { execFile, execFileSync } from "node:child_process";
import { readdirSync, statSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const native = join(root, "target/release/akler");
const wasm = join(root, "target/wasm32-wasip1-threads/release/akler.wasm");
const wasmer = process.env.WASMER ?? join(homedir(), ".wasmer/bin/wasmer");
const THREADS = "4";
const CORPUS = "corpus-monkeyracer";
const PARALLEL = 6;

console.log("Building native and WebAssembly akler…");
for (const args of [["build", "--release"], ["build", "--release", "--target", "wasm32-wasip1-threads"]]) {
  execFileSync("cargo", args, { cwd: root, stdio: ["ignore", "inherit", "inherit"] });
}

// The layouts the site serves (build.mjs skips the same files).
const skip = (name) =>
  name.startsWith(".") || name.endsWith(".bak") || name.endsWith(".run.txt") || name.endsWith(".csv");
const layouts = [];
const walk = (directory) => {
  for (const name of readdirSync(directory).sort()) {
    if (skip(name)) continue;
    const path = join(directory, name);
    if (statSync(path).isDirectory()) walk(path);
    else layouts.push(relative(root, path));
  }
};
walk(join(root, "layouts"));

const commands = [
  ...layouts.map((layout) => ["eval", layout, CORPUS]),
  ["bench", "layouts/vylet.jsonc", CORPUS, "--seeds", "1", "--set", "restarts=8"],
  ["bench", "layouts/night.jsonc", CORPUS, "--seeds", "1", "--set", "restarts=8", "--combos", "4"],
];

const run = (file, args) =>
  new Promise((resolve) => {
    execFile(
      file,
      args,
      { cwd: root, env: { ...process.env, AKLER_THREADS: THREADS }, maxBuffer: 64 << 20 },
      (error, stdout, stderr) => resolve({ status: error?.code ?? 0, stdout, stderr }),
    );
  });
const runNative = (args) => run(native, args);
const runWasm = (args) =>
  run(wasmer, ["run", "--volume", ".:/", "--env", `AKLER_THREADS=${THREADS}`, wasm, "--", ...args]);

/** Output without timings, which differ between any two runs. */
const normalize = (text) =>
  text
    .replace(/wall\s+[\d.]+ s(\/seed)?/g, "wall _")
    .replace(/\d+ trials\/s/g, "_ trials/s")
    .trimEnd();

const failures = [];
let done = 0;
const check = async (args) => {
  const [a, b] = await Promise.all([runNative(args), runWasm(args)]);
  const same = a.status === b.status && normalize(a.stdout) === normalize(b.stdout);
  done += 1;
  const label = args.join(" ");
  if (same) {
    console.log(`  ok   ${label}`);
    return;
  }
  const left = normalize(a.stdout).split("\n");
  const right = normalize(b.stdout).split("\n");
  const line = left.findIndex((text, i) => text !== right[i]);
  failures.push(label);
  console.log(`  DIFF ${label} (exit ${a.status} native, ${b.status} wasm)`);
  if (line >= 0) console.log(`       native: ${left[line]}\n       wasm:   ${right[line] ?? "(missing)"}`);
  if (b.stderr.trim()) console.log(`       wasm stderr: ${b.stderr.trim().split("\n")[0]}`);
};

console.log(`Comparing ${commands.length} commands (${layouts.length} evaluations, 2 benches)…`);
const queue = [...commands];
await Promise.all(
  Array.from({ length: PARALLEL }, async () => {
    while (queue.length) await check(queue.shift());
  }),
);
if (failures.length) {
  console.log(`\n${failures.length} of ${done} differ.`);
  process.exit(1);
}
console.log(`\nAll ${done} match.`);
