//! Undo/redo command pattern. Kept separate from [`crate::model::Document`]
//! itself (rather than nested inside it) so a command can hold `&mut
//! Document` while the stack that dispatched it is borrowed separately —
//! nesting the stack inside the struct it mutates would make `apply`/`undo`
//! unrepresentable in safe Rust.

use std::collections::HashSet;
use std::fmt::Debug;

use uuid::Uuid;

use crate::model::{Background, CornerRadius, Document, ImageSource, LayoutMode, LayoutSettings, ScreenshotElement, ShadowParams, TextElement, Transform};

/// A single reversible mutation of a [`Document`]. Implementations should
/// store enough state to invert themselves cheaply (e.g. the old and new
/// value of a single field), not a full document snapshot.
pub trait Command: Debug {
    fn apply(&self, doc: &mut Document);
    fn undo(&self, doc: &mut Document);
}

#[derive(Default)]
pub struct UndoStack {
    done: Vec<Box<dyn Command>>,
    undone: Vec<Box<dyn Command>>,
}

impl UndoStack {
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies `cmd` to `doc`, pushes it onto the undo history, and clears
    /// the redo history (a fresh action invalidates any previously undone
    /// branch). Re-fits the canvas to content afterward (see
    /// `crate::layout::fit_canvas_to_content`) — centralized here rather
    /// than in each `Command::apply`, so every mutation path (including
    /// undo/redo below) keeps that invariant with no risk of a command
    /// forgetting to maintain it itself.
    pub fn apply(&mut self, cmd: Box<dyn Command>, doc: &mut Document) {
        cmd.apply(doc);
        crate::layout::fit_canvas_to_content(doc);
        self.done.push(cmd);
        self.undone.clear();
    }

