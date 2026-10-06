mod adb;
mod canvas;
mod export;
mod import;
mod window;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gtk4::gdk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use screenforge_core::command::{
    AddCallout, AddScreenshots, ApplyTemplate, Command, DuplicateScreenshot, EnterFreeLayout, RemoveCallout, RemoveScreenshot,
    RemoveScreenshots, ReorderScreenshot, ReplaceScreenshotSource, SetBackground, SetCallout, SetCornerRadiusForAllElements, SetLabelDefaults,
    SetLayoutMode, SetMarginX, SetMarginY, SetScreenshotLabel, SetShadowForAllElements, SetSpacing, SetTransform, SetTransforms, UndoStack,
};
use screenforge_core::model::{
    Background, BackgroundImageFit, Callout, ColorStrategy, CornerRadius, Document, ExportFormat, GeneratedBackground, GeneratorStyle, GradientKind, Mood,
    GradientSpec, HorizontalAnchor, ImageBackgroundSpec, ImageSource, Label, LabelStyle, LayoutMode, Rgba, ScreenshotElement, ShadowParams,
    ShadowPreset, TextAlign, TextBackground, TextPosition, Typography, VerticalAnchor,
};
use uuid::Uuid;

use canvas::Canvas;
use import::DecodedImage;
use window::Window;

const APP_ID: &str = "de.christophlangner.ScreenForge";

/// Repopulates a `LabelStyle`-editing group of sidebar/dialog rows from a
/// given style — see `build_label_style_groups`'s own doc comment.
type LabelStyleSync = Rc<dyn Fn(&LabelStyle)>;

/// Everything import/inspector/export actions mutate. Kept as one `Rc<RefCell<_>>`
/// shared between the window's actions and the canvas widget rather than
/// threaded through every callback individually.
struct EditorState {
    document: Document,
    /// Decoded bytes keyed by *source path*, not by element id. Keying by
    /// path means "replace screenshot" and its undo/redo never need to
    /// touch this cache at all: whichever path an element's `source`
    /// currently names (old or new, before or after undo) is simply looked
    /// up here, decoding on first use — self-healing, and it's also why a
    /// duplicate that shares a source path costs no extra decode.
    image_cache: HashMap<PathBuf, DecodedImage>,
    /// Where this project was last saved to or loaded from, if anywhere.
    /// `win.save` reuses it; `win.save-as` always prompts and updates it.
    project_path: Option<PathBuf>,
    /// GTK-independent undo/redo history (spec §17), kept in `app` rather
    /// than inside `Document` itself — see `core::command` for why.
    undo_stack: UndoStack,
    /// Set for the duration of [`sync_controls_from_document`]. Every
    /// control's change handler checks this first and bails out if set —
    /// sync writes several widgets one at a time (e.g. background type,
    /// then color 1, then color 2, then angle), and without this guard each
    /// intermediate, only-partially-synced write would re-fire its handler
    /// and push a spurious undo command built from a mix of old and new
    /// values, corrupting the very history the sync was restoring.
    syncing_controls: bool,
    /// Bumped on every "Generate from screenshots" click, and fed to
    /// `screenforge_core::palette::suggest_gradient` as its seed — this is
    /// what makes clicking the button again ("Regenerate") suggest a
    /// different palette each time rather than the same one, with no
    /// external RNG state needed. Transient UI convenience, not saved with
    /// the project.
    gradient_auto_seed: u32,
    /// Set by the header bar's "Screenshots ausblenden" toggle. Purely a
    /// preview convenience for judging a generated/gradient background
    /// without the screenshots on top of it — `refresh_canvas` skips
    /// drawing elements while this is set, but never touches
    /// `document.elements` or its `visible` flags, so it leaves undo
    /// history and the saved project completely untouched.
    hide_screenshots: bool,
    /// Repopulates the sidebar's project-wide label-style rows
    /// (`register_label_style_controls`) from a given `LabelStyle` — set
    /// once during that function's own setup and called from
    /// `sync_controls_from_document` afterward, since (unlike the
    /// Settings dialog's copy of the same rows) the sidebar's widgets live
    /// for the whole session and must reflect undo/redo, project load,
    /// preset apply, and canvas label-drags. `None` only during the brief
    /// window before that setup runs.
    label_style_sync: Option<LabelStyleSync>,
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
    fn new() -> Self {
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
fn app_settings() -> gio::Settings {
    gio::Settings::new(APP_ID)
}

/// Looks up `path` in `cache`, decoding and inserting it on first use.
/// `None` only if decoding fails (missing/corrupt file).
fn get_or_decode<'a>(cache: &'a mut HashMap<PathBuf, DecodedImage>, path: &Path) -> Option<&'a DecodedImage> {
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

fn main() -> glib::ExitCode {
    // Safety: called before any thread that could race on the environment
    // exists (the very first thing `main` does), and before any GSettings
    // use — see `app_settings()` for why this is here at all.
    unsafe {
        std::env::set_var("GSETTINGS_SCHEMA_DIR", concat!(env!("OUT_DIR"), "/schemas"));
    }

    gio::resources_register_include!("screenforge.gresource").expect("failed to register GResource bundle");

    let app = adw::Application::builder().application_id(APP_ID).build();
    register_about_action(&app);
    app.connect_activate(build_ui);
    app.run()
}

/// Makes the app's own icon (bundled into the gresource — see
/// `resources/screenforge.gresource.xml`'s `icons` gresource, sourced from
/// the GNOME hicolor-theme-shaped files under `data/icons`) resolve by
/// name — `APP_ID` for the full-color app icon, `"{APP_ID}-symbolic"` for
/// the symbolic one — anywhere the running app looks up an icon by name,
/// notably `AboutDialog::application_icon` in `register_about_action`.
/// Needs no install step: `GtkIconTheme` treats a resource path as if it
/// *were itself* the hicolor theme's own root (`$path/scalable/apps/
/// name.svg`, not `$path/hicolor/scalable/apps/name.svg`) — one directory
/// level short of where the bundle's own `hicolor/...` layout actually
/// starts, hence the extra `/hicolor` here even though nothing else
/// (gresource.xml's alias paths, `data/icons`' own layout) needs it. Must
/// run after a display exists (unlike GSettings/gresource registration in
/// `main`, which don't need one), so this is called from `build_ui` rather
/// than `main`.
fn register_app_icon_theme() {
    let Some(display) = gdk::Display::default() else { return };
    gtk4::IconTheme::for_display(&display).add_resource_path("/de/christophlangner/ScreenForge/icons/hicolor");
}

fn build_ui(app: &adw::Application) {
    register_app_icon_theme();
    let window = Window::new(app);
    let canvas = window.canvas();

    let state = Rc::new(RefCell::new(EditorState::new()));
    refresh_canvas(&window, &canvas, &state);

    register_open_action(app, &window, &canvas, &state);
    register_import_android_action(app, &window, &canvas, &state);
    register_adb_watch(&window);
    register_drop_target(&window, &canvas, &state);
    register_layout_controls(&window, &canvas, &state);
    register_effect_controls(&window, &canvas, &state);
    register_generator_controls(&window, &canvas, &state);
    register_label_controls(&window, &canvas, &state);
    register_label_style_controls(&window, &canvas, &state);
    register_settings_action(app);
    register_presets_menu(&window, &canvas, &state);
    register_selection_sync(&window, &canvas, &state);
    register_label_drag(&window, &canvas, &state);
    register_wallpaper_drag(&window, &canvas, &state);
    register_callouts_controls(&window, &canvas, &state);
    register_callout_drag(&window, &canvas, &state);
    register_export_controls(&window, &state);
    register_export_action(app, &window, &state);
    register_project_actions(app, &window, &canvas, &state);
    register_undo_redo_actions(app, &window, &canvas, &state);
    register_zoom_actions(app, &window, &canvas);
    register_reorder(&window, &canvas, &state);
    register_move(&window, &canvas, &state);
    register_alignment_controls(&window, &canvas, &state);
    register_resize(&window, &canvas, &state);
    register_context_menu(&window, &canvas, &state);
    register_delete_selected(app, &window, &canvas, &state);
    register_paste_action(app, &window, &canvas, &state);
    register_hide_screenshots_toggle(&window, &canvas, &state);
    register_sidebar_toggle(&window);
    register_eyedroppers(&window, &canvas);
    register_text_focus_guards(&window);

    window.present();
}

/// Wires the header bar's "Screenshots ausblenden" toggle to
/// `EditorState::hide_screenshots` — a pure preview flag (see its own doc
/// comment), so this never touches the undo stack.
/// Wires the header bar's sidebar-toggle button to `split_view`'s own
/// `show-sidebar` property (not `collapsed`, which is normally
/// breakpoint-driven and doesn't hide anything by itself) — pure UI state,
/// not part of `Document`/undo, so a plain bidirectional property binding
/// is enough: either side (a click, or `show-sidebar` changing for any
/// other reason) stays in sync with no manual `connect_toggled` bookkeeping.
fn register_sidebar_toggle(window: &Window) {
    window.sidebar_toggle_button().set_active(window.split_view().shows_sidebar());
    window.sidebar_toggle_button().bind_property("active", &window.split_view(), "show-sidebar").bidirectional().sync_create().build();
}

fn register_hide_screenshots_toggle(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    window.hide_screenshots_button().connect_toggled(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |button| {
            state.borrow_mut().hide_screenshots = button.is_active();
            refresh_canvas(&window, &canvas, &state);
        }
    ));
}

/// Builds a fresh `cairo::ImageSurface` per *currently referenced* image
/// (decoding on demand via `image_cache`, so this also self-heals after
/// undoing/redoing a "replace screenshot") and hands the result to the
/// canvas widget. Also refreshes the export sidebar's read-only computed-
/// height display, since `document.canvas` (the content-fitted native
/// size — see `fit_canvas_to_content`) can change on essentially any edit,
/// not just the ones that go through `sync_controls_from_document`.
fn refresh_canvas(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let mut state_ref = state.borrow_mut();
    let EditorState { document, image_cache, hide_screenshots, .. } = &mut *state_ref;
    let mut surfaces = HashMap::new();
    for element in &document.elements {
        let ImageSource::Path(path) = &element.source else { continue };
        if let Some(image) = get_or_decode(image_cache, path) {
            if let Ok(surface) = import::surface_from_decoded(image) {
                surfaces.insert(element.id, surface);
            }
        }
    }
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

/// The output height that results from scaling `canvas_settings`'s
/// content-fitted native size to its target export width, shown read-only
/// in the sidebar (see `export_height_row`'s `sensitive: false`).
fn update_export_height_display(window: &Window, canvas_settings: screenforge_core::model::CanvasSettings) {
    let scale = canvas_settings.export_target_width as f64 / canvas_settings.export_width.max(1) as f64;
    let height = (canvas_settings.export_height as f64 * scale).round().max(1.0);
    window.export_height_row().set_value(height);
}

/// The path to decode for `Background::Image`, if the background is that
/// variant and its source is (as always today) a plain file path.
fn background_image_path(background: &Background) -> Option<PathBuf> {
    let Background::Image(spec) = background else { return None };
    let ImageSource::Path(path) = &spec.source else { return None };
    Some(path.clone())
}

/// Decodes every path and appends the successful ones to `state` as one
/// undoable [`AddScreenshots`] command, then refreshes the canvas once (not
/// per file). Used by both the file-open action and drag-and-drop, so the
/// two import paths can't drift apart.
fn import_paths(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>, paths: Vec<PathBuf>) {
    let mut new_elements = Vec::new();
    {
        let mut state_ref = state.borrow_mut();
        for path in paths {
            if let Some(image) = get_or_decode(&mut state_ref.image_cache, &path) {
                new_elements.push(ScreenshotElement::new(ImageSource::Path(path), image.width as f64, image.height as f64));
            }
        }
    }
    if new_elements.is_empty() {
        return;
    }

    let mut state_ref = state.borrow_mut();
    let EditorState { document, undo_stack, .. } = &mut *state_ref;
    undo_stack.apply(Box::new(AddScreenshots { elements: new_elements }), document);
    drop(state_ref);

    refresh_canvas(window, canvas, state);
    update_undo_redo_sensitivity(window, state);
}

fn register_open_action(app: &adw::Application, window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let open_action = gio::SimpleAction::new("open", None);
    open_action.connect_activate(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |_, _| {
            let window = window.clone();
            let canvas = canvas.clone();
            let state = state.clone();
            glib::spawn_future_local(async move {
                let filter = gtk4::FileFilter::new();
                filter.add_mime_type("image/png");
                filter.add_mime_type("image/jpeg");
                filter.add_mime_type("image/webp");
                filter.set_name(Some("Screenshots"));

                let dialog = gtk4::FileDialog::builder()
                    .title("Screenshots öffnen")
                    .accept_label("Öffnen")
                    .default_filter(&filter)
                    .build();

                match dialog.open_multiple_future(Some(&window)).await {
                    Ok(files) => {
                        let paths: Vec<PathBuf> =
                            files.iter::<gio::File>().flatten().filter_map(|f| f.path()).collect();
                        import_paths(&window, &canvas, &state, paths);
                    }
                    Err(err) => {
                        if !err.matches(gtk4::DialogError::Dismissed) {
                            eprintln!("ScreenForge: open dialog failed: {err}");
                        }
                    }
                }
            });
        }
    ));
    window.add_action(&open_action);
    app.set_accels_for_action("win.open", &["<Ctrl>o"]);
}

/// The "Von Android-Gerät importieren…" action: runs `adb` on a
/// background thread (`gio::spawn_blocking`, mirroring `win.export`'s own
/// use of it — a stuck or slow `adb` call must never freeze the UI),
/// then imports the captured PNG through the same [`import_paths`] every
/// other import route shares.
fn register_import_android_action(app: &adw::Application, window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let action = gio::SimpleAction::new("import-android", None);
    action.connect_activate(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |_, _| {
            let window = window.clone();
            let canvas = canvas.clone();
            let state = state.clone();
            glib::spawn_future_local(async move {
                let toast_overlay = window.toast_overlay();
                let result = gio::spawn_blocking(adb::capture_screenshot).await;
                match result {
                    Ok(Ok(path)) => import_paths(&window, &canvas, &state, vec![path]),
                    Ok(Err(err)) => toast_overlay.add_toast(adw::Toast::new(&err.to_string())),
                    Err(_) => toast_overlay.add_toast(adw::Toast::new("Import fehlgeschlagen: Hintergrundaufgabe abgebrochen")),
                }
            });
        }
    ));
    window.add_action(&action);
    app.set_accels_for_action("win.import-android", &["<Ctrl><Shift>a"]);
}

/// How often `register_adb_watch` re-checks device state. Deliberately a
/// plain, modest-interval poll rather than shelling out to `adb
/// track-devices` (its push-based, no-polling protocol) — that needs a
/// long-lived subprocess with its own reconnect/parsing logic for real
/// gains, where this needs only what's already this app's established
/// idiom for talking to `adb` (`gio::spawn_blocking`, see
/// `register_import_android_action`) at an interval far below "aggressive"
/// while still noticing a plugged-in phone within a couple of seconds.
const ADB_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Starts one off-thread ADB check and, on completion, updates
/// `android_import_button` — but only if the resulting state actually
/// differs from `last_state`, so a device that's already connected costs
/// nothing beyond the `adb devices` call itself on every subsequent tick.
/// A plain fn (not a closure) so `register_adb_watch` below can call it
/// both immediately and from its repeating timer without fighting the
/// borrow checker over which one owns it.
fn check_adb_state_once(window: &Window, last_state: &Rc<RefCell<Option<adb::AdbDeviceState>>>) {
    glib::spawn_future_local(glib::clone!(
        #[weak]
        window,
        #[strong]
        last_state,
        async move {
            let state = gio::spawn_blocking(adb::detect_state)
                .await
                .unwrap_or_else(|_| adb::AdbDeviceState::AdbUnavailable("Geräteprüfung abgebrochen".to_string()));
            if last_state.borrow().as_ref() == Some(&state) {
                return;
            }
            let button = window.android_import_button();
            button.set_sensitive(state.is_usable());
            button.set_tooltip_text(Some(&state.tooltip()));
            *last_state.borrow_mut() = Some(state);
        }
    ));
}

/// Keeps `android_import_button` in sync with whatever `adb` can currently
/// see (spec: the button must reflect a *working ADB connection*, not
/// just "some USB device is plugged in", and must update on its own
/// without the user re-opening anything). Every check happens off the
/// main thread via `gio::spawn_blocking` (see `check_adb_state_once`) —
/// the periodic timer here only ever *starts* one, never runs `adb`
/// inline. Runs the first check immediately rather than waiting a full
/// interval, so the button's initial disabled state (set in `window.ui`)
/// resolves to the truth as soon as the window appears. The timer itself
/// holds only a weak reference to `window` and stops itself
/// (`ControlFlow::Break`) once that upgrade fails, rather than running
/// forever against a widget that's gone.
fn register_adb_watch(window: &Window) {
    let last_state: Rc<RefCell<Option<adb::AdbDeviceState>>> = Rc::new(RefCell::new(None));

    check_adb_state_once(window, &last_state);
    glib::timeout_add_local(
        ADB_POLL_INTERVAL,
        glib::clone!(
            #[weak]
            window,
            #[upgrade_or]
            glib::ControlFlow::Break,
            move || {
                check_adb_state_once(&window, &last_state);
                glib::ControlFlow::Continue
            }
        ),
    );
}

/// Lets screenshots be dragged in directly from a file manager (spec §1).
/// Shares [`import_paths`] with the file-open action so both routes decode
/// identically.
fn register_drop_target(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let target = gtk4::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);

    target.connect_enter(glib::clone!(
        #[weak]
        canvas,
        #[upgrade_or]
        gdk::DragAction::empty(),
        move |_, _, _| {
            canvas.set_drag_active(true);
            gdk::DragAction::COPY
        }
    ));
    target.connect_leave(glib::clone!(
        #[weak]
        canvas,
        move |_| canvas.set_drag_active(false)
    ));
    target.connect_drop(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        #[upgrade_or]
        false,
        move |_, value, _, _| {
            canvas.set_drag_active(false);
            let Ok(file_list) = value.get::<gdk::FileList>() else { return false };
            let paths: Vec<PathBuf> = file_list.files().into_iter().filter_map(|f| f.path()).collect();
            if paths.is_empty() {
                return false;
            }
            import_paths(&window, &canvas, &state, paths);
            true
        }
    ));

    canvas.add_controller(target);
}

fn layout_mode_for_index(index: u32) -> LayoutMode {
    match index {
        0 => LayoutMode::Horizontal,
        1 => LayoutMode::Vertical,
        2 => LayoutMode::Grid,
        _ => LayoutMode::Free,
    }
}

fn index_for_layout_mode(mode: LayoutMode) -> u32 {
    match mode {
        LayoutMode::Horizontal => 0,
        LayoutMode::Vertical => 1,
        LayoutMode::Grid => 2,
        LayoutMode::Free => 3,
    }
}

/// The alignment tool only does anything in `LayoutMode::Free` — every
/// other mode recomputes each element's placement from spacing/margin on
/// every render, so touching `Transform.x`/`.y` there would have no visible
/// effect at all. Selection count isn't part of this check (see
/// `align_selected`'s own guard) since it can change without the layout
/// mode row itself firing.
fn sync_alignment_group_visibility(window: &Window, mode: LayoutMode) {
    window.alignment_group().set_visible(mode == LayoutMode::Free);
}

