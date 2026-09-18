import { TYPE_CHIPS } from "../lib/types";
import { useStash } from "../store/useStash";
import { FolderBar } from "./FolderBar";

export function FilterChips() {
  const typeChips = useStash((s) => s.typeChips);
  const toggleTypeChip = useStash((s) => s.toggleTypeChip);
  const sourceApps = useStash((s) => s.sourceApps);
  const activeSourceApps = useStash((s) => s.activeSourceApps);
  const toggleSourceApp = useStash((s) => s.toggleSourceApp);
  const clearFilters = useStash((s) => s.clearFilters);
  const activeFolderId = useStash((s) => s.activeFolderId);

  const anyActive =
    typeChips.length > 0 || activeSourceApps.length > 0 || activeFolderId !== null;

  return (
    <div className="flex shrink-0 items-center gap-1.5 overflow-x-auto border-b border-edge px-4 py-2 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
      {/* Folders lead the row. Keeping them here rather than in a row of their
          own is what leaves the preview pane its full height. */}
      <FolderBar />

      <div className="mx-1 h-4 w-px shrink-0 bg-edge" />

      {TYPE_CHIPS.map((chip) => (
        <Chip
          key={chip.id}
          label={chip.label}
          active={typeChips.includes(chip.id)}
          onClick={() => toggleTypeChip(chip.id)}
        />
      ))}

      {sourceApps.length > 0 && <div className="mx-1 h-4 w-px shrink-0 bg-edge" />}

      {/* Capped: the chip row is a shortcut, not a directory. Anything not
          listed is still reachable by typing the app name into search. */}
      {sourceApps.slice(0, 8).map((app) => (
        <Chip
          key={app}
          label={app}
          active={activeSourceApps.includes(app)}
          onClick={() => toggleSourceApp(app)}
        />
      ))}

      {anyActive && (
        <button
          onClick={clearFilters}
          className="ml-auto shrink-0 px-2 text-xs text-zinc-500 transition-colors hover:text-zinc-300"
        >
          Clear
        </button>
      )}
    </div>
  );
}

function Chip({
  label,
  active,
  onClick,
}: {
  label: string;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      aria-pressed={active}
      className={[
        "shrink-0 rounded-full border px-2.5 py-1 text-xs transition-colors",
        active
          ? "border-accent/60 bg-accent/15 text-accent"
          : "border-edge text-zinc-400 hover:border-zinc-600 hover:text-zinc-200",
      ].join(" ")}
    >
      {label}
    </button>
  );
}
