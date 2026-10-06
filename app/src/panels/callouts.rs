use crate::*;

/// The single currently-selected screenshot's own id, or `None` when 0 or
/// several are selected — the same "exactly one" rule
/// `single_selected_label_target` uses for the Label section, reused here
/// for the Callouts section since both are per-screenshot.
pub(crate) fn single_selected_screenshot_id(canvas: &Canvas, state: &Rc<RefCell<EditorState>>) -> Option<Uuid> {
    let selected = canvas.selected_ids();
    if selected.len() != 1 {
        return None;
    }
    let id = *selected.iter().next().unwrap();
    state.borrow().document.elements.iter().any(|e| e.id == id).then_some(id)
}

/// Rebuilds the sidebar's "Callouts" section from scratch for the current
/// single-selected screenshot — hidden entirely when 0 or several are
/// selected, same as the Label section. Called on selection change and as
/// part of `sync_controls_from_document` (after undo/redo/load), and after
/// any action that adds/removes a callout, since unlike the Label section
/// (always exactly one, always present) the *number* of rows itself can
/// change.
pub(crate) fn sync_callouts_controls(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let Some(element_id) = single_selected_screenshot_id(canvas, state) else {
        window.callouts_group().set_visible(false);
        return;
    };
    window.callouts_group().set_visible(true);

    let callouts = state.borrow().document.elements.iter().find(|e| e.id == element_id).map(|e| e.callouts.clone()).unwrap_or_default();

    let list_box = window.callouts_list_box();
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }
    list_box.set_visible(!callouts.is_empty());
    for callout in &callouts {
        list_box.append(&build_callout_row(window, canvas, state, element_id, callout));
    }
}