/// Wires the sidebar's layout-mode/spacing/margin rows to `Document.layout`,
/// mutating it directly through the undo stack (spec §17: layout changes are
/// undoable).
fn register_layout_controls(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let layout_mode_row = window.layout_mode_row();
    let spacing_row = window.spacing_row();
    let margin_x_row = window.margin_x_row();
    let margin_y_row = window.margin_y_row();

    {
        let state_ref = state.borrow();
        layout_mode_row.set_selected(index_for_layout_mode(state_ref.document.layout.mode));
        spacing_row.set_value(state_ref.document.layout.spacing_px);
        margin_x_row.set_value(state_ref.document.layout.margin_x);
        margin_y_row.set_value(state_ref.document.layout.margin_y);
        sync_alignment_group_visibility(window, state_ref.document.layout.mode);
    }

    layout_mode_row.connect_selected_notify(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |row| {
            let new = layout_mode_for_index(row.selected());
            sync_alignment_group_visibility(&window, new);
            let mut state_ref = state.borrow_mut();
            let old = state_ref.document.layout.mode;
            if state_ref.syncing_controls || old == new {
                return;
            }
            let command: Box<dyn Command> = if new == LayoutMode::Free {
                // Snapshot each visible element's placement under the old
                // mode as its new transform, so free positioning starts
                // from "wherever auto-layout had it" instead of everyone
                // collapsed onto Transform::default()'s (0, 0) origin.
                let doc = &state_ref.document;
                let visible: Vec<_> = doc.elements.iter().filter(|e| e.visible).cloned().collect();
                let placements = screenforge_core::layout::compute_layout(
                    old,
                    &visible,
                    doc.layout.spacing_px,
                    doc.layout.margin_x,
                    doc.layout.margin_y,
                );
                let transforms = visible
                    .iter()
                    .zip(placements.iter())
                    .map(|(el, placement)| {
                        let mut new_transform = el.transform;
                        new_transform.x = placement.x;
                        new_transform.y = placement.y;
                        new_transform.width = placement.width;
                        new_transform.height = placement.height;
                        (el.id, el.transform, new_transform)
                    })
                    .collect();
                Box::new(EnterFreeLayout { old_mode: old, transforms })
            } else {
                Box::new(SetLayoutMode { old, new })
            };
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(command, document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));

    spacing_row.connect_value_notify(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |row| {
            let new = row.value();
            let mut state_ref = state.borrow_mut();
            let old = state_ref.document.layout.spacing_px;
            if state_ref.syncing_controls || old == new {
                return;
            }
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetSpacing { old, new }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));
    margin_x_row.connect_value_notify(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |row| {
            let new = row.value();
            let mut state_ref = state.borrow_mut();
            let old = state_ref.document.layout.margin_x;
            if state_ref.syncing_controls || old == new {
                return;
            }
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetMarginX { old, new }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));
    margin_y_row.connect_value_notify(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |row| {
            let new = row.value();
            let mut state_ref = state.borrow_mut();
            let old = state_ref.document.layout.margin_y;
            if state_ref.syncing_controls || old == new {
                return;
            }
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetMarginY { old, new }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));
}

fn color_strategy_for_index(index: u32) -> ColorStrategy {
    match index {
        0 => ColorStrategy::Manual,
        1 => ColorStrategy::FromScreenshots,
        2 => ColorStrategy::Grayscale,
        _ => ColorStrategy::Random,
    }
}

fn index_for_color_strategy(strategy: ColorStrategy) -> u32 {
    match strategy {
        ColorStrategy::Manual => 0,
        ColorStrategy::FromScreenshots => 1,
        ColorStrategy::Grayscale => 2,
        ColorStrategy::Random => 3,
    }
}

/// Order of the "Stil" dropdown: the modern styles first, the legacy
/// wave generator last.
const GENERATOR_STYLES: [GeneratorStyle; 7] = [
    GeneratorStyle::Layers,
    GeneratorStyle::Arcs,
    GeneratorStyle::Ribbons,
    GeneratorStyle::Planes,
    GeneratorStyle::Lines,
    GeneratorStyle::Mist,
    GeneratorStyle::Waves,
];

fn generator_style_for_index(index: u32) -> GeneratorStyle {
    GENERATOR_STYLES.get(index as usize).copied().unwrap_or(GeneratorStyle::Layers)
}

fn index_for_generator_style(style: GeneratorStyle) -> u32 {
    GENERATOR_STYLES.iter().position(|&s| s == style).unwrap_or(0) as u32
}

fn mood_for_index(index: u32) -> Mood {
    match index {
        1 => Mood::Light,
        2 => Mood::Dark,
        _ => Mood::Vivid,
    }
}

fn index_for_mood(mood: Mood) -> u32 {
    match mood {
        Mood::Vivid => 0,
        Mood::Light => 1,
        Mood::Dark => 2,
    }
}

/// Reflects a `GeneratedBackground`'s parameters onto the generator
/// controls — used both by `sync_background_controls`'s `Generated` arm
/// and after a fresh "Generieren" click updates the seed, mirroring
/// `sync_label_controls`'s role for the selected screenshot's label.
fn sync_generator_controls(window: &Window, generated: &GeneratedBackground) {
    window.generator_color_strategy_row().set_selected(index_for_color_strategy(generated.color_strategy));
    window.generator_style_row().set_selected(index_for_generator_style(generated.style));
    window.generator_mood_row().set_selected(index_for_mood(generated.mood));
    window.generator_grain_row().set_value(generated.grain * 100.0);
    let manual_buttons =
        [window.generator_manual_color_button_1(), window.generator_manual_color_button_2(), window.generator_manual_color_button_3(), window.generator_manual_color_button_4()];
    for (i, button) in manual_buttons.iter().enumerate() {
        let color = generated.palette.get(i).copied().unwrap_or(Rgba::new(0.5, 0.5, 0.5, 1.0));
        button.set_rgba(&gdk_rgba_from(&color));
    }
    window.generator_adapt_row().set_active(generated.adapt_to_screenshots);
    window.generator_inverse_contrast_row().set_value(generated.inverse_contrast * 100.0);
    window.generator_corner_bias_row().set_value(generated.corner_bias * 100.0);
    window.generator_scale_row().set_value(generated.scale * 100.0);
    window.generator_contrast_row().set_value(generated.contrast * 100.0);
    window.generator_seed_row().set_value(generated.seed as f64);
}

/// The dropdown index reserved for "Angepasst" — a sixth, display-only
/// entry appended after the four real presets on every shadow dropdown in
/// the app (screenshot Effekte, project/global label style, callouts).
/// Never a valid *choice* to build a new shadow from — it only ever shows
/// up as the reflected state of a shadow that doesn't match any of the
/// four named presets (see `shadow_preset_index_for`), which happens any
/// time someone fine-tunes the angle/distance/blur rows directly rather
/// than picking a preset. Every shadow-dropdown handler in this file
/// treats selecting it as a no-op (nothing coherent to "apply"), rather
/// than silently falling back to some other preset's exact values.
const CUSTOM_SHADOW_PRESET_INDEX: u32 = 5;

fn shadow_preset_for_index(index: u32) -> ShadowPreset {
    match index {
        0 => ShadowPreset::NONE,
        1 => ShadowPreset::SUBTLE,
        2 => ShadowPreset::STANDARD,
        3 => ShadowPreset::STRONG,
        _ => ShadowPreset::FLOATING,
    }
}

/// The preset dropdown index matching `shadow`'s current distance/blur/
/// opacity/color — deliberately ignoring `angle_and_distance().0` (the
/// angle), so a shadow with a custom angle still shows its actual
/// Subtle/Standard/Strong/Floating preset instead of falling back to
/// "Angepasst" just because a plain `ShadowParams` equality check would
/// fail once the angle no longer matches the preset's own baked-in 90°.
/// Falls back to `CUSTOM_SHADOW_PRESET_INDEX` — never `0`/"Kein
/// Schatten" — for a shadow that doesn't match any of the four presets on
/// every other axis, so a hand-tuned (but very much enabled) shadow is
/// never mislabeled as no shadow at all.
fn shadow_preset_index_for(shadow: &ShadowParams) -> u32 {
    let (_, distance) = shadow.angle_and_distance();
    let presets = [ShadowPreset::NONE, ShadowPreset::SUBTLE, ShadowPreset::STANDARD, ShadowPreset::STRONG, ShadowPreset::FLOATING];
    presets
        .iter()
        .position(|p| {
            (p.distance - distance).abs() < 0.01
                && (p.blur - shadow.blur).abs() < 0.01
                && (p.opacity - shadow.opacity).abs() < 0.001
                && p.color == shadow.color
        })
        .map(|i| i as u32)
        .unwrap_or(CUSTOM_SHADOW_PRESET_INDEX)
}

fn horizontal_anchor_for_index(index: u32) -> HorizontalAnchor {
    match index {
        0 => HorizontalAnchor::Left,
        2 => HorizontalAnchor::Right,
        _ => HorizontalAnchor::Center,
    }
}

fn index_for_horizontal_anchor(anchor: HorizontalAnchor) -> u32 {
    match anchor {
        HorizontalAnchor::Left => 0,
        HorizontalAnchor::Center => 1,
        HorizontalAnchor::Right => 2,
    }
}

fn vertical_anchor_for_index(index: u32) -> VerticalAnchor {
    match index {
        0 => VerticalAnchor::Top,
        2 => VerticalAnchor::Bottom,
        _ => VerticalAnchor::Center,
    }
}

fn index_for_vertical_anchor(anchor: VerticalAnchor) -> u32 {
    match anchor {
        VerticalAnchor::Top => 0,
        VerticalAnchor::Center => 1,
        VerticalAnchor::Bottom => 2,
    }
}

fn text_align_for_index(index: u32) -> TextAlign {
    match index {
        0 => TextAlign::Left,
        2 => TextAlign::Right,
        _ => TextAlign::Center,
    }
}

fn index_for_text_align(align: TextAlign) -> u32 {
    match align {
        TextAlign::Left => 0,
        TextAlign::Center => 1,
        TextAlign::Right => 2,
    }
}

/// Extracts family/size/weight/italic from a Pango font description — the
/// shape `Typography` stores them in, so a `GtkFontDialogButton` can be
/// this app's one control for all four (spec: "use the native GNOME text
/// stack"). Size is read as whatever raw number Pango carries (points from
/// the system font picker, or pixels if we set it via `set_absolute_size`
/// ourselves) without converting between the two — close enough at
/// typical desktop DPI for a screenshot compositor, and it means this app
/// never needs to know the display's actual DPI.
fn typography_from_font_desc(font_desc: &pango::FontDescription) -> (String, f64, i32, bool) {
    use glib::translate::IntoGlib;
    let family = font_desc.family().map(|f| f.to_string()).unwrap_or_else(|| "Sans".to_string());
    let size = (font_desc.size() as f64 / pango::SCALE as f64).max(1.0);
    let weight = font_desc.weight().into_glib();
    let italic = matches!(font_desc.style(), pango::Style::Italic | pango::Style::Oblique);
    (family, size, weight, italic)
}

fn font_desc_from_typography(typography: &Typography) -> pango::FontDescription {
    let mut font_desc = pango::FontDescription::new();
    font_desc.set_family(&typography.font_family);
    font_desc.set_absolute_size(typography.font_size.max(0.1) * pango::SCALE as f64);
    font_desc.set_weight(pango::Weight::__Unknown(typography.weight));
    font_desc.set_style(if typography.italic { pango::Style::Italic } else { pango::Style::Normal });
    font_desc
}

/// Reflects a `TextElement` (the composition title) onto its controls —
/// used for both the initial sync and after undo/redo/load, mirroring
/// `sync_background_controls`.
/// The screenshot a Label-sidebar edit should target: the document's own
/// current single-selected screenshot, or `None` when 0 or several are
/// selected (in which case the whole `label_group` stays hidden — see
/// `sync_label_controls`).
fn single_selected_label_target(canvas: &Canvas, state: &Rc<RefCell<EditorState>>) -> Option<(Uuid, Label)> {
    let selected = canvas.selected_ids();
    if selected.len() != 1 {
        return None;
    }
    let id = *selected.iter().next().unwrap();
    state.borrow().document.elements.iter().find(|e| e.id == id).map(|e| (id, e.label.clone()))
}

/// Reflects the current single-selected screenshot's label onto the
/// sidebar's "Label" section, hiding that whole section when 0 or several
/// screenshots are selected. Called both when the selection changes and
/// as part of `sync_controls_from_document` (after undo/redo/load, since
/// the selected screenshot's label may have changed underneath it too).
/// Guards its own writes with `EditorState.syncing_controls` so
/// `register_label_controls`'s row handlers don't reinterpret this as a
/// user edit and dispatch a spurious undo step.
fn sync_label_controls(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let Some((_, label)) = single_selected_label_target(canvas, state) else {
        window.label_group().set_visible(false);
        return;
    };
    window.label_group().set_visible(true);

    let was_syncing = state.borrow().syncing_controls;
    state.borrow_mut().syncing_controls = true;

    window.label_enabled_row().set_active(label.enabled);
    window.label_content_view().buffer().set_text(&label.content);
    window.label_content_view().set_sensitive(label.enabled);

    state.borrow_mut().syncing_controls = was_syncing;
}

/// Reflects a `Background` value onto the type/color1/color2/angle controls
/// (used for both the initial sync and after undo/redo/load).
/// Shows/hides the generator's color-strategy-dependent rows: the 4 manual
/// swatches only for `Manual`, the screenshot-contrast dial only for
/// `FromScreenshots` — shared between the initial sync and the
/// color-strategy combo's own live notify handler.
fn sync_generator_color_strategy_visibility(window: &Window, is_generated: bool, strategy: ColorStrategy) {
    let is_manual = is_generated && matches!(strategy, ColorStrategy::Manual);
    let is_from_screenshots = is_generated && matches!(strategy, ColorStrategy::FromScreenshots);
    window.generator_manual_color_row_1().set_visible(is_manual);
    window.generator_manual_color_row_2().set_visible(is_manual);
    window.generator_manual_color_row_3().set_visible(is_manual);
    window.generator_manual_color_row_4().set_visible(is_manual);
    window.generator_inverse_contrast_row().set_visible(is_from_screenshots);
}

fn sync_background_controls(window: &Window, background: &Background) {
    let is_generated = matches!(background, Background::Generated(_));
    window.background_color1_row().set_visible(!matches!(background, Background::Image(_)) && !is_generated);
    window.gradient_color2_row().set_visible(matches!(background, Background::Gradient(_)));
    window.gradient_angle_row().set_visible(matches!(background, Background::Gradient(spec) if matches!(spec.kind, GradientKind::Linear { .. })));
    window.gradient_auto_colors_row().set_visible(matches!(background, Background::Gradient(_)));
    window.background_image_row().set_visible(matches!(background, Background::Image(_)));
    window.background_image_fit_row().set_visible(matches!(background, Background::Image(_)));
    window.background_image_opacity_row().set_visible(matches!(background, Background::Image(_)));
    window.generator_color_strategy_row().set_visible(is_generated);
    window.generator_style_row().set_visible(is_generated);
    window.generator_mood_row().set_visible(is_generated);
    window.generator_grain_row().set_visible(is_generated);
    window.generator_adapt_row().set_visible(is_generated);
    window.generator_corner_bias_row().set_visible(is_generated);
    window.generator_scale_row().set_visible(is_generated);
    window.generator_contrast_row().set_visible(is_generated);
    window.generator_seed_row().set_visible(is_generated);
    window.generator_generate_row().set_visible(is_generated);
    sync_generator_color_strategy_visibility(
        window,
        is_generated,
        if let Background::Generated(generated) = background { generated.color_strategy } else { ColorStrategy::Manual },
    );

    match background {
        Background::Solid(color) => {
            window.background_type_row().set_selected(0);
            window.background_color_button().set_rgba(&gdk_rgba_from(color));
        }
        Background::Gradient(spec) => {
            let is_radial = matches!(spec.kind, GradientKind::Radial { .. });
            window.background_type_row().set_selected(if is_radial { 2 } else { 1 });
            if let Some((_, color)) = spec.stops.first() {
                window.background_color_button().set_rgba(&gdk_rgba_from(color));
            }
            if let Some((_, color)) = spec.stops.get(1) {
                window.gradient_color2_button().set_rgba(&gdk_rgba_from(color));
            }
            if let GradientKind::Linear { angle_deg } = spec.kind {
                window.gradient_angle_row().set_value(angle_deg);
            }
        }
        Background::Image(spec) => {
            window.background_type_row().set_selected(3);
            window.background_image_row().set_subtitle(&background_image_subtitle(&spec.source));
            window.background_image_fit_row().set_selected(index_for_background_image_fit(spec.fit));
            window.background_image_opacity_row().set_value(spec.opacity * 100.0);
        }
        Background::Generated(generated) => {
            window.background_type_row().set_selected(4);
            sync_generator_controls(window, generated);
        }
    }
}

/// Display text for the "Bilddatei" row's subtitle: the file name, or a
/// placeholder if the source isn't (as always today) a plain path.
fn background_image_subtitle(source: &ImageSource) -> String {
    match source {
        ImageSource::Path(path) => path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        ImageSource::Embedded { filename, .. } => filename.clone(),
    }
}

fn background_image_fit_for_index(index: u32) -> BackgroundImageFit {
    match index {
        0 => BackgroundImageFit::Cover,
        1 => BackgroundImageFit::Contain,
        2 => BackgroundImageFit::Fill,
        _ => BackgroundImageFit::Tile,
    }
}

fn index_for_background_image_fit(fit: BackgroundImageFit) -> u32 {
    match fit {
        BackgroundImageFit::Cover => 0,
        BackgroundImageFit::Contain => 1,
        BackgroundImageFit::Fill => 2,
        BackgroundImageFit::Tile => 3,
    }
}

fn gdk_rgba_from(c: &Rgba) -> gdk::RGBA {
    gdk::RGBA::new(c.r as f32, c.g as f32, c.b as f32, c.a as f32)
}

fn rgba_from_gdk(c: &gdk::RGBA) -> Rgba {
    Rgba::new(c.red() as f64, c.green() as f64, c.blue() as f64, c.alpha() as f64)
}

/// Arms the eyedropper (spec: "Farbpipette") for one pick: the canvas's
/// cursor becomes a crosshair, and the very next primary-button click on it
/// samples that pixel's rendered color (`Canvas::sample_color_at`) straight
/// into `target`'s `rgba`, exactly as if the user had picked it from
/// `target`'s own color dialog — so every existing `connect_rgba_notify`
/// wired to `target` elsewhere just fires normally, with no separate
/// "apply a picked color" path to keep in sync.
///
/// Implemented as one temporary, capture-phase `GtkGestureClick` added to
/// the canvas and removed again the moment it fires, rather than any
/// change to the canvas's own selection/drag/resize gesture — capture
/// phase runs before the canvas's own (unphased) click handling, and
/// claiming the sequence stops that normal handling from also seeing the
/// same click, so a pick behaves like a true modal tool without the canvas
/// needing to know picking exists.
fn start_color_picking(window: &Window, canvas: &Canvas, target: gtk4::ColorDialogButton) {
    canvas.set_cursor_from_name(Some("crosshair"));
    let gesture = gtk4::GestureClick::new();
    gesture.set_button(gdk::BUTTON_PRIMARY);
    gesture.set_propagation_phase(gtk4::PropagationPhase::Capture);
    gesture.connect_pressed(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        target,
        move |gesture, _n_press, x, y| {
            gesture.set_state(gtk4::EventSequenceState::Claimed);
            canvas.set_cursor_from_name(None);
            canvas.remove_controller(gesture);
            match canvas.sample_color_at(x, y) {
                Some(color) => target.set_rgba(&gdk_rgba_from(&color)),
                None => {
                    window.toast_overlay().add_toast(adw::Toast::new("An dieser Stelle wurde keine Farbe gefunden"));
                }
            }
        }
    ));
    canvas.add_controller(gesture);
}

/// Wires every color-target's small eyedropper button (spec: "Farbpipette")
/// to `start_color_picking`.
fn register_eyedroppers(window: &Window, canvas: &Canvas) {
    let pairs = [
        (window.background_eyedropper_button(), window.background_color_button()),
        (window.gradient_color2_eyedropper_button(), window.gradient_color2_button()),
        (window.generator_manual_eyedropper_button_1(), window.generator_manual_color_button_1()),
        (window.generator_manual_eyedropper_button_2(), window.generator_manual_color_button_2()),
        (window.generator_manual_eyedropper_button_3(), window.generator_manual_color_button_3()),
        (window.generator_manual_eyedropper_button_4(), window.generator_manual_color_button_4()),
    ];
    for (trigger, target) in pairs {
        trigger.connect_clicked(glib::clone!(
            #[weak]
            window,
            #[weak]
            canvas,
            move |_| start_color_picking(&window, &canvas, target.clone())
        ));
    }
}

/// Reads the background controls (type/color1/color2/angle) and builds the
/// `Background` they currently describe.
fn background_from_controls(window: &Window) -> Background {
    let color1 = rgba_from_gdk(&window.background_color_button().rgba());
    match window.background_type_row().selected() {
        1 => {
            let color2 = rgba_from_gdk(&window.gradient_color2_button().rgba());
            let angle_deg = window.gradient_angle_row().value();
            Background::Gradient(GradientSpec { kind: GradientKind::Linear { angle_deg }, stops: vec![(0.0, color1), (1.0, color2)] })
        }
        2 => {
            let color2 = rgba_from_gdk(&window.gradient_color2_button().rgba());
            // No manual center control yet — radial gradients are centered
            // on the composition (spec §8 leaves per-element/background
            // positioning controls for later).
            Background::Gradient(GradientSpec {
                kind: GradientKind::Radial { center_x: 0.5, center_y: 0.5 },
                stops: vec![(0.0, color1), (1.0, color2)],
            })
        }
        _ => Background::Solid(color1),
    }
}

/// Applies whatever the background controls currently describe as one
/// undoable `SetBackground`, skipping the push if it doesn't actually
/// change anything — needed because `sync_controls_from_document` (after
/// undo/redo/load) sets these same controls to match the document it just
/// applied, which would otherwise re-fire this handler and wipe the redo
/// history it was trying to restore.
fn apply_background_from_controls(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let mut state_ref = state.borrow_mut();
    let new = background_from_controls(window);
    if state_ref.syncing_controls || state_ref.document.background == new {
        return;
    }
    let old = state_ref.document.background.clone();
    let EditorState { document, undo_stack, .. } = &mut *state_ref;
    undo_stack.apply(Box::new(SetBackground { old, new }), document);
    drop(state_ref);
    refresh_canvas(window, canvas, state);
    update_undo_redo_sensitivity(window, state);
}

