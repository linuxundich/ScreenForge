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

/// One of a `Transform`'s four resize handles, in document space (not
/// affected by `rotation_deg` — resize handles operate on the unrotated
/// bounding box, matching `render::compose`'s draw order of scale-then-
/// rotate).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Transform {
    /// Smallest width/height a resize can produce, in document pixels —
    /// keeps a drag from collapsing a screenshot to zero or negative size.
    pub const MIN_SIZE: f64 = 20.0;

    /// The transform that results from dragging `corner` by `(dx, dy)` in
    /// document space, keeping the *opposite* corner fixed in place. When
    /// `aspect_locked` is set, height always follows width to preserve the
    /// original aspect ratio — width is the driving axis regardless of
    /// which direction the drag moved more.
    pub fn resized_from_corner(&self, corner: Corner, dx: f64, dy: f64) -> Transform {
        let (horiz_sign, vert_sign) = match corner {
            Corner::TopLeft => (-1.0, -1.0),
            Corner::TopRight => (1.0, -1.0),
            Corner::BottomLeft => (-1.0, 1.0),
            Corner::BottomRight => (1.0, 1.0),
        };

        let width = (self.width + horiz_sign * dx).max(Self::MIN_SIZE);
        let height = if self.aspect_locked {
            (width * (self.height / self.width.max(Self::MIN_SIZE))).max(Self::MIN_SIZE)
        } else {
            (self.height + vert_sign * dy).max(Self::MIN_SIZE)
        };

        let x = if horiz_sign > 0.0 { self.x } else { self.x + self.width - width };
        let y = if vert_sign > 0.0 { self.y } else { self.y + self.height - height };

        Transform { x, y, width, height, ..*self }
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

    /// This shadow's direction and distance, derived from `offset_x`/
    /// `offset_y` — the polar form the sidebar's "Schatten-Winkel"/
    /// "Schatten-Distanz" controls edit, since a direction+length is a
    /// more direct match for "cast a shadow this way, this far" than
    /// raw x/y. Angle is in degrees, 0..360, using the same
    /// clockwise-from-positive-x convention as `GradientKind::Linear`'s
    /// angle (0° points right, 90° points down).
    pub fn angle_and_distance(&self) -> (f64, f64) {
        let distance = self.offset_x.hypot(self.offset_y);
        let angle = self.offset_y.atan2(self.offset_x).to_degrees();
        (if angle < 0.0 { angle + 360.0 } else { angle }, distance)
    }

    /// The inverse of `angle_and_distance`: the `(offset_x, offset_y)`
    /// pair for a given direction (degrees) and distance (pixels).
    pub fn offset_for_angle_and_distance(angle_deg: f64, distance: f64) -> (f64, f64) {
        let rad = angle_deg.to_radians();
        (distance * rad.cos(), distance * rad.sin())
    }

    /// Applies `preset`'s distance/blur/opacity/color, keeping *this*
    /// shadow's current angle unchanged. This is the fix for the "choosing
    /// a preset resets the angle to 90°" bug: the old code built a whole
    /// new `ShadowParams` from a preset's own hard-coded `offset_x`/
    /// `offset_y` (always straight down), which discarded whatever angle
    /// the user had dialed in. A preset is a statement about *how strong*
    /// a shadow looks, not *which direction* it's cast, so it must only
    /// touch the fields it actually describes.
    pub fn with_preset(&self, preset: ShadowPreset) -> ShadowParams {
        let (angle, _) = self.angle_and_distance();
        let (offset_x, offset_y) = Self::offset_for_angle_and_distance(angle, preset.distance);
        ShadowParams { enabled: preset.enabled(), offset_x, offset_y, blur: preset.blur, opacity: preset.opacity, color: preset.color }
    }
}

impl Default for ShadowParams {
    fn default() -> Self {
        Self::none()
    }
}

/// The "how strong" knobs of a shadow — distance, blur, opacity, color —
/// deliberately without a direction: applying a preset must never overwrite
/// the angle the user chose (see [`ShadowParams::with_preset`]). Mirrors
/// the values `ShadowParams::subtle()`/`standard()`/`strong()`/`floating()`
/// used to hard-code into `offset_x`/`offset_y` at a fixed 90° angle.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ShadowPreset {
    pub distance: f64,
    pub blur: f64,
    pub opacity: f64,
    pub color: Rgba,
}

