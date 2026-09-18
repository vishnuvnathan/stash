//! Every IPC entry point. One file so the surface is auditable at a glance and
//! mirrors `src/lib/ipc.ts` one-to-one.

use crate::archive;
use crate::backup;
use crate::capture::{self, CaptureStatus};
use crate::clipboard::hash_bytes;
use crate::db::model::{
    ContentType, CredentialBody, CredentialView, Filters, Folder, Item, ItemKind, NewItem,
};
use crate::db::{model::now_millis, queries};
use crate::detect;
use crate::paste::{self, PasteStatus};
use crate::query;
use crate::secret;
use crate::settings::{self, Settings};
use crate::startup;
use crate::state::{AppState, SelfCopyGuard};
use crate::transform::{self, Transform};
use crate::window;
use std::path::PathBuf;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

/// How long a copied password is allowed to sit on the clipboard.
///
/// Not a setting. A number the user can raise to "never" is a number that ends
/// up at "never", and 30s is long enough to switch windows and paste.
const PASSWORD_CLIPBOARD_TTL: Duration = Duration::from_secs(30);

/// Commands return a plain string error: the frontend only ever surfaces it as
/// a toast, and this keeps a single, stable shape across the whole surface.
type CmdResult<T> = Result<T, String>;

fn err<E: std::fmt::Display>(context: &'static str) -> impl Fn(E) -> String {
    move |e| {
        tracing::warn!(context, error = %e, "command failed");
        format!("{context}: {e}")
    }
}

/// The one place the query language is applied.
///
/// `query` arrives exactly as typed. Operators are lifted out of it here and
/// merged with the chip filters, so the residual text that reaches FTS never
/// contains `app:Code` — which would otherwise match nothing and make the
/// operator look broken.
#[tauri::command]
pub async fn search_items(
    state: State<'_, AppState>,
    query: String,
    filters: Filters,
    limit: i64,
    offset: i64,
) -> CmdResult<Vec<Item>> {
    let parsed = query::parse(&query);
    let merged = query::merge(&filters, &parsed.filters);

    queries::search(&state.db.pool, &parsed.text, &merged, limit.clamp(1, 500), offset.max(0))
        .await
        .map_err(err("search failed"))
}

