# Stash v0.1 — Implementation Plan

Vertical slice: tray-resident clipboard manager + notes. Local only, no network.
This document is for review **before** any code is written.

---

## 0. Blocker & prerequisites

**Rust is not installed on this machine.** `cargo`, `rustc`, and `rustup` are all
absent (checked `PATH`, `~/.cargo/bin`, `C:\Program Files\Rust*`). Node 22.15.0 /
npm 10.9.2 are present.

Before module 1 you need:

| Requirement | Why | Install |
|---|---|---|
| Rust stable (MSVC) | everything Rust | `winget install Rustlang.Rustup`, then `rustup default stable-msvc` |
| MSVC Build Tools + Windows SDK | Tauri links against them | VS Build Tools, "Desktop development with C++" |
| WebView2 runtime | Tauri v2 webview | Preinstalled on Windows 11 |

Until Rust is installed I cannot run `cargo check`, so the per-module gate
degrades to `tsc --noEmit` only. I will say so explicitly each time rather than
claiming a check passed.

---

## 1. Repository layout

```
Clipmax/
├─ PLAN.md
├─ package.json                  vite + react + ts
├─ vite.config.ts
├─ tailwind.config.js
├─ tsconfig.json
├─ index.html
├─ src/                          frontend
│  ├─ main.tsx
│  ├─ App.tsx
│  ├─ lib/
│  │  ├─ ipc.ts                  <- the ONLY file that calls invoke()
│  │  ├─ types.ts                mirrors Rust serde types
│  │  └─ hotkeys.ts
│  ├─ store/
│  │  └─ useStash.ts             zustand
│  ├─ components/
│  │  ├─ SearchBar.tsx
│  │  ├─ ResultList.tsx          @tanstack/react-virtual
│  │  ├─ ResultRow.tsx
│  │  ├─ FilterChips.tsx
│  │  ├─ NoteEditor.tsx
│  │  └─ Settings.tsx
│  └─ styles.css
└─ src-tauri/
   ├─ Cargo.toml
   ├─ tauri.conf.json
   ├─ build.rs
   ├─ capabilities/default.json
   ├─ migrations/
   │  ├─ 0001_init.sql
   │  └─ 0002_fts.sql
   └─ src/
      ├─ main.rs                 thin: calls lib::run()
      ├─ lib.rs                  builder, plugins, state wiring
      ├─ db/
      │  ├─ mod.rs               pool handle, migration registration
      │  ├─ model.rs             Item, ItemKind, ContentType, Filters
      │  ├─ queries.rs           insert/search/delete/prune
      │  └─ retention.rs         prune timer task
      ├─ clipboard/
      │  ├─ mod.rs               watcher loop, dedupe, dispatch
      │  ├─ types.rs             Snapshot { Text | Image | Concealed | Unchanged }
      │  ├─ win.rs               #[cfg(windows)]
      │  ├─ mac.rs               #[cfg(target_os = "macos")]
      │  └─ linux.rs             #[cfg(target_os = "linux")]
      ├─ capture/
      │  ├─ mod.rs               set_capture_exclusion(enabled)
      │  └─ win.rs  mac.rs  linux.rs
      ├─ detect.rs               content-type classifier
      ├─ window.rs               toggle/show/hide/center, blur + close handling
      ├─ tray.rs                 tray icon + menu
      ├─ settings.rs             JSON settings file in app config dir
      └─ commands.rs             all #[tauri::command] fns, one place
```

---

## 2. Decisions I need you to confirm

**D1 — Rust-side DB access.** `tauri-plugin-sql` is a frontend-facing plugin, but
the clipboard watcher lives in Rust and must write to the same database. Plan:
keep `tauri-plugin-sql` registered (it owns the sqlx `SqlitePool` and runs the
numbered migrations), and reach it from Rust via the plugin's `DbInstances`
state, so there is one pool and one migration path. The frontend then never uses
the plugin's JS API — every read and write goes through a typed
`#[tauri::command]`, which is what the "single ipc.ts wrapper" constraint wants
anyway.

