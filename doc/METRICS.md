# Metrics and weights

[README](../README.md) · [Usage](USAGE.md) · [Layouts](LAYOUTS.md)

Metrics classify physical key presses. For magic layouts these are the keys
actually pressed, not the characters they emit. Weights live in `[weights]`;
SCORE = penalties − rewards, so lower is better.

## Pair metrics

Bigrams (B) are consecutive presses; skipgrams (S) are presses 1 and 3.

Jump names: `d`/`c` = discordant/concordant, `a`/`n` = adjacent/nonadjacent
fingers, `F`/`H` = full (two-row)/half (one-row) jump. Discordant means the
shorter finger is on the higher row (finger length: pinky < ring < index <
middle). Jumps need different fingers on the same hand and exclude thumbs.

| Metric | Meaning | Weight | Default |
|---|---|---|---:|
| SFB / SFS | Different keys, same finger. | `sfb` / `sfs` | 12 / 1.5 |
| SKB / SKS | Same key. | `skb` / `sks` | 0 / 0 |
| daFJB / daFJS | Discordant full jump, adjacent fingers. | `daFJB` / `daFJS` | 4 / 1.5 |
| caFJB / caFJS | Concordant full jump, adjacent fingers. | `caFJB` / `caFJS` | 2 / 0.75 |
| dnFJB / dnFJS | Discordant full jump, nonadjacent fingers. | `dnFJB` / `dnFJS` | 1 / 0.25 |
| cnFJB / cnFJS | Concordant full jump, nonadjacent fingers. | `cnFJB` / `cnFJS` | 0.5 / 0.15 |
| daHJB / daHJS | Discordant half jump, adjacent fingers. | `daHJB` / `daHJS` | 1 / 0.4 |
| caHJB / caHJS | Concordant half jump, adjacent fingers. | `caHJB` / `caHJS` | 0.5 / 0.2 |
| dnHJB / dnHJS | Discordant half jump, nonadjacent fingers. | `dnHJB` / `dnHJS` | 1 / 0.4 |
| cnHJB / cnHJS | Concordant half jump, nonadjacent fingers. | `cnHJB` / `cnHJS` | 0.5 / 0.2 |
| LSB / LSS | Lateral stretch: horizontal span ≥ finger separation + 1 key. | `lsb` / `lss` | 3 / 0.75 |

Display-only columns (no weight): FSB/FSS = daFJB/daFJS, HSB/HSS =
daHJB/daHJS, and FJB/FJS = the sum of the four full-jump kinds. Don't add
them to the weighted metrics.

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
whose consecutive pairs have a jump / stretch.

## Travel and finger use

Travel is in key units per 100 presses (or bigrams for SFTRAVEL). Thumbs add
no travel.

| Metric | Meaning | Weight | Default |
|---|---|---|---:|
| TRAVEL | Distance from each key to its finger's home key. | `travel` | 0.02 |
| VTRAVEL / LTRAVEL | Vertical / horizontal part of TRAVEL. | `vtravel` / `ltravel` | 0 / 0 |
| SFTRAVEL | Distance between consecutive same-finger presses. | `sftravel` | 0.10 |
| Usage | Share of presses per finger. | `usage_pinky`, `usage_ring`, `usage_middle`, `usage_index` | 0.06, 0.02, 0.005, 0 |
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
