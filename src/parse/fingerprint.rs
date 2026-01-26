pub fn is_fingerprint_trace_line(line: &str) -> bool {
    line.contains("cargo::core::compiler::fingerprint")
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
}
