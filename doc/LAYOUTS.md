# Layouts, magic keys, and adaptive swaps

[README](../README.md) · [Metrics](METRICS.md) · [Usage and settings](USAGE.md)

## File formats and saving

Layout format is detected from contents, regardless of filename. DAT, JSON, and
JSONC can all be named `seconds`, for example. JSONC accepts `//` and `/* ... */`
comments and trailing commas. The command line, chooser, ranker, editor, and
optimizer use the same import rules.

Every layout save writes a pair with the same stem: `.dat` and `.jsonc`.
New-copy saves choose an unused stem if either companion or its run record
already exists. Ordinary editor `s` replaces the pair after confirmation and
keeps backups; an imported `.json` or extensionless source remains separate.
Saved files preserve the supported bindings, rules, finger assignments, and
geometry; source comments and formatting are not kept.
JSONC saves use four-space indentation, one keyboard/fingermap row per line,
and the field order `layout`, `fingermap`, `board`, `layers`, `magic`.
Ordinary magic and adaptive rules are written as readable `inputs`/`output`
pairs in `magic.rules`, with physical key labels in the keyboard rows.
The exporter checks that reimport preserves the bindings and reachable action
definitions exactly before using this simpler representation. It does not add
explicit repeat rules for unlisted contexts.

### Mana JSON/JSONC import

The supported base layout uses three `layout.fingers` strings, each containing
10, 11, or 12 whitespace-separated keys. Row widths may differ. Optional
`layout.thumbs` contains at most one key per hand, left then right; an empty
string omits that hand. Omitting `thumbs` supplies left Space. Use `skip`, `~`,
or `blank` for an empty physical slot and `space` for Space.

```jsonc
{
  "layout": {
    "fingers": [
      "q w e r t y u i o p",
      "a s d f g h j k l ;",
      "z x c v b n m , . ◇",
    ],
    "thumbs": ["space", ""],
  },
  "fingermap": [
    "0 1 2 3 3 6 6 7 8 9",
    "0 1 2 3 3 6 6 7 8 9",
    "0 1 2 3 3 6 6 7 8 9",
  ],
  "board": {
    "isRowStaggered": true,
    "rowOrColumnStagger": [0, 0.25, 0.75],
  },
  "magic": {
    "rules": [{"inputs": "h◇", "output": "hr"}],
  },
}
```

