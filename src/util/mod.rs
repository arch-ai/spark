pub mod filter;

use std::cmp::Ordering;

pub use filter::{apply_filter, contains_lower, Filterable};

pub fn cmp_f32(a: f32, b: f32) -> Ordering {
    a.partial_cmp(&b).unwrap_or(Ordering::Equal)
}
