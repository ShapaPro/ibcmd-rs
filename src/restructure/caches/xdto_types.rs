//! `ea13a2c9-0c2f-40fa-b855-710387e3271d.si`: the XDTO model, for a new catalog and a new tabular section.
//!
//! The envelope is the one of [`crate::restructure::xdto`] (`{2,1,{{#base64:...}}}`, 64 characters a
//! line, `CR CR LF`); this module adds the *types* of a new object to the model XML. The XML is
//! generated in the metadata order, so a type stands where the root lists its object:
//!
//! - a **catalog** `X` adds `<valueType name="CatalogRef.X" base="d3p1:AnyDBRef"/>` after the one of the
//!   catalog the root lists before it, and a block after the block of that catalog (its tabular
//!   sections' row types, then `CatalogObject.X`);
//! - a **tabular section** `T` of catalog `X` adds `CatalogTabularSectionRow.X.T` after the row types
//!   of the sections before it (right before `CatalogObject.X` when it is the last) and the
//!   property `T` to `CatalogObject.X` after the property of the section before it (after the last
//!   property when it is the first).
//!
//! The standard properties of `CatalogObject.X` follow the object's flags -- `IsFolder` (a hierarchy of
//! folders and items), `Ref`, `DeletionMark`, `Owner` (the subordinate catalogs), `Parent` (hierarchical),
//! `Code` (a code length; `xs:decimal` for a numeric code), `Description`, `PredefinedDataName` -- and
//! are checked against all 114 catalogs of the БСП corpus (`tests_corpus.rs`).

use anyhow::{Context, Result, bail};

use crate::module_blob::{decode_base64_mime, encode_base64};
use crate::restructure::names::{deflate, inflate};

const PREFIX: &str = "{2,1,\r\n{\r\n{#base64:";
const SUFFIX: &str = "}\r\n}\r\n}";
const LINE: usize = 64;
const SEPARATOR: &str = "\r\r\n";

const CURRENT_CONFIG: &str = "http://v8.1c.ru/8.1/data/enterprise/current-config";
const ENTERPRISE: &str = "http://v8.1c.ru/8.1/data/enterprise";

/// The decoded model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XdtoModel {
    bom: bool,
    /// The XML text without BOM, CRLF line ends.
    pub xml: String,
}

/// What decides the standard properties of `CatalogObject.X`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CatalogShape {
    pub hierarchical: bool,
    /// 0 folders and items, 1 items only.
    pub hierarchy_type: i64,
    pub owners: usize,
    pub code_length: i64,
    /// 0 number, 1 string.
    pub code_type: i64,
    pub description_length: i64,
}

