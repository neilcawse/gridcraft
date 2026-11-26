use gridcraft_core::{CellError, Value};
use gridcraft_model::{PivotDataCell, PivotItemRef, pivot_key};
use std::sync::Arc;

use super::{
    CUSTOM_SUBTOTALS, CellRef, Ctx, DateSystem, EXT_X14_FIELD, EXT_X14_TABLE, EXT_XPDL_TABLE, El, HashMap, IoError, MAX_FIELDS, PivotCalcField,
    PivotColRole, PivotCustomStyle, PivotDateGroup, PivotField, PivotFilter, PivotFunc, PivotLayout, PivotRowRole, PivotScopeItem, PivotShowAs,
    PivotSort, PivotStyleElement, PivotStyleGrid, PivotStyleLayout, PivotStyleOptions, PivotTable, PivotValue, RangeRef, VALUES_FIELD, abs_range,
    func_caption, func_from, number_to_text, quote_sheet, same, serial_from_iso_ymd,
};

/// Cap on custom `<tableStyle>` definitions and elements read from `styles.xml`.
const MAX_TABLE_STYLES: usize = 512;
const MAX_TABLE_STYLE_ELEMENTS: usize = 64;

#[derive(Clone, Debug, Default)]
struct FieldGroupIn {
    base: Option<usize>,
    by: Option<PivotDateGroup>,
    items: Vec<String>,
    values: Vec<Value>,
}

/// A cache field as read.
#[derive(Clone, Debug, Default)]
struct FieldIn {
    name: String,
    database: bool,
    /// Shared item captions.
    items: Vec<String>,
    /// Shared item typed values, parallel to `items`.
    item_values: Vec<Value>,
    /// Field grouping (`by == None` means unsupported grouping).
    group: Option<FieldGroupIn>,
    /// A calculated field's formula.
    formula: Option<String>,
}

impl FieldIn {
    fn labels(&self) -> &[String] {
        match &self.group {
            Some(g) if !g.items.is_empty() => &g.items,
            _ => &self.items,
        }
    }