/// Wires background (solid or linear gradient), shadow preset and
/// corner-radius controls through the undo stack. There's no per-element
/// selection yet (deferred, spec §5), so for the MVP shadow/corner-radius
/// apply uniformly to every screenshot — matching the example workflow in
/// spec §27, where one setting is applied to the whole composition.
fn register_effect_controls(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let background_type_row = window.background_type_row();
    let background_color_button = window.background_color_button();
    let gradient_color2_button = window.gradient_color2_button();
    let gradient_angle_row = window.gradient_angle_row();
    let shadow_row = window.shadow_row();
    let shadow_angle_row = window.shadow_angle_row();
    let shadow_distance_row = window.shadow_distance_row();
    let shadow_blur_row = window.shadow_blur_row();
    let corner_radius_row = window.corner_radius_row();

    {
        let state_ref = state.borrow();
        let shadow_geometry_enabled = state_ref.document.elements.first().is_some_and(|e| e.shadow.enabled);
        let (angle, distance) = state_ref.document.elements.first().map(|e| e.shadow.angle_and_distance()).unwrap_or((90.0, 6.0));
        let blur = state_ref.document.elements.first().map(|e| e.shadow.blur).unwrap_or(16.0);
        shadow_angle_row.set_value(angle);
        shadow_distance_row.set_value(distance);
        shadow_blur_row.set_value(blur);
        shadow_angle_row.set_sensitive(shadow_geometry_enabled);
        shadow_distance_row.set_sensitive(shadow_geometry_enabled);
        shadow_blur_row.set_sensitive(shadow_geometry_enabled);
    }

    sync_background_controls(window, &state.borrow().document.background);

    background_type_row.connect_selected_notify(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |row| {
            let selected = row.selected();
            window.background_color1_row().set_visible(selected != 3 && selected != 4);
            window.gradient_color2_row().set_visible(selected == 1 || selected == 2);
            window.gradient_angle_row().set_visible(selected == 1);
            window.gradient_auto_colors_row().set_visible(selected == 1 || selected == 2);
            window.background_image_row().set_visible(selected == 3);
            window.background_image_fit_row().set_visible(selected == 3);
            window.background_image_opacity_row().set_visible(selected == 3);
            let is_generated = selected == 4;
            window.generator_color_strategy_row().set_visible(is_generated);
            window.generator_style_row().set_visible(is_generated);
            window.generator_mood_row().set_visible(is_generated);
            window.generator_grain_row().set_visible(is_generated);
    window.generator_style_row().set_visible(is_generated);
    window.generator_mood_row().set_visible(is_generated);
    window.generator_grain_row().set_visible(is_generated);
            window.generator_adapt_row().set_visible(is_generated);
            window.generator_corner_bias_row().set_visible(is_generated);
            window.generator_scale_row().set_visible(is_generated);
            window.generator_contrast_row().set_visible(is_generated);
            window.generator_seed_row().set_visible(is_generated);
            window.generator_generate_row().set_visible(is_generated);
            sync_generator_color_strategy_visibility(&window, is_generated, color_strategy_for_index(window.generator_color_strategy_row().selected()));
            // Selecting "Bild" only reveals the file picker — there's
            // nothing to render until a file is actually chosen (below).
            // Selecting "Generiert" needs an actual palette/seed resolved
            // from the current screenshots before there's anything to
            // render either, which `background_from_controls` (a synchronous,
            // state-free helper) has no way to do — so this goes through
            // `generate_background` instead, same as the "Generieren"
            // button itself.
            if is_generated {
                generate_background(&window, &canvas, &state);
            } else if selected != 3 {
                apply_background_from_controls(&window, &canvas, &state);
            }
        }
    ));
    background_color_button.connect_rgba_notify(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |_| apply_background_from_controls(&window, &canvas, &state)
    ));
    gradient_color2_button.connect_rgba_notify(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |_| apply_background_from_controls(&window, &canvas, &state)
    ));
    gradient_angle_row.connect_value_notify(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |_| apply_background_from_controls(&window, &canvas, &state)
    ));
    register_background_image_controls(window, canvas, state);
    register_gradient_auto_colors_control(window, canvas, state);

    shadow_row.connect_selected_notify(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |row| {
            // Bail out *before* touching any sibling widget — this handler
            // reenters whenever `sync_controls_from_document` (including
            // via a preset apply) sets this row's selection while a resync
            // is already in progress. Checking `syncing_controls` only
            // before the undo-push (as this used to) still let the writes
            // below run unconditionally, clobbering the sync's own
            // just-set, correct distance/blur/sensitivity with values
            // derived from `shadow_preset_for_index`'s lossy classification
            // — the Effekte section would then visibly show the wrong
            // shadow even though the document's own shadow was correct all
            // along.
            if state.borrow().syncing_controls {
                return;
            }
            // "Angepasst" is a display-only reflection of a shadow that
            // doesn't match any of the four real presets — never something
            // to actively apply (there's no single "the custom shadow" to
            // build from just this label), so selecting it is a no-op.
            if row.selected() == CUSTOM_SHADOW_PRESET_INDEX {
                return;
            }
            let preset = shadow_preset_for_index(row.selected());
            // `with_preset` only touches distance/blur/opacity/color and
            // keeps whichever angle the shadow already had — a preset is a
            // statement about how strong a shadow looks, not which
            // direction it's cast (spec: choosing Subtle/Standard/Strong
            // must never reset a user-chosen angle back to 90°).
            let mut state_ref = state.borrow_mut();
            let current = state_ref.document.elements.first().map(|e| e.shadow).unwrap_or_default();
            let new = current.with_preset(preset);

            // Guard these writes the same way `sync_controls_from_document`
            // guards its own batch: without it, each `set_value` below
            // reentrantly fires `apply_shadow_geometry`, which would push
            // its own spurious undo command built from a partially-updated
            // mix of old and new values.
            state_ref.syncing_controls = true;
            drop(state_ref);

            // Deliberately not touching shadow_angle_row's value — it
            // already shows the angle `with_preset` preserved.
            window.shadow_distance_row().set_value(new.angle_and_distance().1);
            window.shadow_blur_row().set_value(new.blur);
            window.shadow_angle_row().set_sensitive(new.enabled);
            window.shadow_distance_row().set_sensitive(new.enabled);
            window.shadow_blur_row().set_sensitive(new.enabled);

            let mut state_ref = state.borrow_mut();
            state_ref.syncing_controls = false;
            if state_ref.document.elements.iter().all(|e| e.shadow == new) {
                return;
            }
            let old: Vec<ShadowParams> = state_ref.document.elements.iter().map(|e| e.shadow).collect();
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetShadowForAllElements { old, new }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));

    let apply_shadow_geometry = glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move || {
            let mut state_ref = state.borrow_mut();
            if state_ref.syncing_controls {
                return;
            }
            let angle = window.shadow_angle_row().value();
            let distance = window.shadow_distance_row().value();
            let blur = window.shadow_blur_row().value();
            let (offset_x, offset_y) = ShadowParams::offset_for_angle_and_distance(angle, distance);

            let Some(first) = state_ref.document.elements.first() else { return };
            let mut new = first.shadow;
            new.offset_x = offset_x;
            new.offset_y = offset_y;
            new.blur = blur;
            if state_ref.document.elements.iter().all(|e| e.shadow == new) {
                return;
            }
            let old: Vec<ShadowParams> = state_ref.document.elements.iter().map(|e| e.shadow).collect();
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetShadowForAllElements { old, new }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            // The geometry rows just changed the shadow out from under the
            // "Schatten" dropdown — its own selection only updates on an
            // actual dropdown pick, so without this it would keep showing
            // whatever preset (or "Angepasst") was true a moment ago,
            // silently going stale the instant someone hand-tunes the
            // angle/distance/blur directly.
            let was_syncing = state.borrow().syncing_controls;
            state.borrow_mut().syncing_controls = true;
            window.shadow_row().set_selected(shadow_preset_index_for(&new));
            state.borrow_mut().syncing_controls = was_syncing;
            update_undo_redo_sensitivity(&window, &state);
        }
    );
    shadow_angle_row.connect_value_notify(glib::clone!(
        #[strong]
        apply_shadow_geometry,
        move |_| apply_shadow_geometry()
    ));
    shadow_distance_row.connect_value_notify(glib::clone!(
        #[strong]
        apply_shadow_geometry,
        move |_| apply_shadow_geometry()
    ));
    shadow_blur_row.connect_value_notify(glib::clone!(
        #[strong]
        apply_shadow_geometry,
        move |_| apply_shadow_geometry()
    ));

    corner_radius_row.connect_value_notify(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |row| {
            let new = CornerRadius::uniform(row.value());
            let mut state_ref = state.borrow_mut();
            // See the background-color handler above for why this guard
            // against a reentrant sync-triggered no-op is needed.
            if state_ref.syncing_controls || state_ref.document.elements.iter().all(|e| e.corner_radius == new) {
                return;
            }
            let old: Vec<CornerRadius> = state_ref.document.elements.iter().map(|e| e.corner_radius).collect();
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetCornerRadiusForAllElements { old, new }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));

}

/// Wires the selected screenshot's own Label sidebar controls: just
/// `enabled`/`content` — the only two things ever set per-label (spec:
/// "die Gestaltung soll für alle gleich sein, nur noch den Text möchte
/// ich pro Label setzen können"). Both funnel through one `apply_label`
/// that pushes a single `SetScreenshotLabel` undo step. The whole section
/// only targets a single-selected screenshot (see
/// `single_selected_label_target`); it's re-synced whenever the canvas
/// selection changes (`register_selection_sync`).
fn register_label_controls(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    sync_label_controls(window, canvas, state);

    // Only `enabled`/`content` are ever set per-label now — everything
    // else about how a label looks comes from the project's shared
    // `Document::label_defaults`, edited in its own sidebar section (see
    // `register_label_style_controls`), never here.
    let apply_label = glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move || {
            if state.borrow().syncing_controls {
                return;
            }
            let Some((element_id, current)) = single_selected_label_target(&canvas, &state) else { return };

            let buffer = window.label_content_view().buffer();
            let content = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string();
            let new = Label { enabled: window.label_enabled_row().is_active(), content };
            if current == new {
                return;
            }
            let mut state_ref = state.borrow_mut();
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetScreenshotLabel { element_id, old: current, new }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    );

    window.label_enabled_row().connect_active_notify(glib::clone!(
        #[weak]
        window,
        #[strong]
        apply_label,
        move |row| {
            window.label_content_view().set_sensitive(row.is_active());
            apply_label();
        }
    ));
    window.label_content_view().buffer().connect_changed(glib::clone!(
        #[strong]
        apply_label,
        move |_| apply_label()
    ));
}