`fingermap` is optional; when present, its three strings must match the key
counts. Main-finger IDs are 0/1/2/3 for LP/LR/LM/LI and 6/7/8/9 for RI/RM/RR/RP.
Thumb IDs 4/5 are not valid on the three main rows. With
`board.isRowStaggered: true`, `rowOrColumnStagger` supplies three horizontal row
offsets. With `false`, it supplies one vertical offset per physical column.
Both use the [numeric geometry rules](#geometry-and-finger-assignments) below.

A magic rule's `inputs` ends with the physical key label; the preceding text is
its context. Its `output` must preserve that context and append exactly one
printable ASCII character. The example emits r from ◇ after h. Repeated inputs
use the last definition. Dedicated symbols such as `@`, `*`, and ◇ repeat for
unlisted contexts; rules on ordinary letters or punctuation retain that key's
literal fallback. `char:X` preserves a literal reserved symbol such as `char:@`.

This imports the supported base layout, finger map, geometry, and append-only
rules. Nonempty layers or combos, tap-hold keys, nonempty `magic.magicKeys`, nonzero
`board.splitAngle`, and `board.mirrorLeftRowStagger: true` are unsupported and
produce errors. Import does not reproduce Mana's complete typing engine.
Saved JSONC uses visible `layout`, `fingermap`, and `board` fields for editing.
Layouts requiring calls, explicit `none`, other history bases, or other behavior
that the simple rule format cannot preserve retain an `akler` extension with native
action definitions. Geometry may also need that extension. Existing layouts with
the `layouter` extension remain readable. Edit simple rules in
`magic.rules`, or native definitions in `akler.actions` when present; do not
combine the two action representations in the same file.

## Keyboard and thumbs

A `.dat` file begins with three keyboard rows. Use whitespace-separated keys,
with an optional `|` hand divider. Supported widths are 5|5, 6|5, 5|6, and 6|6;
each row may have a different width. An 11-slot row needs the hand divider.
`~` is an empty slot and `space` is Space.
Ordinary output keys are single printable ASCII characters, normalized to
lowercase. Names such as `capslock` are not ordinary output keys.

```text
q w e r t | y u i o p
a s d f g | h j k l ;
z x c v b | n m , . /
thumbs: space
```

`thumbs: LEFT RIGHT` assigns physical thumb slots explicitly; `thumbs: r space`
puts r on LT and Space on RT. Legacy single-key thumb lines use indentation:
0–10 leading spaces means LT, 11 or more means RT. Without an explicit Space,
the parser adds it to a free thumb slot when possible. With no thumb declaration,
that is LT. In action layouts, `thumbs: space space` creates two real Space slots;
typing selection decides which is pressed, rather than splitting counts evenly.
The ordinary evaluator requires unique output keys, so use one Space there.

Comments and settings follow the three rows. Optional `outer-left: TOP HOME
BOTTOM` / `outer-right: TOP HOME BOTTOM` add an outer pinky column when not already
present. Use `char:~` or `char:|` to distinguish literal output from syntax in an
action layout.

## Geometry and finger assignments

Numeric offsets are in key units, with exact 0.001-unit precision and a range
of −32 to +32. Values outside this range, non-finite values, and finer precision
are errors. Add three horizontal offsets for the top, home, and bottom rows:

```text
row-offsets: 0 0.25 0.75
column-offsets: 0 -0.2 -0.3 0 0 0 0 -0.3 -0.2 0
```

`column-offsets` supplies vertical offsets, left to right, for every physical
column from the leftmost to the rightmost column on the board. Its count must
match that span, including columns absent from a shorter row. The example
above has ten columns. Positive horizontal offsets move right; positive vertical
offsets move down. Offsets change travel and physical stretch; scissors, row
jumps, and same-row preferences still use the three logical rows.

To specify fingers explicitly, use one slash-separated list per keyboard row,
with exactly one finger label per slot:

```text
fingermap: LP LR LM LI LI RI RI RM RR RP / LP LR LM LI LI RI RI RM RR RP / LP LR LM LI LI RI RI RM RR RP
```

Main-row labels are LP/LR/LM/LI and RI/RM/RR/RP; thumbs keep LT/RT. Home travel
uses the matching finger's physical home-row position where present, otherwise
its conventional home coordinate. Finger assignments and geometry stay with
their physical slots when bindings move. Saved imports may include
`col-layout: absolute` to retain contiguous imported column positions.

### Existing row-stagger presets

Add `row-stagger: standard`, `anglemod`, `nokwts`, or `meteorite`. A bare
`row-stagger:` means standard; `standart` is an alias. Omit it or use `off` for
ortholinear defaults. Presets remain supported: `row-offsets` replaces a preset's
horizontal offsets, and an explicit `fingermap` replaces its finger assignments.

All non-off presets default to horizontal row offsets of −0.25, 0, and 0.5 key
units. The table refers to **QWERTY physical positions**, regardless of your
current bindings:

| Mode | Left pinky | Left ring | Left middle | Left index |
|---|---|---|---|---|
| standard | Q A Z | W S X | E D C | R T F G V B |
| anglemod | Q A | W S Z | E D X | R T F G C V B |
| nokwts | Q A Z | W S X | E D R | T F G C V B |
| meteorite | Q A | W S Z | E D C X | R T F G V B |

The presets retain right-hand assignments: index Y U H J N M, middle I K comma,
ring O L period, pinky P semicolon slash. Outer columns remain pinky; thumbs are
unchanged. Metrics, travel, usage, and finger tints follow physical assignments.
Terminal-cell rounding affects drawing only. These names implement the
definitions above; they do not promise compatibility with every analyzer's
similarly named preset.

## Compact magic rules

Put `@`, `*`, or `◇` in a physical slot, then define its table:

```text
@ 'r ay hr ik jo ke rl u' ye
```

Each two-character token is previous **text character + new output**. After h,
pressing this key emits r. Apostrophes are literal characters, not quotes.
Compact contexts/outputs are lowercased; use explicit rules for uppercase output.

`@`, `*`, and `◇` repeat remembered output when no rule matches.
`magic @ ...`, `magic * ...`, and `magic ◇ ...` are equivalent spellings.
Other ASCII symbol keys can use `magic ! ay hr` if `!` occupies a slot.
Diamond is an action label, not Unicode output. The repeat fallback is enforced
for bound text-based magic keys in DAT, JSON, and JSONC, including explicit
definitions that request a different fallback. Listed rules still take priority,
including a listed `none` output. Adaptive ordinary keys retain their literal
fallback.

Repeat availability does not force the evaluator to choose that key: current
effort weights and literal-first ties still determine physical typing choices.

Existing shorthand such as `w@ wh`, `n@ n'`, and `a@ aa` remains supported.
Only the new suffix is emitted by the action. Do not mix old shorthand and a
new compact table for the same key.

## Adaptive keys and swaps

`adaptive n hr ay` makes physical n emit r after h, y after a, and n otherwise.
`swap h nr` exchanges n/r outputs after h; outside that context they are literal.
It expands to `adaptive n hr` plus `adaptive r hn`.

```text
swap ' lf j nu u eo y ,u
```

This is equivalent to `swap ' lf`, `swap j nu`, `swap u eo`, and `swap y ,u` on
separate lines. Pairs have exactly two characters; `,/u` is not valid syntax.
The older multiple-pair form `swap h nr ae` and longer context `swap th nr` also
work. If all tokens after the first context have length two, the old form takes
precedence. Use separate lines for groups with multi-character contexts.

Adaptive keys can be ASCII letters, digits, or punctuation already on the board,
including comma. `@`, `~`, and `|` are reserved; `=` and double quote require
explicit named actions. Distinct contexts may be added across lines; duplicate
contexts are errors. Longest matching text suffix wins. A context of length L
requires a corpus order greater than L, up to the supported maximum order 5.

These rules change the next key's output; they do not rewrite preceding text,
implement firmware timing, or permanently swap slots. During optimization the
binding and its rules move together. Swap partners are not locked together.
Named actions start locked when refining a layout; click to unlock them.
Random/evolve generation starts with only Space locked.

## Explicit actions

Place `@name` in a slot and define it below the keyboard:

```text
action name = magic
map name "h" = "r"
map name "th" = "e"
fallback name = repeat-output
```

Emissions are a quoted ASCII string, `none`, `@another_action`, `repeat-output`,
or `repeat-action` (`repeat`/`again` are their aliases). Explicit rules can extend
compact tables using `map`/`fallback`; do not add a second `action` declaration
for the same compact table.

| Action kind | Context or behavior |
|---|---|
| `magic` | Longest matching suffix of emitted text. |
| `press-magic` | Previous physical press's binding identity. |
| `skip-magic` | Binding identity two physical presses back. |
| `output-magic` | Output of the previous press. |
| `skip-output-magic` | Output two presses back. |
| `alternate` / `alt-repeat` | Remembered output. |
| `repeat-output` / `repeat` | Emit remembered output again. |
| `repeat-action` / `again` | Resolve the remembered physical key again in the current context. |
| `inactive` | No output. |
| `text "..."` | Fixed output; multi-character macros are trace-only. |

The enforced repeat fallback applies to text-based `magic` actions bound to
physical keys. Other action kinds and unbound helper actions retain their
declared fallbacks; adaptive keys retain their ordinary output.

Ordinary emissions update remembered output/key; repeat operations preserve that
memory. Nested calls are resolution machinery, not extra physical presses.
Unsuccessful or losing action attempts do not enter physical history. Unsupported
characters reset semantic and physical history. Static call cycles are rejected;
dynamic cycles/depth limits are guarded at resolution.

Cached evaluation supports one output byte per press and rejects multi-character
macros. It chooses the lowest immediate effort, with literal-first ties. Effort
uses current weights, so changing weights can also change physical typing choices
for action layouts. Evaluation is a bounded-context estimate; optional n-gram
limits additionally reduce its available history. See [corpora and limits](USAGE.md#corpora).
Saving keeps meaning, using compact syntax where possible and explicit rules for
longer contexts, calls, `none`, or custom fallbacks.
