//! The attribute body every attribute-like object shares: attributes of
//! objects and tabular sections, register dimensions/resources/attributes,
//! accounting flags, constants, common attributes. Owned by the
//! simple-objects track.
//!
//! Public API (stable; extended additively):
//! - [`typed_header`]: `{2,<md_base>,<type pattern>}`, the named-and-typed
//!   head (a session parameter's whole payload; the head of every body).
//! - [`attribute_body`]: the 23-slot `{27,...}` body read from an element's
//!   `<Properties>`:
//!
//!   ```text
//!   {27,{2,<md_base>,<pattern>},
//!    [2] PasswordMode, [3] Format, [4] ToolTip, [5] MarkNegatives, [6] Mask,
//!    [7] MultiLine, [8] MinValue, [9] MaxValue, [10] ChoiceFoldersAndItems,
//!    [11] ChoiceForm, [12] QuickChoice, [13] FillChecking,
//!    [14] ChoiceParameterLinks, [15] LinkByType, [16] ChoiceParameters,
//!    [17] ExtendedEdit, [18] EditFormat, [19] FillValue,
//!    [20] FillFromFillingValue, [21] CreateOnInput, [22] ChoiceHistoryOnInput}
//!   ```
//!
//!   Owners wrap it with their own slots: a catalog/document attribute is
//!   `{{5,<body>,<Use>,<Indexing>,<FullTextSearch>,<DataHistory>},0}` (see the
//!   objects track), a constant `{16,<body>,<generated ids>...}`.
//! - Single-property encoders for owners that write the same properties in
//!   their own slots: [`choice_parameter_links`], [`link_by_type`],
//!   [`choice_parameters`], [`fill_checking`], [`quick_choice`],
//!   [`create_on_input`], [`choice_history_on_input`],
//!   [`choice_folders_and_items`], [`bool_prop`], [`localized_prop`],
//!   [`string_prop`].

use anyhow::{Result, anyhow, bail};

use super::brace::Brace;
use super::types::{
    data_path, empty_string_value, form_uuid, type_pattern, typed_value,
};
use super::xml::Element;
use super::{DescriptorContext, localized, md_base, native_text, parse_bool};
use crate::brace_list;

/// `{2,<md_base>,<type pattern>}`.
pub fn typed_header(uuid: &str, properties: &Element, context: &DescriptorContext) -> Result<Brace> {
    Ok(brace_list![
        Brace::num(2),
        md_base(uuid, properties),
        type_pattern(properties.child("Type"), context)?,
    ])
}

/// The `{27,...}` attribute body of an element's `<Properties>`.
pub fn attribute_body(
    uuid: &str,
    properties: &Element,
    context: &DescriptorContext,
) -> Result<Brace> {
    let fill_value = match properties.child("FillValue") {
        // Owners without a fill value (constants) store the empty string.
        None => empty_string_value(),
        Some(value) => typed_value(Some(value), context)?,
    };
    Ok(brace_list![
        Brace::num(27),
        typed_header(uuid, properties, context)?,
        bool_prop(properties, "PasswordMode")?,
        localized_prop(properties, "Format"),
        localized_prop(properties, "ToolTip"),
        bool_prop(properties, "MarkNegatives")?,
        string_prop(properties, "Mask"),
        bool_prop(properties, "MultiLine")?,
        typed_value(properties.child("MinValue"), context)?,
        typed_value(properties.child("MaxValue"), context)?,
        choice_folders_and_items(properties.child_text("ChoiceFoldersAndItems"))?,
        Brace::uuid(&form_uuid(
            properties.child_text("ChoiceForm").unwrap_or_default(),
            context
        )?),
        quick_choice(properties.child_text("QuickChoice"))?,
        fill_checking(properties.child_text("FillChecking"))?,
        choice_parameter_links(properties.child("ChoiceParameterLinks"), context)?,
        link_by_type(properties.child("LinkByType"), context)?,
        choice_parameters(properties.child("ChoiceParameters"), context)?,
        bool_prop(properties, "ExtendedEdit")?,
        localized_prop(properties, "EditFormat"),
        fill_value,
        bool_prop(properties, "FillFromFillingValue")?,
        create_on_input(properties.child_text("CreateOnInput"))?,
        choice_history_on_input(properties.child_text("ChoiceHistoryOnInput"))?,
    ])
}

/// A `true`/`false` property as `1`/`0`; absent is `0`.
pub fn bool_prop(properties: &Element, name: &str) -> Result<Brace> {
    match properties.child_text(name) {
        None => Ok(Brace::flag(false)),
        Some(text) => Ok(Brace::flag(
            parse_bool(text.trim()).map_err(|error| anyhow!("<{name}>: {error}"))?,
        )),
    }
}