    pub fn undo(&mut self, doc: &mut Document) -> bool {
        match self.done.pop() {
            Some(cmd) => {
                cmd.undo(doc);
                crate::layout::fit_canvas_to_content(doc);
                self.undone.push(cmd);
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self, doc: &mut Document) -> bool {
        match self.undone.pop() {
            Some(cmd) => {
                cmd.apply(doc);
                crate::layout::fit_canvas_to_content(doc);
                self.done.push(cmd);
                true
            }
            None => false,
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.done.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.undone.is_empty()
    }
}

/// Appends one or more freshly imported screenshots (spec §17: "Screenshot
/// hinzufügen"). Undo removes exactly the elements this command added, by
/// id — safe even if the user reordered or duplicated elements in between.
#[derive(Debug)]
pub struct AddScreenshots {
    pub elements: Vec<ScreenshotElement>,
}

impl Command for AddScreenshots {
    fn apply(&self, doc: &mut Document) {
        doc.elements.extend(self.elements.iter().cloned());
    }

    fn undo(&self, doc: &mut Document) {
        let ids: HashSet<Uuid> = self.elements.iter().map(|e| e.id).collect();
        doc.elements.retain(|e| !ids.contains(&e.id));
    }
}

/// Moves the element at index `from` to index `to`, both indices into
/// `Document.elements` (spec §2: "Verschieben per Drag & Drop"). `to` is in
/// `Vec::insert` terms — i.e. the position in the array *after* the element
/// has been removed from `from` — which is what makes `apply`/`undo`
/// perfectly symmetric: both are "take from index X, insert at index Y",
/// just with X and Y swapped.
#[derive(Debug)]
pub struct ReorderScreenshot {
    pub from: usize,
    pub to: usize,
}

impl Command for ReorderScreenshot {
    fn apply(&self, doc: &mut Document) {
        if self.from >= doc.elements.len() {
            return;
        }
        let element = doc.elements.remove(self.from);
        let to = self.to.min(doc.elements.len());
        doc.elements.insert(to, element);
    }

    fn undo(&self, doc: &mut Document) {
        if self.to >= doc.elements.len() {
            return;
        }
        let element = doc.elements.remove(self.to);
        let from = self.from.min(doc.elements.len());
        doc.elements.insert(from, element);
    }
}

/// Deletes one element (spec §2/§21: "Löschen"). Keeps a full copy plus its
/// original position so undo can restore it exactly where it was, not just
/// append it back at the end.
#[derive(Debug)]
pub struct RemoveScreenshot {
    pub index: usize,
    pub element: ScreenshotElement,
}

impl Command for RemoveScreenshot {
    fn apply(&self, doc: &mut Document) {
        if self.index < doc.elements.len() {
            doc.elements.remove(self.index);
        }
    }

    fn undo(&self, doc: &mut Document) {
        let index = self.index.min(doc.elements.len());
        doc.elements.insert(index, self.element.clone());
    }
}

/// Deletes several elements as one undo step (spec §5: multi-select
/// delete). `removed` is `(index, element)` pairs captured from the
/// document *before* any of them are removed — `apply` removes
/// highest-index-first so earlier indices stay valid as later removals
/// happen, and `undo` re-inserts lowest-index-first for the same reason.
#[derive(Debug)]
pub struct RemoveScreenshots {
    pub removed: Vec<(usize, ScreenshotElement)>,
}

impl Command for RemoveScreenshots {
    fn apply(&self, doc: &mut Document) {
        let mut indices: Vec<usize> = self.removed.iter().map(|(i, _)| *i).collect();
        indices.sort_unstable_by(|a, b| b.cmp(a));
        for index in indices {
            if index < doc.elements.len() {
                doc.elements.remove(index);
            }
        }
    }

    fn undo(&self, doc: &mut Document) {
        let mut removed = self.removed.clone();
        removed.sort_unstable_by_key(|(index, _)| *index);
        for (index, element) in removed {
            let index = index.min(doc.elements.len());
            doc.elements.insert(index, element);
        }
    }
}

/// Sets several elements' transforms as one undo step — used for a
/// `LayoutMode::Free` drag that moves a multi-selection together (spec
/// §5/§8), and for a single-element move once one is selected alone. Looks
/// elements up by id like [`SetTransform`], rather than assuming a fixed
/// index, since the id is the only thing stable across the elements
/// referenced by different entries of `transforms`.
#[derive(Debug)]
pub struct SetTransforms {
    pub transforms: Vec<(Uuid, Transform, Transform)>,
}

impl Command for SetTransforms {
    fn apply(&self, doc: &mut Document) {
        for (id, _old, new) in &self.transforms {
            if let Some(element) = doc.elements.iter_mut().find(|e| e.id == *id) {
                element.transform = *new;
            }
        }
    }

    fn undo(&self, doc: &mut Document) {
        for (id, old, _new) in &self.transforms {
            if let Some(element) = doc.elements.iter_mut().find(|e| e.id == *id) {
                element.transform = *old;
            }
        }
    }
}

/// Inserts a pre-built duplicate right after the element it was copied from
/// (spec §2/§21: "Duplizieren"). The caller builds `duplicate` (same
/// properties, a fresh id) since generating a new id is an app-layer
/// concern the model shouldn't reach back into `uuid::Uuid::new_v4()` for
/// mid-command — it's simpler to just pass the finished element in.
#[derive(Debug)]
pub struct DuplicateScreenshot {
    pub source_index: usize,
    pub duplicate: ScreenshotElement,
}

impl Command for DuplicateScreenshot {
    fn apply(&self, doc: &mut Document) {
        let insert_at = (self.source_index + 1).min(doc.elements.len());
        doc.elements.insert(insert_at, self.duplicate.clone());
    }

    fn undo(&self, doc: &mut Document) {
        if let Some(pos) = doc.elements.iter().position(|e| e.id == self.duplicate.id) {
            doc.elements.remove(pos);
        }
    }
}

/// Changes one element's position/rotation/flip (spec §21: "Drehen",
/// "Spiegeln"). Covers the whole `Transform` rather than one field each so
/// rotate and flip share a single command type.
#[derive(Debug)]
pub struct SetTransform {
    pub element_id: Uuid,
    pub old: Transform,
    pub new: Transform,
}

impl Command for SetTransform {
    fn apply(&self, doc: &mut Document) {
        if let Some(element) = doc.elements.iter_mut().find(|e| e.id == self.element_id) {
            element.transform = self.new;
        }
    }

    fn undo(&self, doc: &mut Document) {
        if let Some(element) = doc.elements.iter_mut().find(|e| e.id == self.element_id) {
            element.transform = self.old;
        }
    }
}

/// Swaps one element's source image, e.g. after "Screenshot ersetzen" (spec
/// §2/§21), keeping every other property (position in the sequence,
/// shadow, corner radius, transform) untouched.
#[derive(Debug)]
pub struct ReplaceScreenshotSource {
    pub element_id: Uuid,
    pub old_source: ImageSource,
    pub old_natural_width: f64,
    pub old_natural_height: f64,
    pub new_source: ImageSource,
    pub new_natural_width: f64,
    pub new_natural_height: f64,
}

impl Command for ReplaceScreenshotSource {
    fn apply(&self, doc: &mut Document) {
        if let Some(element) = doc.elements.iter_mut().find(|e| e.id == self.element_id) {
            element.source = self.new_source.clone();
            element.natural_width = self.new_natural_width;
            element.natural_height = self.new_natural_height;
        }
    }

    fn undo(&self, doc: &mut Document) {
        if let Some(element) = doc.elements.iter_mut().find(|e| e.id == self.element_id) {
            element.source = self.old_source.clone();
            element.natural_width = self.old_natural_width;
            element.natural_height = self.old_natural_height;
        }
    }
}

#[derive(Debug)]
pub struct SetSpacing {
    pub old: f64,
    pub new: f64,
}

impl Command for SetSpacing {
    fn apply(&self, doc: &mut Document) {
        doc.layout.spacing_px = self.new;
    }

    fn undo(&self, doc: &mut Document) {
        doc.layout.spacing_px = self.old;
    }
}

#[derive(Debug)]
pub struct SetMargin {
    pub old: f64,
    pub new: f64,
}

impl Command for SetMargin {
    fn apply(&self, doc: &mut Document) {
        doc.layout.margin_px = self.new;
    }

    fn undo(&self, doc: &mut Document) {
        doc.layout.margin_px = self.old;
    }
}

#[derive(Debug)]
pub struct SetLayoutMode {
    pub old: LayoutMode,
    pub new: LayoutMode,
}

impl Command for SetLayoutMode {
    fn apply(&self, doc: &mut Document) {
        doc.layout.mode = self.new;
    }

    fn undo(&self, doc: &mut Document) {
        doc.layout.mode = self.old;
    }
}

/// Switches into `LayoutMode::Free` while giving every element a sensible
/// starting position: `transforms` carries each visible element's *last
/// computed* placement (from whichever mode was active before) as its new
/// transform, so free positioning starts from "wherever auto-layout had it"
/// rather than everyone collapsed onto `Transform::default()`'s zeroes.
/// Building `transforms` is the caller's job (it needs `compute_layout` and
/// the current spacing/margin, which aren't this command's concern) —
/// mirrors [`SetShadowForAllElements`] in taking pre-computed per-element
/// state rather than recomputing anything itself.
#[derive(Debug)]
pub struct EnterFreeLayout {
    pub old_mode: LayoutMode,
    pub transforms: Vec<(Uuid, Transform, Transform)>,
}

impl Command for EnterFreeLayout {
    fn apply(&self, doc: &mut Document) {
        doc.layout.mode = LayoutMode::Free;
        for (id, _old, new) in &self.transforms {
            if let Some(element) = doc.elements.iter_mut().find(|e| e.id == *id) {
                element.transform = *new;
            }
        }
    }

    fn undo(&self, doc: &mut Document) {
        doc.layout.mode = self.old_mode;
        for (id, old, _new) in &self.transforms {
            if let Some(element) = doc.elements.iter_mut().find(|e| e.id == *id) {
                element.transform = *old;
            }
        }
    }
}

#[derive(Debug)]
pub struct SetBackground {
    pub old: Background,
    pub new: Background,
}

impl Command for SetBackground {
    fn apply(&self, doc: &mut Document) {
        doc.background = self.new.clone();
    }

    fn undo(&self, doc: &mut Document) {
        doc.background = self.old.clone();
    }
}

/// Applies one shadow preset to every element (spec §9/§27: there's no
/// per-element selection yet, so effects apply to the whole composition).
/// Undo restores each element's *own* prior shadow rather than assuming
/// they were uniform, so this stays correct even if that assumption
/// changes later.
#[derive(Debug)]
pub struct SetShadowForAllElements {
    pub old: Vec<ShadowParams>,
    pub new: ShadowParams,
}

impl Command for SetShadowForAllElements {
    fn apply(&self, doc: &mut Document) {
        for element in &mut doc.elements {
            element.shadow = self.new;
        }
    }

    fn undo(&self, doc: &mut Document) {
        for (element, old) in doc.elements.iter_mut().zip(self.old.iter()) {
            element.shadow = *old;
        }
    }
}

/// Applies one corner radius to every element, mirroring
/// [`SetShadowForAllElements`].
#[derive(Debug)]
pub struct SetCornerRadiusForAllElements {
    pub old: Vec<CornerRadius>,
    pub new: CornerRadius,
}

impl Command for SetCornerRadiusForAllElements {
    fn apply(&self, doc: &mut Document) {
        for element in &mut doc.elements {
            element.corner_radius = self.new;
        }
    }

    fn undo(&self, doc: &mut Document) {
        for (element, old) in doc.elements.iter_mut().zip(self.old.iter()) {
            element.corner_radius = *old;
        }
    }
}

/// Sets the composition-wide title (spec §5) — canvas-relative; see
/// [`SetScreenshotLabel`] for the screenshot-relative kind.
#[derive(Debug)]
pub struct SetTitle {
    pub old: TextElement,
    pub new: TextElement,
}

impl Command for SetTitle {
    fn apply(&self, doc: &mut Document) {
        doc.title = self.new.clone();
    }

    fn undo(&self, doc: &mut Document) {
        doc.title = self.old.clone();
    }
}

/// Sets one screenshot's own label (spec §11) — looked up by id like
/// [`SetTransform`], since a label edit always targets whichever specific
/// element is selected, not "every element" the way shadow/corner-radius
/// commands do.
#[derive(Debug)]
pub struct SetScreenshotLabel {
    pub element_id: Uuid,
    pub old: TextElement,
    pub new: TextElement,
}

impl Command for SetScreenshotLabel {
    fn apply(&self, doc: &mut Document) {
        if let Some(element) = doc.elements.iter_mut().find(|e| e.id == self.element_id) {
            element.label = self.new.clone();
        }
    }

    fn undo(&self, doc: &mut Document) {
        if let Some(element) = doc.elements.iter_mut().find(|e| e.id == self.element_id) {
            element.label = self.old.clone();
        }
    }
}

/// Applies a saved [`crate::template::Template`] — layout mode/spacing/
/// margin, background, and shadow/corner radius for every element — as one
/// undoable step. `old_*` are the caller's job to capture beforehand, same
/// as `SetShadowForAllElements`/`SetCornerRadiusForAllElements`, since a
/// command shouldn't need to reach back into the stack that dispatched it
/// to know what it's reverting to.
#[derive(Debug)]
pub struct ApplyTemplate {
    pub old_layout: LayoutSettings,
    pub old_background: Background,
    pub old_shadows: Vec<ShadowParams>,
    pub old_corner_radii: Vec<CornerRadius>,
    pub new: crate::template::Template,
}

impl Command for ApplyTemplate {
    fn apply(&self, doc: &mut Document) {
        doc.layout = self.new.layout;
        doc.background = self.new.background.clone();
        for element in &mut doc.elements {
            element.shadow = self.new.shadow;
            element.corner_radius = self.new.corner_radius;
        }
    }

    fn undo(&self, doc: &mut Document) {
        doc.layout = self.old_layout;
        doc.background = self.old_background.clone();
        for ((element, shadow), corner_radius) in doc.elements.iter_mut().zip(self.old_shadows.iter()).zip(self.old_corner_radii.iter())
        {
            element.shadow = *shadow;
            element.corner_radius = *corner_radius;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ImageSource, LayoutSettings, Rgba};
    use std::path::PathBuf;

    #[test]
    fn apply_undo_redo_round_trip() {
        let mut doc = Document::new();
        assert_eq!(doc.layout.spacing_px, LayoutSettings::default().spacing_px);
        let mut stack = UndoStack::new();

        stack.apply(Box::new(SetSpacing { old: doc.layout.spacing_px, new: 40.0 }), &mut doc);
        assert_eq!(doc.layout.spacing_px, 40.0);
        assert!(stack.can_undo());
        assert!(!stack.can_redo());

        assert!(stack.undo(&mut doc));
        assert_eq!(doc.layout.spacing_px, LayoutSettings::default().spacing_px);
        assert!(!stack.can_undo());
        assert!(stack.can_redo());

        assert!(stack.redo(&mut doc));
        assert_eq!(doc.layout.spacing_px, 40.0);
    }

    #[test]
    fn new_command_clears_redo_history() {
        let mut doc = Document::new();
        let mut stack = UndoStack::new();
        stack.apply(Box::new(SetSpacing { old: 24.0, new: 40.0 }), &mut doc);
        stack.undo(&mut doc);
        assert!(stack.can_redo());

        stack.apply(Box::new(SetSpacing { old: 24.0, new: 80.0 }), &mut doc);
        assert!(!stack.can_redo());
    }

    #[test]
    fn undo_redo_on_empty_stack_is_noop() {
        let mut doc = Document::new();
        let mut stack = UndoStack::new();
        assert!(!stack.undo(&mut doc));
        assert!(!stack.redo(&mut doc));
    }

    #[test]
    fn add_screenshots_undo_removes_exactly_the_added_elements() {
        let mut doc = Document::new();
        doc.elements.push(ScreenshotElement::new(ImageSource::Path(PathBuf::from("existing.png")), 100.0, 200.0));
        let existing_id = doc.elements[0].id;

        let added = vec![
            ScreenshotElement::new(ImageSource::Path(PathBuf::from("a.png")), 100.0, 200.0),
            ScreenshotElement::new(ImageSource::Path(PathBuf::from("b.png")), 100.0, 200.0),
        ];
        let mut stack = UndoStack::new();
        stack.apply(Box::new(AddScreenshots { elements: added }), &mut doc);
        assert_eq!(doc.elements.len(), 3);

        stack.undo(&mut doc);
        assert_eq!(doc.elements.len(), 1);
        assert_eq!(doc.elements[0].id, existing_id);

        stack.redo(&mut doc);
        assert_eq!(doc.elements.len(), 3);
    }

    /// Regression test for a real bug report: importing a tall portrait
    /// screenshot (e.g. a 1080x2424 phone screenshot) used to leave the
    /// canvas at its 1080-tall default, cropping the bottom off. `UndoStack`
    /// now re-fits the canvas to content after every mutation, so this
    /// exercises the fix through the exact path the app actually uses —
    /// `win.open`'s `AddScreenshots` via `undo_stack.apply` — not just
    /// `fit_canvas_to_content` in isolation.
    #[test]
    fn importing_a_tall_portrait_screenshot_grows_the_canvas_to_fit() {
        let mut doc = Document::new();
        assert_eq!(doc.canvas.export_height, 1080); // the pre-fix default that used to crop

        let portrait = ScreenshotElement::new(ImageSource::Path(PathBuf::from("phone.png")), 1080.0, 2424.0);
        let margin = doc.layout.margin_px;

        let mut stack = UndoStack::new();
        stack.apply(Box::new(AddScreenshots { elements: vec![portrait] }), &mut doc);

        assert_eq!(doc.canvas.export_width, (1080.0 + margin * 2.0) as u32);
        assert_eq!(doc.canvas.export_height, (2424.0 + margin * 2.0) as u32);

        // Undoing removes the last visible element, so `fit_canvas_to_content`
        // is a no-op (by design — see its own doc comment) and the canvas
        // stays at its last fitted size rather than jumping back to the
        // pre-fix default.
        let fitted = doc.canvas;
        stack.undo(&mut doc);
        assert!(doc.elements.is_empty());
        assert_eq!(doc.canvas, fitted);
    }

    #[test]
    fn reorder_screenshot_moves_element_and_undoes_cleanly() {
        let mut doc = Document::new();
        doc.elements = vec![
            ScreenshotElement::new(ImageSource::Path(PathBuf::from("a.png")), 100.0, 200.0),
            ScreenshotElement::new(ImageSource::Path(PathBuf::from("b.png")), 100.0, 200.0),
            ScreenshotElement::new(ImageSource::Path(PathBuf::from("c.png")), 100.0, 200.0),
        ];
        let ids: Vec<_> = doc.elements.iter().map(|e| e.id).collect();

        let mut stack = UndoStack::new();
        stack.apply(Box::new(ReorderScreenshot { from: 0, to: 2 }), &mut doc);
        let after: Vec<_> = doc.elements.iter().map(|e| e.id).collect();
        assert_eq!(after, vec![ids[1], ids[2], ids[0]]);

        stack.undo(&mut doc);
        let restored: Vec<_> = doc.elements.iter().map(|e| e.id).collect();
        assert_eq!(restored, ids);

        stack.redo(&mut doc);
        let after_redo: Vec<_> = doc.elements.iter().map(|e| e.id).collect();
        assert_eq!(after_redo, vec![ids[1], ids[2], ids[0]]);
    }

    #[test]
    fn reorder_screenshot_moving_backward_round_trips() {
        let mut doc = Document::new();
        doc.elements = vec![
            ScreenshotElement::new(ImageSource::Path(PathBuf::from("a.png")), 100.0, 200.0),
            ScreenshotElement::new(ImageSource::Path(PathBuf::from("b.png")), 100.0, 200.0),
            ScreenshotElement::new(ImageSource::Path(PathBuf::from("c.png")), 100.0, 200.0),
        ];
        let ids: Vec<_> = doc.elements.iter().map(|e| e.id).collect();

        let mut stack = UndoStack::new();
        stack.apply(Box::new(ReorderScreenshot { from: 2, to: 0 }), &mut doc);
        let after: Vec<_> = doc.elements.iter().map(|e| e.id).collect();
        assert_eq!(after, vec![ids[2], ids[0], ids[1]]);

        stack.undo(&mut doc);
        let restored: Vec<_> = doc.elements.iter().map(|e| e.id).collect();
        assert_eq!(restored, ids);
    }

    #[test]
    fn remove_screenshot_undo_restores_element_at_its_original_position() {
        let mut doc = Document::new();
        doc.elements = vec![
            ScreenshotElement::new(ImageSource::Path(PathBuf::from("a.png")), 100.0, 200.0),
            ScreenshotElement::new(ImageSource::Path(PathBuf::from("b.png")), 100.0, 200.0),
            ScreenshotElement::new(ImageSource::Path(PathBuf::from("c.png")), 100.0, 200.0),
        ];
        let ids: Vec<_> = doc.elements.iter().map(|e| e.id).collect();
        let removed = doc.elements[1].clone();

        let mut stack = UndoStack::new();
        stack.apply(Box::new(RemoveScreenshot { index: 1, element: removed }), &mut doc);
        let after: Vec<_> = doc.elements.iter().map(|e| e.id).collect();
        assert_eq!(after, vec![ids[0], ids[2]]);

        stack.undo(&mut doc);
        let restored: Vec<_> = doc.elements.iter().map(|e| e.id).collect();
        assert_eq!(restored, ids);
    }

    #[test]
    fn duplicate_screenshot_inserts_after_source_and_undo_removes_it() {
        let mut doc = Document::new();
        doc.elements = vec![
            ScreenshotElement::new(ImageSource::Path(PathBuf::from("a.png")), 100.0, 200.0),
            ScreenshotElement::new(ImageSource::Path(PathBuf::from("b.png")), 100.0, 200.0),
        ];
        let ids: Vec<_> = doc.elements.iter().map(|e| e.id).collect();
        let mut duplicate = doc.elements[0].clone();
        duplicate.id = Uuid::new_v4();
        let dup_id = duplicate.id;

        let mut stack = UndoStack::new();
        stack.apply(Box::new(DuplicateScreenshot { source_index: 0, duplicate }), &mut doc);
        let after: Vec<_> = doc.elements.iter().map(|e| e.id).collect();
        assert_eq!(after, vec![ids[0], dup_id, ids[1]]);

        stack.undo(&mut doc);
        let restored: Vec<_> = doc.elements.iter().map(|e| e.id).collect();
        assert_eq!(restored, ids);
    }

    #[test]
    fn set_transform_round_trips_rotation_and_flip() {
        let mut doc = Document::new();
        doc.elements = vec![ScreenshotElement::new(ImageSource::Path(PathBuf::from("a.png")), 100.0, 200.0)];
        let id = doc.elements[0].id;
        let old = doc.elements[0].transform;
        let mut new = old;
        new.rotation_deg = 90.0;
        new.flip_horizontal = true;

        let mut stack = UndoStack::new();
        stack.apply(Box::new(SetTransform { element_id: id, old, new }), &mut doc);
        assert_eq!(doc.elements[0].transform.rotation_deg, 90.0);
        assert!(doc.elements[0].transform.flip_horizontal);

        stack.undo(&mut doc);
        assert_eq!(doc.elements[0].transform, old);
    }

    #[test]
    fn replace_screenshot_source_round_trips() {
        let mut doc = Document::new();
        doc.elements = vec![ScreenshotElement::new(ImageSource::Path(PathBuf::from("old.png")), 100.0, 200.0)];
        let id = doc.elements[0].id;

        let cmd = ReplaceScreenshotSource {
            element_id: id,
            old_source: ImageSource::Path(PathBuf::from("old.png")),
            old_natural_width: 100.0,
            old_natural_height: 200.0,
            new_source: ImageSource::Path(PathBuf::from("new.png")),
            new_natural_width: 300.0,
            new_natural_height: 400.0,
        };
        let mut stack = UndoStack::new();
        stack.apply(Box::new(cmd), &mut doc);
        assert_eq!(doc.elements[0].source, ImageSource::Path(PathBuf::from("new.png")));
        assert_eq!(doc.elements[0].natural_width, 300.0);

        stack.undo(&mut doc);
        assert_eq!(doc.elements[0].source, ImageSource::Path(PathBuf::from("old.png")));
        assert_eq!(doc.elements[0].natural_width, 100.0);
    }

    #[test]
    fn set_layout_mode_round_trips() {
        let mut doc = Document::new();
        assert_eq!(doc.layout.mode, LayoutMode::Horizontal);

        let mut stack = UndoStack::new();
        stack.apply(Box::new(SetLayoutMode { old: LayoutMode::Horizontal, new: LayoutMode::Grid }), &mut doc);
        assert_eq!(doc.layout.mode, LayoutMode::Grid);

        stack.undo(&mut doc);
        assert_eq!(doc.layout.mode, LayoutMode::Horizontal);
    }

    #[test]
    fn enter_free_layout_snapshots_placements_and_undo_restores_mode_and_transforms() {
        let mut doc = Document::new();
        doc.elements.push(ScreenshotElement::new(ImageSource::Path(PathBuf::from("a.png")), 100.0, 200.0));
        let id = doc.elements[0].id;
        let old_transform = doc.elements[0].transform;
        assert_eq!(old_transform.x, 0.0);

        let mut new_transform = old_transform;
        new_transform.x = 42.0;
        new_transform.width = 100.0;
        new_transform.height = 200.0;

        let mut stack = UndoStack::new();
        stack.apply(
            Box::new(EnterFreeLayout { old_mode: LayoutMode::Horizontal, transforms: vec![(id, old_transform, new_transform)] }),
            &mut doc,
        );
        assert_eq!(doc.layout.mode, LayoutMode::Free);
        assert_eq!(doc.elements[0].transform.x, 42.0);

        stack.undo(&mut doc);
        assert_eq!(doc.layout.mode, LayoutMode::Horizontal);
        assert_eq!(doc.elements[0].transform.x, 0.0);
    }

    #[test]
    fn set_background_round_trips() {
        let mut doc = Document::new();
        let old = doc.background.clone();
        let new = Background::Solid(Rgba::new(0.1, 0.2, 0.3, 1.0));
        let mut stack = UndoStack::new();

        stack.apply(Box::new(SetBackground { old: old.clone(), new: new.clone() }), &mut doc);
        assert!(matches!(doc.background, Background::Solid(c) if c == Rgba::new(0.1, 0.2, 0.3, 1.0)));

        stack.undo(&mut doc);
        assert_eq!(doc.background, old, "undo must restore the exact prior background, whatever its variant");
    }

    #[test]
    fn set_shadow_for_all_elements_restores_each_elements_own_prior_value() {
        let mut doc = Document::new();
        let mut first = ScreenshotElement::new(ImageSource::Path(PathBuf::from("a.png")), 100.0, 200.0);
        first.shadow = ShadowParams::none();
        let mut second = ScreenshotElement::new(ImageSource::Path(PathBuf::from("b.png")), 100.0, 200.0);
        second.shadow = ShadowParams::subtle();
        doc.elements = vec![first, second];

        let old: Vec<ShadowParams> = doc.elements.iter().map(|e| e.shadow).collect();
        let mut stack = UndoStack::new();
        stack.apply(Box::new(SetShadowForAllElements { old, new: ShadowParams::standard() }), &mut doc);
        assert_eq!(doc.elements[0].shadow, ShadowParams::standard());
        assert_eq!(doc.elements[1].shadow, ShadowParams::standard());

        stack.undo(&mut doc);
        assert_eq!(doc.elements[0].shadow, ShadowParams::none());
        assert_eq!(doc.elements[1].shadow, ShadowParams::subtle());
    }

    #[test]
    fn set_corner_radius_for_all_elements_round_trips() {
        let mut doc = Document::new();
        doc.elements = vec![ScreenshotElement::new(ImageSource::Path(PathBuf::from("a.png")), 100.0, 200.0)];
        let old: Vec<CornerRadius> = doc.elements.iter().map(|e| e.corner_radius).collect();

        let mut stack = UndoStack::new();
        stack.apply(Box::new(SetCornerRadiusForAllElements { old, new: CornerRadius::uniform(20.0) }), &mut doc);
        assert_eq!(doc.elements[0].corner_radius, CornerRadius::uniform(20.0));

        stack.undo(&mut doc);
        assert_eq!(doc.elements[0].corner_radius, CornerRadius::none());
    }

    #[test]
    fn apply_template_updates_layout_background_and_every_elements_shadow_and_radius() {
        use crate::model::{Background, LayoutMode, Rgba};
        use crate::template::Template;

        let mut doc = Document::new();
        doc.elements = vec![
            ScreenshotElement::new(ImageSource::Path(PathBuf::from("a.png")), 100.0, 200.0),
            ScreenshotElement::new(ImageSource::Path(PathBuf::from("b.png")), 100.0, 200.0),
        ];
        let old_layout = doc.layout;
        let old_background = doc.background.clone();
        let old_shadows: Vec<ShadowParams> = doc.elements.iter().map(|e| e.shadow).collect();
        let old_corner_radii: Vec<CornerRadius> = doc.elements.iter().map(|e| e.corner_radius).collect();

        let template = Template {
            layout: LayoutSettings { mode: LayoutMode::Grid, spacing_px: 5.0, margin_px: 10.0 },
            background: Background::Solid(Rgba::new(1.0, 0.0, 0.0, 1.0)),
            shadow: ShadowParams::strong(),
            corner_radius: CornerRadius::uniform(8.0),
        };

        let mut stack = UndoStack::new();
        stack.apply(
            Box::new(ApplyTemplate {
                old_layout,
                old_background: old_background.clone(),
                old_shadows,
                old_corner_radii,
                new: template.clone(),
            }),
            &mut doc,
        );

        assert_eq!(doc.layout, template.layout);
        assert_eq!(doc.background, template.background);
        assert!(doc.elements.iter().all(|e| e.shadow == ShadowParams::strong()));
        assert!(doc.elements.iter().all(|e| e.corner_radius == CornerRadius::uniform(8.0)));

        stack.undo(&mut doc);
        assert_eq!(doc.layout, old_layout);
        assert_eq!(doc.background, old_background);
        assert!(doc.elements.iter().all(|e| e.shadow == ShadowParams::none()));
        assert!(doc.elements.iter().all(|e| e.corner_radius == CornerRadius::none()));
    }

    #[test]
    fn remove_screenshots_deletes_all_given_indices_as_one_step() {
        let mut doc = Document::new();
        doc.elements = vec![
            ScreenshotElement::new(ImageSource::Path(PathBuf::from("a.png")), 100.0, 200.0),
            ScreenshotElement::new(ImageSource::Path(PathBuf::from("b.png")), 100.0, 200.0),
            ScreenshotElement::new(ImageSource::Path(PathBuf::from("c.png")), 100.0, 200.0),
        ];
        let ids: Vec<_> = doc.elements.iter().map(|e| e.id).collect();
        // Removing indices 0 and 2 (deliberately unordered/descending in
        // the input, to check `apply` doesn't assume a sort order) should
        // leave only the middle element.
        let removed = vec![(2, doc.elements[2].clone()), (0, doc.elements[0].clone())];

        let mut stack = UndoStack::new();
        stack.apply(Box::new(RemoveScreenshots { removed }), &mut doc);
        let after: Vec<_> = doc.elements.iter().map(|e| e.id).collect();
        assert_eq!(after, vec![ids[1]]);

        stack.undo(&mut doc);
        let restored: Vec<_> = doc.elements.iter().map(|e| e.id).collect();
        assert_eq!(restored, ids);

        stack.redo(&mut doc);
        let after_redo: Vec<_> = doc.elements.iter().map(|e| e.id).collect();
        assert_eq!(after_redo, vec![ids[1]]);
    }

    #[test]
    fn set_transforms_moves_every_given_element_and_undoes_cleanly() {
        let mut doc = Document::new();
        doc.elements = vec![
            ScreenshotElement::new(ImageSource::Path(PathBuf::from("a.png")), 100.0, 200.0),
            ScreenshotElement::new(ImageSource::Path(PathBuf::from("b.png")), 100.0, 200.0),
            ScreenshotElement::new(ImageSource::Path(PathBuf::from("c.png")), 100.0, 200.0),
        ];
        doc.elements[0].transform = Transform { x: 10.0, y: 10.0, ..doc.elements[0].transform };
        doc.elements[1].transform = Transform { x: 50.0, y: 50.0, ..doc.elements[1].transform };
        let (id0, id1, id2) = (doc.elements[0].id, doc.elements[1].id, doc.elements[2].id);
        let (old0, old1) = (doc.elements[0].transform, doc.elements[1].transform);
        let new0 = Transform { x: 30.0, y: 30.0, ..old0 };
        let new1 = Transform { x: 70.0, y: 70.0, ..old1 };

        let mut stack = UndoStack::new();
        stack.apply(Box::new(SetTransforms { transforms: vec![(id0, old0, new0), (id1, old1, new1)] }), &mut doc);
        assert_eq!(doc.elements[0].transform, new0);
        assert_eq!(doc.elements[1].transform, new1);
        // The element not part of the group move is untouched.
        assert_eq!(doc.elements[2].id, id2);

        stack.undo(&mut doc);
        assert_eq!(doc.elements[0].transform, old0);
        assert_eq!(doc.elements[1].transform, old1);
    }
}
