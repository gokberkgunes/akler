# Usage and settings

[README](../README.md) · [Metrics](METRICS.md) · [Layouts](LAYOUTS.md)

## Commands

```sh
akler                                   # home menu
akler ranker [CORPUS]
akler editor|optimizer [LAYOUT] [CORPUS]
akler eval LAYOUT [CORPUS]
akler bench LAYOUT CORPUS [options]     # see Benchmarking
akler magic trace LAYOUT "some text"    # also trace-keys, report
akler corpus list|info|add|build|top ...
```

The home menu has three tabs: **Workbench** (editor, ranker, optimizer),
**Analysis** (weakness report, magic designer, scissor test) and **Data**
(corpora). Every other screen has a Back button at the top left. Press `?`
on any screen for its controls.

The scissor test asks for the layout you type on, shows words that press
each same-hand, cross-row pair of fingers (or a drill like `ab ab ab`), and
asks you to rate your dislike from 0 (fine) to 4 (awful). Ratings, timings,
key geometry and Mana2's rating go to `scissors/LAYOUT.csv`; a later run
continues the pass, then starts the next.

## akler.conf

| Section | Contents |
|---|---|
| `[weights]` | Normal-mode weights; see [METRICS.md](METRICS.md). |
| `[aklgg]` | AKL.GG-mode weights; see [AKLGG.md](AKLGG.md). |
| `[search]` | Optimizer settings (below). |
| `[typing]` | `alt_fingers`, `combos`: use a layout's alternate fingers and combos. |
| `[alts]` | Alternate fingers by key position; see [ALTFINGERS.md](ALTFINGERS.md). |
| `[rolls]` | `include_thumbs`, `include_scissors`, `include_stretches` for roll metrics. |
| `[magic]` | Magic-key locations and rule fitting; see [MAGIC.md](MAGIC.md). |
| `[combos]` | Combo fitting; see [COMBOS.md](COMBOS.md). |
| `[adaptive]` | Adaptive-swap fitting; see [LAYOUTS.md](LAYOUTS.md#fitting-swaps). |
| `[ranker]` | Visible `columns`, `hide_magic`, `hide_non_thumb`. |
| `[ngrams]` | `include_spacegrams` and `trigrams`/`tetragrams`/`pentagrams` caps. |

Entries are `name = value`; `#` starts a comment; missing entries use
defaults. Unknown sections or keys are errors. Saving from the TUI (`s` in
optimizer setup, `q` in the ranker's column chooser, the `f`/`F`/`z` toggles)
updates only the affected settings and keeps comments. Hiding a ranker column
by clicking it also saves `columns`; `u`, `U` and `H` undo, redo and
restore hides made in the grid.

### Spacegrams

A spacegram is an n-gram containing Space (`e␠`, `␠t`, `e␠t`).
`include_spacegrams = false` drops them when loading the corpus, which changes
the denominators. Press `z` in the editor or ranker to toggle it.

### N-gram caps

`trigrams`, `tetragrams` and `pentagrams` accept `all` or a count. Caps keep
the most frequent entries in memory and never rewrite the corpus file. They
are faster but approximate: results can change, so compare layouts with the
same caps. Unigrams and bigrams are never capped.

## Search settings

| Setting | Meaning | Default |
|---|---|---|
| `method` | `hybrid` (annealing, then greedy sweeps) or `sweep`. | hybrid |
| `workers` | Parallel searches; 0 = all CPUs, 1 = serial. | 0 |
| `restarts` | Search restarts. Magic/action optimizers abandon restarts that fall behind early. | 12 |
| `passes` | Greedy sweep passes per local search. | 80 |
| `anneal_steps` | Annealing steps per restart, ordinary optimizer. | 30000 |
| `magic_anneal_steps` | Annealing steps per restart, magic/action optimizers. | 12000 |
| `temperature_start` / `temperature_end` | Annealing schedule. | 0.30 / 0.003 |
| `cycle_probability` | Share of three-key cycle moves. | 0.20 |
| `seconds` | Time limit; 0 = none. | 30 |
| `seed` | Random seed, decimal or `0x` hex. | `0x8a5c3d9127e4b6f1` |
| `archive` | Candidates kept. | 12 |
| `max_sfb_increase` / `max_sfs_increase` | Allowed increase over the original layout, in percentage points; `none` = no cap. | 0 / 0 |
| `max_travel_increase` / `max_sftravel_increase` | Same, in key units per 100. | none / none |
| `design` | `refine` the input, start `random`, or `evolve` from earlier results. | refine |
| `mode` | Scoring: `normal` ([weights]) or `aklgg` ([aklgg]). | normal |
| `preset` | Cap preset: `balanced`, `strict`, `low-travel`, `explore`, `custom`. | custom |
| `min_distance` | Minimum differing positions between kept candidates. | 6 |
| `corpus.NAME` | Training share for a corpus mixture. | none |

Lower scores are better in both modes. Caps always use AKLER's own SFB/SFS
and travel definitions and are checked per training corpus. In the optimizer,
`t` switches the scoring mode and tab `4` (**Score**) breaks the score down
by stat.

## Ranker and editor

- **Ranker:** tabs `1` Rankings, `2` Layouts, `3` Columns. `s` switches the
  scoring mode, `c` picks a corpus, `m`/`t` hide magic layouts / layouts
  without a thumb letter, `u` restores the last hidden row or column, `H`
  restores everything, `r` reloads. Hidden
  layouts are stored in `ranker-hidden.json`. Enter opens a layout in the
  editor.
- **Editor:** click a metric to see its contributors. `m` switches between
  simple and detailed stats, `d` toggles comparing against the last edit or
  the loaded layout, `c` changes the corpus. `s` saves `.dat` and `.jsonc`
  (with backups), `S` saves a copy.
- **Optimizer:** click keys to lock them, `H` locks the home row, `U`/`L`
  unlock/lock everything. Space starts a search, `q` stops it and keeps the
  best result. Results are saved under `layouts/optimized/` with a `.run.txt`
  record.

`f`, `F` and `z` toggle alternate fingers, combos and spacegrams.

## Corpora

```sh
akler corpus add NAME /path/to/text
akler corpus build NAME --order 5 --jobs 4
akler corpus info NAME
akler corpus top NAME 3 20
akler corpus prepare [NAME]
```

Raw text goes in `corpus/raw/` (any extension); the cache is written to
`corpus/processed/corpus-NAME.json`. Corpus names must be unique ignoring
extension. The first load under each `[ngrams]` setting also saves a
pre-parsed copy next to it (`corpus-NAME.KEY.ngrams`), which later loads
read in milliseconds; `corpus prepare` writes it ahead of time. An optional `corpus/raw/NAME.config.json` can set:

| Property | Meaning / default |
|---|---|
| `input_graphemes` | Allowed ASCII characters. Default: printable ASCII, lowercased. |
| `normalization` | Character mapping, e.g. `{"é":"e"}`. |
| `word_separator` | Separator character; default Space. |
| `min_sequence_length` | Shortest sequence kept; default 1. |
| `max_order` | Longest n-gram stored, 3–5; default 5. |
| `jobs` | Build workers, 1–16. |

Unsupported characters split the text; they are never deleted to join their
neighbours. Ordinary skipgrams only need their endpoints (`a!b` counts as
`a…b`). Magic and action layouts use 4- and 5-grams as typing context.

## Benchmarking

```sh
akler bench layouts/packet.dat corpus-reddit --seeds 10 --set mode=aklgg
```

Runs the optimizer without the TUI.

| Option | Effect |
|---|---|
| `--seeds N` | Number of seeds (default 6). |
| `--set key=value` | Override one `[search]` setting for this run. |
| `--config FILE` | Read FILE instead of `akler.conf`. `bench/akler.conf` is a frozen baseline. |
| `--unlocked` | Free every key. |
| `--full-ngrams` / `--rescore-full` | Search without caps / also print each score on all n-grams. |
| `--swaps N` | No search: print the best N single and double swaps. |
| `--chain K` | No search: print the best rotations of exactly K keys (2–12); `--swaps N` sets how many (default 15). |
| `--move A=B` | With `--chain`: only rotations that move key A into B's place. |
| `--hand left\|right` | With `--swaps` or `--chain`: move only that hand's keys. |
| `--keep-wred` | With `--chain`: drop rotations that add weak redirects on either hand. |
| `--combos K`, `--fit-combos K` | Combo limit for a run / fit combos only. |
| `--adaptive K`, `--fit-swaps K` | Swap limit for a run / fit swaps only. |
| `--save DIR`, `--tag NAME` | Save each seed's result. |
| `--show` | Print fitted rules, swaps and combos. |
| `--ranker CORPUS` | Time three full ranker loads. |

## Profiling

```sh
AKLER_PROFILE_LOAD=1 akler ranker CORPUS 2>load.txt
AKLER_PROFILE=1 akler optimizer LAYOUT CORPUS 2>search.txt
```

`AKLER_PROFILE=2` adds finer timings at more overhead. Without redirection,
reports print after the TUI exits. For speed comparisons, disable profiling
and set `seconds = 0` so runs do fixed work.
