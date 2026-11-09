//! Which comment is which. A comment keeps its identity (`Comment::id`) wherever the engine moves
//! it, and the comment a copy writes beside the original is another one. What is keyed by a
//! comment (a reply typed under it in the Comments pane and not sent) follows the comment through
//! all of this, so nothing here may hand one id to two comments or take it away from the first.

use gridcraft_engine::Session;
use serde_json::{Value as Json, json};

fn workbook() -> Session {
    let mut s = Session::new();
    s.new_workbook();
    s
}

fn run(s: &mut Session, command: &str, params: Json) {
    s.execute(command, params).unwrap();
}

/// Every comment of the sheet named `sheet` as (cell, text, id), in cell order. Two comments of
/// one sheet never share an id.
fn comments(s: &Session, sheet: &str) -> Vec<(String, String, u64)> {
    let wb = &s.doc().unwrap().wb;
    let found: Vec<(String, String, u64)> =
        wb.sheet(wb.sheet_index(sheet).unwrap()).unwrap().comments.iter().map(|(at, c)| (at.a1(), c.text.clone(), c.id.get())).collect();
    let mut ids: Vec<u64> = found.iter().map(|(.., id)| *id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), found.len(), "{sheet}: two comments share an id: {found:?}");
    found
}

/// The text and id of the comment at `cell` of `sheet`.
fn at(s: &Session, sheet: &str, cell: &str) -> Option<(String, u64)> {
    comments(s, sheet).into_iter().find(|(a1, ..)| a1 == cell).map(|(_, text, id)| (text, id))
}

/// The id of the comment at `cell` of `sheet`.
fn id(s: &Session, sheet: &str, cell: &str) -> u64 {
    at(s, sheet, cell).unwrap().1
}

/// Sheet1 with the comments "one" at B2 and "two" at C3 (each on a cell with a value), and their ids.
fn two_comments() -> (Session, u64, u64) {
    let mut s = workbook();
    for (cell, text) in [("B2", "one"), ("C3", "two")] {
        run(&mut s, "cell.set", json!({"cell": cell, "input": text}));
        run(&mut s, "review.newComment", json!({"cell": cell, "text": text}));
    }
    let (one, two) = (id(&s, "Sheet1", "B2"), id(&s, "Sheet1", "C3"));
    assert_ne!(one, two, "setup: two comments, two ids");
    (s, one, two)
}

/// Sheet1 holds exactly "one" at `one_at` and "two" at `two_at`, with the ids they had.
fn assert_comments(s: &Session, what: &str, (one_at, one): (&str, u64), (two_at, two): (&str, u64)) {
    let mut found = comments(s, "Sheet1");
    found.sort_by(|a, b| a.1.cmp(&b.1));
    assert_eq!(found, [(one_at.to_string(), "one".to_string(), one), (two_at.to_string(), "two".to_string(), two)], "{what}");
}

#[test]
fn inserting_deleting_and_moving_lines_and_cells_moves_a_comment_with_its_id() {
    let cases = [
        ("home.insertRows", json!({"rows": "1:1"}), "B3", "C4"),
        ("home.insertColumns", json!({"cols": "A:A"}), "C2", "D3"),
        ("home.insertCells", json!({"range": "B1:C1", "shift": "down"}), "B3", "C4"),
        ("home.insertCells", json!({"range": "A2:A3", "shift": "right"}), "C2", "D3"),
        ("home.deleteRows", json!({"rows": "1:1"}), "B1", "C2"),
        ("home.deleteColumns", json!({"cols": "A:A"}), "A2", "B3"),
        ("home.deleteCells", json!({"range": "B1:C1", "shift": "up"}), "B1", "C2"),
        ("home.deleteCells", json!({"range": "A2:A3", "shift": "left"}), "A2", "B3"),
        ("sheet.moveColumns", json!({"cols": "B:B", "before": 4}), "D2", "B3"),
    ];
    for (command, params, one_after, two_after) in cases {
        let (mut s, one, two) = two_comments();
        let what = format!("{command} {params}");
        run(&mut s, command, params);
        assert_comments(&s, &what, (one_after, one), (two_after, two));
        run(&mut s, "edit.undo", json!({}));
        assert_comments(&s, &format!("{what}, undone"), ("B2", one), ("C3", two));
        run(&mut s, "edit.redo", json!({}));
        assert_comments(&s, &format!("{what}, redone"), (one_after, one), (two_after, two));
    }
}