/// The transforms worth offering for this item, with their labels.
///
/// Asked per item rather than computed on the frontend: whether a blob is
/// decodable base64 is decided by trying, and the trying lives in Rust next to
/// the code that would perform it.
#[tauri::command]
pub async fn transforms_for(
    state: State<'_, AppState>,
    id: String,
) -> CmdResult<Vec<TransformOption>> {
    let item = queries::get(&state.db.pool, &id)
        .await
        .map_err(err("load failed"))?
        .ok_or_else(|| "item no longer exists".to_string())?;

    // An image has no text to transform, and a credential's content is its JSON
    // body rather than anything the user would want pasted.
    if item.is_credential() || item.blob_path.is_some() {
        return Ok(Vec::new());
    }

    let content = item.content.unwrap_or_default();
    Ok(transform::applicable_to(&content)
        .into_iter()
        .map(|t| TransformOption { id: t, label: t.label() })
        .collect())
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransformOption {
    id: Transform,
    label: &'static str,
}

#[tauri::command]
pub async fn get_item(state: State<'_, AppState>, id: String) -> CmdResult<Option<Item>> {
    queries::get(&state.db.pool, &id).await.map_err(err("load failed"))
}

/// Writes the item back to the system clipboard. The hash is registered with
/// the self-copy guard first, so the watcher recognises the resulting clipboard
/// change as ours and does not record a duplicate.
/// `transform` is `None` for a plain copy. When set, the transformed text goes
/// to the clipboard and the stored item is left exactly as it was -- the history
/// records what you copied, not what you happened to paste once.
#[tauri::command]
pub async fn copy_item(
    state: State<'_, AppState>,
    id: String,
    transform: Option<Transform>,
) -> CmdResult<()> {
    let item = load_copyable(&state, &id).await?;
    let item = apply_transform(item, transform)?;
    write_item_to_clipboard(state.db.blob_dir.clone(), state.self_copy.clone(), item).await?;
    record_use(&state, &id).await;
    Ok(())
}

/// Copy, hide, then paste into whatever had focus before the panel opened.
///
/// The whole sequence lives in Rust so the ordering and the wait for the focus
/// change are not at the mercy of three separate IPC round trips. A failure to
/// paste is not an error: the item is on the clipboard either way, and the
/// returned status is what tells the user to press Ctrl+V themselves.
#[tauri::command]
pub async fn paste_item(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    transform: Option<Transform>,
) -> CmdResult<PasteStatus> {
    let item = load_copyable(&state, &id).await?;
    let item = apply_transform(item, transform)?;
    write_item_to_clipboard(state.db.blob_dir.clone(), state.self_copy.clone(), item).await?;
    record_use(&state, &id).await;
    Ok(hide_and_paste(&app).await)
}

/// Rewrites the in-memory copy's `content` only. Nothing is persisted, and an
/// image is returned untouched because there is no text to transform.
fn apply_transform(mut item: Item, transform: Option<Transform>) -> CmdResult<Item> {
    let Some(t) = transform else { return Ok(item) };

    if item.blob_path.is_some() {
        return Err("images cannot be transformed".to_string());
    }

    let source = item.content.unwrap_or_default();
    item.content = Some(transform::apply(t, &source).map_err(|e| format!("{}: {e}", t.label()))?);
    Ok(item)
}

/// Usage tracking is best-effort: failing to count a use must never fail the
/// copy the user actually asked for.
async fn record_use(state: &AppState, id: &str) {
    if let Err(e) = queries::bump_usage(&state.db.pool, id).await {
        tracing::debug!(id, error = %e, "could not record item use");
    }
}

/// Loads an item and rejects the one kind that must never go to the clipboard
/// whole.
async fn load_copyable(state: &AppState, id: &str) -> CmdResult<Item> {
    let item = queries::get(&state.db.pool, id)
        .await
        .map_err(err("load failed"))?
        .ok_or_else(|| "item no longer exists".to_string())?;

    // A credential's `content` is the JSON body, not something anyone wants on
    // their clipboard. Fields are copied individually, through their own command.
    if item.is_credential() {
        return Err("use copy_credential_field for credentials".to_string());
    }
    Ok(item)
}

async fn write_item_to_clipboard(
    blob_dir: PathBuf,
    guard: SelfCopyGuard,
    item: Item,
) -> CmdResult<()> {
    // arboard touches platform clipboard APIs and blocks; keep it off the async
    // runtime's worker threads.
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let mut cb = arboard::Clipboard::new().map_err(|e| format!("clipboard unavailable: {e}"))?;

        match item.blob_path.as_deref() {
            Some(rel) => {
                let path = blob_dir.join(rel);
                let img = image::open(&path).map_err(|e| format!("could not read image: {e}"))?;
                let rgba = img.to_rgba8();
                let (w, h) = rgba.dimensions();

                if let Some(hash) = item.hash.clone() {
                    guard.expect(hash);
                }
                cb.set_image(arboard::ImageData {
                    width: w as usize,
                    height: h as usize,
                    bytes: std::borrow::Cow::Owned(rgba.into_raw()),
                })
                .map_err(|e| format!("could not set image: {e}"))
            }
            None => {
                let text = item.content.unwrap_or_default();
                guard.expect(hash_bytes(text.as_bytes()));
                cb.set_text(text).map_err(|e| format!("could not set text: {e}"))
            }
        }
    })
    .await
    .map_err(err("clipboard task failed"))?
}

/// Hides the panel, then hands focus back and sends the keystroke.
///
/// The hide has to come first -- the panel is the foreground window at this
/// point, and the target cannot be focused while it is still up.
async fn hide_and_paste(app: &AppHandle) -> PasteStatus {
    if let Some(win) = window::panel(app) {
        window::hide(&win);
    }

    // `paste_into_previous` sleeps while it waits for the foreground change.
    tauri::async_runtime::spawn_blocking(paste::paste_into_previous)
        .await
        .unwrap_or_else(|e| PasteStatus::failed(format!("paste task failed: {e}")))
}

#[tauri::command]
pub async fn delete_item(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    queries::soft_delete(&state.db.pool, &id)
        .await
        .map_err(err("delete failed"))
}

