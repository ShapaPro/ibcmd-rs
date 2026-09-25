//! The export direction of the descriptor model: a stored Config row ->
//! the object's model -> the exact XML file the native exporter writes.
//!
//! The model is the object's XML DOM (`xml::Element`): the same tree the
//! load direction compiles from. A kind's `decode` walks its row layout
//! backwards into that tree, `write_document` prints it with the platform's
//! layout (BOM, CRLF, tabs, self-closed empty elements), and compiling the
//! decoded tree again must give the stored row back -- the lossless check.
//!
//! References are resolved while decoding, through a [`NameIndex`] (uuid ->
//! full name, type id -> generated type name, predefined item -> name). For
//! now it is the reverse of the load direction's `ConfigIndex`, built from an
//! XML tree; each kind's [`object_names`] says what the object contributes to
//! such an index from its row alone, so the index can later be built from
//! the rows.

#[path = "export_audit.rs"]
pub mod audit;
#[path = "export_values.rs"]
pub mod values;

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Result, anyhow, bail};

use super::brace::{Brace, NIL_UUID, parse_row};
use super::index::ConfigIndex;
use super::objects::parts::Compat;
use super::xml::{Element, parse_element_tree};

/// Names by uuid: what a decoder needs to spell references.
#[derive(Debug, Default)]
pub struct NameIndex {
    /// Objects and child objects: uuid -> full name (`Catalog.X`,
    /// `Catalog.X.Form.F`, `Catalog.X.TabularSection.T.Attribute.A`).
    names: HashMap<String, String>,
    /// Generated types: type id -> name (`CatalogRef.X`, `DefinedType.Y`).
    types: HashMap<String, String>,
    /// Predefined items of catalogs and charts: item uuid -> item name.
    predefined: HashMap<String, String>,
}

impl NameIndex {
    /// The reverse of the load direction's index, plus the predefined items
    /// of every `Ext/Predefined.xml` of the tree.
    pub fn from_config_index(index: &ConfigIndex) -> Self {
        let mut names = HashMap::with_capacity(index.objects.len() + index.children.len());
        for (full_name, entry) in &index.objects {
            names.insert(entry.uuid.clone(), full_name.clone());
        }
        for (full_name, uuid) in &index.children {
            names.entry(uuid.clone()).or_insert_with(|| full_name.clone());
        }
        let mut types = HashMap::with_capacity(index.generated_types.len());
        for generated in index.generated_types.values() {
            types.insert(generated.type_id.clone(), generated.name.clone());
        }
        let mut predefined = HashMap::new();
        for entry in index.objects.values() {
            if !matches!(
                entry.kind.as_str(),
                "Catalog" | "ChartOfCharacteristicTypes" | "ChartOfAccounts" | "ChartOfCalculationTypes"
            ) {
                continue;
            }
            let path = entry.path.with_extension("").join("Ext").join("Predefined.xml");
            if let Ok(bytes) = fs::read(&path)
                && let Ok(root) = parse_element_tree(&bytes)
            {
                collect_predefined(&root, &mut predefined);
            }
        }
        Self {
            names,
            types,
            predefined,
        }
    }

    /// Full name of an object or child object.
    pub fn name(&self, uuid: &str) -> Option<&str> {
        self.names.get(uuid).map(String::as_str)
    }

    /// Name of a generated type (`CatalogRef.X`) by its type id.
    pub fn type_name(&self, type_id: &str) -> Option<&str> {
        self.types.get(type_id).map(String::as_str)
    }

    /// Name of a predefined item by its uuid.
    pub fn predefined(&self, uuid: &str) -> Option<&str> {
        self.predefined.get(uuid).map(String::as_str)
    }

    /// Adds what one object contributes (for an index built from rows).
    pub fn add(&mut self, names: &ObjectNames) {
        self.names.insert(names.uuid.clone(), names.full_name.clone());
        for (full_name, uuid) in &names.children {
            self.names.entry(uuid.clone()).or_insert_with(|| full_name.clone());
        }
        for generated in &names.types {
            self.types
                .insert(generated.type_id.clone(), generated.name.clone());
        }
    }
}

fn collect_predefined(element: &Element, out: &mut HashMap<String, String>) {
    for item in &element.children {
        if item.name == "Item"
            && let (Some(id), Some(name)) = (item.attr("id"), item.child_text("Name"))
        {
            out.insert(id.to_ascii_lowercase(), name.to_string());
        }
        collect_predefined(item, out);
    }
}

