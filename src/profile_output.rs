//! Keep profiler output away from the live raw-mode terminal.
use std::io::{self, IsTerminal, Write};
use std::sync::Mutex;

static OUTPUT: Mutex<ReportOutput> = Mutex::new(ReportOutput {
    terminal_sessions: 0,
    pending: Vec::new(),
});

struct ReportOutput {
    terminal_sessions: usize,
    pending: Vec<u8>,
}

impl ReportOutput {
    fn write(&mut self, report: &[u8], terminal: bool, out: &mut impl Write) {
        if terminal && self.terminal_sessions > 0 {
            self.pending.extend_from_slice(report);
        } else {
            let _ = out.write_all(report);
        }
    }

    fn finish_session(&mut self, out: &mut impl Write) {
        self.terminal_sessions -= 1;
        if self.terminal_sessions == 0 && !self.pending.is_empty() {
            let _ = out.write_all(&self.pending);
            let _ = out.flush();
            self.pending = Vec::new();
        }
    }
}

/// Hold from before raw mode begins until after terminal restoration.
pub(crate) struct TerminalReports;

impl TerminalReports {
    pub(crate) fn begin() -> Self {
        let mut output = OUTPUT.lock().unwrap_or_else(|error| error.into_inner());
        output.terminal_sessions += 1;
        Self
    }
}

impl Drop for TerminalReports {
    fn drop(&mut self) {
        let mut output = OUTPUT.lock().unwrap_or_else(|error| error.into_inner());
        output.finish_session(&mut io::stderr().lock());
    }
}

/// Redirected stderr remains immediate; terminal output waits for TUI exit.
pub(crate) fn write_report(report: &[u8]) {
    let stderr = io::stderr();
    let terminal = stderr.is_terminal();
    let mut output = OUTPUT.lock().unwrap_or_else(|error| error.into_inner());
    output.write(report, terminal, &mut stderr.lock());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(sessions: usize) -> ReportOutput {
        ReportOutput {
            terminal_sessions: sessions,
            pending: Vec::new(),
        }
    }

    #[test]
    fn terminal_reports_wait_until_restoration_and_keep_order() {
        let mut state = output(1);
        let mut written = Vec::new();
        state.write(b"load: 1 s\n", true, &mut written);
        state.write(b"search: 2 s\n", true, &mut written);
        assert!(written.is_empty());

        state.finish_session(&mut written);
        assert_eq!(written.as_slice(), b"load: 1 s\nsearch: 2 s\n");
        assert!(state.pending.is_empty());

        state.terminal_sessions += 1;
        state.finish_session(&mut written);
        assert_eq!(written.as_slice(), b"load: 1 s\nsearch: 2 s\n");
    }

    #[test]
    fn redirected_reports_are_immediate_during_tui() {
        let mut state = output(1);
        let mut written = Vec::new();
        state.write(b"load: 1 s\n", false, &mut written);
        assert_eq!(written.as_slice(), b"load: 1 s\n");
        assert!(state.pending.is_empty());
    }

    #[test]
    fn cli_reports_are_immediate() {
        let mut state = output(0);
        let mut written = Vec::new();
        state.write(b"load: 1 s\n", true, &mut written);
        assert_eq!(written.as_slice(), b"load: 1 s\n");
        assert!(state.pending.is_empty());
    }

    #[test]
    fn nested_session_does_not_flush_early() {
        let mut state = output(2);
        let mut written = Vec::new();
        state.write(b"load: 1 s\n", true, &mut written);
        state.finish_session(&mut written);
        assert!(written.is_empty());

        state.finish_session(&mut written);
        assert_eq!(written.as_slice(), b"load: 1 s\n");
    }
}
