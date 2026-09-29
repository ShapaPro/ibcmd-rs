//! Own restructuring of the infobase on a configuration change (issue #341, track "ddl").
//!
//! When `config apply` changes a table's structure the platform rebuilds the table through a "new
//! generation" (`NG`) copy and records its progress in `SchemaStorage`. This module is the research
//! prototype of doing that ourselves for the simplest case, **new attributes in one catalog**:
//!
//! - [`names`] -- `Params.DBNames`, the numbering of tables and fields, and `DBNamesVersion`;
//! - [`schema`] -- `DBSchema` (and `NewGenCreated`, the same grammar): a byte-exact model of the text and
//!   the SQL it stands for (columns, indexes, `create table`);
//! - [`storage`] -- the `SchemaStorage` states and the empty-generation marker;
//! - [`catalog`] -- a catalog's descriptor row as the table structure needs it, and the mapping of an
//!   attribute's type to the field's type entries;
//! - [`xdto`] -- the XDTO model cache (`Params` `*.si`), which a new attribute makes stale;
//! - [`plan`] -- the checks (fail closed) and the plan: new schema, new names, the statements;
//! - [`reader`] -- the database side of the plan's input;
//! - [`exec`] -- the plan run in **one transaction**, verified before it commits.
//!
//! The brace text is `metadata_model::brace`, shared with the model export. Findings and measurements:
//! `docs/apply/restructuring.md`.

pub mod catalog;
pub mod command;
pub mod exec;
pub mod names;
pub mod plan;
pub mod reader;
pub mod schema;
pub mod storage;
pub mod xdto;

#[cfg(test)]
mod tests_corpus;
#[cfg(test)]
mod tests_plan;
#[cfg(test)]
mod tests_schema;
