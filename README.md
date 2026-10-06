# ScreenForge

A native GNOME app for arranging smartphone screenshots into a single wide
presentation image — built for bloggers, documentation writers and social
media creators who need to show several screenshots side by side.

Drop in a handful of (typically vertical) screenshots, and ScreenForge scales
them to a common height, lays them out horizontally, and lets you fine-tune
spacing, background, shadows and rounded corners before exporting a single
PNG, JPEG or WebP image.

## Features

- **Import** via file dialog (`Ctrl+O`), drag-and-drop, pasting from the
  clipboard (`Ctrl+V`), "Open with" from the file manager, a desktop
  screenshot through the screenshot portal, or directly from a connected
  Android device over `adb` — with a clean status bar (12:00, full
  battery and signal) thanks to Android's demo mode. The toolbar button
  shows live whether a device is reachable.
- **Layout modes**: horizontal, vertical, or grid (each scaling
  screenshots to a common size automatically, with adjustable spacing
  and separate horizontal/vertical outer margins), or free — drag any
  screenshot anywhere on the canvas to position it (snapping into
  alignment with other screenshots and the canvas edges/center), and its
  corner handles to resize it (aspect-locked by default). The canvas
  always resizes to fit its content automatically — nothing is ever
  cropped off.
- **Reordering** by dragging screenshots directly on the canvas.
- **Multi-select**: click, Shift-click, or marquee-drag across empty
  canvas space to select several screenshots at once (outlined on
  screen), then delete them all in one step, or — in Free layout — drag
  any of them to move the whole selection together.
- **Per-screenshot context menu**: duplicate, replace, delete, bring
  forward/backward/to front/to back, rotate 90°, flip horizontal/vertical.
- **Labels**: an optional caption per screenshot, its text set
  individually but its look (position, font, color, background, padding,
  corner radius, shadow) shared by every label in the project and edited
  once for all of them.
- **Callouts**: any number of "look, feature X" text bubbles per
  screenshot, each with an arrow (and a bordered target dot) pointing at
  a spot on the screenshot — text, colors, arrow and dot all editable
  inline, no dialog.
- **Backgrounds**: solid color, linear gradient, radial gradient, an
  image (with Cover/Contain/Fill/Tile fitting and adjustable opacity), or
  a generated abstract background in one of six styles — layered paper,
  concentric arcs, silk ribbons, folded planes, line bundles or soft mist
  — with harmonious palettes (vivid, light or dark, random or derived
  from the screenshots), soft shadows and film grain. The original wave
  generator remains available as "Waves (classic)".
- **Variants**: six alternative backgrounds one click away in the
  sidebar, or nine large previews of the real composition in the
  background studio — per style or mixed, in any mood.
- **Formats**: fit to content or fixed presets (16:9, 1:1, 4:5, 9:16,
  Open Graph 1200 × 630, Mastodon/Bluesky, Play Store feature graphic),
  the content always centered and never cropped.
- **Redactions**: hide parts of a screenshot as a dark bar, pixel blocks
  or a blur.
- **Watermark**: a text mark in any corner, with size, opacity and color.
- **Effects**: shadow presets (None/Subtle/Standard/Strong/Floating) with
  freely adjustable direction, length and blur, and rounded corners.
- **Workspace**: a tabbed sidebar (Layout, Background, Style, Text,
  Export) that turns into an overlay on narrow windows, a floating zoom
  bar (fit, 100 %, step in/out), and a shortcuts overview (Ctrl+?).
- **Undo/redo** for every edit.
- **Export** to PNG, JPEG, WebP, AVIF or PDF, scaled to a freely chosen
  target width (height always following proportionally), optionally with
  a transparent background, rendered off the UI thread so the app never
  blocks. Or copy the result straight to the clipboard (`Ctrl+Shift+C`),
  or drag it out of the window into a browser or chat.
- **Projects**: save/load as self-contained `.screenforge` files (a zip
  archive bundling a versioned JSON manifest with every screenshot's and
  background's own original image bytes, so a saved project keeps
  working even if the source files are later moved or deleted; older
  plain-JSON project files still load).
- **Presets**: save the current layout, background, shadow, corner
  radius and label look under a name from the header bar's Presets menu,
  then reapply, rename or delete it later — presets live in the app's
  own settings, not a separate file to manage.
- **Preferences** (`Ctrl+,`): default spacing, margins, label style and
  export quality for every newly created document.

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
history.

## License

[GPL-3.0-or-later](LICENSE)
