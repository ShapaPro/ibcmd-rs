//! Offline measurement of the export direction.
//!
//! For every metadata XML of a tree the stored row is decoded into the
//! object's model, written as XML and compared byte for byte with the file.
//! Two more checks ride along: the writer alone (the parsed file written
//! back) and losslessness (the decoded model compiled back must be the
//! stored row). Nothing is read from or written to a database.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use rayon::prelude::*;
use serde::Serialize;

use super::super::audit::{descriptor_xmls, read_stored_row};
use super::super::brace::{parse_row, serialize_row};
use super::super::objects::parts::compatibility;
use super::super::{DescriptorContext, ObjectXml, compile_object};
use super::{ExportContext, NameIndex, decode_object, object_names, rewrite_file, write_document};
use crate::module_blob::parse_simple_metadata_xml_properties;
use crate::parallel;

#[derive(Debug, Clone, Default)]
pub struct ExportAuditOptions {
    /// Only these kinds; every kind when empty.
    pub kinds: Vec<String>,
    /// Differing samples kept per kind.
    pub max_samples: usize,
    /// Where the expected and the written XML of each sample go.
    pub diff_dir: Option<PathBuf>,
    /// Also time decode + write over preloaded rows with this many threads.
    pub timing_threads: Option<usize>,
}

#[derive(Debug, Default, Serialize)]
pub struct ExportAuditReport {
    pub root: String,
    pub rows: String,
    pub objects: usize,
    pub identical: usize,
    pub different: usize,
    pub failed: usize,
    pub no_row: usize,
    pub lossless: usize,
    pub lossy: usize,
    pub writer_mismatch: usize,
    pub kinds: BTreeMap<String, ExportKindStats>,
    pub timing: Option<ExportTiming>,
}

#[derive(Debug, Default, Serialize)]
pub struct ExportKindStats {
    pub total: usize,
    pub identical: usize,
    pub different: usize,
    pub failed: usize,
    pub no_row: usize,
    /// Decoded and compiled back to the stored row.
    pub lossless: usize,
    pub lossy: usize,
    /// The parsed file written back differs from the file: a writer gap.
    pub writer_mismatch: usize,
    /// Decode errors by their first line.
    pub failures: BTreeMap<String, usize>,
    /// The element of the first differing line, with counts.
    pub first_lines: BTreeMap<String, usize>,
    pub samples: Vec<ExportSample>,
}

#[derive(Debug, Serialize)]
pub struct ExportSample {
    pub path: String,
    pub uuid: String,
    pub line: usize,
    pub expected: String,
    pub actual: String,
}

#[derive(Debug, Default, Serialize)]
pub struct ExportTiming {
    pub objects: usize,
    pub threads: usize,
    /// The name index from the XML tree (`ConfigIndex` reversed).
    pub name_index_ms: u128,
    /// `object_names` over every row (the index a row set would give).
    pub names_from_rows_ms: u128,
    /// parse + decode + write, wall clock.
    pub decode_write_ms: u128,
    /// parse + decode + write, summed over the objects.
    pub decode_write_cpu_ms: u128,
    pub bytes_written: usize,
}

enum Outcome {
    Identical,
    Different {
        line: usize,
        expected: String,
        actual: String,
        written: String,
    },
    Failed(String),
    NoRow,
}

struct Measured {
    kind: String,
    path: String,
    uuid: String,
    outcome: Outcome,
    lossless: Option<bool>,
    writer_ok: bool,
}

