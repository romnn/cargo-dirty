//! The grammar of cargo's `-vv` status lines.

use regex::Regex;

use super::{CrateId, CrateStatus, StatusKind, WorkKind};

/// Every status line cargo emits under `-vv` has the same shape: a verb, a `name vX.Y.Z` unit
/// id, an optional parenthesised source path, and — for `Dirty` — a trailing reason.
///
/// The path group is lazy on purpose. A greedy one swallows the reason's own parentheses (cargo
/// appends timestamps to file-change reasons) and leaves the reason unmatched.
const STATUS_PATTERN: &str = r"^(?P<verb>Fresh|Dirty|Compiling|Checking|Building)\s+(?P<id>\S+\s+v\S+)(?:\s+\(.*?\))?(?::\s+(?P<reason>.*))?$";

fn status_regex() -> Option<&'static Regex> {
    static STATUS_RE: std::sync::OnceLock<Result<Regex, regex::Error>> = std::sync::OnceLock::new();

    STATUS_RE
        .get_or_init(|| Regex::new(STATUS_PATTERN))
        .as_ref()
        .ok()
}

/// Parses one already-trimmed stderr line as a crate status, or `None` if it is not one.
pub fn parse(line: &str) -> Option<CrateStatus> {
    let caps = status_regex()?.captures(line)?;
    let id = CrateId(caps.name("id")?.as_str().to_owned());

    let kind = match caps.name("verb")?.as_str() {
        "Fresh" => StatusKind::Fresh,
        // A `Dirty` line always states why. One without a reason is a different line that
        // merely starts with the same word, so it is not a status.
        "Dirty" => StatusKind::Dirty {
            reason: caps.name("reason")?.as_str().to_owned(),
        },
        "Compiling" => StatusKind::Work(WorkKind::Compiling),
        "Checking" => StatusKind::Work(WorkKind::Checking),
        "Building" => StatusKind::Work(WorkKind::Building),
        // The verb alternation admits nothing else.
        _ => return None,
    };

    Some(CrateStatus { id, kind })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn parses_fresh() {
        let status = parse("Fresh foo v0.1.0 (/tmp/foo)").unwrap();
        assert_eq!(status.kind, StatusKind::Fresh);
        assert_eq!(status.id, CrateId::new("foo v0.1.0"));
    }

    #[test]
    fn parses_dirty() {
        let status =
            parse("Dirty bar v0.2.0 (/tmp/bar): the file `src/lib.rs` has changed").unwrap();

        assert_eq!(status.id, CrateId::new("bar v0.2.0"));
        assert_eq!(
            status.kind,
            StatusKind::Dirty {
                reason: "the file `src/lib.rs` has changed".to_string()
            }
        );
    }

    #[test]
    fn keeps_parenthesised_detail_inside_the_dirty_reason() {
        let status = parse(
            "Dirty bar v0.2.0 (/tmp/bar): the file `src/lib.rs` has changed (1.0s, 8s after last build at 0.5s)",
        )
        .unwrap();

        assert_eq!(
            status.kind,
            StatusKind::Dirty {
                reason: "the file `src/lib.rs` has changed (1.0s, 8s after last build at 0.5s)"
                    .to_string()
            }
        );
    }

    #[test]
    fn rejects_dirty_without_a_reason() {
        assert_eq!(parse("Dirty bar v0.2.0 (/tmp/bar)"), None);
    }

    #[test]
    fn parses_compiling() {
        let status = parse("Compiling baz v1.2.3 (/tmp/baz)").unwrap();
        assert_eq!(status.kind, StatusKind::Work(WorkKind::Compiling));
        assert_eq!(status.id, CrateId::new("baz v1.2.3"));
    }

    #[test]
    fn parses_checking() {
        let status = parse("Checking qux v0.9.9 (/tmp/qux)").unwrap();
        assert_eq!(status.kind, StatusKind::Work(WorkKind::Checking));
        assert_eq!(status.id, CrateId::new("qux v0.9.9"));
    }

    #[test]
    fn parses_building() {
        let status = parse("Building quux v0.0.1 (/tmp/quux)").unwrap();
        assert_eq!(status.kind, StatusKind::Work(WorkKind::Building));
        assert_eq!(status.id, CrateId::new("quux v0.0.1"));
    }

    #[test]
    fn parses_a_unit_without_a_source_path() {
        let status = parse("Fresh serde v1.0.200").unwrap();
        assert_eq!(status.kind, StatusKind::Fresh);
        assert_eq!(status.id, CrateId::new("serde v1.0.200"));
    }

    #[test]
    fn ignores_unrelated_lines() {
        assert_eq!(parse("warning: something"), None);
        assert_eq!(parse("Running `rustc --crate-name foo`"), None);
        assert_eq!(parse("Finished `dev` profile in 0.1s"), None);
    }
}
