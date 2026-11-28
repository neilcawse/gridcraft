use gridcraft_core::Value;

use super::{CellRef, RangeRef};
use crate::sheet::SheetUid;
use crate::workbook::DefinitionUid;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------- pivot tables

/// How a PivotTable lays out its row fields.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PivotLayout {
    /// All row fields in one column, inner items indented ("Row Labels" header).
    #[default]
    Compact,
    /// One column per row field; each item on its own row.
    Outline,
    /// One column per row field; inner items start on their parent's row, totals below.
    Tabular,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PivotSort {
    #[default]
    Asc,
    Desc,
    /// Source order (first appearance).
    None,
}

/// Grouping of a date field (values are date serials).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PivotDateGroup {
    #[default]
    None,
    Years,
    Quarters,
    Months,
    Days,
}

impl PivotDateGroup {
    /// Finest first: days, months, quarters, years, then ungrouped.
    pub fn coarseness(self) -> u8 {
        match self {
            PivotDateGroup::Days => 0,
            PivotDateGroup::Months => 1,
            PivotDateGroup::Quarters => 2,
            PivotDateGroup::Years => 3,
            PivotDateGroup::None => 4,
        }
    }
}

/// An item of a field on the other axis that a sort by value takes its values from (e.g. the
/// "Apples" column). The field is named by source column and date grouping, so two levels
/// grouped from one date column (Years and Quarters) stay apart.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PivotScopeItem {
    pub source_col: String,
    pub date_group: PivotDateGroup,
    /// The item's label.
    pub item: String,
}

impl PivotScopeItem {
    /// Whether this names an item of `f`.
    pub fn is_of(&self, f: &PivotField) -> bool {
        self.date_group == f.date_group && self.source_col.eq_ignore_ascii_case(&f.source_col)
    }
}

/// A field placed in the Rows or Columns area.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PivotField {
    /// Source column header name.
    pub source_col: String,
    pub sort: PivotSort,
    pub date_group: PivotDateGroup,
    /// Item labels whose detail is hidden (collapsed).
    pub collapsed_items: Vec<String>,
    /// Custom name shown in the field's header (Field Settings > Custom Name); `None` = the
    /// source column name.
    pub caption: Option<String>,
    /// Sort the items by this value field (its caption) instead of by their labels, in the
    /// `sort` direction (Sort > More Sort Options > by value). Ignored when `sort` is `None`.
    pub sort_by: Option<String>,
    /// Where `sort_by` takes its values: an item of each field on the other axis, outer field
    /// first, up to the first field it doesn't name (e.g. sort the rows by the "Apples" column).
    /// Empty, or naming an item that isn't there, = the grand total.
    pub sort_scope: Vec<PivotScopeItem>,
    /// Manual item order (item labels), used when `sort` is `None`; unlisted items follow in
    /// source order (date groups in date order).
    pub item_order: Vec<String>,
    /// Renamed items: (item label, caption shown instead).
    pub item_captions: Vec<(String, String)>,
    /// Subtotals of this field; `None` = the table's `subtotals`.
    pub subtotal: Option<bool>,
    /// Repeat the item labels on every row (Outline/Tabular, Repeat Item Labels).
    pub repeat_labels: bool,
    /// Number format of the item labels (Field Settings > Number Format); `None` = the source
    /// column's format.
    pub number_format: Option<String>,
}

impl PivotField {
    /// The header text: the custom name, else the source column name.
    pub fn header(&self) -> &str {
        self.caption.as_deref().unwrap_or(&self.source_col)
    }
}

/// Summarize-values-by function.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PivotFunc {
    #[default]
    Sum,
    Count,
    Average,
    Max,
    Min,
    Product,
    CountNumbers,
    /// Number of different non-empty values (Excel's Distinct Count, a Data Model summary).
    /// A workbook file has no field for it: it is saved as Count.
    DistinctCount,
    StdDev,
    StdDevP,
    Var,
    VarP,
}

