use gtk4::gio;
use gtk4::glib;
use libadwaita as adw;
use libadwaita::subclass::prelude::ObjectSubclassIsExt;

use crate::canvas::Canvas;

glib::wrapper! {
    pub struct Window(ObjectSubclass<imp::Window>)
        @extends adw::ApplicationWindow, gtk4::ApplicationWindow, gtk4::Window, gtk4::Widget,
        @implements gio::ActionGroup, gio::ActionMap, gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget, gtk4::Native, gtk4::Root, gtk4::ShortcutManager;
}

impl Window {
    pub fn new(app: &adw::Application) -> Self {
        glib::Object::builder().property("application", app).build()
    }

    pub fn canvas(&self) -> Canvas {
        self.imp().canvas.get().clone()
    }

    pub fn split_view(&self) -> adw::OverlaySplitView {
        self.imp().split_view.get().clone()
    }

    pub fn sidebar_toggle_button(&self) -> gtk4::ToggleButton {
        self.imp().sidebar_toggle_button.get().clone()
    }


    pub fn spacing_row(&self) -> adw::SpinRow {
        self.imp().spacing_row.get().clone()
    }

    pub fn margin_x_row(&self) -> adw::SpinRow {
        self.imp().margin_x_row.get().clone()
    }

    pub fn margin_y_row(&self) -> adw::SpinRow {
        self.imp().margin_y_row.get().clone()
    }

    pub fn alignment_group(&self) -> adw::PreferencesGroup {
        self.imp().alignment_group.get().clone()
    }

    pub fn align_left_button(&self) -> gtk4::Button {
        self.imp().align_left_button.get().clone()
    }

    pub fn align_center_h_button(&self) -> gtk4::Button {
        self.imp().align_center_h_button.get().clone()
    }

    pub fn align_right_button(&self) -> gtk4::Button {
        self.imp().align_right_button.get().clone()
    }

    pub fn align_top_button(&self) -> gtk4::Button {
        self.imp().align_top_button.get().clone()
    }

    pub fn align_center_v_button(&self) -> gtk4::Button {
        self.imp().align_center_v_button.get().clone()
    }

    pub fn align_bottom_button(&self) -> gtk4::Button {
        self.imp().align_bottom_button.get().clone()
    }

    pub fn background_type_row(&self) -> adw::ComboRow {
        self.imp().background_type_row.get().clone()
    }

    pub fn background_color1_row(&self) -> adw::ActionRow {
        self.imp().background_color1_row.get().clone()
    }

    pub fn background_color_button(&self) -> gtk4::ColorDialogButton {
        self.imp().background_color_button.get().clone()
    }

    pub fn background_eyedropper_button(&self) -> gtk4::Button {
        self.imp().background_eyedropper_button.get().clone()
    }

    pub fn gradient_color2_row(&self) -> adw::ActionRow {
        self.imp().gradient_color2_row.get().clone()
    }

    pub fn gradient_color2_button(&self) -> gtk4::ColorDialogButton {
        self.imp().gradient_color2_button.get().clone()
    }

    pub fn gradient_color2_eyedropper_button(&self) -> gtk4::Button {
        self.imp().gradient_color2_eyedropper_button.get().clone()
    }

    pub fn gradient_angle_row(&self) -> adw::SpinRow {
        self.imp().gradient_angle_row.get().clone()
    }

    pub fn gradient_auto_colors_row(&self) -> adw::ActionRow {
        self.imp().gradient_auto_colors_row.get().clone()
    }

    pub fn gradient_generate_button(&self) -> gtk4::Button {
        self.imp().gradient_generate_button.get().clone()
    }

    pub fn background_image_row(&self) -> adw::ActionRow {
        self.imp().background_image_row.get().clone()
    }

    pub fn background_image_button(&self) -> gtk4::Button {
        self.imp().background_image_button.get().clone()
    }

    pub fn background_image_fit_row(&self) -> adw::ComboRow {
        self.imp().background_image_fit_row.get().clone()
    }