*Risk:* `DbInstances` internals are plugin-version-sensitive. Fallback if it
fights us: own a `SqlitePool` directly with `sqlx`, run the same numbered `.sql`
files through `sqlx::migrate!`, and drop `tauri-plugin-sql`. Same schema, same
files, roughly 30 lines different. **Tell me which you prefer, or let me start
with the plugin and fall back if it resists.**

**D2 — Linux concealed-clipboard detection.** The spec names only the macOS and
Windows markers. Linux has a de-facto convention (the
`x-kde-passwordManagerHint` target, honoured by KeePassXC, Bitwarden, KWallet),
but `arboard` cannot inspect MIME targets, so detecting it means raw `x11rb`
TARGETS inspection. Options: (a) v0.1 ships Linux without concealed detection,
documented as a known gap; (b) I add the `x11rb` TARGETS check now (~80 lines,
X11 only, still nothing for Wayland). Default if you don't answer: **(a)**,
documented in the README and logged once at startup.

**D3 — Linux clipboard polling.** No sequence-number equivalent exists. Plan:
`arboard` read + hash compare every 400ms, which costs one X round-trip per tick.
XFIXES `SelectionNotify` would make it event-driven and truly zero-idle, but it
is X11-only and a chunk of work. Default: **polling now, XFIXES noted for v0.2.**

**D4 — Markdown editor.** Plain `<textarea>` plus a `react-markdown` preview
toggle, not CodeMirror. Keeps the bundle small and the panel fast. Say so if you
want real editor affordances instead.

---

## 3. Schema (module 1)

`src-tauri/migrations/0001_init.sql`

```sql
CREATE TABLE items (
  id           TEXT PRIMARY KEY,           -- uuid v4
  kind         TEXT NOT NULL CHECK (kind IN ('clip','note')),
  content_type TEXT NOT NULL,              -- url|hex_color|json|code|email|path|image|text|markdown
  content      TEXT,                       -- NULL for image clips
  blob_path    TEXT,                       -- relative to app data dir; NULL unless image
  hash         TEXT,                       -- sha256 hex of text bytes or image bytes
  source_app   TEXT,
  title        TEXT,
  pinned       INTEGER NOT NULL DEFAULT 0,
  created_at   INTEGER NOT NULL,           -- unix millis
  updated_at   INTEGER NOT NULL,
  deleted_at   INTEGER
);

CREATE INDEX idx_items_recent ON items(deleted_at, pinned, updated_at DESC);
CREATE INDEX idx_items_hash   ON items(hash) WHERE deleted_at IS NULL;
CREATE INDEX idx_items_type   ON items(content_type);
CREATE INDEX idx_items_source ON items(source_app);

CREATE TABLE tags (
  id   TEXT PRIMARY KEY,
  name TEXT NOT NULL UNIQUE COLLATE NOCASE
);

CREATE TABLE item_tags (
  item_id TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
  tag_id  TEXT NOT NULL REFERENCES tags(id)  ON DELETE CASCADE,
  PRIMARY KEY (item_id, tag_id)
);
```

`0002_fts.sql` — FTS5 external-content table over `items`, plus the three sync
triggers:

```sql
CREATE VIRTUAL TABLE items_fts USING fts5(
  title, content, content='items', content_rowid='rowid', tokenize='unicode61'
);

CREATE TRIGGER items_ai AFTER INSERT ON items BEGIN
  INSERT INTO items_fts(rowid, title, content)
  VALUES (new.rowid, new.title, new.content);
END;

CREATE TRIGGER items_ad AFTER DELETE ON items BEGIN
  INSERT INTO items_fts(items_fts, rowid, title, content)
  VALUES ('delete', old.rowid, old.title, old.content);
END;

CREATE TRIGGER items_au AFTER UPDATE ON items BEGIN
  INSERT INTO items_fts(items_fts, rowid, title, content)
  VALUES ('delete', old.rowid, old.title, old.content);
  INSERT INTO items_fts(rowid, title, content)
  VALUES (new.rowid, new.title, new.content);
END;
```

