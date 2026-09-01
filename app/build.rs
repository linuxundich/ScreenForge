fn main() {
    glib_build_tools::compile_resources(
        &["resources"],
        "resources/screenforge.gresource.xml",
        "screenforge.gresource",
    );
}
