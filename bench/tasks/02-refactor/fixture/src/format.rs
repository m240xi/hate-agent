pub fn heading_id(raw: &str) -> String {
    raw.trim().to_ascii_lowercase().replace(' ', "-")
}
