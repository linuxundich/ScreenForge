use crate::*;

/// Applies `preset` as one undoable step (spec: "Beim Anwenden eines
/// Presets müssen die darin gespeicherten Einstellungen vollständig
/// übernommen werden... Falls das Preset globale Label-Einstellungen
/// enthält, sollen diese anschließend als globale Standards verwendet
/// werden"). Called from the preset list's own row-activation handler in
/// `build_presets_page`.
pub(crate) fn apply_preset(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>, new: screenforge_core::template::Template) {
    {
        let mut state_ref = state.borrow_mut();
        let old_layout = state_ref.document.layout;
        let old_background = state_ref.document.background.clone();
        let old_shadows: Vec<ShadowParams> = state_ref.document.elements.iter().map(|e| e.shadow).collect();
        let old_corner_radii: Vec<CornerRadius> = state_ref.document.elements.iter().map(|e| e.corner_radius).collect();
        let old_label_defaults = state_ref.document.label_defaults.clone();
        let EditorState { document, undo_stack, .. } = &mut *state_ref;
        undo_stack.apply(
            Box::new(ApplyTemplate { old_layout, old_background, old_shadows, old_corner_radii, old_label_defaults, new }),
            document,
        );
    }
    refresh_canvas(window, canvas, state);
    sync_controls_from_document(window, canvas, state);
    update_undo_redo_sensitivity(window, state);
    window.toast_overlay().add_toast(adw::Toast::new(&gettext("Preset applied")));
}

/// Reads every saved preset from `GSettings` (spec: "Verwende dafür den
/// vorgesehenen persistenten Konfigurationsmechanismus des verwendeten
/// Frameworks" — no external files, no user-managed template directory).
/// An empty key (never saved anything yet) is treated as an empty list
/// rather than an error; a *non-empty but unreadable* value (corrupted, or
/// from an incompatible future version) is logged and also treated as
/// empty rather than losing the rest of the app to a panic — the user
/// loses their saved presets in that case, but keeps everything else.
pub(crate) fn load_presets() -> Vec<screenforge_core::template::NamedPreset> {
    let json = app_settings().string("presets");
    if json.is_empty() {
        return Vec::new();
    }
    match screenforge_core::template::deserialize_presets(&json) {
        Ok(presets) => presets,
        Err(err) => {
            eprintln!("ScreenForge: could not read saved presets: {err}");
            Vec::new()
        }
    }
}

pub(crate) fn save_presets(presets: &[screenforge_core::template::NamedPreset]) {
    let json = screenforge_core::template::serialize_presets(presets);
    if let Err(err) = app_settings().set_string("presets", &json) {
        eprintln!("ScreenForge: could not persist presets: {err}");
    }
}

/// Reads the app-wide default label style from `GSettings` — the top tier
/// of the three-level label configuration (see `EditorState::new`, the
/// only caller that matters for *new* documents; the "Allgemein" settings
/// page also reads/writes this directly to edit it). An empty or
/// unreadable value (never set yet, or from an incompatible future
/// version) falls back to `LabelStyle::default()` rather than erroring —
/// there's no "nothing configured yet" state worth distinguishing from
/// "using the built-in default" here, unlike presets (an empty preset
/// list and "no presets configured" are the same thing either way).
pub(crate) fn load_global_label_defaults() -> screenforge_core::model::LabelStyle {
    let json = app_settings().string("default-label-style");
    if json.is_empty() {
        return screenforge_core::model::LabelStyle::default();
    }
    match screenforge_core::template::deserialize_label_style(&json) {
        Ok(style) => style,
        Err(err) => {
            eprintln!("ScreenForge: could not read the default label style, falling back to the built-in one: {err}");
            screenforge_core::model::LabelStyle::default()
        }
    }
}

pub(crate) fn save_global_label_defaults(style: &screenforge_core::model::LabelStyle) {
    let json = screenforge_core::template::serialize_label_style(style);
    if let Err(err) = app_settings().set_string("default-label-style", &json) {
        eprintln!("ScreenForge: could not persist the default label style: {err}");
    }
}

