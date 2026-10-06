mod adb;
mod canvas;
mod export;
mod import;
mod window;

mod dialogs;
mod editor_model;
mod editing;
mod panels;
mod presets;
mod project;
mod sources;
mod state;

use dialogs::*;
use editor_model::*;
use editing::*;
use panels::*;
use presets::*;
use project::*;
use sources::*;
use state::*;

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
    register_css();
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
    register_variant_controls(&window, &canvas, &state);
    register_shortcuts_action(app);
    let model = state.borrow().model.clone();
    bind_editor_model(&window, &canvas, &model);
    register_text_focus_guards(&window, &model);
    refresh_canvas(&window, &canvas, &state);

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
    let toggle = gio::SimpleAction::new("toggle-sidebar", None);
    toggle.connect_activate(glib::clone!(
        #[weak]
        window,
        move |_, _| {
            let button = window.sidebar_toggle_button();
            button.set_active(!button.is_active());
        }
    ));
    window.add_action(&toggle);
    if let Some(app) = window.application() {
        app.set_accels_for_action("win.toggle-sidebar", &["F9"]);
    }
}

/// Small style additions on top of libadwaita: rounded floating canvas
/// toolbar and the variant thumbnails.
fn register_css() {
    let Some(display) = gdk::Display::default() else { return };
    let provider = gtk4::CssProvider::new();
    provider.load_from_string(
        ".canvas-toolbar { border-radius: 12px; padding: 4px; }
         .variant-thumb { padding: 0; border-radius: 8px; }
         .variant-picture { border-radius: 8px; }
         flowboxchild:selected .variant-picture { outline: 3px solid var(--accent-bg-color); outline-offset: 2px; }
         flowboxchild:selected { background: none; }",
    );
    gtk4::style_context_add_provider_for_display(&display, &provider, gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION);
}
