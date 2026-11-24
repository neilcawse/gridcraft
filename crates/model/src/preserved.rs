//! File content GridCraft does not model, carried through unchanged so that saving a workbook
//! never silently drops it (XLSX pass-through: custom XML, external links, connections, query
//! tables, form controls, OLE objects…).
//!
//! The data is opaque to everything except the file-format crate: the model only holds it, so a
//! sheet's kept content disappears with the sheet. It is not part of the JSON format.

use std::sync::Arc;

use gridcraft_core::{CellRef, Value};

use crate::sheet::Visibility;

/// A relationship from an owning part (the package root, the workbook or a sheet, or a kept part)
/// to a kept part or to an external target.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreservedRel {
    /// The id in the source file (`rId7`). Ids of an owner GridCraft writes are reassigned on save.
    pub id: String,
    /// The full relationship type URI.
    pub rel_type: String,
    /// The original part name of the target (no leading `/`), or the raw target of an external
    /// relationship. External targets are never opened.
    pub target: String,
    pub external: bool,
}

/// A kept package part.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreservedPart {
    /// Part name in the source file (no leading `/`).
    pub name: String,
    pub content_type: String,
    pub data: Arc<[u8]>,
    /// The part's own relationships, ids unchanged (the part's bytes refer to them).
    pub rels: Vec<PreservedRel>,
}

/// A raw XML element of an owning part that refers to kept parts, re-emitted on save with its
/// relationship ids rewritten, or that Excel needs back unchanged (the workbook's `fileVersion`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreservedXml {
    /// Which slot of the owner it belongs in (`externalReferences`, `fileVersion`, `ext`,
    /// `oleObjects`, `controls`).
    pub slot: String,
    /// The element, namespace declarations it needs included.
    pub xml: String,
}

/// What a kept sheet is. GridCraft models none of them: each is written back as it was read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PreservedSheetKind {
    /// A chart sheet: one chart and no cells.
    #[default]
    Chart,
    /// An Excel 5.0 dialog sheet: a form drawn from legacy VML controls.
    Dialog,
    /// An Excel 4.0 (XLM) macro sheet, or an international one. Only macro-enabled workbooks
    /// (`.xlsm`, `.xltm`) can hold them.
    Macro,
    /// A `<sheet>` entry with an empty `r:id` and no part: what Excel (2013, 2016) writes for an
    /// Excel 5.0 VBA module sheet (`Module1`, usually very hidden). Its code, if any, is in the
    /// VBA project; the entry holds the module's name, position, state and `sheetId`.
    Module,
}

/// A sheet GridCraft does not model (see [`PreservedSheetKind`]) kept as it was in the source file.
/// Its part and the drawing, chart, VML, style and colour parts behind it are in
/// [`Preserved::parts`]; the workbook relationship to it is in [`Preserved::rels`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreservedSheet {
    pub name: String,
    pub kind: PreservedSheetKind,
    pub visibility: Visibility,
    /// The id of the workbook relationship to the sheet part, in the source file.
    pub rel_id: String,
    /// How many worksheets come before it: its position among the model's sheets.
    pub before: usize,
    /// Its `sheetId` in the source file.
    pub sheet_id: u32,
    /// The indexes a macro sheet's XML holds into tables of the workbook that are written from
    /// the model (shared strings, cell and differential formats), by where they are in its part:
    /// they are given anew on save. In the order they appear in the part.
    pub indexes: Vec<PreservedIndex>,
    /// The defined names scoped to the sheet; they are written with its position when it is
    /// written.
    pub names: Vec<PreservedName>,
}

/// A defined name scoped to a kept sheet ([`PreservedSheet::names`]): its `<definedName>` as the
/// file had it, but for the formula, which edits that move or rename the cells and sheets it
/// names rewrite as they do the workbook's own names.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreservedName {
    /// The attributes of the element other than `localSheetId` (the sheet's position when it is
    /// written), as they are written: ` name="Rate" hidden="1"`.
    pub attrs: String,
    /// The formula, as the file spells it (the text of the element, not escaped).
    pub formula: String,
}

