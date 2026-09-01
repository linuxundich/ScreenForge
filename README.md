# ScreenForge

A native GNOME app for arranging smartphone screenshots into a single wide
presentation image — built for bloggers, documentation writers and social
media creators who need to show several screenshots side by side.

Drop in a handful of (typically vertical) screenshots, and ScreenForge scales
them to a common height, lays them out horizontally, and lets you fine-tune
spacing, background, shadows and rounded corners before exporting a single
PNG, JPEG or WebP image.

## Features

- **Import** via file dialog (`Ctrl+O`) or drag-and-drop; PNG, JPEG, WebP.
- **Automatic horizontal layout** with adjustable spacing and outer margin.
- **Reordering** by dragging screenshots directly on the canvas.
- **Backgrounds**: solid color or linear gradient.
- **Effects**: shadow presets (None/Subtle/Standard/Strong/Floating) and
  rounded corners.
- **Zoom**: fit to window, 100%, step in/out, with scrolling once zoomed in.
- **Undo/redo** for every edit.
- **Export** to PNG, JPEG or WebP at a freely chosen resolution, rendered
  off the UI thread so the app never blocks.
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
history. Not yet implemented: clipboard paste, a context menu for
per-screenshot actions (delete/duplicate/rotate/flip/replace), grid/snap
guides, text and vector decorations, reusable templates, image backgrounds,
additional layout modes (vertical/grid/free), AVIF export, and a
preferences page.

## License

[GPL-3.0-or-later](LICENSE)
