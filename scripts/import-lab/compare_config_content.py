"""Config rows of two lab databases compared by what they hold (issue #388, the S1 acceptance of the override).

  python compare_config_content.py <db-a> <db-b> [--out <file>]

rcheck's compare_config.ps1 compares the stored bytes, which is right for twins that start from the very same ConfigSave.
The stage of our import and the platform's are not the same bytes (another deflate stream, another generation of the
rows), so here each row is put together from its parts, inflated when it inflates (raw deflate; a parent configuration is
deflated twice and is compared as stored), and compared by length and SHA-256 of the result. Lists the rows only in
one database, and the rows that differ. Read-only; lab databases only.
"""
import argparse
import hashlib
import sys
import zlib

import pyodbc

CONN = ("DRIVER={ODBC Driver 18 for SQL Server};SERVER=localhost;DATABASE=%s;"
        "Trusted_Connection=yes;TrustServerCertificate=yes")


def rows_of(db):
    if not db.startswith("ibcmd_rs_04_"):
        raise SystemExit("lab databases only: " + db)
    con = pyodbc.connect(CONN % db, autocommit=True)
    cur = con.cursor()
    cur.execute("SELECT FileName, PartNo, BinaryData FROM Config ORDER BY FileName, PartNo")
    out = {}
    name, parts = None, []

    def flush():
        if name is None:
            return
        raw = b"".join(parts)
        try:
            plain = zlib.decompress(raw, -15)
            kind = "deflate"
        except zlib.error:
            plain = raw
            kind = "raw"
        out[name] = (kind, len(plain), hashlib.sha256(plain).hexdigest())

    while True:
        batch = cur.fetchmany(200)
        if not batch:
            break
        for file_name, _part, data in batch:
            if file_name != name:
                flush()
                name, parts = file_name, []
            parts.append(bytes(data))
    flush()
    con.close()
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("a")
    ap.add_argument("b")
    ap.add_argument("--out", default="")
    args = ap.parse_args()
    a, b = rows_of(args.a), rows_of(args.b)
    lines = []
    only_a = sorted(set(a) - set(b))
    only_b = sorted(set(b) - set(a))
    differ = sorted(n for n in set(a) & set(b) if a[n][1:] != b[n][1:])
    lines.append("rows: %s %d, %s %d" % (args.a, len(a), args.b, len(b)))
    lines.append("== only in A (%d)" % len(only_a))
    lines += ["  " + n for n in only_a]
    lines.append("== only in B (%d)" % len(only_b))
    lines += ["  " + n for n in only_b]
    lines.append("== same name, other content (%d)" % len(differ))
    lines += ["  %s  A %d bytes  B %d bytes" % (n, a[n][1], b[n][1]) for n in differ]
    text = "\n".join(lines)
    print(text)
    if args.out:
        with open(args.out, "w", encoding="utf-8") as f:
            f.write(text + "\n")


if __name__ == "__main__":
    sys.exit(main())
