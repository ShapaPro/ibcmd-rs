//! The plan of the simplest restructure: new attributes in one catalog.
//!
//! Input: the stored schema and names, and the staged image (`ConfigSave`) against the stored one
//! (`Config`). Output: the new `DBSchema` text, the new `DBNames`, and the statements of the rebuild.
//! Nothing here touches a database; `reader` fills the input, `exec` runs the statements.
//!
//! What is checked, fail closed (anything else is refused with the reason):
//! 1. the staged image holds the same files as the stored one (no object added or removed) plus the
//!    `deleted` marker with no deletion in it;
//! 2. across every descriptor row of every kind, no attribute is removed and none changes its type or its
//!    indexing; the new attributes (uuids with no field yet) all sit in the own attributes of **one catalog**;
//! 3. that catalog is otherwise unchanged as far as its table goes (hierarchy, code and description
//!    lengths, owners, tabular sections) and it has no predefined data, no data history, no
//!    subordination;
//! 4. every stored attribute of the catalog maps to the field the stored `DBSchema` entry has (the
//!    generator reproduces the stored entry before it is trusted to extend it);
//! 5. each new attribute has a supported type, is not indexed and is not a nullable field of another type
//!    than a string.
//!
//! Not checked (needs the decoders of the other kinds, that is the restructuring check's job): a changed
//! property of a non-catalog object that changes its table (a document's number length, a register's
//! dimension order, ...). Run this only on an image the check has passed.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use crate::metadata_model::brace::{Brace, parse_row};
use crate::restructure::catalog::{AttributeFacts, CatalogFacts, type_entries};
use crate::restructure::names::{DbNames, deflate, inflate, next_number, version_row};
use crate::restructure::schema::{
    Column, DbSchema, FieldEntry, PhysicalTable, SqlType, TableView, TypeEntry, create_index_sql,
    create_table_sql, field_columns, insert_field, physical_tables,
};
use crate::restructure::storage::EMPTY_GENERATION;
use crate::restructure::xdto;

/// The staged image against the stored one, reduced to what the plan reads.
#[derive(Clone, Debug, Default)]
pub struct StagedImage {
    /// `FileName` of every `Config` row (any part).
    pub old_files: BTreeSet<String>,
    /// `FileName` of every `ConfigSave` row (any part).
    pub new_files: BTreeSet<String>,
    /// The `Config` rows named by a bare uuid (object descriptors), as stored (raw deflate).
    pub old_descriptors: BTreeMap<String, Vec<u8>>,
    /// The same rows of `ConfigSave`.
    pub new_descriptors: BTreeMap<String, Vec<u8>>,
    /// The `deleted` row of `ConfigSave`, as stored.
    pub deleted: Option<Vec<u8>>,
}

/// Everything the plan reads.
#[derive(Clone, Debug, Default)]
pub struct Inputs {
    /// `SchemaStorage.CurrentSchema` of `SchemaID 0`.
    pub schema: Vec<u8>,
    /// `Params.DBNames`, as stored.
    pub main_names: Vec<u8>,
    /// `Params.DBNames-Ext-*`, as stored, by row name.
    pub extension_names: Vec<(String, Vec<u8>)>,
    /// The `RefSInf<n>` tables (predefined-data state) that hold rows.
    pub predefined_tables: BTreeSet<String>,
    /// The `Params` rows `*.si` (derived caches), as stored: name and bytes.
    pub cache_rows: Vec<(String, Vec<u8>)>,
    pub staged: StagedImage,
}

/// Choices that keep a plan reproducible.
#[derive(Clone, Debug, Default)]
pub struct PlanOptions {
    /// The guid of the new `DBNamesVersion-DBNames`; random when absent.
    pub names_version: Option<String>,
    pub method: Method,
    /// Leave the XDTO model cache (`Params` `*.si`) as it is, stale.
    pub skip_xdto: bool,
}

/// How the changed table gets its new column.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Method {
    /// The platform's way: a new-generation copy of the object's tables, then drop and rename.
    #[default]
    Rebuild,
    /// Research (is it acceptable to the platform?): `ALTER TABLE ... ADD`, the column lands at
    /// the end of the physical table instead of in the schema's place.
    AlterAdd,
}

/// A column added in place (`Method::AlterAdd`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlterColumn {
    pub table: String,
    pub column: Column,
    /// The value of the existing rows (a SQL literal or expression).
    pub value: String,
}

/// One new attribute and the field it becomes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Addition {
    pub uuid: String,
    pub name: String,
    /// The number handed out from the shared counter.
    pub number: u64,
    /// `Fld<number>`.
    pub field: FieldEntry,
    /// Where the field went in the table's field list (0 = first).
    pub position: usize,
}

/// One physical table of the rebuilt object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TablePlan {
    /// The columns the table has now.
    pub old_columns: Vec<Column>,
    /// The table it becomes.
    pub table: PhysicalTable,
    /// The column list and the select list of the copy.
    pub insert_columns: Vec<String>,
    pub insert_values: Vec<String>,
}

