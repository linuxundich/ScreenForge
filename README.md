# ScreenForge

A native GNOME app for arranging smartphone screenshots into a single wide
presentation image — built for bloggers, documentation writers and social
media creators who need to show several screenshots side by side.

Drop in a handful of (typically vertical) screenshots, and ScreenForge scales
them to a common height, lays them out horizontally, and lets you fine-tune
spacing, background, shadows and rounded corners before exporting a single
PNG, JPEG or WebP image.

## Features

- **Import** via file dialog (`Ctrl+O`), drag-and-drop, or pasting from the
  clipboard (`Ctrl+V`); PNG, JPEG, WebP.
- **Layout modes**: horizontal, vertical, or grid (each scaling screenshots
  to a common size automatically, with adjustable spacing and outer
  margin), or free — drag any screenshot anywhere on the canvas to
  position it (snapping into alignment with other screenshots and the
  canvas edges/center), and its corner handles to resize it
  (aspect-locked by default). The canvas always resizes to fit its
  content automatically — nothing is ever cropped off.
- **Reordering** by dragging screenshots directly on the canvas.
- **Per-screenshot context menu**: duplicate, replace, delete, bring
  forward/backward/to front/to back, rotate 90°, flip horizontal/vertical.
- **Backgrounds**: solid color, linear gradient, radial gradient, an
  image (with Cover/Contain/Fill/Tile fitting and adjustable opacity), or
  a vector pattern (dot grid or diagonal stripes) in a chosen color.
- **Effects**: shadow presets (None/Subtle/Standard/Strong/Floating) with
  freely adjustable direction, length and blur, and rounded corners.
- **Text overlay**: an optional caption drawn over the whole composition,
  with adjustable position, font size and color.
- **Zoom**: fit to window, 100%, step in/out, with scrolling once zoomed in.
- **Undo/redo** for every edit.
- **Export** to PNG, JPEG, WebP or AVIF, scaled to a freely chosen target
  width (height always following proportionally), rendered off the UI
  thread so the app never blocks.
- **Projects**: save/load as `.screenforge` files (versioned JSON,
  image references kept as paths — originals are never modified).
- **Templates**: save a composition's style (layout, spacing/margin,
  background, shadow, corner radius) as a `.screenforge-template` file
  and reapply it to a different set of screenshots later.
- **Preferences** (`Ctrl+,`): default spacing, margin and export quality
  for every newly created document.

ScreenForge works entirely offline. Nothing is ever uploaded anywhere.

## Building

Requires a Rust toolchain, GTK 4 (≥ 4.16), libadwaita (≥ 1.5) development
packages, and `glib-compile-schemas` (for the preferences `GSettings`
schema — `build.rs` compiles it into `$OUT_DIR` and points
`GSETTINGS_SCHEMA_DIR` there at startup, so no system-wide schema
installation is needed for `cargo build`/`cargo run`; a real packaged
build's install step should compile it into the standard system location
instead).

```sh
cargo build --release
./target/release/screenforge
```

For day-to-day development:

```sh
cargo run -p screenforge
cargo test --workspace
```

## Project layout

The workspace is split so the composition logic stays independent of the
GUI toolkit and is unit-testable on its own:

- `core/` (`screenforge-core`) — the document model, the horizontal-layout
  engine, the Cairo-based renderer shared by the live preview and the
  full-resolution export, the undo/redo command stack, and `.screenforge`
  project (de)serialization. No GTK dependency.
- `app/` (`screenforge`) — the GTK4 + libadwaita application: the window,
  the canvas widget, file import/export, and project save/load.

## Status

ScreenForge is under active development. The features listed above are
implemented and tested; see [CHANGELOG.md](CHANGELOG.md) for release
history. Not yet implemented: multi-select, and freeform vector shapes (only
the two background patterns above exist so far).

## License

[GPL-3.0-or-later](LICENSE)
