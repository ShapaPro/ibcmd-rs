//! Base-free staging: the complete Config row set an EMPTY infobase needs,
//! built from the source tree alone.
//!
//! Every metadata XML of the tree (nested subsystems, recalculations, forms
//! and templates included) gives its descriptor row through
//! `metadata_model::compile_descriptor` and its bodies through the loader's
//! own body writers, run with `BASE_FREE_STAGE` set so that a writer which
//! would patch a base row fails naming that row instead of querying. The
//! configuration adds `root`, `version` and a fresh `versions`.
//!
//! Two consumers share it: `audit-empty-stage`, which compares the row set
//! with a database's stored Config rows without touching any database, and
//! `mssql-stage-source-objects --base-free`, which loads it into ConfigSave
//! with SQL that does not read Config at all.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use anyhow::{Context, Result, anyhow, bail};
use rayon::prelude::*;
use serde::Serialize;

use super::{
    BASE_FREE_MISSING_ROW, BASE_FREE_STAGE, BulkStageRow, GeneratedBlobReport, MetadataBodyFamily,
    SqlAuth, StageSourceObjectsReport, StagedMetadataBodyReport, StagedMetadataObjectReport,
    StorageTableManifest, bcp_in_with_auth, build_bulk_stage_prepare_sql, bulk_stage_paths,
    bulk_stage_table_name, command_interface_body_suffix, infer_common_module_text_path,
    mssql_compile_axes_from_metadata_xml, pack_module_body_source, prepare_metadata_body_family,
    quote_ident, quote_string, require_non_lab_confirmation, resolve_sqlcmd_password,
    run_sql_capture_with_auth, run_sql_file_with_auth, source_module_body_path,
    source_xml_version_from_bytes, storage_table_stats_with_auth, write_bulk_stage_rows,
};
use crate::cli::MssqlStageSourceObjectsArgs;
use crate::compiler::families::assets::SourceAssetRegistry;
use crate::metadata_model::audit::{brace_path_at, descriptor_xmls};
use crate::metadata_model::root::{
    ConfigurationFacts, MODULE_GROUP_CLASS_ID, configuration_facts, root_row, version_row,
    versions_row,
};
use crate::metadata_model::{DescriptorContext, compile_descriptor};
use crate::module_blob::{
    SimpleMetadataXmlProperties, deflate_raw, hex_sha256, inflate_raw,
    parse_simple_metadata_xml_properties,
};
use crate::parallel;

/// One row of the stage: its Config file name and stored bytes.
#[derive(Debug, Clone)]
pub(crate) struct EmptyStageRow {
    pub file_name: String,
    /// `descriptor`, a body family (`kind body`, `help`, ...), `module` or
    /// `service`.
    pub family: String,
    /// The file it was compiled from, relative to the tree.
    pub source: String,
    /// The stored bytes: raw deflate of the row text.
    pub blob: Vec<u8>,
    /// The row text itself (BOM included), kept for the audit.
    pub plain: Option<Vec<u8>>,
}

/// A row, or a group of rows, the stage could not produce.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct EmptyStageFailure {
    /// The row it would have been, when known.
    pub file_name: Option<String>,
    pub kind: String,
    pub family: String,
    pub source: String,
    pub error: String,
}

/// One metadata XML with every row it gives.
#[derive(Debug)]
pub(crate) struct EmptyStageObject {
    pub kind: String,
    pub uuid: String,
    pub xml: PathBuf,
    pub relative: String,
    pub properties: Option<SimpleMetadataXmlProperties>,
    pub rows: Vec<EmptyStageRow>,
    pub failures: Vec<EmptyStageFailure>,
}

/// What every row of a tree needs besides its own XML.
pub(crate) struct EmptyStageContext {
    pub root: PathBuf,
    pub version: String,
    pub facts: ConfigurationFacts,
    pub descriptors: DescriptorContext,
    /// The managed-application module group the configuration's own rows
    /// are stored under.
    pub module_group: Option<String>,
}

impl EmptyStageContext {
    pub fn new(root: &Path, version: Option<&str>) -> Result<Self> {
        // No base rows exist: every base-row read fails naming its row.
        BASE_FREE_STAGE.store(true, Ordering::Relaxed);
        let configuration_path = root.join("Configuration.xml");
        let configuration = fs::read(&configuration_path)
            .with_context(|| format!("failed to read {}", configuration_path.display()))?;
        let version = match version {
            Some(version) => version.to_string(),
            None => source_xml_version_from_bytes(&configuration)?
                .ok_or_else(|| anyhow!("Configuration.xml declares no source version"))?,
        };
        let facts = configuration_facts(&configuration)?;
        let descriptors = DescriptorContext::new(root, &version)?;
        let module_group = module_group_of(&configuration);
        Ok(Self {
            root: root.to_path_buf(),
            version,
            facts,
            descriptors,
            module_group,
        })
    }
}

/// `<xr:ContainedObject>` of the module-group class in `Configuration.xml`.
fn module_group_of(xml: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(xml);
    let class = text.find(&format!("<xr:ClassId>{MODULE_GROUP_CLASS_ID}</xr:ClassId>"))?;
    let rest = &text[class..];
    let open = rest.find("<xr:ObjectId>")? + "<xr:ObjectId>".len();
    let close = rest[open..].find("</xr:ObjectId>")? + open;
    let uuid = rest[open..close].trim();
    (uuid.len() == 36).then(|| uuid.to_ascii_lowercase())
}

