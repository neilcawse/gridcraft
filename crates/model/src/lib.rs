//! The GridCraft document model: workbooks, sheets, a copy-on-write cell store, interned
//! styles, defined names, tables, conditional formats, validation, comments, charts and print
//! settings. Pure data; calculation lives in `gridcraft-calc`.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod b64;
pub mod cell;
mod cellmap;
mod drawing_file;
pub mod features;
pub mod geometry;
pub mod group;
mod group_projection;
mod line_follow;
pub mod macros;
pub mod password;
mod pivot_source;
pub mod preserved;
pub mod python;
pub mod sheet;
mod sidecars;
pub mod store;
pub mod style;
pub mod table_refs;
pub mod workbook;

pub use cell::{Cell, CellPicture, Formula};
pub use drawing_file::{DrawingFile, DrawingMarker, DrawingPlacement};
pub use features::*;
pub use geometry::{Geometry, SavedSheetGeometry};
pub use group::{DrawingGroup, DrawingRef, GroupFile, GroupFileMember, MAX_GROUP_DEPTH, ObjectRect, rotates};
pub use group_projection::{ChildTransform, GroupProjection, group_child_box, group_quarter_turn, group_unturned_box};
pub use macros::*;
pub use password::PasswordHash;
pub use pivot_source::{FoundSource, PivotSource, SavedSource};
pub use preserved::{
    Preserved, PreservedCellMeta, PreservedChartShapes, PreservedControlProxies, PreservedIndex, PreservedIndexed, PreservedMetadata, PreservedName,
    PreservedNoteFill, PreservedNoteFills, PreservedObjectLink, PreservedObjectLinks, PreservedPart, PreservedPivot, PreservedRel, PreservedSheet,
    PreservedSheetKind, PreservedTable, PreservedTableColumn, PreservedTheme, PreservedXml,
};
pub use python::{PyCode, PyOutput};
pub use sheet::{DEFAULT_COL_WIDTH, DEFAULT_ROW_HEIGHT, LineIndex, Sheet, SheetUid, SheetViewMode, Visibility, points_to_chars};
pub use sidecars::{CellSidecars, FormulaRewrites};
pub use store::CellStore;
pub use style::{
    Alignment, BorderLine, BorderStyle, Borders, Color, Fill, Font, HAlign, NumFmt, PatternType, ReadingOrder, Style, StyleId, StyleTable, Theme,
    Underline, VAlign, VertAlign,
};
pub use workbook::{
    CalcMode, CalcSettings, DefinedName, DefinitionUid, DocProps, EQUATION_BUDGET_MESSAGE, ExternalBook, ExternalName, ExternalSheet,
    MAX_EQUATION_LAYOUT_BYTES, PhoneticProps, PhoneticRun, PhoneticString, ShowObjects, Workbook, WorkbookView,
};

/// Font used for new workbooks. Only a name: renderers substitute an available open font
/// (e.g. the metric-compatible Carlito) when it isn't installed.
pub const DEFAULT_FONT: &str = "Calibri";
pub const DEFAULT_FONT_SIZE: f32 = 11.0;
