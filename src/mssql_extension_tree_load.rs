//! Whole-tree load of a configuration extension.
//!
//! The extension's active image is exported (the export equals the platform's
//! `config export --extension`, measured on the four БСП extensions), the tree
//! the user hands in is compared with that export file by file, and only the
//! objects a changed file belongs to are compiled -- against the active rows,
//! by the same offline staging compiler `cf load` uses for `.cfe` containers
//! (`crate::load::compiled::compile_edit`). The proposed image is exported
//! again and must equal the tree, `ConfigDumpInfo.xml` aside: an edit the
//! compiler does not write faithfully is refused by name and nothing is staged.
//!
//! Activation publishes rows and restructures nothing, so an edit that changes
//! what a database table holds is refused here too: only the bodies of an
//! object (module, form, template, picture, rights, help) and the descriptors
//! of the families that own no table are taken.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use ibcmd_core::limits::ResourceLimits;
use ibcmd_core::storage::{StorageEntry, StorageImage};

use crate::legacy_version::InfobaseConfigSourceVersion;
use crate::load::compiled::{compile_edit, is_dropped, new_entry, with_payload};
use crate::load::{TreeDiff, keys_by_output, relative_files};
use crate::mssql_extension_stage::ExtensionStageRow;
use crate::sql::SqlExec;

const CONFIG_DUMP_INFO: &str = "ConfigDumpInfo.xml";

/// Families whose objects own no database table -- (source directory, the tag
/// of the root's `<ChildObjects>` line) -- so a change of the descriptor
/// itself, or an object of the family coming or going, restructures nothing
/// (`Forms/` and `Templates/` of an object are nested and always allowed to
/// change in place).
const TABLELESS_FAMILIES: &[(&str, &str)] = &[
    ("CommonForms", "CommonForm"),
    ("CommonModules", "CommonModule"),
    ("CommonPictures", "CommonPicture"),
    ("CommonTemplates", "CommonTemplate"),
    ("Roles", "Role"),
];

/// What a whole-tree load proposes.
#[derive(Debug)]
pub(crate) struct TreeEdit {
    /// The complete content rows of the proposed image (no `configinfo`).
    pub rows: Vec<ExtensionStageRow>,
    /// The files of the tree that differ from the active image's export.
    pub changed_files: Vec<String>,
    /// The objects compiled, as source prefixes.
    pub compiled_objects: Vec<String>,
    /// The rows that replace or add an active row.
    pub replaced_rows: usize,
}

/// A directory removed when dropped, on a panic too.
struct Scratch(PathBuf, std::cell::Cell<bool>);

impl Scratch {
    fn new(tag: &str) -> Self {
        Self(
            std::env::temp_dir().join(format!(
                "ibcmd-extload-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or_default()
            )),
            std::cell::Cell::new(false),
        )
    }