    pub fn background_image_opacity_row(&self) -> adw::SpinRow {
        self.imp().background_image_opacity_row.get().clone()
    }

    pub fn background_group(&self) -> adw::PreferencesGroup {
        self.imp().background_group.get().clone()
    }

    pub fn generator_color_strategy_row(&self) -> adw::ComboRow {
        self.imp().generator_color_strategy_row.get().clone()
    }

    pub fn generator_style_row(&self) -> adw::ComboRow {
        self.imp().generator_style_row.get().clone()
    }

    pub fn generator_mood_row(&self) -> adw::ComboRow {
        self.imp().generator_mood_row.get().clone()
    }

    pub fn generator_grain_row(&self) -> adw::SpinRow {
        self.imp().generator_grain_row.get().clone()
    }

    pub fn generator_manual_color_button_1(&self) -> gtk4::ColorDialogButton {
        self.imp().generator_manual_color_button_1.get().clone()
    }

    pub fn generator_manual_eyedropper_button_1(&self) -> gtk4::Button {
        self.imp().generator_manual_eyedropper_button_1.get().clone()
    }

    pub fn generator_manual_color_row_1(&self) -> adw::ActionRow {
        self.imp().generator_manual_color_row_1.get().clone()
    }

    pub fn generator_manual_color_button_2(&self) -> gtk4::ColorDialogButton {
        self.imp().generator_manual_color_button_2.get().clone()
    }

    pub fn generator_manual_eyedropper_button_2(&self) -> gtk4::Button {
        self.imp().generator_manual_eyedropper_button_2.get().clone()
    }

    pub fn generator_manual_color_row_2(&self) -> adw::ActionRow {
        self.imp().generator_manual_color_row_2.get().clone()
    }

    pub fn generator_manual_color_button_3(&self) -> gtk4::ColorDialogButton {
        self.imp().generator_manual_color_button_3.get().clone()
    }

    pub fn generator_manual_eyedropper_button_3(&self) -> gtk4::Button {
        self.imp().generator_manual_eyedropper_button_3.get().clone()
    }

    pub fn generator_manual_color_row_3(&self) -> adw::ActionRow {
        self.imp().generator_manual_color_row_3.get().clone()
    }

    pub fn generator_manual_color_button_4(&self) -> gtk4::ColorDialogButton {
        self.imp().generator_manual_color_button_4.get().clone()
    }

    pub fn generator_manual_eyedropper_button_4(&self) -> gtk4::Button {
        self.imp().generator_manual_eyedropper_button_4.get().clone()
    }

    pub fn generator_manual_color_row_4(&self) -> adw::ActionRow {
        self.imp().generator_manual_color_row_4.get().clone()
    }

    pub fn generator_adapt_row(&self) -> adw::SwitchRow {
        self.imp().generator_adapt_row.get().clone()
    }

    pub fn generator_inverse_contrast_row(&self) -> adw::SpinRow {
        self.imp().generator_inverse_contrast_row.get().clone()
    }

    pub fn generator_corner_bias_row(&self) -> adw::SpinRow {
        self.imp().generator_corner_bias_row.get().clone()
    }

    pub fn generator_scale_row(&self) -> adw::SpinRow {
        self.imp().generator_scale_row.get().clone()
    }

    pub fn generator_contrast_row(&self) -> adw::SpinRow {
        self.imp().generator_contrast_row.get().clone()
    }

    pub fn generator_seed_row(&self) -> adw::SpinRow {
        self.imp().generator_seed_row.get().clone()
    }

    pub fn generator_generate_row(&self) -> adw::ButtonRow {
        self.imp().generator_generate_row.get().clone()
    }


    pub fn shadow_row(&self) -> adw::ComboRow {
        self.imp().shadow_row.get().clone()
    }

    pub fn shadow_angle_row(&self) -> adw::SpinRow {
        self.imp().shadow_angle_row.get().clone()
    }

    pub fn shadow_distance_row(&self) -> adw::SpinRow {
        self.imp().shadow_distance_row.get().clone()
    }

