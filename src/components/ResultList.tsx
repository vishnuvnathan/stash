import { useVirtualizer } from "@tanstack/react-virtual";
import { useCallback, useEffect, useRef } from "react";
import { isCredential } from "../lib/types";
import { useStash } from "../store/useStash";
import { ResultRow } from "./ResultRow";

const ROW_HEIGHT = 56;

/**
 * Fixed rather than flexible: the preview pane takes the rest. At 320px a row
 * has ~244px for its title, which is enough for the names people actually give
 * things. Both returns below must carry it or the empty state collapses the
 * split.
 */
const LIST_WIDTH = "w-[320px] shrink-0";

export function ResultList() {
  const items = useStash((s) => s.items);
  const selected = useStash((s) => s.selected);
  const select = useStash((s) => s.select);
  const copySelected = useStash((s) => s.copySelected);
  const openNote = useStash((s) => s.openNote);
  const openCredential = useStash((s) => s.openCredential);

  const parentRef = useRef<HTMLDivElement>(null);

  const virtualizer = useVirtualizer({
    count: items.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 6,
  });

  // Keyboard navigation moves `selected`; the list follows it rather than the
  // other way round, so arrowing past the fold scrolls by exactly one row.
  useEffect(() => {
    if (items.length === 0) return;
    virtualizer.scrollToIndex(selected, { align: "auto" });
  }, [selected, items.length, virtualizer]);

  const activate = useCallback(
    (index: number) => {
      const item = items[index];
      if (!item) return;
      // Credentials are notes underneath, so this branch must come first or
      // they would open in the markdown editor.
      if (isCredential(item)) {
        openCredential(item.id);
      } else if (item.kind === "note") {
        openNote(item.id);
      } else {
        void copySelected();
      }
    },
    [items, openNote, openCredential, copySelected],
  );

  if (items.length === 0) {
    return (
      <div className={`${LIST_WIDTH} flex items-center justify-center text-sm text-zinc-600`}>
        Nothing here yet
      </div>
    );
  }

  return (
    <div ref={parentRef} className={`${LIST_WIDTH} overflow-y-auto overscroll-contain`}>
      <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
        {virtualizer.getVirtualItems().map((row) => {
          const item = items[row.index];
          if (!item) return null;
          return (
            <div
              key={item.id}
              style={{
                position: "absolute",
                top: 0,
                left: 0,
                width: "100%",
                height: row.size,
                transform: `translateY(${row.start}px)`,
              }}
            >
              <ResultRow
                item={item}
                active={row.index === selected}
                onClick={() => select(row.index)}
                onDoubleClick={() => activate(row.index)}
              />
            </div>
          );
        })}
      </div>
    </div>
  );
}
