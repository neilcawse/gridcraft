//! What cells carry for saving alone, and move with them.
//!
//! A cell's reading ([`Sheet::phonetic_cells`]: furigana that isn't its text's first), the value
//! metadata kept from the file for it ([`crate::Preserved::cell_meta`]: a rich `#CALC!`, a linked
//! data type) and the picture that fills the box of its note ([`crate::Preserved::note_fills`]) are
//! keyed by the cell's address, and no edit reads them: only the writer does. An edit that moves
//! cells moves them too, or the writer finds them at the old address (another cell's, or none)
//! and writes the reading on the wrong cell, or leaves a `#CALC!` to reopen as `#VALUE!`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use gridcraft_core::{CellRef, RangeRef};

use crate::preserved::PreservedCellMeta;
use crate::sheet::Sheet;

/// The readings and kept value metadata of cells cut out of a sheet, from
/// [`Sheet::take_cell_sidecars`] for [`Sheet::put_cell_sidecars`].
#[derive(Debug, Default)]
pub struct CellSidecars {
    readings: Vec<(CellRef, Option<u32>)>,
    meta: Vec<PreservedCellMeta>,
}

impl Sheet {
    /// Moves what cells carry for saving alone: their readings, the value metadata kept for them
    /// and the pictures that fill the boxes of their notes. Each goes where `to` sends its cell
    /// (`None`, or off the sheet: it goes); one sent onto a cell whose own stays takes its place.
    /// For an edit that moves cells within the sheet: rows and columns put in or taken out, a
    /// sort, a column move.
    pub fn move_cell_sidecars(&mut self, to: impl Fn(CellRef) -> Option<CellRef>) {
        if !self.phonetic_cells.is_empty() {
            let readings = remap(std::mem::take(&mut self.phonetic_cells).into_iter().collect(), |r| &mut r.0, &to);
            self.phonetic_cells = readings.into_iter().collect();
        }
        let Some(p) = self.preserved.as_mut() else { return };
        if !p.cell_meta.is_empty() {
            let p = Arc::make_mut(p);
            p.cell_meta = remap(std::mem::take(&mut p.cell_meta), |m| &mut m.cell, &to);
        }
        self.move_note_fills(to);
    }

    /// [`Sheet::move_cell_sidecars`] for the pictures that fill note boxes alone. They belong to
    /// the drawing of their sheet's notes (they name its relationships), so they can move within
    /// the sheet but not to another one.
    pub fn move_note_fills(&mut self, to: impl Fn(CellRef) -> Option<CellRef>) {
        let Some(p) = self.preserved.as_mut().filter(|p| p.note_fills.as_ref().is_some_and(|n| !n.fills.is_empty())) else { return };
        if let Some(n) = Arc::make_mut(p).note_fills.as_mut() {
            n.fills = remap(std::mem::take(&mut n.fills), |f| &mut f.cell, &to);
        }
    }

    /// Takes out the readings and the value metadata of the cells in `area`, for cells that are
    /// cut and pasted: [`Sheet::put_cell_sidecars`] puts them on the cells they land on, which may
    /// be on another sheet. (The pictures that fill note boxes can't leave their sheet:
    /// [`Sheet::move_note_fills`] moves them within it.)
    pub fn take_cell_sidecars(&mut self, area: RangeRef) -> CellSidecars {
        let mut out = CellSidecars::default();
        if self.phonetic_cells.keys().any(|c| area.contains(*c)) {
            let (taken, kept): (BTreeMap<_, _>, BTreeMap<_, _>) =
                std::mem::take(&mut self.phonetic_cells).into_iter().partition(|(c, _)| area.contains(*c));
            self.phonetic_cells = kept;
            out.readings = taken.into_iter().collect();
        }
        if let Some(p) = self.preserved.as_mut().filter(|p| p.cell_meta.iter().any(|m| area.contains(m.cell))) {
            let p = Arc::make_mut(p);
            let (taken, kept) = std::mem::take(&mut p.cell_meta).into_iter().partition(|m| area.contains(m.cell));
            p.cell_meta = kept;
            out.meta = taken;
        }
        out
    }

