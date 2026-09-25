# Usage, configuration, and profiling

[README](../README.md) · [Layouts](LAYOUTS.md) · [Metrics and weights](METRICS.md)

Run commands from your project/data directory. Replace uppercase placeholder
paths with your files. Layout files under `layouts/` are editable user data;
tests contain inline layouts and do not require that directory.

Layouts are recognized by their contents: DAT, JSON, and JSONC work with any
filename, including `seconds` without an extension. Directory discovery ignores
unrelated files and reports errors for recognized but invalid layouts. Equivalent
same-stem `.dat`/`.json`/`.jsonc` companions appear once, preferring `.dat`;
companions with different bindings, rules, or geometry remain separate layouts.

## Configuration

Edit the commented [akler.conf](../akler.conf) in the current directory:

| Section | Contents |
|---|---|
| `[weights]` | Detailed metric and off-home weights; see [METRICS.md](METRICS.md). |
| `[search]` | Search options, simple-mode weights, and `corpus.NAME` mixture shares. |
| `[rolls]` | Independent `include_thumbs`, `include_scissors`, and `include_stretches` toggles. |
| `[ranker]` | `columns = SCORE SFB SFS ...`, using displayed metric names. |
| `[ngrams]` | Independent `trigrams`, `tetragrams`, and `pentagrams` maximum counts. |

Entries use `name = value`; `#` starts a comment. Missing entries use built-in
defaults. Either optimizer's setup `s` saves weights, roll filters, and search settings;
the ranker's column chooser `s` saves columns. These saves update their sections
while preserving comments and other settings. Unknown/duplicate sections or keys,
empty values, and invalid numbers are errors. Layout rules and geometry remain
in each layout file; normalization remains in each corpus's `.config.json`.

An existing `layouter.conf` is read when `akler.conf` is absent. The first TUI
configuration save copies its settings to `akler.conf`. Older
`optimizer-weights.conf`, `optimizer-search.conf`, and `ranker-columns.conf`
are read only when neither unified file exists. The first TUI configuration
save then migrates those values to `akler.conf`.
To migrate manually, copy the old weight/search entries under `[weights]`/`[search]`, and
put the old column list after `columns =` under `[ranker]`. Once the unified file
exists, legacy files are not merged into it. All n-gram limits default to `all`,
which preserves the existing full-cache behavior.

## Roll settings

```ini
[rolls]
include_thumbs = false
include_scissors = true
include_stretches = true
```

Defaults preserve all directional rolls without thumbs. Set both movement
flags to `false` for rolls without scissors or stretches; either flag can also
be disabled independently. `include_stetches` is accepted as an alias for
`include_stretches`. Use `true`/`false`, not numbers.

The switches apply to IN2/OUT2/IN3/OUT3, INROLL/OUTROLL, and combined ROLL in
editor, ranker, evaluation, and both optimizer objectives. They therefore
change roll credit and can change optimizer decisions. They do not change
how magic keys type the corpus or how other metrics are classified.

Filters check consecutive pairs AB and BC, not skip pair AC. They use
akler's full/half-scissor and lateral-stretch definitions. Those movement
classifiers exclude thumb pairs; when thumbs are enabled, a main-finger pair
within the same trigram can still veto its roll. Thumb is inward of index.

The denominator is **all mapped trigrams permitted by `include_thumbs`**,
including non-rolls and rolls rejected by either movement filter. Enabling
thumbs changes only roll denominators, not ALT/RED/etc. Excluding thumbs
never joins presses across a thumb.

After editing the file, use `r` in the ranker or optimizer setup; reopen an
editor to load new settings. Existing open ranker details keep their original
settings until the ranker is reloaded. Corpus tables can be reused: only the
geometry/evaluation state depends on these switches. Saved optimization run
records include the active `[rolls]` settings.

## Ranker and editor

```sh
./target/release/akler ranker CORPUS.json
./target/release/akler editor LAYOUT CORPUS.json
./target/release/akler optimizer LAYOUT CORPUS.json
./target/release/akler eval LAYOUT CORPUS.json
```

The ranker evaluates ordinary and magic/adaptive layouts with their existing
evaluators. Each layout uses its own supported events, so check coverage and
ignored characters before comparing different key sets. Scores use detailed
weights; hiding a column never removes its score contribution.

