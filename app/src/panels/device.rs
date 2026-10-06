//! "Device and Perspective" in the style tab: a generic device frame around
//! the screenshots, and a perspective tilt — the same for all, fanned out
//! so the outer screenshots turn toward the middle, or, with "For All
//! Screenshots" off, set for the selected screenshots only.

use crate::*;

const KINDS: [(DeviceKind, &str); 4] = [
    (DeviceKind::None, N_("No Frame")),
    (DeviceKind::Phone, N_("Phone")),
    (DeviceKind::Tablet, N_("Tablet")),
    (DeviceKind::Browser, N_("Browser Window")),
];
const TONES: [(FrameTone, &str); 2] = [(FrameTone::Dark, N_("Dark")), (FrameTone::Light, N_("Light"))];

fn string_list(labels: impl Iterator<Item = &'static str>) -> gtk4::StringList {
    let translated: Vec<String> = labels.map(gettext).collect();
    gtk4::StringList::new(&translated.iter().map(String::as_str).collect::<Vec<_>>())
}

/// Per-element `(tilt_x, tilt_y)`: everyone the same, or with `fan` the
/// turn scales from `+turn` on the far left to `-turn` on the far right
/// (left-to-right by layout order, or by position in the free layout).
fn tilts_for(doc: &Document, lean: f64, turn: f64, fan: bool) -> Vec<(f64, f64)> {
    let n = doc.elements.len();
    if !fan || n < 2 {
        return vec![(lean, turn); n];
    }
    let mut order: Vec<usize> = (0..n).collect();
    if doc.layout.mode == LayoutMode::Free {
        order.sort_by(|&a, &b| doc.elements[a].transform.x.total_cmp(&doc.elements[b].transform.x));
    }
    let mut tilts = vec![(lean, 0.0); n];
    let half = (n - 1) as f64 / 2.0;
    for (rank, &index) in order.iter().enumerate() {
        let position = (rank as f64 - half) / half;
        tilts[index] = (lean, -position * turn);
    }
    tilts
}

fn commit(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>, command: Box<dyn Command>) {
    {
        let mut state_ref = state.borrow_mut();
        if state_ref.syncing_controls {
            return;
        }
        let EditorState { document, undo_stack, .. } = &mut *state_ref;
        undo_stack.apply(command, document);
    }
    refresh_canvas(window, canvas, state);
    update_undo_redo_sensitivity(window, state);
}

