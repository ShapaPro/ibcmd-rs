//! The cache rows a new catalog (and a new tabular section) rewrites, from the rows as they are.
//!
//! The functions take the **inflated** text of the `Params` rows and return the inflated text of the
//! changed ones; deflating, the `siVersions` guids and the write (`ParamsRewrite`) are the caller's.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};

use crate::metadata_model::brace::Brace;
use crate::restructure::caches::facts::{ObjectFacts, TABULAR_SECTIONS};
use crate::restructure::caches::help_props::HelpProps;
use crate::restructure::caches::names_tables::{NamesTables, NewTable};
use crate::restructure::caches::owner_map::OwnerMap;
use crate::restructure::caches::root::{
    CATALOG_CLASS, Collection, collection_of, collections, kind_of_class,
};
use crate::restructure::caches::synonyms::Synonyms;
use crate::restructure::caches::type_index::{Entry, TypeIndex, TypeSlot};
use crate::restructure::caches::type_sets::TypeSets;
use crate::restructure::caches::xdto_types::{CatalogShape, XdtoModel};

/// The names of the cache rows (`Params` `FileName`).
pub mod rows {
    pub const TYPE_INDEX: &str = "2203278d-ef4f-4f68-98f1-feb257d53ecc.si";
    pub const NAMES_TABLES: &str = "a07b62f0-1f01-484a-93d9-d42764cedac0.si";
    pub const OWNER_MAP: &str = "42ed49cc-765d-4314-bc2d-af425af7bf13.si";
    pub const SYNONYMS: &str = "facbfffe-feb2-4d30-8930-a557b185e5c4.si";
    pub const TYPE_SETS: &str = "fe8acd6a-22c9-4b5a-aeae-232a1c8324cb.si";
    pub const HELP_PROPS: &str = "c4629235-4823-4320-b8b5-1d08f4c6d612.si";
    pub const XDTO: &str = "ea13a2c9-0c2f-40fa-b855-710387e3271d.si";
}

/// One rewritten cache row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheRow {
    pub name: &'static str,
    /// The inflated text (with the BOM the platform writes).
    pub text: Vec<u8>,
    /// Whether the text is what the platform writes byte for byte. `false` only for
    /// `c4629235` (see [`crate::restructure::caches::help_props`]): the entry is exact, the position
    /// of a few other entries is not.
    pub exact: bool,
}

/// The index of a catalog's generated types: the categories of a reference object and their index.
pub(crate) fn reference_type_slots(facts: &ObjectFacts) -> Result<Vec<TypeSlot>> {
    facts
        .generated
        .iter()
        .map(|generated| {
            let index = match generated.category {
                "Object" => 0,
                "Ref" => 1,
                "Selection" => 2,
                "List" => 3,
                "Manager" => 4,
                other => bail!("the index of the generated type {other} is not measured"),
            };
            Ok(TypeSlot {
                type_id: generated.type_id.clone(),
                value_id: generated.value_id.clone(),
                index,
            })
        })
        .collect()
}

/// What a new catalog needs besides its own row.
pub struct NewCatalog<'a> {
    /// The catalog's descriptor row.
    pub descriptor: &'a Brace,
    /// The configuration's root row **after** the change (the catalog is in its collection).
    pub root: &'a Brace,
    /// The number `DBNames` gave the catalog's table (`_Reference11036` -> 11036).
    pub table_number: u64,
    /// Whether the catalog has a help page (a `Config` row `<uuid>.1`).
    pub has_help: bool,
    /// The property lines of the catalog's attributes in the XDTO model (see
    /// [`crate::restructure::caches::xdto_types::primitive_property`]), and the row types of its
    /// tabular sections.
    pub xdto_attribute_lines: &'a [String],
    pub xdto_section_blocks: &'a [String],
    /// The inflated cache rows by name.
    pub cache: &'a dyn Fn(&str) -> Option<Vec<u8>>,
    /// The name of a catalog by its uuid (the neighbours of the new one in the root).
    pub name_of: &'a dyn Fn(&str) -> Option<String>,
}

fn cached(cache: &dyn Fn(&str) -> Option<Vec<u8>>, name: &str) -> Result<Vec<u8>> {
    cache(name).with_context(|| format!("Params has no row {name}"))
}