Press `v` in the ranker to choose columns: arrows or j/k move, Space/Enter toggle,
`d` selects compact defaults, `a` selects all, and `s` saves `[ranker] columns` in
`akler.conf`. Escape/q cancels. Defaults are
shared by ordinary and action layouts. Detailed D/C categories and OSF remain
available without crowding the initial table. The column value accepts
whitespace-separated metric names (case-insensitive); at least one is required.
Compact defaults are SCORE, SFB, SFS, TRAVEL, SFTRAVEL, FSB, HSB, FSS, HSS, LSB,
LSS, RED, INROLL, OUTROLL, ALT, and COVERAGE. IN2/OUT2/IN3/OUT3 and combined
ROLL are available in the column picker; existing saved column selections are
preserved. `H` in the main ranker restores all columns/rows
for the current session. Middle-click hiding is temporary unless saved via `v`, `s`.
Ranker `r` reloads configuration, including weights/limits, and the layouts.
Opened inspectors use the same weight snapshot as their ranking results. Magic
rows initially retain exact summary values. Opening one prepares its detailed
physical reports with a cancellable progress screen, then reuses that result on
subsequent opens. Cancellation leaves the summary intact and lets you retry.
Reloading the ranking or changing corpus discards these cached details. There
is no background prefetch. `i` shows corpus notes, including retained-count/
frequency-mass warnings.

Click metrics in the editor/results to inspect physical contributors; magic
labels denote actual action slots, not their emitted characters. `.` toggles
precision. The help page (`?`) lists mode-specific controls. Action editor `c`
changes corpus, `w` edits weights, `t` traces output text, and `p` traces specified
physical presses. Its `s` saves a new layout copy and `r` restores the original;
these editor keys do not save/reload configuration. Action `i` shows corpus notes.

Each layout save writes both `.dat` and `.jsonc` with the same stem. Ordinary
editor `s` confirms replacement of that pair and keeps backups; `S` saves a new
pair. Imported `.json` and extensionless source files remain separate. Action
editor and optimizer saves always choose a new stem, skipping any existing
companion or run record. Optimizer pairs share one `.run.txt` record.

Both optimizer setup screens use Space to run, `s` to save settings, `r` to
reload settings, `p` for presets, `g` for design, `m` for scoring mode, `n` for
a new seed, and `x` for corpus shares. `d` restores default settings; `o` restores
the original arrangement. Setup reload updates weights/search; corpus n-gram
limits stay with the loaded corpus until it is reopened. Click keys to toggle
locks, `H` restores default locks, `U` unlocks except Space, and `L` locks all. Magic refinement initially
locks named actions as well as home/thumb keys; generation initially locks
Space only. Detailed weights also control action typing effort, even when the
search objective uses simple mode.

