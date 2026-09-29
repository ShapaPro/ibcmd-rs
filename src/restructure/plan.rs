//! The plan of the restructure: new attributes in catalogs and documents.
//!
//! Input: the stored schema and names, and the staged image (`ConfigSave`) against the stored one
//! (`Config`). Output: the new `DBSchema` text, the new `DBNames`, the derived caches (`Params` `*.si`)
//! and the statements of the rebuild. Nothing here touches a database; `reader` fills the input, `exec`
//! runs the statements.
//!
//! What is checked, fail closed (anything else is refused with the reason):
//! 1. the staged image holds the same files as the stored one (no object added or removed) plus the
//!    `deleted` marker with no deletion in it;
//! 2. across every descriptor row of every kind, no attribute is removed and none changes its type or its
//!    indexing; the new attributes (uuids with no field yet) all sit in the own attributes of catalogs or
//!    documents;
//! 3. each such object is otherwise unchanged as far as its table goes (its shape: hierarchy, code and
//!    description lengths, number length, ...; its tabular sections) and it has no predefined data, no data
//!    history, no subordination;
//! 4. every stored attribute of it maps to the field the stored `DBSchema` entry has (the generator
//!    reproduces the stored entry before it is trusted to extend it);
//! 5. each new attribute has a supported type and is not indexed.
//!
//! Not checked (needs the decoders of the other kinds, that is the restructuring check's job): a changed
//! property of another object that changes its table (a register's dimension order, ...). Run this only on
//! an image the check has passed.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use crate::metadata_model::brace::{Brace, parse_row};
use crate::restructure::catalog::{AttributeFacts, type_entries};
use crate::restructure::names::{DbNames, deflate, inflate, next_number, version_row};
use crate::restructure::object::{ObjectFacts, ObjectKind};
use crate::restructure::registry::{self, ObjectAdditions};
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
    /// The `Params` rows `*.si` (derived caches) and `siVersions`, as stored: name and bytes.
    pub cache_rows: Vec<(String, Vec<u8>)>,
    /// The `root` row of `Config`, as stored: it names the configuration whose descriptor lists the
    /// objects in the order the platform walks them.
    pub root_row: Vec<u8>,
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
    /// Leave the object registry (`Params` `1a621f0f-....si`) as it is, stale.
    pub skip_registry: bool,
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
    /// For a nullable column: the `_Folder` value of the rows that get the value (the others stay NULL).
    pub marker: Option<String>,
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
    /// `Use` of a catalog attribute (0 `ForItem`, 1 `ForFolder`, 2 `ForFolderAndItem`).
    pub usage: Option<i64>,
}

impl Addition {
    /// The `_Folder` value of the rows that get the default when the field is nullable: items for an
    /// attribute `ForItem`, folders for one `ForFolder`; the other rows get NULL.
    pub fn folder_marker(&self) -> Option<&'static str> {
        match self.usage {
            Some(0) => Some("0x01"),
            Some(1) => Some("0x00"),
            _ => None,
        }
    }
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

/// One rebuilt object.
#[derive(Clone, Debug)]
pub struct ObjectPlan {
    pub kind: ObjectKind,
    /// `Reference20`, `Document39`.
    pub object: String,
    pub object_uuid: String,
    pub object_name: String,
    pub additions: Vec<Addition>,
    pub tables: Vec<TablePlan>,
    /// The columns of `Method::AlterAdd`.
    pub alter: Vec<AlterColumn>,
}

/// A `Params` row of the derived caches, rewritten.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheUpdate {
    pub row_name: String,
    /// The stored form (raw deflate, or plain text for `siVersions`).
    pub row: Vec<u8>,
    /// What changed in it.
    pub what: String,
    /// The platform writes the `*.si` rows anew (`Creation` moves too); `siVersions` only moves `Modified`.
    pub set_creation: bool,
}

/// The whole plan.
#[derive(Clone, Debug)]
pub struct Plan {
    /// The rebuilt objects, in the order the platform handles them.
    pub objects: Vec<ObjectPlan>,
    /// The new `DBSchema` / `CurrentSchema` text (BOM included).
    pub new_schema: Vec<u8>,
    /// The new `DBNames` text (BOM included) and its stored form.
    pub new_names_text: Vec<u8>,
    pub new_names_row: Vec<u8>,
    /// The new `DBNamesVersion-DBNames` row.
    pub names_version_row: Vec<u8>,
    pub old_schema_sha256: String,
    /// Size and SHA-256 (hex) of the stored `DBNames` row the plan was made from.
    pub old_names_size: usize,
    pub old_names_sha256: String,
    pub method: Method,
    /// The derived caches rewritten with the plan.
    pub caches: Vec<CacheUpdate>,
}

