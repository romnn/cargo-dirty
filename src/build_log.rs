//! The record of a single cargo run.
//!
//! [`BuildLog`] is both the streaming tracker and the accumulator: it decides what to show while
//! the build runs and keeps everything [`crate::explain`] and [`crate::report`] need afterwards.
//! Holding both in one place is what guarantees each stderr line is classified exactly once.

use std::collections::{BTreeMap, HashSet};

use crate::parse::{CrateId, CrateStatus, StatusKind, StderrLine, WorkKind, classify};

/// How many distinct crates fell into each bucket.
///
/// The buckets overlap and need not agree: cargo can declare a unit `Dirty` and then report no
/// work for it, so `dirty` is routinely larger than `work`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    /// Crates cargo reported as up to date.
    pub fresh: usize,
    /// Crates cargo reported as invalidated, whether or not work followed.
    pub dirty: usize,
    /// Crates cargo actually compiled, checked, or built.
    pub work: usize,
}

/// Something worth showing the user as the build unfolds.
///
/// Each event fires at most once per crate, on its first sighting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplayEvent {
    /// A crate cargo skipped as up to date.
    Fresh { id: CrateId },
    /// A crate cargo started working on. `reason` is present when cargo had already explained
    /// the invalidation by then.
    WorkStarted {
        kind: WorkKind,
        id: CrateId,
        reason: Option<String>,
    },
    /// A reason cargo reported only after the crate's work had already been announced, which is
    /// routine with parallel jobs.
    LateReason { id: CrateId, reason: String },
}

/// The single record of a cargo run; see the module docs for why it is one type.
#[derive(Debug, Default)]
pub struct BuildLog {
    fresh: HashSet<CrateId>,
    dirty: HashSet<CrateId>,
    work: HashSet<CrateId>,

    started: HashSet<CrateId>,
    reason_emitted: HashSet<CrateId>,
    reasons: BTreeMap<CrateId, String>,

    statuses: Vec<CrateStatus>,
    fingerprint_lines: Vec<String>,
    other_stderr: Vec<String>,
}

