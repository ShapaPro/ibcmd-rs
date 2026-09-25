//! The export direction for the simple objects: a stored row -> the
//! object's XML DOM, and what the row contributes to a name index.
//!
//! Each decoder is the inverse of the kind's compiler in `simple.rs`; the
//! XML property orders are the ones every file of the four reference trees
//! uses (the same in 2.20 and 2.21).

use anyhow::{Result, anyhow, bail};

use crate::metadata_model::brace::Brace;
use crate::metadata_model::export::values::{
    Owner, attribute_elements, attribute_properties, bool_text, code_text, handler_text, header,
    header_elements, localized_element, metadata_ref_text, reference_name, type_element,
    typed_header_elements, value_element,
};
use crate::metadata_model::export::{
    Build, ExportContext, GeneratedTypeName, ObjectNames, atom, el, item, leaf, list, number,
    short, string, xml_text,
};
use crate::metadata_model::xml::Element;

/// Decodes one stored row of a simple kind into the object's element.
pub(crate) fn decode(kind: &str, row: &Brace, context: &ExportContext) -> Result<Element> {
    let root = list(row)?;
    if atom(item(root, 0)?)? != "1" {
        bail!("not a descriptor row: {}", short(row));
    }
    let payload = item(root, 1)?;
    match kind {
        "Language" => language(payload),
        "Constant" => constant(payload, context),
        "DefinedType" => defined_type(payload, context),
        "SessionParameter" => session_parameter(payload, context),
        "CommonAttribute" => common_attribute(payload, context),
        "FunctionalOption" => functional_option(payload, context),
        "FunctionalOptionsParameter" => functional_options_parameter(payload, context),
        "EventSubscription" => event_subscription(payload, context),
        "ScheduledJob" => scheduled_job(payload, context),
        "SettingsStorage" => settings_storage(root, context),
        "FilterCriterion" => filter_criterion(root, context),
        other => bail!("{other} is not a simple kind"),
    }
}

/// The payload's fields after its version number, checked.
fn record<'a>(payload: &'a Brace, version: &str, kind: &str) -> Result<&'a [Brace]> {
    let fields = list(payload)?;
    if atom(item(fields, 0)?)? != version {
        bail!("not a {kind} record: {}", short(payload));
    }
    Ok(fields)
}

/// `<xr:GeneratedType name=.. category=..><xr:TypeId/><xr:ValueId/>`.
fn generated_type(name: String, category: &str, type_id: &Brace, value_id: &Brace) -> Result<Element> {
    Ok(el("xr:GeneratedType")
        .attr("name", name)
        .attr("category", category)
        .child(leaf("xr:TypeId", atom(type_id)?))
        .child(leaf("xr:ValueId", atom(value_id)?)))
}

/// `<Kind uuid=..>` with its parts.
fn object(kind: &str, uuid: &str, parts: impl IntoIterator<Item = Element>) -> Element {
    el(kind).attr("uuid", uuid).children(parts)
}

/// A form (or other owned object) of `owner` by uuid -> its short name.
fn owned_name(uuid: &str, tag: &str, owner: &str, context: &ExportContext) -> Result<String> {
    let full = context
        .names
        .name(uuid)
        .ok_or_else(|| anyhow!("no name for {tag} {uuid}"))?;
    full.strip_prefix(&format!("{owner}.{tag}."))
        .map(str::to_string)
        .ok_or_else(|| anyhow!("{tag} {full} is not owned by {owner}"))
}

/// A reference slot (a form, a location) -> the full name it names (empty
/// for the nil uuid).
fn named(node: &Brace, context: &ExportContext) -> Result<String> {
    reference_name(atom(node)?, &context.names)
}

/// `{1,{0,<md base>,"<code>"},0}`
fn language(payload: &Brace) -> Result<Element> {
    let fields = record(payload, "0", "Language")?;
    let head = header(item(fields, 1)?)?;
    let [name, synonym, comment] = header_elements(&head)?;
    Ok(object(
        "Language",
        &head.uuid,
        [el("Properties")
            .child(name)
            .child(synonym)
            .child(comment)
            .child(leaf("LanguageCode", string(item(fields, 2)?)?))],
    ))
}

const DATA_LOCK_CONTROL_MODE: &[(&str, i64)] = &[("Automatic", 0), ("Managed", 1)];
const DATA_HISTORY: &[(&str, i64)] = &[("DontUse", 0), ("Use", 1)];

