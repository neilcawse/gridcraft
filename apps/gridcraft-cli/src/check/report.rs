use super::fidelity::value_show;
use super::*;

// ---------------------------------------------------------------- output

/// The files whose cache is not trusted, with what was compared against it, per class.
#[derive(Default)]
struct Untrusted {
    files: usize,
    matched: usize,
    mismatched: usize,
}

#[derive(Default)]
struct Totals {
    files: usize,
    opened: usize,
    failed: usize,
    formula_cells: usize,
    checked: usize,
    matched: usize,
    mismatched: usize,
    locale_dependent: usize,
    no_oracle: usize,
    no_oracle_reasons: BTreeMap<&'static str, usize>,
    volatile: usize,
    volatile_via_names: usize,
    bool_as_number: usize,
    error_detail_cache: usize,
    whitespace_cache: usize,
    solver_tolerance: usize,
    cancellation_residue: usize,
    iteration_change: usize,
    stale_cell: usize,
    legacy_excel_version: usize,
    circular_reference: usize,
    untrusted: BTreeMap<&'static str, Untrusted>,
    round_trip_failed: usize,
    round_trip_differing: usize,
    categories: BTreeMap<&'static str, usize>,
    /// Files by the kind of producer the package names.
    producers: BTreeMap<&'static str, usize>,
    functions: BTreeMap<String, FnStat>,
    signatures: BTreeMap<String, usize>,
}

impl Totals {
    fn untrusted_matched(&self) -> usize {
        self.untrusted.values().map(|u| u.matched).sum()
    }
    fn untrusted_mismatched(&self) -> usize {
        self.untrusted.values().map(|u| u.mismatched).sum()
    }
}

fn totals(reports: &[FileReport]) -> Totals {
    let mut t = Totals { files: reports.len(), ..Default::default() };
    for r in reports {
        *t.producers.entry(r.producer.kind.as_str()).or_default() += 1;
        if r.error.is_some() {
            t.failed += 1;
            continue;
        }
        t.opened += 1;
        t.formula_cells += r.calc.formula_cells;
        t.checked += r.calc.checked;
        t.matched += r.calc.matched;
        t.mismatched += r.calc.mismatched;
        t.locale_dependent += r.calc.locale_dependent;
        t.no_oracle += r.calc.no_oracle;
        for (why, n) in &r.calc.no_oracle_reasons {
            *t.no_oracle_reasons.entry(*why).or_default() += n;
        }
        t.volatile += r.calc.volatile;
        t.volatile_via_names += r.calc.volatile_via_names;
        t.bool_as_number += r.calc.bool_as_number;
        t.error_detail_cache += r.calc.error_detail_cache;
        t.whitespace_cache += r.calc.whitespace_cache;
        t.solver_tolerance += r.calc.solver_tolerance;
        t.cancellation_residue += r.calc.cancellation_residue;
        t.iteration_change += r.calc.iteration_change;
        t.stale_cell += r.calc.stale_cell;
        t.legacy_excel_version += r.calc.legacy_excel_version;
        t.circular_reference += r.calc.circular_reference;
        if r.calc.cache_trust != CacheTrust::Trusted {
            let u = t.untrusted.entry(r.calc.cache_trust.as_str()).or_default();
            u.files += 1;
            u.matched += r.calc.untrusted_matched;
            u.mismatched += r.calc.untrusted_mismatched;
        }
        if r.round_trip.error.is_some() {
            t.round_trip_failed += 1;
        }
        if !r.round_trip.is_clean() {
            t.round_trip_differing += 1;
        }
        for c in r.round_trip.differing() {
            *t.categories.entry(c).or_default() += 1;
        }
        for (n, s) in &r.calc.functions {
            t.functions.entry(n.clone()).or_default().add(s);
        }
        for (sig, n) in &r.calc.signatures {
            *t.signatures.entry(sig.clone()).or_default() += n;
        }
    }
    t
}

/// Functions with mismatches, worst first: most primary mismatches, then most mismatches.
fn ranked(functions: &BTreeMap<String, FnStat>) -> Vec<(&String, &FnStat)> {
    let mut v: Vec<(&String, &FnStat)> = functions.iter().filter(|(_, s)| s.mismatched > 0).collect();
    v.sort_by(|a, b| b.1.primary.cmp(&a.1.primary).then(b.1.mismatched.cmp(&a.1.mismatched)).then(a.0.cmp(b.0)));
    v
}

