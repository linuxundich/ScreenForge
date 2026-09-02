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

    pub fn layout_mode_row(&self) -> adw::ComboRow {
        self.imp().layout_mode_row.get().clone()
    }

    pub fn spacing_row(&self) -> adw::SpinRow {
        self.imp().spacing_row.get().clone()
    }

    pub fn margin_row(&self) -> adw::SpinRow {
        self.imp().margin_row.get().clone()
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

    pub fn gradient_color2_row(&self) -> adw::ActionRow {
        self.imp().gradient_color2_row.get().clone()
    }

    pub fn gradient_color2_button(&self) -> gtk4::ColorDialogButton {
        self.imp().gradient_color2_button.get().clone()
    }

    pub fn gradient_angle_row(&self) -> adw::SpinRow {
        self.imp().gradient_angle_row.get().clone()
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

    pub fn background_decoration_row(&self) -> adw::ComboRow {
        self.imp().background_decoration_row.get().clone()
    }

    pub fn shadow_row(&self) -> adw::ComboRow {
        self.imp().shadow_row.get().clone()
    }

    pub fn corner_radius_row(&self) -> adw::SpinRow {
        self.imp().corner_radius_row.get().clone()
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
        pub layout_mode_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub spacing_row: TemplateChild<adw::SpinRow>,
        #[template_child]
        pub margin_row: TemplateChild<adw::SpinRow>,
        #[template_child]
        pub background_type_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub background_color1_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub background_color_button: TemplateChild<gtk4::ColorDialogButton>,
        #[template_child]
        pub gradient_color2_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub gradient_color2_button: TemplateChild<gtk4::ColorDialogButton>,
        #[template_child]
        pub gradient_angle_row: TemplateChild<adw::SpinRow>,
        #[template_child]
        pub background_image_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub background_image_button: TemplateChild<gtk4::Button>,
        #[template_child]
        pub background_image_fit_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub background_image_opacity_row: TemplateChild<adw::SpinRow>,
        #[template_child]
        pub background_decoration_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub shadow_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub corner_radius_row: TemplateChild<adw::SpinRow>,
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
