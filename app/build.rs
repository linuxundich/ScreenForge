use std::path::Path;
use std::process::Command;

fn main() {
    // `glib_build_tools::compile_resources` doesn't emit its own
    // `rerun-if-changed` for the files it bundles, and `compile_gsettings_schema`
    // below emits one of its own — which, once *any* rerun-if-changed is
    // emitted, opts this whole build script out of Cargo's default "rerun
    // every build" behavior. Without this, editing window.ui (or any other
    // resource) silently doesn't take effect until something else happens
    // to touch build.rs or Cargo.toml.
    println!("cargo:rerun-if-changed=resources");
    // The gresource bundle also pulls the app icon SVGs in from here (see
    // `resources/screenforge.gresource.xml`'s `icons` gresource) — they
    // live under `data/` rather than `resources/` so a future packaging
    // step can `install_data` them straight into a real hicolor icon theme
    // directory unchanged, but that means the rule above alone won't catch
    // edits to them.
    println!("cargo:rerun-if-changed=data/icons");

    glib_build_tools::compile_resources(
        &["resources"],
        "resources/screenforge.gresource.xml",
        "screenforge.gresource",
    );

    compile_gsettings_schema();
    compile_translations();
}

/// Compiles `../po/*.po` into `$OUT_DIR/locale/<lang>/LC_MESSAGES/screenforge.mo`
/// for `cargo run`; `i18n.rs` binds the text domain there unless an
/// installed build (meson) set `SCREENFORGE_LOCALEDIR`. Skipped with a
/// warning if `msgfmt` is missing — the app then simply shows English.
fn compile_translations() {
    println!("cargo:rerun-if-changed=../po");
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
    let Ok(entries) = std::fs::read_dir("../po") else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "po") {
            continue;
        }
        let Some(lang) = path.file_stem().and_then(|s| s.to_str()) else { continue };
        let target = Path::new(&out_dir).join("locale").join(lang).join("LC_MESSAGES");
        std::fs::create_dir_all(&target).expect("failed to create locale output directory");
        match Command::new("msgfmt").arg("-o").arg(target.join("screenforge.mo")).arg(&path).status() {
            Ok(status) if status.success() => {}
            _ => println!("cargo:warning=msgfmt failed or missing; {lang} translation not built"),
        }
    }
}

/// Compiles `data/*.gschema.xml` into `$OUT_DIR/schemas/gschemas.compiled`.
/// For `cargo run` only: `main.rs` points `GSETTINGS_SCHEMA_DIR` at this
/// directory unless the build was configured by meson, whose install step
/// puts the schema where GLib normally looks
/// (`$prefix/share/glib-2.0/schemas/`).
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