/// Show-values-as transform.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PivotShowAs {
    #[default]
    Normal,
    PercentOfGrandTotal,
    PercentOfColumnTotal,
    PercentOfRowTotal,
    /// Running total in the first row field.
    RunningTotal,
    /// Rank largest to smallest among sibling row items.
    Rank,
}

/// A field in the Values area.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PivotValue {
    pub source_col: String,
    pub func: PivotFunc,
    /// Caption, e.g. "Sum of Sales".
    pub name: String,
    pub show_as: PivotShowAs,
    pub number_format: Option<String>,
}

/// A report filter (or, for a field also in Rows/Columns, its hidden-items filter).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PivotFilter {
    pub source_col: String,
    /// Item labels shown; `None` = all.
    pub selected: Option<Vec<String>>,
    /// For a field outside Rows and Columns: it filters the PivotTable without being a report
    /// filter (no filter cell above the table). Set by a slicer on a field the PivotTable doesn't
    /// show, and read from files as hidden items of such a field.
    pub hidden: bool,
    /// The date grouping `selected` is in, when it isn't the first Rows/Columns field's on the
    /// column (`None`): a field in no area keeps its cache field's grouping here (ECMA-376 puts
    /// `fieldGroup` on the cache field), and on a field grouped at several levels it names the
    /// level filtered. See [`PivotTable::filter_group`].
    pub date_group: PivotDateGroup,
}

/// A PivotTable: its definition plus where its output was last written. The output itself is
/// static cell values and styles on the sheet, rewritten on every refresh.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PivotTable {
    pub id: u32,
    pub name: String,
    /// Source: range formula text (`Sheet1!$A$1:$E$200`), a table name or a defined name.
    pub source: String,
    /// Top-left of the output body on the pivot's sheet.
    pub anchor: CellRef,
    pub rows: Vec<PivotField>,
    pub columns: Vec<PivotField>,
    pub values: Vec<PivotValue>,
    pub filters: Vec<PivotFilter>,
    pub layout: PivotLayout,
    /// PivotTable Options › Data › Enable show details: Show Details lists a value cell's
    /// source records only while this is on.
    pub enable_show_details: bool,
    /// Grand total for each row (the "Grand Total" column), like Excel's "On for Rows".
    pub grand_totals_rows: bool,
    /// Grand total for each column (the "Grand Total" row), like Excel's "On for Columns".
    pub grand_totals_cols: bool,
    pub subtotals: bool,
    /// Subtotals at the top of each group (Compact/Outline); otherwise below it.
    pub subtotals_top: bool,
    pub style: String,
    /// Style display flags from `<pivotTableStyleInfo>` (ECMA-376 §18.10.1.74).
    #[serde(default)]
    pub style_options: PivotStyleOptions,
    /// Workbook-defined custom `<tableStyle>` elements from `xl/styles.xml`, when `style` names
    /// a custom PivotTable style rather than a built-in `PivotStyleLight/Medium/Dark` name.
    #[serde(default, skip_serializing_if = "PivotCustomStyle::is_none")]
    pub custom_style: PivotCustomStyle,
    /// Imported or refreshed grid layout metadata for non-destructive dynamic style resolution.
    #[serde(default, skip_serializing_if = "PivotStyleGrid::is_none")]
    pub style_grid: PivotStyleGrid,
    pub show_headers: bool,
    /// Area written by the last refresh.
    pub last_range: Option<RangeRef>,
    /// Header of the row labels in Compact layout; `None` = "Row Labels".
    pub row_header_caption: Option<String>,
    /// Header of the column labels in Compact layout; `None` = "Column Labels".
    pub col_header_caption: Option<String>,
    /// Caption of the Values field; `None` = "Values".
    pub data_caption: Option<String>,
    /// Caption of the grand totals; `None` = "Grand Total".
    pub grand_total_caption: Option<String>,
    /// Shown in value cells that summarize no records (For empty cells show); `None` = blank.
    pub empty_caption: Option<String>,
    /// Show the Values row: a header row with `data_caption` above the value captions
    /// (Outline/Tabular, several values and no column field). Excel's default is on.
    pub show_values_row: bool,
    /// Classic PivotTable layout (fields dragged in the grid): with one value field and no
    /// column field, the value caption sits above the row headers and the column reads "Total".
    pub classic_layout: bool,
    /// Calculated fields, usable in the Values area by name.
    pub calculated_fields: Vec<PivotCalcField>,
    /// Where GETPIVOTDATA finds the report's value cells, as of the last refresh. Shared between
    /// workbook clones (so editing the pivot's sheet doesn't copy it), set on refresh and saved
    /// with the report, so a reopened unrefreshed pivot still reads the report it stored. Only a
    /// pivot loaded without one (XLSX) builds it on first use, checked against the stored report.
    pub data_index: PivotLookup,
    /// The Σ Values pseudo-field (it exists with two or more value fields) sits in Rows rather
    /// than Columns (Excel's "values on rows").
    pub values_on_rows: bool,
    /// Position of Σ Values among the row fields (`None` = after the last). Only used with
    /// `values_on_rows`: in Columns Σ Values always comes after the last column field.
    pub values_position: Option<usize>,
    /// Imported pivot-cache group id, so PivotTables that intentionally used separate caches for
    /// the same source keep separate caches on save.
    #[serde(default, skip_serializing_if = "PivotCacheId::is_none")]
    pub cache_id: PivotCacheId,
    /// What the source stood for when the file was read: the exact defined name or table it
    /// resolved to ([`crate::Workbook::pivot_source`]). Not in JSON or the file.
    #[serde(skip)]
    pub binding: SourceBinding,
    /// XLSX import provenance, independent of subsequent source edits.
    #[serde(skip)]
    pub had_x14_cache_id: X14CacheId,
}

