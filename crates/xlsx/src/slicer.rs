//! Slicers ([MS-XLSX] §2.3): a drawing frame (`sle:slicer`, in a markup-compatibility block with a
//! fallback shape) places the slicer, a slicer part (`xl/slicers/slicerN.xml`) holds its caption
//! and layout, and a slicer cache (`xl/slicerCaches/slicerCacheN.xml`) names what it filters.
//!
//! - A PivotTable slicer (Excel 2010, `x14`) is listed by the sheet's and workbook's `x14` lists; its
//!   cache names the PivotTables, the field and the selected items.
//! - A table slicer (Excel 2013, `x15`) is listed by the `x15` lists; its cache names the table and
//!   column by file id (`x15:tableSlicerCache`, [MS-XLSX] 2.6.97) and holds no items: the selection
//!   is the table's AutoFilter on the column.
//!
//! Reading: frames and slicer parts are collected while sheets are read, then [`resolve`] joins
//! them with the slicer caches once the PivotTables exist. A PivotTable slicer's selection lives in
//! the model as the PivotTables' filters on the field (read from their hidden items); a cache's `s`
//! flags only fill in a PivotTable that has no filter on the field. Table slicers are bound by
//! [`attach_tables`] once table names are repaired. OLAP slicers, and slicers on a coarser date
//! level that has its own cache field, are skipped with a warning.
//!
//! Writing: [`SlicerWriter`] plans the slicers before the sheets are written, so the PivotTable
//! writer can give their fields shared items and their caches an id. Both kinds share one set of
//! slicer and cache names. The sheets write the frames and slicer parts, and record the ids their
//! tables are written with; [`SlicerWriter::finish`] writes the caches. A table slicer whose table
//! or column is not written is dropped with a warning.

use std::fmt::Write as _;

use gridcraft_model::{
    Anchor, PivotDateGroup, PivotFilter, PivotSort, PivotTable, Sheet, Slicer, SlicerCacheOrigin, SlicerCrossFilter, SlicerPivot, SlicerSource,
    TableSlicerItems, Workbook,
};

use crate::package::Rel;
use crate::read::Ctx;
use crate::tables::EMU_PER_PX;
use crate::write::{Out, Rels, XML_DECL};
use crate::xml::{El, decode_xstring, encode_xstring, esc_attr};

pub const CT_SLICER: &str = "application/vnd.ms-excel.slicer+xml";
pub const CT_SLICER_CACHE: &str = "application/vnd.ms-excel.slicerCache+xml";
const REL_SLICER: &str = "http://schemas.microsoft.com/office/2007/relationships/slicer";
const REL_SLICER_CACHE: &str = "http://schemas.microsoft.com/office/2007/relationships/slicerCache";
const NS_X14: &str = "http://schemas.microsoft.com/office/spreadsheetml/2009/9/main";
const NS_X15: &str = "http://schemas.microsoft.com/office/spreadsheetml/2010/11/main";
const NS_MC: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
/// `a:graphicData` uri (and `sle` namespace) of a slicer frame.
pub const FRAME_URI: &str = "http://schemas.microsoft.com/office/drawing/2010/slicer";
/// The namespace a table slicer frame's markup-compatibility choice requires.
const NS_SLE15: &str = "http://schemas.microsoft.com/office/drawing/2012/slicer";
const EXT_SHEET_SLICERS: &str = "{A8765BA9-456A-4dab-B4F3-ACF838C121DE}";
const EXT_WB_SLICER_CACHES: &str = "{BBE1A952-AA13-448e-AADC-164F8A28A991}";
/// The sheet's `x14:slicerList` extension for table slicer parts.
const EXT_SHEET_TABLE_SLICERS: &str = "{3A4CF648-6AED-40f4-86FF-DC5316D8AED3}";
/// The workbook's `x15` list of table slicer caches.
const EXT_WB_TABLE_SLICER_CACHES: &str = "{46BE6895-7355-4a93-B00E-2C351335B9C9}";
/// The slicer cache's extension holding `x15:tableSlicerCache`.
const EXT_TABLE_SLICER_CACHE: &str = "{2F2917AC-EB37-4324-AD4E-5DD8C200BD13}";
/// Excel's default button height (EMU), written when the model has none.
const DEFAULT_ROW_HEIGHT: u32 = 241_300;
/// Most items read from one slicer cache.
const MAX_ITEMS: usize = 1_048_576;
/// Most slicers read from one workbook.
const MAX_SLICERS: usize = 4096;

// ---------------------------------------------------------------- reading

/// Slicer pieces found while reading sheets.
#[derive(Default)]
pub struct SlicersIn {
    /// (sheet name, slicer name, anchor, reserved object ID) of each drawing frame.
    frames: Vec<(String, String, Anchor, u32)>,
    /// (sheet name, slicer) of the slicers the sheets list.
    parts: Vec<(String, PartIn)>,
    /// Table slicers joined with their caches, to bind once table names are repaired.
    tables: Vec<TableIn>,
}

/// One `<slicer>` of a slicer part.
struct PartIn {
    name: String,
    cache: String,
    caption: String,
    columns: u32,
    style: String,
    row_height: Option<f32>,
}

/// A table slicer read with its frame and cache.
struct TableIn {
    /// Model sheet index of the slicer.
    sheet: usize,
    part: PartIn,
    anchor: Anchor,
    id: u32,
    table: TableCache,
    /// The cache's `sourceName`: the column's header.
    source: String,
    origin: SlicerCacheOrigin,
}

/// A slicer cache definition.
struct CacheDef {
    name: String,
    source: String,
    /// (`tabId`, PivotTable name).
    pivots: Vec<(u32, String)>,
    /// `None`: not a PivotTable (tabular) cache.
    tabular: Option<Tabular>,
    /// `x15:tableSlicerCache`, when the cache names a table.
    table: Option<TableCache>,
    olap: bool,
}

struct Tabular {
    cache_id: Option<u32>,
    sort: PivotSort,
    cross: SlicerCrossFilter,
    show_missing: bool,
    /// (shared item index, selected).
    items: Vec<(u32, bool)>,
}

/// `x15:tableSlicerCache`: the file ids of the table and its column, and the item options.
#[derive(Clone, Copy)]
struct TableCache {
    table_id: Option<u32>,
    column_id: Option<u32>,
    items: TableSlicerItems,
}

impl CacheDef {
    /// Whether the cache filters a table: it says so, or it names no PivotTable and has no
    /// PivotTable or OLAP data (the column is then found by `sourceName`).
    fn is_table(&self) -> bool {
        self.table.is_some() || (self.pivots.is_empty() && self.tabular.is_none() && !self.olap)
    }
}

