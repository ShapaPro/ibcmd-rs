//! `mssql-dump-config --model-export` (or `IBCMD_RS_MODEL_EXPORT=1`): the
//! descriptor rows of every kind the metadata model decodes go row -> model
//! -> XML through `metadata_model::export` instead of the legacy converters.
//!
//! Names are resolved through a [`NameIndex`] built from the rows in one
//! parallel pass: each object row through its kind's `object_names`, the
//! root row, and the predefined items of the predefined-data bodies. Kinds
//! whose rows `object_names` cannot read yet borrow their names from the
//! legacy indexes this export builds anyway (object references, generated
//! types, form and template paths), so the export stays whole while the
//! kinds move over one by one.

use super::*;
use crate::metadata_model::brace::parse_row;
use crate::metadata_model::export::names::{
    IndexComparison, compare, has_decoder, has_names, may_own_objects, owned_objects,
    predefined_items, predefined_suffix, root_kinds,
};
use crate::metadata_model::export::{ExportContext, NameIndex, decode_object, write_document};
use crate::metadata_model::index::ConfigIndex;
use crate::metadata_model::objects::parts::Compat;

/// The environment switch, for callers without the command-line flag.
pub(super) const MODEL_EXPORT_ENV: &str = "IBCMD_RS_MODEL_EXPORT";

/// `IBCMD_RS_MODEL_EXPORT_SHADOW=1`: the legacy converter also runs on the
/// modelled rows, to time it on the same rows and compare the bytes.
pub(super) fn shadow() -> bool {
    static SHADOW: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SHADOW.get_or_init(|| {
        std::env::var("IBCMD_RS_MODEL_EXPORT_SHADOW").is_ok_and(|value| value.trim() == "1")
    })
}

/// The bytes a descriptor's file gets (`write_source_xml_file`).
fn as_written(xml: &[u8], source_version: InfobaseConfigSourceVersion) -> Vec<u8> {
    let adapter = MssqlLegacyAdapter::from_legacy_selector(source_version);
    let mut normalized =
        normalize_legacy_source_asset_xml_version_bytes(xml, adapter.xml_dialect());
    if source_version == InfobaseConfigSourceVersion::V2_21 {
        normalized = declare_palette_namespace_beside_style(normalized);
    }
    normalized
}

/// Whether the legacy converter would have written the same file.
pub(super) fn same_as_written(
    legacy: Option<&ExtractedMetadataSourceXml>,
    modelled: &ExtractedMetadataSourceXml,
    source_version: InfobaseConfigSourceVersion,
) -> bool {
    legacy.is_some_and(|legacy| {
        legacy.relative_path == modelled.relative_path
            && as_written(&legacy.xml, source_version) == as_written(&modelled.xml, source_version)
    })
}

/// A modelled row whose file differs from the legacy one (the first twenty).
pub(super) fn report_shadow_difference(file_name: &str, relative_path: &Path) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static REPORTED: AtomicUsize = AtomicUsize::new(0);
    if REPORTED.fetch_add(1, Ordering::Relaxed) < 20 {
        eprintln!(
            "model export shadow: {file_name} ({}) differs from the legacy converter's file",
            relative_path.display()
        );
    }
}

/// `--model-export`, or `IBCMD_RS_MODEL_EXPORT=1`.
pub(super) fn requested(flag: bool) -> bool {
    flag || std::env::var(MODEL_EXPORT_ENV).is_ok_and(|value| value.trim() == "1")
}

/// The legacy indexes the row-built index borrows names from, for the kinds
/// without `object_names`.
pub(super) struct LegacyNames<'a> {
    pub(super) object_refs: &'a BTreeMap<String, String>,
    pub(super) type_index: &'a BTreeMap<String, String>,
    pub(super) form_refs: &'a BTreeMap<String, FormSourceReference>,
    pub(super) template_refs: &'a BTreeMap<String, TemplateSourceReference>,
}

