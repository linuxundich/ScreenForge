use crate::*;

/// Repopulates a `LabelStyle`-editing group of sidebar/dialog rows from a
/// given style — see `build_label_style_groups`'s own doc comment.
pub(crate) type LabelStyleSync = Rc<dyn Fn(&LabelStyle)>;

/// Everything import/inspector/export actions mutate. Kept as one `Rc<RefCell<_>>`
/// shared between the window's actions and the canvas widget rather than
/// threaded through every callback individually.
pub(crate) struct EditorState {
    pub(crate) document: Document,
    /// Decoded bytes keyed by *source path*, not by element id. Keying by
    /// path means "replace screenshot" and its undo/redo never need to
    /// touch this cache at all: whichever path an element's `source`
    /// currently names (old or new, before or after undo) is simply looked
    /// up here, decoding on first use — self-healing, and it's also why a
    /// duplicate that shares a source path costs no extra decode.
    pub(crate) image_cache: HashMap<PathBuf, DecodedImage>,
    /// Where this project was last saved to or loaded from, if anywhere.
    /// `win.save` reuses it; `win.save-as` always prompts and updates it.
    pub(crate) project_path: Option<PathBuf>,
    /// GTK-independent undo/redo history (spec §17), kept in `app` rather
    /// than inside `Document` itself — see `core::command` for why.
    pub(crate) undo_stack: UndoStack,
    /// Set for the duration of [`sync_controls_from_document`]. Every
    /// control's change handler checks this first and bails out if set —
    /// sync writes several widgets one at a time (e.g. background type,
    /// then color 1, then color 2, then angle), and without this guard each
    /// intermediate, only-partially-synced write would re-fire its handler
    /// and push a spurious undo command built from a mix of old and new
    /// values, corrupting the very history the sync was restoring.
    pub(crate) syncing_controls: bool,
    /// Bumped on every "Generate from screenshots" click, and fed to
    /// `screenforge_core::palette::suggest_gradient` as its seed — this is
    /// what makes clicking the button again ("Regenerate") suggest a
    /// different palette each time rather than the same one, with no
    /// external RNG state needed. Transient UI convenience, not saved with
    /// the project.
    pub(crate) gradient_auto_seed: u32,
    /// Set by the header bar's "Screenshots ausblenden" toggle. Purely a
    /// preview convenience for judging a generated/gradient background
    /// without the screenshots on top of it — `refresh_canvas` skips
    /// drawing elements while this is set, but never touches
    /// `document.elements` or its `visible` flags, so it leaves undo
    /// history and the saved project completely untouched.
    pub(crate) hide_screenshots: bool,
    /// Repopulates the sidebar's project-wide label-style rows
    /// (`register_label_style_controls`) from a given `LabelStyle` — set
    /// once during that function's own setup and called from
    /// `sync_controls_from_document` afterward, since (unlike the
    /// Settings dialog's copy of the same rows) the sidebar's widgets live
    /// for the whole session and must reflect undo/redo, project load,
    /// preset apply, and canvas label-drags. `None` only during the brief
    /// window before that setup runs.
    pub(crate) label_style_sync: Option<LabelStyleSync>,
    /// Bumped by "Neue Varianten" so the variant grid shows a fresh,
    /// still deterministic set. Transient, not saved.
    pub(crate) variant_roll: u64,
}

