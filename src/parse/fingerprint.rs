//! Cargo's fingerprint trace, as enabled by `--deep`.
//!
//! Setting `CARGO_LOG=cargo::core::compiler::fingerprint=trace` makes cargo log why each unit's
//! fingerprint comparison failed. The output is a best-effort debugging aid rather than a stable
//! format, so everything here degrades to "no details" instead of failing.

use std::collections::BTreeMap;

use super::CrateId;

/// Whether a stderr line belongs to the fingerprint trace.
pub fn is_fingerprint_trace_line(line: &str) -> bool {
    line.contains("cargo::core::compiler::fingerprint")
}

/// Strips the log framing (`TRACE <target>:` or `[<target>]`) off a trace line, leaving the
/// message cargo actually wrote.
fn payload(line: &str) -> &str {
    let trimmed = line.trim();

    if let Some((_, rest)) = trimmed.split_once("cargo::core::compiler::fingerprint]") {
        return rest.trim();
    }

    if let Some((_, rest)) = trimmed.split_once("cargo::core::compiler::fingerprint:") {
        return rest.trim();
    }

    trimmed
}

/// Header lines that open a block of detail lines. Cargo renamed this message, so both spellings
/// are recognised to keep `--deep` working across cargo versions.
const HEADER_PREFIXES: [&str; 2] = ["fingerprint error for ", "fingerprint dirty for "];

/// Line prefixes that carry a detail worth showing, paired with whether the prefix itself is kept
/// in the rendered text. `Caused by:` reads as prose and stays; the others are labels.
const DETAIL_PREFIXES: [(&str, bool); 4] = [
    ("err:", false),
    ("dirty:", false),
    ("Caused by:", true),
    ("Caused by", false),
];

/// Reads the unit id out of a header line.
///
/// Cargo writes the package's source path between the version and the unit description, and the
/// separators differ between versions, so the id is taken from the first two whitespace-separated
/// tokens rather than by splitting on `/`.
fn header_crate_id(line: &str) -> Option<CrateId> {
    let remainder = HEADER_PREFIXES
        .into_iter()
        .find_map(|prefix| line.strip_prefix(prefix))?;

    let mut parts = remainder.split_whitespace();
    let name = parts.next()?;
    let version = parts.next()?.split('/').next()?;

    version
        .starts_with('v')
        .then(|| CrateId(format!("{name} {version}")))
}

/// Groups `--deep` fingerprint trace lines by the crate whose fingerprint comparison failed.
///
/// The trace is a flat stream: a header names a crate, and the detail lines that follow belong
/// to it until the next header. Lines cargo emits before any header — and any line that is not a
/// recognised detail — are dropped.
pub fn parse_details(lines: &[String]) -> BTreeMap<CrateId, Vec<String>> {
    let mut out: BTreeMap<CrateId, Vec<String>> = BTreeMap::new();
    let mut current: Option<CrateId> = None;

    for raw in lines {
        let line = payload(raw);

        if let Some(crate_id) = header_crate_id(line) {
            current = Some(crate_id);
            continue;
        }

        let Some(crate_id) = current.clone() else {
            continue;
        };

        let Some(detail) = DETAIL_PREFIXES
            .into_iter()
            .find_map(|(prefix, keep_prefix)| {
                let rest = line.strip_prefix(prefix)?.trim();
                if rest.is_empty() {
                    return None;
                }
                Some(if keep_prefix {
                    format!("{prefix} {rest}")
                } else {
                    rest.to_owned()
                })
            })
        else {
            continue;
        };

        out.entry(crate_id).or_default().push(detail);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn detects_trace_lines() {
        assert_eq!(
            is_fingerprint_trace_line("TRACE cargo::core::compiler::fingerprint: some message"),
            true
        );
        assert_eq!(is_fingerprint_trace_line("Compiling foo v0.1.0"), false);
    }

    #[test]
    fn parses_details_for_a_crate() {
        let lines = vec![
            "TRACE cargo::core::compiler::fingerprint: fingerprint error for a v0.1.0/Build/TargetInner { .. }".to_string(),
            "TRACE cargo::core::compiler::fingerprint: err: unit dependency information changed".to_string(),
            "TRACE cargo::core::compiler::fingerprint: Caused by: new (x) != old (y)".to_string(),
        ];

        let details = parse_details(&lines);
        let a = details.get(&CrateId::new("a v0.1.0")).unwrap();

        assert_eq!(a.len(), 2);
        assert_eq!(a[0], "unit dependency information changed".to_string());
        assert_eq!(a[1], "Caused by: new (x) != old (y)".to_string());
    }

    /// Cargo 1.97 writes `fingerprint dirty for`, puts the package path between the version and
    /// the unit description, and labels the detail `dirty:`.
    #[test]
    fn parses_details_from_the_current_cargo_phrasing() {
        let lines = vec![
            "   0.006s  INFO prepare_target{package_id=leaf v0.1.0 (/w/leaf)}: cargo::core::compiler::fingerprint: fingerprint dirty for leaf v0.1.0 (/w/leaf)/Check { test: false }/TargetInner { .. }".to_string(),
            "   0.006s  INFO prepare_target{package_id=leaf v0.1.0 (/w/leaf)}: cargo::core::compiler::fingerprint:     dirty: FsStatusOutdated(StaleItem(ChangedFile { stale: \"/w/leaf/src/lib.rs\" }))".to_string(),
        ];

        let details = parse_details(&lines);
        let leaf = details.get(&CrateId::new("leaf v0.1.0")).unwrap();

        assert_eq!(leaf.len(), 1);
        assert_eq!(
            leaf[0],
            "FsStatusOutdated(StaleItem(ChangedFile { stale: \"/w/leaf/src/lib.rs\" }))"
                .to_string()
        );
    }

    /// The lines cargo logs while deciding staleness arrive before the header that names the
    /// crate, so they must not be attributed to whichever crate was named previously.
    #[test]
    fn ignores_trace_lines_that_are_not_recognised_details() {
        let lines = vec![
            "TRACE cargo::core::compiler::fingerprint: fingerprint dirty for a v0.1.0 (/w/a)/Check"
                .to_string(),
            "TRACE cargo::core::compiler::fingerprint: err: really changed".to_string(),
            "TRACE cargo::core::compiler::fingerprint: fingerprint at: /w/target/.fingerprint/b"
                .to_string(),
            "TRACE cargo::core::compiler::fingerprint: stale: changed \"/w/b/src/lib.rs\""
                .to_string(),
        ];

        let details = parse_details(&lines);

        assert_eq!(
            details.get(&CrateId::new("a v0.1.0")),
            Some(&vec!["really changed".to_string()])
        );
    }

    #[test]
    fn ignores_detail_lines_before_any_header() {
        let lines =
            vec!["TRACE cargo::core::compiler::fingerprint: err: orphaned detail".to_string()];

        assert_eq!(parse_details(&lines), BTreeMap::new());
    }

    #[test]
    fn strips_bracketed_log_framing() {
        assert_eq!(
            payload("2026-07-29 [cargo::core::compiler::fingerprint] err: changed"),
            "err: changed"
        );
    }
}
