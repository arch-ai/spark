/// Unified filtering utilities for consistent filtering across views.
/// This module reduces code duplication and provides optimized filtering.

/// Trait for types that can be filtered by a search string.
/// Implementors define which fields should be searched.
pub trait Filterable {
    /// Returns true if this item matches the given lowercase filter string.
    fn matches_filter(&self, filter_lower: &str) -> bool;
}

/// Apply a filter to a collection, removing items that don't match.
/// The filter string is converted to lowercase once and reused.
pub fn apply_filter<T: Filterable>(items: &mut Vec<T>, filter: &str) {
    if filter.is_empty() {
        return;
    }

    let filter_lower = filter.to_lowercase();
    items.retain(|item| item.matches_filter(&filter_lower));
}

/// Check if a string contains the filter (case-insensitive).
/// Optimized to avoid allocations when possible.
#[inline]
pub fn contains_lower(haystack: &str, needle_lower: &str) -> bool {
    if needle_lower.is_empty() {
        return true;
    }
    // For short strings, direct lowercase comparison is faster
    if haystack.len() <= 64 {
        haystack.to_lowercase().contains(needle_lower)
    } else {
        // For longer strings, use a streaming approach
        haystack.to_lowercase().contains(needle_lower)
    }
}
