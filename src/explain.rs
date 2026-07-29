//! Infers which change started a rebuild cascade.
//!
//! Cargo says *that* a crate was invalidated and, in prose, roughly why. This module reads those
//! reasons as cause edges between crates, finds the one crate nothing else invalidated, and
//! collects everything that traces back to it. It is inference over free text, so it is
//! best-effort by nature and declines to guess when the text is ambiguous.

use std::collections::{BTreeMap, HashMap};

use crate::build_log::BuildLog;
use crate::parse::{CrateId, CrateStatus, StatusKind, WorkKind, fingerprint};

/// The rebuild story: one crate blamed for it, and every crate that followed from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Analysis {
    pub culprit: CrateId,
    pub cascade: Vec<CascadeEntry>,
}

/// One crate in the cascade, in the order cargo first reported work for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CascadeEntry {
    pub crate_id: CrateId,
    pub kind: WorkKind,
    /// Cargo's own explanation, absent when cargo reported work without a `Dirty` line.
    pub reason: Option<String>,
    /// The crate this one was invalidated by; absent for the culprit and whenever the reason was
    /// too ambiguous to attribute.
    pub caused_by: Option<CrateId>,
    /// Fingerprint-trace details, non-empty only under `--deep`.
    pub details: Vec<String>,
}

enum Culprit {
    /// A crate no other crate is known to have invalidated: a genuine root of the rebuild.
    Root(CrateId),
    /// Every crate was blamed on another one, so cargo's reasons form a cycle and no root
    /// exists. The first crate that did work is reported on its own instead.
    Fallback(CrateId),
}

/// Returns `None` when the build did no work at all, and so has nothing to explain.
pub fn analyze(log: &BuildLog) -> Option<Analysis> {
    let work = work_entries(log.statuses());
    let causes = infer_causes_from_reasons(&work, log.reasons());
    let details = fingerprint::parse_details(log.fingerprint_lines());

    let entry = |crate_id: &CrateId, kind: WorkKind| CascadeEntry {
        crate_id: crate_id.clone(),
        kind,
        reason: log.reasons().get(crate_id).cloned(),
        caused_by: causes.get(crate_id).cloned(),
        details: details.get(crate_id).cloned().unwrap_or_default(),
    };

    let (culprit, cascade) = match infer_culprit(&work, &causes)? {
        Culprit::Root(culprit) => {
            let cascade = work
                .iter()
                .filter(|(crate_id, _)| reaches_culprit(crate_id, &culprit, &causes))
                .map(|(crate_id, kind)| entry(crate_id, *kind))
                .collect();
            (culprit, cascade)
        }
        Culprit::Fallback(culprit) => {
            let cascade = work
                .iter()
                .find(|(crate_id, _)| *crate_id == culprit)
                .map(|(crate_id, kind)| entry(crate_id, *kind))
                .into_iter()
                .collect();
            (culprit, cascade)
        }
    };

    Some(Analysis { culprit, cascade })
}

/// The crates that did work, in the order cargo first reported them.
fn work_entries(statuses: &[CrateStatus]) -> Vec<(CrateId, WorkKind)> {
    let mut seen: std::collections::HashSet<CrateId> = std::collections::HashSet::new();
    let mut work = Vec::new();

    for status in statuses {
        if let StatusKind::Work(kind) = status.kind
            && seen.insert(status.id.clone())
        {
            work.push((status.id.clone(), kind));
        }
    }

    work
}

/// Reads cargo's invalidation reasons as cause edges: "dependency `x` was rebuilt" means this
/// crate was invalidated by `x`. Every crate has at most one cause.
fn infer_causes_from_reasons(
    work: &[(CrateId, WorkKind)],
    reasons: &BTreeMap<CrateId, String>,
) -> HashMap<CrateId, CrateId> {
    let mut ids_by_name: HashMap<&str, Vec<&CrateId>> = HashMap::new();
    for (crate_id, _) in work {
        ids_by_name
            .entry(crate_id.name())
            .or_default()
            .push(crate_id);
    }

    let mut causes = HashMap::new();

    for (crate_id, _) in work {
        let Some(reason) = reasons.get(crate_id) else {
            continue;
        };

        let Some(dep_name) = extract_dependency_name(reason) else {
            continue;
        };

        // Cargo names the dependency but not its version. With two versions of one crate in the
        // same build, picking either would misattribute the cause, so decline to guess.
        let Some([dep_id]) = ids_by_name.get(dep_name).map(Vec::as_slice) else {
            continue;
        };

        if *dep_id != crate_id {
            causes.insert(crate_id.clone(), (*dep_id).clone());
        }
    }

    causes
}

fn infer_culprit(
    work: &[(CrateId, WorkKind)],
    causes: &HashMap<CrateId, CrateId>,
) -> Option<Culprit> {
    if let Some((crate_id, _)) = work
        .iter()
        .find(|(crate_id, _)| !causes.contains_key(crate_id))
    {
        return Some(Culprit::Root(crate_id.clone()));
    }

    work.first().map(|(id, _)| Culprit::Fallback(id.clone()))
}

