//! DrawingML spreadsheet drawings (`xl/drawings/drawingN.xml`): anchors, pictures, shapes,
//! equations and chart frames.
//!
//! An equation is written as `mc:AlternateContent`: the `a14` choice is a text box holding the
//! equation as Office Math (`a14:m`), the fallback the same text box holding the LaTeX as plain
//! text. Both carry the source in `descr="gridcraft:latex:…"`, which wins on reading; without it,
//! Office Math in the subset `omml` reads is converted back to LaTeX.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use craft_xlsx::{Anchor as CraftAnchor, Drawing as CraftDrawing, Item as CraftItem, Marker as CraftMarker, Placement as CraftPlacement};

use gridcraft_core::{CellRef, MAX_COLS, MAX_ROWS};
use gridcraft_model::{
    Anchor, AnchorMode, ChildTransform, Color, DrawingFile, DrawingGroup, DrawingMarker, DrawingPlacement, DrawingRef, FormControl, GroupFile,
    GroupFileMember, GroupProjection, Image, MAX_GROUP_DEPTH, ObjectRect, Shape, ShapeFormat, ShapeKind, ShapeTextAlign, ShapeTextAnchor, Sheet,
    Theme, group_child_box, group_quarter_turn, group_unturned_box, rotates,
};

use crate::IoError;
use crate::package::Rel;
use crate::read::Ctx;
use crate::tables::EMU_PER_PX;
use crate::xml::{El, esc, esc_attr};

mod ink;

/// What the drawings of the sheet being read hold that the model doesn't, noted for pass-through
/// (`passthrough::KeptReader::sheet` keeps the parts behind it).
#[derive(Default)]
pub struct DrawingKeptIn {
    pub chart_shapes: Vec<ChartShapesIn>,
    pub links: Vec<ObjectLinkIn>,
}

/// A chart's shapes part, read into shapes and pictures of the sheet.
pub struct ChartShapesIn {
    /// The chart's model id.
    pub chart: u32,
    pub chart_part: String,
    /// The id of the chart part's relationship to the shapes part.
    pub rel_id: String,
    /// The model ids of the shapes and pictures read from the part.
    pub shapes: Vec<u32>,
    pub images: Vec<u32>,
}

/// A hyperlink element (`a:hlinkClick` or `a:hlinkHover`) of a picture or shape read from a
/// drawing.
pub struct ObjectLinkIn {
    /// The object in the model.
    pub object: DrawingRef,
    pub drawing_part: String,
    /// The element, namespaces declared.
    pub xml: String,
    /// The relationship ids it names (its `r:id`, and its sound's `r:embed`).
    pub rel_ids: Vec<String>,
}

/// The attributes of a hyperlink (CT_Hyperlink, ECMA-376 Part 1 §21.1.2.3.5) other than `r:id`.
const LINK_ATTRS: [&str; 7] = ["invalidUrl", "action", "tgtFrame", "tooltip", "history", "highlightClick", "endSnd"];

/// The hyperlink elements of a picture's or shape's `cNvPr` (`a:hlinkClick`, `a:hlinkHover`),
/// each as XML with its namespaces declared and the relationship ids it names. Each goes on its
/// own: one that names no relationship (an empty `r:id`) is left out, not the others.
fn object_links(object: &El) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    let Some(nv) = object.children.iter().find(|e| matches!(e.name.as_str(), "nvPicPr" | "nvSpPr" | "nvCxnSpPr")) else { return out };
    let Some(c_nv_pr) = nv.child("cNvPr") else { return out };
    for link in c_nv_pr.children.iter().filter(|e| matches!(e.name.as_str(), "hlinkClick" | "hlinkHover")) {
        let Some(id) = link.attr("id").filter(|id| !id.trim().is_empty()) else { continue };
        let mut xml = String::new();
        let mut ids = vec![id.to_string()];
        let _ = write!(xml, "<a:{} xmlns:a=\"{NS_A}\" xmlns:r=\"{NS_R}\" r:id=\"{}\"", link.name, esc_attr(id));
        for k in LINK_ATTRS {
            if let Some(v) = link.attr(k) {
                let _ = write!(xml, " {k}=\"{}\"", esc_attr(v));
            }
        }
        // The sound played on click, an embedded part of its own.
        match link.child("snd").and_then(|s| Some((s.attr("embed").filter(|e| !e.is_empty())?, s.attr("name").unwrap_or("")))) {
            Some((embed, name)) => {
                ids.push(embed.to_string());
                let _ = write!(xml, "><a:snd r:embed=\"{}\" name=\"{}\"/></a:{}>", esc_attr(embed), esc_attr(name), link.name);
            }
            None => xml.push_str("/>"),
        }
        out.push((xml, ids));
    }
    out
}

const NS_A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const NS_R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// Notes the hyperlinks of the picture or shape `obj` of drawing `part`, read as `object`.
fn note_link(cx: &mut Ctx<'_>, part: &str, obj: &El, object: DrawingRef) {
    for (xml, rel_ids) in object_links(obj) {
        cx.drawing_kept.links.push(ObjectLinkIn { object, drawing_part: part.to_string(), xml, rel_ids });
    }
}

// ---------------------------------------------------------------- reading

fn emu_px(v: Option<i64>) -> f32 {
    v.map(|e| (e as f64 / EMU_PER_PX).clamp(-1e7, 1e7) as f32).unwrap_or(0.0)
}

pub(crate) fn marker(e: &El) -> (CellRef, f32, f32) {
    let num = |n: &str| e.child(n).and_then(|c| c.text.trim().parse::<i64>().ok());
    let col = num("col").unwrap_or(0).clamp(0, MAX_COLS as i64 - 1) as u32;
    let row = num("row").unwrap_or(0).clamp(0, MAX_ROWS as i64 - 1) as u32;
    (CellRef::new(row, col), emu_px(num("colOff")).max(0.0), emu_px(num("rowOff")).max(0.0))
}

fn ext_px(e: Option<&El>) -> (f32, f32) {
    match e {
        Some(x) => (emu_px(x.attr_i64("cx")).max(0.0), emu_px(x.attr_i64("cy")).max(0.0)),
        None => (0.0, 0.0),
    }
}

fn drawing_placement(p: &CraftPlacement) -> DrawingPlacement {
    let marker = |m: CraftMarker| DrawingMarker { col: m.col, row: m.row, col_off: m.col_off, row_off: m.row_off };
    match p {
        CraftPlacement::TwoCell { from, to, edit_as } => DrawingPlacement::TwoCell { from: marker(*from), to: marker(*to), edit_as: edit_as.clone() },
        CraftPlacement::OneCell { from, cx, cy } => DrawingPlacement::OneCell { from: marker(*from), ext: (*cx, *cy) },
        CraftPlacement::Absolute { x, y, cx, cy } => DrawingPlacement::Absolute { pos: (*x, *y), ext: (*cx, *cy) },
    }
}

fn from_craft_el(e: &craft_xml::El) -> El {
    let name = e.name.split_once(':').map_or(e.name.as_str(), |(_, local)| local).to_string();
    let attrs = e.attrs.iter().map(|(k, v)| (k.split_once(':').map_or(k.as_str(), |(_, local)| local).to_string(), v.clone())).collect();
    let mut children = Vec::new();
    let mut text = String::new();
    for node in &e.children {
        match node {
            craft_xml::Node::El(c) => children.push(from_craft_el(c)),
            craft_xml::Node::Text(t) | craft_xml::Node::CData(t) => text.push_str(t),
            craft_xml::Node::Pi { .. } | craft_xml::Node::Comment(_) => {}
        }
    }
    El { name, attrs, children, text }
}

/// Colour of a DrawingML colour choice inside `e` (e.g. a `solidFill`).
pub fn dml_color(e: &El) -> Option<Color> {
    if let Some(c) = e.child("srgbClr").and_then(|c| c.attr("val")) {
        return Color::from_hex(c);
    }
    if let Some(c) = e.child("sysClr").and_then(|c| c.attr("lastClr")) {
        return Color::from_hex(c);
    }
    if let Some(c) = e.child("schemeClr").and_then(|c| c.attr("val")) {
        let i = match c {
            "bg1" | "lt1" => 0,
            "tx1" | "dk1" => 1,
            "bg2" | "lt2" => 2,
            "tx2" | "dk2" => 3,
            "accent1" => 4,
            "accent2" => 5,
            "accent3" => 6,
            "accent4" => 7,
            "accent5" => 8,
            "accent6" => 9,
            "hlink" => 10,
            "folHlink" => 11,
            _ => return None,
        };
        return Some(Color::Theme(i, 0));
    }
    None
}

fn mime_for(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" | "jpe" => "image/jpeg",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "svg" => "image/svg+xml",
        "emf" => "image/x-emf",
        "wmf" => "image/x-wmf",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
}

/// A drawing's editable anchor plus the transform and enclosing projection for nested groups.
#[derive(Clone, Copy)]
struct Place {
    anchor: Anchor,
    frame: ObjectRect,
    parent: Option<GroupProjection>,
}

/// An object's own transform (`a:xfrm`) inside a group: frames carry it directly, groups in
/// `grpSpPr`, pictures and shapes in `spPr`.
fn obj_xfrm(obj: &El) -> Option<&El> {
    match obj.name.as_str() {
        "graphicFrame" => obj.child("xfrm"),
        "grpSp" => obj.path(&["grpSpPr", "xfrm"]),
        _ => obj.path(&["spPr", "xfrm"]),
    }
}

/// A point (`off`/`chOff`, attributes `x`/`y`) or size (`ext`/`chExt`, `cx`/`cy`) of an `a:xfrm`,
/// in EMUs.
fn xfrm_int_pair(xf: Option<&El>, name: &str, a: &str, b: &str) -> Option<(i64, i64)> {
    let e = xf?.child(name)?;
    Some((e.attr_i64(a).unwrap_or(0), e.attr_i64(b).unwrap_or(0)))
}

/// The flips (`flipH`, `flipV`) of an `a:xfrm`.
fn xfrm_flip(xf: Option<&El>) -> (bool, bool) {
    xf.map_or((false, false), |x| (x.flag("flipH", false), x.flag("flipV", false)))
}

/// The rotation (`rot`, 60000ths of a degree) of an `a:xfrm`.
fn xfrm_rot(xf: Option<&El>) -> i64 {
    xf.and_then(|x| x.attr_i64("rot")).unwrap_or(0)
}

fn child_transform(xf: Option<&El>) -> Option<ChildTransform> {
    Some(ChildTransform {
        off: xfrm_int_pair(xf, "off", "x", "y")?,
        ext: xfrm_int_pair(xf, "ext", "cx", "cy")?,
        rotation: xfrm_rot(xf),
        flip: xfrm_flip(xf),
    })
}

