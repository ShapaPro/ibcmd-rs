//! The export direction for the reference objects: a stored row -> the
//! object's XML DOM, walking the kind's `Layout` backwards, and what the row
//! contributes to a name index.
//!
//! The row stores properties in slot order; the XML writes them in the
//! order of [`Spec::properties`] (measured over the four reference trees:
//! every file of a kind uses the same order). Child objects are written per
//! [`Spec::children`], each collection in stored order.

use std::collections::HashMap;

use anyhow::{Context, Result, anyhow, bail};

use super::parts::{AttributeWrapper, Coll, CommandWrapper, Compat, Layout, Slot, TsWrapper};
use super::{layout, line_number_marker};
use crate::metadata_model::brace::Brace;
use crate::metadata_model::export::values::{
    Header, Owner, attribute_header, attribute_properties, bool_text, code_text, field_text,
    header, header_elements, localized_element, reference_name, standard_attribute_elements,
    standard_attributes_element, type_element, value_element,
};
use crate::metadata_model::export::{
    Build, ExportContext, GeneratedTypeName, ObjectNames, atom, el, item, leaf, list, number,
    short, string,
};
use crate::metadata_model::xml::Element;

/// How a kind writes its XML.
struct Spec {
    /// `<Properties>` children in order.
    properties: &'static [&'static str],
    /// `<ChildObjects>` element kinds in order.
    children: &'static [&'static str],
    /// What follows `ChoiceHistoryOnInput` in a root attribute.
    attribute_tail: &'static [&'static str],
    /// Root attributes write `FillFromFillingValue` and `FillValue`.
    attribute_fill: bool,
    /// Tabular-section attributes write them.
    section_attribute_fill: bool,
    /// What follows `ChoiceHistoryOnInput` in a tabular-section attribute.
    section_attribute_tail: &'static [&'static str],
    /// What follows `StandardAttributes` in a tabular section.
    section_tail: &'static [&'static str],
}

const REFERENCE_TAIL: &[&str] = &["Indexing", "FullTextSearch", "DataHistory"];

fn spec(kind: &str) -> Result<Spec> {
    let reference = |properties, children| Spec {
        properties,
        children,
        attribute_tail: REFERENCE_TAIL,
        attribute_fill: true,
        section_attribute_fill: false,
        section_attribute_tail: REFERENCE_TAIL,
        section_tail: &["LineNumberLength"],
    };
    const STANDARD_CHILDREN: &[&str] =
        &["Attribute", "TabularSection", "Form", "Template", "Command"];
    Ok(match kind {
        "Catalog" => Spec {
            attribute_tail: &["Use", "Indexing", "FullTextSearch", "DataHistory"],
            section_tail: &["Use", "LineNumberLength"],
            ..reference(CATALOG, STANDARD_CHILDREN)
        },
        "Document" => reference(
            DOCUMENT,
            &["Attribute", "Form", "TabularSection", "Template", "Command"],
        ),
        "ExchangePlan" => reference(EXCHANGE_PLAN, STANDARD_CHILDREN),
        "ChartOfCharacteristicTypes" => Spec {
            attribute_tail: &["Indexing", "Use", "FullTextSearch", "DataHistory"],
            section_tail: &["Use", "LineNumberLength"],
            ..reference(CHART_OF_CHARACTERISTIC_TYPES, STANDARD_CHILDREN)
        },
        "ChartOfAccounts" => reference(
            CHART_OF_ACCOUNTS,
            &[
                "Attribute",
                "AccountingFlag",
                "ExtDimensionAccountingFlag",
                "Form",
                "Template",
                "Command",
            ],
        ),
        "ChartOfCalculationTypes" => reference(CHART_OF_CALCULATION_TYPES, STANDARD_CHILDREN),
        "BusinessProcess" => reference(BUSINESS_PROCESS, STANDARD_CHILDREN),
        "Task" => reference(
            TASK,
            &[
                "Attribute",
                "TabularSection",
                "Form",
                "AddressingAttribute",
                "Template",
                "Command",
            ],
        ),
        "Report" | "DataProcessor" => Spec {
            properties: if kind == "Report" {
                REPORT
            } else {
                DATA_PROCESSOR
            },
            children: STANDARD_CHILDREN,
            attribute_tail: &[],
            attribute_fill: false,
            section_attribute_fill: true,
            section_attribute_tail: &[],
            section_tail: &[],
        },
        "Enum" => Spec {
            children: &["EnumValue", "Form", "Template", "Command"],
            ..reference(ENUM, STANDARD_CHILDREN)
        },
        other => bail!("{other} is not a reference object"),
    })
}

const CATALOG: &[&str] = &[
    "Name",
    "Synonym",
    "Comment",
    "Hierarchical",
    "HierarchyType",
    "LimitLevelCount",
    "LevelCount",
    "FoldersOnTop",
    "UseStandardCommands",
    "Owners",
    "SubordinationUse",
    "CodeLength",
    "DescriptionLength",
    "CodeType",
    "CodeAllowedLength",
    "CodeSeries",
    "CheckUnique",
    "Autonumbering",
    "DefaultPresentation",
    "StandardAttributes",
    "Characteristics",
    "PredefinedDataUpdate",
    "EditType",
    "QuickChoice",
    "ChoiceMode",
    "InputByString",
    "SearchStringModeOnInputByString",
    "FullTextSearchOnInputByString",
    "ChoiceDataGetModeOnInputByString",
    "DefaultObjectForm",
    "DefaultFolderForm",
    "DefaultListForm",
    "DefaultChoiceForm",
    "DefaultFolderChoiceForm",
    "AuxiliaryObjectForm",
    "AuxiliaryFolderForm",
    "AuxiliaryListForm",
    "AuxiliaryChoiceForm",
    "AuxiliaryFolderChoiceForm",
    "IncludeHelpInContents",
    "BasedOn",
    "DataLockFields",
    "DataLockControlMode",
    "FullTextSearch",
    "ObjectPresentation",
    "ExtendedObjectPresentation",
    "ListPresentation",
    "ExtendedListPresentation",
    "Explanation",
    "CreateOnInput",
    "ChoiceHistoryOnInput",
    "DataHistory",
    "UpdateDataHistoryImmediatelyAfterWrite",
    "ExecuteAfterWriteDataHistoryVersionProcessing",
];

const DOCUMENT: &[&str] = &[
    "Name",
    "Synonym",
    "Comment",
    "UseStandardCommands",
    "Numerator",
    "NumberType",
    "NumberLength",
    "NumberAllowedLength",
    "NumberPeriodicity",
    "CheckUnique",
    "Autonumbering",
    "StandardAttributes",
    "Characteristics",
    "BasedOn",
    "InputByString",
    "CreateOnInput",
    "SearchStringModeOnInputByString",
    "FullTextSearchOnInputByString",
    "ChoiceDataGetModeOnInputByString",
    "DefaultObjectForm",
    "DefaultListForm",
    "DefaultChoiceForm",
    "AuxiliaryObjectForm",
    "AuxiliaryListForm",
    "AuxiliaryChoiceForm",
    "Posting",
    "RealTimePosting",
    "RegisterRecordsDeletion",
    "RegisterRecordsWritingOnPost",
    "SequenceFilling",
    "RegisterRecords",
    "PostInPrivilegedMode",
    "UnpostInPrivilegedMode",
    "IncludeHelpInContents",
    "DataLockFields",
    "DataLockControlMode",
    "FullTextSearch",
    "ObjectPresentation",
    "ExtendedObjectPresentation",
    "ListPresentation",
    "ExtendedListPresentation",
    "Explanation",
    "ChoiceHistoryOnInput",
    "DataHistory",
    "UpdateDataHistoryImmediatelyAfterWrite",
    "ExecuteAfterWriteDataHistoryVersionProcessing",
];

const EXCHANGE_PLAN: &[&str] = &[
    "Name",
    "Synonym",
    "Comment",
    "UseStandardCommands",
    "CodeLength",
    "CodeAllowedLength",
    "DescriptionLength",
    "DefaultPresentation",
    "EditType",
    "QuickChoice",
    "ChoiceMode",
    "InputByString",
    "SearchStringModeOnInputByString",
    "FullTextSearchOnInputByString",
    "ChoiceDataGetModeOnInputByString",
    "DefaultObjectForm",
    "DefaultListForm",
    "DefaultChoiceForm",
    "AuxiliaryObjectForm",
    "AuxiliaryListForm",
    "AuxiliaryChoiceForm",
    "StandardAttributes",
    "Characteristics",
    "BasedOn",
    "DistributedInfoBase",
    "IncludeConfigurationExtensions",
    "CreateOnInput",
    "ChoiceHistoryOnInput",
    "IncludeHelpInContents",
    "DataLockFields",
    "DataLockControlMode",
    "FullTextSearch",
    "ObjectPresentation",
    "ExtendedObjectPresentation",
    "ListPresentation",
    "ExtendedListPresentation",
    "Explanation",
    "DataHistory",
    "UpdateDataHistoryImmediatelyAfterWrite",
    "ExecuteAfterWriteDataHistoryVersionProcessing",
];

const CHART_OF_CHARACTERISTIC_TYPES: &[&str] = &[
    "Name",
    "Synonym",
    "Comment",
    "UseStandardCommands",
    "IncludeHelpInContents",
    "CharacteristicExtValues",
    "Type",
    "Hierarchical",
    "FoldersOnTop",
    "CodeLength",
    "CodeAllowedLength",
    "DescriptionLength",
    "CodeSeries",
    "CheckUnique",
    "Autonumbering",
    "DefaultPresentation",
    "StandardAttributes",
    "Characteristics",
    "PredefinedDataUpdate",
    "EditType",
    "QuickChoice",
    "ChoiceMode",
    "InputByString",
    "CreateOnInput",
    "SearchStringModeOnInputByString",
    "ChoiceDataGetModeOnInputByString",
    "FullTextSearchOnInputByString",
    "ChoiceHistoryOnInput",
    "DefaultObjectForm",
    "DefaultFolderForm",
    "DefaultListForm",
    "DefaultChoiceForm",
    "DefaultFolderChoiceForm",
    "AuxiliaryObjectForm",
    "AuxiliaryFolderForm",
    "AuxiliaryListForm",
    "AuxiliaryChoiceForm",
    "AuxiliaryFolderChoiceForm",
    "BasedOn",
    "DataLockFields",
    "DataLockControlMode",
    "FullTextSearch",
    "ObjectPresentation",
    "ExtendedObjectPresentation",
    "ListPresentation",
    "ExtendedListPresentation",
    "Explanation",
    "DataHistory",
    "UpdateDataHistoryImmediatelyAfterWrite",
    "ExecuteAfterWriteDataHistoryVersionProcessing",
];