/// The definition an imported PivotTable's source was bound to, if any (a range, a source in
/// another workbook or one already missing in the file has none, nor does a source typed in the
/// app). Compares equal for any two values, like [`PivotCacheId`]: it is where the source text
/// led, not part of the PivotTable.
#[derive(Clone, Debug, Default)]
pub struct SourceBinding(pub Option<Binding>, pub Option<String>);

impl SourceBinding {
    /// True if this unchanged source was an unresolved local name in the input file.
    pub fn unresolved(&self, source: &str) -> bool {
        self.1.as_deref() == Some(source)
    }
}

impl PartialEq for SourceBinding {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

/// Whether the input cache carried an x14 ID. This does not describe the report itself.
#[derive(Clone, Copy, Debug, Default)]
pub struct X14CacheId(pub bool);

impl PartialEq for X14CacheId {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

/// A PivotTable source bound to one definition.
#[derive(Clone, Debug)]
pub struct Binding {
    /// The source text (`PivotTable::source`) the binding was made for. A source changed since
    /// is not bound; a sheet rename rewrites both.
    pub text: String,
    pub kind: BindingKind,
    /// The identity of the defined name or table.
    pub def: DefinitionUid,
    /// How the definition is described in a save warning once it is gone (`sheet "Data"'s name
    /// "rData"`).
    pub was: String,
    /// Source text and supported attributes as imported. Kept parts retain the original bytes;
    /// rebuilt local sources serialize only these attributes.
    pub read_as: Option<(String, WorksheetSourceAttrs)>,
}

/// Unqualified attributes of an imported local, named worksheet source.
#[derive(Clone, Debug, PartialEq)]
pub struct WorksheetSourceAttrs {
    pub name: String,
    pub sheet: Option<String>,
    pub reference: Option<String>,
}

/// What kind of definition a source is bound to, which decides what may replace it once it is
/// gone: only a definition of the same kind and scope.
#[derive(Clone, Copy, Debug)]
pub enum BindingKind {
    /// A bare name (`worksheetSource name`), looked up as a formula would from the PivotTable's
    /// sheet.
    Name,
    /// A bare table name.
    Table,
    /// `name` with `sheet`: the name of that sheet.
    SheetName(SheetUid),
    /// `name` with `sheet`: a workbook name whose reference is on that sheet.
    WorkbookName(SheetUid),
    /// `name` with `sheet`: a table on that sheet.
    SheetTable(SheetUid),
}

/// Imported pivot-cache group id. Compares equal for any two values in [`PartialEq`] so that
/// workbook round-trip assertions comparing logical [`PivotTable`] models ignore package-level
/// cache numbering while the XLSX writer still preserves intentional cache separation via `self.0`.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PivotCacheId(pub Option<u32>);

impl PivotCacheId {
    pub fn is_none(&self) -> bool {
        self.0.is_none()
    }
}

impl PartialEq for PivotCacheId {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}
impl Eq for PivotCacheId {}

/// Style display flags from `<pivotTableStyleInfo>` (ECMA-376 §18.10.1.74).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PivotStyleOptions {
    pub show_row_headers: bool,
    pub show_col_headers: bool,
    pub show_row_stripes: bool,
    pub show_col_stripes: bool,
    pub show_last_column: bool,
}

impl Default for PivotStyleOptions {
    fn default() -> Self {
        Self { show_row_headers: true, show_col_headers: true, show_row_stripes: false, show_col_stripes: false, show_last_column: true }
    }
}

fn one_u32() -> u32 {
    1
}

/// One `<tableStyleElement>` resolved from `xl/styles.xml` (`<tableStyles>` + `<dxfs>`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PivotStyleElement {
    /// OOXML `ST_TableStyleType` (e.g. `"headerRow"`, `"firstRowStripe"`, `"firstSubtotalRow"`,
    /// `"totalRow"`).
    pub kind: String,
    /// Band width in rows/columns for stripe elements (default 1).
    #[serde(default = "one_u32")]
    pub size: u32,
    /// Differential format resolved from `<dxfs>`.
    pub style: crate::style::Style,
}