#[tauri::command]
pub async fn toggle_pin(state: State<'_, AppState>, id: String, pinned: bool) -> CmdResult<()> {
    queries::set_pinned(&state.db.pool, &id, pinned)
        .await
        .map_err(err("pin failed"))
}

/// Creates a note when `id` is absent, updates it when present. Notes live in
/// `items` alongside clips, so they fall out of the same search with no
/// special-casing on either side.
#[tauri::command]
pub async fn save_note(
    state: State<'_, AppState>,
    id: Option<String>,
    title: String,
    content: String,
) -> CmdResult<Item> {
    let title = if title.trim().is_empty() {
        detect::derive_title(&content, 120)
    } else {
        title
    };

    match id {
        Some(id) => {
            // `update_note` writes markdown straight into `content`. Aimed at a
            // credential that would destroy the JSON body and orphan its secret
            // row, so the note editor is not allowed near one.
            let existing = queries::get(&state.db.pool, &id)
                .await
                .map_err(err("load failed"))?
                .ok_or_else(|| "note no longer exists".to_string())?;
            if existing.is_credential() {
                return Err("this item is a credential; edit it in the credential editor".to_string());
            }

            queries::update_note(&state.db.pool, &id, &title, &content)
                .await
                .map_err(err("note save failed"))?;
            queries::get(&state.db.pool, &id)
                .await
                .map_err(err("note reload failed"))?
                .ok_or_else(|| "note no longer exists".to_string())
        }
        None => queries::insert_item(
            &state.db.pool,
            NewItem {
                kind: ItemKind::Note,
                content_type: ContentType::Markdown,
                content: Some(content),
                blob_path: None,
                hash: None,
                source_app: None,
                title: Some(title),
                folder_id: None,
            },
            now_millis(),
        )
        .await
        .map_err(err("note create failed")),
    }
}

#[tauri::command]
pub async fn list_source_apps(state: State<'_, AppState>) -> CmdResult<Vec<String>> {
    queries::list_source_apps(&state.db.pool)
        .await
        .map_err(err("source app list failed"))
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> CmdResult<Settings> {
    state
        .settings
        .lock()
        .map(|s| s.clone())
        .map_err(|_| "settings lock poisoned".to_string())
}

#[tauri::command]
pub fn set_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    mut settings: Settings,
) -> CmdResult<Settings> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(err("no config dir"))?;

    // Window geometry belongs to the backend, which updates it on every hide.
    // The frontend's copy is a snapshot taken when the panel opened, so trusting
    // it here would revert a window the user moved since then.
    {
        let current = state
            .settings
            .lock()
            .map_err(|_| "settings lock poisoned".to_string())?;
        settings.window_width = current.window_width;
        settings.window_height = current.window_height;
        settings.window_x = current.window_x;
        settings.window_y = current.window_y;
    }

    // Before the save, so a registry write that fails does not leave a stored
    // setting claiming something the OS is not going to do.
    startup::apply(&app, settings.launch_on_startup)?;

    settings::save(&config_dir, &settings).map_err(err("could not write settings"))?;

    if let Some(win) = window::panel(&app) {
        capture::apply(&win, settings.capture_exclusion);
    }

    let mut guard = state
        .settings
        .lock()
        .map_err(|_| "settings lock poisoned".to_string())?;
    *guard = settings.clone();
    Ok(settings)
}

/// What this build can actually guarantee, so the settings panel can describe
/// it instead of asserting it.
///
/// Both answers are platform-dependent and neither is visible from the
/// frontend: secrets are only encrypted where DPAPI exists, and pasting only
/// works where synthesising a keystroke is implemented.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecurityInfo {
    secrets_encrypted: bool,
    paste_supported: bool,
    paste_reason: Option<String>,
}

#[tauri::command]
pub fn security_info() -> SecurityInfo {
    let paste = paste::support();
    SecurityInfo {
        secrets_encrypted: secret::encryption_available(),
        paste_supported: paste.supported,
        paste_reason: paste.reason,
    }
}

#[tauri::command]
pub fn set_capture_exclusion(app: AppHandle, enabled: bool) -> CmdResult<CaptureStatus> {
    let win = window::panel(&app).ok_or_else(|| "panel window missing".to_string())?;
    Ok(capture::apply(&win, enabled))
}

