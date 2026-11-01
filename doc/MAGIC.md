# Magic keys

[README](../README.md) · [Layouts](LAYOUTS.md) · [Usage](USAGE.md)

## Magic in the optimizer

In optimizer setup, press `m` to add a magic key: pick a key with the mouse
or `h`/`j`/`k`/`l` and press Enter. Its letter moves to an added outer slot.
The search then moves letters and refits the magic rules together, scored
with the current mode (`normal` or `aklgg`). An unlocked magic key can move.
Caps stay relative to the original layout.

Magic, Swaps, and Alts each take a row under the board, green while on and
red while off; click a row to turn it on or off (Magic: `m`). In optimizer
setup and results, click a row's `[↻ reset]` button to refresh its rules in
memory. Magic
keeps its physical key, fallback, and existing rule contexts: `___` marks each
output queued for refitting. The working outputs stay active until replacements
are evaluated. Press Space to refit outputs in those same slots; rules are
not deleted to make a smaller dictionary. If a search fails, the working
magic outputs remain visible and the error is shown separately. Changing the
Magic rules setting releases the retained slots. Swaps restores the affected keys' literal outputs and
refits according to the current adaptive-swap limit.
Clearing Alts removes the current layout's and session's configured alternate
rules; alternate fingers are configured manually and are not fitted.

The **Mirror** button, at the board's right edge, reflects the layout left to right, including
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

## Magic tab (magic designer)

The optimizer's **5 Magic** tab searches magic-key positions, rules and the
letters around the key, and keeps a table of candidates. **Magic designer** on
the home menu and `g` in the weakness report open the optimizer on this tab; in
the plain optimizer, `5` converts the layout and opens it.

The search uses the optimizer's own layout, locks, corpus, weights, caps,
workers and **Magic rules** limit:

- A locked magic key keeps its key. An unlocked one may move to any unlocked
  key whose finger is not blocked (`p`); without a magic key, the search adds
  one on an outer key first.
- Locked letters never move, and the magic key never takes a locked key.
- Adaptive swaps are set aside during the search and come back on the
  applied candidate.

The table has one row per magic-key position: the best layout found with the
key there, whether or not it beats the start, so a locked key gives one row.
**Positions shown** caps the rows. Start is the layout searched, with its own
magic keys and without its swaps; values are green where better than Start.

Enter on a candidate makes it the optimizer's layout, with the magic key's
lock moving with it. Space on another tab then runs the optimizer, which fits
adaptive swaps and combos as **Settings** says.

| Key | Action |
|---|---|
| Space | Search |
| `j` / `k` | Select a setting or candidate |
| Enter | Edit the setting / apply the candidate |
| Click a key | Lock or unlock it, as on the overview |
| `H` / `U` / `L` | Default locks plus the home row / unlock all / lock all |
| `p` | Choose which fingers may hold the magic key |
| `c` / `v` | A candidate's metrics / its rules and their effects |
| `s` / `S` | Save the candidate to `layouts/` / save every candidate |
| `o` | Open saved candidates, rescored with this corpus and these weights |
| `1`–`4` | Other optimizer tabs |

Each pass tries magic positions, fits rules, optimizes the letters around the
key and refits. A rule is kept only if it fixes a real movement problem (SFB,
same key, jump, stretch, same-finger travel) and lowers the score. **Rules
gain** compares a candidate with its rules removed.

`S` writes `layouts/magic/<layout>-NNN/` with `table.tsv`, `original.jsonc`,
one `candidate-NNN.jsonc` per row and `manifest.jsonc`.
