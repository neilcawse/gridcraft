//! Replies typed under a comment in the Comments pane and not sent.
//!
//! The app keeps them, not egui's temporary data: they come back with the pane, and they count as
//! unsaved work ([`SheetApp::has_unsaved_work`](crate::SheetApp::has_unsaved_work)) whether or not
//! the pane is drawn.
//!
//! A reply belongs to one comment: its workbook by uid, its sheet by name, and the comment itself by
//! its [`CommentId`](gridcraft_engine::model::CommentId), never by its cell. The id goes with the
//! comment wherever the engine moves it, so rows, columns or cells inserted above it, a sort, a cut
//! and paste, an undo and a redo leave the reply with its comment, while the comment that takes over
//! its cell has none. A comment written over it (after a delete, as a note, or by a paste) is
//! another comment with an id of its own. A workbook can't have two sheets of one name, while a
//! sheet's place among the tabs changes when a sheet is moved, added or deleted, so the place never
//! says whose reply it is.
//!
//! The reply is also for the comment as it was when the reply was typed: its author and text. New
//! Comment on a cell that has one edits that comment in place and keeps its id, but a reply to what
//! it said is not a reply to what it says now, so the reply goes. Resolving the comment, or another
//! reply to it, changes neither, and the reply stays.
//!
//! A reply goes when its workbook closes, when its sheet or its comment is gone, or when its comment
//! has been edited.

use gridcraft_engine::Session;
use gridcraft_engine::model::Comment;

/// A reply and the comment it is for.
struct Draft {
    doc: u64,
    sheet: String,
    /// Which comment: its [`CommentId::get`](gridcraft_engine::model::CommentId::get). The ids
    /// themselves all compare equal, so only the number tells comments apart.
    comment: u64,
    /// Who wrote the comment, and what it said, when the reply was typed.
    author: String,
    said: String,
    /// The reply.
    text: String,
}

impl Draft {
    /// Whether this is a reply to comment `comment` of sheet `sheet` of workbook `doc`, however
    /// that comment reads now.
    fn is_of(&self, doc: u64, sheet: &str, comment: u64) -> bool {
        self.doc == doc && self.comment == comment && self.sheet == sheet
    }

    /// Whether `comment` is the one this reply was typed under, as it was then.
    fn answers(&self, comment: &Comment) -> bool {
        self.comment == comment.id.get() && self.author == comment.author && self.said == comment.text
    }

    /// Whether this is the reply to `comment` of sheet `sheet` of workbook `doc`.
    fn is_for(&self, doc: u64, sheet: &str, comment: &Comment) -> bool {
        self.doc == doc && self.sheet == sheet && self.answers(comment)
    }

    /// Whether the comment it is for is still in an open workbook, as it was.
    fn has_comment(&self, session: &Session) -> bool {
        session
            .documents()
            .iter()
            .find(|d| d.uid == self.doc)
            .and_then(|d| d.wb.sheets.iter().find(|s| s.name == self.sheet))
            .is_some_and(|s| s.comments.values().any(|c| self.answers(c)))
    }
}

/// What has been typed under comments and not sent.
#[derive(Default)]
pub(crate) struct ReplyDrafts {
    drafts: Vec<Draft>,
}

impl ReplyDrafts {
    /// The reply typed under `comment` of sheet `sheet` of workbook `doc`; empty when there is none,
    /// or when the comment has been edited since.
    pub(crate) fn get(&self, doc: u64, sheet: &str, comment: &Comment) -> String {
        self.drafts.iter().find(|d| d.is_for(doc, sheet, comment)).map(|d| d.text.clone()).unwrap_or_default()
    }

    /// Keeps `text` as that reply. An empty one is no draft. A reply kept for an earlier version of
    /// the comment (it was edited since) goes first.
    pub(crate) fn set(&mut self, doc: u64, sheet: &str, comment: &Comment, text: String) {
        let id = comment.id.get();
        self.drafts.retain(|d| !d.is_of(doc, sheet, id) || d.answers(comment));
        if text.is_empty() {
            self.drafts.retain(|d| !d.is_of(doc, sheet, id));
        } else if let Some(d) = self.drafts.iter_mut().find(|d| d.is_of(doc, sheet, id)) {
            d.text = text;
        } else {
            self.drafts.push(Draft { doc, sheet: sheet.to_string(), comment: id, author: comment.author.clone(), said: comment.text.clone(), text });
        }
    }