fn reaches_culprit(start: &CrateId, culprit: &CrateId, causes: &HashMap<CrateId, CrateId>) -> bool {
    let mut current = start;

    // Each crate has at most one cause, so reachability is a walk along a single chain. A chain
    // longer than the cause map must have revisited a node, so stop there: a cycle that never
    // met the culprit cannot reach it.
    for _ in 0..=causes.len() {
        if current == culprit {
            return true;
        }

        match causes.get(current) {
            Some(next) => current = next,
            None => return false,
        }
    }

    false
}

fn extract_dependency_name(reason: &str) -> Option<&str> {
    ["dependency on `", "dependency `"]
        .into_iter()
        .find_map(|marker| {
            let (_, remainder) = reason.split_once(marker)?;
            let (name, _) = remainder.split_once('`')?;
            (!name.is_empty()).then_some(name)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn log_from(lines: &[&str]) -> BuildLog {
        let mut log = BuildLog::new();
        for line in lines {
            let _ = log.ingest_line(line);
        }
        log
    }

    #[test]
    fn picks_first_root_as_culprit_and_builds_cascade() {
        let log = log_from(&[
            "Dirty a v0.1.0 (/tmp/a): the file `src/lib.rs` has changed",
            "Compiling a v0.1.0 (/tmp/a)",
            "Dirty b v0.1.0 (/tmp/b): dependency on `a` is newer than we are",
            "Compiling b v0.1.0 (/tmp/b)",
        ]);

        let a = analyze(&log).unwrap();
        assert_eq!(a.culprit, CrateId::new("a v0.1.0"));
        assert_eq!(a.cascade.len(), 2);
        assert_eq!(a.cascade[0].crate_id, CrateId::new("a v0.1.0"));
        assert_eq!(a.cascade[0].caused_by, None);
        assert_eq!(a.cascade[1].crate_id, CrateId::new("b v0.1.0"));
        assert_eq!(a.cascade[1].caused_by, Some(CrateId::new("a v0.1.0")));
    }

    #[test]
    fn falls_back_to_first_work_crate_if_no_roots_can_be_inferred() {
        let log = log_from(&[
            "Dirty a v0.1.0 (/tmp/a): dependency on `b` is newer than we are",
            "Compiling a v0.1.0 (/tmp/a)",
            "Dirty b v0.1.0 (/tmp/b): dependency on `a` is newer than we are",
            "Compiling b v0.1.0 (/tmp/b)",
        ]);

        let a = analyze(&log).unwrap();
        assert_eq!(a.culprit, CrateId::new("a v0.1.0"));
        assert_eq!(a.cascade.len(), 1);
        assert_eq!(a.cascade[0].crate_id, CrateId::new("a v0.1.0"));
    }

    #[test]
    fn has_nothing_to_explain_when_no_crate_did_work() {
        let log = log_from(&["Fresh a v0.1.0 (/tmp/a)"]);
        assert!(analyze(&log).is_none());
    }

    #[test]
    fn declines_to_infer_a_cause_when_the_dependency_name_is_ambiguous() {
        let work = vec![
            (CrateId::new("syn v1.0.0"), WorkKind::Compiling),
            (CrateId::new("syn v2.0.0"), WorkKind::Compiling),
            (CrateId::new("c v0.1.0"), WorkKind::Compiling),
        ];

        let mut reasons = BTreeMap::new();
        reasons.insert(
            CrateId::new("c v0.1.0"),
            "dependency on `syn` is newer than we are".to_string(),
        );

        assert_eq!(infer_causes_from_reasons(&work, &reasons), HashMap::new());
    }

    #[test]
    fn infers_a_cause_when_exactly_one_version_of_the_dependency_was_built() {
        let work = vec![
            (CrateId::new("syn v2.0.0"), WorkKind::Compiling),
            (CrateId::new("c v0.1.0"), WorkKind::Compiling),
        ];

        let mut reasons = BTreeMap::new();
        reasons.insert(
            CrateId::new("c v0.1.0"),
            "dependency on `syn` is newer than we are".to_string(),
        );

        let causes = infer_causes_from_reasons(&work, &reasons);
        assert_eq!(
            causes.get(&CrateId::new("c v0.1.0")),
            Some(&CrateId::new("syn v2.0.0"))
        );
    }

    #[test]
    fn a_cause_cycle_that_misses_the_culprit_terminates() {
        let mut causes = HashMap::new();
        causes.insert(CrateId::new("a v0.1.0"), CrateId::new("b v0.1.0"));
        causes.insert(CrateId::new("b v0.1.0"), CrateId::new("a v0.1.0"));

        assert_eq!(
            reaches_culprit(
                &CrateId::new("a v0.1.0"),
                &CrateId::new("z v0.1.0"),
                &causes
            ),
            false
        );
    }
}
