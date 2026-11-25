//! Repairs the workbook-wide table namespace at the XLSX boundary.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use gridcraft_core::{CellError, quote_sheet, split_sheet, valid_table_name};
use gridcraft_formula::{Expr, StructRef, fold_name};
use gridcraft_model::table_refs::{self, TablePosition as Position, TableRefRewrite};
use gridcraft_model::{PivotSource, PivotTable, Workbook};

/// ECMA-376 Part 1 §18.5.1.2 makes a table's `displayName` unique amongst all other display
/// names and defined names in the workbook, whatever their scope; GridCraft's own Insert Table
/// and Rename Table refuse a name any defined name has. Sheet-scoped names, including those kept
/// with chart, dialog and macro sheets, therefore reserve their spelling as well.
pub(crate) fn repair(wb: &mut Cow<'_, Workbook>, extra_names: impl IntoIterator<Item = String>, warnings: &mut Vec<String>) {
    if wb.sheets.iter().all(|sheet| sheet.tables.is_empty()) {
        return;
    }
    let mut used: HashSet<String> = extra_names.into_iter().map(|n| fold_name(&n)).collect();
    used.extend(wb.names.iter().filter(|n| !n.name.is_empty()).map(|n| fold_name(&n.name)));
    if wb.sheets.iter().any(|s| s.print.print_area.is_some()) {
        used.insert(fold_name("_xlnm.Print_Area"));
    }
    if wb.sheets.iter().any(|s| s.print.title_cols.is_some() || s.print.title_rows.is_some()) {
        used.insert(fold_name("_xlnm.Print_Titles"));
    }
    let kept = wb.preserved.iter().flat_map(|p| &p.sheets).flat_map(|s| &s.names);
    used.extend(kept.filter_map(|n| crate::xml::parse(format!("<n{}/>", n.attrs).as_bytes()).ok()?.attr("name").map(fold_name)));
    let mut pending = Vec::new();
    for (si, sheet) in wb.sheets.iter().enumerate() {
        for (ti, t) in sheet.tables.iter().enumerate() {
            if !valid_table_name(&t.name) || !used.insert(fold_name(&t.name)) {
                pending.push(((si, ti), t.name.as_str()));
            }
        }
    }
    if pending.is_empty() {
        return;
    }
    let mut next = HashMap::new();
    let mut renamed = HashMap::new();
    for (at, old) in pending {
        let base: String = old
            .chars()
            .take(255)
            .enumerate()
            .map(
                |(i, c)| {
                    if c == '_' || (i == 0 && c == '\\') || c.is_alphabetic() || (i != 0 && (c == '.' || c.is_alphanumeric())) { c } else { '_' }
                },
            )
            .collect();
        let base = if valid_table_name(&base) { base } else { format!("Table_{base}") };
        let suffix = next.entry(fold_name(&base)).or_insert(1usize);
        loop {
            let tail = if *suffix == 1 { String::new() } else { format!("_{suffix}") };
            *suffix += 1;
            let mut name: String = base.chars().take(255 - tail.len()).collect();
            name.push_str(&tail);
            if valid_table_name(&name) && used.insert(fold_name(&name)) {
                // Names as the user typed them: `{:?}` would escape a table's leading `\`.
                warnings.push(format!("Table \"{old}\" on sheet \"{}\" was renamed to \"{name}\".", wb.sheets[at.0].name));
                renamed.insert(at, name);
                break;
            }
        }
    }
    // The native table lookup is ASCII-insensitive. Allocation above uses the stronger
    // Excel-style fold, but it must not change what an existing token originally meant.
    let mut first = HashMap::new();
    for (si, sheet) in wb.sheets.iter().enumerate() {
        for (ti, table) in sheet.tables.iter().enumerate() {
            first.entry(table.name.to_ascii_lowercase()).or_insert((si, ti));
        }
    }
    let repaired_names = renamed.values().map(|n| n.to_ascii_lowercase()).collect();
    // Lookups must see every original name, even when the first duplicate is renamed too.
    let original;
    let target = match wb {
        Cow::Borrowed(book) => {
            let original = *book;
            let mut repaired = original.clone();
            Rewrite { original, renamed, first, repaired_names }.apply(&mut repaired);
            *wb = Cow::Owned(repaired);
            return;
        }
        Cow::Owned(book) => {
            original = book.clone();
            book
        }
    };
    Rewrite { original: &original, renamed, first, repaired_names }.apply(target);
}

struct Rewrite<'a> {
    original: &'a Workbook,
    renamed: HashMap<Position, String>,
    first: HashMap<String, Position>,
    repaired_names: HashSet<String>,
}

