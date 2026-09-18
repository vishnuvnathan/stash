//! Launch on startup.
//!
//! A thin wrapper over `tauri-plugin-autostart` so the plugin is imported in
//! exactly one place and the settings command does not have to know how the
//! registry entry is written. On Windows that is an `HKCU\...\Run` value; the
//! panel is already `visible: false` in `tauri.conf.json`, so starting this way
//! lands in the tray rather than popping a window in the user's face at login.

use tauri::AppHandle;
use tauri_plugin_autostart::ManagerExt;

/// Makes the OS match `enabled`.
///
/// Errors are returned rather than logged, because the caller is a settings
/// write: saving `launchOnStartup: true` when the registry write failed would
/// leave the toggle lying to the user about what happens at their next login.
pub fn apply(app: &AppHandle, enabled: bool) -> Result<(), String> {
    let manager = app.autolaunch();

    if manager.is_enabled().unwrap_or(false) == enabled {
        return Ok(());
    }

    let result = if enabled { manager.enable() } else { manager.disable() };
    result.map_err(|e| format!("could not change launch on startup: {e}"))
}

/// Startup reconcile. The stored setting is the user's intent, so it wins over
/// whatever the OS state happens to be -- but a failure here is not worth
/// blocking the app's launch over.
pub fn reconcile(app: &AppHandle, wanted: bool) {
    if let Err(e) = apply(app, wanted) {
        tracing::warn!(error = %e, wanted, "could not reconcile launch on startup");
    }
}
