//! What an `.xls` says about the application that last saved it and about its calculation, read
//! from the file itself: the property streams of its compound file ([MS-OLEPS]
//! `SummaryInformation` and `DocumentSummaryInformation`) and the records of its workbook stream
//! ([MS-XLS] `BOF`, `Uncalced`).

use super::producer::kind_of_application;
use super::{Producer, ProducerKind};

/// [MS-XLS] 2.4.21 BOF.
const BOF: u16 = 0x0809;
/// [MS-XLS] 2.4.331 Uncalced: "formulas were pending recalculation when the file was saved".
const UNCALCED: u16 = 0x005E;
/// BIFF8, the `vers` of an Excel 97–2016 workbook stream's BOF.
const BIFF8: u16 = 0x0600;
/// BIFF5, the `vers` of an Excel 5.0/95 workbook stream's BOF (the `Book` stream).
const BIFF5: u16 = 0x0500;
/// Excel 95's major version.
const EXCEL_95: u32 = 7;
/// [MS-OLEPS] 2.25.1 `PIDSI_APPNAME`: the name of the application that created the document.
const PIDSI_APPNAME: u32 = 0x12;
/// [MS-OLEPS] 2.25.2 `PIDDSI_VERSION`: the version of the application that wrote the property
/// set, its major version in the high word (Excel 2003 writes `0x000B_xxxx`).
const PIDDSI_VERSION: u32 = 0x17;
const VT_I4: u16 = 0x0003;
const VT_LPSTR: u16 = 0x001E;
/// The most properties read from one property set: the summary sets hold a few dozen.
const MAX_PROPERTIES: usize = 4096;

/// What an `.xls` says about itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct XlsFacts {
    /// The application its `SummaryInformation` names, and the Excel version that last saved it
    /// (as `AppVersion` spells it, `11.0000`), from `DocumentSummaryInformation` or else, for
    /// a file Excel says it wrote, its first BOF (see [`bof_last_saved`]).
    pub producer: Producer,
    /// The workbook stream holds an `Uncalced` record.
    pub uncalced: bool,
}

/// The facts of the compound file `bytes`, or `None` when it isn't one with a workbook stream.
pub(super) fn read(bytes: &[u8]) -> Option<XlsFacts> {
    let stream = ["Workbook", "Book"].iter().find_map(|name| gridcraft_xlsx::compound_root_stream(bytes, name))?;
    let walk = walk_records(&stream);
    let application =
        gridcraft_xlsx::compound_root_stream(bytes, "\u{5}SummaryInformation").and_then(|s| property(&s, PIDSI_APPNAME)).and_then(|p| match p {
            Property::Text(t) if !t.is_empty() => Some(t),
            _ => None,
        });
    let version = gridcraft_xlsx::compound_root_stream(bytes, "\u{5}DocumentSummaryInformation").and_then(|s| property(&s, PIDDSI_VERSION)).and_then(
        |p| match p {
            Property::Int(v) => u32::try_from(v).ok(),
            Property::Text(_) => None,
        },
    );
    let mut kind = application.as_deref().map_or(ProducerKind::Unknown, kind_of_application);
    // An Office application writes its build into the low word of `PIDDSI_VERSION` (Office 2000
    // SR-1: `0x0009_0EED`, 9.0.3821), even where it names no application in `SummaryInformation`
    // (Excel 98 for the Mac: `0x0008_133A`). Of the corpus's 4,300 BIFF8 property sets with a
    // build that name an application, every one names Excel; LibreOffice's has build 0.
    if kind == ProducerKind::Unknown && version.is_some_and(|v| v & 0xFFFF != 0 && v >> 16 > 0) {
        kind = ProducerKind::Excel;
    }
    // Excel writes `reserved1` of every FORMULA record as zero ([MS-XLS] 2.4.127: "MUST be
    // zero"); a workbook whose every formula has it set was written by another application.
    // Excel's `WriteAccess` names the user in an XLUnicodeString ([MS-XLS] 2.4.349; a byte string
    // in the BIFF5/7 `Book` stream); a library
    // that writes the bare name instead (xlwt: its default owner `None` padded with spaces, which
    // also caches every formula as an empty string; JExcelAPI) isn't Excel. Excel writes a
    // number RK can hold in an RK or MULRK record, never a NUMBER one ([`fits_rk`]); a worksheet
    // of NUMBER records for such values and no RK at all was written by a library that has no
    // RK (POI's HSSF turns every RK it reads into a NUMBER), whatever the summary stream it
    // kept from the file it started from says.
    if ((walk.formulas > 0 && walk.reserved_set == walk.formulas) || walk.bad_write_access || walk.numbers_without_rk()) && !kind.is_foreign() {
        kind = ProducerKind::Other;
    }
    let written = version.map(|v| v >> 16).filter(|m| *m > 0);
    // The BOF field is Excel's own; another writer leaves it 0 (Excel 97), so it only dates a
    // file Excel says it wrote.
    let major = written.or(walk.last_saved.filter(|_| kind.is_excel()));
    let producer = Producer { application, version: major.map(|m| format!("{m}.0000")), kind };
    Some(XlsFacts { producer, uncalced: walk.uncalced })
}

