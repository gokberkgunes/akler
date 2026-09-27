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

In jump names, the lowercase prefix uses `d`/`c` for discordant/concordant
geometry and `a`/`n` for adjacent/nonadjacent fingers. The uppercase suffix
uses `F`/`H` for a full (two-row)/half (one-row) jump, `J` for jump, and `B`/`S`
for bigram/skipgram. Discordant/concordant describes geometry, not typing
direction. Config keys accept either case.
Discordant means the shorter finger is on the higher row; concordant means the
longer finger is on the higher row. The model's finger-length ordering is
pinky < ring < index < middle; this is a modeling choice.

| Metric | Meaning | Weight key | Default |
|---|---|---|---:|
| SFB | Different keys on the same finger; excludes same-key repeats. | `sfb` | 12 |
| SKB | Same physical key twice in a row. | `skb` | 0 |
| SFS | Different keys on the same finger at the skipgram endpoints. | `sfs` | 1.5 |
| SKS | Same physical key at both skipgram endpoints. | `sks` | 0 |
| FSB / FSS | Adjacent-finger, discordant two-row jumps. Display-only aliases of daFJB / daFJS. | Display only; old `fsb` / `fss` migrate | — |
| HSB / HSS | Adjacent-finger, discordant one-row jumps. Display-only aliases of daHJB / daHJS. | Display only; old `hsb` / `hss` migrate | — |
| daFJB / daFJS | Discordant two-row jumps between adjacent fingers. | `daFJB` / `daFJS` | 4 / 1.5 |
| caFJB / caFJS | Concordant two-row jumps between adjacent fingers. | `caFJB` / `caFJS` | 2 / 0.75 |
| daHJB / daHJS | Discordant one-row jumps between adjacent fingers. | `daHJB` / `daHJS` | 1 / 0.4 |
| caHJB / caHJS | Concordant one-row jumps between adjacent fingers. | `caHJB` / `caHJS` | 0.5 / 0.2 |
| dnHJB / dnHJS | Discordant one-row jumps between nonadjacent fingers. | `dnHJB` / `dnHJS` | 1 / 0.4 |
| cnHJB / cnHJS | Concordant one-row jumps between nonadjacent fingers. | `cnHJB` / `cnHJS` | 0.5 / 0.2 |
| LSB / LSS | Lateral stretch: same hand, different fingers; horizontal span is at least finger-rank separation + 1 key unit. | `lsb` / `lss` | 3 / 0.75 |
| dnFJB / dnFJS | Discordant two-row jumps between nonadjacent fingers. | `dnFJB` / `dnFJS` | 1 / 0.25 |
| cnFJB / cnFJS | Concordant two-row jumps between nonadjacent fingers. | `cnFJB` / `cnFJS` | 0.5 / 0.15 |

Jump categories require different fingers on the same hand. Jump, scissors,
stretch, and row-change categories exclude thumbs. SFB/SFS use physical
finger identity, so different physical keys assigned to the same thumb can count.
SKB/SKS use physical key identity and include thumb keys.

Jumps, scissors, and same-row preferences use logical keyboard
rows. Numeric horizontal row offsets and vertical column offsets do not change
those row categories. Lateral stretch uses physical horizontal coordinates;
travel uses physical coordinates on both axes, in key units.

**Do not sum every displayed column.** FSB/FSS duplicate daFJB/daFJS, and
HSB/HSS duplicate daHJB/daHJS. Adjacent and nonadjacent jump categories are
disjoint and each has its own weight. Lateral stretch may overlap a jump.
Different metrics also use different denominators.

Old `dfab`, `cfab`, `dfas`, `cfas`, `dfjb`, `cfjb`, `dfjs`, `cfjs`,
`dfsb`, `cfsb`, `dfss`, and `cfss` weight keys are accepted as aliases for
the corresponding adjacent full-jump keys. Old `fsb`/`fss` values seed the
discordant and concordant full-jump weights using the previous migration rule;
old `hsb`/`hss` values seed discordant half jumps and half-sized concordant
half jumps across both finger distances. Old `dhjb`, `chjb`, `dhjs`, and `chjs` values
seed both adjacent and nonadjacent versions of their half-jump metric. Old
`dsb`, `dss`, `csb`, `css`, `dfnb`, `dfns`, `cfnb`, and
`cfns` keys map to the nonadjacent full-jump weights. Explicit new jump keys
take precedence.
Old ranker columns `DHJB`, `CHJB`, `DHJS`, and `CHJS` show both corresponding
adjacent and nonadjacent columns.

## Triple metrics and preferences

Rhythm metrics exclude thumbs by default; `[rolls] include_thumbs` can include
them specifically in roll metrics. A *clean* SRAF/ALT pattern has no SFB/SFS,
SKB/SKS, jumps, lateral stretch, or redirect across
AB, BC, and skip AC.

