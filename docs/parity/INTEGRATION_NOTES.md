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

Reading: one reader finds slicer parts and caches through their relationships, even if the workbook or worksheet listing is missing. After X17's repair a table slicer resolves by file table and column id. Within a resolved table, `sourceName` can recover a column id that is missing, dangling (no column of the table has it) or malformed. Without a table id, a header must match exactly one table; a dangling table id is not rebound to another table. Unresolved slicers and frames without slicer parts warn and are skipped. Table slicers retain their name, style, button height and cache provenance; captions are `ST_Xstring`-decoded and encoded for both kinds. OLAP slicers are still skipped with a warning.

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
| `sheet.move` with `copy` | the copy's tables are dropped at this base, so no name is given | unchanged by X27 itself. The integrated `copied_table_names` allocator uses `table_name_taken`, including kept-sheet definitions and marker-less slicer-cache names found by X23's review |
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

P1 also measured a defined-name difference. Excel's first copy creates local `Local` and a local `Global` that shadows the workbook name, both referring to `Sales5[Qty]`; the second copy from `Data` does the equivalent for `Sales7`. On the copy of the first copy, both locals still refer to `Sales5[Qty]`, even though the copied cells now use `Sales59[Qty]`. X35 adds the sheet-level copies of workbook names and Excel's tab name for a copy of a copy; it deliberately leaves GridCraft's copy-of-copy retargeting unchanged.

Sheet copy uses X27's `Workbook::table_name_taken` plus the names already allocated to this copy.

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

Base: X23 head `ddd2380538e78d5e67872771ec675aa45e532bc3`. Evidence: `x23-p1-copies` in `ACTION-RESULTS-BATCH4.md`, Excel for Mac 16.115.2. Three synthetic run sets agree on the tab and defined-name readback.

Measured (P1: `Data` and `Report` exist; copy `Data` after `Data`, copy `Data` to the end, then copy `Data (2)` to the end):

| Copy | Sheets before | Excel tab | Excel names on the copy | GridCraft after X35 |
| --- | --- | --- | --- | --- |
| 1, of `Data` | `Data`, `Report` | `Data (2)` | `Global` and `Local`, both `=SUM(Sales5[Qty])` | Same |
| 2, of `Data` | `Data`, `Data (2)`, `Report` | `Data (3)` | `Global` and `Local`, both `=SUM(Sales7[Qty])` | Same |
| 3, of `Data (2)` | `Data`, `Data (2)`, `Report`, `Data (3)` | `Data (4)` | `Global` and `Local`, both `=SUM(Sales5[Qty])`; the copied cells use `Sales59` | Tab same; both names `=SUM(Sales59[Qty])` (GridCraft still retargets them; see below) |

In every copy, the workbook-scoped `Global` and `Data`'s `Local` remain `=SUM(Sales[Qty])`. Table names (`Sales5`/`Keys6`, `Sales7`/`Keys8`, `Sales59`/`Keys610`) are X23's and unchanged.

Tab name (`copied_sheet_name` in `crates/engine/src/cmd/sheet/tabs.rs`): a source name ending in ` (n)` (decimal digits) is numbered from its base, so a copy of `Data (2)` takes the next free `Data (k)` instead of becoming `Data (2) (2)`. Inferred, not measured:
- Lowest free number from 2, not one more than the source's or the largest existing number. In P1 the free numbers were 2, 3, then 4, so these rules give the same result. With gaps (`Data (3)` alone, copied) GridCraft gives `Data (2)`. A user name such as `Budget (2019)` is treated as a copy number too, so its copy is `Budget (2)`.
- Only ` (digits)` at the end counts: `Q(2)`, `Q (2a)` and `Q ()` copy as `Q(2) (2)` and so on. A name that is only a number in parentheses keeps it. Leading zeros count as digits (`Data (02)` copies to `Data (2)` if free), and the digit run is not parsed as a number.
- To stay within 31 characters, GridCraft counts Unicode scalar values as elsewhere in the workbook model, fits the suffix, and trims trailing whitespace from the shortened base. If nothing remains, it uses `Sheet` as the base. The generated tab skips case-folded collisions, including non-ASCII names and names of tabs preserved for export. General sheet lookup is unchanged.
No Microsoft documentation found describes the numbering rule.

