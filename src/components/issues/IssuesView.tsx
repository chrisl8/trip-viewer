import { useEffect, useMemo, useRef, useState } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { deleteToTrash } from "../../ipc/issues";
import { scanFolder } from "../../ipc/scanner";
import { useStore } from "../../state/store";
import type { ScanError, ScanErrorKind } from "../../types/model";
import { basename, formatBytes, formatTimestamp } from "../../utils/format";
import { KIND_META, kindCounts } from "../../utils/issueKinds";

const GRID_COLS =
  "grid-cols-[minmax(14rem,2fr)_6rem_10rem_6rem_minmax(12rem,3fr)_7rem]";

type SortCol = "name" | "size" | "modified" | "type" | "reason";
type SortDir = "asc" | "desc";

const DEFAULT_DIR: Record<SortCol, SortDir> = {
  name: "asc",
  size: "desc",
  modified: "desc",
  type: "asc",
  reason: "asc",
};

function compareNullable<T>(
  a: T | null | undefined,
  b: T | null | undefined,
  dir: SortDir,
  cmp: (x: T, y: T) => number,
): number {
  const aNull = a == null;
  const bNull = b == null;
  if (aNull && bNull) return 0;
  if (aNull) return 1;
  if (bNull) return -1;
  const raw = cmp(a as T, b as T);
  return dir === "asc" ? raw : -raw;
}

function sortKey(e: ScanError, col: SortCol): string | number | null {
  switch (col) {
    case "name":
      return basename(e.path).toLocaleLowerCase();
    case "size":
      return e.sizeBytes;
    case "modified":
      return e.modifiedMs;
    case "type":
      return KIND_META[e.kind].label.toLocaleLowerCase();
    case "reason":
      return e.message.toLocaleLowerCase();
  }
}

export function IssuesView() {
  const scanErrors = useStore((s) => s.scanErrors);
  const setMainView = useStore((s) => s.setMainView);
  const setScanResult = useStore((s) => s.setScanResult);
  const setStatus = useStore((s) => s.setStatus);
  const removeScanErrors = useStore((s) => s.removeScanErrors);

  const [filterKind, setFilterKind] = useState<ScanErrorKind | null>(null);
  const [bulkConfirmOpen, setBulkConfirmOpen] = useState(false);

  // If the active filter has no rows left (all deleted), auto-clear it.
  useEffect(() => {
    if (filterKind && !scanErrors.some((e) => e.kind === filterKind)) {
      setFilterKind(null);
    }
  }, [scanErrors, filterKind]);

  const filtered = useMemo(
    () =>
      filterKind
        ? scanErrors.filter((e) => e.kind === filterKind)
        : scanErrors,
    [scanErrors, filterKind],
  );

  const breakdown = useMemo(() => kindCounts(scanErrors), [scanErrors]);

  const onDelete = async (path: string) => {
    try {
      await deleteToTrash(path);
      removeScanErrors([path]);
    } catch (e) {
      console.error("delete failed", path, e);
    }
  };

  const onBulkDelete = async () => {
    const paths = filtered.map((e) => e.path);
    setBulkConfirmOpen(false);
    const deleted: string[] = [];
    const failures: string[] = [];
    for (const p of paths) {
      try {
        await deleteToTrash(p);
        deleted.push(p);
      } catch (e) {
        console.error("bulk delete failed", p, e);
        failures.push(p);
      }
    }
    if (deleted.length > 0) removeScanErrors(deleted);
    if (failures.length > 0) {
      console.warn(
        `bulk delete: ${deleted.length} succeeded, ${failures.length} failed`,
      );
    }
  };

  const onRescan = async () => {
    const lastPath = localStorage.getItem("tripviewer:lastFolder");
    if (!lastPath) return;
    try {
      setStatus("loading");
      const result = await scanFolder(lastPath);
      setScanResult(result);
      // setScanResult flips mainView back to "player" — reopen the issues
      // view so the rescan stays in context.
      setMainView("issues");
    } catch (e) {
      console.error("rescan failed", e);
      setStatus("ready");
    }
  };

  return (
    <div className="flex h-full flex-col bg-neutral-950">
      <header className="flex items-center justify-between border-b border-neutral-800 px-4 py-3">
        <div className="flex items-baseline gap-3">
          <h2 className="text-sm font-semibold text-neutral-200">Scan issues</h2>
          <span className="text-xs text-neutral-500">
            {scanErrors.length === 0
              ? "no files flagged"
              : `${scanErrors.length} ${
                  scanErrors.length === 1 ? "file" : "files"
                } flagged`}
          </span>
        </div>
        <button
          onClick={onRescan}
          className="rounded-md px-2 py-1 text-xs text-neutral-400 hover:bg-neutral-800 hover:text-neutral-200"
          title="Rescan the current folder"
        >
          ⟳ Rescan
        </button>
      </header>

      {scanErrors.length === 0 ? (
        <div className="flex flex-1 items-center justify-center text-xs text-neutral-500">
          No files flagged in the current scan.
        </div>
      ) : (
        <>
          <FilterBar
            breakdown={breakdown}
            filterKind={filterKind}
            onFilterChange={setFilterKind}
            filteredCount={filtered.length}
            totalCount={scanErrors.length}
            onBulkDelete={() => setBulkConfirmOpen(true)}
          />
          <IssuesTable
            rows={filtered}
            onSetFilter={setFilterKind}
            onDelete={onDelete}
          />
        </>
      )}

      {bulkConfirmOpen && (
        <BulkDeleteConfirm
          rows={filtered}
          kind={filterKind}
          onConfirm={onBulkDelete}
          onCancel={() => setBulkConfirmOpen(false)}
        />
      )}
    </div>
  );
}

