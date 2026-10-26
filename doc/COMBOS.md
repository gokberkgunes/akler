# Combos

A combo types its output by pressing two keys at once: `a+b → je` types "je"
on the a and b keys. Combos apply while `[typing] combos = true`; press `F`
to toggle.

```json
"combos": [{"keys":["a","u"],"output":"au"}]
```

DAT uses the same JSON on one line after `combos:`. The `inputs` form also
works: `{"inputs":["au"],"output":"au"}`.

## Scoring

A combo is one chord press:

- both keys add finger usage;
- movement into and out of the chord counts once per key (in "pil" with
  `r+y → pi`, both r→l and y→l count);
- there is no movement between the chord's own keys.

The longest matching output wins; ties go to the later definition. Combos
need the corpus cached through 4-grams (5-grams for full accuracy).

## Fitting

The magic and action optimizers fit up to `[combos] max` combos. A fitted
combo types a costly bigram on two neighbouring fingers in the same row
(with `fatfingering = true`, also on two neighbouring keys of one finger).

```ini
[combos]
fatfingering = false
max = 4         # 0 leaves combos as they are
exact = false   # true fits exactly max
shortlist = 12  # candidates given exact trials per round
```

Each round tries removing each combo and adding each candidate, keeps the
best improvement, and stops when nothing improves. Combos are refitted after
every annealing chunk. For a plain layout, press `c` in the optimizer to
open the action optimizer.

Combos roughly double search time.
