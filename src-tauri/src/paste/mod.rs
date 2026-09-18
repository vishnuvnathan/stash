//! Pasting into whatever window was focused before the panel opened.
//!
//! Two halves, and the order matters. `remember_foreground` runs in
//! `window::show` *before* the panel takes focus, because once it has focus the
//! window we want is no longer the foreground one. `paste_into_previous` runs
//! after the panel hides: it hands focus back and synthesises the paste
//! keystroke.
//!
//! Laid out like `capture/`: one API, three backends, and a status struct
//! rather than a bool so the UI can say what actually happened instead of
//! assuming it worked.

#[cfg(windows)]
#[path = "win.rs"]
mod platform;

#[cfg(target_os = "macos")]
#[path = "mac.rs"]
mod platform;

#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod platform;

use serde::Serialize;

/// What a paste attempt did.
///
/// `pasted` is only true once the target window was verifiably in the
/// foreground and the keystroke was sent. On any other outcome the caller has
/// still put the item on the clipboard, so the user can paste it by hand -- and
/// `reason` is what tells them to.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PasteStatus {
    pub supported: bool,
    pub pasted: bool,
    pub reason: Option<String>,
}

impl PasteStatus {
    pub fn pasted() -> Self {
        PasteStatus { supported: true, pasted: true, reason: None }
    }

    pub fn failed(reason: impl Into<String>) -> Self {
        PasteStatus { supported: true, pasted: false, reason: Some(reason.into()) }
    }

    /// Only ever constructed by the macOS and Linux backends, so it reads as
    /// dead code on a Windows build.
    #[cfg_attr(windows, allow(dead_code))]
    pub fn unsupported(reason: impl Into<String>) -> Self {
        PasteStatus { supported: false, pasted: false, reason: Some(reason.into()) }
    }
}

/// Records the window to paste back into. Cheap enough to call on every panel
/// open; it is one syscall on Windows and a no-op elsewhere.
pub fn remember_foreground() {
    platform::remember_foreground();
}

/// Restores focus to that window and sends the paste keystroke. Blocks for up
/// to a few hundred milliseconds waiting for the focus change to land, so
/// callers run it off the async runtime's worker threads.
pub fn paste_into_previous() -> PasteStatus {
    platform::paste_into_previous()
}

/// Whether this platform can paste at all, without attempting one. The settings
/// panel uses it to describe the auto-paste toggle honestly rather than offering
/// a switch that does nothing.
pub fn support() -> PasteStatus {
    platform::support()
}

/// Logged once at startup on platforms that cannot paste, the same way
/// `capture` reports an unavailable exclusion.
pub fn warn_if_unsupported() {
    let status = support();
    if !status.supported {
        if let Some(reason) = &status.reason {
            tracing::info!(reason, "paste-on-Enter unavailable; Enter will copy only");
        }
    }
}
