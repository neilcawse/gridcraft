#!/usr/bin/env python3
"""Compare the slicers of two XLSX packages (typically a file and GridCraft's re-save of it).

Usage: table_slicer_package_diff.py BEFORE.xlsx AFTER.xlsx

Each package is reduced to what a slicer depends on, with ids resolved to names so renumbered
parts compare equal:

- every slicer cache the workbook relationships name: its name, `sourceName`, kind (table or
  PivotTable), the workbook `extLst` entry listing it, a table cache's table and column (resolved
  from `x15:tableSlicerCache` file ids to the table's `displayName` and the column's name) and item
  options, and whether a workbook-scoped `#N/A` defined name (its marker) exists;
- every slicer of every worksheet (by sheet name): its name, cache, caption, column count, style,
  button height and the sheet `extLst` entry listing its part;
- every slicer drawing frame: its name, compatibility choice, anchor type and edit mode, and
  its geometry (within one pixel). Cell positions use each package's own grid, assuming a 7 px
  maximum digit width; this is a comparison convention, not an Excel rendering measurement.
  Expressing an overflowing offset in the next row or column does not change the position.

Prints the reduced form of BEFORE and every difference; exits 1 when they differ, 2 when BEFORE has
no slicer.
"""

import posixpath
import sys
import xml.etree.ElementTree as ET
import zipfile

NS_REL = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
EMU_PER_PX = 9525
EMU_PER_PT = 12700
MAX_DIGIT_PX = 7


class Grid:
    """A worksheet's column widths and row heights in EMU."""

    def __init__(self, ws):
        fmt = first(ws, "sheetFormatPr") if ws is not None else None
        get = (lambda k, d: d) if fmt is None else (lambda k, d: float(fmt.get(k, d)))
        self.default_col = self.col_emu(get("defaultColWidth", get("baseColWidth", 8) + 5 / MAX_DIGIT_PX))
        self.default_row = round(get("defaultRowHeight", 15) * EMU_PER_PT)
        self.cols, self.rows = {}, {}
        cols = first(ws, "cols") if ws is not None else None
        for c in [] if cols is None else kids(cols, "col"):
            w = self.default_col if c.get("width") is None else self.col_emu(float(c.get("width")))
            for i in range(int(c.get("min", 1)) - 1, min(int(c.get("max", 1)), 16384)):
                self.cols[i] = 0 if c.get("hidden") in ("1", "true") else w
        data = first(ws, "sheetData") if ws is not None else None
        for r in [] if data is None else kids(data, "row"):
            if r.get("r") is not None:
                if r.get("hidden") in ("1", "true"):
                    self.rows[int(r.get("r")) - 1] = 0
                elif r.get("ht") is not None:
                    self.rows[int(r.get("r")) - 1] = round(float(r.get("ht")) * EMU_PER_PT)

    @staticmethod
    def col_emu(width):
        return int((256 * width + int(128 / MAX_DIGIT_PX)) / 256 * MAX_DIGIT_PX) * EMU_PER_PX

    def position(self, cell):
        result = {}
        for axis, sizes, default in (("col", self.cols, self.default_col), ("row", self.rows, self.default_row)):
            index = cell.get(axis, 0)
            result[axis] = index * default + sum(size - default for i, size in sizes.items() if i < index) + cell.get(f"{axis}Off", 0)
        return result


def local(tag):
    return tag.rsplit("}", 1)[-1]


def kids(el, name):
    return [k for k in el if local(k.tag) == name]


def first(el, name):
    return next(iter(kids(el, name)), None)


def descendants(el, name):
    return [d for d in el.iter() if local(d.tag) == name]


class Package:
    def __init__(self, path):
        self.zip = zipfile.ZipFile(path)
        self.names = set(self.zip.namelist())

    def xml(self, part):
        return ET.fromstring(self.zip.read(part)) if part in self.names else None

    def rels(self, part):
        d, f = posixpath.split(part)
        root = self.xml(posixpath.join(d, "_rels", f + ".rels"))
        out = {}
        for r in [] if root is None else kids(root, "Relationship"):
            if r.get("TargetMode") == "External":
                continue
            raw = r.get("Target", "")
            target = raw.lstrip("/") if raw.startswith("/") else posixpath.normpath(posixpath.join(d, raw))
            out[r.get("Id")] = (r.get("Type", "").rsplit("/", 1)[-1], target)
        return out


def ext_uris(root, list_name):
    """r:id → uri of the extLst entry listing it in a `list_name` element."""
    out = {}
    lst = first(root, "extLst")
    for ext in [] if lst is None else kids(lst, "ext"):
        for l in descendants(ext, list_name):
            for item in l:
                rid = item.get(f"{{{NS_REL}}}id")
                if rid:
                    out[rid] = ext.get("uri")
    return out