/// Builds the "Schrift &amp; Farbe"/"Position"/"Gestaltung" groups shared
/// by both places a [`LabelStyle`] is edited — the current project's own
/// `Document::label_defaults`, live in the sidebar
/// (`register_label_style_controls`), and the app-wide default used only
/// to seed a *new* project, in the Settings dialog's "Allgemein" tab
/// (`build_global_label_defaults_groups`) — so the two don't duplicate
/// ~250 lines of near-identical widget construction. `get`/`commit` are
/// the only things that differ between the two: `get` reads whatever
/// `LabelStyle` is being edited (called once up front to populate every
/// row, and again fresh inside each row's own handler), and
/// `commit(old, new)` decides what "saving a change" means — an
/// undo-tracked `SetLabelDefaults` for the project-level case, a plain
/// `GSettings` write for the global case (which has no undo history to
/// speak of). `scope_description`, when set, becomes the first group's
/// description — used only by the global editor, to make clear at a
/// glance that it seeds *new* projects rather than affecting the one
/// currently open.
///
/// Returns the three groups plus a `sync` closure that repopulates every
/// row from a given [`LabelStyle`] — the sidebar's copy of these groups
/// lives for the whole session (unlike the Settings dialog's, which is
/// rebuilt fresh every time it's opened) and must reflect external
/// changes too: undo/redo, loading a different project, applying a
/// preset, or dragging a label's position on the canvas. `is_syncing`
/// must report whether such a resync is currently in progress — every
/// handler below checks it first and bails out, the same guard pattern
/// `EditorState::syncing_controls` provides everywhere else in this file
/// — otherwise `sync` calling e.g. `shadow_row.set_selected(..)` would
/// re-trigger that row's own change handler mid-resync and push a
/// spurious commit built from a half-updated state. The global editor has
/// no ongoing resync need (its dialog page is thrown away on close), so
/// it passes a closure that always returns `false`.
fn build_label_style_groups(
    get: Rc<dyn Fn() -> LabelStyle>,
    commit: Rc<dyn Fn(LabelStyle, LabelStyle)>,
    is_syncing: Rc<dyn Fn() -> bool>,
    scope_description: Option<&str>,
) -> (adw::PreferencesGroup, adw::PreferencesGroup, adw::PreferencesGroup, LabelStyleSync) {
    let defaults = get();

            let position_mode_row = adw::ComboRow::builder().title("Position").build();
            position_mode_row.set_model(Some(&gtk4::StringList::new(&["Automatisch", "Manuell (X/Y)"])));
            let horizontal_row = adw::ComboRow::builder().title("Horizontal").build();
            horizontal_row.set_model(Some(&gtk4::StringList::new(&["Links", "Mitte", "Rechts"])));
            let vertical_row = adw::ComboRow::builder().title("Vertikal").build();
            vertical_row.set_model(Some(&gtk4::StringList::new(&["Oben", "Mitte", "Unten"])));
            // Negative allowed — lets the shared default itself push every
            // label above/left of its screenshot's own edge.
            let padding_row = adw::SpinRow::with_range(-500.0, 500.0, 4.0);
            padding_row.set_title("Randabstand");
            padding_row.set_subtitle("Zum Screenshot-Rand, in Pixeln");
            let x_row = adw::SpinRow::with_range(-4000.0, 8000.0, 4.0);
            x_row.set_title("X-Position");
            let y_row = adw::SpinRow::with_range(-4000.0, 8000.0, 4.0);
            y_row.set_title("Y-Position");

            let background_row = adw::ComboRow::builder().title("Hintergrund").build();
            background_row.set_model(Some(&gtk4::StringList::new(&["Kein Hintergrund", "Einfarbig", "Verlauf"])));
            let background_color_row = adw::ActionRow::builder().title("Hintergrundfarbe").build();
            let background_color_button = gtk4::ColorDialogButton::new(Some(gtk4::ColorDialog::builder().with_alpha(true).build()));
            background_color_button.set_valign(gtk4::Align::Center);
            background_color_row.add_suffix(&background_color_button);
            let background_color2_row = adw::ActionRow::builder().title("Hintergrundfarbe 2").build();
            let background_color2_button = gtk4::ColorDialogButton::new(Some(gtk4::ColorDialog::builder().with_alpha(true).build()));
            background_color2_button.set_valign(gtk4::Align::Center);
            background_color2_row.add_suffix(&background_color2_button);

            let font_row = adw::ActionRow::builder().title("Schrift").build();
            let font_button = gtk4::FontDialogButton::new(Some(gtk4::FontDialog::new()));
            font_button.set_valign(gtk4::Align::Center);
            font_row.add_suffix(&font_button);
            let alignment_row = adw::ComboRow::builder().title("Textausrichtung").subtitle("Bei mehrzeiligem Text").build();
            alignment_row.set_model(Some(&gtk4::StringList::new(&["Links", "Mitte", "Rechts"])));
            let color_row = adw::ActionRow::builder().title("Textfarbe").build();
            let color_button = gtk4::ColorDialogButton::new(Some(gtk4::ColorDialog::new()));
            color_button.set_valign(gtk4::Align::Center);
            color_row.add_suffix(&color_button);
            let opacity_row = adw::SpinRow::with_range(0.0, 100.0, 5.0);
            opacity_row.set_title("Deckkraft");
            opacity_row.set_subtitle("In Prozent");

            let corner_radius_row = adw::SpinRow::with_range(0.0, 200.0, 2.0);
            corner_radius_row.set_title("Eckenradius");
            let padding_x_row = adw::SpinRow::with_range(0.0, 200.0, 2.0);
            padding_x_row.set_title("Innenabstand horizontal");
            let padding_y_row = adw::SpinRow::with_range(0.0, 200.0, 2.0);
            padding_y_row.set_title("Innenabstand vertikal");
            let wrap_row = adw::SwitchRow::builder().title("Automatisch umbrechen").build();
            let line_spacing_row = adw::SpinRow::with_range(0.5, 3.0, 0.1);
            line_spacing_row.set_title("Zeilenabstand");
            line_spacing_row.set_subtitle("Faktor der Schriftgröße, 1.0 = normal");
            line_spacing_row.set_digits(1);

            let shadow_row = adw::ComboRow::builder().title("Schatten").build();
            shadow_row.set_model(Some(&gtk4::StringList::new(&["Kein Schatten", "Subtil", "Standard", "Stark", "Floating", "Angepasst"])));
            let shadow_angle_row = adw::SpinRow::with_range(0.0, 360.0, 5.0);
            shadow_angle_row.set_title("Schatten-Winkel");
            let shadow_distance_row = adw::SpinRow::with_range(0.0, 300.0, 2.0);
            shadow_distance_row.set_title("Schatten-Distanz");
            let shadow_blur_row = adw::SpinRow::with_range(0.0, 150.0, 2.0);
            shadow_blur_row.set_title("Weichzeichner");

            // Populates every row from a given style — called once below
            // to seed the initial values (*before* any change handler is
            // attached, so that first call can never itself trigger a
            // spurious commit) and returned as `sync` for the sidebar's
            // ongoing use afterward.
            let populate: LabelStyleSync = {
                let position_mode_row = position_mode_row.clone();
                let horizontal_row = horizontal_row.clone();
                let vertical_row = vertical_row.clone();
                let padding_row = padding_row.clone();
                let x_row = x_row.clone();
                let y_row = y_row.clone();
                let background_row = background_row.clone();
                let background_color_row = background_color_row.clone();
                let background_color2_row = background_color2_row.clone();
                let background_color_button = background_color_button.clone();
                let background_color2_button = background_color2_button.clone();
                let font_button = font_button.clone();
                let alignment_row = alignment_row.clone();
                let color_button = color_button.clone();
                let opacity_row = opacity_row.clone();
                let corner_radius_row = corner_radius_row.clone();
                let padding_x_row = padding_x_row.clone();
                let padding_y_row = padding_y_row.clone();
                let wrap_row = wrap_row.clone();
                let line_spacing_row = line_spacing_row.clone();
                let shadow_row = shadow_row.clone();
                let shadow_angle_row = shadow_angle_row.clone();
                let shadow_distance_row = shadow_distance_row.clone();
                let shadow_blur_row = shadow_blur_row.clone();
                Rc::new(move |defaults: &LabelStyle| {
                    let is_absolute = matches!(defaults.position, TextPosition::Absolute { .. });
                    position_mode_row.set_selected(if is_absolute { 1 } else { 0 });
                    horizontal_row.set_visible(!is_absolute);
                    vertical_row.set_visible(!is_absolute);
                    padding_row.set_visible(!is_absolute);
                    x_row.set_visible(is_absolute);
                    y_row.set_visible(is_absolute);
                    match defaults.position {
                        TextPosition::Semantic { horizontal, vertical, padding } => {
                            horizontal_row.set_selected(index_for_horizontal_anchor(horizontal));
                            vertical_row.set_selected(index_for_vertical_anchor(vertical));
                            padding_row.set_value(padding);
                        }
                        TextPosition::Absolute { x, y } => {
                            x_row.set_value(x);
                            y_row.set_value(y);
                        }
                    }
                    let background_index = match &defaults.background {
                        TextBackground::None => 0,
                        TextBackground::Solid(_) => 1,
                        TextBackground::Gradient(_) => 2,
                    };
                    background_row.set_selected(background_index);
                    background_color_row.set_visible(background_index != 0);
                    background_color2_row.set_visible(background_index == 2);
                    match &defaults.background {
                        TextBackground::Solid(c) => background_color_button.set_rgba(&gdk_rgba_from(c)),
                        TextBackground::Gradient(spec) => {
                            if let Some((_, c)) = spec.stops.first() {
                                background_color_button.set_rgba(&gdk_rgba_from(c));
                            }
                            if let Some((_, c)) = spec.stops.get(1) {
                                background_color2_button.set_rgba(&gdk_rgba_from(c));
                            }
                        }
                        TextBackground::None => {}
                    }
                    font_button.set_font_desc(&font_desc_from_typography(&defaults.typography));
                    alignment_row.set_selected(index_for_text_align(defaults.typography.alignment));
                    color_button.set_rgba(&gdk_rgba_from(&defaults.typography.color));
                    opacity_row.set_value(defaults.typography.opacity * 100.0);
                    corner_radius_row.set_value(defaults.corner_radius.top_left);
                    padding_x_row.set_value(defaults.padding_x);
                    padding_y_row.set_value(defaults.padding_y);
                    wrap_row.set_active(defaults.typography.wrap);
                    line_spacing_row.set_value(defaults.typography.line_spacing);
                    shadow_row.set_selected(shadow_preset_index_for(&defaults.shadow));
                    let (angle, distance) = defaults.shadow.angle_and_distance();
                    shadow_angle_row.set_value(angle);
                    shadow_distance_row.set_value(distance);
                    shadow_blur_row.set_value(defaults.shadow.blur);
                    let shadow_geometry_enabled = defaults.shadow.enabled;
                    shadow_angle_row.set_sensitive(shadow_geometry_enabled);
                    shadow_distance_row.set_sensitive(shadow_geometry_enabled);
                    shadow_blur_row.set_sensitive(shadow_geometry_enabled);
                })
            };
            populate(&defaults);

            let apply = glib::clone!(
                #[strong]
                get,
                #[strong]
                commit,
                #[strong]
                is_syncing,
                #[weak]
                position_mode_row,
                #[weak]
                horizontal_row,
                #[weak]
                vertical_row,
                #[weak]
                padding_row,
                #[weak]
                x_row,
                #[weak]
                y_row,
                #[weak]
                background_row,
                #[weak]
                background_color_button,
                #[weak]
                background_color2_button,
                #[weak]
                font_button,
                #[weak]
                alignment_row,
                #[weak]
                color_button,
                #[weak]
                opacity_row,
                #[weak]
                corner_radius_row,
                #[weak]
                padding_x_row,
                #[weak]
                padding_y_row,
                #[weak]
                wrap_row,
                #[weak]
                line_spacing_row,
                move || {
                    if is_syncing() {
                        return;
                    }
                    let old = get();
                    let position = if position_mode_row.selected() == 1 {
                        TextPosition::Absolute { x: x_row.value(), y: y_row.value() }
                    } else {
                        TextPosition::Semantic {
                            horizontal: horizontal_anchor_for_index(horizontal_row.selected()),
                            vertical: vertical_anchor_for_index(vertical_row.selected()),
                            padding: padding_row.value(),
                        }
                    };
                    let background = match background_row.selected() {
                        1 => TextBackground::Solid(rgba_from_gdk(&background_color_button.rgba())),
                        2 => TextBackground::Gradient(GradientSpec {
                            kind: GradientKind::Linear { angle_deg: 135.0 },
                            stops: vec![
                                (0.0, rgba_from_gdk(&background_color_button.rgba())),
                                (1.0, rgba_from_gdk(&background_color2_button.rgba())),
                            ],
                        }),
                        _ => TextBackground::None,
                    };
                    let font_desc = font_button.font_desc().unwrap_or_else(pango::FontDescription::new);
                    let (font_family, font_size, weight, italic) = typography_from_font_desc(&font_desc);
                    // The shadow *preset*/geometry rows have their own
                    // dedicated handlers below (mirroring the per-label
                    // shadow split), so this closure carries the shadow
                    // over from whatever's already current.
                    let shadow = old.shadow;
                    let new = LabelStyle {
                        position,
                        typography: Typography {
                            font_family,
                            font_size,
                            weight,
                            italic,
                            color: rgba_from_gdk(&color_button.rgba()),
                            alignment: text_align_for_index(alignment_row.selected()),
                            opacity: opacity_row.value() / 100.0,
                            letter_spacing: old.typography.letter_spacing,
                            line_spacing: line_spacing_row.value(),
                            wrap: wrap_row.is_active(),
                        },
                        background,
                        corner_radius: CornerRadius::uniform(corner_radius_row.value()),
                        padding_x: padding_x_row.value(),
                        padding_y: padding_y_row.value(),
                        shadow,
                    };
                    if old == new {
                        return;
                    }
                    commit(old, new);
                }
            );

            position_mode_row.connect_selected_notify(glib::clone!(
                #[weak]
                horizontal_row,
                #[weak]
                vertical_row,
                #[weak]
                padding_row,
                #[weak]
                x_row,
                #[weak]
                y_row,
                #[strong]
                apply,
                move |row| {
                    let is_absolute = row.selected() == 1;
                    horizontal_row.set_visible(!is_absolute);
                    vertical_row.set_visible(!is_absolute);
                    padding_row.set_visible(!is_absolute);
                    x_row.set_visible(is_absolute);
                    y_row.set_visible(is_absolute);
                    apply();
                }
            ));
            horizontal_row.connect_selected_notify(glib::clone!(#[strong] apply, move |_| apply()));
            vertical_row.connect_selected_notify(glib::clone!(#[strong] apply, move |_| apply()));
            padding_row.connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));
            x_row.connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));
            y_row.connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));
            background_row.connect_selected_notify(glib::clone!(
                #[weak]
                background_color_row,
                #[weak]
                background_color2_row,
                #[strong]
                apply,
                move |row| {
                    let selected = row.selected();
                    background_color_row.set_visible(selected != 0);
                    background_color2_row.set_visible(selected == 2);
                    apply();
                }
            ));
            background_color_button.connect_rgba_notify(glib::clone!(#[strong] apply, move |_| apply()));
            background_color2_button.connect_rgba_notify(glib::clone!(#[strong] apply, move |_| apply()));
            font_button.connect_font_desc_notify(glib::clone!(#[strong] apply, move |_| apply()));
            alignment_row.connect_selected_notify(glib::clone!(#[strong] apply, move |_| apply()));
            color_button.connect_rgba_notify(glib::clone!(#[strong] apply, move |_| apply()));
            opacity_row.connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));
            corner_radius_row.connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));
            padding_x_row.connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));
            padding_y_row.connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));
            wrap_row.connect_active_notify(glib::clone!(#[strong] apply, move |_| apply()));
            line_spacing_row.connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));

            // Shadow preset/geometry: same preset-preserves-angle split as
            // every other shadow control in this app
            // (`ShadowParams::with_preset`), committed directly since
            // `apply` above deliberately doesn't touch the shadow.
            shadow_row.connect_selected_notify(glib::clone!(
                #[strong]
                get,
                #[strong]
                commit,
                #[strong]
                is_syncing,
                #[weak]
                shadow_angle_row,
                #[weak]
                shadow_distance_row,
                #[weak]
                shadow_blur_row,
                move |row| {
                    if is_syncing() {
                        return;
                    }
                    // "Angepasst" only ever reflects a shadow that doesn't
                    // match any real preset — selecting it has nothing
                    // coherent to apply.
                    if row.selected() == CUSTOM_SHADOW_PRESET_INDEX {
                        return;
                    }
                    let old = get();
                    let preset = shadow_preset_for_index(row.selected());
                    let new_shadow = old.shadow.with_preset(preset);
                    shadow_distance_row.set_value(new_shadow.angle_and_distance().1);
                    shadow_blur_row.set_value(new_shadow.blur);
                    shadow_angle_row.set_sensitive(new_shadow.enabled);
                    shadow_distance_row.set_sensitive(new_shadow.enabled);
                    shadow_blur_row.set_sensitive(new_shadow.enabled);
                    let mut new = old.clone();
                    new.shadow = new_shadow;
                    if old == new {
                        return;
                    }
                    commit(old, new);
                }
            ));
            let apply_shadow_geometry = glib::clone!(
                #[strong]
                get,
                #[strong]
                commit,
                #[strong]
                is_syncing,
                #[weak]
                shadow_row,
                #[weak]
                shadow_angle_row,
                #[weak]
                shadow_distance_row,
                #[weak]
                shadow_blur_row,
                move || {
                    if is_syncing() {
                        return;
                    }
                    let old = get();
                    let (offset_x, offset_y) =
                        ShadowParams::offset_for_angle_and_distance(shadow_angle_row.value(), shadow_distance_row.value());
                    let mut new = old.clone();
                    new.shadow.offset_x = offset_x;
                    new.shadow.offset_y = offset_y;
                    new.shadow.blur = shadow_blur_row.value();
                    if old == new {
                        return;
                    }
                    commit(old, new.clone());
                    // See the matching comment on the screenshot-level
                    // `apply_shadow_geometry` (`register_effect_controls`)
                    // for why the dropdown needs this explicit nudge —
                    // its own selection only updates on an actual pick.
                    shadow_row.set_selected(shadow_preset_index_for(&new.shadow));
                }
            );
            shadow_angle_row.connect_value_notify(glib::clone!(#[strong] apply_shadow_geometry, move |_| apply_shadow_geometry()));
            shadow_distance_row.connect_value_notify(glib::clone!(#[strong] apply_shadow_geometry, move |_| apply_shadow_geometry()));
            shadow_blur_row.connect_value_notify(glib::clone!(#[strong] apply_shadow_geometry, move |_| apply_shadow_geometry()));

            let position_group = adw::PreferencesGroup::new();
            position_group.set_title("Position");
            position_group.add(&position_mode_row);
            position_group.add(&horizontal_row);
            position_group.add(&vertical_row);
            position_group.add(&padding_row);
            position_group.add(&x_row);
            position_group.add(&y_row);

            let look_group = adw::PreferencesGroup::new();
            look_group.set_title("Schrift &amp; Farbe");
            if let Some(description) = scope_description {
                look_group.set_description(Some(description));
            }
            look_group.add(&font_row);
            look_group.add(&alignment_row);
            look_group.add(&color_row);
            look_group.add(&opacity_row);
            look_group.add(&background_row);
            look_group.add(&background_color_row);
            look_group.add(&background_color2_row);

            let styling_group = adw::PreferencesGroup::new();
            styling_group.set_title("Gestaltung");
            styling_group.add(&corner_radius_row);
            styling_group.add(&padding_x_row);
            styling_group.add(&padding_y_row);
            styling_group.add(&wrap_row);
            styling_group.add(&line_spacing_row);
            styling_group.add(&shadow_row);
            styling_group.add(&shadow_angle_row);
            styling_group.add(&shadow_distance_row);
            styling_group.add(&shadow_blur_row);

            (look_group, position_group, styling_group, populate)
}

/// The app-wide "default label style" groups (spec: "globale
/// Anwendungseinstellungen" — the top tier), appended onto the
/// "Allgemein" page (`build_general_page`) rather than living in their
/// own dialog page, since that's exactly the same category as the
/// spacing/margin/export-quality rows already there: values that seed a
/// *new* project and are otherwise inert. Writes straight to `GSettings`
/// (`save_global_label_defaults`) with no undo history of its own — an
/// app-wide setting isn't part of any document's undo stack. The dialog
/// page these groups live on is thrown away and rebuilt fresh every time
/// it's opened, so there's no ongoing resync need — `is_syncing` is a
/// constant `false`.
fn build_global_label_defaults_groups() -> (adw::PreferencesGroup, adw::PreferencesGroup, adw::PreferencesGroup) {
    let get: Rc<dyn Fn() -> LabelStyle> = Rc::new(load_global_label_defaults);
    let commit: Rc<dyn Fn(LabelStyle, LabelStyle)> = Rc::new(|_old, new| save_global_label_defaults(&new));
    let is_syncing: Rc<dyn Fn() -> bool> = Rc::new(|| false);
    let (look_group, position_group, styling_group, _sync) = build_label_style_groups(
        get,
        commit,
        is_syncing,
        Some("Ausgangswerte für neu erstellte Projekte — bereits bestehende Projekte bleiben davon unverändert"),
    );
    (look_group, position_group, styling_group)
}

/// The project-wide label style, live in the sidebar (spec: "im Projekt
/// sollen sich die Einstellungen anpassen lassen, für alle Screenshots
/// gemeinsam, nicht getrennt einzeln") — the middle tier of the
/// three-level configuration: seeded from the app-wide default when the
/// project is created (`EditorState::new`), freely editable here
/// afterward without ever reaching back into that global setting, and
/// captured into a preset alongside the project's other settings
/// (`Template::from_document`). Always visible regardless of selection,
/// like Layout/Hintergrund/Effekte — unlike the per-screenshot "Label"
/// section right above it (enabled/content only), since this is the one
/// shared style every label in the project uses.
///
/// Inserts the three groups right after the sidebar's per-screenshot
/// `label_group`, using `AdwPreferencesPage::remove`/`add` to reorder —
/// the only way to place them precisely, since `add` alone only ever
/// appends to the end of the page. Returns nothing; the `sync` closure
/// `build_label_style_groups` hands back is stored on `EditorState` so
/// `sync_controls_from_document` can keep these rows in step with
/// undo/redo, project load, preset apply, and canvas label-drags.
fn register_label_style_controls(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let get: Rc<dyn Fn() -> LabelStyle> = {
        let state = state.clone();
        Rc::new(move || state.borrow().document.label_defaults.clone())
    };
    let commit: Rc<dyn Fn(LabelStyle, LabelStyle)> = {
        let window = window.clone();
        let canvas = canvas.clone();
        let state = state.clone();
        Rc::new(move |old, new| {
            let mut state_ref = state.borrow_mut();
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetLabelDefaults { old, new }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        })
    };
    let is_syncing: Rc<dyn Fn() -> bool> = {
        let state = state.clone();
        Rc::new(move || state.borrow().syncing_controls)
    };
    let (look_group, position_group, styling_group, sync) = build_label_style_groups(
        get,
        commit,
        is_syncing,
        Some("Für alle Labels dieses Projekts gemeinsam — nicht einzeln pro Screenshot"),
    );

    let page = window.sidebar_page();
    let callouts_group = window.callouts_group();
    let export_group = window.export_group();
    page.remove(&callouts_group);
    page.remove(&export_group);
    page.add(&look_group);
    page.add(&position_group);
    page.add(&styling_group);
    page.add(&callouts_group);
    page.add(&export_group);

    state.borrow_mut().label_style_sync = Some(sync);
}

/// Re-syncs the Label sidebar section whenever the canvas selection
/// changes (spec: selecting a screenshot shows/focuses its label
/// controls directly, no dialog) — and, once synced, focuses the text
/// entry so typing a label is a single click away.
fn register_selection_sync(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    canvas.connect_selection_changed(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move || {
            sync_label_controls(&window, &canvas, &state);
            sync_callouts_controls(&window, &canvas, &state);
            if single_selected_label_target(&canvas, &state).is_some() {
                window.label_content_view().grab_focus();
            }
        }
    ));
}

/// Wires the canvas's own label-drag gesture to an undoable
/// `SetLabelDefaults` — every label in the project shares one position, so
/// dragging any one label's box moves them all identically. Works in every
/// layout mode, since a label's position is always relative to its own
/// screenshot regardless of how the screenshots themselves are arranged
/// (spec: label positioning is independent of the composition's
/// auto-layout).
fn register_label_drag(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    canvas.connect_label_move(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |new_position| {
            let mut state_ref = state.borrow_mut();
            let old = state_ref.document.label_defaults.clone();
            let new = LabelStyle { position: new_position, ..old.clone() };
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetLabelDefaults { old, new: new.clone() }), document);
            let sync = state_ref.label_style_sync.clone();
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            // The drag changed the sidebar's Position/X/Y rows from
            // outside the sidebar entirely, so they need an explicit
            // resync — nothing else triggers one for a canvas-originated
            // change (contrast the rows' own handlers, which are already
            // the source of truth for what they display).
            if let Some(sync) = sync {
                let was_syncing = state.borrow().syncing_controls;
                state.borrow_mut().syncing_controls = true;
                sync(&new);
                state.borrow_mut().syncing_controls = was_syncing;
            }
            update_undo_redo_sensitivity(&window, &state);
        }
    ));
}

/// Wires the canvas's own Alt-held wallpaper-drag gesture to an undoable
/// `SetBackground` — mirrors `register_label_drag`, but for a generated
/// background's own focus point (`offset_x`/`offset_y`) rather than a
/// screenshot's label. Never fires unless `doc.background` is already
/// `Background::Generated` at drag time (see `Canvas::connect_wallpaper_move`).
fn register_wallpaper_drag(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    canvas.connect_wallpaper_move(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |new_offset_x, new_offset_y| {
            let mut state_ref = state.borrow_mut();
            let Background::Generated(current) = &state_ref.document.background else { return };
            let mut new = current.clone();
            new.offset_x = new_offset_x;
            new.offset_y = new_offset_y;
            let old = state_ref.document.background.clone();
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetBackground { old, new: Background::Generated(new) }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));
}

/// The single currently-selected screenshot's own id, or `None` when 0 or
/// several are selected — the same "exactly one" rule
/// `single_selected_label_target` uses for the Label section, reused here
/// for the Callouts section since both are per-screenshot.
fn single_selected_screenshot_id(canvas: &Canvas, state: &Rc<RefCell<EditorState>>) -> Option<Uuid> {
    let selected = canvas.selected_ids();
    if selected.len() != 1 {
        return None;
    }
    let id = *selected.iter().next().unwrap();
    state.borrow().document.elements.iter().any(|e| e.id == id).then_some(id)
}

/// Rebuilds the sidebar's "Callouts" section from scratch for the current
/// single-selected screenshot — hidden entirely when 0 or several are
/// selected, same as the Label section. Called on selection change and as
/// part of `sync_controls_from_document` (after undo/redo/load), and after
/// any action that adds/removes a callout, since unlike the Label section
/// (always exactly one, always present) the *number* of rows itself can
/// change.
fn sync_callouts_controls(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let Some(element_id) = single_selected_screenshot_id(canvas, state) else {
        window.callouts_group().set_visible(false);
        return;
    };
    window.callouts_group().set_visible(true);

    let callouts = state.borrow().document.elements.iter().find(|e| e.id == element_id).map(|e| e.callouts.clone()).unwrap_or_default();

    let list_box = window.callouts_list_box();
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }
    list_box.set_visible(!callouts.is_empty());
    for callout in &callouts {
        list_box.append(&build_callout_row(window, canvas, state, element_id, callout));
    }
}

/// Builds one fully-wired `AdwExpanderRow` for a single callout: enable
/// switch and delete button in the row's own header, text/colors/corner
/// radius/arrow styling nested inside — collapsed by default so several
/// callouts on one screenshot stay manageable, expanding to edit rather
/// than needing a separate dialog. Rebuilt from scratch by
/// `sync_callouts_controls` whenever the callout list itself changes, so
/// this only needs to wire live-editing, not incremental updates.
fn build_callout_row(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>, element_id: Uuid, callout: &Callout) -> adw::ExpanderRow {
    let callout_id = callout.id;

    let row = adw::ExpanderRow::new();
    let title_for = |content: &str| if content.trim().is_empty() { "Callout".to_string() } else { content.to_string() };
    row.set_title(&title_for(&callout.text.content));

    let enabled_switch = gtk4::Switch::new();
    enabled_switch.set_active(callout.enabled);
    enabled_switch.set_valign(gtk4::Align::Center);
    row.add_suffix(&enabled_switch);

    let delete_button = gtk4::Button::from_icon_name("user-trash-symbolic");
    delete_button.set_valign(gtk4::Align::Center);
    delete_button.add_css_class("flat");
    delete_button.set_tooltip_text(Some("Callout löschen"));
    row.add_suffix(&delete_button);

    let content_box = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    content_box.set_margin_top(12);
    content_box.set_margin_bottom(12);
    content_box.set_margin_start(12);
    content_box.set_margin_end(12);
    let content_label = gtk4::Label::new(Some("Text"));
    content_label.set_halign(gtk4::Align::Start);
    content_label.add_css_class("caption-heading");
    content_box.append(&content_label);
    let content_scroller = gtk4::ScrolledWindow::new();
    content_scroller.set_hscrollbar_policy(gtk4::PolicyType::Never);
    content_scroller.set_min_content_height(64);
    content_scroller.set_max_content_height(120);
    content_scroller.add_css_class("card");
    let content_view = gtk4::TextView::new();
    content_view.set_wrap_mode(gtk4::WrapMode::WordChar);
    content_view.set_top_margin(8);
    content_view.set_bottom_margin(8);
    content_view.set_left_margin(8);
    content_view.set_right_margin(8);
    content_view.buffer().set_text(&callout.text.content);
    content_scroller.set_child(Some(&content_view));
    content_box.append(&content_scroller);
    row.add_row(&content_box);

    let wrap_row = adw::SwitchRow::new();
    wrap_row.set_title("Automatisch umbrechen");
    wrap_row.set_active(callout.text.typography.wrap);
    row.add_row(&wrap_row);

    let line_spacing_row = spin_row("Zeilenabstand", 0.5, 3.0, callout.text.typography.line_spacing);
    line_spacing_row.set_subtitle("Faktor der Schriftgröße, 1.0 = normal");
    row.add_row(&line_spacing_row);

    let font_row = adw::ActionRow::new();
    font_row.set_title("Schrift");
    let font_button = gtk4::FontDialogButton::new(Some(gtk4::FontDialog::new()));
    font_button.set_valign(gtk4::Align::Center);
    font_button.set_font_desc(&font_desc_from_typography(&callout.text.typography));
    font_row.add_suffix(&font_button);
    row.add_row(&font_row);

    let alignment_row = adw::ComboRow::builder().title("Textausrichtung").subtitle("Bei mehrzeiligem Text").build();
    alignment_row.set_model(Some(&gtk4::StringList::new(&["Links", "Mitte", "Rechts"])));
    alignment_row.set_selected(index_for_text_align(callout.text.typography.alignment));
    row.add_row(&alignment_row);

    let opacity_row = spin_row("Deckkraft", 0.0, 100.0, callout.text.typography.opacity * 100.0);
    opacity_row.set_subtitle("In Prozent");
    row.add_row(&opacity_row);

    let background_type_row = adw::ComboRow::builder().title("Hintergrund").build();
    background_type_row.set_model(Some(&gtk4::StringList::new(&["Kein Hintergrund", "Einfarbig", "Verlauf"])));
    let initial_background_index = match callout.text.background {
        TextBackground::None => 0,
        TextBackground::Solid(_) => 1,
        TextBackground::Gradient(_) => 2,
    };
    background_type_row.set_selected(initial_background_index);
    row.add_row(&background_type_row);

    let background_color_row = adw::ActionRow::new();
    background_color_row.set_title("Hintergrundfarbe");
    background_color_row.set_visible(initial_background_index != 0);
    let background_color_button = gtk4::ColorDialogButton::new(Some(gtk4::ColorDialog::builder().with_alpha(true).build()));
    background_color_button.set_valign(gtk4::Align::Center);
    let initial_background = match &callout.text.background {
        TextBackground::Solid(color) => *color,
        TextBackground::Gradient(spec) => spec.stops.first().map(|(_, c)| *c).unwrap_or(Rgba::WHITE),
        TextBackground::None => Rgba::WHITE,
    };
    background_color_button.set_rgba(&gdk_rgba_from(&initial_background));
    background_color_row.add_suffix(&background_color_button);
    row.add_row(&background_color_row);

    let background_color2_row = adw::ActionRow::new();
    background_color2_row.set_title("Hintergrundfarbe 2");
    background_color2_row.set_visible(initial_background_index == 2);
    let background_color2_button = gtk4::ColorDialogButton::new(Some(gtk4::ColorDialog::builder().with_alpha(true).build()));
    background_color2_button.set_valign(gtk4::Align::Center);
    if let TextBackground::Gradient(spec) = &callout.text.background {
        if let Some((_, c)) = spec.stops.get(1) {
            background_color2_button.set_rgba(&gdk_rgba_from(c));
        }
    }
    background_color2_row.add_suffix(&background_color2_button);
    row.add_row(&background_color2_row);

    let color_row = adw::ActionRow::new();
    color_row.set_title("Textfarbe");
    let color_button = gtk4::ColorDialogButton::new(Some(gtk4::ColorDialog::new()));
    color_button.set_valign(gtk4::Align::Center);
    color_button.set_rgba(&gdk_rgba_from(&callout.text.typography.color));
    color_row.add_suffix(&color_button);
    row.add_row(&color_row);

    let corner_radius_row = spin_row("Eckenradius", 0.0, 200.0, callout.text.corner_radius.top_left);
    row.add_row(&corner_radius_row);

    let padding_x_row = spin_row("Innenabstand horizontal", 0.0, 200.0, callout.text.padding_x);
    row.add_row(&padding_x_row);

    let padding_y_row = spin_row("Innenabstand vertikal", 0.0, 200.0, callout.text.padding_y);
    row.add_row(&padding_y_row);

    let shadow_row = adw::ComboRow::builder().title("Schatten").build();
    shadow_row.set_model(Some(&gtk4::StringList::new(&["Kein Schatten", "Subtil", "Standard", "Stark", "Floating", "Angepasst"])));
    shadow_row.set_selected(shadow_preset_index_for(&callout.text.shadow));
    row.add_row(&shadow_row);

    let (initial_shadow_angle, initial_shadow_distance) = callout.text.shadow.angle_and_distance();
    let shadow_angle_row = spin_row("Schatten-Winkel", 0.0, 360.0, initial_shadow_angle);
    shadow_angle_row.set_sensitive(callout.text.shadow.enabled);
    row.add_row(&shadow_angle_row);

    let shadow_distance_row = spin_row("Schatten-Distanz", 0.0, 300.0, initial_shadow_distance);
    shadow_distance_row.set_sensitive(callout.text.shadow.enabled);
    row.add_row(&shadow_distance_row);

    let shadow_blur_row = spin_row("Weichzeichner", 0.0, 150.0, callout.text.shadow.blur);
    shadow_blur_row.set_sensitive(callout.text.shadow.enabled);
    row.add_row(&shadow_blur_row);

    let arrow_color_row = adw::ActionRow::new();
    arrow_color_row.set_title("Pfeilfarbe");
    let arrow_color_button = gtk4::ColorDialogButton::new(Some(gtk4::ColorDialog::builder().with_alpha(true).build()));
    arrow_color_button.set_valign(gtk4::Align::Center);
    arrow_color_button.set_rgba(&gdk_rgba_from(&callout.arrow_color));
    arrow_color_row.add_suffix(&arrow_color_button);
    row.add_row(&arrow_color_row);

    let arrow_width_row = spin_row("Pfeilbreite", 0.5, 20.0, callout.arrow_width);
    row.add_row(&arrow_width_row);

    let dot_radius_row = spin_row("Punktgröße", 0.0, 20.0, callout.dot_radius);
    row.add_row(&dot_radius_row);

    let apply = glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        #[weak]
        enabled_switch,
        #[weak]
        content_view,
        #[weak]
        wrap_row,
        #[weak]
        line_spacing_row,
        #[weak]
        font_button,
        #[weak]
        alignment_row,
        #[weak]
        opacity_row,
        #[weak]
        background_type_row,
        #[weak]
        background_color_button,
        #[weak]
        background_color2_button,
        #[weak]
        color_button,
        #[weak]
        corner_radius_row,
        #[weak]
        padding_x_row,
        #[weak]
        padding_y_row,
        #[weak]
        arrow_color_button,
        #[weak]
        arrow_width_row,
        #[weak]
        dot_radius_row,
        move || {
            if state.borrow().syncing_controls {
                return;
            }
            let mut state_ref = state.borrow_mut();
            let Some(element) = state_ref.document.elements.iter().find(|e| e.id == element_id) else { return };
            let Some(current) = element.callouts.iter().find(|c| c.id == callout_id).cloned() else { return };

            let buffer = content_view.buffer();
            let mut new = current.clone();
            new.enabled = enabled_switch.is_active();
            new.text.content = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string();
            new.text.typography.wrap = wrap_row.is_active();
            new.text.typography.line_spacing = line_spacing_row.value();
            let font_desc = font_button.font_desc().unwrap_or_else(pango::FontDescription::new);
            let (font_family, font_size, weight, italic) = typography_from_font_desc(&font_desc);
            new.text.typography.font_family = font_family;
            new.text.typography.font_size = font_size;
            new.text.typography.weight = weight;
            new.text.typography.italic = italic;
            new.text.typography.alignment = text_align_for_index(alignment_row.selected());
            new.text.typography.opacity = opacity_row.value() / 100.0;
            new.text.background = match background_type_row.selected() {
                1 => TextBackground::Solid(rgba_from_gdk(&background_color_button.rgba())),
                2 => TextBackground::Gradient(GradientSpec {
                    kind: GradientKind::Linear { angle_deg: 135.0 },
                    stops: vec![
                        (0.0, rgba_from_gdk(&background_color_button.rgba())),
                        (1.0, rgba_from_gdk(&background_color2_button.rgba())),
                    ],
                }),
                _ => TextBackground::None,
            };
            new.text.typography.color = rgba_from_gdk(&color_button.rgba());
            new.text.corner_radius = CornerRadius::uniform(corner_radius_row.value());
            new.text.padding_x = padding_x_row.value();
            new.text.padding_y = padding_y_row.value();
            new.arrow_color = rgba_from_gdk(&arrow_color_button.rgba());
            new.arrow_width = arrow_width_row.value();
            new.dot_radius = dot_radius_row.value();
            if current == new {
                return;
            }
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetCallout { element_id, callout_id, old: current, new }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    );

    enabled_switch.connect_active_notify(glib::clone!(
        #[strong]
        apply,
        move |_| apply()
    ));
    content_view.buffer().connect_changed(glib::clone!(
        #[strong]
        apply,
        #[weak]
        row,
        move |buffer| {
            row.set_title(&title_for(&buffer.text(&buffer.start_iter(), &buffer.end_iter(), false)));
            apply();
        }
    ));
    wrap_row.connect_active_notify(glib::clone!(
        #[strong]
        apply,
        move |_| apply()
    ));
    line_spacing_row.connect_value_notify(glib::clone!(
        #[strong]
        apply,
        move |_| apply()
    ));
    font_button.connect_font_desc_notify(glib::clone!(
        #[strong]
        apply,
        move |_| apply()
    ));
    alignment_row.connect_selected_notify(glib::clone!(
        #[strong]
        apply,
        move |_| apply()
    ));
    opacity_row.connect_value_notify(glib::clone!(
        #[strong]
        apply,
        move |_| apply()
    ));
    background_type_row.connect_selected_notify(glib::clone!(
        #[weak]
        background_color_row,
        #[weak]
        background_color2_row,
        #[strong]
        apply,
        move |row| {
            let selected = row.selected();
            background_color_row.set_visible(selected != 0);
            background_color2_row.set_visible(selected == 2);
            apply();
        }
    ));
    background_color_button.connect_rgba_notify(glib::clone!(
        #[strong]
        apply,
        move |_| apply()
    ));
    background_color2_button.connect_rgba_notify(glib::clone!(
        #[strong]
        apply,
        move |_| apply()
    ));
    color_button.connect_rgba_notify(glib::clone!(
        #[strong]
        apply,
        move |_| apply()
    ));
    corner_radius_row.connect_value_notify(glib::clone!(
        #[strong]
        apply,
        move |_| apply()
    ));
    padding_x_row.connect_value_notify(glib::clone!(
        #[strong]
        apply,
        move |_| apply()
    ));
    padding_y_row.connect_value_notify(glib::clone!(
        #[strong]
        apply,
        move |_| apply()
    ));
    arrow_color_button.connect_rgba_notify(glib::clone!(
        #[strong]
        apply,
        move |_| apply()
    ));
    arrow_width_row.connect_value_notify(glib::clone!(
        #[strong]
        apply,
        move |_| apply()
    ));
    dot_radius_row.connect_value_notify(glib::clone!(
        #[strong]
        apply,
        move |_| apply()
    ));

    // Shadow preset/geometry: same preset-preserves-angle split as every
    // other shadow control in this app (`ShadowParams::with_preset`), and
    // the same `syncing_controls` guard as the screenshot-level version
    // in `register_effect_controls` — needed here too, since setting
    // `shadow_distance_row`/`shadow_blur_row` below reentrantly fires
    // `apply_shadow_geometry`, which needs its own borrow of `state`.
    shadow_row.connect_selected_notify(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        #[weak]
        shadow_angle_row,
        #[weak]
        shadow_distance_row,
        #[weak]
        shadow_blur_row,
        move |row| {
            // Bail out *before* touching any sibling widget — see the
            // matching comment on the screenshot-level `shadow_row`
            // handler (`register_effect_controls`) for why: this handler
            // reenters whenever `syncing_controls` is already set, and
            // the writes below must not run unconditionally in that case.
            if state.borrow().syncing_controls {
                return;
            }
            // "Angepasst" only ever reflects a shadow that doesn't match
            // any real preset — selecting it has nothing coherent to apply.
            if row.selected() == CUSTOM_SHADOW_PRESET_INDEX {
                return;
            }
            let mut state_ref = state.borrow_mut();
            let Some(element) = state_ref.document.elements.iter().find(|e| e.id == element_id) else { return };
            let Some(current) = element.callouts.iter().find(|c| c.id == callout_id).cloned() else { return };
            let preset = shadow_preset_for_index(row.selected());
            let new_shadow = current.text.shadow.with_preset(preset);

            // Guard these writes the same way the screenshot-level
            // handler does: `set_value` below reentrantly fires
            // `apply_shadow_geometry`, which needs its own borrow of
            // `state` — held open across these calls, `state_ref` here
            // would make that borrow panic instead of just no-op'ing.
            state_ref.syncing_controls = true;
            drop(state_ref);

            shadow_distance_row.set_value(new_shadow.angle_and_distance().1);
            shadow_blur_row.set_value(new_shadow.blur);
            shadow_angle_row.set_sensitive(new_shadow.enabled);
            shadow_distance_row.set_sensitive(new_shadow.enabled);
            shadow_blur_row.set_sensitive(new_shadow.enabled);

            let mut state_ref = state.borrow_mut();
            state_ref.syncing_controls = false;
            let Some(element) = state_ref.document.elements.iter().find(|e| e.id == element_id) else { return };
            let Some(current) = element.callouts.iter().find(|c| c.id == callout_id).cloned() else { return };
            let mut new = current.clone();
            new.text.shadow = new_shadow;
            if current == new {
                return;
            }
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetCallout { element_id, callout_id, old: current, new }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));

    let apply_shadow_geometry = glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        #[weak]
        shadow_row,
        #[weak]
        shadow_angle_row,
        #[weak]
        shadow_distance_row,
        #[weak]
        shadow_blur_row,
        move || {
            let mut state_ref = state.borrow_mut();
            if state_ref.syncing_controls {
                return;
            }
            let Some(element) = state_ref.document.elements.iter().find(|e| e.id == element_id) else { return };
            let Some(current) = element.callouts.iter().find(|c| c.id == callout_id).cloned() else { return };
            let (offset_x, offset_y) = ShadowParams::offset_for_angle_and_distance(shadow_angle_row.value(), shadow_distance_row.value());
            let mut new = current.clone();
            new.text.shadow.offset_x = offset_x;
            new.text.shadow.offset_y = offset_y;
            new.text.shadow.blur = shadow_blur_row.value();
            if current == new {
                return;
            }
            let new_shadow = new.text.shadow;
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetCallout { element_id, callout_id, old: current, new }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            // See the matching comment on the screenshot-level
            // `apply_shadow_geometry` (`register_effect_controls`) for why
            // the dropdown needs this explicit nudge — its own selection
            // only updates on an actual pick.
            shadow_row.set_selected(shadow_preset_index_for(&new_shadow));
            update_undo_redo_sensitivity(&window, &state);
        }
    );
    shadow_angle_row.connect_value_notify(glib::clone!(#[strong] apply_shadow_geometry, move |_| apply_shadow_geometry()));
    shadow_distance_row.connect_value_notify(glib::clone!(#[strong] apply_shadow_geometry, move |_| apply_shadow_geometry()));
    shadow_blur_row.connect_value_notify(glib::clone!(#[strong] apply_shadow_geometry, move |_| apply_shadow_geometry()));

    delete_button.connect_clicked(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |_| {
            let mut state_ref = state.borrow_mut();
            let Some(element) = state_ref.document.elements.iter().find(|e| e.id == element_id) else { return };
            let Some(index) = element.callouts.iter().position(|c| c.id == callout_id) else { return };
            let callout = element.callouts[index].clone();
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(RemoveCallout { element_id, index, callout }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            sync_callouts_controls(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));

    row
}

/// Adds a new callout to the current single-selected screenshot (spec:
/// "Callouts/Feature-Hinweise") and re-syncs the sidebar so it shows up
/// immediately, expanded state aside — a no-op if 0 or several screenshots
/// are selected, same as every other Label/Callout action.
fn add_callout_to_selected_screenshot(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let Some(element_id) = single_selected_screenshot_id(canvas, state) else { return };
    let mut state_ref = state.borrow_mut();
    let Some(natural_width) = state_ref.document.elements.iter().find(|e| e.id == element_id).map(|e| e.natural_width) else { return };
    let callout = Callout::new_for_width(natural_width);
    let EditorState { document, undo_stack, .. } = &mut *state_ref;
    undo_stack.apply(Box::new(AddCallout { element_id, callout }), document);
    drop(state_ref);
    refresh_canvas(window, canvas, state);
    sync_callouts_controls(window, canvas, state);
    update_undo_redo_sensitivity(window, state);
}

/// Wires the sidebar's Callouts section: the initial build for whatever is
/// selected when the app starts, and the "+ Callout hinzufügen" button.
fn register_callouts_controls(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    sync_callouts_controls(window, canvas, state);

    window.add_callout_button().connect_clicked(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |_| add_callout_to_selected_screenshot(&window, &canvas, &state)
    ));
}

/// Wires the canvas's own callout-drag gestures (text bubble and arrow
/// target, each independently draggable) to undoable `SetCallout`s —
/// mirrors `register_label_drag`; works in every layout mode.
fn register_callout_drag(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    canvas.connect_callout_box_move(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |element_id, callout_id, new_position| {
            let mut state_ref = state.borrow_mut();
            let Some(element) = state_ref.document.elements.iter().find(|e| e.id == element_id) else { return };
            let Some(old) = element.callouts.iter().find(|c| c.id == callout_id).cloned() else { return };
            let mut new = old.clone();
            new.text.position = new_position;
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetCallout { element_id, callout_id, old, new }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));

    canvas.connect_callout_target_move(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |element_id, callout_id, new_x, new_y| {
            let mut state_ref = state.borrow_mut();
            let Some(element) = state_ref.document.elements.iter().find(|e| e.id == element_id) else { return };
            let Some(old) = element.callouts.iter().find(|c| c.id == callout_id).cloned() else { return };
            let mut new = old.clone();
            new.target_x = new_x;
            new.target_y = new_y;
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetCallout { element_id, callout_id, old, new }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));
}

/// A titled `AdwSpinRow` with a plain numeric adjustment — shared by every
/// callout row's "Eckenradius"/"Pfeilbreite" controls.
fn spin_row(title: &str, lower: f64, upper: f64, value: f64) -> adw::SpinRow {
    let adjustment = gtk4::Adjustment::new(value, lower, upper, 1.0, 10.0, 0.0);
    let row = adw::SpinRow::new(Some(&adjustment), 1.0, 1);
    row.set_title(title);
    row
}

/// Wires the "Bild" background's file picker, fit mode and opacity
/// controls (spec §8). Picking a file is the only action that actually
/// turns the background into `Background::Image` — selecting "Bild" in the
/// type row alone just reveals these controls, since there's nothing to
/// render without a file yet.
fn register_background_image_controls(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let button = window.background_image_button();
    let fit_row = window.background_image_fit_row();
    let opacity_row = window.background_image_opacity_row();

    button.connect_clicked(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |_| {
            let window = window.clone();
            let canvas = canvas.clone();
            let state = state.clone();
            glib::spawn_future_local(async move {
                let filter = gtk4::FileFilter::new();
                filter.add_mime_type("image/png");
                filter.add_mime_type("image/jpeg");
                filter.add_mime_type("image/webp");
                filter.set_name(Some("Bilder"));

                let dialog =
                    gtk4::FileDialog::builder().title("Hintergrundbild wählen").accept_label("Wählen").default_filter(&filter).build();

                let file = match dialog.open_future(Some(&window)).await {
                    Ok(file) => file,
                    Err(err) => {
                        if !err.matches(gtk4::DialogError::Dismissed) {
                            eprintln!("ScreenForge: background image dialog failed: {err}");
                        }
                        return;
                    }
                };
                let Some(path) = file.path() else { return };

                let mut state_ref = state.borrow_mut();
                if get_or_decode(&mut state_ref.image_cache, &path).is_none() {
                    return;
                }
                let fit = background_image_fit_for_index(window.background_image_fit_row().selected());
                let opacity = window.background_image_opacity_row().value() / 100.0;
                let old = state_ref.document.background.clone();
                let new = Background::Image(ImageBackgroundSpec { source: ImageSource::Path(path.clone()), fit, opacity });
                let EditorState { document, undo_stack, .. } = &mut *state_ref;
                undo_stack.apply(Box::new(SetBackground { old, new }), document);
                drop(state_ref);
                window.background_image_row().set_subtitle(&background_image_subtitle(&ImageSource::Path(path)));
                refresh_canvas(&window, &canvas, &state);
                update_undo_redo_sensitivity(&window, &state);
            });
        }
    ));

    fit_row.connect_selected_notify(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |row| {
            let mut state_ref = state.borrow_mut();
            let Background::Image(spec) = &state_ref.document.background else { return };
            let new_fit = background_image_fit_for_index(row.selected());
            if state_ref.syncing_controls || spec.fit == new_fit {
                return;
            }
            let old = state_ref.document.background.clone();
            let mut new_spec = spec.clone();
            new_spec.fit = new_fit;
            let new = Background::Image(new_spec);
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetBackground { old, new }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));

    opacity_row.connect_value_notify(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |row| {
            let mut state_ref = state.borrow_mut();
            let Background::Image(spec) = &state_ref.document.background else { return };
            let new_opacity = row.value() / 100.0;
            if state_ref.syncing_controls || (spec.opacity - new_opacity).abs() < f64::EPSILON {
                return;
            }
            let old = state_ref.document.background.clone();
            let mut new_spec = spec.clone();
            new_spec.opacity = new_opacity;
            let new = Background::Image(new_spec);
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetBackground { old, new }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));
}

/// Wires the "Automatische Farben" gradient button (spec: "Generate from
/// screenshots" / "Regenerate" — one button doing both, since a repeat
/// click naturally reads as "try another one").
fn register_gradient_auto_colors_control(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    window.gradient_generate_button().connect_clicked(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |_| generate_gradient_from_screenshots(&window, &canvas, &state)
    ));
}

/// Analyzes every currently visible screenshot's decoded pixels and
/// replaces the background with a suggested complementary gradient (spec
/// §3), preserving whichever of Linear/Radial the user currently has
/// selected. A no-op if nothing is imported yet — there's nothing to
/// analyze, and generating a plausible palette from zero screenshots would
/// just be an arbitrary color.
fn generate_gradient_from_screenshots(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let mut state_ref = state.borrow_mut();

    // Decode-on-demand into `image_cache` (self-healing/shared with every
    // other consumer — see its doc comment), then collect owned handles
    // (cheap: `DecodedImage` wraps an `Arc<[u8]>`) so the borrow of
    // `image_cache` ends before `PixelSample`s borrow from them below.
    let paths: Vec<PathBuf> = state_ref
        .document
        .elements
        .iter()
        .filter(|e| e.visible)
        .filter_map(|e| match &e.source {
            ImageSource::Path(path) => Some(path.clone()),
            ImageSource::Embedded { .. } => None,
        })
        .collect();
    let images: Vec<DecodedImage> =
        paths.iter().filter_map(|path| get_or_decode(&mut state_ref.image_cache, path).cloned()).collect();
    if images.is_empty() {
        return;
    }

    let seed = state_ref.gradient_auto_seed;
    state_ref.gradient_auto_seed = seed.wrapping_add(1);

    let samples: Vec<screenforge_core::palette::PixelSample> =
        images.iter().map(|image| screenforge_core::palette::PixelSample { bytes: &image.bytes, width: image.width, height: image.height }).collect();
    let mut spec = screenforge_core::palette::suggest_gradient(&samples, seed);
    // A suggestion is always computed as a linear gradient (an angle only
    // means something for Linear) -- if the user has Radial selected,
    // keep the same two suggested colors but center them, rather than
    // silently switching their chosen gradient kind back to Linear.
    if window.background_type_row().selected() == 2 {
        spec.kind = GradientKind::Radial { center_x: 0.5, center_y: 0.5 };
    }
    let new = Background::Gradient(spec);

    let old = state_ref.document.background.clone();
    let EditorState { document, undo_stack, .. } = &mut *state_ref;
    undo_stack.apply(Box::new(SetBackground { old, new: new.clone() }), document);
    drop(state_ref);

    // The generated colors didn't come from the color/angle controls (the
    // usual source of truth for `apply_background_from_controls`), so
    // those controls need to be told what actually landed, the same way
    // undo/redo/project-load does via `sync_background_controls` --
    // guarded the same way, since e.g. `set_rgba` below would otherwise
    // reentrantly fire `apply_background_from_controls` for each control.
    state.borrow_mut().syncing_controls = true;
    sync_background_controls(window, &new);
    state.borrow_mut().syncing_controls = false;

    refresh_canvas(window, canvas, state);
    update_undo_redo_sensitivity(window, state);
}

/// Wires every generator parameter control (style, color strategy, adapt-
/// to-screenshots, the eight sliders, and directly editing the seed row)
/// straight onto the current `GeneratedBackground` — live and undoable,
/// but never re-resolving the palette or picking a new seed on its own;
/// only `generate_background` (the "Generieren" button, or first
/// switching the background type to "Generiert") does that. This mirrors
/// `apply_title`/`apply_shadow_geometry`'s split elsewhere: cheap
/// parameter edits stay separate from the one action that's meant to
/// visibly reroll the composition.
fn register_generator_controls(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let apply = glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move || {
            let mut state_ref = state.borrow_mut();
            if state_ref.syncing_controls {
                return;
            }
            let Background::Generated(current) = state_ref.document.background.clone() else { return };
            let color_strategy = color_strategy_for_index(window.generator_color_strategy_row().selected());
            let palette = if matches!(color_strategy, ColorStrategy::Manual) {
                [
                    window.generator_manual_color_button_1().rgba(),
                    window.generator_manual_color_button_2().rgba(),
                    window.generator_manual_color_button_3().rgba(),
                    window.generator_manual_color_button_4().rgba(),
                ]
                .iter()
                .map(rgba_from_gdk)
                .collect()
            } else {
                current.palette.clone()
            };
            let style = generator_style_for_index(window.generator_style_row().selected());
            let mood = mood_for_index(window.generator_mood_row().selected());
            let seed = window.generator_seed_row().value() as u64;
            // Style and mood decide how a derived palette is resolved (see
            // `resolve_palette_for`), so switching either one re-resolves
            // it for the current seed rather than waiting for the next
            // "Generieren" click — otherwise the mood dropdown would look
            // like it does nothing.
            let palette = if !matches!(color_strategy, ColorStrategy::Manual) && (style != current.style || mood != current.mood) {
                let inverse_contrast = window.generator_inverse_contrast_row().value() / 100.0;
                resolve_generator_palette(&mut state_ref, color_strategy, inverse_contrast, seed, style, mood)
            } else {
                palette
            };
            let new = GeneratedBackground {
                seed,
                style,
                mood,
                grain: window.generator_grain_row().value() / 100.0,
                color_strategy,
                palette,
                adapt_to_screenshots: window.generator_adapt_row().is_active(),
                inverse_contrast: window.generator_inverse_contrast_row().value() / 100.0,
                corner_bias: window.generator_corner_bias_row().value() / 100.0,
                scale: window.generator_scale_row().value() / 100.0,
                // No sliders for these — `offset_x`/`offset_y` are only
                // ever changed by dragging the wallpaper directly on the
                // canvas (see `register_wallpaper_drag`), and the rest are
                // only ever redrawn from scratch by `generate_background`'s
                // own randomization — so editing any *other* generator
                // control must leave all of them exactly as they were.
                offset_x: current.offset_x,
                offset_y: current.offset_y,
                density: current.density,
                flow: current.flow,
                variation: current.variation,
                contrast: window.generator_contrast_row().value() / 100.0,
                softness: current.softness,
            };
            if current == new {
                return;
            }
            let old = state_ref.document.background.clone();
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetBackground { old, new: Background::Generated(new) }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    );

    window.generator_color_strategy_row().connect_selected_notify(glib::clone!(
        #[weak]
        window,
        #[strong]
        state,
        #[strong]
        apply,
        move |row| {
            let new_strategy = color_strategy_for_index(row.selected());
            // Entering Manual mode for the first time in this background
            // (not just re-visiting it) starts from a curated, genuinely
            // contrasting palette rather than whatever the *previous*
            // strategy happened to leave in the 4 buttons — spec: "die
            // vier Felder [enthielten] lediglich unterschiedliche
            // Rotwerte", which is exactly what inheriting e.g. a
            // `FromScreenshots` palette (same hue, only lightness varies)
            // produced. Guarded by `syncing_controls` so setting all 4
            // buttons doesn't fire `apply()` four times over.
            if matches!(new_strategy, ColorStrategy::Manual) {
                let was_already_manual =
                    matches!(&state.borrow().document.background, Background::Generated(g) if matches!(g.color_strategy, ColorStrategy::Manual));
                if !was_already_manual {
                    let was_syncing = state.borrow().syncing_controls;
                    state.borrow_mut().syncing_controls = true;
                    let buttons = [
                        window.generator_manual_color_button_1(),
                        window.generator_manual_color_button_2(),
                        window.generator_manual_color_button_3(),
                        window.generator_manual_color_button_4(),
                    ];
                    for (button, color) in buttons.iter().zip(screenforge_core::palette::DEFAULT_MANUAL_PALETTE.iter()) {
                        button.set_rgba(&gdk_rgba_from(color));
                    }
                    state.borrow_mut().syncing_controls = was_syncing;
                }
            }
            sync_generator_color_strategy_visibility(&window, true, new_strategy);
            apply();
        }
    ));
    window.generator_manual_color_button_1().connect_rgba_notify(glib::clone!(#[strong] apply, move |_| apply()));
    window.generator_manual_color_button_2().connect_rgba_notify(glib::clone!(#[strong] apply, move |_| apply()));
    window.generator_manual_color_button_3().connect_rgba_notify(glib::clone!(#[strong] apply, move |_| apply()));
    window.generator_manual_color_button_4().connect_rgba_notify(glib::clone!(#[strong] apply, move |_| apply()));
    window.generator_adapt_row().connect_active_notify(glib::clone!(#[strong] apply, move |_| apply()));
    window.generator_inverse_contrast_row().connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));
    window.generator_corner_bias_row().connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));
    window.generator_scale_row().connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));
    window.generator_contrast_row().connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));
    window.generator_seed_row().connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));
    window.generator_style_row().connect_selected_notify(glib::clone!(#[strong] apply, move |_| apply()));
    window.generator_mood_row().connect_selected_notify(glib::clone!(#[strong] apply, move |_| apply()));
    window.generator_grain_row().connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));

    window.generator_generate_button().connect_clicked(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |_| generate_background(&window, &canvas, &state)
    ));
}

/// Resolves a generator palette from the currently visible screenshots'
/// pixels (decoded through the shared image cache) for `strategy`, `style`
/// and `mood`. Not for `ColorStrategy::Manual`, whose palette is the user's
/// own colors.
fn resolve_generator_palette(
    state: &mut EditorState,
    strategy: ColorStrategy,
    inverse_contrast: f64,
    seed: u64,
    style: GeneratorStyle,
    mood: Mood,
) -> Vec<Rgba> {
    let paths: Vec<PathBuf> = state
        .document
        .elements
        .iter()
        .filter(|e| e.visible)
        .filter_map(|e| match &e.source {
            ImageSource::Path(path) => Some(path.clone()),
            ImageSource::Embedded { .. } => None,
        })
        .collect();
    let images: Vec<DecodedImage> = paths.iter().filter_map(|path| get_or_decode(&mut state.image_cache, path).cloned()).collect();
    let samples: Vec<screenforge_core::palette::PixelSample> = images
        .iter()
        .map(|image| screenforge_core::palette::PixelSample { bytes: &image.bytes, width: image.width, height: image.height })
        .collect();
    screenforge_core::palette::resolve_palette_for(&samples, strategy, inverse_contrast, seed, style, mood)
}

/// Resolves a fresh palette (from the currently visible screenshots, per
/// whatever color strategy is selected) and picks a new seed, then commits
/// a `Background::Generated` built from the current controls — the
/// "Generieren"/"Regenerate" action (spec: one button doing both, a repeat
/// click reading naturally as "try another one", same as the earlier
/// gradient auto-colors button). Also what switching the background type
/// to "Generiert" calls, since there's nothing to render yet at that point
/// either.
fn generate_background(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let mut state_ref = state.borrow_mut();
    if state_ref.syncing_controls {
        // Reached via `background_type_row`'s own notify handler, which
        // `sync_controls_from_document`'s `background_type_row.set_selected(5)`
        // fires reentrantly while restoring a saved/undone `Generated`
        // background. Regenerating here would discard the very seed and
        // palette that call was trying to restore — exactly the
        // reproducibility guarantee this whole feature exists for.
        return;
    }

    let previous_seed = match &state_ref.document.background {
        Background::Generated(g) => g.seed,
        _ => 0,
    };
    // A "Generieren"/"Regenerate" click rerolls the pattern itself, not
    // where the user has already dragged its focus point to — carried
    // over unchanged, the same way `sync_generator_controls` never touches
    // it either.
    let (previous_offset_x, previous_offset_y) = match &state_ref.document.background {
        Background::Generated(g) => (g.offset_x, g.offset_y),
        _ => (0.0, 0.0),
    };
    // A fresh seed each click, derived deterministically from the last one
    // (plus a fixed salt) through the same `Rng` generation itself uses —
    // this needs no external randomness source, and still gives a
    // different-looking result practically every time.
    let mut seed_source = screenforge_core::rng::Rng::new(previous_seed ^ 0x5EED_5EED_5EED_5EED);
    let new_seed = seed_source.next_u64() % 1_000_000_000;
    // Density/flow/variation/softness aren't exposed as sliders — spec:
    // "immer wieder neu Zufallswerte erzeugen" (always generate fresh random
    // values) rather than have the user tune them by hand. Drawing them from
    // the same `seed_source` keeps a "Generieren" click's whole result
    // (seed *and* these) deterministic from `previous_seed`, matching every
    // other value derived here.
    let density = seed_source.range(0.0, 1.0);
    let flow = seed_source.range(0.0, 1.0);
    let variation = seed_source.range(0.0, 1.0);
    let softness = seed_source.range(0.0, 1.0);

    let color_strategy = color_strategy_for_index(window.generator_color_strategy_row().selected());
    let inverse_contrast = window.generator_inverse_contrast_row().value() / 100.0;
    let style = generator_style_for_index(window.generator_style_row().selected());
    let mood = mood_for_index(window.generator_mood_row().selected());
    let palette = if matches!(color_strategy, ColorStrategy::Manual) {
        [
            window.generator_manual_color_button_1().rgba(),
            window.generator_manual_color_button_2().rgba(),
            window.generator_manual_color_button_3().rgba(),
            window.generator_manual_color_button_4().rgba(),
        ]
        .iter()
        .map(rgba_from_gdk)
        .collect()
    } else {
        resolve_generator_palette(&mut state_ref, color_strategy, inverse_contrast, new_seed, style, mood)
    };

    let new = GeneratedBackground {
        seed: new_seed,
        style,
        mood,
        grain: window.generator_grain_row().value() / 100.0,
        color_strategy,
        palette,
        adapt_to_screenshots: window.generator_adapt_row().is_active(),
        inverse_contrast,
        corner_bias: window.generator_corner_bias_row().value() / 100.0,
        offset_x: previous_offset_x,
        offset_y: previous_offset_y,
        scale: window.generator_scale_row().value() / 100.0,
        density,
        flow,
        variation,
        contrast: window.generator_contrast_row().value() / 100.0,
        softness,
    };

    let old = state_ref.document.background.clone();
    let EditorState { document, undo_stack, .. } = &mut *state_ref;
    undo_stack.apply(Box::new(SetBackground { old, new: Background::Generated(new.clone()) }), document);
    drop(state_ref);

    // The seed just changed; reflect it (and the freshly resolved
    // strategy-driven visibility) back onto the controls the same guarded
    // way `generate_gradient_from_screenshots` does.
    state.borrow_mut().syncing_controls = true;
    sync_generator_controls(window, &new);
    state.borrow_mut().syncing_controls = false;

    refresh_canvas(window, canvas, state);
    update_undo_redo_sensitivity(window, state);
}

fn export_format_for_index(index: u32) -> ExportFormat {
    match index {
        0 => ExportFormat::Png,
        1 => ExportFormat::Jpeg,
        2 => ExportFormat::WebP,
        _ => ExportFormat::Avif,
    }
}

fn index_for_export_format(format: ExportFormat) -> u32 {
    match format {
        ExportFormat::Png => 0,
        ExportFormat::Jpeg => 1,
        ExportFormat::WebP => 2,
        ExportFormat::Avif => 3,
    }
}

/// Wires the export-size/format/quality sidebar rows to `Document.canvas`.
/// These don't trigger a re-render (they don't affect the composition, only
/// the eventual export resolution/encoding), just a direct mutation.
fn register_export_controls(window: &Window, state: &Rc<RefCell<EditorState>>) {
    let width_row = window.export_width_row();
    let format_row = window.export_format_row();
    let quality_row = window.export_quality_row();

    {
        let canvas_settings = state.borrow().document.canvas;
        width_row.set_value(canvas_settings.export_target_width as f64);
        format_row.set_selected(index_for_export_format(canvas_settings.export_format));
        quality_row.set_value(canvas_settings.export_quality as f64);
        quality_row.set_sensitive(format_supports_quality(canvas_settings.export_format));
        update_export_height_display(window, canvas_settings);
    }

    // `export_height_row` is read-only (see its `sensitive: false` in the
    // template) — it only ever gets `set_value`d, by `update_export_height_display`,
    // never a change handler of its own.
    width_row.connect_value_notify(glib::clone!(
        #[weak]
        window,
        #[strong]
        state,
        move |row| {
            let canvas_settings = {
                let mut state_ref = state.borrow_mut();
                state_ref.document.canvas.export_target_width = row.value() as u32;
                state_ref.document.canvas
            };
            update_export_height_display(&window, canvas_settings);
        }
    ));
    format_row.connect_selected_notify(glib::clone!(
        #[weak]
        quality_row,
        #[strong]
        state,
        move |row| {
            let format = export_format_for_index(row.selected());
            state.borrow_mut().document.canvas.export_format = format;
            // Updates immediately on every format change, per spec — not
            // just at startup/undo-sync — so switching to PNG visibly
            // disables the control right away rather than leaving a
            // quality value that the encoder will just ignore.
            quality_row.set_sensitive(format_supports_quality(format));
        }
    ));
    quality_row.connect_value_notify(glib::clone!(
        #[strong]
        state,
        move |row| state.borrow_mut().document.canvas.export_quality = row.value() as u8
    ));
}

/// Whether `format`'s encoder in `export.rs` actually reads
/// `CanvasSettings.export_quality` — PNG is lossless and ignores it
/// entirely (see `export::render_and_write`'s `match`), so the Quality
/// spin row must be disabled rather than implying a control that does
/// nothing.
fn format_supports_quality(format: ExportFormat) -> bool {
    !matches!(format, ExportFormat::Png)
}

fn extension_for_format(format: ExportFormat) -> &'static str {
    match format {
        ExportFormat::Png => "png",
        ExportFormat::Jpeg => "jpg",
        ExportFormat::WebP => "webp",
        ExportFormat::Avif => "avif",
    }
}

/// The `win.export` action: picks a destination via `gtk::FileDialog::save`,
/// then renders and encodes at full resolution on a background thread
/// (`gio::spawn_blocking`) so the UI stays responsive, per spec §23/§14 — a
/// failed or slow export must never block or lose the in-memory document.
fn register_export_action(app: &adw::Application, window: &Window, state: &Rc<RefCell<EditorState>>) {
    let export_action = gio::SimpleAction::new("export", None);
    export_action.connect_activate(glib::clone!(
        #[weak]
        window,
        #[strong]
        state,
        move |_, _| {
            let window = window.clone();
            let state = state.clone();
            glib::spawn_future_local(async move {
                let format = state.borrow().document.canvas.export_format;
                let dialog = gtk4::FileDialog::builder()
                    .title("Komposition exportieren")
                    .accept_label("Exportieren")
                    .initial_name(format!("screenforge-export.{}", extension_for_format(format)))
                    .build();

                let file = match dialog.save_future(Some(&window)).await {
                    Ok(file) => file,
                    Err(err) => {
                        if !err.matches(gtk4::DialogError::Dismissed) {
                            eprintln!("ScreenForge: save dialog failed: {err}");
                        }
                        return;
                    }
                };
                let Some(path) = file.path() else { return };

                let export_button = window.export_button();
                let toast_overlay = window.toast_overlay();
                export_button.set_sensitive(false);

                let doc = state.borrow().document.clone();
                let (decoded_images, background_image) = {
                    let mut state_ref = state.borrow_mut();
                    let EditorState { document, image_cache, .. } = &mut *state_ref;
                    let decoded_images = document
                        .elements
                        .iter()
                        .filter_map(|el| {
                            let ImageSource::Path(path) = &el.source else { return None };
                            get_or_decode(image_cache, path).map(|image| (el.id, image.clone()))
                        })
                        .collect::<HashMap<_, _>>();
                    let background_image = background_image_path(&document.background)
                        .and_then(|path| get_or_decode(image_cache, &path))
                        .cloned();
                    (decoded_images, background_image)
                };
                let result =
                    gio::spawn_blocking(move || export::render_and_write(&doc, &decoded_images, background_image.as_ref(), &path))
                        .await;

                export_button.set_sensitive(true);
                let toast = match result {
                    Ok(Ok(())) => adw::Toast::new("Export erfolgreich"),
                    Ok(Err(err)) => adw::Toast::new(&format!("Export fehlgeschlagen: {err}")),
                    Err(_) => adw::Toast::new("Export fehlgeschlagen: Hintergrundaufgabe abgebrochen"),
                };
                toast_overlay.add_toast(toast);
            });
        }
    ));
    window.add_action(&export_action);
    app.set_accels_for_action("win.export", &["<Ctrl>e"]);
}

/// Where a zip-format project's embedded images get extracted to before
/// `screenforge_core::project::load` hands back a `Document` — a single
/// fixed directory (sibling of `import::save_pasted_image`'s own cache
/// dir), cleared and recreated here at the start of every load rather than
/// given a fresh unique name each time, so at most one project's worth of
/// extracted assets ever sits on disk regardless of how many projects get
/// opened over a session. Re-saving a zip-loaded project needs no special
/// handling as a result: its elements are ordinary `Path`s into this
/// directory by the time `save` sees them, read exactly like any other
/// imported file.
fn prepare_project_asset_extract_dir() -> PathBuf {
    let dir = glib::user_cache_dir().join("screenforge").join("project-assets");
    std::fs::remove_dir_all(&dir).ok();
    dir
}

fn save_project_to(window: &Window, state: &Rc<RefCell<EditorState>>, path: &std::path::Path) {
    let doc = state.borrow().document.clone();
    let toast = match screenforge_core::project::save(&doc, path) {
        Ok(()) => adw::Toast::new("Projekt gespeichert"),
        Err(err) => adw::Toast::new(&format!("Speichern fehlgeschlagen: {err}")),
    };
    window.toast_overlay().add_toast(toast);
}

async fn save_project_as(window: &Window, state: &Rc<RefCell<EditorState>>) {
    let filter = gtk4::FileFilter::new();
    filter.add_pattern("*.screenforge");
    filter.set_name(Some("ScreenForge-Projekte"));

    let dialog = gtk4::FileDialog::builder()
        .title("Projekt speichern unter")
        .accept_label("Speichern")
        .initial_name("komposition.screenforge")
        .default_filter(&filter)
        .build();

    let file = match dialog.save_future(Some(window)).await {
        Ok(file) => file,
        Err(err) => {
            if !err.matches(gtk4::DialogError::Dismissed) {
                eprintln!("ScreenForge: save-as dialog failed: {err}");
            }
            return;
        }
    };
    let Some(path) = file.path() else { return };

    save_project_to(window, state, &path);
    state.borrow_mut().project_path = Some(path);
}

/// Re-reads every sidebar control from `state.document` — used after loading
/// a project so the sidebar reflects what was actually loaded rather than
/// whatever the user had set before. The shadow/corner-radius rows re-apply
/// their value to every element on change (§9), which is a harmless no-op
/// here only because ScreenForge itself never saves a document with
/// per-element shadow/radius variation; that assumption would need
/// revisiting if per-element controls are added later.
fn sync_controls_from_document(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    state.borrow_mut().syncing_controls = true;

    let doc = state.borrow().document.clone();

    window.layout_mode_row().set_selected(index_for_layout_mode(doc.layout.mode));
    window.spacing_row().set_value(doc.layout.spacing_px);
    window.margin_x_row().set_value(doc.layout.margin_x);
    window.margin_y_row().set_value(doc.layout.margin_y);
    sync_alignment_group_visibility(window, doc.layout.mode);

    sync_background_controls(window, &doc.background);

    if let Some(first) = doc.elements.first() {
        window.shadow_row().set_selected(shadow_preset_index_for(&first.shadow));
        let (angle, distance) = first.shadow.angle_and_distance();
        window.shadow_angle_row().set_value(angle);
        window.shadow_distance_row().set_value(distance);
        window.shadow_blur_row().set_value(first.shadow.blur);
        window.shadow_angle_row().set_sensitive(first.shadow.enabled);
        window.shadow_distance_row().set_sensitive(first.shadow.enabled);
        window.shadow_blur_row().set_sensitive(first.shadow.enabled);
        window.corner_radius_row().set_value(first.corner_radius.top_left);
    }

    window.export_width_row().set_value(doc.canvas.export_target_width as f64);
    update_export_height_display(window, doc.canvas);
    window.export_format_row().set_selected(index_for_export_format(doc.canvas.export_format));
    window.export_quality_row().set_value(doc.canvas.export_quality as f64);
    window.export_quality_row().set_sensitive(format_supports_quality(doc.canvas.export_format));

    if let Some(sync) = state.borrow().label_style_sync.clone() {
        sync(&doc.label_defaults);
    }
    sync_label_controls(window, canvas, state);
    sync_callouts_controls(window, canvas, state);

    state.borrow_mut().syncing_controls = false;
}

/// `win.save`, `win.save-as` and `win.open-project` — the `.screenforge`
/// project file, distinct from `win.open`'s image import (spec §16/§18).
/// Missing source images on load are reported per-element via a toast, not
/// as a load failure (spec §4: local editing must keep working offline even
/// when referenced files have moved or been deleted).
fn register_project_actions(app: &adw::Application, window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let save_action = gio::SimpleAction::new("save", None);
    save_action.connect_activate(glib::clone!(
        #[weak]
        window,
        #[strong]
        state,
        move |_, _| {
            let window = window.clone();
            let state = state.clone();
            glib::spawn_future_local(async move {
                let existing = state.borrow().project_path.clone();
                match existing {
                    Some(path) => save_project_to(&window, &state, &path),
                    None => save_project_as(&window, &state).await,
                }
            });
        }
    ));
    window.add_action(&save_action);
    app.set_accels_for_action("win.save", &["<Ctrl>s"]);

    let save_as_action = gio::SimpleAction::new("save-as", None);
    save_as_action.connect_activate(glib::clone!(
        #[weak]
        window,
        #[strong]
        state,
        move |_, _| {
            let window = window.clone();
            let state = state.clone();
            glib::spawn_future_local(async move {
                save_project_as(&window, &state).await;
            });
        }
    ));
    window.add_action(&save_as_action);
    app.set_accels_for_action("win.save-as", &["<Ctrl><Shift>s"]);

    let open_project_action = gio::SimpleAction::new("open-project", None);
    open_project_action.connect_activate(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |_, _| {
            let window = window.clone();
            let canvas = canvas.clone();
            let state = state.clone();
            glib::spawn_future_local(async move {
                let filter = gtk4::FileFilter::new();
                filter.add_pattern("*.screenforge");
                filter.set_name(Some("ScreenForge-Projekte"));

                let dialog = gtk4::FileDialog::builder()
                    .title("Projekt öffnen")
                    .accept_label("Öffnen")
                    .default_filter(&filter)
                    .build();

                let file = match dialog.open_future(Some(&window)).await {
                    Ok(file) => file,
                    Err(err) => {
                        if !err.matches(gtk4::DialogError::Dismissed) {
                            eprintln!("ScreenForge: open-project dialog failed: {err}");
                        }
                        return;
                    }
                };
                let Some(path) = file.path() else { return };

                match screenforge_core::project::load(&path, &prepare_project_asset_extract_dir()) {
                    Ok(doc) => {
                        let mut image_cache = HashMap::new();
                        let mut missing = 0u32;
                        for element in &doc.elements {
                            let ImageSource::Path(source_path) = &element.source else { continue };
                            if get_or_decode(&mut image_cache, source_path).is_none() {
                                missing += 1;
                            }
                        }

                        {
                            let mut state_ref = state.borrow_mut();
                            state_ref.document = doc;
                            state_ref.image_cache = image_cache;
                            state_ref.project_path = Some(path);
                            // A freshly loaded project starts with a clean
                            // undo history — undoing past "load" into the
                            // previous document would be surprising.
                            state_ref.undo_stack = UndoStack::new();
                        }
                        refresh_canvas(&window, &canvas, &state);
                        sync_controls_from_document(&window, &canvas, &state);
                        update_undo_redo_sensitivity(&window, &state);

                        let toast = if missing > 0 {
                            adw::Toast::new(&format!("Projekt geladen ({missing} Bild(er) fehlen)"))
                        } else {
                            adw::Toast::new("Projekt geladen")
                        };
                        window.toast_overlay().add_toast(toast);
                    }
                    Err(err) => {
                        window.toast_overlay().add_toast(adw::Toast::new(&format!("Projekt konnte nicht geladen werden: {err}")));
                    }
                }
            });
        }
    ));
    window.add_action(&open_project_action);
}

/// Applies `preset` as one undoable step (spec: "Beim Anwenden eines
/// Presets müssen die darin gespeicherten Einstellungen vollständig
/// übernommen werden... Falls das Preset globale Label-Einstellungen
/// enthält, sollen diese anschließend als globale Standards verwendet
/// werden"). Called from the preset list's own row-activation handler in
/// `build_presets_page`.
fn apply_preset(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>, new: screenforge_core::template::Template) {
    {
        let mut state_ref = state.borrow_mut();
        let old_layout = state_ref.document.layout;
        let old_background = state_ref.document.background.clone();
        let old_shadows: Vec<ShadowParams> = state_ref.document.elements.iter().map(|e| e.shadow).collect();
        let old_corner_radii: Vec<CornerRadius> = state_ref.document.elements.iter().map(|e| e.corner_radius).collect();
        let old_label_defaults = state_ref.document.label_defaults.clone();
        let EditorState { document, undo_stack, .. } = &mut *state_ref;
        undo_stack.apply(
            Box::new(ApplyTemplate { old_layout, old_background, old_shadows, old_corner_radii, old_label_defaults, new }),
            document,
        );
    }
    refresh_canvas(window, canvas, state);
    sync_controls_from_document(window, canvas, state);
    update_undo_redo_sensitivity(window, state);
    window.toast_overlay().add_toast(adw::Toast::new("Preset angewendet"));
}

/// Reads every saved preset from `GSettings` (spec: "Verwende dafür den
/// vorgesehenen persistenten Konfigurationsmechanismus des verwendeten
/// Frameworks" — no external files, no user-managed template directory).
/// An empty key (never saved anything yet) is treated as an empty list
/// rather than an error; a *non-empty but unreadable* value (corrupted, or
/// from an incompatible future version) is logged and also treated as
/// empty rather than losing the rest of the app to a panic — the user
/// loses their saved presets in that case, but keeps everything else.
fn load_presets() -> Vec<screenforge_core::template::NamedPreset> {
    let json = app_settings().string("presets");
    if json.is_empty() {
        return Vec::new();
    }
    match screenforge_core::template::deserialize_presets(&json) {
        Ok(presets) => presets,
        Err(err) => {
            eprintln!("ScreenForge: could not read saved presets: {err}");
            Vec::new()
        }
    }
}

fn save_presets(presets: &[screenforge_core::template::NamedPreset]) {
    let json = screenforge_core::template::serialize_presets(presets);
    if let Err(err) = app_settings().set_string("presets", &json) {
        eprintln!("ScreenForge: could not persist presets: {err}");
    }
}

/// Reads the app-wide default label style from `GSettings` — the top tier
/// of the three-level label configuration (see `EditorState::new`, the
/// only caller that matters for *new* documents; the "Allgemein" settings
/// page also reads/writes this directly to edit it). An empty or
/// unreadable value (never set yet, or from an incompatible future
/// version) falls back to `LabelStyle::default()` rather than erroring —
/// there's no "nothing configured yet" state worth distinguishing from
/// "using the built-in default" here, unlike presets (an empty preset
/// list and "no presets configured" are the same thing either way).
fn load_global_label_defaults() -> screenforge_core::model::LabelStyle {
    let json = app_settings().string("default-label-style");
    if json.is_empty() {
        return screenforge_core::model::LabelStyle::default();
    }
    match screenforge_core::template::deserialize_label_style(&json) {
        Ok(style) => style,
        Err(err) => {
            eprintln!("ScreenForge: could not read the default label style, falling back to the built-in one: {err}");
            screenforge_core::model::LabelStyle::default()
        }
    }
}

fn save_global_label_defaults(style: &screenforge_core::model::LabelStyle) {
    let json = screenforge_core::template::serialize_label_style(style);
    if let Err(err) = app_settings().set_string("default-label-style", &json) {
        eprintln!("ScreenForge: could not persist the default label style: {err}");
    }
}

/// A small named-text prompt (used for both "save as" and "rename") built
/// from an `AdwAlertDialog` with a single `GtkEntry` as its extra child —
/// `None` for Escape/Abbrechen or an empty name, `Some(name)` (trimmed)
/// otherwise.
async fn prompt_for_preset_name(window: &Window, heading: &str, initial: &str) -> Option<String> {
    let entry = gtk4::Entry::new();
    entry.set_text(initial);
    entry.set_activates_default(true);

    let dialog = adw::AlertDialog::builder().heading(heading).extra_child(&entry).build();
    dialog.add_response("cancel", "Abbrechen");
    dialog.add_response("save", "Speichern");
    dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("save"));
    dialog.set_close_response("cancel");

    let response = dialog.choose_future(Some(window)).await;
    if response != "save" {
        return None;
    }
    let name = entry.text().trim().to_string();
    if name.is_empty() { None } else { Some(name) }
}

/// (Re)builds `list_box`'s rows from persisted storage — called once
/// whenever the presets popover is about to show, and again after every
/// save/rename/delete, so it always reflects what's actually saved
/// without needing to be closed and reopened. Each row's "Anwenden"/
/// rename/delete handler captures its preset's *index* into the
/// freshly-loaded list at click time (not the list this function built
/// the row from) — safe because every mutation immediately calls this
/// function again before the user can interact with anything else (the
/// save/rename prompt is itself a modal dialog, and delete's own handler
/// is synchronous), so no click can ever land against a stale index.
/// `popover` is popped down after a row's "Anwenden" activates a preset —
/// applying one reads as a complete action, the same way picking an entry
/// from any other GNOME popover menu dismisses it — but never after a
/// rename/delete, which are edits to the list the user stays in.
fn rebuild_preset_list(list_box: &gtk4::ListBox, window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>, popover: &gtk4::Popover) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }
    let presets = load_presets();
    if presets.is_empty() {
        let row = adw::ActionRow::builder().title("Noch keine Presets gespeichert").sensitive(false).build();
        list_box.append(&row);
        return;
    }
    for (index, named) in presets.iter().enumerate() {
        let row = adw::ActionRow::builder().title(named.name.clone()).activatable(true).build();
        row.set_subtitle("Anwenden antippen");

        let rename_button = gtk4::Button::from_icon_name("document-edit-symbolic");
        rename_button.set_valign(gtk4::Align::Center);
        rename_button.set_tooltip_text(Some("Umbenennen"));
        rename_button.add_css_class("flat");
        row.add_suffix(&rename_button);

        let delete_button = gtk4::Button::from_icon_name("user-trash-symbolic");
        delete_button.set_valign(gtk4::Align::Center);
        delete_button.set_tooltip_text(Some("Löschen"));
        delete_button.add_css_class("flat");
        row.add_suffix(&delete_button);

        row.connect_activated(glib::clone!(
            #[weak]
            window,
            #[weak]
            canvas,
            #[strong]
            state,
            #[weak]
            popover,
            move |_| {
                let presets = load_presets();
                if let Some(named) = presets.get(index) {
                    apply_preset(&window, &canvas, &state, named.template.clone());
                    popover.popdown();
                }
            }
        ));

        rename_button.connect_clicked(glib::clone!(
            #[weak]
            window,
            #[weak]
            canvas,
            #[strong]
            state,
            #[weak]
            list_box,
            #[weak]
            popover,
            move |_| {
                glib::spawn_future_local(glib::clone!(
                    #[weak]
                    window,
                    #[weak]
                    canvas,
                    #[strong]
                    state,
                    #[weak]
                    list_box,
                    #[weak]
                    popover,
                    async move {
                        let presets = load_presets();
                        let Some(current) = presets.get(index).cloned() else { return };
                        let Some(new_name) = prompt_for_preset_name(&window, "Preset umbenennen", &current.name).await else { return };
                        let mut presets = load_presets();
                        if let Some(named) = presets.get_mut(index) {
                            named.name = new_name;
                        }
                        save_presets(&presets);
                        rebuild_preset_list(&list_box, &window, &canvas, &state, &popover);
                    }
                ));
            }
        ));

        delete_button.connect_clicked(glib::clone!(
            #[weak]
            window,
            #[weak]
            canvas,
            #[strong]
            state,
            #[weak]
            list_box,
            #[weak]
            popover,
            move |_| {
                let mut presets = load_presets();
                if index < presets.len() {
                    presets.remove(index);
                    save_presets(&presets);
                }
                rebuild_preset_list(&list_box, &window, &canvas, &state, &popover);
            }
        ));

        list_box.append(&row);
    }
}

/// The header bar's "Presets" menu (`presets_menu_button`): an in-app,
/// persisted replacement for the old external "Vorlage speichern/laden"
/// file actions (spec: "Es soll stattdessen ein internes Preset-System...
/// geben" — no export/import of settings files, no user-managed template
/// directory) — and, since this round, deliberately its *own* header-bar
/// entry point rather than a page inside the "Einstellungen" dialog, so
/// saving/restoring a preset never gets mistaken for touching the app-wide
/// settings on that dialog's other pages (see `register_settings_action`'s
/// own doc comment for why that mattered). A preset bundles the *current
/// project's* layout, background, shadow, corner radius, and label
/// defaults (see `screenforge_core::template::Template::from_document`) —
/// never any screenshot's own label content or callouts; applying one
/// never removes or changes those (see
/// `apply_preset`/`ApplyTemplate`'s own doc comment).
///
/// The popover is rebuilt fresh (`rebuild_preset_list`) every time it's
/// about to show, via `GtkPopover`'s own `show` signal, so it always
/// reflects whatever's actually saved without this needing to track
/// changes itself.
fn register_presets_menu(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    // Built before anything that references it (every row's "Anwenden"
    // pops it down; the save button and the popover's own `show` handler
    // both need to pass it to `rebuild_preset_list`), then given its real
    // child content only once everything else exists.
    let popover = gtk4::Popover::new();

    let list_box = gtk4::ListBox::new();
    list_box.set_selection_mode(gtk4::SelectionMode::None);
    list_box.add_css_class("boxed-list");

    let save_button = gtk4::Button::with_label("Aktuelles als Preset speichern…");
    save_button.add_css_class("suggested-action");
    save_button.connect_clicked(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        #[weak]
        list_box,
        #[weak]
        popover,
        move |_| {
            glib::spawn_future_local(glib::clone!(
                #[weak]
                window,
                #[weak]
                canvas,
                #[strong]
                state,
                #[weak]
                list_box,
                #[weak]
                popover,
                async move {
                    let Some(name) = prompt_for_preset_name(&window, "Preset speichern", "").await else { return };
                    let template = screenforge_core::template::Template::from_document(&state.borrow().document);
                    let mut presets = load_presets();
                    presets.push(screenforge_core::template::NamedPreset { name, template });
                    save_presets(&presets);
                    rebuild_preset_list(&list_box, &window, &canvas, &state, &popover);
                }
            ));
        }
    ));

    let hint = gtk4::Label::new(Some("Layout, Hintergrund, Schatten, Eckenradius und Label-Einstellungen dieses Projekts"));
    hint.set_wrap(true);
    hint.set_xalign(0.0);
    hint.add_css_class("caption");
    hint.add_css_class("dim-label");

    let scroller = gtk4::ScrolledWindow::builder().hscrollbar_policy(gtk4::PolicyType::Never).child(&list_box).build();
    scroller.set_max_content_height(360);
    scroller.set_propagate_natural_height(true);

    let content = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(12);
    content.set_margin_end(12);
    content.set_width_request(320);
    content.append(&save_button);
    content.append(&hint);
    content.append(&scroller);
    popover.set_child(Some(&content));

    popover.connect_show(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        #[weak]
        list_box,
        move |popover| rebuild_preset_list(&list_box, &window, &canvas, &state, popover)
    ));

    window.presets_menu_button().set_popover(Some(&popover));
}