fn relative_of(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn error_text(error: &anyhow::Error) -> String {
    format!("{error:#}")
}

/// The row a failed writer would have written, when the failure names it or
/// the family has one row.
fn failed_row_name(
    error: &str,
    family: MetadataBodyFamily,
    kind: &str,
    uuid: &str,
    module_group: Option<&str>,
) -> Option<String> {
    if let Some(at) = error.find(BASE_FREE_MISSING_ROW) {
        let name = error[at + BASE_FREE_MISSING_ROW.len()..]
            .trim_start()
            .split(|ch: char| ch.is_whitespace() || ch == ':' || ch == ',')
            .next()
            .unwrap_or_default();
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    let owner = if kind == "Configuration" {
        module_group.unwrap_or(uuid)
    } else {
        uuid
    };
    let suffix = match family {
        MetadataBodyFamily::KindBody => match kind {
            "Form" | "CommonForm" | "Role" | "Template" | "CommonTemplate" | "CommonPicture"
            | "Style" | "ScheduledJob" | "XDTOPackage" | "WSReference" => "0",
            "ExchangePlan" => "1",
            "Catalog" => "1c",
            "ChartOfCharacteristicTypes" | "BusinessProcess" => "7",
            _ => return None,
        },
        MetadataBodyFamily::Help => SourceAssetRegistry
            .help_suffix(kind)?
            .trim_start_matches('.'),
        MetadataBodyFamily::CommandInterface => command_interface_body_suffix(kind)?,
        MetadataBodyFamily::AdditionalIndexes => match kind {
            "Document" => "3",
            "AccumulationRegister" => "4",
            _ => return None,
        },
        MetadataBodyFamily::ObjectModules | MetadataBodyFamily::NestedCommandModules => {
            return None;
        }
    };
    Some(format!("{owner}.{suffix}"))
}

fn catch<T>(run: impl FnOnce() -> Result<T>) -> Result<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)) {
        Ok(result) => result,
        Err(panic) => {
            let message = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|text| text.to_string()))
                .unwrap_or_default();
            Err(anyhow!("writer panicked: {message}"))
        }
    }
}

/// Every row one metadata XML gives, base-free.
pub(crate) fn prepare_empty_object(
    context: &EmptyStageContext,
    path: &Path,
    keep_plain: bool,
) -> EmptyStageObject {
    let relative = relative_of(&context.root, path);
    let mut object = EmptyStageObject {
        kind: String::new(),
        uuid: String::new(),
        xml: path.to_path_buf(),
        relative: relative.clone(),
        properties: None,
        rows: Vec::new(),
        failures: Vec::new(),
    };
    let fail = |object: &mut EmptyStageObject, family: &str, error: String| {
        object.failures.push(EmptyStageFailure {
            file_name: None,
            kind: object.kind.clone(),
            family: family.to_string(),
            source: relative.clone(),
            error,
        });
    };
    let xml = match fs::read(path) {
        Ok(xml) => xml,
        Err(error) => {
            fail(&mut object, "read", error.to_string());
            return object;
        }
    };
    let properties = match parse_simple_metadata_xml_properties(&xml) {
        Ok(properties) => properties,
        Err(error) => {
            fail(&mut object, "parse", error_text(&error));
            return object;
        }
    };
    object.kind = properties.kind.clone();
    object.uuid = properties.uuid.clone();
    let axes = match mssql_compile_axes_from_metadata_xml(&xml) {
        Ok(axes) => axes,
        Err(error) => {
            fail(&mut object, "parse", error_text(&error));
            return object;
        }
    };

    // The descriptor row.
    match catch(|| compile_descriptor(&properties.kind, path, &xml, &context.descriptors))
        .and_then(|plain| Ok((deflate_raw(&plain)?, plain)))
    {
        Ok((blob, plain)) => object.rows.push(EmptyStageRow {
            file_name: properties.uuid.clone(),
            family: "descriptor".to_string(),
            source: relative.clone(),
            blob,
            plain: keep_plain.then_some(plain),
        }),
        Err(error) => object.failures.push(EmptyStageFailure {
            file_name: Some(properties.uuid.clone()),
            kind: properties.kind.clone(),
            family: "descriptor".to_string(),
            source: relative.clone(),
            error: error_text(&error),
        }),
    }

    // The bodies.
    let source = Some(&context.descriptors.source);
    if properties.kind == "CommonModule" {
        if let Some(text_path) = source_module_body_path(infer_common_module_text_path(path)) {
            let body_id = format!("{}.0", properties.uuid);
            match catch(|| pack_module_body_source(&text_path, &body_id, &axes)) {
                Ok(packed) => object.rows.push(EmptyStageRow {
                    file_name: body_id,
                    family: "module".to_string(),
                    source: relative_of(&context.root, &text_path),
                    plain: None,
                    blob: packed.blob,
                }),
                Err(error) => object.failures.push(EmptyStageFailure {
                    file_name: Some(body_id),
                    kind: properties.kind.clone(),
                    family: "module".to_string(),
                    source: relative_of(&context.root, &text_path),
                    error: error_text(&error),
                }),
            }
        }
    } else {
        for family in MetadataBodyFamily::ALL {
            let result = catch(|| {
                prepare_metadata_body_family(
                    family,
                    Path::new("sqlcmd"),
                    "",
                    SqlAuth::integrated(),
                    "",
                    path,
                    &xml,
                    &properties,
                    source,
                    &axes,
                )
            });
            match result {
                Ok(rows) => object
                    .rows
                    .extend(rows.into_iter().map(|row| EmptyStageRow {
                        file_name: row.body_id,
                        family: family.label().to_string(),
                        source: relative_of(&context.root, &row.path),
                        blob: row.blob,
                        plain: None,
                    })),
                Err(error) => {
                    let error = error_text(&error);
                    object.failures.push(EmptyStageFailure {
                        file_name: failed_row_name(
                            &error,
                            family,
                            &properties.kind,
                            &properties.uuid,
                            context.module_group.as_deref(),
                        ),
                        kind: properties.kind.clone(),
                        family: family.label().to_string(),
                        source: relative.clone(),
                        error,
                    });
                }
            }
        }
    }
    object.properties = Some(properties);
    object
}

/// The service rows: `root`, `version`, and a `versions` naming `names`.
pub(crate) fn service_rows(
    context: &EmptyStageContext,
    names: &[String],
) -> Result<Vec<EmptyStageRow>> {
    let fresh = || uuid::Uuid::new_v4().hyphenated().to_string();
    let mut rows = Vec::with_capacity(3);
    for (file_name, plain) in [
        ("root", root_row(&context.facts)),
        ("version", version_row(&context.facts, &fresh())),
        ("versions", versions_row(names, fresh)),
    ] {
        rows.push(EmptyStageRow {
            file_name: file_name.to_string(),
            family: "service".to_string(),
            source: "Configuration.xml".to_string(),
            blob: deflate_raw(&plain)?,
            plain: Some(plain),
        });
    }
    Ok(rows)
}

