# Atomic physical-pattern reports

Atomic reports select weighted physical key pairs or triples with a typed
query. The classifier's terms are definitions for this experimental feature;
they do not rename or change akler's existing metrics, scoring, or optimizer.

## Atomic editor

Choose **Atomic editor** from the main menu, then a corpus and layout. It
shows the existing keyboard drawing and the same weighted, sorted physical
rows as the command-line report. Click two keys or drag one onto another to
swap bindings. Arrows move the physical cursor; Space selects and swaps.
`j`/`k` move through table rows. Enter or `l` opens the selected group,
and `h` or Esc returns to the group list. Clicking a group row opens it;
clicking a detail row selects it. `u` undoes the last swap. Space stays fixed.

Use `/` to enter a query, `g` to group the matching rows, `Tab` to cycle
bigrams, trigrams, and skip1, `?` for help and examples, `s` to save a new
DAT/JSONC layout copy, `x` to export the full plain-text report to a separate
new file, Page Up/Down or the wheel to scroll, and `q` to return. Neither save
overwrites an existing file. Opening
another layout prepares a new physical snapshot; **Change corpus...** and
**Reload corpus snapshot** in the layout chooser replace cached corpus data.

A blank query selects all patterns. Canceling a prompt keeps the current results.
An invalid query leaves the previous valid results visible with an error
banner. A field that applies only to triples matches no pairs, even with
`!=`; an unavailable geometry attribute produces an error.

For example, try `roll.direction = inward and row.direction = descending`,
`redirect = true and endpoints.same_key = false`, or
`gap[0] = 1 and endpoints.same_finger = true`.

Press `g` to open a checklist. Move with Up/Down or `j`/`k`, toggle fields
with Space or a click. Enter adds the highlighted field and applies all
selected fields, including when none were selected. Press `c` to clear;
`q` cancels. Common row and finger fields appear first. `/` inside the
checklist adds an advanced field by name. For example,
`row.direction` groups ascending, descending, level, and mixed movements;
`start.finger_type, end.finger_type` groups anatomical movements such as
pinky to ring while folding left/right mirrors together; and
`start.row, end.row` groups logical row paths. Displayed rows remain 1-based.
Grouping is presentation only: it preserves the
active query and the full-population denominator. It also stays active through
population changes, swaps, and undo. A grouped text export contains both the
group summary and every matching physical detail row.

## Command line

Choose one population with `--patterns`:

```sh
./target/release/akler atomic layouts/afterburner.jsonc corpus/processed/corpus-e200.json \
  --patterns bigrams \
  --query 'roll.direction = inward and row.direction = descending'

./target/release/akler atomic layouts/afterburner.jsonc corpus/processed/corpus-e200.json \
  --patterns trigrams \
  --query 'redirect = true and endpoints.same_key = false'

./target/release/akler atomic layouts/racket.dat corpus/processed/corpus-e200.json \
  --patterns skip1 \
  --query 'endpoints.same_finger = true and endpoints.same_key = false'
```

`bigrams` uses adjacent positions `[0, 1]`, `trigrams` uses `[0, 1, 2]`,
and `skip1` uses `[0, 2]` with one intervening press. Omitting `--query`
selects every pattern in the chosen population. Invalid queries report the
source position of the error.

The report is printed to standard output. Add `--output PATH` to write the
same complete plain-text report to a new file:

```sh
./target/release/akler atomic layouts/racket.dat e200 --patterns bigrams \
  --output racket-inward.txt \
  --query 'roll.direction = inward'
```

An existing output file is not overwritten. Reports contain no terminal
colors or screen-width truncation. The aligned result table shows physical
keys, fingers, rows, `%`, and `Stats`; it omits per-row frequency. Rows display
from 1 (top = 1, home = 2, bottom = 3); Atomic queries still use their existing
logical row values and names.

`Stats` lists the applicable existing akler metric names for each physical pattern, including
overlapping broad categories and subsets such as `DFJB, FSB` or `RED, WRED`. Bigrams use
bigram flags, skip1 uses skip flags, and trigrams use triple flags. A row with
no applicable flag shows `—`. These names follow the existing metric rules and
configured roll settings; they are separate from Atomic query attributes.
Summary frequencies show six decimal places, while percentages show at most
two. Matching and sorting use the original unrounded values.

## Frequencies and denominators

Each row groups one physical slot sequence and gap structure. Rows are sorted
by descending frequency, then by physical slot IDs. Labels are presentation
only: duplicate labels include slot IDs and are never merged by label.

For the selected population:

```text
match percentage = 100 × matching frequency / population frequency
row percentage   = 100 × row frequency / population frequency
```

The population frequency is computed before the query, so changing a query
does not change its denominator. Bigrams, trigrams, and skip-1 pairs have
separate populations. An empty population reports `n/a`.

## Physical data and source limits

Atomic analysis reuses the evaluator's weighted physical n-gram data. A magic
winner contributes its actual root slot once; nested calls and unsuccessful
attempts add no presses. Swapped or moved bindings retain their current slot
identity and geometry. The emitted character is never used to recover a
physical key.

Existing typing boundaries, resets, gaps, and frequency weights are preserved.
Skip-1 pairs are taken from the evaluator's skip population; spaces or thumbs
are not removed to manufacture them. Magic skip pairs require three valid
physical positions, so missing characters and resets are not joined. Ordinary
stored skipgrams contain only endpoints. Their intervening key or reset cannot
be checked after loading; the report follows the existing ordinary evaluator's
endpoint-only skip policy, including its treatment of an unavailable middle.

This is an n-gram estimate rather than ordered raw-corpus traversal. Magic
analysis keeps the action evaluator's cached-context approximation and its
selected context order; it is not a fresh decode of the raw corpus. For an
ordinary corpus without an explicit skipgram table, skipgrams are derived from
its stored trigrams, so source pruning limits the available skip evidence.
The existing magic evaluator requires cached letters, bigrams, and trigrams
and rejects reachable actions that emit multiple characters in one press.

When an evaluator retains only a capped subset, the denominator is the
available retained population and the report includes that warning. A
population or geometry attribute unavailable from the selected source is
rejected instead of being treated as false.
