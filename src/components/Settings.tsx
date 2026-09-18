import { useEffect, useState } from "react";
import { BackupSection } from "./BackupSection";
import { securityInfo, setCaptureExclusion } from "../lib/ipc";
import type { CaptureStatus, SecurityInfo } from "../lib/types";
import { useStash } from "../store/useStash";

export function Settings() {
  const settings = useStash((s) => s.settings);
  const updateSettings = useStash((s) => s.updateSettings);
  const setView = useStash((s) => s.setView);

  const [capture, setCapture] = useState<CaptureStatus | null>(null);
  const [security, setSecurity] = useState<SecurityInfo | null>(null);

  // Ask the backend what the platform can actually do, so the toggle below can
  // say "unsupported here" instead of pretending to work.
  useEffect(() => {
    if (!settings) return;
    void setCaptureExclusion(settings.captureExclusion)
      .then(setCapture)
      .catch(() => undefined);
  }, [settings?.captureExclusion]);

  // Platform facts, not preferences: whether secrets are really encrypted and
  // whether pasting can work at all. Fetched once.
  useEffect(() => {
    void securityInfo().then(setSecurity).catch(() => undefined);
  }, []);

  if (!settings) {
    return (
      <div className="flex flex-1 items-center justify-center text-sm text-zinc-600">
        Loading settings…
      </div>
    );
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex items-center justify-between border-b border-edge px-4 py-3">
        <h1 className="text-[15px] text-zinc-100">Settings</h1>
        <button
          onClick={() => setView("list")}
          className="rounded border border-edge px-2 py-1 text-xs text-zinc-400 hover:text-zinc-200"
        >
          Back
        </button>
      </div>

      {/* min-h-0 on both levels, for the reason spelled out in App.tsx: a
          flex-1 child defaults to min-height:auto, so it grows to fit its
          content instead of shrinking, and the overflow-y-auto below it
          then has nothing to scroll. */}
      <div className="min-h-0 flex-1 overflow-y-auto px-4 py-3">
        <Toggle
          label="Paste directly into the active app"
          description={pasteDescription(security)}
          checked={settings.autoPaste}
          disabled={security ? !security.pasteSupported : false}
          onChange={(v) => void updateSettings({ autoPaste: v })}
        />

        <Toggle
          label="Hide panel from screen capture"
          description={captureDescription(capture)}
          checked={settings.captureExclusion}
          disabled={capture ? !capture.supported : false}
          onChange={(v) => void updateSettings({ captureExclusion: v })}
        />

        <Toggle
          label="Reopen the panel where I left it"
          description="Off: the panel centres on the monitor under your cursor each time. On: it returns to the last place you dragged it."
          checked={settings.rememberPosition}
          onChange={(v) => void updateSettings({ rememberPosition: v })}
        />

        <Toggle
          label="Launch on startup"
          description="Start Stash with Windows, straight to the tray."
          checked={settings.launchOnStartup}
          onChange={(v) => void updateSettings({ launchOnStartup: v })}
        />

        <FolderList />

        <BackupSection />

        <CredentialSecurityNote encrypted={security?.secretsEncrypted ?? null} />

        <div className="mt-5 border-t border-edge pt-3 text-[11px] leading-5 text-zinc-600">
          <p>Stash keeps the newest 1000 unpinned clips. Pinned items, notes and credentials are never pruned.</p>
          <p className="mt-1">
            Everything stays on this machine. Stash makes no network requests.
          </p>
        </div>
      </div>
    </div>
  );
}

/**
 * Rename and delete only. Creating one lives on the chip row, where you are
 * when you realise you want it.
 */
function FolderList() {
  const folders = useStash((s) => s.folders);
  const renameFolder = useStash((s) => s.renameFolder);
  const deleteFolder = useStash((s) => s.deleteFolder);

  if (folders.length === 0) {
    return (
      <div className="mt-5 border-t border-edge pt-3">
        <h2 className="text-[13px] text-zinc-200">Folders</h2>
        <p className="mt-1.5 text-[11px] text-zinc-600">
          None yet. Use the + on the filter row to add one.
        </p>
      </div>
    );
  }

  return (
    <div className="mt-5 border-t border-edge pt-3">
      <h2 className="text-[13px] text-zinc-200">Folders</h2>
      <p className="mt-1 text-[11px] leading-4 text-zinc-600">
        Deleting a folder unfiles its items. Nothing is lost.
      </p>

      <ul className="mt-2 space-y-1">
        {folders.map((folder) => (
          <FolderRow
            key={folder.id}
            id={folder.id}
            name={folder.name}
            onRename={renameFolder}
            onDelete={deleteFolder}
          />
        ))}
      </ul>
    </div>
  );
}

/**
 * Rename and delete are inline rather than `window.prompt`/`confirm`: those are
 * unreliable inside a WebView2 host, and a dialog that silently returns null
 * would look like a dead button.
 */
