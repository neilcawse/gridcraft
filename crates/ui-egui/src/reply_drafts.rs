//! Replies typed under a comment in the Comments pane and not sent.
//!
//! The app keeps them, not egui's temporary data: they come back with the pane, and they count as
//! unsaved work ([`SheetApp::has_unsaved_work`](crate::SheetApp::has_unsaved_work)) whether or not
//! the pane is drawn. A reply belongs to one comment: its workbook by uid, its sheet by name, its
//! cell. A workbook can't have two sheets of one name, while a sheet's place among the tabs changes
//! when a sheet is moved, added or deleted, so the place never says whose reply it is. A reply goes
//! when its workbook closes or when its sheet or its comment is gone.

use gridcraft_engine::Session;
use gridcraft_engine::core::CellRef;

/// A reply and the comment it is for.
struct Draft {
    doc: u64,
    sheet: String,
    cell: CellRef,
    text: String,
}

impl Draft {
    fn is(&self, doc: u64, sheet: &str, cell: CellRef) -> bool {
        self.doc == doc && self.cell == cell && self.sheet == sheet
    }

    /// Whether the comment it is for is still in an open workbook.
    fn has_comment(&self, session: &Session) -> bool {
        session
            .documents()
            .iter()
            .find(|d| d.uid == self.doc)
            .and_then(|d| d.wb.sheets.iter().find(|s| s.name == self.sheet))
            .is_some_and(|s| s.comments.contains_key(&self.cell))
    }
}

/// What has been typed under comments and not sent.
#[derive(Default)]
pub(crate) struct ReplyDrafts {
    drafts: Vec<Draft>,
}

impl ReplyDrafts {
    /// The reply typed under the comment at `cell` of sheet `sheet` of workbook `doc`; empty when
    /// there is none.
    pub(crate) fn get(&self, doc: u64, sheet: &str, cell: CellRef) -> String {
        self.drafts.iter().find(|d| d.is(doc, sheet, cell)).map(|d| d.text.clone()).unwrap_or_default()
    }

    /// Keeps `text` as that reply. An empty one is no draft.
    pub(crate) fn set(&mut self, doc: u64, sheet: &str, cell: CellRef, text: String) {
        if text.is_empty() {
            self.drafts.retain(|d| !d.is(doc, sheet, cell));
        } else if let Some(d) = self.drafts.iter_mut().find(|d| d.is(doc, sheet, cell)) {
            d.text = text;
        } else {
            self.drafts.push(Draft { doc, sheet: sheet.to_string(), cell, text });
        }
    }

    /// Drops the replies of a workbook that has closed and those whose sheet or comment is gone
    /// (deleted, or by an undo), whichever pane is drawn.
    pub(crate) fn forget_stale(&mut self, session: &Session) {
        if !self.drafts.is_empty() {
            self.drafts.retain(|d| d.has_comment(session));
        }
    }

    /// Whether a reply that is not blank waits under a comment that is still there.
    pub(crate) fn pending(&self, session: &Session) -> bool {
        self.drafts.iter().any(|d| !d.text.trim().is_empty() && d.has_comment(session))
    }
}

#[cfg(test)]
mod tests {
    use gridcraft_engine::Session;
    use gridcraft_engine::core::CellRef;
    use serde_json::json;

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

    const B2: CellRef = CellRef::new(1, 1);

    #[test]
    fn a_reply_counts_unless_it_is_blank_or_its_comment_is_gone() {
        let (mut s, uid) = commented();
        let mut d = ReplyDrafts::default();
        d.set(uid, "Alpha", B2, "   ".into());
        assert_eq!(d.get(uid, "Alpha", B2), "   ", "the field keeps what was typed");
        assert!(!d.pending(&s), "spaces are not a reply");
        d.set(uid, "Alpha", B2, "Will do".into());
        assert!(d.pending(&s));
        d.set(uid, "Alpha", B2, String::new());
        assert!(!d.pending(&s) && d.drafts.is_empty(), "an emptied field is no draft");
        d.set(uid, "Alpha", B2, "Will do".into());
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
        let mut d = ReplyDrafts::default();
        d.set(uid, "Alpha", B2, "Only for Alpha".into());
        s.execute("sheet.move", json!({"sheet": 0, "to": 1})).unwrap();
        let names: Vec<String> = s.active().unwrap().wb.sheets.iter().map(|sh| sh.name.clone()).collect();
        assert_eq!(names, ["Beta", "Alpha"], "Alpha is not the first sheet any more");
        assert_eq!(d.get(uid, "Alpha", B2), "Only for Alpha");
        assert_eq!(d.get(uid, "Beta", B2), "", "Beta, now first, has no reply");
        assert!(d.pending(&s));
        d.forget_stale(&s);
        assert!(d.pending(&s), "a move leaves the reply where it was");
        s.execute("home.deleteSheet", json!({"sheet": 1})).unwrap();
        assert!(!d.pending(&s), "Alpha is gone, and Beta has no reply to keep dirty");
        d.forget_stale(&s);
        assert!(d.drafts.is_empty());
    }

    #[test]
    fn closing_a_workbook_drops_its_replies() {
        let (mut s, uid) = commented();
        let mut d = ReplyDrafts::default();
        d.set(uid, "Alpha", B2, "Will do".into());
        d.set(uid + 1000, "Alpha", B2, "Another workbook's".into());
        assert!(d.pending(&s));
        s.close_document(0);
        assert!(!d.pending(&s), "nothing waits in a workbook that is gone");
        d.forget_stale(&s);
        assert!(d.drafts.is_empty());
    }
}