/// A small named-text prompt (used for both "save as" and "rename") built
/// from an `AdwAlertDialog` with a single `AdwEntryRow` as its extra child —
/// `None` for Escape/Abbrechen or an empty name, `Some(name)` (trimmed)
/// otherwise.
async fn prompt_for_preset_name(window: &Window, heading: &str, initial: &str) -> Option<String> {
    let entry = adw::EntryRow::builder().title(gettext("Name")).text(initial).activates_default(true).build();
    let list = gtk4::ListBox::builder().selection_mode(gtk4::SelectionMode::None).css_classes(["boxed-list"]).build();
    list.append(&entry);

    let dialog = adw::AlertDialog::builder().heading(heading).extra_child(&list).build();
    dialog.connect_map(glib::clone!(
        #[weak]
        entry,
        move |_| {
            entry.grab_focus();
        }
    ));
    dialog.add_response("cancel", &gettext("Cancel"));
    dialog.add_response("save", &gettext("Save"));
    dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("save"));
    dialog.set_close_response("cancel");

    let response = dialog.choose_future(Some(window)).await;
    if response != "save" {
        return None;
    }
    let name = entry.text().trim().to_string();
    if name.is_empty() { None } else { Some(name) }
}

/// (Re)builds `list_box`'s rows from persisted storage — called once
/// whenever the presets popover is about to show, and again after every
/// save/rename/delete, so it always reflects what's actually saved
/// without needing to be closed and reopened. Each row's "Anwenden"/
/// rename/delete handler captures its preset's *index* into the
/// freshly-loaded list at click time (not the list this function built
/// the row from) — safe because every mutation immediately calls this
/// function again before the user can interact with anything else (the
/// save/rename prompt is itself a modal dialog, and delete's own handler
/// is synchronous), so no click can ever land against a stale index.
/// `popover` is popped down after a row's "Anwenden" activates a preset —
/// applying one reads as a complete action, the same way picking an entry
/// from any other GNOME popover menu dismisses it — but never after a
/// rename/delete, which are edits to the list the user stays in.
pub(crate) fn rebuild_preset_list(list_box: &gtk4::ListBox, window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>, popover: &gtk4::Popover) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }
    let presets = load_presets();
    if presets.is_empty() {
        let row = adw::ActionRow::builder().title(gettext("No presets saved yet")).sensitive(false).build();
        list_box.append(&row);
        return;
    }
    for (index, named) in presets.iter().enumerate() {
        let row = adw::ActionRow::builder().title(named.name.clone()).activatable(true).build();
        row.set_subtitle(&gettext("Click to apply"));

        let rename_button = gtk4::Button::from_icon_name("document-edit-symbolic");
        rename_button.set_valign(gtk4::Align::Center);
        rename_button.set_tooltip_text(Some(&gettext("Rename")));
        rename_button.add_css_class("flat");
        row.add_suffix(&rename_button);

        let delete_button = gtk4::Button::from_icon_name("user-trash-symbolic");
        delete_button.set_valign(gtk4::Align::Center);
        delete_button.set_tooltip_text(Some(&gettext("Delete")));
        delete_button.add_css_class("flat");
        row.add_suffix(&delete_button);

        row.connect_activated(glib::clone!(
            #[weak]
            window,
            #[weak]
            canvas,
            #[strong]
            state,
            #[weak]
            popover,
            move |_| {
                let presets = load_presets();
                if let Some(named) = presets.get(index) {
                    apply_preset(&window, &canvas, &state, named.template.clone());
                    popover.popdown();
                }
            }
        ));

        rename_button.connect_clicked(glib::clone!(
            #[weak]
            window,
            #[weak]
            canvas,
            #[strong]
            state,
            #[weak]
            list_box,
            #[weak]
            popover,
            move |_| {
                glib::spawn_future_local(glib::clone!(
                    #[weak]
                    window,
                    #[weak]
                    canvas,
                    #[strong]
                    state,
                    #[weak]
                    list_box,
                    #[weak]
                    popover,
                    async move {
                        let presets = load_presets();
                        let Some(current) = presets.get(index).cloned() else { return };
                        let Some(new_name) = prompt_for_preset_name(&window, &gettext("Rename Preset"), &current.name).await else { return };
                        let mut presets = load_presets();
                        if let Some(named) = presets.get_mut(index) {
                            named.name = new_name;
                        }
                        save_presets(&presets);
                        rebuild_preset_list(&list_box, &window, &canvas, &state, &popover);
                    }
                ));
            }
        ));

        delete_button.connect_clicked(glib::clone!(
            #[weak]
            window,
            #[weak]
            canvas,
            #[strong]
            state,
            #[weak]
            list_box,
            #[weak]
            popover,
            move |_| {
                let mut presets = load_presets();
                if index < presets.len() {
                    presets.remove(index);
                    save_presets(&presets);
                }
                rebuild_preset_list(&list_box, &window, &canvas, &state, &popover);
            }
        ));

        list_box.append(&row);
    }
}

