use crate::settings;
use crate::state::AppState;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewWindow};

pub const PANEL: &str = "panel";

/// Below this the panel is not a window the user moved, it is a minimise or a
/// hide artefact; persisting it would reopen the panel at nothing.
const MIN_SANE_SIZE: u32 = 200;

pub fn panel(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(PANEL)
}

/// Set while the user is dragging the panel by its title strip or a resize
/// grip. Handing the mouse to the OS drag loop makes the webview give up
/// keyboard focus, which fires the same `Focused(false)` that dismisses the
/// panel when you click away -- so without this the panel disappears the
/// instant you grab an edge. The frontend arms it before starting either drag;
/// the focus that returns when the drag loop ends disarms it.
static DRAG_ARMED: Mutex<Option<Instant>> = Mutex::new(None);

/// A ceiling on the guard, for the case where that returning focus never
/// arrives. Click-away-to-dismiss is the Linux stand-in for capture exclusion,
/// so a stuck guard must not be able to switch it off for the whole session.
const DRAG_GUARD_MAX: Duration = Duration::from_secs(30);

pub fn arm_drag_guard() {
    if let Ok(mut guard) = DRAG_ARMED.lock() {
        *guard = Some(Instant::now());
    }
}

pub fn disarm_drag_guard() {
    if let Ok(mut guard) = DRAG_ARMED.lock() {
        *guard = None;
    }
}

/// Number of native dialogs currently open.
///
/// A file picker takes focus, which fires the same `Focused(false)` that
/// dismisses the panel when you click away -- so opening one would hide the
/// window the dialog belongs to. The drag guard cannot be reused: it expires
/// after 30s, and choosing where to save a backup can easily take longer.
///
/// A counter rather than a flag because nothing forbids two dialogs, and it is
/// only ever changed through `HideGuard`, whose `Drop` runs on every exit path
/// including the error ones -- so it cannot be left armed.
static DIALOGS_OPEN: AtomicUsize = AtomicUsize::new(0);

/// Suppresses hide-on-blur for as long as it is alive.
pub struct HideGuard;

impl HideGuard {
    pub fn new() -> Self {
        DIALOGS_OPEN.fetch_add(1, Ordering::SeqCst);
        HideGuard
    }
}

impl Drop for HideGuard {
    fn drop(&mut self) {
        DIALOGS_OPEN.fetch_sub(1, Ordering::SeqCst);
    }
}

/// True while a native dialog owns the focus.
pub fn dialog_open() -> bool {
    DIALOGS_OPEN.load(Ordering::SeqCst) > 0
}

/// True when a `Focused(false)` is the drag loop taking focus rather than the
/// user clicking away.
pub fn drag_in_progress() -> bool {
    let Ok(mut guard) = DRAG_ARMED.lock() else {
        return false;
    };
    match *guard {
        Some(at) if at.elapsed() < DRAG_GUARD_MAX => true,
        Some(_) => {
            *guard = None;
            false
        }
        None => false,
    }
}

/// Global-shortcut handler. Visible -> hide; hidden -> re-center and show.
pub fn toggle(app: &AppHandle) {
    let Some(win) = panel(app) else { return };
    if win.is_visible().unwrap_or(false) {
        hide(&win);
    } else {
        show(&win);
    }
}

/// Restores the saved size, then either restores the saved position or centers
/// on whichever monitor holds the cursor, so on a multi-monitor desk the panel
/// opens where you are looking rather than on the primary display.
pub fn show(win: &WebviewWindow) {
    // Before anything shows: the window we may need to paste back into is the
    // foreground one right now, and stops being so the moment the panel appears.
    crate::paste::remember_foreground();

    restore_size(win);

    if !restore_position(win) {
        if let Err(e) = center_on_active_monitor(win) {
            tracing::debug!(error = %e, "could not center panel; showing at last position");
        }
    }

    let _ = win.show();
    let _ = win.set_focus();
    // The frontend listens for this to focus and select the search box.
    let _ = win.emit("panel-opened", ());
}

