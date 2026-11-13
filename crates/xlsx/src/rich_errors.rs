//! Errors that are rich values. Excel writes `#SPILL!` and `#CALC!` (the errors dynamic arrays
//! added) as an ordinary error cell, `t="e"` holding `#VALUE!`, plus a `vm` that points at a rich
//! value of structure `_error`; the value's `errorType` says which error the cell shows.
//! `vm` and metadata type IDs are one-based; every other table index is zero-based.

use gridcraft_core::CellError;

use crate::package::Rel;
use crate::read::Ctx;
use crate::xml::{self, El};

const RICH_EXT: &str = "{3E2802C4-A4D2-4D8B-9148-E3BE6C30E623}";
const MAX_RECORDS: usize = 100_000;
const MAX_XML: u64 = 16 * 1024 * 1024;

/// The error of a rich value's `errorType`. Excel numbers them as `ERROR.TYPE` does, less one;
/// the two files give `8` for `#SPILL!` and `13` for `#CALC!`. Others are left to the cell's
/// `<v>` text.
fn error_of(error_type: u32) -> Option<CellError> {
    match error_type {
        8 => Some(CellError::Spill),
        13 => Some(CellError::Calc),
        _ => None,
    }
}

fn part(rels: &[Rel], kind: &str) -> Option<String> {
    rels.iter().find(|rel| rel.kind == kind && !rel.external).map(|rel| rel.target.clone())
}

fn read_part(cx: &mut Ctx<'_>, path: &str, root: &str) -> Option<El> {
    let bytes = cx.pkg.read_limited(path, MAX_XML).ok().flatten()?;
    xml::parse(&bytes).ok().filter(|el| el.name == root)
}

/// The error each value-metadata block stands for (index `vm - 1`), `None` for one that is not an
/// error rich value. Empty when the workbook has no rich values: a part that is missing or
/// damaged makes the cells keep the `#VALUE!` they were saved with, and cell pictures report it.
pub fn read(cx: &mut Ctx<'_>, workbook_rels: &[Rel]) -> Vec<Option<CellError>> {
    read_inner(cx, workbook_rels).unwrap_or_default()
}

fn read_inner(cx: &mut Ctx<'_>, workbook_rels: &[Rel]) -> Option<Vec<Option<CellError>>> {
    let metadata_path = part(workbook_rels, "sheetMetadata")?;
    let metadata = read_part(cx, &metadata_path, "metadata")?;
    let types: Vec<&El> = metadata.child("metadataTypes")?.kids("metadataType").take(MAX_RECORDS).collect();
    let mut rich = types.iter().enumerate().filter(|(_, el)| el.attr("name") == Some("XLRICHVALUE")).map(|(i, _)| i + 1);
    let type_id = rich.next()?;
    if rich.next().is_some() {
        return None;
    }
    // The data and structure parts hang off the workbook, the metadata part or the data part.
    let mut rels = workbook_rels.to_vec();
    rels.extend(cx.pkg.rels(&metadata_path).ok()?);
    let data_path = part(&rels, "rdRichValue")?;
    rels.extend(cx.pkg.rels(&data_path).ok()?);
    let structure_path = part(&rels, "rdRichValueStructure")?;
    let data = read_part(cx, &data_path, "rvData")?;
    let structures = read_part(cx, &structure_path, "rvStructures")?;
    let structures: Vec<&El> = structures.kids("s").take(MAX_RECORDS).collect();
    // The error of each rich value: the `errorType` entry of an `_error` structure.
    let errors: Vec<Option<CellError>> = data
        .kids("rv")
        .take(MAX_RECORDS)
        .map(|value| {
            let structure = value.attr_u32("s").and_then(|n| structures.get(n as usize))?;
            if structure.attr("t") != Some("_error") {
                return None;
            }
            let at = structure.kids("k").position(|k| k.attr("n") == Some("errorType"))?;
            let error_type = value.kids("v").nth(at)?.text.trim().parse::<u32>().ok()?;
            error_of(error_type)
        })
        .collect();
    let future: Vec<Option<usize>> = metadata
        .kids("futureMetadata")
        .find(|el| el.attr("name") == Some("XLRICHVALUE"))?
        .kids("bk")
        .take(MAX_RECORDS)
        .map(|block| {
            block
                .child("extLst")
                .and_then(|el| el.kids("ext").find(|ext| ext.attr("uri").is_some_and(|uri| uri.eq_ignore_ascii_case(RICH_EXT))))
                .and_then(|ext| ext.child("rvb"))
                .and_then(|rvb| rvb.attr_u32("i"))
                .map(|n| n as usize)
        })
        .collect();
    let blocks = metadata.child("valueMetadata")?;
    Some(
        blocks
            .kids("bk")
            .take(MAX_RECORDS)
            .map(|block| {
                block
                    .kids("rc")
                    .find(|rc| rc.attr_u32("t").map(|n| n as usize) == Some(type_id))
                    .and_then(|rc| rc.attr_u32("v"))
                    .and_then(|n| future.get(n as usize))
                    .copied()
                    .flatten()
                    .and_then(|n| errors.get(n))
                    .copied()
                    .flatten()
            })
            .collect(),
    )
}
