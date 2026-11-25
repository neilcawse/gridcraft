use gridcraft_core::CellRef;

#[test]
fn imported_formulatext_keeps_layout_and_displays_compatibility_operators() {
    let body = r#"<sheetData><row r="1">
        <c r="A1"><f>A2 * 2</f><v>0</v></c>
        <c r="B1" cm="1"><f>_xlfn._xlws.SORT(A2:A4 + B2:B4)</f><v>0</v></c>
        <c r="C1" cm="1"><f>_xlfn.SINGLE(-A2:A4)</f><v>0</v></c>
        <c r="D1"><f>SUM((_xlfn.ANCHORARRAY(A2), B2))</f><v>0</v></c>
        <c r="E1" cm="1"><f>_xlfn.SINGLE(A2:A4 * 2)</f><v>0</v></c>
        <c r="F1"><f>A2*2</f><v>0</v></c>
        <c r="G1"><f>"_xlfn.SINGLE(A2)" &amp; '_xlfn.Simple'!A2</f><v>0</v></c>
        <c r="H1"><f>T[[#This Row],[A]] + 1</f><v>0</v></c>
        <c r="I1" cm="1"><f>F5:_xlfn.ANCHORARRAY(INDEX(F5:F14, 3))</f><v>0</v></c>
        <c r="J1" cm="1"><f>_xlfn.GROUPBY(A2:A4, B2:B4, _xleta.SUM)</f><v>0</v></c>
        <c r="K1"><f>_xludf.MYFN(A2) + 1</f><v>0</v></c>
        <c r="L1" cm="1"><f>_xlfn._TRO_ALL(A1:E10) + 0</f><v>0</v></c>
    </row></sheetData>"#;
    let (wb, _) = crate::read_xlsx(&super::minimal(body, &[], "", "")).unwrap();
    let sheet = wb.sheet(0).unwrap();
    for (cell, display) in [
        ("A1", Some("A2 * 2")),
        ("B1", Some("SORT(A2:A4 + B2:B4)")),
        ("C1", Some("@-A2:A4")),
        ("D1", Some("SUM((A2#, B2))")),
        ("E1", Some("@(A2:A4 * 2)")),
        ("F1", None),
        ("G1", Some(r#""_xlfn.SINGLE(A2)" & '_xlfn.Simple'!A2"#)),
        // As public Excel caches of FORMULATEXT show it.
        ("I1", Some("F5:INDEX(F5:F14, 3)#")),
        // A file-only spelling the formula bar shows otherwise keeps canonical text: a
        // structured reference's `[#This Row]`, an eta function, a user-defined function's
        // prefix and a trim reference.
        ("H1", None),
        ("J1", None),
        ("K1", None),
        ("L1", None),
    ] {
        let f = sheet.cell(CellRef::parse(cell).unwrap()).unwrap().formula.as_deref().unwrap();
        assert_eq!(f.display_text.as_deref().map(String::as_str), display, "{cell}");
    }
    assert_eq!(sheet.cell(CellRef::new(0, 0)).unwrap().formula.as_deref().unwrap().text, "A2*2");
}

#[test]
fn imported_formulatext_distinguishes_file_tokens_from_user_spellings() -> Result<(), Box<dyn std::error::Error>> {
    let body = r#"<sheetData><row r="1">
        <c r="A1" cm="1"><f>_xlBudget + 1</f><v>0</v></c>
        <c r="B1" cm="1"><f>_xlLambda(A2) + 1</f><v>0</v></c>
        <c r="C1" cm="1"><f>_xlfn.SINGLE(+A2:A4)</f><v>0</v></c>
        <c r="D1" cm="1"><f>_xlfn.SINGLE(( - A2))</f><v>0</v></c>
        <c r="E1" cm="1"><f>[#This Row] + 1</f><v>0</v></c>
        <c r="F1" cm="1"><f>T['#This Row] + 1</f><v>0</v></c>
        <c r="G1" cm="1"><f>_xlfn.SINGLE(A2)+</f><v>0</v></c>
        <c r="H1" cm="1"><f>T[@['#This Row]] + 1</f><v>0</v></c>
        <c r="I1" cm="1"><f>_xlfn.LET(_xlpm._TRO_CUSTOM, _xlfn.LAMBDA(_xlpm.x, _xlpm.x), _xlpm._TRO_CUSTOM(A2))</f><v>0</v></c>
        <c r="J1" cm="1"><f>T[Prior '#This Row] + 1</f><v>0</v></c>
    </row></sheetData>"#;
    let names = r#"<definedNames><definedName name="_xlBudget">3</definedName><definedName name="_XLLAMBDA">_xlfn.LAMBDA(_xlpm.x,_xlpm.x)</definedName></definedNames>"#;
    let (wb, _) = crate::read_xlsx(&super::minimal(body, &[], "", names))?;
    let sheet = wb.sheet(0).ok_or("missing test sheet")?;
    let mut mismatches = Vec::new();
    for (cell, expected) in [
        ("A1", "_xlBudget + 1"),
        ("B1", "_xlLambda(A2) + 1"),
        ("C1", "@+A2:A4"),
        ("D1", "@( - A2)"),
        ("E1", "[@]+1"),
        ("F1", "T['#This Row] + 1"),
        ("G1", "_xlfn.SINGLE(A2)+"),
        ("H1", "T[@['#This Row]] + 1"),
        ("I1", "LET(_TRO_CUSTOM, LAMBDA(x, x), _TRO_CUSTOM(A2))"),
        ("J1", "T[Prior '#This Row] + 1"),
    ] {
        let at = CellRef::parse(cell).ok_or("invalid test address")?;
        let f = sheet.cell(at).and_then(|c| c.formula.as_deref()).ok_or("missing imported formula")?;
        let actual = f.display_text.as_deref().unwrap_or(&f.text);
        if actual != expected {
            mismatches.push((cell, actual.as_str(), expected));
        }
        assert_eq!(f.expr.is_none(), cell == "G1", "{cell}");
    }
    assert!(mismatches.is_empty(), "(cell, actual, expected): {mismatches:?}");
    Ok(())
}

#[test]
fn imported_base_column_width_is_distinct_from_geometry() {
    for (attrs, expected) in [("", Some(10)), (r#" defaultColWidth="12""#, None)] {
        let body = format!(r#"<sheetFormatPr baseColWidth="10"{attrs}/><sheetData/>"#);
        let (wb, _) = crate::read_xlsx(&super::minimal(&body, &[], "", "")).unwrap();
        let sheet = wb.sheet(0).unwrap();
        assert_eq!(sheet.default_col_width, if expected.is_some() { 80.0 } else { 84.0 });
        assert_eq!(sheet.xlsx_base_col_width, expected);
    }
}
