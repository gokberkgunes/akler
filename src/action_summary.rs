//! Ranking totals without retaining the detailed physical histogram adapter.
use crate::action_keys as ak;

use crate::action_ngrams as ng;

use crate::*;

pub(crate) struct Summary {
    pub(crate) raw: Raw,
    pub(crate) totals: [f64; 4],
    pub(crate) metrics: Metrics,
    pub(crate) score: f64,
}

pub(crate) fn evaluate(
    layout: &ak::Layout,
    corpus: &ng::NgramCorpus,
    weights: &Weights,
    stop: &AtomicBool,
    progress: &AtomicU64,
) -> ak::Result<Summary> {
    let mut timing = load_profile::LoadProfile::new("action ranking summary");
    let effort = action_ui::LocalEffort::new(layout, weights);
    timing.mark("Physical effort tables");

    let counts = corpus.evaluate(layout, stop, progress, |a, b, key| effort.get(a, b, key))?;
    timing.mark("Physical mapping and transient histograms (nested report)");

    let summary = from_counts(layout, &counts, weights)?;
    timing.mark("Physical geometry, raw metrics and score");
    Ok(summary)
}

fn from_counts(layout: &ak::Layout, counts: &ng::Counts, weights: &Weights) -> ak::Result<Summary> {
    if counts.presses == 0.0 {
        return Err("no decoded keypresses".into());
    }

    let n = layout.slots.len();
    // Match the detailed evaluator's supported key count and error. Its ASCII
    // surrogate labels are assigned in increasing physical-slot order, so its
    // canonical IDs and original positions are both the identity permutation.
    let adapter_capacity = (33u8..=126)
        .filter(|byte| !byte.is_ascii_alphabetic())
        .count();
    if n > adapter_capacity {
        return Err("too many keys for the existing metric adapter".into());
    }

    let geometry = Geometry::with_rolls(action_ui::physical_keys(layout), weights.rolls());
    let positions: Vec<usize> = (0..n).collect();
    let physical = [
        &counts.tables[0],
        &counts.tables[1],
        &counts.skip,
        &counts.tables[2],
    ];
    let mut raw = Raw::default();
    let mut totals = [0.0; 4];

    // Keep the detailed adapter's kind order, lexicographic physical-key order,
    // zero-frequency filtering, and add_gram arithmetic. Accumulating straight
    // from mapped contexts would change floating-point summation order.
    for (kind, table) in physical.into_iter().enumerate() {
        for (keys, &frequency) in table {
            if frequency == 0.0 {
                continue;
            }

            totals[kind] += frequency;
            let mut ids = [0; 3];
            ids[..keys.len()].copy_from_slice(keys);
            add_gram(
                &mut raw,
                &Gram {
                    ids,
                    len: keys.len(),
                    kind,
                    f: frequency,
                },
                &positions,
                &geometry,
                1.0,
            );
        }
    }

    if totals[0] <= 0.0 || totals[1] <= 0.0 {
        return Err("layout has no usable letters/bigrams in this corpus".into());
    }

    let metrics = metrics_totals(&raw, &totals);
    let score = breakdown(&metrics, weights).net;
    Ok(Summary {
        raw,
        totals,
        metrics,
        score,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus(order: usize) -> ng::NgramCorpus {
        let tables = ak::text_ngrams(b"i'i'a quu qqqqu qxuqu hnr thqe h!nr qu\0qu aan aap y,u ");
        let names = ["letters", "bigrams", "trigrams", "fourgrams", "fivegrams"];
        let fields: Vec<_> = (0..order)
            .map(|index| {
                let entries: Vec<_> = tables[index]
                    .iter()
                    .map(|(gram, count)| {
                        // Decimal fractions exercise the adapter's exact summation order.
                        format!("{}:{}", ak::quote(gram), *count as f64 * 0.17)
                    })
                    .collect();
                format!("\"{}\":{{{}}}", names[index], entries.join(","))
            })
            .collect();
        ng::NgramCorpus::from_text(
            &format!("{{{}}}", fields.join(",")),
            Path::new("summary-inline.json"),
        )
        .unwrap()
    }

    #[test]
    fn summary_matches_detailed_metrics_and_score_bit_for_bit() {
        let stop = AtomicBool::new(false);
        let progress = AtomicU64::new(0);
        for source in crate::action_fast::tests::fixtures() {
            for order in 3..=5 {
                let corpus = corpus(order);
                let mut layout =
                    ak::Layout::parse(&source, Path::new("summary-inline.dat")).unwrap();
                for swap in [None, Some((0, layout.slots.len() - 1)), Some((2, 17))] {
                    if let Some((a, b)) = swap {
                        layout.swap(a, b);
                    }
                    for weights in [Weights::default(), Weights::new([0.0; N_WEIGHTS])] {
                        let summary =
                            evaluate(&layout, &corpus, &weights, &stop, &progress).unwrap();
                        let detailed = action_ui::evaluate_progress(
                            &layout, &corpus, &weights, &stop, &progress,
                        )
                        .unwrap();

                        assert_eq!(
                            summary.raw.0.map(f64::to_bits),
                            detailed.raw.0.map(f64::to_bits)
                        );
                        assert_eq!(
                            summary.totals.map(f64::to_bits),
                            detailed.corpus.totals.map(f64::to_bits)
                        );
                        assert_eq!(
                            summary.metrics.v.map(f64::to_bits),
                            detailed.metrics.v.map(f64::to_bits)
                        );
                        assert_eq!(
                            summary.metrics.usage.map(f64::to_bits),
                            detailed.metrics.usage.map(f64::to_bits)
                        );
                        assert_eq!(
                            summary.metrics.off.map(f64::to_bits),
                            detailed.metrics.off.map(f64::to_bits)
                        );
                        assert_eq!(
                            summary.metrics.simple.map(f64::to_bits),
                            detailed.metrics.simple.map(f64::to_bits)
                        );
                        assert_eq!(summary.score.to_bits(), detailed.score.to_bits());
                    }
                }
            }
        }
    }

    #[test]
    fn summary_preserves_cancellation_and_empty_corpus_errors() {
        let layout = ak::Layout::parse(
            &crate::action_fast::tests::fixtures()[0],
            Path::new("summary-inline.dat"),
        )
        .unwrap();
        let weights = Weights::default();
        let progress = AtomicU64::new(0);
        for text in [
            r#"{"letters":{"q":1},"bigrams":{},"trigrams":{}}"#,
            r#"{"letters":{"!":1},"bigrams":{},"trigrams":{}}"#,
        ] {
            let corpus =
                ng::NgramCorpus::from_text(text, Path::new("summary-inline.json")).unwrap();
            for cancelled in [false, true] {
                let stop = AtomicBool::new(cancelled);
                let summary = evaluate(&layout, &corpus, &weights, &stop, &progress)
                    .err()
                    .unwrap();
                let detailed =
                    action_ui::evaluate_progress(&layout, &corpus, &weights, &stop, &progress)
                        .err()
                        .unwrap();
                assert_eq!(summary, detailed);
            }
        }
    }
}
