//! Workbooks.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use gridcraft_core::{CellRef, DateSystem, RangeRef, Value};
use gridcraft_formula::Expr;
use serde::{Deserialize, Serialize};

use crate::cell::Formula;
use crate::sheet::Sheet;
use crate::style::{StyleTable, Theme};

struct TableRename<'a> {
    /// Present on rename, absent on sheet copy (which only retargets structured references).
    wb: Option<&'a Workbook>,
    old: &'a str,
    new: &'a str,
}

impl TableRename<'_> {
    fn replacement(&self, n: &str, sheet: usize, env: &[String]) -> Option<Expr> {
        let wb = self.wb?;
        if n.starts_with('[') {
            return None;
        }
        let (scope, bare, qualifier) = match n.rsplit_once('!') {
            Some((s, b)) => (wb.sheet_index(s)?, b, Some(s)),
            None => (sheet, n, None),
        };
        if !bare.eq_ignore_ascii_case(self.old) || wb.name(bare, scope).is_some() {
            return None;
        }
        let renamed = qualifier.map_or_else(|| self.new.into(), |s| format!("{s}!{}", self.new));
        Some(if env.iter().any(|local| gridcraft_formula::same_name(local, &renamed)) || wb.name(self.new, scope).is_some() {
            Expr::Struct(gridcraft_formula::StructRef { table: self.new.into(), specifiers: vec![], col_start: None, col_end: None })
        } else {
            Expr::Name(renamed)
        })
    }

    /// The replacement for a table token (`structured`) or a free name read on `sheet`.
    fn token(&self, n: &str, structured: bool, sheet: usize, env: &[String]) -> Option<Expr> {
        if structured {
            return n.eq_ignore_ascii_case(self.old).then(|| Expr::Name(self.new.into()));
        }
        self.replacement(n, sheet, env)
    }

    /// Formula text read on `sheet` with the table renamed, when that changes it.
    fn text(&self, text: &str, sheet: usize) -> Option<String> {
        self.expression_text(gridcraft_formula::parse_name(text).ok()?, text, sheet)
    }

    fn expression_text(&self, mut e: Expr, text: &str, sheet: usize) -> Option<String> {
        if !self.might_use_table(&e) || !rewrite_table_tokens(&mut e, &|n, structured, env| self.token(n, structured, sheet, env)) {
            return None;
        }
        let prefix = if text.trim_start().starts_with('=') { "=" } else { "" };
        Some(format!("{prefix}{}", gridcraft_formula::print(&e)))
    }

    /// Sheet `i`'s cell formulas that change with the table renamed.
    fn cells(&self, sheet: &Sheet, i: usize) -> Vec<(CellRef, Formula)> {
        sheet
            .cells
            .iter()
            .filter_map(|(c, cell)| {
                let f = cell.formula.as_ref()?;
                f.with_expr(|e| {
                    if !self.might_use_table(e) {
                        return None;
                    }
                    let mut ne = e.clone();
                    rewrite_table_tokens(&mut ne, &|n, structured, env| self.token(n, structured, i, env))
                        .then(|| (c, Formula { array: f.array, ..f.rewritten(ne) }))
                })?
            })
            .collect()
    }

    fn features(&self, sheet: &mut Sheet, si: usize) {
        let text = |f: &mut String| {
            if let Some(new) = self.text(f, si) {
                *f = new;
            }
        };
        let threshold = |v: &mut crate::CfValueKind| {
            if let crate::CfValueKind::Formula(f) = v {
                text(f);
            }
        };
        for cf in &mut sheet.cond_formats {
            match &mut cf.rule {
                crate::CfRule::CellIs { a, b, .. } => {
                    text(a);
                    b.iter_mut().for_each(text);
                }
                crate::CfRule::Expression { formula, .. } => text(formula),
                crate::CfRule::ColorScale { stops } => stops.iter_mut().for_each(|(v, _)| threshold(v)),
                crate::CfRule::DataBar { min, max, .. } => {
                    threshold(min);
                    threshold(max);
                }
                crate::CfRule::IconSet { thresholds, .. } => thresholds.iter_mut().for_each(threshold),
                _ => {}
            }
        }
        for dv in &mut sheet.validations {
            text(&mut dv.f1);
            dv.f2.iter_mut().for_each(text);
        }
        for chart in &mut sheet.charts {
            for series in &mut chart.series {
                text(&mut series.values);
                series.categories.iter_mut().for_each(text);
                series.bubble_sizes.iter_mut().for_each(text);
                // A bare series title can be literal text, even if it is the table name.
                if let Some(name) = &mut series.name
                    && let Ok(e) = gridcraft_formula::parse(name)
                    && (name.trim_start().starts_with('=') || matches!(&e, Expr::Struct(r) if r.table.eq_ignore_ascii_case(self.old)))
                    && let Some(new) = self.expression_text(e, name, si)
                {
                    *name = new;
                }
            }
        }
    }

    fn might_use_table(&self, e: &Expr) -> bool {
        let mut found = false;
        e.walk(&mut |e| {
            found |= match e {
                Expr::Name(n) => n.rsplit('!').next().is_some_and(|n| n.eq_ignore_ascii_case(self.old)),
                Expr::Struct(r) => r.table.eq_ignore_ascii_case(self.old),
                _ => false,
            };
        });
        found
    }
}

