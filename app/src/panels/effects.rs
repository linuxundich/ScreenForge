use crate::*;

/// The dropdown index reserved for "Angepasst" — a sixth, display-only
/// entry appended after the four real presets on every shadow dropdown in
/// the app (screenshot Effekte, project/global label style, callouts).
/// Never a valid *choice* to build a new shadow from — it only ever shows
/// up as the reflected state of a shadow that doesn't match any of the
/// four named presets (see `shadow_preset_index_for`), which happens any
/// time someone fine-tunes the angle/distance/blur rows directly rather
/// than picking a preset. Every shadow-dropdown handler in this file
/// treats selecting it as a no-op (nothing coherent to "apply"), rather
/// than silently falling back to some other preset's exact values.
pub(crate) const CUSTOM_SHADOW_PRESET_INDEX: u32 = 5;

pub(crate) fn shadow_preset_for_index(index: u32) -> ShadowPreset {
    match index {
        0 => ShadowPreset::NONE,
        1 => ShadowPreset::SUBTLE,
        2 => ShadowPreset::STANDARD,
        3 => ShadowPreset::STRONG,
        _ => ShadowPreset::FLOATING,
    }
}

/// The preset dropdown index matching `shadow`'s current distance/blur/
/// opacity/color — deliberately ignoring `angle_and_distance().0` (the
/// angle), so a shadow with a custom angle still shows its actual
/// Subtle/Standard/Strong/Floating preset instead of falling back to
/// "Angepasst" just because a plain `ShadowParams` equality check would
/// fail once the angle no longer matches the preset's own baked-in 90°.
/// Falls back to `CUSTOM_SHADOW_PRESET_INDEX` — never `0`/"Kein
/// Schatten" — for a shadow that doesn't match any of the four presets on
/// every other axis, so a hand-tuned (but very much enabled) shadow is
/// never mislabeled as no shadow at all.
pub(crate) fn shadow_preset_index_for(shadow: &ShadowParams) -> u32 {
    let (_, distance) = shadow.angle_and_distance();
    let presets = [ShadowPreset::NONE, ShadowPreset::SUBTLE, ShadowPreset::STANDARD, ShadowPreset::STRONG, ShadowPreset::FLOATING];
    presets
        .iter()
        .position(|p| {
            (p.distance - distance).abs() < 0.01
                && (p.blur - shadow.blur).abs() < 0.01
                && (p.opacity - shadow.opacity).abs() < 0.001
                && p.color == shadow.color
        })
        .map(|i| i as u32)
        .unwrap_or(CUSTOM_SHADOW_PRESET_INDEX)
}