    /// Puts what [`Sheet::take_cell_sidecars`] took out on this sheet's cells, each where `to`
    /// sends its cell (`None`, or off the sheet: it goes), in place of what those cells had.
    pub fn put_cell_sidecars(&mut self, s: CellSidecars, to: impl Fn(CellRef) -> Option<CellRef>) {
        for (c, reading) in s.readings {
            if let Some(at) = to(c).filter(CellRef::is_valid) {
                self.phonetic_cells.insert(at, reading);
            }
        }
        let meta: Vec<PreservedCellMeta> =
            s.meta.into_iter().filter_map(|m| Some(PreservedCellMeta { cell: to(m.cell).filter(CellRef::is_valid)?, ..m })).collect();
        if meta.is_empty() {
            return;
        }
        let landed: HashSet<CellRef> = meta.iter().map(|m| m.cell).collect();
        let p = Arc::make_mut(self.preserved.get_or_insert_with(Default::default));
        p.cell_meta.retain(|m| !landed.contains(&m.cell));
        p.cell_meta.extend(meta);
    }
}

/// The formulas of cells that keep value metadata, which an edit rewrote while leaving them
/// reading the same cells (their references moved or were renamed). The metadata describes the
/// cell's value and is written back only while the cell still holds the formula and value it was
/// read with, so a rewritten formula has to be noted to carry the metadata along:
///
/// ```text
/// let mut rewrites = FormulaRewrites::of(sheet);
/// for each formula rewritten: rewrites.note(cell, old_text, new_text);
/// rewrites.apply(sheet);
/// ```
///
/// Only the cells that keep metadata are watched, so noting every rewrite of a large sheet costs
/// one set lookup each (none when nothing is kept).
#[derive(Debug, Default)]
pub struct FormulaRewrites {
    watched: HashSet<CellRef>,
    done: Vec<(CellRef, String, String)>,
}

impl FormulaRewrites {
    /// Watches the cells of `sheet` that keep value metadata.
    pub fn of(sheet: &Sheet) -> Self {
        let watched = sheet.preserved.as_deref().map(|p| p.cell_meta.iter().map(|m| m.cell).collect()).unwrap_or_default();
        FormulaRewrites { watched, done: vec![] }
    }

    /// Watches the cells of `moved` that have kept value metadata (by the cell they were taken
    /// from).
    pub fn of_moved(moved: &CellSidecars) -> Self {
        FormulaRewrites { watched: moved.meta.iter().map(|m| m.cell).collect(), done: vec![] }
    }

    /// The formula of the cell `at` was rewritten from the text `from` to `to`.
    pub fn note(&mut self, at: CellRef, from: &str, to: &str) {
        if from != to && self.watched.contains(&at) {
            self.done.push((at, from.to_string(), to.to_string()));
        }
    }

    /// Records the rewritten formulas with the metadata kept on `sheet` for those cells.
    pub fn apply(self, sheet: &mut Sheet) {
        if self.done.is_empty() {
            return;
        }
        if let Some(p) = sheet.preserved.as_mut() {
            self.apply_to(&mut Arc::make_mut(p).cell_meta);
        }
    }

    /// [`FormulaRewrites::apply`] for cells taken out of a sheet.
    pub fn apply_moved(self, moved: &mut CellSidecars) {
        self.apply_to(&mut moved.meta);
    }

    fn apply_to(self, meta: &mut [PreservedCellMeta]) {
        let at: HashMap<CellRef, usize> = meta.iter().enumerate().map(|(i, m)| (m.cell, i)).collect();
        for (cell, from, to) in self.done {
            if let Some(m) = at.get(&cell).and_then(|&i| meta.get_mut(i))
                && m.formula.as_deref() == Some(from.as_str())
            {
                m.formula = Some(to);
            }
        }
    }
}

/// `items` with the cell of each (`cell`) sent where `to` sends it; those sent nowhere, or off the
/// sheet, are left out, and one sent onto the cell of one that stays replaces it.
fn remap<T>(items: Vec<T>, cell: fn(&mut T) -> &mut CellRef, to: &impl Fn(CellRef) -> Option<CellRef>) -> Vec<T> {
    let (mut stay, mut moved) = (Vec::with_capacity(items.len()), vec![]);
    for mut item in items {
        let from = *cell(&mut item);
        match to(from).filter(CellRef::is_valid) {
            Some(at) if at == from => stay.push(item),
            Some(at) => {
                *cell(&mut item) = at;
                moved.push(item);
            }
            None => {}
        }
    }
    if !moved.is_empty() {
        let landed: HashSet<CellRef> = moved.iter_mut().map(|m| *cell(m)).collect();
        stay.retain_mut(|s| !landed.contains(cell(s)));
        stay.append(&mut moved);
    }
    stay
}
