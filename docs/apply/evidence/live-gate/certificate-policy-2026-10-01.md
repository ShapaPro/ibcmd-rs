# Direct activation certificate policy (F12, partial)

`mssql-activate-staged-main` now propagates its existing
`--sqlcmd-trust-cert` setting into the SQL handle used for actual row capture,
preflights and execution. With the option absent, certificate validation stays
enabled; with it present, the caller explicitly accepts the server certificate.
Both sqlcmd query and script builders add `-C` only for a trusted handle. The
built-in client receives the same policy; bounded binary capture already uses
that handle's policy. Profile verification and execution no longer disagree.

High-level source apply now supplies its explicit SQL handle to active export,
source staging and dry-run parity preparation. These paths use the same requested
certificate policy as profile verification, preflights and activation. Main
source apply therefore no longer requires `--sqlcmd-trust-cert`: the option is
an explicit choice, and absence keeps validation enabled. The original standalone
dump/stage/audit entry points retain their legacy defaults. RAC password
command-line handling also remains open.

A regression constructs both client/tool handles without any connection or
process launch, checks both certificate choices and verifies the actual query
and script argument lists. Legacy stage policy is checked separately. This is
policy propagation coverage, not a live certificate-chain acceptance matrix.

The source-apply regression exercises actual dump, stage and parity entry points
with a supplied read-only SQL trap for both certificate choices. Each route
reaches that supplied handle; a refused preparation read prevents any write.
Both built-in/tool constructor choices are checked without opening a connection.
The first test fixture omitted required CommonModule properties and failed before
the stage SQL read; a valid XML fixture corrected that test setup. No product
fallback or certificate validation was relaxed to pass the test.
