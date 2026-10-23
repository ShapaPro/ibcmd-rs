//! `infobase config export objects`: the files of the named objects only.
//!
//! An object is the rows of `ConfigDumpInfo.xml` it owns. The rule is read
//! off 8.5.1.1529 (`ibcmd infobase config export objects --out=<dir>` over a
//! БСП file infobase):
//! - `Catalog.Валюты` is the row of that name and the rows named by it and
//!   one more segment (`.ObjectModule`, `.ManagerModule`, `.Help`): its XML,
//!   modules and help, 5 files; its forms only with `--recursive` (23
//!   files). A nested subsystem is an object of its own the same way.
//! - a row is named by its own name: `Catalog.Валюты.ObjectModule` is the
//!   module alone, `Catalog.Валюты.Form.ФормаЭлемента.Form` the form's
//!   `Form.xml` and module;
//! - `Configuration` (or `Configuration.<name>`) is `Configuration.xml` and
//!   the files of `Ext/`; with `--recursive` the whole configuration;
//! - names are compared without regard to case (`catalog.валюты` is
//!   `Catalog.Валюты`); `ConfigDumpInfo.xml` is not written;
//! - a part of a row (`Catalog.Валюты.Attribute.Наценка`) is refused, as is
//!   a name the configuration does not have.
//!
//! The platform takes the Russian names of the kinds too (`Справочник.Валюты`);
//! ibcmd-rs does not yet.

use std::collections::BTreeSet;

use crate::mssql_dump::incremental::DumpInfo;

/// The kind of the configuration's own row.
const CONFIGURATION: &str = "configuration";

/// Why an object cannot be exported, in the platform's words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    /// The configuration has no such object.
    Absent(String),
    /// A part of a row: written in its owner's file only.
    Part(String),
    /// A kind named in Russian: not served yet.
    RussianKind(String),
}

impl Refused {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::Absent(name) => {
                format!("Объект метаданных {name} не существует в конфигурации.")
            }
            Self::Part(name) => format!(
                "Невозможно сохранить объект метаданных {name} в отдельный XML-файл. \
Вы можете сохранить его как часть родительского объекта."
            ),
            Self::RussianKind(name) => format!(
                "Имя объекта {name}: русские имена видов метаданных в этой версии ibcmd-rs \
не поддерживаются, укажите английское (Catalog.Имя)"
            ),
        }
    }

    /// Whether the platform would export it: then ibcmd-rs refuses it as not
    /// served rather than failed.
    pub(crate) fn unsupported(&self) -> bool {
        matches!(self, Self::RussianKind(_))
    }
}

/// The ids of the rows the named objects own.
pub(crate) fn rows(
    info: &DumpInfo,
    objects: &[String],
    recursive: bool,
) -> Result<BTreeSet<String>, Refused> {
    let mut selected = BTreeSet::new();
    for object in objects {
        let wanted = object.to_lowercase();
        let kind = wanted.split('.').next().unwrap_or_default();
        if !kind.is_ascii() {
            return Err(Refused::RussianKind(object.clone()));
        }
        let configuration = kind == CONFIGURATION && !wanted.contains('.');
        let mut found = false;
        for entry in &info.entries {
            let name = entry.name.to_lowercase();
            let owned = if configuration {
                let Some(rest) = name
                    .strip_prefix(CONFIGURATION)
                    .and_then(|rest| rest.strip_prefix('.'))
                else {
                    if recursive {
                        found = true;
                        selected.insert(entry.id.clone());
                    }
                    continue;
                };
                // `Configuration.<name>` and its `Ext` rows
                recursive || rest.split('.').count() <= 2
            } else if name == wanted {
                true
            } else {
                name.strip_prefix(wanted.as_str())
                    .and_then(|rest| rest.strip_prefix('.'))
                    .is_some_and(|rest| recursive || !rest.contains('.'))
            };
            if owned {
                found = true;
                selected.insert(entry.id.clone());
            }
        }
        if !found {
            let part = info
                .parts
                .values()
                .any(|name| name.to_lowercase() == wanted);
            return Err(if part {
                Refused::Part(object.clone())
            } else {
                Refused::Absent(object.clone())
            });
        }
    }
    Ok(selected)
}