impl Plan {
    /// Every physical table of every rebuilt object.
    pub fn tables(&self) -> impl Iterator<Item = &TablePlan> {
        self.objects.iter().flat_map(|object| object.tables.iter())
    }

    /// Every new attribute of every object.
    pub fn additions(&self) -> impl Iterator<Item = &Addition> {
        self.objects
            .iter()
            .flat_map(|object| object.additions.iter())
    }
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

/// The new attributes of one object.
struct Change {
    /// The descriptor of the object that got attributes.
    owner: String,
    /// The new attributes' uuids.
    added: BTreeSet<String>,
}

/// Compares the attribute inventory of the two images: the new attributes by object.
fn find_changes(image: &StagedImage) -> Result<Vec<Change>> {
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
    let mut added: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (uuid, entry) in &new {
        match old.get(uuid) {
            None => {
                added
                    .entry(entry.owner.clone())
                    .or_default()
                    .insert(uuid.clone());
            }
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
    Ok(added
        .into_iter()
        .map(|(owner, added)| Change { owner, added })
        .collect())
}

// ---------------------------------------------------------------------------
// The plan.

/// The select-list expression of a column of the new table: the old column of the same name,
/// or the default of a column the old table did not have.
fn select_expr(
    column: &Column,
    alias: &str,
    old: &HashMap<&str, &Column>,
    folder_marker: Option<&str>,
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
    default_expr(column, folder_marker, entry)
}

/// The type in a `CAST(... AS <type>)`.
fn cast_type(sql_type: &SqlType) -> Result<String> {
    Ok(match sql_type {
        SqlType::Binary(n) => format!("BINARY({n})"),
        SqlType::VarBinary(n) => format!("VARBINARY({n})"),
        SqlType::VarBinaryMax => "VARBINARY(MAX)".to_owned(),
        SqlType::Numeric(p, s) => format!("NUMERIC({p}, {s})"),
        SqlType::Int => "INT".to_owned(),
        SqlType::BigInt => "BIGINT".to_owned(),
        SqlType::DateTime2 => "DATETIME2(0)".to_owned(),
        SqlType::NVarChar(n) => format!("NVARCHAR({n})"),
        SqlType::NVarCharMax => "NVARCHAR(MAX)".to_owned(),
        SqlType::NChar(n) => format!("NCHAR({n})"),
        SqlType::Timestamp => bail!("a timestamp column has no default"),
    })
}

/// The value of a column the old table did not have (measured in cases a2, h and the types case).
/// `folder_marker` is the value of `_Folder` of the rows that get the default when the column is
/// nullable (`0x01` items for an attribute `ForItem`, `0x00` folders for one `ForFolder`); the other
/// rows get NULL.
fn default_expr(
    column: &Column,
    folder_marker: Option<&str>,
    entry: Option<&TypeEntry>,
) -> Result<String> {
    if column.nullable && folder_marker.is_none() {
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
    if let (true, Some(marker)) = (column.nullable, folder_marker) {
        // A field of a hierarchical catalog that only items (or only folders) have: the default for
        // them, NULL for the others (`_Folder = 0x01` marks an item, `0x00` a folder).
        return Ok(format!(
            "CAST(CASE WHEN T1._Folder = {marker} THEN {value} END AS {})",
            cast_type(&column.sql_type)?
        ));
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

const ROOT_CATALOGS: &str = "cf4abea6-37b2-11d4-940f-008048da11f9";
const ROOT_DOCUMENTS: &str = "061d872a-5787-460e-95ac-ed74ea3a3e84";

/// The position of every catalog and document in the configuration's own lists.
fn configuration_order(inputs: &Inputs) -> Result<HashMap<String, usize>> {
    let root = parse_row(&row_bytes(&inputs.root_row)).context("the root row")?;
    let configuration = root
        .as_list()
        .and_then(|items| items.get(1))
        .and_then(Brace::as_atom)
        .context("the root row names no configuration")?
        .to_ascii_lowercase();
    let descriptor = inputs
        .staged
        .old_descriptors
        .get(&configuration)
        .context("the configuration's own descriptor is not among the stored rows")?;
    let tree = parse_row(&row_bytes(descriptor)).context("the configuration's descriptor")?;
    let mut order = HashMap::new();
    collect_order(&tree, &mut order);
    Ok(order)
}

fn collect_order(node: &Brace, out: &mut HashMap<String, usize>) {
    let Some(members) = node.as_list() else {
        return;
    };
    if let (Some(class), Some(count)) = (
        members.first().and_then(Brace::as_atom),
        members.get(1).and_then(Brace::as_atom),
    ) && (class == ROOT_CATALOGS || class == ROOT_DOCUMENTS)
        && let Ok(count) = count.parse::<usize>()
        && members.len() == count + 2
        && members[2..].iter().all(|member| member.as_atom().is_some())
    {
        for (position, member) in members[2..].iter().enumerate() {
            if let Some(uuid) = member.as_atom() {
                out.insert(uuid.to_ascii_lowercase(), position);
            }
        }
        return;
    }
    for member in members {
        collect_order(member, out);
    }
}

/// One changed object, read.
struct Prepared {
    change: Change,
    old: ObjectFacts,
    new: ObjectFacts,
    /// The number of its main table (`Reference20` -> 20).
    number: u64,
}

/// The stored side: schema and names, and the counters the objects share.
struct Stored<'a> {
    schema: &'a DbSchema,
    main_names: &'a DbNames,
    predefined: &'a BTreeSet<String>,
}

/// What the objects share while they are planned one after another.
struct Running {
    /// The next number of the counter shared with the extensions.
    next: u64,
    names_after: DbNames,
}

/// Builds the plan.
pub fn plan(inputs: &Inputs, options: &PlanOptions) -> Result<Plan> {
    check_files(&inputs.staged)?;
    let changes = find_changes(&inputs.staged)?;

    let schema = DbSchema::parse(&inputs.schema).context("the stored DBSchema")?;
    let main_names = DbNames::parse_row(&inputs.main_names).context("the stored DBNames")?;
    let extension_names = inputs
        .extension_names
        .iter()
        .map(|(name, row)| DbNames::parse_row(row).with_context(|| format!("the stored {name}")))
        .collect::<Result<Vec<_>>>()?;

    let mut prepared = Vec::new();
    for change in changes {
        let read = |rows: &BTreeMap<String, Vec<u8>>, which: &str| -> Result<ObjectFacts> {
            let row = rows
                .get(&change.owner)
                .with_context(|| format!("the {which} image has no descriptor {}", change.owner))?;
            ObjectFacts::parse(&parse_row(&row_bytes(row))?).with_context(|| {
                format!(
                    "the {which} descriptor {} is not a catalog or a document this prototype reads",
                    change.owner
                )
            })
        };
        let old = read(&inputs.staged.old_descriptors, "stored")?;
        let new = read(&inputs.staged.new_descriptors, "staged")?;
        check_object(&old, &new, &change)?;
        let number = main_names
            .number_of(new.uuid(), new.kind().table_kind())
            .with_context(|| {
                format!("{} {} has no table number", new.kind().label(), new.name())
            })?;
        prepared.push(Prepared {
            change,
            old,
            new,
            number,
        });
    }
    // The platform walks the kinds in the configuration's order and the objects of a kind in the order
    // the configuration's descriptor lists them (traced: the types case; not by table number, not by name).
    if prepared.len() > 1 {
        let order = configuration_order(inputs)?;
        let mut keyed = Vec::new();
        for item in prepared {
            let position = *order.get(item.new.uuid()).with_context(|| {
                format!(
                    "the configuration does not list {} {}",
                    item.new.kind().label(),
                    item.new.name()
                )
            })?;
            keyed.push(((item.new.kind(), position), item));
        }
        keyed.sort_by_key(|(key, _)| *key);
        prepared = keyed.into_iter().map(|(_, item)| item).collect();
    }

    // Numbers come from the counter shared with the extensions.
    let mut counter_sources: Vec<&DbNames> = extension_names.iter().collect();
    counter_sources.push(&main_names);
    let mut running = Running {
        next: next_number(counter_sources.iter().copied()),
        names_after: main_names.clone(),
    };
    let stored = Stored {
        schema: &schema,
        main_names: &main_names,
        predefined: &inputs.predefined_tables,
    };

    let mut objects = Vec::new();
    let mut entries = Vec::new();
    for item in &prepared {
        let (object, entry) = plan_object(&stored, &mut running, item, options.method)?;
        entries.push((object.object.clone(), entry));
        objects.push(object);
    }

    // Publish: the schema with the entries moved before ConfigChngR (rebuilt tables sit at the end of the
    // list in the order they were rebuilt, ConfigChngR last), the names with the new numbers.
    let mut new_schema = schema.clone();
    for (object, entry) in entries {
        new_schema.remove(&object)?;
        new_schema.insert_before("ConfigChngR", entry);
    }
    let new_names_text = running.names_after.to_text();
    let new_names_row = deflate(&new_names_text)?;
    let version = options
        .names_version
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let mut caches = Vec::new();
    if !options.skip_xdto {
        caches.push(xdto_update(inputs, &prepared, &objects)?);
    }
    if !options.skip_registry {
        caches.push(registry_update(inputs, &prepared, &objects)?);
    }
    if !caches.is_empty() {
        let names: Vec<&str> = caches.iter().map(|cache| cache.row_name.as_str()).collect();
        caches.push(versions_update(inputs, &names)?);
    }
    Ok(Plan {
        objects,
        new_schema: new_schema.to_text(),
        new_names_text,
        new_names_row,
        names_version_row: version_row(&version),
        old_schema_sha256: sha256_hex(&inputs.schema),
        old_names_size: inputs.main_names.len(),
        old_names_sha256: sha256_hex(&inputs.main_names),
        method: options.method,
        caches,
    })
}

/// Plans one object: the fields, the tables and the copy.
fn plan_object(
    stored: &Stored<'_>,
    running: &mut Running,
    item: &Prepared,
    method: Method,
) -> Result<(ObjectPlan, Brace)> {
    let new_facts = &item.new;
    let kind = new_facts.kind();
    let object = format!("{}{}", kind.table_kind(), item.number);
    // The tables the object's other names stand for. The change-registration table of an exchange
    // plan (`ReferenceChngR`, `DocumentChngR`) is not rebuilt with the object (cases a2, b); the
    // predefined-data table (`RefSInf`) is empty for a catalog without predefined items and was not
    // rebuilt either, but for the catalog that has them (14 of 75) the platform rebuilt it with the
    // object (case h): that one is refused. Any other table of the object is refused.
    let change_registration = format!("{}ChngR", kind.table_kind());
    for entry in stored.main_names.entries_of(new_facts.uuid()) {
        if entry.kind == kind.table_kind() {
            continue;
        }
        let table = format!("{}{}", entry.kind, entry.number);
        if stored.schema.position(&table).is_none() {
            continue;
        }
        let harmless = if entry.kind == change_registration {
            true
        } else if entry.kind == "RefSInf" {
            !stored.predefined.contains(&table)
        } else {
            false
        };
        if !harmless {
            bail!(
                "{} {} has the companion table {table}: the platform rebuilds it with the object (predefined data?), not supported",
                kind.label(),
                new_facts.name()
            );
        }
    }
    let old_position = stored
        .schema
        .position(&object)
        .with_context(|| format!("the stored DBSchema has no table {object}"))?;
    let old_view = stored.schema.view(old_position)?;
    let old_fields = old_view.fields()?;
    check_stored_fields(&item.old, &old_view, &old_fields, stored.main_names)?;
    let has_folder = old_fields.iter().any(|field| field.name == "Folder");

    let mut additions = Vec::new();
    let mut fields_now = old_fields.clone();
    // Metadata order decides both the numbers and the places.
    let attributes = new_facts.attributes();
    for (index, attribute) in attributes.iter().enumerate() {
        if !item.change.added.contains(&attribute.uuid) {
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
        let nullable = new_facts.nullable(attribute, has_folder);
        let field = FieldEntry::new(&format!("Fld{}", running.next), nullable, entries);
        // After the field of the attribute before it; the first attribute goes after the
        // standard fields.
        let position = match index.checked_sub(1).map(|before| &attributes[before]) {
            Some(previous) => {
                let previous_number = running
                    .names_after
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
            None => fields_now
                .iter()
                .rposition(|candidate| kind.standard_fields().contains(&candidate.name.as_str()))
                .map_or(0, |last| last + 1),
        };
        fields_now.insert(position, field.clone());
        running
            .names_after
            .append(&attribute.uuid, "Fld", running.next)?;
        additions.push(Addition {
            uuid: attribute.uuid.clone(),
            name: attribute.name.clone(),
            number: running.next,
            field,
            position,
            usage: attribute.usage,
        });
        running.next += 1;
    }
    if additions.len() != item.change.added.len() {
        bail!(
            "some new attributes are not among the own attributes of {} {}",
            kind.label(),
            new_facts.name()
        );
    }

    // The new table entry: the stored one with the fields inserted.
    let mut entry = stored.schema.tables()[old_position].clone();
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
            // The new field this column belongs to (`_Fld1`, `_Fld1_S`, ...): its first type entry
            // and the folder marker of a nullable one.
            let new_field = additions.iter().find(|addition| {
                let base = format!("_{}", addition.field.name);
                column.name == base || column.name.starts_with(&format!("{base}_"))
            });
            insert_columns.push(column.name.clone());
            insert_values.push(select_expr(
                column,
                &alias,
                &old_columns,
                new_field.and_then(|addition| addition.folder_marker()),
                new_field.and_then(|addition| addition.field.types.first()),
            )?);
        }
        tables.push(TablePlan {
            old_columns: before.columns.clone(),
            table: after,
            insert_columns,
            insert_values,
        });
    }

    let mut alter = Vec::new();
    if method == Method::AlterAdd {
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
                value: default_expr(&plain, None, addition.field.types.first())?,
                marker: addition.folder_marker().map(str::to_owned),
                column: column.clone(),
            });
        }
    }
    Ok((
        ObjectPlan {
            kind,
            object,
            object_uuid: new_facts.uuid().to_owned(),
            object_name: new_facts.name().to_owned(),
            additions,
            tables,
            alter,
        },
        entry,
    ))
}

/// The cache row of the XDTO model: a property for each new attribute, after the property of the
/// attribute before it.
fn xdto_update(
    inputs: &Inputs,
    prepared: &[Prepared],
    objects: &[ObjectPlan],
) -> Result<CacheUpdate> {
    let first = prepared
        .first()
        .context("no object to update the XDTO model for")?;
    let needle = format!("{}.{}", first.new.kind().xdto_object(), first.new.name());
    for (name, stored) in &inputs.cache_rows {
        let text = inflate(stored).unwrap_or_default();
        if !xdto::is_model(&text) {
            continue;
        }
        let mut model = xdto::Model::open(&text)?;
        if !model.has_object_type(&needle) {
            continue;
        }
        let mut count = 0usize;
        for (item, object) in prepared.iter().zip(objects) {
            let kind = item.new.kind();
            let type_name = format!("{}.{}", kind.xdto_object(), item.new.name());
            let attributes = item.new.attributes();
            for addition in &object.additions {
                let index = attributes
                    .iter()
                    .position(|attribute| attribute.uuid == addition.uuid)
                    .context("a new attribute is not among the object's attributes")?;
                let entry = addition
                    .field
                    .types
                    .first()
                    .context("a new field has no type")?;
                let line = xdto::property_line(entry, &addition.name, addition.field.nullable)?;
                let after = index
                    .checked_sub(1)
                    .map(|before| attributes[before].name.as_str());
                model.add_property(
                    &type_name,
                    after,
                    kind.xdto_standard(),
                    &addition.name,
                    &line,
                )?;
                count += 1;
            }
        }
        return Ok(CacheUpdate {
            row_name: name.clone(),
            row: deflate(&model.to_text())?,
            what: format!("{count} property line(s) in the XDTO model"),
            set_creation: true,
        });
    }
    bail!("no XDTO model cache row knows {needle}: pass --skip-xdto to leave the caches alone")
}

/// The cache row of the object registry: a record for each new attribute.
fn registry_update(
    inputs: &Inputs,
    prepared: &[Prepared],
    objects: &[ObjectPlan],
) -> Result<CacheUpdate> {
    let (name, stored) = inputs
        .cache_rows
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(registry::REGISTRY_ROW))
        .with_context(|| {
            format!(
                "Params has no row {}: pass --skip-registry to leave the caches alone",
                registry::REGISTRY_ROW
            )
        })?;
    let text = inflate(stored).context("the object registry row is not raw deflate")?;
    let added: Vec<BTreeSet<String>> = objects
        .iter()
        .map(|object| {
            object
                .additions
                .iter()
                .map(|addition| addition.uuid.clone())
                .collect()
        })
        .collect();
    let additions: Vec<ObjectAdditions<'_>> = prepared
        .iter()
        .zip(&added)
        .map(|(item, added)| ObjectAdditions {
            kind: item.new.kind(),
            owner: item.new.uuid(),
            attributes: item.new.attributes(),
            added,
        })
        .collect();
    let updated = registry::add_attributes(&text, &additions)?;
    let count: usize = added.iter().map(BTreeSet::len).sum();
    Ok(CacheUpdate {
        row_name: name.clone(),
        row: deflate(&updated)?,
        what: format!("{count} record(s) of attributes in the object registry"),
        set_creation: true,
    })
}

/// `siVersions` with a new version for each cache row rewritten.
fn versions_update(inputs: &Inputs, rows: &[&str]) -> Result<CacheUpdate> {
    let (name, stored) = inputs
        .cache_rows
        .iter()
        .find(|(name, _)| name == registry::VERSIONS_ROW)
        .with_context(|| format!("Params has no row {}", registry::VERSIONS_ROW))?;
    let text = stored
        .strip_prefix(&[0xEF, 0xBB, 0xBF])
        .unwrap_or(stored.as_slice());
    let updated = registry::bump_versions(text, rows)?;
    let mut row = Vec::new();
    if stored.starts_with(&[0xEF, 0xBB, 0xBF]) {
        row.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    }
    row.extend_from_slice(&updated);
    Ok(CacheUpdate {
        row_name: name.clone(),
        row,
        what: format!("new versions of {} cache row(s)", rows.len()),
        set_creation: false,
    })
}

/// The object is the same as far as the table goes, except for the appended attributes.
fn check_object(old: &ObjectFacts, new: &ObjectFacts, change: &Change) -> Result<()> {
    if old.kind() != new.kind() || old.uuid() != new.uuid() || old.uuid() != change.owner {
        bail!("the changed descriptor is not the object the attributes sit in");
    }
    let (before, after) = (old.shape(), new.shape());
    if before != after {
        let names: Vec<&str> = before
            .iter()
            .zip(&after)
            .filter(|(a, b)| a != b)
            .map(|(a, _)| a.0)
            .collect();
        bail!(
            "{} {} changes {}: not supported",
            new.kind().label(),
            new.name(),
            names.join(", ")
        );
    }
    old.check_supported()?;
    let (old_sections, new_sections) = (old.sections(), new.sections());
    let same_sections = old_sections.len() == new_sections.len()
        && old_sections
            .iter()
            .zip(new_sections)
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
            "{} {} changes a tabular section: not supported",
            new.kind().label(),
            new.name()
        );
    }
    // The stored attributes are the new ones in the same order, with the same type and indexing.
    let kept: Vec<&AttributeFacts> = new
        .attributes()
        .iter()
        .filter(|attribute| !change.added.contains(&attribute.uuid))
        .collect();
    let same_attributes = kept.len() == old.attributes().len()
        && kept.iter().zip(old.attributes()).all(|(after, before)| {
            after.uuid == before.uuid
                && after.pattern == before.pattern
                && after.indexing == before.indexing
                && after.usage == before.usage
        });
    if !same_attributes {
        bail!(
            "{} {} reorders or changes its stored attributes: not supported",
            new.kind().label(),
            new.name()
        );
    }
    Ok(())
}

