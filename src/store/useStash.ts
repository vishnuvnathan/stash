import { create } from "zustand";
import * as ipc from "../lib/ipc";
import {
  emptyFilters,
  isCredential,
  TYPE_CHIPS,
  type ContentType,
  type CredentialDraft,
  type Filters,
  type Folder,
  type Item,
  type Settings,
  type TransformOption,
  type TypeChipId,
} from "../lib/types";

export type View = "list" | "note" | "credential" | "settings";

interface StashState {
  query: string;
  items: Item[];
  selected: number;
  loading: boolean;
  error: string | null;

  view: View;
  editingNoteId: string | null;
  editingCredentialId: string | null;

  typeChips: TypeChipId[];
  sourceApps: string[];
  activeSourceApps: string[];

  folders: Folder[];
  activeFolderId: string | null;

  /** Plaintext of the selected credential while it is revealed. Transient. */
  revealedPassword: string | null;

  settings: Settings | null;

  setQuery: (q: string) => void;
  refresh: () => Promise<void>;
  move: (delta: number) => void;
  select: (index: number) => void;

  toggleTypeChip: (id: TypeChipId) => void;
  toggleSourceApp: (app: string) => void;
  clearFilters: () => void;

  copySelected: () => Promise<void>;
  pasteSelected: () => Promise<void>;
  deleteSelected: () => Promise<void>;

  /** The "paste as" menu: null when closed, a list of options when open. */
  transforms: TransformOption[] | null;
  openTransforms: () => Promise<void>;
  closeTransforms: () => void;
  applyTransform: (transformId: string, paste: boolean) => Promise<void>;
  pinSelected: () => Promise<void>;

  loadFolders: () => Promise<void>;
  setFolder: (id: string | null) => void;
  createFolder: (name: string) => Promise<Folder | null>;
  renameFolder: (id: string, name: string) => Promise<void>;
  deleteFolder: (id: string) => Promise<void>;
  assignSelectedToFolder: (folderId: string | null) => Promise<void>;

  openCredential: (id: string | null) => void;
  saveCredential: (draft: Omit<CredentialDraft, "id">) => Promise<void>;
  copySelectedUsername: () => Promise<void>;
  copySelectedPassword: () => Promise<void>;
  revealSelectedPassword: () => Promise<void>;
  clearRevealed: () => void;

  openNote: (id: string | null) => void;
  saveNote: (title: string, content: string) => Promise<void>;
  setView: (v: View) => void;

  loadSettings: () => Promise<void>;
  updateSettings: (patch: Partial<Settings>) => Promise<void>;

  bootstrap: () => Promise<void>;
}

/** Search runs on a trailing debounce so holding a key does not queue a query per keystroke. */
let searchTimer: ReturnType<typeof setTimeout> | undefined;
const DEBOUNCE_MS = 80;

/** Guards against an older, slower search overwriting a newer one's results. */
let searchSeq = 0;

function buildFilters(
  chips: TypeChipId[],
  sourceApps: string[],
  folderId: string | null,
): Filters {
  const contentTypes: ContentType[] = [];
  for (const chip of TYPE_CHIPS) {
    if (chips.includes(chip.id)) contentTypes.push(...chip.types);
  }
  return {
    ...emptyFilters(),
    contentTypes,
    sourceApps,
    folderIds: folderId ? [folderId] : [],
  };
}

