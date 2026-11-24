//! Who wrote the workbook: the producer of the values it caches next to its formulas. Read from
//! what the package says about itself (the `Application` of its extended properties, or the
//! signature of an export that writes none), never from the file's name or from where it came from.

use std::io::Read as _;

use super::package::{local_name, part_key, read_zip_part, workbook_entry, zip_names, zip_package};
use super::rels::related_part;

/// The product that wrote a workbook.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProducerKind {
    /// Microsoft Excel for Windows (or the mobile apps, which carry its calculation engine).
    Excel,
    ExcelMac,
    ExcelOnline,
    LibreOffice,
    Wps,
    OnlyOffice,
    GoogleSheets,
    Poi,
    /// This program: the package was saved by GridCraft, so the cache is GridCraft's own.
    GridCraft,
    /// Another product the package names: a library, a converter, Access…
    Other,
    /// The package names no application and carries no export signature that does.
    #[default]
    Unknown,
}

impl ProducerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ProducerKind::Excel => "excel",
            ProducerKind::ExcelMac => "excelMac",
            ProducerKind::ExcelOnline => "excelOnline",
            ProducerKind::LibreOffice => "libreOffice",
            ProducerKind::Wps => "wps",
            ProducerKind::OnlyOffice => "onlyOffice",
            ProducerKind::GoogleSheets => "googleSheets",
            ProducerKind::Poi => "poi",
            ProducerKind::GridCraft => "gridCraft",
            ProducerKind::Other => "other",
            ProducerKind::Unknown => "unknown",
        }
    }

    /// Excel itself, in any of its editions, calculated what the file caches.
    pub fn is_excel(self) -> bool {
        matches!(self, ProducerKind::Excel | ProducerKind::ExcelMac | ProducerKind::ExcelOnline)
    }

    /// The file says that a product other than Excel wrote it. A package that says nothing, and
    /// one GridCraft wrote, are not: nothing in them argues against the cache.
    pub fn is_foreign(self) -> bool {
        !self.is_excel() && !matches!(self, ProducerKind::GridCraft | ProducerKind::Unknown)
    }
}

/// What a package says about the application that wrote it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Producer {
    /// `<Application>` of the extended properties (`docProps/app.xml`), trimmed; `None` when the
    /// package has no such part, or none with an application in it.
    pub application: Option<String>,
    /// `<AppVersion>`.
    pub version: Option<String>,
    pub kind: ProducerKind,
}

/// The namespaces Google Sheets declares on the root of every workbook it exports (the Mac Excel
/// 2008 VML and the Excel 2006 `xm` namespaces, next to its own choice of markup compatibility
/// prefixes). It writes no `docProps`, so these are all the package says about itself. Excel never
/// declares the `xm` namespace on a workbook.
const GOOGLE_EXPORT_NAMESPACES: [&str; 2] = ["urn:schemas-microsoft-com:mac:vml", "http://schemas.microsoft.com/office/excel/2006/main"];

/// The stem of LibreOffice's own extension namespaces (`loext`, `calcext`, `field`, `formx`:
/// `urn:org:documentfoundation:names:experimental:calc:xmlns:calcext:1.0`…), The Document
/// Foundation's namespaces for what ODF doesn't define, declared on the document root by
/// LibreOffice (and Collabora, built from it) whenever it saves. A document that names no
/// generator (LibreOffice's test documents strip `<office:meta>`) is still recognised by them.
const LIBREOFFICE_NAMESPACE_STEM: &str = "urn:org:documentfoundation:names:experimental:";

/// The custom document property Kingsoft WPS Office writes into every workbook it saves (its
/// language and build, `2052-12.1.0.16729`), next to an `Application` of `Microsoft Excel`. Excel
/// keeps custom properties it didn't write, and saving it in Excel doesn't replace WPS's values:
/// Excel recalculates a workbook on open only when its `calcId` is older than Excel's own
/// engine, and otherwise only the cells it marked dirty. A workbook with it holds WPS's results.
const WPS_BUILD_PROPERTY: &str = "KSOProductBuildVer";

