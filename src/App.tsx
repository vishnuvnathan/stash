import { useCallback, useEffect, useMemo } from "react";
import { CredentialEditor } from "./components/CredentialEditor";
import { FilterChips } from "./components/FilterChips";
import { NoteEditor } from "./components/NoteEditor";
import { PreviewPane } from "./components/PreviewPane";
import { ResultList } from "./components/ResultList";
import { SearchBar } from "./components/SearchBar";
import { Settings } from "./components/Settings";
import { TransformMenu } from "./components/TransformMenu";
import { useHotkeys } from "./lib/hotkeys";
import {
  hidePanel,
  onItemAdded,
  onItemBumped,
  onOpenSettings,
  onPanelOpened,
  startMove,
  startResize,
  type ResizeDirection,
} from "./lib/ipc";
import { isCredential } from "./lib/types";
import { useStash } from "./store/useStash";

export default function App() {
  const view = useStash((s) => s.view);
  const error = useStash((s) => s.error);

  useBackendEvents();
  usePanelHotkeys();

  return (
    <div className="relative flex h-screen w-screen flex-col overflow-hidden rounded-xl border border-edge bg-panel font-ui text-zinc-200 antialiased">
      {/*
        The window is frameless, so this strip is the only title bar there is.
        It deliberately does not use `data-tauri-drag-region`: that attribute is
        handled inside Tauri, which would start the drag without arming the
        guard in window.rs -- and the panel would dismiss itself mid-move. The
        grip bar is a visual affordance; it must not swallow the mousedown,
        hence pointer-events-none.
      */}
      <div
        onMouseDown={(e) => {
          if (e.button !== 0) return;
          e.preventDefault();
          void startMove();
        }}
        className="group flex h-6 shrink-0 cursor-grab items-center justify-center bg-panelalt active:cursor-grabbing"
      >
        <div className="pointer-events-none h-[3px] w-11 rounded-full bg-edge transition-colors group-hover:bg-zinc-600" />
      </div>

      {view === "list" && (
        <>
          <SearchBar />
          <FilterChips />
          {/* min-h-0 is load-bearing: without it the overflow-y-auto children
              refuse to scroll inside this flex-col parent. */}
          <div className="flex min-h-0 flex-1">
            <ResultList />
            <div className="w-px shrink-0 bg-edge" />
            <PreviewPane />
          </div>
          <Footer />
        </>
      )}
      {view === "note" && <NoteEditor />}
      {view === "credential" && <CredentialEditor />}
      {view === "settings" && <Settings />}

      <TransformMenu />
      <ResizeGrips />

      {error && (
        <div className="shrink-0 border-t border-red-900/50 bg-red-950/40 px-4 py-1.5 text-[11px] text-red-300">
          {error}
        </div>
      )}
    </div>
  );
}

function Footer() {
  const setView = useStash((s) => s.setView);

  return (
    <div className="flex shrink-0 items-center gap-3 overflow-hidden border-t border-edge px-4 py-2 text-[11px] text-zinc-600">
      {/* ↑↓ came off this row to make space: the arrow keys are the one
          binding nobody needs telling about. */}
      <span>↵ paste</span>
      <span>Ctrl+↵ copy</span>
      <span>Ctrl+T as…</span>
      <span>Ctrl+K cred</span>
      <span>Ctrl+N note</span>
      <span>Ctrl+P pin</span>
      <span className="ml-auto">Esc hide</span>
      {/* The only in-window route to settings -- and so to renaming and
          deleting folders. Without it the tray menu is the sole discoverable
          path, since Ctrl+, is listed nowhere. */}
      <button
        onClick={() => setView("settings")}
        title="Settings (Ctrl+,)"
        aria-label="Settings"
        className="shrink-0 rounded px-1 leading-none text-zinc-600 transition-colors hover:text-zinc-300"
      >
        ⚙
      </button>
    </div>
  );
}

/**
 * The window is undecorated, so the OS draws no resize border. These invisible
 * strips along each edge supply one. They need
 * `core:window:allow-start-resize-dragging` in capabilities/default.json.
 */