const CHART_OF_ACCOUNTS: &[&str] = &[
    "Name",
    "Synonym",
    "Comment",
    "UseStandardCommands",
    "IncludeHelpInContents",
    "BasedOn",
    "ExtDimensionTypes",
    "MaxExtDimensionCount",
    "CodeMask",
    "CodeLength",
    "DescriptionLength",
    "CodeSeries",
    "CheckUnique",
    "DefaultPresentation",
    "StandardAttributes",
    "Characteristics",
    "StandardTabularSections",
    "PredefinedDataUpdate",
    "EditType",
    "QuickChoice",
    "ChoiceMode",
    "InputByString",
    "SearchStringModeOnInputByString",
    "FullTextSearchOnInputByString",
    "ChoiceDataGetModeOnInputByString",
    "CreateOnInput",
    "ChoiceHistoryOnInput",
    "DefaultObjectForm",
    "DefaultListForm",
    "DefaultChoiceForm",
    "AuxiliaryObjectForm",
    "AuxiliaryListForm",
    "AuxiliaryChoiceForm",
    "AutoOrderByCode",
    "OrderLength",
    "DataLockFields",
    "DataLockControlMode",
    "FullTextSearch",
    "DataHistory",
    "UpdateDataHistoryImmediatelyAfterWrite",
    "ExecuteAfterWriteDataHistoryVersionProcessing",
    "ObjectPresentation",
    "ExtendedObjectPresentation",
    "ListPresentation",
    "ExtendedListPresentation",
    "Explanation",
];

const CHART_OF_CALCULATION_TYPES: &[&str] = &[
    "Name",
    "Synonym",
    "Comment",
    "UseStandardCommands",
    "CodeLength",
    "DescriptionLength",
    "CodeType",
    "CodeAllowedLength",
    "DefaultPresentation",
    "EditType",
    "QuickChoice",
    "ChoiceMode",
    "InputByString",
    "SearchStringModeOnInputByString",
    "FullTextSearchOnInputByString",
    "ChoiceDataGetModeOnInputByString",
    "CreateOnInput",
    "ChoiceHistoryOnInput",
    "DefaultObjectForm",
    "DefaultListForm",
    "DefaultChoiceForm",
    "AuxiliaryObjectForm",
    "AuxiliaryListForm",
    "AuxiliaryChoiceForm",
    "BasedOn",
    "DependenceOnCalculationTypes",
    "BaseCalculationTypes",
    "ActionPeriodUse",
    "StandardAttributes",
    "Characteristics",
    "StandardTabularSections",
    "PredefinedDataUpdate",
    "IncludeHelpInContents",
    "DataLockFields",
    "DataLockControlMode",
    "FullTextSearch",
    "ObjectPresentation",
    "ExtendedObjectPresentation",
    "ListPresentation",
    "ExtendedListPresentation",
    "Explanation",
    "DataHistory",
    "UpdateDataHistoryImmediatelyAfterWrite",
    "ExecuteAfterWriteDataHistoryVersionProcessing",
];

const BUSINESS_PROCESS: &[&str] = &[
    "Name",
    "Synonym",
    "Comment",
    "UseStandardCommands",
    "EditType",
    "InputByString",
    "CreateOnInput",
    "SearchStringModeOnInputByString",
    "ChoiceDataGetModeOnInputByString",
    "FullTextSearchOnInputByString",
    "DefaultObjectForm",
    "DefaultListForm",
    "DefaultChoiceForm",
    "AuxiliaryObjectForm",
    "AuxiliaryListForm",
    "AuxiliaryChoiceForm",
    "ChoiceHistoryOnInput",
    "NumberType",
    "NumberLength",
    "NumberAllowedLength",
    "CheckUnique",
    "StandardAttributes",
    "Characteristics",
    "Autonumbering",
    "BasedOn",
    "NumberPeriodicity",
    "Task",
    "CreateTaskInPrivilegedMode",
    "DataLockFields",
    "DataLockControlMode",
    "IncludeHelpInContents",
    "FullTextSearch",
    "ObjectPresentation",
    "ExtendedObjectPresentation",
    "ListPresentation",
    "ExtendedListPresentation",
    "Explanation",
    "DataHistory",
    "UpdateDataHistoryImmediatelyAfterWrite",
    "ExecuteAfterWriteDataHistoryVersionProcessing",
];

const TASK: &[&str] = &[
    "Name",
    "Synonym",
    "Comment",
    "UseStandardCommands",
    "NumberType",
    "NumberLength",
    "NumberAllowedLength",
    "CheckUnique",
    "Autonumbering",
    "TaskNumberAutoPrefix",
    "DescriptionLength",
    "Addressing",
    "MainAddressingAttribute",
    "CurrentPerformer",
    "BasedOn",
    "StandardAttributes",
    "Characteristics",
    "DefaultPresentation",
    "EditType",
    "InputByString",
    "SearchStringModeOnInputByString",
    "FullTextSearchOnInputByString",
    "ChoiceDataGetModeOnInputByString",
    "CreateOnInput",
    "DefaultObjectForm",
    "DefaultListForm",
    "DefaultChoiceForm",
    "AuxiliaryObjectForm",
    "AuxiliaryListForm",
    "AuxiliaryChoiceForm",
    "ChoiceHistoryOnInput",
    "IncludeHelpInContents",
    "DataLockFields",
    "DataLockControlMode",
    "FullTextSearch",
    "ObjectPresentation",
    "ExtendedObjectPresentation",
    "ListPresentation",
    "ExtendedListPresentation",
    "Explanation",
    "DataHistory",
    "UpdateDataHistoryImmediatelyAfterWrite",
    "ExecuteAfterWriteDataHistoryVersionProcessing",
];

const REPORT: &[&str] = &[
    "Name",
    "Synonym",
    "Comment",
    "UseStandardCommands",
    "DefaultForm",
    "AuxiliaryForm",
    "MainDataCompositionSchema",
    "DefaultSettingsForm",
    "AuxiliarySettingsForm",
    "DefaultVariantForm",
    "AuxiliaryVariantForm",
    "VariantsStorage",
    "SettingsStorage",
    "IncludeHelpInContents",
    "ExtendedPresentation",
    "Explanation",
];

const DATA_PROCESSOR: &[&str] = &[
    "Name",
    "Synonym",
    "Comment",
    "UseStandardCommands",
    "DefaultForm",
    "AuxiliaryForm",
    "IncludeHelpInContents",
    "ExtendedPresentation",
    "Explanation",
];

const ENUM: &[&str] = &[
    "Name",
    "Synonym",
    "Comment",
    "UseStandardCommands",
    "StandardAttributes",
    "Characteristics",
    "QuickChoice",
    "ChoiceMode",
    "DefaultListForm",
    "DefaultChoiceForm",
    "AuxiliaryListForm",
    "AuxiliaryChoiceForm",
    "ListPresentation",
    "ExtendedListPresentation",
    "Explanation",
    "ChoiceHistoryOnInput",
];

/// Properties only the 2.21 dialect writes.
const V85_ONLY: &[&str] = &["AuxiliaryVariantForm"];

/// Properties the 2.21 dialect writes although an older row does not store
/// them.
fn xml_only_default(kind: &str, property: &str, context: &ExportContext) -> Option<Element> {
    match (kind, property) {
        ("Report", "AuxiliaryVariantForm") if context.is_v85() => Some(el(property)),
        _ => None,
    }
}

/// The name of a generated type of an object (or of its tabular section).
pub(crate) fn generated_type_name(
    kind: &str,
    category: &str,
    object: &str,
    section: Option<&str>,
) -> String {
    match (kind, category, section) {
        (_, _, Some(section)) => format!("{kind}{category}.{object}.{section}"),
        ("ChartOfCharacteristicTypes", "Characteristic", _) => format!("Characteristic.{object}"),
        (
            "ChartOfCalculationTypes",
            "DisplacingCalculationTypes"
            | "DisplacingCalculationTypesRow"
            | "BaseCalculationTypes"
            | "BaseCalculationTypesRow"
            | "LeadingCalculationTypes"
            | "LeadingCalculationTypesRow",
            _,
        ) => format!("{category}.{object}"),
        _ => format!("{kind}{category}.{object}"),
    }
}

fn generated_type_element(name: String, category: &str, type_id: &str, value_id: &str) -> Element {
    el("xr:GeneratedType")
        .attr("name", name)
        .attr("category", category)
        .child(leaf("xr:TypeId", type_id))
        .child(leaf("xr:ValueId", value_id))
}

fn modern(compat: Compat) -> bool {
    compat >= Compat(8, 3, 27)
}

fn v851(compat: Compat) -> bool {
    compat >= Compat(8, 5, 1)
}

/// How many owner-record values a slot takes.
fn slot_width(slot: &Slot, compat: Compat) -> usize {
    match slot {
        Slot::Generated(_) => 2,
        Slot::Modern(inner) => {
            if modern(compat) {
                slot_width(inner, compat)
            } else {
                0
            }
        }
        Slot::Since851(inner) => {
            if v851(compat) {
                slot_width(inner, compat)
            } else {
                0
            }
        }
        _ => 1,
    }
}

/// The object's md header from its owner record.
fn owner_header(layout: &Layout, owner: &[Brace], compat: Compat) -> Result<Header> {
    let mut position = 0;
    for slot in layout.slots {
        match slot {
            Slot::Header => return header(item(owner, position)?),
            Slot::WrappedHeader => return header(item(list(item(owner, position)?)?, 1)?),
            _ => position += slot_width(slot, compat),
        }
    }
    bail!("layout has no header slot")
}

