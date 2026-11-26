//! Test support: builds password-encrypted workbooks and other hand-made packages in code, so
//! tests here and in the crates above (engine, UI) never need such a file on disk. Compiled for
//! this crate's tests and, elsewhere, only with the `test-support` feature (a dev-dependency
//! feature, never shipped). The encryption is `craft-offcrypto`'s and the compound file
//! `craft-cfb`'s.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

/// The spin count `encrypt_workbook` uses: Excel's default.
pub const SPIN_COUNT: u32 = 100_000;

/// `package` (normally an `.xlsx` zip) as Excel stores it with a password: Agile encryption
/// (AES-256, SHA-512) inside a compound file.
pub fn encrypt_workbook(package: &[u8], password: &str) -> Vec<u8> {
    craft_offcrypto::encrypt_with_spin_count(package, password, SPIN_COUNT).unwrap()
}

/// The same with Standard encryption (AES-128, SHA-1), as Excel 2007 wrote it.
pub fn encrypt_workbook_standard(package: &[u8], password: &str) -> Vec<u8> {
    craft_offcrypto::encrypt_standard(package, password, 128).unwrap()
}

/// A version 3 compound file (512-byte sectors) holding `streams` under the root: streams under
/// 4096 bytes in the mini stream, larger ones in regular sectors.
pub fn compound_file(streams: &[(&str, &[u8])]) -> Vec<u8> {
    craft_cfb::write_streams(craft_cfb::Version::V3, streams).unwrap()
}

