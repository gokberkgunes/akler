# Layout files

[README](../README.md) · [Metrics](METRICS.md) · [Usage](USAGE.md)

AKLER reads Mana JSON/JSONC and Spark `spark/1` files, detected from the
contents. Saving writes `.jsonc`; comments and formatting are not kept.
Unsupported features (layers, tap-hold, split angle) are errors.

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
  offset per column. Offsets are in key units (positive = right / down) and
  affect travel and stretches; jumps use logical rows.
- In `magic`, `*` is the wildcard: `*@ → **` makes `@` repeat the previous
  output, and later rules like `h@ → hr` override it. `_` is one press in
  between, so `*_$ → *_*` repeats the output from two presses back. `call`
  runs another magic key. `char:@` is a literal reserved symbol.
- Top-level `alts` and `combos` arrays: see [ALTFINGERS.md](ALTFINGERS.md)
  and [COMBOS.md](COMBOS.md).

## Adaptive keys and swaps

A rule whose input ends in a letter key makes that key adaptive:

```jsonc
"magic": {
  "rules": [
    {"inputs": "hn", "output": "hr"},  // n types r after h
    {"inputs": "hr", "output": "hn"},  // with the rule above: after h, n and r swap
  ],
}
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

When a literal key and a magic key can type the same character, evaluation
picks the one that avoids a same-finger press, otherwise the cheaper one.
Saving reports an error if JSONC can't represent a layout.