/// Workbook-defined custom `<tableStyle>` attached to a [`PivotTable`]. Shared via [`std::sync::Arc`]
/// so workbook clones stay cheap, and compares equal in [`PartialEq`] like [`PivotCacheId`] so
/// logical model equality focuses on the PivotTable definition.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PivotCustomStyle(pub Option<std::sync::Arc<Vec<PivotStyleElement>>>);

impl PivotCustomStyle {
    pub fn is_none(&self) -> bool {
        self.0.is_none()
    }
}

impl PartialEq for PivotCustomStyle {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}
impl Eq for PivotCustomStyle {}

/// Role of one body row starting at `body_range.start.row + first_data_row`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PivotRowRole {
    Data,
    /// Subtotal row at nesting depth `0` (firstSubtotalRow), `1` (secondSubtotalRow), or `2+`.
    Subtotal(u8),
    GrandTotal,
    Blank,
}

/// Role of one data column starting at `body_range.start.col + first_data_col`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PivotColRole {
    Data,
    /// Subtotal column at nesting depth `0`, `1`, or `2+`.
    Subtotal(u8),
    GrandTotal,
    Blank,
}

/// Layout metadata of the PivotTable's rendered cells for dynamic style resolution without
/// mutating underlying cell styles.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PivotStyleLayout {
    /// Body range (`<location ref="..."/>`), excluding report filter rows above `anchor`.
    pub body_range: Option<RangeRef>,
    /// 0-based row offset from `body_range.start.row` where column headers start (`firstHeaderRow`).
    pub first_header_row: u32,
    /// 0-based row offset from `body_range.start.row` where data rows begin (`firstDataRow`).
    pub first_data_row: u32,
    /// 0-based column offset from `body_range.start.col` where data columns begin (`firstDataCol`).
    pub first_data_col: u32,
    /// Number of report filter rows above `body_range` (`rowPageCount`).
    pub page_rows: u32,
    /// Number of report filter column pairs (`colPageCount`).
    pub page_cols: u32,
    /// Role of each row starting at `body_range.start.row + first_data_row`.
    pub row_roles: Vec<PivotRowRole>,
    /// Subheading depth (`0`..=`2`) for parent-level group rows, used when `show_row_headers` is on.
    pub row_subheading: Vec<Option<u8>>,
    /// Role of each column starting at `body_range.start.col + first_data_col`.
    pub col_roles: Vec<PivotColRole>,
    /// Subheading depth (`0`..=`2`) for parent-level column groups, used when `show_col_headers` is on.
    pub col_subheading: Vec<Option<u8>>,
}

