use std::collections::{BTreeMap, HashSet};

use crate::parse::{CrateStatusKind, parse_status_line};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    pub fresh: usize,
    pub dirty: usize,
    pub work: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    WorkStarted {
        kind: CrateStatusKind,
        crate_id: String,
        reason: Option<String>,
    },
    WorkReason {
        crate_id: String,
        reason: String,
    },
}

#[derive(Debug, Default)]
pub struct Engine {
    fresh: HashSet<String>,
    dirty: HashSet<String>,
    work: HashSet<String>,

    started: HashSet<String>,
    reason_emitted: HashSet<String>,
    reasons: BTreeMap<String, String>,
}

impl Engine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn counts(&self) -> Counts {
        Counts {
            fresh: self.fresh.len(),
            dirty: self.dirty.len(),
            work: self.work.len(),
        }
    }

    pub fn ingest_stderr_line(&mut self, line: &str) -> Vec<Event> {
        let mut out = Vec::new();

        let line_trimmed = line.trim_start();
        let Some(ev) = parse_status_line(line_trimmed) else {
            return out;
        };

        if ev.crate_id.starts_with('<') {
            return out;
        }

        match ev.kind {
            CrateStatusKind::Fresh => {
                self.fresh.insert(ev.crate_id);
            }
            CrateStatusKind::Dirty => {
                self.dirty.insert(ev.crate_id.clone());
                if let Some(reason) = ev.reason {
                    let crate_id = ev.crate_id;

                    let already_had_reason = self.reasons.contains_key(&crate_id);
                    if !already_had_reason {
                        self.reasons.insert(crate_id.clone(), reason.clone());

                        if self.started.contains(&crate_id)
                            && self.reason_emitted.insert(crate_id.clone())
                        {
                            out.push(Event::WorkReason { crate_id, reason });
                        }
                    }
                }
            }
            CrateStatusKind::Compiling | CrateStatusKind::Checking | CrateStatusKind::Building => {
                self.work.insert(ev.crate_id.clone());

                if self.started.insert(ev.crate_id.clone()) {
                    let reason = self.reasons.get(&ev.crate_id).cloned();
                    if reason.is_some() {
                        self.reason_emitted.insert(ev.crate_id.clone());
                    }

                    out.push(Event::WorkStarted {
                        kind: ev.kind,
                        crate_id: ev.crate_id,
                        reason,
                    });
                }
            }
            _ => {}
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn emits_work_started_only_once() {
        let mut eng = Engine::new();

        let e1 = eng.ingest_stderr_line("Compiling foo v0.1.0 (/tmp/foo)");
        let e2 = eng.ingest_stderr_line("Compiling foo v0.1.0 (/tmp/foo)");

        assert_eq!(e1.len(), 1);
        assert_eq!(e2.len(), 0);

        assert_eq!(
            eng.counts(),
            Counts {
                fresh: 0,
                dirty: 0,
                work: 1
            }
        );
    }

    #[test]
    fn captures_indented_fresh_and_counts_it() {
        let mut eng = Engine::new();
        let evs = eng.ingest_stderr_line("       Fresh bar v0.2.0 (/tmp/bar)");
        assert_eq!(evs.len(), 0);
        assert_eq!(eng.counts().fresh, 1);
    }

    #[test]
    fn attaches_reason_if_known_before_work() {
        let mut eng = Engine::new();
        let _ = eng
            .ingest_stderr_line("Dirty foo v0.1.0 (/tmp/foo): the file `src/lib.rs` has changed");
        let evs = eng.ingest_stderr_line("Compiling foo v0.1.0 (/tmp/foo)");

        assert_eq!(
            evs,
            vec![Event::WorkStarted {
                kind: CrateStatusKind::Compiling,
                crate_id: "foo v0.1.0".to_string(),
                reason: Some("the file `src/lib.rs` has changed".to_string()),
            }]
        );
    }

    #[test]
    fn emits_reason_update_if_reason_arrives_after_work_started() {
        let mut eng = Engine::new();
        let evs1 = eng.ingest_stderr_line("Checking foo v0.1.0 (/tmp/foo)");
        assert_eq!(evs1.len(), 1);

        let evs2 =
            eng.ingest_stderr_line("Dirty foo v0.1.0 (/tmp/foo): the file `build.rs` has changed");

        assert_eq!(
            evs2,
            vec![Event::WorkReason {
                crate_id: "foo v0.1.0".to_string(),
                reason: "the file `build.rs` has changed".to_string(),
            }]
        );
    }
}
