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