/// The properties a constant writes, in order.
const CONSTANT_PROPERTIES: &[&str] = &[
    "Name",
    "Synonym",
    "Comment",
    "Type",
    "UseStandardCommands",
    "DefaultForm",
    "ExtendedPresentation",
    "Explanation",
    "PasswordMode",
    "Format",
    "EditFormat",
    "ToolTip",
    "MarkNegatives",
    "Mask",
    "MultiLine",
    "ExtendedEdit",
    "MinValue",
    "MaxValue",
    "FillChecking",
    "ChoiceFoldersAndItems",
    "ChoiceParameterLinks",
    "ChoiceParameters",
    "QuickChoice",
    "ChoiceForm",
    "LinkByType",
    "ChoiceHistoryOnInput",
    "DataLockControlMode",
    "DataHistory",
    "UpdateDataHistoryImmediatelyAfterWrite",
    "ExecuteAfterWriteDataHistoryVersionProcessing",
];

/// `{16,<body>,<Manager ids>,<ValueManager ids>,<DataLockControlMode>,
/// <UseStandardCommands>,<ExtendedPresentation>,<Explanation>,<DefaultForm>,
/// <flag>,<DataHistory>,<ValueKey ids>,<Update...>,<Execute...>}`
fn constant(payload: &Brace, context: &ExportContext) -> Result<Element> {
    let fields = record(payload, "16", "Constant")?;
    if fields.len() != 17 {
        bail!("a constant record has 17 values, not {}", fields.len());
    }
    let body = item(fields, 1)?;
    let head = crate::metadata_model::export::values::attribute_header(body)?;
    let full_name = format!("Constant.{}", head.name);
    let owner = Owner {
        kind: "Constant",
        full_name: &full_name,
    };
    let elements = attribute_elements(body, owner, context)?;
    let properties = elements.ordered(CONSTANT_PROPERTIES, |name| {
        Ok(match name {
            "UseStandardCommands" => leaf(name, bool_text(&fields[7])?),
            "DefaultForm" => leaf(name, named(&fields[10], context)?),
            "ExtendedPresentation" => localized_element(name, &fields[8])?,
            "Explanation" => localized_element(name, &fields[9])?,
            "DataLockControlMode" => leaf(name, code_text(&fields[6], DATA_LOCK_CONTROL_MODE)?),
            "DataHistory" => leaf(name, code_text(&fields[12], DATA_HISTORY)?),
            "UpdateDataHistoryImmediatelyAfterWrite" => leaf(name, bool_text(&fields[15])?),
            "ExecuteAfterWriteDataHistoryVersionProcessing" => {
                leaf(name, bool_text(&fields[16])?)
            }
            other => bail!("a constant has no <{other}>"),
        })
    })?;
    let internal = el("InternalInfo")
        .child(generated_type(
            format!("ConstantManager.{}", head.name),
            "Manager",
            &fields[2],
            &fields[3],
        )?)
        .child(generated_type(
            format!("ConstantValueManager.{}", head.name),
            "ValueManager",
            &fields[4],
            &fields[5],
        )?)
        .child(generated_type(
            format!("ConstantValueKey.{}", head.name),
            "ValueKey",
            &fields[13],
            &fields[14],
        )?);
    Ok(object(
        "Constant",
        &head.uuid,
        [internal, el("Properties").children(properties)],
    ))
}

/// `{0,<type id>,<value id>,<md base>,<pattern>}`
fn defined_type(payload: &Brace, context: &ExportContext) -> Result<Element> {
    let fields = record(payload, "0", "DefinedType")?;
    let head = header(item(fields, 3)?)?;
    let [name, synonym, comment] = header_elements(&head)?;
    let internal = el("InternalInfo").child(generated_type(
        format!("DefinedType.{}", head.name),
        "DefinedType",
        item(fields, 1)?,
        item(fields, 2)?,
    )?);
    Ok(object(
        "DefinedType",
        &head.uuid,
        [
            internal,
            el("Properties")
                .child(name)
                .child(synonym)
                .child(comment)
                .child(type_element("Type", item(fields, 4)?, &context.names)?),
        ],
    ))
}

/// `{1,{2,<md base>,<pattern>}}`
fn session_parameter(payload: &Brace, context: &ExportContext) -> Result<Element> {
    let fields = record(payload, "1", "SessionParameter")?;
    let (head, type_) = typed_header_elements(item(fields, 1)?, &context.names)?;
    let [name, synonym, comment] = header_elements(&head)?;
    Ok(object(
        "SessionParameter",
        &head.uuid,
        [el("Properties")
            .child(name)
            .child(synonym)
            .child(comment)
            .child(type_)],
    ))
}

