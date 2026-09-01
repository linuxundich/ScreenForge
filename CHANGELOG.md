# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
