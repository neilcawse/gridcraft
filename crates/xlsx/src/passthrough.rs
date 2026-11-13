//! Pass-through of package content GridCraft does not model, so that open → edit → save keeps it.
//!
//! The rule is a deny-list: every part GridCraft does not rebuild itself is written back byte for
//! byte, with its relationship chain re-targeted, and nothing is dropped silently. The kinds
//! GridCraft rebuilds are [`REWRITTEN`]; the owner's relationships of any other kind are kept
//! ([`is_root`]). Content comes in per owner: the package root (custom document properties, the
//! thumbnail, sensitivity labels, ribbon customisation, add-in task panes), the workbook (custom
//! XML, links to external workbooks with their `<externalReferences>`, data connections, the Power
//! Pivot data model, XML maps,
//! shared-workbook revision logs…) and each sheet (query tables, cells mapped to XML elements,
//! form controls other than the buttons, scroll bars and spin buttons `controls.rs` models,
//! ActiveX controls, OLE objects, the sheet background, custom sheet properties and
//! printer settings, with the sheet's `<picture>`, `<oleObjects>` and `<controls>` XML, the `r:id`
//! of `<pageSetup>`, every shape of the legacy VML drawing that isn't a note or a modelled
//! control (controls, buttons, pictures: GridCraft writes the notes and the controls it models)
//! and the pictures of the headers and
//! footers, `&G`, with their `<legacyDrawingHF>`). The workbook also keeps its
//! `<fileVersion>` and the calcFeatures extension, which Excel needs to trust the saved values.
//! The core and extended document properties are written from the few properties the document
//! models (title, author, company…); the other elements of those two parts (`cp:revision`,
//! `cp:category`, `dc:language`, the manager, the hyperlink base…) are kept as XML.
//!
//! Value metadata GridCraft does not model (rich errors with their reason, linked data types, cube
//! members: a cell's `vm`, the metadata part and the rich-value parts behind it) is kept as one
//! set ([`PreservedMetadata`]), with the cells that carry it. A cell gets its `vm` back only while
//! it holds the value and formula the metadata was read with; when no cell does, the set is not
//! written (a deleted cell's data does not stay in the file). GridCraft writes metadata of its own
//! for cell pictures: the kept set then gives way to it, but for the rich-value types part, which
//! holds only global flags. Its dynamic-array formulas are `cm="1"`: the kept metadata must have
//! that flag as its first cell metadata, or the set is left out.
//!
//! A picture that fills the box of a note (`<v:fill type="frame" o:relid>`) goes back into the
//! shape GridCraft writes for the note at the same cell, with the image ([`PreservedNoteFills`]).
//! It moves with the note when the note's cell does (rows or columns inserted or deleted, a sort,
//! a cut), as far as the note's sheet; a note that is gone leaves its picture behind.
//!
//! Sheets GridCraft does not model (chart sheets, Excel 5.0 dialog sheets, Excel 4.0 macro
//! sheets) are kept whole, at their place among the worksheets: the sheet part and everything
//! behind it ([`KEPT_SHEET_GRAPH`]). Macro sheets only go into macro-enabled workbooks, like the
//! VBA project. An Excel 5.0 VBA module sheet has no part, only its `<sheet>` entry with an empty
//! `r:id` (as Excel writes it): the entry is kept in place. The defined names scoped to a kept
//! sheet go with it, numbered by its written position, and edits that rename a sheet or move the
//! cells they name rewrite their formulas as they do the workbook's own ([`rewrite_kept_names`]).
//! A macro sheet's cells name their text by its index in the workbook's shared strings and their
//! format by its index in the cell formats (and a conditional format's rule its differential
//! format), which are written from the model: what they refer to is kept
//! ([`PreservedSheet::indexes`]) and the indexes given anew on save.
//!
//! On write, kept parts get fresh part names where needed and new relationship ids; the `r:id`s in
//! kept XML are rewritten to match. Kept content is conservative:
//! - Anything whose relationship graph reaches a part GridCraft reads and rewrites (worksheets,
//!   drawings, charts, tables, PivotTables, comments, slicers…) is not kept: it would point at
//!   content that no longer exists in the same form.
//! - External relationship targets (the path of a linked workbook) are kept as text and never
//!   opened, followed or refreshed. A relative path to a linked workbook is rewritten when the
//!   workbook is saved into another folder ([`rebase_links`]), so it still names the same file.
//! - Sheet content lives on the sheet: deleting the sheet drops it, Copy Sheet doesn't copy it.
//!   Cell references inside kept parts (a control's linked cell, a query table's range, a cell
//!   mapped to an XML element, a macro sheet's formulas) are kept as they were, not adjusted to
//!   inserted or deleted rows or to a renamed sheet.
//! - Strict Open XML files are passed through like the others: the names of their XML namespaces
//!   and relationship types are renamed to the Transitional ones GridCraft writes ([`strict`]).
//! - Sizes are bounded: per part and per workbook.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Write as _;
use std::path::{Component, Path};
use std::sync::Arc;

