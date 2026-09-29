//! The derived caches of a new object (issue #403, part of S1 = #391): the `Params` `*.si` rows the
//! platform rewrites when a configuration change adds an object or a tabular section.
//!
//! See `docs/apply/derived-caches.md` for the measurements behind every rule here.

pub mod facts;
pub mod order;
pub mod root;
pub mod slots;
pub mod type_index;

#[cfg(test)]
mod tests_corpus;
