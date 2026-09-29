//! The derived caches of a new object (issue #403, part of S1 = #391): the `Params` `*.si` rows the
//! platform rewrites when a configuration change adds an object or a tabular section.
//!
//! See `docs/apply/derived-caches.md` for the measurements behind every rule here.

pub mod facts;
pub mod help_props;
pub mod names_tables;
pub mod order;
pub mod owner_map;
pub mod plan;
pub mod root;
pub mod slots;
pub mod synonyms;
pub mod type_index;
pub mod type_sets;
pub mod xdto_types;

#[cfg(test)]
mod tests_cases;
#[cfg(test)]
mod tests_corpus;