impl XdtoModel {
    /// Reads an inflated row.
    pub fn parse(row_text: &[u8]) -> Result<Self> {
        let (bom, text) = match row_text.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
            Some(rest) => (true, rest),
            None => (false, row_text),
        };
        let text = std::str::from_utf8(text).context("the model row is not UTF-8")?;
        let body = text
            .strip_prefix(PREFIX)
            .and_then(|rest| rest.strip_suffix(SUFFIX))
            .context("the row is not an XDTO model ({2,1,{{#base64:...}}})")?;
        let stripped: String = body.split(SEPARATOR).collect();
        let bytes = decode_base64_mime(&stripped).context("the model is not base64")?;
        let xml = String::from_utf8(bytes).context("the model XML is not UTF-8")?;
        let xml = xml.strip_prefix('\u{feff}').unwrap_or(&xml).to_owned();
        Ok(Self { bom, xml })
    }

    /// The inflated row text.
    pub fn render(&self) -> Vec<u8> {
        let mut xml = Vec::with_capacity(self.xml.len() + 3);
        xml.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
        xml.extend_from_slice(self.xml.as_bytes());
        let encoded = encode_base64(&xml);
        let mut text = String::with_capacity(encoded.len() + encoded.len() / LINE * 3 + 64);
        text.push_str(PREFIX);
        let mut lines = encoded.as_bytes().chunks(LINE).peekable();
        while let Some(line) = lines.next() {
            text.push_str(std::str::from_utf8(line).unwrap_or_default());
            if lines.peek().is_some() {
                text.push_str(SEPARATOR);
            }
        }
        text.push_str(SUFFIX);
        let mut out = Vec::with_capacity(text.len() + 3);
        if self.bom {
            out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
        }
        out.extend_from_slice(text.as_bytes());
        out
    }

    /// The stored (deflated) row.
    pub fn to_stored(&self) -> Result<Vec<u8>> {
        deflate(&self.render())
    }

    pub fn from_stored(stored: &[u8]) -> Result<Self> {
        Self::parse(&inflate(stored)?)
    }

    // -----------------------------------------------------------------------------------------
    // finding things
    // -----------------------------------------------------------------------------------------

    /// The byte range of the line that starts at or contains `at` (with its CRLF).
    fn line_end(&self, at: usize) -> Result<usize> {
        self.xml[at..]
            .find("\r\n")
            .map(|offset| at + offset + 2)
            .context("an unterminated line in the model")
    }

    fn value_type_line(&self, name: &str) -> Option<(usize, usize)> {
        let needle = format!("<valueType xmlns:d3p1=\"{ENTERPRISE}\" name=\"CatalogRef.{name}\"");
        let at = self.xml.find(&needle)?;
        let start = self.xml[..at].rfind('\n').map_or(0, |n| n + 1);
        Some((start, self.line_end(at).ok()?))
    }

    /// The range of `<objectType name="<name>">...</objectType>` including its indent and CRLF.
    fn object_type(&self, name: &str) -> Option<(usize, usize)> {
        let needle = format!("\t\t<objectType name=\"{name}\">\r\n");
        let start = self.xml.find(&needle)?;
        let close = self.xml[start..].find("\t\t</objectType>\r\n")? + start;
        Some((start, close + "\t\t</objectType>\r\n".len()))
    }

    // -----------------------------------------------------------------------------------------
    // a catalog
    // -----------------------------------------------------------------------------------------

    /// Adds the types of a new catalog. `predecessor` / `successor` are the catalogs the root lists
    /// before and after it. `attribute_lines` are the property lines of its attributes (and of its
    /// tabular sections' properties) in order; the row types of the sections come in
    /// `section_blocks`.
    pub fn add_catalog(
        &mut self,
        name: &str,
        shape: CatalogShape,
        predecessor: Option<&str>,
        successor: Option<&str>,
        attribute_lines: &[String],
        section_blocks: &[String],
    ) -> Result<()> {
        if self.value_type_line(name).is_some()
            || self.object_type(&format!("CatalogObject.{name}")).is_some()
        {
            bail!("the model has the types of catalog {name} already");
        }
        // the block: row types, then the object
        let mut block = String::new();
        for section in section_blocks {
            block.push_str(section);
        }
        block.push_str(&format!(
            "\t\t<objectType name=\"CatalogObject.{name}\">\r\n"
        ));
        for line in standard_properties(name, shape) {
            block.push_str(&line);
        }
        for line in attribute_lines {
            block.push_str(line);
        }
        block.push_str("\t\t</objectType>\r\n");
        let value_type = format!(
            "\t\t<valueType xmlns:d3p1=\"{ENTERPRISE}\" name=\"CatalogRef.{name}\" base=\"d3p1:AnyDBRef\"/>\r\n"
        );

        // positions are computed on the unchanged text; insert the later one first
        let block_at = self.block_position(predecessor, successor)?;
        let value_at = if let Some(predecessor) = predecessor {
            self.value_type_line(predecessor)
                .with_context(|| format!("the model has no CatalogRef.{predecessor}"))?
                .1
        } else {
            let successor = successor.context("the new catalog has neither neighbour")?;
            self.value_type_line(successor)
                .with_context(|| format!("the model has no CatalogRef.{successor}"))?
                .0
        };
        if value_at > block_at {
            bail!("the value types come after the object types in the model");
        }
        self.xml.insert_str(block_at, &block);
        self.xml.insert_str(value_at, &value_type);
        Ok(())
    }

    /// Where the block of a new catalog goes: after the block of the predecessor (the end of its
    /// `CatalogObject`), or before the first type of the successor's block.
    fn block_position(&self, predecessor: Option<&str>, successor: Option<&str>) -> Result<usize> {
        if let Some(predecessor) = predecessor {
            let (_, end) = self
                .object_type(&format!("CatalogObject.{predecessor}"))
                .with_context(|| format!("the model has no CatalogObject.{predecessor}"))?;
            return Ok(end);
        }
        let successor = successor.context("the new catalog has neither neighbour")?;
        let (mut start, _) = self
            .object_type(&format!("CatalogObject.{successor}"))
            .with_context(|| format!("the model has no CatalogObject.{successor}"))?;
        // step back over the row types of the successor's sections
        let prefix = format!("\t\t<objectType name=\"CatalogTabularSectionRow.{successor}.");
        loop {
            let before = &self.xml[..start];
            let Some(previous) = before.rfind("\t\t<objectType name=\"") else {
                break;
            };
            if !self.xml[previous..].starts_with(&prefix) {
                break;
            }
            start = previous;
        }
        Ok(start)
    }

    // -----------------------------------------------------------------------------------------
    // a tabular section
    // -----------------------------------------------------------------------------------------

    /// Adds a tabular section `section` to `catalog`. `after` is the section the root lists before
    /// it (`None`: it is the first). `row_lines` are the property lines of the section's attributes.
    pub fn add_tabular_section(
        &mut self,
        catalog: &str,
        section: &str,
        after: Option<&str>,
        row_lines: &[String],
    ) -> Result<()> {
        let row_name = format!("CatalogTabularSectionRow.{catalog}.{section}");
        let object_name = format!("CatalogObject.{catalog}");
        if self.object_type(&row_name).is_some() {
            bail!("the model has {row_name} already");
        }
        if after.is_none()
            && self.xml.contains(&format!(
                "<objectType name=\"CatalogTabularSectionRow.{catalog}."
            ))
        {
            bail!(
                "{catalog} has tabular sections in the model: say which one the new section follows"
            );
        }
        let (object_start, object_end) = self
            .object_type(&object_name)
            .with_context(|| format!("the model has no {object_name}"))?;

        // the property of the section in the object
        let property = format!(
            "\t\t\t<property xmlns:d4p1=\"{CURRENT_CONFIG}\" name=\"{section}\" type=\"d4p1:{row_name}\" lowerBound=\"0\" upperBound=\"99999\"/>\r\n"
        );
        let property_at = if let Some(after) = after {
            let anchor = format!(
                "name=\"{after}\" type=\"d4p1:CatalogTabularSectionRow.{catalog}.{after}\""
            );
            let at = self.xml[object_start..object_end]
                .find(&anchor)
                .with_context(|| format!("{object_name} has no property {after}"))?
                + object_start;
            self.line_end(at)?
        } else {
            object_end - "\t\t</objectType>\r\n".len()
        };

        // the row type block
        let mut block = format!("\t\t<objectType name=\"{row_name}\">\r\n");
        for line in row_lines {
            block.push_str(line);
        }
        block.push_str("\t\t</objectType>\r\n");
        let block_at = if let Some(after) = after {
            self.object_type(&format!("CatalogTabularSectionRow.{catalog}.{after}"))
                .with_context(|| {
                    format!("the model has no CatalogTabularSectionRow.{catalog}.{after}")
                })?
                .1
        } else {
            // before the object's own block, after the sections of nothing
            object_start
        };
        if block_at > property_at {
            bail!("the row type of a section stands after its owner in the model");
        }
        self.xml.insert_str(property_at, &property);
        self.xml.insert_str(block_at, &block);
        Ok(())
    }
}

