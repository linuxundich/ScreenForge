use crate::*;

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
pub(crate) fn prepare_project_asset_extract_dir() -> PathBuf {
    let dir = glib::user_cache_dir().join("screenforge").join("project-assets");
    std::fs::remove_dir_all(&dir).ok();
    dir
}

pub(crate) fn save_project_to(window: &Window, state: &Rc<RefCell<EditorState>>, path: &std::path::Path) {
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
pub(crate) fn sync_controls_from_document(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    state.borrow_mut().syncing_controls = true;

    let doc = state.borrow().document.clone();

    window.layout_mode_toggle().set_active(index_for_layout_mode(doc.layout.mode));
    window.spacing_row().set_value(doc.layout.spacing_px);
    window.margin_x_row().set_value(doc.layout.margin_x);
    window.margin_y_row().set_value(doc.layout.margin_y);

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
    let syncs = state.borrow().document_syncs.clone();
    for sync in syncs {
        sync(&doc);
    }

    state.borrow_mut().syncing_controls = false;
}

/// `win.save`, `win.save-as` and `win.open-project` — the `.screenforge`
/// project file, distinct from `win.open`'s image import (spec §16/§18).
/// Missing source images on load are reported per-element via a toast, not
/// as a load failure (spec §4: local editing must keep working offline even
/// when referenced files have moved or been deleted).
pub(crate) fn register_project_actions(app: &adw::Application, window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
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

                open_project_file(&window, &canvas, &state, path);
            });
        }
    ));
    window.add_action(&open_project_action);
}

/// Loads the `.screenforge` project at `path` into the window, replacing
/// the current document and its undo history.
pub(crate) fn open_project_file(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>, path: PathBuf) {
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
            refresh_canvas(window, canvas, state);
            sync_controls_from_document(window, canvas, state);
            update_undo_redo_sensitivity(window, state);

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
}
