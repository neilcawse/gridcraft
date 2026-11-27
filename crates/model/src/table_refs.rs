//! Traversal of modeled table references. Interactive rename and XLSX namespace repair share
//! the consumers, not the rules that bind ambiguous tokens to a table.

use std::sync::Arc;

use gridcraft_core::{CellRef, link_location, quote_sheet, split_sheet};
use gridcraft_formula::{Expr, StructRef, same_name};

use crate::workbook::rewrite_table_tokens;
use crate::{CfRule, CfValueKind, Formula, PivotSource, PivotTable, Sheet, SlicerSource, TotalsFn, Workbook};

pub type TablePosition = (usize, usize);

/// An interactive rename resolves references in the original workbook, before the old name
/// disappears and before the new one can shadow anything.
pub struct TableRename<'a> {
    wb: &'a Workbook,
    at: TablePosition,
    old: &'a str,
    new: &'a str,
    first: bool,
    /// LAMBDA names, as the workbook spells them.
    lambdas: Vec<String>,
}

impl<'a> TableRename<'a> {
    pub fn new(wb: &'a Workbook, at: TablePosition, new: &'a str) -> Option<Self> {
        let old = wb.sheets.get(at.0)?.tables.get(at.1)?.name.as_str();
        (old != new).then(|| Self { wb, at, old, new, first: wb.table(old) == Some(at), lambdas: wb.lambda_names() })
    }

    pub fn apply(&self, wb: &mut Workbook) {
        // Interim X22 policy: await Excel measurements for direct references in CF, DV,
        // charts, sparklines, hyperlinks and form controls. X17's repair is unaffected.
        const REWRITE_FORMULA_OBJECTS_PENDING_EXCEL_MEASUREMENT: bool = false;
        apply(self, wb, REWRITE_FORMULA_OBJECTS_PENDING_EXCEL_MEASUREMENT);
    }
}

impl TableRefRewrite for TableRename<'_> {
    fn table(&self, name: &str) -> Option<TablePosition> {
        self.wb.table(name)
    }

    fn renamed(&self, at: TablePosition) -> Option<&str> {
        (at == self.at).then_some(self.new)
    }

    fn might_change(&self, name: &str, owner: Option<TablePosition>) -> bool {
        (self.first || owner == Some(self.at)) && name.eq_ignore_ascii_case(self.old)
    }

    fn replacement(&self, name: &str, structured: bool, sheet: usize, owner: Option<TablePosition>, binders: &[String]) -> Option<Expr> {
        if name.starts_with('[') || structured && name.contains('!') {
            return None;
        }
        let (scope, bare, qualifier) =
            if !structured && let Some((s, bare)) = name.rsplit_once('!') { (self.wb.sheet_index(s)?, bare, Some(s)) } else { (sheet, name, None) };
        if !self.might_change(bare, owner.filter(|_| qualifier.is_none())) {
            return None;
        }
        if structured {
            return Some(Expr::Name(self.new.into()));
        }
        // Workbook-scoped name bodies can use a different local definition for each caller.
        if self.wb.name(bare, scope).is_some()
            || scope == usize::MAX && self.wb.names.iter().any(|n| n.scope.is_some() && n.name.eq_ignore_ascii_case(bare))
        {
            return None;
        }
        let renamed = qualifier.map_or_else(|| self.new.into(), |s| format!("{s}!{}", self.new));
        let captured = binders.iter().any(|local| same_name(local, &renamed))
            || self.wb.name(self.new, scope).is_some()
            || scope == usize::MAX && self.wb.names.iter().any(|n| same_name(&n.name, self.new));
        Some(if captured {
            Expr::Struct(StructRef { table: self.new.into(), specifiers: vec![], col_start: None, col_end: None })
        } else {
            Expr::Name(renamed)
        })
    }

    fn pivot(&self, pivot: &PivotTable, sheet: usize) -> Option<(String, Option<&str>)> {
        // What the PivotTable reads: its bound table, the same-kind one that took a gone binding's
        // place, or for unbound text the table that the text names first.
        let found = self.wb.pivot_source(pivot, sheet).ok()?;
        let qualified = match found.source {
            PivotSource::Table { sheet, table } if (sheet, table) == self.at => split_sheet(&pivot.source).0,
            PivotSource::Text(text) if !pivot.binding.unresolved(text) && self.might_change(text.trim().trim_start_matches('=').trim(), None) => None,
            _ => return None,
        };
        let source = qualified.map_or_else(|| self.new.into(), |sheet| format!("{}!{}", quote_sheet(&sheet), self.new));
        Some((source, found.rebound.is_none().then_some(self.new)))
    }

    fn respell(&self, mut expr: Expr, sheet: usize) -> Expr {
        if !self.lambdas.is_empty() {
            spell_lambda_calls(&mut expr, &self.lambdas, self.wb, (sheet != usize::MAX).then_some(sheet), &mut Vec::new());
        }
        expr
    }

    fn keep_link_marker(&self) -> bool {
        true
    }
}

