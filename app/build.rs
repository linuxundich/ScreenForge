use std::path::Path;
use std::process::Command;

fn main() {
    glib_build_tools::compile_resources(
        &["resources"],
        "resources/screenforge.gresource.xml",
        "screenforge.gresource",
    );

    compile_gsettings_schema();
}

/// Compiles `data/*.gschema.xml` into `$OUT_DIR/schemas/gschemas.compiled`.
/// There's no meson/install step yet to put this where GLib normally looks
/// for compiled schemas (`$XDG_DATA_DIRS/glib-2.0/schemas/`), so `main.rs`
/// points `GSETTINGS_SCHEMA_DIR` at this directory instead — confirmed
/// empirically that GLib treats that env var as an *additional* search
/// path, not a replacement, so this needs no system-wide installation.
/// Whenever a real packaged build exists, its install step should compile
/// the schema into the standard system location and this becomes a no-op
/// fallback for `cargo run`.
fn compile_gsettings_schema() {
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
    let schema_dir = Path::new(&out_dir).join("schemas");
    std::fs::create_dir_all(&schema_dir).expect("failed to create schema output directory");

    let source = Path::new("data/de.christophlangner.ScreenForge.gschema.xml");
    std::fs::copy(source, schema_dir.join("de.christophlangner.ScreenForge.gschema.xml"))
        .expect("failed to copy gschema.xml into OUT_DIR");

    let status = Command::new("glib-compile-schemas")
        .arg(&schema_dir)
        .status()
        .expect("failed to run glib-compile-schemas — is it installed?");
    assert!(status.success(), "glib-compile-schemas failed");

    println!("cargo:rerun-if-changed={}", source.display());
}
