//! The S0 system prompt and (later) the cache-breakpoint planner.
//!
//! S0 is a single `include_str!` constant with zero interpolation — prefix
//! immutability is a hard invariant, and this module is the only place the
//! bytes are allowed to live.

/// Hard token budget for the S0 system prompt (spec §9). Enforced by test
/// with a real tokenizer — never `chars / 4`.
pub const SYSTEM_PROMPT_TOKEN_BUDGET: u64 = 1400;

/// The static system prompt. Byte-identical on every request, forever.
pub const SYSTEM_PROMPT: &str = include_str!("base.md");

/// Segment labels for the ordered request assembly (S0..S5). The numeric
/// order IS the assembly order — most-stable first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Segment {
    /// S0 — static system prompt.
    System,
    /// S1 — core-tier tool definitions.
    Tools,
    /// S2 — environment facts stable for the process lifetime.
    Environment,
    /// S3 — project instructions (AGENTS.md etc.).
    Project,
    /// S3.5 — learned-skill index + matching skill bodies.
    Skills,
    /// S4 — conversation history.
    History,
    /// S5 — the new user message.
    User,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The budget test the spec demands: S0 must stay under 1,400 tokens
    /// measured by a real tokenizer, so prompt growth is a reviewed
    /// decision, not an accident.
    #[test]
    fn system_prompt_within_budget() {
        let bpe = tiktoken_rs::o200k_base().expect("o200k ranks load");
        let tokens = bpe.encode_ordinary(SYSTEM_PROMPT).len() as u64;
        assert!(
            tokens <= SYSTEM_PROMPT_TOKEN_BUDGET,
            "S0 is {tokens} tokens, budget is {SYSTEM_PROMPT_TOKEN_BUDGET}"
        );
    }

    /// S0 must not contain interpolation artifacts or trailing junk —
    /// snapshot-adjacent guard so cache stability is visible in review.
    #[test]
    fn system_prompt_is_pure_static_text() {
        assert!(SYSTEM_PROMPT.starts_with("You are a coding agent"));
        assert!(!SYSTEM_PROMPT.contains("{{"));
        assert!(!SYSTEM_PROMPT.contains('{'));
    }
}
