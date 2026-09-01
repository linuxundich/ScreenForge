mod canvas;
mod export;
mod import;
mod window;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use gtk4::gdk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use screenforge_core::command::{
    AddScreenshots, ReorderScreenshot, SetBackground, SetCornerRadiusForAllElements, SetMargin, SetShadowForAllElements, SetSpacing,
    UndoStack,
};
use screenforge_core::model::{
    Background, CornerRadius, Document, ExportFormat, GradientKind, GradientSpec, ImageSource, Rgba, ScreenshotElement, ShadowParams,
};
use uuid::Uuid;

use canvas::Canvas;
use import::DecodedImage;
use window::Window;

const APP_ID: &str = "de.christophlangner.ScreenForge";

/// Everything import/inspector/export actions mutate. Kept as one `Rc<RefCell<_>>`
/// shared between the window's actions and the canvas widget rather than
/// threaded through every callback individually.
struct EditorState {
    document: Document,
    decoded_images: HashMap<Uuid, DecodedImage>,
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
}

impl EditorState {
    fn new() -> Self {
        Self {
            document: Document::new(),
            decoded_images: HashMap::new(),
            project_path: None,
            undo_stack: UndoStack::new(),
            syncing_controls: false,
        }
    }
}

fn main() -> glib::ExitCode {
    gio::resources_register_include!("screenforge.gresource").expect("failed to register GResource bundle");

    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run()
}

fn build_ui(app: &adw::Application) {
    let window = Window::new(app);
    let canvas = window.canvas();

    let state = Rc::new(RefCell::new(EditorState::new()));
    refresh_canvas(&canvas, &state);

    register_open_action(app, &window, &canvas, &state);
    register_drop_target(&window, &canvas, &state);
    register_layout_controls(&window, &canvas, &state);
    register_effect_controls(&window, &canvas, &state);
    register_export_controls(&window, &state);
    register_export_action(app, &window, &state);
    register_project_actions(app, &window, &canvas, &state);
    register_undo_redo_actions(app, &window, &canvas, &state);
    register_zoom_actions(app, &window, &canvas);
    register_reorder(&window, &canvas, &state);

    window.present();
}

/// Materializes a fresh `cairo::ImageSurface` per decoded image (see
/// `import.rs` for why these aren't cached) and hands the result to the
/// canvas widget.
fn refresh_canvas(canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let state_ref = state.borrow();
    let surfaces: HashMap<Uuid, gtk4::cairo::ImageSurface> = state_ref
        .decoded_images
        .iter()
        .filter_map(|(id, image)| import::surface_from_decoded(image).ok().map(|s| (*id, s)))
        .collect();
    canvas.set_document(state_ref.document.clone(), surfaces);
}

/// Decodes every path and appends the successful ones to `state` as one
/// undoable [`AddScreenshots`] command, then refreshes the canvas once (not
/// per file). Used by both the file-open action and drag-and-drop, so the
/// two import paths can't drift apart.
fn import_paths(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>, paths: Vec<PathBuf>) {
    let mut new_elements = Vec::new();
    let mut decoded = Vec::new();
    for path in paths {
        match import::decode_image(&path) {
            Ok(image) => {
                let element = ScreenshotElement::new(ImageSource::Path(path), image.width as f64, image.height as f64);
                decoded.push((element.id, image));
                new_elements.push(element);
            }
            Err(err) => eprintln!("ScreenForge: failed to import {}: {err}", path.display()),
        }
    }
    if new_elements.is_empty() {
        return;
    }

    let mut state_ref = state.borrow_mut();
    state_ref.decoded_images.extend(decoded);
    let EditorState { document, undo_stack, .. } = &mut *state_ref;
    undo_stack.apply(Box::new(AddScreenshots { elements: new_elements }), document);
    drop(state_ref);

    refresh_canvas(canvas, state);
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

/// Wires the sidebar's spacing/margin rows to `Document.layout`, mutating it
/// directly through the undo stack (spec §17: spacing/margin changes are
/// undoable).
fn register_layout_controls(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let spacing_row = window.spacing_row();
    let margin_row = window.margin_row();

    {
        let state_ref = state.borrow();
        spacing_row.set_value(state_ref.document.layout.spacing_px);
        margin_row.set_value(state_ref.document.layout.margin_px);
    }

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
            refresh_canvas(&canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));
    margin_row.connect_value_notify(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |row| {
            let new = row.value();
            let mut state_ref = state.borrow_mut();
            let old = state_ref.document.layout.margin_px;
            if state_ref.syncing_controls || old == new {
                return;
            }
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetMargin { old, new }), document);
            drop(state_ref);
            refresh_canvas(&canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));
}