export const useStash = create<StashState>((set, get) => ({
  query: "",
  items: [],
  selected: 0,
  loading: false,
  error: null,

  view: "list",
  editingNoteId: null,
  editingCredentialId: null,

  typeChips: [],
  sourceApps: [],
  activeSourceApps: [],

  folders: [],
  activeFolderId: null,

  revealedPassword: null,
  transforms: null,

  settings: null,

  setQuery(q) {
    set({ query: q });
    if (searchTimer) clearTimeout(searchTimer);
    searchTimer = setTimeout(() => void get().refresh(), DEBOUNCE_MS);
  },

  async refresh() {
    const seq = ++searchSeq;
    const { query, typeChips, activeSourceApps, activeFolderId } = get();
    set({ loading: true });
    try {
      const items = await ipc.searchItems(
        query,
        buildFilters(typeChips, activeSourceApps, activeFolderId),
      );
      if (seq !== searchSeq) return; // a newer search already landed
      set((s) => ({
        items,
        loading: false,
        error: null,
        // Keep the cursor in range without snapping to the top on every keystroke.
        selected: Math.min(s.selected, Math.max(0, items.length - 1)),
      }));
    } catch (e) {
      if (seq !== searchSeq) return;
      set({ loading: false, error: String(e) });
    }
  },

  // Both movement paths drop a revealed password: it belongs to the row you
  // were looking at, and leaving that row is the clearest signal you are done
  // with it. Same reasoning in `setView` and on `panel-opened`.
  move(delta) {
    const { items, selected } = get();
    if (items.length === 0) return;
    const next = Math.min(items.length - 1, Math.max(0, selected + delta));
    if (next === selected) return;
    set({ selected: next, revealedPassword: null });
  },

  select(index) {
    if (get().selected === index) return;
    set({ selected: index, revealedPassword: null });
  },

  toggleTypeChip(id) {
    set((s) => ({
      typeChips: s.typeChips.includes(id)
        ? s.typeChips.filter((c) => c !== id)
        : [...s.typeChips, id],
      selected: 0,
      revealedPassword: null,
    }));
    void get().refresh();
  },

  toggleSourceApp(app) {
    set((s) => ({
      activeSourceApps: s.activeSourceApps.includes(app)
        ? s.activeSourceApps.filter((a) => a !== app)
        : [...s.activeSourceApps, app],
      selected: 0,
    }));
    void get().refresh();
  },

  clearFilters() {
    set({
      typeChips: [],
      activeSourceApps: [],
      activeFolderId: null,
      selected: 0,
      revealedPassword: null,
    });
    void get().refresh();
  },

  /**
   * Enter on a row. A credential copies its username rather than its body --
   * `copy_item` refuses credentials outright, so this dispatch is not merely a
   * nicety, it is the only thing that makes Enter work on one.
   */
  async copySelected() {
    const { items, selected } = get();
    const item = items[selected];
    if (!item) return;
    if (isCredential(item)) {
      await get().copySelectedUsername();
      return;
    }
    try {
      await ipc.copyItem(item.id);
      await ipc.hidePanel();
    } catch (e) {
      set({ error: String(e) });
    }
  },

  /**
   * What Enter does: paste into the window the panel was opened from.
   *
   * Falls back to a plain copy when auto-paste is off, and reports — rather
   * than swallows — a paste that could not be delivered. The item is on the
   * clipboard in every one of those cases, so the message says so instead of
   * reading as a failure.
   */
  async pasteSelected() {
    const { items, selected, settings } = get();
    const item = items[selected];
    if (!item) return;

    if (settings && !settings.autoPaste) {
      await get().copySelected();
      return;
    }

    try {
      const status = isCredential(item)
        ? await ipc.pasteCredentialField(item.id, "username")
        : await ipc.pasteItem(item.id);

      // The panel is already hidden by this point; the message is waiting when
      // it next opens.
      set({ error: status.pasted ? null : status.reason });
    } catch (e) {
      set({ error: String(e) });
    }
  },

  /**
   * Asks Rust which transforms suit the selected item. An item with nothing
   * worth offering — an image, a credential — sets an error rather than opening
   * an empty menu, which would look like the key did nothing.
   */
  async openTransforms() {
    const { items, selected } = get();
    const item = items[selected];
    if (!item) return;
    try {
      const transforms = await ipc.transformsFor(item.id);
      if (transforms.length === 0) {
        set({ error: "Nothing to transform on this item" });
        return;
      }
      set({ transforms, error: null });
    } catch (e) {
      set({ error: String(e) });
    }
  },

  closeTransforms() {
    if (get().transforms !== null) set({ transforms: null });
  },

  /**
   * `paste` follows whatever Enter would have done, so the transform menu does
   * not quietly change the action — only the text it acts on.
   */
  async applyTransform(transformId, paste) {
    const { items, selected, settings } = get();
    const item = items[selected];
    set({ transforms: null });
    if (!item) return;

    const wantPaste = paste && (settings?.autoPaste ?? true);
    try {
      if (wantPaste) {
        const status = await ipc.pasteItem(item.id, transformId);
        set({ error: status.pasted ? null : status.reason });
      } else {
        await ipc.copyItem(item.id, transformId);
        await ipc.hidePanel();
      }
    } catch (e) {
      set({ error: String(e) });
    }
  },

  async deleteSelected() {
    const { items, selected } = get();
    const item = items[selected];
    if (!item) return;
    // Drop it locally first: the row is gone from the user's point of view the
    // moment they press the key, and the query would return the same result.
    set({
      items: items.filter((i) => i.id !== item.id),
      selected: Math.min(selected, Math.max(0, items.length - 2)),
    });
    try {
      await ipc.deleteItem(item.id);
    } catch (e) {
      set({ error: String(e) });
      void get().refresh();
    }
  },

  async pinSelected() {
    const { items, selected } = get();
    const item = items[selected];
    if (!item) return;
    try {
      await ipc.togglePin(item.id, !item.pinned);
      await get().refresh();
    } catch (e) {
      set({ error: String(e) });
    }
  },

  /* -- folders ------------------------------------------------------------ */

  async loadFolders() {
    try {
      set({ folders: await ipc.listFolders() });
    } catch (e) {
      set({ error: String(e) });
    }
  },

  setFolder(id) {
    set({ activeFolderId: id, selected: 0, revealedPassword: null });
    void get().refresh();
  },

  async createFolder(name) {
    try {
      const folder = await ipc.createFolder(name);
      await get().loadFolders();
      return folder;
    } catch (e) {
      set({ error: String(e) });
      return null;
    }
  },

  async renameFolder(id, name) {
    try {
      await ipc.renameFolder(id, name);
      await get().loadFolders();
    } catch (e) {
      set({ error: String(e) });
    }
  },

  async deleteFolder(id) {
    try {
      await ipc.deleteFolder(id);
      // Items survive as unfiled, but the active filter now points at nothing.
      if (get().activeFolderId === id) set({ activeFolderId: null });
      await get().loadFolders();
      await get().refresh();
    } catch (e) {
      set({ error: String(e) });
    }
  },

  async assignSelectedToFolder(folderId) {
    const { items, selected } = get();
    const item = items[selected];
    if (!item) return;
    try {
      await ipc.setItemFolder(item.id, folderId);
      await get().refresh();
    } catch (e) {
      set({ error: String(e) });
    }
  },

  /* -- credentials -------------------------------------------------------- */

  openCredential(id) {
    set({ view: "credential", editingCredentialId: id, revealedPassword: null });
  },

  async saveCredential(draft) {
    const { editingCredentialId } = get();
    try {
      await ipc.saveCredential({ ...draft, id: editingCredentialId });
      set({ view: "list", editingCredentialId: null });
      await Promise.all([get().refresh(), get().loadFolders()]);
    } catch (e) {
      set({ error: String(e) });
    }
  },

  async copySelectedUsername() {
    const { items, selected } = get();
    const item = items[selected];
    if (!item) return;
    try {
      await ipc.copyCredentialField(item.id, "username");
      await ipc.hidePanel();
    } catch (e) {
      set({ error: String(e) });
    }
  },

  async copySelectedPassword() {
    const { items, selected } = get();
    const item = items[selected];
    if (!item || !isCredential(item)) return;
    try {
      // The plaintext goes straight from Rust to the clipboard; nothing here
      // ever holds it.
      await ipc.copyCredentialField(item.id, "password");
      await ipc.hidePanel();
    } catch (e) {
      set({ error: String(e) });
    }
  },

  async revealSelectedPassword() {
    const { items, selected, revealedPassword } = get();
    const item = items[selected];
    if (!item || !isCredential(item)) return;
    if (revealedPassword !== null) {
      set({ revealedPassword: null });
      return;
    }
    try {
      set({ revealedPassword: await ipc.revealPassword(item.id) });
    } catch (e) {
      set({ error: String(e) });
    }
  },

  clearRevealed() {
    if (get().revealedPassword !== null) set({ revealedPassword: null });
  },

  openNote(id) {
    set({ view: "note", editingNoteId: id, revealedPassword: null });
  },

  async saveNote(title, content) {
    const { editingNoteId } = get();
    try {
      await ipc.saveNote(editingNoteId, title, content);
      set({ view: "list", editingNoteId: null });
      await get().refresh();
    } catch (e) {
      set({ error: String(e) });
    }
  },

  setView(v) {
    set({ view: v, revealedPassword: null });
  },

  async loadSettings() {
    try {
      set({ settings: await ipc.getSettings() });
    } catch (e) {
      set({ error: String(e) });
    }
  },

  async updateSettings(patch) {
    const current = get().settings;
    if (!current) return;
    const next = { ...current, ...patch };
    set({ settings: next });
    try {
      set({ settings: await ipc.setSettings(next) });
    } catch (e) {
      set({ settings: current, error: String(e) });
    }
  },

  async bootstrap() {
    await Promise.all([
      get().refresh(),
      get().loadSettings(),
      get().loadFolders(),
      ipc
        .listSourceApps()
        .then((sourceApps) => set({ sourceApps }))
        .catch(() => undefined),
    ]);
  },
}));
