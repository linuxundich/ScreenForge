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
    /// changed. `background_image` is only consulted when the document's
    /// background is `Background::Image`.
    pub fn set_document(
        &self,
        document: Document,
        resolved_images: HashMap<Uuid, gtk4::cairo::ImageSurface>,
        background_image: Option<gtk4::cairo::ImageSurface>,
    ) {
        self.imp().set_document(document, resolved_images, background_image);
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

    /// Called on a secondary (right) click that lands on a screenshot,
    /// with the element's index into `Document.elements` and the click
    /// point in widget coordinates (for positioning a popover menu) — spec
    /// §21: "Für ausgewählte Screenshots soll ein Kontextmenü verfügbar
    /// sein". Never fires for a click on empty canvas space.
    pub fn connect_context_menu<F: Fn(usize, f64, f64) + 'static>(&self, f: F) {
        self.imp().set_context_menu_callback(f);
    }

    /// Called at the end of a `LayoutMode::Free` drag that actually moved
    /// one or more elements (each entry: element id, new x/y in document
    /// space) — spec §8 manual positioning, extended by spec §5 to move a
    /// whole multi-selection together when the dragged element is part of
    /// one. Never fires outside Free mode, or for a drag that starts on
    /// empty canvas space, or one that ends back where it started.
    pub fn connect_move<F: Fn(Vec<(Uuid, f64, f64)>) + 'static>(&self, f: F) {
        self.imp().set_move_callback(f);
    }

    /// The ids of the currently selected elements (spec §5: multi-select).
    /// Selection is this widget's own transient UI state, not part of the
    /// undoable `Document` — clicking, Shift-clicking or marquee-dragging
    /// updates it directly, with no undo step of its own.
    pub fn selected_ids(&self) -> std::collections::HashSet<Uuid> {
        self.imp().selected.borrow().clone()
    }

    /// Called at the end of a `LayoutMode::Free` corner-handle drag that
    /// actually resized an element (its index into `Document.elements`,
    /// and its complete new transform — width/height and, unless the
    /// bottom-right corner was grabbed, x/y too, since the opposite corner
    /// stays anchored). Never fires outside Free mode or for a drag that
    /// ends back where it started.
    pub fn connect_resize<F: Fn(usize, screenforge_core::model::Transform) + 'static>(&self, f: F) {
        self.imp().set_resize_callback(f);
    }
}

impl Default for Canvas {
    fn default() -> Self {
        Self::new()
    }
}

mod imp {
    use std::cell::{Cell, RefCell};
    use std::collections::{HashMap, HashSet};

    use gtk4::cairo;
    use gtk4::gdk;
    use gtk4::glib;
    use gtk4::graphene;
    use gtk4::prelude::*;
    use gtk4::subclass::prelude::*;
    use screenforge_core::layout::Placement;
    use screenforge_core::model::{Corner, Document, LayoutMode, Transform};
    use screenforge_core::snap::{self, Guide};
    use uuid::Uuid;

    type ReorderCallback = Box<dyn Fn(usize, usize)>;
    type ContextMenuCallback = Box<dyn Fn(usize, f64, f64)>;
    type MoveCallback = Box<dyn Fn(Vec<(Uuid, f64, f64)>)>;
    type ResizeCallback = Box<dyn Fn(usize, Transform)>;
    /// `(drag-start point, moved elements' own (id, original x, original y))`
    /// — one entry per element for a multi-selection group move, or a
    /// single entry for an ordinary single-element move.
    type MoveDragOrigin = ((f64, f64), Vec<(Uuid, f64, f64)>);
    /// `(corner grabbed, dragged element's original transform, drag-start
    /// point in document space)`.
    type ResizeDragOrigin = (Corner, Transform, (f64, f64));

