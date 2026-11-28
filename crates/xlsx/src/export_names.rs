//! Saving the defined names of an Excel 97-2003 (`.xls`) workbook as XLSX.
//!
//! Excel keeps an `.xls` workbook in Compatibility Mode, on the 65,536 × 256 grid, and so does
//! GridCraft for the names the `.xls` importer marks [`DefinedName::legacy_grid`]: their relative
//! parts wrap there ([MS-XLS] RgceLocRel). An XLSX file has no such names, so the save writes each
//! one's relative residues as the signed offset [`legacy_signed_offset`] picks, for the big grid.
//! One definition can't stand for both readings when a use sits near an edge (one-left from
//! column A is IV on the legacy grid, XFD on the big one), so the save never refuses: it writes
//! the converted names and warns, naming the uses it found reaching another cell and the names it
//! could not check. The live workbook keeps its marked names.
//!
//! The save also warns of names spelled like a cell on the big grid (`On2`, a valid name in
//! Compatibility Mode, is cell ON2 in an XLSX formula): their binding is not kept.
//!
//! [`legacy_signed_offset`]: gridcraft_formula::adjust::legacy_signed_offset

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::rc::Rc;

use gridcraft_core::{CellRef, MAX_COLS, MAX_ROWS, RangeRef};
use gridcraft_formula::adjust::{BIFF8_COLS, BIFF8_ROWS, legacy_name_text_for_xlsx, legacy_signed_offset};
use gridcraft_formula::{Anchor, Expr, RefKind, quote_sheet};
use gridcraft_model::{DefinedName, ValidationKind, Workbook};

/// The workbook as the XLSX file holds it and the warning for what the conversion can change.
/// Both write attempts of one save take this one projection, so they write the same names.
pub(crate) struct LegacyExport<'a> {
    pub wb: Cow<'a, Workbook>,
    /// The save's first warning, when there is something to say.
    pub notice: Option<String>,
}

/// Projects `wb` for an XLSX file: marked names converted, the marker dropped.
pub(crate) fn prepare(wb: &Workbook, functions: crate::FunctionInfo) -> LegacyExport<'_> {
    let any_marked = wb.names.iter().any(|n| n.legacy_grid);
    let cell_shaped: Vec<&DefinedName> = wb.names.iter().filter(|n| CellRef::parse(&n.name).is_some()).collect();
    let mut notice = if wb.names.is_empty() { String::new() } else { notices(wb, functions, true) };
    if !cell_shaped.is_empty() {
        if !notice.is_empty() {
            notice.push(' ');
        }
        notice.push_str("These names are spelled like cells on the .xlsx grid; formulas using those spellings can read cells instead of the names: ");
        notice.push_str(&cell_shaped.iter().map(|n| scoped(wb, n)).collect::<Vec<_>>().join(", "));
        notice.push('.');
    }
    let notice = (!notice.is_empty()).then_some(notice);
    if !any_marked {
        return LegacyExport { wb: Cow::Borrowed(wb), notice };
    }
    let mut out = wb.clone();
    for n in out.names.iter_mut().filter(|n| n.legacy_grid) {
        n.legacy_grid = false;
        if let Some(text) = legacy_name_text_for_xlsx(&n.formula) {
            n.formula = text;
        }
    }
    LegacyExport { wb: Cow::Owned(out), notice }
}

/// `Name (Sheet)` or `Name (workbook)`.
fn scoped(wb: &Workbook, n: &DefinedName) -> String {
    match n.scope.and_then(|s| wb.sheets.get(s)) {
        Some(sh) => format!("{} ({})", n.name, sh.name),
        None => format!("{} (workbook)", n.name),
    }
}

/// What one marked name's uses showed.
#[derive(Default)]
struct Seen {
    /// A use reached: some formula's lookup found the name.
    used: bool,
    /// The first use whose target differs after conversion (`'Sheet'!A1`).
    differs: Option<String>,
    /// A modeled use with multiple possible origins, or no cell origin at all.
    unknown_origin: bool,
    /// A literal INDIRECT can resolve to this definition.
    indirect: bool,
}

