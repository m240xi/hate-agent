#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    High,
    Normal,
    Low,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskItem {
    pub title: String,
    pub priority: Priority,
}
