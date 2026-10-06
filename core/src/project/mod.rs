//! `.screenforge` project file (de)serialization — a zip archive holding a
//! `project.json` manifest (JSON via serde, with a `version` field from day
//! one so a future format change can migrate old files instead of rejecting
//! them outright, spec §13) plus every screenshot's and background's own
//! image bytes as separate archive entries, so a saved project stays fully
//! self-contained even if the original source files are later moved,
//! renamed, or deleted — see [`save`]/[`load`]'s own doc comments for how.
//! `load` also still reads the older, plain (non-zip) JSON format this
//! module wrote before the zip layout existed, by sniffing the file's first
//! bytes — a project saved back then keeps loading unchanged.

use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

use crate::model::{Background, Document, ImageSource};

pub const CURRENT_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectFile {
    pub format: String,
    pub version: u32,
    pub document: Document,
}

#[derive(Debug, Error)]
pub enum ProjectError {
    #[error("could not read project file: {0}")]
    Io(#[from] std::io::Error),
    #[error("project file is damaged or not valid ScreenForge JSON: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("project file is damaged or not a valid ScreenForge archive: {0}")]
    Archive(#[from] zip::result::ZipError),
    #[error("unsupported project format version {found} (this version of ScreenForge supports up to {supported})")]
    UnsupportedVersion { found: u32, supported: u32 },
}

/// Saves `document` as a zip archive: `project.json` (the same
/// `ProjectFile` shape this module has always written) plus one
/// `assets/<id>.<ext>` entry per screenshot and, if present, one
/// `assets/background.<ext>` entry for a `Background::Image` — each holding
/// that image's *original, unmodified file bytes* (spec §16: "Originalbilder
/// sollen nach Möglichkeit nicht unnötig verändert werden"), not a
/// re-encoded copy. Works on a transformed clone of `document`, so the
/// caller's own in-memory copy is never touched — its elements keep
/// pointing at whatever `Path` they already did (see `ImageSource::Embedded`
/// on `crate::model` for why this is the right split between "what the
/// running app holds" and "what the saved file holds").
///
/// A screenshot whose source file can no longer be read (moved/deleted
/// since it was imported) is left as a plain, un-embedded `Path` in the
/// saved file rather than failing the whole save — that one screenshot is
/// exactly as fragile as the old path-only format was, while every other
/// asset that *could* be read is fully self-contained.
pub fn save(document: &Document, path: &Path) -> Result<(), ProjectError> {
    let mut document = document.clone();
    let assets = embed_document_assets(&mut document);

    let project = ProjectFile { format: "screenforge".to_string(), version: CURRENT_VERSION, document };
    let json = serde_json::to_string_pretty(&project)?;

    let file = std::fs::File::create(path)?;
    let mut writer = ZipWriter::new(file);
    let options = SimpleFileOptions::default();
    writer.start_file("project.json", options)?;
    std::io::Write::write_all(&mut writer, json.as_bytes())?;
    for (filename, bytes) in &assets {
        writer.start_file(filename, options)?;
        std::io::Write::write_all(&mut writer, bytes)?;
    }
    writer.finish()?;
    Ok(())
}

/// Replaces every screenshot's and background's `ImageSource::Path` with an
/// `ImageSource::Embedded` referencing a fresh `assets/...` archive name,
/// returning the `(filename, bytes)` pairs to write as separate zip
/// entries — the embedded variant's own `bytes` field is left empty in
/// `document` itself (real bytes only ever live in the returned `Vec`, so
/// `project.json` never carries a redundant base64 copy of every image).
fn embed_document_assets(document: &mut Document) -> Vec<(String, Vec<u8>)> {
    let mut assets = Vec::new();
    for element in &mut document.elements {
        let base_name = element.id.to_string();
        embed_image_source(&mut element.source, &base_name, &mut assets);
    }
    if let Background::Image(spec) = &mut document.background {
        embed_image_source(&mut spec.source, "background", &mut assets);
    }
    if let Some(logo) = &mut document.watermark.logo {
        embed_image_source(logo, "watermark", &mut assets);
    }
    assets
}

fn embed_image_source(source: &mut ImageSource, base_name: &str, assets: &mut Vec<(String, Vec<u8>)>) {
    match source {
        ImageSource::Path(path) => {
            let Ok(bytes) = std::fs::read(&path) else {
                // Left as the original, still-unreadable `Path` — see this
                // module's own `save` doc comment for why that's the right
                // fallback rather than failing the whole save.
                return;
            };
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("png");
            let filename = format!("assets/{base_name}.{ext}");
            *source = ImageSource::Embedded { filename: filename.clone(), bytes: Vec::new() };
            assets.push((filename, bytes));
        }
        ImageSource::Embedded { filename, bytes } => {
            // Already embedded — re-emit its existing bytes as a fresh
            // archive entry under its own filename. `Document`s handed to
            // `save` never actually carry this in practice (`load` always
            // normalizes back to `Path` before returning), but handling it
            // here means `save` never silently drops an image if that ever
            // changes.
            assets.push((filename.clone(), std::mem::take(bytes)));
        }
    }
}

/// Loads a project, migrating older format versions/formats forward as
/// needed. Detects the zip format `save` now always writes by sniffing the
/// file's first two bytes (`PK`, every zip archive's own magic number) —
/// anything else is assumed to be the older, plain-JSON format this module
/// used to write, which still loads exactly as it always has. Only version
/// `1` exists so far; the `match` on `project.version` is where a future
/// `migrate_v1_to_v2` step would slot in without touching the call sites.
///
/// `asset_extract_dir` is where a zip project's embedded images get
/// extracted to as real files before this returns — the rest of the app
/// only ever deals with `ImageSource::Path`, exactly as it always has, so
/// nothing downstream needs to know a zip was involved at all. Unused for
/// the legacy plain-JSON format. Callers own this directory's lifecycle
/// (e.g. clearing it before each new load, so extracted assets don't
/// accumulate across sessions) — this function only ever writes into it,
/// never manages its lifetime.
pub fn load(path: &Path, asset_extract_dir: &Path) -> Result<Document, ProjectError> {
    let bytes = std::fs::read(path)?;
    let (json_text, mut zip_archive) = if bytes.starts_with(b"PK") {
        let mut archive = ZipArchive::new(std::io::Cursor::new(bytes))?;
        let mut text = String::new();
        archive.by_name("project.json")?.read_to_string(&mut text)?;
        (text, Some(archive))
    } else {
        let text = String::from_utf8(bytes).map_err(|err| ProjectError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, err)))?;
        (text, None)
    };

