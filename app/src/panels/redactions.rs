//! "Schwärzen" in the style tab: hide areas of the selected screenshot
//! (an e-mail address, a token) as a dark bar, pixel blocks or a blur.
//! Each area is an expander row with its style and position in percent of
//! the screenshot; changes are undoable via `SetRedactions`.

use crate::*;

const STYLES: [(RedactionStyle, &str); 3] =
    [(RedactionStyle::Blackout, N_("Black Bar")), (RedactionStyle::Pixelate, N_("Pixelate")), (RedactionStyle::Blur, N_("Blur"))];

/// The widgets this panel owns, kept so selection changes and undo can
/// rebuild the list.
pub(crate) struct RedactionsPanel {
    group: adw::PreferencesGroup,
    list: gtk4::ListBox,
}

fn style_label(style: RedactionStyle) -> String {
    STYLES.iter().find(|s| s.0 == style).map(|s| gettext(s.1)).unwrap_or_default()
}

fn redactions_of(state: &Rc<RefCell<EditorState>>, element_id: Uuid) -> Vec<Redaction> {
    state.borrow().document.elements.iter().find(|e| e.id == element_id).map(|e| e.redactions.clone()).unwrap_or_default()
}

fn set_redactions(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>, element_id: Uuid, new: Vec<Redaction>) {
    {
        let mut state_ref = state.borrow_mut();
        if state_ref.syncing_controls {
            return;
        }
        let old = state_ref.document.elements.iter().find(|e| e.id == element_id).map(|e| e.redactions.clone()).unwrap_or_default();
        if old == new {
            return;
        }
        let EditorState { document, undo_stack, .. } = &mut *state_ref;
        undo_stack.apply(Box::new(SetRedactions { element_id, old, new }), document);
    }
    refresh_canvas(window, canvas, state);
    update_undo_redo_sensitivity(window, state);
}

fn percent_row(title: &str, value: f64) -> adw::SpinRow {
    let row = adw::SpinRow::with_range(0.0, 100.0, 1.0);
    row.set_title(title);
    row.set_digits(1);
    row.set_value(value * 100.0);
    row
}

/// One expander row per redaction. Edits replace just that redaction in
/// the element's list (looked up by id at edit time).
fn build_row(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>, panel: &Rc<RedactionsPanel>, element_id: Uuid, index: usize, r: &Redaction) -> adw::ExpanderRow {
    let row = adw::ExpanderRow::builder().title(gettext("Area {number}").replace("{number}", &(index + 1).to_string())).subtitle(style_label(r.style)).build();
    let delete = gtk4::Button::builder().icon_name("user-trash-symbolic").tooltip_text(gettext("Remove Area")).valign(gtk4::Align::Center).css_classes(["flat"]).build();
    row.add_suffix(&delete);

    let style_row = adw::ComboRow::builder().title(gettext("Type")).model(&gtk4::StringList::new(&STYLES.iter().map(|s| gettext(s.1)).collect::<Vec<_>>().iter().map(String::as_str).collect::<Vec<_>>())).build();
    style_row.set_selected(STYLES.iter().position(|s| s.0 == r.style).unwrap_or(0) as u32);
    let x = percent_row(&gettext("Left"), r.x);
    let y = percent_row(&gettext("Top"), r.y);
    let w = percent_row(&gettext("Width"), r.width);
    let h = percent_row(&gettext("Height"), r.height);
    row.add_row(&style_row);
    for spin in [&x, &y, &w, &h] {
        row.add_row(spin);
    }

    let id = r.id;
    let apply = Rc::new(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        #[weak]
        row,
        #[weak]
        style_row,
        #[weak]
        x,
        #[weak]
        y,
        #[weak]
        w,
        #[weak]
        h,
        move || {
            let style = STYLES.get(style_row.selected() as usize).map(|s| s.0).unwrap_or_default();
            row.set_subtitle(&style_label(style));
            let mut list = redactions_of(&state, element_id);
            if let Some(entry) = list.iter_mut().find(|e| e.id == id) {
                *entry = Redaction { id, x: x.value() / 100.0, y: y.value() / 100.0, width: w.value() / 100.0, height: h.value() / 100.0, style };
            }
            set_redactions(&window, &canvas, &state, element_id, list);
        }
    ));
    style_row.connect_selected_notify(glib::clone!(#[strong] apply, move |_| apply()));
    for spin in [&x, &y, &w, &h] {
        spin.connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));
    }
    delete.connect_clicked(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        #[strong]
        panel,
        move |_| {
            let list: Vec<Redaction> = redactions_of(&state, element_id).into_iter().filter(|e| e.id != id).collect();
            set_redactions(&window, &canvas, &state, element_id, list);
            sync_redactions(&window, &canvas, &state, &panel);
        }
    ));
    row
}