impl BuildLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one raw line of cargo's stderr and reports what it means for the live display.
    pub fn ingest_line(&mut self, line: &str) -> Vec<DisplayEvent> {
        let mut out = Vec::new();

        let status = match classify(line) {
            StderrLine::Status(status) => status,
            StderrLine::FingerprintTrace(line) => {
                self.fingerprint_lines.push(line);
                return out;
            }
            StderrLine::Other(line) => {
                self.other_stderr.push(line);
                return out;
            }
        };

        match &status.kind {
            StatusKind::Fresh => {
                if self.fresh.insert(status.id.clone()) {
                    out.push(DisplayEvent::Fresh {
                        id: status.id.clone(),
                    });
                }
            }
            StatusKind::Dirty { reason } => {
                self.dirty.insert(status.id.clone());

                // Cargo can report a unit dirty more than once; the first reason is the one
                // that explains the rebuild.
                if !self.reasons.contains_key(&status.id) {
                    self.reasons.insert(status.id.clone(), reason.clone());

                    if self.started.contains(&status.id)
                        && self.reason_emitted.insert(status.id.clone())
                    {
                        out.push(DisplayEvent::LateReason {
                            id: status.id.clone(),
                            reason: reason.clone(),
                        });
                    }
                }
            }
            StatusKind::Work(kind) => {
                self.work.insert(status.id.clone());

                if self.started.insert(status.id.clone()) {
                    let reason = self.reasons.get(&status.id).cloned();
                    if reason.is_some() {
                        self.reason_emitted.insert(status.id.clone());
                    }

                    out.push(DisplayEvent::WorkStarted {
                        kind: *kind,
                        id: status.id.clone(),
                        reason,
                    });
                }
            }
        }

        self.statuses.push(status);
        out
    }

    pub fn counts(&self) -> Counts {
        Counts {
            fresh: self.fresh.len(),
            dirty: self.dirty.len(),
            work: self.work.len(),
        }
    }

    /// Why each invalidated crate was invalidated, keyed by crate and holding the *first* reason
    /// cargo gave — the one that explains the rebuild rather than a later consequence of it.
    pub fn reasons(&self) -> &BTreeMap<CrateId, String> {
        &self.reasons
    }

    /// Every status line seen, in the order cargo emitted them.
    pub fn statuses(&self) -> &[CrateStatus] {
        &self.statuses
    }

    /// The fingerprint trace lines, empty unless `--deep` was passed.
    pub fn fingerprint_lines(&self) -> &[String] {
        &self.fingerprint_lines
    }

    /// Cargo's stderr lines that carried no crate status, kept verbatim with their original
    /// indentation so a failing build can be reported in cargo's own words.
    pub fn other_stderr(&self) -> &[String] {
        &self.other_stderr
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn emits_work_started_only_once() {
        let mut log = BuildLog::new();

        let e1 = log.ingest_line("Compiling foo v0.1.0 (/tmp/foo)");
        let e2 = log.ingest_line("Compiling foo v0.1.0 (/tmp/foo)");

        assert_eq!(e1.len(), 1);
        assert_eq!(e2.len(), 0);

        assert_eq!(
            log.counts(),
            Counts {
                fresh: 0,
                dirty: 0,
                work: 1
            }
        );
    }

    #[test]
    fn captures_indented_fresh_and_counts_it() {
        let mut log = BuildLog::new();
        let events = log.ingest_line("       Fresh bar v0.2.0 (/tmp/bar)");

        assert_eq!(
            events,
            vec![DisplayEvent::Fresh {
                id: CrateId::new("bar v0.2.0"),
            }]
        );
        assert_eq!(log.counts().fresh, 1);
        assert_eq!(log.statuses().len(), 1);
        assert_eq!(log.statuses()[0].id, CrateId::new("bar v0.2.0"));
    }

    #[test]
    fn emits_fresh_only_once_per_crate() {
        let mut log = BuildLog::new();

        let e1 = log.ingest_line("Fresh bar v0.2.0 (/tmp/bar)");
        let e2 = log.ingest_line("Fresh bar v0.2.0 (/tmp/bar)");

        assert_eq!(e1.len(), 1);
        assert_eq!(e2.len(), 0);
        assert_eq!(log.counts().fresh, 1);
    }

    #[test]
    fn attaches_reason_if_known_before_work() {
        let mut log = BuildLog::new();
        let _ = log.ingest_line("Dirty foo v0.1.0 (/tmp/foo): the file `src/lib.rs` has changed");
        let events = log.ingest_line("Compiling foo v0.1.0 (/tmp/foo)");

        assert_eq!(
            events,
            vec![DisplayEvent::WorkStarted {
                kind: WorkKind::Compiling,
                id: CrateId::new("foo v0.1.0"),
                reason: Some("the file `src/lib.rs` has changed".to_string()),
            }]
        );
    }

    #[test]
    fn emits_reason_update_if_reason_arrives_after_work_started() {
        let mut log = BuildLog::new();
        let started = log.ingest_line("Checking foo v0.1.0 (/tmp/foo)");
        assert_eq!(started.len(), 1);

        let late = log.ingest_line("Dirty foo v0.1.0 (/tmp/foo): the file `build.rs` has changed");

        assert_eq!(
            late,
            vec![DisplayEvent::LateReason {
                id: CrateId::new("foo v0.1.0"),
                reason: "the file `build.rs` has changed".to_string(),
            }]
        );
    }

    #[test]
    fn records_status_lines_and_extracts_reason() {
        let mut log = BuildLog::new();
        log.ingest_line("Dirty foo v0.1.0 (/tmp/foo): the file `src/lib.rs` has changed");

        assert_eq!(log.statuses().len(), 1);
        assert_eq!(
            log.reasons().get(&CrateId::new("foo v0.1.0")),
            Some(&"the file `src/lib.rs` has changed".to_string())
        );
    }

    #[test]
    fn records_fingerprint_trace_lines() {
        let mut log = BuildLog::new();
        let events = log
            .ingest_line("TRACE cargo::core::compiler::fingerprint: compare fingerprints for foo");

        assert_eq!(events, Vec::new());
        assert_eq!(log.fingerprint_lines().len(), 1);
    }

    #[test]
    fn collects_unrelated_lines_for_failure_dump() {
        let mut log = BuildLog::new();
        let events = log.ingest_line("warning: something happened");

        assert_eq!(events, Vec::new());
        assert_eq!(log.other_stderr(), ["warning: something happened"]);
    }

    fn ingest_all(log: &mut BuildLog, transcript: &str) -> Vec<DisplayEvent> {
        transcript
            .lines()
            .flat_map(|line| log.ingest_line(line))
            .collect()
    }

    /// A run of `cargo check -vv` on a two-crate workspace after touching the leaf crate,
    /// abridged but otherwise verbatim. `build-only` stands in for a unit cargo declares dirty
    /// without separately announcing work for it.
    const TRANSCRIPT: &str = "\
       Fresh unicode-ident v1.0.12
       Fresh serde v1.0.200
       Dirty leaf v0.1.0 (/tmp/w/leaf): the file `leaf/src/lib.rs` has changed (1785346860.4s, 8s after last build at 1785346852.3s)
    Checking leaf v0.1.0 (/tmp/w/leaf)
     Running `rustc --crate-name leaf --edition=2021 leaf/src/lib.rs --crate-type lib`
       Fresh serde v1.0.200
       Dirty build-only v0.3.0 (/tmp/w/build-only): the profile configuration changed
       Dirty root v0.1.0 (/tmp/w/root): the dependency `leaf` was rebuilt
    Checking root v0.1.0 (/tmp/w/root)
     Running `rustc --crate-name root --edition=2021 root/src/lib.rs --crate-type lib`
TRACE cargo::core::compiler::fingerprint: fingerprint error for leaf v0.1.0/Unit
warning: unused manifest key: package.foo
warning: leaf@0.1.0: cc: warning: unknown option

    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.42s
";

    #[test]
    fn counts_a_whole_transcript() {
        let mut log = BuildLog::new();
        let _ = ingest_all(&mut log, TRANSCRIPT);

        // `dirty` exceeds `work` because cargo declared `build-only` stale without reporting a
        // work unit for it.
        assert_eq!(
            log.counts(),
            Counts {
                fresh: 2,
                dirty: 3,
                work: 2
            }
        );
    }

    #[test]
    fn emits_one_event_per_first_sighting_across_a_transcript() {
        let mut log = BuildLog::new();
        let events = ingest_all(&mut log, TRANSCRIPT);

        assert_eq!(
            events,
            vec![
                DisplayEvent::Fresh {
                    id: CrateId::new("unicode-ident v1.0.12"),
                },
                DisplayEvent::Fresh {
                    id: CrateId::new("serde v1.0.200"),
                },
                DisplayEvent::WorkStarted {
                    kind: WorkKind::Checking,
                    id: CrateId::new("leaf v0.1.0"),
                    reason: Some(
                        "the file `leaf/src/lib.rs` has changed (1785346860.4s, 8s after last build at 1785346852.3s)"
                            .to_string()
                    ),
                },
                DisplayEvent::WorkStarted {
                    kind: WorkKind::Checking,
                    id: CrateId::new("root v0.1.0"),
                    reason: Some("the dependency `leaf` was rebuilt".to_string()),
                },
            ]
        );
    }

    #[test]
    fn keeps_every_reason_from_a_transcript() {
        let mut log = BuildLog::new();
        let _ = ingest_all(&mut log, TRANSCRIPT);

        assert_eq!(
            log.reasons(),
            &BTreeMap::from([
                (
                    CrateId::new("build-only v0.3.0"),
                    "the profile configuration changed".to_string()
                ),
                (
                    CrateId::new("leaf v0.1.0"),
                    "the file `leaf/src/lib.rs` has changed (1785346860.4s, 8s after last build at 1785346852.3s)"
                        .to_string()
                ),
                (
                    CrateId::new("root v0.1.0"),
                    "the dependency `leaf` was rebuilt".to_string()
                ),
            ])
        );
    }

    #[test]
    fn keeps_unrecognised_transcript_lines_verbatim() {
        let mut log = BuildLog::new();
        let _ = ingest_all(&mut log, TRANSCRIPT);

        assert_eq!(
            log.other_stderr(),
            [
                "     Running `rustc --crate-name leaf --edition=2021 leaf/src/lib.rs --crate-type lib`",
                "     Running `rustc --crate-name root --edition=2021 root/src/lib.rs --crate-type lib`",
                "warning: unused manifest key: package.foo",
                "warning: leaf@0.1.0: cc: warning: unknown option",
                "",
                "    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.42s",
            ]
        );
        assert_eq!(
            log.fingerprint_lines(),
            ["TRACE cargo::core::compiler::fingerprint: fingerprint error for leaf v0.1.0/Unit"]
        );
    }

    /// With parallel jobs a `Dirty` line can land after other crates have already been
    /// announced, which is what `LateReason` exists to disambiguate.
    const PARALLEL_TRANSCRIPT: &str = "\
    Checking leaf v0.1.0 (/tmp/w/leaf)
    Checking root v0.1.0 (/tmp/w/root)
       Dirty leaf v0.1.0 (/tmp/w/leaf): the file `leaf/src/lib.rs` has changed
";

    #[test]
    fn attributes_a_reason_that_arrives_after_other_crates_started() {
        let mut log = BuildLog::new();
        let events = ingest_all(&mut log, PARALLEL_TRANSCRIPT);

        assert_eq!(
            events,
            vec![
                DisplayEvent::WorkStarted {
                    kind: WorkKind::Checking,
                    id: CrateId::new("leaf v0.1.0"),
                    reason: None,
                },
                DisplayEvent::WorkStarted {
                    kind: WorkKind::Checking,
                    id: CrateId::new("root v0.1.0"),
                    reason: None,
                },
                DisplayEvent::LateReason {
                    id: CrateId::new("leaf v0.1.0"),
                    reason: "the file `leaf/src/lib.rs` has changed".to_string(),
                },
            ]
        );
    }
}
