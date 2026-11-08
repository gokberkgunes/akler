# Metrics and weights

[README](../README.md) · [Usage](USAGE.md) · [Layouts](LAYOUTS.md)

Metrics classify physical key presses. For magic layouts these are the keys
actually pressed, not the characters they emit. Weights live in `[weights]`;
SCORE = penalties − rewards, so lower is better.

## Pair metrics

Bigrams (B) are consecutive presses; skipgrams (S) are presses 1 and 3.

Scissor names: `d`/`c` = discordant/concordant, `a`/`n` = adjacent/nonadjacent
fingers, `F`/`H` = full (two or more rows)/half (one-row) scissor. Discordant means the
shorter finger is on the higher row (finger length: pinky < ring < index <
middle). Jumps need different fingers on the same hand and exclude thumbs.

| Metric | Meaning | Weight | Default |
|---|---|---|---:|
| SFB / SFS | Different keys, same finger. | `sfb` / `sfs` | 12 / 1.5 |
| SKB / SKS | Same key. | `skb` / `sks` | 0 / 0 |
| daFSB / daFSS | Discordant full jump, adjacent fingers. | `daFSB` / `daFSS` | 4 / 1.5 |
| caFSB / caFSS | Concordant full jump, adjacent fingers. | `caFSB` / `caFSS` | 2 / 0.75 |
| dnFSB / dnFSS | Discordant full jump, nonadjacent fingers. | `dnFSB` / `dnFSS` | 1 / 0.25 |
| cnFSB / cnFSS | Concordant full jump, nonadjacent fingers. | `cnFSB` / `cnFSS` | 0.5 / 0.15 |
| daHSB / daHSS | Discordant half jump, adjacent fingers. | `daHSB` / `daHSS` | 1 / 0.4 |
| caHSB / caHSS | Concordant half jump, adjacent fingers. | `caHSB` / `caHSS` | 0.5 / 0.2 |
| dnHSB / dnHSS | Discordant half jump, nonadjacent fingers. | `dnHSB` / `dnHSS` | 1 / 0.4 |
| cnHSB / cnHSS | Concordant half jump, nonadjacent fingers. | `cnHSB` / `cnHSS` | 0.5 / 0.2 |
| LSB / LSS | Lateral stretch: horizontal span ≥ finger separation + 1 key. | `lsb` / `lss` | 3 / 0.75 |

Display-only columns (no weight): FSB/FSS = the sum of the four full kinds
and HSB/HSS = the sum of the four half kinds. Don't add them to the weighted
metrics.

### Jump comfort models

FJB/HJB and their skipgram forms FJS/HJS give each pair on one hand, on
different fingers and rows, a cost C in comfort levels above "very easy"
(1 = one level worse). The value is C per 100 bigrams (or skipgrams), and
full (F) means two or more rows. Same-row pairs are left to LSB. Three
models were fitted to rated pairs (`akler-scissor-models-top3.md`):

| Metrics | Model | C |
|---|---|---|
| FJB HJB FJS HJS | v13 | half-scale scissor table + misalignment beyond half a key × (1 + 0.5 rows) + curl landing offset |
| FJB2 HJB2 FJS2 HJS2 | v12 | 0.5 × table + (1.5 F spread + 0.7 crowding) × (1 + 0.5 rows) + 1.25 × curl offset |
| FJB3 HJB3 FJS3 HJS3 | Model B | 0.5 × table + 0.7 × (spread + crowding) |

The table is 5F for full scissors, 5F/8 for discordant halves and 5F/12 for
concordant halves. F is the finger-pair factor: middle–ring and ring–pinky
1, index–middle 0.7, index–ring and middle–pinky 0.4, index–pinky 0.25.
v13 and v12 halve F for the table when the index is involved, and measure
the index from its own column (R–F–V). The hand angle is 20° on
row-staggered boards and 0° otherwise. Weights `fjb` … `hjs3` default to 0;
the fitted models score skipgrams at C/4.

## Trigram metrics

Trigram metrics exclude thumbs (rolls can include them with
`[rolls] include_thumbs`). Inward is pinky → index; outward is the reverse.

| Metric | Meaning | Weight | Default |
|---|---|---|---:|
| RED | One hand, finger direction reverses. | `red` | 0.75 |
| WRED | RED without index finger (added on top of RED). | `wred` | 2.5 |
| WISH | RED with index plus ring/pinky (added on top of RED). | `wish` | 1.25 |
| ALT | Hands alternate: LRL or RLR. | `alt_reward` | 0.05 |
| INROLL / OUTROLL | IN2 + IN3 / OUT2 + OUT3. | `inroll` / `outroll` | 0.25 / 0.25 |
| IN2 / OUT2 | Two-finger roll inside a mixed-hand trigram (`asj` / `saj`). | display | — |
| IN3 / OUT3 | Three-finger one-hand roll (`asd` / `dsa`). | display | — |
| ROLL | INROLL + OUTROLL. | display | — |
| INSRAF / OUTSRAF | Same-row adjacent-finger bigram with no jump or stretch, inward / outward. | `insraf_reward` / `outsraf_reward` | 0.25 / 0.25 |
| SRAF | INSRAF + OUTSRAF. | display | — |

`[rolls] include_scissors = false` / `include_stretches = false` reject rolls
whose consecutive pairs have an adjacent-finger discordant scissor (daFSB,
daHSB) / a stretch.

## Travel and finger use

Travel is in key units per 100 presses (or bigrams for SFTRAVEL). Thumbs add
no travel.

| Metric | Meaning | Weight | Default |
|---|---|---|---:|
| TRAVEL | Distance from each key to its finger's home key. | `travel` | 0.02 |
| VTRAVEL / LTRAVEL | Vertical / horizontal part of TRAVEL. | `vtravel` / `ltravel` | 0 / 0 |
| SFTRAVEL | Distance between consecutive same-finger presses. | `sftravel` | 0.10 |
| Usage | Share of presses per finger. | `usage_pinky`, `usage_ring`, `usage_middle`, `usage_index` | 0.06, 0.02, 0.005, 0 |
| Off-home caps | Each pinky's and each ring finger's share of off-home presses above its cap, in the manner of akl.gg's finger-usage schedules. | `pinky_over` above `pinky_cap`, `ring_over` above `ring_cap` | 0 above 2.5, 0 above 4.5 |
| Off-home | Presses off the finger's home key. | `off_pinky`, `off_ring`, `off_middle`, `off_index` | 0.60, 0.20, 0.05, 0 |

Fingers: LP LR LM LI (left pinky to index), RI RM RR RP, LT RT (thumbs).

## Denominators

Each value is 100 × weighted count / its population:

- Bigram and skip metrics: all bigrams / skipgrams, thumbs included.
- SRAF: non-thumb bigrams.
- RED, ALT: non-thumb trigrams.
- Rolls: non-thumb trigrams, or all trigrams with `include_thumbs`.
- Usage, off-home, travel: all presses.

Metrics overlap on purpose (a stretch can also be a jump; WRED is also RED).
In the ranker, `COVERAGE` is the share of the corpus's bigram mass the layout
can type.
