//! Reading charts as Excel saved them: PivotChart series and deleted automatic titles.

use std::sync::Arc;

use gridcraft_core::{CellRef, Value};
use gridcraft_model::{Chart, ChartKind, Sheet, Workbook};

const NS: &str =
    r#"xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main""#;

/// Reads a chart part as part of `wb` (sheet 0): PivotChart series whose cells don't hold their
/// cached points plot the cache.
fn read_in(wb: &Workbook, xml: &str) -> Chart {
    let (mut c, caches) = crate::chart::read_chart(&crate::xml::parse(xml.as_bytes()).unwrap()).unwrap();
    crate::chart::plot_cached(wb, 0, &mut c, &caches);
    c
}

fn read(xml: &str) -> Chart {
    read_in(&Workbook::new(), xml)
}

/// A workbook with one sheet holding `cells` (A1 address, value).
fn book(name: &str, cells: &[(&str, Value)]) -> Workbook {
    let mut sheet = Sheet::new(name);
    for (a, v) in cells {
        sheet.set_value(CellRef::parse(a).unwrap(), v.clone());
    }
    let mut wb = Workbook::new();
    wb.sheets = vec![Arc::new(sheet)];
    wb
}

fn str_cache(f: &str, pts: &[&str]) -> String {
    let p: String = pts.iter().enumerate().map(|(i, v)| format!(r#"<c:pt idx="{i}"><c:v>{v}</c:v></c:pt>"#)).collect();
    format!(r#"<c:strRef><c:f>{f}</c:f><c:strCache><c:ptCount val="{}"/>{p}</c:strCache></c:strRef>"#, pts.len())
}

/// A numeric cache of `count` points; `pts` are (idx, value), so gaps can be left out.
fn num_cache(f: &str, count: usize, pts: &[(usize, &str)]) -> String {
    let p: String = pts.iter().map(|(i, v)| format!(r#"<c:pt idx="{i}"><c:v>{v}</c:v></c:pt>"#)).collect();
    format!(r#"<c:numRef><c:f>{f}</c:f><c:numCache><c:formatCode>General</c:formatCode><c:ptCount val="{count}"/>{p}</c:numCache></c:numRef>"#)
}

fn ser(tx: &str, cat: &str, val: &str) -> String {
    format!("<c:ser><c:idx val=\"0\"/><c:order val=\"0\"/><c:tx>{tx}</c:tx><c:cat>{cat}</c:cat><c:val>{val}</c:val></c:ser>")
}

fn chart_xml(pivot: bool, plot: &str, title: &str) -> String {
    let src = if pivot { "<c:pivotSource><c:name>[Book.xlsx]Graph!PivotTable1</c:name><c:fmtId val=\"0\"/></c:pivotSource>" } else { "" };
    format!(
        r#"<c:chartSpace {NS}>{src}<c:chart>{title}<c:plotArea>{plot}<c:catAx><c:axId val="1"/></c:catAx><c:valAx><c:axId val="2"/></c:valAx></c:plotArea></c:chart></c:chartSpace>"#
    )
}

#[test]
fn pivot_charts_plot_their_cached_points_when_the_references_do_not_cover_them() {
    // Excel points every formula of this combo PivotChart at the PivotTable's corner cell and
    // draws it from the PivotTable; the cached points are what it shows.
    let cats = ["Truck 1", "Van \"B\"", "Car"];
    let bar =
        ser(&str_cache("Graph!$J$2", &["Events"]), &str_cache("Graph!$J$2", &cats), &num_cache("Graph!$J$2", 3, &[(0, "18"), (1, "12"), (2, "5")]));
    // A point the cache leaves out stays a gap, so later points keep their positions.
    let line = ser(&str_cache("Graph!$J$2", &["Share"]), &str_cache("Graph!$J$2", &cats), &num_cache("Graph!$J$2", 3, &[(0, "5.3E-2"), (2, "0.5")]));
    let plot = format!(
        r#"<c:barChart><c:barDir val="col"/><c:grouping val="clustered"/>{bar}<c:axId val="1"/><c:axId val="2"/></c:barChart><c:lineChart><c:grouping val="standard"/>{line}<c:axId val="1"/><c:axId val="2"/></c:lineChart>"#
    );
    let c = read(&chart_xml(true, &plot, ""));
    assert_eq!(c.pivot.as_deref(), Some("Graph!PivotTable1"));
    assert_eq!(c.kind, ChartKind::Combo);
    let s = &c.series;
    assert_eq!(s[0].name.as_deref(), Some("\"Events\""));
    assert_eq!(s[0].categories.as_deref(), Some(r#"{"Truck 1","Van ""B""","Car"}"#));
    assert_eq!(s[0].values, "{18,12,5}");
    assert_eq!(s[1].name.as_deref(), Some("\"Share\""));
    assert_eq!(s[1].values, "{0.053,#N/A,0.5}");

    // The block Excel writes takes in the grand total row (4 cells for 3 points): the total is
    // not plotted.
    let totals = ser(
        &str_cache("Summary!$B$4", &["Total"]),
        &str_cache("Summary!$A$5:$A$8", &cats),
        &num_cache("Summary!$B$5:$B$8", 3, &[(0, "1010"), (1, "185.14"), (2, "120")]).replace("General", "\"$\"#,##0"),
    );
    let c = read(&chart_xml(true, &format!(r#"<c:pieChart>{totals}</c:pieChart>"#), ""));
    assert_eq!(c.series[0].values, "{1010,185.14,120}");
    assert_eq!(c.series[0].categories.as_deref(), Some(r#"{"Truck 1","Van ""B""","Car"}"#));
    // The cached points keep the cache's number format (data labels show "$185"), and so does
    // a save.
    assert_eq!(c.series[0].number_format.as_deref(), Some("\"$\"#,##0"));
    let back = read(&crate::chart::write_chart(&Workbook::new(), 0, &c, ""));
    assert_eq!(back.series[0].number_format, c.series[0].number_format);

    // References whose cells hold the cached points stay live, and so do an ordinary chart's.
    let live = ser(&str_cache("Summary!$B$4", &["Total"]), &str_cache("Summary!$A$5:$A$7", &cats), &num_cache("Summary!$B$5:$B$7", 3, &[(0, "1")]));
    let mut cells = vec![("B4", Value::text("Total")), ("B5", Value::Number(1.0))];
    cells.extend(["A5", "A6", "A7"].into_iter().zip(cats).map(|(a, t)| (a, Value::text(t))));
    let c = read_in(&book("Summary", &cells), &chart_xml(true, &format!(r#"<c:pieChart>{live}</c:pieChart>"#), ""));
    assert_eq!((c.series[0].name.as_deref(), c.series[0].values.as_str()), (Some("Summary!$B$4"), "Summary!$B$5:$B$7"));
    assert_eq!(c.series[0].number_format, None, "live references use their cells' format");
    let plain = ser(&str_cache("Graph!$J$2", &["Events"]), &str_cache("Graph!$J$2", &cats), &num_cache("Graph!$J$2", 3, &[(0, "18")]));
    let c = read(&chart_xml(false, &format!(r#"<c:pieChart>{plain}</c:pieChart>"#), ""));
    assert_eq!(c.series[0].values, "Graph!$J$2");

    // Saved again, the cached points come back as they were: quotes, gaps and all. (A literal
    // name is saved as plain text, which names the series the same way.)
    let c = read(&chart_xml(true, &plot, ""));
    let back = read(&crate::chart::write_chart(&Workbook::new(), 0, &c, ""));
    assert_eq!(back.pivot, c.pivot);
    let parts = |c: &Chart| c.series.iter().map(|s| (shown(s.name.as_deref()), s.categories.clone(), s.values.clone())).collect::<Vec<_>>();
    assert_eq!(parts(&back), parts(&c));
}

#[test]
fn a_one_point_pivot_chart_at_the_corner_cell_plots_its_cache() {
    // A PivotTable with one row item: every formula points at the corner cell, which holds the
    // table's header, and each cache has one point, like the cell count. Excel plots the cache.
    let bar = ser(&str_cache("Data!$J$2", &["Events"]), &str_cache("Data!$J$2", &["Truck 1"]), &num_cache("Data!$J$2", 1, &[(0, "18")]));
    let chart = chart_xml(true, &format!(r#"<c:barChart><c:barDir val="col"/>{bar}<c:axId val="1"/><c:axId val="2"/></c:barChart>"#), "");
    let drawing = r#"<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><xdr:oneCellAnchor><xdr:from><xdr:col>0</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>4</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:ext cx="4572000" cy="2743200"/><xdr:graphicFrame><xdr:nvGraphicFramePr><xdr:cNvPr id="2" name="C"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr><xdr:xfrm/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" r:id="rId1"/></a:graphicData></a:graphic></xdr:graphicFrame><xdr:clientData/></xdr:oneCellAnchor></xdr:wsDr>"#;
    let rel = |kind: &str, target: &str| {
        format!(
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/{kind}" Target="{target}"/></Relationships>"#
        )
    };
    let (srels, drels) = (rel("drawing", "../drawings/drawing1.xml"), rel("chart", "../charts/chart1.xml"));
    let body = r#"<sheetData><row r="2"><c r="J2" t="inlineStr"><is><t>Sum of Events</t></is></c></row></sheetData><drawing r:id="rId1"/>"#;
    let parts = [
        ("xl/worksheets/_rels/sheet1.xml.rels", srels.as_str()),
        ("xl/drawings/drawing1.xml", drawing),
        ("xl/drawings/_rels/drawing1.xml.rels", drels.as_str()),
        ("xl/charts/chart1.xml", chart.as_str()),
    ];
    let (wb, _) = crate::read_xlsx(&super::minimal(body, &parts, "", "")).unwrap();
    let s = &wb.sheets[0].charts[0].series[0];
    assert_eq!((s.name.as_deref(), s.categories.as_deref(), s.values.as_str()), (Some("\"Events\""), Some(r#"{"Truck 1"}"#), "{18}"));
}

#[test]
fn multi_level_pivot_categories_plot_their_innermost_level() {
    // Two levels of row labels in A4:B7, the outer one (country) on the left; the cache keeps
    // the innermost level first.
    let cities = ["Ottawa", "Toronto", "Boston", "NYC"];
    let leaf: String = cities.iter().enumerate().map(|(i, v)| format!(r#"<c:pt idx="{i}"><c:v>{v}</c:v></c:pt>"#)).collect();
    let outer = r#"<c:pt idx="0"><c:v>Canada</c:v></c:pt><c:pt idx="2"><c:v>USA</c:v></c:pt>"#;
    let xml = |f: &str, levels: &str| {
        let cat =
            format!(r#"<c:multiLvlStrRef><c:f>{f}</c:f><c:multiLvlStrCache><c:ptCount val="4"/>{levels}</c:multiLvlStrCache></c:multiLvlStrRef>"#);
        let val = num_cache("Graph!$C$4:$C$7", 4, &[(0, "1"), (1, "2"), (2, "3"), (3, "4")]);
        chart_xml(
            true,
            &format!(
                r#"<c:barChart><c:barDir val="col"/>{}<c:axId val="1"/><c:axId val="2"/></c:barChart>"#,
                ser(&str_cache("Graph!$C$3", &["Total"]), &cat, &val)
            ),
            "",
        )
    };
    let mut cells = vec![("A4", Value::text("Canada")), ("A6", Value::text("USA")), ("C3", Value::text("Total"))];
    for (i, city) in cities.iter().enumerate() {
        cells.push((["B4", "B5", "B6", "B7"][i], Value::text(*city)));
        cells.push((["C4", "C5", "C6", "C7"][i], Value::Number(i as f64 + 1.0)));
    }
    let wb = book("Graph", &cells);
    // The innermost column of the two-level block holds the cache: the series stays live, its
    // references (categories and name) kept as saved, and only the innermost level is drawn.
    let two_levels = xml("Graph!$A$4:$B$7", &format!("<c:lvl>{leaf}</c:lvl><c:lvl>{outer}</c:lvl>"));
    let c = read_in(&wb, &two_levels);
    let s = &c.series[0];
    assert_eq!(
        (s.name.as_deref(), s.categories.as_deref(), s.values.as_str(), s.innermost_categories),
        (Some("Graph!$C$3"), Some("Graph!$A$4:$B$7"), "Graph!$C$4:$C$7", true)
    );
    // A save keeps the references and writes the block as a `multiLvlStrRef` whose cache has
    // both levels, innermost first, so it reads back the same way.
    let saved = crate::chart::write_chart(&wb, 0, &c, "");
    let cat = format!(
        r#"<c:cat><c:multiLvlStrRef><c:f>Graph!$A$4:$B$7</c:f><c:multiLvlStrCache><c:ptCount val="4"/><c:lvl>{leaf}</c:lvl><c:lvl>{outer}</c:lvl></c:multiLvlStrCache></c:multiLvlStrRef></c:cat>"#
    );
    assert!(saved.contains(&cat) && saved.contains("<c:f>Graph!$C$3</c:f>"), "{saved}");
    let back = read_in(&wb, &saved).series[0].clone();
    assert_eq!((back.name, back.categories, back.innermost_categories), (s.name.clone(), s.categories.clone(), true));

    // When the innermost column shows another label, the block is stale: the innermost level
    // is plotted from the cache, one label per point.
    let labels = r#"{"Ottawa","Toronto","Boston","NYC"}"#;
    let mut moved = cells.clone();
    moved.push(("B5", Value::text("Montreal")));
    let s = read_in(&book("Graph", &moved), &two_levels).series[0].clone();
    assert_eq!((s.categories.as_deref(), s.innermost_categories), (Some(labels), false));

    // A one-level reference whose cells hold the cache stays live; when a cell shows another
    // label, the cache is plotted.
    let one_level = xml("Graph!$B$4:$B$7", &format!("<c:lvl>{leaf}</c:lvl>"));
    assert_eq!(read_in(&wb, &one_level).series[0].categories.as_deref(), Some("Graph!$B$4:$B$7"));
    assert_eq!(read_in(&book("Graph", &moved), &one_level).series[0].categories.as_deref(), Some(labels));
}

/// A series name as the chart shows it: a quoted literal without its quotes, `""` read as `"`.
fn shown(name: Option<&str>) -> Option<String> {
    name.map(|n| n.strip_prefix('"').and_then(|q| q.strip_suffix('"')).map_or_else(|| n.to_string(), |q| q.replace("\"\"", "\"")))
}

#[test]
fn series_names_with_quotes_survive_a_save() {
    // A stale PivotChart series takes its cached name, quotes and all.
    let s =
        ser(&str_cache("Graph!$J$2", &["Sales \"East\""]), &str_cache("Graph!$J$2", &["a", "b"]), &num_cache("Graph!$J$2", 2, &[(0, "1"), (1, "2")]));
    let c = read(&chart_xml(true, &format!("<c:pieChart>{s}</c:pieChart>"), ""));
    assert_eq!(shown(c.series[0].name.as_deref()).as_deref(), Some("Sales \"East\""));
    let back = read(&crate::chart::write_chart(&Workbook::new(), 0, &c, ""));
    assert_eq!(shown(back.series[0].name.as_deref()).as_deref(), Some("Sales \"East\""));

    // So does a literal name on a chartex chart.
    let mut c = Chart { kind: ChartKind::Waterfall, ..c };
    c.series[0].categories = None;
    let mut names = Vec::new();
    let xml = crate::chartex::write_chartex(&Workbook::new(), 0, &c, &mut names);
    let back = crate::chartex::read_chartex(&crate::xml::parse(xml.as_bytes()).unwrap()).unwrap();
    assert_eq!(shown(back.series[0].name.as_deref()).as_deref(), Some("Sales \"East\""));
}

#[test]
fn literal_series_keep_their_points_without_a_valid_pt_count() {
    // Not valid by the schema, but readable: the points present set the count, and points
    // without an index go in document order.
    let values = |lit: &str| {
        let s = format!("<c:ser><c:idx val=\"0\"/><c:order val=\"0\"/><c:val><c:numLit>{lit}</c:numLit></c:val></c:ser>");
        read(&chart_xml(false, &format!("<c:pieChart>{s}</c:pieChart>"), "")).series[0].values.clone()
    };
    let pts = r#"<c:pt idx="0"><c:v>1</c:v></c:pt><c:pt idx="1"><c:v>2</c:v></c:pt>"#;
    assert_eq!(values(&format!(r#"<c:ptCount val="x"/>{pts}"#)), "{1,2}");
    assert_eq!(values(&format!(r#"<c:ptCount val="0"/>{pts}"#)), "{1,2}");
    assert_eq!(values(&format!(r#"<c:ptCount val="1"/>{pts}"#)), "{1,2}");
    assert_eq!(values("<c:pt><c:v>1</c:v></c:pt><c:pt><c:v>2</c:v></c:pt>"), "{1,2}");
    // A valid count still keeps trailing gaps.
    assert_eq!(values(&format!(r#"<c:ptCount val="3"/>{pts}"#)), "{1,2,#N/A}");
}

#[test]
fn chart_cache_counts_and_series_are_bounded() {
    let sparse = r#"<c:ser><c:idx val="0"/><c:order val="0"/><c:val><c:numLit><c:ptCount val="40"/><c:pt idx="0"><c:v>1</c:v></c:pt><c:pt idx="9"><c:v>10</c:v></c:pt></c:numLit></c:val></c:ser>"#;
    let chart = read(&chart_xml(false, &format!("<c:pieChart>{sparse}</c:pieChart>"), ""));
    assert_eq!(chart.series[0].values.split(',').count(), 40);

    let huge = r#"<c:ser><c:idx val="0"/><c:order val="0"/><c:val><c:numLit><c:ptCount val="2000000"/><c:pt idx="1"><c:v>2</c:v></c:pt></c:numLit></c:val></c:ser>"#;
    let chart = read(&chart_xml(false, &format!("<c:pieChart>{huge}</c:pieChart>"), ""));
    assert_eq!(chart.series[0].values.split(',').count(), gridcraft_core::MAX_ROWS as usize);

    let series: String = (0..300)
        .map(|i| format!(r#"<c:ser><c:idx val="{i}"/><c:order val="{i}"/><c:val><c:numLit><c:ptCount val="1"/><c:pt idx="0"><c:v>{i}</c:v></c:pt></c:numLit></c:val></c:ser>"#))
        .collect();
    let chart = read(&chart_xml(false, &format!("<c:pieChart>{series}</c:pieChart>"), ""));
    assert_eq!(chart.series.len(), 300);
}

#[test]
fn chart_cache_budget_bounds_many_empty_series() {
    let series: String = (0..1_000)
        .map(|i| format!(r#"<c:ser><c:idx val="{i}"/><c:order val="{i}"/><c:val><c:numLit><c:ptCount val="1048576"/></c:numLit></c:val></c:ser>"#))
        .collect();
    let root = crate::xml::parse(chart_xml(false, &format!("<c:pieChart>{series}</c:pieChart>"), "").as_bytes()).unwrap();
    let mut budget = crate::chart::ChartReadBudget::new();
    let (chart, _) = crate::chart::read_chart_with_budget(&root, &mut budget).unwrap();
    assert_eq!(chart.series.len(), 1_000);
    assert!(chart.series.iter().map(|s| s.values.split(',').count()).sum::<usize>() <= 16_777_216 + chart.series.len());
    assert!(budget.exceeded);
}

#[test]
fn chart_cache_budget_drops_points_beyond_boundary() {
    let series: String = (0..17)
        .map(|i| format!(r#"<c:ser><c:idx val="{i}"/><c:order val="{i}"/><c:val><c:numLit><c:ptCount val="1048576"/><c:pt idx="1048575"><c:v>{i}</c:v></c:pt></c:numLit></c:val></c:ser>"#))
        .collect();
    let root = crate::xml::parse(chart_xml(false, &format!("<c:pieChart>{series}</c:pieChart>"), "").as_bytes()).unwrap();
    let mut budget = crate::chart::ChartReadBudget::new();
    let (chart, _) = crate::chart::read_chart_with_budget(&root, &mut budget).unwrap();
    assert_eq!(chart.series.len(), 17);
    assert!(chart.series.iter().take(16).all(|s| !s.values.is_empty()));
    assert_eq!(chart.series[16].values, "");
    assert!(budget.exceeded);
}

#[test]
fn a_deleted_automatic_title_stays_deleted() {
    let one = ser("<c:v>Vehicles</c:v>", &str_cache("S!$A$1:$A$2", &["a", "b"]), &num_cache("S!$B$1:$B$2", 2, &[]));
    let pie = format!("<c:pieChart>{one}</c:pieChart>");
    // A lone series names an automatic title unless the file turned it off.
    assert_eq!(read(&chart_xml(false, &pie, "")).title, None);
    assert_eq!(read(&chart_xml(false, &pie, r#"<c:autoTitleDeleted val="0"/>"#)).title, None);
    let deleted = read(&chart_xml(false, &pie, r#"<c:autoTitleDeleted val="1"/>"#));
    assert_eq!(deleted.title.as_deref(), Some(""));
    // An explicit title wins over the flag.
    let t = r#"<c:title><c:tx><c:rich><a:p><a:r><a:t>Risk</a:t></a:r></a:p></c:rich></c:tx></c:title><c:autoTitleDeleted val="1"/>"#;
    assert_eq!(read(&chart_xml(false, &pie, t)).title.as_deref(), Some("Risk"));

    // Both states survive a save.
    let wb = Workbook::new();
    assert_eq!(read(&crate::chart::write_chart(&wb, 0, &deleted, "")).title.as_deref(), Some(""));
    let auto = read(&chart_xml(false, &pie, ""));
    assert_eq!(read(&crate::chart::write_chart(&wb, 0, &auto, "")).title, None);
}

#[test]
fn dpt_scheme_lum_mod_off_and_outlines_round_trip() {
    use gridcraft_model::Color;

    let dpts = r#"
      <c:dPt><c:idx val="0"/><c:spPr>
        <a:solidFill><a:schemeClr val="accent2"><a:lumMod val="40000"/><a:lumOff val="60000"/></a:schemeClr></a:solidFill>
        <a:ln w="19050"><a:solidFill><a:schemeClr val="accent2"/></a:solidFill></a:ln>
      </c:spPr></c:dPt>
      <c:dPt><c:idx val="1"/><c:spPr>
        <a:solidFill><a:srgbClr val="804020"><a:shade val="75000"/></a:srgbClr></a:solidFill>
        <a:ln w="12700"><a:solidFill><a:srgbClr val="112233"/></a:solidFill></a:ln>
      </c:spPr></c:dPt>
      <c:dPt><c:idx val="2"/><c:spPr><a:noFill/><a:ln><a:noFill/></a:ln></c:spPr></c:dPt>
    "#;
    let ser_xml = format!(
        r#"<c:ser><c:idx val="0"/><c:order val="0"/>{dpts}<c:val><c:numLit><c:ptCount val="3"/><c:pt idx="0"><c:v>8</c:v></c:pt><c:pt idx="1"><c:v>7</c:v></c:pt><c:pt idx="2"><c:v>5</c:v></c:pt></c:numLit></c:val></c:ser>"#
    );
    let c = read(&chart_xml(false, &format!("<c:doughnutChart>{ser_xml}<c:holeSize val=\"65\"/></c:doughnutChart>"), ""));
    let pts = &c.series[0].points;
    assert_eq!(pts.len(), 3);
    assert_eq!(pts[0].fill, Some(Color::Theme(5, 600)));
    assert_eq!(pts[0].line_color, Some(Color::Theme(5, 0)));
    assert_eq!(pts[0].line_width, Some(1.5));
    assert!(matches!(pts[1].fill, Some(Color::Rgb(v)) if v != 0x804020), "shade applied to srgbClr");
    assert_eq!(pts[1].line_color, Some(Color::Rgb(0x112233)));
    assert_eq!(pts[1].line_width, Some(1.0));
    assert!(pts[2].no_fill && pts[2].no_line);

    let wb = Workbook::new();
    let written = crate::chart::write_chart(&wb, 0, &c, "Book.xlsx");
    let back = read(&written);
    assert_eq!(back.series[0].points.len(), 3);
    assert_eq!(back.series[0].points[0].line_width, Some(1.5));
    assert!(back.series[0].points[2].no_fill && back.series[0].points[2].no_line);
    assert_eq!(back.series[0].points[0].fill.and_then(|x| x.resolve(&wb.theme)), Color::Theme(5, 600).resolve(&wb.theme));
}

#[test]
fn dlbls_percent_cat_leader_delete_and_per_point_overrides_round_trip() {
    use gridcraft_model::DataLabelPos;

    let ser0 = r#"<c:ser><c:idx val="0"/><c:order val="0"/>
      <c:dLbls>
        <c:dLbl><c:idx val="1"/><c:delete val="1"/></c:dLbl>
        <c:dLbl><c:idx val="2"/><c:tx><c:rich><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Top</a:t></a:r></a:p></c:rich></c:tx></c:dLbl>
        <c:numFmt formatCode="[$-409]0.0%;[$-409];[$-409];[$-409]" sourceLinked="0"/>
        <c:dLblPos val="bestFit"/>
        <c:showVal val="0"/><c:showCatName val="1"/><c:showPercent val="1"/><c:showLeaderLines val="1"/>
        <c:separator> | </c:separator>
      </c:dLbls>
      <c:val><c:numLit><c:ptCount val="3"/><c:pt idx="0"><c:v>4</c:v></c:pt><c:pt idx="1"><c:v>3</c:v></c:pt><c:pt idx="2"><c:v>3</c:v></c:pt></c:numLit></c:val>
    </c:ser>"#;
    let c = read(&chart_xml(false, &format!("<c:pieChart>{ser0}</c:pieChart>"), ""));
    assert!(c.data_labels);
    let dl = c.series[0].data_labels.as_ref().unwrap();
    assert!(!dl.show_val && dl.show_percent && dl.show_cat_name && dl.show_leader_lines);
    assert_eq!(dl.pos, DataLabelPos::BestFit);
    assert_eq!(dl.separator.as_deref(), Some(" | "));
    assert_eq!(dl.num_fmt.as_deref(), Some("[$-409]0.0%;[$-409];[$-409];[$-409]"));
    assert_eq!(dl.points.len(), 2);
    assert!(dl.points[0].delete && dl.points[0].idx == 1);
    assert_eq!(dl.points[1].text.as_deref(), Some("Top"));

    let written = crate::chart::write_chart(&Workbook::new(), 0, &c, "Book.xlsx");
    let back = read(&written);
    assert_eq!(back.series[0].data_labels, c.series[0].data_labels);

    // Series-level <c:dLbls><c:delete val="1"/></c:dLbls> overrides group showVal="1".
    let deleted = r#"<c:doughnutChart>
      <c:ser><c:idx val="0"/><c:order val="0"/><c:dLbls><c:delete val="1"/></c:dLbls>
        <c:val><c:numLit><c:ptCount val="2"/><c:pt idx="0"><c:v>1</c:v></c:pt><c:pt idx="1"><c:v>2</c:v></c:pt></c:numLit></c:val>
      </c:ser>
      <c:dLbls><c:showVal val="1"/></c:dLbls>
    </c:doughnutChart>"#;
    let cd = read(&chart_xml(false, deleted, ""));
    assert!(!cd.data_labels);
    assert!(cd.series[0].data_labels.as_ref().unwrap().delete);
}

#[test]
fn chart_space_sp_pr_no_fill_and_border_round_trip() {
    use gridcraft_model::Color;

    let make = |sp: &str| {
        format!(
            r#"<c:chartSpace {NS}><c:chart><c:plotArea><c:pieChart><c:ser><c:idx val="0"/><c:order val="0"/><c:val><c:numLit><c:ptCount val="1"/><c:pt idx="0"><c:v>5</c:v></c:pt></c:numLit></c:val></c:ser></c:pieChart></c:plotArea></c:chart>{sp}</c:chartSpace>"#
        )
    };
    let wb = Workbook::new();

    let c_none = read(&make("<c:spPr><a:noFill/><a:ln><a:noFill/></a:ln></c:spPr>"));
    assert!(c_none.area_style.no_fill && c_none.area_style.no_line);
    let back_none = read(&crate::chart::write_chart(&wb, 0, &c_none, ""));
    assert!(back_none.area_style.no_fill && back_none.area_style.no_line);

    let c_borderless = read(&make("<c:spPr><a:ln><a:noFill/></a:ln></c:spPr>"));
    assert!(!c_borderless.area_style.no_fill && c_borderless.area_style.no_line);

    let c_styled = read(&make(
        r#"<c:spPr><a:solidFill><a:srgbClr val="F0F4F8"/></a:solidFill><a:ln w="25400"><a:solidFill><a:srgbClr val="336699"/></a:solidFill></a:ln></c:spPr>"#,
    ));
    assert_eq!(c_styled.area_style.fill, Some(Color::Rgb(0xF0F4F8)));
    assert_eq!(c_styled.area_style.line_color, Some(Color::Rgb(0x336699)));
    assert_eq!(c_styled.area_style.line_width, Some(2.0));
    let back_styled = read(&crate::chart::write_chart(&wb, 0, &c_styled, ""));
    assert_eq!(back_styled.area_style, c_styled.area_style);
}

#[test]
fn parse_user_shapes_handles_rel_abs_txlink_and_fld_text() {
    use gridcraft_model::{Color, ShapeTextAlign, ShapeTextAnchor};

    let xml = r#"<cdr:userShapes xmlns:cdr="http://schemas.openxmlformats.org/drawingml/2006/chartDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
      <cdr:relSizeAnchor>
        <cdr:from><cdr:x>0.2</cdr:x><cdr:y>0.3</cdr:y></cdr:from>
        <cdr:to><cdr:x>0.8</cdr:x><cdr:y>0.6</cdr:y></cdr:to>
        <cdr:sp textlink="'Summary'!$D$13">
          <cdr:spPr><a:noFill/><a:ln><a:noFill/></a:ln></cdr:spPr>
          <cdr:txBody>
            <a:bodyPr anchor="ctr"/>
            <a:p><a:pPr algn="ctr"/><a:fld type="TxLink"><a:rPr sz="2400" b="1"><a:solidFill><a:srgbClr val="112233"/></a:solidFill></a:rPr><a:t>86.0</a:t></a:fld></a:p>
          </cdr:txBody>
        </cdr:sp>
      </cdr:relSizeAnchor>
      <cdr:absSizeAnchor>
        <cdr:from><cdr:x>0.1</cdr:x><cdr:y>0.1</cdr:y></cdr:from>
        <cdr:ext cx="952500" cy="476250"/>
        <cdr:sp>
          <cdr:spPr><a:solidFill><a:srgbClr val="FFFCEB"/></a:solidFill><a:ln w="12700"><a:solidFill><a:srgbClr val="888888"/></a:solidFill></a:ln></cdr:spPr>
          <cdr:txBody><a:bodyPr anchor="t"/><a:p><a:pPr algn="l"/><a:r><a:rPr sz="1000"/><a:t>Note</a:t></a:r></a:p></cdr:txBody>
        </cdr:sp>
      </cdr:absSizeAnchor>
    </cdr:userShapes>"#;
    let root = crate::xml::parse(xml.as_bytes()).unwrap();
    let shapes = crate::chart::parse_user_shapes(&root);
    assert_eq!(shapes.len(), 2);
    assert!((shapes[0].x - 0.2).abs() < 1e-4 && (shapes[0].w - 0.6).abs() < 1e-4);
    assert_eq!(shapes[0].text, "86.0");
    assert_eq!(shapes[0].text_link.as_deref(), Some("'Summary'!$D$13"));
    assert!(shapes[0].no_fill && shapes[0].no_line && shapes[0].bold);
    assert_eq!(shapes[0].font_size, Some(24.0));
    assert_eq!(shapes[0].text_color, Some(Color::Rgb(0x112233)));
    assert_eq!(shapes[0].align, Some(ShapeTextAlign::Center));
    assert_eq!(shapes[0].anchor, Some(ShapeTextAnchor::Middle));
    assert_eq!(shapes[1].text, "Note");
    assert_eq!(shapes[1].fill, Some(Color::Rgb(0xFFFCEB)));
    assert_eq!(shapes[1].line_color, Some(Color::Rgb(0x888888)));
    assert_eq!(shapes[1].line_width, Some(1.0));
    assert!(shapes[1].abs_width.is_some_and(|w| (w - 100.0).abs() < 1.0));
    assert!(shapes[1].abs_height.is_some_and(|h| (h - 50.0).abs() < 1.0));
}