/// `<property name="<name>" type="<type>"[ lowerBound="0"]/>` at the indent of an object type.
pub fn primitive_property(name: &str, xsd_type: &str, lower_bound_zero: bool) -> String {
    let lower = if lower_bound_zero {
        " lowerBound=\"0\""
    } else {
        ""
    };
    format!("\t\t\t<property name=\"{name}\" type=\"{xsd_type}\"{lower}/>\r\n")
}

/// A property of a type of the configuration (`CatalogRef.X`, `EnumRef.Y`).
pub fn reference_property(name: &str, type_name: &str, lower_bound_zero: bool) -> String {
    let lower = if lower_bound_zero {
        " lowerBound=\"0\""
    } else {
        ""
    };
    format!(
        "\t\t\t<property xmlns:d4p1=\"{CURRENT_CONFIG}\" name=\"{name}\" type=\"d4p1:{type_name}\"{lower}/>\r\n"
    )
}

/// The standard property lines of `CatalogObject.<name>`.
pub fn standard_properties(name: &str, shape: CatalogShape) -> Vec<String> {
    let reference =
        |property: &str| reference_property(property, &format!("CatalogRef.{name}"), false);
    let mut lines = Vec::new();
    if shape.hierarchical && shape.hierarchy_type == 0 {
        lines.push(primitive_property("IsFolder", "xs:boolean", false));
    }
    lines.push(reference("Ref"));
    lines.push(primitive_property("DeletionMark", "xs:boolean", false));
    if shape.owners > 0 {
        lines.push(format!(
            "\t\t\t<property xmlns:d4p1=\"{ENTERPRISE}\" name=\"Owner\" type=\"d4p1:AnyIBRef\" nillable=\"true\"/>\r\n"
        ));
    }
    if shape.hierarchical {
        lines.push(reference("Parent"));
    }
    if shape.code_length > 0 {
        let code_type = if shape.code_type == 0 {
            "xs:decimal"
        } else {
            "xs:string"
        };
        lines.push(primitive_property("Code", code_type, false));
    }
    if shape.description_length > 0 {
        lines.push(primitive_property("Description", "xs:string", false));
    }
    lines.push(primitive_property("PredefinedDataName", "xs:string", true));
    lines
}
