// Publish web/dist to the gh-pages branch of the repository's origin as a
// single fresh commit, replacing what was there. Run `npm run build` first.
import { execFileSync } from "node:child_process";
import { existsSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const web = dirname(fileURLToPath(import.meta.url));
const root = join(web, "..");
const dist = join(web, "dist");
const git = (cwd, ...args) =>
  execFileSync("git", args, { cwd, encoding: "utf8", stdio: ["ignore", "pipe", "inherit"] }).trim();

if (!existsSync(join(dist, "akler.wasm"))) throw new Error("run `npm run build` first");
const remote = git(root, "remote", "get-url", "origin");
const revision = git(root, "rev-parse", "--short", "HEAD");
const repository = join(dist, ".git");
rmSync(repository, { recursive: true, force: true });
try {
  git(dist, "init", "-q", "-b", "gh-pages");
  git(dist, "add", "-A");
  git(dist, "commit", "-q", "-m", `Publish akler web from ${revision}`);
  console.log(`Pushing web/dist to gh-pages on ${remote}…`);
  execFileSync("git", ["push", "-f", remote, "gh-pages"], { cwd: dist, stdio: "inherit" });
} finally {
  rmSync(repository, { recursive: true, force: true });
}