    fn values(&self) -> &[Value] {
        match &self.group {
            Some(g) if !g.items.is_empty() => &g.values,
            _ => &self.item_values,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct CacheIn {
    id: u32,
    source: String,
    external_source: bool,
    fields: Vec<FieldIn>,
    /// `x14:pivotCacheDefinition pivotCacheId`: how slicer caches name the cache.
    slicer_id: Option<u32>,
}

/// Source parts of a PivotTable read from an XLSX sheet, used to preserve raw pivot XML
/// when the source is external or cannot be resolved locally.
#[derive(Clone, Debug)]
pub struct ReadPivotSource {
    pub pivot_id: u32,
    pub table_part: String,
    pub cache_part: String,
    pub external_source: bool,
}

/// Workbook-level pivot cache lookup, filled from `<pivotCaches>`; caches are parsed once.
#[derive(Default)]
pub struct CachesIn {
    by_id: HashMap<u32, String>,
    parsed: HashMap<String, Option<CacheIn>>,
    next_id: u32,
    next_cache_id: u32,
    styles_part: Option<String>,
    table_styles: Option<HashMap<String, Arc<Vec<PivotStyleElement>>>>,
}

impl CachesIn {
    pub fn new(wb_xml: &El, wb_rels: &[crate::package::Rel]) -> CachesIn {
        let mut by_id = HashMap::new();
        if let Some(pc) = wb_xml.child("pivotCaches") {
            for c in pc.kids("pivotCache") {
                if let (Some(id), Some(rid)) = (c.attr_u32("cacheId"), c.attr("id"))
                    && let Some(r) = wb_rels.iter().find(|r| r.id == rid)
                {
                    by_id.insert(id, r.target.clone());
                }
            }
        }
        let styles_part = wb_rels.iter().find(|r| r.kind == "styles" && !r.external).map(|r| r.target.clone());
        CachesIn { by_id, parsed: HashMap::new(), next_id: 1, next_cache_id: 1, styles_part, table_styles: None }
    }

    /// Lazily reads custom `<tableStyle>` definitions from `styles.xml` using `cx.dxfs`.
    fn custom_style(&mut self, cx: &mut Ctx<'_>, name: &str) -> Option<Arc<Vec<PivotStyleElement>>> {
        if name.is_empty() {
            return None;
        }
        if self.table_styles.is_none() {
            let mut map = HashMap::new();
            if let Some(part) = self.styles_part.as_deref()
                && let Ok(Some(root)) = cx.pkg.read_xml(part)
            {
                read_custom_table_styles(&root, &cx.dxfs, &cx.palette, &mut map);
            }
            self.table_styles = Some(map);
        }
        self.table_styles.as_ref()?.get(&name.to_ascii_lowercase()).cloned()
    }

    /// Whether a slicer cache id resolves to a parsed PivotTable cache.
    pub fn has_slicer_cache(&self, cache_id: u32) -> bool {
        self.parsed.values().flatten().any(|c| c.slicer_id == Some(cache_id))
    }

    /// Item captions of field `field` in the cache a slicer cache names by `pivotCacheId`, in
    /// shared item order (what the slicer cache's `<i x>` indices refer to), and the field's date
    /// grouping. `None` for a grouping GridCraft can't compute.
    pub fn slicer_labels(&self, cache_id: u32, field: &str) -> Option<(&[String], PivotDateGroup)> {
        let cache = self.parsed.values().flatten().find(|c| c.slicer_id == Some(cache_id))?;
        let f = cache.fields.iter().find(|f| f.database && same(&f.name, field))?;
        let group = match &f.group {
            None => PivotDateGroup::None,
            Some(g) => g.by?,
        };
        Some((f.labels(), group))
    }

    /// What `field` names in the cache a slicer cache names by `pivotCacheId`, when it is not a
    /// source column. `None` when it is one, or the cache isn't known.
    pub fn slicer_non_source(&self, cache_id: u32, field: &str) -> Option<NonSourceField<'_>> {
        let cache = self.parsed.values().flatten().find(|c| c.slicer_id == Some(cache_id))?;
        if cache.fields.iter().any(|f| f.database && same(&f.name, field)) {
            return None;
        }
        let base = cache
            .fields
            .iter()
            .find(|f| same(&f.name, field))
            .and_then(|f| f.group.as_ref())
            .and_then(|g| g.by.and(g.base.and_then(|b| cache.fields.get(b))))
            .filter(|b| b.database);
        Some(match base {
            Some(b) => NonSourceField::DateLevel { base: &b.name },
            None => NonSourceField::Other,
        })
    }
}

/// A slicer cache's source field that is not a source column of its pivot cache.
pub enum NonSourceField<'a> {
    /// A coarser date level, which Excel keeps in a field of its own when a date is grouped at
    /// several levels ("Years" over "Date" grouped by years and months), grouping column `base`.
    DateLevel { base: &'a str },
    /// Another field (calculated, grouped in a way GridCraft can't compute) or none of that name.
    Other,
}

fn item_label_val_el(e: &El, sys: DateSystem) -> (String, Value) {
    let v = e.attr("v").unwrap_or("");
    match e.name.as_str() {
        "m" => ("(blank)".into(), Value::Empty),
        "n" => match v.trim().parse::<f64>().ok().filter(|n| n.is_finite()) {
            Some(n) => (number_to_text(n), Value::Number(n)),
            None => (v.to_string(), Value::text(v)),
        },
        "b" => {
            let b = matches!(v.trim(), "1" | "true");
            ((if b { "TRUE" } else { "FALSE" }).into(), Value::Bool(b))
        }
        // Cache dates count real days from the epoch, as Excel writes them (see `iso`).
        "d" => match crate::sheet_read::parse_iso_datetime_with(v, |y, m, d| serial_from_iso_ymd(sys, y, m, d)) {
            Some(s) => (number_to_text(s), Value::Number(s)),
            None => (v.to_string(), Value::text(v)),
        },
        "e" => {
            let val = CellError::parse(v).map(Value::Error).unwrap_or_else(|| Value::text(v));
            (v.to_string(), val)
        }
        _ => (v.to_string(), Value::text(v)),
    }
}

fn read_cache(cx: &mut Ctx<'_>, part: &str, sheet_name: &str) -> Option<CacheIn> {
    // A cache that can't be read loses its PivotTable: say so instead of dropping it silently.
    // An oversized cache is refused before it inflates, so warning and opening the rest is safe.
    let x = match cx.optional_xml(part) {
        Ok(Some(x)) => x,
        Ok(None) => {
            cx.warn(format!("a PivotTable's cache {part} is missing or could not be read; kept as plain cells"));
            return None;
        }
        Err(IoError::TooLarge(m)) => {
            cx.warn(format!("a PivotTable's cache is too large to read ({m}); kept as plain cells"));
            return None;
        }
        Err(e) => {
            cx.warn(format!("a PivotTable's cache could not be read ({e}); kept as plain cells"));
            return None;
        }
    };
    if x.name != "pivotCacheDefinition" {
        cx.warn(format!("{part} is not a pivot cache definition; its PivotTable was kept as plain cells"));
        return None;
    }
    let src = x.child("cacheSource");
    let ty = src.and_then(|s| s.attr("type")).unwrap_or("worksheet");
    if ty != "worksheet" {
        cx.warn(format!("PivotTables with a {ty} data source are not supported; kept as plain cells"));
        return None;
    }
    let ws = src.and_then(|s| s.child("worksheetSource"));
    let external_source = ws.is_some_and(|w| w.attr("id").is_some());
    if external_source {
        cx.warn("a PivotTable's source is in another workbook; its source reference may not resolve");
    }
    let source = match ws {
        Some(w) if w.attr("name").is_some_and(|n| !n.trim().is_empty()) => w.attr("name").unwrap_or("").trim().to_string(),
        Some(w) => {
            let r = w.attr("ref").map(|r| r.replace('$', "")).and_then(|r| RangeRef::parse(&r));
            let Some(r) = r else {
                cx.warn("a PivotTable's source reference is missing or invalid; kept as plain cells");
                return None;
            };
            let sheet = w.attr("sheet").filter(|s| !s.is_empty()).unwrap_or(sheet_name);
            format!("{}!{}", quote_sheet(sheet), abs_range(&r))
        }
        None => {
            cx.warn("a PivotTable's cache has no worksheet source; kept as plain cells");
            return None;
        }
    };
    let sys = cx.date_system;
    let mut fields = Vec::new();
    let mut truncated = false;
    if let Some(cf) = x.child("cacheFields") {
        for f in capped(cf.kids("cacheField"), &mut truncated) {
            let (items, item_values): (Vec<String>, Vec<Value>) =
                f.child("sharedItems").map(|si| si.children.iter().map(|e| item_label_val_el(e, sys)).unzip()).unwrap_or_default();
            let group = f.child("fieldGroup").map(|g| {
                let base = g.attr_u32("base").map(|b| b as usize);
                let by = match g.child("rangePr").map(|r| r.attr("groupBy").unwrap_or("range")) {
                    Some("years") => Some(PivotDateGroup::Years),
                    Some("quarters") => Some(PivotDateGroup::Quarters),
                    Some("months") => Some(PivotDateGroup::Months),
                    Some("days") => Some(PivotDateGroup::Days),
                    _ => None,
                };
                let (items, values): (Vec<String>, Vec<Value>) =
                    g.child("groupItems").map(|gi| gi.children.iter().map(|e| item_label_val_el(e, sys)).unzip()).unwrap_or_default();
                FieldGroupIn { base, by, items, values }
            });
            fields.push(FieldIn {
                name: f.attr("name").unwrap_or("").to_string(),
                database: f.flag("databaseField", true),
                items,
                item_values,
                group,
                formula: f.attr("formula").map(str::to_string),
            });
        }
    }
    if truncated {
        cx.warn(format!("{part}: cache fields past {MAX_FIELDS} were dropped; its PivotTables may be incomplete"));
    }
    if fields.is_empty() {
        cx.warn("a PivotTable's cache has no fields; kept as plain cells");
        return None;
    }
    let slicer_id =
        x.child("extLst").and_then(|l| l.kids("ext").find_map(|e| e.child("pivotCacheDefinition"))).and_then(|d| d.attr_u32("pivotCacheId"));
    Some(CacheIn { id: 0, source, external_source, fields, slicer_id })
}

/// Reads the PivotTables related to a worksheet part.
pub fn read_pivots(
    cx: &mut Ctx<'_>,
    caches: &mut CachesIn,
    sheet_part: &str,
    sheet_name: &str,
    sources: &mut Vec<ReadPivotSource>,
) -> Result<Vec<PivotTable>, IoError> {
    let rels = cx.pkg.rels(sheet_part)?;
    let mut out = Vec::new();
    for rel in rels.iter().filter(|r| r.kind == "pivotTable" && !r.external) {
        let Some(x) = cx.optional_xml(&rel.target)? else { continue };
        if x.name != "pivotTableDefinition" {
            cx.warn(format!("{} is not a PivotTable definition; skipped", rel.target));
            continue;
        }
        // The cache: the table's own relationship, else the workbook's cache id.
        let trels = cx.pkg.rels(&rel.target)?;
        let cache_part = trels
            .iter()
            .find(|r| r.kind == "pivotCacheDefinition" && !r.external)
            .map(|r| r.target.clone())
            .or_else(|| x.attr_u32("cacheId").and_then(|id| caches.by_id.get(&id).cloned()));
        let Some(cache_part) = cache_part else {
            cx.warn(format!("PivotTable in {} has no cache; kept as plain cells", rel.target));
            continue;
        };
        let cache = match caches.parsed.get(&cache_part) {
            Some(c) => c.clone(),
            None => {
                let c = read_cache(cx, &cache_part, sheet_name).map(|mut c| {
                    let cid = caches.next_cache_id;
                    caches.next_cache_id = caches.next_cache_id.saturating_add(1);
                    c.id = cid;
                    c
                });
                caches.parsed.insert(cache_part.clone(), c.clone());
                c
            }
        };
        let Some(cache) = cache else { continue };
        if let Some(mut pt) = read_table(cx, &x, &cache, caches.next_id) {
            if !pt.style.is_empty() {
                pt.custom_style = PivotCustomStyle(caches.custom_style(cx, &pt.style));
            }
            sources.push(ReadPivotSource {
                pivot_id: pt.id,
                table_part: rel.target.clone(),
                cache_part: cache_part.clone(),
                external_source: cache.external_source,
            });
            caches.next_id = caches.next_id.saturating_add(1);
            out.push(pt);
        }
    }
    Ok(out)
}

/// A pivot field's data items (not subtotal or grand-total items).
fn items_of(e: &El) -> Vec<&El> {
    e.child("items").map(|it| it.kids("item").filter(|i| i.attr("t").is_none_or(|t| t == "data")).collect()).unwrap_or_default()
}

/// Whether a `<pivotField>` shows subtotals: custom subtotal functions (Field Settings ›
/// Subtotals › Custom, `sumSubtotal` and the like, ECMA-376 §18.10.1.69) turn them on as
/// well as `defaultSubtotal`.
fn field_subtotals(e: &El) -> bool {
    has_custom_subtotals(e) || e.flag("defaultSubtotal", true)
}

fn has_custom_subtotals(e: &El) -> bool {
    CUSTOM_SUBTOTALS.iter().any(|a| e.flag(a, false))
}

/// The x14 element of an `<extLst><ext uri>` (e.g. `x14:pivotField`), if present.
fn x14_ext<'a>(e: &'a El, uri: &str) -> Option<&'a El> {
    let ext = e.child("extLst")?.kids("ext").find(|x| x.attr("uri").is_some_and(|u| u.eq_ignore_ascii_case(uri)))?;
    ext.children.first()
}

