# Load into an empty database: all four corpora (2026-09-25 .. 2026-09-28)

Branch `feat/metadata-model`. Every Config row is compiled from the XML tree
alone (`mssql-stage-source-objects --base-free`): no base row is read from the
target, so the target can be a freshly created, empty infobase.

## Procedure

`scripts/empty-load/run_empty.ps1`, one fresh disposable database per run
(`ibcmd_rs_empty_<corpus>_ours_<date>_<tag>`, files on F:):

1. `CREATE DATABASE` (SIMPLE recovery).
2. Native `ibcmd infobase create` with no configuration.
3. `ibcmd-rs mssql-stage-source-objects --base-free` writes ConfigSave.
4. Native `ibcmd infobase config apply --force --dynamic=disable`.
5. Native `ibcmd infobase config export`, then `ibcmd-rs source-diff`
   against the reference tree the load started from.

Before each real run the same rows went through the offline cycle
`scripts/empty-load/ve.sh` (stage rows to files, export them, diff), which
gave the same file counts.

## Results

| Corpus | Platform | Rows staged | Stage | Native apply | Native export vs reference |
|---|---|---|---|---|---|
| БСП 8.3.27 | 8.3.27.2214 | 9 838 | 133 s | 814 s, then SDBL (see below) | **12 197 / 12 198** |
| БСП 8.5 | 8.5.1.1150 | 9 935 | 156 s | 1 395 s | **12 336 / 12 337** |
| ERP УХ 8.3.27 | 8.3.27.2214 | 118 025 | 2 034 s | 14 564 s | **140 708 / 140 709** |
| ERP УХ 8.5 | 8.5.1.1150 | 118 025 | 1 220 s | 12 533 s | **140 708 / 140 709** |

The one file that differs in every run is `ConfigDumpInfo.xml`. Every other
file is byte-identical.

- **БСП 8.3.27 and 8.5:** `ConfigDumpInfo.xml` is identical to the
  reference once its `configVersion` values are blanked. Those are the
  generation ids the platform writes anew on every save.
- **ERP УХ 8.3.27 and 8.5:** `ConfigDumpInfo.xml` also lacks 145 entries for
  rows the source database keeps:
  - 132 empty Help rows, 10 bytes each;
  - 11 Predefined rows of 644–727 bytes, for owners with no predefined items;
  - 1 Aggregates row;
  - 1 CommandInterface row of 18 bytes.

  None of these rows has content the export writes as a file, which is why
  every other file is identical. The stage does not write them, and the
  platform does not create them on apply. Open item: write them from the
  source tree's `ConfigDumpInfo.xml`.

The binaries: `int_*` builds of `feat/metadata-model` (БСП 8.3.27 on
the build of 2026-09-25, БСП 8.5 `dd9b7920`, УХ 8.3.27 `2059e589`) and track
E's `t2` build (`540a5601` merged with `a930aa9c`) for УХ 8.5.

Times are from a shared machine (five agents building and testing), so the
stage times are high: on a quiet machine the offline УХ stage takes 462 s.
Native apply spends most of its УХ time in "Принятие изменений", renaming the
118 026 Config rows with one `UPDATE config SET FileName = ...` each.

## What the real runs taught

- **БСП 8.3.27, SDBL on the first apply.** The first `config apply` of the
  staged БСП into a fresh 8.3.27.2214 database on SQL Server 2025 ends at
  "Принятие изменений" with SDBL "Использование быстрой вставки недопустимо
  без удаления индексов"; a second apply says "не требуется" and the export is
  complete. Native's own route fails the same way: `config save` to a `.cf`
  and `infobase create --load=<cf> --apply` into a fresh database
  (`native_cf_oracle.ps1`) stops with the same SDBL error, and its export is
  12 198 / 12 198. It is platform behaviour, not our rows; neither УХ 8.3.27
  nor the 8.5 runs hit it.
- **8.5 `version` row: a feature list, not a random uuid.** `version` is
  `{{217,0,{<compat>,<count>,{<feature uuid>}...}}}` for 8.5. БСП 8.5 lists
  one feature, `2dd2d9e1-40c8-430b-a433-a81ec6856ab0`, which `backend.dll`
  8.5.1.1150 registers (with twelve 8.3.x features) as the 8.5.1 palette
  colours: БСП 8.5 has two PaletteColor objects. A random uuid there made
  `config apply` refuse with "Для использования этой конфигурации требуется
  более новая версия платформы". Fixed in `e3695b8e`: the feature is listed
  when the configuration has palette colours, and an unknown 8.5
  compatibility level is refused rather than guessed. The root row's 8.5
  signature and the Configuration row's footer (`{1,"",""},{23}` native,
  `{0,"",""}` ours) are not needed: apply and export are identical without
  them.
- **2.21 table lines at compatibility 8.3.27.** ERP УХ 8.5 is a 2.21 tree at
  compatibility 8.3.27, so its forms are stored in the 8.3.27 layout. There a
  table's `HorizontalLinesBWA`, `VerticalLinesBWA` and
  `UseAlternationRowColorBWA`, when unset, mean a true old slot; reading them
  as false put `...BWA>false` into 7 258 exported forms. Fixed in `540a5601`.
- **An empty 8.5 infobase has no Config rows at all** before the first
  apply; the stage writes ConfigSave all the same.