Notes: FTS5 is compiled in by `libsqlite3-sys`'s bundled build
(`SQLITE_ENABLE_FTS5`), so there is no system-SQLite dependency. Soft delete sets
`deleted_at`; the retention task does the hard `DELETE`, which fires `items_ad`
and unlinks the blob. Pragmas set on connect: `journal_mode=WAL`,
`synchronous=NORMAL`, `foreign_keys=ON`.

---

## 4. Clipboard watcher (module 2)

One `tokio::spawn` loop on `interval(400ms)`. Each tick calls
`platform::poll(&mut last_change_counter) -> Snapshot`, which returns
`Unchanged` without touching the clipboard when the counter has not moved. That
is the idle-CPU guarantee: on an idle tick the only work is one syscall and an
integer compare — no allocation, no DB access, no event emitted, so no React
re-render.

### Windows (`clipboard/win.rs`) — `windows` crate

| Purpose | API |
|---|---|
| change counter | `Win32::System::DataExchange::GetClipboardSequenceNumber` |
| open/close | `OpenClipboard(HWND(0))` / `CloseClipboard` (retry 5x on `ERROR_ACCESS_DENIED`) |
| format probe | `IsClipboardFormatAvailable(CF_UNICODETEXT / CF_DIBV5 / CF_DIB)` |
| concealed flag | `RegisterClipboardFormatW("ExcludeClipboardContentFromMonitorProcessing")` then `IsClipboardFormatAvailable(id)` |
| read | `GetClipboardData` + `Globals::GlobalLock` / `GlobalSize` / `GlobalUnlock` |
| foreground app | `GetForegroundWindow` -> `GetWindowThreadProcessId` -> `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` -> `QueryFullProcessImageNameW` -> file stem |

The concealed check happens **first**, while the clipboard is open and before any
content is read; on a hit we bump the counter and return `Concealed` without ever
materialising the bytes. `CF_DIB`/`CF_DIBV5` are converted to PNG with the
`image` crate before hashing, so the hash is stable across paste sources.

### macOS (`clipboard/mac.rs`) — `objc2` + `objc2-app-kit` / `objc2-foundation`

| Purpose | API |
|---|---|
| change counter | `NSPasteboard::generalPasteboard().changeCount()` |
| concealed flag | `pasteboard.types()` contains `"org.nspasteboard.ConcealedType"` (also skip on `"org.nspasteboard.TransientType"` and `"com.agilebits.onepassword"`) |
| text | `stringForType(NSPasteboardTypeString)` |
| image | `dataForType(NSPasteboardTypePNG)`, else `NSPasteboardTypeTIFF` re-encoded to PNG |
| foreground app | `NSWorkspace::sharedWorkspace().frontmostApplication()?.localizedName()` |

Structured but untested, per scope.

### Linux (`clipboard/linux.rs`)

`arboard` `get_text` / `get_image`, hash-compared (see D3). Foreground app via
X11 `_NET_ACTIVE_WINDOW` -> `_NET_WM_PID` -> `/proc/<pid>/comm`; `None` on
Wayland.

### Shared pipeline (`clipboard/mod.rs`)

```
Snapshot::Unchanged  -> return immediately
Snapshot::Concealed  -> log at trace, return
Snapshot::Text(s)    -> hash = sha256(s)
Snapshot::Image(png) -> reject if len > 10 MiB; hash = sha256(png)
```

Then: if `hash == last_hash_in_memory`, return without touching the DB.
Otherwise look up `SELECT id FROM items WHERE hash = ?1 AND deleted_at IS NULL`.
On a hit, `UPDATE items SET updated_at = ?` and emit a lightweight `item-bumped`
event. On a miss, images are written to
`<app_data>/blobs/<hash[0..2]>/<hash>.png` (skipped if the file already exists),
`detect::classify()` runs, the row is inserted, and `item-added` is emitted.