    /// Lab aid: `IBCMD_RS_EXTLOAD_KEEP=1` leaves the directory for a look.
    fn keep_if_asked(&self) -> bool {
        let keep = std::env::var_os("IBCMD_RS_EXTLOAD_KEEP").is_some();
        self.1.set(keep);
        keep
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if !self.1.get() {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

/// Compiles the difference between `input_dir` and the export of `active`;
/// `None` when the tree is the export. With `prefixes` (source paths: an object
/// as `Catalogs/К`, a file) only the files under them are taken, and every
/// other file of the proposal stays the active image's.
///
/// One extension per process: the offline staging compiler reads its base rows
/// from a process-wide table.
pub(crate) fn compile_tree_edit(
    sql: &SqlExec,
    database: &str,
    source_version: InfobaseConfigSourceVersion,
    input_dir: &Path,
    active: &StorageImage,
    prefixes: &[String],
) -> Result<Option<TreeEdit>> {
    let base_export = Scratch::new("base");
    let report = crate::mssql_dump::extension::export_extension_image_to_source(
        active,
        &base_export.0,
        source_version,
        Some(crate::mssql_dump::extension::base_index_provider(
            sql,
            database,
            source_version,
        )),
    )
    .context("failed to export the active extension image for comparison")?;
    if report.storage.failed > 0 || report.storage.opaque > 0 {
        bail!(
            "the export of the active image has {} failed and {} opaque rows, so the tree cannot \
             be compared with it; nothing can be loaded whole",
            report.storage.failed,
            report.storage.opaque
        );
    }
    let mut keys = keys_by_output(&report);
    let mut diff = diff_trees_normalized(&base_export.0, input_dir)?;
    let prefixes = prefixes
        .iter()
        .map(|prefix| prefix.trim_matches('/').replace('\\', "/"))
        .collect::<Vec<_>>();
    if !prefixes.is_empty() {
        for list in [&mut diff.changed, &mut diff.added, &mut diff.removed] {
            list.retain(|path| under_prefixes(path, &prefixes));
        }
    }
    add_side_file_keys(
        &mut keys,
        diff.changed.iter().chain(&diff.added).chain(&diff.removed),
    );
    if diff.changed.is_empty() && diff.added.is_empty() && diff.removed.is_empty() {
        return Ok(None);
    }
    let root_children_only = diff.changed.iter().any(|path| path == "Configuration.xml")
        && match (
            std::fs::read_to_string(base_export.0.join("Configuration.xml")),
            std::fs::read_to_string(input_dir.join("Configuration.xml")),
        ) {
            (Ok(active_root), Ok(tree_root)) => root_children_only_change(&active_root, &tree_root),
            _ => false,
        };
    let refused = structural_refusals(&diff, root_children_only);
    if !refused.is_empty() {
        bail!(
            "these changes are not loaded (activation restructures nothing, so an edit of what a \
             table holds is the platform's own load): {}",
            list_paths(&refused)
        );
    }

    let base_rows = active
        .entries()
        .iter()
        .map(|entry| {
            (
                entry.logical_key().as_str().to_owned(),
                entry.packed_payload().to_vec(),
            )
        })
        .collect::<HashMap<_, _>>();
    let edit = compile_edit(input_dir, base_rows, &diff, &keys)
        .map_err(|error| {
            let text = error.to_string();
            if text.contains("has already run in this process") {
                anyhow!(
                    "the staging compiler reads its base rows from a process-wide table, so one \
                     process compiles one extension's tree; load each changed extension with its \
                     own --extension"
                )
            } else {
                anyhow!("{text}")
            }
        })
        .context("failed to compile the changed objects")?;

    let image = proposed_image(active, &edit.rows, &edit.dropped)?;
    verify_export(
        &image,
        &Expected {
            tree: input_dir,
            active_export: &base_export.0,
            prefixes: &prefixes,
        },
        source_version,
        sql,
        database,
    )?;

    let rows = image
        .entries()
        .iter()
        .map(|entry| {
            ExtensionStageRow::new(
                entry.logical_key().as_str(),
                entry.packed_payload().to_vec(),
            )
        })
        .collect::<Vec<_>>();
    let mut changed_files = diff
        .changed
        .iter()
        .chain(&diff.added)
        .chain(&diff.removed)
        .cloned()
        .collect::<Vec<_>>();
    changed_files.sort();
    Ok(Some(TreeEdit {
        rows,
        changed_files,
        compiled_objects: edit.prefixes.into_iter().collect(),
        replaced_rows: edit.rows.len(),
    }))
}

/// A module's text as the platform stores it: no byte order mark of its own
/// (the export writes one) and CRLF line ends. An editor may save either
/// differently, and the load stores what the platform stores.
fn module_text_normalized(bytes: &[u8]) -> Vec<u8> {
    let body = bytes.strip_prefix(&[0xef, 0xbb, 0xbf][..]).unwrap_or(bytes);
    let mut out = Vec::with_capacity(body.len() + body.len() / 32);
    for (index, &byte) in body.iter().enumerate() {
        if byte == b'\n' && (index == 0 || body[index - 1] != b'\r') {
            out.push(b'\r');
        }
        out.push(byte);
    }
    out
}

/// Two files of a tree are the same file: byte for byte, and a module
/// (`.bsl`) also when only its byte order mark or line ends differ.
fn same_content(path: &str, left: &[u8], right: &[u8]) -> bool {
    left == right
        || (path.ends_with(".bsl") && module_text_normalized(left) == module_text_normalized(right))
}

/// `diff_trees` for a tree an editor may have saved: `ConfigDumpInfo.xml` is
/// derived and left out, and a module counts as changed only by its text.
fn diff_trees_normalized(base: &Path, edited: &Path) -> Result<TreeDiff> {
    let (a, b) = (relative_files(base)?, relative_files(edited)?);
    let mut diff = TreeDiff::default();
    for path in a.intersection(&b) {
        if path == CONFIG_DUMP_INFO {
            continue;
        }
        let (x, y) = (
            std::fs::read(base.join(path))?,
            std::fs::read(edited.join(path))?,
        );
        if !same_content(path, &x, &y) {
            diff.changed.push(path.clone());
        }
    }
    diff.added = b
        .difference(&a)
        .filter(|path| path.as_str() != CONFIG_DUMP_INFO)
        .cloned()
        .collect();
    diff.removed = a
        .difference(&b)
        .filter(|path| path.as_str() != CONFIG_DUMP_INFO)
        .cloned()
        .collect();
    Ok(diff)
}

/// The export report names the file an entry is written as, not the files it
/// is unpacked into: the pages of a help (`Ext/Help/ru.html`) and the picture
/// data (`Ext/Picture/Picture.png`) are written beside the `Ext/Help.xml` and
/// `Ext/Picture.xml` of the entry that holds them. A file `<dir>/Ext/<Name>/...`
/// with no entry of its own belongs to the entry of `<dir>/Ext/<Name>.xml`.
fn add_side_file_keys<'a>(
    keys: &mut BTreeMap<String, String>,
    paths: impl Iterator<Item = &'a String>,
) {
    for path in paths {
        if keys.contains_key(path) {
            continue;
        }
        let segments = path.split('/').collect::<Vec<_>>();
        // `<...>/Ext/<Name>/<file>`, at any depth of nested directories.
        let Some(ext_at) = segments.iter().rposition(|segment| *segment == "Ext") else {
            continue;
        };
        if segments.len() < ext_at + 3 {
            continue;
        }
        let owner = format!(
            "{}/{}.xml",
            segments[..=ext_at].join("/"),
            segments[ext_at + 1]
        );
        if let Some(key) = keys.get(&owner).cloned() {
            keys.insert(path.clone(), key);
        }
    }
}

/// The active entries in their order with `rows` put in (replaced where the
/// image has the key, appended where it has not) and the `dropped` keys and
/// their bodies left out.
fn proposed_image(
    active: &StorageImage,
    rows: &BTreeMap<String, Vec<u8>>,
    dropped: &BTreeSet<String>,
) -> Result<StorageImage> {
    let base = active.entries();
    let mut entries: Vec<StorageEntry> = Vec::with_capacity(base.len() + rows.len());
    let mut placed = BTreeSet::new();
    for entry in base {
        let key = entry.logical_key().as_str();
        if is_dropped(dropped, key) {
            continue;
        }
        match rows.get(key) {
            Some(packed) => {
                entries.push(with_payload(entry, packed)?);
                placed.insert(key.to_owned());
            }
            None => entries.push(entry.clone()),
        }
    }
    let template = base
        .first()
        .context("the active image has no entries to take an element header from")?;
    for (key, packed) in rows {
        if !placed.contains(key) && !is_dropped(dropped, key) {
            entries.push(new_entry(template, key, packed)?);
        }
    }
    let total = entries
        .iter()
        .map(|entry| entry.packed_payload().len() as u64)
        .sum::<u64>();
    StorageImage::with_retained_byte_limit(
        entries,
        ResourceLimits::for_input_bytes(total).max_retained_bytes_usize(),
    )
    .context("the proposed extension image is not valid")
}

/// What the proposed image must export to: the tree, and -- for a load
/// restricted to `prefixes` -- the active image's own export outside them.
struct Expected<'a> {
    tree: &'a Path,
    active_export: &'a Path,
    prefixes: &'a [String],
}

impl Expected<'_> {
    /// The directory holding the expected file of `path`.
    fn root_of(&self, path: &str) -> &Path {
        if self.prefixes.is_empty() || under_prefixes(path, self.prefixes) {
            self.tree
        } else {
            self.active_export
        }
    }
}