/// The whole plan.
#[derive(Clone, Debug)]
pub struct Plan {
    /// `Reference20`.
    pub object: String,
    pub object_uuid: String,
    pub object_name: String,
    pub additions: Vec<Addition>,
    /// The new `DBSchema` / `CurrentSchema` text (BOM included).
    pub new_schema: Vec<u8>,
    /// The new `DBNames` text (BOM included) and its stored form.
    pub new_names_text: Vec<u8>,
    pub new_names_row: Vec<u8>,
    /// The new `DBNamesVersion-DBNames` row.
    pub names_version_row: Vec<u8>,
    pub old_schema_sha256: String,
    pub tables: Vec<TablePlan>,
    pub method: Method,
    /// The columns of `Method::AlterAdd`.
    pub alter: Vec<AlterColumn>,
    /// The XDTO model row with the new properties.
    pub xdto: Option<XdtoUpdate>,
}

/// The `Params` row of the XDTO model cache, updated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XdtoUpdate {
    pub row_name: String,
    /// The stored form (raw deflate).
    pub row: Vec<u8>,
    /// The properties added: `(after, name, type)`.
    pub properties: Vec<(String, String, String)>,
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A stored or staged row as text: raw deflate when it is, itself otherwise.
fn row_bytes(stored: &[u8]) -> Vec<u8> {
    inflate(stored).unwrap_or_else(|_| stored.to_vec())
}

// ---------------------------------------------------------------------------
// The staged image against the stored one.

/// What the whole image says about attributes.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Inventory {
    /// The descriptor the attribute sits in.
    owner: String,
    name: String,
    pattern: String,
    /// The wrapper's element after the body (indexing for the shapes that have one).
    flag: String,
}

fn attribute_inventory(
    rows: &BTreeMap<String, Vec<u8>>,
    only: &BTreeSet<String>,
) -> Result<HashMap<String, Inventory>> {
    let mut out = HashMap::new();
    for (owner, stored) in rows {
        if !only.contains(owner) {
            continue;
        }
        let tree = parse_row(&row_bytes(stored))
            .with_context(|| format!("descriptor {owner} is not brace text"))?;
        scan(&tree, owner, &mut out);
    }
    Ok(out)
}

fn is_body(node: &Brace) -> bool {
    let Some(items) = node.as_list() else {
        return false;
    };
    items.first().and_then(Brace::as_atom) == Some("27")
        && items.get(1).and_then(Brace::as_list).is_some_and(|typed| {
            typed.first().and_then(Brace::as_atom) == Some("2")
                && typed
                    .get(2)
                    .and_then(Brace::as_list)
                    .and_then(|pattern| pattern.first())
                    .and_then(Brace::as_str)
                    == Some("Pattern")
        })
}

fn register(body: &Brace, flag: &str, owner: &str, out: &mut HashMap<String, Inventory>) {
    let typed = body.at(&[1]).and_then(Brace::as_list).unwrap_or_default();
    let uuid = typed
        .get(1)
        .and_then(|header| header.at(&[1, 2]))
        .and_then(Brace::as_atom)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let name = typed
        .get(1)
        .and_then(|header| header.at(&[2]))
        .and_then(Brace::as_str)
        .unwrap_or_default()
        .to_owned();
    let pattern = typed
        .get(2)
        .map(crate::metadata_model::brace::serialize)
        .unwrap_or_default();
    out.entry(uuid).or_insert(Inventory {
        owner: owner.to_owned(),
        name,
        pattern,
        flag: flag.to_owned(),
    });
}

fn scan(node: &Brace, owner: &str, out: &mut HashMap<String, Inventory>) {
    let Some(items) = node.as_list() else {
        return;
    };
    // A catalog's attributes: the collection `{<class>,<n>,{{v,<body>,<indexing>,<use>,...},0}...}`.
    // `Use` decides whether the field is nullable, so it is part of what must not change.
    if items.first().and_then(Brace::as_atom) == Some(crate::restructure::catalog::ATTRIBUTES) {
        for item in items.iter().skip(2) {
            let Some(record) = item.at(&[0]).and_then(Brace::as_list) else {
                continue;
            };
            if let Some(body) = record.get(1).filter(|body| is_body(body)) {
                let atom = |index: usize| {
                    record
                        .get(index)
                        .and_then(Brace::as_atom)
                        .unwrap_or_default()
                };
                register(body, &format!("{}|{}", atom(2), atom(3)), owner, out);
            }
        }
    }
    // Any other wrapper record {v,<body>,<indexing>...}: the element after the body is the indexing.
    if let Some(body) = items.get(1).filter(|body| is_body(body)) {
        let indexing = items.get(2).and_then(Brace::as_atom).unwrap_or_default();
        register(body, &format!("{indexing}|"), owner, out);
    }
    if is_body(node) {
        register(node, "", owner, out);
    }
    for child in items {
        scan(child, owner, out);
    }
}