/// Shared layout cache for PivotTable dynamic style resolution. Compares equal in [`PartialEq`]
/// like [`PivotLookup`] and [`PivotCacheId`] so logical round-trip tests compare definitions.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PivotStyleGrid(pub Option<std::sync::Arc<PivotStyleLayout>>);

impl PivotStyleGrid {
    pub fn is_none(&self) -> bool {
        self.0.is_none()
    }
}

impl PartialEq for PivotStyleGrid {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}
impl Eq for PivotStyleGrid {}

/// A calculated field: a formula over other fields (`=Sales*0.1`, `='Unit Price'*Units`). As in
/// Excel, it is evaluated on the sums of the fields it names for each report cell, not per record.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PivotCalcField {
    pub name: String,
    /// Formula text, starting with `=`.
    pub formula: String,
}

impl PivotTable {
    fn axis(&self) -> impl Iterator<Item = &PivotField> {
        self.rows.iter().chain(self.columns.iter())
    }

    /// The grouping a filter on `col` gets when it names none: the first Rows/Columns field's.
    pub fn implied_group(&self, col: &str) -> PivotDateGroup {
        self.axis().find(|f| f.source_col.eq_ignore_ascii_case(col)).map(|f| f.date_group).unwrap_or_default()
    }

    /// The date grouping a filter's labels are in.
    pub fn filter_group(&self, f: &PivotFilter) -> PivotDateGroup {
        if f.date_group == PivotDateGroup::None { self.implied_group(&f.source_col) } else { f.date_group }
    }

    /// Clears each filter's `date_group` where the first Rows/Columns field implies it, so one
    /// filter has one representation.
    pub fn normalize_filter_groups(&mut self) {
        let implied: Vec<PivotDateGroup> = self.filters.iter().map(|f| self.implied_group(&f.source_col)).collect();
        for (f, g) in self.filters.iter_mut().zip(implied) {
            if f.date_group == g {
                f.date_group = PivotDateGroup::None;
            }
        }
    }

    /// The grouping of `col`'s base cache field: the finest of its Rows/Columns fields' and its
    /// filter's, as a cache groups the base field by the finest level. A slicer on `col` lists
    /// these items.
    pub fn field_group(&self, col: &str) -> PivotDateGroup {
        let filter = self.filters.iter().filter(|f| f.source_col.eq_ignore_ascii_case(col)).map(|f| self.filter_group(f));
        let axis = self.axis().filter(|f| f.source_col.eq_ignore_ascii_case(col)).map(|f| f.date_group);
        axis.chain(filter).min_by_key(|g| g.coarseness()).unwrap_or_default()
    }

    /// Sets the filter on `col` to `selected` (`None`: everything) with its labels in `group`,
    /// adding a hidden filter for a field in no area. Clearing a filter removes it, except a
    /// report filter and a filter that holds the field's grouping (none of its Rows/Columns
    /// fields has it), which keep `selected: None`.
    pub fn set_filter(&mut self, col: &str, selected: Option<Vec<String>>, group: PivotDateGroup) {
        let on_axis = self.axis().any(|f| f.source_col.eq_ignore_ascii_case(col));
        let date_group = if group == self.implied_group(col) { PivotDateGroup::None } else { group };
        let i = self.filters.iter().position(|f| f.source_col.eq_ignore_ascii_case(col));
        match (i, selected) {
            (Some(i), None) => {
                let Some(f) = self.filters.get(i) else { return };
                let grouping = f.date_group != PivotDateGroup::None
                    && !self.axis().any(|a| a.source_col.eq_ignore_ascii_case(col) && a.date_group == f.date_group);
                if grouping || !(on_axis || f.hidden) {
                    if let Some(f) = self.filters.get_mut(i) {
                        f.selected = None;
                    }
                } else {
                    self.filters.remove(i);
                }
            }
            (Some(i), Some(sel)) => {
                if let Some(f) = self.filters.get_mut(i) {
                    f.selected = Some(sel);
                    f.date_group = date_group;
                }
            }
            (None, Some(sel)) => self.filters.push(PivotFilter { source_col: col.to_string(), selected: Some(sel), hidden: !on_axis, date_group }),
            (None, None) => {}
        }
    }
}

