pub mod model;
pub mod store;

pub use model::{Priority, TaskItem};
pub use store::TaskStore;

pub fn top_titles(store: &TaskStore) -> Vec<&str> {
    store
        .ordered()
        .into_iter()
        .map(|t| t.title.as_str())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_by_priority_then_insertion() {
        let mut store = TaskStore::default();
        store.add("low", Priority::Low);
        store.add("high-1", Priority::High);
        store.add("normal", Priority::Normal);
        store.add("high-2", Priority::High);
        assert_eq!(top_titles(&store), ["high-1", "high-2", "normal", "low"]);
    }
}
