# The derived caches of a new object (S1-G, issue #403)

Part of S1 (#391, `docs/apply/restructuring.md` 12.5 and 12.7). When a configuration change adds an object or
a tabular section the platform rewrites the `Params` `*.si` rows. This is what each row holds, what a new
catalog / a new tabular section changes in it, and how `src/restructure/caches/` reproduces it. Everything
here is measured on the lab corpora of the `ddl` track (BSP 8.3.27, compatibility 8.3.27):

| name | database / label | what it is |
|---|---|---|
| `pristine` | `bsp8327_a` / `a2_staged` | the caches before cases c and h |
| `c2` | `bsp8327_c2` / `c2_now` | native after case c: a new catalog `ДемоНовыйСправочник` |
| `m` | `bsp8327_m` / `m_now` | native after case h: a new tabular section `ДемоНоваяТЧ` of `_ДемоКонтрагенты` |
| `t1_before`, `t1_nat` | `s1_base` / `t1_staged`, `s1_t1_nat` / `nat_after` | the types case (attributes only) |

`cargo test --lib restructure::caches` runs the checks below against them (they skip without the lab).

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
root section of `2203278d` (21 sections, 3 snapshots), the tabular-section section (111 and 112 entries),
`42ed49cc` (114 and 115 catalogs), `facbfffe` (sections 1-6, both snapshots) and, in a scratch experiment, `c77bc206`, byte for byte.

**Insertion orders found.**

| container | insertion order |
|---|---|
| a `2203278d` section of a kind the root lists | the root's collection of the kind, in the root's order |
| `2203278d`, tabular sections `932159f9-...` | the catalogs in the root's order, each one's sections in the order of its descriptor |
| `42ed49cc`, catalogs | the root's catalog collection |
| `facbfffe`, a section | the kinds in the platform's order (`ExchangePlan`, `Constant`, `Catalog`, `Document`, ... `synonyms::CLASS_ORDER`), each in the root's order |
| `a07b62f0` | not a hash map: metadata order, kind by kind, each in the root's order |
| `fe8acd6a` | not a hash map: sets sorted as text |
| `c77bc206` (unchanged by all cases) | the uuids sorted as text **in a freshly restored database**; in `s1_base` it is another order (native reads `Config` in the physical order of the pages?) |
| `c4629235` | **not found** (see 4) |

The kind order of `facbfffe` is a partial order (`ExchangePlan < Catalog < Document < DocumentJournal <
InformationRegister < ChartOfAccounts`, `Constant < Report < DataProcessor`, `ChartOfCharacteristicTypes <
ChartOfAccounts`); it is exact for sections 1-6 of the corpus; sections 0 and 7 (7 and 65 entries; they have keys that the root does not list) are refused.

## 2. The rows and what a new catalog does

| row | holds | new catalog | equal to native |
|---|---|---|---|
| `2203278d` | per kind: object -> generated types `(TypeId, ValueId, index)`; the index numbers the *category* (`Object` 0, `Ref` 1, `Selection` 2, `List` 3, `Manager` 4), not the position | one entry (5 types) in the catalog section, section refilled in the root's order | yes |
| `a07b62f0` | `"Catalog.X","Справочник.X",<uuid>,1,0,"Reference11036",11036,<Ref TypeId>` (then a second list of change tables) | one entry after the entry of the previous catalog; the count | yes |
| `42ed49cc` | three maps; the one whose keys are the catalogs: `<catalog>,<n>,<owner>...` | `<uuid>,0` (or its owners); section refilled | yes |
| `facbfffe` | eight maps object -> one localized string of one property (1 `ListPresentation`, 2 `ExtendedListPresentation`, 3 `ObjectPresentation`, 4 `ExtendedObjectPresentation`, 7 `Explanation`) | an entry per non-empty property; **the object presentation, not the synonym** (case c: the catalog was copied from `Удалить_ДемоОбщиеСведения` and kept its presentation) | yes (section 3) |
| `fe8acd6a` | named sets of type ids, sorted; `e61ef7b8` = every catalog `Ref`, `e2cb8e3e` = every catalog `Object`, `280f5f0e` = every reference type | the `Ref` id into two sets, the `Object` id into one (`type_sets::FAMILIES`, checked against the corpus) | yes |
| `c4629235` | one map over the objects and their nested elements (2376 keys, 4096 buckets): `2 EditType`, `5 Hierarchical`, `6 HierarchyType`, `21 SubordinationUse`, `10 QuickChoice=1`, `0` a help reference when a `Config` row `<uuid>.1` exists, `3` the `Ref` type, `24` type sets that mention the object | one entry, exact for all 115 catalogs of `c2` (property 24 aside: it is not made of the descriptor) | **entry yes, position no** |
| `ea13a2c9` | the XDTO model | `CatalogRef.X` after the previous catalog's; the block (row types of its sections, then `CatalogObject.X` with the standard properties) after the previous catalog's block | yes (native's file has more lines from the case's other edits: every line we insert is in native's file at the same place) |

The standard properties of `CatalogObject.X` are `IsFolder` (hierarchical, folders and items), `Ref`,
`DeletionMark`, `Owner` (`AnyIBRef`, nillable), `Parent` (hierarchical), `Code` (`xs:decimal` for a numeric
code), `Description`, `PredefinedDataName`; checked against all 114 catalogs of the corpus.

## 3. A new tabular section (case h)

Only two of my rows change: `2203278d` (the tabular-section section: an entry `<section>,2,<type>,<value>,0,
<row type>,<row value>,1`, refilled in the traversal order of 1) and the XDTO model
(`CatalogTabularSectionRow.X.T` right before `CatalogObject.X` or after the previous section's row type, and
the property `T` in `CatalogObject.X`, `lowerBound="0" upperBound="99999"`). Both equal native's.

## 4. What is left

- **`c4629235`: the position of five other entries.** The map is filled in an order that is neither the root's
  nor the registry's (`1a621f0f`) nor a finer hash order (tested); its tail interleaves the nested elements
  of documents, data processors and registers. A new catalog at the root position 39 moves five other entries of the 2377
  (four places: the growth points shift by one); with the approximate placement 10 of the 2375 neighbour
  links of native's row are broken (the test prints it). The entry is exact; `HelpProps::add_entry_approximately` puts it
  where a last-inserted key would go and `CacheRow::exact` is `false`. The platform tolerates a stale
  `c4629235` (12.5) and rewrites it at the next native apply.
- **`facbfffe` section 7** (`Explanation`) and the sections 0, 5, 6 (constants, registers) are refused.
- **Kinds other than catalogs** are refused by `a07b62f0`/`fe8acd6a` where the corpus has not measured them,
  but the tables (`kind_names`, `FAMILIES`) already list the reference kinds that have one entry per object.
- **`c77bc206`** is not touched by any case measured; its order depends on the database it was built in.
- The **necessity table** of 12.5 is not repeated on the result (twin protocol 12.6 checks 6 and 9 are the
  first users).

## 5. API (`src/restructure/caches/plan.rs`)

```rust
new_catalog(&NewCatalog { descriptor, root, table_number, has_help,
                          xdto_attribute_lines, xdto_section_blocks, cache, name_of })
    -> Result<Vec<CacheRow>>        // 2203278d a07b62f0 42ed49cc facbfffe fe8acd6a c4629235 ea13a2c9
new_tabular_section(&NewSection { root, descriptor, catalog, section, xdto_row_lines, cache })
    -> Result<Vec<CacheRow>>        // 2203278d ea13a2c9
```

`cache(name)` gives the inflated text of a `Params` row, `root` is the root row **after** the change,
`descriptor(uuid)` the descriptor rows after the change. The result rows are inflated text; deflating, the
`siVersions` guids and the guarded rewrite (`ParamsRewrite`) are the apply's.