pub(crate) fn register_device_controls(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let kind_row = adw::ComboRow::builder().title(gettext("Device")).model(&string_list(KINDS.iter().map(|k| k.1))).build();
    let tone_row = adw::ComboRow::builder().title(gettext("Frame Color")).model(&string_list(TONES.iter().map(|t| t.1))).build();
    let turn_row = adw::SpinRow::with_range(-45.0, 45.0, 1.0);
    turn_row.set_title(&gettext("Turn"));
    turn_row.set_subtitle(&gettext("Around the vertical axis, in degrees"));
    // A new SpinRow starts at its minimum; untilted is 0.
    turn_row.set_value(0.0);
    let lean_row = adw::SpinRow::with_range(-30.0, 30.0, 1.0);
    lean_row.set_title(&gettext("Lean"));
    lean_row.set_subtitle(&gettext("Around the horizontal axis, in degrees"));
    lean_row.set_value(0.0);
    let fan_row = adw::SwitchRow::builder().title(gettext("Fan Out")).subtitle(gettext("The outer screenshots turn toward the middle")).build();
    let all_row = adw::SwitchRow::builder()
        .title(gettext("For All Screenshots"))
        .subtitle(gettext("Off: changes apply to the selected screenshots only"))
        .active(true)
        .build();

    let group = adw::PreferencesGroup::builder().title(gettext("Device and Perspective")).build();
    for row in [all_row.upcast_ref::<gtk4::Widget>(), kind_row.upcast_ref(), tone_row.upcast_ref(), turn_row.upcast_ref(), lean_row.upcast_ref(), fan_row.upcast_ref()] {
        group.add(row);
    }
    kind_row.bind_property("selected", &tone_row, "visible").transform_to(|_, selected: u32| Some(selected != 0)).sync_create().build();
    // Fanning out only makes sense across all screenshots.
    all_row.bind_property("active", &fan_row, "sensitive").sync_create().build();
    window.style_page().add(&group);

    let apply_frame = Rc::new(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        #[weak]
        kind_row,
        #[weak]
        tone_row,
        #[weak]
        all_row,
        move || {
            let frame = DeviceFrame {
                kind: KINDS.get(kind_row.selected() as usize).map(|k| k.0).unwrap_or_default(),
                tone: TONES.get(tone_row.selected() as usize).map(|t| t.0).unwrap_or_default(),
            };
            let targets = target_ids(&canvas, &state, all_row.is_active());
            let (old, new) = {
                let state_ref = state.borrow();
                let old: Vec<DeviceFrame> = state_ref.document.elements.iter().map(|e| e.frame).collect();
                let new = state_ref.document.elements.iter().map(|e| if targets.contains(&e.id) { frame } else { e.frame }).collect::<Vec<_>>();
                (old, new)
            };
            if old != new {
                commit(&window, &canvas, &state, Box::new(SetFrames { old, new }));
            }
        }
    ));
    kind_row.connect_selected_notify(glib::clone!(#[strong] apply_frame, move |_| apply_frame()));
    tone_row.connect_selected_notify(glib::clone!(#[strong] apply_frame, move |_| apply_frame()));

    let apply_tilt = Rc::new(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        #[weak]
        turn_row,
        #[weak]
        lean_row,
        #[weak]
        fan_row,
        #[weak]
        all_row,
        move || {
            let all = all_row.is_active();
            let targets = target_ids(&canvas, &state, all);
            let (old, new) = {
                let state_ref = state.borrow();
                let doc = &state_ref.document;
                let old: Vec<(f64, f64)> = doc.elements.iter().map(|e| (e.transform.tilt_x, e.transform.tilt_y)).collect();
                let wanted = tilts_for(doc, lean_row.value(), turn_row.value(), all && fan_row.is_active());
                let new = doc.elements.iter().zip(wanted).zip(old.iter()).map(|((e, w), o)| if targets.contains(&e.id) { w } else { *o }).collect();
                (old, new)
            };
            if old != new {
                commit(&window, &canvas, &state, Box::new(SetTilts { old, new }));
            }
        }
    ));
    turn_row.connect_value_notify(glib::clone!(#[strong] apply_tilt, move |_| apply_tilt()));
    lean_row.connect_value_notify(glib::clone!(#[strong] apply_tilt, move |_| apply_tilt()));
    fan_row.connect_active_notify(glib::clone!(#[strong] apply_tilt, move |_| apply_tilt()));

    // Reflect the document: the first (or, for "selected only", the first
    // selected) element's frame, the largest turn, and "fan" when the
    // screenshots' turns differ.
    let show: Rc<dyn Fn(&Document)> = Rc::new(glib::clone!(
        #[weak]
        canvas,
        #[weak]
        all_row,
        #[weak]
        kind_row,
        #[weak]
        tone_row,
        #[weak]
        turn_row,
        #[weak]
        lean_row,
        #[weak]
        fan_row,
        move |doc: &Document| {
        let selected = canvas.selected_ids();
        let first = if all_row.is_active() { doc.elements.first() } else { doc.elements.iter().find(|e| selected.contains(&e.id)).or(doc.elements.first()) };
        let Some(first) = first else { return };
        kind_row.set_selected(KINDS.iter().position(|k| k.0 == first.frame.kind).unwrap_or(0) as u32);
        tone_row.set_selected(TONES.iter().position(|t| t.0 == first.frame.tone).unwrap_or(0) as u32);
        let turns: Vec<f64> = doc.elements.iter().map(|e| e.transform.tilt_y).collect();
        let fan = turns.iter().any(|t| (t - turns[0]).abs() > 0.01);
        let turn = if fan { turns.iter().cloned().fold(0.0_f64, f64::max) } else { turns[0] };
        fan_row.set_active(fan);
        turn_row.set_value(turn);
        lean_row.set_value(first.transform.tilt_x);
    }));
    let sync: DocumentSync = show.clone();
    let doc = state.borrow().document.clone();
    state.borrow_mut().syncing_controls = true;
    sync(&doc);
    state.borrow_mut().syncing_controls = false;
    state.borrow_mut().document_syncs.push(sync);
    // With "selected only", picking another screenshot shows its values.
    let on_selection: Rc<dyn Fn()> = Rc::new(glib::clone!(
        #[strong]
        state,
        #[weak]
        all_row,
        #[strong]
        show,
        move || {
            if all_row.is_active() {
                return;
            }
            let doc = state.borrow().document.clone();
            state.borrow_mut().syncing_controls = true;
            show(&doc);
            state.borrow_mut().syncing_controls = false;
        }
    ));
    state.borrow_mut().selection_syncs.push(on_selection);
}

/// The ids a change applies to: every screenshot, or only the selected
/// ones (none selected means nothing changes).
fn target_ids(canvas: &Canvas, state: &Rc<RefCell<EditorState>>, all: bool) -> std::collections::HashSet<Uuid> {
    if all {
        state.borrow().document.elements.iter().map(|e| e.id).collect()
    } else {
        canvas.selected_ids()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fanning_turns_the_outer_screenshots_toward_the_middle() {
        let mut doc = Document::new();
        for _ in 0..3 {
            doc.elements.push(ScreenshotElement::new(ImageSource::Path("x.png".into()), 100.0, 200.0));
        }
        let tilts = tilts_for(&doc, 5.0, 20.0, true);
        assert_eq!(tilts, vec![(5.0, 20.0), (5.0, 0.0), (5.0, -20.0)]);
        assert_eq!(tilts_for(&doc, 0.0, 10.0, false), vec![(0.0, 10.0); 3]);
    }
}
