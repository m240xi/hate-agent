# Task 03: Implement `parse_duration_ms`

Implement `pub fn parse_duration_ms(input: &str) -> Option<u64>` in `src/lib.rs`:
- Trim surrounding ASCII whitespace.
- Support `"ms"` (x1), `"s"` (x1000), `"m"` (x60_000) suffixes using checked multiplication.
- Return `None` for empty strings, unknown suffixes, negative values, or overflow.
- Verify with `cargo test --quiet`.
