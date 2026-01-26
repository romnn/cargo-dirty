use std::collections::{BTreeMap, HashSet};

use owo_colors::OwoColorize;

use crate::cli::Args;
use crate::cargo::CargoExecution;
use crate::parse::{CrateStatusEvent, CrateStatusKind, ParsedCargoOutput};

#[derive(Debug)]
pub struct TimelineEntry {
    pub crate_id: String,
    pub kind: CrateStatusKind,
    pub reason: Option<String>,
}

pub fn compute_counts(parsed: &ParsedCargoOutput) -> SummaryCounts {
    let mut fresh: HashSet<String> = HashSet::new();
    let mut dirty: HashSet<String> = HashSet::new();
    let mut work: HashSet<String> = HashSet::new();

    for ev in &parsed.stderr_events {
        if ev.crate_id.starts_with('<') {
            continue;
        }

        match ev.kind {
            CrateStatusKind::Fresh => {
                fresh.insert(ev.crate_id.clone());
            }
            CrateStatusKind::Dirty => {
                dirty.insert(ev.crate_id.clone());
            }
            CrateStatusKind::Compiling | CrateStatusKind::Checking | CrateStatusKind::Building => {
                work.insert(ev.crate_id.clone());
            }
            _ => {}
        }
    }

    SummaryCounts {
        fresh: fresh.len(),
        dirty: dirty.len(),
        work: work.len(),
    }
}

pub fn print_summary(exec: &CargoExecution, counts: SummaryCounts) {
    let secs = exec.duration.as_secs_f64();

    if exec.status.success() {
        println!(
            "{} in {:.2}s (fresh {}, dirty {}, work {})",
            "ok".green(),
            secs,
            counts.fresh,
            counts.dirty,
            counts.work
        );
    } else {
        println!(
            "{} in {:.2}s (fresh {}, dirty {}, work {})",
            "failed".red(),
            secs,
            counts.fresh,
            counts.dirty,
            counts.work
        );
    }
}

pub fn print_errors(parsed: &ParsedCargoOutput) {
    for msg in &parsed.messages {
        if let cargo_metadata::Message::CompilerMessage(cm) = msg {
            if matches!(cm.message.level, cargo_metadata::diagnostic::DiagnosticLevel::Error) {
                eprintln!("{}", cm.message.rendered.as_deref().unwrap_or(""));
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct Timeline {
    pub entries: Vec<TimelineEntry>,
    pub reasons: BTreeMap<String, String>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SummaryCounts {
    pub fresh: usize,
    pub dirty: usize,
    pub work: usize,
}

pub fn build_timeline(parsed: &ParsedCargoOutput, args: &Args) -> Timeline {
    let mut seen: HashSet<String> = HashSet::new();
    let mut entries: Vec<TimelineEntry> = Vec::new();

    for ev in &parsed.stderr_events {
        match ev.kind {
            CrateStatusKind::Compiling | CrateStatusKind::Checking | CrateStatusKind::Building => {
                if seen.insert(ev.crate_id.clone()) {
                    entries.push(TimelineEntry {
                        crate_id: ev.crate_id.clone(),
                        kind: ev.kind.clone(),
                        reason: parsed.crate_reasons.get(&ev.crate_id).cloned(),
                    });
                }
            }
            CrateStatusKind::Fresh if args.show_fresh => {
                if seen.insert(ev.crate_id.clone()) {
                    entries.push(TimelineEntry {
                        crate_id: ev.crate_id.clone(),
                        kind: ev.kind.clone(),
                        reason: None,
                    });
                }
            }
            _ => {}
        }
    }

    Timeline {
        entries,
        reasons: parsed.crate_reasons.clone(),
    }
}

pub fn print_report(tl: &Timeline, args: &Args) -> anyhow::Result<()> {
    if tl.entries.is_empty() {
        return Ok(());
    }

    let culprit = find_first_culprit(tl);

    for (idx, entry) in tl.entries.iter().enumerate() {
        let idx_raw = format!("{:>3}", idx + 1);
        let idx_s = idx_raw.dimmed();

        let verb = match entry.kind {
            CrateStatusKind::Compiling => "Compiling",
            CrateStatusKind::Checking => "Checking",
            CrateStatusKind::Building => "Building",
            CrateStatusKind::Fresh => "Fresh",
            _ => "Work",
        };

        let mut line = format!("{idx_s} {verb} {}", entry.crate_id.bold());

        if Some(&entry.crate_id) == culprit.as_ref() {
            line.push_str(&format!(" {}", "<culprit>".red().bold()));
        }

        println!("{line}");

        if let Some(reason) = &entry.reason {
            println!("     {} {reason}", "reason:".dimmed());
        } else if args.deep {
            println!("     {} (no high-confidence reason found in v1)", "reason:".dimmed());
        }
    }

    Ok(())
}

fn find_first_culprit(tl: &Timeline) -> Option<String> {
    for entry in &tl.entries {
        if let Some(reason) = entry.reason.as_deref() {
            let lower = reason.to_ascii_lowercase();
            if !lower.contains("dependency") && !lower.contains("dependencies") {
                return Some(entry.crate_id.clone());
            }
        } else {
            return Some(entry.crate_id.clone());
        }
    }
    tl.entries.first().map(|e| e.crate_id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Args, CargoSubcommand};

    fn mk_event(kind: CrateStatusKind, crate_id: &str, reason: Option<&str>) -> CrateStatusEvent {
        CrateStatusEvent {
            kind,
            crate_id: crate_id.to_string(),
            reason: reason.map(|s| s.to_string()),
        }
    }

    #[test]
    fn timeline_keeps_first_work_order() {
        let mut parsed = ParsedCargoOutput::default();
        parsed.stderr_events.push(mk_event(CrateStatusKind::Compiling, "a v0.1.0", None));
        parsed.stderr_events.push(mk_event(CrateStatusKind::Compiling, "b v0.1.0", None));
        parsed.stderr_events.push(mk_event(CrateStatusKind::Compiling, "a v0.1.0", None));

        let args = Args {
            show_fresh: false,
            deep: false,
            linear: false,
            cargo_path: None,
            cargo: CargoSubcommand::Cargo(vec!["build".into()]),
        };
        let tl = build_timeline(&parsed, &args);
        assert_eq!(tl.entries.len(), 2);
        assert_eq!(tl.entries[0].crate_id, "a v0.1.0");
        assert_eq!(tl.entries[1].crate_id, "b v0.1.0");
    }
}
