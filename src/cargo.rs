use std::ffi::OsString;
use std::io::{BufRead, BufReader};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Context;

use crate::cli::Args;
use crate::parse::{parse_status_line, CrateStatusKind, ParsedCargoOutput};

pub struct CargoExecution {
    pub status: ExitStatus,
    pub parsed: ParsedCargoOutput,
    pub duration: Duration,
}

#[derive(Debug)]
enum StreamEvent {
    Work {
        kind: CrateStatusKind,
        crate_id: String,
        reason: Option<String>,
    },
}

pub fn compose_cargo_args(args: &Args) -> Vec<OsString> {
    let mut cargo_args: Vec<OsString> = Vec::new();

    let mut user_args: Vec<OsString> = Vec::new();
    user_args.push(args.cargo_cmd().clone());
    user_args.extend(args.cargo_args().iter().cloned());

    let user_has_verbose = user_args
        .iter()
        .any(|a| a == "--verbose" || a.to_string_lossy().starts_with("-v"));
    if !user_has_verbose {
        cargo_args.push(OsString::from("-vv"));
    }

    cargo_args.extend(user_args);

    let has_message_format = cargo_args.iter().any(|a| {
        let s = a.to_string_lossy();
        s == "--message-format" || s.starts_with("--message-format=")
    });
    if !has_message_format {
        cargo_args.push(OsString::from("--message-format=json"));
    }

    if args.linear {
        let has_jobs = cargo_args.iter().any(|a| {
            let s = a.to_string_lossy();
            s == "--jobs" || s.starts_with("--jobs=") || s == "-j" || s.starts_with("-j")
        });
        if !has_jobs {
            cargo_args.push(OsString::from("--jobs=1"));
        }
    }

    cargo_args
}