/// What the owner-record walk collects.
struct Walk {
    properties: HashMap<&'static str, Element>,
    generated: Vec<(&'static str, String, String)>,
    this_node: Option<String>,
}

pub(crate) fn decode(kind: &str, row: &Brace, context: &ExportContext) -> Result<Element> {
    let layout = layout(kind).ok_or_else(|| anyhow!("{kind} has no layout"))?;
    let spec = spec(kind)?;
    let compat = context.compat;
    let root = list(row)?;
    if atom(item(root, 0)?)? != "1" {
        bail!("not a descriptor row: {}", short(row));
    }
    let owner_record = list(item(root, 1)?)?;
    let head = owner_header(layout, owner_record, compat)?;
    let full_name = format!("{kind}.{}", head.name);
    let owner = Owner {
        kind,
        full_name: &full_name,
    };

    let mut walk = Walk {
        properties: HashMap::new(),
        generated: Vec::new(),
        this_node: None,
    };
    let mut position = 0;
    for slot in layout.slots {
        decode_slot(
            slot,
            owner_record,
            &mut position,
            &mut walk,
            &head,
            owner,
            context,
        )
        .with_context(|| format!("slot {position} ({slot:?})"))?;
    }
    if position != owner_record.len() {
        bail!(
            "owner record has {} values, the layout reads {position}",
            owner_record.len()
        );
    }

    // InternalInfo: the exchange plan's node first, then the generated
    // types in stored order.
    let mut internal = el("InternalInfo");
    if let Some(node) = walk.this_node.take() {
        internal.children.push(leaf("xr:ThisNode", node));
    }
    for (category, type_id, value_id) in &walk.generated {
        internal.children.push(generated_type_element(
            generated_type_name(kind, category, &head.name, None),
            category,
            type_id,
            value_id,
        ));
    }

    let mut properties = el("Properties");
    for name in spec.properties {
        match walk.properties.remove(name) {
            Some(element) => properties.children.push(element),
            None if *name == "StandardAttributes" => {}
            None => match xml_only_default(kind, name, context) {
                Some(element) => properties.children.push(element),
                None if !context.is_v85() && V85_ONLY.contains(name) => {}
                None => bail!("no slot decodes <{name}>"),
            },
        }
    }
    if let Some(left) = walk.properties.keys().next() {
        bail!("decoded <{left}> has no place in the XML order");
    }

    // Child objects by kind, each collection in stored order.
    let mut collections: HashMap<&str, &Brace> = HashMap::new();
    let count = number(item(root, 2)?)? as usize;
    for collection in root.iter().skip(3).take(count) {
        let fields = list(collection)?;
        collections.insert(atom(item(fields, 0)?)?, collection);
    }
    let mut children = el("ChildObjects");
    for tag in spec.children {
        for (class, content) in layout.collections {
            if collection_tag(content) != Some(*tag) {
                continue;
            }
            let Some(stored) = collections.get(class) else {
                bail!("row has no collection {class}");
            };
            let fields = list(stored)?;
            let declared = number(item(fields, 1)?)? as usize;
            for stored_item in fields.iter().skip(2).take(declared) {
                children.children.push(decode_child(
                    kind,
                    &spec,
                    content,
                    stored_item,
                    owner,
                    context,
                )?);
            }
        }
    }

    Ok(el(kind)
        .attr("uuid", head.uuid.clone())
        .child(internal)
        .child(properties)
        .child(children))
}

/// The XML element kind a collection holds.
fn collection_tag(content: &Coll) -> Option<&'static str> {
    Some(match content {
        Coll::Templates => "Template",
        Coll::Forms => "Form",
        Coll::Commands(_) => "Command",
        Coll::Children(tag, _) => tag,
        Coll::TabularSections { .. } => "TabularSection",
        Coll::EnumValues => "EnumValue",
        Coll::Empty => return None,
    })
}

#[allow(clippy::too_many_arguments)]
fn decode_slot(
    slot: &Slot,
    record: &[Brace],
    position: &mut usize,
    walk: &mut Walk,
    head: &Header,
    owner: Owner<'_>,
    context: &ExportContext,
) -> Result<()> {
    let names = &context.names;
    let compat = context.compat;
    match *slot {
        Slot::Modern(inner) => {
            if modern(compat) {
                decode_slot(inner, record, position, walk, head, owner, context)?;
            }
            return Ok(());
        }
        Slot::Since851(inner) => {
            if v851(compat) {
                decode_slot(inner, record, position, walk, head, owner, context)?;
            }
            return Ok(());
        }
        Slot::Generated(category) => {
            let type_id = atom(item(record, *position)?)?.to_string();
            let value_id = atom(item(record, *position + 1)?)?.to_string();
            walk.generated.push((category, type_id, value_id));
            *position += 2;
            return Ok(());
        }
        _ => {}
    }
    let value = item(record, *position)?;
    *position += 1;
    if let Slot::ThisNode = slot {
        walk.this_node = Some(atom(value)?.to_string());
        return Ok(());
    }
    let mut put = |name: &'static str, element: Element| {
        walk.properties.insert(name, element);
    };
    match *slot {
        Slot::Tag(old, new, latest) => {
            let expected = if v851(compat) {
                latest
            } else if modern(compat) {
                new
            } else {
                old
            };
            let stored = number(value)?;
            if stored != expected {
                bail!("record version {stored}, the compatibility mode stores {expected}");
            }
        }
        Slot::Header | Slot::WrappedHeader => {
            let [name, synonym, comment] = header_elements(head)?;
            put("Name", name);
            put("Synonym", synonym);
            put("Comment", comment);
        }
        Slot::Flag(name) => put(name, leaf(name, bool_text(value)?)),
        Slot::Number(name) => put(name, leaf(name, atom(value)?)),
        Slot::Code(name, table) => put(name, leaf(name, code_text(value, table)?)),
        Slot::Text(name) => put(
            name,
            leaf(
                name,
                crate::metadata_model::export::xml_text(string(value)?),
            ),
        ),
        Slot::Localized(name) => put(name, localized_element(name, value)?),
        Slot::Reference(name) => put(name, leaf(name, reference_name(atom(value)?, names)?)),
        Slot::References(name) => {
            let fields = list(value)?;
            let count = number(item(fields, 1)?)? as usize;
            let mut uuids = Vec::with_capacity(count);
            for reference in fields.iter().skip(2).take(count) {
                let payload = list(item(list(reference)?, 2)?)?;
                uuids.push(atom(item(payload, 1)?)?);
            }
            // A document's register records are a set: the XML lists them
            // in uuid order even where the row kept an append order.
            if name == "RegisterRecords" {
                uuids.sort_unstable();
            }
            let mut element = el(name);
            for uuid in uuids {
                element.children.push(
                    leaf("xr:Item", reference_name(uuid, names)?).attr("type", "xr:MDObjectRef"),
                );
            }
            put(name, element);
        }
        Slot::Fields(name) => {
            let body = list(item(list(value)?, 1)?)?;
            let count = number(item(body, 1)?)? as usize;
            let mut element = el(name);
            for field in body.iter().skip(2).take(count) {
                let segment = item(list(field)?, 2)?;
                element.children.push(leaf(
                    "xr:Field",
                    field_text(segment, owner.full_name, names)?,
                ));
            }
            put(name, element);
        }
        Slot::InputModes => {
            let modes = list(value)?;
            put(
                "SearchStringModeOnInputByString",
                leaf(
                    "SearchStringModeOnInputByString",
                    code_text(item(modes, 0)?, &[("Begin", 1), ("AnyPart", 2)])?,
                ),
            );
            put(
                "FullTextSearchOnInputByString",
                leaf(
                    "FullTextSearchOnInputByString",
                    code_text(item(modes, 1)?, &[("Use", 1), ("DontUse", 2)])?,
                ),
            );
            put(
                "ChoiceDataGetModeOnInputByString",
                leaf(
                    "ChoiceDataGetModeOnInputByString",
                    code_text(item(modes, 2)?, &[("Directly", 0), ("Background", 1)])?,
                ),
            );
        }
        Slot::TypePattern(name) => put(name, type_element(name, value, names)?),
        Slot::StandardAttributes(codes) => {
            if let Some(element) =
                standard_attributes_element("StandardAttributes", value, codes, owner, context)?
            {
                put("StandardAttributes", element);
            }
        }
        Slot::StandardTabularSections(definitions) => {
            put(
                "StandardTabularSections",
                standard_tabular_sections(value, definitions, owner, context)?,
            );
        }
        Slot::Characteristics => put("Characteristics", characteristics(value, context)?),
        // Constants and nil slots carry nothing the XML writes; the
        // lossless check compares them.
        Slot::Const(_) | Slot::Nil => {}
        Slot::Generated(_) | Slot::Modern(_) | Slot::Since851(_) | Slot::ThisNode => {
            unreachable!()
        }
    }
    Ok(())
}

/// `{1,{0,<n>,<marker>,{3,<synonym>,"comment",<fill>,0,<attrs>,<tooltip>}...}}`.
fn standard_tabular_sections(
    node: &Brace,
    definitions: super::parts::StandardSections,
    owner: Owner<'_>,
    context: &ExportContext,
) -> Result<Element> {
    let mut element = el("StandardTabularSections");
    let fields = list(node)?;
    if atom(item(fields, 0)?)? == "0" {
        return Ok(element);
    }
    let body = list(item(fields, 1)?)?;
    let count = number(item(body, 1)?)? as usize;
    for index in 0..count {
        let marker = number(item(body, 2 + 2 * index)?)?;
        let section = list(item(body, 3 + 2 * index)?)?;
        let (name, _, codes) = definitions
            .iter()
            .find(|(_, candidate, _)| *candidate == marker)
            .ok_or_else(|| anyhow!("unknown standard tabular section {marker}"))?;
        let mut xml = el("xr:StandardTabularSection")
            .attr("name", *name)
            .child(localized_element("xr:Synonym", item(section, 1)?)?)
            .child(leaf(
                "xr:Comment",
                crate::metadata_model::export::xml_text(string(item(section, 2)?)?),
            ))
            .child(localized_element("xr:ToolTip", item(section, 6)?)?)
            .child(leaf(
                "xr:FillChecking",
                code_text(item(section, 3)?, super::parts::FILL_CHECKING)?,
            ));
        let attributes = item(section, 5)?;
        let mut block = el("xr:StandardAttributes");
        if list(attributes)?.len() > 1 {
            block.children.extend(standard_attribute_elements(
                attributes, codes, owner, context,
            )?);
        }
        xml.children.push(block);
        element.children.push(xml);
    }
    Ok(element)
}