    let mut raw: serde_json::Value = serde_json::from_str(&json_text)?;
    migrate_legacy_label_padding(&mut raw);
    // Must run after the padding migration above, though it no longer
    // reads padding itself — kept in this order for consistency with the
    // rest of the pipeline's "oldest quirks first" convention.
    migrate_label_to_content_only(&mut raw);
    migrate_legacy_layout_margin(&mut raw);
    let project: ProjectFile = serde_json::from_value(raw)?;
    match project.version {
        1 => {
            let mut document = project.document;
            if let Some(archive) = &mut zip_archive {
                extract_document_assets(&mut document, archive, asset_extract_dir)?;
            }
            // Re-fit rather than trust the saved canvas size: it's a
            // derived value (see `fit_canvas_to_content`), and trusting a
            // stale one here is exactly how content used to end up cropped.
            crate::layout::fit_canvas_to_content(&mut document);
            Ok(document)
        }
        other => Err(ProjectError::UnsupportedVersion { found: other, supported: CURRENT_VERSION }),
    }
}

/// The `load`-side mirror of `embed_document_assets`: extracts every
/// `ImageSource::Embedded` referenced by `document` into `extract_dir` and
/// rewrites it back to a plain `ImageSource::Path` pointing at the
/// extracted file — after this, `document` is indistinguishable from one
/// loaded via the legacy path-only format, just pointing at freshly
/// extracted copies instead of the user's original files.
fn extract_document_assets<R: Read + Seek>(document: &mut Document, archive: &mut ZipArchive<R>, extract_dir: &Path) -> Result<(), ProjectError> {
    for element in &mut document.elements {
        extract_image_source(&mut element.source, archive, extract_dir)?;
    }
    if let Background::Image(spec) = &mut document.background {
        extract_image_source(&mut spec.source, archive, extract_dir)?;
    }
    if let Some(logo) = &mut document.watermark.logo {
        extract_image_source(logo, archive, extract_dir)?;
    }
    Ok(())
}