/// The staged image may be the whole configuration (what a native `config import` stages) or only the
/// files that changed (`import files --partial`): a file that is not staged stays as it is. What it must
/// not do is add a file (a new object) or delete one (the `deleted` marker names the deletions).
fn check_files(image: &StagedImage) -> Result<()> {
    let added: Vec<_> = image
        .new_files
        .difference(&image.old_files)
        .filter(|name| name.as_str() != "deleted")
        .collect();
    if !added.is_empty() {
        bail!(
            "the staged image adds {} file(s) (first: {:?}): a new object is not supported",
            added.len(),
            added.first()
        );
    }
    if let Some(deleted) = &image.deleted {
        let text = String::from_utf8_lossy(&row_bytes(deleted)).into_owned();
        if text.trim_start_matches('\u{feff}').trim() != "0" {
            bail!("the staged image deletes objects ({text:?}): not supported");
        }
    }
    Ok(())
}

struct Change {
    /// The descriptor of the catalog that got attributes.
    catalog: String,
    /// The new attributes' uuids.
    added: Vec<String>,
}

/// Compares the attribute inventory of the two images.
fn find_change(image: &StagedImage) -> Result<Change> {
    // Only descriptors whose text differs are parsed: the rest cannot have changed a fact.
    let mut differing = BTreeSet::new();
    for (name, new) in &image.new_descriptors {
        match image.old_descriptors.get(name) {
            Some(old) if row_bytes(old) == row_bytes(new) => {}
            _ => {
                differing.insert(name.clone());
            }
        }
    }
    let old = attribute_inventory(&image.old_descriptors, &differing)?;
    let new = attribute_inventory(&image.new_descriptors, &differing)?;
    let mut added = Vec::new();
    for (uuid, entry) in &new {
        match old.get(uuid) {
            None => added.push(uuid.clone()),
            Some(before) => {
                if before.pattern != entry.pattern {
                    bail!(
                        "attribute {} of {} changes its type: not supported",
                        entry.name,
                        entry.owner
                    );
                }
                if before.flag != entry.flag {
                    bail!(
                        "attribute {} of {} changes its indexing or flags ({:?} -> {:?}): not supported",
                        entry.name,
                        entry.owner,
                        before.flag,
                        entry.flag
                    );
                }
                if before.owner != entry.owner {
                    bail!(
                        "attribute {} moves between objects: not supported",
                        entry.name
                    );
                }
            }
        }
    }
    if let Some((uuid, entry)) = old.iter().find(|(uuid, _)| !new.contains_key(*uuid)) {
        bail!(
            "attribute {} ({uuid}) of {} is removed: not supported",
            entry.name,
            entry.owner
        );
    }
    if added.is_empty() {
        bail!("the staged image adds no attribute: nothing this prototype restructures");
    }
    added.sort();
    let owners: BTreeSet<&str> = added.iter().map(|uuid| new[uuid].owner.as_str()).collect();
    if owners.len() != 1 {
        bail!(
            "new attributes sit in {} objects, this prototype takes one catalog",
            owners.len()
        );
    }
    let catalog = owners.into_iter().next().unwrap_or_default().to_owned();
    Ok(Change { catalog, added })
}

// ---------------------------------------------------------------------------
// The plan.

/// The select-list expression of a column of the new table: the old column of the same name,
/// or the default of a column the old table did not have.
fn select_expr(
    column: &Column,
    alias: &str,
    old: &HashMap<&str, &Column>,
    has_folder: bool,
    entry: Option<&TypeEntry>,
) -> Result<String> {
    if let Some(before) = old.get(column.name.as_str()) {
        if before.sql_type != column.sql_type || before.nullable != column.nullable {
            bail!(
                "column {} changes from {:?} to {:?}: a type change is converted by the platform, not by this prototype",
                column.name,
                before.sql_type,
                column.sql_type
            );
        }
        return Ok(format!("{alias}.{}", column.name));
    }
    default_expr(column, has_folder, entry)
}

