import { useEffect, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import { getItem } from "../lib/ipc";
import { useStash } from "../store/useStash";

export function NoteEditor() {
  const editingNoteId = useStash((s) => s.editingNoteId);
  const saveNote = useStash((s) => s.saveNote);
  const setView = useStash((s) => s.setView);

  const [title, setTitle] = useState("");
  const [content, setContent] = useState("");
  const [preview, setPreview] = useState(false);
  const [loaded, setLoaded] = useState(false);

  const bodyRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    if (!editingNoteId) {
      setLoaded(true);
      bodyRef.current?.focus();
      return;
    }
    let cancelled = false;
    void getItem(editingNoteId).then((item) => {
      if (cancelled || !item) return;
      setTitle(item.title ?? "");
      setContent(item.content ?? "");
      setLoaded(true);
    });
    return () => {
      cancelled = true;
    };
  }, [editingNoteId]);

  useEffect(() => {
    if (loaded && !preview) bodyRef.current?.focus();
  }, [loaded, preview]);

  // Ctrl+S saves; Escape goes back to the list without hiding the panel, so a
  // stray Escape never costs you a half-written note.
  const onKeyDown = (e: React.KeyboardEvent) => {
    const mod = e.ctrlKey || e.metaKey;
    if (mod && e.key.toLowerCase() === "s") {
      e.preventDefault();
      e.stopPropagation();
      void saveNote(title, content);
    }
    if (mod && e.key.toLowerCase() === "e") {
      e.preventDefault();
      e.stopPropagation();
      setPreview((p) => !p);
    }
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      setView("list");
    }
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col" onKeyDown={onKeyDown}>
      <div className="flex items-center gap-3 border-b border-edge px-4 py-3">
        <input
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          placeholder="Untitled note"
          className="flex-1 bg-transparent text-[15px] text-zinc-100 outline-none placeholder:text-zinc-500"
        />
        <button
          onClick={() => setPreview((p) => !p)}
          className="rounded border border-edge px-2 py-1 text-xs text-zinc-400 hover:text-zinc-200"
        >
          {preview ? "Edit" : "Preview"}
        </button>
      </div>

      {preview ? (
        <div className="prose-stash min-h-0 flex-1 overflow-y-auto px-4 py-3 text-[13px] leading-6 text-zinc-200">
          <ReactMarkdown>{content || "*Nothing to preview yet.*"}</ReactMarkdown>
        </div>
      ) : (
        <textarea
          ref={bodyRef}
          value={content}
          onChange={(e) => setContent(e.target.value)}
          placeholder="Markdown…"
          spellCheck={false}
          className="flex-1 resize-none bg-transparent px-4 py-3 font-mono text-[13px] leading-6 text-zinc-200 outline-none placeholder:text-zinc-600"
        />
      )}

      <div className="flex items-center justify-between border-t border-edge px-4 py-2 text-[11px] text-zinc-500">
        <span>Ctrl+S save · Ctrl+E preview · Esc back</span>
        <button
          onClick={() => void saveNote(title, content)}
          className="rounded bg-accent/20 px-2.5 py-1 text-accent hover:bg-accent/30"
        >
          Save
        </button>
      </div>
    </div>
  );
}
