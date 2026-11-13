//! What the source file's XML says about its cached values that the importer's model doesn't
//! keep, read only when a comparison needs it.

use super::package::{local_name, read_zip_part, workbook_entry, zip_names};
use super::rels::sheet_parts;
use super::{CellRef, Cursor, HashSet, Key, Workbook};

/// The cells of the workbook whose cached value `<v>` the producer wrote with
/// `xml:space="preserve"`: the surrounding whitespace of those is part of the value. A producer
/// that doesn't write the attribute leaves surrounding whitespace unrecorded, as XML allows.
/// Cells of sheets the workbook doesn't list are not looked at; an unreadable package has none.
pub(super) fn preserved_space(bytes: &[u8], wb: &Workbook) -> HashSet<Key> {
    let mut out = HashSet::new();
    let Ok(mut zip) = zip::ZipArchive::new(Cursor::new(bytes)) else { return out };
    let names = zip_names(&mut zip);
    let Some(workbook) = workbook_entry(&mut zip, &names) else { return out };
    for (sheet, part) in sheet_parts(&mut zip, &names, &workbook) {
        let Some(si) = wb.sheet_index(&sheet) else { continue };
        if let Ok(xml) = read_zip_part(&mut zip, &part) {
            scan_preserved(&xml, si, &mut out);
        }
    }
    out
}

/// Adds the cells of worksheet XML `xml` (sheet `si`) with a preserved `<v>` to `out`.
fn scan_preserved(xml: &[u8], si: usize, out: &mut HashSet<Key>) {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_reader(xml);
    let mut buf = Vec::new();
    let mut cell: Option<CellRef> = None;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => match local_name(e.name().as_ref()) {
                b"c" => {
                    cell = e.attributes().with_checks(false).flatten().find(|a| a.key.as_ref() == b"r").and_then(|a| {
                        let r = String::from_utf8_lossy(&a.value).into_owned();
                        CellRef::parse(r.trim())
                    });
                }
                b"v" => {
                    let preserved =
                        e.attributes().with_checks(false).flatten().any(|a| a.key.as_ref() == b"xml:space" && a.value.as_ref() == b"preserve");
                    if let (true, Some(c)) = (preserved, cell) {
                        out.insert((si, c));
                    }
                }
                _ => {}
            },
            Ok(Event::End(e)) if local_name(e.name().as_ref()) == b"c" => cell = None,
            Ok(Event::Eof) | Err(_) => break,
            Ok(_) => {}
        }
        buf.clear();
    }
}