/// One value cell of a computed PivotTable report: the value field, the row and column items
/// that locate it (fewer for subtotals, none for the grand total) and where the report shows it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PivotDataCell {
    /// The value field's caption ("Sum of Sales").
    pub data_field: String,
    /// The value field's source column ("Sales").
    pub source_col: String,
    pub items: Vec<PivotItemRef>,
    /// Row and column of the cell, counted from the report's top-left cell.
    pub row: u32,
    pub col: u32,
}

/// Failure building a PivotTable's GETPIVOTDATA index from its source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PivotIndexError {
    /// The stored report no longer matches the source; lookup must return `#REF!` until refresh.
    Stale,
    /// The source could not be resolved or computed (e.g. external workbook or missing report).
    Other(String),
}

/// Builds a PivotTable's value cells from its source (`sheet` is the pivot's sheet). The engine
/// sets it; the model can't compute reports.
pub type PivotIndexer = fn(&crate::Workbook, usize, &PivotTable) -> Result<Vec<PivotDataCell>, PivotIndexError>;
pub static PIVOT_INDEXER: std::sync::OnceLock<PivotIndexer> = std::sync::OnceLock::new();

fn norm(s: &str) -> String {
    s.trim().to_lowercase()
}

/// One part of a lookup key: length-prefixed, so no label text can imitate a separator.
fn key_part(out: &mut String, s: &str) {
    out.push_str(&s.len().to_string());
    out.push(':');
    out.push_str(s);
}

/// The lookup key of a value field with field/item labels (names ignore case and outer blanks).
/// Each item is `(field, label, is_bool)`: a Boolean item and a text item with the same label
/// are different items.
pub fn pivot_key(data_field: &str, items: &[(&str, &str, bool)]) -> String {
    let mut parts: Vec<String> = items
        .iter()
        .map(|(f, l, b)| {
            let mut p = String::new();
            key_part(&mut p, &norm(f));
            key_part(&mut p, &norm(l));
            p.push(if *b { 'b' } else { 't' });
            p
        })
        .collect();
    parts.sort_unstable();
    let mut key = String::new();
    key_part(&mut key, &norm(data_field));
    key.push_str(&parts.len().to_string());
    key.push('|');
    for p in parts {
        key.push_str(&p);
    }
    key
}

#[derive(Default)]
struct LookupInner {
    cells: std::sync::OnceLock<Vec<PivotDataCell>>,
    keys: std::sync::OnceLock<std::collections::HashMap<String, usize>>,
}

/// A PivotTable's GETPIVOTDATA index. Clones share it, so `Workbook` clones per command stay
/// cheap however large the report is. The cells are set on refresh or on first use; the hash of
/// them is built on the first lookup.
#[derive(Clone, Default)]
pub struct PivotLookup(std::sync::Arc<LookupInner>);

impl PivotLookup {
    /// The value cells, if they were set.
    pub fn cells(&self) -> Option<&[PivotDataCell]> {
        self.0.cells.get().map(Vec::as_slice)
    }

    /// Sets the value cells (the first set wins) and returns them.
    pub fn set_cells(&self, cells: Vec<PivotDataCell>) -> &[PivotDataCell] {
        self.0.cells.get_or_init(|| cells)
    }

    /// The cell with this value field and exactly these items, by hash.
    pub fn find(&self, data_field: &str, items: &[(&str, &str, bool)]) -> Option<&PivotDataCell> {
        let cells = self.0.cells.get()?;
        let keys = self.0.keys.get_or_init(|| {
            let mut m = std::collections::HashMap::with_capacity(cells.len());
            for (i, c) in cells.iter().enumerate() {
                let items: Vec<(&str, &str, bool)> =
                    c.items.iter().map(|it| (it.field.as_str(), it.label.as_str(), matches!(it.value, Value::Bool(_)))).collect();
                m.entry(pivot_key(&c.data_field, &items)).or_insert(i);
                m.entry(pivot_key(&c.source_col, &items)).or_insert(i);
            }
            m
        });
        keys.get(&pivot_key(data_field, items)).and_then(|i| cells.get(*i))
    }
}

