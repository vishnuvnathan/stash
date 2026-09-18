import { useEffect, useState } from "react";
import ReactMarkdown from "react-markdown";
import { readBlob } from "../lib/ipc";
import { isCredential, type Item } from "../lib/types";
import { useStash } from "../store/useStash";
import { CredentialCard } from "./CredentialCard";

/**
 * The right half of the list view.
 *
 * It renders straight from `items[selected]` with no IPC of its own:
 * `search_items` already returns each row's full `content`, so arrowing through
 * the list stays instant. The one exception is revealing a password, which is a
 * deliberate click.
 */
export function PreviewPane() {
  const items = useStash((s) => s.items);
  const selected = useStash((s) => s.selected);
  const folders = useStash((s) => s.folders);

  const item = items[selected];

  if (!item) {
    return (
      <div className="flex min-w-0 flex-1 items-center justify-center text-[12px] text-zinc-600">
        Nothing selected
      </div>
    );
  }

  const folder = folders.find((f) => f.id === item.folderId);

  return (
    <div className="flex min-w-0 flex-1 flex-col">
      <div className="flex shrink-0 items-center gap-2 border-b border-edge px-4 py-2">
        <span className="truncate text-[12px] text-zinc-100">{item.title || "(empty)"}</span>
        <span
          className={[
            "shrink-0 rounded px-1.5 py-0.5 text-[10px] uppercase tracking-wide",
            isCredential(item) ? "bg-accent/15 text-accent" : "bg-panelalt text-zinc-500",
          ].join(" ")}
        >
          {isCredential(item) ? "cred" : item.kind === "note" ? "note" : item.contentType}
        </span>
        {/* What `sort:used` ranks by. Shown only once it is non-zero, so a
            history full of "used 0×" does not add noise to every row. */}
        {item.useCount > 0 && (
          <span
            className="ml-auto shrink-0 text-[11px] tabular-nums text-zinc-500"
            title="Times copied or pasted from Stash. Rank by this with sort:used"
          >
            used {item.useCount}×
          </span>
        )}
        {folder && (
          <span
            className={[
              "shrink-0 truncate text-[11px] text-zinc-500",
              item.useCount > 0 ? "" : "ml-auto",
            ].join(" ")}
          >
            {folder.name}
          </span>
        )}
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto px-4 py-3">
        <Body item={item} />
      </div>

      <Hints item={item} />
    </div>
  );
}

function Body({ item }: { item: Item }) {
  if (isCredential(item)) {
    return <CredentialCard item={item} />;
  }

  if (item.contentType === "image" && item.blobPath) {
    return <ImageBody relPath={item.blobPath} />;
  }

  if (item.kind === "note") {
    return (
      <div className="prose-stash text-[13px] leading-6 text-zinc-200">
        <ReactMarkdown>{item.content || "*This note is empty.*"}</ReactMarkdown>
      </div>
    );
  }

  return (
    <pre className="select-text whitespace-pre-wrap break-words font-mono text-[12px] leading-[1.6] text-zinc-300">
      {item.content || ""}
    </pre>
  );
}

/** Same object-URL discipline as the row thumbnails: revoked on unmount. */
function ImageBody({ relPath }: { relPath: string }) {
  const [url, setUrl] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    let objectUrl: string | undefined;

    void readBlob(relPath)
      .then((blob) => {
        if (cancelled) return;
        objectUrl = URL.createObjectURL(blob);
        setUrl(objectUrl);
      })
      .catch(() => undefined);

    return () => {
      cancelled = true;
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [relPath]);

  if (!url) return <div className="h-32 rounded border border-edge bg-panelalt" />;
  return <img src={url} alt="" className="max-h-full max-w-full rounded object-contain" />;
}

function Hints({ item }: { item: Item }) {
  const hints = isCredential(item)
    ? [
        ["↵", "username"],
        ["Ctrl+⇧+C", "password"],
        ["Ctrl+⇧+E", "edit"],
      ]
    : item.kind === "note"
      ? [
          ["↵", "copy"],
          ["Ctrl+⇧+E", "edit"],
        ]
      : [["↵", "copy"]];

  return (
    <div className="flex shrink-0 items-center gap-3 border-t border-edge px-4 py-2 text-[11px] text-zinc-500">
      {hints.map(([key, label]) => (
        <span key={label}>
          <span className="text-zinc-300">{key}</span> {label}
        </span>
      ))}
    </div>
  );
}
