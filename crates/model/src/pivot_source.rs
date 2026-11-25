//! What a PivotTable's source stands for. A source read from a file is bound to the exact defined
//! name or table it resolved to then ([`Workbook::bind_pivot_source`]); every later use (the
//! report the app shows, the cache a save rebuilds, the cache a save keeps) goes through
//! [`Workbook::pivot_source`], so they agree. A save writes the source so that reading the file
//! binds it to the same definition again ([`Workbook::saved_source`]). Once the definition is
//! gone only one of the same kind and scope may take its place, and the save says so.

use gridcraft_core::{CellRef, RangeRef, quote_sheet, split_sheet};

use crate::{Binding, BindingKind, DefinedName, DefinitionUid, PivotTable, SheetUid, SourceBinding, Workbook};

/// What a PivotTable's source stands for.
#[derive(Clone, Copy, Debug)]
pub enum PivotSource<'a> {
    /// No binding: the source text, resolved as before (a range, a table, a defined name).
    Text(&'a str),
    /// A defined name, its reference looked up from the `from`th sheet (its own for a sheet's
    /// name, the PivotTable's for a workbook name).
    Name { name: &'a DefinedName, from: usize },
    /// The `table`th table of the `sheet`th sheet.
    Table { sheet: usize, table: usize },
}

/// A PivotTable's source as found ([`Workbook::pivot_source`]).
#[derive(Clone, Debug)]
pub struct FoundSource<'a> {
    pub source: PivotSource<'a>,
    /// When the definition the source was bound to is gone and `source` is the one of the same
    /// kind and scope that took its place: (the old one, the new one), described for a warning.
    pub rebound: Option<(String, String)>,
}

impl FoundSource<'_> {
    fn same(source: PivotSource<'_>) -> FoundSource<'_> {
        FoundSource { source, rebound: None }
    }
}

/// How a save writes a PivotTable's source ([`Workbook::saved_source`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SavedSource {
    /// `worksheetSource name` (with `sheet`): read back, it binds to the same definition.
    /// `original`: it is the source text as it is (so the attributes it was read from, while it
    /// is still the text read, are written back as they were).
    Name { name: String, sheet: Option<String>, original: bool },
    /// No name form binds to the definition when read back (another definition of the same name
    /// hides it): written as the definition's range, which the save says.
    Range,
}

impl Workbook {
    /// What `source`, as read from a file for a PivotTable on the `host`th sheet, is bound to,
    /// once the workbook's names and tables are read. A bare name is the table or defined name a
    /// formula on that sheet would find. A name with a sheet (`Data!rData`, from `worksheetSource
    /// name="rData" sheet="Data"`) is, of these, the first there is: the name of that sheet, a
    /// workbook name referring to cells of that sheet, a table on that sheet. A range, a source in
    /// another workbook or one that finds nothing is not bound.
    pub fn bind_pivot_source(&self, source: &str, host: usize) -> SourceBinding {
        let t = source.trim();
        if t.is_empty() || t.starts_with('[') {
            return SourceBinding::default();
        }
        let bind = |kind, def, was| SourceBinding(Some(Binding { text: source.to_string(), kind, def, was, read_as: None }), None);
        if let Some((si, ti)) = self.table(t) {
            let tb = &self.sheets[si].tables[ti];
            return bind(BindingKind::Table, tb.uid, table_text(&tb.name));
        }
        let (sheet, body) = split_sheet(t);
        if RangeRef::parse(body).is_some() || CellRef::parse(body).is_some() {
            return SourceBinding::default();
        }
        let Some(sheet) = sheet else {
            return self.name(t, host).map_or_else(SourceBinding::default, |n| bind(BindingKind::Name, n.uid, self.name_text(n)));
        };
        let Some(at) = self.sheet_index(&sheet) else { return SourceBinding::default() };
        let uid = self.sheets[at].uid;
        if let Some(n) = self.name_in_scope(body, Some(at)) {
            bind(BindingKind::SheetName(uid), n.uid, self.name_text(n))
        } else if let Some(n) = self.workbook_name_on(body, at) {
            bind(BindingKind::WorkbookName(uid), n.uid, self.name_text(n))
        } else if let Some(tb) = self.sheets[at].tables.iter().find(|tb| tb.name.eq_ignore_ascii_case(body)) {
            bind(BindingKind::SheetTable(uid), tb.uid, table_text(&tb.name))
        } else {
            SourceBinding::default()
        }
    }

