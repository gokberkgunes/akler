//! Opt-in search instrumentation. Const parameters remove hot-loop hooks when off.
use std::io::{self, Write};

use std::time::{Duration, Instant};

#[derive(Default, Debug, PartialEq, Eq)]
pub(crate) struct MappingCounts {
    pub resolutions: u64,
    pub recursive: u64,
    pub suffix_checks: u64,
    pub effort_lookups: u64,
    pub effort_comparisons: u64,
    pub suffix_index_lookups: u64,
    pub terminal_shortcuts: u64,
    pub positions_processed: u64,
    pub positions_reused: u64,
    pub action_attempts: u64,
    pub action_no_output: u64,
    pub action_mismatch: u64,
    pub action_matches: u64,
    pub action_effort_losses: u64,
    pub action_winners: u64,
    pub action_nonfinite_effort: u64,
    pub text_one: u64,
    pub text_longer: u64,
    // Per physical attempt: root TextOne selected a repeat terminal (1),
    // another emission (2), or was not TextOne (0). Nested calls never set it.
    pub text_one_selection: usize,
    // Outcome columns: no output, target mismatch, target match.
    pub text_one_repeat_outcomes: [u64; 3],
    pub text_one_other_outcomes: [u64; 3],
    pub terminal_none: u64,
    pub terminal_byte: u64,
    pub terminal_repeat: u64,
    pub terminal_depth_rejected: u64,
}

#[derive(Default)]
pub(crate) struct Profile {
    pub total: Duration,
    pub setup: Duration,
    pub affected: Duration,
    pub contexts: Duration,
    pub score: Duration,
    pub checks: Duration,
    pub commit: Duration,
    pub rebase: Duration,
    pub named_time: Duration,
    pub mapping: Duration,
    pub contributions: Duration,
    pub attempted: u64,
    pub candidates: u64,
    pub accepted: u64,
    pub rejected: u64,
    pub named: u64,
    pub literals: u64,
    pub other_candidates: u64,
    pub full_scans: u64,
    pub cache_size: u64,
    pub affected_total: u64,
    pub affected_max: u64,
    pub unaffected: u64,
    pub mapped: u64,
    pub changed: u64,
    pub rebases: u64,
    pub ops: MappingCounts,
}

#[inline]
pub(crate) fn clock<const ON: bool>() -> Option<Instant> {
    if ON {
        Some(Instant::now())
    } else {
        None
    }
}

#[inline]
pub(crate) fn elapsed<const ON: bool>(start: Option<Instant>) -> Duration {
    if ON {
        start.expect("profiling clock").elapsed()
    } else {
        Duration::ZERO
    }
}

impl Profile {
    fn accounted(&self) -> Duration {
        self.setup
            + self.affected
            + self.contexts
            + self.score
            + self.checks
            + self.commit
            + self.rebase
    }

