//! Writing worksheet parts and the parts they own (tables, comments, drawings, charts).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::sync::Arc;

use craft_xlsx::{
    Break as CraftBreak, CellRef as CraftCellRef, Column as CraftColumn, ConditionalFormatting as CraftCf, DataValidations as CraftDvs,
    Features as CraftFeatures, HeaderFooter as CraftHf, Hyperlink as CraftHyperlink, PageMargins as CraftMargins, PageSetup as CraftPageSetup,
    Pane as CraftPane, Print as CraftPrint, PrintOptions as CraftPrintOptions, Range as CraftRange, Selection as CraftSelection,
    SheetFormat as CraftSheetFormat, SheetPart as CraftSheetPart, SheetView as CraftSheetView,
};
use gridcraft_core::{CellError, CellRef, MAX_COLS, MAX_ROWS, RangeRef, Value};
use gridcraft_model::{
    Cell, CfOperator, CfRule, CfValueKind, Color, CondFormat, ErrorStyle, FilterCriterion, FormControl, LineInfo, NoteBox, NoteCorner, Orientation,
    PageOrder, Sheet, SparklineKind, Style, Table, TotalsFn, Validation, ValidationKind, Workbook, default_icon_thresholds, icon_count,
};

use crate::drawing::{Obj, image_content_type, image_ext};
use crate::fmla::text_to_file;
use crate::passthrough::TableBinding;
use crate::styles::{color_attrs, dxf_id, dxf_xml};
use crate::tables::{paper_id, px_to_col_width, px_to_pt};
use crate::write::{
    CT_CHART, CT_COMMENTS, CT_DRAWING, CT_TABLE, CT_THREADED_COMMENTS, NS_MAIN, NS_REL, NS_THREADED, Out, REL_THREADED_COMMENT, Rels, XML_DECL,
    x14_cf_rule_guid,
};
use crate::xml::{encode_xstring, esc, esc_attr, num, push_a1, push_esc, push_num, push_u64};
/// The worksheet extension holding Excel 2010 conditional formats.
const EXT_X14_CF: &str = "{78C0D931-6437-407d-A8EE-F0AAD7539E65}";

mod parts;
mod rules;

pub(crate) use parts::password_attrs;
use parts::{
    autofilter_xml, comments_xml, protected_ranges_xml, table_column_id, table_columns, table_filter, table_xml, thread_ids, threaded_xml, vml_xml,
};
use rules::{cf_rule_xml, solid_bar_x14, validation_xml, x14_cf_rule_xml};

fn sqref(ranges: &[RangeRef]) -> String {
    ranges.iter().map(|r| r.a1()).collect::<Vec<_>>().join(" ")
}

fn to_craft_cell_ref(c: CellRef) -> CraftCellRef {
    CraftCellRef::new(c.row, c.col)
}

fn to_craft_range(r: RangeRef) -> CraftRange {
    CraftRange::new(to_craft_cell_ref(r.start), to_craft_cell_ref(r.end))
}

fn parse_craft_fragment(xml: &str) -> Option<craft_xml::El> {
    let opts = craft_xml::Options::default().with_limits(craft_xml::Limits {
        max_input: crate::package::MAX_PART as usize,
        max_depth: crate::xml::MAX_DEPTH,
        max_nodes: crate::xml::MAX_XML_NODES,
        ..craft_xml::Limits::DEFAULT
    });
    opts.parse(xml.as_bytes()).ok()
}

fn push_fragment_kids(root: &mut craft_xml::El, xml: &str) {
    if xml.is_empty() {
        return;
    }
    let wrapped = format!("<root xmlns:r=\"{NS_REL}\">{xml}</root>");
    if let Some(parsed) = parse_craft_fragment(&wrapped) {
        root.children.extend(parsed.children);
    }
}

/// Projects the repaired names of tables that are emitted.
pub(crate) fn table_names(wb: &Workbook) -> Vec<Vec<Option<String>>> {
    wb.sheets
        .iter()
        .map(|sheet| sheet.tables.iter().map(|t| (t.range.height() > u32::from(t.totals_row)).then(|| t.name.clone())).collect())
        .collect()
}

