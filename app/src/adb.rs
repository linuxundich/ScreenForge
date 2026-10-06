//! Capturing a screenshot straight from a connected Android device via
//! `adb` — shells out to whatever `adb` the user already has on their
//! `PATH` rather than bundling any Android tooling. `adb exec-out
//! screencap -p` streams the PNG straight to this process's stdout, no
//! push/pull through the device's own storage needed, and it works
//! identically over USB or wireless debugging since that negotiation
//! happens inside `adb` itself before this ever runs.

use std::path::PathBuf;

use gettextrs::gettext;
use std::process::{Command, Output};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AdbError {
    #[error("{}", gettext("adb was not found — are the Android Platform Tools installed and in PATH?"))]
    NotFound,
    #[error("{}", gettext("adb could not be started: {0}").replace("{0}", &.0.to_string()))]
    Spawn(std::io::Error),
    #[error("{}", gettext("no authorized Android device found — enable USB debugging and confirm access on the device"))]
    NoDevice,
    #[error("{}", gettext("several Android devices found ({0}) — please keep only one connected").replace("{0}", .0))]
    MultipleDevices(String),
    #[error("{}", gettext("screencap failed: {0}").replace("{0}", .0))]
    Screencap(String),
    #[error("{}", gettext("could not store the screenshot: {0}").replace("{0}", &.0.to_string()))]
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

/// Inside a Flatpak sandbox there's no `adb`; the host's is reached via
/// `flatpak-spawn --host` (needs `--talk-name=org.freedesktop.Flatpak`).
fn adb_command() -> Command {
    if std::path::Path::new("/.flatpak-info").exists() {
        let mut command = Command::new("flatpak-spawn");
        command.args(["--host", "adb"]);
        command
    } else {
        Command::new("adb")
    }
}

fn run_adb(args: &[&str]) -> Result<Output, AdbError> {
    adb_command()
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

/// The toolbar import button's state, watched by
/// `main.rs::register_adb_watch` and refreshed on a timer — coarser than
/// [`AdbError`] (which is about *why one capture attempt failed*): this is
/// about *what to show the user right now*, before they've clicked
/// anything. `Eq`/`Hash` so a watcher can cheaply skip a widget update
/// when nothing actually changed between two polls.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AdbDeviceState {
    /// No device at all — nothing shows up in `adb devices -l`, or only
    /// `offline` entries (mid-(re)connect, treated the same as absent
    /// since there's nothing to do about it but wait).
    NoDevice,
    /// A device is physically connected and `adb` sees it, but the user
    /// hasn't confirmed the "Allow USB debugging?" prompt on the device
    /// yet (`adb`'s own `unauthorized`/`no permissions` status).
    Unauthorized,
    /// At least one device is connected, authorized, and responsive.
    /// Whether it's *exactly* one (vs. several, which `pick_device` below
    /// still refuses at capture time) is deliberately not distinguished
    /// here — the button's job is just "is importing possible at all
    /// right now", not "will the next click definitely succeed".
    Reachable,
    /// `adb` itself couldn't be asked at all — not installed/not on
    /// `PATH`, or failed to run for some other reason. Carries a short,
    /// user-facing reason for the button's tooltip.
    AdbUnavailable(String),
}

impl AdbDeviceState {
    /// Whether the toolbar import button should be clickable in this
    /// state — only when a capture attempt has a real chance of working;
    /// see `AdbDeviceState`'s own doc comment for why `Reachable` doesn't
    /// also require "exactly one device".
    pub fn is_usable(&self) -> bool {
        matches!(self, AdbDeviceState::Reachable)
    }

    /// A short, user-facing sentence for the toolbar button's tooltip,
    /// explaining *why* it's disabled (or confirming it's ready) without
    /// requiring the user to already know what ADB is.
    pub fn tooltip(&self) -> String {
        match self {
            AdbDeviceState::NoDevice => gettext("Import from Android device — no device found. Connect via USB and enable USB debugging."),
            AdbDeviceState::Unauthorized => gettext("Import from Android device — confirm access on the device (“Allow USB debugging?”)"),
            AdbDeviceState::Reachable => gettext("Import from Android Device"),
            AdbDeviceState::AdbUnavailable(reason) => gettext("Import from Android device — not possible: {reason}").replace("{reason}", reason),
        }
    }
}

/// Every device `adb devices -l` currently reports, *including*
/// unauthorized/offline ones — unlike `list_devices` above, which only
/// the actual capture path needs (and which deliberately drops anything
/// it can't use). Used only by `detect_state`, which needs to tell those
/// cases apart to show a meaningful toolbar-button tooltip rather than
/// just "not available".
fn list_devices_with_status() -> Result<Vec<(String, String)>, AdbError> {
    let output = run_adb(&["devices", "-l"])?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout
        .lines()
        .skip(1) // "List of devices attached"
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let serial = fields.next()?;
            let status = fields.next()?;
            Some((serial.to_string(), status.to_string()))
        })
        .collect())
}

