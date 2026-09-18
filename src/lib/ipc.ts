/**
 * The only module in the app that talks to Rust. Components import these
 * functions; nothing else imports `@tauri-apps/api`. If you find yourself
 * reaching for `invoke` in a component, add a wrapper here instead.
 */

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  CaptureStatus,
  CredentialDraft,
  CredentialView,
  Filters,
  Folder,
  ExportFormat,
  ExportReport,
  ImportReport,
  Item,
  PasteStatus,
  SecurityInfo,
  Settings,
  TransformOption,
} from "./types";

export async function searchItems(
  query: string,
  filters: Filters,
  limit = 200,
  offset = 0,
): Promise<Item[]> {
  return invoke<Item[]>("search_items", { query, filters, limit, offset });
}

export async function getItem(id: string): Promise<Item | null> {
  return invoke<Item | null>("get_item", { id });
}

/**
 * `transform` changes only what lands on the clipboard. The stored item keeps
 * what was originally copied, so transforming is never destructive.
 */
export async function copyItem(id: string, transform?: string): Promise<void> {
  return invoke<void>("copy_item", { id, transform: transform ?? null });
}

/** The "paste as" entries that make sense for this item. Empty for images and credentials. */
export async function transformsFor(id: string): Promise<TransformOption[]> {
  return invoke<TransformOption[]>("transforms_for", { id });
}

/**
 * Copies the item, hides the panel, and pastes it into whatever window was
 * focused before the panel opened. The whole sequence runs in Rust: the panel
 * must be down before focus can go back, and the wait for that focus change is
 * not something three separate IPC calls can order reliably.
 *
 * Resolves with `pasted: false` rather than rejecting when the keystroke could
 * not be sent — the copy half still happened.
 */
export async function pasteItem(id: string, transform?: string): Promise<PasteStatus> {
  return invoke<PasteStatus>("paste_item", { id, transform: transform ?? null });
}

export async function deleteItem(id: string): Promise<void> {
  return invoke<void>("delete_item", { id });
}

export async function togglePin(id: string, pinned: boolean): Promise<void> {
  return invoke<void>("toggle_pin", { id, pinned });
}

export async function saveNote(
  id: string | null,
  title: string,
  content: string,
): Promise<Item> {
  return invoke<Item>("save_note", { id, title, content });
}

export async function listSourceApps(): Promise<string[]> {
  return invoke<string[]>("list_source_apps");
}

export async function getSettings(): Promise<Settings> {
  return invoke<Settings>("get_settings");
}

export async function setSettings(settings: Settings): Promise<Settings> {
  return invoke<Settings>("set_settings", { settings });
}

export async function setCaptureExclusion(enabled: boolean): Promise<CaptureStatus> {
  return invoke<CaptureStatus>("set_capture_exclusion", { enabled });
}

/**
 * Whether secrets are really encrypted and pasting really works here. Both
 * answers are platform-dependent and invisible from this side, so the settings
 * panel asks rather than asserting.
 */
export async function securityInfo(): Promise<SecurityInfo> {
  return invoke<SecurityInfo>("security_info");
}

export async function hidePanel(): Promise<void> {
  return invoke<void>("hide_panel");
}

/**
 * Image bytes for a thumbnail. Returned as a byte array and wrapped in an
 * object URL by the caller, which avoids granting the webview filesystem
 * access just to show a picture.
 */
export async function readBlob(relPath: string): Promise<Blob> {
  const bytes = await invoke<number[]>("read_blob", { relPath });
  return new Blob([new Uint8Array(bytes)], { type: "image/png" });
}

/* -- folders --------------------------------------------------------------- */

export async function listFolders(): Promise<Folder[]> {
  return invoke<Folder[]>("list_folders");
}

/** Create-or-get: an existing name returns that folder rather than erroring. */
export async function createFolder(name: string): Promise<Folder> {
  return invoke<Folder>("create_folder", { name });
}

export async function renameFolder(id: string, name: string): Promise<void> {
  return invoke<void>("rename_folder", { id, name });
}

/** Items in the folder are unfiled, not deleted. */
export async function deleteFolder(id: string): Promise<void> {
  return invoke<void>("delete_folder", { id });
}

export async function setItemFolder(id: string, folderId: string | null): Promise<void> {
  return invoke<void>("set_item_folder", { id, folderId });
}

/* -- credentials ----------------------------------------------------------- */

