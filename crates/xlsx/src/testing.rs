//! Test support: builds password-encrypted workbooks in code, so tests here and in the crates
//! above (engine, UI) never need an encrypted file on disk. Compiled for this crate's tests and,
//! elsewhere, only with the `test-support` feature (a dev-dependency feature, never shipped).
//! The encryption is `craft-offcrypto`'s and the compound file `craft-cfb`'s.
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