function FilterBar({
  breakdown,
  filterKind,
  onFilterChange,
  filteredCount,
  totalCount,
  onBulkDelete,
}: {
  breakdown: Array<{ kind: ScanErrorKind; count: number }>;
  filterKind: ScanErrorKind | null;
  onFilterChange: (k: ScanErrorKind | null) => void;
  filteredCount: number;
  totalCount: number;
  onBulkDelete: () => void;
}) {
  return (
    <div className="flex items-center justify-between gap-3 border-b border-neutral-800 bg-neutral-925 px-4 py-2 text-xs">
      <div className="flex items-center gap-2">
        <span className="text-neutral-500">Filter:</span>
        <button
          onClick={() => onFilterChange(null)}
          className={`rounded px-2 py-0.5 text-[11px] ${
            filterKind == null
              ? "bg-neutral-700 text-neutral-100"
              : "text-neutral-400 hover:bg-neutral-800 hover:text-neutral-200"
          }`}
        >
          all
        </button>
        {breakdown.map(({ kind, count }) => {
          const m = KIND_META[kind];
          const active = filterKind === kind;
          return (
            <button
              key={kind}
              onClick={() => onFilterChange(active ? null : kind)}
              className={`rounded px-2 py-0.5 text-[11px] font-semibold uppercase tracking-wide ${
                active ? m.className : `${m.className} opacity-60 hover:opacity-100`
              }`}
              title={
                active
                  ? `Clear ${m.label.toLowerCase()} filter`
                  : `Filter to ${m.label.toLowerCase()}`
              }
            >
              {m.label} {count}
            </button>
          );
        })}
      </div>
      {filterKind != null && (
        <div className="flex items-center gap-3">
          <span className="text-neutral-500">
            Showing {filteredCount} of {totalCount}
          </span>
          <button
            onClick={onBulkDelete}
            className="rounded-md bg-red-900 px-2 py-1 text-[11px] text-red-200 hover:bg-red-800"
          >
            Delete all {filteredCount} shown
          </button>
        </div>
      )}
    </div>
  );
}