    pub(crate) fn report(
        &self,
        out: &mut impl Write,
        status: &str,
        detailed: bool,
    ) -> io::Result<()> {
        let seconds = self.total.as_secs_f64();
        let percent = |d: Duration| {
            if seconds > 0.0 {
                100.0 * d.as_secs_f64() / seconds
            } else {
                0.0
            }
        };
        let average = |n: u64, d: u64| if d > 0 { n as f64 / d as f64 } else { 0.0 };
        writeln!(
            out,
            "\nMagic optimizer profile ({status}; level {})",
            if detailed { 2 } else { 1 }
        )?;
        writeln!(
            out,
            "Total search: {:.3} s (worker wall time; excludes editor/final report)",
            seconds
        )?;
        writeln!(
            out,
            "Candidates: {} evaluated / {} attempted; accepted: {}; rejected: {}; incomplete: {}",
            self.candidates,
            self.attempted,
            self.accepted,
            self.rejected,
            self.attempted.saturating_sub(self.candidates)
        )?;
        writeln!(
            out,
            "Candidate types (attempts): named-action {}; literal/literal {}; other {}",
            self.named, self.literals, self.other_candidates
        )?;
        for (name, d) in [
            ("Setup/cache initialization", self.setup),
            ("Affected-list construction", self.affected),
            ("Context mapping/scoring", self.contexts),
            ("CandidateScore normalization", self.score),
            ("Cap/improvement checks", self.checks),
            ("Commit (excluding rebase)", self.commit),
            ("Periodic rebase", self.rebase),
            (
                "Other/unaccounted",
                self.total.saturating_sub(self.accounted()),
            ),
        ] {
            writeln!(out, "{name}: {:.3} s ({:.1}%)", d.as_secs_f64(), percent(d))?;
        }
        writeln!(
            out,
            "Contexts: {} mapped; {} scheduled; {} unaffected/skipped; cache {}",
            self.mapped, self.affected_total, self.unaffected, self.cache_size
        )?;
        // Scheduled averages use candidates reaching list construction (full scans included).
        let listed = self.attempted;
        writeln!(
            out,
            "Avg contexts/candidate: {:.1}; max {}; avg cache affected: {:.2}%",
            average(self.affected_total, listed),
            self.affected_max,
            if self.cache_size > 0 {
                100.0 * average(self.affected_total, listed) / self.cache_size as f64
            } else {
                0.0
            }
        )?;
        writeln!(
            out,
            "Context mapping/scoring average: {:.1} ns/mapped context",
            if self.mapped > 0 {
                self.contexts.as_secs_f64() * 1e9 / self.mapped as f64
            } else {
                0.0
            }
        )?;
        writeln!(
            out,
            "Named-action full scans: {}; candidate time {:.3} s ({:.1}%; OVERLAPS phases above)",
            self.full_scans,
            self.named_time.as_secs_f64(),
            percent(self.named_time)
        )?;
        writeln!(
            out,
            "Changed context tails: {}; rebases: {}",
            self.changed, self.rebases
        )?;
        writeln!(out, "Operations (candidate mapping only): action resolutions {}; recursive {}; suffix checks {}; effort lookups {}; effort comparisons {}",
            self.ops.resolutions, self.ops.recursive, self.ops.suffix_checks, self.ops.effort_lookups, self.ops.effort_comparisons)?;
        writeln!(out, "Suffix-index lookups {}; terminal-call shortcuts {} (suffix checks now count longer packed comparisons)", self.ops.suffix_index_lookups, self.ops.terminal_shortcuts)?;
        writeln!(out, "Mapper positions: newly processed {}; prefix reused {} (reused positions perform no action attempts)", self.ops.positions_processed, self.ops.positions_reused)?;
        writeln!(
            out,
            "Action-key attempts: {}; no output {}; target mismatch {}; target match {}",
            self.ops.action_attempts,
            self.ops.action_no_output,
            self.ops.action_mismatch,
            self.ops.action_matches
        )?;
        writeln!(out, "Matching action outcomes: effort loss {} (includes ties and displaced leaders); final winner {}; non-finite effort {}", self.ops.action_effort_losses, self.ops.action_winners, self.ops.action_nonfinite_effort)?;
        writeln!(out, "Text resolution paths: TextOne {}; longer-suffix index {} (includes nested resolutions; not extra action attempts)", self.ops.text_one, self.ops.text_longer)?;
        for (name, counts) in [
            (
                "TextOne -> repeat-output terminal",
                self.ops.text_one_repeat_outcomes,
            ),
            (
                "TextOne -> other selected emission",
                self.ops.text_one_other_outcomes,
            ),
        ] {
            writeln!(
                out,
                "{name}: no output {}; target mismatch {}; target match {}",
                counts[0], counts[1], counts[2]
            )?;
        }
        writeln!(out, "TextOne cross-counts classify root table selections; nested calls do not reclassify an attempt. Matches are counted before effort selection.")?;
        writeln!(out, "Terminal shortcuts by type: none {}; byte {}; repeat-output {}; depth rejected {} (subset of types)", self.ops.terminal_none, self.ops.terminal_byte, self.ops.terminal_repeat, self.ops.terminal_depth_rejected)?;
        writeln!(out, "Outcome counts cover attempted physical action keys, not recursive calls. On mapping error, the unfinished position may have an unclassified leader.")?;
        if detailed {
            writeln!(out, "Nested context timers (NOT additive to phases): mapping {:.3} s; contribution updates {:.3} s", self.mapping.as_secs_f64(), self.contributions.as_secs_f64())?;
            writeln!(out, "Mapping alone average: {:.1} ns/context; per-context clock overhead is included in context phase", if self.mapped>0 {
                self.mapping.as_secs_f64()*1e9 / self.mapped as f64
            } else {
                0.0
            })?;
        } else {
            writeln!(out, "Mapping/contribution split not timed at level 1; use AKLER_PROFILE=2. Action/suffix/effort internals are counts only.")?;
        }
        writeln!(out, "Phases sum to total before rounding. UI runs concurrently; other is worker overhead, not UI CPU time.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn phase_accounting_and_empty_report_are_well_defined() {
        let mut profile = Profile::default();
        profile.total = Duration::from_secs(10);
        profile.setup = Duration::from_secs(1);
        profile.contexts = Duration::from_secs(5);
        profile.commit = Duration::from_secs(1);
        profile.rebase = Duration::from_secs(1);
        profile.named_time = Duration::from_secs(6);
        // Overlapping subset, not added.
        assert_eq!(profile.accounted(), Duration::from_secs(8));
        let mut out = Vec::new();
        profile.report(&mut out, "completed", false).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("Other/unaccounted: 2.000 s (20.0%)"));
        assert!(text.contains("OVERLAPS"));
        let mut empty = Vec::new();
        Profile::default()
            .report(&mut empty, "cancelled/partial", true)
            .unwrap();
        let text = String::from_utf8(empty).unwrap();
        assert!(!text.contains("NaN"));
        assert!(!text.contains("inf"));
        assert!(clock::<false>().is_none());
    }
}