fn cross_filter(v: Option<&str>) -> SlicerCrossFilter {
    match v {
        Some("none") => SlicerCrossFilter::None,
        Some("showItemsWithNoData") => SlicerCrossFilter::ShowItemsWithNoData,
        _ => SlicerCrossFilter::ShowItemsWithDataAtTop,
    }
}

/// Records a drawing frame for delayed attachment and returns its reserved model ID.
pub fn read_frame(cx: &mut Ctx<'_>, sheet: &str, gd: &El, anchor: Anchor) -> Option<u32> {
    let name = gd.child("slicer")?.attr("name")?;
    if gd.attr("uri") != Some(FRAME_URI) || cx.slicers.frames.len() >= MAX_SLICERS {
        return None;
    }
    let id = cx.next_object_id();
    cx.slicers.frames.push((sheet.to_owned(), name.to_owned(), anchor, id));
    Some(id)
}

/// Reads a worksheet's slicer parts: those its relationships name (which its `x14` and `x15`
/// `slicerList`s list). A part that can't be read is skipped (its frame then warns); only a part
/// over the size limit fails the read.
pub fn read_sheet_parts(cx: &mut Ctx<'_>, rels: &[Rel], sheet: &str) -> Result<(), crate::IoError> {
    for rel in rels.iter().filter(|r| !r.external && r.kind.eq_ignore_ascii_case("slicer")) {
        let target = rel.target.clone();
        let x = match cx.optional_xml(&target) {
            Ok(Some(x)) => x,
            Err(crate::IoError::TooLarge(m)) => return Err(crate::IoError::TooLarge(m)),
            Ok(None) | Err(_) => continue,
        };
        for s in x.kids("slicer") {
            if cx.slicers.parts.len() >= MAX_SLICERS {
                return Ok(());
            }
            let part = PartIn {
                name: s.attr("name").unwrap_or("").to_string(),
                cache: s.attr("cache").unwrap_or("").to_string(),
                caption: decode_xstring(s.attr("caption").unwrap_or("")),
                columns: s.attr_u32("columnCount").unwrap_or(1).clamp(1, 20_000),
                style: s.attr("style").unwrap_or("").to_string(),
                row_height: s.attr_u32("rowHeight").filter(|h| *h > 0).map(|h| (h as f64 / EMU_PER_PX) as f32),
            };
            cx.slicers.parts.push((sheet.to_string(), part));
        }
    }
    Ok(())
}

fn read_cache_def(x: &El) -> Option<CacheDef> {
    if x.name != "slicerCacheDefinition" {
        return None;
    }
    let pivots = x
        .child("pivotTables")
        .map(|p| p.kids("pivotTable").filter_map(|t| Some((t.attr_u32("tabId")?, t.attr("name")?.to_string()))).collect())
        .unwrap_or_default();
    let tabular = x.path(&["data", "tabular"]).map(|t| Tabular {
        cache_id: t.attr_u32("pivotCacheId"),
        sort: if t.attr("sortOrder") == Some("descending") { PivotSort::Desc } else { PivotSort::Asc },
        cross: cross_filter(t.attr("crossFilter")),
        show_missing: t.flag("showMissing", true),
        items: t
            .child("items")
            .map(|i| i.kids("i").filter_map(|e| Some((e.attr_u32("x")?, e.flag("s", false)))).take(MAX_ITEMS).collect())
            .unwrap_or_default(),
    });
    let table = x.child("extLst").and_then(|l| l.kids("ext").find_map(|e| e.child("tableSlicerCache"))).map(|t| TableCache {
        // Zero is never a recorded table id. Do not treat malformed ids as absent.
        table_id: t.attr("tableId").map(|id| id.trim().parse().unwrap_or(0)),
        column_id: t.attr_u32("column"),
        items: TableSlicerItems {
            descending: t.attr("sortOrder") == Some("descending"),
            custom_list_sort: t.flag("customListSort", true),
            cross_filter: cross_filter(t.attr("crossFilter")),
        },
    });
    Some(CacheDef {
        name: x.attr("name").unwrap_or("").to_string(),
        source: x.attr("sourceName").unwrap_or("").to_string(),
        pivots,
        tabular,
        table,
        olap: x.path(&["data", "olap"]).is_some(),
    })
}

/// The imported cache's name and its workbook marker (a workbook-scoped `#N/A` name of the same
/// spelling), if any.
fn origin(wb: &Workbook, def: &CacheDef) -> SlicerCacheOrigin {
    let marker = wb.names.iter().find(|n| n.scope.is_none() && n.name.eq_ignore_ascii_case(&def.name)).filter(|n| is_cache_marker(n));
    SlicerCacheOrigin { name: def.name.clone(), marker: marker.map(|n| n.uid), table_items: def.table.map(|t| t.items) }
}

