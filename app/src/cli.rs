//! Command-line rendering without a window: apply a saved preset to a set
//! of images and export the result.
//!
//! ```text
//! screenforge --preset NAME [--output FILE] [--width N] IMAGE...
//! screenforge --list-presets
//! ```
//!
//! The output format follows the file extension (png, jpg, webp, avif,
//! pdf); default output is `screenforge.png` in the current directory.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::*;

/// Whether `args` (without the program name) ask for the command line
/// instead of the window.
pub(crate) fn wants_cli(args: &[String]) -> bool {
    args.iter().any(|a| matches!(a.as_str(), "--preset" | "--list-presets" | "--help" | "-h"))
}

fn usage() -> String {
    gettext(
        "Usage:\n  screenforge [FILE…]                  open the app\n  screenforge --preset NAME [--output FILE] [--width N] IMAGE…\n                                       render IMAGEs with a saved preset\n  screenforge --list-presets           list saved presets\n\nThe output format follows the file extension: png, jpg, webp, avif, pdf or webm (animated).",
    )
}

/// Runs the command line and returns the process exit code.
pub(crate) fn run(args: &[String]) -> glib::ExitCode {
    match run_inner(args) {
        Ok(()) => glib::ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("screenforge: {message}");
            glib::ExitCode::FAILURE
        }
    }
}

fn run_inner(args: &[String]) -> Result<(), String> {
    let mut preset_name = None;
    let mut output = PathBuf::from("screenforge.png");
    let mut width = None;
    let mut images = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!("{}", usage());
                return Ok(());
            }
            "--list-presets" => {
                for preset in load_presets() {
                    println!("{}", preset.name);
                }
                return Ok(());
            }
            "--preset" => preset_name = Some(iter.next().ok_or_else(usage)?.clone()),
            "--output" | "-o" => output = PathBuf::from(iter.next().ok_or_else(usage)?),
            "--width" => {
                let value = iter.next().ok_or_else(usage)?;
                width = Some(value.parse::<u32>().map_err(|_| gettext("--width needs a number of pixels"))?);
            }
            other if other.starts_with('-') => return Err(format!("{}\n\n{}", gettext("Unknown option: {option}").replace("{option}", other), usage())),
            image => images.push(PathBuf::from(image)),
        }
    }
    let preset_name = preset_name.ok_or_else(usage)?;
    if images.is_empty() {
        return Err(gettext("No images given."));
    }
    let preset = load_presets()
        .into_iter()
        .find(|p| p.name.eq_ignore_ascii_case(&preset_name))
        .ok_or_else(|| gettext("No preset named “{name}”. See --list-presets.").replace("{name}", &preset_name))?;

    let mut document = Document::new();
    let mut decoded = HashMap::new();
    for path in &images {
        let image = import::decode_image(path).map_err(|err| format!("{}: {err}", path.display()))?;
        let element = ScreenshotElement::new(ImageSource::Path(path.clone()), image.width as f64, image.height as f64);
        decoded.insert(element.id, image);
        document.elements.push(element);
    }
    ApplyTemplate::capturing(&document, preset.template.clone()).apply(&mut document);
    if let Some(ImageSource::Path(path)) = &document.watermark.logo {
        if let Ok(image) = import::decode_image(path) {
            decoded.insert(screenforge_core::render::WATERMARK_LOGO_ID, image);
        }
    }
    document.canvas.export_format = format_for(&output)?;
    if let Some(width) = width {
        document.canvas.export_target_width = width;
    }
    screenforge_core::layout::fit_canvas_to_content(&mut document);
    let background_image = background_image_path(&document.background).map(|p| import::decode_image(&p)).transpose().map_err(|e| e.to_string())?;
    export::render_and_write(&document, &decoded, background_image.as_ref(), &output).map_err(|err| err.to_string())?;
    println!("{}", output.display());
    Ok(())
}

fn format_for(path: &std::path::Path) -> Result<ExportFormat, String> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "png" => Ok(ExportFormat::Png),
        "jpg" | "jpeg" => Ok(ExportFormat::Jpeg),
        "webp" => Ok(ExportFormat::WebP),
        "avif" => Ok(ExportFormat::Avif),
        "pdf" => Ok(ExportFormat::Pdf),
        "webm" => Ok(ExportFormat::WebM),
        _ => Err(gettext("Unsupported output format “{ext}”; use png, jpg, webp, avif, pdf or webm.").replace("{ext}", &ext)),
    }
}