/// The value of a column the old table did not have (measured in cases a2 and h).
fn default_expr(column: &Column, has_folder: bool, entry: Option<&TypeEntry>) -> Result<String> {
    if column.nullable && !has_folder {
        bail!(
            "no default is verified for the nullable column {}",
            column.name
        );
    }
    let value = match &column.sql_type {
        SqlType::Binary(1) => "0x00".to_owned(),
        SqlType::Binary(n) => format!("0x{}", "00".repeat(*n as usize)),
        SqlType::VarBinaryMax => match entry {
            Some(entry) if entry.tag == "B" && entry.a == 0x8000_0000 => {
                "0x01010800000000000000EFBBBF7B2255227D".to_owned()
            }
            _ => bail!(
                "no default is verified for the binary column {}",
                column.name
            ),
        },
        SqlType::Numeric(p, s) => format!("CAST(0 AS NUMERIC({p}, {s}))"),
        SqlType::Int | SqlType::BigInt => "0".to_owned(),
        SqlType::DateTime2 => "CAST('2001-01-01T00:00:00' AS DATETIME2(0))".to_owned(),
        SqlType::NVarChar(_) | SqlType::NVarCharMax => "N''".to_owned(),
        SqlType::NChar(n) => format!("CAST(N'{}' AS NCHAR({n}))", " ".repeat(*n as usize)),
        other => bail!("no default is known for {other:?} ({})", column.name),
    };
    if column.nullable {
        // A field of a hierarchical catalog that only items have: NULL for folders,
        // the default for items (`_Folder = 0x01` marks an item).
        return match &column.sql_type {
            SqlType::NVarChar(n) => Ok(format!(
                "CAST(CASE WHEN T1._Folder = 0x01 THEN {value} END AS NVARCHAR({n}))"
            )),
            SqlType::NVarCharMax => Ok(format!(
                "CAST(CASE WHEN T1._Folder = 0x01 THEN {value} END AS NVARCHAR(MAX))"
            )),
            _ => bail!(
                "no default is verified for the nullable column {}",
                column.name
            ),
        };
    }
    Ok(value)
}

/// The columns of the copy, in the order the platform lists them: the fields first, then, for a
/// sub-table, the separator columns, the owner key and the line key.
fn insert_order(table: &PhysicalTable) -> Vec<&Column> {
    let prefix = table.implicit_prefix;
    if prefix == 0 {
        return table.columns.iter().collect();
    }
    let (head, fields) = table.columns.split_at(prefix);
    let mut order: Vec<&Column> = fields.iter().collect();
    order.extend(&head[1..prefix - 1]);
    order.push(&head[0]);
    order.push(&head[prefix - 1]);
    order
}