/// Joins the collected frames and slicer parts with the workbook's slicer caches and adds the
/// PivotTable slicers to their sheets; table slicers wait for [`attach_tables`]. `sheet_els` are
/// the workbook's `<sheet>` elements and `file_to_model` maps each to a model sheet.
pub fn resolve(
    cx: &mut Ctx<'_>,
    wb_rels: &[Rel],
    sheet_els: &[&El],
    file_to_model: &[Option<usize>],
    caches: &crate::pivot::CachesIn,
    wb: &mut Workbook,
) {
    let found = std::mem::take(&mut cx.slicers);
    if found.frames.is_empty() && found.parts.is_empty() {
        return;
    }
    // The caches the workbook's relationships name (which its `x14` and `x15` `slicerCaches`
    // list).
    let mut defs: Vec<CacheDef> = Vec::new();
    for rel in wb_rels.iter().filter(|r| !r.external && r.kind.eq_ignore_ascii_case("slicerCache")) {
        let target = rel.target.clone();
        if let Ok(Some(x)) = cx.optional_xml(&target)
            && let Some(d) = read_cache_def(&x)
            && defs.len() < MAX_SLICERS
        {
            defs.push(d);
        }
    }
    // `tabId` is the `sheetId` of the PivotTable's sheet.
    let sheet_of_tab = |tab: u32| -> Option<usize> {
        let i = sheet_els.iter().position(|s| s.attr_u32("sheetId") == Some(tab))?;
        file_to_model.get(i).copied().flatten()
    };
    let mut used = vec![false; found.frames.len()];
    let mut tables = Vec::new();
    for (sheet_name, part) in found.parts {
        let Some(si) = wb.sheet_index(&sheet_name) else { continue };
        // The part's frame is accounted for even when the slicer is skipped below with its own
        // warning.
        let fi = found.frames.iter().position(|(s, n, _, _)| s.eq_ignore_ascii_case(&sheet_name) && n.eq_ignore_ascii_case(&part.name));
        if let Some(u) = fi.and_then(|i| used.get_mut(i)) {
            *u = true;
        }
        let Some(def) = defs.iter().find(|d| d.name.eq_ignore_ascii_case(&part.cache)) else {
            cx.warn(format!("slicer \"{}\" has no slicer cache and was skipped", part.name));
            continue;
        };
        if def.is_table() {
            let Some(&(_, _, anchor, id)) = fi.and_then(|i| found.frames.get(i)) else {
                cx.warn(format!("slicer \"{}\" has no drawing frame and was skipped", part.name));
                continue;
            };
            let table = def.table.unwrap_or(TableCache {
                table_id: None,
                column_id: None,
                items: TableSlicerItems { descending: false, custom_list_sort: true, cross_filter: SlicerCrossFilter::ShowItemsWithDataAtTop },
            });
            if tables.len() < MAX_SLICERS {
                tables.push(TableIn { sheet: si, part, anchor, id, table, source: decode_xstring(&def.source), origin: origin(wb, def) });
            }
            continue;
        }
        let Some(tab) = &def.tabular else {
            cx.warn(format!("slicer \"{}\" filters an external data source, which is not supported; skipped", part.name));
            continue;
        };
        // The model's slicer filters a source column; a cache field that isn't one (a coarser date
        // level Excel keeps in its own field, such as "Years" over "Date") can't be filtered or
        // saved.
        let Some(cache_id) = tab.cache_id.filter(|id| caches.has_slicer_cache(*id)) else {
            cx.warn(format!("slicer \"{}\" has no resolvable PivotTable cache; skipped", part.name));
            continue;
        };
        if let Some(other) = caches.slicer_non_source(cache_id, &def.source) {
            cx.warn(match other {
                crate::pivot::NonSourceField::DateLevel { base } => {
                    format!(
                        "slicer \"{}\" filters \"{}\", a coarser date level of \"{base}\", which is not supported; skipped",
                        part.name, def.source
                    )
                }
                crate::pivot::NonSourceField::Other => {
                    format!("slicer \"{}\" filters \"{}\", which is not a column of its PivotTable's source; skipped", part.name, def.source)
                }
            });
            continue;
        }
        // The connected PivotTables that were read: (sheet, index).
        let targets: Vec<(usize, usize)> = def
            .pivots
            .iter()
            .filter_map(|(t, name)| {
                let psi = sheet_of_tab(*t)?;
                let pi = wb.sheet(psi)?.pivots.iter().position(|p| p.name.eq_ignore_ascii_case(name))?;
                Some((psi, pi))
            })
            .collect();
        if targets.is_empty() || def.source.trim().is_empty() {
            cx.warn(format!("slicer \"{}\": its PivotTable is missing or could not be read; skipped", part.name));
            continue;
        }
        let Some(&(_, _, anchor, id)) = fi.and_then(|i| found.frames.get(i)) else {
            cx.warn(format!("slicer \"{}\" has no drawing frame and was skipped", part.name));
            continue;
        };
        if wb.sheet(si).is_some_and(|sh| sh.slicers.iter().any(|s| s.id == id)) {
            continue;
        }
        // The cache field's items and date grouping. A PivotTable without a filter on the field
        // takes the cache's selection; with everything selected it still keeps the field's
        // grouping (the cache field's, ECMA-376 §18.10.1.33) when no Rows/Columns field has it.
        let field = caches.slicer_labels(cache_id, &def.source);
        let group = field.map(|(_, g)| g).unwrap_or_default();
        let selected: Option<Vec<String>> = field.and_then(|(labels, _)| {
            let sel: Vec<String> = tab.items.iter().filter(|(_, s)| *s).filter_map(|(x, _)| labels.get(*x as usize).cloned()).collect();
            (tab.items.iter().any(|(_, s)| !s) && !sel.is_empty()).then_some(sel)
        });
        for &(psi, pi) in &targets {
            let Some(pt) = wb.sheet_mut(psi).and_then(|sh| sh.pivots.get_mut(pi)) else { continue };
            if pt.filters.iter().any(|f| f.source_col.eq_ignore_ascii_case(&def.source)) {
                continue;
            }
            match &selected {
                Some(sel) => pt.set_filter(&def.source, Some(sel.clone()), group),
                None if group != PivotDateGroup::None && pt.field_group(&def.source) != group => {
                    pt.filters.push(PivotFilter { source_col: def.source.clone(), selected: None, hidden: true, date_group: group });
                    pt.normalize_filter_groups();
                }
                None => {}
            }
        }
        let pivots = targets
            .iter()
            .filter_map(|&(psi, pi)| {
                let sh = wb.sheet(psi)?;
                Some(SlicerPivot { sheet: sh.name.clone(), name: sh.pivots.get(pi)?.name.clone() })
            })
            .collect();
        let slicer = Slicer {
            id,
            source: SlicerSource::Pivot {
                pivots,
                field: def.source.clone(),
                sort: tab.sort,
                cross_filter: tab.cross,
                show_missing: tab.show_missing,
            },
            caption: part.caption,
            anchor,
            columns: part.columns,
            name: part.name,
            style: part.style,
            row_height: part.row_height,
            imported_cache: origin(wb, def),
        };
        if let Some(sh) = wb.sheet_mut(si) {
            sh.slicers.push(slicer);
        }
    }
    for ((_, name, _, _), u) in found.frames.iter().zip(used) {
        if !u {
            cx.warn(format!("slicer \"{name}\" has no slicer part and was skipped"));
        }
    }
    cx.slicers.tables = tables;
}

