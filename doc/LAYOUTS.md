# Layout files

[README](../README.md) · [Metrics](METRICS.md) · [Usage](USAGE.md)

AKLER reads DAT, Mana JSON/JSONC and Spark `spark/1` files, detected from the
contents. Saving writes both `.dat` and `.jsonc` with the same stem; comments
and formatting are not kept. Unsupported features (layers, tap-hold, split
angle) are errors.

## DAT

```text
q w e r t | y u i o p
a s d f g | h j k l ;
z x c v b | n m , . /
thumbs: space
```

- Rows are 5|5, 6|5, 5|6 or 6|6 keys; `~` is empty, `space` is Space.
- `thumbs: LEFT RIGHT` places thumb keys; use `|` between hands for several
  keys (`thumbs: none | l r`). Space is added to a free thumb if missing.
- `outer-left:` / `outer-right: TOP HOME BOTTOM` add an outer pinky column.
- `char:~` or `char:|` writes a literal symbol.

### Geometry and fingers

```text
row-stagger: standard
row-offsets: 0 0.25 0.75
column-offsets: 0 -0.2 -0.3 0 0 0 0 -0.3 -0.2 0
fingermap: LP LR LM LI LI RI RI RM RR RP / ... / ...
```

Offsets are in key units (positive = right / down) and affect travel and
stretches; jumps use logical rows. `row-stagger` presets are `standard`,
`anglemod`, `nokwts`, `meteorite` or `off`. Presets set row offsets −0.25,
0, 0.5 and their left-hand finger assignments (anglemod moves QWERTY Z X C
one finger right, nokwts puts R on the middle finger, meteorite puts Z on
ring and X C on middle). `row-offsets` and `fingermap` override a preset.

## Mana JSONC

```jsonc
{
  "layout": {
    "fingers": [
      "q w e r t y u i o p",
      "a s d f g h j k @ $",
      "z x c v b n m , . /",
    ],
    "thumbs": ["space", ""],
  },
  "fingermap": [
    "0 1 2 3 3 6 6 7 8 9",
    "0 1 2 3 3 6 6 7 8 9",
    "0 1 2 3 3 6 6 7 8 9",
  ],
  "board": { "isRowStaggered": true, "rowOrColumnStagger": [0, 0.25, 0.75] },
  "magic": {
    "keys": "@$",
    "wildcards": "*",
    "rules": [
      {"inputs": "*@", "output": "**"},
      {"inputs": "h@", "output": "hr"},
      {"inputs": "*_$", "output": "*_*"},
      {"inputs": "@_$", "call": "@"},
    ],
  },
}
```

- Rows have 10–12 slots; `skip` is empty. `thumbs` lists left then right;
  `"none"` disables the default Space.
- Finger IDs: 0–3 = LP LR LM LI, 6–9 = RI RM RR RP.
- `isRowStaggered` gives three row offsets; `isColumnStaggered` one vertical
  offset per column.
- In `magic`, `*` is the wildcard: `*@ → **` makes `@` repeat the previous
  output, and later rules like `h@ → hr` override it. `_` is one press in
  between, so `*_$ → *_*` repeats the output from two presses back. `call`
  runs another magic key. `char:@` is a literal reserved symbol.
- Top-level `alts` and `combos` arrays: see [ALTFINGERS.md](ALTFINGERS.md)
  and [COMBOS.md](COMBOS.md).

## Magic keys in DAT

Put `@`, `*` or `◇` in a slot and list its rules:

```text
@ 'r ay hr ik jo ke rl u' ye
```

Each pair is previous character + output: after h, the key types r. Without
a matching rule the key repeats the previous output. `magic ! ay hr` uses
another symbol key.

## Adaptive keys and swaps

```text
adaptive n hr ay   # n types r after h, y after a
swap h nr          # after h, n and r exchange outputs
swap ' lf j nu     # several swaps on one line
```

"After h" means after the h key is pressed. Contexts can be up to four
characters long; the longest match wins.

### Fitting swaps

The magic and action optimizers fit up to `[adaptive] max` swaps (0 leaves a
layout's swaps alone; `exact = true` fits exactly `max`). Candidates come
from same-finger bigrams "C X": swapping X with Y on another finger types X
on Y's key after C. The best `shortlist` candidates get exact trials, and
swaps are refitted after every annealing chunk. Results show as
`Swaps: u:eq` (after u, e and q exchange).

## Explicit actions

```text
action name = magic
map name "h" = "r"
fallback name = repeat-output
```

Place `@name` in a slot. Outputs: a quoted string, `none`, `@other`,
`repeat-output`, `repeat-previous-output` or `repeat-action`.

| Kind | Context |
|---|---|
| `magic` | Longest matching suffix of typed text. |
| `press-magic` / `skip-magic` | Key pressed one / two presses back. |
| `output-magic` / `skip-output-magic` | Output one / two presses back. |
| `alternate`, `repeat-output`, `repeat-previous-output`, `repeat-action` | Repeat remembered output or key. |
| `inactive` | No output. |

When a literal key and a magic key can type the same character, evaluation
picks the one that avoids a same-finger press, otherwise the cheaper one.
Every action saves to DAT; JSONC reports an error if it can't represent one.
