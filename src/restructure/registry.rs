//! The object registry (`Params` `1a621f0f-....si`, the "search information" of the configuration): a
//! pre-order list of every metadata object, `uuid, owner uuid, kind, "name", {synonyms}, flag, flag`.
//! The kind is the index of the object's class id in the row's own class list; the count in the header is
//! the number of records.
//!
//! A new attribute of a catalog or a document is one more record (kind 36 and 41 in the БСП, flags `0,0`)
//! among its owner's children: after the previous attribute in metadata order, or, when it is the first,
//! between the groups the other owners of the kind list before and after the attributes. The editing is
//! the apply track's [`crate::mssql_config_apply::si`] (measured there for new forms and templates); this
//! module only says where the attributes go. Measured on case a2 and on the native twin of the types case
//! (`docs/apply/restructuring.md`).

use std::collections::BTreeSet;

use anyhow::{Context, Result};

use crate::mssql_config_apply::si::{self, Insertion, NewRecord};
use crate::restructure::catalog::AttributeFacts;
use crate::restructure::object::ObjectKind;

/// The `Params` row of the registry.
pub const REGISTRY_ROW: &str = "1a621f0f-5568-4183-bd9f-f6ef670e7090.si";
/// The row of the versions of the `*.si` rows.
pub const VERSIONS_ROW: &str = "siVersions";

/// The registry text with the records of the added attributes of one object.
pub struct ObjectAdditions<'a> {
    pub kind: ObjectKind,
    pub owner: &'a str,
    /// All the attributes of the object, in metadata order, as the staged descriptor lists them.
    pub attributes: &'a [AttributeFacts],
    /// The uuids of the new ones.
    pub added: &'a BTreeSet<String>,
}

/// The records for the added attributes, at their places; the count follows. Nothing else changes.
pub fn add_attributes(text: &[u8], objects: &[ObjectAdditions<'_>]) -> Result<Vec<u8>> {
    let main = si::parse(text).context("the object registry does not parse")?;
    let mut insertions = Vec::new();
    for object in objects {
        let kind = main
            .kind_of_class(object.kind.attribute_class())
            .with_context(|| {
                format!(
                    "the registry has no class {} for the attributes of a {}",
                    object.kind.attribute_class(),
                    object.kind.label()
                )
            })?;
        // The new attributes that follow the same existing one go in together, in metadata order.
        let mut previous: Option<&AttributeFacts> = None;
        let mut pending: Vec<&AttributeFacts> = Vec::new();
        let mut flush = |previous: Option<&AttributeFacts>,
                         next: Option<&AttributeFacts>,
                         pending: &mut Vec<&AttributeFacts>|
         -> Result<()> {
            if pending.is_empty() {
                return Ok(());
            }
            let at = main.place(
                object.owner,
                kind,
                previous.map(|attribute| attribute.uuid.as_str()),
                next.map(|attribute| attribute.uuid.as_str()),
            )?;
            let records = pending
                .drain(..)
                .map(|attribute| NewRecord {
                    uuid: attribute.uuid.clone(),
                    parent: object.owner.to_owned(),
                    kind,
                    name: attribute.name.clone(),
                    synonyms: attribute.synonyms.clone(),
                    flags: (0, 0),
                })
                .collect();
            insertions.push(Insertion { at, records });
            Ok(())
        };
        for attribute in object.attributes {
            if object.added.contains(&attribute.uuid) {
                pending.push(attribute);
            } else {
                flush(previous, Some(attribute), &mut pending)?;
                previous = Some(attribute);
            }
        }
        flush(previous, None, &mut pending)?;
    }
    si::insert_records(text, &main, &insertions)
}

/// `siVersions` with a new version for each named row.
pub fn bump_versions(text: &[u8], rows: &[&str]) -> Result<Vec<u8>> {
    let mut out = text.to_vec();
    for row in rows {
        out = si::set_si_version(&out, row, uuid::Uuid::new_v4())?;
    }
    Ok(out)
}
