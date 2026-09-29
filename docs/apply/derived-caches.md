# The derived caches of a new object (S1-G, issue #403)

Part of S1 (#391, `docs/apply/restructuring.md` 12.5 and 12.7). When a configuration change adds an object or
a tabular section the platform rewrites the `Params` `*.si` rows. This is what each row holds, what a new
catalog, a new document and a new tabular section change in it, and how `src/restructure/caches/` reproduces
it. Everything here is measured on БСП 8.3.27 (compatibility 8.3.27):

| name | database / label | what it is |
|---|---|---|
| `pristine` | `ddl_bsp8327_a` / `a2_staged` | the caches before cases c and h |
| `c2` | `ddl_bsp8327_c2` / `c2_now` | native after case c: a new catalog `ДемоНовыйСправочник` |
| `m` | `ddl_bsp8327_m` / `m_now` | native after case h: a new tabular section `ДемоНоваяТЧ` of `_ДемоКонтрагенты` |
| `t1_before`, `t1_nat` | `ddl_s1_base` / `t1_staged`, `ddl_s1_t1_nat` / `nat_after` | the types case (attributes only) |
| `d_staged`, `d_after` | `trace_d_base` / `d_staged`, `d_after` | case d: a new document `ДемоНовыйДокумент` with an attribute and a tabular section, cloned from `_ДемоОприходованиеТоваров`, staged with the native partial import on a fresh БСП clone and applied natively (kit `scripts/apply-trace/lab/s1g-caches/`) |

`cargo test --lib restructure::caches` runs the checks below against them (they skip without the lab; the
`d_*` snapshots come from `IBCMD_RS_TRACE_LAB`, default `F:\ibcmd\lab\05\s1g\store`).

## 1. The order is a hash map's

Six of the seven rows are dumps of in-memory hash maps, and to be byte-exact a row has to repeat the
**iteration order** of the map. Measured on all of them, the map behaves like a Microsoft
`std::unordered_map` keyed by the uuid with `Data1` (the first 8 hex digits) as the hash:

- the elements are one list; the elements of a bucket are contiguous;
- the table starts with 8 buckets and grows after an insertion that leaves more elements than buckets:
  x8 while below 512 buckets, x2 from there (8, 64, 512, 1024, 2048, 4096);
- an element of an **empty** bucket goes to the **end** of the list, an element of a bucket that has elements
  goes to the **front** of that bucket's run;
- growing walks the list and regroups it by the new mask: a bucket's run stands where its first element
  stood and is reversed inside (each later element goes to the front).

The order is a function of the insertion order of the keys, so adding **one** key early shifts every growth
point after it; that is why a new object moves a few unrelated entries (case c: `28e59c50` next to the new
catalog and `d6704410`, in four rows). `caches::order` implements the model; the tests reproduce **every**
root section of `2203278d` (21 sections, 3 snapshots), the tabular-section sections of catalogs (111 and 112
entries) and of documents (48), `42ed49cc` (114 and 115 catalogs), `facbfffe` (sections 1-6, both snapshots) and,
in a scratch experiment, `c77bc206`, byte for byte.

**Insertion orders found.**

| container | insertion order |
|---|---|
| a `2203278d` section of a kind the root lists | the root's collection of the kind, in the root's order |
| `2203278d`, tabular sections (`932159f9-...` of catalogs, `21c53e09-...` of documents) | the objects of the kind in the root's order, each one's sections in the order of its descriptor |
| `42ed49cc`, catalogs | the root's catalog collection |
| `facbfffe`, a section | the kinds in the platform's order (`ExchangePlan`, `Constant`, `Catalog`, `Document`, ... `synonyms::CLASS_ORDER`), each in the root's order |
| `a07b62f0` | not a hash map: metadata order, kind by kind, each in the root's order |
| `fe8acd6a` | not a hash map: sets sorted as text |
| `c77bc206` (unchanged by all cases) | the uuids sorted as text in the `pristine` clone; in `s1_base` and in a fresh restore of the corpus (`d_staged`) it is another order, and a native apply leaves either text as it is |
| `c4629235` | **not found** (see 4) |

The kind order of `facbfffe` is a partial order (`ExchangePlan < Catalog < Document < DocumentJournal <
InformationRegister < ChartOfAccounts`, `Constant < Report < DataProcessor`, `ChartOfCharacteristicTypes <
ChartOfAccounts`); it is exact for sections 1-6 of the corpus; sections 0 and 7 (7 and 65 entries; they have
keys that the root does not list) are refused.

## 2. The rows and what a new object does

| row | holds | new catalog (c) / new document (d) | equal to native |
|---|---|---|---|
| `2203278d` | per kind: object -> generated types `(TypeId, ValueId, index)`; the index numbers the *category* (`Object` 0, `Ref` 1, `Selection` 2, `List` 3, `Manager` 4), not the position | one entry (5 types) in the section of the kind, the section refilled in the root's order; the object's tabular sections join the section of their class | yes (c, d) |
| `a07b62f0` | `"Catalog.X","Справочник.X",<uuid>,1,0,"Reference11036",11036,<Ref TypeId>` (documents: `"Document.X","Документ.X",...,"Document11034",...`; then a second list of change tables) | one entry after the entry of the previous object of the kind; the count | yes (c, d) |
| `42ed49cc` | three maps; the one whose keys are the catalogs: `<catalog>,<n>,<owner>...` | catalog: `<uuid>,0` (or its owners), section refilled. **A document changes nothing in this row** (d: text equal before and after) | yes (c); n/a (d) |
| `facbfffe` | eight maps object -> one localized string of one property (1 `ListPresentation`, 2 `ExtendedListPresentation`, 3 `ObjectPresentation`, 4 `ExtendedObjectPresentation`, 7 `Explanation`) | an entry per non-empty property; **the object presentation, not the synonym** (case c: the catalog was copied from `Удалить_ДемоОбщиеСведения` and kept its presentation) | yes (c: section 3; d: sections 1 and 3) |
| `fe8acd6a` | named sets of type ids, sorted; `e61ef7b8` = every catalog `Ref`, `e2cb8e3e` = every catalog `Object`, `38bfd075`/`f72bc2d7` the same for documents, `280f5f0e` = every reference type | the `Ref` id into two sets, the `Object` id into one (`type_sets::FAMILIES`, checked against the corpus) | yes (c, d) |
| `c4629235` | one map over the objects and their nested elements (2376 keys, 4096 buckets). Catalog: `2 EditType`, `5 Hierarchical`, `6 HierarchyType`, `21 SubordinationUse`, `10 QuickChoice=1`, `0` a help reference when a `Config` row `<uuid>.1` exists, `3` the `Ref` type, `24` type sets that mention the object. Document: `19 Posting`, `20 RealTimePosting`, `0`, `3`, `24` | one entry, exact for all 115 catalogs of `c2`, all 25 documents and the new one of `d` (property 24 aside: it is not made of the descriptor) | **entry yes, position no** |
| `ea13a2c9` | the XDTO model | `CatalogRef.X` / `DocumentRef.X` after the previous object's; the block (row types of its sections, then `CatalogObject.X` / `DocumentObject.X` with the standard properties) after the previous object's block | yes (c: native's file has more lines from the case's other edits, every line we insert is in native's file at the same place; d: the insertion is native's exactly) |

The standard properties of `CatalogObject.X` are `IsFolder` (hierarchical, folders and items), `Ref`,
`DeletionMark`, `Owner` (`AnyIBRef`, nillable), `Parent` (hierarchical), `Code` (`xs:decimal` for a numeric
code), `Description`, `PredefinedDataName`; those of `DocumentObject.X` are `Ref`, `DeletionMark`, `Date`,
`Number` (`xs:string`), `Posted`; both are checked against all objects of the corpus (114 catalogs, 25 documents).

## 3. A new tabular section (case h)

Only two of my rows change: `2203278d` (the section of the tabular-section class: an entry
`<section>,2,<type>,<value>,0,<row type>,<row value>,1`, refilled in the traversal order of 1) and the XDTO
model (`CatalogTabularSectionRow.X.T` / `DocumentTabularSectionRow.X.T` right before the object type or after
the previous section's row type, and the property `T` in the object type, `lowerBound="0"
upperBound="99999"`). Both equal native's for catalogs (h); for documents the new section of case d is part of
the new document and equals native's too.

## 4. `c4629235`: proven acceptable, not byte-exact

The map is filled in an order that is neither the root's nor the registry's (`1a621f0f`) nor a finer hash
order (tested), and its tail interleaves the nested elements of documents, data processors and registers (the
insertion looks like "an object and the objects it refers to", subsystems in root order between the register and
the common-form passes). A new object early in its collection moves **the same** few other entries whatever
it is: in both cases c and d the entries `0aba89f9`, `778cd9f9`, `02a84df9` (near position 731) and `d74ccadb`
(from 1946 to 2188) stand elsewhere, plus one entry next to the new one (`28e59c50` in c, `36810e6e` in d) --
the growth points of the table shift by one. With the approximate placement (`HelpProps::add_entry_approximately`:
where a last-inserted key goes) 10 of the 2375 neighbour links of native's row are broken and
`CacheRow::exact` is `false`.

**The platform accepts the row.** Two twins of the native state after case c (`ddl_bsp8327_c2_native_after.bak`);
in one the row `c4629235` is replaced by ours with a new `siVersions` guid, the other is untouched
(`scripts/apply-trace/lab/s1g-caches/prove.ps1`, results in `docs/apply/evidence/derived-caches/`):

| step | untouched twin | twin with our `c4629235` |
|---|---|---|
| stand-alone server (`ibsrv`) starts, thin client runs the new-catalog probe (metadata, query, write, type of the reference, XDTO of the reference and the object, query by the reference) | ready in 9 s, 10 of 10 lines | ready in 6 s, the same 10 lines |
| the same probe in a session of the 8.3.27 cluster | the same 10 lines | the same 10 lines |
| native `infobase config check` | exit 0, «Проверка корректности метаданных успешно завершена» | the same |
| native `infobase config apply` afterwards | exit 0, «Обновление конфигурации базы данных не требуется» | the same |

Neither of these operations reads the row in a way that shows (12.5 measured the same for a stale and a
deleted `c4629235`), so this proves that our row does no harm, not that it is used; the next structural native
apply rewrites all sixteen rows anyway. Twin check 6 ("16 of 16 rows have the same text") should therefore
tolerate the order of the entries of this row, like the other drift (see 6).

## 5. What is left

- **`c4629235` exact order** (see 4). The moved entries are the same in c and d, which suggests the changes of
  a new object are a function of the trigger elements of the growth (ranks 9, 65, 513, 1025, 2049 of the
  insertion order) that could be recovered; not done.
- **`facbfffe` section 7** (`Explanation`) and the sections 0, 5, 6 (constants, registers) are refused.
- **Kinds other than catalogs and documents** are refused (`Enum` and `ExchangePlan` on purpose).
- The **necessity table** of 12.5 is not repeated on the result (twin protocol 12.6 checks 6 and 9 are the
  first users).

## 6. Proposed text for the drift list of 12.6

Add to the platform's own derived state that a twin comparison tolerates:

- `Params` `facbfffe`: section 3 holds the **object presentation** (the second localized string of the row),
  not the synonym; a twin whose new object was cloned from another keeps the source's presentation. Compare the
  rows, not the synonyms.
- `Params` `c77bc206`: the text is equal before and after a native apply in the same database but differs
  between databases of the same configuration (a fresh restore of the corpus, `s1_base`, the `pristine` clone):
  the order of its hash sets depends on the database. Compare as sets, or skip in check 6.
- `Params` `c4629235`: our rows carry every entry exactly but not the position of about five entries of 2376
  (see 4); the platform accepts the row (server start, cluster session, `config check`, apply «не требуется»).

## 7. API (`src/restructure/caches/plan.rs`)

```rust
new_object(&NewObject { kind /* "Catalog" | "Document" */, descriptor, root, table_number, has_help,
                        xdto_attribute_lines, xdto_section_blocks, cache, name_of, descriptors })
    -> Result<Vec<CacheRow>>        // 2203278d a07b62f0 [42ed49cc: catalogs] facbfffe fe8acd6a c4629235 ea13a2c9
new_tabular_section(&NewSection { kind, root, descriptor, owner, section, xdto_row_lines, cache })
    -> Result<Vec<CacheRow>>        // 2203278d ea13a2c9
```

`cache(name)` gives the inflated text of a `Params` row, `root` is the root row **after** the change,
`descriptor(uuid)` / `descriptors(uuid)` the descriptor rows after the change (the latter is read only when the
new object has tabular sections). `xdto_attribute_lines` / `xdto_section_blocks` are built with
`xdto_types::{primitive_property, reference_property, section_property_line, section_row_block}`. The result
rows are inflated text; deflating, the `siVersions` guids and the guarded rewrite (`ParamsRewrite`) are the
apply's.