/// Resolution policy for a table reference rewrite. Lookups must use the pre-edit workbook.
/// `usize::MAX` is the sheet context of a workbook-scoped defined name.
pub trait TableRefRewrite {
    /// The table visible to a consumer that uses direct workbook lookup, without name shadowing.
    fn table(&self, name: &str) -> Option<TablePosition>;
    fn renamed(&self, at: TablePosition) -> Option<&str>;
    /// File repair can introduce a spelling that would change the meaning of an unbound token.
    fn captures(&self, _name: &str) -> bool {
        false
    }
    fn cell_owner(&self, _sheet: usize, _cell: CellRef) -> Option<TablePosition> {
        None
    }
    /// Cheap prefilter to avoid cloning unaffected cell expressions.
    fn might_change(&self, name: &str, owner: Option<TablePosition>) -> bool;
    fn replacement(&self, name: &str, structured: bool, sheet: usize, owner: Option<TablePosition>, binders: &[String]) -> Option<Expr>;
    /// The new source text and, if the binding's last-seen table name should change, its new name.
    fn pivot(&self, pivot: &PivotTable, sheet: usize) -> Option<(String, Option<&str>)>;
    /// Preserve the optional hash marker for interactive edits; file repair keeps its previous
    /// canonicalization to a bare location.
    fn keep_link_marker(&self) -> bool {
        false
    }
    /// Spell user function calls in a changed expression. The parser upper-cases them
    /// (`DOUBLE(…)` for `Double(…)`), in formula text and in a cell tree read from a native file.
    /// File repair leaves that to the reader's later pass over every formula.
    fn respell(&self, expr: Expr, _sheet: usize) -> Expr {
        expr
    }
}

pub fn expression(rewrite: &impl TableRefRewrite, expr: &mut Expr, sheet: usize, owner: Option<TablePosition>) -> bool {
    let changed = rewrite_table_tokens(expr, &|name, structured, env| rewrite.replacement(name, structured, sheet, owner, env));
    if changed {
        *expr = rewrite.respell(std::mem::replace(expr, Expr::Missing), sheet);
    }
    changed
}

