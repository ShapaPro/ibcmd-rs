//! The cache rows against the native ones for the cases of the `ddl` track: c (a new catalog),
//! h (a new tabular section) and the types case (attributes only). See `tests_corpus.rs` for the
//! snapshots; every test skips itself without the lab.

use std::collections::{BTreeMap, BTreeSet};

use crate::metadata_model::brace::{Brace, parse_row, serialize_row};
use crate::restructure::caches::facts::{ObjectFacts, TABULAR_SECTIONS};
use crate::restructure::caches::help_props::HelpProps;
use crate::restructure::caches::names_tables::{NamesTables, NewTable};
use crate::restructure::caches::order::iteration_order;
use crate::restructure::caches::owner_map::OwnerMap;
use crate::restructure::caches::plan::{
    CacheRow, NewCatalog, NewSection, catalog_shape, new_catalog, new_tabular_section,
    reference_type_slots, rows, section_traversal,
};
use crate::restructure::caches::root::{CATALOG_CLASS, collection_of, collections, kind_of_class};
use crate::restructure::caches::synonyms::{PROPERTY_SECTIONS, Synonyms, insertion_order};
use crate::restructure::caches::tests_corpus::{ROOT_ROW, Snap, lab_snap, row_name};
use crate::restructure::caches::type_index::TypeIndex;
use crate::restructure::caches::type_sets::{FAMILIES, TypeSets, member_id};
use crate::restructure::caches::xdto_types::{XdtoModel, primitive_property, standard_properties};

fn root_of(snap: &Snap) -> Brace {
    parse_row(&snap.config(ROOT_ROW).unwrap()).unwrap()
}

// ---------------------------------------------------------------------------------------------
// every row through its decoder
// ---------------------------------------------------------------------------------------------