/// The rows a new catalog rewrites: `2203278d`, `a07b62f0`, `42ed49cc`, `facbfffe`, `fe8acd6a`,
/// `c4629235` and the XDTO model `ea13a2c9`.
pub fn new_catalog(input: &NewCatalog<'_>) -> Result<Vec<CacheRow>> {
    let facts = ObjectFacts::parse("Catalog", input.descriptor)?;
    let collections = collections(input.root);
    let catalogs = collection_of(&collections, CATALOG_CLASS)?;
    let position = catalogs
        .objects
        .iter()
        .position(|object| *object == facts.uuid)
        .with_context(|| format!("the root does not list catalog {}", facts.name))?;
    let predecessor = position
        .checked_sub(1)
        .map(|index| catalogs.objects[index].as_str());
    let successor = catalogs.objects.get(position + 1).map(String::as_str);
    let mut out = Vec::new();

    // 2203278d: the type index
    let mut index = TypeIndex::parse(&cached(input.cache, rows::TYPE_INDEX)?)?;
    index.refill_section(
        CATALOG_CLASS,
        &catalogs.objects,
        vec![Entry {
            object: facts.uuid.clone(),
            types: reference_type_slots(&facts)?,
        }],
    )?;
    out.push(CacheRow {
        name: rows::TYPE_INDEX,
        text: index.render(),
        exact: true,
    });

    // a07b62f0: names -> tables
    let mut names = NamesTables::parse(&cached(input.cache, rows::NAMES_TABLES)?)?;
    names.add_object(&NewTable {
        kind: "Catalog",
        name: &facts.name,
        object: &facts.uuid,
        table_number: input.table_number,
        ref_type_id: &facts.generated_type("Ref")?.type_id,
        predecessor,
        successor,
    })?;
    out.push(CacheRow {
        name: rows::NAMES_TABLES,
        text: names.render(),
        exact: true,
    });

    // 42ed49cc: owners
    let mut owner_map = OwnerMap::parse(&cached(input.cache, rows::OWNER_MAP)?)?;
    owner_map.add_catalog(&catalogs.objects, &facts.uuid, &facts.references("Owners")?)?;
    out.push(CacheRow {
        name: rows::OWNER_MAP,
        text: owner_map.render(),
        exact: true,
    });

    // facbfffe: presentations
    let mut synonyms = Synonyms::parse(&cached(input.cache, rows::SYNONYMS)?)?;
    for property in [
        "ListPresentation",
        "ExtendedListPresentation",
        "ObjectPresentation",
        "ExtendedObjectPresentation",
        "Explanation",
    ] {
        let pairs = facts.localized(property)?;
        if pairs.is_empty() {
            continue;
        }
        synonyms.add_object(property, &facts.uuid, &pairs, &collections, &kind_of_class)?;
    }
    out.push(CacheRow {
        name: rows::SYNONYMS,
        text: synonyms.render(),
        exact: true,
    });

    // fe8acd6a: sets of types
    let mut sets = TypeSets::parse(&cached(input.cache, rows::TYPE_SETS)?)?;
    let types: Vec<(u32, String)> = reference_type_slots(&facts)?
        .into_iter()
        .map(|slot| (slot.index, slot.type_id))
        .collect();
    sets.add_object("Catalog", &types)?;
    out.push(CacheRow {
        name: rows::TYPE_SETS,
        text: sets.render(),
        exact: true,
    });

    // c4629235: properties (entry exact, position approximate)
    let mut help = HelpProps::parse(&cached(input.cache, rows::HELP_PROPS)?)?;
    help.add_entry_approximately(HelpProps::catalog_entry(&facts, input.has_help)?)?;
    out.push(CacheRow {
        name: rows::HELP_PROPS,
        text: help.render(),
        exact: false,
    });

    // ea13a2c9: the XDTO model
    let neighbour = |uuid: Option<&str>| -> Result<Option<String>> {
        uuid.map(|uuid| {
            (input.name_of)(uuid).with_context(|| format!("the name of catalog {uuid} is unknown"))
        })
        .transpose()
    };
    let predecessor_name = neighbour(predecessor)?;
    let successor_name = neighbour(successor)?;
    let mut model = XdtoModel::parse(&cached(input.cache, rows::XDTO)?)?;
    model.add_catalog(
        &facts.name,
        catalog_shape(&facts)?,
        predecessor_name.as_deref(),
        successor_name.as_deref(),
        input.xdto_attribute_lines,
        input.xdto_section_blocks,
    )?;
    out.push(CacheRow {
        name: rows::XDTO,
        text: model.render(),
        exact: true,
    });
    Ok(out)
}

