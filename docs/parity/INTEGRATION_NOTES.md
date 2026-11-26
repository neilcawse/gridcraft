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
on their existing lines. Paste Special (including All), the system clipboard, drag-and-drop,
Insert Copied Cells, filtered and packed copies, and tiled pastes remain unmeasured and retain
their previous behavior.

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
- Inferred: a hidden row counts (its height is zero) but a hidden column does not (a width alone
  does not count). An outline level or a collapsed mark counts on either axis; this is what
  explains the extra line after a pasted hidden group (row 10, column J), whose readbacks show
  only default geometry. A line above or left of the cells extends the top or left bound too.
  With no cells and lines on only one axis, the other axis spans its first line. Thick-edge flags
  do not count.
- `saved_element` (a bare `<row>` kept for save and custom-default fitting) does not count, per
  `x25-used-bare-row`. The sizing probe that saw a trailing bare row in UsedRange used a switched
  custom-default sheet, where that row fits to a non-default height; GridCraft gives the fitted
  row a size, so it counts there by the height rule.

**Last Cell rule** (`Sheet::last_cell`). The bottom-right corner of the reported extent, stepped
back over its trailing rows and columns of zero effective size. Measured: hidden trailing row 9,
11 and column I are skipped (`H8`, `J8`, `H10`); a trailing row without a style or cells is not
(`H10` after row paste), so only hidden lines are excluded. Inferred: zero-size lines that are not
flagged hidden, and filtered rows, are skipped like hidden ones; when every line back to the
extent's first is hidden, Last Cell stays on that first line; an empty sheet gives A1.

**Consumers switched.** `document.inspect` `usedRange` (and so CLI `info`) reports the new
extent. `edit.goToSpecial` `lastCell`, Ctrl+End and Ctrl+Shift+End in the grid, and
`review.workbookStatistics` `endOfSheet` use Last Cell. The end-of-sheet field moved because
[Microsoft documents it as "End of sheet / Last cell"](https://support.microsoft.com/en-us/excel/check-workbook-statistics);
it is still `null` on an empty sheet. Every other `Sheet::used_range` caller is unchanged,
including print/PDF, CSV and other exports, calculation, charts, pivots, Ctrl+arrow navigation,
Go To Special's search scope, `sheet.read` and the grid's scroll extent. This changes published
command output: `document.inspect` and `review.workbookStatistics` can report a larger range or a
different end cell than before.

**Residual differences** (paste payload, documented in `docs/gaps.md` §25). After GridCraft's own
whole-row/column Copy or Cut the destination keeps its outline level and has no collapsed summary
line, so it reports `A1:I9` where Excel reports `A1:I10` / `A1:J9`; Paste Special All still does not
transfer line formatting at all. A copied bare or absent row leaves no GridCraft record, so
`x25-bare-row-over-untouched` and `x25-absent-row-over-untouched` still report `A1:I9`. Pending
Excel probes: OutlineLevel/ShowDetail after whole-row copy, and save/reopen of pasted bare or
absent rows.

Tests (`crates/engine/tests/it/inspect_extent.rs`) build each measured final state in code; the
outlined-group states carry the inferred outline level and collapsed summary line.