#[tauri::command]
pub fn hide_panel(app: AppHandle) -> CmdResult<()> {
    if let Some(win) = window::panel(&app) {
        window::hide(&win);
    }
    Ok(())
}

/// Called by the panel's title strip and resize grips immediately before they
/// start an OS drag. The drag itself stays in JS; this only arms the guard that
/// stops the drag's focus loss from dismissing the panel -- see
/// `window::drag_in_progress`.
#[tauri::command]
pub fn begin_window_drag() -> CmdResult<()> {
    window::arm_drag_guard();
    Ok(())
}

/* -- folders --------------------------------------------------------------- */

#[tauri::command]
pub async fn list_folders(state: State<'_, AppState>) -> CmdResult<Vec<Folder>> {
    queries::list_folders(&state.db.pool)
        .await
        .map_err(err("folder list failed"))
}

#[tauri::command]
pub async fn create_folder(state: State<'_, AppState>, name: String) -> CmdResult<Folder> {
    queries::create_folder(&state.db.pool, &name)
        .await
        .map_err(err("folder create failed"))
}

#[tauri::command]
pub async fn rename_folder(state: State<'_, AppState>, id: String, name: String) -> CmdResult<()> {
    queries::rename_folder(&state.db.pool, &id, &name)
        .await
        .map_err(err("folder rename failed"))
}

/// Items in the folder are unfiled, not deleted -- `folder_id` is
/// ON DELETE SET NULL.
#[tauri::command]
pub async fn delete_folder(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    queries::delete_folder(&state.db.pool, &id)
        .await
        .map_err(err("folder delete failed"))
}

#[tauri::command]
pub async fn set_item_folder(
    state: State<'_, AppState>,
    id: String,
    folder_id: Option<String>,
) -> CmdResult<()> {
    queries::set_item_folder(&state.db.pool, &id, folder_id.as_deref())
        .await
        .map_err(err("could not move item"))
}

/* -- credentials ----------------------------------------------------------- */

/// Creates when `id` is absent, updates when present.
///
/// `password: None` leaves the stored secret exactly as it is; `Some("")`
/// clears it. That contract is what lets the editor save a label change without
/// ever pulling the plaintext across IPC to put it back.
///
/// The item is stored as `kind='note'` so retention never touches it, and
/// `content_type='credential'` so the UI can tell it apart. Only the searchable
/// fields go in `items.content`; the password goes to `item_secrets`.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn save_credential(
    state: State<'_, AppState>,
    id: Option<String>,
    title: String,
    username: Option<String>,
    password: Option<String>,
    url: Option<String>,
    notes: Option<String>,
    folder_id: Option<String>,
) -> CmdResult<Item> {
    let blank = |s: Option<String>| s.filter(|v| !v.trim().is_empty());

    let body = CredentialBody {
        username: blank(username),
        url: blank(url),
        notes: blank(notes),
    };
    let content = serde_json::to_string(&body).map_err(err("could not encode credential"))?;

    let title = if title.trim().is_empty() {
        body.username.clone().unwrap_or_else(|| "Untitled".to_string())
    } else {
        title
    };

    let item = match id {
        Some(id) => {
            queries::update_credential(
                &state.db.pool,
                &id,
                &title,
                &content,
                folder_id.as_deref(),
            )
            .await
            .map_err(err("credential save failed"))?;
            queries::get(&state.db.pool, &id)
                .await
                .map_err(err("credential reload failed"))?
                .ok_or_else(|| "credential no longer exists".to_string())?
        }
        None => queries::insert_item(
            &state.db.pool,
            NewItem {
                kind: ItemKind::Note,
                content_type: ContentType::Credential,
                content: Some(content),
                blob_path: None,
                hash: None,
                source_app: None,
                title: Some(title),
                folder_id,
            },
            now_millis(),
        )
        .await
        .map_err(err("credential create failed"))?,
    };

    match password {
        None => {}
        Some(p) if p.is_empty() => queries::delete_secret(&state.db.pool, &item.id)
            .await
            .map_err(err("could not clear password"))?,
        Some(p) => {
            // Sealing is fallible now, and a failure must not fall back to
            // writing the plaintext: the UI would report a saved, encrypted
            // password that is neither.
            let (stored, enc) = secret::seal(&p).map_err(err("could not encrypt password"))?;
            queries::upsert_secret(&state.db.pool, &item.id, &stored, enc)
                .await
                .map_err(err("could not store password"))?;
        }
    }

    Ok(item)
}

