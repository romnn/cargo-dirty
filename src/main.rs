use anyhow::Context;

mod cli;
mod cargo;
mod parse;
mod report;

fn main() -> anyhow::Result<()> {
    let args = cli::Args::parse();

    let exec = cargo::run_cargo(&args).context("failed to run cargo")?;

    let counts = report::compute_counts(&exec.parsed);

    if !exec.status.success() {
        report::print_errors(&exec.parsed);
    }

    report::print_summary(&exec, counts);

    std::process::exit(exec.status.code().unwrap_or(1));
}