const SEPARATION: &[(&str, i64)] = &[("DontUse", 0), ("Separate", 1)];
const CONTENT_USE: &[(&str, i64)] = &[("Auto", 0), ("Use", 1), ("DontUse", 2)];

/// `{1,<uuid>}` -> the full name it names (empty for the nil uuid).
fn wrapped_reference(node: &Brace, context: &ExportContext) -> Result<String> {
    let fields = list(node)?;
    if atom(item(fields, 0)?)? != "1" {
        bail!("not a wrapped reference: {}", short(node));
    }
    reference_name(atom(item(fields, 1)?)?, &context.names)
}

/// `{5,<body>,<content>,<Indexing>,<FullTextSearch>,<DataSeparation>,
/// <AutoUse>,{1,<DataSeparationValue>},{1,<DataSeparationUse>},
/// {1,<ConditionalSeparation>},<UsersSeparation>,<AuthenticationSeparation>,
/// <SeparatedDataUse>,<ConfigurationExtensionsSeparation>,<DataHistory>}`
fn common_attribute(payload: &Brace, context: &ExportContext) -> Result<Element> {
    let fields = record(payload, "5", "CommonAttribute")?;
    if fields.len() != 15 {
        bail!("a common attribute record has 15 values, not {}", fields.len());
    }
    let body = item(fields, 1)?;
    let head = crate::metadata_model::export::values::attribute_header(body)?;
    let full_name = format!("CommonAttribute.{}", head.name);
    let owner = Owner {
        kind: "CommonAttribute",
        full_name: &full_name,
    };
    // `{3,<n>,<object uuid>,{2,<use>,<conditional separation>},...}`
    let stored = list(item(fields, 2)?)?;
    let count = number(item(stored, 1)?)? as usize;
    let mut content = el("Content");
    for index in 0..count {
        let metadata = reference_name(atom(item(stored, 2 + 2 * index)?)?, &context.names)?;
        let usage = list(item(stored, 3 + 2 * index)?)?;
        content.children.push(
            el("xr:Item")
                .child(leaf("xr:Metadata", metadata))
                .child(leaf("xr:Use", code_text(item(usage, 1)?, CONTENT_USE)?))
                .child(leaf(
                    "xr:ConditionalSeparation",
                    reference_name(atom(item(usage, 2)?)?, &context.names)?,
                )),
        );
    }
    let tail = vec![
        content,
        leaf("AutoUse", code_text(&fields[6], &[("Use", 0), ("DontUse", 1)])?),
        leaf(
            "DataSeparation",
            code_text(&fields[5], &[("Separate", 0), ("DontUse", 1)])?,
        ),
        leaf(
            "SeparatedDataUse",
            code_text(
                &fields[12],
                &[("Independently", 0), ("IndependentlyAndSimultaneously", 1)],
            )?,
        ),
        leaf("DataSeparationValue", wrapped_reference(&fields[7], context)?),
        leaf("DataSeparationUse", wrapped_reference(&fields[8], context)?),
        leaf("ConditionalSeparation", wrapped_reference(&fields[9], context)?),
        leaf("UsersSeparation", code_text(&fields[10], SEPARATION)?),
        leaf("AuthenticationSeparation", code_text(&fields[11], SEPARATION)?),
        leaf(
            "ConfigurationExtensionsSeparation",
            code_text(&fields[13], SEPARATION)?,
        ),
        leaf(
            "Indexing",
            code_text(
                &fields[3],
                &[("DontIndex", 0), ("Index", 1), ("IndexWithAdditionalOrder", 2)],
            )?,
        ),
        leaf("FullTextSearch", code_text(&fields[4], &[("DontUse", 0), ("Use", 1)])?),
        leaf("DataHistory", code_text(&fields[14], DATA_HISTORY)?),
    ];
    Ok(object(
        "CommonAttribute",
        &head.uuid,
        [attribute_properties(body, true, tail, owner, context)?],
    ))
}