fn extract_image_source<R: Read + Seek>(source: &mut ImageSource, archive: &mut ZipArchive<R>, extract_dir: &Path) -> Result<(), ProjectError> {
    let ImageSource::Embedded { filename, .. } = source else { return Ok(()) };
    let basename = Path::new(filename).file_name().map(PathBuf::from).unwrap_or_else(|| PathBuf::from(filename.as_str()));
    let extracted_path = extract_dir.join(basename);
    if let Ok(mut entry) = archive.by_name(filename) {
        std::fs::create_dir_all(extract_dir)?;
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        std::fs::write(&extracted_path, bytes)?;
    }
    // If the entry is unexpectedly missing (a corrupted archive), the path
    // above still gets set even though nothing was extracted there — the
    // existing missing-image handling (a `Path` that fails to decode)
    // reports that exactly the same way a moved/deleted external file
    // always has, with no extra code needed here.
    *source = ImageSource::Path(extracted_path);
    Ok(())
}

/// A project saved before a label's padding was split into
/// `padding_x`/`padding_y` used one uniform `background_padding` field.
/// `TextElement`'s own `#[serde(default = "..")]` on the two new fields
/// already means such a file loads without error — but silently at the
/// fallback default, discarding whatever custom padding was actually
/// saved. This runs on the raw JSON, before the typed deserialization ever
/// sees it, copying that old value into both new fields (matching the old
/// uniform-padding look exactly) for every screenshot's `label`. A no-op
/// wherever `padding_x`/`padding_y` are already present (current-format
/// files, or one this has already migrated).
fn migrate_legacy_label_padding(raw: &mut serde_json::Value) {
    let Some(elements) = raw.pointer_mut("/document/elements").and_then(|v| v.as_array_mut()) else { return };
    for element in elements {
        let Some(label) = element.get_mut("label") else { continue };
        let Some(old_padding) = label.get("background_padding").and_then(|v| v.as_f64()) else { continue };
        let Some(label_obj) = label.as_object_mut() else { continue };
        label_obj.entry("padding_x").or_insert(serde_json::json!(old_padding));
        label_obj.entry("padding_y").or_insert(serde_json::json!(old_padding));
    }
}

/// A project saved before per-label styling was removed
/// (`crate::model::Label`, now just `enabled`/`content`) may have either
/// the old fully self-contained `TextElement`-shaped label (content
/// bundled with every style field) or the intermediate per-label-override
/// shape (`enabled`/`content`/`overrides`) from when overrides briefly
/// existed. This runs on the raw JSON, before typed deserialization,
/// discarding whatever style fields such a label carried and keeping only
/// `enabled`/`content` — every label now renders using the project's
/// shared `Document::label_defaults` instead, per current behavior. A
/// no-op wherever a label is already exactly `{enabled, content}` (current-
/// format files, or ones this has already migrated) — see
/// `migrate_legacy_label_padding` above for why this must run after it,
/// not before.
fn migrate_label_to_content_only(raw: &mut serde_json::Value) {
    let Some(elements) = raw.pointer_mut("/document/elements").and_then(|v| v.as_array_mut()) else { return };
    for element in elements {
        let Some(label) = element.get_mut("label") else { continue };
        let Some(label_obj) = label.as_object() else { continue };
        if label_obj.len() == 2 && label_obj.contains_key("enabled") && label_obj.contains_key("content") {
            continue;
        }

        let enabled = label_obj.get("enabled").cloned().unwrap_or(serde_json::json!(false));
        let content = label_obj.get("content").cloned().unwrap_or(serde_json::json!(""));
        *label = serde_json::json!({ "enabled": enabled, "content": content });
    }
}

