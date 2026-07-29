//! Command-line surface.
//!
//! Own flags come first, then the cargo command and its arguments, which are captured verbatim
//! as an external subcommand and never reinterpreted.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::Parser;

/// The user's cargo command and its arguments, taken as-is.
#[derive(clap::Subcommand, Debug, Clone)]
pub enum CargoSubcommand {
    #[command(external_subcommand)]
    Cargo(Vec<OsString>),
}

/// Runs a cargo command and reports only what was rebuilt, and why.
#[derive(Parser, Debug)]
#[command(bin_name = "cargo dirty", version)]
pub struct Args {
    /// Also list crates cargo considered fresh.
    #[arg(long)]
    pub show_fresh: bool,

    /// Collect cargo's fingerprint trace, which enriches `--explain` with per-crate details.
    #[arg(long)]
    pub deep: bool,

    /// Report the crate blamed for the rebuild and the cascade it caused.
    #[arg(long)]
    pub explain: bool,

    /// Limit cargo to one job so work is streamed in a deterministic order.
    #[arg(long)]
    pub linear: bool,

    /// Cargo binary to run, instead of `cargo` from `PATH`.
    #[arg(long)]
    pub cargo_path: Option<PathBuf>,

    #[command(subcommand)]
    pub cargo: CargoSubcommand,
}

impl Args {
    /// Parses the process arguments, tolerating both `cargo dirty …` and a direct
    /// `cargo-dirty …` invocation.
    pub fn parse() -> Self {
        let mut argv: Vec<OsString> = std::env::args_os().collect();
        if argv.get(1).is_some_and(|a| a.to_string_lossy() == "dirty") {
            argv.remove(1);
        }

        <Self as Parser>::parse_from(argv)
    }

    pub fn cargo_cmd(&self) -> Option<&OsString> {
        match &self.cargo {
            CargoSubcommand::Cargo(v) => v.first(),
        }
    }

    pub fn cargo_args(&self) -> &[OsString] {
        match &self.cargo {
            CargoSubcommand::Cargo(v) => v.get(1..).unwrap_or_default(),
        }
    }

    #[cfg(test)]
    pub fn parse_from<I, T>(itr: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString> + Clone,
    {
        <Self as Parser>::parse_from(itr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;

    #[test]
    fn parses_own_flags_before_cargo_cmd() -> anyhow::Result<()> {
        let args = Args::parse_from([
            "cargo-dirty",
            "--linear",
            "check",
            "--workspace",
            "--all-targets",
        ]);

        assert!(args.linear);
        assert_eq!(
            args.cargo_cmd()
                .context("parsed arguments omitted the cargo command")?
                .to_string_lossy(),
            "check"
        );
        assert_eq!(
            args.cargo_args()
                .iter()
                .map(|s| s.to_string_lossy().to_string())
                .collect::<Vec<_>>(),
            vec!["--workspace".to_string(), "--all-targets".to_string()]
        );
        Ok(())
    }

    #[test]
    fn allows_hyphen_args_as_cargo_args() -> anyhow::Result<()> {
        let args = Args::parse_from(["cargo-dirty", "build", "-Z", "unstable-options"]);
        assert_eq!(
            args.cargo_cmd()
                .context("parsed arguments omitted the cargo command")?
                .to_string_lossy(),
            "build"
        );
        assert_eq!(
            args.cargo_args()
                .iter()
                .map(|s| s.to_string_lossy().to_string())
                .collect::<Vec<_>>(),
            vec!["-Z".to_string(), "unstable-options".to_string()]
        );
        Ok(())
    }

    #[test]
    fn parses_explain_flag() -> anyhow::Result<()> {
        let args = Args::parse_from(["cargo-dirty", "--explain", "check"]);
        assert!(args.explain);
        assert_eq!(
            args.cargo_cmd()
                .context("parsed arguments omitted the cargo command")?
                .to_string_lossy(),
            "check"
        );
        Ok(())
    }
}
