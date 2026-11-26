use super::{Arg, Array, CellError, Criterion, Ctx, R, Value, as_array, has, num_val};

// ---------------------------------------------------------------------------------------------
// Criteria aggregates

type CriteriaPair<'a> = (std::borrow::Cow<'a, Array>, Criterion);

fn criteria_match(pairs: &[CriteriaPair<'_>], row: usize, col: usize) -> bool {
    pairs.iter().all(|(r, c)| c.matches(r.get(row, col).unwrap_or(&Value::Empty)))
}

/// Visit matching coordinates in a read rectangle, in row-major order. A criterion that
/// rejects blanks may constrain the scan to its own read rectangle or indexed hits.
fn visit_matches(pairs: &[CriteriaPair<'_>], rows: usize, cols: usize, mut visit: impl FnMut(usize, usize) -> R<()>) -> R<()> {
    let mut anchor = None;
    let (mut rows, mut cols) = (rows, cols);
    for (i, (range, criterion)) in pairs.iter().enumerate() {
        if !criterion.matches(&Value::Empty) {
            rows = rows.min(range.rows);
            cols = cols.min(range.cols);
        }
        if let Some(pat) = criterion.exact_text()
            && let Some(positions) = range.text_positions(pat)
            && anchor.is_none_or(|(_, _, best): (usize, usize, &[u32])| positions.len() < best.len())
        {
            anchor = Some((i, range.cols, positions));
        }
    }
    if let Some((i, stride, positions)) = anchor {
        for &pos in positions {
            let pos = pos as usize;
            let (row, col) = (pos / stride, pos % stride);
            if row < rows && col < cols && criteria_match(&pairs[..i], row, col) && criteria_match(&pairs[i + 1..], row, col) {
                visit(row, col)?;
            }
        }
    } else {
        for row in 0..rows {
            for col in 0..cols {
                if criteria_match(pairs, row, col) {
                    visit(row, col)?;
                }
            }
        }
    }
    Ok(())
}

/// Parses `(range, criteria)` pairs and checks that their written shapes match.
fn parse_criteria_pairs(a: &[Arg], first_pair: usize, shape: (usize, usize)) -> R<Vec<CriteriaPair<'_>>> {
    let rest = a.get(first_pair..).unwrap_or(&[]);
    if rest.is_empty() || rest.len() % 2 != 0 {
        return Err(CellError::Value);
    }
    // A text criterion over 255 characters is `#VALUE!` only when there is a single criteria
    // pair. With two or more pairs Excel matches its first 255 characters instead.
    let single_pair = rest.len() == 2;
    let mut pairs = Vec::with_capacity(rest.len() / 2);
    for pair in rest.chunks(2) {
        let (Some(range), Some(crit)) = (pair.first(), pair.get(1)) else { return Err(CellError::Value) };
        if range.shape() != shape {
            return Err(CellError::Value);
        }
        let scalar = crit.value.scalar();
        let c = if single_pair { Criterion::parse_arg(&scalar)? } else { Criterion::parse_arg_truncated(&scalar) };
        pairs.push((as_array(&range.value), c));
    }
    Ok(pairs)
}

pub(super) fn countifs(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let shape = crate::util::arg(a, 0)?.shape();
    let mut pairs = parse_criteria_pairs(a, 0, shape)?;
    let mut count = 0u64;
    if pairs.iter().all(|(_, c)| c.matches(&Value::Empty)) {
        // Everything outside the union of the read rectangles matches. Subtract misses inside
        // the union once each: after a wider rectangle, only the rows below it can be new.
        count = (shape.0 as u64).saturating_mul(shape.1 as u64);
        pairs.sort_unstable_by_key(|(r, _)| std::cmp::Reverse(r.cols));
        let mut seen_rows = 0;
        for (range, _) in &pairs {
            for row in seen_rows..range.rows {
                for col in 0..range.cols {
                    if !criteria_match(&pairs, row, col) {
                        count = count.saturating_sub(1);
                    }
                }
            }
            seen_rows = seen_rows.max(range.rows);
        }
    } else {
        visit_matches(&pairs, shape.0, shape.1, |_, _| {
            count = count.saturating_add(1);
            Ok(())
        })?;
    }
    num_val(count as f64)
}

/// Collect numbers in the materialized target whose coordinates match the criteria.
fn ifs_numbers(a: &[Arg]) -> R<Vec<f64>> {
    let target = crate::util::arg(a, 0)?;
    let pairs = parse_criteria_pairs(a, 1, target.shape())?;
    let target = as_array(&target.value);
    let mut out = Vec::new();
    visit_matches(&pairs, target.rows, target.cols, |row, col| {
        match target.get(row, col) {
            Some(Value::Number(n)) => out.push(*n),
            Some(Value::Error(e)) => return Err(*e),
            _ => {}
        }
        Ok(())
    })?;
    Ok(out)
}

pub(super) fn sumifs(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    num_val(ifs_numbers(a)?.iter().sum())
}

pub(super) fn averageifs(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let v = ifs_numbers(a)?;
    if v.is_empty() {
        return Err(CellError::Div0);
    }
    num_val(v.iter().sum::<f64>() / v.len() as f64)
}

pub(super) fn maxifs(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let v = ifs_numbers(a)?;
    num_val(v.iter().copied().reduce(f64::max).unwrap_or(0.0))
}

pub(super) fn minifs(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let v = ifs_numbers(a)?;
    num_val(v.iter().copied().reduce(f64::min).unwrap_or(0.0))
}

pub(super) fn countif(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let range_arg = crate::util::arg(a, 0)?;
    let range = as_array(&range_arg.value);
    let c = Criterion::parse_arg(&crate::util::arg(a, 1)?.value.scalar())?;
    if let Some(pat) = c.exact_text()
        && let Some(pos) = range.text_positions(pat)
    {
        return num_val(pos.len() as f64);
    }
    // A whole-column reference is read inside the used range; the cells past it are blank.
    let cut = range_arg.cut_blanks();
    let cut_matches = if cut > 0 && c.matches(&Value::Empty) { cut } else { 0 };
    let read_matches = range.iter().filter(|v| c.matches(v)).count() as u64;
    num_val(read_matches.saturating_add(cut_matches) as f64)
}

/// SUMIF / AVERAGEIF: matched numbers from the sum range, read at the criteria range's positions.
/// Excel sizes a sum range reference like the criteria range, anchored at its own top-left cell;
/// the evaluator does that resizing (it has the references), so cells line up one to one here.
fn if_numbers(a: &[Arg]) -> R<Vec<f64>> {
    let range_arg = crate::util::arg(a, 0)?;
    let range = as_array(&range_arg.value);
    let c = Criterion::parse_arg(&crate::util::arg(a, 1)?.value.scalar())?;
    let sum_range = if has(a, 2) { Some(as_array(&crate::util::arg(a, 2)?.value)) } else { None };
    let mut out = Vec::new();
    if range.cols != 0
        && let Some(pat) = c.exact_text()
        && let Some(positions) = range.text_positions(pat)
    {
        for &pos in positions {
            let idx = pos as usize;
            let r = idx / range.cols;
            let col = idx % range.cols;
            let target = match &sum_range {
                Some(s) => s.get(r, col),
                None => range.data.get(idx),
            };
            match target {
                Some(Value::Number(n)) => out.push(*n),
                Some(Value::Error(e)) => return Err(*e),
                _ => {}
            }
        }
        return Ok(out);
    }
    let target = sum_range.as_deref().unwrap_or(&range);
    let (rows, cols) = if c.matches(&Value::Empty) {
        let (h, w) = range_arg.shape();
        (h.min(target.rows), w.min(target.cols))
    } else {
        (range.rows.min(target.rows), range.cols.min(target.cols))
    };
    for r in 0..rows {
        for col in 0..cols {
            if !c.matches(range.get(r, col).unwrap_or(&Value::Empty)) {
                continue;
            }
            let target = target.get(r, col);
            match target {
                Some(Value::Number(n)) => out.push(*n),
                Some(Value::Error(e)) => return Err(*e),
                _ => {}
            }
        }
    }
    Ok(out)
}

pub(super) fn sumif(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    num_val(if_numbers(a)?.iter().sum())
}

pub(super) fn averageif(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let v = if_numbers(a)?;
    if v.is_empty() {
        return Err(CellError::Div0);
    }
    num_val(v.iter().sum::<f64>() / v.len() as f64)
}
