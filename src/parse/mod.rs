mod fingerprint;
mod stderr_verbose;

use std::collections::BTreeMap;

use cargo_metadata::Message;

pub use stderr_verbose::parse_line as parse_status_line;
pub use stderr_verbose::{CrateStatusEvent, CrateStatusKind, StderrLineDisposition};

#[derive(Default, Debug)]
pub struct ParsedCargoOutput {
    pub stderr_events: Vec<CrateStatusEvent>,
    pub fingerprint_lines: Vec<String>,
    pub other_stderr: Vec<String>,
    pub messages: Vec<Message>,
    pub crate_reasons: BTreeMap<String, String>,
}

impl ParsedCargoOutput {
    pub fn ingest_stderr_line(&mut self, line: &str) -> StderrLineDisposition {
        let line_trimmed = line.trim_start();

        if let Some(event) = stderr_verbose::parse_line(line_trimmed) {
            if let Some(reason) = event.reason.clone() {
                self.crate_reasons.insert(event.crate_id.clone(), reason);
            }
            self.stderr_events.push(event);
            return StderrLineDisposition::Suppress;
        }

        if fingerprint::is_fingerprint_trace_line(line_trimmed) {
            self.fingerprint_lines.push(line_trimmed.to_owned());
            return StderrLineDisposition::Suppress;
        }

        self.other_stderr.push(line.to_owned());
        StderrLineDisposition::Suppress
    }

    pub fn ingest_message(&mut self, msg: Message) {
        self.messages.push(msg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn suppresses_verbose_status_lines_and_extracts_reason() {
        let mut parsed = ParsedCargoOutput::default();
        let disp = parsed
            .ingest_stderr_line("Dirty foo v0.1.0 (/tmp/foo): the file `src/lib.rs` has changed");

        assert_eq!(disp, StderrLineDisposition::Suppress);
        assert_eq!(parsed.stderr_events.len(), 1);
        assert_eq!(
            parsed.crate_reasons.get("foo v0.1.0"),
            Some(&"the file `src/lib.rs` has changed".to_string())
        );
    }

    #[test]
    fn suppresses_fingerprint_trace_lines() {
        let mut parsed = ParsedCargoOutput::default();
        let disp = parsed.ingest_stderr_line(
            "TRACE cargo::core::compiler::fingerprint: compare fingerprints for foo",
        );

        assert_eq!(disp, StderrLineDisposition::Suppress);
        assert_eq!(parsed.fingerprint_lines.len(), 1);
    }

    #[test]
    fn forwards_unrelated_lines() {
        let mut parsed = ParsedCargoOutput::default();
        let disp = parsed.ingest_stderr_line("warning: something happened");
        assert_eq!(disp, StderrLineDisposition::Suppress);
        assert_eq!(parsed.other_stderr.len(), 1);
    }

    #[test]
    fn parses_indented_fresh_lines() {
        let mut parsed = ParsedCargoOutput::default();
        let disp = parsed.ingest_stderr_line("       Fresh foo v0.1.0 (/tmp/foo)");
        assert_eq!(disp, StderrLineDisposition::Suppress);
        assert_eq!(parsed.stderr_events.len(), 1);
        assert_eq!(parsed.stderr_events[0].crate_id, "foo v0.1.0");
    }
}