/// An index a kept sheet's XML holds into a table of the workbook GridCraft writes itself: the
/// byte range of the attribute value or element text in the kept part, and what it refers to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreservedIndex {
    pub at: std::ops::Range<usize>,
    pub to: PreservedIndexed,
}

/// What a [`PreservedIndex`] refers to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreservedIndexed {
    /// A shared string (the `<v>` of a `t="s"` cell): its text, and the reading it names
    /// (`Sheet::phonetic_cells`: `None` for its text's first one).
    String { text: Arc<str>, reading: Option<Option<u32>> },
    /// A cell format (`s` of a cell or row, `style` of a column): the model style it was read as.
    Style(crate::StyleId),
    /// A differential format (`dxfId` of a conditional-formatting rule…).
    Dxf(crate::Style),
    /// A cell's `vm` or `cm` attribute, whole: an index into the workbook's kept cell and value
    /// metadata ([`PreservedMetadata`]), written as it was while that is written and left out
    /// otherwise.
    Metadata,
}

/// A column of a table as the source file numbered it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreservedTableColumn {
    /// The header text.
    pub name: String,
    /// The `id` of the `<tableColumn>`: the fields of a query table name their column by it.
    pub id: u32,
    /// `queryTableFieldId`: the field of the query table the column shows.
    pub query_field_id: Option<u32>,
}

/// What ties a table to the parts that hang off its table part (the query table behind an
/// external data range). GridCraft writes the table itself from the model, so the part's own
/// relationships are kept here, with the parts behind them, together with the file's numbering of
/// the table's columns that those parts refer to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreservedTable {
    /// The table's `id` in the model (`Table::id`), which is not the `id` its file gave it: the
    /// model numbers its tables itself.
    pub id: u32,
    /// `tableType` of the table element (`queryTable`).
    pub table_type: Option<String>,
    /// `connectionId` of the table element.
    pub connection_id: Option<u32>,
    /// The columns in file order.
    pub columns: Vec<PreservedTableColumn>,
    /// The table part's relationships to kept parts, ids unchanged.
    pub rels: Vec<PreservedRel>,
    /// Every kept part reachable from `rels`.
    pub parts: Vec<PreservedPart>,
}

/// The workbook's theme part as the source file had it. GridCraft writes a theme from the colours
/// and fonts of [`crate::Theme`] only, so the rest of the part (the format scheme with its picture
/// fills, object defaults, extensions) is kept here, with the parts behind the part's
/// relationships. It is written back as it was while the model's theme is still the one the
/// part describes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreservedTheme {
    /// The theme part, byte for byte.
    pub data: Arc<[u8]>,
    /// The part's relationships to kept parts (the images of its fill styles), ids unchanged.
    pub rels: Vec<PreservedRel>,
    /// Every kept part reachable from `rels`.
    pub parts: Vec<PreservedPart>,
}

/// The workbook's cell and value metadata (`xl/metadata.xml`) with the rich-value parts behind it
/// (`xl/richData/*`), as the source file had them. GridCraft models only cell pictures and the
/// dynamic-array flag, and writes its own metadata for those. Value metadata it does not model
/// (rich errors, linked data types, cube members) is kept here, to be written back with the `vm`
/// of the cells in [`Preserved::cell_meta`] that are still as they were.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreservedMetadata {
    /// The workbook's relationships to the metadata part and to the rich-value parts, ids unchanged.
    pub rels: Vec<PreservedRel>,
    /// Every kept part reachable from `rels`.
    pub parts: Vec<PreservedPart>,
    /// Whether the cells carry value metadata that is not a picture. Only then are all the parts
    /// kept: without it, `rels` holds the rich-value types part alone, which GridCraft writes next
    /// to the rich values of its own cell pictures.
    pub values: bool,
    /// Whether entry 1 of the cell metadata is the dynamic-array flag. GridCraft marks its
    /// dynamic-array formulas `cm="1"`: the kept part can only be written back next to them when
    /// that entry is there.
    pub dynamic_arrays: bool,
}

/// A cell whose value carries value metadata (`vm`) that GridCraft does not model, with the cell
/// as it was read. The metadata describes that value: it is written back only while the cell
/// still holds it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PreservedCellMeta {
    pub cell: CellRef,
    /// The one-based `vm` of the source file, an index into the kept metadata's value metadata.
    pub vm: u32,
    /// The cell's value as read (for a formula, its cached result).
    pub value: Value,
    /// The cell's formula as read.
    pub formula: Option<String>,
}

