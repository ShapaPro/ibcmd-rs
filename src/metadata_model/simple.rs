//! Simple objects: Constant, DefinedType, SessionParameter, CommonAttribute,
//! FunctionalOption, FunctionalOptionsParameter, EventSubscription,
//! ScheduledJob, SettingsStorage, FilterCriterion, Language.

use anyhow::Result;

use super::brace::Brace;
use super::{DescriptorContext, ObjectXml, md_base, not_yet};
use crate::brace_list;

pub fn compile(object: &ObjectXml<'_>, _context: &DescriptorContext) -> Result<Brace> {
    match object.kind {
        "Language" => language(object),
        _ => Err(not_yet(object)),
    }
}

/// `{1,{0,<md base>,"<LanguageCode>"},0}`
fn language(object: &ObjectXml<'_>) -> Result<Brace> {
    let properties = object.properties()?;
    Ok(brace_list![
        Brace::num(1),
        brace_list![
            Brace::num(0),
            md_base(&object.uuid, properties),
            Brace::str(object.prop_text("LanguageCode")?),
        ],
        Brace::num(0),
    ])
}
