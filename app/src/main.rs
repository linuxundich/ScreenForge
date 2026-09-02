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
    AddScreenshots, ApplyTemplate, Command, DuplicateScreenshot, EnterFreeLayout, RemoveScreenshot, RemoveScreenshots, ReorderScreenshot,
    ReplaceScreenshotSource, SetBackground, SetCornerRadiusForAllElements, SetLayoutMode, SetMargin, SetScreenshotLabel,
    SetShadowForAllElements, SetSpacing, SetTitle, SetTransform, SetTransforms, UndoStack,
};
use screenforge_core::model::{
    Background, BackgroundImageFit, ColorStrategy, CornerRadius, Document, ExportFormat, GeneratedBackground, GradientKind, GradientSpec,
    HorizontalAnchor, ImageBackgroundSpec, ImageSource, LayoutMode, Rgba, ScreenshotElement, ShadowParams, ShadowPreset, TextAlign,
    TextBackground, TextElement, TextPosition, Typography, VerticalAnchor,
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
}

impl EditorState {
    /// A fresh document seeded from the user's saved preferences (default
    /// spacing/margin/export quality) rather than `LayoutSettings`'s own
    /// hardcoded defaults — see `app_settings()`.
    fn new() -> Self {
        let settings = app_settings();
        let mut document = Document::new();
        document.layout.spacing_px = settings.double("default-spacing");
        document.layout.margin_px = settings.double("default-margin");
        document.canvas.export_quality = settings.double("default-export-quality").round().clamp(1.0, 100.0) as u8;
        Self {
            document,
            image_cache: HashMap::new(),
            project_path: None,
            undo_stack: UndoStack::new(),
            syncing_controls: false,
            gradient_auto_seed: 0,
            hide_screenshots: false,
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
    register_preferences_action(&app);
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
    register_generator_controls(&window, &canvas, &state);
    register_title_controls(&window, &canvas, &state);
    register_export_controls(&window, &state);
    register_export_action(app, &window, &state);
    register_project_actions(app, &window, &canvas, &state);
    register_template_actions(&window, &canvas, &state);
    register_undo_redo_actions(app, &window, &canvas, &state);
    register_zoom_actions(app, &window, &canvas);
    register_reorder(&window, &canvas, &state);
    register_move(&window, &canvas, &state);
    register_resize(&window, &canvas, &state);
    register_context_menu(&window, &canvas, &state);
    register_delete_selected(app, &window, &canvas, &state);
    register_paste_action(app, &window, &canvas, &state);
    register_hide_screenshots_toggle(&window, &canvas, &state);

    window.present();
}

/// Wires the header bar's "Screenshots ausblenden" toggle to
/// `EditorState::hide_screenshots` — a pure preview flag (see its own doc
/// comment), so this never touches the undo stack.
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

/// Reflects a `GeneratedBackground`'s parameters onto the generator
/// controls — used both by `sync_background_controls`'s `Generated` arm
/// and after a fresh "Generieren" click updates the seed, mirroring
/// `sync_title_controls`'s role for the title.
fn sync_generator_controls(window: &Window, generated: &GeneratedBackground) {
    window.generator_color_strategy_row().set_selected(index_for_color_strategy(generated.color_strategy));
    let manual_buttons =
        [window.generator_manual_color_button_1(), window.generator_manual_color_button_2(), window.generator_manual_color_button_3(), window.generator_manual_color_button_4()];
    for (i, button) in manual_buttons.iter().enumerate() {
        let color = generated.palette.get(i).copied().unwrap_or(Rgba::new(0.5, 0.5, 0.5, 1.0));
        button.set_rgba(&gdk_rgba_from(&color));
    }
    window.generator_adapt_row().set_active(generated.adapt_to_screenshots);
    window.generator_inverse_contrast_row().set_value(generated.inverse_contrast * 100.0);
    window.generator_corner_bias_row().set_value(generated.corner_bias * 100.0);
    window.generator_offset_x_row().set_value(generated.offset_x * 100.0);
    window.generator_offset_y_row().set_value(generated.offset_y * 100.0);
    window.generator_scale_row().set_value(generated.scale * 100.0);
    window.generator_contrast_row().set_value(generated.contrast * 100.0);
    window.generator_seed_row().set_value(generated.seed as f64);
}

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
/// Subtle/Standard/Strong/Floating preset instead of falling back to "Kein
/// Schatten" just because a plain `ShadowParams` equality check would fail
/// once the angle no longer matches the preset's own baked-in 90°.
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
        .unwrap_or(0)
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
fn sync_title_controls(window: &Window, title: &TextElement) {
    window.title_enabled_row().set_active(title.enabled);
    window.title_content_row().set_text(&title.content);

    let is_absolute = matches!(title.position, TextPosition::Absolute { .. });
    window.title_position_mode_row().set_selected(if is_absolute { 1 } else { 0 });
    window.title_horizontal_row().set_visible(!is_absolute);
    window.title_vertical_row().set_visible(!is_absolute);
    window.title_padding_row().set_visible(!is_absolute);
    window.title_x_row().set_visible(is_absolute);
    window.title_y_row().set_visible(is_absolute);
    match title.position {
        TextPosition::Semantic { horizontal, vertical, padding } => {
            window.title_horizontal_row().set_selected(index_for_horizontal_anchor(horizontal));
            window.title_vertical_row().set_selected(index_for_vertical_anchor(vertical));
            window.title_padding_row().set_value(padding);
        }
        TextPosition::Absolute { x, y } => {
            window.title_x_row().set_value(x);
            window.title_y_row().set_value(y);
        }
    }

    let background_index = match &title.background {
        TextBackground::None => 0,
        TextBackground::Solid(_) => 1,
        TextBackground::Gradient(_) => 2,
    };
    window.title_background_row().set_selected(background_index);
    window.title_background_color_row().set_visible(background_index != 0);
    window.title_background_color2_row().set_visible(background_index == 2);
    match &title.background {
        TextBackground::Solid(color) => window.title_background_color_button().set_rgba(&gdk_rgba_from(color)),
        TextBackground::Gradient(spec) => {
            if let Some((_, color)) = spec.stops.first() {
                window.title_background_color_button().set_rgba(&gdk_rgba_from(color));
            }
            if let Some((_, color)) = spec.stops.get(1) {
                window.title_background_color2_button().set_rgba(&gdk_rgba_from(color));
            }
        }
        TextBackground::None => {}
    }

    window.title_corner_radius_row().set_value(title.corner_radius.top_left);
    window.title_font_button().set_font_desc(&font_desc_from_typography(&title.typography));
    window.title_alignment_row().set_selected(index_for_text_align(title.typography.alignment));
    window.title_letter_spacing_row().set_value(title.typography.letter_spacing);
    window.title_line_spacing_row().set_value(title.typography.line_spacing);
    window.title_color_button().set_rgba(&gdk_rgba_from(&title.typography.color));
    window.title_opacity_row().set_value(title.typography.opacity * 100.0);

    window.title_shadow_row().set_selected(shadow_preset_index_for(&title.shadow));
    let (angle, distance) = title.shadow.angle_and_distance();
    window.title_shadow_angle_row().set_value(angle);
    window.title_shadow_distance_row().set_value(distance);
    window.title_shadow_blur_row().set_value(title.shadow.blur);

    let controls_enabled = title.enabled;
    for row in [
        &window.title_content_row().clone().upcast::<gtk4::Widget>(),
        &window.title_position_mode_row().clone().upcast::<gtk4::Widget>(),
        &window.title_background_row().clone().upcast::<gtk4::Widget>(),
        &window.title_corner_radius_row().clone().upcast::<gtk4::Widget>(),
        &window.title_font_row().clone().upcast::<gtk4::Widget>(),
        &window.title_alignment_row().clone().upcast::<gtk4::Widget>(),
        &window.title_letter_spacing_row().clone().upcast::<gtk4::Widget>(),
        &window.title_line_spacing_row().clone().upcast::<gtk4::Widget>(),
        &window.title_color_row().clone().upcast::<gtk4::Widget>(),
        &window.title_opacity_row().clone().upcast::<gtk4::Widget>(),
        &window.title_shadow_row().clone().upcast::<gtk4::Widget>(),
    ] {
        row.set_sensitive(controls_enabled);
    }
    window.title_horizontal_row().set_sensitive(controls_enabled);
    window.title_vertical_row().set_sensitive(controls_enabled);
    window.title_padding_row().set_sensitive(controls_enabled);
    window.title_x_row().set_sensitive(controls_enabled);
    window.title_y_row().set_sensitive(controls_enabled);
    window.title_background_color_row().set_sensitive(controls_enabled);
    window.title_background_color2_row().set_sensitive(controls_enabled);
    let shadow_geometry_enabled = controls_enabled && title.shadow.enabled;
    window.title_shadow_angle_row().set_sensitive(shadow_geometry_enabled);
    window.title_shadow_distance_row().set_sensitive(shadow_geometry_enabled);
    window.title_shadow_blur_row().set_sensitive(shadow_geometry_enabled);
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
    window.generator_adapt_row().set_visible(is_generated);
    window.generator_corner_bias_row().set_visible(is_generated);
    window.generator_offset_x_row().set_visible(is_generated);
    window.generator_offset_y_row().set_visible(is_generated);
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
            window.generator_adapt_row().set_visible(is_generated);
            window.generator_corner_bias_row().set_visible(is_generated);
            window.generator_offset_x_row().set_visible(is_generated);
            window.generator_offset_y_row().set_visible(is_generated);
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
            let preset = shadow_preset_for_index(row.selected());
            // `with_preset` only touches distance/blur/opacity/color and
            // keeps whichever angle the shadow already had — a preset is a
            // statement about how strong a shadow looks, not which
            // direction it's cast (spec: choosing Subtle/Standard/Strong
            // must never reset a user-chosen angle back to 90°).
            let mut state_ref = state.borrow_mut();
            let was_syncing = state_ref.syncing_controls;
            let current = state_ref.document.elements.first().map(|e| e.shadow).unwrap_or_default();
            let new = current.with_preset(preset);

            // Guard these writes the same way `sync_controls_from_document`
            // guards its own batch: without it, each `set_value` below
            // reentrantly fires `apply_shadow_geometry`, which would push
            // its own spurious undo command built from a partially-updated
            // mix of old and new values. Saved/restored rather than
            // unconditionally cleared, since this handler itself can be
            // reentered from inside that batch (`shadow_row.set_selected`
            // in `sync_controls_from_document`) — clearing the flag there
            // would drop the guard for that batch's *remaining* writes.
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
            state_ref.syncing_controls = was_syncing;
            if was_syncing || state_ref.document.elements.iter().all(|e| e.shadow == new) {
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

/// Wires the composition-wide title's sidebar controls (spec §5-§10):
/// enable/content, semantic or manual position, background, corner radius,
/// typography (via a native `GtkFontDialogButton`), and an independently
/// cached shadow — all funneled through one `apply_title` that rebuilds
/// the whole `TextElement` and pushes a single `SetTitle` undo step,
/// except the shadow *preset* dropdown, which (like the screenshot
/// shadow's) needs to preserve the current angle rather than reset it —
/// see `ShadowParams::with_preset`.
fn register_title_controls(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    {
        let state_ref = state.borrow();
        sync_title_controls(window, &state_ref.document.title);
    }

    let apply_title = glib::clone!(
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

            let position = if window.title_position_mode_row().selected() == 1 {
                TextPosition::Absolute { x: window.title_x_row().value(), y: window.title_y_row().value() }
            } else {
                TextPosition::Semantic {
                    horizontal: horizontal_anchor_for_index(window.title_horizontal_row().selected()),
                    vertical: vertical_anchor_for_index(window.title_vertical_row().selected()),
                    padding: window.title_padding_row().value(),
                }
            };
            let background = match window.title_background_row().selected() {
                1 => TextBackground::Solid(rgba_from_gdk(&window.title_background_color_button().rgba())),
                2 => TextBackground::Gradient(GradientSpec {
                    kind: GradientKind::Linear { angle_deg: 135.0 },
                    stops: vec![
                        (0.0, rgba_from_gdk(&window.title_background_color_button().rgba())),
                        (1.0, rgba_from_gdk(&window.title_background_color2_button().rgba())),
                    ],
                }),
                _ => TextBackground::None,
            };
            let font_desc = window.title_font_button().font_desc().unwrap_or_else(pango::FontDescription::new);
            let (font_family, font_size, weight, italic) = typography_from_font_desc(&font_desc);

            // The shadow has its own dedicated handlers below (mirroring
            // the screenshot shadow's preset-vs-geometry split), so it's
            // carried over unchanged here rather than rebuilt from
            // controls this closure doesn't read.
            let shadow = state_ref.document.title.shadow;
            let background_padding = state_ref.document.title.background_padding;

            let new = TextElement {
                enabled: window.title_enabled_row().is_active(),
                content: window.title_content_row().text().to_string(),
                position,
                typography: Typography {
                    font_family,
                    font_size,
                    weight,
                    italic,
                    color: rgba_from_gdk(&window.title_color_button().rgba()),
                    alignment: text_align_for_index(window.title_alignment_row().selected()),
                    opacity: window.title_opacity_row().value() / 100.0,
                    letter_spacing: window.title_letter_spacing_row().value(),
                    line_spacing: window.title_line_spacing_row().value(),
                    wrap: false,
                },
                background,
                corner_radius: CornerRadius::uniform(window.title_corner_radius_row().value()),
                background_padding,
                shadow,
            };
            if state_ref.document.title == new {
                return;
            }
            let old = state_ref.document.title.clone();
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetTitle { old, new }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    );

    window.title_enabled_row().connect_active_notify(glib::clone!(
        #[weak]
        window,
        #[strong]
        apply_title,
        move |row| {
            let enabled = row.is_active();
            for widget in [
                window.title_content_row().upcast::<gtk4::Widget>(),
                window.title_position_mode_row().upcast(),
                window.title_horizontal_row().upcast(),
                window.title_vertical_row().upcast(),
                window.title_padding_row().upcast(),
                window.title_x_row().upcast(),
                window.title_y_row().upcast(),
                window.title_background_row().upcast(),
                window.title_background_color_row().upcast(),
                window.title_background_color2_row().upcast(),
                window.title_corner_radius_row().upcast(),
                window.title_font_row().upcast(),
                window.title_alignment_row().upcast(),
                window.title_letter_spacing_row().upcast(),
                window.title_line_spacing_row().upcast(),
                window.title_color_row().upcast(),
                window.title_opacity_row().upcast(),
                window.title_shadow_row().upcast(),
            ] {
                widget.set_sensitive(enabled);
            }
            let shadow_geometry_enabled = enabled && window.title_shadow_row().selected() != 0;
            window.title_shadow_angle_row().set_sensitive(shadow_geometry_enabled);
            window.title_shadow_distance_row().set_sensitive(shadow_geometry_enabled);
            window.title_shadow_blur_row().set_sensitive(shadow_geometry_enabled);
            apply_title();
        }
    ));
    window.title_content_row().connect_changed(glib::clone!(
        #[strong]
        apply_title,
        move |_| apply_title()
    ));
    window.title_position_mode_row().connect_selected_notify(glib::clone!(
        #[weak]
        window,
        #[strong]
        apply_title,
        move |row| {
            let is_absolute = row.selected() == 1;
            window.title_horizontal_row().set_visible(!is_absolute);
            window.title_vertical_row().set_visible(!is_absolute);
            window.title_padding_row().set_visible(!is_absolute);
            window.title_x_row().set_visible(is_absolute);
            window.title_y_row().set_visible(is_absolute);
            apply_title();
        }
    ));
    window.title_horizontal_row().connect_selected_notify(glib::clone!(
        #[strong]
        apply_title,
        move |_| apply_title()
    ));
    window.title_vertical_row().connect_selected_notify(glib::clone!(
        #[strong]
        apply_title,
        move |_| apply_title()
    ));
    window.title_padding_row().connect_value_notify(glib::clone!(
        #[strong]
        apply_title,
        move |_| apply_title()
    ));
    window.title_x_row().connect_value_notify(glib::clone!(
        #[strong]
        apply_title,
        move |_| apply_title()
    ));
    window.title_y_row().connect_value_notify(glib::clone!(
        #[strong]
        apply_title,
        move |_| apply_title()
    ));
    window.title_background_row().connect_selected_notify(glib::clone!(
        #[weak]
        window,
        #[strong]
        apply_title,
        move |row| {
            let selected = row.selected();
            window.title_background_color_row().set_visible(selected != 0);
            window.title_background_color2_row().set_visible(selected == 2);
            apply_title();
        }
    ));
    window.title_background_color_button().connect_rgba_notify(glib::clone!(
        #[strong]
        apply_title,
        move |_| apply_title()
    ));
    window.title_background_color2_button().connect_rgba_notify(glib::clone!(
        #[strong]
        apply_title,
        move |_| apply_title()
    ));
    window.title_corner_radius_row().connect_value_notify(glib::clone!(
        #[strong]
        apply_title,
        move |_| apply_title()
    ));
    window.title_font_button().connect_font_desc_notify(glib::clone!(
        #[strong]
        apply_title,
        move |_| apply_title()
    ));
    window.title_alignment_row().connect_selected_notify(glib::clone!(
        #[strong]
        apply_title,
        move |_| apply_title()
    ));
    window.title_letter_spacing_row().connect_value_notify(glib::clone!(
        #[strong]
        apply_title,
        move |_| apply_title()
    ));
    window.title_line_spacing_row().connect_value_notify(glib::clone!(
        #[strong]
        apply_title,
        move |_| apply_title()
    ));
    window.title_color_button().connect_rgba_notify(glib::clone!(
        #[strong]
        apply_title,
        move |_| apply_title()
    ));
    window.title_opacity_row().connect_value_notify(glib::clone!(
        #[strong]
        apply_title,
        move |_| apply_title()
    ));

    // Shadow preset dropdown: mirrors the screenshot shadow preset handler
    // exactly (`ShadowParams::with_preset` preserves the angle; the
    // syncing_controls save/restore guards against a reentrant partial-
    // state undo push while distance/blur are updated below) but commits
    // directly via `SetTitle` rather than routing through `apply_title`,
    // since `apply_title` deliberately doesn't touch the shadow at all.
    window.title_shadow_row().connect_selected_notify(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |row| {
            let preset = shadow_preset_for_index(row.selected());
            let mut state_ref = state.borrow_mut();
            let was_syncing = state_ref.syncing_controls;
            let current_shadow = state_ref.document.title.shadow;
            let new_shadow = current_shadow.with_preset(preset);
            state_ref.syncing_controls = true;
            drop(state_ref);

            window.title_shadow_distance_row().set_value(new_shadow.angle_and_distance().1);
            window.title_shadow_blur_row().set_value(new_shadow.blur);
            window.title_shadow_angle_row().set_sensitive(new_shadow.enabled);
            window.title_shadow_distance_row().set_sensitive(new_shadow.enabled);
            window.title_shadow_blur_row().set_sensitive(new_shadow.enabled);

            let mut state_ref = state.borrow_mut();
            state_ref.syncing_controls = was_syncing;
            if was_syncing {
                return;
            }
            let mut new_title = state_ref.document.title.clone();
            new_title.shadow = new_shadow;
            if state_ref.document.title == new_title {
                return;
            }
            let old = state_ref.document.title.clone();
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetTitle { old, new: new_title }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));

    let apply_title_shadow_geometry = glib::clone!(
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
            let angle = window.title_shadow_angle_row().value();
            let distance = window.title_shadow_distance_row().value();
            let blur = window.title_shadow_blur_row().value();
            let (offset_x, offset_y) = ShadowParams::offset_for_angle_and_distance(angle, distance);

            let mut new_title = state_ref.document.title.clone();
            new_title.shadow.offset_x = offset_x;
            new_title.shadow.offset_y = offset_y;
            new_title.shadow.blur = blur;
            if state_ref.document.title == new_title {
                return;
            }
            let old = state_ref.document.title.clone();
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetTitle { old, new: new_title }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    );
    window.title_shadow_angle_row().connect_value_notify(glib::clone!(
        #[strong]
        apply_title_shadow_geometry,
        move |_| apply_title_shadow_geometry()
    ));
    window.title_shadow_distance_row().connect_value_notify(glib::clone!(
        #[strong]
        apply_title_shadow_geometry,
        move |_| apply_title_shadow_geometry()
    ));
    window.title_shadow_blur_row().connect_value_notify(glib::clone!(
        #[strong]
        apply_title_shadow_geometry,
        move |_| apply_title_shadow_geometry()
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
            let Background::Generated(current) = &state_ref.document.background else { return };
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
            let new = GeneratedBackground {
                seed: window.generator_seed_row().value() as u64,
                color_strategy,
                palette,
                adapt_to_screenshots: window.generator_adapt_row().is_active(),
                inverse_contrast: window.generator_inverse_contrast_row().value() / 100.0,
                corner_bias: window.generator_corner_bias_row().value() / 100.0,
                offset_x: window.generator_offset_x_row().value() / 100.0,
                offset_y: window.generator_offset_y_row().value() / 100.0,
                scale: window.generator_scale_row().value() / 100.0,
                // No sliders for these — they're only ever redrawn from
                // scratch by `generate_background`'s own randomization, so
                // editing any *other* generator control must leave them
                // exactly as they were.
                density: current.density,
                flow: current.flow,
                variation: current.variation,
                contrast: window.generator_contrast_row().value() / 100.0,
                softness: current.softness,
            };
            if *current == new {
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
        apply,
        move |row| {
            sync_generator_color_strategy_visibility(&window, true, color_strategy_for_index(row.selected()));
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
    window.generator_offset_x_row().connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));
    window.generator_offset_y_row().connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));
    window.generator_scale_row().connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));
    window.generator_contrast_row().connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));
    window.generator_seed_row().connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));

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

    let previous_seed = match &state_ref.document.background {
        Background::Generated(g) => g.seed,
        _ => 0,
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
        let samples: Vec<screenforge_core::palette::PixelSample> = images
            .iter()
            .map(|image| screenforge_core::palette::PixelSample { bytes: &image.bytes, width: image.width, height: image.height })
            .collect();
        screenforge_core::palette::resolve_palette(&samples, color_strategy, inverse_contrast, new_seed)
    };

    let new = GeneratedBackground {
        seed: new_seed,
        color_strategy,
        palette,
        adapt_to_screenshots: window.generator_adapt_row().is_active(),
        inverse_contrast,
        corner_bias: window.generator_corner_bias_row().value() / 100.0,
        offset_x: window.generator_offset_x_row().value() / 100.0,
        offset_y: window.generator_offset_y_row().value() / 100.0,
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

    sync_title_controls(window, &doc.title);

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

/// `win.save-template`/`win.load-template`: a template captures everything
/// that makes a composition *look* the way it does (layout mode/spacing/
/// margin, background, shadow, corner radius) separately from the
/// screenshots themselves, so it can be reapplied to a different set of
/// images later. Saving never touches `Document.elements`; loading applies
/// the saved style as one undoable [`ApplyTemplate`], mirroring how
/// project-loading resyncs the sidebar afterward.
fn register_template_actions(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let save_action = gio::SimpleAction::new("save-template", None);
    save_action.connect_activate(glib::clone!(
        #[weak]
        window,
        #[strong]
        state,
        move |_, _| {
            let window = window.clone();
            let state = state.clone();
            glib::spawn_future_local(async move {
                let filter = gtk4::FileFilter::new();
                filter.add_pattern("*.screenforge-template");
                filter.set_name(Some("ScreenForge-Vorlagen"));

                let dialog = gtk4::FileDialog::builder()
                    .title("Vorlage speichern unter")
                    .accept_label("Speichern")
                    .initial_name("vorlage.screenforge-template")
                    .default_filter(&filter)
                    .build();

                let file = match dialog.save_future(Some(&window)).await {
                    Ok(file) => file,
                    Err(err) => {
                        if !err.matches(gtk4::DialogError::Dismissed) {
                            eprintln!("ScreenForge: save-template dialog failed: {err}");
                        }
                        return;
                    }
                };
                let Some(path) = file.path() else { return };

                let template = screenforge_core::template::Template::from_document(&state.borrow().document);
                let toast = match screenforge_core::template::save(&template, &path) {
                    Ok(()) => adw::Toast::new("Vorlage gespeichert"),
                    Err(err) => adw::Toast::new(&format!("Vorlage konnte nicht gespeichert werden: {err}")),
                };
                window.toast_overlay().add_toast(toast);
            });
        }
    ));
    window.add_action(&save_action);

    let load_action = gio::SimpleAction::new("load-template", None);
    load_action.connect_activate(glib::clone!(
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
                filter.add_pattern("*.screenforge-template");
                filter.set_name(Some("ScreenForge-Vorlagen"));

                let dialog = gtk4::FileDialog::builder()
                    .title("Vorlage laden")
                    .accept_label("Laden")
                    .default_filter(&filter)
                    .build();

                let file = match dialog.open_future(Some(&window)).await {
                    Ok(file) => file,
                    Err(err) => {
                        if !err.matches(gtk4::DialogError::Dismissed) {
                            eprintln!("ScreenForge: load-template dialog failed: {err}");
                        }
                        return;
                    }
                };
                let Some(path) = file.path() else { return };

                let new = match screenforge_core::template::load(&path) {
                    Ok(template) => template,
                    Err(err) => {
                        window.toast_overlay().add_toast(adw::Toast::new(&format!("Vorlage konnte nicht geladen werden: {err}")));
                        return;
                    }
                };

                {
                    let mut state_ref = state.borrow_mut();
                    let old_layout = state_ref.document.layout;
                    let old_background = state_ref.document.background.clone();
                    let old_shadows: Vec<ShadowParams> = state_ref.document.elements.iter().map(|e| e.shadow).collect();
                    let old_corner_radii: Vec<CornerRadius> = state_ref.document.elements.iter().map(|e| e.corner_radius).collect();
                    let EditorState { document, undo_stack, .. } = &mut *state_ref;
                    undo_stack.apply(Box::new(ApplyTemplate { old_layout, old_background, old_shadows, old_corner_radii, new }), document);
                }
                refresh_canvas(&window, &canvas, &state);
                sync_controls_from_document(&window, &state);
                update_undo_redo_sensitivity(&window, &state);
                window.toast_overlay().add_toast(adw::Toast::new("Vorlage angewendet"));
            });
        }
    ));
    window.add_action(&load_action);
}