impl TableRefRewrite for Rewrite<'_> {
    fn table(&self, name: &str) -> Option<Position> {
        if name.bytes().any(|b| b.is_ascii_uppercase()) { self.first.get(&name.to_ascii_lowercase()).copied() } else { self.first.get(name).copied() }
    }

    fn cell_owner(&self, sheet: usize, cell: gridcraft_core::CellRef) -> Option<Position> {
        let mut containing = self.original.sheets[sheet].tables.iter().enumerate().filter(|(_, t)| t.range.contains(cell));
        let (ti, table) = containing.next()?;
        // Headers aren't calculated-column cells. Overlap makes ownership ambiguous, so fall
        // back to the original workbook lookup rather than guessing which table owns the cell.
        (containing.next().is_none() && !table.header_range().is_some_and(|r| r.contains(cell))).then_some((sheet, ti))
    }

    fn renamed(&self, at: Position) -> Option<&str> {
        self.renamed.get(&at).map(String::as_str)
    }

    fn might_change(&self, bare: &str, owner: Option<Position>) -> bool {
        match self.resolve(bare, owner) {
            Some(at) => self.renamed.contains_key(&at),
            None => self.captures(bare),
        }
    }

    fn captures(&self, bare: &str) -> bool {
        if bare.bytes().any(|b| b.is_ascii_uppercase()) {
            self.repaired_names.contains(&bare.to_ascii_lowercase())
        } else {
            self.repaired_names.contains(bare)
        }
    }

    fn replacement(&self, n: &str, structured: bool, sheet: usize, owner: Option<Position>, env: &[String]) -> Option<Expr> {
        if n.starts_with('[') || structured && n.contains('!') {
            return None;
        }
        let (scope, bare, qualifier) =
            if !structured && let Some((s, bare)) = n.rsplit_once('!') { (self.original.sheet_index(s)?, bare, Some(s)) } else { (sheet, n, None) };
        if !structured && self.original.name(bare, scope).is_some() {
            return None;
        }
        // A workbook-scoped body's bare names use the caller's sheet. Do not commit an
        // ambiguous token to the table when a local definition can override it.
        if !structured && scope == usize::MAX && self.original.names.iter().any(|n| n.scope.is_some() && n.name.eq_ignore_ascii_case(bare)) {
            return None;
        }
        let Some(at) = self.resolve(bare, owner.filter(|_| qualifier.is_none())) else {
            // The reference stays the error it evaluates to rather than reaching a renamed table.
            return self.captures(bare).then_some(Expr::Error(if structured { CellError::Ref } else { CellError::Name }));
        };
        let name = self.renamed.get(&at)?.clone();
        Some(if structured {
            Expr::Name(name)
        } else {
            let new = qualifier.map_or_else(|| name.clone(), |s| format!("{s}!{name}"));
            let conflicts = env.iter().any(|local| gridcraft_formula::same_name(local, &new))
                || self.original.name(&name, scope).is_some()
                || scope == usize::MAX && self.original.names.iter().any(|n| gridcraft_formula::same_name(&n.name, &name));
            if conflicts { Expr::Struct(StructRef { table: name, specifiers: vec![], col_start: None, col_end: None }) } else { Expr::Name(new) }
        })
    }

    fn pivot(&self, p: &PivotTable, si: usize) -> Option<(String, Option<&str>)> {
        let (at, qualified) = match self.original.pivot_source(p, si).ok()?.source {
            PivotSource::Table { sheet, table } => ((sheet, table), split_sheet(&p.source).0.is_some()),
            PivotSource::Text(t) if !p.binding.unresolved(t) && !t.trim_start().starts_with('[') => {
                (self.table(t.trim().trim_start_matches('=').trim())?, false)
            }
            _ => return None,
        };
        let new = self.renamed(at)?;
        let source = if qualified { format!("{}!{new}", quote_sheet(&self.original.sheets[at.0].name)) } else { new.to_string() };
        Some((source, Some(new)))
    }
}

impl Rewrite<'_> {
    /// The table an unqualified token names: the owner table when it is spelled like the owner,
    /// else the first table of that spelling.
    fn resolve(&self, bare: &str, owner: Option<Position>) -> Option<Position> {
        owner.filter(|&(si, ti)| self.original.sheets[si].tables[ti].name.eq_ignore_ascii_case(bare)).or_else(|| self.table(bare))
    }

    fn apply(&self, wb: &mut Workbook) {
        table_refs::apply(self, wb, true);
        crate::passthrough::rewrite_kept_names(wb, |mut e| {
            table_refs::expression(self, &mut e, usize::MAX, None);
            e
        });
    }
}