impl ShadowPreset {
    pub const NONE: ShadowPreset = ShadowPreset { distance: 0.0, blur: 0.0, opacity: 0.0, color: Rgba::BLACK };
    pub const SUBTLE: ShadowPreset = ShadowPreset { distance: 2.0, blur: 8.0, opacity: 0.15, color: Rgba::BLACK };
    pub const STANDARD: ShadowPreset = ShadowPreset { distance: 6.0, blur: 16.0, opacity: 0.25, color: Rgba::BLACK };
    pub const STRONG: ShadowPreset = ShadowPreset { distance: 12.0, blur: 28.0, opacity: 0.4, color: Rgba::BLACK };
    pub const FLOATING: ShadowPreset = ShadowPreset { distance: 24.0, blur: 40.0, opacity: 0.3, color: Rgba::BLACK };

    /// A shadow is "on" once it would actually paint something — mirrors
    /// `ShadowParams::none()` being the only preset with `enabled: false`.
    pub fn enabled(&self) -> bool {
        self.distance > 0.0 || self.blur > 0.0 || self.opacity > 0.0
    }
}

/// Where a [`TextElement`] sits horizontally within its reference rect —
/// the whole canvas for a composition title, or one screenshot's placement
/// for a label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HorizontalAnchor {
    Left,
    Center,
    Right,
}

/// The vertical counterpart of [`HorizontalAnchor`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VerticalAnchor {
    Top,
    Center,
    Bottom,
}

/// Where a [`TextElement`]'s box (its background + padding + text) is
/// placed within its reference rect. `Semantic` is resolved against that
/// rect's *current* width/height wherever it's used (see
/// `TextElement::resolve_box_origin`), never against a stored pixel
/// coordinate — that's what keeps a title correctly positioned when the
/// export size changes, and a label correctly positioned when its
/// screenshot is resized, with nothing needing to be recomputed by the
/// caller.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "kebab-case")]
pub enum TextPosition {
    Semantic { horizontal: HorizontalAnchor, vertical: VerticalAnchor, padding: f64 },
    /// Advanced/manual placement: the box's top-left corner, in the same
    /// coordinate space `Semantic` resolves against (document pixels for a
    /// title, screenshot-local pixels for a label).
    Absolute { x: f64, y: f64 },
}

impl TextPosition {
    /// The top-left corner of a `box_w`×`box_h` box within a
    /// `ref_w`×`ref_h` reference rect, per this position. Pure geometry —
    /// no font/rendering dependency — so both `core::render` and the
    /// canvas/inspector UI can resolve exactly the same point.
    pub fn resolve_box_origin(&self, ref_w: f64, ref_h: f64, box_w: f64, box_h: f64) -> (f64, f64) {
        match *self {
            TextPosition::Absolute { x, y } => (x, y),
            TextPosition::Semantic { horizontal, vertical, padding } => {
                let x = match horizontal {
                    HorizontalAnchor::Left => padding,
                    HorizontalAnchor::Center => (ref_w - box_w) / 2.0,
                    HorizontalAnchor::Right => ref_w - box_w - padding,
                };
                let y = match vertical {
                    VerticalAnchor::Top => padding,
                    VerticalAnchor::Center => (ref_h - box_h) / 2.0,
                    VerticalAnchor::Bottom => ref_h - box_h - padding,
                };
                (x, y)
            }
        }
    }
}

/// Horizontal alignment of text *within* a [`TextElement`]'s own box —
/// distinct from [`HorizontalAnchor`], which places the box itself within
/// the reference rect. Only matters for multi-line content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

/// Typography for one [`TextElement`]. `weight` uses Pango's numeric scale
/// (100..=900; 400 is normal, 700 is bold) rather than a `bool` — Pango
/// natively supports the finer range and the render side hands it straight
/// to `pango::FontDescription::set_weight`, so there's no separate mapping
/// to maintain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Typography {
    pub font_family: String,
    pub font_size: f64,
    pub weight: i32,
    pub italic: bool,
    pub color: Rgba,
    pub alignment: TextAlign,
    /// `0.0..=1.0`, multiplies `color.a` — kept separate so a user can dim
    /// the whole label without having to remember/restore the color's own
    /// alpha.
    pub opacity: f64,
    /// Extra space between characters, in pixels; `0.0` is Pango's normal
    /// spacing.
    pub letter_spacing: f64,
    /// Pango's line-spacing factor; `1.0` is normal, matching a single
    /// line's natural height.
    pub line_spacing: f64,
    /// Whether long content wraps within the box's available width
    /// (`TextElement::wrap_width`) instead of growing the box to fit a
    /// single line.
    pub wrap: bool,
}

