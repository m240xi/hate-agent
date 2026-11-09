pub const SYSTEM_PROMPT_TOKEN_BUDGET: u64 = 1400;

pub const SYSTEM_PROMPT: &str = include_str!("base.md");

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Segment {
    System,
    Tools,
    Environment,
    Project,
    Skills,
    History,
    User,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_prompt_within_budget() {
        let bpe = tiktoken_rs::o200k_base().expect("o200k ranks load");
        let tokens = bpe.encode_ordinary(SYSTEM_PROMPT).len() as u64;
        assert!(
            tokens <= SYSTEM_PROMPT_TOKEN_BUDGET,
            "S0 is {tokens} tokens, budget is {SYSTEM_PROMPT_TOKEN_BUDGET}"
        );
    }

    #[test]
    fn system_prompt_is_pure_static_text() {
        assert!(SYSTEM_PROMPT.starts_with("You are a coding agent"));
        assert!(!SYSTEM_PROMPT.contains("{{"));
        assert!(!SYSTEM_PROMPT.contains('{'));
    }
}