pub fn audit_export(
    root: &Path,
    rows: &Path,
    version: &str,
    options: &ExportAuditOptions,
) -> Result<ExportAuditReport> {
    let started = Instant::now();
    let descriptor_context = DescriptorContext::new(root, version)?;
    let names = NameIndex::from_config_index(&descriptor_context.index);
    let name_index_ms = started.elapsed().as_millis();
    let context = ExportContext {
        names,
        version: version.to_string(),
        compat: compatibility(&descriptor_context),
    };
    let paths = descriptor_xmls(root);
    let measured = parallel::install(|| {
        paths
            .par_iter()
            .filter_map(|path| {
                measure_one(root, rows, path, &context, &descriptor_context, options).transpose()
            })
            .collect::<Result<Vec<_>>>()
    })??;

    let mut report = ExportAuditReport {
        root: root.display().to_string(),
        rows: rows.display().to_string(),
        ..Default::default()
    };
    for item in measured {
        let stats = report.kinds.entry(item.kind.clone()).or_default();
        stats.total += 1;
        report.objects += 1;
        if !item.writer_ok {
            stats.writer_mismatch += 1;
            report.writer_mismatch += 1;
        }
        match item.lossless {
            Some(true) => {
                stats.lossless += 1;
                report.lossless += 1;
            }
            Some(false) => {
                stats.lossy += 1;
                report.lossy += 1;
            }
            None => {}
        }
        match item.outcome {
            Outcome::Identical => {
                stats.identical += 1;
                report.identical += 1;
            }
            Outcome::NoRow => {
                stats.no_row += 1;
                report.no_row += 1;
            }
            Outcome::Failed(message) => {
                stats.failed += 1;
                report.failed += 1;
                let key = message
                    .lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take(200)
                    .collect();
                *stats.failures.entry(key).or_default() += 1;
            }
            Outcome::Different {
                line,
                expected,
                actual,
                written,
            } => {
                stats.different += 1;
                report.different += 1;
                *stats.first_lines.entry(element_of(&expected)).or_default() += 1;
                if stats.samples.len() < options.max_samples {
                    if let Some(dir) = &options.diff_dir {
                        write_sample(dir, &item.kind, &item.path, root, &written)?;
                    }
                    stats.samples.push(ExportSample {
                        path: item.path,
                        uuid: item.uuid,
                        line,
                        expected,
                        actual,
                    });
                }
            }
        }
    }
    if let Some(threads) = options.timing_threads {
        let mut timing = time_export(root, rows, &paths, &context, options, threads)?;
        timing.name_index_ms = name_index_ms;
        report.timing = Some(timing);
    }
    Ok(report)
}

fn wanted(options: &ExportAuditOptions, kind: &str) -> bool {
    options.kinds.is_empty() || options.kinds.iter().any(|candidate| candidate == kind)
}

fn measure_one(
    root: &Path,
    rows: &Path,
    path: &Path,
    context: &ExportContext,
    descriptor_context: &DescriptorContext,
    options: &ExportAuditOptions,
) -> Result<Option<Measured>> {
    let relative = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    let Ok(properties) = parse_simple_metadata_xml_properties(&bytes) else {
        return Ok(None);
    };
    if !wanted(options, &properties.kind) {
        return Ok(None);
    }
    let expected = String::from_utf8_lossy(&bytes).into_owned();
    let writer_ok = rewrite_file(&bytes)
        .map(|written| written == expected)
        .unwrap_or(false);
    let mut lossless = None;
    let outcome = match read_stored_row(rows, &properties.uuid)? {
        None => Outcome::NoRow,
        Some(stored) => {
            let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                parse_row(&stored).and_then(|tree| decode_object(&properties.kind, &tree, context))
            }));
            match decoded {
                Err(_) => Outcome::Failed("decoder panicked".to_string()),
                Ok(Err(error)) => Outcome::Failed(format!("{error:#}")),
                Ok(Ok(object)) => {
                    lossless = Some(is_lossless(&object, &properties.kind, path, &stored, descriptor_context));
                    let written = write_document(&object, &context.version);
                    if written == expected {
                        Outcome::Identical
                    } else {
                        let (line, expected_line, actual_line) = first_line_difference(&expected, &written);
                        Outcome::Different {
                            line,
                            expected: expected_line,
                            actual: actual_line,
                            written,
                        }
                    }
                }
            }
        }
    };
    Ok(Some(Measured {
        kind: properties.kind,
        path: relative,
        uuid: properties.uuid,
        outcome,
        lossless,
        writer_ok,
    }))
}

/// The decoded model compiled again gives the stored row back.
fn is_lossless(
    object: &super::super::xml::Element,
    kind: &str,
    path: &Path,
    stored: &[u8],
    context: &DescriptorContext,
) -> bool {
    let name = object
        .path(&["Properties", "Name"])
        .map(|name| name.text.clone())
        .unwrap_or_default();
    let xml = ObjectXml {
        element: object,
        kind,
        uuid: object.attr("uuid").unwrap_or_default().to_ascii_lowercase(),
        name,
        path,
    };
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        compile_object(&xml, context)
            .map(|tree| serialize_row(&tree) == stored)
            .unwrap_or(false)
    }))
    .unwrap_or(false)
}

/// (1-based line, expected line, written line) of the first difference.
fn first_line_difference(expected: &str, actual: &str) -> (usize, String, String) {
    let mut expected_lines = expected.split("\r\n");
    let mut actual_lines = actual.split("\r\n");
    let mut number = 0;
    loop {
        number += 1;
        match (expected_lines.next(), actual_lines.next()) {
            (Some(left), Some(right)) if left == right => continue,
            (left, right) => {
                return (
                    number,
                    left.unwrap_or("<end>").trim().chars().take(300).collect(),
                    right.unwrap_or("<end>").trim().chars().take(300).collect(),
                );
            }
        }
    }
}