pub fn write_sheet(wb: &Workbook, si: usize, selected: bool, table_names: &[Option<String>], out: &mut Out) -> (String, Rels) {
    let mut rels = Rels::default();
    let Some(sheet) = wb.sheet(si) else { return (String::new(), rels) };
    let nstyles = wb.styles.len() as u32;
    let sid = |s: gridcraft_model::StyleId| if s.0 < nstyles { s.0 } else { 0 };

    let folded: Vec<Table> = match &sheet.autofilter {
        Some(af) if !af.criteria.is_empty() => sheet
            .tables
            .iter()
            .cloned()
            .map(|mut t| {
                if t.filter_button
                    && (t.matches_autofilter(af.range)
                        || (t.header_row && t.filter.is_empty() && t.range.contains_range(&af.range) && t.range.start.col == af.range.start.col))
                {
                    for (offset, criterion) in &af.criteria {
                        if !t.filter.iter().any(|(o, _)| o == offset) {
                            t.filter.push((*offset, criterion.clone()));
                        }
                    }
                }
                t
            })
            .collect(),
        _ => sheet.tables.clone(),
    };
    let mut overrides: BTreeMap<CellRef, String> = BTreeMap::new();
    let tables: Vec<_> =
        folded.iter().zip(table_names).enumerate().filter_map(|(ti, (t, name))| Some((ti, t, table_columns(t), name.as_deref()?))).collect();
    for (_, t, cols, _) in &tables {
        if t.header_row {
            for (i, name) in cols.iter().enumerate() {
                overrides.insert(CellRef::new(t.range.start.row, t.range.start.col + i as u32), name.clone());
            }
        }
    }
    // Dynamic-array spills: anchor → range, and the spilled values to cache. A legacy formula
    // over a linked workbook is saved as read, without its spill (`external::saved_as_read`).
    let as_read = |a: &CellRef| sheet.cells.get(*a).and_then(|c| c.formula.as_ref()).is_some_and(|f| crate::external::saved_as_read(f));
    let dynamic: BTreeMap<CellRef, RangeRef> = sheet
        .spill_ranges
        .iter()
        .filter(|(a, r)| {
            !r.is_single()
                && sheet.cells.get(**a).is_some_and(|c| c.py.is_some() || c.formula.as_ref().is_some_and(|f| f.array.is_none()))
                && !as_read(a)
        })
        .map(|(a, r)| (*a, *r))
        .collect();
    let mut extra: BTreeMap<CellRef, Value> = BTreeMap::new();
    // Values inside spills and legacy array ranges.
    for (a, r) in sheet.spill_ranges.iter().filter(|(a, r)| !r.is_single() && !as_read(a)) {
        for c in r.iter().take(1_000_000) {
            if c != *a
                && let Some(v) = sheet.spill.get(&c)
                && sheet.cells.get(c).is_none_or(|x| x.formula.is_none() && x.value.is_empty())
            {
                extra.insert(c, v.clone());
            }
        }
    }

    let mut part = CraftSheetPart { root: craft_xml::El::new("worksheet"), ..Default::default() };
    // sheetPr
    let fit = sheet.print.fit_to.is_some();
    let summary = !sheet.summary_below || !sheet.summary_right;
    if sheet.tab_color.is_some() || fit || summary || sheet.transition_evaluation {
        let mut pr = craft_xml::El::new("sheetPr");
        if sheet.transition_evaluation {
            pr.set_attr("transitionEvaluation", "1");
        }
        if let Some(c) = &sheet.tab_color {
            let xml = format!("<tabColor {}/>", color_attrs(c));
            if let Some(el) = parse_craft_fragment(&xml) {
                pr.push(el);
            }
        }
        if summary {
            let mut outline = craft_xml::El::new("outlinePr");
            if !sheet.summary_below {
                outline.set_attr("summaryBelow", "0");
            }
            if !sheet.summary_right {
                outline.set_attr("summaryRight", "0");
            }
            pr.push(outline);
        }
        if fit {
            pr.push(craft_xml::El::new("pageSetUpPr").with_attr("fitToPage", "1"));
        }
        part.root.push(pr);
    }

    let max_row_outline = sheet.rows.values().map(|r| r.outline).max().unwrap_or(0);
    let max_col_outline = sheet.cols.values().map(|r| r.outline).max().unwrap_or(0);
    let geometry = gridcraft_model::Geometry::host(wb.xlsx_width_font.as_ref().unwrap_or(&wb.styles.get(gridcraft_model::StyleId::DEFAULT).font));
    let saved = sheet.saved_geometry;
    let width_unchanged = saved.is_some_and(|g| g.original_width_px == sheet.default_col_width);
    let height_unchanged = saved.is_some_and(|g| g.original_height_px == sheet.default_row_height);
    if saved.is_none_or(|g| g.has_format) || !width_unchanged || !height_unchanged || max_row_outline != 0 || max_col_outline != 0 {
        part.features.format = Some(CraftSheetFormat {
            base_col_width: saved.filter(|_| width_unchanged).and_then(|g| g.base_col_width),
            default_col_width: if width_unchanged {
                saved.and_then(|g| g.default_col_width)
            } else {
                (geometry != gridcraft_model::Geometry::Legacy || (sheet.default_col_width - gridcraft_model::DEFAULT_COL_WIDTH).abs() > 0.01)
                    .then(|| stored_width(geometry, sheet.default_col_width))
            },
            default_row_height: if height_unchanged { saved.and_then(|g| g.default_row_height) } else { Some(px_to_pt(sheet.default_row_height)) },
            custom_height: if height_unchanged {
                saved.and_then(|g| g.custom_height)
            } else {
                (sheet.default_row_height_custom || (sheet.default_row_height - gridcraft_model::DEFAULT_ROW_HEIGHT).abs() > 0.01).then_some(true)
            },
            outline_level_row: (max_row_outline > 0).then_some(u32::from(max_row_outline)),
            outline_level_col: (max_col_outline > 0).then_some(u32::from(max_col_outline)),
            ..Default::default()
        });
    }

    part.features.views = vec![build_craft_view(sheet, selected)];
    part.columns = build_craft_cols(sheet, geometry, &sid);
    part.merges = sheet.merges.iter().filter(|m| !m.is_single()).copied().map(to_craft_range).collect();

    // Register preserved parts before building the sheet elements that reference them.
    let kept = crate::passthrough::SheetKept::write(sheet, &mut rels, out);
    let (rules_frag, x14_cf) = build_sheet_rules(sheet, &mut part.features, &mut rels, out);
    push_fragment_kids(&mut part.root, &rules_frag);
    push_fragment_kids(&mut part.root, &kept.custom_sheet_views());
    push_fragment_kids(&mut part.root, &kept.custom_properties());
    build_sheet_printing(sheet, &mut part.features.print, &kept);
    let legacy = legacy_ids(sheet, &kept, out);
    part.features.drawing = write_sheet_drawing_rel(wb, si, &kept, legacy.as_ref(), &mut rels, out);
    let (legacy_rid, notes_frag) = write_sheet_notes_frag(sheet, si, &kept, legacy, &mut rels, out);
    part.features.legacy_drawing = legacy_rid;

    if !tables.is_empty() {
        for (ti, t, cols, name) in &tables {
            out.tables += 1;
            out.table_id += 1;
            let n = out.tables;
            let bound = kept.table_binding(t.id, cols);
            let filter = table_filter(sheet, t);
            let xml = table_xml(t, cols, name, out.table_id, &filter, bound.as_ref(), out);
            out.slicers.table_written((si, *ti), out.table_id, cols, |i| table_column_id(bound.as_ref(), i));
            let path = format!("xl/tables/table{n}.xml");
            out.part(&path, Some(CT_TABLE), xml.into_bytes());
            if let Some(b) = &bound {
                b.write(&path, out);
            }
            part.features.table_parts.push(rels.add("table", &format!("../tables/table{n}.xml")));
        }
    }

    let mut ext_xml = String::new();
    write_sheet_extensions(&mut ext_xml, sheet, &x14_cf, &out.slicers.on_sheet(si), out, &mut rels);
    push_fragment_kids(&mut part.root, &ext_xml);

    // Build the compact worksheet shell once with empty rows, then splice streamed `<sheetData>`.
    let shell = String::from_utf8(part.to_xml(&[])).unwrap_or_default();
    let (sd_start, sd_end) = if let Some(p) = shell.find("<sheetData/>") {
        (p, p + "<sheetData/>".len())
    } else if let Some(p) = shell.find("<sheetData></sheetData>") {
        (p, p + "<sheetData></sheetData>".len())
    } else {
        (shell.len(), shell.len())
    };
    let data = SheetCellData { workbook: wb, index: si, sheet, overrides: &overrides, extra: &extra, dynamic: &dynamic };
    let (sheet_data, used_dim) = stream_sheet_data(data, &sid, out);

    let mut xml = String::with_capacity(shell.len() + sheet_data.len() + notes_frag.len() + 32);
    let prefix = &shell[..sd_start];
    if let Some((lo, hi)) = used_dim
        && let Some(dp) = prefix.find("<dimension ref=\"A1\"/>")
    {
        xml.push_str(&prefix[..dp]);
        xml.push_str("<dimension ref=\"");
        push_a1(&mut xml, lo);
        if lo != hi {
            xml.push(':');
            push_a1(&mut xml, hi);
        }
        xml.push_str("\"/>");
        xml.push_str(&prefix[dp + "<dimension ref=\"A1\"/>".len()..]);
    } else {
        xml.push_str(prefix);
    }
    xml.push_str(&sheet_data);
    let suffix = &shell[sd_end..];
    if !notes_frag.is_empty() {
        let pos =
            ["<webPublishItems", "<tableParts", "<extLst", "</worksheet>"].iter().filter_map(|tag| suffix.find(tag)).min().unwrap_or(suffix.len());
        xml.push_str(&suffix[..pos]);
        xml.push_str(&notes_frag);
        xml.push_str(&suffix[pos..]);
    } else {
        xml.push_str(suffix);
    }
    (xml, rels)
}

fn stored_width(geometry: gridcraft_model::Geometry, px: f32) -> f64 {
    match geometry {
        gridcraft_model::Geometry::Legacy => px_to_col_width(px),
        gridcraft_model::Geometry::Mac { digit_pt } => (f64::from(px) * 0.75).round() / f64::from(digit_pt.max(1)),
    }
}

fn build_craft_cols(sheet: &Sheet, geometry: gridcraft_model::Geometry, sid: &dyn Fn(gridcraft_model::StyleId) -> u32) -> Vec<CraftColumn> {
    // The stored width and its `customWidth`/`bestFit` flags. An unchanged column keeps the
    // file's width; a stored width carries its flags as Excel writes them: a width that AutoFit
    // or a number format sized is bestFit too, so it keeps widening when the file is opened
    // again, and one set by hand is customWidth only. A width that is just the sheet's default
    // (which a host geometry stores for every column) is flagged only if it was set by hand or
    // the file flagged it. A `<col>` without a width keeps its flags.
    let width_and_flags = |i: &LineInfo| -> (Option<f64>, bool, bool) {
        let width = match i.saved_col_flags {
            Some(_) if i.size.is_none_or(|size| i.saved_size.is_some_and(|raw| sheet.geometry.stored_width_px(raw) == size)) => i.saved_size,
            _ => i.size.map(|px| stored_width(geometry, px)),
        };
        if let (None, Some((custom, best_fit))) = (width, i.saved_col_flags) {
            return (None, custom, best_fit);
        }
        let stored = i.size.is_some_and(|px| i.custom || px != sheet.default_col_width || i.saved_col_flags.is_some_and(|(custom, _)| custom));
        (width, stored, stored && !i.custom)
    };
    let mut cols = Vec::new();
    let mut run: Option<(u32, u32, LineInfo)> = None;
    let push_run = |cols: &mut Vec<CraftColumn>, (a, b, i): (u32, u32, LineInfo)| {
        let (width, custom_width, best_fit) = width_and_flags(&i);
        cols.push(CraftColumn {
            first: a,
            last: b,
            width,
            custom_width,
            best_fit,
            hidden: i.hidden,
            outline_level: i.outline.min(7),
            collapsed: i.collapsed,
            style: i.style.map(sid),
        });
    };
    // Columns that write the same `<col>` form one run, whatever flags the file gave them.
    let same_col = |a: &LineInfo, b: &LineInfo| {
        LineInfo { saved_col_flags: None, ..*a } == LineInfo { saved_col_flags: None, ..*b } && width_and_flags(a) == width_and_flags(b)
    };
    for (&c, info) in &sheet.cols {
        if c >= gridcraft_core::MAX_COLS {
            continue;
        }
        run = match run {
            Some((a, b, i)) if b + 1 == c && same_col(&i, info) => Some((a, c, i)),
            Some(r) => {
                push_run(&mut cols, r);
                Some((c, c, *info))
            }
            None => Some((c, c, *info)),
        };
    }
    if let Some(r) = run {
        push_run(&mut cols, r);
    }
    cols
}

