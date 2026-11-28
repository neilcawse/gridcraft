//! What a sheet's drawing holds that the model doesn't: the hyperlinks of its pictures and
//! shapes. A hyperlink goes back on its picture or shape while that is there, wherever it moved.
//! (The shapes drawn on a chart, its `c:userShapes` part, are kept with the drawing's other
//! companions: see `KeptReader::keep_drawing_companions`.)

use super::read::{KeptReader, map_rel_ids, raw_rels};
use super::rebase::SheetKept;
use super::write::{NameMap, reachable_in, remap, write_parts};
use super::{HashMap, HashSet, MAX_META, Out, Preserved, PreservedRel, Rels, Sheet, Workbook, normalize, rels_part_name, xml};
use crate::drawing::ObjectLinkIn;
use crate::read::Ctx;
use gridcraft_core::{CellRef, RangeRef};
use gridcraft_model::{DrawingRef, PreservedObjectLink, PreservedObjectLinks};

impl KeptReader {
    /// Keeps the hyperlinks of the sheet's pictures and shapes (`found` while its drawing was read).
    pub(super) fn drawing_content(&mut self, cx: &mut Ctx<'_>, name: &str, found: crate::drawing::DrawingKeptIn, p: &mut Preserved) {
        self.object_links(cx, name, found.links, p);
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
    links: Option<&'a PreservedObjectLinks>,
}

impl<'a> SheetKept<'a> {
    /// What of the sheet's kept drawing content goes into the save.
    pub fn drawing(&self) -> DrawingKept<'a> {
        DrawingKept { links: self.preserved().and_then(|p| p.object_links.as_ref()) }
    }
}

impl DrawingKept<'_> {
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
            let (DrawingRef::Image(id) | DrawingRef::Shape(id) | DrawingRef::Chart(id) | DrawingRef::Slicer(id) | DrawingRef::Group(id)) =
                link.object;
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
