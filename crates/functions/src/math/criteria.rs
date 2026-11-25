use super::{Arg, Array, CellError, Criterion, Ctx, R, Value, as_array, has, num_val};

// ---------------------------------------------------------------------------------------------
// Criteria aggregates

enum MatchedPositions<'a> {
    Indexed(&'a [u32]),
    Mask(Vec<bool>),
}

/// Matched positions for every (range, criteria) pair.
fn criteria_positions<'a>(pairs: &'a [(std::borrow::Cow<'a, Array>, Criterion)], rows: usize, cols: usize) -> MatchedPositions<'a> {
    let mut best_indexed: Option<(usize, &'a [u32])> = None;
    for (idx, (r, c)) in pairs.iter().enumerate() {
        if let Some(pat) = c.exact_text()
            && let Some(pos) = r.text_positions(pat)
            && best_indexed.is_none_or(|(_, prev)| pos.len() < prev.len())
        {
            best_indexed = Some((idx, pos));
        }
    }
    if let Some((best_idx, candidates)) = best_indexed {
        if pairs.len() == 1 {
            return MatchedPositions::Indexed(candidates);
        }
        let mut mask = vec![false; rows.saturating_mul(cols)];
        'cand: for &pos in candidates {
            let i = pos as usize;
            for (pair_idx, (r, c)) in pairs.iter().enumerate() {
                if pair_idx != best_idx {
                    let v = r.data.get(i).unwrap_or(&Value::Empty);
                    if !c.matches(v) {
                        continue 'cand;
                    }
                }
            }
            if let Some(slot) = mask.get_mut(i) {
                *slot = true;
            }
        }
        return MatchedPositions::Mask(mask);
    }
    let mut mask = vec![true; rows.saturating_mul(cols)];
    for (r, c) in pairs {
        for (m, v) in mask.iter_mut().zip(r.iter()) {
            if *m && !c.matches(v) {
                *m = false;
            }
        }
    }
    MatchedPositions::Mask(mask)
}

/// `arr` padded with blanks to `rows` × `cols` (borrowed when it has that shape already).
fn padded(arr: std::borrow::Cow<'_, Array>, rows: usize, cols: usize) -> std::borrow::Cow<'_, Array> {
    if arr.rows == rows && arr.cols == cols {
        return arr;
    }
    let data = (0..rows).flat_map(|r| (0..cols).map(move |c| (r, c))).map(|(r, c)| arr.get(r, c).cloned().unwrap_or(Value::Empty)).collect();
    match Array::new(rows, cols, data) {
        Some(a) => std::borrow::Cow::Owned(a),
        None => arr,
    }
}

/// The block every range of a criteria call is read over: the largest read part among `ranges`.
/// A whole-column range is read inside its sheet's used range only, so ranges on different sheets
/// can come back cut to different sizes; past its own part each is blank.
fn read_block(ranges: &[&Arg]) -> (usize, usize) {
    ranges.iter().fold((0, 0), |(r, c), a| match &a.value {
        Value::Array(x) => (r.max(x.rows), c.max(x.cols)),
        _ => (r.max(1), c.max(1)),
    })
}

/// Parses `(range, criteria)` argument pairs starting at `first_pair`, checks that every range
/// has the written shape `shape`, and reads each over the `rows` × `cols` block.
fn parse_criteria_pairs<'a>(
    a: &'a [Arg],
    first_pair: usize,
    shape: (usize, usize),
    rows: usize,
    cols: usize,
) -> R<Vec<(std::borrow::Cow<'a, Array>, Criterion)>> {
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
        let r = padded(as_array(&range.value), rows, cols);
        let scalar = crit.value.scalar();
        let c = if single_pair { Criterion::parse_arg(&scalar)? } else { Criterion::parse_arg_truncated(&scalar) };
        pairs.push((r, c));
    }
    Ok(pairs)
}

/// The range arguments of an *IFS call: the target (when `first_pair` is 1) and every pair's range.
fn ifs_ranges(a: &[Arg], first_pair: usize) -> Vec<&Arg> {
    let mut out: Vec<&Arg> = a.get(..first_pair).unwrap_or(&[]).iter().collect();
    out.extend(a.get(first_pair..).unwrap_or(&[]).iter().step_by(2));
    out
}

/// Numbers in `target` at matched positions (errors at matched positions propagate).
fn masked_numbers(target: &Array, matched: MatchedPositions<'_>) -> R<Vec<f64>> {
    let mut out = Vec::new();
    match matched {
        MatchedPositions::Indexed(positions) => {
            for &pos in positions {
                match target.data.get(pos as usize) {
                    Some(Value::Number(n)) => out.push(*n),
                    Some(Value::Error(e)) => return Err(*e),
                    _ => {}
                }
            }
        }
        MatchedPositions::Mask(mask) => {
            for (v, m) in target.iter().zip(mask.iter()) {
                if !*m {
                    continue;
                }
                match v {
                    Value::Number(n) => out.push(*n),
                    Value::Error(e) => return Err(*e),
                    _ => {}
                }
            }
        }
    }
    Ok(out)
}

pub(super) fn countifs(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let shape = crate::util::arg(a, 0)?.shape();
    let ranges = ifs_ranges(a, 0);
    let (rows, cols) = read_block(&ranges);
    let pairs = parse_criteria_pairs(a, 0, shape, rows, cols)?;
    let count = match criteria_positions(&pairs, rows, cols) {
        MatchedPositions::Indexed(pos) => pos.len(),
        MatchedPositions::Mask(mask) => mask.iter().filter(|m| **m).count(),
    };
    // Cells of whole-column ranges past every range's used part are blank in all of them.
    let cut = shape.0.saturating_mul(shape.1).saturating_sub(rows.saturating_mul(cols));
    let cut_matches = if cut > 0 && pairs.iter().all(|(_, c)| c.matches(&Value::Empty)) { cut } else { 0 };
    num_val(count.saturating_add(cut_matches) as f64)
}

/// The values of the `*IFS` target range with the mask from its pairs. Blank cells add nothing,
/// so the cells a whole-column reference leaves out past the used range don't matter here.
fn ifs_numbers(a: &[Arg]) -> R<Vec<f64>> {
    let shape = crate::util::arg(a, 0)?.shape();
    let ranges = ifs_ranges(a, 1);
    let (rows, cols) = read_block(&ranges);
    let target = padded(as_array(&crate::util::arg(a, 0)?.value), rows, cols);
    let pairs = parse_criteria_pairs(a, 1, shape, rows, cols)?;
    let matched = criteria_positions(&pairs, rows, cols);
    masked_numbers(&target, matched)
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
    num_val(range.iter().filter(|v| c.matches(v)).count().saturating_add(cut_matches) as f64)
}

/// SUMIF / AVERAGEIF: matched numbers from the sum range, read at the criteria range's positions.
/// Excel sizes a sum range reference like the criteria range, anchored at its own top-left cell;
/// the evaluator does that resizing (it has the references), so cells line up one to one here.
fn if_numbers(a: &[Arg]) -> R<Vec<f64>> {
    let range = as_array(&crate::util::arg(a, 0)?.value);
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
    for r in 0..range.rows {
        for col in 0..range.cols {
            let Some(v) = range.get(r, col) else { continue };
            if !c.matches(v) {
                continue;
            }
            let target = match &sum_range {
                Some(s) => s.get(r, col),
                None => Some(v),
            };
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
