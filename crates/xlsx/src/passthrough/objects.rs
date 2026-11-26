//! What a sheet's drawing holds that the model doesn't: the shapes drawn on its charts (a chart's
//! `c:userShapes` part) and the hyperlinks of its pictures and shapes.
//!
//! GridCraft reads a chart's shapes into shapes and pictures of the sheet. The part goes back with
//! its chart, byte for byte, while the chart is there and those shapes and pictures are as they
//! were read; they are then not written as objects of the sheet as well. Once one of them was
//! changed or the chart deleted, they are written as objects of the sheet, as they show, and the
//! save says so.
//!
//! A hyperlink goes back on its picture or shape while that is there, wherever it moved.

use super::read::{KeptReader, map_rel_ids, raw_rels};
use super::rebase::SheetKept;
use super::write::{NameMap, reachable_in, remap, write_parts};
use super::{HashMap, HashSet, MAX_META, Out, Preserved, PreservedRel, Rels, Sheet, Workbook, normalize, rels_part_name, xml};
use crate::drawing::{ChartShapesIn, ObjectLinkIn};
use crate::read::Ctx;
use gridcraft_core::{CellRef, RangeRef};
use gridcraft_model::{DrawingRef, PreservedChartShapes, PreservedObjectLink, PreservedObjectLinks};

/// The relationship type of a chart's shapes part.
const REL_CHART_SHAPES: &str = "chartUserShapes";

impl KeptReader {
    /// Keeps the shapes parts of the sheet's charts and the hyperlinks of its pictures and shapes
    /// (`found` while its drawing was read).
    pub(super) fn drawing_content(&mut self, cx: &mut Ctx<'_>, name: &str, sheet: &Sheet, found: crate::drawing::DrawingKeptIn, p: &mut Preserved) {
        for cs in found.chart_shapes {
            self.chart_shapes(cx, name, sheet, cs, p);
        }
        self.object_links(cx, name, found.links, p);
    }

    fn chart_shapes(&mut self, cx: &mut Ctx<'_>, name: &str, sheet: &Sheet, cs: ChartShapesIn, p: &mut Preserved) {
        // Histogram, waterfall… charts are written without a shapes part.
        if !sheet.charts.iter().any(|c| c.id == cs.chart && !crate::chartex::is_chartex(c.kind)) {
            return;
        }
        let Some(rel) = raw_rels(cx, &cs.chart_part).ok().and_then(|rels| rels.into_iter().find(|r| r.id == cs.rel_id && !r.external)) else {
            return;
        };
        let mut q = Preserved::default();
        if let Err(why) = self.keep_through(cx, &mut q, std::slice::from_ref(&rel), &mut HashSet::new(), &[REL_CHART_SHAPES]) {
            cx.warn(format!("sheet \"{name}\": the shapes drawn on a chart will be saved as shapes of the sheet: {why}"));
            return;
        }
        let shapes = sheet.shapes.iter().filter(|s| cs.shapes.contains(&s.id)).cloned().collect();
        let images = sheet.images.iter().filter(|i| cs.images.contains(&i.id)).cloned().collect();
        p.chart_shapes.push(PreservedChartShapes { chart: cs.chart, rel, parts: q.parts, shapes, images });
    }

    /// Keeps each hyperlink element of the sheet's pictures and shapes on its own: one whose
    /// relationship is missing is dropped with a warning, not the object's other link.
    fn object_links(&mut self, cx: &mut Ctx<'_>, name: &str, found: Vec<ObjectLinkIn>, p: &mut Preserved) {
        let mut kept = PreservedObjectLinks::default();
        // A sheet has one drawing; its relationships are read once.
        let mut drawing_rels: HashMap<String, Vec<PreservedRel>> = HashMap::new();
        let mut seen = HashSet::new();
        for link in found {
            let rels = match drawing_rels.get(&link.drawing_part) {
                Some(r) => r,
                None => {
                    let r = drawing_link_rels(cx, &link.drawing_part);
                    drawing_rels.entry(link.drawing_part.clone()).or_insert(r)
                }
            };
            let wanted: Option<Vec<PreservedRel>> = link.rel_ids.iter().map(|id| rels.iter().find(|r| r.id == *id).cloned()).collect();
            let Some(wanted) = wanted else {
                cx.warn(format!("sheet \"{name}\": the hyperlink of a picture or shape was dropped: the drawing has no relationship for it"));
                continue;
            };
            // A link to a place in the workbook (`#Sheet1!A1`) names no part.
            let parts: Vec<PreservedRel> = wanted.iter().filter(|r| !r.target.starts_with('#')).cloned().collect();
            let mut q = Preserved::default();
            if let Err(why) = self.keep(cx, &mut q, &parts, &mut seen) {
                cx.warn(format!("sheet \"{name}\": the hyperlink of a picture or shape was dropped: {why}"));
                continue;
            }
            kept.parts.extend(q.parts);
            for r in wanted {
                if !kept.rels.iter().any(|x| x.id == r.id) {
                    kept.rels.push(r);
                }
            }
            kept.links.push(PreservedObjectLink { object: link.object, xml: self.kept_xml(link.xml) });
        }
        if !kept.links.is_empty() {
            p.object_links = Some(kept);
        }
    }
}