fn build_craft_view(sheet: &Sheet, selected: bool) -> CraftSheetView {
    let active = sheet.view_active.a1();
    let (pane, selections) = match sheet.freeze {
        Some((rows, cols)) if rows > 0 || cols > 0 => {
            let pane_name = match (rows > 0, cols > 0) {
                (true, true) => "bottomRight",
                (true, false) => "bottomLeft",
                _ => "topRight",
            };
            let top_left = CellRef::new(
                rows.max(sheet.view_top_left.row).min(gridcraft_core::MAX_ROWS - 1),
                cols.max(sheet.view_top_left.col).min(gridcraft_core::MAX_COLS - 1),
            )
            .a1();
            let pane = CraftPane {
                x_split: (cols > 0).then_some(f64::from(cols)),
                y_split: (rows > 0).then_some(f64::from(rows)),
                top_left_cell: Some(top_left),
                active_pane: Some(pane_name.to_string()),
                state: Some("frozen".to_string()),
                ..Default::default()
            };
            let sel = CraftSelection {
                pane: Some(pane_name.to_string()),
                active_cell: Some(active),
                active_cell_id: None,
                sqref: vec![to_craft_range(RangeRef::cell(sheet.view_active))],
                ..Default::default()
            };
            (Some(pane), vec![sel])
        }
        _ => {
            let sel = CraftSelection {
                pane: None,
                active_cell: Some(active),
                active_cell_id: None,
                sqref: vec![to_craft_range(RangeRef::cell(sheet.view_active))],
                ..Default::default()
            };
            (None, vec![sel])
        }
    };
    CraftSheetView {
        tab_selected: selected.then_some(true),
        show_grid_lines: (!sheet.show_gridlines).then_some(false),
        show_row_col_headers: (!sheet.show_headings).then_some(false),
        show_formulas: sheet.show_formulas.then_some(true),
        show_zeros: (!sheet.show_zeros).then_some(false),
        show_outline_symbols: (!sheet.show_outline_symbols).then_some(false),
        right_to_left: sheet.right_to_left.then_some(true),
        view: (sheet.view != gridcraft_model::SheetViewMode::Normal).then(|| sheet.view.as_str().to_string()),
        zoom_scale: (sheet.zoom != 100 && (10..=400).contains(&sheet.zoom)).then_some(u32::from(sheet.zoom)),
        zoom_scale_normal: (sheet.zoom != 100 && (10..=400).contains(&sheet.zoom)).then_some(u32::from(sheet.zoom)),
        top_left_cell: (sheet.view_top_left != CellRef::default() && sheet.freeze.is_none()).then(|| sheet.view_top_left.a1()),
        workbook_view_id: Some(0),
        pane,
        selections,
        ..Default::default()
    }
}

struct CellWriteInput<'a> {
    reference: CellRef,
    cell: Option<&'a Cell>,
    picture: Option<&'a std::sync::Arc<gridcraft_model::CellPicture>>,
    header_override: Option<&'a String>,
    spill_value: Option<&'a Value>,
    dynamic_range: Option<&'a RangeRef>,
    /// The `vm` of value metadata kept by pass-through, for a cell that still holds its value.
    value_meta: Option<u32>,
    /// The reading (furigana) a text cell names instead of its text's first ([`named_reading`]).
    reading: Option<Option<u32>>,
}

/// The reading a text cell names (`Sheet::phonetic_cells`: an index into `Workbook::phonetics`,
/// or none), while the cell still holds that reading's text and it is not the text's first one.
fn named_reading(wb: &Workbook, sheet: &Sheet, at: CellRef, cell: Option<&Cell>) -> Option<Option<u32>> {
    let named = *sheet.phonetic_cells.get(&at)?;
    let cell = cell.filter(|c| c.formula.is_none())?;
    let Value::Text(t) = &cell.value else { return None };
    // A text without any reading has only plain entries.
    let first = wb.phonetics.iter().position(|p| p.text == **t)?;
    match named {
        None => Some(None),
        Some(i) if i as usize != first && wb.phonetics.get(i as usize).is_some_and(|p| p.text == **t) => Some(Some(i)),
        Some(_) => None,
    }
}

fn write_sheet_extensions(s: &mut String, sheet: &Sheet, x14_cf: &str, slicers: &[crate::slicer::SheetSlicer], out: &mut Out, rels: &mut Rels) {
    // Excel 2010 extensions: conditional formats, sparklines, PivotTable slicers, then Excel 2013's
    // table slicers, then kept extensions.
    let slicer_ext = crate::slicer::SlicerWriter::sheet_part(sheet, slicers, out, rels);
    let has_kept_exts = !crate::extlist::sheet_exts(sheet, "").is_empty();
    let has_exts = !x14_cf.is_empty() || !sheet.sparklines.is_empty() || !slicer_ext.is_empty() || has_kept_exts;
    if has_exts {
        s.push_str("<extLst>");
    }
    let exts_start = s.len();
    if !x14_cf.is_empty() {
        let _ = write!(s, "<ext uri=\"{EXT_X14_CF}\" xmlns:x14=\"{X14_NS}\"><x14:conditionalFormattings>{x14_cf}</x14:conditionalFormattings></ext>");
    }
    if !sheet.sparklines.is_empty() {
        let _ = write!(s, "<ext uri=\"{{05C60535-1F16-4fd2-B633-F4F36F0B64E0}}\" xmlns:x14=\"{X14_NS}\"><x14:sparklineGroups xmlns:xm=\"{XM_NS}\">");
        for sp in &sheet.sparklines {
            let ty = match sp.kind {
                SparklineKind::Line => "",
                SparklineKind::Column => " type=\"column\"",
                SparklineKind::WinLoss => " type=\"stacked\"",
            };
            let src = if sp.source.contains('!') || crate::chart::is_structured_ref(&sp.source) {
                std::borrow::Cow::Borrowed(sp.source.as_str())
            } else {
                std::borrow::Cow::Owned(format!("{}!{}", gridcraft_formula::quote_sheet(&sheet.name), sp.source))
            };
            let color = if sp.color == Color::Auto { "rgb=\"FF376092\"".to_string() } else { color_attrs(&sp.color) };
            let _ = write!(
                s,
                "<x14:sparklineGroup displayEmptyCellsAs=\"gap\"{ty}{}><x14:colorSeries {color}/><x14:sparklines><x14:sparkline><xm:f>{}</xm:f><xm:sqref>{}</xm:sqref></x14:sparkline></x14:sparklines></x14:sparklineGroup>",
                if sp.markers { " markers=\"1\"" } else { "" },
                esc(&src),
                sp.cell.a1()
            );
        }
        s.push_str("</x14:sparklineGroups></ext>");
    }
    s.push_str(&slicer_ext);
    if has_exts {
        // Only a sheet with kept extensions scans what was just written for their URIs.
        if has_kept_exts {
            let kept_exts = crate::extlist::sheet_exts(sheet, s.get(exts_start..).unwrap_or(""));
            s.push_str(&kept_exts);
        }
        s.push_str("</extLst>");
    }
}

/// The VML shape numbering of a sheet with notes or controls: the `o:idmap` blocks its legacy
/// drawing takes and the number its shapes count from (the notes first, then the controls).
struct LegacyIds {
    idmap: String,
    base: u64,
}

impl LegacyIds {
    /// The VML shape number of the sheet's control `i`, which its DrawingML stand-in names too.
    fn control(&self, sheet: &Sheet, i: usize) -> u64 {
        self.base.saturating_add(1).saturating_add(sheet.comments.len() as u64).saturating_add(i as u64)
    }
}