/// The Excel major version of a BIFF8 BOF `verLastXLSaved` ([MS-XLS] 2.4.21 and its product
/// behavior notes: 0 Excel 97, 1 Excel 2000, 2 Excel 2002, 3 Excel 2003, 4 Excel 2007, 6 Excel
/// 2010, 7 Excel 2013).
fn excel_major_of_last_saved(v: u32) -> Option<u32> {
    matches!(v, 0..=4 | 6 | 7).then_some(v + 8)
}

/// What [`walk_records`] finds in a workbook stream.
#[derive(Debug, Default, PartialEq, Eq)]
struct Walk {
    /// The Excel version that last saved it (its first BOF, see [`bof_last_saved`]).
    last_saved: Option<u32>,
    /// A record is `Uncalced`.
    uncalced: bool,
    /// FORMULA records, and how many of them have `reserved1` set.
    formulas: usize,
    reserved_set: usize,
    /// The workbook globals' `WriteAccess`, in a stream without `FilePass`, isn't the string its
    /// BIFF version writes there (see [`is_write_access_string`]).
    bad_write_access: bool,
    /// NUMBER records in worksheets whose value an RK record could hold ([`fits_rk`]), and RK or
    /// MULRK records there.
    number_as_rk: usize,
    rk_records: usize,
}

impl Walk {
    /// Some worksheet has a NUMBER record for a value Excel would have written as RK, and none
    /// has an RK record.
    fn numbers_without_rk(&self) -> bool {
        self.number_as_rk > 0 && self.rk_records == 0
    }
}

/// Excel writes a number as an RK record when an RkNumber ([MS-XLS] 2.5.217) holds it exactly: a
/// 30-bit signed integer; a double whose 34 least significant bits are zero; or either of those
/// divided by 100, which Excel takes only for an `x` with `x * 100` integral and `i / 100 == x`
/// (`1.1` stays a NUMBER: 1.1 * 100 isn't 110). The corpus's 4,734 `.xls` that name Excel hold no
/// NUMBER of such a value next to an RK; the four with one have no RK at all, and are POI's.
fn fits_rk(x: f64) -> bool {
    const INT_LIMIT: f64 = 536_870_912.0; // 2^29
    if !x.is_finite() {
        return false;
    }
    if x.to_bits() & ((1 << 34) - 1) == 0 || (x.fract() == 0.0 && x.abs() < INT_LIMIT) {
        return true;
    }
    let hundredths = x * 100.0;
    hundredths.fract() == 0.0 && hundredths.abs() < INT_LIMIT && hundredths / 100.0 == x
}

/// [MS-XLS] 2.4.127 Formula.
const FORMULA: u16 = 0x0006;
/// `reserved1` of a FORMULA record's flags (after `cell` and `val`, at byte 14).
const FORMULA_RESERVED1: u16 = 0x0002;
/// [MS-XLS] 2.4.349 WriteAccess.
const WRITE_ACCESS: u16 = 0x005C;
/// [MS-XLS] 2.4.117 FilePass: the records after it are encrypted.
const FILE_PASS: u16 = 0x002F;
/// [MS-XLS] 2.4.66 EOF.
const EOF: u16 = 0x000A;
/// [MS-XLS] 2.4.180 Number: the cell (`rw`, `col`, `ixfe`: 6 bytes), then the value, an Xnum.
const NUMBER: u16 = 0x0203;
/// [MS-XLS] 2.4.220 RK: the cell, then an RkNumber.
const RK: u16 = 0x027E;
/// [MS-XLS] 2.4.141 MulRk: the cells of a row's run, each with an RkNumber.
const MUL_RK: u16 = 0x00BD;
/// The `dt` of a worksheet's BOF ([MS-XLS] 2.4.21).
const WORKSHEET: u16 = 0x0010;