impl Typography {
    pub fn title_default() -> Self {
        Self {
            font_family: "Sans".to_string(),
            font_size: 32.0,
            weight: 700,
            italic: false,
            color: Rgba::BLACK,
            alignment: TextAlign::Center,
            opacity: 1.0,
            letter_spacing: 0.0,
            line_spacing: 1.2,
            wrap: false,
        }
    }

    pub fn label_default() -> Self {
        Self {
            font_family: "Sans".to_string(),
            font_size: 18.0,
            weight: 700,
            italic: false,
            color: Rgba::WHITE,
            alignment: TextAlign::Center,
            opacity: 1.0,
            letter_spacing: 0.0,
            line_spacing: 1.2,
            wrap: false,
        }
    }
}

/// A [`TextElement`]'s box background. Reuses [`Rgba`]'s own alpha channel
/// for "solid with adjustable opacity" (matching how [`Background::Solid`]
/// already works) and [`GradientSpec`] for a gradient fill, rather than
/// inventing parallel types.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "kebab-case")]
pub enum TextBackground {
    #[default]
    None,
    Solid(Rgba),
    Gradient(GradientSpec),
}

/// A reusable text/label object (spec: composition title and per-screenshot
/// labels share this one shape rather than being separate hard-coded draw
/// calls). Two positioning *contexts* reuse the same type: a
/// canvas-relative title lives on [`Document`], a screenshot-relative
/// label lives on [`ScreenshotElement`] — which reference rect
/// `position`/`shadow` resolve against is entirely up to the caller
/// (`core::render`), not encoded in this struct itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextElement {
    pub enabled: bool,
    pub content: String,
    pub position: TextPosition,
    pub typography: Typography,
    pub background: TextBackground,
    pub corner_radius: CornerRadius,
    /// Padding, in pixels, between the background box's own edge and the
    /// text inside it — distinct from `TextPosition::Semantic`'s
    /// `padding`, which is the gap between the box and the canvas/
    /// screenshot edge.
    pub background_padding: f64,
    pub shadow: ShadowParams,
}

impl TextElement {
    pub fn title_default() -> Self {
        Self {
            enabled: false,
            content: String::new(),
            position: TextPosition::Semantic { horizontal: HorizontalAnchor::Center, vertical: VerticalAnchor::Top, padding: 32.0 },
            typography: Typography::title_default(),
            background: TextBackground::None,
            corner_radius: CornerRadius::none(),
            background_padding: 16.0,
            shadow: ShadowParams::none(),
        }
    }

    pub fn label_default() -> Self {
        Self {
            enabled: false,
            content: String::new(),
            position: TextPosition::Semantic { horizontal: HorizontalAnchor::Center, vertical: VerticalAnchor::Bottom, padding: 16.0 },
            typography: Typography::label_default(),
            background: TextBackground::None,
            corner_radius: CornerRadius::none(),
            background_padding: 8.0,
            shadow: ShadowParams::none(),
        }
    }

