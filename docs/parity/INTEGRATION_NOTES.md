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
- `crates/xlsx/src/{package,passthrough}.rs`: retain typed OPC relationship resolution with the PR's deny-list. Declared-empty ZIP members need no inflater; successful reads must match the declared size and charge decoded bytes. ZIP failures, including size mismatches, spend the attempted read allowance, capped at the remaining package budget. Once that budget is gone, non-empty parts are rejected before inflation. Keep table-child bindings for unmodelled sidecars without duplicating model-owned relationships and parts.
- `crates/xlsx/src/read.rs`: retain the incremental sheet-name allocator and Excel's acceptance of spaces-only names.
- Release the source shared strings and per-index phonetics immediately after worksheet import unless a retained macro sheet needs them. Chart and dialog capture do not read shared-string indexes; macro capture resolves its indexes before releasing the tables.

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

## X25 — Whole-row and whole-column paste

- [Move or copy cells, rows, and columns](https://support.microsoft.com/en-us/excel/move-or-copy-cells-rows-and-columns) describes copying or moving full lines, including their cell formats and hidden cells; it does not specify every stored line property. [Paste options](https://support.microsoft.com/en-us/excel/paste-options) distinguishes an ordinary range paste from explicit Column widths and Keep Source Column Widths options. A [Microsoft Community example](https://learn.microsoft.com/en-us/answers/questions/4770030/on-excel-how-do-i-copy-the-row-heights-on-part-of) reports that copying whole rows and pasting at a row heading also copies row heights, while copying whole columns keeps their widths.
- The XLSX used-extent gap is documented separately in [gaps.md](../gaps.md#25-used-worksheet-extent-and-whole-line-paste-special-f). The shared extent is intentionally unchanged: evidence for Excel's reported extent cannot establish its print and export bounds.

The Excel probe uses fresh synthetic sheets per case. Record both axes independently:

| Case | Source | Destination | Compare |
|---|---|---|---|
| Copy, same sheet | `3:4` or `C:D`, with one populated and one cell-less line | Existing custom `8:9` or `H:I`, first by line selection and then in a fresh case by `A8` or `H1` | Source unchanged; destination line fields and cell content |
| Copy, another sheet | Same source | Same destination on another sheet | Destination line fields and cell content; undo and redo in GridCraft |
| Repeated copy | Same source | Two different destinations | Second paste reads the original copy, not the first destination |
| Cut, same or another sheet | Same source | Same destination variants | Source and destination line fields and cell content; undo and redo in GridCraft |
| Explicit full-size range | `A3:XFD4` or `C1:D1048576` | Corresponding line selection or first cell | Compare with the whole-line syntax |
| Default over custom | Default source lines, including one cell-less line | Custom destination lines | Which destination fields reset and which remain |
| Bare record | One otherwise empty source row with a saved bare `<row>` | Untouched destination | Whether destination XML gains a row record |
| Ordinary range control | `C3:D4`, with custom source row heights and column widths | `H8`, with different custom destination height and width | Destination line sizes unchanged; cell content transfers |

For both lines in each case, record effective size, hidden state, outline level, style, fill and
bold on a blank cell, and saved line attributes (`ht`/`width`, `customHeight`/`customWidth`,
`customFormat`, style index, `hidden`, `outlineLevel`, `collapsed`, thickness and `bestFit` where
present). Use `Range.Copy Destination:=...` and `Range.Cut Destination:=...` for the Excel readback;
these do not measure the clipboard-based Paste Special variants.

Keep line-map edits inside the paste transaction so history and drawing placement see the same
change as cell paste. Do not infer a transfer policy for packed/filtered selections, Insert Copied
Cells or drag-and-drop solely from contiguous `Copy`/`Cut` readbacks: those paths share the paste
implementation but have different source or destination handling. Protected-sheet line formatting
permissions are separate from the transaction's locked-cell check, which only examines changed
cells.
