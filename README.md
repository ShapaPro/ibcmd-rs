# ibcmd-rs

Быстрая замена штатного `ibcmd` для выгрузки и загрузки конфигурации 1С
между XML-файлами и базой на Microsoft SQL Server. Утилита работает с
таблицами базы напрямую, а на выходе даёт тот же XML, что и штатный
`ibcmd infobase config export`.

> **Внимание.** Проект экспериментальный и предназначен для исследований и
> обучения. Команды, которые пишут в базу, запускайте только на копиях баз.

## Что умеет

| Задача | Команда | Проверено |
|---|---|---|
| Выгрузить конфигурацию из базы в XML (аналог `config export`) | `mssql-dump-config` | БСП и ERP УХ, платформы 8.3.27 и 8.5: все файлы совпадают со штатной выгрузкой |
| Загрузить XML в существующую базу (аналог `config import`) | `mssql-stage-source-objects` | БСП и ERP УХ: после загрузки штатная выгрузка совпадает с исходным XML |
| Загрузить XML в пустую базу | `mssql-stage-source-objects --base-free` | БСП и ERP УХ, 8.3.27 и 8.5 |
| Сравнить два дерева XML | `source-diff` | — |
| Выгрузить XML без сервера, из сохранённых строк таблицы Config | `mssql-dump-config --rows-dir` | — |

Скорость на одной машине, ERP УХ (около 140 тысяч файлов):

| | штатный `ibcmd` | `ibcmd-rs` |
|---|---|---|
| Выгрузка ERP УХ 8.3.27 | 6–7 мин | 3,5–4,5 мин |
| Выгрузка ERP УХ 8.5 | 5,5–7 мин | 3–4 мин |
| Загрузка ERP УХ 8.5 (XML в ConfigSave) | 11 мин 49 с | 7 мин 16 с |

Применение конфигурации (`config apply`) в обоих случаях выполняет платформа.

## Что нужно

- Windows x64.
- Microsoft SQL Server и его утилиты `sqlcmd` и `bcp` (Command Line Utilities).
- Для загрузки: штатная платформа 1С (`ibcmd.exe`) той же версии, что и база.
  Она создаёт пустую базу и применяет загруженную конфигурацию.

Скачайте архив `ibcmd-rs-<версия>-x86_64-pc-windows-msvc.zip` со страницы
[Releases](https://github.com/Untru/ibcmd-rs/releases) и распакуйте его.
Версия утилиты: `ibcmd-rs --version`.

## Как указать версию

**Версия формата XML** задаётся параметром `--source-version`:

| Значение | Платформа | Как узнать |
|---|---|---|
| `2.20` | 8.3 (выгрузка `ibcmd` 8.3.27) | `version="2.20"` в первой строке `Configuration.xml` |
| `2.21` | 8.5 | `version="2.21"` там же |

При выгрузке это формат, в котором будет записан XML. При загрузке — формат
загружаемых файлов: утилита проверяет, что он совпадает.

**Режим совместимости конфигурации** указывать не нужно, он берётся из
`Configuration.xml`. Например, ERP УХ для 8.5 выгружается в формате `2.21`,
но с режимом совместимости 8.3.27.

## Подключение к SQL Server

По умолчанию утилита подключается к `localhost` с входом Windows. Для входа
SQL Server укажите `--sql-user <логин>`, а пароль положите в переменную
окружения `IBCMD_DB_PSW` или передайте `--sql-pwd`. Имя сервера задаёт
`--server`. Путь к `sqlcmd` задаёт `--sqlcmd`; `bcp` ищется рядом с ним.

В примерах ниже:

```
set SQLCMD=C:\Program Files\Microsoft SQL Server\Client SDK\ODBC\180\Tools\Binn\SQLCMD.EXE
```

## Выгрузить конфигурацию в XML

```
ibcmd-rs mssql-dump-config --database MyBase --sqlcmd "%SQLCMD%" ^
  --extract-metadata-xml --extract-module-text --no-binary-rows ^
  --source-version 2.20 -o C:\export\MyBase
```

В папке `C:\export\MyBase` окажется то же дерево XML, что у штатной выгрузки,
плюс служебный `manifest.json`. `--overwrite` разрешает писать в непустую
папку.

## Загрузить XML в существующую базу

Загрузка идёт в два шага, как у штатного `ibcmd`: сначала конфигурация
записывается в таблицу ConfigSave, потом её применяет платформа.

```
ibcmd-rs mssql-stage-source-objects --database MyBase --sqlcmd "%SQLCMD%" ^
  --source-root C:\export\MyBase --source-version 2.20 ^
  --replace-config-save --allow-non-lab

ibcmd infobase config apply --dbms=MSSQLServer --db-server=localhost ^
  --db-name=MyBase --force
```

`--allow-non-lab` — обязательное подтверждение, что команда будет писать в
базу. `--script-only` только подготовит данные и SQL, ничего не записывая.
Штатному `ibcmd` при входе SQL Server добавьте `--db-user` и `--db-pwd`.

## Загрузить XML в пустую базу

```
ibcmd infobase create --dbms=MSSQLServer --db-server=localhost ^
  --db-name=NewBase --create-database

ibcmd-rs mssql-stage-source-objects --database NewBase --sqlcmd "%SQLCMD%" ^
  --source-root C:\export\MyBase --source-version 2.20 ^
  --replace-config-save --allow-non-lab --base-free

ibcmd infobase config apply --dbms=MSSQLServer --db-server=localhost ^
  --db-name=NewBase --force
```

`--base-free` собирает все строки конфигурации только из XML, ничего не
читая из целевой базы.

## Сравнить две выгрузки

```
ibcmd-rs source-diff -o diff.json C:\export\native C:\export\ours
```

В `diff.json` будут итоги (`unchanged`, `different`, `left_only`,
`right_only`) и список расхождений по файлам.

## Ограничения

- Только Microsoft SQL Server.
- Создание базы и применение конфигурации (`config apply`) выполняет
  штатная платформа 1С.
- Проверено на БСП и ERP УХ, платформы 8.3.27.2214 и 8.5.1.1150. Другие
  конфигурации и версии платформы могут дать расхождения.
- При загрузке ERP УХ в пустую базу в `ConfigDumpInfo.xml` нет 145 пустых
  служебных записей: справки и предопределённых данных без содержимого.
  На остальные файлы XML это не влияет.
- Остальные команды (`ibcmd-rs --help`: расширения, CF, аудит) —
  исследовательские и проверены меньше.

## Подробнее

- [docs/HISTORY.md](docs/HISTORY.md) — история разработки, прежний README и
  справочник всех команд.
- [scripts/empty-load](scripts/empty-load) — скрипты проверки загрузки в пустую
  базу; [scripts/timing](scripts/timing) — замеры против штатного `ibcmd`.
- [openspec/changes](openspec/changes) — спецификации и результаты проверок.