/// Builds the plan.
pub fn plan(inputs: &Inputs, options: &PlanOptions) -> Result<Plan> {
    check_files(&inputs.staged)?;
    let change = find_change(&inputs.staged)?;

    let old_row = inputs
        .staged
        .old_descriptors
        .get(&change.catalog)
        .context("the stored image has no descriptor of the changed object")?;
    let new_row = inputs
        .staged
        .new_descriptors
        .get(&change.catalog)
        .context("the staged image has no descriptor of the changed object")?;
    let old_facts = CatalogFacts::parse(&parse_row(&row_bytes(old_row))?).with_context(|| {
        format!(
            "descriptor {} is not a catalog this prototype reads",
            change.catalog
        )
    })?;
    let new_facts = CatalogFacts::parse(&parse_row(&row_bytes(new_row))?).with_context(|| {
        format!(
            "staged descriptor {} is not a catalog this prototype reads",
            change.catalog
        )
    })?;
    check_catalog(&old_facts, &new_facts, &change)?;

    let schema = DbSchema::parse(&inputs.schema).context("the stored DBSchema")?;
    let main_names = DbNames::parse_row(&inputs.main_names).context("the stored DBNames")?;
    let extension_names = inputs
        .extension_names
        .iter()
        .map(|(name, row)| DbNames::parse_row(row).with_context(|| format!("the stored {name}")))
        .collect::<Result<Vec<_>>>()?;

    let number = main_names
        .number_of(&new_facts.uuid, "Reference")
        .with_context(|| format!("catalog {} has no table number", new_facts.name))?;
    let object = format!("Reference{number}");
    // The tables the object's other names stand for. The change-registration table of an exchange
    // plan (`ReferenceChngR`) is not rebuilt with the object (case a2); the predefined-data
    // table (`RefSInf`) is empty for a catalog without predefined items and was not rebuilt
    // either, but for the catalog that has them (14 of 75) the platform rebuilt it with the
    // object (case h): that one is refused. Any other table of the object is refused.
    for entry in main_names.entries_of(&new_facts.uuid) {
        if entry.kind == "Reference" {
            continue;
        }
        let table = format!("{}{}", entry.kind, entry.number);
        if schema.position(&table).is_none() {
            continue;
        }
        let harmless = match entry.kind.as_str() {
            "ReferenceChngR" => true,
            "RefSInf" => !inputs.predefined_tables.contains(&table),
            _ => false,
        };
        if !harmless {
            bail!(
                "catalog {} has the companion table {table}: the platform rebuilds it with the object (predefined data?), not supported",
                new_facts.name
            );
        }
    }
    let old_position = schema
        .position(&object)
        .with_context(|| format!("the stored DBSchema has no table {object}"))?;
    let old_view = schema.view(old_position)?;
    let old_fields = old_view.fields()?;
    check_stored_fields(&old_facts, &old_view, &old_fields, &main_names)?;

    // Numbers come from the counter shared with the extensions.
    let mut counter_sources: Vec<&DbNames> = extension_names.iter().collect();
    counter_sources.push(&main_names);
    let mut next = next_number(counter_sources.iter().copied());
    let has_folder = old_fields.iter().any(|field| field.name == "Folder");

    let mut additions = Vec::new();
    let mut fields_now = old_fields.clone();
    let mut names_after = main_names.clone();
    // Metadata order decides both the numbers and the places.
    for (index, attribute) in new_facts.attributes.iter().enumerate() {
        if !change.added.contains(&attribute.uuid) {
            continue;
        }
        if attribute.indexing != Some(0) {
            bail!(
                "attribute {} is indexed: an index is a separate case of the rebuild",
                attribute.name
            );
        }
        let entries = type_entries(&attribute.pattern_node)
            .with_context(|| format!("attribute {}", attribute.name))?;
        let nullable = has_folder && attribute.usage == Some(0);
        if has_folder && attribute.usage == Some(1) {
            bail!(
                "attribute {} is for folders: its nullability is not verified",
                attribute.name
            );
        }
        let field = FieldEntry::new(&format!("Fld{next}"), nullable, entries);
        // After the field of the attribute before it; the first attribute goes after the
        // standard fields.
        let previous = new_facts.attributes[..index].last();
        let position = match previous {
            Some(previous) => {
                let previous_number = names_after
                    .number_of(&previous.uuid, "Fld")
                    .with_context(|| format!("attribute {} has no field number", previous.name))?;
                fields_now
                    .iter()
                    .position(|candidate| candidate.name == format!("Fld{previous_number}"))
                    .with_context(|| {
                        format!("the stored table has no field Fld{previous_number}")
                    })?
                    + 1
            }
            None => {
                const STANDARD: [&str; 9] = [
                    "ID",
                    "Version",
                    "Marked",
                    "PredefinedID",
                    "OwnerID",
                    "ParentID",
                    "Folder",
                    "Code",
                    "Description",
                ];
                fields_now
                    .iter()
                    .rposition(|candidate| STANDARD.contains(&candidate.name.as_str()))
                    .map_or(0, |last| last + 1)
            }
        };
        fields_now.insert(position, field.clone());
        names_after.append(&attribute.uuid, "Fld", next)?;
        additions.push(Addition {
            uuid: attribute.uuid.clone(),
            name: attribute.name.clone(),
            number: next,
            field,
            position,
        });
        next += 1;
    }
    if additions.len() != change.added.len() {
        bail!("some new attributes are not among the catalog's own attributes");
    }
    let xdto = if options.skip_xdto {
        None
    } else {
        Some(xdto_update(inputs, &new_facts, &additions)?)
    };

    // The new table entry: the stored one with the fields inserted.
    let mut entry = schema.tables()[old_position].clone();
    for addition in &additions {
        insert_field(&mut entry, addition.position, &addition.field)?;
    }
    let new_view = TableView::new(&entry)?;
    let old_tables = physical_tables(&old_view)?;
    let new_tables = physical_tables(&new_view)?;
    if old_tables.len() != new_tables.len() {
        bail!("the rebuild changes the number of tables of {object}");
    }
    let mut tables = Vec::new();
    for (index, (before, after)) in old_tables.into_iter().zip(new_tables).enumerate() {
        if before.name != after.name {
            bail!("table {} became {}", before.name, after.name);
        }
        let alias = format!("T{}", index + 1);
        let old_columns: HashMap<&str, &Column> = before
            .columns
            .iter()
            .map(|column| (column.name.as_str(), column))
            .collect();
        let mut insert_columns = Vec::new();
        let mut insert_values = Vec::new();
        for column in insert_order(&after) {
            if column.sql_type == SqlType::Timestamp {
                continue;
            }
            if column.identity {
                bail!(
                    "column {} is an identity column: not supported",
                    column.name
                );
            }
            // The type entry of the new field this column belongs to (`_Fld1`, `_Fld1_S`, ...).
            let entry_of_new = additions.iter().find_map(|addition| {
                let base = format!("_{}", addition.field.name);
                (column.name == base || column.name.starts_with(&format!("{base}_")))
                    .then(|| addition.field.types.first())
                    .flatten()
            });
            insert_columns.push(column.name.clone());
            insert_values.push(select_expr(
                column,
                &alias,
                &old_columns,
                has_folder,
                entry_of_new,
            )?);
        }
        tables.push(TablePlan {
            old_columns: before.columns.clone(),
            table: after,
            insert_columns,
            insert_values,
        });
    }

    // Publish: the schema with the entry moved before ConfigChngR (rebuilt tables sit at the end
    // of the list, ConfigChngR last), the names with the new numbers.
    let mut new_schema = schema.clone();
    new_schema.remove(&object)?;
    new_schema.insert_before("ConfigChngR", entry);
    let new_names_text = names_after.to_text();
    let new_names_row = deflate(&new_names_text)?;
    let version = options
        .names_version
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let mut alter = Vec::new();
    if options.method == Method::AlterAdd {
        for addition in &additions {
            let columns = field_columns(&addition.field)?;
            let [column] = &columns[..] else {
                bail!(
                    "the alter method takes fields of one column, {} has {}",
                    addition.name,
                    columns.len()
                );
            };
            let mut plain = column.clone();
            plain.nullable = false;
            alter.push(AlterColumn {
                table: format!("_{object}"),
                value: default_expr(&plain, has_folder, addition.field.types.first())?,
                column: column.clone(),
            });
        }
    }
    Ok(Plan {
        object,
        object_uuid: new_facts.uuid.clone(),
        object_name: new_facts.name.clone(),
        additions,
        new_schema: new_schema.to_text(),
        new_names_text,
        new_names_row,
        names_version_row: version_row(&version),
        old_schema_sha256: sha256_hex(&inputs.schema),
        tables,
        method: options.method,
        alter,
        xdto,
    })
}

