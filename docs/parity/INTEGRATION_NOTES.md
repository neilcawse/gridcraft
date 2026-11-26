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

## X21 — XLSX hyperlink kinds

XLSX hyperlink kind comes from file structure on open and the model's `external` flag on save. A resolved relationship with a target is external; a nonempty `location` on that relationship is kept after `#` (for example, `https://a.example/book.xlsx#Sheet1!A2`). A `location` alone is internal, including [a zero-length `location`](https://learn.microsoft.com/en-us/openspecs/office_standards/ms-oi29500/842e6c5e-cfaf-4d16-8395-a64547b5e22b). The model adds a `#` marker to scheme or network-path text that navigation would otherwise open externally or refuse: `location="http://a.example"` becomes `#http://a.example`, and `location="file:report.xlsx"` becomes `#file:report.xlsx`. This does not bypass navigation's validation or X17's handling of error tokens and doubled markers.

Only an external model link writes a relationship. Internal links write `location` using X17's rules. The first save can canonicalize the spelling without changing the destination (for example, `#'#Data'!A1` becomes `'#Data'!A1` and `##NAME?` becomes `#NAME?`); later cycles retain that spelling. Insert › Link still sorts new targets as before (`links_outside`), and `HYPERLINK()` formulas are unchanged.

Native workbook JSON with no `external` field uses the old export heuristic once on read: after trimming for the check only, `://` anywhere, a leading backslash UNC prefix, or a case-insensitive leading `mailto:` or `file:` means external. Explicit `true` and `false` always win. XLSX export itself never infers the kind from the target.

A scan of 49,655 public XLSX-family packages (24 unreadable) found 268,581 worksheet hyperlinks in 3,283 packages: no `location` contains `://`, no `location`-only link starts with a URL scheme or network path, and no hyperlink relationship target starts with `#`. The change is tested with synthetic packages; how Excel itself follows a `location` that reads like a URL remains unmeasured.

Known gap: OPC-Internal hyperlink relationships are read as external worksheet links, so the in-package destination and relationship-target fragment are not preserved. Scanning 33,370 public XLSX-family packages found 1,179 `/hyperlink` relationships without `TargetMode="External"` in 34 packages across all parts (23 packages could not be fully read). Of the worksheet `<hyperlink r:id>` references, zero in zero packages resolved to a relationship without `TargetMode="External"`, including zero with a target fragment (25 packages could not be fully read for this check).

## X19 — XLSX table slicers

Table slicers are written on save instead of being dropped. The package follows Excel's own files (public corpus: the 118-8 SpreadsheetBench workbooks) and [MS-XLSX §2.6.97 `CT_TableSlicerCache`](https://learn.microsoft.com/en-us/openspecs/office_standards/ms-xlsx/c3629be2-99fe-44b7-970c-3ea6df625cf5): a cache with `name`, `sourceName` (the written column header) and an extension `{2F2917AC-EB37-4324-AD4E-5DD8C200BD13}` with `x15:tableSlicerCache`. The workbook lists it in `x15:slicerCaches`, extension `{46BE6895-7355-4a93-B00E-2C351335B9C9}`. The sheet lists its table slicer part in `x14:slicerList`, extension `{3A4CF648-6AED-40f4-86FF-DC5316D8AED3}`. The drawing frame's compatibility choice requires `sle15`, and the cache has a workbook-scoped `#N/A` defined name. The selection is the table's AutoFilter. The cache uses the actual serialized table and column ids, recorded while writing the table.

Table and PivotTable slicers go through one planner, so X09's name-ownership rules apply to both: slicer names are unique across kinds, an imported cache keeps its name only while it is free or still owned by its original marker, and new caches are `Slicer_<column>`. An imported table cache also keeps its item options (`sortOrder`, `customListSort`, `crossFilter`), which the model doesn't edit; a copied sheet's slicers drop them with the rest of the import provenance.

Table slicers follow table and column renames, including X17's save-time table name repair. Removing a table or slicer column removes its slicers on every sheet in the same undoable edit. Saving defensively drops slicers whose source table is not written, has no data rows, or lacks the column, and reports a warning. Dropped slicers leave no cache, part, list entry or frame; an imported marker survives as an ordinary workbook name, as for pivot slicers. No new marker is generated for a dropped slicer.

