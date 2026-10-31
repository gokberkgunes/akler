#!/usr/bin/env python3
"""Optimize every physical magic-key position from the same JSONC layout.

Each position gets a separate search, a locked magic key, and the same input
baseline and configuration. Results and logs are saved under bench/results/.
"""

import argparse
import csv
from datetime import datetime
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import time


ROOT = Path(__file__).resolve().parent.parent


def read_jsonc(path):
    text = path.read_text()
    text = re.sub(
        r'("(?:\\.|[^"\\])*")|//[^\n]*|/\*[\s\S]*?\*/',
        lambda match: match[1] or "",
        text,
    )
    return json.loads(text)


def physical_keys(layout):
    keys = []
    for row, line in enumerate(layout["layout"]["fingers"], 1):
        for col, label in enumerate(line.split(), 1):
            keys.append(dict(slot=len(keys) + 1, row=row, column=col, key=label))
    thumb_column = 1
    thumb_columns = layout.get("board", {}).get("thumbColumns", [])
    thumb_index = 0
    for line in layout["layout"].get("thumbs", []):
        for label in line.split():
            column = thumb_columns[thumb_index] + 1 if thumb_index < len(thumb_columns) else thumb_column
            keys.append(dict(slot=len(keys) + 1, row=4, column=column, key=label))
            thumb_column += 1
            thumb_index += 1
    return keys


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--layout", default="layouts/baris.jsonc")
    parser.add_argument("--corpus", default="corpus/processed/corpus-reddit.json")
    parser.add_argument("--config", default="akler.conf")
    parser.add_argument("--binary", default="target/release/akler")
    parser.add_argument("--seconds", type=int, default=180)
    parser.add_argument("--workers", type=int, default=16)
    parser.add_argument("--output")
    parser.add_argument("--slots", type=int, nargs="+", help="only these 1-based physical slots")
    args = parser.parse_args()
    if args.seconds < 1 or not 1 <= args.workers <= 256:
        parser.error("seconds must be positive; workers must be in 1..256")
    binary, source, config, corpus = [
        (ROOT / value).resolve()
        for value in [args.binary, args.layout, args.config, args.corpus]
    ]
    layout = read_jsonc(source)
    keys = physical_keys(layout)
    if args.slots and any(slot < 1 or slot > len(keys) for slot in args.slots):
        parser.error(f"slots must be in 1..{len(keys)}")
    selected = [key for key in keys if not args.slots or key["slot"] in args.slots]
    # Measure the starting magic position first, then every other position.
    selected.sort(key=lambda key: (key["key"] != "*", key["slot"]))
    output = (ROOT / args.output).resolve() if args.output else (
        ROOT / "bench/results" / f"{source.stem}-magic-positions-{datetime.now():%Y%m%d-%H%M%S}"
    )
    output.mkdir(parents=True, exist_ok=False)
    (output / "layouts").mkdir()
    shutil.copyfile(config, output / "akler.conf")
    original = output / source.name
    shutil.copyfile(source, original)
    version = subprocess.check_output([str(binary), "--version"], text=True).strip()
    metadata = dict(
        version=version, layout=str(source), corpus=str(corpus),
        layout_sha256=hashlib.sha256(source.read_bytes()).hexdigest(),
        corpus_sha256=hashlib.sha256(corpus.read_bytes()).hexdigest(),
        seconds=args.seconds, workers=args.workers, positions=selected,
        baseline="Original input, subject to the configured combo/adaptive count limits",
    )
    (output / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
    fields = ["slot", "row", "column", "key", "status", "score", "sfb", "sfs", "wred", "wall_seconds", "layout", "error"]
    results = []

    def save_table():
        with (output / "results.csv").open("w", newline="") as file:
            writer = csv.DictWriter(file, fieldnames=fields)
            writer.writeheader()
            writer.writerows(sorted(results, key=lambda row: (row["status"] != "ok", row.get("score", float("inf")))))
        (output / "progress.json").write_text(json.dumps(dict(completed=len(results), total=len(selected), results=results), indent=2) + "\n")

    print(f"{len(selected)} positions, {args.workers} workers, {args.seconds}s per position", flush=True)
    print(f"Results: {output}", flush=True)
    for index, key in enumerate(selected, 1):
        slot = key["slot"]
        command = [str(binary), "bench", str(original), str(corpus),
                   "--config", str(output / "akler.conf"), "--seeds", "1",
                   "--magic-at", str(slot), "--set", f"seconds={args.seconds}",
                   "--set", f"workers={args.workers}", "--show",
                   "--save", str(output / "layouts"), "--tag", f"pos{slot:02}"]
        print(f"[{index}/{len(selected)}] slot {slot}: row {key['row']} col {key['column']} ({key['key']})", flush=True)
        started = time.monotonic()
        row = dict(key, status="failed")
        try:
            proc = subprocess.run(command, cwd=output, text=True, capture_output=True, timeout=args.seconds + 300)
            log = f"Command: {json.dumps(command)}\n\n{proc.stdout}\n{proc.stderr}"
            (output / f"pos{slot:02}.log").write_text(log)
            if proc.returncode:
                raise RuntimeError(proc.stderr.strip() or f"exit {proc.returncode}")
            match = re.search(r"SFB ([\d.]+)%  SFS ([\d.]+)%  WRED ([\d.]+)%  rescored ([\d.+-]+)", proc.stdout)
            if not match:
                raise RuntimeError("missing final metrics")
            saved = output / "layouts" / f"gen-{source.stem}-pos{slot:02}-s1.jsonc"
            optimized = physical_keys(read_jsonc(saved))
            if optimized[slot - 1]["key"] != "*":
                raise RuntimeError("magic key moved away from its locked position")
            sfb, sfs, wred, score = map(float, match.groups())
            row.update(status="ok", score=score, sfb=sfb, sfs=sfs, wred=wred, layout=str(saved))
            print(f"  score {score:.6f}, SFB {sfb:.6f}, SFS {sfs:.6f}, WRED {wred:.6f}; lock verified", flush=True)
        except (RuntimeError, subprocess.TimeoutExpired) as error:
            row["error"] = str(error)
            print(f"  failed: {error}", flush=True)
        row["wall_seconds"] = round(time.monotonic() - started, 3)
        results.append(row)
        save_table()
    successful = [row for row in results if row["status"] == "ok"]
    print(f"Finished: {len(successful)}/{len(selected)} positions produced eligible results. Table: {output / 'results.csv'}", flush=True)


if __name__ == "__main__":
    main()