/// Rewrites table tokens and free names, leaving LET/LAMBDA binders, bound names and text alone.
/// The callback receives a name, whether it is the table token of a structured reference, and
/// the lexical binders currently in scope. For a table token it returns the new name, or any
/// other expression to take the whole structured reference's place.
pub fn rewrite_table_tokens(e: &mut Expr, replacement: &impl Fn(&str, bool, &[String]) -> Option<Expr>) -> bool {
    fn args(xs: &mut [Expr], env: &mut Vec<String>, replacement: &impl Fn(&str, bool, &[String]) -> Option<Expr>) -> bool {
        xs.iter_mut().fold(false, |changed, e| visit(e, env, replacement) | changed)
    }
    fn visit(mut e: &mut Expr, env: &mut Vec<String>, replacement: &impl Fn(&str, bool, &[String]) -> Option<Expr>) -> bool {
        let mut changed = false;
        while let Expr::Binary(_, a, b) = e {
            changed |= visit(b, env, replacement);
            e = a;
        }
        changed
            | match e {
                Expr::Struct(st) if !st.table.is_empty() => match replacement(&st.table, true, env) {
                    Some(Expr::Name(n)) => {
                        st.table = n;
                        true
                    }
                    Some(other) => {
                        *e = other;
                        true
                    }
                    None => false,
                },
                Expr::Name(n) if !env.iter().any(|local| gridcraft_formula::same_name(local, n)) => match replacement(n, false, env) {
                    Some(new) => {
                        *e = new;
                        true
                    }
                    None => false,
                },
                Expr::Call(name, args) if name == "LET" && args.len() >= 3 && args.len() % 2 != 0 => {
                    let Some((body, pairs)) = args.split_last_mut() else { return changed };
                    let before = env.len();
                    let mut rewritten = false;
                    for [binder, value] in pairs.as_chunks_mut::<2>().0 {
                        let Expr::Name(n) = binder else {
                            env.truncate(before);
                            return changed | rewritten;
                        };
                        rewritten |= visit(value, env, replacement);
                        env.push(n.clone());
                    }
                    rewritten |= visit(body, env, replacement);
                    env.truncate(before);
                    rewritten
                }
                Expr::Call(name, args) if name == "LAMBDA" => {
                    let Some((body, params)) = args.split_last_mut() else { return changed };
                    let before = env.len();
                    for param in params {
                        let Expr::Name(n) = param else {
                            env.truncate(before);
                            return changed;
                        };
                        env.push(n.trim_start_matches('[').trim_end_matches(']').into());
                    }
                    let rewritten = visit(body, env, replacement);
                    env.truncate(before);
                    rewritten
                }
                Expr::Call(name, _) if name == "LET" => false,
                Expr::Call(_, xs) => args(xs, env, replacement),
                Expr::Invoke(c, xs) => visit(c, env, replacement) | args(xs, env, replacement),
                Expr::Array(rows) => rows.iter_mut().fold(false, |changed, row| args(row, env, replacement) | changed),
                Expr::Unary(_, inner) | Expr::Paren(inner) => visit(inner, env, replacement),
                _ => false,
            }
    }
    visit(e, &mut Vec::new(), replacement)
}

/// Most equation source, in bytes, one [`Workbook::refresh_equations`] lays out: about 2,500
/// typical equations, or a few seconds of the slowest 4 KB sources.
pub const MAX_EQUATION_LAYOUT_BYTES: usize = 128 * 1024;

/// Why an equation past [`MAX_EQUATION_LAYOUT_BYTES`] has no layout.
pub const EQUATION_BUDGET_MESSAGE: &str = "This workbook has more equations than are laid out at once; edit this equation to lay it out.";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CalcMode {
    #[default]
    Automatic,
    AutomaticExceptTables,
    Manual,
}

/// Options › Advanced › "For objects, show" (`<workbookPr showObjects>`, ECMA-376 `ST_Objects`):
/// whether charts, pictures, shapes and slicers are drawn. Ctrl+6 switches between `All` and
/// `None`; `Placeholders` only comes from files and is kept as it is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShowObjects {
    #[default]
    All,
    Placeholders,
    None,
}

/// The workbook window's display settings from `<workbookView>` (Excel: File › Options ›
/// Advanced, "Display options for this workbook"): whether the window shows the sheet tabs
/// and the two scroll bars, and how much of the horizontal bar the tabs take (`tabRatio`,
/// thousandths of the bar; the format's default is 600). XLSX I/O normalizes this
/// share to 0–1000, including values supplied directly through the model.
///
/// The values are modelled so an open + save keeps them — a workbook Excel saved with the
/// tabs hidden must not come back with them shown and the settings gone. The app does not
/// act on them yet and there is no Options control for them; that is the other half of
/// storytold/gridcraft#3057.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkbookView {
    #[serde(default = "yes")]
    pub show_sheet_tabs: bool,
    #[serde(default = "yes")]
    pub show_horizontal_scroll: bool,
    #[serde(default = "yes")]
    pub show_vertical_scroll: bool,
    #[serde(default = "default_tab_ratio")]
    pub tab_ratio: u32,
}

fn yes() -> bool {
    true
}

fn default_tab_ratio() -> u32 {
    600
}

impl Default for WorkbookView {
    fn default() -> Self {
        WorkbookView { show_sheet_tabs: true, show_horizontal_scroll: true, show_vertical_scroll: true, tab_ratio: default_tab_ratio() }
    }
}

impl WorkbookView {
    /// The entire bar, in thousandths: the upper bound used by XLSX I/O.
    pub const MAX_TAB_RATIO: u32 = 1000;

    fn is_default(&self) -> bool {
        *self == WorkbookView::default()
    }
}

/// File › Options › Formulas › Calculation options, stored per workbook (`<calcPr>`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CalcSettings {
    pub mode: CalcMode,
    /// Enable iterative calculation: circular references are calculated repeatedly instead of
    /// reporting an error.
    pub iterative: bool,
    /// Maximum Iterations, 1 to [`CalcSettings::MAX_ITERATIONS`].
    pub max_iterations: u32,
    /// Maximum Change: iteration stops once no value in a circular reference changes by more.
    pub max_change: f64,
    /// Use precision as displayed.
    pub precision_as_displayed: bool,
    /// R1C1 reference style (`<calcPr refMode="R1C1"/>`): kept with the file and written back;
    /// GridCraft itself shows and takes A1 references.
    #[serde(default)]
    pub r1c1: bool,
}

impl Default for CalcSettings {
    fn default() -> Self {
        CalcSettings {
            mode: CalcMode::Automatic,
            iterative: false,
            max_iterations: Self::DEFAULT_ITERATIONS,
            max_change: Self::DEFAULT_CHANGE,
            precision_as_displayed: false,
            r1c1: false,
        }
    }
}

impl CalcSettings {
    pub const DEFAULT_ITERATIONS: u32 = 100;
    pub const DEFAULT_CHANGE: f64 = 0.001;
    /// The largest Maximum Iterations Excel accepts.
    pub const MAX_ITERATIONS: u32 = 32_767;

    /// Sets Maximum Iterations, kept within 1 to [`CalcSettings::MAX_ITERATIONS`].
    pub fn set_max_iterations(&mut self, n: u32) {
        self.max_iterations = n.clamp(1, Self::MAX_ITERATIONS);
    }

