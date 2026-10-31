# Magic keys

[README](../README.md) · [Layouts](LAYOUTS.md) · [Usage](USAGE.md)

## Magic in the optimizer

In optimizer setup, press `m` to add a magic key: pick a key with the mouse
or `h`/`j`/`k`/`l` and press Enter. Its letter moves to an added outer slot.
The search then moves letters and refits the magic rules together, scored
with the current mode (`normal` or `aklgg`). An unlocked magic key can move.
Caps stay relative to the original layout.

Magic, Swaps, and Alts occupy three rows in one shared reset box. In optimizer setup and
results, click a line's `↻` button to refresh its rules in memory. Magic
keeps its physical key, fallback, and existing rule contexts: `___` marks each
output queued for refitting. The working outputs stay active until replacements
are evaluated. Press Space to refit outputs in those same slots; rules are
not deleted to make a smaller dictionary. If a search fails, the working
magic outputs remain visible and the error is shown separately. Changing the
Magic rules setting releases the retained slots. Swaps restores the affected keys' literal outputs and
refits according to the current adaptive-swap limit.
Clearing Alts removes the current layout's and session's configured alternate
rules; alternate fingers are configured manually and are not fitted.

The adjacent **Mirror** button reflects the layout left to right, including
thumbs. Locks stay at their physical positions; layout-owned alternate-finger
assignments switch hands along with their keys. Click Mirror again to undo it.

```ini
[magic]
allowed_fingers = LM LI RI RM LT RT   # where a magic key may go
allowed_keys = all                    # or exact row:column slots, e.g. 1:3 2:4
max_rules = 5                         # 1-68 rules per key
shortlist = 24                        # proposals given exact trials per round
```

Plain repeats such as `e → e` don't count toward `max_rules`. A rule that is
the only way to type a character is always kept. In results, `v` shows the
rules; in setup, `m` removes the magic key again.

## Magic designer

Open **Magic designer** from the home menu, or press `g` in the weakness
report. It searches magic-key locations and rules for one layout and keeps a
table of candidates, always using `normal` weights.

| Key | Action |
|---|---|
| `1` / `2` | Design / Candidates tab |
| `p` | Choose which fingers may hold the magic key |
| `3`–`6` | Passes, rule limit, shortlist, archive size |
| Space / `r` | Start search (Design) / refine candidate (Candidates) |
| Enter | Edit setting / inspect a candidate's rules |
| `c` | Full metrics for a candidate |
| `s` / `S` | Save the candidate to `layouts/` / save the whole table |
| `o` | Open a saved table |

Each pass tries magic locations, fits rules, optimizes the letters around
the key and refits. A rule is kept only if it fixes a real movement problem
(SFB, same key, jump, stretch, same-finger travel) and lowers the score.
**Rules gain** compares a candidate with its rules removed.

`S` writes `layouts/magic/<layout>-NNN/` with `table.tsv`, `original.jsonc`,
one `candidate-NNN.jsonc` per row and `manifest.jsonc`.
