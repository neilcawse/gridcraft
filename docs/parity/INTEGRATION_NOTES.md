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

Reading: one reader finds slicer parts and caches through their relationships, even if the workbook or worksheet listing is missing. After X17's repair a table slicer resolves by file table and column id. Within a resolved table, `sourceName` can recover a missing column id. Without a table id, a header must match exactly one table; a dangling table id is not rebound to another table. Unresolved slicers and frames without slicer parts warn and are skipped. Table slicers retain their name, style, button height and cache provenance; captions are `ST_Xstring`-decoded and encoded for both kinds. OLAP slicers are still skipped with a warning.

Baseline failures: `cross_sheet_table_slicer_imports_without_skipped_object_warning` was this gap (the PivotTable reader warned about the table slicer's frame) and passes. `pivot_slicer_round_trips` and `pivot_slicer_keeps_every_connected_pivot_table` are not: they compare a PivotTable filter's selected list in order, and the fixture's custom Product item order (`C, A, B`, from `ce8f5fca`) reads the hidden items back as `["C", "A"]` for `["A", "C"]`. The selection is the same set; the fix belongs to the PivotTable reader or the test, not to slicers.

## X22 — Table rename follows references

[Microsoft's structured-reference guidance](https://support.microsoft.com/en-us/excel/using-structured-references-with-excel-tables), under “Rename a table or column,” says Excel automatically changes the name in all structured references used in the workbook. Microsoft documents structured references in [calculated columns](https://support.microsoft.com/en-us/excel/use-calculated-columns-in-an-excel-table) and the [Total Row](https://support.microsoft.com/en-us/excel/get-started/total-the-data-in-an-excel-table), and Excel tables as [PivotTable sources](https://support.microsoft.com/en-us/excel/get-started/create-a-pivottable-to-analyze-worksheet-data). The PivotTable documentation does not say what its source property returns after rename; that Excel readback remains unmeasured.

In GridCraft, the same undoable edit updates cell and defined-name formulas, calculated-column and custom-total formulas, table slicers, and PivotTable sources that directly name the table. Names scoped to retained chart, dialog and macro sheets also follow the rename; their sheets' opaque parts and VBA do not.

Interactive rename and X17's XLSX repair share their modeled-consumer traversal but resolve against the original workbook under different rules. Rename respects workbook lookup and a PivotTable's existing definition binding. X17 retains its original duplicate-table ownership and unbound-token capture rules. Bare tokens shadowed by a defined name or lexical binder are not treated as table references. An ambiguous caller-dependent bare token in a workbook name is left alone; use an explicit structured reference when a name should always refer to the same table. Quoted strings, ordinary totals labels, and explicitly unresolved imported PivotTable sources are not changed.

Interim policy while awaiting Excel measurements: interactive rename does not yet rewrite direct structured references in conditional formatting, data validation, chart series, sparklines, internal hyperlinks and form-control links. The gate is `REWRITE_FORMULA_OBJECTS_PENDING_EXCEL_MEASUREMENT` in `TableRename::apply`. X17 still repairs their modeled text on XLSX open and save. Entry policies and evaluator behavior are unchanged. Python `xl()` literals and opaque package content are outside the structured-formula rewrite.
