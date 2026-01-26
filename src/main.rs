use anyhow::Context;

mod cli;
mod cargo;
mod engine;
mod explain;
mod parse;
mod report;

fn main() -> anyhow::Result<()> {
    let args = cli::Args::parse();

    let exec = cargo::run_cargo(&args).context("failed to run cargo")?;

    if !exec.status.success() {
        report::print_errors(&exec.parsed);
    }

    if args.explain {
        report::print_explain(&exec.parsed);
    }

    report::print_summary(&exec, exec.counts);

    std::process::exit(exec.status.code().unwrap_or(1));
}
