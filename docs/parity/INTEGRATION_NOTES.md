# GridCraft Excel-parity integration

Base: Neil's `origin/integrated` at `2e8dc12384775b61675a918863c79a632773e9cc`.

| PR | Original squash | Integration commit |
| --- | --- | --- |
| X01 | `9127227d` | `ad7dc33a2c98c1b420ef3652ba6b99a68e1d6e4e` |
| X02 | `0aa0af00` | `573837efb50773cef1246598613884d5ba97aad2` |
| X06 | `2237e688` | `e48e070f4b73eabb2dc15c28f119b05641b3ed4e` |
| X08 | `d60d2996` | `16628e184833de00f1bae7c6e17927e18a1080ce` |

## X01 — XLSX pass-through and rich metadata

- `crates/xlsx/src/{read,sheet_read,sheet_write,write}.rs`: the base used typed `craft-xlsx` cells and parts; X01 added phonetics, rich errors and value metadata, custom views and properties, and ordered external links. Port those features to the typed path so x14 formatting, slicers, pivots, external caches, spill and Python formulas, and VML note ordering continue to use the base codec.
- `crates/xlsx/src/{package,passthrough}.rs`: retain typed OPC relationship resolution with the PR's deny-list. For reads through GridCraft's `ZipParts`, declared-empty ZIP members need no inflater; successful reads must match the declared size and charge decoded bytes. ZIP failures, including size mismatches, spend the attempted read allowance, capped at the remaining package budget. Once that budget is gone, non-empty parts are rejected before inflation. Keep table-child bindings for unmodelled sidecars without duplicating model-owned relationships and parts.
- `crates/xlsx/src/read.rs`: retain the incremental sheet-name allocator and Excel's acceptance of spaces-only names.
- Release the source shared strings and per-index phonetics immediately after worksheet import unless a retained macro sheet needs them. Chart and dialog capture do not read shared-string indexes; macro capture resolves its indexes before releasing the tables.

### Known limitation: typed worksheet ZIP reads

GridCraft's `ZipParts` check does not cover the separate `craft-xlsx` package reader at `a20dfd16`. In XLSX that reader opens root relationships and reads worksheet XML and its relationships, tables, comments and note VML. XLSB detection and import also use the dependency reader, including for workbook metadata, worksheets and table indexing. The parser streams rows from a fully decoded byte buffer, not directly from the ZIP decoder. The bounded ODS mimetype probe is separate and is not a worksheet extraction path.

`craft_zip::Archive::read` uses `craft_zip::preflight::read_entry`. It rejects decoder errors and output exceeding the directory declaration, but accepts a short successful decode, including zero output. `craft_opc::LazyPackage` charges decoded bytes, not compressed input: a successful empty decode costs zero, while a decoder error exhausts its separate budget. A short worksheet can reach the model without a ZIP diagnostic. The typed reader does not memoize part reads, so reads per import are bounded by sheet and table limits rather than unique parts.

The shared reader in `craft-libs` should skip the decoder for declared-empty entries, treat a short non-empty decode as a ZIP error, and charge it the failed-read allowance. GridCraft and the typed reader should use the same checked part reader and budget. Rejecting or memoizing reads of aliased or overlapping members is a low-cost partial mitigation for repeated reads, not a substitute for validating the decoded size.

## X02 — ODS/FODS imports

- `crates/xlsx/src/{ods,package}.rs`: the base's data-only adapter cannot carry X02 formulas, names, styles, or FODS. Use one rich parser, retaining exact bounded compressed mimetype detection, ZIP entry preflight, XML namespace bounds, and the decompression limits. Share a ZIP index between detection and bounded ODS reads without opening a typed XLSX package for ODS.
- `crates/xlsx/src/{ods,import_rules}.rs` and ODS tests: use the incremental sheet-name allocator with the incoming apostrophe cleanup and `History` reservation. Ignore foreign-namespace tables, clip rows and columns beyond the Excel grid with a warning, enforce in-grid expansion limits, and use typed cached errors. Retain malformed-package and resource-limit regressions as well as format and formula fixtures. The typed XLSB reader stays in place; the X02 ODS name-repair helper is not shared by that reader in this integration.
- `crates/engine/src/cmd/insert.rs`: retain the base's split command layout and expanded invalid-table-name test; both commands call the shared `gridcraft_core::valid_table_name`.