/// Takes the `o:idmap` blocks for the sheet's notes and controls (none without either).
fn legacy_ids(sheet: &Sheet, kept: &crate::passthrough::SheetKept<'_>, out: &mut Out) -> Option<LegacyIds> {
    if sheet.comments.is_empty() && sheet.controls.is_empty() {
        return None;
    }
    let shapes = (sheet.comments.len() as u64).saturating_add(sheet.controls.len() as u64);
    let blocks = u32::try_from(shapes.saturating_add(1).div_ceil(1024)).unwrap_or(u32::MAX).max(1);
    let first = out.kept.notes_block(out.vml_block.saturating_add(1));
    out.vml_block = out.vml_block.saturating_add(blocks);
    let idmap = (first..first.saturating_add(blocks)).map(|b| b.to_string()).collect::<Vec<_>>().join(",");
    let base = kept.shape_base().unwrap_or(u64::from(first) * 1024);
    Some(LegacyIds { idmap, base })
}

fn write_sheet_notes_frag(
    sheet: &Sheet,
    si: usize,
    kept: &crate::passthrough::SheetKept<'_>,
    legacy: Option<LegacyIds>,
    rels: &mut Rels,
    out: &mut Out,
) -> (Option<String>, String) {
    let mut controls = String::new();
    let legacy_rid = if let Some(legacy) = legacy {
        let mut control_shapes = String::new();
        if !sheet.controls.is_empty() {
            let w = crate::controls::write(out, rels, sheet, |i| legacy.control(sheet, i));
            controls = w.entries;
            control_shapes = format!("{}{}", crate::controls::SHAPETYPE, w.vml);
        }
        if !sheet.comments.is_empty() {
            out.comments += 1;
            let n = out.comments;
            let ids = thread_ids(sheet, si);
            out.part(&format!("xl/comments{n}.xml"), Some(CT_COMMENTS), comments_xml(sheet, &ids).into_bytes());
            if !ids.is_empty() {
                let x = threaded_xml(sheet, &ids, &mut out.persons);
                out.part(&format!("xl/threadedComments/threadedComment{n}.xml"), Some(CT_THREADED_COMMENTS), x.into_bytes());
                rels.add_uri(REL_THREADED_COMMENT, &format!("../threadedComments/threadedComment{n}.xml"));
            }
            rels.add("comments", &format!("../comments{n}.xml"));
        }
        out.vmls += 1;
        let v = out.vmls;
        out.default_type("vml", "application/vnd.openxmlformats-officedocument.vmlDrawing");
        let vml_name = format!("xl/drawings/vmlDrawing{v}.vml");
        let vml = kept.notes_vml(vml_xml(sheet, &legacy.idmap, legacy.base, &control_shapes), &vml_name, out);
        out.part(&vml_name, None, vml.into_bytes());
        Some(rels.add("vmlDrawing", &format!("../drawings/vmlDrawing{v}.vml")))
    } else {
        kept.lone_vml(rels, out)
    };
    (legacy_rid, kept.elements(&controls))
}

/// The sheet's drawing part: its pictures, charts, shapes and slicers, and a hidden stand-in for
/// each form control (`legacy` numbers their VML shapes), as Excel writes them.
fn write_sheet_drawing_rel(
    wb: &Workbook,
    si: usize,
    kept: &crate::passthrough::SheetKept<'_>,
    legacy: Option<&LegacyIds>,
    rels: &mut Rels,
    out: &mut Out,
) -> Option<String> {
    let sheet = wb.sheet(si)?;
    use gridcraft_model::{DrawingRef, Image, Shape};
    let slicers = out.slicers.on_sheet(si);
    let drawn = kept.drawing();
    use crate::passthrough::ChartSave;
    // Chart annotations a chart writes with itself are left out of the sheet drawing.
    let mut mirrored = std::collections::HashSet::new();
    let charts = crate::passthrough::chart_save_plans(wb, sheet, &mut mirrored);
    let images: Vec<&Image> = sheet.images.iter().filter(|i| !i.data.is_empty() && !mirrored.contains(&DrawingRef::Image(i.id))).collect();
    let shapes: Vec<&Shape> = sheet.shapes.iter().filter(|s| !mirrored.contains(&DrawingRef::Shape(s.id))).collect();
    let stand_ins: Vec<(&FormControl, u64)> = match legacy {
        Some(ids) => sheet.controls.iter().enumerate().map(|(i, c)| (c, ids.control(sheet, i))).collect(),
        None => Vec::new(),
    };
    let mut drels = Rels::default();
    let proxies = kept.control_proxies(&mut drels);
    if images.is_empty() && sheet.charts.is_empty() && shapes.is_empty() && slicers.is_empty() && stand_ins.is_empty() && proxies.is_empty() {
        return None;
    }
    out.drawings += 1;
    let dn = out.drawings;
    let mut objs: Vec<Obj<'_>> = vec![];
    for &img in &images {
        let ext = image_ext(&img.mime, &img.data);
        out.default_type(ext, image_content_type(ext));
        let file = out.image_part(ext, &img.data);
        let rid = drels.add("image", &format!("../media/{file}"));
        objs.push(Obj::Image(img, rid));
    }
    // No plans when the sheet keeps nothing: every chart then writes from the model with its mirrors.
    let mut plans = charts.into_iter();
    for ch in &sheet.charts {
        let plan = plans.next().unwrap_or(ChartSave::Mirrors);
        out.charts += 1;
        let rid = if crate::chartex::is_chartex(ch.kind) {
            let n = out.charts;
            let xml = crate::chartex::write_chartex(wb, si, ch, &mut out.chart_names);
            out.part(&format!("xl/charts/chartEx{n}.xml"), Some(crate::chartex::CT_CHARTEX), xml.into_bytes());
            out.part(&format!("xl/charts/style{n}.xml"), Some(crate::chartex::CT_STYLE), crate::chartex::style_xml().into_bytes());
            out.part(&format!("xl/charts/colors{n}.xml"), Some(crate::chartex::CT_COLORS), crate::chartex::colors_xml().into_bytes());
            let mut crels = Rels::default();
            crels.add_uri(crate::chartex::REL_STYLE, &format!("style{n}.xml"));
            crels.add_uri(crate::chartex::REL_COLORS, &format!("colors{n}.xml"));
            out.part(&format!("xl/charts/_rels/chartEx{n}.xml.rels"), None, crels.xml().into_bytes());
            drels.add_uri(crate::chartex::REL_CHARTEX, &format!("../charts/chartEx{n}.xml"))
        } else {
            let name = format!("xl/charts/chart{}.xml", out.charts);
            match plan {
                ChartSave::Kept(k) => crate::passthrough::write_kept_chart(k, &name, &mut drels, out),
                plan => {
                    let user_shapes = match plan {
                        ChartSave::Graft { root, closure } => Some(crate::passthrough::attach_chart_user_shapes(root, &closure, out.charts, out)),
                        _ => None,
                    };
                    let xml = crate::chart::write_chart_with_user_shapes(wb, si, ch, &out.book_name, user_shapes.as_deref());
                    out.part(&name, Some(CT_CHART), xml.into_bytes());
                    drels.add("chart", &format!("../charts/chart{}.xml", out.charts))
                }
            }
        };
        objs.push(Obj::Chart(ch, rid));
    }
    for sh in shapes {
        objs.push(Obj::Shape(sh));
    }
    for p in &slicers {
        if let Some(sl) = sheet.slicers.get(p.slicer) {
            objs.push(Obj::Slicer(sl, p.name.clone()));
        }
    }
    let objects = objs.iter().filter_map(|o| match o {
        Obj::Image(i, _) => Some(DrawingRef::Image(i.id)),
        Obj::Shape(s) => Some(DrawingRef::Shape(s.id)),
        _ => None,
    });
    let links = drawn.links(wb, (si, sheet), objects, &mut drels, out);
    out.part(
        &format!("xl/drawings/drawing{dn}.xml"),
        Some(CT_DRAWING),
        crate::drawing::write_drawing(sheet, &wb.theme, &objs, &links, &stand_ins, &proxies).into_bytes(),
    );
    if !drels.is_empty() {
        out.part(&format!("xl/drawings/_rels/drawing{dn}.xml.rels"), None, drels.xml().into_bytes());
    }
    // The relationship ids the pictures were written with (control stand-ins may come first).
    let written = objs.iter().filter_map(|o| match o {
        Obj::Image(i, rid) => Some((i.id, rid.as_str())),
        _ => None,
    });
    crate::passthrough::attach_svg_blips(sheet, dn, written, out);
    Some(rels.add("drawing", &format!("../drawings/drawing{dn}.xml")))
}