/// The header bar's "Presets" menu (`presets_menu_button`): an in-app,
/// persisted replacement for the old external "Vorlage speichern/laden"
/// file actions (spec: "Es soll stattdessen ein internes Preset-System...
/// geben" — no export/import of settings files, no user-managed template
/// directory) — and, since this round, deliberately its *own* header-bar
/// entry point rather than a page inside the "Einstellungen" dialog, so
/// saving/restoring a preset never gets mistaken for touching the app-wide
/// settings on that dialog's other pages (see `register_settings_action`'s
/// own doc comment for why that mattered). A preset bundles the *current
/// project's* layout, background, shadow, corner radius, and label
/// defaults (see `screenforge_core::template::Template::from_document`) —
/// never any screenshot's own label content or callouts; applying one
/// never removes or changes those (see
/// `apply_preset`/`ApplyTemplate`'s own doc comment).
///
/// The popover is rebuilt fresh (`rebuild_preset_list`) every time it's
/// about to show, via `GtkPopover`'s own `show` signal, so it always
/// reflects whatever's actually saved without this needing to track
/// changes itself.
pub(crate) fn register_presets_menu(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    // Built before anything that references it (every row's "Anwenden"
    // pops it down; the save button and the popover's own `show` handler
    // both need to pass it to `rebuild_preset_list`), then given its real
    // child content only once everything else exists.
    let popover = gtk4::Popover::new();

    let list_box = gtk4::ListBox::new();
    list_box.set_selection_mode(gtk4::SelectionMode::None);
    list_box.add_css_class("boxed-list");

    let save_button = gtk4::Button::with_label(&gettext("Save Current Settings as Preset…"));
    save_button.add_css_class("suggested-action");
    save_button.connect_clicked(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        #[weak]
        list_box,
        #[weak]
        popover,
        move |_| {
            glib::spawn_future_local(glib::clone!(
                #[weak]
                window,
                #[weak]
                canvas,
                #[strong]
                state,
                #[weak]
                list_box,
                #[weak]
                popover,
                async move {
                    let Some(name) = prompt_for_preset_name(&window, &gettext("Save Preset"), "").await else { return };
                    let template = screenforge_core::template::Template::from_document(&state.borrow().document);
                    let mut presets = load_presets();
                    presets.push(screenforge_core::template::NamedPreset { name, template });
                    save_presets(&presets);
                    rebuild_preset_list(&list_box, &window, &canvas, &state, &popover);
                }
            ));
        }
    ));

    let hint = gtk4::Label::new(Some(&gettext("This project's layout, background, shadow, corner radius and label settings")));
    hint.set_wrap(true);
    hint.set_xalign(0.0);
    hint.add_css_class("caption");
    hint.add_css_class("dim-label");

    let scroller = gtk4::ScrolledWindow::builder().hscrollbar_policy(gtk4::PolicyType::Never).child(&list_box).build();
    scroller.set_max_content_height(360);
    scroller.set_propagate_natural_height(true);

    let content = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(12);
    content.set_margin_end(12);
    content.set_width_request(320);
    content.append(&save_button);
    content.append(&hint);
    content.append(&scroller);
    popover.set_child(Some(&content));

    popover.connect_show(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        #[weak]
        list_box,
        move |popover| rebuild_preset_list(&list_box, &window, &canvas, &state, popover)
    ));

    window.presets_menu_button().set_popover(Some(&popover));
}
