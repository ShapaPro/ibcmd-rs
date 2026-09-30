//! The plan of a stage that creates an object against the native result (S1-F): case d of the `trace` track,
//! a new document with an attribute and a tabular section, from the real staged snapshot `d_staged`; and
//! case c, a new catalog, from a stage made of the `c2` rows. Skipped without the lab.

use std::collections::BTreeSet;

use crate::metadata_model::brace::{Brace, serialize};
use crate::restructure::caches::tests_corpus::{ROOT_ROW, Snap, lab_snap, row_name};
use crate::restructure::create::{context_uuids, new_descriptor_names};
use crate::restructure::names::{DbNames, deflate, inflate};
use crate::restructure::plan::{Inputs, Plan, PlanOptions, StagedImage, plan};
use crate::restructure::schema::DbSchema;

const VERSION: &str = "00000000-0000-0000-0000-00000000cafe";

fn bare(name: &str) -> bool {
    name.len() == 36 && !name.contains('.')
}

/// The plan's input from a snapshot that holds a staged image (`ConfigSave`).
fn inputs_of(snap: &Snap) -> Inputs {
    let mut inputs = Inputs {
        schema: snap.schema().unwrap(),
        main_names: snap.stored_row("Params", "DBNames").unwrap(),
        root_row: snap.stored_row("Config", "root").unwrap(),
        ..Inputs::default()
    };
    for name in snap.names("Params") {
        if name.starts_with("DBNames-Ext-") {
            inputs
                .extension_names
                .push((name.clone(), snap.stored_row("Params", &name).unwrap()));
        }
        if name.ends_with(".si") || name == "siVersions" {
            inputs
                .cache_rows
                .push((name.clone(), snap.stored_row("Params", &name).unwrap()));
        }
    }
    let mut image = StagedImage {
        old_files: snap.names("Config").into_iter().collect(),
        new_files: snap.names("ConfigSave").into_iter().collect(),
        deleted: snap.stored_row("ConfigSave", "deleted"),
        ..StagedImage::default()
    };
    for name in snap.names("Config") {
        if bare(&name) {
            image
                .old_descriptors
                .insert(name.clone(), snap.stored_row("Config", &name).unwrap());
        }
    }
    for name in snap.names("ConfigSave") {
        if bare(&name) {
            image
                .new_descriptors
                .insert(name.clone(), snap.stored_row("ConfigSave", &name).unwrap());
        }
    }
    inputs.staged = image;
    inputs.objects = objects_of(snap, &inputs.root_row);
    inputs
}

/// `Inputs::objects`: the stored descriptors the reader would read.
fn objects_of(snap: &Snap, root_row: &[u8]) -> std::collections::BTreeMap<String, Vec<u8>> {
    let configuration = crate::restructure::plan::configuration_uuid(root_row).unwrap();
    let row = snap.stored_row("Config", &configuration).unwrap();
    context_uuids(&row)
        .unwrap()
        .into_iter()
        .map(|uuid| {
            let stored = snap.stored_row("Config", &uuid).unwrap();
            (uuid, stored)
        })
        .collect()
}

fn options() -> PlanOptions {
    PlanOptions {
        names_version: Some(VERSION.to_owned()),
        ..PlanOptions::default()
    }
}

/// The native `DBNames` without the entries the platform adds for its own new system tables.
fn without_upgrade(names: &DbNames) -> DbNames {
    let mut names = names.clone();
    names.entries.retain(|entry| {
        ![
            "DbCopiesInfoBaseUse",
            "DbCopiesUpdateTableStat",
            "DbCopiesUpdateStat",
        ]
        .contains(&entry.kind.as_str())
    });
    names.max = names
        .entries
        .iter()
        .map(|entry| entry.number)
        .max()
        .unwrap();
    names
}

