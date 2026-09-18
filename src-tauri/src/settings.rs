use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// `default` at the container level matters: a settings.json written by an
/// older build has none of the window fields, and without it serde would reject
/// the whole file and silently drop the user's other preferences.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Screen-capture exclusion. On by default: a clipboard panel showing your
    /// last thousand copies is exactly what you do not want in a screen share.
    pub capture_exclusion: bool,
    pub launch_on_startup: bool,
    pub poll_interval_ms: u64,

    /// Enter pastes into the window you came from instead of only copying.
    ///
    /// On by default: copying without pasting is half an action, and the paste
    /// path falls back to a plain copy whenever it cannot restore focus, so the
    /// worst case is the old behaviour. Off gives you that old behaviour
    /// deliberately -- and on macOS and Linux, where synthesising the keystroke
    /// is not implemented, it is what happens regardless.
    pub auto_paste: bool,

    /// Reopen the panel where it was last dragged instead of centering it on
    /// the monitor under the cursor.
    ///
    /// On by default: moving a window and having it snap back is indistinguishable
    /// from the move not working. The cost is that the panel stops following the
    /// cursor across monitors, which is what the original centering bought -- so
    /// this is a toggle rather than a removal.
    pub remember_position: bool,

    /// Last known geometry. Size is restored whether or not `remember_position`
    /// is set; a window that forgets it was resized is just broken.
    pub window_width: Option<u32>,
    pub window_height: Option<u32>,
    pub window_x: Option<i32>,
    pub window_y: Option<i32>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            capture_exclusion: true,
            launch_on_startup: false,
            poll_interval_ms: 400,
            auto_paste: true,
            remember_position: true,
            window_width: None,
            window_height: None,
            window_x: None,
            window_y: None,
        }
    }
}

fn path(config_dir: &Path) -> PathBuf {
    config_dir.join("settings.json")
}

/// Missing or corrupt settings fall back to defaults rather than failing
/// startup -- the app must always come up.
pub fn load(config_dir: &Path) -> Settings {
    let p = path(config_dir);
    match std::fs::read_to_string(&p) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "settings.json unreadable; using defaults");
            Settings::default()
        }),
        Err(_) => Settings::default(),
    }
}

pub fn save(config_dir: &Path, settings: &Settings) -> Result<()> {
    std::fs::create_dir_all(config_dir)?;
    let raw = serde_json::to_string_pretty(settings)?;
    std::fs::write(path(config_dir), raw)?;
    Ok(())
}
