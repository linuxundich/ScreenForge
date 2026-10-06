//! The export tab's additions: canvas format presets (fixed aspect ratio
//! plus export width), a transparent background, and the watermark. All
//! groups are built here in code and slotted into the export page around
//! the template's own "Export" group.

use crate::*;

/// One entry of the "Format" dropdown: label, fixed aspect (or `None` to
/// fit the content) and the export width it sets.
struct FormatPreset {
    label: &'static str,
    aspect: Option<(u32, u32)>,
    width: u32,
}

const FORMAT_PRESETS: [FormatPreset; 10] = [
    FormatPreset { label: N_("Fit to Content"), aspect: None, width: 0 },
    FormatPreset { label: "16:9 · 1920 × 1080", aspect: Some((16, 9)), width: 1920 },
    FormatPreset { label: "1:1 · 1080 × 1080", aspect: Some((1, 1)), width: 1080 },
    FormatPreset { label: N_("4:5 · 1080 × 1350 (Instagram)"), aspect: Some((4, 5)), width: 1080 },
    FormatPreset { label: N_("9:16 · 1080 × 1920 (Story)"), aspect: Some((9, 16)), width: 1080 },
    FormatPreset { label: N_("Open Graph · 1200 × 630"), aspect: Some((1200, 630)), width: 1200 },
    FormatPreset { label: N_("Mastodon/Bluesky · 1600 × 900"), aspect: Some((16, 9)), width: 1600 },
    FormatPreset { label: N_("Play Store Graphic · 1024 × 500"), aspect: Some((1024, 500)), width: 1024 },
    FormatPreset { label: N_("App Store iPhone 6.9″ · 1320 × 2868"), aspect: Some((1320, 2868)), width: 1320 },
    FormatPreset { label: N_("App Store iPad 13″ · 2064 × 2752"), aspect: Some((2064, 2752)), width: 2064 },
];

fn format_index_for(canvas: &screenforge_core::model::CanvasSettings) -> u32 {
    match canvas.aspect {
        None => 0,
        Some(aspect) => FORMAT_PRESETS
            .iter()
            .position(|p| p.aspect == Some(aspect) && p.width * canvas.slices.max(1) == canvas.export_target_width)
            .or_else(|| FORMAT_PRESETS.iter().position(|p| p.aspect == Some(aspect)))
            .unwrap_or(0) as u32,
    }
}

const CORNERS: [(WatermarkCorner, &str); 4] = [
    (WatermarkCorner::BottomRight, N_("Bottom Right")),
    (WatermarkCorner::BottomLeft, N_("Bottom Left")),
    (WatermarkCorner::TopRight, N_("Top Right")),
    (WatermarkCorner::TopLeft, N_("Top Left")),
];

/// Pushes `command` onto the undo stack unless controls are being synced
/// from the document, then refreshes.
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