fn build_sheet_printing(sheet: &Sheet, print: &mut CraftPrint, kept: &crate::passthrough::SheetKept<'_>) {
    let p = &sheet.print;
    if p.gridlines || p.headings || p.center_h || p.center_v {
        print.options = Some(CraftPrintOptions {
            horizontal_centered: p.center_h.then_some(true),
            vertical_centered: p.center_v.then_some(true),
            headings: p.headings.then_some(true),
            grid_lines: p.gridlines.then_some(true),
            grid_lines_set: None,
            ..Default::default()
        });
    }
    // Margins are kept as f32: widen each through its shortest decimal, so 0.3 is written as
    // `0.3`, as Excel writes it, and not as `0.30000001192092896` (#1609).
    let m = |i: usize| {
        let inches = p.margins.get(i).copied().unwrap_or(0.75);
        inches.to_string().parse::<f64>().unwrap_or(f64::from(inches))
    };
    print.margins = Some(CraftMargins {
        left: Some(m(0)),
        right: Some(m(1)),
        top: Some(m(2)),
        bottom: Some(m(3)),
        header: Some(m(4)),
        footer: Some(m(5)),
        ..Default::default()
    });
    print.setup = Some(CraftPageSetup {
        paper_size: paper_id(&p.paper).filter(|&id| id != 1),
        scale: (p.scale != 100 && (10..=400).contains(&p.scale)).then_some(u32::from(p.scale)),
        first_page_number: p.first_page_number,
        fit_to_width: p.fit_to.map(|(w, _)| u32::from(w)).filter(|&w| w != 1),
        fit_to_height: p.fit_to.map(|(_, h)| u32::from(h)).filter(|&h| h != 1),
        page_order: (p.page_order != PageOrder::DownThenOver).then(|| p.page_order.id().to_string()),
        orientation: Some(if p.orientation == Orientation::Landscape { "landscape" } else { "portrait" }.to_string()),
        use_first_page_number: p.first_page_number.map(|_| true),
        rel_id: kept.page_setup_id().map(str::to_string),
        ..Default::default()
    });
    if p.has_header_footer() || p.different_first || p.different_odd_even || !p.hf_scale_with_doc || !p.hf_align_with_margins {
        let opt = |t: &String| (!t.is_empty()).then(|| t.clone());
        print.header_footer = Some(CraftHf {
            different_odd_even: p.different_odd_even.then_some(true),
            different_first: p.different_first.then_some(true),
            scale_with_doc: (!p.hf_scale_with_doc).then_some(false),
            align_with_margins: (!p.hf_align_with_margins).then_some(false),
            odd_header: opt(&p.header),
            odd_footer: opt(&p.footer),
            even_header: opt(&p.even_header),
            even_footer: opt(&p.even_footer),
            first_header: opt(&p.first_header),
            first_footer: opt(&p.first_footer),
            ..Default::default()
        });
    }
    let to_breaks = |list: &[u32], max: u32| {
        list.iter().copied().filter(|&b| b > 0).map(|id| CraftBreak { id: Some(id), max: Some(max), man: Some(true), ..Default::default() }).collect()
    };
    print.row_breaks = to_breaks(&p.row_breaks, 16383);
    print.col_breaks = to_breaks(&p.col_breaks, 1048575);
}

fn build_sheet_rules(sheet: &Sheet, features: &mut CraftFeatures, rels: &mut Rels, out: &mut Out) -> (String, String) {
    let mut frag = String::new();
    if let Some(p) = &sheet.protection {
        frag.push_str("<sheetProtection sheet=\"1\" objects=\"1\" scenarios=\"1\"");
        for (attr, allowed) in [
            ("formatCells", p.format_cells),
            ("formatColumns", p.format_columns),
            ("formatRows", p.format_rows),
            ("insertColumns", p.insert_columns),
            ("insertRows", p.insert_rows),
            ("deleteColumns", p.delete_columns),
            ("deleteRows", p.delete_rows),
            ("sort", p.sort),
            ("autoFilter", p.autofilter),
        ] {
            if allowed {
                let _ = write!(frag, " {attr}=\"0\"");
            }
        }
        if !p.select_locked {
            frag.push_str(" selectLockedCells=\"1\"");
        }
        if !p.select_unlocked {
            frag.push_str(" selectUnlockedCells=\"1\"");
        }
        if let Some(h) = &p.password {
            frag.push_str(&password_attrs(h, ""));
        }
        frag.push_str("/>");
    }
    frag.push_str(&protected_ranges_xml(sheet));
    if let Some(af) = &sheet.autofilter
        && !sheet.tables.iter().any(|t| t.range.intersects(&af.range))
    {
        frag.push_str(&autofilter_xml(af.range, &af.criteria, out));
    }
    let mut order: Vec<usize> = (0..sheet.cond_formats.len()).collect();
    order.sort_by_key(|&i| sheet.cond_formats.get(i).map(|c| c.priority).unwrap_or(0));
    let mut prio = vec![0u32; sheet.cond_formats.len()];
    for (rank, &i) in order.iter().enumerate() {
        if let Some(p) = prio.get_mut(i) {
            *p = rank as u32 + 1;
        }
    }
    let mut x14_cf = String::new();
    for (i, cf) in sheet.cond_formats.iter().enumerate() {
        if cf.ranges.is_empty() {
            continue;
        }
        let priority = prio.get(i).copied().unwrap_or(1);
        let x14 = match x14_cf_rule_xml(cf, &sheet.name, priority, out) {
            Some(rule) => Some(rule),
            None => {
                let xml =
                    format!("<conditionalFormatting sqref=\"{}\">{}</conditionalFormatting>", sqref(&cf.ranges), cf_rule_xml(cf, priority, out));
                if let Some(el) = parse_craft_fragment(&xml) {
                    features.conditional_formats.push(CraftCf::read(&el));
                }
                solid_bar_x14(cf, priority)
            }
        };
        if let Some(rule) = x14 {
            let _ = write!(
                x14_cf,
                "<x14:conditionalFormatting xmlns:xm=\"{XM_NS}\">{rule}<xm:sqref>{}</xm:sqref></x14:conditionalFormatting>",
                sqref(&cf.ranges)
            );
        }
    }
    let vals: Vec<&Validation> = sheet.validations.iter().filter(|v| !v.ranges.is_empty()).collect();
    if !vals.is_empty() {
        let items = vals.iter().filter_map(|v| parse_craft_fragment(&validation_xml(v)).map(|el| craft_xlsx::DataValidation::read(&el))).collect();
        features.validations = Some(CraftDvs { items, ..Default::default() });
    }
    for (c, h) in &sheet.hyperlinks {
        let t = h.target.trim();
        if h.external && t.is_empty() {
            continue;
        }
        // The model's flag alone picks a relationship or a `location`; the text never does.
        let (rel_id, location, display, url) = if h.external {
            let (u, loc) = match t.split_once('#') {
                Some((u, l)) if !u.is_empty() => (u, Some(l.to_string())),
                _ => (t, None),
            };
            let id = rels.add_external("hyperlink", u);
            (Some(id), loc, None, Some(u.to_string()))
        } else {
            let loc = crate::sheet_read::internal_link_location(t);
            (None, Some(loc.clone()), Some(loc), None)
        };
        features.hyperlinks.push(CraftHyperlink {
            rel_id,
            location,
            tooltip: h.tooltip.clone(),
            display,
            range: to_craft_range(RangeRef::cell(*c)),
            url,
            ..Default::default()
        });
    }
    (frag, x14_cf)
}

struct SheetCellData<'a> {
    workbook: &'a Workbook,
    index: usize,
    sheet: &'a Sheet,
    overrides: &'a BTreeMap<CellRef, String>,
    extra: &'a BTreeMap<CellRef, Value>,
    dynamic: &'a BTreeMap<CellRef, RangeRef>,
}

/// A contiguous vertical shared-formula run in one column (`start_row..=end_row`).
struct SharedRun {
    start_row: u32,
    end_row: u32,
    si: u32,
}