/// `body` is an XLUnicodeString ([MS-XLS] 2.5.294): `cch`, then `fHighByte` with seven reserved
/// zero bits, then `cch` one- or two-byte characters that fit in the record.
fn is_xl_unicode_string(body: &[u8]) -> bool {
    let (Some(&[c0, c1]), Some(&flags)) = (body.get(..2), body.get(2)) else {
        return false;
    };
    let width = if flags == 1 { 2 } else { 1 };
    flags & 0xFE == 0 && usize::from(u16::from_le_bytes([c0, c1])).saturating_mul(width).saturating_add(3) <= body.len()
}

/// `body` is a BIFF5/BIFF7 byte string with an 8-bit length: `cch`, then `cch` code-page
/// characters that fit in the record (the Excel 5.0/95 format writes WRITEACCESS's user name so,
/// padded with spaces to the record's fixed size; Unicode strings are BIFF8's).
fn is_byte_string(body: &[u8]) -> bool {
    body.first().is_some_and(|&cch| usize::from(cch) < body.len())
}

/// `body` is the user-name string a `WriteAccess` record of a stream of BIFF version `vers`
/// holds: an XLUnicodeString in BIFF8, a byte string in BIFF5/7 ([`is_byte_string`]).
fn is_write_access_string(vers: Option<u16>, body: &[u8]) -> bool {
    if vers == Some(BIFF5) { is_byte_string(body) } else { is_xl_unicode_string(body) }
}

/// Walks the records of a workbook stream that starts with a BOF. Record headers stay readable
/// in an encrypted stream, but the FORMULA flags of one, and the values of its cells, are
/// ciphertext: they aren't counted.
fn walk_records(stream: &[u8]) -> Walk {
    let mut walk = Walk::default();
    let mut at = 0usize;
    let (mut globals, mut encrypted, mut vers) = (true, false, None);
    // The `dt` of every substream the walk is in (a chart's sits inside its worksheet's).
    let mut open: Vec<Option<u16>> = Vec::new();
    while let Some(&[k0, k1, l0, l1]) = stream.get(at..at.saturating_add(4)) {
        let kind = u16::from_le_bytes([k0, k1]);
        let len = usize::from(u16::from_le_bytes([l0, l1]));
        let start = at.saturating_add(4);
        let body = stream.get(start..start.saturating_add(len)).unwrap_or_default();
        if at == 0 {
            if kind != BOF {
                break;
            }
            walk.last_saved = bof_last_saved(body);
            vers = body.get(..2).and_then(|v| v.try_into().ok()).map(u16::from_le_bytes);
        }
        walk.uncalced |= kind == UNCALCED;
        if globals {
            encrypted |= kind == FILE_PASS;
            if kind == WRITE_ACCESS && !encrypted {
                walk.bad_write_access = !is_write_access_string(vers, body);
            }
            globals = kind != EOF;
        }
        match kind {
            BOF => open.push(body.get(2..4).and_then(|dt| dt.try_into().ok()).map(u16::from_le_bytes)),
            EOF => {
                open.pop();
            }
            FORMULA if !encrypted => {
                walk.formulas += 1;
                if let Some(&[f0, f1]) = body.get(14..16) {
                    walk.reserved_set += usize::from(u16::from_le_bytes([f0, f1]) & FORMULA_RESERVED1 != 0);
                }
            }
            NUMBER if !encrypted && open == [Some(WORKSHEET)] => {
                if let Some(&[v0, v1, v2, v3, v4, v5, v6, v7]) = body.get(6..14) {
                    walk.number_as_rk += usize::from(fits_rk(f64::from_le_bytes([v0, v1, v2, v3, v4, v5, v6, v7])));
                }
            }
            RK | MUL_RK if !encrypted && open == [Some(WORKSHEET)] => walk.rk_records += 1,
            _ => {}
        }
        at = start.saturating_add(len);
    }
    walk
}

