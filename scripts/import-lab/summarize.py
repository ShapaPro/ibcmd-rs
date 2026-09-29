"""Summary of a staging matrix run (run_matrix.ps1) as a table and per-change row lists.

  python summarize.py <out\\matrix\\tag> [--md table.md] [--detail]

Reads <change>.<mode>.meta.json (exit code, message, stage mode, ConfigSave rows) and, for a stage that
succeeded, <change>.<mode>.json (rowdiff against the stage of the unchanged tree in the same mode).
`versions` always differs (fresh generation ids) and is reported apart.
"""
import argparse
import glob
import json
import os
import re
from collections import OrderedDict

MODES = ["patch", "bf", "native"]
ORDER = ["attr", "ts", "newcat", "syn", "prop", "attrprop", "attrdel", "newform", "newtpl", "predef", "enumval",
         "rights", "subsys", "nestsub", "ci", "module", "confver", "catdel"]


def label(name, what):
    """`Catalog.Имя [path].0` -> `Catalog.Имя .0`."""
    m = re.match(r"^(.*?) \[[^\]]*\](.*)$", what)
    if m:
        return (m.group(1) + m.group(2)).strip()
    return what


def load(outdir):
    result = OrderedDict()
    for meta_path in sorted(glob.glob(os.path.join(outdir, "*.meta.json"))):
        with open(meta_path, encoding="utf-8-sig") as f:
            meta = json.load(f)
        change, mode = meta["change"], meta["mode"]
        entry = {"meta": meta, "rows": None}
        rows_path = meta_path.replace(".meta.json", ".json")
        if os.path.exists(rows_path) and meta.get("exit") == 0:
            with open(rows_path, encoding="utf-8") as f:
                entry["rows"] = json.load(f)
        result.setdefault(change, {})[mode] = entry
    return result


def changed_rows(entry):
    out = {"different": [], "new": [], "dropped": [], "versions": False}
    for name, row in entry["rows"]["rows"].items():
        cls = row["class"]
        if name == "versions":
            out["versions"] = cls not in ("identical_bytes", "identical_text")
            continue
        if cls == "different":
            out["different"].append((name, label(name, row["what"])))
        elif cls == "only_left":
            out["new"].append((name, label(name, row["what"])))
        elif cls == "only_right":
            out["dropped"].append((name, label(name, row["what"])))
    return out


def cell(entry):
    if entry is None:
        return "-"
    meta = entry["meta"]
    if meta.get("exit") != 0:
        tail = meta.get("tail") or ""
        m = re.findall(r"\[ERROR\] (.*)", tail.replace(" | ", "\n"))
        msg = next((x for x in m if "завершен с ошибкой" not in x), tail)
        return "REFUSED: " + msg[:110]
    rows = changed_rows(entry)
    n = len(rows["different"]) + len(rows["new"]) + len(rows["dropped"])
    if n == 0:
        return "LOST (no row differs from the unchanged stage)"
    return "%d rows (%d changed, %d new, %d dropped)" % (n, len(rows["different"]), len(rows["new"]), len(rows["dropped"]))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("outdir")
    ap.add_argument("--md")
    ap.add_argument("--detail", action="store_true")
    args = ap.parse_args()
    data = load(args.outdir)
    lines = ["| change | patch | base-free | native |", "|---|---|---|---|"]
    for change in [c for c in ORDER if c in data] + [c for c in data if c not in ORDER]:
        cells = [cell(data[change].get(m)) for m in MODES]
        lines.append("| %s | %s |" % (change, " | ".join(cells)))
    print("\n".join(lines))
    if args.detail:
        for change in [c for c in ORDER if c in data] + [c for c in data if c not in ORDER]:
            print("\n### " + change)
            for mode in MODES:
                entry = data[change].get(mode)
                if entry is None:
                    continue
                meta = entry["meta"]
                print("- %s: exit=%s rows=%s %ss" % (mode, meta.get("exit"), meta.get("rows"), meta.get("seconds")))
                if entry["rows"] is None:
                    print("    " + (meta.get("tail") or "")[:300])
                    continue
                rows = changed_rows(entry)
                for key in ("different", "new", "dropped"):
                    for name, what in rows[key][:12]:
                        print("    %-9s %s" % (key, what))
                    if len(rows[key]) > 12:
                        print("    %-9s ... %d more" % (key, len(rows[key]) - 12))
    if args.md:
        with open(args.md, "w", encoding="utf-8") as f:
            f.write("\n".join(lines) + "\n")


if __name__ == "__main__":
    main()
