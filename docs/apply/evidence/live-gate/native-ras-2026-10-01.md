# Native staged/RAS checkpoint, 2026-10-01

Scoped research on SQL Server **17.0.1135.8**, service cluster **8.3.27.2214**,
with newly restored, Track live BSP clones only. No cluster settings changed.
Raw evidence is retained at `F:/ibcmd/lab/05/wave2/live`; ownership and current
remaining work are in its `STATUS.md`. This is not full F-5 or zero-error acceptance.

## Fixture boundary

The corpus contains native dynamic history. Native partial import of
`CommonModules/ОбсужденияСлужебныйКлиентСервер/Ext/Module.bsl`, followed by
`config apply --force --dynamic=disable`, removes Config aliases/marker but
retains an 82-byte `Params.DynamicallyUpdated`. The supported checkpoint still
refuses that history. The proof fixture explicitly removes this captured residual
marker after checking no RAS/SQL user sessions; it does not claim unchanged native BSP.

`proof-import-baseline.command.json` and `proof-force-baseline.command.json`
preserve exact executable/flags. `proof-normalize.sql`/`.log` retain the marker
bytes, conditional exactly-one deletion, and equal fingerprints of all 9841
ordinary Config rows. That fingerprint includes filename, part, DataSize,
binary length and SHA, **but not Creation/Modified/Attributes**. It is a partial
header fingerprint; do not retroactively interpret it as full-header parity.
The separate exploratory clone's first normalization lacked raw-marker and
whole-Config fingerprint preservation and is excluded from this audited proof.

## Measured checkpoint and refusal

The real native sparse stage contains five complete rows including `versions`,
with no `commit`/`.new` rows (`proof-stage-idle2.log`). A fresh iter build from the
wave2 source is required; binary SHA/source provenance is saved separately.

| Case | Observed result / raw files |
|---|---|
| Production renderer token | BASE dry-run refused before DB writes because uppercase report SHA differed from lowercase validation (`proof-idle2-dry.stderr`). Digest comparison now accepts either hex spelling, retains backup names, and refuses wrong/nonhex/short tokens. |
| Empty RAS / two idle 1C SQL handles | Real promotion/cycle 1 and automatic cycle 2 complete (`proof-idle2-execute.json`); new COM session reads `LIVE-idle2`. |
| Repeated continuation | Two `already_complete`, `cycle_2_executed=false` reports; tail SHA unchanged; actual HEADERONLY contains the two owned sets (`proof-idle2-noop*.json`, `proof-idle2-header.log`, `proof-idle2-assertions.log`). |
| Own observer with real open SQL transaction | F-10 `57238` before script/recovery/tail publication. Config and ConfigSave snapshots unchanged (`explore-active-{before,after}.log`, `.stderr`, `-assertions.log`). |
| Warm polling user | Promotion/cycle 1 commit, one owned log set, ConfigSave empty. Cycle 2 is retained. Initial native RAS info encounters an OLE DB reconnect error; later strict UTF-8 inventory fails on OEM username bytes (`proof-warm1-execute.json`, `-refusal-retry.stderr`, `-ras-raw.stdout.bin`). |
| Fixed inventory reader | Nonempty OEM inventory emits the intended warm-session refusal, with byte-identical tail (`proof-warm1-fixed-refusal*`). |
| Client process closed | Exact recorded session remains hibernating; inventory still refuses. Only the journal/infobase-bound owned UUID is terminated (`proof-warm1-orphan-ras.txt`, `-terminate.command.json`, `-terminated-ras.txt`). |
| Resume after ending that session | `complete`, then byte-identical `already_complete`. First fresh COM attempt reports a terminated-admin session; the next succeeds with `LIVE-warm1` (`proof-warm1-closed-continue.json`, `-noop.json`, `-new-marker*.log`). Both outcomes are evidence; success does not erase the first error. |

The warm client and server continue reading the old marker before their session
ends; this experiment does not establish client-code refresh in the same session.
The production RAS deadline remains five seconds. Windows subprocess protocol
tests allow 20 seconds for pwsh startup; the deadline/kill regression remains bounded.

## Validation and remaining acceptance

Quick gates on the unchanged production version: fmt, policy guard, workspace
layer clippy and root library **3652 passed, 0 failed, 10 ignored**; all-targets
check passed. The final tests-only pwsh startup allowance is followed by focused
continuation tests and fmt. No full/release track gates were run.

L1 is measured on this explicitly normalized native fixture. L2 has real active
work/warm refusal and safe operator continuation evidence; repeated warm runs and
full-header normalization control are still being recorded. Warm readiness that
permits connected users, load acceptance, same-session client refresh and zero
connection errors remain **OPEN**. Five repetitions alone will not establish those
properties. Source-to-promotion checkpoint support and broader SQL/platform builds
remain unsupported.
