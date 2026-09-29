//! Rows of a configuration *extension*: what they add to the ordinary
//! configuration row format, and how the platform prints it.
//!
//! An extension stores every adopted (borrowed) object in the ordinary
//! descriptor layout of its kind, but the object's md header carries the
//! adoption state after the comment:
//!
//! ```text
//! {3,{1,0,<uuid>},"Name",<synonym>,"Comment",
//!  <belonging>,<N>,(<property guid>,<state>) x N,<extended object uuid>,
//!  <M>,(<property guid>,<state>,<value>) x M}
//! ```
//!
//! An object of the extension itself, like every object of an ordinary
//! configuration, carries `0,0,<nil uuid>,0` there. `belonging` 1 is an adopted
//! object; the `N` pairs name the properties the extension can override (the
//! platform prints exactly those, beside `Name` and `Comment`), `state` 2 is
//! a property that only records its value and 3 one the extension overrides;
//! the extended object uuid is the base configuration's object when the
//! mapping is not by identity; the `M` triples carry the values of the
//! properties the extension adds to (`MultiState`, type lists).
//!
//! The export works in two steps that keep every ordinary converter blind to
//! extensions: [`normalize_descriptor`] rewrites the adopted tail to the
//! ordinary one and remembers it, the ordinary converters print the object,
//! and [`project_object_xml`] then reduces that print to what the platform
//! writes for an adopted object.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use anyhow::{Context, Result, anyhow, bail};
use ibcmd_cf::export::{
    StorageExportDisposition, StorageExportEntryReport, StorageExportPlan, StorageExportReport,
};
use ibcmd_core::storage::StorageImage;
use sha1::{Digest, Sha1};

use super::{
    BinaryConfigRow, DirectStorageExportRecord, StorageImageSourceExportReport,
    config_row_from_binary, export_direct_storage_rows_to_source, inflate_raw_deflate,
};
use crate::cli::InfobaseConfigSourceVersion;

mod project;
mod properties;
pub(super) mod root;

pub(crate) use project::project_object_xml;

pub(crate) const NIL_UUID: &str = "00000000-0000-0000-0000-000000000000";

/// The white space a stored row puts between the elements of a list.
const WHITESPACE: [char; 4] = ['\r', '\n', '\t', ' '];

/// `<xr:State>` values of the stored state codes.
const STATE_RECORDED: u8 = 2;
const STATE_EXTENDED: u8 = 3;

/// Property id every adopted object lists when the platform prints
/// `ExtendedConfigurationObject` for it.
pub(crate) const EXTENDED_OBJECT_PROPERTY: &str = "9595ddd6-e72c-47ad-a156-672db811628c";

/// The adoption state of one md header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AdoptedHeader {
    /// The object's own uuid (the header's `{1,0,<uuid>}`).
    pub uuid: String,
    /// `(property guid, state)` in stored order.
    pub properties: Vec<(String, u8)>,
    /// The base object the adopted one is mapped to, when it is not the same
    /// uuid.
    pub extended_object: Option<String>,
    /// `(property guid, state, value as stored)` of the added values.
    pub added_values: Vec<(String, u8, String)>,
}

/// A descriptor row with its adoption headers normalized.
#[derive(Debug)]
pub(crate) struct NormalizedDescriptor {
    pub text: String,
    pub adopted: Vec<AdoptedHeader>,
}

/// The two spellings of an md header the platform has used: the `{3,...}` block
/// of every kind since 8.3.24 and the older `{1,...}` block (a language of a
/// 8.3.21 extension). Only the identity tuple and the length of an ordinary
/// tail differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HeaderSpelling {
    /// `{3,{1,0,<uuid>},name,synonym,comment,<tail of 4>}`.
    Current,
    /// `{1,{0,0,<uuid>},name,synonym,comment,<tail of 2>}`.
    Older,
}

impl HeaderSpelling {
    fn ordinary_tail(self) -> String {
        match self {
            Self::Current => format!("0,0,{NIL_UUID},0"),
            Self::Older => "0,0".to_owned(),
        }
    }

    fn opening(self) -> &'static str {
        match self {
            Self::Current => "{3,",
            Self::Older => "{1,",
        }
    }
}

