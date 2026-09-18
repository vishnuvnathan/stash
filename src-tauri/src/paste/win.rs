//! Windows paste-back: restore the previous foreground window, send Ctrl+V.
//!
//! The awkward part is not the keystroke, it is the focus. Windows only lets
//! the process that currently owns the foreground give it away, which is
//! exactly our situation -- the panel has focus when the user presses Enter.
//! Even so `SetForegroundWindow` can be refused, so nothing here trusts it:
//! the keystroke is only sent once `GetForegroundWindow` actually reports the
//! target, and a timeout reports failure rather than typing into whatever
//! happens to be focused instead.

use super::PasteStatus;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VIRTUAL_KEY,
    VK_CONTROL, VK_V,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowThreadProcessId, IsWindow, SetForegroundWindow,
};

/// The window that had focus when the panel was last opened.
///
/// Stored as `isize` because `HWND` is a raw pointer and therefore not `Send`,
/// and this is read from a different thread than the one that wrote it. The
/// handle is re-validated with `IsWindow` before use -- the app may well have
/// been closed while the panel was up.
static PREVIOUS: Mutex<Option<isize>> = Mutex::new(None);

/// How long to wait for the focus change to land before giving up. Generous
/// enough for a loaded machine, short enough that a failure still feels
/// immediate.
const FOREGROUND_TIMEOUT: Duration = Duration::from_millis(400);
const POLL_STEP: Duration = Duration::from_millis(10);

pub fn remember_foreground() {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return;
    }

    // Our own panel is never the paste target. Without this, toggling the panel
    // while it is already focused would overwrite the real target with itself.
    if is_own_window(hwnd) {
        return;
    }

    if let Ok(mut guard) = PREVIOUS.lock() {
        *guard = Some(hwnd.0 as isize);
    }
}

/// True when the window belongs to this process.
fn is_own_window(hwnd: HWND) -> bool {
    let mut pid: u32 = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid == std::process::id()
}

pub fn support() -> PasteStatus {
    PasteStatus { supported: true, pasted: false, reason: None }
}

pub fn paste_into_previous() -> PasteStatus {
    let Some(raw) = PREVIOUS.lock().ok().and_then(|g| *g) else {
        return PasteStatus::failed("nothing was focused when the panel opened");
    };

    let target = HWND(raw as *mut std::ffi::c_void);
    if !unsafe { IsWindow(Some(target)) }.as_bool() {
        return PasteStatus::failed("the window you copied from has closed");
    }

    if !focus(target) {
        return PasteStatus::failed(
            "could not focus the previous window; the item is on your clipboard",
        );
    }

    match send_paste() {
        Ok(()) => PasteStatus::pasted(),
        Err(reason) => PasteStatus::failed(reason),
    }
}

/// Brings `target` to the foreground and waits until Windows agrees that it is
/// there. Returns false rather than pressing on, because sending Ctrl+V to an
/// unknown window is worse than not pasting at all.
fn focus(target: HWND) -> bool {
    unsafe {
        let _ = SetForegroundWindow(target);
    }
    if wait_for_foreground(target) {
        return true;
    }

    // Second attempt: borrow the target thread's input state so the foreground
    // change is allowed. This is the long-standing workaround for the cases
    // where a bare SetForegroundWindow is refused.
    let target_thread = unsafe { GetWindowThreadProcessId(target, None) };
    if target_thread == 0 {
        return false;
    }
    let this_thread = unsafe { GetCurrentThreadId() };
    if target_thread == this_thread {
        return false;
    }

    unsafe {
        if !AttachThreadInput(this_thread, target_thread, true).as_bool() {
            return false;
        }
        let _ = SetForegroundWindow(target);
        let ok = wait_for_foreground(target);
        let _ = AttachThreadInput(this_thread, target_thread, false);
        ok
    }
}

fn wait_for_foreground(target: HWND) -> bool {
    let deadline = Instant::now() + FOREGROUND_TIMEOUT;
    loop {
        if unsafe { GetForegroundWindow() }.0 == target.0 {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(POLL_STEP);
    }
}

fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { Default::default() },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Ctrl down, V down, V up, Ctrl up -- in one `SendInput` call so nothing can
/// interleave between the modifier and the key.
fn send_paste() -> Result<(), String> {
    let inputs = [
        key(VK_CONTROL, false),
        key(VK_V, false),
        key(VK_V, true),
        key(VK_CONTROL, true),
    ];

    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize == inputs.len() {
        Ok(())
    } else {
        // The usual cause is UIPI: a window running elevated will not accept
        // synthetic input from a process that is not.
        Err("Windows blocked the keystroke (an elevated window?); the item is on your clipboard"
            .to_string())
    }
}