impl EditorState {
    /// A fresh document seeded from the user's saved preferences (default
    /// spacing/margin/export quality, and the global default label style)
    /// rather than `LayoutSettings`'s/`LabelStyle`'s own hardcoded
    /// defaults — see `app_settings()`. This is the *only* place the
    /// global `default-label-style` setting is ever read: from here on,
    /// `document.label_defaults` is this specific document's own
    /// independent copy (spec: "Eine Änderung der globalen
    /// Anwendungseinstellungen darf bestehende Projekte nicht automatisch
    /// verändern") — changing the global setting later, or loading a
    /// different/older project, never reaches back into it.
    pub(crate) fn new() -> Self {
        let settings = app_settings();
        let mut document = Document::new();
        document.layout.spacing_px = settings.double("default-spacing");
        document.layout.margin_x = settings.double("default-margin-x");
        document.layout.margin_y = settings.double("default-margin-y");
        document.canvas.export_quality = settings.double("default-export-quality").round().clamp(1.0, 100.0) as u8;
        document.label_defaults = load_global_label_defaults();
        Self {
            document,
            image_cache: HashMap::new(),
            project_path: None,
            undo_stack: UndoStack::new(),
            syncing_controls: false,
            gradient_auto_seed: 0,
            hide_screenshots: false,
            label_style_sync: None,
            variant_roll: 0,
        }
    }
}

/// The app's `GSettings` handle (spec: a preferences page backed by
/// GSettings). There's no packaged/installed build yet that would put the
/// compiled schema where GLib normally looks
/// (`$XDG_DATA_DIRS/glib-2.0/schemas/`), so `main()` points
/// `GSETTINGS_SCHEMA_DIR` at the copy `build.rs` compiles into `OUT_DIR`
/// before this is ever called — confirmed empirically that GLib treats
/// that env var as an additional search path, not a replacement, so this
/// needs no system-wide installation for `cargo run`.
pub(crate) fn app_settings() -> gio::Settings {
    gio::Settings::new(APP_ID)
}

/// Looks up `path` in `cache`, decoding and inserting it on first use.
/// `None` only if decoding fails (missing/corrupt file).
pub(crate) fn get_or_decode<'a>(cache: &'a mut HashMap<PathBuf, DecodedImage>, path: &Path) -> Option<&'a DecodedImage> {
    if !cache.contains_key(path) {
        match import::decode_image(path) {
            Ok(image) => {
                cache.insert(path.to_path_buf(), image);
            }
            Err(err) => {
                eprintln!("ScreenForge: failed to decode {}: {err}", path.display());
                return None;
            }
        }
    }
    cache.get(path)
}

/// Builds a fresh `cairo::ImageSurface` per *currently referenced* image
/// (decoding on demand via `image_cache`, so this also self-heals after
/// undoing/redoing a "replace screenshot") and hands the result to the
/// canvas widget. Also refreshes the export sidebar's read-only computed-
/// height display, since `document.canvas` (the content-fitted native
/// size — see `fit_canvas_to_content`) can change on essentially any edit,
/// not just the ones that go through `sync_controls_from_document`.
pub(crate) fn refresh_canvas(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let mut state_ref = state.borrow_mut();
    let surfaces = element_surfaces(&mut state_ref);
    update_window_title(window, &state_ref);
    let is_empty = state_ref.document.elements.is_empty();
    window.empty_state().set_visible(is_empty);
    window.canvas_toolbar().set_visible(!is_empty);
    canvas.set_visible(!is_empty);
    let EditorState { document, image_cache, hide_screenshots, .. } = &mut *state_ref;
    let background_image = background_image_path(&document.background)
        .and_then(|path| get_or_decode(image_cache, &path))
        .and_then(|image| import::surface_from_decoded(image).ok());
    let canvas_settings = document.canvas;
    // "Screenshots ausblenden" only ever affects what gets handed to the
    // canvas widget for this one render — `document` itself (and therefore
    // undo history and the saved project) is never touched.
    let mut doc_for_render = document.clone();
    if *hide_screenshots {
        doc_for_render.elements.clear();
    }
    canvas.set_document(doc_for_render, surfaces, background_image);
    drop(state_ref);

    update_export_height_display(window, canvas_settings);
}