/// Wires background (solid or linear gradient), shadow preset and
/// corner-radius controls through the undo stack. There's no per-element
/// selection yet (deferred, spec §5), so for the MVP shadow/corner-radius
/// apply uniformly to every screenshot — matching the example workflow in
/// spec §27, where one setting is applied to the whole composition.
pub(crate) fn register_effect_controls(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let background_type_row = window.background_type_row();
    let background_color_button = window.background_color_button();
    let gradient_color2_button = window.gradient_color2_button();
    let gradient_angle_row = window.gradient_angle_row();
    let shadow_row = window.shadow_row();
    let shadow_angle_row = window.shadow_angle_row();
    let shadow_distance_row = window.shadow_distance_row();
    let shadow_blur_row = window.shadow_blur_row();
    let corner_radius_row = window.corner_radius_row();

    {
        let state_ref = state.borrow();
        let shadow_geometry_enabled = state_ref.document.elements.first().is_some_and(|e| e.shadow.enabled);
        let (angle, distance) = state_ref.document.elements.first().map(|e| e.shadow.angle_and_distance()).unwrap_or((90.0, 6.0));
        let blur = state_ref.document.elements.first().map(|e| e.shadow.blur).unwrap_or(16.0);
        shadow_angle_row.set_value(angle);
        shadow_distance_row.set_value(distance);
        shadow_blur_row.set_value(blur);
        shadow_angle_row.set_sensitive(shadow_geometry_enabled);
        shadow_distance_row.set_sensitive(shadow_geometry_enabled);
        shadow_blur_row.set_sensitive(shadow_geometry_enabled);
    }

    sync_background_controls(window, &state.borrow().document.background);

    background_type_row.connect_selected_notify(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |row| {
            let selected = row.selected();
            let is_generated = selected == 4;
            // Selecting "Bild" only reveals the file picker — there's
            // nothing to render until a file is actually chosen (below).
            // Selecting "Generiert" needs an actual palette/seed resolved
            // from the current screenshots before there's anything to
            // render either, which `background_from_controls` (a synchronous,
            // state-free helper) has no way to do — so this goes through
            // `generate_background` instead, same as the "Generieren"
            // button itself.
            if is_generated {
                generate_background(&window, &canvas, &state);
            } else if selected != 3 {
                apply_background_from_controls(&window, &canvas, &state);
            }
        }
    ));
    background_color_button.connect_rgba_notify(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |_| apply_background_from_controls(&window, &canvas, &state)
    ));
    gradient_color2_button.connect_rgba_notify(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |_| apply_background_from_controls(&window, &canvas, &state)
    ));
    gradient_angle_row.connect_value_notify(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |_| apply_background_from_controls(&window, &canvas, &state)
    ));
    for row in [window.blurred_blur_row(), window.blurred_brightness_row()] {
        row.connect_value_notify(glib::clone!(
            #[weak]
            window,
            #[weak]
            canvas,
            #[strong]
            state,
            move |_| apply_background_from_controls(&window, &canvas, &state)
        ));
    }
    register_background_image_controls(window, canvas, state);
    register_gradient_auto_colors_control(window, canvas, state);

    shadow_row.connect_selected_notify(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |row| {
            // Bail out *before* touching any sibling widget — this handler
            // reenters whenever `sync_controls_from_document` (including
            // via a preset apply) sets this row's selection while a resync
            // is already in progress. Checking `syncing_controls` only
            // before the undo-push (as this used to) still let the writes
            // below run unconditionally, clobbering the sync's own
            // just-set, correct distance/blur/sensitivity with values
            // derived from `shadow_preset_for_index`'s lossy classification
            // — the Effekte section would then visibly show the wrong
            // shadow even though the document's own shadow was correct all
            // along.
            if state.borrow().syncing_controls {
                return;
            }
            // "Angepasst" is a display-only reflection of a shadow that
            // doesn't match any of the four real presets — never something
            // to actively apply (there's no single "the custom shadow" to
            // build from just this label), so selecting it is a no-op.
            if row.selected() == CUSTOM_SHADOW_PRESET_INDEX {
                return;
            }
            let preset = shadow_preset_for_index(row.selected());
            // `with_preset` only touches distance/blur/opacity/color and
            // keeps whichever angle the shadow already had — a preset is a
            // statement about how strong a shadow looks, not which
            // direction it's cast (spec: choosing Subtle/Standard/Strong
            // must never reset a user-chosen angle back to 90°).
            let mut state_ref = state.borrow_mut();
            let current = state_ref.document.elements.first().map(|e| e.shadow).unwrap_or_default();
            let new = current.with_preset(preset);

            // Guard these writes the same way `sync_controls_from_document`
            // guards its own batch: without it, each `set_value` below
            // reentrantly fires `apply_shadow_geometry`, which would push
            // its own spurious undo command built from a partially-updated
            // mix of old and new values.
            state_ref.syncing_controls = true;
            drop(state_ref);

            // Deliberately not touching shadow_angle_row's value — it
            // already shows the angle `with_preset` preserved.
            window.shadow_distance_row().set_value(new.angle_and_distance().1);
            window.shadow_blur_row().set_value(new.blur);
            window.shadow_angle_row().set_sensitive(new.enabled);
            window.shadow_distance_row().set_sensitive(new.enabled);
            window.shadow_blur_row().set_sensitive(new.enabled);

            let mut state_ref = state.borrow_mut();
            state_ref.syncing_controls = false;
            if state_ref.document.elements.iter().all(|e| e.shadow == new) {
                return;
            }
            let old: Vec<ShadowParams> = state_ref.document.elements.iter().map(|e| e.shadow).collect();
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetShadowForAllElements { old, new }), document);
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
        move || {
            let mut state_ref = state.borrow_mut();
            if state_ref.syncing_controls {
                return;
            }
            let angle = window.shadow_angle_row().value();
            let distance = window.shadow_distance_row().value();
            let blur = window.shadow_blur_row().value();
            let (offset_x, offset_y) = ShadowParams::offset_for_angle_and_distance(angle, distance);

            let Some(first) = state_ref.document.elements.first() else { return };
            let mut new = first.shadow;
            new.offset_x = offset_x;
            new.offset_y = offset_y;
            new.blur = blur;
            if state_ref.document.elements.iter().all(|e| e.shadow == new) {
                return;
            }
            let old: Vec<ShadowParams> = state_ref.document.elements.iter().map(|e| e.shadow).collect();
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetShadowForAllElements { old, new }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            // The geometry rows just changed the shadow out from under the
            // "Schatten" dropdown — its own selection only updates on an
            // actual dropdown pick, so without this it would keep showing
            // whatever preset (or "Angepasst") was true a moment ago,
            // silently going stale the instant someone hand-tunes the
            // angle/distance/blur directly.
            let was_syncing = state.borrow().syncing_controls;
            state.borrow_mut().syncing_controls = true;
            window.shadow_row().set_selected(shadow_preset_index_for(&new));
            state.borrow_mut().syncing_controls = was_syncing;
            update_undo_redo_sensitivity(&window, &state);
        }
    );
    shadow_angle_row.connect_value_notify(glib::clone!(
        #[strong]
        apply_shadow_geometry,
        move |_| apply_shadow_geometry()
    ));
    shadow_distance_row.connect_value_notify(glib::clone!(
        #[strong]
        apply_shadow_geometry,
        move |_| apply_shadow_geometry()
    ));
    shadow_blur_row.connect_value_notify(glib::clone!(
        #[strong]
        apply_shadow_geometry,
        move |_| apply_shadow_geometry()
    ));

    corner_radius_row.connect_value_notify(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |row| {
            let new = CornerRadius::uniform(row.value());
            let mut state_ref = state.borrow_mut();
            // See the background-color handler above for why this guard
            // against a reentrant sync-triggered no-op is needed.
            if state_ref.syncing_controls || state_ref.document.elements.iter().all(|e| e.corner_radius == new) {
                return;
            }
            let old: Vec<CornerRadius> = state_ref.document.elements.iter().map(|e| e.corner_radius).collect();
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetCornerRadiusForAllElements { old, new }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));

}
