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

Zero size demonstrates effective geometry. These probes did not save XML; their Batch 5 re-runs
did return `Hidden`, `OutlineLevel` and `ShowDetail`, and since
[X44](#x44--whole-line-paste-carries-outline-levels) the outline level and collapsed-summary
marker transfer with the line. Thickness and bare markers stay on their existing lines. The
system clipboard, drag-and-drop, filtered and packed copies, and tiled pastes remain unmeasured
and retain their previous behavior. See X32 for Paste Special All and Insert Copied Cells.

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
Excel result. The whole-column readback stayed `false` after Insert. Batch 4 returned no hidden,
outline or named style for row 9, and no XML was saved. GridCraft follows ordinary Paste:
visibility transfers and an unstyled source line clears the destination's line style. Boundary
edges and bare-row markers stay on their own lines. X44 also transfers outline levels and
collapsed-summary markers; Batch 5 observed the new summary marker after Paste Special All,
but not after Insert Copied Rows. Whole columns use the same rule as rows; the clipboard
probes only exercised rows. Undo/redo, Insert Cut Cells and the system clipboard were not compared.
Insert Copied and Insert Cut Cells check row/column formatting protection against the sheet as
the insertion left it, so a new line that inherited the format of the line before it and is
then given a different source format counts as a formatting change. In Batch 5's protected
Insert Copied Rows probe (Insert Rows allowed, Format Rows denied), Excel returned `Parameter
error.` and left A8/A9 and rows 7–10 unchanged. That is consistent with a refusal, but the error
text does not name protection and the probe did not isolate which change caused it.

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
  was not isolated in a probe. An outline level or a collapsed mark counts on either axis. After
  a pasted hidden group, Excel's extent includes the next line (row 10, column J), and Batch 5
  read `ShowDetail` false there; X44 sets the collapsed mark on that line, which reproduces the
  extent. Whether the mark is what Excel counts stays inferred. A line above or left of the
  cells extends the top or left bound too. With no cells and lines on only one axis, the other
  axis spans its first line. Thick-edge flags do not count.
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

**Residual differences** (paste payload, documented in `docs/gaps.md` §25). The outline residual
is resolved by [X44](#x44--whole-line-paste-carries-outline-levels): whole-row/column Copy, Cut,
Paste Special All and Insert Copied Cells now report Excel's `A1:I10` / `H10`, `A1:J9` / `J8`
and `A1:I11` / `H10`. Still open: a copied bare or absent row leaves no GridCraft record, so
`x25-bare-row-over-untouched` and `x25-absent-row-over-untouched` still report `A1:I9`. Their
Batch 5 readbacks show no change in level, visibility, height or `ShowDetail` on rows 11/12, so
outline is not the cause. Pending Excel probe: save/reopen of pasted bare or absent rows.

Tests (`crates/engine/tests/it/inspect_extent.rs`) build each measured final state in code;
`crates/engine/tests/it/row_paste_outline.rs` reaches the outlined-group states through paste.

## X44 — Whole-line paste carries outline levels

Measured in Excel for Mac 16.115.2 (Batch 5, `Range.Copy`/`Range.Cut Destination:=`, no save).
Fixture of `x33-copy-rows-to-flat`, `x33-copy-rows-to-group`, `x33-cut-rows-to-flat` and
`x33-cut-rows-to-group`: rows 3:5 at OutlineLevel 2, hidden, under summary row 6
(`ShowDetail` false); rows 20:23 ungrouped; rows 30:33 at level 2, expanded, under row 34.
Whole rows 3:6 are copied or cut to 20:23 or 30:33.

| Readback after | Rows 20:22 / 30:32 | Row 23 / 33 | Row 34 | Source rows 3:6 after Cut |
|---|---|---|---|---|
| OutlineLevel | 2 (flat: 1 → 2; group: stays 2) | 1 (group: 2 → 1) | 1 | 1 |
| Hidden, height | true, 0 | false, 15 | false, 15 | false, 15 |
| ShowDetail | true | false | true | true (row 6: false → true) |
| UsedRange / Last Cell | Copy `A3:A34` / `A34`; Cut `A20:A34` / `A34` | | | |

The Batch 5 re-runs of the X25 cases read the same fields on rows and columns. Copying S rows
3:4 (level 1 hidden, level 2 hidden) to T row 8 turns T rows 8/9 from level 3/1 to 1/2, and
row 10, outside the paste, reads `ShowDetail` false; columns C:D to H do the same to H/I/J
(`x25-copy-rows-*`, `x25-copy-cols-*`, `x25-cut-*`, `x25-paste-special-all-rows`). A cut source
returns to level 1, visible and default size. A summary line outside the cut keeps
`ShowDetail` false: row 5, column E. A default source line resets the destination's level 3 to 1
(`x25-default-row-over-custom`, `x25-default-col-over-custom`). Insert Copied Cells gives new
rows 8/9 levels 1/2; row 11 reads `ShowDetail` true, consistent with group 9:10 containing a visible row.

Whole-line `edit.paste`, Paste Special All and Insert Copied Cells transfer each source line's
outline level and collapsed-summary marker with its size, visibility and style. A missing source
line resets them, and a same-workbook Cut resets them on vacated source lines. GridCraft
infers a collapsed marker on the adjacent summary if a fully hidden outline group ends at the
paste's last line (or starts at its first line with before-detail summaries). This rule gives
the measured `ShowDetail` false on row 10 / column J and the corresponding extent (X33 residual).
GridCraft represents `ShowDetail` false with the line's `collapsed` field and saves that field
in XLSX. After `x25-cut-rows-to-s-a8`, row 5 still reads false even without detail below it;
this supports retaining the marker independently of the current group. Undo restores the lines
and save/reopen keeps their outline state in GridCraft.

Batch 5 did not inspect Excel's saved XML or measure summary placement above/left, nesting,
stale markers at the destination boundary, or either of these GridCraft policies:

- A marker-only write requires Format Rows or Format Columns on a protected sheet, even if
  the pasted line records already match. Without permission the paste is refused.
- A same-sheet Cut resets the vacated source lines before marking the destination boundary.
  A vacated line immediately outside the paste can receive a marker based on the final group.

GridCraft does not clear a marker outside the paste. AutoFill keeps visibility and outline on
the destination, as X31 documents. Batch 5's
`x25-autofill-hidden-*` readbacks concern AutoFill visibility, not outline, and are outside this
lane. Paste Special modes other than All, transposed, linked, filtered, tiled and drag-and-drop
pastes keep their previous behavior: they transfer no line formats.

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

P1 also measured the defined-name bindings now implemented by X43: both locals follow `Sales5` or `Sales7` on the first two copies from `Data`, but stay on `Sales5` when `Data (2)` is copied. The copied cells still move to `Sales59`.

Sheet copy uses X27's `Workbook::table_name_taken` plus the names already allocated to this copy.

GridCraft retargets explicit structured references in the copied sheet's cells, calculated columns, custom totals, conditional formatting, data validation, and charts. The copy's table slicers follow its copied tables too (`Workbook::retarget_table_on_sheet`). Defined names are handled separately (see X35/X43 below). Table-local structured references with no table token need no rewrite; bare table-name formulas are left alone. The public sample also shows an explicit totals-row reference retargeted to `SUBTOTAL(109,Table69[Sales])`.

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

| Copy | Sheets before | Excel tab | Excel names on the copy | GridCraft |
| --- | --- | --- | --- | --- |
| 1, of `Data` | `Data`, `Report` | `Data (2)` | `Global` and `Local`, both `=SUM(Sales5[Qty])` | Same |
| 2, of `Data` | `Data`, `Data (2)`, `Report` | `Data (3)` | `Global` and `Local`, both `=SUM(Sales7[Qty])` | Same |
| 3, of `Data (2)` | `Data`, `Data (2)`, `Report`, `Data (3)` | `Data (4)` | `Global` and `Local`, both `=SUM(Sales5[Qty])`; the copied cells use `Sales59` | Tab same; X43 also preserves the two names' bindings |

In every copy, the workbook-scoped `Global` and `Data`'s `Local` remain `=SUM(Sales[Qty])`. Table names (`Sales5`/`Keys6`, `Sales7`/`Keys8`, `Sales59`/`Keys610`) are X23's and unchanged.

X35 originally numbered a spaced suffix from the lowest free number at or above 2. X43 replaces that inference using the Batch 5 readbacks below.

Sheet-level copies of workbook names: when a sheet is copied, each workbook-scoped name that refers to it gets a sheet-level copy on the copy. That copy has the same name, comment and hidden flag, and the copy's sheet name and table names in its formula. It is written with the copy's `localSheetId` and reopens with that scope. Within the copy, it shadows the workbook name, as Excel's readback shows. The copy is part of the same undoable edit as the sheet copy, so undo removes it along with the sheet. Inferred, not measured:
- "Refers to the sheet" means the formula names the sheet (a qualified reference or name; 3-D span ends follow the rename rule already applied to the sheet's own names) or has an explicit structured reference to one of the sheet's tables. A bare table name (`=ROWS(Sales)`), a constant and references to other sheets or their tables give no copy. Only structured references were measured.
- A name the source sheet already defines (copy 3: `Data (2)!Global`) is copied from that sheet-level definition, not again from the workbook name. P1 fits this: copy 3 has one `Global`. If an imported file contains duplicate names within a scope, only its first definition is eligible, matching GridCraft's name lookup.
- Workbook names starting with `_xl` (Excel's internal names, such as `_xlchart.*`) do not gain sheet-local copies.

X43 supersedes X35's decision to retarget inherited local names even on a copy of a copy.

Out of scope: P3's reserved `R` and split `name`/`displayName` surface (see X43 below).

X35 introduced sheet-level definitions of workbook names that refer to the source and stopped appending a second suffix to an already numbered tab such as `Data (2)`.

Tests: `crates/engine/src/tests/sheet_copy_names.rs` covers the P1 sequence (tabs, tables, names and their values), the reference cases, undo/redo across two copies, and XLSX `localSheetId` and reopen before and after moving and deleting tabs. `sheet_copy_tables.rs` now expects `Data (4)` for its copies of copies. X43 replaced X35's inferred numbering tests with the Batch 5 readbacks.

## X43 — Increment numbered copy tabs; preserve inherited local table bindings

Batch 5, Excel for Mac 16.115.2, synthetic staged read-only fixtures. The independent readbacks show:

| Case | Source and result |
| --- | --- |
| `x35-number-gap` | `Data (3)` → `Data (4)` even with `Data (2)` free. |
| `x35-number-year` | `Budget (2019)` → `Budget (2020)`. |
| `x35-number-glued` | `Q(2)` → `Q(3)` with no added space. |
| `x35-number-31-chars` | `ABCDEFGHIJKLMNOPQRSTUVWXYZabcde` → `ABCDEFGHIJKLMNOPQRSTUVWXYZa (2)` (31 characters). |
| `x35-total-copy`, `x35-copy-of-copy` | `Data` → `Data (2)` → `Data (3)`; tables `Sales5` → `Sales53` → `Sales534`. On both copies the local `Total` reads `=SUM(Sales53[Qty])`, whereas H1 on each sheet names that sheet's own table. Workbook `Total` remains `=SUM(Sales5[Qty])`. |

The inferred naming rule starts after the source's trailing parenthesised number, keeping the separator, and skips occupied candidates (Batch 4 P1 skips `Data (3)` when copying `Data (2)`). Without a numbered suffix, start with ` (2)`. Only a single ASCII space counts as the separator; any other character before `(` stays part of the base, as in `Q(2)`. The base is kept verbatim unless it must be shortened; only a shortened base loses trailing whitespace. The 31-character limit was confirmed for an ASCII base; existing case-folded collision checks, including preserved tabs, are unchanged. Not measured, kept from X35: a malformed suffix (`Q (2a)`, `Q ()`) or a bare parenthesised number (`(2)`) gets ` (2)` appended; a spaced suffix with a leading zero, including `(0)`, or one too large for a 64-bit count restarts at ` (2)` after its base (`Data (02)` → `Data (2)`). A glued suffix in those cases is treated as part of the base (`Q(0)` → `Q(0) (2)`). If the incremented number reaches the 64-bit limit while every candidate is taken, numbering restarts at ` (2)` after the whole source name.

Also unmeasured: GridCraft uses `Sheet` when the whole base is whitespace or shortening leaves only whitespace (for example, a tab named `   ` copies to `Sheet (2)`).

An original sheet's local names and newly localized workbook names follow the copied tables. When copying a sheet that was itself copied in this session, every name in the source sheet's local scope keeps its table reference; sheet qualifiers are still adjusted as before. That includes a local name the user defined on the copy after it was made, which no probe measured. Inferred, not measured: a workbook name that is newly localized on a copy of a copy (one the source sheet does not shadow) still follows the new tables, as do cells and other table consumers (measured for cells). Workbook names are never rewritten. GridCraft tracks whether the sheet was copied only in memory (`Sheet::copied_in_session`, not written to XLSX or JSON). Undo and redo restore it with the sheet, and renaming or moving the copy keeps it. After save and reopen, GridCraft treats the source as an original for name binding. Excel's behavior on a copy after save and reopen remains unmeasured, as do later edits to the names in these sequences.

Published behavior changes (from X35's `24e48086`): copying `Data (3)` when `Data (2)` is free gives `Data (4)` (was `Data (2)`). `Budget (2019)` copies to `Budget (2020)` (was `Budget (2)`), and `Q(2)` to `Q(3)` (was `Q(2) (2)`). On a copy of an in-session copy, the inherited local names keep the source copy's table: P1's `'Data (4)'!Global` and `'Data (4)'!Local` are now `=SUM(Sales5[Qty])` (were `=SUM(Sales59[Qty])`), and Batch 5's `'Data (3)'!Total` is `=SUM(Sales53[Qty])` (was `=SUM(Sales534[Qty])`). Cell formulas are unchanged.

No X23 table-name allocator change was needed. P3's copy of formula-visible `R_` still produces `R__3`. Excel's raw `AB` → `R` object-model rename splits `name=R` / `display_name=R_`; GridCraft's `table.rename` rejects `R` as invalid and retains the single-name Table Design model. P1's entry of `=SUM(Data!Sales[Qty])` already works: Excel and GridCraft both expose `=SUM(Sales[Qty])` with value 18.

Tests build the workbooks in code in `crates/engine/src/tests/sheet_copy_names.rs` and `sheet_copy_tables.rs`.

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

The formula bar, editor, Show Formulas and formula readback (`cell.get` and `sheet.read`) omit the identifier only for references bound to the cell's containing table, including its total row. Other table identifiers and text literals are kept. Like [Microsoft's structured-reference grammar](https://learn.microsoft.com/en-us/openspecs/office_standards/ms-oe376/bcd72180-31a3-423b-8f83-d224b2286da3), re-entry of the short form binds it to the containing table: cell entry stores a reference typed without a table name inside a table with that table's name, so confirming the shown text stores the formula it came from, the file keeps the table name, and the formula keeps its table when it is cut out of it (unmeasured: whether Excel stores the name for such typed input). Typed outside every table, such a reference is refused (X46). A structured reference on the right of the intersection operator that would be shown starting with `[` keeps its table name, because the bracket would otherwise attach to the word before the separator. Display does not rewrite stored expressions or change the XLSX writer; tests check that input with explicit table identifiers still writes them and that the writer keeps its long `#This Row` spelling. Ctrl+' (formula from the cell above) spells the copied formula for the cell it goes into, so a total row's `=SUBTOTAL(109,[Qty])` arrives below the table as `=SUBTOTAL(109,Table1[Qty])` (unmeasured).

Unmeasured in Excel: range fill (Ctrl+Enter) across table boundaries and Ctrl+' with localized formula syntax. GridCraft binds the short form independently at each fill destination (since X46 a destination outside every table refuses the whole fill), and Ctrl+' uses the editor's language for function names and separators.

Copying the table itself and retargeting its formulas depend on X23; X36 does not change sheet-copy behavior on its own.

X22 and X23 conflict in `crates/model/src/workbook.rs`: keep X22's `Workbook::rename_table` delegation to `table_refs::TableRename` and retain X23's `Workbook::retarget_table_on_sheet` path with its local helpers (X23's workbook-local `TableRename` struct, which sheet copy builds with `wb: None`, plus `might_use_table` and `set_formulas`); do not restore X23's older workbook-rename body.

**GridCraft trial (not an Excel measurement).** A scratch merge of X36 (`545f39bc`) and X23 (`ddd23805`) used that rule and kept both sets of documentation. The `sheet_copy_tables` run passed all 14 X23 tests plus one synthetic X36 probe. On the copy, `cell.get` returned `=SUBTOTAL(109,[Qty])` and `=[@Qty]*2`; the stored formulas and worksheet XML used the copied table's name, as did the outside `SUM` formula.
The same trial passed all 10 engine tests selected by `table_rename` and the model's `rename_table_keeps_the_array_range_of_a_formula_it_rewrites` test.

## X46 — Typed bare structured references outside every table

**Measured (Batch 5, Excel for Mac 16.115.2, `x36-short-structured-refs`; table `Sales` (Region, Qty, empty Dbl and Sub) on `Data`).** Writing `=SUM([Qty])` to F2, outside the table, was refused: Excel acknowledged the write, but F2's `Formula` and `Formula2` stayed empty, while the other writes in the same session succeeded. In the table, `=[@Qty]*2` (C2) and `=SUBTOTAL(109,[Qty])` (D2) were stored as typed, read back as typed, and filled their columns (C3 and C4 = 10 and 4; D3 and D4 = 10). This contradicts X36's rule that a bare reference outside every table is entered as typed.

**GridCraft rule.** Typing a formula with a structured reference without a table name (`[Qty]`, `[@Qty]`, `[[#Headers],[Qty]]`) into a cell with no table to bind it to is refused like any other formula that can't be entered. Only `=SUM([Qty])` was measured; the other short forms follow the same grammar rule (a table name may be left out only inside its table). The error starts with "There's a problem with this formula:" and names the reference; the cell, the rest of the command's cells and the undo stack are left unchanged. The editor keeps the draft open with the message, as it does for other refused formulas. In a table, X36's binding to the containing table is unchanged. A cell whose table is not found by its own name (duplicate table names in a damaged file) has no table to bind to either, so it refuses the formula too.

**GridCraft entry routes (route-specific behavior was not measured in Excel):**
- `cell.set`, including `array: true`: the formula bar and in-cell editor commit through it (Enter, Tab, Ctrl+Shift+Enter, and a Ctrl+' draft once confirmed).
- `range.setValues`, with or without `parse`: one refused cell refuses the whole write.
- `range.fill`: Ctrl+Enter in the editor. GridCraft choice, unmeasured in Excel: any destination outside every table refuses the whole fill, including destinations in a table (Excel was not measured filling across a table boundary; the choice follows from each outside cell being refused on its own).
- `formulas.autoSum`: generated formulas use the same validation when written to one or several cells; a refusal leaves every destination unchanged.

The CLI (`gridcraft-cli run --cmd`/`--script`), MCP, control protocol and recorded scripts run these same engine commands, so the refusal applies there. `formulas.evaluate` stores nothing and is unchanged.

**Kept, unmeasured.** These keep entering or reading the formula as written: file import of such formulas, read as stored and saved back unchanged (tested for XLSX); pasted text, Replace, Text to Columns and scenarios (`input_to_cell`); copy/paste and fill of cells that produce them; defined names; and conditional-formatting and data-validation formulas.

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
- **Scope (inferred):** The hold applies when either the outermost group's own box or the box around its members overlaps the visible sorted block. The gap between those two boxes does not count, and a block whose rows or columns are all collapsed has no visible area to overlap. A zero-size group axis counts at its start, including the near edge of the block but not the far edge. Nested groups and members follow their outermost group. Other groups follow line changes by their placement mode, such as when row heights travel with the sorted rows beside them. Since X48 a row sort leaves hidden rows in place and only reorders the heights of the rows showing, so the permutation keeps the block's height and moves nothing below it; only the automatic row fit after it can. Excel did not measure these cases.

## X38 — XLSX group projection

- XLSX import, collapsed-axis recovery after line sizing, and edited-group export share a DrawingML `a:xfrm` projection (ECMA-376 Part 1 §20.1.7.5). Source child coordinates retain the information that a zero-sized sheet axis loses. Nested transforms compose in each parent group's child units.
- Exact 90°, 180° and 270° group angles now affect member boxes even in noncollapsed groups. Other group angles keep the prior unturned geometry and import warning. This is spec-derived; Excel for Mac group layout and sizing are unmeasured. Content rotation and mirroring are not rendered by this change, and a quarter-turned group warns that its content is drawn unturned.
- **Superseded by X45 (decision a):** X38 treated the outermost worksheet anchor as the unturned frame. Nested groups already used the turned box for placement and their local unturned frame for projection.
- Collapse recovery needs the source transform tree to still describe the members; a member skipped on open (SmartArt, OLE) does not prevent it. When X34 holds every box in place, sort preserves collapse recovery only if the rows in the coincident span still occupy the same range. A sort never moves hidden or zero-height rows (X48), so a span whose rows all have no height always stays. A span holds a row that shows only when a marker offset runs past its cell (unhiding a span row moves the members anchored below it, which already ends recovery); if a sort moves such a row out of the span, recovery ends even when the row heights did not change. This also applies to subsequent sorts. When the original markers are no longer valid, sort retains only the coincident line pair and refreshes the transform snapshots: a later fit changes the collapsed extent by the change in distance between those lines, without reusing stale worksheet markers on export. Recovery ends when the resize changes the saved layout. This is a GridCraft choice; Excel for Mac is unmeasured.
- Untouched groups still write their original worksheet anchor fields and group/member transforms, including grouped members' own flips, which were previously dropped. Line resizing still applies the existing placement invalidation and anchor-mode rules; hide/unhide and chart `userShapes` ownership are unchanged.

## X47 — Direct formatting on protected sheets

Batch 5 `protect-format-rows` (Excel for Mac 16.115.2) verified the unprotected controls on `P`:
row 7 height 15 → 24 pt, row `7:7` fill white → yellow, and Locked off on `E7`. With only Format
Rows enabled, row 5 height changed from 15 to 30 pt, but fill on `5:5` stayed white and attempts
to set Locked on `5:5` or its blank cell `C5` were refused; both read back as unlocked. The
probe's encoded rule had predicted the fill would succeed. GridCraft's direct setters already
matched all four measured outcomes on the base: size and visibility use the relevant axis
permission, and direct fill, font and cell style commands require Format Cells regardless of
the range shape. The new tests pin that distinction and the two Locked refusals.

The column rule is documentation-derived, not live-measured: [Microsoft's protection options for
Mac](https://support.microsoft.com/en-us/excel/protect-a-worksheet-in-mac) distinguish column
width/visibility and row height/visibility from cell formatting. Microsoft lists [Default
Width](https://support.microsoft.com/en-us/excel/change-the-column-width-and-row-height#change-the-default-width-for-all-columns-on-a-worksheet-or-workbook)
as a column-sizing command on the same Home › Cells › Format menu. `home.defaultWidth` used to
bypass protection; it now requires Format Columns, like `home.columnWidth`, and refuses before
editing with the same message. The Standard Width dialog and its `HOD` key tip dispatch that
command, so they share the gate; undo and redo restore workbook snapshots without replaying it.

Batch 5 did not measure formatting locked versus unlocked cells with Format Cells enabled;
direct changes to Locked or Formula Hidden still refuse with that permission enabled. Data
outline commands, Clear Formats, Format Painter, and [X31](#x31--whole-row-autofill-carries-row-formatting)
and [X32](#x32--paste-special-all-and-insert-copied-cells-carry-line-formatting) transfers are
outside this direct-setter rule and unchanged. `protect-unlocked-destination` did not verify
formula transfer to `E2:E3`; it does not establish how Excel pastes protection or formatting
into unlocked cells.

## X48 — Sort leaves hidden rows and columns in place

- **Measured:** In the Batch 5 `x38-collapsed-sort-moves` case (Excel for Mac 16.115.2), requesting an ascending sort of `Data!A5:D10` by A with keys 3, 4, 6, 5, 1, 2 and rows 7:8 saved as `ht="0" customHeight="1"` without `hidden` gave 1, 2, 6, 5, 3, 4. The probe's full-range sort verifier rejected this order; the readback matches sorting only the four visible rows among their original positions, not a general rule about every way of hiding rows. The collapsed move-and-size group stayed at F7 at zero height and recovered to 30 pt after setting rows 7:8 to 15 pt each. The independently verified `x38-collapsed-sort-stays` case is consistent with this result.
- **Behaviour change:** GridCraft used to sort every line of the block, hidden ones included; row heights travelled with their rows while the hidden flag stayed with the position, so a zero-height row's height went with its key. Now a line on the sort axis that is hidden, or shown with no size (a row with zero height, a column with zero width), keeps its place with its cells, notes, links, formats, cell pictures and line settings; the lines showing are ranked by the keys and placed in the positions they held, ties in their old order. Undo restores the sheet. The group in the probe keeps its markers and grows back as Excel's did.
- **Visibility:** Sort uses the model's effective height or width: a hidden flag yields zero, a line without an explicit size inherits the default, and any positive effective size participates. Geometry conversion can round an imported custom height down to zero in the Mac profile; sorting that case was not measured.
- **Documented, not measured:** Microsoft's [Sort data in a range or table in Excel](https://support.microsoft.com/en-us/excel/sort-data-in-a-range-or-table-in-excel) says that row sorts don't move hidden rows and column sorts don't move hidden columns. Applying that rule to filter-hidden rows and extending it to zero-width columns are GridCraft choices, not Excel readbacks.
- **Not handled:** the same Microsoft page says that sorting rows in a worksheet outline sorts the level-1 groups so their detail rows stay together, even when those detail rows are hidden. GridCraft has no outline-aware sort: collapsed detail rows now stay in place while their shown summary rows are sorted.
- **X34 placement:** The row permutation conserves total block height. It does not push drawings below the block by moving a height into or out of a hidden row. Drawings beside the block can still follow changes in how unequal visible heights are distributed, and the separate automatic fit can still change heights.
- **X38 hold:** a hidden or zero-height row can no longer leave a held collapsed span, and the zero-height rows in a span can no longer be reordered. The span check and the lost-rows flag stay for the one case still reachable, a span that holds a row that shows because a marker offset runs past its cell, so neither was removed. What the sort dropped is keeping each destination's hidden flag when it moves a row's settings: both ends of a move are now shown rows.
- **Separate row fit:** After a non-identity permutation, automatic row fitting keeps the existing 200,000-cell body cutoff. Row sorts fit visible body rows, including those that kept their positions, but never excluded rows. Column sorts still fit the rows crossing the sort body; row visibility does not select which columns move. A sort that moves no visible line changes nothing and fits nothing, as before when the whole block was already in order; a block whose only disorder was among hidden lines used to be permuted and fitted, and now is left alone.

## X49 — Filter menu sorts the filter's own range

- **Before (reproduced on X48):** Filter-menu sort passed only the column to quick sort. Sorting the all-text `Name/Status` example with A1 active moved the header into the data; with `D1:E3` selected it sorted that other block. A table button had the same routing bug when the active cell was outside its table.
- **Now (doc-derived, unmeasured in Excel):** Ascending and Descending target the worksheet AutoFilter or table filter whose header button opened the menu. They sort by the clicked column, preserve the header and table totals, and leave filter-hidden rows in place (X48). The UI already knows the clicked header; it resolves the filter and calls the existing `data.sort` with an explicit range, `header: true`, and one key. The selection, undo/redo, and all other quick-sort callers keep their existing semantics.
- **Owning sheet (GridCraft choice):** The popup captures its document and worksheet when the header opens it, before the first draw. If a keyboard batch or a control command switches sheets, it closes rather than sorting or filtering the sheet now shown; this also covers Apply and Clear. The filter is resolved again on every frame, so a filter removed or resized while the popup is open is never sorted at its old range: a removed one makes Sort a no-op.
- **Evidence:** Microsoft's [AutoFilter.Sort](https://learn.microsoft.com/en-us/office/vba/api/excel.autofilter.sort) attaches sort columns and order to the AutoFilter, [ListObject.AutoFilter](https://learn.microsoft.com/en-us/office/vba/api/excel.listobject.autofilter) identifies the table's own filter, and [Sort data in a range or table in Excel](https://support.microsoft.com/en-us/excel/sort-data-in-a-range-or-table-in-excel) discusses headers and hidden lines. No Excel for Mac popup or object-model sort was measured for this lane.
- **NEEDS DECISION — saved sort metadata:** The model does not carry SpreadsheetML [`sortState`](https://learn.microsoft.com/en-us/dotnet/api/documentformat.openxml.spreadsheet.sortstate). A review probe at X49 read a package with `<sortState>` inside a worksheet `<autoFilter>`, at worksheet level and in a table part; the save wrote none of the three, with no warning. So no stale sort description is written after a menu sort, and none from the source file is kept. The sorted cells are saved normally; a metadata model and round-trip support for the filter's sort range, column and direction are out of scope. Verify Excel's emission for worksheet and table filters before implementing persistence.

## X50 — Filter menu Apply and Clear change only the owning filter

- **Before (reproduced on X49 `48e70e1e` through real popup clicks):** Apply and Clear sent only the column to `data.filterBy`, which always acted on the worksheet AutoFilter (`sh.autofilter`). With a worksheet AutoFilter over `A1:B4` and a table over `A8:B12`, the table's A8 button listed the worksheet filter's values (Ann, Ben, Cat), Apply filtered the worksheet range, and Clear cleared the worksheet filter's column A; with the table in other columns, Apply failed silently ("column outside the filter range"). On a sheet with no worksheet AutoFilter, merely opening a table's popup installed a worksheet AutoFilter over the whole table, totals row included, outside undo and without changing the revision. After that, a second table's button listed and filtered the first table; Data › Filter on a plain range elsewhere removed the stand-in filter instead of turning one on there; and undo of the first Apply left the stand-in behind, to disappear only with the unrelated step before it. A popup whose worksheet AutoFilter was removed while it was open turned a new filter on over the selection's region on Apply. The totals row itself was never hidden: the effective filter range already left it out.
- **Model (code):** a table's filter is its own: `Table::filter` (offset within the table → criterion), saved as the table part's `<autoFilter>` over `Table::autofilter_range` (header and data rows, no totals row) and read back into `Table::filter`. Slicers write it. A worksheet AutoFilter over the exact table is a proxy from older GridCraft versions that `Sheet::adopt_table_autofilter` folds into the table. No save-format or model change was needed.
- **Now (doc-derived, unmeasured in Excel):** `data.filterBy` on the active sheet accepts `table` as follows: omitted means legacy owner resolution (install a worksheet filter when needed, or follow its native table proxy); null or empty means only the existing worksheet AutoFilter; a name means that table's own filter. For a named table the column offset counts from the table and the criterion goes to its filter, adopting a legacy proxy first in the same undo step. The popup uses an explicit target for Apply and Clear, and lists values and criteria from the owning filter. Opening a popup does not change the workbook. The cell menu's Filter by Selected Cell's Value sends the table name when the selected cell is inside a table with visible filter buttons; outside those tables it keeps legacy routing.
- **Owning filter (GridCraft choice, extends X49):** besides its document and sheet, the popup records the table's run-time identity or the worksheet AutoFilter's exact range. If a rendered frame sees no filter or a different owner (including a changed worksheet range), the popup closes before Sort, Apply, Clear or Enter can act. Removing and recreating a worksheet filter at the identical range between frames counts as the same owner; this limit is not measured Excel behavior.
- **Evidence:** Microsoft's [Filter data in a range or table](https://support.microsoft.com/en-us/office/filter-data-in-a-range-or-table-01832226-31b5-4568-8806-38c37dcc180e) puts filter controls on a table's own headers, says the header row "won't be filtered", and has Clear Filter From remove a column's criterion from "a range or table"; [ListObject.AutoFilter](https://learn.microsoft.com/en-us/office/vba/api/excel.listobject.autofilter) gives a table its own AutoFilter, separate from [Worksheet.AutoFilter](https://learn.microsoft.com/en-us/office/vba/api/excel.worksheet.autofilter) (one per sheet, created by [Range.AutoFilter](https://learn.microsoft.com/en-us/office/vba/api/excel.range.autofilter)) whose [Range](https://learn.microsoft.com/en-us/office/vba/api/excel.autofilter.range) is the range it applies to. That each filter change is one undo step is a GridCraft choice. No Excel popup or object-model readback was measured for this lane.
- **Not changed:** Data › Clear (`data.clearFilter`, every filter on the sheet) and slicers are a different path. A popup filter on a slicer's column still doesn't update the slicer's selection, as before.
- **X51 MCP:** The `filter` tool forwards `table` to `data.filterBy` without collapsing explicit null into omission. A name targets the table; null or blank targets only an existing worksheet AutoFilter. `range` cannot be combined with `table`; calls that omit `table` keep their legacy behavior.

## X52 — Copy and cut marquee stays on its source sheet

- **Before (reproduced on X51 `96b79187`):** The marching-ants marquee drew on whichever sheet was active at the position the copy recorded. After copying or cutting on Sheet2, inserting a sheet before it put the marquee on the new sheet and none on Sheet2; deleting an earlier sheet or the source sheet put it on the sheet that took that position.
- **Now (GridCraft choice, unmeasured in Excel):** The marquee draws only on the source sheet, found by the same run-time sheet identity that X29's `Clipboard::check_cut_source` uses, wherever that sheet now is. A deleted source sheet shows no marquee. Copy and cut behave the same.
- **Not changed:** Clipboard lifetime and paste. Whether Excel ends copy mode when a sheet is inserted, deleted or moved is still unmeasured (see X29); the copy stays pending.

## X56 — Values read through external links

Measured on the public corpus, without opening or updating any linked file: 449 gating cells in 7 files whose formulas name another workbook (`[n]`), plus a scan of every public XLSX with an `externalLink` part (3,948 packages), comparing each formula's Excel cache with the link part's `sheetDataSet`.

- **Uncached cells of a failed linked sheet (`refreshError`).** Scalar references to a cell the failed sheet didn't save show 0 in SpreadsheetBench 59932 (`2_59932_input.xlsx`/`_answer.xlsx`: `='[1]Stock '!B6`, `='[2]1996-2000'!X338:X348`; 12 cells each, Excel 16) and, through a VLOOKUP hit, in the fuse file `0003fea7…` (`IFERROR(VLOOKUP(…,'[1]PIVOT FOR FTES SUBJECT TOTALS'!$A:M,…),"")`, 356 cells, Excel 2010). They show `#REF!` for 3,946 cells in 18 Enron workbooks converted from `.xls`. No stored blanks, namespace, refresh flag or geometry in the link parts separates the two groups. **Now:** such a read notes `Host::note_unknown_link`, as X13 already did for linked names and sheets saved without `sheetData`. The formula keeps the value Excel saved until it is calculated again (it is entered, or an edit changes what it reads), and then reads `#REF!` as before. A failed sheet that saved no cells is still `#REF!` without the note: every cache in the corpus shows that (1,055 cells in 32 files). Block reads (`SUM([1]Failed!A1:B9)`, lookup key columns) are unchanged.
- **Approximate lookup over a whole linked column.** The fuse file `1803f231…` caches 9,800 for `VLOOKUP(F162,'[1]Cognos Feb 14, 2014'!$A:$I,7)`. The column saved 77 rows: three report headings that sort above the key and one below it, two zeros, then sorted account codes. Reaching row 8 (the key) needs the search to continue below the *midpoint* after a probe in the blank tail sorts after the key. Continuing below the fallback entry (row 76), as the base did, reaches row 3 (0). `bsearch_last_within` now always continues below the midpoint. For probes inside materialized data this changes only the order of the probes over runs of other types; the FormulaEvalTestData bisection cases still pass.
- **Not changed, explained residuals.**
  - The fuse file `1df77834…`, 200 cells: `-SUMIF('[1]Alt Fac'!$A$18:$A$139,A38,'[1]Alt Fac'!$U$18:$U$139)` stays `#VALUE!`. Microsoft documents SUMIF/COUNTIF/COUNTBLANK as `#VALUE!` over a closed workbook, and 12,016 cached cells in 36 public files agree, among them 1,656 in 59932 itself. The cached values here (and 1,336 in 14 files) were saved while the source was open.
  - The two Enron `P&LSum2001` files, 1 cell each: `=+B23-[12]Report!$AE$56` caches 0 where both operands are cached 2 ULP apart (6425091.5382533884 and 6425091.5382533865). GridCraft reads both correctly and gives 1.86e-9. Excel's near-equal subtraction rounds to 0. That is general arithmetic, not external links, and is left for an arithmetic lane.
  - `1803f231…` `'1213A 080212'!J191:K191` (`SUM(J174,#REF!,…)`, cache `#REF!`, GridCraft `#N/A`) is not an external reference and predates this lane.
- **Inferred, not measured.** That the 0 caches were computed while the source was readable (a blank source cell isn't saved in the cache), and the Enron `#REF!` caches by a recalculation with the source unreachable. Proposed Excel probe: book `Src.xlsx` with `Data!A1=5`, `Data!B1` blank. Book `Dst.xlsx` has `A1='[Src.xlsx]Data'!A1`, `A2='[Src.xlsx]Data'!B1` and `A3='[Src.xlsx]Data'!C7`, where C7 is typed while `Src.xlsx` is open. Save `Dst.xlsx` with `Src.xlsx` open, close both, then rename `Src.xlsx`. Reopen `Dst.xlsx` without updating links, run Data › Edit Links › Update Values (expect `refreshError`), then `Application.CalculateFull`. Read `Range("A1:A3").Value` and `.Text` after each step, and save to inspect `externalLink1.xml`.
- **`.xlsb` / `.xls`.** XLSB link parts (`BrtSupBookSrc`, sheet caches with their refresh flag) are converted to `externalLink` XML and read by the same code, so both changes apply. The `.xls` reader records `SupBook` and `ExternName` only. It doesn't read the `XCT`/`CRN` cache, and it refuses formulas into other workbooks, which keep their saved results. That gap remains.
- **Round trip.** The link parts are still passed through byte for byte. The engine test `an_uncached_cell_of_a_failed_linked_sheet_keeps_the_saved_formula_value` builds a link part in code and checks this after a save.
- **Corpus result.** The selection is the 449-cell query plus every public file with an `externalLink` part: 6,634 files, including 3,000 guard files. Result: WORSE=0, BETTER=4, SAME=6,630, FIXED=3 (`0003fea7…` and both 59932 files), GUARD_REGRESSIONS=0. Gating cells went from 2,773 to 2,110. Of the 449 target cells, 202 remain, all explained above: 200 SUMIF cells and 2 subtraction cells.

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

## X45 — The root anchor of a quarter-turned XLSX group is its on-screen box

- **Measured:** Excel for Mac 16.115.2, Batch 5 cases `x38-turned-{90,270}-{frame,turned}`. In each case row 4 changed from 25 pt to 50 pt, with move-and-size placement and the same marker cells. The group's frame readbacks below are `(Left, Top, Width, Height)` in points; both rotations gave the same values.

| Cases | Marker cells | On-screen box, initial → resized | Excel frame, initial → resized |
|---|---|---|---|
| `x38-turned-90-frame`, `x38-turned-270-frame` | B3:F7 | `(53, 30, 212, 100)` → `(53, 30, 212, 125)` | `(109, -26, 100, 212)` → `(96.5, -13.5, 125, 212)` |
| `x38-turned-90-turned`, `x38-turned-270-turned` | B3:D11 | `(53, 30, 106, 200)` → `(53, 30, 106, 225)` | `(6, 77, 200, 106)` → `(-6.5, 89.5, 225, 106)` |

- **Behavior change:** At exactly 90° and 270° the outermost group's worksheet anchor is the turned box, not the frame. Swap its dimensions about its centre to project the child transforms from the unturned frame. Row and column sizing carry members in sheet axes; moves and X34's sort hold use the same turned box, and subsequent row fit sizes that box. Recomputed saves write on-screen worksheet markers and use the unturned frame for the group transform and inverse projection. Untouched source markers and transforms are still reused.
- **Measured versus extrapolated:** only the open-time frame and a row height edit inside the anchor are measured (above). Column width edits, moves and sorts of a quarter-turned group use the same on-screen box mapped in sheet axes; that is a consistent extrapolation of the row result (the frame is always the anchor swapped about its centre), not a measurement. Equivalent stored angles (`-5400000`, `27000000`, …) are normalized before the 90°/270° test.
- **Unmeasured:** Excel-written markers and transforms (no saves); quarter-turned moves, column edits, sorts, nested variants and non-quarter angles. The measured frame at open matches the anchor-derived box, not the source root `off`/`ext` (for B3:F7, `(109, -26, 100, 212)` pt rather than `(0, 0, 200, 100)` pt). This does not establish how Excel handles those fields in other workbooks. GridCraft's inherited export rule keeps the file's `ext`/`chExt` scale; after a resize `off` is the new frame origin but `ext` retains the file scale. Nested local transforms, ungrouped shapes/pictures, content rendering, hide/unhide and the X38(c) collapsed-span rule are unchanged. The sort edit in Batch 5's `x38-collapsed-sort-moves` errored, so it does not measure Excel's behavior for a displaced collapsed span.
- **Compatibility:** this reverses X38 decision (a) for saved files too. A workbook written by an integration build with X38's convention (root anchor = unturned frame) now opens with that anchor read as the on-screen box, so its members swap proportions. X38 never reached a release (v0.9.0 has no group-turn layout), and no file marker distinguishes those workbooks from Excel's, so no migration is attempted.
- **Corpus coverage:** the scan found no quarter-turned groups at any depth among the readable files in 49,653 public OOXML workbooks (24 unreadable). The lane eval and drawing audit did not exercise the changed behavior.

## Integration X51 (`wip/integration-x51`)

Base: published integration `4a40b8867755e6f2541f6e9f2ec45dda2f6f25f0`. Each lane was merged with `git merge --no-ff` after checking that its ref equaled the published SHA. X41–X48 are siblings on the base; X49–X51 stack on X48; X52 and X53 are siblings on X51.

| Merge | Lane head | Conflicts and resolution |
|---|---|---|
| X41 lookup argument order | `b4a5d472` | None. |
| X42 LAMBDA name calls | `5c39502c` | None. |
| X43 sheet-copy numbering | `0a481f76` | None. |
| X44 row-paste outline | `10cbe249` | None. |
| X45 turned group anchor | `97e6fddb` | None. |
| X46 bare structured-reference entry | `757fbf2b` | `crates/engine/src/cmd/edit/cell.rs`: both lanes changed the typed-formula line. Kept X42's sheet-aware `name_call_case(…, sheet)` together with X46's `BareTableRefs` bind-or-refuse match. |
| X47 protected line formatting | `68fab507` | `docs/cli.md`: X43's `sheet.move` copy paragraph and X47's protection paragraph were added at the same place. Kept both. |
| X48 sort hidden lines | `7b1afb79` | `crates/engine/src/io/tests.rs`: in the collapsed-group sort test, kept X45's member assertion (`r.h > 0.0`, the turned box) and X48's list of checked workbooks (no `sorted`, because the sort now holds the group). `INTEGRATION_NOTES.md` X38 section: kept X45's "Superseded by X45" bullet in place of the old frame bullet, and X48's updated collapse-recovery bullet. The X47 and X48 sections follow in merge order. |
| X49 filter-menu sort | `48e70e1e` | None. |
| X50 filter-menu apply | `4510ba8d` | None. |
| X51 MCP filter table | `96b79187` | None. |
| X52 copy marquee sheet | `5acf076e` | None. |
| X53 filter value undo | `2f980e2b` | None. |

All 13 merge commits have the published lane head as their second parent. The resolutions above retain both sides' applicable changes. X50 supersedes X49's popup implementation while preserving its sort routing; X53 adds undo coverage for the cell context-menu filter without changing the header popup, and updates X50's `context_filter_by_value_targets_the_table_not_the_worksheet` to match. X52's copy marquee is independent of those filter paths. After the merges, every line each lane added and the body of each of the 116 tests the lanes added or changed were compared with the published lane heads. The only differences were the X45/X48 test combined in the X48 row above, X53's update to that X50 test, and X50's replacement of X49's popup lines.

- **X44 × X48 (fix).** A row sort moves a shown row's whole line record (X48). A source row without a record used to clear only the destination's size, so the destination kept its outline level, collapsed-summary marker and line style. After an X44 paste marks a summary row, a sort that moves that row left a copy of the marker behind on the row that took its place. Now a recordless source row also resets the destination line (`crates/engine/src/cmd/data/sort.rs`), as X48 describes: "the moved line's settings replace the destination's". Test: `row_paste_outline::sort_after_whole_row_paste_keeps_hidden_detail_and_moves_the_summary_line`. The sort still has no outline awareness (X48 "Not handled"), so the marker moves with its row away from the hidden group.
- **Row record coverage.** Under GridCraft's X48 rule, the style, thick-edge flags, custom height and XLSX bare-row marker travel with the source record rather than staying at its old index. A recordless source does not duplicate these settings. `row_sort_replaces_each_moved_line_record_including_absence` checks both directions, a hidden row, the header and the row just below the sort. X48's Excel readback establishes the visible-row permutation and group recovery in its fixture, not the treatment of those individual metadata fields.
- **X45 × X48/X34/X38.** X48's collapsed-group sort tests use a 270° group. With X45 they pass with the root anchor as the turned box: members have height after the fit, and the hold, span check and recovery behave as X48 states. No code change.
- **X44 × X47.** X44 gates its outline and marker writes on Format Rows or Format Columns, as whole-line paste already gated line formats; X47 gates direct sizing, including Default Width, on the same permissions. Under Format Rows only, a direct whole-row fill is refused (X47, measured). A whole-row paste still carries the source row's line style and outline level (X25/X44; X47 leaves transfers outside its direct-setter rule). Pinned by `protected_format_rows_refuses_direct_row_fill_but_whole_row_paste_carries_line_format`. Whether Excel lets a whole-row paste change fill under Format Rows only is unmeasured.
- **X41 × X42.** An index computed by a `_xludf.` call is an index expression, like the unprefixed call of the same LAMBDA, so X41's order applies to it: its error beats a missed key and invalid mode text. Pinned by calc `name_bound_lookup_index_takes_the_x41_expression_order`.
- **X42 × X46.** Typed entry applies both rules to one formula. `=_xludf.vstack([@Qty])` in a table takes the casing of the sheet-level name and binds to that table; outside every table it is refused. Pinned by `typed_name_call_with_a_bare_structured_ref_binds_or_is_refused`.