/// The next `{1,0,<uuid>}` (preceded by `{3,`) or `{0,0,<uuid>}` (preceded by
/// `{1,`) at or after `from`: the index of its `{`, the spelling.
fn next_identity(text: &str, from: usize) -> Option<(usize, HeaderSpelling)> {
    let mut from = from;
    loop {
        let current = text[from..].find("{1,0,").map(|found| from + found);
        let older = text[from..].find("{0,0,").map(|found| from + found);
        let (start, spelling) = match (current, older) {
            (None, None) => return None,
            (Some(current), None) => (current, HeaderSpelling::Current),
            (None, Some(older)) => (older, HeaderSpelling::Older),
            (Some(current), Some(older)) if current <= older => (current, HeaderSpelling::Current),
            (Some(_), Some(older)) => (older, HeaderSpelling::Older),
        };
        let uuid_start = start + 5;
        let bytes = text.as_bytes();
        let shaped = bytes.get(uuid_start + 36) == Some(&b'}')
            && is_uuid(&text[uuid_start..uuid_start + 36])
            && text[..start]
                .trim_end_matches(WHITESPACE)
                .ends_with(spelling.opening());
        if shaped {
            return Some((start, spelling));
        }
        from = start + 5;
    }
}

/// Every adopted header of `text` rewritten to the ordinary tail of its
/// spelling. Text without an adopted header comes back unchanged.
pub(crate) fn normalize_descriptor(text: &str) -> Result<NormalizedDescriptor> {
    let mut adopted = Vec::new();
    let mut output = String::with_capacity(text.len());
    let mut copied = 0usize;
    let mut search = 0usize;
    while let Some((identity_start, spelling)) = next_identity(text, search) {
        search = identity_start + 5;
        let uuid = &text[identity_start + 5..identity_start + 41];
        let before = text[..identity_start].trim_end_matches(WHITESPACE);
        let open = before.len() - 3;
        let Some(header_end) = scan_braced_end(text, open) else {
            continue;
        };
        let Some(tail_start) = header_tail_start(text, open, header_end) else {
            continue;
        };
        if tail_start < copied {
            continue;
        }
        let tail = &text[tail_start..header_end - 1];
        let fields = split_top_level_fields(tail)?;
        if fields.first().map(|field| field.trim()) == Some("0") {
            // An ordinary header; nothing to remember.
            search = header_end.max(search);
            continue;
        }
        let header = parse_adopted_tail(uuid, &fields, spelling == HeaderSpelling::Older)?;
        output.push_str(&text[copied..tail_start]);
        output.push_str(&spelling.ordinary_tail());
        copied = header_end - 1;
        adopted.push(header);
        search = search.max(header_end);
    }
    if adopted.is_empty() {
        return Ok(NormalizedDescriptor {
            text: text.to_owned(),
            adopted,
        });
    }
    output.push_str(&text[copied..]);
    Ok(NormalizedDescriptor {
        text: output,
        adopted,
    })
}

