# Developing ScreenForge

## Requirements

A Rust toolchain, GTK 4 (≥ 4.22) and libadwaita (≥ 1.9) development
packages, `glib-compile-schemas`, and gettext (`msgfmt`) for the
translations. meson and flatpak-builder for installable builds.

## Quick start

```sh
cargo run -p screenforge
cargo test --workspace
```

`cargo run` needs no installation: `app/build.rs` compiles the GSettings
schema and the translations into `OUT_DIR`, and the binary uses them from
there. Run with `LANGUAGE=en` to see the English source strings on a
German system.

## meson

```sh
meson setup _build --prefix="$HOME/.local"
meson compile -C _build
meson test -C _build      # validates desktop file, metainfo and schema
meson install -C _build
```

meson builds the binary through cargo (`build-aux/cargo-build.sh`) and
passes the install locale directory in as `SCREENFORGE_LOCALEDIR`, which
also tells the binary to use the installed GSettings schema.

## Flatpak

```sh
build-aux/flatpak/build.sh           # build and install for the current user
build-aux/flatpak/build.sh --run     # ... and start it
build-aux/flatpak/build.sh --bundle  # ... and write screenforge.flatpak
```

The build runs offline: the script vendors all crates first. It uses the
GNOME 51 runtime. Inside the sandbox, Android import calls the host's
`adb` through `flatpak-spawn --host`.

## Project layout

- `core/` (`screenforge-core`): document model, layout engine, the Cairo
  renderer shared by preview and export, background generator, undo
  stack, `.screenforge` project files. No GTK dependency, unit-tested.
- `app/` (`screenforge`): the GTK 4 + libadwaita application.
  - `main.rs` builds the window and wires the modules together.
  - `panels/`: one module per sidebar tab and topic (layout, background,
    variants, effects, text, callouts, redactions, output, export).
  - `editor_model.rs`: the window's UI-facing state as a GObject that
    widgets bind to.
  - `sources.rs` (import, adb, portal screenshot, open with), `project.rs`,
    `presets.rs`, `dialogs.rs`, `editing.rs` (canvas actions), `i18n.rs`.
- `app/src/cli.rs`: the command line (`--preset`, `--list-presets`);
  `app/src/video.rs`: the animated WebM export through GStreamer
  (needs the `vp9enc` and `webmmux` elements, part of gst-plugins-good).
- `core/src/frame.rs`: device frames; `core/src/warp.rs`: the
  perspective warp for tilted screenshots.
- `po/`: translations, see [po/README.md](../po/README.md).
- `build-aux/`: meson's cargo wrapper and the Flatpak manifest.