impl std::fmt::Debug for PivotLookup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PivotLookup({} cells)", self.cells().map_or(0, <[PivotDataCell]>::len))
    }
}

/// Saved as the list of value cells (or nothing before the first refresh); the hash is rebuilt.
/// An empty list is never saved: it is no index, and restoring one would lock the shared lookup
/// against the lazy rebuild that a reopened workbook needs.
impl Serialize for PivotLookup {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.cells().filter(|cells| !cells.is_empty()).serialize(s)
    }
}

impl<'de> Deserialize<'de> for PivotLookup {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let l = PivotLookup::default();
        if let Some(cells) = Option::<Vec<PivotDataCell>>::deserialize(d)?
            && !cells.is_empty()
        {
            l.set_cells(cells);
        }
        Ok(l)
    }
}

/// The index is a cache, not part of a PivotTable's identity.
impl PartialEq for PivotLookup {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

/// A row or column field item: the field's source column, the item's label and its value.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PivotItemRef {
    pub field: String,
    pub label: String,
    pub value: Value,
}

impl Default for PivotTable {
    fn default() -> Self {
        PivotTable {
            id: 0,
            name: "PivotTable1".into(),
            source: String::new(),
            anchor: CellRef::default(),
            rows: vec![],
            columns: vec![],
            values: vec![],
            filters: vec![],
            layout: PivotLayout::Compact,
            enable_show_details: true,
            grand_totals_rows: true,
            grand_totals_cols: true,
            subtotals: true,
            subtotals_top: true,
            style: "PivotStyleLight16".into(),
            style_options: PivotStyleOptions::default(),
            custom_style: PivotCustomStyle::default(),
            style_grid: PivotStyleGrid::default(),
            show_headers: true,
            last_range: None,
            row_header_caption: None,
            col_header_caption: None,
            data_caption: None,
            grand_total_caption: None,
            empty_caption: None,
            show_values_row: true,
            classic_layout: false,
            calculated_fields: vec![],
            data_index: PivotLookup::default(),
            values_on_rows: false,
            values_position: None,
            cache_id: PivotCacheId::default(),
            binding: SourceBinding::default(),
            had_x14_cache_id: X14CacheId::default(),
        }
    }
}

#[cfg(test)]
mod pivot_lookup_tests {
    use super::*;

    fn cell(field: &str, label: &str, value: Value, row: u32) -> PivotDataCell {
        PivotDataCell {
            data_field: "Sum of Sales".into(),
            source_col: "Sales".into(),
            items: vec![PivotItemRef { field: field.into(), label: label.into(), value }],
            row,
            col: 1,
        }
    }

    #[test]
    fn labels_with_separator_characters_do_not_collide() {
        let l = PivotLookup::default();
        // Two different (field, label) pairs that a separator-joined key would make identical.
        l.set_cells(vec![cell("A\u{2}B", "C", Value::Text("C".into()), 1), cell("A", "B\u{2}C", Value::Text("B\u{2}C".into()), 2)]);
        assert_eq!(l.find("Sales", &[("A\u{2}B", "C", false)]).unwrap().row, 1);
        assert_eq!(l.find("Sales", &[("A", "B\u{2}C", false)]).unwrap().row, 2);
        // A label carrying the old pair separator cannot stand for two items.
        assert!(l.find("Sales", &[("A", "B", false), ("C", "D", false)]).is_none());
    }

    #[test]
    fn boolean_and_text_items_with_one_label_are_different_items() {
        let l = PivotLookup::default();
        l.set_cells(vec![cell("Flag", "TRUE", Value::Text("TRUE".into()), 1), cell("Flag", "TRUE", Value::Bool(true), 2)]);
        assert_eq!(l.find("Sales", &[("Flag", "TRUE", true)]).unwrap().row, 2);
        assert_eq!(l.find("Sales", &[("Flag", "TRUE", false)]).unwrap().row, 1);
    }