/// `short_tail`: the older spelling ends after the property entries.
fn parse_adopted_tail(uuid: &str, fields: &[&str], short_tail: bool) -> Result<AdoptedHeader> {
    let field = |index: usize| -> Result<&str> {
        fields
            .get(index)
            .map(|field| field.trim())
            .ok_or_else(|| anyhow!("adopted header {uuid} ends before field {index}"))
    };
    if field(0)? != "1" {
        bail!(
            "header {uuid} has object belonging {:?}; only 0 and 1 are known",
            field(0)?
        );
    }
    let count: usize = field(1)?.parse().map_err(|_| {
        anyhow!(
            "adopted header {uuid} has a bad property count {:?}",
            field(1)
        )
    })?;
    let mut properties = Vec::with_capacity(count);
    let mut index = 2usize;
    for _ in 0..count {
        let guid = field(index)?;
        let state: u8 = field(index + 1)?.parse().map_err(|_| {
            anyhow!(
                "adopted header {uuid} has a bad state {:?} for {guid}",
                field(index + 1)
            )
        })?;
        if !is_uuid(guid) || !matches!(state, STATE_RECORDED | STATE_EXTENDED) {
            bail!("adopted header {uuid} has an unknown property entry {guid},{state}");
        }
        properties.push((guid.to_ascii_lowercase(), state));
        index += 2;
    }
    if short_tail && index == fields.len() {
        return Ok(AdoptedHeader {
            uuid: uuid.to_ascii_lowercase(),
            properties,
            extended_object: None,
            added_values: Vec::new(),
        });
    }
    let extended = field(index)?;
    if !is_uuid(extended) {
        bail!("adopted header {uuid} has a bad extended object {extended:?}");
    }
    index += 1;
    let extended_object = (extended != NIL_UUID).then(|| extended.to_ascii_lowercase());
    let added: usize = field(index)?
        .parse()
        .map_err(|_| anyhow!("adopted header {uuid} has a bad added-value count"))?;
    index += 1;
    let mut added_values = Vec::with_capacity(added);
    for _ in 0..added {
        let guid = field(index)?;
        let state: u8 = field(index + 1)?
            .parse()
            .map_err(|_| anyhow!("adopted header {uuid} has a bad added-value state"))?;
        let value = fields
            .get(index + 2)
            .ok_or_else(|| anyhow!("adopted header {uuid} ends inside an added value"))?
            .trim();
        added_values.push((guid.to_ascii_lowercase(), state, value.to_owned()));
        index += 3;
    }
    if index != fields.len() {
        bail!(
            "adopted header {uuid} has {} trailing fields",
            fields.len() - index
        );
    }
    Ok(AdoptedHeader {
        uuid: uuid.to_ascii_lowercase(),
        properties,
        extended_object,
        added_values,
    })
}

fn is_uuid(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => *byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}

/// The index just past the `}` that closes the braced value opened at `open`.
fn scan_braced_end(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.get(open) != Some(&b'{') {
        return None;
    }
    let mut depth = 0usize;
    let mut index = open;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => index = skip_string(bytes, index + 1)?,
            b'{' => {
                depth += 1;
                index += 1;
            }
            b'}' => {
                depth = depth.checked_sub(1)?;
                index += 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => index += 1,
        }
    }
    None
}

/// The index just past the quote that closes a string whose body starts at
/// `from` (`""` is an escaped quote).
fn skip_string(bytes: &[u8], mut from: usize) -> Option<usize> {
    loop {
        let quote = from + bytes.get(from..)?.iter().position(|byte| *byte == b'"')?;
        if bytes.get(quote + 1) == Some(&b'"') {
            from = quote + 2;
        } else {
            return Some(quote + 1);
        }
    }
}

/// Where the tail of the md header at `open..end` starts: after the comment
/// and its comma, i.e. after the fifth top-level field.
fn header_tail_start(text: &str, open: usize, end: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut index = open + 1;
    for _ in 0..5 {
        while index < end && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        match *bytes.get(index)? {
            b'{' => index = scan_braced_end(text, index)?,
            b'"' => index = skip_string(bytes, index + 1)?,
            _ => {
                while index < end && !matches!(bytes[index], b',' | b'}') {
                    index += 1;
                }
            }
        }
        while index < end && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if bytes.get(index) != Some(&b',') {
            return None;
        }
        index += 1;
    }
    (index < end).then_some(index)
}

/// The top-level, comma separated fields of `tail` (no surrounding braces),
/// untrimmed.
fn split_top_level_fields(tail: &str) -> Result<Vec<&str>> {
    let bytes = tail.as_bytes();
    let mut fields = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                index = skip_string(bytes, index + 1)
                    .ok_or_else(|| anyhow!("unterminated string in an md header tail"))?;
                continue;
            }
            b'{' => depth += 1,
            b'}' => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| anyhow!("unbalanced braces in an md header tail"))?
            }
            b',' if depth == 0 => {
                fields.push(&tail[start..index]);
                start = index + 1;
            }
            _ => {}
        }
        index += 1;
    }
    fields.push(&tail[start..]);
    Ok(fields)
}

