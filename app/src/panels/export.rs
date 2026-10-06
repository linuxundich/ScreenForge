use crate::*;

pub(crate) fn export_format_for_index(index: u32) -> ExportFormat {
    match index {
        0 => ExportFormat::Png,
        1 => ExportFormat::Jpeg,
        2 => ExportFormat::WebP,
        3 => ExportFormat::Avif,
        4 => ExportFormat::Pdf,
        _ => ExportFormat::WebM,
    }
}

pub(crate) fn index_for_export_format(format: ExportFormat) -> u32 {
    match format {
        ExportFormat::Png => 0,
        ExportFormat::Jpeg => 1,
        ExportFormat::WebP => 2,
        ExportFormat::Avif => 3,
        ExportFormat::Pdf => 4,
        ExportFormat::WebM => 5,
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
    !matches!(format, ExportFormat::Png | ExportFormat::Pdf | ExportFormat::WebM)
}

pub(crate) fn extension_for_format(format: ExportFormat) -> &'static str {
    match format {
        ExportFormat::Png => "png",
        ExportFormat::Jpeg => "jpg",
        ExportFormat::WebP => "webp",
        ExportFormat::Avif => "avif",
        ExportFormat::Pdf => "pdf",
        ExportFormat::WebM => "webm",
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
                    .title(gettext("Export Composition"))
                    .accept_label(gettext("Export"))
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

                let (doc, decoded_images, background_image) = export_inputs(&state);
                let result =
                    gio::spawn_blocking(move || export::render_and_write(&doc, &decoded_images, background_image.as_ref(), &path))
                        .await;

                export_button.set_sensitive(true);
                export_button.set_label(&gettext("Export"));
                let toast = match result {
                    Ok(Ok(())) => adw::Toast::new(&gettext("Export successful")),
                    Ok(Err(err)) => adw::Toast::new(&gettext("Export failed: {err}").replace("{err}", &err.to_string())),
                    Err(_) => adw::Toast::new(&gettext("Export failed: background task was cancelled")),
                };
                toast_overlay.add_toast(toast);
            });
        }
    ));
    window.add_action(&export_action);
    app.set_accels_for_action("win.export", &["<Ctrl>e"]);
}

/// Everything a background-thread render needs, all `Send`: a copy of the
/// document, its decoded screenshots and the decoded background image.
pub(crate) fn export_inputs(
    state: &Rc<RefCell<EditorState>>,
) -> (Document, HashMap<Uuid, DecodedImage>, Option<DecodedImage>) {
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
    let background_image = background_image_path(&document.background).and_then(|path| get_or_decode(image_cache, &path)).cloned();
    (document.clone(), decoded_images, background_image)
}

/// `win.copy-image` (Ctrl+Shift+C): renders the export in the background
/// and puts it on the clipboard as an image.
pub(crate) fn register_copy_image_action(app: &adw::Application, window: &Window, state: &Rc<RefCell<EditorState>>) {
    let action = gio::SimpleAction::new("copy-image", None);
    action.connect_activate(glib::clone!(
        #[weak]
        window,
        #[strong]
        state,
        move |_, _| {
            if state.borrow().document.elements.is_empty() {
                return;
            }
            let (doc, decoded_images, background_image) = export_inputs(&state);
            let window = window.clone();
            glib::spawn_future_local(async move {
                let result = gio::spawn_blocking(move || export::render_pixels(&doc, &decoded_images, background_image.as_ref())).await;
                let toast = match result {
                    Ok(Ok((data, width, height, stride))) => {
                        let bytes = glib::Bytes::from_owned(data);
                        let texture = gdk::MemoryTexture::new(width, height, gdk::MemoryFormat::B8g8r8a8Premultiplied, &bytes, stride);
                        window.clipboard().set_texture(&texture);
                        adw::Toast::new(&gettext("Image copied to clipboard"))
                    }
                    Ok(Err(err)) => adw::Toast::new(&gettext("Copying failed: {err}").replace("{err}", &err.to_string())),
                    Err(_) => adw::Toast::new(&gettext("Copying failed: background task was cancelled")),
                };
                window.toast_overlay().add_toast(toast);
            });
        }
    ));
    window.add_action(&action);
    app.set_accels_for_action("win.copy-image", &["<Ctrl><Shift>c"]);
}

/// The floating toolbar's drag-out button: dragging it hands a freshly
/// rendered PNG file to wherever it's dropped (browser upload field, file
/// manager, chat). Rendering happens synchronously when the drag starts;
/// at export size that takes a fraction of a second.
pub(crate) fn register_drag_out(window: &Window, state: &Rc<RefCell<EditorState>>) {
    let source = gtk4::DragSource::new();
    source.set_actions(gdk::DragAction::COPY);
    source.connect_prepare(glib::clone!(
        #[strong]
        state,
        move |_, _, _| {
            if state.borrow().document.elements.is_empty() {
                return None;
            }
            let (doc, decoded_images, background_image) = export_inputs(&state);
            let dir = glib::user_cache_dir().join("screenforge").join("drag");
            std::fs::create_dir_all(&dir).ok()?;
            let name = state
                .borrow()
                .project_path
                .as_ref()
                .and_then(|p| p.file_stem())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "screenforge".to_owned());
            let path = dir.join(format!("{name}.png"));
            if let Err(err) = export::render_png(&doc, &decoded_images, background_image.as_ref(), &path) {
                eprintln!("ScreenForge: drag-out render failed: {err}");
                return None;
            }
            let files = gdk::FileList::from_array(&[gio::File::for_path(&path)]);
            Some(gdk::ContentProvider::for_value(&files.to_value()))
        }
    ));
    source.connect_drag_begin(|source, _| {
        source.set_icon(Some(&gtk4::IconTheme::default().lookup_icon("image-x-generic-symbolic", &[], 32, 1, gtk4::TextDirection::None, gtk4::IconLookupFlags::empty())), 16, 16);
    });
    window.drag_out_button().add_controller(source);
}
