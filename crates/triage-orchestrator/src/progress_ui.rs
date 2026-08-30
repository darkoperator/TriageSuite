//! Append-only terminal progress printer for a `TriageSuite run`: host
//! header, `▶` tool-started lines, `[n/total]` colored tool-finished lines,
//! and a final `Done` line. Never repositions the cursor, so the ~15 tool
//! source files that `eprintln!` during parsing can never strand a frame.
//! Best-effort: decorations disabled cleanly when stderr is not a terminal
//! or `--no-progress` is set. Never affects parsing, the manifest, or exit
//! codes.

use crate::archive::ExtractReport;
use crate::execute::ToolRunResult;
use crate::external::ExternalToolReport;
use std::io::IsTerminal;
use std::path::Path;

/// Format one external tool's report (Hayabusa/Takajo). Same three-state shape as
/// `summary_line`, plus a fourth ("not found on PATH") this crate's external tools can
/// also produce — a state in-process `Tool`s never have.
pub fn external_tool_line(report: &ExternalToolReport, colored: bool) -> String {
    if !report.found {
        let glyph = "\u{25CB}"; // ○
        let marker = if colored {
            console::Style::new()
                .force_styling(true)
                .yellow()
                .apply_to(glyph)
                .to_string()
        } else {
            glyph.to_string()
        };
        return format!("{marker} {} not found on PATH, skipped", report.tool);
    }
    let ok = report.error.is_none();
    let glyph = if ok { "\u{2714}" } else { "\u{2718}" }; // ✔ / ✘
    let marker = if colored {
        let style = console::Style::new().force_styling(true);
        let style = if ok { style.green() } else { style.red() };
        style.apply_to(glyph).to_string()
    } else {
        glyph.to_string()
    };
    match &report.error {
        Some(e) => format!("{marker} {} {e}", report.tool),
        None if report.output_paths.is_empty() => format!("{marker} {}", report.tool),
        None => {
            let paths = report
                .output_paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            format!("{marker} {} -> {paths}", report.tool)
        }
    }
}

/// Format one tool's result. `colored` forces ANSI styling on/off deterministically
/// (independent of ambient TTY detection) so callers control color per Global Constraints.
pub fn summary_line(result: &ToolRunResult, colored: bool) -> String {
    let ok = result.error.is_none() && result.failed == 0;
    let glyph = if ok { "\u{2714}" } else { "\u{2718}" }; // ✔ / ✘
    let marker = if colored {
        let style = console::Style::new().force_styling(true);
        let style = if ok { style.green() } else { style.red() };
        style.apply_to(glyph).to_string()
    } else {
        glyph.to_string()
    };
    match (&result.error, result.failed) {
        (None, 0) => format!(
            "{marker} {} {} parsed, {} records",
            result.binary_name, result.parsed, result.records
        ),
        (None, failed) => format!(
            "{marker} {} {failed} artifact(s) failed",
            result.binary_name
        ),
        (Some(e), _) => format!("{marker} {} {}", result.binary_name, e),
    }
}

/// Color is enabled only on a TTY and when NO_COLOR is unset.
fn resolve_color(tty: bool, no_color_set: bool) -> bool {
    tty && !no_color_set
}

// 256-color palette for the startup banner ("diagnostic pulse" theme).
mod banner_palette {
    pub const CYAN: u8 = 51;
    pub const BLUE: u8 = 39;
    pub const DARK_GRAY: u8 = 242;
    pub const LIGHT_GRAY: u8 = 250;
    pub const RED: u8 = 196;
    pub const AMBER: u8 = 214;
    pub const GREEN: u8 = 46;
}

/// (color256, bold, text) — one styled run within a banner line.
type BannerSeg<'a> = (Option<u8>, bool, &'a str);