/// What an export of one extension knows about its adopted objects.
#[derive(Debug, Default)]
pub(crate) struct ExtensionContext {
    adopted: BTreeMap<String, AdoptedHeader>,
    /// SHA-1 of every storage row's packed bytes, by row name: the digests the
    /// extension's CAS manifest holds and `ConfigDumpInfo.xml` prints.
    packed_sha1: Option<BTreeMap<String, [u8; 20]>>,
    /// The uuid of the md header of the extension's root object: the owner
    /// of the root's module and command-interface rows.
    root_header: Option<String>,
    /// Why the ordinary converters printed nothing for a row, by row name.
    diagnostics: Mutex<BTreeMap<String, String>>,
    /// The references the export resolved: type id -> `cfg:` name, and
    /// object id -> metadata reference. The projection prints values that
    /// were stored as ids with them.
    indexes: OnceLock<ResolvedIndexes>,
}

/// See [`ExtensionContext::note_indexes`].
#[derive(Debug, Default)]
pub(crate) struct ResolvedIndexes {
    pub type_index: BTreeMap<String, String>,
    pub object_refs: BTreeMap<String, String>,
}

impl ExtensionContext {
    pub(crate) fn new(adopted: impl IntoIterator<Item = AdoptedHeader>) -> Self {
        Self {
            adopted: adopted
                .into_iter()
                .map(|header| (header.uuid.clone(), header))
                .collect(),
            packed_sha1: None,
            root_header: None,
            diagnostics: Mutex::default(),
            indexes: OnceLock::new(),
        }
    }

    /// Remembers the reference indexes of the export (once).
    pub(crate) fn note_indexes(
        &self,
        type_index: &BTreeMap<String, String>,
        object_refs: &BTreeMap<String, String>,
    ) {
        let _ = self.indexes.set(ResolvedIndexes {
            type_index: type_index.clone(),
            object_refs: object_refs.clone(),
        });
    }

    pub(crate) fn indexes(&self) -> Option<&ResolvedIndexes> {
        self.indexes.get()
    }

    /// Remembers why the converters printed nothing for `row`.
    pub(crate) fn note_diagnostic(&self, row: &str, text: String) {
        if let Ok(mut diagnostics) = self.diagnostics.lock() {
            diagnostics.entry(row.to_owned()).or_insert(text);
        }
    }

    fn diagnostic(&self, row: &str) -> Option<String> {
        self.diagnostics.lock().ok()?.get(row).cloned()
    }

    pub(crate) fn with_packed_sha1(mut self, packed_sha1: BTreeMap<String, [u8; 20]>) -> Self {
        self.packed_sha1 = Some(packed_sha1);
        self
    }

    pub(crate) fn with_root_header(mut self, root_header: Option<String>) -> Self {
        self.root_header = root_header;
        self
    }

    /// Whether `owner` is the md header uuid of the extension's root object.
    pub(crate) fn is_root_header(&self, owner: &str) -> bool {
        self.root_header
            .as_deref()
            .is_some_and(|root| root.eq_ignore_ascii_case(owner))
    }

    pub(crate) fn packed_sha1(&self) -> Option<&BTreeMap<String, [u8; 20]>> {
        self.packed_sha1.as_ref()
    }

    pub(crate) fn adopted(&self, uuid: &str) -> Option<&AdoptedHeader> {
        self.adopted.get(&uuid.to_ascii_lowercase())
    }

    pub(crate) fn len(&self) -> usize {
        self.adopted.len()
    }
}

static ACTIVE: RwLock<Option<Arc<ExtensionContext>>> = RwLock::new(None);

/// Keeps a context active for the process; the export is ordinary again once
/// it is dropped.
pub(crate) struct ExtensionGuard {
    _private: (),
}

impl Drop for ExtensionGuard {
    fn drop(&mut self) {
        if let Ok(mut active) = ACTIVE.write() {
            *active = None;
        }
    }
}

pub(crate) fn activate(context: ExtensionContext) -> Result<ExtensionGuard> {
    let mut active = ACTIVE
        .write()
        .map_err(|_| anyhow!("the extension export context is poisoned"))?;
    if active.is_some() {
        bail!("an extension export is already running in this process");
    }
    *active = Some(Arc::new(context));
    Ok(ExtensionGuard { _private: () })
}

