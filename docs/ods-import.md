# Importing OpenDocument spreadsheets

Open an `.ods` file, or a flat OpenDocument spreadsheet (`.fods`, one XML document), with File →
Open, drag it onto GridCraft, or pass it to the CLI. GridCraft
imports worksheet data into a new workbook and shows a warning describing the conversion limits.

The import reads sheets, numbers, text, booleans, error results, supported dates/times,
repeated cells/rows, merged cells, **formulas** and named ranges. Each formula keeps its last
saved result until GridCraft recalculates. OpenFormula (`of:=…`, ODF 1.3 part 4) is translated to
Excel syntax: bracketed references (`[.A1]`, `[$Sheet1.$A$1:.$B$2]`, 3-D spans, whole columns and
rows), `;` argument separators, `{1;2|3;4}` inline arrays, the `~` union and `!` intersection,
vendor-prefixed functions (`COM.MICROSOFT.XLOOKUP` → `XLOOKUP`, `LEGACY.CHIDIST` → `CHIDIST`) and
matrix (array) formulas. Functions whose OpenFormula signature differs from Excel's function of the
same name become the Excel function with the same meaning: `FLOOR`/`CEILING` (with their Mode
argument) → `FLOOR.MATH`/`CEILING.MATH`, left-tailed `FDIST`/`FINV` → `F.DIST`/`F.INV`,
`CHISQDIST`/`CHISQINV` → `CHISQ.DIST`/`CHISQ.INV`, the cumulative forms of `BETADIST`,
`LOGNORMDIST` and `HYPGEOMDIST` → `BETA.DIST`, `LOGNORM.DIST` and `HYPGEOM.DIST`, and omitted
optional arguments Excel requires (`ROUND`'s digits, `POISSON`'s/`NORMDIST`'s cumulative flag)
are filled with OpenFormula's defaults. `MROUND`, which in OpenFormula takes the greater of two
equally near multiples whatever the signs (Excel's rounds a tie away from zero and is `#NUM!` for
opposite signs), becomes `LET(mround_b,ABS(B),FLOOR.MATH((A)+mround_b/2,mround_b))`, so
`MROUND(-1;-2)` stays 0; the `LET` name is one neither the formula nor any named expression of
the document spells (`mround_b_`, …), because a name is evaluated where it is used. `INDIRECT`
text written with OpenFormula's sheet separator (`INDIRECT("Sheet2.A1")`, ODF 1.3 part 4
§6.14.7) becomes Excel's (`INDIRECT("'Sheet2'!A1")`), and a sheet named in the text, either
way, is named as imported (a sheet name Excel forbids is repaired); an `INDIRECT` whose text the
formula computes (`ADDRESS`'s excepted, unless a sheet was repaired), or whose text has a dot but
isn't an A1 reference, keeps its saved result.
Formulas that can't be translated (references to other
files, functions of the producing application such as `ORG.LIBREOFFICE.*`, `MULTIPLE.OPERATIONS`,
names the file doesn't define, functions GridCraft doesn't have) keep their saved results, with a
warning naming the reason. A named expression that can't be imported is left out, and so is
every name that uses it, directly or through other names; formulas using any of them keep their
saved results. A sheet's name that is left out doesn't give way to a workbook name of the same
spelling (a name is read where it is used, and the sheet's hides the workbook's): the sheet's
formulas and names using it keep their saved results too, and so do those using a workbook name
that uses that spelling, directly or through other workbook names: a name is looked up from the
sheet using it, so there the workbook name would read the workbook's instead (the other sheets
still recalculate with it). Saved results may be out of date if
the source application did not recalculate before saving, and they are that application's
results, which need not be Excel's.

Names Excel can't hold are renamed, with one warning, as the `.xls` and XLSB readers rename the
names Excel renames when it opens a file: a name that reads as a cell in today's grid (`TAX2020`,
`MIN1`, `Q1`) or as an R1C1 reference (`R`, `C`, `RC`, `R1C1`), and a name with a space, hyphen,
slash or other character Excel forbids, or a leading digit, or over 255 characters. Each forbidden
character becomes `_` and a `_` goes in front of what still doesn't start or read as a name; a name
that would equal another (one the document defines that Excel accepts, or an earlier renamed one,
compared without regard to case) gets more underscores in front. The one rule serves the definition
and every formula that uses the name, so `=TAX2020*2` reads the name and not the cell. `INDIRECT`
with literal text finds a name the way LibreOffice's does, before any cell: with a sheet before the
first `.` or `!` (`"Data.Q1"`, `"'Data'.Q1"`, `"data!q1"`, in any case) the text is that sheet's
own name of the part after it, exactly as written (a `$`, as in `"$Data.Q1"`, or spaces around it
leave no name); without a sheet it is the formula's own sheet's name, then the workbook's. A name
the import renamed is named so in the text (`INDIRECT("TAX2020")` and `INDIRECT("Data.TAX2020")`
become `INDIRECT("_TAX2020")` and `INDIRECT("'Data'!_TAX2020")`), and a text that finds no name
stays the cell: a name only another sheet has leaves `INDIRECT("Q1")` and `INDIRECT("S2!Q1")` on
`S2` reading the cell `Q1`. An `INDIRECT` whose text is read in R1C1 style or computed (which
could spell a renamed name), one naming a workbook name through a sheet (LibreOffice finds no name
there, Excel does) or with spaces around a name, keeps its saved result, as does a formula written
in Excel's own syntax (`msoxl:`) that uses a renamed name.

