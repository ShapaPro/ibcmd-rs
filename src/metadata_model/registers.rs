//! Registers and their kin: InformationRegister, AccumulationRegister, AccountingRegister, CalculationRegister, Recalculation, DocumentJournal, Sequence, DocumentNumerator.

use anyhow::Result;

use super::brace::Brace;
use super::{DescriptorContext, ObjectXml, not_yet};

pub fn compile(object: &ObjectXml<'_>, _context: &DescriptorContext) -> Result<Brace> {
    match object.kind {
        _ => Err(not_yet(object)),
    }
}
