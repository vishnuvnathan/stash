mod archive;
mod backup;
mod capture;
mod clipboard;
mod commands;
mod db;
mod detect;
mod paste;
mod query;
mod secret;
mod settings;
mod startup;
mod state;
mod transform;
mod tray;
mod window;

use state::{AppState, PasswordClipboard, SelfCopyGuard};
use std::sync::Mutex;
use tauri::{Manager, WindowEvent};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

const TOGGLE_KEY: Code = Code::KeyV;

fn toggle_shortcut() -> Shortcut {
    // CmdOrCtrl+Shift+V: Cmd on macOS, Ctrl elsewhere.
    #[cfg(target_os = "macos")]
    let modifiers = Modifiers::SUPER | Modifiers::SHIFT;
    #[cfg(not(target_os = "macos"))]
    let modifiers = Modifiers::CONTROL | Modifiers::SHIFT;

    Shortcut::new(Some(modifiers), TOGGLE_KEY)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("STASH_LOG")
                .unwrap_or_else(|_| "stash_lib=info,warn".into()),
        )
        .init();

    let shortcut = toggle_shortcut();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, received, event| {
                    // Fire on press only; the release event would toggle straight back.
                    if received == &shortcut && event.state() == ShortcutState::Pressed {
                        window::toggle(app);
                    }
                })
                .build(),
        )
        .setup(move |app| {
            let handle = app.handle().clone();

            let config_dir = handle.path().app_config_dir()?;
            let data_dir = handle.path().app_data_dir()?;

            let loaded = settings::load(&config_dir);

            // The database must exist before the watcher or any command runs,
            // so this one step is intentionally blocking.
            let db = tauri::async_runtime::block_on(db::init(&data_dir))
                .map_err(|e| format!("database init failed: {e}"))?;

            let self_copy = SelfCopyGuard::default();

            app.manage(AppState {
                db: db.clone(),
                settings: Mutex::new(loaded.clone()),
                self_copy: self_copy.clone(),
                password_clipboard: PasswordClipboard::default(),
            });

            // Secrets written before encryption landed are still plaintext in
            // the database. Re-seal them now rather than waiting for each to be
            // edited, which might be never.
            {
                let pool = db.pool.clone();
                tauri::async_runtime::spawn(async move {
                    match db::queries::upgrade_unsealed_secrets(&pool).await {
                        Ok((0, 0)) => {}
                        Ok((upgraded, failed)) => {
                            tracing::info!(upgraded, failed, "re-sealed plaintext secrets");
                        }
                        Err(e) => tracing::warn!(error = %e, "secret upgrade pass failed"),
                    }
                });
            }

            startup::reconcile(&handle, loaded.launch_on_startup);
            paste::warn_if_unsupported();

            tray::build(&handle)?;

            if let Some(win) = window::panel(&handle) {
                let status = capture::apply(&win, loaded.capture_exclusion);
                if !status.supported {
                    if let Some(reason) = &status.reason {
                        tracing::info!(reason, "capture exclusion unavailable");
                    }
                }
            }

            app.global_shortcut().register(toggle_shortcut())?;

            clipboard::spawn(handle.clone(), db.clone(), self_copy);
            db::retention::spawn(db);

            // No Dock icon on macOS; skipTaskbar covers Windows and Linux.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            Ok(())
        })
        .on_window_event(|win, event| match event {
            // Closing must not quit: the tray Quit item is the only exit.
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                hide_panel_window(win);
            }
            // Clicking away dismisses the panel, which is also the Linux
            // mitigation for having no capture exclusion. A move or resize
            // drag blurs the webview too, and that one must not dismiss it.
            WindowEvent::Focused(false) => {
                if !window::drag_in_progress() && !window::dialog_open() {
                    hide_panel_window(win);
                }
            }
            // Focus comes back when the drag loop ends.
            WindowEvent::Focused(true) => window::disarm_drag_guard(),
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            commands::search_items,
            commands::transforms_for,
            commands::get_item,
            commands::copy_item,
            commands::paste_item,
            commands::delete_item,
            commands::toggle_pin,
            commands::save_note,
            commands::list_source_apps,
            commands::get_settings,
            commands::set_settings,
            commands::set_capture_exclusion,
            commands::security_info,
            commands::hide_panel,
            commands::begin_window_drag,
            commands::read_blob,
            commands::list_folders,
            commands::create_folder,
            commands::rename_folder,
            commands::delete_folder,
            commands::set_item_folder,
            commands::save_credential,
            commands::get_credential,
            commands::reveal_password,
            commands::copy_credential_field,
            commands::paste_credential_field,
            commands::export_data,
            commands::import_data,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Stash");
}

/// Routes every dismissal through `window::hide` so the panel's size and
/// position are saved on the way out. The event handler is given a `Window`,
/// not the `WebviewWindow` that carries those helpers, hence the lookup.
fn hide_panel_window(win: &tauri::Window) {
    match window::panel(win.app_handle()) {
        Some(panel) => window::hide(&panel),
        None => {
            let _ = win.hide();
        }
    }
}