fn functions_json(functions: &BTreeMap<String, FnStat>) -> Json {
    let mut v: Vec<(&String, &FnStat)> = functions.iter().collect();
    v.sort_by(|a, b| b.1.primary.cmp(&a.1.primary).then(b.1.mismatched.cmp(&a.1.mismatched)).then(a.0.cmp(b.0)));
    Json::Array(v.into_iter().map(|(n, s)| json!({"name": n, "checked": s.checked, "mismatched": s.mismatched, "primary": s.primary})).collect())
}

fn calc_props_json(p: &CalcProps) -> Json {
    json!({
        "mode": p.mode.as_str(), "calcCompleted": p.completed, "fullCalcOnLoad": p.full_calc_on_load, "iterate": p.iterate,
        "calcId": p.calc_id,
    })
}

fn producer_json(p: &Producer) -> Json {
    json!({"application": p.application, "version": p.version, "kind": p.kind.as_str()})
}

/// Per untrusted-cache class: how many files, and what was compared against it.
fn untrusted_json(t: &Totals) -> Json {
    let by_class: serde_json::Map<String, Json> = t
        .untrusted
        .iter()
        .map(|(class, u)| (class.to_string(), json!({"files": u.files, "matched": u.matched, "mismatched": u.mismatched})))
        .collect();
    Json::Object(by_class)
}

pub fn report_json(reports: &[FileReport], opts: &Options) -> Json {
    let files: Vec<Json> = reports
        .iter()
        .map(|r| {
            let c = &r.calc;
            let mismatches: Vec<Json> = c
                .mismatches
                .iter()
                .map(|m| {
                    json!({
                        "cell": m.cell, "formula": m.formula, "expected": value_json(&m.expected), "actual": value_json(&m.actual),
                        "kind": m.kind, "functions": m.functions, "primary": m.primary, "class": m.class.as_str(),
                    })
                })
                .collect();
            let categories: serde_json::Map<String, Json> = r
                .round_trip
                .categories
                .iter()
                .map(|cat| (cat.name.to_string(), json!({"equal": cat.diffs == 0, "diffs": cat.diffs, "examples": cat.examples})))
                .collect();
            let mut file = json!({
                "path": r.path,
                "opened": r.error.is_none(),
                "error": r.error,
                "warnings": r.warnings,
                "producer": producer_json(&r.producer),
                "calc": {
                    "formulaCells": c.formula_cells, "checked": c.checked, "matched": c.matched, "mismatched": c.mismatched,
                    "localeDependent": c.locale_dependent, "locale": c.locale,
                    "noOracle": c.no_oracle, "noOracleReasons": c.no_oracle_reasons, "staleCache": c.cache_trust != CacheTrust::Trusted,
                    "volatile": c.volatile, "volatileViaNames": c.volatile_via_names,
                    "boolAsNumber": c.bool_as_number, "errorDetailCache": c.error_detail_cache,
                    "whitespaceCache": c.whitespace_cache, "solverTolerance": c.solver_tolerance, "staleCell": c.stale_cell,
                    "cancellationResidue": c.cancellation_residue, "iterationChange": c.iteration_change,
                    "legacyExcelVersion": c.legacy_excel_version, "circularReference": c.circular_reference,
                    "cacheTrust": c.cache_trust.as_str(), "calcPr": calc_props_json(&c.calc_props),
                    "untrustedMatched": c.untrusted_matched, "untrustedMismatched": c.untrusted_mismatched,
                    "mismatches": mismatches, "functions": functions_json(&c.functions),
                    "signatures": c.signatures, "untrustedSignatures": c.untrusted_signatures,
                },
                "roundTrip": {"error": r.round_trip.error, "differing": r.round_trip.differing(), "categories": categories},
            });
            if opts.cells {
                let cells: serde_json::Map<String, Json> = c.cells.iter().map(|(cell, bucket)| (cell.clone(), Json::from(bucket.as_str()))).collect();
                file["calc"]["cells"] = Json::Object(cells);
            }
            file
        })
        .collect();
    let t = totals(reports);
    json!({
        "schema": SCHEMA,
        "tolerance": opts.tolerance,
        "files": files,
        "totals": {
            "files": t.files, "opened": t.opened, "failedToOpen": t.failed, "formulaCells": t.formula_cells, "checked": t.checked,
            "matched": t.matched, "mismatched": t.mismatched, "localeDependent": t.locale_dependent, "noOracle": t.no_oracle,
            "noOracleReasons": t.no_oracle_reasons,
            "volatile": t.volatile,
            "volatileViaNames": t.volatile_via_names, "boolAsNumber": t.bool_as_number, "errorDetailCache": t.error_detail_cache,
            "whitespaceCache": t.whitespace_cache, "solverTolerance": t.solver_tolerance, "staleCell": t.stale_cell,
            "cancellationResidue": t.cancellation_residue, "iterationChange": t.iteration_change,
            "legacyExcelVersion": t.legacy_excel_version, "circularReference": t.circular_reference,
            "untrustedMatched": t.untrusted_matched(), "untrustedMismatched": t.untrusted_mismatched(), "untrusted": untrusted_json(&t),
            "roundTripFailed": t.round_trip_failed, "roundTripDiffering": t.round_trip_differing, "categoriesDiffering": t.categories,
            "producers": t.producers,
        },
        "functions": functions_json(&t.functions),
        "signatures": t.signatures,
    })
}