    pub fn shadow_blur_row(&self) -> adw::SpinRow {
        self.imp().shadow_blur_row.get().clone()
    }

    pub fn corner_radius_row(&self) -> adw::SpinRow {
        self.imp().corner_radius_row.get().clone()
    }


    pub fn label_group(&self) -> adw::PreferencesGroup {
        self.imp().label_group.get().clone()
    }

    pub fn callouts_group(&self) -> adw::PreferencesGroup {
        self.imp().callouts_group.get().clone()
    }

    pub fn export_group(&self) -> adw::PreferencesGroup {
        self.imp().export_group.get().clone()
    }

    pub fn callouts_list_box(&self) -> gtk4::ListBox {
        self.imp().callouts_list_box.get().clone()
    }

    pub fn add_callout_button(&self) -> gtk4::Button {
        self.imp().add_callout_button.get().clone()
    }

    pub fn label_enabled_row(&self) -> adw::SwitchRow {
        self.imp().label_enabled_row.get().clone()
    }

    pub fn label_content_view(&self) -> gtk4::TextView {
        self.imp().label_content_view.get().clone()
    }

    pub fn export_width_row(&self) -> adw::SpinRow {
        self.imp().export_width_row.get().clone()
    }

    pub fn export_height_row(&self) -> adw::SpinRow {
        self.imp().export_height_row.get().clone()
    }

    pub fn export_format_row(&self) -> adw::ComboRow {
        self.imp().export_format_row.get().clone()
    }

    pub fn export_quality_row(&self) -> adw::SpinRow {
        self.imp().export_quality_row.get().clone()
    }

    pub fn export_button(&self) -> gtk4::Button {
        self.imp().export_button.get().clone()
    }

    pub fn hide_screenshots_button(&self) -> gtk4::ToggleButton {
        self.imp().hide_screenshots_button.get().clone()
    }

    pub fn android_import_button(&self) -> gtk4::Button {
        self.imp().android_import_button.get().clone()
    }

    pub fn presets_menu_button(&self) -> gtk4::MenuButton {
        self.imp().presets_menu_button.get().clone()
    }

    pub fn window_title(&self) -> adw::WindowTitle {
        self.imp().window_title.get().clone()
    }

    pub fn sidebar_stack(&self) -> adw::ViewStack {
        self.imp().sidebar_stack.get().clone()
    }

    pub fn layout_page(&self) -> adw::PreferencesPage {
        self.imp().layout_page.get().clone()
    }

    pub fn background_page(&self) -> adw::PreferencesPage {
        self.imp().background_page.get().clone()
    }

    pub fn style_page(&self) -> adw::PreferencesPage {
        self.imp().style_page.get().clone()
    }

    pub fn text_page(&self) -> adw::PreferencesPage {
        self.imp().text_page.get().clone()
    }

    pub fn export_page(&self) -> adw::PreferencesPage {
        self.imp().export_page.get().clone()
    }

    pub fn layout_mode_toggle(&self) -> adw::ToggleGroup {
        self.imp().layout_mode_toggle.get().clone()
    }

    pub fn canvas_overlay(&self) -> gtk4::Overlay {
        self.imp().canvas_overlay.get().clone()
    }

    pub fn empty_state(&self) -> adw::StatusPage {
        self.imp().empty_state.get().clone()
    }

    pub fn canvas_toolbar(&self) -> gtk4::Box {
        self.imp().canvas_toolbar.get().clone()
    }

    pub fn zoom_menu_button(&self) -> gtk4::MenuButton {
        self.imp().zoom_menu_button.get().clone()
    }

    pub fn variants_group(&self) -> adw::PreferencesGroup {
        self.imp().variants_group.get().clone()
    }

    pub fn variants_actions_group(&self) -> adw::PreferencesGroup {
        self.imp().variants_actions_group.get().clone()
    }

    pub fn variants_flow(&self) -> gtk4::FlowBox {
        self.imp().variants_flow.get().clone()
    }