/// Binds the table slicers [`resolve`] found to their tables, once the tables have their repaired
/// names. A present table id limits recovery to that table; only an absent id permits a unique
/// workbook-wide header match.
pub fn attach_tables(cx: &mut Ctx<'_>, wb: &mut Workbook) {
    for t in std::mem::take(&mut cx.slicers.tables) {
        let tables = || wb.sheets.iter().flat_map(|sh| &sh.tables);
        let resolved = match t.table.table_id {
            Some(id) => cx.file_tables.get(&id).and_then(|(model_id, columns)| {
                let table = tables().find(|x| x.id == *model_id)?;
                let ci = t
                    .table
                    .column_id
                    .and_then(|id| columns.get(&id))
                    .filter(|h| !h.is_empty())
                    .and_then(|h| table.column_index(h))
                    .or_else(|| if t.source.is_empty() { None } else { table.column_index(&t.source) })?;
                Some((table.name.as_str(), table.columns[ci].name.as_str()))
            }),
            None if !t.source.is_empty() => {
                let mut matches = tables().filter_map(|table| {
                    let ci = table.column_index(&t.source)?;
                    Some((table.name.as_str(), table.columns[ci].name.as_str()))
                });
                matches.next().filter(|_| matches.next().is_none())
            }
            None => None,
        };
        let Some((table, column)) = resolved.filter(|(table, column)| !table.is_empty() && !column.is_empty()) else {
            cx.warn(format!("slicer \"{}\": its table column could not be found; skipped", t.part.name));
            continue;
        };
        let selected = saved_table_column_selection(wb, t.sheet, table, column);
        let (table, column) = (table.to_owned(), column.to_owned());
        let id = t.id;
        let caption = if t.part.caption.is_empty() { column.clone() } else { t.part.caption };
        let slicer = Slicer {
            id,
            source: SlicerSource::Table { table, column, selected },
            caption,
            anchor: t.anchor,
            columns: t.part.columns,
            name: t.part.name,
            style: t.part.style,
            row_height: t.part.row_height,
            imported_cache: t.origin,
        };
        if let Some(sh) = wb.sheet_mut(t.sheet)
            && !sh.slicers.iter().any(|s| s.id == id)
        {
            sh.slicers.push(slicer);
        }
    }
}

/// A table slicer's selected values: the table column's saved AutoFilter values.
fn saved_table_column_selection(wb: &Workbook, slicer_sheet: usize, table: &str, column: &str) -> Option<Vec<String>> {
    let (target_sheet, t) = wb
        .sheets
        .get(slicer_sheet)
        .and_then(|sh| sh.tables.iter().find(|t| t.name.eq_ignore_ascii_case(table)).map(|t| (sh.as_ref(), t)))
        .or_else(|| wb.sheets.iter().find_map(|sh| sh.tables.iter().find(|t| t.name.eq_ignore_ascii_case(table)).map(|t| (sh.as_ref(), t))))?;
    let ci = t.column_index(column)? as u32;
    if let Some((_, gridcraft_model::FilterCriterion::Values { values, .. })) = t.filter.iter().find(|(col, _)| *col == ci) {
        return Some(values.clone());
    }
    let af = target_sheet.autofilter.as_ref()?;
    let abs_col = t.range.start.col + ci;
    if abs_col < af.range.start.col || abs_col > af.range.end.col {
        return None;
    }
    let off = abs_col - af.range.start.col;
    af.criteria.iter().find_map(|(col, crit)| match crit {
        gridcraft_model::FilterCriterion::Values { values, .. } if *col == off => Some(values.clone()),
        _ => None,
    })
}

// ---------------------------------------------------------------- writing

/// A slicer to write.
struct Planned {
    sheet: usize,
    /// Index in the sheet's slicers.
    slicer: usize,
    /// Unique slicer name.
    name: String,
    /// Unique slicer cache name (also a workbook defined name).
    cache: String,
    /// The imported marker is written with the other defined names, without losing its metadata.
    has_marker: bool,
    /// The PivotTable field, or the table column.
    field: String,
    /// The connected PivotTables written (sheet, index), which share one pivot cache. Empty for a
    /// table slicer.
    pivots: Vec<(usize, usize)>,
    /// A table slicer's table and column.
    table: Option<TableTarget>,
}

/// The table a table slicer filters.
struct TableTarget {
    /// (sheet, index) of the table.
    at: (usize, usize),
    /// Index of the column in the table.
    column: usize,
    /// The table's `id`, the column's `id` and its written name, once the table is written.
    written: Option<(u32, u32, String)>,
}

/// A slicer on the sheet being written: its index in the sheet's slicers, name and cache name.
pub struct SheetSlicer {
    pub slicer: usize,
    pub name: String,
    pub cache: String,
}

/// The slicers of a workbook being written.
#[derive(Default)]
pub struct SlicerWriter {
    planned: Vec<Planned>,
    /// PivotTables (sheet, index) written with one pivot cache: those the same slicers connect.
    groups: Vec<Vec<(usize, usize)>>,
    parts: u32,
}

