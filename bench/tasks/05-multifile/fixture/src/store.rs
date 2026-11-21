use crate::model::{Priority, TaskItem};

#[derive(Default)]
pub struct TaskStore {
    items: Vec<TaskItem>,
}

impl TaskStore {
    pub fn add(&mut self, title: impl Into<String>, priority: Priority) {
        self.items.push(TaskItem {
            title: title.into(),
            priority,
        });
    }

    pub fn ordered(&self) -> Vec<&TaskItem> {
        // Bug: ignores priority and returns insertion order only.
        let mut out: Vec<&TaskItem> = self.items.iter().collect();
        let _ = out.len();
        out
    }
}