/// Spells calls of the workbook's LAMBDA names as the names are defined. A call of a LET or
/// LAMBDA binder in scope is not a call of the workbook's name and keeps its spelling.
fn spell_lambda_calls(mut e: &mut Expr, lambdas: &[String], wb: &Workbook, sheet: Option<usize>, env: &mut Vec<String>) {
    while let Expr::Binary(_, a, b) = e {
        spell_lambda_calls(b, lambdas, wb, sheet, env);
        e = a;
    }
    let before = env.len();
    match e {
        Expr::Call(name, args) if name == "LET" && args.len() >= 3 && args.len() % 2 != 0 => {
            if let Some((body, pairs)) = args.split_last_mut() {
                for [binder, value] in pairs.as_chunks_mut::<2>().0 {
                    spell_lambda_calls(value, lambdas, wb, sheet, env);
                    if let Expr::Name(n) = binder {
                        env.push(n.clone());
                    }
                }
                spell_lambda_calls(body, lambdas, wb, sheet, env);
            }
        }
        Expr::Call(name, args) if name == "LAMBDA" => {
            if let Some((body, params)) = args.split_last_mut() {
                for param in params.iter() {
                    if let Expr::Name(n) = param {
                        env.push(n.trim_start_matches('[').trim_end_matches(']').into());
                    }
                }
                spell_lambda_calls(body, lambdas, wb, sheet, env);
            }
        }
        Expr::Call(name, args) => {
            if !env.iter().any(|local| same_name(local, name))
                && let Some(defined) = crate::workbook::lambda_call_spelling(name, lambdas, &wb.names, sheet)
            {
                *name = defined;
            }
            args.iter_mut().for_each(|a| spell_lambda_calls(a, lambdas, wb, sheet, env));
        }
        Expr::Invoke(callee, args) => {
            spell_lambda_calls(callee, lambdas, wb, sheet, env);
            args.iter_mut().for_each(|a| spell_lambda_calls(a, lambdas, wb, sheet, env));
        }
        Expr::Array(rows) => rows.iter_mut().flatten().for_each(|x| spell_lambda_calls(x, lambdas, wb, sheet, env)),
        Expr::Unary(_, x) | Expr::Paren(x) => spell_lambda_calls(x, lambdas, wb, sheet, env),
        _ => {}
    }
    env.truncate(before);
}

fn rewritten_text(rewrite: &impl TableRefRewrite, text: &str, sheet: usize, owner: Option<TablePosition>) -> Option<String> {
    let mut expr = gridcraft_formula::parse_name(text).ok()?;
    expression(rewrite, &mut expr, sheet, owner).then(|| {
        let prefix = if text.trim_start().starts_with('=') { "=" } else { "" };
        format!("{prefix}{}", gridcraft_formula::print(&expr))
    })
}

/// Apply the reference policy to a workbook cloned from the one used for lookup. Take mutable
/// access to each shared sheet only when a consumer on it has actually changed.
pub fn apply(rewrite: &impl TableRefRewrite, wb: &mut Workbook, formula_objects: bool) {
    for n in &mut wb.names {
        if let Some(new) = rewritten_text(rewrite, &n.formula, n.scope.unwrap_or(usize::MAX), None) {
            n.formula = new;
        }
    }
    for (si, sh) in wb.sheets.iter_mut().enumerate() {
        let rewritten: Vec<_> = sh
            .cells
            .iter()
            .filter_map(|(c, cell)| {
                let f = cell.formula.as_ref()?;
                let owner = rewrite.cell_owner(si, c);
                f.with_expr(|expr| {
                    let mut found = false;
                    expr.walk(&mut |e| {
                        found |= match e {
                            Expr::Name(n) => match n.rsplit_once('!') {
                                Some((_, bare)) => rewrite.might_change(bare, None),
                                None => rewrite.might_change(n, owner),
                            },
                            Expr::Struct(s) => rewrite.might_change(&s.table, owner),
                            _ => false,
                        };
                    });
                    if !found {
                        return None;
                    }
                    let mut expr = expr.clone();
                    expression(rewrite, &mut expr, si, owner).then(|| (c, Formula { array: f.array, ..f.rewritten(expr) }))
                })?
            })
            .collect();
        for (c, f) in rewritten {
            if let Some(cell) = Arc::make_mut(sh).cells.get_mut(c) {
                cell.formula = Some(Arc::new(f));
            }
        }
        if formula_objects {
            rewrite_objects(rewrite, sh, si);
        }
        for ti in 0..sh.tables.len() {
            let local = |f: &str| rewritten_text(rewrite, f, si, Some((si, ti)));
            for ci in 0..sh.tables[ti].columns.len() {
                let col = &sh.tables[ti].columns[ci];
                let formula = col.formula.as_deref().and_then(local);
                let totals = if col.totals == TotalsFn::Custom { col.totals_label.as_deref().and_then(local) } else { None };
                if formula.is_some() || totals.is_some() {
                    let col = &mut Arc::make_mut(sh).tables[ti].columns[ci];
                    if let Some(f) = formula {
                        col.formula = Some(f);
                    }
                    if let Some(f) = totals {
                        col.totals_label = Some(f);
                    }
                }
            }
            if let Some(name) = rewrite.renamed((si, ti)) {
                Arc::make_mut(sh).tables[ti].name = name.into();
            }
        }
        for i in 0..sh.slicers.len() {
            if let SlicerSource::Table { table, .. } = &sh.slicers[i].source
                && let Some(name) = rewrite.table(table).and_then(|at| rewrite.renamed(at))
                && let SlicerSource::Table { table, .. } = &mut Arc::make_mut(sh).slicers[i].source
            {
                *table = name.into();
            }
        }
        for i in 0..sh.pivots.len() {
            if let Some((source, name)) = rewrite.pivot(&sh.pivots[i], si) {
                let p = &mut Arc::make_mut(sh).pivots[i];
                if let Some(binding) = &mut p.binding.0
                    && binding.text == p.source
                {
                    binding.text.clone_from(&source);
                    if let Some(name) = name {
                        binding.was = format!("table \"{name}\"");
                    }
                }
                p.source = source;
            }
        }
    }
}

