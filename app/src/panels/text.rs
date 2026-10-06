use crate::*;

pub(crate) fn horizontal_anchor_for_index(index: u32) -> HorizontalAnchor {
    match index {
        0 => HorizontalAnchor::Left,
        2 => HorizontalAnchor::Right,
        _ => HorizontalAnchor::Center,
    }
}

pub(crate) fn index_for_horizontal_anchor(anchor: HorizontalAnchor) -> u32 {
    match anchor {
        HorizontalAnchor::Left => 0,
        HorizontalAnchor::Center => 1,
        HorizontalAnchor::Right => 2,
    }
}

pub(crate) fn vertical_anchor_for_index(index: u32) -> VerticalAnchor {
    match index {
        0 => VerticalAnchor::Top,
        2 => VerticalAnchor::Bottom,
        _ => VerticalAnchor::Center,
    }
}

pub(crate) fn index_for_vertical_anchor(anchor: VerticalAnchor) -> u32 {
    match anchor {
        VerticalAnchor::Top => 0,
        VerticalAnchor::Center => 1,
        VerticalAnchor::Bottom => 2,
    }
}

pub(crate) fn text_align_for_index(index: u32) -> TextAlign {
    match index {
        0 => TextAlign::Left,
        2 => TextAlign::Right,
        _ => TextAlign::Center,
    }
}

pub(crate) fn index_for_text_align(align: TextAlign) -> u32 {
    match align {
        TextAlign::Left => 0,
        TextAlign::Center => 1,
        TextAlign::Right => 2,
    }
}

/// Extracts family/size/weight/italic from a Pango font description — the
/// shape `Typography` stores them in, so a `GtkFontDialogButton` can be
/// this app's one control for all four (spec: "use the native GNOME text
/// stack"). Size is read as whatever raw number Pango carries (points from
/// the system font picker, or pixels if we set it via `set_absolute_size`
/// ourselves) without converting between the two — close enough at
/// typical desktop DPI for a screenshot compositor, and it means this app
/// never needs to know the display's actual DPI.
pub(crate) fn typography_from_font_desc(font_desc: &pango::FontDescription) -> (String, f64, i32, bool) {
    use glib::translate::IntoGlib;
    let family = font_desc.family().map(|f| f.to_string()).unwrap_or_else(|| "Sans".to_string());
    let size = (font_desc.size() as f64 / pango::SCALE as f64).max(1.0);
    let weight = font_desc.weight().into_glib();
    let italic = matches!(font_desc.style(), pango::Style::Italic | pango::Style::Oblique);
    (family, size, weight, italic)
}

pub(crate) fn font_desc_from_typography(typography: &Typography) -> pango::FontDescription {
    let mut font_desc = pango::FontDescription::new();
    font_desc.set_family(&typography.font_family);
    font_desc.set_absolute_size(typography.font_size.max(0.1) * pango::SCALE as f64);
    font_desc.set_weight(pango::Weight::__Unknown(typography.weight));
    font_desc.set_style(if typography.italic { pango::Style::Italic } else { pango::Style::Normal });
    font_desc
}

/// Reflects a `TextElement` (the composition title) onto its controls —
/// used for both the initial sync and after undo/redo/load, mirroring
/// `sync_background_controls`.
/// The screenshot a Label-sidebar edit should target: the document's own
/// current single-selected screenshot, or `None` when 0 or several are
/// selected (in which case the whole `label_group` stays hidden — see
/// `sync_label_controls`).
pub(crate) fn single_selected_label_target(canvas: &Canvas, state: &Rc<RefCell<EditorState>>) -> Option<(Uuid, Label)> {
    let selected = canvas.selected_ids();
    if selected.len() != 1 {
        return None;
    }
    let id = *selected.iter().next().unwrap();
    state.borrow().document.elements.iter().find(|e| e.id == id).map(|e| (id, e.label.clone()))
}

