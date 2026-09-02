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
  position it manually.
- **Reordering** by dragging screenshots directly on the canvas.
- **Per-screenshot context menu**: duplicate, replace, delete, bring
  forward/backward/to front/to back, rotate 90°, flip horizontal/vertical.
- **Backgrounds**: solid color, linear gradient, radial gradient, or an
  image (with Cover/Contain/Fill/Tile fitting and adjustable opacity).
- **Effects**: shadow presets (None/Subtle/Standard/Strong/Floating) and
  rounded corners.
- **Zoom**: fit to window, 100%, step in/out, with scrolling once zoomed in.
- **Undo/redo** for every edit.
- **Export** to PNG, JPEG, WebP or AVIF at a freely chosen resolution,
  rendered off the UI thread so the app never blocks.
- **Projects**: save/load as `.screenforge` files (versioned JSON,
  image references kept as paths — originals are never modified).

ScreenForge works entirely offline. Nothing is ever uploaded anywhere.

## Building

Requires a Rust toolchain, GTK 4 (≥ 4.16) and libadwaita (≥ 1.5) development
packages.

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
history. Not yet implemented: snap guides, resize handles for
free-positioned screenshots, multi-select, text and vector decorations,
reusable templates, and a preferences page.

## License

[GPL-3.0-or-later](LICENSE)