    #[test]
    fn index_round_trips_through_serde() {
        let l = PivotLookup::default();
        l.set_cells(vec![cell("Region", "East", Value::Text("East".into()), 1)]);
        let back: PivotLookup = serde_json::from_str(&serde_json::to_string(&l).unwrap()).unwrap();
        assert_eq!(back.find("Sales", &[("Region", "East", false)]).unwrap().row, 1);
        let none: PivotLookup = serde_json::from_str(&serde_json::to_string(&PivotLookup::default()).unwrap()).unwrap();
        assert!(none.cells().is_none());
    }

    #[test]
    fn an_empty_lookup_is_saved_as_no_index() {
        // An empty cell list is no index, so it must not be saved as an empty one: restoring an
        // empty index counts as an initialized lookup, which blocks the lazy rebuild.
        let l = PivotLookup::default();
        l.set_cells(vec![]);
        let saved = serde_json::to_string(&l).unwrap();
        assert_eq!(saved, "null", "an empty lookup saves no index: {saved}");
        let back: PivotLookup = serde_json::from_str("[]").unwrap();
        assert!(back.cells().is_none(), "an empty saved list restores no index");
    }
}

#[cfg(test)]
mod pivot_style_model_tests {
    use std::sync::Arc;

    use super::*;
    use crate::sheet::Sheet;
    use crate::style::{Color, Fill, Style};

    #[test]
    fn style_options_defaults_and_serde_roundtrip() {
        let opts = PivotStyleOptions::default();
        assert!(opts.show_row_headers);
        assert!(opts.show_col_headers);
        assert!(!opts.show_row_stripes);
        assert!(!opts.show_col_stripes);
        assert!(opts.show_last_column);

        let custom = PivotStyleOptions {
            show_row_headers: false,
            show_col_headers: true,
            show_row_stripes: true,
            show_col_stripes: true,
            show_last_column: false,
        };
        let json = serde_json::to_string(&custom).unwrap();
        let back: PivotStyleOptions = serde_json::from_str(&json).unwrap();
        assert_eq!(back, custom);
    }

    #[test]
    fn custom_style_and_style_grid_do_not_break_logical_equality() {
        let mut a = PivotTable::default();
        let mut b = PivotTable::default();
        let hdr = Style { fill: Fill::solid(Color::Rgb(0x1F4E79)), ..Style::default() };
        a.custom_style = PivotCustomStyle(Some(Arc::new(vec![PivotStyleElement { kind: "headerRow".into(), size: 1, style: hdr }])));
        a.style_grid = PivotStyleGrid(Some(Arc::new(PivotStyleLayout {
            body_range: Some(RangeRef::new(CellRef::new(1, 0), CellRef::new(5, 2))),
            first_header_row: 0,
            first_data_row: 1,
            first_data_col: 1,
            page_rows: 0,
            page_cols: 1,
            row_roles: vec![PivotRowRole::Data, PivotRowRole::Subtotal(0), PivotRowRole::Data, PivotRowRole::GrandTotal],
            row_subheading: vec![None; 4],
            col_roles: vec![PivotColRole::Data, PivotColRole::GrandTotal],
            col_subheading: vec![None; 2],
        })));
        assert_eq!(a, b);
        // Changing style_options DOES affect equality.
        b.style_options.show_row_stripes = true;
        assert_ne!(a, b);
    }

    #[test]
    fn sheet_pivot_at_matches_last_range() {
        let mut sh = Sheet::new("Summary");
        let pt = PivotTable {
            id: 1,
            name: "PT1".into(),
            anchor: CellRef::new(3, 1),
            last_range: Some(RangeRef::new(CellRef::new(1, 1), CellRef::new(8, 4))),
            ..PivotTable::default()
        };
        sh.pivots.push(pt);
        assert!(sh.pivot_at(CellRef::new(1, 1)).is_some());
        assert!(sh.pivot_at(CellRef::new(5, 3)).is_some());
        assert!(sh.pivot_at(CellRef::new(0, 1)).is_none());
        assert!(sh.pivot_at(CellRef::new(9, 1)).is_none());
    }
}