/// One report line: line breaks and tabs shown escaped so that they stay visible as differences.
fn one_line(s: &str) -> String {
    s.replace('\n', "\\n").replace('\r', "\\r").replace('\t', "\\t")
}

/// `…tail` of a long label.
fn fit(s: &str, w: usize) -> String {
    let n = s.chars().count();
    if n <= w { s.to_string() } else { format!("…{}", s.chars().skip(n + 1 - w).collect::<String>()) }
}

/// `udf 3, downstream 12`: the causes of the formulas without an oracle, when any is more than a
/// missing cached value (the thing every producer that doesn't calculate leaves).
fn foreign_reasons(reasons: &BTreeMap<&'static str, usize>) -> Option<String> {
    if reasons.keys().all(|w| *w == foreign::why::NO_CACHED_VALUE) {
        return None;
    }
    Some(reasons.iter().map(|(w, n)| format!("{w} {n}")).collect::<Vec<_>>().join(", "))
}

/// The notes under a file's calc line: what was counted apart from the counts above them.
fn print_calc_notes(c: &CalcReport) {
    if let Some(why) = foreign_reasons(&c.no_oracle_reasons) {
        outln!("  (no oracle: {why})");
    }
    if c.volatile_via_names > 0 {
        outln!("  ({} of the volatile formulas reach the volatile value through a defined name)", c.volatile_via_names);
    }
    if c.cache_trust != CacheTrust::Trusted {
        outln!(
            "  ({}: {} of {} cells compared against it differ, none of them counted above)",
            c.cache_trust.reason(),
            c.untrusted_mismatched,
            c.untrusted_matched + c.untrusted_mismatched
        );
    }
    if c.stale_cell > 0 {
        outln!("  ({} checked cells are stale: {}; listed, not counted as mismatches)", c.stale_cell, CacheTrust::StaleCell.reason());
    }
    if c.legacy_excel_version > 0 {
        outln!(
            "  ({} checked cells are the older Excel's: {}; listed, not counted as mismatches)",
            c.legacy_excel_version,
            CacheTrust::LegacyExcelVersion.reason()
        );
    }
    if c.circular_reference > 0 {
        outln!(
            "  ({} checked cells are on or read a circular reference: {}; listed, not counted as mismatches)",
            c.circular_reference,
            CacheTrust::CircularReference.reason()
        );
    }
    if c.bool_as_number > 0 {
        outln!("  ({} matched TRUE/FALSE results were saved by the producer as 1/0, without t=\"b\")", c.bool_as_number);
    }
    if c.error_detail_cache > 0 {
        outln!("  ({} matched errors were saved as text with a reason after the code, `#N/A:lookupNotFound:…`)", c.error_detail_cache);
    }
    if c.whitespace_cache > 0 {
        outln!("  ({} matched texts differ from the cache in surrounding whitespace the producer didn't record)", c.whitespace_cache);
    }
    if c.solver_tolerance > 0 {
        outln!("  ({} matched solver results (IRR, XIRR, RATE…) agree to the solvers' accuracy, not to --tolerance)", c.solver_tolerance);
    }
    print_undecided_notes(c.cancellation_residue, c.iteration_change, "  (", ")");
    if c.locale_dependent > 0 {
        outln!(
            "  ({} matched results are the answers of an Excel set to {}, reproduced under its locale)",
            c.locale_dependent,
            c.locale.unwrap_or("another locale")
        );
    }
}

