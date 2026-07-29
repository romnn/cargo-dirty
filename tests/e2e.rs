//! End-to-end tests that drive the built binary against throwaway fixture workspaces.
//!
//! Assertions deliberately match on substrings: cargo's exact phrasing of status lines and
//! diagnostics varies between releases, and pinning it would make this suite a version tripwire
//! instead of a regression net.

use std::path::Path;
use std::process::{Command, Output};

use anyhow::Context;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_cargo-dirty")
}

/// Writes a two-crate workspace with no external dependencies: `leaf` is a library and `root`
/// depends on it by path, so touching `leaf` must cascade into `root`.
fn fixture_workspace(dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir.join("leaf").join("src"))?;
    std::fs::create_dir_all(dir.join("root").join("src"))?;

    // The `[workspace]` table stops cargo from walking up into whatever workspace happens to
    // contain the system temp directory.
    std::fs::write(
        dir.join("Cargo.toml"),
        "[workspace]\nmembers = [\"leaf\", \"root\"]\nresolver = \"2\"\n",
    )?;

    std::fs::write(
        dir.join("leaf").join("Cargo.toml"),
        "[package]\nname = \"leaf\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    std::fs::write(
        dir.join("leaf").join("src").join("lib.rs"),
        "pub fn leaf() -> u32 {\n    1\n}\n",
    )?;

    std::fs::write(
        dir.join("root").join("Cargo.toml"),
        "[package]\nname = \"root\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\nleaf = { path = \"../leaf\" }\n",
    )?;
    std::fs::write(
        dir.join("root").join("src").join("lib.rs"),
        "pub fn root() -> u32 {\n    leaf::leaf() + 1\n}\n",
    )?;

    Ok(())
}

/// Adds a binary crate that echoes its own arguments, so tests can prove that injected cargo
/// flags never leak past the `--` separator.
fn add_bin_crate(dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir.join("printer").join("src"))?;

    std::fs::write(
        dir.join("Cargo.toml"),
        "[workspace]\nmembers = [\"leaf\", \"root\", \"printer\"]\nresolver = \"2\"\n",
    )?;
    std::fs::write(
        dir.join("printer").join("Cargo.toml"),
        "[package]\nname = \"printer\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    std::fs::write(
        dir.join("printer").join("src").join("main.rs"),
        "fn main() {\n    \
         let args: Vec<String> = std::env::args().skip(1).collect();\n    \
         println!(\"ARGS: {}\", args.join(\" \"));\n\
         }\n",
    )?;

    Ok(())
}

fn run_dirty(dir: &Path, args: &[&str]) -> anyhow::Result<Output> {
    Command::new(bin())
        .args(args)
        .current_dir(dir)
        .env("CARGO_TERM_COLOR", "never")
        .output()
        .context("failed to run the cargo-dirty binary")
}

/// The binary colors its output unconditionally, so tests compare against the plain text.
fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        if chars.next_if_eq(&'[').is_some() {
            // A CSI sequence runs until its final byte, which is the first one in `@`..=`~`.
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
        }
    }

    out
}

fn stdout_of(output: &Output) -> String {
    strip_ansi(&String::from_utf8_lossy(&output.stdout))
}

fn stderr_of(output: &Output) -> String {
    strip_ansi(&String::from_utf8_lossy(&output.stderr))
}

#[test]
fn first_check_reports_work() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    fixture_workspace(dir.path())?;

    let output = run_dirty(dir.path(), &["check"])?;
    let stdout = stdout_of(&output);

    assert!(output.status.success(), "cargo-dirty failed: {stdout}");
    assert!(stdout.contains("Checking leaf"), "stdout was: {stdout}");
    assert!(stdout.contains("Checking root"), "stdout was: {stdout}");
    assert!(stdout.contains("ok"), "stdout was: {stdout}");
    assert!(stdout.contains("work"), "stdout was: {stdout}");

    Ok(())
}

#[test]
fn second_check_is_all_fresh() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    fixture_workspace(dir.path())?;

    run_dirty(dir.path(), &["check"])?;
    let output = run_dirty(dir.path(), &["check"])?;
    let stdout = stdout_of(&output);

    assert!(output.status.success(), "cargo-dirty failed: {stdout}");
    assert!(!stdout.contains("Checking"), "stdout was: {stdout}");
    assert!(stdout.contains("fresh 2"), "stdout was: {stdout}");

    Ok(())
}