/// `{0,{<n>,{"#",<characteristic>,{4,...}}...}}` -> `<Characteristics>`.
fn characteristics(node: &Brace, context: &ExportContext) -> Result<Element> {
    let names = &context.names;
    let mut element = el("Characteristics");
    let body = list(item(list(node)?, 1)?)?;
    let count = number(item(body, 0)?)? as usize;
    for typed in body.iter().skip(1).take(count) {
        let fields = list(item(list(typed)?, 2)?)?;
        let source = |index: usize| -> Result<String> {
            let reference = list(item(fields, index)?)?;
            reference_name(atom(item(reference, 1)?)?, names)
        };
        let types_from = source(1)?;
        let values_from = source(2)?;
        let field = |index: usize, base: &str| -> Result<String> {
            match fields.get(index) {
                // Layouts that stop short write the undefined sentinel.
                None => Ok("-1".to_string()),
                Some(value) => field_text(item(list(value)?, 1)?, base, names),
            }
        };
        let types = el("xr:CharacteristicTypes")
            .attr("from", types_from.clone())
            .child(leaf("xr:KeyField", field(8, &types_from)?))
            .child(leaf("xr:TypesFilterField", field(6, &types_from)?))
            .child(value_element(
                "xr:TypesFilterValue",
                item(fields, 7)?,
                names,
            )?)
            .child(leaf("xr:DataPathField", field(9, &types_from)?))
            .child(leaf("xr:MultipleValuesUseField", field(10, &types_from)?));
        let values = el("xr:CharacteristicValues")
            .attr("from", values_from.clone())
            .child(leaf("xr:ObjectField", field(3, &values_from)?))
            .child(leaf("xr:TypeField", field(4, &values_from)?))
            .child(leaf("xr:ValueField", field(5, &values_from)?))
            .child(leaf("xr:MultipleValuesKeyField", field(11, &values_from)?))
            .child(leaf(
                "xr:MultipleValuesOrderField",
                field(12, &values_from)?,
            ));
        element
            .children
            .push(el("xr:Characteristic").child(types).child(values));
    }
    Ok(element)
}

fn decode_child(
    kind: &str,
    spec: &Spec,
    content: &Coll,
    stored: &Brace,
    owner: Owner<'_>,
    context: &ExportContext,
) -> Result<Element> {
    let names = &context.names;
    let modern = modern(context.compat);
    match *content {
        Coll::Forms | Coll::Templates => {
            let tag = if matches!(content, Coll::Forms) {
                "Form"
            } else {
                "Template"
            };
            let uuid = atom(stored)?;
            let full = names
                .name(uuid)
                .ok_or_else(|| anyhow!("no name for {tag} {uuid}"))?;
            let prefix = format!("{}.{tag}.", owner.full_name);
            let short_name = full
                .strip_prefix(&prefix)
                .ok_or_else(|| anyhow!("{tag} {full} is not owned by {}", owner.full_name))?;
            Ok(leaf(tag, short_name))
        }
        Coll::Commands(wrapper) => {
            let inner = item(list(item(list(stored)?, 0)?)?, 1)?;
            let body = match wrapper {
                CommandWrapper::Owner => item(list(inner)?, 3)?,
                CommandWrapper::Bare => inner,
            };
            command(body, context)
        }
        Coll::Children(tag, wrapper) => {
            let record = list(item(list(stored)?, 0)?)?;
            let wrapper = if modern { wrapper.modern } else { wrapper.old };
            let (with_fill, tail) = match tag {
                "Attribute" => (spec.attribute_fill, spec.attribute_tail),
                "AccountingFlag" | "ExtDimensionAccountingFlag" => (true, &["DataHistory"][..]),
                "AddressingAttribute" => (
                    true,
                    &[
                        "Indexing",
                        "AddressingDimension",
                        "FullTextSearch",
                        "DataHistory",
                    ][..],
                ),
                other => bail!("unknown attribute kind {other}"),
            };
            attribute(tag, record, wrapper, with_fill, tail, owner, context)
        }
        Coll::TabularSections {
            wrapper,
            attribute: attribute_wrapper,
            ..
        } => {
            let wrapper = if modern { wrapper.modern } else { wrapper.old };
            tabular_section(
                kind,
                spec,
                stored,
                wrapper,
                attribute_wrapper,
                owner,
                context,
            )
        }
        Coll::EnumValues => {
            let record = list(item(list(stored)?, 0)?)?;
            let head = header(item(record, 1)?)?;
            let [name, synonym, comment] = header_elements(&head)?;
            let mut properties = el("Properties").child(name).child(synonym).child(comment);
            if context.is_v85() {
                let color = match record.get(2) {
                    Some(color) => enum_value_color(color)?,
                    None => "auto".to_string(),
                };
                properties.children.push(leaf("Color", color));
            }
            Ok(el("EnumValue").attr("uuid", head.uuid).child(properties))
        }
        Coll::Empty => bail!("an empty collection holds items"),
    }
}

/// `{4,4,{0},4}` -> `auto`; `{4,4,{<index>},5}` -> `pal:<name>`.
fn enum_value_color(node: &Brace) -> Result<String> {
    let fields = list(node)?;
    let index = number(item(list(item(fields, 2)?)?, 0)?)?;
    Ok(match (atom(item(fields, 3)?)?, index) {
        ("4", 0) => "auto",
        ("5", 0) => "pal:FirstBrand",
        ("5", 1) => "pal:SecondBrand",
        ("5", 2) => "pal:Red",
        ("5", 3) => "pal:Orange",
        ("5", 4) => "pal:Yellow",
        ("5", 5) => "pal:Green",
        ("5", 6) => "pal:LightBlue",
        ("5", 7) => "pal:Blue",
        ("5", 15) => "pal:Gray",
        _ => bail!("unsupported enum value colour {}", short(node)),
    }
    .to_string())
}

const INDEXING: &[(&str, i64)] = &[
    ("DontIndex", 0),
    ("Index", 1),
    ("IndexWithAdditionalOrder", 2),
];
const ATTRIBUTE_USE: &[(&str, i64)] = &[("ForItem", 0), ("ForFolder", 1), ("ForFolderAndItem", 2)];
const USE: &[(&str, i64)] = &[("DontUse", 0), ("Use", 1)];

/// An attribute-like child: its wrapper's own slots and the shared body.
fn attribute(
    tag: &str,
    record: &[Brace],
    wrapper: AttributeWrapper,
    with_fill: bool,
    tail_order: &[&str],
    owner: Owner<'_>,
    context: &ExportContext,
) -> Result<Element> {
    let body = item(record, 1)?;
    let head = attribute_header(body)?;
    let mut tail: HashMap<&str, Element> = HashMap::new();
    let mut indexing = |value: &Brace| -> Result<()> {
        tail.insert("Indexing", leaf("Indexing", code_text(value, INDEXING)?));
        Ok(())
    };
    match wrapper {
        AttributeWrapper::Bare => {}
        AttributeWrapper::Plain(_)
        | AttributeWrapper::PlainModern(_)
        | AttributeWrapper::TabularSection => {
            indexing(item(record, 2)?)?;
            tail.insert(
                "FullTextSearch",
                leaf("FullTextSearch", code_text(item(record, 3)?, USE)?),
            );
            tail.insert(
                "DataHistory",
                leaf("DataHistory", code_text(item(record, 4)?, USE)?),
            );
        }
        AttributeWrapper::Hierarchical(_) | AttributeWrapper::HierarchicalModern(_) => {
            indexing(item(record, 2)?)?;
            tail.insert(
                "Use",
                leaf("Use", code_text(item(record, 3)?, ATTRIBUTE_USE)?),
            );
            tail.insert(
                "FullTextSearch",
                leaf("FullTextSearch", code_text(item(record, 4)?, USE)?),
            );
            tail.insert(
                "DataHistory",
                leaf("DataHistory", code_text(item(record, 5)?, USE)?),
            );
        }
        AttributeWrapper::Addressing(_) => {
            indexing(item(record, 2)?)?;
            tail.insert(
                "AddressingDimension",
                leaf(
                    "AddressingDimension",
                    reference_name(atom(item(record, 3)?)?, &context.names)?,
                ),
            );
            tail.insert(
                "FullTextSearch",
                leaf("FullTextSearch", code_text(item(record, 4)?, USE)?),
            );
            tail.insert(
                "DataHistory",
                leaf("DataHistory", code_text(item(record, 5)?, USE)?),
            );
        }
        AttributeWrapper::Flag(_) => {
            tail.insert(
                "DataHistory",
                leaf("DataHistory", code_text(item(record, 2)?, USE)?),
            );
        }
    }
    let mut ordered = Vec::with_capacity(tail_order.len());
    for name in tail_order {
        ordered.push(
            tail.remove(name)
                .ok_or_else(|| anyhow!("attribute wrapper has no <{name}>"))?,
        );
    }
    let properties = attribute_properties(body, with_fill, ordered, owner, context)?;
    Ok(el(tag).attr("uuid", head.uuid).child(properties))
}