def reduce(path):
    """The package's slicer facts."""
    pkg = Package(path)
    wb_part = next((t for k, t in pkg.rels("").values() if k == "officeDocument"), "xl/workbook.xml")
    wb = pkg.xml(wb_part)
    wb_rels = pkg.rels(wb_part)
    markers = set()
    dn = first(wb, "definedNames")
    for d in [] if dn is None else kids(dn, "definedName"):
        if d.get("localSheetId") is None and (d.text or "").strip().lstrip("=").strip().upper() == "#N/A":
            markers.add(d.get("name", "").lower())

    # Worksheets, their tables (file id → displayName, column id → name) and parts.
    tables, sheets = {}, []
    for s in kids(first(wb, "sheets"), "sheet"):
        rel = wb_rels.get(s.get(f"{{{NS_REL}}}id"))
        if not rel or rel[0] != "worksheet":
            continue
        sheets.append((s.get("name"), rel[1]))
        for kind, target in pkg.rels(rel[1]).values():
            if kind == "table" and (t := pkg.xml(target)) is not None:
                cols = {c.get("id"): c.get("name") for c in descendants(t, "tableColumn")}
                tables[t.get("id")] = (t.get("displayName"), cols)

    caches = {}
    cache_ext = ext_uris(wb, "slicerCaches")
    for rid, (kind, target) in wb_rels.items():
        if kind != "slicerCache" or (c := pkg.xml(target)) is None:
            continue
        tsc = next(iter(descendants(c, "tableSlicerCache")), None)
        entry = {
            "sourceName": c.get("sourceName"),
            "kind": "table" if tsc is not None else "pivot",
            "listed_by": cache_ext.get(rid),
            "marker": c.get("name", "").lower() in markers,
        }
        if tsc is not None:
            name, cols = tables.get(tsc.get("tableId"), (None, {}))
            entry["table"] = name
            entry["column"] = cols.get(tsc.get("column"))
            entry["options"] = {k: tsc.get(k) for k in ("sortOrder", "customListSort", "crossFilter") if tsc.get(k) is not None}
        else:
            entry["pivotTables"] = sorted(p.get("name") for p in descendants(c, "pivotTable"))
        caches[c.get("name")] = entry

    slicers, frames = {}, {}
    for sheet_name, part in sheets:
        ws = pkg.xml(part)
        grid = Grid(ws)
        listed = ext_uris(ws, "slicerList")
        for rid, (kind, target) in pkg.rels(part).items():
            if kind == "slicer" and (x := pkg.xml(target)) is not None:
                for s in kids(x, "slicer"):
                    slicers[(sheet_name, s.get("name"))] = {
                        "cache": s.get("cache"),
                        "caption": s.get("caption"),
                        "columnCount": s.get("columnCount", "1"),
                        "style": s.get("style"),
                        "rowHeight": s.get("rowHeight", "241300"),
                        "listed_by": listed.get(rid),
                    }
            if kind == "drawing" and (d := pkg.xml(target)) is not None:
                for anchor in d:
                    for sl in descendants(anchor, "slicer"):
                        choice = next(iter(descendants(anchor, "Choice")), None)
                        geometry = {}
                        for edge in ("from", "to", "pos", "ext"):
                            e = first(anchor, edge)
                            if e is not None:
                                geometry[edge] = (grid.position({local(k.tag): int(k.text or 0) for k in e})
                                                  if edge in ("from", "to") else {k: int(v) for k, v in e.attrib.items()})
                        frames[(sheet_name, sl.get("name"))] = {
                            "requires": None if choice is None else choice.get("Requires"),
                            "type": local(anchor.tag),
                            "editAs": anchor.get("editAs", "twoCell") if local(anchor.tag) == "twoCellAnchor" else None,
                            "anchor": geometry,
                        }
    return {"caches": caches, "slicers": slicers, "frames": frames}


def same_anchor(a, b):
    if a.keys() != b.keys():
        return False
    for edge in a:
        if a[edge].keys() != b[edge].keys():
            return False
        for k in a[edge]:
            if abs(a[edge][k] - b[edge][k]) > EMU_PER_PX:
                return False
    return True


def main():
    if len(sys.argv) != 3:
        print(__doc__)
        return 64
    before = reduce(sys.argv[1])
    after = reduce(sys.argv[2])
    for section in ("caches", "slicers", "frames"):
        print(f"[{section}]")
        for k, v in sorted(before[section].items(), key=str):
            print(f"  {k}: {v}")
    if not before["slicers"]:
        print("BEFORE has no slicers")
        return 2
    diffs = []
    for section in ("caches", "slicers", "frames"):
        b, a = before[section], after[section]
        for k in sorted(set(b) | set(a), key=str):
            if k not in a:
                diffs.append(f"{section} {k}: lost")
            elif k not in b:
                diffs.append(f"{section} {k}: added {a[k]}")
            elif section == "frames":
                if any(b[k][f] != a[k][f] for f in ("requires", "type", "editAs")) or not same_anchor(b[k]["anchor"], a[k]["anchor"]):
                    diffs.append(f"{section} {k}: {b[k]} -> {a[k]}")
            elif b[k] != a[k]:
                diffs.append(f"{section} {k}: {b[k]} -> {a[k]}")
    for d in diffs:
        print("DIFF", d)
    summary = "SAME" if not diffs else f"{len(diffs)} differences"
    print(f"{len(before['slicers'])} slicers, {len(before['caches'])} caches: {summary}")
    return 1 if diffs else 0


if __name__ == "__main__":
    sys.exit(main())
