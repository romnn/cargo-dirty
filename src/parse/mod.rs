//! Turns raw lines of cargo's stderr into typed values.
//!
//! [`classify`] is the single entry point, and the only place in the crate where a line is
//! interpreted. Everything downstream consumes the types defined here, so the streamed view of a
//! build and the post-hoc analysis of it can never disagree about what a line meant.

pub mod fingerprint;
mod status;

/// A cargo build-unit identifier as it appears in `-vv` status lines, e.g. `serde v1.0.200`.
///
/// The field is private because an id is only meaningful when it came from a parsed cargo line;
/// nothing else in the pipeline is allowed to invent one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CrateId(String);

impl CrateId {
    /// The package name, without the version.
    pub fn name(&self) -> &str {
        self.0.split_whitespace().next().unwrap_or(&self.0)
    }

    /// Test fixtures need ids that never passed through a cargo line; production code only ever
    /// obtains a `CrateId` by parsing.
    #[cfg(test)]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl std::fmt::Display for CrateId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The kinds of work cargo reports doing on a crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkKind {
    Compiling,
    Checking,
    Building,
}

impl WorkKind {
    /// The word cargo itself prints for this kind of work.
    pub fn verb(self) -> &'static str {
        match self {
            Self::Compiling => "Compiling",
            Self::Checking => "Checking",
            Self::Building => "Building",
        }
    }
}

/// What cargo said about one build unit.
///
/// A unit is often reported twice — `Dirty` when its fingerprint is compared, then a work verb
/// when the compile actually starts — so these are events, not exclusive states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusKind {
    /// Up to date; cargo did nothing for this unit.
    Fresh,
    /// Invalidated, with cargo's explanation. The reason is not optional: cargo's `Dirty` line
    /// format always carries one, and a line without one is not a status line at all.
    Dirty { reason: String },
    /// Work started on this unit.
    Work(WorkKind),
}

/// One `-vv` status line: which unit, and what cargo said about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrateStatus {
    pub id: CrateId,
    pub kind: StatusKind,
}

/// What a single line of cargo's stderr turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StderrLine {
    Status(CrateStatus),
    FingerprintTrace(String),
    Other(String),
}

/// Classifies one raw stderr line.
pub fn classify(line: &str) -> StderrLine {
    let trimmed = line.trim_start();

    if let Some(status) = status::parse(trimmed) {
        return StderrLine::Status(status);
    }

    if fingerprint::is_fingerprint_trace_line(trimmed) {
        return StderrLine::FingerprintTrace(trimmed.to_owned());
    }

    // Unrecognised lines keep their original indentation: they are reproduced verbatim when a
    // failing build has no compiler diagnostics to show.
    StderrLine::Other(line.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn classifies_indented_status_lines() {
        assert_eq!(
            classify("       Fresh foo v0.1.0 (/tmp/foo)"),
            StderrLine::Status(CrateStatus {
                id: CrateId::new("foo v0.1.0"),
                kind: StatusKind::Fresh,
            })
        );
    }

    #[test]
    fn keeps_indentation_on_unrecognised_lines() {
        assert_eq!(
            classify("   warning: something happened"),
            StderrLine::Other("   warning: something happened".to_string())
        );
    }

    #[test]
    fn trims_indentation_from_fingerprint_trace_lines() {
        assert_eq!(
            classify("  TRACE cargo::core::compiler::fingerprint: err: changed"),
            StderrLine::FingerprintTrace(
                "TRACE cargo::core::compiler::fingerprint: err: changed".to_string()
            )
        );
    }

    #[test]
    fn classifies_run_and_finish_lines_as_other() {
        assert!(matches!(
            classify("     Running `rustc --crate-name foo`"),
            StderrLine::Other(_)
        ));
        assert!(matches!(
            classify("    Finished `dev` profile [unoptimized] target(s) in 0.10s"),
            StderrLine::Other(_)
        ));
    }
}
