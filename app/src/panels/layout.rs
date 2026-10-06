use crate::*;

pub(crate) fn layout_mode_for_index(index: u32) -> LayoutMode {
    match index {
        0 => LayoutMode::Horizontal,
        1 => LayoutMode::Vertical,
        2 => LayoutMode::Grid,
        _ => LayoutMode::Free,
    }
}

pub(crate) fn index_for_layout_mode(mode: LayoutMode) -> u32 {
    match mode {
        LayoutMode::Horizontal => 0,
        LayoutMode::Vertical => 1,
        LayoutMode::Grid => 2,
        LayoutMode::Free => 3,
    }
}

/// The alignment tool only does anything in `LayoutMode::Free` — every
/// other mode recomputes each element's placement from spacing/margin on
/// every render, so touching `Transform.x`/`.y` there would have no visible
/// effect at all. Selection count isn't part of this check (see
/// `align_selected`'s own guard) since it can change without the layout
/// mode row itself firing.
/// Wires the sidebar's layout-mode/spacing/margin rows to `Document.layout`,
/// mutating it directly through the undo stack (spec §17: layout changes are
/// undoable).
pub(crate) fn register_layout_controls(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let layout_mode_toggle = window.layout_mode_toggle();
    let spacing_row = window.spacing_row();
    let margin_x_row = window.margin_x_row();
    let margin_y_row = window.margin_y_row();

    {
        let state_ref = state.borrow();
        layout_mode_toggle.set_active(index_for_layout_mode(state_ref.document.layout.mode));
        spacing_row.set_value(state_ref.document.layout.spacing_px);
        margin_x_row.set_value(state_ref.document.layout.margin_x);
        margin_y_row.set_value(state_ref.document.layout.margin_y);
    }

    layout_mode_toggle.connect_active_notify(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |row| {
            let new = layout_mode_for_index(row.active());
            let mut state_ref = state.borrow_mut();
            let old = state_ref.document.layout.mode;
            if state_ref.syncing_controls || old == new {
                return;
            }
            let command: Box<dyn Command> = if new == LayoutMode::Free {
                // Snapshot each visible element's placement under the old
                // mode as its new transform, so free positioning starts
                // from "wherever auto-layout had it" instead of everyone
                // collapsed onto Transform::default()'s (0, 0) origin.
                let doc = &state_ref.document;
                let visible: Vec<_> = doc.elements.iter().filter(|e| e.visible).cloned().collect();
                let placements = screenforge_core::layout::compute_layout(
                    old,
                    &visible,
                    doc.layout.spacing_px,
                    doc.layout.margin_x,
                    doc.layout.margin_y,
                );
                let transforms = visible
                    .iter()
                    .zip(placements.iter())
                    .map(|(el, placement)| {
                        let mut new_transform = el.transform;
                        new_transform.x = placement.x;
                        new_transform.y = placement.y;
                        new_transform.width = placement.width;
                        new_transform.height = placement.height;
                        (el.id, el.transform, new_transform)
                    })
                    .collect();
                Box::new(EnterFreeLayout { old_mode: old, transforms })
            } else {
                Box::new(SetLayoutMode { old, new })
            };
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(command, document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));

    spacing_row.connect_value_notify(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |row| {
            let new = row.value();
            let mut state_ref = state.borrow_mut();
            let old = state_ref.document.layout.spacing_px;
            if state_ref.syncing_controls || old == new {
                return;
            }
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetSpacing { old, new }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));
    margin_x_row.connect_value_notify(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |row| {
            let new = row.value();
            let mut state_ref = state.borrow_mut();
            let old = state_ref.document.layout.margin_x;
            if state_ref.syncing_controls || old == new {
                return;
            }
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetMarginX { old, new }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));
    margin_y_row.connect_value_notify(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |row| {
            let new = row.value();
            let mut state_ref = state.borrow_mut();
            let old = state_ref.document.layout.margin_y;
            if state_ref.syncing_controls || old == new {
                return;
            }
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetMarginY { old, new }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));
}

/// One of the six ways `align_selected` can line up the current
/// multi-selection's `Transform`s against each other.
#[derive(Clone, Copy)]
pub(crate) enum Alignment {
    Left,
    CenterHorizontal,
    Right,
    Top,
    CenterVertical,
    Bottom,
}

/// Aligns every currently selected screenshot's `Transform.x`/`.y` against
/// the selection's own bounding box, as one undoable [`SetTransforms`] —
/// e.g. `Alignment::Left` moves every selected element's left edge to the
/// leftmost selected element's left edge, `Alignment::CenterHorizontal`
/// centers each one within the selection's horizontal span. Only
/// meaningful in `LayoutMode::Free` (the alignment buttons are hidden
/// otherwise, see `editor_model::bind_editor_model`) and with at least two
/// elements selected — selection lives entirely in the `Canvas` widget
/// (`canvas.selected_ids()`), so unlike the layout mode this can go stale
/// between clicks without any signal telling this function about it,
/// hence the toast rather than a disabled button for the "too few
/// selected" case.
pub(crate) fn align_selected(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>, alignment: Alignment) {
    let selected = canvas.selected_ids();
    if selected.len() < 2 {
        window.toast_overlay().add_toast(adw::Toast::new("Mindestens 2 Screenshots auswählen, um sie auszurichten"));
        return;
    }
    let mut state_ref = state.borrow_mut();

    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for element in state_ref.document.elements.iter().filter(|e| selected.contains(&e.id)) {
        let t = element.transform;
        min_x = min_x.min(t.x);
        min_y = min_y.min(t.y);
        max_x = max_x.max(t.x + t.width);
        max_y = max_y.max(t.y + t.height);
    }

    let transforms: Vec<(Uuid, screenforge_core::model::Transform, screenforge_core::model::Transform)> = state_ref
        .document
        .elements
        .iter()
        .filter(|e| selected.contains(&e.id))
        .filter_map(|element| {
            let old = element.transform;
            let mut new = old;
            match alignment {
                Alignment::Left => new.x = min_x,
                Alignment::CenterHorizontal => new.x = min_x + ((max_x - min_x) - old.width) / 2.0,
                Alignment::Right => new.x = max_x - old.width,
                Alignment::Top => new.y = min_y,
                Alignment::CenterVertical => new.y = min_y + ((max_y - min_y) - old.height) / 2.0,
                Alignment::Bottom => new.y = max_y - old.height,
            }
            (old != new).then_some((element.id, old, new))
        })
        .collect();
    if transforms.is_empty() {
        return;
    }
    let EditorState { document, undo_stack, .. } = &mut *state_ref;
    undo_stack.apply(Box::new(SetTransforms { transforms }), document);
    drop(state_ref);
    refresh_canvas(window, canvas, state);
    update_undo_redo_sensitivity(window, state);
}

/// Wires the sidebar's six alignment buttons (spec: "Ausrichtungswerkzeug")
/// to `align_selected`.
pub(crate) fn register_alignment_controls(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let buttons = [
        (window.align_left_button(), Alignment::Left),
        (window.align_center_h_button(), Alignment::CenterHorizontal),
        (window.align_right_button(), Alignment::Right),
        (window.align_top_button(), Alignment::Top),
        (window.align_center_v_button(), Alignment::CenterVertical),
        (window.align_bottom_button(), Alignment::Bottom),
    ];
    for (button, alignment) in buttons {
        button.connect_clicked(glib::clone!(
            #[weak]
            window,
            #[weak]
            canvas,
            #[strong]
            state,
            move |_| align_selected(&window, &canvas, &state, alignment)
        ));
    }
}

/// "Gleichmäßig verteilen" (free layout): one row, equal gaps, centers on
/// one line — see `screenforge_core::layout::balanced_transforms`.
pub(crate) fn register_balance_control(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    window.balance_row().connect_activated(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |_| {
            let transforms = {
                let state_ref = state.borrow();
                screenforge_core::layout::balanced_transforms(&state_ref.document.elements, state_ref.document.layout.spacing_px)
            };
            if transforms.is_empty() {
                return;
            }
            {
                let mut state_ref = state.borrow_mut();
                let EditorState { document, undo_stack, .. } = &mut *state_ref;
                undo_stack.apply(Box::new(SetTransforms { transforms }), document);
            }
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));
}