/// What a decoder may read besides the row.
pub struct ExportContext {
    pub names: NameIndex,
    /// XML dialect to write: `2.20` (8.3.27) or `2.21` (8.5).
    pub version: String,
    /// The configuration's compatibility mode: it decides the record
    /// versions of the rows.
    pub(crate) compat: Compat,
}

impl ExportContext {
    pub fn is_v85(&self) -> bool {
        self.version != "2.20"
    }
}

/// A generated type an object declares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedTypeName {
    pub name: String,
    pub category: String,
    pub type_id: String,
    pub value_id: String,
}

/// What one object contributes to a name index, read from its row alone.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ObjectNames {
    pub uuid: String,
    pub full_name: String,
    /// Child objects inside the row (attributes, tabular sections and their
    /// attributes, commands, enum values...): (full name, uuid).
    pub children: Vec<(String, String)>,
    pub types: Vec<GeneratedTypeName>,
}

/// Decodes one stored row into the object's model (its XML element).
pub fn decode_object(kind: &str, row: &Brace, context: &ExportContext) -> Result<Element> {
    match kind {
        "Catalog"
        | "Document"
        | "ExchangePlan"
        | "ChartOfCharacteristicTypes"
        | "ChartOfAccounts"
        | "ChartOfCalculationTypes"
        | "BusinessProcess"
        | "Task"
        | "Report"
        | "DataProcessor"
        | "Enum" => super::objects::export::decode(kind, row, context),
        other => bail!("not yet: no row decoder for {other}"),
    }
}

/// What one stored row contributes to a name index.
pub fn object_names(kind: &str, row: &Brace) -> Result<ObjectNames> {
    match kind {
        "Catalog"
        | "Document"
        | "ExchangePlan"
        | "ChartOfCharacteristicTypes"
        | "ChartOfAccounts"
        | "ChartOfCalculationTypes"
        | "BusinessProcess"
        | "Task"
        | "Report"
        | "DataProcessor"
        | "Enum" => super::objects::export::names(kind, row),
        other => bail!("not yet: no row decoder for {other}"),
    }
}

/// A stored row (inflated, BOM optional) -> the XML file text.
pub fn export_descriptor(kind: &str, row: &[u8], context: &ExportContext) -> Result<String> {
    let tree = parse_row(row)?;
    let object = decode_object(kind, &tree, context)?;
    Ok(write_document(&object, &context.version))
}

// ---------------------------------------------------------------------------
// The DOM builder the decoders use.

/// An empty element by qualified name (`Properties`, `xr:Field`).
pub(crate) fn el(qname: &str) -> Element {
    let (prefix, name) = match qname.split_once(':') {
        Some((prefix, name)) => (prefix, name),
        None => ("", qname),
    };
    Element {
        name: name.to_string(),
        prefix: prefix.to_string(),
        ..Element::default()
    }
}

/// An element holding text.
pub(crate) fn leaf(qname: &str, text: impl Into<String>) -> Element {
    let mut element = el(qname);
    element.text = text.into();
    element
}

/// Builder methods on the DOM element.
pub(crate) trait Build: Sized {
    fn attr(self, key: &str, value: impl Into<String>) -> Self;
    fn child(self, child: Element) -> Self;
    fn children(self, children: impl IntoIterator<Item = Element>) -> Self;
}

impl Build for Element {
    fn attr(mut self, key: &str, value: impl Into<String>) -> Self {
        self.attrs.push((key.to_string(), value.into()));
        self
    }
    fn child(mut self, child: Element) -> Self {
        self.children.push(child);
        self
    }
    fn children(mut self, children: impl IntoIterator<Item = Element>) -> Self {
        self.children.extend(children);
        self
    }
}

// ---------------------------------------------------------------------------
// Row access with errors instead of panics.

pub(crate) fn list(node: &Brace) -> Result<&[Brace]> {
    node.as_list()
        .ok_or_else(|| anyhow!("expected a list, got {}", short(node)))
}

pub(crate) fn atom(node: &Brace) -> Result<&str> {
    node.as_atom()
        .ok_or_else(|| anyhow!("expected a value, got {}", short(node)))
}

pub(crate) fn string(node: &Brace) -> Result<&str> {
    node.as_str()
        .ok_or_else(|| anyhow!("expected a string, got {}", short(node)))
}

pub(crate) fn number(node: &Brace) -> Result<i64> {
    let text = atom(node)?;
    text.parse()
        .map_err(|_| anyhow!("expected a number, got {text}"))
}

pub(crate) fn item(items: &[Brace], index: usize) -> Result<&Brace> {
    items
        .get(index)
        .ok_or_else(|| anyhow!("list too short: no member {index} of {}", items.len()))
}