function IssuesTable({
  rows,
  onSetFilter,
  onDelete,
}: {
  rows: ScanError[];
  onSetFilter: (k: ScanErrorKind | null) => void;
  onDelete: (path: string) => void | Promise<void>;
}) {
  const [sortCol, setSortCol] = useState<SortCol>("name");
  const [sortDir, setSortDir] = useState<SortDir>(DEFAULT_DIR.name);

  const onHeaderClick = (col: SortCol) => {
    if (col === sortCol) {
      setSortDir((d) => (d === "asc" ? "desc" : "asc"));
    } else {
      setSortCol(col);
      setSortDir(DEFAULT_DIR[col]);
    }
  };

  const sorted = useMemo(() => {
    const copy = rows.slice();
    copy.sort((a, b) => {
      const ka = sortKey(a, sortCol);
      const kb = sortKey(b, sortCol);
      return compareNullable(ka, kb, sortDir, (x, y) => {
        if (typeof x === "number" && typeof y === "number") return x - y;
        return String(x).localeCompare(String(y));
      });
    });
    return copy;
  }, [rows, sortCol, sortDir]);

  return (
    <div className="flex flex-1 flex-col overflow-hidden">
      <div
        className={`grid ${GRID_COLS} items-center gap-3 border-b border-neutral-800 bg-neutral-900 px-4 py-2 text-[11px] font-semibold uppercase tracking-wide text-neutral-400`}
      >
        <HeaderCell col="name" active={sortCol} dir={sortDir} onClick={onHeaderClick}>
          Name
        </HeaderCell>
        <HeaderCell
          col="size"
          active={sortCol}
          dir={sortDir}
          onClick={onHeaderClick}
          align="right"
        >
          Size
        </HeaderCell>
        <HeaderCell col="modified" active={sortCol} dir={sortDir} onClick={onHeaderClick}>
          Modified
        </HeaderCell>
        <HeaderCell col="type" active={sortCol} dir={sortDir} onClick={onHeaderClick}>
          Type
        </HeaderCell>
        <HeaderCell col="reason" active={sortCol} dir={sortDir} onClick={onHeaderClick}>
          Reason
        </HeaderCell>
        <div aria-hidden />
      </div>
      <div className="flex-1 overflow-y-auto">
        {sorted.map((e, i) => {
          const meta = KIND_META[e.kind];
          return (
            <div
              key={`${e.path}-${i}`}
              className={`grid ${GRID_COLS} items-center gap-3 border-b border-neutral-900 px-4 py-2 text-xs hover:bg-neutral-900`}
            >
              <div className="truncate text-neutral-200" title={e.path}>
                {basename(e.path)}
              </div>
              <div className="text-right tabular-nums text-neutral-400">
                {formatBytes(e.sizeBytes)}
              </div>
              <div className="tabular-nums text-neutral-400">
                {formatTimestamp(e.modifiedMs)}
              </div>
              <div>
                <button
                  onClick={() => onSetFilter(e.kind)}
                  className={`rounded px-2 py-0.5 text-[10px] font-semibold uppercase tracking-wide ${meta.className} hover:brightness-125`}
                  title={`Filter to ${meta.label.toLowerCase()}`}
                >
                  {meta.label}
                </button>
              </div>
              <div className="truncate text-neutral-300" title={e.message}>
                {e.message}
              </div>
              <RowActions path={e.path} onDelete={() => onDelete(e.path)} />
            </div>
          );
        })}
      </div>
    </div>
  );
}

