import { useEffect, useRef, useState } from "react";
import { getCredential } from "../lib/ipc";
import { useStash } from "../store/useStash";

/**
 * Sibling of NoteEditor, same Ctrl+S / Esc convention and the same
 * stopPropagation so the list's global bindings stand down.
 *
 * The password field starts empty even when editing an existing credential —
 * deliberately. `save_credential` treats `password: null` as "leave the stored
 * one alone", so the editor never has to pull the plaintext across IPC just to
 * put it back. Typing in the field replaces it; clearing it explicitly is done
 * with the Clear button.
 */
export function CredentialEditor() {
  const editingCredentialId = useStash((s) => s.editingCredentialId);
  const saveCredential = useStash((s) => s.saveCredential);
  const setView = useStash((s) => s.setView);
  const folders = useStash((s) => s.folders);
  const createFolder = useStash((s) => s.createFolder);
  const activeFolderId = useStash((s) => s.activeFolderId);

  const [title, setTitle] = useState("");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [clearPassword, setClearPassword] = useState(false);
  const [showPassword, setShowPassword] = useState(false);
  const [url, setUrl] = useState("");
  const [notes, setNotes] = useState("");
  const [folderId, setFolderId] = useState<string | null>(activeFolderId);
  const [hasPassword, setHasPassword] = useState(false);

  const titleRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!editingCredentialId) {
      titleRef.current?.focus();
      return;
    }
    let cancelled = false;
    void getCredential(editingCredentialId)
      .then((view) => {
        if (cancelled) return;
        setTitle(view.item.title ?? "");
        setUsername(view.username ?? "");
        setUrl(view.url ?? "");
        setNotes(view.notes ?? "");
        setFolderId(view.item.folderId);
        setHasPassword(view.hasPassword);
        titleRef.current?.focus();
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [editingCredentialId]);

  const submit = () => {
    void saveCredential({
      title,
      username: username || null,
      // "" is the signal to delete the stored secret; null means leave it.
      password: clearPassword ? "" : password ? password : null,
      url: url || null,
      notes: notes || null,
      folderId,
    });
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    const mod = e.ctrlKey || e.metaKey;
    if (mod && e.key.toLowerCase() === "s") {
      e.preventDefault();
      e.stopPropagation();
      submit();
    }
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      setView("list");
    }
  };

  // Inline rather than `window.prompt`, which is unreliable in WebView2.
  const [newFolder, setNewFolder] = useState<string | null>(null);

  const commitNewFolder = async () => {
    const name = (newFolder ?? "").trim();
    setNewFolder(null);
    if (!name) return;
    const folder = await createFolder(name);
    if (folder) setFolderId(folder.id);
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col" onKeyDown={onKeyDown}>
      <div className="flex shrink-0 items-center gap-3 border-b border-edge px-4 py-3">
        <button
          onClick={() => setView("list")}
          aria-label="Back to list"
          className="flex h-6 w-6 shrink-0 items-center justify-center rounded border border-edge text-zinc-400 hover:text-zinc-200"
        >
          ‹
        </button>
        <h1 className="text-[13px] text-zinc-100">
          {editingCredentialId ? "Edit credential" : "New credential"}
        </h1>
        <button
          onClick={submit}
          className="ml-auto rounded bg-accent px-3.5 py-1.5 text-[11.5px] font-semibold text-panel transition-opacity hover:opacity-90"
        >
          Save
        </button>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto px-4 py-3">
        <Labelled label="Label" htmlFor="cred-title">
          <input
            id="cred-title"
            ref={titleRef}
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            placeholder="e.g. SAP Production"
            className={inputClass}
          />
        </Labelled>

        <div className="mt-3 flex gap-3">
          <Labelled label="Username" htmlFor="cred-user" className="min-w-0 flex-1">
            <input
              id="cred-user"
              value={username}
              onChange={(e) => setUsername(e.target.value)}
              spellCheck={false}
              className={`${inputClass} font-mono`}
            />
          </Labelled>

          <Labelled label="Password" htmlFor="cred-pass" className="min-w-0 flex-1">
            <div className="flex gap-1.5">
              <input
                id="cred-pass"
                type={showPassword ? "text" : "password"}
                value={password}
                disabled={clearPassword}
                onChange={(e) => setPassword(e.target.value)}
                placeholder={hasPassword ? "unchanged" : ""}
                spellCheck={false}
                className={`${inputClass} min-w-0 flex-1 font-mono disabled:opacity-40`}
              />
              <button
                type="button"
                onClick={() => setShowPassword((s) => !s)}
                aria-label={showPassword ? "Hide password" : "Show password"}
                className="h-[34px] w-[34px] shrink-0 rounded-md border border-edge bg-panelalt text-[13px] text-accent"
              >
                {showPassword ? "◎" : "◉"}
              </button>
            </div>
            {hasPassword && (
              <label className="mt-1.5 flex cursor-pointer items-center gap-1.5 text-[10.5px] text-zinc-500">
                <input
                  type="checkbox"
                  checked={clearPassword}
                  onChange={(e) => setClearPassword(e.target.checked)}
                  className="h-3 w-3 accent-[#7aa2f7]"
                />
                Remove the stored password
              </label>
            )}
          </Labelled>
        </div>

        <div className="mt-3 flex gap-3">
          <Labelled label="URL" htmlFor="cred-url" className="min-w-0 flex-1">
            <input
              id="cred-url"
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              spellCheck={false}
              className={`${inputClass} font-mono`}
            />
          </Labelled>

          <Labelled label="Folder" htmlFor="cred-folder" className="w-44 shrink-0">
            {newFolder === null ? (
              <div className="flex gap-1.5">
                <select
                  id="cred-folder"
                  value={folderId ?? ""}
                  onChange={(e) => setFolderId(e.target.value || null)}
                  className={`${inputClass} min-w-0 flex-1`}
                >
                  <option value="">No folder</option>
                  {folders.map((f) => (
                    <option key={f.id} value={f.id}>
                      {f.name}
                    </option>
                  ))}
                </select>
                <button
                  type="button"
                  onClick={() => setNewFolder("")}
                  aria-label="New folder"
                  title="New folder"
                  className="h-[34px] w-[34px] shrink-0 rounded-md border border-dashed border-edge bg-panelalt text-accent"
                >
                  +
                </button>
              </div>
            ) : (
              <input
                autoFocus
                value={newFolder}
                onChange={(e) => setNewFolder(e.target.value)}
                onBlur={() => void commitNewFolder()}
                onKeyDown={(e) => {
                  e.stopPropagation();
                  if (e.key === "Enter") void commitNewFolder();
                  if (e.key === "Escape") setNewFolder(null);
                }}
                placeholder="New folder name"
                className={`${inputClass} border-accent/60`}
              />
            )}
          </Labelled>
        </div>

        <Labelled label="Notes" htmlFor="cred-notes" className="mt-3 block">
          <textarea
            id="cred-notes"
            value={notes}
            onChange={(e) => setNotes(e.target.value)}
            rows={4}
            placeholder="MFA, client number, rotation schedule…"
            className={`${inputClass} h-24 resize-none py-2 leading-[1.6]`}
          />
        </Labelled>

        <p className="mt-3 rounded-md border border-amber-500/25 bg-amber-500/[0.06] px-2.5 py-2 text-[10.5px] leading-[1.5] text-amber-400/90">
          Stored unencrypted in stash.db on this machine. Anyone signed into your Windows account
          can read it.
        </p>
      </div>

      <div className="flex shrink-0 items-center gap-3 border-t border-edge px-4 py-2 text-[11px] text-zinc-500">
        <span>Ctrl+S save</span>
        <span>Esc back</span>
      </div>
    </div>
  );
}

const inputClass =
  "w-full rounded-md border border-edge bg-panelalt px-2.5 text-[13px] text-zinc-200 outline-none h-[34px] placeholder:text-zinc-600 focus:border-accent/60";

function Labelled({
  label,
  htmlFor,
  className,
  children,
}: {
  label: string;
  htmlFor: string;
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <div className={className}>
      <label
        htmlFor={htmlFor}
        className="mb-1.5 block text-[10px] uppercase tracking-[0.08em] text-zinc-500"
      >
        {label}
      </label>
      {children}
    </div>
  );
}
