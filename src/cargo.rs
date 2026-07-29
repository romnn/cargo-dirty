//! Runs the user's cargo command under instrumentation.
//!
//! Composes the child's arguments, spawns it, and reads both its streams concurrently. Three
//! threads are involved: one per output stream, plus a printer that owns stdout so live status
//! lines are not interleaved mid-line with each other.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write as _};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow};
use cargo_metadata::Message;

use crate::build_log::{BuildLog, DisplayEvent};
use crate::cli::Args;
use crate::report;

/// A finished cargo run and everything observed while it ran.
pub struct CargoExecution {
    pub status: ExitStatus,
    pub duration: Duration,
    /// Everything the stderr reader made of the run.
    pub log: BuildLog,
    /// Cargo's JSON messages, as collected by the stdout reader.
    pub messages: Vec<Message>,
}

fn print_stream_events(events: mpsc::Receiver<DisplayEvent>, show_fresh: bool) {
    for event in events {
        report::print_stream_event(&event, show_fresh);
    }
}

fn ingest_stderr(
    stderr: impl Read,
    events: &mpsc::Sender<DisplayEvent>,
) -> anyhow::Result<BuildLog> {
    let mut log = BuildLog::new();
    let mut stderr_reader = BufReader::new(stderr);
    let mut line_bytes = Vec::new();

    loop {
        line_bytes.clear();
        // Bytes, not `read_line`: build scripts and localized toolchains write non-UTF-8, and one
        // such line must not abort the reader — dropping this end of the pipe would kill cargo
        // mid-build with a broken pipe.
        let bytes_read = stderr_reader
            .read_until(b'\n', &mut line_bytes)
            .context("failed to read cargo stderr")?;
        if bytes_read == 0 {
            break;
        }

        if line_bytes.last() == Some(&b'\n') {
            line_bytes.pop();
            if line_bytes.last() == Some(&b'\r') {
                line_bytes.pop();
            }
        }

        let line = String::from_utf8_lossy(&line_bytes);
        for event in log.ingest_line(&line) {
            // A failed send means the printer is gone and nothing displays events anymore.
            // Keep reading regardless: the record must stay complete for the final report, and
            // cargo must never see this end of its stderr pipe close mid-build.
            let _ = events.send(event);
        }
    }

    Ok(log)
}

/// Reads cargo's JSON message stream. Unreadable lines are reported and skipped rather than
/// failing the run, so this cannot fail.
///
/// Writes ignore failure: this runs while cargo does, and panicking here would drop cargo's
/// stdout pipe and kill the user's build just because our own output has no consumer left.
fn ingest_stdout(stdout: impl Read) -> Vec<Message> {
    let mut messages = Vec::new();
    let reader = BufReader::new(stdout);

    for message in cargo_metadata::Message::parse_stream(reader) {
        match message {
            // Anything the built program itself writes reaches us as a text line. Forwarding it
            // immediately is what keeps `cargo dirty run` and `cargo dirty test` usable.
            Ok(Message::TextLine(text)) => {
                let _ = writeln!(std::io::stdout().lock(), "{text}");
            }
            Ok(message) => messages.push(message),
            // Cargo's exit status stays the source of truth, so one unreadable line must not
            // blind us to the rest of the stream.
            Err(err) => {
                let _ = writeln!(
                    std::io::stderr().lock(),
                    "warning: cargo-dirty: unreadable cargo output line: {err}"
                );
            }
        }
    }

    messages
}

fn join_worker<T>(worker: JoinHandle<T>, description: &str) -> anyhow::Result<T> {
    worker
        .join()
        .map_err(|_| anyhow!("{description} thread panicked"))
}

/// Splits user arguments at the first literal `--` into the cargo-side arguments and the
/// arguments meant for the built binary. `None` means the separator was absent, which is
/// distinct from a separator with nothing after it.
fn split_at_separator(args: &[OsString]) -> (&[OsString], Option<&[OsString]>) {
    let Some(idx) = args.iter().position(|a| a == "--") else {
        return (args, None);
    };

    (args.get(..idx).unwrap_or_default(), args.get(idx + 1..))
}

/// Every count this tool reports is derived from cargo's `-vv` status lines, and cargo refuses
/// to run with both `-vv` and `--quiet`, so the flag is a dead end rather than a degraded mode.
fn has_quiet_flag(args: &[OsString]) -> bool {
    args.iter().any(|a| a == "--quiet" || a == "-q")
}