impl SlicerWriter {
    /// Plans the slicers that can be written. A PivotTable slicer's PivotTables must share a pivot
    /// cache; where they can't (different sources or date grouping), the slicer keeps only its
    /// first PivotTable. A table slicer needs its table (found by its repaired name) to be written
    /// with the column; otherwise it is dropped with a warning in `warnings`.
    pub fn new(wb: &Workbook, table_names: &[Vec<Option<String>>], warnings: &mut Vec<String>) -> SlicerWriter {
        let mut planned: Vec<Planned> = Vec::new();
        let mut names: Vec<String> = Vec::new();
        for (si, sheet) in wb.sheets.iter().enumerate() {
            for (i, sl) in sheet.slicers.iter().enumerate() {
                let base = [sl.name.trim(), sl.caption.trim(), sl.field().trim()].into_iter().find(|n| !n.is_empty()).unwrap_or("Slicer");
                let (field, pivots, table) = match &sl.source {
                    SlicerSource::Pivot { pivots, field, .. } => {
                        let mut targets: Vec<(usize, usize)> = Vec::new();
                        for p in pivots {
                            let Some(psi) = wb.sheet_index(&p.sheet) else { continue };
                            let Some(sh) = wb.sheet(psi) else { continue };
                            let Some(pi) = sh.pivots.iter().position(|t| t.name.eq_ignore_ascii_case(&p.name)) else { continue };
                            let ok = sh.pivots.get(pi).is_some_and(|pt| crate::pivot::slicer_field_ok(wb, wb, pt, psi, field));
                            if ok && !targets.contains(&(psi, pi)) {
                                targets.push((psi, pi));
                            }
                        }
                        if targets.is_empty() {
                            continue;
                        }
                        (field.clone(), targets, None)
                    }
                    SlicerSource::Table { table, column, .. } => {
                        // The table's sheet first, as the slicer's tiles find it.
                        let at = sheet.tables.iter().position(|t| t.name.eq_ignore_ascii_case(table)).map(|ti| (si, ti)).or_else(|| wb.table(table));
                        let written = at.filter(|&(tsi, ti)| table_names.get(tsi).and_then(|n| n.get(ti)).is_some_and(Option::is_some));
                        let Some((tsi, ti)) = written else {
                            warnings.push(format!("slicer \"{base}\" was dropped because its table \"{table}\" is not in the saved workbook"));
                            continue;
                        };
                        let t = &wb.sheets[tsi].tables[ti];
                        if t.data_range().is_none() {
                            warnings.push(format!("slicer \"{base}\" was dropped because table \"{table}\" has no data rows"));
                            continue;
                        }
                        let Some(ci) = t.column_index(column).filter(|&ci| ci < t.range.width() as usize) else {
                            warnings.push(format!("slicer \"{base}\" was dropped because table \"{table}\" has no column \"{column}\""));
                            continue;
                        };
                        (t.columns[ci].name.clone(), vec![], Some(TableTarget { at: (tsi, ti), column: ci, written: None }))
                    }
                };
                let name = unique(base, " ", &mut names);
                planned.push(Planned { sheet: si, slicer: i, name, cache: String::new(), has_marker: false, field, pivots, table });
            }
        }
        if planned.is_empty() {
            return Self::default();
        }
        // A workbook-scoped marker owns its name unless another workbook-scoped name or a table
        // shares it. Sheet-scoped names are separate definitions: they stop a cache from taking
        // a free name, but not from keeping its own marker.
        let mut reserved = std::collections::HashMap::new();
        for n in wb.names.iter().filter(|n| n.scope.is_none()) {
            let marker = is_cache_marker(n).then(|| n.uid.get());
            reserved.entry(n.name.to_ascii_lowercase()).and_modify(|uid| *uid = None).or_insert(marker);
        }
        for n in wb.names.iter().filter(|n| n.scope.is_some()) {
            reserved.entry(n.name.to_ascii_lowercase()).or_insert(None);
        }
        for name in wb.sheets.iter().flat_map(|s| s.tables.iter().map(|t| &t.name)).chain(table_names.iter().flatten().flatten()) {
            reserved.insert(name.to_ascii_lowercase(), None);
        }
        // Give imported caches first claim to their original names. An occupied name can be
        // reused only by the cache that imported that exact workbook marker.
        for p in &mut planned {
            let SlicerCacheOrigin { name, marker, .. } = &wb.sheets[p.sheet].slicers[p.slicer].imported_cache;
            if name.is_empty() {
                continue;
            }
            let key = name.to_ascii_lowercase();
            if !gridcraft_core::valid_defined_name(name) || crate::read::is_special_defined_name(&key) {
                continue;
            }
            if let Some(uid) = reserved.get(&key) {
                if !uid.is_some_and(|uid| marker.is_some_and(|marker| marker.get() == uid)) {
                    continue;
                }
                p.has_marker = true;
            }
            p.cache.clone_from(name);
            reserved.insert(key, None);
        }
        for p in planned.iter_mut().filter(|p| p.cache.is_empty()) {
            let clean: String = p.field.chars().map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' }).collect();
            let base = format!("Slicer_{clean}");
            let mut name = base.clone();
            let mut suffix = 2;
            while reserved.contains_key(&name.to_ascii_lowercase()) {
                name = format!("{base}{suffix}");
                suffix += 1;
            }
            reserved.insert(name.to_ascii_lowercase(), None);
            p.cache = name;
        }
        SlicerWriter { planned, groups: vec![], parts: 0 }
    }

    /// Remove PivotTable targets that cannot be rebuilt against the catalog as serialized. Return
    /// whether any marker names disappeared, so the caller can reproject the catalog before
    /// grouping.
    pub fn prune_targets(&mut self, wb: &Workbook, catalog: &Workbook) -> bool {
        let count = self.planned.len();
        for p in &mut self.planned {
            p.pivots.retain(|&(si, pi)| crate::pivot::slicer_field_ok(wb, catalog, &wb.sheets[si].pivots[pi], si, &p.field));
        }
        self.planned.retain(|p| p.table.is_some() || !p.pivots.is_empty());
        self.planned.len() != count
    }

    pub fn finish_plan(&mut self, wb: &Workbook, catalog: &Workbook) {
        let planned = &mut self.planned;
        let mut groups = merge_groups(planned);
        let mut split = false;
        for g in groups.iter().filter(|g| g.len() > 1) {
            if !crate::pivot::can_share(wb, catalog, g, &group_fields(planned, g)) {
                for p in planned.iter_mut().filter(|p| p.pivots.iter().any(|x| g.contains(x))) {
                    p.pivots.truncate(1);
                }
                split = true;
            }
        }
        if split {
            groups = merge_groups(planned);
        }
        self.groups = groups;
    }

    /// The PivotTables written with the same pivot cache as PivotTable `pi` on sheet `si`
    /// (itself included), when slicers connect it.
    pub fn cache_group(&self, si: usize, pi: usize) -> Option<Vec<(usize, usize)>> {
        self.groups.iter().find(|g| g.contains(&(si, pi))).cloned()
    }

    /// Fields that slicers filter in the pivot cache of PivotTable `pi` on sheet `si`.
    pub fn fields_for(&self, si: usize, pi: usize) -> Vec<String> {
        self.groups.iter().find(|g| g.contains(&(si, pi))).map(|g| group_fields(&self.planned, g)).unwrap_or_default()
    }

    /// The slicers to write on sheet `si`.
    pub fn on_sheet(&self, si: usize) -> Vec<SheetSlicer> {
        self.planned
            .iter()
            .filter(|p| p.sheet == si)
            .map(|p| SheetSlicer { slicer: p.slicer, name: p.name.clone(), cache: p.cache.clone() })
            .collect()
    }

    /// Records the ids table `at` (sheet, index) is written with: the table's `table_id`, and its
    /// columns' written `names` and `column_id`s, which the table slicer caches name.
    pub fn table_written(&mut self, at: (usize, usize), table_id: u32, names: &[String], column_id: impl Fn(usize) -> u32) {
        for t in self.planned.iter_mut().filter_map(|p| p.table.as_mut()).filter(|t| t.at == at) {
            if let Some(name) = names.get(t.column) {
                t.written = Some((table_id, column_id(t.column), name.clone()));
            }
        }
    }

    /// Names whose markers are not already written by the workbook's defined names.
    pub fn new_marker_names(&self) -> impl Iterator<Item = &str> {
        self.planned.iter().filter(|p| !p.has_marker).map(|p| p.cache.as_str())
    }

    /// `<definedName>` elements Excel keeps for slicer caches.
    pub fn defined_names(&self) -> String {
        self.new_marker_names().map(|name| format!("<definedName name=\"{}\">#N/A</definedName>", esc_attr(name))).collect()
    }

