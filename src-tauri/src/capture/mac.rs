use super::CaptureStatus;
use objc2::rc::Retained;
use objc2_app_kit::{NSWindow, NSWindowSharingType};

/// `NSWindowSharingNone` removes the window from screen recordings and
/// screenshots. Structured but untested, per the v0.1 scope.
pub fn apply(window: &tauri::WebviewWindow, enabled: bool) -> CaptureStatus {
    let ptr = match window.ns_window() {
        Ok(p) => p,
        Err(e) => return CaptureStatus::unsupported(&format!("no NSWindow handle: {e}")),
    };

    let sharing = if enabled {
        NSWindowSharingType::None
    } else {
        NSWindowSharingType::ReadOnly
    };

    // Safety: `ns_window` hands back a live NSWindow owned by the app; we only
    // borrow it, and AppKit calls must happen on the main thread, which is
    // where Tauri dispatches commands for this window.
    unsafe {
        let ns: Retained<NSWindow> = Retained::retain(ptr as *mut NSWindow)
            .expect("NSWindow pointer from Tauri was null");
        ns.setSharingType(sharing);
    }

    CaptureStatus::ok()
}