    pub fn variants_mix_row(&self) -> adw::SwitchRow {
        self.imp().variants_mix_row.get().clone()
    }

    pub fn variants_reroll_row(&self) -> adw::ButtonRow {
        self.imp().variants_reroll_row.get().clone()
    }

    pub fn variants_studio_row(&self) -> adw::ButtonRow {
        self.imp().variants_studio_row.get().clone()
    }

    pub fn toast_overlay(&self) -> adw::ToastOverlay {
        self.imp().toast_overlay.get().clone()
    }
}

mod imp {
    use gtk4::glib;
    use gtk4::glib::prelude::StaticTypeExt;
    use gtk4::subclass::prelude::*;
    use gtk4::CompositeTemplate;
    use libadwaita as adw;
    use libadwaita::subclass::prelude::*;

    use crate::canvas::Canvas;

    #[derive(CompositeTemplate, Default)]
    #[template(resource = "/de/christophlangner/ScreenForge/ui/window.ui")]
    pub struct Window {
        #[template_child]
        pub canvas: TemplateChild<Canvas>,
        #[template_child]
        pub split_view: TemplateChild<adw::OverlaySplitView>,
        #[template_child]
        pub sidebar_toggle_button: TemplateChild<gtk4::ToggleButton>,
        #[template_child]
        pub spacing_row: TemplateChild<adw::SpinRow>,
        #[template_child]
        pub margin_x_row: TemplateChild<adw::SpinRow>,
        #[template_child]
        pub margin_y_row: TemplateChild<adw::SpinRow>,
        #[template_child]
        pub alignment_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub align_left_button: TemplateChild<gtk4::Button>,
        #[template_child]
        pub align_center_h_button: TemplateChild<gtk4::Button>,
        #[template_child]
        pub align_right_button: TemplateChild<gtk4::Button>,
        #[template_child]
        pub align_top_button: TemplateChild<gtk4::Button>,
        #[template_child]
        pub align_center_v_button: TemplateChild<gtk4::Button>,
        #[template_child]
        pub align_bottom_button: TemplateChild<gtk4::Button>,
        #[template_child]
        pub background_type_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub background_color1_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub background_color_button: TemplateChild<gtk4::ColorDialogButton>,
        #[template_child]
        pub background_eyedropper_button: TemplateChild<gtk4::Button>,
        #[template_child]
        pub gradient_color2_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub gradient_color2_button: TemplateChild<gtk4::ColorDialogButton>,
        #[template_child]
        pub gradient_color2_eyedropper_button: TemplateChild<gtk4::Button>,
        #[template_child]
        pub gradient_angle_row: TemplateChild<adw::SpinRow>,
        #[template_child]
        pub gradient_auto_colors_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub gradient_generate_button: TemplateChild<gtk4::Button>,
        #[template_child]
        pub background_image_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub background_image_button: TemplateChild<gtk4::Button>,
        #[template_child]
        pub background_image_fit_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub background_image_opacity_row: TemplateChild<adw::SpinRow>,
        #[template_child]
        pub background_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub generator_color_strategy_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub generator_style_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub generator_mood_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub generator_grain_row: TemplateChild<adw::SpinRow>,
        #[template_child]
        pub generator_manual_color_row_1: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub generator_manual_color_button_1: TemplateChild<gtk4::ColorDialogButton>,
        #[template_child]
        pub generator_manual_eyedropper_button_1: TemplateChild<gtk4::Button>,
        #[template_child]
        pub generator_manual_color_row_2: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub generator_manual_color_button_2: TemplateChild<gtk4::ColorDialogButton>,
        #[template_child]
        pub generator_manual_eyedropper_button_2: TemplateChild<gtk4::Button>,
        #[template_child]
        pub generator_manual_color_row_3: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub generator_manual_color_button_3: TemplateChild<gtk4::ColorDialogButton>,
        #[template_child]
        pub generator_manual_eyedropper_button_3: TemplateChild<gtk4::Button>,
        #[template_child]
        pub generator_manual_color_row_4: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub generator_manual_color_button_4: TemplateChild<gtk4::ColorDialogButton>,
        #[template_child]
        pub generator_manual_eyedropper_button_4: TemplateChild<gtk4::Button>,
        #[template_child]
        pub generator_adapt_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub generator_inverse_contrast_row: TemplateChild<adw::SpinRow>,
        #[template_child]
        pub generator_corner_bias_row: TemplateChild<adw::SpinRow>,
        #[template_child]
        pub generator_scale_row: TemplateChild<adw::SpinRow>,
        #[template_child]
        pub generator_contrast_row: TemplateChild<adw::SpinRow>,
        #[template_child]
        pub generator_seed_row: TemplateChild<adw::SpinRow>,
        #[template_child]
        pub generator_generate_row: TemplateChild<adw::ButtonRow>,
        #[template_child]
        pub shadow_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub shadow_angle_row: TemplateChild<adw::SpinRow>,
        #[template_child]
        pub shadow_distance_row: TemplateChild<adw::SpinRow>,
        #[template_child]
        pub shadow_blur_row: TemplateChild<adw::SpinRow>,
        #[template_child]
        pub corner_radius_row: TemplateChild<adw::SpinRow>,
        #[template_child]
        pub label_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub callouts_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub callouts_list_box: TemplateChild<gtk4::ListBox>,
        #[template_child]
        pub add_callout_button: TemplateChild<gtk4::Button>,
        #[template_child]
        pub label_enabled_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub label_content_view: TemplateChild<gtk4::TextView>,
        #[template_child]
        pub export_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub export_width_row: TemplateChild<adw::SpinRow>,
        #[template_child]
        pub export_height_row: TemplateChild<adw::SpinRow>,
        #[template_child]
        pub export_format_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub export_quality_row: TemplateChild<adw::SpinRow>,
        #[template_child]
        pub export_button: TemplateChild<gtk4::Button>,
        #[template_child]
        pub hide_screenshots_button: TemplateChild<gtk4::ToggleButton>,
        #[template_child]
        pub android_import_button: TemplateChild<gtk4::Button>,
        #[template_child]
        pub presets_menu_button: TemplateChild<gtk4::MenuButton>,
        #[template_child]
        pub window_title: TemplateChild<adw::WindowTitle>,
        #[template_child]
        pub sidebar_stack: TemplateChild<adw::ViewStack>,
        #[template_child]
        pub layout_page: TemplateChild<adw::PreferencesPage>,
        #[template_child]
        pub background_page: TemplateChild<adw::PreferencesPage>,
        #[template_child]
        pub style_page: TemplateChild<adw::PreferencesPage>,
        #[template_child]
        pub text_page: TemplateChild<adw::PreferencesPage>,
        #[template_child]
        pub export_page: TemplateChild<adw::PreferencesPage>,
        #[template_child]
        pub layout_mode_toggle: TemplateChild<adw::ToggleGroup>,
        #[template_child]
        pub canvas_overlay: TemplateChild<gtk4::Overlay>,
        #[template_child]
        pub empty_state: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub canvas_toolbar: TemplateChild<gtk4::Box>,
        #[template_child]
        pub zoom_menu_button: TemplateChild<gtk4::MenuButton>,
        #[template_child]
        pub variants_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub variants_actions_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub variants_flow: TemplateChild<gtk4::FlowBox>,
        #[template_child]
        pub variants_mix_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub variants_reroll_row: TemplateChild<adw::ButtonRow>,
        #[template_child]
        pub variants_studio_row: TemplateChild<adw::ButtonRow>,
        #[template_child]
        pub toast_overlay: TemplateChild<adw::ToastOverlay>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Window {
        const NAME: &'static str = "ScreenForgeWindow";
        type Type = super::Window;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            Canvas::ensure_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for Window {}
    impl WidgetImpl for Window {}
    impl WindowImpl for Window {}
    impl ApplicationWindowImpl for Window {}
    impl AdwApplicationWindowImpl for Window {}
}
