//! The central preview widget: renders a [`screenforge_core::model::Document`]
//! via the shared `screenforge_core::render::compose` function, scaled to
//! fit the widget ("fit to window"), with a neutral canvas background
//! visible outside the export area so the export bounds stay legible. Also
//! owns the drag-to-reorder gesture (spec §2: "Verschieben per Drag & Drop"),
//! since hit-testing needs the same placement math used to render.

use std::collections::HashMap;

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use screenforge_core::model::Document;
use uuid::Uuid;

glib::wrapper! {
    pub struct Canvas(ObjectSubclass<imp::Canvas>)
        @extends gtk4::Widget,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl Canvas {
    pub fn new() -> Self {
        glib::Object::new()
    }

    /// Replaces the document and its decoded images, and forces a re-render
    /// on the next `snapshot()` regardless of whether the widget size
    /// changed.
    pub fn set_document(&self, document: Document, resolved_images: HashMap<Uuid, gtk4::cairo::ImageSurface>) {
        self.imp().set_document(document, resolved_images);
        self.queue_draw();
    }

    /// Toggles the drag-and-drop hover highlight drawn around the canvas
    /// (spec §1: "visuelles Feedback beim Ziehen").
    pub fn set_drag_active(&self, active: bool) {
        self.imp().drag_active.set(active);
        self.queue_draw();
    }

    /// `None` = fit to the available widget size (the default — no
    /// scrolling). `Some(zoom)` fixes the render scale and reports that
    /// size via `measure()`, so the enclosing `GtkScrolledWindow` shows
    /// scrollbars once the zoomed content is larger than the viewport
    /// (spec §10: "Zoom auf 100%", "An Fenster anpassen", "Verschieben der
    /// Arbeitsfläche").
    pub fn set_zoom(&self, zoom: Option<f64>) {
        self.imp().set_zoom(zoom);
        self.queue_resize();
        self.queue_draw();
    }

    pub fn zoom(&self) -> Option<f64> {
        self.imp().zoom()
    }

    /// Called once a press-drag-release inside the canvas ends up moving an
    /// element to a different position (`from`/`to` are indices into
    /// `Document.elements`, `Vec::insert`-style — see
    /// `screenforge_core::command::ReorderScreenshot`). Never fires for a
    /// drag that starts on empty canvas space or ends where it started.
    pub fn connect_reorder<F: Fn(usize, usize) + 'static>(&self, f: F) {
        self.imp().set_reorder_callback(f);
    }
}

impl Default for Canvas {
    fn default() -> Self {
        Self::new()
    }
}

mod imp {
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;

    use gtk4::cairo;
    use gtk4::glib;
    use gtk4::graphene;
    use gtk4::prelude::*;
    use gtk4::subclass::prelude::*;
    use screenforge_core::layout::Placement;
    use screenforge_core::model::Document;
    use uuid::Uuid;

    type ReorderCallback = Box<dyn Fn(usize, usize)>;

    pub struct Canvas {
        document: RefCell<Document>,
        resolved_images: RefCell<HashMap<Uuid, cairo::ImageSurface>>,
        /// The last rendered composition, cached so `snapshot()` doesn't
        /// re-run `compose()` on every frame — only when content changes or
        /// the widget is resized (which changes the "fit to window" scale).
        cached: RefCell<Option<(cairo::ImageSurface, i32, i32)>>,
        content_dirty: Cell<bool>,
        pub(super) drag_active: Cell<bool>,
        /// `None` = fit to window (default); `Some(z)` = fixed render scale,
        /// advertised as this widget's natural size via `measure()`.
        manual_zoom: Cell<Option<f64>>,
        /// Placements from the last render, in *document* space, kept only
        /// for reorder-drag hit-testing (assumes every element is visible —
        /// true today since there's no per-element hide UI yet; if one is
        /// added, this needs to carry the original `Document.elements`
        /// index alongside each placement instead of relying on them lining
        /// up positionally).
        last_placements: RefCell<Vec<Placement>>,
        last_scale: Cell<f64>,
        /// Index into `last_placements`/`Document.elements` the current
        /// reorder drag picked up, if any.
        drag_from: Cell<Option<usize>>,
        /// Live insertion point while dragging, for the indicator line in
        /// `snapshot()`. In "visible list" terms (`0..=last_placements.len()`),
        /// not yet converted to the `Vec::insert`-style index the reorder
        /// callback expects.
        drag_hover: Cell<Option<usize>>,
        reorder_callback: RefCell<Option<ReorderCallback>>,
    }

