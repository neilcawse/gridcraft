//! Frozen panes from an OpenDocument spreadsheet's view settings: `settings.xml` (or the
//! `office:settings` of a flat document), the `ooo:view-settings` set's first view, whose
//! `Tables` map holds each sheet's split. ODF 1.3 part 3 section 3.10 defines the settings
//! containers; the item names are the application's (`HorizontalSplitMode` 2 is a frozen split
//! whose `HorizontalSplitPosition` counts the frozen columns, and likewise vertically for rows).

use std::collections::HashMap;

use gridcraft_core::{MAX_COLS, MAX_ROWS};
use quick_xml::NsReader;
use quick_xml::XmlVersion;
use quick_xml::events::Event;
use quick_xml::name::ResolveResult;

const CONFIG: &[u8] = b"urn:oasis:names:tc:opendocument:xmlns:config:1.0";
const OFFICE: &[u8] = b"urn:oasis:names:tc:opendocument:xmlns:office:1.0";
/// Sheets whose views are read; more is not a real document.
const MAX_TABLES: usize = 4096;

/// A sheet's frozen rows and columns, and the first cell of its scrolling pane.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Frozen {
    pub rows: u32,
    pub cols: u32,
    pub top: u32,
    pub left: u32,
}

#[derive(Default)]
struct Items {
    h_mode: u32,
    v_mode: u32,
    h_pos: u32,
    v_pos: u32,
    right: u32,
    bottom: u32,
}

impl Items {
    fn frozen(&self) -> Option<Frozen> {
        let cols = if self.h_mode == 2 { self.h_pos.min(MAX_COLS - 1) } else { 0 };
        let rows = if self.v_mode == 2 { self.v_pos.min(MAX_ROWS - 1) } else { 0 };
        (rows > 0 || cols > 0).then(|| Frozen {
            rows,
            cols,
            top: if rows > 0 { self.bottom.clamp(rows, MAX_ROWS - 1) } else { 0 },
            left: if cols > 0 { self.right.clamp(cols, MAX_COLS - 1) } else { 0 },
        })
    }
}

fn config_name(reader: &NsReader<&[u8]>, e: &quick_xml::events::BytesStart<'_>) -> Option<String> {
    e.attributes().flatten().find_map(|a| {
        let (ns, local) = reader.resolver().resolve_attribute(a.key);
        (matches!(ns, ResolveResult::Bound(n) if n.as_ref() == CONFIG) && local.as_ref() == b"name")
            .then(|| a.normalized_value(XmlVersion::Explicit1_0).ok().map(|v| v.into_owned()))
            .flatten()
    })
}

/// The frozen panes of each sheet, by the name the document writes. Unreadable settings are
/// no settings: they never cost the document.
pub(crate) fn frozen_panes(bytes: &[u8]) -> HashMap<String, Frozen> {
    let mut out = HashMap::new();
    let mut reader = NsReader::from_reader(bytes);
    // Element names (config local name, `config:name`) from the root down.
    let mut stack: Vec<(Vec<u8>, Option<String>)> = vec![];
    let mut views_seen = 0usize;
    let mut table: Option<(String, Items)> = None;
    let mut item: Option<String> = None;
    let mut text = String::new();
    while let Ok(event) = reader.read_event() {
        match event {
            Event::Start(e) => {
                if stack.len() > 64 {
                    break;
                }
                let (ns, local) = reader.resolver().resolve_element(e.name());
                let config = matches!(ns, ResolveResult::Bound(n) if n.as_ref() == CONFIG);
                let office = matches!(ns, ResolveResult::Bound(n) if n.as_ref() == OFFICE);
                // A flat document's body follows its settings.
                if office && local.as_ref() == b"body" {
                    break;
                }
                let local = if config { local.as_ref().to_vec() } else { vec![] };
                let name = if config { config_name(&reader, &e) } else { None };
                let in_views = stack.iter().any(|(l, n)| l == b"config-item-set" && n.as_deref() == Some("ooo:view-settings"))
                    && stack.last().is_some_and(|(l, n)| l == b"config-item-map-indexed" && n.as_deref() == Some("Views"));
                if in_views && local == b"config-item-map-entry" {
                    views_seen += 1;
                }
                let in_tables = views_seen == 1 && stack.last().is_some_and(|(l, n)| l == b"config-item-map-named" && n.as_deref() == Some("Tables"));
                if in_tables && local == b"config-item-map-entry" && out.len() < MAX_TABLES {
                    table = name.clone().map(|n| (n, Items::default()));
                }
                if table.is_some() && local == b"config-item" {
                    item = name.clone();
                    text.clear();
                }
                stack.push((local, name));
            }
            Event::Text(t) if item.is_some() && text.len() < 64 => {
                if let Ok(s) = t.xml_content(XmlVersion::Explicit1_0) {
                    text.push_str(&s);
                }
            }
            Event::End(_) => {
                let Some((local, _)) = stack.pop() else { break };
                if local == b"config-item"
                    && let (Some(name), Some((_, items))) = (item.take(), table.as_mut())
                {
                    let v = text.trim().parse::<u32>().unwrap_or(0);
                    match name.as_str() {
                        "HorizontalSplitMode" => items.h_mode = v,
                        "VerticalSplitMode" => items.v_mode = v,
                        "HorizontalSplitPosition" => items.h_pos = v,
                        "VerticalSplitPosition" => items.v_pos = v,
                        "PositionRight" => items.right = v,
                        "PositionBottom" => items.bottom = v,
                        _ => {}
                    }
                } else if local == b"config-item-map-entry"
                    && item.is_none()
                    && let Some((name, items)) = table.take()
                    && let Some(f) = items.frozen()
                {
                    out.insert(name, f);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    out
}