/// The most bytes of a part read to find what its root declares.
const ROOT_HEAD: u64 = 64 * 1024;

/// The product an `Application` string names.
pub(super) fn kind_of_application(application: &str) -> ProducerKind {
    let a = application.trim().to_lowercase();
    if a.is_empty() {
        return ProducerKind::Unknown;
    }
    // Libraries that borrow Excel's name for compatibility say so after it
    // (`Microsoft Excel Compatible / Openpyxl 3.1`).
    if a.contains("compatible") || a.contains("openpyxl") {
        return ProducerKind::Other;
    }
    if a.starts_with("microsoft macintosh excel") {
        ProducerKind::ExcelMac
    } else if a.starts_with("microsoft excel online") {
        ProducerKind::ExcelOnline
    } else if a.starts_with("microsoft excel") || a == "excel" || a.starts_with("excel android") || a.starts_with("excel ios") {
        ProducerKind::Excel
    } else if a.starts_with("libreoffice") || a.starts_with("collabora") {
        ProducerKind::LibreOffice
    } else if a.starts_with("wps") || a.contains("kingsoft") {
        ProducerKind::Wps
    } else if a.starts_with("onlyoffice") {
        ProducerKind::OnlyOffice
    } else if a.starts_with("apache poi") {
        ProducerKind::Poi
    } else if a.contains("google") {
        ProducerKind::GoogleSheets
    } else if a.starts_with("gridcraft") {
        ProducerKind::GridCraft
    } else {
        ProducerKind::Other
    }
}

/// `(Application, AppVersion)` of an extended properties part.
fn app_properties(xml: &[u8]) -> (Option<String>, Option<String>) {
    (first_text(xml, b"Application"), first_text(xml, b"AppVersion"))
}

/// The trimmed text of the first element with the local name `name` that has any.
fn first_text(xml: &[u8], name: &[u8]) -> Option<String> {
    use quick_xml::XmlVersion;
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_reader(xml);
    let mut buf = Vec::new();
    let mut inside = false;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => inside = local_name(e.name().as_ref()) == name,
            Ok(Event::Text(t)) if inside => {
                if let Ok(text) = t.xml_content(XmlVersion::Explicit1_0)
                    && !text.trim().is_empty()
                {
                    return Some(text.trim().to_string());
                }
            }
            Ok(Event::End(_)) => inside = false,
            Ok(Event::Eof) | Err(_) => return None,
            Ok(_) => {}
        }
        buf.clear();
    }
}

/// What an OpenDocument file names as its producer: `<meta:generator>` (`LibreOffice/25.2…`,
/// `OpenOffice.org/3.2…`), in `meta.xml` of a package or in a flat `.fods` document itself.
fn odf_generator(xml: &[u8]) -> Option<String> {
    first_text(xml, b"generator")
}

/// The namespaces (and other attribute values) the root element of `xml` declares.
fn root_attribute_values(xml: &[u8]) -> Vec<String> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_reader(xml);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e) | Event::Empty(e)) => {
                return e.attributes().with_checks(false).flatten().map(|a| String::from_utf8_lossy(&a.value).into_owned()).collect();
            }
            Ok(Event::Eof) | Err(_) => return Vec::new(),
            Ok(_) => {}
        }
        buf.clear();
    }
}

/// Whether the XML `xml` declares every namespace of `uris` on its root element.
fn root_declares(xml: &[u8], uris: &[&str]) -> bool {
    let declared = root_attribute_values(xml);
    uris.iter().all(|u| declared.iter().any(|d| d == u))
}

/// Whether the root of the OpenDocument `xml` declares one of LibreOffice's extension namespaces.
fn root_declares_libreoffice(xml: &[u8]) -> bool {
    root_attribute_values(xml).iter().any(|v| v.starts_with(LIBREOFFICE_NAMESPACE_STEM))
}