Reading: one reader finds slicer parts and caches through their relationships, even if the workbook or worksheet listing is missing. After X17's repair a table slicer resolves by file table and column id. Within a resolved table, `sourceName` can recover a missing column id. Without a table id, a header must match exactly one table; a dangling table id is not rebound to another table. Unresolved slicers and frames without slicer parts warn and are skipped. Table slicers retain their name, style, button height and cache provenance; captions are `ST_Xstring`-decoded and encoded for both kinds. OLAP slicers are still skipped with a warning.

Baseline failures: `cross_sheet_table_slicer_imports_without_skipped_object_warning` was this gap (the PivotTable reader warned about the table slicer's frame) and passes. `pivot_slicer_round_trips` and `pivot_slicer_keeps_every_connected_pivot_table` are not: they compare a PivotTable filter's selected list in order, and the fixture's custom Product item order (`C, A, B`, from `ce8f5fca`) reads the hidden items back as `["C", "A"]` for `["A", "C"]`. The selection is the same set; the fix belongs to the PivotTable reader or the test, not to slicers.

## X26 — Keep table totals visible during filtering

Excel-saved public SpreadsheetBench examples `31202/1_31202_input.xlsx` and `49939/1_49939_input.xlsx` have table refs `A1:J48` and `A19:H28`, but AutoFilter refs `A1:J47` and `A19:H27`; both have `totalsRowCount="1"` and a visible totals row. [ECMA-376 Part 1](https://ecma-international.org/wp-content/uploads/ECMA-376-1_5th_edition_december_2016.zip), §§18.3.1.2 and 18.5.1.2, describes the filter ref and the table's totals count; it does not require a totals-free filter ref. In fact, the informative Annex L.2.4.6 shows a full-table filter ref without criteria. The writer follows the observed Excel save convention.

Native table filters evaluate data rows only. A worksheet filter counts as a table proxy when it matches the table header, column bounds, and either the last data row or the full table's end. The full-table proxy also excludes totals from dropdown items, Top/Bottom and Above/Below Average samples, and the classification of filter-hidden rows. Turning the totals row on or off moves a full-table proxy with the table, so it never strands on the removed totals row or loses its criteria on save. Wider and partial worksheet filters keep their original evaluation ranges; existing XLSX overlap serialization restrictions still apply. Slicers edit native table criteria, taking over an exact worksheet proxy if present; other filters remain distinct. If separate active ranges cover the same row, either can hide it. These coexistence and proxy rules are GridCraft choices, not measured Excel behavior.

The model stores the worksheet filter's range and criteria, not whether it came from the table dropdown or was created by hand. An exact match on a headered table follows the table; this preserves its data scope and criteria across the toggle and save. An exact match on a headerless table is not a proxy and does not move.

Known gaps for a later measured lane: Excel's Data > Clear targets the filter around the active cell; GridCraft clears criteria from the worksheet filter and every native table filter on the sheet, and does not sync slicer selections on Clear.

XLSX has one row hidden flag without hide provenance. On import, an active native or worksheet filter whose ref exactly equals its full table's range is treated as stale: the totals-row flag is cleared but the data-row flags are kept. A correctly bounded or inactive filter does not clear a hidden totals row. This recovery policy is not measured Excel behavior and may reveal a manually hidden totals row in the stale active case. Likewise, an older file saved with an already-correct filter ref but a hidden totals row cannot be repaired safely; the hidden flag is preserved as manual visibility.
No recovery is attempted for the correctly bounded legacy case, even when a slicer cache records a selection: that cache does not distinguish a filter-hidden totals row from a manually hidden one.

## X27 — Table names and sheet-scoped defined names

A table's name must be unique among table and defined names of any scope (ECMA-376 Part 1 §18.5.1.2), regardless of case ([Microsoft: rename an Excel table](https://support.microsoft.com/en-us/office/rename-an-excel-table-fbf49a4f-82a3-43eb-8ba2-44d21233b114)). Table Name checked other tables and `Workbook::names`, which holds workbook names and names scoped to worksheets. It missed names scoped to kept chart, dialog and macro sheets, which live only in `PreservedSheet::names`. A table could therefore be renamed to such a name, and X17 then renamed it on save without being asked. Insert Table's own `Table<n>` and Forecast Sheet's checked tables only.

One check, `Workbook::table_name_taken(name, except)`, now decides for every editing command that gives a table a name. Names given by the user are refused with "The name entered already exists. Enter a unique name."; generated names come from `Workbook::next_table_name`, the first free `Table<n>`.

| Entry point | Before | Now |
| --- | --- | --- |
| `table.rename` | tables, `wb.names` | shared check; the renamed table keeps its own name in another case |
| `insert.table`, `home.formatAsTable` with `name` | tables, `wb.names` | shared check |
| `insert.table`, `home.formatAsTable` without `name`, `data.fromTextCsv` | `Table<n>` free among tables | `next_table_name` |
| `data.forecastSheet` | `Table<n>` free among tables | `next_table_name` |
| `sheet.move` with `copy` | the copy's tables are dropped at this base, so no name is given | unchanged here. When X23 (`parity/X23-sheet-copy-tables`) merges, its copied-table allocator (`copied_table_names` in `cmd/sheet/tabs.rs`) should switch to `table_name_taken`, which already covers the kept-sheet definitions and marker-less slicer-cache names its review found missing |
| Paste, duplicate | no path creates a table | unchanged |
| XLSX import and save repair | X17 already counts kept sheets' names | reads the stored name instead of re-parsing attributes, and assigns the first free `Table<n>` instead of preserving or suffixing a reserved prefix; XLSB, ODS and CSV import create no tables |

A name is taken by:

- another table: spec-derived (§18.5.1.2);
- a defined name of any scope, including those of kept sheets: spec-derived (§18.5.1.2);
- an imported slicer cache's name (`Slicer::imported_cache`): GridCraft's rule, from X09/X19. Save keeps that name and writes it as a workbook `#N/A` name when the file had no marker for it, so the name shares the defined-name scope. A marker read from the file is already in `Workbook::names`;
- a name starting `_xlnm.`, in any case: spec-derived. ECMA-376 Part 1 §18.2.5 reserves the prefix for built-in names ("End users shall not use this string for custom names in the user interface"). Whether Excel refuses such a table name, and with which message, is unmeasured;
- a name starting `_xlchart.`: GridCraft's rule. The XLSX writer makes these names for Excel 2016 charts at save time, and the reader takes them back as chart data, so they never reach `Workbook::names`. Excel's behaviour is unmeasured.

X17's repair and the editing commands use the same reserved-prefix check. Cache priority is intentionally different at the XLSX boundary: if an imported table and a cache without its own defined-name marker conflict, keep the table spelling, which formulas may reference, and let the writer choose a different cache name. In an edit the user is choosing a new table name, so the existing cache name is taken.

Names compare with `gridcraft_formula::same_name`: ASCII case-insensitive like `Workbook::table`, plus the Unicode case fold already used by Define Name's table check and by X17's repair. Without it, a `GRÖSSE` table beside a `Größe` name would still be renamed on save. How Excel folds non-ASCII table names is unmeasured. `same_name` now compares folded characters lazily, so the check allocates nothing.

To be read without parsing XML on every keystroke, a kept sheet's defined name now holds its unescaped `name` apart from its other attributes (`PreservedName::name`). Its element is written with the same bytes as before: `localSheetId`, then `name`, then the other attributes. A kept `definedName` without its required `name` is dropped on read, as a workbook name is.

Unmeasured, for the lead to measure in Excel: the message for a taken name (the text is the base's, unmeasured in this lane), the behaviour for `_xlnm.` and `_xlchart.` table names, whether Excel's own `Table<n>` skips a number a sheet-scoped defined name or a differently cased name holds, and Excel's case fold beyond ASCII.

Tests: `crates/engine/src/tests/table_name_scopes.rs` covers every entry point against a synthetic package whose dialog sheet holds sheet-scoped names. It includes case-insensitive, reserved, slicer-cache and non-ASCII cases, and a guard showing that a free name works and the kept names are written back unchanged.

## X25 — Whole-row and whole-column paste

- [Microsoft's move/copy guidance](https://support.microsoft.com/en-us/excel/move-or-copy-cells-rows-and-columns) describes transferring full lines, including their cell formats and hidden cells. [Paste options](https://support.microsoft.com/en-us/excel/paste-options) distinguishes ordinary paste from explicit column-width options. The documentation does not settle how Copy or Cut affects line sizes or inherited line formatting.

The following results were measured in Excel for Mac 16.115.2 using synthetic action probes.
`Range.Copy Destination:=...` and `Range.Cut Destination:=...` were used to isolate worksheet
effects from clipboard effects. All the returned `Formula` strings were literals or blank, not
formulas. The observed styles of sampled populated cells moved with those cells.

| Synthetic action probe | Observed worksheet effect | Excel extent after (UsedRange / Last Cell) |
|---|---|---|
| `x25-copy-rows-to-t-a8`, `x25-copy-rows-to-t-8-9`, `x25-copy-rows-explicit-to-t-a8`, `x25-copy-rows-explicit-to-t-8-9` | Both destination heights become zero; the first row takes the source's fill and bold, the second loses the old fill. Source unchanged. | Destination `A1:I10` / `H10` |
| `x25-copy-cols-to-t-h1`, `x25-copy-cols-to-t-h-i`, `x25-copy-cols-explicit-to-t-h1`, `x25-copy-cols-explicit-to-t-h-i` | Both destination widths become zero; sampled cells take the copied cell styles. Source unchanged. | Destination `A1:J9` / `J8` |
| `x25-copy-blank-column-style` | An empty styled source column changes the destination column and its blank cells from yellow/italic to blue/bold. Width is unchanged. | Not sampled |
| `x25-cut-rows-to-s-a8`, `x25-cut-rows-to-t-a8` | Same destination effect as row copy. Source lines return to default height, lose visible row formatting and no longer contain the moved values. | Destination `A1:I10` / `H10` |
| `x25-cut-cols-to-s-h1`, `x25-cut-cols-to-t-h1` | Same destination geometry as column copy. Blank destination cells inherit the source blue/bold style. Source widths and blank-cell styles reset, and the moved values are gone. | Destination `A1:J9` / `J8` |
| `x25-default-row-over-custom`, `x25-default-col-over-custom` | A default source resets the destination size, and the destination row loses its prior fill. In the column case only one stored destination cell was sampled; it is cleared and reads Normal. | Destination unchanged: `A1:I9` / `H8` |
| `x25-bare-row-over-untouched`, `x25-absent-row-over-untouched` | Both empty copies leave destination geometry and formatting unchanged. Bare versus absent row has no measured effect other than the different destination address. | Destination `A1:I11` / `H11` and `A1:I12` / `H12`, respectively |
| `x25-control-a3-b4` | An ordinary rectangle changes cell formatting and content, not line sizes. The aggregate row style readback becomes mixed, which does not establish loss of inherited formatting on untouched cells. | Destination unchanged: `A1:I9` / `H8` |
| `x25-used-base`, `x25-used-bare-row`, `x25-used-width-col` | Bare row and width-only column do not extend this data rectangle. | `C3:E5` / `E5` |
| `x25-used-height-row`, `x25-used-style-row` | Height-only and styled rows extend the reported row bound. | `C3:E40` / `E40` |
| `x25-used-style-col` | A styled column extends the reported column bound. | `C3:J5` / `J5` |

For the action cases the baseline Excel extent is `A1:I9` / `H8`; a sheet not receiving the paste
retains that extent. Last Cell need not be UsedRange's bottom-right, including with hidden lines
in these workbooks. Since X33, inspection reports Excel's UsedRange and navigation uses its Last
Cell, while print, export and calculation keep the shared work extent; see
[X33](#x33--reported-usedrange-and-last-cell).

GridCraft transfers size and effective visibility for contiguous whole-line `edit.paste`, and
transfers inherited row and column styles. An absent source resets those fields. For a default
source column, that destination style reset is inferred: Excel showed the style reset on blank
cells in the vacated Cut source, but the default-column Copy did not measure a blank destination
cell or the destination column's aggregate style. Same-workbook Cut resets the transferred
fields on the source and moves them as they stand at paste time. If the original source sheet
is no longer at the recorded index, GridCraft refuses the paste rather than clearing another
sheet. Full-size explicit ranges and line syntax are equivalent. Line and cell edits share one
undo entry and line changes respect the sheet's formatting permissions.

As of X31, a protected sheet's formatting permission does not let a whole-line paste or cut
change inherited Locked or formula-Hidden flags, including on blank cells. A refused transfer
leaves the workbook unchanged. This guard compares final effective protection, including where
a cell's explicit style masks a line style; it does not change protection on other paste paths.

Zero size demonstrates effective geometry; the probes did not recover `Hidden`, `OutlineLevel`
or `ShowDetail` values, nor did they save XML. Outline/collapse, thickness and bare markers stay
on their existing lines. The system clipboard, drag-and-drop, filtered and packed copies, and
tiled pastes remain unmeasured and retain their previous behavior. See X32 for Paste Special All
and Insert Copied Cells.

## X29 — Cut source sheet identity

A pending Cut records its source sheet by position. Inserting a sheet before it, deleting it or
moving it can leave another sheet at that position. Paste, Insert Cut Cells and the whole-line
paste of X25 now compare the sheet there with the one that was cut. If they differ, the command
is refused before anything changes, and the cut stays pending; cutting again works. Sheets
added after the source, and renames, leave the cut valid. After a rename, references to the moved
cells follow them under the sheet's new name.

Whether Excel cancels cut mode when a sheet is inserted, deleted, moved or renamed is unmeasured.
Refusing is the conservative choice; it never moves cells from a sheet the person did not cut.
Copy is unaffected, because it pastes from the snapshot taken when copying. Drag-and-drop cuts and
pastes in one step, so no sheet change can come between them.

Enter-to-Paste shows the refusal without dismissing it on the same keystroke and leaves the
clipboard pending, like Cmd+V and the ribbon. It still ends copy mode after a successful paste.
Excel's behavior on refusal is unmeasured; this keeps GridCraft's own paste paths consistent.

## X31 — Whole-row AutoFill carries row formatting

Measured in Excel for Mac 16.115.2 on a synthetic workbook using
`Range("3:4").AutoFill Range("3:9")` with the default fill type, not a fill-handle drag. Sheet
`S` stores rows 3, 4 and 8 with a row style, `customFormat` and a custom height: red bold 30 pt,
blue bold 38 pt and green 22 pt. Column A holds 1, 2 and 6 and column B holds `=A<n>*2`.

| Readback | Rows 5–9 after the fill |
|---|---|
| Height (`RowHeight`, pt) | 30, 38, 30, 38, 30: the source rows in turn |
| Row style, fill, bold | GC Red Bold, GC Blue Bold, GC Red Bold, GC Blue Bold, GC Red Bold; row 8 loses its green |
| Cells A and B | A continues the series 3–7 (row 8's stored 6 is replaced by the series value, also 6); B holds `=A5*2` … `=A9*2` with values 6–14; each cell takes its source row's style |
| Rows 2, 3, 4, 10 | Unchanged |
| `Hidden`, `OutlineLevel`, `ShowDetail` | Not returned by Excel (unmeasured) |

The probe did not read UsedRange or Last Cell for this case, did not save the workbook (so no
`customHeight`/`customFormat` XML), and sampled only columns A and B.

GridCraft `edit.autoFill` now gives each line the target adds to an entire-row source the height,
custom-height flag, saved size and row style of the source row it lines up with, counting in
source-sized steps from the source's first row. It reuses X25's `LineAxis::replace`, so line
and cell edits share one undo entry. Destination visibility, outline/collapse, thickness and
bare-row markers stay on their own lines.

Inferred, not measured:

- **Hidden** was not measured. AutoFill retains destination visibility; X25 paste still copies it.
- **A source row with no record** resets the target row to the default height and no row style,
  as in X25 paste; both measured source rows had a record.
- **Protected sheets** need row-formatting (or column-formatting) permission for line changes.
  That permission cannot change inherited Locked or formula-Hidden flags; neither can clearing
  a cell format expose such a change. Existing protection rules for explicit cell-style writes
  otherwise remain unchanged.
- **Upward fills** (target above the source) line rows up as the cell fill does: the row just
  above the source takes its last row.
- **Non-default fill modes.** Copy, Formats, Series, Alternate, Extend and the date modes repeat line
  formatting. Formats leaves content alone: populated cells paired with an absent source cell
  take its effective format. Fill Without Formatting (`values`) does not transfer line formats;
  new content in an empty cell takes the destination's effective format. The existing AutoFit
  pass may recompute automatic heights after a small fill; custom heights are not recomputed.
- **Whole columns** are handled symmetrically: width, saved width flags and column style
  repeat across the target. Whole-column AutoFill was not measured. Its reach is
  limited, as before, by the 5,000,000-cell fill cap (at most four 1,048,576-row columns).

Partial-range AutoFill (any source that is not entire rows or entire columns) and Fill
Down/Right/Up/Left keep their previous behaviour and leave line formatting alone.

## X32 — Paste Special All and Insert Copied Cells carry line formatting

**Measured (`x25-paste-special-all-rows`, `x25-insert-copied-rows`):** In Excel for Mac 16.115.2,
copying `S!3:4` then using Paste Special All or Insert Copied Cells at `S!A8` gave rows 8 and 9
an effective height of 0 pt. Row 8 took row 3's red, bold style; cell A9 took A4's Normal style.
A3–A5 were unchanged and A8/A9 took A3/A4's values. With Insert, the old A8/A9 and their row
formats moved to rows 10/11 (12 pt green and 0 pt yellow); with Paste Special All, A10/A11 stayed blank.

GridCraft's internal Paste Special All now shares the ordinary whole-line Paste path. It copies
the line style, size (including saved size and custom-height flag) and visibility. The shared
protection guard also applies. Insert Copied Cells uses Paste Special All after shifting the
existing lines; one undo or redo restores both cell and line changes. The rules for reporting
UsedRange and Last Cell are unchanged by this lane.

**Inferred or unmeasured:** Column C's whole-column Bold readback changed from `false` to `true`
after Paste Special All; GridCraft tests C8/C9's inherited bold as an inference, not as a sampled
Excel cell result. The whole-column readback stayed `false` after Insert. Excel did not return
hidden, outline or a named style for row 9, and did not save XML. GridCraft follows ordinary
Paste: visibility transfers and an unstyled source line clears the destination's line style,
but outline/collapse, boundary edges and bare-row markers stay on their own lines.
Whole columns use the same rule as rows; the clipboard cases only exercised rows. Protection,
undo/redo, Insert Cut Cells and the system clipboard were not compared in these probes.
Insert Copied and Insert Cut Cells check row/column formatting protection against the sheet as
the insertion left it, so a new line that inherited the format of the line before it and is
then given a different source format counts as a formatting change and is refused when the
sheet does not allow formatting rows (or columns). That is GridCraft's choice; Excel was not
measured on protected sheets here.

Other Paste Special modes (including Values, Formulas, Formats and All Except Borders), arithmetic
operations, Skip Blanks, Transpose, Paste Link and partial-range paste retain their previous
behavior. Drag-and-drop and filtered, packed, tiled or full-sheet sources do not transfer line
formats via this change.

## X33 — Reported UsedRange and Last Cell

Evidence: the `used_range` and `last_cell` object-model readbacks of every X25 case in the three
default-cohort run sets and the X25-clipboard run (Excel for Mac 16.115.2); all four agree. They
are `UsedRange` and `SpecialCells(xlCellTypeLastCell)` readbacks, not Ctrl+End keypresses.

| Final state | UsedRange / Last Cell |
|---|---|
| Fixture S and T before any action: cells to A1:I9, row 9 and column I hidden | `A1:I9` / `H8` |
| Data `C3:E5` alone, plus a bare `<row r="40"/>`, plus a width-only column J | `C3:E5` / `E5` |
| Data `C3:E5` plus a height-only or a style-only row 40 | `C3:E40` / `E40` |
| Data `C3:E5` plus a style-only column J at the default width | `C3:J5` / `J5` |
| Whole rows 3:4 copied, cut or Paste Special All to row 8 (rows 8:9 hidden) | `A1:I10` / `H10` |
| Whole columns C:D copied or cut to column H (H:I hidden) | `A1:J9` / `J8` |
| Insert Copied Cells at row 8 (old row 9, hidden, now row 11) | `A1:I11` / `H10` |
| A3:B4 copied to A8; default row/column copied over custom row 8 / column H | `A1:I9` / `H8` |
| Bare or absent row copied over untouched row 11 / 12 | `A1:I11` / `H11`, `A1:I12` / `H12` |

**UsedRange rule** (`Sheet::reported_used_range`). The shared extent (stored cells, styled
blanks included, and spills) grown along each axis by the line records Excel counts.

- Measured: a row height or row style counts; a column style counts; a column width alone and a
  bare row element do not. A row record grows only the row bounds (`C3:E40`, not `A3:E40` or a
  full-width range), and a column record only the column bounds.
- Inferred: a saved row height also counts when it does not override the default geometry
  (`ht` without `customHeight`). A visibility-only row counts and a visibility-only column does
  not, following the measured difference between a row height and a column width. This does not
  override a column style: a hidden styled column still counts in UsedRange. Visibility alone
  was not isolated in a probe. An outline level or a collapsed mark counts on either axis;
  this could explain the extra line after a pasted hidden group (row 10, column J), whose readbacks
  show only default geometry. A line above or left of the cells extends the top or left bound too.
  With no cells and lines on only one axis, the other axis spans its first line. Thick-edge flags
  do not count.
- `saved_element` (a bare `<row>` kept for save and custom-default fitting) does not count, per
  `x25-used-bare-row`. The sizing probe that saw a trailing bare row in UsedRange used a switched
  custom-default sheet, where that row fits to a non-default height; GridCraft gives the fitted
  row a size, so it counts there by the height rule.

**Last Cell rule** (`Sheet::last_cell`). The bottom-right corner of the reported extent, stepped
back over its trailing rows and columns of zero effective size. Measured: hidden trailing row 9,
11 and column I are skipped (`H8`, `J8`, `H10`); a trailing row without a style or cells is not
(`H10` after row paste). Inferred: non-hidden zero-size lines and filtered rows are skipped like
manually hidden ones; an entirely hidden axis keeps its first reported line; an empty sheet gives A1.

**Consumers switched.** `document.inspect` `usedRange` (and so CLI `info`) reports the new
extent. `edit.goToSpecial` `lastCell`, Ctrl+End and Ctrl+Shift+End in the grid, and
`review.workbookStatistics` `endOfSheet` use the same Last Cell rule. The end-of-sheet field moved because
[Microsoft documents it as "End of sheet / Last cell"](https://support.microsoft.com/en-us/excel/check-workbook-statistics);
it is still `null` on an empty sheet. Every other `Sheet::used_range` caller is unchanged,
including print/PDF, CSV and other exports, calculation, charts, pivots, Ctrl+arrow target
calculation, Go To Special's search scope, `sheet.read` and the scrollbar extent. This changes published
command output: `document.inspect` and `review.workbookStatistics` can report a larger range or a
different end cell than before.

In the normal grid view, a small viewport may need to scroll beyond the legacy physical scroll
cap to show a selected endpoint near the last worksheet row or column. The view now allows the
offset needed to show that endpoint while it remains selected, without changing the shared
used-range scrollbar bounds. This also fixes the pre-existing snapback after Ctrl+arrow to a
physical sheet edge. On base `c40a8d51`, a 600×420 keyboard test reached `XFD1048576` with
Ctrl+Down and Ctrl+Right but failed the visibility assertion after three more frames. The
regression also checks Ctrl+End and Ctrl+Shift+End with formatting-only bounds. From such a
revealed offset the vertical scrollbar can still move up, but a further wheel, drag or click
down over it no longer snaps the view back to the legacy limit.

**Residual differences** (paste payload, documented in `docs/gaps.md` §25). After GridCraft's own
whole-row/column Copy or Cut the destination keeps its outline level and has no collapsed summary
line, so it reports `A1:I9` where Excel reports `A1:I10` / `A1:J9`. At the X33 base, Paste Special
All and Insert Copied Cells do not transfer the incoming line formatting; X32 adds that transfer
but does not resolve the outline/collapse difference. A copied bare or absent row leaves no GridCraft
record, so `x25-bare-row-over-untouched` and `x25-absent-row-over-untouched` still report `A1:I9`.
Pending Excel probes: OutlineLevel/ShowDetail after whole-row copy, and save/reopen of pasted bare
or absent rows.

Tests (`crates/engine/tests/it/inspect_extent.rs`) build each measured final state in code; the
outlined-group states carry the inferred outline level and collapsed summary line.