/// Geometry is written here rather than on every `Moved`/`Resized` event: a
/// single drag emits hundreds of those, and settings.json is a file.
pub fn hide(win: &WebviewWindow) {
    persist_geometry(win);
    let _ = win.hide();
}

fn restore_size(win: &WebviewWindow) {
    let Some((w, h)) = with_settings(win, |s| s.window_width.zip(s.window_height)).flatten() else {
        return;
    };
    if w < MIN_SANE_SIZE || h < MIN_SANE_SIZE {
        return;
    }
    let _ = win.set_size(PhysicalSize::new(w, h));
}

/// Returns true when a saved position was applied, so the caller knows not to
/// centre. A position on a monitor that is no longer connected is refused --
/// unplugging a display must not strand the panel where it cannot be reached.
fn restore_position(win: &WebviewWindow) -> bool {
    let remember = with_settings(win, |s| s.remember_position).unwrap_or(false);
    if !remember {
        return false;
    }

    let Some((x, y)) = with_settings(win, |s| s.window_x.zip(s.window_y)).flatten() else {
        return false;
    };
    if !point_on_some_monitor(win, x, y) {
        tracing::debug!(x, y, "saved panel position is off-screen; centering instead");
        return false;
    }

    win.set_position(PhysicalPosition::new(x, y)).is_ok()
}

fn point_on_some_monitor(win: &WebviewWindow, x: i32, y: i32) -> bool {
    let Ok(monitors) = win.available_monitors() else {
        return false;
    };
    monitors.iter().any(|m| {
        let p = m.position();
        let s = m.size();
        x >= p.x && y >= p.y && x < p.x + s.width as i32 && y < p.y + s.height as i32
    })
}

fn persist_geometry(win: &WebviewWindow) {
    let (Ok(size), Ok(pos)) = (win.outer_size(), win.outer_position()) else {
        return;
    };
    if size.width < MIN_SANE_SIZE || size.height < MIN_SANE_SIZE {
        return;
    }

    let app = win.app_handle();
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };

    let snapshot = {
        let Ok(mut guard) = state.settings.lock() else {
            return;
        };
        let unchanged = guard.window_width == Some(size.width)
            && guard.window_height == Some(size.height)
            && guard.window_x == Some(pos.x)
            && guard.window_y == Some(pos.y);
        if unchanged {
            return;
        }
        guard.window_width = Some(size.width);
        guard.window_height = Some(size.height);
        guard.window_x = Some(pos.x);
        guard.window_y = Some(pos.y);
        guard.clone()
    };

    let Ok(config_dir) = app.path().app_config_dir() else {
        return;
    };
    if let Err(e) = settings::save(&config_dir, &snapshot) {
        tracing::warn!(error = %e, "could not persist window geometry");
    }
}

/// Reads one field out of the settings mutex. `None` means state is not managed
/// yet or the lock is poisoned -- both are "carry on with the default".
fn with_settings<T>(win: &WebviewWindow, f: impl FnOnce(&settings::Settings) -> T) -> Option<T> {
    let app = win.app_handle();
    let state = app.try_state::<AppState>()?;
    let guard = state.settings.lock().ok()?;
    Some(f(&guard))
}

fn center_on_active_monitor(win: &WebviewWindow) -> tauri::Result<()> {
    let cursor = win.cursor_position()?;
    let monitor = win
        .monitor_from_point(cursor.x, cursor.y)?
        .or(win.primary_monitor()?);

    let Some(monitor) = monitor else {
        return win.center();
    };

    let area = monitor.size();
    let origin = monitor.position();
    let size = win.outer_size()?;

    let x = origin.x + ((area.width as i32 - size.width as i32) / 2);
    let y = origin.y + ((area.height as i32 - size.height as i32) / 2);
    win.set_position(PhysicalPosition::new(x, y))
}
