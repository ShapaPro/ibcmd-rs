//! The S1 gate: what the gate lets through of the classification of the restructuring check
//! (`apply_check::s1::classify`), and what it refuses (the fixtures of case a2 stand in for the database).

use super::tests_plan::{
    CATALOG, NEW_ROW, OLD_ROW, client_as_string, inputs, options, string_client_inputs,
    widen_client,
};
use crate::apply_check::{ChangeOp, Reason, ReasonClass, RuleId, Seg, Verdict};
use crate::mssql_config_apply::gate::{GateBlocker, GateVerdict};
use crate::restructure::s1::decide;

fn seg(name: &str, label: Option<&str>) -> Seg {
    Seg {
        name: name.to_owned(),
        label: label.map(str::to_owned),
    }
}

/// A typed reason of class `structure` on the descriptor of the catalog of case a2.
fn structure(
    rule: RuleId,
    kind: &str,
    object: &str,
    path: Vec<Seg>,
    op: Option<ChangeOp>,
) -> Reason {
    Reason {
        class: ReasonClass::Structure,
        object: object.to_owned(),
        file_name: CATALOG.to_owned(),
        property: path
            .iter()
            .map(|seg| seg.name.clone())
            .collect::<Vec<_>>()
            .join("/"),
        change: "x".to_owned(),
        rule,
        kind: kind.to_owned(),
        path,
        op,
    }
}

fn check_of(reasons: Vec<Reason>) -> Verdict {
    let mut verdict = Verdict::new("rows");
    for reason in reasons {
        verdict.push_reason(reason);
    }
    verdict
}

const OBJECT: &str = "Catalog._ДемоПартнеры";

fn attribute_path(name: &str) -> Vec<Seg> {
    vec![seg("ChildObjects", None), seg("Attribute", Some(name))]
}

fn attribute(name: &str, op: ChangeOp) -> Reason {
    structure(
        RuleId::ColumnAddedOrDropped,
        "Catalog",
        OBJECT,
        attribute_path(name),
        Some(op),
    )
}

fn length(name: &str, from: &str, to: &str) -> Reason {
    let mut path = attribute_path(name);
    path.extend([
        seg("Properties", None),
        seg("Type", None),
        seg("StringQualifiers", None),
        seg("Length", None),
    ]);
    structure(
        RuleId::AttributePropertyNotCovered,
        "Catalog",
        OBJECT,
        path,
        Some(ChangeOp::Modified {
            old: from.to_owned(),
            new: to.to_owned(),
        }),
    )
}

fn indexing(name: &str) -> Reason {
    let mut path = attribute_path(name);
    path.extend([seg("Properties", None), seg("Indexing", None)]);
    structure(
        RuleId::AttributePropertyNotCovered,
        "Catalog",
        OBJECT,
        path,
        Some(ChangeOp::Modified {
            old: "DontIndex".to_owned(),
            new: "Index".to_owned(),
        }),
    )
}

/// The check's verdict for case a2: one new attribute.
fn check_of_a2() -> Verdict {
    check_of(vec![attribute("ДемоНовыйРеквизит", ChangeOp::Added)])
}

/// The conservative gate's verdict: the descriptors that differ are blockers.
fn conservative(rows: &[&str]) -> GateVerdict {
    GateVerdict {
        restructuring_required: !rows.is_empty(),
        blockers: rows
            .iter()
            .map(|row| GateBlocker {
                row: (*row).to_owned(),
                reason: "the descriptor's text differs from the active one: a metadata change, possibly structural".to_owned(),
            })
            .collect(),
        gate: "conservative".to_owned(),
        ..GateVerdict::default()
    }
}

fn blocked_with(verdict: &GateVerdict, text: &str) -> bool {
    verdict
        .blockers
        .iter()
        .any(|blocker| blocker.reason.starts_with("S1: ") && blocker.reason.contains(text))
}

#[test]
fn a_planned_attribute_is_let_through_and_its_blocker_withdrawn() {
    let (verdict, phase) = decide(
        conservative(&[CATALOG]),
        &check_of_a2(),
        &inputs(OLD_ROW, NEW_ROW),
        &options(),
    );
    assert!(!verdict.restructuring_required, "{:?}", verdict.blockers);
    assert!(verdict.blockers.is_empty());
    assert!(verdict.gate.starts_with("s1"));
    let phase = phase.expect("a structure phase");
    assert_eq!(
        phase.tables,
        ["_Reference20", "_Reference20_VT155", "_Reference20_VT159"]
    );
    assert_eq!(phase.objects.len(), 1);
    assert!(phase.objects[0].contains("Fld11034 = ДемоНовыйРеквизит"));
    assert!(phase.sql.contains("create table dbo._Reference20NG"));
    assert!(phase.sql.contains("Modified = @now"));
    // The fixtures hold no cache rows and the options leave the caches alone.
    assert!(phase.params_rewrites.is_empty());
    // The fixture stage has the `deleted` row of a native import (an empty list): the phase answers for it.
    assert_eq!(phase.consumed_staged_rows, 1);
}

