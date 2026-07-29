//! Everything the tool prints.
//!
//! Rendering lives here alone — live status lines, compiler errors, the failure dump, the
//! `--explain` cascade, and the summary — so an output change is a change to one file.
//!
//! Every write deliberately ignores failure. Output that has lost its consumer (`cargo dirty …
//! | head` after `head` exits) is worthless, but finishing the run and mirroring cargo's exit
//! code still matters — and a panicking print in a reader thread would drop cargo's pipe and
//! kill the user's build.

use std::io::Write as _;

use owo_colors::OwoColorize;

use crate::build_log::{BuildLog, DisplayEvent};
use crate::cargo::CargoExecution;
use crate::explain;

/// Renders one live build event. `show_fresh` decides whether untouched crates are listed;
/// they are always tracked, only their display is optional.
pub fn print_stream_event(event: &DisplayEvent, show_fresh: bool) {
    let mut out = std::io::stdout().lock();

    match event {
        DisplayEvent::Fresh { id } => {
            if show_fresh {
                let _ = writeln!(out, "{} {}", "Fresh".dimmed(), id.dimmed());
            }
        }
        DisplayEvent::WorkStarted { kind, id, reason } => {
            let _ = writeln!(out, "{} {}", kind.verb().green().bold(), id.bold());
            if let Some(reason) = reason {
                let _ = writeln!(out, "     {} {}", "reason:".dimmed(), reason.dimmed());
            }
        }
        // A late reason lands after other crates have already been announced, so it has to name
        // the crate it belongs to.
        DisplayEvent::LateReason { id, reason } => {
            let label = format!("reason({id}):");
            let _ = writeln!(out, "     {} {}", label.dimmed(), reason.dimmed());
        }
    }
}

/// Prints the closing line: the outcome, how long cargo took, and the staleness tally.
pub fn print_summary(exec: &CargoExecution) {
    let counts = exec.log.counts();
    let secs = exec.duration.as_secs_f64();

    let details = format!(
        "in {:.2}s (fresh {}, dirty {}, work {})",
        secs, counts.fresh, counts.dirty, counts.work
    );

    let mut out = std::io::stdout().lock();
    if exec.status.success() {
        let _ = writeln!(out, "{} {}", "ok".green(), details.dimmed());
    } else {
        let _ = writeln!(out, "{} {}", "failed".red(), details.dimmed());
    }
}

/// Prints every rendered compiler error and reports how many were printed, so callers can tell
/// a compile failure from a failure cargo itself reported on stderr.
pub fn print_errors(messages: &[cargo_metadata::Message]) -> usize {
    let mut err = std::io::stderr().lock();
    let mut printed = 0;

    for msg in messages {
        if let cargo_metadata::Message::CompilerMessage(cm) = msg
            && matches!(
                cm.message.level,
                cargo_metadata::diagnostic::DiagnosticLevel::Error
            )
            && let Some(rendered) = cm.message.rendered.as_deref()
        {
            let _ = writeln!(err, "{rendered}");
            printed += 1;
        }
    }

    printed
}

/// How many trailing stderr lines to show when cargo failed without saying `error` anywhere.
const RAW_STDERR_TAIL_LINES: usize = 20;

/// Dumps cargo's own stderr for failures that produce no compiler diagnostics — a missing
/// manifest, a build script that died, a bad profile name.
pub fn print_raw_stderr_tail(log: &BuildLog) {
    let other_stderr = log.other_stderr();

    let start = other_stderr
        .iter()
        .position(|line| line.trim_start().starts_with("error"))
        .unwrap_or_else(|| other_stderr.len().saturating_sub(RAW_STDERR_TAIL_LINES));

    let mut err = std::io::stderr().lock();
    for line in other_stderr.get(start..).unwrap_or_default() {
        let _ = writeln!(err, "{line}");
    }
}

/// Prints the crate blamed for the rebuild and the cascade that followed from it. Prints nothing
/// when the build did no work.
pub fn print_explain(log: &BuildLog) {
    let Some(analysis) = explain::analyze(log) else {
        return;
    };

    let mut out = std::io::stdout().lock();
    let _ = writeln!(
        out,
        "{} {}",
        "culprit".green().bold(),
        analysis.culprit.bold()
    );

    for (idx, entry) in analysis.cascade.iter().enumerate() {
        let idx_raw = format!("{:>3}", idx + 1);
        let idx_s = idx_raw.dimmed();

        let _ = writeln!(
            out,
            "{idx_s} {} {}",
            entry.kind.verb().green().bold(),
            entry.crate_id.bold()
        );

        if let Some(caused_by) = &entry.caused_by {
            let _ = writeln!(out, "     {} {}", "caused by:".dimmed(), caused_by.dimmed());
        }

        if let Some(reason) = &entry.reason {
            let _ = writeln!(out, "     {} {}", "reason:".dimmed(), reason.dimmed());
        }

        for detail in &entry.details {
            let _ = writeln!(out, "     {} {}", "detail:".dimmed(), detail.dimmed());
        }
    }
}
