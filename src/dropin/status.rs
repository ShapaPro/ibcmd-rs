//! `infobase config export status`: what changed in the configuration since
//! a saved `ConfigDumpInfo.xml`.
//!
//! The rule is read off 8.5.1.1529 (`ibcmd infobase config export status
//! --base=<file>` over a БСП file infobase after a patch import):
//! - the rows are matched by their id; a row of the current state that the
//!   base does not list is `added: <name>`, one whose `configVersion`
//!   differs is `modified: <name>`, in the order of the current
//!   `ConfigDumpInfo.xml` (442 forms and an object module listed one by one
//!   after an edit of the module);
//! - once a row of the base is gone the whole configuration is reported as
//!   `modified: all`, nothing else (the same import with the module deleted
//!   instead: 441 modified rows and one gone gave that single line);
//! - `--short` writes `A:` and `M:` in place of `added:` and `modified:`;
//! - the state compared with itself gives no line at all.

use anyhow::Result;

use crate::mssql_dump::incremental::DumpInfo;

/// One line of the status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Change {
    Added(String),
    Modified(String),
    /// A row of the base is gone: the whole configuration changed.
    All,
}

/// The changes of `current` since `base`, both `ConfigDumpInfo.xml` bytes.
pub(crate) fn changes(current: &[u8], base: &[u8]) -> Result<Vec<Change>> {
    let current = DumpInfo::parse(current)?;
    let base = DumpInfo::parse(base)?;
    let base_versions = base
        .entries
        .iter()
        .map(|entry| (entry.id.as_str(), entry.config_version.as_str()))
        .collect::<std::collections::HashMap<_, _>>();
    let current_ids = current
        .entries
        .iter()
        .map(|entry| entry.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    if base_versions.keys().any(|id| !current_ids.contains(id)) {
        return Ok(vec![Change::All]);
    }
    Ok(current
        .entries
        .iter()
        .filter_map(|entry| match base_versions.get(entry.id.as_str()) {
            None => Some(Change::Added(entry.name.clone())),
            Some(version) if *version != entry.config_version => {
                Some(Change::Modified(entry.name.clone()))
            }
            Some(_) => None,
        })
        .collect())
}

/// The lines as 8.5.1.1529 prints them, each ended by a line break.
pub(crate) fn render(changes: &[Change], short: bool) -> String {
    let (added, modified) = if short {
        ("A:", "M:")
    } else {
        ("added:", "modified:")
    };
    let mut text = String::new();
    for change in changes {
        let (label, name) = match change {
            Change::Added(name) => (added, name.as_str()),
            Change::Modified(name) => (modified, name.as_str()),
            Change::All => (modified, "all"),
        };
        text.push_str(label);
        text.push(' ');
        text.push_str(name);
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(rows: &[(&str, &str, &str)]) -> Vec<u8> {
        let mut xml = String::from(
            "\u{feff}<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n\
<ConfigDumpInfo xmlns=\"http://v8.1c.ru/8.3/xcf/dumpinfo\" format=\"Hierarchical\" version=\"2.21\">\r\n\
\t<ConfigVersions>\r\n",
        );
        for (name, id, version) in rows {
            xml.push_str(&format!(
                "\t\t<Metadata name=\"{name}\" id=\"{id}\" configVersion=\"{version}\"/>\r\n"
            ));
        }
        xml.push_str("\t</ConfigVersions>\r\n</ConfigDumpInfo>");
        xml.into_bytes()
    }

    #[test]
    fn added_and_modified_rows_are_listed_in_the_current_order() {
        let base = info(&[("Catalog.Б", "b", "1"), ("CommonModule.М", "m", "1")]);
        let current = info(&[
            ("CommonForm.Ф", "f", "1"),
            ("CommonForm.Ф.Form", "f.0", "1"),
            ("Catalog.Б", "b", "1"),
            ("CommonModule.М", "m", "2"),
        ]);
        let changes = changes(&current, &base).unwrap();
        assert_eq!(
            render(&changes, false),
            "added: CommonForm.Ф\nadded: CommonForm.Ф.Form\nmodified: CommonModule.М\n"
        );
        assert_eq!(
            render(&changes, true),
            "A: CommonForm.Ф\nA: CommonForm.Ф.Form\nM: CommonModule.М\n"
        );
    }

    #[test]
    fn a_gone_row_makes_the_whole_configuration_modified() {
        let base = info(&[
            ("Catalog.Б", "b", "1"),
            ("Catalog.Б.ObjectModule", "b.0", "1"),
        ]);
        let current = info(&[("Catalog.Б", "b", "2"), ("CommonForm.Ф", "f", "1")]);
        let changes = changes(&current, &base).unwrap();
        assert_eq!(changes, vec![Change::All]);
        assert_eq!(render(&changes, false), "modified: all\n");
        assert_eq!(render(&changes, true), "M: all\n");
    }

    #[test]
    fn the_state_against_itself_has_no_line() {
        let state = info(&[("Catalog.Б", "b", "1")]);
        assert_eq!(render(&changes(&state, &state).unwrap(), false), "");
    }
}
