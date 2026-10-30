#!/usr/bin/env python3
"""Compare fixed-work optimizer throughput with 16 workers.

Example: python3 bench/trials16.py --layout layouts/esp-r.jsonc --before /tmp/old-akler
"""

import argparse
import re
import shutil
import statistics
import subprocess
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
ROW = re.compile(
    r"seed\s+1\s+score\s+([\d.+-]+)\s+wall\s+([\d.]+)\s+s\s+"
    r"(\d+) restarts\s+(\d+) trials\s+(\d+) trials/s"
)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--layout", default="layouts/lyrebird.jsonc")
    parser.add_argument("--corpus", default="corpus/processed/corpus-reddit.json")
    parser.add_argument("--config", default="bench/akler.conf", help="configuration to benchmark")
    parser.add_argument("--after", default="target/release/akler")
    parser.add_argument("--before", help="optional older executable to compare")
    parser.add_argument("--runs", type=int, default=4, help="runs per executable")
    parser.add_argument("--restarts", type=int, default=16, help="search restarts")
    parser.add_argument("--passes", type=int, default=1, help="maximum sweep passes")
    parser.add_argument("--steps", type=int, default=3000, help="annealing steps per restart")
    parser.add_argument("--adaptive", type=int, default=0, help="maximum adaptive swaps to fit")
    parser.add_argument("--caps", action="store_true", help="enable SFB and SFS limits")
    args = parser.parse_args()
    if args.runs < 1:
        parser.error("--runs must be positive")
    if args.steps < 1:
        parser.error("--steps must be positive")
    if args.restarts < 1 or args.passes < 1 or args.adaptive < 0:
        parser.error("--restarts and --passes must be positive, and --adaptive nonnegative")

    def path(value):
        return (ROOT / value).resolve()

    layout, corpus, config = path(args.layout), path(args.corpus), path(args.config)
    binaries = {"after": path(args.after)}
    if args.before:
        binaries = {"before": path(args.before), **binaries}
    for item in [layout, corpus, config, *binaries.values()]:
        if not item.is_file():
            parser.error(f"file not found: {item}")

    # Corpus caches are written beside their JSON; keep benchmark artifacts in
    # a temporary directory instead of changing the user's corpus directory.
    with tempfile.TemporaryDirectory(prefix="akler-trials16-") as directory:
        fixture = Path(directory)
        (fixture / "layouts").mkdir()
        local_layout = fixture / "layouts" / layout.name
        local_corpus = fixture / corpus.name
        shutil.copyfile(layout, local_layout)
        shutil.copyfile(corpus, local_corpus)
        common = [
            "bench", str(local_layout), str(local_corpus),
            "--config", str(config), "--seeds", "1",
            "--set", "workers=16", "--set", f"restarts={args.restarts}",
            "--set", f"passes={args.passes}",
            "--set", f"magic_anneal_steps={args.steps}", "--set", "seconds=0",
            "--set", "mode=normal", "--combos", "0", "--adaptive", str(args.adaptive),
        ]
        if args.caps:
            common += ["--set", "max_sfb_increase=0.02", "--set", "max_sfs_increase=0.04"]

        results = {name: [] for name in binaries}
        limits = "SFB/SFS overrides on" if args.caps else "limits from config"
        print(f"{layout.name} / {corpus.name}: 16 workers, {args.restarts} restarts, "
              f"{args.passes} passes, {args.steps} steps, {args.adaptive} adaptive swaps, "
              f"config {config}, {limits}", flush=True)
        for run in range(args.runs):
            names = list(binaries)
            if run % 2:
                names.reverse()
            for name in names:
                proc = subprocess.run(
                    [str(binaries[name]), *common], cwd=fixture, text=True,
                    stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                )
                if proc.returncode:
                    raise SystemExit(proc.stdout + proc.stderr)
                match = ROW.search(proc.stdout)
                if match is None:
                    raise SystemExit(f"Could not read benchmark result:\n{proc.stdout}\n{proc.stderr}")
                score, _, restarts, trials, rate = match.groups()
                rate = int(rate)
                results[name].append(rate)
                print(f"{name} {run + 1}: {rate:,} trials/s ({trials} trials, {restarts} restarts, score {score})", flush=True)
        for name, rates in results.items():
            print(f"{name}: median {statistics.median(rates):,.0f} trials/s; range {min(rates):,}–{max(rates):,}")
        if len(results) == 2:
            before = statistics.median(results["before"])
            after = statistics.median(results["after"])
            print(f"change: {(after / before - 1) * 100:+.1f}%")


if __name__ == "__main__":
    main()
