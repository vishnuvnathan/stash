use crate::window;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter};

/// Builds the tray icon and its menu. Quit is the only path that ends the
/// process -- closing the window merely hides it.
pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show Stash", true, Some("CmdOrCtrl+Shift+V"))?;
    let settings = MenuItem::with_id(app, "settings", "Settings\u{2026}", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Stash", true, None::<&str>)?;

    let menu = Menu::with_items(app, &[&show, &settings, &sep, &quit])?;

    let mut builder = TrayIconBuilder::with_id("stash-tray")
        .tooltip("Stash")
        .menu(&menu)
        .show_menu_on_left_click(false);

    // Falling back to no icon is better than failing startup: the menu still
    // works, which is the only way to quit.
    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }

    builder
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => {
                if let Some(win) = window::panel(app) {
                    window::show(&win);
                }
            }
            "settings" => {
                if let Some(win) = window::panel(app) {
                    window::show(&win);
                    let _ = win.emit("open-settings", ());
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            // Left click toggles, matching the shortcut. Right click is left to
            // the platform so the menu still opens.
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                window::toggle(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}
