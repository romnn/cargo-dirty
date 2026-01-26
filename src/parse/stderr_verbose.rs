use regex::Regex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StderrLineDisposition {
    Forward,
    Suppress,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrateStatusEvent {
    pub kind: CrateStatusKind,
    pub crate_id: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrateStatusKind {
    Fresh,
    Dirty,
    Compiling,
    Checking,
    Building,
    Running,
    Finished,
}

pub fn parse_line(line: &str) -> Option<CrateStatusEvent> {
    static FRESH_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static DIRTY_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static COMPILING_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static CHECKING_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static BUILDING_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static RUNNING_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();

    let fresh_re = FRESH_RE.get_or_init(|| {
        Regex::new(r"^Fresh\s+(?P<id>[^\s]+\s+v[^\s]+)(?:\s+\(.*\))?$").unwrap()
    });
    let dirty_re = DIRTY_RE.get_or_init(|| {
        Regex::new(r"^Dirty\s+(?P<id>[^\s]+\s+v[^\s]+)(?:\s+\(.*\))?:\s+(?P<reason>.*)$").unwrap()
    });
    let compiling_re = COMPILING_RE.get_or_init(|| {
        Regex::new(r"^Compiling\s+(?P<id>[^\s]+\s+v[^\s]+)(?:\s+\(.*\))?$").unwrap()
    });
    let checking_re = CHECKING_RE.get_or_init(|| {
        Regex::new(r"^Checking\s+(?P<id>[^\s]+\s+v[^\s]+)(?:\s+\(.*\))?$").unwrap()
    });
    let building_re = BUILDING_RE.get_or_init(|| {
        Regex::new(r"^Building\s+(?P<id>[^\s]+\s+v[^\s]+)(?:\s+\(.*\))?$").unwrap()
    });
    let running_re = RUNNING_RE.get_or_init(|| {
        Regex::new(r"^Running\s+(?P<rest>.*)$").unwrap()
    });

    if let Some(caps) = fresh_re.captures(line) {
        return Some(CrateStatusEvent {
            kind: CrateStatusKind::Fresh,
            crate_id: caps["id"].to_string(),
            reason: None,
        });
    }

    if let Some(caps) = dirty_re.captures(line) {
        return Some(CrateStatusEvent {
            kind: CrateStatusKind::Dirty,
            crate_id: caps["id"].to_string(),
            reason: Some(caps["reason"].to_string()),
        });
    }

    if let Some(caps) = compiling_re.captures(line) {
        return Some(CrateStatusEvent {
            kind: CrateStatusKind::Compiling,
            crate_id: caps["id"].to_string(),
            reason: None,
        });
    }

    if let Some(caps) = checking_re.captures(line) {
        return Some(CrateStatusEvent {
            kind: CrateStatusKind::Checking,
            crate_id: caps["id"].to_string(),
            reason: None,
        });
    }

    if let Some(caps) = building_re.captures(line) {
        return Some(CrateStatusEvent {
            kind: CrateStatusKind::Building,
            crate_id: caps["id"].to_string(),
            reason: None,
        });
    }

    if running_re.is_match(line) {
        return Some(CrateStatusEvent {
            kind: CrateStatusKind::Running,
            crate_id: "<running>".to_string(),
            reason: Some(line.to_string()),
        });
    }

    if line.starts_with("Finished") {
        return Some(CrateStatusEvent {
            kind: CrateStatusKind::Finished,
            crate_id: "<finished>".to_string(),
            reason: Some(line.to_string()),
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn parses_fresh() {
        let ev = parse_line("Fresh foo v0.1.0 (/tmp/foo)").unwrap();
        assert_eq!(ev.kind, CrateStatusKind::Fresh);
        assert_eq!(ev.crate_id, "foo v0.1.0");
        assert_eq!(ev.reason, None);
    }

    #[test]
    fn parses_dirty() {
        let ev = parse_line("Dirty bar v0.2.0 (/tmp/bar): the file `src/lib.rs` has changed").unwrap();
        assert_eq!(ev.kind, CrateStatusKind::Dirty);
        assert_eq!(ev.crate_id, "bar v0.2.0");
        assert_eq!(
            ev.reason,
            Some("the file `src/lib.rs` has changed".to_string())
        );
    }

    #[test]
    fn parses_compiling() {
        let ev = parse_line("Compiling baz v1.2.3 (/tmp/baz)").unwrap();
        assert_eq!(ev.kind, CrateStatusKind::Compiling);
        assert_eq!(ev.crate_id, "baz v1.2.3");
    }

    #[test]
    fn parses_checking() {
        let ev = parse_line("Checking qux v0.9.9 (/tmp/qux)").unwrap();
        assert_eq!(ev.kind, CrateStatusKind::Checking);
        assert_eq!(ev.crate_id, "qux v0.9.9");
    }

    #[test]
    fn ignores_unrelated_lines() {
        assert_eq!(parse_line("warning: something"), None);
    }
}