/// Builds the child's argument list: the user's own arguments plus the flags this tool needs to
/// see staleness (`-vv`), read diagnostics (`--message-format=json`), and — under `--linear` —
/// keep the stream ordered (`--jobs=1`).
///
/// A flag is only injected when the user did not already set it, so their choice always wins.
///
/// # Errors
///
/// Returns an error when no cargo subcommand was given.
pub fn compose_cargo_args(args: &Args) -> anyhow::Result<Vec<OsString>> {
    let subcommand = args
        .cargo_cmd()
        .context("cargo-dirty requires a cargo command")?;
    // Everything after `--` belongs to the user's binary, so it must neither be scanned for
    // flags we would otherwise skip injecting, nor receive our injected flags.
    let (cargo_side, passthrough) = split_at_separator(args.cargo_args());

    let mut cargo_args: Vec<OsString> = Vec::new();

    let user_has_verbose = cargo_side
        .iter()
        .any(|a| a == "--verbose" || a.to_string_lossy().starts_with("-v"));
    if !user_has_verbose {
        cargo_args.push(OsString::from("-vv"));
    }

    cargo_args.push(subcommand.clone());
    cargo_args.extend(cargo_side.iter().cloned());

    let has_message_format = cargo_side.iter().any(|a| {
        let s = a.to_string_lossy();
        s == "--message-format" || s.starts_with("--message-format=")
    });
    if !has_message_format {
        cargo_args.push(OsString::from("--message-format=json"));
    }

    if args.linear {
        let has_jobs = cargo_side.iter().any(|a| {
            let s = a.to_string_lossy();
            s == "--jobs" || s.starts_with("--jobs=") || s.starts_with("-j")
        });
        if !has_jobs {
            cargo_args.push(OsString::from("--jobs=1"));
        }
    }

    if let Some(passthrough) = passthrough {
        cargo_args.push(OsString::from("--"));
        cargo_args.extend(passthrough.iter().cloned());
    }

    Ok(cargo_args)
}

/// Runs cargo to completion, printing live status as it goes.
///
/// A non-zero cargo exit status is not an error here: it is reported through
/// [`CargoExecution::status`] so callers can render the failure and mirror the code.
///
/// # Errors
///
/// Returns an error when cargo cannot be spawned or waited on, when its stderr cannot be read,
/// or when one of the reader threads panics.
pub fn run_cargo(args: &Args) -> anyhow::Result<CargoExecution> {
    let (cargo_side, _) = split_at_separator(args.cargo_args());
    if has_quiet_flag(cargo_side) {
        eprintln!(
            "warning: cargo-dirty needs cargo's verbose output; cargo rejects --quiet alongside the -vv it injects"
        );
    }

    let started_at = Instant::now();
    let mut cmd = Command::new(
        args.cargo_path
            .as_deref()
            .unwrap_or_else(|| "cargo".as_ref()),
    );
    cmd.args(compose_cargo_args(args)?);
    cmd.env("CARGO_TERM_COLOR", "never");

    let (tx, rx) = mpsc::channel::<DisplayEvent>();
    let show_fresh = args.show_fresh;
    let printer = thread::spawn(move || print_stream_events(rx, show_fresh));

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

    // Each reader owns its half of the record outright: the threads touch disjoint data, so
    // there is nothing to share and no lock to poison.
    let tx_for_stderr = tx.clone();
    let stderr_thread = thread::spawn(move || ingest_stderr(stderr, &tx_for_stderr));
    let stdout_thread = thread::spawn(move || ingest_stdout(stdout));

    let status = child.wait().context("failed to wait for cargo")?;
    let duration = started_at.elapsed();

    // Every worker is joined before any failure is propagated, so an error in one never leaves
    // another thread unjoined.
    let stderr_result = join_worker(stderr_thread, "cargo stderr reader");
    let stdout_result = join_worker(stdout_thread, "cargo stdout reader");
    drop(tx);
    let printer_result = join_worker(printer, "cargo progress printer");

    let log = stderr_result??;
    let messages = stdout_result?;
    printer_result?;

    Ok(CargoExecution {
        status,
        duration,
        log,
        messages,
    })
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
    fn injected_flags_go_before_double_dash() -> anyhow::Result<()> {
        let args = mk_args("run", &["--", "--marker"]);
        let out = composed_args(&args)?;

        let separator = out
            .iter()
            .position(|a| a == "--")
            .context("composed args lost the `--` separator")?;
        let message_format = out
            .iter()
            .position(|a| a == "--message-format=json")
            .context("composed args lost the injected message format")?;

        assert!(message_format < separator);
        assert_eq!(out.last().map(String::as_str), Some("--marker"));
        Ok(())
    }

    #[test]
    fn verbose_after_double_dash_does_not_suppress_vv() -> anyhow::Result<()> {
        let args = mk_args("run", &["--", "-v"]);
        let out = composed_args(&args)?;
        assert!(out.contains(&"-vv".to_string()));
        Ok(())
    }

    #[test]
    fn jobs_after_double_dash_does_not_suppress_linear() -> anyhow::Result<()> {
        let mut args = mk_args("run", &["--", "-j4"]);
        args.linear = true;
        let out = composed_args(&args)?;

        let separator = out
            .iter()
            .position(|a| a == "--")
            .context("composed args lost the `--` separator")?;
        let jobs = out
            .iter()
            .position(|a| a == "--jobs=1")
            .context("composed args lost the injected job limit")?;

        assert!(jobs < separator);
        Ok(())
    }

    #[test]
    fn omits_the_separator_when_the_user_did_not_pass_one() -> anyhow::Result<()> {
        let args = mk_args("check", &["--workspace"]);
        let out = composed_args(&args)?;
        assert!(!out.contains(&"--".to_string()));
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
    fn detects_quiet_only_on_the_cargo_side() {
        let quiet = mk_args("check", &["-q"]);
        let (cargo_side, _) = split_at_separator(quiet.cargo_args());
        assert!(has_quiet_flag(cargo_side));

        let passthrough_quiet = mk_args("run", &["--", "-q"]);
        let (cargo_side, _) = split_at_separator(passthrough_quiet.cargo_args());
        assert!(!has_quiet_flag(cargo_side));
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