    /// How close (in *screen* pixels, regardless of zoom) a press has to
    /// land to a Free-mode element's corner to grab it for resizing rather
    /// than moving the whole element.
    const HANDLE_HIT_RADIUS_PX: f64 = 10.0;
    /// Half the side length of the little squares `snapshot()` draws at
    /// each Free-mode element's corners.
    const HANDLE_DRAW_HALF_PX: f64 = 5.0;
    /// How close (in *screen* pixels, regardless of zoom) a Free-mode move
    /// has to come to another element's edge/center, or the canvas's own
    /// edge/center, before it snaps into exact alignment.
    const SNAP_THRESHOLD_PX: f64 = 8.0;

    pub struct Canvas {
        document: RefCell<Document>,
        resolved_images: RefCell<HashMap<Uuid, cairo::ImageSurface>>,
        background_image: RefCell<Option<cairo::ImageSurface>>,
        /// The last rendered composition, cached so `snapshot()` doesn't
        /// re-run `compose()` on every frame — only when content changes or
        /// the widget is resized (which changes the "fit to window" scale).
        cached: RefCell<Option<(cairo::ImageSurface, i32, i32)>>,
        /// Rendered shadow bitmaps, kept across `ensure_rendered()` calls
        /// (unlike `cached` above, which holds the *whole* composited
        /// frame) so a move/reorder drag — which re-composites the whole
        /// document every frame via `content_dirty` — reuses each
        /// element's shadow bitmap instead of re-blurring it on every
        /// single mouse-move event. See `screenforge_core::shadow_cache`.
        shadow_cache: screenforge_core::shadow_cache::ShadowCache,
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
        /// Set instead of `drag_hover` when the drag that picked up
        /// `drag_from` is a `LayoutMode::Free` move rather than a reorder:
        /// the document-space point where the drag began, and the moved
        /// elements' own transform.x/y at that moment (one entry, or one
        /// per selected element for a group move). Together they turn the
        /// gesture's cumulative offset into each element's absolute new
        /// position.
        move_drag_origin: RefCell<Option<MoveDragOrigin>>,
        /// Set instead of `move_drag_origin` when the drag that picked up
        /// `drag_from` grabbed one of that element's corner handles —
        /// checked first, so grabbing near a corner always resizes rather
        /// than moves.
        resize_drag_origin: Cell<Option<ResizeDragOrigin>>,
        /// Alignment guides from the current Free-mode move drag, drawn by
        /// `snapshot()`; empty outside of a move drag that's actually
        /// snapped to something. Only a single-element move snaps — a
        /// group move's relative positions would make "snap to another
        /// selected element" ambiguous, so it's skipped for those.
        active_guides: RefCell<Vec<Guide>>,
        /// Ids of the currently selected elements (spec §5: multi-select).
        /// Transient UI state owned entirely by this widget, not the
        /// document — see `Canvas::selected_ids`.
        pub(super) selected: RefCell<HashSet<Uuid>>,
        /// Document-space start/current points of an in-progress
        /// empty-space drag that selects every element it overlaps on
        /// release (spec §5: marquee select). `None` outside such a drag.
        marquee_start: Cell<Option<(f64, f64)>>,
        marquee_current: Cell<Option<(f64, f64)>>,
        /// Whether Shift was held when the marquee drag began — an
        /// additive marquee extends the existing selection instead of
        /// replacing it.
        marquee_additive: Cell<bool>,
        reorder_callback: RefCell<Option<ReorderCallback>>,
        context_menu_callback: RefCell<Option<ContextMenuCallback>>,
        move_callback: RefCell<Option<MoveCallback>>,
        resize_callback: RefCell<Option<ResizeCallback>>,
    }