    /// Writes a sheet's slicer parts and relationships: one for its PivotTable slicers and one for
    /// its table slicers, as Excel does. Returns the sheet's `extLst` entries listing them (empty
    /// when the sheet has none).
    pub fn sheet_part(sheet: &Sheet, list: &[SheetSlicer], out: &mut Out, rels: &mut Rels) -> String {
        let is_table = |p: &&SheetSlicer| sheet.slicers.get(p.slicer).is_some_and(|sl| matches!(sl.source, SlicerSource::Table { .. }));
        let (tables, pivots): (Vec<&SheetSlicer>, Vec<&SheetSlicer>) = list.iter().partition(is_table);
        let mut ext = String::new();
        if let Some(rid) = Self::slicer_part(sheet, &pivots, out, rels) {
            let _ = write!(
                ext,
                "<ext uri=\"{EXT_SHEET_SLICERS}\" xmlns:x14=\"{NS_X14}\"><x14:slicerList><x14:slicer r:id=\"{rid}\"/></x14:slicerList></ext>"
            );
        }
        if let Some(rid) = Self::slicer_part(sheet, &tables, out, rels) {
            let _ = write!(
                ext,
                "<ext uri=\"{EXT_SHEET_TABLE_SLICERS}\" xmlns:x15=\"{NS_X15}\"><x14:slicerList xmlns:x14=\"{NS_X14}\"><x14:slicer r:id=\"{rid}\"/></x14:slicerList></ext>"
            );
        }
        ext
    }

    /// Writes one slicer part holding `list`; returns its relationship id (`None` when empty).
    fn slicer_part(sheet: &Sheet, list: &[&SheetSlicer], out: &mut Out, rels: &mut Rels) -> Option<String> {
        let mut x = format!("{XML_DECL}<slicers xmlns=\"{NS_X14}\" xmlns:mc=\"{NS_MC}\" mc:Ignorable=\"x\" xmlns:x=\"{}\">", crate::write::NS_MAIN);
        let mut any = false;
        for p in list {
            let Some(sl) = sheet.slicers.get(p.slicer) else { continue };
            any = true;
            let _ = write!(x, "<slicer name=\"{}\" cache=\"{}\"", esc_attr(&p.name), esc_attr(&p.cache));
            if !sl.caption.is_empty() {
                let _ = write!(x, " caption=\"{}\"", esc_attr(&encode_xstring(&sl.caption)));
            }
            if sl.columns > 1 {
                let _ = write!(x, " columnCount=\"{}\"", sl.columns.min(20_000));
            }
            if let Some(st) = builtin_style(&sl.style) {
                let _ = write!(x, " style=\"{st}\"");
            }
            let h = sl.row_height.filter(|h| h.is_finite() && *h > 0.0).map(|h| (h as f64 * EMU_PER_PX).round().min(u32::MAX as f64) as u32);
            let _ = write!(x, " rowHeight=\"{}\"/>", h.unwrap_or(DEFAULT_ROW_HEIGHT));
        }
        if !any {
            return None;
        }
        x.push_str("</slicers>");
        out.slicers.parts += 1;
        let n = out.slicers.parts;
        out.part(&format!("xl/slicers/slicer{n}.xml"), Some(CT_SLICER), x.into_bytes());
        Some(rels.add_uri(REL_SLICER, &format!("../slicers/slicer{n}.xml")))
    }

    /// Writes the slicer caches; returns the workbook `extLst`'s entries listing them (empty when
    /// there are none). `sheet_ids` is the `sheetId` written for each model sheet, which a cache's
    /// `tabId` names.
    pub fn finish(self, wb: &Workbook, sheet_ids: &[usize], pivots: &crate::pivot::PivotWriter, out: &mut Out, wb_rels: &mut Rels) -> String {
        let mut list = String::new();
        let mut table_list = String::new();
        for (k, p) in self.planned.iter().enumerate() {
            let Some(sl) = wb.sheet(p.sheet).and_then(|s| s.slicers.get(p.slicer)) else { continue };
            let x = match (&p.table, &sl.source) {
                (Some(t), _) => {
                    // Its table's sheet records the ids; a table that wasn't written has no cache.
                    let Some((table_id, column_id, source)) = &t.written else { continue };
                    table_cache_xml(&p.cache, source, *table_id, *column_id, sl.imported_cache.table_items)
                }
                (None, SlicerSource::Pivot { sort, cross_filter, show_missing, .. }) => {
                    let Some(x) = pivot_cache_xml(wb, p, sheet_ids, pivots, (*sort, *cross_filter, *show_missing)) else { continue };
                    x
                }
                (None, SlicerSource::Table { .. }) => continue,
            };
            let part = format!("slicerCaches/slicerCache{}.xml", k + 1);
            out.part(&format!("xl/{part}"), Some(CT_SLICER_CACHE), x.into_bytes());
            let rid = wb_rels.add_uri(REL_SLICER_CACHE, &part);
            let _ = write!(if p.table.is_some() { &mut table_list } else { &mut list }, "<x14:slicerCache r:id=\"{rid}\"/>");
        }
        let mut ext = String::new();
        if !list.is_empty() {
            let _ = write!(ext, "<ext uri=\"{EXT_WB_SLICER_CACHES}\" xmlns:x14=\"{NS_X14}\"><x14:slicerCaches>{list}</x14:slicerCaches></ext>");
        }
        if !table_list.is_empty() {
            let _ = write!(
                ext,
                "<ext uri=\"{EXT_WB_TABLE_SLICER_CACHES}\" xmlns:x15=\"{NS_X15}\"><x15:slicerCaches xmlns:x14=\"{NS_X14}\">{table_list}</x15:slicerCaches></ext>"
            );
        }
        ext
    }
}

/// A table slicer cache: the cache's name, the column's written header (`sourceName`) and the
/// table and column ids it was written with, with the item options kept from the file.
fn table_cache_xml(cache: &str, source: &str, table_id: u32, column_id: u32, items: Option<TableSlicerItems>) -> String {
    let mut x = format!(
        "{XML_DECL}<slicerCacheDefinition xmlns=\"{NS_X14}\" xmlns:mc=\"{NS_MC}\" mc:Ignorable=\"x\" xmlns:x=\"{}\" name=\"{}\" sourceName=\"{}\"><extLst><x:ext uri=\"{EXT_TABLE_SLICER_CACHE}\" xmlns:x15=\"{NS_X15}\"><x15:tableSlicerCache tableId=\"{table_id}\" column=\"{column_id}\"",
        crate::write::NS_MAIN,
        esc_attr(cache),
        esc_attr(&encode_xstring(source)),
    );
    if let Some(items) = items {
        if items.descending {
            x.push_str(" sortOrder=\"descending\"");
        }
        if !items.custom_list_sort {
            x.push_str(" customListSort=\"0\"");
        }
        match items.cross_filter {
            SlicerCrossFilter::None => x.push_str(" crossFilter=\"none\""),
            SlicerCrossFilter::ShowItemsWithNoData => x.push_str(" crossFilter=\"showItemsWithNoData\""),
            SlicerCrossFilter::ShowItemsWithDataAtTop => {}
        }
    }
    x.push_str("/></x:ext></extLst></slicerCacheDefinition>");
    x
}