    /// The width (in the same coordinate space as `position`) long content
    /// should wrap within, when `typography.wrap` is set — the reference
    /// rect's own width for `Absolute` placement (no better bound exists),
    /// or the rect's width minus the semantic edge padding on both sides,
    /// so wrapped text never overflows past where the box itself is
    /// anchored.
    pub fn wrap_width(&self, ref_w: f64) -> f64 {
        match self.position {
            TextPosition::Absolute { .. } => ref_w.max(1.0),
            TextPosition::Semantic { padding, .. } => (ref_w - 2.0 * padding).max(1.0),
        }
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
    /// This screenshot's own label (spec §11) — screenshot-relative, so it
    /// moves/scales with the element rather than living on `Document`
    /// alongside the canvas-relative title. `#[serde(default)]` so a
    /// project saved before labels existed still loads, with every
    /// existing screenshot getting a disabled default label.
    #[serde(default = "TextElement::label_default")]
    pub label: TextElement,
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
            label: TextElement::label_default(),
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

/// How a [`GeneratedBackground`]'s palette is derived. `Random` and
/// `Grayscale` ignore the screenshots entirely; `Manual` uses the user's own
/// 4 chosen colors as-is; `FromScreenshots` derives from the currently
/// visible screenshots' dominant color, with `inverse_contrast` sliding
/// between staying close to it and swinging to its complement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ColorStrategy {
    /// The user's own 4 colors, picked directly (`palette` holds exactly
    /// what they chose; nothing is derived).
    Manual,
    /// The screenshots' dominant color, shifted toward its complement by
    /// `inverse_contrast`.
    FromScreenshots,
    /// A pure lightness ramp, no hue.
    Grayscale,
    /// Ignores the screenshots entirely; a fresh aesthetically-valid
    /// palette each time, seeded like everything else here.
    Random,
}

/// A background rendered procedurally from a seed and a handful of
/// parameters, rather than a fixed image or a simple gradient (spec §4).
/// Deliberately holds only *inputs* to generation — `palette`, `seed`, and
/// every numeric knob below — never the resolved vector scene itself: the
/// same inputs always regenerate the identical scene (see
/// `crate::generator::render`), so there's nothing else worth persisting
/// (spec §20/§24 — "the project must be able to regenerate the exact same
/// background", not store a rendered image of it).
///
/// The scene itself is a single algorithm — a fan of nested, wave-perturbed
/// arc layers around a focus point (see `crate::generator::draw_wave_layers`)
/// — rather than a choice of unrelated styles; `corner_bias` alone controls
/// whether that focus point (and thus the whole look) reads as flat wave
/// bands or as nested arcs anchored in a canvas corner.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeneratedBackground {
    pub seed: u64,
    pub color_strategy: ColorStrategy,
    /// The resolved palette this background was last generated with —
    /// derived from `color_strategy` (and the screenshots, unless it's
    /// `Random`) at the moment Generate/Regenerate was pressed. Kept
    /// explicit rather than re-derived from the live document on every
    /// render, so the background stays stable while editing unrelated
    /// things, and so `adapt_to_screenshots = false` really means
    /// "frozen" (spec §21 "Lock Background"). For `ColorStrategy::Manual`
    /// this *is* the user's input, not a derived value.
    pub palette: Vec<Rgba>,
    /// When true, changing which screenshots are visible re-resolves
    /// `palette` (and re-renders) automatically; when false, the palette
    /// stays exactly as generated until the user explicitly regenerates
    /// (spec §22). Meaningless for `Manual`/`Random`, which never derive
    /// from the screenshots in the first place.
    pub adapt_to_screenshots: bool,
    /// `0.0..=1.0` — how strongly `ColorStrategy::FromScreenshots` contrasts
    /// against, rather than matches, the screenshots' own colors (spec §2).
    pub inverse_contrast: f64,
    /// `0.0..=1.0` generation knobs — see `GeneratedBackground::new` for
    /// sensible defaults.
    pub density: f64,
    pub flow: f64,
    pub variation: f64,
    pub contrast: f64,
    pub softness: f64,
    /// `0.0..=1.0` — 0 reads as flat, near-parallel wave bands; 1 as nested
    /// arcs anchored in a canvas corner; in between, a continuous morph
    /// between the two. See `crate::generator::draw_wave_layers`.
    pub corner_bias: f64,
    /// `-1.0..=1.0` — shifts the pattern's focus point horizontally
    /// (`-1.0`/`1.0` move it a full half-canvas-width left/right), letting
    /// the user recenter it without touching `corner_bias`'s wave-vs-arc
    /// morph. `#[serde(default)]` so a project saved before this field
    /// existed still loads, with the pattern staying exactly where it was
    /// (`0.0` is "unmoved").
    #[serde(default)]
    pub offset_x: f64,
    /// The vertical counterpart of `offset_x`.
    #[serde(default)]
    pub offset_y: f64,
    /// `>0.0` — zooms the whole pattern in (`> 1.0`) or out (`< 1.0`) around
    /// its focus point; `1.0` is unscaled. `#[serde(default = "..")]` so a
    /// project saved before this field existed still loads at its original
    /// size.
    #[serde(default = "default_generated_scale")]
    pub scale: f64,
}

fn default_generated_scale() -> f64 {
    1.0
}