/// A pivot field's sort by value, from its `<autoSortScope>`: the data field (dataFields
/// position) the scope's Values reference (`4294967294`) names, and the other references as
/// (cache field, index into that field's `<items>`). `Err(())` for a scope GridCraft can't
/// represent (a reference naming no item or several).
type AutoSort = Result<(usize, Vec<(usize, usize)>), ()>;

fn auto_sort_scope(e: &El) -> Option<AutoSort> {
    let refs = e.path(&["autoSortScope", "pivotArea", "references"])?;
    let r = refs.kids("reference").find(|r| r.attr("field") == Some(VALUES_FIELD))?;
    let data = r.child("x").and_then(|x| x.attr_u32("v"))? as usize;
    let mut scope = Vec::new();
    for r in refs.kids("reference").filter(|r| r.attr("field") != Some(VALUES_FIELD)) {
        let xs: Vec<&El> = r.kids("x").collect();
        match (r.attr_u32("field"), xs.as_slice()) {
            (Some(f), [x]) if !r.flag("byPosition", false) && !r.flag("relative", false) => match x.attr_u32("v") {
                Some(v) => scope.push((f as usize, v as usize)),
                None => return Some(Err(())),
            },
            _ => return Some(Err(())),
        }
    }
    Some(Ok((data, scope)))
}

/// (source column name, date grouping) of a cache field used on an axis.
fn field_ref(cx: &mut Ctx<'_>, cache: &CacheIn, i: usize) -> Option<(String, PivotDateGroup)> {
    let f = cache.fields.get(i)?;
    if f.formula.is_some() {
        cx.warn("calculated PivotTable fields are not supported");
        return None;
    }
    match &f.group {
        Some(g) => {
            let by = match g.by {
                Some(b) => b,
                None => {
                    cx.warn(format!("PivotTable grouping of \"{}\" is not supported; the field is ungrouped", f.name));
                    PivotDateGroup::None
                }
            };
            let base_name = if f.database { f.name.clone() } else { g.base.and_then(|b| cache.fields.get(b))?.name.clone() };
            Some((base_name, by))
        }
        None if !f.database => {
            cx.warn(format!("PivotTable field \"{}\" (grouped items) is not supported", f.name));
            None
        }
        None => Some((f.name.clone(), PivotDateGroup::None)),
    }
}

