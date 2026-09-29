# Many generations, and the main configuration: 29.09.2026 (#390)

Measured on localhost, platform 8.3.27.2214, with the drop-in
`ibcmd-rs infobase config export --dbms=MSSQLServer --db-server=localhost
--db-name=<db> --platform=8.3.27 <folder>` and the native
`ibcmd infobase config export` (`--user=Администратор`, its own `--data`
folder) of the same database. Trees compared by SHA-256 of every file
(`compare_trees.py` of the lab folder `F:\ibcmd\lab\04\export-dyngen\tools`).
Databases are only read, except the two clones named below.

## The database that failed

`ibcmd_rs_04_rcheck_bsp_a` (БСП 8.3.27 after 16 `--dynamic=force` applies of a
full stage, on top of an earlier generation):

| | |
|---|---|
| rows in `Config` | 137 727 |
| rows with the `_dynupdate_` infix | 127 885 |
| plain rows | 9 842 |
| generations in `DynamicallyUpdated` | 17 |
| rows in `ConfigSave` | 9 516 (a full stage, `versions` of 9 839 pairs) |

The export before this change: exit -1 after 12.1 s,

```
failed to read the rows of ibcmd_rs_04_rcheck_bsp_a.Config: ... executing  on line 1 (code: 8621, ...)
```

The expression that failed listed each alias as a `CASE` branch and twice in
an `IN` list (nearly 10 000 names each). The expression now built is 2 343
characters and does not depend on the number of aliases (a unit test builds it
for 120 000 aliases and gets the same text).

After this change the export ends with exit 0 and writes **12 198 files, all
byte-identical to the native export** (12 198 of 12 198, `ConfigDumpInfo.xml`
included).

## Why the native export differed from `Config` alone

The native export publishes the *main* configuration. `ConfigSave` holds the
stage a completed import left, and the native export writes it:

- `ConfigDumpInfo.xml` of the native export carries the stamps of
  `ConfigSave.versions` on all 9 835 of 9 835 entries, and of none of the 17
  `versions_dynupdate_*` records;
- an export of `Config` alone was 12 040 files identical, 156 different, 2
  native-only, 1 ours-only;
- on `ibcmd_rs_04_rcheck_nat_a2` (a native stage of an added catalog attribute,
  no dynamic generation at all) an export of `Config` alone differed in 2 files
  (`Catalogs/_ДемоПартнеры.xml`, which lacks the attribute, and
  `ConfigDumpInfo.xml`); with the main configuration it differs in 2 other
  files, listed under "What is left" below.

The stage's `versions` row is not shaped like a `Config` one: the header
declares 9 834 pairs for 9 836 and no service pairs (`root`, `version`,
`versions`) are inside. The lab databases a native import and apply left
behind carry the same shape in `Config` itself (`ddl_a2_nat`, `_fin`, `_own`:
9 834 for 9 836; `ddl_bsp8327_a`: 9 835 for 9 836), and the strict reader
refused every one of them ("declares 9834 pairs but contains 19674 fields").

## What the platform does with generations (native, own clones)

- `ibcmd_rs_04_exdg_b`, the БСП corpus with its generation `06cb0442-…` and a
  crafted second generation that carries `ab132638-….0` with the module text
  of another module: the native export writes that text. A crafted third
  generation with an edited descriptor of the same object: the native export
  writes the edited property. The newest generation of the marker wins, for a
  module row and for a descriptor row.
- `ibcmd_rs_04_exdg_a`, a copy of the database above with `ConfigSave`
  emptied (the native export then reads `Config` alone): it fails in 88 s with
  «Ошибка формата потока» while `Config` holds rows the newest `versions` does
  not list (6 names, 11 rows: the objects and the modules a later generation
  removed). With those 11 rows deleted it exports, and its `ConfigDumpInfo.xml`
  is the newest `versions` on 9 834 of 9 834 entries. The export then differs
  from ours in 17 files: 17 objects whose newest carrier is a generation the
  tool-made history flipped a flag in are written by the platform from an
  older variant of the row, and the same happens with the history cut back to
  16, 13 and 10 generations. That is not explained and the export does not
  imitate it; it does not matter for the target database, where the staged
  rows decide those objects.

## Regression checks

| check | result |
|---|---|
| БСП with an active generation from SQL, `ibcmd_rs_bsp_8327_native_20260919` against `..._bsp_recheck\native` | 12 198 of 12 198 identical |
| БСП 8.3.27 offline (`--rows-dir F:\ibcmd\lab\rawrows\bsp\Config`), old binary against new | 12 199 of 12 199 identical; new against the native reference tree 12 198 identical (the 12 199th file is `manifest.json`) |
| БСП 8.5 offline (`F:\ibcmd\lab\v85\rawrows\bsp\Config`), old against new | 12 338 of 12 338 identical; new against the native reference 12 337 identical (+ `manifest.json`) |
| ERP УХ 8.3.27 offline, the 234 rows around its 156 superseded aliases, old against new | 255 of 255 identical |
| УХ, the constants: the native tree names 126 distinct `Constant.<n>.StandardCommand.<x>` and all belong to constants with `UseStandardCommands=true` | the constant rule cannot move a УХ file |
| the drop-in export of `ibcmd_rs_04_rcheck_nat_a2` (a native stage) | 12 196 of 12 198 identical; the two others are listed under "What is left" |

No full УХ tree was written (lab rule); the УХ tables have no `DynamicallyUpdated`
row and no staged rows, so `install_storage_overlay` returns their headers
unchanged and every statement is built on the bare table name.

## Time

The small БСП database with the active generation, alternating runs on a
machine other tracks were loading:

| | wall | process CPU | SQL statements | SQL CPU | logical reads |
|---|---|---|---|---|---|
| old, run 1 / 2 | 67.1 s / 25.2 s | 22.5 s / 20.8 s | 18 | 1 096 ms / 924 ms | 110 644 / 108 784 |
| new, run 1 / 2 | 25.2 s / 35.9 s | 22.3 s / 21.3 s | 20 | 1 187 ms / 1 045 ms | 193 584 / 190 054 |

The two extra statements read the `ConfigSave` headers and its `versions`
row. The wall time follows the load of the machine; the client CPU is the
same and the server CPU is 0.1 s more on a 25 s export.

`ibcmd_rs_04_rcheck_bsp_a`: 20 statements, 16 of them the range slices of the
row fetch, 21.0 s of server CPU in total (each slice reads the narrow columns
of the table once), 1.15 million logical reads; process CPU 22.1 s; 89 s on
the loaded machine. The old expression does not run at all.

## What is left

- `nat_a2`, `Configuration.xml`: `ConfigurationExtensionCompatibilityMode` is
  `Version8_3_27` in the native export and `Version8_3_24` in ours. The stage
  rewrites the Configuration row from the `{67}` to the `{68}` shape with the
  compatibility mode 80324 and the extension compatibility 80327; the model
  refuses that row and the legacy converter reads the wrong slot. The native
  export tells which one the platform prints: 80327 (field 43).
- `nat_a2`, `ConfigDumpInfo.xml`: the native export omits the entries of
  `Catalog.НастройкиАвторизацииИнтернетСервисов.ObjectModule` and
  `.ManagerModule` (`ffd14055-….0` and `.3`), which the staged `versions` lists
  and whose files it writes. The staged descriptor of that catalog is in the
  record format 57.