/// `{<wrapped>,1,{<attribute class>,<n>,<attribute>...}}`.
#[allow(clippy::too_many_arguments)]
fn tabular_section(
    kind: &str,
    spec: &Spec,
    stored: &Brace,
    wrapper: TsWrapper,
    attribute_wrapper: AttributeWrapper,
    owner: Owner<'_>,
    context: &ExportContext,
) -> Result<Element> {
    let fields = list(stored)?;
    let wrapped = list(item(fields, 0)?)?;
    let record = list(item(wrapped, 1)?)?;
    if atom(item(record, 0)?)? != "11" {
        bail!("not a tabular section record: {}", short(item(wrapped, 1)?));
    }
    let head = header(item(list(item(record, 5)?)?, 1)?)?;
    let object_name = owner
        .full_name
        .split_once('.')
        .map(|(_, name)| name)
        .unwrap_or_default();
    let internal = el("InternalInfo")
        .child(generated_type_element(
            generated_type_name(kind, "TabularSection", object_name, Some(&head.name)),
            "TabularSection",
            atom(item(record, 1)?)?,
            atom(item(record, 2)?)?,
        ))
        .child(generated_type_element(
            generated_type_name(kind, "TabularSectionRow", object_name, Some(&head.name)),
            "TabularSectionRow",
            atom(item(record, 3)?)?,
            atom(item(record, 4)?)?,
        ));
    let [name, synonym, comment] = header_elements(&head)?;
    let mut properties = el("Properties")
        .child(name)
        .child(synonym)
        .child(comment)
        .child(localized_element("ToolTip", item(record, 8)?)?)
        .child(leaf(
            "FillChecking",
            code_text(item(record, 6)?, super::parts::FILL_CHECKING)?,
        ));
    let codes = [("LineNumber", line_number_marker(kind))];
    if let Some(block) = standard_attributes_element(
        "StandardAttributes",
        item(record, 7)?,
        &codes,
        owner,
        context,
    )? {
        properties.children.push(block);
    }
    let (usage, length) = match wrapper {
        TsWrapper::Bare(_) => (None, None),
        TsWrapper::Use(_) => (Some(item(wrapped, 2)?), None),
        TsWrapper::Length(_) => (None, Some(item(wrapped, 2)?)),
        TsWrapper::UseAndLength(_) => (Some(item(wrapped, 2)?), Some(item(wrapped, 3)?)),
    };
    for property in spec.section_tail {
        match *property {
            "Use" => {
                let usage = usage.ok_or_else(|| anyhow!("tabular section wrapper has no Use"))?;
                properties
                    .children
                    .push(leaf("Use", code_text(usage, ATTRIBUTE_USE)?));
            }
            "LineNumberLength" => {
                let length = match length {
                    Some(length) => atom(length)?.to_string(),
                    // Rows before 8.3.27 do not store it; the XML writes the default.
                    None => "5".to_string(),
                };
                properties.children.push(leaf("LineNumberLength", length));
            }
            other => bail!("unknown tabular section property {other}"),
        }
    }
    let attributes = list(item(fields, 2)?)?;
    let count = number(item(attributes, 1)?)? as usize;
    let mut children = el("ChildObjects");
    for stored_attribute in attributes.iter().skip(2).take(count) {
        let record = list(item(list(stored_attribute)?, 0)?)?;
        children.children.push(attribute(
            "Attribute",
            record,
            attribute_wrapper,
            spec.section_attribute_fill,
            spec.section_attribute_tail,
            owner,
            context,
        )?);
    }
    Ok(el("TabularSection")
        .attr("uuid", head.uuid)
        .child(internal)
        .child(properties)
        .child(children))
}

// ---------------------------------------------------------------------------
// Commands.

fn builtin_command_group(uuid: &str) -> Option<&'static str> {
    Some(match uuid {
        "77ea1b8f-dd79-4717-9dba-5628e7f348cf" => "NavigationPanelOrdinary",
        "bc80566a-86a5-4e87-acd4-872239385a2e" => "NavigationPanelSeeAlso",
        "1af6d528-0b86-4fba-ab95-bd7475db03ba" => "NavigationPanelImportant",
        "4f499c31-050b-47c5-aa84-d0366c0a0da8" => "ActionsPanelCreate",
        "5b360bff-01a1-49b6-93d2-26e7e8e3a038" => "ActionsPanelReports",
        "aabb34e1-98c1-4bd0-bf7f-243f95437b44" => "ActionsPanelTools",
        "dc2ade0f-383e-4c78-85f2-c0dabc0e2dc0" => "FormCommandBarCreateBasedOn",
        "cb50f5c0-8013-4262-93a2-f0db379d6b6b" => "FormCommandBarImportant",
        "eacad741-96b9-4b3a-bf79-dde9ecead1a1" => "FormNavigationPanelGoTo",
        "8ab1540c-0bfa-4fa6-a1e1-5d5069efc7d8" => "FormNavigationPanelSeeAlso",
        "dc11a6be-de1f-4b64-a7a5-9b17bf4ec9f2" => "FormNavigationPanelImportant",
        _ => return None,
    })
}

fn std_picture_by_code(code: i64) -> Option<&'static str> {
    Some(match code {
        -1 => "InputFieldSelect",
        -2 => "InputFieldClear",
        -3 => "MoveUp",
        -4 => "MoveDown",
        -5 => "InputFieldCalendar",
        -7 => "InputFieldOpen",
        -8 => "MoveLeft",
        -9 => "MoveRight",
        -10 => "CheckAll",
        -11 => "UncheckAll",
        -13 => "Print",
        -14 => "InputFieldChooseType",
        -15 => "ZoomOut",
        -16 => "ZoomIn",
        -100 => "Select",
        _ => return None,
    })
}

/// `{4,<present>,<value>,"",<x>,<y>,<load transparent>,0,""}` -> `<Picture>`.
fn picture(node: &Brace, context: &ExportContext) -> Result<Element> {
    let fields = list(node)?;
    if atom(item(fields, 1)?)? == "0" {
        return Ok(el("Picture"));
    }
    let value = list(item(fields, 2)?)?;
    let reference = match value {
        [code] => {
            let code = number(code)?;
            format!(
                "StdPicture.{}",
                std_picture_by_code(code).ok_or_else(|| anyhow!("unknown picture code {code}"))?
            )
        }
        [_, uuid] => {
            let uuid = atom(uuid)?;
            match STANDARD_PICTURES
                .iter()
                .find_map(|(id, name)| (*id == uuid).then_some(*name))
            {
                Some(name) => name.to_string(),
                None => context
                    .names
                    .name(uuid)
                    .map(str::to_string)
                    .ok_or_else(|| anyhow!("no name for picture {uuid}"))?,
            }
        }
        _ => bail!("unsupported picture {}", short(node)),
    };
    let mut element = el("Picture")
        .child(leaf("xr:Ref", reference))
        .child(leaf("xr:LoadTransparent", bool_text(item(fields, 6)?)?));
    let x = atom(item(fields, 4)?)?;
    let y = atom(item(fields, 5)?)?;
    if x != "-1" || y != "-1" {
        element
            .children
            .push(el("xr:TransparentPixel").attr("x", x).attr("y", y));
    }
    Ok(element)
}

/// `{0,<key>,<modifiers>}` -> `Ctrl+Alt+Shift+Key`; empty for `{0,0,0}`.
pub(crate) fn shortcut(node: &Brace) -> Result<String> {
    let fields = list(node)?;
    let code = number(item(fields, 1)?)?;
    let mask = number(item(fields, 2)?)?;
    if code == 0 {
        return Ok(String::new());
    }
    let key = match code {
        8 => "BackSpace".to_string(),
        9 => "Tab".to_string(),
        13 => "Enter".to_string(),
        27 => "Esc".to_string(),
        32 => "Space".to_string(),
        33 => "PageUp".to_string(),
        34 => "PageDown".to_string(),
        35 => "End".to_string(),
        36 => "Home".to_string(),
        37 => "Left".to_string(),
        38 => "Up".to_string(),
        39 => "Right".to_string(),
        40 => "Down".to_string(),
        45 => "Insert".to_string(),
        46 => "Delete".to_string(),
        48..=57 | 65..=90 => char::from(code as u8).to_string(),
        96..=105 => format!("Num {}", code - 96),
        106 => "Num *".to_string(),
        107 => "Num +".to_string(),
        109 => "Num -".to_string(),
        110 => "Num .".to_string(),
        111 => "Num /".to_string(),
        112..=123 => format!("F{}", code - 111),
        other => bail!("unknown shortcut key {other}"),
    };
    let mut parts = Vec::new();
    if mask & 8 != 0 {
        parts.push("Ctrl".to_string());
    }
    if mask & 16 != 0 {
        parts.push("Alt".to_string());
    }
    if mask & 4 != 0 {
        parts.push("Shift".to_string());
    }
    parts.push(key);
    Ok(parts.join("+"))
}

/// `{1,{2,<uuid>,<command value>},{9,...}}` -> `<Command>`.
pub(crate) fn command(body: &Brace, context: &ExportContext) -> Result<Element> {
    let names = &context.names;
    let fields = list(body)?;
    let identity = list(item(fields, 1)?)?;
    let uuid = atom(item(identity, 1)?)?;
    let record = list(item(fields, 2)?)?;
    if atom(item(record, 0)?)? != "9" {
        bail!("not a command record: {}", short(item(fields, 2)?));
    }
    let head = header(item(record, 9)?)?;
    let [name, synonym, comment] = header_elements(&head)?;
    let group_uuid = atom(item(list(item(record, 7)?)?, 1)?)?;
    let group = match builtin_command_group(group_uuid) {
        Some(group) => group.to_string(),
        None => reference_name(group_uuid, names)?,
    };
    let properties = el("Properties")
        .child(name)
        .child(synonym)
        .child(comment)
        .child(leaf("Group", group))
        .child(type_element(
            "CommandParameterType",
            item(record, 8)?,
            names,
        )?)
        .child(leaf(
            "ParameterUseMode",
            code_text(item(record, 11)?, &[("Single", 0), ("Multiple", 1)])?,
        ))
        .child(leaf("ModifiesData", bool_text(item(record, 10)?)?))
        .child(leaf(
            "Representation",
            code_text(
                item(record, 2)?,
                &[
                    ("Text", 0),
                    ("Picture", 1),
                    ("PictureAndText", 2),
                    ("Auto", 3),
                ],
            )?,
        ))
        .child(localized_element("ToolTip", item(record, 3)?)?)
        .child(picture(item(record, 1)?, context)?)
        .child(leaf("Shortcut", shortcut(item(record, 5)?)?))
        .child(leaf(
            "OnMainServerUnavalableBehavior",
            code_text(item(record, 12)?, &[("Auto", 0)])?,
        ));
    Ok(el("Command").attr("uuid", uuid).child(properties))
}

// ---------------------------------------------------------------------------
// Names.