    impl Default for Canvas {
        fn default() -> Self {
            Self {
                document: RefCell::new(Document::new()),
                resolved_images: RefCell::new(HashMap::new()),
                cached: RefCell::new(None),
                content_dirty: Cell::new(true),
                drag_active: Cell::new(false),
                manual_zoom: Cell::new(None),
                last_placements: RefCell::new(Vec::new()),
                last_scale: Cell::new(1.0),
                drag_from: Cell::new(None),
                drag_hover: Cell::new(None),
                reorder_callback: RefCell::new(None),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Canvas {
        const NAME: &'static str = "ScreenForgeCanvas";
        type Type = super::Canvas;
        type ParentType = gtk4::Widget;
    }

    impl ObjectImpl for Canvas {
        fn constructed(&self) {
            self.parent_constructed();

            let obj = self.obj();
            let drag = gtk4::GestureDrag::new();
            drag.connect_drag_begin(glib::clone!(
                #[weak]
                obj,
                move |_, x, y| obj.imp().on_drag_begin(x, y)
            ));
            drag.connect_drag_update(glib::clone!(
                #[weak]
                obj,
                move |gesture, offset_x, offset_y| {
                    if let Some((start_x, start_y)) = gesture.start_point() {
                        obj.imp().on_drag_update(start_x + offset_x, start_y + offset_y);
                    }
                }
            ));
            drag.connect_drag_end(glib::clone!(
                #[weak]
                obj,
                move |gesture, offset_x, offset_y| {
                    if let Some((start_x, start_y)) = gesture.start_point() {
                        obj.imp().on_drag_end(start_x + offset_x, start_y + offset_y);
                    } else {
                        obj.imp().cancel_drag();
                    }
                }
            ));
            obj.add_controller(drag);
        }
    }

    impl WidgetImpl for Canvas {
        /// In fit mode (`manual_zoom == None`) this widget has no intrinsic
        /// size — it just fills whatever the `GtkScrolledWindow`'s viewport
        /// gives it (`hexpand`/`vexpand` are set in the `.ui`). In manual
        /// zoom mode it reports the document size at that zoom level, which
        /// is what makes the viewport show scrollbars once that exceeds the
        /// visible area (spec §10: "Verschieben der Arbeitsfläche").
        fn measure(&self, orientation: gtk4::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let Some(zoom) = self.manual_zoom.get() else {
                return (0, 0, -1, -1);
            };
            let doc = self.document.borrow();
            let size = match orientation {
                gtk4::Orientation::Horizontal => doc.canvas.export_width as f64 * zoom,
                _ => doc.canvas.export_height as f64 * zoom,
            };
            let size = size.round().max(1.0) as i32;
            (size, size, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk4::Snapshot) {
            let widget = self.obj();
            let width = widget.width();
            let height = widget.height();
            if width <= 0 || height <= 0 {
                return;
            }

            let bounds = graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
            let ctx = snapshot.append_cairo(&bounds);

            // Neutral canvas background outside the export area (spec §10).
            ctx.set_source_rgba(0.13, 0.13, 0.15, 1.0);
            ctx.rectangle(0.0, 0.0, width as f64, height as f64);
            let _ = ctx.fill();

            self.ensure_rendered(width, height);

            let render_size = self.cached.borrow().as_ref().map(|(_, w, h)| (*w, *h));
            if let Some((render_w, render_h)) = render_size {
                let offset_x = (width as f64 - render_w as f64) / 2.0;
                let offset_y = (height as f64 - render_h as f64) / 2.0;

                if let Some((surface, _, _)) = self.cached.borrow().as_ref() {
                    if ctx.set_source_surface(surface, offset_x, offset_y).is_ok() {
                        let _ = ctx.paint();
                    }
                }

                if let Some(hover) = self.drag_hover.get() {
                    let placements = self.last_placements.borrow();
                    let doc_x = match placements.get(hover) {
                        Some(p) => p.x,
                        None => placements.last().map(|p| p.x + p.width).unwrap_or(0.0),
                    };
                    let scale = self.last_scale.get();
                    let line_x = offset_x + doc_x * scale;
                    ctx.set_source_rgba(0.29, 0.56, 0.89, 0.95);
                    ctx.set_line_width(3.0);
                    ctx.move_to(line_x, offset_y);
                    ctx.line_to(line_x, offset_y + render_h as f64);
                    let _ = ctx.stroke();
                }
            }

            if self.drag_active.get() {
                let inset = 3.0;
                ctx.set_source_rgba(0.29, 0.56, 0.89, 0.9);
                ctx.set_line_width(inset * 2.0);
                ctx.rectangle(inset, inset, width as f64 - 2.0 * inset, height as f64 - 2.0 * inset);
                let _ = ctx.stroke();
            }
        }
    }

    impl Canvas {
        pub fn set_document(&self, document: Document, resolved_images: HashMap<Uuid, cairo::ImageSurface>) {
            *self.document.borrow_mut() = document;
            *self.resolved_images.borrow_mut() = resolved_images;
            self.content_dirty.set(true);
        }

        pub fn set_zoom(&self, zoom: Option<f64>) {
            self.manual_zoom.set(zoom);
            self.content_dirty.set(true);
        }

        pub fn zoom(&self) -> Option<f64> {
            self.manual_zoom.get()
        }

        pub fn set_reorder_callback<F: Fn(usize, usize) + 'static>(&self, f: F) {
            *self.reorder_callback.borrow_mut() = Some(Box::new(f));
        }

        /// Re-renders into `cached` at the current scale — either a fixed
        /// manual zoom, or "fit to window" against the widget's allocated
        /// size — but only if the document changed since the last render or
        /// the effective scale changed (widget resize in fit mode). Also
        /// refreshes `last_placements`/`last_scale`, which is what makes
        /// reorder-drag hit-testing match what's actually on screen.
        fn ensure_rendered(&self, widget_width: i32, widget_height: i32) {
            let doc = self.document.borrow();
            let doc_w = doc.canvas.export_width as f64;
            let doc_h = doc.canvas.export_height as f64;
            if doc_w <= 0.0 || doc_h <= 0.0 {
                return;
            }

            let scale = match self.manual_zoom.get() {
                Some(zoom) => zoom.max(0.01),
                None => (widget_width as f64 / doc_w).min(widget_height as f64 / doc_h).max(0.001),
            };
            let render_w = (doc_w * scale).round().max(1.0) as i32;
            let render_h = (doc_h * scale).round().max(1.0) as i32;

            let size_changed = match self.cached.borrow().as_ref() {
                Some((_, w, h)) => *w != render_w || *h != render_h,
                None => true,
            };
            if !self.content_dirty.get() && !size_changed {
                return;
            }

            let Ok(surface) = cairo::ImageSurface::create(cairo::Format::ARgb32, render_w, render_h) else {
                return;
            };
            let resolved = self.resolved_images.borrow();
            if let Err(err) = screenforge_core::render::compose(&doc, &surface, scale, &resolved) {
                eprintln!("ScreenForge: render error: {err}");
                return;
            }
            drop(resolved);

            let visible: Vec<_> = doc.elements.iter().filter(|e| e.visible).cloned().collect();
            *self.last_placements.borrow_mut() =
                screenforge_core::layout::compute_layout(doc.layout.mode, &visible, doc.layout.spacing_px, doc.layout.margin_px);
            self.last_scale.set(scale);

            *self.cached.borrow_mut() = Some((surface, render_w, render_h));
            self.content_dirty.set(false);
        }

        /// Converts a widget-space point to document space using the scale
        /// and centering offset from the last render, or `None` before
        /// anything has been rendered yet.
        fn widget_to_document(&self, wx: f64, wy: f64) -> Option<(f64, f64)> {
            let scale = self.last_scale.get();
            if scale <= 0.0 {
                return None;
            }
            let (render_w, render_h) = self.cached.borrow().as_ref().map(|(_, w, h)| (*w, *h))?;
            let widget = self.obj();
            let offset_x = (widget.width() as f64 - render_w as f64) / 2.0;
            let offset_y = (widget.height() as f64 - render_h as f64) / 2.0;
            Some(((wx - offset_x) / scale, (wy - offset_y) / scale))
        }

        fn element_index_at(&self, doc_x: f64, doc_y: f64) -> Option<usize> {
            self.last_placements
                .borrow()
                .iter()
                .position(|p| doc_x >= p.x && doc_x <= p.x + p.width && doc_y >= p.y && doc_y <= p.y + p.height)
        }

        /// Where `doc_x` would be inserted among the current placements, as
        /// a position in `0..=last_placements.len()` (not yet adjusted for
        /// the removal of the dragged element — see `on_drag_end`).
        fn insertion_index_at(&self, doc_x: f64) -> usize {
            let placements = self.last_placements.borrow();
            placements.iter().position(|p| doc_x < p.x + p.width / 2.0).unwrap_or(placements.len())
        }

        pub(super) fn on_drag_begin(&self, x: f64, y: f64) {
            let hit = self.widget_to_document(x, y).and_then(|(dx, dy)| self.element_index_at(dx, dy));
            self.drag_from.set(hit);
            self.drag_hover.set(hit);
        }

        pub(super) fn on_drag_update(&self, abs_x: f64, abs_y: f64) {
            if self.drag_from.get().is_none() {
                return;
            }
            if let Some((doc_x, _)) = self.widget_to_document(abs_x, abs_y) {
                self.drag_hover.set(Some(self.insertion_index_at(doc_x)));
                self.obj().queue_draw();
            }
        }

        pub(super) fn on_drag_end(&self, abs_x: f64, abs_y: f64) {
            let Some(from) = self.drag_from.take() else {
                return;
            };
            let insertion = self
                .widget_to_document(abs_x, abs_y)
                .map(|(doc_x, _)| self.insertion_index_at(doc_x))
                .unwrap_or(from);
            self.drag_hover.set(None);
            self.obj().queue_draw();

            // `insertion` is a position in the visible list *before removing
            // the dragged element*; `ReorderScreenshot`/`Vec::insert` expect
            // the position *after* that removal, which is one less whenever
            // the drop point is past the element's original slot.
            let to = if insertion > from { insertion - 1 } else { insertion };
            if to != from {
                if let Some(cb) = self.reorder_callback.borrow().as_ref() {
                    cb(from, to);
                }
            }
        }

        pub(super) fn cancel_drag(&self) {
            self.drag_from.set(None);
            self.drag_hover.set(None);
            self.obj().queue_draw();
        }
    }
}
