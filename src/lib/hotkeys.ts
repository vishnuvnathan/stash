import { useEffect } from "react";

export interface HotkeyHandlers {
  onUp: () => void;
  onDown: () => void;
  /** Enter: paste into the window the panel was opened from. */
  onEnter: () => void;
  /** Ctrl+Enter: put it on the clipboard without pasting. */
  onCopyOnly: () => void;
  onDelete: () => void;
  onEscape: () => void;
  onNewNote: () => void;
  onNewCredential: () => void;
  /** Ctrl+T: open the "paste as" menu for the selection. */
  onTransform: () => void;
  onSettings: () => void;
  onPin: () => void;
  onCopyPassword: () => void;
  onToggleReveal: () => void;
  onEditSelected: () => void;
  /** 0 clears the folder filter; 1-9 select the folder at that position. */
  onFolderDigit: (digit: number) => void;
}

/**
 * One keydown listener for the whole panel. Rows do not attach their own, so
 * the cost of a 1000-item list is the same as a 10-item one.
 *
 * Keys are read from `event.code` where the physical key matters and `event.key`
 * where the character does, so a non-QWERTY layout still behaves.
 */
export function useHotkeys(handlers: HotkeyHandlers, enabled: boolean) {
  useEffect(() => {
    if (!enabled) return;

    const onKeyDown = (e: KeyboardEvent) => {
      const mod = e.ctrlKey || e.metaKey;

      switch (e.key) {
        case "ArrowUp":
          e.preventDefault();
          handlers.onUp();
          return;
        case "ArrowDown":
          e.preventDefault();
          handlers.onDown();
          return;
        case "Enter":
          // Handled here, above the `!mod` guard below, so the Ctrl variant has
          // to be split out explicitly rather than falling through to it.
          e.preventDefault();
          if (mod) handlers.onCopyOnly();
          else handlers.onEnter();
          return;
        case "Escape":
          e.preventDefault();
          handlers.onEscape();
          return;
        case "Backspace":
          // Plain Backspace has to keep editing the search box.
          if (mod) {
            e.preventDefault();
            handlers.onDelete();
          }
          return;
      }

      // Alt+0..9 picks a folder by position. Read from `e.code` because Alt
      // rewrites `e.key` to a dead character on several keyboard layouts.
      if (e.altKey && !mod) {
        const digit = /^Digit([0-9])$/.exec(e.code)?.[1];
        if (digit !== undefined) {
          e.preventDefault();
          handlers.onFolderDigit(Number(digit));
        }
        return;
      }

      if (!mod) return;

      // Ctrl+Shift+… are the credential bindings. Checked before the plain
      // Ctrl+… set so Ctrl+Shift+C is not read as a bare Ctrl+C.
      if (e.shiftKey) {
        switch (e.key.toLowerCase()) {
          case "c":
            e.preventDefault();
            handlers.onCopyPassword();
            break;
          case "u":
            e.preventDefault();
            handlers.onToggleReveal();
            break;
          case "e":
            e.preventDefault();
            handlers.onEditSelected();
            break;
        }
        return;
      }

      switch (e.key.toLowerCase()) {
        case "n":
          e.preventDefault();
          handlers.onNewNote();
          break;
        case "k":
          e.preventDefault();
          handlers.onNewCredential();
          break;
        case "p":
          e.preventDefault();
          handlers.onPin();
          break;
        case "t":
          e.preventDefault();
          handlers.onTransform();
          break;
        case ",":
          e.preventDefault();
          handlers.onSettings();
          break;
      }
    };

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [handlers, enabled]);
}