The calculation settings (`table:calculation-settings`, ODF 1.3 part 3 §9.4.1) are read: a
null date (`table:null-date`, §9.4.2) of 1899-12-30 is Excel's 1900 date system and 1904-01-01
its 1904 system (dates read as that system's serial numbers); under any other null date every
formula keeps its saved result, because its date arithmetic has no Excel equivalent. Precision
as shown (`table:precision-as-shown`) becomes Excel's "Set precision as displayed": formula
results are rounded as their format shows them when they calculate.

Criteria and lookups match text the way the document's calculation settings say
(`table:calculation-settings`, ODF 1.3 part 3 §19.646 table 16): with regular expressions unless
the document turns them off, without wildcards unless it turns them on, against whole cells
unless it says otherwise. Excel matches with wildcards against whole cells. Under any other
setting, a formula calling COUNTIF(S), SUMIF(S), AVERAGEIF(S), MAXIFS, MINIFS, MATCH, VLOOKUP,
HLOOKUP, LOOKUP, SEARCH or a database function (DSUM…) keeps its saved result unless every
criterion is a number, an ordered comparison (`">2"`) or text without the characters either side
reads specially (`"abc"` under regular expressions, but not `"a.*"`, a cell reference or a
database criteria range, whose text can't be checked). Text comparisons and criteria are
case-sensitive unless the document sets `table:case-sensitive="false"` (§19.594); Excel ignores
case, so in a case-sensitive document a formula comparing operands that may be text with letters
(`=`, `<>`, `<`, …), or a criterion or lookup value with letters, keeps its saved result.

Cell formatting is imported from the document's styles (ODF 1.3 part 3, chapters 16 and 17):
number formats (data styles become Excel format codes: digits, grouping, thousands scaling,
conditional sections (OpenFormula `value()` conditions, with or without their namespace
prefix) with their colours, percentages, currencies, scientific notation,
fractions, embedded and literal text, dates, times and elapsed durations, text formats, whose
own section stays the text section after the ones their maps make for numbers), fonts
(face, size, bold, italic, underline, strike-through, colour, superscript/subscript), fills,
borders (including diagonals), horizontal and vertical alignment, wrapping, shrink to fit,
rotation and cell protection. Common styles inherit from their parents property by property
(the alignment's source and the paragraph alignment separately; a percentage font size scales
the size of the style's own parent, never what a row's or column's default style gives;
column, row and table styles too, then from their family's default style), and what a cell's
style and its parents leave unset comes from its row's default cell style, then its column's;
the document's `Default` cell style becomes the workbook's Normal style. Aligned
by value type, a logical value is right-aligned, as ODF aligns every value type but string,
where Excel's General would centre it, and an error that the file types as a string (as
LibreOffice does) is left-aligned. Column widths (the most common width
is the sheet's standard width), set row heights, hidden rows and columns, column and row default
cell styles, hidden sheets, tab colours and frozen panes (from `settings.xml`) are read; row
heights LibreOffice fitted to the content stay automatic, and set heights stay fixed, also when
the most common one becomes the sheet's standard height. Date-style parts Excel has no code for
(eras, quarters, week numbers, other calendars) are left out of the format. A document that
formats more than a million empty cells, or sets more than two million row or column entries,
keeps its values and loses that formatting past the limit, with a warning.

This is an import, not full ODS compatibility. Charts, images, comments, conditional formatting,
data validation and other workbook features are not preserved. GridCraft does not write ODS files.

Save opens Save As and suggests a new `.xlsx` file. The source ODS file is not used as the
workbook's save destination; requests to save to `.ods` return an error.

```sh
gridcraft-cli info source.ods
gridcraft-cli convert source.ods imported.xlsx
```

CLI import warnings go to stderr, leaving structured output on stdout available for scripts.