use gridcraft_core::{CellRef, MAX_COLS, MAX_ROWS};
use gridcraft_model::{
    Cell, Preserved, PreservedCellMeta, PreservedIndex, PreservedIndexed, PreservedMetadata, PreservedName, PreservedNoteFill, PreservedNoteFills,
    PreservedPart, PreservedRel, PreservedSheet, PreservedSheetKind, PreservedTable, PreservedTableColumn, PreservedTheme, PreservedXml, Sheet,
    Table, Visibility, Workbook,
};
use quick_xml::events::attributes::Attribute;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::QName;
use quick_xml::{Reader, Writer};

use crate::IoError;
use crate::package::{self, normalize};
use crate::read::Ctx;
use crate::write::{NS_REL, Out, Rels, XML_DECL};
use crate::xml::{self, esc_attr};

mod names;
mod read;
mod rebase;
pub(crate) mod strict;
mod write;

pub use names::rewrite_kept_names;
pub use read::{KeptReader, seal_cell_meta};
pub use rebase::{SheetKept, TableBinding, rebase_links};
#[cfg(test)]
pub(crate) use write::sheet_names_in;
pub use write::{
    KeptOut, SheetIds, WorkbookKept, cell_meta_holds, cell_meta_index, metadata_fits_dynamic_arrays, workbook_elements, write_metadata,
    write_package_rels, write_rich_types, write_theme, write_workbook,
};

/// Most bytes kept for one workbook.
pub const MAX_KEPT_TOTAL: usize = 256 * 1024 * 1024;
/// Largest single kept part.
pub const MAX_KEPT_PART: u64 = 64 * 1024 * 1024;
/// Most parts kept for one workbook.
const MAX_KEPT_PARTS: usize = 4096;
/// Most relationships read from one rels part.
const MAX_RELS: usize = 10_000;
/// Largest relationships, content-types or VML part read here.
const MAX_META: u64 = 16 * 1024 * 1024;
/// Longest chain of relationships followed from an owner.
const MAX_CHAIN: usize = 16;
/// Largest kept XML fragment of a sheet.
const MAX_FRAGMENT: usize = 4 * 1024 * 1024;
/// Deepest element nesting accepted in a kept fragment.
const MAX_FRAGMENT_DEPTH: usize = 256;
/// Most cells whose value metadata is kept for one workbook.
const MAX_CELL_META: usize = 100_000;
/// Most pictures kept in note boxes for one sheet.
const MAX_NOTE_FILLS: usize = 10_000;

#[cfg(test)]
thread_local! {
    /// A smaller kept-bytes allowance for tests on this thread.
    pub(crate) static KEPT_TOTAL_FOR_TEST: std::cell::Cell<usize> = const { std::cell::Cell::new(MAX_KEPT_TOTAL) };
}

fn kept_total() -> usize {
    #[cfg(test)]
    {
        KEPT_TOTAL_FOR_TEST.with(std::cell::Cell::get)
    }
    #[cfg(not(test))]
    {
        MAX_KEPT_TOTAL
    }
}

