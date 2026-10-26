# AKL.GG stat set

Set `mode = aklgg` (or press `t` in the optimizer, `s` in the ranker) to score
with the stats from akl.gg's tables, weighted by `[aklgg]`. Negative weights
penalize, positive weights reward, 0 only reports.

| Group | Stats |
|---|---|
| Bigram / skipgram | Same Finger, Same Key, Stretch, Scissor |
| Trigram (all / no thumbs) | Alternation, Redirect, Weak redirect, each with "& SFS"; Roll, Inroll2/3, Outroll2/3 |
| Per finger | Usage, Fspeed, Weighted speed |

Stretch and Scissor are Mana2's ratings, not AKLER's jump metrics. Fspeed is
a movement index, not real speed; Weighted divides it by finger strength.
`weighted_speed` is the total akl.gg optimizes.

Extra Mana2 stats not in the table (`offpinky`, `pinkyringcurl`, `sfbw`,
`sfsw`, `goodroll`) and per-finger `fspeed-XX` / `weighted-speed-XX` default
to 0. Any weight can be a schedule `[slope, boundary, slope, ...]`:
`finger-usage-LP = [0, 6, -4]` is free up to 6% of presses, then −4 per
extra percent. Without a usage or `offpinky` weight, the optimizer loads each
pinky like an index finger.

`akler eval LAYOUT CORPUS` prints every value under `"Mana"`. With
`[ngrams] trigrams = all` they match akl.gg to its displayed precision. One
difference: akler counts a magic key's default repeat, which akl.gg ignores.
