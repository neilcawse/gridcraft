//! Bounded ZIP part reads, with `craft-opc` names, relationships and ZIP output,
//! and `craft-xlsx` worksheet streaming.

use std::collections::HashMap;
use std::io::{Cursor, Read as _};

use crate::IoError;
use crate::xml::{self, El};

pub use craft_opc::{normalize, resolve, split_dir};

/// Largest decompressed part accepted.
pub const MAX_PART: u64 = 512 * 1024 * 1024;
/// Largest total decompressed size read from one package.
pub const MAX_TOTAL: u64 = 1024 * 1024 * 1024;
/// Most entries accepted in the zip directory.
pub const MAX_ENTRIES: usize = 200_000;

const LIMITS: craft_opc::Limits = craft_opc::Limits::new(MAX_ENTRIES, MAX_PART, MAX_TOTAL);

pub(crate) type ZipArchive<'a> = zip::ZipArchive<Cursor<&'a [u8]>>;

pub(crate) struct Package<'a> {
    parts: ZipParts<'a>,
    craft: craft_xlsx::Package<'a>,
}

pub(crate) struct ZipParts<'a> {
    zip: ZipArchive<'a>,
    names: HashMap<String, usize>,
    total: u64,
}

/// A relationship.
#[derive(Clone, Debug)]
pub struct Rel {
    pub id: String,
    /// Last path segment of the relationship type (`worksheet`, `styles`, …): the same for the
    /// transitional and strict namespaces, but for the two kinds Strict spells without a hyphen
    /// (`extendedProperties`, `customProperties`), which are named as Transitional does.
    pub kind: String,
    /// Resolved part name (no leading `/`), or the raw target for external relationships.
    pub target: String,
    pub external: bool,
}

/// GridCraft's wording of a package error.
fn error(e: craft_opc::Error) -> IoError {
    use craft_opc::Error as E;
    match e {
        E::Zip(m) | E::Write(m) => IoError::Zip(m),
        E::TooManyEntries { declared, .. } => IoError::TooLarge(format!("{declared} zip entries")),
        E::PartTooLarge { part, limit } => IoError::TooLarge(format!("{part} expands to more than {limit} bytes")),
        E::PackageTooLarge { .. } => IoError::TooLarge("package expands to more than 1 GB".into()),
        E::Xml { part, error } => IoError::Xml(format!("{part}: {error}")),
        e if e.is_limit() => IoError::TooLarge(e.to_string()),
        e => IoError::Zip(e.to_string()),
    }
}

/// GridCraft's wording of a `craft-xlsx` error.
pub(crate) fn craft_error(e: craft_xlsx::Error) -> IoError {
    match e {
        craft_xlsx::Error::Zip(m) => IoError::Zip(m),
        craft_xlsx::Error::Xml(m) => IoError::Xml(m),
        craft_xlsx::Error::Invalid(m) => IoError::Format(m),
        craft_xlsx::Error::TooLarge(m) => IoError::TooLarge(m),
    }
}

pub(crate) fn open_zip(bytes: &[u8]) -> Result<ZipArchive<'_>, IoError> {
    craft_opc::check_entry_count(bytes, MAX_ENTRIES as u64).map_err(error)?;
    let zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| IoError::Zip(e.to_string()))?;
    if zip.len() > MAX_ENTRIES {
        return Err(IoError::TooLarge(format!("{} zip entries", zip.len())));
    }
    Ok(zip)
}

impl<'a> Package<'a> {
    pub fn open(bytes: &'a [u8]) -> Result<Package<'a>, IoError> {
        let parts = ZipParts::new(open_zip(bytes)?);
        let craft = craft_xlsx::Package::open(bytes, &craft_xlsx::Limits::default()).map_err(craft_error)?;
        Ok(Package { parts, craft })
    }

    pub(crate) fn craft_mut(&mut self) -> &mut craft_xlsx::Package<'a> {
        &mut self.craft
    }
}

impl<'a> std::ops::Deref for Package<'a> {
    type Target = ZipParts<'a>;

    fn deref(&self) -> &Self::Target {
        &self.parts
    }
}

impl std::ops::DerefMut for Package<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.parts
    }
}

impl<'a> ZipParts<'a> {
    pub(crate) fn new(zip: ZipArchive<'a>) -> Self {
        let mut names = HashMap::with_capacity(zip.len());
        for idx in 0..zip.len() {
            if let Some(name) = zip.name_for_index(idx) {
                names.entry(normalize(name)).or_insert(idx);
            }
        }
        Self { zip, names, total: 0 }
    }