pub(crate) fn is_nil(uuid: &str) -> bool {
    uuid == NIL_UUID
}

/// A short printable excerpt of a node, for errors.
pub(crate) fn short(node: &Brace) -> String {
    let text = super::brace::serialize(node).replace("\r\n", "");
    if text.chars().count() > 80 {
        format!("{}...", text.chars().take(80).collect::<String>())
    } else {
        text
    }
}

/// Stored text -> XML text: the rows keep CRLF, the XML writes LF.
pub(crate) fn xml_text(text: &str) -> String {
    if text.contains('\r') {
        text.replace("\r\n", "\n")
    } else {
        text.to_string()
    }
}

// ---------------------------------------------------------------------------
// The writer.

/// Namespaces the root element declares, in the order the platform writes
/// them; 2.21 adds the palette.
fn root_namespaces(version: &str) -> &'static str {
    if version == "2.20" {
        r#"xmlns="http://v8.1c.ru/8.3/MDClasses" xmlns:app="http://v8.1c.ru/8.2/managed-application/core" xmlns:cfg="http://v8.1c.ru/8.1/data/enterprise/current-config" xmlns:cmi="http://v8.1c.ru/8.2/managed-application/cmi" xmlns:ent="http://v8.1c.ru/8.1/data/enterprise" xmlns:lf="http://v8.1c.ru/8.2/managed-application/logform" xmlns:style="http://v8.1c.ru/8.1/data/ui/style" xmlns:sys="http://v8.1c.ru/8.1/data/ui/fonts/system" xmlns:v8="http://v8.1c.ru/8.1/data/core" xmlns:v8ui="http://v8.1c.ru/8.1/data/ui" xmlns:web="http://v8.1c.ru/8.1/data/ui/colors/web" xmlns:win="http://v8.1c.ru/8.1/data/ui/colors/windows" xmlns:xen="http://v8.1c.ru/8.3/xcf/enums" xmlns:xpr="http://v8.1c.ru/8.3/xcf/predef" xmlns:xr="http://v8.1c.ru/8.3/xcf/readable" xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance""#
    } else {
        r#"xmlns="http://v8.1c.ru/8.3/MDClasses" xmlns:app="http://v8.1c.ru/8.2/managed-application/core" xmlns:cfg="http://v8.1c.ru/8.1/data/enterprise/current-config" xmlns:cmi="http://v8.1c.ru/8.2/managed-application/cmi" xmlns:ent="http://v8.1c.ru/8.1/data/enterprise" xmlns:lf="http://v8.1c.ru/8.2/managed-application/logform" xmlns:pal="http://v8.1c.ru/8.1/data/ui/colors/palette" xmlns:style="http://v8.1c.ru/8.1/data/ui/style" xmlns:sys="http://v8.1c.ru/8.1/data/ui/fonts/system" xmlns:v8="http://v8.1c.ru/8.1/data/core" xmlns:v8ui="http://v8.1c.ru/8.1/data/ui" xmlns:web="http://v8.1c.ru/8.1/data/ui/colors/web" xmlns:win="http://v8.1c.ru/8.1/data/ui/colors/windows" xmlns:xen="http://v8.1c.ru/8.3/xcf/enums" xmlns:xpr="http://v8.1c.ru/8.3/xcf/predef" xmlns:xr="http://v8.1c.ru/8.3/xcf/readable" xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance""#
    }
}

const ROOT_PREFIXES: &[&str] = &[
    "app", "cfg", "cmi", "ent", "lf", "pal", "style", "sys", "v8", "v8ui", "web", "win", "xen",
    "xpr", "xr", "xs", "xsi",
];

/// The namespace a type value's prefix stands for when the root does not
/// declare it: the platform declares it on the element itself.
fn value_namespace(prefix: &str, local: &str) -> Option<&'static str> {
    Some(match prefix {
        "mxl" => "http://v8.1c.ru/8.2/data/spreadsheet",
        "dcsset" => "http://v8.1c.ru/8.1/data-composition-system/settings",
        "dcscor" => "http://v8.1c.ru/8.1/data-composition-system/core",
        "dcssch" => "http://v8.1c.ru/8.1/data-composition-system/schema",
        "fd" => "http://v8.1c.ru/8.2/data/formatted-document",
        "pdfdoc" => "http://v8.1c.ru/8.3/data/pdf",
        "pl" => "http://v8.1c.ru/8.3/data/planner",
        // Generated prefixes (`d<depth>p1`) name the namespace of the type.
        _ if prefix.starts_with('d') && prefix.ends_with("p1") => match local {
            "Chart" | "GanttChart" => "http://v8.1c.ru/8.2/data/chart",
            "TextDocument" => "http://v8.1c.ru/8.1/data/txtedt",
            "GeographicalSchema" => "http://v8.1c.ru/8.2/data/geo",
            "FlowchartContextType" => "http://v8.1c.ru/8.2/data/graphscheme",
            "DataAnalysisTimeIntervalUnitType" => "http://v8.1c.ru/8.2/data/data-analysis",
            "ConditionalAppearance" => "http://v8.1c.ru/8.3/data/entext",
            _ => "http://v8.1c.ru/8.1/data/enterprise/current-config",
        },
        _ => return None,
    })
}

