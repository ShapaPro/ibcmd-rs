//! Reference objects: Catalog, Document, ExchangePlan, the three charts, BusinessProcess, Task, Report, DataProcessor, Enum.

use anyhow::Result;

use super::brace::Brace;
use super::{DescriptorContext, ObjectXml, not_yet};

pub fn compile(object: &ObjectXml<'_>, _context: &DescriptorContext) -> Result<Brace> {
    match object.kind {
        _ => Err(not_yet(object)),
    }
}