struct Family {
    first: usize,
    scopes: BTreeSet<Option<usize>>,
    report_duplicate: bool,
    /// Report unresolved uses on XLS import; on save, limit the notice to marked names and
    /// cell-shaped spellings. The spelling is a heuristic, not evidence of XLS origin.
    report_unresolved: bool,
    cell_shaped: bool,
    callers: BTreeSet<(usize, CellRef)>,
    unresolved: BTreeSet<(usize, CellRef)>,
}

pub(crate) fn import_notice(wb: &Workbook, functions: crate::FunctionInfo) -> Option<String> {
    if wb.names.is_empty() {
        return None;
    }
    let notice = notices(wb, functions, false);
    (!notice.is_empty()).then_some(notice)
}

/// Collect scope ambiguity and, on export, changes to targets on the larger grid.
fn notices(wb: &Workbook, functions: crate::FunctionInfo, export: bool) -> String {
    let mut families = BTreeMap::new();
    let mut cells = BTreeMap::new();
    for (i, n) in wb.names.iter().enumerate() {
        let cell = CellRef::parse(&n.name);
        let cell_shaped = cell.is_some();
        let key = n.name.to_ascii_lowercase();
        if let Some(cell) = cell {
            cells.entry(cell).or_insert_with(|| key.clone());
        }
        let f = families.entry(key).or_insert_with(|| Family {
            first: i,
            scopes: BTreeSet::new(),
            report_duplicate: false,
            report_unresolved: !export,
            cell_shaped,
            callers: BTreeSet::new(),
            unresolved: BTreeSet::new(),
        });
        f.scopes.insert(n.scope);
        f.report_duplicate |= n.legacy_grid || cell_shaped;
        f.report_unresolved |= n.legacy_grid || cell_shaped;
    }
    let check_grid = export && wb.names.iter().any(|n| n.legacy_grid);
    if !check_grid && cells.is_empty() && !wb.names.iter().any(|n| n.scope.is_some()) {
        return String::new();
    }
    let mut walk = Walk {
        wb,
        functions,
        check_grid,
        families,
        cells,
        bodies: vec![None; wb.names.len()],
        seen: wb.names.iter().map(|_| Seen::default()).collect(),
        any_indirect: false,
    };
    walk.scan_consumers(check_grid);
    let mut notice = walk.ambiguity();
    if !check_grid {
        return notice;
    }
    // A name body can call INDIRECT too, used or not.
    for (i, name) in wb.names.iter().enumerate() {
        if let Some(body) = walk.body(i) {
            walk.indirect(&body, name.scope);
        }
    }
    append_grid_notice(&walk, &mut notice);
    notice
}

