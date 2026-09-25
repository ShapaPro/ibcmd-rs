//! The Configuration object's own row.

use anyhow::Result;

use super::brace::Brace;
use super::{DescriptorContext, ObjectXml, not_yet};

pub fn compile(object: &ObjectXml<'_>, _context: &DescriptorContext) -> Result<Brace> {
    match object.kind {
        _ => Err(not_yet(object)),
    }
}