/// The Excel major version a workbook-globals BOF body dates its cache to: a BIFF8 BOF's
/// `verLastXLSaved`; a BIFF5 BOF's format, the Excel 5.0/95 Binary File Format ("Microsoft Excel
/// 5.0/95 Workbook" in "File formats that are supported in Excel"), as Excel 95 (7), the last
/// version whose own format it is. A BIFF5 stream has no field naming the Excel that wrote it; a
/// later one saving down to it caches its own algorithms' results, so the date matters only where
/// a cache differs from GridCraft's by an older algorithm's inaccuracy (see `legacy::explains`).
fn bof_last_saved(body: &[u8]) -> Option<u32> {
    let vers = u16::from_le_bytes(body.get(..2)?.try_into().ok()?);
    if vers == BIFF5 {
        return Some(EXCEL_95);
    }
    let flags = u32::from_le_bytes(body.get(12..16)?.try_into().ok()?);
    if vers != BIFF8 {
        return None;
    }
    excel_major_of_last_saved((flags >> 8) & 0xF)
}

/// A property value GridCraft reads.
#[derive(Debug, PartialEq, Eq)]
enum Property {
    Int(i32),
    Text(String),
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

/// Property `id` of the first property set of the property set stream `s` ([MS-OLEPS] 2.21
/// PropertySetStream, 2.20 PropertySet): a 4-byte integer or a code-page string, cut at its first
/// NUL and read as Latin-1 (the application names are ASCII).
fn property(s: &[u8], id: u32) -> Option<Property> {
    if s.get(..2)? != [0xFE, 0xFF] {
        return None;
    }
    let set = usize::try_from(u32_at(s, 44)?).ok()?;
    let count = usize::try_from(u32_at(s, set.checked_add(4)?)?).ok()?.min(MAX_PROPERTIES);
    let offset = (0..count).find_map(|i| {
        let entry = set.checked_add(8)?.checked_add(i.checked_mul(8)?)?;
        (u32_at(s, entry)? == id).then(|| u32_at(s, entry + 4)).flatten()
    })?;
    let at = set.checked_add(usize::try_from(offset).ok()?)?;
    let vt = u16::from_le_bytes(s.get(at..at.checked_add(2)?)?.try_into().ok()?);
    match vt {
        VT_I4 => Some(Property::Int(i32::from_le_bytes(s.get(at + 4..at + 8)?.try_into().ok()?))),
        VT_LPSTR => {
            let len = usize::try_from(u32_at(s, at + 4)?).ok()?;
            let bytes = s.get(at + 8..(at + 8).checked_add(len)?.min(s.len()))?;
            let text = bytes.split(|b| *b == 0).next().unwrap_or_default();
            Some(Property::Text(text.iter().map(|&b| char::from(b)).collect::<String>().trim().to_string()))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn record(kind: u16, body: &[u8]) -> Vec<u8> {
        let mut r = kind.to_le_bytes().to_vec();
        r.extend_from_slice(&u16::try_from(body.len()).unwrap().to_le_bytes());
        r.extend_from_slice(body);
        r
    }

    /// A BIFF8 workbook-globals BOF whose `verLastXLSaved` is `last_saved`.
    fn bof(last_saved: u32) -> Vec<u8> {
        let mut b = BIFF8.to_le_bytes().to_vec();
        b.extend_from_slice(&0x0005u16.to_le_bytes()); // dt: workbook globals
        b.extend_from_slice(&6319u16.to_le_bytes()); // rupBuild
        b.extend_from_slice(&1997u16.to_le_bytes()); // rupYear
        b.extend_from_slice(&0x0000_C009u32.to_le_bytes()); // fWin, fWinAny, verXLHigh
        b.extend_from_slice(&(6 | (last_saved << 8)).to_le_bytes()); // verLowestBiff, verLastXLSaved
        record(BOF, &b)
    }

    /// A property set stream holding one property set with the property `id` = `value`
    /// (`type`, then the value's bytes).
    fn property_set(id: u32, vt: u16, value: &[u8]) -> Vec<u8> {
        let mut s = vec![0xFE, 0xFF, 0, 0, 0, 0, 0, 0];
        s.extend_from_slice(&[0; 16]); // CLSID
        s.extend_from_slice(&1u32.to_le_bytes());
        s.extend_from_slice(&[0; 16]); // FMTID0
        s.extend_from_slice(&48u32.to_le_bytes()); // Offset0
        let mut property = vt.to_le_bytes().to_vec();
        property.extend_from_slice(&[0, 0]);
        property.extend_from_slice(value);
        let header = 8 + 8;
        s.extend_from_slice(&u32::try_from(header + property.len()).unwrap().to_le_bytes());
        s.extend_from_slice(&1u32.to_le_bytes());
        s.extend_from_slice(&id.to_le_bytes());
        s.extend_from_slice(&u32::try_from(header).unwrap().to_le_bytes());
        s.extend_from_slice(&property);
        s
    }

    fn app_name(name: &str) -> Vec<u8> {
        let mut v = u32::try_from(name.len() + 1).unwrap().to_le_bytes().to_vec();
        v.extend_from_slice(name.as_bytes());
        v.push(0);
        property_set(PIDSI_APPNAME, VT_LPSTR, &v)
    }

    fn xls(streams: &[(&str, &[u8])]) -> Vec<u8> {
        gridcraft_xlsx::testing::compound_file(streams)
    }

    #[test]
    fn the_summary_streams_name_the_application_and_its_version() {
        let book = [bof(1), record(0x000A, &[])].concat();
        let si = app_name("Microsoft Excel");
        let dsi = property_set(PIDDSI_VERSION, VT_I4, &0x000B_1A2Bu32.to_le_bytes());
        let facts = read(&xls(&[("Workbook", &book), ("\u{5}SummaryInformation", &si), ("\u{5}DocumentSummaryInformation", &dsi)])).unwrap();
        assert_eq!(
            facts.producer,
            Producer { application: Some("Microsoft Excel".into()), version: Some("11.0000".into()), kind: ProducerKind::Excel }
        );
        assert!(!facts.uncalced);
        // Without DocumentSummaryInformation the BOF dates a file Excel says it wrote …
        let facts = read(&xls(&[("Workbook", &book), ("\u{5}SummaryInformation", &si)])).unwrap();
        assert_eq!(facts.producer.version.as_deref(), Some("9.0000"));
        // … and nothing else: other writers leave verLastXLSaved 0.
        let facts = read(&xls(&[("Workbook", &book)])).unwrap();
        assert_eq!(facts.producer, Producer::default());
        assert_eq!(read(b"PK\x03\x04 not a compound file"), None);
    }

    #[test]
    fn an_excel_5_0_95_workbook_is_dated_by_its_format() {
        // A BIFF5 BOF: vers, dt, rupBuild, rupYear (Excel 5.0/95 writes nothing that names its version).
        let biff5 = record(BOF, &[BIFF5.to_le_bytes(), 0x0005u16.to_le_bytes(), 4413u16.to_le_bytes(), 1994u16.to_le_bytes()].concat());
        let book = [biff5, record(0x000A, &[])].concat();
        let si = app_name("Microsoft Excel");
        let facts = read(&xls(&[("Book", &book), ("\u{5}SummaryInformation", &si)])).unwrap();
        assert_eq!(facts.producer.version.as_deref(), Some("7.0000"));
        // Only a file Excel says it wrote.
        assert_eq!(read(&xls(&[("Book", &book)])).unwrap().producer, Producer::default());
    }

    #[test]
    fn an_office_build_without_an_application_name_is_excel_s() {
        let book = [bof(0), record(0x000A, &[])].concat();
        // Excel 98 for the Mac: 8.0, build 4922, and no PIDSI_APPNAME.
        let dsi = property_set(PIDDSI_VERSION, VT_I4, &0x0008_133Au32.to_le_bytes());
        let facts = read(&xls(&[("Workbook", &book), ("\u{5}DocumentSummaryInformation", &dsi)])).unwrap();
        assert_eq!(facts.producer, Producer { application: None, version: Some("8.0000".into()), kind: ProducerKind::Excel });
        // A version without a build says nothing about the writer.
        let dsi = property_set(PIDDSI_VERSION, VT_I4, &0x000E_0000u32.to_le_bytes());
        let facts = read(&xls(&[("Workbook", &book), ("\u{5}DocumentSummaryInformation", &dsi)])).unwrap();
        assert_eq!(facts.producer, Producer { application: None, version: Some("14.0000".into()), kind: ProducerKind::Unknown });
    }

    #[test]
    fn an_uncalced_record_says_formulas_were_pending() {
        let book = [bof(3), record(UNCALCED, &[0, 0]), record(0x000A, &[])].concat();
        assert!(read(&xls(&[("Workbook", &book)])).unwrap().uncalced);
        // A stream that doesn't start with a BOF isn't walked.
        let book = [record(UNCALCED, &[0, 0]), bof(3)].concat();
        assert!(!read(&xls(&[("Book", &book)])).unwrap().uncalced);
    }

    /// A FORMULA record: cell, cached value, then the flags.
    fn formula(flags: u16) -> Vec<u8> {
        let mut b = vec![0; 14];
        b.extend_from_slice(&flags.to_le_bytes());
        b.extend_from_slice(&[0; 4]); // chn
        b.extend_from_slice(&[0, 0]); // an empty CellParsedFormula
        record(FORMULA, &b)
    }

    #[test]
    fn formulas_that_all_set_a_bit_excel_keeps_zero_are_another_writer_s() {
        let si = app_name("Microsoft Excel");
        let book =
            |flags: &[u16]| [bof(0)].into_iter().chain(flags.iter().map(|f| formula(*f))).chain([record(0x000A, &[])]).collect::<Vec<_>>().concat();
        let kind = |flags: &[u16]| read(&xls(&[("Workbook", &book(flags)), ("\u{5}SummaryInformation", &si)])).unwrap().producer.kind;
        assert_eq!(kind(&[0x0003, 0x0002]), ProducerKind::Other);
        assert_eq!(kind(&[0x0003, 0x0001]), ProducerKind::Excel, "one Excel-like formula: not all set");
        assert_eq!(kind(&[0x0001]), ProducerKind::Excel);
        assert_eq!(kind(&[]), ProducerKind::Excel, "no formulas say nothing");
    }

    #[test]
    fn a_write_access_that_isnt_an_xl_unicode_string_is_another_writer_s() {
        let si = app_name("Microsoft Excel");
        let kind = |records: &[Vec<u8>]| {
            let book = [vec![bof(0)], records.to_vec(), vec![record(0x000A, &[])]].concat().concat();
            read(&xls(&[("Workbook", &book), ("\u{5}SummaryInformation", &si)])).unwrap().producer.kind
        };
        let padded = |name: &[u8]| [name, &vec![b' '; 112 - name.len()]].concat();
        // Excel: cch 5, fHighByte 0, "Excel", padded with spaces to 112 bytes.
        assert_eq!(kind(&[record(WRITE_ACCESS, &padded(b"\x05\x00\x00Excel"))]), ProducerKind::Excel);
        assert_eq!(kind(&[record(WRITE_ACCESS, &padded(b"\x02\x00\x01E\x00x\x00"))]), ProducerKind::Excel);
        // xlwt: the bare name, so `cch` reads as "No" = 28494 characters.
        assert_eq!(kind(&[record(WRITE_ACCESS, &padded(b"None"))]), ProducerKind::Other);
        // An encrypted stream's WriteAccess is ciphertext.
        assert_eq!(kind(&[record(FILE_PASS, &[0, 0]), record(WRITE_ACCESS, &padded(b"None"))]), ProducerKind::Excel);
    }

    #[test]
    fn a_biff5_write_access_is_a_byte_string() {
        let si = app_name("Microsoft Excel");
        let biff5 = record(BOF, &[BIFF5.to_le_bytes(), 0x0005u16.to_le_bytes(), 4413u16.to_le_bytes(), 1994u16.to_le_bytes()].concat());
        let kind = |access: &[u8]| {
            let book = [biff5.clone(), record(WRITE_ACCESS, access), record(0x000A, &[])].concat();
            read(&xls(&[("Book", &book), ("\u{5}SummaryInformation", &si)])).unwrap().producer
        };
        // Excel 5.0/95: an 8-bit cch, then the name, padded with spaces to 54 bytes; it is no
        // XLUnicodeString (cch would read as 0x6108 characters).
        let excel95 = [b"\x08abraharo".as_slice(), &[b' '; 45]].concat();
        assert_eq!(
            kind(&excel95),
            Producer { application: Some("Microsoft Excel".into()), version: Some("7.0000".into()), kind: ProducerKind::Excel }
        );
        // A byte string longer than its record isn't one.
        assert_eq!(kind(b"\x40abc").kind, ProducerKind::Other);
        assert_eq!(kind(b"").kind, ProducerKind::Other);
    }

    #[test]
    fn rk_holds_integers_binary_fractions_and_hundredths() {
        for x in [0.0, -0.0, 1.0, -7.0, 12345.0, 536_870_911.0, 0.5, 0.1, 12.34, 0.99, -0.078_125] {
            assert!(fits_rk(x), "{x}");
        }
        // Values Excel's own files hold as NUMBER: 1.1 * 100 isn't 110, and 89.6 * 100 is 8960
        // but 8960 / 100 isn't 89.60000000000001.
        for x in [1.1, 291.4, 2235.2, 89.600_000_000_000_01, 1_000_000_007.0, 1e300, f64::NAN, f64::INFINITY] {
            assert!(!fits_rk(x), "{x}");
        }
    }

    /// A BIFF8 BOF of a substream of type `dt`.
    fn sheet_bof(dt: u16) -> Vec<u8> {
        let mut b = BIFF8.to_le_bytes().to_vec();
        b.extend_from_slice(&dt.to_le_bytes());
        b.extend_from_slice(&[0; 12]);
        record(BOF, &b)
    }

    fn number(x: f64) -> Vec<u8> {
        record(NUMBER, &[[0u8; 6].as_slice(), &x.to_le_bytes()].concat())
    }

    /// The globals, then a worksheet holding `cells`.
    fn with_sheet(cells: &[Vec<u8>]) -> Vec<u8> {
        [vec![bof(0), record(EOF, &[]), sheet_bof(WORKSHEET)], cells.to_vec(), vec![record(EOF, &[])]].concat().concat()
    }

    /// A NUMBER record for a value an RK record holds, with no RK record in the workbook, is
    /// another writer's: POI's HSSF reads every RK as a number and writes it as NUMBER, and keeps
    /// the summary streams of the Excel file it started from (poi 44861.xls).
    #[test]
    fn a_number_record_for_a_value_rk_holds_is_another_writer_s() {
        let si = app_name("Microsoft Excel");
        let kind = |cells: &[Vec<u8>]| read(&xls(&[("Workbook", &with_sheet(cells)), ("\u{5}SummaryInformation", &si)])).unwrap().producer.kind;
        let rk = || record(RK, &[0; 10]);
        assert_eq!(kind(&[number(12.0)]), ProducerKind::Other);
        assert_eq!(kind(&[number(1.1), number(0.5)]), ProducerKind::Other, "one is enough");
        // Excel writes NUMBER for what RK cannot hold …
        assert_eq!(kind(&[number(1.1), number(291.4), number(1e300)]), ProducerKind::Excel);
        assert_eq!(kind(&[]), ProducerKind::Excel);
        // … and RK or MULRK for the rest: a workbook with either is Excel's.
        assert_eq!(kind(&[number(12.0), rk()]), ProducerKind::Excel);
        assert_eq!(kind(&[record(MUL_RK, &[0; 12]), number(12.0)]), ProducerKind::Excel);
        // Only a worksheet's own records count, not the cached values of a chart in it.
        let chart = [vec![sheet_bof(0x0020)], vec![number(12.0)], vec![record(EOF, &[])]].concat();
        assert_eq!(kind(&chart), ProducerKind::Excel);
    }

    #[test]
    fn hostile_property_streams_never_panic() {
        let si = app_name("Microsoft Excel");
        for cut in 0..si.len() {
            let _ = property(&si[..cut], PIDSI_APPNAME);
        }
        for at in [44usize, 52, 56, 60, 64, 68] {
            let mut s = si.clone();
            s[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
            let _ = property(&s, PIDSI_APPNAME);
        }
        assert_eq!(walk_records(&[0x09, 0x08, 0xFF, 0xFF, 1]), Walk::default());
    }
}
