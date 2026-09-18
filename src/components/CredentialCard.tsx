import ReactMarkdown from "react-markdown";
import type { Item } from "../lib/types";
import { useStash } from "../store/useStash";

interface Props {
  item: Item;
}

/**
 * The credential body of the preview pane.
 *
 * Two things worth knowing about what is *not* here. The password is not in
 * `item.content` — it lives in `item_secrets` and only arrives via an explicit
 * `revealPassword` call. And Copy does not route through this component either:
 * it calls into Rust, which writes the clipboard directly, so the plaintext
 * never enters the webview on that path.
 */
export function CredentialCard({ item }: Props) {
  const revealed = useStash((s) => s.revealedPassword);
  const reveal = useStash((s) => s.revealSelectedPassword);
  const copyUsername = useStash((s) => s.copySelectedUsername);
  const copyPassword = useStash((s) => s.copySelectedPassword);

  const body = parseBody(item.content);

  return (
    <div className="space-y-3">
      <Field label="Username">
        {body.username ? (
          <div className="flex items-center gap-2">
            <Value mono>{body.username}</Value>
            <Action onClick={() => void copyUsername()}>Copy</Action>
          </div>
        ) : (
          <Empty>No username stored</Empty>
        )}
      </Field>

      <Field label="Password">
        <div className="flex items-center gap-2">
          {revealed === null ? (
            <Value mono muted>
              ••••••••••
            </Value>
          ) : (
            <div className="min-w-0 flex-1 select-text truncate rounded-md border border-amber-500/35 bg-amber-500/[0.07] px-2.5 py-1.5 font-mono text-[13px] text-amber-300">
              {revealed || <span className="italic text-amber-400/70">(empty)</span>}
            </div>
          )}
          <button
            onClick={() => void reveal()}
            aria-label={revealed === null ? "Reveal password" : "Hide password"}
            title={revealed === null ? "Reveal (Ctrl+Shift+U)" : "Hide (Ctrl+Shift+U)"}
            className="flex h-8 w-8 shrink-0 items-center justify-center rounded-md border border-edge bg-panelalt text-zinc-400 transition-colors hover:text-accent"
          >
            <EyeIcon off={revealed !== null} />
          </button>
          <button
            onClick={() => void copyPassword()}
            title="Copy password (Ctrl+Shift+C)"
            className="h-8 shrink-0 rounded-md bg-accent px-2.5 text-[11px] font-semibold text-panel transition-opacity hover:opacity-90"
          >
            Copy
          </button>
        </div>
        {revealed !== null && (
          <p className="mt-1.5 text-[10.5px] leading-4 text-amber-400/80">
            Hides when you move the selection, leave the list, or reopen the panel.
          </p>
        )}
      </Field>

      {body.url && (
        <Field label="URL">
          <p className="select-text break-all text-[12px] text-accent">{body.url}</p>
        </Field>
      )}

      {body.notes && (
        <Field label="Notes">
          <div className="prose-stash select-text text-[12px] leading-[1.6] text-zinc-300">
            <ReactMarkdown>{body.notes}</ReactMarkdown>
          </div>
        </Field>
      )}
    </div>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div>
      <div className="mb-1.5 text-[10px] uppercase tracking-[0.08em] text-zinc-500">{label}</div>
      {children}
    </div>
  );
}

function Value({
  children,
  mono,
  muted,
}: {
  children: React.ReactNode;
  mono?: boolean;
  muted?: boolean;
}) {
  return (
    <div
      className={[
        "min-w-0 flex-1 select-text truncate rounded-md border border-edge bg-panelalt px-2.5 py-1.5 text-[13px]",
        mono ? "font-mono" : "",
        muted ? "tracking-[0.14em] text-zinc-500" : "text-zinc-200",
      ].join(" ")}
    >
      {children}
    </div>
  );
}

function Action({ children, onClick }: { children: React.ReactNode; onClick: () => void }) {
  return (
    <button
      onClick={onClick}
      className="h-8 shrink-0 rounded-md border border-edge bg-panelalt px-2.5 text-[11px] text-accent transition-colors hover:border-zinc-600"
    >
      {children}
    </button>
  );
}

function Empty({ children }: { children: React.ReactNode }) {
  return <p className="text-[12px] italic text-zinc-600">{children}</p>;
}

function EyeIcon({ off }: { off: boolean }) {
  return (
    <svg
      viewBox="0 0 24 24"
      className="h-[15px] w-[15px]"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      aria-hidden="true"
    >
      <path d="M2 12s4-7 10-7 10 7 10 7-4 7-10 7-10-7-10-7z" />
      {off ? <path d="M3 3l18 18" /> : <circle cx="12" cy="12" r="3" />}
    </svg>
  );
}

/**
 * The body is JSON written by `save_credential`. A mangled one degrades to an
 * empty card rather than throwing: the title still shows and the password is
 * still recoverable, which is what matters when something has gone wrong.
 */
function parseBody(content: string | null): {
  username?: string;
  url?: string;
  notes?: string;
} {
  if (!content) return {};
  try {
    const parsed: unknown = JSON.parse(content);
    return typeof parsed === "object" && parsed !== null ? parsed : {};
  } catch {
    return {};
  }
}
