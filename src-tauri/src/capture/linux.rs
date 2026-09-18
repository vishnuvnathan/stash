use super::CaptureStatus;
use std::sync::Once;

static WARNED: Once = Once::new();

/// No portable equivalent exists under X11 or Wayland: a compositor decides
/// what a screencast sees, and there is no client-side opt-out. Hide-on-blur is
/// the mitigation, and we say so once rather than on every toggle.
pub fn apply(_window: &tauri::WebviewWindow, _enabled: bool) -> CaptureStatus {
    WARNED.call_once(|| {
        tracing::warn!(
            "screen-capture exclusion is not supported on Linux; \
             relying on hide-on-blur to keep the panel off screen shares"
        );
    });
    CaptureStatus::unsupported("Not supported on Linux; the panel hides on blur instead.")
}