/// The relationships of the drawing part `part`, with the target of a link to a place in the
/// workbook (`#Sheet1!A1`, no part) as the file wrote it: [`raw_rels`] resolves targets to parts.
fn drawing_link_rels(cx: &mut Ctx<'_>, part: &str) -> Vec<PreservedRel> {
    let mut rels = raw_rels(cx, part).unwrap_or_default();
    let raw = cx.pkg.read_limited(&rels_part_name(part), MAX_META).ok().flatten().and_then(|b| xml::parse(&b).ok());
    if let Some(raw) = raw {
        for r in raw.kids("Relationship") {
            if let (Some(id), Some(target)) = (r.attr("Id"), r.attr("Target").filter(|t| t.starts_with('#')))
                && let Some(rel) = rels.iter_mut().find(|x| x.id == id && !x.external)
            {
                rel.target = target.to_string();
            }
        }
    }
    rels
}

/// The kept content of a sheet's drawing as it goes into a save.
pub struct DrawingKept<'a> {
    /// The shapes parts written with their charts, by chart id.
    charts: HashMap<u32, &'a PreservedChartShapes>,
    /// The sheet's shapes and pictures written in a chart's shapes part instead of the drawing.
    in_charts: HashSet<DrawingRef>,
    links: Option<&'a PreservedObjectLinks>,
}

impl<'a> SheetKept<'a> {
    /// What of the sheet's kept drawing content goes into the save. The shapes of a chart go back
    /// in its shapes part only while the chart is there and they are as they were read; otherwise
    /// they are written as objects of the sheet, and the save says so.
    pub fn drawing(&self, sheet: &Sheet, out: &mut Out) -> DrawingKept<'a> {
        let mut kept = DrawingKept { charts: HashMap::new(), in_charts: HashSet::new(), links: None };
        let Some(p) = self.preserved() else { return kept };
        kept.links = p.object_links.as_ref();
        for cs in &p.chart_shapes {
            let chart = sheet.charts.iter().any(|c| c.id == cs.chart && !crate::chartex::is_chartex(c.kind));
            let unchanged = cs.shapes.iter().all(|s| sheet.shapes.contains(s)) && cs.images.iter().all(|i| sheet.images.contains(i));
            if chart && unchanged {
                kept.in_charts.extend(cs.shapes.iter().map(|s| DrawingRef::Shape(s.id)));
                kept.in_charts.extend(cs.images.iter().map(|i| DrawingRef::Image(i.id)));
                kept.charts.insert(cs.chart, cs);
            } else {
                let why = if chart { "they were changed" } else { "the chart was deleted" };
                out.warn(format!(
                    "Sheet \"{}\": shapes drawn on a chart were saved as shapes of the sheet, because {why}; the chart's own shapes part was left out.",
                    sheet.name
                ));
            }
        }
        kept
    }
}

