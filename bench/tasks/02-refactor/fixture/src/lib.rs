pub mod format;

pub fn branch_slug(raw: &str) -> String {
    // Incomplete normalization that leaves leading/trailing/repeated dashes.
    raw.trim().to_ascii_lowercase().replace(' ', "-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_and_header_slugs_match_strict_rules() {
        assert_eq!(branch_slug("  Feat / Add  Auth! "), "feat-add-auth");
        assert_eq!(format::heading_id("  Feat / Add  Auth! "), "feat-add-auth");
        assert_eq!(branch_slug("---already__clean---"), "already-clean");
    }
}