function FolderRow({
  id,
  name,
  onRename,
  onDelete,
}: {
  id: string;
  name: string;
  onRename: (id: string, name: string) => Promise<void>;
  onDelete: (id: string) => Promise<void>;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(name);
  const [confirming, setConfirming] = useState(false);

  const commit = () => {
    setEditing(false);
    if (draft.trim() && draft.trim() !== name) void onRename(id, draft);
    else setDraft(name);
  };

  return (
    <li className="flex items-center gap-2 rounded border border-edge bg-panelalt px-2.5 py-1.5">
      {editing ? (
        <input
          autoFocus
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => {
            e.stopPropagation();
            if (e.key === "Enter") commit();
            if (e.key === "Escape") {
              setDraft(name);
              setEditing(false);
            }
          }}
          className="min-w-0 flex-1 rounded border border-accent/60 bg-transparent px-1.5 py-0.5 text-[12px] text-zinc-100 outline-none"
        />
      ) : (
        <span className="min-w-0 flex-1 truncate text-[12px] text-zinc-200">{name}</span>
      )}

      {confirming ? (
        <>
          <span className="shrink-0 text-[11px] text-zinc-500">Items stay.</span>
          <button
            onClick={() => void onDelete(id)}
            className="shrink-0 text-[11px] text-red-400 hover:text-red-300"
          >
            Delete
          </button>
          <button
            onClick={() => setConfirming(false)}
            className="shrink-0 text-[11px] text-zinc-500 hover:text-zinc-200"
          >
            Cancel
          </button>
        </>
      ) : (
        <>
          <button
            onClick={() => setEditing(true)}
            className="shrink-0 text-[11px] text-zinc-500 hover:text-zinc-200"
          >
            Rename
          </button>
          <button
            onClick={() => setConfirming(true)}
            className="shrink-0 text-[11px] text-zinc-500 hover:text-red-400"
          >
            Delete
          </button>
        </>
      )}
    </li>
  );
}

/**
 * Says what encryption does and, just as importantly, what it does not. DPAPI
 * ties the ciphertext to the Windows account, which makes a stolen stash.db
 * useless — but it does not stop a program running as you from asking Windows
 * to decrypt it. The old warning was blunter and, until now, correct; watering
 * it down to "encrypted ✓" would trade one inaccuracy for another.
 */
function CredentialSecurityNote({ encrypted }: { encrypted: boolean | null }) {
  if (encrypted === null) return null;

  if (!encrypted) {
    return (
      <div className="mt-5 rounded-md border border-amber-500/25 bg-amber-500/[0.06] px-3 py-2.5">
        <p className="text-[12px] text-amber-300">Credentials are not encrypted</p>
        <p className="mt-1 text-[11px] leading-[1.5] text-amber-400/80">
          This platform has no key store Stash can use, so passwords are stored as plain text in
          stash.db. Anyone who can read that file can read them.
        </p>
      </div>
    );
  }

  return (
    <div className="mt-5 rounded-md border border-edge bg-panelalt px-3 py-2.5">
      <p className="text-[12px] text-zinc-300">Credentials are encrypted</p>
      <p className="mt-1 text-[11px] leading-[1.5] text-zinc-500">
        Passwords are sealed with Windows DPAPI under your account, so stash.db is unreadable on
        another machine or from another Windows account. It is not protection against a program
        running as you — that program can ask Windows to decrypt them, exactly as Stash does.
      </p>
      <p className="mt-1.5 text-[11px] leading-[1.5] text-zinc-500">
        A copied password is cleared from the clipboard after 30 seconds. Keep a backup elsewhere:
        this file is the only copy, and it is tied to this Windows account.
      </p>
    </div>
  );
}

function pasteDescription(status: SecurityInfo | null): string {
  if (status && !status.pasteSupported) {
    return status.pasteReason ?? "Not supported on this platform.";
  }
  return "Enter pastes into the window you came from. Ctrl+Enter copies without pasting.";
}

function captureDescription(status: CaptureStatus | null): string {
  if (!status) return "Keeps the panel out of screen shares and recordings.";
  if (!status.supported) {
    return status.reason ?? "Not supported on this platform.";
  }
  if (status.reason) return status.reason;
  return "Keeps the panel out of screen shares and recordings.";
}

function Toggle({
  label,
  description,
  checked,
  disabled,
  onChange,
}: {
  label: string;
  description: string;
  checked: boolean;
  disabled?: boolean;
  onChange: (value: boolean) => void;
}) {
  return (
    <label
      className={[
        "flex items-start gap-3 py-2.5",
        disabled ? "opacity-50" : "cursor-pointer",
      ].join(" ")}
    >
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
        className="mt-0.5 h-4 w-4 shrink-0 accent-[#7aa2f7]"
      />
      <span className="min-w-0">
        <span className="block text-[13px] text-zinc-200">{label}</span>
        <span className="block text-[11px] leading-4 text-zinc-500">{description}</span>
      </span>
    </label>
  );
}
