# Native 8.5 signed root restamp

These 222-byte fixtures are the exact raw-deflate-inflated `root` bytes from
the fresh BSP85 pure-B module-only stage on platform 8.5.1.1150. They retain the
native BOM and CR/LF bytes. They are not decoded signatures or licensing data.

The active and staged rows use tag `2`, the same canonical root UUID, and a
128-byte opaque base64 payload. Its first 112 bytes are equal; only the final
16 bytes differ. The helper does not declare any write capability or accept
another payload size or platform build.

Historical lab provenance (2026-10-01):
`F:/ibcmd/lab/05/wave3/platform85/snapshots/staged_b_own/{Config,ConfigSave}.pack`
and the adjacent `inventory.json` retain the compressed bytes and complete
physical headers. `logs/pure-b-contract.json` records the native stage/twin
comparison. SHA-256 of these inflated fixtures:

- active: `886608f3fc22864506a0399ba623dc0d2d41c71f7a89e0a8ff352a15bc764f30`
- staged: `a827e300a3e396a77d19ad2f7e3e5aff4a751fcad02d56dcce2d752571534d47`