/// `path` is one of the `prefixes`, or a file under it (`Catalogs/К` takes
/// `Catalogs/К.xml` and `Catalogs/К/...`).
fn under_prefixes(path: &str, prefixes: &[String]) -> bool {
    prefixes.iter().any(|prefix| {
        path == prefix
            || path == format!("{prefix}.xml")
            || path
                .strip_prefix(prefix.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

/// The proposed image exported back must be the tree, file for file.
fn verify_export(
    image: &StorageImage,
    expected: &Expected<'_>,
    source_version: InfobaseConfigSourceVersion,
    sql: &SqlExec,
    database: &str,
) -> Result<()> {
    let scratch = Scratch::new("proposed");
    crate::mssql_dump::extension::export_extension_image_to_source(
        image,
        &scratch.0,
        source_version,
        Some(crate::mssql_dump::extension::base_index_provider(
            sql,
            database,
            source_version,
        )),
    )
    .context("failed to export the proposed extension image")?;
    let back = relative_files(&scratch.0)?;
    let mut paths = back.clone();
    paths.extend(relative_files(expected.tree)?);
    if !expected.prefixes.is_empty() {
        paths.extend(relative_files(expected.active_export)?);
    }
    let mut differ = Vec::new();
    for path in &paths {
        if path == CONFIG_DUMP_INFO {
            continue;
        }
        let want = expected.root_of(path).join(path);
        let same = match (want.is_file(), back.contains(path)) {
            (true, true) => same_content(
                path,
                &std::fs::read(&want).context("expected file")?,
                &std::fs::read(scratch.0.join(path)).context("exported file")?,
            ),
            (false, false) => true,
            _ => false,
        };
        if !same {
            differ.push(path.clone());
        }
    }
    if differ.is_empty() {
        return Ok(());
    }
    let kept = if scratch.keep_if_asked() {
        format!(" (the export is kept in {})", scratch.0.display())
    } else {
        String::new()
    };
    bail!(
        "the proposed extension does not export back to the tree -- the compiler does not write \
         these faithfully yet, so nothing was staged: {}{kept}",
        list_paths(&differ)
    );
}

fn list_paths(paths: &[String]) -> String {
    const LISTED: usize = 20;
    let mut text = paths
        .iter()
        .take(LISTED)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if paths.len() > LISTED {
        text.push_str(&format!(" and {} more", paths.len() - LISTED));
    }
    text
}

/// The changed, added or removed files a stage cannot carry: activation
/// publishes rows and restructures nothing, so an edit that changes what an
/// object's table holds (a descriptor of a family that owns one, an object or
/// a body added or removed) belongs to the platform's own load. Each is named
/// with the reason. `root_children_only` says the root descriptor differs from
/// the active one in the `<ChildObjects>` lines of table-less families alone.
pub(crate) fn structural_refusals(diff: &TreeDiff, root_children_only: bool) -> Vec<String> {
    let mut refused = Vec::new();
    let mut check = |path: &str, moved: Option<&str>| {
        if let Some(reason) = refusal(path, moved, root_children_only) {
            refused.push(format!("{path} ({reason})"));
        }
    };
    for path in &diff.changed {
        check(path, None);
    }
    for path in &diff.added {
        check(path, Some("added"));
    }
    for path in &diff.removed {
        check(path, Some("removed"));
    }
    refused
}

fn is_tableless_family(family: &str) -> bool {
    TABLELESS_FAMILIES.iter().any(|(dir, _)| *dir == family)
}

fn refusal(path: &str, moved: Option<&str>, root_children_only: bool) -> Option<&'static str> {
    if path == CONFIG_DUMP_INFO {
        return None;
    }
    let segments = path.split('/').collect::<Vec<_>>();
    if let Some(moved) = moved {
        // An object of a table-less family comes or goes with its descriptor
        // and its bodies (the root's child list follows, see
        // `root_children_only`); the module of a form is a part of the form's
        // body row.
        match segments.as_slice() {
            [family, name] if is_tableless_family(family) && name.ends_with(".xml") => {
                return None;
            }
            [family, _, "Ext", ..] if is_tableless_family(family) => return None,
            [_, _, "Forms", _, "Ext", "Form", "Module.bsl"] => return None,
            _ => {}
        }
        return Some(if moved == "added" {
            "a file added: the object's structure changes"
        } else {
            "a file removed: the object's structure changes"
        });
    }
    if path == "Configuration.xml" {
        return if root_children_only {
            None
        } else {
            Some("the root descriptor")
        };
    }
    if path.starts_with("Ext/") {
        return if path.ends_with(".bsl") {
            None
        } else {
            Some("an asset of the root")
        };
    }
    if path.contains("/Ext/") {
        return None;
    }
    if !path.ends_with(".xml") {
        return Some("a file outside every Ext directory");
    }
    match segments.as_slice() {
        [family, _] if is_tableless_family(family) => None,
        [_, _] => Some("the descriptor of an object that may own a table"),
        [_, _, "Forms" | "Templates", _] => None,
        _ => Some("a descriptor outside the known layout"),
    }
}

/// The root descriptors differ in the `<ChildObjects>` lines of table-less
/// families and nowhere else.
pub(crate) fn root_children_only_change(active: &str, tree: &str) -> bool {
    fn without_tableless_children(text: &str) -> Option<String> {
        let open = "<ChildObjects>";
        let close = "</ChildObjects>";
        let start = text.find(open)? + open.len();
        let end = start + text[start..].find(close)?;
        let kept = text[start..end]
            .lines()
            .filter(|line| {
                let line = line.trim();
                !TABLELESS_FAMILIES.iter().any(|(_, tag)| {
                    line.strip_prefix(&format!("<{tag}>"))
                        .is_some_and(|rest| rest.ends_with(&format!("</{tag}>")))
                })
            })
            .collect::<Vec<_>>()
            .join("\n");
        Some(format!("{}{}{}", &text[..start], kept, &text[end..]))
    }
    let (active, tree) = (active.replace("\r\n", "\n"), tree.replace("\r\n", "\n"));
    match (
        without_tableless_children(&active),
        without_tableless_children(&tree),
    ) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diff(changed: &[&str], added: &[&str], removed: &[&str]) -> TreeDiff {
        let own = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect();
        TreeDiff {
            changed: own(changed),
            added: own(added),
            removed: own(removed),
        }
    }

    #[test]
    fn bodies_and_tableless_descriptors_change_in_place() {
        let refused = structural_refusals(
            &diff(
                &[
                    "Catalogs/К/Ext/ObjectModule.bsl",
                    "Catalogs/К/Forms/Ф/Ext/Form.xml",
                    "Catalogs/К/Forms/Ф.xml",
                    "Reports/О/Templates/М/Ext/Template.xml",
                    "CommonPictures/К.xml",
                    "CommonPictures/К/Ext/Picture/Picture.png",
                    "Roles/Р/Ext/Rights.xml",
                    "Ext/ManagedApplicationModule.bsl",
                    "ConfigDumpInfo.xml",
                ],
                &[],
                &[],
            ),
            false,
        );
        assert!(refused.is_empty(), "{refused:?}");
    }

    #[test]
    fn a_side_file_belongs_to_the_entry_of_its_manifest() {
        let mut keys = BTreeMap::new();
        keys.insert("К/Ext/Help.xml".to_owned(), "u.1".to_owned());
        keys.insert("К/Ext/Picture.xml".to_owned(), "p.0".to_owned());
        keys.insert("К/Ext/Form.xml".to_owned(), "f.0".to_owned());
        let paths = [
            "К/Ext/Help/ru.html".to_owned(),
            "К/Ext/Picture/Picture.png".to_owned(),
            "К/Ext/Other/x.bin".to_owned(),
            "К/Ext/Form.xml".to_owned(),
            "К/Ext/Help.xml".to_owned(),
        ];
        add_side_file_keys(&mut keys, paths.iter());
        assert_eq!(keys["К/Ext/Help/ru.html"], "u.1");
        assert_eq!(keys["К/Ext/Picture/Picture.png"], "p.0");
        assert!(!keys.contains_key("К/Ext/Other/x.bin"));
        assert_eq!(keys["К/Ext/Form.xml"], "f.0");
    }

    #[test]
    fn a_module_saved_without_a_bom_or_with_lf_is_the_same_module() {
        let stored = "\u{feff}А = 1;\r\nБ = 2;\r\n".as_bytes();
        let lf = "А = 1;\nБ = 2;\n".as_bytes();
        assert!(same_content("М/Ext/Module.bsl", stored, lf));
        assert!(same_content("М/Ext/Module.bsl", stored, stored));
        let other = "А = 1;\r\nБ = 3;\r\n".as_bytes();
        assert!(!same_content("М/Ext/Module.bsl", stored, other));
        // Only a module is compared by its text.
        assert!(!same_content("М/Ext/Form.xml", stored, lf));
    }

    #[test]
    fn descriptors_that_may_own_a_table_are_refused() {
        let refused = structural_refusals(
            &diff(
                &["Catalogs/К.xml", "Configuration.xml", "Ext/Logo.png"],
                &[],
                &[],
            ),
            false,
        );
        assert_eq!(refused.len(), 3, "{refused:?}");
        assert!(refused[0].starts_with("Catalogs/К.xml"), "{refused:?}");
    }

    #[test]
    fn a_body_added_or_removed_changes_the_structure_of_its_object() {
        let refused = structural_refusals(
            &diff(
                &[],
                &["Catalogs/К/Ext/Help.xml", "Catalogs/К/Ext/ObjectModule.bsl"],
                &[
                    "Catalogs/К/Ext/ObjectModule.bin",
                    "Catalogs/К/Ext/Help/ru.html",
                ],
            ),
            false,
        );
        assert_eq!(refused.len(), 4, "{refused:?}");
    }

    #[test]
    fn the_module_of_a_form_and_an_object_of_a_tableless_family_come_and_go() {
        let refused = structural_refusals(
            &diff(
                &["Configuration.xml"],
                &[
                    "Catalogs/К/Forms/Ф/Ext/Form/Module.bsl",
                    "CommonPictures/Н.xml",
                    "CommonPictures/Н/Ext/Picture.xml",
                    "CommonPictures/Н/Ext/Picture/Picture.png",
                    "CommonModules/М.xml",
                    "CommonModules/М/Ext/Module.bsl",
                ],
                &["Roles/Р.xml", "Roles/Р/Ext/Rights.xml"],
            ),
            true,
        );
        assert!(refused.is_empty(), "{refused:?}");
        // The same without the root's child list explained: only it is refused.
        let refused = structural_refusals(
            &diff(&["Configuration.xml"], &["CommonPictures/Н.xml"], &[]),
            false,
        );
        assert_eq!(refused.len(), 1, "{refused:?}");
        // A new object of a family that may own a table is refused, and so is
        // a new form of an object (its owner's descriptor changes too).
        let refused = structural_refusals(
            &diff(&[], &["Catalogs/К.xml", "Catalogs/К/Forms/Ф.xml"], &[]),
            true,
        );
        assert_eq!(refused.len(), 2, "{refused:?}");
    }

    #[test]
    fn a_root_differing_in_tableless_children_only_is_recognised() {
        let root = |children: &[&str], comment: &str| {
            format!(
                "<Configuration>\r\n\t<Properties><Comment>{comment}</Comment></Properties>\r\n\t<ChildObjects>\r\n{}\t</ChildObjects>\r\n</Configuration>",
                children
                    .iter()
                    .map(|line| format!("\t\t{line}\r\n"))
                    .collect::<String>()
            )
        };
        let base = root(
            &[
                "<Language>Русский</Language>",
                "<CommonPicture>А</CommonPicture>",
            ],
            "c",
        );
        let with_picture = root(
            &[
                "<Language>Русский</Language>",
                "<CommonPicture>А</CommonPicture>",
                "<CommonPicture>Б</CommonPicture>",
            ],
            "c",
        );
        assert!(root_children_only_change(&base, &with_picture));
        // A catalog in the list, or another property, is more than that.
        let with_catalog = root(
            &[
                "<Language>Русский</Language>",
                "<CommonPicture>А</CommonPicture>",
                "<Catalog>К</Catalog>",
            ],
            "c",
        );
        assert!(!root_children_only_change(&base, &with_catalog));
        let with_comment = root(
            &[
                "<Language>Русский</Language>",
                "<CommonPicture>А</CommonPicture>",
            ],
            "другой",
        );
        assert!(!root_children_only_change(&base, &with_comment));
    }
}