impl Walk<'_> {
    fn scan_consumers(&mut self, check_grid: bool) {
        let wb = self.wb;
        let walk = self;
        for (si, sh) in wb.sheets.iter().enumerate() {
            for (at, cell) in sh.cells.iter() {
                if let Some(f) = cell.formula.as_ref() {
                    if let Some((e, dr, dc)) = f.expr_shifted() {
                        walk.consumer_shifted(e, si, Some(at), (dr, dc));
                    } else {
                        f.with_expr(|e| walk.consumer(e, si, Some(at)));
                    }
                }
            }
            for dv in &sh.validations {
                let at = RangeRef::top_left(&dv.ranges);
                for f in dv.formulas() {
                    // A literal list (`"a,b"` or `a,b`) holds no references; a list from cells starts with `=`.
                    if dv.kind == ValidationKind::List && !f.trim_start().starts_with('=') {
                        continue;
                    }
                    if let Ok(e) = gridcraft_formula::parse(f.trim_start().trim_start_matches('=')) {
                        walk.rule(&e, si, at, &dv.ranges);
                    }
                }
            }
            for cf in &sh.cond_formats {
                let at = RangeRef::top_left(&cf.ranges);
                for f in cf.rule.formulas() {
                    if let Ok(e) = gridcraft_formula::parse(f.trim_start_matches('=')) {
                        walk.rule(&e, si, at, &cf.ranges);
                    }
                }
            }
            if check_grid {
                walk.scan_unanchored(si, sh);
            }
        }
    }

    fn scan_unanchored(&mut self, si: usize, sh: &gridcraft_model::Sheet) {
        let wb = self.wb;
        let mut unanchored = |f: &str| {
            if let Ok(e) = gridcraft_formula::parse(f.trim_start().trim_start_matches('=')) {
                self.consumer(&e, si, None);
            }
        };
        for table in &sh.tables {
            for col in &table.columns {
                if let Some(f) = &col.formula {
                    unanchored(f);
                }
                if col.totals == gridcraft_model::TotalsFn::Custom
                    && let Some(f) = &col.totals_label
                {
                    unanchored(f);
                }
            }
        }
        for chart in &sh.charts {
            for series in &chart.series {
                unanchored(&series.values);
                for f in [&series.categories, &series.bubble_sizes].into_iter().flatten() {
                    unanchored(f);
                }
                // A chart legend's series name can also be literal text. Only a workbook name
                // actually resolved by the chart writer is a reference here.
                if let Some(f) = &series.name
                    && crate::chart::is_defined_name(wb, si, f)
                {
                    unanchored(f);
                }
            }
        }
        for spark in &sh.sparklines {
            unanchored(&spark.source);
        }
        for pivot in &sh.pivots {
            if wb.table(pivot.source.trim().trim_start_matches('=')).is_none() {
                unanchored(&pivot.source);
            }
        }
    }
}

fn append_grid_notice(walk: &Walk<'_>, notice: &mut String) {
    let wb = walk.wb;
    let mut differs = Vec::new();
    let mut unchecked = Vec::new();
    for (n, s) in wb.names.iter().zip(&walk.seen).filter(|(n, _)| n.legacy_grid) {
        if let Some(caller) = &s.differs {
            differs.push(format!("{} from {caller}", scoped(wb, n)));
        }
        if walk.any_indirect || s.indirect {
            unchecked.push(format!("{} (INDIRECT may reach it)", scoped(wb, n)));
        } else if s.unknown_origin {
            unchecked.push(format!("{} (not all caller locations could be checked)", scoped(wb, n)));
        } else if !s.used {
            unchecked.push(format!("{} (no checked use found)", scoped(wb, n)));
        }
    }
    if differs.is_empty() && unchecked.is_empty() {
        return;
    }
    if !notice.is_empty() {
        notice.push(' ');
    }
    notice
        .push_str("Names from the Excel 97-2003 workbook that point at cells relative to where they are used were saved for the larger .xlsx grid.");
    if !differs.is_empty() {
        let _ = write!(notice, " These uses can read different cells after recalculation: {}.", differs.join(", "));
    }
    if !unchecked.is_empty() {
        let _ = write!(notice, " These names could not be checked: {}.", unchecked.join(", "));
    }
}

struct Walk<'a> {
    wb: &'a Workbook,
    functions: crate::FunctionInfo,
    check_grid: bool,
    families: BTreeMap<String, Family>,
    cells: BTreeMap<CellRef, String>,
    /// Each name's parsed body, parsed on first use (`Some(None)`: it doesn't parse).
    bodies: Vec<Option<Option<Rc<Expr>>>>,
    seen: Vec<Seen>,
    /// A computed `INDIRECT` argument: any marked name may be reached.
    any_indirect: bool,
}

