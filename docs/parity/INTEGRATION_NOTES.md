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

## X37 — Form check boxes

Base: published X11 form drop-downs head `b435c44f`. Legacy form check boxes become `ControlKind::Check` (`CheckState` unchecked/checked/mixed, caption, link, `no_three_d`, kept macro, caption markup), following the drop-down's path end to end; cell checkboxes (`checkbox` number format, `feature_bags.rs`) are untouched.

- `crates/model/src/features.rs`: `ControlKind::Check`, `CheckState`, `CaptionMarkup`, `CheckVmlLook` and the check box fields on `FormControl`.
- `crates/xlsx/src/controls.rs`: reads x14 `formControlPr objectType="CheckBox"` (`checked`, `fmlaLink`, `noThreeD`, `lockText`) with VML fallback, and VML-only `ObjectType="Checkbox"` shapes (`x:Checked`, `x:FmlaLink`, `x:FmlaMacro`, `x:NoThreeD`, caption). Writes the properties part, worksheet control, VML and DrawingML stand-in. For ordinary names, VML keeps the name in `id` and the shared numeric shape identifier in `o:spid`, including when generated and opaque shapes share a drawing. Names that look like shape numbers rely on the worksheet control name. Plain caption whitespace and macro text survive both import paths; unsupported rich captions, conflicting types and settings outside the model stay in pass-through.
- The VML writer retains explicit caption locks and tight wrapping, and writes an imported check box back with the look its shape had (`CheckVmlLook`): paint attributes, `v:fill`, `o:lock` (including its aspect-ratio and text locks) and `v:path` as read or left out, and `x:AutoFill`/`x:AutoLine False` and `x:TextVAlign Center` as read; the properties part's `autoFill="0"`/`autoLine="0"` follow the VML. A worksheet control entry whose `defaultSize`, `autoPict`, `autoFill` or `autoLine` differs from what would be written keeps the check box in pass-through. The x14 caption lock keeps its own value when both parts exist. GridCraft does not enforce the VML aspect lock when resizing. Inch- and point-based VML styles use the same cell-anchor geometry on import. Observed paint values on explicitly unpainted shapes do not prevent editing; unsupported paint, hidden or flipped geometry, image data, and unsupported text-box settings stay in pass-through.
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
