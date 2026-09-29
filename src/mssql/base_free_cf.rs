//! XML → CF with no base (Untru/ibcmd-rs#351): the base-free stage an empty
//! infobase is loaded from (`empty_stage::prepare_empty_stage`) gives every
//! Config row of the tree; a row's stored bytes (raw deflate of its text) are
//! what a `.cf` element holds, so the rows become the container's entries as
//! they are.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Result, bail};
use ibcmd_core::storage::{
    MultipartIdentity, StorageKey, StoragePatch, StoragePatchEntry, StoragePatchOutcome,
    StoragePatchTarget, StorageProvenance,
};

use crate::legacy_version::InfobaseConfigSourceVersion;

/// The container entries of the tree at `root`: name → stored bytes. Any row
/// the stage could not compile refuses the whole tree, naming each.
pub fn base_free_entries(
    root: &Path,
    source_version: InfobaseConfigSourceVersion,
) -> Result<BTreeMap<String, Vec<u8>>> {
    let stage = super::empty_stage::prepare_empty_stage(root, Some(source_version.as_str()))?;
    let failures = stage.failures().collect::<Vec<_>>();
    if !failures.is_empty() {
        let shown = failures
            .iter()
            .take(20)
            .map(|failure| {
                format!(
                    "{} {} {}: {}",
                    failure.source,
                    failure.family,
                    failure.file_name.as_deref().unwrap_or(""),
                    failure.error
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        bail!(
            "{} rows of the tree cannot be compiled without a base:\n{shown}",
            failures.len()
        );
    }
    let mut entries = BTreeMap::new();
    for row in stage.rows() {
        match entries.get(&row.file_name) {
            Some(existing) if existing != &row.blob => {
                bail!("row {} is compiled twice with different bytes", row.file_name)
            }
            Some(_) => {}
            None => {
                entries.insert(row.file_name.clone(), row.blob.clone());
            }
        }
    }
    Ok(entries)
}

/// [`base_free_entries`] as the patch `cf bootstrap` publishes, and the
/// entries' total stored bytes.
pub fn base_free_patch(
    root: &Path,
    source_version: InfobaseConfigSourceVersion,
) -> Result<(StoragePatch, usize)> {
    let entries = base_free_entries(root, source_version)?;
    let total = entries.values().map(Vec::len).sum();
    let mut patch = Vec::with_capacity(entries.len());
    for (name, bytes) in entries {
        patch.push(StoragePatchEntry::new(
            StoragePatchTarget::new(
                StorageKey::new(&name)?,
                MultipartIdentity::single(),
                StorageProvenance::new(&format!("bootstrap:base-free:{name}"))?,
            ),
            StoragePatchOutcome::compiled(bytes)?,
        ));
    }
    Ok((StoragePatch::new(patch)?, total))
}