    /// Every part's (normalized) name, sorted.
    #[cfg(test)]
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<_> = self.names.keys().cloned().collect();
        names.sort_unstable();
        names
    }

    pub fn has(&self, part: &str) -> bool {
        self.names.contains_key(&normalize(part))
    }

    /// Reads a part's bytes; `Ok(None)` when it doesn't exist.
    pub fn read(&mut self, part: &str) -> Result<Option<Vec<u8>>, IoError> {
        self.read_limited(part, MAX_PART)
    }

    /// Reads a small metadata part without allowing it to expand to the general part limit. A part
    /// whose directory entry declares no bytes is empty and is not decompressed: some producers
    /// write an empty file as a deflate entry with no stream at all, which no reader can decode.
    ///
    /// Success charges decoded bytes; ZIP failures charge the entire attempted read allowance.
    pub fn read_limited(&mut self, part: &str, maximum: u64) -> Result<Option<Vec<u8>>, IoError> {
        let maximum = maximum.min(MAX_PART);
        let Some(&idx) = self.names.get(&normalize(part)) else {
            return Ok(None);
        };
        let zip_error = |e| IoError::Zip(format!("{part}: {e}"));
        let remaining = MAX_TOTAL - self.total;
        let limit = maximum.min(remaining);
        let failed_charge = (limit + 1).min(remaining);
        let size = match self.zip.by_index_raw(idx) {
            Ok(f) => f.size(),
            Err(e) if remaining != 0 => {
                self.total += failed_charge;
                return Err(zip_error(e));
            }
            Err(_) => return Err(IoError::TooLarge("package expands to more than 1 GB".into())),
        };
        if size > maximum {
            return Err(IoError::TooLarge(format!("{part} expands to more than {maximum} bytes")));
        }
        if size == 0 {
            return Ok(Some(Vec::new()));
        }
        if remaining == 0 {
            return Err(IoError::TooLarge("package expands to more than 1 GB".into()));
        }
        let mut f = match self.zip.by_index(idx) {
            Ok(f) => f,
            Err(e) => {
                self.total += failed_charge;
                return Err(zip_error(e));
            }
        };
        let mut out = Vec::with_capacity(size.min(limit).min(16 * 1024 * 1024) as usize);
        let read = (&mut f).take(limit + 1).read_to_end(&mut out);
        let decoded = out.len() as u64;
        self.total += if read.is_err() || decoded != size { failed_charge } else { decoded.min(remaining) };
        if decoded > limit {
            return Err(IoError::TooLarge(if remaining < maximum {
                "package expands to more than 1 GB".into()
            } else {
                format!("{part} expands to more than {maximum} bytes")
            }));
        }
        read.map_err(|e| IoError::Zip(format!("{part}: {e}")))?;
        if decoded != size {
            return Err(IoError::Zip(format!("{part}: decoded {decoded} bytes, expected {size}")));
        }
        Ok(Some(out))
    }

    pub fn read_xml(&mut self, part: &str) -> Result<Option<El>, IoError> {
        match self.read(part)? {
            Some(b) => xml::parse(&b).map(Some).map_err(|e| prefix(part, e)),
            None => Ok(None),
        }
    }

    /// Relationships of a part (`xl/workbook.xml` → `xl/_rels/workbook.xml.rels`). Missing or
    /// broken rels parts give an empty list.
    pub fn rels(&mut self, part: &str) -> Result<Vec<Rel>, IoError> {
        let Some(bytes) = self.read(&craft_opc::rels_path(part))? else { return Ok(Vec::new()) };
        let rels = craft_opc::Rels::parse_in(&bytes, part, |n| self.has(n).then(|| n.to_string()));
        Ok(rels
            .list
            .into_iter()
            .map(|r| {
                let kind = match r.kind() {
                    "extendedProperties" => "extended-properties",
                    "customProperties" => "custom-properties",
                    other => other,
                }
                .to_string();
                Rel { id: r.id, kind, target: r.target, external: r.external }
            })
            .collect())
    }
}

/// The first part whose declared unpacked size is over [`MAX_PART`], with that size.
pub fn oversized_part(bytes: &[u8]) -> Option<(String, u64)> {
    craft_opc::declared_oversized(bytes, &LIMITS)
}

fn prefix(part: &str, e: IoError) -> IoError {
    match e {
        IoError::Xml(m) => IoError::Xml(format!("{part}: {m}")),
        o => o,
    }
}

// ---------------------------------------------------------------- writing

/// A zip package being written: every entry deflated, with one fixed timestamp.
pub struct ZipOut {
    zip: craft_opc::ZipOut,
}

