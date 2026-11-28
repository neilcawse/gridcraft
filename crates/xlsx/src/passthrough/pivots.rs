//! PivotTables GridCraft reads but can't rebuild from the model: those whose cache's source is a
//! defined name, which may be a formula (`OFFSET(…)`) or missing. (A source in another workbook,
//! a connection or the Data Model is kept unmodelled instead, [`crate::pivot::ExternalPivot`].)
//! The PivotTable part, its cache definition and records go back as the file had them, while the
//! PivotTable is as it was read but for its name and place and its source still stands for the
//! definition it was bound to (the writer checks that first,
//! [`gridcraft_model::Workbook::pivot_source`]).
//!
//! What the parts refer to outside themselves is given anew: the cache gets the id the writer
//! gives it (the table's `cacheId`, the cache's `x14:pivotCacheId`), and the number formats
//! (`numFmtId`) and differential formats (`dxfId`) they use are added to the styles GridCraft
//! writes, by what they are, and renumbered to match. A table that uses a format the file didn't
//! define is not kept. The cache is written once however many PivotTables share it.

use std::collections::BTreeMap;

use super::read::{KeptReader, attribute_spans};
use super::write::{NameMap, part_rels_xml, write_parts};
use super::{Arc, Ctx, HashMap, HashSet, Out, Preserved, PreservedPart, PreservedRel, Reader, Sheet, is_kind, normalize, rels_part_name};
use crate::pivot::PivotPartsIn;
use gridcraft_model::{PivotTable, PreservedPivot};
use quick_xml::events::Event;

/// The kinds GridCraft rebuilds that a kept PivotTable carries through.
const THROUGH: &[&str] = &["pivotTable", "pivotCacheDefinition", "pivotCacheRecords"];

/// The first custom number format id; lower ones are built in.
const FIRST_CUSTOM_FMT: u32 = 164;

impl KeptReader {
    /// Keeps the parts of the sheet's PivotTables `found` while it was read. `rels` are the
    /// sheet part's relationships.
    pub(super) fn pivots(&mut self, cx: &mut Ctx<'_>, name: &str, sheet: &Sheet, rels: &[PreservedRel], found: Vec<PivotPartsIn>, p: &mut Preserved) {
        for f in found {
            let Some(table) = sheet.pivots.iter().find(|t| t.id == f.pivot) else { continue };
            let key = normalize(&f.table_part);
            let Some(rel) = rels.iter().find(|r| !r.external && is_kind(r, &["pivotTable"]) && normalize(&r.target) == key) else { continue };
            let mut q = Preserved::default();
            let why = |why: &str| format!("sheet \"{name}\": PivotTable \"{}\" can't be kept as the file had it: {why}", table.name);
            // Each PivotTable has its own copy of a shared cache: the writer writes it once.
            if let Err(e) = self.keep_through(cx, &mut q, std::slice::from_ref(rel), &mut HashSet::new(), THROUGH) {
                cx.warn(why(&e));
                continue;
            }
            let cache = normalize(&f.cache_part);
            let Some(at) = q.parts.iter().position(|x| normalize(&x.name) == key) else { continue };
            let part = q.parts.remove(at);
            let Some(cache_part) = q.parts.iter().find(|x| normalize(&x.name) == cache) else {
                cx.warn(why("its cache is not among its parts"));
                continue;
            };
            let styles = match style_refs(&part.data, &cache_part.data, cx) {
                Ok(s) => s,
                Err(e) => {
                    cx.warn(why(&e));
                    continue;
                }
            };
            p.pivots.push(PreservedPivot { table: table.clone(), part, cache, parts: q.parts, num_fmts: styles.0, dxfs: styles.1 });
        }
    }
}

type StyleRefs = (Vec<(u32, String)>, Vec<(u32, gridcraft_model::Style)>);