fn read_table(cx: &mut Ctx<'_>, x: &El, cache: &CacheIn, id: u32) -> Option<PivotTable> {
    let mut pt = PivotTable { id, cache_id: gridcraft_model::PivotCacheId(Some(cache.id)), ..PivotTable::default() };
    if let Some(n) = x.attr("name").filter(|n| !n.trim().is_empty()) {
        pt.name = n.to_string();
    } else {
        pt.name = format!("PivotTable{id}");
    }
    pt.source = cache.source.clone();
    let Some(loc) = x.child("location").and_then(|l| l.attr("ref")).and_then(|r| RangeRef::parse(&r.replace('$', ""))) else {
        cx.warn(format!("PivotTable \"{}\" has no valid location; kept as plain cells", pt.name));
        return None;
    };
    // Field lists are capped, and a field is used once (Excel puts a field on one axis, once):
    // each use walks the field's items.
    // A list cut at the cap is reported, so an incomplete PivotTable can be told apart.
    let mut truncated = false;
    let pfs: Vec<&El> = x.child("pivotFields").map(|p| capped(p.kids("pivotField"), &mut truncated)).unwrap_or_default();
    let pf = |i: usize| pfs.get(i).copied();
    let mut used_fields = std::collections::HashSet::new();
    let mut idx_list = |name: &str, attr: &str| -> Vec<i64> {
        x.child(name)
            .map(|e| capped(e.children.iter().filter_map(|f| f.attr_i64(attr)).filter(|&i| i < 0 || used_fields.insert(i)), &mut truncated))
            .unwrap_or_default()
    };
    let row_x = idx_list("rowFields", "x");
    let col_x = idx_list("colFields", "x");
    let mut page_fields = std::collections::HashSet::new();
    let pages: Vec<&El> = x
        .child("pageFields")
        .map(|p| capped(p.kids("pageField").filter(|f| f.attr_i64("fld").is_none_or(|i| page_fields.insert(i))), &mut truncated))
        .unwrap_or_default();

    // Layout from the first axis field (pivotField defaults: compact and outline on).
    let first_axis = row_x.iter().chain(col_x.iter()).find(|i| **i >= 0).and_then(|i| pf(*i as usize));
    let (compact, outline) = match first_axis {
        Some(f) => (f.flag("compact", true), f.flag("outline", true)),
        None => (x.flag("compact", true), x.flag("outline", false)),
    };
    pt.layout = if compact {
        PivotLayout::Compact
    } else if outline {
        PivotLayout::Outline
    } else {
        PivotLayout::Tabular
    };
    if let Some(f) = first_axis {
        pt.subtotals = x14_ext(x, EXT_XPDL_TABLE)
            .and_then(|e| e.attr("EnabledSubtotalsDefault"))
            .map_or_else(|| field_subtotals(f), |v| v == "1" || v.eq_ignore_ascii_case("true"));
        pt.subtotals_top = f.flag("subtotalTop", true);
    }
    pt.grand_totals_rows = x.flag("colGrandTotals", true);
    pt.grand_totals_cols = x.flag("rowGrandTotals", true);
    pt.show_headers = x.flag("showHeaders", true);
    pt.enable_show_details = x.flag("enableDrill", true);
    read_style_info(x, &mut pt);
    let caption = |a: &str| x.attr(a).filter(|c| !c.is_empty()).map(str::to_string);
    pt.row_header_caption = caption("rowHeaderCaption");
    pt.col_header_caption = caption("colHeaderCaption");
    pt.data_caption = caption("dataCaption").filter(|c| c != "Values");
    pt.grand_total_caption = caption("grandTotalCaption");
    pt.empty_caption = caption("missingCaption").filter(|_| x.flag("showMissing", true));
    pt.show_values_row = !x14_ext(x, EXT_X14_TABLE).is_some_and(|e| e.flag("hideValuesRow", false));
    pt.classic_layout = x.flag("gridDropZones", false);
    if x.child("calculatedItems").is_some() || x.path(&["filters", "filter"]).is_some() {
        cx.warn(format!("PivotTable \"{}\": calculated items and label/value filters are not supported", pt.name));
    }

    let mut filters: Vec<PivotFilter> = Vec::new();

    // Per axis field (in rows-then-columns order): its cache field and its sort by value.
    let mut sort_by: Vec<Option<AutoSort>> = Vec::new();
    let mut axis_ix: Vec<usize> = Vec::new();
    let table_subtotals = pt.subtotals;
    // Lower-cased names of the fields already filtered on an axis, the first of which wins (as
    // `same` compares them): scanning the filters for each field would be quadratic.
    let mut axis_filtered: std::collections::HashSet<String> = std::collections::HashSet::new();
    // (fields, position of Σ Values (`-2`) among them)
    let mut axis = |cx: &mut Ctx<'_>, list: &[i64], filters: &mut Vec<PivotFilter>| -> (Vec<PivotField>, Option<usize>) {
        let mut v = Vec::new();
        let mut values_at = None;
        for &i in list {
            if i == -2 {
                values_at = Some(v.len());
                continue;
            }
            if i < 0 {
                continue;
            }
            let i = i as usize;
            let Some((name, group)) = field_ref(cx, cache, i) else { continue };
            let labels: &[String] = cache.fields.get(i).map(|f| f.labels()).unwrap_or(&[]);
            let mut f = PivotField { source_col: name.clone(), date_group: group, sort: PivotSort::None, ..PivotField::default() };
            let mut by = None;
            if let Some(e) = pf(i) {
                f.sort = match e.attr("sortType") {
                    Some("ascending") => PivotSort::Asc,
                    Some("descending") => PivotSort::Desc,
                    _ => PivotSort::None,
                };
                by = auto_sort_scope(e);
                f.caption = e.attr("name").filter(|n| !n.is_empty()).map(str::to_string);
                let custom = has_custom_subtotals(e);
                if custom {
                    cx.warn(format!("PivotTable field \"{name}\": custom subtotal functions are not supported; it shows the automatic subtotal"));
                }
                let own = field_subtotals(e);
                f.subtotal = (own != table_subtotals).then_some(own);
                f.repeat_labels = x14_ext(e, EXT_X14_FIELD).is_some_and(|x| x.flag("fillDownLabels", false));
                if group == PivotDateGroup::None {
                    f.number_format = e
                        .attr_u32("numFmtId")
                        .filter(|id| *id != 0)
                        .and_then(|id| crate::tables::builtin_format(id).map(str::to_string).or_else(|| cx.num_fmts.get(&id).cloned()));
                }
                let items = items_of(e);
                let lab = |it: &El| it.attr_u32("x").and_then(|k| labels.get(k as usize)).cloned();
                f.collapsed_items = items.iter().filter(|it| !it.flag("sd", true)).filter_map(|it| lab(it)).collect();
                f.item_captions = items.iter().filter_map(|it| Some((lab(it)?, it.attr("n").filter(|n| !n.is_empty())?.to_string()))).collect();
                // Not auto-sorted: the `<items>` list fixes the order, even when it happens to be
                // cache order (new items follow it). The engine shows date groups in date order,
                // so they keep a list only when it was reordered.
                let xs: Vec<u32> = items.iter().filter_map(|it| it.attr_u32("x")).collect();
                let reordered = xs.windows(2).any(|w| w[0] > w[1]);
                if f.sort == PivotSort::None && (group == PivotDateGroup::None || reordered) {
                    f.item_order = items.iter().filter_map(|it| lab(it)).collect();
                }
                if items.iter().any(|it| it.flag("h", false)) && axis_filtered.insert(name.to_lowercase()) {
                    let shown: Vec<String> = items.iter().filter(|it| !it.flag("h", false)).filter_map(|it| lab(it)).collect();
                    filters.push(PivotFilter { source_col: name.clone(), selected: Some(shown), hidden: false, date_group: group });
                }
            }
            sort_by.push(by);
            axis_ix.push(i);
            v.push(f);
        }
        (v, values_at)
    };
    let mut page_filters = Vec::new();
    for p in &pages {
        let Some(i) = p.attr_i64("fld").filter(|i| *i >= 0).map(|i| i as usize) else { continue };
        let Some((name, group)) = field_ref(cx, cache, i) else { continue };
        let labels: &[String] = cache.fields.get(i).map(|f| f.labels()).unwrap_or(&[]);
        let selected = page_field_selected(p, pf(i), labels);
        page_filters.push(PivotFilter { source_col: name, selected, hidden: false, date_group: group });
    }
    let mut axis_filters = Vec::new();
    let (rows, rows_values) = axis(cx, &row_x, &mut axis_filters);
    let (columns, cols_values) = axis(cx, &col_x, &mut axis_filters);
    pt.rows = rows;
    pt.columns = columns;
    // Σ Values: where its `-2` entry is, else `dataOnRows` and `dataPosition`; after the last
    // field by default. In Columns it always comes last, as the column layout puts it there.
    pt.values_on_rows = rows_values.is_some() || (cols_values.is_none() && x.flag("dataOnRows", false));
    pt.values_position =
        if pt.values_on_rows { rows_values.or_else(|| x.attr_u32("dataPosition").map(|p| p as usize)).filter(|p| *p < pt.rows.len()) } else { None };
    filters.extend(page_filters);
    filters.extend(axis_filters);
    // Hidden items of a field in no area: a slicer's selection on a field the table doesn't show.
    // A date-grouped cache field keeps its grouping, so the items are its groups.
    let page_x: Vec<i64> = pages.iter().filter_map(|p| p.attr_i64("fld")).collect();
    for (i, e) in pfs.iter().enumerate() {
        let placed = [&row_x, &col_x, &page_x].iter().any(|l| l.contains(&(i as i64)));
        let items = items_of(e);
        if placed || !items.iter().any(|it| it.flag("h", false)) {
            continue;
        }
        // Items of a grouping GridCraft can't compute could never match a source value.
        if let Some(f) = cache.fields.get(i).filter(|f| f.group.as_ref().is_some_and(|g| g.by.is_none())) {
            cx.warn(format!("PivotTable \"{}\": the filter on grouped field \"{}\" is not supported and was ignored", pt.name, f.name));
            continue;
        }
        let Some((name, group)) = field_ref(cx, cache, i) else { continue };
        if filters.iter().any(|f| same(&f.source_col, &name)) {
            continue;
        }
        let labels = cache.fields.get(i).map(|f| f.labels().to_vec()).unwrap_or_default();
        let shown =
            items.iter().filter(|it| !it.flag("h", false)).filter_map(|it| it.attr_u32("x").and_then(|k| labels.get(k as usize)).cloned()).collect();
        filters.push(PivotFilter { source_col: name, selected: Some(shown), hidden: true, date_group: group });
    }
    pt.filters = filters;
    pt.normalize_filter_groups();

    // Values.
    // dataFields position -> value index.
    let mut data_value: Vec<Option<usize>> = Vec::new();
    if let Some(df) = x.child("dataFields") {
        truncated |= df.kids("dataField").nth(MAX_FIELDS).is_some();
        for d in df.kids("dataField").take(MAX_FIELDS) {
            data_value.push(None);
            let Some(i) = d.attr_u32("fld").map(|i| i as usize) else { continue };
            let Some(f) = cache.fields.get(i) else { continue };
            let func = match d.attr("subtotal") {
                None => PivotFunc::Sum,
                Some(s) => func_from(s).unwrap_or_else(|| {
                    cx.warn(format!("PivotTable function {s} is not supported; using Sum"));
                    PivotFunc::Sum
                }),
            };
            let ext = d
                .child("extLst")
                .into_iter()
                .flat_map(|l| l.kids("ext"))
                .filter_map(|e| e.child("dataField"))
                .find_map(|e| e.attr("pivotShowAs").map(str::to_string));
            let show_as = match (ext.as_deref(), d.attr("showDataAs")) {
                (Some("rankDescending"), _) => PivotShowAs::Rank,
                (Some(other), _) => {
                    cx.warn(format!("PivotTable show-values-as {other} is not supported"));
                    PivotShowAs::Normal
                }
                (None, None | Some("normal")) => PivotShowAs::Normal,
                (None, Some("percentOfTotal")) => PivotShowAs::PercentOfGrandTotal,
                (None, Some("percentOfCol")) => PivotShowAs::PercentOfColumnTotal,
                (None, Some("percentOfRow")) => PivotShowAs::PercentOfRowTotal,
                (None, Some("runTotal")) => PivotShowAs::RunningTotal,
                (None, Some(other)) => {
                    cx.warn(format!("PivotTable show-values-as {other} is not supported"));
                    PivotShowAs::Normal
                }
            };
            let number_format = d
                .attr_u32("numFmtId")
                .filter(|id| *id != 0)
                .and_then(|id| crate::tables::builtin_format(id).map(str::to_string).or_else(|| cx.num_fmts.get(&id).cloned()));
            let name = d.attr("name").map(str::to_string).unwrap_or_else(|| format!("{} of {}", func_caption(func), f.name));
            if let Some(last) = data_value.last_mut() {
                *last = Some(pt.values.len());
            }
            pt.values.push(PivotValue { source_col: f.name.clone(), func, name, show_as, number_format });
        }
    }
    // Sort scopes: each reference must name an item of a field on the other axis.
    let n_rows = pt.rows.len();
    let keys: Vec<(String, PivotDateGroup)> = pt.rows.iter().chain(pt.columns.iter()).map(|f| (f.source_col.clone(), f.date_group)).collect();
    let item_label = |j: usize, x: usize| -> Option<String> {
        let k = items_of(pf(j)?).get(x)?.attr_u32("x")?;
        cache.fields.get(j)?.labels().get(k as usize).cloned()
    };
    let mut sorts: Vec<Option<(usize, Vec<PivotScopeItem>)>> = Vec::with_capacity(sort_by.len());
    for (p, by) in sort_by.into_iter().enumerate() {
        let others = if p < n_rows { n_rows..keys.len() } else { 0..n_rows };
        let resolved = by.map(|by| {
            let (d, refs) = by?;
            let scope = refs
                .into_iter()
                .map(|(j, x)| {
                    let k = others.clone().find(|k| axis_ix.get(*k) == Some(&j)).ok_or(())?;
                    let (source_col, date_group) = keys.get(k).cloned().ok_or(())?;
                    Ok(PivotScopeItem { source_col, date_group, item: item_label(j, x).ok_or(())? })
                })
                .collect::<Result<Vec<_>, ()>>()?;
            Ok((d, scope))
        });
        if matches!(resolved, Some(Err(()))) {
            cx.warn(format!(
                "PivotTable \"{}\": a sort by value of \"{}\" has a scope that is not supported; it sorts by the label",
                pt.name,
                keys.get(p).map(|k| k.0.as_str()).unwrap_or("")
            ));
        }
        sorts.push(resolved.and_then(Result::ok));
    }
    let values = &pt.values;
    for (f, by) in pt.rows.iter_mut().chain(pt.columns.iter_mut()).zip(sorts) {
        let Some((d, scope)) = by else { continue };
        f.sort_by = data_value.get(d).copied().flatten().and_then(|v| values.get(v)).map(|v| v.name.clone());
        if f.sort_by.is_some() {
            f.sort_scope = scope;
        }
    }

    // Calculated fields of the cache (each PivotTable on it can use them); the first of a
    // lower-cased name wins, looked up in a set rather than by scanning the fields kept so far.
    let mut calc_names: std::collections::HashSet<String> = pt.calculated_fields.iter().map(|c| c.name.to_lowercase()).collect();
    for f in &cache.fields {
        if let Some(formula) = &f.formula
            && !f.name.trim().is_empty()
            && calc_names.insert(f.name.to_lowercase())
        {
            let formula = formula.trim();
            let formula = if formula.starts_with('=') { formula.to_string() } else { format!("={formula}") };
            pt.calculated_fields.push(PivotCalcField { name: f.name.clone(), formula });
        }
    }

    if truncated {
        cx.warn(format!("PivotTable \"{}\": field lists past {MAX_FIELDS} entries were dropped; the table is incomplete", pt.name));
    }
    pt.anchor = loc.start;
    let loc_el = x.child("location");
    let page_rows = loc_el.and_then(|l| l.attr_u32("rowPageCount")).unwrap_or(pages.len() as u32);
    let above = if page_rows == 0 { 0 } else { page_rows.saturating_add(1) };
    let report_start_row = loc.start.row.saturating_sub(above);
    let last_range = RangeRef::new(CellRef::new(report_start_row, loc.start.col), loc.end);
    pt.last_range = Some(last_range);
    pt.style_grid = PivotStyleGrid(Some(Arc::new(read_style_layout(x, loc, loc_el, page_rows, row_x.len(), col_x.len()))));
    if let Some(cells) = build_ooxml_data_index(x, loc, loc_el, report_start_row, &row_x, &col_x, &pfs, cache, &pt.values, &data_value) {
        pt.data_index.set_cells(cells);
    }
    Some(pt)
}