/// The `ConfigDumpInfo.xml` an export with `--base` compares with to write
/// the `selected` rows only: `current` with another version for each of
/// them. The rest of the file is left as it is, so the names and parts of
/// the base are the configuration's own (no rename: no full export).
pub(crate) fn base_for(current: &str, selected: &BTreeSet<String>) -> anyhow::Result<String> {
    let mut base = current.to_string();
    for id in selected {
        let marker = format!("id=\"{id}\" configVersion=\"");
        let Some(start) = base.find(&marker).map(|at| at + marker.len()) else {
            anyhow::bail!("the row {id} has no version in ConfigDumpInfo.xml");
        };
        let end = base[start..]
            .find('"')
            .map(|length| start + length)
            .ok_or_else(|| anyhow::anyhow!("the version of the row {id} is not closed"))?;
        base.replace_range(start..end, "");
    }
    Ok(base)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(rows: &[&str]) -> String {
        let mut xml = String::from(
            "<ConfigDumpInfo format=\"Hierarchical\" version=\"2.21\"><ConfigVersions>",
        );
        for (index, name) in rows.iter().enumerate() {
            xml.push_str(&format!(
                "<Metadata name=\"{name}\" id=\"{index}\" configVersion=\"v{index}\">\
<Metadata name=\"{name}.Attribute.Реквизит\" id=\"p{index}\"/></Metadata>"
            ));
        }
        xml.push_str("</ConfigVersions></ConfigDumpInfo>");
        xml
    }

    const ROWS: &[&str] = &[
        "Catalog.Валюты",
        "Catalog.Валюты.Form.ФормаЭлемента",
        "Catalog.Валюты.Form.ФормаЭлемента.Form",
        "Catalog.Валюты.Help",
        "Catalog.Валюты.ObjectModule",
        "Catalog.ВалютыДругие",
        "Configuration.БСП",
        "Configuration.БСП.SessionModule",
        "Subsystem.А",
        "Subsystem.А.Subsystem.Б",
    ];

    fn names(objects: &[&str], recursive: bool) -> Result<Vec<&'static str>, Refused> {
        let info = DumpInfo::parse(info(ROWS).as_bytes()).unwrap();
        let objects = objects
            .iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>();
        let ids = rows(&info, &objects, recursive)?;
        Ok(ROWS
            .iter()
            .enumerate()
            .filter(|(index, _)| ids.contains(&index.to_string()))
            .map(|(_, name)| *name)
            .collect())
    }

    #[test]
    fn an_object_is_its_row_and_its_own_rows() {
        assert_eq!(
            names(&["Catalog.Валюты"], false).unwrap(),
            [
                "Catalog.Валюты",
                "Catalog.Валюты.Help",
                "Catalog.Валюты.ObjectModule"
            ]
        );
        assert_eq!(
            names(&["catalog.валюты"], true).unwrap(),
            [
                "Catalog.Валюты",
                "Catalog.Валюты.Form.ФормаЭлемента",
                "Catalog.Валюты.Form.ФормаЭлемента.Form",
                "Catalog.Валюты.Help",
                "Catalog.Валюты.ObjectModule"
            ]
        );
        assert_eq!(
            names(&["Catalog.Валюты.Form.ФормаЭлемента.Form"], false).unwrap(),
            ["Catalog.Валюты.Form.ФормаЭлемента.Form"]
        );
        assert_eq!(names(&["Subsystem.А"], false).unwrap(), ["Subsystem.А"]);
        // an object named again inside another is no error (8.5.1.1529)
        assert_eq!(
            names(&["Catalog.Валюты", "Catalog.Валюты.ObjectModule"], false).unwrap(),
            [
                "Catalog.Валюты",
                "Catalog.Валюты.Help",
                "Catalog.Валюты.ObjectModule"
            ]
        );
        assert_eq!(
            names(&["Subsystem.А"], true).unwrap(),
            ["Subsystem.А", "Subsystem.А.Subsystem.Б"]
        );
    }

    #[test]
    fn the_configuration_is_its_root_and_with_recursive_everything() {
        assert_eq!(
            names(&["Configuration"], false).unwrap(),
            ["Configuration.БСП", "Configuration.БСП.SessionModule"]
        );
        assert_eq!(
            names(&["configuration.бсп"], false).unwrap(),
            ["Configuration.БСП", "Configuration.БСП.SessionModule"]
        );
        assert_eq!(names(&["Configuration"], true).unwrap(), ROWS);
    }

    #[test]
    fn parts_absent_objects_and_russian_kinds_are_refused() {
        assert_eq!(
            names(&["Catalog.Валюты.Attribute.Реквизит"], false),
            Err(Refused::Part(
                "Catalog.Валюты.Attribute.Реквизит".to_string()
            ))
        );
        assert_eq!(
            names(&["Catalog.Нет"], false),
            Err(Refused::Absent("Catalog.Нет".to_string()))
        );
        assert_eq!(
            names(&["Справочник.Валюты"], false),
            Err(Refused::RussianKind("Справочник.Валюты".to_string()))
        );
    }

    #[test]
    fn the_base_differs_in_the_versions_of_the_selected_rows_only() {
        let current = info(ROWS);
        let base = base_for(&current, &BTreeSet::from(["1".to_string()])).unwrap();
        assert_eq!(
            base,
            current.replace(
                "id=\"1\" configVersion=\"v1\"",
                "id=\"1\" configVersion=\"\""
            )
        );
        let parsed = DumpInfo::parse(base.as_bytes()).unwrap();
        assert_eq!(parsed.entries.len(), ROWS.len());
    }
}