/// The number formats and differential formats the table XML `table` and the cache XML `cache`
/// use, with what each is in the file's styles. An error names what can't be kept: damaged XML,
/// or a format the styles part doesn't define.
fn style_refs(table: &[u8], cache: &[u8], cx: &Ctx<'_>) -> Result<StyleRefs, String> {
    let mut fmts: BTreeMap<u32, String> = BTreeMap::new();
    let mut dxfs: BTreeMap<u32, gridcraft_model::Style> = BTreeMap::new();
    let mut missing: Option<String> = None;
    for (xml, is_table) in [(table, true), (cache, false)] {
        let seen = rewrite_attrs(xml, &mut |_, attr, value| {
            let id = value.trim().parse::<u32>().ok();
            match (attr, id) {
                ("numFmtId", Some(id)) => match cx.num_fmts.get(&id) {
                    Some(code) => {
                        fmts.insert(id, code.clone());
                    }
                    None if id < FIRST_CUSTOM_FMT => {}
                    None => missing = Some(format!("it uses number format {id}, which the file doesn't define")),
                },
                ("dxfId", Some(id)) if is_table => match cx.dxfs.get(id as usize) {
                    Some(style) => {
                        dxfs.insert(id, style.clone());
                    }
                    None => missing = Some(format!("it uses differential format {id}, which the file doesn't define")),
                },
                ("numFmtId" | "dxfId", None) => missing = Some(format!("its {attr} \"{value}\" is not a number")),
                _ => {}
            }
            None
        });
        if seen.is_none() {
            return Err("its part is damaged".into());
        }
    }
    if let Some(m) = missing {
        return Err(m);
    }
    if set_cache_id(table, 0).is_none() {
        return Err("its part has no cacheId".into());
    }
    Ok((fmts.into_iter().collect(), dxfs.into_iter().collect()))
}

/// The kept parts of the PivotTable `pt` of `sheet`, while it is as it was read and stands for
/// what it stood for as read. Its name and place may have changed since (renamed, or moved by
/// inserted or deleted cells), as Excel keeps a PivotTable through those; the writer then writes
/// them into the kept part. Its source text may have changed too (a sheet or the name it names
/// was renamed) while it is still bound to the same definition; the writer then writes the kept
/// cache's source anew. Any other change leaves it unkept.
pub fn kept_pivot<'a>(sheet: &'a Sheet, pt: &PivotTable) -> Option<&'a PreservedPivot> {
    let def = |pt: &PivotTable| pt.binding.0.as_ref().filter(|b| b.text == pt.source).map(|b| b.def.get());
    sheet.preserved.as_deref()?.pivots.iter().find(|k| {
        if def(&k.table) != def(pt) {
            return false;
        }
        let moved = PivotTable { name: pt.name.clone(), anchor: pt.anchor, last_range: pt.last_range, ..k.table.clone() };
        moved == *pt || (def(pt).is_some() && PivotTable { source: pt.source.clone(), ..moved } == *pt)
    })
}

/// A kept PivotTable as it goes into the save: the table part's XML, and the cache's parts when
/// the cache is to be written with it.
pub struct KeptPivotOut {
    pub table_xml: Vec<u8>,
    cache_parts: Option<Vec<PreservedPart>>,
}