/// What the index was built from.
#[derive(Debug, Default, Clone, Serialize)]
pub(super) struct ModelIndexReport {
    /// Rows read through `object_names`, by kind.
    pub(super) rows_by_kind: BTreeMap<String, usize>,
    /// Rows of kinds without `object_names`, named from the legacy indexes.
    pub(super) legacy_rows_by_kind: BTreeMap<String, usize>,
    /// Rows `object_names` failed on (kind: first error line -> count).
    pub(super) failures: BTreeMap<String, usize>,
    pub(super) names: usize,
    pub(super) types: usize,
    pub(super) predefined: usize,
    /// Owned objects (forms, templates, recalculations, nested subsystems)
    /// named through their owner's row, and those only the legacy path
    /// indexes named.
    pub(super) owned_names: usize,
    pub(super) legacy_owned_names: usize,
    /// Of `names`/`types`, what the legacy indexes gave.
    pub(super) legacy_names: usize,
    pub(super) legacy_types: usize,
    pub(super) predefined_bodies: usize,
    pub(super) compatibility_mode: Option<String>,
}

pub(super) struct ModelExport {
    context: ExportContext,
    /// Every top-level object's kind, from the root row.
    kinds: HashMap<String, &'static str>,
    /// The root row's own name: the configuration's uuid.
    root: Option<String>,
    pub(super) report: ModelIndexReport,
}

/// A text row's raw row: inflated as stored, with no rewriting.
fn raw_row_text(row: &ConfigRow) -> Result<Vec<u8>> {
    let bytes = row.binary_bytes()?;
    inflate_raw_deflate(&bytes)
}

/// The root kind of a full name (`Catalog.X.Attribute.Y` -> `Catalog`).
fn root_kind(full_name: &str) -> &str {
    full_name.split('.').next().unwrap_or_default()
}

/// The folder a top-level kind's files go to (`Catalog` -> `Catalogs`).
fn kind_folder(kind: &str) -> Option<String> {
    let folder = if kind == "FilterCriterion" {
        "FilterCriteria".to_string()
    } else if let Some(rest) = kind.strip_prefix("ChartOf") {
        format!("ChartsOf{rest}")
    } else if kind.ends_with("ss") {
        format!("{kind}es")
    } else {
        format!("{kind}s")
    };
    (crate::metadata_model::index::kind_of_collection(&folder) == Some(kind)).then_some(folder)
}

/// The root row among the text rows: the configuration's own row.
fn root_text_row(texts: &[MetadataTextRow]) -> Option<&MetadataTextRow> {
    texts.iter().find(|row| {
        !row.file_name.contains('.')
            && parse_configuration_reference_text_for_row(&row.text, &row.file_name).is_some()
    })
}