fn render_banner_line(segs: &[BannerSeg], colored: bool, out: &mut String) {
    for &(color, bold, text) in segs {
        if colored && (color.is_some() || bold) {
            let mut style = console::Style::new().force_styling(true);
            if let Some(c) = color {
                style = style.color256(c);
            }
            if bold {
                style = style.bold();
            }
            out.push_str(&style.apply_to(text).to_string());
        } else {
            out.push_str(text);
        }
    }
    out.push('\n');
}

/// Startup banner: an EKG-style diagnostic pulse over a "TRIAGE SUITE" block-letter
/// logo, with a telemetry footer. `colored` forces ANSI styling on/off deterministically,
/// same convention as [`summary_line`].
pub fn banner(colored: bool) -> String {
    use banner_palette::*;

    const ENGINE_VERSION: &str = concat!("DFIR Core v", env!("CARGO_PKG_VERSION"));

    let lines: [&[BannerSeg]; 10] = [
        &[(Some(DARK_GRAY), false, "──[ NORMAL DIAGNOSTICS ]──/▄╗▀═══════════"), (Some(RED), false, "/\\▄▄_/▀\\▄"), (Some(DARK_GRAY), false, "──────"), (Some(AMBER), false, "[ ALERT: INVESTIGATION ACTIVE ]───")],
        &[(None, false, "  "), (Some(CYAN), false, "████████╗██████╗ ██╗ █████╗  ██████╗ ███████╗   "), (Some(BLUE), false, "███████╗██╗   ██╗██╗████████╗███████╗")],
        &[(None, false, "  "), (Some(CYAN), false, "╚══██╔══╝██╔══██╗██║██╔══██╗██╔════╝ ██╔════╝   "), (Some(BLUE), false, "██╔════╝██║   ██║██║╚══██╔══╝██╔════╝")],
        &[(None, false, "  "), (Some(CYAN), false, "   ██║   ██████╔╝██║███████║██║  ███╗█████╗     "), (Some(BLUE), false, "███████╗██║   ██║██║   ██║   █████╗  ")],
        &[(None, false, "  "), (Some(CYAN), false, "   ██║   ██╔══██╗██║██╔══██║██║   ██║██╔══╝     "), (Some(BLUE), false, "╚════██║██║   ██║██║   ██║   ██╔══╝  ")],
        &[(None, false, "  "), (Some(CYAN), false, "   ██║   ██║  ██║██║██║  ██║╚██████╔╝███████╗   "), (Some(BLUE), false, "███████║╚██████╔╝██║   ██║   ███████╗")],
        &[(None, false, "  "), (Some(CYAN), false, "   ╚═╝   ╚═╝  ╚═╝╚═╝╚═╝  ╚═╝ ╚═════╝ ╚══════╝   "), (Some(BLUE), false, "╚══════╝ ╚═════╝ ╚═╝   ╚═╝   ╚══════╝")],
        &[(None, false, "  "), (Some(DARK_GRAY), false, "══════════════════════════════════════════════════════════════════════════════════════")],
        &[(None, false, "  "), (Some(DARK_GRAY), false, "├── "), (Some(LIGHT_GRAY), false, "PROJECT:"), (None, false, " "), (None, true, "TRIAGE SUITE"), (None, false, "        "), (Some(DARK_GRAY), false, "├── "), (Some(LIGHT_GRAY), false, "STATUS:"), (None, false, " "), (Some(RED), true, "COMPROMISED HOST ASSESSMENT")],
        &[(None, false, "  "), (Some(DARK_GRAY), false, "└── "), (Some(LIGHT_GRAY), false, "ENGINE:"), (None, false, "  "), (Some(CYAN), false, ENGINE_VERSION), (None, false, "    "), (Some(DARK_GRAY), false, "└── "), (Some(LIGHT_GRAY), false, "TELEMETRY:"), (None, false, " "), (Some(GREEN), false, "ACTIVE & STREAMING")],
    ];

    let mut out = String::new();
    out.push('\n');
    for segs in lines {
        render_banner_line(segs, colored, &mut out);
    }
    out.push('\n');
    out
}

