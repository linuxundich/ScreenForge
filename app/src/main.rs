mod canvas;
mod export;
mod import;
mod window;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk4::gdk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use screenforge_core::command::{
    AddScreenshots, Command, DuplicateScreenshot, EnterFreeLayout, RemoveScreenshot, ReorderScreenshot, ReplaceScreenshotSource,
    SetBackground, SetCornerRadiusForAllElements, SetLayoutMode, SetMargin, SetShadowForAllElements, SetSpacing, SetTransform, UndoStack,
};
use screenforge_core::model::{
    Background, BackgroundImageFit, CornerRadius, Document, ExportFormat, GradientKind, GradientSpec, ImageBackgroundSpec, ImageSource,
    LayoutMode, Rgba, ScreenshotElement, ShadowParams,
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
}

impl EditorState {
    fn new() -> Self {
        Self {
            document: Document::new(),
            image_cache: HashMap::new(),
            project_path: None,
            undo_stack: UndoStack::new(),
            syncing_controls: false,
        }
    }
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
    gio::resources_register_include!("screenforge.gresource").expect("failed to register GResource bundle");

    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run()
}

fn build_ui(app: &adw::Application) {
    let window = Window::new(app);
    let canvas = window.canvas();

    let state = Rc::new(RefCell::new(EditorState::new()));
    refresh_canvas(&window, &canvas, &state);

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
    register_move(&window, &canvas, &state);
    register_resize(&window, &canvas, &state);
    register_context_menu(&window, &canvas, &state);
    register_paste_action(app, &window, &canvas, &state);

    window.present();
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
    let EditorState { document, image_cache, .. } = &mut *state_ref;
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
    canvas.set_document(document.clone(), surfaces, background_image);
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

/// Wires the sidebar's layout-mode/spacing/margin rows to `Document.layout`,
/// mutating it directly through the undo stack (spec §17: layout changes are
/// undoable).
fn register_layout_controls(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let layout_mode_row = window.layout_mode_row();
    let spacing_row = window.spacing_row();
    let margin_row = window.margin_row();

    {
        let state_ref = state.borrow();
        layout_mode_row.set_selected(index_for_layout_mode(state_ref.document.layout.mode));
        spacing_row.set_value(state_ref.document.layout.spacing_px);
        margin_row.set_value(state_ref.document.layout.margin_px);
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
                let placements =
                    screenforge_core::layout::compute_layout(old, &visible, doc.layout.spacing_px, doc.layout.margin_px);
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
            refresh_canvas(&window, &canvas, &state);
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
    window.background_color1_row().set_visible(!matches!(background, Background::Image(_)));
    window.gradient_color2_row().set_visible(matches!(background, Background::Gradient(_)));
    window.gradient_angle_row().set_visible(matches!(background, Background::Gradient(spec) if matches!(spec.kind, GradientKind::Linear { .. })));
    window.background_image_row().set_visible(matches!(background, Background::Image(_)));
    window.background_image_fit_row().set_visible(matches!(background, Background::Image(_)));
    window.background_image_opacity_row().set_visible(matches!(background, Background::Image(_)));

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
        Background::Decoration(_) => {
            // Not settable via this UI yet (spec §8 stub) — leave controls
            // as they are rather than guessing a representative value.
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

/// Reads the four background controls (type/color1/color2/angle) and builds
/// the `Background` they currently describe.
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
    let new = background_from_controls(window);
    let mut state_ref = state.borrow_mut();
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
            let selected = row.selected();
            window.background_color1_row().set_visible(selected != 3);
            window.gradient_color2_row().set_visible(selected == 1 || selected == 2);
            window.gradient_angle_row().set_visible(selected == 1);
            window.background_image_row().set_visible(selected == 3);
            window.background_image_fit_row().set_visible(selected == 3);
            window.background_image_opacity_row().set_visible(selected == 3);
            // Selecting "Bild" only reveals the file picker — there's
            // nothing to render until a file is actually chosen (below), so
            // this doesn't push a command yet.
            if selected != 3 {
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
            refresh_canvas(&window, &canvas, &state);
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
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));
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

    window.layout_mode_row().set_selected(index_for_layout_mode(doc.layout.mode));
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

    window.export_width_row().set_value(doc.canvas.export_target_width as f64);
    update_export_height_display(window, doc.canvas);
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
            refresh_canvas(&window, &canvas, &state);
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
            refresh_canvas(&window, &canvas, &state);
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
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));
}

/// Wires the canvas's `LayoutMode::Free` move-drag to an undoable
/// [`SetTransform`] (spec §8: manual positioning).
fn register_move(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    canvas.connect_move(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |index, new_x, new_y| {
            let mut state_ref = state.borrow_mut();
            let Some(element) = state_ref.document.elements.get(index) else { return };
            let old = element.transform;
            let mut new = old;
            new.x = new_x;
            new.y = new_y;
            let element_id = element.id;
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetTransform { element_id, old, new }), document);
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
