use indicatif::{ProgressBar, ProgressStyle};
use std::io::Write;
use std::time::{Duration, Instant};

/// Progress reporting per spec section 3.5. All output goes to stderr.
pub trait Progress {
    /// Called per filesystem entry during discovery.
    fn discovery_tick(&mut self, path: &std::path::Path);
    /// Discovery finished; `total` artifacts will be parsed.
    fn begin(&mut self, total: u64);
    fn file_start(&mut self, name: &str);
    fn file_done(&mut self);
    /// Terminal state: "Completed", "Partial", or "Failed".
    fn finish(&mut self, state: &str);
    /// Per-file informational message (suppressed by --quiet). Implementations
    /// must not corrupt an active progress bar.
    fn info(&mut self, msg: &str);
}

/// Interactive-terminal progress: discovery spinner then determinate bar
/// with counts, percentage, elapsed time, and current basename.
pub struct TtyProgress {
    spinner: Option<ProgressBar>,
    bar: Option<ProgressBar>,
}

impl TtyProgress {
    pub fn new() -> Self {
        let spinner = ProgressBar::new_spinner();
        spinner.set_style(ProgressStyle::with_template("{spinner} Discovering... {msg}").unwrap());
        spinner.enable_steady_tick(Duration::from_millis(120));
        Self {
            spinner: Some(spinner),
            bar: None,
        }
    }
}

impl Default for TtyProgress {
    fn default() -> Self {
        Self::new()
    }
}

impl Progress for TtyProgress {
    fn discovery_tick(&mut self, path: &std::path::Path) {
        if let (Some(s), Some(name)) = (&self.spinner, path.file_name()) {
            s.set_message(name.to_string_lossy().into_owned());
        }
    }

    fn begin(&mut self, total: u64) {
        if let Some(s) = self.spinner.take() {
            s.finish_and_clear();
        }
        let bar = ProgressBar::new(total);
        bar.set_style(
            ProgressStyle::with_template(
                "Parsing [{bar:30}] {pos}/{len} ({percent}%) {elapsed} {msg}",
            )
            .unwrap(),
        );
        self.bar = Some(bar);
    }

    fn file_start(&mut self, name: &str) {
        if let Some(b) = &self.bar {
            b.set_message(name.to_string());
        }
    }

    fn file_done(&mut self) {
        if let Some(b) = &self.bar {
            b.inc(1);
        }
    }

    fn finish(&mut self, state: &str) {
        if let Some(b) = self.bar.take() {
            b.finish_with_message(state.to_string());
        }
        if let Some(s) = self.spinner.take() {
            s.finish_with_message(state.to_string());
        }
    }

    fn info(&mut self, msg: &str) {
        if let Some(b) = &self.bar {
            b.println(msg);
        } else if let Some(s) = &self.spinner {
            s.println(msg);
        } else {
            eprintln!("{msg}");
        }
    }
}

/// Non-interactive progress: line-based reports at least every 5% or
/// every 30 seconds during active work.
pub struct LineProgress {
    out: Box<dyn Write + Send>,
    total: u64,
    done: u64,
    last_reported: u64,
    last_time: Instant,
    started: Instant,
}

impl LineProgress {
    pub fn new(out: Box<dyn Write + Send>) -> Self {
        Self {
            out,
            total: 0,
            done: 0,
            last_reported: 0,
            last_time: Instant::now(),
            started: Instant::now(),
        }
    }

    pub fn stderr() -> Self {
        Self::new(Box::new(std::io::stderr()))
    }

    fn report(&mut self, current: &str) {
        let pct = (self.done * 100).checked_div(self.total).unwrap_or(100);
        let _ = writeln!(
            self.out,
            "Parsing: {}/{} ({}%) elapsed {}s {}",
            self.done,
            self.total,
            pct,
            self.started.elapsed().as_secs(),
            current
        );
        self.last_reported = self.done;
        self.last_time = Instant::now();
    }
}

impl Progress for LineProgress {
    fn discovery_tick(&mut self, _path: &std::path::Path) {}