/// Rebuilds the list for the single selected screenshot, or hides the
/// group when none or several are selected.
pub(crate) fn sync_redactions(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>, panel: &Rc<RedactionsPanel>) {
    while let Some(child) = panel.list.first_child() {
        panel.list.remove(&child);
    }
    let Some(element_id) = single_selected_screenshot_id(canvas, state) else {
        panel.group.set_visible(false);
        return;
    };
    panel.group.set_visible(true);
    let list = redactions_of(state, element_id);
    panel.list.set_visible(!list.is_empty());
    let was = state.borrow().syncing_controls;
    state.borrow_mut().syncing_controls = true;
    for (i, r) in list.iter().enumerate() {
        panel.list.append(&build_row(window, canvas, state, panel, element_id, i, r));
    }
    state.borrow_mut().syncing_controls = was;
}

pub(crate) fn register_redaction_controls(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let list = gtk4::ListBox::builder().selection_mode(gtk4::SelectionMode::None).css_classes(["boxed-list"]).visible(false).build();
    let add = adw::ButtonRow::builder().title(gettext("Add Area")).start_icon_name("list-add-symbolic").build();
    let draw = adw::ButtonRow::builder().title(gettext("Draw on Screenshot")).start_icon_name("edit-select-all-symbolic").build();
    let add_list = gtk4::ListBox::builder().selection_mode(gtk4::SelectionMode::None).css_classes(["boxed-list"]).build();
    add_list.append(&draw);
    add_list.append(&add);
    let content = gtk4::Box::builder().orientation(gtk4::Orientation::Vertical).spacing(12).build();
    content.append(&list);
    content.append(&add_list);
    let group = adw::PreferencesGroup::builder()
        .title(gettext("Redact"))
        .description(gettext("Make areas of the selected screenshot unreadable, e.g. e-mail addresses"))
        .visible(false)
        .build();
    group.add(&content);
    window.style_page().add(&group);

    let panel = Rc::new(RedactionsPanel { group, list });
    add.connect_activated(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        #[strong]
        panel,
        move |_| {
            let Some(element_id) = single_selected_screenshot_id(&canvas, &state) else { return };
            let mut list = redactions_of(&state, element_id);
            list.push(Redaction::new(RedactionStyle::Blackout));
            set_redactions(&window, &canvas, &state, element_id, list);
            sync_redactions(&window, &canvas, &state, &panel);
            if let Some(last) = panel.list.last_child().and_downcast::<adw::ExpanderRow>() {
                last.set_expanded(true);
            }
        }
    ));
    draw.connect_activated(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        move |_| {
            canvas.set_redact_mode(true);
            window.toast_overlay().add_toast(adw::Toast::new(&gettext("Drag a rectangle over the area to hide")));
        }
    ));
    canvas.connect_redaction_drawn(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        #[strong]
        panel,
        move |element_id, x, y, width, height| {
            let mut list = redactions_of(&state, element_id);
            let style = list.last().map(|r| r.style).unwrap_or_default();
            list.push(Redaction { x, y, width, height, ..Redaction::new(style) });
            set_redactions(&window, &canvas, &state, element_id, list);
            sync_redactions(&window, &canvas, &state, &panel);
        }
    ));
    canvas.connect_redaction_changed(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        #[strong]
        panel,
        move |element_id, changed| {
            let mut list = redactions_of(&state, element_id);
            if let Some(r) = list.iter_mut().find(|r| r.id == changed.id) {
                *r = changed;
            }
            set_redactions(&window, &canvas, &state, element_id, list);
            sync_redactions(&window, &canvas, &state, &panel);
        }
    ));
    let on_selection: Rc<dyn Fn()> = Rc::new(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        #[strong]
        panel,
        move || sync_redactions(&window, &canvas, &state, &panel)
    ));
    let on_document: Rc<dyn Fn(&Document)> = {
        let on_selection = on_selection.clone();
        Rc::new(move |_: &Document| on_selection())
    };
    let mut state_ref = state.borrow_mut();
    state_ref.selection_syncs.push(on_selection);
    state_ref.document_syncs.push(on_document);
}
