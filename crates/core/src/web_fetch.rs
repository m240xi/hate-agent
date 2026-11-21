use std::time::Duration;

const MAX_URL_BYTES: usize = 512 * 1024;
const MAX_RETURN_BYTES: usize = 24 * 1024;

pub async fn fetch_url_text(url: &str) -> Result<String, String> {
    let url = url.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("URL must start with http:// or https://".to_string());
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(12))
        .user_agent("hate-agent/0.1")
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client.get(url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    let slice = &bytes[..bytes.len().min(MAX_URL_BYTES)];
    let raw = String::from_utf8_lossy(slice);
    let cleaned = if content_type.contains("html") || raw.trim_start().starts_with('<') {
        html_to_text(&raw)
    } else {
        raw.into_owned()
    };
    Ok(clip_utf8(&cleaned, MAX_RETURN_BYTES).to_string())
}

pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut in_tag = false;
    let mut skip_block: Option<&'static str> = None;
    let lower = html.to_ascii_lowercase();
    let chars: Vec<(usize, char)> = html.char_indices().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let (byte_idx, ch) = chars[i];
        if let Some(end_tag) = skip_block {
            if lower[byte_idx..].starts_with(end_tag) {
                skip_block = None;
                in_tag = true;
            }
            i += 1;
            continue;
        }
        if ch == '<' {
            if lower[byte_idx..].starts_with("<script") {
                skip_block = Some("</script");
                i += 1;
                continue;
            }
            if lower[byte_idx..].starts_with("<style") {
                skip_block = Some("</style");
                i += 1;
                continue;
            }
            in_tag = true;
            if (lower[byte_idx..].starts_with("<br")
                || lower[byte_idx..].starts_with("<p")
                || lower[byte_idx..].starts_with("<div")
                || lower[byte_idx..].starts_with("<li")
                || lower[byte_idx..].starts_with("<h")
                || lower[byte_idx..].starts_with("<tr"))
                && !out.ends_with('\n')
            {
                out.push('\n');
            }
            i += 1;
            continue;
        }
        if in_tag {
            if ch == '>' {
                in_tag = false;
            }
            i += 1;
            continue;
        }
        out.push(ch);
        i += 1;
    }
    let decoded = out
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ");
    let mut collapsed = String::with_capacity(decoded.len());
    let mut blank_run = 0u8;
    for line in decoded.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            blank_run = blank_run.saturating_add(1);
            if blank_run == 1 && !collapsed.is_empty() {
                collapsed.push('\n');
            }
        } else {
            blank_run = 0;
            collapsed.push_str(trimmed);
            collapsed.push('\n');
        }
    }
    collapsed
}

pub fn clip_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_to_text_strips_scripts_and_tags() {
        let html = "<html><head><style>body{}</style></head><body><h1>Title</h1><script>alert(1)</script><p>Hello &amp; world</p></body></html>";
        let txt = html_to_text(html);
        assert!(txt.contains("Title"));
        assert!(txt.contains("Hello & world"));
        assert!(!txt.contains("alert"));
        assert!(!txt.contains("body{}"));
    }
}
