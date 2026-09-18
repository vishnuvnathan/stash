//! Linux clipboard reader.
//!
//! X11 has no sequence-number equivalent, so this path reads the selection each
//! tick and compares a hash -- the one platform where an idle tick costs an X
//! round-trip. XFIXES `SelectionNotify` would make it event-driven and is the
//! obvious v0.2 follow-up.
//!
//! Known gap: there is no portable concealed-content marker on Linux. The KDE
//! convention (`x-kde-passwordManagerHint`) needs raw TARGETS inspection, which
//! `arboard` does not expose. Until that lands, hide-on-blur and the retention
//! limit are the only mitigations, and this is logged once at startup.

use super::types::{PollState, Snapshot};
use std::sync::Once;

static CONCEALED_WARNING: Once = Once::new();

pub fn poll(state: &mut PollState) -> Snapshot {
    CONCEALED_WARNING.call_once(|| {
        tracing::warn!(
            "Linux build: concealed-clipboard detection is unavailable; \
             password-manager copies cannot be filtered out"
        );
    });

    let mut cb = match arboard::Clipboard::new() {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!(error = %e, "clipboard unavailable this tick");
            return Snapshot::Unsupported;
        }
    };

    if let Ok(text) = cb.get_text() {
        if !text.is_empty() {
            let h = super::hash_bytes(text.as_bytes());
            if state.primed && h == state.last_text_hash {
                return Snapshot::Unchanged;
            }
            state.last_text_hash = h;
            if !state.primed {
                state.primed = true;
                return Snapshot::Unchanged;
            }
            return Snapshot::Text(text);
        }
    }

    if let Ok(img) = cb.get_image() {
        let raw_len = img.bytes.len();
        if raw_len > super::types::MAX_IMAGE_BYTES {
            tracing::debug!(bytes = raw_len, "clipboard image over limit; skipped");
            return Snapshot::Unsupported;
        }
        let buf = match image::RgbaImage::from_raw(
            img.width as u32,
            img.height as u32,
            img.bytes.into_owned(),
        ) {
            Some(b) => b,
            None => return Snapshot::Unsupported,
        };
        let mut png = Vec::new();
        if image::DynamicImage::ImageRgba8(buf)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .is_err()
        {
            return Snapshot::Unsupported;
        }
        let h = super::hash_bytes(&png);
        if state.primed && h == state.last_text_hash {
            return Snapshot::Unchanged;
        }
        state.last_text_hash = h;
        if !state.primed {
            state.primed = true;
            return Snapshot::Unchanged;
        }
        return Snapshot::Image(png);
    }

    Snapshot::Unsupported
}

/// Active window -> `_NET_WM_PID` -> `/proc/<pid>/comm`. Returns `None` under
/// Wayland, where no equivalent is exposed to unprivileged clients.
pub fn foreground_app() -> Option<String> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};

    let (conn, screen_num) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots.get(screen_num)?.root;

    let active_atom = conn.intern_atom(true, b"_NET_ACTIVE_WINDOW").ok()?.reply().ok()?.atom;
    let pid_atom = conn.intern_atom(true, b"_NET_WM_PID").ok()?.reply().ok()?.atom;

    let active = conn
        .get_property(false, root, active_atom, AtomEnum::WINDOW, 0, 1)
        .ok()?
        .reply()
        .ok()?;
    let window = active.value32()?.next()?;

    let pid_prop = conn
        .get_property(false, window, pid_atom, AtomEnum::CARDINAL, 0, 1)
        .ok()?
        .reply()
        .ok()?;
    let pid = pid_prop.value32()?.next()?;

    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}