---

## 5. Window + tray (module 3)

`tauri.conf.json` window: `width 720`, `height 480`, `decorations false`,
`transparent false`, `resizable false`, `center true`, `visible false`,
`skipTaskbar true`, `alwaysOnTop true`, `focus true`.

- Global shortcut: `tauri-plugin-global-shortcut`, `CmdOrCtrl+Shift+V` ->
  `window::toggle()` (visible: hide; hidden: re-center on the cursor's monitor,
  show, focus, emit `panel-opened` so the search box selects itself).
- `WindowEvent::Focused(false)` -> hide, unless the settings panel has a native
  dialog open.
- `WindowEvent::CloseRequested` -> `api.prevent_close()` then hide.
- Escape is handled frontend-side and calls `hidePanel()` through ipc.
- Tray: `TrayIconBuilder` with menu `Show / Settings / --- / Quit`. Only the Quit
  item calls `app.exit(0)`.
- macOS: `app.set_activation_policy(ActivationPolicy::Accessory)` so there is no
  Dock entry; Windows and Linux get no taskbar entry from `skipTaskbar`.

---

## 6. Capture exclusion (module 4)

One command, three implementations:

```rust
#[tauri::command]
fn set_capture_exclusion(
    window: tauri::WebviewWindow,
    enabled: bool,
) -> Result<CaptureStatus, String>
```

returning `CaptureStatus { supported: bool, applied: bool, reason: Option<String> }`
so the settings UI can honestly show "not supported on this platform".

- **Windows:** `SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)` or
  `WDA_NONE`. `WDA_EXCLUDEFROMCAPTURE` needs build >= 19041; on older builds the
  call fails with `ERROR_INVALID_PARAMETER`, so we retry with `WDA_MONITOR`
  (blanks in captures rather than vanishing) and report `applied: true,
  reason: "degraded to WDA_MONITOR"`. The HWND comes from `window.hwnd()`.
- **macOS:** `NSWindow::setSharingType(NSWindowSharingType::None | ::ReadOnly)`
  via `objc2-app-kit`, on the `NSWindow` behind `window.ns_window()`, dispatched
  to the main thread.
- **Linux:** a `std::sync::Once` guarding one `tracing::warn!` that capture
  exclusion is unsupported and hide-on-blur is the mitigation; returns
  `supported: false`.

Applied on window creation when the setting is on (default on), and re-applied
whenever the setting is toggled.

---

## 7. Type detection (`detect.rs`)

Runs once on insert, first match wins:

1. `image` — set by the watcher, never sniffed
2. `url` — `^(https?|ftp|file)://` or `^www\.`, parses as a URL, single line
3. `email` — single line, conservative addr-spec regex
4. `hex_color` — `^#?([0-9a-fA-F]{3}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})$`
5. `path` — Windows `^[A-Za-z]:[\\/]` or UNC `^\\\\`, or POSIX `^[~/]` with at
   least one separator; single line
6. `json` — trimmed input starts with `{` or `[` **and**
   `serde_json::from_str::<Value>` succeeds
7. `code` — heuristic score >= 3: fenced block, two or more lines with leading
   indent, lines ending in `;` `{` `}`, keyword hits
   (`fn|def|class|import|const|let|function|public|#include`), `=>` `->` `::`
8. `text` — fallback

Notes get `markdown`. Detection is pure and unit-tested; regexes are `once_cell`
`Lazy` statics so nothing recompiles per insert.

---

## 8. Retention (`db/retention.rs`)

`tokio::spawn` plus `interval(5 min)`, with one extra run 30s after startup:

```sql
UPDATE items SET deleted_at = ?now
WHERE kind = 'clip' AND pinned = 0 AND deleted_at IS NULL
  AND id NOT IN (
    SELECT id FROM items
    WHERE kind = 'clip' AND pinned = 0 AND deleted_at IS NULL
    ORDER BY updated_at DESC LIMIT 1000
  );
```