    /// Drops the replies of a workbook that has closed and those whose sheet or comment is gone
    /// (deleted, or by an undo) or was edited, whichever pane is drawn.
    pub(crate) fn forget_stale(&mut self, session: &Session) {
        if !self.drafts.is_empty() {
            self.drafts.retain(|d| d.has_comment(session));
        }
    }

    /// Whether a reply that is not blank waits under a comment that is still there.
    pub(crate) fn pending(&self, session: &Session) -> bool {
        self.drafts.iter().any(|d| !d.text.trim().is_empty() && d.has_comment(session))
    }

    /// [`Self::pending`] for the replies of workbook `doc` alone.
    pub(crate) fn pending_in(&self, session: &Session, doc: u64) -> bool {
        self.drafts.iter().any(|d| d.doc == doc && !d.text.trim().is_empty() && d.has_comment(session))
    }

    /// Throws away the replies of workbook `doc` (Don't Save when it closes); other workbooks'
    /// replies stay.
    pub(crate) fn discard(&mut self, doc: u64) {
        self.drafts.retain(|d| d.doc != doc);
    }
}

#[cfg(test)]
mod tests {
    use gridcraft_engine::Session;
    use gridcraft_engine::core::CellRef;
    use gridcraft_engine::model::Comment;
    use serde_json::{Value, json};

    use super::ReplyDrafts;

    /// A session with workbook "Alpha" and a comment at B2, and its uid.
    fn commented() -> (Session, u64) {
        let mut s = Session::new();
        s.new_workbook();
        s.execute("sheet.rename", json!({"name": "Alpha"})).unwrap();
        s.execute("review.newComment", json!({"cell": "B2", "text": "Check the figures"})).unwrap();
        let uid = s.active().unwrap().uid;
        (s, uid)
    }

    /// A session with workbook "Alpha" and two threads, "Other thread" at `other` and "Intended
    /// thread" at `mine`, and its uid.
    fn two_threads(other: &str, mine: &str) -> (Session, u64) {
        let mut s = Session::new();
        s.new_workbook();
        s.execute("sheet.rename", json!({"name": "Alpha"})).unwrap();
        s.execute("review.newComment", json!({"cell": other, "text": "Other thread"})).unwrap();
        s.execute("review.newComment", json!({"cell": mine, "text": "Intended thread"})).unwrap();
        let uid = s.active().unwrap().uid;
        (s, uid)
    }

    /// The comment at `cell` of sheet `sheet` of the active workbook.
    fn at(s: &Session, sheet: &str, cell: &str) -> Comment {
        let cell = CellRef::parse(cell).unwrap();
        s.active().unwrap().wb.sheets.iter().find(|sh| sh.name == sheet).unwrap().comments.get(&cell).unwrap().clone()
    }

    /// The reply field the app would show under the comment at `cell` of Alpha.
    fn field(d: &ReplyDrafts, s: &Session, uid: u64, cell: &str) -> String {
        d.get(uid, "Alpha", &at(s, "Alpha", cell))
    }

    #[test]
    fn a_reply_counts_unless_it_is_blank_or_its_comment_is_gone() {
        let (mut s, uid) = commented();
        let b2 = at(&s, "Alpha", "B2");
        let mut d = ReplyDrafts::default();
        d.set(uid, "Alpha", &b2, "   ".into());
        assert_eq!(d.get(uid, "Alpha", &b2), "   ", "the field keeps what was typed");
        assert!(!d.pending(&s), "spaces are not a reply");
        d.set(uid, "Alpha", &b2, "Will do".into());
        assert!(d.pending(&s));
        d.set(uid, "Alpha", &b2, String::new());
        assert!(!d.pending(&s) && d.drafts.is_empty(), "an emptied field is no draft");
        d.set(uid, "Alpha", &b2, "Will do".into());
        s.execute("review.deleteComment", json!({"cell": "B2"})).unwrap();
        assert!(!d.pending(&s), "the comment it answers is gone");
        d.forget_stale(&s);
        assert!(d.drafts.is_empty());
    }

