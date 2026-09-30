"""S1-F in the combination matrix (issue #391, after S1-F #402): a stage that creates a catalog or a document and also changes other objects.

The platform stages a new object on a base that already had a native apply only (docs/apply/new-object.md 8): the base is the backup
bak\\ibcmd_rs_04_ddl_bsp8327_c2_native_after.bak, its native export is tree_c2. The new objects are the ones of the S1-F cases of the trace track
(newobj\\n1 a flat catalog, n4 a document, n5 a catalog and a document; copies of F:\\ibcmd\\lab\\05\\s1g\\n, made by scripts/apply-trace/lab/
s1g-caches/make_cases_n.py), their files and Configuration.xml go into the stage as they are, the changes of the other objects are made on
tree_c2 like the cases of edit_cases_s4.py.

Run the case with the base in the environment:
  $env:MIX_BASE_BAK = 'F:\\ibcmd\\lab\\04\\restructure\\bak\\ibcmd_rs_04_ddl_bsp8327_c2_native_after.bak'
  $env:MIX_BASE_TREE = 'F:\\ibcmd\\lab\\04\\restructure\\tree_c2'
  pwsh -NoProfile -File mix_case.ps1 -Case f1 [-Refused] [-Ours]

usage: python edit_cases_s5.py <out dir> <case>
  f1  a new catalog next to C and D on other objects: two widened strings, three switched indexes (a catalog, a hierarchical one, a document)
  f3  a new catalog and a new document next to A, C and D on other objects
  r7  REFUSED: a new document and an attribute removed from a catalog (the caches of a new object are chained on additions only)
  r8  REFUSED: a new catalog and new tabular sections (a section, an attribute of one) of existing objects
"""
import os
import shutil
import sys

os.environ.setdefault("DDL_NATIVE_TREE", r"F:\ibcmd\lab\04\restructure\tree_c2")
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from edit_cases_s2 import BANKS, JOBS, KEYOPS, VOLUMES, PAYROLL, Tree, flip, remove_attribute, widen  # noqa: E402
from edit_cases_s3 import add_section_attribute, section_attribute, s  # noqa: E402
from edit_cases_s4 import CASH, INTAKE, PAYMENT, own, section_of  # noqa: E402

NEWOBJ = os.environ.get("MIX_NEWOBJ", r"F:\ibcmd\lab\04\restructure\newobj")


def new_objects(tree, case):
    """The files of a new-object case of S1-F, copied into the stage as they are."""
    source = os.path.join(NEWOBJ, case)
    with open(os.path.join(source, "files.txt"), encoding="utf-8") as f:
        files = [line.strip() for line in f if line.strip()]
    for rel in files:
        target = os.path.join(tree.stage, rel)
        os.makedirs(os.path.dirname(target), exist_ok=True)
        shutil.copyfile(os.path.join(source, "stage", rel), target)
        print("new", rel)
    tree.extra = files


def run_f1(tree):
    new_objects(tree, "n1")
    widen(tree, KEYOPS, "ИмяХеш", 40, 100)
    widen(tree, PAYMENT, "НазначениеПлатежа", 210, 300)
    flip(tree, BANKS, "Телефоны", "DontIndex", "IndexWithAdditionalOrder")
    flip(tree, VOLUMES, "МаксимальныйРазмер", "DontIndex", "Index")
    flip(tree, PAYMENT, "НомерВходящегоДокумента", "DontIndex", "Index")


def run_f3(tree):
    new_objects(tree, "n5")
    own(tree, JOBS, "КлючНовый", 64, "ForItem")
    own(tree, CASH)
    widen(tree, KEYOPS, "ИмяХеш", 40, 100)
    flip(tree, VOLUMES, "МаксимальныйРазмер", "DontIndex", "Index")
    flip(tree, PAYROLL, "ПериодРегистрации", "IndexWithAdditionalOrder", "DontIndex")


def run_r7(tree):
    new_objects(tree, "n4")
    remove_attribute(tree, BANKS, "КоррСчет")
    widen(tree, KEYOPS, "ИмяХеш", 40, 100)


def run_r8(tree):
    new_objects(tree, "n1")
    section_of(tree, KEYOPS, "ДемоТЧ", [s("ДемоСтрока", 20)])
    add_section_attribute(tree, INTAKE, "Товары", section_attribute(*s("ДемоСтрока", 10, "Index")))
    widen(tree, BANKS, "Город", 50, 200)


CASES = {"f1": run_f1, "f3": run_f3, "r7": run_r7, "r8": run_r8}

if __name__ == "__main__":
    out, case = sys.argv[1], sys.argv[2]
    if case not in CASES:
        raise SystemExit("unknown case %s; known: %s" % (case, ", ".join(CASES)))
    tree = Tree(out)
    tree.extra = []
    CASES[case](tree)
    tree.save()
    # files.txt lists the new objects' files too
    path = os.path.join(out, "files.txt")
    with open(path, encoding="utf-8") as f:
        files = [line.strip() for line in f if line.strip()]
    with open(path, "w", encoding="utf-8") as f:
        f.write("\n".join(sorted(set(files) | set(tree.extra))) + "\n")
