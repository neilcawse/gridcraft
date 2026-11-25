//! PivotTables whose cache GridCraft can't rebuild from the model (a source in another workbook,
//! or a defined name holding a formula) go back as the file had them while they are unchanged:
//! the PivotTable part, the cache definition and records, and the cache's link to the other
//! workbook. A cache shared by several PivotTables is written once, every `cacheId` and
//! `x14:pivotCacheId` is unique and agrees, and the number and differential formats they use
//! are in the written styles under the ids they name.

use super::*;

const CT_PREFIX: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml";

fn table(name: &str, at: &str) -> String {
    format!(
        r#"<?xml version="1.0"?><pivotTableDefinition xmlns="{NS_MAIN}" name="{name}" cacheId="7" dataCaption="Values"><location ref="{at}" firstHeaderRow="1" firstDataRow="1" firstDataCol="1"/><pivotFields count="2"><pivotField axis="axisRow" showAll="0"><items count="3"><item x="0"/><item x="1"/><item t="default"/></items></pivotField><pivotField dataField="1" showAll="0"/></pivotFields><rowFields count="1"><field x="0"/></rowFields><rowItems count="3"><i><x/></i><i><x v="1"/></i><i t="grand"><x/></i></rowItems><colItems count="1"><i/></colItems><dataFields count="1"><dataField name="Sum of V" fld="1" baseField="0" baseItem="0" numFmtId="200"/></dataFields><formats count="1"><format dxfId="1"><pivotArea outline="0" fieldPosition="0"><references count="1"><reference field="4294967294" count="1"><x v="0"/></reference></references></pivotArea></format></formats></pivotTableDefinition>"#
    )
}

fn cache(source: &str) -> String {
    format!(
        r#"<?xml version="1.0"?><pivotCacheDefinition xmlns="{NS_MAIN}" xmlns:r="{NS_R}" r:id="rId1" recordCount="2"><cacheSource type="worksheet">{source}</cacheSource><cacheFields count="2"><cacheField name="K" numFmtId="0"><sharedItems><s v="a"/><s v="b"/></sharedItems></cacheField><cacheField name="V" numFmtId="200"><sharedItems containsSemiMixedTypes="0" containsString="0" containsNumber="1" containsInteger="1" minValue="1" maxValue="2"/></cacheField></cacheFields><extLst><ext uri="{{725AE2AE-9491-48be-B2B4-4EB974FC3084}}" xmlns:x14="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main"><x14:pivotCacheDefinition pivotCacheId="1"/></ext></extLst></pivotCacheDefinition>"#
    )
}

const RECORDS: &str = r#"<?xml version="1.0"?><pivotCacheRecords xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="2"><r><x v="0"/><n v="1"/></r><r><x v="1"/><n v="2"/></r></pivotCacheRecords>"#;
const OTHER_BOOK: &str = "file:///C:/data/Other.xlsx";
/// Custom number format 200 (`0.000`); differential formats 0 (italic) and 1 (bold): the tables
/// use 1.
const STYLES: &str = r#"<?xml version="1.0"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><numFmts count="1"><numFmt numFmtId="200" formatCode="0.000"/></numFmts><fonts count="1"><font><sz val="11"/><name val="Calibri"/></font></fonts><fills count="1"><fill><patternFill patternType="none"/></fill></fills><borders count="1"><border/></borders><cellStyleXfs count="1"><xf/></cellStyleXfs><cellXfs count="1"><xf/></cellXfs><dxfs count="2"><dxf><font><i/></font></dxf><dxf><font><b/></font></dxf></dxfs></styleSheet>"#;

/// Sheet "Data" holding K/V in A1:B3 and two PivotTables (D3, G3) sharing one cache whose source
/// is `source` (a `<worksheetSource>`); `external` gives the cache a link to another workbook.
fn package(source: &str, external: bool) -> Vec<u8> {
    package_with_cache(&cache(source), external, "OFFSET(Data!$A$1,0,0,3,2)")
}