pub fn run_cargo(args: &Args) -> anyhow::Result<CargoExecution> {
    let started_at = Instant::now();
    let mut cmd = Command::new(args.cargo_path.as_deref().unwrap_or_else(|| "cargo".as_ref()));
    cmd.args(compose_cargo_args(args));
    cmd.env("CARGO_TERM_COLOR", "never");

    let deep = args.deep;

    let (tx, rx) = mpsc::channel::<StreamEvent>();
    let printer = thread::spawn(move || {
        for ev in rx {
            match ev {
                StreamEvent::Work { kind, crate_id, reason } => {
                    println!("{} {crate_id}", verb(kind));
                    if let Some(reason) = reason {
                        println!("     {} {reason}", "reason:");
                    } else if deep {
                        println!("     {} (no high-confidence reason found in v1)", "reason:");
                    }
                }
            }
        }
    });

    if args.deep {
        cmd.env("CARGO_LOG", "cargo::core::compiler::fingerprint=trace");
    }

    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().context("failed to spawn cargo")?;

    let stdout = child.stdout.take().context("failed to capture cargo stdout")?;
    let stderr = child.stderr.take().context("failed to capture cargo stderr")?;

    let parsed = Arc::new(Mutex::new(ParsedCargoOutput::default()));

    let parsed_for_stderr = Arc::clone(&parsed);
    let tx_for_stderr = tx.clone();
    let stderr_thread = thread::spawn(move || {
        let mut streamed_work: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut stderr_reader = BufReader::new(stderr);
        let mut line = String::new();
        loop {
            line.clear();
            let n = stderr_reader.read_line(&mut line).ok()?;
            if n == 0 {
                break;
            }

            if line.ends_with('\n') {
                line.pop();
                if line.ends_with('\r') {
                    line.pop();
                }
            }

            let line_trimmed = line.trim_start();

            let status_event = parse_status_line(line_trimmed);

            let mut parsed_guard = parsed_for_stderr.lock().ok()?;
            parsed_guard.ingest_stderr_line(&line);

            if let Some(ev) = status_event {
                match ev.kind {
                    CrateStatusKind::Compiling
                    | CrateStatusKind::Checking
                    | CrateStatusKind::Building => {
                        if streamed_work.insert(ev.crate_id.clone()) {
                            let reason = parsed_guard.crate_reasons.get(&ev.crate_id).cloned();
                            let _ = tx_for_stderr.send(StreamEvent::Work {
                                kind: ev.kind,
                                crate_id: ev.crate_id,
                                reason,
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
        Some(())
    });

    let parsed_for_stdout = Arc::clone(&parsed);
    let stdout_thread = thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for message in cargo_metadata::Message::parse_stream(reader) {
            match message {
                Ok(msg) => {
                    let mut parsed_guard = parsed_for_stdout.lock().ok()?;
                    parsed_guard.ingest_message(msg);
                }
                Err(_) => {}
            }
        }
        Some(())
    });

    let status = child.wait().context("failed to wait for cargo")?;
    let duration = started_at.elapsed();

    let _ = stderr_thread.join();
    let _ = stdout_thread.join();

    drop(tx);
    let _ = printer.join();

    let parsed = Arc::try_unwrap(parsed)
        .ok()
        .and_then(|m| m.into_inner().ok())
        .unwrap_or_default();

    Ok(CargoExecution {
        status,
        parsed,
        duration,
    })
}

fn verb(kind: CrateStatusKind) -> &'static str {
    match kind {
        CrateStatusKind::Compiling => "Compiling",
        CrateStatusKind::Checking => "Checking",
        CrateStatusKind::Building => "Building",
        _ => "Work",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::CargoSubcommand;

    fn mk_args(cmd: &str, cargo_args: Vec<&str>) -> Args {
        Args {
            show_fresh: false,
            deep: false,
            linear: false,
            cargo_path: None,
            cargo: CargoSubcommand::Cargo(
                std::iter::once(cmd)
                    .chain(cargo_args.clone())
                    .map(OsString::from)
                    .collect(),
            ),
        }
    }

    fn args_to_strings(v: Vec<OsString>) -> Vec<String> {
        v.into_iter().map(|s| s.to_string_lossy().to_string()).collect()
    }

    #[test]
    fn injects_vv_when_no_user_verbose() {
        let args = mk_args("build", vec![]);
        let out = args_to_strings(compose_cargo_args(&args));
        assert!(out.contains(&"-vv".to_string()));
    }

    #[test]
    fn does_not_inject_vv_when_user_verbose() {
        let args = mk_args("build", vec!["-v"]);
        let out = args_to_strings(compose_cargo_args(&args));
        assert!(!out.contains(&"-vv".to_string()));
    }

    #[test]
    fn injects_message_format_json_when_missing() {
        let args = mk_args("build", vec![]);
        let out = args_to_strings(compose_cargo_args(&args));
        assert!(out.contains(&"--message-format=json".to_string()));
    }

    #[test]
    fn does_not_override_existing_message_format() {
        let args = mk_args(
            "build",
            vec!["--message-format", "json-render-diagnostics"],
        );
        let out = args_to_strings(compose_cargo_args(&args));
        assert!(!out.contains(&"--message-format=json".to_string()));
    }

    #[test]
    fn injects_jobs_1_for_linear_when_missing() {
        let mut args = mk_args("build", vec![]);
        args.linear = true;
        let out = args_to_strings(compose_cargo_args(&args));
        assert!(out.contains(&"--jobs=1".to_string()));
    }

    #[test]
    fn does_not_inject_jobs_when_user_specified_jobs() {
        let mut args = mk_args("build", vec!["-j4"]);
        args.linear = true;
        let out = args_to_strings(compose_cargo_args(&args));
        assert!(!out.contains(&"--jobs=1".to_string()));
    }

    #[test]
    fn forwards_command_and_args() {
        let args = mk_args("check", vec!["--workspace"]);
        let out = args_to_strings(compose_cargo_args(&args));
        assert!(out.iter().any(|a| a == "check"));
        assert!(out.iter().any(|a| a == "--workspace"));
    }
}
