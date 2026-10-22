use std::error::Error;
use std::io::{Cursor, Write};

use gridcraft_calc::Calc;
use gridcraft_core::{CellRef, RangeRef, Value};
use gridcraft_model::{Cell, Formula, Workbook};
use gridcraft_xlsx::{read_xlsx, write_xlsx};

fn fixture(body: Option<&str>) -> Result<Vec<u8>, Box<dyn Error>> {
    let parts = [
        (
            "[Content_Types].xml",
            r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/></Types>"#,
        ),
        (
            "_rels/.rels",
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="wb" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
        ),
        (
            "xl/workbook.xml",
            r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Data" sheetId="1" r:id="sheet"/></sheets><externalReferences><externalReference r:id="link"/></externalReferences></workbook>"#,
        ),
        (
            "xl/_rels/workbook.xml.rels",
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="sheet" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="link" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/externalLink" Target="externalLinks/externalLink1.xml"/><Relationship Id="styles" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#,
        ),
        (
            "xl/externalLinks/externalLink1.xml",
            r#"<externalLink xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><oleLink progId="Unavailable"/></externalLink>"#,
        ),
        (
            "xl/styles.xml",
            r#"<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><fonts count="2"><font/><font><b/></font></fonts><cellXfs count="2"><xf/><xf fontId="1" applyFont="1"/></cellXfs></styleSheet>"#,
        ),
        (
            "xl/worksheets/sheet1.xml",
            r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1"><f t="array" ref="A1:B2">[1]Data!A1:B2</f><v>11</v></c><c r="B1"><v>22</v></c><c r="D1"><f t="array" ref="D1:E2">{1,2;3,4}</f><v>91</v></c><c r="E1"><v>92</v></c><c r="G1"><f>1+2</f><v>999</v></c></row><row r="2"><c r="A2"><v>33</v></c><c r="B2" s="1"><v>44</v></c><c r="D2"><v>93</v></c><c r="E2" s="1"><v>94</v></c><c r="G2"><f>[1]Data!Z1</f><v>77</v></c></row><row r="3"><c r="G3"><f>SUM(A1:B2)</f><v>0</v></c></row></sheetData></worksheet>"#,
        ),
    ];
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, xml) in parts {
        let xml = if name == "xl/worksheets/sheet1.xml" { body.unwrap_or(xml) } else { xml };
        zip.start_file(name, zip::write::SimpleFileOptions::default())?;
        zip.write_all(xml.as_bytes())?;
    }
    Ok(zip.finish()?.into_inner())
}

fn check_legacy(wb: &Workbook) {
    let sh = wb.sheet(0).unwrap();
    for (address, expected) in [("A1", 11.0), ("B1", 22.0), ("A2", 33.0), ("B2", 44.0)] {
        assert_eq!(sh.value(CellRef::parse(address).unwrap()), Value::Number(expected), "{address}");
    }
    for (anchor, range) in [("A1", "A1:B2"), ("D1", "D1:E2")] {
        let f = sh.cell(CellRef::parse(anchor).unwrap()).unwrap().formula.as_ref().unwrap();
        assert_eq!(f.array, Some(RangeRef::parse(range).unwrap()));
        assert!(f.legacy && !f.dynamic);
    }
    for address in ["B2", "E2"] {
        let c = sh.cell(CellRef::parse(address).unwrap()).unwrap();
        assert!(c.value.is_empty() && c.formula.is_none());
        assert!(wb.styles.get(c.style).font.bold);
    }
}

#[test]
fn legacy_cse_saved_values_survive_import_and_full_recalculation() -> Result<(), Box<dyn Error>> {
    let (mut wb, _) = read_xlsx(&fixture(None)?)?;
    assert!(wb.external_books[0].opaque);
    check_legacy(&wb);
    assert_eq!(wb.sheet(0).unwrap().value(CellRef::parse("E2").unwrap()), Value::Number(94.0));
    let mut calc = Calc::new();
    for _ in 0..2 {
        calc.recalc_all(&mut wb);
        check_legacy(&wb);
        for (address, expected) in [("D1", 1.0), ("E1", 2.0), ("D2", 3.0), ("E2", 4.0), ("G1", 3.0), ("G2", 77.0), ("G3", 110.0)] {
            assert_eq!(wb.sheet(0).unwrap().value(CellRef::parse(address).unwrap()), Value::Number(expected), "{address}");
        }
    }
    let (saved, _) = read_xlsx(&write_xlsx(&wb)?)?;
    check_legacy(&saved);
    println!(
        "legacy CSE: import + two public Calc::recalc_all calls + XLSX round trip preserve 11,22,33,44; calculable arrays/scalars recompute; styled followers do not obstruct"
    );
    Ok(())
}

#[test]
fn imported_whole_grid_cse_can_be_replaced_without_enumerating_its_range() -> Result<(), Box<dyn Error>> {
    let sheet = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>
      <row r="1048576"><c r="XFD1048576"><v>30</v></c></row>
      <row r="2"><c r="A2"><v>20</v></c></row>
      <row r="1"><c r="A1"><f t="array" ref="A1:XFD1048576">[1]Data!A1:XFD1048576</f><v>10</v></c></row>
    </sheetData></worksheet>"#;
    let (imported, _) = read_xlsx(&fixture(Some(sheet))?)?;
    let (saved, _) = read_xlsx(&write_xlsx(&imported)?)?;
    for replacement in [Cell::value(Value::Number(3.0)), Cell::formula(Formula::new("1+2"))] {
        let mut wb = saved.clone();
        let mut calc = Calc::new();
        calc.rebuild(&wb);
        let anchor = CellRef::parse("A1").unwrap();
        wb.sheet_mut(0).unwrap().cells.set(anchor, replacement);
        calc.cells_changed(&mut wb, &[(0, anchor)]);
        let sh = wb.sheet(0).unwrap();
        assert_eq!(sh.value(anchor), Value::Number(3.0));
        assert_eq!(sh.value(CellRef::parse("A2").unwrap()), Value::Empty);
        assert_eq!(sh.value(CellRef::parse("XFD1048576").unwrap()), Value::Empty);
        assert!(sh.spill.is_empty());
        assert!(sh.spill_ranges.is_empty());
    }
    Ok(())
}
