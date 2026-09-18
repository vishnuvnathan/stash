import { useState } from "react";
import { exportData, importData } from "../lib/ipc";
import type { ExportFormat, ExportReport, ImportReport } from "../lib/types";
import { useStash } from "../store/useStash";

/**
 * Export and import, in the settings panel.
 *
 * The answer to "what happens when I want to leave". Everything else in Stash
 * is local-only and DPAPI-sealed to one Windows account, which is exactly what
 * makes a way out necessary rather than optional.
 *
 * The passphrase field only appears for the encrypted format, and the
 * plaintext-secrets checkbox only for JSON — Markdown never carries passwords
 * and an encrypted backup always does, so in both those cases there is nothing
 * to decide.
 */
export function BackupSection() {
  const refresh = useStash((s) => s.refresh);

  const [format, setFormat] = useState<ExportFormat>("encrypted");
  const [includeSecrets, setIncludeSecrets] = useState(false);
  const [passphrase, setPassphrase] = useState("");
  const [busy, setBusy] = useState<null | "export" | "import">(null);
  const [result, setResult] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  const needsPassphrase = format === "encrypted";
  const canChooseSecrets = format === "json";

  // A cancelled dialog is a decision, not a failure, and must not surface as a
  // red error message.
  const cancelled = (e: unknown) => String(e).includes("cancelled");

  async function runExport() {
    setBusy("export");
    setResult(null);
    setProblem(null);
    try {
      const r: ExportReport = await exportData(format, includeSecrets, passphrase);
      setPassphrase("");
      setResult(
        `Exported ${r.items} items (${formatBytes(r.bytes)})${
          r.includesSecrets ? ", passwords included" : ""
        } to ${r.path}`,
      );
    } catch (e) {
      if (!cancelled(e)) setProblem(String(e));
    } finally {
      setBusy(null);
    }
  }

  async function runImport() {
    setBusy("import");
    setResult(null);
    setProblem(null);
    try {
      const r: ImportReport = await importData(passphrase);
      setPassphrase("");
      setResult(describeImport(r));
      void refresh();
    } catch (e) {
      if (!cancelled(e)) setProblem(String(e));
    } finally {
      setBusy(null);
    }
  }

  return (
    <div className="mt-5 border-t border-edge pt-3">
      <h2 className="text-[13px] text-zinc-200">Backup and restore</h2>
      <p className="mt-1 text-[11px] leading-4 text-zinc-600">
        Your data lives only on this machine, and passwords are tied to this Windows account.
        An export is the only way to move them anywhere else.
      </p>

      <div className="mt-2.5 flex flex-wrap gap-1.5">
        <FormatButton id="encrypted" active={format} onPick={setFormat}>
          Encrypted backup
        </FormatButton>
        <FormatButton id="json" active={format} onPick={setFormat}>
          JSON
        </FormatButton>
        <FormatButton id="markdown" active={format} onPick={setFormat}>
          Markdown
        </FormatButton>
      </div>

      <p className="mt-1.5 text-[11px] leading-4 text-zinc-500">{describeFormat(format)}</p>

      {canChooseSecrets && (
        <label className="mt-2 flex cursor-pointer items-start gap-2">
          <input
            type="checkbox"
            checked={includeSecrets}
            onChange={(e) => setIncludeSecrets(e.target.checked)}
            className="mt-0.5 h-3.5 w-3.5 shrink-0 accent-[#7aa2f7]"
          />
          <span className="text-[11px] leading-4 text-zinc-400">
            Include passwords in plain text
            {includeSecrets && (
              <span className="text-amber-400/90">
                {" "}
                — anyone who opens the file can read them
              </span>
            )}
          </span>
        </label>
      )}

      {needsPassphrase && (
        <input
          type="password"
          value={passphrase}
          onChange={(e) => setPassphrase(e.target.value)}
          onKeyDown={(e) => e.stopPropagation()}
          placeholder="Passphrase — you will need this to restore"
          spellCheck={false}
          className="mt-2 w-full rounded border border-edge bg-panelalt px-2 py-1.5 text-[12px] text-zinc-100 outline-none placeholder:text-zinc-600 focus:border-accent/60"
        />
      )}

      <div className="mt-2.5 flex gap-2">
        <button
          onClick={() => void runExport()}
          disabled={busy !== null || (needsPassphrase && passphrase.trim() === "")}
          className="rounded border border-edge px-2.5 py-1 text-[11px] text-zinc-300 hover:text-zinc-100 disabled:opacity-40"
        >
          {busy === "export" ? "Exporting…" : "Export…"}
        </button>
        <button
          onClick={() => void runImport()}
          disabled={busy !== null}
          className="rounded border border-edge px-2.5 py-1 text-[11px] text-zinc-300 hover:text-zinc-100 disabled:opacity-40"
        >
          {busy === "import" ? "Importing…" : "Import…"}
        </button>
      </div>

      <p className="mt-1.5 text-[11px] leading-4 text-zinc-600">
        Importing merges — it never deletes or overwrites. Clips you already have are skipped,
        so restoring the same file twice is safe. For an encrypted backup, type its passphrase
        above first.
      </p>

      {result && (
        <p className="mt-2 break-all rounded border border-edge bg-panelalt px-2.5 py-2 text-[11px] leading-4 text-zinc-300">
          {result}
        </p>
      )}
      {problem && (
        <p className="mt-2 rounded border border-red-900/50 bg-red-950/40 px-2.5 py-2 text-[11px] leading-4 text-red-300">
          {problem}
        </p>
      )}
    </div>
  );
}

function FormatButton({
  id,
  active,
  onPick,
  children,
}: {
  id: ExportFormat;
  active: ExportFormat;
  onPick: (f: ExportFormat) => void;
  children: string;
}) {
  const on = id === active;
  return (
    <button
      onClick={() => onPick(id)}
      className={[
        "rounded-full border px-2.5 py-0.5 text-[11px] transition-colors",
        on
          ? "border-accent/60 bg-accent/15 text-zinc-100"
          : "border-edge text-zinc-500 hover:text-zinc-300",
      ].join(" ")}
    >
      {children}
    </button>
  );
}

function describeFormat(format: ExportFormat): string {
  switch (format) {
    case "encrypted":
      return "Everything, including passwords, sealed with a passphrase. The safe way to carry credentials to another machine. Restorable.";
    case "json":
      return "Everything, restorable. Passwords are left out unless you ask for them below.";
    case "markdown":
      return "Readable notes and clips for printing or pasting elsewhere. Never includes passwords, and cannot be imported back.";
  }
}

function describeImport(r: ImportReport): string {
  const parts = [`Imported ${r.imported} items`];
  if (r.skipped > 0) parts.push(`${r.skipped} already here`);
  if (r.foldersCreated > 0) parts.push(`${r.foldersCreated} folders created`);
  if (r.secretsRestored > 0) parts.push(`${r.secretsRestored} passwords restored`);
  if (r.secretsAbsent) parts.push("this archive carried no passwords");
  if (r.failed > 0) parts.push(`${r.failed} could not be read`);
  return `${parts.join(" · ")}.`;
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}
