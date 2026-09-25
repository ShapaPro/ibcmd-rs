//! Common objects and services: CommonModule, CommonPicture, CommonTemplate, CommonCommand, CommandGroup, Role, XDTOPackage, StyleItem, Style, WebService, HTTPService, WSReference, IntegrationService, Bot, ExternalDataSource, Subsystem, owned Form and Template, CommonForm, Interface.

use anyhow::Result;

use super::brace::Brace;
use super::{DescriptorContext, ObjectXml, not_yet};

pub fn compile(object: &ObjectXml<'_>, _context: &DescriptorContext) -> Result<Brace> {
    match object.kind {
        _ => Err(not_yet(object)),
    }
}