/// The "Allgemein" page: `GSettings`-backed defaults applied to every
/// *newly created* document (see `EditorState::new`) — changing one never
/// touches the document currently open, only what a fresh one starts
/// with. Each row binds straight to its `GSettings` key via
/// `Settings::bind`, so there's no manual load/save glue: GLib keeps the
/// setting and the widget in sync both ways for as long as the dialog is
/// open.
fn build_general_page() -> adw::PreferencesPage {
    let settings = app_settings();

    let spacing_row = adw::SpinRow::with_range(0.0, 500.0, 4.0);
    spacing_row.set_title("Abstand");
    spacing_row.set_subtitle("Zwischen den Screenshots, in Pixeln");
    settings.bind("default-spacing", &spacing_row, "value").build();

    let margin_x_row = adw::SpinRow::with_range(0.0, 500.0, 4.0);
    margin_x_row.set_title("Außenabstand horizontal");
    margin_x_row.set_subtitle("Links/rechts um die Komposition, in Pixeln");
    settings.bind("default-margin-x", &margin_x_row, "value").build();

    let margin_y_row = adw::SpinRow::with_range(0.0, 500.0, 4.0);
    margin_y_row.set_title("Außenabstand vertikal");
    margin_y_row.set_subtitle("Oben/unten um die Komposition, in Pixeln");
    settings.bind("default-margin-y", &margin_y_row, "value").build();

    let quality_row = adw::SpinRow::with_range(1.0, 100.0, 5.0);
    quality_row.set_title("Export-Qualität");
    quality_row.set_subtitle("Für JPEG/WebP/AVIF, in Prozent");
    settings.bind("default-export-quality", &quality_row, "value").build();

    let group = adw::PreferencesGroup::new();
    group.set_title("Standardwerte für neue Projekte");
    group.add(&spacing_row);
    group.add(&margin_x_row);
    group.add(&margin_y_row);
    group.add(&quality_row);

    let page = adw::PreferencesPage::new();
    page.set_title("Allgemein");
    page.set_icon_name(Some("preferences-system-symbolic"));
    page.add(&group);

    let (look_group, position_group, styling_group) = build_global_label_defaults_groups();
    page.add(&look_group);
    page.add(&position_group);
    page.add(&styling_group);

    page
}