#[test]
fn every_cache_row_round_trips_through_its_decoder() {
    for name in ["pristine", "c2", "m", "t1_before"] {
        let snap = lab_snap!(name);
        let text = |short: &str| snap.params(row_name(short)).unwrap();
        assert!(
            NamesTables::parse(&text("a07b62f0")).unwrap().render() == text("a07b62f0"),
            "{name} a07b62f0"
        );
        assert!(
            OwnerMap::parse(&text("42ed49cc")).unwrap().render() == text("42ed49cc"),
            "{name} 42ed49cc"
        );
        assert!(
            Synonyms::parse(&text("facbfffe")).unwrap().render() == text("facbfffe"),
            "{name} facbfffe"
        );
        assert!(
            TypeSets::parse(&text("fe8acd6a")).unwrap().render() == text("fe8acd6a"),
            "{name} fe8acd6a"
        );
        assert!(
            HelpProps::parse(&text("c4629235")).unwrap().render() == text("c4629235"),
            "{name} c4629235"
        );
        assert!(
            XdtoModel::parse(&text("ea13a2c9")).unwrap().render() == text("ea13a2c9"),
            "{name} ea13a2c9"
        );
        // the rest are plain brace text in the platform's layout
        for short in [
            "0b698dcd", "1a621f0f", "215d232c", "59274b8d", "c40aafd6", "c77bc206", "cf8b5e0f",
            "e05c0074", "fd1b2a86",
        ] {
            let bytes = text(short);
            assert!(
                serialize_row(&parse_row(&bytes).unwrap()) == bytes,
                "{name} {short}"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// the type index
// ---------------------------------------------------------------------------------------------

fn traversal(snap: &Snap, root: &Brace) -> Vec<String> {
    let collections = collections(root);
    section_traversal(&collections, CATALOG_CLASS, &|uuid| snap.descriptor(uuid)).unwrap()
}

#[test]
fn the_tabular_section_section_is_the_hash_order_of_the_traversal() {
    for name in ["pristine", "m"] {
        let snap = lab_snap!(name);
        let root = root_of(&snap);
        let index = TypeIndex::parse(&snap.params(row_name("2203278d")).unwrap()).unwrap();
        let section = index.section(TABULAR_SECTIONS).unwrap();
        let dump: Vec<&str> = section.entries.iter().map(|e| e.object.as_str()).collect();
        let order = traversal(&snap, &root);
        assert_eq!(order.len(), dump.len(), "{name}");
        let expected = iteration_order(order.iter().map(String::as_str)).unwrap();
        assert_eq!(dump, expected, "{name}");
    }
}

#[test]
fn the_generated_types_of_an_entry_are_the_ones_of_its_descriptor() {
    let snap = lab_snap!("c2");
    let index = TypeIndex::parse(&snap.params(row_name("2203278d")).unwrap()).unwrap();
    let mut checked = 0;
    for section in &index.sections {
        let Some(kind) = kind_of_class(&section.class) else {
            continue;
        };
        if !["Catalog", "Document", "ExchangePlan"].contains(&kind) {
            continue;
        }
        for entry in &section.entries {
            let facts = ObjectFacts::parse(kind, &snap.descriptor(&entry.object).unwrap()).unwrap();
            assert_eq!(
                reference_type_slots(&facts).unwrap(),
                entry.types,
                "{kind} {}",
                facts.name
            );
            checked += 1;
        }
    }
    assert!(checked > 140, "{checked}");
    // the tabular sections: their own type and the type of their row
    let sections = index.section(TABULAR_SECTIONS).unwrap();
    let mut from_descriptors = BTreeMap::new();
    for object in &collection_of(&collections(&root_of(&snap)), CATALOG_CLASS)
        .unwrap()
        .objects
    {
        let facts = ObjectFacts::parse("Catalog", &snap.descriptor(object).unwrap()).unwrap();
        for section in facts.sections {
            from_descriptors.insert(section.uuid.clone(), section.types.clone());
        }
    }
    assert_eq!(from_descriptors.len(), sections.entries.len());
    for entry in &sections.entries {
        assert_eq!(
            entry.types.iter().map(|t| t.index).collect::<Vec<_>>(),
            [0, 1]
        );
        let [(t0, v0), (t1, v1)] = &from_descriptors[&entry.object];
        assert_eq!(
            (
                &entry.types[0].type_id,
                &entry.types[0].value_id,
                &entry.types[1].type_id,
                &entry.types[1].value_id
            ),
            (t0, v0, t1, v1)
        );
    }
}

#[test]
fn a_catalog_removed_from_every_row_comes_back_where_the_platform_had_it() {
    let snap = lab_snap!("pristine");
    let root = root_of(&snap);
    let collections = collections(&root);
    let catalogs = collection_of(&collections, CATALOG_CLASS).unwrap().clone();

    let index = TypeIndex::parse(&snap.params(row_name("2203278d")).unwrap()).unwrap();
    let names = NamesTables::parse(&snap.params(row_name("a07b62f0")).unwrap()).unwrap();
    let synonyms = Synonyms::parse(&snap.params(row_name("facbfffe")).unwrap()).unwrap();
    let mut readded = 0;
    for (position, object) in catalogs.objects.iter().enumerate() {
        let facts = ObjectFacts::parse("Catalog", &snap.descriptor(object).unwrap()).unwrap();

        // 2203278d
        let mut again = index.clone();
        let section = again.section_mut(CATALOG_CLASS).unwrap();
        let at = section
            .entries
            .iter()
            .position(|e| e.object == *object)
            .unwrap();
        let removed = section.entries.remove(at);
        again
            .refill_section(CATALOG_CLASS, &catalogs.objects, vec![removed])
            .unwrap();
        assert_eq!(again, index, "2203278d, {}", facts.name);

        // a07b62f0
        let mut again = names.clone();
        let at = again
            .entries
            .iter()
            .position(|e| e.object == *object)
            .unwrap();
        let removed = again.entries.remove(at);
        let table = removed.table.clone().unwrap();
        let predecessor = position
            .checked_sub(1)
            .map(|i| catalogs.objects[i].as_str());
        let successor = catalogs.objects.get(position + 1).map(String::as_str);
        again
            .add_object(&NewTable {
                kind: "Catalog",
                name: &facts.name,
                object,
                table_number: table.number,
                ref_type_id: &table.type_id,
                predecessor,
                successor,
            })
            .unwrap();
        assert_eq!(again, names, "a07b62f0, {}", facts.name);

        // facbfffe: every property the catalog has
        for property in [
            "ListPresentation",
            "ExtendedListPresentation",
            "ObjectPresentation",
            "ExtendedObjectPresentation",
        ] {
            let pairs = facts.localized(property).unwrap();
            if pairs.is_empty() {
                continue;
            }
            let mut again = synonyms.clone();
            let &(_, section_index) = PROPERTY_SECTIONS
                .iter()
                .find(|(name, _)| *name == property)
                .unwrap();
            let section = &mut again.sections[section_index];
            let at = section
                .entries
                .iter()
                .position(|(key, _)| key == object)
                .unwrap();
            section.entries.remove(at);
            again
                .add_object(property, object, &pairs, &collections, &kind_of_class)
                .unwrap();
            assert_eq!(again, synonyms, "facbfffe {property}, {}", facts.name);
            readded += 1;
        }
    }
    assert!(readded > 100, "{readded}");
}

#[test]
fn the_sections_of_facbfffe_are_filled_kind_by_kind_in_the_roots_order() {
    let mut exact = Vec::new();
    for name in ["pristine", "c2"] {
        let snap = lab_snap!(name);
        let root = root_of(&snap);
        let collections = collections(&root);
        let synonyms = Synonyms::parse(&snap.params(row_name("facbfffe")).unwrap()).unwrap();
        for (index, section) in synonyms.sections.iter().enumerate() {
            let keys: BTreeSet<String> = section.entries.iter().map(|(k, _)| k.clone()).collect();
            let Ok(insertion) = insertion_order(&keys, &collections, &kind_of_class) else {
                continue; // keys the root does not list (sections 0 and 7)
            };
            let order = iteration_order(insertion.iter().copied()).unwrap();
            let dump: Vec<&str> = section.entries.iter().map(|(k, _)| k.as_str()).collect();
            if order == dump {
                exact.push((name, index));
            } else {
                eprintln!(
                    "facbfffe {name} section {index} ({} entries) is not reproduced",
                    dump.len()
                );
            }
        }
    }
    eprintln!("facbfffe exact: {exact:?}");
    for section in [2, 3, 4, 5, 6] {
        assert!(exact.contains(&("pristine", section)), "section {section}");
        assert!(exact.contains(&("c2", section)), "section {section}");
    }
}

#[test]
fn the_families_of_the_type_sets_are_the_sets_of_the_corpus() {
    for name in ["pristine", "c2"] {
        let snap = lab_snap!(name);
        let index = TypeIndex::parse(&snap.params(row_name("2203278d")).unwrap()).unwrap();
        let sets = TypeSets::parse(&snap.params(row_name("fe8acd6a")).unwrap()).unwrap();
        for (set, families) in FAMILIES {
            let mut expected = BTreeSet::new();
            for section in &index.sections {
                let Some(kind) = kind_of_class(&section.class) else {
                    continue;
                };
                for (family_kind, family_index) in *families {
                    if *family_kind != kind {
                        continue;
                    }
                    for entry in &section.entries {
                        for slot in &entry.types {
                            if slot.index == *family_index {
                                expected.insert(slot.type_id.clone());
                            }
                        }
                    }
                }
            }
            let members = &sets.sets.iter().find(|(key, _)| key == set).unwrap().1;
            let ids: Vec<String> = members
                .iter()
                .map(|member| member_id(member).unwrap().to_owned())
                .collect();
            assert_eq!(
                ids.iter().cloned().collect::<BTreeSet<_>>(),
                expected,
                "{name}: set {set}"
            );
            assert!(
                ids.windows(2).all(|pair| pair[0] < pair[1]),
                "{name}: set {set} is sorted"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// c4629235 and the XDTO standard properties, all catalogs
// ---------------------------------------------------------------------------------------------

#[test]
fn a_catalogs_entry_of_c4629235_is_made_of_its_descriptor() {
    let snap = lab_snap!("c2");
    let root = root_of(&snap);
    let help = HelpProps::parse(&snap.params(row_name("c4629235")).unwrap()).unwrap();
    let by_key: BTreeMap<&str, _> = help
        .entries
        .iter()
        .map(|entry| (entry.key.as_str(), entry))
        .collect();
    let catalogs = collection_of(&collections(&root), CATALOG_CLASS)
        .unwrap()
        .clone();
    for object in &catalogs.objects {
        let facts = ObjectFacts::parse("Catalog", &snap.descriptor(object).unwrap()).unwrap();
        let has_help = snap.has_config(&format!("{object}.1"));
        let expected = HelpProps::catalog_entry(&facts, has_help).unwrap();
        let native = by_key[object.as_str()];
        // property 24 lists the type sets that mention the object: not made of the descriptor
        let without_24: Vec<_> = native
            .props
            .iter()
            .filter(|(id, _)| id != "24")
            .cloned()
            .collect();
        assert_eq!(expected.props, without_24, "{}", facts.name);
    }
}

#[test]
fn the_standard_xdto_properties_follow_the_shape_of_the_catalog() {
    let snap = lab_snap!("pristine");
    let root = root_of(&snap);
    let model = XdtoModel::parse(&snap.params(row_name("ea13a2c9")).unwrap()).unwrap();
    let catalogs = collection_of(&collections(&root), CATALOG_CLASS)
        .unwrap()
        .clone();
    for object in &catalogs.objects {
        let facts = ObjectFacts::parse("Catalog", &snap.descriptor(object).unwrap()).unwrap();
        let open = format!("\t\t<objectType name=\"CatalogObject.{}\">\r\n", facts.name);
        let at = model.xml.find(&open).unwrap() + open.len();
        let expected: String =
            standard_properties(&facts.name, catalog_shape(&facts).unwrap()).concat();
        assert!(model.xml[at..].starts_with(&expected), "{}", facts.name);
    }
}

// ---------------------------------------------------------------------------------------------
// case c: a new catalog; case h: a new tabular section; the types case
// ---------------------------------------------------------------------------------------------

/// The lines of `derived` that are not in `base` (which must be a subsequence of it), each with the
/// index of the base line before it.
fn inserted_lines(base: &str, derived: &str) -> Vec<(usize, String)> {
    let base_lines: Vec<&str> = base.split("\r\n").collect();
    let mut out = Vec::new();
    let mut at = 0;
    for line in derived.split("\r\n") {
        if at < base_lines.len() && base_lines[at] == line {
            at += 1;
        } else {
            out.push((at, line.to_owned()));
        }
    }
    assert_eq!(
        at,
        base_lines.len(),
        "the base is not a subsequence of the derived text"
    );
    out
}

fn by_name(rows: Vec<CacheRow>) -> BTreeMap<&'static str, CacheRow> {
    rows.into_iter().map(|row| (row.name, row)).collect()
}

const NEW_CATALOG: &str = "5ff28850-03db-4a0f-b95e-c2ea4d8c516d";

#[test]
fn a_new_catalog_gets_the_rows_native_wrote_in_case_c() {
    let before = lab_snap!("pristine");
    let after = lab_snap!("c2");
    let root = root_of(&after);
    let descriptor = after.descriptor(NEW_CATALOG).unwrap();
    let cache = |name: &str| before.params(name);
    let name_of = |uuid: &str| {
        let row = before.descriptor(uuid)?;
        ObjectFacts::parse("Catalog", &row)
            .ok()
            .map(|facts| facts.name)
    };
    let rows_out = by_name(
        new_catalog(&NewCatalog {
            descriptor: &descriptor,
            root: &root,
            table_number: 11036,
            has_help: after.has_config(&format!("{NEW_CATALOG}.1")),
            xdto_attribute_lines: &[],
            xdto_section_blocks: &[],
            cache: &cache,
            name_of: &name_of,
        })
        .unwrap(),
    );
    assert_eq!(rows_out.len(), 7);
    // five rows are byte for byte the native ones
    for (name, short) in [
        (rows::TYPE_INDEX, "2203278d"),
        (rows::NAMES_TABLES, "a07b62f0"),
        (rows::OWNER_MAP, "42ed49cc"),
        (rows::SYNONYMS, "facbfffe"),
        (rows::TYPE_SETS, "fe8acd6a"),
    ] {
        let native = after.params(row_name(short)).unwrap();
        let ours = &rows_out[name];
        assert!(ours.exact);
        assert!(
            ours.text == native,
            "{short}: the row differs from native's"
        );
    }

    // c4629235: the entry is native's, a few other entries stand elsewhere
    let ours = HelpProps::parse(&rows_out[rows::HELP_PROPS].text).unwrap();
    let native = HelpProps::parse(&after.params(row_name("c4629235")).unwrap()).unwrap();
    assert!(!rows_out[rows::HELP_PROPS].exact);
    assert_eq!(ours.entries.len(), native.entries.len());
    let of = |help: &HelpProps, key: &str| help.entries.iter().find(|e| e.key == key).cloned();
    assert_eq!(of(&ours, NEW_CATALOG), of(&native, NEW_CATALOG));
    // the other entries: how many neighbours of native's row are not neighbours in ours, once the new
    // key is taken out of both (one entry put elsewhere breaks up to three links)
    let without = |help: &HelpProps| -> Vec<String> {
        help.entries
            .iter()
            .filter(|e| e.key != NEW_CATALOG)
            .map(|e| e.key.clone())
            .collect()
    };
    let (ours_keys, native_keys) = (without(&ours), without(&native));
    let links: BTreeSet<(&str, &str)> = ours_keys
        .windows(2)
        .map(|pair| (pair[0].as_str(), pair[1].as_str()))
        .collect();
    let broken = native_keys
        .windows(2)
        .filter(|pair| !links.contains(&(pair[0].as_str(), pair[1].as_str())))
        .count();
    eprintln!(
        "c4629235: {broken} of {} neighbour links of native's row are broken in ours",
        native_keys.len() - 1
    );
    assert!(broken < 20, "{broken}");

    // the XDTO model: everything native inserted for the catalog, in native's place; native's text
    // has more inserted lines (the attributes of the case's other edits), which are not ours
    let ours = XdtoModel::parse(&rows_out[rows::XDTO].text).unwrap();
    let base = XdtoModel::parse(&before.params(row_name("ea13a2c9")).unwrap()).unwrap();
    let theirs = XdtoModel::parse(&after.params(row_name("ea13a2c9")).unwrap()).unwrap();
    let ours_inserted = inserted_lines(&base.xml, &ours.xml);
    let native_inserted = inserted_lines(&base.xml, &theirs.xml);
    assert_eq!(ours_inserted.len(), 8, "{ours_inserted:?}");
    for line in &ours_inserted {
        assert!(
            native_inserted.contains(line),
            "native did not insert {line:?}"
        );
    }
}

#[test]
fn a_new_tabular_section_gets_the_rows_native_wrote_in_case_h() {
    let before = lab_snap!("pristine");
    let after = lab_snap!("m");
    let root = root_of(&after);
    let cache = |name: &str| before.params(name);
    let descriptor = |uuid: &str| after.descriptor(uuid);
    let owner = "28e59c50-3315-4759-98cc-24ace88cc4cf";
    let section = "465a7d10-1055-46dd-9222-ea045b572c74";
    let lines = vec![
        primitive_property("КодЯзыка", "xs:string", false),
        primitive_property("НаименованиеПолное", "xs:string", false),
    ];
    let rows_out = by_name(
        new_tabular_section(&NewSection {
            root: &root,
            descriptor: &descriptor,
            catalog: owner,
            section,
            xdto_row_lines: &lines,
            cache: &cache,
        })
        .unwrap(),
    );
    let native = after.params(row_name("2203278d")).unwrap();
    assert!(
        rows_out[rows::TYPE_INDEX].text == native,
        "2203278d differs from native's"
    );

    let ours = XdtoModel::parse(&rows_out[rows::XDTO].text).unwrap();
    let base = XdtoModel::parse(&before.params(row_name("ea13a2c9")).unwrap()).unwrap();
    let theirs = XdtoModel::parse(&after.params(row_name("ea13a2c9")).unwrap()).unwrap();
    let ours_inserted = inserted_lines(&base.xml, &ours.xml);
    let native_inserted = inserted_lines(&base.xml, &theirs.xml);
    assert_eq!(ours_inserted.len(), 5, "{ours_inserted:?}");
    for line in &ours_inserted {
        assert!(
            native_inserted.contains(line),
            "native did not insert {line:?}"
        );
    }
}

#[test]
fn the_types_case_changes_none_of_these_rows() {
    let before = lab_snap!("t1_before");
    let after = lab_snap!("t1_nat");
    for short in [
        "2203278d", "a07b62f0", "42ed49cc", "facbfffe", "fe8acd6a", "c4629235", "c77bc206",
        "0b698dcd", "215d232c", "59274b8d", "c40aafd6", "cf8b5e0f", "e05c0074", "fd1b2a86",
    ] {
        assert!(
            before.params(row_name(short)).unwrap() == after.params(row_name(short)).unwrap(),
            "{short} changed in the types case"
        );
    }
}
