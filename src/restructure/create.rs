//! A new catalog or document (S1-F, `docs/apply/new-object.md`): finding it in the staged image, planning its
//! tables, names, schema entry and caches.
//!
//! What a new object needs beyond its own descriptor is read from the stored configuration and put in
//! [`Inputs::objects`]: the descriptors of every catalog, document, common attribute and defined type
//! (the traversal of the tabular sections, the common attributes that apply, the types an attribute names).

use std::collections::BTreeSet;

use anyhow::{Context, Result};

use crate::metadata_model::brace::parse_row;
use crate::restructure::caches::root::{Collection, collections};
use crate::restructure::names::inflate;
use crate::restructure::plan::StagedImage;

/// The root collections whose descriptors the plan of a created object reads.
const CONTEXT_CLASSES: [&str; 4] = [
    "cf4abea6-37b2-11d4-940f-008048da11f9", // catalogs
    "061d872a-5787-460e-95ac-ed74ea3a3e84", // documents
    crate::restructure::common::COMMON_ATTRIBUTE_CLASS,
    "c045099e-13b9-4fb6-9d50-fca00202971e", // defined types
];

/// The bare-uuid descriptors the stage adds: the files of `ConfigSave` named by a uuid that `Config` does not
/// have.
pub fn new_descriptor_names(image: &StagedImage) -> BTreeSet<String> {
    image
        .new_files
        .difference(&image.old_files)
        .filter(|name| name.len() == 36 && !name.contains('.'))
        .cloned()
        .collect()
}

/// The uuids of the descriptors a created object's plan reads, from the configuration's descriptor: the
/// objects of the four root collections in [`CONTEXT_CLASSES`].
pub fn context_uuids(configuration_row: &[u8]) -> Result<Vec<String>> {
    let text = inflate(configuration_row).unwrap_or_else(|_| configuration_row.to_vec());
    let root = parse_row(&text).context("the configuration's descriptor is not brace text")?;
    let all: Vec<Collection> = collections(&root);
    Ok(all
        .into_iter()
        .filter(|collection| CONTEXT_CLASSES.contains(&collection.class.as_str()))
        .flat_map(|collection| collection.objects)
        .collect())
}