fn assert_names_equal(plan: &Plan, after: &Snap) {
    let native = DbNames::parse(&after.params("DBNames").unwrap()).unwrap();
    let native = without_upgrade(&native);
    let ours = DbNames::parse(&plan.new_names_text).unwrap();
    assert_eq!(ours.max, native.max, "the header");
    let only = |a: &DbNames, b: &DbNames| -> Vec<String> {
        a.entries
            .iter()
            .filter(|entry| !b.entries.contains(entry))
            .map(|entry| format!("{} {} {}", entry.number, entry.kind, entry.uuid))
            .collect()
    };
    let (extra, missing) = (only(&ours, &native), only(&native, &ours));
    assert!(
        extra.is_empty() && missing.is_empty(),
        "DBNames entries only in ours: {extra:?}; only in the platform's: {missing:?}"
    );
    assert_eq!(ours.entries, native.entries, "the order of the entries");
}

/// Every table of ours is the platform's; the platform also has its own new system tables and upgraded two.
fn assert_schema_equal(plan: &Plan, after: &Snap) {
    let ours = DbSchema::parse(&plan.new_schema).unwrap();
    let native = DbSchema::parse(&after.schema().unwrap()).unwrap();
    let upgraded = [
        "DbCopies",
        "DbCopiesUpdates",
        "DbCopiesInfoBaseUse",
        "DbCopiesUpdateStat",
        "DbCopiesUpdateTableStat",
        "WebSocketClients",
        // the platform's own rebuild of a system table in case c (it moves to the tail)
        "BPrPoints10",
    ];
    for position in 0..ours.len() {
        let view = ours.view(position).unwrap();
        let name = view.name().to_owned();
        let Some(other) = native.position(&name) else {
            panic!("the platform has no table {name}");
        };
        if upgraded.contains(&name.as_str()) {
            continue;
        }
        assert!(
            serialize(&ours.tables()[position]) == serialize(&native.tables()[other]),
            "the entry of {name} differs from the platform's"
        );
    }
    // the platform's order of the tables that are not its own: the same as ours
    let order = |schema: &DbSchema| -> Vec<String> {
        (0..schema.len())
            .map(|i| schema.view(i).unwrap().name().to_owned())
            .filter(|name| !upgraded.contains(&name.as_str()))
            .collect()
    };
    let (mine, theirs) = (order(&ours), order(&native));
    if mine != theirs {
        let at = mine
            .iter()
            .zip(&theirs)
            .position(|(a, b)| a != b)
            .unwrap_or(mine.len().min(theirs.len()));
        let show = |names: &[String]| -> Vec<String> {
            names[at.saturating_sub(2)..(at + 4).min(names.len())].to_vec()
        };
        panic!(
            "the order of the tables differs at {at} of {}: ours {:?}, the platform's {:?}; the tails: ours {:?}, the platform's {:?}",
            mine.len(),
            show(&mine),
            show(&theirs),
            &mine[mine.len().saturating_sub(6)..],
            &theirs[theirs.len().saturating_sub(6)..]
        );
    }
}

/// The cache rows: text equal to native's after inflate; `c4629235` by its entries.
fn assert_caches_equal(plan: &Plan, before: &Snap, after: &Snap, rows: &[&str]) {
    for short in rows {
        let name = row_name(short);
        let update = plan
            .caches
            .iter()
            .find(|cache| cache.row_name == name)
            .unwrap_or_else(|| panic!("the plan does not update {short}"));
        let ours = inflate(&update.row).unwrap();
        let native = after.params(name).unwrap();
        if *short == "c4629235" {
            let a = crate::restructure::caches::help_props::HelpProps::parse(&ours).unwrap();
            let b = crate::restructure::caches::help_props::HelpProps::parse(&native).unwrap();
            let key = |help: &crate::restructure::caches::help_props::HelpProps| {
                help.entries
                    .iter()
                    .map(|entry| (entry.key.clone(), entry.clone()))
                    .collect::<std::collections::BTreeMap<_, _>>()
            };
            assert!(key(&a) == key(&b), "c4629235: the entries differ");
        } else {
            assert!(ours == native, "{short} differs from the platform's");
        }
    }
    // a row the plan does not write is one the platform did not change
    for update in &plan.caches {
        if update.row_name == "siVersions" {
            continue;
        }
        assert!(
            rows.iter().any(|short| row_name(short) == update.row_name),
            "the plan writes {}, which is not expected",
            update.row_name
        );
    }
    let _ = before;
}

