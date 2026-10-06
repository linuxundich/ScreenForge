use crate::*;

/// Decodes every path and appends the successful ones to `state` as one
/// undoable [`AddScreenshots`] command, then refreshes the canvas once (not
/// per file). Used by both the file-open action and drag-and-drop, so the
/// two import paths can't drift apart.
pub(crate) fn import_paths(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>, paths: Vec<PathBuf>) {
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

pub(crate) fn register_open_action(app: &adw::Application, window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
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
pub(crate) fn register_import_android_action(app: &adw::Application, window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
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
pub(crate) const ADB_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Starts one off-thread ADB check and, on completion, updates
/// `android_import_button` — but only if the resulting state actually
/// differs from `last_state`, so a device that's already connected costs
/// nothing beyond the `adb devices` call itself on every subsequent tick.
/// A plain fn (not a closure) so `register_adb_watch` below can call it
/// both immediately and from its repeating timer without fighting the
/// borrow checker over which one owns it.
pub(crate) fn check_adb_state_once(window: &Window, last_state: &Rc<RefCell<Option<adb::AdbDeviceState>>>) {
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
pub(crate) fn register_adb_watch(window: &Window) {
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
pub(crate) fn register_drop_target(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
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

/// `win.paste` (`Ctrl+V`, spec §1: "Screenshot aus der Zwischenablage
/// einfügen"). Silently does nothing if the clipboard holds no image —
/// pasting text or nothing is not an error condition here.
pub(crate) fn register_paste_action(app: &adw::Application, window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
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
