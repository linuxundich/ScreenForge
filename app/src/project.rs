use crate::*;

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

/// Puts `doc` into the editor in place of the current document: decodes
/// its images, starts a fresh undo history and refreshes every control.
/// Returns how many images could not be found.
pub(crate) fn install_document(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>, doc: Document) -> u32 {
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
        // Undoing past a load into the previous document would be surprising.
        state_ref.undo_stack = UndoStack::new();
    }
    refresh_canvas(window, canvas, state);
    sync_controls_from_document(window, canvas, state);
    update_undo_redo_sensitivity(window, state);
    missing
}
