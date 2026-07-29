use std::collections::{BTreeMap, HashMap};

use regex::Regex;

use crate::parse::{CrateStatusEvent, CrateStatusKind, ParsedCargoOutput};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Analysis {
    pub culprit: Option<String>,
    pub cascade: Vec<CascadeEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CascadeEntry {
    pub crate_id: String,
    pub kind: CrateStatusKind,
    pub reason: Option<String>,
    pub caused_by: Option<String>,
    pub details: Vec<String>,
}

pub fn analyze(parsed: &ParsedCargoOutput) -> Analysis {
    let work = work_entries(&parsed.stderr_events);
    let name_to_id = crate_name_to_id(&work);

    let causes = infer_causes_from_reasons(&work, &name_to_id, &parsed.crate_reasons);
    let fingerprint_details = parse_fingerprint_details(&parsed.fingerprint_lines);

    let Some((culprit_id, is_fallback)) = infer_culprit(&work, &causes) else {
        return Analysis {
            culprit: None,
            cascade: Vec::new(),
        };
    };

    if is_fallback {
        let entry = work
            .iter()
            .find(|(id, _)| id == &culprit_id)
            .map(|(id, kind)| CascadeEntry {
                crate_id: id.clone(),
                kind: kind.clone(),
                reason: parsed.crate_reasons.get(id).cloned(),
                caused_by: causes.get(id).cloned(),
                details: fingerprint_details.get(id).cloned().unwrap_or_default(),
            })
            .into_iter()
            .collect::<Vec<_>>();

        return Analysis {
            culprit: Some(culprit_id),
            cascade: entry,
        };
    }

    let mut memo: HashMap<String, bool> = HashMap::new();
    let mut cascade: Vec<CascadeEntry> = Vec::new();

    for (crate_id, kind) in &work {
        if reaches_culprit(crate_id, &culprit_id, &causes, &mut memo) {
            cascade.push(CascadeEntry {
                crate_id: crate_id.clone(),
                kind: kind.clone(),
                reason: parsed.crate_reasons.get(crate_id).cloned(),
                caused_by: causes.get(crate_id).cloned(),
                details: fingerprint_details
                    .get(crate_id)
                    .cloned()
                    .unwrap_or_default(),
            });
        }
    }

    Analysis {
        culprit: Some(culprit_id),
        cascade,
    }
}

fn work_entries(events: &[CrateStatusEvent]) -> Vec<(String, CrateStatusKind)> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut work = Vec::new();

    for ev in events {
        match ev.kind {
            CrateStatusKind::Compiling | CrateStatusKind::Checking | CrateStatusKind::Building => {
                if seen.insert(ev.crate_id.clone()) {
                    work.push((ev.crate_id.clone(), ev.kind.clone()));
                }
            }
            _ => {}
        }
    }

    work
}

fn crate_name_to_id(work: &[(String, CrateStatusKind)]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for (crate_id, _) in work {
        let Some(name) = crate_id.split_whitespace().next() else {
            continue;
        };
        out.insert(name.to_string(), crate_id.clone());
    }
    out
}

fn fingerprint_payload(line: &str) -> &str {
    let trimmed = line.trim();

    if let Some((_, rest)) = trimmed.split_once("cargo::core::compiler::fingerprint]") {
        return rest.trim();
    }

    if let Some((_, rest)) = trimmed.split_once("cargo::core::compiler::fingerprint:") {
        return rest.trim();
    }

    trimmed
}

fn infer_causes_from_reasons(
    work: &[(String, CrateStatusKind)],
    name_to_id: &HashMap<String, String>,
    reasons: &BTreeMap<String, String>,
) -> HashMap<String, String> {
    let mut causes = HashMap::new();

    for (crate_id, _) in work {
        let Some(reason) = reasons.get(crate_id) else {
            continue;
        };

        let Some(dep_name) = extract_dependency_name(reason) else {
            continue;
        };

        let Some(dep_id) = name_to_id.get(dep_name) else {
            continue;
        };

        if dep_id != crate_id {
            causes.insert(crate_id.clone(), dep_id.clone());
        }
    }

    causes
}

fn infer_culprit(
    work: &[(String, CrateStatusKind)],
    causes: &HashMap<String, String>,
) -> Option<(String, bool)> {
    for (crate_id, _) in work {
        if !causes.contains_key(crate_id) {
            return Some((crate_id.clone(), false));
        }
    }

    work.first().map(|(id, _)| (id.clone(), true))
}

fn reaches_culprit(
    crate_id: &str,
    culprit_id: &str,
    causes: &HashMap<String, String>,
    memo: &mut HashMap<String, bool>,
) -> bool {
    if crate_id == culprit_id {
        return true;
    }

    if let Some(v) = memo.get(crate_id) {
        return *v;
    }

    let Some(cause) = causes.get(crate_id) else {
        memo.insert(crate_id.to_string(), false);
        return false;
    };

    if cause == crate_id {
        memo.insert(crate_id.to_string(), false);
        return false;
    }

    if memo.contains_key(cause) {
        let v = memo.get(cause).copied().unwrap_or(false);
        memo.insert(crate_id.to_string(), v);
        return v;
    }

    let mut stack_guard = std::collections::HashSet::new();
    let v = reaches_culprit_inner(crate_id, culprit_id, causes, memo, &mut stack_guard);
    memo.insert(crate_id.to_string(), v);
    v
}