#[test]
fn show_fresh_lists_fresh_crates() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    fixture_workspace(dir.path())?;

    run_dirty(dir.path(), &["check"])?;

    let shown = run_dirty(dir.path(), &["--show-fresh", "check"])?;
    let shown_stdout = stdout_of(&shown);
    assert!(shown.status.success(), "cargo-dirty failed: {shown_stdout}");
    assert!(
        shown_stdout.contains("Fresh leaf"),
        "stdout was: {shown_stdout}"
    );
    assert!(
        shown_stdout.contains("Fresh root"),
        "stdout was: {shown_stdout}"
    );

    let hidden = run_dirty(dir.path(), &["check"])?;
    let hidden_stdout = stdout_of(&hidden);
    assert!(
        hidden.status.success(),
        "cargo-dirty failed: {hidden_stdout}"
    );
    assert!(
        !hidden_stdout.contains("Fresh"),
        "stdout was: {hidden_stdout}"
    );

    Ok(())
}

#[test]
fn touching_leaf_rebuilds_both() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    fixture_workspace(dir.path())?;

    run_dirty(dir.path(), &["check"])?;
    std::fs::write(
        dir.path().join("leaf").join("src").join("lib.rs"),
        "pub fn leaf() -> u32 {\n    1\n}\n\npub fn extra() -> u32 {\n    2\n}\n",
    )?;

    let output = run_dirty(dir.path(), &["check"])?;
    let stdout = stdout_of(&output);

    assert!(output.status.success(), "cargo-dirty failed: {stdout}");
    assert!(stdout.contains("Checking leaf"), "stdout was: {stdout}");
    assert!(stdout.contains("Checking root"), "stdout was: {stdout}");
    assert!(stdout.contains("reason"), "stdout was: {stdout}");

    Ok(())
}

#[test]
fn explain_names_the_culprit() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    fixture_workspace(dir.path())?;

    run_dirty(dir.path(), &["check"])?;
    std::fs::write(
        dir.path().join("leaf").join("src").join("lib.rs"),
        "pub fn leaf() -> u32 {\n    1\n}\n\npub fn extra() -> u32 {\n    2\n}\n",
    )?;

    let output = run_dirty(dir.path(), &["--explain", "check"])?;
    let stdout = stdout_of(&output);

    assert!(output.status.success(), "cargo-dirty failed: {stdout}");

    // The cascade below the culprit line depends on cargo's reason phrasing, so only the
    // culprit itself is pinned: the touched crate, not the one it invalidated downstream.
    let culprit = stdout
        .lines()
        .find(|line| line.starts_with("culprit"))
        .with_context(|| format!("no culprit line in: {stdout}"))?;
    assert!(
        culprit.contains("leaf v0.1.0"),
        "culprit line was: {culprit}"
    );

    Ok(())
}

#[test]
fn cargo_level_error_is_visible() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;

    let output = run_dirty(dir.path(), &["check"])?;
    let stderr = stderr_of(&output);

    assert!(!output.status.success(), "expected a non-zero exit status");
    assert!(stderr.contains("could not find"), "stderr was: {stderr}");

    Ok(())
}

#[test]
fn passthrough_args_reach_binary_unpolluted() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    fixture_workspace(dir.path())?;
    add_bin_crate(dir.path())?;

    let output = run_dirty(dir.path(), &["run", "-p", "printer", "--", "--user-flag"])?;
    let stdout = stdout_of(&output);

    assert!(output.status.success(), "cargo-dirty failed: {stdout}");

    let echoed = stdout
        .lines()
        .find(|line| line.starts_with("ARGS:"))
        .context("the fixture binary printed no argument line")?;

    assert!(echoed.contains("--user-flag"), "echoed args were: {echoed}");
    assert!(
        !echoed.contains("--message-format"),
        "echoed args were: {echoed}"
    );
    assert!(!echoed.contains("--jobs"), "echoed args were: {echoed}");

    Ok(())
}

#[test]
fn test_output_is_visible() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    fixture_workspace(dir.path())?;

    std::fs::write(
        dir.path().join("leaf").join("src").join("lib.rs"),
        "pub fn leaf() -> u32 {\n    1\n}\n\n\
         #[cfg(test)]\nmod tests {\n    \
         #[test]\n    fn ok() {\n        assert_eq!(super::leaf(), 1);\n    }\n\
         }\n",
    )?;

    let output = run_dirty(dir.path(), &["test", "-p", "leaf"])?;
    let stdout = stdout_of(&output);

    assert!(output.status.success(), "cargo-dirty failed: {stdout}");
    assert!(stdout.contains("1 passed"), "stdout was: {stdout}");

    Ok(())
}

#[test]
fn compile_error_prints_compiler_error() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    fixture_workspace(dir.path())?;

    std::fs::write(
        dir.path().join("leaf").join("src").join("lib.rs"),
        "pub fn leaf( -> {\n",
    )?;

    let output = run_dirty(dir.path(), &["check"])?;
    let stdout = stdout_of(&output);
    let stderr = stderr_of(&output);

    assert!(!output.status.success(), "expected a non-zero exit status");
    assert!(stderr.contains("error"), "stderr was: {stderr}");
    assert!(stdout.contains("failed"), "stdout was: {stdout}");

    Ok(())
}