#[test]
fn deleting_the_line_a_comment_is_on_takes_the_comment_and_no_other() {
    let (mut s, one, two) = two_comments();
    run(&mut s, "home.deleteRows", json!({"rows": "2:2"}));
    assert_eq!(comments(&s, "Sheet1"), [("C2".to_string(), "two".to_string(), two)]);
    run(&mut s, "edit.undo", json!({}));
    assert_comments(&s, "undone", ("B2", one), ("C3", two));
}

#[test]
fn sorting_moves_a_comment_with_its_row() {
    let mut s = workbook();
    run(&mut s, "range.setValues", json!({"range": "A1", "values": [[3], [1], [2]]}));
    for (cell, text) in [("B1", "three"), ("B2", "one"), ("B3", "two")] {
        run(&mut s, "review.newComment", json!({"cell": cell, "text": text}));
    }
    let ids = comments(&s, "Sheet1");
    run(&mut s, "data.sort", json!({"range": "A1:B3", "header": false, "keys": [{"column": "A"}]}));
    let (three, one, two) = (ids[0].2, ids[1].2, ids[2].2);
    let sorted =
        [("B1".to_string(), "one".to_string(), one), ("B2".to_string(), "two".to_string(), two), ("B3".to_string(), "three".to_string(), three)];
    assert_eq!(comments(&s, "Sheet1"), sorted, "each comment went with its row");
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(comments(&s, "Sheet1"), ids);
    run(&mut s, "edit.redo", json!({}));
    assert_eq!(comments(&s, "Sheet1"), sorted);
}

#[test]
fn removing_duplicates_moves_a_comment_with_its_row() {
    let mut s = workbook();
    // Column B has values too: the command works on the part of the range that holds data.
    run(&mut s, "range.setValues", json!({"range": "A1", "values": [["x", "p"], ["y", "q"], ["x", "r"], ["z", "s"]]}));
    for (cell, text) in [("B1", "first"), ("B2", "second"), ("B3", "duplicate"), ("B4", "last")] {
        run(&mut s, "review.newComment", json!({"cell": cell, "text": text}));
    }
    let (first, second, last) = (id(&s, "Sheet1", "B1"), id(&s, "Sheet1", "B2"), id(&s, "Sheet1", "B4"));
    run(&mut s, "data.removeDuplicates", json!({"range": "A1:B4", "columns": ["A"], "header": false}));
    assert_eq!(
        comments(&s, "Sheet1"),
        [
            ("B1".to_string(), "first".to_string(), first),
            ("B2".to_string(), "second".to_string(), second),
            ("B3".to_string(), "last".to_string(), last)
        ],
        "the duplicate row's comment went with the row; the last row moved up with its comment"
    );
}

#[test]
fn cut_and_paste_moves_a_comment_with_its_id() {
    let (mut s, one, two) = two_comments();
    run(&mut s, "edit.cut", json!({"range": "B2"}));
    run(&mut s, "edit.paste", json!({"at": "E5"}));
    assert_comments(&s, "cut and pasted", ("E5", one), ("C3", two));
    run(&mut s, "edit.undo", json!({}));
    assert_comments(&s, "undone", ("B2", one), ("C3", two));
    run(&mut s, "edit.redo", json!({}));
    assert_comments(&s, "redone", ("E5", one), ("C3", two));

    // To another sheet: the comment is the same one there, and the first sheet has none of it.
    run(&mut s, "home.insertSheet", json!({"name": "Sheet2"}));
    run(&mut s, "sheet.activate", json!({"sheet": "Sheet1"}));
    run(&mut s, "edit.cut", json!({"range": "C3"}));
    run(&mut s, "sheet.activate", json!({"sheet": "Sheet2"}));
    run(&mut s, "edit.paste", json!({"at": "A1"}));
    assert_eq!(comments(&s, "Sheet1"), [("E5".to_string(), "one".to_string(), one)]);
    assert_eq!(comments(&s, "Sheet2"), [("A1".to_string(), "two".to_string(), two)]);
}