impl ModelExport {
    /// The index and context for one export.
    pub(super) fn build(
        metadata_rows: &[ConfigRow],
        texts: &[MetadataTextRow],
        predefined_rows: &[ConfigRow],
        legacy: &LegacyNames<'_>,
        source_version: InfobaseConfigSourceVersion,
    ) -> Result<Self> {
        let version = source_version.as_str().to_string();
        let raw_by_name = metadata_rows
            .iter()
            .map(|row| (row.file_name.as_str(), row))
            .collect::<HashMap<_, _>>();
        let mut report = ModelIndexReport::default();

        // The root row: the configuration's own uuid, its compatibility and
        // the kind of every top-level object.
        let root = root_text_row(texts);
        let compatibility_mode = root.and_then(|row| {
            parse_configuration_properties_from_text(&row.text, &BTreeMap::new(), source_version)
                .and_then(|properties| properties.compatibility_mode)
        });
        report.compatibility_mode = compatibility_mode.clone();
        let compat = compatibility_mode
            .as_deref()
            .and_then(Compat::parse)
            .unwrap_or(if source_version == InfobaseConfigSourceVersion::V2_21 {
                Compat(8, 5, 1)
            } else {
                Compat(8, 3, 27)
            });
        let kinds = root
            .and_then(|row| raw_by_name.get(row.file_name.as_str()))
            .and_then(|raw| raw_row_text(raw).ok())
            .and_then(|text| parse_row(&text).ok())
            .map(|tree| root_kinds(&tree))
            .unwrap_or_default();

        // Every top-level object of a kind with `object_names`, in parallel.
        let named = parallel::install_memory_bound_or_inline(|| {
            texts
                .par_iter()
                .filter_map(|row| {
                    let kind = *kinds.get(row.file_name.as_str())?;
                    if !has_names(kind) {
                        return None;
                    }
                    let result = raw_by_name
                        .get(row.file_name.as_str())
                        .ok_or_else(|| anyhow!("no stored row"))
                        .and_then(|raw| raw_row_text(raw))
                        .and_then(|text| parse_row(&text))
                        .and_then(|tree| crate::metadata_model::export::object_names(kind, &tree));
                    Some((kind, result))
                })
                .collect::<Vec<_>>()
        });
        let mut index = NameIndex::default();
        for (kind, result) in named {
            match result {
                Ok(names) => {
                    *report.rows_by_kind.entry(kind.to_string()).or_default() += 1;
                    index.add(&names);
                }
                Err(error) => {
                    let line = format!("{error:#}");
                    let line = line.lines().next().unwrap_or_default();
                    *report.failures.entry(format!("{kind}: {line}")).or_default() += 1;
                }
            }
        }
        if let Some(root) = root {
            let names = raw_by_name
                .get(root.file_name.as_str())
                .ok_or_else(|| anyhow!("no stored root row"))
                .and_then(|raw| raw_row_text(raw))
                .and_then(|text| parse_row(&text))
                .and_then(|tree| {
                    crate::metadata_model::export::object_names("Configuration", &tree)
                });
            match names {
                Ok(names) => {
                    *report.rows_by_kind.entry("Configuration".to_string()).or_default() += 1;
                    index.add(&names);
                }
                Err(_) => {
                    index.insert_name(&root.file_name, "Configuration");
                }
            }
        }

        // Top-level objects of the other kinds: the root row's kind and the
        // row's own name.
        for row in texts {
            if let Some(kind) = kinds.get(row.file_name.as_str())
                && !has_names(kind)
            {
                *report.legacy_rows_by_kind.entry(kind.to_string()).or_default() += 1;
                if let Some(header) = row.header.as_ref()
                    && index.insert_name(&row.file_name, &format!("{kind}.{}", header.name))
                {
                    report.legacy_names += 1;
                }
            }
        }

        // Owned objects with rows of their own (forms, templates,
        // recalculations, nested subsystems): the owner's row lists them by
        // uuid, their own row names them. Owners first, then what they own.
        let texts_by_name = texts
            .iter()
            .map(|row| (row.file_name.as_str(), row))
            .collect::<HashMap<_, _>>();
        let owned_lists = |uuid: &str| -> Vec<(&'static str, String)> {
            raw_by_name
                .get(uuid)
                .and_then(|raw| raw_row_text(raw).ok())
                .filter(|text| std::str::from_utf8(text).is_ok_and(may_own_objects))
                .and_then(|text| parse_row(&text).ok())
                .map(|tree| owned_objects(&tree))
                .unwrap_or_default()
        };
        let mut queue = parallel::install_memory_bound_or_inline(|| {
            texts
                .par_iter()
                .filter(|row| kinds.contains_key(row.file_name.as_str()))
                .map(|row| (row.file_name.as_str(), owned_lists(&row.file_name)))
                .filter(|(_, owned)| !owned.is_empty())
                .collect::<Vec<_>>()
        });
        while let Some((owner, owned)) = queue.pop() {
            let Some(owner_name) = index.name(owner).map(str::to_string) else {
                continue;
            };
            for (kind, uuid) in owned {
                let Some(row) = texts_by_name.get(uuid.as_str()) else {
                    continue;
                };
                let Some(header) = row.header.as_ref() else {
                    continue;
                };
                index.set_name(&uuid, &format!("{owner_name}.{kind}.{}", header.name));
                report.owned_names += 1;
                if kind == "Subsystem" {
                    let nested = owned_lists(&row.file_name);
                    if !nested.is_empty() {
                        queue.push((row.file_name.as_str(), nested));
                    }
                }
            }
        }
        // What the owner lists did not name, from the legacy path indexes.
        for (uuid, form_ref) in legacy.form_refs {
            if let Some(name) = form_source_reference_name(form_ref)
                && index.insert_name(uuid, &name)
            {
                report.legacy_owned_names += 1;
            }
        }
        for (uuid, template_ref) in legacy.template_refs {
            if let Some(name) = template_source_reference_name(template_ref)
                && index.insert_name(uuid, &name)
            {
                report.legacy_owned_names += 1;
            }
        }

        // What else the kinds without `object_names` name (their children,
        // nested subsystems, recalculations, generated types): the legacy
        // object references and type index.
        for (uuid, name) in legacy.object_refs {
            if root_kind(name) == "Configuration" {
                continue;
            }
            if index.insert_name(uuid, name) {
                report.legacy_names += 1;
            }
        }
        for (type_id, name) in legacy.type_index {
            let name = name.strip_prefix("cfg:").unwrap_or(name);
            if index.insert_type(type_id, name) {
                report.legacy_types += 1;
            }
        }

        // Predefined items, from the bodies, under their owner's full name.
        let items = parallel::install_memory_bound_or_inline(|| {
            predefined_rows
                .par_iter()
                .filter_map(|row| {
                    let (owner, suffix) = row.file_name.split_once('.')?;
                    let kind = *kinds.get(owner)?;
                    (predefined_suffix(kind) == Some(suffix)).then_some(())?;
                    let text = raw_row_text(row).ok()?;
                    if text.is_empty() {
                        return None;
                    }
                    let body = parse_row(&text).ok()?;
                    Some((owner, predefined_items(kind, &body).ok()?))
                })
                .collect::<Vec<_>>()
        });
        report.predefined_bodies = items.len();
        for (owner, owner_items) in items {
            let Some(owner) = index.name(owner).map(str::to_string) else {
                continue;
            };
            for (uuid, name) in owner_items {
                index.insert_predefined(&owner, &uuid, &name);
            }
        }

        let (names, types, predefined) = index.sizes();
        report.names = names;
        report.types = types;
        report.predefined = predefined;
        Ok(Self {
            context: ExportContext {
                names: index,
                version,
                compat,
            },
            kinds,
            root: root.map(|row| row.file_name.clone()),
            report,
        })
    }

