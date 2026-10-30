# Code review — 2026-10-28

Findings reviewed against commit `264452b`. Line numbers refer to that
revision; function names are included to help locate code after changes.

## 1. Alternate fingers can drop a physical press from the counts

**Status: fixed in the current working tree.** The final context now counts
the preceding key, including its resolved finger. Regression tests cover
sequence boundaries, unmapped keys, and capped contexts. After the fix, the
release suite had 331 passing tests and the same two failures listed below.

**Priority: P1.** At the reviewed revision, shifted unigram accounting in `chord_grams` at [action_ngrams.rs:3187](../src/action_ngrams.rs#L3187) did not restore the penultimate key at the end of a sequence. With a QWERTY layout, an order-5 corpus containing `asdf`, no combos, and the enabled position rule `q_w (LP LR)` (which never matches), evaluation reported four presses but only three physical unigrams: `d` disappeared. Finger usage, travel, and their denominators were affected.

## 2. Optimizer limits use approximate metrics as hard checks

**Status: fixed in the current working tree.** The optimizer checks caps and
reports live metrics using full-context counts, while retaining the faster
approximate objective for screening. Regression tests cover hard limits,
joint-search penalties, live statistics, and 16-worker searches.
On the local `esp-r` layout and Reddit corpus, using `bench/akler.conf` with
alternate fingers enabled, the 16-worker benchmark fell from 33,985 to 27,329
trials/s without caps (three-run medians), and from 36,874 to 14,628 trials/s
with SFB/SFS caps (two-run medians). With the workspace's `akler.conf`, where
alternate fingers are disabled and caps are enabled, a longer four-run
comparison measured 110,464 versus 110,461 trials/s (16 workers, 12,000
annealing steps, one sweep, fitting disabled).
With adaptive fitting enabled as configured (up to five swaps, 64 restarts,
160 passes, and 12,000 steps), three-run medians were 82,460 before and 83,499
after; every run made 473,465 trials and reached the same score.

**Priority: P1.** At the reviewed revision, the search selected reduced alternate-finger contexts in `Incremental::new_with_policy` at [action_ngrams.rs:4937](../src/action_ngrams.rs#L4937), then checked limits against those counts at [action_search.rs:525](../src/action_search.rs#L525). With `a_s (LP LM)` and `aqsvd` repeated, starting with `q` and `e` swapped and swapping them back appeared as SFS **0% → 0%** to the search but **0% → 20%** in full evaluation. A zero-increase limit could admit that move. Live optimizer statistics also used the approximate counters.

## 3. Some alternate rules are invisible to bigram scoring

**Status: partly fixed in the current working tree.** The reported `g_x`
case now uses two-key lookahead when retained contexts and uniquely mapped
keys determine the correction safely. Backward-looking rules, physical counts,
cached corpora, and incremental swaps have regression coverage. When both
ends depend on keys outside the same five-gram, the evaluator retains its
history-based estimate: five-gram frequencies alone cannot distinguish every
such case. Ambiguous action mappings and contexts discarded by corpus limits
also remain estimates.

On the local `esp-r` layout and Reddit corpus, three paired 16-worker runs
using the workspace configuration measured 85,474 versus 83,005 trials/s
before and after this change. With alternate fingers enabled and SFB/SFS
caps, the corresponding medians were 13,866 versus 13,705 trials/s. The
benchmark scores and trial counts matched in each comparison.

**Priority: P1.** At the reviewed revision, the main context windows had only one following character at [action_ngrams.rs:1267](../src/action_ngrams.rs#L1267). For `g_x (LM LR)` in `afgvx`, the full text assigns `g` to LM, but the `fg` bigram was counted before `x` was visible and assigned `g` to LI. Repeating that text yielded **18.75% SFB** even though the effective fingers for `f` and `g` differ.

## 4. Mapping reuse can keep counts from the wrong context mode

**Priority: P1.** `Mappings::reuse` at [action_ngrams.rs:2990](../src/action_ngrams.rs#L2990) treats adding or removing a combo as a local change, although it also switches how alternate-finger contexts are counted. Adding an unused `q+p → qp` combo to the preceding example gave **66.67% REDIR** from a fresh evaluation and **33.33%** from reused counts. This can mis-score fitting candidates.

## 5. An unused combo changes fresh evaluation results

**Priority: P2.** `Program::for_contexts` at [combos.rs:282](../src/combos.rs#L282) selects the combo context path whenever any combo is declared, even when its output never occurs. For the `g_x` / `afgvx` example, enabling an unused `q+p → qp` combo changed fresh REDIR from **33.33% to 66.67%**. Alternate-finger context treatment needs to remain consistent when combos are enabled.

## 6. Alternate-finger evaluation bypasses the trigram limit

**Priority: P2.** The filter in `build_chord_windows` at [action_ngrams.rs:1349](../src/action_ngrams.rs#L1349) retains every three-character context in the alternate-finger path. With `trigrams = 1`, an unmatched alternate rule, and corpus `asdf asdf qwer qwer jkl; jkl;`, evaluation counted **17 physical trigrams**, while evaluation with alternates disabled retained one and the warning reported one retained sequence.

## 7. Rule ordering can defeat precedence and the clash check

**Priority: P2.** `apply_rule` at [alt_fingers.rs:149](../src/alt_fingers.rs#L149) checks proposed neighboring fingers before applying per-key priority. For overlapping `as (LM LR)` and `asf (. LM .)` rules, reversing their order changes the result; one order leaves both `a` and `s` on LM despite the clash rule. The check must account for assignments that actually win priority.

## 8. Ranker benchmarks fail when a corpus is selected by name

**Priority: P2.** `ranker_command` at [bench.rs:832](../src/bench.rs#L832) can resolve a name to its raw text file and pass it directly to the JSON ranker loader. With both raw and processed `e200` present, `akler bench --ranker e200` fails with `expected '{' at byte 0`; passing the processed JSON path works.

## Review validation

`cargo test --release -- --test-threads=1` finished with **329 passed and 2 failed**. `action_ngrams::tests::reused_mappings_match_fresh_evaluations` demonstrates finding 4. `tests::alt_rules_name_qwerty_positions_and_pick_the_rules_by_stagger` has an expectation that appears outdated after the latest rule change. The remaining findings were reproduced with isolated probes.

After fixing findings 1 and 2 and adding regression tests, the release suite
finished with **337 passed and the same 2 failures**. The full-context cache's
finger usage and physical score were also checked against fresh evaluation.
After the finding 3 correction, a release run finished with **343 passed and
the same 2 failures**; subsequent targeted regression tests also passed.
