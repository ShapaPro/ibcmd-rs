//! The root `Configuration` object of an extension.
//!
//! The record is the ordinary configuration properties tuple with the same
//! member positions (`{63,...}` since 8.3.21, `{68,...}` since 8.3.24) and the
//! extension's own values in members 41-45:
//!
//! | member | holds                                                      |
//! |--------|------------------------------------------------------------|
//! | 41     | `2`: keep the mapping to the extended objects by identity  |
//! | 42     | `NamePrefix`                                               |
//! | 43     | `ConfigurationExtensionCompatibilityMode` (packed version) |
//! | 44     | purpose: 0 patch, 1 customization, 2 add-on                |
//!
//! The platform prints a fixed set of them beside the header (the properties
//! of an extension of its own) and, from the adopted header of the root, the
//! properties the extension adopted from the extended configuration.

use std::collections::BTreeMap;

use super::properties::{Meaning, meaning};
use super::{ExtensionContext, STATE_EXTENDED};
use crate::cli::InfobaseConfigSourceVersion;
use crate::mssql_dump::{
    format_full_metadata_source_xml, insert_configuration_internal_info_xml,
    insert_configuration_root_child_objects_xml, insert_metadata_properties_xml,
    parse_1c_quoted_string, parse_1c_synonyms, push_localized_property,
    push_optional_simple_property_xml, refs,
};