/// Everything about a credential except the password itself.
#[tauri::command]
pub async fn get_credential(state: State<'_, AppState>, id: String) -> CmdResult<CredentialView> {
    let item = queries::get(&state.db.pool, &id)
        .await
        .map_err(err("load failed"))?
        .ok_or_else(|| "credential no longer exists".to_string())?;

    if !item.is_credential() {
        return Err("this item is not a credential".to_string());
    }

    let body = CredentialBody::parse(item.content.as_deref());
    let has_password = queries::has_secret(&state.db.pool, &id)
        .await
        .map_err(err("could not check password"))?;

    Ok(CredentialView {
        item,
        username: body.username,
        url: body.url,
        notes: body.notes,
        has_password,
    })
}

/// The only command in the app that returns a plaintext secret. It exists
/// because "show me what I stored" is a real need; it is reached by an explicit
/// click, and the frontend drops the value as soon as the selection changes.
#[tauri::command]
pub async fn reveal_password(state: State<'_, AppState>, id: String) -> CmdResult<String> {
    let (stored, enc) = queries::read_secret(&state.db.pool, &id)
        .await
        .map_err(err("could not read password"))?
        .ok_or_else(|| "no password stored for this item".to_string())?;

    secret::open(&stored, &enc).map_err(err("could not decode password"))
}

/// Copies one field straight to the clipboard from inside Rust.
///
/// The password never crosses IPC on this path -- it is read, unsealed and
/// written to the clipboard without the webview ever seeing it.
#[tauri::command]
pub async fn copy_credential_field(
    state: State<'_, AppState>,
    id: String,
    field: String,
) -> CmdResult<()> {
    let text = credential_field_text(&state, &id, &field).await?;
    let hash = hash_bytes(text.as_bytes());

    write_text_to_clipboard(state.self_copy.clone(), text, hash.clone()).await?;

    // A password that stays on the clipboard until the next copy is a password
    // sitting in every paste target, forever. Arm the clear before returning.
    if field == "password" {
        arm_password_expiry(&state, hash);
    } else {
        // Copying a username replaces the password on the clipboard, so a
        // pending timer must stop thinking it owns what is there now.
        state.password_clipboard.disarm();
    }

    record_use(&state, &id).await;
    Ok(())
}

/// Copies a credential field and pastes it into the window the panel was opened
/// from.
///
/// Username only. A password lands on the clipboard and stays there for the
/// user to place deliberately: the field that had focus before the panel opened
/// is not reliably the password field, and typing a password into the wrong one
/// is not a recoverable mistake.
#[tauri::command]
pub async fn paste_credential_field(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    field: String,
) -> CmdResult<PasteStatus> {
    if field != "username" {
        return Err(format!("'{field}' is copied, not pasted"));
    }

    let text = credential_field_text(&state, &id, &field).await?;
    let hash = hash_bytes(text.as_bytes());
    write_text_to_clipboard(state.self_copy.clone(), text, hash).await?;
    state.password_clipboard.disarm();
    record_use(&state, &id).await;

    Ok(hide_and_paste(&app).await)
}

/// Resolves one field to its plaintext. The password path is the only place a
/// secret is unsealed outside `reveal_password`.
async fn credential_field_text(
    state: &AppState,
    id: &str,
    field: &str,
) -> CmdResult<String> {
    match field {
        "username" => {
            let item = queries::get(&state.db.pool, id)
                .await
                .map_err(err("load failed"))?
                .ok_or_else(|| "credential no longer exists".to_string())?;
            CredentialBody::parse(item.content.as_deref())
                .username
                .ok_or_else(|| "no username stored for this item".to_string())
        }
        "password" => {
            let (stored, enc) = queries::read_secret(&state.db.pool, id)
                .await
                .map_err(err("could not read password"))?
                .ok_or_else(|| "no password stored for this item".to_string())?;
            secret::open(&stored, &enc).map_err(err("could not decode password"))
        }
        other => Err(format!("unknown credential field '{other}'")),
    }
}