/// A PivotTable slicer cache, or `None` when none of its PivotTables was written.
fn pivot_cache_xml(
    wb: &Workbook,
    p: &Planned,
    sheet_ids: &[usize],
    pivots: &crate::pivot::PivotWriter,
    (sort, cross_filter, show_missing): (PivotSort, SlicerCrossFilter, bool),
) -> Option<String> {
    // The connected PivotTables that were written, with their (shared) cache's data.
    let written: Vec<((usize, usize), &PivotTable, &crate::pivot::PivotSlicerInfo)> =
        p.pivots.iter().filter_map(|&(psi, pi)| Some(((psi, pi), wb.sheet(psi)?.pivots.get(pi)?, pivots.slicer_info(psi, pi)?))).collect();
    let &(_, pt, info) = written.first()?;
    let field = info.fields.iter().find(|f| f.name.eq_ignore_ascii_case(&p.field));
    let labels: &[String] = field.map(|f| f.labels.as_slice()).unwrap_or(&[]);
    // The selection at the base cache field's level (the items written below).
    let level = pt.field_group(&p.field);
    let sel =
        pt.filters.iter().find(|f| f.source_col.eq_ignore_ascii_case(&p.field) && pt.filter_group(f) == level).and_then(|f| f.selected.as_deref());
    let mut on: Vec<bool> = labels.iter().map(|l| sel.is_none_or(|s| s.iter().any(|x| x.eq_ignore_ascii_case(l)))).collect();
    if !on.contains(&true) {
        on.iter_mut().for_each(|o| *o = true);
    }
    // Date groups are in date order already; other items sort by value.
    let mut order: Vec<usize> = (0..labels.len()).collect();
    match (sort, field.is_some_and(|f| f.grouped)) {
        (PivotSort::Asc, false) => order.sort_by(|a, b| cmp_label(&labels[*a], &labels[*b])),
        (PivotSort::Desc, false) => order.sort_by(|a, b| cmp_label(&labels[*b], &labels[*a])),
        (PivotSort::Desc, true) => order.reverse(),
        (PivotSort::Asc | PivotSort::None, true) | (PivotSort::None, false) => {}
    }
    let mut x = format!(
        "{XML_DECL}<slicerCacheDefinition xmlns=\"{NS_X14}\" xmlns:mc=\"{NS_MC}\" mc:Ignorable=\"x\" xmlns:x=\"{}\" name=\"{}\" sourceName=\"{}\"><pivotTables>",
        crate::write::NS_MAIN,
        esc_attr(&p.cache),
        esc_attr(&p.field),
    );
    for &((psi, pi), t, i) in &written {
        if i.cache_id != info.cache_id {
            continue;
        }
        let Some(tab) = sheet_ids.get(psi) else { continue };
        let pname = if t.name.trim().is_empty() { format!("PivotTable{}", pi + 1) } else { t.name.clone() };
        let _ = write!(x, "<pivotTable tabId=\"{tab}\" name=\"{}\"/>", esc_attr(&pname));
    }
    let _ = write!(x, "</pivotTables><data><tabular pivotCacheId=\"{}\"", info.cache_id);
    if sort == PivotSort::Desc {
        x.push_str(" sortOrder=\"descending\"");
    }
    if !show_missing {
        x.push_str(" showMissing=\"0\"");
    }
    match cross_filter {
        SlicerCrossFilter::None => x.push_str(" crossFilter=\"none\""),
        SlicerCrossFilter::ShowItemsWithNoData => x.push_str(" crossFilter=\"showItemsWithNoData\""),
        SlicerCrossFilter::ShowItemsWithDataAtTop => {}
    }
    let _ = write!(x, "><items count=\"{}\">", order.len());
    for i in order {
        let _ = write!(x, "<i x=\"{i}\"{}/>", if on[i] { " s=\"1\"" } else { "" });
    }
    x.push_str("</items></tabular></data></slicerCacheDefinition>");
    Some(x)
}

/// The PivotTables (sheet, index) that must share a pivot cache: each PivotTable slicer's, merged
/// where slicers have a PivotTable in common.
fn merge_groups(planned: &[Planned]) -> Vec<Vec<(usize, usize)>> {
    let mut groups: Vec<Vec<(usize, usize)>> = Vec::new();
    for p in planned.iter().filter(|p| !p.pivots.is_empty()) {
        let mut g = p.pivots.clone();
        let (overlap, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut groups).into_iter().partition(|o| o.iter().any(|x| g.contains(x)));
        groups = rest;
        for x in overlap.into_iter().flatten() {
            if !g.contains(&x) {
                g.push(x);
            }
        }
        groups.push(g);
    }
    groups
}

/// The fields that the slicers on the PivotTables `group` filter.
fn group_fields(planned: &[Planned], group: &[(usize, usize)]) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for p in planned.iter().filter(|p| p.pivots.iter().any(|x| group.contains(x))) {
        if !v.iter().any(|f| f.eq_ignore_ascii_case(&p.field)) {
            v.push(p.field.clone());
        }
    }
    v
}

/// `base`, or `base{sep}2`, `base{sep}3`… if taken (case-insensitively); records the result.
fn unique(base: &str, sep: &str, taken: &mut Vec<String>) -> String {
    let mut n = base.to_string();
    let mut i = 2;
    while taken.iter().any(|t| t.eq_ignore_ascii_case(&n)) {
        n = format!("{base}{sep}{i}");
        i += 1;
    }
    taken.push(n.clone());
    n
}

/// Whether `n` has the form Excel gives a slicer cache's defined name: workbook scope, `#N/A`.
fn is_cache_marker(n: &gridcraft_model::DefinedName) -> bool {
    n.scope.is_none() && n.formula.trim().trim_start_matches('=').trim().eq_ignore_ascii_case("#N/A")
}

/// Ascending item order: numbers (dates are serials) by value, before text by case-insensitive
/// text.
fn cmp_label(a: &str, b: &str) -> std::cmp::Ordering {
    match (a.parse::<f64>().ok().filter(|n| n.is_finite()), b.parse::<f64>().ok().filter(|n| n.is_finite())) {
        (Some(x), Some(y)) => x.total_cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.to_lowercase().cmp(&b.to_lowercase()),
    }
}