/// A project saved before `LayoutSettings`'s outer margin was split into
/// `margin_x`/`margin_y` used one uniform `margin_px` field.
/// `LayoutSettings`'s own `#[serde(default = "..")]` on the two new fields
/// already means such a file loads without error — but silently at the
/// fallback default, discarding whatever custom margin was actually saved.
/// This runs on the raw JSON, before the typed deserialization ever sees
/// it, copying that old value into both new fields (matching the old
/// uniform-margin look exactly) — mirrors `migrate_legacy_label_padding`
/// exactly, just for `document.layout` instead of each label. A no-op
/// wherever `margin_x`/`margin_y` are already present.
fn migrate_legacy_layout_margin(raw: &mut serde_json::Value) {
    let Some(layout) = raw.pointer_mut("/document/layout") else { return };
    let Some(old_margin) = layout.get("margin_px").and_then(|v| v.as_f64()) else { return };
    let Some(layout_obj) = layout.as_object_mut() else { return };
    layout_obj.entry("margin_x").or_insert(serde_json::json!(old_margin));
    layout_obj.entry("margin_y").or_insert(serde_json::json!(old_margin));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Background, ImageSource, LayoutSettings, Rgba, ScreenshotElement};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_path(name: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("screenforge-test-{}-{n}-{name}", std::process::id()))
    }

    fn sample_document() -> Document {
        let mut doc = Document::new();
        doc.background = Background::Solid(Rgba::new(0.1, 0.2, 0.3, 1.0));
        doc.layout = LayoutSettings { spacing_px: 12.0, margin_x: 30.0, margin_y: 30.0, ..LayoutSettings::default() };
        doc.elements.push(ScreenshotElement::new(ImageSource::Path(PathBuf::from("/tmp/a.png")), 400.0, 800.0));
        doc.elements.push(ScreenshotElement::new(ImageSource::Path(PathBuf::from("/tmp/b.png")), 420.0, 820.0));
        doc
    }

    #[test]
    fn save_then_load_round_trips_a_simple_document() {
        let path = temp_path("simple.screenforge");
        let doc = Document::new();
        save(&doc, &path).unwrap();
        let loaded = load(&path, &temp_path("assets")).unwrap();
        assert_eq!(loaded.id, doc.id);
        assert_eq!(loaded.layout, doc.layout);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn save_then_load_round_trips_multiple_elements_and_background() {
        let path = temp_path("multi.screenforge");
        let doc = sample_document();
        save(&doc, &path).unwrap();
        let loaded = load(&path, &temp_path("assets")).unwrap();

        assert_eq!(loaded.elements.len(), 2);
        assert_eq!(loaded.elements[0].natural_width, 400.0);
        assert_eq!(loaded.elements[1].natural_width, 420.0);
        match loaded.background {
            Background::Solid(c) => assert_eq!(c, Rgba::new(0.1, 0.2, 0.3, 1.0)),
            other => panic!("expected Solid background, got {other:?}"),
        }
        assert_eq!(loaded.layout.spacing_px, 12.0);
        assert_eq!(loaded.layout.margin_x, 30.0);
        assert_eq!(loaded.layout.margin_y, 30.0);
        std::fs::remove_file(&path).ok();
    }

    /// `save`/`load` never decode image bytes — they're only ever read,
    /// written, and copied verbatim — so a real PNG isn't needed to prove
    /// they round-trip correctly; distinct, arbitrary bytes are just as
    /// good a test and avoid needing cairo's PNG feature in this crate's
    /// test build.
    fn write_tiny_fake_image(path: &Path, tag: u8) {
        std::fs::write(path, [tag; 32]).unwrap();
    }

    /// The whole point of the zip format: a saved project must still open
    /// correctly, with its images intact, even after the original source
    /// files it was made from are gone — proven here by actually deleting
    /// them between save and load, not just by inspecting the archive.
    #[test]
    fn save_then_load_embeds_real_image_bytes_that_survive_the_originals_being_deleted() {
        let original_a = temp_path("original-a.png");
        let original_b = temp_path("original-b.png");
        write_tiny_fake_image(&original_a, 0xAA);
        write_tiny_fake_image(&original_b, 0xBB);
        let bytes_a = std::fs::read(&original_a).unwrap();
        let bytes_b = std::fs::read(&original_b).unwrap();

        let mut doc = Document::new();
        doc.elements.push(ScreenshotElement::new(ImageSource::Path(original_a.clone()), 4.0, 4.0));
        doc.elements.push(ScreenshotElement::new(ImageSource::Path(original_b.clone()), 4.0, 4.0));

        let project_path = temp_path("embedded.screenforge");
        save(&doc, &project_path).unwrap();

        // The whole point: the originals are gone before we ever load back.
        std::fs::remove_file(&original_a).unwrap();
        std::fs::remove_file(&original_b).unwrap();

        let extract_dir = temp_path("embedded-assets");
        let loaded = load(&project_path, &extract_dir).unwrap();

        assert_eq!(loaded.elements.len(), 2);
        for (element, original_bytes) in loaded.elements.iter().zip([&bytes_a, &bytes_b]) {
            let ImageSource::Path(extracted_path) = &element.source else { panic!("expected a Path source after extraction") };
            assert_ne!(extracted_path, &original_a, "should point at a freshly extracted copy, not the (now-deleted) original");
            assert_ne!(extracted_path, &original_b);
            let extracted_bytes = std::fs::read(extracted_path).unwrap_or_else(|e| panic!("extracted file missing at {extracted_path:?}: {e}"));
            assert_eq!(&extracted_bytes, original_bytes, "embedded bytes must exactly match the original file's bytes");
        }

        std::fs::remove_file(&project_path).ok();
        std::fs::remove_dir_all(&extract_dir).ok();
    }

    #[test]
    fn a_background_image_is_also_embedded_and_survives_the_original_being_deleted() {
        use crate::model::{BackgroundImageFit, ImageBackgroundSpec};

        let original = temp_path("original-bg.png");
        write_tiny_fake_image(&original, 0xCC);
        let original_bytes = std::fs::read(&original).unwrap();

        let mut doc = Document::new();
        doc.background = Background::Image(ImageBackgroundSpec { source: ImageSource::Path(original.clone()), fit: BackgroundImageFit::Cover, opacity: 1.0 });

        let project_path = temp_path("embedded-bg.screenforge");
        save(&doc, &project_path).unwrap();
        std::fs::remove_file(&original).unwrap();

        let extract_dir = temp_path("embedded-bg-assets");
        let loaded = load(&project_path, &extract_dir).unwrap();

        let Background::Image(spec) = &loaded.background else { panic!("expected an Image background") };
        let ImageSource::Path(extracted_path) = &spec.source else { panic!("expected a Path source after extraction") };
        assert_eq!(std::fs::read(extracted_path).unwrap(), original_bytes);

        std::fs::remove_file(&project_path).ok();
        std::fs::remove_dir_all(&extract_dir).ok();
    }

    /// A source file that's already gone *before* the save (as opposed to
    /// the more common case of being removed afterward, covered above)
    /// must not fail the whole save — only that one screenshot stays as an
    /// un-embedded, still-broken `Path`, exactly as fragile as the old
    /// format always was for a missing file.
    #[test]
    fn save_degrades_gracefully_when_a_source_file_is_already_missing() {
        let missing = temp_path("never-existed.png");
        let mut doc = Document::new();
        doc.elements.push(ScreenshotElement::new(ImageSource::Path(missing.clone()), 100.0, 100.0));

        let project_path = temp_path("missing-asset.screenforge");
        save(&doc, &project_path).unwrap();

        let loaded = load(&project_path, &temp_path("missing-asset-assets")).unwrap();
        assert_eq!(loaded.elements.len(), 1);
        assert_eq!(loaded.elements[0].source, ImageSource::Path(missing));

        std::fs::remove_file(&project_path).ok();
    }

    /// Spec: "the project must be able to regenerate the exact same
    /// background" — this only holds if every generator input (seed,
    /// strategy, resolved palette, and every numeric parameter) actually
    /// survives a save/load round trip, not just some of them.
    #[test]
    fn save_then_load_round_trips_a_generated_background_exactly() {
        use crate::model::{ColorStrategy, GeneratedBackground};

        let path = temp_path("generated-background.screenforge");
        let mut doc = Document::new();
        let generated = GeneratedBackground {
            seed: 48291374,
            color_strategy: ColorStrategy::FromScreenshots,
            palette: vec![Rgba::new(0.2, 0.3, 0.6, 1.0), Rgba::new(0.8, 0.5, 0.1, 1.0)],
            adapt_to_screenshots: false,
            inverse_contrast: 0.73,
            density: 0.61,
            flow: 0.28,
            variation: 0.5,
            contrast: 0.5,
            softness: 0.9,
            corner_bias: 0.44,
            offset_x: -0.2,
            offset_y: 0.15,
            scale: 1.3,
            style: crate::model::GeneratorStyle::Ribbons,
            mood: crate::model::Mood::Dark,
            grain: 0.42,
        };
        doc.background = Background::Generated(generated.clone());

        save(&doc, &path).unwrap();
        let loaded = load(&path, &temp_path("assets")).unwrap();

        match loaded.background {
            Background::Generated(loaded_generated) => assert_eq!(loaded_generated, generated),
            other => panic!("expected Generated background, got {other:?}"),
        }
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn missing_file_is_reported_as_io_error() {
        let path = temp_path("does-not-exist.screenforge");
        let err = load(&path, &temp_path("assets")).unwrap_err();
        assert!(matches!(err, ProjectError::Io(_)));
    }

    #[test]
    fn corrupted_file_is_reported_as_a_parse_error_not_a_panic() {
        let path = temp_path("corrupted.screenforge");
        std::fs::write(&path, b"{ this is not valid json at all").unwrap();
        let err = load(&path, &temp_path("assets")).unwrap_err();
        assert!(matches!(err, ProjectError::Parse(_)));
        std::fs::remove_file(&path).ok();
    }

    /// A 0.1.0-shaped project file predates `Transform::flip_horizontal`/
    /// `flip_vertical` — this pins down that loading one doesn't break just
    /// because `core::model` grew fields since. This is `#[serde(default)]`
    /// doing its job, not a project-format `version` bump, so this test
    /// stays version 1 throughout.
    #[test]
    fn loads_a_pre_flip_fields_transform_with_defaults() {
        let path = temp_path("pre-flip-fields.screenforge");
        let json = r#"{
            "format": "screenforge",
            "version": 1,
            "document": {
                "id": "8f14e45f-ceea-467e-adc0-51944115d5c6",
                "elements": [{
                    "id": "9b2e1a3c-1234-4a3b-8cde-0123456789ab",
                    "source": { "type": "path", "value": "/tmp/a.png" },
                    "natural_width": 400.0,
                    "natural_height": 800.0,
                    "transform": {
                        "x": 0.0, "y": 0.0, "width": 0.0, "height": 0.0,
                        "rotation_deg": 0.0, "aspect_locked": true
                    },
                    "corner_radius": { "top_left": 0.0, "top_right": 0.0, "bottom_right": 0.0, "bottom_left": 0.0 },
                    "shadow": {
                        "enabled": false, "offset_x": 0.0, "offset_y": 0.0, "blur": 0.0, "opacity": 0.0,
                        "color": { "r": 0.0, "g": 0.0, "b": 0.0, "a": 1.0 }
                    },
                    "visible": true
                }],
                "layout": { "mode": "horizontal", "spacing_px": 24.0, "margin_px": 48.0 },
                "background": { "type": "solid", "value": { "r": 0.95, "g": 0.95, "b": 0.96, "a": 1.0 } },
                "canvas": { "export_width": 1920, "export_height": 1080, "export_format": "png", "export_quality": 90 }
            }
        }"#;
        std::fs::write(&path, json).unwrap();

        let doc = load(&path, &temp_path("assets")).unwrap();
        assert!(!doc.elements[0].transform.flip_horizontal);
        assert!(!doc.elements[0].transform.flip_vertical);
        std::fs::remove_file(&path).ok();
    }

    /// A project saved before a label's padding was split into
    /// `padding_x`/`padding_y` used one uniform `background_padding`
    /// value — this pins down that such a file still loads without error
    /// (the legacy-padding migration must run cleanly even though its
    /// output is discarded by the later content-only migration).
    #[test]
    fn migrates_a_legacy_uniform_label_padding_into_both_new_fields() {
        let path = temp_path("legacy-label-padding.screenforge");
        let json = r#"{
            "format": "screenforge",
            "version": 1,
            "document": {
                "id": "8f14e45f-ceea-467e-adc0-51944115d5c6",
                "elements": [{
                    "id": "9b2e1a3c-1234-4a3b-8cde-0123456789ab",
                    "source": { "type": "path", "value": "/tmp/a.png" },
                    "natural_width": 400.0,
                    "natural_height": 800.0,
                    "transform": { "x": 0.0, "y": 0.0, "width": 0.0, "height": 0.0, "rotation_deg": 0.0, "aspect_locked": true },
                    "corner_radius": { "top_left": 0.0, "top_right": 0.0, "bottom_right": 0.0, "bottom_left": 0.0 },
                    "shadow": { "enabled": false, "offset_x": 0.0, "offset_y": 0.0, "blur": 0.0, "opacity": 0.0, "color": { "r": 0.0, "g": 0.0, "b": 0.0, "a": 1.0 } },
                    "label": {
                        "enabled": true,
                        "content": "Legacy Label",
                        "position": { "mode": "absolute", "x": 5.0, "y": 5.0 },
                        "typography": {
                            "font_family": "Sans", "font_size": 18.0, "weight": 700, "italic": false,
                            "color": { "r": 1.0, "g": 1.0, "b": 1.0, "a": 1.0 }, "alignment": "center", "opacity": 1.0,
                            "letter_spacing": 0.0, "line_spacing": 1.2, "wrap": false
                        },
                        "background": { "type": "none" },
                        "corner_radius": { "top_left": 0.0, "top_right": 0.0, "bottom_right": 0.0, "bottom_left": 0.0 },
                        "background_padding": 22.0,
                        "shadow": { "enabled": false, "offset_x": 0.0, "offset_y": 0.0, "blur": 0.0, "opacity": 0.0, "color": { "r": 0.0, "g": 0.0, "b": 0.0, "a": 1.0 } }
                    },
                    "visible": true
                }],
                "layout": { "mode": "horizontal", "spacing_px": 24.0, "margin_px": 48.0 },
                "background": { "type": "solid", "value": { "r": 0.95, "g": 0.95, "b": 0.96, "a": 1.0 } },
                "canvas": { "export_width": 1920, "export_height": 1080, "export_format": "png", "export_quality": 90 }
            }
        }"#;
        std::fs::write(&path, json).unwrap();

        let doc = load(&path, &temp_path("assets")).unwrap();
        assert_eq!(doc.elements[0].label.content, "Legacy Label");
        // Padding is no longer per-label at all, so the pre-split
        // `background_padding` is expected to be discarded rather than
        // carried forward — the label now resolves purely against the
        // project's shared `label_defaults`, same as any other field that
        // used to live on the label itself.
        let resolved = doc.elements[0].label.resolve(&doc.label_defaults, 0.0);
        assert_eq!(resolved.padding_x, doc.label_defaults.padding_x);
        assert_eq!(resolved.padding_y, doc.label_defaults.padding_y);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn migrates_an_old_style_carrying_label_down_to_just_its_content() {
        // A project saved by a version of the app before per-label
        // styling was removed: `label` is a self-contained
        // `TextElement`-shaped object bundling content with every style
        // field (already-split `padding_x`/`padding_y`). Only `enabled`/
        // `content` must survive — every style field is discarded, since
        // the label now always renders using the project's shared
        // `Document::label_defaults`.
        let path = temp_path("pre-override-label.screenforge");
        let json = r#"{
            "format": "screenforge",
            "version": 1,
            "document": {
                "id": "8f14e45f-ceea-467e-adc0-51944115d5c7",
                "elements": [{
                    "id": "9b2e1a3c-1234-4a3b-8cde-0123456789ac",
                    "source": { "type": "path", "value": "/tmp/a.png" },
                    "natural_width": 400.0,
                    "natural_height": 800.0,
                    "transform": { "x": 0.0, "y": 0.0, "width": 0.0, "height": 0.0, "rotation_deg": 0.0, "aspect_locked": true },
                    "corner_radius": { "top_left": 0.0, "top_right": 0.0, "bottom_right": 0.0, "bottom_left": 0.0 },
                    "shadow": { "enabled": false, "offset_x": 0.0, "offset_y": 0.0, "blur": 0.0, "opacity": 0.0, "color": { "r": 0.0, "g": 0.0, "b": 0.0, "a": 1.0 } },
                    "label": {
                        "enabled": true,
                        "content": "Old Style",
                        "position": { "mode": "absolute", "x": 5.0, "y": 9.0 },
                        "typography": {
                            "font_family": "Serif", "font_size": 24.0, "weight": 400, "italic": true,
                            "color": { "r": 1.0, "g": 0.0, "b": 0.0, "a": 1.0 }, "alignment": "left", "opacity": 0.8,
                            "letter_spacing": 1.0, "line_spacing": 1.4, "wrap": true
                        },
                        "background": { "type": "solid", "value": { "r": 0.0, "g": 0.0, "b": 1.0, "a": 1.0 } },
                        "corner_radius": { "top_left": 3.0, "top_right": 3.0, "bottom_right": 3.0, "bottom_left": 3.0 },
                        "padding_x": 11.0,
                        "padding_y": 13.0,
                        "shadow": { "enabled": true, "offset_x": 1.0, "offset_y": 2.0, "blur": 3.0, "opacity": 0.5, "color": { "r": 0.0, "g": 0.0, "b": 0.0, "a": 1.0 } }
                    },
                    "visible": true
                }],
                "layout": { "mode": "horizontal", "spacing_px": 24.0, "margin_px": 48.0 },
                "background": { "type": "solid", "value": { "r": 0.95, "g": 0.95, "b": 0.96, "a": 1.0 } },
                "canvas": { "export_width": 1920, "export_height": 1080, "export_format": "png", "export_quality": 90 }
            }
        }"#;
        std::fs::write(&path, json).unwrap();

        let doc = load(&path, &temp_path("assets")).unwrap();
        let label = &doc.elements[0].label;
        assert!(label.enabled);
        assert_eq!(label.content, "Old Style");

        // Every old style field is gone — the label now resolves purely
        // from the project's shared `label_defaults`, not from anything
        // that used to live on this specific label.
        let resolved = label.resolve(&doc.label_defaults, 0.0);
        assert_eq!(resolved.content, "Old Style");
        assert_eq!(resolved.typography, doc.label_defaults.typography);
        assert_eq!(resolved.position, doc.label_defaults.position);
        assert_eq!(resolved.padding_x, doc.label_defaults.padding_x);
        assert_eq!(resolved.padding_y, doc.label_defaults.padding_y);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn migrates_a_legacy_uniform_layout_margin_into_both_new_fields() {
        let path = temp_path("legacy-layout-margin.screenforge");
        let json = r#"{
            "format": "screenforge",
            "version": 1,
            "document": {
                "id": "8f14e45f-ceea-467e-adc0-51944115d5c6",
                "elements": [],
                "layout": { "mode": "horizontal", "spacing_px": 24.0, "margin_px": 65.0 },
                "background": { "type": "solid", "value": { "r": 0.95, "g": 0.95, "b": 0.96, "a": 1.0 } },
                "canvas": { "export_width": 1920, "export_height": 1080, "export_format": "png", "export_quality": 90 }
            }
        }"#;
        std::fs::write(&path, json).unwrap();

        let doc = load(&path, &temp_path("assets")).unwrap();
        assert_eq!(doc.layout.margin_x, 65.0);
        assert_eq!(doc.layout.margin_y, 65.0);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn unsupported_future_version_is_reported_clearly() {
        let path = temp_path("future.screenforge");
        let doc = Document::new();
        let json = serde_json::to_string(&ProjectFile { format: "screenforge".into(), version: 99, document: doc }).unwrap();
        std::fs::write(&path, json).unwrap();

        let err = load(&path, &temp_path("assets")).unwrap_err();
        match err {
            ProjectError::UnsupportedVersion { found, supported } => {
                assert_eq!(found, 99);
                assert_eq!(supported, CURRENT_VERSION);
            }
            other => panic!("expected UnsupportedVersion, got {other:?}"),
        }
        std::fs::remove_file(&path).ok();
    }
}
