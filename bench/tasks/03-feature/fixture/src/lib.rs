pub fn parse_duration_ms(_input: &str) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_and_rejects_invalid_durations() {
        assert_eq!(parse_duration_ms("250ms"), Some(250));
        assert_eq!(parse_duration_ms(" 3s "), Some(3_000));
        assert_eq!(parse_duration_ms("2m"), Some(120_000));
        assert_eq!(parse_duration_ms("0ms"), Some(0));
        assert_eq!(parse_duration_ms(""), None);
        assert_eq!(parse_duration_ms("-5s"), None);
        assert_eq!(parse_duration_ms("10h"), None);
    }
}
