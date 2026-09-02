# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.15.0] - 2026-09-02

### Added

- Text captions ("Text-Overlay"): an optional single-line or multi-line
  caption drawn over the whole composition, with adjustable position,
  font size and color. Rendered via Cairo's text API directly (no Pango
  dependency), on top of every element, and fully undoable.

## [0.14.0] - 2026-09-02

### Added

- Adjustable shadow direction and length: the "Schatten-Winkel" (0–360°)
  and "Schatten-Distanz" controls replace the fixed offsets baked into
  each shadow preset, exposed as an intuitive angle/distance pair over
  the existing Cartesian offset model.
- Adjustable shadow blur ("Weichzeichner"): a box-blur approximation of
  a Gaussian blur (three passes, horizontal+vertical sliding window),
  applied to the shadow shape before compositing, replacing the
  previously hard-edged shadow.
- Selecting a shadow preset still sets sensible starting values for all
  three controls, but they're now freely adjustable afterwards and
  fully undoable like every other edit.

## [0.13.0] - 2026-09-02

### Added

- Vector-pattern backgrounds ("Vektor-Muster"): a dot grid or diagonal
  stripes, in a chosen color, covering the whole canvas — the first real
  use of the `Background::Decoration` variant that previously only
  existed as a model stub.

## [0.12.0] - 2026-09-02

### Added

- A `GSettings`-backed preferences dialog (`Ctrl+,` or the new primary
  menu button), for the default spacing, margin and export quality used
  for every newly created document. Changing a preference never touches
  the document currently open.

## [0.11.0] - 2026-09-02

### Added

- Reusable templates: save a composition's style (layout mode/spacing/
  margin, background, shadow, corner radius — everything except the
  screenshots themselves) as a `.screenforge-template` file, and load it
  back later to reapply that look to a different set of screenshots.
  Reachable from the "Öffnen" menu ("Vorlage speichern unter…" /
  "Vorlage laden…"). Applying a template is undoable, like every other
  edit.

## [0.10.0] - 2026-09-02

### Fixed

- The canvas now always automatically resizes to fit its content — every
  screenshot plus spacing and margin — instead of staying at a fixed
  default size. Previously, a tall portrait screenshot (e.g. a phone's
  1080×2424 screenshot) could get cropped at the bottom because the
  canvas stayed at its 1920×1080 default regardless of what was imported.

### Changed

- Export size is now a single "Zielbreite" (target width): the
  composition renders scaled so its width matches it, with height always
  following proportionally — never distorted, never cropped. The old
  independent width/height fields are gone; height is shown read-only.

## [0.9.0] - 2026-09-02

### Added

- Snap ("smart") guides while dragging a screenshot in Free mode: edges
  and centers snap into exact alignment with other screenshots and with
  the canvas's own edges/center, with a thin pink guide line drawn for
  each active snap. Pure alignment math lives in `core::snap` and is
  unit-tested independently of the canvas widget.

## [0.8.0] - 2026-09-02

### Added

- Resize handles for Free-mode screenshots: drag any corner to resize,
  keeping the opposite corner anchored. Respects each screenshot's
  aspect-lock (on by default) by deriving height from width. Undoable,
  like every other edit.

### Note

Snap guides and multi-select are still not part of this slice.

## [0.7.0] - 2026-09-02

### Added

- Free layout mode ("Frei"): drag any screenshot anywhere on the canvas to
  position it manually, instead of an automatic arrangement. Switching
  into it snapshots each screenshot's current (auto-computed) placement as
  its starting position, so nothing jumps to the origin. Moves are
  undoable, like every other edit.

### Note

Resizing, snap guides and multi-select are deliberately not part of this
slice — manual positioning ships first, those build on top of it next.

## [0.6.0] - 2026-09-02

### Added

- Image backgrounds (spec §8): pick a file as the composition's
  background, with Cover/Contain/Fill/Tile fitting and an opacity slider,
  alongside solid and gradient backgrounds. Undoable, and rendered
  identically in the live preview and the export.

## [0.5.0] - 2026-09-02

### Added

- AVIF export, alongside PNG, JPEG and WebP, using the quality slider
  already shared with JPEG/WebP.

## [0.4.0] - 2026-09-02

### Added

- Radial gradient backgrounds, alongside solid and linear-gradient (spec
  §8), selectable from the existing "Art" row in the Hintergrund section.
  Centered on the composition; undoable like every other background
  change.

## [0.3.0] - 2026-09-02

### Added

- Vertical and grid layout modes, alongside the existing horizontal one,
  selectable from a new "Art" row in the Layout section of the sidebar
  (spec §4). Vertical stacks screenshots top-to-bottom scaled to a common
  width; grid arranges them into a roughly square, row-scaled grid.
  Switching modes is undoable, like every other edit.

### Fixed

- A `GtkPopoverMenu` used for the per-screenshot context menu could still
  be attached to its parent widget when the window closed, producing a
  harmless but noisy "finalizing widget but it still has children left"
  warning on quit. It's now explicitly unparented on window destroy.

## [0.2.0] - 2026-09-01

### Added

- A right-click context menu on any screenshot (spec §21): Duplicate,
  Replace…, Delete, Bring forward/backward/to front/to back, Rotate 90°,
  and Flip horizontal/vertical.
- Paste a screenshot directly from the clipboard (`Ctrl+V`).
- All of the above are undoable, alongside every existing edit.

## [0.1.0] - 2026-09-01

Initial release: a GTK4 + libadwaita GNOME app for arranging smartphone
screenshots into a single wide presentation image.

### Added

- Import screenshots via a file dialog (`Ctrl+O`) or by dragging them onto
  the canvas; PNG, JPEG and WebP are supported.
- Automatic horizontal layout that scales every screenshot to a common
  height, with adjustable spacing and outer margin.
- Solid-color and linear-gradient backgrounds.
- Shadow presets (None, Subtle, Standard, Strong, Floating) and adjustable
  rounded corners, applied to the whole composition.
- Canvas zoom: fit to window, 100%, step in/out (`Ctrl+0/1/+/-`), with a
  scrollable view once the zoomed content exceeds the visible area.
- Drag-and-drop reordering of screenshots directly on the canvas.
- Undo/redo (`Ctrl+Z` / `Ctrl+Shift+Z`) covering every edit above.
- Export to PNG, JPEG or WebP at a freely configurable resolution,
  rendered on a background thread so the UI never blocks.
- Save and load projects as `.screenforge` files — a versioned JSON format
  that keeps image references as paths rather than copying originals.
- A responsive, HIG-compliant window (header bar, collapsible sidebar,
  toast notifications) built with GTK4 and libadwaita.
