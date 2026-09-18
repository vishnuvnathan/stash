use super::CaptureStatus;
use std::ffi::c_void;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_MONITOR, WDA_NONE,
};

/// `WDA_EXCLUDEFROMCAPTURE` needs Windows 10 2004 (build 19041). On older
/// builds the call fails with ERROR_INVALID_PARAMETER, so we fall back to
/// `WDA_MONITOR`, which blanks the window in captures instead of hiding it.
/// Either way the content stays out of the recording; the caller is told which
/// one it got rather than being left to assume.
pub fn apply(window: &tauri::WebviewWindow, enabled: bool) -> CaptureStatus {
    let hwnd = match window.hwnd() {
        Ok(h) => HWND(h.0 as *mut c_void),
        Err(e) => return CaptureStatus::unsupported(&format!("no HWND: {e}")),
    };

    if !enabled {
        return match unsafe { SetWindowDisplayAffinity(hwnd, WDA_NONE) } {
            Ok(()) => CaptureStatus::ok(),
            Err(e) => CaptureStatus::unsupported(&format!("could not clear affinity: {e}")),
        };
    }

    if unsafe { SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) }.is_ok() {
        return CaptureStatus::ok();
    }

    match unsafe { SetWindowDisplayAffinity(hwnd, WDA_MONITOR) } {
        Ok(()) => {
            tracing::warn!(
                "WDA_EXCLUDEFROMCAPTURE unavailable (needs Windows 10 build 19041); \
                 degraded to WDA_MONITOR"
            );
            CaptureStatus::degraded(
                "This Windows build predates full capture exclusion; \
                 the panel is blanked in recordings instead of hidden.",
            )
        }
        Err(e) => CaptureStatus::unsupported(&format!("display affinity rejected: {e}")),
    }
}