/// The shape of a catalog for the XDTO standard properties.
pub fn catalog_shape(facts: &ObjectFacts) -> Result<CatalogShape> {
    Ok(CatalogShape {
        hierarchical: facts.number("Hierarchical")? == 1,
        hierarchy_type: facts.number("HierarchyType")?,
        owners: facts.references("Owners")?.len(),
        code_length: facts.number("CodeLength")?,
        code_type: facts.number("CodeType")?,
        description_length: facts.number("DescriptionLength")?,
    })
}

/// The tabular sections of every object of the root that has some, in the root's order
/// (`(section uuid)`), for the `932159f9-...` collection of `owner_class` objects.
pub fn section_traversal(
    collections: &[Collection],
    owner_class: &str,
    descriptor: &dyn Fn(&str) -> Option<Brace>,
) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut seen: BTreeMap<String, ()> = BTreeMap::new();
    for collection in collections.iter().filter(|c| c.class == owner_class) {
        for object in &collection.objects {
            let row = descriptor(object)
                .with_context(|| format!("the descriptor of {object} is not available"))?;
            let facts = ObjectFacts::parse(
                kind_of_class(owner_class).context("an unknown owner class")?,
                &row,
            )?;
            for section in facts.sections {
                if seen.insert(section.uuid.clone(), ()).is_some() {
                    bail!("tabular section {} is listed twice", section.uuid);
                }
                out.push(section.uuid);
            }
        }
    }
    Ok(out)
}

/// What a new tabular section of a catalog needs.
pub struct NewSection<'a> {
    /// The root row (its catalogs are the ones to traverse).
    pub root: &'a Brace,
    /// The descriptors of the catalogs **after** the change, by uuid.
    pub descriptor: &'a dyn Fn(&str) -> Option<Brace>,
    /// The catalog that gets the section, and the section (uuid).
    pub catalog: &'a str,
    pub section: &'a str,
    /// The property lines of the section's attributes in the XDTO model.
    pub xdto_row_lines: &'a [String],
    pub cache: &'a dyn Fn(&str) -> Option<Vec<u8>>,
}

/// The rows a new tabular section of a catalog rewrites: `2203278d` and the XDTO model.
pub fn new_tabular_section(input: &NewSection<'_>) -> Result<Vec<CacheRow>> {
    let collections = collections(input.root);
    let row = (input.descriptor)(input.catalog)
        .with_context(|| format!("the descriptor of {} is not available", input.catalog))?;
    let owner = ObjectFacts::parse("Catalog", &row)?;
    let position = owner
        .sections
        .iter()
        .position(|section| section.uuid == input.section)
        .with_context(|| format!("catalog {} has no section {}", owner.name, input.section))?;
    let section = &owner.sections[position];
    let after = position
        .checked_sub(1)
        .map(|index| owner.sections[index].name.as_str());

    let traversal = section_traversal(&collections, CATALOG_CLASS, input.descriptor)?;
    let mut index = TypeIndex::parse(&cached(input.cache, rows::TYPE_INDEX)?)?;
    let [(section_type, section_value), (row_type, row_value)] = section.types.clone();
    index.refill_section(
        TABULAR_SECTIONS,
        &traversal,
        vec![Entry {
            object: section.uuid.clone(),
            types: vec![
                TypeSlot {
                    type_id: section_type,
                    value_id: section_value,
                    index: 0,
                },
                TypeSlot {
                    type_id: row_type,
                    value_id: row_value,
                    index: 1,
                },
            ],
        }],
    )?;
    let mut out = vec![CacheRow {
        name: rows::TYPE_INDEX,
        text: index.render(),
        exact: true,
    }];

    let mut model = XdtoModel::parse(&cached(input.cache, rows::XDTO)?)?;
    model.add_tabular_section(&owner.name, &section.name, after, input.xdto_row_lines)?;
    out.push(CacheRow {
        name: rows::XDTO,
        text: model.render(),
        exact: true,
    });
    Ok(out)
}