Sheet-level copies of workbook names: when a sheet is copied, each workbook-scoped name that refers to it gets a sheet-level copy on the copy. That copy has the same name, comment and hidden flag, and the copy's sheet name and table names in its formula. It is written with the copy's `localSheetId` and reopens with that scope. Within the copy, it shadows the workbook name, as Excel's readback shows. The copy is part of the same undoable edit as the sheet copy, so undo removes it along with the sheet. Inferred, not measured:
- "Refers to the sheet" means the formula names the sheet (a qualified reference or name; 3-D span ends follow the rename rule already applied to the sheet's own names) or has an explicit structured reference to one of the sheet's tables. A bare table name (`=ROWS(Sales)`), a constant and references to other sheets or their tables give no copy. Only structured references were measured.
- A name the source sheet already defines (copy 3: `Data (2)!Global`) is copied from that sheet-level definition, not again from the workbook name. P1 fits this: copy 3 has one `Global`. If an imported file contains duplicate names within a scope, only its first definition is eligible, matching GridCraft's name lookup.
- Workbook names starting with `_xl` (Excel's internal names, such as `_xlchart.*`) do not gain sheet-local copies.

Copy-of-copy binding is a known mismatch. In P1's third copy Excel kept both names at `Sales5` while the cells moved to `Sales59`. GridCraft deliberately keeps its current behavior: both copied names retarget to `Sales59`, as the regression test asserts. The Excel rule behind the observed mismatch remains unmeasured; X35 adds no copy provenance, table-origin tracking, or name-shape heuristic.

Out of scope: P3's reserved `R` and split `name`/`displayName` surface wait on the lead decision shared with X27.

Published behavior changes: a copy of a numbered sheet is named from the base (`Data (4)`, was `Data (2) (2)`), and a copy now gains sheet-level definitions of workbook names that refer to its source.

Tests: `crates/engine/src/tests/sheet_copy_names.rs` covers the P1 sequence (tabs, tables, names and their values), the inferred numbering and reference cases, undo/redo across two copies, and XLSX `localSheetId` and reopen before and after moving and deleting tabs. `sheet_copy_tables.rs` now expects `Data (4)` for its copies of copies.

## X22 — Table rename follows references

[Microsoft's structured-reference guidance](https://support.microsoft.com/en-us/excel/using-structured-references-with-excel-tables), under “Rename a table or column,” says Excel automatically changes the name in all structured references used in the workbook. Microsoft documents structured references in [calculated columns](https://support.microsoft.com/en-us/excel/use-calculated-columns-in-an-excel-table) and the [Total Row](https://support.microsoft.com/en-us/excel/get-started/total-the-data-in-an-excel-table), and Excel tables as [PivotTable sources](https://support.microsoft.com/en-us/excel/get-started/create-a-pivottable-to-analyze-worksheet-data). The PivotTable documentation does not say what its source property returns after rename; that Excel readback remains unmeasured.

In GridCraft, the same undoable edit updates cell and defined-name formulas, calculated-column and custom-total formulas, table slicers, and PivotTable sources that directly name the table. Names scoped to retained chart, dialog and macro sheets also follow the rename; their sheets' opaque parts and VBA do not.

Interactive rename and X17's XLSX repair share their modeled-consumer traversal but resolve against the original workbook under different rules. Rename respects workbook lookup and what a PivotTable's definition binding resolves to, including a same-named table that took a deleted bound table's place. When the replacement is renamed, the save warning still names the deleted table as the old source. X17 retains its original duplicate-table ownership and unbound-token capture rules. Bare tokens shadowed by a defined name or lexical binder are not treated as table references. Quoted strings, ordinary totals labels, and explicitly unresolved imported PivotTable sources are not changed.

Unmeasured GridCraft choices, pending Excel readback:

- An ambiguous caller-dependent bare token in a workbook name (a sheet-scoped defined name shares the table's old name, which a valid file cannot hold) is left alone; use an explicit structured reference when a name should always refer to the same table.
- A bare table token whose new spelling a LET or LAMBDA binder in scope would capture becomes `NewName[]`, the same data body.
- A rewritten formula is reprinted from its parse: the table token changes, `[[#This Row],[Col]]` displays as `[@Col]` (files still hold the long form), and insignificant whitespace is dropped. Calls of the workbook's LAMBDA names take the names' spelling, in cells (including cells read from a native file) and in formula text alike. Calls of built-ins and of LET or LAMBDA binders in scope are not respelled to a workbook name. Formulas without a reference to the table keep their text.

Batch 3 measured internal hyperlink sub-addresses in Excel for Mac 16.115.2: `Sales[Amount]`, `Sales`, and `SalesAmt` all stayed unchanged when `Sales` was renamed to `Revenue`. GridCraft likewise leaves them unchanged. Direct structured references in conditional formatting, data validation, chart series, sparklines, and form-control links remain under the interim no-rewrite policy (`REWRITE_FORMULA_OBJECTS_PENDING_EXCEL_MEASUREMENT` in `TableRename::apply`). X17 still repairs modeled text for these objects and hyperlinks on XLSX open and save. Python `xl()` literals and opaque package content are outside the structured-formula rewrite.

**Open definition-entry gap.** In Batch 3, the Excel object model already read back `=Data!$B$2:$B$3` from `RefersTo` immediately after assigning `=Sales[Amount]`; that absolute range stayed unchanged across the table rename. GridCraft stores `=Sales[Amount]` at definition time and rewrites it to `=Revenue[Amount]` on rename. This difference begins at definition entry/readback, not at rename. Batch 4 tests file-seeded names separately; X30 does not change name storage.

## X30 — Built-in call spelling

**Measured (Batch 3, Excel for Mac 16.115.2).** With a workbook name `IsRef` defined as `=LAMBDA(x,FALSE)`, entered formulas `=ISREF(A1)` and `=isref(A1)` both read back as `=ISREF(A1)` and returned `TRUE`. This held both for an edited name and for a name seeded in workbook XML. With the non-built-in name `MyFn` defined as `=LAMBDA(x,x+1)`, `=myfn(1)` read back as `=MyFn(1)` and returned `2`.

**GridCraft rule.** The same built-in catalog protects call spelling on entry, XLSX import, and table rename. A matching workbook LAMBDA cannot change a built-in's uppercase spelling, but other LAMBDA calls still take the defined name's spelling. Applying the rule to all cataloged functions and to formulas already present at import generalizes the measured entry behavior. Evaluation of an already-stored mixed-case built-in AST uses the same semantics as its uppercase spelling without rewriting the stored text. Batch 3 did not measure mixed-case built-in calls seeded in formula XML; this is an inferred compatibility rule. Explicit user-defined call markers retain their meaning.

**Import gap, closed by X42.** X30 read both `TRIMRANGE(1)` and `_xlfn.TRIMRANGE(1)` in formula XML as built-in calls when a workbook LAMBDA `TrimRange` existed. Batch 5 measured Excel reading the unprefixed call as the LAMBDA; see X42.

## X42 — Newer-function calls that share a workbook LAMBDA's name

**Measured (Batch 5, Excel for Mac 16.115.2, `x30-lambda-collisions`; read at open, nothing saved).** Workbook names `TrimRange` and `VStack` hold `_xlfn.LAMBDA(_xlpm.x,"lam")`. The fixture's formula XML and Excel's readbacks:

| cell | formula XML | value | `Range.Formula` |
|---|---|---|---|
| A1 | `TRIMRANGE(1)` | `lam` | `=TrimRange(1)` |
| A2 | `_xlfn.TRIMRANGE(1)` | 1 | `=TRIMRANGE(1)` |
| A3 | `VSTACK(1)` | `lam` | `=VStack(1)` |
| A4 | `_xlfn.VSTACK(1)` | 1 | `=VSTACK(1)` |
| A5 | `_xludf.VSTACK(1)` | `lam` | `=VStack(1)` |
| A6 | `_xlfn.LET(_xlpm.vstack,_xlfn.LAMBDA(_xlpm.x,5),_xlpm.vstack(1))` | 5 | `=LET(vstack,LAMBDA(x,5),vstack(1))` |

This contradicts X30's published rule that the built-in wins for unprefixed calls.
Excel's `Range.Formula2` added a leading `@` to every formula in the table, including the LET call.

**GridCraft rule.** These are the functions the file format writes with `_xlfn.` (the writer's newer-function list, now `gridcraft_formula::file_prefix`, which the reader, the evaluator and the writer share).
- XLSX open: an unprefixed cell-formula call binds to a LAMBDA name of the same spelling if the visible definition holds a LAMBDA. A sheet-level name takes precedence, even if it holds no LAMBDA. The reader stores bound calls as `_xludf.<Name>`. `_xlfn.` normally selects the built-in; see the lexical exception below.
- Model and evaluator: `_xludf.Name(…)` calls the workbook name `Name` (sheet scope first, then workbook), never a LET/LAMBDA variable or the built-in. Its dependencies and link checks follow the name's definition. An unprefixed call in the model is the built-in. A LET or LAMBDA variable that holds a LAMBDA comes before a newer function of its name, so A6 gives 5.
- Spelling: `_xludf.` calls take the visible defined name's casing (`_xludf.VSTACK` → `_xludf.VStack`) on import, entry and table rename. Explicit cell calls follow a rename of their resolved definition; built-in calls do not.
- Save: a name-bound call is written `_xludf.VStack(1)`, and a built-in call `_xlfn.VSTACK(1)`. Reopening gives the same bindings in GridCraft, and Excel reads both spellings as measured (A5, A4).
- Legacy formulas without dynamic-array metadata use implicit intersection for both built-in and explicit name calls that may return arrays. For example, the stored text is `=@_xludf.TrimRange(1)`; `cell.get`, `sheet.read` and `FORMULATEXT` expose it. The formula bar hides `_xludf.`. The writer removes the implied `@` in legacy XML without adding dynamic metadata; dynamic calls remain dynamic.

**Why `_xludf.` and not unprefixed.** `_xludf.` is GridCraft's existing marker for a user-defined call (#726). The parser keeps it, the writer writes it unchanged, and the editor hides and restores it, so no new model state is needed. Excel measurably reads it as the name (A5). An unprefixed spelling would need the writer to know each call's binding. Other readers that apply the file format's rule would also take an unprefixed call as the built-in.

**Unmeasured in Excel.**
- Older functions (no `_xlfn.` prefix in files, e.g. `ISREF`) with a same-named LAMBDA keep X30's rule: the built-in. A LET-bound call of an older function, such as `LET(sum,LAMBDA(x,5),sum(1))`, also stays the built-in (1).
- Typed entry: `=vstack(1)` stays the built-in, spelled `VSTACK`, and `=_xludf.vstack(1)` calls the name. The LET rule is in the evaluator, so a typed `LET(vstack,LAMBDA(x,5),vstack(1))` also gives 5. Excel measured that formula only as read from a file.
- A same-named defined name that holds no LAMBDA leaves the call with the built-in. Unprefixed calls inside defined names, conditional formats, validation and chart formulas are read as before. Sheet-level LAMBDA names bind like workbook-level ones from their own sheet, which is an inference.
- Excel's saved spelling for these calls (no workbook was saved). `.xls`, `.xlsb` and ODS import are unchanged.
- Unsupported newer built-ins such as `_xlfn.TRANSLATE(1)` give the same result as when no colliding name exists, and retain the built-in spelling on save. Typed `translate(1)` likewise gives the unsupported-function result instead of calling the LAMBDA, consistent with typed entry binding to the built-in. An unprefixed `TRANSLATE(1)` in file XML still binds to a visible LAMBDA and is saved as `_xludf.Translate(1)`.
- A call of a LET variable shows in upper case (`VSTACK(1)`; Excel shows `vstack(1)`). GridCraft also discards the `_xlfn.` marker during parsing, so an explicitly prefixed call inside a LET with a same-named lexical LAMBDA uses that LAMBDA. Excel's behavior for that combination was not measured; the fixture used `_xlpm.vstack` at the call site.
- Mixed bindings on rename: GridCraft rewrites a workbook-scoped name body even if the old name resolved to a sheet-local definition for some callers. This matches the existing behavior for plain name references; those callers can therefore switch to the renamed global definition. Excel behavior is unmeasured.

## X36 — Structured-reference entry and display text

**Measured (Batch 4, Excel for Mac 16.115.2, `x23-p1-copies`).** Excel accepted `=SUM(Data!Sales[Qty])` in `Data!H4`; `Range.Formula` returned `=SUM(Sales[Qty])` and the value was 18. In the table, the total formula read back as `=SUBTOTAL(109,[Qty])` and the calculated-column formula as `=[@Qty]*2`; those spellings persisted on copies, while the outside formula named the copied table. The run did not measure the formula bar or the workbook XML Excel writes.

**GridCraft choices.** Entry also accepts quoted sheet names, case-insensitive matches, cross-sheet entry, localized input and `[0]` (the current workbook). The sheet must own the resolved table; missing or mismatched names, linked workbooks and 3-D sheet qualifiers are rejected rather than silently dropping the qualifier. After validation, GridCraft stores the existing table-qualified structured reference. File and defined-name parsing are unchanged.

The formula bar, editor, Show Formulas and formula readback (`cell.get` and `sheet.read`) omit the identifier only for references bound to the cell's containing table, including its total row. Other table identifiers and text literals are kept. Like [Microsoft's structured-reference grammar](https://learn.microsoft.com/en-us/openspecs/office_standards/ms-oe376/bcd72180-31a3-423b-8f83-d224b2286da3), re-entry of the short form binds it to the containing table: cell entry stores a reference typed without a table name inside a table with that table's name, so confirming the shown text stores the formula it came from, the file keeps the table name, and the formula keeps its table when it is cut out of it (unmeasured: whether Excel stores the name for such typed input; a bare reference outside every table is still entered as typed). A structured reference on the right of the intersection operator that would be shown starting with `[` keeps its table name, because the bracket would otherwise attach to the word before the separator. Display does not rewrite stored expressions or change the XLSX writer; tests check that input with explicit table identifiers still writes them and that the writer keeps its long `#This Row` spelling. Ctrl+' (formula from the cell above) spells the copied formula for the cell it goes into, so a total row's `=SUBTOTAL(109,[Qty])` arrives below the table as `=SUBTOTAL(109,Table1[Qty])` (unmeasured).

Unmeasured in Excel: range fill (Ctrl+Enter) across table boundaries and Ctrl+' with localized formula syntax. GridCraft binds the short form independently at each fill destination, and Ctrl+' uses the editor's language for function names and separators.

Copying the table itself and retargeting its formulas depend on X23; X36 does not change sheet-copy behavior on its own.

X22 and X23 conflict in `crates/model/src/workbook.rs`: keep X22's `Workbook::rename_table` delegation to `table_refs::TableRename` and retain X23's `Workbook::retarget_table_on_sheet` path with its local helpers (X23's workbook-local `TableRename` struct, which sheet copy builds with `wb: None`, plus `might_use_table` and `set_formulas`); do not restore X23's older workbook-rename body.

**GridCraft trial (not an Excel measurement).** A scratch merge of X36 (`545f39bc`) and X23 (`ddd23805`) used that rule and kept both sets of documentation. The `sheet_copy_tables` run passed all 14 X23 tests plus one synthetic X36 probe. On the copy, `cell.get` returned `=SUBTOTAL(109,[Qty])` and `=[@Qty]*2`; the stored formulas and worksheet XML used the copied table's name, as did the outside `SUM` formula.
The same trial passed all 10 engine tests selected by `table_rename` and the model's `rename_table_keeps_the_array_range_of_a_formula_it_rewrites` test.

## X37 — Form check boxes

Base: published X11 form drop-downs head `b435c44f`. Legacy form check boxes become `ControlKind::Check` (`CheckState` unchecked/checked/mixed, caption, link, `no_three_d`, kept macro, caption markup), following the drop-down's path end to end; cell checkboxes (`checkbox` number format, `feature_bags.rs`) are untouched.

- `crates/model/src/features.rs`: `ControlKind::Check`, `CheckState`, `CaptionMarkup`, `CheckVmlLook` and the check box fields on `FormControl`.
- `crates/xlsx/src/controls.rs`: reads x14 `formControlPr objectType="CheckBox"` (`checked`, `fmlaLink`, `noThreeD`, `lockText`) with VML fallback, and VML-only `ObjectType="Checkbox"` shapes (`x:Checked`, `x:FmlaLink`, `x:FmlaMacro`, `x:NoThreeD`, caption). Writes the properties part, worksheet control, VML and DrawingML stand-in. For ordinary names, VML keeps the name in `id` and the shared numeric shape identifier in `o:spid`, including when generated and opaque shapes share a drawing. Names that look like shape numbers rely on the worksheet control name. Plain caption whitespace and macro text survive both import paths; unsupported rich captions, conflicting types and settings outside the model stay in pass-through.
- The VML writer retains explicit caption locks and tight wrapping, and writes an imported check box back with the look its shape had (`CheckVmlLook`): paint attributes, `v:fill`, `o:lock` (including its aspect-ratio and text locks) and `v:path` as read or left out, and `x:AutoFill`/`x:AutoLine False` and `x:TextVAlign Center` as read. The worksheet control's `autoFill`/`autoLine` are kept independently of the VML; VML-only imports use their VML flags for the generated worksheet entry. Worksheet entries without explicit `defaultSize="0"` and `autoPict="0"`, or with unsupported explicit flag values, stay in pass-through. The x14 caption lock keeps its own value when both parts exist. GridCraft does not enforce the VML aspect lock when resizing. Inch- and point-based VML styles use the same cell-anchor geometry on import. Observed paint values on explicitly unpainted shapes do not prevent editing; unsupported paint, hidden or flipped geometry, image data, and unsupported text-box settings stay in pass-through.
- `crates/engine/src/cmd/controls.rs`: `insert.checkBoxControl`; `control.list` reports `kind: "checkBox"`, `value` (0/1/2), `state`, `text`, `link`, `threeD`; `control.setValue` takes `false`/`true`, 0/1/2 or the state name (it reuses the slider/drop-down convention rather than `control.press`, which stays the button's run-an-action command, so the state is explicit and scripts can set mixed). `control.format` sets caption, state, link and `threeD`. Linked-cell writes go through `cell.set`, so Undo, protection (a locked linked cell on a protected sheet refuses the change) and the linked-cell sync behave as for sliders and drop-downs; links move with structural edits and renames through the shared control-link rewrite.
- `crates/ui-egui/src/{form_controls,control_ui}.rs`, `ribbon/insert.rs`: draws the box, tick or mixed shading and caption; a click or Space toggles (mixed → checked); Format Control has caption, state, cell link and 3-D shading; Insert > Controls has Check Box.

Public corpus (GridCraft CLI `control.list`, 46 workbooks): the review fixes raised editable legacy check boxes from 536 to 1,185 of 1,231. By comparing editable counts with VML checkbox counts, fully editable workbooks rose from 16 to 36, partial fell from 11 to 6, and none-editable fell from 19 to 4. No workbook lost editable check boxes, and the CLI reported no errors. This measures import coverage, not Excel rendering or round-trip fidelity.

Measured (Excel object model, synthetic workbook): Excel lists all three VML-only check boxes `GC Check 1..3` by name at every stage.

Documented, not measured (Microsoft's form check box documentation, ECMA-376, [MS-OI29500], [MS-XLSX]):

- the linked cell gets TRUE when checked, FALSE when cleared and #N/A when mixed (Excel's linked-cell readback failed in the measurement run);
- GridCraft's inverse sync is its own choice: a linked cell holding TRUE/FALSE/#N/A, or a number (0 → cleared, other → checked), moves the box; text, empty cells and other errors leave it alone;
- a click on a mixed box checks it (GridCraft's choice, inferred; not documented or measured);
- `x:Checked` 0/1/2 and `checked` Unchecked/Checked/Mixed, `noThreeD` / `x:NoThreeD` and the VML caption `<div>`/`<font>`, as the specs describe them. The defaults written for a new check box (`autoFill="0" autoLine="0"`, `lockText="1"`, `x:AutoLine False`, `x:TextVAlign Center`) are inferred from the specs' examples, not measured;
- An existing x14 properties part without `lockText` stays unlocked (MS-XLSX `CT_FormControlPr`). In VML, explicit false `MoveWithCells`/`SizeWithCells` flags behave like omission under Excel's reversed flag convention (MS-OI29500 §§19.4.2.42, 19.4.2.59). The rewritten files are tested in GridCraft; Excel readback of the rewritten geometry, caption layout, and names has not been measured.
- a check box's macro is kept as written and never run (GridCraft's choice, as for VBA);
- with no link, a check box on a protected sheet still toggles, as other controls do; that matches GridCraft's existing control rule, not a measurement.

## X24 — Drawing placement after row autofit

- Mechanism: `crates/xlsx/src/drawing.rs` resolved each anchor to the model's cell, offset and pixel size while the sheet still had the heights the file saved; `crates/engine/src/io.rs` `fit_imported_rows` (Mac fit, the Legacy grow-only fit and the oversized-font fit of #833) ran afterwards. Two-cell objects kept their stale height, and `absoluteAnchor` objects, stored cell-relative, moved with rows above them. Stored X18 markers kept the file right; display, print/PDF, `display --json` and rebuilt markers used the stale box.
- `crates/model/src/drawing_file.rs`: `DrawingPlacement::anchor` is the one placement-to-anchor conversion (it replaces the reader's `placement_to_anchor`). `crates/model/src/line_follow.rs`: `Sheet::place_after_open_sizing` re-resolves top-level objects with a stored placement over the final lines and keeps `DrawingFile::anchor` and group file anchors current, so the XLSX writer still emits the original marker values and `editAs`; objects without a stored placement (chart `userShapes`, form controls) follow by mode. `Sheet::follow_line_sizes` applies `ST_EditAs` to later line-size changes; hide/unhide is excluded on purpose.
- `crates/engine/src/cmd.rs` `commit`: non-restructured sheets follow the command's own line changes before the row fit, skipping the objects the command placed itself (X28); every sheet then follows its row fit. `discard_placement_after_line_changes` is unchanged and still decides which stored placements survive.
- In-app resizing uses a live two-cell placement for move-and-size drawings, including remapped placements during a post-shift row fit. A marker retains its named row or column even when its offset crosses into another cell or its position coincides with zero-height rows. Model-only drawings and drawings whose file placement is stale infer the far corner from the old box.
- Internal reanchoring of fixed drawings and group members preserves sheet coordinates beyond the object-command input cap (10 million pixels); the input cap itself is unchanged.
- Evidence: ECMA-376 Part 1, 20.5.2.33 `twoCellAnchor`, 20.5.2.24 `oneCellAnchor`, 20.5.2.1 `absoluteAnchor`, 20.5.3.2 `ST_EditAs` (spec-derived). Whether Excel for Mac's own open-time fit counts as a resize for `editAs="oneCell"`/`"absolute"` two-cell anchors is unmeasured; GridCraft places them from both markers.
- Known gaps outside X24:
- Chart `userShapes` are flattened into sheet drawings on import, so they do not retain ownership by or stay relative to the chart as it moves.
- Collapsed imported group member boxes are recovered from child coordinates in X38 below.
- Stored far markers are honoured only until the first resize inside the drawing's span. After that, X18 drops the stored placement and the far corner is recomputed from pixels, so a far marker whose offset runs into a later row, or that sits on a zero-height boundary, loses its named row on a later resize. Excel's behaviour across repeated resizes is unmeasured.

## X28 — Drawing sort settle detection

- `crates/engine/src/cmd.rs`: `Ctx::placed` records the floating objects a command placed itself (sheet uid and object). The commit no longer infers this by comparing an object's anchor with its pre-command anchor. `sheet::follow_line_changes` follows the command's own line changes for everything not in `placed`, before the row fit. The fit is then followed for every object on its sheet, from the lines each fitted row had before, which are recorded only when a fit changes a row. Restructured and other sheets now share that fit path. Groups are left out of the pre-fit follow when they or any member is in `placed`, and they follow the fit like any object.
- `crates/engine/src/cmd/data/sort.rs`: `sorted_anchor` returns `None` for objects the sort leaves where they are (absolute ones, ones outside the sorted block). Every object it re-anchors goes into `placed`, even when its anchor comes out unchanged. `Sheet::object_refs` (model) names the objects in `object_anchors` order.
- Before the row fit, sort drops a placed drawing's saved markers if they no longer describe its editable anchor on the sorted grid. An unchanged anchor alone does not make the old markers valid.
- Which objects a sort moves, and where it puts them, are unchanged. Excel's own sort-with-shapes behaviour is unmeasured.
- Groups straddling the sort block are unmeasured. On the base, a group followed the pre-fit line changes when all its editable anchors were unchanged. X28 skips that follow if the sort placed any member, even when the member's editable anchor is unchanged.
- X34 below supersedes X28's member-level sort placement for grouped drawings.

## X34 — Grouped shapes stay in place during sort

- **Measured:** In the Batch 4 `x28-group-sort` case (Excel for Mac 16.115.2), sorting `A5:D10` ascending by A with no header left both group boxes and their move-and-size placement unchanged. One group straddled the top of the block; the other was wholly inside it. All six rows sorted intact.
- **Behavior change from X28:** Grouped drawings no longer follow their individual rows during the permutation. Sort holds the group's geometry and every member's sheet position, including when permuted row heights require different cell-relative anchors. Ungrouped drawings keep X28's placement behavior. A separate automatic row fit after the permutation still follows each group's anchor mode at its held position; X38 below describes when a collapsed group can recover. Save/reopen persists the resulting placement and undo/redo restores the corresponding workbook state.
- **Inferred, not measured:** Unchanged group bounds are consistent with unmoved members, but lookup by name failed and a swap within the same bounds cannot be ruled out. The probe used uniform row heights and did not measure permuted unequal heights, a post-sort fit that changes heights, horizontal sorts, other placement modes, or nested groups. GridCraft applies the same group policy to those permutations and treats any subsequent fit as a resize, using its existing X24 placement behavior. Excel did not write anchors or child offsets; the round-trip tests use synthetic GridCraft workbooks.
- **Scope (inferred):** The hold applies when either the outermost group's own box or the box around its members overlaps the visible sorted block. The gap between those two boxes does not count, and a block whose rows or columns are all collapsed has no visible area to overlap. A zero-size group axis counts at its start, including the near edge of the block but not the far edge. Nested groups and members follow their outermost group. Other groups follow line changes by their placement mode, such as when row heights travel with the sorted rows or a hidden row takes another height and moves the rows below it. Excel did not measure these cases.

## X38 — XLSX group projection

- XLSX import, collapsed-axis recovery after line sizing, and edited-group export share a DrawingML `a:xfrm` projection (ECMA-376 Part 1 §20.1.7.5). Source child coordinates retain the information that a zero-sized sheet axis loses. Nested transforms compose in each parent group's child units.
- Exact 90°, 180° and 270° group angles now affect member boxes even in noncollapsed groups. Other group angles keep the prior unturned geometry and import warning. This is spec-derived; Excel for Mac group layout and sizing are unmeasured. Content rotation and mirroring are not rendered by this change, and a quarter-turned group warns that its content is drawn unturned.
- The outermost group's worksheet anchor is treated as its unturned frame; after a 90° or 270° turn its members fill that box swapped about its centre, so they can extend past the anchor, and a member past the top or left sheet edge is clamped onto the sheet. A nested group's sheet box is its turned box. Whether Excel for Mac writes the anchor of a quarter-turned group as the unturned frame or as the turned box is unmeasured.
- Collapse recovery needs the source transform tree to still describe the members; a member skipped on open (SmartArt, OLE) does not prevent it. When X34 holds every box in place, sort preserves collapse recovery only if the rows in the coincident span still occupy the same range. Reordering within the span is fine; if any row leaves it, recovery ends even when the row heights did not change. This also applies to subsequent sorts. When the original markers are no longer valid, sort retains only the coincident line pair and refreshes the transform snapshots: a later fit changes the collapsed extent by the change in distance between those lines, without reusing stale worksheet markers on export. Recovery ends when the resize changes the saved layout. This is a GridCraft choice; Excel for Mac is unmeasured.
- Untouched groups still write their original worksheet anchor fields and group/member transforms, including grouped members' own flips, which were previously dropped. Line resizing still applies the existing placement invalidation and anchor-mode rules; hide/unhide and chart `userShapes` ownership are unchanged.

## X14b–X38 integration (`wip/integration-x38`)

**X15 lookup criteria, updated by X41 (Batch 5, Excel for Mac 16.115.2).** VLOOKUP and HLOOKUP over a readable local reference or an evaluated table array apply these checks after resolving the lookup value and table:
1. **Index expression error.** An index that evaluates to an error in the formula itself (computed `1/0`, literal `#NAME?`, an inner lookup's error) returns that error before the mode is coerced and before the search, whether the key is found or missed (A2/A3, A11/A12, A20/A21, A29/A30, A38/A39, A47/A48, A56/A57). The public Enron workbook `larry_may__21911__larryoptmodel.xlsx` (sheet `nymex`, e.g. H31) also caches `#DIV/0!` for `VLOOKUP($A31,skewtable,HLOOKUP(ROUND(I31-AO31,1),skewtable,2,FALSE),FALSE)`, where `$A31` is not in `skewtable`. Exception kept from X15, unmeasured: when the mode argument is itself an error value, the mode's error is returned.
2. **Mode.** A mode that cannot be coerced to a logical returns `#VALUE!` before the search or cell-read index validation (A46, A49–A55, A58–A63); an error-valued mode returns its error.
3. **Search.** A miss returns `#N/A` whatever an index read from a cell holds, including an error (A15–A18, A42–A45), and for any numeric index, including one out of range (A10, A13/A14, A37, A40/A41).
4. **Index, on a hit.** An error read from a cell returns `#REF!` (A8/A9, A26/A27, A35/A36). Text read from a cell returns `#REF!` (G1 `"abc"`: A6, A24, A33) unless it parses as a whole number, or as a number in [0, 1), which takes the numeric below-one check (G2 `"0.9"`: A7, A25, A34). A number below 1 returns `#VALUE!` (A5, A23, A32) and one past the table `#REF!` (A4, A22, A31).

Changes against the published integration `4a40b886` (X38 order: mode, then any error index, then search, then index):
- An index expression error now beats an invalid non-error mode; before, the mode's `#VALUE!` won. Batch 5 measured literal `"x"`. GridCraft applies the same order when the mode is invalid text read from a cell or an invalid element of a multi-cell mode reference; those cases are unmeasured.
- An error read from the index cell now returns `#N/A` on a miss; before, it returned `#REF!` ahead of the search (X38's "an error index beats a miss" now holds only for index expression errors). On a hit it is still `#REF!`. With a single-area table reference, a multi-area index fails dereferencing but is still treated as reference-sourced (`#N/A` on a miss, `#REF!` on a hit). With an evaluated table (an array constant or a multi-area table), the multi-area index is instead lifted over its cells, and each cell follows the rules above: `VLOOKUP(2,{1,10;2,20},(D1,D2),FALSE)` spills `2` and `20`, and `(G3,G4)` holding errors spills `#N/A` twice on a miss (`#REF!` before X41). Batch 5 did not measure multi-area indexes.
- On a hit, numeric text in [0, 1) read from the index cell now returns `#VALUE!`; before, it returned `#REF!`. This covers spellings that parse to such a number, such as `"0.9"`, `".9"`, `" 0.9"`, `"90%"`, `"0.5e0"` and formula text `="0.9"`; only `"0.9"` was measured.
- Unchanged: whole-number text (`"2"`, `"1.0"`, `" 2 "`, `"2e0"`), booleans, blanks, numeric indexes, other fractional text (`"1.5"`) and negative text (`"-0.5"` stays `#REF!`). Batch 5 did not measure whole-number text, booleans, blanks, fractional text of 1 or more, or negative text.

**X20 external names.** Direct references to linked tables (including `INDEX` over one) do not use the X41 local path and are unchanged from `4a40b886`: they keep the index-first order with X15's linked argument coercion, so text `"0.9"` read from the index cell is still `#REF!`. A failed, empty cache returns `#N/A` for a `#REF!` index only when the mode is valid or omitted. Batch 5 did not measure linked tables, including precedence or the text-index result for readable caches.

**Residual: `COUNTIF` over a whole column in an Excel 97 cache.** EUSES `TRAMP.xls` (sheet `Unknown Data Entry`, C9:M9) caches 6, 11, 10, 12, 0 and 0 for `=COUNTIF(ThresholdCalc!A:A,"<> 9999")-1`; GridCraft gives 1048531 and similar. ThresholdCalc's columns are filled through row 51 and empty below. The cache was written by Excel 97 (producer 8.0), which counted only the used cells. That conflicts with X15's Excel 365 probe, where `COUNTIF($G:$G,"<>23")` counts the blanks of the whole column. GridCraft follows current Excel. The current-Excel value for this file has not been measured, and neither has the `.xls` whole-column extent (65,536 rows in Compatibility Mode); the grid size is unchanged here.