#[test]
fn a_widened_string_is_let_through_and_narrowing_is_not() {
    let widened = widen_client(
        &client_as_string(OLD_ROW, 50),
        "{\"S\",50,1}",
        "{\"S\",200,1}",
    );
    let staged = string_client_inputs(&widened);
    let check = check_of(vec![length("Клиент", "50", "200")]);
    let (verdict, phase) = decide(conservative(&[CATALOG]), &check, &staged, &options());
    assert!(!verdict.restructuring_required, "{:?}", verdict.blockers);
    let phase = phase.expect("a structure phase");
    assert!(phase.objects[0].contains("widened attributes"));
    assert!(phase.objects[0].contains("(50 -> 200)"));
    assert!(phase.params_rewrites.is_empty() && phase.caches.is_empty());
    assert!(phase.sql.contains("create table dbo._Reference20NG"));

    // The check names another attribute than the plan widens: the decoders disagree.
    let other = check_of(vec![length("Другой", "50", "200")]);
    let (verdict, phase) = decide(conservative(&[CATALOG]), &other, &staged, &options());
    assert!(verdict.restructuring_required && phase.is_none());
    assert!(blocked_with(&verdict, "disagree"), "{:?}", verdict.blockers);

    // The check itself refuses a limit that shrank (a typed refusal of the classification) ...
    let shrank = check_of(vec![length("Клиент", "50", "20")]);
    let (verdict, phase) = decide(conservative(&[CATALOG]), &shrank, &staged, &options());
    assert!(verdict.restructuring_required && phase.is_none());
    assert!(
        blocked_with(&verdict, "length-not-widened"),
        "{:?}",
        verdict.blockers
    );
    // ... and when it names it as a widening the plan refuses it.
    let narrowed = widen_client(
        &client_as_string(OLD_ROW, 50),
        "{\"S\",50,1}",
        "{\"S\",20,1}",
    );
    let (verdict, phase) = decide(
        conservative(&[CATALOG]),
        &check,
        &string_client_inputs(&narrowed),
        &options(),
    );
    assert!(verdict.restructuring_required && phase.is_none());
    assert!(
        blocked_with(&verdict, "the limit is shorter"),
        "{:?}",
        verdict.blockers
    );
}

#[test]
fn a_deleted_attribute_is_let_through_with_the_deleted_row_the_phase_answers_for() {
    // The reverse of case a2: the images swapped is not a removal the fixtures can hold (DBNames has no
    // number for the attribute), so the removal is exercised on the state after the addition.
    let added = crate::restructure::plan::plan(&inputs(OLD_ROW, NEW_ROW), &options()).unwrap();
    let mut back = inputs(NEW_ROW, OLD_ROW);
    back.schema = added.new_schema.clone();
    back.main_names = added.new_names_row.clone();
    back.staged.deleted = Some(
        crate::restructure::names::deflate(
            "\u{feff}1,\"c60cdc87-198a-4f6e-8f17-76bcb1b1914b\",1".as_bytes(),
        )
        .unwrap(),
    );
    let check = check_of(vec![attribute("ДемоНовыйРеквизит", ChangeOp::Removed)]);
    let (verdict, phase) = decide(conservative(&[CATALOG]), &check, &back, &options());
    assert!(!verdict.restructuring_required, "{:?}", verdict.blockers);
    let phase = phase.expect("a structure phase");
    assert!(phase.objects[0].contains("removed attributes Fld11034"));
    assert_eq!(phase.consumed_staged_rows, 1);

    // The check calls the `deleted` list unknown: the gate lets that one reason through, no other.
    let mut with_deleted = check.clone();
    with_deleted.push_reason(Reason {
        class: ReasonClass::Unknown,
        object: "deleted".to_owned(),
        file_name: "deleted".to_owned(),
        rule: RuleId::DeletedRowNotEmpty,
        ..Reason::default()
    });
    let (verdict, phase) = decide(conservative(&[CATALOG]), &with_deleted, &back, &options());
    assert!(!verdict.restructuring_required, "{:?}", verdict.blockers);
    assert!(phase.is_some());

    // A list that names anything but the attributes the stage removes is the plan's refusal.
    let mut wrong = back.clone();
    wrong.staged.deleted = Some(
        crate::restructure::names::deflate(
            "\u{feff}1,\"aaaaaaaa-0000-4000-8000-000000000000\",1".as_bytes(),
        )
        .unwrap(),
    );
    let (verdict, phase) = decide(conservative(&[CATALOG]), &with_deleted, &wrong, &options());
    assert!(verdict.restructuring_required && phase.is_none());
    assert!(
        blocked_with(&verdict, "deletes aaaaaaaa"),
        "{:?}",
        verdict.blockers
    );
}