## X06 — Calculation locale

- `crates/core/src/value.rs`: add X06's printable ASCII weights to both the base's ASCII fast path and its Unicode primary/final collation passes; retain Arabic marks and digit-script weights at the secondary level. Keep one copy of the shared numeric-boundary tests, plus the incoming unique assertions. Localize numeric coercion at the text boundary, keeping formula literals invariant.
- `crates/numfmt/src/{lib,decimal,render}.rs`: separate width-bounded cell General from full export General and worksheet `TEXT` General. Keep the base's shared rendering and displayed-precision calculation; `crates/engine/src/io.rs` now uses the existing constant-rounding sweep when ODS requests precision as shown, so constants and formula results share rounding rules.

## X08 — Producer-specific workbook audit

- `apps/gridcraft-cli/src/check.rs`: layer producer classification, volatile-value pinning, and locale retries on the in-place oracle and compact round-trip snapshot. Keep the pivot indexer for `GETPIVOTDATA` and the directory walk's inode/depth handling while expanding directory scans to all six supported workbook extensions.
- `apps/gridcraft-cli/src/check/{fidelity,package,rels}.rs`: retain one bounded, owner-aware relationship comparison. The calculation chain is the only unconditional kind exemption. Missing pivot cache records are excused only for a surviving saved cache owner whose definitions explicitly request refresh on load. Owner filters and count checks still apply.
- `apps/gridcraft-cli/src/check/calc.rs`: trim cached error literals and recognize `#N/A!` as `#N/A`. Restore bare-literal matching for `Other` and `Unknown` producers, counting those matches in `errorDetailCache`; only Google Sheets can use the colon-delimited reason. Excel and other specifically identified producers still compare text as text, an intentional difference from the base's producer-independent grading.
- `crates/calc/src/{eval,recalc}.rs` and their child modules: add the legacy one-cell `SUMPRODUCT` reading to the existing evaluation host, retaining its active-cell, spill/table occupancy, and linked-workbook features. The alternate reading is for checking older Excel caches, not normal recalculation.
- `crates/xlsx/src/lib.rs`: implement the root-stream API for XLS producer metadata on the base's bounded `craft-cfb` reader; do not restore the deleted CFB parser. Retain the generated case-insensitive root-stream regression.

## X09 — XLSX package round-trip

X09b checks both definition identity and the range native refresh resolves. Save tries the original name, the bare name, then a sheet-qualified name before falling back to an explicit range. An unchanged cache can retain its snapshot even if its bound name now points to different cells; the next refresh uses the current definition. An unresolved imported name cannot silently acquire a source. External caches are kept only, never rebuilt from local cells.

Kept worksheet sources retain their bytes when the original attributes still bind correctly. Rebuilt local sources write only supported unqualified attributes. Cache IDs are assigned across kept and rebuilt caches; the x14 cache ID is written only when imported or needed by a slicer. Slicer targets are checked against the definitions that will be written before any slicer or cache parts are emitted.

Pivot slicer caches reuse an imported cache name only when the name is free or still belongs to that cache's original marker. A marker is owned only while no other workbook-scoped name or table shares its name; a sheet-scoped name of the same spelling does not take it away, but does keep a new cache from using a free name. Imported markers are written as ordinary defined names, retaining their attributes even if the cache is pruned; only caches without an existing marker synthesize a name. A copied sheet's slicers drop the import provenance, so the original keeps its cache name.

## X17 — XLSX table name collisions