#[test]
fn a_cut_pasted_over_a_larger_selection_moves_the_comment_once() {
    // Whatever the engine writes over the selection besides the block itself, the comment that was
    // cut is on the sheet once (`comments` also checks that no two comments share an id).
    let (mut s, one, _) = two_comments();
    run(&mut s, "edit.cut", json!({"range": "B2"}));
    run(&mut s, "selection.set", json!({"range": "E5:E7"}));
    run(&mut s, "edit.paste", json!({}));
    assert_eq!(at(&s, "Sheet1", "E5"), Some(("one".to_string(), one)), "the comment that was cut is in the first place, with its id");
    assert!(at(&s, "Sheet1", "B2").is_none(), "and no longer at the source");
}

#[test]
fn pasting_a_copy_writes_another_comment_with_an_id_of_its_own() {
    let (mut s, one, two) = two_comments();
    run(&mut s, "edit.copy", json!({"range": "B2"}));
    run(&mut s, "edit.paste", json!({"at": "E5"}));
    run(&mut s, "edit.paste", json!({"at": "G7"}));
    let (first, second) = (at(&s, "Sheet1", "E5").unwrap(), at(&s, "Sheet1", "G7").unwrap());
    assert_eq!((first.0.as_str(), second.0.as_str()), ("one", "one"), "the copies say what the original says");
    let mut ids = [one, two, first.1, second.1];
    ids.sort_unstable();
    assert!(ids.windows(2).all(|w| w[0] != w[1]), "four comments, four ids: {ids:?}");
    assert_eq!(id(&s, "Sheet1", "B2"), one, "the original stays what it was");

    // Undo takes the second copy away, and redo brings back that copy: the one it was.
    run(&mut s, "edit.undo", json!({}));
    assert!(at(&s, "Sheet1", "G7").is_none());
    run(&mut s, "edit.redo", json!({}));
    assert_eq!(at(&s, "Sheet1", "G7"), Some(second));
    assert_eq!(id(&s, "Sheet1", "B2"), one);
}

#[test]
fn every_copy_pasted_over_a_larger_selection_is_a_comment_of_its_own() {
    let (mut s, one, _) = two_comments();
    run(&mut s, "edit.copy", json!({"range": "B2"}));
    run(&mut s, "selection.set", json!({"range": "E5:E7"}));
    run(&mut s, "edit.paste", json!({}));
    let tiles: Vec<u64> = ["E5", "E6", "E7"].iter().map(|c| id(&s, "Sheet1", c)).collect();
    assert!(!tiles.contains(&one), "{tiles:?}");
    assert!(tiles[0] != tiles[1] && tiles[1] != tiles[2] && tiles[0] != tiles[2], "{tiles:?}");
    assert_eq!(id(&s, "Sheet1", "B2"), one);
}

#[test]
fn paste_special_writes_another_comment_whatever_the_source_does() {
    let (mut s, one, two) = two_comments();
    // Comments only, from a copy and from a cut (a cut keeps its source until a full paste).
    run(&mut s, "edit.copy", json!({"range": "B2"}));
    run(&mut s, "edit.pasteSpecial", json!({"what": "comments", "at": "E5"}));
    run(&mut s, "edit.cut", json!({"range": "C3"}));
    run(&mut s, "edit.pasteSpecial", json!({"what": "comments", "at": "G7"}));
    let (from_copy, from_cut) = (id(&s, "Sheet1", "E5"), id(&s, "Sheet1", "G7"));
    assert_eq!(at(&s, "Sheet1", "E5").unwrap().0, "one");
    assert_eq!(at(&s, "Sheet1", "G7").unwrap().0, "two");
    assert!(![one, two].contains(&from_copy) && ![one, two].contains(&from_cut) && from_copy != from_cut);
    assert_eq!((id(&s, "Sheet1", "B2"), id(&s, "Sheet1", "C3")), (one, two), "the sources keep theirs");

    // Everything, transposed, to another sheet: a copy is a copy wherever it lands.
    run(&mut s, "home.insertSheet", json!({"name": "Sheet2"}));
    run(&mut s, "sheet.activate", json!({"sheet": "Sheet1"}));
    run(&mut s, "edit.copy", json!({"range": "B2:C3"}));
    run(&mut s, "sheet.activate", json!({"sheet": "Sheet2"}));
    run(&mut s, "edit.pasteSpecial", json!({"what": "all", "at": "A1", "transpose": true}));
    let there = comments(&s, "Sheet2");
    assert_eq!(there.iter().map(|(a1, text, _)| (a1.as_str(), text.as_str())).collect::<Vec<_>>(), [("A1", "one"), ("B2", "two")]);
    assert!(there.iter().all(|(.., id)| ![one, two, from_copy, from_cut].contains(id)), "{there:?}");
}

