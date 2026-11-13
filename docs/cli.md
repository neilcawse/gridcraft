# gridcraft-cli

Headless GridCraft: inspect, convert, evaluate, script and serve MCP without a window. Errors go to stderr with a
non-zero exit code.

`.xls` (Excel 97-2003, BIFF8) files are imported with values, formulas, number formats, merges,
column widths, row heights and defined names; fonts, fills, borders, charts and other objects are
not imported yet. Import warnings go to stderr. Convert to `.xlsx` to save; `.xls` output is not
supported.

`.xlsb` files support [worksheet data import](xlsb-import.md). Formula cells use their last saved
values; source formatting and other workbook features are omitted. Import warnings go to stderr.
Convert to `.xlsx` to save the imported workbook; XLSB output is not supported.

`.ods` and flat `.fods` files are [imported](ods-import.md) with values, formulas (translated
from OpenFormula, with their saved results) and named ranges; source formatting and other
workbook features are omitted. Import warnings go to stderr.
Convert to `.xlsx` to save the imported workbook; ODS output is not supported.

```sh
cargo install --path apps/gridcraft-cli     # or: cargo run -p gridcraft-cli -- <subcommand>
```

| Subcommand | Example |
|---|---|
| `info <file> [--json]` | `gridcraft-cli info book.xlsx` — sheets, used ranges, cell counts, tables, charts, names. |
| `convert <in> <out> [--sheet NAME]` | `gridcraft-cli convert book.xlsx data.csv --sheet Sales` — output format from the extension: `.xlsx`, `.csv`, `.tsv`, `.json`, `.html`. |
| `eval <formula> [--in FILE] [--sheet S] [--cell A1] [--json]` | `gridcraft-cli eval '=PMT(5%/12,360,-300000)'`; `gridcraft-cli eval '=SUM(B2:B9)' --in book.xlsx` — formulas in English with `,` between arguments. A formula that couldn't be entered in a cell (it doesn't parse, or a function gets the wrong number of arguments) is an error saying why; an unknown function is `#NAME?`, as in Excel. |
| `cat <file> [--range R] [--sheet S] [--formulas] [--csv]` | `gridcraft-cli cat book.xlsx --range A1:F20` — aligned table of displayed values. |
| `display <file> [--json] [--sheet S] [--range R] [--probe FILE]` | `gridcraft-cli display book.xlsx --json` — what each cell shows (`t`: text, literal spaces and line breaks kept, with General narrowed to its column and `####` for numbers that don't fit, a number format's repeat fill (`*x`) expanded to the column as the grid draws it, with `fill`: `[char, index, count]`, `nf`, `ha` resolved alignment) and, for the cells a probe file lists (`{"sheets": [{"name", "rows", "cols", "cells": ["A1", …], "merges": ["S4:S11", …]}]}`), the effective style (own or table style with conditional formats applied: number format, font — its colour the one the text is drawn in, a number format section's colour such as `[Red]` over the font's — fill, alignment) and the borders on each edge (on an edge shared with the nearest neighbour the grid draws — hidden rows and columns are skipped, as the grid draws none of their cells — whichever of the two lines the grid draws on top: heavier, then darker; without conditional formats); also column widths and row heights (points and characters; 0 = hidden), merges (over the extent and over the probe's `merges` ranges wherever they lie), drawings (charts, pictures, shapes and slicers; a grouped one names its outermost group: `group`, `group_id`) and sheet view. Uses the grid's own display code; text widths are the font metrics of `gridcraft-fonts` (the installed face's advances, else an openly licensed metric-compatible font's), not the faces the window loads, so `####`, General narrowing and wrapped row heights right at a column edge can differ from the window. Column widths are points and characters of the Normal style font on the workbook's column device (Excel for Mac's 72 dpi on macOS, Windows's 96 dpi elsewhere; `GRIDCRAFT_COLUMN_DPI=72|96` overrides); a sheet that shows formulas lays its columns out twice as wide, as Excel does, so its widths in points are twice the stored width's and its characters are the stored width's. |
| `check <file-or-dir>... [--json] [--tolerance 1e-9] [--max-report N] [--cells]` | `gridcraft-cli check corpus/` — calc oracle and XLSX round-trip fidelity, see below. |
| `run …` | see below |
| `commands [--json] [--search X]` | `gridcraft-cli commands --search border` — every command with its parameter docs. |
| `functions [--json] [--search X] [--category C]` | `gridcraft-cli functions --search lookup` |
| `mcp [--connect PORT] [--in FILE \| --sample NAME]` | MCP server on stdio ([mcp.md](mcp.md)). |
| `send <port> <method> [json]` | `gridcraft-cli send 7979 engine.execute '{"command":"home.bold","params":{"range":"A1:C1"}}'` — one control-channel request to a running app (`gridcraft --control 7979`). |
| `version` | Prints `gridcraft-cli <version>`. |

## `run`

```sh
gridcraft-cli run [--in FILE | --sample budget|sales|grades] \
    [--cmd 'id={json}']... [--script steps.jsonl] \
    [--out FILE] [--print RANGE|used] [--sheet S] [--csv] [--formulas] [--quiet]
```

Opens a file (or a sample, or a blank workbook), runs `--cmd` and `--script` steps in the order given, then saves
`--out` and prints each `--print` range. Each non-null command result is printed as a JSON line unless `--quiet`.
The first failing step stops the run with a non-zero exit.

A `--cmd` is `id`, `id={json}` or `id {json}`. A script has one command per line in the same forms, or as JSON objects
`{"command": "cell.set", "params": {...}}`; blank lines and `#` comments are skipped.

```sh
gridcraft-cli run --in book.xlsx \
  --cmd 'home.bold={"range":"A1:C1"}' \
  --cmd 'cell.set={"cell":"D2","input":"=B2*C2"}' \
  --cmd 'edit.fillDown={"range":"D2:D20"}' \
  --out book.xlsx --print A1:D5
```

Command ids and parameters: `gridcraft-cli commands`.

## `check`

```sh
gridcraft-cli check <file-or-dir>... [--json] [--tolerance 1e-9] [--max-report 20]
```

Measures calculation correctness and file fidelity on any set of workbooks GridCraft reads with their cached values:
`.xlsx`/`.xlsm` and `.xls`, `.xlsb`, `.ods`/`.fods` (directories are searched recursively for all of them). Nothing is
written next to the inputs. A file that isn't SpreadsheetML XML is compared as it reads: its cells, formulas and cached
values, and the XLSX written from it read back; the package relationships (see *Round trip*) are compared only between
two SpreadsheetML packages (a zip from the file's first byte: an `.xls` that embeds an `.xlsx` as an OLE object is not
one). Cache trust reads `<calcPr>` where there is one; manual calculation also comes from the
reader's calculation mode (an XLS `CALCMODE` record). The producer of an OpenDocument file is its `<meta:generator>`
(see *Producer*).

**Calc oracle.** A file saved by Excel (or another calculating app) stores the app's own result beside every formula.
`check` reads each file keeping those cached values, recalculates every formula with GridCraft and compares:
numbers within a relative `--tolerance` (absolute when the magnitude is below 1), errors by kind, text and logicals
exactly. Skipped, and counted separately:

- *no oracle*: formulas with nothing to compare against, counted by cause in `noOracleReasons`. Evidence rule: a
  formula is moved here only when Excel itself, given this file and nothing else, could not produce the cached
  value, so the cache is no answer GridCraft could be asked to reproduce. Calls are classified by name against
  Microsoft's published list of Excel's worksheet functions (embedded in `check`, `excel_functions.txt`), GridCraft's own
  function table and the names the workbook defines:
  - `noCachedValue`: the file saved the formula without a value.
  - `addin`: a call to an XLL add-in function (`_xll.EURO`). The cache is what the add-in returned; Excel without it
    answers `#NAME?`, as GridCraft does.
  - `udf`: a call to a name that is not an Excel function, not defined by the workbook (a defined name holding a
    `LAMBDA`, a `LET` or `LAMBDA` variable called like a function) and unknown to GridCraft: a VBA or other
    user-defined function, or a third-party add-in's. The cache is that code's answer, which GridCraft does not run and
    Excel without it answers `#NAME?`. `_xludf.NAME` marks a user-defined call, and counts here when `NAME` is not an
    Excel function; for the name of one it stays graded (the call is a user function, #726, so GridCraft's `#NAME?`
    against the cached value is a mismatch to look at, not a reclassification).
  - `xlm`: an Excel 4 macro function (`CALL`, `REGISTER`, `EXEC`, `EVALUATE`, the `GET.*`, `SET.*` and `FORMULA.*`
    families): it runs macro-sheet code or the host's state, not the formula's arguments.
  - `externalData`: `RTD`, `WEBSERVICE`, `STOCKHISTORY`, `IMAGE`, `PY`, `DETECTLANGUAGE`, `TRANSLATE`, `COPILOT`,
    `FIELDVALUE` and the `CUBE*` functions: the value comes from a server, the data model or the web at the time
    of the calculation.
  - `downstream`: a formula that reads a cell counted under one of the causes above whose value GridCraft couldn't
    keep: the file saved none, or the engine calculated another. A call whose saved value the engine keeps gives its
    readers what Excel read, so those are graded like any other formula. One that calls such a function through a
    defined name stays here.
  - `missingSheet`: a reference to a sheet the workbook doesn't have. Excel turns the references to a sheet it
    deletes into `#REF!`, so it never saves one: the package lost the sheet after the producer calculated the cache
    (LibreOffice's empty_ext_defined_name.xlsx keeps `VLOOKUP(…,Auftragsdaten!A1:M23,…)` over two sheets its
    `docProps/app.xml` still lists, with Excel 16's cached values).

  An Excel function that GridCraft lacks is never a cause: it stays a mismatch.
- *volatile*: formulas whose result changes between two calculations of the same file, so their own cache can't be
  compared: calls to `RAND`, `RANDARRAY`, `RANDBETWEEN`, `NOW`, `TODAY`, `INFO` and `CELL("filename")`, and formulas
  that convert a literal date text without a year (`DATEVALUE("Feb 22")`, `"01-Sep"+0`, `DAY("Feb 22")`: an argument
  of `DATEVALUE`, `VALUE`, `NUMBERVALUE` or a date function, or an operand of arithmetic). Evidence rule for those:
  Microsoft's DATEVALUE "uses the current year from your computer's built-in clock" when the year is omitted, and
  arithmetic and the date functions read text dates the same way, so the result changes with the year it is
  calculated in (poi `63819.xls` cached `DATEVALUE("FEB-01")` as 2020-02-01). A text only compared or joined is not
  converted and is graded. `WEEKNUM` with a negative return type typed in the formula (`WEEKNUM(D,-1)`) is volatile
  too: the documented return types are 1, 2, 11-17 and 21, and Excel 16.114 calculating
  `WEEKNUM(DATE(2013,1,1),-1)` in four cells twice gave 1 or `#NUM!` per cell and per calculation (poi
  FormulaEvalTestData.xls cached 1). Such a
  formula that saved a value (not an array or dynamic-array formula) holds that value while `check` calculates the
  workbook, and is put back afterwards: what reads it is calculated from the value Excel's own calculation gave
  it, and is graded like any other formula. Evidence rule: Excel calculates the volatile cell before its readers
  in the same recalculation and saves the value they read, so their caches are Excel's answers to known inputs
  (`OFFSET` and `INDIRECT`, volatile to Excel's recalculation, are deterministic given their inputs and never
  counted here). Volatility still spreads from a volatile formula that saved nothing, and from defined names: a
  name counts when its definition calls such a function, uses a name that does, or refers to a cell or range that
  holds a volatile formula that saved nothing (a workbook's "as of" date: `AsOf = Sheet1!$B$1` with `B1: =TODAY()`);
  every formula that reads one, through the dependency graph and through defined names, is volatile too.
  Workbook- and sheet-scoped names are resolved as formulas resolve them, so a sheet's own `Stamp` hides the
  workbook's. `volatileViaNames` counts the formulas that are volatile only through a name.

**Cache trust.** Only a cache that is the result of a completed calculation by Excel can fail a file. `check` reads
what the file says about its own cache (the workbook part's `<calcPr>` and the producer it names) and takes it at its
word; the comparison itself always runs, and what it finds is counted in the bucket the file's trust names (JSON
`calc.cacheTrust`):

| the file says | `cacheTrust` | evidence | counted as |
|---|---|---|---|
| nothing against the cache | `trusted` | the producer calculated before it saved, and is Excel or says nothing | `matched` / `mismatched`: a mismatch gates |
| `<calcPr calcCompleted="0">`, or an `.xls` `Uncalced` record | `staleCache` | the producer states that it did not finish calculating before it saved; in practice a manual-mode workbook (`calcMode="manual"`) saved with changes pending, whose values were never brought up to date ([MS-XLS] 2.4.331: `Uncalced` "specifies that formulas were pending recalculation when the file was saved"). Excel's own full recalculation gives GridCraft's answer when GridCraft is right | `untrustedMatched` / `untrustedMismatched`: listed, not gating |
| `<calcPr fullCalcOnLoad="1">` | `fullCalcOnLoad` | the producer asks the consumer to recalculate on open: a library that doesn't calculate (xlsxwriter, openpyxl…) stores 0 or nothing, and a calculating producer's values may predate its last edit | `untrustedMatched` / `untrustedMismatched`: listed, not gating |
| a producer other than Excel (see *Producer*) | `foreignProducer` | the cache is that product's answer, not Excel's, and GridCraft models Excel. Where the products differ (a reference to an empty cell cached as `""` where Excel shows 0, a date joined to text cached as a date where Excel joins its serial, `@` over a range, a named range of LibreOffice's own) Excel agrees with GridCraft, not with the cache | `untrustedMatched` / `untrustedMismatched`: listed, not gating |

Manual calculation on its own (`calcMode="manual"` with `calcCompleted` absent or `1`) does not make a cache
untrusted: Excel recalculates before saving unless told not to, and records an unfinished calculation as
`calcCompleted="0"`. A file that gives several reasons counts under the strongest: `staleCache`, then
`fullCalcOnLoad`, then `foreignProducer`.

**Stale cells (precedent-local oracle).** In a trusted manual-mode workbook, a mismatch that no locale reproduces
(see *Locale* below) is evaluated a second time when its formula is IEEE 754 arithmetic on numbers only (`+ - * /`,
`%`, ABS, SUM, MIN and MAX over numbers and empty cells): its formula alone, on the file's own saved values for every
cell it reads (each formula cell holding the value the producer cached for it, nothing where it cached none). When
that gives GridCraft's result again and the cache is further from it than that arithmetic's rounding,
the cache is inconsistent with the file's own inputs: class `staleCell`, counted in `staleCell` (still in `checked`,
not in `mismatched`, the per-function mismatch counts or `signatures`; its signature goes to `untrustedSignatures`),
listed, not gating. Evidence rule: Excel in manual mode recalculates only the cells it marked dirty (a sheet copied
and columns inserted keeps the copied value: an Excel-saved workbook's `=ABS(T46)+ABS(J46)` cached 5608 where its own
cached inputs give 7008), and its full recalculation replaces such a value with the one its inputs give. What such a
formula gives from its inputs is fixed by the arithmetic, not by GridCraft's functions; a formula that calls any
other function, or reads text, a logical or an error, is never judged, since GridCraft evaluating it again on any
inputs reproduces GridCraft's own semantics, which is what is under test. A formula that gives the cache back from
the saved inputs stays a mismatch: its inputs differ, not its calculation. Only numbers that differ by more than
rounding can make (`1e-7` of the larger, or of 1 near zero) are judged. Nor is a formula with a subnormal number
anywhere in its calculation (read, written or computed, or possible in a partial sum of SUM terms below 2^-970):
Excel "does not support denormalized numbers" and underflows them to 0 (Microsoft, "Floating-point arithmetic may
give inaccurate results in Excel"; Excel 16.114 gives 0 for `=1E-307/10*1E307*10`, IEEE 754 gives 1), so IEEE 754
arithmetic there isn't Excel's and reproduces only GridCraft. Array formulas, dynamic-array formulas and
workbooks that spill (spilled cells hold GridCraft's values, not the file's) are never judged. Automatic mode never
has stale cells.

**Older Excel versions (version oracle).** A trusted mismatch that the version of Excel which calculated the cache
explains is class `legacyExcelVersion`, counted in `legacyExcelVersion` (in `checked`, not in `mismatched`, its
signature in `untrustedSignatures`), listed, not gating. The version is the XLSX `<calcPr calcId>` (the calculation
engine: `124519` → 12, Excel 2007), or else the producer's `version` (see *Producer*), for a file whose producer is
Excel. Five evidence rules:
- *An algorithm Excel 2010 replaced*: Microsoft's "Changes in Excel 2010" lists the statistical distributions whose
  algorithms were "modified or redesigned completely for better accuracy" (BINOMDIST, CRITBINOM, CHIDIST, CHIINV,
  EXPONDIST, FDIST, FINV, GAMMADIST, GAMMAINV, HYPGEOMDIST, LOGNORMDIST, LOGINV, NEGBINOMDIST, NORMDIST, NORMINV,
  NORMSDIST, NORMSINV, POISSON, TDIST, TINV, WEIBULL) and the functions "for which the accuracy is improved" (ASINH,
  CEILING, CONVERT, ERF, ERFC, FLOOR, GAMMALN, GEOMEAN, STDEV, VAR); "What's new: Changes made to Excel functions"
  (*Functions with accuracy improvements*) adds BETADIST, BETAINV, CUMIPMT, CUMPRINC, FACTDOUBLE, IMLOG2, IMPOWER,
  IPMT, IRR, LINEST, MOD, PMT, PPMT and XIRR; CONFIDENCE, ZTEST, TTEST, CHITEST and FTEST are defined over those
  distributions. A formula that is one call of such a function (in parentheses or not), cached by Excel 2007 or
  older (version < 14), is the old algorithm's result when its number differs from GridCraft's by at most `1e-4` of
  the larger (or `1e-6` absolute near 0). A larger formula is the old algorithm's only for a number, and only as far
  as its arithmetic (the operations and functions of *Cancellation residue*, what it reads within the tolerance)
  carries each such call taken within that accuracy to the result: `=NORMSINV(p)*2+1` within twice it,
  `=X+0*IRR(…)` not at all, so its mismatch gates, and a formula that isn't arithmetic around the call
  (`=IF(…,GAMMADIST(…))`) has no such bound.
  An error cache is only ever the older answer of a call that real older-Excel evidence backs (a cache that version
  saved, or a cited published table), for the inputs it is evidenced for; no other error is put in a call's place,
  and a function or input with no entry below is never substituted:
  - `PMT` with no periods (`nper` 0): `#DIV/0!` to Excel 2007 and older, `#NUM!` since 2010 (two workbooks
    calculated by Excel 2007 cache `#DIV/0!`; Excel 16.114 gives
    `#NUM!` for `PMT(0,0,0,0,0)`);
  - `IRR` that has no result: `#DIV/0!` instead of `#NUM!` before Excel 2003 (Microsoft KB 30567, "XL: IRR Returns
    #DIV/0! Instead of #NUM! If No IRR Exists", Excel 97, 2000 and 2002);
  - `IRR` where GridCraft finds a root: `#NUM!` for Excel 2007 and for the flows (default guess) that an Excel 2007
    cache shows giving up: the first two to seven flows of an Excel 2007 workbook's row 54
    (`IF(ISERROR(IRR($F54:G54)),"NA",IRR(…))` cached `NA` for -3600000, 203454.98, … and the number from the eighth flow
    on). That is all the evidence there is, and no model of the search stands in for it: Newton on the rate (the
    "20 tries" of the documentation) gives up on `{-5,1}` and `{-100,50}`, which Excel 16.115 solves exactly (-0.8, -0.5:
    Excel iterates on the discount factor `1/(1+r)`, like `irr_discount_newton`), and on -85, 120, 100, -138 at a guess
    of 22%, which Excel 2002 itself cached as 0.45188 (a published homework answer key). Where Excel 16.115 answers
    `#NUM!` and GridCraft's fallback search gives a number (a workbook's Ret!K12
    and K24, whose flows Excel 2007 isn't known to give up on), the number is GridCraft's own difference and gates;
  - `NORMSINV` and `NORMINV` have no entry: the tail's `#N/A` of the Excel 97 help ("does not converge after 100
    iterations") has no cache or table behind it. Excel XP and 97 gave numbers (KB 826772; McCullough and Wilson
    2005, Table 1: -4.75367 at `p` 1E-6, -4.99152 at 3E-7, -5.06928 at 2E-7; Knüsel: -7.15 and -5.0000 for 3E-7 and
    2E-7 in Excel 97), which are still the old algorithm's within its accuracy; a cached `#N/A` there, or an
    `ISNA` of it, gates, in Excel 2002, 97 and 2007 alike. `NORMSINV(0.5)` is 0 in every version;
  - `MOD` where GridCraft finds a number: `#NUM!` when the number is at least 2^27 (134,217,728) times the divisor,
    Excel 2003 and older only (Pearson, "The MOD Function": gone in Excel 2007); `MOD(5,0)` is `#DIV/0!` in every
    version.
  XIRR, BETAINV, CHIINV, FINV, GAMMAINV and TINV have no documented failing input and are not substituted (their
  numbers are still the old algorithm's within its accuracy).
  A larger formula is the old algorithm's answer when the formula proves it, with the call replaced by an evidenced
  answer, and nothing else differing: calculated as GridCraft does (every input holding GridCraft's value, as for the
  SUMPRODUCT reading below), it gives the cache back with the older answer in the call's place, differs from
  GridCraft's own value there (the call is what the formula depends on), and gives GridCraft's value again with the
  calls held at their current values. `=MATCH("a",B1:B2,0)+MOD(5,0)` cached `#N/A` (GridCraft 1 for the MATCH) gates,
  as does `+NORMSINV(0.5)`: no call has an older answer for those inputs, and the operand that differs is not one.
  The call's own arguments are judged separately, because the held-values control swaps the whole call and so says
  nothing about an operand inside it (`=NORMSINV(MATCH("a",B1:B2,0)*1E-7)` cached `#N/A`,
  where only the MATCH is wrong): a call is replaced by its older answer only when every argument is verified, i.e.
  a literal, an omitted argument, or a reference to cells that are constants or formulas whose cache GridCraft's
  value matches, with parentheses, signs and `+ - * / ^` over those. A call, a defined name, a whole row or column, or
  an array or another workbook's cell in an argument is not verified, so such a call is never excused (the narrowest
  sound rule: nothing compares GridCraft's value for the argument with Excel's). A bare MATCH cell cached `#N/A`
  gates, and so does every cell that reads it through the arguments of a call.
  `=(PMT($A$90/12,$B$90*12,-$C$90,0,0)*12)/Income!$L$25` over empty inputs is `#DIV/0!` in an Excel 2007 workbook,
  `#NUM!` in GridCraft and in Excel 16.114: with `#DIV/0!` in the PMT's place the formula gives its cache, so
  that cache is Excel 2007's. A formula the replacement doesn't give the cache back for gates, and so does any Excel
  2010 or later.
  Microsoft documents the old searches' inaccuracy without bounding it (KB 826772, NORMSINV in Excel 2002 and
  earlier); the corpus's Excel 97–2003 caches differ by at most 9e-5 relative (NORMINV).
  A normal density that underflows is another case: a workbook calculated by Excel 2007
  caches `#NUM!` for `NORMDIST(220,P1,P2,0)`, a density below 2.2250738585072E-308, where Excel 16.115 (recalculated
  copy of the file) and GridCraft give 0 (no denormalized numbers: it underflows). A formula that is one call of
  `NORMDIST` with a FALSE cumulative argument, cached `#NUM!` by Excel older than 2010, where GridCraft gives 0
  and the density `exp(-z²/2)/(sd·√(2π))` of its arguments is below that smallest normal number, is the old
  algorithm's answer; the cumulative form, a density that doesn't underflow, and any Excel 2010 or later gate.
- *An algorithm Excel 2003 replaced*: "Description of improvements in the statistical functions in Excel 2003 and in
  Excel 2004 for Mac" (KB 828888) lists LINEST's QR decomposition and its callers LOGEST, TREND and GROWTH; the
  two-pass sum of squared deviations of VAR, VARA, VARP, VARPA, STDEV, STDEVA, STDEVP, STDEVPA, PEARSON, SLOPE and
  STEYX and their callers FORECAST, INTERCEPT, RSQ, TTEST and ZTEST; and DVAR, DVARP, DSTDEV and DSTDEVP. A formula
  calling one, cached by Excel 2002 or older (version < 11), follows the same accuracy rule. Excel 2000's one-pass
  formula `(nΣxy − ΣxΣy)/(nΣx² − (Σx)²)` reproduces a financial workbook's SLOPE caches bit for bit where the
  two-pass slope differs by 1e-8.
- *A function the calculating version lacked*: `#NAME?` cached for a function files write as `_xlfn.NAME` (one newer
  than Excel 2007's file format), where GridCraft gives a value: an Excel that has the function never names it
  `#NAME?` (Microsoft: an `_xlfn.` prefix shows "when the workbook contains a function that is not supported in the
  version of Excel you're running"). MINIFS cached `#NAME?` by an Excel 2016 without it is the example.
- *Typed text before Excel 2016*: `#VALUE!` cached by Excel 2013 or older (version ≤ 15) for a formula that is one
  call of MODE, MODE.SNGL, MODE.MULT, AND, OR or XOR (in parentheses or not) with a text typed in it as an argument,
  where GridCraft gives a logical (AND, OR, XOR) or anything but `#VALUE!` (MODE). The same formulas cached by both
  versions show the change: `MODE(B8,"1",2,3)`, `MODE("1",1,#DIV/0!,2,3)` and `AND("1",1)` are `#VALUE!` in poi
  FormulaEvalTestData_Copy.xlsx (Excel 15.0300) and `#N/A`, `#DIV/0!` and `TRUE` in FormulaEvalTestData.xls
  (Excel 16), which skips the typed `"1"`; Excel 16.114 calculates `AND("1",1)` as TRUE too. A call without typed
  text, or one inside a larger formula (`X+0*AND(TRUE)`), is not this change and stays a mismatch.
- *A reference to one cell in SUMPRODUCT before Excel 2016*: Excel 2013 and older (version ≤ 15) read a reference that
  holds one cell, however it is written (`G1330`, `G1330:G1330`, `INDEX(…)`, `INDEX(…):INDEX(…)`), as the value in the
  cell, so text there made SUMPRODUCT `#VALUE!`; Excel 2016 and later read it as an array, whose text counts as 0, as
  GridCraft does (Excel 16.114: `SUMPRODUCT(text,number)`, a cell holding the empty text a formula returned and one
  holding a number, is 0 for all 162 ways of writing the two cells, each as `A1`, `A1:A1`, `INDEX(…)`,
  `INDEX(…):INDEX(…)`, `OFFSET(A1,0,0)`, a defined name, `(A1)`, `A1:INDEX(…)` or `INDEX(…):A1`, in both orders). A
  mismatch cached by an Excel of version ≤ 15 for a formula that calls SUMPRODUCT is `legacyExcelVersion` when the
  formula, evaluated on the values GridCraft calculated with SUMPRODUCT taking a reference to one cell as the value in it
  and nothing else changed, gives the cache back. Every input keeps GridCraft's value, whether the file cached it or not,
  so the older reading is the only difference from GridCraft's own calculation, and it is the proof: it holds wherever
  the call is in the formula (inside IF, as the dividend of a division) and whatever the cache is; a cache the older
  reading doesn't give back gates, as does a formula that differs from its cache for any other reason.
  `=A1+0*SUMPRODUCT(C1,D1)` cached 0 over an `A1` that GridCraft calculates as 1 where the file's Excel had 0
  (`=1E-307/10*1E307*10`: Excel has no subnormal numbers) is that: both readings of the SUMPRODUCT of two numbers give the
  same 0, the cache follows from the file's `A1` and not from GridCraft's, and it gates, with an `A1` that has a cache or
  one that has none. Two Excels' caches of the same formula: `SUMPRODUCT(G1330:G1330,I1330:I1330)` over the text "4" and
  2 is `#VALUE!` in FormulaEvalTestData_Copy.xlsx (Excel 15.0300) and 0 in FormulaEvalTestData.xls (Excel 16). Two
  workbooks saved by Excel 15.0300 (calcId 152511) cache `#VALUE!` in 156 cells
  each of `IF(OR(…),"",IF(…,@INDEX(…),SUMPRODUCT(INDEX(…):INDEX(…),INDEX(…):INDEX(…))/SQRT(SUM(INDEX(…):INDEX(…))*SUM(INDEX(…):INDEX(…)))))`,
  where the first `INDEX(…):INDEX(…)` is one cell that holds `""`. Excel 16.114 calculates a replica of that formula
  (the same names, ranges and the cell holding `""`, opened from a file) as `#DIV/0!`, as GridCraft does: SUMPRODUCT is
  0 and its divisor `SQRT(0)`. A division gives the error of its left operand first, which is how the older
  SUMPRODUCT's `#VALUE!` became the result (Excel 16.114: `=NA()/0` is `#N/A`, `=("a"+1)/SQRT(0)` `#VALUE!`,
  `=(1/0)/NA()` `#DIV/0!`, `=NA()/(1/0)` `#N/A`, `=1/SQRT(0)` `#DIV/0!`).

A trusted mismatch that reads such cells is `legacyExcelVersion` too when its formula, evaluated on the values GridCraft
calculated except that each such cell holds the value the file saved for it (and each number GridCraft agrees with the
cache on, to the tolerance, holds the cache's digits: a formula that multiplies a tolerated difference reads what Excel
computed it from), gives its cache back (an array formula's cell holds the first value of the array; a formula that
calls SUMPRODUCT is read as the calculating Excel read it, above): its cache was calculated from the older version's
results, and what it does with them is GridCraft's calculation too. Every other input keeps GridCraft's value, another
mismatch or a cell the file never cached, so an input GridCraft calculates differently from Excel keeps the formula from
giving its cache back. One that doesn't give its cache back (`=X+0*A1` over a legacy A1, `=IF(ISERROR(A1),0,1)+X` over
an `X` GridCraft gets wrong, cached or not) stays gating. The formula is judged on the cells it reads, not on the range
it reads them from: an `INDEX` over a block that holds other mismatches is not held open by them.

**Circular references (loop oracle).** With iterative calculation off (`<calcPr iterate>` unset, an XLS without
`ITERATION`), a trusted mismatch on a circular reference the calculation finds, or reading one (through the dependency
graph or defined names, however far downstream), is class `circularReference`, counted in `circularReference` (in
`checked`, not in `mismatched`, its signature in `untrustedSignatures`), listed, not gating. Evidence rule: Microsoft
("Remove or allow a circular reference"): with `=D1+D2+D3` in D3 "Excel can't finish the calculation because D3
depends on D3". What the loop's cells and their readers cached is what they held when Excel stopped, which depends on
the order it took them in (an Excel-saved workbook: `J46: =SUM(C46:J46)` read by `Y46: =ABS(T46)+ABS(J46)`).
With iterative calculation on, the loop is calculated to its convergence and graded.

**Producer.** JSON `producer` has the `Application` and `AppVersion` of the package's extended properties
(`docProps/app.xml`, found through the package's `extended-properties` relationship, Strict or Transitional spelling)
and the `kind` they name: `excel` (Microsoft Excel, Excel for Android/iOS), `excelMac`, `excelOnline`, `libreOffice`
(LibreOffice, Collabora), `wps`, `onlyOffice`, `poi`, `googleSheets`, `gridCraft` (a package GridCraft saved),
`other` (any other named product: a library, a converter, Access; a name that says it is only Excel-compatible, like
`Microsoft Excel Compatible / Openpyxl`, is `other`) and `unknown`. A package that names no application is `googleSheets`
when the root of its workbook part declares both the Mac Excel 2008 VML namespace (`urn:schemas-microsoft-com:mac:vml`)
and the Excel 2006 `xm` namespace (`http://schemas.microsoft.com/office/excel/2006/main`): Google Sheets writes no
`docProps` at all and Excel never declares `xm` on a workbook. Otherwise it is `unknown`. A package whose custom
properties (`docProps/custom.xml`) hold `KSOProductBuildVer` is `wps` (with that build as `version` when there is no
`AppVersion`): Kingsoft WPS Office tags every workbook it saves with it, writing `Microsoft Excel` as its `Application`,
and an Excel save keeps both the tag and WPS's values, since Excel recalculates on open only a workbook whose `calcId`
is older than its own engine, and otherwise only the cells it marked dirty. A package that names Excel, or no
application, is the product its workbook's `<fileVersion appName>` names when that isn't Excel's `xl` (ECMA-376
§18.2.10: "the application that last saved the workbook"): `Calc` is `libreOffice`, `gridcraft` (what GridCraft
writes when the workbook had none) `gridCraft`, anything else `other` (`SpreadsheetGear …`, `SpreadsheetLight`,
`HCell` in the corpus). One that names Excel and puts `xml:space` on the
workbook root is `other`: Excel never does (none of the corpus's 29,000 other Excel-named packages), a library that
writes Excel's name with Excel 2007's `fileVersion` and no calculation chain does (322 corpus packages). An
OpenDocument file (`.ods` `meta.xml`, or a flat `.fods` itself) names its producer in `<meta:generator>`; one that names
none is `libreOffice` when the root of its document (`content.xml`, or the `.fods`) declares one of LibreOffice's
extension namespaces (`urn:org:documentfoundation:names:experimental:…`: `loext`, `calcext`…), which LibreOffice
declares whenever it saves (LibreOffice's own test documents strip `<office:meta>`), and `other` otherwise: Excel's
OpenDocument export always names itself (`MicrosoftOffice/16.0 MicrosoftExcel/CalculationVersion-…`, in every
Excel-saved ODS of the corpus), so a document that names nobody wasn't Excel's and its cache isn't Excel's answer
(LibreOffice's solver fixtures save `office:value="0"` on every formula). An `.xls` names its application in
its compound file's `SummaryInformation` property stream ([MS-OLEPS] `PIDSI_APPNAME`: `Microsoft Excel`); its
`version` (spelled like `AppVersion`, `11.0000`) is the major version in the high word of
`DocumentSummaryInformation`'s `PIDDSI_VERSION`, or else, when the application is Excel, the BIFF8 BOF's
`verLastXLSaved` ([MS-XLS] 2.4.21: 0 Excel 97 … 3 Excel 2003, 4 Excel 2007, 6 Excel 2010, 7 Excel 2013), a field other
writers leave 0, or for an Excel 5.0/95 (BIFF5) workbook stream, which has no field naming the Excel that wrote it,
the format's own version (`7.0000`, Excel 95): a later Excel saving down to it caches its own results, so the date
only matters where the cache differs from GridCraft's by an older algorithm's inaccuracy (see *Older Excel
versions*). An `.xls` that names no application but whose `PIDDSI_VERSION` carries an Office build in its low word
(Excel 98 for the Mac writes `0x0008_133A` and no `PIDSI_APPNAME`) is `excel`: of the corpus's 4,300 BIFF8 property
sets with a build that name an application, every one names Excel, and LibreOffice writes build 0. The producer is read from the
file only, never from the file's name or where it came from. Every kind but `excel`, `excelMac`, `excelOnline`,
`gridCraft` and `unknown` makes the cache `foreignProducer`. A package that names no producer, or that GridCraft saved,
stays `trusted`: nothing in it argues against the cache. An `.xls` whose every FORMULA record sets `reserved1` (bit 1 of
its flags, which [MS-XLS] 2.4.127 says "MUST be zero", so Excel writes 0; not read at all in an encrypted stream) is `other`
whatever its `SummaryInformation`
says (LibreOffice and POI write it set; 19 of 400 clean corpus `.xls` do, all LibreOffice- or POI-written). So is an
`.xls` (unencrypted, or read decrypted when Excel's default password protects it) whose `WriteAccess` user name isn't the string its BIFF version writes there ([MS-XLS] 2.4.349
an XLUnicodeString in BIFF8; a byte string with an 8-bit length in the BIFF5/7 `Book` stream, as the Excel 5.0/95
format writes it): xlwt writes its default owner `None` as bare padded bytes and caches every formula as an empty
string, JExcelAPI writes `Java Excel API`; Excel writes the length-prefixed string (5,127 of 5,147 corpus BIFF8
files; the other 20 are encrypted, xlwt or JExcelAPI). So is an `.xls` with a NUMBER record in a worksheet
for a value an RK record holds ([MS-XLS] 2.5.217: a 30-bit integer, a double whose 34 low bits are zero, or either
divided by 100 where `x * 100` is integral and `i / 100 == x`) and no RK or MULRK record in any worksheet: Excel writes
RK for those, and POI's HSSF, which turns every RK it reads into a NUMBER, keeps the summary streams of the Excel
file it started from. A workbook it re-saved holds the formula results of the file before its data cells changed
(poi 44861.xls: every `=data!A5` caches 0 next to a `data!A5` that holds `France`). Of the corpus's 4,734 `.xls`
that name Excel, the four with such a NUMBER have no RK record at all, and are POI's; none of the 3,589 with an RK has one.

Untrusted comparisons never leave the report: the cell counts are in `untrustedMatched` and `untrustedMismatched` (per
file and in `totals`, with a per-class breakdown in `totals.untrusted`), `untrustedSignatures` has the complete
`expected type → actual type` counts, and every mismatch is listed (up to `--max-report`) with its `class`. They stay
out of `checked`, `matched`, `mismatched`, the per-function and signature roll-ups, and the exit code. The `calcPr`
values read (`mode`, `calcCompleted`, `fullCalcOnLoad`, `iterate`, `calcId`) are reported as `calcPr`.

A logical result the producer cached as the number 1 or 0, without `t="b"` (Business Central's "Open in Excel"
exports write `<c><f>TRUE()</f><v>1</v></c>`), holds the same value: it matches GridCraft's TRUE or FALSE and is also
counted as *booleans saved as 1/0* (`boolAsNumber`). Excel types logical results, so on files it saved this count
should be 0; a non-zero count there would point at a formula where GridCraft returns a logical and Excel a number.

Three more comparisons accept a cache that holds the same answer in another spelling. Each is counted apart (the
cells are also in `matched`, or `untrustedMatched`), so a surprising number stays visible, and each states the Excel
behaviour that makes the two values one answer:

- *Errors saved as text* (`errorDetailCache`). Google Sheets writes an error as the text `#N/A`,
  `#N/A:lookupNotFound:Toxic` or `#VALUE!:notNumber:FILL IN` in a `t="str"` cell; the part before the first colon is
  an error code, the rest says why. In a workbook Google Sheets wrote (see *Producer*), the text matches when its
  code is the error GridCraft computes: Excel shows the code and drops the reason, and a full recalculation in Excel
  gives the same error. A different code, or text that isn't an error code, still mismatches. From any other
  producer a text is a text result, whatever it spells (Excel writes an error as `t="e"`), and differs from an error.
- *Whitespace a writer other than Excel dropped* (`whitespaceCache`). In a workbook another product wrote (WPS,
  LibreOffice…), a cached text that is the result without the spaces, tabs and line breaks around it matches when the
  cell's `<v>` has no `xml:space="preserve"`: such writers save `CONCATENATE(A2," ",B2," ")` as `AA AB`. Excel writes
  the attribute (Excel 16.114 saves `="x "&""` as `<v xml:space="preserve">x </v>`), so from Excel, or from a package
  that names no producer, a cache without the whitespace is a different text. A cache with whitespace the result
  lacks is never this. `check` reads the attribute from the sheet XML of the file itself, as the importer's model
  has no place for it.
- *Solvers* (`solverTolerance`). `IRR`, `XIRR`, `RATE`, `YIELD`, `ODDFYIELD` and `ODDLYIELD` find their result by
  iteration and Excel documents their accuracy (0.00001 percent), not their last digit: a solver call is taken as
  its result within a relative 1e-7 (different iteration schemes, or versions of Excel, agree to about 1e-8), and
  that is carried through the formula like the rounding below: `=IRR(A1:A3)` and `=B1*2` over it match within
  that accuracy (twice it for `B1*2`), `=0*IRR(…)+X` carries none of it and is graded as `X`. A formula reads a
  solver's accuracy only from a cell already graded a match: a reader of `=X+0*IRR(…)` that stays a mismatch gets
  nothing from it, however close to its cache it came (`=A1*1E8` over such an A1 5e-8 off is a mismatch 5 off). A
  reader of a solver whose formula isn't arithmetic (`=NPV(A10,A2:A6)` over an IRR in A10) matches when it gives its
  cache back from the file's saved values and every input it reads whose cache differs from GridCraft's is a solver
  mismatch settled so: what differs is the solver's accuracy, carried in.
- *Cancellation residue* (`cancellationResidue`). A difference within the rounding error the formula's own
  operations propagate (a running error analysis of `+ - * /`, `^`, `%`, EXP, ABS, SUM, MIN and MAX (over a
  range, or between two names of one cell each, `TOP:LOW`), a defined name
  that is one absolute cell, and IF over a condition no rounding can flip: a comparison whose sides are further apart
  than the errors they carry, OR, AND, NOT, and ISERROR of a cell or name; `x^n` carries `n` times `x`'s relative error and
  `n+1` roundings, any other power and EXP carry a few units in the last place of their own, as Excel's
  `e^(y·ln x)` isn't GridCraft's exponential): every operation adds the
  rounding of its result (one unit in the last place of a double, for two implementations) to the errors it carries
  in, a constant it reads is exact, and a formula it reads carries the difference between its cache and
  GridCraft's value when that is within the tolerance or the cell is itself graded a match (a solver's, residue
  or an iterative loop's); one still a mismatch leaves its readers unbounded. The formula's last operation, when it
  is a sum or difference no larger than `1e-14` of its larger operand, may be Excel's 0 (Microsoft,
  "Floating-point arithmetic may give inaccurate results in Excel": Excel compensates results "at or very close to
  zero"); no other operation is compensated: Excel 16.114 gives 0 for `=0.5-0.4-0.1` and `=(0.5-0.4)-0.1` but
  -2.78E-17 for `=(0.5-0.4-0.1)`, `=1*(0.5-0.4-0.1)`, `=ABS(0.5-0.4-0.1)` and `=SUM(0.5-0.4-0.1)`, and 1E15 for
  `=((1E15+1)-1E15)*1E15`. So `=B18-E18` cached 0 where GridCraft gives 3.3e-9 matches when
  B18 and E18 carry that much between them (an Excel-saved workbook: 1219956.1391067398 cached for both, GridCraft 3e-9
  apart), while `=2+0*A1` over `A1=1E20` carries nothing and `A1-B1` over two equal constants must be 0. A formula
  that calls anything else, reads text, a logical or an error, or has a subnormal number in its calculation (see
  *Stale cells*) is unbounded and its mismatch stands.
- *Iterative loops* (`iterationChange`). With iterative calculation on, a number on a circular reference, or reading
  one, within Maximum Change (`<calcPr iterateDelta>`, 0.001 by default) of GridCraft's, whose formula evaluated once
  on the file's saved state gives the cache back, or moves it no further than moving the loop by Maximum Change
  moves the formula: Excel stops iterating once no value changes by more than that ("the
  maximum amount of change between recalculation results") and caches the pass after the last, so to first order
  its cache is that far from one more pass (poi `57535.xlsx`). The loop is moved one cell at a time (each cell the
  formula reads that is on or reads the loop, higher and lower by Maximum Change, the larger move of the two) and
  the moves add up, so `=H24-H33` over two cells that settle together isn't held to their difference alone. The
  loops are found by calculating the workbook with iteration off, as a loop closed through a defined name
  (an engineering workbook's `RECYR3` = `Flowsheet!$E$17`) or a lookup isn't in the dependency graph. A formula that doesn't read
  the loop's state (`=B1*0+X`) moves by nothing: its cache must be its value. The formula is evaluated as arithmetic
  (the operations and functions of *Cancellation residue*, each cell it reads exact), never by GridCraft's
  evaluator, and GridCraft's own value of it on the saved state must be that arithmetic's: a sensitivity measured
  with the evaluator under test would excuse its own errors (`=A1*0.5+SQRT(4)*0.0002` whose cache leaves out the
  SQRT term). A formula that calls any other function, reads its own cell, or has a subnormal number in its
  calculation stays a mismatch.

`#CALC!` and `#SPILL!` need no such comparison. The cell types of the file format have no code for them, so Excel
writes such a cell as `t="e" vm="N"` with `<v>#VALUE!</v>` and gives the true error in rich value `N`
(`xl/metadata.xml` → `xl/richData/rdrichvalue.xml`, a value of the `_error` structure). The importer reads these cells
as their true error, so they compare plainly; a `#VALUE!` without a rich value is `#VALUE!`.

**Locale.** A cache can be the answer of an Excel with other regional settings: `INT("01/09/2020 15:02:40")` is
1 September in en-GB, `TEXT(A1,"mmmm")` gives `enero` in Spanish, `TEXT(0.15,"0.0%")` gives `15%` in Italian (`.`
groups thousands there), `ÅÅÅÅ` is the year in Swedish. GridCraft calculates as the en-US Excel does; when a trusted
cache has mismatches, the workbook is recalculated afresh under each candidate locale (en-GB, en-IE, de-DE, fr-FR, es-ES,
it-IT, pt-BR, nl-NL, sv-SE, tr-TR: date order, decimal and thousands separators, currency symbol, month and day
names, the Swedish year letter, and for fr-FR the `info_type` names of CELL that Microsoft's French page lists
(`CELL("adresse",AV3)` caches `$AV$3` in poi StringContinueRecords.xls, `#VALUE!` in en-US Excel);
`crates/core/src/locale.rs`). Evidence rule: a locale counts only when every cell
that matched under en-US still matches under it and it reproduces at least one mismatch exactly (the same comparison,
`--tolerance` included); of those, the one reproducing the most wins (ties: the order above). The mismatches it
reproduces are matched, counted in the file's `localeDependent`, and the file reports the locale's tag as `locale`.
Every other mismatch gates as before, so a cache partly from another Excel, or a locale effect GridCraft doesn't model
yet (localized function names in a cache, `TRUE`/`FALSE` words), stays a mismatch.

Each mismatch lists the cell, formula, expected (cached) and actual (GridCraft) values and the functions the formula
calls. Mismatches roll up by function: *checked* (formula cells calling it), *mismatched*, and *primary* (mismatched
while none of the cell's precedents mismatch — the likely culprit, as opposed to a downstream effect). They also roll
up by *signature*, `expected type → actual type` (e.g. `number → #SPILL!`), which often groups one root cause.

**Round trip.** The recalculated workbook is written with GridCraft's XLSX writer, under the checked file's own
package kind (`.xlsm` stays `.xlsm`, so its VBA project is expected back; `.xltx`/`.xltm` likewise), validated (every
part has a content type, every content-type override and internal relationship target exists, relationship ids are
unique: what Excel would "repair"; a target that is only a fragment, `#Sheet1!A1`, points inside its own part and has
nothing to miss), compared with the source and read back. Categories compared: `package`, `sheets`
(names and order), `values`, `formulas`, `numFmts` (per cell), `merges`, `names`, `tables` (name, range, columns),
`condFormats` (each sheet's rules in priority order, priority numbers aside), `validations`, `charts` (kinds and
series counts), `comments`, `hyperlinks`, `pivots`. Sheets are paired by name (then, among the sheets left on both
sides, in order), so one dropped sheet doesn't shift the comparison of the rest. `#SPILL!`/`#CALC!` compare equal to
`#VALUE!` and `#CIRC!` to 0, as that's what the file format stores.

`package` compares the relationships *owner by owner*. The owners are the package, the workbook, each sheet and every
part reached from them (a sheet's drawing, a drawing's charts…), found by following relationships from the package
root, so losing one of four drawings shows. A kind is lost at an owner when the source had it there and the output
doesn't: `lost relationship printerSettings in sheet "Data"`. For charts, tables, PivotTables and the sheets that
"Data" > drawing (1 of 3)`. Each excused case is a rule with the Excel behaviour it rests on, and nothing else is
excused:

- *Rebuilt, so not a loss:* the calculation chain (Excel rebuilds it), chart style / colour / user-shape sidecars and
  theme overrides (written inline), PivotTable cache records (rebuilt with the cache), shared strings (written from the
  cells; absent when no string is left) and the workbook metadata that only flags dynamic-array formulas (rebuilt from
  the formulas it flagged).
- *Holds nothing, so not a loss:* a link to a place in the workbook (`#Sheet1!A1`, written as the cell's `location`),
  a relationship to a part the package doesn't contain, to an XML part with no content (a comments part without
  comments, a drawing or VML without shapes) and the relationships of parts nothing leads to (Excel drops them on save
  too). A part that isn't XML (printer settings, a VBA project, a thumbnail) is content, unless the zip directory
  declares it 0 bytes: that part is empty whatever its stream looks like (an empty thumbnail written as a deflate entry
  with no stream can't be decoded, and is not an unreadable part). A part that holds bytes and can't be read counts as
  content, so a file whose such part GridCraft can't carry reports it as lost.
- *Never used, so not a loss:* a relationship of a drawing or a chart that the part's XML never names (each picture,
  chart and hyperlink of DrawingML is used through the `r:embed` / `r:link` / `r:id` its markup holds, ECMA-376
  Part 1 §9.2; POI's `npe.xlsx` holds an image relationship no picture names), a sheet relationship of the workbook
  that `<sheets>` doesn't list (`r:id=""`), and a workbook pivot cache that no PivotTable reached from the workbook
  uses (Excel deletes a cache together with the last PivotTable using it).
- *The same kind under another spelling:* Strict OOXML names kinds like `extendedProperties` where Transitional uses
  `extended-properties`; kinds compare without case, `-` and `_`.
- *Decided once:* a dropped `docProps/thumbnail.*` is reported as a loss, not excused as rebuilt, so that preserving
  it stays a fix to make.
- *Required by Excel:* a table's header cell has to hold its column's name, so a blank header cell that comes back
  holding the name of its table column isn't a `values` difference.

The comparison has limits, and reaching one is a difference, never a pass: a check that stopped short has listed only
the relationships it reached, so it can neither find a loss nor rule one out. It follows a chain of at most 12 owners,
looks at at most 5,000,000 relationships and 200,000 owners in a package of at most 200,000 entries, and reads at most
512 MiB of decompressed XML in all: every relationship part, and of each target only as far as the first element that
decides whether it holds anything (what follows that element is not read, and doesn't count against the limit). A
relationship part larger than 64 MiB is over the limit too. When a package reaches one, `package` lists
`relationships of the checked file not compared: relationship check stopped at its limit of 12 owners deep` (the
others read `…its decompression limit`, with the part it was reading in front, `…its limit of 5000000 relationships`,
`…of 200000 package entries` and `…of 200000 owners`), with `the written file` in place of `the checked file` for a
limit reached by the XLSX GridCraft wrote. The round trip then fails, as it does for a lost relationship.

Files that fail to open are reported with GridCraft's error and don't stop the run. Unreadable directories,
directory-entry/metadata errors, and subtrees or files omitted by the depth (32) and file (100,000) limits appear
as failed path reports too, in both human and JSON output. Intentional hidden/lock-file exclusions remain ignored.
JSON `totals.files` includes failed discovery paths; `totals.opened` counts only workbooks that opened successfully.
An empty scan fails with a diagnostic. Exit code: 0 when at least one file was checked, discovery was complete, and every file
opened with no mismatch and no round-trip difference, 1 otherwise, 2 on a usage error. `--max-report` caps the
examples listed per file and category (counts are always complete). `--cells` adds `calc.cells` to the JSON: every
formula cell (`"Sheet1!B2"`) with the bucket it was counted in, named as the corpus database names its buckets:
`matched`, `matched_alt:<grade>` (`boolAsNumber`, `errorDetailCache`, `whitespaceCache`, `solverTolerance`,
`cancellationResidue`, `iterationChange`, `localeDependent`),
`calc_mismatch`, `stale_cell`, `legacy_excel_version`, `circular_reference`, `untrusted_match:<cacheTrust>`, `untrusted_mismatch:<cacheTrust>`, `no_oracle:<why>`
or `volatile`, for an audit that follows single cells from one run to the next.

The human report prints each file's mismatches and differing categories, a summary table, the most mismatching
functions, mismatch signatures and totals. `--json` prints one object (schema `gridcraft-check/2`; new fields may be
added, a changed meaning bumps the version: in version 2, `staleCache` means that the cache is not trusted
(`cacheTrust` is not `trusted`) and such files are compared instead of skipped):

```jsonc
{
  "schema": "gridcraft-check/2",
  "tolerance": 1e-9,
  "files": [{
    "path": "book.xlsx",
    "opened": true,
    "error": null,                     // GridCraft's message when the file can't be opened
    "warnings": ["..."],               // import warnings
    "producer": {"application": "Microsoft Excel", "version": "16.0300", "kind": "excel"},  // see `Producer`
    "calc": {
      "formulaCells": 3, "checked": 2, "matched": 1, "mismatched": 1, "noOracle": 1, "volatile": 0, "volatileViaNames": 0,
      "localeDependent": 0,            // matched only under the cache's locale (counted in "matched")
      "locale": null,                  // that locale's tag (en-GB, de-DE, …), null for en-US; see Locale
      "noOracleReasons": {"noCachedValue": 1},  // by cause: noCachedValue | addin | udf | xlm | externalData | downstream
      "staleCache": false,             // cacheTrust != "trusted"
      "cacheTrust": "trusted",         // trusted | staleCache | fullCalcOnLoad | foreignProducer
      "calcPr": {"mode": "auto", "calcCompleted": null, "fullCalcOnLoad": false, "iterate": false, "calcId": 191029},
      "untrustedMatched": 0, "untrustedMismatched": 0,  // compared against an untrusted cache: not in the counts above
      "untrustedSignatures": {},
      "boolAsNumber": 0,               // matched TRUE/FALSE results the producer saved as 1/0 (no t="b")
      "errorDetailCache": 0,           // matched errors Google Sheets saved as text `#CODE` / `#CODE:reason`
      "whitespaceCache": 0,            // matched texts a non-Excel writer saved without their surrounding whitespace
      "solverTolerance": 0,            // matched numbers within a solver's 1e-7 carried through the formula
      "cancellationResidue": 0,        // matched numbers within the rounding the formula's operations propagate
      "iterationChange": 0,            // matched numbers on an iterative loop that Excel's stopping rule explains
      "staleCell": 0,                  // of "checked": manual-mode caches inconsistent with their own cached inputs, not gating
      "legacyExcelVersion": 0,         // of "checked": mismatches the calculating Excel's older version explains, not gating
      "circularReference": 0,          // of "checked": mismatches on or reading a circular reference, iteration off, not gating
      "mismatches": [{"cell": "Sheet1!B1", "formula": "=SUM(A1:A2)", "expected": 7, "actual": 6,
                      "kind": "numeric",  // numeric | text | bool | error | blank (GridCraft empty) | type
                      "functions": ["SUM"], "primary": true,
                      "class": "trusted"}],  // trusted | staleCell | legacyExcelVersion | circularReference | staleCache | fullCalcOnLoad | foreignProducer
      "functions": [{"name": "SUM", "checked": 1, "mismatched": 1, "primary": 1}],
      "signatures": {"number → number": 1}
    },
    "roundTrip": {
      "error": null,                   // write or read-back failure
      "differing": ["values"],
      "categories": {"values": {"equal": false, "diffs": 1, "examples": ["Sheet1!A1: 2 → 3"]}, "...": {}}
    }
  }],
  "totals": {"files": 1, "opened": 1, "failedToOpen": 0, "formulaCells": 3, "checked": 2, "matched": 1,
             "mismatched": 1, "noOracle": 1, "noOracleReasons": {"noCachedValue": 1}, "volatile": 0, "volatileViaNames": 0, "boolAsNumber": 0,
             "errorDetailCache": 0, "whitespaceCache": 0, "solverTolerance": 0, "cancellationResidue": 0, "iterationChange": 0,
             "staleCell": 0, "legacyExcelVersion": 0, "circularReference": 0,
             "untrustedMatched": 0, "untrustedMismatched": 0, "untrusted": {},  // {"staleCache": {"files", "matched", "mismatched"}}
             "localeDependent": 0, "producers": {"excel": 1},  // files by producer kind
             "roundTripFailed": 0, "roundTripDiffering": 1, "categoriesDiffering": {"values": 1}},
  "functions": [{"name": "SUM", "checked": 1, "mismatched": 1, "primary": 1}],  // all files, most mismatched first
  "signatures": {"number → number": 1}
}
```

Values use the same JSON shapes as `eval --json`: numbers, strings, booleans, `null` (empty) and
`{"error": "#N/A"}`. Mismatch lists are capped by `--max-report`; the counts are not.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-11 | patch | `check`, older Excel versions: a legacy error cache is only a call's documented older answer for the inputs it is documented for (PMT with no periods `#DIV/0!`; IRR `#DIV/0!` before Excel 2003 per KB 30567 and `#NUM!` where its 20 tries give up; NORMSINV/NORMINV `#N/A` in the tails before Excel 2003; MOD `#NUM!` from 2^27 times the divisor, Excel 2003 and older); no other error is put in a call's place and the other inverse distributions and XIRR are not substituted. The formula must give the cache back with the older answer, differ from GridCraft's own value there, and give GridCraft's value with the calls held (`MATCH(…)+MOD(5,0)` and `+NORMSINV(0.5)` cached `#N/A` were excused by a made-up error and gate again) |
| 2026-11-12 | patch | `check`, older Excel versions: every old-answer entry is backed by real older-Excel evidence. IRR's `#NUM!` give-up (Newton on the rate) is replaced by the flows an Excel 2007 cache shows giving up (Excel iterates on the discount factor, so the old model excused `{-5,1}`, `{-100,50}`, a workbook's Ret!K12/K24 flows, and contradicted Excel 2002's own 0.45188 for -85, 120, 100, -138 at 22%); the NORMSINV/NORMINV tail `#N/A` is removed (Excel XP and 97 tables are numbers: KB 826772, McCullough and Wilson 2005, Knüsel); a call is replaced by its older answer only when its arguments are verified (literals, or references to cells whose cache GridCraft matches), so `NORMSINV(MATCH(…)*1E-7)` cached `#N/A` gates with the bare MATCH; the human-report reason names the evidenced answers. PMT with no periods, IRR `#DIV/0!` before Excel 2003 and MOD 2^27 are unchanged. |
