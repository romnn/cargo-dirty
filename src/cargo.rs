use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow};
use owo_colors::OwoColorize;

use crate::cli::Args;
use crate::engine::{Counts, Engine, Event};
use crate::parse::{CrateStatusKind, ParsedCargoOutput};

pub struct CargoExecution {
    pub status: ExitStatus,
    pub parsed: ParsedCargoOutput,
    pub duration: Duration,
    pub counts: Counts,
}

#[derive(Debug)]
enum StreamEvent {
    WorkStarted {
        kind: CrateStatusKind,
        crate_id: String,
        reason: Option<String>,
    },
    WorkReason {
        reason: String,
    },
}

fn print_stream_events(events: mpsc::Receiver<StreamEvent>) {
    for event in events {
        match event {
            StreamEvent::WorkStarted {
                kind,
                crate_id,
                reason,
            } => {
                println!("{} {}", verb(kind).green().bold(), crate_id.bold());
                if let Some(reason) = reason {
                    println!("     {} {}", "reason:".dimmed(), reason.dimmed());
                }
            }
            StreamEvent::WorkReason { reason } => {
                println!("     {} {}", "reason:".dimmed(), reason.dimmed());
            }
        }
    }
}

fn ingest_stderr(
    stderr: impl Read,
    parsed: &Mutex<ParsedCargoOutput>,
    events: &mpsc::Sender<StreamEvent>,
) -> anyhow::Result<Engine> {
    let mut engine = Engine::new();
    let mut stderr_reader = BufReader::new(stderr);
    let mut line = String::new();

    loop {
        line.clear();
        let bytes_read = stderr_reader
            .read_line(&mut line)
            .context("failed to read cargo stderr")?;
        if bytes_read == 0 {
            break;
        }

        if line.ends_with('\n') {
            line.pop();
            if line.ends_with('\r') {
                line.pop();
            }
        }

        {
            let mut parsed_guard = parsed
                .lock()
                .map_err(|_| anyhow!("cargo output lock was poisoned"))?;
            parsed_guard.ingest_stderr_line(&line);
        }

        for event in engine.ingest_stderr_line(&line) {
            let stream_event = match event {
                Event::WorkStarted {
                    kind,
                    crate_id,
                    reason,
                } => StreamEvent::WorkStarted {
                    kind,
                    crate_id,
                    reason,
                },
                Event::WorkReason {
                    crate_id: _,
                    reason,
                } => StreamEvent::WorkReason { reason },
            };
            events
                .send(stream_event)
                .context("failed to report cargo work")?;
        }
    }

    Ok(engine)
}

fn ingest_stdout(stdout: impl Read, parsed: &Mutex<ParsedCargoOutput>) -> anyhow::Result<()> {
    let reader = BufReader::new(stdout);
    for message in cargo_metadata::Message::parse_stream(reader) {
        let message = message.context("failed to parse cargo JSON output")?;
        let mut parsed_guard = parsed
            .lock()
            .map_err(|_| anyhow!("cargo output lock was poisoned"))?;
        parsed_guard.ingest_message(message);
    }
    Ok(())
}

fn join_worker<T>(worker: JoinHandle<anyhow::Result<T>>, description: &str) -> anyhow::Result<T> {
    worker
        .join()
        .map_err(|_| anyhow!("{description} thread panicked"))?
}

pub fn compose_cargo_args(args: &Args) -> anyhow::Result<Vec<OsString>> {
    let mut cargo_args: Vec<OsString> = Vec::new();

    let mut user_args: Vec<OsString> = Vec::new();
    user_args.push(
        args.cargo_cmd()
            .context("cargo-dirty requires a cargo command")?
            .clone(),
    );
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

    Ok(cargo_args)
}

