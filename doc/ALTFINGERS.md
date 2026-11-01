# Alternate fingers

An alternate-finger rule presses a key with a different finger in a given
context. The key and its geometry stay put. Rules apply while
`[typing] alt_fingers = true`; press `f` to toggle.

## Layout rules

The same rule form works in Spark, JSONC (top-level `alts` array) and DAT
(one-line `alts:`):

```json
"alts": [{"ngram":["p",null,"g"],"fingers":["LM",null,"LI"]}]
```

```text
alts: [{"ngram":["p",null,"g"],"fingers":["LM",null,"LI"]}]
```

`ngram` is two or three characters; `null` matches any character or keeps the
normal finger. Longer rules win, then rules with more literal characters, then
the later rule. As on akl.gg, a rule is skipped where a key it moves would land
on the finger of the key next to it: with r on RT, `e␣ (LT RT)` leaves Space on
LT in "e␣r", and `e_␣` does in "er␣". A three-key rule also checks the keys two
away outside it, so `e_␣` leaves Space on LT in "ey␣ar". A key pressed by two
different fingers, like Space on either thumb, is no same-key repeat. A file
without `alts` may instead keep
rules in a leading comment block, as stray's notes do:

```jsonc
// alt fingers
//   sg s_g (LM LI) · gs g_s (LI LM)
//   e␣ e_␣ (LT RT) · *e *_e (RT LT)
//
```

`␣` names Space, and thumb keys may take `LT` or `RT`. A magic key is named
by its label (`*`).

## Position alternates

`[alts]` in `akler.conf` gives rules by key position, so they stay put while
the optimizer moves letters. Positions are named by the QWERTY key in that
place; `_` is any one key in between.

```ini
[alts]
colstag = fg f_g fb f_b (LM LI) gf g_f bf b_f (LI LM)
rowstag = rt r_t (LM LI)
```

`rt r_t (LM LI)` presses the key under QWERTY r with the left middle finger
and t with the left index whenever r is followed by t, adjacent or one key
apart. `.` keeps a key's normal finger. Row-staggered boards use `rowstag`;
all others use `colstag`. A layout's own `alts` take precedence.

Magic and action evaluation uses centered contexts when the corpus has
four-grams or better. For bigrams, it also uses longer contexts to recognize
two-key lookahead when their key mappings and alternate rules make the result
unambiguous. It retains the history-based estimate when the available context
cannot determine both fingers. In particular, two ends can depend on a span
of six or more keys, which a five-gram corpus cannot fully represent.
A skipgram is counted in a context that ends on its last key, so a key two
after it that blocks a move goes unseen there; five-key contexts correct such
skipgrams when every other key in them keeps its own finger. On monkeyracer
this puts akl.gg's espr at 4.11% SFS against akl.gg's 4.12% (4.05% before),
and checking limits takes about 8% longer.
Optimizer searches use a faster, approximate context for their objective
score. They check limits and report statistics using the same contexts as
evaluation, including these limits on the available information. When every
key a rule names or moves is locked, and no free key sits on a finger a rule
moves keys onto (thumb rules on locked thumbs, as in esp-r), the rules give
every arrangement the same fingers: the evaluation contexts then differ from
the search's by a constant, which the search measures once and checks limits
with, confirming it exactly at the end of each restart. Capped esp-r runs on
monkeyracer go from 12k to 36k trials/s this way. Rules on keys the search
moves, magic keys and combos keep the exact check on every kept move. In
every search, a context whose keys cannot cover all the keys some rule
names, before or after a move, keeps its keys' own fingers, so the move
only relabels it; that makes lyrebird's capped runs about a quarter faster.
Without combos, and while at most half the contexts could have a rule read
them, the search also moves contexts in groups sharing their last three keys,
as without alternate fingers, and maps only the possible readers one by one:
lyrebird and yeast search 60% faster, esp-r 30%.

Under `[ngrams]` limits, a context the limits dropped falls back to a shorter
one, which can hide the key a rule reads: a dropped `asfg` loses the `a` that
`a_f` needs. Alternate fingers therefore keep up to six times as many four-
and five-key contexts as the limits, the most frequent first, as long as
every trigram they count is one the trigram limit kept. On reddit with
5000/5000/5000 limits this brings espr's SFB from 0.505% to 0.341%, against
0.335% with no limits; capped optimizer runs check their limits more slowly.
Layouts with combos and alternate fingers use the same retained contexts, so
a combo no longer changes how fingers are counted elsewhere. A combo whose
output the corpus never types changes nothing at all.

The ordinary model applies them only within each bigram, skipgram or trigram,
so it misses some same-finger pairs (about 15% of SFB on stray). Finger usage
still counts the normal finger.

`v` in the editor (`a` in the ranker) lists a layout's alternate fingers,
combos and magic rules.