/// The whole file: BOM, declaration, root element, the object, no newline
/// after the root's end tag.
pub fn write_document(object: &Element, version: &str) -> String {
    let mut out = String::with_capacity(16 * 1024);
    out.push('\u{feff}');
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n<MetaDataObject ");
    out.push_str(root_namespaces(version));
    out.push_str(" version=\"");
    out.push_str(version);
    out.push_str("\">\r\n");
    write_element(&mut out, object, 1);
    out.push_str("</MetaDataObject>");
    out
}

fn push_qname(out: &mut String, element: &Element) {
    if !element.prefix.is_empty() {
        out.push_str(&element.prefix);
        out.push(':');
    }
    out.push_str(&element.name);
}

fn write_element(out: &mut String, element: &Element, depth: usize) {
    for _ in 0..depth {
        out.push('\t');
    }
    out.push('<');
    push_qname(out, element);
    for (key, value) in &element.attrs {
        out.push(' ');
        match key.as_str() {
            "type" | "nil" => out.push_str("xsi:"),
            _ => {}
        }
        out.push_str(key);
        out.push_str("=\"");
        escape(out, value, true);
        out.push('"');
    }
    for (prefix, uri) in &element.namespaces {
        out.push_str(" xmlns");
        if !prefix.is_empty() {
            out.push(':');
            out.push_str(prefix);
        }
        out.push_str("=\"");
        escape(out, uri, true);
        out.push('"');
    }
    let leaf = element.children.is_empty();
    // A decoded type value whose prefix the root does not declare
    // (`mxl:...`, `d0p1:Chart`) gets its own declaration; a generated prefix
    // is the element's depth.
    let mut text: &str = &element.text;
    let rewritten;
    if leaf
        && element.namespaces.is_empty()
        && element.prefix == "v8"
        && matches!(element.name.as_str(), "Type" | "TypeSet")
        && let Some((prefix, local)) = text.split_once(':')
        && !ROOT_PREFIXES.contains(&prefix)
        && let Some(uri) = value_namespace(prefix, local)
    {
        let prefix = if prefix.starts_with('d') && prefix.ends_with("p1") {
            format!("d{}p1", depth + 1)
        } else {
            prefix.to_string()
        };
        out.push_str(" xmlns:");
        out.push_str(&prefix);
        out.push_str("=\"");
        out.push_str(uri);
        out.push('"');
        rewritten = format!("{prefix}:{local}");
        text = &rewritten;
    }
    if leaf {
        if text.is_empty() {
            out.push_str("/>\r\n");
        } else {
            out.push('>');
            escape(out, text, false);
            out.push_str("</");
            push_qname(out, element);
            out.push_str(">\r\n");
        }
        return;
    }
    out.push_str(">\r\n");
    for child in &element.children {
        write_element(out, child, depth + 1);
    }
    for _ in 0..depth {
        out.push('\t');
    }
    out.push_str("</");
    push_qname(out, element);
    out.push_str(">\r\n");
}

/// `&`, `<`, `>` everywhere; `"` in attribute values.
fn escape(out: &mut String, text: &str, attribute: bool) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attribute => out.push_str("&quot;"),
            '\r' => out.push_str("&#xD;"),
            _ => out.push(ch),
        }
    }
}

/// A parsed file's object element written back: the writer's own check.
pub fn rewrite_file(bytes: &[u8]) -> Result<String> {
    let doc = super::xml::MetadataXml::parse(bytes)?;
    let version = doc.version().unwrap_or("2.20").to_string();
    Ok(write_document(doc.object()?, &version))
}

/// The version a tree's files are written in, from its `Configuration.xml`.
pub fn tree_version(root: &Path) -> Option<String> {
    let bytes = fs::read(root.join("Configuration.xml")).ok()?;
    let doc = super::xml::MetadataXml::parse(&bytes).ok()?;
    doc.version().map(str::to_string)
}
