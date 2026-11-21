# Task 01: Bugfix (`paginate_slice`)

Fix `paginate_slice(items: &[i32], page: usize, per_page: usize) -> &[i32]` in `src/lib.rs` so:
- 0-indexed `page` returns items `[page * per_page .. ((page + 1) * per_page).min(items.len())]`.
- Out-of-range pages or `per_page == 0` return `&[]` instead of panicking or dropping elements.
- Verify with `cargo test --quiet`.