    pub(super) fn names(&self) -> &NameIndex {
        &self.context.names
    }

    /// The descriptor XML of a top-level object's row through the model, or
    /// `None` when its kind has no decoder: the legacy converter writes it
    /// then.
    pub(super) fn export_row(
        &self,
        row: &MetadataTextRow,
        stored: &[u8],
    ) -> Option<Result<ExtractedMetadataSourceXml>> {
        let is_root = self.root.as_deref() == Some(row.file_name.as_str());
        let kind = if is_root {
            "Configuration"
        } else {
            *self.kinds.get(row.file_name.as_str())?
        };
        if !has_decoder(kind, &self.context) {
            return None;
        }
        let folder = if is_root { None } else { Some(kind_folder(kind)?) };
        Some((|| {
            let text = inflate_raw_deflate(stored)?;
            let tree = parse_row(&text)?;
            let object = decode_object(kind, &tree, &self.context)?;
            let xml = write_document(&object, &self.context.version);
            let relative_path = match &folder {
                None => PathBuf::from("Configuration.xml"),
                Some(folder) => {
                    let name = match row.header.as_ref() {
                        Some(header) => header.name.clone(),
                        None => object
                            .path(&["Properties", "Name"])
                            .map(|name| name.text.clone())
                            .ok_or_else(|| anyhow!("{kind} without a name"))?,
                    };
                    PathBuf::from(folder)
                        .join(sanitize_source_path_segment(&name))
                        .with_extension("xml")
                }
            };
            Ok(ExtractedMetadataSourceXml {
                relative_path,
                xml: xml.into_bytes(),
            })
        })())
    }
}

/// The predefined-data bodies an index needs: `<owner>.<suffix>` of every
/// top-level object whose kind stores predefined items.
pub(super) fn predefined_body_file_names(
    metadata_rows: &[ConfigRow],
    texts: &[MetadataTextRow],
) -> BTreeSet<String> {
    let kinds = root_text_row(texts)
        .and_then(|row| {
            metadata_rows
                .iter()
                .find(|raw| raw.file_name == row.file_name)
        })
        .and_then(|raw| raw_row_text(raw).ok())
        .and_then(|text| parse_row(&text).ok())
        .map(|tree| root_kinds(&tree))
        .unwrap_or_default();
    kinds
        .iter()
        .filter_map(|(uuid, kind)| Some(format!("{uuid}.{}", predefined_suffix(kind)?)))
        .collect()
}

// ---------------------------------------------------------------------------
// The audit: the row-built index against the tree's.