fn rewrite_objects(rewrite: &impl TableRefRewrite, sh: &mut Arc<Sheet>, si: usize) {
    let text = |f: &str| rewritten_text(rewrite, f, si, None);
    macro_rules! field {
        ($($field:tt)+) => {
            if let Some(new) = text(&sh.$($field)+) {
                Arc::make_mut(sh).$($field)+ = new;
            }
        };
    }
    macro_rules! optional {
        ($($field:tt)+) => {
            if let Some(new) = sh.$($field)+.as_deref().and_then(text) {
                Arc::make_mut(sh).$($field)+ = Some(new);
            }
        };
    }
    for i in 0..sh.cond_formats.len() {
        if let Some(rule) = conditional_format(&sh.cond_formats[i].rule, text) {
            Arc::make_mut(sh).cond_formats[i].rule = rule;
        }
    }
    for i in 0..sh.validations.len() {
        field!(validations[i].f1);
        optional!(validations[i].f2);
    }
    for i in 0..sh.charts.len() {
        for j in 0..sh.charts[i].series.len() {
            field!(charts[i].series[j].values);
            optional!(charts[i].series[j].categories);
            optional!(charts[i].series[j].bubble_sizes);
            if let Some(n) = &sh.charts[i].series[j].name
                && let Ok(Expr::Struct(r)) = gridcraft_formula::parse(n)
                && !r.table.is_empty()
            {
                let new = if rewrite.table(&r.table).is_some() {
                    text(n)
                } else {
                    // Do not turn an unbound lookalike into a reference (and a ChartEx alias).
                    rewrite.captures(&r.table).then(|| format!("\"{}\"", n.replace('"', "\"\"")))
                };
                if let Some(new) = new {
                    Arc::make_mut(sh).charts[i].series[j].name = Some(new);
                }
            }
        }
    }
    let links: Vec<_> = sh
        .hyperlinks
        .iter()
        .filter_map(|(&cell, link)| {
            if link.external {
                return None;
            }
            let raw = link.target.trim();
            let target = link_location(raw);
            if target.starts_with('#') {
                return None;
            }
            let mut new = text(target)?;
            if rewrite.keep_link_marker() && target != raw {
                new.insert(0, '#');
            }
            Some((cell, new))
        })
        .collect();
    for (cell, new) in links {
        if let Some(link) = Arc::make_mut(sh).hyperlinks.get_mut(&cell) {
            link.target = new;
        }
    }
    for i in 0..sh.sparklines.len() {
        field!(sparklines[i].source);
    }
    for i in 0..sh.controls.len() {
        optional!(controls[i].link);
    }
}