    /// What the source of `pt`, a PivotTable on the `host`th sheet, stands for: the definition it
    /// is bound to while that is in the workbook (however it was edited since), else the one of
    /// the same kind and scope that took its place (a name of the same sheet made again, the name
    /// a bare one now finds), else nothing: an error saying why. A source not bound (or changed
    /// since it was) is its text.
    pub fn pivot_source<'a>(&'a self, pt: &'a PivotTable, host: usize) -> Result<FoundSource<'a>, String> {
        let Some(b) = pt.binding.0.as_ref().filter(|b| b.text == pt.source) else {
            return Ok(FoundSource::same(PivotSource::Text(&pt.source)));
        };
        let gone = || format!("its source, {}, was deleted, and nothing of the same kind takes its place", b.was);
        let rebound = |source, now: String| Ok(FoundSource { source, rebound: Some((b.was.clone(), now)) });
        let text = pt.source.trim();
        let (named, body) = split_sheet(text);
        // The sheet a source naming one was bound to, while the source names it (a sheet rename
        // rewrites both): a stale name would save a cache naming a sheet that isn't there.
        let sheet_of = |uid: SheetUid| {
            let at = self.sheets.iter().position(|s| s.uid.get() == uid.get()).ok_or_else(gone)?;
            match named.as_deref() {
                Some(n) if self.sheet_index(n) == Some(at) => Ok(at),
                n => Err(format!("its source names sheet \"{}\", not the sheet \"{}\" it was on", n.unwrap_or(""), self.sheets[at].name)),
            }
        };
        let name_by_uid = || self.names.iter().find(|n| n.uid.get() == b.def.get());
        let table_by_uid = || self.table_by_uid(b.def);
        // The definition itself, however its reference, scope or sheet changed since.
        if let Some(name) = name_by_uid() {
            return Ok(FoundSource::same(PivotSource::Name { name, from: name.scope.unwrap_or(host) }));
        }
        if let Some((sheet, table)) = table_by_uid() {
            return Ok(FoundSource::same(PivotSource::Table { sheet, table }));
        }
        match b.kind {
            BindingKind::Name => {
                let name = self.name(text, host).ok_or_else(gone)?;
                rebound(PivotSource::Name { name, from: name.scope.unwrap_or(host) }, self.name_text(name))
            }
            BindingKind::Table => {
                let (sheet, table) = self.table(text).ok_or_else(gone)?;
                rebound(PivotSource::Table { sheet, table }, table_text(&self.sheets[sheet].tables[table].name))
            }
            BindingKind::SheetName(uid) => {
                let at = sheet_of(uid)?;
                let name = self.name_in_scope(body, Some(at)).ok_or_else(gone)?;
                rebound(PivotSource::Name { name, from: at }, self.name_text(name))
            }
            BindingKind::WorkbookName(uid) => {
                let at = sheet_of(uid)?;
                let name = self.workbook_name_on(body, at).ok_or_else(gone)?;
                rebound(PivotSource::Name { name, from: host }, self.name_text(name))
            }
            BindingKind::SheetTable(uid) => {
                let at = sheet_of(uid)?;
                let table = self.sheets[at].tables.iter().position(|tb| tb.name.eq_ignore_ascii_case(body)).ok_or_else(gone)?;
                rebound(PivotSource::Table { sheet: at, table }, table_text(&self.sheets[at].tables[table].name))
            }
        }
    }