const NS_REL_STRICT: &str = "http://purl.oclc.org/ooxml/officeDocument/relationships";
const CT_VML: &str = "application/vnd.openxmlformats-officedocument.vmlDrawing";

/// Workbook relationship kinds that an element of the workbook XML takes care of: the links to
/// external workbooks are kept all together (formulas address them by position in
/// `<externalReferences>`), and GridCraft writes the feature property bag itself.
const WORKBOOK_ELEMENT_KINDS: &[&str] = &["externalLink", "featurePropertyBag"];
/// Sheet relationship kinds that mean the sheet XML may hold elements worth keeping, and that
/// those elements take care of (`<pageSetup r:id>`, `<oleObjects>`, `<controls>`, `<picture>`,
/// `<customProperties>`): kept through them, not as plain roots.
const SHEET_ELEMENT_KINDS: &[&str] = &["oleObject", "ctrlProp", "control", "image", "package", "printerSettings", "customProperty"];
/// Sheet relationship kinds GridCraft writes itself although they never block a part's graph
/// (external links are text).
const SHEET_WRITTEN: &[&str] = &["hyperlink"];
/// Attributes of `<fileVersion>` (CT_FileVersion).
const FILE_VERSION_ATTRS: &[&str] = &["appName", "lastEdited", "lowestEdited", "rupBuild", "codeName"];
/// The workbook extension listing the calculation features the saved values were computed with.
const CALC_FEATURES_URI: &str = "{B58B0392-4F1F-4190-BB64-5DF3571DCE5F}";
const NS_CALC_FEATURES: &str = "http://schemas.microsoft.com/office/spreadsheetml/2018/calcfeatures";
/// Kept sheet elements that go right after `<legacyDrawing>`, in schema order (before
/// `<tableParts>`).
const SHEET_SLOTS: &[&str] = &["legacyDrawingHF", "picture", "oleObjects", "controls"];
/// The kept sheet element that goes after `<colBreaks>` (schema order: before `<drawing>`).
const SLOT_CUSTOM_PROPERTIES: &str = "customProperties";
/// The kept sheet element that goes after `<autoFilter>` (schema order: before `<mergeCells>`):
/// the settings of the workbook's custom views (View → Custom Views) for this sheet, with the
/// printer settings of each.
const SLOT_CUSTOM_SHEET_VIEWS: &str = "customSheetViews";
/// The kept workbook element that lists the custom views; it goes after `<calcPr>`.
pub(crate) const SLOT_CUSTOM_WORKBOOK_VIEWS: &str = "customWorkbookViews";
/// The workbook relationship kinds of the cell and value metadata and of the rich-value parts
/// behind it. GridCraft rebuilds them for its cell pictures; what it does not model is kept as one
/// set ([`PreservedMetadata`]).
const METADATA_KINDS: &[&str] = &[
    "sheetMetadata",
    "rdRichValue",
    "rdRichValueStructure",
    "rdRichValueTypes",
    "rdRichValueWebImage",
    "rdArray",
    "rdSupportingPropertyBag",
    "rdSupportingPropertyBagStructure",
    "richValueRel",
];
/// The rich-value types part: global flags of the rich-value keys, not tied to any value.
const KIND_RICH_TYPES: &str = "rdRichValueTypes";
/// What GridCraft writes into the core properties part (`docProps/core.xml`) from the document
/// properties it models. The other elements of the part (`cp:revision`, `cp:category`,
/// `dc:language`, `cp:lastPrinted`…) are kept as XML, in the slot of the same name as the
/// relationship kind.
pub(crate) const SLOT_CORE_PROPERTIES: &str = "core-properties";
const CORE_WRITTEN: &[&str] = &["title", "subject", "creator", "keywords", "description", "lastModifiedBy", "created", "modified"];
/// The extended (application) properties part (`docProps/app.xml`): GridCraft writes the
/// application and the company. Not kept are what describes the file as it was saved rather than
/// the one written: the application's version, the lists of sheets Excel builds from the sheets
/// (`HeadingPairs`, `TitlesOfParts`), the security flags and the digital-signature marker. The
/// manager, the hyperlink base, the template… are kept.
pub(crate) const SLOT_EXTENDED_PROPERTIES: &str = "extended-properties";
const EXTENDED_WRITTEN: &[&str] = &[
    "Application",
    "AppVersion",
    "Company",
    "HeadingPairs",
    "TitlesOfParts",
    "DocSecurity",
    "ScaleCrop",
    "LinksUpToDate",
    "SharedDoc",
    "HyperlinksChanged",
    "DigSig",
];
/// The default namespace of the extended properties part GridCraft writes (the core properties
/// part has none: its root is `cp:coreProperties`).
pub(crate) const NS_EXTENDED_PROPERTIES: &str = "http://schemas.openxmlformats.org/officeDocument/2006/extended-properties";
/// The namespace prefixes the writer declares on the roots of the two parts.
const PROPERTY_NAMESPACES: &[(&str, &str)] = &[
    ("cp", "http://schemas.openxmlformats.org/package/2006/metadata/core-properties"),
    ("dc", "http://purl.org/dc/elements/1.1/"),
    ("dcterms", "http://purl.org/dc/terms/"),
    ("dcmitype", "http://purl.org/dc/dcmitype/"),
    ("xsi", "http://www.w3.org/2001/XMLSchema-instance"),
    ("vt", "http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes"),
];
/// Most elements kept from one document properties part.
const MAX_PROPERTY_ELEMENTS: usize = 256;
/// Every sheet element kept as XML.
fn is_sheet_slot(slot: &str) -> bool {
    SHEET_SLOTS.contains(&slot) || slot == SLOT_CUSTOM_PROPERTIES || slot == SLOT_CUSTOM_SHEET_VIEWS
}