/// `app.preferences` (`Ctrl+,`): the app's settings dialog — just
/// "Allgemein" (`build_general_page`), GSettings-backed defaults that seed
/// *new* documents (spacing/margin/export quality, and the app-wide
/// default label style). The current project's *own* label style is no
/// longer edited here — it lives directly in the sidebar
/// (`register_label_style_controls`), always reachable and always
/// reflecting the open project, so there's no separate dialog page for it
/// to fall out of sync with.
///
/// Presets deliberately live *outside* this dialog too, in their own
/// header-bar menu (`register_presets_menu`/`presets_menu_button`) rather
/// than as a page here: a preset captures the *current project's*
/// settings (spec: "als Preset möchte ich die AKTUELLEN Einstellungen
/// speichern, nicht die globalen"), and putting its save/restore/delete UI
/// in the very same dialog as the app-wide "Allgemein" settings blurred
/// that distinction — someone opening "Einstellungen" to save a preset
/// could easily read it as "save my global settings", which is exactly
/// backward. Rebuilt fresh every time it's opened.
fn register_settings_action(app: &adw::Application) {
    let action = gio::SimpleAction::new("preferences", None);
    action.connect_activate(glib::clone!(
        #[weak]
        app,
        move |_, _| {
            let dialog = adw::PreferencesDialog::new();
            dialog.set_title("Einstellungen");
            dialog.add(&build_general_page());
            dialog.present(app.active_window().as_ref());
        }
    ));
    app.add_action(&action);
    app.set_accels_for_action("app.preferences", &["<Ctrl>comma"]);
}