/// The objects among `children`, resolving markup-compatibility blocks to their first choice
/// holding a frame (chartex charts), else the fallback.
fn drawing_objects(children: &[El]) -> Vec<&El> {
    children
        .iter()
        .flat_map(|o| match o.name.as_str() {
            "AlternateContent" => o
                .child("Choice")
                .filter(|c| {
                    // Office Math is taken only when the box holds one equation that
                    // converts: otherwise the fallback (a picture or text from the writer)
                    // keeps everything visible.
                    c.child("graphicFrame").is_some()
                        || c.child("sp").and_then(|sp| sp.child("txBody")).and_then(crate::omml::equation_source).is_some()
                })
                .or_else(|| o.child("Fallback"))
                .map(|c| c.children.iter().collect())
                .unwrap_or_default(),
            _ => vec![o],
        })
        .collect()
}

/// The original XML of a drawing's opaque control stand-ins, not editable GridCraft shapes.
#[derive(Debug)]
pub struct KeptProxies {
    pub part: String,
    pub anchors: Vec<String>,
}

/// Reads the drawing, separating modeled objects from their legacy control stand-ins.
pub fn read_drawing(cx: &mut Ctx<'_>, part: &str, controls: &mut crate::controls::ReadControls, sheet: &mut Sheet) -> Result<(), IoError> {
    let Some(bytes) = cx.pkg.read(part)? else {
        cx.warn(format!("missing part {part}"));
        return Ok(());
    };
    let opts = craft_xml::Options::canonical(&craft_xlsx::ns::CANONICAL).with_limits(craft_xml::Limits {
        max_input: crate::package::MAX_PART as usize,
        max_depth: crate::xml::MAX_DEPTH,
        max_nodes: crate::xml::MAX_XML_NODES,
        ..craft_xml::Limits::DEFAULT
    });
    let root = match opts.parse(&bytes) {
        Ok(r) => r,
        Err(e) if e.is_limit() => return Err(IoError::TooLarge(format!("{part}: {e}"))),
        Err(e) => {
            cx.warn(format!("skipped {part}: {e}"));
            return Ok(());
        }
    };
    let mut retained = HashSet::new();
    let rels = cx.pkg.rels(part)?;
    let mut n = 0;
    for (i, item) in CraftDrawing::read(&root).items.into_iter().enumerate() {
        let alternate;
        let anchors = match &item {
            CraftItem::Anchor(a) => std::slice::from_ref(a),
            CraftItem::Other(e) if e.name == "mc:AlternateContent" => {
                alternate = CraftDrawing::alternate_anchors(e);
                alternate.as_slice()
            }
            CraftItem::Other(_) => continue,
        };
        let mut opaque = 0;
        for craft_a in anchors {
            n += 1;
            if let Some(id) = proxy_number(craft_a, controls) {
                opaque += usize::from(!controls.modelled.contains(&id));
                continue;
            }
            let placement = drawing_placement(&craft_a.placement);
            let anchor = placement.anchor(sheet);
            let place = Place { anchor, frame: sheet.anchor_rect(&anchor), parent: None };
            let obj_el = from_craft_el(&craft_a.object);
            for obj in drawing_objects(std::slice::from_ref(&obj_el)) {
                if let Some(reference) = read_object(cx, (part, n), &rels, sheet, obj, place, 0)? {
                    sheet.drawing_files.insert(reference, DrawingFile { anchor, placement: placement.clone() });
                    sheet.drawing_order.insert(reference, n);
                }
            }
        }
        if opaque != 0 {
            if opaque == anchors.len() {
                retained.insert(i);
            } else {
                cx.warn(format!("a mixed alternate drawing in {part} contains form control stand-ins that could not be kept"));
            }
        }
    }
    if !retained.is_empty() {
        let fragments = crate::passthrough::drawing_fragments(&bytes, &retained);
        if fragments.len() != retained.len() {
            cx.warn(format!("some form control stand-ins in {part} could not be kept"));
        }
        controls.drawing = Some(KeptProxies { part: part.into(), anchors: fragments });
    }
    Ok(())
}

fn read_object(
    cx: &mut Ctx<'_>,
    location: (&str, usize),
    rels: &[Rel],
    sheet: &mut Sheet,
    obj: &El,
    place: Place,
    depth: usize,
) -> Result<Option<DrawingRef>, IoError> {
    let reference = read_object_inner(cx, location, rels, sheet, obj, place, depth)?;
    // An object's name is kept even when its nonvisual ID is the placeholder 0: the writer then
    // gives it a fresh ID, but macros and the Selection Pane still find it by name.
    if let Some(r) = reference
        && let Some(nv) = obj.children.iter().find_map(|e| e.child("cNvPr"))
        && let (Some(id), Some(name)) = (nv.attr("id").and_then(|s| s.parse::<u32>().ok()), nv.attr("name"))
        && (id != 0 || !name.is_empty())
    {
        sheet.drawing_names.insert(r, (id as usize, name.to_owned()));
    }
    Ok(reference)
}

/// `compatExt` is authoritative. Without it, require the hidden shape's id and name both to
/// match the worksheet entry: ordinary DrawingML ids and VML ids can otherwise overlap.
fn proxy_number(a: &CraftAnchor, controls: &crate::controls::ReadControls) -> Option<u64> {
    if controls.known.is_empty() || a.object.name != "xdr:sp" {
        return None;
    }
    let pr = a.props()?;
    let compat = pr.child("a:extLst").into_iter().flat_map(|l| l.elements()).find_map(|e| e.child("a14:compatExt"));
    if let Some(compat) = compat {
        return compat.attr("spid").and_then(crate::controls::shape_number).filter(|n| controls.known.contains_key(n));
    }
    let id = pr.attr("id").and_then(crate::controls::shape_number)?;
    let name = controls.known.get(&id)?.as_deref()?;
    (matches!(pr.attr("hidden"), Some("1" | "true")) && a.name() == Some(name)).then_some(id)
}

pub fn preserved_ids(xml: &str) -> impl Iterator<Item = usize> + '_ {
    let mut reader = quick_xml::Reader::from_str(xml);
    std::iter::from_fn(move || {
        loop {
            match reader.read_event().ok()? {
                quick_xml::events::Event::Start(e) | quick_xml::events::Event::Empty(e) if e.local_name().as_ref() == b"cNvPr" => {
                    if let Some(id) =
                        e.attributes().flatten().find(|a| a.key.as_ref() == b"id").and_then(|a| std::str::from_utf8(&a.value).ok()?.parse().ok())
                    {
                        return Some(id);
                    }
                }
                quick_xml::events::Event::Eof => return None,
                _ => {}
            }
        }
    })
}