Magic search results use `r` to return to setup, Space to refine the selected
candidate with a new seed, `b` to choose a retained candidate, and `[`/`]` to
navigate candidates. Caps continue to reference the original loaded layout.
`s` saves the current layout with its run record; `S` saves each retained
candidate and its record (without the ordinary optimizer's batch CSV). The
candidate chooser lists objectives; it has no side-by-side comparison grid.
Detailed metrics are prepared on first viewing each candidate. Simple-mode
contributors also show physical action labels; `d` sorts and `a` switches
between top rows and all rows. The labeled search objective uses
the configured mode and training mixture; the metric cards describe the selected
corpus. Live search shows the keyboard, metric and finger-usage panels, and
progress. See [layout syntax](LAYOUTS.md) for supported rules.

Explicit action diagnostics are also available:

```sh
./target/release/akler magic trace LAYOUT "some text"
./target/release/akler magic trace-keys LAYOUT "h @"
./target/release/akler magic report LAYOUT CORPUS.json report.json
```

Trace mode is a separate diagnostic and is not a replacement for the bounded
n-gram evaluation used by the editor, ranker, and optimizer.

## Corpora

```sh
./target/release/akler corpus add mycorpus /path/to/input
./target/release/akler corpus build mycorpus --order 5
./target/release/akler corpus info mycorpus
./target/release/akler corpus top mycorpus 3 20
```

Without a corpus, Editor, Ranker, and Optimizer show setup instructions.
**Corpora → Import text corpus** remains available even when the corpus list
is empty: enter the text file's path and a corpus name. Import copies the source
and builds its cache with the usual cancellable progress display. Cancelling
the build leaves the imported text available for a later rebuild.

Raw text files can have any extension or no extension. Place them directly in
`corpus/raw/`, or import them through the TUI or `corpus add`. Imports retain the
existing internal `NAME.txt` naming convention; your input file needs no suffix.
Files ending in `.config.json` are reserved for corpus configuration and are
not offered as text sources. Outside `corpus/raw/`, `.json` paths remain frequency
caches; use import for a raw text file named with that suffix.
Keep corpus names unique: `book`, `book.txt`, and `book.md` share the same cache
name, so keeping more than one of them in `corpus/raw/` produces an error.

Raw text lives in `corpus/raw/`; the built cache is
`corpus/processed/corpus-mycorpus.json`. `--order 3`, `4`, or `5` selects maximum
order; `--jobs N` selects corpus-build workers. The Corpora TUI also supports
rebuilding with keys 3/4/5. Rebuilding requires raw text; an existing JSON can
still be evaluated without it. Runtime evaluation reads cached n-grams, not an
ordered raw-corpus traversal.

An optional `corpus/raw/mycorpus.config.json` contains these JSON properties:

| Property | Meaning / default |
|---|---|
| `input_graphemes` | Allowed one-character ASCII outputs. Omitted: printable ASCII, with uppercase normalized to lowercase. An explicit list defines its own accepted characters. |
| `normalization` | Map individual Unicode input characters to one accepted printable ASCII output, e.g. `{"é":"e"}`. Default empty. |
| `word_separator` | One printable ASCII separator. Default Space. Repeated separators are collapsed and leading/trailing separators omitted. |
| `min_sequence_length` | Minimum normalized sequence length retained; default 1, allowed 1–1,000,000. |
| `max_order` | Maximum stored contiguous n-gram order; default 5, allowed 3–5. |
| `jobs` | Build worker count, 1–16. Default available parallelism capped at 4. |

`corpus add NAME INPUT CONFIG.json` copies an optional configuration.
Unsupported input characters form boundaries while building a corpus. A layout
may also lack a character that is present in the stored corpus. Such characters
are not deleted to join their neighbors: `abc!def` never becomes `abcdef`.
Stored adjacent pairs/triples require every character they contain to be available.

Ordinary skipgrams deliberately use **endpoints only**: the stored `a…b` from
`a!b` counts when `a` and `b` are available, even if `!` is not. This applies to
all ordinary skip metrics and their denominators. It never creates an adjacent
`ab` bigram. Imported endpoint-only skip tables remain usable without reconstructing
or validating their middle characters.

Action evaluation is different: its physical histories are produced by typing
selection, and an unavailable output resets that history and action memory.
It therefore does not create a physical skipgram across the gap. Ordinary and
action skip totals can differ on layouts with unavailable corpus characters.
Use the same supported character set when comparing them.

Skipgrams derived from an already-pruned trigram file describe only that file's
remaining evidence; evaluation cannot recover missing source data.

Ordinary layouts use unigrams, bigrams, skipgrams, and trigrams. Stored fourth/
fifth orders provide additional typing context for action layouts, not new
four/five-key ergonomic metric definitions. Actions use the highest contiguous
order available, up to 5, and require consistent frequency scales across orders.

Each action context is mapped from a fresh bounded history; shorter residual
contexts account for sequence starts. Only its terminal press/pair/triple is
counted with that context's frequency. Physical skipgrams come from mapped
trigrams. This is an estimate of stateful typing, not exact whole-text decoding.
Increasing context length does not prove exactness for arbitrary repeat chains.

### Optional n-gram limits

The default keeps every entry. To set separate maximum counts, edit:

```ini
[ngrams]
trigrams = 2000
tetragrams = 2000
pentagrams = 2000
```

Each accepts `all` or a positive integer. These are evaluation limits; they do
not rewrite your corpus JSON or change which orders the corpus builder stores.
Read the retained-count/frequency-mass warnings when comparing limited results.
`corpus info` and `corpus top` inspect the full stored data, regardless of these
evaluation limits.

For action evaluation the original tables are validated first. Trigrams are
ranked by frequency, with lexical order breaking ties. Higher orders are first
restricted to entries whose shorter suffix remains, then ranked/capped the same
way. Each order has its own hard maximum, but the retained sets interact: a tight
trigram limit also restricts which tetragrams/pentagrams can survive. No parent
table is silently enlarged past its limit.

An order may retain no entries even with its own limit set to `all` if its
shorter suffixes were removed. The source context-order label stays unchanged;
it describes available source order, not a guarantee of retained long history.
Warnings list each changed order's retained unique count and frequency percentage.

Unigrams and bigrams remain complete. Dropped longer contexts fall back to
shorter residual contexts, preserving terminal text-frequency mass within the
existing floating-point tolerances. No characters are joined across gaps. This loses
typing history: magic/repeat selection, physical counts, metrics, caps, accepted
swaps, and rankings may change. A cap of 2,000 is a speed/coverage tradeoff, not
an exact substitute for full evaluation. Increasing any limit can also change
results; use identical settings across comparisons.

Ordinary evaluation applies only the trigram limit. Skipgrams are not pruned by
that limit; if absent, they are derived from the original stored trigrams before
limiting. Ordinary SFB/SFS therefore use the full stored usable pair/endpoint-skip
evidence, while trigram/rhythm statistics use the retained trigrams.
Tetragram/pentagram limits only affect action evaluation.

Imported caches may already be pruned; limits cannot recover missing evidence.
Coverage refers to stored source tables, not original raw text. Use `all` for
final comparisons against the existing full-cache behavior. Action evaluation
remains a bounded-context estimate even with `all`.

## Search settings

Detailed weights live under `[weights]` in `akler.conf`; definitions/defaults
are in [METRICS.md](METRICS.md). Both ordinary and magic optimizers consume the
following settings under `[search]` in the same file.

| Setting | Meaning | Default |
|---|---|---|
| `method` | `hybrid` adds annealing before greedy sweeps; `sweep` uses sweeps. | hybrid |
| `restarts` | Maximum search restarts. | 12 |
| `passes` | Maximum greedy sweep passes per local search. | 80 |
| `anneal_steps` | Annealing proposals per restart. | 6000 |
| `temperature_start` / `temperature_end` | Geometric annealing schedule; permits some worse feasible moves. | 0.30 / 0.003 |
| `cycle_probability` | Fraction of annealing moves using three-key cycles. | 0.20 |
| `seconds` | Search wall-time budget; 0 disables the time limit. | 30 |
| `seed` | Random seed, decimal or `0x` hexadecimal. | `0x8a5c3d9127e4b6f1` |
| `archive` | Maximum retained candidates. | 12 |
| `max_sfb_increase` / `max_sfs_increase` | Allowed increase from the original layout, in percentage points, per training corpus; `none` disables. | 0 / 0 |
| `max_travel_increase` / `max_sftravel_increase` | Analogous limits in key units/100; `none` disables. | none / none |
| `design` | `refine` improves the input; `random` starts shuffled layouts; `evolve` can cross/mutate retained parents. | refine |
| `mode` | `detailed` weights or seven-term `simple` scoring. | detailed |
| `preset` | Name recorded for a settings selection. File loading does not apply other preset values automatically. | custom |
| `min_distance` | Minimum differing letter positions for candidate diversity, and from the template for generation. | 6 |
| `simple_*` | Seven simple-mode weights listed in METRICS.md. | See table |
| `corpus.NAME` | Relative training share; positive shares are normalized to sum to 1. No mixture means selected corpus only. | No mixture |

`metrics` aliases `mode`; legacy `max_sfb`/`max_sfs` are also baseline-relative
increase limits, not absolute ceilings. Caps are checked per training corpus,
not just on the mixture's mean score. Weights and floating settings must be
finite/nonnegative; cycle probability is at most 1, and hybrid mode requires
0 < end temperature ≤ start temperature. Iteration counts must be positive.

Choosing a preset in either optimizer changes settings immediately:

| Preset | Effect |
|---|---|
| balanced | Simple defaults; SFB +0.05 pp, SFS +0.25 pp; no travel caps. |
| strict | Simple defaults; zero increase in SFB/SFS/TRAVEL/SFTRAVEL. |
| low-travel | Balanced caps plus zero increase in TRAVEL/SFTRAVEL. |
| explore | Simple defaults; SFB/SFS/travel caps disabled. |
| custom | Keeps current values. |

Magic search now supports the same hybrid/sweep methods, restarts, annealing,
three-key cycles, time budget, simple/detailed objectives, corpus mixtures, all
four caps, generation modes, and candidate archive controls. This intentionally
changes its search behavior: the unchanged defaults now mean hybrid search with
a 30-second budget, rather than the old fixed 40-pass greedy search. Neither
optimizer invents action rules or changes finger assignments.

Action mapping still uses detailed effort weights and its existing exact
incremental evaluator. It retains its 128-commit rebase schedule; the ordinary
evaluator has a different rebase schedule. Algorithm support does not imply
identical decisions between the two evaluators. Caps use the original layout's
metrics separately on each positive-share training corpus. Space remains fixed.
Magic diversity counts lowercase physical key labels, including adaptive letter
keys. Generation excludes known layouts only when geometry, action definitions,
and the binding permutation are compatible.

## Profiling

```sh
AKLER_PROFILE_LOAD=1 ./target/release/akler ranker CORPUS.json 2>ranker-load.txt
AKLER_PROFILE_LOAD=1 ./target/release/akler editor LAYOUT CORPUS.json 2>load-profile.txt
AKLER_PROFILE=1 ./target/release/akler optimizer LAYOUT CORPUS.json 2>search-profile.txt
```

Reports redirected to a file are written as each operation finishes. Without
redirection, reports are buffered while the TUI is active and printed to stderr
after exiting the TUI, once the terminal is restored. This prevents timing text
from corrupting the screen. Buffered text uses memory only when reports are
produced; redirect to a file for long profiling sessions. The loading variable
is `AKLER_PROFILE_LOAD`, not `PROFILE_LOAD`.

Menu-driven editor/optimizer loading selects a corpus path and layout before
preparing the required evaluator. Switching layouts within that chooser reuses
the parsed corpus; switching between ordinary and magic prepares only the
missing engine from the same retained JSON text. Layouts, action rules, and
geometry are reparsed on each open. No optimizer context cache is reused between
layouts.

This session cache checks the resolved corpus path, file metadata, and n-gram
limits before reuse. Changed corpus metadata discards its text and engines;
changed limits retain the text but rebuild the engines. Leaving the chooser or
changing corpus starts fresh. The chooser's **Reload corpus snapshot** item
forces a fresh read if an external replacement escaped metadata detection.
Memory holds one JSON text plus the evaluators requested during that chooser
session; there is no global cache or speculative background preparation.

Ranker startup separately discovers layouts first, reads its corpus file once,
and prepares only the required evaluators. An action-only ranking avoids ordinary
corpus preparation; a mixed ranking runs two different table parsers over the
one read. The action corpus is shared across layouts. Loading progress covers
discovery and evaluation without restarting for each layout.

Action ranking still maps every context for every layout and accumulates
transient physical histograms in the original order, preserving exact summary
values. It discards those histograms after scoring and defers detailed
metric-adapter construction until that row is opened. The action corpus is
shared between rows; opened details are cached until ranking reload/corpus
change. Ordinary rows retain their existing eager metric data. Use the phase
report to measure startup, first-open latency, and repeated opens locally;
loading on demand does not eliminate per-layout mapping.

Load profiling reports coarse operation phases; nested operation reports overlap
and must not be summed. `AKLER_PROFILE=1` enables action-search phase timings
and operation counters, with no per-context clock reads. `=2` additionally times
each mapping/contribution update and has more overhead. Other values disable it.
Press Space to start search; one aggregate report is collected at completion or
cancellation and routed as described above. No output is printed inside
candidate/context loops.

Top-level search phases are setup, affected-list construction, mapping/scoring,
normalization, cap/improvement checks, commit, rebase, and other. Named-action
time overlaps these phases; level-2 mapping/contribution times are nested within
mapping/scoring. Search timing excludes idle UI time and final detailed reporting.
With corpus mixtures, context counters sum all training corpora while candidates count search
moves. Sweep probes count as rejected; accepted commits count their separate
reevaluation. Restart/local-optimum restoration remaps tails outside the candidate
counters and appears in other time. These counts therefore differ from the old
single-corpus greedy loop even on the same layout.

Before checking a patch locally:

```sh
cargo check --release
cargo test --release -- --test-threads=1
cargo build --release
```

For speed comparisons set `seconds = 0` and keep input layouts, corpus, limits,
weights, method, restarts, passes, annealing steps, seed, caps, and locks identical.
Use profiling **disabled** and restart before each run:

```sh
env -u AKLER_PROFILE -u AKLER_PROFILE_LOAD ./target/release/akler optimizer MAGIC.dat CORPUS.json
env -u AKLER_PROFILE -u AKLER_PROFILE_LOAD ./target/release/akler optimizer ORDINARY.dat CORPUS.json
```

Start search with Space. Repeat with matching `AKLER_PROFILE=1` runs to
compare candidate/context/action operation counts. Profiling adds overhead, so
those timings are not the uninstrumented benchmark. A nonzero `seconds` budget
compares results achieved within a time limit rather than fixed-work runtime;
wall-clock cutoffs can stop seeded runs at different points.