pub fn run_cargo(args: &Args) -> anyhow::Result<CargoExecution> {
    let started_at = Instant::now();
    let mut cmd = Command::new(
        args.cargo_path
            .as_deref()
            .unwrap_or_else(|| "cargo".as_ref()),
    );
    cmd.args(compose_cargo_args(args)?);
    cmd.env("CARGO_TERM_COLOR", "never");

    let (tx, rx) = mpsc::channel::<StreamEvent>();
    let printer = thread::spawn(move || print_stream_events(rx));

    if args.deep {
        cmd.env("CARGO_LOG", "cargo::core::compiler::fingerprint=trace");
    }

    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().context("failed to spawn cargo")?;

    let stdout = child
        .stdout
        .take()
        .context("failed to capture cargo stdout")?;
    let stderr = child
        .stderr
        .take()
        .context("failed to capture cargo stderr")?;

    let parsed = Arc::new(Mutex::new(ParsedCargoOutput::default()));

    let parsed_for_stderr = Arc::clone(&parsed);
    let tx_for_stderr = tx.clone();
    let stderr_thread =
        thread::spawn(move || ingest_stderr(stderr, &parsed_for_stderr, &tx_for_stderr));

    let parsed_for_stdout = Arc::clone(&parsed);
    let stdout_thread = thread::spawn(move || ingest_stdout(stdout, &parsed_for_stdout));

    let status = child.wait().context("failed to wait for cargo")?;
    let duration = started_at.elapsed();

    let stderr_result = join_worker(stderr_thread, "cargo stderr reader");
    let stdout_result = join_worker(stdout_thread, "cargo stdout reader");
    drop(tx);
    let printer_result = printer
        .join()
        .map_err(|_| anyhow!("cargo progress printer thread panicked"));

    let counts = stderr_result?.counts();
    stdout_result?;
    printer_result?;

    let parsed = Arc::try_unwrap(parsed)
        .map_err(|_| anyhow!("cargo output readers retained shared state"))?
        .into_inner()
        .map_err(|_| anyhow!("cargo output lock was poisoned"))?;

    Ok(CargoExecution {
        status,
        parsed,
        duration,
        counts,
    })
}

pub fn verb(kind: CrateStatusKind) -> &'static str {
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

    fn mk_args(cmd: &str, cargo_args: &[&str]) -> Args {
        Args {
            show_fresh: false,
            deep: false,
            explain: false,
            linear: false,
            cargo_path: None,
            cargo: CargoSubcommand::Cargo(
                std::iter::once(cmd)
                    .chain(cargo_args.iter().copied())
                    .map(OsString::from)
                    .collect(),
            ),
        }
    }

    fn composed_args(args: &Args) -> anyhow::Result<Vec<String>> {
        Ok(compose_cargo_args(args)?
            .into_iter()
            .map(|s| s.to_string_lossy().to_string())
            .collect())
    }

    #[test]
    fn injects_vv_when_no_user_verbose() -> anyhow::Result<()> {
        let args = mk_args("build", &[]);
        let out = composed_args(&args)?;
        assert!(out.contains(&"-vv".to_string()));
        Ok(())
    }

    #[test]
    fn does_not_inject_vv_when_user_verbose() -> anyhow::Result<()> {
        let args = mk_args("build", &["-v"]);
        let out = composed_args(&args)?;
        assert!(!out.contains(&"-vv".to_string()));
        Ok(())
    }

    #[test]
    fn injects_message_format_json_when_missing() -> anyhow::Result<()> {
        let args = mk_args("build", &[]);
        let out = composed_args(&args)?;
        assert!(out.contains(&"--message-format=json".to_string()));
        Ok(())
    }

    #[test]
    fn does_not_override_existing_message_format() -> anyhow::Result<()> {
        let args = mk_args("build", &["--message-format", "json-render-diagnostics"]);
        let out = composed_args(&args)?;
        assert!(!out.contains(&"--message-format=json".to_string()));
        Ok(())
    }

    #[test]
    fn injects_jobs_1_for_linear_when_missing() -> anyhow::Result<()> {
        let mut args = mk_args("build", &[]);
        args.linear = true;
        let out = composed_args(&args)?;
        assert!(out.contains(&"--jobs=1".to_string()));
        Ok(())
    }

    #[test]
    fn does_not_inject_jobs_when_user_specified_jobs() -> anyhow::Result<()> {
        let mut args = mk_args("build", &["-j4"]);
        args.linear = true;
        let out = composed_args(&args)?;
        assert!(!out.contains(&"--jobs=1".to_string()));
        Ok(())
    }

    #[test]
    fn forwards_command_and_args() -> anyhow::Result<()> {
        let args = mk_args("check", &["--workspace"]);
        let out = composed_args(&args)?;
        assert!(out.iter().any(|a| a == "check"));
        assert!(out.iter().any(|a| a == "--workspace"));
        Ok(())
    }

    #[test]
    fn rejects_a_missing_cargo_command() {
        let args = Args {
            show_fresh: false,
            deep: false,
            explain: false,
            linear: false,
            cargo_path: None,
            cargo: CargoSubcommand::Cargo(Vec::new()),
        };

        assert!(compose_cargo_args(&args).is_err());
    }
}