function RowActions({
  path,
  onDelete,
}: {
  path: string;
  onDelete: () => void | Promise<void>;
}) {
  const [copied, setCopied] = useState(false);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const copyTimer = useRef<number | null>(null);
  const confirmTimer = useRef<number | null>(null);

  useEffect(
    () => () => {
      if (copyTimer.current != null) window.clearTimeout(copyTimer.current);
      if (confirmTimer.current != null) window.clearTimeout(confirmTimer.current);
    },
    [],
  );

  const onReveal = async (e: React.MouseEvent) => {
    e.stopPropagation();
    try {
      await revealItemInDir(path);
    } catch (err) {
      console.error("revealItemInDir failed", path, err);
    }
  };

  const onCopy = async (e: React.MouseEvent) => {
    e.stopPropagation();
    try {
      await navigator.clipboard.writeText(path);
      setCopied(true);
      if (copyTimer.current != null) window.clearTimeout(copyTimer.current);
      copyTimer.current = window.setTimeout(() => setCopied(false), 1500);
    } catch (err) {
      console.error("clipboard.writeText failed", err);
    }
  };

  const onDeleteClick = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (!confirmingDelete) {
      // First click — arm the confirm. Auto-disarm after 5s of inactivity.
      setConfirmingDelete(true);
      if (confirmTimer.current != null) window.clearTimeout(confirmTimer.current);
      confirmTimer.current = window.setTimeout(
        () => setConfirmingDelete(false),
        5000,
      );
      return;
    }
    // Second click — actually delete.
    if (confirmTimer.current != null) window.clearTimeout(confirmTimer.current);
    setConfirmingDelete(false);
    await onDelete();
  };

  const btn =
    "rounded-md px-1.5 py-0.5 text-[11px] text-neutral-400 hover:bg-neutral-800 hover:text-neutral-200";
  const confirmBtn =
    "rounded-md bg-red-900 px-1.5 py-0.5 text-[11px] text-red-200 hover:bg-red-800";

  return (
    <div className="flex items-center justify-end gap-1">
      <button
        type="button"
        onClick={onReveal}
        className={btn}
        title="Reveal in file manager"
        aria-label="Reveal in file manager"
      >
        📂
      </button>
      <button
        type="button"
        onClick={onCopy}
        className={btn}
        title={copied ? "Copied!" : "Copy full path"}
        aria-label="Copy full path"
      >
        {copied ? "✓" : "⧉"}
      </button>
      <button
        type="button"
        onClick={onDeleteClick}
        className={confirmingDelete ? confirmBtn : btn}
        title={
          confirmingDelete
            ? "Click again to confirm — moves to recycle bin"
            : "Delete (moves to recycle bin)"
        }
        aria-label={confirmingDelete ? "Confirm delete" : "Delete"}
      >
        {confirmingDelete ? "Confirm?" : "🗑"}
      </button>
    </div>
  );
}

function HeaderCell({
  col,
  active,
  dir,
  onClick,
  align,
  children,
}: {
  col: SortCol;
  active: SortCol;
  dir: SortDir;
  onClick: (c: SortCol) => void;
  align?: "left" | "right";
  children: React.ReactNode;
}) {
  const isActive = active === col;
  const arrow = isActive ? (dir === "asc" ? " ↑" : " ↓") : "";
  return (
    <button
      type="button"
      onClick={() => onClick(col)}
      className={`flex items-center gap-1 text-left uppercase tracking-wide hover:text-neutral-200 ${
        align === "right" ? "justify-end text-right" : ""
      } ${isActive ? "text-neutral-200" : ""}`}
    >
      <span>
        {children}
        {arrow}
      </span>
    </button>
  );
}

function BulkDeleteConfirm({
  rows,
  kind,
  onConfirm,
  onCancel,
}: {
  rows: ScanError[];
  kind: ScanErrorKind | null;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const label = kind ? KIND_META[kind].label.toLowerCase() : "files";
  const preview = rows.slice(0, 5);
  const rest = rows.length - preview.length;
  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60"
      onClick={onCancel}
    >
      <div
        className="w-full max-w-lg rounded-lg border border-neutral-700 bg-neutral-900 p-6"
        onClick={(e) => e.stopPropagation()}
      >
        <h2 className="mb-2 text-sm font-semibold text-neutral-200">
          Delete {rows.length} {label}?
        </h2>
        <p className="mb-3 text-xs text-neutral-400">
          These files will be moved to the OS recycle bin. You can restore
          them from there if needed.
        </p>
        <ul className="mb-4 max-h-48 overflow-y-auto rounded-md bg-neutral-950 p-2 font-mono text-[11px] text-neutral-400">
          {preview.map((e) => (
            <li key={e.path} className="truncate" title={e.path}>
              {basename(e.path)}
            </li>
          ))}
          {rest > 0 && (
            <li className="text-neutral-500">…and {rest} more</li>
          )}
        </ul>
        <div className="flex justify-end gap-2">
          <button
            onClick={onCancel}
            className="rounded-md border border-neutral-700 px-3 py-1.5 text-xs font-medium text-neutral-300 hover:bg-neutral-800"
          >
            Cancel
          </button>
          <button
            onClick={onConfirm}
            className="rounded-md bg-red-700 px-3 py-1.5 text-xs font-medium text-white hover:bg-red-600"
          >
            Delete {rows.length}
          </button>
        </div>
      </div>
    </div>
  );
}
