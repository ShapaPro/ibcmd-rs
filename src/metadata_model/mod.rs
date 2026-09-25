//! Base-free metadata descriptor compiler: a metadata object's XML becomes the
//! complete native text of its Config row without reading the target
//! database, so a configuration can be loaded into an empty infobase.
//!
//! One module per family of kinds; `compile_descriptor` dispatches by the
//! XML's kind. `audit` measures every kind against the rows a platform stored.

pub mod audit;

use std::path::Path;

use anyhow::{Result, anyhow};

use crate::module_blob::MetadataSourceContext;

/// What a kind compiler may read besides its own XML.
pub struct DescriptorContext {
    /// The whole source tree, for names other objects resolve to.
    pub source: MetadataSourceContext,
    /// XML dialect of the tree: `2.20` (8.3.27) or `2.21` (8.5).
    pub version: String,
}

impl DescriptorContext {
    pub fn new(root: &Path, version: &str) -> Result<Self> {
        Ok(Self {
            source: MetadataSourceContext::new(root.to_path_buf()),
            version: version.to_string(),
        })
    }
}

/// Compiles one metadata XML into the inflated text of its Config row (BOM included).
pub fn compile_descriptor(
    kind: &str,
    _xml_path: &Path,
    _xml: &[u8],
    _context: &DescriptorContext,
) -> Result<Vec<u8>> {
    match kind {
        _ => Err(anyhow!("no base-free compiler for {kind} yet")),
    }
}