/// The context of the extension export running now, if any.
pub(crate) fn active() -> Option<Arc<ExtensionContext>> {
    ACTIVE.read().ok()?.clone()
}

/// Exports one configuration extension's storage image to a source tree.
///
/// The rows go through the ordinary family decoders with their adoption
/// headers normalized ([`normalize_descriptor`]); the objects they print are
/// then reduced to what the platform writes for an adopted object
/// ([`project_object_xml`]) and `ConfigDumpInfo.xml` is written from the CAS
/// digests.
pub fn export_extension_image_to_source(
    image: &StorageImage,
    output_dir: &Path,
    source_version: InfobaseConfigSourceVersion,
) -> Result<StorageImageSourceExportReport> {
    let plan = StorageExportPlan::from_image(image);
    let mut rows = Vec::with_capacity(plan.records().len());
    let mut records = Vec::with_capacity(plan.records().len());
    let mut adopted = Vec::new();
    let mut adopted_rows = BTreeMap::<String, usize>::new();
    let mut packed_sha1 = BTreeMap::new();
    let mut refused = Vec::<StorageExportEntryReport>::new();
    let mut root_header = None::<String>;
    for record in plan.records() {
        let payload = record.packed_payload().with_context(|| {
            format!(
                "failed to materialize storage record `{}`",
                record.logical_key()
            )
        })?;
        packed_sha1.insert(
            record.logical_name().to_owned(),
            <[u8; 20]>::from(Sha1::digest(&payload)),
        );
        let packed_bytes = payload.len();
        let mut bytes = payload.into_owned();
        if !record.logical_name().contains('.') {
            match normalize_row(&bytes) {
                Ok(Some(normalized)) => {
                    adopted_rows.insert(record.logical_name().to_owned(), normalized.adopted.len());
                    if normalized.is_root {
                        root_header = normalized.adopted.first().map(|header| header.uuid.clone());
                    }
                    adopted.extend(normalized.adopted);
                    bytes = normalized.packed;
                }
                Ok(None) => {}
                Err(error) => {
                    refused.push(StorageExportEntryReport::failed_packed(
                        record.logical_name(),
                        record.logical_key(),
                        record.part_count(),
                        packed_bytes,
                        format!("{error:#}"),
                    ));
                    continue;
                }
            }
        }
        let data_size = i64::try_from(bytes.len()).with_context(|| {
            format!(
                "storage record `{}` is too large for the row boundary",
                record.logical_key()
            )
        })?;
        rows.push(config_row_from_binary(BinaryConfigRow {
            file_name: record.logical_name().to_owned(),
            part_no: 0,
            data_size,
            binary: bytes,
        }));
        records.push(DirectStorageExportRecord {
            logical_name: record.logical_name().to_owned(),
            logical_key: record.logical_key().to_owned(),
            part_count: record.part_count(),
            packed_bytes,
        });
    }
    // A refused row is neither exported nor versioned.
    for entry in &refused {
        packed_sha1.remove(&entry.logical_name);
    }
    write_lab_normalized_rows(&rows)?;

    let guard = activate(
        ExtensionContext::new(adopted)
            .with_packed_sha1(packed_sha1)
            .with_root_header(root_header),
    )?;
    let exported = export_direct_storage_rows_to_source(
        rows,
        records,
        image
            .source_profile()
            .map(|profile| profile.as_str().to_owned()),
        plan.physical_entries(),
        output_dir,
        false,
        source_version,
    );
    let context = active();
    drop(guard);
    let mut report = exported?;
    let context = context.ok_or_else(|| anyhow!("the extension export context vanished"))?;
    let mut entries = std::mem::take(&mut report.storage.entries);
    for entry in &mut entries {
        if entry.disposition == StorageExportDisposition::Opaque
            && let Some(reason) = context.diagnostic(&entry.logical_name)
        {
            entry.message = Some(format!(
                "no legacy family decoder recognized this storage entry: {reason}"
            ));
        }
        if !adopted_rows.contains_key(&entry.logical_name)
            || entry.disposition != StorageExportDisposition::Supported
        {
            continue;
        }
        if let Err(error) = project_outputs(output_dir, entry, &context) {
            entry.disposition = StorageExportDisposition::Failed;
            entry.message = Some(format!("{error:#}"));
            for output in std::mem::take(&mut entry.outputs) {
                // A file the platform would print differently is not left behind.
                let _ = std::fs::remove_file(output_dir.join(output));
            }
        }
    }
    entries.extend(refused);
    report.storage = StorageExportReport::from_parts(
        report.storage.source_profile.clone(),
        report.storage.physical_entries,
        plan.records().len(),
        entries,
    );
    Ok(report)
}