/// Relationship kinds GridCraft reads and rewrites, or that depend on content it rewrites:
/// kept content whose graph reaches one of them is dropped.
const REWRITTEN: &[&str] = &[
    "officedocument",
    "core-properties",
    "extended-properties",
    "worksheet",
    "chartsheet",
    "dialogsheet",
    "xlmacrosheet",
    "xlintlmacrosheet",
    "drawing",
    "chart",
    "chartex",
    "chartstyle",
    "chartcolorstyle",
    "chartusershapes",
    "table",
    "pivottable",
    "pivotcachedefinition",
    "pivotcacherecords",
    "comments",
    "vmldrawing",
    "threadedcomment",
    "person",
    "sharedstrings",
    "styles",
    "theme",
    "sheetmetadata",
    "calcchain",
    // The Python part ([MS-XLSX] 2.1.23) is written from the Python cells' code on every save. The
    // one the file came with is not kept beside it: a workbook names one, and the reader takes the
    // first.
    "python",
    "vbaproject",
    "rdrichvalue",
    "rdrichvaluestructure",
    "rdrichvaluetypes",
    "rdrichvaluewebimage",
    "rdarray",
    "rdsupportingpropertybag",
    "rdsupportingpropertybagstructure",
    "richvaluerel",
    "slicer",
    "slicercache",
    "timeline",
    "timelinecache",
];

/// Relationship kinds a kept sheet (chart, dialog or macro sheet) reaches that are otherwise
/// rewritten: its drawing, its chart and their style, colour and shape parts, its legacy VML
/// drawing (a dialog sheet's controls) and its notes are kept byte for byte, because the sheet
/// itself is not modelled and no worksheet shares them.
const KEPT_SHEET_GRAPH: &[&str] = &[
    "chartsheet",
    "dialogsheet",
    "xlMacrosheet",
    "xlIntlMacrosheet",
    "drawing",
    "vmlDrawing",
    "comments",
    "chart",
    "chartex",
    "chartstyle",
    "chartcolorstyle",
    "chartusershapes",
];

