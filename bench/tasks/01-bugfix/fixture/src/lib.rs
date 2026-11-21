pub fn paginate_slice(items: &[i32], page: usize, per_page: usize) -> &[i32] {
    if per_page == 0 {
        return &[];
    }
    let start = page.saturating_mul(per_page);
    if start >= items.len() {
        return &[];
    }
    // Intentional off-by-one bug for benchmark fixture.
    let end = start.saturating_add(per_page.saturating_sub(1)).min(items.len());
    &items[start..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_cover_all_elements() {
        let data = [10, 20, 30, 40, 50];
        assert_eq!(paginate_slice(&data, 0, 2), &[10, 20]);
        assert_eq!(paginate_slice(&data, 1, 2), &[30, 40]);
        assert_eq!(paginate_slice(&data, 2, 2), &[50]);
        assert_eq!(paginate_slice(&data, 3, 2), &[] as &[i32]);
        assert_eq!(paginate_slice(&data, 0, 0), &[] as &[i32]);
    }
}