Then hard-delete rows soft-deleted more than 24h ago, and unlink any `blob_path`
no longer referenced by a live row. Notes and pinned items are never pruned. If
the first statement affects zero rows the task does nothing else, so an idle
system performs no writes.

---

## 9. IPC surface (`commands.rs` <-> `src/lib/ipc.ts`)

Every command is typed on both sides; `ipc.ts` is the only importer of
`@tauri-apps/api/core`.

| Command | Signature |
|---|---|
| `search_items` | `(query, filters: { kinds, content_types, source_apps, pinned_only }, limit, offset) -> Item[]` |
| `get_item` | `(id) -> Item` |
| `copy_item` | `(id) -> ()` — writes back to the clipboard and suppresses the next watcher tick for that hash |
| `delete_item` | `(id) -> ()` — soft delete |
| `toggle_pin` | `(id, pinned) -> ()` |
| `save_note` | `(id: Option<String>, title, content) -> Item` |
| `list_source_apps` | `() -> string[]` — for the filter chips |
| `get_settings` / `set_settings` | `() -> Settings` / `(Settings) -> Settings` |
| `set_capture_exclusion` | `(enabled) -> CaptureStatus` |
| `hide_panel` | `() -> ()` |

Events emitted to the frontend: `item-added`, `item-bumped`, `panel-opened`. The
store subscribes once in `App.tsx`; JS never polls.

---

## 10. UI (module 5) and notes (module 6)

- **Panel:** search input (auto-focused on `panel-opened`), filter chip row,
  virtualized list via `@tanstack/react-virtual` (fixed 56px rows, overscan 6).
- **Keyboard:** up/down move the selection and scroll it into view, `Enter` runs
  `copy_item` then `hide_panel`, `Ctrl+Backspace` runs `delete_item`, `Escape`
  runs `hide_panel`, `Ctrl+N` opens the note editor, `Ctrl+,` opens settings. One
  `useHotkeys` hook on the panel root; no per-row listeners.
- **Chips:** type (`text` / `image` / `link` / `code`, where `link` maps to
  `url`) and source app, sourced from `list_source_apps`. Multi-select, AND
  across categories and OR within one.
- **Notes:** `kind='note'`, `content_type='markdown'`, stored in the same table
  so they fall out of the same `search_items` query with no special-casing.
  `Ctrl+N` swaps the panel body for the editor; `Ctrl+S` or blur saves; `Escape`
  returns to the list without hiding the panel.
- **Search:** an empty query gives a recent-first list; a non-empty query uses
  FTS5 `MATCH` with prefix tokenisation, ranked by `bm25()`, pinned items first.
  Debounced 80ms.

---

## 11. Order of work and gates

| # | Module | Gate |
|---|---|---|
| 0 | Scaffold (vite / tauri / tailwind / zustand, configs) | `tsc --noEmit`, `cargo check` |
| 1 | Schema + migrations + `db/` | `cargo check`, migration smoke test |
| 2 | Clipboard watcher (+ `detect.rs`) | `cargo check`, unit tests on `detect` |
| 3 | Window + tray + global shortcut | `cargo check`, manual toggle test |
| 4 | Capture exclusion | `cargo check` |
| 5 | UI (ipc.ts, store, list, chips) | `tsc --noEmit` |
| 6 | Notes + retention | both |

Windows is the only platform I can build and run here. The Linux and macOS paths
will be written and `cfg`-gated but compile-checked only if you want me to add
`cargo check --target` runs — cross-checking macOS from Windows needs the Apple
SDK, so realistically macOS ships written-but-unverified, as scoped.

---

## 12. Explicitly out of scope for v0.1

Sync, encryption, pinned-order history, tag UI (the tables exist, no UI),
rich-text and HTML clipboard formats, file-drop clipboard entries, multi-window,
auto-update, and any network client. No crate that opens a socket is added.
