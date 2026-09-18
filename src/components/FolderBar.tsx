import { useState } from "react";
import { useStash } from "../store/useStash";

/**
 * Folder chips, rendered inside the existing filter row rather than in a row of
 * their own. That is what keeps the preview pane free: the split body loses no
 * vertical space to this feature.
 *
 * Only the first nine carry an Alt-digit hint, because that is how many digits
 * there are. The rest are still clickable.
 */
export function FolderBar() {
  const folders = useStash((s) => s.folders);
  const activeFolderId = useStash((s) => s.activeFolderId);
  const setFolder = useStash((s) => s.setFolder);
  const createFolder = useStash((s) => s.createFolder);

  const [adding, setAdding] = useState(false);
  const [draft, setDraft] = useState("");

  const commit = async () => {
    const name = draft.trim();
    setAdding(false);
    setDraft("");
    if (!name) return;
    const folder = await createFolder(name);
    if (folder) setFolder(folder.id);
  };

  return (
    <>
      <Chip label="All" active={activeFolderId === null} onClick={() => setFolder(null)} />

      {folders.map((folder, i) => (
        <Chip
          key={folder.id}
          label={folder.name}
          hint={i < 9 ? `Alt+${i + 1}` : undefined}
          active={activeFolderId === folder.id}
          onClick={() => setFolder(activeFolderId === folder.id ? null : folder.id)}
        />
      ))}

      {adding ? (
        <input
          autoFocus
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={() => void commit()}
          onKeyDown={(e) => {
            // The list's global bindings would otherwise eat these keys.
            e.stopPropagation();
            if (e.key === "Enter") void commit();
            if (e.key === "Escape") {
              setAdding(false);
              setDraft("");
            }
          }}
          placeholder="Folder name"
          className="w-28 shrink-0 rounded-full border border-accent/60 bg-transparent px-2.5 py-1 text-xs text-zinc-100 outline-none placeholder:text-zinc-600"
        />
      ) : (
        <button
          onClick={() => setAdding(true)}
          title="New folder"
          aria-label="New folder"
          className="shrink-0 rounded-full border border-dashed border-edge px-2 py-1 text-xs text-zinc-500 transition-colors hover:border-zinc-600 hover:text-zinc-300"
        >
          +
        </button>
      )}
    </>
  );
}

function Chip({
  label,
  hint,
  active,
  onClick,
}: {
  label: string;
  hint?: string;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      aria-pressed={active}
      title={hint ? `${label} (${hint})` : label}
      className={[
        "max-w-[7rem] shrink-0 truncate rounded-full border px-2.5 py-1 text-xs transition-colors",
        active
          ? "border-accent/60 bg-accent/15 text-accent"
          : "border-edge text-zinc-400 hover:border-zinc-600 hover:text-zinc-200",
      ].join(" ")}
    >
      {label}
    </button>
  );
}