    #[test]
    fn a_reply_belongs_to_its_sheet_by_name_not_by_place() {
        let (mut s, uid) = commented();
        s.execute("sheet.move", json!({"sheet": 0, "to": 1, "copy": true})).unwrap();
        s.execute("sheet.rename", json!({"sheet": 1, "name": "Beta"})).unwrap();
        let (alpha, beta) = (at(&s, "Alpha", "B2"), at(&s, "Beta", "B2"));
        let mut d = ReplyDrafts::default();
        d.set(uid, "Alpha", &alpha, "Only for Alpha".into());
        s.execute("sheet.move", json!({"sheet": 0, "to": 1})).unwrap();
        let names: Vec<String> = s.active().unwrap().wb.sheets.iter().map(|sh| sh.name.clone()).collect();
        assert_eq!(names, ["Beta", "Alpha"], "Alpha is not the first sheet any more");
        assert_eq!(d.get(uid, "Alpha", &alpha), "Only for Alpha");
        assert_eq!(d.get(uid, "Beta", &at(&s, "Beta", "B2")), "", "Beta, now first, has no reply");
        assert_eq!(d.get(uid, "Beta", &alpha), "", "the sheet's name is part of whose reply it is");
        assert_eq!(d.get(uid, "Beta", &beta), "");
        assert!(d.pending(&s));
        d.forget_stale(&s);
        assert!(d.pending(&s), "a move leaves the reply where it was");
        s.execute("home.deleteSheet", json!({"sheet": 1})).unwrap();
        assert!(!d.pending(&s), "Alpha is gone, and Beta has no reply to keep dirty");
        d.forget_stale(&s);
        assert!(d.drafts.is_empty());
    }

    #[test]
    fn a_sheet_that_takes_over_a_name_does_not_take_over_the_reply() {
        // Alpha and Beta say the same, as a copy does. Swapping their names hands "Alpha" to the
        // copy, whose comment looks just like the one the reply was typed under: it is another
        // comment, so the reply is not shown there, and is not sent there either.
        let (mut s, uid) = commented();
        s.execute("sheet.move", json!({"sheet": 0, "to": 1, "copy": true})).unwrap();
        s.execute("sheet.rename", json!({"sheet": 1, "name": "Beta"})).unwrap();
        let mut d = ReplyDrafts::default();
        d.set(uid, "Alpha", &at(&s, "Alpha", "B2"), "Only for the first sheet".into());
        for (sheet, name) in [(0, "Temp"), (1, "Alpha"), (0, "Beta")] {
            s.execute("sheet.rename", json!({"sheet": sheet, "name": name})).unwrap();
        }
        assert_eq!(d.get(uid, "Alpha", &at(&s, "Alpha", "B2")), "", "the copy answers to the name, but is not the comment");
        assert_eq!(d.get(uid, "Beta", &at(&s, "Beta", "B2")), "");
        assert!(!d.pending(&s));
        d.forget_stale(&s);
        assert!(d.drafts.is_empty(), "the comment it was typed under is on no sheet of that name");
    }

    #[test]
    fn closing_a_workbook_drops_its_replies() {
        let (mut s, uid) = commented();
        let b2 = at(&s, "Alpha", "B2");
        let mut d = ReplyDrafts::default();
        d.set(uid, "Alpha", &b2, "Will do".into());
        d.set(uid + 1000, "Alpha", &b2, "Another workbook's".into());
        assert!(d.pending(&s));
        s.close_document(0);
        assert!(!d.pending(&s), "nothing waits in a workbook that is gone");
        d.forget_stale(&s);
        assert!(d.drafts.is_empty());
    }

    #[test]
    fn a_workbook_never_shows_another_workbooks_reply() {
        let (mut s, uid) = commented();
        let mut d = ReplyDrafts::default();
        d.set(uid, "Alpha", &at(&s, "Alpha", "B2"), "For the first workbook".into());
        // A second workbook with an Alpha sheet and a comment at B2 that says the same.
        s.new_workbook();
        s.execute("sheet.rename", json!({"name": "Alpha"})).unwrap();
        s.execute("review.newComment", json!({"cell": "B2", "text": "Check the figures"})).unwrap();
        let other = s.active().unwrap().uid;
        assert_ne!(other, uid);
        assert_eq!(d.get(other, "Alpha", &at(&s, "Alpha", "B2")), "", "the second workbook has no reply");
        assert!(d.pending(&s), "the first workbook's reply still waits in the first workbook");
        s.set_active(0);
        assert_eq!(d.get(uid, "Alpha", &at(&s, "Alpha", "B2")), "For the first workbook");
    }