/// The most recent versions' changelog, most recent first — each entry
/// drawn straight from this repo's own version-bump commit messages (`git
/// log --oneline | grep '(v'`), never invented. Handed to
/// `AdwAboutDialog::set_release_notes`, whose accepted markup is the same
/// restricted subset AppStream release-notes use: `<p>`/`<ul>`/`<li>` only.
const RELEASE_NOTES: &str = "\
<p>Version 0.26.0</p>
<ul>
<li>Sechs neue Stile für generierte Hintergründe: Schichten, Bögen, Bänder, Flächen, Linien und Nebel — mit weichen Kurven, Schatten und Farbverläufen</li>
<li>Neue Einstellung „Stimmung“ (kräftig, hell, dunkel) für harmonischere Farben</li>
<li>Neue Einstellung „Körnung“ gegen Farbstufen</li>
<li>Der bisherige Generator bleibt als „Wellen (klassisch)“ erhalten, alte Projekte sehen unverändert aus</li>
</ul>
<p>Version 0.23.0</p>
<ul>
<li>Neuer Info-Dialog („Info zu ScreenForge…“) mit Danksagung, Lizenzen und Changelog</li>
<li>Neues App-Icon im Schmiede-Motiv, inklusive symbolischer Variante</li>
<li>Hintergrund lässt sich jetzt direkt mit der Maus verschieben (Alt+Ziehen) — spürbar flüssiger, da nicht mehr bei jeder Mausbewegung neu berechnet</li>
<li>Labels unterstützen jetzt mehrzeiligen Text mit eigenem Umbruch-Schalter</li>
<li>Labels und Callouts dürfen jetzt über den Rand ihres Screenshots hinausragen — die Leinwand passt sich automatisch an, statt sie abzuschneiden</li>
</ul>
<p>Version 0.22.0</p>
<ul>
<li>Anwendungsweiten Titel durch Labels pro Screenshot ersetzt</li>
<li>Callouts hinzugefügt: Sprechblasen mit Pfeil auf einen Punkt im Screenshot</li>
</ul>
<p>Version 0.21.0</p>
<ul>
<li>Rendering des Hintergrund-Generators überarbeitet: harte Kanten und geschichtete Kontaktschatten</li>
</ul>
<p>Version 0.20.0</p>
<ul>
<li>Screenshots lassen sich jetzt direkt von einem verbundenen Android-Gerät importieren</li>
</ul>
<p>Version 0.19.0</p>
<ul>
<li>Neues Ausrichtungswerkzeug für Screenshots</li>
<li>Pipette zum Aufnehmen von Farben direkt von der Leinwand</li>
</ul>
<p>Version 0.18.0</p>
<ul>
<li>Vektor-Musterhintergründe durch einen einheitlichen Generator ersetzt</li>
<li>Schatten-Caching für spürbar bessere Performance beim Bearbeiten</li>
</ul>
<p>Version 0.17.0</p>
<ul>
<li>Freiform-Vektorformen für das benutzerdefinierte Dekorationsmuster hinzugefügt</li>
</ul>";

/// The "Info zu ScreenForge…" dialog (spec: application info, dedication/
/// acknowledgements for the toolchain and libraries this is built on,
/// their licenses, and a changelog — all in one `AdwAboutDialog`, GNOME's
/// standard shape for exactly this). Every credited project and every
/// license below is one this app (or one of its *direct* Cargo
/// dependencies) actually uses — checked against `Cargo.toml` and each
/// dependency's own published license via `cargo metadata`, not assumed —
/// and grouped by what they're *for* rather than dumped as a flat list, so
/// it reads as a real "built with" page instead of a dependency dump.
/// `application_icon` resolves via `register_app_icon_theme`, called once
/// before any window (including this dialog) can exist.
fn register_about_action(app: &adw::Application) {
    let action = gio::SimpleAction::new("about", None);
    action.connect_activate(glib::clone!(
        #[weak]
        app,
        move |_, _| {
            let dialog = adw::AboutDialog::builder()
                .application_name("ScreenForge")
                .application_icon(APP_ID)
                .developer_name("Christoph Langner")
                .version(env!("CARGO_PKG_VERSION"))
                .comments("Ordnet Smartphone-Screenshots zu einer einzigen Präsentationsgrafik an — eine native GNOME-App.")
                .website("https://github.com/linuxundich/ScreenForge")
                .issue_url("https://github.com/linuxundich/ScreenForge/issues")
                .copyright("© 2025–2026 Christoph Langner")
                .license_type(gtk4::License::Gpl30)
                .release_notes(RELEASE_NOTES)
                .release_notes_version(env!("CARGO_PKG_VERSION"))
                .build();

            // "Built with" — who/what, not the legal details (those are
            // their own section below); `add_credit_section`'s row format
            // is "Name https://url", the same one GNOME's own about
            // dialogs use to make a name double as a link.
            dialog.add_credit_section(Some("Sprache"), &["Rust https://www.rust-lang.org"]);
            dialog.add_credit_section(
                Some("GUI-Framework"),
                &[
                    "GTK https://www.gtk.org",
                    "libadwaita https://gnome.pages.gitlab.gnome.org/libadwaita/",
                    "gtk4-rs / libadwaita-rs (Rust-Bindings) https://gtk-rs.org",
                ],
            );
            dialog.add_credit_section(Some("Grafik &amp; Rendering"), &["Cairo https://www.cairographics.org", "Pango https://pango.gnome.org"]);
            dialog.add_credit_section(
                Some("Weitere Bibliotheken"),
                &[
                    "image-rs https://github.com/image-rs/image",
                    "serde / serde_json https://serde.rs",
                    "uuid https://github.com/uuid-rs/uuid",
                    "thiserror / anyhow https://github.com/dtolnay",
                ],
            );

            // Licenses — one section per distinct license actually in use
            // (verified via `cargo metadata`), not one per crate: the
            // GNOME platform libraries GTK/libadwaita/GLib/Pango/Cairo
            // ship under LGPL-2.1-or-later; the Rust *bindings* to them
            // (gtk4-rs/libadwaita-rs) are a separate MIT-licensed project;
            // the remaining direct Rust dependencies are dual-licensed,
            // which `gtk4::License` has no single variant for, so that one
            // uses `Custom` with the real, unabridged statement instead of
            // picking just one half of it.
            dialog.add_legal_section("GTK, libadwaita, GLib, Pango, Cairo", None, gtk4::License::Lgpl21, None);
            dialog.add_legal_section("gtk4-rs, libadwaita-rs (Rust-Bindings)", None, gtk4::License::MitX11, None);
            dialog.add_legal_section(
                "serde, serde_json, thiserror, anyhow, uuid, image-rs, Rust",
                None,
                gtk4::License::Custom,
                Some("Dual-lizenziert unter MIT oder Apache-2.0, nach Wahl der Rechteinhaberin oder des Rechteinhabers."),
            );

            dialog.present(app.active_window().as_ref());
        }
    ));
    app.add_action(&action);
}