fn print_calc(c: &CalcReport) {
    outln!(
        "  formulas {}: {} checked, {} matched, {} mismatched, {} no oracle, {} volatile",
        c.formula_cells,
        c.checked,
        c.matched,
        c.mismatched,
        c.no_oracle,
        c.volatile
    );
    print_calc_notes(c);
    for m in &c.mismatches {
        let tag = match (m.class, m.primary) {
            (CacheTrust::Trusted, true) => "mismatch",
            (CacheTrust::Trusted, false) => "  (downstream)",
            (CacheTrust::StaleCell, true) => "stale cell",
            (CacheTrust::StaleCell, false) => "  (stale cell, downstream)",
            (CacheTrust::LegacyExcelVersion, true) => "older Excel",
            (CacheTrust::LegacyExcelVersion, false) => "  (older Excel, downstream)",
            (CacheTrust::CircularReference, true) => "circular reference",
            (CacheTrust::CircularReference, false) => "  (circular reference, downstream)",
            (_, true) => "untrusted mismatch",
            (_, false) => "  (untrusted, downstream)",
        };
        outln!(
            "  {tag} {}  {}  expected {}  got {}  [{}]",
            m.cell,
            one_line(&m.formula),
            value_show(&m.expected),
            value_show(&m.actual),
            m.functions.join(" ")
        );
    }
    let more = (c.mismatched + c.untrusted_mismatched).saturating_sub(c.mismatches.len());
    if more > 0 {
        outln!("  … {more} more mismatches");
    }
}

fn print_round_trip(rt: &RoundTrip) {
    if let Some(e) = &rt.error {
        outln!("  round trip: {e}");
    }
    let differing: Vec<&Category> = rt.categories.iter().filter(|c| c.diffs > 0).collect();
    if differing.is_empty() && rt.error.is_none() {
        outln!("  round trip: all equal");
    }
    for cat in differing {
        outln!("  round trip {}: {} difference(s)", cat.name, cat.diffs);
        for e in &cat.examples {
            outln!("    {}", one_line(e));
        }
        if cat.diffs > cat.examples.len() {
            outln!("    … {} more", cat.diffs - cat.examples.len());
        }
    }
}

fn print_file_reports(reports: &[FileReport]) {
    for r in reports {
        outln!("== {}", r.path);
        if let Some(e) = &r.error {
            outln!("  can't open: {e}");
            continue;
        }
        if r.producer.kind.is_foreign() {
            outln!("  producer: {} ({})", r.producer.application.as_deref().unwrap_or("no application named"), r.producer.kind.as_str());
        }
        print_calc(&r.calc);
        print_round_trip(&r.round_trip);
    }
}

fn print_file_table(reports: &[FileReport]) {
    let w = reports.iter().map(|r| r.path.chars().count()).max().unwrap_or(4).clamp(4, 50);
    outln!(
        "{:<w$}  {:>8}  {:>8}  {:>8}  {:>8}  {:>9}  {:>8}  {:>9}  round trip",
        "file",
        "formulas",
        "checked",
        "matched",
        "mismatch",
        "no-oracle",
        "volatile",
        "untrusted"
    );
    for r in reports {
        let label = fit(&r.path, w);
        if let Some(e) = &r.error {
            outln!("{label:<w$}  can't open: {}", one_line(&fit(e, 100)));
            continue;
        }
        let c = &r.calc;
        let rt = match (&r.round_trip.error, r.round_trip.differing()) {
            (Some(_), _) => "FAILED".to_string(),
            (None, d) if d.is_empty() => "ok".to_string(),
            (None, d) => d.join(","),
        };
        let untrusted = if c.cache_trust == CacheTrust::Trusted {
            "-".to_string()
        } else {
            format!("{}/{}", c.untrusted_mismatched, c.untrusted_matched + c.untrusted_mismatched)
        };
        outln!(
            "{label:<w$}  {:>8}  {:>8}  {:>8}  {:>8}  {:>9}  {:>8}  {untrusted:>9}  {rt}",
            c.formula_cells,
            c.checked,
            c.matched,
            c.mismatched,
            c.no_oracle,
            c.volatile
        );
    }
}

fn print_rankings(t: &Totals, opts: &Options) {
    let top = ranked(&t.functions);
    if !top.is_empty() {
        outln!("");
        outln!("top mismatching functions (primary = no mismatching precedent):");
        outln!("  {:<20}  {:>8}  {:>10}  {:>8}", "function", "primary", "mismatched", "checked");
        for (n, s) in top.iter().take(opts.max_report.max(1)) {
            outln!("  {:<20}  {:>8}  {:>10}  {:>8}", n, s.primary, s.mismatched, s.checked);
        }
    }
    let mut sigs: Vec<(&String, &usize)> = t.signatures.iter().collect();
    sigs.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    if !sigs.is_empty() {
        outln!("");
        outln!("mismatch signatures (expected → GridCraft):");
        for (sig, n) in sigs.iter().take(opts.max_report.max(1)) {
            outln!("  {n:>8}  {sig}");
        }
    }
}

