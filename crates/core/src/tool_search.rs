use std::collections::{HashMap, HashSet};

use crate::provider::ToolSpec;

#[derive(Debug, Clone)]
pub struct ToolCandidate {
    pub spec: ToolSpec,
    pub keywords: String,
}

impl ToolCandidate {
    pub fn from_spec(spec: ToolSpec, extra_keywords: &str) -> Self {
        let mut keywords = String::with_capacity(
            spec.name.as_str().len() * 2 + spec.description.len() + extra_keywords.len() + 32,
        );
        // Weight tool name tokens higher by including the name twice alongside schema property names.
        keywords.push_str(spec.name.as_str());
        keywords.push(' ');
        keywords.push_str(spec.name.as_str());
        keywords.push(' ');
        keywords.push_str(&spec.description);
        if let Some(props) = spec
            .input_schema
            .get("properties")
            .and_then(|v| v.as_object())
        {
            for k in props.keys() {
                keywords.push(' ');
                keywords.push_str(k);
            }
        }
        if !extra_keywords.is_empty() {
            keywords.push(' ');
            keywords.push_str(extra_keywords);
        }
        Self { spec, keywords }
    }
}

pub fn tokenize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;

    for ch in text.chars() {
        if !ch.is_ascii_alphanumeric() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            prev_lower = false;
            continue;
        }
        if ch.is_ascii_uppercase() && prev_lower && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        prev_lower = ch.is_ascii_lowercase() || ch.is_ascii_digit();
        cur.push(ch.to_ascii_lowercase());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Rank candidate tools against `query` using BM25 (`k1 = 1.2`, `b = 0.75`).
pub fn search_tools(query: &str, limit: usize, candidates: &[ToolCandidate]) -> Vec<ToolSpec> {
    const K1: f64 = 1.2;
    const B: f64 = 0.75;

    let limit = limit.clamp(1, 5);
    let query_tokens = tokenize(query);
    if query_tokens.is_empty() || candidates.is_empty() {
        return Vec::new();
    }

    let n_docs = candidates.len() as f64;
    let doc_tokens: Vec<Vec<String>> = candidates.iter().map(|c| tokenize(&c.keywords)).collect();
    let avg_dl = (doc_tokens.iter().map(Vec::len).sum::<usize>() as f64 / n_docs).max(1.0);

    let mut df: HashMap<&str, usize> = HashMap::new();
    for tokens in &doc_tokens {
        let unique: HashSet<&str> = tokens.iter().map(String::as_str).collect();
        for term in unique {
            *df.entry(term).or_insert(0) += 1;
        }
    }

    let query_unique: Vec<&str> = {
        let mut seen = HashSet::new();
        query_tokens
            .iter()
            .filter_map(|t| seen.insert(t.as_str()).then_some(t.as_str()))
            .collect()
    };

    let mut scored: Vec<(f64, usize)> = Vec::new();
    for (idx, tokens) in doc_tokens.iter().enumerate() {
        let dl = tokens.len() as f64;
        let mut tf: HashMap<&str, usize> = HashMap::new();
        for t in tokens {
            *tf.entry(t.as_str()).or_insert(0) += 1;
        }

        let mut score = 0.0f64;
        for &qt in &query_unique {
            // Exact token BM25 + light prefix/substring fallback so partial queries like "diff" or "mcp" match reliably.
            let freq = tf.get(qt).copied().unwrap_or(0) as f64;
            let partial_freq = if freq == 0.0 && qt.len() >= 3 {
                tokens
                    .iter()
                    .filter(|dt| dt.starts_with(qt) || dt.contains(qt))
                    .count() as f64
                    * 0.5
            } else {
                0.0
            };
            let effective_tf = freq + partial_freq;
            if effective_tf <= 0.0 {
                continue;
            }
            let doc_freq = df.get(qt).copied().unwrap_or(1) as f64;
            let idf = ((n_docs - doc_freq + 0.5) / (doc_freq + 0.5) + 1.0).ln();
            let denom = effective_tf + K1 * (1.0 - B + B * (dl / avg_dl));
            score += idf * ((effective_tf * (K1 + 1.0)) / denom);
        }

        if score > 0.0 {
            scored.push((score, idx));
        }
    }

    scored.sort_by(|a, b| {
        b.0.total_cmp(&a.0).then_with(|| {
            candidates[a.1]
                .spec
                .name
                .as_str()
                .cmp(candidates[b.1].spec.name.as_str())
        })
    });

    scored
        .into_iter()
        .take(limit)
        .map(|(_, idx)| candidates[idx].spec.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::types::ToolName;

    fn cand(name: &str, desc: &str) -> ToolCandidate {
        ToolCandidate::from_spec(
            ToolSpec {
                name: ToolName::new(name),
                description: desc.to_string(),
                input_schema: json!({"type": "object", "properties": {}}),
            },
            "",
        )
    }

    #[test]
    fn tokenize_splits_snake_and_camel_case() {
        assert_eq!(
            tokenize("mcp__web_search__fetchPage"),
            vec!["mcp", "web", "search", "fetch", "page"]
        );
    }

    #[test]
    fn bm25_ranks_relevant_tools_first() {
        let cands = vec![
            cand("git_diff", "Show git diff of tracked files."),
            cand("git_log", "Show recent git commit history."),
            cand("web_fetch", "Fetch an HTTP URL and extract readable text."),
            cand("outline", "List symbol outline of a source file."),
            cand(
                "apply_patch",
                "Apply a unified diff patch to workspace files.",
            ),
        ];

        let hits = search_tools("git commit history log", 3, &cands);
        assert!(!hits.is_empty());
        assert_eq!(hits[0].name.as_str(), "git_log");

        let web_hits = search_tools("fetch url webpage", 2, &cands);
        assert_eq!(web_hits[0].name.as_str(), "web_fetch");
    }
}
