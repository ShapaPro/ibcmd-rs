//! The XDTO model cache of the configuration: a `Params` row `<uuid>.si`.
//!
//! Of the 16 `*.si` rows (derived caches the platform rewrites on every apply) one holds the XDTO model
//! of the configuration's types: raw deflate of `{2,1,{{#base64:<b64>}}}` where the base64 (64 characters a
//! line, lines separated by `CR CR LF`) encodes the model XML (`<model xmlns="http://v8.1c.ru/8.1/xdto">`,
//! UTF-8 with BOM, CRLF). A new attribute of a catalog is one more `<property .../>` line in the
//! `<objectType name="CatalogObject.<Name>">` element, right after the property of the attribute before it
//! (case a2: `<property name="ДемоНовыйРеквизит" type="xs:string" lowerBound="0"/>`, 25 173 365 -> 25 173 491
//! characters of row text). Left stale, serialization of the object through XDTO fails with "Свойство ... не
//! обнаружено" while queries and writes work (measured); a deleted row is rebuilt in memory by the platform.

use anyhow::{Context, Result, bail};

use crate::module_blob::{decode_base64_mime, encode_base64};
use crate::restructure::names::{deflate, inflate};
use crate::restructure::schema::TypeEntry;

const PREFIX: &str = "{2,1,\r\n{\r\n{#base64:";
const SUFFIX: &str = "}\r\n}\r\n}";
const LINE: usize = 64;
const SEPARATOR: &str = "\r\r\n";

/// The XDTO property type of a field's type entry, for the shapes the corpus uses on catalog objects
/// (`xs:string`, `xs:boolean`, `xs:decimal`, `xs:dateTime`, all with `lowerBound="0"`); only the string was
/// traced on the platform (case a2), the others are read off the 191 / 64 / 9 / 10 properties of the model.
pub fn property_type(entry: &TypeEntry) -> Result<&'static str> {
    Ok(match entry.tag.as_str() {
        "S" => "xs:string",
        "L" => "xs:boolean",
        "N" => "xs:decimal",
        "T" => "xs:dateTime",
        other => bail!("no XDTO property type is known for the type tag {other}"),
    })
}

/// The row text (inflated, BOM optional) cut into its parts.
struct Parts {
    bom: bool,
    xml: Vec<u8>,
}

fn split(row_text: &[u8]) -> Result<Parts> {
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
    let xml = decode_base64_mime(&stripped).context("the model is not base64")?;
    Ok(Parts { bom, xml })
}

fn join(parts: &Parts) -> Vec<u8> {
    let encoded = encode_base64(&parts.xml);
    let mut text =
        String::with_capacity(encoded.len() + encoded.len() / LINE * SEPARATOR.len() + 64);
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
    if parts.bom {
        out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    }
    out.extend_from_slice(text.as_bytes());
    out
}

/// Whether an inflated row is the XDTO model.
pub fn is_model(row_text: &[u8]) -> bool {
    let text = row_text
        .strip_prefix(&[0xEF, 0xBB, 0xBF])
        .unwrap_or(row_text);
    text.starts_with(PREFIX.as_bytes())
}

/// Whether the model has `<objectType name="<object_type>">` (`CatalogObject.X`).
pub fn has_object_type(row_text: &[u8], object_type: &str) -> Result<bool> {
    let parts = split(row_text)?;
    let tag = format!("<objectType name=\"{object_type}\">");
    Ok(find(&parts.xml, tag.as_bytes(), 0).is_some())
}

/// Inserts `<property name="<name>" type="<type>" lowerBound="0"/>` into
/// `CatalogObject.<catalog>` right after the property `after`; returns the new row text.
pub fn add_catalog_property(
    row_text: &[u8],
    catalog: &str,
    after: &str,
    name: &str,
    property_type: &str,
) -> Result<Vec<u8>> {
    let mut parts = split(row_text)?;
    let start_tag = format!("<objectType name=\"CatalogObject.{catalog}\">");
    let start = find(&parts.xml, start_tag.as_bytes(), 0)
        .with_context(|| format!("the model has no {start_tag}"))?;
    let end = find(&parts.xml, b"</objectType>", start)
        .context("the object type of the catalog is not closed")?;
    let anchor = format!("name=\"{after}\"");
    let at = find(&parts.xml[..end], anchor.as_bytes(), start)
        .with_context(|| format!("CatalogObject.{catalog} has no property {after}"))?;
    let line_end = find(&parts.xml[..end], b"\r\n", at)
        .context("the property line of the attribute before is not terminated")?
        + 2;
    let line =
        format!("\t\t\t<property name=\"{name}\" type=\"{property_type}\" lowerBound=\"0\"/>\r\n");
    if find(
        &parts.xml[start..end],
        format!("name=\"{name}\"").as_bytes(),
        0,
    )
    .is_some()
    {
        bail!("CatalogObject.{catalog} already has a property {name}");
    }
    parts.xml.splice(line_end..line_end, line.bytes());
    Ok(join(&parts))
}