/// Whether the root element of `xml` carries the attribute `key` (qualified name as written).
fn root_has_attribute(xml: &[u8], key: &[u8]) -> bool {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_reader(xml);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e) | Event::Empty(e)) => return e.attributes().with_checks(false).flatten().any(|a| a.key.as_ref() == key),
            Ok(Event::Eof) | Err(_) => return false,
            Ok(_) => {}
        }
        buf.clear();
    }
}

/// The value of the custom document property `name` (`docProps/custom.xml`).
fn custom_property(xml: &[u8], name: &str) -> Option<String> {
    use quick_xml::XmlVersion;
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_reader(xml);
    let mut buf = Vec::new();
    let mut inside = false;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) if local_name(e.name().as_ref()) == b"property" => {
                inside = e.attributes().with_checks(false).flatten().any(|a| a.key.as_ref() == b"name" && a.value.as_ref() == name.as_bytes());
            }
            Ok(Event::Text(t)) if inside => {
                if let Ok(text) = t.xml_content(XmlVersion::Explicit1_0)
                    && !text.trim().is_empty()
                {
                    return Some(text.trim().to_string());
                }
            }
            Ok(Event::End(e)) if local_name(e.name().as_ref()) == b"property" => inside = false,
            Ok(Event::Eof) | Err(_) => return None,
            Ok(_) => {}
        }
        buf.clear();
    }
}

/// The first `limit` bytes of `bytes`.
fn head(bytes: &[u8], limit: u64) -> &[u8] {
    bytes.get(..bytes.len().min(usize::try_from(limit).unwrap_or(usize::MAX))).unwrap_or_default()
}

/// The OpenDocument office namespace, declared on the root of every OpenDocument document.
const ODF_OFFICE_NAMESPACE: &str = "urn:oasis:names:tc:opendocument:xmlns:office:1.0";

/// The producer an OpenDocument file names (`generator`), or else the one its root `root`
/// declares the extension namespaces of. An OpenDocument document that names no generator wasn't
/// written by Excel, whose OpenDocument export always names itself (`MicrosoftOffice/16.0
/// MicrosoftExcel/CalculationVersion-…` in `<meta:generator>`, in every Excel-saved ODS of the
/// corpus): it is [`ProducerKind::Other`], and its cache isn't Excel's.
fn odf_producer(generator: Option<String>, root: &[u8]) -> Producer {
    let kind = match generator.as_deref() {
        Some(application) => kind_of_application(application),
        None if root_declares_libreoffice(root) => ProducerKind::LibreOffice,
        None if root_declares(root, &[ODF_OFFICE_NAMESPACE]) => ProducerKind::Other,
        None => ProducerKind::Unknown,
    };
    Producer { application: generator, version: None, kind }
}

