//! Reference objects: Catalog, Document, ExchangePlan, the three charts,
//! BusinessProcess, Task, Report, DataProcessor, Enum.
//!
//! Every kind is `{1,<owner record>,<n>,<collection>...}`: the owner record
//! is a flat list of slots (generated types, the md header, properties) and
//! each collection is `{<class uuid>,<count>,<item>...}`, the collections in
//! uuid order. The parts every kind composes live in `objects_parts.rs`:
//! standard attributes, commands, tabular sections, attribute wrappers,
//! characteristics, field and metadata references.
//!
//! Record versions follow the configuration's compatibility mode, not the
//! platform: 8.3.24 stores the older versions; 8.3.27 adds
//! `LineNumberLength` to tabular sections, `TypeReductionMode` to standard
//! attributes and a constant tail to attributes; 8.5.1 adds
//! `AuxiliaryVariantForm` to reports and colours to enum values.

#[path = "objects_parts.rs"]
pub(crate) mod parts;

use anyhow::{Result, anyhow};

use self::parts::{
    AttributeWrapper, CommandWrapper, Obj, TsWrapper, collection, nil, num, restore_crlf,
};
use super::brace::Brace;
use super::{DescriptorContext, ObjectXml};

pub fn compile(object: &ObjectXml<'_>, context: &DescriptorContext) -> Result<Brace> {
    let obj = Obj::new(object, context)?;
    let mut tree = match object.kind {
        "Catalog" => catalog(&obj),
        "Document" => document(&obj),
        "ExchangePlan" => exchange_plan(&obj),
        "ChartOfCharacteristicTypes" => chart_of_characteristic_types(&obj),
        "ChartOfAccounts" => chart_of_accounts(&obj),
        "ChartOfCalculationTypes" => chart_of_calculation_types(&obj),
        "BusinessProcess" => business_process(&obj),
        "Task" => task(&obj),
        "Report" => report(&obj),
        "DataProcessor" => data_processor(&obj),
        "Enum" => enumeration(&obj),
        other => Err(anyhow!("{other} is not a reference object")),
    }?;
    restore_crlf(&mut tree);
    Ok(tree)
}

// Collection class uuids (the platform's, not the configuration's).
pub(crate) const TEMPLATES: &str = "3daea016-69b7-4ed4-9453-127911372fe6";
pub(crate) const TS_ATTRIBUTES: &str = "888744e1-b616-11d4-9436-004095e12fc7";

// Enumerations as the rows store them.
pub(crate) const EDIT_TYPE: &[(&str, i64)] = &[("InList", 0), ("InDialog", 1), ("BothWays", 2)];
pub(crate) const CHOICE_MODE: &[(&str, i64)] =
    &[("FromForm", 0), ("QuickChoice", 1), ("BothWays", 2)];
pub(crate) const CODE_TYPE: &[(&str, i64)] = &[("Number", 0), ("String", 1)];
pub(crate) const DEFAULT_PRESENTATION: &[(&str, i64)] = &[("AsCode", 0), ("AsDescription", 1)];
pub(crate) const DATA_LOCK_CONTROL_MODE: &[(&str, i64)] = &[("Automatic", 0), ("Managed", 1)];
pub(crate) const USE: &[(&str, i64)] = &[("DontUse", 0), ("Use", 1)];
pub(crate) const ALLOWED_LENGTH: &[(&str, i64)] = &[("Fixed", 0), ("Variable", 1)];
pub(crate) const CREATE_ON_INPUT: &[(&str, i64)] = &[("Auto", 0), ("DontUse", 1), ("Use", 2)];
pub(crate) const PREDEFINED_DATA_UPDATE: &[(&str, i64)] =
    &[("Auto", 0), ("AutoUpdate", 1), ("DontAutoUpdate", 2)];
pub(crate) const CHOICE_HISTORY_ON_INPUT: &[(&str, i64)] = &[("Auto", 0), ("DontUse", 1)];
pub(crate) const HIERARCHY_TYPE: &[(&str, i64)] =
    &[("HierarchyFoldersAndItems", 0), ("HierarchyOfItems", 1)];
pub(crate) const SUBORDINATION_USE: &[(&str, i64)] =
    &[("ToItems", 0), ("ToFolders", 1), ("ToFoldersAndItems", 2)];
pub(crate) const CATALOG_CODE_SERIES: &[(&str, i64)] = &[
    ("WholeCatalog", 0),
    ("WithinSubordination", 1),
    ("WithinOwnerSubordination", 2),
];
pub(crate) const POSTING: &[(&str, i64)] = &[("Allow", 0), ("Deny", 1)];
pub(crate) const REGISTER_RECORDS_DELETION: &[(&str, i64)] = &[
    ("AutoDelete", 0),
    ("AutoDeleteOff", 1),
    ("AutoDeleteOnUnpost", 2),
];
pub(crate) const REGISTER_RECORDS_WRITING: &[(&str, i64)] =
    &[("WriteSelected", 0), ("WriteModified", 1)];
pub(crate) const SEQUENCE_FILLING: &[(&str, i64)] = &[("AutoFill", 0), ("AutoFillOff", 1)];
pub(crate) const NUMBER_PERIODICITY: &[(&str, i64)] = &[
    ("Nonperiodical", 0),
    ("Year", 1),
    ("Quarter", 2),
    ("Month", 3),
    ("Day", 4),
];