impl DrawingKept<'_> {
    /// Whether the sheet's object `r` is written in a chart's shapes part, not in the drawing.
    pub fn in_chart(&self, r: DrawingRef) -> bool {
        self.in_charts.contains(&r)
    }

    /// Writes the shapes part of the chart `chart` with the parts behind it, adding the chart
    /// part's relationship to it to `chart_rels`; returns the chart's `<c:userShapes>` element.
    pub fn chart_shapes(&self, chart: u32, chart_rels: &mut Rels, out: &mut Out) -> Option<String> {
        let cs = self.charts.get(&chart)?;
        let names = write_parts(out, &cs.parts);
        let target = names.get(&normalize(&cs.rel.target))?;
        let id = chart_rels.add_raw(&cs.rel.rel_type, &format!("/{target}"), false);
        Some(format!("<c:userShapes r:id=\"{id}\"/>"))
    }

    /// The hyperlink elements of each picture and shape of `objects` (on `sheet`, the `si`th of
    /// `wb`) that has one, by object id, with their relationships added to the drawing's
    /// (`drawing_rels`) and the parts behind them written. A link goes back on the object it was
    /// read with, however that was edited (no other object can take its id: see
    /// [`gridcraft_model::Workbook::free_object_ids`]). Each link goes on its own; one that leads
    /// to a place no longer in the workbook is left out, and the save says so.
    pub fn links(
        &self,
        wb: &Workbook,
        (si, sheet): (usize, &Sheet),
        objects: impl Iterator<Item = DrawingRef>,
        drawing_rels: &mut Rels,
        out: &mut Out,
    ) -> HashMap<u32, String> {
        let mut found: HashMap<u32, String> = HashMap::new();
        let Some(kept) = self.links else { return found };
        let objects: HashSet<DrawingRef> = objects.collect();
        let mut live: Vec<(&PreservedObjectLink, Vec<String>)> = vec![];
        for link in kept.links.iter().filter(|l| objects.contains(&l.object)) {
            let mut used = Vec::new();
            let _ = map_rel_ids(&link.xml, &mut |id| {
                used.push(id.to_string());
                Some(id.to_string())
            });
            // A place in this workbook (`#Data!A2`), whatever the relationship's target mode.
            let gone = used
                .iter()
                .filter_map(|id| kept.rels.iter().find(|r| r.id == *id))
                .find(|r| r.target.strip_prefix('#').is_some_and(|place| !place_exists(wb, si, place)));
            if let Some(r) = gone {
                out.warn(format!(
                    "Sheet \"{}\": the hyperlink of a picture or shape was left out: it leads to \"{}\", which is no longer in the workbook.",
                    sheet.name, r.target
                ));
                continue;
            }
            live.push((link, used));
        }
        if live.is_empty() {
            return found;
        }
        let names: NameMap = if kept.parts.is_empty() {
            NameMap::new()
        } else {
            let used: HashSet<&str> = live.iter().flat_map(|(_, ids)| ids.iter().map(String::as_str)).collect();
            let roots = kept.rels.iter().filter(|r| !r.external && !r.target.starts_with('#') && used.contains(r.id.as_str()));
            let needed = reachable_in(&kept.parts, roots);
            write_parts(out, kept.parts.iter().filter(|p| needed.contains(&normalize(&p.name))))
        };
        let mut ids: HashMap<String, String> = HashMap::new();
        for (link, used) in live {
            for old in used {
                if ids.contains_key(&old) {
                    continue;
                }
                let Some(r) = kept.rels.iter().find(|r| r.id == old) else { continue };
                let target = if r.external || r.target.starts_with('#') {
                    Some(r.target.clone())
                } else {
                    names.get(&normalize(&r.target)).map(|n| format!("/{n}"))
                };
                if let Some(target) = target {
                    ids.insert(old, drawing_rels.add_raw(&r.rel_type, &target, r.external));
                }
            }
            let (DrawingRef::Image(id) | DrawingRef::Shape(id) | DrawingRef::Chart(id) | DrawingRef::Group(id)) = link.object;
            if let Some(xml) = remap(&link.xml, &ids) {
                // Click before hover, as read (CT_NonVisualDrawingProps).
                found.entry(id).or_default().push_str(&xml);
            }
        }
        found
    }
}

/// Whether the place a hyperlink leads to (`Sheet1!A1`, `'My sheet'!B2:C3`, `Data!LocalName`, a
/// defined name or a cell of the same sheet) is in the workbook. A name qualified by a sheet is
/// that sheet's name only.
fn place_exists(wb: &Workbook, si: usize, place: &str) -> bool {
    let (sheet, body) = gridcraft_core::split_sheet(place);
    let cells = RangeRef::parse(body).is_some() || CellRef::parse(body).is_some();
    match sheet {
        Some(name) => wb.sheet_index(&name).is_some_and(|at| cells || wb.name_in_scope(body, Some(at)).is_some()),
        None => cells || wb.name(body, si).is_some(),
    }
}