    /// Types a reply under the intended thread at `mine` (the other thread is at `other`), runs
    /// `command`, and checks that the reply went with its thread to `mine_after`, and that the other
    /// thread, now at `other_after`, has none. The same after undo, and after redo.
    fn insertion_moves_the_reply_with_its_thread(other: &str, mine: &str, command: &str, params: Value, other_after: &str, mine_after: &str) {
        let (mut s, uid) = two_threads(other, mine);
        let mut d = ReplyDrafts::default();
        d.set(uid, "Alpha", &at(&s, "Alpha", mine), "Will do".into());
        assert_eq!(field(&d, &s, uid, other), "");

        let check = |s: &Session, d: &mut ReplyDrafts, (mine_now, other_now): (&str, &str), when: &str| {
            assert_eq!(at(s, "Alpha", mine_now).text, "Intended thread", "{command} {when}: the thread is at {mine_now}");
            assert_eq!(at(s, "Alpha", other_now).text, "Other thread", "{command} {when}: the other thread is at {other_now}");
            assert_eq!(field(d, s, uid, mine_now), "Will do", "{command} {when}: the reply is under the thread it was typed under");
            assert_eq!(field(d, s, uid, other_now), "", "{command} {when}: the other thread has no reply");
            assert!(d.pending(s), "{command} {when}: the reply is still unsent work");
            d.forget_stale(s);
            assert!(d.pending(s), "{command} {when}: a thread that moved is not gone");
        };
        s.execute(command, params).unwrap();
        check(&s, &mut d, (mine_after, other_after), "inserted");
        s.execute("edit.undo", json!({})).unwrap();
        check(&s, &mut d, (mine, other), "undone");
        s.execute("edit.redo", json!({})).unwrap();
        check(&s, &mut d, (mine_after, other_after), "redone");
    }

    #[test]
    fn a_reply_follows_its_comment_when_a_row_is_inserted_above() {
        insertion_moves_the_reply_with_its_thread("B1", "B2", "home.insertRows", json!({"rows": "1:1"}), "B2", "B3");
    }

    #[test]
    fn a_reply_follows_its_comment_when_a_column_is_inserted_to_the_left() {
        insertion_moves_the_reply_with_its_thread("A2", "B2", "home.insertColumns", json!({"cols": "A:A"}), "B2", "C2");
    }

    #[test]
    fn a_reply_follows_its_comment_when_cells_are_inserted_above_it_shifting_down() {
        insertion_moves_the_reply_with_its_thread("B1", "B2", "home.insertCells", json!({"range": "B1:B1", "shift": "down"}), "B2", "B3");
    }

    #[test]
    fn a_reply_follows_its_comment_when_cells_are_inserted_to_its_left_shifting_right() {
        insertion_moves_the_reply_with_its_thread("A2", "B2", "home.insertCells", json!({"range": "A2:A2", "shift": "right"}), "B2", "C2");
    }

    #[test]
    fn a_reply_follows_its_comment_when_the_rows_above_it_are_deleted() {
        // Row 1 is deleted; both threads move up, and the intended one lands on the address the
        // other one had.
        insertion_moves_the_reply_with_its_thread("B2", "B3", "home.deleteRows", json!({"rows": "1:1"}), "B1", "B2");
    }

    #[test]
    fn a_comment_written_over_a_deleted_one_does_not_inherit_its_reply() {
        let (mut s, uid) = commented();
        let mut d = ReplyDrafts::default();
        d.set(uid, "Alpha", &at(&s, "Alpha", "B2"), "Will do".into());
        s.execute("review.deleteComment", json!({"cell": "B2"})).unwrap();
        // The same words by the same author at the same cell: only its identity tells it apart.
        s.execute("review.newComment", json!({"cell": "B2", "text": "Check the figures"})).unwrap();
        assert_eq!(field(&d, &s, uid, "B2").as_str(), "", "a comment of its own has no reply");
        assert!(!d.pending(&s));
        d.forget_stale(&s);
        assert!(d.drafts.is_empty());

        // A note written over a comment is another comment too.
        let (mut s, uid) = commented();
        let mut d = ReplyDrafts::default();
        d.set(uid, "Alpha", &at(&s, "Alpha", "B2"), "Will do".into());
        s.execute("review.newNote", json!({"cell": "B2", "text": "Check the figures"})).unwrap();
        assert_eq!(field(&d, &s, uid, "B2").as_str(), "");
        assert!(!d.pending(&s));
    }

    #[test]
    fn editing_a_comment_in_place_drops_the_reply_to_what_it_said() {
        // New Comment on a cell that has one edits it and keeps who it is: its id stays the same.
        for edit in [json!({"text": "Check the other figures"}), json!({"text": "Check the figures", "author": "Someone Else"})] {
            let (mut s, uid) = commented();
            let before = at(&s, "Alpha", "B2");
            let mut d = ReplyDrafts::default();
            d.set(uid, "Alpha", &before, "Will do".into());
            let mut params = json!({"cell": "B2"});
            params.as_object_mut().unwrap().extend(edit.as_object().unwrap().clone());
            s.execute("review.newComment", params).unwrap();
            let after = at(&s, "Alpha", "B2");
            assert_eq!(after.id.get(), before.id.get(), "{edit}: the same comment");
            assert_ne!((&after.author, &after.text), (&before.author, &before.text), "{edit}: said something else");
            assert_eq!(d.get(uid, "Alpha", &after), "", "{edit}: a reply to what it said is not a reply to what it says");
            assert!(!d.pending(&s), "{edit}");
            d.forget_stale(&s);
            assert!(d.drafts.is_empty(), "{edit}");
            // Typing a new reply is a reply to the comment as it reads now.
            d.set(uid, "Alpha", &after, "Seen".into());
            assert_eq!(d.get(uid, "Alpha", &after), "Seen");
            assert!(d.pending(&s));
        }
    }