/// `{2,<md base>,<Location>,{0,<n>,{"#",3ea29ea5-...,{0,<metadata ref>}}...},
/// <PrivilegedGetMode>}`
fn functional_option(payload: &Brace, context: &ExportContext) -> Result<Element> {
    let fields = record(payload, "2", "FunctionalOption")?;
    let head = header(item(fields, 1)?)?;
    let [name, synonym, comment] = header_elements(&head)?;
    let stored = list(item(fields, 3)?)?;
    let count = number(item(stored, 1)?)? as usize;
    let mut content = el("Content");
    for entry in stored.iter().skip(2).take(count) {
        let reference = item(list(item(list(entry)?, 2)?)?, 1)?;
        content.children.push(leaf(
            "xr:Object",
            metadata_ref_text(reference, &context.names)?,
        ));
    }
    Ok(object(
        "FunctionalOption",
        &head.uuid,
        [el("Properties")
            .child(name)
            .child(synonym)
            .child(comment)
            .child(leaf("Location", named(item(fields, 2)?, context)?))
            .child(leaf("PrivilegedGetMode", bool_text(item(fields, 4)?)?))
            .child(content)],
    ))
}

/// `{0,<md base>,{0,<n>,<metadata ref>...}}` (`{0}` when empty)
fn functional_options_parameter(payload: &Brace, context: &ExportContext) -> Result<Element> {
    let fields = record(payload, "0", "FunctionalOptionsParameter")?;
    let head = header(item(fields, 1)?)?;
    let [name, synonym, comment] = header_elements(&head)?;
    let stored = list(item(fields, 2)?)?;
    let count = match stored.get(1) {
        Some(count) => number(count)? as usize,
        None => 0,
    };
    let mut uses = el("Use");
    for entry in stored.iter().skip(2).take(count) {
        uses.children.push(value_element("xr:Item", entry, &context.names)?);
    }
    Ok(object(
        "FunctionalOptionsParameter",
        &head.uuid,
        [el("Properties")
            .child(name)
            .child(synonym)
            .child(comment)
            .child(uses)],
    ))
}

/// `{1,<md base>,<source pattern>,"<Event>_<Событие>",<module>,"<method>"}`
fn event_subscription(payload: &Brace, context: &ExportContext) -> Result<Element> {
    let fields = record(payload, "1", "EventSubscription")?;
    let head = header(item(fields, 1)?)?;
    let [name, synonym, comment] = header_elements(&head)?;
    let event = string(item(fields, 3)?)?;
    let event = event.split_once('_').map_or(event, |(english, _)| english);
    Ok(object(
        "EventSubscription",
        &head.uuid,
        [el("Properties")
            .child(name)
            .child(synonym)
            .child(comment)
            .child(type_element("Source", item(fields, 2)?, &context.names)?)
            .child(leaf("Event", event))
            .child(leaf(
                "Handler",
                handler_text(item(fields, 4)?, item(fields, 5)?, &context.names)?,
            ))],
    ))
}

/// `{2,<md base>,"<Key>","<Description>",<Use>,<Predefined>,<module>,
/// "<method>",<RestartCountOnFailure>,<RestartIntervalOnFailure>}`
fn scheduled_job(payload: &Brace, context: &ExportContext) -> Result<Element> {
    let fields = record(payload, "2", "ScheduledJob")?;
    let head = header(item(fields, 1)?)?;
    let [name, synonym, comment] = header_elements(&head)?;
    Ok(object(
        "ScheduledJob",
        &head.uuid,
        [el("Properties")
            .child(name)
            .child(synonym)
            .child(comment)
            .child(leaf(
                "MethodName",
                handler_text(item(fields, 6)?, item(fields, 7)?, &context.names)?,
            ))
            .child(leaf("Description", xml_text(string(item(fields, 3)?)?)))
            .child(leaf("Key", xml_text(string(item(fields, 2)?)?)))
            .child(leaf("Use", bool_text(item(fields, 4)?)?))
            .child(leaf("Predefined", bool_text(item(fields, 5)?)?))
            .child(leaf("RestartCountOnFailure", atom(item(fields, 8)?)?))
            .child(leaf("RestartIntervalOnFailure", atom(item(fields, 9)?)?))],
    ))
}

const SETTINGS_STORAGE_TEMPLATES: &str = "3daea016-69b7-4ed4-9453-127911372fe6";
const SETTINGS_STORAGE_FORMS: &str = "b8533c0c-2342-4db3-91a2-c2b08cbf6b23";
const FILTER_CRITERION_FORMS: &str = "00867c40-06b1-11d6-a3c7-0050bae0a776";
const FILTER_CRITERION_COMMANDS: &str = "23fa3b84-220a-40e9-8331-e588bed87f7d";