X17 repairs table names on XLSX open and before save. A [table's `displayName`](https://learn.microsoft.com/en-us/dotnet/api/documentformat.openxml.spreadsheet.table?view=openxml-3.0.1#remarks) must be unique among table and defined names (ECMA-376 Part 1 §18.5.1.2). Defined names of either scope take precedence, including print areas and titles, kept definitions and hidden storage names. Otherwise the first valid table in sheet order, then table order, keeps its spelling. Each rename is reported with the sheet and old and new table names; reopening the repaired file does not repeat it.

GridCraft refuses a case-insensitive table name collision in Define Name and Name Manager rename. A name that already has a table's spelling (from an earlier GridCraft version, which allowed it) can still be edited or defined over; only a new spelling is refused. Create from Selection reports each colliding label as skipped, even when replacing defined names. Solver's fixed names and the stash's fixed chunk spellings are unchanged: the XLSX repair renames a colliding table rather than its hidden definition.

An ambiguous reference follows the first original table with that spelling. Unqualified references in a table's column formulas and in its data or totals cells can instead identify that table by its original name; header cells and overlapping tables do not establish a cell owner. A reference to no table that is spelled like a repaired name becomes the error it evaluates to (`#REF!` for a structured reference, `#NAME?` for a bare name), so it does not start naming that table on reopen. File table IDs identify table slicers; X09b's bound pivot definition identities are retained. Literal text (including chart series titles and `INDIRECT` arguments) is not rewritten as a table reference. Unparseable formula references are not inferred from text.

Save warnings reach the app's info bar on desktop path saves (including AutoSave), host saves, and Backstage exports, once the file is written or handed over: a failed host save shows only its error. The info bar uses the same summary as open; the engine's save result carries the complete warning list. A save without warnings leaves any existing notice alone. Save repairs the written package, not the live workbook, so saving the same malformed workbook again repeats its rename warnings. A save the user asks for always shows them; AutoSave shows them only when they differ from the warnings last shown for that open workbook at its path, so a dismissed notice stays dismissed. Saving to another path, closing or reopening starts afresh.

An in-workbook hyperlink target has at most one leading `#` marker: `#Data!A1` names `Data`, but `##Data!A1` names the distinct sheet `#Data`, with no fallback to `Data`. Instead of discarding extra markers, XLSX saves these sheet locations in Excel's quoted spelling (`'#Data'!A1`) and preserves opaque targets such as `##Something` without following them. Both `#NAME?` and `##NAME?` still canonicalize to the broken reference `#NAME?`.

A bare token in a workbook-scoped defined-name body can depend on the caller's sheet. If a sheet-local name shadows that token, X17 leaves it alone rather than forcing all callers onto the table; when the table is renamed because of that local name, the token no longer reaches the table from other sheets. Use an explicit structured reference to identify the table in a caller-independent way.

Kept package content GridCraft does not parse (connections and the data model, kept chart and macro sheets' own parts) is not rewritten; only kept sheets' defined names are.

Table slicers follow a repaired table on open and on save, and are written since X19.

See Microsoft's [table naming rules](https://support.microsoft.com/en-us/excel/rename-an-excel-table) and [structure reference grammar](https://learn.microsoft.com/en-us/openspecs/office_standards/ms-oe376/bcd72180-31a3-423b-8f83-d224b2286da3). A scan of conventional table parts found no collisions in 1,122 public table workbooks (33,370 eligible XLSX-family rows; 19 unreadable). A second scan of 2,125 public packages with table parts (307 of them with sheet-scoped names) found no table sharing a name with a defined name of either scope. The repair is tested with synthetic packages; how Excel repairs such malformed packages remains unmeasured.

## X19 — XLSX table slicers

Table slicers are written on save instead of being dropped. The package follows Excel's own files (public corpus: the 118-8 SpreadsheetBench workbooks) and [MS-XLSX §2.6.97 `CT_TableSlicerCache`](https://learn.microsoft.com/en-us/openspecs/office_standards/ms-xlsx/c3629be2-99fe-44b7-970c-3ea6df625cf5): a cache with `name`, `sourceName` (the written column header) and an extension `{2F2917AC-EB37-4324-AD4E-5DD8C200BD13}` with `x15:tableSlicerCache`. The workbook lists it in `x15:slicerCaches`, extension `{46BE6895-7355-4a93-B00E-2C351335B9C9}`. The sheet lists its table slicer part in `x14:slicerList`, extension `{3A4CF648-6AED-40f4-86FF-DC5316D8AED3}`. The drawing frame's compatibility choice requires `sle15`, and the cache has a workbook-scoped `#N/A` defined name. The selection is the table's AutoFilter. The cache uses the actual serialized table and column ids, recorded while writing the table.

Table and PivotTable slicers go through one planner, so X09's name-ownership rules apply to both: slicer names are unique across kinds, an imported cache keeps its name only while it is free or still owned by its original marker, and new caches are `Slicer_<column>`. An imported table cache also keeps its item options (`sortOrder`, `customListSort`, `crossFilter`), which the model doesn't edit; a copied sheet's slicers drop them with the rest of the import provenance.

Table slicers follow table and column renames, including X17's save-time table name repair. Removing a table or slicer column removes its slicers on every sheet in the same undoable edit. Saving defensively drops slicers whose source table is not written, has no data rows, or lacks the column, and reports a warning. Dropped slicers leave no cache, part, list entry or frame; an imported marker survives as an ordinary workbook name, as for pivot slicers. No new marker is generated for a dropped slicer.

Reading: one reader finds slicer parts and caches through their relationships, even if the workbook or worksheet listing is missing. After X17's repair a table slicer resolves by file table and column id. Within a resolved table, `sourceName` can recover a column id that is missing, dangling (no column of the table has it) or malformed. Without a table id, a header must match exactly one table; a dangling table id is not rebound to another table. Unresolved slicers and frames without slicer parts warn and are skipped. Table slicers retain their name, style, button height and cache provenance; captions are `ST_Xstring`-decoded and encoded for both kinds. OLAP slicers are still skipped with a warning.

Baseline failures: `cross_sheet_table_slicer_imports_without_skipped_object_warning` was this gap (the PivotTable reader warned about the table slicer's frame) and passes. `pivot_slicer_round_trips` and `pivot_slicer_keeps_every_connected_pivot_table` are not: they compare a PivotTable filter's selected list in order, and the fixture's custom Product item order (`C, A, B`, from `ce8f5fca`) reads the hidden items back as `["C", "A"]` for `["A", "C"]`. The selection is the same set; the fix belongs to the PivotTable reader or the test, not to slicers.

## X23 — Sheet copy keeps tables

Move or Copy > Create a copy used to drop the copy's tables. The copy's structured references then named the original's tables, and so did its table slicers. Now the copy has its own tables, in one undoable edit. Each copied table keeps its range, style, options, totals row, columns and filter. It gets a fresh object id and its own identity, so a PivotTable bound to the original stays bound to the original.

Two public Excel-saved files show copy names ending in one more than the saved table id. For example, `Table6` became `Table69` (id 8) and `Table610` (id 9). GridCraft does not retain Excel's table ids; it writes sequential ids in sheet order. With `N` existing tables, its `k`th copy candidate starts at `N + k + 1`. It skips names taken by tables, defined names of any scope (including retained sheets), or imported slicer caches, case-insensitively.

Measured, Excel for Mac 16.115.2 (synthetic action probes):

| Probe | Excel readback | GridCraft behavior |
| --- | --- | --- |
| `x23-p1-copies` | With three original tables, the two tables on `Data` copy as `Sales5`, `Keys6`, then `Sales7`, `Keys8`. Copying the first copy gives `Sales59`, `Keys610`. Structured cell references follow the copies; `ROWS(Sales)` stays unchanged. References on the source and other sheets and in the workbook-scoped name stay unchanged. `[@Qty]*2` and `SUBTOTAL(109,[Qty])` keep their table-local spelling. Excel canonicalizes entered `SUM(Data!Sales[Qty])` to `SUM(Sales[Qty])` **before** the copy. | Same table names and structured-only cell retargeting. The test uses the canonical H4 formula; no change to GridCraft's qualified formula entry syntax. |
| `x23-p2-name-taken` | One table `Sales` and workbook name `Sales3`: the copy gets `Sales4`. | Same; the defined name reserves the first candidate. |
| `x23-p3-table-r` | Source `name=R`, `display_name=R_`; copy `R__3` in both properties. Formula changes from `SUM(R_[Qty])` to `SUM(R__3[Qty])`. | The table model stores the display spelling. Sheet copy of modeled `R_` inserts a separator to produce `R__3`; table rename and validation are unchanged. |
| `x23-p4-table-255` | `T` + 254 `x` becomes `T` + 252 `x` + `3` (254 characters); the copied formula uses that exact name. | Same; when the base plus suffix would exceed 255 characters, trim the base so the result fits 254. |

P1 also measured a defined-name difference. Excel's first copy creates local `Local` and a local `Global` that shadows the workbook name, both referring to `Sales5[Qty]`; the second copy from `Data` does the equivalent for `Sales7`. On the copy of the first copy, both locals still refer to `Sales5[Qty]`, even though the copied cells now use `Sales59[Qty]`. X35 adds the sheet-level copies of workbook names and Excel's tab name for a copy of a copy; the copy-of-copy binding is still open there.

X27 (`parity/X27-table-name-scopes`, in progress) adds a shared free-name check; sheet copy should switch to it when both lanes merge.

GridCraft retargets explicit structured references in the copied sheet's cells, calculated columns, custom totals, sheet-scoped names, conditional formatting, data validation, and charts. The copy's table slicers follow its copied tables too (`Workbook::retarget_table_on_sheet`). Table-local structured references with no table token need no rewrite; bare table-name formulas are left alone. The public sample also shows an explicit totals-row reference retargeted to `SUBTOTAL(109,Table69[Sales])`.

Slicers: a copied slicer drops its import provenance (cache name, marker and item options), as X19 documents. At save, X19's planner gives it a fresh `Slicer_<column>` cache name and a unique slicer name, and binds the cache to the copied table's written id. Save, reopen and save again keep the same tables, bindings and formulas. `scripts/parity/table_slicer_package_diff.py` was run on a synthetic copy against its re-save. The copy's cache `Slicer_Region2` resolves to the copied table `Sales3`, and the original's `Slicer_Region` resolves to `Sales`. The copy has slicer name `Region 2` and visible caption `Region`. The diff reports no difference, and the re-saved package is byte-identical.

Unmeasured in Excel:
- Whether Excel uses current table count or a table-id counter for naming. The measured sequence fits both; gapped ids, collisions across multiple tables in a single copy, and numbering when inserting before the source are unmeasured.
- Other invalid candidates (`AB` + `12` looks like a cell; GridCraft uses `AB_12`), an `R_` entered directly instead of normalized from `R`, and long-name boundaries other than the measured 255-character ASCII base and one-digit suffix (including Unicode length units).
- Structured references in conditional formatting, data validation, and charts. Rebinding a stored sheet-qualified structured reference was not measured: the probe's H4 was canonicalized before copying.
- The copied slicer's name, cache name and visible caption, including whether placing the copy before the original affects which one gets the unsuffixed name.
- Whether a copied PivotTable sourced from a table on the same sheet moves to the copy. Here it stays on the original, as before.
- Hyperlinks on the copy whose location is a table name. They are not rewritten.

## X35 — Sheet copy names (tabs and sheet-level copies of workbook names)

Base: X23 head `ddd2380538e78d5e67872771ec675aa45e532bc3`. Evidence: `x23-p1-copies` in the default run set (`3ac6d05f`) and the two earlier sets (`c851940b`, `fc1a1367`), Excel for Mac 16.115.2. All three agree on every tab and defined-name readback; they differ only in step status labels and the earlier set's pivot attempts.

Measured (P1: `Data` and `Report` exist; copy `Data` after `Data`, copy `Data` to the end, then copy `Data (2)` to the end):

| Copy | Sheets before | Excel tab | Excel names on the copy | GridCraft after X35 |
| --- | --- | --- | --- | --- |
| 1, of `Data` | `Data`, `Report` | `Data (2)` | `Global` and `Local`, both `=SUM(Sales5[Qty])` | Same |
| 2, of `Data` | `Data`, `Data (2)`, `Report` | `Data (3)` | `Global` and `Local`, both `=SUM(Sales7[Qty])` | Same |
| 3, of `Data (2)` | `Data`, `Data (2)`, `Report`, `Data (3)` | `Data (4)` | `Global` and `Local`, both `=SUM(Sales5[Qty])`; the copied cells use `Sales59` | Tab same; both names `=SUM(Sales59[Qty])` (unchanged from X23, see below) |

In every copy, the workbook-scoped `Global` and `Data`'s `Local` remain `=SUM(Sales[Qty])`. Table names (`Sales5`/`Keys6`, `Sales7`/`Keys8`, `Sales59`/`Keys610`) are X23's and unchanged.

Tab name (`copied_sheet_name` in `crates/engine/src/cmd/sheet/tabs.rs`): a source name ending in ` (n)` (decimal digits) is numbered from its base, so a copy of `Data (2)` takes the next free `Data (k)` instead of becoming `Data (2) (2)`. Inferred, not measured:
- Lowest free number from 2, not one more than the source's or the largest existing number. In P1 the free numbers were 2, 3, then 4, so these rules give the same result. With gaps (`Data (3)` alone, copied) GridCraft gives `Data (2)`. A user name such as `Budget (2019)` is treated as a copy number too, so its copy is `Budget (2)`.
- Only ` (digits)` at the end counts: `Q(2)`, `Q (2a)` and `Q ()` copy as `Q(2) (2)` and so on. A name that is only a number in parentheses keeps it.
- The 31-character limit shortens the base, as before X35.
No Microsoft documentation found describes the numbering rule.

Sheet-level copies of workbook names: when a sheet is copied, each workbook-scoped name that refers to it gets a sheet-level copy on the copy. That copy has the same name, comment and hidden flag, and the copy's sheet name and table names in its formula. It is written with the copy's `localSheetId` and reopens with that scope. Within the copy, it shadows the workbook name, as Excel's readback shows. The copy is part of the same undoable edit as the sheet copy, so undo removes it along with the sheet. Inferred, not measured:
- "Refers to the sheet" means the formula names the sheet (a qualified reference or name; 3-D span ends follow the rename rule already applied to the sheet's own names) or has an explicit structured reference to one of the sheet's tables. A bare table name (`=ROWS(Sales)`), a constant and references to other sheets or their tables give no copy. Only structured references were measured.
- A name the source sheet already defines (copy 3: `Data (2)!Global`) is copied from that sheet-level definition, not again from the workbook name. P1 fits this: copy 3 has one `Global`.
- Names starting with `_xl` (Excel's internal names, such as `_xlchart.*`) are not copied.

Copy of a copy (step 3 of the change): **not implemented; NEEDS DECISION.** In copies 1 and 2, Excel retargets the copied names' structured references to the copy's tables (`Sales` to `Sales5` and `Sales7`). In copy 3 it leaves both names at `Sales5` while retargeting the cells. More than one rule explains all three copies, and the candidates differ in cases P1 doesn't measure:
- (a) Names copied from a sheet that is itself a copy keep their formulas verbatim. That requires recording which sheets are copies, which the file does not store, so a reopened `Data (2)` would behave like an original.
- (b) Names retarget only references to tables from the opened file, not tables created by earlier copies in the session. This would also need in-session provenance.
- (c) Names retarget only references to tables whose names are not copy-allocated (`<name><n>`), a text heuristic that would misfire on user names like `Sales5`.
GridCraft keeps X23's retargeting (names follow the copied tables like cells), so copy 3's names read `Sales59`. A test asserts that and points to this note.

Out of scope: P3's reserved `R` and split `name`/`displayName` surface wait on the lead decision shared with X27.

Published behavior changes: a copy of a numbered sheet is named from the base (`Data (4)`, was `Data (2) (2)`), and a copy now gains sheet-level definitions of workbook names that refer to its source.

Tests: `crates/engine/src/tests/sheet_copy_names.rs` covers the P1 sequence (tabs, tables, names and their values), the inferred numbering and reference cases, undo/redo across two copies, and save to `xl/workbook.xml` `localSheetId`s and reopen. `sheet_copy_tables.rs` now expects `Data (4)` for its copies of copies.
