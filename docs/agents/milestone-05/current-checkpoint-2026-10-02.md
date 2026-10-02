# Current 0.5 verification checkpoint

The immutable source `7aebba15` completed the full local run with **14 PASS and
two packaging failures**. Both packaging scripts stopped at Python's inherited
invalid stdin handle (`WinError 6`); no Rust test or compilation failed. The
original failed outcome remains unchanged, and the exact owned heavy lease was
released with known exit zero and no unresolved child.

A separately frozen V5 child controller establishes the started child identity
and output tracking before closing redirected stdin to EOF. The live-child
close-failure regression and independent peer review passed. The unchanged SBOM
and release-audit scripts then both returned zero on the same clean source and
unchanged release executable. This is **14 original PASS + two targeted PASS**,
not a rewritten claim that the original single run passed all sixteen gates.
The release binary SHA256 is
`44513F752CB946644C856B69D740189A045452682989C6E7FC955A59035D0E38`.
Library tests: **3729 passed / 0 failed / 10 ignored**. The producer and exact
verification receipts are preserved in [the raw checkpoint](evidence/current-checks-2026-10-02/checkpoint.json).

The D2 desired source already exists on F: root and peer independently read
all 12198 baseline and desired files, checked SHA256, lengths and ordinary path
ancestry, and found exactly the three measured template donor differences.
All module B bytes and ConfigDumpInfo remain preserved. A fresh read-only
certificate corrects the earlier incomplete *knowledge state*; historical
receipts remain immutable. No redundant copy, acquire or SQL/native write was
performed. This proves source readiness only; template activation remains open.

Compact7's actual prewrite archive refusal and the separate compact8 archive
identity preparation are now included in this PR. Compact8's complete runtime
acceptance, the native/OWN Params matrix, nested forms, larger stage and other
roadmap criteria remain open. No issue closure or release tag follows from
this checkpoint.