impl GeneratedBackground {
    /// A fresh background with reasonable defaults — restrained enough
    /// that the very first "Generate" click already looks intentional,
    /// not just technically valid.
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            color_strategy: ColorStrategy::FromScreenshots,
            palette: Vec::new(),
            adapt_to_screenshots: true,
            inverse_contrast: 0.5,
            density: 0.4,
            flow: 0.5,
            variation: 0.4,
            contrast: 0.5,
            softness: 0.6,
            corner_bias: 0.5,
            offset_x: 0.0,
            offset_y: 0.0,
            scale: 1.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "kebab-case")]
pub enum Background {
    Solid(Rgba),
    Gradient(GradientSpec),
    Image(ImageBackgroundSpec),
    Generated(GeneratedBackground),
}

impl Default for Background {
    fn default() -> Self {
        Background::Gradient(GradientSpec {
            kind: GradientKind::Linear { angle_deg: 135.0 },
            stops: vec![(0.0, Rgba::new(0.86, 0.90, 0.97, 1.0)), (1.0, Rgba::new(0.98, 0.98, 0.99, 1.0))],
        })
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
    Avif,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CanvasSettings {
    /// The composition's native size in content pixels — always kept equal
    /// to the current layout's actual extent (every visible element plus
    /// spacing/margin) by [`crate::layout::fit_canvas_to_content`], called
    /// after every undo-tracked mutation and on project load. Not
    /// user-editable: there's deliberately no UI control bound to these
    /// two fields directly, since editing them independently of the
    /// content is exactly what used to let content get cropped off.
    pub export_width: u32,
    pub export_height: u32,
    /// The user-facing export knob: renders the composition scaled so its
    /// width equals this, height following proportionally so nothing is
    /// ever stretched or cropped (`#[serde(default)]` so a project saved
    /// before this field existed still loads, falling back to no scaling
    /// relative to whatever `export_width` it carries).
    #[serde(default = "default_export_target_width")]
    pub export_target_width: u32,
    pub export_format: ExportFormat,
    /// `0..=100`, ignored for [`ExportFormat::Png`] (lossless).
    pub export_quality: u8,
}

fn default_export_target_width() -> u32 {
    1920
}

impl Default for CanvasSettings {
    fn default() -> Self {
        Self {
            export_width: 1920,
            export_height: 1080,
            export_target_width: default_export_target_width(),
            export_format: ExportFormat::Png,
            export_quality: 90,
        }
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
    /// The composition-wide title (spec §5) — canvas-relative; see
    /// [`ScreenshotElement::label`] for the screenshot-relative kind.
    /// `#[serde(default)]` so a project saved before titles existed, or
    /// one saved with the older single-purpose `TextOverlay`, still loads
    /// (with no title rather than a load error — the older overlay's
    /// content isn't migrated, since its shape doesn't map cleanly onto
    /// `TextElement`'s).
    #[serde(default = "TextElement::title_default")]
    pub title: TextElement,
}

impl Document {
    pub fn new() -> Self {
        Self {
            id: Uuid::new_v4(),
            elements: Vec::new(),
            layout: LayoutSettings::default(),
            background: Background::default(),
            canvas: CanvasSettings::default(),
            title: TextElement::title_default(),
        }
    }
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Transform {
        Transform { x: 100.0, y: 100.0, width: 200.0, height: 100.0, aspect_locked: false, ..Transform::default() }
    }

    #[test]
    fn bottom_right_grows_size_and_keeps_top_left_anchored() {
        let resized = fixture().resized_from_corner(Corner::BottomRight, 20.0, 30.0);
        assert_eq!(resized.x, 100.0);
        assert_eq!(resized.y, 100.0);
        assert_eq!(resized.width, 220.0);
        assert_eq!(resized.height, 130.0);
    }

    #[test]
    fn top_left_shrinks_size_and_keeps_bottom_right_anchored() {
        let original = fixture();
        let resized = original.resized_from_corner(Corner::TopLeft, 20.0, 10.0);
        assert_eq!(resized.width, 180.0);
        assert_eq!(resized.height, 90.0);
        assert_eq!(resized.x, 120.0);
        assert_eq!(resized.y, 110.0);
        // The opposite corner (bottom-right) stays at the same document point.
        assert_eq!(resized.x + resized.width, original.x + original.width);
        assert_eq!(resized.y + resized.height, original.y + original.height);
    }

    #[test]
    fn top_right_and_bottom_left_anchor_their_own_opposite_corner() {
        let original = fixture();

        let top_right = original.resized_from_corner(Corner::TopRight, 20.0, 10.0);
        assert_eq!(top_right.x, 100.0);
        assert_eq!(top_right.width, 220.0);
        assert_eq!(top_right.height, 90.0);
        assert_eq!(top_right.y, 110.0);
        assert_eq!(top_right.y + top_right.height, original.y + original.height);

        let bottom_left = original.resized_from_corner(Corner::BottomLeft, 20.0, 10.0);
        assert_eq!(bottom_left.y, 100.0);
        assert_eq!(bottom_left.height, 110.0);
        assert_eq!(bottom_left.width, 180.0);
        assert_eq!(bottom_left.x, 120.0);
        assert_eq!(bottom_left.x + bottom_left.width, original.x + original.width);
    }

    #[test]
    fn resize_never_shrinks_below_the_minimum_size() {
        let resized = fixture().resized_from_corner(Corner::BottomRight, -9999.0, -9999.0);
        assert_eq!(resized.width, Transform::MIN_SIZE);
        assert_eq!(resized.height, Transform::MIN_SIZE);
    }

    #[test]
    fn aspect_locked_derives_height_from_width_regardless_of_dy() {
        let mut original = fixture();
        original.aspect_locked = true; // 200x100, aspect 2:1
        let resized = original.resized_from_corner(Corner::BottomRight, 40.0, 9999.0);
        assert_eq!(resized.width, 240.0);
        assert_eq!(resized.height, 120.0);
    }

    #[test]
    fn shadow_angle_and_distance_matches_known_offsets() {
        // 0° points right (+x), 90° points down (+y) -- same convention as
        // GradientKind::Linear's angle.
        let mut shadow = ShadowParams::none();
        shadow.offset_x = 10.0;
        shadow.offset_y = 0.0;
        assert_eq!(shadow.angle_and_distance(), (0.0, 10.0));

        shadow.offset_x = 0.0;
        shadow.offset_y = 10.0;
        assert_eq!(shadow.angle_and_distance(), (90.0, 10.0));

        shadow.offset_x = -10.0;
        shadow.offset_y = 0.0;
        assert_eq!(shadow.angle_and_distance(), (180.0, 10.0));

        // atan2 alone would give a negative angle here -- confirms the
        // 0..360 normalization.
        shadow.offset_x = 0.0;
        shadow.offset_y = -10.0;
        assert_eq!(shadow.angle_and_distance(), (270.0, 10.0));
    }

    #[test]
    fn shadow_offset_for_angle_and_distance_round_trips() {
        for (angle, distance) in [(0.0, 12.0), (45.0, 20.0), (90.0, 6.0), (200.0, 50.0), (359.0, 8.0)] {
            let (x, y) = ShadowParams::offset_for_angle_and_distance(angle, distance);
            let mut shadow = ShadowParams::none();
            shadow.offset_x = x;
            shadow.offset_y = y;
            let (round_tripped_angle, round_tripped_distance) = shadow.angle_and_distance();
            assert!((round_tripped_angle - angle).abs() < 1e-9, "angle: {round_tripped_angle} vs {angle}");
            assert!((round_tripped_distance - distance).abs() < 1e-9, "distance: {round_tripped_distance} vs {distance}");
        }
    }

    #[test]
    fn shadow_zero_distance_has_no_particular_angle_but_does_not_panic() {
        let shadow = ShadowParams::none();
        let (_, distance) = shadow.angle_and_distance();
        assert_eq!(distance, 0.0);
    }

    /// Regression test for the "changing the shadow preset resets the
    /// angle to 90°" bug: a custom angle must survive every preset switch.
    #[test]
    fn with_preset_preserves_a_custom_angle_across_every_preset() {
        let mut shadow = ShadowParams::none();
        shadow.offset_x = ShadowParams::offset_for_angle_and_distance(135.0, 6.0).0;
        shadow.offset_y = ShadowParams::offset_for_angle_and_distance(135.0, 6.0).1;

        for preset in [ShadowPreset::SUBTLE, ShadowPreset::STANDARD, ShadowPreset::STRONG, ShadowPreset::FLOATING] {
            shadow = shadow.with_preset(preset);
            let (angle, _) = shadow.angle_and_distance();
            assert!((angle - 135.0).abs() < 1e-9, "expected angle to stay 135°, got {angle}");
        }
    }

    #[test]
    fn with_preset_applies_the_presets_own_blur_opacity_and_color() {
        let shadow = ShadowParams::none().with_preset(ShadowPreset::STRONG);
        assert_eq!(shadow.blur, ShadowPreset::STRONG.blur);
        assert_eq!(shadow.opacity, ShadowPreset::STRONG.opacity);
        assert_eq!(shadow.color, ShadowPreset::STRONG.color);
        assert!(shadow.enabled);
    }

    #[test]
    fn with_preset_none_disables_the_shadow() {
        let shadow = ShadowParams::strong().with_preset(ShadowPreset::NONE);
        assert!(!shadow.enabled);
    }

    #[test]
    fn semantic_position_places_the_box_at_each_named_corner() {
        let (ref_w, ref_h, box_w, box_h, padding) = (1000.0, 800.0, 200.0, 100.0, 20.0);

        let top_left = TextPosition::Semantic { horizontal: HorizontalAnchor::Left, vertical: VerticalAnchor::Top, padding };
        assert_eq!(top_left.resolve_box_origin(ref_w, ref_h, box_w, box_h), (20.0, 20.0));

        let bottom_right = TextPosition::Semantic { horizontal: HorizontalAnchor::Right, vertical: VerticalAnchor::Bottom, padding };
        assert_eq!(bottom_right.resolve_box_origin(ref_w, ref_h, box_w, box_h), (1000.0 - 200.0 - 20.0, 800.0 - 100.0 - 20.0));

        let center = TextPosition::Semantic { horizontal: HorizontalAnchor::Center, vertical: VerticalAnchor::Center, padding };
        assert_eq!(center.resolve_box_origin(ref_w, ref_h, box_w, box_h), ((1000.0 - 200.0) / 2.0, (800.0 - 100.0) / 2.0));
    }

    /// The core of spec §16 "responsive positioning": the same semantic
    /// position must resolve to a *different* absolute point when the
    /// reference rect (export size, or a resized screenshot) changes size
    /// — this is what keeps a title centered/padded correctly rather than
    /// staying at a stale pixel coordinate.
    #[test]
    fn semantic_position_tracks_a_changing_reference_rect_size() {
        let position = TextPosition::Semantic { horizontal: HorizontalAnchor::Center, vertical: VerticalAnchor::Top, padding: 32.0 };

        let at_1080p = position.resolve_box_origin(1920.0, 1080.0, 400.0, 80.0);
        let at_4k = position.resolve_box_origin(3840.0, 2160.0, 400.0, 80.0);

        assert_eq!(at_1080p, ((1920.0 - 400.0) / 2.0, 32.0));
        assert_eq!(at_4k, ((3840.0 - 400.0) / 2.0, 32.0));
        assert_ne!(at_1080p.0, at_4k.0, "centering must follow the new width, not stay at the old absolute x");
    }

    #[test]
    fn absolute_position_ignores_the_reference_rect_entirely() {
        let position = TextPosition::Absolute { x: 42.0, y: 17.0 };
        assert_eq!(position.resolve_box_origin(100.0, 100.0, 30.0, 30.0), (42.0, 17.0));
        assert_eq!(position.resolve_box_origin(5000.0, 5000.0, 30.0, 30.0), (42.0, 17.0));
    }

    #[test]
    fn wrap_width_is_bounded_by_semantic_padding_on_both_sides() {
        let el = TextElement { position: TextPosition::Semantic { horizontal: HorizontalAnchor::Center, vertical: VerticalAnchor::Top, padding: 50.0 }, ..TextElement::title_default() };
        assert_eq!(el.wrap_width(1000.0), 900.0);
    }

    #[test]
    fn wrap_width_for_absolute_position_falls_back_to_the_full_reference_width() {
        let el = TextElement { position: TextPosition::Absolute { x: 10.0, y: 10.0 }, ..TextElement::title_default() };
        assert_eq!(el.wrap_width(1000.0), 1000.0);
    }

    #[test]
    fn screenshot_element_gets_a_disabled_default_label() {
        let el = ScreenshotElement::new(ImageSource::Path(std::path::PathBuf::from("a.png")), 100.0, 200.0);
        assert!(!el.label.enabled);
        assert_eq!(el.label.content, "");
    }

    #[test]
    fn document_gets_a_disabled_default_title() {
        let doc = Document::new();
        assert!(!doc.title.enabled);
    }
}
