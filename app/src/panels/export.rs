use crate::*;

pub(crate) fn export_format_for_index(index: u32) -> ExportFormat {
    match index {
        0 => ExportFormat::Png,
        1 => ExportFormat::Jpeg,
        2 => ExportFormat::WebP,
        _ => ExportFormat::Avif,
    }
}

pub(crate) fn index_for_export_format(format: ExportFormat) -> u32 {
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
pub(crate) fn register_export_controls(window: &Window, state: &Rc<RefCell<EditorState>>) {
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
pub(crate) fn format_supports_quality(format: ExportFormat) -> bool {
    !matches!(format, ExportFormat::Png)
}

pub(crate) fn extension_for_format(format: ExportFormat) -> &'static str {
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
pub(crate) fn register_export_action(app: &adw::Application, window: &Window, state: &Rc<RefCell<EditorState>>) {
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
                export_button.set_child(Some(&adw::Spinner::new()));

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
                export_button.set_label("Exportieren");
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