#[derive(Debug, Serialize)]
pub struct NameIndexAudit {
    pub tree_compatibility_mode: Option<String>,
    pub rows_compatibility_mode: Option<String>,
    pub rows: serde_json::Value,
    pub comparison: IndexComparison,
    pub equal: bool,
    pub build_rows_ms: u64,
    pub build_tree_ms: u64,
    /// Of `build_rows_ms`: reading the rows, the legacy indexes, the model's
    /// own index.
    pub fetch_ms: u64,
    pub legacy_ms: u64,
    pub model_ms: u64,
}

/// Builds the index from a folder of stored rows the way the export does and
/// compares it with the one the source tree gives.
pub fn audit_name_index(
    tree: &Path,
    rows_dir: &Path,
    source_version: InfobaseConfigSourceVersion,
    max_samples: usize,
) -> Result<NameIndexAudit> {
    let _offline = offline_rows::activate(rows_dir)?;
    let started = Instant::now();
    let none = Path::new("");
    let metadata_rows = fetch_metadata_rows_bcp(none, none, "", None, None, "", "Config")?;
    let texts = build_metadata_text_rows_audited(&metadata_rows).rows;
    let fetch_ms = elapsed_ms(started);
    let legacy_started = Instant::now();
    let object_refs = build_metadata_object_reference_indexes_from_texts(&texts).references;
    let type_index = build_metadata_type_indexes_from_texts(&texts).references;
    let form_refs = build_complete_form_source_reference_index(&texts);
    let template_refs = build_template_source_reference_index_from_texts(&metadata_rows, &texts);
    let legacy_ms = elapsed_ms(legacy_started);
    let model_started = Instant::now();
    let predefined_names = predefined_body_file_names(&metadata_rows, &texts);
    let predefined_rows = fetch_config_rows_bcp(
        none,
        none,
        "",
        None,
        None,
        "",
        "Config",
        &predefined_names,
    )?;
    let model = ModelExport::build(
        &metadata_rows,
        &texts,
        &predefined_rows,
        &LegacyNames {
            object_refs: &object_refs,
            type_index: &type_index,
            form_refs: &form_refs,
            template_refs: &template_refs,
        },
        source_version,
    )?;
    let model_ms = elapsed_ms(model_started);
    let build_rows_ms = elapsed_ms(started);
    let started = Instant::now();
    let config_index = ConfigIndex::build(tree)?;
    let expected = NameIndex::from_config_index(&config_index);
    let build_tree_ms = elapsed_ms(started);
    let comparison = compare(&expected, model.names(), max_samples);
    Ok(NameIndexAudit {
        tree_compatibility_mode: config_index.compatibility_mode.clone(),
        rows_compatibility_mode: model.report.compatibility_mode.clone(),
        rows: serde_json::to_value(&model.report)?,
        equal: comparison.is_equal(),
        comparison,
        build_rows_ms,
        build_tree_ms,
        fetch_ms,
        legacy_ms,
        model_ms,
    })
}

impl ModelExport {
    /// One line on stderr: what the index holds and where it came from.
    pub(super) fn log_summary(&self, elapsed_ms: u64) {
        let report = &self.report;
        let rows = report.rows_by_kind.values().sum::<usize>();
        let failed = report.failures.values().sum::<usize>();
        eprintln!(
            "model export: name index of {} names, {} types, {} predefined items in {elapsed_ms} ms; \
             object_names read {rows} rows ({failed} failed) of {} kinds, the legacy indexes gave {} names and {} types",
            report.names,
            report.types,
            report.predefined,
            report.rows_by_kind.len(),
            report.legacy_names,
            report.legacy_types,
        );
        for (failure, count) in report.failures.iter().take(10) {
            eprintln!("model export: object_names failed {count}x: {failure}");
        }
    }
}

/// A row of a modelled kind the model could not write: said once per row on
/// stderr (the first twenty), and the legacy converter writes it.
pub(super) fn report_fallback(file_name: &str, error: &anyhow::Error) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static REPORTED: AtomicUsize = AtomicUsize::new(0);
    if REPORTED.fetch_add(1, Ordering::Relaxed) < 20 {
        let text = format!("{error:#}");
        eprintln!(
            "model export: {file_name}: {}; the legacy converter writes it",
            text.lines().next().unwrap_or_default()
        );
    }
}