/// The application that wrote the package `bytes`. An unreadable package, or one that says
/// nothing, is [`ProducerKind::Unknown`].
pub(super) fn detect(bytes: &[u8]) -> Producer {
    let Some(mut zip) = zip_package(bytes) else {
        // A flat OpenDocument file (`.fods`) is its own metadata.
        if bytes.trim_ascii_start().starts_with(b"<") {
            let head = head(bytes, ROOT_HEAD);
            return odf_producer(odf_generator(head), head);
        }
        return Producer::default();
    };
    let names = zip_names(&mut zip);
    let part = |zip: &mut super::package::Zip<'_>, key: &str| {
        let name = names.iter().find(|n| part_key(n) == key)?;
        read_zip_part(zip, name).ok()
    };
    let app_part =
        related_part(&mut zip, &names, "", "extended-properties").or_else(|| names.iter().find(|n| part_key(n) == "docprops/app.xml").cloned());
    let (application, version) = app_part.and_then(|p| read_zip_part(&mut zip, &p).ok()).map_or((None, None), |xml| app_properties(&xml));
    let Some(application) = application else {
        // An OpenDocument package has no extended properties; its `meta.xml` names the generator.
        if names.iter().any(|n| part_key(n) == "content.xml") {
            let generator = part(&mut zip, "meta.xml").and_then(|xml| odf_generator(&xml));
            let content = zip_part_head(&mut zip, "content.xml", ROOT_HEAD);
            return odf_producer(generator, &content);
        }
        // No application anywhere: the one export that writes no properties at all is recognised
        // by what it declares on the workbook root, another by the application its `fileVersion` names.
        let head = workbook_head(&mut zip, &names);
        let kind = if root_declares(&head, &GOOGLE_EXPORT_NAMESPACES) {
            ProducerKind::GoogleSheets
        } else {
            last_saved_by(&head).unwrap_or(ProducerKind::Unknown)
        };
        return Producer { version, kind, ..Default::default() };
    };
    let mut producer = Producer { kind: kind_of_application(&application), application: Some(application), version };
    if let Some(build) = part(&mut zip, "docprops/custom.xml").and_then(|xml| custom_property(&xml, WPS_BUILD_PROPERTY)) {
        producer.kind = ProducerKind::Wps;
        producer.version.get_or_insert(build);
    } else if producer.kind.is_excel() {
        let head = workbook_head(&mut zip, &names);
        if let Some(kind) = last_saved_by(&head) {
            producer.kind = kind;
        } else if root_has_attribute(&head, b"xml:space") {
            // Excel never writes `xml:space` on the workbook root (none of the corpus's 29,000
            // other Excel-named packages has it, and an Excel save writes the root anew); a
            // library that names its packages `Microsoft Excel` does, along with the Excel 2007
            // `fileVersion` it copies and no calculation chain: its values are its own.
            producer.kind = ProducerKind::Other;
        }
    }
    producer
}

/// The first [`ROOT_HEAD`] bytes of the package's workbook part, or nothing.
fn workbook_head(zip: &mut super::package::Zip<'_>, names: &[String]) -> Vec<u8> {
    workbook_entry(zip, names).map(|w| zip_part_head(zip, &w, ROOT_HEAD)).unwrap_or_default()
}

/// The product the workbook's `<fileVersion appName>` names, when it isn't Excel. ECMA-376
/// §18.2.10: `appName` "specifies the application that last saved the workbook"; Excel writes
/// `xl`, LibreOffice `Calc` (corpus: `xl` on every Excel-, WPS- and Excel Online-saved package
/// that has the attribute; `Calc` on LibreOffice's, `SpreadsheetGear …`, `SpreadsheetLight`,
/// `HCell` on the libraries'). `None` when the attribute is missing or names Excel.
fn last_saved_by(workbook: &[u8]) -> Option<ProducerKind> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_reader(workbook);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e) | Event::Empty(e)) if local_name(e.name().as_ref()) == b"fileVersion" => {
                let app = e.attributes().with_checks(false).flatten().find(|a| a.key.as_ref() == b"appName")?;
                let app = String::from_utf8_lossy(&app.value).trim().to_ascii_lowercase();
                return match app.as_str() {
                    "" | "xl" | "excel" => None,
                    "calc" => Some(ProducerKind::LibreOffice),
                    // What GridCraft writes for a workbook that had no `fileVersion` (an Excel save
                    // writes `xl` over it).
                    "gridcraft" => Some(ProducerKind::GridCraft),
                    _ => Some(ProducerKind::Other),
                };
            }
            // `fileVersion` is the workbook's first child: past it, there is none.
            Ok(Event::Start(e) | Event::Empty(e)) if local_name(e.name().as_ref()) != b"workbook" => return None,
            Ok(Event::Eof) | Err(_) => return None,
            Ok(_) => {}
        }
        buf.clear();
    }
}

/// The first `limit` bytes of the part `name`, or nothing when it can't be read.
fn zip_part_head(zip: &mut super::package::Zip<'_>, name: &str, limit: u64) -> Vec<u8> {
    let mut head = Vec::new();
    if let Ok(file) = zip.by_name(name) {
        let _ = file.take(limit).read_to_end(&mut head);
    }
    head
}
