//! GTK-independent document model for a ScreenForge composition.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A straight RGBA color, channels in `0.0..=1.0`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rgba {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

impl Rgba {
    pub const fn new(r: f64, g: f64, b: f64, a: f64) -> Self {
        Self { r, g, b, a }
    }

    pub const WHITE: Rgba = Rgba::new(1.0, 1.0, 1.0, 1.0);
    pub const BLACK: Rgba = Rgba::new(0.0, 0.0, 0.0, 1.0);
}

/// Where a screenshot's pixel data comes from. `Embedded` is a forward-looking
/// stub for the later "save project with assets" option; the MVP only writes
/// `Path` into project files.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "kebab-case")]
pub enum ImageSource {
    Path(PathBuf),
    Embedded { filename: String, bytes: Vec<u8> },
}

/// Position, size and rotation of a screenshot element in document space
/// (i.e. canvas/export pixels, not screen pixels).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub rotation_deg: f64,
    pub aspect_locked: bool,
    /// `#[serde(default)]` so a `.screenforge` file saved before these two
    /// fields existed (0.1.0) still loads — absent means "never flipped",
    /// which `false` already means, so no migration logic is needed.
    #[serde(default)]
    pub flip_horizontal: bool,
    #[serde(default)]
    pub flip_vertical: bool,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
            rotation_deg: 0.0,
            aspect_locked: true,
            flip_horizontal: false,
            flip_vertical: false,
        }
    }
}

/// Per-corner radius, in document pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CornerRadius {
    pub top_left: f64,
    pub top_right: f64,
    pub bottom_right: f64,
    pub bottom_left: f64,
}

impl CornerRadius {
    pub const fn uniform(radius: f64) -> Self {
        Self { top_left: radius, top_right: radius, bottom_right: radius, bottom_left: radius }
    }

    pub const fn none() -> Self {
        Self::uniform(0.0)
    }
}

impl Default for CornerRadius {
    fn default() -> Self {
        Self::none()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ShadowParams {
    pub enabled: bool,
    pub offset_x: f64,
    pub offset_y: f64,
    pub blur: f64,
    pub opacity: f64,
    pub color: Rgba,
}

impl ShadowParams {
    pub const fn none() -> Self {
        Self { enabled: false, offset_x: 0.0, offset_y: 0.0, blur: 0.0, opacity: 0.0, color: Rgba::BLACK }
    }

    pub const fn subtle() -> Self {
        Self { enabled: true, offset_x: 0.0, offset_y: 2.0, blur: 8.0, opacity: 0.15, color: Rgba::BLACK }
    }

    pub const fn standard() -> Self {
        Self { enabled: true, offset_x: 0.0, offset_y: 6.0, blur: 16.0, opacity: 0.25, color: Rgba::BLACK }
    }

    pub const fn strong() -> Self {
        Self { enabled: true, offset_x: 0.0, offset_y: 12.0, blur: 28.0, opacity: 0.4, color: Rgba::BLACK }
    }

    pub const fn floating() -> Self {
        Self { enabled: true, offset_x: 0.0, offset_y: 24.0, blur: 40.0, opacity: 0.3, color: Rgba::BLACK }
    }
}

impl Default for ShadowParams {
    fn default() -> Self {
        Self::none()
    }
}

/// A single imported screenshot placed on the canvas.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenshotElement {
    pub id: Uuid,
    pub source: ImageSource,
    /// Decoded pixel size of the source image, in pixels. Populated by the
    /// app layer after decoding; the layout engine treats this as the
    /// element's un-transformed natural size.
    pub natural_width: f64,
    pub natural_height: f64,
    pub transform: Transform,
    pub corner_radius: CornerRadius,
    pub shadow: ShadowParams,
    pub visible: bool,
}

impl ScreenshotElement {
    pub fn new(source: ImageSource, natural_width: f64, natural_height: f64) -> Self {
        Self {
            id: Uuid::new_v4(),
            source,
            natural_width,
            natural_height,
            transform: Transform::default(),
            corner_radius: CornerRadius::default(),
            shadow: ShadowParams::default(),
            visible: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GradientKind {
    Linear { angle_deg: f64 },
    Radial { center_x: f64, center_y: f64 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GradientSpec {
    pub kind: GradientKind,
    /// `(position 0..=1, color)` pairs, ordered by position.
    pub stops: Vec<(f64, Rgba)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BackgroundImageFit {
    Cover,
    Contain,
    Fill,
    Tile,
}

/// Stub for the later "image as background" feature (spec §8) — the enum
/// variant and a plausible field shape exist now so the project-file schema
/// won't need a breaking migration when it's implemented.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageBackgroundSpec {
    pub source: ImageSource,
    pub fit: BackgroundImageFit,
    pub opacity: f64,
}

/// Stub for the later decorative vector background elements (spec §8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "kebab-case")]
pub enum VectorShape {
    Circle { cx: f64, cy: f64, radius: f64, color: Rgba },
    Line { x1: f64, y1: f64, x2: f64, y2: f64, width: f64, color: Rgba },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "kebab-case")]
pub enum Background {
    Solid(Rgba),
    Gradient(GradientSpec),
    Image(ImageBackgroundSpec),
    Decoration(Vec<VectorShape>),
}

impl Default for Background {
    fn default() -> Self {
        Background::Solid(Rgba::new(0.95, 0.95, 0.96, 1.0))
    }
}

/// Only `Horizontal` is implemented by the MVP layout engine; the other
/// variants exist so the model/project schema doesn't need to change when
/// they're implemented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LayoutMode {
    Horizontal,
    Vertical,
    Grid,
    Free,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LayoutSettings {
    pub mode: LayoutMode,
    pub spacing_px: f64,
    pub margin_px: f64,
}

impl Default for LayoutSettings {
    fn default() -> Self {
        Self { mode: LayoutMode::Horizontal, spacing_px: 24.0, margin_px: 48.0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExportFormat {
    Png,
    Jpeg,
    WebP,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CanvasSettings {
    pub export_width: u32,
    pub export_height: u32,
    pub export_format: ExportFormat,
    /// `0..=100`, ignored for [`ExportFormat::Png`] (lossless).
    pub export_quality: u8,
}

impl Default for CanvasSettings {
    fn default() -> Self {
        Self { export_width: 1920, export_height: 1080, export_format: ExportFormat::Png, export_quality: 90 }
    }
}

/// The full state of one composition. Pure data — no GTK types, no undo
/// history (that lives in [`crate::command::UndoStack`], kept separately so
/// commands can mutably borrow a `Document` without also holding the stack
/// that dispatched them).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub id: Uuid,
    pub elements: Vec<ScreenshotElement>,
    pub layout: LayoutSettings,
    pub background: Background,
    pub canvas: CanvasSettings,
}

impl Document {
    pub fn new() -> Self {
        Self {
            id: Uuid::new_v4(),
            elements: Vec::new(),
            layout: LayoutSettings::default(),
            background: Background::default(),
            canvas: CanvasSettings::default(),
        }
    }
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}
