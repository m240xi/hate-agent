# Task 04: Add Edge-Case Test Coverage

Review `TokenBucket` in `src/lib.rs` and add unit tests verifying:
1. Initial capacity is capped at `max_tokens`.
2. `try_acquire(0)` always returns `true`.
3. `refill(amount)` never exceeds `max_tokens` (saturating).
4. Requests larger than remaining tokens return `false` without partially deducting.
- Verify with `cargo test --quiet`.
