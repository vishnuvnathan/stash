import { useEffect, useRef } from "react";
import { onPanelOpened } from "../lib/ipc";
import { useStash } from "../store/useStash";

export function SearchBar() {
  const query = useStash((s) => s.query);
  const setQuery = useStash((s) => s.setQuery);
  const count = useStash((s) => s.items.length);
  const inputRef = useRef<HTMLInputElement>(null);

  // Rust emits `panel-opened` on every show, so the box is focused and its
  // contents selected each time the panel appears -- typing always replaces
  // the previous search rather than appending to it.
  useEffect(() => {
    inputRef.current?.focus();
    inputRef.current?.select();

    let unlisten: (() => void) | undefined;
    void onPanelOpened(() => {
      inputRef.current?.focus();
      inputRef.current?.select();
    }).then((fn) => {
      unlisten = fn;
    });

    return () => unlisten?.();
  }, []);

  return (
    <div className="border-b border-edge px-4 py-3">
      <div className="flex items-center gap-3">
        <SearchIcon />
        <input
          ref={inputRef}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search, or type app: type: folder: is: sort:"
          spellCheck={false}
          autoComplete="off"
          className="flex-1 bg-transparent text-[15px] text-zinc-100 outline-none placeholder:text-zinc-500"
        />
        <span className="shrink-0 text-xs tabular-nums text-zinc-500">
          {count === 0 ? "none" : `${count}${count === 200 ? "+" : ""}`}
        </span>
      </div>
      <QueryHint query={query} />
    </div>
  );
}

/**
 * The query language is invisible unless something mentions it. This appears
 * once the user types a colon — the moment they might be reaching for an
 * operator — and lists what actually exists, so a guess like `tag:` is quickly
 * corrected rather than silently searched for as text.
 */
function QueryHint({ query }: { query: string }) {
  if (!query.includes(":")) return null;

  return (
    <div className="mt-1.5 flex flex-wrap gap-x-3 gap-y-0.5 pl-7 text-[10px] leading-4 text-zinc-600">
      <span>
        <Op>app:</Op>Code
      </span>
      <span>
        <Op>type:</Op>code|link|text|image|cred
      </span>
      <span>
        <Op>folder:</Op>work
      </span>
      <span>
        <Op>is:</Op>pinned|note|clip|unfiled
      </span>
      <span>
        <Op>sort:</Op>used
      </span>
      <span>"exact phrase"</span>
    </div>
  );
}

function Op({ children }: { children: string }) {
  return <span className="text-zinc-400">{children}</span>;
}

function SearchIcon() {
  return (
    <svg
      viewBox="0 0 20 20"
      className="h-4 w-4 shrink-0 text-zinc-500"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.8}
      aria-hidden="true"
    >
      <circle cx="9" cy="9" r="6" />
      <path d="m13.5 13.5 3.5 3.5" strokeLinecap="round" />
    </svg>
  );
}