/// The XDTO model row with a property for each new attribute, after the property of the attribute
/// before it.
fn xdto_update(
    inputs: &Inputs,
    facts: &CatalogFacts,
    additions: &[Addition],
) -> Result<XdtoUpdate> {
    let mut properties = Vec::new();
    for addition in additions {
        let index = facts
            .attributes
            .iter()
            .position(|attribute| attribute.uuid == addition.uuid)
            .context("a new attribute is not among the catalog's attributes")?;
        let Some(previous) = index.checked_sub(1).map(|before| &facts.attributes[before]) else {
            bail!(
                "attribute {} is the catalog's first: its place among the XDTO properties is not known (--skip-xdto leaves the cache stale)",
                addition.name
            );
        };
        let entry = addition
            .field
            .types
            .first()
            .context("a new field has no type")?;
        properties.push((
            previous.name.clone(),
            addition.name.clone(),
            xdto::property_type(entry)?.to_owned(),
        ));
    }
    let needle = format!("CatalogObject.{}", facts.name);
    for (name, stored) in &inputs.cache_rows {
        let text = inflate(stored).unwrap_or_default();
        if !xdto::is_model(&text) || !xdto::has_object_type(&text, &needle)? {
            continue;
        }
        let mut text = text;
        for (after, property, property_type) in &properties {
            text = xdto::add_catalog_property(&text, &facts.name, after, property, property_type)?;
        }
        return Ok(XdtoUpdate {
            row_name: name.clone(),
            row: deflate(&text)?,
            properties,
        });
    }
    bail!("no XDTO model cache row knows {needle}: pass --skip-xdto to leave the caches alone")
}

/// The catalog is the same as far as the table goes, except for the appended attributes.
fn check_catalog(old: &CatalogFacts, new: &CatalogFacts, change: &Change) -> Result<()> {
    if old.uuid != new.uuid || old.uuid != change.catalog {
        bail!("the changed descriptor is not the catalog the attributes sit in");
    }
    let shape = |facts: &CatalogFacts| {
        (
            facts.hierarchical,
            facts.hierarchy_type,
            facts.code_length,
            facts.code_type,
            facts.code_allowed_length,
            facts.description_length,
            facts.owners,
            facts.data_history,
        )
    };
    if shape(old) != shape(new) {
        bail!(
            "catalog {} changes its hierarchy, code, description, owners or data history: not supported",
            new.name
        );
    }
    if old.owners != 0 {
        bail!(
            "catalog {} is subordinate to owners: its owner field is not covered",
            new.name
        );
    }
    if old.data_history != 0 {
        bail!("catalog {} keeps data history: not supported", new.name);
    }
    let same_sections = old.sections.len() == new.sections.len()
        && old
            .sections
            .iter()
            .zip(&new.sections)
            .all(|(before, after)| {
                before.uuid == after.uuid
                    && before.attributes.len() == after.attributes.len()
                    && before
                        .attributes
                        .iter()
                        .zip(&after.attributes)
                        .all(|(a, b)| a.uuid == b.uuid && a.pattern == b.pattern)
            });
    if !same_sections {
        bail!(
            "catalog {} changes a tabular section: not supported",
            new.name
        );
    }
    // The stored attributes are the new ones in the same order, with the same type and indexing.
    let kept: Vec<&AttributeFacts> = new
        .attributes
        .iter()
        .filter(|attribute| !change.added.contains(&attribute.uuid))
        .collect();
    let same_attributes = kept.len() == old.attributes.len()
        && kept.iter().zip(&old.attributes).all(|(after, before)| {
            after.uuid == before.uuid
                && after.pattern == before.pattern
                && after.indexing == before.indexing
                && after.usage == before.usage
        });
    if !same_attributes {
        bail!(
            "catalog {} reorders or changes its stored attributes: not supported",
            new.name
        );
    }
    Ok(())
}