/// Valid `(min_dr, max_dr, min_dc, max_dc)` relative-shift bounds for `e` such that no
/// `Expr::Ref` shifts outside `0..MAX_ROWS` × `0..MAX_COLS` (producing `#REF!`), or `None` when
/// `e` contains constructs Excel rejects or mis-rewrites inside `<f t="shared">` (per `[MS-XLSB]`
/// §2.5.98.98 `SharedParsedFormula`: cross-sheet/3D/linked references `PtgRef3d`/`PtgArea3d`,
/// deleted `#REF!` literals, array constants, reference operators `:` / `,` / intersection,
/// sheet-qualified/external names `PtgNameX`, `RTD` (`0x017B`), `LAMBDA`/invocations, structured
/// table references, spill `#` references, or trim-range references).
fn shared_run_bounds(e: &gridcraft_formula::Expr) -> Option<(i64, i64, i64, i64)> {
    use gridcraft_formula::{BinOp, Expr, RefKind, SheetSel, UnOp};
    let mut min_dr = i64::MIN;
    let mut max_dr = i64::MAX;
    let mut min_dc = i64::MIN;
    let mut max_dc = i64::MAX;
    let mut supported = true;
    let clamp_row = |r: u32, abs: bool, min_dr: &mut i64, max_dr: &mut i64| {
        if !abs {
            *min_dr = (*min_dr).max(-(r as i64));
            *max_dr = (*max_dr).min((gridcraft_core::MAX_ROWS - 1) as i64 - r as i64);
        }
    };
    let clamp_col = |c: u32, abs: bool, min_dc: &mut i64, max_dc: &mut i64| {
        if !abs {
            *min_dc = (*min_dc).max(-(c as i64));
            *max_dc = (*max_dc).min((gridcraft_core::MAX_COLS - 1) as i64 - c as i64);
        }
    };
    e.walk(&mut |x| match x {
        Expr::Struct(_)
        | Expr::Unary(UnOp::Spill | UnOp::Trim(_), _)
        | Expr::LinkedRefError(_)
        | Expr::Error(CellError::Ref)
        | Expr::Array(_)
        | Expr::Binary(BinOp::Range | BinOp::Union | BinOp::Intersect, _, _)
        | Expr::Invoke(_, _) => supported = false,
        Expr::Name(n) if n.contains('!') => supported = false,
        Expr::Call(name, _) if name.eq_ignore_ascii_case("LAMBDA") || name.eq_ignore_ascii_case("RTD") => supported = false,
        Expr::Ref(r) => {
            if !matches!(r.sheet, SheetSel::Current) {
                supported = false;
                return;
            }
            match r.kind {
                RefKind::Cell(a) => {
                    clamp_row(a.row, a.row_abs, &mut min_dr, &mut max_dr);
                    clamp_col(a.col, a.col_abs, &mut min_dc, &mut max_dc);
                }
                RefKind::Range(a, b) => {
                    clamp_row(a.row, a.row_abs, &mut min_dr, &mut max_dr);
                    clamp_col(a.col, a.col_abs, &mut min_dc, &mut max_dc);
                    clamp_row(b.row, b.row_abs, &mut min_dr, &mut max_dr);
                    clamp_col(b.col, b.col_abs, &mut min_dc, &mut max_dc);
                }
                RefKind::Rows(r0, a0, r1, a1) => {
                    clamp_row(r0, a0, &mut min_dr, &mut max_dr);
                    clamp_row(r1, a1, &mut min_dr, &mut max_dr);
                }
                RefKind::Cols(c0, a0, c1, a1) => {
                    clamp_col(c0, a0, &mut min_dc, &mut max_dc);
                    clamp_col(c1, a1, &mut min_dc, &mut max_dc);
                }
            }
        }
        _ => {}
    });
    supported.then_some((min_dr, max_dr, min_dc, max_dc))
}

/// Finds contiguous vertical shared-formula runs of length >= 64 that can be emitted as
/// `<f t="shared" ref="..." si="..."/>` without changing any formula's exported spelling or
/// metadata. Assigns `si = 0, 1, 2, ...` strictly monotonically in `(start_row, col)` order so
/// every master appears in ascending `si` document order in `<sheetData>` (required by Excel).
fn collect_shared_runs(data: &SheetCellData<'_>, out: &mut Out) -> HashMap<u32, Vec<SharedRun>> {
    let SheetCellData { workbook, index, sheet, overrides, dynamic, .. } = data;
    if !sheet.cells.iter().any(|(_, c)| c.formula.as_ref().is_some_and(|f| f.is_shared())) {
        return HashMap::new();
    }
    struct OpenRun {
        base_ptr: usize,
        dc: i32,
        dr_minus_row: i64,
        start_row: u32,
        end_row: u32,
    }
    let mut open: HashMap<u32, OpenRun> = HashMap::new();
    let mut finished: Vec<(u32, u32, u32)> = Vec::new();
    let mut bounds_cache: HashMap<usize, Option<(i64, i64, i64, i64)>> = HashMap::new();
    let flush = |col: u32, run: OpenRun, finished: &mut Vec<(u32, u32, u32)>| {
        if run.end_row.saturating_sub(run.start_row) + 1 >= 64 {
            finished.push((col, run.start_row, run.end_row));
        }
    };

    for (pos, cell) in sheet.cells.iter() {
        let eligible = if overrides.contains_key(&pos) || sheet.cell_pictures.contains_key(&pos) || dynamic.contains_key(&pos) {
            None
        } else if let Some(f) = cell.formula.as_ref()
            && f.is_shared()
            && f.array.is_none()
            && !f.dynamic
            && !crate::external::saved_as_read(f)
            && let Some((base, dr, dc)) = f.expr_shifted()
        {
            let base_key = Arc::as_ptr(base) as usize;
            let bounds = *bounds_cache.entry(base_key).or_insert_with(|| shared_run_bounds(base));
            if let Some((min_dr, max_dr, min_dc, max_dc)) = bounds
                && (min_dr..=max_dr).contains(&i64::from(dr))
                && (min_dc..=max_dc).contains(&i64::from(dc))
            {
                match crate::array_formula::export(workbook, *index, pos, f, out.functions, &mut out.names) {
                    crate::array_formula::Export::Unchanged => Some((base_key, dr, dc)),
                    crate::array_formula::Export::LegacyShared(b) => Some((Arc::as_ptr(&b) as usize, dr, dc)),
                    crate::array_formula::Export::Dynamic | crate::array_formula::Export::Legacy(_) => None,
                }
            } else {
                None
            }
        } else {
            None
        };

        if let Some((base_ptr, dr, dc)) = eligible {
            let dr_minus_row = i64::from(dr) - i64::from(pos.row);
            if let Some(cur) = open.get_mut(&pos.col)
                && cur.base_ptr == base_ptr
                && cur.dc == dc
                && cur.dr_minus_row == dr_minus_row
                && cur.end_row + 1 == pos.row
            {
                cur.end_row = pos.row;
                continue;
            }
            if let Some(prev) = open.insert(pos.col, OpenRun { base_ptr, dc, dr_minus_row, start_row: pos.row, end_row: pos.row }) {
                flush(pos.col, prev, &mut finished);
            }
        } else if let Some(prev) = open.remove(&pos.col) {
            flush(pos.col, prev, &mut finished);
        }
    }
    for (col, run) in open {
        flush(col, run, &mut finished);
    }
    finished.sort_unstable_by_key(|&(col, start_row, _)| (start_row, col));
    let mut runs_by_col: HashMap<u32, Vec<SharedRun>> = HashMap::new();
    for (idx, (col, start_row, end_row)) in finished.into_iter().enumerate() {
        runs_by_col.entry(col).or_default().push(SharedRun { start_row, end_row, si: idx as u32 });
    }
    runs_by_col
}

#[derive(Clone, Copy)]
enum SharedEmit {
    None,
    Master { si: u32, end_row: u32 },
    Dependent { si: u32 },
}