async fn write_text_to_clipboard(
    guard: SelfCopyGuard,
    text: String,
    hash: String,
) -> CmdResult<()> {
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let mut cb = arboard::Clipboard::new().map_err(|e| format!("clipboard unavailable: {e}"))?;
        // Load-bearing. Without this the watcher's next poll sees the password
        // on the clipboard, does not recognise it as ours, and records it as an
        // ordinary clip -- where the FTS triggers index it as plain text.
        guard.expect(hash);
        cb.set_text(text).map_err(|e| format!("could not set text: {e}"))
    })
    .await
    .map_err(err("clipboard task failed"))?
}

/// Schedules the clipboard wipe for a password just copied.
///
/// Two conditions guard the clear, and both matter. The generation check means
/// a superseded timer does nothing. The hash check means we only ever clear
/// content we put there -- copy anything else in the meantime and it survives.
fn arm_password_expiry(state: &AppState, hash: String) {
    let generation = state.password_clipboard.arm(hash);
    let tracker = state.password_clipboard.clone();

    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(PASSWORD_CLIPBOARD_TTL).await;

        let Some(expected) = tracker.take_if_current(generation) else {
            return; // something newer was copied; not ours to clear
        };

        let cleared = tauri::async_runtime::spawn_blocking(move || -> Result<bool, String> {
            let mut cb = arboard::Clipboard::new().map_err(|e| format!("{e}"))?;
            match cb.get_text() {
                Ok(current) if hash_bytes(current.as_bytes()) == expected => {
                    // No self-copy guard needed here: an empty clipboard offers
                    // neither CF_UNICODETEXT nor CF_DIB, so the watcher's poll
                    // returns `Unsupported` and records nothing.
                    cb.clear().map_err(|e| format!("{e}"))?;
                    Ok(true)
                }
                // Either the user copied something else, or the clipboard holds
                // an image now. Leave it alone.
                _ => Ok(false),
            }
        })
        .await;

        match cleared {
            Ok(Ok(true)) => tracing::debug!("copied password cleared from the clipboard"),
            Ok(Ok(false)) => {}
            Ok(Err(e)) => tracing::warn!(error = %e, "could not clear the copied password"),
            Err(e) => tracing::warn!(error = %e, "clipboard clear task failed"),
        }
    });
}

/// Backs the image thumbnails in the list. Returned as raw PNG bytes so the
/// frontend can build a blob URL without a custom asset protocol.
#[tauri::command]
pub async fn read_blob(state: State<'_, AppState>, rel_path: String) -> CmdResult<Vec<u8>> {
    // Refuse anything that could climb out of the blob directory.
    if rel_path.contains("..") || rel_path.starts_with('/') || rel_path.contains('\\') {
        return Err("invalid blob path".to_string());
    }
    let full = state.db.blob_dir.join(&rel_path);
    tokio::fs::read(&full).await.map_err(err("could not read image"))
}

/* -- export / import -------------------------------------------------------- */

/// What the user picked in the export dialog.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    /// Full fidelity, re-importable. Passwords only if asked for.
    Json,
    /// Human-readable. Never carries passwords.
    Markdown,
    /// Full fidelity, passphrase-encrypted. Always carries passwords.
    Encrypted,
}