/// Imports one drawing object at `place`, returning what it became. A group's children are laid
/// out through the group's child coordinate space (nested groups compose), keep the outer
/// anchor's mode and are listed, back to front, in a [`DrawingGroup`]; children that aren't
/// supported are dropped with their own warning.
fn read_object_inner(
    cx: &mut Ctx<'_>,
    (part, n): (&str, usize),
    rels: &[Rel],
    sheet: &mut Sheet,
    obj: &El,
    place: Place,
    depth: usize,
) -> Result<Option<DrawingRef>, IoError> {
    let anchor = place.anchor;
    match obj.name.as_str() {
        "pic" => {
            let Some(rid) = obj.path(&["blipFill", "blip"]).and_then(|b| b.attr("embed")) else { return Ok(None) };
            let Some(rel) = rels.iter().find(|r| r.id == rid && !r.external) else {
                cx.warn("a linked (not embedded) picture was skipped");
                return Ok(None);
            };
            let target = rel.target.clone();
            let Some(data) = cx.pkg.read(&target)? else {
                cx.warn(format!("missing picture {target}"));
                return Ok(None);
            };
            let alt = obj.path(&["nvPicPr", "cNvPr"]).and_then(|c| c.attr("descr")).unwrap_or("").to_string();
            let id = cx.next_object_id();
            sheet.images.push(Image { id, anchor, data: data.into(), mime: mime_for(&target).to_string(), alt });
            note_link(cx, part, obj, DrawingRef::Image(id));
            Ok(Some(DrawingRef::Image(id)))
        }
        "graphicFrame" => {
            let Some(gd) = obj.path(&["graphic", "graphicData"]) else { return Ok(None) };
            let uri = gd.attr("uri").unwrap_or("");
            if let Some(rid) = gd.child("chart").and_then(|c| c.attr("id")) {
                let Some(rel) = rels.iter().find(|r| r.id == rid) else {
                    cx.warn(format!("a chart without its chart part was skipped ({part}, object {n})"));
                    return Ok(None);
                };
                let target = rel.target.clone();
                if let Some(cx_xml) = cx.optional_xml(&target)? {
                    let chartex = uri.contains("chartex");
                    let was_exceeded = cx.chart_budget.exceeded;
                    let parsed = if chartex {
                        crate::chartex::read_chartex(&cx_xml).map(|c| (c, Vec::new()))
                    } else {
                        crate::chart::read_chart_with_budget(&cx_xml, &mut cx.chart_budget)
                    };
                    match parsed {
                        Some((mut ch, caches)) => {
                            let id = cx.next_object_id();
                            ch.id = id;
                            ch.anchor = anchor;
                            if !was_exceeded && cx.chart_budget.exceeded {
                                cx.warn("cached chart points were dropped after the workbook's chart import budget was exceeded");
                            }
                            if !caches.is_empty() {
                                cx.pivot_charts.push((id, caches));
                            }
                            sheet.charts.push(ch);
                            read_user_shapes(cx, (id, &target), &cx_xml, anchor, sheet)?;
                            return Ok(Some(DrawingRef::Chart(id)));
                        }
                        None => cx.warn(format!("chart {target} has no supported plot and was skipped")),
                    }
                } else {
                    cx.warn(format!("chart {target} is missing from the package and was skipped ({part}, object {n})"));
                }
            } else if let Some(id) = crate::slicer::read_frame(cx, &sheet.name, gd, anchor) {
                // Grouped slicers attach outside their group; only top-level frames carry placement.
                return Ok((depth == 0).then_some(DrawingRef::Slicer(id)));
            } else if uri.contains("slicer") || gd.child("slicer").is_some() {
                // Other slicer-like frames, such as timelines, have no model.
            } else if uri.contains("chartex") || uri.contains("chartEx") {
                cx.warn(format!("a histogram, waterfall, treemap… chart without its chart part was skipped ({part}, object {n})"));
            } else {
                cx.warn(format!("an embedded object (SmartArt, OLE…) was skipped ({part}, object {n})"));
            }
            Ok(None)
        }
        "sp" | "cxnSp" => {
            let Some(s) = read_shape(obj, anchor, depth > 0) else { return Ok(None) };
            let id = cx.next_object_id();
            sheet.shapes.push(Shape { id, ..s });
            note_link(cx, part, obj, DrawingRef::Shape(id));
            Ok(Some(DrawingRef::Shape(id)))
        }
        "grpSp" => {
            if depth >= MAX_GROUP_DEPTH {
                cx.warn(format!("deeply nested grouped shapes were skipped ({part}, object {n})"));
                return Ok(None);
            }
            let id = cx.next_object_id();
            let xf = obj_xfrm(obj);
            let file_off = xfrm_int_pair(xf, "off", "x", "y");
            let file_ext = xfrm_int_pair(xf, "ext", "cx", "cy");
            let file_child_offset = xfrm_int_pair(xf, "chOff", "x", "y");
            let file_child_extent = xfrm_int_pair(xf, "chExt", "cx", "cy");
            let flip = xfrm_flip(xf);
            let rotation = xfrm_rot(xf);
            let fallback = if place.parent.is_some() { 1.0 } else { 1.0 / EMU_PER_PX };
            let child_box = group_child_box(
                place.frame,
                file_child_offset.or(file_off).unwrap_or_default(),
                file_child_extent.unwrap_or_default(),
                file_ext.unwrap_or_default(),
                fallback,
            );
            let own = GroupProjection::new(place.frame, child_box, (flip, rotation), fallback);
            let map = place.parent.map_or(own, |parent| parent.compose(own));
            let mut file_members = Vec::new();
            let mut file_complete = true;
            let mut member_rotations = Vec::new();
            let mut members = Vec::new();
            for child in drawing_objects(&obj.children) {
                if !matches!(child.name.as_str(), "pic" | "graphicFrame" | "sp" | "cxnSp" | "grpSp" | "AlternateContent") {
                    continue;
                }
                let cxf = obj_xfrm(child);
                let transform = child_transform(cxf);
                let nested = child.name == "grpSp";
                let rot = if nested { 0 } else { xfrm_rot(cxf) };
                let frame = transform.map_or(child_box, ChildTransform::rect);
                let box_in_parent = if nested { group_unturned_box(frame, xfrm_rot(cxf)) } else { transform.map_or(frame, ChildTransform::leaf_box) };
                let rect = map.project(box_in_parent);
                let place = Place { anchor: sheet.rect_anchor(rect, anchor.mode), frame, parent: Some(map) };
                let Some(m) = read_object(cx, (part, n), rels, sheet, child, place, depth + 1)? else {
                    file_complete = false;
                    continue;
                };
                if rot != 0 {
                    member_rotations.push((m, rot));
                }
                file_complete &= nested || transform.is_some();
                file_members.push(GroupFileMember { reference: m, anchor: place.anchor, transform });
                members.push(m);
            }
            if members.is_empty() {
                return Ok(None);
            }
            let name = obj.path(&["nvGrpSpPr", "cNvPr"]).and_then(|c| c.attr("name")).unwrap_or("").to_string();
            let effective = ChildTransform::from_rect(child_box, 0, (false, false));
            let (child_offset, child_extent) = (effective.off, effective.ext);
            match group_quarter_turn(rotation) {
                None => cx.warn("a rotated group was laid out without its rotation"),
                Some(0) => {}
                Some(_) => cx.warn("a rotated group was laid out turned, but its pictures and shapes aren't drawn rotated"),
            }
            if member_rotations.iter().any(|&(_, r)| rotates(r)) {
                cx.warn("a rotated picture or shape in a group is drawn without its rotation");
            }
            if flip.0 || flip.1 {
                cx.warn("a flipped group was laid out mirrored, but its pictures and shapes aren't drawn mirrored");
            }
            let file = GroupFile {
                anchor,
                off: file_off.unwrap_or_default(),
                ext: file_ext.unwrap_or_default(),
                child_offset: file_child_offset.unwrap_or(child_offset),
                child_extent: file_child_extent.unwrap_or(child_extent),
                model_child_offset: child_offset,
                model_child_extent: child_extent,
                fields: [file_off.is_some(), file_ext.is_some(), file_child_offset.is_some(), file_child_extent.is_some()],
                has_transform: xf.is_some(),
                edited: false,
                held_collapse: [None; 2],
                sort_lost_collapsed_rows: false,
                complete: file_complete,
                flip,
                rotation,
                member_rotations: member_rotations.clone(),
                members: file_members,
            };
            sheet.groups.push(DrawingGroup {
                id,
                name,
                anchor,
                child_offset,
                child_extent,
                flip,
                rotation,
                member_rotations,
                members,
                file: Some(file),
            });
            Ok(Some(DrawingRef::Group(id)))
        }
        _ => Ok(None),
    }
}

const MAX_ANNOTATION_BYTES: u64 = 16 * 1024 * 1024;
const MAX_ANNOTATION_RELS_BYTES: u64 = 4 * 1024 * 1024;

fn optional_annotation_rels(cx: &mut Ctx<'_>, part: &str) -> Result<Option<Vec<crate::package::Rel>>, IoError> {
    let rels_part = craft_opc::rels_path(part);
    match cx.pkg.read_limited(&rels_part, MAX_ANNOTATION_RELS_BYTES) {
        Ok(Some(bytes)) => {
            let rels = craft_opc::Rels::parse_in(&bytes, part, |n| cx.pkg.has(n).then(|| n.to_string()));
            Ok(Some(
                rels.list
                    .into_iter()
                    .map(|r| {
                        let kind = r.kind().to_string();
                        crate::package::Rel { id: r.id, kind, target: r.target, external: r.external }
                    })
                    .collect(),
            ))
        }
        Ok(None) => Ok(Some(Vec::new())),
        Err(IoError::TooLarge(m)) => Err(IoError::TooLarge(m)),
        Err(e) => {
            cx.warn(format!("skipped {rels_part}: {e}"));
            Ok(None)
        }
    }
}

fn optional_annotation_xml(cx: &mut Ctx<'_>, part: &str) -> Result<Option<El>, IoError> {
    match cx.pkg.read_limited(part, MAX_ANNOTATION_BYTES) {
        Ok(Some(b)) => match crate::xml::parse(&b) {
            Ok(e) => Ok(Some(e)),
            Err(e) => {
                cx.warn(format!("skipped {part}: {part}: {e}"));
                Ok(None)
            }
        },
        Ok(None) => {
            cx.warn(format!("missing part {part}"));
            Ok(None)
        }
        Err(IoError::TooLarge(m)) => Err(IoError::TooLarge(m)),
        Err(e) => {
            cx.warn(format!("skipped {part}: {e}"));
            Ok(None)
        }
    }
}
fn read_user_shapes(
    cx: &mut Ctx<'_>,
    (chart, chart_part): (u32, &str),
    chart_xml: &El,
    chart_anchor: Anchor,
    sheet: &mut Sheet,
) -> Result<(), IoError> {
    let Some(rid) = chart_xml.child("userShapes").and_then(|u| u.attr("id")) else { return Ok(()) };
    let Some(chart_rels) = optional_annotation_rels(cx, chart_part)? else { return Ok(()) };
    let Some(rel) = chart_rels.iter().find(|r| r.id == rid && !r.external) else { return Ok(()) };
    let us_part = rel.target.clone();
    let Some(us_xml) = optional_annotation_xml(cx, &us_part)? else { return Ok(()) };
    let Some(us_rels) = optional_annotation_rels(cx, &us_part)? else { return Ok(()) };
    let chart_x = sheet.col_left(chart_anchor.cell.col) + chart_anchor.dx as f64;
    let chart_y = sheet.row_top(chart_anchor.cell.row) + chart_anchor.dy as f64;
    let chart_w = chart_anchor.width.max(0.0) as f64;
    let chart_h = chart_anchor.height.max(0.0) as f64;
    let mut kept = ChartShapesIn { chart, chart_part: chart_part.to_string(), rel_id: rid.to_string(), shapes: vec![], images: vec![] };
    for a in &us_xml.children {
        let frac = |parent: &str, child: &str, def: f64| {
            a.path(&[parent, child]).and_then(|e| e.text.trim().parse::<f64>().ok()).filter(|v| v.is_finite()).unwrap_or(def).clamp(0.0, 1.0)
        };
        let (fx1, fy1) = (frac("from", "x", 0.0), frac("from", "y", 0.0));
        let (width, height) = match a.name.as_str() {
            "relSizeAnchor" => {
                let fx2 = frac("to", "x", 1.0).max(fx1);
                let fy2 = frac("to", "y", 1.0).max(fy1);
                (((fx2 - fx1) * chart_w) as f32, ((fy2 - fy1) * chart_h) as f32)
            }
            "absSizeAnchor" => ext_px(a.child("ext")),
            _ => continue,
        };
        let x = chart_x + fx1 * chart_w;
        let y = chart_y + fy1 * chart_h;
        let col = sheet.col_at(x);
        let row = sheet.row_at(y);
        let sub_anchor = Anchor {
            cell: CellRef::new(row, col),
            dx: (x - sheet.col_left(col)).max(0.0) as f32,
            dy: (y - sheet.row_top(row)).max(0.0) as f32,
            width,
            height,
            mode: chart_anchor.mode,
        };
        for obj in &a.children {
            match obj.name.as_str() {
                "pic" => {
                    let Some(prid) = obj.path(&["blipFill", "blip"]).and_then(|b| b.attr("embed")) else { continue };
                    let Some(prel) = us_rels.iter().find(|r| r.id == prid && !r.external) else {
                        cx.warn("a picture with a missing or external relationship in a chart drawing was skipped");
                        continue;
                    };
                    let target = prel.target.clone();
                    let Some(data) = cx.pkg.read(&target)? else {
                        cx.warn(format!("missing picture {target}"));
                        continue;
                    };
                    let alt = obj.path(&["nvPicPr", "cNvPr"]).and_then(|c| c.attr("descr")).unwrap_or("").to_string();
                    let id = cx.next_object_id();
                    sheet.images.push(Image { id, anchor: sub_anchor, data: data.into(), mime: mime_for(&target).to_string(), alt });
                    kept.images.push(id);
                }
                "sp" | "cxnSp" => {
                    if let Some(s) = read_shape(obj, sub_anchor, false) {
                        let id = cx.next_object_id();
                        sheet.shapes.push(Shape { id, ..s });
                        kept.shapes.push(id);
                    }
                }
                _ => {}
            }
        }
    }
    cx.drawing_kept.chart_shapes.push(kept);
    Ok(())
}

