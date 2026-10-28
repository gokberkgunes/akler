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
the later rule. A rule is skipped where a key its `_` matches already uses, next
to it, the finger the rule assigns: `e_␣ (LT RT)` leaves Space on its own finger
in "er␣" when r is on RT. A file without `alts` may instead keep rules in a leading
comment block, as stray's notes do:

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

Magic and action evaluation applies these rules exactly but runs about three
times slower. The ordinary model applies them only within each bigram,
skipgram or trigram, so it misses some same-finger pairs (about 15% of SFB on
stray). Finger usage still counts the normal finger.

`v` in the editor (`a` in the ranker) lists a layout's alternate fingers,
combos and magic rules.
