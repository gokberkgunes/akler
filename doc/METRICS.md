# Metrics and weights

[README](../README.md) · [Layouts](LAYOUTS.md) · [Usage and settings](USAGE.md)

Metrics classify **physical key positions**. For action layouts these are the
presses that win typing selection, including the root magic/adaptive key;
emitted characters and nested calls are not extra presses. Action skipgrams
compare the first and third presses of a mapped three-press sequence; gaps
reset that sequence.

Ordinary evaluation uses stored text n-grams mapped to physical positions.
Its skipgrams require only their endpoints: `a!b` contributes `a…b` when `a`
and `b` are available, even if `!` is not. It does not create adjacent `ab`.
This endpoint rule applies to every ordinary skip metric and its denominator.
See [corpus behavior](USAGE.md#corpora) for the distinction between evaluators.

## Pair metrics

`B` denotes consecutive presses; `S` denotes skipgram endpoints. D/C means
discordant/concordant geometry, not the temporal direction of typing.
Discordant means the shorter finger is on the higher row; concordant means the
longer finger is on the higher row. The model's finger-length ordering is
pinky < ring < index < middle; this is a modeling choice.

| Metric | Meaning | Weight key | Default |
|---|---|---|---:|
| SFB | Different keys on the same finger; excludes same-key repeats. | `sfb` | 12 |
| SKB | Same physical key twice in a row. | `skb` | 0 |
| SFS | Different keys on the same finger at the skipgram endpoints. | `sfs` | 1.5 |
| SKS | Same physical key at both skipgram endpoints. | `sks` | 0 |
| FSB / FSS | Full scissors: adjacent fingers on the same hand, two rows apart. Totals of their D/C children. | Display totals only | — |
| DFSB / DFSS | Discordant full scissors. | `dfsb` / `dfss` | 4 / 1.5 |
| CFSB / CFSS | Concordant full scissors. | `cfsb` / `cfss` | 2 / 0.75 |
| HSB / HSS | Half scissors: adjacent fingers, one row apart, discordant only. | `hsb` / `hss` | 1 / 0.4 |
| LSB / LSS | Lateral stretch: same hand, different fingers; horizontal span is at least finger-rank separation + 1 key unit. | `lsb` / `lss` | 3 / 0.75 |
| DSB / DSS | Other discordant two-row changes: non-adjacent fingers on the same hand. Excludes full scissors. | `dsb` / `dss` | 1 / 0.25 |
| CSB / CSS | Other concordant two-row changes: non-adjacent fingers on the same hand. Excludes full scissors. | `csb` / `css` | 0.5 / 0.15 |

Scissors/stretch/row-change categories exclude thumbs. SFB/SFS use physical
finger identity, so different physical keys assigned to the same thumb can count.
SKB/SKS use physical key identity and include thumb keys.

Scissors, one/two-row changes, and same-row preferences use logical keyboard
rows. Numeric horizontal row offsets and vertical column offsets do not change
those row categories. Lateral stretch uses physical horizontal coordinates;
travel uses physical coordinates on both axes, in key units.

**Do not sum every displayed column.** FSB = DFSB + CFSB, and FSS = DFSS + CFSS;
the totals add no extra score penalty. Other two-row changes and full scissors
are disjoint by finger adjacency, but lateral stretch can overlap either.
Different metrics also use different denominators.

## Triple metrics and preferences

Rhythm metrics exclude thumbs by default; `[rolls] include_thumbs` can include
them specifically in roll metrics. A *clean* SRAF/ALT pattern has no SFB/SFS,
SKB/SKS, scissors, lateral stretch, other two-row change, or redirect across
AB, BC, and skip AC.

| Metric | Meaning | Weight key | Default |
|---|---|---|---:|
| RED | Three presses on one hand; finger direction reverses, with both steps nonzero. | `red` | 0.75 |
| WRED | RED with no index finger. Subset of RED, carrying an additional penalty. | `wred` | 2.5 |
| WISH | RED with an index finger and at least one ring/pinky. Subset of RED. | `wish` | 1.25 |
| SRAF | Clean same-row adjacent-finger pair on one hand. | `sraf_reward` | 0.25 |
| ROLL | IN2 + OUT2 + IN3 + OUT3; combined roll credit. | `roll_reward` | 0.25 |
| INROLL / OUTROLL | IN2 + IN3 / OUT2 + OUT3. | Display only | — |
| IN2 / OUT2 | Mixed-hand trigram (LLR/RRL/LRR/RLL); its consecutive same-hand pair moves inward / outward on different fingers. | Display only | — |
| IN3 / OUT3 | Three distinct fingers on one hand, moving strictly inward / outward. | Display only | — |
| ALT | Clean LRL/RLR triple; the returning AC pair uses different fingers. | `alt_reward` | 0.05 |

L/R means left/right hand. **All four roll types are trigram metrics.** The
“2” means two fingers on one hand within a three-press sequence, not a bigram
percentage. Inward runs pinky → ring → middle → index; outward reverses that
order, mirrored on the right hand. Fingers need not be adjacent.

These are the basic Mana2 finger-direction categories. **Thumbs are excluded
by default**, and can be included with `[rolls] include_thumbs = true`.
They exclude repeated fingers, redirects and LRL/RLR alternation. By default,
scissors, stretches, row changes and off-home placement do not veto a roll.
Set `include_scissors = false` and/or `include_stretches = false` to filter
consecutive pairs AB and BC using akler's movement definitions. Skip AC is
not a veto. Both detailed and simple mode use the selected filters; neither
adds an extra row-change filter. Other penalties are still counted normally.
This optional scissors-plus-stretch filter is stricter in scope than Mana2's
`IsGoodRoll`, which only calls its scissor classifier. The geometry classifiers
also differ between the applications.

For a QWERTY fingermap, `asj` is IN2, `saj` is OUT2, `asd` is IN3 and `dsa`
is OUT3. With thumbs excluded, thumb-space followed by `th` is not a roll;
the thumb-containing trigram is discarded, without joining surrounding presses
across the thumb. With thumbs enabled, thumb ranks inward of index and its
physical hand participates in the direction test.
An action key uses the finger of its winning physical slot, not its output.

ROLL = INROLL + OUTROLL. Each roll belongs to exactly one of IN2/OUT2/IN3/OUT3.
The six breakdown columns are display-only: `roll_reward` rewards the combined
ROLL percentage once. Existing configurations using ROLL remain supported.
The definition change intentionally changes roll credit and may change scores
and optimized layouts; weights themselves are unchanged.

SRAF remains a pair metric. SRAF/ALT details can show `clean`, `raw` (shape
before blockers), or `rejected` (raw minus clean); their scoring remains clean
credit. Roll details show the directional physical trigrams directly.

## Travel, usage, and score

| Field | Meaning | Weight key | Default |
|---|---|---|---:|
| TRAVEL | Euclidean distance from each main-finger key to that finger's home position. | `travel` | 0.02 |
| VTRAVEL | Vertical component of that home distance. | `vtravel` | 0 |
| LTRAVEL | Horizontal component of that home distance, including numeric row offsets and preset stagger. | `ltravel` | 0 |
| SFTRAVEL | Euclidean distance between consecutive main-finger presses using the same finger. | `sftravel` | 0.10 |
| Usage | Presses assigned to each finger. | None | — |
| Off | Main-finger presses outside the eight home positions. | `off_pinky`, `off_ring`, `off_middle`, `off_index` | 0.60, 0.20, 0.05, 0.1 |

Travel uses **key units per 100 events** (`u/100`), not percent. TRAVEL/VTRAVEL/
LTRAVEL measure distance from home, not a reconstructed hand trajectory. Thumb
presses add no travel here. SFTRAVEL's same-key repeats have zero distance.
The off-home weight combines the left and right finger of each type.

Finger labels are LP/LR/LM/LI (left pinky/ring/middle/index), RI/RM/RR/RP
(right index/middle/ring/pinky), and LT/RT (thumbs). Keyboard gray tints identify
fingers, not usage intensity.

Normalization uses supported physical events:

- Bigram penalties (including SKB) and SFTRAVEL: all mapped bigrams, including thumbs.
- Skip penalties (including SKS): all mapped skipgrams, including thumbs.
- SRAF: mapped non-thumb bigrams; blocked pairs stay in the denominator.
- RED/WRED/WISH and ALT: mapped non-thumb trigrams; nonmatching and blocked
  triples stay in the denominator.
- All roll types/totals: mapped non-thumb trigrams by default, or all mapped
  trigrams with `include_thumbs = true`. Rolls rejected by movement filters
  stay in the denominator. All roll columns share this denominator; none
  divides by only inward rolls, only good rolls, or only rolls.
- Usage, off-home, and home-travel metrics: all mapped presses, including thumbs.

Each percentage is 100 × weighted event count / its denominator. An empty
denominator gives zero. Context frequencies weight events; distinct sequences
do not each count equally. Raw totals are unnormalized frequency sums.

`Penalty` sums positive weighted terms; `Credit` sums weighted preferences;
`Objective`/`SCORE` = Penalty − Credit, so lower is better. In detailed mode each
metric value is multiplied by its configured weight. Overlapping penalties
are intentional; this is not a partition of all typing into exclusive classes.

`Before`/`After` are the corresponding baseline/current metric values.
Physical detail rows show contributions in the metric's units; their changes
are not automatically multiplied by objective weights. `Coverage` in ordinary
ranking is mapped bigram mass / stored source bigram mass, not raw-text coverage.
Action rows show coverage as unavailable and report physical presses/ignored
characters in their inspector instead of substituting a different denominator.

## Simple mode: what 1u and 2u meant

The old “1u/2u jump” wording referred to **one-row/two-row separation**, not
Euclidean movement length: home↔top/bottom versus top↔bottom. The settings are
`row1` and `row2`. These include all same-hand, different-finger pairs with that
row separation, so they include scissors and the corresponding other-row
changes. They must not be added as extra columns to a detailed scissors total.

Numeric geometry does not turn these logical row changes into distance bins.
Simple mode uses only seven terms, configured in `[search]` in `akler.conf`:

| Setting | Meaning | Default |
|---|---|---:|
| `simple_sfb` | SFB penalty. | 12 |
| `simple_sfs` | SFS penalty. | 4 |
| `simple_lateral` | Combined bigram/skipgram lateral-stretch penalty. | 2 |
| `simple_row1` | Combined one-row-change penalty. | 0.75 |
| `simple_row2` | Combined two-row-change penalty. | 2 |
| `simple_sraf_reward` | Clean SRAF credit. | 0.25 |
| `simple_roll_reward` | Combined ROLL credit, using the same `[rolls]` filters as detailed mode. | 0.25 |

Combined pair/skip terms divide their summed counts by bigram + skipgram mass.
Simple mode does not add detailed scissors, SKB/SKS, travel, or off-home weights.
Both optimizers support simple mode. The ranker always uses detailed weights;
its SCORE need not equal a simple-mode or mixed-corpus search objective. See
[settings](USAGE.md#search-settings).

## Weight configuration

The `[weights]` section of `akler.conf` accepts `name = nonnegative_number`
and `#` comments. Omitted entries retain defaults; duplicate/unknown names are
errors. Values must be finite and at most 1,000,000. Lowercase keys are listed
above. See [configuration and migration](USAGE.md#configuration).

Legacy `fsb`/`fss` entries are accepted for migration: they set the discordant
child to that value and concordant child to half, unless those children are
explicitly set. The aggregate itself is never weighted. Column visibility
does not edit weights, caps, or metric calculations.