    #[test]
    fn typing_after_an_edit_replaces_the_reply_to_the_old_text() {
        let (mut s, uid) = commented();
        let before = at(&s, "Alpha", "B2");
        let mut d = ReplyDrafts::default();
        d.set(uid, "Alpha", &before, "Will do".into());
        s.execute("review.newComment", json!({"cell": "B2", "text": "Check the other figures"})).unwrap();
        let after = at(&s, "Alpha", "B2");
        // The pane shows an empty field for the edited comment and writes back what it shows.
        d.set(uid, "Alpha", &after, String::new());
        assert!(d.drafts.is_empty(), "the old reply is not kept beside the new state");
        d.set(uid, "Alpha", &after, "Done".into());
        d.set(uid, "Alpha", &after, "Done!".into());
        assert_eq!(d.drafts.len(), 1);
        assert_eq!(d.get(uid, "Alpha", &after), "Done!");
    }

    #[test]
    fn resolving_a_comment_or_a_reply_sent_meanwhile_keeps_the_reply() {
        let (mut s, uid) = commented();
        let mut d = ReplyDrafts::default();
        d.set(uid, "Alpha", &at(&s, "Alpha", "B2"), "Will do".into());
        s.execute("review.resolveComment", json!({"cell": "B2"})).unwrap();
        assert!(at(&s, "Alpha", "B2").resolved);
        assert_eq!(field(&d, &s, uid, "B2"), "Will do", "Resolve keeps the reply");
        s.execute("review.replyComment", json!({"cell": "B2", "text": "Meanwhile", "author": "Someone Else"})).unwrap();
        s.execute("review.resolveComment", json!({"cell": "B2", "resolved": false})).unwrap();
        assert_eq!(field(&d, &s, uid, "B2"), "Will do", "another reply doesn't change what this one answers");
        d.forget_stale(&s);
        assert!(d.pending(&s));
    }

    #[test]
    fn a_pasted_copy_of_a_comment_has_no_reply_and_a_cut_comment_takes_it_along() {
        let (mut s, uid) = commented();
        let original = at(&s, "Alpha", "B2");
        let mut d = ReplyDrafts::default();
        d.set(uid, "Alpha", &original, "Will do".into());

        // Copy and paste: the original stays, and so does the reply; the copy is another comment.
        s.execute("edit.copy", json!({"range": "B2"})).unwrap();
        s.execute("edit.paste", json!({"at": "D5"})).unwrap();
        s.execute("edit.paste", json!({"at": "F7"})).unwrap();
        let copy = at(&s, "Alpha", "D5");
        assert_eq!(copy.text, "Check the figures");
        assert_eq!(field(&d, &s, uid, "B2"), "Will do", "the original keeps its reply");
        assert_eq!(field(&d, &s, uid, "D5"), "", "the copy has none");
        assert_eq!(field(&d, &s, uid, "F7"), "", "nor has the second copy");
        d.forget_stale(&s);
        assert!(d.pending(&s));

        // Paste Special ▸ Comments is a copy too.
        s.execute("edit.pasteSpecial", json!({"what": "comments", "at": "H9"})).unwrap();
        assert_eq!(field(&d, &s, uid, "H9"), "");
        assert_eq!(field(&d, &s, uid, "B2"), "Will do");

        // Cut and paste moves the comment, and the reply goes with it.
        s.execute("edit.cut", json!({"range": "B2"})).unwrap();
        s.execute("edit.paste", json!({"at": "B6"})).unwrap();
        assert!(!s.active().unwrap().wb.active().unwrap().comments.contains_key(&CellRef::parse("B2").unwrap()), "B2 was cut");
        assert_eq!(field(&d, &s, uid, "B6"), "Will do", "the moved comment keeps its reply");
        assert_eq!(field(&d, &s, uid, "D5"), "");
        d.forget_stale(&s);
        assert!(d.pending(&s), "a comment that moved is not gone");
    }
}