    fn begin(&mut self, total: u64) {
        self.total = total;
        self.started = Instant::now();
        let _ = writeln!(self.out, "Discovery complete: {total} candidate file(s)");
        // Emit 0% baseline so callers see at least 21 lines for 100-file runs.
        self.report("");
    }

    fn file_start(&mut self, name: &str) {
        let five_pct = (self.total / 20).max(1);
        if self.done.saturating_sub(self.last_reported) >= five_pct
            || self.last_time.elapsed() >= Duration::from_secs(30)
        {
            let name = name.to_string();
            self.report(&name);
        }
    }

    fn file_done(&mut self) {
        self.done += 1;
    }

    fn finish(&mut self, state: &str) {
        let done = self.done;
        let total = self.total;
        let _ = writeln!(self.out, "{state}: {done}/{total}");
    }

    fn info(&mut self, msg: &str) {
        let _ = writeln!(self.out, "{msg}");
    }
}

/// Progress sink that discards everything. Used by the orchestrator, which
/// renders its own per-tool aggregate line.
pub struct NullProgress;

impl Progress for NullProgress {
    fn discovery_tick(&mut self, _path: &std::path::Path) {}
    fn begin(&mut self, _total: u64) {}
    fn file_start(&mut self, _name: &str) {}
    fn file_done(&mut self) {}
    fn finish(&mut self, _state: &str) {}
    fn info(&mut self, _msg: &str) {}
}

/// A `Progress` implementation backed by an `indicatif::ProgressBar`.
/// `begin` sets the bar length (the tool's matched-file count); each
/// `file_done` advances it by one. The owner of the bar is responsible for
/// clearing it (`finish_and_clear`) after parsing completes.
pub struct BarProgress {
    bar: indicatif::ProgressBar,
}

impl BarProgress {
    pub fn new(bar: indicatif::ProgressBar) -> Self {
        Self { bar }
    }
}

impl Progress for BarProgress {
    fn discovery_tick(&mut self, _path: &std::path::Path) {}
    fn begin(&mut self, total: u64) {
        self.bar.set_length(total);
        self.bar.set_position(0);
    }
    fn file_start(&mut self, name: &str) {
        self.bar.set_message(name.to_string());
    }
    fn file_done(&mut self) {
        self.bar.inc(1);
    }
    fn finish(&mut self, _state: &str) {}
    fn info(&mut self, _msg: &str) {}
}

/// Pick the progress style for the current terminal.
pub fn auto() -> Box<dyn Progress> {
    use std::io::IsTerminal;
    if std::io::stderr().is_terminal() {
        Box::new(TtyProgress::new())
    } else {
        Box::new(LineProgress::stderr())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Capture {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn line_progress_reports_at_least_every_five_percent() {
        let cap = Capture::default();
        let mut p = LineProgress::new(Box::new(cap.clone()));
        p.begin(100);
        for i in 0..100 {
            p.file_start(&format!("f{i}"));
            p.file_done();
        }
        p.finish("Completed");
        let text = String::from_utf8(cap.0.lock().unwrap().clone()).unwrap();
        let lines: Vec<_> = text.lines().filter(|l| l.contains('%')).collect();
        assert!(
            lines.len() >= 20,
            "expected >=20 progress lines, got {}",
            lines.len()
        );
        assert!(text.contains("Completed"));
    }

    #[test]
    fn line_progress_zero_total_does_not_panic_or_divide_by_zero() {
        let cap = Capture::default();
        let mut p = LineProgress::new(Box::new(cap.clone()));
        p.begin(0);
        p.finish("Completed");
    }

    #[test]
    fn bar_progress_tracks_position_and_length() {
        // hidden() renders nothing but still tracks state.
        let bar = indicatif::ProgressBar::hidden();
        let mut p = BarProgress::new(bar.clone());
        p.begin(3);
        assert_eq!(bar.length(), Some(3));
        p.file_start("a.pf");
        p.file_done();
        p.file_done();
        p.file_done();
        assert_eq!(bar.position(), 3);
    }
}