    /// Chooses the first name form that native import binds to the intended definition and that
    /// the caller has checked still resolves to the intended data. `self` is the emitted catalog.
    pub fn saved_source(
        &self,
        source: &str,
        host: usize,
        def: DefinitionUid,
        source_sheet: Option<usize>,
        same_data: impl Fn(&Binding) -> bool,
    ) -> SavedSource {
        let (bare, on) = if let Some(name) = self.names.iter().find(|n| n.uid.get() == def.get()) {
            (&name.name, name.scope.or(source_sheet))
        } else if let Some((si, ti)) = self.table_by_uid(def) {
            (&self.sheets[si].tables[ti].name, Some(si))
        } else {
            return SavedSource::Range;
        };
        let original = source.trim();
        let qualified = on.and_then(|s| self.sheet(s)).map(|s| format!("{}!{bare}", quote_sheet(&s.name)));
        [Some(original), Some(bare.as_str()), qualified.as_deref()]
            .into_iter()
            .flatten()
            .find(|t| self.bind_pivot_source(t, host).0.is_some_and(|b| b.def.get() == def.get() && same_data(&b)))
            .map_or(SavedSource::Range, |t| {
                let (sheet, name) = split_sheet(t);
                SavedSource::Name { name: name.to_string(), sheet, original: t == original }
            })
    }

    /// The (sheet, table) index of the table whose identity is `uid`.
    fn table_by_uid(&self, uid: DefinitionUid) -> Option<(usize, usize)> {
        self.sheets.iter().enumerate().find_map(|(si, s)| s.tables.iter().position(|t| t.uid.get() == uid.get()).map(|ti| (si, ti)))
    }

    /// The workbook name `name` when it refers to cells of the `at`th sheet (`Data!$A$1:$B$3`).
    fn workbook_name_on(&self, name: &str, at: usize) -> Option<&DefinedName> {
        self.name_in_scope(name, None).filter(|n| {
            let (sheet, body) = split_sheet(n.formula.trim().trim_start_matches('='));
            sheet.is_some_and(|s| self.sheet_index(&s) == Some(at)) && (RangeRef::parse(body).is_some() || CellRef::parse(body).is_some())
        })
    }

    /// `sheet "Data"'s name "rData"`, `the workbook's name "rData"`.
    fn name_text(&self, n: &DefinedName) -> String {
        match n.scope.and_then(|i| self.sheets.get(i)) {
            Some(s) => format!("sheet \"{}\"'s name \"{}\"", s.name, n.name),
            None => format!("the workbook's name \"{}\"", n.name),
        }
    }

    /// After the defined name or table whose identity is `uid` was renamed: the source of every
    /// PivotTable bound to it names it by its new name (with the sheet it named, if any), still
    /// bound to it, as formulas that use it follow the rename. A kept PivotTable is then saved
    /// from the model, with a cache that names the definition as it is now.
    pub fn follow_renamed_definition(&mut self, uid: DefinitionUid) {
        let (new, was) = if let Some(n) = self.names.iter().find(|n| n.uid.get() == uid.get()) {
            (n.name.clone(), self.name_text(n))
        } else if let Some((si, ti)) = self.table_by_uid(uid) {
            let name = self.sheets[si].tables[ti].name.clone();
            let was = table_text(&name);
            (name, was)
        } else {
            return;
        };
        let bound = |pt: &PivotTable| pt.binding.0.as_ref().is_some_and(|b| b.text == pt.source && b.def.get() == uid.get());
        for si in 0..self.sheets.len() {
            if !self.sheets[si].pivots.iter().any(bound) {
                continue;
            }
            let sheet = std::sync::Arc::make_mut(&mut self.sheets[si]);
            for pt in sheet.pivots.iter_mut().filter(|pt| bound(pt)) {
                let source = match split_sheet(&pt.source).0 {
                    Some(s) => format!("{}!{new}", quote_sheet(&s)),
                    None => new.clone(),
                };
                if let Some(b) = pt.binding.0.as_mut() {
                    b.text.clone_from(&source);
                    b.was.clone_from(&was);
                }
                pt.source = source;
            }
        }
    }
}

fn table_text(name: &str) -> String {
    format!("table \"{name}\"")
}
