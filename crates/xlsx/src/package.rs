//! Bounded ZIP part reads, with `craft-opc` names, relationships and ZIP output,
//! and `craft-xlsx` worksheet streaming.

use std::collections::HashMap;
use std::io::{Cursor, Read};

use crate::IoError;
use crate::xml::{self, El};

pub use craft_opc::{normalize, resolve, split_dir};

/// Largest decompressed part accepted.
pub const MAX_PART: u64 = 512 * 1024 * 1024;
/// Largest total decompressed size read from one package.
pub const MAX_TOTAL: u64 = 1024 * 1024 * 1024;
/// Most output reserved up front for one part; larger parts grow as they inflate.
const INITIAL_CAPACITY: u64 = 16 * 1024 * 1024;
/// Compressed bytes fed to the inflater at a time.
const INFLATE_INPUT: usize = 64 * 1024;
/// Least output a part grows by while it inflates (it otherwise doubles, up to its limit).
const INFLATE_OUTPUT: u64 = 1024 * 1024;

#[cfg(test)]
thread_local! {
    /// A smaller part limit for tests on this thread.
    pub(crate) static PART_LIMIT_FOR_TEST: std::cell::Cell<u64> = const { std::cell::Cell::new(MAX_PART) };
}

/// The part limit: [`MAX_PART`], or a test's smaller one.
fn max_part() -> u64 {
    #[cfg(test)]
    {
        PART_LIMIT_FOR_TEST.with(std::cell::Cell::get)
    }
    #[cfg(not(test))]
    {
        MAX_PART
    }
}

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
            if let Some(Ok(name)) = zip.name_for_index(idx) {
                names.entry(normalize(&name)).or_insert(idx);
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

    /// Reads a part the workbook can't do without (a worksheet). Running into the size limits is
    /// fatal: one that declares more bytes than the part limit or what remains of the package
    /// allowance is refused unread, and one that inflates past them is refused, each as
    /// [`IoError::TooLarge`]. A damaged one within the limits (a wrong checksum, invalid deflate,
    /// a size other than the declared one) is charged exactly the bytes it inflated.
    pub fn read_required(&mut self, part: &str) -> Result<Option<Vec<u8>>, IoError> {
        self.read_part(part, MAX_PART, true)
    }

    /// Reads a small metadata part without allowing it to expand to the general part limit. A part
    /// whose directory entry declares no bytes is empty and is not decompressed: some producers
    /// write an empty file as a deflate entry with no stream at all, which no reader can decode.
    ///
    /// Success charges decoded bytes; ZIP failures charge the entire attempted read allowance.
    pub fn read_limited(&mut self, part: &str, maximum: u64) -> Result<Option<Vec<u8>>, IoError> {
        self.read_part(part, maximum, false)
    }

    /// [`Self::read_limited`], or [`Self::read_required`] when `required`.
    fn read_part(&mut self, part: &str, maximum: u64, required: bool) -> Result<Option<Vec<u8>>, IoError> {
        use zip::CompressionMethod;
        let maximum = maximum.min(max_part());
        let Some(&idx) = self.names.get(&normalize(part)) else {
            return Ok(None);
        };
        let remaining = MAX_TOTAL - self.total;
        let limit = maximum.min(remaining);
        // What a failed read costs: a required part pays for what it inflated; an optional one for
        // the whole attempted allowance.
        let failed_charge = if required { 0 } else { (limit + 1).min(remaining) };
        let mut raw = match self.zip.by_index_raw(idx) {
            Ok(f) => f,
            Err(e) if remaining != 0 => {
                self.total += failed_charge;
                return Err(IoError::Zip(format!("{part}: {e}")));
            }
            Err(_) => return Err(IoError::TooLarge("package expands to more than 1 GB".into())),
        };
        let size = raw.size();
        if size > maximum {
            return Err(IoError::TooLarge(format!("{part} expands to more than {maximum} bytes")));
        }
        // A required part declared empty but with compressed bytes is inflated all the same: its
        // real output must still stay within the limits.
        if size == 0 && (!required || raw.compressed_size() == 0) {
            return Ok(Some(Vec::new()));
        }
        if remaining == 0 || (required && size > remaining) {
            self.total = if required { MAX_TOTAL } else { self.total + failed_charge };
            return Err(IoError::TooLarge("package expands to more than 1 GB".into()));
        }
        let method = raw.compression();
        let unsupported = if raw.encrypted() {
            Some("the entry is encrypted".to_string())
        } else if !matches!(method, CompressionMethod::Stored | CompressionMethod::Deflated) {
            Some(format!("unsupported compression method {method}"))
        } else {
            None
        };
        if let Some(why) = unsupported {
            self.total += failed_charge;
            return Err(IoError::Zip(format!("{part}: {why}")));
        }
        let expected_crc = raw.crc32();
        let mut out = Vec::with_capacity(size.min(limit).min(INITIAL_CAPACITY) as usize);
        let read = inflate(&mut raw, method == CompressionMethod::Deflated, limit + 1, &mut out);
        let decoded = out.len() as u64;
        let damage = match read {
            Err(e) => Some(e),
            Ok(()) if decoded != size => Some(format!("decoded {decoded} bytes, expected {size}")),
            Ok(()) if decoded <= limit && crc32fast::hash(&out) != expected_crc => Some("invalid checksum".to_string()),
            Ok(()) => None,
        };
        self.total += if damage.is_some() && !required { failed_charge } else { decoded.min(remaining) };
        if decoded > limit {
            return Err(IoError::TooLarge(if remaining < maximum {
                "package expands to more than 1 GB".into()
            } else {
                format!("{part} expands to more than {maximum} bytes")
            }));
        }
        match damage {
            Some(e) => Err(IoError::Zip(format!("{part}: {e}"))),
            None => Ok(Some(out)),
        }
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

/// Inflates the compressed bytes `raw` (stored as they are unless `deflated`) into `out`, stopping
/// once `out` holds `cap` bytes. `out` always holds exactly what was inflated, also when the stream
/// turns out to be damaged, so a failed read is charged what it really cost.
fn inflate(raw: &mut impl Read, deflated: bool, cap: u64, out: &mut Vec<u8>) -> Result<(), String> {
    use flate2::{Decompress, FlushDecompress, Status};
    if !deflated {
        return raw.take(cap).read_to_end(out).map(drop).map_err(|e| e.to_string());
    }
    let mut z = Decompress::new(false);
    let mut input = vec![0u8; INFLATE_INPUT];
    let (mut pos, mut end) = (0, 0);
    loop {
        if pos == end {
            end = raw.read(&mut input).map_err(|e| e.to_string())?;
            pos = 0;
        }
        let room = cap - out.len() as u64;
        if room == 0 {
            return Ok(());
        }
        if out.len() == out.capacity() {
            // Doubling, never past the cap: an understated part grows in few steps.
            out.reserve_exact(room.min((out.capacity() as u64).max(INFLATE_OUTPUT)) as usize);
        }
        let (in_before, out_before) = (z.total_in(), out.len());
        // `decompress_vec` keeps what it inflated in `out` even when it then fails.
        let status = z.decompress_vec(&input[pos..end], out, FlushDecompress::None).map_err(|e| e.to_string())?;
        pos += (z.total_in() - in_before) as usize;
        let stalled = out.len() == out_before && z.total_in() == in_before;
        match status {
            Status::StreamEnd => return Ok(()),
            // No progress with output room left: the input ran out mid-stream, or the inflater
            // can't use what remains of it.
            _ if stalled && (end == 0 || pos < end) => return Err("incomplete deflate stream".into()),
            _ => {}
        }
    }
}

/// Whether a relationship of kind `kind` (its type's last segment) is the path to another
/// workbook: the standard `externalLinkPath`, or one of the variants Excel writes under
/// `http://schemas.microsoft.com/office/2006/relationships/xlExternalLinkPath/` (the path was
/// missing when saved, or the workbook is in a startup or library folder) [MS-XLSX].
pub(crate) fn is_external_workbook_path(kind: &str) -> bool {
    matches!(kind, "externalLinkPath" | "xlPathMissing" | "xlStartup" | "xlAlternateStartup" | "xlLibrary")
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

    /// A damaged worksheet within the limits is charged exactly what it inflated (it is recovered
    /// as far as it reads), never a whole allowance; one whose output reaches the part limit, or
    /// that declares more than the part limit or what remains, is refused as too large.
    #[test]
    fn a_required_part_pays_for_what_it_inflated_and_stops_at_the_limits() {
        let z = damaged_zip(&[
            Entry::BadChecksum("s1.xml", b"<a>damaged</a>"),
            Entry::BadStream("s2.xml", 40),
            Entry::BadSize("s3.xml", b"a", 2),
            Entry::BadStream("huge.xml", (MAX_PART + 1) as u32),
            Entry::Intact("b.xml", b"hello"),
        ]);
        let mut pkg = Package::open(&z).unwrap();
        for (part, charged) in [("s1.xml", 14), ("s2.xml", 0), ("s3.xml", 1)] {
            let before = pkg.total;
            assert!(matches!(pkg.read_required(part), Err(IoError::Zip(_))), "{part}");
            assert_eq!(pkg.total - before, charged, "{part}");
        }
        assert_eq!(pkg.read_required("b.xml").unwrap().as_deref(), Some(&b"hello"[..]));
        assert_eq!(pkg.total, 14 + 1 + 5);
        assert!(matches!(pkg.read_required("huge.xml"), Err(IoError::TooLarge(m)) if m.contains("huge.xml")));
        pkg.total = MAX_TOTAL - 4;
        assert!(matches!(pkg.read_required("b.xml"), Err(IoError::TooLarge(m)) if m.contains("1 GB")));
        assert_eq!(pkg.total, MAX_TOTAL, "declared bytes beyond what remains end the budget");
    }

    /// Under a small part limit: output of exactly the limit with a wrong checksum is damage, charged
    /// what it inflated; one byte more, past a smaller declared size, is too large.
    #[test]
    fn a_required_part_is_too_large_only_when_its_output_passes_the_limit() {
        let data = [b'x'; 65];
        let z = damaged_zip(&[
            Entry::BadChecksum("at.xml", &data[..64]),
            Entry::BadChecksumAndSize("over.xml", &data, 60),
            Entry::BadChecksumAndSize("zero.xml", &data, 0),
        ]);
        PART_LIMIT_FOR_TEST.with(|c| c.set(64));
        let mut pkg = Package::open(&z).unwrap();
        assert!(matches!(pkg.read_required("at.xml"), Err(IoError::Zip(m)) if m.contains("checksum")));
        assert_eq!(pkg.total, 64);
        assert!(matches!(pkg.read_required("over.xml"), Err(IoError::TooLarge(m)) if m.contains("over.xml")));
        assert_eq!(pkg.total, 64 + 65);
        assert!(matches!(pkg.read_required("zero.xml"), Err(IoError::TooLarge(m)) if m.contains("zero.xml")), "declared empty");
        PART_LIMIT_FOR_TEST.with(|c| c.set(MAX_PART));
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