/// Lab aid: `IBCMD_RS_EXTENSION_NORMALIZED_ROWS_OUT=<dir>` writes the rows the
/// converters receive (adoption headers normalized) in the `--rows-dir` layout.
fn write_lab_normalized_rows(rows: &[super::ConfigRow]) -> Result<()> {
    let Some(dir) = std::env::var_os("IBCMD_RS_EXTENSION_NORMALIZED_ROWS_OUT")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
    else {
        return Ok(());
    };
    std::fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    for row in rows {
        let bytes = row.binary_bytes()?;
        let path = dir.join(format!("{}__part0.bin", row.file_name));
        std::fs::write(&path, bytes.as_ref())
            .with_context(|| format!("failed to write {}", path.display()))?;
    }
    Ok(())
}

/// A one-line account of a converter diagnostic (tokens only, no payload).
pub(crate) fn describe_diagnostic(
    diagnostic: &super::MetadataSourceExtractionDiagnostic,
) -> String {
    let mut text = format!(
        "{} (family {}, stage {}, signature {}",
        diagnostic.code,
        diagnostic.family,
        diagnostic.parser_stage,
        diagnostic.structural_signature
    );
    for (name, value) in [
        ("field", diagnostic.field_index),
        ("collection", diagnostic.collection_index),
        ("item", diagnostic.item_index),
    ] {
        if let Some(value) = value {
            text.push_str(&format!(", {name} {value}"));
        }
    }
    if let Some(role) = &diagnostic.collection_role {
        text.push_str(&format!(", role {role}"));
    }
    text.push(')');
    text
}

/// The normalized packed row of a descriptor that holds an adopted header.
struct NormalizedRow {
    packed: Vec<u8>,
    adopted: Vec<AdoptedHeader>,
    /// The row is the root `Configuration` record (`{2,...}`).
    is_root: bool,
}

fn normalize_row(packed: &[u8]) -> Result<Option<NormalizedRow>> {
    let Ok(plain) = inflate_raw_deflate(packed) else {
        return Ok(None);
    };
    let Ok(text) = String::from_utf8(plain) else {
        return Ok(None);
    };
    let (bom, body) = match text.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", text.as_str()),
    };
    let normalized = normalize_descriptor(body)?;
    if normalized.adopted.is_empty() {
        return Ok(None);
    }
    let is_root = normalized.text.trim_start().starts_with("{2,");
    let packed = crate::module_blob::deflate_raw(format!("{bom}{}", normalized.text).as_bytes())?;
    Ok(Some(NormalizedRow {
        packed,
        adopted: normalized.adopted,
        is_root,
    }))
}