    /// Sets Maximum Change (its magnitude); a value that isn't finite is ignored.
    pub fn set_max_change(&mut self, d: f64) {
        if d.is_finite() {
            self.max_change = d.abs();
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DefinedName {
    pub name: String,
    /// `None` = workbook scope; otherwise the sheet index.
    pub scope: Option<usize>,
    /// Formula text without `=` (`Sheet1!$A$1:$A$10`, `0.07`, `LAMBDA(x,x*2)`).
    pub formula: String,
    pub comment: String,
    pub hidden: bool,
    /// The name's identity while the app runs ([`DefinitionUid`]). Not in JSON or the file.
    #[serde(skip)]
    pub uid: DefinitionUid,
}

/// The identity of a defined name or table while the app runs, so that what was bound to it (a
/// PivotTable's source) can tell it from another one with the same text. Editing the definition
/// keeps it; a definition deleted and made again gets a new one (they are never reused), and
/// undo brings the old one back with the definition. Never saved. It is not content: any two
/// compare equal, so comparing names compares what they hold; compare identities with
/// [`DefinitionUid::get`].
#[derive(Clone, Copy, Debug)]
pub struct DefinitionUid(u64);

/// The next [`DefinitionUid`]; 0 is never given out.
static NEXT_DEFINITION_UID: AtomicU64 = AtomicU64::new(1);

impl DefinitionUid {
    pub fn fresh() -> DefinitionUid {
        DefinitionUid(NEXT_DEFINITION_UID.fetch_add(1, Ordering::Relaxed))
    }
    pub fn get(self) -> u64 {
        self.0
    }
}

impl Default for DefinitionUid {
    fn default() -> DefinitionUid {
        DefinitionUid::fresh()
    }
}

impl PartialEq for DefinitionUid {
    fn eq(&self, _: &DefinitionUid) -> bool {
        true
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DocProps {
    pub title: String,
    pub subject: String,
    pub author: String,
    pub company: String,
    pub keywords: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_modified_by: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub created: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub modified: String,
}

/// The values a linked workbook had when the file was last saved with the link updated
/// (`<externalLink>` `sheetDataSet`). `[n]` in a formula is the workbook's `external_books[n - 1]`.
/// GridCraft doesn't open linked files: references into them read these values, as Excel does
/// while the source is closed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExternalBook {
    pub sheets: Vec<ExternalSheet>,
    /// The linked workbook's defined names (`[1]!Rates`, `[1]Sheet1!Rates`).
    pub names: Vec<ExternalName>,
    /// The link isn't to a workbook (a DDE or OLE link), or its part is missing or unreadable.
    /// What formulas read through it can't be calculated, apart from `items`: the cells keep the
    /// values saved for them.
    pub opaque: bool,
    /// A DDE link's items with the values saved for them (`[1]!Price` reads item `Price`). An
    /// item saved without values isn't here.
    pub items: Vec<(String, Value)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExternalSheet {
    pub name: String,
    /// Cached cells, sorted by position (row, then column).
    pub cells: Vec<(CellRef, Value)>,
    /// The last update of the link couldn't read this sheet (`refreshError`): Excel keeps the
    /// cells it had saved, and references to anything else in it are `#REF!`.
    pub refresh_error: bool,
    /// The link saved no cells for this sheet at all (no `sheetData` for it, as when the file
    /// has no `sheetDataSet`): what Excel showed for references into it isn't known.
    pub missing: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExternalName {
    pub name: String,
    /// Index into the book's `sheets` of the sheet the name is scoped to; `None` = workbook scope.
    pub sheet: Option<usize>,
    /// What the name refers to (without `=`); its sheets are the linked workbook's. Empty when
    /// the file saved the name without a definition.
    pub refers_to: String,
}

impl ExternalBook {
    pub fn sheet(&self, name: &str) -> Option<&ExternalSheet> {
        self.sheets.get(self.sheet_index(name)?)
    }
    /// The position in `sheets` of the sheet named `name`.
    pub fn sheet_index(&self, name: &str) -> Option<usize> {
        self.sheets.iter().position(|s| s.name.eq_ignore_ascii_case(name))
    }
    /// The name `name` as seen from sheet `sheet` (`None` = from the workbook): the sheet's own
    /// name first, then the workbook-scoped one.
    pub fn name(&self, name: &str, sheet: Option<&str>) -> Option<&ExternalName> {
        let si = sheet.and_then(|s| self.sheet_index(s));
        let find = |scope: Option<usize>| self.names.iter().find(|n| n.sheet == scope && n.name.eq_ignore_ascii_case(name));
        si.and_then(|i| find(Some(i))).or_else(|| find(None))
    }
    /// The value saved for DDE item `name` as a formula writes it: `[1]!Price`, or quoted when it
    /// isn't a plain word (`[1]!'0#NG: 2 16'`, `''` for a quote).
    pub fn item(&self, name: &str) -> Option<&Value> {
        let name = name.strip_prefix('\'').and_then(|n| n.strip_suffix('\'')).unwrap_or(name);
        let decoded = name.contains("''").then(|| name.replace("''", "'"));
        let name = decoded.as_deref().unwrap_or(name);
        self.items.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v)
    }
}

impl ExternalSheet {
    /// The cached value at `c` (blank when the link didn't store the cell).
    pub fn value(&self, c: CellRef) -> Value {
        self.cached_value(c).cloned().unwrap_or_default()
    }

    /// Looks up a cached cell record, distinguishing a saved blank from an absent record.
    pub fn cached_value(&self, c: CellRef) -> Option<&Value> {
        self.cells.binary_search_by_key(&c, |(p, _)| *p).ok().and_then(|i| self.cells.get(i)).map(|(_, v)| v)
    }
    /// The last refresh failed and this sheet contains no cached cells.
    pub fn unreadable(&self) -> bool {
        self.refresh_error && self.cells.is_empty()
    }
    /// The cached cells inside `range`, in order.
    pub fn cells_in(&self, range: RangeRef) -> impl Iterator<Item = &(CellRef, Value)> {
        let from = self.cells.partition_point(|(p, _)| p.row < range.start.row);
        self.cells.get(from..).unwrap_or_default().iter().take_while(move |(p, _)| p.row <= range.end.row).filter(move |(p, _)| range.contains(*p))
    }
    /// The smallest range holding every cached cell.
    pub fn extent(&self) -> Option<RangeRef> {
        let (first, last) = (self.cells.first()?.0, self.cells.last()?.0);
        let (c0, c1) = self.cells.iter().fold((u32::MAX, 0), |(a, b), (p, _)| (a.min(p.col), b.max(p.col)));
        Some(RangeRef::new(CellRef::new(first.row, c0), CellRef::new(last.row, c1)))
    }
}
/// One phonetic (furigana) run of a string item (`<rPh>`): the reading `text` and the range of the
/// base text it annotates.
///
/// `start` and `end` are zero-based offsets into the base text (`sb` and `eb` in the file) with
/// `start < end`; reading keeps them within the text's length in UTF-16 code units. Behaviour
/// reference: ISO/IEC 29500-1 §18.4.6 `rPh` and the Microsoft Learn `PhoneticRun` class.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhoneticRun {
    pub start: u32,
    pub end: u32,
    pub text: String,
}

/// The `<phoneticPr>` of a string item: how its phonetic runs are displayed. `kind` (`type`) and
/// `alignment` only ever hold values ISO/IEC 29500-1 §18.18.57 `ST_PhoneticType` / §18.18.56
/// `ST_PhoneticAlignment` define, so damaged files cannot be written back invalid.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhoneticProps {
    /// `fontId`: a required attribute, an index into the style sheet's fonts. A file without one
    /// gets 0 — the Normal style's font, which an out-of-range index falls back to anyway.
    #[serde(default)]
    pub font_id: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alignment: Option<String>,
}

/// The phonetic (furigana) data of one shared string: its `<rPh>` runs, in document order, and its
/// `<phoneticPr>` when the file had one.
///
/// `text` is the string's base text — the only handle a cell keeps on its shared string — and is
/// the key the writer uses to re-emit the runs next to the right `<si>`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhoneticString {
    pub text: String,
    pub runs: Vec<PhoneticRun>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub props: Option<PhoneticProps>,
}

fn is_all(s: &ShowObjects) -> bool {
    *s == ShowObjects::All
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Workbook {
    pub sheets: Vec<Arc<Sheet>>,
    pub names: Vec<DefinedName>,
    pub styles: StyleTable,
    pub theme: Theme,
    pub date_system: DateSystem,
    pub calc: CalcSettings,
    #[serde(default, skip_serializing_if = "is_all")]
    pub show_objects: ShowObjects,
    /// Trust Center › Privacy Options › "Remove personal information from file properties on save"
    /// (`<workbookPr filterPrivacy="1"/>`): kept with the file, and while it is on the author and
    /// company are left out of the XLSX files GridCraft writes, as Excel leaves them empty. Other
    /// formats aren't filtered: the workbook's own serialization still holds them.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub remove_personal_info: bool,
    /// Read and set with [`Workbook::active_sheet`] and [`Workbook::set_active_sheet`].
    pub active_sheet: ActiveSheet,
    /// The workbook window's display settings (`<workbookView>`), kept across an open + save.
    #[serde(default, skip_serializing_if = "WorkbookView::is_default")]
    pub view: WorkbookView,
    pub props: DocProps,
    /// Workbook structure protection.
    pub protected_structure: bool,
    /// Password of the structure protection (XLSX `workbookProtection`), never the password.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structure_password: Option<crate::PasswordHash>,
    /// Custom fill lists (Options › Custom Lists) beyond the built-in ones.
    pub custom_lists: Vec<Vec<String>>,
    /// Named cell styles (Cell Styles gallery): name → style.
    pub cell_styles: Vec<(String, crate::style::Style)>,
    /// XLSX font-table entry zero when it differs from the default cell format's font.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xlsx_width_font: Option<crate::Font>,
    /// Phonetic (furigana) data of shared strings (`xl/sharedStrings.xml`), one entry per distinct
    /// reading of a text, so that an open + save keeps a Japanese workbook's readings. A text cell
    /// takes the first entry with its text, unless its sheet names another one (or none) for it
    /// in [`crate::Sheet::phonetic_cells`]: the same kanji can be read differently in two cells,
    /// and Excel then holds two `<si>` items with the same base text.
    ///
    /// Empty for workbooks without phonetic data; not present in older JSON. Modelled data
    /// (unlike [`Workbook::preserved`], which holds unmodelled parts), so it is saved to JSON as
    /// well as to XLSX.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub phonetics: Vec<PhoneticString>,
    /// VBA project (`xl/vbaProject.bin`) carried through unchanged. GridCraft does not run
    /// macros; the blob is preserved so that saving a macro-enabled workbook never destroys the
    /// macros it arrived with. base64 in JSON. `None` = the workbook has no macros. Shared, so a
    /// copy of the workbook (every edit's undo step) doesn't copy it.
    #[serde(default, with = "crate::b64::opt")]
    pub vba_project: Option<Arc<Vec<u8>>>,
    /// Field separator of the CSV this workbook was read from, so saving it as CSV writes the same
    /// separator back (a `;` file stays a `;` file). `None` = not read from CSV: CSV saves use `,`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub csv_delimiter: Option<u8>,
    /// GridCraft macros (typed command lists, not VBA), in the order the macro list shows them.
    /// The JSON format keeps them as this field; xlsx keeps them in hidden defined names.
    #[serde(default, skip_serializing_if = "crate::macros::Macros::is_empty", skip_deserializing)]
    pub macros: crate::macros::Macros,
    /// The `macros` entries of a JSON file as read, before the engine checks each one on its own
    /// and moves it into `macros`, so one damaged or newer macro doesn't stop the file opening.
    /// Never written.
    #[serde(rename = "macros", default, skip_serializing, deserialize_with = "crate::macros::entries")]
    pub macros_read: Vec<serde_json::Value>,
    /// Macros in a newer format than this build reads, as the file held them. They are never read,
    /// listed or run; a save writes them back unchanged, so the newer build gets them back whole.
    /// Shared between snapshots, as nothing changes them.
    #[serde(skip)]
    pub macros_unread: Arc<Vec<serde_json::Value>>,
    /// Unmodelled file content kept for saving (package root and workbook level). Not in JSON.
    #[serde(skip)]
    pub preserved: Option<Arc<crate::preserved::Preserved>>,
    /// The folder the workbook was read from, which relative paths to linked workbooks in
    /// `preserved` are relative to. Saving into another folder rewrites them from there. Not in JSON.
    #[serde(skip)]
    pub source_dir: Option<std::path::PathBuf>,
    /// Linked workbooks' cached values, in `<externalReferences>` order (`[1]` is the first).
    /// Read from the file; the link parts themselves are kept for saving in `preserved`. Shared,
    /// as sheets are: edits and undo history clone the workbook, never this cache. Not in JSON,
    /// like the link parts: a workbook without them has no links, so a typed `[1]` is refused
    /// rather than saved into a file that has no link for it.
    #[serde(skip)]
    pub external_books: Arc<Vec<ExternalBook>>,
    /// The region whose formats this workbook is typed and shown in (Options › Region). A runtime
    /// setting of the application, not of the file: it is not saved, and the engine session sets
    /// it on every document it holds. en-US until then.
    #[serde(skip, default = "gridcraft_core::region::default_region")]
    pub region: &'static gridcraft_core::Region,
    /// The file this workbook reports as its own, which is what `CELL("filename")` returns.
    /// `None` = no file yet, which Excel reports as an empty string until the workbook has been
    /// saved once. Deliberately separate from `source_dir`: that one names the folder relative
    /// links are relative to and must keep naming it after a Save As into another folder, since
    /// the links in memory are still written for the old one. Not in JSON.
    #[serde(skip)]
    pub source_path: Option<std::path::PathBuf>,
}

impl Default for Workbook {
    fn default() -> Self {
        Workbook::new()
    }
}

/// The active sheet index of a [`Workbook`].
///
/// Every other part of a workbook value is immutable once shared (the engine keeps versions as
/// `Arc<Workbook>` and tells unsaved changes by pointer). The active sheet is the exception: it
/// is the view state of the window showing the workbook, and several windows on one workbook
/// swap their own sheet into it whenever one of them becomes the one commands act on. Keeping it
/// in an atomic lets that swap happen through the shared `Arc` without cloning the workbook.
/// Copies (undo versions, a cloned workbook) take the value at the time of the copy.
#[derive(Default, Serialize, Deserialize)]
#[serde(from = "usize", into = "usize")]
pub struct ActiveSheet(AtomicUsize);

impl ActiveSheet {
    pub fn get(&self) -> usize {
        self.0.load(Ordering::Relaxed)
    }
    pub fn set(&self, i: usize) {
        self.0.store(i, Ordering::Relaxed);
    }
}

impl From<usize> for ActiveSheet {
    fn from(i: usize) -> ActiveSheet {
        ActiveSheet(AtomicUsize::new(i))
    }
}

impl From<ActiveSheet> for usize {
    fn from(a: ActiveSheet) -> usize {
        a.get()
    }
}

impl Clone for ActiveSheet {
    fn clone(&self) -> ActiveSheet {
        ActiveSheet::from(self.get())
    }
}

impl PartialEq for ActiveSheet {
    fn eq(&self, other: &ActiveSheet) -> bool {
        self.get() == other.get()
    }
}

impl std::fmt::Debug for ActiveSheet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.get().fmt(f)
    }
}

impl Workbook {
    /// A new workbook with one sheet, "Sheet1".
    pub fn new() -> Workbook {
        Workbook {
            sheets: vec![Arc::new(Sheet::new("Sheet1"))],
            names: vec![],
            styles: StyleTable::default(),
            theme: Theme::default(),
            date_system: DateSystem::D1900,
            calc: CalcSettings::default(),
            show_objects: ShowObjects::All,
            remove_personal_info: false,
            active_sheet: ActiveSheet::default(),
            view: WorkbookView::default(),
            props: DocProps::default(),
            protected_structure: false,
            structure_password: None,
            custom_lists: vec![],
            cell_styles: vec![],
            xlsx_width_font: None,
            phonetics: vec![],
            vba_project: None,
            csv_delimiter: None,
            macros: crate::macros::Macros::default(),
            macros_read: vec![],
            macros_unread: Arc::default(),
            preserved: None,
            source_dir: None,
            external_books: Arc::default(),
            region: gridcraft_core::region::default_region(),
            source_path: None,
        }
    }
    pub fn sheet(&self, i: usize) -> Option<&Sheet> {
        self.sheets.get(i).map(|s| s.as_ref())
    }
    pub fn sheet_mut(&mut self, i: usize) -> Option<&mut Sheet> {
        self.sheets.get_mut(i).map(Arc::make_mut)
    }
    pub fn sheet_index(&self, name: &str) -> Option<usize> {
        self.sheets.iter().position(|s| s.name.eq_ignore_ascii_case(name))
    }
    /// The index of the sheet the workbook's window shows (the file's active tab).
    pub fn active_sheet(&self) -> usize {
        self.active_sheet.get()
    }
    /// Changes the active sheet in place, also through a shared `Arc<Workbook>`: it is view state,
    /// so switching sheets or windows neither copies the workbook nor makes it a new version.
    pub fn set_active_sheet(&self, i: usize) {
        self.active_sheet.set(i);
    }
    pub fn active(&self) -> Option<&Sheet> {
        self.sheet(self.active_sheet())
    }
    /// A fresh sheet name `SheetN` not in use.
    pub fn next_sheet_name(&self) -> String {
        (1..).map(|n| format!("Sheet{n}")).find(|n| self.sheet_index(n).is_none()).unwrap_or_else(|| "Sheet".into())
    }
    /// Validates a sheet name like Excel: 1–31 chars, none of `: \ / ? * [ ]`, not starting or
    /// ending with `'`, unique (case-insensitive) unless it is `except`'s own name.
    pub fn check_sheet_name(&self, name: &str, except: Option<usize>) -> Result<(), String> {
        let n = name.chars().count();
        if n == 0 {
            return Err("A sheet name can't be blank.".into());
        }
        if n > 31 {
            return Err("A sheet name can't be longer than 31 characters.".into());
        }
        if name.chars().any(|c| matches!(c, ':' | '\\' | '/' | '?' | '*' | '[' | ']')) {
            return Err("A sheet name can't contain any of these characters: : \\ / ? * [ ]".into());
        }
        if name.starts_with('\'') || name.ends_with('\'') {
            return Err("A sheet name can't begin or end with an apostrophe.".into());
        }
        if name.eq_ignore_ascii_case("History") {
            return Err("\"History\" is a reserved name.".into());
        }
        if let Some(i) = self.sheet_index(name)
            && Some(i) != except
        {
            return Err("That name is already taken. Try a different one.".into());
        }
        Ok(())
    }
    /// Finds a defined name visible from `sheet`: sheet scope first, then workbook scope.
    pub fn name(&self, name: &str, sheet: usize) -> Option<&DefinedName> {
        self.names
            .iter()
            .find(|n| n.scope == Some(sheet) && n.name.eq_ignore_ascii_case(name))
            .or_else(|| self.names.iter().find(|n| n.scope.is_none() && n.name.eq_ignore_ascii_case(name)))
    }
    /// Finds the defined name of exactly scope `scope` (`None`: workbook scope), as `Data!rData`
    /// names the one of sheet `Data` only.
    pub fn name_in_scope(&self, name: &str, scope: Option<usize>) -> Option<&DefinedName> {
        self.names.iter().find(|n| n.scope == scope && n.name.eq_ignore_ascii_case(name))
    }
    /// Spells calls of LAMBDA names the way the names are defined (`Double(4)`, where the
    /// parser gives `DOUBLE(4)`: it upper-cases function names, as built-in ones are shown).
    pub fn name_call_case(&self, e: Expr) -> Expr {
        spell_calls(e, &self.lambda_names())
    }
    /// [`Workbook::name_call_case`] for every cell formula and defined name (after reading a
    /// file).
    pub fn apply_name_call_case(&mut self) {
        let lambdas = self.lambda_names();
        if lambdas.is_empty() {
            return;
        }
        for sheet in self.sheets.iter_mut() {
            let fixed: Vec<_> = sheet
                .cells
                .iter()
                .filter_map(|(c, cell)| {
                    let f = cell.formula.as_ref()?;
                    let e = f.expr()?;
                    let ne = spell_calls(e.clone(), &lambdas);
                    (ne != e).then(|| (c, Formula { array: f.array, ..f.rewritten(ne) }))
                })
                .collect();
            if fixed.is_empty() {
                continue;
            }
            let sheet = Arc::make_mut(sheet);
            for (c, f) in fixed {
                if let Some(cell) = sheet.cells.get_mut(c) {
                    cell.formula = Some(Arc::new(f));
                }
            }
        }
        for n in self.names.iter_mut() {
            if let Ok(e) = gridcraft_formula::parse_name(&n.formula) {
                let ne = spell_calls(e.clone(), &lambdas);
                if ne != e {
                    n.formula = gridcraft_formula::print(&ne);
                }
            }
        }
    }
    /// Defined LAMBDA names eligible for call respelling. Built-ins keep the parser's spelling.
    pub(crate) fn lambda_names(&self) -> Vec<String> {
        let is_lambda = |f: &str| f.trim_start_matches('=').trim_start().get(..7).is_some_and(|p| p.eq_ignore_ascii_case("LAMBDA("));
        self.names.iter().filter(|n| is_lambda(&n.formula) && !gridcraft_functions::is_known_function(&n.name)).map(|n| n.name.clone()).collect()
    }
    /// Table by name across sheets: (sheet index, table index).
    pub fn table(&self, name: &str) -> Option<(usize, usize)> {
        for (si, s) in self.sheets.iter().enumerate() {
            if let Some(ti) = s.tables.iter().position(|t| t.name.eq_ignore_ascii_case(name)) {
                return Some((si, ti));
            }
        }
        None
    }
    /// Whether a table can't take `name` because the one namespace tables share with defined names
    /// (ECMA-376 Part 1 §18.5.1.2) already holds it: another table than `except` (the table being
    /// renamed), a defined name of any scope, including the names of kept chart, dialog and macro
    /// sheets, an imported slicer cache's name (written as a workbook name, `#N/A`, when the file
    /// had no marker for it), or a name the file format reserves ([`reserved_name`]). Names are
    /// the same regardless of case in any script ([`gridcraft_formula::same_name`]), as the XLSX
    /// save's repair of colliding table names compares them. Allocates nothing: it runs on what
    /// the user types.
    pub fn table_name_taken(&self, name: &str, except: Option<(usize, usize)>) -> bool {
        let same = |other: &str| gridcraft_formula::same_name(other, name);
        let tables = self.sheets.iter().enumerate().flat_map(|(si, s)| s.tables.iter().enumerate().map(move |(ti, t)| ((si, ti), t)));
        let mut kept_names = self.preserved.iter().flat_map(|p| &p.sheets).flat_map(|s| &s.names);
        reserved_name(name)
            || tables.filter(|(at, _)| Some(*at) != except).any(|(_, t)| same(&t.name))
            || self.names.iter().any(|n| same(&n.name))
            || kept_names.any(|n| same(&n.name))
            || self.sheets.iter().flat_map(|s| &s.slicers).any(|sl| !sl.imported_cache.name.is_empty() && same(&sl.imported_cache.name))
    }
    /// The name Insert Table gives a new table: the first of `Table1`, `Table2`… that
    /// [`Self::table_name_taken`] leaves free.
    pub fn next_table_name(&self) -> String {
        next_table_name_with(&mut 1, |name| self.table_name_taken(name, None))
    }
    /// Renames a table using the original workbook to resolve references. Callers with an
    /// existing original snapshot can apply [`crate::table_refs::TableRename`] directly.
    pub fn rename_table(&mut self, si: usize, ti: usize, name: &str) {
        let original = self.clone();
        if let Some(rename) = crate::table_refs::TableRename::new(&original, (si, ti), name) {
            rename.apply(self);
        }
    }