/// Streams `<sheetData>…</sheetData>` directly into a single `String` without intermediate
/// `CraftRow`/`CraftCell` structs or per-cell `String` allocations, returning `(xml, used_bounds)`.
fn stream_sheet_data(data: SheetCellData<'_>, sid: &dyn Fn(gridcraft_model::StyleId) -> u32, out: &mut Out) -> (String, Option<(CellRef, CellRef)>) {
    let SheetCellData { workbook, index, sheet, overrides, extra, dynamic } = data;
    let shared_runs = collect_shared_runs(&data, out);
    let mut run_cursors: HashMap<u32, usize> = HashMap::with_capacity(shared_runs.len());
    let metas = crate::passthrough::cell_meta_index(sheet, out);

    let mut aux_keys: BTreeSet<CellRef> = BTreeSet::new();
    aux_keys.extend(sheet.cell_pictures.keys().copied());
    aux_keys.extend(extra.keys().copied());
    aux_keys.extend(overrides.keys().copied());

    let mut main_it = sheet.cells.iter().peekable();
    let mut aux_it = aux_keys.into_iter().peekable();
    let mut row_infos = sheet.rows.iter().peekable();

    let mut s = String::with_capacity(sheet.cells.len().saturating_mul(28).min(64 * 1024 * 1024) + 64);
    s.push_str("<sheetData>");

    let mut min_row = u32::MAX;
    let mut min_col = u32::MAX;
    let mut max_row = 0u32;
    let mut max_col = 0u32;
    let mut any_cell = false;
    let mut row_cells: Vec<(CellRef, Option<&Cell>)> = Vec::with_capacity(64);

    loop {
        let main_r = main_it.peek().map(|(c, _)| c.row);
        let aux_r = aux_it.peek().map(|c| c.row);
        let info_r = row_infos.peek().map(|(r, _)| **r);
        let next_cell_r = match (main_r, aux_r) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let row = match (next_cell_r, info_r) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => break,
        };
        let info = match row_infos.next_if(|(r, _)| **r == row) {
            Some((_, info)) => *info,
            None => LineInfo::default(),
        };
        row_cells.clear();
        loop {
            let m = main_it.peek().filter(|(c, _)| c.row == row).map(|(c, _)| *c);
            let a = aux_it.peek().filter(|c| c.row == row).copied();
            match (m, a) {
                (Some(mc), Some(ac)) => match mc.cmp(&ac) {
                    std::cmp::Ordering::Less => {
                        if let Some((c, cell)) = main_it.next() {
                            row_cells.push((c, Some(cell)));
                        }
                    }
                    std::cmp::Ordering::Greater => {
                        if let Some(c) = aux_it.next() {
                            row_cells.push((c, None));
                        }
                    }
                    std::cmp::Ordering::Equal => {
                        let _ = aux_it.next();
                        if let Some((c, cell)) = main_it.next() {
                            row_cells.push((c, Some(cell)));
                        }
                    }
                },
                (Some(_), None) => {
                    if let Some((c, cell)) = main_it.next() {
                        row_cells.push((c, Some(cell)));
                    }
                }
                (None, Some(_)) => {
                    if let Some(c) = aux_it.next() {
                        row_cells.push((c, None));
                    }
                }
                (None, None) => break,
            }
        }
        if row_cells.is_empty() && info == LineInfo::default() {
            continue;
        }
        s.push_str("<row r=\"");
        push_u64(&mut s, u64::from(row) + 1);
        s.push('"');
        if let Some(st) = info.style {
            s.push_str(" s=\"");
            push_u64(&mut s, u64::from(sid(st)));
            s.push_str("\" customFormat=\"1\"");
        }
        // The file's own height while the row is as it was read; otherwise a measured height only
        // where Excel saves one (legacy geometry, or a height set by hand).
        let height = info
            .saved_size
            .or_else(|| if sheet.geometry == gridcraft_model::Geometry::Legacy || info.custom { info.size.map(px_to_pt) } else { None });
        if let Some(ht) = height {
            s.push_str(" ht=\"");
            push_num(&mut s, ht);
            s.push('"');
        }
        if info.custom {
            s.push_str(" customHeight=\"1\"");
        }
        if info.hidden {
            s.push_str(" hidden=\"1\"");
        }
        let outline = info.outline.min(7);
        if outline > 0 {
            s.push_str(" outlineLevel=\"");
            push_u64(&mut s, u64::from(outline));
            s.push('"');
        }
        if info.collapsed {
            s.push_str(" collapsed=\"1\"");
        }
        if info.thick_edges.0 {
            s.push_str(" thickTop=\"1\"");
        }
        if info.thick_edges.1 {
            s.push_str(" thickBot=\"1\"");
        }
        if row_cells.is_empty() {
            s.push_str("/>");
            continue;
        }
        s.push('>');
        for &(cell_ref, cell) in &row_cells {
            any_cell = true;
            min_row = min_row.min(cell_ref.row);
            min_col = min_col.min(cell_ref.col);
            max_row = max_row.max(cell_ref.row);
            max_col = max_col.max(cell_ref.col);

            let shared_emit = if let Some(col_runs) = shared_runs.get(&cell_ref.col) {
                let cursor = run_cursors.entry(cell_ref.col).or_insert(0);
                while let Some(r) = col_runs.get(*cursor)
                    && r.end_row < cell_ref.row
                {
                    *cursor += 1;
                }
                match col_runs.get(*cursor) {
                    Some(r) if r.start_row == cell_ref.row => SharedEmit::Master { si: r.si, end_row: r.end_row },
                    Some(r) if r.start_row < cell_ref.row && cell_ref.row <= r.end_row => SharedEmit::Dependent { si: r.si },
                    _ => SharedEmit::None,
                }
            } else {
                SharedEmit::None
            };

            write_streamed_cell(
                &mut s,
                workbook,
                index,
                CellWriteInput {
                    reference: cell_ref,
                    cell,
                    picture: sheet.cell_pictures.get(&cell_ref),
                    header_override: overrides.get(&cell_ref),
                    spill_value: extra.get(&cell_ref),
                    dynamic_range: dynamic.get(&cell_ref),
                    value_meta: metas.get(&cell_ref).filter(|m| crate::passthrough::cell_meta_holds(m, cell)).map(|m| m.vm),
                    reading: if sheet.phonetic_cells.is_empty() { None } else { named_reading(workbook, sheet, cell_ref, cell) },
                },
                shared_emit,
                sid,
                out,
            );
        }
        s.push_str("</row>");
    }
    s.push_str("</sheetData>");
    let used = any_cell.then(|| (CellRef::new(min_row, min_col), CellRef::new(max_row, max_col)));
    (s, used)
}

