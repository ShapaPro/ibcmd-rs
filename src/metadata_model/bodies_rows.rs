//! The body rows track D writes for a base-free stage: predefined data
//! (Catalog `.1c`, ChartOfCharacteristicTypes `.7`, ChartOfAccounts `.9`,
//! ChartOfCalculationTypes `.2`), business process flowcharts (`.7`) and
//! accumulation register aggregates (`.3`). Each is compiled from the
//! object's XML and its `Ext` file alone; an object without that file has
//! no row.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};

use super::DescriptorContext;
use super::bodies_aggregates::{aggregates_path, aggregates_row};
use super::bodies_flowchart::{flowchart_path, flowchart_row};
use super::bodies_predefined::{predefined_path, predefined_row, predefined_suffix};
use super::brace::serialize_row;
use super::common::crlf_strings;
use super::xml::MetadataXml;

/// One body row: its Config file name, the file it was compiled from and
/// the stored text (BOM included).
#[derive(Clone, Debug)]
pub struct BodyRow {
    pub file_name: String,
    pub source: PathBuf,
    pub text: Vec<u8>,
}

/// Whether these writers replace the loader's own kind-body writer for a
/// kind (whose writer only patches a base row).
pub fn owns_kind_body(kind: &str) -> bool {
    matches!(
        kind,
        "Catalog" | "ChartOfCharacteristicTypes" | "BusinessProcess"
    )
}

/// The suffix of the row these writers give a kind, if any.
pub fn body_row_suffix(kind: &str) -> Option<&'static str> {
    match kind {
        "BusinessProcess" => Some("7"),
        "AccumulationRegister" => Some("3"),
        other => predefined_suffix(other),
    }
}

/// The rows of one object (none, or one).
pub fn compile_body_rows(
    kind: &str,
    xml_path: &Path,
    xml: &[u8],
    context: &DescriptorContext,
) -> Result<Vec<BodyRow>> {
    let Some(suffix) = body_row_suffix(kind) else {
        return Ok(Vec::new());
    };
    let doc = MetadataXml::parse(xml)?;
    let object = doc.object()?;
    let uuid = object
        .attr("uuid")
        .ok_or_else(|| anyhow!("{kind} has no uuid"))?
        .to_ascii_lowercase();
    let (tree, source) = match kind {
        "BusinessProcess" => (flowchart_row(xml_path, context)?, flowchart_path(xml_path)),
        "AccumulationRegister" => (
            aggregates_row(object, xml_path, context)?,
            aggregates_path(xml_path),
        ),
        _ => (
            predefined_row(kind, object, xml_path, context)?,
            predefined_path(xml_path),
        ),
    };
    let Some(mut tree) = tree else {
        return Ok(Vec::new());
    };
    crlf_strings(&mut tree);
    Ok(vec![BodyRow {
        file_name: format!("{uuid}.{suffix}"),
        source,
        text: serialize_row(&tree),
    }])
}
