# Independent first-wave review

Reviewer: coordinator, independent of the three implementation authors.
Review combines specification and code correctness; actual source and raw
test logs were inspected, not only implementer summaries.

## Dynamic checkpoint: PASS

Base `2a55cb3462aae8ce7cb8e08e8837bb404154bd68`,
implementation `d807ec00f0f3bdfef316549b0b7c3e45500c4ec6`.

- Explicit force is the only dynamic route. Exclusive modes/session refusal
  remain unchanged; unsupported platforms, structural targets and nonempty
  overlay deletion lists refuse before publication.
- The recovered feature diff retains v0.4 synonym/import/restructure fixes.
  The extracted parity writer preserves exclusive SQL statements/order, and
  only the new dynamic capability changes in the platform profiles.
- Descriptor/root/deleted semantic reads bind to their initial digests; the
  final engine snapshot and both markers bind to that judged inventory.
  Transient safe deleted content cannot authorize different published bytes.
- SQL assertions, marker/parity writes and stage consumption remain in one
  serializable transaction. No-op consumes staging without publication/parity.
- Unknown commit outcome retains recovery location/token and never promises
  rollback. Post-commit verification failure is distinguished explicitly.
- Raw final quick logs confirm 3624 passed/0 failed/9 ignored; focused suites
  and CLI integration match the versioned checkpoint evidence. Diff whitespace
  check passes. Current native/session proof is expressly unclaimed.

No outstanding P1/P2 findings in this bounded checkpoint. This review does
not close #347 or approve the wider pending-overlay/8.5/load boundaries.

## Extension checkpoint: PASS

Reviewer: `release_review`, independent of the extension implementation author.
Base `2a55cb3462aae8ce7cb8e08e8837bb404154bd68`, implementation
`c2cccffc061578e3f14cb7d0a8aa9219eaeffcb6`.

- Stored primary/additional event codes produce native Before/After/Override
  forms. Unsupported or ambiguous shapes refuse, retaining the BaseForm path
  and the measured Before command behavior.
- The lexical XML finding is repaired: semantic attributes/dispatch, comment
  handling and after-fold checks prevent silent loss of interceptors.
- The reviewer verified native row hashes/blocks, raw trees and thirteen
  versioned comparisons. All source bytes match; the only CDI normalization
  is configVersion values. Applied own CAS roots match the staged reports.
- Final sequential raw logs on the fixed source report 3607 passed/0 failed/
  9 ignored, with fmt/policy/clippy passing. The policy baseline adds exactly
  two reviewed name-special-case occurrences and preserves old allowances.
- The evidence identifies the actual staging binary, subsequent parser
  repairs, native Version seed 1.7.0.0 → 1.7.0.1, no built CFE, and the
  unchanged direct root-Version refusal. Scope does not close all of #348.

No outstanding P1/P2 findings. Coordinator cherry-pick applied without a
source conflict; the shared mssql.rs change remains the reviewed dispatch.

## Live checkpoint

Pending fixed implementation commit, real SQL-only protocol evidence and
independent final review. Warm-session/native RAS acceptance remains open.
