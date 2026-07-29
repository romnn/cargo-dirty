//! Reports which Cargo packages were recompiled and why.

use std::process::ExitStatus;

use anyhow::Context;

mod build_log;
mod cargo;
mod cli;
mod explain;
mod parse;
mod report;

fn main() -> anyhow::Result<()> {
    let args = cli::Args::parse();

    let exec = cargo::run_cargo(&args).context("failed to run cargo")?;

    if !exec.status.success() && report::print_errors(&exec.messages) == 0 {
        report::print_raw_stderr_tail(&exec.log);
    }

    if args.explain {
        report::print_explain(&exec.log);
    }

    report::print_summary(&exec);

    std::process::exit(exit_code(exec.status));
}

/// Mirrors cargo's own exit status so wrapping a build in cargo-dirty stays transparent to
/// scripts, including the shell convention of reporting a signalled child as `128 + signal`.
fn exit_code(status: ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;

        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }

    1
}

// Only unix exposes a way to build an `ExitStatus` without running a process, so these cases
// cannot be covered on other platforms.
#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::process::ExitStatusExt as _;

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn reports_the_child_exit_code() {
        assert_eq!(exit_code(ExitStatus::from_raw(3 << 8)), 3);
    }

    #[test]
    fn reports_a_signalled_child_as_128_plus_signal() {
        assert_eq!(exit_code(ExitStatus::from_raw(9)), 128 + 9);
    }
}