/// Reflects `undo_stack.can_undo()/can_redo()` onto the `win.undo`/`win.redo`
/// `GSimpleAction`s. The header-bar buttons are bound to these actions via
/// `action-name` in the `.ui` file, so disabling the action alone is enough
/// to grey out the button — no separate widget bookkeeping needed.
fn update_undo_redo_sensitivity(window: &Window, state: &Rc<RefCell<EditorState>>) {
    let state_ref = state.borrow();
    if let Some(action) = window.lookup_action("undo").and_downcast::<gio::SimpleAction>() {
        action.set_enabled(state_ref.undo_stack.can_undo());
    }
    if let Some(action) = window.lookup_action("redo").and_downcast::<gio::SimpleAction>() {
        action.set_enabled(state_ref.undo_stack.can_redo());
    }
}

/// `win.undo`/`win.redo` (spec §17/§18: Ctrl+Z / Ctrl+Shift+Z). Both actions
/// start disabled (empty history) and are re-enabled/disabled by
/// [`update_undo_redo_sensitivity`] after every undoable mutation.
fn register_undo_redo_actions(app: &adw::Application, window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let undo_action = gio::SimpleAction::new("undo", None);
    undo_action.set_enabled(false);
    undo_action.connect_activate(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |_, _| {
            {
                let mut state_ref = state.borrow_mut();
                let EditorState { document, undo_stack, .. } = &mut *state_ref;
                undo_stack.undo(document);
            }
            refresh_canvas(&window, &canvas, &state);
            sync_controls_from_document(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));
    window.add_action(&undo_action);
    app.set_accels_for_action("win.undo", &["<Ctrl>z"]);

    let redo_action = gio::SimpleAction::new("redo", None);
    redo_action.set_enabled(false);
    redo_action.connect_activate(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |_, _| {
            {
                let mut state_ref = state.borrow_mut();
                let EditorState { document, undo_stack, .. } = &mut *state_ref;
                undo_stack.redo(document);
            }
            refresh_canvas(&window, &canvas, &state);
            sync_controls_from_document(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));
    window.add_action(&redo_action);
    app.set_accels_for_action("win.redo", &["<Ctrl><Shift>z"]);
}

const ZOOM_STEP: f64 = 1.25;
const ZOOM_MIN: f64 = 0.1;
const ZOOM_MAX: f64 = 8.0;

/// `win.zoom-fit`/`win.zoom-100`/`win.zoom-in`/`win.zoom-out` (spec §10).
/// Purely a canvas-widget display setting — not part of the document, so
/// not undoable and not persisted in the project file.
fn register_zoom_actions(app: &adw::Application, window: &Window, canvas: &Canvas) {
    let zoom_fit = gio::SimpleAction::new("zoom-fit", None);
    zoom_fit.connect_activate(glib::clone!(
        #[weak]
        canvas,
        move |_, _| canvas.set_zoom(None)
    ));
    window.add_action(&zoom_fit);
    app.set_accels_for_action("win.zoom-fit", &["<Ctrl>0"]);

    let zoom_100 = gio::SimpleAction::new("zoom-100", None);
    zoom_100.connect_activate(glib::clone!(
        #[weak]
        canvas,
        move |_, _| canvas.set_zoom(Some(1.0))
    ));
    window.add_action(&zoom_100);
    app.set_accels_for_action("win.zoom-100", &["<Ctrl>1"]);

    let zoom_in = gio::SimpleAction::new("zoom-in", None);
    zoom_in.connect_activate(glib::clone!(
        #[weak]
        canvas,
        move |_, _| {
            let current = canvas.zoom().unwrap_or(1.0);
            canvas.set_zoom(Some((current * ZOOM_STEP).min(ZOOM_MAX)));
        }
    ));
    window.add_action(&zoom_in);
    app.set_accels_for_action("win.zoom-in", &["<Ctrl>plus", "<Ctrl>equal"]);

    let zoom_out = gio::SimpleAction::new("zoom-out", None);
    zoom_out.connect_activate(glib::clone!(
        #[weak]
        canvas,
        move |_, _| {
            let current = canvas.zoom().unwrap_or(1.0);
            canvas.set_zoom(Some((current / ZOOM_STEP).max(ZOOM_MIN)));
        }
    ));
    window.add_action(&zoom_out);
    app.set_accels_for_action("win.zoom-out", &["<Ctrl>minus"]);
}

/// Wires the canvas's press-drag-release reorder gesture to an undoable
/// [`ReorderScreenshot`] (spec §2: "Verschieben per Drag & Drop").
fn register_reorder(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    canvas.connect_reorder(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |from, to| {
            let mut state_ref = state.borrow_mut();
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(ReorderScreenshot { from, to }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));
}

/// `win.delete-selected` (`Delete`/`BackSpace`): removes every currently
/// selected screenshot as one undo step (spec §5: multi-select delete).
/// Independent of the context menu's single-target `win.delete-screenshot`
/// — right-clicking a screenshot and choosing "Löschen" always acts on
/// just that one, regardless of the current selection.
fn register_delete_selected(app: &adw::Application, window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let action = gio::SimpleAction::new("delete-selected", None);
    action.connect_activate(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |_, _| {
            let selected = canvas.selected_ids();
            if selected.is_empty() {
                return;
            }
            let mut state_ref = state.borrow_mut();
            let removed: Vec<(usize, ScreenshotElement)> = state_ref
                .document
                .elements
                .iter()
                .enumerate()
                .filter(|(_, e)| selected.contains(&e.id))
                .map(|(index, e)| (index, e.clone()))
                .collect();
            if removed.is_empty() {
                return;
            }
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(RemoveScreenshots { removed }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));
    window.add_action(&action);
    app.set_accels_for_action("win.delete-selected", &["Delete", "BackSpace"]);
}

/// Disables `delete-selected`/`undo`/`redo`/`paste` whenever a text-input
/// widget has keyboard focus, re-enabling them the instant it doesn't.
///
/// These four are registered as global accelerators (`Delete`/`BackSpace`,
/// `<Ctrl>z`, `<Ctrl><Shift>z`, `<Ctrl>v`) via `app.set_accels_for_action`,
/// which GTK4 dispatches through a capture-phase shortcut controller on the
/// window — that wins the race against the focused widget's own key
/// handling. A no-op check *inside* an action's own activate callback
/// isn't enough to fix this: GTK already marks the key event as handled
/// once a matching, *enabled* action fires, regardless of what the
/// callback body does, so the keystroke would never reach the focused
/// entry either way. Disabling the `GSimpleAction` itself is what makes
/// GTK skip it and let the event fall through normally — e.g. Backspace
/// then deletes a character in the focused field instead of deleting the
/// selected screenshot (the bug this fixes), and Ctrl+Z reaches the
/// field's own built-in undo instead of the document's.
///
/// `gtk4::Text` is the internal widget every `AdwEntryRow`/`GtkEntry`/
/// `GtkSpinButton` entry focuses; `gtk4::TextView` covers the multi-line
/// label/callout text fields.
fn register_text_focus_guards(window: &Window) {
    let update = glib::clone!(
        #[weak]
        window,
        move || {
            let text_focused = gtk4::prelude::RootExt::focus(&window).is_some_and(|w| w.is::<gtk4::Text>() || w.is::<gtk4::TextView>());
            for name in ["delete-selected", "undo", "redo", "paste"] {
                if let Some(action) = window.lookup_action(name) {
                    if let Some(action) = action.downcast_ref::<gio::SimpleAction>() {
                        action.set_enabled(!text_focused);
                    }
                }
            }
        }
    );
    window.connect_notify_local(
        Some("focus-widget"),
        glib::clone!(
            #[strong]
            update,
            move |_, _| update()
        ),
    );
    update();
}

/// One of the six ways `align_selected` can line up the current
/// multi-selection's `Transform`s against each other.
#[derive(Clone, Copy)]
enum Alignment {
    Left,
    CenterHorizontal,
    Right,
    Top,
    CenterVertical,
    Bottom,
}

/// Aligns every currently selected screenshot's `Transform.x`/`.y` against
/// the selection's own bounding box, as one undoable [`SetTransforms`] —
/// e.g. `Alignment::Left` moves every selected element's left edge to the
/// leftmost selected element's left edge, `Alignment::CenterHorizontal`
/// centers each one within the selection's horizontal span. Only
/// meaningful in `LayoutMode::Free` (the alignment buttons are hidden
/// otherwise, see `sync_alignment_group_visibility`) and with at least two
/// elements selected — selection lives entirely in the `Canvas` widget
/// (`canvas.selected_ids()`), so unlike the layout mode this can go stale
/// between clicks without any signal telling this function about it,
/// hence the toast rather than a disabled button for the "too few
/// selected" case.
fn align_selected(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>, alignment: Alignment) {
    let selected = canvas.selected_ids();
    if selected.len() < 2 {
        window.toast_overlay().add_toast(adw::Toast::new("Mindestens 2 Screenshots auswählen, um sie auszurichten"));
        return;
    }
    let mut state_ref = state.borrow_mut();

    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for element in state_ref.document.elements.iter().filter(|e| selected.contains(&e.id)) {
        let t = element.transform;
        min_x = min_x.min(t.x);
        min_y = min_y.min(t.y);
        max_x = max_x.max(t.x + t.width);
        max_y = max_y.max(t.y + t.height);
    }

    let transforms: Vec<(Uuid, screenforge_core::model::Transform, screenforge_core::model::Transform)> = state_ref
        .document
        .elements
        .iter()
        .filter(|e| selected.contains(&e.id))
        .filter_map(|element| {
            let old = element.transform;
            let mut new = old;
            match alignment {
                Alignment::Left => new.x = min_x,
                Alignment::CenterHorizontal => new.x = min_x + ((max_x - min_x) - old.width) / 2.0,
                Alignment::Right => new.x = max_x - old.width,
                Alignment::Top => new.y = min_y,
                Alignment::CenterVertical => new.y = min_y + ((max_y - min_y) - old.height) / 2.0,
                Alignment::Bottom => new.y = max_y - old.height,
            }
            (old != new).then_some((element.id, old, new))
        })
        .collect();
    if transforms.is_empty() {
        return;
    }
    let EditorState { document, undo_stack, .. } = &mut *state_ref;
    undo_stack.apply(Box::new(SetTransforms { transforms }), document);
    drop(state_ref);
    refresh_canvas(window, canvas, state);
    update_undo_redo_sensitivity(window, state);
}

/// Wires the sidebar's six alignment buttons (spec: "Ausrichtungswerkzeug")
/// to `align_selected`.
fn register_alignment_controls(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let buttons = [
        (window.align_left_button(), Alignment::Left),
        (window.align_center_h_button(), Alignment::CenterHorizontal),
        (window.align_right_button(), Alignment::Right),
        (window.align_top_button(), Alignment::Top),
        (window.align_center_v_button(), Alignment::CenterVertical),
        (window.align_bottom_button(), Alignment::Bottom),
    ];
    for (button, alignment) in buttons {
        button.connect_clicked(glib::clone!(
            #[weak]
            window,
            #[weak]
            canvas,
            #[strong]
            state,
            move |_| align_selected(&window, &canvas, &state, alignment)
        ));
    }
}

/// Wires the canvas's `LayoutMode::Free` move-drag to an undoable
/// [`SetTransforms`] (spec §8: manual positioning, extended by spec §5 to
/// move a multi-selection together as one undo step).
fn register_move(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    canvas.connect_move(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |new_positions: Vec<(Uuid, f64, f64)>| {
            let mut state_ref = state.borrow_mut();
            let transforms: Vec<(Uuid, screenforge_core::model::Transform, screenforge_core::model::Transform)> = new_positions
                .into_iter()
                .filter_map(|(id, new_x, new_y)| {
                    let element = state_ref.document.elements.iter().find(|e| e.id == id)?;
                    let old = element.transform;
                    let mut new = old;
                    new.x = new_x;
                    new.y = new_y;
                    Some((id, old, new))
                })
                .collect();
            if transforms.is_empty() {
                return;
            }
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetTransforms { transforms }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));
}

/// Wires the canvas's `LayoutMode::Free` corner-handle resize-drag to an
/// undoable [`SetTransform`] (spec §8: manual positioning).
fn register_resize(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    canvas.connect_resize(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |index, new| {
            let mut state_ref = state.borrow_mut();
            let Some(element) = state_ref.document.elements.get(index) else { return };
            let old = element.transform;
            let element_id = element.id;
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetTransform { element_id, old, new }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));
}

fn build_context_menu() -> gio::Menu {
    let menu = gio::Menu::new();

    let edit_section = gio::Menu::new();
    edit_section.append(Some("Duplizieren"), Some("win.duplicate-screenshot"));
    edit_section.append(Some("Screenshot ersetzen…"), Some("win.replace-screenshot"));
    edit_section.append(Some("Löschen"), Some("win.delete-screenshot"));
    menu.append_section(None, &edit_section);

    let order_section = gio::Menu::new();
    order_section.append(Some("Nach vorne"), Some("win.bring-forward"));
    order_section.append(Some("Nach hinten"), Some("win.send-backward"));
    order_section.append(Some("Ganz nach vorne"), Some("win.bring-to-front"));
    order_section.append(Some("Ganz nach hinten"), Some("win.send-to-back"));
    menu.append_section(None, &order_section);

    let transform_section = gio::Menu::new();
    transform_section.append(Some("Um 90° drehen"), Some("win.rotate-screenshot"));
    transform_section.append(Some("Horizontal spiegeln"), Some("win.flip-horizontal"));
    transform_section.append(Some("Vertikal spiegeln"), Some("win.flip-vertical"));
    menu.append_section(None, &transform_section);

    menu
}

/// Registers one `win.<name>` action that acts on whatever element the
/// context menu was last opened for. `build` computes the command from the
/// target's index and the current document, or returns `None` to silently
/// do nothing (e.g. "bring forward" on the first element already).
fn register_element_action<F>(
    window: &Window,
    canvas: &Canvas,
    state: &Rc<RefCell<EditorState>>,
    context_target: &Rc<Cell<Option<usize>>>,
    name: &str,
    build: F,
) where
    F: Fn(usize, &Document) -> Option<Box<dyn Command>> + 'static,
{
    let action = gio::SimpleAction::new(name, None);
    action.connect_activate(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        #[strong]
        context_target,
        move |_, _| {
            let Some(index) = context_target.get() else { return };
            let mut state_ref = state.borrow_mut();
            if index >= state_ref.document.elements.len() {
                return;
            }
            let Some(cmd) = build(index, &state_ref.document) else { return };
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(cmd, document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));
    window.add_action(&action);
}

/// `win.replace-screenshot`: swaps one element's source image, keeping its
/// position, effects and place in the sequence (spec §2/§21: "Screenshot
/// ersetzen"). Handled separately from [`register_element_action`] because
/// it needs an async file dialog and a decode step.
fn register_replace_action(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>, context_target: &Rc<Cell<Option<usize>>>) {
    let action = gio::SimpleAction::new("replace-screenshot", None);
    action.connect_activate(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        #[strong]
        context_target,
        move |_, _| {
            let Some(index) = context_target.get() else { return };
            let window = window.clone();
            let canvas = canvas.clone();
            let state = state.clone();
            glib::spawn_future_local(async move {
                let filter = gtk4::FileFilter::new();
                filter.add_mime_type("image/png");
                filter.add_mime_type("image/jpeg");
                filter.add_mime_type("image/webp");
                filter.set_name(Some("Screenshots"));

                let dialog = gtk4::FileDialog::builder()
                    .title("Screenshot ersetzen")
                    .accept_label("Ersetzen")
                    .default_filter(&filter)
                    .build();

                let file = match dialog.open_future(Some(&window)).await {
                    Ok(file) => file,
                    Err(err) => {
                        if !err.matches(gtk4::DialogError::Dismissed) {
                            eprintln!("ScreenForge: replace dialog failed: {err}");
                        }
                        return;
                    }
                };
                let Some(new_path) = file.path() else { return };

                let mut state_ref = state.borrow_mut();
                if index >= state_ref.document.elements.len() {
                    return;
                }
                let Some(image) = get_or_decode(&mut state_ref.image_cache, &new_path) else { return };
                let (new_w, new_h) = (image.width as f64, image.height as f64);

                let element = &state_ref.document.elements[index];
                let cmd = ReplaceScreenshotSource {
                    element_id: element.id,
                    old_source: element.source.clone(),
                    old_natural_width: element.natural_width,
                    old_natural_height: element.natural_height,
                    new_source: ImageSource::Path(new_path),
                    new_natural_width: new_w,
                    new_natural_height: new_h,
                };
                let EditorState { document, undo_stack, .. } = &mut *state_ref;
                undo_stack.apply(Box::new(cmd), document);
                drop(state_ref);
                refresh_canvas(&window, &canvas, &state);
                update_undo_redo_sensitivity(&window, &state);
            });
        }
    ));
    window.add_action(&action);
}

/// Right-click on a screenshot opens a `GtkPopoverMenu` with per-element
/// actions (spec §21). `context_target` remembers which element it was
/// opened for, since GAction activation carries no click-position payload.
fn register_context_menu(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let context_target: Rc<Cell<Option<usize>>> = Rc::new(Cell::new(None));

    let menu_model = build_context_menu();
    let popover = gtk4::PopoverMenu::from_model(Some(&menu_model));
    popover.set_has_arrow(false);
    popover.set_parent(canvas);

    // A popped-up popover's own native surface can otherwise still be
    // attached when the window tears down, which trips GTK's "finalizing
    // widget but it still has children left" diagnostic on quit.
    window.connect_destroy(glib::clone!(
        #[weak]
        popover,
        move |_| popover.unparent()
    ));

    canvas.connect_context_menu(glib::clone!(
        #[strong]
        context_target,
        #[weak]
        popover,
        move |index, x, y| {
            context_target.set(Some(index));
            popover.set_pointing_to(Some(&gdk::Rectangle::new(x.round() as i32, y.round() as i32, 1, 1)));
            popover.popup();
        }
    ));

    register_element_action(window, canvas, state, &context_target, "delete-screenshot", |index, doc| {
        Some(Box::new(RemoveScreenshot { index, element: doc.elements[index].clone() }))
    });

    register_element_action(window, canvas, state, &context_target, "duplicate-screenshot", |index, doc| {
        let mut duplicate = doc.elements[index].clone();
        duplicate.id = Uuid::new_v4();
        Some(Box::new(DuplicateScreenshot { source_index: index, duplicate }))
    });

    register_element_action(window, canvas, state, &context_target, "bring-forward", |index, _doc| {
        (index > 0).then(|| Box::new(ReorderScreenshot { from: index, to: index - 1 }) as Box<dyn Command>)
    });

    register_element_action(window, canvas, state, &context_target, "send-backward", |index, doc| {
        (index + 1 < doc.elements.len()).then(|| Box::new(ReorderScreenshot { from: index, to: index + 1 }) as Box<dyn Command>)
    });

    register_element_action(window, canvas, state, &context_target, "bring-to-front", |index, _doc| {
        (index > 0).then(|| Box::new(ReorderScreenshot { from: index, to: 0 }) as Box<dyn Command>)
    });

    register_element_action(window, canvas, state, &context_target, "send-to-back", |index, doc| {
        let last = doc.elements.len() - 1;
        (index != last).then(|| Box::new(ReorderScreenshot { from: index, to: last }) as Box<dyn Command>)
    });

    register_element_action(window, canvas, state, &context_target, "rotate-screenshot", |index, doc| {
        let el = &doc.elements[index];
        let old = el.transform;
        let mut new = old;
        new.rotation_deg = (old.rotation_deg + 90.0) % 360.0;
        Some(Box::new(SetTransform { element_id: el.id, old, new }))
    });

    register_element_action(window, canvas, state, &context_target, "flip-horizontal", |index, doc| {
        let el = &doc.elements[index];
        let old = el.transform;
        let mut new = old;
        new.flip_horizontal = !old.flip_horizontal;
        Some(Box::new(SetTransform { element_id: el.id, old, new }))
    });

    register_element_action(window, canvas, state, &context_target, "flip-vertical", |index, doc| {
        let el = &doc.elements[index];
        let old = el.transform;
        let mut new = old;
        new.flip_vertical = !old.flip_vertical;
        Some(Box::new(SetTransform { element_id: el.id, old, new }))
    });

    register_replace_action(window, canvas, state, &context_target);
}

/// `win.paste` (`Ctrl+V`, spec §1: "Screenshot aus der Zwischenablage
/// einfügen"). Silently does nothing if the clipboard holds no image —
/// pasting text or nothing is not an error condition here.
fn register_paste_action(app: &adw::Application, window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let action = gio::SimpleAction::new("paste", None);
    action.connect_activate(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |_, _| {
            let window = window.clone();
            let canvas = canvas.clone();
            let state = state.clone();
            glib::spawn_future_local(async move {
                let clipboard = window.clipboard();
                let texture = match clipboard.read_texture_future().await {
                    Ok(Some(texture)) => texture,
                    Ok(None) => return,
                    Err(err) => {
                        eprintln!("ScreenForge: clipboard read failed: {err}");
                        return;
                    }
                };

                let image = import::decoded_image_from_texture(&texture);
                let path = match import::save_pasted_image(&image) {
                    Ok(path) => path,
                    Err(err) => {
                        eprintln!("ScreenForge: could not save pasted image: {err}");
                        return;
                    }
                };

                let mut state_ref = state.borrow_mut();
                let element = ScreenshotElement::new(ImageSource::Path(path.clone()), image.width as f64, image.height as f64);
                state_ref.image_cache.insert(path, image);
                let EditorState { document, undo_stack, .. } = &mut *state_ref;
                undo_stack.apply(Box::new(AddScreenshots { elements: vec![element] }), document);
                drop(state_ref);
                refresh_canvas(&window, &canvas, &state);
                update_undo_redo_sensitivity(&window, &state);
            });
        }
    ));
    window.add_action(&action);
    app.set_accels_for_action("win.paste", &["<Ctrl>v"]);
}
