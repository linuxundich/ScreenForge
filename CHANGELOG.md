# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
