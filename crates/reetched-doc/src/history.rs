//! Undo/redo by snapshot: the document is small (bitmaps live in `Resources`),
//! so each finished edit stores a copy of the previous state.

use crate::Document;

const MAX_STEPS: usize = 200;

pub struct History {
    undo: Vec<Document>,
    redo: Vec<Document>,
    /// Last state the user finished editing.
    committed: Document,
    /// Counts commits, undos and redos (tells listeners the document changed).
    steps: u64,
}

impl History {
    pub fn new(doc: &Document) -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            committed: doc.clone(),
            steps: 0,
        }
    }

    /// Changes whenever a step is committed, undone or redone.
    pub fn steps(&self) -> u64 {
        self.steps
    }

    /// Call once per frame. While `interacting` (e.g. a slider is held) changes are
    /// not committed, so a whole drag becomes one undo step.
    pub fn observe(&mut self, doc: &Document, interacting: bool) {
        if interacting || *doc == self.committed {
            return;
        }
        let previous = std::mem::replace(&mut self.committed, doc.clone());
        self.undo.push(previous);
        if self.undo.len() > MAX_STEPS {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.steps += 1;
    }

    pub fn undo(&mut self, doc: &mut Document) -> bool {
        // Commit any pending edit first so it can be redone.
        self.observe(doc, false);
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        self.redo.push(std::mem::replace(doc, previous));
        self.committed = doc.clone();
        self.steps += 1;
        true
    }

    pub fn redo(&mut self, doc: &mut Document) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push(std::mem::replace(doc, next));
        self.committed = doc.clone();
        self.steps += 1;
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_is_one_step_and_undo_redo_round_trip() {
        let mut doc = Document::new(None, &["a".to_string()]);
        let mut h = History::new(&doc);
        for r in [600, 700, 800] {
            doc.texture_sets[0].resolution = r;
            h.observe(&doc, true); // still dragging
        }
        h.observe(&doc, false);
        assert!(h.can_undo());
        assert!(h.undo(&mut doc));
        assert_eq!(doc.texture_sets[0].resolution, 2048);
        assert!(!h.can_undo());
        assert!(h.redo(&mut doc));
        assert_eq!(doc.texture_sets[0].resolution, 800);
        h.observe(&doc, false);
        assert!(h.can_undo() && !h.can_redo());
    }

    #[test]
    fn new_edit_clears_redo() {
        let mut doc = Document::new(None, &["a".to_string()]);
        let mut h = History::new(&doc);
        doc.texture_sets[0].resolution = 512;
        h.observe(&doc, false);
        h.undo(&mut doc);
        doc.texture_sets[0].name = "b".into();
        h.observe(&doc, false);
        assert!(!h.can_redo());
    }
}