/// The matches the calculation leaves undecided (see `noise`), each line between `open` and `close`.
fn print_undecided_notes(cancellation_residue: usize, iteration_change: usize, open: &str, close: &str) {
    if cancellation_residue > 0 {
        outln!("{open}{cancellation_residue} matched numbers are both cancellation residue, below 15 digits of what their formula reads{close}");
    }
    if iteration_change > 0 {
        outln!("{open}{iteration_change} matched numbers on an iterative loop agree within Maximum Change{close}");
    }
}

/// The totals of the trusted mismatches listed in a class of their own, not gating.
fn print_listed_totals(t: &Totals) {
    if t.stale_cell > 0 {
        outln!(
            "stale cells (manual mode, cache inconsistent with the file's own cached inputs): {} (listed, not counted as mismatches)",
            t.stale_cell
        );
    }
    if t.legacy_excel_version > 0 {
        outln!("cells an older Excel calculated with what it had: {} (listed, not counted as mismatches)", t.legacy_excel_version);
    }
    if t.circular_reference > 0 {
        outln!("cells on or reading a circular reference, iteration off: {} (listed, not counted as mismatches)", t.circular_reference);
    }
}

fn print_totals(t: &Totals) {
    outln!("");
    outln!("files: {} ({} opened, {} failed to open)", t.files, t.opened, t.failed);
    outln!(
        "formula cells: {} ({} checked: {} matched, {} mismatched; {} no oracle; {} volatile)",
        t.formula_cells,
        t.checked,
        t.matched,
        t.mismatched,
        t.no_oracle,
        t.volatile
    );
    if let Some(why) = foreign_reasons(&t.no_oracle_reasons) {
        outln!("formulas without an oracle, by cause: {why}");
    }
    if t.volatile_via_names > 0 {
        outln!("volatile through defined names: {}", t.volatile_via_names);
    }
    if t.bool_as_number > 0 {
        outln!("booleans saved as 1/0 by the producer: {} (counted as matched)", t.bool_as_number);
    }
    if t.error_detail_cache > 0 {
        outln!("errors saved as text with a reason after the code: {} (counted as matched)", t.error_detail_cache);
    }
    if t.whitespace_cache > 0 {
        outln!("texts equal to the cache but for surrounding whitespace it doesn't record: {} (counted as matched)", t.whitespace_cache);
    }
    if t.solver_tolerance > 0 {
        outln!("solver results matched to the solvers' accuracy: {} (counted as matched)", t.solver_tolerance);
    }
    print_undecided_notes(t.cancellation_residue, t.iteration_change, "", " (counted as matched)");
    print_listed_totals(t);
    if t.locale_dependent > 0 {
        outln!("results reproduced under the locale their cache was computed in: {} (counted as matched)", t.locale_dependent);
    }
    for (class, u) in &t.untrusted {
        outln!(
            "cache not trusted ({class}): {} files, {} cells compared, {} differ (counted apart, not as mismatches)",
            u.files,
            u.matched + u.mismatched,
            u.mismatched
        );
    }
    if t.producers.len() > 1 || t.producers.keys().any(|k| *k != ProducerKind::Excel.as_str()) {
        let by_kind: Vec<String> = t.producers.iter().map(|(k, n)| format!("{k} {n}")).collect();
        outln!("files by producer: {}", by_kind.join(", "));
    }
    let cats: Vec<String> = t.categories.iter().map(|(c, n)| format!("{c} {n}")).collect();
    outln!(
        "round trip: {} of {} files differ{}{}",
        t.round_trip_differing,
        t.opened,
        if t.round_trip_failed > 0 { format!(" ({} failed)", t.round_trip_failed) } else { String::new() },
        if cats.is_empty() { String::new() } else { format!("; categories (files): {}", cats.join(", ")) }
    );
}

pub(super) fn print_human(reports: &[FileReport], opts: &Options) {
    print_file_reports(reports);
    outln!("");
    print_file_table(reports);
    let t = totals(reports);
    print_rankings(&t, opts);
    print_totals(&t);
}