// Standard-attribute markers per family, root level.
pub(crate) const CATALOG_STANDARD: &[(&str, i64)] = &[
    ("PredefinedDataName", -13),
    ("Predefined", -10),
    ("Ref", -8),
    ("DeletionMark", -7),
    ("IsFolder", -6),
    ("Owner", -5),
    ("Parent", -4),
    ("Description", -3),
    ("Code", -2),
];
pub(crate) const DOCUMENT_STANDARD: &[(&str, i64)] = &[
    ("Posted", -7),
    ("Ref", -5),
    ("DeletionMark", -4),
    ("Date", -3),
    ("Number", -2),
];
pub(crate) const EXCHANGE_PLAN_STANDARD: &[(&str, i64)] = &[
    ("ExchangeDate", -14),
    ("ThisNode", -13),
    ("ReceivedNo", -10),
    ("SentNo", -9),
    ("Ref", -6),
    ("DeletionMark", -4),
    ("Description", -3),
    ("Code", -2),
];
pub(crate) const CCT_STANDARD: &[(&str, i64)] = &[
    ("PredefinedDataName", -14),
    ("ValueType", -11),
    ("Description", -9),
    ("Code", -8),
    ("IsFolder", -7),
    ("Parent", -6),
    ("Predefined", -5),
    ("DeletionMark", -4),
    ("Ref", -2),
];
pub(crate) const COA_STANDARD: &[(&str, i64)] = &[
    ("PredefinedDataName", -28),
    ("Order", -17),
    ("OffBalance", -11),
    ("Type", -10),
    ("Description", -8),
    ("Code", -7),
    ("Parent", -6),
    ("Predefined", -5),
    ("DeletionMark", -4),
    ("Ref", -2),
];
pub(crate) const CCALC_STANDARD: &[(&str, i64)] = &[
    ("PredefinedDataName", -11),
    ("Predefined", -8),
    ("Ref", -6),
    ("DeletionMark", -5),
    ("ActionPeriodIsBasic", -4),
    ("Description", -3),
    ("Code", -2),
];
pub(crate) const BUSINESS_PROCESS_STANDARD: &[(&str, i64)] = &[
    ("Started", -9),
    ("HeadTask", -8),
    ("Completed", -7),
    ("Ref", -5),
    ("DeletionMark", -4),
    ("Date", -3),
    ("Number", -2),
];
pub(crate) const TASK_STANDARD: &[(&str, i64)] = &[
    ("Executed", -10),
    ("Description", -9),
    ("RoutePoint", -8),
    ("BusinessProcess", -7),
    ("Ref", -5),
    ("DeletionMark", -4),
    ("Date", -3),
    ("Number", -2),
];
pub(crate) const ENUM_STANDARD: &[(&str, i64)] = &[("Order", -3), ("Ref", -2)];
pub(crate) const INFORMATION_REGISTER_STANDARD: &[(&str, i64)] = &[
    ("Active", -5),
    ("LineNumber", -4),
    ("Recorder", -3),
    ("Period", -2),
];

/// The root-level standard attribute markers of a family, by kind name.
pub(crate) fn standard_markers(kind: &str) -> Option<&'static [(&'static str, i64)]> {
    Some(match kind {
        "Catalog" => CATALOG_STANDARD,
        "Document" => DOCUMENT_STANDARD,
        "ExchangePlan" => EXCHANGE_PLAN_STANDARD,
        "ChartOfCharacteristicTypes" => CCT_STANDARD,
        "ChartOfAccounts" => COA_STANDARD,
        "ChartOfCalculationTypes" => CCALC_STANDARD,
        "BusinessProcess" => BUSINESS_PROCESS_STANDARD,
        "Task" => TASK_STANDARD,
        "Enum" => ENUM_STANDARD,
        "InformationRegister" => INFORMATION_REGISTER_STANDARD,
        _ => return None,
    })
}

/// The `LineNumber` marker of a family's tabular sections.
pub(crate) fn line_number_marker(kind: &str) -> i64 {
    match kind {
        "Report" | "DataProcessor" => -3,
        "ChartOfCalculationTypes" => -100,
        _ => -10,
    }
}

/// Catalog, tag 56 (compatibility 8.3.24) or 57, 61 slots.
fn catalog(o: &Obj<'_>) -> Result<Brace> {
    let [object_type, object_value] = o.generated("Object")?;
    let [ref_type, ref_value] = o.generated("Ref")?;
    let [selection_type, selection_value] = o.generated("Selection")?;
    let [list_type, list_value] = o.generated("List")?;
    let [manager_type, manager_value] = o.generated("Manager")?;
    let owner = vec![
        num(if o.modern() { 57 } else { 56 }),
        object_type,
        object_value,
        ref_type,
        ref_value,
        selection_type,
        selection_value,
        list_type,
        list_value,
        o.wrapped_header(),
        o.number("LevelCount")?,
        o.code("EditType", EDIT_TYPE)?,
        o.references("Owners")?,
        o.flag("FoldersOnTop")?,
        o.flag("CheckUnique")?,
        o.flag("Autonumbering")?,
        o.code("CodeSeries", CATALOG_CODE_SERIES)?,
        o.number("CodeLength")?,
        o.code("CodeType", CODE_TYPE)?,
        o.number("DescriptionLength")?,
        o.code("DefaultPresentation", DEFAULT_PRESENTATION)?,
        o.form("DefaultObjectForm")?,
        o.form("DefaultFolderForm")?,
        o.form("DefaultListForm")?,
        o.form("DefaultChoiceForm")?,
        o.form("DefaultFolderChoiceForm")?,
        o.form("AuxiliaryObjectForm")?,
        o.form("AuxiliaryFolderForm")?,
        o.form("AuxiliaryListForm")?,
        o.form("AuxiliaryChoiceForm")?,
        o.form("AuxiliaryFolderChoiceForm")?,
        o.flag("UseStandardCommands")?,
        o.references("BasedOn")?,
        o.flag("IncludeHelpInContents")?,
        manager_type,
        manager_value,
        o.code("HierarchyType", HIERARCHY_TYPE)?,
        o.flag("Hierarchical")?,
        o.flag("LimitLevelCount")?,
        o.code("SubordinationUse", SUBORDINATION_USE)?,
        o.code("ChoiceMode", CHOICE_MODE)?,
        o.flag("QuickChoice")?,
        o.fields("InputByString")?,
        o.code("DataLockControlMode", DATA_LOCK_CONTROL_MODE)?,
        o.code("FullTextSearch", USE)?,
        o.standard_attributes(CATALOG_STANDARD)?,
        o.loc("ObjectPresentation"),
        o.loc("ExtendedObjectPresentation"),
        o.loc("ListPresentation"),
        o.loc("ExtendedListPresentation"),
        o.loc("Explanation"),
        o.code("CodeAllowedLength", ALLOWED_LENGTH)?,
        o.characteristics()?,
        o.code("CreateOnInput", CREATE_ON_INPUT)?,
        o.fields("DataLockFields")?,
        o.code("PredefinedDataUpdate", PREDEFINED_DATA_UPDATE)?,
        o.input_modes()?,
        o.code("ChoiceHistoryOnInput", CHOICE_HISTORY_ON_INPUT)?,
        o.code("DataHistory", USE)?,
        o.flag("UpdateDataHistoryImmediatelyAfterWrite")?,
        o.flag("ExecuteAfterWriteDataHistoryVersionProcessing")?,
    ];
    let attribute = if o.modern() {
        AttributeWrapper::HierarchicalModern(6)
    } else {
        AttributeWrapper::Hierarchical(5)
    };
    let ts = if o.modern() {
        TsWrapper::UseAndLength(2)
    } else {
        TsWrapper::Use(1)
    };
    o.root(
        owner,
        vec![
            collection(TEMPLATES, o.templates()?),
            collection(
                "4fe87c89-9ad4-43f6-9fdb-9dc83b3879c6",
                o.commands(CommandWrapper::Owner)?,
            ),
            collection(
                "932159f9-95b2-4e76-a8dd-8849fe5c5ded",
                o.tabular_sections(ts, TS_ATTRIBUTES, AttributeWrapper::TabularSection)?,
            ),
            collection("cf4abea7-37b2-11d4-940f-008048da11f9", o.attributes(attribute)?),
            collection("fdf816d2-1ead-11d5-b975-0050bae0a95d", o.forms()?),
        ],
    )
}