/// A shape at `anchor`; `grouped` when it sits in a group, at its group's child-space transform.
fn read_shape(obj: &El, anchor: Anchor, grouped: bool) -> Option<Shape> {
    let sp = obj.child("spPr")?;
    if let Some(shape) = ink::read(obj, anchor, grouped) {
        return Some(shape);
    }
    let prst = sp.child("prstGeom").and_then(|g| g.attr("prst")).unwrap_or("rect");
    let tx_box = obj.path(&["nvSpPr", "cNvSpPr"]).is_some_and(|c| c.flag("txBox", false));
    let ln = sp.child("ln");
    let arrow = ln.is_some_and(|l| l.child("tailEnd").or_else(|| l.child("headEnd")).is_some_and(|t| t.attr("type").is_some_and(|v| v != "none")));
    // WordArt: our marker, a text body with the non-warping text warp `textNoShape`, on a rectangle
    // that is a text box (as the writer saves it) or has no fill and no outline of its own (as
    // WordArt usually comes). `textNoShape` is the standard "no warp" value, so on an ordinary
    // shape (an ellipse, a filled rectangle) it, like any other warp, leaves the shape what its
    // geometry says.
    let word_art = prst == "rect"
        && (tx_box || (sp.child("noFill").is_some() && ln.is_none_or(|l| l.child("solidFill").is_none())))
        && obj.path(&["txBody", "bodyPr", "prstTxWarp"]).is_some_and(|w| w.attr("prst") == Some("textNoShape"));
    let kind = match prst {
        _ if word_art => ShapeKind::WordArt,
        _ if tx_box => ShapeKind::TextBox,
        "roundRect" => ShapeKind::RoundedRectangle,
        "ellipse" => ShapeKind::Ellipse,
        "triangle" => ShapeKind::Triangle,
        "line" | "straightConnector1" if arrow => ShapeKind::Arrow,
        "line" | "straightConnector1" => ShapeKind::Line,
        "rightArrow" => ShapeKind::Arrow,
        _ => ShapeKind::Rectangle,
    };
    // An equation: GridCraft's own source while the marker in the description still matches the
    // body (another application that edits the Office Math, or the text of a box without Office
    // Math, leaves the description alone, as Excel does), else the Office Math converted back to
    // LaTeX. A box without Office Math whose text was edited is a text box with the edited text,
    // not the stale source, which the next save would write over the edit; as the marker joins
    // run text across paragraphs, such a box must also hold the paragraphs the writer wrote.
    let descr = obj.path(&["nvSpPr", "cNvPr"]).and_then(|c| c.attr("descr")).unwrap_or("");
    let tb = obj.child("txBody");
    let has_math = tb.is_some_and(|tb| crate::omml::find_math(tb, 0).is_some());
    let source = match (has_math, described_source(descr)) {
        (_, Some((marker, src))) if tb.is_some_and(|tb| crate::omml::body_marker(tb) == marker && (has_math || holds_written_text(tb, src))) => {
            Some(src.to_string())
        }
        (true, _) => tb.and_then(crate::omml::equation_source),
        (false, _) => None,
    };
    if let Some(text) = source {
        let ink = obj.child("txBody").and_then(|tb| first_run_color(tb, 0)).unwrap_or_default();
        return Some(Shape {
            id: 0,
            kind: ShapeKind::Equation,
            anchor,
            fill: Color::Auto,
            line: ink,
            text,
            format: Default::default(),
            rotation: 0.0,
            math: Default::default(),
        });
    }
    let fill = sp.child("solidFill").and_then(dml_color).unwrap_or_default();
    let line = ln.and_then(|l| l.child("solidFill")).and_then(dml_color).unwrap_or_default();
    let text = tb.map(|tb| if has_math { math_body_text(tb) } else { body_text(tb) }).unwrap_or_default();
    let mut format = read_text_format(obj.child("txBody"));
    format.no_line = ln.is_some_and(|l| l.child("noFill").is_some());
    let rotation = sp
        .child("xfrm")
        .and_then(|x| x.attr("rot"))
        .and_then(|r| r.parse::<f64>().ok())
        .filter(|r| r.is_finite())
        .map_or(0.0, |r| ((r / 60000.0).rem_euclid(360.0)) as f32);
    Some(Shape { id: 0, kind, anchor, fill, line, text, format, rotation, math: Default::default() })
}

/// A shape's text formatting: vertical anchor (`a:bodyPr anchor`), the first paragraph's
/// alignment and direction (`a:pPr algn`, `rtl`) and the first run's properties (`a:rPr`: size,
/// bold, italic, fill, outline, `a:latin` typeface).
fn read_text_format(body: Option<&El>) -> ShapeFormat {
    let mut f = ShapeFormat::default();
    let Some(body) = body else { return f };
    f.anchor = body.child("bodyPr").and_then(|b| b.attr("anchor")).map(|a| match a {
        "ctr" => ShapeTextAnchor::Middle,
        "b" => ShapeTextAnchor::Bottom,
        _ => ShapeTextAnchor::Top,
    });
    let ppr = body.kids("p").next().and_then(|p| p.child("pPr"));
    f.align = ppr.and_then(|p| p.attr("algn")).map(|a| match a {
        "ctr" => ShapeTextAlign::Center,
        "r" => ShapeTextAlign::Right,
        _ => ShapeTextAlign::Left,
    });
    f.rtl = ppr.is_some_and(|p| p.flag("rtl", false));
    let rpr = body.kids("p").flat_map(|p| p.kids("r")).find_map(|r| r.child("rPr"));
    if let Some(r) = rpr {
        // `sz` is in hundredths of a point (100..=400000 in the schema).
        f.font_size = r.attr_f64("sz").filter(|v| v.is_finite()).map(|v| (v / 100.0).clamp(1.0, 4000.0) as f32);
        f.bold = r.flag("b", false);
        f.italic = r.flag("i", false);
        f.font = r.child("latin").and_then(|l| l.attr("typeface")).filter(|t| !t.is_empty() && !t.starts_with('+')).map(str::to_string);
        f.text_color = r.child("solidFill").and_then(dml_color);
        f.text_shadow = r.child("effectLst").and_then(|e| e.child("outerShdw")).and_then(dml_color);
        f.text_outline = r.child("ln").and_then(|l| {
            let c = l.child("solidFill").and_then(dml_color)?;
            // Width in EMU (12700 per point); Excel's default hairline is 0.75 pt.
            let w = l.attr_f64("w").filter(|w| w.is_finite()).map_or(0.75, |w| (w / 12700.0).clamp(0.0, 100.0)) as f32;
            Some((c, w))
        });
    }
    f
}

/// The `descr` prefix that carries an equation's LaTeX source, as
/// `gridcraft:latex:<marker>:<source>`: the marker is [`crate::omml::body_marker`] of the text
/// body written beside it, 16 hex digits.
const LATEX_DESCR: &str = "gridcraft:latex:";

/// The marker and source of a `gridcraft:latex:<marker>:<source>` description; `None` for any
/// other description, a malformed marker or a blank source.
fn described_source(descr: &str) -> Option<(&str, &str)> {
    let rest = descr.strip_prefix(LATEX_DESCR)?;
    let (marker, src) = (rest.get(..16)?, rest.get(17..)?);
    (rest.as_bytes().get(16) == Some(&b':') && marker.bytes().all(|b| b.is_ascii_hexdigit()) && !src.trim().is_empty()).then_some((marker, src))
}

/// The colour of the first run (DrawingML `rPr` with a fill) under `e`.
fn first_run_color(e: &El, depth: usize) -> Option<Color> {
    if depth > 12 {
        return None;
    }
    if e.name == "rPr"
        && let Some(c) = e.child("solidFill").and_then(dml_color)
    {
        return Some(c);
    }
    e.children.iter().find_map(|c| first_run_color(c, depth + 1))
}

// ---------------------------------------------------------------- writing

/// The size, in points, of the plain-text box that stands in for an equation.
const PLAIN_TEXT_PT: f32 = 11.0;

/// An equation shape (inside the anchor): Office Math with a plain-text fallback when the source
/// is in the subset OMML covers, else the plain-text box alone. Both carry the source.
fn write_equation(s: &mut String, sh: &Shape, (id, name): (usize, &str), theme: &Theme, transform: ChildTransform) {
    // The font size the equation is drawn at (it scales with its frame), in points.
    let size_pt = sh
        .math
        .get(&sh.text)
        .and_then(Result::ok)
        .map(|l| {
            let (w, h) = (l.width.max(0.01), l.total_height().max(0.01));
            ((sh.anchor.width - 8.0) / w).min((sh.anchor.height - 8.0) / h) * 72.0 / 96.0
        })
        .filter(|v| v.is_finite() && *v >= 1.0)
        .unwrap_or(11.0)
        .min(409.0);
    let fill = srgb(&sh.line, theme).map(|c| format!("<a:solidFill>{c}</a:solidFill>")).unwrap_or_default();
    let sz = (size_pt * 100.0).round() as u32;
    // Each box carries the marker of its own body beside the source in its description, the
    // Office Math box and the plain-text fallback alike; the reader takes the source only while
    // the body it reads still matches its marker.
    let tx = |body: &str| {
        format!(
            "<xdr:txBody><a:bodyPr wrap=\"none\" lIns=\"0\" tIns=\"0\" rIns=\"0\" bIns=\"0\" rtlCol=\"0\" anchor=\"ctr\" anchorCtr=\"1\"/><a:lstStyle/>{body}</xdr:txBody>"
        )
    };
    let marker = |tx: &str| crate::xml::parse(tx.as_bytes()).map(|e| crate::omml::body_marker(&e)).unwrap_or_else(|_| "0".repeat(16));
    let sp = |tx: &str, marker: &str| {
        format!(
            "<xdr:sp macro=\"\" textlink=\"\"><xdr:nvSpPr><xdr:cNvPr id=\"{id}\" name=\"{name}\" descr=\"{}\"/><xdr:cNvSpPr txBox=\"1\"/></xdr:nvSpPr><xdr:spPr>{}<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom><a:noFill/></xdr:spPr>{tx}</xdr:sp>",
            esc_attr(&format!("{LATEX_DESCR}{marker}:{}", sh.text)),
            xfrm("a:xfrm", transform)
        )
    };
    // The plain text is the LaTeX source, much wider than the typeset equation: at the equation's
    // size it would overflow the frame in Excel, so it uses a normal text size (11 pt, Excel's
    // default for a text box), smaller only when the frame itself is.
    let plain_sz = (size_pt.min(PLAIN_TEXT_PT) * 100.0).round() as u32;
    let plain = plain_paragraphs(&sh.text, &format!("<a:rPr lang=\"en-US\" sz=\"{plain_sz}\">{fill}</a:rPr>"));
    match gridcraft_math::to_omml(&sh.text, size_pt, sh.line.resolve(theme)) {
        Ok(omml) => {
            let math = format!(
                "<a:p><a:pPr algn=\"ctr\"/><a14:m><m:oMathPara xmlns:m=\"http://schemas.openxmlformats.org/officeDocument/2006/math\"><m:oMathParaPr><m:jc m:val=\"centerGroup\"/></m:oMathParaPr>{omml}</m:oMathPara></a14:m><a:endParaRPr lang=\"en-US\" sz=\"{sz}\"/></a:p>"
            );
            let _ = write!(
                s,
                "<mc:AlternateContent xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\"><mc:Choice xmlns:a14=\"http://schemas.microsoft.com/office/drawing/2010/main\" Requires=\"a14\">{}</mc:Choice><mc:Fallback>{}</mc:Fallback></mc:AlternateContent>",
                sp(&tx(&math), &marker(&tx(&math))),
                sp(&tx(&plain), &marker(&tx(&plain)))
            );
        }
        Err(_) => s.push_str(&sp(&tx(&plain), &marker(&tx(&plain)))),
    }
}

