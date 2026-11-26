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
- **Behavior change from X28:** Grouped drawings no longer follow their individual rows during the permutation. Sort holds the group's geometry and every member's sheet position, including when permuted row heights require different cell-relative anchors. Ungrouped drawings keep X28's placement behavior. A separate automatic row fit after the permutation still follows each group's anchor mode; save/reopen persists the resulting placement and undo/redo restores the corresponding workbook state.
- **Inferred, not measured:** Unchanged group bounds are consistent with unmoved members, but lookup by name failed and a swap within the same bounds cannot be ruled out. The probe used uniform row heights and did not measure permuted unequal heights, a post-sort fit that changes heights, horizontal sorts, other placement modes, or nested groups. GridCraft applies the same group policy to those permutations and treats any subsequent fit as a resize, using its existing X24 placement behavior. Excel did not write anchors or child offsets; the round-trip tests use synthetic GridCraft workbooks.
- **Scope (inferred):** The hold applies when either the outermost group's own box or the box around its members overlaps the visible sorted block. The gap between those two boxes does not count, and a block whose rows or columns are all collapsed has no visible area to overlap. A zero-size group axis counts at its start, including the near edge of the block but not the far edge. Nested groups and members follow their outermost group. Other groups follow line changes by their placement mode, such as when row heights travel with the sorted rows or a hidden row takes another height and moves the rows below it. Excel did not measure these cases.

## X38 — XLSX group projection

- XLSX import, collapsed-axis recovery after line sizing, and edited-group export share a DrawingML `a:xfrm` projection (ECMA-376 Part 1 §20.1.7.5). Source child coordinates retain the information that a zero-sized sheet axis loses. Nested transforms compose in each parent group's child units.
- Exact 90°, 180° and 270° group angles now affect member boxes even in noncollapsed groups. Other group angles keep the prior unturned geometry and import warning. This is spec-derived; Excel for Mac group layout and sizing are unmeasured. Content rotation and mirroring are not rendered by this change, and a quarter-turned group warns that its content is drawn unturned.
- The outermost group's worksheet anchor is treated as its unturned frame; after a 90° or 270° turn its members fill that box swapped about its centre, so they can extend past the anchor, and a member past the top or left sheet edge is clamped onto the sheet. A nested group's sheet box is its turned box. Whether Excel for Mac writes the anchor of a quarter-turned group as the unturned frame or as the turned box is unmeasured.
- Collapse recovery needs every member's anchor unchanged since the open; a member skipped on open (SmartArt, OLE) does not prevent it.
- Untouched groups still write their original worksheet anchor fields and group/member transforms, including grouped members' own flips, which were previously dropped. Line resizing still applies the existing placement invalidation and anchor-mode rules; hide/unhide and chart `userShapes` ownership are unchanged.
