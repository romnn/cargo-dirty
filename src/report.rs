use owo_colors::OwoColorize;

use crate::cargo::CargoExecution;
use crate::engine::Counts;
use crate::explain;
use crate::parse::ParsedCargoOutput;

pub fn print_summary(exec: &CargoExecution, counts: Counts) {
    let secs = exec.duration.as_secs_f64();

    let details = format!(
        "in {:.2}s (fresh {}, dirty {}, work {})",
        secs, counts.fresh, counts.dirty, counts.work
    );

    if exec.status.success() {
        println!("{} {}", "ok".green(), details.dimmed());
    } else {
        println!("{} {}", "failed".red(), details.dimmed());
    }
}

pub fn print_errors(parsed: &ParsedCargoOutput) {
    for msg in &parsed.messages {
        if let cargo_metadata::Message::CompilerMessage(cm) = msg
            && matches!(
                cm.message.level,
                cargo_metadata::diagnostic::DiagnosticLevel::Error
            )
        {
            eprintln!("{}", cm.message.rendered.as_deref().unwrap_or(""));
        }
    }
}

pub fn print_explain(parsed: &ParsedCargoOutput) {
    let analysis = explain::analyze(parsed);
    let Some(culprit) = analysis.culprit else {
        return;
    };

    println!("{} {}", "culprit".green().bold(), culprit.bold());

    if analysis.cascade.is_empty() {
        return;
    }

    for (idx, entry) in analysis.cascade.iter().enumerate() {
        let idx_raw = format!("{:>3}", idx + 1);
        let idx_s = idx_raw.dimmed();

        println!(
            "{idx_s} {} {}",
            crate::cargo::verb(entry.kind).green().bold(),
            entry.crate_id.bold()
        );

        if let Some(caused_by) = &entry.caused_by {
            println!("     {} {}", "caused by:".dimmed(), caused_by.dimmed());
        }

        if let Some(reason) = &entry.reason {
            println!("     {} {}", "reason:".dimmed(), reason.dimmed());
        }

        for detail in &entry.details {
            println!("     {} {}", "detail:".dimmed(), detail.dimmed());
        }
    }
}