/// A built-in slicer style name to write: the style itself, or the built-in a custom copy is named
/// after (`SlicerStyleDark1 2`). Custom style definitions aren't written, so other names are
/// dropped and Excel uses its default.
fn builtin_style(style: &str) -> Option<&str> {
    let base = style.split_whitespace().next()?;
    let ok =
        ["SlicerStyleLight", "SlicerStyleDark"].iter().any(|p| base.strip_prefix(p).is_some_and(|n| matches!(n, "1" | "2" | "3" | "4" | "5" | "6")))
            || matches!(base, "SlicerStyleOther1" | "SlicerStyleOther2");
    ok.then_some(base)
}

/// The drawing XML of a slicer frame, with a fallback shape for readers without slicers. A table
/// slicer's choice requires Excel 2013's `sle15` namespace, a PivotTable slicer's Excel 2010's `a14`.
pub fn frame_xml(id: usize, name: &str, drawing_name: Option<&str>, table: bool, xfrm: &str, a_xfrm: &str) -> String {
    let (choice, message) = if table {
        (format!("xmlns:sle15=\"{NS_SLE15}\" Requires=\"sle15\""), "This is a table slicer. Table slicers need Excel 2013 or later.")
    } else {
        (
            "xmlns:a14=\"http://schemas.microsoft.com/office/drawing/2010/main\" Requires=\"a14\"".to_string(),
            "This is a PivotTable slicer. Slicers need Excel 2010 or later.",
        )
    };
    let n = esc_attr(name);
    let nv = drawing_name.map(esc_attr);
    let nv = nv.as_deref().unwrap_or(&n);
    format!(
        "<mc:AlternateContent xmlns:mc=\"{NS_MC}\"><mc:Choice {choice}><xdr:graphicFrame macro=\"\"><xdr:nvGraphicFramePr><xdr:cNvPr id=\"{id}\" name=\"{nv}\"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr>{xfrm}<a:graphic><a:graphicData uri=\"{FRAME_URI}\"><sle:slicer xmlns:sle=\"{FRAME_URI}\" name=\"{n}\"/></a:graphicData></a:graphic></xdr:graphicFrame></mc:Choice><mc:Fallback><xdr:sp macro=\"\" textlink=\"\"><xdr:nvSpPr><xdr:cNvPr id=\"0\" name=\"\"/><xdr:cNvSpPr><a:spLocks noTextEdit=\"1\"/></xdr:cNvSpPr></xdr:nvSpPr><xdr:spPr>{a_xfrm}<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></xdr:spPr><xdr:txBody><a:bodyPr vertOverflow=\"clip\" horzOverflow=\"clip\"/><a:lstStyle/><a:p><a:r><a:rPr lang=\"en-US\" sz=\"1100\"/><a:t>{message}</a:t></a:r></a:p></xdr:txBody></xdr:sp></mc:Fallback></mc:AlternateContent>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use gridcraft_core::{CellRef, RangeRef, Value};
    use gridcraft_model::Table;

    #[test]
    fn pruning_keeps_a_later_serializable_target_and_removes_empty_markers() {
        let mut wb = Workbook::new();
        let mut data = Sheet::new("Data");
        for row in [0, 3] {
            data.set_value(CellRef::new(row, 0), Value::text("K"));
            data.set_value(CellRef::new(row + 1, 0), Value::text("value"));
        }
        for (id, name, source) in [(1, "First", "Data!$A$1:$A$2"), (2, "Second", "Data!$A$4:$A$5")] {
            data.pivots.push(PivotTable { id, name: name.into(), source: source.into(), ..Default::default() });
        }
        for (id, name, targets) in [(3, "Both", &["First", "Second"][..]), (4, "OnlyFirst", &["First"][..])] {
            data.slicers.push(Slicer {
                id,
                name: name.into(),
                caption: name.into(),
                anchor: Anchor { cell: CellRef::new(0, 9), dx: 0.0, dy: 0.0, width: 180.0, height: 240.0, mode: Default::default() },
                columns: 1,
                style: String::new(),
                row_height: None,
                imported_cache: Default::default(),
                source: SlicerSource::Pivot {
                    pivots: targets.iter().map(|n| SlicerPivot { sheet: "Data".into(), name: (*n).into() }).collect(),
                    field: "K".into(),
                    sort: PivotSort::Asc,
                    cross_filter: SlicerCrossFilter::ShowItemsWithDataAtTop,
                    show_missing: true,
                },
            });
        }
        wb.sheets[0] = std::sync::Arc::new(data);
        let mut writer = SlicerWriter::new(&wb, &[vec![]], &mut vec![]);
        assert_eq!(writer.new_marker_names().collect::<Vec<_>>(), ["Slicer_K", "Slicer_K2"]);
        // Exercise the writer's catalog boundary: the first target's explicit range would import
        // as a table binding rather than the range used by native refresh in the model.
        let mut catalog = wb.clone();
        catalog.sheet_mut(0).unwrap().tables.push(Table {
            name: "Data!$A$1:$A$2".into(),
            range: RangeRef::parse("A4:A5").unwrap(),
            id: 1,
            uid: Default::default(),
            header_row: true,
            totals_row: false,
            columns: vec![],
            style: String::new(),
            banded_rows: false,
            banded_cols: false,
            first_col: false,
            last_col: false,
            filter_button: false,
            filter: vec![],
            table_type: None,
            connection_id: None,
        });
        assert!(writer.prune_targets(&wb, &catalog));
        writer.finish_plan(&wb, &catalog);
        assert_eq!(writer.new_marker_names().collect::<Vec<_>>(), ["Slicer_K"]);
        assert_eq!(writer.planned[0].pivots, [(0, 1)]);

        let marker = gridcraft_model::DefinedName {
            name: "Slicer_K2".into(),
            scope: None,
            formula: "#N/A".into(),
            hidden: true,
            comment: "Imported marker".into(),
            uid: Default::default(),
        };
        wb.sheet_mut(0).unwrap().slicers[1].imported_cache =
            gridcraft_model::SlicerCacheOrigin { name: marker.name.clone(), marker: Some(marker.uid), table_items: None };
        wb.names.push(marker);
        let mut writer = SlicerWriter::new(&wb, &[vec![]], &mut vec![]);
        assert_eq!(writer.planned[1].cache, "Slicer_K2");
        assert!(writer.planned[1].has_marker);
        assert!(writer.prune_targets(&wb, &catalog));
        assert_eq!(writer.new_marker_names().collect::<Vec<_>>(), ["Slicer_K"]);
    }
}