/// What a reference object's row contributes to a name index.
pub(crate) fn names(kind: &str, row: &Brace) -> Result<ObjectNames> {
    let layout = layout(kind).ok_or_else(|| anyhow!("{kind} has no layout"))?;
    let root = list(row)?;
    let owner_record = list(item(root, 1)?)?;
    // The record versions only shift slots that follow every name bearer,
    // so the widest reading is exact for the names.
    let compat = Compat(8, 5, 1);
    let head = owner_header(layout, owner_record, compat)?;
    let object_name = head.name.clone();
    let full_name = format!("{kind}.{object_name}");
    let mut out = ObjectNames {
        uuid: head.uuid.clone(),
        full_name: full_name.clone(),
        children: Vec::new(),
        types: Vec::new(),
    };
    let mut position = 0;
    for slot in layout.slots {
        if let Slot::Generated(category) = slot {
            out.types.push(GeneratedTypeName {
                name: generated_type_name(kind, category, &head.name, None),
                category: category.to_string(),
                type_id: atom(item(owner_record, position)?)?.to_string(),
                value_id: atom(item(owner_record, position + 1)?)?.to_string(),
            });
        }
        position += slot_width(slot, compat);
        if position >= owner_record.len() {
            break;
        }
    }
    let count = number(item(root, 2)?)? as usize;
    let collections: HashMap<&str, &Brace> = root
        .iter()
        .skip(3)
        .take(count)
        .filter_map(|collection| {
            let fields = collection.as_list()?;
            Some((fields.first()?.as_atom()?, collection))
        })
        .collect();
    for (class, content) in layout.collections {
        let Some(stored) = collections.get(class) else {
            continue;
        };
        let fields = list(stored)?;
        let declared = number(item(fields, 1)?)? as usize;
        for stored_item in fields.iter().skip(2).take(declared) {
            match content {
                Coll::Children(tag, _) => {
                    let record = list(item(list(stored_item)?, 0)?)?;
                    let head = attribute_header(item(record, 1)?)?;
                    out.children
                        .push((format!("{full_name}.{tag}.{}", head.name), head.uuid));
                }
                Coll::Commands(wrapper) => {
                    let inner = item(list(item(list(stored_item)?, 0)?)?, 1)?;
                    let body = match wrapper {
                        CommandWrapper::Owner => item(list(inner)?, 3)?,
                        CommandWrapper::Bare => inner,
                    };
                    let record = list(item(list(body)?, 2)?)?;
                    let head = header(item(record, 9)?)?;
                    out.children
                        .push((format!("{full_name}.Command.{}", head.name), head.uuid));
                }
                Coll::EnumValues => {
                    let record = list(item(list(stored_item)?, 0)?)?;
                    let head = header(item(record, 1)?)?;
                    out.children
                        .push((format!("{full_name}.EnumValue.{}", head.name), head.uuid));
                }
                Coll::TabularSections { .. } => {
                    let section = list(stored_item)?;
                    let record = list(item(list(item(section, 0)?)?, 1)?)?;
                    let head = header(item(list(item(record, 5)?)?, 1)?)?;
                    let section_full = format!("{full_name}.TabularSection.{}", head.name);
                    for (category, type_index) in [("TabularSection", 1), ("TabularSectionRow", 3)]
                    {
                        out.types.push(GeneratedTypeName {
                            name: generated_type_name(
                                kind,
                                category,
                                &object_name,
                                Some(&head.name),
                            ),
                            category: category.to_string(),
                            type_id: atom(item(record, type_index)?)?.to_string(),
                            value_id: atom(item(record, type_index + 1)?)?.to_string(),
                        });
                    }
                    out.children.push((section_full.clone(), head.uuid));
                    let attributes = list(item(section, 2)?)?;
                    let count = number(item(attributes, 1)?)? as usize;
                    for stored_attribute in attributes.iter().skip(2).take(count) {
                        let record = list(item(list(stored_attribute)?, 0)?)?;
                        let head = attribute_header(item(record, 1)?)?;
                        out.children
                            .push((format!("{section_full}.Attribute.{}", head.name), head.uuid));
                    }
                }
                Coll::Forms | Coll::Templates | Coll::Empty => {}
            }
        }
    }
    Ok(out)
}

