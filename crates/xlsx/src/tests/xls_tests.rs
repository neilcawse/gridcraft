//! Legacy `.xls` (BIFF8) import. Every test file is assembled here from the [MS-CFB] and
//! [MS-XLS] structures; no Excel-produced file is used.

use gridcraft_core::{CellError, CellRef, DateSystem, RangeRef, Value};

use crate::{Format, IoError, read_xls, read_xlsx, sniff, write_xlsx};

const END: u32 = 0xFFFF_FFFE;
const FREE: u32 = 0xFFFF_FFFF;

fn put32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn dir_entry(name: &str, kind: u8, child: u32, start: u32, size: u32) -> [u8; 128] {
    let mut e = [0u8; 128];
    let units: Vec<u16> = name.encode_utf16().collect();
    for (i, u) in units.iter().enumerate() {
        e[i * 2..i * 2 + 2].copy_from_slice(&u.to_le_bytes());
    }
    if !name.is_empty() {
        e[64..66].copy_from_slice(&((units.len() as u16 + 1) * 2).to_le_bytes());
    }
    e[66] = kind;
    e[67] = 1; // black
    put32(&mut e, 68, FREE);
    put32(&mut e, 72, FREE);
    put32(&mut e, 76, child);
    put32(&mut e, 116, start);
    put32(&mut e, 120, size);
    e
}