impl Walk<'_> {
    fn body(&mut self, i: usize) -> Option<Rc<Expr>> {
        let wb = self.wb;
        self.bodies[i].get_or_insert_with(|| gridcraft_formula::parse_name(&wb.names[i].formula).ok().map(Rc::new)).clone()
    }

    fn rule(&mut self, e: &Expr, sheet: usize, at: Option<CellRef>, ranges: &[RangeRef]) {
        if let Some(at) = at {
            self.consumer(e, sheet, Some(at));
        }
        if at.is_none() || ranges.iter().any(|r| r.start != r.end || Some(r.start) != at) {
            self.consumer(e, sheet, None);
        }
    }

    /// Keep the original caller even when a local name switches the evaluation sheet.
    fn consumer(&mut self, e: &Expr, sheet: usize, at: Option<CellRef>) {
        self.consumer_shifted(e, sheet, at, (0, 0));
    }

    fn consumer_shifted(&mut self, e: &Expr, sheet: usize, at: Option<CellRef>, shift: (i32, i32)) {
        let mut pending = Vec::new();
        let mut visited = Vec::new();
        let caller = at.map(|a| (sheet, a));
        self.collect(e, sheet, caller, shift, &mut pending);
        while let Some((i, inner)) = pending.pop() {
            if visited.contains(&(i, inner)) {
                continue;
            }
            visited.push((i, inner));
            let Some(body) = self.body(i) else { continue };
            if self.check_grid && self.wb.names[i].legacy_grid {
                if let Some(at) = at {
                    self.compare(i, &body, at, (sheet, at));
                } else {
                    self.seen[i].unknown_origin = true;
                }
            }
            self.collect(&body, inner, caller, (0, 0), &mut pending);
        }
    }

    fn collect(&mut self, e: &Expr, sheet: usize, caller: Option<(usize, CellRef)>, shift: (i32, i32), pending: &mut Vec<(usize, usize)>) {
        visit_uses(e, self.functions, |site| match site {
            Use::Name(n, operand) => {
                let (lookup, bare, qualified) = match n.rsplit_once('!') {
                    Some((s, b)) => match self.wb.sheet_index(s) {
                        Some(i) => (i, b, true),
                        None => return,
                    },
                    None => (sheet, n, false),
                };
                let visible = self.lookup(bare, lookup);
                if let Some(caller) = caller
                    && let Some(f) = self.families.get_mut(&bare.to_ascii_lowercase())
                {
                    if f.report_unresolved && operand && !qualified && visible.is_none() && self.wb.table(bare).is_none() {
                        f.unresolved.insert(caller);
                    }
                    if f.report_duplicate && f.scopes.len() > 1 {
                        f.callers.insert(caller);
                    }
                }
                if let Some(i) = visible {
                    pending.push((i, self.wb.names[i].scope.unwrap_or(sheet)));
                }
            }
            Use::Cell(a) => {
                let row = i64::from(a.row) + i64::from(shift.0);
                let col = i64::from(a.col) + i64::from(shift.1);
                if let Some(caller) = caller
                    && (0..i64::from(gridcraft_core::MAX_ROWS)).contains(&row)
                    && (0..i64::from(gridcraft_core::MAX_COLS)).contains(&col)
                    && (row >= i64::from(BIFF8_ROWS) || col >= i64::from(BIFF8_COLS))
                    && let Some(key) = self.cells.get(&CellRef::new(row as u32, col as u32))
                    && let Some(f) = self.families.get_mut(key)
                {
                    if !f.scopes.contains(&None) && !f.scopes.contains(&Some(sheet)) {
                        f.unresolved.insert(caller);
                    }
                    if f.report_duplicate && f.scopes.len() > 1 {
                        f.callers.insert(caller);
                    }
                }
            }
            Use::Indirect(arg) if self.check_grid => self.indirect_arg(arg, Some(sheet)),
            Use::Indirect(_) => {}
        });
    }

    fn ambiguity(&self) -> String {
        let mut details = Vec::new();
        let callers = |sites: &BTreeSet<(usize, CellRef)>| {
            sites.iter().map(|&(si, at)| format!("{}!{at}", quote_sheet(&self.wb.sheets[si].name))).collect::<Vec<_>>().join(", ")
        };
        for f in self.families.values() {
            let duplicate = f.report_duplicate && f.scopes.len() > 1;
            if !duplicate && f.unresolved.is_empty() {
                continue;
            }
            let n = &self.wb.names[f.first];
            let scopes = f.scopes.iter().map(|s| s.and_then(|i| self.wb.sheet(i)).map_or("workbook", |sh| sh.name.as_str())).collect::<Vec<_>>();
            let mut desc = format!("{} (scopes: {})", n.name, scopes.join(", "));
            if duplicate {
                if f.callers.is_empty() {
                    desc.push_str("; no consuming cell found");
                } else {
                    let _ = write!(desc, "; possible consuming cells: {}", callers(&f.callers));
                }
            }
            if !f.unresolved.is_empty() {
                let _ = write!(desc, "; no definition in scope at {}", callers(&f.unresolved));
            }
            if f.cell_shaped {
                desc.push_str("; GridCraft currently reads the cell reference, not the name");
            }
            details.push(desc);
        }
        if details.is_empty() { String::new() } else { format!("Name bindings are unproven: {}.", details.join("; ")) }
    }

    /// The index of the name `Workbook::name` finds: sheet scope first, then workbook scope.
    fn lookup(&self, bare: &str, sheet: usize) -> Option<usize> {
        let names = &self.wb.names;
        names
            .iter()
            .position(|n| n.scope == Some(sheet) && n.name.eq_ignore_ascii_case(bare))
            .or_else(|| names.iter().position(|n| n.scope.is_none() && n.name.eq_ignore_ascii_case(bare)))
    }

    /// Whether marked name `i`, used at `at`, reaches the same cells once converted.
    fn compare(&mut self, i: usize, body: &Expr, at: CellRef, (si, cell): (usize, CellRef)) {
        let seen = &mut self.seen[i];
        seen.used = true;
        if seen.differs.is_some() {
            return;
        }
        let differs = |k, origin, legacy, modern| {
            let old = (i64::from(k) + i64::from(origin)).rem_euclid(i64::from(legacy));
            let new = (legacy_signed_offset(k, legacy) + i64::from(origin)).rem_euclid(i64::from(modern));
            old != new
        };
        let row = |k, absolute: bool| !absolute && differs(k, at.row, BIFF8_ROWS, MAX_ROWS);
        let col = |k, absolute: bool| !absolute && differs(k, at.col, BIFF8_COLS, MAX_COLS);
        let anchor = |a: &Anchor| row(a.row, a.row_abs) || col(a.col, a.col_abs);
        let mut different = false;
        body.walk(&mut |e| {
            if !different && let Expr::Ref(r) = e {
                different = match &r.kind {
                    RefKind::Cell(a) => anchor(a),
                    RefKind::Range(a, b) => anchor(a) || anchor(b),
                    RefKind::Rows(a, aa, b, ba) => row(*a, *aa) || row(*b, *ba),
                    RefKind::Cols(a, aa, b, ba) => col(*a, *aa) || col(*b, *ba),
                };
            }
        });
        if different {
            seen.differs = Some(format!("{}!{cell}", quote_sheet(&self.wb.sheets[si].name)));
        }
    }

    /// The `INDIRECT` calls of a name body.
    fn indirect(&mut self, e: &Expr, sheet: Option<usize>) {
        visit_uses(e, self.functions, |site| {
            if let Use::Indirect(arg) = site {
                self.indirect_arg(arg, sheet);
            }
        });
    }

    /// An `INDIRECT` text: a literal may name a marked name (`"Tests"`, `"Sheet1!Tests"`); a
    /// computed one may name any.
    fn indirect_arg(&mut self, arg: &Expr, sheet: Option<usize>) {
        let Expr::Text(text) = arg else {
            self.any_indirect |= !matches!(arg, Expr::Missing);
            return;
        };
        let Ok(Expr::Name(name)) = gridcraft_formula::parse(text) else { return };
        if name.starts_with('[') {
            return;
        }
        if let Some((qualified, bare)) = name.rsplit_once('!') {
            if let Some(index) = self.wb.sheet_index(qualified).and_then(|sheet| self.lookup(bare, sheet)) {
                self.seen[index].indirect = true;
            }
        } else if let Some(sheet) = sheet {
            if let Some(index) = self.lookup(&name, sheet) {
                self.seen[index].indirect = true;
            }
        } else {
            // A workbook definition has no fixed sheet for an unqualified operand.
            for (definition, seen) in self.wb.names.iter().zip(&mut self.seen) {
                if definition.name.eq_ignore_ascii_case(&name) {
                    seen.indirect = true;
                }
            }
        }
    }
}