#[test]
fn nothing_is_let_through_that_the_gate_does_not_cover() {
    let base = inputs(OLD_ROW, NEW_ROW);

    // Nothing to restructure: a plain apply.
    let (verdict, phase) = decide(conservative(&[]), &check_of_a2(), &base, &options());
    assert!(!verdict.restructuring_required && phase.is_none());

    // Another descriptor differs and nothing explains it: the refusal stands, for that row.
    let (verdict, phase) = decide(
        conservative(&[CATALOG, "ffffffff-0000-4000-8000-000000000000"]),
        &check_of_a2(),
        &base,
        &options(),
    );
    assert!(verdict.restructuring_required && phase.is_none());
    assert_eq!(verdict.blockers.len(), 1);
    assert_eq!(
        verdict.blockers[0].row,
        "ffffffff-0000-4000-8000-000000000000"
    );

    // An operation of S1 that is designed but not built (the index flag, a tabular section, an object).
    let check = check_of(vec![
        attribute("ДемоНовыйРеквизит", ChangeOp::Added),
        indexing("Х"),
    ]);
    let (verdict, phase) = decide(conservative(&[CATALOG]), &check, &base, &options());
    assert!(verdict.restructuring_required && phase.is_none());
    assert!(
        blocked_with(&verdict, "designed") && blocked_with(&verdict, "switch-index"),
        "{:?}",
        verdict.blockers
    );
    let section = check_of(vec![structure(
        RuleId::TabularSectionAddedDroppedMoved,
        "Catalog",
        OBJECT,
        vec![
            seg("ChildObjects", None),
            seg("TabularSection", Some("ДемоНоваяТЧ")),
        ],
        Some(ChangeOp::Added),
    )]);
    let (verdict, phase) = decide(conservative(&[CATALOG]), &section, &base, &options());
    assert!(verdict.restructuring_required && phase.is_none());
    assert!(
        blocked_with(&verdict, "add-tabular-section"),
        "{:?}",
        verdict.blockers
    );

    // A reason the classification refuses: a data change, an unknown row, another kind of object.
    for (class, rule, kind, object) in [
        (ReasonClass::Data, RuleId::Unspecified, "Catalog", OBJECT),
        (ReasonClass::Unknown, RuleId::RowUnknown, "", "Enum.X"),
        (
            ReasonClass::Structure,
            RuleId::ColumnAddedOrDropped,
            "AccumulationRegister",
            "AccumulationRegister.X",
        ),
    ] {
        let mut check = check_of_a2();
        check.push_reason(Reason {
            class,
            object: object.to_owned(),
            file_name: CATALOG.to_owned(),
            rule,
            kind: kind.to_owned(),
            path: vec![seg("ChildObjects", None), seg("Resource", Some("Р"))],
            op: Some(ChangeOp::Added),
            ..Reason::default()
        });
        let (verdict, phase) = decide(conservative(&[CATALOG]), &check, &base, &options());
        assert!(
            verdict.restructuring_required && phase.is_none(),
            "{rule:?}"
        );
    }

    // The check and the plan disagree about the attribute.
    let check = check_of(vec![attribute("ДругойРеквизит", ChangeOp::Added)]);
    let (verdict, phase) = decide(conservative(&[CATALOG]), &check, &base, &options());
    assert!(verdict.restructuring_required && phase.is_none());
    assert!(blocked_with(&verdict, "disagree"), "{:?}", verdict.blockers);

    // A conservative verdict with more blockers than it lists.
    let mut crowded = conservative(&[CATALOG]);
    crowded.blockers_omitted = 3;
    let (verdict, phase) = decide(crowded, &check_of_a2(), &base, &options());
    assert!(verdict.restructuring_required && phase.is_none());

    // The plan refuses (the images are the same: nothing is new, removed or retyped).
    let (verdict, phase) = decide(
        conservative(&[CATALOG]),
        &check_of_a2(),
        &inputs(OLD_ROW, OLD_ROW),
        &options(),
    );
    assert!(verdict.restructuring_required && phase.is_none());
    assert!(
        blocked_with(&verdict, "adds no attribute"),
        "{:?}",
        verdict.blockers
    );

    // A check that needs a restructuring and names no reason is refused: nothing is an operation.
    let silent = Verdict {
        needs_restructuring: true,
        ..Verdict::default()
    };
    let (verdict, phase) = decide(conservative(&[CATALOG]), &silent, &base, &options());
    assert!(verdict.restructuring_required && phase.is_none());
}