impl ZipOut {
    pub fn new() -> ZipOut {
        ZipOut { zip: craft_opc::ZipOut::new() }
    }
    pub fn add(&mut self, name: &str, data: &[u8]) -> Result<(), IoError> {
        self.zip.add(name, data, craft_opc::Compression::Deflate).map_err(error)
    }
    pub fn finish(self) -> Result<Vec<u8>, IoError> {
        self.zip.finish().map_err(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Entry, damaged_zip};

    #[test]
    fn resolving() {
        assert_eq!(resolve("xl", "worksheets/sheet1.xml"), "xl/worksheets/sheet1.xml");
        assert_eq!(resolve("xl/worksheets", "../drawings/d1.xml"), "xl/drawings/d1.xml");
        assert_eq!(resolve("xl/worksheets", "/xl/tables/t.xml"), "xl/tables/t.xml");
        assert_eq!(resolve("", "xl/workbook.xml"), "xl/workbook.xml");
        assert_eq!(resolve("a", "../../../x"), "x");
        assert_eq!(split_dir("xl/workbook.xml"), ("xl", "workbook.xml"));
        assert_eq!(split_dir("x.xml"), ("", "x.xml"));
    }

    /// A read that runs over the package's allowance spends it: loops over many oversized parts
    /// (a sheet's optional VML relationships, for one) otherwise re-decompress the remaining
    /// allowance again and again.
    #[test]
    fn an_over_budget_read_ends_the_package_budget() {
        let z = crate::tests::make_zip(&[("a.xml", b"0123456789"), ("b.xml", b"hello")]);
        let mut pkg = Package::open(&z).unwrap();
        assert!(pkg.read("a.xml").unwrap().is_some());
        assert_eq!(pkg.total, 10);
        // Nothing left to spend within the per-part limit either.
        pkg.total = MAX_TOTAL - 1;
        assert!(matches!(pkg.read("b.xml"), Err(IoError::TooLarge(m)) if m.contains("1 GB")));
        assert_eq!(pkg.total, MAX_TOTAL, "the over-budget read is charged");
        // Repeated attempts stay cheap errors: no allowance is left to pay for.
        assert!(matches!(pkg.read("b.xml"), Err(IoError::TooLarge(_))));
    }

    #[test]
    fn a_damaged_part_spends_its_allowance_but_not_the_whole_budget() {
        let z = damaged_zip(&[Entry::BadChecksum("a.xml", b"<a>damaged</a>"), Entry::Intact("b.xml", b"hello")]);
        let mut pkg = Package::open(&z).unwrap();
        assert!(matches!(pkg.read_limited("a.xml", 1024), Err(IoError::Zip(_))));
        assert_eq!(pkg.total, 1025);
        assert_eq!(pkg.read("b.xml").unwrap().as_deref(), Some(&b"hello"[..]));
        // Reading the damaged part again fails the same way, never as an exhausted package.
        assert!(matches!(pkg.read("a.xml"), Err(IoError::Zip(_))));
        assert_eq!(pkg.total, 1025 + 5 + MAX_PART + 1);
    }

    #[test]
    fn a_failed_read_without_output_still_spends_the_budget() {
        let z = damaged_zip(&[Entry::BadStream("a.bin", 40), Entry::Intact("b.xml", b"hello")]);
        let mut pkg = Package::open(&z).unwrap();
        assert!(matches!(pkg.read_limited("a.bin", 1024), Err(IoError::Zip(_))));
        assert_eq!(pkg.total, 1025);
        assert_eq!(pkg.read_limited("b.xml", 1024).unwrap().as_deref(), Some(&b"hello"[..]));
        pkg.total = MAX_TOTAL - 2;
        assert!(matches!(pkg.read_limited("a.bin", 1024), Err(IoError::Zip(_))));
        assert_eq!(pkg.total, MAX_TOTAL);
        for part in ["a.bin", "b.xml"] {
            assert!(matches!(pkg.read(part), Err(IoError::TooLarge(m)) if m.contains("1 GB")));
        }
    }

    #[test]
    fn a_size_mismatch_with_valid_checksum_spends_the_failed_read_allowance() {
        let z = damaged_zip(&[Entry::BadSize("a.bin", b"a", 2)]);
        let mut pkg = Package::open(&z).unwrap();
        assert!(matches!(pkg.read_limited("a.bin", 8), Err(IoError::Zip(m)) if m == "a.bin: decoded 1 bytes, expected 2"));
        assert_eq!(pkg.total, 9);
    }

    /// A part whose directory entry declares 0 bytes is an empty part, read as such even when its
    /// deflate entry has no stream (the thumbnail some producers write that way).
    #[test]
    fn a_part_declared_empty_is_empty() {
        let z = damaged_zip(&[Entry::EmptyWithoutStream("docProps/thumbnail.wmf"), Entry::Intact("b.xml", b"hello")]);
        let mut pkg = Package::open(&z).unwrap();
        assert_eq!(pkg.read_limited("docProps/thumbnail.wmf", 1024).unwrap(), Some(vec![]));
        assert_eq!(pkg.total, 0);
        assert_eq!(pkg.read("b.xml").unwrap().as_deref(), Some(&b"hello"[..]));
    }
}
