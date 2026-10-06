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
pub(crate) fn register_settings_action(app: &adw::Application) {
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
pub(crate) const RELEASE_NOTES: &str = "\
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