fn reaches_culprit_inner(
    crate_id: &str,
    culprit_id: &str,
    causes: &HashMap<String, String>,
    memo: &mut HashMap<String, bool>,
    stack_guard: &mut std::collections::HashSet<String>,
) -> bool {
    if crate_id == culprit_id {
        return true;
    }

    if let Some(v) = memo.get(crate_id) {
        return *v;
    }

    if !stack_guard.insert(crate_id.to_string()) {
        return false;
    }

    let Some(cause) = causes.get(crate_id) else {
        stack_guard.remove(crate_id);
        memo.insert(crate_id.to_string(), false);
        return false;
    };

    let v = reaches_culprit_inner(cause, culprit_id, causes, memo, stack_guard);
    stack_guard.remove(crate_id);
    memo.insert(crate_id.to_string(), v);
    v
}

fn extract_dependency_name(reason: &str) -> Option<&str> {
    static DEP_ON_TICK_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static DEP_TICK_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();

    let dep_on_tick_re =
        DEP_ON_TICK_RE.get_or_init(|| Regex::new(r"dependency on `(?P<name>[^`]+)`").unwrap());
    let dep_tick_re =
        DEP_TICK_RE.get_or_init(|| Regex::new(r"dependency `(?P<name>[^`]+)`").unwrap());

    if let Some(caps) = dep_on_tick_re.captures(reason) {
        return Some(caps.name("name")?.as_str());
    }

    if let Some(caps) = dep_tick_re.captures(reason) {
        return Some(caps.name("name")?.as_str());
    }

    None
}

fn parse_fingerprint_details(lines: &[String]) -> BTreeMap<String, Vec<String>> {
    static FP_ERR_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();

    let fp_err_re = FP_ERR_RE
        .get_or_init(|| Regex::new(r"^fingerprint error for (?P<id>[^\s]+\s+v[^\s/]+)/").unwrap());

    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut current: Option<String> = None;

    for raw in lines {
        let line = fingerprint_payload(raw);

        if let Some(caps) = fp_err_re.captures(line) {
            current = Some(caps["id"].to_string());
            continue;
        }

        let Some(crate_id) = current.clone() else {
            continue;
        };

        if let Some(rest) = line.strip_prefix("err:") {
            let msg = rest.trim();
            if !msg.is_empty() {
                out.entry(crate_id).or_default().push(msg.to_string());
            }
            continue;
        }

        if let Some(rest) = line.strip_prefix("Caused by:") {
            let msg = format!("Caused by: {}", rest.trim());
            out.entry(crate_id).or_default().push(msg);
            continue;
        }

        if let Some(rest) = line.strip_prefix("Caused by") {
            let msg = rest.trim();
            if !msg.is_empty() {
                out.entry(crate_id).or_default().push(msg.to_string());
            }
            continue;
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn mk_event(kind: CrateStatusKind, crate_id: &str) -> CrateStatusEvent {
        CrateStatusEvent {
            kind,
            crate_id: crate_id.to_string(),
            reason: None,
        }
    }

    #[test]
    fn picks_first_root_as_culprit_and_builds_cascade() {
        let mut parsed = ParsedCargoOutput::default();
        parsed
            .stderr_events
            .push(mk_event(CrateStatusKind::Compiling, "a v0.1.0"));
        parsed
            .stderr_events
            .push(mk_event(CrateStatusKind::Compiling, "b v0.1.0"));

        parsed.crate_reasons.insert(
            "a v0.1.0".to_string(),
            "the file `src/lib.rs` has changed".to_string(),
        );
        parsed.crate_reasons.insert(
            "b v0.1.0".to_string(),
            "dependency on `a` is newer than we are".to_string(),
        );

        let a = analyze(&parsed);
        assert_eq!(a.culprit, Some("a v0.1.0".to_string()));
        assert_eq!(a.cascade.len(), 2);
        assert_eq!(a.cascade[0].crate_id, "a v0.1.0");
        assert_eq!(a.cascade[0].caused_by, None);
        assert_eq!(a.cascade[1].crate_id, "b v0.1.0");
        assert_eq!(a.cascade[1].caused_by, Some("a v0.1.0".to_string()));
    }

    #[test]
    fn falls_back_to_first_work_crate_if_no_roots_can_be_inferred() {
        let mut parsed = ParsedCargoOutput::default();
        parsed
            .stderr_events
            .push(mk_event(CrateStatusKind::Compiling, "a v0.1.0"));
        parsed
            .stderr_events
            .push(mk_event(CrateStatusKind::Compiling, "b v0.1.0"));

        parsed.crate_reasons.insert(
            "a v0.1.0".to_string(),
            "dependency on `b` is newer than we are".to_string(),
        );
        parsed.crate_reasons.insert(
            "b v0.1.0".to_string(),
            "dependency on `a` is newer than we are".to_string(),
        );

        let a = analyze(&parsed);
        assert_eq!(a.culprit, Some("a v0.1.0".to_string()));
        assert_eq!(a.cascade.len(), 1);
        assert_eq!(a.cascade[0].crate_id, "a v0.1.0");
    }

    #[test]
    fn parses_fingerprint_details_for_a_crate() {
        let lines = vec![
            "TRACE cargo::core::compiler::fingerprint: fingerprint error for a v0.1.0/Build/TargetInner { .. }".to_string(),
            "TRACE cargo::core::compiler::fingerprint: err: unit dependency information changed".to_string(),
            "TRACE cargo::core::compiler::fingerprint: Caused by: new (x) != old (y)".to_string(),
        ];

        let d = parse_fingerprint_details(&lines);
        assert_eq!(d.get("a v0.1.0").unwrap().len(), 2);
        assert_eq!(
            d.get("a v0.1.0").unwrap()[0],
            "unit dependency information changed".to_string()
        );
        assert_eq!(
            d.get("a v0.1.0").unwrap()[1],
            "Caused by: new (x) != old (y)".to_string()
        );
    }
}