/// A picture that fills the box of a note: the `<v:fill type="frame" o:relid>` of its shape in the
/// sheet's legacy VML drawing. GridCraft writes the shapes of the notes itself, with the plain
/// fill; the picture goes back into the shape of the note at the same cell.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreservedNoteFill {
    /// The note's cell, as the file had it (`<x:Row>` and `<x:Column>` of its shape).
    pub cell: CellRef,
    /// The `<v:fill>` element. Its `o:relid` names a relationship of the drawing, in
    /// [`PreservedNoteFills::rels`].
    pub xml: String,
}

/// The pictures that fill the note boxes of a sheet, with the images they show. The drawing is
/// GridCraft's own, so its relationships are kept here: a picture is written back only while the
/// note it fills is there.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreservedNoteFills {
    pub fills: Vec<PreservedNoteFill>,
    /// The relationships of the source drawing that the fills name, ids unchanged.
    pub rels: Vec<PreservedRel>,
    /// The parts behind `rels` that the sheet's other kept content doesn't hold already.
    pub parts: Vec<PreservedPart>,
}

/// Unmodelled content kept for one owner (the workbook, which also holds package-root content,
/// or one sheet).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Preserved {
    /// Relationships of the package root (`_rels/.rels`) to kept parts (workbook owner only).
    pub package_rels: Vec<PreservedRel>,
    /// Relationships of the owning workbook or sheet part to kept parts.
    pub rels: Vec<PreservedRel>,
    /// Every kept part reachable from `package_rels` and `rels`.
    pub parts: Vec<PreservedPart>,
    /// Owner XML elements that refer to kept parts.
    pub elements: Vec<PreservedXml>,
    /// The sheet's legacy VML drawing when kept elements need its shapes (form controls, OLE
    /// objects). Its own relationships point into `parts`.
    pub vml: Option<PreservedPart>,
    /// Kept sheets (chart, dialog, macro), in file order (workbook owner only).
    pub sheets: Vec<PreservedSheet>,
    /// Tables with kept relationships of their own (sheet owner only).
    pub tables: Vec<PreservedTable>,
    /// The theme part with what hangs off it (workbook owner only).
    pub theme: Option<PreservedTheme>,
    /// The cell and value metadata with the rich-value parts (workbook owner only).
    pub metadata: Option<PreservedMetadata>,
    /// Cells whose value metadata is in `metadata` (sheet owner only).
    pub cell_meta: Vec<PreservedCellMeta>,
    /// The pictures that fill note boxes (sheet owner only).
    pub note_fills: Option<PreservedNoteFills>,
}

impl Preserved {
    pub fn is_empty(&self) -> bool {
        self.package_rels.is_empty()
            && self.rels.is_empty()
            && self.parts.is_empty()
            && self.elements.is_empty()
            && self.vml.is_none()
            && self.sheets.is_empty()
            && self.tables.is_empty()
            && self.theme.is_none()
            && self.metadata.is_none()
            && self.cell_meta.is_empty()
            && self.note_fills.is_none()
    }
    /// Total bytes held.
    pub fn byte_len(&self) -> usize {
        let tables = self.tables.iter().flat_map(|t| &t.parts);
        let theme = self.theme.iter().flat_map(|t| &t.parts);
        let metadata = self.metadata.iter().flat_map(|m| &m.parts);
        let fills = self.note_fills.iter().flat_map(|n| &n.parts);
        let held = self.parts.iter().chain(self.vml.iter()).chain(tables).chain(theme).chain(metadata).chain(fills);
        let theme_bytes = self.theme.as_ref().map_or(0, |t| t.data.len());
        held.fold(theme_bytes, |n, p| n.saturating_add(p.data.len()))
    }
    /// Whether an Excel 4.0 macro sheet is kept: such a workbook has to be written as a
    /// macro-enabled one.
    pub fn has_macro_sheets(&self) -> bool {
        self.sheets.iter().any(|s| s.kind == PreservedSheetKind::Macro)
    }
}
