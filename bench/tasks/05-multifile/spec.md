# Task 05: Multi-File Coordinated Change

Update `src/model.rs`, `src/store.rs`, and `src/lib.rs` so:
- `TaskItem` tracks `priority: Priority` (`High`, `Normal`, `Low`).
- `TaskStore::ordered(&self) -> Vec<&TaskItem>` returns items ordered highest-priority first (`High` -> `Normal` -> `Low`), preserving insertion order within the same priority.
- Fix existing constructor/method signatures so `cargo test --quiet` passes.