/// Resolves a `<pageField>`'s selected item(s). ECMA-376 §18.10.1.62: `pageField/@item` indexes
/// `<pivotField><items>`, whose `@x` indexes the cache field's items (falling back to direct
/// cache indexing only when the `<pivotField>` carries no `<items>`).
fn page_field_selected(p: &El, pf: Option<&El>, labels: &[String]) -> Option<Vec<String>> {
    if let Some(k) = p.attr_u32("item") {
        let resolved = match pf.and_then(|e| e.child("items")) {
            Some(its) => its.kids("item").nth(k as usize).and_then(|it| it.attr_u32("x")).and_then(|sx| labels.get(sx as usize)),
            None => labels.get(k as usize),
        };
        return resolved.map(|l| vec![l.clone()]);
    }
    pf.and_then(|e| {
        let items = items_of(e);
        items.iter().any(|it| it.flag("h", false)).then(|| {
            items.iter().filter(|it| !it.flag("h", false)).filter_map(|it| it.attr_u32("x").and_then(|k| labels.get(k as usize)).cloned()).collect()
        })
    })
}

/// Cap on cells indexed from OOXML `<rowItems>`/`<colItems>` (matches `engine::pivot::MAX_INDEXED_CELLS`).
const MAX_INDEXED_CELLS: usize = 1_000_000;