/// Reflects the current single-selected screenshot's label onto the
/// sidebar's "Label" section, hiding that whole section when 0 or several
/// screenshots are selected. Called both when the selection changes and
/// as part of `sync_controls_from_document` (after undo/redo/load, since
/// the selected screenshot's label may have changed underneath it too).
/// Guards its own writes with `EditorState.syncing_controls` so
/// `register_label_controls`'s row handlers don't reinterpret this as a
/// user edit and dispatch a spurious undo step.
pub(crate) fn sync_label_controls(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let Some((_, label)) = single_selected_label_target(canvas, state) else {
        window.label_group().set_visible(false);
        return;
    };
    window.label_group().set_visible(true);

    let was_syncing = state.borrow().syncing_controls;
    state.borrow_mut().syncing_controls = true;

    window.label_enabled_row().set_active(label.enabled);
    window.label_content_view().buffer().set_text(&label.content);
    window.label_content_view().set_sensitive(label.enabled);

    state.borrow_mut().syncing_controls = was_syncing;
}

/// Wires the selected screenshot's own Label sidebar controls: just
/// `enabled`/`content` — the only two things ever set per-label (spec:
/// "die Gestaltung soll für alle gleich sein, nur noch den Text möchte
/// ich pro Label setzen können"). Both funnel through one `apply_label`
/// that pushes a single `SetScreenshotLabel` undo step. The whole section
/// only targets a single-selected screenshot (see
/// `single_selected_label_target`); it's re-synced whenever the canvas
/// selection changes (`register_selection_sync`).
pub(crate) fn register_label_controls(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    sync_label_controls(window, canvas, state);

    // Only `enabled`/`content` are ever set per-label now — everything
    // else about how a label looks comes from the project's shared
    // `Document::label_defaults`, edited in its own sidebar section (see
    // `register_label_style_controls`), never here.
    let apply_label = glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move || {
            if state.borrow().syncing_controls {
                return;
            }
            let Some((element_id, current)) = single_selected_label_target(&canvas, &state) else { return };

            let buffer = window.label_content_view().buffer();
            let content = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string();
            let new = Label { enabled: window.label_enabled_row().is_active(), content };
            if current == new {
                return;
            }
            let mut state_ref = state.borrow_mut();
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetScreenshotLabel { element_id, old: current, new }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    );

    window.label_enabled_row().connect_active_notify(glib::clone!(
        #[weak]
        window,
        #[strong]
        apply_label,
        move |row| {
            window.label_content_view().set_sensitive(row.is_active());
            apply_label();
        }
    ));
    window.label_content_view().buffer().connect_changed(glib::clone!(
        #[strong]
        apply_label,
        move |_| apply_label()
    ));
}