/// The whole stage, in tree order, service rows last.
pub(crate) struct EmptyStage {
    pub context: EmptyStageContext,
    pub objects: Vec<EmptyStageObject>,
    pub service: Vec<EmptyStageRow>,
}

impl EmptyStage {
    pub fn failures(&self) -> impl Iterator<Item = &EmptyStageFailure> {
        self.objects
            .iter()
            .flat_map(|object| object.failures.iter())
    }

    pub fn rows(&self) -> impl Iterator<Item = &EmptyStageRow> {
        self.objects
            .iter()
            .flat_map(|object| object.rows.iter())
            .chain(self.service.iter())
    }
}

pub(crate) fn prepare_empty_stage(root: &Path, version: Option<&str>) -> Result<EmptyStage> {
    let context = EmptyStageContext::new(root, version)?;
    let paths = descriptor_xmls(root);
    let objects = parallel::install(|| {
        paths
            .par_iter()
            .map(|path| prepare_empty_object(&context, path, false))
            .collect::<Vec<_>>()
    })?;
    let names = objects
        .iter()
        .flat_map(|object| object.rows.iter().map(|row| row.file_name.clone()))
        .collect::<Vec<_>>();
    let service = service_rows(&context, &names)?;
    Ok(EmptyStage {
        context,
        objects,
        service,
    })
}

// ---------------------------------------------------------------------------
// The audit.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct EmptyStageAuditOptions {
    /// Samples kept per (pattern, kind, outcome).
    pub max_samples: usize,
    /// Where the stored and produced text of differing samples is written.
    pub diff_dir: Option<PathBuf>,
    /// A TSV of every produced row (file name, family, bytes, sha256 of the
    /// stored bytes, sha256 of the inflated text), to check a
    /// `--base-free --script-only` bcp file against.
    pub manifest: Option<PathBuf>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct EmptyStageOutcomes {
    pub identical: usize,
    pub different: usize,
    /// Of `different`: equal once line breaks are ignored.
    pub different_layout: usize,
    /// Of `different`: v8 containers whose elements are equal (their
    /// headers carry the time a platform wrote them).
    pub different_headers: usize,
    /// In Config, not produced.
    pub missing: usize,
    /// Produced, not in Config.
    pub extra: usize,
    /// Would have been produced; the writer failed.
    pub failed: usize,
}

impl EmptyStageOutcomes {
    fn add(&mut self, outcome: Outcome, benign: Option<Benign>) {
        match outcome {
            Outcome::Identical => self.identical += 1,
            Outcome::Different => {
                self.different += 1;
                match benign {
                    Some(Benign::Layout) => self.different_layout += 1,
                    Some(Benign::Headers) => self.different_headers += 1,
                    None => {}
                }
            }
            Outcome::Missing => self.missing += 1,
            Outcome::Extra => self.extra += 1,
            Outcome::Failed => self.failed += 1,
        }
    }
    fn total(&self) -> usize {
        self.identical + self.different + self.missing + self.extra + self.failed
    }
}

/// A difference that leaves the content equal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Benign {
    Layout,
    Headers,
}

impl Benign {
    fn label(self) -> &'static str {
        match self {
            Self::Layout => "layout",
            Self::Headers => "container headers",
        }
    }
}

fn benign_difference(stored: &[u8], produced: &[u8]) -> Option<Benign> {
    let strip = |bytes: &[u8]| {
        bytes
            .iter()
            .copied()
            .filter(|byte| !matches!(byte, b'\r' | b'\n'))
            .collect::<Vec<_>>()
    };
    if strip(stored) == strip(produced) {
        return Some(Benign::Layout);
    }
    containers_equal(stored, produced, 0).then_some(Benign::Headers)
}