/// Prepares the kept PivotTable `pp`, as `pt` is now, with cache id `cache_id`, its cache's parts
/// too when `with_cache`: its number and differential formats are added to the styles being
/// written and renumbered, its name and location are `pt`'s, and the cache's `worksheetSource`
/// gets the attributes `source` when given (the writer's, so that it reads back to what the
/// PivotTable stands for). An error says why it can't be kept (its part is damaged).
pub fn kept_pivot_out(
    pp: &PreservedPivot,
    pt: &PivotTable,
    cache_id: u32,
    with_cache: bool,
    source: Option<&str>,
    out: &mut Out,
) -> Result<KeptPivotOut, String> {
    let fmts: HashMap<u32, u32> = pp.num_fmts.iter().map(|(id, code)| (*id, out.num_fmts.id(code))).collect();
    let dxfs: HashMap<u32, u32> = pp.dxfs.iter().map(|(id, style)| (*id, crate::styles::dxf_id(&mut out.dxfs, style))).collect();
    let remap = |attr: &str, value: &str| -> Option<String> {
        let id = value.trim().parse::<u32>().ok()?;
        match attr {
            "numFmtId" => fmts.get(&id).map(u32::to_string),
            "dxfId" => dxfs.get(&id).map(u32::to_string),
            _ => None,
        }
    };
    let damaged = || "its part is damaged".to_string();
    let mut table_xml = rewrite_attrs(&pp.part.data, &mut |root, attr, value| match attr {
        "cacheId" if root => Some(cache_id.to_string()),
        _ => remap(attr, value),
    })
    .ok_or_else(damaged)?;
    if (&pt.name, pt.anchor, pt.last_range) != (&pp.table.name, pp.table.anchor, pp.table.last_range) {
        table_xml = crate::pivot::rewrite_preserved_pivot_table(&table_xml, pt, cache_id);
    }
    let cache_parts = if with_cache {
        let mut parts = pp.parts.clone();
        let cache = parts.iter_mut().find(|p| normalize(&p.name) == pp.cache).ok_or_else(damaged)?;
        let mut xml = rewrite_attrs(&cache.data, &mut |_, attr, value| remap(attr, value))
            .and_then(|xml| rewrite_x14_cache_id(&xml, cache_id))
            .ok_or_else(damaged)?;
        if let Some(attrs) = source {
            xml = set_worksheet_source(&xml, attrs).ok_or_else(damaged)?;
        }
        cache.data = Arc::from(xml);
        Some(parts)
    } else {
        None
    };
    Ok(KeptPivotOut { table_xml, cache_parts })
}

/// `xml` (a cache definition) with the attributes of its `worksheetSource` replaced by `attrs`;
/// `None` when it is damaged or has none.
fn set_worksheet_source(xml: &[u8], attrs: &str) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(xml).ok()?;
    let mut reader = Reader::from_str(text);
    loop {
        let start = usize::try_from(reader.buffer_position()).ok()?;
        let event = reader.read_event().ok()?;
        let end = usize::try_from(reader.buffer_position()).ok()?;
        let (e, empty) = match &event {
            Event::Eof => return None,
            Event::Start(e) => (e, false),
            Event::Empty(e) => (e, true),
            _ => continue,
        };
        if e.local_name().as_ref() != b"worksheetSource" {
            continue;
        }
        let qname = std::str::from_utf8(e.name().as_ref()).ok()?.to_string();
        let close = if empty { "/>" } else { ">" };
        return Some(format!("{}<{qname} {attrs}{close}{}", text.get(..start)?, text.get(end..)?).into_bytes());
    }
}

/// Writes the cache of a kept PivotTable prepared with its cache (its definition, records and the
/// parts behind them); returns the name the definition was written at.
pub fn write_pivot_cache(pp: &PreservedPivot, kept: &mut KeptPivotOut, out: &mut Out) -> Option<String> {
    let parts = kept.cache_parts.take()?;
    write_parts(out, &parts).remove(&pp.cache)
}

/// Writes a kept PivotTable part at `name`; its relationship to the cache leads to `cache_part`,
/// where the cache was written.
pub fn write_pivot_table(pp: &PreservedPivot, name: &str, kept: KeptPivotOut, cache_part: &str, out: &mut Out) {
    out.part(name, Some(&pp.part.content_type), kept.table_xml);
    let names: NameMap = HashMap::from([(pp.cache.clone(), cache_part.to_string())]);
    out.part(&rels_part_name(name), None, part_rels_xml(&pp.part.rels, &names).into_bytes());
}

/// `xml` with the `cacheId` of its root element set to `id`; `None` when it is damaged or the root
/// has none.
fn set_cache_id(xml: &[u8], id: u32) -> Option<Vec<u8>> {
    let mut found = false;
    let out = rewrite_attrs(xml, &mut |root, attr, _| {
        (root && attr == "cacheId").then(|| {
            found = true;
            id.to_string()
        })
    })?;
    found.then_some(out)
}