function ResizeGrips() {
  const grip = (dir: ResizeDirection, className: string, cursor: string) => (
    <div
      key={dir}
      onMouseDown={(e) => {
        // Left button only, and never let the drag-region handler also fire.
        if (e.button !== 0) return;
        e.preventDefault();
        e.stopPropagation();
        void startResize(dir);
      }}
      className={`absolute z-50 ${className}`}
      style={{ cursor }}
    />
  );

  return (
    <>
      {grip("North", "left-3 right-3 top-0 h-1.5", "ns-resize")}
      {grip("South", "bottom-0 left-3 right-3 h-1.5", "ns-resize")}
      {grip("West", "bottom-3 left-0 top-3 w-1.5", "ew-resize")}
      {grip("East", "bottom-3 right-0 top-3 w-1.5", "ew-resize")}
      {grip("NorthWest", "left-0 top-0 h-3 w-3", "nwse-resize")}
      {grip("NorthEast", "right-0 top-0 h-3 w-3", "nesw-resize")}
      {grip("SouthWest", "bottom-0 left-0 h-3 w-3", "nesw-resize")}
      {grip("SouthEast", "bottom-0 right-0 h-3.5 w-3.5", "nwse-resize")}
    </>
  );
}

/**
 * Subscribes once to the events Rust emits. Nothing here polls: when the
 * clipboard does not change, the backend sends nothing and React never
 * re-renders, which is what keeps idle cost at zero.
 */
function useBackendEvents() {
  const refresh = useStash((s) => s.refresh);
  const bootstrap = useStash((s) => s.bootstrap);
  const setView = useStash((s) => s.setView);
  const clearRevealed = useStash((s) => s.clearRevealed);

  useEffect(() => {
    void bootstrap();

    const unlisteners: Array<() => void> = [];
    const track = (p: Promise<() => void>) => {
      void p.then((fn) => unlisteners.push(fn));
    };

    track(onItemAdded(() => void refresh()));
    track(onItemBumped(() => void refresh()));
    track(
      onPanelOpened(() => {
        // Reopening the panel must never show a password left revealed from
        // the last time it was up.
        clearRevealed();
        void refresh();
      }),
    );
    track(onOpenSettings(() => setView("settings")));

    return () => unlisteners.forEach((fn) => fn());
  }, [refresh, bootstrap, setView, clearRevealed]);
}

function usePanelHotkeys() {
  const view = useStash((s) => s.view);
  const move = useStash((s) => s.move);
  const copySelected = useStash((s) => s.copySelected);
  const pasteSelected = useStash((s) => s.pasteSelected);
  const copySelectedPassword = useStash((s) => s.copySelectedPassword);
  const revealSelectedPassword = useStash((s) => s.revealSelectedPassword);
  const deleteSelected = useStash((s) => s.deleteSelected);
  const pinSelected = useStash((s) => s.pinSelected);
  const openNote = useStash((s) => s.openNote);
  const openCredential = useStash((s) => s.openCredential);
  const setView = useStash((s) => s.setView);
  const setFolder = useStash((s) => s.setFolder);
  const openTransforms = useStash((s) => s.openTransforms);
  const transformsOpen = useStash((s) => s.transforms !== null);

  const onEscape = useCallback(() => {
    void hidePanel();
  }, []);

  // Reads the current selection at call time rather than subscribing to items,
  // so the handler identity does not change on every search.
  const onEditSelected = useCallback(() => {
    const { items, selected } = useStash.getState();
    const item = items[selected];
    if (!item) return;
    if (isCredential(item)) openCredential(item.id);
    else if (item.kind === "note") openNote(item.id);
  }, [openCredential, openNote]);

  const onFolderDigit = useCallback(
    (digit: number) => {
      if (digit === 0) {
        setFolder(null);
        return;
      }
      const folder = useStash.getState().folders[digit - 1];
      if (folder) setFolder(folder.id);
    },
    [setFolder],
  );

  const handlers = useMemo(
    () => ({
      onUp: () => move(-1),
      onDown: () => move(1),
      onEnter: () => void pasteSelected(),
      onCopyOnly: () => void copySelected(),
      onDelete: () => void deleteSelected(),
      onEscape,
      onNewNote: () => openNote(null),
      onNewCredential: () => openCredential(null),
      onTransform: () => void openTransforms(),
      onSettings: () => setView("settings"),
      onPin: () => void pinSelected(),
      onCopyPassword: () => void copySelectedPassword(),
      onToggleReveal: () => void revealSelectedPassword(),
      onEditSelected,
      onFolderDigit,
    }),
    [
      move,
      copySelected,
      pasteSelected,
      deleteSelected,
      onEscape,
      openNote,
      openCredential,
      setView,
      pinSelected,
      copySelectedPassword,
      revealSelectedPassword,
      onEditSelected,
      onFolderDigit,
      openTransforms,
    ],
  );

  // The note editor and settings own their own Escape and Ctrl+S handling, so
  // the global list bindings stand down while either is open. So does the
  // transform menu, which would otherwise have Up/Down move the list behind it.
  useHotkeys(handlers, view === "list" && !transformsOpen);
}