fn package_with_cache(cache_xml: &str, external: bool, formula: &str) -> Vec<u8> {
    let types = format!(
        r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Override PartName="/xl/workbook.xml" ContentType="{CT_PREFIX}.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="{CT_PREFIX}.worksheet+xml"/><Override PartName="/xl/pivotTables/pivotTable1.xml" ContentType="{CT_PREFIX}.pivotTable+xml"/><Override PartName="/xl/pivotTables/pivotTable2.xml" ContentType="{CT_PREFIX}.pivotTable+xml"/><Override PartName="/xl/pivotCache/pivotCacheDefinition1.xml" ContentType="{CT_PREFIX}.pivotCacheDefinition+xml"/><Override PartName="/xl/pivotCache/pivotCacheRecords1.xml" ContentType="{CT_PREFIX}.pivotCacheRecords+xml"/></Types>"#
    );
    let workbook = format!(
        r#"<?xml version="1.0"?><workbook xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheets><sheet name="Data" sheetId="1" r:id="rId1"/></sheets><definedNames><definedName name="rData">{formula}</definedName></definedNames><pivotCaches><pivotCache cacheId="7" r:id="rId2"/></pivotCaches></workbook>"#
    );
    let c = |r: &str, v: &str| {
        if v.parse::<f64>().is_ok() {
            format!(r#"<c r="{r}"><v>{v}</v></c>"#)
        } else {
            format!(r#"<c r="{r}" t="inlineStr"><is><t>{v}</t></is></c>"#)
        }
    };
    let sheet = format!(
        r#"<?xml version="1.0"?><worksheet xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheetData><row r="1">{}{}</row><row r="2">{}{}</row><row r="3">{}{}</row></sheetData></worksheet>"#,
        c("A1", "K"),
        c("B1", "V"),
        c("A2", "a"),
        c("B2", "1"),
        c("A3", "b"),
        c("B3", "2")
    );
    let wb_rels = rels(&[
        ("rId1", "worksheet", "worksheets/sheet1.xml"),
        ("rId2", "pivotCacheDefinition", "pivotCache/pivotCacheDefinition1.xml"),
        ("rId3", "styles", "styles.xml"),
    ]);
    let sheet_rels = rels(&[("rId1", "pivotTable", "../pivotTables/pivotTable1.xml"), ("rId2", "pivotTable", "../pivotTables/pivotTable2.xml")]);
    let table_rels = rels(&[("rId1", "pivotCacheDefinition", "../pivotCache/pivotCacheDefinition1.xml")]);
    let mut cache_rel = vec![("rId1", "pivotCacheRecords", "pivotCacheRecords1.xml")];
    if external {
        cache_rel.push(("rId2", "externalLinkPath", OTHER_BOOK));
    }
    let cache_rels = rels(&cache_rel);
    let (t1, t2) = (table("P1", "D3:E6"), table("P2", "G3:H6"));
    make_zip(&[
        ("[Content_Types].xml", types.as_bytes()),
        ("_rels/.rels", super::super::ROOT_RELS.as_bytes()),
        ("xl/workbook.xml", workbook.as_bytes()),
        ("xl/_rels/workbook.xml.rels", wb_rels.as_bytes()),
        ("xl/worksheets/sheet1.xml", sheet.as_bytes()),
        ("xl/worksheets/_rels/sheet1.xml.rels", sheet_rels.as_bytes()),
        ("xl/pivotTables/pivotTable1.xml", t1.as_bytes()),
        ("xl/pivotTables/_rels/pivotTable1.xml.rels", table_rels.as_bytes()),
        ("xl/pivotTables/pivotTable2.xml", t2.as_bytes()),
        ("xl/pivotTables/_rels/pivotTable2.xml.rels", table_rels.as_bytes()),
        ("xl/pivotCache/pivotCacheDefinition1.xml", cache_xml.as_bytes()),
        ("xl/pivotCache/_rels/pivotCacheDefinition1.xml.rels", cache_rels.as_bytes()),
        ("xl/pivotCache/pivotCacheRecords1.xml", RECORDS.as_bytes()),
        ("xl/styles.xml", STYLES.as_bytes()),
    ])
}

/// The written PivotTables of sheet "Data": (name, cache part); see [`pivots_on`].
fn written_pivots(u: &Unpacked) -> Vec<(String, String)> {
    pivots_on(u, "Data")
}

/// The written PivotTables of sheet `sheet`: (name, cache part); asserts every `cacheId` names
/// the workbook's entry for that cache, and that each cache's `x14:pivotCacheId` (when it has
/// one) is its `cacheId`, unique in the package.
fn pivots_on(u: &Unpacked, sheet: &str) -> Vec<(String, String)> {
    let wb = u.text("xl/workbook.xml");
    let mut caches: HashMap<String, String> = HashMap::new();
    let mut slicer_ids = std::collections::HashSet::new();
    let mut rest = wb.as_str();
    while let Some(i) = rest.find("<pivotCache ") {
        let id = attr_of(&rest[i..], "pivotCache", "cacheId");
        let rid = attr_of(&rest[i..], "pivotCache", "r:id");
        let (ty, part) = u.target("xl/workbook.xml", &rid);
        assert!(ty.ends_with("/pivotCacheDefinition"));
        let x = u.text(&part);
        if x.contains("<x14:pivotCacheDefinition ") {
            let slicer_id = attr_of(&x, "x14:pivotCacheDefinition", "pivotCacheId");
            assert_eq!(slicer_id, id, "{part}: pivotCacheId is not the cacheId");
            assert!(slicer_ids.insert(slicer_id), "duplicate pivotCacheId");
        }
        assert!(caches.insert(id, part).is_none(), "duplicate cacheId");
        rest = &rest[i + 1..];
    }
    let sheet = sheet_part(u, sheet);
    let mut out = vec![];
    for (_, ty, part, _) in u.rels(&sheet) {
        if !ty.ends_with("/pivotTable") {
            continue;
        }
        let x = u.text(&part);
        let (_, cache) = u.rels(&part).into_iter().find(|r| r.1.ends_with("/pivotCacheDefinition")).map(|r| (r.0, r.2)).unwrap();
        assert_eq!(caches.get(&attr_of(&x, "pivotTableDefinition", "cacheId")), Some(&cache), "{part}: cacheId names another cache");
        out.push((attr_of(&x, "pivotTableDefinition", "name"), cache));
    }
    out
}

/// The written styles: custom number formats (id → code), and whether each differential format
/// is bold.
fn written_styles(u: &Unpacked) -> (HashMap<String, String>, Vec<bool>) {
    let root = xml::parse(u.text("xl/styles.xml").as_bytes()).unwrap();
    let fmts = root
        .child("numFmts")
        .map(|n| n.kids("numFmt").map(|f| (f.attr("numFmtId").unwrap().to_string(), f.attr("formatCode").unwrap().to_string())).collect())
        .unwrap_or_default();
    let dxfs = root.child("dxfs").map(|d| d.kids("dxf").map(|x| x.child("font").and_then(|f| f.child("b")).is_some()).collect()).unwrap_or_default();
    (fmts, dxfs)
}

/// The kept tables use the file's number format 200 (`0.000`) and differential format 1 (bold):
/// both are in the written styles, under the ids the written parts name.
fn assert_styles_resolve(u: &Unpacked, table: &str, cache: &str) -> String {
    let (fmts, dxfs) = written_styles(u);
    let fid = fmts.iter().find(|(_, code)| *code == "0.000").map(|(id, _)| id.clone()).expect("the pivot number format is written");
    let t = u.text(table);
    assert_eq!(attr_of(&t, "dataField", "numFmtId"), fid, "{table}: dataField numFmtId");
    let dxf: usize = attr_of(&t, "format", "dxfId").parse().unwrap();
    assert_eq!(dxfs.get(dxf), Some(&true), "{table}: dxfId {dxf} is not the bold format");
    let c = u.text(cache);
    assert!(c.contains(&format!(r#"<cacheField name="V" numFmtId="{fid}">"#)), "{cache}: cacheField numFmtId");
    fid
}

/// The part of the PivotTable named `name` on sheet "Data".
fn table_part(u: &Unpacked, name: &str) -> String {
    let sheet = sheet_part(u, "Data");
    u.rels(&sheet).into_iter().map(|r| r.2).find(|p| p.contains("pivotTable") && attr_of(&u.text(p), "pivotTableDefinition", "name") == name).unwrap()
}

#[test]
fn pivot_tables_on_a_named_formula_source_are_kept() {
    let (wb, report) = read_xlsx(&package(r#"<worksheetSource name="rData"/>"#, false)).unwrap();
    assert_eq!(wb.sheet(0).unwrap().pivots.len(), 2, "{:?}", report.warnings);
    let out = write_xlsx(&wb).unwrap();
    let u = Unpacked::new(&out);
    u.assert_valid();
    let pivots = written_pivots(&u);
    assert_eq!(pivots.iter().map(|p| p.0.as_str()).collect::<Vec<_>>(), ["P1", "P2"]);
    assert_eq!(pivots[0].1, pivots[1].1, "the shared cache is written once");
    let cache = &pivots[0].1;
    let fid = assert_styles_resolve(&u, &table_part(&u, "P1"), cache);
    let cache_id = attr_of(&u.text(&table_part(&u, "P1")), "pivotTableDefinition", "cacheId");
    let expected = self::cache(r#"<worksheetSource name="rData"/>"#)
        .replace(r#"numFmtId="200""#, &format!(r#"numFmtId="{fid}""#))
        .replace(r#"pivotCacheId="1""#, &format!(r#"pivotCacheId="{cache_id}""#));
    assert_eq!(u.text(cache), expected, "only the style and cache ids change");
    let (_, ty, records, _) = u.rels(cache).into_iter().next().unwrap();
    assert!(ty.ends_with("/pivotCacheRecords"));
    assert_eq!(u.text(&records), RECORDS);
    // And again.
    let (again, _) = read_xlsx(&out).unwrap();
    let u2 = Unpacked::new(&write_xlsx(&again).unwrap());
    u2.assert_valid();
    assert_eq!(written_pivots(&u2).len(), 2);
    assert_styles_resolve(&u2, &table_part(&u2, "P2"), &written_pivots(&u2)[0].1);
}

#[test]
fn pivot_cache_keeps_its_link_to_another_workbook() {
    let source = format!(r#"<worksheetSource xmlns:link="{NS_R}" ref="A1:B3" sheet="Data" link:id="rId2"/>"#);
    let (wb, _) = read_xlsx(&package(&source, true)).unwrap();
    assert!(wb.sheets[0].pivots.iter().all(|pt| pt.source.contains("[")), "the aliased relationship must resolve as external");
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    let pivots = written_pivots(&u);
    assert_eq!(pivots.len(), 2);
    let cache = &pivots[0].1;
    assert!(u.text(cache).contains(&source), "the source still names the other workbook");
    assert_eq!(u.target(cache, "rId2"), (format!("{NS_R}/externalLinkPath"), OTHER_BOOK.to_string()));
}

#[test]
fn a_changed_pivot_table_is_no_longer_kept() {
    let (mut wb, _) = read_xlsx(&package(r#"<worksheetSource name="rData"/>"#, false)).unwrap();
    Arc::make_mut(&mut wb.sheets[0]).pivots[0].name = "Renamed".into();
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    // GridCraft can't rebuild a cache over `OFFSET(…)`: the changed table isn't written; the
    // unchanged one keeps the cache.
    let pivots = written_pivots(&u);
    assert_eq!(pivots.iter().map(|p| p.0.as_str()).collect::<Vec<_>>(), ["P2"]);
}

#[test]
fn rebuilt_named_source_uses_only_unique_unqualified_attributes_and_keeps_x14_id() {
    let source = r#"<worksheetSource xmlns:e="urn:example" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" mc:Ignorable="e" e:name="wrong" e:ref="future" e:id="rId2" id="rId3" name="rData" sheet="Data" ref="A1:B3"/>"#;
    let (mut wb, _) = read_xlsx(&package_with_cache(&cache(source), false, "Data!$A$1:$B$3")).unwrap();
    assert!(wb.sheets[0].pivots.iter().all(|pt| pt.binding.0.is_some()));
    Arc::make_mut(&mut wb.sheets[0]).pivots[0].name = "Rebuilt".into();
    let out = write_xlsx(&wb).unwrap();
    let u = Unpacked::new(&out);
    u.assert_valid();
    let mut written = written_pivots(&u);
    written.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(written.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(), ["P2", "Rebuilt"]);
    assert_ne!(written[0].1, written[1].1);
    for (name, part) in written {
        let xml = u.text(&part);
        assert_eq!(xml.matches("<x14:pivotCacheDefinition ").count(), 1, "{name}: {xml}");
        if name == "P2" {
            assert!(xml.contains(source), "kept source bytes changed: {xml}");
            continue;
        }
        assert_eq!(name, "Rebuilt");
        let mut reader = quick_xml::NsReader::from_str(&xml);
        let mut found = false;
        loop {
            match reader.read_event().unwrap() {
                quick_xml::events::Event::Empty(e) | quick_xml::events::Event::Start(e) if e.local_name().as_ref() == b"worksheetSource" => {
                    assert!(!found);
                    found = true;
                    let attrs = e
                        .attributes()
                        .map(|a| {
                            let a = a.unwrap();
                            (a.key.as_ref().to_vec(), a.value.into_owned())
                        })
                        .collect::<Vec<_>>();
                    assert_eq!(
                        attrs,
                        [(b"name".to_vec(), b"rData".to_vec()), (b"sheet".to_vec(), b"Data".to_vec()), (b"ref".to_vec(), b"A1:B3".to_vec())]
                    );
                }
                quick_xml::events::Event::Eof => break,
                _ => {}
            }
        }
        assert!(found);
    }
    let (back, _) = read_xlsx(&out).unwrap();
    assert_eq!(back.sheets[0].pivots.len(), 2);
    assert!(
        back.sheets[0].pivots.iter().all(|pt| back.pivot_source(pt, 0).is_ok_and(|p| matches!(p.source, gridcraft_model::PivotSource::Name { .. })))
    );
}

#[test]
fn a_foreign_cache_id_is_neither_rewritten_nor_copied_into_a_rebuild() {
    let fake = r#"<foreign:pivotCacheDefinition xmlns:foreign="urn:example" pivotCacheId="72"/>"#;
    let xml = cache(r#"<worksheetSource name="rData"/>"#).replace(r#"<x14:pivotCacheDefinition pivotCacheId="1"/>"#, fake);
    let (mut wb, _) = read_xlsx(&package_with_cache(&xml, false, "Data!$A$1:$B$3")).unwrap();
    assert!(!wb.sheets[0].pivots[0].had_x14_cache_id.0);
    Arc::make_mut(&mut wb.sheets[0]).pivots[0].name = "Rebuilt".into();
    let (u, warnings) = saved(&wb);
    assert!(warnings.is_empty(), "{warnings:?}");
    let xml_of = |name| {
        let table = table_part(&u, name);
        let cache = u.rels(&table).into_iter().find(|r| r.1.ends_with("/pivotCacheDefinition")).unwrap().2;
        u.text(&cache)
    };
    assert!(xml_of("P2").contains(fake));
    assert!(!xml_of("Rebuilt").contains("pivotCacheId="));
}

#[test]
fn an_edited_external_pivot_is_not_rebuilt_over_a_local_sheet() {
    // The source names sheet "Data" of another workbook; this workbook has a sheet "Data" too.
    let source = r#"<worksheetSource ref="A1:B3" sheet="Data" r:id="rId2"/>"#;
    let (mut wb, _) = read_xlsx(&package(source, true)).unwrap();
    Arc::make_mut(&mut wb.sheets[0]).pivots[0].name = "Renamed".into();
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    let pivots = written_pivots(&u);
    assert_eq!(pivots.iter().map(|p| p.0.as_str()).collect::<Vec<_>>(), ["P2"], "the edited table is not written from local data");
    assert!(u.text(&pivots[0].1).contains(source));
}

#[test]
fn kept_rebuilt_and_slicer_caches_have_distinct_ids() {
    use gridcraft_model::{
        Anchor, AnchorMode, PivotField, PivotFunc, PivotSort, PivotTable, PivotValue, Slicer, SlicerCrossFilter, SlicerPivot, SlicerSource,
    };
    let (mut wb, _) = read_xlsx(&package(r#"<worksheetSource name="rData"/>"#, false)).unwrap();
    let at = |s: &str| gridcraft_core::CellRef::parse(s).unwrap();
    let sheet = Arc::make_mut(&mut wb.sheets[0]);
    for (row, (k, v)) in
        [("K", gridcraft_core::Value::text("V")), ("a", gridcraft_core::Value::Number(21.0)), ("b", gridcraft_core::Value::Number(22.0))]
            .into_iter()
            .enumerate()
    {
        sheet.set_value(gridcraft_core::CellRef::new(row as u32 + 9, 0), gridcraft_core::Value::text(k));
        sheet.set_value(gridcraft_core::CellRef::new(row as u32 + 9, 1), v);
    }
    for (name, anchor, source) in [("Local", "K3", "Data!$A$1:$B$3"), ("Sliced", "N3", "Data!$A$10:$B$12")] {
        sheet.pivots.push(PivotTable {
            id: 900 + sheet.pivots.len() as u32,
            name: name.into(),
            source: source.into(),
            anchor: at(anchor),
            rows: vec![PivotField { source_col: "K".into(), ..PivotField::default() }],
            values: vec![PivotValue { source_col: "V".into(), func: PivotFunc::Sum, name: "Sum of V".into(), ..PivotValue::default() }],
            ..PivotTable::default()
        });
    }
    sheet.slicers.push(Slicer {
        id: 1,
        source: SlicerSource::Pivot {
            pivots: vec![SlicerPivot { sheet: "Data".into(), name: "Sliced".into() }],
            field: "K".into(),
            sort: PivotSort::Asc,
            cross_filter: SlicerCrossFilter::ShowItemsWithDataAtTop,
            show_missing: true,
        },
        caption: "K".into(),
        anchor: Anchor { cell: at("R1"), dx: 0.0, dy: 0.0, width: 180.0, height: 240.0, mode: AnchorMode::default() },
        columns: 1,
        name: "K".into(),
        style: String::new(),
        row_height: None,
    });
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    // `written_pivots` checks cacheId and pivotCacheId are unique and agree.
    let pivots = written_pivots(&u);
    let names: Vec<&str> = pivots.iter().map(|p| p.0.as_str()).collect();
    for n in ["P1", "P2", "Local", "Sliced"] {
        assert!(names.contains(&n), "{n} written: {names:?}");
    }
    let cache_of = |n: &str| pivots.iter().find(|p| p.0 == n).unwrap().1.clone();
    assert_ne!(cache_of("P1"), cache_of("Sliced"));
    assert_ne!(cache_of("Local"), cache_of("Sliced"));
    assert_eq!(cache_of("P1"), cache_of("P2"));
    assert!(!u.text(&cache_of("Local")).contains("pivotCacheId="));
    for (pivot, values) in [("P1", ["1", "2"]), ("P2", ["1", "2"]), ("Local", ["1", "2"]), ("Sliced", ["21", "22"])] {
        let def = cache_of(pivot);
        let rid = attr_of(&u.text(&def), "pivotCacheDefinition", "r:id");
        let (kind, part) = u.target(&def, &rid);
        assert!(kind.ends_with("/pivotCacheRecords"));
        let xml = u.text(&part);
        let actual: Vec<_> = xml.split("<n v=\"").skip(1).filter_map(|s| s.split('"').next()).collect();
        assert_eq!(actual, values, "records related to {pivot}");
    }
    // The slicer cache names the sliced table's cache by its id.
    let sliced_id = attr_of(&u.text(&table_part(&u, "Sliced")), "pivotTableDefinition", "cacheId");
    let kept_id = attr_of(&u.text(&table_part(&u, "P1")), "pivotTableDefinition", "cacheId");
    assert_ne!(sliced_id, kept_id);
    let slicer_cache = u.parts.keys().find(|k| k.starts_with("xl/slicerCaches/") && !k.contains("_rels")).expect("a slicer cache is written");
    assert_eq!(attr_of(&u.text(slicer_cache), "tabular", "pivotCacheId"), sliced_id);
}

#[test]
fn a_pivot_whose_source_was_already_missing_is_kept() {
    // The file's cache names a source that isn't in it: the table and its cache go back as they were.
    let (wb, _) = read_xlsx(&package(r#"<worksheetSource name="Missing"/>"#, false)).unwrap();
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    assert_eq!(written_pivots(&u).len(), 2);
}

#[test]
fn removing_the_source_name_drops_the_kept_pivots() {
    let (mut wb, _) = read_xlsx(&package(r#"<worksheetSource name="rData"/>"#, false)).unwrap();
    wb.names.retain(|n| !n.name.eq_ignore_ascii_case("rData"));
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    assert!(written_pivots(&u).is_empty(), "a table over a name that is gone is not written");
    assert!(!u.parts.keys().any(|k| k.starts_with("xl/pivotCache/")), "no orphan cache");
}

// ---------------------------------------------------------------- a sheet's own source name

/// Sheets "Report" (K/V in A10:B12, values 901 and 902) and `data` (K/V in A1:B3, values 1 and
/// 2). The workbook's `rData` is Report!A10:B12; with `local`, sheet `data` has its own `rData`,
/// its A1:B3. A PivotTable on Report has a cache whose source is `name="rData" sheet="{data}"`.
fn scoped_package(data: &str, local: bool) -> Vec<u8> {
    scoped_package_with(data, local, &format!(r#"name="rData" sheet="{}""#, crate::xml::esc_attr(data)))
}

/// [`scoped_package`] with the cache's `worksheetSource` attributes `source`.
fn scoped_package_with(data: &str, local: bool, source: &str) -> Vec<u8> {
    use crate::xml::esc_attr;
    let types = format!(
        r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Override PartName="/xl/workbook.xml" ContentType="{CT_PREFIX}.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="{CT_PREFIX}.worksheet+xml"/><Override PartName="/xl/worksheets/sheet2.xml" ContentType="{CT_PREFIX}.worksheet+xml"/><Override PartName="/xl/pivotTables/pivotTable1.xml" ContentType="{CT_PREFIX}.pivotTable+xml"/><Override PartName="/xl/pivotCache/pivotCacheDefinition1.xml" ContentType="{CT_PREFIX}.pivotCacheDefinition+xml"/><Override PartName="/xl/pivotCache/pivotCacheRecords1.xml" ContentType="{CT_PREFIX}.pivotCacheRecords+xml"/><Override PartName="/xl/styles.xml" ContentType="{CT_PREFIX}.styles+xml"/></Types>"#
    );
    let local_name = if local {
        format!(r#"<definedName name="rData" localSheetId="1">{}!$A$1:$B$3</definedName>"#, crate::xml::esc(&gridcraft_core::quote_sheet(data)))
    } else {
        String::new()
    };
    let workbook = format!(
        r#"<?xml version="1.0"?><workbook xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheets><sheet name="Report" sheetId="1" r:id="rId1"/><sheet name="{}" sheetId="2" r:id="rId4"/></sheets><definedNames>{local_name}<definedName name="rData">Report!$A$10:$B$12</definedName></definedNames><pivotCaches><pivotCache cacheId="7" r:id="rId2"/></pivotCaches></workbook>"#,
        esc_attr(data)
    );
    let c = |r: &str, v: &str| {
        if v.parse::<f64>().is_ok() {
            format!(r#"<c r="{r}"><v>{v}</v></c>"#)
        } else {
            format!(r#"<c r="{r}" t="inlineStr"><is><t>{v}</t></is></c>"#)
        }
    };
    let rows = |first: u32, v: [&str; 2]| {
        let mut s = String::new();
        for (i, (k, v)) in [("K", "V"), ("a", v[0]), ("b", v[1])].into_iter().enumerate() {
            let r = first + i as u32;
            s.push_str(&format!(r#"<row r="{r}">{}{}</row>"#, c(&format!("A{r}"), k), c(&format!("B{r}"), v)));
        }
        s
    };
    let report = format!(
        r#"<?xml version="1.0"?><worksheet xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheetData>{}</sheetData></worksheet>"#,
        rows(10, ["901", "902"])
    );
    let data_sheet =
        format!(r#"<?xml version="1.0"?><worksheet xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheetData>{}</sheetData></worksheet>"#, rows(1, ["1", "2"]));
    let wb_rels = rels(&[
        ("rId1", "worksheet", "worksheets/sheet1.xml"),
        ("rId2", "pivotCacheDefinition", "pivotCache/pivotCacheDefinition1.xml"),
        ("rId3", "styles", "styles.xml"),
        ("rId4", "worksheet", "worksheets/sheet2.xml"),
    ]);
    let sheet_rels = rels(&[("rId1", "pivotTable", "../pivotTables/pivotTable1.xml")]);
    let table_rels = rels(&[("rId1", "pivotCacheDefinition", "../pivotCache/pivotCacheDefinition1.xml")]);
    let cache_rels = rels(&[("rId1", "pivotCacheRecords", "pivotCacheRecords1.xml")]);
    let (t1, cx) = (table("P1", "D3:E6"), cache(&format!("<worksheetSource {source}/>")));
    make_zip(&[
        ("[Content_Types].xml", types.as_bytes()),
        ("_rels/.rels", super::super::ROOT_RELS.as_bytes()),
        ("xl/workbook.xml", workbook.as_bytes()),
        ("xl/_rels/workbook.xml.rels", wb_rels.as_bytes()),
        ("xl/worksheets/sheet1.xml", report.as_bytes()),
        ("xl/worksheets/_rels/sheet1.xml.rels", sheet_rels.as_bytes()),
        ("xl/worksheets/sheet2.xml", data_sheet.as_bytes()),
        ("xl/pivotTables/pivotTable1.xml", t1.as_bytes()),
        ("xl/pivotTables/_rels/pivotTable1.xml.rels", table_rels.as_bytes()),
        ("xl/pivotCache/pivotCacheDefinition1.xml", cx.as_bytes()),
        ("xl/pivotCache/_rels/pivotCacheDefinition1.xml.rels", cache_rels.as_bytes()),
        ("xl/pivotCache/pivotCacheRecords1.xml", RECORDS.as_bytes()),
        ("xl/styles.xml", STYLES.as_bytes()),
    ])
}

/// The written cache of Report's only PivotTable, if it has one.
fn report_cache(u: &Unpacked) -> Option<String> {
    let pivots = pivots_on(u, "Report");
    assert!(pivots.len() <= 1);
    pivots.first().map(|p| u.text(&p.1))
}

#[test]
fn a_sheet_names_source_is_bound_to_that_sheet() {
    let (wb, report) = read_xlsx(&scoped_package("Data", true)).unwrap();
    assert_eq!(wb.sheet(0).unwrap().pivots[0].source, "Data!rData", "{:?}", report.warnings);
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    assert!(report_cache(&u).expect("kept").contains(r#"<worksheetSource name="rData" sheet="Data"/>"#));
}

#[test]
fn an_options_edit_rebuilds_from_the_same_binding() {
    let (mut wb, _) = read_xlsx(&scoped_package("Data", true)).unwrap();
    let pt = &mut Arc::make_mut(&mut wb.sheets[0]).pivots[0];
    pt.enable_show_details = !pt.enable_show_details;
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    let cache = report_cache(&u).expect("rebuilt");
    assert!(cache.contains(r#"<worksheetSource name="rData" sheet="Data"/>"#), "{cache}");
    assert!(!cache.contains("901") && cache.contains(r#"maxValue="2""#), "sheet Data's data, not the workbook name's: {cache}");
}

#[test]
fn the_attributes_read_go_back_as_they_were_while_they_still_read_back() {
    const SOURCE: &str = r#"<worksheetSource name="rData" sheet="Data" ref="A1:B3"/>"#;
    for rebuilt in [false, true] {
        let (mut wb, _) = read_xlsx(&scoped_package_with("Data", true, r#"name="rData" sheet="Data" ref="A1:B3""#)).unwrap();
        if rebuilt {
            let pt = &mut Arc::make_mut(&mut wb.sheets[0]).pivots[0];
            pt.enable_show_details = !pt.enable_show_details;
        }
        let (bytes, report) = crate::write_xlsx_named_with(&wb, crate::XlsxKind::Workbook, "book.xlsx", crate::FunctionInfo::default()).unwrap();
        let u = Unpacked::new(&bytes);
        u.assert_valid();
        let cache = report_cache(&u).expect("written");
        assert!(cache.contains(SOURCE), "rebuilt: {rebuilt}: {cache}");
        assert_eq!(cache.contains(r#"refreshOnLoad="1""#), rebuilt, "kept or rebuilt as asked: {cache}");
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        let (back, _) = read_xlsx(&bytes).unwrap();
        let local = back.names.iter().find(|n| n.scope == Some(1)).unwrap().uid;
        let pt = &back.sheet(0).unwrap().pivots[0];
        assert_eq!(pt.binding.0.as_ref().map(|b| b.def.get()), Some(local.get()), "bound to sheet Data's rData again");
    }
    // Once the source is another text (its sheet renamed, which the name's reference follows),
    // the attributes read no longer say it.
    let (mut wb, _) = read_xlsx(&scoped_package_with("Data", true, r#"name="rData" sheet="Data" ref="A1:B3""#)).unwrap();
    Arc::make_mut(&mut wb.sheets[1]).name = "Other".into();
    wb.names.iter_mut().find(|n| n.scope == Some(1)).unwrap().formula = "Other!$A$1:$B$3".into();
    let pt = &mut Arc::make_mut(&mut wb.sheets[0]).pivots[0];
    pt.source = "Other!rData".into();
    pt.binding.0.as_mut().expect("bound").text = pt.source.clone();
    pt.enable_show_details = !pt.enable_show_details;
    let (u, warnings) = saved(&wb);
    let cache = report_cache(&u).unwrap_or_else(|| panic!("rebuilt: {warnings:?}"));
    assert!(cache.contains(r#"<worksheetSource name="rData" sheet="Other"/>"#), "{cache}");
}

#[test]
fn a_deleted_sheet_name_is_not_replaced_by_the_workbooks() {
    let (mut wb, _) = read_xlsx(&scoped_package("Data", true)).unwrap();
    wb.names.retain(|n| n.scope.is_none());
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    assert_eq!(report_cache(&u), None, "neither kept nor rebuilt over the workbook's rData");
    assert!(!u.parts.keys().any(|k| k.starts_with("xl/pivotCache/")), "no orphan cache");
}

#[test]
fn a_renamed_source_sheet_is_renamed_in_the_kept_cache() {
    let (mut wb, _) = read_xlsx(&scoped_package("My 'Data'", true)).unwrap();
    assert_eq!(wb.sheet(0).unwrap().pivots[0].source, "'My ''Data'''!rData");
    // What `sheet.rename` does: the sheet, the PivotTable's source qualifier and the text its
    // binding was made for.
    Arc::make_mut(&mut wb.sheets[1]).name = "O'Brien & Co".into();
    let pt = &mut Arc::make_mut(&mut wb.sheets[0]).pivots[0];
    pt.source = "'O''Brien & Co'!rData".into();
    pt.binding.0.as_mut().expect("bound").text = pt.source.clone();
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    let cache = report_cache(&u).expect("kept");
    assert!(cache.contains(r#"<worksheetSource name="rData" sheet="O'Brien &amp; Co"/>"#), "{cache}");
    assert!(cache.contains(r#"recordCount="2""#), "the kept cache, not a rebuilt one: {cache}");
}

#[test]
fn a_stale_sheet_qualifier_still_stands_for_its_definition() {
    let (mut wb, _) = read_xlsx(&scoped_package("Data", true)).unwrap();
    // Renamed without the source following: the source is still bound to sheet Data's rData,
    // which the save names as it is now so that it reads back to it.
    Arc::make_mut(&mut wb.sheets[1]).name = "Other".into();
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    let cache = report_cache(&u).expect("kept");
    assert!(cache.contains(r#"<worksheetSource name="rData" sheet="Other"/>"#) && cache.contains(r#"recordCount="2""#), "{cache}");
}

#[test]
fn a_sheet_name_already_missing_keeps_the_cache_but_never_takes_the_workbooks() {
    // The file's source names sheet Data's rData, which it doesn't have: the cache goes back as it
    // was while the table is unchanged; edited, it is not rebuilt over the workbook's rData.
    let (wb, _) = read_xlsx(&scoped_package("Data", false)).unwrap();
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    assert!(report_cache(&u).expect("kept").contains(r#"sheet="Data""#));
    let mut wb = wb;
    let pt = &mut Arc::make_mut(&mut wb.sheets[0]).pivots[0];
    pt.enable_show_details = !pt.enable_show_details;
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    assert_eq!(report_cache(&u), None);
}

#[test]
fn a_bare_names_binding_to_its_sheets_name_is_kept_only_while_it_holds() {
    // The cache's source is a bare `rData`; sheet Report, the PivotTable's, has its own rData
    // (Data!A1:B3), which shadows the workbook's (Report!A10:B12).
    let mut parts = Unpacked::new(&scoped_package("Data", true)).parts;
    let fix = |parts: &mut HashMap<String, Vec<u8>>, name: &str, from: &str, to: &str| {
        let text = String::from_utf8(parts[name].clone()).unwrap();
        assert!(text.contains(from), "{name}");
        parts.insert(name.into(), text.replace(from, to).into_bytes());
    };
    fix(&mut parts, "xl/workbook.xml", r#"localSheetId="1""#, r#"localSheetId="0""#);
    fix(
        &mut parts,
        "xl/pivotCache/pivotCacheDefinition1.xml",
        r#"<worksheetSource name="rData" sheet="Data"/>"#,
        r#"<worksheetSource name="rData"/>"#,
    );
    let list: Vec<(&str, &[u8])> = parts.iter().map(|(k, v)| (k.as_str(), v.as_slice())).collect();
    let (wb, report) = read_xlsx(&make_zip(&list)).unwrap();
    assert_eq!(wb.sheet(0).unwrap().pivots[0].source, "rData", "{:?}", report.warnings);
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    assert!(report_cache(&u).expect("kept").contains(r#"maxValue="2""#));
    // Report's rData deleted, the bare name now means the workbook's: the cache kept for Report's
    // is not written for it.
    let mut wb = wb;
    wb.names.retain(|n| n.scope.is_none());
    let u = Unpacked::new(&write_xlsx(&wb).unwrap());
    u.assert_valid();
    assert!(report_cache(&u).is_none_or(|c| !c.contains(r#"maxValue="2""#)), "the kept cache is not written over another binding");
}

/// The written package and what the save said.
fn saved(wb: &gridcraft_model::Workbook) -> (Unpacked, Vec<String>) {
    let (bytes, report) = crate::write_xlsx_named_with(wb, crate::XlsxKind::Workbook, "book.xlsx", crate::FunctionInfo::default()).unwrap();
    let u = Unpacked::new(&bytes);
    u.assert_valid();
    (u, report.warnings)
}

#[test]
fn a_renamed_source_name_is_followed_by_the_source_and_the_cache() {
    let (mut wb, _) = read_xlsx(&scoped_package("Data", true)).unwrap();
    // What `formulas.defineName` with `oldName` does: the name keeps its identity under its new
    // name, and what is bound to it follows.
    let n = wb.names.iter_mut().find(|n| n.scope == Some(1)).unwrap();
    n.name = "rNew".into();
    let uid = n.uid;
    wb.follow_renamed_definition(uid);
    assert_eq!(wb.sheet(0).unwrap().pivots[0].source, "Data!rNew");
    let (u, warnings) = saved(&wb);
    let cache = report_cache(&u).expect("rebuilt under the new name");
    assert!(cache.contains(r#"<worksheetSource name="rNew" sheet="Data"/>"#), "{cache}");
    assert!(cache.contains(r#"maxValue="2""#), "sheet Data's data: {cache}");
    assert!(!warnings.iter().any(|w| w.contains("PivotTable")), "{warnings:?}");
}

#[test]
fn a_source_name_made_again_is_another_definition_and_the_save_says_so() {
    let (mut wb, _) = read_xlsx(&scoped_package("Data", true)).unwrap();
    // Deleted and defined again over the same cells: equal text, but not the name it was bound to.
    let i = wb.names.iter().position(|n| n.scope == Some(1)).unwrap();
    let again = gridcraft_model::DefinedName { uid: gridcraft_model::DefinitionUid::fresh(), ..wb.names.remove(i) };
    wb.names.push(again);
    let (u, warnings) = saved(&wb);
    let cache = report_cache(&u).expect("rebuilt from the name that took its place");
    assert!(cache.contains(r#"<worksheetSource name="rData" sheet="Data"/>"#), "{cache}");
    assert!(warnings.iter().any(|w| w.contains("rebuilt from a different source") && w.contains(r#"sheet "Data"'s name "rData""#)), "{warnings:?}");
}

#[test]
fn an_edited_source_name_keeps_its_binding_and_the_kept_cache() {
    let (mut wb, _) = read_xlsx(&scoped_package("Data", true)).unwrap();
    wb.names.iter_mut().find(|n| n.scope == Some(1)).unwrap().formula = "Data!$A$1:$B$2".into();
    let (u, warnings) = saved(&wb);
    assert!(report_cache(&u).expect("kept").contains(r#"recordCount="2""#));
    assert!(warnings.is_empty(), "{warnings:?}");
}