/// Every stored attribute has its field in the stored table, and the fields the generator can
/// compute agree with the stored ones.
fn check_stored_fields(
    facts: &ObjectFacts,
    table: &TableView<'_>,
    fields: &[FieldEntry],
    names: &DbNames,
) -> Result<()> {
    let has_folder = fields.iter().any(|field| field.name == "Folder");
    let mut last = None;
    for attribute in facts.attributes() {
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
            let expected_nullable = facts.nullable(attribute, has_folder);
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
    /// The rows the statement must change (a `Params` update touches exactly one).
    pub expect_rows: Option<u64>,
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
        let mut push = |phase: Phase,
                        label: String,
                        sql: String,
                        params: Vec<StatementParam>,
                        expect_rows: Option<u64>| {
            out.push(Statement {
                phase,
                label,
                sql,
                params,
                expect_rows,
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
            None,
        );
        if self.method == Method::AlterAdd {
            for object in &self.objects {
                for added in &object.alter {
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
                            None,
                        );
                        push(
                            Phase::Create,
                            format!("set {} of the existing rows", column.name),
                            format!(
                                "UPDATE dbo.{} SET {} = {} WHERE _Folder = {};",
                                added.table,
                                column.name,
                                added.value,
                                added.marker.as_deref().unwrap_or("0x01")
                            ),
                            Vec::new(),
                            None,
                        );
                    } else {
                        push(
                            Phase::Create,
                            format!("alter {} add {}", added.table, column.name),
                            format!(
                                "ALTER TABLE dbo.{table} ADD {name} {ty} NOT NULL CONSTRAINT DF_ddl_tmp DEFAULT {value};\nALTER TABLE dbo.{table} DROP CONSTRAINT DF_ddl_tmp;",
                                table = added.table,
                                name = column.name,
                                ty = column.sql_type.ddl(),
                                value = added.value
                            ),
                            Vec::new(),
                            None,
                        );
                    }
                }
            }
        } else {
            for table in self.tables() {
                push(
                    Phase::Create,
                    format!("create {}NG", table.table.name),
                    create_table_sql(&table.table, "NG"),
                    Vec::new(),
                    None,
                );
            }
            for object in &self.objects {
                for (index, table) in object.tables.iter().enumerate() {
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
                        None,
                    );
                }
            }
            for table in self.tables() {
                for index in &table.table.indexes {
                    push(
                        Phase::Indexes,
                        format!("index {}NG", index.name),
                        create_index_sql(&table.table.name, index, "NG"),
                        Vec::new(),
                        None,
                    );
                }
            }
            for table in self.tables() {
                push(
                    Phase::DropOld,
                    format!("drop {}", table.table.name),
                    format!("drop table dbo.{};", table.table.name),
                    Vec::new(),
                    None,
                );
            }
            for table in self.tables() {
                push(
                    Phase::Rename,
                    format!("rename {}NG", table.table.name),
                    format!(
                        "EXEC sp_rename N'{}', N'{}', 'OBJECT';",
                        Self::ng(&table.table.name),
                        table.table.name
                    ),
                    Vec::new(),
                    None,
                );
            }
            for table in self.tables() {
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
                        None,
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
            Some(1),
        );
        push(
            Phase::Publish,
            "DBSchema".to_owned(),
            "UPDATE dbo.DBSchema SET SerializedData = @P1;".to_owned(),
            vec![StatementParam::Bytes(self.new_schema.clone())],
            Some(1),
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
                Some(1),
            );
        }
        for cache in &self.caches {
            let creation = if cache.set_creation {
                "Creation = DATEADD(YEAR, 2000, SYSDATETIME()), "
            } else {
                ""
            };
            push(
                Phase::Publish,
                format!("Params {}: {}", cache.row_name, cache.what),
                format!(
                    "UPDATE dbo.Params SET BinaryData = @P1, DataSize = @P2, {creation}Modified = DATEADD(YEAR, 2000, SYSDATETIME()) WHERE FileName = N'{}' AND PartNo = 0;",
                    cache.row_name
                ),
                vec![
                    StatementParam::Bytes(cache.row.clone()),
                    StatementParam::Int(cache.row.len() as i64),
                ],
                Some(1),
            );
        }
        push(
            Phase::Publish,
            "Config: the changed staged rows replace the stored ones".to_owned(),
            PROMOTE_CONFIG.to_owned(),
            Vec::new(),
            None,
        );
        push(
            Phase::Publish,
            "ConfigSave: emptied".to_owned(),
            "DELETE FROM dbo.ConfigSave;".to_owned(),
            Vec::new(),
            None,
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
        self.objects
            .iter()
            .map(|object| {
                format!(
                    "{} {} ({}): {} new attribute(s) -> {}",
                    object.kind.label(),
                    object.object_name,
                    object.object,
                    object.additions.len(),
                    object
                        .additions
                        .iter()
                        .map(|addition| format!("{} = {}", addition.field.name, addition.name))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    }
}
