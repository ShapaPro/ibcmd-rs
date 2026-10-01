# Direct activation certificate policy (F12, partial)

`mssql-activate-staged-main` now propagates its existing
`--sqlcmd-trust-cert` setting into the SQL handle used for actual row capture,
preflights and execution. With the option absent, certificate validation stays
enabled; with it present, the caller explicitly accepts the server certificate.
Both sqlcmd query and script builders add `-C` only for a trusted handle. The
built-in client receives the same policy; bounded binary capture already uses
that handle's policy. Profile verification and execution no longer disagree.

High-level source apply propagates this setting into its own read handle, but
still requires explicit trust because its legacy dump/staging helpers retain
their existing trust policy. Other staging/export commands are not converted
by this checkpoint. RAC password command-line handling also remains open.

A regression constructs both client/tool handles without any connection or
process launch, checks both certificate choices and verifies the actual query
and script argument lists. Legacy stage policy is checked separately. This is
policy propagation coverage, not a live certificate-chain acceptance matrix.