/// Rewrites the object XML files one storage row produced.
fn project_outputs(
    output_dir: &Path,
    entry: &StorageExportEntryReport,
    context: &ExtensionContext,
) -> Result<()> {
    for output in &entry.outputs {
        if !output.ends_with(".xml") {
            continue;
        }
        let path = output_dir.join(output);
        let bytes = std::fs::read(&path).with_context(|| format!("failed to read {output}"))?;
        let text = String::from_utf8(bytes).with_context(|| format!("{output} is not UTF-8"))?;
        if !text.contains("<MetaDataObject") {
            continue;
        }
        let projected = project_object_xml(&text, context)
            .with_context(|| format!("failed to project {output}"))?;
        if projected != text {
            std::fs::write(&path, projected.as_bytes())
                .with_context(|| format!("failed to write {output}"))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADOPTED_MODULE: &str = "{1,\r\n{12,\r\n{3,\r\n{1,0,eb50ccde-ac43-46b8-a693-56b559ca323a},\"Name\",\r\n{0},\"\",1,3,9595ddd6-e72c-47ad-a156-672db811628c,2,d5963243-262e-4398-b4d7-fb16d06484f6,3,c474bab9-d13a-4fbd-bfb0-9214d6dc2fde,2,640d7486-8abd-40aa-a244-2ed899b7225a,0},1,1,1,0,0,0,0,0},0}";

    #[test]
    fn an_adopted_tail_becomes_the_ordinary_one() {
        let normalized = normalize_descriptor(ADOPTED_MODULE).unwrap();
        assert_eq!(normalized.adopted.len(), 1);
        let header = &normalized.adopted[0];
        assert_eq!(header.uuid, "eb50ccde-ac43-46b8-a693-56b559ca323a");
        assert_eq!(
            header.properties,
            vec![
                (EXTENDED_OBJECT_PROPERTY.to_owned(), 2),
                ("d5963243-262e-4398-b4d7-fb16d06484f6".to_owned(), 3),
                ("c474bab9-d13a-4fbd-bfb0-9214d6dc2fde".to_owned(), 2),
            ]
        );
        assert_eq!(
            header.extended_object.as_deref(),
            Some("640d7486-8abd-40aa-a244-2ed899b7225a")
        );
        assert!(header.added_values.is_empty());
        assert_eq!(
            normalized.text,
            "{1,\r\n{12,\r\n{3,\r\n{1,0,eb50ccde-ac43-46b8-a693-56b559ca323a},\"Name\",\r\n{0},\"\",0,0,00000000-0000-0000-0000-000000000000,0},1,1,1,0,0,0,0,0},0}"
        );
    }

    #[test]
    fn an_ordinary_header_is_left_alone() {
        let text = "{1,{12,{3,{1,0,eb50ccde-ac43-46b8-a693-56b559ca323a},\"N\",{0},\"\",0,0,00000000-0000-0000-0000-000000000000,0},1}}";
        let normalized = normalize_descriptor(text).unwrap();
        assert!(normalized.adopted.is_empty());
        assert_eq!(normalized.text, text);
    }

    #[test]
    fn added_values_and_nested_headers_are_read() {
        let text = concat!(
            "{1,{2,{3,{1,0,98b71d7d-a47a-4538-aa66-3201fccbf890},\"C, \"\"q\"\"\",{0},\"a,b\",",
            "1,2,9595ddd6-e72c-47ad-a156-672db811628c,2,7d14f63a-87e8-4188-a28b-02da93f6bcbd,3,",
            "41f87891-ddd5-4e0f-a4f2-8039fb71981b,1,7d14f63a-87e8-4188-a28b-02da93f6bcbd,3,{\"#\",f5c65050-3bbb-11d5-b988-0050bae0a95d,{\"Pattern\",{\"#\",2064b508-2456-409c-b230-603228757b20}}}},",
            "{3,{1,0,523d19b5-0dd0-4377-9e32-3b28b327d814},\"A\",{0},\"\",1,1,b1053250-abe6-11d4-9434-004095e12fc7,2,00000000-0000-0000-0000-000000000000,0}}}"
        );
        let normalized = normalize_descriptor(text).unwrap();
        assert_eq!(normalized.adopted.len(), 2);
        assert_eq!(normalized.adopted[0].added_values.len(), 1);
        assert_eq!(
            normalized.adopted[0].added_values[0].2,
            "{\"#\",f5c65050-3bbb-11d5-b988-0050bae0a95d,{\"Pattern\",{\"#\",2064b508-2456-409c-b230-603228757b20}}}"
        );
        assert_eq!(normalized.adopted[1].extended_object, None);
        assert!(!normalized.text.contains("b1053250"));
        assert_eq!(
            normalized
                .text
                .matches(",0,0,00000000-0000-0000-0000-000000000000,0}")
                .count(),
            2
        );
    }

    #[test]
    fn an_unknown_belonging_is_refused() {
        let text = "{1,{3,{1,0,eb50ccde-ac43-46b8-a693-56b559ca323a},\"N\",{0},\"\",5,0,00000000-0000-0000-0000-000000000000,0}}";
        assert!(normalize_descriptor(text).is_err());
    }
}