/// The paragraphs of an equation's plain text, one per line of `text`, each a run with `rpr` as
/// its properties: the fallback body, and the body of an equation outside the Office Math subset.
fn plain_paragraphs(text: &str, rpr: &str) -> String {
    text.split('\n').map(|line| format!("<a:p><a:pPr algn=\"ctr\"/><a:r>{rpr}<a:t>{}</a:t></a:r></a:p>", esc(line))).collect()
}

/// The text of a text body: the runs of each paragraph, paragraphs joined with newlines.
fn body_text(tb: &El) -> String {
    tb.kids("p").map(|p| p.kids("r").filter_map(|r| r.child("t")).map(|t| t.text.as_str()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

/// A rejected Office Math box still needs its visible run text. Do not reinterpret
/// that text as LaTeX. Keep traversal and output bounded independently of conversion.
fn math_body_text(tb: &El) -> String {
    const MAX_BYTES: usize = 1 << 20;
    fn append(text: &str, out: &mut String) {
        let mut end = text.len().min(MAX_BYTES.saturating_sub(out.len()));
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        out.push_str(&text[..end]);
    }
    fn visit(e: &El, depth: usize, left: &mut usize, out: &mut String) {
        if depth > 64 || *left == 0 || out.len() >= MAX_BYTES {
            return;
        }
        *left -= 1;
        if e.name.ends_with("Pr") {
            return;
        }
        if e.name == "t" {
            append(&e.text, out);
        } else if e.name == "AlternateContent" {
            // Choice and fallback describe the same content, not two text runs.
            if let Some(c) = e.child("Fallback").or_else(|| e.child("Choice")) {
                visit(c, depth + 1, left, out);
            }
        } else {
            for child in &e.children {
                visit(child, depth + 1, left, out);
                if *left == 0 || out.len() >= MAX_BYTES {
                    break;
                }
            }
        }
    }
    let mut out = String::new();
    let mut left = 100_000;
    for (i, p) in tb.kids("p").enumerate() {
        if left == 0 || out.len() >= MAX_BYTES {
            break;
        }
        if i > 0 {
            append("\n", &mut out);
        }
        visit(p, 0, &mut left, &mut out);
    }
    out
}

/// Whether a body without Office Math still holds the text the writer wrote for `src`: the same
/// paragraphs, read back through the same parser. The marker joins run text across paragraphs,
/// so a paragraph split that changes no character passes it; the text does not.
fn holds_written_text(tb: &El, src: &str) -> bool {
    let written = format!("<xdr:txBody xmlns:xdr=\"x\" xmlns:a=\"a\">{}</xdr:txBody>", plain_paragraphs(src, ""));
    crate::xml::parse(written.as_bytes()).is_ok_and(|w| body_text(&w) == body_text(tb))
}

/// Carry an offset at a cell edge, allowing for the precision of the model's f32 extents.
fn marker_edge(line: u32, offset: f64, span: f64, limit: u32, error: f64) -> (u32, f64) {
    let tolerance = 0.5 / EMU_PER_PX + error * f64::from(f32::EPSILON) / 2.0;
    if span > 0.0 && line + 1 < limit && (offset - span).abs() <= tolerance {
        (line + 1, 0.0)
    } else if offset.abs() <= tolerance {
        (line, 0.0)
    } else {
        (line, offset)
    }
}

type PixelMarker = (CellRef, f64, f64);

fn anchor_markers(sheet: &Sheet, a: &Anchor) -> (PixelMarker, PixelMarker) {
    let (mut fc, mut fr) = (a.cell.col, a.cell.row);
    let (mut fdx, mut fdy) = (f64::from(a.dx), f64::from(a.dy));
    let (w, h) = (f64::from(a.width.max(0.0)), f64::from(a.height.max(0.0)));
    let (x2, y2) = (sheet.col_left(fc) + fdx + w, sheet.row_top(fr) + fdy + h);
    let (mut tc, mut tr) = (sheet.col_at(x2), sheet.row_at(y2));
    let (mut tdx, mut tdy) = (x2 - sheet.col_left(tc), y2 - sheet.row_top(tr));
    if sheet.geometry != gridcraft_model::Geometry::Legacy {
        (tc, tdx) = marker_edge(tc, tdx, f64::from(sheet.col_width(tc)), MAX_COLS, fdx.abs() + w);
        (tr, tdy) = marker_edge(tr, tdy, f64::from(sheet.row_height(tr)), MAX_ROWS, fdy.abs() + h);
        (fc, fdx) = marker_edge(fc, fdx, f64::from(sheet.col_width(fc)), MAX_COLS, fdx.abs());
        (fr, fdy) = marker_edge(fr, fdy, f64::from(sheet.row_height(fr)), MAX_ROWS, fdy.abs());
    }
    ((CellRef::new(fr, fc), fdx, fdy), (CellRef::new(tr, tc), tdx, tdy))
}

/// Use the same snapped edges for group transforms as for their worksheet anchors.
fn drawing_rect(sheet: &Sheet, anchor: &Anchor) -> ObjectRect {
    if sheet.geometry == gridcraft_model::Geometry::Legacy {
        return sheet.anchor_rect(anchor);
    }
    let ((from, dx, dy), (to, tdx, tdy)) = anchor_markers(sheet, anchor);
    let (x, y) = (sheet.col_left(from.col) + dx, sheet.row_top(from.row) + dy);
    ObjectRect { x, y, w: (sheet.col_left(to.col) + tdx - x).max(0.0), h: (sheet.row_top(to.row) + tdy - y).max(0.0) }
}

/// The opening `<xdr:twoCellAnchor>` for a model anchor, carrying Excel's `editAs` when the mode
/// is not the default: omitted for "Move and size with cells", `editAs="oneCell"` for "Move but
/// don't size", `editAs="absolute"` for "Don't move or size". Untouched imported objects
/// instead use `source_anchor_open`.
/// Returns the opening tag and the matching closing tag.
fn anchor_open(sheet: &Sheet, a: &Anchor) -> (String, &'static str) {
    let px = |v: f64| ((v.max(0.0)) * EMU_PER_PX).round() as i64;
    let ((from, fdx, fdy), (to, tdx, tdy)) = anchor_markers(sheet, a);
    let edit = match a.mode {
        AnchorMode::MoveAndSize => "",
        AnchorMode::MoveOnly => " editAs=\"oneCell\"",
        AnchorMode::Absolute => " editAs=\"absolute\"",
    };
    let open = format!(
        "<xdr:twoCellAnchor{edit}><xdr:from><xdr:col>{}</xdr:col><xdr:colOff>{}</xdr:colOff><xdr:row>{}</xdr:row><xdr:rowOff>{}</xdr:rowOff></xdr:from><xdr:to><xdr:col>{}</xdr:col><xdr:colOff>{}</xdr:colOff><xdr:row>{}</xdr:row><xdr:rowOff>{}</xdr:rowOff></xdr:to>",
        from.col,
        px(fdx),
        from.row,
        px(fdy),
        to.col,
        px(tdx),
        to.row,
        px(tdy)
    );
    (open, "</xdr:twoCellAnchor>")
}

fn source_anchor_open(p: &DrawingPlacement) -> (String, &'static str) {
    fn marker(s: &mut String, tag: &str, m: &DrawingMarker) {
        let _ = write!(
            s,
            "<xdr:{tag}><xdr:col>{}</xdr:col><xdr:colOff>{}</xdr:colOff><xdr:row>{}</xdr:row><xdr:rowOff>{}</xdr:rowOff></xdr:{tag}>",
            m.col, m.col_off, m.row, m.row_off
        );
    }
    match p {
        DrawingPlacement::TwoCell { from, to, edit_as } => {
            let mut s = String::from("<xdr:twoCellAnchor");
            if let Some(edit_as) = edit_as {
                let _ = write!(s, " editAs=\"{}\"", esc_attr(edit_as));
            }
            s.push('>');
            marker(&mut s, "from", from);
            marker(&mut s, "to", to);
            (s, "</xdr:twoCellAnchor>")
        }
        DrawingPlacement::OneCell { from, ext: (cx, cy) } => {
            let mut s = String::from("<xdr:oneCellAnchor>");
            marker(&mut s, "from", from);
            let _ = write!(s, "<xdr:ext cx=\"{cx}\" cy=\"{cy}\"/>");
            (s, "</xdr:oneCellAnchor>")
        }
        DrawingPlacement::Absolute { pos: (x, y), ext: (cx, cy) } => {
            (format!("<xdr:absoluteAnchor><xdr:pos x=\"{x}\" y=\"{y}\"/><xdr:ext cx=\"{cx}\" cy=\"{cy}\"/>"), "</xdr:absoluteAnchor>")
        }
    }
}

/// EMUs for a length or position in pixels.
fn emu(v: f64) -> i64 {
    if v.is_finite() { (v * EMU_PER_PX).round().clamp(-1e15, 1e15) as i64 } else { 0 }
}

/// An `a:xfrm`/`xdr:xfrm` with an offset and extent in EMUs and a rotation (omitted when 0).
fn xfrm(tag: &str, ChildTransform { off, ext, rotation, flip }: ChildTransform) -> String {
    let mut attrs = String::new();
    if rotation != 0 {
        let _ = write!(attrs, " rot=\"{rotation}\"");
    }
    if flip.0 {
        attrs.push_str(" flipH=\"1\"");
    }
    if flip.1 {
        attrs.push_str(" flipV=\"1\"");
    }
    format!("<{tag}{attrs}><a:off x=\"{}\" y=\"{}\"/><a:ext cx=\"{}\" cy=\"{}\"/></{tag}>", off.0, off.1, ext.0.max(0), ext.1.max(0))
}

pub fn srgb(c: &Color, theme: &Theme) -> Option<String> {
    let [r, g, b] = c.resolve(theme)?;
    Some(format!("<a:srgbClr val=\"{r:02X}{g:02X}{b:02X}\"/>"))
}

/// One drawing object to write.
pub enum Obj<'a> {
    Image(&'a Image, String),
    Chart(&'a gridcraft_model::Chart, String),
    Shape(&'a Shape),
    /// A slicer and its (unique) name.
    Slicer(&'a gridcraft_model::Slicer, String),
}

impl Obj<'_> {
    fn anchor(&self) -> &Anchor {
        match self {
            Obj::Image(o, _) => &o.anchor,
            Obj::Chart(o, _) => &o.anchor,
            Obj::Shape(o) => &o.anchor,
            Obj::Slicer(o, _) => &o.anchor,
        }
    }

    fn drawing_ref(&self) -> DrawingRef {
        match self {
            Obj::Image(o, _) => DrawingRef::Image(o.id),
            Obj::Chart(o, _) => DrawingRef::Chart(o.id),
            Obj::Shape(o) => DrawingRef::Shape(o.id),
            Obj::Slicer(o, _) => DrawingRef::Slicer(o.id),
        }
    }
}

/// A drawing part being written.
struct DrawingOut<'a> {
    s: String,
    sheet: &'a Sheet,
    theme: &'a Theme,
    objs: &'a [Obj<'a>],
    /// The kept hyperlink elements of pictures and shapes, by object id.
    links: &'a HashMap<u32, String>,
    /// Where each object is in `objs` (the first, should ids repeat).
    index: HashMap<DrawingRef, usize>,
    /// The direct parent of each member, built once for the drawing write.
    member_parent: HashMap<DrawingRef, u32>,
    /// Objects already written, by index in `objs`, and groups already written: each goes out
    /// once.
    done: Vec<bool>,
    groups_done: HashSet<u32>,
    next_id: usize,
    used_ids: HashSet<usize>,
    reserved_ids: HashSet<usize>,
}

impl DrawingOut<'_> {
    fn outer_group(&self, r: DrawingRef) -> Option<u32> {
        let mut cur = *self.member_parent.get(&r)?;
        for _ in 0..MAX_GROUP_DEPTH {
            match self.member_parent.get(&DrawingRef::Group(cur)).copied() {
                Some(p) if p != cur => cur = p,
                _ => break,
            }
        }
        Some(cur)
    }

    fn take_id(&mut self, reference: DrawingRef) -> usize {
        if let Some(&(id, _)) = self.sheet.drawing_names.get(&reference)
            && id != 0
            && self.used_ids.insert(id)
        {
            return id;
        }
        self.take_fresh_id()
    }

    fn take_fresh_id(&mut self) -> usize {
        while self.reserved_ids.contains(&self.next_id) {
            self.next_id += 1;
        }
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// A member's box; a group's is the box around its members that are written, except that a
    /// rotated group keeps its own box (it turns about the box's centre, so a box measured from
    /// fewer members, or from turned ones, would move them).
    fn rect(&self, r: DrawingRef) -> Option<ObjectRect> {
        let members =
            self.sheet.drawing_rect_with(r, &|o| self.index.get(&o).and_then(|&i| self.objs.get(i)).map(|o| drawing_rect(self.sheet, o.anchor())));
        match r {
            DrawingRef::Group(id) => match self.sheet.drawing_group(id) {
                Some(g) if g.rotated() => members.map(|_| drawing_rect(self.sheet, &g.anchor)),
                _ => members,
            },
            _ => members,
        }
    }

    fn source<'g>(&self, g: &'g DrawingGroup) -> Option<&'g GroupFile> {
        self.sheet.source_group(g)
    }

    /// Write a group spanning `b` on the sheet, with its frame in the parent's child space.
    fn group(
        &mut self,
        g: &DrawingGroup,
        b: ObjectRect,
        frame: ChildTransform,
        parent: Option<(GroupProjection, bool)>,
        source: Option<&GroupFile>,
        depth: usize,
    ) {
        let sheet = self.sheet;
        if depth >= MAX_GROUP_DEPTH || !self.groups_done.insert(g.id) {
            return;
        }
        let exact_frame = source.is_some() && parent.is_none_or(|(_, exact)| exact);
        let (off, ext) = (frame.off, frame.ext);
        // Use the last layout frame as the reference space, not the possibly smaller box of
        // members being written. For a nested group undo the parent's projection first.
        let reference = drawing_rect(sheet, &g.anchor);
        let reference = parent.map_or(reference, |(map, _)| group_unturned_box(map.unproject(reference), g.rotation));
        let fallback = if parent.is_some() { 1.0 } else { 1.0 / EMU_PER_PX };
        let use_frame = |origin: f64, extent: f64, off: i64, span: i64, start: f64, len: f64| {
            if extent > 0.0 && span > 0 { (origin, extent, off as f64, span as f64) } else { (start, len, start / fallback, len / fallback) }
        };
        let written = if parent.is_some() { frame.rect() } else { b };
        let (x, w, cx, cw) = use_frame(reference.x, reference.w, g.child_offset.0, g.child_extent.0, written.x, written.w);
        let (y, h, cy, ch) = use_frame(reference.y, reference.h, g.child_offset.1, g.child_extent.1, written.y, written.h);
        let rect = ObjectRect { x, y, w, h };
        let own = GroupProjection::new(rect, ObjectRect { x: cx, y: cy, w: cw, h: ch }, (g.flip, g.rotation), fallback);
        let space = parent.map_or(own, |(map, _)| map.compose(own));
        let (group_off, group_ext, ch_off, ch_ext) = source.as_ref().map_or_else(
            || {
                let visible =
                    parent.map_or_else(|| group_unturned_box(b, g.rotation), |(map, _)| map.project(group_unturned_box(written, g.rotation)));
                let ch = ChildTransform::from_rect(space.unproject(visible), 0, (false, false));
                let (ch_off, ch_ext) = (ch.off, ch.ext);
                // An edited top-level group from a file keeps the file's scale from its child
                // space to the sheet: `ext` from the sheet's own pixel metrics can disagree with
                // the child space by the file's column widths, and Excel scales members by
                // `ext`/`chExt`. A pure move then writes the file's `ext` again.
                let scale =
                    |e: i64, ce: i64, ch: i64, fallback: i64| if ce > 0 { (ch as f64 * e as f64 / ce as f64).round() as i64 } else { fallback };
                let ext = match &g.file {
                    Some(f) if depth == 0 && f.fields[1] && f.model_child_extent == g.child_extent => {
                        (scale(f.ext.0, f.child_extent.0, ch_ext.0, ext.0), scale(f.ext.1, f.child_extent.1, ch_ext.1, ext.1))
                    }
                    _ => ext,
                };
                (off, ext, ch_off, ch_ext)
            },
            // A nested group is placed where its parent put it: from the source too when the
            // parent is unedited, or in the parent's recomputed child space when it isn't.
            |f| if exact_frame { (f.off, f.ext, f.child_offset, f.child_extent) } else { (off, ext, f.child_offset, f.child_extent) },
        );
        let reference = DrawingRef::Group(g.id);
        let cid = self.take_id(reference);
        let name = if g.name.is_empty() && !sheet.drawing_names.contains_key(&reference) { format!("Group {}", cid - 1) } else { esc_attr(&g.name) };
        let mut attrs = String::new();
        if g.rotation != 0 {
            let _ = write!(attrs, " rot=\"{}\"", g.rotation);
        }
        if g.flip.0 {
            attrs.push_str(" flipH=\"1\"");
        }
        if g.flip.1 {
            attrs.push_str(" flipV=\"1\"");
        }
        let _ = write!(self.s, "<xdr:grpSp><xdr:nvGrpSpPr><xdr:cNvPr id=\"{cid}\" name=\"{}\"/><xdr:cNvGrpSpPr/></xdr:nvGrpSpPr><xdr:grpSpPr>", name);
        if !exact_frame || source.is_some_and(|f| f.has_transform) {
            let fields = if exact_frame { source.map_or([true; 4], |f| f.fields) } else { [true; 4] };
            let _ = write!(self.s, "<a:xfrm{attrs}>");
            for (i, (tag, x, y, pair)) in
                [("off", "x", "y", group_off), ("ext", "cx", "cy", group_ext), ("chOff", "x", "y", ch_off), ("chExt", "cx", "cy", ch_ext)]
                    .into_iter()
                    .enumerate()
            {
                if fields[i] {
                    let recomputed = source.is_none() || (!exact_frame && i < 2);
                    let (u, v) = if recomputed && i % 2 == 1 { (pair.0.max(0), pair.1.max(0)) } else { pair };
                    let _ = write!(self.s, "<a:{tag} {x}=\"{u}\" {y}=\"{v}\"/>");
                }
            }
            self.s.push_str("</a:xfrm>");
        }
        self.s.push_str("</xdr:grpSpPr>");
        for (i, m) in g.members.iter().enumerate() {
            let Some(r) = self.rect(*m) else { continue };
            let rot = g.member_rotations.iter().find(|(o, _)| o == m).map_or(0, |&(_, rot)| rot);
            let child = match m {
                DrawingRef::Group(id) => sheet.drawing_group(*id),
                _ => None,
            };
            let nested_source = child.and_then(|ng| if source.is_some() { ng.file.as_ref() } else { self.source(ng) });
            let member = source.and_then(|f| f.members[i].transform).unwrap_or_else(|| {
                let r = nested_source.map_or(r, |f| sheet.anchor_rect(&f.anchor));
                let local = space.unproject(r);
                if let Some(child) = child {
                    ChildTransform::from_rect(group_unturned_box(local, child.rotation), child.rotation, child.flip)
                } else {
                    let file_member = g.file.as_ref().and_then(|f| f.members.iter().find(|f| f.reference == *m));
                    let flip = file_member.and_then(|f| f.transform).map_or((false, false), |t| t.flip);
                    ChildTransform::from_leaf_box(local, rot, flip)
                }
            });
            match m {
                DrawingRef::Group(_) => {
                    if let Some(child) = child {
                        self.group(child, r, member, Some((space, source.is_some())), nested_source, depth + 1);
                    }
                }
                leaf => {
                    let Some(&i) = self.index.get(leaf) else { continue };
                    let (Some(o), Some(false)) = (self.objs.get(i), self.done.get(i).copied()) else { continue };
                    if let Some(d) = self.done.get_mut(i) {
                        *d = true;
                    }
                    let oid = self.take_id(*leaf);
                    let name = sheet.drawing_names.get(leaf).map(|(_, name)| name.as_str());
                    let start = self.s.len();
                    write_obj(&mut self.s, self.theme, o, oid, name, member);
                    link_into(&mut self.s, start, o, self.links);
                }
            }
        }
        self.s.push_str("</xdr:grpSp>");
    }
}

/// The drawing part for a sheet's objects (relationship ids already assigned). A grouped object
/// goes out with its outermost group: one anchor holding the `grpSp`, written where the group's
/// first object comes in `objs`. `links` holds the kept hyperlinks by object id. `stand_ins`
/// adds the hidden shapes for modeled controls; `proxies` holds the opaque control anchors.
pub fn write_drawing(
    sheet: &Sheet,
    theme: &Theme,
    objs: &[Obj<'_>],
    links: &HashMap<u32, String>,
    stand_ins: &[(&FormControl, u64)],
    proxies: &[String],
) -> String {
    let mut index = HashMap::new();
    for (i, o) in objs.iter().enumerate() {
        index.entry(o.drawing_ref()).or_insert(i);
    }
    let mut member_parent = HashMap::new();
    for g in &sheet.groups {
        for member in &g.members {
            member_parent.entry(*member).or_insert(g.id);
        }
    }
    let opaque_ids: HashSet<_> = proxies.iter().flat_map(|a| preserved_ids(a)).collect();
    let mut out = DrawingOut {
        s: String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<xdr:wsDr xmlns:xdr=\"http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\">",
        ),
        sheet,
        theme,
        objs,
        links,
        index,
        member_parent,
        done: vec![false; objs.len()],
        groups_done: HashSet::new(),
        next_id: 2,
        used_ids: opaque_ids.iter().copied().chain(stand_ins.iter().filter_map(|&(_, n)| usize::try_from(n).ok())).collect(),
        reserved_ids: sheet.drawing_names.values().map(|(id, _)| *id)
            .chain(opaque_ids.iter().copied())
            .chain(stand_ins.iter().filter_map(|&(_, n)| usize::try_from(n).ok())).collect(),
    };
    let mut order: Vec<_> = (0..objs.len()).collect();
    if !sheet.drawing_order.is_empty() {
        order.sort_by_cached_key(|&i| {
            let r = objs[i].drawing_ref();
            sheet.drawing_order.get(&out.outer_group(r).map_or(r, DrawingRef::Group)).copied().unwrap_or(usize::MAX)
        });
    }
    for i in order {
        let o = &objs[i];
        let reference = o.drawing_ref();
        if let Some(gid) = out.outer_group(reference)
            && !out.groups_done.contains(&gid)
            && let (Some(g), Some(b)) = (sheet.drawing_group(gid), out.rect(DrawingRef::Group(gid)))
        {
            let source = out.source(g);
            let anchor = source.map_or_else(|| sheet.rect_anchor(b, g.anchor.mode), |f| f.anchor);
            let (open, close) = sheet
                .drawing_files
                .get(&DrawingRef::Group(gid))
                .filter(|f| f.anchor == g.anchor && source.is_some())
                .map_or_else(|| anchor_open(sheet, &anchor), |f| source_anchor_open(&f.placement));
            out.s.push_str(&open);
            out.group(
                g,
                b,
                ChildTransform::from_rect(
                    ObjectRect { x: b.x * EMU_PER_PX, y: b.y * EMU_PER_PX, w: b.w * EMU_PER_PX, h: b.h * EMU_PER_PX },
                    g.rotation,
                    g.flip,
                ),
                None,
                source,
                0,
            );
            out.s.push_str("<xdr:clientData/>");
            out.s.push_str(close);
        }
        if out.done.get(i).copied().unwrap_or(true) {
            continue;
        }
        if let Some(d) = out.done.get_mut(i) {
            *d = true;
        }
        let a = o.anchor();
        let (open, close) = sheet
            .drawing_files
            .get(&reference)
            .filter(|f| f.anchor == *a)
            .map_or_else(|| anchor_open(sheet, a), |f| source_anchor_open(&f.placement));
        out.s.push_str(&open);
        let id = out.take_id(reference);
        let name = sheet.drawing_names.get(&reference).map(|(_, name)| name.as_str());
        let rot = match o {
            Obj::Shape(sh) => (f64::from(sh.rotation.rem_euclid(360.0)) * 60000.0).round() as i64,
            _ => 0,
        };
        let start = out.s.len();
        let transform =
            ChildTransform { off: (0, 0), ext: (emu(a.width.max(0.0) as f64), emu(a.height.max(0.0) as f64)), rotation: rot, flip: (false, false) };
        write_obj(&mut out.s, theme, o, id, name, transform);
        link_into(&mut out.s, start, o, links);
        out.s.push_str("<xdr:clientData/>");
        out.s.push_str(close);
    }
    for &(c, n) in stand_ins {
        let id = if opaque_ids.contains(&(n as usize)) { out.take_fresh_id() as u64 } else { n };
        write_stand_in(&mut out.s, sheet, c, id, n);
    }
    for proxy in proxies {
        out.s.push_str(proxy);
    }
    out.s.push_str("</xdr:wsDr>");
    out.s
}

/// Puts the kept hyperlink of picture or shape `o` (from `links`) into the `cNvPr` that
/// [`write_obj`] wrote from `start` on, as its first children (CT_NonVisualDrawingProps).
fn link_into(s: &mut String, start: usize, o: &Obj<'_>, links: &HashMap<u32, String>) {
    let id = match o {
        Obj::Image(i, _) => i.id,
        Obj::Shape(sh) => sh.id,
        _ => return,
    };
    let Some(xml) = links.get(&id) else { return };
    let Some(at) = s.get(start..).and_then(|w| w.find("<xdr:cNvPr ")).map(|i| start + i) else { return };
    // Attribute values have `>` escaped: the first one closes the tag.
    let Some(close) = s.get(at..).and_then(|w| w.find('>')).map(|i| at + i) else { return };
    if close > 0 && s.get(close - 1..close) == Some("/") {
        s.replace_range(close - 1..=close, &format!(">{xml}</xdr:cNvPr>"));
    }
}

/// Control `c`'s DrawingML stand-in, as Excel writes it: a hidden, unfilled rectangle at the
/// control's anchor. The VML shape number `n` is normally also the DrawingML id, unless a kept
/// anchor already owns that id. `compatExt` always links the two, regardless of the drawing id.
fn write_stand_in(s: &mut String, sheet: &Sheet, c: &FormControl, id: u64, n: u64) {
    let (open, close) = anchor_open(sheet, &c.anchor);
    let _ = write!(
        s,
        "<mc:AlternateContent xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\"><mc:Choice xmlns:a14=\"http://schemas.microsoft.com/office/drawing/2010/main\" Requires=\"a14\">{open}<xdr:sp macro=\"\" textlink=\"\"><xdr:nvSpPr><xdr:cNvPr id=\"{id}\" name=\"{}\" hidden=\"1\"><a:extLst><a:ext uri=\"{{63B3BB69-23CF-44E3-9099-C40C66FF867C}}\"><a14:compatExt spid=\"_x0000_s{n}\"/></a:ext></a:extLst></xdr:cNvPr><xdr:cNvSpPr/></xdr:nvSpPr><xdr:spPr bwMode=\"auto\"><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"0\" cy=\"0\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom><a:noFill/><a:ln><a:noFill/></a:ln></xdr:spPr></xdr:sp><xdr:clientData fPrintsWithSheet=\"{}\"/>{close}</mc:Choice><mc:Fallback/></mc:AlternateContent>",
        esc_attr(&c.display_name()),
        u8::from(c.print),
    );
}

/// One object's element, with a `cNvPr` id and a transform in DrawingML units.
fn write_obj(s: &mut String, theme: &Theme, o: &Obj<'_>, id: usize, name: Option<&str>, transform: ChildTransform) {
    let object_name = |prefix| match name {
        Some(name) => esc_attr(name),
        None => format!("{prefix} {}", id - 1),
    };
    match o {
        Obj::Image(img, rid) => {
            let _ = write!(
                s,
                "<xdr:pic><xdr:nvPicPr><xdr:cNvPr id=\"{id}\" name=\"{}\" descr=\"{}\"/><xdr:cNvPicPr><a:picLocks noChangeAspect=\"1\"/></xdr:cNvPicPr></xdr:nvPicPr><xdr:blipFill><a:blip r:embed=\"{rid}\"/><a:stretch><a:fillRect/></a:stretch></xdr:blipFill><xdr:spPr>{}<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></xdr:spPr></xdr:pic>",
                object_name("Picture"),
                esc_attr(&img.alt),
                xfrm("a:xfrm", transform)
            );
        }
        Obj::Chart(ch, rid) if crate::chartex::is_chartex(ch.kind) => {
            // Chartex frames come with a fallback for readers that don't know them.
            let (prefix, ns) = crate::chartex::requires(ch.kind);
            let _ = write!(
                s,
                "<mc:AlternateContent xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\"><mc:Choice xmlns:{prefix}=\"{ns}\" Requires=\"{prefix}\"><xdr:graphicFrame macro=\"\"><xdr:nvGraphicFramePr><xdr:cNvPr id=\"{id}\" name=\"{}\"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr>{}<a:graphic><a:graphicData uri=\"http://schemas.microsoft.com/office/drawing/2014/chartex\"><cx:chart xmlns:cx=\"http://schemas.microsoft.com/office/drawing/2014/chartex\" r:id=\"{rid}\"/></a:graphicData></a:graphic></xdr:graphicFrame></mc:Choice><mc:Fallback><xdr:sp macro=\"\" textlink=\"\"><xdr:nvSpPr><xdr:cNvPr id=\"0\" name=\"\"/><xdr:cNvSpPr><a:spLocks noTextEdit=\"1\"/></xdr:cNvSpPr></xdr:nvSpPr><xdr:spPr>{}<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></xdr:spPr><xdr:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang=\"en-US\"/><a:t>This chart type can't be shown by this application.</a:t></a:r></a:p></xdr:txBody></xdr:sp></mc:Fallback></mc:AlternateContent>",
                object_name("Chart"),
                xfrm("xdr:xfrm", transform),
                xfrm("a:xfrm", transform)
            );
        }
        Obj::Slicer(sl, slicer_name) => {
            let table = matches!(sl.source, gridcraft_model::SlicerSource::Table { .. });
            s.push_str(&crate::slicer::frame_xml(
                id,
                slicer_name,
                name,
                table,
                &xfrm("xdr:xfrm", transform),
                &xfrm("a:xfrm", transform),
            ));
        }
        Obj::Chart(_, rid) => {
            let _ = write!(
                s,
                "<xdr:graphicFrame macro=\"\"><xdr:nvGraphicFramePr><xdr:cNvPr id=\"{id}\" name=\"{}\"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr>{}<a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/chart\"><c:chart r:id=\"{rid}\"/></a:graphicData></a:graphic></xdr:graphicFrame>",
                object_name("Chart"),
                xfrm("xdr:xfrm", transform)
            );
        }
        Obj::Shape(sh) if sh.kind == ShapeKind::Equation => {
            write_equation(s, sh, (id, &object_name("Equation")), theme, transform);
        }
        Obj::Shape(sh) => {
            if let Some(xml) = ink::write(sh, theme, (id, name), transform) {
                s.push_str(&xml);
                return;
            }
            let (prst, tx) = match sh.kind {
                ShapeKind::Rectangle => ("rect", false),
                ShapeKind::RoundedRectangle => ("roundRect", false),
                ShapeKind::Ellipse => ("ellipse", false),
                ShapeKind::Triangle => ("triangle", false),
                ShapeKind::Line | ShapeKind::Arrow => ("line", false),
                // Equations are written by `write_equation`; this arm keeps the match whole.
                ShapeKind::TextBox | ShapeKind::WordArt | ShapeKind::Equation => ("rect", true),
                ShapeKind::Icon | ShapeKind::Ink => ("rect", false),
            };
            let _ = write!(
                s,
                "<xdr:sp macro=\"\" textlink=\"\"><xdr:nvSpPr><xdr:cNvPr id=\"{id}\" name=\"{}\"/><xdr:cNvSpPr{}/></xdr:nvSpPr><xdr:spPr>{}<a:prstGeom prst=\"{prst}\"><a:avLst/></a:prstGeom>",
                object_name("Shape"),
                if tx { " txBox=\"1\"" } else { "" },
                xfrm("a:xfrm", transform)
            );
            if let Some(c) = srgb(&sh.fill, theme) {
                let _ = write!(s, "<a:solidFill>{c}</a:solidFill>");
            }
            let line_c = srgb(&sh.line, theme);
            if sh.format.no_line {
                s.push_str("<a:ln><a:noFill/></a:ln>");
            } else if line_c.is_some() || sh.kind == ShapeKind::Arrow {
                s.push_str("<a:ln w=\"9525\">");
                match line_c {
                    Some(c) => {
                        let _ = write!(s, "<a:solidFill>{c}</a:solidFill>");
                    }
                    None => s.push_str("<a:solidFill><a:srgbClr val=\"000000\"/></a:solidFill>"),
                }
                if sh.kind == ShapeKind::Arrow {
                    s.push_str("<a:tailEnd type=\"triangle\"/>");
                }
                s.push_str("</a:ln>");
            }
            s.push_str("</xdr:spPr>");
            // A WordArt with no text still writes its body, which carries the WordArt marker.
            if !sh.text.is_empty() || sh.kind == ShapeKind::WordArt {
                let f = &sh.format;
                let anchor = match f.anchor {
                    Some(ShapeTextAnchor::Top) => " anchor=\"t\"",
                    Some(ShapeTextAnchor::Middle) => " anchor=\"ctr\"",
                    Some(ShapeTextAnchor::Bottom) => " anchor=\"b\"",
                    None if sh.kind == ShapeKind::TextBox => " anchor=\"t\"",
                    None => "",
                };
                if sh.kind == ShapeKind::WordArt {
                    // Our WordArt marker: a (non-warping) text warp, which the reader maps back to WordArt.
                    let _ = write!(
                        s,
                        "<xdr:txBody><a:bodyPr wrap=\"square\" rtlCol=\"0\"{anchor}><a:prstTxWarp prst=\"textNoShape\"><a:avLst/></a:prstTxWarp></a:bodyPr><a:lstStyle/>"
                    );
                } else {
                    let _ = write!(s, "<xdr:txBody><a:bodyPr wrap=\"square\" rtlCol=\"0\"{anchor}/><a:lstStyle/>");
                }
                let algn = match f.align {
                    None => "",
                    Some(ShapeTextAlign::Left) => " algn=\"l\"",
                    Some(ShapeTextAlign::Center) => " algn=\"ctr\"",
                    Some(ShapeTextAlign::Right) => " algn=\"r\"",
                };
                let rtl = if f.rtl { " rtl=\"1\"" } else { "" };
                let ppr = if algn.is_empty() && rtl.is_empty() { String::new() } else { format!("<a:pPr{algn}{rtl}/>") };
                let (rpr_attrs, rpr_body) = run_props(f, theme);
                for line in sh.text.split('\n') {
                    if line.is_empty() {
                        let _ = write!(s, "<a:p>{ppr}<a:endParaRPr lang=\"en-US\"{rpr_attrs}/></a:p>");
                    } else if rpr_body.is_empty() {
                        let _ = write!(s, "<a:p>{ppr}<a:r><a:rPr lang=\"en-US\"{rpr_attrs}/><a:t>{}</a:t></a:r></a:p>", esc(line));
                    } else {
                        let _ = write!(s, "<a:p>{ppr}<a:r><a:rPr lang=\"en-US\"{rpr_attrs}>{rpr_body}</a:rPr><a:t>{}</a:t></a:r></a:p>", esc(line));
                    }
                }
                s.push_str("</xdr:txBody>");
            }
            s.push_str("</xdr:sp>");
        }
    }
}

/// A text run's `a:rPr` attributes (size, bold, italic) and children in schema order (outline,
/// fill, typeface).
fn run_props(f: &ShapeFormat, theme: &Theme) -> (String, String) {
    let mut attrs = String::new();
    if let Some(sz) = f.font_size.filter(|v| v.is_finite()) {
        let _ = write!(attrs, " sz=\"{}\"", (sz.clamp(1.0, 4000.0) * 100.0).round() as u32);
    }
    if f.bold {
        attrs.push_str(" b=\"1\"");
    }
    if f.italic {
        attrs.push_str(" i=\"1\"");
    }
    let mut body = String::new();
    if let Some((c, w)) = &f.text_outline
        && let Some(c) = srgb(c, theme)
    {
        let emu = (w.clamp(0.0, 100.0) * 12700.0).round() as u32;
        let _ = write!(body, "<a:ln w=\"{emu}\"><a:solidFill>{c}</a:solidFill></a:ln>");
    }
    if let Some(c) = f.text_color.as_ref().and_then(|c| srgb(c, theme)) {
        let _ = write!(body, "<a:solidFill>{c}</a:solidFill>");
    }
    if let Some(c) = f.text_shadow.as_ref().and_then(|c| srgb(c, theme)) {
        let _ = write!(
            body,
            "<a:effectLst><a:outerShdw blurRad=\"38100\" dist=\"38100\" dir=\"2700000\" algn=\"tl\" rotWithShape=\"0\">{c}</a:outerShdw></a:effectLst>"
        );
    }
    if let Some(font) = &f.font {
        let _ = write!(body, "<a:latin typeface=\"{}\"/>", esc_attr(font));
    }
    (attrs, body)
}

/// File extension for an image MIME type.
pub fn image_ext(mime: &str, data: &[u8]) -> &'static str {
    match mime.to_ascii_lowercase().as_str() {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpeg",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        "image/tiff" => "tiff",
        "image/svg+xml" => "svg",
        "image/x-emf" => "emf",
        "image/x-wmf" => "wmf",
        "image/webp" => "webp",
        _ => {
            if data.starts_with(b"\x89PNG") {
                "png"
            } else if data.starts_with(&[0xFF, 0xD8]) {
                "jpeg"
            } else if data.starts_with(b"GIF8") {
                "gif"
            } else {
                "png"
            }
        }
    }
}