/// Builds the "Schrift &amp; Farbe"/"Position"/"Gestaltung" groups shared
/// by both places a [`LabelStyle`] is edited — the current project's own
/// `Document::label_defaults`, live in the sidebar
/// (`register_label_style_controls`), and the app-wide default used only
/// to seed a *new* project, in the Settings dialog's "Allgemein" tab
/// (`build_global_label_defaults_groups`) — so the two don't duplicate
/// ~250 lines of near-identical widget construction. `get`/`commit` are
/// the only things that differ between the two: `get` reads whatever
/// `LabelStyle` is being edited (called once up front to populate every
/// row, and again fresh inside each row's own handler), and
/// `commit(old, new)` decides what "saving a change" means — an
/// undo-tracked `SetLabelDefaults` for the project-level case, a plain
/// `GSettings` write for the global case (which has no undo history to
/// speak of). `scope_description`, when set, becomes the first group's
/// description — used only by the global editor, to make clear at a
/// glance that it seeds *new* projects rather than affecting the one
/// currently open.
///
/// Returns the three groups plus a `sync` closure that repopulates every
/// row from a given [`LabelStyle`] — the sidebar's copy of these groups
/// lives for the whole session (unlike the Settings dialog's, which is
/// rebuilt fresh every time it's opened) and must reflect external
/// changes too: undo/redo, loading a different project, applying a
/// preset, or dragging a label's position on the canvas. `is_syncing`
/// must report whether such a resync is currently in progress — every
/// handler below checks it first and bails out, the same guard pattern
/// `EditorState::syncing_controls` provides everywhere else in this file
/// — otherwise `sync` calling e.g. `shadow_row.set_selected(..)` would
/// re-trigger that row's own change handler mid-resync and push a
/// spurious commit built from a half-updated state. The global editor has
/// no ongoing resync need (its dialog page is thrown away on close), so
/// it passes a closure that always returns `false`.
pub(crate) fn build_label_style_groups(
    get: Rc<dyn Fn() -> LabelStyle>,
    commit: Rc<dyn Fn(LabelStyle, LabelStyle)>,
    is_syncing: Rc<dyn Fn() -> bool>,
    scope_description: Option<&str>,
) -> (adw::PreferencesGroup, adw::PreferencesGroup, adw::PreferencesGroup, LabelStyleSync) {
    let defaults = get();

            let position_mode_row = adw::ComboRow::builder().title("Position").build();
            position_mode_row.set_model(Some(&gtk4::StringList::new(&["Automatisch", "Manuell (X/Y)"])));
            let horizontal_row = adw::ComboRow::builder().title("Horizontal").build();
            horizontal_row.set_model(Some(&gtk4::StringList::new(&["Links", "Mitte", "Rechts"])));
            let vertical_row = adw::ComboRow::builder().title("Vertikal").build();
            vertical_row.set_model(Some(&gtk4::StringList::new(&["Oben", "Mitte", "Unten"])));
            // Negative allowed — lets the shared default itself push every
            // label above/left of its screenshot's own edge.
            let padding_row = adw::SpinRow::with_range(-500.0, 500.0, 4.0);
            padding_row.set_title("Randabstand");
            padding_row.set_subtitle("Zum Screenshot-Rand, in Pixeln");
            let x_row = adw::SpinRow::with_range(-4000.0, 8000.0, 4.0);
            x_row.set_title("X-Position");
            let y_row = adw::SpinRow::with_range(-4000.0, 8000.0, 4.0);
            y_row.set_title("Y-Position");

            let background_row = adw::ComboRow::builder().title("Hintergrund").build();
            background_row.set_model(Some(&gtk4::StringList::new(&["Kein Hintergrund", "Einfarbig", "Verlauf"])));
            let background_color_row = adw::ActionRow::builder().title("Hintergrundfarbe").build();
            let background_color_button = gtk4::ColorDialogButton::new(Some(gtk4::ColorDialog::builder().with_alpha(true).build()));
            background_color_button.set_valign(gtk4::Align::Center);
            background_color_row.add_suffix(&background_color_button);
            let background_color2_row = adw::ActionRow::builder().title("Hintergrundfarbe 2").build();
            let background_color2_button = gtk4::ColorDialogButton::new(Some(gtk4::ColorDialog::builder().with_alpha(true).build()));
            background_color2_button.set_valign(gtk4::Align::Center);
            background_color2_row.add_suffix(&background_color2_button);

            let font_row = adw::ActionRow::builder().title("Schrift").build();
            let font_button = gtk4::FontDialogButton::new(Some(gtk4::FontDialog::new()));
            font_button.set_valign(gtk4::Align::Center);
            font_row.add_suffix(&font_button);
            let alignment_row = adw::ComboRow::builder().title("Textausrichtung").subtitle("Bei mehrzeiligem Text").build();
            alignment_row.set_model(Some(&gtk4::StringList::new(&["Links", "Mitte", "Rechts"])));
            let color_row = adw::ActionRow::builder().title("Textfarbe").build();
            let color_button = gtk4::ColorDialogButton::new(Some(gtk4::ColorDialog::new()));
            color_button.set_valign(gtk4::Align::Center);
            color_row.add_suffix(&color_button);
            let opacity_row = adw::SpinRow::with_range(0.0, 100.0, 5.0);
            opacity_row.set_title("Deckkraft");
            opacity_row.set_subtitle("In Prozent");

            let corner_radius_row = adw::SpinRow::with_range(0.0, 200.0, 2.0);
            corner_radius_row.set_title("Eckenradius");
            let padding_x_row = adw::SpinRow::with_range(0.0, 200.0, 2.0);
            padding_x_row.set_title("Innenabstand horizontal");
            let padding_y_row = adw::SpinRow::with_range(0.0, 200.0, 2.0);
            padding_y_row.set_title("Innenabstand vertikal");
            let wrap_row = adw::SwitchRow::builder().title("Automatisch umbrechen").build();
            let line_spacing_row = adw::SpinRow::with_range(0.5, 3.0, 0.1);
            line_spacing_row.set_title("Zeilenabstand");
            line_spacing_row.set_subtitle("Faktor der Schriftgröße, 1.0 = normal");
            line_spacing_row.set_digits(1);

            let shadow_row = adw::ComboRow::builder().title("Schatten").build();
            shadow_row.set_model(Some(&gtk4::StringList::new(&["Kein Schatten", "Subtil", "Standard", "Stark", "Floating", "Angepasst"])));
            let shadow_angle_row = adw::SpinRow::with_range(0.0, 360.0, 5.0);
            shadow_angle_row.set_title("Schatten-Winkel");
            let shadow_distance_row = adw::SpinRow::with_range(0.0, 300.0, 2.0);
            shadow_distance_row.set_title("Schatten-Distanz");
            let shadow_blur_row = adw::SpinRow::with_range(0.0, 150.0, 2.0);
            shadow_blur_row.set_title("Weichzeichner");

            // Populates every row from a given style — called once below
            // to seed the initial values (*before* any change handler is
            // attached, so that first call can never itself trigger a
            // spurious commit) and returned as `sync` for the sidebar's
            // ongoing use afterward.
            let populate: LabelStyleSync = {
                let position_mode_row = position_mode_row.clone();
                let horizontal_row = horizontal_row.clone();
                let vertical_row = vertical_row.clone();
                let padding_row = padding_row.clone();
                let x_row = x_row.clone();
                let y_row = y_row.clone();
                let background_row = background_row.clone();
                let background_color_row = background_color_row.clone();
                let background_color2_row = background_color2_row.clone();
                let background_color_button = background_color_button.clone();
                let background_color2_button = background_color2_button.clone();
                let font_button = font_button.clone();
                let alignment_row = alignment_row.clone();
                let color_button = color_button.clone();
                let opacity_row = opacity_row.clone();
                let corner_radius_row = corner_radius_row.clone();
                let padding_x_row = padding_x_row.clone();
                let padding_y_row = padding_y_row.clone();
                let wrap_row = wrap_row.clone();
                let line_spacing_row = line_spacing_row.clone();
                let shadow_row = shadow_row.clone();
                let shadow_angle_row = shadow_angle_row.clone();
                let shadow_distance_row = shadow_distance_row.clone();
                let shadow_blur_row = shadow_blur_row.clone();
                Rc::new(move |defaults: &LabelStyle| {
                    let is_absolute = matches!(defaults.position, TextPosition::Absolute { .. });
                    position_mode_row.set_selected(if is_absolute { 1 } else { 0 });
                    horizontal_row.set_visible(!is_absolute);
                    vertical_row.set_visible(!is_absolute);
                    padding_row.set_visible(!is_absolute);
                    x_row.set_visible(is_absolute);
                    y_row.set_visible(is_absolute);
                    match defaults.position {
                        TextPosition::Semantic { horizontal, vertical, padding } => {
                            horizontal_row.set_selected(index_for_horizontal_anchor(horizontal));
                            vertical_row.set_selected(index_for_vertical_anchor(vertical));
                            padding_row.set_value(padding);
                        }
                        TextPosition::Absolute { x, y } => {
                            x_row.set_value(x);
                            y_row.set_value(y);
                        }
                    }
                    let background_index = match &defaults.background {
                        TextBackground::None => 0,
                        TextBackground::Solid(_) => 1,
                        TextBackground::Gradient(_) => 2,
                    };
                    background_row.set_selected(background_index);
                    background_color_row.set_visible(background_index != 0);
                    background_color2_row.set_visible(background_index == 2);
                    match &defaults.background {
                        TextBackground::Solid(c) => background_color_button.set_rgba(&gdk_rgba_from(c)),
                        TextBackground::Gradient(spec) => {
                            if let Some((_, c)) = spec.stops.first() {
                                background_color_button.set_rgba(&gdk_rgba_from(c));
                            }
                            if let Some((_, c)) = spec.stops.get(1) {
                                background_color2_button.set_rgba(&gdk_rgba_from(c));
                            }
                        }
                        TextBackground::None => {}
                    }
                    font_button.set_font_desc(&font_desc_from_typography(&defaults.typography));
                    alignment_row.set_selected(index_for_text_align(defaults.typography.alignment));
                    color_button.set_rgba(&gdk_rgba_from(&defaults.typography.color));
                    opacity_row.set_value(defaults.typography.opacity * 100.0);
                    corner_radius_row.set_value(defaults.corner_radius.top_left);
                    padding_x_row.set_value(defaults.padding_x);
                    padding_y_row.set_value(defaults.padding_y);
                    wrap_row.set_active(defaults.typography.wrap);
                    line_spacing_row.set_value(defaults.typography.line_spacing);
                    shadow_row.set_selected(shadow_preset_index_for(&defaults.shadow));
                    let (angle, distance) = defaults.shadow.angle_and_distance();
                    shadow_angle_row.set_value(angle);
                    shadow_distance_row.set_value(distance);
                    shadow_blur_row.set_value(defaults.shadow.blur);
                    let shadow_geometry_enabled = defaults.shadow.enabled;
                    shadow_angle_row.set_sensitive(shadow_geometry_enabled);
                    shadow_distance_row.set_sensitive(shadow_geometry_enabled);
                    shadow_blur_row.set_sensitive(shadow_geometry_enabled);
                })
            };
            populate(&defaults);

            let apply = glib::clone!(
                #[strong]
                get,
                #[strong]
                commit,
                #[strong]
                is_syncing,
                #[weak]
                position_mode_row,
                #[weak]
                horizontal_row,
                #[weak]
                vertical_row,
                #[weak]
                padding_row,
                #[weak]
                x_row,
                #[weak]
                y_row,
                #[weak]
                background_row,
                #[weak]
                background_color_button,
                #[weak]
                background_color2_button,
                #[weak]
                font_button,
                #[weak]
                alignment_row,
                #[weak]
                color_button,
                #[weak]
                opacity_row,
                #[weak]
                corner_radius_row,
                #[weak]
                padding_x_row,
                #[weak]
                padding_y_row,
                #[weak]
                wrap_row,
                #[weak]
                line_spacing_row,
                move || {
                    if is_syncing() {
                        return;
                    }
                    let old = get();
                    let position = if position_mode_row.selected() == 1 {
                        TextPosition::Absolute { x: x_row.value(), y: y_row.value() }
                    } else {
                        TextPosition::Semantic {
                            horizontal: horizontal_anchor_for_index(horizontal_row.selected()),
                            vertical: vertical_anchor_for_index(vertical_row.selected()),
                            padding: padding_row.value(),
                        }
                    };
                    let background = match background_row.selected() {
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
                    let font_desc = font_button.font_desc().unwrap_or_else(pango::FontDescription::new);
                    let (font_family, font_size, weight, italic) = typography_from_font_desc(&font_desc);
                    // The shadow *preset*/geometry rows have their own
                    // dedicated handlers below (mirroring the per-label
                    // shadow split), so this closure carries the shadow
                    // over from whatever's already current.
                    let shadow = old.shadow;
                    let new = LabelStyle {
                        position,
                        typography: Typography {
                            font_family,
                            font_size,
                            weight,
                            italic,
                            color: rgba_from_gdk(&color_button.rgba()),
                            alignment: text_align_for_index(alignment_row.selected()),
                            opacity: opacity_row.value() / 100.0,
                            letter_spacing: old.typography.letter_spacing,
                            line_spacing: line_spacing_row.value(),
                            wrap: wrap_row.is_active(),
                        },
                        background,
                        corner_radius: CornerRadius::uniform(corner_radius_row.value()),
                        padding_x: padding_x_row.value(),
                        padding_y: padding_y_row.value(),
                        shadow,
                    };
                    if old == new {
                        return;
                    }
                    commit(old, new);
                }
            );

            position_mode_row.connect_selected_notify(glib::clone!(
                #[weak]
                horizontal_row,
                #[weak]
                vertical_row,
                #[weak]
                padding_row,
                #[weak]
                x_row,
                #[weak]
                y_row,
                #[strong]
                apply,
                move |row| {
                    let is_absolute = row.selected() == 1;
                    horizontal_row.set_visible(!is_absolute);
                    vertical_row.set_visible(!is_absolute);
                    padding_row.set_visible(!is_absolute);
                    x_row.set_visible(is_absolute);
                    y_row.set_visible(is_absolute);
                    apply();
                }
            ));
            horizontal_row.connect_selected_notify(glib::clone!(#[strong] apply, move |_| apply()));
            vertical_row.connect_selected_notify(glib::clone!(#[strong] apply, move |_| apply()));
            padding_row.connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));
            x_row.connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));
            y_row.connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));
            background_row.connect_selected_notify(glib::clone!(
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
            background_color_button.connect_rgba_notify(glib::clone!(#[strong] apply, move |_| apply()));
            background_color2_button.connect_rgba_notify(glib::clone!(#[strong] apply, move |_| apply()));
            font_button.connect_font_desc_notify(glib::clone!(#[strong] apply, move |_| apply()));
            alignment_row.connect_selected_notify(glib::clone!(#[strong] apply, move |_| apply()));
            color_button.connect_rgba_notify(glib::clone!(#[strong] apply, move |_| apply()));
            opacity_row.connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));
            corner_radius_row.connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));
            padding_x_row.connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));
            padding_y_row.connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));
            wrap_row.connect_active_notify(glib::clone!(#[strong] apply, move |_| apply()));
            line_spacing_row.connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));

            // Shadow preset/geometry: same preset-preserves-angle split as
            // every other shadow control in this app
            // (`ShadowParams::with_preset`), committed directly since
            // `apply` above deliberately doesn't touch the shadow.
            shadow_row.connect_selected_notify(glib::clone!(
                #[strong]
                get,
                #[strong]
                commit,
                #[strong]
                is_syncing,
                #[weak]
                shadow_angle_row,
                #[weak]
                shadow_distance_row,
                #[weak]
                shadow_blur_row,
                move |row| {
                    if is_syncing() {
                        return;
                    }
                    // "Angepasst" only ever reflects a shadow that doesn't
                    // match any real preset — selecting it has nothing
                    // coherent to apply.
                    if row.selected() == CUSTOM_SHADOW_PRESET_INDEX {
                        return;
                    }
                    let old = get();
                    let preset = shadow_preset_for_index(row.selected());
                    let new_shadow = old.shadow.with_preset(preset);
                    shadow_distance_row.set_value(new_shadow.angle_and_distance().1);
                    shadow_blur_row.set_value(new_shadow.blur);
                    shadow_angle_row.set_sensitive(new_shadow.enabled);
                    shadow_distance_row.set_sensitive(new_shadow.enabled);
                    shadow_blur_row.set_sensitive(new_shadow.enabled);
                    let mut new = old.clone();
                    new.shadow = new_shadow;
                    if old == new {
                        return;
                    }
                    commit(old, new);
                }
            ));
            let apply_shadow_geometry = glib::clone!(
                #[strong]
                get,
                #[strong]
                commit,
                #[strong]
                is_syncing,
                #[weak]
                shadow_row,
                #[weak]
                shadow_angle_row,
                #[weak]
                shadow_distance_row,
                #[weak]
                shadow_blur_row,
                move || {
                    if is_syncing() {
                        return;
                    }
                    let old = get();
                    let (offset_x, offset_y) =
                        ShadowParams::offset_for_angle_and_distance(shadow_angle_row.value(), shadow_distance_row.value());
                    let mut new = old.clone();
                    new.shadow.offset_x = offset_x;
                    new.shadow.offset_y = offset_y;
                    new.shadow.blur = shadow_blur_row.value();
                    if old == new {
                        return;
                    }
                    commit(old, new.clone());
                    // See the matching comment on the screenshot-level
                    // `apply_shadow_geometry` (`register_effect_controls`)
                    // for why the dropdown needs this explicit nudge —
                    // its own selection only updates on an actual pick.
                    shadow_row.set_selected(shadow_preset_index_for(&new.shadow));
                }
            );
            shadow_angle_row.connect_value_notify(glib::clone!(#[strong] apply_shadow_geometry, move |_| apply_shadow_geometry()));
            shadow_distance_row.connect_value_notify(glib::clone!(#[strong] apply_shadow_geometry, move |_| apply_shadow_geometry()));
            shadow_blur_row.connect_value_notify(glib::clone!(#[strong] apply_shadow_geometry, move |_| apply_shadow_geometry()));

            let position_group = adw::PreferencesGroup::new();
            position_group.set_title("Position");
            position_group.add(&position_mode_row);
            position_group.add(&horizontal_row);
            position_group.add(&vertical_row);
            position_group.add(&padding_row);
            position_group.add(&x_row);
            position_group.add(&y_row);

            let look_group = adw::PreferencesGroup::new();
            look_group.set_title("Schrift &amp; Farbe");
            if let Some(description) = scope_description {
                look_group.set_description(Some(description));
            }
            look_group.add(&font_row);
            look_group.add(&alignment_row);
            look_group.add(&color_row);
            look_group.add(&opacity_row);
            look_group.add(&background_row);
            look_group.add(&background_color_row);
            look_group.add(&background_color2_row);

            let styling_group = adw::PreferencesGroup::new();
            styling_group.set_title("Gestaltung");
            styling_group.add(&corner_radius_row);
            styling_group.add(&padding_x_row);
            styling_group.add(&padding_y_row);
            styling_group.add(&wrap_row);
            styling_group.add(&line_spacing_row);
            styling_group.add(&shadow_row);
            styling_group.add(&shadow_angle_row);
            styling_group.add(&shadow_distance_row);
            styling_group.add(&shadow_blur_row);

            (look_group, position_group, styling_group, populate)
}

/// The app-wide "default label style" groups (spec: "globale
/// Anwendungseinstellungen" — the top tier), appended onto the
/// "Allgemein" page (`build_general_page`) rather than living in their
/// own dialog page, since that's exactly the same category as the
/// spacing/margin/export-quality rows already there: values that seed a
/// *new* project and are otherwise inert. Writes straight to `GSettings`
/// (`save_global_label_defaults`) with no undo history of its own — an
/// app-wide setting isn't part of any document's undo stack. The dialog
/// page these groups live on is thrown away and rebuilt fresh every time
/// it's opened, so there's no ongoing resync need — `is_syncing` is a
/// constant `false`.
pub(crate) fn build_global_label_defaults_groups() -> (adw::PreferencesGroup, adw::PreferencesGroup, adw::PreferencesGroup) {
    let get: Rc<dyn Fn() -> LabelStyle> = Rc::new(load_global_label_defaults);
    let commit: Rc<dyn Fn(LabelStyle, LabelStyle)> = Rc::new(|_old, new| save_global_label_defaults(&new));
    let is_syncing: Rc<dyn Fn() -> bool> = Rc::new(|| false);
    let (look_group, position_group, styling_group, _sync) = build_label_style_groups(
        get,
        commit,
        is_syncing,
        Some("Ausgangswerte für neu erstellte Projekte — bereits bestehende Projekte bleiben davon unverändert"),
    );
    (look_group, position_group, styling_group)
}

/// The project-wide label style, live in the sidebar (spec: "im Projekt
/// sollen sich die Einstellungen anpassen lassen, für alle Screenshots
/// gemeinsam, nicht getrennt einzeln") — the middle tier of the
/// three-level configuration: seeded from the app-wide default when the
/// project is created (`EditorState::new`), freely editable here
/// afterward without ever reaching back into that global setting, and
/// captured into a preset alongside the project's other settings
/// (`Template::from_document`). Always visible regardless of selection,
/// like Layout/Hintergrund/Effekte — unlike the per-screenshot "Label"
/// section right above it (enabled/content only), since this is the one
/// shared style every label in the project uses.
///
/// Inserts the three groups right after the sidebar's per-screenshot
/// `label_group`, using `AdwPreferencesPage::remove`/`add` to reorder —
/// the only way to place them precisely, since `add` alone only ever
/// appends to the end of the page. Returns nothing; the `sync` closure
/// `build_label_style_groups` hands back is stored on `EditorState` so
/// `sync_controls_from_document` can keep these rows in step with
/// undo/redo, project load, preset apply, and canvas label-drags.
pub(crate) fn register_label_style_controls(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let get: Rc<dyn Fn() -> LabelStyle> = {
        let state = state.clone();
        Rc::new(move || state.borrow().document.label_defaults.clone())
    };
    let commit: Rc<dyn Fn(LabelStyle, LabelStyle)> = {
        let window = window.clone();
        let canvas = canvas.clone();
        let state = state.clone();
        Rc::new(move |old, new| {
            let mut state_ref = state.borrow_mut();
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetLabelDefaults { old, new }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        })
    };
    let is_syncing: Rc<dyn Fn() -> bool> = {
        let state = state.clone();
        Rc::new(move || state.borrow().syncing_controls)
    };
    let (look_group, position_group, styling_group, sync) = build_label_style_groups(
        get,
        commit,
        is_syncing,
        Some("Für alle Labels dieses Projekts gemeinsam — nicht einzeln pro Screenshot"),
    );

    let page = window.text_page();
    let callouts_group = window.callouts_group();
    page.remove(&callouts_group);
    page.add(&look_group);
    page.add(&position_group);
    page.add(&styling_group);
    page.add(&callouts_group);

    state.borrow_mut().label_style_sync = Some(sync);
}

/// Wires the canvas's own label-drag gesture to an undoable
/// `SetLabelDefaults` — every label in the project shares one position, so
/// dragging any one label's box moves them all identically. Works in every
/// layout mode, since a label's position is always relative to its own
/// screenshot regardless of how the screenshots themselves are arranged
/// (spec: label positioning is independent of the composition's
/// auto-layout).
pub(crate) fn register_label_drag(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    canvas.connect_label_move(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |new_position| {
            let mut state_ref = state.borrow_mut();
            let old = state_ref.document.label_defaults.clone();
            let new = LabelStyle { position: new_position, ..old.clone() };
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetLabelDefaults { old, new: new.clone() }), document);
            let sync = state_ref.label_style_sync.clone();
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            // The drag changed the sidebar's Position/X/Y rows from
            // outside the sidebar entirely, so they need an explicit
            // resync — nothing else triggers one for a canvas-originated
            // change (contrast the rows' own handlers, which are already
            // the source of truth for what they display).
            if let Some(sync) = sync {
                let was_syncing = state.borrow().syncing_controls;
                state.borrow_mut().syncing_controls = true;
                sync(&new);
                state.borrow_mut().syncing_controls = was_syncing;
            }
            update_undo_redo_sensitivity(&window, &state);
        }
    ));
}