#[test]
fn the_plan_of_a_new_document_equals_the_native_result_of_case_d() {
    let (before, after) = (lab_snap!("d_staged"), lab_snap!("d_after"));
    let inputs = inputs_of(&before);
    assert_eq!(new_descriptor_names(&inputs.staged).len(), 1);
    let plan = plan(&inputs, &options()).unwrap();
    assert_eq!(plan.objects.len(), 1);
    let object = &plan.objects[0];
    assert!(object.created);
    assert_eq!(object.object, "Document11034");
    let tables: Vec<&str> = plan.tables().map(|t| t.table.name.as_str()).collect();
    assert_eq!(tables, ["_Document11034", "_Document11034_VT11036"]);
    assert!(plan.tables().all(|table| table.create));

    assert_names_equal(&plan, &after);
    assert_schema_equal(&plan, &after);
    assert_caches_equal(
        &plan,
        &before,
        &after,
        &[
            "1a621f0f", "2203278d", "a07b62f0", "c4629235", "ea13a2c9", "facbfffe", "fe8acd6a",
        ],
    );

    // the statements: create, index, rename; nothing copied, nothing dropped
    let statements = plan.statements();
    let sql: Vec<&str> = statements.iter().map(|s| s.sql.as_str()).collect();
    assert!(
        sql.iter()
            .any(|s| s.contains("create table dbo._Document11034NG"))
    );
    assert!(
        sql.iter()
            .any(|s| s.contains("create table dbo._Document11034_VT11036NG"))
    );
    assert!(
        !sql.iter()
            .any(|s| s.contains("INSERT INTO dbo._Document11034"))
    );
    assert!(
        !sql.iter()
            .any(|s| s.contains("drop table dbo._Document11034"))
    );
    plan.phase_sql("@now").unwrap();
    let _ = (
        Brace::num(0),
        ROOT_ROW,
        deflate(b"").unwrap(),
        BTreeSet::<String>::new(),
    );
}

/// A stage made of the rows the platform stored after a case: what differs from `before`'s `Config`.
fn stage_of(before: &Snap, after: &Snap) -> Inputs {
    let mut inputs = inputs_of(before);
    let old_files: BTreeSet<String> = before.names("Config").into_iter().collect();
    let mut image = StagedImage {
        old_files: old_files.clone(),
        old_descriptors: inputs.staged.old_descriptors.clone(),
        ..StagedImage::default()
    };
    for name in after.names("Config") {
        let now = after.stored_row("Config", &name).unwrap();
        let unchanged = before.stored_row("Config", &name).as_ref() == Some(&now);
        if unchanged {
            continue;
        }
        image.new_files.insert(name.clone());
        if bare(&name) {
            image.new_descriptors.insert(name.clone(), now);
        }
    }
    inputs.staged = image;
    inputs
}

#[test]
fn the_plan_of_a_new_catalog_and_two_attributes_equals_the_native_result_of_case_c() {
    let (before, after) = (lab_snap!("pristine"), lab_snap!("c2"));
    let inputs = stage_of(&before, &after);
    assert_eq!(new_descriptor_names(&inputs.staged).len(), 1);
    let plan = plan(&inputs, &options()).unwrap();
    // the two rebuilt objects first (a catalog, a document), then the created catalog
    let objects: Vec<(&str, bool)> = plan
        .objects
        .iter()
        .map(|object| (object.object.as_str(), object.created))
        .collect();
    assert_eq!(
        objects,
        [
            ("Reference20", false),
            ("Document39", false),
            ("Reference11036", true)
        ]
    );
    assert_names_equal(&plan, &after);
    assert_schema_equal(&plan, &after);
    assert_caches_equal(
        &plan,
        &before,
        &after,
        &[
            "1a621f0f", "2203278d", "42ed49cc", "a07b62f0", "c4629235", "ea13a2c9", "facbfffe",
            "fe8acd6a",
        ],
    );
    let _ = ROOT_ROW;
}
