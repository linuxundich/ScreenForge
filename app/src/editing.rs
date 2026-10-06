use crate::*;

pub(crate) fn register_hide_screenshots_toggle(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
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

/// Arms the eyedropper (spec: "Farbpipette") for one pick: the canvas's
/// cursor becomes a crosshair, and the very next primary-button click on it
/// samples that pixel's rendered color (`Canvas::sample_color_at`) straight
/// into `target`'s `rgba`, exactly as if the user had picked it from
/// `target`'s own color dialog — so every existing `connect_rgba_notify`
/// wired to `target` elsewhere just fires normally, with no separate
/// "apply a picked color" path to keep in sync.
///
/// Implemented as one temporary, capture-phase `GtkGestureClick` added to
/// the canvas and removed again the moment it fires, rather than any
/// change to the canvas's own selection/drag/resize gesture — capture
/// phase runs before the canvas's own (unphased) click handling, and
/// claiming the sequence stops that normal handling from also seeing the
/// same click, so a pick behaves like a true modal tool without the canvas
/// needing to know picking exists.
pub(crate) fn start_color_picking(window: &Window, canvas: &Canvas, target: gtk4::ColorDialogButton) {
    canvas.set_cursor_from_name(Some("crosshair"));
    let gesture = gtk4::GestureClick::new();
    gesture.set_button(gdk::BUTTON_PRIMARY);
    gesture.set_propagation_phase(gtk4::PropagationPhase::Capture);
    gesture.connect_pressed(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        target,
        move |gesture, _n_press, x, y| {
            gesture.set_state(gtk4::EventSequenceState::Claimed);
            canvas.set_cursor_from_name(None);
            canvas.remove_controller(gesture);
            match canvas.sample_color_at(x, y) {
                Some(color) => target.set_rgba(&gdk_rgba_from(&color)),
                None => {
                    window.toast_overlay().add_toast(adw::Toast::new("An dieser Stelle wurde keine Farbe gefunden"));
                }
            }
        }
    ));
    canvas.add_controller(gesture);
}

/// Wires every color-target's small eyedropper button (spec: "Farbpipette")
/// to `start_color_picking`.
pub(crate) fn register_eyedroppers(window: &Window, canvas: &Canvas) {
    let pairs = [
        (window.background_eyedropper_button(), window.background_color_button()),
        (window.gradient_color2_eyedropper_button(), window.gradient_color2_button()),
        (window.generator_manual_eyedropper_button_1(), window.generator_manual_color_button_1()),
        (window.generator_manual_eyedropper_button_2(), window.generator_manual_color_button_2()),
        (window.generator_manual_eyedropper_button_3(), window.generator_manual_color_button_3()),
        (window.generator_manual_eyedropper_button_4(), window.generator_manual_color_button_4()),
    ];
    for (trigger, target) in pairs {
        trigger.connect_clicked(glib::clone!(
            #[weak]
            window,
            #[weak]
            canvas,
            move |_| start_color_picking(&window, &canvas, target.clone())
        ));
    }
}

/// Re-syncs the Label sidebar section whenever the canvas selection
/// changes (spec: selecting a screenshot shows/focuses its label
/// controls directly, no dialog) — and, once synced, focuses the text
/// entry so typing a label is a single click away.
pub(crate) fn register_selection_sync(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    canvas.connect_selection_changed(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move || {
            sync_label_controls(&window, &canvas, &state);
            sync_callouts_controls(&window, &canvas, &state);
            if single_selected_label_target(&canvas, &state).is_some() {
                window.label_content_view().grab_focus();
            }
        }
    ));
}

/// `win.undo`/`win.redo` (spec §17/§18: Ctrl+Z / Ctrl+Shift+Z). Both actions
/// start disabled (empty history) and are re-enabled/disabled by
/// [`update_undo_redo_sensitivity`] after every undoable mutation.
pub(crate) fn register_undo_redo_actions(app: &adw::Application, window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
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
            sync_controls_from_document(&window, &canvas, &state);
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
            sync_controls_from_document(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));
    window.add_action(&redo_action);
    app.set_accels_for_action("win.redo", &["<Ctrl><Shift>z"]);
}

pub(crate) const ZOOM_STEP: f64 = 1.25;
pub(crate) const ZOOM_MIN: f64 = 0.1;
pub(crate) const ZOOM_MAX: f64 = 8.0;

/// `win.zoom-fit`/`win.zoom-100`/`win.zoom-in`/`win.zoom-out` (spec §10).
/// Purely a canvas-widget display setting — not part of the document, so
/// not undoable and not persisted in the project file.
pub(crate) fn register_zoom_actions(app: &adw::Application, window: &Window, canvas: &Canvas) {
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
pub(crate) fn register_reorder(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
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
pub(crate) fn register_delete_selected(app: &adw::Application, window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
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

/// Disables `delete-selected`/`undo`/`redo`/`paste` whenever a text-input
/// widget has keyboard focus, re-enabling them the instant it doesn't.
///
/// These four are registered as global accelerators (`Delete`/`BackSpace`,
/// `<Ctrl>z`, `<Ctrl><Shift>z`, `<Ctrl>v`) via `app.set_accels_for_action`,
/// which GTK4 dispatches through a capture-phase shortcut controller on the
/// window — that wins the race against the focused widget's own key
/// handling. A no-op check *inside* an action's own activate callback
/// isn't enough to fix this: GTK already marks the key event as handled
/// once a matching, *enabled* action fires, regardless of what the
/// callback body does, so the keystroke would never reach the focused
/// entry either way. Disabling the `GSimpleAction` itself is what makes
/// GTK skip it and let the event fall through normally — e.g. Backspace
/// then deletes a character in the focused field instead of deleting the
/// selected screenshot (the bug this fixes), and Ctrl+Z reaches the
/// field's own built-in undo instead of the document's.
///
/// `gtk4::Text` is the internal widget every `AdwEntryRow`/`GtkEntry`/
/// `GtkSpinButton` entry focuses; `gtk4::TextView` covers the multi-line
/// label/callout text fields.
pub(crate) fn register_text_focus_guards(window: &Window) {
    let update = glib::clone!(
        #[weak]
        window,
        move || {
            let text_focused = gtk4::prelude::RootExt::focus(&window).is_some_and(|w| w.is::<gtk4::Text>() || w.is::<gtk4::TextView>());
            for name in ["delete-selected", "undo", "redo", "paste"] {
                if let Some(action) = window.lookup_action(name) {
                    if let Some(action) = action.downcast_ref::<gio::SimpleAction>() {
                        action.set_enabled(!text_focused);
                    }
                }
            }
        }
    );
    window.connect_notify_local(
        Some("focus-widget"),
        glib::clone!(
            #[strong]
            update,
            move |_, _| update()
        ),
    );
    update();
}

/// Wires the canvas's `LayoutMode::Free` move-drag to an undoable
/// [`SetTransforms`] (spec §8: manual positioning, extended by spec §5 to
/// move a multi-selection together as one undo step).
pub(crate) fn register_move(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
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
pub(crate) fn register_resize(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
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

pub(crate) fn build_context_menu() -> gio::Menu {
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
pub(crate) fn register_element_action<F>(
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
pub(crate) fn register_replace_action(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>, context_target: &Rc<Cell<Option<usize>>>) {
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
pub(crate) fn register_context_menu(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
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