/// Replace only the unqualified ID in the real x14 cache extension, leaving all other bytes intact.
fn rewrite_x14_cache_id(xml: &[u8], id: u32) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(xml).ok()?;
    let mut reader = quick_xml::NsReader::from_str(text);
    let mut path = Vec::new();
    let mut out = Vec::with_capacity(xml.len());
    let mut last = 0;
    let id = id.to_string();
    loop {
        let start = usize::try_from(reader.buffer_position()).ok()?;
        let ev = reader.read_event().ok()?;
        let end = usize::try_from(reader.buffer_position()).ok()?;
        match &ev {
            Event::Eof => break,
            Event::Start(e) | Event::Empty(e) => {
                if path == [true, true, true] && crate::pivot::is_x14_cache_definition(reader.resolver(), e) {
                    for (name, value, _) in attribute_spans(text.get(start..end)?) {
                        if name == "pivotCacheId" {
                            out.extend_from_slice(xml.get(last..start + value.start)?);
                            out.extend_from_slice(id.as_bytes());
                            last = start + value.end;
                        }
                    }
                }
                if matches!(ev, Event::Start(_)) {
                    let expected = [b"pivotCacheDefinition".as_slice(), b"extLst", b"ext"].get(path.len()).copied();
                    path.push(expected == Some(e.local_name().as_ref()));
                }
            }
            Event::End(_) => {
                path.pop();
            }
            _ => {}
        }
    }
    out.extend_from_slice(xml.get(last..)?);
    Some(out)
}

/// `xml` with the value of each attribute `f(root element?, attribute name, value)` gives a new one
/// for replaced (the value is a number: nothing needs escaping); `None` when the XML is damaged.
fn rewrite_attrs(xml: &[u8], f: &mut dyn FnMut(bool, &str, &str) -> Option<String>) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(xml).ok()?;
    let mut reader = Reader::from_str(text);
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut root = true;
    loop {
        let start = usize::try_from(reader.buffer_position()).ok()?;
        let event = reader.read_event().ok()?;
        let end = usize::try_from(reader.buffer_position()).ok()?;
        match event {
            Event::Eof => break,
            Event::Start(_) | Event::Empty(_) => {
                let tag = text.get(start..end)?;
                for (name, value, _) in attribute_spans(tag) {
                    if let Some(new) = f(root, name, tag.get(value.clone())?) {
                        out.push_str(text.get(last..start + value.start)?);
                        out.push_str(&new);
                        last = start + value.end;
                    }
                }
                root = false;
            }
            _ => {}
        }
    }
    out.push_str(text.get(last..)?);
    Some(out.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::{rewrite_attrs, set_cache_id};

    #[test]
    fn cache_id_is_rewritten_on_the_root_only() {
        let xml = "<?xml version=\"1.0\"?>\n<pivotTableDefinition xmlns=\"m\" name=\"P\" cacheId=\"12\"><location ref=\"A1\" cacheId=\"9\"/></pivotTableDefinition>";
        let out = String::from_utf8(set_cache_id(xml.as_bytes(), 3).unwrap()).unwrap();
        assert_eq!(out, xml.replace("cacheId=\"12\"", "cacheId=\"3\""));
        assert!(set_cache_id(b"<pivotTableDefinition name=\"P\"/>", 3).is_none());
    }

    #[test]
    fn attributes_are_rewritten_everywhere_else_left_alone() {
        let xml = "<a numFmtId=\"200\" x=\"1\"><b dxfId=\"0\"/><c numFmtId=\"3\">text &amp; more</c></a>";
        let out = rewrite_attrs(xml.as_bytes(), &mut |_, attr, value| match (attr, value) {
            ("numFmtId", "200") => Some("164".into()),
            ("dxfId", _) => Some("4".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "<a numFmtId=\"164\" x=\"1\"><b dxfId=\"4\"/><c numFmtId=\"3\">text &amp; more</c></a>");
        assert!(rewrite_attrs(b"<a><b></a>", &mut |_, _, _| None).is_none());
    }
}