/// Parses an optional unsigned integer XML attribute strictly: absent -> `Ok(None)`,
/// valid non-negative integer -> `Ok(Some(v))`, malformed/negative -> `Err(())`.
fn strict_opt_u32(el: &El, k: &str) -> Result<Option<u32>, ()> {
    match el.attr(k) {
        None => Ok(None),
        Some(raw) => raw.trim().parse::<u32>().map(Some).map_err(|_| ()),
    }
}

/// One decoded row or column entry from `<rowItems>` / `<colItems>`.
struct AxisEntry {
    /// `(cache_field_index, PivotItemRef)` for each non-Values field on this axis.
    items: Vec<(usize, PivotItemRef)>,
    /// Data field index (into `<dataFields>`) when `Values` (`-2`) is on this axis or `@i` is set.
    data_field_idx: Option<usize>,
    /// True for `t="grand"`, subtotal `t` (`default`, `sum`, ...), or trailing subtotal `<item>`.
    is_subtotal_or_grand: bool,
    /// True when this entry stepped into the `-2` (Values) axis field.
    has_values_field: bool,
    /// Raw `(field_idx, item_idx)` path (including `-2`) before stripping subtotal markers.
    raw_path: Vec<(i64, usize)>,
}

/// Decodes `<rowItems>` or `<colItems>` (ECMA-376 §18.10.1.84 / §18.10.1.17) into per-row/column
/// axis item paths. `None` entries in the returned `Vec` correspond to `t="blank"` spacer lines
/// or non-subtotal item-header lines. Returns `None` if any axis item is malformed or cannot be
/// resolved, so callers fall back instead of producing truncated keys.
fn decode_axis_items(axis_el: &El, field_indices: &[i64], pfs: &[&El], cache: &CacheIn, limit: usize) -> Option<Vec<Option<AxisEntry>>> {
    let pf_items: Vec<Option<Vec<&El>>> = pfs.iter().map(|pf| pf.child("items").map(|its| its.kids("item").collect())).collect();
    let mut out = Vec::new();
    let mut prev: Vec<(i64, usize)> = Vec::new();
    for iel in axis_el.kids("i").take(limit) {
        let t = iel.attr("t").unwrap_or("data");
        if t == "blank" {
            out.push(None);
            continue;
        }
        let r = strict_opt_u32(iel, "r").ok()?.unwrap_or(0) as usize;
        let mut df_idx = strict_opt_u32(iel, "i").ok()?.map(|v| v as usize);
        let mut xs = Vec::new();
        for x in iel.kids("x") {
            xs.push(strict_opt_u32(x, "v").ok()?.unwrap_or(0) as usize);
        }
        if t == "grand" {
            let mut has_values_field = false;
            for (j, &xv) in xs.iter().enumerate() {
                let pos = r.checked_add(j)?;
                if field_indices.get(pos).copied() == Some(-2) {
                    if df_idx.is_none() {
                        df_idx = Some(xv);
                    }
                    has_values_field = true;
                }
            }
            out.push(Some(AxisEntry {
                items: Vec::new(),
                data_field_idx: df_idx,
                is_subtotal_or_grand: true,
                has_values_field,
                raw_path: Vec::new(),
            }));
            prev.clear();
            continue;
        }
        if r > prev.len() {
            return None;
        }
        let mut cur: Vec<(i64, usize)> = prev.iter().take(r).copied().collect();
        for (j, &xv) in xs.iter().enumerate() {
            let pos = r.checked_add(j)?;
            if let Some(&fidx) = field_indices.get(pos) {
                cur.push((fidx, xv));
            } else if !field_indices.is_empty() {
                return None;
            }
        }
        prev = cur.clone();
        let mut is_subtotal_or_grand = is_subtotal_item_type(t);
        let mut has_values_field = false;
        let mut items = Vec::with_capacity(cur.len());
        for &(fidx, pitem_idx) in &cur {
            if fidx == -2 {
                df_idx = Some(pitem_idx);
                has_values_field = true;
                continue;
            }
            if fidx < 0 {
                continue;
            }
            let fi = fidx as usize;
            let cf = cache.fields.get(fi)?;
            let pitems = pf_items.get(fi)?.as_deref()?;
            let pit = pitems.get(pitem_idx)?;
            if pit.attr("t").is_some_and(|it_t| it_t != "data") {
                // Trailing subtotal/total marker item (e.g. `t="default"`): omitted from item path.
                is_subtotal_or_grand = true;
                continue;
            }
            let sx = strict_opt_u32(pit, "x").ok()?? as usize;
            let base_label = cf.labels().get(sx)?;
            let value = cf.values().get(sx).cloned().unwrap_or_else(|| Value::text(base_label.as_str()));
            let label = pit.attr("n").filter(|n| !n.is_empty()).map(str::to_string).unwrap_or_else(|| base_label.clone());
            // Use the canonical cache/source field name (matching `compute.rs::item_refs`), not
            // `pivotField/@name` caption, so GETPIVOTDATA works identically before and after refresh
            // for both source names and custom captions (resolved via caption->source retry).
            items.push((fi, PivotItemRef { field: cf.name.clone(), label, value }));
        }
        out.push(Some(AxisEntry { items, data_field_idx: df_idx, is_subtotal_or_grand, has_values_field, raw_path: cur }));
    }

    // Filter out non-subtotal item-header rows/columns (compact/outline layout headers when
    // subtotals are at the bottom or disabled, or outer item headers before stepping into `-2`
    // Values on the same axis) so blank header lines never shadow real data/subtotal rows.
    let axis_has_values = field_indices.contains(&-2);
    let mut drop_header = vec![false; out.len()];
    for idx in 0..out.len() {
        let Some(cur) = out[idx].as_ref() else { continue };
        if cur.is_subtotal_or_grand {
            continue;
        }
        let next = out[idx + 1..].iter().flatten().next();
        let is_item_header = next.is_some_and(|n| {
            (cur.raw_path.len() < n.raw_path.len() && n.raw_path.starts_with(&cur.raw_path))
                || (axis_has_values
                    && !cur.has_values_field
                    && n.items.len() >= cur.items.len()
                    && n.items[..cur.items.len()] == cur.items[..]
                    && (n.has_values_field || n.is_subtotal_or_grand))
        });
        if !is_item_header {
            continue;
        }
        if axis_has_values && !cur.has_values_field {
            drop_header[idx] = true;
            continue;
        }
        let last_fidx = cur.raw_path.last().map(|(f, _)| *f);
        match last_fidx {
            Some(fi) if fi >= 0 => {
                let pf = pfs.get(fi as usize).copied();
                let subtotals_on = pf.is_none_or(field_subtotals);
                let subtotal_top = pf.is_none_or(|e| e.attr_bool("subtotalTop").unwrap_or(true));
                if !subtotals_on || !subtotal_top {
                    drop_header[idx] = true;
                }
            }
            _ => {
                // `-2` Values header (when Values precedes another axis field) never holds a top subtotal.
                drop_header[idx] = true;
            }
        }
    }
    for (slot, drop) in out.iter_mut().zip(drop_header) {
        if drop {
            *slot = None;
        }
    }
    Some(out)
}