/// A version 3 compound file (512-byte sectors) whose root holds one stream. Streams under
/// 4096 bytes go to the mini stream, like real files.
pub(crate) fn compound(name: &str, data: &[u8]) -> Vec<u8> {
    let mini = data.len() < 4096;
    let pad = |n: usize, to: usize| n.div_ceil(to) * to;
    // Sector payloads after the FAT: directory, mini FAT (if any), then data.
    let mut payload: Vec<Vec<u8>> = vec![];
    let mut chains: Vec<(usize, usize)> = vec![]; // (first payload index, sector count)
    let (dir_at, minifat_at, data_at);
    let mut dir = vec![];
    let mut minifat = vec![];
    let stream_sectors;
    if mini {
        let m = data.len().div_ceil(64).max(1);
        for i in 0..m {
            minifat.extend_from_slice(&(if i + 1 == m { END } else { i as u32 + 1 }).to_le_bytes());
        }
        minifat.resize(512, 0xFF);
        let mut container = data.to_vec();
        container.resize(pad(data.len().max(1), 64), 0);
        stream_sectors = container.len().div_ceil(512);
        container.resize(stream_sectors * 512, 0);
        dir_at = 0;
        minifat_at = Some(1);
        data_at = 2;
        payload.push(vec![]);
        payload.push(minifat);
        payload.extend(container.chunks(512).map(<[u8]>::to_vec));
        chains.push((data_at, stream_sectors));
        dir.extend_from_slice(&dir_entry("Root Entry", 5, 1, 0, (stream_sectors * 512) as u32));
        dir.extend_from_slice(&dir_entry(name, 2, FREE, 0, data.len() as u32));
    } else {
        let mut body = data.to_vec();
        stream_sectors = body.len().div_ceil(512);
        body.resize(stream_sectors * 512, 0);
        dir_at = 0;
        minifat_at = None;
        data_at = 1;
        payload.push(vec![]);
        payload.extend(body.chunks(512).map(<[u8]>::to_vec));
        chains.push((data_at, stream_sectors));
        dir.extend_from_slice(&dir_entry("Root Entry", 5, 1, END, 0));
        dir.extend_from_slice(&dir_entry(name, 2, FREE, 0, data.len() as u32));
    }
    dir.extend_from_slice(&dir_entry("", 0, FREE, FREE, 0));
    dir.extend_from_slice(&dir_entry("", 0, FREE, FREE, 0));
    // FAT sectors come first; their count depends on the total.
    let mut fat_n = 1;
    while (fat_n + payload.len()).div_ceil(128) > fat_n {
        fat_n += 1;
    }
    let sec = |i: usize| (fat_n + i) as u32;
    // Fix the directory's start sectors now that the layout is known.
    let first_data = sec(data_at);
    if mini {
        put32(&mut dir, 116, first_data); // root: mini stream container
    } else {
        put32(&mut dir, 128 + 116, first_data);
    }
    payload[dir_at] = dir;
    let total = fat_n + payload.len();
    let mut fat = vec![FREE; fat_n * 128];
    for f in fat.iter_mut().take(fat_n) {
        *f = 0xFFFF_FFFD;
    }
    fat[sec(dir_at) as usize] = END;
    if let Some(m) = minifat_at {
        fat[sec(m) as usize] = END;
    }
    for (first, n) in chains {
        for k in 0..n {
            fat[sec(first + k) as usize] = if k + 1 == n { END } else { sec(first + k + 1) };
        }
    }
    let mut out = vec![0u8; 512];
    out[..8].copy_from_slice(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
    out[24..26].copy_from_slice(&0x3Eu16.to_le_bytes());
    out[26..28].copy_from_slice(&3u16.to_le_bytes());
    out[28..30].copy_from_slice(&0xFFFEu16.to_le_bytes());
    out[30..32].copy_from_slice(&9u16.to_le_bytes());
    out[32..34].copy_from_slice(&6u16.to_le_bytes());
    put32(&mut out, 44, fat_n as u32);
    put32(&mut out, 48, sec(dir_at));
    put32(&mut out, 56, 4096);
    put32(&mut out, 60, minifat_at.map_or(END, sec));
    put32(&mut out, 64, u32::from(minifat_at.is_some()));
    put32(&mut out, 68, END);
    for i in 0..109 {
        put32(&mut out, 76 + i * 4, if i < fat_n { i as u32 } else { FREE });
    }
    for f in &fat {
        out.extend_from_slice(&f.to_le_bytes());
    }
    for p in payload {
        let mut p = p;
        p.resize(512, 0);
        out.extend_from_slice(&p);
    }
    assert_eq!(out.len(), 512 * (1 + total));
    out
}

pub(super) fn rec(id: u16, payload: &[u8]) -> Vec<u8> {
    let mut v = id.to_le_bytes().to_vec();
    v.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    v.extend_from_slice(payload);
    v
}

pub(super) fn cat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

pub(super) fn u16s(v: &[u16]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// Cell header: row, column, XF index.
fn cell(r: u16, c: u16, xf: u16) -> Vec<u8> {
    u16s(&[r, c, xf])
}

fn bof(dt: u16) -> Vec<u8> {
    rec(0x0809, &cat(&[&u16s(&[0x0600, dt, 0x0DBB, 0x07CC]), &[0; 8]]))
}

fn eof() -> Vec<u8> {
    rec(0x000A, &[])
}

/// XLUnicodeString with compressed characters.
fn xlstr(s: &str) -> Vec<u8> {
    cat(&[&(s.len() as u16).to_le_bytes(), &[0], s.as_bytes()])
}

/// A FORMULA record with a numeric cached result.
fn formula(r: u16, c: u16, cached: [u8; 8], rgce: &[u8]) -> Vec<u8> {
    let mut p = cell(r, c, 0);
    p.extend_from_slice(&cached);
    p.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // flags, chn
    p.extend_from_slice(&(rgce.len() as u16).to_le_bytes());
    p.extend_from_slice(rgce);
    rec(0x0006, &p)
}

fn num(n: f64) -> [u8; 8] {
    n.to_le_bytes()
}

fn special(kind: u8, v: u8) -> [u8; 8] {
    [kind, 0, v, 0, 0, 0, 0xFF, 0xFF]
}

/// PtgRef (value class) with relative row and column.
fn ptg_ref(r: u16, c: u16) -> Vec<u8> {
    cat(&[&[0x44], &u16s(&[r, c | 0xC000])])
}

fn ptg_int(n: u16) -> Vec<u8> {
    cat(&[&[0x1E], &n.to_le_bytes()])
}

fn exp(r: u16, c: u16) -> Vec<u8> {
    cat(&[&[0x01], &u16s(&[r, c])])
}

/// A workbook with two worksheets and a chart sheet covering the stage-1 features.
fn sample(date1904: bool) -> Vec<u8> {
    // SST: "Hello", then "Wörld" as UTF-16 split across a Continue record.
    let mut sst = cat(&[&2u32.to_le_bytes(), &2u32.to_le_bytes(), &xlstr("Hello")]);
    sst.extend_from_slice(&[5, 0, 0, b'W']);
    let sst_rec = cat(&[&rec(0x00FC, &sst), &rec(0x003C, &cat(&[&[1], &u16s(&[0xF6, 'r' as u16, 'l' as u16, 'd' as u16])]))]);
    let xf = |ifmt: u16| rec(0x00E0, &cat(&[&u16s(&[0, ifmt, 0]), &[0; 14]]));
    // Lbl "Rate" = Data!$A$1 (PtgRef3d, absolute).
    let rate = cat(&[&[0x3A], &1u16.to_le_bytes(), &u16s(&[0, 0])]);
    let lbl = rec(0x0018, &cat(&[&u16s(&[0]), &[0, 4], &u16s(&[rate.len() as u16, 0, 0]), &[0; 4], &[0], b"Rate", &rate]));
    // Print_Area on Sheet1 (built-in name 6, itab 1) = Sheet1!$A$1:$B$2.
    let pa = cat(&[&[0x3B], &0u16.to_le_bytes(), &u16s(&[0, 1, 0, 1])]);
    let print_area = rec(0x0018, &cat(&[&u16s(&[0x0020]), &[0, 1], &u16s(&[pa.len() as u16, 0, 1]), &[0; 4], &[0, 6], &pa]));
    let boundsheet = |pos: u32, kind: u8, name: &str| rec(0x0085, &cat(&[&pos.to_le_bytes(), &[0, kind, name.len() as u8, 0], name.as_bytes()]));
    let globals = |positions: [u32; 3]| {
        cat(&[
            &bof(0x0005),
            &rec(0x0022, &u16s(&[u16::from(date1904)])),
            &rec(0x041E, &cat(&[&164u16.to_le_bytes(), &xlstr("yyyy-mm-dd")])),
            &xf(0),
            &xf(164),
            &xf(10), // built-in 0.00%
            &boundsheet(positions[0], 0, "Sheet1"),
            &boundsheet(positions[1], 0, "Data"),
            &boundsheet(positions[2], 2, "Chart1"),
            &rec(0x01AE, &u16s(&[3, 0x0401])),
            &rec(0x0017, &cat(&[&u16s(&[2]), &u16s(&[0, 0, 0]), &u16s(&[0, 1, 1])])),
            &lbl,
            &print_area,
            &sst_rec,
            &eof(),
        ])
    };
    let mulrk = cat(&[&u16s(&[1, 2]), &u16s(&[0]), &((7u32 << 2) | 2).to_le_bytes(), &u16s(&[2]), &((150u32 << 2) | 3).to_le_bytes(), &u16s(&[3])]);
    let shared = cat(&[&[0x4C], &u16s(&[0, 0xFFFF]), &ptg_int(10), &[0x05]]);
    let array = cat(&[&[0x65], &u16s(&[4, 5, 0xC000, 0xC000]), &ptg_int(2), &[0x05]]);
    let sum = cat(&[&[0x25], &u16s(&[0, 0, 1 | 0xC000, 2 | 0xC000]), &[0x22, 1], &4u16.to_le_bytes()]);
    let concat = cat(&[&ptg_ref(0, 0), &[0x17, 1, 0, b'!'], &[0x08]]);
    let round = cat(&[&ptg_ref(0, 1), &ptg_int(0), &[0x41], &27u16.to_le_bytes()]);
    let name_ref = cat(&[&[0x43], &1u32.to_le_bytes(), &ptg_int(2), &[0x05]]);
    let ref3d = cat(&[&[0x5A], &1u16.to_le_bytes(), &u16s(&[0, 0xC000])]);
    let sheet1 = cat(&[
        &bof(0x0010),
        &rec(0x0225, &u16s(&[0, 255])),
        &rec(0x007D, &u16s(&[0, 0, 20 * 256, 0, 0x0002, 0])),
        &rec(0x0208, &cat(&[&u16s(&[0, 0, 4, 600, 0, 0]), &(0x0140u32).to_le_bytes()])),
        &rec(0x00FD, &cat(&[&cell(0, 0, 0), &0u32.to_le_bytes()])),
        &rec(0x0203, &cat(&[&cell(0, 1, 0), &1.5f64.to_le_bytes()])),
        &rec(0x027E, &cat(&[&cell(0, 2, 0), &((42u32 << 2) | 2).to_le_bytes()])),
        &rec(0x00FD, &cat(&[&cell(0, 3, 0), &1u32.to_le_bytes()])),
        &rec(0x0205, &cat(&[&cell(1, 0, 0), &[1, 0]])),
        &rec(0x0205, &cat(&[&cell(1, 1, 0), &[0x07, 1]])),
        &rec(0x00BD, &mulrk),
        &formula(2, 0, num(3.0), &cat(&[&ptg_ref(0, 1), &ptg_int(2), &[0x05]])),
        &formula(2, 1, num(43.5), &sum),
        &formula(2, 2, special(0, 0), &concat),
        &rec(0x0207, &xlstr("Hello!")),
        &formula(2, 3, special(1, 1), &cat(&[&ptg_ref(0, 1), &ptg_int(1), &[0x0D]])),
        &formula(2, 4, num(14.0), &name_ref),
        &formula(2, 5, num(2.0), &round),
        &formula(2, 6, num(7.0), &ref3d),
        &formula(2, 7, special(2, 0x2A), &[0x1C, 0x2A]),
        &rec(0x0203, &cat(&[&cell(3, 0, 1), &0.0f64.to_le_bytes()])),
        &rec(0x0203, &cat(&[&cell(3, 1, 2), &0.25f64.to_le_bytes()])),
        &rec(0x027E, &cat(&[&cell(4, 0, 0), &((1u32 << 2) | 2).to_le_bytes()])),
        &rec(0x027E, &cat(&[&cell(5, 0, 0), &((2u32 << 2) | 2).to_le_bytes()])),
        &formula(4, 1, num(10.0), &exp(4, 1)),
        &rec(0x04BC, &cat(&[&u16s(&[4, 5]), &[1, 1, 0, 2], &(shared.len() as u16).to_le_bytes(), &shared])),
        &formula(5, 1, num(20.0), &exp(4, 1)),
        &formula(4, 2, num(2.0), &exp(4, 2)),
        &rec(0x0221, &cat(&[&u16s(&[4, 5]), &[2, 2], &[0; 6], &(array.len() as u16).to_le_bytes(), &array])),
        &formula(5, 2, num(4.0), &exp(4, 2)),
        &rec(0x00E5, &cat(&[&u16s(&[1]), &u16s(&[6, 6, 0, 1])])),
        // An embedded chart substream: its records are not cells.
        &bof(0x0020),
        &rec(0x0203, &cat(&[&cell(9, 9, 0), &9.0f64.to_le_bytes()])),
        &eof(),
        &eof(),
    ]);
    let data = cat(&[&bof(0x0010), &rec(0x0203, &cat(&[&cell(0, 0, 0), &7.0f64.to_le_bytes()])), &eof()]);
    let chart = cat(&[&bof(0x0020), &eof()]);
    let len = globals([0; 3]).len() as u32;
    let p1 = len;
    let p2 = p1 + sheet1.len() as u32;
    let p3 = p2 + data.len() as u32;
    cat(&[&globals([p1, p2, p3]), &sheet1, &data, &chart])
}

fn v(wb: &gridcraft_model::Workbook, sheet: usize, a1: &str) -> Value {
    wb.sheets[sheet].value(CellRef::parse(a1).unwrap())
}

fn ftext(wb: &gridcraft_model::Workbook, a1: &str) -> Option<String> {
    wb.sheets[0].cell(CellRef::parse(a1).unwrap()).and_then(|c| c.formula.as_ref()).map(|f| f.text().into_owned())
}

#[test]
fn reads_values_formulas_formats_and_layout() {
    for big in [false, true] {
        let mut stream = sample(false);
        if big {
            // Past the mini stream cutoff: the stream lives in regular sectors.
            stream.extend(std::iter::repeat_n(0u8, 5000));
        }
        let bytes = compound("Workbook", &stream);
        assert_eq!(sniff(&bytes), Format::Xls);
        let (wb, report) = read_xls(&bytes).unwrap();
        assert!(report.warnings[0].contains("Excel 97-2003"), "{:?}", report.warnings);
        assert!(report.warnings.iter().any(|w| w.contains("Chart sheets")));
        assert_eq!(wb.sheets.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Sheet1", "Data"]);
        assert_values_roundtrip(&wb);
        assert_formula_roundtrip(&wb);
        assert_layout_roundtrip(&wb);
        assert_xlsx_conversion(&wb);
    }
    let (wb, _) = read_xls(&compound("Workbook", &sample(true))).unwrap();
    assert_eq!(wb.date_system, DateSystem::D1904);
}

fn assert_values_roundtrip(wb: &gridcraft_model::Workbook) {
    assert_eq!(v(wb, 0, "A1"), Value::from("Hello"));
    assert_eq!(v(wb, 0, "B1"), Value::Number(1.5));
    assert_eq!(v(wb, 0, "C1"), Value::Number(42.0));
    assert_eq!(v(wb, 0, "D1"), Value::from("Wörld"));
    assert_eq!(v(wb, 0, "A2"), Value::Bool(true));
    assert_eq!(v(wb, 0, "B2"), Value::Error(CellError::Div0));
    assert_eq!(v(wb, 0, "C2"), Value::Number(7.0));
    assert_eq!(v(wb, 0, "D2"), Value::Number(1.5));
}

fn assert_formula_roundtrip(wb: &gridcraft_model::Workbook) {
    // Formulas: text plus the cached result.
    assert_eq!(ftext(wb, "A3").as_deref(), Some("B1*2"));
    assert_eq!(v(wb, 0, "A3"), Value::Number(3.0));
    assert_eq!(ftext(wb, "B3").as_deref(), Some("SUM(B1:C1)"));
    assert_eq!(ftext(wb, "C3").as_deref(), Some("A1&\"!\""));
    assert_eq!(v(wb, 0, "C3"), Value::from("Hello!"));
    assert_eq!(ftext(wb, "D3").as_deref(), Some("B1>1"));
    assert_eq!(v(wb, 0, "D3"), Value::Bool(true));
    assert_eq!(ftext(wb, "E3").as_deref(), Some("Rate*2"));
    assert_eq!(ftext(wb, "F3").as_deref(), Some("ROUND(B1,0)"));
    assert_eq!(ftext(wb, "G3").as_deref(), Some("Data!A1"));
    assert_eq!(v(wb, 0, "H3"), Value::Error(CellError::NA));
    // Shared formula: relative offsets resolve per cell.
    assert_eq!(ftext(wb, "B5").as_deref(), Some("A5*10"));
    assert_eq!(ftext(wb, "B6").as_deref(), Some("A6*10"));
    // Array formula: the anchor holds it; the other cell's cached result is dropped so that
    // the formula fills the range on recalculation (as when an .xlsx is read, #156).
    let c5 = wb.sheets[0].cell(CellRef::parse("C5").unwrap()).unwrap().formula.clone().unwrap();
    assert_eq!((c5.text(), c5.array), (std::borrow::Cow::Borrowed("A5:A6*2"), Some(RangeRef::parse("C5:C6").unwrap())));
    assert_eq!(ftext(wb, "C6"), None);
    assert_eq!(v(wb, 0, "C6"), Value::Empty);
}

fn assert_layout_roundtrip(wb: &gridcraft_model::Workbook) {
    // Number formats, merges, sizes, names, print area; embedded chart records ignored.
    let sh = &wb.sheets[0];
    assert_eq!(wb.styles.get(sh.style_id(CellRef::parse("A4").unwrap())).num_fmt.as_str(), "yyyy-mm-dd");
    assert_eq!(wb.styles.get(sh.style_id(CellRef::parse("B4").unwrap())).num_fmt.as_str(), "0.00%");
    assert_eq!(sh.merges, vec![RangeRef::parse("A7:B7").unwrap()]);
    assert!(sh.cols.get(&0).unwrap().size.unwrap() > sh.default_col_width);
    assert_eq!(sh.rows.get(&0).unwrap().size, Some(40.0));
    assert!(!sh.rows.get(&0).unwrap().hidden && sh.rows.get(&0).unwrap().custom);
    assert_eq!(sh.print.print_area, RangeRef::parse("A1:B2"));
    assert_eq!(v(wb, 0, "J10"), Value::Empty);
    assert_eq!(wb.names.len(), 1);
    assert_eq!((wb.names[0].name.as_str(), wb.names[0].formula.as_str()), ("Rate", "Data!$A$1"));
    assert_eq!(v(wb, 1, "A1"), Value::Number(7.0));
    assert_eq!(wb.date_system, DateSystem::D1900);
}

fn assert_xlsx_conversion(wb: &gridcraft_model::Workbook) {
    // The import saves as .xlsx and reads back.
    let (back, _) = read_xlsx(&write_xlsx(wb).unwrap()).unwrap();
    assert_eq!(back.sheets[0].cell(CellRef::parse("A3").unwrap()).unwrap().formula.as_ref().unwrap().text(), "B1*2");
    assert_eq!(back.sheets[0].value(CellRef::parse("C3").unwrap()), Value::from("Hello!"));
}

#[test]
fn text_result_of_a_formula_kept_as_its_value() {
    // A what-if data table cell ({=TABLE(B1,)}) can't be imported as a formula, so its saved result
    // is kept. With the default XF the cell is blank until the String record brings the text.
    let table = cat(&[&[0x02], &u16s(&[0, 0])]);
    let sheet = cat(&[&bof(0x0010), &formula(0, 0, special(0, 0), &table), &rec(0x0207, &xlstr("103a")), &eof()]);
    let boundsheet = |pos: u32| rec(0x0085, &cat(&[&pos.to_le_bytes(), &[0, 0, 6, 0], b"Sheet1"]));
    let len = cat(&[&bof(0x0005), &boundsheet(0), &eof()]).len() as u32;
    let stream = cat(&[&bof(0x0005), &boundsheet(len), &eof(), &sheet]);
    let (wb, report) = read_xls(&compound("Workbook", &stream)).unwrap();
    assert!(report.warnings.iter().any(|w| w.contains("what-if data tables")), "{:?}", report.warnings);
    assert_eq!(ftext(&wb, "A1"), None);
    assert_eq!(v(&wb, 0, "A1"), Value::from("103a"));
}

#[test]
fn refuses_encrypted_old_and_foreign_files() {
    // FILEPASS in the globals: a damaged encryption record must be rejected as such.
    let mut stream = bof(0x0005);
    stream.extend(rec(0x002F, &[1, 0, 1, 0]));
    stream.extend(eof());
    let err = read_xls(&compound("Workbook", &stream)).unwrap_err().to_string();
    assert!(err.contains("encryption record is damaged"), "{err}");
    // A BIFF5 (Excel 5.0/95) workbook keeps its data in a `Book` stream.
    let book = compound("Book", &stream);
    assert_eq!(sniff(&book), Format::Xls);
    assert!(read_xls(&book).unwrap_err().to_string().contains("BIFF5"));
    // Another compound file (a word processor document): not a workbook.
    let doc = compound("WordDocument", &[0u8; 100]);
    assert_eq!(sniff(&doc), Format::LegacyBinary);
    assert!(read_xls(&doc).is_err());
}

#[test]
fn hostile_xls_input_never_panics_or_hangs() {
    let stream = sample(false);
    let good = compound("Workbook", &stream);
    // Truncation anywhere.
    for cut in (0..good.len()).step_by(37) {
        let _ = read_xls(&good[..cut]);
        let _ = sniff(&good[..cut]);
    }
    // Byte mutations in the container and the stream.
    for at in (0..good.len()).step_by(7) {
        for x in [0x00u8, 0x01, 0x7F, 0xFF] {
            let mut b = good.clone();
            b[at] = x;
            let _ = read_xls(&b);
        }
    }
    for at in 0..stream.len() {
        let mut s = stream.clone();
        s[at] ^= 0xA5;
        let _ = read_xls(&compound("Workbook", &s));
    }
    // A FAT chain that loops on itself (big stream in regular sectors).
    let mut big = stream.clone();
    big.extend(std::iter::repeat_n(0u8, 5000));
    let mut b = compound("Workbook", &big);
    let fat_at = 512;
    let first_data = 2usize; // FAT, directory, then data
    put32(&mut b, fat_at + (first_data + 3) * 4, first_data as u32);
    assert!(read_xls(&b).is_err());
    // Huge declared counts: shared strings, merges, external sheets.
    let mut huge = bof(0x0005);
    huge.extend(rec(0x00FC, &cat(&[&u32::MAX.to_le_bytes(), &u32::MAX.to_le_bytes()])));
    huge.extend(eof());
    assert!(matches!(read_xls(&compound("Workbook", &huge)), Err(IoError::TooLarge(_))));
    let mut huge = bof(0x0005);
    huge.extend(rec(0x00FC, &cat(&[&9u32.to_le_bytes(), &1_000_000u32.to_le_bytes(), &xlstr("x")])));
    huge.extend(rec(0x0017, &u16s(&[0xFFFF])));
    huge.extend(eof());
    assert!(read_xls(&compound("Workbook", &huge)).is_err()); // no worksheets
    // A sheet pointing outside the stream and a stream size larger than the file.
    let mut bad = bof(0x0005);
    bad.extend(rec(0x0085, &cat(&[&u32::MAX.to_le_bytes(), &[0, 0, 1, 0], b"S"])));
    bad.extend(eof());
    assert!(read_xls(&compound("Workbook", &bad)).is_err());
    let mut b = compound("Workbook", &big);
    put32(&mut b, 512 * 2 + 128 + 120, 0x3FFF_FFFF);
    assert!(read_xls(&b).is_err());
    // Random bytes behind a compound-file signature.
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    for _ in 0..200 {
        let mut b = good[..512].to_vec();
        for _ in 0..2048 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            b.push(seed as u8);
        }
        let _ = read_xls(&b);
    }
}

#[test]
fn dialog_sheets_are_skipped_not_read_as_worksheets() {
    let boundsheet = |pos: u32, name: &str| rec(0x0085, &cat(&[&pos.to_le_bytes(), &[0, 0, name.len() as u8, 0], name.as_bytes()]));
    let globals = |positions: [u32; 2]| cat(&[&bof(0x0005), &boundsheet(positions[0], "Dialog1"), &boundsheet(positions[1], "Sheet1"), &eof()]);
    let dialog = cat(&[&bof(0x0010), &rec(0x0081, &[0x10, 0x00]), &rec(0x0203, &cat(&[&cell(0, 0, 0), &1.0f64.to_le_bytes()])), &eof()]);
    let worksheet = cat(&[&bof(0x0010), &rec(0x0081, &[0x01, 0x04]), &rec(0x0203, &cat(&[&cell(0, 0, 0), &7.0f64.to_le_bytes()])), &eof()]);
    let first = globals([0; 2]).len() as u32;
    let stream = cat(&[&globals([first, first + dialog.len() as u32]), &dialog, &worksheet]);
    let (wb, report) = read_xls(&compound("Workbook", &stream)).unwrap();
    assert_eq!(wb.sheets.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Sheet1"]);
    assert_eq!(v(&wb, 0, "A1"), Value::Number(7.0));
    assert!(report.warnings.iter().any(|w| w.contains("Dialog sheets")), "{:?}", report.warnings);
}

/// A workbook opens on the sheet it was saved on (WINDOW1's `itabCur`), as Excel does: counted
/// over all the file's sheets, skipped ones included. A hidden or skipped sheet, or no window
/// record, falls back to the first visible sheet.
#[test]
fn opens_on_the_sheet_it_was_saved_on() {
    // BoundSheet: position, visibility (0 visible, 1 hidden), kind, name.
    let boundsheet = |pos: u32, state: u8, name: &str| rec(0x0085, &cat(&[&pos.to_le_bytes(), &[state, 0, name.len() as u8, 0], name.as_bytes()]));
    let window1 = |tab: u16| rec(0x003D, &u16s(&[0, 0, 0x4000, 0x2000, 0x0038, tab, 0, 1, 600]));
    let dialog = cat(&[&bof(0x0010), &rec(0x0081, &[0x10, 0x00]), &eof()]);
    let worksheet = |n: f64| cat(&[&bof(0x0010), &rec(0x0081, &[0x01, 0x04]), &rec(0x0203, &cat(&[&cell(0, 0, 0), &n.to_le_bytes()])), &eof()]);
    // Dialog1 (skipped), Sheet1, Sheet2, Hidden (hidden).
    let book = |window: &[u8]| {
        let globals = |p: [u32; 4]| {
            cat(&[
                &bof(0x0005),
                window,
                &boundsheet(p[0], 0, "Dialog1"),
                &boundsheet(p[1], 0, "Sheet1"),
                &boundsheet(p[2], 0, "Sheet2"),
                &boundsheet(p[3], 1, "Hidden"),
                &eof(),
            ])
        };
        let (d, s1, s2, h) = (dialog.clone(), worksheet(1.0), worksheet(2.0), worksheet(3.0));
        let base = globals([0; 4]).len() as u32;
        let p = [base, base + d.len() as u32, base + (d.len() + s1.len()) as u32, base + (d.len() + s1.len() + s2.len()) as u32];
        let stream = cat(&[&globals(p), &d, &s1, &s2, &h]);
        let (wb, _) = read_xls(&compound("Workbook", &stream)).unwrap();
        wb.active().map(|s| s.name.clone()).unwrap_or_default()
    };
    assert_eq!(book(&window1(2)), "Sheet2", "itabCur 2 is Sheet2, counting the skipped dialog sheet");
    assert_eq!(book(&window1(1)), "Sheet1");
    assert_eq!(book(&window1(3)), "Sheet1", "a hidden sheet falls back to the first visible one");
    assert_eq!(book(&window1(0)), "Sheet1", "a skipped sheet falls back to the first visible one");
    assert_eq!(book(&window1(99)), "Sheet1", "an index past the sheets falls back too");
    assert_eq!(book(&[]), "Sheet1", "no window record");
    assert_eq!(book(&rec(0x003D, &u16s(&[0, 0, 0, 0, 0]))), "Sheet1", "a window record too short for the index");
}

#[test]
fn sheets_share_one_record_budget() {
    // Many BoundSheet records point into one long substream (at successive nested BOFs), so each
    // sheet would read the whole tail. The record allowance is shared, not per sheet.
    let n = 300u32;
    let tail: Vec<u8> = (0..2000u16).flat_map(|r| rec(0x0203, &cat(&[&cell(r, 0, 0), &1.0f64.to_le_bytes()]))).collect();
    let boundsheet = |pos: u32, name: &str| rec(0x0085, &cat(&[&pos.to_le_bytes(), &[0, 0, name.len() as u8, 0], name.as_bytes()]));
    let globals = |base: u32| {
        let mut g = bof(0x0005);
        for i in 0..n {
            g.extend(boundsheet(base + i * bof(0x0010).len() as u32, &format!("S{i}")));
        }
        g.extend(eof());
        g
    };
    let base = globals(0).len() as u32;
    let mut stream = globals(base);
    for _ in 0..n {
        stream.extend(bof(0x0010));
    }
    stream.extend(tail);
    stream.extend(eof());
    let file = compound("Workbook", &stream);
    // About n × 2,000 records in all: over a 100,000 shared allowance, though each sheet alone fits.
    assert!(matches!(
        crate::xls::read_xls_budgeted(&file, craft_xls::Limits { max_records: 100_000, ..Default::default() }, crate::FunctionInfo::default()),
        Err(IoError::TooLarge(_))
    ));
    assert!(
        crate::xls::read_xls_budgeted(&file, craft_xls::Limits { max_records: 10_000_000, ..Default::default() }, crate::FunctionInfo::default())
            .is_ok()
    );

    // Two sheets at the same offset are refused outright.
    let mut dup = bof(0x0005);
    let at = (dup.len() + 2 * boundsheet(0, "A").len() + eof().len()) as u32;
    dup.extend(boundsheet(at, "A"));
    dup.extend(boundsheet(at, "B"));
    dup.extend(eof());
    dup.extend(bof(0x0010));
    dup.extend(eof());
    let err = read_xls(&compound("Workbook", &dup)).unwrap_err().to_string();
    assert!(err.contains("same substream"), "{err}");
}

#[test]
fn formulas_using_a_skipped_name_keep_their_saved_results() {
    // Lbl "ExtRange" = [Source.xlsx]Sheet1!$A$1:$A$3 (PtgArea3d through an external SupBook),
    // "Local" = Sheet1!$A$1 and "Twice" = ExtRange*2. Only the first is skipped on import; a
    // name that uses it is still read as it was.
    let ext = cat(&[&[0x3B], &1u16.to_le_bytes(), &u16s(&[0, 2, 0, 0])]);
    let local = cat(&[&[0x3A], &0u16.to_le_bytes(), &u16s(&[0, 0])]);
    let twice = cat(&[&[0x23], &1u32.to_le_bytes(), &ptg_int(2), &[0x05]]);
    let lbl = |name: &str, rgce: &[u8]| {
        rec(0x0018, &cat(&[&u16s(&[0]), &[0, name.len() as u8], &u16s(&[rgce.len() as u16, 0, 0]), &[0; 4], &[0], name.as_bytes(), rgce]))
    };
    let boundsheet = |pos: u32| rec(0x0085, &cat(&[&pos.to_le_bytes(), &[0, 0, 6, 0], b"Sheet1"]));
    let globals = |pos: u32| {
        cat(&[
            &bof(0x0005),
            &boundsheet(pos),
            &rec(0x01AE, &u16s(&[1, 0x0401])),
            &rec(0x01AE, &cat(&[&u16s(&[1]), &xlstr("Source.xlsx"), &xlstr("Sheet1")])),
            &rec(0x0017, &cat(&[&u16s(&[2]), &u16s(&[0, 0, 0]), &u16s(&[1, 0, 0])])),
            &lbl("ExtRange", &ext),
            &lbl("Local", &local),
            &lbl("Twice", &twice),
            &eof(),
        ])
    };
    let sum_ext = cat(&[&[0x43], &1u32.to_le_bytes(), &[0x22, 1], &4u16.to_le_bytes()]);
    let local_x2 = cat(&[&[0x43], &2u32.to_le_bytes(), &ptg_int(2), &[0x05]]);
    let sheet = cat(&[
        &bof(0x0010),
        &rec(0x0203, &cat(&[&cell(0, 0, 0), &10.0f64.to_le_bytes()])),
        &formula(5, 1, num(60.0), &sum_ext),
        &formula(6, 1, num(20.0), &local_x2),
        &eof(),
    ]);
    let pos = globals(0).len() as u32;
    let (wb, report) = read_xls(&compound("Workbook", &cat(&[&globals(pos), &sheet]))).unwrap();
    let names: Vec<_> = wb.names.iter().map(|n| (n.name.as_str(), n.formula.as_str())).collect();
    assert_eq!(names, [("Local", "Sheet1!$A$1"), ("Twice", "ExtRange*2")]);
    assert_eq!(ftext(&wb, "B6"), None);
    assert_eq!(v(&wb, 0, "B6"), Value::Number(60.0));
    assert_eq!(ftext(&wb, "B7").as_deref(), Some("Local*2"));
    assert!(report.warnings.iter().any(|w| w.contains("defined names") && w.contains("saved results")), "{:?}", report.warnings);
}

#[test]
fn trim_reference_calls_keep_their_meaning_without_the_xlfn_prefix() {
    // craft-xls gives a newer function called by name (PtgFuncVar iftab 255 over a PtgName,
    // [MS-XLS] 2.5.198.63) without its `_xlfn.` prefix. Lbl 1 is a synthetic hidden name
    // `_xlfn._TRO_ALL` with no formula, Lbl 2 is a workbook name `_TRO_LEADING`, and Lbl 3
    // is `Trimmed` = `_xlfn._TRO_ALL(Sheet1!$A$1:$E$10)`.
    let lbl = |flags: u16, name: &str, rgce: &[u8]| {
        rec(0x0018, &cat(&[&u16s(&[flags]), &[0, name.len() as u8], &u16s(&[rgce.len() as u16, 0, 0]), &[0; 4], &[0], name.as_bytes(), rgce]))
    };
    let call = |name: u32, args: &[u8]| cat(&[&[0x23], &name.to_le_bytes(), args, &[0x22, 2], &0x00FFu16.to_le_bytes()]);
    let area3d = cat(&[&[0x3B], &0u16.to_le_bytes(), &u16s(&[0, 9, 0, 4])]);
    let boundsheet = |pos: u32| rec(0x0085, &cat(&[&pos.to_le_bytes(), &[0, 0, 6, 0], b"Sheet1"]));
    let globals = |pos: u32| {
        cat(&[
            &bof(0x0005),
            &boundsheet(pos),
            &rec(0x01AE, &u16s(&[1, 0x0401])),
            &rec(0x0017, &cat(&[&u16s(&[1]), &u16s(&[0, 0, 0])])),
            &lbl(1, "_xlfn._TRO_ALL", &[]),
            &lbl(0, "_TRO_LEADING", &ptg_int(5)),
            &lbl(0, "Trimmed", &call(1, &area3d)),
            &eof(),
        ])
    };
    let area = cat(&[&[0x25], &u16s(&[0, 9, 0, 4])]);
    let trim = call(1, &area);
    let text = [0x17, 1, 1, 0xE9, 0]; // PtgStr: the single UTF-16 character é.
    let dv = cat(&[&3u32.to_le_bytes(), &[1, 0, 0, 0].repeat(4), &u16s(&[trim.len() as u16, 0]), &trim, &u16s(&[0, 0, 1, 0, 0, 6, 6])]);
    let sheet = cat(&[
        &bof(0x0010),
        &formula(0, 6, num(3.0), &cat(&[&trim, &[0x41], &76u16.to_le_bytes()])),
        &formula(1, 6, num(5.0), &call(2, &ptg_int(41))),
        &formula(2, 6, special(0x03, 0), &cat(&[&text, &trim, &[0x41], &76u16.to_le_bytes(), &[0x08]])),
        &rec(0x01B2, &cat(&[&[0; 14], &1u32.to_le_bytes()])),
        &rec(0x01BE, &dv),
        &eof(),
    ]);
    let pos = globals(0).len() as u32;
    let (wb, _) = read_xls(&compound("Workbook", &cat(&[&globals(pos), &sheet]))).unwrap();
    assert_eq!(ftext(&wb, "G1").as_deref(), Some("ROWS($A$1.:.$E$10)"));
    assert_eq!(ftext(&wb, "G2").as_deref(), Some("_TRO_LEADING(41)"));
    assert_eq!(ftext(&wb, "G3").as_deref(), Some("\"é\"&ROWS($A$1.:.$E$10)"));
    assert_eq!(wb.sheets[0].validations[0].f1, "=$A$1.:.$E$10");
    let trimmed = wb.names.iter().find(|n| n.name == "Trimmed").map(|n| n.formula.as_str());
    assert_eq!(trimmed, Some("Sheet1!$A$1.:.$E$10"));
}

#[test]
fn reads_iterative_calculation_settings() {
    // CalcMode, CalcCount, CalcIter and CalcDelta repeat in every worksheet substream; here only
    // the first sheet carries them, so the sheet after it must not reset them to the defaults.
    // CalcPrecision sits in the globals.
    let calc = cat(&[&rec(0x000D, &u16s(&[0])), &rec(0x000C, &u16s(&[50])), &rec(0x0011, &u16s(&[1])), &rec(0x0010, &0.01f64.to_le_bytes())]);
    let sheet1 = cat(&[&bof(0x0010), &calc, &eof()]);
    let sheet2 = cat(&[&bof(0x0010), &eof()]);
    let boundsheet = |pos: u32, name: &str| rec(0x0085, &cat(&[&pos.to_le_bytes(), &[0, 0, name.len() as u8, 0], name.as_bytes()]));
    let globals = |p1: u32, p2: u32| cat(&[&bof(0x0005), &rec(0x000E, &u16s(&[0])), &boundsheet(p1, "A"), &boundsheet(p2, "B"), &eof()]);
    let p1 = globals(0, 0).len() as u32;
    let p2 = p1 + sheet1.len() as u32;
    let (wb, _) = read_xls(&compound("Workbook", &cat(&[&globals(p1, p2), &sheet1, &sheet2]))).unwrap();
    assert!(wb.calc.iterative);
    assert_eq!((wb.calc.max_iterations, wb.calc.max_change), (50, 0.01));
    assert_eq!(wb.calc.mode, gridcraft_model::CalcMode::Manual);
    assert!(wb.calc.precision_as_displayed);
    // Without the records: Excel's defaults, iteration off.
    let (wb, _) = read_xls(&compound("Workbook", &sample(false))).unwrap();
    assert_eq!(wb.calc, gridcraft_model::CalcSettings::default());
}

#[test]
fn a_format_shorter_than_its_count_keeps_its_characters() {
    // A FORMAT whose cch is one more than the characters in the record (seen in real files):
    // Excel opens them, the record's characters are the format code.
    let short = rec(0x041E, &cat(&[&165u16.to_le_bytes(), &6u16.to_le_bytes(), &[0], b"0.000"]));
    let xf = |ifmt: u16| rec(0x00E0, &cat(&[&u16s(&[0, ifmt, 0]), &[0; 14]]));
    let globals = |pos: u32| {
        cat(&[
            &bof(0x0005),
            &short,
            &rec(0x041E, &cat(&[&164u16.to_le_bytes(), &xlstr("yyyy-mm-dd")])),
            &xf(0),
            &xf(164),
            &xf(165),
            &rec(0x0085, &cat(&[&pos.to_le_bytes(), &[0, 0, 6, 0], b"Sheet1"])),
            &eof(),
        ])
    };
    let sheet = cat(&[
        &bof(0x0010),
        &rec(0x0203, &cat(&[&cell(0, 0, 0), &7.0f64.to_le_bytes()])),
        &rec(0x0203, &cat(&[&cell(1, 0, 1), &0.0f64.to_le_bytes()])),
        &rec(0x0203, &cat(&[&cell(2, 0, 2), &1.25f64.to_le_bytes()])),
        &eof(),
    ]);
    let stream = cat(&[&globals(globals(0).len() as u32), &sheet]);
    let (wb, report) = read_xls(&compound("Workbook", &stream)).unwrap();
    let sh = &wb.sheets[0];
    let fmt = |a1: &str| wb.styles.get(sh.style_id(CellRef::parse(a1).unwrap())).num_fmt.as_str().to_string();
    assert_eq!(v(&wb, 0, "A1"), Value::Number(7.0));
    assert_eq!(v(&wb, 0, "A3"), Value::Number(1.25));
    assert_eq!(fmt("A2"), "yyyy-mm-dd");
    assert_eq!(fmt("A3"), "0.000");
    assert!(report.warnings.iter().any(|w| w.contains("declared length")), "{:?}", report.warnings);
}

/// A workbook whose "Report" sheet holds legacy formulas over ranges of "Data": Data!A1 = 11,
/// B1:G1 = 1, A2 = 5, A3 = 7, and Report!B1:G1 filled, so a formula in Report!A1 that spilled
/// would be blocked.
fn legacy_formulas_book() -> Vec<u8> {
    let area3d = |r0: u16, r1: u16, c0: u16, c1: u16| cat(&[&[0x3B], &0u16.to_le_bytes(), &u16s(&[r0, r1, c0, c1])]);
    let a2a3 = area3d(1, 2, 0, 0);
    let sum = |inner: &[u8]| cat(&[inner, &[0x22, 1], &4u16.to_le_bytes()]);
    let boundsheet = |pos: u32, name: &str| rec(0x0085, &cat(&[&pos.to_le_bytes(), &[0, 0, name.len() as u8, 0], name.as_bytes()]));
    let globals = |p1: u32, p2: u32| {
        cat(&[
            &bof(0x0005),
            &boundsheet(p1, "Data"),
            &boundsheet(p2, "Report"),
            &rec(0x01AE, &u16s(&[2, 0x0401])),
            &rec(0x0017, &cat(&[&u16s(&[1]), &u16s(&[0, 0, 0])])),
            &eof(),
        ])
    };
    let number = |r: u16, c: u16, n: f64| rec(0x0203, &cat(&[&cell(r, c, 0), &n.to_le_bytes()]));
    let mut data = bof(0x0010);
    data.extend(number(0, 0, 11.0));
    for c in 1..7 {
        data.extend(number(0, c, 1.0));
    }
    data.extend(number(1, 0, 5.0));
    data.extend(number(2, 0, 7.0));
    data.extend(eof());
    let shared = cat(&[&a2a3, &ptg_int(10), &[0x05]]);
    let array = cat(&[&a2a3, &ptg_int(2), &[0x05]]);
    let single_array = sum(&cat(&[&a2a3, &ptg_int(2), &[0x05]]));
    let mut report = bof(0x0010);
    for c in 1..7 {
        report.extend(number(0, c, 0.0));
    }
    report.extend(cat(&[
        // A1 = Data!A1:G1, A2 = +Data!A1:G1 (a title row repeated from another sheet).
        &formula(0, 0, num(11.0), &area3d(0, 0, 0xC000, 6 | 0xC000)),
        &formula(1, 0, num(11.0), &cat(&[&area3d(0, 0, 0xC000, 6 | 0xC000), &[0x12]])),
        // C2:C3 share Data!$A$2:$A$3*10.
        &formula(1, 2, num(50.0), &exp(1, 2)),
        &rec(0x04BC, &cat(&[&u16s(&[1, 2]), &[2, 2, 0, 2], &(shared.len() as u16).to_le_bytes(), &shared])),
        &formula(2, 2, num(70.0), &exp(1, 2)),
        // D2:D3 = {Data!$A$2:$A$3*2}, entered with Ctrl+Shift+Enter.
        &formula(1, 3, num(10.0), &exp(1, 3)),
        &rec(0x0221, &cat(&[&u16s(&[1, 2]), &[3, 3], &[0; 6], &(array.len() as u16).to_le_bytes(), &array])),
        &formula(2, 3, num(14.0), &exp(1, 3)),
        // H1 = {SUM(Data!$A$2:$A$3*2)}, a one-cell Ctrl+Shift+Enter array.
        &formula(0, 7, num(24.0), &exp(0, 7)),
        &rec(0x0221, &cat(&[&u16s(&[0, 0]), &[7, 7], &[0; 6], &(single_array.len() as u16).to_le_bytes(), &single_array])),
        // H2 = MAX(SUM(Data!$A$2:$A$3/12),0), H3 = SUM(Data!$A$2:$A$3), H4 = SQRT(Data!$A$2:$A$3).
        &formula(1, 7, num(5.0 / 12.0), &cat(&[&sum(&cat(&[&a2a3, &ptg_int(12), &[0x06]])), &ptg_int(0), &[0x22, 2], &7u16.to_le_bytes()])),
        &formula(2, 7, num(12.0), &sum(&a2a3)),
        &formula(3, 7, special(2, 0x0F), &cat(&[&a2a3, &[0x21], &20u16.to_le_bytes()])),
        &eof(),
    ]));
    let p1 = globals(0, 0).len() as u32;
    let p2 = p1 + data.len() as u32;
    cat(&[&globals(p1, p2), &data, &report])
}

#[test]
fn legacy_formulas_read_with_implicit_intersection() {
    let bytes = compound("Workbook", &legacy_formulas_book());
    let formula = |wb: &gridcraft_model::Workbook, a1: &str| {
        let f = wb.sheets[1].cell(CellRef::parse(a1).unwrap()).and_then(|c| c.formula.clone()).unwrap();
        (f.text().into_owned(), f.array)
    };
    let range = |a1: &str| RangeRef::parse(a1);
    let (wb, _) = read_xls(&bytes).unwrap();
    // Every .xls formula is legacy: a range where one value is expected is intersected.
    assert_eq!(formula(&wb, "A1"), ("@Data!A1:G1".into(), None));
    assert_eq!(formula(&wb, "A2"), ("+@Data!A1:G1".into(), None));
    // Shared formulas too, each cell on its own.
    assert_eq!(formula(&wb, "C2"), ("@Data!$A$2:$A$3*10".into(), None));
    assert_eq!(formula(&wb, "C3"), ("@Data!$A$2:$A$3*10".into(), None));
    // Ctrl+Shift+Enter arrays, one cell or several, stay arrays and are not intersected.
    assert_eq!(formula(&wb, "D2"), ("Data!$A$2:$A$3*2".into(), range("D2:D3")));
    assert_eq!(formula(&wb, "H1"), ("SUM(Data!$A$2:$A$3*2)".into(), range("H1")));
    // An operation inside SUM takes values; SUM of the range itself is left alone.
    assert_eq!(formula(&wb, "H2"), ("MAX(SUM(@Data!$A$2:$A$3/12),0)".into(), None));
    assert_eq!(formula(&wb, "H3"), ("SUM(Data!$A$2:$A$3)".into(), None));
    assert_eq!(formula(&wb, "H4"), ("SQRT(@Data!$A$2:$A$3)".into(), None));

    // The caller's function facts decide: here SQRT takes its argument as a whole.
    let library = crate::FunctionInfo { is_builtin: |_| true, is_scalar_arg: |n, _| n != "SQRT" };
    let (custom, _) = crate::read_xls_with(&bytes, library).unwrap();
    assert_eq!(formula(&custom, "H4"), ("SQRT(Data!$A$2:$A$3)".into(), None));
    assert_eq!(formula(&custom, "A1"), ("@Data!A1:G1".into(), None));

    // Saved as .xlsx, they stay legacy: no dynamic-array metadata, no _xlfn.SINGLE, and they read
    // back as they were opened.
    let saved = write_xlsx(&wb).unwrap();
    let mut sheet = String::new();
    std::io::Read::read_to_string(
        &mut zip::ZipArchive::new(std::io::Cursor::new(&saved)).unwrap().by_name("xl/worksheets/sheet2.xml").unwrap(),
        &mut sheet,
    )
    .unwrap();
    assert!(!sheet.contains("cm=") && !sheet.contains("SINGLE"), "{sheet}");
    assert!(sheet.contains("<f>Data!A1:G1</f>") && sheet.contains("<f>MAX(SUM(Data!$A$2:$A$3/12),0)</f>"), "{sheet}");
    let (back, _) = read_xlsx(&saved).unwrap();
    for a1 in ["A1", "A2", "C2", "C3", "D2", "H1", "H2", "H3", "H4"] {
        assert_eq!(formula(&back, a1), formula(&wb, a1), "{a1}");
    }
}

fn one_sheet_with(sheet_records: &[u8]) -> Vec<u8> {
    let globals = |pos: u32| cat(&[&bof(0x0005), &rec(0x0085, &cat(&[&pos.to_le_bytes(), &[0, 0, 6, 0], b"Sheet1"])), &eof()]);
    let sheet = cat(&[&bof(0x0010), sheet_records, &rec(0x0203, &cat(&[&cell(0, 0, 0), &7.0f64.to_le_bytes()])), &eof()]);
    compound("Workbook", &cat(&[&globals(globals(0).len() as u32), &sheet]))
}

#[test]
fn xls_window2_right_to_left_sheet_opens_right_to_left() {
    let window2 = |flags: u16| rec(0x023E, &u16s(&[flags, 0, 0, 64, 0, 0, 0, 0, 0]));
    let view = |file: &[u8]| {
        let (wb, _) = read_xls(file).unwrap();
        let sh = &wb.sheets[0];
        (sh.right_to_left, sh.show_gridlines, sh.show_headings, sh.show_zeros, sh.show_formulas)
    };
    assert_eq!(view(&one_sheet_with(&window2(0x0040 | 0x0004))), (true, false, true, false, false));
    assert_eq!(view(&one_sheet_with(&window2(0x0001 | 0x0002 | 0x0010))), (false, true, false, true, true));
}

/// COLINFO's fUserSet marks a width set by hand, which a wider number format leaves alone (#1077).
/// craft-xls does not report fBestFit, so fUserSet with fBestFit also reads as set by hand.
#[test]
fn xls_column_widths_set_by_hand() {
    let colinfo = |col: u16, flags: u16| rec(0x007D, &u16s(&[col, col, 20 * 256, 0, flags, 0]));
    let file = one_sheet_with(&cat(&[&colinfo(0, 0x0002), &colinfo(1, 0), &colinfo(2, 0x0006)]));
    let (wb, _) = read_xls(&file).unwrap();
    let sh = &wb.sheets[0];
    let custom: Vec<bool> = (0..3).map(|c| sh.cols.get(&c).is_some_and(|i| i.custom)).collect();
    assert_eq!(custom, [true, false, true]);
    assert!((0..3).all(|c| sh.col_width(c) > sh.default_col_width), "all three keep their stored width");
}

#[test]
fn xls_short_window2_is_ignored() {
    for payload in [&[][..], &[0x40][..], &[0x40, 0][..], &[0x40, 0, 0, 0, 0, 0, 0, 0, 0][..]] {
        let (wb, _) = read_xls(&one_sheet_with(&rec(0x023E, payload))).unwrap();
        let sh = &wb.sheets[0];
        assert_eq!((sh.right_to_left, sh.show_gridlines, sh.show_headings, sh.show_zeros), (false, true, true, true));
        assert_eq!(v(&wb, 0, "A1"), Value::Number(7.0));
    }
}

/// Shared-formula followers each keep their own text, charged to the workbook's text allowance:
/// with no allowance left they keep their cached values, and the import still succeeds.
#[test]
fn shared_formula_text_counts_against_the_text_allowance() {
    let file = compound("Workbook", &sample(false));
    // The smallest allowance that reads every string: nothing is left for expanded formulas.
    let left = (0..1000)
        .find(|n| crate::xls::read_xls_budgeted(&file, craft_xls::Limits { max_text: *n, ..Default::default() }, Default::default()).is_ok())
        .unwrap();
    let (wb, report) = crate::xls::read_xls_budgeted(&file, craft_xls::Limits { max_text: left, ..Default::default() }, Default::default()).unwrap();
    assert_eq!(ftext(&wb, "B6"), None);
    assert_eq!(v(&wb, 0, "B6"), Value::Number(20.0));
    assert!(report.warnings.iter().any(|w| w.contains("shared-formula text")), "{:?}", report.warnings);
    // an array anchor after the cut-off still gets its formula
    assert!(ftext(&wb, "C5").is_some());
    // With room, the followers keep their formulas.
    let (wb, _) = crate::xls::read_xls_budgeted(&file, Default::default(), Default::default()).unwrap();
    assert!(ftext(&wb, "B6").is_some());
}

/// Shared-formula text is optional: an early sheet's expanded formulas must not use the allowance
/// a later sheet's inline strings need, or the whole import fails for text that fits.
#[test]
fn shared_formulas_do_not_starve_a_later_sheets_text() {
    let shared = cat(&[&[0x4C], &u16s(&[0, 0xFFFF]), &ptg_int(10), &[0x05]]);
    let boundsheet = |pos: u32, name: &str| rec(0x0085, &cat(&[&pos.to_le_bytes(), &[0, 0, name.len() as u8, 0], name.as_bytes()]));
    let globals = |p: [u32; 2]| cat(&[&bof(0x0005), &boundsheet(p[0], "One"), &boundsheet(p[1], "Two"), &eof()]);
    let one = cat(&[
        &bof(0x0010),
        &formula(4, 1, num(10.0), &exp(4, 1)),
        &rec(0x04BC, &cat(&[&u16s(&[4, 5]), &[1, 1, 0, 2], &(shared.len() as u16).to_le_bytes(), &shared])),
        &formula(5, 1, num(20.0), &exp(4, 1)),
        &eof(),
    ]);
    let two = cat(&[&bof(0x0010), &rec(0x0204, &cat(&[&cell(0, 0, 0), &xlstr("abcdefgh")])), &eof()]);
    let p1 = globals([0; 2]).len() as u32;
    let file = compound("Workbook", &cat(&[&globals([p1, p1 + one.len() as u32]), &one, &two]));
    // The smallest allowance that reads every string leaves nothing for expanded formulas.
    let left = (0..1000)
        .find(|n| crate::xls::read_xls_budgeted(&file, craft_xls::Limits { max_text: *n, ..Default::default() }, Default::default()).is_ok())
        .unwrap();
    let (wb, _) = crate::xls::read_xls_budgeted(&file, craft_xls::Limits { max_text: left, ..Default::default() }, Default::default()).unwrap();
    assert_eq!(v(&wb, 1, "A1"), Value::text("abcdefgh"));
    assert_eq!(ftext(&wb, "B6"), None);
    assert_eq!(v(&wb, 0, "B6"), Value::Number(20.0));
    let (wb, _) = crate::xls::read_xls_budgeted(&file, Default::default(), Default::default()).unwrap();
    assert!(ftext(&wb, "B6").is_some());
}

/// Two readable worksheets with the same name are refused at import, as they were when each sheet
/// was added to the workbook as it was read.
#[test]
fn xls_duplicate_sheet_names_are_refused() {
    let boundsheet = |name: &str| rec(0x0085, &cat(&[&0u32.to_le_bytes(), &[0, 0, name.len() as u8, 0], name.as_bytes()]));
    let sheet = cat(&[&bof(0x0010), &eof()]);
    let globals = |a: &str, b: &str| cat(&[&bof(0x0005), &boundsheet(a), &boundsheet(b), &eof()]);
    let file = |a: &str, b: &str| {
        let p = globals(a, b).len() as u32;
        let mut g = cat(&[
            &bof(0x0005),
            &rec(0x0085, &cat(&[&p.to_le_bytes(), &[0, 0, a.len() as u8, 0], a.as_bytes()])),
            &rec(0x0085, &cat(&[&(p + sheet.len() as u32).to_le_bytes(), &[0, 0, b.len() as u8, 0], b.as_bytes()])),
            &eof(),
        ]);
        g.extend(&sheet);
        g.extend(&sheet);
        compound("Workbook", &g)
    };
    let err = crate::xls::read_xls_budgeted(&file("One", "one"), Default::default(), Default::default()).unwrap_err().to_string();
    assert!(err.contains("already taken"), "{err}");
    assert!(crate::xls::read_xls_budgeted(&file("One", "Two"), Default::default(), Default::default()).is_ok());
}

/// A one-sheet (`Sheet1`) workbook with workbook-scope names `names` (Lbl 1, 2, … in order) and
/// the cell records `cells`.
fn names_book(names: &[(&str, &[u8])], cells: &[&[u8]]) -> Vec<u8> {
    let names = names.iter().map(|&(name, body)| (name, 0, body)).collect::<Vec<_>>();
    scoped_names_book(&names, &[cells.concat()])
}

fn scoped_names_book(names: &[(&str, u16, &[u8])], sheets: &[Vec<u8>]) -> Vec<u8> {
    let globals = |mut pos: u32| {
        let mut g = bof(0x0005);
        for (i, cells) in sheets.iter().enumerate() {
            let name = format!("Sheet{}", i + 1);
            g.extend(rec(0x0085, &cat(&[&pos.to_le_bytes(), &[0, 0, name.len() as u8, 0], name.as_bytes()])));
            pos += (bof(0x0010).len() + cells.len() + eof().len()) as u32;
        }
        g.extend(rec(0x01AE, &u16s(&[sheets.len() as u16, 0x0401])));
        g.extend(rec(0x0017, &u16s(&[1, 0, 0, 0])));
        for &(name, scope, body) in names {
            g.extend(rec(
                0x0018,
                &cat(&[&u16s(&[0]), &[0, name.len() as u8], &u16s(&[body.len() as u16, 0, scope]), &[0; 4], &[0], name.as_bytes(), body]),
            ));
        }
        g.extend(eof());
        g
    };
    let mut stream = globals(globals(0).len() as u32);
    for cells in sheets {
        stream.extend(bof(0x0010));
        stream.extend(cells);
        stream.extend(eof());
    }
    compound("Workbook", &stream)
}

/// PtgRef3d on Sheet1 as a name stores it: a relative row/column is an offset from A1
/// ([MS-XLS] RgceLocRel), `0xFFFA` six rows up, column byte `0xFF` one column left.
fn name_ref(row: u16, col: u16) -> Vec<u8> {
    cat(&[&[0x3A], &0u16.to_le_bytes(), &u16s(&[row, col])])
}

/// PtgName for Lbl `i` (one-based).
fn use_name(i: u32) -> Vec<u8> {
    cat(&[&[0x43], &i.to_le_bytes()])
}

fn saved_names(bytes: &[u8]) -> Vec<(String, String)> {
    let (back, _) = read_xlsx(bytes).unwrap();
    back.names.iter().map(|n| (n.name.clone(), n.formula.clone())).collect()
}

fn workbook_xml(bytes: &[u8]) -> String {
    use std::io::Read as _;
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut s = String::new();
    z.by_name("xl/workbook.xml").unwrap().read_to_string(&mut s).unwrap();
    s
}

fn save(wb: &gridcraft_model::Workbook) -> (Vec<u8>, Vec<String>) {
    let (bytes, report) = crate::write_xlsx_named_with(wb, crate::XlsxKind::Workbook, "book.xlsx", crate::FunctionInfo::default()).unwrap();
    (bytes, report.warnings)
}

const LEGACY_NOTICE: &str = "point at cells relative to where they are used";

#[test]
fn xls_relative_names_keep_their_residue_and_are_marked_for_the_legacy_grid() {
    // Attend = six rows up (A65531 seen from A1), Left = one column left (IV1), Rate absolute.
    let attend = name_ref(0xFFFA, 0xC000);
    let left = name_ref(0, 0xC0FF);
    let rate = name_ref(0, 0);
    let file = names_book(
        &[("Attend", &attend), ("Left", &left), ("Rate", &rate)],
        &[&formula(20, 5, num(0.0), &use_name(1)), &formula(21, 1, num(0.0), &use_name(2))],
    );
    let (wb, _) = read_xls(&file).unwrap();
    let names: Vec<_> = wb.names.iter().map(|n| (n.name.as_str(), n.formula.as_str(), n.legacy_grid)).collect();
    assert_eq!(names, [("Attend", "Sheet1!A65531", true), ("Left", "Sheet1!IV1", true), ("Rate", "Sheet1!$A$1", false)]);
    assert_eq!(ftext(&wb, "F21").as_deref(), Some("Attend"));
    assert_eq!(ftext(&wb, "B22").as_deref(), Some("Left"));
}

#[test]
fn xls_relative_names_save_as_signed_offsets_for_the_xlsx_grid() {
    let attend = name_ref(0xFFFA, 0xC000);
    let left = name_ref(0, 0xC0FF);
    let ahead = name_ref(2, 0xC003);
    let file = names_book(
        &[("Attend", &attend), ("Left", &left), ("Ahead", &ahead)],
        &[&formula(20, 5, num(0.0), &use_name(1)), &formula(21, 1, num(0.0), &use_name(2)), &formula(1, 1, num(0.0), &use_name(3))],
    );
    let (wb, _) = read_xls(&file).unwrap();
    let (bytes, warnings) = save(&wb);
    // Six up is -6 rows on the big grid too, one left is -1 column, a small positive offset stays.
    assert_eq!(
        saved_names(&bytes),
        [("Attend".into(), "Sheet1!A1048571".into()), ("Left".into(), "Sheet1!XFD1".into()), ("Ahead".into(), "Sheet1!D3".into())]
    );
    // Every use is inside both grids' wrap, so it reads the same cells: nothing to warn of.
    assert!(!warnings.iter().any(|w| w.contains(LEGACY_NOTICE)), "{warnings:?}");
    // The marker never reaches the file, and the live workbook keeps its legacy names.
    let xml = workbook_xml(&bytes);
    assert!(!xml.to_ascii_lowercase().contains("legacy"), "{xml}");
    assert!(wb.names.iter().filter(|n| n.name != "Rate").all(|n| n.legacy_grid));
    assert_eq!(wb.names[0].formula, "Sheet1!A65531");
    let (back, _) = read_xlsx(&bytes).unwrap();
    assert!(back.names.iter().all(|n| !n.legacy_grid));
}

#[test]
fn saving_warns_of_legacy_name_uses_that_read_other_cells_and_names_it_cannot_check() {
    let left = name_ref(0, 0xC0FF);
    let unused = name_ref(1, 0xC000);
    let attend = name_ref(0xFFFA, 0xC000);
    // INDIRECT("Attend"): PtgStr then PtgFuncVar INDIRECT (iftab 148) with one argument.
    let indirect = cat(&[&[0x17, 6, 0], b"Attend", &[0x22, 1], &148u16.to_le_bytes()]);
    let file = names_book(
        &[("Left", &left), ("Unused", &unused), ("Attend", &attend)],
        &[
            // One-left from B22 is A22 on both grids; from A5 it is IV5 in Compatibility Mode
            // and XFD5 once saved.
            &formula(21, 1, num(0.0), &use_name(1)),
            &formula(4, 0, num(0.0), &use_name(1)),
            &formula(20, 5, num(0.0), &use_name(3)),
            &formula(30, 0, num(0.0), &indirect),
        ],
    );
    let (wb, _) = read_xls(&file).unwrap();
    assert_eq!(ftext(&wb, "A31").as_deref(), Some("INDIRECT(\"Attend\")"));
    let (_, warnings) = save(&wb);
    let first = &warnings[0];
    assert!(first.contains(LEGACY_NOTICE), "{warnings:?}");
    assert!(first.contains("Left (workbook) from Sheet1!A5"), "{first}");
    assert!(first.contains("Unused (workbook) (no checked use found)"), "{first}");
    assert!(first.contains("Attend (workbook) (INDIRECT may reach it)"), "{first}");
    assert!(!first.contains("B22"), "{first}");
}

#[test]
fn cell_shaped_xls_names_are_unchanged_and_warned_of_on_save() {
    // `On2` is a name in Compatibility Mode (column ON is past IV) but cell ON2 on the XLSX grid.
    // X60 leaves it as it was read: the formula text reads cell ON2, the name is written as it is,
    // and the save says its binding is not kept. The follow-up lane changes this deliberately.
    let on2 = name_ref(0, 0);
    let file = names_book(&[("On2", &on2)], &[&rec(0x0203, &cat(&[&cell(0, 0, 0), &5.0f64.to_le_bytes()])), &formula(2, 0, num(5.0), &use_name(1))]);
    let (wb, _) = read_xls(&file).unwrap();
    let names: Vec<_> = wb.names.iter().map(|n| (n.name.as_str(), n.formula.as_str(), n.legacy_grid)).collect();
    assert_eq!(names, [("On2", "Sheet1!$A$1", false)]);
    // The use is read as the cell: its text is the reference ON2, not the name.
    assert_eq!(ftext(&wb, "A3").as_deref(), Some("ON2"));
    let use_ = wb.sheets[0]
        .cell(CellRef::parse("A3").unwrap())
        .and_then(|c| c.formula.as_ref())
        .and_then(|f| f.with_expr(|e| matches!(e, gridcraft_formula::Expr::Ref(_))));
    assert_eq!(use_, Some(true));
    let (bytes, warnings) = save(&wb);
    assert!(workbook_xml(&bytes).contains("<definedName name=\"On2\">Sheet1!$A$1</definedName>"));
    assert!(warnings[0].contains("spelled like cells") && warnings[0].contains("On2 (workbook)"), "{warnings:?}");
}

#[test]
fn native_xlsx_names_save_without_legacy_warnings() {
    let mut wb = gridcraft_model::Workbook::new();
    wb.names.push(gridcraft_model::DefinedName {
        name: "Left".into(),
        scope: None,
        formula: "Sheet1!XFD1".into(),
        comment: String::new(),
        hidden: false,
        uid: Default::default(),
        legacy_grid: false,
    });
    let (bytes, warnings) = save(&wb);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(saved_names(&bytes), [("Left".into(), "Sheet1!XFD1".into())]);
}

/// A native workbook's sheet-local name used from another sheet is an ordinary `#NAME?`: no
/// binding was lost, so saving says nothing about it.
#[test]
fn native_local_name_used_from_another_sheet_saves_without_a_binding_notice() {
    let mut wb = gridcraft_model::Workbook::new();
    wb.sheets.push(std::sync::Arc::new(gridcraft_model::Sheet::new("Sheet2")));
    wb.names.push(gridcraft_model::DefinedName {
        name: "LocalOnly".into(),
        scope: Some(0),
        formula: "Sheet1!B1".into(),
        comment: String::new(),
        hidden: false,
        uid: Default::default(),
        legacy_grid: false,
    });
    wb.sheet_mut(1).unwrap().set_cell(
        CellRef::parse("A1").unwrap(),
        gridcraft_model::Cell { formula: Some(std::sync::Arc::new(gridcraft_model::Formula::new("LocalOnly"))), ..Default::default() },
    );
    let (_, warnings) = save(&wb);
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn scoped_xls_names_report_unproven_bindings_on_open_and_save() {
    // The two callers of each duplicate spelling originally point at distinct Lbl indexes.
    let global = name_ref(0xFFFF, 0xC000);
    let local = name_ref(0, 0xC001);
    let abs = name_ref(0, 0);
    let names: [(&str, u16, &[u8]); 6] =
        [("Attend", 0, &global), ("ATTEND", 1, &local), ("On2", 0, &abs), ("ON2", 1, &local), ("LocalOnly", 1, &local), ("On3", 1, &abs)];
    let sheet = |row, indexes: &[u32]| indexes.iter().enumerate().flat_map(|(c, &i)| formula(row, c as u16, num(-1.0), &use_name(i))).collect();
    let bytes = scoped_names_book(&names, &[sheet(4, &[1, 2, 3, 4]), sheet(5, &[1, 5, 6])]);
    let (wb, report) = read_xls(&bytes).unwrap();
    for (a, b) in [("A5", "B5"), ("C5", "D5")] {
        assert_eq!(ftext(&wb, a).unwrap().to_ascii_lowercase(), ftext(&wb, b).unwrap().to_ascii_lowercase());
    }
    let (saved, save_warnings) = save(&wb);
    let (back, _) = read_xlsx(&saved).unwrap();
    for first in [&report.warnings[0], &save_warnings[0]] {
        assert!(first.starts_with("Name bindings are unproven:"), "{first}");
        for expected in [
            "Attend (scopes: workbook, Sheet1); possible consuming cells: Sheet1!A5, Sheet1!B5, Sheet2!A6",
            "On2 (scopes: workbook, Sheet1); possible consuming cells: Sheet1!C5, Sheet1!D5",
            "LocalOnly (scopes: Sheet1); no definition in scope at Sheet2!B6",
            "On3 (scopes: Sheet1); no definition in scope at Sheet2!C6; GridCraft currently reads the cell reference",
        ] {
            assert!(first.contains(expected), "missing {expected}: {first}");
        }
    }
    let definitions = |b: &gridcraft_model::Workbook| b.names.iter().map(|n| (n.name.clone(), n.scope)).collect::<Vec<_>>();
    assert_eq!(definitions(&wb), definitions(&back));
    assert_eq!(ftext(&back, "C5").as_deref(), Some("ON2"));
}

#[test]
fn legacy_name_notice_respects_lexical_bindings_and_quoted_sheet_names() {
    use gridcraft_model::{Cell, DefinedName, Formula, Sheet, Workbook};
    let mut wb = Workbook::new();
    wb.sheets.push(std::sync::Arc::new(Sheet::new("'Edge'")));
    for (name, scope, formula) in
        [("Shadowed", None, "IV1"), ("Relative", Some(1), "IV1"), ("Calling", None, "LAMBDA(IV1)"), ("Explicit", None, "LAMBDA(IV1)")]
    {
        wb.names.push(DefinedName {
            name: name.into(),
            scope,
            formula: formula.into(),
            comment: String::new(),
            hidden: false,
            uid: Default::default(),
            legacy_grid: true,
        });
    }
    for (row, text) in
        ["LET(Shadowed,2,Shadowed+1)", "LAMBDA(Shadowed,Shadowed)(1)", "'''Edge'''!Relative", "Calling()", "LET(Explicit,1,_xludf.Explicit())"]
            .into_iter()
            .enumerate()
    {
        wb.sheet_mut(0)
            .unwrap()
            .set_cell(CellRef::new(row as u32, 0), Cell { formula: Some(std::sync::Arc::new(Formula::new(text))), ..Default::default() });
    }
    let functions = crate::FunctionInfo { is_builtin: |name| gridcraft_functions::lookup(name).is_some(), ..Default::default() };
    let (_, report) = crate::write_xlsx_named_with(&wb, crate::XlsxKind::Workbook, "Names.xlsx", functions).unwrap();
    let first = &report.warnings[0];
    assert!(first.contains("Shadowed (workbook) (no checked use found)"), "{first}");
    assert!(first.contains("Relative ('Edge') from Sheet1!A3"), "{first}");
    assert!(first.contains("Calling (workbook) from Sheet1!A4"), "{first}");
    assert!(first.contains("Explicit (workbook) from Sheet1!A5"), "{first}");
    assert!(!first.contains("Sheet1!A1") && !first.contains("Sheet1!A2"), "{first}");
}

#[test]
fn qualified_indirect_to_a_modern_local_does_not_trigger_a_legacy_warning() {
    let left = name_ref(0, 0xC0FF);
    let file = names_book(&[("Left", &left)], &[&formula(4, 1, num(-1.0), &use_name(1))]);
    let (mut wb, _) = read_xls(&file).unwrap();
    wb.sheets.push(std::sync::Arc::new(gridcraft_model::Sheet::new("Modern")));
    let mut local = wb.names[0].clone();
    local.scope = Some(1);
    local.legacy_grid = false;
    local.formula = "7".into();
    wb.names.push(local);
    let mut other = wb.names[0].clone();
    other.name = "Unrelated".into();
    wb.names.push(other);
    let at = CellRef::new(1, 1);
    for (text, dynamic) in [
        ("INDIRECT(\"Modern!Left\")", false),
        ("INDIRECT(\"Sheet1!Left\")", true),
        ("INDIRECT(A1)", true),
        ("LET(INDIRECT,0,INDIRECT(\"Left\"))", true),
        ("LET(Left,0,INDIRECT(\"Left\"))", false),
    ] {
        wb.sheet_mut(0)
            .unwrap()
            .set_cell(at, gridcraft_model::Cell { formula: Some(std::sync::Arc::new(gridcraft_model::Formula::new(text))), ..Default::default() });
        let (_, warnings) = save(&wb);
        assert_eq!(warnings[0].contains("Left (workbook) (INDIRECT may reach it)"), dynamic, "{text}: {warnings:?}");
        assert_eq!(warnings[0].contains("Unrelated (workbook) (INDIRECT may reach it)"), text == "INDIRECT(A1)", "{text}: {warnings:?}");
    }
}

#[test]
fn legacy_name_notice_checks_shared_and_array_callers_and_unanchored_consumers() {
    use gridcraft_model::{Cell, DefinedName, Formula, Sparkline, Workbook};
    use std::sync::Arc;

    let mut wb = Workbook::new();
    for (name, formula, legacy_grid) in [("SharedName", "IV1", true), ("ArrayName", "IV1", true), ("SourceName", "IV1", true), ("On2", "$A$1", false)]
    {
        wb.names.push(DefinedName {
            name: name.into(),
            formula: formula.into(),
            legacy_grid,
            scope: None,
            hidden: false,
            comment: String::new(),
            uid: Default::default(),
        });
    }
    let a1 = CellRef::new(0, 0);
    let a2 = CellRef::new(1, 0);
    let a3 = CellRef::new(2, 0);
    let base = Arc::new(gridcraft_formula::parse("LET(SharedName,SharedName,SharedName)").unwrap());
    let sheet = wb.sheet_mut(0).unwrap();
    for (at, formula) in [
        (a1, Formula::from_shared(base, 0, -1)),
        (a2, Formula::new("ArrayName").with_array(Some(gridcraft_core::RangeRef::new(a2, CellRef::new(1, 1)))).with_legacy(true)),
        (a3, Formula::unparsed("SourceName")),
    ] {
        sheet.set_cell(at, Cell { formula: Some(Arc::new(formula)), ..Default::default() });
    }
    sheet.sparklines.push(Sparkline {
        cell: CellRef::new(0, 2),
        source: "SourceName".into(),
        kind: Default::default(),
        color: Default::default(),
        markers: false,
    });
    let (_, warnings) = save(&wb);
    let first = &warnings[0];
    assert!(first.contains("SharedName (workbook) from Sheet1!A1"), "{first}");
    assert!(first.contains("ArrayName (workbook) from Sheet1!A2"), "{first}");
    assert!(first.contains("SourceName (workbook) from Sheet1!A3"), "{first}");
    assert!(first.contains("SourceName (workbook) (not all caller locations could be checked)"), "{first}");
    assert!(first.contains("spelled like cells on the .xlsx grid") && first.contains("On2 (workbook)"), "{first}");
    assert!(!first.contains("These names from the Excel 97-2003 workbook"), "{first}");
}