#[test]
fn insert_copied_cells_writes_a_copy_and_insert_cut_cells_moves_the_comment() {
    let (mut s, one, two) = two_comments();
    run(&mut s, "edit.copy", json!({"range": "2:2"}));
    run(&mut s, "home.insertCopiedCells", json!({"at": "A4"}));
    let found = comments(&s, "Sheet1");
    assert_eq!(found.iter().map(|(a1, text, _)| (a1.as_str(), text.as_str())).collect::<Vec<_>>(), [("B2", "one"), ("C3", "two"), ("B4", "one")]);
    assert_eq!((found[0].2, found[1].2), (one, two), "the comments that were there keep their ids");
    assert!(found[2].2 != one && found[2].2 != two, "the inserted copy is a comment of its own");

    let (mut s, one, two) = two_comments();
    run(&mut s, "edit.cut", json!({"range": "2:2"}));
    run(&mut s, "home.insertCopiedCells", json!({"at": "A6"}));
    assert_comments(&s, "row 2 moved to just above row 6 (row 5 once the gap closes)", ("B5", one), ("C2", two));
}

#[test]
fn a_comment_edited_in_place_keeps_its_id_and_another_written_over_it_does_not() {
    let (mut s, one, _) = two_comments();
    run(&mut s, "review.newComment", json!({"cell": "B2", "text": "edited", "author": "Someone Else"}));
    run(&mut s, "review.replyComment", json!({"cell": "B2", "text": "reply"}));
    run(&mut s, "review.resolveComment", json!({"cell": "B2"}));
    assert_eq!(
        at(&s, "Sheet1", "B2"),
        Some(("edited".to_string(), one)),
        "an edit, a reply and Resolve change what it says, not which comment it is"
    );

    // A note over a comment replaces it, as does a comment written after a delete.
    run(&mut s, "review.newNote", json!({"cell": "B2", "text": "a note"}));
    let note = id(&s, "Sheet1", "B2");
    assert_ne!(note, one);
    run(&mut s, "review.deleteComment", json!({"cell": "B2"}));
    run(&mut s, "review.newComment", json!({"cell": "B2", "text": "a note"}));
    let again = id(&s, "Sheet1", "B2");
    assert!(again != one && again != note);

    // Undo gives back the comments as they were, with their ids.
    run(&mut s, "edit.undo", json!({}));
    assert!(at(&s, "Sheet1", "B2").is_none());
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(id(&s, "Sheet1", "B2"), note);
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(id(&s, "Sheet1", "B2"), one);
}

#[test]
fn a_copied_sheet_has_comments_of_its_own_and_a_moved_sheet_keeps_its_ids() {
    let (mut s, one, two) = two_comments();
    run(&mut s, "sheet.move", json!({"sheet": 0, "to": 1, "copy": true}));
    run(&mut s, "sheet.rename", json!({"sheet": 1, "name": "Copy"}));
    assert_comments(&s, "the original is untouched", ("B2", one), ("C3", two));
    let copy = comments(&s, "Copy");
    assert_eq!(copy.iter().map(|(a1, text, _)| (a1.as_str(), text.as_str())).collect::<Vec<_>>(), [("B2", "one"), ("C3", "two")]);
    assert!(copy.iter().all(|(.., id)| *id != one && *id != two), "{copy:?}");

    run(&mut s, "sheet.move", json!({"sheet": 0, "to": 1}));
    let names: Vec<String> = s.doc().unwrap().wb.sheets.iter().map(|sh| sh.name.clone()).collect();
    assert_eq!(names, ["Copy", "Sheet1"]);
    assert_comments(&s, "a moved sheet is the same sheet", ("B2", one), ("C3", two));
    assert_eq!(comments(&s, "Copy"), copy);
}