/// Every stored attribute has its field in the stored table, and the fields the generator can
/// compute agree with the stored ones.
fn check_stored_fields(
    facts: &CatalogFacts,
    table: &TableView<'_>,
    fields: &[FieldEntry],
    names: &DbNames,
) -> Result<()> {
    let has_folder = fields.iter().any(|field| field.name == "Folder");
    let mut last = None;
    for attribute in &facts.attributes {
        let number = names
            .number_of(&attribute.uuid, "Fld")
            .with_context(|| format!("attribute {} has no number in DBNames", attribute.name))?;
        let field_name = format!("Fld{number}");
        let position = fields
            .iter()
            .position(|field| field.name == field_name)
            .with_context(|| {
                format!(
                    "table {} has no field {field_name} for attribute {}",
                    table.name(),
                    attribute.name
                )
            })?;
        if last.is_some_and(|last| position <= last) {
            bail!(
                "the stored fields of {} are not in metadata order",
                table.name()
            );
        }
        last = Some(position);
        if let Ok(entries) = type_entries(&attribute.pattern_node) {
            let expected_nullable = has_folder && attribute.usage == Some(0);
            let stored = &fields[position];
            if stored.types != entries || stored.nullable != expected_nullable {
                bail!(
                    "the stored field {field_name} of attribute {} differs from what the generator makes of it: {:?} vs {:?}",
                    attribute.name,
                    stored,
                    FieldEntry::new(&field_name, expected_nullable, entries)
                );
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The statements.

/// One statement of the restructure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Statement {
    pub phase: Phase,
    pub label: String,
    pub sql: String,
    pub params: Vec<StatementParam>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Guard,
    Create,
    Load,
    Indexes,
    DropOld,
    Rename,
    Publish,
}

/// A parameter of a statement (`@P1`, ...).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StatementParam {
    Bytes(Vec<u8>),
    Int(i64),
}

impl Plan {
    /// The NG name of a table or index.
    fn ng(name: &str) -> String {
        format!("{name}NG")
    }

    /// The statements in execution order, without the transaction control and the
    /// verification steps `exec` adds between the phases.
    pub fn statements(&self) -> Vec<Statement> {
        let mut out = Vec::new();
        let mut push = |phase: Phase, label: String, sql: String, params: Vec<StatementParam>| {
            out.push(Statement {
                phase,
                label,
                sql,
                params,
            });
        };
        push(
            Phase::Guard,
            "the stored schema is the one the plan was made from".to_owned(),
            format!(
                "IF NOT EXISTS (SELECT 1 FROM dbo.SchemaStorage WHERE SchemaID = 0 AND Status = 100 \
                 AND DATALENGTH(NewGenCreated) = {len} AND DATALENGTH(NewGenDropped) = {len} \
                 AND CONVERT(varchar(64), HASHBYTES('SHA2_256', CurrentSchema), 2) = '{hash}') \
                 THROW 51000, 'SchemaStorage is not idle or its schema is not the planned one', 1;",
                len = EMPTY_GENERATION.len(),
                hash = self.old_schema_sha256.to_ascii_uppercase()
            ),
            Vec::new(),
        );
        if self.method == Method::AlterAdd {
            for added in &self.alter {
                let column = &added.column;
                if column.nullable {
                    push(
                        Phase::Create,
                        format!("alter {} add {}", added.table, column.name),
                        format!(
                            "ALTER TABLE dbo.{} ADD {} {} NULL;",
                            added.table,
                            column.name,
                            column.sql_type.ddl()
                        ),
                        Vec::new(),
                    );
                    push(
                        Phase::Create,
                        format!("set {} of the existing items", column.name),
                        format!(
                            "UPDATE dbo.{} SET {} = {} WHERE _Folder = 0x01;",
                            added.table, column.name, added.value
                        ),
                        Vec::new(),
                    );
                } else {
                    push(
                        Phase::Create,
                        format!("alter {} add {}", added.table, column.name),
                        format!(
                            "ALTER TABLE dbo.{table} ADD {name} {ty} NOT NULL CONSTRAINT DF_ddl_tmp DEFAULT {value};
ALTER TABLE dbo.{table} DROP CONSTRAINT DF_ddl_tmp;",
                            table = added.table,
                            name = column.name,
                            ty = column.sql_type.ddl(),
                            value = added.value
                        ),
                        Vec::new(),
                    );
                }
            }
        } else {
            for table in &self.tables {
                push(
                    Phase::Create,
                    format!("create {}NG", table.table.name),
                    create_table_sql(&table.table, "NG"),
                    Vec::new(),
                );
            }
            for (index, table) in self.tables.iter().enumerate() {
                push(
                    Phase::Load,
                    format!("copy {} into {}NG", table.table.name, table.table.name),
                    format!(
                        "INSERT INTO dbo.{new}NG WITH(TABLOCK) ({columns}) SELECT\n{values}\nFROM dbo.{new} T{alias} WITH(NOLOCK);",
                        new = table.table.name,
                        columns = table.insert_columns.join(", "),
                        values = table.insert_values.join(",\n"),
                        alias = index + 1
                    ),
                    Vec::new(),
                );
            }
            for table in &self.tables {
                for index in &table.table.indexes {
                    push(
                        Phase::Indexes,
                        format!("index {}NG", index.name),
                        create_index_sql(&table.table.name, index, "NG"),
                        Vec::new(),
                    );
                }
            }
            for table in &self.tables {
                push(
                    Phase::DropOld,
                    format!("drop {}", table.table.name),
                    format!("drop table dbo.{};", table.table.name),
                    Vec::new(),
                );
            }
            for table in &self.tables {
                push(
                    Phase::Rename,
                    format!("rename {}NG", table.table.name),
                    format!(
                        "EXEC sp_rename N'{}', N'{}', 'OBJECT';",
                        Self::ng(&table.table.name),
                        table.table.name
                    ),
                    Vec::new(),
                );
            }
            for table in &self.tables {
                for index in table
                    .table
                    .indexes
                    .iter()
                    .filter(|index| !index.name.is_empty())
                {
                    push(
                        Phase::Rename,
                        format!("rename index {}NG", index.name),
                        format!(
                            "EXEC sp_rename N'{}.{}', N'{}', 'INDEX';",
                            table.table.name,
                            Self::ng(&index.name),
                            index.name
                        ),
                        Vec::new(),
                    );
                }
            }
        }
        push(
            Phase::Publish,
            "SchemaStorage: idle, the new schema, empty generations".to_owned(),
            "UPDATE dbo.SchemaStorage SET Status = 100, CurrentSchema = @P1, NewGenCreated = @P2, NewGenDropped = @P3 WHERE SchemaID = 0;"
                .to_owned(),
            vec![
                StatementParam::Bytes(self.new_schema.clone()),
                StatementParam::Bytes(EMPTY_GENERATION.to_vec()),
                StatementParam::Bytes(EMPTY_GENERATION.to_vec()),
            ],
        );
        push(
            Phase::Publish,
            "DBSchema".to_owned(),
            "UPDATE dbo.DBSchema SET SerializedData = @P1;".to_owned(),
            vec![StatementParam::Bytes(self.new_schema.clone())],
        );
        for (name, row) in [
            ("DBNames", &self.new_names_row),
            ("DBNamesVersion-DBNames", &self.names_version_row),
        ] {
            push(
                Phase::Publish,
                format!("Params {name}"),
                format!(
                    "UPDATE dbo.Params SET BinaryData = @P1, DataSize = @P2, Modified = DATEADD(YEAR, 2000, SYSDATETIME()) WHERE FileName = N'{name}' AND PartNo = 0;"
                ),
                vec![
                    StatementParam::Bytes(row.clone()),
                    StatementParam::Int(row.len() as i64),
                ],
            );
        }
        if let Some(update) = &self.xdto {
            push(
                Phase::Publish,
                format!(
                    "Params {}: the XDTO model with the new properties",
                    update.row_name
                ),
                format!(
                    "UPDATE dbo.Params SET BinaryData = @P1, DataSize = @P2, Modified = DATEADD(YEAR, 2000, SYSDATETIME()) WHERE FileName = N'{}' AND PartNo = 0;",
                    update.row_name
                ),
                vec![
                    StatementParam::Bytes(update.row.clone()),
                    StatementParam::Int(update.row.len() as i64),
                ],
            );
        }
        push(
            Phase::Publish,
            "Config: the changed staged rows replace the stored ones".to_owned(),
            PROMOTE_CONFIG.to_owned(),
            Vec::new(),
        );
        push(
            Phase::Publish,
            "ConfigSave: emptied".to_owned(),
            "DELETE FROM dbo.ConfigSave;".to_owned(),
            Vec::new(),
        );
        out
    }
}

/// The staged files replace the stored ones (a file that is not staged stays; the `deleted` marker is
/// not copied): the parts of a staged file that differ are deleted, the missing ones inserted. The same
/// statements serve a whole-configuration stage and a partial one.
pub const PROMOTE_CONFIG: &str = "\
DELETE c FROM dbo.Config c WHERE c.FileName IN (SELECT FileName FROM dbo.ConfigSave WHERE FileName <> N'deleted') \
AND NOT EXISTS (SELECT 1 FROM dbo.ConfigSave s WHERE s.FileName = c.FileName AND s.PartNo = c.PartNo \
AND s.Creation = c.Creation AND s.Modified = c.Modified AND s.Attributes = c.Attributes AND s.DataSize = c.DataSize AND s.BinaryData = c.BinaryData);
INSERT INTO dbo.Config (FileName, Creation, Modified, Attributes, DataSize, BinaryData, PartNo) \
SELECT s.FileName, s.Creation, s.Modified, s.Attributes, s.DataSize, s.BinaryData, s.PartNo FROM dbo.ConfigSave s \
WHERE s.FileName <> N'deleted' AND NOT EXISTS (SELECT 1 FROM dbo.Config c WHERE c.FileName = s.FileName AND c.PartNo = s.PartNo);";

/// A one-line summary for reports.
impl Plan {
    pub fn summary(&self) -> String {
        format!(
            "{} ({}): {} new attribute(s) -> {}",
            self.object_name,
            self.object,
            self.additions.len(),
            self.additions
                .iter()
                .map(|addition| format!("{} = {}", addition.field.name, addition.name))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}