/// `app.preferences` (`Ctrl+,`): a `GSettings`-backed preferences dialog
/// with defaults applied to every *newly created* document (see
/// `EditorState::new`) — changing a preference never touches the document
/// currently open, only what a fresh one starts with. Each row binds
/// straight to its `GSettings` key via `Settings::bind`, so there's no
/// manual load/save glue: GLib keeps the setting and the widget in sync
/// both ways for as long as the dialog is open.
fn register_preferences_action(app: &adw::Application) {
    let action = gio::SimpleAction::new("preferences", None);
    action.connect_activate(glib::clone!(
        #[weak]
        app,
        move |_, _| {
            let settings = app_settings();

            let spacing_row = adw::SpinRow::with_range(0.0, 500.0, 4.0);
            spacing_row.set_title("Abstand");
            spacing_row.set_subtitle("Zwischen den Screenshots, in Pixeln");
            settings.bind("default-spacing", &spacing_row, "value").build();

            let margin_row = adw::SpinRow::with_range(0.0, 500.0, 4.0);
            margin_row.set_title("Außenrand");
            margin_row.set_subtitle("Rand um die Komposition, in Pixeln");
            settings.bind("default-margin", &margin_row, "value").build();

            let quality_row = adw::SpinRow::with_range(1.0, 100.0, 5.0);
            quality_row.set_title("Export-Qualität");
            quality_row.set_subtitle("Für JPEG/WebP/AVIF, in Prozent");
            settings.bind("default-export-quality", &quality_row, "value").build();

            let group = adw::PreferencesGroup::new();
            group.set_title("Standardwerte für neue Projekte");
            group.add(&spacing_row);
            group.add(&margin_row);
            group.add(&quality_row);

            let page = adw::PreferencesPage::new();
            page.add(&group);

            let dialog = adw::PreferencesDialog::new();
            dialog.add(&page);
            dialog.present(app.active_window().as_ref());
        }
    ));
    app.add_action(&action);
    app.set_accels_for_action("app.preferences", &["<Ctrl>comma"]);
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

    let label_section = gio::Menu::new();
    label_section.append(Some("Beschriftung…"), Some("win.edit-screenshot-label"));
    menu.append_section(None, &label_section);

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
/// Opens a small live-editing dialog for one screenshot's own label (spec
/// §11-§14), reached via the context menu's "Beschriftung…" rather than
/// living in the main sidebar — most screenshots don't have one, and the
/// sidebar already carries a very similar control set for the
/// composition-wide title (`register_title_controls`). Edits apply live
/// and undoably as controls change, the same as every other control in
/// this app, rather than needing an OK/Cancel of their own. Looked up by
/// `element_id` rather than index so the dialog stays valid even if
/// reordering/undo shifts indices while it's open.
fn spin_row(title: &str, lower: f64, upper: f64, value: f64) -> adw::SpinRow {
    let adjustment = gtk4::Adjustment::new(value, lower, upper, 1.0, 10.0, 0.0);
    let row = adw::SpinRow::new(Some(&adjustment), 1.0, 1);
    row.set_title(title);
    row
}

fn open_label_editor(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>, element_id: Uuid) {
    let Some(label) = state.borrow().document.elements.iter().find(|e| e.id == element_id).map(|e| e.label.clone()) else { return };

    let dialog = adw::PreferencesDialog::new();
    dialog.set_title("Beschriftung");
    dialog.set_content_width(420);

    let page = adw::PreferencesPage::new();
    let group = adw::PreferencesGroup::new();
    group.set_title("Beschriftung");
    group.set_description(Some("Bewegt und skaliert sich mit diesem Screenshot"));

    let enabled_row = adw::SwitchRow::new();
    enabled_row.set_title("Beschriftung anzeigen");
    group.add(&enabled_row);

    let content_row = adw::EntryRow::new();
    content_row.set_title("Text");
    group.add(&content_row);

    let position_mode_row = adw::ComboRow::new();
    position_mode_row.set_title("Position");
    position_mode_row.set_model(Some(&gtk4::StringList::new(&["Automatisch", "Manuell (X/Y)"])));
    group.add(&position_mode_row);

    let horizontal_row = adw::ComboRow::new();
    horizontal_row.set_title("Horizontal");
    horizontal_row.set_model(Some(&gtk4::StringList::new(&["Links", "Mitte", "Rechts"])));
    group.add(&horizontal_row);

    let vertical_row = adw::ComboRow::new();
    vertical_row.set_title("Vertikal");
    vertical_row.set_model(Some(&gtk4::StringList::new(&["Oben", "Mitte", "Unten"])));
    group.add(&vertical_row);

    let padding_row = spin_row("Randabstand", 0.0, 500.0, 16.0);
    group.add(&padding_row);

    let x_row = spin_row("X-Position", 0.0, 8000.0, 0.0);
    x_row.set_subtitle("In Pixeln, relativ zum Screenshot");
    x_row.set_visible(false);
    group.add(&x_row);

    let y_row = spin_row("Y-Position", 0.0, 8000.0, 0.0);
    y_row.set_subtitle("In Pixeln, relativ zum Screenshot");
    y_row.set_visible(false);
    group.add(&y_row);

    let background_row = adw::ComboRow::new();
    background_row.set_title("Hintergrund");
    background_row.set_model(Some(&gtk4::StringList::new(&["Kein Hintergrund", "Einfarbig", "Verlauf"])));
    group.add(&background_row);

    let background_color_row = adw::ActionRow::new();
    background_color_row.set_title("Hintergrundfarbe");
    let background_color_button = gtk4::ColorDialogButton::new(Some(gtk4::ColorDialog::builder().with_alpha(true).build()));
    background_color_button.set_valign(gtk4::Align::Center);
    background_color_row.add_suffix(&background_color_button);
    group.add(&background_color_row);

    let background_color2_row = adw::ActionRow::new();
    background_color2_row.set_title("Hintergrundfarbe 2");
    let background_color2_button = gtk4::ColorDialogButton::new(Some(gtk4::ColorDialog::builder().with_alpha(true).build()));
    background_color2_button.set_valign(gtk4::Align::Center);
    background_color2_row.add_suffix(&background_color2_button);
    group.add(&background_color2_row);

    let corner_radius_row = spin_row("Eckenradius", 0.0, 200.0, 0.0);
    group.add(&corner_radius_row);

    let font_row = adw::ActionRow::new();
    font_row.set_title("Schrift");
    let font_button = gtk4::FontDialogButton::new(Some(gtk4::FontDialog::new()));
    font_button.set_valign(gtk4::Align::Center);
    font_row.add_suffix(&font_button);
    group.add(&font_row);

    let color_row = adw::ActionRow::new();
    color_row.set_title("Textfarbe");
    let color_button = gtk4::ColorDialogButton::new(Some(gtk4::ColorDialog::new()));
    color_button.set_valign(gtk4::Align::Center);
    color_row.add_suffix(&color_button);
    group.add(&color_row);

    let letter_spacing_row = adw::SpinRow::new(Some(&gtk4::Adjustment::new(0.0, -5.0, 50.0, 0.5, 2.0, 0.0)), 0.5, 1);
    letter_spacing_row.set_title("Zeichenabstand");
    letter_spacing_row.set_subtitle("In Pixeln");
    group.add(&letter_spacing_row);

    let line_spacing_row = adw::SpinRow::new(Some(&gtk4::Adjustment::new(1.2, 0.5, 3.0, 0.1, 0.5, 0.0)), 0.1, 2);
    line_spacing_row.set_title("Zeilenabstand");
    line_spacing_row.set_subtitle("Faktor, 1,0 = normal");
    group.add(&line_spacing_row);

    let opacity_row = spin_row("Deckkraft (%)", 0.0, 100.0, 100.0);
    group.add(&opacity_row);

    let shadow_row = adw::ComboRow::new();
    shadow_row.set_title("Schatten");
    shadow_row.set_model(Some(&gtk4::StringList::new(&["Kein Schatten", "Subtil", "Standard", "Stark", "Floating"])));
    group.add(&shadow_row);

    let shadow_angle_row = spin_row("Schatten-Winkel", 0.0, 360.0, 90.0);
    group.add(&shadow_angle_row);
    let shadow_distance_row = spin_row("Schatten-Distanz", 0.0, 300.0, 6.0);
    group.add(&shadow_distance_row);
    let shadow_blur_row = spin_row("Weichzeichner", 0.0, 150.0, 16.0);
    group.add(&shadow_blur_row);

    page.add(&group);
    dialog.add(&page);

    // -- reflect the label's current value onto every row --
    let sync_from = |label: &TextElement| {
        enabled_row.set_active(label.enabled);
        content_row.set_text(&label.content);
        let is_absolute = matches!(label.position, TextPosition::Absolute { .. });
        position_mode_row.set_selected(if is_absolute { 1 } else { 0 });
        horizontal_row.set_visible(!is_absolute);
        vertical_row.set_visible(!is_absolute);
        padding_row.set_visible(!is_absolute);
        x_row.set_visible(is_absolute);
        y_row.set_visible(is_absolute);
        match label.position {
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

        let background_index = match &label.background {
            TextBackground::None => 0,
            TextBackground::Solid(_) => 1,
            TextBackground::Gradient(_) => 2,
        };
        background_row.set_selected(background_index);
        background_color_row.set_visible(background_index != 0);
        background_color2_row.set_visible(background_index == 2);
        match &label.background {
            TextBackground::Solid(color) => background_color_button.set_rgba(&gdk_rgba_from(color)),
            TextBackground::Gradient(spec) => {
                if let Some((_, color)) = spec.stops.first() {
                    background_color_button.set_rgba(&gdk_rgba_from(color));
                }
                if let Some((_, color)) = spec.stops.get(1) {
                    background_color2_button.set_rgba(&gdk_rgba_from(color));
                }
            }
            TextBackground::None => {}
        }

        corner_radius_row.set_value(label.corner_radius.top_left);
        font_button.set_font_desc(&font_desc_from_typography(&label.typography));
        color_button.set_rgba(&gdk_rgba_from(&label.typography.color));
        letter_spacing_row.set_value(label.typography.letter_spacing);
        line_spacing_row.set_value(label.typography.line_spacing);
        opacity_row.set_value(label.typography.opacity * 100.0);

        shadow_row.set_selected(shadow_preset_index_for(&label.shadow));
        let (angle, distance) = label.shadow.angle_and_distance();
        shadow_angle_row.set_value(angle);
        shadow_distance_row.set_value(distance);
        shadow_blur_row.set_value(label.shadow.blur);

        let enabled = label.enabled;
        for widget in [
            content_row.clone().upcast::<gtk4::Widget>(),
            position_mode_row.clone().upcast(),
            horizontal_row.clone().upcast(),
            vertical_row.clone().upcast(),
            padding_row.clone().upcast(),
            x_row.clone().upcast(),
            y_row.clone().upcast(),
            background_row.clone().upcast(),
            background_color_row.clone().upcast(),
            background_color2_row.clone().upcast(),
            corner_radius_row.clone().upcast(),
            font_row.clone().upcast(),
            color_row.clone().upcast(),
            letter_spacing_row.clone().upcast(),
            line_spacing_row.clone().upcast(),
            opacity_row.clone().upcast(),
            shadow_row.clone().upcast(),
        ] {
            widget.set_sensitive(enabled);
        }
        let shadow_geometry_enabled = enabled && label.shadow.enabled;
        shadow_angle_row.set_sensitive(shadow_geometry_enabled);
        shadow_distance_row.set_sensitive(shadow_geometry_enabled);
        shadow_blur_row.set_sensitive(shadow_geometry_enabled);
    };
    sync_from(&label);

    // Guards the sync above (and the shadow preset handler's own display-
    // only writes below) against reentrantly pushing a spurious partial
    // undo command the same way `register_title_controls` does — this
    // dialog doesn't share `EditorState.syncing_controls` (it isn't
    // touching the sidebar), so it keeps its own tiny local flag instead.
    let syncing = Rc::new(Cell::new(false));

    let apply = glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        #[strong]
        syncing,
        #[weak]
        enabled_row,
        #[weak]
        content_row,
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
        corner_radius_row,
        #[weak]
        font_button,
        #[weak]
        color_button,
        #[weak]
        letter_spacing_row,
        #[weak]
        line_spacing_row,
        #[weak]
        opacity_row,
        move || {
            if syncing.get() {
                return;
            }
            let mut state_ref = state.borrow_mut();
            let Some(current) = state_ref.document.elements.iter().find(|e| e.id == element_id).map(|e| e.label.clone()) else { return };

            let background = match background_row.selected() {
                1 => TextBackground::Solid(rgba_from_gdk(&background_color_button.rgba())),
                2 => TextBackground::Gradient(GradientSpec {
                    kind: GradientKind::Linear { angle_deg: 135.0 },
                    stops: vec![(0.0, rgba_from_gdk(&background_color_button.rgba())), (1.0, rgba_from_gdk(&background_color2_button.rgba()))],
                }),
                _ => TextBackground::None,
            };
            let font_desc = font_button.font_desc().unwrap_or_else(pango::FontDescription::new);
            let (font_family, font_size, weight, italic) = typography_from_font_desc(&font_desc);

            let position = if position_mode_row.selected() == 1 {
                TextPosition::Absolute { x: x_row.value(), y: y_row.value() }
            } else {
                TextPosition::Semantic {
                    horizontal: horizontal_anchor_for_index(horizontal_row.selected()),
                    vertical: vertical_anchor_for_index(vertical_row.selected()),
                    padding: padding_row.value(),
                }
            };

            let new = TextElement {
                enabled: enabled_row.is_active(),
                content: content_row.text().to_string(),
                position,
                typography: Typography {
                    font_family,
                    font_size,
                    weight,
                    italic,
                    color: rgba_from_gdk(&color_button.rgba()),
                    alignment: current.typography.alignment,
                    opacity: opacity_row.value() / 100.0,
                    letter_spacing: letter_spacing_row.value(),
                    line_spacing: line_spacing_row.value(),
                    wrap: current.typography.wrap,
                },
                background,
                corner_radius: CornerRadius::uniform(corner_radius_row.value()),
                background_padding: current.background_padding,
                shadow: current.shadow,
            };
            if current == new {
                return;
            }
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetScreenshotLabel { element_id, old: current, new }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    );

    enabled_row.connect_active_notify(glib::clone!(
        #[strong]
        state,
        #[strong]
        apply,
        #[weak]
        enabled_row,
        #[weak]
        content_row,
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
        background_color_row,
        #[weak]
        background_color2_row,
        #[weak]
        corner_radius_row,
        #[weak]
        font_row,
        #[weak]
        color_row,
        #[weak]
        letter_spacing_row,
        #[weak]
        line_spacing_row,
        #[weak]
        opacity_row,
        #[weak]
        shadow_row,
        #[weak]
        shadow_angle_row,
        #[weak]
        shadow_distance_row,
        #[weak]
        shadow_blur_row,
        move |_| {
            let enabled = enabled_row.is_active();
            if let Some(label) = state.borrow().document.elements.iter().find(|e| e.id == element_id).map(|e| e.label.clone()) {
                let shadow_geometry_enabled = enabled && label.shadow.enabled;
                shadow_angle_row.set_sensitive(shadow_geometry_enabled);
                shadow_distance_row.set_sensitive(shadow_geometry_enabled);
                shadow_blur_row.set_sensitive(shadow_geometry_enabled);
            }
            for widget in [
                content_row.clone().upcast::<gtk4::Widget>(),
                position_mode_row.clone().upcast(),
                horizontal_row.clone().upcast(),
                vertical_row.clone().upcast(),
                padding_row.clone().upcast(),
                x_row.clone().upcast(),
                y_row.clone().upcast(),
                background_row.clone().upcast(),
                background_color_row.clone().upcast(),
                background_color2_row.clone().upcast(),
                corner_radius_row.clone().upcast(),
                font_row.clone().upcast(),
                color_row.clone().upcast(),
                letter_spacing_row.clone().upcast(),
                line_spacing_row.clone().upcast(),
                opacity_row.clone().upcast(),
                shadow_row.clone().upcast(),
            ] {
                widget.set_sensitive(enabled);
            }
            apply();
        }
    ));
    content_row.connect_changed(glib::clone!(#[strong] apply, move |_| apply()));
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
    x_row.connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));
    y_row.connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));
    horizontal_row.connect_selected_notify(glib::clone!(#[strong] apply, move |_| apply()));
    vertical_row.connect_selected_notify(glib::clone!(#[strong] apply, move |_| apply()));
    padding_row.connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));
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
    corner_radius_row.connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));
    font_button.connect_font_desc_notify(glib::clone!(#[strong] apply, move |_| apply()));
    color_button.connect_rgba_notify(glib::clone!(#[strong] apply, move |_| apply()));
    letter_spacing_row.connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));
    line_spacing_row.connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));
    opacity_row.connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));

    shadow_row.connect_selected_notify(glib::clone!(
        #[strong]
        state,
        #[strong]
        syncing,
        #[weak]
        shadow_angle_row,
        #[weak]
        shadow_distance_row,
        #[weak]
        shadow_blur_row,
        #[weak]
        window,
        #[weak]
        canvas,
        move |row| {
            let preset = shadow_preset_for_index(row.selected());
            let mut state_ref = state.borrow_mut();
            let Some(current) = state_ref.document.elements.iter().find(|e| e.id == element_id) else { return };
            let new_shadow = current.label.shadow.with_preset(preset);

            syncing.set(true);
            shadow_distance_row.set_value(new_shadow.angle_and_distance().1);
            shadow_blur_row.set_value(new_shadow.blur);
            shadow_angle_row.set_sensitive(new_shadow.enabled);
            shadow_distance_row.set_sensitive(new_shadow.enabled);
            shadow_blur_row.set_sensitive(new_shadow.enabled);
            syncing.set(false);

            let mut new_label = current.label.clone();
            new_label.shadow = new_shadow;
            let old_label = current.label.clone();
            if old_label == new_label {
                return;
            }
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetScreenshotLabel { element_id, old: old_label, new: new_label }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));

    let apply_shadow_geometry = glib::clone!(
        #[strong]
        state,
        #[strong]
        syncing,
        #[weak]
        shadow_angle_row,
        #[weak]
        shadow_distance_row,
        #[weak]
        shadow_blur_row,
        #[weak]
        window,
        #[weak]
        canvas,
        move || {
            if syncing.get() {
                return;
            }
            let mut state_ref = state.borrow_mut();
            let Some(current) = state_ref.document.elements.iter().find(|e| e.id == element_id) else { return };
            let (offset_x, offset_y) = ShadowParams::offset_for_angle_and_distance(shadow_angle_row.value(), shadow_distance_row.value());
            let mut new_label = current.label.clone();
            new_label.shadow.offset_x = offset_x;
            new_label.shadow.offset_y = offset_y;
            new_label.shadow.blur = shadow_blur_row.value();
            let old_label = current.label.clone();
            if old_label == new_label {
                return;
            }
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetScreenshotLabel { element_id, old: old_label, new: new_label }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    );
    shadow_angle_row.connect_value_notify(glib::clone!(#[strong] apply_shadow_geometry, move |_| apply_shadow_geometry()));
    shadow_distance_row.connect_value_notify(glib::clone!(#[strong] apply_shadow_geometry, move |_| apply_shadow_geometry()));
    shadow_blur_row.connect_value_notify(glib::clone!(#[strong] apply_shadow_geometry, move |_| apply_shadow_geometry()));

    dialog.present(Some(window));
}

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

    let edit_label_action = gio::SimpleAction::new("edit-screenshot-label", None);
    edit_label_action.connect_activate(glib::clone!(
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
            let Some(element_id) = state.borrow().document.elements.get(index).map(|e| e.id) else { return };
            open_label_editor(&window, &canvas, &state, element_id);
        }
    ));
    window.add_action(&edit_label_action);
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