pub fn image_content_type(ext: &str) -> &'static str {
    match ext {
        "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "tiff" => "image/tiff",
        "svg" => "image/svg+xml",
        "emf" => "image/x-emf",
        "wmf" => "image/x-wmf",
        "webp" => "image/webp",
        _ => "image/png",
    }
}

#[cfg(test)]
mod mac_marker_tests {
    use super::*;
    use gridcraft_model::{Font, Geometry};

    fn mac_sheet() -> Sheet {
        let mut sheet = Sheet::new("Data");
        sheet.geometry = Geometry::mac(&Font::default());
        sheet.default_col_width = (53.0_f64 * 4.0 / 3.0) as f32;
        sheet
    }

    fn edge_anchor(sheet: &Sheet, col_off: i64) -> Anchor {
        drawing_placement(&CraftPlacement::TwoCell {
            from: CraftMarker::default(),
            to: CraftMarker { col: 5, col_off, ..CraftMarker::default() },
            edit_as: None,
        })
        .anchor(sheet)
    }

    #[test]
    fn fractional_end_at_cell_edge_carries_to_zero_offset() {
        let sheet = mac_sheet();
        let anchor = edge_anchor(&sheet, 0);
        assert!(f64::from(anchor.width) < sheet.col_left(5), "the f32 extent rounds down");
        assert_eq!(anchor_markers(&sheet, &anchor).1, (CellRef::new(0, 5), 0.0, 0.0));

        let from = Anchor { dx: sheet.col_width(0), ..anchor };
        assert_eq!(anchor_markers(&sheet, &from).0, (CellRef::new(0, 1), 0.0, 0.0));
    }