| Metric | Meaning | Weight key | Default |
|---|---|---|---:|
| RED | Three presses on one hand; finger direction reverses, with both steps nonzero. | `red` | 0.75 |
| WRED | RED with no index finger. Subset of RED, carrying an additional penalty. | `wred` | 2.5 |
| WISH | RED with an index finger and at least one ring/pinky. Subset of RED. | `wish` | 1.25 |
| SRAF | All clean same-row adjacent-finger pairs on one hand: INSRAF + OUTSRAF. | Display total only | — |
| INSRAF / OUTSRAF | Clean SRAF moving inward / outward. | `insraf_reward` / `outsraf_reward` | 0.25 / 0.25 |
| ROLL | IN2 + OUT2 + IN3 + OUT3; combined roll total. | Display total only | — |
| INROLL / OUTROLL | IN2 + IN3 / OUT2 + OUT3. | `inroll` / `outroll` | 0.25 / 0.25 |
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
The four type columns are display-only. INROLL and OUTROLL have independent
weights, while combined ROLL remains an unweighted display total.

SRAF remains a pair metric. INSRAF moves from pinky toward index and OUTSRAF
moves from index toward pinky, using the same anatomical order on each hand;
physical left-to-right direction is irrelevant. SRAF is their unweighted
display total. SRAF/ALT details can show `clean`, `raw` (shape before blockers),
or `rejected` (raw minus clean); directional SRAF scoring uses clean credit.
Roll details show the directional physical trigrams directly.

## Travel, usage, and score

| Field | Meaning | Weight key | Default |
|---|---|---|---:|
| TRAVEL | Euclidean distance from each main-finger key to that finger's home position. | `travel` | 0.02 |
| VTRAVEL | Vertical component of that home distance. | `vtravel` | 0 |
| LTRAVEL | Horizontal component of that home distance, including numeric row offsets and preset stagger. | `ltravel` | 0 |
| SFTRAVEL | Euclidean distance between consecutive main-finger presses using the same finger. | `sftravel` | 0.10 |
| Usage | Share of all mapped presses assigned to each finger. Left/right pairs are penalized by type. | `usage_pinky`, `usage_ring`, `usage_middle`, `usage_index` | 0.06, 0.02, 0.005, 0 |
| Off | Main-finger presses outside the eight home positions. | `off_pinky`, `off_ring`, `off_middle`, `off_index` | 0.60, 0.20, 0.05, 0 |

Travel uses **key units per 100 events** (`u/100`), not percent. TRAVEL/VTRAVEL/
LTRAVEL measure distance from home, not a reconstructed hand trajectory. Thumb
presses add no travel here. SFTRAVEL's same-key repeats have zero distance.
Usage and off-home weights each combine the left and right finger of a type.
Usage includes home and off-home presses; the two penalties therefore add.
Thumb usage is displayed but has no usage weight. The built-in usage weights
are one tenth of the corresponding off-home weights; `akler.conf` uses the
same ratio.
Each main finger's home position comes from the home-row finger map. JSONC rows
must list the same number of slots; use `skip` to show empty positions.

Finger labels are LP/LR/LM/LI (left pinky/ring/middle/index), RI/RM/RR/RP
(right index/middle/ring/pinky), and LT/RT (thumbs). Keyboard gray tints identify
fingers, not usage intensity.

Normalization uses supported physical events:

- Bigram penalties (including SKB) and SFTRAVEL: all mapped bigrams, including thumbs.
- Skip penalties (including SKS): all mapped skipgrams, including thumbs.
- SRAF, INSRAF, and OUTSRAF: mapped non-thumb bigrams; blocked pairs stay in
  their shared denominator.
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
Simple mode uses nine terms, configured in `[search]` in `akler.conf`:

| Setting | Meaning | Default |
|---|---|---:|
| `simple_sfb` | SFB penalty. | 12 |
| `simple_sfs` | SFS penalty. | 4 |
| `simple_lateral` | Combined bigram/skipgram lateral-stretch penalty. | 2 |
| `simple_row1` | Combined one-row-change penalty. | 0.75 |
| `simple_row2` | Combined two-row-change penalty. | 2 |
| `simple_insraf_reward` | Clean inward SRAF credit. | 0.25 |
| `simple_outsraf_reward` | Clean outward SRAF credit. | 0.25 |
| `simple_inroll_reward` | Inward roll credit, using the detailed mode `[rolls]` filters. | 0.25 |
| `simple_outroll_reward` | Outward roll credit, using the detailed mode `[rolls]` filters. | 0.25 |

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

Legacy `sraf_reward` and `roll_reward` entries set both of their directional
children, unless a corresponding child is explicitly configured. Likewise,
legacy `simple_sraf_reward` and `simple_roll_reward` set both simple-mode
directions. Equal inward and outward weights reproduce the old combined reward;
the directional keys allow the optimizer to prefer one direction.