/// Document, tag 40, 53 slots.
fn document(o: &Obj<'_>) -> Result<Brace> {
    let [object_type, object_value] = o.generated("Object")?;
    let [ref_type, ref_value] = o.generated("Ref")?;
    let [selection_type, selection_value] = o.generated("Selection")?;
    let [list_type, list_value] = o.generated("List")?;
    let [manager_type, manager_value] = o.generated("Manager")?;
    let owner = vec![
        num(40),
        object_type,
        object_value,
        ref_type,
        ref_value,
        selection_type,
        selection_value,
        list_type,
        list_value,
        o.wrapped_header(),
        o.reference("Numerator")?,
        o.code("NumberType", CODE_TYPE)?,
        o.number("NumberLength")?,
        o.code("NumberPeriodicity", NUMBER_PERIODICITY)?,
        o.flag("CheckUnique")?,
        o.flag("Autonumbering")?,
        o.form("DefaultObjectForm")?,
        o.form("DefaultListForm")?,
        o.form("DefaultChoiceForm")?,
        o.code("Posting", POSTING)?,
        o.code("RegisterRecordsDeletion", REGISTER_RECORDS_DELETION)?,
        o.code("RealTimePosting", POSTING)?,
        o.references("BasedOn")?,
        o.flag("UseStandardCommands")?,
        o.references("RegisterRecords")?,
        o.flag("IncludeHelpInContents")?,
        manager_type,
        manager_value,
        o.code("SequenceFilling", SEQUENCE_FILLING)?,
        o.fields("InputByString")?,
        o.code("DataLockControlMode", DATA_LOCK_CONTROL_MODE)?,
        o.code("FullTextSearch", USE)?,
        o.standard_attributes(DOCUMENT_STANDARD)?,
        o.flag("PostInPrivilegedMode")?,
        o.flag("UnpostInPrivilegedMode")?,
        o.form("AuxiliaryObjectForm")?,
        o.form("AuxiliaryListForm")?,
        o.form("AuxiliaryChoiceForm")?,
        o.loc("ObjectPresentation"),
        o.loc("ExtendedObjectPresentation"),
        o.loc("ListPresentation"),
        o.loc("ExtendedListPresentation"),
        o.loc("Explanation"),
        o.code("RegisterRecordsWritingOnPost", REGISTER_RECORDS_WRITING)?,
        o.code("NumberAllowedLength", ALLOWED_LENGTH)?,
        o.characteristics()?,
        o.code("CreateOnInput", CREATE_ON_INPUT)?,
        o.fields("DataLockFields")?,
        o.input_modes()?,
        o.code("ChoiceHistoryOnInput", CHOICE_HISTORY_ON_INPUT)?,
        o.code("DataHistory", USE)?,
        o.flag("UpdateDataHistoryImmediatelyAfterWrite")?,
        o.flag("ExecuteAfterWriteDataHistoryVersionProcessing")?,
    ];
    let attribute = if o.modern() {
        AttributeWrapper::PlainModern(6)
    } else {
        AttributeWrapper::Plain(5)
    };
    let ts = if o.modern() {
        TsWrapper::Length(2)
    } else {
        TsWrapper::Bare(1)
    };
    o.root(
        owner,
        vec![
            collection(
                "21c53e09-8950-4b5e-a6a0-1054f1bbc274",
                o.tabular_sections(ts, TS_ATTRIBUTES, AttributeWrapper::TabularSection)?,
            ),
            collection(TEMPLATES, o.templates()?),
            collection("45e46cbc-3e24-4165-8b7b-cc98a6f80211", o.attributes(attribute)?),
            collection(
                "b544fc6a-2ba3-4885-8fb2-cb289fb6d65e",
                o.commands(CommandWrapper::Owner)?,
            ),
            collection("fb880e93-47d7-4127-9357-a20e69c17545", o.forms()?),
        ],
    )
}

