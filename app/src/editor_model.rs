//! `EditorModel`: the editor's UI-facing state as a GObject. Widgets don't
//! get this state pushed into them by hand; they bind to its properties
//! (`bind_property`, or a `gtk4::ClosureExpression` where a widget depends
//! on more than one property). The document itself stays plain Rust data
//! in `EditorState`; this object carries what the window shows *about* it
//! (undo availability, title, empty or not) and the three selections that
//! decide which sidebar rows are visible.
//!
//! The three selection properties are bound bidirectionally to their
//! controls, so they always equal what the control shows — including the
//! moment a user picks a new background type, before anything has been
//! committed to the document.

use crate::*;

/// Index of `background_type_row`'s "Generiert" entry.
pub(crate) const BACKGROUND_KIND_GENERATED: u32 = 4;
/// Index of `background_type_row`'s "Screenshot (unscharf)" entry.
pub(crate) const BACKGROUND_KIND_BLURRED: u32 = 5;
/// Index of `background_type_row`'s "Bild" entry.
pub(crate) const BACKGROUND_KIND_IMAGE: u32 = 3;

mod imp {
    use std::cell::{Cell, RefCell};

    use gtk4::glib;
    use gtk4::glib::prelude::*;
    use gtk4::glib::subclass::prelude::*;

    #[derive(glib::Properties, Default)]
    #[properties(wrapper_type = super::EditorModel)]
    pub struct EditorModel {
        #[property(get, set)]
        can_undo: Cell<bool>,
        #[property(get, set)]
        can_redo: Cell<bool>,
        /// A text field has keyboard focus, so Ctrl+Z etc. belong to it
        /// rather than to the document (see `register_text_focus_guards`).
        #[property(get, set)]
        text_focused: Cell<bool>,
        #[property(get, set)]
        title: RefCell<String>,
        #[property(get, set)]
        subtitle: RefCell<String>,
        #[property(get, set)]
        is_empty: Cell<bool>,
        /// Index into `background_type_row` (solid, linear, radial, image,
        /// generated).
        #[property(get, set)]
        background_kind: Cell<u32>,
        /// Index into `generator_color_strategy_row`.
        #[property(get, set)]
        color_strategy: Cell<u32>,
        /// Index into `layout_mode_toggle`.
        #[property(get, set)]
        layout_mode: Cell<u32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for EditorModel {
        const NAME: &'static str = "ScreenForgeEditorModel";
        type Type = super::EditorModel;
    }

    #[glib::derived_properties]
    impl ObjectImpl for EditorModel {}
}

glib::wrapper! {
    pub struct EditorModel(ObjectSubclass<imp::EditorModel>);
}

impl Default for EditorModel {
    fn default() -> Self {
        glib::Object::builder().property("is-empty", true).property("title", gettext("New Scene")).build()
    }
}

impl EditorModel {
    /// Refreshes the document-derived properties from `state`. Called from
    /// `refresh_canvas`, which every edit goes through.
    pub(crate) fn update_from(&self, state: &EditorState) {
        let title = state.scene.as_ref().map(|s| s.name.clone()).unwrap_or_else(|| gettext("New Scene"));
        let count = state.document.elements.len();
        let subtitle = if count == 0 {
            String::new()
        } else {
            let c = state.document.canvas;
            let scale = c.export_target_width as f64 / c.export_width.max(1) as f64;
            let height = (c.export_height as f64 * scale).round().max(1.0);
            // Translators: the header bar subtitle, e.g. "3 screenshots · 1920 × 1080 px".
            ngettext("{count} screenshot · {width} × {height} px", "{count} screenshots · {width} × {height} px", count as u32)
                .replace("{count}", &count.to_string())
                .replace("{width}", &c.export_target_width.to_string())
                .replace("{height}", &format!("{height:.0}"))
        };
        if self.title() != title {
            self.set_title(title);
        }
        if self.subtitle() != subtitle {
            self.set_subtitle(subtitle);
        }
        if self.is_empty() != (count == 0) {
            self.set_is_empty(count == 0);
        }
        self.update_undo(state);
    }

    pub(crate) fn update_undo(&self, state: &EditorState) {
        if self.can_undo() != state.undo_stack.can_undo() {
            self.set_can_undo(state.undo_stack.can_undo());
        }
        if self.can_redo() != state.undo_stack.can_redo() {
            self.set_can_redo(state.undo_stack.can_redo());
        }
    }
}

/// Binds `widget`'s `visible` to a predicate over one model property.
fn show_when<T: for<'a> glib::value::FromValue<'a> + 'static>(
    model: &EditorModel,
    property: &str,
    widget: &impl IsA<glib::Object>,
    predicate: fn(T) -> bool,
) {
    model.bind_property(property, widget, "visible").transform_to(move |_, value: T| Some(predicate(value))).sync_create().build();
}