/**
 * Fields are listed out rather than spread so the wire shape is visible here --
 * and note `password`, where `null` means "keep the stored one" and `""` means
 * "delete it". That distinction is the whole reason the editor never has to
 * read a plaintext password back just to re-save a label.
 */
export async function saveCredential(draft: CredentialDraft): Promise<Item> {
  return invoke<Item>("save_credential", {
    id: draft.id,
    title: draft.title,
    username: draft.username,
    password: draft.password,
    url: draft.url,
    notes: draft.notes,
    folderId: draft.folderId,
  });
}

export async function getCredential(id: string): Promise<CredentialView> {
  return invoke<CredentialView>("get_credential", { id });
}

/**
 * The only call that brings a plaintext password into the webview. Everything
 * else -- including copying it -- happens inside Rust.
 */
export async function revealPassword(id: string): Promise<string> {
  return invoke<string>("reveal_password", { id });
}

/**
 * Copies a field straight to the clipboard from Rust. For the password this
 * means the value never enters the webview at all.
 */
export async function copyCredentialField(
  id: string,
  field: "username" | "password",
): Promise<void> {
  return invoke<void>("copy_credential_field", { id, field });
}

/**
 * Username only, and deliberately so: the field that had focus before the panel
 * opened is not reliably the password field, and a password typed into the
 * wrong one is not a mistake you can take back. Passwords are copied, then
 * cleared from the clipboard 30 seconds later.
 */
export async function pasteCredentialField(
  id: string,
  field: "username",
): Promise<PasteStatus> {
  return invoke<PasteStatus>("paste_credential_field", { id, field });
}

/* -- window ---------------------------------------------------------------- */

/**
 * Moving and resizing both hand the mouse to the OS drag loop, and the webview
 * loses focus on the way in. `Focused(false)` is what dismisses the panel when
 * you click away, so the panel used to vanish the moment you grabbed an edge.
 * `begin_window_drag` arms the Rust-side guard against exactly that, and has to
 * land before the drag starts -- hence the await.
 */
async function beginDrag(): Promise<void> {
  await invoke<void>("begin_window_drag");
}

/** Drag-to-move for the frameless window. Needs `core:window:allow-start-dragging`. */
export async function startMove(): Promise<void> {
  await beginDrag();
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  await getCurrentWindow().startDragging();
}

/**
 * Manual resize handles for the frameless window. Tauri draws no resize border
 * on an undecorated window on every platform, so the panel supplies its own
 * grips and calls this. Needs `core:window:allow-start-resize-dragging`.
 */
export async function startResize(direction: ResizeDirection): Promise<void> {
  await beginDrag();
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  await getCurrentWindow().startResizeDragging(direction);
}

export type ResizeDirection =
  | "East"
  | "North"
  | "NorthEast"
  | "NorthWest"
  | "South"
  | "SouthEast"
  | "SouthWest"
  | "West";

/* -- events emitted by Rust ------------------------------------------------ */

export function onItemAdded(fn: (item: Item) => void): Promise<UnlistenFn> {
  return listen<Item>("item-added", (e) => fn(e.payload));
}

export function onItemBumped(fn: (id: string) => void): Promise<UnlistenFn> {
  return listen<string>("item-bumped", (e) => fn(e.payload));
}

export function onPanelOpened(fn: () => void): Promise<UnlistenFn> {
  return listen("panel-opened", () => fn());
}

export function onOpenSettings(fn: () => void): Promise<UnlistenFn> {
  return listen("open-settings", () => fn());
}

/* -- export / import -------------------------------------------------------- */

/**
 * Opens a save dialog and writes the whole database to it.
 *
 * Rejects with "export cancelled" when the dialog is dismissed, which the
 * caller treats as a non-event rather than an error.
 */
export async function exportData(
  format: ExportFormat,
  includeSecrets: boolean,
  passphrase?: string,
): Promise<ExportReport> {
  return invoke<ExportReport>("export_data", {
    format,
    includeSecrets,
    passphrase: passphrase ?? null,
  });
}

/**
 * Opens a file picker and merges the chosen archive in. `passphrase` is only
 * used if the file turns out to be an encrypted backup, which is detected from
 * its contents rather than its name.
 */
export async function importData(passphrase?: string): Promise<ImportReport> {
  return invoke<ImportReport>("import_data", { passphrase: passphrase ?? null });
}