/// ExchangePlan, tag 36 (50 slots) or 37 (51 slots).
fn exchange_plan(o: &Obj<'_>) -> Result<Brace> {
    let [object_type, object_value] = o.generated("Object")?;
    let [ref_type, ref_value] = o.generated("Ref")?;
    let [selection_type, selection_value] = o.generated("Selection")?;
    let [list_type, list_value] = o.generated("List")?;
    let [manager_type, manager_value] = o.generated("Manager")?;
    let mut owner = vec![
        num(if o.modern() { 37 } else { 36 }),
        object_type,
        object_value,
        ref_type,
        ref_value,
        selection_type,
        selection_value,
        list_type,
        list_value,
        manager_type,
        manager_value,
        o.this_node()?,
        o.header(),
        o.flag("UseStandardCommands")?,
        o.form("DefaultObjectForm")?,
        o.number("CodeLength")?,
        num(0),
        o.number("DescriptionLength")?,
        o.flag("IncludeHelpInContents")?,
        o.form("DefaultListForm")?,
        o.form("DefaultChoiceForm")?,
        o.code("EditType", EDIT_TYPE)?,
        o.code("ChoiceMode", CHOICE_MODE)?,
        o.flag("QuickChoice")?,
        o.references("BasedOn")?,
        o.code("DefaultPresentation", DEFAULT_PRESENTATION)?,
        o.flag("DistributedInfoBase")?,
        o.fields("InputByString")?,
        o.code("DataLockControlMode", DATA_LOCK_CONTROL_MODE)?,
        o.code("FullTextSearch", USE)?,
        o.standard_attributes(EXCHANGE_PLAN_STANDARD)?,
        o.form("AuxiliaryObjectForm")?,
        o.form("AuxiliaryListForm")?,
        o.form("AuxiliaryChoiceForm")?,
        o.loc("ObjectPresentation"),
        o.loc("ExtendedObjectPresentation"),
        o.loc("ListPresentation"),
        o.loc("ExtendedListPresentation"),
        o.loc("Explanation"),
        o.code("CodeAllowedLength", ALLOWED_LENGTH)?,
        o.characteristics()?,
        o.code("CreateOnInput", CREATE_ON_INPUT)?,
        o.fields("DataLockFields")?,
        o.input_modes()?,
        o.code("ChoiceHistoryOnInput", CHOICE_HISTORY_ON_INPUT)?,
        o.flag("IncludeConfigurationExtensions")?,
        o.code("DataHistory", USE)?,
        o.flag("UpdateDataHistoryImmediatelyAfterWrite")?,
        o.flag("ExecuteAfterWriteDataHistoryVersionProcessing")?,
        num(0),
    ];
    if o.modern() {
        owner.push(num(1));
    }
    let attribute = if o.modern() {
        AttributeWrapper::PlainModern(4)
    } else {
        AttributeWrapper::Plain(3)
    };
    let ts = if o.modern() {
        TsWrapper::Length(1)
    } else {
        TsWrapper::Bare(0)
    };
    o.root(
        owner,
        vec![
            collection("1a1b4fea-e093-470d-94ff-1d2f16cda2ab", o.attributes(attribute)?),
            collection(TEMPLATES, o.templates()?),
            collection(
                "52293f4b-f98c-43ea-a80f-41047ae7ab58",
                o.tabular_sections(ts, TS_ATTRIBUTES, AttributeWrapper::TabularSection)?,
            ),
            collection("87c509ab-3d38-4d67-b379-aca796298578", o.forms()?),
            collection(
                "d5207c64-11d5-4d46-bba2-55b7b07ff4eb",
                o.commands(CommandWrapper::Owner)?,
            ),
        ],
    )
}

/// ChartOfCharacteristicTypes, tag 34, 59 slots.
fn chart_of_characteristic_types(o: &Obj<'_>) -> Result<Brace> {
    let [object_type, object_value] = o.generated("Object")?;
    let [ref_type, ref_value] = o.generated("Ref")?;
    let [selection_type, selection_value] = o.generated("Selection")?;
    let [list_type, list_value] = o.generated("List")?;
    let [characteristic_type, characteristic_value] = o.generated("Characteristic")?;
    let [manager_type, manager_value] = o.generated("Manager")?;
    let owner = vec![
        num(34),
        object_type,
        object_value,
        ref_type,
        ref_value,
        selection_type,
        selection_value,
        list_type,
        list_value,
        characteristic_type,
        characteristic_value,
        manager_type,
        manager_value,
        o.wrapped_header(),
        o.flag("UseStandardCommands")?,
        o.references("BasedOn")?,
        o.flag("IncludeHelpInContents")?,
        o.reference("CharacteristicExtValues")?,
        o.type_pattern("Type")?,
        o.flag("Hierarchical")?,
        o.flag("FoldersOnTop")?,
        o.number("CodeLength")?,
        o.flag("Autonumbering")?,
        o.number("DescriptionLength")?,
        o.code("DefaultPresentation", DEFAULT_PRESENTATION)?,
        o.code("EditType", EDIT_TYPE)?,
        o.form("DefaultObjectForm")?,
        o.form("DefaultFolderForm")?,
        o.form("DefaultListForm")?,
        o.form("DefaultChoiceForm")?,
        o.form("DefaultFolderChoiceForm")?,
        o.code("ChoiceMode", CHOICE_MODE)?,
        o.flag("QuickChoice")?,
        o.fields("InputByString")?,
        o.flag("CheckUnique")?,
        o.code(
            "CodeSeries",
            &[("WholeCharacteristicKind", 0), ("WithinSubordination", 1)],
        )?,
        o.code("DataLockControlMode", DATA_LOCK_CONTROL_MODE)?,
        o.code("FullTextSearch", USE)?,
        o.standard_attributes(CCT_STANDARD)?,
        o.form("AuxiliaryObjectForm")?,
        o.form("AuxiliaryFolderForm")?,
        o.form("AuxiliaryListForm")?,
        o.form("AuxiliaryChoiceForm")?,
        o.form("AuxiliaryFolderChoiceForm")?,
        o.loc("ObjectPresentation"),
        o.loc("ExtendedObjectPresentation"),
        o.loc("ListPresentation"),
        o.loc("ExtendedListPresentation"),
        o.loc("Explanation"),
        o.code("CodeAllowedLength", ALLOWED_LENGTH)?,
        o.characteristics()?,
        o.code("CreateOnInput", CREATE_ON_INPUT)?,
        o.fields("DataLockFields")?,
        o.code("PredefinedDataUpdate", PREDEFINED_DATA_UPDATE)?,
        o.input_modes()?,
        o.code("ChoiceHistoryOnInput", CHOICE_HISTORY_ON_INPUT)?,
        o.code("DataHistory", USE)?,
        o.flag("UpdateDataHistoryImmediatelyAfterWrite")?,
        o.flag("ExecuteAfterWriteDataHistoryVersionProcessing")?,
    ];
    let attribute = if o.modern() {
        AttributeWrapper::HierarchicalModern(3)
    } else {
        AttributeWrapper::Hierarchical(2)
    };
    let ts = if o.modern() {
        TsWrapper::UseAndLength(1)
    } else {
        TsWrapper::Use(0)
    };
    o.root(
        owner,
        vec![
            collection("31182525-9346-4595-81f8-6f91a72ebe06", o.attributes(attribute)?),
            collection(TEMPLATES, o.templates()?),
            collection(
                "54e36536-7863-42fd-bea3-c5edd3122fdc",
                o.tabular_sections(ts, TS_ATTRIBUTES, AttributeWrapper::TabularSection)?,
            ),
            collection(
                "95b5e1d4-abfa-4a16-818d-a5b07b7d3f73",
                o.commands(CommandWrapper::Owner)?,
            ),
            collection("eb2b78a8-40a6-4b7e-b1b3-6ca9966cbc94", o.forms()?),
        ],
    )
}