fn shadow_preset_for_index(index: u32) -> ShadowParams {
    match index {
        0 => ShadowParams::none(),
        1 => ShadowParams::subtle(),
        2 => ShadowParams::standard(),
        3 => ShadowParams::strong(),
        _ => ShadowParams::floating(),
    }
}

/// Reflects a `Background` value onto the type/color1/color2/angle controls
/// (used for both the initial sync and after undo/redo/load).
fn sync_background_controls(window: &Window, background: &Background) {
    match background {
        Background::Solid(color) => {
            window.background_type_row().set_selected(0);
            window.background_color_button().set_rgba(&gdk_rgba_from(color));
            window.gradient_color2_row().set_visible(false);
            window.gradient_angle_row().set_visible(false);
        }
        Background::Gradient(spec) => {
            window.background_type_row().set_selected(1);
            window.gradient_color2_row().set_visible(true);
            window.gradient_angle_row().set_visible(true);
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
        Background::Image(_) | Background::Decoration(_) => {
            // Not settable via this UI yet (spec §8 stubs) — leave controls
            // as they are rather than guessing a representative value.
        }
    }
}

fn gdk_rgba_from(c: &Rgba) -> gdk::RGBA {
    gdk::RGBA::new(c.r as f32, c.g as f32, c.b as f32, c.a as f32)
}

fn rgba_from_gdk(c: &gdk::RGBA) -> Rgba {
    Rgba::new(c.red() as f64, c.green() as f64, c.blue() as f64, c.alpha() as f64)
}

/// Reads the four background controls (type/color1/color2/angle) and builds
/// the `Background` they currently describe.
fn background_from_controls(window: &Window) -> Background {
    let color1 = rgba_from_gdk(&window.background_color_button().rgba());
    if window.background_type_row().selected() == 1 {
        let color2 = rgba_from_gdk(&window.gradient_color2_button().rgba());
        let angle_deg = window.gradient_angle_row().value();
        Background::Gradient(GradientSpec { kind: GradientKind::Linear { angle_deg }, stops: vec![(0.0, color1), (1.0, color2)] })
    } else {
        Background::Solid(color1)
    }
}

/// Applies whatever the background controls currently describe as one
/// undoable `SetBackground`, skipping the push if it doesn't actually
/// change anything — needed because `sync_controls_from_document` (after
/// undo/redo/load) sets these same controls to match the document it just
/// applied, which would otherwise re-fire this handler and wipe the redo
/// history it was trying to restore.
fn apply_background_from_controls(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let new = background_from_controls(window);
    let mut state_ref = state.borrow_mut();
    if state_ref.syncing_controls || state_ref.document.background == new {
        return;
    }
    let old = state_ref.document.background.clone();
    let EditorState { document, undo_stack, .. } = &mut *state_ref;
    undo_stack.apply(Box::new(SetBackground { old, new }), document);
    drop(state_ref);
    refresh_canvas(canvas, state);
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
    let corner_radius_row = window.corner_radius_row();

    sync_background_controls(window, &state.borrow().document.background);

    background_type_row.connect_selected_notify(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |row| {
            let is_gradient = row.selected() == 1;
            window.gradient_color2_row().set_visible(is_gradient);
            window.gradient_angle_row().set_visible(is_gradient);
            apply_background_from_controls(&window, &canvas, &state);
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

    shadow_row.connect_selected_notify(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |row| {
            let new = shadow_preset_for_index(row.selected());
            let mut state_ref = state.borrow_mut();
            // See the background-color handler above for why this guard
            // against a reentrant sync-triggered no-op is needed.
            if state_ref.syncing_controls || state_ref.document.elements.iter().all(|e| e.shadow == new) {
                return;
            }
            let old: Vec<ShadowParams> = state_ref.document.elements.iter().map(|e| e.shadow).collect();
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetShadowForAllElements { old, new }), document);
            drop(state_ref);
            refresh_canvas(&canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
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
            refresh_canvas(&canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));
}

fn export_format_for_index(index: u32) -> ExportFormat {
    match index {
        0 => ExportFormat::Png,
        1 => ExportFormat::Jpeg,
        _ => ExportFormat::WebP,
    }
}

fn index_for_export_format(format: ExportFormat) -> u32 {
    match format {
        ExportFormat::Png => 0,
        ExportFormat::Jpeg => 1,
        ExportFormat::WebP => 2,
    }
}

/// Wires the export-size/format/quality sidebar rows to `Document.canvas`.
/// These don't trigger a re-render (they don't affect the composition, only
/// the eventual export resolution/encoding), just a direct mutation.
fn register_export_controls(window: &Window, state: &Rc<RefCell<EditorState>>) {
    let width_row = window.export_width_row();
    let height_row = window.export_height_row();
    let format_row = window.export_format_row();
    let quality_row = window.export_quality_row();

    {
        let canvas_settings = state.borrow().document.canvas;
        width_row.set_value(canvas_settings.export_width as f64);
        height_row.set_value(canvas_settings.export_height as f64);
        format_row.set_selected(index_for_export_format(canvas_settings.export_format));
        quality_row.set_value(canvas_settings.export_quality as f64);
    }

    width_row.connect_value_notify(glib::clone!(
        #[strong]
        state,
        move |row| state.borrow_mut().document.canvas.export_width = row.value() as u32
    ));
    height_row.connect_value_notify(glib::clone!(
        #[strong]
        state,
        move |row| state.borrow_mut().document.canvas.export_height = row.value() as u32
    ));
    format_row.connect_selected_notify(glib::clone!(
        #[strong]
        state,
        move |row| state.borrow_mut().document.canvas.export_format = export_format_for_index(row.selected())
    ));
    quality_row.connect_value_notify(glib::clone!(
        #[strong]
        state,
        move |row| state.borrow_mut().document.canvas.export_quality = row.value() as u8
    ));
}

fn extension_for_format(format: ExportFormat) -> &'static str {
    match format {
        ExportFormat::Png => "png",
        ExportFormat::Jpeg => "jpg",
        ExportFormat::WebP => "webp",
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
                let decoded_images = state.borrow().decoded_images.clone();
                let result = gio::spawn_blocking(move || export::render_and_write(&doc, &decoded_images, &path)).await;

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
fn sync_controls_from_document(window: &Window, state: &Rc<RefCell<EditorState>>) {
    state.borrow_mut().syncing_controls = true;

    let doc = state.borrow().document.clone();

    window.spacing_row().set_value(doc.layout.spacing_px);
    window.margin_row().set_value(doc.layout.margin_px);

    sync_background_controls(window, &doc.background);

    if let Some(first) = doc.elements.first() {
        let presets =
            [ShadowParams::none(), ShadowParams::subtle(), ShadowParams::standard(), ShadowParams::strong(), ShadowParams::floating()];
        let index = presets.iter().position(|p| *p == first.shadow).unwrap_or(0) as u32;
        window.shadow_row().set_selected(index);
        window.corner_radius_row().set_value(first.corner_radius.top_left);
    }

    window.export_width_row().set_value(doc.canvas.export_width as f64);
    window.export_height_row().set_value(doc.canvas.export_height as f64);
    window.export_format_row().set_selected(index_for_export_format(doc.canvas.export_format));
    window.export_quality_row().set_value(doc.canvas.export_quality as f64);

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

                match screenforge_core::project::load(&path) {
                    Ok(doc) => {
                        let mut decoded = HashMap::new();
                        let mut missing = 0u32;
                        for element in &doc.elements {
                            let ImageSource::Path(source_path) = &element.source else { continue };
                            match import::decode_image(source_path) {
                                Ok(image) => {
                                    decoded.insert(element.id, image);
                                }
                                Err(err) => {
                                    missing += 1;
                                    eprintln!("ScreenForge: missing/unreadable image {}: {err}", source_path.display());
                                }
                            }
                        }

                        {
                            let mut state_ref = state.borrow_mut();
                            state_ref.document = doc;
                            state_ref.decoded_images = decoded;
                            state_ref.project_path = Some(path);
                            // A freshly loaded project starts with a clean
                            // undo history — undoing past "load" into the
                            // previous document would be surprising.
                            state_ref.undo_stack = UndoStack::new();
                        }
                        refresh_canvas(&canvas, &state);
                        sync_controls_from_document(&window, &state);
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
            refresh_canvas(&canvas, &state);
            sync_controls_from_document(&window, &state);
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
            refresh_canvas(&canvas, &state);
            sync_controls_from_document(&window, &state);
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
            refresh_canvas(&canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));
}
