/** Mirrors the serde types in `src-tauri/src/db/model.rs` and `settings.rs`. */

export type ItemKind = "clip" | "note";

export type ContentType =
  | "url"
  | "hex_color"
  | "json"
  | "code"
  | "email"
  | "path"
  | "image"
  | "text"
  | "markdown"
  | "credential";

export interface Item {
  id: string;
  kind: ItemKind;
  contentType: ContentType;
  content: string | null;
  blobPath: string | null;
  hash: string | null;
  sourceApp: string | null;
  title: string | null;
  pinned: boolean;
  createdAt: number;
  updatedAt: number;
  folderId: string | null;
  /** Times copied or pasted *out* of Stash — not the same as `updatedAt`. */
  useCount: number;
  lastUsedAt: number | null;
}

/** True for items stored by the credential editor. */
export const isCredential = (item: Item): boolean => item.contentType === "credential";

export interface Folder {
  id: string;
  name: string;
  sortOrder: number;
  createdAt: number;
  updatedAt: number;
}

/**
 * Everything about a credential except the password. Getting the plaintext is a
 * separate, explicit call to `revealPassword`.
 */
export interface CredentialView {
  item: Item;
  username: string | null;
  url: string | null;
  notes: string | null;
  hasPassword: boolean;
}

/** What the editor sends back. `password: null` leaves the stored one alone. */
export interface CredentialDraft {
  id: string | null;
  title: string;
  username: string | null;
  password: string | null;
  url: string | null;
  notes: string | null;
  folderId: string | null;
}

export interface Filters {
  kinds: ItemKind[];
  contentTypes: ContentType[];
  sourceApps: string[];
  pinnedOnly: boolean;
  folderIds: string[];
  unfiledOnly: boolean;
  /** Set by the query parser in Rust (`folder:work`), never by the chips. */
  folderNames: string[];
  /** `sort:used`. */
  mostUsed: boolean;
}

/**
 * One entry in the "paste as" menu. Which entries exist depends on the item —
 * Rust decides by actually trying to parse the content, so the menu never
 * offers a decode that would produce mojibake.
 */
export interface TransformOption {
  id: string;
  label: string;
}

export interface Settings {
  captureExclusion: boolean;
  launchOnStartup: boolean;
  pollIntervalMs: number;
  autoPaste: boolean;
  rememberPosition: boolean;
  windowWidth: number | null;
  windowHeight: number | null;
  windowX: number | null;
  windowY: number | null;
}

export interface CaptureStatus {
  supported: boolean;
  applied: boolean;
  reason: string | null;
}

/**
 * The outcome of a paste attempt. `pasted: false` is not an error — the item is
 * on the clipboard either way, and `reason` is what tells the user to press
 * Ctrl+V themselves.
 */
export interface PasteStatus {
  supported: boolean;
  pasted: boolean;
  reason: string | null;
}

/** What this build can actually guarantee on this platform. */
export interface SecurityInfo {
  secretsEncrypted: boolean;
  pasteSupported: boolean;
  pasteReason: string | null;
}

export const emptyFilters = (): Filters => ({
  kinds: [],
  contentTypes: [],
  sourceApps: [],
  pinnedOnly: false,
  folderIds: [],
  unfiledOnly: false,
  folderNames: [],
  mostUsed: false,
});

/** The type chips in the UI; `link` is the user-facing name for `url`. */
export const TYPE_CHIPS = [
  { id: "cred", label: "Creds", types: ["credential"] },
  { id: "text", label: "Text", types: ["text", "json", "markdown"] },
  { id: "image", label: "Image", types: ["image"] },
  { id: "link", label: "Link", types: ["url", "email"] },
  { id: "code", label: "Code", types: ["code", "path", "hex_color"] },
] as const satisfies ReadonlyArray<{
  id: string;
  label: string;
  types: readonly ContentType[];
}>;

export type TypeChipId = (typeof TYPE_CHIPS)[number]["id"];

/* -- export / import -------------------------------------------------------- */

export type ExportFormat = "json" | "markdown" | "encrypted";

export interface ExportReport {
  path: string;
  items: number;
  bytes: number;
  includesSecrets: boolean;
}

/**
 * Import merges, so the interesting number is not "did it work" but how much
 * arrived and how much was already here.
 */
export interface ImportReport {
  imported: number;
  skipped: number;
  foldersCreated: number;
  secretsRestored: number;
  failed: number;
  /** The archive carried no passwords, so none could be restored. */
  secretsAbsent: boolean;
}