#[allow(clippy::too_many_arguments)]
fn write_streamed_cell(
    s: &mut String,
    workbook: &Workbook,
    index: usize,
    input: CellWriteInput<'_>,
    shared_emit: SharedEmit,
    sid: &dyn Fn(gridcraft_model::StyleId) -> u32,
    out: &mut Out,
) {
    let CellWriteInput { reference: c, cell, picture, header_override: ov, spill_value: extra, dynamic_range, value_meta, reading } = input;
    let style = cell.map(|x| sid(x.style)).unwrap_or(0);

    if let Some(picture) = picture {
        let vm = out.cell_pictures.index(picture);
        s.push_str("<c r=\"");
        push_a1(s, c);
        s.push('"');
        if style != 0 {
            s.push_str(" s=\"");
            push_u64(s, u64::from(style));
            s.push('"');
        }
        s.push_str(" t=\"e\" vm=\"");
        push_u64(s, u64::from(vm));
        s.push_str("\"><v>#VALUE!</v></c>");
        return;
    }

    if ov.is_none()
        && extra.is_none()
        && let Some(cell_ref) = cell
        && cell_ref.formula.is_none()
        && cell_ref.py.is_none()
    {
        s.push_str("<c r=\"");
        push_a1(s, c);
        s.push('"');
        if style != 0 {
            s.push_str(" s=\"");
            push_u64(s, u64::from(style));
            s.push('"');
        }
        match &cell_ref.value {
            Value::Error(CellError::Circ) => {
                push_vm(s, value_meta);
                s.push_str("><v>0</v></c>");
            }
            Value::Error(CellError::Spill | CellError::Calc | CellError::Python | CellError::Busy | CellError::Timeout | CellError::Blocked) => {
                write_plain_value_tail(s, &Value::Error(CellError::Value), false, value_meta, None, out);
            }
            Value::Array(a) => {
                let v = a.get(0, 0).cloned().unwrap_or(Value::Empty);
                write_plain_value_tail(s, &v, false, value_meta, None, out);
            }
            v => write_plain_value_tail(s, v, false, value_meta, reading, out),
        }
        return;
    }

    let formula = if ov.is_some() { None } else { cell.and_then(|x| x.formula.as_ref()) };
    let py_text = if ov.is_some() || formula.is_some() {
        None
    } else {
        cell.and_then(|x| x.py.as_ref()).map(|p| {
            out.python_scripts.push(p.code.clone());
            crate::python_part::file_formula(out.python_scripts.len().saturating_sub(1), p.object)
        })
    };

    let value: Value = match (ov, extra) {
        (Some(t), _) => Value::text(t.as_str()),
        (None, Some(v)) => v.clone(),
        _ => cell.map(|x| x.value.scalar()).unwrap_or(Value::Empty),
    };
    let value = match value {
        Value::Array(a) => a.get(0, 0).cloned().unwrap_or(Value::Empty),
        v => v,
    };
    let value = match value {
        Value::Error(CellError::Circ) => Value::Number(0.0),
        Value::Error(CellError::Spill | CellError::Calc | CellError::Python | CellError::Busy | CellError::Timeout | CellError::Blocked) => {
            Value::Error(CellError::Value)
        }
        v => v,
    };

    if formula.is_none() && py_text.is_none() {
        s.push_str("<c r=\"");
        push_a1(s, c);
        s.push('"');
        if style != 0 {
            s.push_str(" s=\"");
            push_u64(s, u64::from(style));
            s.push('"');
        }
        write_plain_value_tail(s, &value, extra.is_some(), value_meta, reading.filter(|_| ov.is_none()), out);
        return;
    }

    if let SharedEmit::Dependent { si } = shared_emit {
        s.push_str("<c r=\"");
        push_a1(s, c);
        s.push('"');
        if style != 0 {
            s.push_str(" s=\"");
            push_u64(s, u64::from(style));
            s.push('"');
        }
        if let Some(t) = formula_value_type(&value) {
            s.push_str(" t=\"");
            s.push_str(t);
            s.push('"');
        }
        push_vm(s, value_meta);
        s.push_str("><f t=\"shared\" si=\"");
        push_u64(s, u64::from(si));
        s.push_str("\"/>");
        write_formula_cached_v(s, &value, out);
        s.push_str("</c>");
        return;
    }

    let mut legacy_override: Option<String> = None;
    let array_range = dynamic_range.copied().or_else(|| {
        let f = formula.filter(|f| f.array.is_none())?;
        if f.dynamic {
            return Some(RangeRef::cell(c));
        }
        match crate::array_formula::export(workbook, index, c, f, out.functions, &mut out.names) {
            crate::array_formula::Export::Unchanged => None,
            crate::array_formula::Export::Dynamic => Some(RangeRef::cell(c)),
            crate::array_formula::Export::Legacy(e) => {
                legacy_override = Some(crate::fmla::to_file_shifted(&e, 0, 0));
                None
            }
            crate::array_formula::Export::LegacyShared(base) => {
                let (_, dr, dc) = f.expr_shifted().unwrap_or((&base, 0, 0));
                legacy_override = Some(crate::fmla::to_file_shifted(&base, dr, dc));
                None
            }
        }
    });

    let own = formula.filter(|f| f.dynamic && f.array.is_none()).map(|_| RangeRef::cell(c));
    let dynamic_eff = array_range.as_ref().or(own.as_ref());
    let cell_metadata = if dynamic_eff.is_some() && (formula.is_some_and(|f| f.array.is_none()) || py_text.is_some()) {
        out.dynamic_arrays = true;
        Some(1u32)
    } else {
        None
    };

    let text = legacy_override.or_else(|| formula.map(|f| crate::fmla::to_file(f))).or(py_text).unwrap_or_default();

    s.push_str("<c r=\"");
    push_a1(s, c);
    s.push('"');
    if style != 0 {
        s.push_str(" s=\"");
        push_u64(s, u64::from(style));
        s.push('"');
    }
    if let Some(t) = formula_value_type(&value) {
        s.push_str(" t=\"");
        s.push_str(t);
        s.push('"');
    }
    if let Some(cm) = cell_metadata {
        s.push_str(" cm=\"");
        push_u64(s, u64::from(cm));
        s.push('"');
    }
    push_vm(s, value_meta);
    s.push_str("><f");
    match (shared_emit, formula.and_then(|f| f.array), dynamic_eff) {
        (SharedEmit::Master { si, end_row }, None, None) => {
            s.push_str(" t=\"shared\" ref=\"");
            push_a1(s, c);
            s.push(':');
            push_a1(s, CellRef::new(end_row, c.col));
            s.push_str("\" si=\"");
            push_u64(s, u64::from(si));
            s.push('"');
        }
        (_, Some(r), _) => {
            s.push_str(" t=\"array\" ref=\"");
            push_range_a1(s, r);
            s.push('"');
        }
        (_, None, Some(r)) => {
            s.push_str(" t=\"array\" ref=\"");
            push_range_a1(s, *r);
            s.push('"');
        }
        _ => {}
    }
    if text.is_empty() {
        s.push_str("/>");
    } else {
        s.push('>');
        push_esc(s, &text);
        s.push_str("</f>");
    }
    write_formula_cached_v(s, &value, out);
    s.push_str("</c>");
}

#[inline]
fn push_range_a1(s: &mut String, r: RangeRef) {
    push_a1(s, r.start);
    if r.start != r.end {
        s.push(':');
        push_a1(s, r.end);
    }
}

#[inline]
fn formula_value_type(value: &Value) -> Option<&'static str> {
    match value {
        Value::Text(_) => Some("str"),
        Value::Bool(_) => Some("b"),
        Value::Error(_) => Some("e"),
        _ => None,
    }
}

#[inline]
fn write_formula_cached_v(s: &mut String, value: &Value, out: &mut Out) {
    match value {
        Value::Number(n) => {
            s.push_str("<v>");
            push_num(s, *n);
            s.push_str("</v>");
        }
        Value::Text(t) => {
            s.push_str("<v>");
            let enc = encode_xstring(t);
            push_esc(s, &enc);
            s.push_str("</v>");
        }
        Value::Bool(b) => s.push_str(if *b { "<v>1</v>" } else { "<v>0</v>" }),
        Value::Error(e) => {
            s.push_str("<v>");
            s.push_str(e.as_str());
            s.push_str("</v>");
        }
        _ => {
            out.needs_calc = true;
        }
    }
}

/// ` vm="…"`: the kept value metadata of a cell, after its `t` and `cm` as Excel writes them.
#[inline]
fn push_vm(s: &mut String, vm: Option<u32>) {
    if let Some(vm) = vm {
        s.push_str(" vm=\"");
        push_u64(s, u64::from(vm));
        s.push('"');
    }
}

/// The rest of a cell with no formula after its `r` and `s`: its type, kept value metadata (`vm`)
/// and value. `reading` is the furigana a text cell names ([`named_reading`]).
#[inline]
fn write_plain_value_tail(s: &mut String, value: &Value, inline_str_as_str: bool, vm: Option<u32>, reading: Option<Option<u32>>, out: &mut Out) {
    match value {
        Value::Empty | Value::Array(_) => {
            push_vm(s, vm);
            s.push_str("/>");
        }
        Value::Number(n) => {
            push_vm(s, vm);
            s.push_str("><v>");
            push_num(s, *n);
            s.push_str("</v></c>");
        }
        Value::Text(t) if inline_str_as_str => {
            s.push_str(" t=\"str\"");
            push_vm(s, vm);
            s.push_str("><v>");
            let enc = encode_xstring(t);
            push_esc(s, &enc);
            s.push_str("</v></c>");
        }
        Value::Text(t) => {
            let idx = match reading {
                Some(r) => out.sst.index_with(t, r),
                None => out.sst.index(t),
            };
            s.push_str(" t=\"s\"");
            push_vm(s, vm);
            s.push_str("><v>");
            push_u64(s, u64::from(idx));
            s.push_str("</v></c>");
        }
        Value::Bool(b) => {
            s.push_str(" t=\"b\"");
            push_vm(s, vm);
            s.push_str(if *b { "><v>1</v></c>" } else { "><v>0</v></c>" });
        }
        Value::Error(e) => {
            s.push_str(" t=\"e\"");
            push_vm(s, vm);
            s.push_str("><v>");
            s.push_str(e.as_str());
            s.push_str("</v></c>");
        }
    }
}
const X14_NS: &str = "http://schemas.microsoft.com/office/spreadsheetml/2009/9/main";
const XM_NS: &str = "http://schemas.microsoft.com/office/excel/2006/main";

/// The box a note made here gets: two columns right of its cell, four rows tall, 108 x 59.25 pt.
/// A box read from a file that equals it is kept as `None`, so a note's model survives a save.
pub(crate) const DEFAULT_NOTE_BOX: NoteBox = NoteBox {
    from: NoteCorner { col: 1, col_px: 15, row: 0, row_px: 2 },
    to: NoteCorner { col: 3, col_px: 15, row: 4, row_px: 16 },
    size_pt: Some(DEFAULT_NOTE_SIZE_PT),
};
const DEFAULT_NOTE_SIZE_PT: (f64, f64) = (108.0, 59.25);