/// `<xr:Field>...` -> `xr:Field`: the grouping key of a differing line.
fn element_of(line: &str) -> String {
    let line = line.trim();
    let Some(rest) = line.strip_prefix('<') else {
        return line.chars().take(40).collect();
    };
    rest.chars()
        .take_while(|ch| !matches!(ch, ' ' | '>' | '/'))
        .collect::<String>()
        .trim_start_matches('/')
        .to_string()
}

fn write_sample(dir: &Path, kind: &str, relative: &str, root: &Path, written: &str) -> Result<()> {
    let folder = dir.join(kind);
    fs::create_dir_all(&folder)?;
    let stem = relative.trim_end_matches(".xml").replace(['/', '\\'], "__");
    fs::copy(root.join(relative), folder.join(format!("{stem}.expected.xml")))?;
    fs::write(folder.join(format!("{stem}.actual.xml")), written)?;
    Ok(())
}

/// decode + write over preloaded rows, on a pool of `threads`.
fn time_export(
    root: &Path,
    rows: &Path,
    paths: &[PathBuf],
    context: &ExportContext,
    options: &ExportAuditOptions,
    threads: usize,
) -> Result<ExportTiming> {
    let mut loaded = Vec::new();
    for path in paths {
        let bytes = fs::read(path)?;
        let Ok(properties) = parse_simple_metadata_xml_properties(&bytes) else {
            continue;
        };
        if !wanted(options, &properties.kind) {
            continue;
        }
        if let Some(stored) = read_stored_row(rows, &properties.uuid)? {
            loaded.push((properties.kind, stored));
        }
    }
    let _ = root;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .context("failed to build the timing pool")?;
    let names_started = Instant::now();
    let names_ok = pool.install(|| {
        loaded
            .par_iter()
            .filter(|(kind, stored)| {
                parse_row(stored)
                    .and_then(|tree| object_names(kind, &tree))
                    .is_ok()
            })
            .count()
    });
    let names_from_rows_ms = names_started.elapsed().as_millis();
    let started = Instant::now();
    let results = pool.install(|| {
        loaded
            .par_iter()
            .map(|(kind, stored)| {
                let one = Instant::now();
                let written = parse_row(stored)
                    .and_then(|tree| decode_object(kind, &tree, context))
                    .map(|object| write_document(&object, &context.version).len())
                    .unwrap_or(0);
                (one.elapsed().as_micros(), written)
            })
            .collect::<Vec<_>>()
    });
    let decode_write_ms = started.elapsed().as_millis();
    let _ = names_ok;
    Ok(ExportTiming {
        objects: loaded.len(),
        threads,
        name_index_ms: 0,
        names_from_rows_ms,
        decode_write_ms,
        decode_write_cpu_ms: results.iter().map(|(micros, _)| micros).sum::<u128>() / 1000,
        bytes_written: results.iter().map(|(_, bytes)| bytes).sum(),
    })
}

/// One line per kind, for the terminal.
pub fn summary_table(report: &ExportAuditReport) -> String {
    let mut lines = vec![format!(
        "{:<30} {:>6} {:>9} {:>9} {:>7} {:>6} {:>8} {:>6} {:>7}",
        "kind", "total", "identical", "different", "failed", "no_row", "lossless", "lossy", "writer!"
    )];
    for (kind, stats) in &report.kinds {
        lines.push(format!(
            "{:<30} {:>6} {:>9} {:>9} {:>7} {:>6} {:>8} {:>6} {:>7}",
            kind,
            stats.total,
            stats.identical,
            stats.different,
            stats.failed,
            stats.no_row,
            stats.lossless,
            stats.lossy,
            stats.writer_mismatch
        ));
    }
    lines.push(format!(
        "{:<30} {:>6} {:>9} {:>9} {:>7} {:>6} {:>8} {:>6} {:>7}",
        "TOTAL",
        report.objects,
        report.identical,
        report.different,
        report.failed,
        report.no_row,
        report.lossless,
        report.lossy,
        report.writer_mismatch
    ));
    if let Some(timing) = &report.timing {
        lines.push(format!(
            "timing: {} objects, {} threads: name index {} ms, names from rows {} ms, decode+write {} ms wall ({} ms cpu), {} bytes",
            timing.objects,
            timing.threads,
            timing.name_index_ms,
            timing.names_from_rows_ms,
            timing.decode_write_ms,
            timing.decode_write_cpu_ms,
            timing.bytes_written
        ));
    }
    lines.join("\n")
}
