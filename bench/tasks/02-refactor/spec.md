# Task 02: Cross-Module Refactor

Refactor `src/lib.rs` and `src/format.rs` to share one `normalize_slug(raw: &str) -> String` helper that:
- Lowercases ASCII alphanumerics
- Replaces separator runs (`' '`, `'_'`, `'-'`, `'/'`) with a single `'-'`
- Strips leading/trailing `'-'`
- Verify with `cargo test --quiet`.