/// Decoded Cairo surfaces for every element whose source is a file,
/// keyed by element id, as `screenforge_core::render::compose` expects.
pub(crate) fn element_surfaces(state: &mut EditorState) -> HashMap<Uuid, cairo::ImageSurface> {
    let EditorState { document, image_cache, .. } = state;
    let mut surfaces = HashMap::new();
    for element in &document.elements {
        let ImageSource::Path(path) = &element.source else { continue };
        if let Some(image) = get_or_decode(image_cache, path) {
            if let Ok(surface) = import::surface_from_decoded(image) {
                surfaces.insert(element.id, surface);
            }
        }
    }
    surfaces
}

/// Header bar title: the project's file name (or "Neues Projekt"), and as
/// subtitle how many screenshots it holds and the export size.
fn update_window_title(window: &Window, state: &EditorState) {
    let title = state
        .project_path
        .as_ref()
        .and_then(|p| p.file_stem())
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Neues Projekt".to_owned());
    let count = state.document.elements.len();
    let subtitle = if count == 0 {
        String::new()
    } else {
        let c = state.document.canvas;
        let scale = c.export_target_width as f64 / c.export_width.max(1) as f64;
        let height = (c.export_height as f64 * scale).round().max(1.0);
        let noun = if count == 1 { "Screenshot" } else { "Screenshots" };
        format!("{count} {noun} · {} × {height:.0} px", c.export_target_width)
    };
    let title_widget = window.window_title();
    title_widget.set_title(&title);
    title_widget.set_subtitle(&subtitle);
}

/// The output height that results from scaling `canvas_settings`'s
/// content-fitted native size to its target export width, shown read-only
/// in the sidebar (see `export_height_row`'s `sensitive: false`).
pub(crate) fn update_export_height_display(window: &Window, canvas_settings: screenforge_core::model::CanvasSettings) {
    let scale = canvas_settings.export_target_width as f64 / canvas_settings.export_width.max(1) as f64;
    let height = (canvas_settings.export_height as f64 * scale).round().max(1.0);
    window.export_height_row().set_value(height);
}

/// The path to decode for `Background::Image`, if the background is that
/// variant and its source is (as always today) a plain file path.
pub(crate) fn background_image_path(background: &Background) -> Option<PathBuf> {
    let Background::Image(spec) = background else { return None };
    let ImageSource::Path(path) = &spec.source else { return None };
    Some(path.clone())
}

pub(crate) fn gdk_rgba_from(c: &Rgba) -> gdk::RGBA {
    gdk::RGBA::new(c.r as f32, c.g as f32, c.b as f32, c.a as f32)
}

pub(crate) fn rgba_from_gdk(c: &gdk::RGBA) -> Rgba {
    Rgba::new(c.red() as f64, c.green() as f64, c.blue() as f64, c.alpha() as f64)
}

/// A titled `AdwSpinRow` with a plain numeric adjustment — shared by every
/// callout row's "Eckenradius"/"Pfeilbreite" controls.
pub(crate) fn spin_row(title: &str, lower: f64, upper: f64, value: f64) -> adw::SpinRow {
    let adjustment = gtk4::Adjustment::new(value, lower, upper, 1.0, 10.0, 0.0);
    let row = adw::SpinRow::new(Some(&adjustment), 1.0, 1);
    row.set_title(title);
    row
}

/// Reflects `undo_stack.can_undo()/can_redo()` onto the `win.undo`/`win.redo`
/// `GSimpleAction`s. The header-bar buttons are bound to these actions via
/// `action-name` in the `.ui` file, so disabling the action alone is enough
/// to grey out the button — no separate widget bookkeeping needed.
pub(crate) fn update_undo_redo_sensitivity(window: &Window, state: &Rc<RefCell<EditorState>>) {
    let state_ref = state.borrow();
    if let Some(action) = window.lookup_action("undo").and_downcast::<gio::SimpleAction>() {
        action.set_enabled(state_ref.undo_stack.can_undo());
    }
    if let Some(action) = window.lookup_action("redo").and_downcast::<gio::SimpleAction>() {
        action.set_enabled(state_ref.undo_stack.can_redo());
    }
}