    /// Points sheet `si`'s own references to table `old` at table `new`, as a sheet copy does for
    /// each table it copies (`si` the copy, `old` the original table and `new` its copy). That
    /// retargets cells, formatting and validation rules, chart series, and the supplied copy-local
    /// names. Table slicers also follow the copy. Bare table names are left alone.
    pub fn retarget_table_on_sheet(&mut self, si: usize, old: &str, new: &str, copied_names: &mut [DefinedName]) {
        if si >= self.sheets.len() {
            return;
        }
        let rename = TableRename { wb: None, old, new };
        for name in copied_names {
            if let Some(formula) = rename.text(&name.formula, si) {
                name.formula = formula;
            }
        }
        let columns: Vec<(usize, usize, Option<String>, Option<String>)> = self.sheets[si]
            .tables
            .iter()
            .enumerate()
            .flat_map(|(ti, t)| t.columns.iter().enumerate().map(move |(ci, c)| (ti, ci, c)))
            .filter_map(|(ti, ci, c)| {
                let formula = c.formula.as_deref().and_then(|f| rename.text(f, si));
                let label = c.totals_label.as_deref().filter(|_| c.totals == crate::TotalsFn::Custom).and_then(|f| rename.text(f, si));
                (formula.is_some() || label.is_some()).then_some((ti, ci, formula, label))
            })
            .collect();
        let cells = rename.cells(&self.sheets[si], si);
        rename.features(Arc::make_mut(&mut self.sheets[si]), si);
        self.set_formulas(si, cells);
        let names_old = |s: &crate::Slicer| matches!(&s.source, crate::SlicerSource::Table { table, .. } if table.eq_ignore_ascii_case(old));
        if columns.is_empty() && !self.sheets[si].slicers.iter().any(names_old) {
            return;
        }
        let sheet = Arc::make_mut(&mut self.sheets[si]);
        for (ti, ci, formula, label) in columns {
            let c = &mut sheet.tables[ti].columns[ci];
            if formula.is_some() {
                c.formula = formula;
            }
            if label.is_some() {
                c.totals_label = label;
            }
        }
        for sl in sheet.slicers.iter_mut() {
            if let crate::SlicerSource::Table { table, .. } = &mut sl.source
                && table.eq_ignore_ascii_case(old)
            {
                *table = new.to_string();
            }
        }
    }

