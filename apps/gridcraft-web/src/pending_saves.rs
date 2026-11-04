//! Workbook saves handed to a host page that confirms its saves (`?host=parent&ack=1`) and not
//! yet confirmed: each save's workbook and its content then, so a confirmation marks that
//! workbook saved as it was when it was saved, even after later edits or once another workbook
//! is active.

/// The most saves of one workbook kept waiting. The host confirms a newer save of a workbook
/// over an older one, so dropping the oldest only means its late confirmation is ignored (the
/// workbook stays unsaved, never wrongly saved).
pub const MAX_PER_DOC: usize = 8;

/// One save waiting for the host: the workbook it came from (`DocState::uid`) and its content.
struct Pending<T> {
    save_id: u64,
    doc: u64,
    content: T,
}

/// The saves waiting for the host, oldest first.
pub struct PendingSaves<T> {
    pending: Vec<Pending<T>>,
}

impl<T> Default for PendingSaves<T> {
    fn default() -> Self {
        Self { pending: Vec::new() }
    }
}

impl<T> PendingSaves<T> {
    /// Save `save_id` of workbook `doc`, with `content`, went to the host.
    pub fn push(&mut self, save_id: u64, doc: u64, content: T) {
        self.pending.push(Pending { save_id, doc, content });
        let of_doc = self.pending.iter().filter(|p| p.doc == doc).count();
        if of_doc > MAX_PER_DOC
            && let Some(oldest) = self.pending.iter().position(|p| p.doc == doc)
        {
            self.pending.remove(oldest);
        }
    }

    /// The host stored save `save_id`: its workbook and content, if it was waiting. Older saves of
    /// that workbook are forgotten, since the host won't store them after this one.
    pub fn stored(&mut self, save_id: u64) -> Option<(u64, T)> {
        let at = self.pending.iter().position(|p| p.save_id == save_id)?;
        let save = self.pending.remove(at);
        self.pending.retain(|p| p.doc != save.doc || p.save_id > save_id);
        Some((save.doc, save.content))
    }

    /// The host will never store save `save_id` (the user cancelled the picker, the write failed,
    /// a newer save replaced it): forget it. Its workbook stays unsaved.
    pub fn discard(&mut self, save_id: u64) {
        self.pending.retain(|p| p.save_id != save_id);
    }

    /// Forgets saves of workbooks no longer open.
    pub fn keep_open(&mut self, open: &[u64]) {
        self.pending.retain(|p| open.contains(&p.doc));
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stored_save_gives_its_own_workbook_and_content() {
        let mut p = PendingSaves::default();
        p.push(1, 10, "a v1");
        p.push(2, 20, "b v1");
        // The second workbook's save is stored while the first is active: it is still the second's.
        assert_eq!(p.stored(2), Some((20, "b v1")));
        assert_eq!(p.stored(2), None, "a save is stored once");
        assert_eq!(p.stored(1), Some((10, "a v1")));
    }

    #[test]
    fn a_discarded_save_is_never_stored() {
        let mut p = PendingSaves::default();
        p.push(1, 10, "v1");
        p.push(2, 10, "v2");
        p.discard(2);
        assert_eq!(p.stored(2), None, "the workbook stays unsaved");
        assert_eq!(p.len(), 1);
        p.discard(1);
        assert_eq!(p.len(), 0);
    }

    #[test]
    fn a_newer_stored_save_forgets_older_ones_of_that_workbook_only() {
        let mut p = PendingSaves::default();
        p.push(1, 10, "a v1");
        p.push(2, 20, "b v1");
        p.push(3, 10, "a v2");
        assert_eq!(p.stored(3), Some((10, "a v2")));
        assert_eq!(p.stored(1), None);
        assert_eq!(p.stored(2), Some((20, "b v1")));
    }

    #[test]
    fn waiting_saves_stay_bounded() {
        let mut p = PendingSaves::default();
        for id in 1..=100 {
            p.push(id, 10, id);
        }
        p.push(101, 20, 101);
        assert_eq!(p.len(), MAX_PER_DOC + 1);
        assert_eq!(p.stored(1), None, "the oldest went");
        assert_eq!(p.stored(100), Some((10, 100)));
        p.keep_open(&[10]);
        assert_eq!(p.stored(101), None, "a closed workbook's saves are forgotten");
    }
}