/// A row as stored, updated: inflate, insert, deflate.
pub fn update_row(
    stored: &[u8],
    catalog: &str,
    after: &str,
    name: &str,
    property_type: &str,
) -> Result<Vec<u8>> {
    let text = inflate(stored)?;
    deflate(&add_catalog_property(
        &text,
        catalog,
        after,
        name,
        property_type,
    )?)
}

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() || from > haystack.len() - needle.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|position| position + from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(xml: &str) -> Vec<u8> {
        join(&Parts {
            bom: true,
            xml: xml.as_bytes().to_vec(),
        })
    }

    const MODEL: &str = "\u{feff}<model xmlns=\"http://v8.1c.ru/8.1/xdto\">\r\n\t<package>\r\n\t\t<objectType name=\"CatalogObject.X\">\r\n\t\t\t<property name=\"Code\" type=\"xs:string\"/>\r\n\t\t\t<property name=\"Кл\" type=\"xs:boolean\" lowerBound=\"0\"/>\r\n\t\t\t<property name=\"Таб\" type=\"d4p1:Row\" lowerBound=\"0\" upperBound=\"99999\"/>\r\n\t\t</objectType>\r\n\t</package>\r\n</model>";

    #[test]
    fn a_row_is_wrapped_like_the_platforms() {
        let text = row(MODEL);
        assert!(text.starts_with(&[0xEF, 0xBB, 0xBF]));
        assert!(is_model(&text));
        let inner = std::str::from_utf8(&text[3..]).unwrap();
        assert!(inner.starts_with("{2,1,\r\n{\r\n{#base64:"));
        assert!(inner.ends_with("}\r\n}\r\n}"));
        // 64 characters a line, CR CR LF between the lines
        let body = inner
            .strip_prefix(PREFIX)
            .unwrap()
            .strip_suffix(SUFFIX)
            .unwrap();
        let lines: Vec<_> = body.split("\r\r\n").collect();
        assert!(lines.len() > 1);
        assert!(lines[..lines.len() - 1].iter().all(|line| line.len() == 64));
        assert_eq!(split(&text).unwrap().xml, MODEL.as_bytes());
    }

    #[test]
    fn a_property_goes_after_the_property_of_the_attribute_before_it() {
        let updated = add_catalog_property(&row(MODEL), "X", "Кл", "Новый", "xs:string").unwrap();
        let xml = String::from_utf8(split(&updated).unwrap().xml).unwrap();
        let expected = MODEL.replace(
            "\t\t\t<property name=\"Таб\"",
            "\t\t\t<property name=\"Новый\" type=\"xs:string\" lowerBound=\"0\"/>\r\n\t\t\t<property name=\"Таб\"",
        );
        assert_eq!(xml, expected);
        // Refusals: no such type, no such property, a property that is there already.
        assert!(add_catalog_property(&row(MODEL), "Y", "Кл", "Н", "xs:string").is_err());
        assert!(add_catalog_property(&row(MODEL), "X", "Нет", "Н", "xs:string").is_err());
        assert!(add_catalog_property(&row(MODEL), "X", "Кл", "Таб", "xs:string").is_err());
    }

    #[test]
    fn the_stored_form_round_trips() {
        let stored = deflate(&row(MODEL)).unwrap();
        let updated = update_row(&stored, "X", "Кл", "Новый", "xs:boolean").unwrap();
        let text = inflate(&updated).unwrap();
        assert!(is_model(&text));
        let xml = String::from_utf8(split(&text).unwrap().xml).unwrap();
        assert!(xml.contains("name=\"Новый\" type=\"xs:boolean\" lowerBound=\"0\""));
    }

    #[test]
    fn property_types_follow_the_type_entries() {
        let t = |tag: &str| property_type(&TypeEntry::new(tag, 0, 0, "", 0));
        assert_eq!(t("S").unwrap(), "xs:string");
        assert_eq!(t("L").unwrap(), "xs:boolean");
        assert_eq!(t("N").unwrap(), "xs:decimal");
        assert_eq!(t("T").unwrap(), "xs:dateTime");
        assert!(t("B").is_err());
        assert!(!is_model(b"{0,{0}}"));
    }
}