/// ChartOfAccounts, tag 32, 57 slots, seven collections.
fn chart_of_accounts(o: &Obj<'_>) -> Result<Brace> {
    let [object_type, object_value] = o.generated("Object")?;
    let [ref_type, ref_value] = o.generated("Ref")?;
    let [selection_type, selection_value] = o.generated("Selection")?;
    let [list_type, list_value] = o.generated("List")?;
    let [manager_type, manager_value] = o.generated("Manager")?;
    let [ext_type, ext_value] = o.generated("ExtDimensionTypes")?;
    let [ext_row_type, ext_row_value] = o.generated("ExtDimensionTypesRow")?;
    let owner = vec![
        num(32),
        object_type,
        object_value,
        ref_type,
        ref_value,
        selection_type,
        selection_value,
        list_type,
        list_value,
        manager_type,
        manager_value,
        ext_type,
        ext_value,
        ext_row_type,
        ext_row_value,
        o.wrapped_header(),
        o.flag("UseStandardCommands")?,
        o.flag("IncludeHelpInContents")?,
        o.references("BasedOn")?,
        o.reference("ExtDimensionTypes")?,
        o.number("MaxExtDimensionCount")?,
        Brace::str(o.text("CodeMask")),
        o.number("CodeLength")?,
        o.number("DescriptionLength")?,
        o.code("EditType", EDIT_TYPE)?,
        o.number("OrderLength")?,
        o.code("DefaultPresentation", DEFAULT_PRESENTATION)?,
        o.flag("AutoOrderByCode")?,
        o.form("DefaultObjectForm")?,
        o.form("DefaultListForm")?,
        o.form("DefaultChoiceForm")?,
        o.code("ChoiceMode", CHOICE_MODE)?,
        o.flag("QuickChoice")?,
        o.fields("InputByString")?,
        o.flag("CheckUnique")?,
        o.code(
            "CodeSeries",
            &[("WholeChartOfAccounts", 0), ("WithinSubordination", 1)],
        )?,
        o.code("DataLockControlMode", DATA_LOCK_CONTROL_MODE)?,
        o.code("FullTextSearch", USE)?,
        o.standard_attributes(COA_STANDARD)?,
        o.standard_tabular_sections(&[(
            "ExtDimensionTypes",
            -12,
            &[
                ("TurnoversOnly", -15),
                ("Predefined", -14),
                ("ExtDimensionType", -13),
                ("LineNumber", -12),
            ],
        )])?,
        o.form("AuxiliaryObjectForm")?,
        o.form("AuxiliaryListForm")?,
        o.form("AuxiliaryChoiceForm")?,
        o.loc("ObjectPresentation"),
        o.loc("ExtendedObjectPresentation"),
        o.loc("ListPresentation"),
        o.loc("ExtendedListPresentation"),
        o.loc("Explanation"),
        o.characteristics()?,
        o.code("CreateOnInput", CREATE_ON_INPUT)?,
        o.fields("DataLockFields")?,
        o.code("PredefinedDataUpdate", PREDEFINED_DATA_UPDATE)?,
        o.input_modes()?,
        o.code("ChoiceHistoryOnInput", CHOICE_HISTORY_ON_INPUT)?,
        o.code("DataHistory", USE)?,
        o.flag("UpdateDataHistoryImmediatelyAfterWrite")?,
        o.flag("ExecuteAfterWriteDataHistoryVersionProcessing")?,
    ];
    let attribute = if o.modern() {
        AttributeWrapper::PlainModern(3)
    } else {
        AttributeWrapper::Plain(2)
    };
    o.root(
        owner,
        vec![
            collection(
                "0df30176-6865-4787-9fc8-609eb144174f",
                o.commands(CommandWrapper::Owner)?,
            ),
            collection(TEMPLATES, o.templates()?),
            collection("4c7fec95-d1bd-4508-8a01-f1db090d9af8", Vec::new()),
            collection("5372e285-03db-4f8c-8565-fe56f1aea40e", o.forms()?),
            collection("6e65cbf5-daa8-4d8d-bef8-59723f4e5777", o.attributes(attribute)?),
            collection(
                "78bd1243-c4df-46c3-8138-e147465cb9a4",
                o.children_of("AccountingFlag", AttributeWrapper::Flag(6))?,
            ),
            collection(
                "c70ca527-5042-4cad-a315-dcb4007e32a3",
                o.children_of("ExtDimensionAccountingFlag", AttributeWrapper::Flag(6))?,
            ),
        ],
    )
}