/// One entry of a package [`damaged_zip`] builds.
#[derive(Clone, Copy)]
pub enum Entry<'a> {
    /// An intact entry (stored, with its checksum).
    Intact(&'a str, &'a [u8]),
    /// A deflate entry whose directory declares 0 bytes, 0 compressed bytes and no checksum, with
    /// no stream behind it and only a data descriptor after the header (general-purpose flag
    /// bit 3): what some producers write for an empty file, such as a thumbnail. Readers fail to
    /// decode it ("incomplete deflate stream"), though it says plainly that the part is empty.
    EmptyWithoutStream(&'a str),
    /// A deflate entry that declares `size` bytes and whose stream isn't valid deflate.
    BadStream(&'a str, u32),
    /// A stored entry whose checksum is wrong.
    BadChecksum(&'a str, &'a [u8]),
    /// A stored entry with a correct checksum but the wrong uncompressed size.
    BadSize(&'a str, &'a [u8], u32),
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 == 1 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// A zip package of `entries`, some of them damaged as their variants say. Built byte by byte so
/// that tests never need a damaged file on disk.
pub fn damaged_zip(entries: &[Entry<'_>]) -> Vec<u8> {
    struct Item<'a> {
        name: &'a str,
        flags: u16,
        method: u16,
        crc: u32,
        compressed: &'a [u8],
        size: u32,
    }
    let items: Vec<Item<'_>> = entries
        .iter()
        .map(|e| match *e {
            Entry::Intact(name, data) => Item { name, flags: 0, method: 0, crc: crc32(data), compressed: data, size: data.len() as u32 },
            Entry::EmptyWithoutStream(name) => Item { name, flags: 0x808, method: 8, crc: 0, compressed: &[], size: 0 },
            // Block type 3 is reserved: the first byte (final block, type 11) is not deflate.
            Entry::BadStream(name, size) => Item { name, flags: 0, method: 8, crc: 0, compressed: &[0x07, 0xFF, 0xFF, 0xFF], size },
            Entry::BadChecksum(name, data) => Item { name, flags: 0, method: 0, crc: crc32(data) ^ 1, compressed: data, size: data.len() as u32 },
            Entry::BadSize(name, data, size) => Item { name, flags: 0, method: 0, crc: crc32(data), compressed: data, size },
        })
        .collect();
    let mut out: Vec<u8> = vec![];
    let mut offsets = vec![];
    for it in &items {
        offsets.push(out.len() as u32);
        let described = it.flags & 0x8 != 0;
        out.extend(0x0403_4B50u32.to_le_bytes());
        out.extend(20u16.to_le_bytes());
        out.extend(it.flags.to_le_bytes());
        out.extend(it.method.to_le_bytes());
        out.extend([0, 0, 0x21, 0]);
        let (crc, compressed, size) = if described { (0, 0, 0) } else { (it.crc, it.compressed.len() as u32, it.size) };
        out.extend(crc.to_le_bytes());
        out.extend(compressed.to_le_bytes());
        out.extend(size.to_le_bytes());
        out.extend((it.name.len() as u16).to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out.extend(it.name.as_bytes());
        out.extend(it.compressed);
        if described {
            out.extend(0x0807_4B50u32.to_le_bytes());
            out.extend(it.crc.to_le_bytes());
            out.extend((it.compressed.len() as u32).to_le_bytes());
            out.extend(it.size.to_le_bytes());
        }
    }
    let directory = out.len() as u32;
    for (it, offset) in items.iter().zip(&offsets) {
        out.extend(0x0201_4B50u32.to_le_bytes());
        out.extend(20u16.to_le_bytes());
        out.extend(20u16.to_le_bytes());
        out.extend(it.flags.to_le_bytes());
        out.extend(it.method.to_le_bytes());
        out.extend([0, 0, 0x21, 0]);
        out.extend(it.crc.to_le_bytes());
        out.extend((it.compressed.len() as u32).to_le_bytes());
        out.extend(it.size.to_le_bytes());
        out.extend((it.name.len() as u16).to_le_bytes());
        out.extend([0u8; 12]);
        out.extend(offset.to_le_bytes());
        out.extend(it.name.as_bytes());
    }
    let size = out.len() as u32 - directory;
    out.extend(0x0605_4B50u32.to_le_bytes());
    out.extend([0u8; 4]);
    out.extend((items.len() as u16).to_le_bytes());
    out.extend((items.len() as u16).to_le_bytes());
    out.extend(size.to_le_bytes());
    out.extend(directory.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out
}

/// The names of the three check boxes [`legacy_check_boxes`] holds.
pub const CHECK_BOX_NAMES: [&str; 3] = ["GC Check 1", "GC Check 2", "GC Check 3"];

/// A one-sheet workbook ("Data") with three legacy form check boxes and an option button, as
/// ECMA-376 and MS-OI29500 describe them:
///
/// - `GC Check 1` (shape 1025): checked, linked to `$B$2` (which holds TRUE), flat
///   (`noThreeD`), caption "Ship it" in a `<font>`;
/// - `GC Check 2` (1026): unchecked, no link, caption "Second" in a plain `<div>`;
/// - `GC Check 3` (1027): mixed, a macro (`[0]!Mixed_Click`), caption "Maybe" in a `<font>`;
/// - an option button (1028), which GridCraft doesn't model.
///
/// With `x14` false the controls live only in the VML drawing, each check box named by its VML
/// `id` (its number in `o:spid`). With `x14` true the sheet lists them in `<controls>` (with their
/// names) and each has a control properties part, as Excel 2010 and later write them.
pub fn legacy_check_boxes(x14: bool) -> Vec<u8> {
    const NS_MAIN: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
    const NS_R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    let look = r#"filled="f" fillcolor="window [65]" stroked="f" strokecolor="windowText [64]" o:insetmode="auto""#;
    let font = |text: &str| format!(r#"<div style="text-align:left"><font face="Segoe UI" size="160" color="auto">{text}</font></div>"#);
    // (number, caption markup, client data after the anchor)
    let boxes = [
        (1025, font("Ship it"), "<x:FmlaLink>$B$2</x:FmlaLink><x:Checked>1</x:Checked><x:NoThreeD/>"),
        (1026, r#"<div style="text-align:left">Second</div>"#.to_string(), ""),
        (1027, font("Maybe"), "<x:FmlaMacro>[0]!Mixed_Click</x:FmlaMacro><x:Checked>2</x:Checked>"),
    ];
    let mut shapes = String::new();
    for (i, (n, caption, data)) in boxes.iter().enumerate() {
        let id = if x14 { format!("_x0000_s{n}") } else { format!("{}\" o:spid=\"_x0000_s{n}", CHECK_BOX_NAMES[i]) };
        let row = 1 + 2 * i;
        shapes.push_str(&format!(
            r##"<v:shape id="{id}" type="#_x0000_t201" style="position:absolute;margin-left:120pt;margin-top:{}pt;width:90pt;height:15pt;z-index:{}" {look}><v:fill color2="window [65]" o:detectmouseclick="t"/><o:lock v:ext="edit" rotation="t"/><v:textbox style="mso-direction-alt:auto" o:singleclick="f">{caption}</v:textbox><x:ClientData ObjectType="Checkbox"><x:Anchor>3, 0, {row}, 0, 5, 0, {}, 0</x:Anchor><x:AutoFill>False</x:AutoFill><x:AutoLine>False</x:AutoLine><x:TextVAlign>Center</x:TextVAlign>{data}</x:ClientData></v:shape>"##,
            15 * row,
            i + 1,
            row + 1,
        ));
    }
    shapes.push_str(r##"<v:shape id="_x0000_s1028" type="#_x0000_t201" style="position:absolute;margin-left:120pt;margin-top:120pt;width:90pt;height:15pt;z-index:4" filled="f" stroked="f" o:insetmode="auto"><v:textbox style="mso-direction-alt:auto" o:singleclick="f"><div style="text-align:left">Pick me</div></v:textbox><x:ClientData ObjectType="Radio"><x:Anchor>3, 0, 8, 0, 5, 0, 9, 0</x:Anchor><x:AutoFill>False</x:AutoFill><x:AutoLine>False</x:AutoLine><x:TextVAlign>Center</x:TextVAlign><x:FmlaLink>$C$9</x:FmlaLink><x:FirstButton/></x:ClientData></v:shape>"##);
    let vml = format!(
        r##"<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office" xmlns:x="urn:schemas-microsoft-com:office:excel"><o:shapelayout v:ext="edit"><o:idmap v:ext="edit" data="1"/></o:shapelayout><v:shapetype id="_x0000_t201" coordsize="21600,21600" o:spt="201" path="m,l,21600r21600,l21600,xe"><v:stroke joinstyle="miter"/></v:shapetype>{shapes}</xml>"##
    );
    let props = [
        r#"objectType="CheckBox" checked="Checked" fmlaLink="$B$2" lockText="1" noThreeD="1""#,
        r#"objectType="CheckBox" lockText="1""#,
        r#"objectType="CheckBox" checked="Mixed" lockText="1""#,
        r#"objectType="Radio" firstButton="1" fmlaLink="$C$9" lockText="1""#,
    ]
    .map(|a| format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><formControlPr xmlns="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main" {a}/>"#));
    let mut sheet_rels = format!(r#"<Relationship Id="rId1" Type="{REL}/vmlDrawing" Target="../drawings/vmlDrawing1.vml"/>"#);
    let mut controls = String::new();
    if x14 {
        let names = [CHECK_BOX_NAMES[0], CHECK_BOX_NAMES[1], CHECK_BOX_NAMES[2], "Option Button 4"];
        for (i, name) in names.iter().enumerate() {
            let (n, row) = (1025 + i, 1 + 2 * i);
            sheet_rels.push_str(&format!(r#"<Relationship Id="rId{}" Type="{REL}/ctrlProp" Target="../ctrlProps/ctrlProp{}.xml"/>"#, i + 2, i + 1));
            controls.push_str(&format!(
                r#"<mc:AlternateContent><mc:Choice Requires="x14"><control shapeId="{n}" r:id="rId{}" name="{name}"><controlPr defaultSize="0" autoFill="0" autoLine="0" autoPict="0"><anchor moveWithCells="1"><from><xdr:col>3</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>{row}</xdr:row><xdr:rowOff>0</xdr:rowOff></from><to><xdr:col>5</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>{}</xdr:row><xdr:rowOff>0</xdr:rowOff></to></anchor></controlPr></control></mc:Choice></mc:AlternateContent>"#,
                i + 2,
                row + 1
            ));
        }
        controls = format!(
            r#"<mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><mc:Choice Requires="x14"><controls xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing">{controls}</controls></mc:Choice></mc:AlternateContent>"#
        );
    }
    let sheet = format!(
        r#"<?xml version="1.0"?><worksheet xmlns="{NS_MAIN}" xmlns:r="{NS_R}" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:x14="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main"><sheetData><row r="2"><c r="B2" t="b"><v>1</v></c></row></sheetData><legacyDrawing r:id="rId1"/>{controls}</worksheet>"#
    );
    let types = r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="vml" ContentType="application/vnd.openxmlformats-officedocument.vmlDrawing"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#;
    let rels = |body: &str| format!(r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{body}</Relationships>"#);
    let root = rels(&format!(r#"<Relationship Id="rId1" Type="{REL}/officeDocument" Target="xl/workbook.xml"/>"#));
    let wb_rels = rels(&format!(r#"<Relationship Id="rId1" Type="{REL}/worksheet" Target="worksheets/sheet1.xml"/>"#));
    let workbook = format!(r#"<?xml version="1.0"?><workbook xmlns="{NS_MAIN}" xmlns:r="{NS_R}"><sheets><sheet name="Data" sheetId="1" r:id="rId1"/></sheets></workbook>"#);
    let sheet_rels = rels(&sheet_rels);
    let mut entries = vec![
        Entry::Intact("[Content_Types].xml", types.as_bytes()),
        Entry::Intact("_rels/.rels", root.as_bytes()),
        Entry::Intact("xl/workbook.xml", workbook.as_bytes()),
        Entry::Intact("xl/_rels/workbook.xml.rels", wb_rels.as_bytes()),
        Entry::Intact("xl/worksheets/sheet1.xml", sheet.as_bytes()),
        Entry::Intact("xl/worksheets/_rels/sheet1.xml.rels", sheet_rels.as_bytes()),
        Entry::Intact("xl/drawings/vmlDrawing1.vml", vml.as_bytes()),
    ];
    let names = ["xl/ctrlProps/ctrlProp1.xml", "xl/ctrlProps/ctrlProp2.xml", "xl/ctrlProps/ctrlProp3.xml", "xl/ctrlProps/ctrlProp4.xml"];
    if x14 {
        entries.extend(names.iter().zip(&props).map(|(n, p)| Entry::Intact(n, p.as_bytes())));
    }
    damaged_zip(&entries)
}
