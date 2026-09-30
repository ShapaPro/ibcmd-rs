"""What a native apply changed in the DBSchema entries and in DBNames, between two snapshots (they may belong to two databases:
the staged state of a case and the native result of its twin).

usage: python entries_diff.py <db before> <label before> <db after> <label after>
Prints, for every table entry that differs (but the platform's DbCopies*), the field list and the declared indexes side by
side, and the DBNames entries added or removed. Read-only; the snapshots come from snapshot.py.
"""
import sys
import zlib

import bracefmt as bf
import lab
import names


def tables(db, label):
    schema = bf.parse(lab.dbschema(label, db))
    return {str(t[0]): t for t in schema[1][1:]}


def fields(table):
    return [bf.dumps(f).replace("\r\n", "") for f in table[4][1:]]


def indexes(table):
    return [bf.dumps(i).replace("\r\n", "") for i in table[6][1:]]


def names_of(db, label):
    text = zlib.decompress(lab.row(label, "Params", "DBNames", db=db), -15)
    return names.parse_dbnames(text)


def main():
    db1, label1, db2, label2 = sys.argv[1:5]
    before, after = tables(db1, label1), tables(db2, label2)
    for name in sorted(set(before) | set(after)):
        if name.startswith("DbCopies") or before.get(name) == after.get(name):
            continue
        print("== table", name)
        if name not in before or name not in after:
            print("   only in", "after" if name in after else "before")
            continue
        f1, f2 = fields(before[name]), fields(after[name])
        if f1 != f2:
            for line in f1:
                if line not in f2:
                    print("   - field", line[:200])
            for line in f2:
                if line not in f1:
                    print("   + field", line[:200])
        i1, i2 = indexes(before[name]), indexes(after[name])
        print("   indexes before (%d):" % len(i1))
        for line in i1:
            print("     ", line[:220])
        print("   indexes after (%d):" % len(i2))
        for line in i2:
            print("     ", line[:220])
    max1, count1, entries1 = names_of(db1, label1)
    max2, count2, entries2 = names_of(db2, label2)
    print("== DBNames: header max %d -> %d, count %d -> %d" % (max1, max2, count1, count2))
    s1, s2 = set(entries1), set(entries2)
    for uuid, kind, number in sorted(s2 - s1, key=lambda e: e[2]):
        print("   ADDED", number, kind, uuid)
    for uuid, kind, number in sorted(s1 - s2, key=lambda e: e[2]):
        print("   REMOVED", number, kind, uuid)


main()