/// ChartOfCalculationTypes, tag 35, 63 slots.
fn chart_of_calculation_types(o: &Obj<'_>) -> Result<Brace> {
    let [object_type, object_value] = o.generated("Object")?;
    let [ref_type, ref_value] = o.generated("Ref")?;
    let [selection_type, selection_value] = o.generated("Selection")?;
    let [list_type, list_value] = o.generated("List")?;
    let [manager_type, manager_value] = o.generated("Manager")?;
    let [displacing_type, displacing_value] = o.generated("DisplacingCalculationTypes")?;
    let [displacing_row_type, displacing_row_value] =
        o.generated("DisplacingCalculationTypesRow")?;
    let [base_type, base_value] = o.generated("BaseCalculationTypes")?;
    let [base_row_type, base_row_value] = o.generated("BaseCalculationTypesRow")?;
    let [leading_type, leading_value] = o.generated("LeadingCalculationTypes")?;
    let [leading_row_type, leading_row_value] = o.generated("LeadingCalculationTypesRow")?;
    let calculation_type_rows: &[(&str, i64)] = &[
        ("Predefined", -102),
        ("CalculationType", -101),
        ("LineNumber", -100),
    ];
    let owner = vec![
        num(35),
        o.wrapped_header(),
        object_type,
        object_value,
        ref_type,
        ref_value,
        selection_type,
        selection_value,
        list_type,
        list_value,
        manager_type,
        manager_value,
        displacing_type,
        displacing_value,
        displacing_row_type,
        displacing_row_value,
        base_type,
        base_value,
        base_row_type,
        base_row_value,
        leading_type,
        leading_value,
        leading_row_type,
        leading_row_value,
        o.flag("UseStandardCommands")?,
        o.number("CodeLength")?,
        o.code("CodeType", CODE_TYPE)?,
        o.code("EditType", EDIT_TYPE)?,
        o.references("BaseCalculationTypes")?,
        o.flag("ActionPeriodUse")?,
        o.number("DescriptionLength")?,
        o.code("DefaultPresentation", DEFAULT_PRESENTATION)?,
        o.form("DefaultObjectForm")?,
        o.form("DefaultListForm")?,
        o.form("DefaultChoiceForm")?,
        o.code(
            "DependenceOnCalculationTypes",
            &[("DontUse", 0), ("OnActionPeriod", 1), ("OnBasePeriod", 2)],
        )?,
        o.references("BasedOn")?,
        o.flag("IncludeHelpInContents")?,
        o.code("ChoiceMode", CHOICE_MODE)?,
        o.flag("QuickChoice")?,
        o.fields("InputByString")?,
        o.code("DataLockControlMode", DATA_LOCK_CONTROL_MODE)?,
        o.code("FullTextSearch", USE)?,
        o.standard_attributes(CCALC_STANDARD)?,
        o.standard_tabular_sections(&[
            ("LeadingCalculationTypes", -30, calculation_type_rows),
            ("DisplacingCalculationTypes", -20, calculation_type_rows),
            ("BaseCalculationTypes", -10, calculation_type_rows),
        ])?,
        o.form("AuxiliaryObjectForm")?,
        o.form("AuxiliaryListForm")?,
        o.form("AuxiliaryChoiceForm")?,
        o.loc("ObjectPresentation"),
        o.loc("ExtendedObjectPresentation"),
        o.loc("ListPresentation"),
        o.loc("ExtendedListPresentation"),
        o.loc("Explanation"),
        o.code("CodeAllowedLength", ALLOWED_LENGTH)?,
        o.characteristics()?,
        o.code("CreateOnInput", CREATE_ON_INPUT)?,
        o.fields("DataLockFields")?,
        o.code("PredefinedDataUpdate", PREDEFINED_DATA_UPDATE)?,
        o.input_modes()?,
        o.code("ChoiceHistoryOnInput", CHOICE_HISTORY_ON_INPUT)?,
        o.code("DataHistory", USE)?,
        o.flag("UpdateDataHistoryImmediatelyAfterWrite")?,
        o.flag("ExecuteAfterWriteDataHistoryVersionProcessing")?,
    ];
    let attribute = if o.modern() {
        AttributeWrapper::PlainModern(4)
    } else {
        AttributeWrapper::Plain(3)
    };
    let ts = if o.modern() {
        TsWrapper::Length(1)
    } else {
        TsWrapper::Bare(0)
    };
    o.root(
        owner,
        vec![
            collection(
                "054aa8cf-faa6-4634-aef4-1087ca0d88fc",
                o.tabular_sections(ts, TS_ATTRIBUTES, AttributeWrapper::TabularSection)?,
            ),
            collection("0dc22ad2-476a-4794-afae-cfa7ed251752", o.attributes(attribute)?),
            collection(
                "2e90c75b-2f0c-4899-a7d4-5426eaefc96e",
                o.commands(CommandWrapper::Owner)?,
            ),
            collection(TEMPLATES, o.templates()?),
            collection("a7f8f92a-7a4b-484b-937e-42d242e64144", o.forms()?),
        ],
    )
}

