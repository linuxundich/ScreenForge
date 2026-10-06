use crate::*;

/// The "Allgemein" page: `GSettings`-backed defaults applied to every
/// *newly created* document (see `EditorState::new`) — changing one never
/// touches the document currently open, only what a fresh one starts
/// with. Each row binds straight to its `GSettings` key via
/// `Settings::bind`, so there's no manual load/save glue: GLib keeps the
/// setting and the widget in sync both ways for as long as the dialog is
/// open.
pub(crate) fn build_general_page() -> adw::PreferencesPage {
    let settings = app_settings();

    let spacing_row = adw::SpinRow::with_range(0.0, 500.0, 4.0);
    spacing_row.set_title(&gettext("Spacing"));
    spacing_row.set_subtitle(&gettext("Between the screenshots, in pixels"));
    settings.bind("default-spacing", &spacing_row, "value").build();

    let margin_x_row = adw::SpinRow::with_range(0.0, 500.0, 4.0);
    margin_x_row.set_title(&gettext("Horizontal Margin"));
    margin_x_row.set_subtitle(&gettext("Left/right around the composition, in pixels"));
    settings.bind("default-margin-x", &margin_x_row, "value").build();

    let margin_y_row = adw::SpinRow::with_range(0.0, 500.0, 4.0);
    margin_y_row.set_title(&gettext("Vertical Margin"));
    margin_y_row.set_subtitle(&gettext("Above/below the composition, in pixels"));
    settings.bind("default-margin-y", &margin_y_row, "value").build();

    let quality_row = adw::SpinRow::with_range(1.0, 100.0, 5.0);
    quality_row.set_title(&gettext("Export Quality"));
    quality_row.set_subtitle(&gettext("For JPEG/WebP/AVIF, in percent"));
    settings.bind("default-export-quality", &quality_row, "value").build();

    let group = adw::PreferencesGroup::new();
    group.set_title(&gettext("Defaults for New Projects"));
    group.add(&spacing_row);
    group.add(&margin_x_row);
    group.add(&margin_y_row);
    group.add(&quality_row);

    let demo_row = adw::SwitchRow::builder()
        .title(gettext("Clean Status Bar"))
        .subtitle(gettext("Android imports show 12:00, full battery, full signal and no notifications (demo mode)"))
        .build();
    settings.bind("adb-demo-mode", &demo_row, "active").build();
    let android_group = adw::PreferencesGroup::new();
    android_group.set_title(&gettext("Android"));
    android_group.add(&demo_row);

    let page = adw::PreferencesPage::new();
    page.set_title(&gettext("General"));
    page.set_icon_name(Some("preferences-system-symbolic"));
    page.add(&group);
    page.add(&android_group);

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
pub(crate) fn register_settings_action(app: &adw::Application) {
    let action = gio::SimpleAction::new("preferences", None);
    action.connect_activate(glib::clone!(
        #[weak]
        app,
        move |_, _| {
            let dialog = adw::PreferencesDialog::new();
            dialog.set_title(&gettext("Preferences"));
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
pub(crate) const RELEASE_NOTES: &str = "\
<p>Version 0.34.0</p>
<ul>
<li>Scenes: compositions save themselves with all their settings</li>
<li>Start screen with preview cards to reopen, rename, duplicate and delete scenes</li>
<li>Export and import scenes as .screenforge files</li>
</ul>
<p>Version 0.33.0</p>
<ul>
<li>Label types: capsule, caption and headline</li>
<li>Labels and callouts size themselves to the screenshot</li>
<li>Redesigned callouts with Light, Dark and Accent looks</li>
<li>Much smoother dragging of callouts, labels and screenshots</li>
</ul>
<p>Version 0.32.1</p>
<ul>
<li>New app icon: three phones in a forge fire</li>
</ul>
<p>Version 0.32.0</p>
<ul>
<li>Move and resize redaction areas directly on the screenshot</li>
</ul>
<p>Version 0.31.0</p>
<ul>
<li>Device frame and tilt for the selected screenshots only</li>
<li>Draw redactions directly on the screenshot</li>
<li>Logo image in the watermark</li>
<li>Export iPhone, iPad and Google Play sizes in one go</li>
<li>Choice of animation and tempo for WebM export</li>
</ul>
<p>Version 0.30.0</p>
<ul>
<li>Phone, tablet and browser frames, perspective tilt and fan-out</li>
<li>Panoramas split across several app store images, App Store size presets</li>
<li>Animated WebM export and a command line for rendering with presets</li>
<li>Preview images in the presets list</li>
</ul>
<p>Version 0.29.0</p>
<ul>
<li>English user interface with a complete German translation, following the system language</li>
<li>Installable with meson and as a Flatpak, with AppStream metadata</li>
</ul>
<p>Version 0.28.0</p>
<ul>
<li>Format presets (16:9, 1:1, Open Graph, Mastodon, Play Store …), transparent background, watermark, PDF export</li>
<li>Copy the image to the clipboard (Ctrl+Shift+C) or drag it out of the window</li>
<li>Redact, pixelate or blur areas of a screenshot</li>
<li>Blurred screenshot as background, even distribution in the free layout</li>
<li>Clean Android status bar on import, screenshots via the desktop portal, “Open with” from the file manager</li>
</ul>
<p>Version 0.27.0</p>
<ul>
<li>New interface: tabbed sidebar, tidier header bar, floating zoom bar</li>
<li>Variants for generated backgrounds and a background studio with large previews</li>
<li>Start page instead of an empty canvas, keyboard shortcuts overview (Ctrl+?)</li>
</ul>
<p>Version 0.26.0</p>
<ul>
<li>Six new styles for generated backgrounds: layers, arcs, ribbons, planes, lines and mist</li>
<li>New mood setting (vivid, light, dark) and film grain</li>
</ul>
";

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
pub(crate) fn register_about_action(app: &adw::Application) {
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
                .comments(gettext("Arranges smartphone screenshots into a single presentation image — a native GNOME app."))
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
            dialog.add_credit_section(Some(&gettext("Language")), &["Rust https://www.rust-lang.org"]);
            dialog.add_credit_section(
                Some(&gettext("GUI Framework")),
                &[
                    "GTK https://www.gtk.org",
                    "libadwaita https://gnome.pages.gitlab.gnome.org/libadwaita/",
                    &gettext("gtk4-rs / libadwaita-rs (Rust bindings) https://gtk-rs.org"),
                ],
            );
            dialog.add_credit_section(Some(&gettext("Graphics &amp; Rendering")), &["Cairo https://www.cairographics.org", "Pango https://pango.gnome.org"]);
            dialog.add_credit_section(
                Some(&gettext("Other Libraries")),
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
            dialog.add_legal_section(&gettext("gtk4-rs, libadwaita-rs (Rust bindings)"), None, gtk4::License::MitX11, None);
            dialog.add_legal_section(
                "serde, serde_json, thiserror, anyhow, uuid, image-rs, Rust",
                None,
                gtk4::License::Custom,
                Some(&gettext("Dual-licensed under MIT or Apache-2.0, at the copyright holder's option.")),
            );

            dialog.present(app.active_window().as_ref());
        }
    ));
    app.add_action(&action);
}

/// `app.shortcuts` (Ctrl+?): the shortcuts overview, built from the
/// accelerators the app actually registers.
pub(crate) fn register_shortcuts_action(app: &adw::Application) {
    let action = gio::SimpleAction::new("shortcuts", None);
    action.connect_activate(glib::clone!(
        #[weak]
        app,
        move |_, _| {
            let dialog = adw::ShortcutsDialog::new();
            let sections: [(&str, &[(&str, &str)]); 4] = [
                (
                    &gettext("Scenes"),
                    &[
                        (&gettext("New Scene"), "<Ctrl>n"),
                        (&gettext("Back to Scenes"), "<Alt>Left"),
                        (&gettext("Duplicate Scene"), "<Ctrl>d"),
                        (&gettext("Save Now"), "<Ctrl>s"),
                        (&gettext("Import File"), "<Ctrl>i"),
                        (&gettext("Export as File"), "<Ctrl><Shift>s"),
                    ],
                ),
                (
                    &gettext("General"),
                    &[
                        (&gettext("Open Images"), "<Ctrl>o"),
                        (&gettext("Paste from Clipboard"), "<Ctrl>v"),
                        (&gettext("Import from Android"), "<Ctrl><Shift>a"),
                        (&gettext("Export"), "<Ctrl>e"),
                        (&gettext("Preferences"), "<Ctrl>comma"),
                        (&gettext("Keyboard Shortcuts"), "<Ctrl>question"),
                    ],
                ),
                (
                    &gettext("Editing"),
                    &[(&gettext("Undo"), "<Ctrl>z"), (&gettext("Redo"), "<Ctrl><Shift>z"), (&gettext("Delete Selection"), "Delete")],
                ),
                (
                    &gettext("View"),
                    &[
                        (&gettext("Fit to Window"), "<Ctrl>0"),
                        (&gettext("Original Size"), "<Ctrl>1"),
                        (&gettext("Zoom In"), "<Ctrl>plus"),
                        (&gettext("Zoom Out"), "<Ctrl>minus"),
                        (&gettext("Toggle Sidebar"), "F9"),
                    ],
                ),
            ];
            for (title, items) in sections {
                let section = adw::ShortcutsSection::new(Some(title));
                for (label, accel) in items {
                    section.add(adw::ShortcutsItem::new(label, accel));
                }
                dialog.add(section);
            }
            dialog.present(app.active_window().as_ref());
        }
    ));
    app.add_action(&action);
    app.set_accels_for_action("app.shortcuts", &["<Ctrl>question"]);
}
