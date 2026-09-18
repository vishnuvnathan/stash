//! Windows clipboard reader.
//!
//! The change counter (`GetClipboardSequenceNumber`) is read on every tick
//! without opening the clipboard, so an idle tick is one syscall. The clipboard
//! is only opened once the counter has actually moved, and the concealed-content
//! check runs before any content is read.

use super::types::{PollState, Snapshot};
use once_cell::sync::Lazy;
use std::ffi::c_void;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, GetClipboardData, GetClipboardSequenceNumber, IsClipboardFormatAvailable,
    OpenClipboard, RegisterClipboardFormatW,
};
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

const CF_UNICODETEXT: u32 = 13;
const CF_DIB: u32 = 8;

/// Set by password managers and anything else that must not be recorded.
/// Registering once is enough -- the atom is process-wide and stable.
static CF_EXCLUDE_FROM_MONITORING: Lazy<u32> = Lazy::new(|| {
    let name: Vec<u16> = "ExcludeClipboardContentFromMonitorProcessing\0"
        .encode_utf16()
        .collect();
    unsafe { RegisterClipboardFormatW(PCWSTR(name.as_ptr())) }
});

/// Closes the clipboard on every exit path, including early returns and panics.
struct ClipboardGuard;

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

/// Another process can hold the clipboard open briefly; a few short retries are
/// normal and far better than dropping the entry.
fn open_clipboard() -> Option<ClipboardGuard> {
    for attempt in 0..5 {
        let ok = unsafe { OpenClipboard(None) };
        if ok.is_ok() {
            return Some(ClipboardGuard);
        }
        std::thread::sleep(std::time::Duration::from_millis(10 * (attempt + 1)));
    }
    tracing::debug!("clipboard stayed locked by another process; skipping this change");
    None
}

pub fn poll(state: &mut PollState) -> Snapshot {
    let seq = unsafe { GetClipboardSequenceNumber() } as u64;
    if state.primed && seq == state.last_change {
        return Snapshot::Unchanged;
    }
    state.last_change = seq;

    // The first tick after launch establishes the baseline. Recording whatever
    // happened to be on the clipboard before Stash started would be surprising.
    if !state.primed {
        state.primed = true;
        return Snapshot::Unchanged;
    }

    let _guard = match open_clipboard() {
        Some(g) => g,
        None => return Snapshot::Unsupported,
    };

    // Hard requirement: check this before touching any content, and return
    // without ever materialising the bytes.
    let excluded = *CF_EXCLUDE_FROM_MONITORING;
    if excluded != 0 && unsafe { IsClipboardFormatAvailable(excluded) }.is_ok() {
        return Snapshot::Concealed;
    }

    if unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT) }.is_ok() {
        if let Some(text) = read_text() {
            return Snapshot::Text(text);
        }
    }

    if unsafe { IsClipboardFormatAvailable(CF_DIB) }.is_ok() {
        if let Some(png) = read_dib_as_png() {
            return Snapshot::Image(png);
        }
    }

    Snapshot::Unsupported
}

/// Locks an HGLOBAL and hands the bytes to `f`. Unlock always runs; its error
/// return just means the lock count reached zero, which is the expected result.
fn with_global<T>(handle: HANDLE, f: impl FnOnce(*const u8, usize) -> Option<T>) -> Option<T> {
    let hglobal = HGLOBAL(handle.0 as *mut c_void);
    let ptr = unsafe { GlobalLock(hglobal) } as *const u8;
    if ptr.is_null() {
        return None;
    }
    let size = unsafe { GlobalSize(hglobal) };
    let out = if size == 0 { None } else { f(ptr, size) };
    unsafe {
        let _ = GlobalUnlock(hglobal);
    }
    out
}

fn read_text() -> Option<String> {
    let handle = unsafe { GetClipboardData(CF_UNICODETEXT) }.ok()?;
    with_global(handle, |ptr, size| {
        let units = size / 2;
        let wide = unsafe { std::slice::from_raw_parts(ptr as *const u16, units) };
        // The buffer is NUL-terminated and GlobalSize rounds up, so cut at the
        // first NUL rather than trusting the reported length.
        let end = wide.iter().position(|&c| c == 0).unwrap_or(units);
        let s = String::from_utf16_lossy(&wide[..end]);
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    })
}

/// CF_DIB is a BMP without its 14-byte file header. We prepend one so the
/// `image` decoder can read it, then re-encode to PNG so every stored image has
/// one format and one stable hash regardless of the source app.
fn read_dib_as_png() -> Option<Vec<u8>> {
    let handle = unsafe { GetClipboardData(CF_DIB) }.ok()?;
    let dib = with_global(handle, |ptr, size| {
        Some(unsafe { std::slice::from_raw_parts(ptr, size) }.to_vec())
    })?;

    if dib.len() > super::types::MAX_IMAGE_BYTES {
        tracing::debug!(bytes = dib.len(), "clipboard image over limit; skipped");
        return None;
    }

    let bmp = dib_to_bmp(&dib)?;
    let decoded = image::load_from_memory_with_format(&bmp, image::ImageFormat::Bmp).ok()?;
    let mut rgba = decoded.to_rgba8();

    // Screenshots arrive as 32bpp BI_RGB where the fourth byte is padding left
    // at zero. Read literally that is a fully transparent image, so treat an
    // all-zero alpha channel as opaque.
    if rgba.pixels().all(|p| p.0[3] == 0) {
        for p in rgba.pixels_mut() {
            p.0[3] = 255;
        }
    }

    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(rgba)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    Some(png)
}

fn dib_to_bmp(dib: &[u8]) -> Option<Vec<u8>> {
    if dib.len() < 40 {
        return None;
    }
    let u32_at = |o: usize| -> Option<u32> {
        Some(u32::from_le_bytes(dib.get(o..o + 4)?.try_into().ok()?))
    };
    let u16_at = |o: usize| -> Option<u16> {
        Some(u16::from_le_bytes(dib.get(o..o + 2)?.try_into().ok()?))
    };

    let header_size = u32_at(0)? as usize;
    let bit_count = u16_at(14)?;
    let compression = u32_at(16)?;
    let colors_used = u32_at(32)?;

    // BI_BITFIELDS stores three masks after a plain 40-byte header; the V4/V5
    // headers carry them inline instead.
    let mask_bytes = if compression == 3 && header_size == 40 { 12 } else { 0 };

    let palette_entries = if bit_count <= 8 {
        if colors_used != 0 {
            colors_used as usize
        } else {
            1usize << bit_count
        }
    } else {
        colors_used as usize
    };

    let offset = 14 + header_size + mask_bytes + palette_entries * 4;
    if offset > dib.len() + 14 {
        return None;
    }

    let mut out = Vec::with_capacity(14 + dib.len());
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&((14 + dib.len()) as u32).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(offset as u32).to_le_bytes());
    out.extend_from_slice(dib);
    Some(out)
}

/// Executable stem of whatever window had focus when the copy happened.
pub fn foreground_app() -> Option<String> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }

        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return None;
        }

        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;

        let mut buf = [0u16; 260];
        let mut len = buf.len() as u32;
        let res = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(handle);
        res.ok()?;

        let path = String::from_utf16_lossy(&buf[..len as usize]);
        std::path::Path::new(&path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
    }
}
