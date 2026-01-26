use std::ffi::OsString;
use std::path::PathBuf;

use clap::Parser;

#[derive(clap::Subcommand, Debug, Clone)]
pub enum CargoSubcommand {
    #[command(external_subcommand)]
    Cargo(Vec<OsString>),
}

#[derive(Parser, Debug)]
#[command(bin_name = "cargo")]
pub struct Args {
    #[arg(long)]
    pub show_fresh: bool,

    #[arg(long)]
    pub deep: bool,

    #[arg(long)]
    pub explain: bool,

    #[arg(long)]
    pub linear: bool,

    #[arg(long)]
    pub cargo_path: Option<PathBuf>,

    #[command(subcommand)]
    pub cargo: CargoSubcommand,
}

impl Args {
    pub fn parse() -> Self {
        let mut argv: Vec<OsString> = std::env::args_os().collect();
        if argv
            .get(1)
            .is_some_and(|a| a.to_string_lossy() == "dirty")
        {
            argv.remove(1);
        }

        <Self as Parser>::parse_from(argv)
    }

    pub fn cargo_cmd(&self) -> &OsString {
        match &self.cargo {
            CargoSubcommand::Cargo(v) => v
                .first()
                .expect("cargo-dirty requires a cargo command"),
        }
    }

    pub fn cargo_args(&self) -> &[OsString] {
        match &self.cargo {
            CargoSubcommand::Cargo(v) => {
                if v.len() <= 1 {
                    &[]
                } else {
                    &v[1..]
                }
            }
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

    #[test]
    fn parses_own_flags_before_cargo_cmd() {
        let args = Args::parse_from([
            "cargo-dirty",
            "--linear",
            "check",
            "--workspace",
            "--all-targets",
        ]);

        assert!(args.linear);
        assert_eq!(args.cargo_cmd().to_string_lossy(), "check");
        assert_eq!(
            args.cargo_args()
                .iter()
                .map(|s| s.to_string_lossy().to_string())
                .collect::<Vec<_>>(),
            vec!["--workspace".to_string(), "--all-targets".to_string()]
        );
    }

    #[test]
    fn allows_hyphen_args_as_cargo_args() {
        let args = Args::parse_from(["cargo-dirty", "build", "-Z", "unstable-options"]);
        assert_eq!(args.cargo_cmd().to_string_lossy(), "build");
        assert_eq!(
            args.cargo_args()
                .iter()
                .map(|s| s.to_string_lossy().to_string())
                .collect::<Vec<_>>(),
            vec!["-Z".to_string(), "unstable-options".to_string()]
        );
    }

    #[test]
    fn parses_explain_flag() {
        let args = Args::parse_from(["cargo-dirty", "--explain", "check"]);
        assert!(args.explain);
        assert_eq!(args.cargo_cmd().to_string_lossy(), "check");
    }
}