/// BusinessProcess, tag 30, 49 slots.
fn business_process(o: &Obj<'_>) -> Result<Brace> {
    let [object_type, object_value] = o.generated("Object")?;
    let [ref_type, ref_value] = o.generated("Ref")?;
    let [selection_type, selection_value] = o.generated("Selection")?;
    let [list_type, list_value] = o.generated("List")?;
    let [manager_type, manager_value] = o.generated("Manager")?;
    let [route_type, route_value] = o.generated("RoutePointRef")?;
    let owner = vec![
        num(30),
        o.header(),
        o.flag("UseStandardCommands")?,
        object_type,
        object_value,
        ref_type,
        ref_value,
        selection_type,
        selection_value,
        list_type,
        list_value,
        manager_type,
        manager_value,
        route_type,
        route_value,
        o.references("BasedOn")?,
        o.code("EditType", EDIT_TYPE)?,
        o.code("NumberType", &[("String", 0), ("Number", 1)])?,
        o.number("NumberLength")?,
        o.code("CreateOnInput", CREATE_ON_INPUT)?,
        o.flag("CheckUnique")?,
        o.flag("Autonumbering")?,
        o.form("DefaultObjectForm")?,
        o.form("DefaultListForm")?,
        o.form("DefaultChoiceForm")?,
        o.reference("Task")?,
        o.flag("IncludeHelpInContents")?,
        o.fields("InputByString")?,
        o.code("NumberAllowedLength", ALLOWED_LENGTH)?,
        o.flag("CreateTaskInPrivilegedMode")?,
        o.standard_attributes(BUSINESS_PROCESS_STANDARD)?,
        o.code(
            "NumberPeriodicity",
            &[
                ("Year", 0),
                ("Nonperiodical", 1),
                ("Quarter", 2),
                ("Month", 3),
                ("Day", 4),
            ],
        )?,
        o.form("AuxiliaryObjectForm")?,
        o.form("AuxiliaryListForm")?,
        o.form("AuxiliaryChoiceForm")?,
        o.loc("ObjectPresentation"),
        o.loc("ExtendedObjectPresentation"),
        o.loc("ListPresentation"),
        o.loc("ExtendedListPresentation"),
        o.loc("Explanation"),
        o.code("DataLockControlMode", DATA_LOCK_CONTROL_MODE)?,
        o.characteristics()?,
        o.code("FullTextSearch", USE)?,
        o.fields("DataLockFields")?,
        o.input_modes()?,
        o.code("ChoiceHistoryOnInput", CHOICE_HISTORY_ON_INPUT)?,
        o.code("DataHistory", USE)?,
        o.flag("UpdateDataHistoryImmediatelyAfterWrite")?,
        o.flag("ExecuteAfterWriteDataHistoryVersionProcessing")?,
    ];
    let attribute = if o.modern() {
        AttributeWrapper::PlainModern(3)
    } else {
        AttributeWrapper::Plain(2)
    };
    let ts = if o.modern() {
        TsWrapper::Length(1)
    } else {
        TsWrapper::Bare(0)
    };
    o.root(
        owner,
        vec![
            collection(TEMPLATES, o.templates()?),
            collection("3f7a8120-b71a-4265-98bf-4d9bc09b7719", o.forms()?),
            collection(
                "7a3e533c-f232-40d5-a932-6a311d2480bf",
                o.commands(CommandWrapper::Owner)?,
            ),
            collection("87c988de-ecbf-413b-87b0-b9516df05e28", o.attributes(attribute)?),
            collection(
                "a3fe6537-d787-40f7-8a06-419d2f0c1cfd",
                o.tabular_sections(ts, TS_ATTRIBUTES, AttributeWrapper::TabularSection)?,
            ),
        ],
    )
}

/// Task, tag 33, 52 slots, six collections.
fn task(o: &Obj<'_>) -> Result<Brace> {
    let [object_type, object_value] = o.generated("Object")?;
    let [ref_type, ref_value] = o.generated("Ref")?;
    let [selection_type, selection_value] = o.generated("Selection")?;
    let [list_type, list_value] = o.generated("List")?;
    let [manager_type, manager_value] = o.generated("Manager")?;
    let owner = vec![
        num(33),
        o.header(),
        o.flag("UseStandardCommands")?,
        object_type,
        object_value,
        ref_type,
        ref_value,
        selection_type,
        selection_value,
        list_type,
        list_value,
        manager_type,
        manager_value,
        nil(),
        nil(),
        o.form("DefaultObjectForm")?,
        o.form("DefaultListForm")?,
        o.form("DefaultChoiceForm")?,
        o.code("NumberType", CODE_TYPE)?,
        o.number("NumberLength")?,
        o.code("EditType", EDIT_TYPE)?,
        o.flag("CheckUnique")?,
        o.number("DescriptionLength")?,
        o.flag("Autonumbering")?,
        o.flag("IncludeHelpInContents")?,
        o.reference("Addressing")?,
        o.reference("MainAddressingAttribute")?,
        o.code("DefaultPresentation", DEFAULT_PRESENTATION)?,
        o.fields("InputByString")?,
        o.reference("CurrentPerformer")?,
        o.references("BasedOn")?,
        o.code(
            "TaskNumberAutoPrefix",
            &[("BusinessProcessNumber", 0), ("DontUse", 1)],
        )?,
        o.code("FullTextSearch", USE)?,
        o.code("DataLockControlMode", DATA_LOCK_CONTROL_MODE)?,
        o.standard_attributes(TASK_STANDARD)?,
        o.form("AuxiliaryObjectForm")?,
        o.form("AuxiliaryListForm")?,
        o.form("AuxiliaryChoiceForm")?,
        o.loc("ObjectPresentation"),
        o.loc("ExtendedObjectPresentation"),
        o.loc("ListPresentation"),
        o.loc("ExtendedListPresentation"),
        o.loc("Explanation"),
        o.code("NumberAllowedLength", ALLOWED_LENGTH)?,
        o.characteristics()?,
        o.code("CreateOnInput", CREATE_ON_INPUT)?,
        o.fields("DataLockFields")?,
        o.input_modes()?,
        o.code("ChoiceHistoryOnInput", CHOICE_HISTORY_ON_INPUT)?,
        o.code("DataHistory", USE)?,
        o.flag("UpdateDataHistoryImmediatelyAfterWrite")?,
        o.flag("ExecuteAfterWriteDataHistoryVersionProcessing")?,
    ];
    let attribute = if o.modern() {
        AttributeWrapper::PlainModern(3)
    } else {
        AttributeWrapper::Plain(2)
    };
    let ts = if o.modern() {
        TsWrapper::Length(1)
    } else {
        TsWrapper::Bare(0)
    };
    o.root(
        owner,
        vec![
            collection(TEMPLATES, o.templates()?),
            collection("3f58cbfb-4172-4e54-be49-561a579bb38b", o.forms()?),
            collection("8ddfb495-c5fc-46b9-bdc5-bcf58341bff0", o.attributes(attribute)?),
            collection(
                "e97c0570-251c-4566-b0f1-10686820f143",
                o.children_of("AddressingAttribute", AttributeWrapper::Addressing(4))?,
            ),
            collection(
                "ee865d4b-a458-48a0-b38f-5a26898feeb0",
                o.tabular_sections(ts, TS_ATTRIBUTES, AttributeWrapper::TabularSection)?,
            ),
            collection(
                "f27c2152-a2c9-4c30-adb1-130f5eb2590f",
                o.commands(CommandWrapper::Owner)?,
            ),
        ],
    )
}