    impl Default for Canvas {
        fn default() -> Self {
            Self {
                document: RefCell::new(Document::new()),
                resolved_images: RefCell::new(HashMap::new()),
                background_image: RefCell::new(None),
                cached: RefCell::new(None),
                shadow_cache: screenforge_core::shadow_cache::ShadowCache::new(),
                content_dirty: Cell::new(true),
                drag_active: Cell::new(false),
                manual_zoom: Cell::new(None),
                last_placements: RefCell::new(Vec::new()),
                last_scale: Cell::new(1.0),
                drag_from: Cell::new(None),
                drag_hover: Cell::new(None),
                move_drag_origin: RefCell::new(None),
                resize_drag_origin: Cell::new(None),
                active_guides: RefCell::new(Vec::new()),
                selected: RefCell::new(HashSet::new()),
                marquee_start: Cell::new(None),
                marquee_current: Cell::new(None),
                marquee_additive: Cell::new(false),
                reorder_callback: RefCell::new(None),
                context_menu_callback: RefCell::new(None),
                move_callback: RefCell::new(None),
                resize_callback: RefCell::new(None),
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
                move |gesture, x, y| {
                    let shift = gesture.current_event_state().contains(gdk::ModifierType::SHIFT_MASK);
                    obj.imp().on_drag_begin(x, y, shift);
                }
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

            let secondary_click = gtk4::GestureClick::new();
            secondary_click.set_button(gtk4::gdk::BUTTON_SECONDARY);
            secondary_click.connect_released(glib::clone!(
                #[weak]
                obj,
                move |_, _n_press, x, y| obj.imp().on_secondary_click(x, y)
            ));
            obj.add_controller(secondary_click);
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

                // Selection highlight (spec §5) — drawn in every layout
                // mode, not just Free, since selecting/deleting several
                // screenshots at once is useful regardless of how they're
                // arranged.
                {
                    let doc = self.document.borrow();
                    let selected = self.selected.borrow();
                    if !selected.is_empty() {
                        let scale = self.last_scale.get();
                        let placements = self.last_placements.borrow();
                        for (el, p) in doc.elements.iter().zip(placements.iter()) {
                            if !selected.contains(&el.id) {
                                continue;
                            }
                            let inset = 3.0;
                            let sx = offset_x + p.x * scale - inset;
                            let sy = offset_y + p.y * scale - inset;
                            let sw = p.width * scale + inset * 2.0;
                            let sh = p.height * scale + inset * 2.0;
                            ctx.rectangle(sx, sy, sw, sh);
                            ctx.set_source_rgba(0.95, 0.6, 0.1, 0.95);
                            ctx.set_line_width(2.5);
                            let _ = ctx.stroke();
                        }
                    }
                }

                // Resize handles at every element's corners — only in Free
                // mode, where dragging one actually does something (spec
                // §8). Drawn for every element regardless of selection —
                // any element can still be resized individually.
                if self.document.borrow().layout.mode == LayoutMode::Free {
                    let scale = self.last_scale.get();
                    for p in self.last_placements.borrow().iter() {
                        for (cx, cy) in
                            [(p.x, p.y), (p.x + p.width, p.y), (p.x, p.y + p.height), (p.x + p.width, p.y + p.height)]
                        {
                            let (sx, sy) = (offset_x + cx * scale, offset_y + cy * scale);
                            ctx.rectangle(
                                sx - HANDLE_DRAW_HALF_PX,
                                sy - HANDLE_DRAW_HALF_PX,
                                HANDLE_DRAW_HALF_PX * 2.0,
                                HANDLE_DRAW_HALF_PX * 2.0,
                            );
                            ctx.set_source_rgba(1.0, 1.0, 1.0, 1.0);
                            let _ = ctx.fill_preserve();
                            ctx.set_source_rgba(0.29, 0.56, 0.89, 1.0);
                            ctx.set_line_width(1.5);
                            let _ = ctx.stroke();
                        }
                    }
                }

                // Alignment guides from an in-progress Free-mode move.
                let scale = self.last_scale.get();
                ctx.set_source_rgba(0.95, 0.25, 0.55, 0.95);
                ctx.set_line_width(1.0);
                for guide in self.active_guides.borrow().iter() {
                    match *guide {
                        Guide::Vertical(doc_x) => {
                            let sx = offset_x + doc_x * scale;
                            ctx.move_to(sx, offset_y);
                            ctx.line_to(sx, offset_y + render_h as f64);
                        }
                        Guide::Horizontal(doc_y) => {
                            let sy = offset_y + doc_y * scale;
                            ctx.move_to(offset_x, sy);
                            ctx.line_to(offset_x + render_w as f64, sy);
                        }
                    }
                }
                let _ = ctx.stroke();

                // The marquee-select rectangle, if a select-drag is in
                // progress (spec §5) — drawn last so it stays on top of
                // everything else.
                if let (Some((sx0, sy0)), Some((sx1, sy1))) = (self.marquee_start.get(), self.marquee_current.get()) {
                    let (x0, x1) = (sx0.min(sx1), sx0.max(sx1));
                    let (y0, y1) = (sy0.min(sy1), sy0.max(sy1));
                    let rx = offset_x + x0 * scale;
                    let ry = offset_y + y0 * scale;
                    let rw = (x1 - x0) * scale;
                    let rh = (y1 - y0) * scale;
                    ctx.rectangle(rx, ry, rw, rh);
                    ctx.set_source_rgba(0.29, 0.56, 0.89, 0.15);
                    let _ = ctx.fill_preserve();
                    ctx.set_source_rgba(0.29, 0.56, 0.89, 0.9);
                    ctx.set_line_width(1.5);
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
        pub fn set_document(
            &self,
            document: Document,
            resolved_images: HashMap<Uuid, cairo::ImageSurface>,
            background_image: Option<cairo::ImageSurface>,
        ) {
            // Drop selected ids the new document no longer has (deleted,
            // or undone/redone past their existence) — otherwise a stale
            // id could linger in `selected` forever, invisible but still
            // affecting a later group move.
            let valid_ids: HashSet<Uuid> = document.elements.iter().map(|e| e.id).collect();
            self.selected.borrow_mut().retain(|id| valid_ids.contains(id));
            *self.document.borrow_mut() = document;
            *self.resolved_images.borrow_mut() = resolved_images;
            *self.background_image.borrow_mut() = background_image;
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

        pub fn set_context_menu_callback<F: Fn(usize, f64, f64) + 'static>(&self, f: F) {
            *self.context_menu_callback.borrow_mut() = Some(Box::new(f));
        }

        pub fn set_move_callback<F: Fn(Vec<(Uuid, f64, f64)>) + 'static>(&self, f: F) {
            *self.move_callback.borrow_mut() = Some(Box::new(f));
        }

        pub fn set_resize_callback<F: Fn(usize, Transform) + 'static>(&self, f: F) {
            *self.resize_callback.borrow_mut() = Some(Box::new(f));
        }

        pub(super) fn on_secondary_click(&self, x: f64, y: f64) {
            let Some(index) = self.widget_to_document(x, y).and_then(|(dx, dy)| self.element_index_at(dx, dy)) else {
                return;
            };
            if let Some(cb) = self.context_menu_callback.borrow().as_ref() {
                cb(index, x, y);
            }
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
            let background_image = self.background_image.borrow();
            if let Err(err) =
                screenforge_core::render::compose(&doc, &surface, scale, &resolved, background_image.as_ref(), &self.shadow_cache)
            {
                eprintln!("ScreenForge: render error: {err}");
                return;
            }
            drop(background_image);
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

        /// Which element's corner handle (if any) covers `(doc_x, doc_y)` —
        /// only meaningful in `LayoutMode::Free`, where `snapshot()` draws
        /// these handles in the first place. `HANDLE_HIT_RADIUS_PX` is a
        /// screen-pixel radius, so it's converted through `last_scale`
        /// first to stay a constant on-screen hit target at any zoom level.
        fn corner_handle_at(&self, doc_x: f64, doc_y: f64) -> Option<(usize, Corner)> {
            let scale = self.last_scale.get();
            if scale <= 0.0 {
                return None;
            }
            let tolerance = HANDLE_HIT_RADIUS_PX / scale;
            let tolerance_sq = tolerance * tolerance;

            for (index, p) in self.last_placements.borrow().iter().enumerate() {
                let corners = [
                    (Corner::TopLeft, p.x, p.y),
                    (Corner::TopRight, p.x + p.width, p.y),
                    (Corner::BottomLeft, p.x, p.y + p.height),
                    (Corner::BottomRight, p.x + p.width, p.y + p.height),
                ];
                for (corner, cx, cy) in corners {
                    let (dx, dy) = (doc_x - cx, doc_y - cy);
                    if dx * dx + dy * dy <= tolerance_sq {
                        return Some((index, corner));
                    }
                }
            }
            None
        }

        /// Mutates one element's live position in place during a move drag
        /// (looked up by id, since a group move touches several elements
        /// at once and their positions in `Document.elements` aren't
        /// necessarily contiguous or known up front).
        fn apply_live_position(&self, id: Uuid, x: f64, y: f64) {
            if let Some(el) = self.document.borrow_mut().elements.iter_mut().find(|e| e.id == id) {
                el.transform.x = x;
                el.transform.y = y;
            }
        }

        /// Where `doc_x` would be inserted among the current placements, as
        /// a position in `0..=last_placements.len()` (not yet adjusted for
        /// the removal of the dragged element — see `on_drag_end`).
        fn insertion_index_at(&self, doc_x: f64) -> usize {
            let placements = self.last_placements.borrow();
            placements.iter().position(|p| doc_x < p.x + p.width / 2.0).unwrap_or(placements.len())
        }

        /// Snaps a Free-mode move's proposed `(x, y)` against every other
        /// element and the canvas bounds/center, recording the resulting
        /// guides for `snapshot()` to draw. `index` is excluded from its own
        /// candidate set — an element obviously always aligns with itself.
        fn snapped_move(&self, index: usize, proposed: (f64, f64)) -> (f64, f64) {
            let scale = self.last_scale.get();
            if scale <= 0.0 {
                self.active_guides.borrow_mut().clear();
                return proposed;
            }

            let doc = self.document.borrow();
            let Some(moving_el) = doc.elements.get(index) else {
                drop(doc);
                self.active_guides.borrow_mut().clear();
                return proposed;
            };
            let moving = snap::Rect { x: proposed.0, y: proposed.1, width: moving_el.transform.width, height: moving_el.transform.height };
            let others: Vec<snap::Rect> = doc
                .elements
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != index)
                .map(|(_, el)| snap::Rect { x: el.transform.x, y: el.transform.y, width: el.transform.width, height: el.transform.height })
                .collect();
            let (canvas_w, canvas_h) = (doc.canvas.export_width as f64, doc.canvas.export_height as f64);
            drop(doc);

            let result = snap::snap_position(moving, &others, canvas_w, canvas_h, SNAP_THRESHOLD_PX / scale);
            *self.active_guides.borrow_mut() = result.guides;
            (result.x, result.y)
        }

        pub(super) fn on_drag_begin(&self, x: f64, y: f64, shift: bool) {
            self.active_guides.borrow_mut().clear();
            let Some((doc_x, doc_y)) = self.widget_to_document(x, y) else { return };
            let free_mode = self.document.borrow().layout.mode == LayoutMode::Free;

            if free_mode {
                if let Some((index, corner)) = self.corner_handle_at(doc_x, doc_y) {
                    let original = self.document.borrow().elements[index].transform;
                    self.drag_from.set(Some(index));
                    self.resize_drag_origin.set(Some((corner, original, (doc_x, doc_y))));
                    return;
                }
            }

            let hit = self.element_index_at(doc_x, doc_y);

            let Some(index) = hit else {
                // Empty canvas space: start a marquee-select drag rather
                // than any move/resize/reorder (spec §5).
                self.drag_from.set(None);
                self.marquee_start.set(Some((doc_x, doc_y)));
                self.marquee_current.set(Some((doc_x, doc_y)));
                self.marquee_additive.set(shift);
                return;
            };
            let id = self.document.borrow().elements[index].id;

            if shift {
                // A Shift-click purely toggles selection membership — it
                // never also starts a move/reorder/resize, so a
                // deselecting click can't simultaneously drag the element
                // away.
                let mut selected = self.selected.borrow_mut();
                if !selected.remove(&id) {
                    selected.insert(id);
                }
                drop(selected);
                self.drag_from.set(None);
                self.obj().queue_draw();
                return;
            }

            let already_in_group = {
                let selected = self.selected.borrow();
                selected.contains(&id) && selected.len() > 1
            };
            if !already_in_group {
                *self.selected.borrow_mut() = std::iter::once(id).collect();
            }
            self.obj().queue_draw();

            if free_mode {
                let moving_ids: Vec<Uuid> =
                    if already_in_group { self.selected.borrow().iter().copied().collect() } else { vec![id] };
                let origins: Vec<(Uuid, f64, f64)> = {
                    let doc = self.document.borrow();
                    moving_ids
                        .iter()
                        .filter_map(|mid| doc.elements.iter().find(|e| e.id == *mid).map(|e| (*mid, e.transform.x, e.transform.y)))
                        .collect()
                };
                self.drag_from.set(Some(index));
                *self.move_drag_origin.borrow_mut() = Some(((doc_x, doc_y), origins));
                return;
            }

            self.drag_from.set(Some(index));
            self.drag_hover.set(Some(index));
        }

        pub(super) fn on_drag_update(&self, abs_x: f64, abs_y: f64) {
            if let Some((corner, original, (start_x, start_y))) = self.resize_drag_origin.get() {
                let Some(index) = self.drag_from.get() else { return };
                if let Some((doc_x, doc_y)) = self.widget_to_document(abs_x, abs_y) {
                    if let Some(el) = self.document.borrow_mut().elements.get_mut(index) {
                        el.transform = original.resized_from_corner(corner, doc_x - start_x, doc_y - start_y);
                    }
                    self.content_dirty.set(true);
                    self.obj().queue_draw();
                }
                return;
            }

            if let Some(((start_x, start_y), origins)) = self.move_drag_origin.borrow().clone() {
                if let Some((doc_x, doc_y)) = self.widget_to_document(abs_x, abs_y) {
                    let (dx, dy) = (doc_x - start_x, doc_y - start_y);
                    if origins.len() == 1 {
                        let (id, orig_x, orig_y) = origins[0];
                        let index = self.drag_from.get();
                        let proposed = (orig_x + dx, orig_y + dy);
                        let (new_x, new_y) = index.map(|i| self.snapped_move(i, proposed)).unwrap_or(proposed);
                        self.apply_live_position(id, new_x, new_y);
                    } else {
                        // A group move skips snapping (see `active_guides`'
                        // doc comment) but still needs the guides cleared,
                        // since a single-element move earlier in the same
                        // drag session may have left some behind.
                        self.active_guides.borrow_mut().clear();
                        for (id, orig_x, orig_y) in &origins {
                            self.apply_live_position(*id, orig_x + dx, orig_y + dy);
                        }
                    }
                    self.content_dirty.set(true);
                    self.obj().queue_draw();
                }
                return;
            }

            if self.marquee_start.get().is_some() {
                if let Some((doc_x, doc_y)) = self.widget_to_document(abs_x, abs_y) {
                    self.marquee_current.set(Some((doc_x, doc_y)));
                    self.obj().queue_draw();
                }
                return;
            }

            if self.drag_from.get().is_none() {
                return;
            }
            if let Some((doc_x, _)) = self.widget_to_document(abs_x, abs_y) {
                self.drag_hover.set(Some(self.insertion_index_at(doc_x)));
                self.obj().queue_draw();
            }
        }

        pub(super) fn on_drag_end(&self, abs_x: f64, abs_y: f64) {
            if let Some((corner, original, (start_x, start_y))) = self.resize_drag_origin.take() {
                let Some(index) = self.drag_from.take() else { return };
                if let Some((doc_x, doc_y)) = self.widget_to_document(abs_x, abs_y) {
                    let new = original.resized_from_corner(corner, doc_x - start_x, doc_y - start_y);
                    if new != original {
                        if let Some(cb) = self.resize_callback.borrow().as_ref() {
                            cb(index, new);
                        }
                    }
                }
                return;
            }

            if let Some(((start_x, start_y), origins)) = self.move_drag_origin.take() {
                let index = self.drag_from.take();
                self.active_guides.borrow_mut().clear();
                if let Some((doc_x, doc_y)) = self.widget_to_document(abs_x, abs_y) {
                    let (dx, dy) = (doc_x - start_x, doc_y - start_y);
                    let finals: Vec<(Uuid, f64, f64)> = if origins.len() == 1 {
                        let (id, orig_x, orig_y) = origins[0];
                        let proposed = (orig_x + dx, orig_y + dy);
                        let (new_x, new_y) = index.map(|i| self.snapped_move(i, proposed)).unwrap_or(proposed);
                        if new_x == orig_x && new_y == orig_y { Vec::new() } else { vec![(id, new_x, new_y)] }
                    } else if dx == 0.0 && dy == 0.0 {
                        Vec::new()
                    } else {
                        origins.iter().map(|(id, orig_x, orig_y)| (*id, orig_x + dx, orig_y + dy)).collect()
                    };
                    if !finals.is_empty() {
                        if let Some(cb) = self.move_callback.borrow().as_ref() {
                            cb(finals);
                        }
                    }
                }
                self.obj().queue_draw();
                return;
            }

            if let Some((sx, sy)) = self.marquee_start.take() {
                let additive = self.marquee_additive.get();
                self.marquee_current.set(None);
                if let Some((ex, ey)) = self.widget_to_document(abs_x, abs_y) {
                    let (x0, x1) = (sx.min(ex), sx.max(ex));
                    let (y0, y1) = (sy.min(ey), sy.max(ey));
                    let doc = self.document.borrow();
                    let placements = self.last_placements.borrow();
                    let hits: Vec<Uuid> = doc
                        .elements
                        .iter()
                        .zip(placements.iter())
                        .filter(|(_, p)| p.x < x1 && p.x + p.width > x0 && p.y < y1 && p.y + p.height > y0)
                        .map(|(el, _)| el.id)
                        .collect();
                    drop(placements);
                    drop(doc);
                    let mut selected = self.selected.borrow_mut();
                    if !additive {
                        selected.clear();
                    }
                    selected.extend(hits);
                }
                self.obj().queue_draw();
                return;
            }

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
            // A cancelled Free-mode drag (move or resize) needs its in-place
            // preview mutation undone explicitly — unlike reorder's hover
            // indicator, the element's transform was mutated live during the
            // drag, and nothing else will put it back until the next
            // `set_document`.
            self.active_guides.borrow_mut().clear();
            let index = self.drag_from.take();
            let resize_origin = self.resize_drag_origin.take();
            let move_origin = self.move_drag_origin.take();
            self.marquee_start.set(None);
            self.marquee_current.set(None);

            if let (Some(index), Some((_, original, _))) = (index, resize_origin) {
                if let Some(el) = self.document.borrow_mut().elements.get_mut(index) {
                    el.transform = original;
                }
                self.content_dirty.set(true);
            } else if let Some((_, origins)) = move_origin {
                for (id, orig_x, orig_y) in origins {
                    self.apply_live_position(id, orig_x, orig_y);
                }
                self.content_dirty.set(true);
            }
            self.drag_hover.set(None);
            self.obj().queue_draw();
        }
    }
}