/// Builds the `GETPIVOTDATA` lookup index directly from the OOXML `<rowItems>` and `<colItems>`
/// describing the stored PivotTable report. Returns `None` when the XML carries no stored layout
/// or cannot be decoded cleanly, letting the lazy engine indexer run.
#[allow(clippy::too_many_arguments)]
fn build_ooxml_data_index(
    x: &El,
    loc: RangeRef,
    loc_el: Option<&El>,
    report_start_row: u32,
    row_x: &[i64],
    col_x: &[i64],
    pfs: &[&El],
    cache: &CacheIn,
    pt_values: &[PivotValue],
    data_value: &[Option<usize>],
) -> Option<Vec<PivotDataCell>> {
    if pt_values.is_empty() {
        return None;
    }
    let row_el = x.child("rowItems");
    let col_el = x.child("colItems");
    if row_el.is_none() && col_el.is_none() {
        return None;
    }
    let first_data_row = loc_el.and_then(|l| l.attr_u32("firstDataRow")).unwrap_or(1);
    let first_data_col = loc_el.and_then(|l| l.attr_u32("firstDataCol")).unwrap_or(1);
    let body_rows = loc.end.row.saturating_sub(loc.start.row).saturating_add(1) as usize;
    let body_cols = loc.end.col.saturating_sub(loc.start.col).saturating_add(1) as usize;
    let row_entries = match row_el {
        Some(el) => decode_axis_items(el, row_x, pfs, cache, body_rows)?,
        None => vec![Some(AxisEntry {
            items: Vec::new(),
            data_field_idx: None,
            is_subtotal_or_grand: false,
            has_values_field: false,
            raw_path: Vec::new(),
        })],
    };
    let col_entries = match col_el {
        Some(el) => decode_axis_items(el, col_x, pfs, cache, body_cols)?,
        None => vec![Some(AxisEntry {
            items: Vec::new(),
            data_field_idx: None,
            is_subtotal_or_grand: false,
            has_values_field: false,
            raw_path: Vec::new(),
        })],
    };
    if row_entries.is_empty() || col_entries.is_empty() {
        return None;
    }
    let data_row_base = loc.start.row.saturating_sub(report_start_row).saturating_add(first_data_row);
    let max_row_off = loc.end.row.saturating_sub(report_start_row);
    let max_col_off = loc.end.col.saturating_sub(loc.start.col);
    let mut cells: Vec<PivotDataCell> = Vec::new();
    // Maps lookup key -> (index in `cells`, precedence) so explicit subtotal or `-2` value rows
    // win over any earlier header row that produced the same key.
    let mut seen: HashMap<String, (usize, u8)> = HashMap::new();
    let mut record = |cell: PivotDataCell, prio: u8| -> bool {
        let key_items: Vec<(&str, &str, bool)> =
            cell.items.iter().map(|it| (it.field.as_str(), it.label.as_str(), matches!(it.value, Value::Bool(_)))).collect();
        let key = pivot_key(&cell.data_field, &key_items);
        if let Some((old_idx, old_prio)) = seen.get_mut(&key)
            && cells[*old_idx].items == cell.items
        {
            if prio > *old_prio {
                cells[*old_idx] = cell;
                *old_prio = prio;
            }
            return true;
        }
        if cells.len() >= MAX_INDEXED_CELLS {
            return false;
        }
        let idx = cells.len();
        cells.push(cell);
        seen.insert(key, (idx, prio));
        true
    };
    for (ri, r_opt) in row_entries.iter().enumerate() {
        let Some(re) = r_opt else { continue };
        let row_off = data_row_base.saturating_add(ri as u32);
        if row_off > max_row_off {
            break;
        }
        for (ci, c_opt) in col_entries.iter().enumerate() {
            let Some(ce) = c_opt else { continue };
            let col_off = first_data_col.saturating_add(ci as u32);
            if col_off > max_col_off {
                break;
            }
            let raw_df = re.data_field_idx.or(ce.data_field_idx).unwrap_or(0);
            let Some(vi) = data_value.get(raw_df).copied().flatten() else { continue };
            let Some(vdef) = pt_values.get(vi) else { continue };
            let full_items: Vec<PivotItemRef> = re.items.iter().chain(&ce.items).map(|(_, it)| it.clone()).collect();
            let prio = 1 + (u8::from(re.is_subtotal_or_grand || ce.is_subtotal_or_grand) << 1) + u8::from(re.has_values_field || ce.has_values_field);
            let full_cell =
                PivotDataCell { data_field: vdef.name.clone(), source_col: vdef.source_col.clone(), items: full_items, row: row_off, col: col_off };
            if !record(full_cell, prio) {
                return Some(cells);
            }
            // If a manual/non-date parent group field (`fieldGroup/@base = b`, no `rangePr`)
            // appears alongside its base field `b`, Excel allows `GETPIVOTDATA` to omit the
            // parent group because each base item uniquely identifies its group.
            let is_manual_parent = |fi: usize| {
                cache.fields.get(fi).is_some_and(|cf| {
                    cf.group.as_ref().is_some_and(|g| {
                        g.by.is_none() && g.base.is_some_and(|base| re.items.iter().chain(&ce.items).any(|(other, _)| *other == base))
                    })
                })
            };
            let has_manual_parent = re.items.iter().chain(&ce.items).any(|(fi, _)| is_manual_parent(*fi));
            if has_manual_parent {
                let stripped: Vec<PivotItemRef> =
                    re.items.iter().chain(&ce.items).filter(|(fi, _)| !is_manual_parent(*fi)).map(|(_, it)| it.clone()).collect();
                let stripped_cell =
                    PivotDataCell { data_field: vdef.name.clone(), source_col: vdef.source_col.clone(), items: stripped, row: row_off, col: col_off };
                if !record(stripped_cell, 0) {
                    return Some(cells);
                }
            }
        }
    }
    (!cells.is_empty()).then_some(cells)
}