/// Both v8 containers, with the same element names and equal data (nested
/// containers compared the same way).
fn containers_equal(left: &[u8], right: &[u8], depth: usize) -> bool {
    if depth > 3 {
        return false;
    }
    let (Ok(left), Ok(right)) = (
        crate::v8_container::parse_v8_container(left),
        crate::v8_container::parse_v8_container(right),
    ) else {
        return false;
    };
    left.len() == right.len()
        && left.iter().zip(right.iter()).all(|(left, right)| {
            left.name == right.name
                && (left.data == right.data || containers_equal(&left.data, &right.data, depth + 1))
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Outcome {
    Identical,
    Different,
    Missing,
    Extra,
    Failed,
}

impl Outcome {
    fn label(self) -> &'static str {
        match self {
            Self::Identical => "identical",
            Self::Different => "different",
            Self::Missing => "missing",
            Self::Extra => "extra",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct EmptyStageSample {
    pub file_name: String,
    pub kind: String,
    pub family: String,
    pub source: String,
    /// First differing byte of the inflated text, and its brace path.
    pub offset: Option<usize>,
    pub brace_path: Option<String>,
    pub detail: String,
}

#[derive(Debug, Default, Serialize)]
pub struct EmptyStageVersionsReport {
    pub stored_names: usize,
    pub produced_names: usize,
    /// Names only the stored `versions` lists (samples).
    pub only_stored: Vec<String>,
    pub only_stored_count: usize,
    /// Names only the produced one lists (samples).
    pub only_produced: Vec<String>,
    pub only_produced_count: usize,
}

#[derive(Debug, Default, Serialize)]
pub struct EmptyStageAuditReport {
    pub root: String,
    pub rows: String,
    pub source_version: String,
    pub configuration_shape: String,
    pub compatibility: u32,
    pub descriptor_xmls: usize,
    pub stored_rows: usize,
    /// Stored rows kept in more than one part (only part 0 is compared).
    pub multipart_rows: Vec<String>,
    pub produced_rows: usize,
    pub failures: usize,
    pub totals: EmptyStageOutcomes,
    /// By FileName pattern: `<uuid>`, `<uuid>.0`, `root`, ...
    pub by_pattern: BTreeMap<String, EmptyStageOutcomes>,
    /// By owner kind (the metadata class whose uuid the file name starts with).
    pub by_kind: BTreeMap<String, EmptyStageOutcomes>,
    /// By what produced (or would have produced) the row.
    pub by_family: BTreeMap<String, EmptyStageOutcomes>,
    /// `pattern | kind` for the rows that are not identical.
    pub by_pattern_kind: BTreeMap<String, EmptyStageOutcomes>,
    /// Failure reasons (uuids masked) with counts, by family.
    pub failure_reasons: BTreeMap<String, usize>,
    /// `versions` compared by names (its uuids are fresh by design).
    pub versions: EmptyStageVersionsReport,
    pub samples: BTreeMap<String, Vec<EmptyStageSample>>,
}

/// A stored Config row set: `<name>__part<N>.bin` (raw deflate) or
/// `<name>__part<N>.txt` (inflated).
struct StoredRows {
    dir: PathBuf,
    /// name -> is `.txt`
    names: BTreeMap<String, bool>,
    multipart: Vec<String>,
}

impl StoredRows {
    fn scan(dir: &Path) -> Result<Self> {
        let mut names = BTreeMap::new();
        let mut multipart = BTreeSet::new();
        for entry in
            fs::read_dir(dir).with_context(|| format!("failed to list {}", dir.display()))?
        {
            let entry = entry?;
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let (stem, text) = if let Some(stem) = file_name.strip_suffix(".bin") {
                (stem, false)
            } else if let Some(stem) = file_name.strip_suffix(".txt") {
                (stem, true)
            } else {
                continue;
            };
            let Some((name, part)) = stem.rsplit_once("__part") else {
                continue;
            };
            if part == "0" {
                names.entry(name.to_string()).or_insert(text);
            } else {
                multipart.insert(name.to_string());
            }
        }
        Ok(Self {
            dir: dir.to_path_buf(),
            names,
            multipart: multipart.into_iter().collect(),
        })
    }

    /// The inflated text of a stored row (a row that is not deflate is
    /// returned as stored).
    fn plain(&self, name: &str) -> Result<Option<Vec<u8>>> {
        let Some(text) = self.names.get(name) else {
            return Ok(None);
        };
        let path = self.dir.join(format!(
            "{name}__part0.{}",
            if *text { "txt" } else { "bin" }
        ));
        let bytes =
            fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
        if *text {
            return Ok(Some(bytes));
        }
        Ok(Some(inflate_raw(&bytes).unwrap_or(bytes)))
    }
}

fn is_uuid(text: &str) -> bool {
    text.len() == 36
        && text.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}

/// `<uuid>`, `<uuid>.<suffix>`, `<uuid>_dynupdate_<uuid>[.<suffix>]`, or the
/// name itself.
fn pattern_of(name: &str) -> String {
    let (base, suffix) = match name.split_once('.') {
        Some((base, suffix)) => (base, Some(suffix)),
        None => (name, None),
    };
    let base = if is_uuid(base) {
        "<uuid>".to_string()
    } else if let Some((left, right)) = base.split_once("_dynupdate_")
        && is_uuid(left)
        && is_uuid(right)
    {
        "<uuid>_dynupdate_<uuid>".to_string()
    } else {
        return name.to_string();
    };
    match suffix {
        Some(suffix) => format!("{base}.{suffix}"),
        None => base,
    }
}

/// Masks uuids and cuts a failure to one short line.
fn reason_of(error: &str) -> String {
    let line = error.lines().next().unwrap_or_default();
    let mut out = String::with_capacity(line.len());
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if index + 36 <= bytes.len()
            && line.is_char_boundary(index + 36)
            && is_uuid(&line[index..index + 36])
        {
            out.push_str("<uuid>");
            index += 36;
            continue;
        }
        let ch = line[index..].chars().next().unwrap_or(' ');
        out.push(ch);
        index += ch.len_utf8();
    }
    // Paths differ per object; keep the text before the first one.
    let cut = out
        .find(":\\")
        .map(|at| at.saturating_sub(1))
        .into_iter()
        .chain(out.find(" F:/"))
        .chain(out.find(" E:/"))
        .min();
    if let Some(cut) = cut {
        out.truncate(cut);
        out.push_str(" <path>");
    }
    out.chars().take(220).collect()
}

fn first_difference(left: &[u8], right: &[u8]) -> usize {
    left.iter()
        .zip(right.iter())
        .position(|(l, r)| l != r)
        .unwrap_or_else(|| left.len().min(right.len()))
}

fn excerpt(text: &[u8], offset: usize) -> String {
    let start = offset.saturating_sub(40);
    let end = (offset + 60).min(text.len());
    String::from_utf8_lossy(&text[start.min(end)..end]).replace("\r\n", "⏎")
}

/// uuids that are masked when a service row is compared: they are fresh by
/// design.
fn mask_uuids(text: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(text);
    reason_of_all(&text).into_bytes()
}

fn reason_of_all(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < text.len() {
        if index + 36 <= text.len()
            && text.is_char_boundary(index + 36)
            && is_uuid(&text[index..index + 36])
        {
            out.push_str("<uuid>");
            index += 36;
            continue;
        }
        let ch = text[index..].chars().next().unwrap_or(' ');
        out.push(ch);
        index += ch.len_utf8();
    }
    out
}

/// The names a `versions` row lists (the leading `""` aside).
fn versions_names(plain: &[u8]) -> BTreeSet<String> {
    let text = String::from_utf8_lossy(plain);
    let mut names = BTreeSet::new();
    let mut rest = text.as_ref();
    while let Some(open) = rest.find('"') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('"') else {
            break;
        };
        let name = &after[..close];
        if !name.is_empty() {
            names.insert(name.to_string());
        }
        rest = &after[close + 1..];
    }
    names
}

struct Measured {
    file_name: String,
    kind: String,
    family: String,
    source: String,
    outcome: Outcome,
    benign: Option<Benign>,
    offset: Option<usize>,
    brace_path: Option<String>,
    detail: String,
    expected: Option<Vec<u8>>,
    actual: Option<Vec<u8>>,
}

pub fn audit_empty_stage(
    root: &Path,
    rows: &Path,
    version: Option<&str>,
    options: &EmptyStageAuditOptions,
) -> Result<EmptyStageAuditReport> {
    let stored = StoredRows::scan(rows)?;
    let context = EmptyStageContext::new(root, version)?;
    let paths = descriptor_xmls(root);

    // Owner kind of every uuid the tree names.
    let mut kinds: HashMap<String, String> = HashMap::new();
    for entry in context.descriptors.index.objects.values() {
        kinds.insert(entry.uuid.clone(), entry.kind.clone());
    }
    for (full, uuid) in &context.descriptors.index.children {
        let parts = full.rsplitn(3, '.').collect::<Vec<_>>();
        let kind = parts.get(1).copied().unwrap_or("Child");
        kinds
            .entry(uuid.clone())
            .or_insert_with(|| kind.to_string());
    }
    if let Some(group) = &context.module_group {
        kinds.insert(group.clone(), "Configuration".to_string());
    }
    let kind_of = |name: &str| -> String {
        let base = name.split(['.', '_']).next().unwrap_or(name);
        kinds.get(base).cloned().unwrap_or_else(|| {
            if matches!(name, "root" | "version" | "versions") {
                "Configuration".to_string()
            } else {
                "<unknown>".to_string()
            }
        })
    };
    let keep_samples = options.diff_dir.is_some();

    // Produce and compare object by object, in parallel; keep outcomes only.
    let per_object = parallel::install(|| {
        paths
            .par_iter()
            .map(
                |path| -> Result<(
                    Vec<Measured>,
                    Vec<String>,
                    Vec<(String, String, usize, String, String)>,
                )> {
                    let object = prepare_empty_object(&context, path, true);
                    let mut measured = Vec::new();
                    let mut names = Vec::new();
                    let mut manifest = Vec::new();
                    for row in &object.rows {
                        names.push(row.file_name.clone());
                        let plain = match &row.plain {
                            Some(plain) => plain.clone(),
                            None => inflate_raw(&row.blob).unwrap_or_else(|_| row.blob.clone()),
                        };
                        if options.manifest.is_some() {
                            manifest.push((
                                row.file_name.clone(),
                                row.family.clone(),
                                row.blob.len(),
                                hex_sha256(&row.blob),
                                hex_sha256(&plain),
                            ));
                        }
                        let (outcome, benign, offset, brace_path, detail, expected) =
                            match stored.plain(&row.file_name)? {
                                None => (Outcome::Extra, None, None, None, String::new(), None),
                                Some(expected) if expected == plain => {
                                    (Outcome::Identical, None, None, None, String::new(), None)
                                }
                                Some(expected) => {
                                    let offset = first_difference(&expected, &plain);
                                    let detail = format!(
                                        "stored {} | produced {}",
                                        excerpt(&expected, offset),
                                        excerpt(&plain, offset)
                                    );
                                    (
                                        Outcome::Different,
                                        benign_difference(&expected, &plain),
                                        Some(offset),
                                        Some(brace_path_at(&expected, offset)),
                                        detail,
                                        Some(expected),
                                    )
                                }
                            };
                        measured.push(Measured {
                            file_name: row.file_name.clone(),
                            kind: object.kind.clone(),
                            family: row.family.clone(),
                            source: row.source.clone(),
                            outcome,
                            benign,
                            offset,
                            brace_path,
                            detail,
                            expected: if keep_samples { expected } else { None },
                            actual: if keep_samples && outcome == Outcome::Different {
                                Some(plain)
                            } else {
                                None
                            },
                        });
                    }
                    for failure in &object.failures {
                        measured.push(Measured {
                            file_name: failure.file_name.clone().unwrap_or_default(),
                            kind: if failure.kind.is_empty() {
                                "<unparsed>".to_string()
                            } else {
                                failure.kind.clone()
                            },
                            family: failure.family.clone(),
                            source: failure.source.clone(),
                            outcome: Outcome::Failed,
                            benign: None,
                            offset: None,
                            brace_path: None,
                            detail: failure.error.clone(),
                            expected: None,
                            actual: None,
                        });
                    }
                    Ok((measured, names, manifest))
                },
            )
            .collect::<Result<Vec<_>>>()
    })??;

    let mut measured = Vec::new();
    let mut names = Vec::new();
    let mut manifest = Vec::new();
    for (object_measured, object_names, object_manifest) in per_object {
        measured.extend(object_measured);
        names.extend(object_names);
        manifest.extend(object_manifest);
    }

    // Service rows: root and version compared with generation uuids masked,
    // versions by its names.
    let service = service_rows(&context, &names)?;
    let mut versions_report = EmptyStageVersionsReport::default();
    for row in &service {
        let plain = row.plain.clone().unwrap_or_default();
        if options.manifest.is_some() {
            manifest.push((
                row.file_name.clone(),
                row.family.clone(),
                row.blob.len(),
                hex_sha256(&row.blob),
                hex_sha256(&plain),
            ));
        }
        names.push(row.file_name.clone());
        let expected = stored.plain(&row.file_name)?;
        let (outcome, detail) = match &expected {
            None => (Outcome::Extra, String::new()),
            Some(expected) if row.file_name == "versions" => {
                let stored_names = versions_names(expected);
                let produced_names = versions_names(&plain);
                versions_report.stored_names = stored_names.len();
                versions_report.produced_names = produced_names.len();
                let only_stored = stored_names.difference(&produced_names).collect::<Vec<_>>();
                let only_produced = produced_names.difference(&stored_names).collect::<Vec<_>>();
                versions_report.only_stored_count = only_stored.len();
                versions_report.only_produced_count = only_produced.len();
                versions_report.only_stored = only_stored
                    .iter()
                    .take(50)
                    .map(|name| name.to_string())
                    .collect();
                versions_report.only_produced = only_produced
                    .iter()
                    .take(50)
                    .map(|name| name.to_string())
                    .collect();
                if only_stored.is_empty() && only_produced.is_empty() {
                    (
                        Outcome::Identical,
                        "names identical, uuids fresh".to_string(),
                    )
                } else {
                    (
                        Outcome::Different,
                        format!(
                            "{} names only stored, {} only produced",
                            only_stored.len(),
                            only_produced.len()
                        ),
                    )
                }
            }
            Some(expected) => {
                let left = mask_uuids(expected);
                let right = mask_uuids(&plain);
                if left == right {
                    (Outcome::Identical, String::new())
                } else {
                    let offset = first_difference(&left, &right);
                    (
                        Outcome::Different,
                        format!(
                            "stored {} | produced {}",
                            excerpt(&left, offset),
                            excerpt(&right, offset)
                        ),
                    )
                }
            }
        };
        measured.push(Measured {
            file_name: row.file_name.clone(),
            kind: "Configuration".to_string(),
            family: row.family.clone(),
            source: row.source.clone(),
            outcome,
            benign: None,
            offset: None,
            brace_path: None,
            detail,
            expected: None,
            actual: None,
        });
    }

    // Stored rows nobody produced or failed to produce.
    let accounted = measured
        .iter()
        .filter(|item| !item.file_name.is_empty())
        .map(|item| item.file_name.clone())
        .collect::<BTreeSet<_>>();
    for name in stored.names.keys() {
        if !accounted.contains(name) {
            measured.push(Measured {
                file_name: name.clone(),
                kind: kind_of(name),
                family: "<none>".to_string(),
                source: String::new(),
                outcome: Outcome::Missing,
                benign: None,
                offset: None,
                brace_path: None,
                detail: String::new(),
                expected: None,
                actual: None,
            });
        }
    }

    let mut report = EmptyStageAuditReport {
        root: root.display().to_string(),
        rows: rows.display().to_string(),
        source_version: context.version.clone(),
        configuration_shape: format!("{:?}", context.facts.shape),
        compatibility: context.facts.compatibility,
        descriptor_xmls: paths.len(),
        stored_rows: stored.names.len(),
        multipart_rows: stored.multipart.clone(),
        produced_rows: names.len(),
        failures: measured
            .iter()
            .filter(|item| item.outcome == Outcome::Failed)
            .count(),
        versions: versions_report,
        ..Default::default()
    };
    measured.sort_by(|left, right| left.file_name.cmp(&right.file_name));
    for item in &measured {
        let pattern = if item.file_name.is_empty() {
            "<unattributed>".to_string()
        } else {
            pattern_of(&item.file_name)
        };
        report.totals.add(item.outcome, item.benign);
        report
            .by_pattern
            .entry(pattern.clone())
            .or_default()
            .add(item.outcome, item.benign);
        report
            .by_kind
            .entry(item.kind.clone())
            .or_default()
            .add(item.outcome, item.benign);
        report
            .by_family
            .entry(item.family.clone())
            .or_default()
            .add(item.outcome, item.benign);
        if item.outcome != Outcome::Identical {
            report
                .by_pattern_kind
                .entry(format!("{pattern} | {}", item.kind))
                .or_default()
                .add(item.outcome, item.benign);
        }
        if item.outcome == Outcome::Failed {
            *report
                .failure_reasons
                .entry(format!(
                    "{} | {} | {}",
                    item.kind,
                    item.family,
                    reason_of(&item.detail)
                ))
                .or_default() += 1;
        }
        if item.outcome != Outcome::Identical {
            let outcome = match item.benign {
                Some(benign) => format!("{} ({})", item.outcome.label(), benign.label()),
                None => item.outcome.label().to_string(),
            };
            let key = format!("{outcome} | {pattern} | {}", item.kind);
            let samples = report.samples.entry(key).or_default();
            if samples.len() < options.max_samples {
                if let (Some(dir), Some(expected), Some(actual)) =
                    (&options.diff_dir, &item.expected, &item.actual)
                {
                    let folder = dir.join(&item.kind);
                    fs::create_dir_all(&folder)?;
                    fs::write(
                        folder.join(format!("{}.stored.txt", item.file_name)),
                        expected,
                    )?;
                    fs::write(
                        folder.join(format!("{}.produced.txt", item.file_name)),
                        actual,
                    )?;
                }
                samples.push(EmptyStageSample {
                    file_name: item.file_name.clone(),
                    kind: item.kind.clone(),
                    family: item.family.clone(),
                    source: item.source.clone(),
                    offset: item.offset,
                    brace_path: item.brace_path.clone(),
                    detail: item.detail.chars().take(400).collect(),
                });
            }
        }
    }
    if let Some(path) = &options.manifest {
        manifest.sort();
        let mut text = String::from("file_name\tfamily\tbytes\tblob_sha256\tplain_sha256\n");
        for (name, family, bytes, blob, plain) in manifest {
            text.push_str(&format!("{name}\t{family}\t{bytes}\t{blob}\t{plain}\n"));
        }
        fs::write(path, text).with_context(|| format!("failed to write {}", path.display()))?;
    }
    Ok(report)
}

/// Tables for the terminal.
pub fn empty_stage_summary(report: &EmptyStageAuditReport) -> String {
    fn table(title: &str, rows: &BTreeMap<String, EmptyStageOutcomes>, limit: usize) -> String {
        let mut rows = rows.iter().collect::<Vec<_>>();
        rows.sort_by(|left, right| {
            right
                .1
                .total()
                .cmp(&left.1.total())
                .then(left.0.cmp(right.0))
        });
        let mut lines = vec![format!(
            "{:<44} {:>7} {:>9} {:>9} {:>7} {:>7} {:>7} {:>6} {:>6}",
            title,
            "total",
            "identical",
            "different",
            "layout",
            "headers",
            "missing",
            "extra",
            "failed"
        )];
        for (key, counts) in rows.into_iter().take(limit) {
            lines.push(format!(
                "{:<44} {:>7} {:>9} {:>9} {:>7} {:>7} {:>7} {:>6} {:>6}",
                key.chars().take(44).collect::<String>(),
                counts.total(),
                counts.identical,
                counts.different,
                counts.different_layout,
                counts.different_headers,
                counts.missing,
                counts.extra,
                counts.failed
            ));
        }
        lines.join("\n")
    }
    let mut out = vec![
        format!(
            "{} | {} | shape {} compat {} | {} XMLs | stored {} rows, produced {}, failures {}",
            report.root,
            report.source_version,
            report.configuration_shape,
            report.compatibility,
            report.descriptor_xmls,
            report.stored_rows,
            report.produced_rows,
            report.failures
        ),
        format!(
            "TOTAL identical {} different {} (layout only {}, container headers only {}) missing {} extra {} failed {}",
            report.totals.identical,
            report.totals.different,
            report.totals.different_layout,
            report.totals.different_headers,
            report.totals.missing,
            report.totals.extra,
            report.totals.failed
        ),
        table("pattern", &report.by_pattern, 40),
        table("family", &report.by_family, 20),
        table("kind", &report.by_kind, 60),
    ];
    if !report.multipart_rows.is_empty() {
        out.push(format!(
            "{} stored rows have more than one part (only part 0 compared)",
            report.multipart_rows.len()
        ));
    }
    let mut reasons = report.failure_reasons.iter().collect::<Vec<_>>();
    reasons.sort_by(|left, right| right.1.cmp(left.1));
    out.push("failure reasons:".to_string());
    for (reason, count) in reasons.into_iter().take(40) {
        out.push(format!("{count:>7}  {reason}"));
    }
    out.join("\n")
}

// ---------------------------------------------------------------------------
// The loader mode.
// ---------------------------------------------------------------------------

/// `mssql-stage-source-objects --base-free`: every row of the tree into
/// ConfigSave through the bulk path, with no read of the target's Config.
pub(super) fn stage_source_objects_base_free(
    args: &MssqlStageSourceObjectsArgs,
) -> Result<StageSourceObjectsReport> {
    require_non_lab_confirmation(args.allow_non_lab, "source tree staging")?;
    if !args.replace_config_save {
        bail!("staging deletes existing ConfigSave rows; pass --replace-config-save");
    }
    if args.per_row {
        bail!("--base-free stages through the bulk path; drop --per-row");
    }
    if !args.path_prefix.is_empty() {
        bail!(
            "--base-free stages the whole tree (an empty infobase needs every row); drop --path-prefix"
        );
    }
    let version = args.source_version.map(|version| version.as_str());
    let stage = prepare_empty_stage(&args.source_root, version)?;
    let failures = stage.failures().collect::<Vec<_>>();
    if !failures.is_empty() {
        let mut reasons = BTreeMap::<String, usize>::new();
        for failure in &failures {
            *reasons
                .entry(format!(
                    "{} | {} | {}",
                    failure.kind,
                    failure.family,
                    reason_of(&failure.error)
                ))
                .or_default() += 1;
        }
        let mut reasons = reasons.into_iter().collect::<Vec<_>>();
        reasons.sort_by(|left, right| right.1.cmp(&left.1));
        let listed = reasons
            .iter()
            .take(25)
            .map(|(reason, count)| format!("  {count:>6}  {reason}"))
            .collect::<Vec<_>>()
            .join("\n");
        bail!(
            "a base-free stage needs every row, and {} could not be produced (audit-empty-stage lists them all):\n{listed}",
            failures.len()
        );
    }

    let rows = stage.rows().collect::<Vec<_>>();
    let mut seen = BTreeSet::new();
    for row in &rows {
        if !seen.insert(row.file_name.as_str()) {
            bail!("two rows of the tree share the file name {}", row.file_name);
        }
    }
    let bulk = rows
        .iter()
        .map(|row| BulkStageRow {
            file_name: &row.file_name,
            requires_config_row: false,
            blob: &row.blob,
        })
        .collect::<Vec<_>>();
    let (rows_path, prepare_path, apply_path) =
        bulk_stage_paths(args.script_output.as_ref(), &args.database);
    if let Some(parent) = rows_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let table = bulk_stage_table_name(&args.database);
    write_bulk_stage_rows(&rows_path, &bulk)?;
    fs::write(&prepare_path, build_bulk_stage_prepare_sql(&table))
        .with_context(|| format!("failed to write {}", prepare_path.display()))?;
    fs::write(
        &apply_path,
        build_base_free_bulk_stage_apply_sql(&args.database, &table, bulk.len()),
    )
    .with_context(|| format!("failed to write {}", apply_path.display()))?;

    let not_queried = |table: &str| StorageTableManifest {
        table_name: table.to_string(),
        file_name: String::new(),
        row_count: -1,
        binary_bytes: -1,
        row_checksum: None,
    };
    let mut before = not_queried("ConfigSave");
    let mut after = not_queried("ConfigSave");
    if !args.script_only {
        let sql_password = resolve_sqlcmd_password(
            args.sql_user.as_deref(),
            args.sql_pwd.as_deref(),
            &args.sql_pwd_env,
        );
        let sql_auth = SqlAuth {
            user: args.sql_user.as_deref(),
            password: sql_password.as_deref(),
        };
        before = storage_table_stats_with_auth(
            &args.sqlcmd,
            &args.server,
            sql_auth,
            &args.database,
            "ConfigSave",
        )?;
        let bcp = args
            .bcp_executable
            .clone()
            .unwrap_or_else(|| crate::mssql_dump::bcp_executable_for_sqlcmd(&args.sqlcmd));
        run_sql_file_with_auth(&args.sqlcmd, &args.server, sql_auth, &prepare_path)?;
        let loaded = bcp_in_with_auth(
            &bcp,
            &format!("tempdb.dbo.{}", quote_ident(&table)),
            &rows_path,
            &args.server,
            sql_auth,
        )
        .and_then(|()| run_sql_file_with_auth(&args.sqlcmd, &args.server, sql_auth, &apply_path));
        if let Err(error) = loaded {
            let drop = format!(
                "IF OBJECT_ID(N'tempdb.dbo.{name}', N'U') IS NOT NULL DROP TABLE tempdb.dbo.{table};",
                name = quote_string(&quote_ident(&table)),
                table = quote_ident(&table),
            );
            let _ = run_sql_capture_with_auth(&args.sqlcmd, &args.server, sql_auth, &drop);
            return Err(error);
        }
        let _ = fs::remove_file(&rows_path);
        after = storage_table_stats_with_auth(
            &args.sqlcmd,
            &args.server,
            sql_auth,
            &args.database,
            "ConfigSave",
        )?;
    }

    let versions = stage
        .service
        .iter()
        .find(|row| row.file_name == "versions")
        .map(|row| GeneratedBlobReport {
            bytes: row.blob.len(),
            sha256: hex_sha256(&row.blob),
        })
        .unwrap_or(GeneratedBlobReport {
            bytes: 0,
            sha256: String::new(),
        });
    let metadata_objects = stage
        .objects
        .iter()
        .filter_map(|object| {
            let properties = object.properties.clone()?;
            let descriptor = object.rows.iter().find(|row| row.file_name == object.uuid);
            Some(StagedMetadataObjectReport {
                object_id: object.uuid.clone(),
                kind: object.kind.clone(),
                xml: object.xml.clone(),
                properties,
                metadata_plain_bytes: 0,
                metadata_blob: GeneratedBlobReport {
                    bytes: descriptor.map_or(0, |row| row.blob.len()),
                    sha256: descriptor
                        .map(|row| hex_sha256(&row.blob))
                        .unwrap_or_default(),
                },
                body_rows: object
                    .rows
                    .iter()
                    .filter(|row| row.file_name != object.uuid)
                    .map(|row| StagedMetadataBodyReport {
                        body_id: row.file_name.clone(),
                        path: stage.context.root.join(&row.source),
                        blob: GeneratedBlobReport {
                            bytes: row.blob.len(),
                            sha256: hex_sha256(&row.blob),
                        },
                    })
                    .collect(),
            })
        })
        .collect();
    Ok(StageSourceObjectsReport {
        database: args.database.clone(),
        source_version: Some(stage.context.version.clone()),
        metadata_objects,
        common_modules: Vec::new(),
        scripts: vec![prepare_path, apply_path.clone()],
        script: apply_path,
        before,
        after,
        versions_blob: versions,
        version_replacements: Vec::new(),
    })
}

/// The bulk apply of a base-free stage: the staged rows are the whole
/// configuration, `root`, `version` and `versions` among them, so nothing is
/// copied from or checked against Config. Attributes are 0 (what every
/// staged body row the target lacks already gets on the default path) and
/// every row is part 0.
fn build_base_free_bulk_stage_apply_sql(database: &str, table: &str, staged_rows: usize) -> String {
    let stage = format!("tempdb.dbo.{}", quote_ident(table));
    format!(
        "SET NOCOUNT ON;\n\
         SET XACT_ABORT ON;\n\
         USE {db};\n\
         IF (SELECT COUNT_BIG(*) FROM {stage}) <> {staged_rows}\n\
             THROW 55002, 'bcp loaded an unexpected number of staged rows', 1;\n\
         IF EXISTS (SELECT 1 FROM {stage} WHERE DATALENGTH(BinaryData) <> DataSize)\n\
             THROW 55003, 'A staged row lost bytes on its way in', 1;\n\
         IF EXISTS (SELECT FileName FROM {stage} GROUP BY FileName HAVING COUNT_BIG(*) > 1)\n\
             THROW 55004, 'Two staged rows share a file name', 1;\n\
         IF (SELECT COUNT_BIG(*) FROM {stage} WHERE FileName IN (N'root', N'version', N'versions')) <> 3\n\
             THROW 55005, 'A base-free stage must carry root, version and versions', 1;\n\
         BEGIN TRAN;\n\
         DELETE FROM dbo.ConfigSave;\n\
         INSERT INTO dbo.ConfigSave (FileName, Creation, Modified, Attributes, DataSize, BinaryData, PartNo)\n\
         SELECT s.FileName, SYSUTCDATETIME(), SYSUTCDATETIME(), 0, s.DataSize, s.BinaryData, 0\n\
         FROM {stage} s;\n\
         IF (SELECT COUNT_BIG(*) FROM dbo.ConfigSave) <> {staged_rows}\n\
             THROW 56999, 'Unexpected ConfigSave row count after base-free staging', 1;\n\
         COMMIT;\n\
         DROP TABLE {stage};\n",
        db = quote_ident(database),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_and_reasons() {
        assert_eq!(pattern_of("66193438-abc5-410b-a1f1-a204102d1a62"), "<uuid>");
        assert_eq!(
            pattern_of("66193438-abc5-410b-a1f1-a204102d1a62.1c"),
            "<uuid>.1c"
        );
        assert_eq!(pattern_of("versions"), "versions");
        assert_eq!(
            pattern_of(
                "66193438-abc5-410b-a1f1-a204102d1a62_dynupdate_06cb0442-0c47-4fad-986a-f08f28287c1b.0"
            ),
            "<uuid>_dynupdate_<uuid>.0"
        );
        assert_eq!(
            reason_of(
                "base-free stage has no base Config row 66193438-abc5-410b-a1f1-a204102d1a62.0"
            ),
            "base-free stage has no base Config row <uuid>.0"
        );
        let names = versions_names(b"{1,3,\"\",u,\"a\",u,\"root\",u}");
        assert_eq!(names.into_iter().collect::<Vec<_>>(), vec!["a", "root"]);
    }

    #[test]
    fn base_free_apply_reads_no_config() {
        let sql = build_base_free_bulk_stage_apply_sql("db", "stage", 3);
        assert!(!sql.contains("dbo.Config "));
        assert!(!sql.contains("dbo.Config\n"));
        assert!(sql.contains("DELETE FROM dbo.ConfigSave"));
    }
}
