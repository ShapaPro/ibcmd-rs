"""Puts the edited files of a case over the working tree, and takes them off again.

usage:
  python overlay.py apply   <case dir> <tree>   copy <case dir>/stage/<rel> over <tree>/<rel>
  python overlay.py restore <case dir> <tree>   copy <case dir>/before/<rel> back
  python overlay.py verify  <case dir> <tree>   the tree's files equal the reference (after a restore) or the stage (after an apply)

The working tree is a copy of the reference native export made once (robocopy); a case edits a handful of files of it,
our import reads the tree, and the files go back, so one tree serves every case.
"""
import hashlib
import os
import shutil
import sys


def rel_files(case_dir):
    with open(os.path.join(case_dir, "files.txt"), encoding="utf-8") as f:
        return [line.strip() for line in f if line.strip()]


def sha(path):
    with open(path, "rb") as f:
        return hashlib.sha256(f.read()).hexdigest()


def main():
    command, case_dir, tree = sys.argv[1], sys.argv[2], sys.argv[3]
    files = rel_files(case_dir)
    if command == "apply":
        for rel in files:
            shutil.copyfile(os.path.join(case_dir, "stage", rel), os.path.join(tree, rel))
        print("applied", len(files), "files")
    elif command == "restore":
        for rel in files:
            shutil.copyfile(os.path.join(case_dir, "before", rel), os.path.join(tree, rel))
        print("restored", len(files), "files")
    elif command == "verify":
        bad = 0
        for rel in files:
            stage = os.path.join(case_dir, "stage", rel)
            before = os.path.join(case_dir, "before", rel)
            current = os.path.join(tree, rel)
            state = "stage" if sha(current) == sha(stage) else "before" if sha(current) == sha(before) else "other"
            print(state, rel)
            bad += state == "other"
        sys.exit(1 if bad else 0)
    else:
        raise SystemExit("unknown command " + command)


if __name__ == "__main__":
    main()