fn conditional_format(rule: &CfRule, text: impl Fn(&str) -> Option<String>) -> Option<CfRule> {
    let threshold = |v: &CfValueKind| match v {
        CfValueKind::Formula(f) => text(f).map(CfValueKind::Formula),
        _ => None,
    };
    match rule {
        CfRule::CellIs { a, b, .. } => {
            let new_a = text(a);
            let new_b = b.as_deref().and_then(text);
            if new_a.is_none() && new_b.is_none() {
                return None;
            }
            let mut out = rule.clone();
            if let CfRule::CellIs { a, b, .. } = &mut out {
                if let Some(new) = new_a {
                    *a = new;
                }
                if let Some(new) = new_b {
                    *b = Some(new);
                }
            }
            Some(out)
        }
        CfRule::Expression { formula, style } => Some(CfRule::Expression { formula: text(formula)?, style: style.clone() }),
        CfRule::ColorScale { stops } => {
            let mut out = None;
            for (i, (value, _)) in stops.iter().enumerate() {
                if let Some(new) = threshold(value) {
                    out.get_or_insert_with(|| stops.clone())[i].0 = new;
                }
            }
            Some(CfRule::ColorScale { stops: out? })
        }
        CfRule::DataBar { min, max, color, gradient, show_value } => {
            let lo = threshold(min);
            let hi = threshold(max);
            if lo.is_none() && hi.is_none() {
                return None;
            }
            Some(CfRule::DataBar {
                min: lo.unwrap_or_else(|| min.clone()),
                max: hi.unwrap_or_else(|| max.clone()),
                color: *color,
                gradient: *gradient,
                show_value: *show_value,
            })
        }
        CfRule::IconSet { set, thresholds, reverse, show_value } => {
            let mut out = None;
            for (i, value) in thresholds.iter().enumerate() {
                if let Some(new) = threshold(value) {
                    out.get_or_insert_with(|| thresholds.clone())[i] = new;
                }
            }
            Some(CfRule::IconSet { set: set.clone(), thresholds: out?, reverse: *reverse, show_value: *show_value })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use gridcraft_core::RangeRef;

    use super::*;
    use crate::{Cell, CondFormat, DefinedName, Table, TableColumn, Validation};

    fn book() -> Workbook {
        let mut wb = Workbook::new();
        let col = |n: &str| TableColumn { name: n.into(), totals: TotalsFn::None, totals_label: None, formula: None, unique_name: None, xml: None };
        let sh = wb.sheet_mut(0).unwrap();
        sh.name = "Data".into();
        sh.tables.push(Table {
            id: 1,
            name: "Sales".into(),
            range: RangeRef::parse("A1:C3").unwrap(),
            header_row: true,
            totals_row: false,
            columns: vec![col("V"), col("Q'ty [#]"), col("A B")],
            style: String::new(),
            banded_rows: false,
            banded_cols: false,
            first_col: false,
            last_col: false,
            filter_button: true,
            filter: vec![],
            table_type: None,
            connection_id: None,
            uid: Default::default(),
        });
        wb
    }

    fn name(name: &str, formula: &str) -> DefinedName {
        DefinedName { name: name.into(), scope: None, formula: formula.into(), comment: String::new(), hidden: false, uid: Default::default() }
    }

    /// Defined names keep their escapes, binders, literals and LAMBDA call spelling; only the
    /// table token changes, whatever its case.
    #[test]
    fn rename_rewrites_only_table_tokens_in_defined_names() {
        let cases = [
            ("SUM(Sales[V])", "SUM(Renamed[V])"),
            ("Sales[[#Headers],[A B]]", "Renamed[[#Headers],[A B]]"),
            ("Sales[#All]", "Renamed[#All]"),
            ("Sales[@[A B]]", "Renamed[@[A B]]"),
            ("Sales[[#This Row],[Q''ty '['#']]]", "Renamed[@[Q''ty '['#']]]"),
            ("\"Sales[V]\"&INDIRECT(\"Sales\")", "\"Sales[V]\"&INDIRECT(\"Sales\")"),
            ("LET(Renamed,1,SUM(Sales)+Renamed)", "LET(Renamed,1,SUM(Renamed[])+Renamed)"),
            ("LAMBDA(sales,sales+SUM(SALES[v]))", "LAMBDA(sales,sales+SUM(Renamed[v]))"),
            ("ROWS(sales)+ROWS(Sales_1)", "ROWS(Renamed)+ROWS(Sales_1)"),
            ("ROWS(Data!Sales)", "ROWS(Data!Renamed)"),
            ("Double(SUM(Sales[V]))", "Double(SUM(Renamed[V]))"),
            ("LET(double,LAMBDA(x,x+1),DOUBLE(SUM(Sales[V])))", "LET(double,LAMBDA(x,x+1),DOUBLE(SUM(Renamed[V])))"),
            (
                "LET(double,LAMBDA(x,DOUBLE(x)+SUM(Sales[V])),LET(inner,DOUBLE(1),DOUBLE(inner)))+DOUBLE(3)",
                "LET(double,LAMBDA(x,Double(x)+SUM(Renamed[V])),LET(inner,DOUBLE(1),DOUBLE(inner)))+Double(3)",
            ),
            (
                "LAMBDA(double,arg,DOUBLE(arg+SUM(Sales[V])))(LAMBDA(x,x+1),DOUBLE(1))",
                "LAMBDA(double,arg,DOUBLE(arg+SUM(Renamed[V])))(LAMBDA(x,x+1),Double(1))",
            ),
        ];
        let mut wb = book();
        wb.names.push(name("Double", "LAMBDA(x,x*2)"));
        wb.names.extend(cases.iter().enumerate().map(|(i, (f, _))| name(&format!("n{i}"), f)));
        wb.rename_table(0, 0, "Renamed");
        for (i, (before, after)) in cases.iter().enumerate() {
            assert_eq!(wb.names[i + 1].formula, *after, "{before}");
        }
    }

    #[test]
    fn rename_keeps_existing_cell_call_spelling() {
        let mut wb = book();
        let at = CellRef::new(5, 5);
        let formula = Formula::new("LET(double,LAMBDA(x,x+1),DOUBLE(SUM(Sales[V])))");
        wb.sheet_mut(0).unwrap().set_cell(at, Cell::formula(formula));
        // Defining a global Double later does not change the cell's existing local call.
        wb.names.push(name("Double", "LAMBDA(x,x*2)"));
        wb.rename_table(0, 0, "Renamed");
        let text = &wb.sheets[0].cell(at).unwrap().formula.as_ref().unwrap().text;
        assert_eq!(text, "LET(double,LAMBDA(x,x+1),DOUBLE(SUM(Renamed[V])))");
    }

    /// A native file caches each cell's tree parsed from its text, which upper-cases the calls.
    #[test]
    fn rename_keeps_lambda_call_spelling_of_a_cell_read_from_a_native_file() {
        let mut wb = book();
        wb.names.push(name("Double", "LAMBDA(x,x*2)"));
        let at = CellRef::new(5, 5);
        let formula: Formula = serde_json::from_str(r#"{"text":"Double(SUM(Sales[V]))","array":null}"#).unwrap();
        wb.sheet_mut(0).unwrap().set_cell(at, Cell::formula(formula));
        wb.rename_table(0, 0, "Renamed");
        let text = &wb.sheets[0].cell(at).unwrap().formula.as_ref().unwrap().text;
        assert_eq!(text, "Double(SUM(Renamed[V]))");
    }

    /// The interim X22 gate: interactive rename leaves formula objects as they are until Excel
    /// is measured, while cell formulas follow.
    #[test]
    fn rename_leaves_gated_formula_objects_alone() {
        let mut wb = book();
        let at = CellRef::new(5, 5);
        let sh = wb.sheet_mut(0).unwrap();
        sh.set_cell(at, Cell::formula(Formula::new("SUM(Sales[V])")));
        sh.cond_formats.push(CondFormat {
            ranges: vec![RangeRef::parse("A2:A3").unwrap()],
            rule: CfRule::Expression { formula: "SUM(Sales[V])>0".into(), style: Box::default() },
            priority: 1,
            stop_if_true: false,
        });
        sh.validations.push(Validation { f1: "SUM(Sales[V])".into(), ..Default::default() });
        wb.rename_table(0, 0, "Renamed");
        let sh = &wb.sheets[0];
        assert_eq!(sh.cell(at).and_then(|c| c.formula.as_ref()).map(|f| f.text.as_str()), Some("SUM(Renamed[V])"));
        assert!(matches!(&sh.cond_formats[0].rule, CfRule::Expression { formula, .. } if formula == "SUM(Sales[V])>0"));
        assert_eq!(sh.validations[0].f1, "SUM(Sales[V])");
    }
}