/// The items of the row's collection `class` (after `{1,<payload>,<n>,...}`).
fn collection<'a>(root: &'a [Brace], class: &str) -> Result<&'a [Brace]> {
    let count = number(item(root, 2)?)? as usize;
    for stored in root.iter().skip(3).take(count) {
        let fields = list(stored)?;
        if atom(item(fields, 0)?)? == class {
            let declared = number(item(fields, 1)?)? as usize;
            return fields
                .get(2..2 + declared)
                .ok_or_else(|| anyhow!("short collection {class}"));
        }
    }
    bail!("row has no collection {class}")
}

/// `<Form>F</Form>` / `<Template>T</Template>` for owned objects by uuid.
fn owned(tag: &str, items: &[Brace], owner: &str, context: &ExportContext) -> Result<Vec<Element>> {
    items
        .iter()
        .map(|uuid| Ok(leaf(tag, owned_name(atom(uuid)?, tag, owner, context)?)))
        .collect()
}

/// `{1,{2,{0,<md base>},<Manager ids>,<DefaultLoadForm>,<DefaultSaveForm>,
/// <AuxiliaryLoadForm>,<AuxiliarySaveForm>},2,{3daea016-...,<templates>},
/// {b8533c0c-...,<forms>}}`
fn settings_storage(root: &[Brace], context: &ExportContext) -> Result<Element> {
    let fields = record(item(root, 1)?, "2", "SettingsStorage")?;
    let head = header(item(list(item(fields, 1)?)?, 1)?)?;
    let full_name = format!("SettingsStorage.{}", head.name);
    let [name, synonym, comment] = header_elements(&head)?;
    let internal = el("InternalInfo").child(generated_type(
        format!("SettingsStorageManager.{}", head.name),
        "Manager",
        item(fields, 2)?,
        item(fields, 3)?,
    )?);
    let properties = el("Properties")
        .child(name)
        .child(synonym)
        .child(comment)
        .child(leaf("DefaultSaveForm", named(item(fields, 5)?, context)?))
        .child(leaf("DefaultLoadForm", named(item(fields, 4)?, context)?))
        .child(leaf("AuxiliarySaveForm", named(item(fields, 7)?, context)?))
        .child(leaf("AuxiliaryLoadForm", named(item(fields, 6)?, context)?));
    let children = el("ChildObjects")
        .children(owned(
            "Form",
            collection(root, SETTINGS_STORAGE_FORMS)?,
            &full_name,
            context,
        )?)
        .children(owned(
            "Template",
            collection(root, SETTINGS_STORAGE_TEMPLATES)?,
            &full_name,
            context,
        )?);
    Ok(object(
        "SettingsStorage",
        &head.uuid,
        [internal, properties, children],
    ))
}

/// `{1,{14,<Manager ids>,<List ids>,{2,<md base>,<pattern>},
/// {0,<n>,<metadata ref>...},<UseStandardCommands>,<DefaultForm>,
/// <AuxiliaryForm>,<ListPresentation>,<ExtendedListPresentation>,
/// <Explanation>},2,{00867c40-...,<forms>},{23fa3b84-...,<commands>}}`
fn filter_criterion(root: &[Brace], context: &ExportContext) -> Result<Element> {
    let fields = record(item(root, 1)?, "14", "FilterCriterion")?;
    let (head, type_) = typed_header_elements(item(fields, 5)?, &context.names)?;
    let full_name = format!("FilterCriterion.{}", head.name);
    let [name, synonym, comment] = header_elements(&head)?;
    let internal = el("InternalInfo")
        .child(generated_type(
            format!("FilterCriterionManager.{}", head.name),
            "Manager",
            item(fields, 1)?,
            item(fields, 2)?,
        )?)
        .child(generated_type(
            format!("FilterCriterionList.{}", head.name),
            "List",
            item(fields, 3)?,
            item(fields, 4)?,
        )?);
    let stored = list(item(fields, 6)?)?;
    let count = number(item(stored, 1)?)? as usize;
    let mut content = el("Content");
    for entry in stored.iter().skip(2).take(count) {
        content.children.push(value_element("xr:Item", entry, &context.names)?);
    }
    let properties = el("Properties")
        .child(name)
        .child(synonym)
        .child(comment)
        .child(type_)
        .child(leaf("UseStandardCommands", bool_text(item(fields, 7)?)?))
        .child(content)
        .child(leaf("DefaultForm", named(item(fields, 8)?, context)?))
        .child(leaf("AuxiliaryForm", named(item(fields, 9)?, context)?))
        .child(localized_element("ListPresentation", item(fields, 10)?)?)
        .child(localized_element("ExtendedListPresentation", item(fields, 11)?)?)
        .child(localized_element("Explanation", item(fields, 12)?)?);
    let mut children = el("ChildObjects").children(owned(
        "Form",
        collection(root, FILTER_CRITERION_FORMS)?,
        &full_name,
        context,
    )?);
    // `{{0,{0,0,0,<command>}},0}`, the reference objects' owned command.
    for stored in collection(root, FILTER_CRITERION_COMMANDS)? {
        let inner = item(list(item(list(stored)?, 0)?)?, 1)?;
        let body = item(list(inner)?, 3)?;
        children
            .children
            .push(crate::metadata_model::objects::export::command(body, context)?);
    }
    Ok(object(
        "FilterCriterion",
        &head.uuid,
        [internal, properties, children],
    ))
}