/// Classifies an already-fetched `(serial, status)` list into the state
/// the toolbar button should show — pulled out of `detect_state` purely so
/// this part (the actual decision logic) is testable without an `adb`
/// binary or a real device: any one `device` status wins outright
/// (`Reachable`), then any `unauthorized`/`no` (the first word of "no
/// permissions", the two being separate whitespace-split fields — see
/// `list_devices_with_status`), otherwise nothing usable is connected.
fn classify(devices: &[(String, String)]) -> AdbDeviceState {
    if devices.iter().any(|(_, status)| status == "device") {
        AdbDeviceState::Reachable
    } else if devices.iter().any(|(_, status)| status == "unauthorized" || status == "no") {
        AdbDeviceState::Unauthorized
    } else {
        AdbDeviceState::NoDevice
    }
}

/// Classifies the current device situation for the toolbar button — see
/// `AdbDeviceState`. Blocking (shells out to `adb`, same as
/// `capture_screenshot`); callers run this via `gio::spawn_blocking`, most
/// notably `main.rs::register_adb_watch`'s periodic check.
pub fn detect_state() -> AdbDeviceState {
    match list_devices_with_status() {
        Ok(devices) => classify(&devices),
        Err(AdbError::NotFound) => AdbDeviceState::AdbUnavailable(gettext("adb not found")),
        Err(err) => AdbDeviceState::AdbUnavailable(err.to_string()),
    }
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
pub fn capture_screenshot(demo_mode: bool) -> Result<PathBuf, AdbError> {
    let device = pick_device()?;
    if demo_mode {
        enter_demo_mode(&device.serial);
    }
    let output = run_adb(&["-s", &device.serial, "exec-out", "screencap", "-p"]);
    if demo_mode {
        demo_command(&device.serial, &["-e", "command", "exit"]);
    }
    let output = output?;
    if !output.status.success() || output.stdout.is_empty() {
        return Err(AdbError::Screencap(String::from_utf8_lossy(&output.stderr).trim().to_string()));
    }

    let dir = glib::user_cache_dir().join("screenforge").join("android");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("android-{}.png", uuid::Uuid::new_v4()));
    std::fs::write(&path, &output.stdout)?;
    Ok(path)
}

/// Android's System UI demo mode: a clean status bar for the screenshot —
/// 12:00, full battery not charging, full Wi-Fi and mobile signal, no
/// notification icons. Best effort: a device or ROM that ignores it just
/// produces an ordinary screenshot.
fn enter_demo_mode(serial: &str) {
    let _ = run_adb(&["-s", serial, "shell", "settings", "put", "global", "sysui_demo_allowed", "1"]);
    demo_command(serial, &["-e", "command", "enter"]);
    demo_command(serial, &["-e", "command", "clock", "-e", "hhmm", "1200"]);
    demo_command(serial, &["-e", "command", "battery", "-e", "level", "100", "-e", "plugged", "false"]);
    demo_command(serial, &["-e", "command", "network", "-e", "wifi", "show", "-e", "level", "4"]);
    demo_command(serial, &["-e", "command", "network", "-e", "mobile", "show", "-e", "datatype", "none", "-e", "level", "4"]);
    demo_command(serial, &["-e", "command", "notifications", "-e", "visible", "false"]);
    // Give System UI a moment to redraw before the capture.
    std::thread::sleep(std::time::Duration::from_millis(600));
}

fn demo_command(serial: &str, extras: &[&str]) {
    let mut args = vec!["-s", serial, "shell", "am", "broadcast", "-a", "com.android.systemui.demo"];
    args.extend_from_slice(extras);
    let _ = run_adb(&args);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn devices(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(serial, status)| (serial.to_string(), status.to_string())).collect()
    }

    #[test]
    fn no_devices_at_all_is_no_device() {
        assert_eq!(classify(&devices(&[])), AdbDeviceState::NoDevice);
    }

    #[test]
    fn an_offline_device_alone_is_still_no_device() {
        // "offline" is mid-(re)connect — nothing the user can act on, so
        // it's shown the same as nothing being connected at all.
        assert_eq!(classify(&devices(&[("ABC123", "offline")])), AdbDeviceState::NoDevice);
    }

    #[test]
    fn an_unauthorized_device_is_reported_distinctly() {
        assert_eq!(classify(&devices(&[("ABC123", "unauthorized")])), AdbDeviceState::Unauthorized);
    }

    #[test]
    fn no_permissions_status_is_also_unauthorized() {
        // "no permissions (missing udev rules?)..." — the status field is
        // itself two whitespace-separated words, so the second column
        // `list_devices_with_status` captures is literally "no".
        assert_eq!(classify(&devices(&[("ABC123", "no")])), AdbDeviceState::Unauthorized);
    }

    #[test]
    fn one_ready_device_is_reachable() {
        assert_eq!(classify(&devices(&[("ABC123", "device")])), AdbDeviceState::Reachable);
    }

    #[test]
    fn one_ready_device_among_others_is_still_reachable() {
        // A second, unauthorized phone shouldn't mask that *something*
        // usable is connected — `capture_screenshot`'s own `pick_device`
        // is what actually enforces "exactly one" at capture time.
        assert_eq!(classify(&devices(&[("ABC123", "unauthorized"), ("DEF456", "device")])), AdbDeviceState::Reachable);
    }

    #[test]
    fn only_reachable_state_is_usable() {
        assert!(AdbDeviceState::Reachable.is_usable());
        assert!(!AdbDeviceState::NoDevice.is_usable());
        assert!(!AdbDeviceState::Unauthorized.is_usable());
        assert!(!AdbDeviceState::AdbUnavailable("x".to_string()).is_usable());
    }
}