    /// Puts rewritten formulas in sheet `i`'s cells.
    fn set_formulas(&mut self, i: usize, rewritten: Vec<(CellRef, Formula)>) {
        if rewritten.is_empty() {
            return;
        }
        let sheet = Arc::make_mut(&mut self.sheets[i]);
        for (c, f) in rewritten {
            if let Some(cell) = sheet.cells.get_mut(c) {
                cell.formula = Some(Arc::new(f));
            }
        }
    }

    /// Rebuilds the cached layout of every equation whose source changed (or was never laid
    /// out, as after loading a file). One call lays out at most [`MAX_EQUATION_LAYOUT_BYTES`] of
    /// source: one equation can take a tenth of a second to lay out, so a file with thousands of
    /// them would otherwise hold the app for minutes. Equations past the budget get an error
    /// instead (they show their source; editing one lays it out).
    pub fn refresh_equations(&mut self) {
        let stale = |s: &crate::Shape| s.kind == crate::ShapeKind::Equation && s.math.get(&s.text).is_none();
        let mut budget = MAX_EQUATION_LAYOUT_BYTES;
        // Only sheets with a stale equation are copied (sheets are shared with undo snapshots).
        for sh in self.sheets.iter_mut().filter(|sh| sh.shapes.iter().any(stale)) {
            for s in Arc::make_mut(sh).shapes.iter_mut().filter(|s| stale(s)) {
                // A source over the length cap never gets here: the cache answers for it without
                // a lookup. An empty source still costs a call.
                let cost = s.text.len().max(1);
                if cost > budget {
                    s.math.refuse(&s.text, EQUATION_BUDGET_MESSAGE);
                    continue;
                }
                budget -= cost;
                s.math.refresh(&s.text);
            }
        }
    }