impl ExportFormat {
    fn extension(self) -> &'static str {
        match self {
            ExportFormat::Json => "json",
            ExportFormat::Markdown => "md",
            ExportFormat::Encrypted => "stashbk",
        }
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportReport {
    path: String,
    items: usize,
    bytes: usize,
    includes_secrets: bool,
}

/// Writes the whole database to a file the user chooses.
///
/// `include_secrets` is ignored for Markdown, which never carries passwords,
/// and forced on for an encrypted backup, where carrying them is the entire
/// point -- a passphrase-protected archive without the credentials would not
/// answer "what happens when I want to leave".
#[tauri::command]
pub async fn export_data(
    app: AppHandle,
    state: State<'_, AppState>,
    format: ExportFormat,
    include_secrets: bool,
    passphrase: Option<String>,
) -> CmdResult<ExportReport> {
    let include_secrets = match format {
        ExportFormat::Markdown => false,
        ExportFormat::Encrypted => true,
        ExportFormat::Json => include_secrets,
    };

    let archive = archive::io::build(&state.db, include_secrets)
        .await
        .map_err(err("could not read the database"))?;

    let bytes: Vec<u8> = match format {
        ExportFormat::Markdown => archive::to_markdown(&archive).into_bytes(),
        ExportFormat::Json => archive.to_json()?.into_bytes(),
        ExportFormat::Encrypted => {
            let pass = passphrase.unwrap_or_default();
            if pass.trim().is_empty() {
                return Err("an encrypted backup needs a passphrase".to_string());
            }
            let json = archive.to_json()?;
            backup::seal(json.as_bytes(), &pass).map_err(err("could not encrypt the backup"))?
        }
    };

    let default_name = format!("stash-export-{}.{}", today_stamp(), format.extension());
    let path = save_dialog(&app, &default_name, format)
        .ok_or_else(|| "export cancelled".to_string())?;

    tokio::fs::write(&path, &bytes)
        .await
        .map_err(err("could not write the file"))?;

    Ok(ExportReport {
        path: path.to_string_lossy().into_owned(),
        items: archive.items.len(),
        bytes: bytes.len(),
        includes_secrets: include_secrets,
    })
}

/// Reads an archive back in, merging rather than replacing.
///
/// `passphrase` is only consulted when the chosen file turns out to be an
/// encrypted backup, which is detected from its magic rather than its
/// extension -- a renamed file should still work.
#[tauri::command]
pub async fn import_data(
    app: AppHandle,
    state: State<'_, AppState>,
    passphrase: Option<String>,
) -> CmdResult<archive::io::ImportReport> {
    let path = open_dialog(&app).ok_or_else(|| "import cancelled".to_string())?;

    let raw = tokio::fs::read(&path).await.map_err(err("could not read the file"))?;

    let json = if backup::is_encrypted(&raw) {
        let pass = passphrase.unwrap_or_default();
        if pass.trim().is_empty() {
            return Err("this is an encrypted backup; enter its passphrase first".to_string());
        }
        let plain = backup::open(&raw, &pass).map_err(|e| e.to_string())?;
        String::from_utf8(plain).map_err(|_| "the backup is damaged".to_string())?
    } else {
        String::from_utf8(raw).map_err(|_| {
            "that file is not text; pick a .json export or a .stashbk backup".to_string()
        })?
    };

    let parsed = archive::Archive::from_json(&json)?;
    let report = archive::io::restore(&state.db, &parsed)
        .await
        .map_err(err("import failed"))?;

    // The list behind the dialog is now stale.
    let _ = app.emit("item-added", serde_json::Value::Null);
    Ok(report)
}

/// Both dialogs run on a blocking thread with hide-on-blur suppressed: a file
/// picker takes focus, and without the guard the panel would dismiss itself the
/// moment the dialog appeared.
fn save_dialog(app: &AppHandle, default_name: &str, format: ExportFormat) -> Option<PathBuf> {
    use tauri_plugin_dialog::DialogExt;

    let _guard = window::HideGuard::new();
    let (label, ext) = match format {
        ExportFormat::Json => ("Stash export", "json"),
        ExportFormat::Markdown => ("Markdown", "md"),
        ExportFormat::Encrypted => ("Stash encrypted backup", "stashbk"),
    };

    app.dialog()
        .file()
        .set_file_name(default_name)
        .add_filter(label, &[ext])
        .blocking_save_file()
        .and_then(|p| p.into_path().ok())
}

fn open_dialog(app: &AppHandle) -> Option<PathBuf> {
    use tauri_plugin_dialog::DialogExt;

    let _guard = window::HideGuard::new();
    app.dialog()
        .file()
        .add_filter("Stash export or backup", &["json", "stashbk"])
        .blocking_pick_file()
        .and_then(|p| p.into_path().ok())
}

/// `YYYY-MM-DD` for the default filename.
fn today_stamp() -> String {
    archive::date_stamp(now_millis())
}
