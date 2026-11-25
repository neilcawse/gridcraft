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

X17 repairs the table namespace on XLSX import and before export. Defined names take precedence, whatever their scope: ECMA-376 Part 1 §18.5.1.2 requires a table's `displayName` to be unique amongst all defined names in the workbook, and GridCraft's own Insert Table and Rename Table already refuse any defined name's spelling. Otherwise the first valid table in sheet order, then table order, keeps its spelling. Explicit references with an ambiguous original token follow that same order, except formulas of a table's own (its column formulas and the formulas in its cells, such as a calculated column or totals row) identify that table. File table IDs identify table slicers; X09b's bound pivot definition identities are retained. Ordinary strings, including literal `INDIRECT` arguments, are not rewritten. A malformed formula whose intended table cannot be parsed is not inferred from text. A bare series title is written as a reference only when it names an existing table; otherwise it stays literal text.

A bare token in a workbook-scoped defined-name body can depend on the caller's sheet. If a sheet-local name shadows that token, X17 leaves it alone rather than forcing all callers onto the table; when the table is renamed because of that local name, the token no longer reaches the table from other sheets. Use an explicit structured reference to identify the table in a caller-independent way.

Kept package content GridCraft does not parse (connections and the data model, kept chart and macro sheets' own parts) is not rewritten; only kept sheets' defined names are.

Table slicers resolve their source on import, but table slicer export is unsupported on the base and remains out of scope: they are dropped on save.

See Microsoft's [table naming rules](https://support.microsoft.com/en-us/excel/rename-an-excel-table) and [structure reference grammar](https://learn.microsoft.com/en-us/openspecs/office_standards/ms-oe376/bcd72180-31a3-423b-8f83-d224b2286da3). A scan of conventional table parts found no collisions in 1,122 public table workbooks (33,370 eligible XLSX-family rows; 19 unreadable). A second scan of 2,125 public packages with table parts (307 of them with sheet-scoped names) found no table sharing a name with a defined name of either scope. The repair is tested with synthetic packages; how Excel repairs such malformed packages remains unmeasured.