/// Builds one fully-wired `AdwExpanderRow` for a single callout: enable
/// switch and delete button in the row's own header, text/colors/corner
/// radius/arrow styling nested inside — collapsed by default so several
/// callouts on one screenshot stay manageable, expanding to edit rather
/// than needing a separate dialog. Rebuilt from scratch by
/// `sync_callouts_controls` whenever the callout list itself changes, so
/// this only needs to wire live-editing, not incremental updates.
pub(crate) fn build_callout_row(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>, element_id: Uuid, callout: &Callout) -> adw::ExpanderRow {
    let callout_id = callout.id;

    let row = adw::ExpanderRow::new();
    let title_for = |content: &str| if content.trim().is_empty() { gettext("Callout") } else { content.to_string() };
    row.set_title(&title_for(&callout.text.content));

    let enabled_switch = gtk4::Switch::new();
    enabled_switch.set_active(callout.enabled);
    enabled_switch.set_valign(gtk4::Align::Center);
    row.add_suffix(&enabled_switch);

    let delete_button = gtk4::Button::from_icon_name("user-trash-symbolic");
    delete_button.set_valign(gtk4::Align::Center);
    delete_button.add_css_class("flat");
    delete_button.set_tooltip_text(Some(&gettext("Delete Callout")));
    row.add_suffix(&delete_button);

    let content_box = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    content_box.set_margin_top(12);
    content_box.set_margin_bottom(12);
    content_box.set_margin_start(12);
    content_box.set_margin_end(12);
    let content_label = gtk4::Label::new(Some(&gettext("Text")));
    content_label.set_halign(gtk4::Align::Start);
    content_label.add_css_class("caption-heading");
    content_box.append(&content_label);
    let content_scroller = gtk4::ScrolledWindow::new();
    content_scroller.set_hscrollbar_policy(gtk4::PolicyType::Never);
    content_scroller.set_min_content_height(64);
    content_scroller.set_max_content_height(120);
    content_scroller.add_css_class("card");
    let content_view = gtk4::TextView::new();
    content_view.set_wrap_mode(gtk4::WrapMode::WordChar);
    content_view.set_top_margin(8);
    content_view.set_bottom_margin(8);
    content_view.set_left_margin(8);
    content_view.set_right_margin(8);
    content_view.buffer().set_text(&callout.text.content);
    content_scroller.set_child(Some(&content_view));
    content_box.append(&content_scroller);
    row.add_row(&content_box);

    let wrap_row = adw::SwitchRow::new();
    wrap_row.set_title(&gettext("Wrap Automatically"));
    wrap_row.set_active(callout.text.typography.wrap);
    row.add_row(&wrap_row);

    let line_spacing_row = spin_row(&gettext("Line Spacing"), 0.5, 3.0, callout.text.typography.line_spacing);
    line_spacing_row.set_subtitle(&gettext("Factor of the font size, 1.0 = normal"));
    row.add_row(&line_spacing_row);

    let font_row = adw::ActionRow::new();
    font_row.set_title(&gettext("Font"));
    let font_button = gtk4::FontDialogButton::new(Some(gtk4::FontDialog::new()));
    font_button.set_valign(gtk4::Align::Center);
    font_button.set_font_desc(&font_desc_from_typography(&callout.text.typography));
    font_row.add_suffix(&font_button);
    row.add_row(&font_row);

    let alignment_row = adw::ComboRow::builder().title(gettext("Text Alignment")).subtitle(gettext("For text with several lines")).build();
    alignment_row.set_model(Some(&gtk4::StringList::new(&[&gettext("Left"), &gettext("Center"), &gettext("Right")])));
    alignment_row.set_selected(index_for_text_align(callout.text.typography.alignment));
    row.add_row(&alignment_row);

    let opacity_row = spin_row(&gettext("Opacity"), 0.0, 100.0, callout.text.typography.opacity * 100.0);
    opacity_row.set_subtitle(&gettext("In percent"));
    row.add_row(&opacity_row);

    let background_type_row = adw::ComboRow::builder().title(gettext("Background")).build();
    background_type_row.set_model(Some(&gtk4::StringList::new(&[&gettext("No Background"), &gettext("Solid Color"), &gettext("Gradient")])));
    let initial_background_index = match callout.text.background {
        TextBackground::None => 0,
        TextBackground::Solid(_) => 1,
        TextBackground::Gradient(_) => 2,
    };
    background_type_row.set_selected(initial_background_index);
    row.add_row(&background_type_row);

    let background_color_row = adw::ActionRow::new();
    background_color_row.set_title(&gettext("Background Color"));
    background_color_row.set_visible(initial_background_index != 0);
    let background_color_button = gtk4::ColorDialogButton::new(Some(gtk4::ColorDialog::builder().with_alpha(true).build()));
    background_color_button.set_valign(gtk4::Align::Center);
    let initial_background = match &callout.text.background {
        TextBackground::Solid(color) => *color,
        TextBackground::Gradient(spec) => spec.stops.first().map(|(_, c)| *c).unwrap_or(Rgba::WHITE),
        TextBackground::None => Rgba::WHITE,
    };
    background_color_button.set_rgba(&gdk_rgba_from(&initial_background));
    background_color_row.add_suffix(&background_color_button);
    row.add_row(&background_color_row);

    let background_color2_row = adw::ActionRow::new();
    background_color2_row.set_title(&gettext("Background Color 2"));
    background_color2_row.set_visible(initial_background_index == 2);
    let background_color2_button = gtk4::ColorDialogButton::new(Some(gtk4::ColorDialog::builder().with_alpha(true).build()));
    background_color2_button.set_valign(gtk4::Align::Center);
    if let TextBackground::Gradient(spec) = &callout.text.background {
        if let Some((_, c)) = spec.stops.get(1) {
            background_color2_button.set_rgba(&gdk_rgba_from(c));
        }
    }
    background_color2_row.add_suffix(&background_color2_button);
    row.add_row(&background_color2_row);

    let color_row = adw::ActionRow::new();
    color_row.set_title(&gettext("Text Color"));
    let color_button = gtk4::ColorDialogButton::new(Some(gtk4::ColorDialog::new()));
    color_button.set_valign(gtk4::Align::Center);
    color_button.set_rgba(&gdk_rgba_from(&callout.text.typography.color));
    color_row.add_suffix(&color_button);
    row.add_row(&color_row);

    let corner_radius_row = spin_row(&gettext("Corner Radius"), 0.0, 200.0, callout.text.corner_radius.top_left);
    row.add_row(&corner_radius_row);

    let padding_x_row = spin_row(&gettext("Horizontal Padding"), 0.0, 200.0, callout.text.padding_x);
    row.add_row(&padding_x_row);

    let padding_y_row = spin_row(&gettext("Vertical Padding"), 0.0, 200.0, callout.text.padding_y);
    row.add_row(&padding_y_row);

    let shadow_row = adw::ComboRow::builder().title(gettext("Shadow")).build();
    shadow_row.set_model(Some(&gtk4::StringList::new(&[&gettext("No Shadow"), &gettext("Subtle"), &gettext("Standard"), &gettext("Strong"), &gettext("Floating"), &gettext("Custom")])));
    shadow_row.set_selected(shadow_preset_index_for(&callout.text.shadow));
    row.add_row(&shadow_row);

    let (initial_shadow_angle, initial_shadow_distance) = callout.text.shadow.angle_and_distance();
    let shadow_angle_row = spin_row(&gettext("Shadow Angle"), 0.0, 360.0, initial_shadow_angle);
    shadow_angle_row.set_sensitive(callout.text.shadow.enabled);
    row.add_row(&shadow_angle_row);

    let shadow_distance_row = spin_row(&gettext("Shadow Distance"), 0.0, 300.0, initial_shadow_distance);
    shadow_distance_row.set_sensitive(callout.text.shadow.enabled);
    row.add_row(&shadow_distance_row);

    let shadow_blur_row = spin_row(&gettext("Softness"), 0.0, 150.0, callout.text.shadow.blur);
    shadow_blur_row.set_sensitive(callout.text.shadow.enabled);
    row.add_row(&shadow_blur_row);

    let arrow_color_row = adw::ActionRow::new();
    arrow_color_row.set_title(&gettext("Arrow Color"));
    let arrow_color_button = gtk4::ColorDialogButton::new(Some(gtk4::ColorDialog::builder().with_alpha(true).build()));
    arrow_color_button.set_valign(gtk4::Align::Center);
    arrow_color_button.set_rgba(&gdk_rgba_from(&callout.arrow_color));
    arrow_color_row.add_suffix(&arrow_color_button);
    row.add_row(&arrow_color_row);

    let arrow_width_row = spin_row(&gettext("Arrow Width"), 0.5, 20.0, callout.arrow_width);
    row.add_row(&arrow_width_row);

    let dot_radius_row = spin_row(&gettext("Dot Size"), 0.0, 20.0, callout.dot_radius);
    row.add_row(&dot_radius_row);

    let apply = glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        #[weak]
        enabled_switch,
        #[weak]
        content_view,
        #[weak]
        wrap_row,
        #[weak]
        line_spacing_row,
        #[weak]
        font_button,
        #[weak]
        alignment_row,
        #[weak]
        opacity_row,
        #[weak]
        background_type_row,
        #[weak]
        background_color_button,
        #[weak]
        background_color2_button,
        #[weak]
        color_button,
        #[weak]
        corner_radius_row,
        #[weak]
        padding_x_row,
        #[weak]
        padding_y_row,
        #[weak]
        arrow_color_button,
        #[weak]
        arrow_width_row,
        #[weak]
        dot_radius_row,
        move || {
            if state.borrow().syncing_controls {
                return;
            }
            let mut state_ref = state.borrow_mut();
            let Some(element) = state_ref.document.elements.iter().find(|e| e.id == element_id) else { return };
            let Some(current) = element.callouts.iter().find(|c| c.id == callout_id).cloned() else { return };

            let buffer = content_view.buffer();
            let mut new = current.clone();
            new.enabled = enabled_switch.is_active();
            new.text.content = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string();
            new.text.typography.wrap = wrap_row.is_active();
            new.text.typography.line_spacing = line_spacing_row.value();
            let font_desc = font_button.font_desc().unwrap_or_else(pango::FontDescription::new);
            let (font_family, font_size, weight, italic) = typography_from_font_desc(&font_desc);
            new.text.typography.font_family = font_family;
            new.text.typography.font_size = font_size;
            new.text.typography.weight = weight;
            new.text.typography.italic = italic;
            new.text.typography.alignment = text_align_for_index(alignment_row.selected());
            new.text.typography.opacity = opacity_row.value() / 100.0;
            new.text.background = match background_type_row.selected() {
                1 => TextBackground::Solid(rgba_from_gdk(&background_color_button.rgba())),
                2 => TextBackground::Gradient(GradientSpec {
                    kind: GradientKind::Linear { angle_deg: 135.0 },
                    stops: vec![
                        (0.0, rgba_from_gdk(&background_color_button.rgba())),
                        (1.0, rgba_from_gdk(&background_color2_button.rgba())),
                    ],
                }),
                _ => TextBackground::None,
            };
            new.text.typography.color = rgba_from_gdk(&color_button.rgba());
            new.text.corner_radius = CornerRadius::uniform(corner_radius_row.value());
            new.text.padding_x = padding_x_row.value();
            new.text.padding_y = padding_y_row.value();
            new.arrow_color = rgba_from_gdk(&arrow_color_button.rgba());
            new.arrow_width = arrow_width_row.value();
            new.dot_radius = dot_radius_row.value();
            if current == new {
                return;
            }
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetCallout { element_id, callout_id, old: current, new }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    );

    enabled_switch.connect_active_notify(glib::clone!(
        #[strong]
        apply,
        move |_| apply()
    ));
    content_view.buffer().connect_changed(glib::clone!(
        #[strong]
        apply,
        #[weak]
        row,
        move |buffer| {
            row.set_title(&title_for(&buffer.text(&buffer.start_iter(), &buffer.end_iter(), false)));
            apply();
        }
    ));
    wrap_row.connect_active_notify(glib::clone!(
        #[strong]
        apply,
        move |_| apply()
    ));
    line_spacing_row.connect_value_notify(glib::clone!(
        #[strong]
        apply,
        move |_| apply()
    ));
    font_button.connect_font_desc_notify(glib::clone!(
        #[strong]
        apply,
        move |_| apply()
    ));
    alignment_row.connect_selected_notify(glib::clone!(
        #[strong]
        apply,
        move |_| apply()
    ));
    opacity_row.connect_value_notify(glib::clone!(
        #[strong]
        apply,
        move |_| apply()
    ));
    background_type_row.connect_selected_notify(glib::clone!(
        #[weak]
        background_color_row,
        #[weak]
        background_color2_row,
        #[strong]
        apply,
        move |row| {
            let selected = row.selected();
            background_color_row.set_visible(selected != 0);
            background_color2_row.set_visible(selected == 2);
            apply();
        }
    ));
    background_color_button.connect_rgba_notify(glib::clone!(
        #[strong]
        apply,
        move |_| apply()
    ));
    background_color2_button.connect_rgba_notify(glib::clone!(
        #[strong]
        apply,
        move |_| apply()
    ));
    color_button.connect_rgba_notify(glib::clone!(
        #[strong]
        apply,
        move |_| apply()
    ));
    corner_radius_row.connect_value_notify(glib::clone!(
        #[strong]
        apply,
        move |_| apply()
    ));
    padding_x_row.connect_value_notify(glib::clone!(
        #[strong]
        apply,
        move |_| apply()
    ));
    padding_y_row.connect_value_notify(glib::clone!(
        #[strong]
        apply,
        move |_| apply()
    ));
    arrow_color_button.connect_rgba_notify(glib::clone!(
        #[strong]
        apply,
        move |_| apply()
    ));
    arrow_width_row.connect_value_notify(glib::clone!(
        #[strong]
        apply,
        move |_| apply()
    ));
    dot_radius_row.connect_value_notify(glib::clone!(
        #[strong]
        apply,
        move |_| apply()
    ));

    // Shadow preset/geometry: same preset-preserves-angle split as every
    // other shadow control in this app (`ShadowParams::with_preset`), and
    // the same `syncing_controls` guard as the screenshot-level version
    // in `register_effect_controls` — needed here too, since setting
    // `shadow_distance_row`/`shadow_blur_row` below reentrantly fires
    // `apply_shadow_geometry`, which needs its own borrow of `state`.
    shadow_row.connect_selected_notify(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        #[weak]
        shadow_angle_row,
        #[weak]
        shadow_distance_row,
        #[weak]
        shadow_blur_row,
        move |row| {
            // Bail out *before* touching any sibling widget — see the
            // matching comment on the screenshot-level `shadow_row`
            // handler (`register_effect_controls`) for why: this handler
            // reenters whenever `syncing_controls` is already set, and
            // the writes below must not run unconditionally in that case.
            if state.borrow().syncing_controls {
                return;
            }
            // "Angepasst" only ever reflects a shadow that doesn't match
            // any real preset — selecting it has nothing coherent to apply.
            if row.selected() == CUSTOM_SHADOW_PRESET_INDEX {
                return;
            }
            let mut state_ref = state.borrow_mut();
            let Some(element) = state_ref.document.elements.iter().find(|e| e.id == element_id) else { return };
            let Some(current) = element.callouts.iter().find(|c| c.id == callout_id).cloned() else { return };
            let preset = shadow_preset_for_index(row.selected());
            let new_shadow = current.text.shadow.with_preset(preset);

            // Guard these writes the same way the screenshot-level
            // handler does: `set_value` below reentrantly fires
            // `apply_shadow_geometry`, which needs its own borrow of
            // `state` — held open across these calls, `state_ref` here
            // would make that borrow panic instead of just no-op'ing.
            state_ref.syncing_controls = true;
            drop(state_ref);

            shadow_distance_row.set_value(new_shadow.angle_and_distance().1);
            shadow_blur_row.set_value(new_shadow.blur);
            shadow_angle_row.set_sensitive(new_shadow.enabled);
            shadow_distance_row.set_sensitive(new_shadow.enabled);
            shadow_blur_row.set_sensitive(new_shadow.enabled);

            let mut state_ref = state.borrow_mut();
            state_ref.syncing_controls = false;
            let Some(element) = state_ref.document.elements.iter().find(|e| e.id == element_id) else { return };
            let Some(current) = element.callouts.iter().find(|c| c.id == callout_id).cloned() else { return };
            let mut new = current.clone();
            new.text.shadow = new_shadow;
            if current == new {
                return;
            }
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetCallout { element_id, callout_id, old: current, new }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));

    let apply_shadow_geometry = glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        #[weak]
        shadow_row,
        #[weak]
        shadow_angle_row,
        #[weak]
        shadow_distance_row,
        #[weak]
        shadow_blur_row,
        move || {
            let mut state_ref = state.borrow_mut();
            if state_ref.syncing_controls {
                return;
            }
            let Some(element) = state_ref.document.elements.iter().find(|e| e.id == element_id) else { return };
            let Some(current) = element.callouts.iter().find(|c| c.id == callout_id).cloned() else { return };
            let (offset_x, offset_y) = ShadowParams::offset_for_angle_and_distance(shadow_angle_row.value(), shadow_distance_row.value());
            let mut new = current.clone();
            new.text.shadow.offset_x = offset_x;
            new.text.shadow.offset_y = offset_y;
            new.text.shadow.blur = shadow_blur_row.value();
            if current == new {
                return;
            }
            let new_shadow = new.text.shadow;
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetCallout { element_id, callout_id, old: current, new }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            // See the matching comment on the screenshot-level
            // `apply_shadow_geometry` (`register_effect_controls`) for why
            // the dropdown needs this explicit nudge — its own selection
            // only updates on an actual pick.
            shadow_row.set_selected(shadow_preset_index_for(&new_shadow));
            update_undo_redo_sensitivity(&window, &state);
        }
    );
    shadow_angle_row.connect_value_notify(glib::clone!(#[strong] apply_shadow_geometry, move |_| apply_shadow_geometry()));
    shadow_distance_row.connect_value_notify(glib::clone!(#[strong] apply_shadow_geometry, move |_| apply_shadow_geometry()));
    shadow_blur_row.connect_value_notify(glib::clone!(#[strong] apply_shadow_geometry, move |_| apply_shadow_geometry()));

    delete_button.connect_clicked(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |_| {
            let mut state_ref = state.borrow_mut();
            let Some(element) = state_ref.document.elements.iter().find(|e| e.id == element_id) else { return };
            let Some(index) = element.callouts.iter().position(|c| c.id == callout_id) else { return };
            let callout = element.callouts[index].clone();
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(RemoveCallout { element_id, index, callout }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            sync_callouts_controls(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));

    row
}

/// Adds a new callout to the current single-selected screenshot (spec:
/// "Callouts/Feature-Hinweise") and re-syncs the sidebar so it shows up
/// immediately, expanded state aside — a no-op if 0 or several screenshots
/// are selected, same as every other Label/Callout action.
pub(crate) fn add_callout_to_selected_screenshot(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let Some(element_id) = single_selected_screenshot_id(canvas, state) else { return };
    let mut state_ref = state.borrow_mut();
    let Some(natural_width) = state_ref.document.elements.iter().find(|e| e.id == element_id).map(|e| e.natural_width) else { return };
    let callout = Callout::new_for_width(natural_width);
    let EditorState { document, undo_stack, .. } = &mut *state_ref;
    undo_stack.apply(Box::new(AddCallout { element_id, callout }), document);
    drop(state_ref);
    refresh_canvas(window, canvas, state);
    sync_callouts_controls(window, canvas, state);
    update_undo_redo_sensitivity(window, state);
}

/// Wires the sidebar's Callouts section: the initial build for whatever is
/// selected when the app starts, and the "+ Callout hinzufügen" button.
pub(crate) fn register_callouts_controls(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    sync_callouts_controls(window, canvas, state);

    window.add_callout_button().connect_clicked(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |_| add_callout_to_selected_screenshot(&window, &canvas, &state)
    ));
}

/// Wires the canvas's own callout-drag gestures (text bubble and arrow
/// target, each independently draggable) to undoable `SetCallout`s —
/// mirrors `register_label_drag`; works in every layout mode.
pub(crate) fn register_callout_drag(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    canvas.connect_callout_box_move(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |element_id, callout_id, new_position| {
            let mut state_ref = state.borrow_mut();
            let Some(element) = state_ref.document.elements.iter().find(|e| e.id == element_id) else { return };
            let Some(old) = element.callouts.iter().find(|c| c.id == callout_id).cloned() else { return };
            let mut new = old.clone();
            new.text.position = new_position;
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetCallout { element_id, callout_id, old, new }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));

    canvas.connect_callout_target_move(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |element_id, callout_id, new_x, new_y| {
            let mut state_ref = state.borrow_mut();
            let Some(element) = state_ref.document.elements.iter().find(|e| e.id == element_id) else { return };
            let Some(old) = element.callouts.iter().find(|c| c.id == callout_id).cloned() else { return };
            let mut new = old.clone();
            new.target_x = new_x;
            new.target_y = new_y;
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetCallout { element_id, callout_id, old, new }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));
}
