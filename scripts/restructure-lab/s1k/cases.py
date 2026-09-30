"""The cases of S1-K (#407): an edit of the БСП 8.3.27 tree, then OUR import and OUR apply against the native ones.

usage:
  python cases.py list
  python cases.py edit <case> <out dir>     writes <out>/stage/<rel> (the edited files), <out>/before/<rel> (the
                                            originals) and <out>/files.txt; the reference tree is never written
  python cases.py files <out dir>           the relative paths of the edited files, one per line

Every editor is the ddl track's (`edit_cases_s1.py`, `edit_cases_s2.py`) or a small one of this kit; the edits are
the ones the S1 operations were traced and twin-verified with, so a difference between our chain and the native one is
the chain's, not the edit's. `REF` is the reference native export of the БСП 8.3.27 corpus clone (12 198 files).
"""
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))

import edit_cases_s1 as s1  # noqa: E402
import edit_cases_s2 as s2  # noqa: E402

REF = s1.REF
EXT_TREES = os.environ.get("S1K_EXT_TREES", r"F:\ibcmd\lab\05\ext\native\8327")
# The objects the four БСП extensions adopt (a fallback for a machine without the reference trees of the ext track):
# an object in this list is refused by S1-I, so the cases of the built operations avoid them.
ADOPTED_FALLBACK = {
    "Catalogs/_ДемоГруппыДоступаПартнеров.xml", "Catalogs/_ДемоКонтрагенты.xml", "Catalogs/_ДемоМестаХранения.xml",
    "Catalogs/_ДемоНоменклатура.xml", "Catalogs/_ДемоОрганизации.xml", "Catalogs/_ДемоПартнеры.xml",
    "Catalogs/ВидыКонтактнойИнформации.xml", "Catalogs/Пользователи.xml",
    "Documents/_ДемоПоступлениеТоваров.xml", "Documents/_ДемоРеализацияТоваров.xml",
    "Documents/_ДемоСписаниеТоваров.xml", "Documents/_ДемоСчетНаОплатуПокупателю.xml",
}


def adopted_files():
    """Relative paths (Catalogs/X.xml) of the objects any extension tree of the ext track adopts."""
    found = set()
    if os.path.isdir(EXT_TREES):
        for extension in os.listdir(EXT_TREES):
            for kind in ("Catalogs", "Documents"):
                folder = os.path.join(EXT_TREES, extension, kind)
                if not os.path.isdir(folder):
                    continue
                for name in os.listdir(folder):
                    if name.endswith(".xml"):
                        path = os.path.join(folder, name)
                        with open(path, encoding="utf-8-sig") as f:
                            if "<ObjectBelonging>Adopted</ObjectBelonging>" in f.read(6000):
                                found.add(kind + "/" + name)
    return found or ADOPTED_FALLBACK


def run_a1(out):
    """A: attributes of every primitive type on the objects no extension adopts (the types case of the ddl track
    without its three adopted catalogs)."""
    adopted = adopted_files()
    tree = s1.Tree(out, "a1")
    files = {}
    for rel, position, attrs in s1.T1:
        if rel in adopted:
            continue
        text, bom = files[rel] if rel in files else tree.read(rel)
        nl = s1.nl_of(text)
        is_document = rel.startswith("Documents/")
        for name, synonym, type_xml, use in attrs:
            block = s1.attribute_block(name, synonym, type_xml, nl, use=None if is_document else use)
            if position == "end":
                text = s1.insert_after_last_attribute(text, block, nl)
            elif position == "first":
                text = s1.add_first_attribute(text, block, nl)
            else:
                text = s1.insert_before_attribute(text, position[1], block, nl)
        files[rel] = (text, bom)
    for rel, (text, bom) in files.items():
        tree.write(rel, text, bom)
        print("edited", rel)
    with open(os.path.join(out, "files.txt"), "w", encoding="utf-8") as f:
        f.write("\n".join(sorted(files)) + "\n")


def run_i1(out):
    """I: a string attribute on two objects the extension `_ДемоРасширение` adopts, one of them with a table of the
    extension's own (`_Reference18X1`): S1-I refuses both."""
    tree = s2.Tree(out)
    s2.append_attribute(tree, "Catalogs/_ДемоПартнеры.xml", "ДемоНовыйРеквизитСИ", "Демо новый реквизит СИ", s1.string_var(50), "ForItem")
    s2.append_attribute(tree, "Catalogs/_ДемоНоменклатура.xml", "ДемоНовыйРеквизитСИ", "Демо новый реквизит СИ", s1.string_var(50), "ForItem")
    tree.save()


def from_s2(name):
    def run(out):
        tree = s2.Tree(out)
        s2.CASES[name](tree)
        tree.save()
    return run


# id -> (S1 operation, title, editor, built, expected outcome of the chain with the import of this branch, notes)
CASES = {
    "a1": ("A", "add attributes of every primitive type (objects no extension adopts)", run_a1, True),
    "b1": ("B", "delete attributes (middle, last, first, indexed, only, hierarchical, document, delete + add)", from_s2("b1"), True),
    "b2": ("B", "delete an attribute with the additional-order index (catalog, document)", from_s2("b2"), True),
    "c1": ("C", "widen variable strings (catalog, hierarchical, document, indexed, up to 1024)", from_s2("c1"), True),
    "d0": ("D", "the index flag: one alone", from_s2("d0"), True),
    "d1": ("D", "the index flag on and off, additional order", from_s2("d1"), True),
    "i1": ("I", "an attribute on adopted objects (refused by S1-I)", run_i1, True),
    # designed in docs/apply/restructuring.md 12.3, not built in the gate yet: the editors come with the operations
    "e1": ("E", "add a tabular section (not built)", None, False),
    "f1": ("F", "add a catalog / a document (not built)", None, False),
}

if __name__ == "__main__":
    command = sys.argv[1] if len(sys.argv) > 1 else "list"
    if command == "list":
        for case, (operation, title, editor, built) in CASES.items():
            print("%s\t%s\t%s\t%s" % (case, operation, "built" if built else "pending", title))
    elif command == "edit":
        case, out = sys.argv[2], sys.argv[3]
        operation, title, editor, built = CASES[case]
        if editor is None:
            raise SystemExit("case %s (%s) has no editor: the operation is not built yet" % (case, title))
        os.makedirs(out, exist_ok=True)
        editor(out)
    elif command == "files":
        with open(os.path.join(sys.argv[2], "files.txt"), encoding="utf-8") as f:
            print(f.read().strip())
    else:
        raise SystemExit("unknown command " + command)