pub(crate) fn register_output_controls(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    // Format
    let format_row = adw::ComboRow::builder()
        .title(gettext("Format"))
        .subtitle(gettext("Fixed aspect ratios grow the canvas around the content"))
        .model(&gtk4::StringList::new(&FORMAT_PRESETS.iter().map(|p| gettext(p.label)).collect::<Vec<_>>().iter().map(String::as_str).collect::<Vec<_>>()))
        .build();
    let slices_row = adw::SpinRow::with_range(1.0, 10.0, 1.0);
    slices_row.set_title(&gettext("Split Into"));
    slices_row.set_subtitle(&gettext("Several images side by side with one continuous background, e.g. for app store screenshots"));
    let format_group = adw::PreferencesGroup::builder().title(gettext("Format")).build();
    format_group.add(&format_row);
    format_group.add(&slices_row);
    let apply_format = Rc::new(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        #[weak]
        format_row,
        #[weak]
        slices_row,
        move || {
            let Some(preset) = FORMAT_PRESETS.get(format_row.selected() as usize) else { return };
            let current = state.borrow().document.canvas;
            let slices = slices_row.value().round().max(1.0) as u32;
            let old = (current.aspect, current.export_target_width, current.slices);
            // A preset's width is per image; with several images the export
            // is that many times as wide. Without a fixed format the
            // current width stays.
            let width = if preset.aspect.is_none() { current.export_target_width } else { preset.width * slices };
            let new = (preset.aspect, width, slices);
            if old == new {
                return;
            }
            commit(&window, &canvas, &state, Box::new(SetCanvasFormat { old, new }));
            let was = state.borrow().syncing_controls;
            state.borrow_mut().syncing_controls = true;
            window.export_width_row().set_value(width as f64);
            state.borrow_mut().syncing_controls = was;
        }
    ));
    format_row.connect_selected_notify(glib::clone!(#[strong] apply_format, move |_| apply_format()));
    slices_row.connect_value_notify(glib::clone!(#[strong] apply_format, move |_| apply_format()));

    // Transparent background, part of the template's export group.
    let transparent_row = adw::SwitchRow::builder()
        .title(gettext("Transparent Background"))
        .subtitle(gettext("Export without background (PNG, WebP, AVIF, PDF; JPEG becomes white)"))
        .build();
    window.export_group().add(&transparent_row);
    transparent_row.connect_active_notify(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |row| {
            if state.borrow().document.canvas.transparent_background != row.is_active() {
                commit(&window, &canvas, &state, Box::new(SetTransparentBackground { new: row.is_active() }));
            }
        }
    ));
    let copy_row = adw::ButtonRow::builder().title(gettext("Copy to Clipboard")).start_icon_name("edit-copy-symbolic").action_name("win.copy-image").build();
    window.export_group().add(&copy_row);

    // Watermark
    let wm_enabled = adw::SwitchRow::builder().title(gettext("Show Watermark")).build();
    let wm_text = adw::EntryRow::builder().title(gettext("Text")).show_apply_button(true).build();
    let wm_corner = adw::ComboRow::builder()
        .title(gettext("Position"))
        .model(&gtk4::StringList::new(&CORNERS.iter().map(|c| gettext(c.1)).collect::<Vec<_>>().iter().map(String::as_str).collect::<Vec<_>>()))
        .build();
    let wm_size = adw::SpinRow::with_range(1.0, 10.0, 0.5);
    wm_size.set_title(&gettext("Size"));
    wm_size.set_subtitle(&gettext("In percent of the shorter side"));
    wm_size.set_digits(1);
    let wm_opacity = adw::SpinRow::with_range(10.0, 100.0, 5.0);
    wm_opacity.set_title(&gettext("Opacity"));
    wm_opacity.set_subtitle(&gettext("In percent"));
    let wm_color = gtk4::ColorDialogButton::builder().dialog(&gtk4::ColorDialog::new()).valign(gtk4::Align::Center).build();
    let wm_color_row = adw::ActionRow::builder().title(gettext("Color")).build();
    wm_color_row.add_suffix(&wm_color);
    let watermark_group = adw::PreferencesGroup::builder().title(gettext("Watermark")).description(gettext("For example your blog's domain, in a corner of the image")).build();
    for row in [wm_enabled.upcast_ref::<gtk4::Widget>(), wm_text.upcast_ref(), wm_corner.upcast_ref(), wm_size.upcast_ref(), wm_opacity.upcast_ref(), wm_color_row.upcast_ref()] {
        watermark_group.add(row);
    }
    for row in [wm_text.upcast_ref::<gtk4::Widget>(), wm_corner.upcast_ref(), wm_size.upcast_ref(), wm_opacity.upcast_ref(), wm_color_row.upcast_ref()] {
        wm_enabled.bind_property("active", row, "sensitive").sync_create().build();
    }

    let read_watermark = {
        let (wm_enabled, wm_text, wm_corner, wm_size, wm_opacity, wm_color) =
            (wm_enabled.clone(), wm_text.clone(), wm_corner.clone(), wm_size.clone(), wm_opacity.clone(), wm_color.clone());
        move || Watermark {
            enabled: wm_enabled.is_active(),
            text: wm_text.text().to_string(),
            corner: CORNERS.get(wm_corner.selected() as usize).map(|c| c.0).unwrap_or_default(),
            size: wm_size.value() / 100.0,
            color: rgba_from_gdk(&wm_color.rgba()),
            opacity: wm_opacity.value() / 100.0,
        }
    };
    let apply_watermark: Rc<dyn Fn()> = Rc::new(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move || {
            let new = read_watermark();
            let old = state.borrow().document.watermark.clone();
            if old != new {
                commit(&window, &canvas, &state, Box::new(SetWatermark { old, new }));
            }
        }
    ));
    wm_enabled.connect_active_notify(glib::clone!(#[strong] apply_watermark, move |_| apply_watermark()));
    wm_text.connect_apply(glib::clone!(#[strong] apply_watermark, move |_| apply_watermark()));
    wm_corner.connect_selected_notify(glib::clone!(#[strong] apply_watermark, move |_| apply_watermark()));
    wm_size.connect_value_notify(glib::clone!(#[strong] apply_watermark, move |_| apply_watermark()));
    wm_opacity.connect_value_notify(glib::clone!(#[strong] apply_watermark, move |_| apply_watermark()));
    wm_color.connect_rgba_notify(glib::clone!(#[strong] apply_watermark, move |_| apply_watermark()));

    // Order on the page: Format, Export (template), Wasserzeichen.
    let page = window.export_page();
    let export_group = window.export_group();
    page.remove(&export_group);
    page.add(&format_group);
    page.add(&export_group);
    page.add(&watermark_group);

    // Reflect the document onto these rows after undo/redo/load.
    let sync: Rc<dyn Fn(&Document)> = Rc::new(move |doc: &Document| {
        format_row.set_selected(format_index_for(&doc.canvas));
        slices_row.set_value(doc.canvas.slices.max(1) as f64);
        transparent_row.set_active(doc.canvas.transparent_background);
        let wm = &doc.watermark;
        wm_enabled.set_active(wm.enabled);
        wm_text.set_text(&wm.text);
        wm_corner.set_selected(CORNERS.iter().position(|c| c.0 == wm.corner).unwrap_or(0) as u32);
        wm_size.set_value(wm.size * 100.0);
        wm_opacity.set_value(wm.opacity * 100.0);
        wm_color.set_rgba(&gdk_rgba_from(&wm.color));
    });
    // Never hold the state borrowed while setting widgets: their change
    // handlers borrow it themselves (and bail out on `syncing_controls`).
    let doc = state.borrow().document.clone();
    state.borrow_mut().syncing_controls = true;
    sync(&doc);
    state.borrow_mut().syncing_controls = false;
    state.borrow_mut().document_syncs.push(sync);
}