enum Use<'a> {
    Name(&'a str, bool), // spelling, true for a name operand (rather than a function call)
    Cell(&'a gridcraft_formula::Anchor),
    Indirect(&'a Expr),
}

/// Visits free name operands, user-defined call targets and INDIRECT arguments. An explicit
/// stack handles both long operator spines and lexical scope exit without recursive AST walks.
fn visit_uses(e: &Expr, functions: crate::FunctionInfo, mut visit: impl FnMut(Use<'_>)) {
    enum Step<'a> {
        Expr(&'a Expr),
        Bind(&'a str),
        Restore(usize),
    }
    let mut bindings: Vec<&str> = Vec::new();
    let mut steps = Vec::with_capacity(8);
    steps.push(Step::Expr(e));
    while let Some(step) = steps.pop() {
        let e = match step {
            Step::Expr(e) => e,
            Step::Bind(n) => {
                bindings.push(n);
                continue;
            }
            Step::Restore(len) => {
                bindings.truncate(len);
                continue;
            }
        };
        let free = |n: &str| !n.starts_with('[') && !bindings.iter().any(|b| gridcraft_formula::same_name(b, n));
        match e {
            Expr::Name(n) if free(n) => visit(Use::Name(n, true)),
            // A BIFF8 cell cannot be outside its grid. This is only a possible name operand.
            Expr::Ref(r) if r.sheet == gridcraft_formula::SheetSel::Current => {
                if let gridcraft_formula::RefKind::Cell(a) = &r.kind
                    && !a.row_abs
                    && !a.col_abs
                {
                    visit(Use::Cell(a));
                }
            }
            Expr::Call(f, args)
                if f == "LET"
                    && args.len() >= 3
                    && !args.len().is_multiple_of(2)
                    && let Some((body, pairs)) = args.split_last() =>
            {
                steps.push(Step::Restore(bindings.len()));
                steps.push(Step::Expr(body));
                for pair in pairs.as_chunks::<2>().0.iter().rev() {
                    if let Expr::Name(n) = &pair[0] {
                        steps.push(Step::Bind(n));
                    }
                    steps.push(Step::Expr(&pair[1]));
                }
            }
            Expr::Call(f, args) if f == "LAMBDA" => {
                if let Some((body, params)) = args.split_last() {
                    steps.push(Step::Restore(bindings.len()));
                    bindings.extend(params.iter().filter_map(|p| if let Expr::Name(n) = p { Some(n.as_str()) } else { None }));
                    steps.push(Step::Expr(body));
                }
            }
            Expr::Call(f, args) => {
                let user_defined = gridcraft_formula::user_defined_name(f);
                let n = user_defined.unwrap_or(f);
                if f == "INDIRECT" {
                    if let [arg] | [arg, _] = args.as_slice()
                        && !matches!(arg, Expr::Text(t) if gridcraft_formula::parse(t).is_ok_and(|e| matches!(e, Expr::Name(n) if !free(&n))))
                    {
                        visit(Use::Indirect(arg));
                    }
                } else if user_defined.is_some() || (free(n) && !(functions.is_builtin)(f)) {
                    visit(Use::Name(n, false));
                }
                steps.extend(args.iter().rev().map(Step::Expr));
            }
            Expr::Unary(_, e) | Expr::Paren(e) => steps.push(Step::Expr(e)),
            Expr::Binary(_, a, b) => steps.extend([Step::Expr(b), Step::Expr(a)]),
            Expr::Array(rows) => steps.extend(rows.iter().flatten().rev().map(Step::Expr)),
            Expr::Invoke(c, args) => {
                steps.extend(args.iter().rev().map(Step::Expr));
                steps.push(Step::Expr(c));
            }
            _ => {}
        }
    }
}
