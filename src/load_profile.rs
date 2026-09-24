//! Coarse operation-scoped loading timings; no item/context-loop hooks.
use std::{
    io::Write,
    sync::OnceLock,
    time::{Duration, Instant},
};

static ENABLED: OnceLock<bool> = OnceLock::new();

pub(crate) struct LoadProfile {
    operation: &'static str,
    start: Option<Instant>,
    last: Option<Instant>,
    phases: [(&'static str, Duration); 16],
    count: usize,
}

impl LoadProfile {
    pub(crate) fn new(operation: &'static str) -> Self {
        let enabled =
            *ENABLED.get_or_init(|| std::env::var("LAYOUTER_PROFILE_LOAD").as_deref() == Ok("1"));
        Self::with_enabled(operation, enabled)
    }

    fn with_enabled(operation: &'static str, enabled: bool) -> Self {
        let start = if enabled { Some(Instant::now()) } else { None };

        Self {
            operation,
            start,
            last: start,
            phases: [("", Duration::ZERO); 16],
            count: 0,
        }
    }

    pub(crate) fn mark(&mut self, name: &'static str) {
        if let Some(last) = self.last {
            let now = Instant::now();
            if self.count < self.phases.len() {
                self.phases[self.count] = (name, now.duration_since(last));
                self.count += 1;
                self.last = Some(now);
            }
        }
    }
}

impl Drop for LoadProfile {
    fn drop(&mut self) {
        let Some(start) = self.start else {
            return;
        };

        self.mark("Remaining / partial on error");
        let total = self.last.unwrap().duration_since(start);
        let mut out = Vec::new();
        let _ = writeln!(
            out,
            "\nLoad profile: {} (operation scope; nested reports overlap)",
            self.operation
        );
        for &(name, d) in &self.phases[..self.count] {
            let _ = writeln!(out, "  {name}: {:.6} s", d.as_secs_f64());
        }
        let _ = writeln!(out, "  Total: {:.6} s", total.as_secs_f64());
        crate::profile_output::write_report(&out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_profile_records_nothing() {
        let mut p = LoadProfile::with_enabled("disabled test", false);
        p.mark("phase");
        assert!(p.start.is_none());
        assert!(p.last.is_none());
        assert_eq!(p.count, 0);
    }
}
