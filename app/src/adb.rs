//! Capturing a screenshot straight from a connected Android device via
//! `adb` — shells out to whatever `adb` the user already has on their
//! `PATH` rather than bundling any Android tooling. `adb exec-out
//! screencap -p` streams the PNG straight to this process's stdout, no
//! push/pull through the device's own storage needed, and it works
//! identically over USB or wireless debugging since that negotiation
//! happens inside `adb` itself before this ever runs.

use std::path::PathBuf;
use std::process::{Command, Output};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AdbError {
    #[error("adb wurde nicht gefunden — sind die Android Platform Tools installiert und im PATH?")]
    NotFound,
    #[error("adb konnte nicht gestartet werden: {0}")]
    Spawn(std::io::Error),
    #[error("kein autorisiertes Android-Gerät gefunden — USB-Debugging aktivieren und den Zugriff auf dem Gerät bestätigen")]
    NoDevice,
    #[error("mehrere Android-Geräte gefunden ({0}) — bitte nur eines verbunden lassen")]
    MultipleDevices(String),
    #[error("screencap ist fehlgeschlagen: {0}")]
    Screencap(String),
    #[error("konnte den Screenshot nicht zwischenspeichern: {0}")]
    Io(#[from] std::io::Error),
}

struct AdbDevice {
    serial: String,
    model: Option<String>,
}

impl AdbDevice {
    /// A human-readable identifier for error messages — the model name
    /// when `adb` reported one, the bare serial otherwise.
    fn label(&self) -> String {
        match &self.model {
            Some(model) => format!("{model} ({})", self.serial),
            None => self.serial.clone(),
        }
    }
}

fn run_adb(args: &[&str]) -> Result<Output, AdbError> {
    Command::new("adb")
        .args(args)
        .output()
        .map_err(|err| if err.kind() == std::io::ErrorKind::NotFound { AdbError::NotFound } else { AdbError::Spawn(err) })
}

/// Every device `adb devices -l` reports as `device` — i.e. connected,
/// authorized, and responsive. `unauthorized`/`offline` entries are
/// dropped rather than surfaced as a different kind of device, since
/// there's nothing this app can do about either short of the user
/// confirming the on-device prompt themselves.
fn list_devices() -> Result<Vec<AdbDevice>, AdbError> {
    let output = run_adb(&["devices", "-l"])?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout
        .lines()
        .skip(1) // "List of devices attached"
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let serial = fields.next()?;
            let status = fields.next()?;
            if status != "device" {
                return None;
            }
            let model = fields.find_map(|f| f.strip_prefix("model:")).map(str::to_string);
            Some(AdbDevice { serial: serial.to_string(), model })
        })
        .collect())
}

/// Picks the one authorized device to capture from — an error if there
/// are zero or more than one, since silently guessing which phone the
/// user meant would be worse than asking them to unplug the others.
fn pick_device() -> Result<AdbDevice, AdbError> {
    let mut devices = list_devices()?;
    match devices.len() {
        0 => Err(AdbError::NoDevice),
        1 => Ok(devices.remove(0)),
        _ => Err(AdbError::MultipleDevices(devices.iter().map(AdbDevice::label).collect::<Vec<_>>().join(", "))),
    }
}

/// Captures the one connected device's current screen as a PNG and writes
/// it under the same per-app cache directory `import::save_pasted_image`
/// uses for clipboard pastes, ready to hand straight to `import_paths`
/// exactly like a file-picked screenshot. Blocking (shells out and waits
/// for both `adb` calls to exit) — callers run this via `gio::spawn_blocking`.
pub fn capture_screenshot() -> Result<PathBuf, AdbError> {
    let device = pick_device()?;
    let output = run_adb(&["-s", &device.serial, "exec-out", "screencap", "-p"])?;
    if !output.status.success() || output.stdout.is_empty() {
        return Err(AdbError::Screencap(String::from_utf8_lossy(&output.stderr).trim().to_string()));
    }

    let dir = glib::user_cache_dir().join("screenforge").join("android");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("android-{}.png", uuid::Uuid::new_v4()));
    std::fs::write(&path, &output.stdout)?;
    Ok(path)
}