/// Part-name prefixes (lower case) GridCraft's writer never produces: kept parts there keep their
/// original names. Anything else is renamed so it can't collide with written parts.
const OWN_DIRS: &[&str] = &[
    "customxml/",
    "docprops/custom.xml",
    "docprops/thumbnail.",
    "docmetadata/",
    "customui/",
    "xl/externallinks/",
    "xl/connections.xml",
    "xl/xmlmaps.xml",
    "xl/chartsheets/",
    "xl/dialogsheets/",
    "xl/macrosheets/",
    "xl/intlmacrosheets/",
    "xl/querytables/",
    "xl/ctrlprops/",
    "xl/embeddings/",
    "xl/activex/",
    "xl/webextensions/",
    "xl/printersettings/",
    "xl/revisions/",
    "xl/volatiledependencies.",
    "xl/wssortmap",
    "xl/namedsheetviews/",
    "xl/customproperty",
    "xl/attachedtoolbars.",
    "xl/model/",
    "xl/metadata.xml",
    "xl/richdata/",
];

fn kind(rel_type: &str) -> &str {
    rel_type.rsplit('/').next().unwrap_or("")
}

/// What kind of sheet a workbook relationship (its type's last segment) leads to, when GridCraft
/// keeps that sheet whole instead of modelling it.
pub(crate) fn sheet_kind(rel_kind: &str) -> Option<PreservedSheetKind> {
    match rel_kind.to_ascii_lowercase().as_str() {
        "chartsheet" => Some(PreservedSheetKind::Chart),
        "dialogsheet" => Some(PreservedSheetKind::Dialog),
        "xlmacrosheet" | "xlintlmacrosheet" => Some(PreservedSheetKind::Macro),
        _ => None,
    }
}

/// What a kept sheet is called in a message.
fn sheet_label(kind: PreservedSheetKind) -> &'static str {
    match kind {
        PreservedSheetKind::Chart => "chart sheet",
        PreservedSheetKind::Dialog => "dialog sheet",
        PreservedSheetKind::Macro => "macro sheet",
        PreservedSheetKind::Module => "module sheet",
    }
}

/// Whether a workbook's `<sheet>` entry is a VBA module sheet's: Excel writes those with an empty
/// `r:id`, since a module has no sheet part ([`PreservedSheetKind::Module`]).
pub(crate) fn is_module_entry(se: &xml::El) -> bool {
    se.attr("id") == Some("")
}

fn is_kind(r: &PreservedRel, kinds: &[&str]) -> bool {
    let k = kind(&r.rel_type);
    kinds.iter().any(|x| x.eq_ignore_ascii_case(k))
}

fn rewritten(r: &PreservedRel) -> bool {
    !r.external && REWRITTEN.contains(&kind(&r.rel_type).to_ascii_lowercase().as_str())
}

/// Who owns a relationship.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Owner {
    Package,
    Workbook,
    Sheet,
}

/// Whether `r`, a relationship of `owner`, is kept for its own sake: GridCraft doesn't rebuild what
/// it targets ([`REWRITTEN`]) and no element of the owner's XML decides what happens to it. This
/// is the deny-list: a kind nobody has heard of is kept too.
fn is_root(owner: Owner, r: &PreservedRel) -> bool {
    let k = kind(&r.rel_type).to_ascii_lowercase();
    let listed = |list: &[&str]| list.iter().any(|x| x.eq_ignore_ascii_case(&k));
    !REWRITTEN.contains(&k.as_str())
        && match owner {
            Owner::Package => true,
            Owner::Workbook => !listed(WORKBOOK_ELEMENT_KINDS),
            Owner::Sheet => !listed(SHEET_ELEMENT_KINDS) && !listed(SHEET_WRITTEN),
        }
}

fn rels_part_name(part: &str) -> String {
    let (dir, file) = package::split_dir(part);
    if dir.is_empty() { format!("_rels/{file}.rels") } else { format!("{dir}/_rels/{file}.rels") }
}