/// `Configuration.xml` of an extension, or `None` (the row stays opaque) when
/// the record is not one this reader can name every printed member of.
pub(crate) fn extension_root_xml(
    context: &ExtensionContext,
    text: &str,
    uuid: &str,
    object_refs: &BTreeMap<String, String>,
    source_version: InfobaseConfigSourceVersion,
) -> Option<String> {
    let parts = refs::extension_root_parts(text, uuid, object_refs)?;
    let adopted = context.adopted(&parts.header_uuid)?;
    let fields = &parts.fields;
    let field = |index: usize| fields.get(index).map(String::as_str);

    // What the header lists.
    let mut adopted_names = Vec::<&'static str>::new();
    let mut states = Vec::<(&'static str, &'static str)>::new();
    let mut run_mode_members = 0usize;
    for (guid, state) in &adopted.properties {
        match meaning("Configuration", guid)? {
            Meaning::Property(name) => adopted_names.push(name),
            Meaning::Block(name) => {
                if *state == STATE_EXTENDED {
                    states.push((name, "Extended"));
                }
            }
            Meaning::Hidden(_) => {}
            Meaning::Group(_) => run_mode_members += 1,
            Meaning::Multi(_) | Meaning::ExtendedObject => return None,
        }
    }

    // The three ids of the run-mode group appear whole or not at all.
    let run_mode = match run_mode_members {
        0 => false,
        3 => true,
        _ => return None,
    };
    let mut insert = String::new();
    let purpose = match field(44)? {
        "0" => "Patch",
        "1" => "Customization",
        "2" => "AddOn",
        _ => return None,
    };
    push_optional_simple_property_xml(&mut insert, "ConfigurationExtensionPurpose", Some(purpose));
    // Member 41 is 2 in every extension on record; 0 in an ordinary
    // configuration.
    let keep_mapping = match field(41)? {
        "2" => "true",
        _ => return None,
    };
    push_optional_simple_property_xml(
        &mut insert,
        "KeepMappingToExtendedConfigurationObjectsByIDs",
        Some(keep_mapping),
    );
    let name_prefix = parse_1c_quoted_string(field(42)?)?;
    push_optional_simple_property_xml(&mut insert, "NamePrefix", Some(&name_prefix));
    let compatibility = refs::configuration_compatibility_mode_xml(field(43)?)?;
    if let Ok(packed_version) = field(43)?.parse::<u32>() {
        context.note_compatibility(packed_version);
    }
    push_optional_simple_property_xml(
        &mut insert,
        "ConfigurationExtensionCompatibilityMode",
        Some(&compatibility),
    );
    if run_mode {
        let default_run_mode = match field(3)? {
            "0" => "OrdinaryApplication",
            "1" => "ManagedApplication",
            _ => return None,
        };
        push_optional_simple_property_xml(&mut insert, "DefaultRunMode", Some(default_run_mode));
        // UsePurposes: `{1,{"#",1708fdaa-...,1}}` is PlatformApplication.
        let purposes = crate::mssql_dump::split_1c_braced_fields(field(33)?, 0)?;
        let purpose_ok = purposes.len() == 2
            && purposes[0].trim() == "1"
            && crate::mssql_dump::split_1c_braced_fields(purposes[1].trim(), 0).is_some_and(
                |typed| {
                    typed.len() == 3
                        && parse_1c_quoted_string(typed[0].trim()).as_deref() == Some("#")
                        && typed[1].trim() == "1708fdaa-cbce-4289-b373-07a5a74bee91"
                        && typed[2].trim() == "1"
                },
            );
        if !purpose_ok {
            return None;
        }
        insert.push_str(
            "\t\t\t<UsePurposes>\r\n\t\t\t\t<v8:Value xsi:type=\"app:ApplicationUsePurpose\">PlatformApplication</v8:Value>\r\n\t\t\t</UsePurposes>\r\n",
        );
    }
    push_optional_simple_property_xml(&mut insert, "ScriptVariant", Some("Russian"));
    let roles = default_roles(field(39)?, object_refs)?;
    if !roles.is_empty() {
        insert.push_str("\t\t\t<DefaultRoles>\r\n");
        for role in &roles {
            insert.push_str(&format!(
                "\t\t\t\t<xr:Item xsi:type=\"xr:MDObjectRef\">{role}</xr:Item>\r\n"
            ));
        }
        insert.push_str("\t\t\t</DefaultRoles>\r\n");
    }
    push_optional_simple_property_xml(
        &mut insert,
        "Vendor",
        Some(&parse_1c_quoted_string(field(14)?)?),
    );
    push_optional_simple_property_xml(
        &mut insert,
        "Version",
        Some(&parse_1c_quoted_string(field(15)?)?),
    );
    if adopted_names.contains(&"DefaultLanguage") {
        let language = object_refs.get(field(10)?)?;
        if !language.starts_with("Language.") {
            return None;
        }
        push_optional_simple_property_xml(&mut insert, "DefaultLanguage", Some(language));
    }
    // The localized texts: members 4 (detailed), 5 (brief), 6 (copyright),
    // 7 (vendor address), 8 (configuration address).
    for (name, index) in [
        ("BriefInformation", 5),
        ("DetailedInformation", 4),
        ("Copyright", 6),
        ("VendorInformationAddress", 7),
        ("ConfigurationInformationAddress", 8),
    ] {
        push_localized_property(
            &mut insert,
            "\t\t\t",
            name,
            &parse_1c_synonyms(field(index)?),
        );
    }
    if run_mode {
        let interface = match field(38)? {
            "0" => "Version8_2",
            "2" => "TaxiEnableVersion8_2",
            "3" => "Taxi",
            _ => return None,
        };
        push_optional_simple_property_xml(
            &mut insert,
            "InterfaceCompatibilityMode",
            Some(interface),
        );
    }

    let mut xml = format_full_metadata_source_xml("Configuration", &parts.header, source_version);
    // ObjectBelonging opens the properties.
    let properties_open = xml.find("\t\t<Properties>\r\n")? + "\t\t<Properties>\r\n".len();
    xml.insert_str(
        properties_open,
        "\t\t\t<ObjectBelonging>Adopted</ObjectBelonging>\r\n",
    );
    insert_metadata_properties_xml(&mut xml, &insert);
    insert_configuration_internal_info_xml(&mut xml, &parts.contained_objects).ok()?;
    if !states.is_empty() {
        let mut property_states = String::new();
        for (name, state) in &states {
            property_states.push_str(&format!(
                "\t\t\t<xr:PropertyState>\r\n\t\t\t\t<xr:Property>{name}</xr:Property>\r\n\t\t\t\t<xr:State>{state}</xr:State>\r\n\t\t\t</xr:PropertyState>\r\n"
            ));
        }
        let close = xml.find("\t\t</InternalInfo>\r\n")?;
        xml.insert_str(close, &property_states);
    }
    insert_configuration_root_child_objects_xml(&mut xml, parts.child_objects.as_deref()?);
    Some(xml)
}

/// `{0,N,{"#",157fa490-...,{1,<role uuid>}}...}` as `Role.Name` references.
fn default_roles(field: &str, object_refs: &BTreeMap<String, String>) -> Option<Vec<String>> {
    let list = crate::mssql_dump::split_1c_braced_fields(field, 0)?;
    if list.first()?.trim() != "0" {
        return None;
    }
    let count: usize = list.get(1)?.trim().parse().ok()?;
    if list.len() != count + 2 {
        return None;
    }
    let mut roles = Vec::with_capacity(count);
    for entry in &list[2..] {
        let typed = crate::mssql_dump::split_1c_braced_fields(entry.trim(), 0)?;
        if typed.len() != 3
            || parse_1c_quoted_string(typed[0].trim()).as_deref() != Some("#")
            || typed[1].trim() != "157fa490-4ce9-11d4-9415-008048da11f9"
        {
            return None;
        }
        let target = crate::mssql_dump::split_1c_braced_fields(typed[2].trim(), 0)?;
        if target.len() != 2 || target[0].trim() != "1" {
            return None;
        }
        let name = object_refs.get(target[1].trim())?;
        if !name.starts_with("Role.") {
            return None;
        }
        roles.push(name.clone());
    }
    Some(roles)
}