/// The first [`MAX_FIELDS`] items; sets `truncated` when there were more.
fn capped<T>(items: impl Iterator<Item = T>, truncated: &mut bool) -> Vec<T> {
    let mut v: Vec<T> = items.take(MAX_FIELDS.saturating_add(1)).collect();
    if v.len() > MAX_FIELDS {
        v.truncate(MAX_FIELDS);
        *truncated = true;
    }
    v
}

/// Parses workbook-defined `<tableStyles>` elements that apply to PivotTables (`pivot != "0"`).
fn read_custom_table_styles(root: &El, dxfs: &[gridcraft_model::Style], palette: &[u32], out: &mut HashMap<String, Arc<Vec<PivotStyleElement>>>) {
    let Some(ts_list) = root.child("tableStyles") else { return };
    let raw_dxfs: Vec<&El> = root.child("dxfs").map(|d| d.kids("dxf").collect()).unwrap_or_default();
    for ts in ts_list.kids("tableStyle").take(MAX_TABLE_STYLES) {
        if !ts.flag("pivot", true) {
            continue;
        }
        let Some(name) = ts.attr("name").filter(|n| !n.is_empty()) else { continue };
        let mut elems = Vec::new();
        for el in ts.kids("tableStyleElement").take(MAX_TABLE_STYLE_ELEMENTS) {
            let Some(kind) = el.attr("type").filter(|t| !t.is_empty()) else { continue };
            let Some(dxf_id) = el.attr_u32("dxfId") else { continue };
            let Some(base_style) = dxfs.get(dxf_id as usize) else { continue };
            let mut style = base_style.clone();
            // `read_font` normalizes `<color theme="1"/>` (dk1/black) to `Color::Auto`, which would
            // look unset when layering higher-priority DXFs (e.g. `firstRowStripe`, `firstSubtotalRow`)
            // over a coloured `wholeTable` DXF. Preserve any explicitly stated font color.
            if style.font.color == gridcraft_model::Color::Auto
                && let Some(raw_color) = raw_dxfs.get(dxf_id as usize).and_then(|d| d.child("font")).and_then(|f| f.child("color"))
            {
                let c = crate::styles::read_color(raw_color, palette);
                style.font.color = if c == gridcraft_model::Color::Auto { gridcraft_model::Color::Theme(1, 0) } else { c };
            }
            let size = el.attr_u32("size").unwrap_or(1).max(1);
            elems.push(PivotStyleElement { kind: kind.to_string(), size, style });
        }
        out.insert(name.to_ascii_lowercase(), Arc::new(elems));
    }
}

fn is_subtotal_item_type(t: &str) -> bool {
    matches!(t, "default" | "sum" | "countA" | "avg" | "max" | "min" | "product" | "count" | "stdDev" | "stdDevP" | "var" | "varP")
}

fn read_style_layout(x: &El, loc: RangeRef, loc_el: Option<&El>, page_rows: u32, row_field_count: usize, col_field_count: usize) -> PivotStyleLayout {
    let first_header_row = loc_el.and_then(|l| l.attr_u32("firstHeaderRow")).unwrap_or(0);
    let first_data_row = loc_el.and_then(|l| l.attr_u32("firstDataRow")).unwrap_or(1);
    let first_data_col = loc_el.and_then(|l| l.attr_u32("firstDataCol")).unwrap_or(1);
    let page_cols = loc_el.and_then(|l| l.attr_u32("colPageCount")).unwrap_or(1).max(1);
    let body_rows = loc.end.row.saturating_sub(loc.start.row).saturating_add(1) as usize;
    let body_cols = loc.end.col.saturating_sub(loc.start.col).saturating_add(1) as usize;

    let mut row_roles = Vec::new();
    let mut row_subheading = Vec::new();
    if let Some(ri) = x.child("rowItems") {
        for item in ri.kids("i").take(body_rows) {
            let t = item.attr("t").unwrap_or("data");
            let r = item.attr_u32("r").unwrap_or(0);
            let xc = item.kids("x").count() as u32;
            let role = if t == "grand" {
                PivotRowRole::GrandTotal
            } else if t == "blank" {
                PivotRowRole::Blank
            } else if is_subtotal_item_type(t) {
                PivotRowRole::Subtotal(r.saturating_add(xc.saturating_sub(1)).min(2) as u8)
            } else {
                PivotRowRole::Data
            };
            let sub = if role == PivotRowRole::Data && row_field_count > 1 && (r.saturating_add(xc) as usize) < row_field_count {
                Some(r.saturating_add(xc.saturating_sub(1)).min(2) as u8)
            } else {
                None
            };
            row_roles.push(role);
            row_subheading.push(sub);
        }
    }

    let mut col_roles = Vec::new();
    let mut col_subheading = Vec::new();
    if let Some(ci) = x.child("colItems") {
        for item in ci.kids("i").take(body_cols) {
            let t = item.attr("t").unwrap_or("data");
            let r = item.attr_u32("r").unwrap_or(0);
            let xc = item.kids("x").count() as u32;
            let role = if t == "grand" {
                PivotColRole::GrandTotal
            } else if t == "blank" {
                PivotColRole::Blank
            } else if is_subtotal_item_type(t) {
                PivotColRole::Subtotal(r.saturating_add(xc.saturating_sub(1)).min(2) as u8)
            } else {
                PivotColRole::Data
            };
            let sub = if role == PivotColRole::Data && col_field_count > 1 && (r.saturating_add(xc) as usize) < col_field_count {
                Some(r.saturating_add(xc.saturating_sub(1)).min(2) as u8)
            } else {
                None
            };
            col_roles.push(role);
            col_subheading.push(sub);
        }
    }

    PivotStyleLayout {
        body_range: Some(loc),
        first_header_row,
        first_data_row,
        first_data_col,
        page_rows,
        page_cols,
        row_roles,
        row_subheading,
        col_roles,
        col_subheading,
    }
}

/// Reads `<pivotTableStyleInfo>` name and display flags into `pt`.
fn read_style_info(x: &El, pt: &mut PivotTable) {
    if let Some(si) = x.child("pivotTableStyleInfo") {
        pt.style = si.attr("name").unwrap_or("").to_string();
        pt.style_options = PivotStyleOptions {
            show_row_headers: si.flag("showRowHeaders", false),
            show_col_headers: si.flag("showColHeaders", false),
            show_row_stripes: si.flag("showRowStripes", false),
            show_col_stripes: si.flag("showColStripes", false),
            show_last_column: si.flag("showLastColumn", false),
        };
    } else {
        pt.style.clear();
    }
}
