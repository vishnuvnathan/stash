import { useEffect, useRef, useState } from "react";
import { useStash } from "../store/useStash";

/**
 * The "paste as" menu, opened with Ctrl+T.
 *
 * The entries come from Rust, which decides them by actually trying to parse
 * the content — so Prettify only appears for real JSON and Decode only for
 * something that really decodes. The frontend just renders what it is given.
 *
 * It owns its own keyboard handling while open, because the panel's global
 * bindings would otherwise move the selection underneath it.
 */
export function TransformMenu() {
  const transforms = useStash((s) => s.transforms);
  const closeTransforms = useStash((s) => s.closeTransforms);
  const applyTransform = useStash((s) => s.applyTransform);

  const [active, setActive] = useState(0);
  const listRef = useRef<HTMLUListElement>(null);

  // A fresh menu always starts at the top; leaving the old index would put the
  // cursor on an unrelated entry when the options differ between items.
  useEffect(() => {
    setActive(0);
  }, [transforms]);

  useEffect(() => {
    if (!transforms) return;

    const onKeyDown = (e: KeyboardEvent) => {
      // Capture phase and stopPropagation: the panel's own listener is on
      // window too, and without this Up/Down would also move the result list.
      const count = transforms.length;
      switch (e.key) {
        case "ArrowDown":
          e.preventDefault();
          e.stopPropagation();
          setActive((i) => (i + 1) % count);
          return;
        case "ArrowUp":
          e.preventDefault();
          e.stopPropagation();
          setActive((i) => (i - 1 + count) % count);
          return;
        case "Escape":
          e.preventDefault();
          e.stopPropagation();
          closeTransforms();
          return;
        case "Enter": {
          e.preventDefault();
          e.stopPropagation();
          const option = transforms[active];
          // Ctrl+Enter copies instead of pasting, matching the list.
          if (option) void applyTransform(option.id, !(e.ctrlKey || e.metaKey));
          return;
        }
      }
    };

    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [transforms, active, closeTransforms, applyTransform]);

  useEffect(() => {
    listRef.current
      ?.querySelector<HTMLElement>(`[data-index="${active}"]`)
      ?.scrollIntoView({ block: "nearest" });
  }, [active]);

  if (!transforms) return null;

  return (
    <div
      className="absolute inset-0 z-40 flex items-center justify-center bg-black/50"
      onMouseDown={closeTransforms}
    >
      <div
        onMouseDown={(e) => e.stopPropagation()}
        className="max-h-[80%] w-72 overflow-hidden rounded-lg border border-edge bg-panel shadow-xl"
      >
        <div className="border-b border-edge px-3 py-2">
          <p className="text-[12px] text-zinc-200">Paste as</p>
          <p className="text-[10px] leading-4 text-zinc-600">
            Enter pastes · Ctrl+Enter copies · Esc cancels
          </p>
        </div>

        <ul ref={listRef} className="max-h-72 overflow-y-auto py-1">
          {transforms.map((option, i) => (
            <li key={option.id}>
              <button
                data-index={i}
                onMouseEnter={() => setActive(i)}
                onClick={() => void applyTransform(option.id, true)}
                className={[
                  "block w-full px-3 py-1.5 text-left text-[12px]",
                  i === active ? "bg-accent/20 text-zinc-100" : "text-zinc-400",
                ].join(" ")}
              >
                {option.label}
              </button>
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}