/// Print the startup banner, decorated only on a TTY (honors `NO_COLOR`).
/// Suppressed entirely when stderr is not a terminal, matching the rest of
/// this module's "clean when redirected" convention.
pub fn print_banner() {
    let tty = std::io::stderr().is_terminal();
    if !tty {
        return;
    }
    let no_color_set = std::env::var_os("NO_COLOR").is_some();
    eprint!("{}", banner(resolve_color(tty, no_color_set)));
}

/// Compact duration: "3s", "1m4s".
fn fmt_dur(d: std::time::Duration) -> String {
    let s = d.as_secs();
    if s >= 60 {
        format!("{}m{}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

/// Append-only progress printer. Never repositions the cursor, so the tools'
/// own stderr diagnostics can never strand a frame. Decorations show only on a
/// TTY (unless `--no-progress`); color additionally honors `NO_COLOR`.
pub struct ProgressUi {
    decorate: bool,
    color: bool,
}

impl ProgressUi {
    pub fn new(no_progress: bool) -> Self {
        let tty = std::io::stderr().is_terminal();
        let decorate = tty && !no_progress;
        let no_color_set = std::env::var_os("NO_COLOR").is_some();
        let color = resolve_color(tty, no_color_set);
        Self { decorate, color }
    }

    /// Always printed: which host is being processed.
    pub fn host_header(&self, host: &str, os: &str) {
        eprintln!("Host {host}: {os}");
    }

    /// A tool began (shown only with decorations).
    pub fn tool_started(&self, name: &str) {
        if self.decorate {
            eprintln!("\u{25B6} {name}"); // ▶
        }
    }

    /// A tool finished: colored summary, with a `[done/total] … (dur)` frame
    /// when decorating, else the bare summary line.
    pub fn tool_finished(
        &self,
        done: usize,
        total: usize,
        result: &ToolRunResult,
        dur: std::time::Duration,
    ) {
        let summary = summary_line(result, self.color);
        if self.decorate {
            eprintln!("[{done}/{total}] {summary} ({})", fmt_dur(dur));
        } else {
            eprintln!("{summary}");
        }
    }

    /// End-of-host line (shown only with decorations).
    pub fn host_done(&self, total: usize, dur: std::time::Duration) {
        if self.decorate {
            eprintln!("Done \u{2014} {total} tools in {}", fmt_dur(dur)); // —
        }
    }

    /// An external tool (Hayabusa/Takajo) invocation finished. Always printed, same as
    /// `host_header` — external-tool failures previously vanished into the manifest with
    /// no console trace at all, which is exactly the gap this closes.
    pub fn external_tool_finished(&self, report: &ExternalToolReport) {
        eprintln!("{}", external_tool_line(report, self.color));
    }

    /// Extraction of an archive began (shown only with decorations). Worth
    /// announcing because a multi-GB archive can take minutes, and the
    /// append-only contract rules out a spinner.
    pub fn archive_started(&self, archive: &Path, entries: usize, declared: Option<u128>) {
        if !self.decorate {
            return;
        }
        let name = file_name_of(archive);
        match declared {
            Some(total) => eprintln!(
                "\u{25B6} Extracting {name} ({entries} entries, {})",
                fmt_bytes(total)
            ),
            None => eprintln!("\u{25B6} Extracting {name} ({entries} entries)"),
        }
    }

    /// Periodic heartbeat during a long extraction (decorations only). Appends
    /// a line rather than repainting one, per the module contract.
    pub fn archive_progress(&self, done: u64, total: Option<u128>) {
        if !self.decorate {
            return;
        }
        match total {
            Some(t) if t > 0 => {
                let pct = (done as u128 * 100 / t).min(100);
                eprintln!(
                    "  \u{2026} {pct}% ({} / {})",
                    fmt_bytes(done as u128),
                    fmt_bytes(t)
                );
            }
            _ => eprintln!("  \u{2026} {}", fmt_bytes(done as u128)),
        }
    }

    /// Extraction finished. Always printed, for the same reason as
    /// `external_tool_finished`: an archive that yields no host must never be
    /// invisible, including under `--no-progress`.
    pub fn archive_finished(&self, report: &ExtractReport) {
        eprintln!("{}", archive_line(report, self.color));
    }

    /// An archive was skipped. Always printed, same rationale.
    pub fn archive_skipped(&self, archive: &Path, reason: &str) {
        eprintln!("{}", archive_skip_line(archive, reason, self.color));
    }
}

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Human-readable byte count: "3.4 GiB".
fn fmt_bytes(n: u128) -> String {
    const KIB: u128 = 1024;
    const MIB: u128 = KIB * 1024;
    const GIB: u128 = MIB * 1024;
    const TIB: u128 = GIB * 1024;
    if n >= TIB {
        format!("{:.1} TiB", n as f64 / TIB as f64)
    } else if n >= GIB {
        format!("{:.1} GiB", n as f64 / GIB as f64)
    } else if n >= MIB {
        format!("{:.1} MiB", n as f64 / MIB as f64)
    } else if n >= KIB {
        format!("{:.1} KiB", n as f64 / KIB as f64)
    } else {
        format!("{n} B")
    }
}

fn glyph(marker: &str, colored: bool, style: fn(console::Style) -> console::Style) -> String {
    if colored {
        style(console::Style::new().force_styling(true))
            .apply_to(marker)
            .to_string()
    } else {
        marker.to_string()
    }
}

/// One line describing an extraction outcome, in the same shape as
/// `summary_line`/`external_tool_line` so it can be unit-tested without a TTY.
pub fn archive_line(report: &ExtractReport, colored: bool) -> String {
    let name = file_name_of(&report.archive);
    if let Some(err) = &report.error {
        let marker = glyph("\u{2718}", colored, |s| s.red()); // ✘
        return format!("{marker} {name} extraction failed: {err}");
    }
    let dest = file_name_of(&report.dest);
    if report.reused {
        // Yellow ○, matching "not found on PATH, skipped": we did no work.
        let marker = glyph("\u{25CB}", colored, |s| s.yellow());
        return format!("{marker} {name} reusing existing extraction at _extracted/{dest}");
    }
    let marker = glyph("\u{2714}", colored, |s| s.green()); // ✔
    let verb = if report.re_extracted {
        "re-extracted"
    } else {
        "extracted"
    };
    let mut line = format!(
        "{marker} {name} -> _extracted/{dest} ({verb}, {} files, {}, {})",
        report.files_written,
        fmt_bytes(report.bytes_written as u128),
        fmt_dur(report.duration)
    );
    if report.skipped_entries > 0 {
        line.push_str(&format!("; {} entries skipped", report.skipped_entries));
    }
    line
}

/// One line describing a skipped archive.
pub fn archive_skip_line(archive: &Path, reason: &str, colored: bool) -> String {
    let marker = glyph("\u{25CB}", colored, |s| s.yellow()); // ○
    format!("{marker} {} skipped: {reason}", file_name_of(archive))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execute::ToolRunResult;

    fn ok_result() -> ToolRunResult {
        ToolRunResult {
            key: "pe".into(),
            binary_name: "PETriage".into(),
            files_matched: 312,
            parsed: 312,
            failed: 0,
            records: 1155,
            supported: 312,
            unsupported: 0,
            corrupt: 0,
            unreadable: 0,
            deduplicated: 0,
            reason_samples: vec![],
            output_paths: vec![],
            error: None,
            exit: None,
        }
    }
    fn err_result() -> ToolRunResult {
        ToolRunResult {
            key: "sum".into(),
            binary_name: "SumETriage".into(),
            files_matched: 1,
            parsed: 0,
            failed: 1,
            records: 0,
            supported: 1,
            unsupported: 0,
            corrupt: 0,
            unreadable: 0,
            deduplicated: 0,
            reason_samples: vec![],
            output_paths: vec![],
            error: Some("ESE revision 300 unsupported".into()),
            exit: Some(triage_core::error::RunExit::Fatal),
        }
    }

    #[test]
    fn summary_line_plain_has_no_ansi() {
        let s = summary_line(&ok_result(), false);
        assert!(s.contains("PETriage"));
        assert!(s.contains("312 parsed"));
        assert!(s.contains("1155 records"));
        assert!(s.starts_with('\u{2714}')); // ✔
        assert!(!s.contains('\u{1b}'), "no ANSI escape when colored=false");
    }

    #[test]
    fn summary_line_error_shows_error_text() {
        let s = summary_line(&err_result(), false);
        assert!(s.starts_with('\u{2718}')); // ✘
        assert!(s.contains("SumETriage"));
        assert!(s.contains("ESE revision 300 unsupported"));
    }

    #[test]
    fn summary_line_colored_contains_ansi() {
        let s = summary_line(&ok_result(), true);
        assert!(
            s.contains('\u{1b}'),
            "expected ANSI escape when colored=true"
        );
    }

    fn ext_report(
        tool: &str,
        found: bool,
        error: Option<&str>,
        output_paths: Vec<std::path::PathBuf>,
    ) -> ExternalToolReport {
        ExternalToolReport {
            tool: tool.to_string(),
            found,
            invoked: found,
            exit_code: found.then_some(if error.is_some() { 1 } else { 0 }),
            output_paths,
            error: error.map(str::to_string),
        }
    }

    #[test]
    fn external_tool_line_not_found_is_distinct_from_failure() {
        let s = external_tool_line(&ext_report("hayabusa", false, None, vec![]), false);
        assert!(s.contains("hayabusa"));
        assert!(s.contains("not found on PATH"));
    }

    #[test]
    fn external_tool_line_success_shows_output_paths() {
        let s = external_tool_line(
            &ext_report(
                "hayabusa-csv",
                true,
                None,
                vec![std::path::PathBuf::from("/out/timeline.csv")],
            ),
            false,
        );
        assert!(s.starts_with('\u{2714}')); // ✔
        assert!(s.contains("hayabusa-csv"));
        assert!(s.contains("/out/timeline.csv"));
    }

    #[test]
    fn external_tool_line_failure_shows_error_text() {
        let s = external_tool_line(
            &ext_report("hayabusa-json", true, Some("exited with status 1"), vec![]),
            false,
        );
        assert!(s.starts_with('\u{2718}')); // ✘
        assert!(s.contains("exited with status 1"));
    }

    #[test]
    fn banner_plain_has_no_ansi() {
        let s = banner(false);
        assert!(!s.contains('\u{1b}'), "no ANSI escape when colored=false");
        assert!(s.contains("TRIAGE"));
        assert!(s.contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn banner_colored_contains_ansi() {
        let s = banner(true);
        assert!(
            s.contains('\u{1b}'),
            "expected ANSI escape when colored=true"
        );
    }

    #[test]
    fn resolve_color_honors_tty_and_no_color() {
        assert!(resolve_color(true, false)); // TTY, NO_COLOR unset -> color
        assert!(!resolve_color(true, true)); // NO_COLOR set -> no color
        assert!(!resolve_color(false, false)); // not a TTY -> no color
    }

    #[test]
    fn fmt_dur_formats_seconds_and_minutes() {
        assert_eq!(fmt_dur(std::time::Duration::from_secs(0)), "0s");
        assert_eq!(fmt_dur(std::time::Duration::from_secs(3)), "3s");
        assert_eq!(fmt_dur(std::time::Duration::from_secs(64)), "1m4s");
    }
}