    #[test]
    fn negative_source_offset_clamps_to_the_marked_cell_edge() {
        let sheet = mac_sheet();
        let anchor = edge_anchor(&sheet, -27_000);
        assert_eq!(anchor_markers(&sheet, &anchor).1, (CellRef::new(0, 5), 0.0, 0.0));
    }

    #[test]
    fn fractional_group_extent_uses_the_snapped_edge() {
        let sheet = mac_sheet();
        let anchor = edge_anchor(&sheet, 0);
        let group = DrawingGroup {
            id: 1,
            name: "Group".into(),
            anchor,
            child_offset: (0, 0),
            child_extent: (3_365_500, 0),
            flip: (false, false),
            rotation: 0,
            member_rotations: Vec::new(),
            file: None,
            members: Vec::new(),
        };
        let rect = drawing_rect(&sheet, &anchor);
        assert_eq!(rect.w, sheet.col_left(5));
        assert_eq!(emu(rect.w), 3_365_500);
        let child = ChildTransform { ext: group.child_extent, ..ChildTransform::default() }.rect();
        let map = GroupProjection::new(rect, child, (group.flip, group.rotation), 1.0 / EMU_PER_PX);
        assert_eq!(ChildTransform::from_rect(map.unproject(rect), 0, (false, false)).ext, group.child_extent);
    }
}

#[cfg(test)]
mod math_fallback_tests {
    use super::*;

    #[test]
    fn fallback_uses_one_alternative_and_ignores_properties() {
        let tb = crate::xml::parse(br#"<txBody><p><rPr><t>hidden</t></rPr><r><t>before </t></r><AlternateContent><Choice><r><t>choice</t></r></Choice><Fallback><r><t>fallback</t></r></Fallback></AlternateContent><r><t> after</t></r></p><p><r><t>next</t></r></p></txBody>"#).unwrap();
        assert_eq!(math_body_text(&tb), "before fallback after\nnext");
    }

    #[test]
    fn fallback_output_cap_keeps_utf8_intact() {
        let xml = format!("<txBody><p><r><t>a{}</t></r></p></txBody>", "λ".repeat(1 << 19));
        let tb = crate::xml::parse(xml.as_bytes()).unwrap();
        let text = math_body_text(&tb);
        assert_eq!(text.len(), (1 << 20) - 1);
        assert!(text.starts_with('a') && text.ends_with('λ'));
    }
}