// ---------------------------------------------------------------------------
// Names.

fn generated(name: String, category: &str, type_id: &Brace, value_id: &Brace) -> Result<GeneratedTypeName> {
    Ok(GeneratedTypeName {
        name,
        category: category.to_string(),
        type_id: atom(type_id)?.to_string(),
        value_id: atom(value_id)?.to_string(),
    })
}

/// What a simple object's row contributes to a name index.
pub(crate) fn names(kind: &str, row: &Brace) -> Result<ObjectNames> {
    let root = list(row)?;
    let fields = list(item(root, 1)?)?;
    let (head, types, children) = match kind {
        "Language" | "FunctionalOption" | "FunctionalOptionsParameter" | "EventSubscription"
        | "ScheduledJob" => (header(item(fields, 1)?)?, Vec::new(), Vec::new()),
        "SessionParameter" => (
            header(item(list(item(fields, 1)?)?, 1)?)?,
            Vec::new(),
            Vec::new(),
        ),
        "CommonAttribute" => (
            crate::metadata_model::export::values::attribute_header(item(fields, 1)?)?,
            Vec::new(),
            Vec::new(),
        ),
        "Constant" => {
            let head = crate::metadata_model::export::values::attribute_header(item(fields, 1)?)?;
            let types = vec![
                generated(
                    format!("ConstantManager.{}", head.name),
                    "Manager",
                    item(fields, 2)?,
                    item(fields, 3)?,
                )?,
                generated(
                    format!("ConstantValueManager.{}", head.name),
                    "ValueManager",
                    item(fields, 4)?,
                    item(fields, 5)?,
                )?,
                generated(
                    format!("ConstantValueKey.{}", head.name),
                    "ValueKey",
                    item(fields, 13)?,
                    item(fields, 14)?,
                )?,
            ];
            (head, types, Vec::new())
        }
        "DefinedType" => {
            let head = header(item(fields, 3)?)?;
            let types = vec![generated(
                format!("DefinedType.{}", head.name),
                "DefinedType",
                item(fields, 1)?,
                item(fields, 2)?,
            )?];
            (head, types, Vec::new())
        }
        "SettingsStorage" => {
            let head = header(item(list(item(fields, 1)?)?, 1)?)?;
            let types = vec![generated(
                format!("SettingsStorageManager.{}", head.name),
                "Manager",
                item(fields, 2)?,
                item(fields, 3)?,
            )?];
            (head, types, Vec::new())
        }
        "FilterCriterion" => {
            let head = header(item(list(item(fields, 5)?)?, 1)?)?;
            let types = vec![
                generated(
                    format!("FilterCriterionManager.{}", head.name),
                    "Manager",
                    item(fields, 1)?,
                    item(fields, 2)?,
                )?,
                generated(
                    format!("FilterCriterionList.{}", head.name),
                    "List",
                    item(fields, 3)?,
                    item(fields, 4)?,
                )?,
            ];
            let mut children = Vec::new();
            for stored in collection(root, FILTER_CRITERION_COMMANDS)? {
                let inner = item(list(item(list(stored)?, 0)?)?, 1)?;
                let body = list(item(list(inner)?, 3)?)?;
                let command = list(item(body, 2)?)?;
                let command_head = header(item(command, 9)?)?;
                children.push((
                    format!("FilterCriterion.{}.Command.{}", head.name, command_head.name),
                    command_head.uuid,
                ));
            }
            (head, types, children)
        }
        other => bail!("{other} is not a simple kind"),
    };
    Ok(ObjectNames {
        uuid: head.uuid.clone(),
        full_name: format!("{kind}.{}", head.name),
        children,
        types,
    })
}