    /// A free id for a new chart, picture, shape, table, slicer or form control: one more than the largest in
    /// use, or, when that is `u32::MAX` (a file can carry any id), the lowest free one.
    pub fn next_object_id(&self) -> u32 {
        self.free_object_ids(1).first().copied().unwrap_or(u32::MAX)
    }

    /// `count` distinct ids no object in the workbook uses, sorted: the ones above the largest id
    /// in use while they last, then the lowest free ones. Fewer come back only when the id space
    /// is exhausted.
    pub fn free_object_ids(&self, count: usize) -> Vec<u32> {
        let mut used: Vec<u32> = self
            .sheets
            .iter()
            .flat_map(|s| {
                s.charts
                    .iter()
                    .map(|o| o.id)
                    .chain(s.images.iter().map(|o| o.id))
                    .chain(s.shapes.iter().map(|o| o.id))
                    .chain(s.tables.iter().map(|o| o.id))
                    .chain(s.slicers.iter().map(|o| o.id))
                    // Members too: an id a group still lists (its object deleted) isn't handed out again.
                    .chain(s.groups.iter().flat_map(|g| {
                        std::iter::once(g.id).chain(g.members.iter().map(|r| {
                            let (crate::DrawingRef::Chart(id)
                            | crate::DrawingRef::Image(id)
                            | crate::DrawingRef::Shape(id)
                            | crate::DrawingRef::Slicer(id)
                            | crate::DrawingRef::Group(id)) = *r;
                            id
                        }))
                    }))
                    .chain(s.controls.iter().map(|o| o.id))
                    // And the objects kept hyperlinks belong to: a link read from the file stays
                    // with its object (deleted, undo brings both back), never a new one.
                    .chain(s.preserved.iter().flat_map(|p| p.object_links.iter().flat_map(|l| l.links.iter())).map(|l| {
                        let (crate::DrawingRef::Chart(id)
                        | crate::DrawingRef::Image(id)
                        | crate::DrawingRef::Shape(id)
                        | crate::DrawingRef::Slicer(id)
                        | crate::DrawingRef::Group(id)) = l.object;
                        id
                    }))
                    // Chart annotation snapshots, like links, refer to the original objects.
                    .chain(
                        s.preserved
                            .iter()
                            .flat_map(|p| p.chart_shapes.iter())
                            .flat_map(|cs| std::iter::once(cs.chart).chain(cs.shapes.iter().map(|o| o.id)).chain(cs.images.iter().map(|o| o.id))),
                    )
            })
            .collect();
        used.sort_unstable();
        used.dedup();
        let max = used.last().copied().unwrap_or(0);
        let mut out = Vec::with_capacity(count.min(1 << 16));
        let mut next = max.checked_add(1);
        while out.len() < count
            && let Some(id) = next
        {
            out.push(id);
            next = id.checked_add(1);
        }
        // Past the top: the gaps below the largest id, from 1 (0 is never an object's id).
        let mut taken = used.iter().copied().peekable();
        let mut id = 1u32;
        while out.len() < count && id < max {
            while taken.next_if(|&t| t < id).is_some() {}
            if taken.peek() == Some(&id) {
                taken.next();
            } else {
                out.push(id);
            }
            id += 1;
        }
        out.sort_unstable();
        out
    }
}

/// Allocates the first free `Table<n>` at or after `next`, advancing the cursor so batch
/// repair need not search occupied numbers again. `taken` receives the lowercase spelling.
pub fn next_table_name_with(next: &mut u64, mut taken: impl FnMut(&str) -> bool) -> String {
    use std::fmt::Write;

    let mut name = String::from("table");
    loop {
        name.truncate("table".len());
        let _ = write!(name, "{next}");
        *next += 1;
        if !taken(&name) {
            name[..1].make_ascii_uppercase();
            return name;
        }
    }
}

/// Whether the file format keeps `name` for itself, in any case: `_xlnm.` starts SpreadsheetML's
/// built-in names (ECMA-376 Part 1 §18.2.5: "End users shall not use this string for custom
/// names"), and `_xlchart.` the hidden names an Excel 2016 chart (histogram, waterfall…) reads its
/// data through, which the XLSX writer makes on save and the reader takes back as the charts'.
pub fn reserved_name(name: &str) -> bool {
    ["_xlnm.", "_xlchart."].iter().any(|p| name.get(..p.len()).is_some_and(|head| head.eq_ignore_ascii_case(p)))
}

/// Calls of the names in `lambdas` spelled as the names are.
fn spell_calls(e: Expr, lambdas: &[String]) -> Expr {
    if lambdas.is_empty() {
        return e;
    }
    e.map(&mut |x| match x {
        Expr::Call(n, args) => match lambdas.iter().find(|l| l.eq_ignore_ascii_case(&n)) {
            Some(l) => Expr::Call(l.clone(), args),
            None => Expr::Call(n, args),
        },
        other => other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Renaming a table rewrites a legacy array formula's text and keeps its array range.
    #[test]
    fn rename_table_keeps_the_array_range_of_a_formula_it_rewrites() {
        let mut wb = Workbook::new();
        let range = |a: &str| RangeRef::parse(a).unwrap_or_else(|| RangeRef::new(CellRef::new(0, 0), CellRef::new(0, 0)));
        let at = CellRef::new(0, 2);
        if let Some(sh) = wb.sheet_mut(0) {
            let col = |n: &str| crate::TableColumn {
                name: n.into(),
                totals: crate::TotalsFn::None,
                totals_label: None,
                formula: None,
                unique_name: None,
                xml: None,
            };
            sh.tables.push(crate::Table {
                id: 1,
                name: "Items".into(),
                range: range("A1:A3"),
                header_row: true,
                totals_row: false,
                columns: vec![col("V")],
                style: String::new(),
                banded_rows: false,
                banded_cols: false,
                first_col: false,
                last_col: false,
                filter_button: true,
                filter: vec![],
                table_type: None,
                connection_id: None,
                uid: Default::default(),
            });
            sh.set_cell(at, crate::Cell::formula(Formula { array: Some(range("C1:D1")), ..Formula::new("TRANSPOSE(Items[V])") }));
        }
        // Table slicers on any sheet follow the rename; one naming another table doesn't.
        let slicer = |table: &str| crate::Slicer {
            id: 0,
            source: crate::SlicerSource::Table { table: table.into(), column: "V".into(), selected: Some(vec!["1".into()]) },
            caption: String::new(),
            anchor: crate::Anchor::default(),
            columns: 1,
            name: String::new(),
            style: String::new(),
            row_height: None,
            imported_cache: Default::default(),
        };
        let mut other = crate::Sheet::new("Other");
        other.slicers = vec![slicer("ITEMS"), slicer("Elsewhere")];
        wb.sheets.push(Arc::new(other));
        wb.rename_table(0, 0, "Renamed");
        let f = wb.sheet(0).and_then(|sh| sh.cell(at)).and_then(|c| c.formula.clone());
        assert_eq!(f.as_deref().map(|f| f.text.as_str()), Some("TRANSPOSE(Renamed[V])"));
        assert_eq!(f.and_then(|f| f.array), Some(range("C1:D1")));
        let tables: Vec<&str> = wb.sheets[1]
            .slicers
            .iter()
            .filter_map(|s| match &s.source {
                crate::SlicerSource::Table { table, .. } => Some(table.as_str()),
                crate::SlicerSource::Pivot { .. } => None,
            })
            .collect();
        assert_eq!(tables, ["Renamed", "Elsewhere"]);
    }

    #[test]
    fn phonetics_are_additive_in_json() -> Result<(), Box<dyn std::error::Error>> {
        let mut wb = Workbook::new();
        assert!(wb.phonetics.is_empty(), "new workbooks have no phonetic data");
        wb.phonetics.push(PhoneticString {
            text: "東".into(),
            runs: vec![PhoneticRun { start: 0, end: 1, text: "ヒガシ".into() }],
            props: Some(PhoneticProps { font_id: 3, kind: Some("Hiragana".into()), alignment: None }),
        });
        let json = serde_json::to_string(&wb)?;
        assert_eq!(serde_json::from_str::<Workbook>(&json)?.phonetics, wb.phonetics);
        // JSON written before the field existed still loads.
        let mut value: serde_json::Value = serde_json::from_str(&json)?;
        value.as_object_mut().ok_or("none")?.remove("phonetics");
        assert!(serde_json::from_value::<Workbook>(value)?.phonetics.is_empty());
        Ok(())
    }

    #[test]
    fn sheet_names() {
        let wb = Workbook::new();
        assert_eq!(wb.next_sheet_name(), "Sheet2");
        assert!(wb.check_sheet_name("sheet1", None).is_err());
        assert!(wb.check_sheet_name("sheet1", Some(0)).is_ok());
        assert!(wb.check_sheet_name("a/b", None).is_err());
        assert!(wb.check_sheet_name(&"x".repeat(32), None).is_err());
        assert!(wb.check_sheet_name("Budget 2026", None).is_ok());
    }

    #[test]
    fn object_ids_never_overflow_or_repeat() {
        let mut wb = Workbook::new();
        let image = |id| crate::Image { id, anchor: Default::default(), data: Default::default(), mime: String::new(), alt: String::new() };
        assert_eq!(wb.next_object_id(), 1);
        if let Some(sh) = wb.sheet_mut(0) {
            sh.images.push(image(7));
        }
        assert_eq!(wb.next_object_id(), 8);
        if let Some(sh) = wb.sheet_mut(0) {
            sh.controls.push(crate::FormControl { id: 12, ..Default::default() });
        }
        assert_eq!(wb.next_object_id(), 13, "form controls take ids from the same sequence");
        if let Some(sh) = wb.sheet_mut(0) {
            sh.images.push(image(u32::MAX));
        }
        // An id read from a file at the maximum: the lowest free id, not the maximum again.
        assert_eq!(wb.next_object_id(), 1);
        if let Some(sh) = wb.sheet_mut(0) {
            sh.images.push(image(1));
            sh.images.push(image(3));
        }
        assert_eq!(wb.next_object_id(), 2);
        assert_eq!(wb.free_object_ids(4), vec![2, 4, 5, 6], "the gaps, skipping 1, 3 and 7");
        let mut wb = Workbook::new();
        if let Some(sh) = wb.sheet_mut(0) {
            sh.images.push(image(u32::MAX - 1));
        }
        assert_eq!(wb.free_object_ids(3), vec![1, 2, u32::MAX], "the top first, then from 1");
    }

    /// A form control's id is in use like any object's: the free ids skip it, also when it is the
    /// largest id a file could carry.
    #[test]
    fn free_object_ids_skip_a_form_controls_id() {
        let mut wb = Workbook::new();
        if let Some(sh) = wb.sheet_mut(0) {
            sh.controls.push(crate::FormControl { id: 3, ..Default::default() });
            sh.controls.push(crate::FormControl { id: u32::MAX, ..Default::default() });
        }
        assert_eq!(wb.free_object_ids(4), vec![1, 2, 4, 5], "3 and the largest id are taken");
        assert_eq!(wb.next_object_id(), 1);
    }

    /// A file can use the largest id and the one below it: nothing is free above them, so the free
    /// ids are the lowest unused ones from 1 (0 is never an object's id), and a large request is
    /// served in time linear in the count and the ids in use.
    #[test]
    fn free_object_ids_come_from_below_when_nothing_is_free_above() {
        let mut wb = Workbook::new();
        let image = |id| crate::Image { id, anchor: Default::default(), data: Default::default(), mime: String::new(), alt: String::new() };
        if let Some(sh) = wb.sheet_mut(0) {
            sh.images.push(image(u32::MAX));
            sh.images.push(image(u32::MAX - 1));
            sh.images.push(image(0));
        }
        assert_eq!(wb.free_object_ids(3), vec![1, 2, 3]);
        assert_eq!(wb.next_object_id(), 1);
        // The 100,000 lowest free ids: distinct, ascending, none of them in use.
        let many = wb.free_object_ids(100_000);
        assert_eq!(many.len(), 100_000);
        assert!(many.windows(2).all(|w| w[0] < w[1]), "ascending without repeats");
        assert!(!many.contains(&0) && !many.contains(&(u32::MAX - 1)) && !many.contains(&u32::MAX));
        assert_eq!(wb.free_object_ids(0), Vec::<u32>::new());
    }
}