/// A localized string property (`{N,"lang","text",...}` or `{0}`).
pub fn localized_prop(properties: &Element, name: &str) -> Brace {
    localized(properties.child(name))
}

/// A plain string property as a quoted string; absent is `""`.
pub fn string_prop(properties: &Element, name: &str) -> Brace {
    Brace::str(native_text(properties.child_text(name).unwrap_or_default()))
}

fn enumerated(value: Option<&str>, name: &str, table: &[(&str, i64)], absent: i64) -> Result<Brace> {
    let Some(value) = value else {
        return Ok(Brace::num(absent));
    };
    table
        .iter()
        .find(|(candidate, _)| *candidate == value.trim())
        .map(|(_, code)| Brace::num(*code))
        .ok_or_else(|| anyhow!("unknown {name} {value:?}"))
}

/// `DontCheck` 0, `ShowError` 1.
pub fn fill_checking(value: Option<&str>) -> Result<Brace> {
    enumerated(
        value,
        "FillChecking",
        &[("DontCheck", 0), ("ShowError", 1)],
        0,
    )
}

/// `DontUse` 0, `Use` 1, `Auto` 2.
pub fn quick_choice(value: Option<&str>) -> Result<Brace> {
    enumerated(
        value,
        "QuickChoice",
        &[("DontUse", 0), ("Use", 1), ("Auto", 2)],
        2,
    )
}

/// `Auto` 0, `DontUse` 1, `Use` 2.
pub fn create_on_input(value: Option<&str>) -> Result<Brace> {
    enumerated(
        value,
        "CreateOnInput",
        &[("Auto", 0), ("DontUse", 1), ("Use", 2)],
        0,
    )
}

/// `Auto` 0, `DontUse` 1.
pub fn choice_history_on_input(value: Option<&str>) -> Result<Brace> {
    enumerated(
        value,
        "ChoiceHistoryOnInput",
        &[("Auto", 0), ("DontUse", 1)],
        0,
    )
}

/// `Items` 0, `Folders` 1, `FoldersAndItems` 2.
pub fn choice_folders_and_items(value: Option<&str>) -> Result<Brace> {
    enumerated(
        value,
        "ChoiceFoldersAndItems",
        &[("Items", 0), ("Folders", 1), ("FoldersAndItems", 2)],
        0,
    )
}

/// `{5006,<count>,"<name>",<segment count>,<segment>...,<0 clear | 1 don't change>,...}`.
pub fn choice_parameter_links(
    links: Option<&Element>,
    context: &DescriptorContext,
) -> Result<Brace> {
    let mut items = vec![Brace::num(5006), Brace::num(0)];
    let mut count = 0;
    if let Some(links) = links {
        for link in links.children_named("Link") {
            count += 1;
            items.push(Brace::str(link.child_text("Name").unwrap_or_default()));
            let segments = data_path(link.child_text("DataPath").unwrap_or_default(), context)?;
            items.push(Brace::num(segments.len() as i64));
            items.extend(segments);
            items.push(match link.child_text("ValueChange").unwrap_or("Clear") {
                "Clear" => Brace::num(0),
                "DontChange" => Brace::num(1),
                other => bail!("unknown ValueChange {other:?}"),
            });
        }
    }
    items[1] = Brace::num(count);
    Ok(Brace::List(items))
}

/// `{3,<segment count>,<segment>...,<link item>}`; `{3,0,0}` when empty.
pub fn link_by_type(link: Option<&Element>, context: &DescriptorContext) -> Result<Brace> {
    let Some(link) = link.filter(|link| link.child("DataPath").is_some()) else {
        return Ok(brace_list![Brace::num(3), Brace::num(0), Brace::num(0)]);
    };
    let segments = data_path(link.child_text("DataPath").unwrap_or_default(), context)?;
    let mut items = vec![Brace::num(3), Brace::num(segments.len() as i64)];
    items.extend(segments);
    let link_item = link.child_text("LinkItem").unwrap_or("0").trim();
    items.push(Brace::atom(link_item));
    Ok(Brace::List(items))
}

/// `{0,<count>,"<name>",<typed value>,...}`; `{0,0}` when empty.
pub fn choice_parameters(
    parameters: Option<&Element>,
    context: &DescriptorContext,
) -> Result<Brace> {
    let mut items = vec![Brace::num(0), Brace::num(0)];
    let mut count = 0;
    if let Some(parameters) = parameters {
        for item in parameters.children_named("item") {
            count += 1;
            items.push(Brace::str(item.attr("name").unwrap_or_default()));
            items.push(typed_value(item.child("value"), context)?);
        }
    }
    items[1] = Brace::num(count);
    Ok(Brace::List(items))
}