/// Report, tag 19 (18 slots) or, from compatibility 8.5.1, 20 (19 slots).
fn report(o: &Obj<'_>) -> Result<Brace> {
    let [object_type, object_value] = o.generated("Object")?;
    let [manager_type, manager_value] = o.generated("Manager")?;
    let mut owner = vec![
        num(if o.v851() { 20 } else { 19 }),
        object_type,
        object_value,
        o.wrapped_header(),
        o.reference("DefaultForm")?,
        o.reference("MainDataCompositionSchema")?,
        o.reference("DefaultSettingsForm")?,
        o.flag("UseStandardCommands")?,
        o.reference("VariantsStorage")?,
        o.reference("SettingsStorage")?,
        o.reference("DefaultVariantForm")?,
        o.flag("IncludeHelpInContents")?,
        manager_type,
        manager_value,
        o.reference("AuxiliaryForm")?,
        o.loc("ExtendedPresentation"),
        o.loc("Explanation"),
        o.reference("AuxiliarySettingsForm")?,
    ];
    if o.v851() {
        owner.push(o.reference("AuxiliaryVariantForm")?);
    }
    o.root(
        owner,
        vec![
            collection(TEMPLATES, o.templates()?),
            collection(
                "7e7123e0-29e2-11d6-a3c7-0050bae0a776",
                o.attributes(AttributeWrapper::Bare)?,
            ),
            collection("a3b368c0-29e2-11d6-a3c7-0050bae0a776", o.forms()?),
            collection(
                "b077d780-29e2-11d6-a3c7-0050bae0a776",
                o.tabular_sections(
                    TsWrapper::Bare(0),
                    "c339c860-29e2-11d6-a3c7-0050bae0a776",
                    AttributeWrapper::Bare,
                )?,
            ),
            collection(
                "e7ff38c0-ec3c-47a0-ae90-20c73ca72246",
                o.commands(CommandWrapper::Bare)?,
            ),
        ],
    )
}

/// DataProcessor, tag 17, 12 slots.
fn data_processor(o: &Obj<'_>) -> Result<Brace> {
    let [object_type, object_value] = o.generated("Object")?;
    let [manager_type, manager_value] = o.generated("Manager")?;
    let owner = vec![
        num(17),
        object_type,
        object_value,
        o.wrapped_header(),
        o.reference("DefaultForm")?,
        o.flag("UseStandardCommands")?,
        o.flag("IncludeHelpInContents")?,
        manager_type,
        manager_value,
        o.reference("AuxiliaryForm")?,
        o.loc("ExtendedPresentation"),
        o.loc("Explanation"),
    ];
    o.root(
        owner,
        vec![
            collection(
                "2bcef0d1-0981-11d6-b9b8-0050bae0a95d",
                o.tabular_sections(
                    TsWrapper::Bare(0),
                    "5d24a9d1-098e-11d6-b9b8-0050bae0a95d",
                    AttributeWrapper::Bare,
                )?,
            ),
            collection(TEMPLATES, o.templates()?),
            collection(
                "45556acb-826a-4f73-898a-6025fc9536e1",
                o.commands(CommandWrapper::Bare)?,
            ),
            collection("d5b0e5ed-256d-401c-9c36-f630cafd8a62", o.forms()?),
            collection(
                "ec6bb5e5-b7a8-4d75-bec9-658107a699cf",
                o.attributes(AttributeWrapper::Bare)?,
            ),
        ],
    )
}

/// Enum, tag 20, 21 slots, four collections.
fn enumeration(o: &Obj<'_>) -> Result<Brace> {
    let [ref_type, ref_value] = o.generated("Ref")?;
    let [manager_type, manager_value] = o.generated("Manager")?;
    let [list_type, list_value] = o.generated("List")?;
    let owner = vec![
        num(20),
        ref_type,
        ref_value,
        manager_type,
        manager_value,
        o.wrapped_header(),
        o.flag("UseStandardCommands")?,
        list_type,
        list_value,
        o.form("DefaultListForm")?,
        o.form("DefaultChoiceForm")?,
        o.code("ChoiceMode", CHOICE_MODE)?,
        o.flag("QuickChoice")?,
        o.form("AuxiliaryListForm")?,
        o.form("AuxiliaryChoiceForm")?,
        o.loc("ListPresentation"),
        o.loc("ExtendedListPresentation"),
        o.loc("Explanation"),
        o.standard_attributes(ENUM_STANDARD)?,
        o.characteristics()?,
        o.code("ChoiceHistoryOnInput", CHOICE_HISTORY_ON_INPUT)?,
    ];
    o.root(
        owner,
        vec![
            collection("33f2e54b-37ce-4a7a-a569-b648d7aa4634", o.forms()?),
            collection(TEMPLATES, o.templates()?),
            collection(
                "6d8d73a7-ba29-401d-9032-3872ec2d6433",
                o.commands(CommandWrapper::Owner)?,
            ),
            collection("bee0a08c-07eb-40c0-8544-5c364c171465", o.enum_values()?),
        ],
    )
}
