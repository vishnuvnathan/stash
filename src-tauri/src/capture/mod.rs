//! Screen-capture exclusion, behind one command with platform-gated backends.

use serde::Serialize;

#[cfg(windows)]
#[path = "win.rs"]
mod platform;

#[cfg(target_os = "macos")]
#[path = "mac.rs"]
mod platform;

#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod platform;

/// Reported back to the settings UI so it can say what actually happened
/// rather than showing a toggle that silently does nothing.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureStatus {
    pub supported: bool,
    pub applied: bool,
    pub reason: Option<String>,
}

impl CaptureStatus {
    pub fn unsupported(reason: &str) -> Self {
        CaptureStatus { supported: false, applied: false, reason: Some(reason.to_string()) }
    }
    pub fn ok() -> Self {
        CaptureStatus { supported: true, applied: true, reason: None }
    }
    pub fn degraded(reason: &str) -> Self {
        CaptureStatus { supported: true, applied: true, reason: Some(reason.to_string()) }
    }
}

pub fn apply(window: &tauri::WebviewWindow, enabled: bool) -> CaptureStatus {
    platform::apply(window, enabled)
}
