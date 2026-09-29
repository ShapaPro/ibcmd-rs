//! Where the named slots and the generated types are in the owner record of a reference object.
//!
//! The layouts of `metadata_model::objects` (the base-free compiler) already say what every slot of
//! every kind holds; this walks one the way the compiler lays it out and reports the position of each
//! slot in the record (`{<tag>,...}`, position 0 is the tag).

use std::collections::HashMap;

use anyhow::{Context, Result, bail};

use crate::metadata_model::brace::Brace;
use crate::metadata_model::objects::{layout, parts::Slot};

/// The uuid pair of a generated type: `TypeId` and `ValueId`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedType {
    /// `Object`, `Ref`, `Selection`, `List`, `Manager`, ...
    pub category: &'static str,
    pub type_id: String,
    pub value_id: String,
}

/// The positions of one kind's record for one record version.
#[derive(Clone, Debug)]
pub struct RecordMap {
    pub names: HashMap<&'static str, usize>,
    /// `(category, position of the TypeId)` in layout order; the ValueId follows.
    pub generated: Vec<(&'static str, usize)>,
    /// The position of the header (`{0,<md base>}` or `<md base>`).
    pub header: usize,
}

impl RecordMap {
    /// The map of `kind` for a record starting with `tag`.
    pub fn new(kind: &str, tag: i64) -> Result<Self> {
        let layout = layout(kind).with_context(|| format!("no layout for {kind}"))?;
        let Some(Slot::Tag(old, new, latest)) = layout.slots.first().copied() else {
            bail!("the {kind} layout does not start with its tag");
        };
        let modern = tag != old;
        let since_8_5_1 = tag == latest && latest != new;
        let mut map = Self {
            names: HashMap::new(),
            generated: Vec::new(),
            header: usize::MAX,
        };
        let mut position = 0usize;
        for slot in layout.slots {
            map.walk(slot, modern, since_8_5_1, &mut position);
        }
        if map.header == usize::MAX {
            bail!("the {kind} layout has no header slot");
        }
        Ok(map)
    }

    fn walk(&mut self, slot: &'static Slot, modern: bool, since_8_5_1: bool, position: &mut usize) {
        match slot {
            Slot::Modern(inner) => {
                if modern {
                    self.walk(inner, modern, since_8_5_1, position);
                }
            }
            Slot::Since8_5_1(inner) => {
                if since_8_5_1 {
                    self.walk(inner, modern, since_8_5_1, position);
                }
            }
            Slot::Generated(category) => {
                self.generated.push((category, *position));
                *position += 2;
            }
            Slot::Header | Slot::WrappedHeader => {
                if self.header == usize::MAX {
                    self.header = *position;
                }
                *position += 1;
            }
            Slot::Flag(name)
            | Slot::Number(name)
            | Slot::Code(name, _)
            | Slot::Text(name)
            | Slot::Localized(name)
            | Slot::Reference(name)
            | Slot::References(name)
            | Slot::Fields(name)
            | Slot::TypePattern(name) => {
                self.names.insert(*name, *position);
                *position += 1;
            }
            _ => *position += 1,
        }
    }

    /// The slot named `name` of a record.
    pub fn slot<'a>(&self, record: &'a [Brace], name: &str) -> Result<&'a Brace> {
        let position = *self
            .names
            .get(name)
            .with_context(|| format!("the layout has no slot {name}"))?;
        record
            .get(position)
            .with_context(|| format!("the record is too short for {name}"))
    }

    /// The generated types of a record, in layout order.
    pub fn generated_types(&self, record: &[Brace]) -> Result<Vec<GeneratedType>> {
        self.generated
            .iter()
            .map(|(category, position)| {
                let atom = |offset: usize| {
                    record
                        .get(position + offset)
                        .and_then(Brace::as_atom)
                        .map(str::to_ascii_lowercase)
                        .with_context(|| format!("no uuid for the {category} type"))
                };
                Ok(GeneratedType {
                    category,
                    type_id: atom(0)?,
                    value_id: atom(1)?,
                })
            })
            .collect()
    }
}

/// The owner record of a descriptor row `{1,<record>,<count>,<collection>...}`, and its tag.
pub fn owner_record(row: &Brace) -> Result<(&[Brace], i64)> {
    let root = row.as_list().context("a descriptor row is not a list")?;
    if root.first().and_then(Brace::as_atom) != Some("1") {
        bail!("not a descriptor row");
    }
    let record = root
        .get(1)
        .and_then(Brace::as_list)
        .context("a descriptor row has no owner record")?;
    let tag = record
        .first()
        .and_then(Brace::as_atom)
        .and_then(|tag| tag.parse().ok())
        .context("a descriptor row has no tag")?;
    Ok((record, tag))
}

/// `{3,{1,0,<uuid>},"Name",...}` -> (uuid, name).
pub fn md_base(node: &Brace) -> Result<(String, String)> {
    let items = node.as_list().context("an md header is not a list")?;
    let uuid = items
        .get(1)
        .and_then(Brace::as_list)
        .and_then(|inner| inner.get(2))
        .and_then(Brace::as_atom)
        .context("an md header has no uuid")?;
    let name = items
        .get(2)
        .and_then(Brace::as_str)
        .context("an md header has no name")?;
    Ok((uuid.to_ascii_lowercase(), name.to_owned()))
}

/// The uuid and the name of a descriptor row's object.
pub fn object_identity(row: &Brace, kind: &str) -> Result<(String, String)> {
    let (record, tag) = owner_record(row)?;
    let map = RecordMap::new(kind, tag)?;
    let header = record
        .get(map.header)
        .context("the record is too short for its header")?;
    let base = match header.as_list() {
        // `{0,<md base>}`
        Some([first, second]) if first.as_atom() == Some("0") => second,
        _ => header,
    };
    md_base(base)
}