/// Platform pictures by uuid, as the exporter names them (from
/// `mssql_dump::STANDARD_PICTURES`).
pub(crate) const STANDARD_PICTURES: &[(&str, &str)] = &[
    (
        "4b54770b-d069-4c0e-9b17-5cc2a01134d9",
        "StdPicture.Information",
    ),
    (
        "818ab7d0-4654-4542-bd5e-fd9d1352b5a1",
        "StdPicture.SaveFile",
    ),
    ("6ff3ddbd-56e3-4ddf-a5bf-048c1e2dfb2f", "StdPicture.User"),
    (
        "283ecabd-aaed-41d1-ad46-6cca91c29120",
        "StdPicture.LoadReportSettings",
    ),
    (
        "942e0303-a3ec-4fe8-887c-5aea8516d424",
        "StdPicture.ReportSettings",
    ),
    (
        "6a248caf-0a7b-46ad-a595-74890ea202f7",
        "StdPicture.Conversations",
    ),
    (
        "5b87ad1b-d8cc-43c1-b5c4-dc43613c518c",
        "StdPicture.InformationRegister",
    ),
    (
        "a064544f-6037-48ca-b19f-8ad63e43af23",
        "StdPicture.ShowData",
    ),
    (
        "f04794cb-c198-4172-86c3-649386013c85",
        "StdPicture.CustomizeList",
    ),
    ("97b2cc97-d5c6-45fb-9824-9d6d73db21fe", "StdPicture.Change"),
    ("37cf7cc0-abad-4385-b597-6fd2d8dc085a", "StdPicture.Task"),
    (
        "2f130057-bb2a-4e22-bba5-e108fac26940",
        "StdPicture.ChooseValue",
    ),
    (
        "47f01799-7968-4f44-9acc-fe1bdde8beb2",
        "StdPicture.ActiveUsers",
    ),
    (
        "e8a49985-fef7-45a9-b6bb-ddd2b9028172",
        "StdPicture.DataHistory",
    ),
    (
        "a24cff7f-a1a5-4403-af82-a7b31852cde9",
        "StdPicture.BusinessProcessObject",
    ),
    (
        "f6532868-30b9-44ab-803c-78f0f0b06b02",
        "StdPicture.CloneObject",
    ),
    (
        "448d6f55-d885-496c-870d-d1bd78374745",
        "StdPicture.CloneListItem",
    ),
    (
        "977e831a-0e73-4d60-af51-091a6fa8612e",
        "StdPicture.CreateListItem",
    ),
    (
        "0e2da390-5c04-46a2-a74b-1b7e13a40f2b",
        "StdPicture.HidePassword",
    ),
    (
        "97f87955-b88a-4225-a0d8-03af981ecd86",
        "StdPicture.ShowPassword",
    ),
    (
        "fc6a06a8-1308-4385-b1b2-9d302d2054ed",
        "StdPicture.SearchControl",
    ),
    (
        "723765ab-0b92-4745-a621-1ba0f77c92c9",
        "StdPicture.EventLog",
    ),
    (
        "4fddea39-5129-4b4c-83fe-4e443cd61940",
        "StdPicture.EventLogByUser",
    ),
    ("ffab30f1-da11-44b5-b34c-24da22badcf4", "StdPicture.Find"),
    (
        "785362cb-3756-48ed-87d2-292ded17054a",
        "StdPicture.OpenFile",
    ),
    (
        "4d2570b5-205f-413c-b4cc-b2097f61684f",
        "StdPicture.CreateInitialImage",
    ),
    (
        "0ce78048-0196-4f80-a781-9829cdb7f43e",
        "StdPicture.GenerateReport",
    ),
    (
        "4bf9fbb5-53c5-4b09-bab2-d69bbfab945b",
        "StdPicture.Dendrogram",
    ),
    (
        "18492a87-2fe4-44af-b218-304897fed020",
        "StdPicture.MarkToDelete",
    ),
    ("20ebc47b-f4d9-439c-acd3-fdc624fbac2a", "StdPicture.Post"),
    (
        "23f940bf-7381-4c2b-85a1-e541ed428042",
        "StdPicture.SaveValues",
    ),
    (
        "a7707ed1-39b0-418f-974d-4d500d27a9c6",
        "StdPicture.RestoreValues",
    ),
    ("8f29e0e2-d5e6-41e8-a34d-9a0288156322", "StdPicture.Reread"),
    ("db817ee1-fd28-4e7f-bb4a-53686b2b153c", "StdPicture.Report"),
    (
        "1970a480-9b38-405e-9d9e-8209f3fad5f1",
        "StdPicture.ScheduledJob",
    ),
    (
        "58174855-39be-462e-8723-cb2d95182146",
        "StdPicture.SetDateInterval",
    ),
    (
        "2ef82795-06fe-4365-bd0c-44b486264620",
        "StdPicture.FilterCriterion",
    ),
    (
        "b1406535-6cc2-4410-95ea-753556e8460f",
        "StdPicture.FilterByCurrentValue",
    ),
    (
        "479470e0-ea0f-4266-8549-e2b1e8c06534",
        "StdPicture.ClearFilter",
    ),
    (
        "fb7e9fb5-110b-41cb-adc6-753969ae1c81",
        "StdPicture.ExpandAll",
    ),
    (
        "27ee3053-952c-49e5-8261-9215098e0e9c",
        "StdPicture.CollapseAll",
    ),
    (
        "5289d9a4-b012-4d54-9bce-50473fe29b57",
        "StdPicture.DialogExclamation",
    ),
    ("55ef0776-5ee4-4daf-9a9b-70d63643ab8d", "StdPicture.SetTime"),
    ("fc4f29e0-d168-4fe0-8e64-e982fabf2595", "StdPicture.Refresh"),
    (
        "91022b99-b610-48ad-954e-a297848081ce",
        "StdPicture.SortListAsc",
    ),
    (
        "1fa32fdb-a180-418f-a6eb-db7516b7a30b",
        "StdPicture.SortListDesc",
    ),
    (
        "894afc03-9904-465d-b671-f555ffb9b21c",
        "StdPicture.Document",
    ),
    ("1cd7b762-ec6a-4e92-ac9a-1832be228ec3", "StdPicture.Stop"),
    (
        "8ca4ea33-603d-4992-8a41-c7924b5bd40b",
        "StdPicture.UndoPosting",
    ),
    ("894cf65b-4109-4533-a1d7-c87b1fcc80a3", "StdPicture.Write"),
    (
        "e6fc55a0-3d58-4b15-bdd3-717453929598",
        "StdPicture.WriteAndClose",
    ),
    ("08a45a70-c221-4339-b3b1-9f11cb22147d", "StdPicture.Delete"),
    (
        "6e3687cf-a8d1-446a-833a-bfaf38516353",
        "StdPicture.SwitchActivity",
    ),
    (
        "7a9cd2fd-6372-4342-9a9e-3ebbd754fd83",
        "StdPicture.AppearanceCheckBox",
    ),
    (
        "0c1f7756-6143-4903-a94c-8f22c85e44de",
        "StdPicture.Attribute",
    ),
    ("3c904ff7-1195-4a7c-9a38-7b1f6ca49cce", "StdPicture.Back"),
    (
        "509c4a7f-6406-4388-bb8c-bc81fb5131aa",
        "StdPicture.BusinessProcess",
    ),
    (
        "97c5a6d5-47ed-43f9-8c8c-10e9903c23d2",
        "StdPicture.Calendar",
    ),
    (
        "4ab0e87f-7d9b-4aa8-ac4b-680a78522da8",
        "StdPicture.CreateFolder",
    ),
    (
        "ee7c4a5b-2d9b-4087-ae3e-947792085f09",
        "StdPicture.DataCompositionOutputParameters",
    ),
    (
        "544fdbe8-5956-4512-bc62-93b4c022d291",
        "StdPicture.ExchangePlan",
    ),
    (
        "003024ed-fa25-42ac-9f53-f5014e383801",
        "StdPicture.ExecuteTask",
    ),
    ("1a4342a5-fa06-4556-8a85-e8738fc25821", "StdPicture.GetURL"),
    (
        "3d4ad3b1-17de-4cf1-a2e4-0c2c83a5b5c2",
        "StdPicture.ListViewModeHierarchicalList",
    ),
    ("64837726-d2a2-4682-a788-737423e80013", "StdPicture.Picture"),
    (
        "b4c7ab2c-bcda-4468-a28f-5fee93838c4e",
        "StdPicture.Properties",
    ),
    (
        "b5a0aaba-3a83-4a71-b6f9-24aae1574681",
        "StdPicture.SaveReportSettings",
    ),
    (
        "be23a908-fe1b-44df-be94-d0f6e8353abe",
        "StdPicture.SendMessage",
    ),
    (
        "03665ff1-3a05-41d1-96d3-04bda2d8ede3",
        "StdPicture.SpreadsheetInsertComment",
    ),
    (
        "aa96f4bb-cf28-4dad-bc42-5ed53de95c0c",
        "StdPicture.SpreadsheetDeletePageBreak",
    ),
    (
        "2846af8d-af84-47e3-82b9-01b01f960426",
        "StdPicture.SpreadsheetReadOnly",
    ),
    (
        "3bdc16c8-6a96-4467-9442-a8e4804b3fa2",
        "StdPicture.SyncContents",
    ),
    (
        "8bdf1079-8fad-4d21-ad7f-4b2e4ecdce3d",
        "StdPicture.DialogInformation",
    ),
    (
        "60643198-e4b2-4c39-9de1-53cca3fff382",
        "StdPicture.DeleteDirectly",
    ),
    (
        "9fecbaff-2a05-4da6-9ef1-807e754b928d",
        "StdPicture.BusinessProcessStart",
    ),
    (
        "83db1f8a-41bd-4016-bdb2-a28e3a8d6dcc",
        "StdPicture.DialogStop",
    ),
    (
        "01ec9d9a-7497-4d88-b93f-066c633a4866",
        "StdPicture.InputOnBasis",
    ),
    (
        "c7cdd3c0-3879-436a-b145-5e2615e9b3e1",
        "StdPicture.FindInList",
    ),
    (
        "984b0a3e-daa2-4a9e-b75c-1f230a6e592a",
        "StdPicture.DataCompositionConditionalAppearance",
    ),
    (
        "ef27ae9e-7040-4374-b93c-0d276de2ea23",
        "StdPicture.DialogQuestion",
    ),
    (
        "9c96aa25-d656-4b3d-ab3e-81d9718da238",
        "StdPicture.ShowInList",
    ),
    (
        "549a2c45-4fce-493f-94ee-9a3a4f426551",
        "StdPicture.ListViewMode",
    ),
    (
        "b39aa431-a32f-4447-984a-45606474c82d",
        "StdPicture.AppearanceExclamationMarkIcon",
    ),
    (
        "c757209d-a87f-4410-b1a3-76000178f1f0",
        "StdPicture.ListViewModeList",
    ),
    (
        "501b8c1d-8062-408e-bde8-b6549324713e",
        "StdPicture.AppearanceExclamationMark",
    ),
    ("ed0bec43-4633-416c-8c08-0384ca444e32", "StdPicture.EndEdit"),
    (
        "0abdab67-5c90-4296-8168-239d22024d11",
        "StdPicture.PrintImmediately",
    ),
    (
        "31b93f03-0ba2-4631-a171-0d3a3d2ecc48",
        "StdPicture.ListSettings",
    ),
    (
        "37e91e77-93ce-4c3b-8d30-a9d8cfd3d3b0",
        "StdPicture.MoveItem",
    ),
    (
        "5182f57f-e834-4d11-9c9f-4aedc002b6e9",
        "StdPicture.FixTable",
    ),
    (
        "affb1617-24bc-4170-9c84-0902cc3ef206",
        "StdPicture.DataCompositionSettingsWizard",
    ),
    (
        "ad8cb448-a6bb-43b4-886a-7d6a8367eef2",
        "StdPicture.DataCompositionGroupFields",
    ),
    (
        "021c20a0-071b-4a60-8e44-12487adde0c8",
        "StdPicture.EditInDialog",
    ),
    (
        "64ca52ee-f1a3-468f-8055-311935077515",
        "StdPicture.QueryWizardTempTable",
    ),
    (
        "a9481ba4-dc85-4112-9c50-f9f340a61298",
        "StdPicture.QueryWizardReplaceTable",
    ),
    (
        "f695666a-bad9-49f6-ab7c-5198d7ea4739",
        "StdPicture.CustomExpression",
    ),
    (
        "7df3febb-2640-41b7-ad8b-7a23b7ad4aec",
        "StdPicture.QueryWizardCreateTempTableDropQuery",
    ),
    (
        "8ac19694-383a-457a-b050-0a3ee937f5f3",
        "StdPicture.DataCompositionNewNestedScheme",
    ),
    ("6cb69e7f-fe19-4f64-bfb5-1a4fad6c2ef9", "StdPicture.Replace"),
    (
        "6c2759b1-2b63-4fa5-8d13-75786c7e1e89",
        "StdPicture.DataCompositionNewTable",
    ),
    ("caf2e58b-ca3d-4b63-82c9-f21f1c9bc9eb", "StdPicture.Setting"),
    ("069b8324-7c51-4c73-a6a8-c06d4fc383b5", "StdPicture.Catalog"),
    (
        "31bf709f-3b50-4137-9b51-ebc7fb802a7c",
        "StdPicture.GotoExternalURL",
    ),
    ("1377931c-5744-4948-bade-cb35117b5f63", "StdPicture.Close"),
    (
        "d90a7482-9a1d-4d3d-ae96-6db440214d96",
        "StdPicture.DataCompositionFilter",
    ),
    (
        "2a0c2238-cb59-4473-ada6-352b60f3c0a9",
        "StdPicture.AddListItem",
    ),
    (
        "b7c81c62-d6ad-4eae-9cea-0e203182db67",
        "StdPicture.FormHelp",
    ),
    (
        "c2e2d966-5b7f-4699-903b-28a6f50d5471",
        "StdPicture.OutputList",
    ),
    (
        "9e808d29-787b-4825-863a-13c6844ce91d",
        "StdPicture.CancelSearch",
    ),
    (
        "eb47324b-85f9-4172-9315-bba8015d9970",
        "StdPicture.NewWindow",
    ),
    (
        "dcd23a32-5c7c-43f2-9021-80d98128556f",
        "StdPicture.CheckSyntax",
    ),
    (
        "e93f538e-dfaf-4a91-a9b6-c053555bcf60",
        "StdPicture.Constant",
    ),
    (
        "a075c3ef-bc4e-4c96-bdad-2245ec09c28e",
        "StdPicture.DataCompositionDataParameters",
    ),
    (
        "b0dd988f-2d9f-4364-b1f4-a4d5f45ffb78",
        "StdPicture.AppearanceFlagRed",
    ),
    (
        "b68eb29c-2372-46e1-b84e-13843899ccf6",
        "StdPicture.DataCompositionUserFields",
    ),
    (
        "9ef73565-2250-4a35-9fb3-470bd19ca9ca",
        "StdPicture.AppearanceCrossIcon",
    ),
    (
        "c78c9f3d-e92c-4f38-bb72-d8bd7fa5dbe3",
        "StdPicture.Dimension",
    ),
    (
        "7c75b1df-1fdc-471f-aae6-6b7870318cd4",
        "StdPicture.SpreadsheetDeleteComment",
    ),
    ("892196a9-c94f-4e50-8224-3c0cea4ea6b8", "StdPicture.GoBack"),
    (
        "1001ae3e-9289-4303-9699-3c0c17e20e61",
        "StdPicture.AddToFavorites",
    ),
    ("14b24498-e49c-4713-be64-75101d0abfb9", "StdPicture.GoToEnd"),
    (
        "167a160b-fa48-4337-87ab-7e0fe95c4b5a",
        "StdPicture.DeleteListItem",
    ),
    (
        "dfcd2d21-24ea-4b27-ab9a-6bf754577536",
        "StdPicture.FunctionMenuCommand",
    ),
    (
        "6cbf8f9a-3d2f-427b-bfce-5e2bc7a8589d",
        "StdPicture.DeleteListItemDirectly",
    ),
    (
        "1f046bc2-d6c5-46a3-a459-b2c0508f86fb",
        "StdPicture.QueryWizard",
    ),
    (
        "2c732bfa-f734-48bc-a18b-7554db8a3888",
        "StdPicture.DataCompositionSelection",
    ),
    (
        "38bbcebe-e456-461b-8457-07c9a72344a3",
        "StdPicture.FindInTree",
    ),
    (
        "4bf588d5-b8d8-47fd-8f41-eb9b668981ce",
        "StdPicture.SetListItemDeletionMark",
    ),
    (
        "5b612c21-e223-4997-9e61-86f7a67ec945",
        "StdPicture.ExternalDataSourceTable",
    ),
    (
        "6206a729-16e5-4e32-b53c-122de4e30c8d",
        "StdPicture.ScheduledJobs",
    ),
    (
        "6511326b-20c3-4bf8-8503-c2c2c9072c6c",
        "StdPicture.CustomizeForm",
    ),
    (
        "6b909f65-95a4-4697-8ca0-c8f331227b9a",
        "StdPicture.SettingsStorage",
    ),
    ("6ecee038-9722-4d80-bb91-7ee7046ec4c7", "StdPicture.Help"),
    (
        "7168f070-087c-448e-ae3a-6740f424076a",
        "StdPicture.ChooseFromList",
    ),
    (
        "732f9dd3-5baf-47ff-af7b-edfe16dac2a1",
        "StdPicture.DataCompositionNewChart",
    ),
    (
        "7562cef7-0e57-4f63-a754-b61128a4f3ae",
        "StdPicture.GoForward",
    ),
    (
        "75a40cc4-c719-4c3f-91ea-fc5787bc34ca",
        "StdPicture.UserWithAuthentication",
    ),
    (
        "77180b5e-8faa-4712-a788-e9f8903e3419",
        "StdPicture.DataCompositionNewGroup",
    ),
    (
        "efda7350-6cd7-4416-b188-f5ca9baf66c2",
        "StdPicture.DataCompositionOrder",
    ),
    (
        "83c8f18d-8701-41f3-bef4-53f88adbb868",
        "StdPicture.ReadChanges",
    ),
    (
        "85cc7dd0-44fc-41aa-967f-f52f202ee2e6",
        "StdPicture.GoToBegin",
    ),
    (
        "928075d1-b90b-416c-b0b2-c3104cf084aa",
        "StdPicture.Notifications",
    ),
    ("fc34a694-e99b-4d1c-a526-63f5571bdb09", "StdPicture.Form"),
    (
        "85998f14-805b-4e2b-ba19-9d79b0464042",
        "StdPicture.AppearanceCheckIcon",
    ),
    ("c283cd1c-3187-451d-8ef2-7df55daeef06", "StdPicture.History"),
    (
        "c1a61df2-f280-49d0-a8b3-7e5fc6f56ff7",
        "StdPicture.AppearanceCircleRed",
    ),
    (
        "b2202798-23e0-4165-9982-24878f432488",
        "StdPicture.AppearanceCross",
    ),
    (
        "71cbcb5c-f3f0-4ffd-a4d0-19b802b5ed6b",
        "StdPicture.AppearanceCircleGreen",
    ),
    (
        "e51185a4-d915-45b8-b201-1c46cc2d8104",
        "StdPicture.DocumentJournal",
    ),
    (
        "a6cbfd77-fcf0-40f4-a8de-ee0d3e580fe6",
        "StdPicture.DataProcessor",
    ),
    (
        "fada8a16-8b14-4151-87a9-775099f37832",
        "StdPicture.Calculator",
    ),
    (
        "8d7e5026-9c1c-4542-bec0-2b729c84e139",
        "StdPicture.AppearanceCircleYellow",
    ),
    (
        "c7f70aa3-b944-4efe-97e3-0fa3bda3cd88",
        "StdPicture.RotateClockwise",
    ),
    (
        "a43fcd1b-ad8d-4318-9d55-fd1ba086e65b",
        "StdPicture.RotateCounterclockwise",
    ),
    ("f874b0cc-db1d-4577-8c77-d4ba206eb05d", "StdPicture.Forward"),
    (
        "2721abfb-fbff-4a3a-98ac-b7c9eb29cd85",
        "StdPicture.AppearanceCircleEmpty",
    ),
    (
        "da9ac044-0ff7-4bcf-a441-3187bd1d951f",
        "StdPicture.ListViewModeTree",
    ),
    (
        "ed067d76-b144-4d00-bb36-d1833dd1350c",
        "StdPicture.DebitCredit",
    ),
    (
        "fad46a2b-2e56-47cc-b90c-3c2d4b061937",
        "StdPicture.ExternalDataSourceCube",
    ),
    (
        "05612131-3e11-49c0-9592-07e6d9318ef7",
        "StdPicture.FindNext",
    ),
    (
        "87d032df-0956-47e9-bead-4e15330f1983",
        "StdPicture.AppearanceUpArrowGreen",
    ),
    ("78da2c47-172f-4d57-ab52-a06e40548136", "StdPicture.Message"),
    (
        "251aaa98-0127-44c3-a163-6f5ab4367ee2",
        "StdPicture.DataCompositionStandardSettings",
    ),
    (
        "70f51581-87b6-41cb-a21b-c9dcdcc7fa93",
        "StdPicture.AccumulationRegister",
    ),
    ("f6e88116-03d8-4400-9d88-791895d7031a", "StdPicture.Attach"),
    ("9cf611dc-2370-4357-910d-a2b49c7a1ec6", "StdPicture.Next"),
    ("55bc1099-a7df-4d0a-b332-a45f0473b368", "StdPicture.Credit"),
    ("196622f7-0941-435b-992b-722f3082adf4", "StdPicture.Debit"),
    (
        "2a7e58e2-a6c5-4387-a459-5249c441947a",
        "StdPicture.GroupConversation",
    ),
    (
        "788667db-61c9-45f3-9c4f-5f660ecdf3e1",
        "StdPicture.AppearanceCircleFilled",
    ),
    (
        "e3b38083-0191-4a10-8f5b-51571f2419b4",
        "StdPicture.FindPrevious",
    ),
    (
        "e3b29b1d-4694-4f56-8d55-922f83afed7a",
        "StdPicture.AppearanceDownArrowGray",
    ),
    (
        "fc058833-e57f-4f93-ba7a-803992a65c3e",
        "StdPicture.AppearanceCircleOneFourthFilled",
    ),
    (
        "a722bc14-4edb-4eed-84b9-5d9b2b443e04",
        "StdPicture.CollaborationSystemUser",
    ),
    (
        "2954e819-f3fc-40de-9769-292efce9a355",
        "StdPicture.ExternalDataSourceFunction",
    ),
    (
        "f62488ee-f90c-47f7-929d-f42ec11a1e63",
        "StdPicture.WriteChanges",
    ),
    ("cb34c423-3d6a-4202-a809-3b3f45fb14ab", "StdPicture.LevelUp"),
    (
        "c38cc4cf-111d-4bc8-8dcb-4464e2ddfb25",
        "StdPicture.Favorites",
    ),
    (
        "35bc8caa-f7ce-4158-87da-d9bf785afa39",
        "StdPicture.DataSearch",
    ),
    (
        "d35bd799-1cc3-44d1-8ae3-09755a09d44b",
        "StdPicture.DataCompositionFilterDisabled",
    ),
    (
        "a9152be7-62cf-4523-be34-a23f018f497e",
        "StdPicture.GeographicalSchema",
    ),
    (
        "fe740df0-d828-4241-a12f-7414e12302e8",
        "StdPicture.QueryWizardTableParameters",
    ),
    (
        "01743054-d102-4e7c-bf15-5ed7fd84441b",
        "StdPicture.LevelDown",
    ),
    (
        "0bac63da-5b4e-48af-b593-7c5d29663e83",
        "StdPicture.FilterByType",
    ),
    (
        "73af51dd-6cda-48be-a093-5a7161c60c77",
        "StdPicture.FilterAndSort",
    ),
    (
        "ccb3d8f7-6da2-4c65-aba6-17b2ffbba78c",
        "StdPicture.ChartOfAccounts",
    ),
    (
        "835db646-1531-494b-b7c1-3239b0080bcb",
        "StdPicture.Parameters",
    ),
    (
        "46598f81-5f95-4485-9b33-bfe4fd1276d0",
        "StdPicture.SpreadsheetShowHeaders",
    ),
    (
        "52b637e5-f95f-4c70-9a72-2a4b5a9df449",
        "StdPicture.NestedTable",
    ),
    (
        "584b470d-ba34-4b25-9620-8de4066ffeaa",
        "StdPicture.Previous",
    ),
    (
        "fa67cb81-8d56-4534-90bd-b62fb0dbf5f0",
        "StdPicture.GanttChart",
    ),
    (
        "f3b8f300-5a54-4eea-8136-5798413a479c",
        "StdPicture.CalculationRegister",
    ),
    (
        "92e24ce1-3917-4ee4-bbde-adce48b6c96b",
        "StdPicture.AppearanceUpInclineArrowGray",
    ),
    (
        "20b82e97-5fcc-4c68-8e0d-d01060847520",
        "StdPicture.AppearanceRightArrowGray",
    ),
    (
        "a30ab2ef-6076-457d-9293-44edc7c6767e",
        "StdPicture.AppearanceDownInclineArrowGray",
    ),
    (
        "ba592483-bc90-4e26-ba4d-2126359c6529",
        "StdPicture.AppearanceBoxesFilled",
    ),
    (
        "3689585c-a3e2-45d0-a302-caeb31b78835",
        "StdPicture.AppearanceStarFilled",
    ),
    ("d66b6f73-53b8-49b9-8efc-33c54aa06e3f", "StdPicture.Notify"),
    (
        "702a9e16-0bb6-4efb-af11-10faf1e6ee87",
        "StdPicture.SpreadsheetShowGroups",
    ),
    (
        "e96de06b-fa83-48cf-b033-190a249855c9",
        "StdPicture.GraphicalSchema",
    ),
    ("a594c8a1-7218-420a-860f-7b493c5e65c4", "StdPicture.Sort"),
    (
        "093dd4ed-e03c-4fc6-a95a-01f51379cccf",
        "StdPicture.ActivateTask",
    ),
    (
        "26518e18-e364-475a-8026-e41134658b2a",
        "StdPicture.SpreadsheetInsertPageBreak",
    ),
    ("f3c1376a-d2ee-46c4-9e44-aa2f7dae31c4", "StdPicture.Chart"),
    (
        "d6eefec0-792a-4720-8933-e2a57f9e312c",
        "StdPicture.Resource",
    ),
    (
        "da0c4924-973c-4ef0-9dcf-f1fc3307e5e2",
        "StdPicture.ChangeListItem",
    ),
];
