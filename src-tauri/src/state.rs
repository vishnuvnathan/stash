use crate::db::Db;
use crate::settings::Settings;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Remembers the hash `copy_item` just wrote to the clipboard so the watcher
/// does not immediately re-record our own paste-back as a fresh entry.
///
/// One-shot on purpose: it is cleared by the first matching snapshot, so a
/// genuine later copy of the same content still registers as a bump.
#[derive(Clone, Default)]
pub struct SelfCopyGuard(Arc<Mutex<Option<String>>>);

impl SelfCopyGuard {
    pub fn expect(&self, hash: String) {
        if let Ok(mut g) = self.0.lock() {
            *g = Some(hash);
        }
    }

    pub fn take_if_matches(&self, hash: &str) -> bool {
        let Ok(mut g) = self.0.lock() else {
            return false;
        };
        if g.as_deref() == Some(hash) {
            *g = None;
            true
        } else {
            false
        }
    }
}

/// Tracks the password currently sitting on the clipboard so it can be taken
/// back off again.
///
/// The hash alone is not enough to decide whether a clear is safe. Copy the
/// same password twice and the second copy's content is indistinguishable from
/// the first's, so the first timer would wipe it early -- at 30s from the first
/// copy rather than the second. The generation counter is what makes "is this
/// still the copy I armed?" answerable.
#[derive(Clone, Default)]
pub struct PasswordClipboard {
    armed: Arc<Mutex<Option<(u64, String)>>>,
    generation: Arc<AtomicU64>,
}

impl PasswordClipboard {
    /// Records a freshly copied password and returns the generation the
    /// expiring task must present to be allowed to clear it.
    pub fn arm(&self, hash: String) -> u64 {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        if let Ok(mut g) = self.armed.lock() {
            *g = Some((generation, hash));
        }
        generation
    }

    /// The hash to clear, if `generation` is still the newest copy. `None` means
    /// something has been copied since and this timer has been superseded.
    pub fn take_if_current(&self, generation: u64) -> Option<String> {
        let mut g = self.armed.lock().ok()?;
        match g.as_ref() {
            Some((armed_gen, hash)) if *armed_gen == generation => {
                let hash = hash.clone();
                *g = None;
                Some(hash)
            }
            _ => None,
        }
    }

    /// Called when a password is deliberately superseded, so a pending timer
    /// stops considering itself current.
    pub fn disarm(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut g) = self.armed.lock() {
            *g = None;
        }
    }
}

/// Everything the commands need, registered once as Tauri managed state.
pub struct AppState {
    pub db: Db,
    pub settings: Mutex<Settings>,
    pub self_copy: SelfCopyGuard,
    pub password_clipboard: PasswordClipboard,
}