/// Binds `widget`'s `visible` to a predicate over the background kind and
/// the generator's color strategy together.
fn show_when_kind_and_strategy(model: &EditorModel, widget: &impl IsA<glib::Object>, predicate: fn(u32, u32) -> bool) {
    let kind = gtk4::PropertyExpression::new(EditorModel::static_type(), None::<gtk4::Expression>, "background-kind");
    let strategy = gtk4::PropertyExpression::new(EditorModel::static_type(), None::<gtk4::Expression>, "color-strategy");
    let visible = gtk4::ClosureExpression::new::<bool>(
        [kind, strategy],
        glib::closure!(move |_: Option<glib::Object>, kind: u32, strategy: u32| predicate(kind, strategy)),
    );
    visible.bind(widget, "visible", Some(model));
}

/// Wires every model-driven widget property. Call once, after the undo/
/// redo actions exist.
pub(crate) fn bind_editor_model(window: &Window, canvas: &Canvas, model: &EditorModel) {
    let both = glib::BindingFlags::BIDIRECTIONAL | glib::BindingFlags::SYNC_CREATE;
    window.background_type_row().bind_property("selected", model, "background-kind").flags(both).build();
    window.generator_color_strategy_row().bind_property("selected", model, "color-strategy").flags(both).build();
    window.layout_mode_toggle().bind_property("active", model, "layout-mode").flags(both).build();

    show_when(model, "background-kind", &window.background_color1_row(), |k: u32| k < BACKGROUND_KIND_IMAGE);
    show_when(model, "background-kind", &window.blurred_blur_row(), |k: u32| k == BACKGROUND_KIND_BLURRED);
    show_when(model, "background-kind", &window.blurred_brightness_row(), |k: u32| k == BACKGROUND_KIND_BLURRED);
    show_when(model, "background-kind", &window.gradient_color2_row(), |k: u32| k == 1 || k == 2);
    show_when(model, "background-kind", &window.gradient_angle_row(), |k: u32| k == 1);
    show_when(model, "background-kind", &window.gradient_auto_colors_row(), |k: u32| k == 1 || k == 2);
    for row in [window.background_image_row().upcast::<gtk4::Widget>(), window.background_image_fit_row().upcast(), window.background_image_opacity_row().upcast()] {
        show_when(model, "background-kind", &row, |k: u32| k == BACKGROUND_KIND_IMAGE);
    }
    let generator_widgets: [gtk4::Widget; 12] = [
        window.generator_color_strategy_row().upcast(),
        window.generator_style_row().upcast(),
        window.generator_mood_row().upcast(),
        window.generator_grain_row().upcast(),
        window.generator_adapt_row().upcast(),
        window.generator_corner_bias_row().upcast(),
        window.generator_scale_row().upcast(),
        window.generator_contrast_row().upcast(),
        window.generator_seed_row().upcast(),
        window.generator_generate_row().upcast(),
        window.variants_group().upcast(),
        window.variants_actions_group().upcast(),
    ];
    for widget in &generator_widgets {
        show_when(model, "background-kind", widget, |k: u32| k == BACKGROUND_KIND_GENERATED);
    }
    for row in [
        window.generator_manual_color_row_1(),
        window.generator_manual_color_row_2(),
        window.generator_manual_color_row_3(),
        window.generator_manual_color_row_4(),
    ] {
        show_when_kind_and_strategy(model, &row, |k, s| k == BACKGROUND_KIND_GENERATED && s == index_for_color_strategy(ColorStrategy::Manual));
    }
    show_when_kind_and_strategy(model, &window.generator_inverse_contrast_row(), |k, s| {
        k == BACKGROUND_KIND_GENERATED && s == index_for_color_strategy(ColorStrategy::FromScreenshots)
    });

    show_when(model, "layout-mode", &window.alignment_group(), |m: u32| m == index_for_layout_mode(LayoutMode::Free));

    for (action, property) in [("undo", "can-undo"), ("redo", "can-redo")] {
        if let Some(action) = window.lookup_action(action) {
            let possible = gtk4::PropertyExpression::new(EditorModel::static_type(), None::<gtk4::Expression>, property);
            let typing = gtk4::PropertyExpression::new(EditorModel::static_type(), None::<gtk4::Expression>, "text-focused");
            let enabled = gtk4::ClosureExpression::new::<bool>(
                [possible, typing],
                glib::closure!(|_: Option<glib::Object>, possible: bool, typing: bool| possible && !typing),
            );
            enabled.bind(&action, "enabled", Some(model));
        }
    }

    let title = window.window_title();
    model.bind_property("title", &title, "title").sync_create().build();
    model.bind_property("subtitle", &title, "subtitle").sync_create().build();
    show_when(model, "is-empty", &window.empty_state(), |empty: bool| empty);
    show_when(model, "is-empty", &window.canvas_toolbar(), |empty: bool| !empty);
    show_when(model, "is-empty", canvas, |empty: bool| !empty);
}
