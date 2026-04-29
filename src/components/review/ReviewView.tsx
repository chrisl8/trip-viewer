import { useEffect, useMemo, useRef, useState } from "react";
import clsx from "clsx";
import {
  addUserTag,
  deleteSegmentsToTrash,
  getAllTags,
  removeUserTag,
  type DeleteReport,
  type UserApplicableTag,
} from "../../ipc/tags";
import { useStore } from "../../state/store";
import type { Tag, TagCategory } from "../../types/model";
import {
  CATEGORY_COLORS,
  categoryForTag,
  displayNameForTag,
} from "../../utils/tagColors";
import { TagBadge } from "./TagBadge";

function TagMenu({
  tags,
  onPick,
}: {
  tags: UserApplicableTag[];
  onPick: (name: string) => void;
}) {
  return (
    <div className="absolute right-0 top-full z-20 mt-1 min-w-[10rem] rounded-md border border-neutral-700 bg-neutral-900 py-1 shadow-lg">
      {tags.map((t) => {
        const colors = CATEGORY_COLORS[t.category];
        return (
          <button
            key={t.name}
            onClick={() => onPick(t.name)}
            title={t.description}
            className="flex w-full items-center gap-2 px-3 py-1 text-left text-sm text-neutral-200 hover:bg-neutral-800"
          >
            <span
              className={clsx("h-2 w-2 rounded-full", colors.band)}
              aria-hidden
            />
            {t.displayName}
          </button>
        );
      })}
    </div>
  );
}

interface Row {
  segmentId: string;
  tripId: string;
  tripStart: string;
  segmentStart: string;
  durationS: number;
  masterPath: string;
  tags: Tag[];
}

const ALL_CATEGORIES: TagCategory[] = [
  "event",
  "motion",
  "audio",
  "quality",
  "user",
];

export function ReviewView() {
  const setMainView = useStore((s) => s.setMainView);
  const trips = useStore((s) => s.trips);
  const selectTrip = useStore((s) => s.selectTrip);
  const setActiveSegmentId = useStore((s) => s.setActiveSegmentId);
  const refreshTripTagCounts = useStore((s) => s.refreshTripTagCounts);
  const selectedTripId = useStore((s) => s.selectedTripId);
  const refreshTripTags = useStore((s) => s.refreshTripTags);

  const [allTags, setAllTags] = useState<Tag[]>([]);
  const [selectedSegments, setSelectedSegments] = useState<Set<string>>(
    new Set(),
  );
  const [hideKept, setHideKept] = useState(true);
  const [tagFilter, setTagFilter] = useState<Set<string>>(new Set());
  const [categoryFilter, setCategoryFilter] = useState<Set<TagCategory>>(
    new Set(),
  );
  const [sortKey, setSortKey] = useState<"start" | "duration" | "tags">(
    "start",
  );
  const [sortDir, setSortDir] = useState<"asc" | "desc">("desc");
  const [busy, setBusy] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [deleteReport, setDeleteReport] = useState<DeleteReport | null>(null);
  const [openMenu, setOpenMenu] = useState<"add" | "remove" | null>(null);
  const menuRootRef = useRef<HTMLDivElement>(null);
  const userApplicable = useStore((s) => s.userApplicableTags);
  const placesById = useStore((s) => s.placesById);

  // Close any open tag menu when the user clicks outside of the action
  // row. Pointerdown so we race ahead of any button inside a re-render.
  useEffect(() => {
    if (!openMenu) return;
    function onDocumentPointerDown(e: PointerEvent) {
      if (
        menuRootRef.current &&
        !menuRootRef.current.contains(e.target as Node)
      ) {
        setOpenMenu(null);
      }
    }
    document.addEventListener("pointerdown", onDocumentPointerDown);
    return () =>
      document.removeEventListener("pointerdown", onDocumentPointerDown);
  }, [openMenu]);

  async function refetchTags() {
    const tags = await getAllTags();
    setAllTags(tags);
  }

  useEffect(() => {
    refetchTags().catch((e) => console.error("getAllTags failed", e));
  }, []);

  // Build review rows by joining in-memory trips with fetched tags.
  const rows: Row[] = useMemo(() => {
    const bySeg = new Map<string, Tag[]>();
    for (const tag of allTags) {
      if (!tag.segmentId) continue;
      const list = bySeg.get(tag.segmentId);
      if (list) list.push(tag);
      else bySeg.set(tag.segmentId, [tag]);
    }
    const out: Row[] = [];
    for (const trip of trips) {
      for (const seg of trip.segments) {
        const tags = bySeg.get(seg.id) ?? [];
        out.push({
          segmentId: seg.id,
          tripId: trip.id,
          tripStart: trip.startTime,
          segmentStart: seg.startTime,
          durationS: seg.durationS,
          masterPath: seg.channels[0]?.filePath ?? "",
          tags,
        });
      }
    }
    return out;
  }, [trips, allTags]);

  const filtered = useMemo(() => {
    let result = rows;
    if (hideKept) {
      result = result.filter((r) => !r.tags.some((t) => t.name === "keep"));
    }
    if (tagFilter.size > 0) {
      result = result.filter((r) =>
        r.tags.some((t) => tagFilter.has(t.name)),
      );
    }
    if (categoryFilter.size > 0) {
      result = result.filter((r) =>
        r.tags.some((t) => categoryFilter.has(t.category)),
      );
    }
    result = [...result].sort((a, b) => {
      let cmp = 0;
      if (sortKey === "start") {
        cmp = a.segmentStart.localeCompare(b.segmentStart);
      } else if (sortKey === "duration") {
        cmp = a.durationS - b.durationS;
      } else {
        cmp = a.tags.length - b.tags.length;
      }
      return sortDir === "asc" ? cmp : -cmp;
    });
    return result;
  }, [rows, hideKept, tagFilter, categoryFilter, sortKey, sortDir]);

  const availableTagNames = useMemo(() => {
    const set = new Set<string>();
    for (const t of allTags) set.add(t.name);
    return Array.from(set).sort();
  }, [allTags]);

  function toggleSort(key: "start" | "duration" | "tags") {
    if (sortKey === key) {
      setSortDir(sortDir === "asc" ? "desc" : "asc");
    } else {
      setSortKey(key);
      setSortDir("desc");
    }
  }

  function toggleRow(id: string) {
    const next = new Set(selectedSegments);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    setSelectedSegments(next);
  }

  function toggleAll() {
    if (selectedSegments.size === filtered.length) {
      setSelectedSegments(new Set());
    } else {
      setSelectedSegments(new Set(filtered.map((r) => r.segmentId)));
    }
  }

  async function onBulkTagChange(name: string, mode: "add" | "remove") {
    // Same intersection rule as delete: bulk-tag changes apply only
    // to visible-and-selected rows. Tagging is reversible but the
    // surprise of "I changed tags on rows I couldn't see" is the
    // same trap, so apply the same safety here.
    if (filteredSelectedCount === 0) return;
    setBusy(true);
    try {
      const ids = Array.from(filteredSelectedIds);
      if (mode === "add") {
        await addUserTag(ids, name);
      } else {
        await removeUserTag(ids, name);
      }
      await refetchTags();
      await refreshTripTagCounts();
      if (selectedTripId) await refreshTripTags(selectedTripId);
      // Drop only the segments we just tagged from the selection;
      // hidden selections stay so the user can clear the filter and
      // act on them.
      setSelectedSegments((prev) => {
        const next = new Set(prev);
        for (const id of filteredSelectedIds) next.delete(id);
        return next;
      });
    } catch (e) {
      console.error(`bulk ${mode} tag failed`, e);
    } finally {
      setBusy(false);
    }
  }

  const selectedKeptCount = useMemo(
    () =>
      filtered.filter(
        (r) =>
          selectedSegments.has(r.segmentId) &&
          r.tags.some((t) => t.name === "keep"),
      ).length,
    [filtered, selectedSegments],
  );

  // Bulk actions only operate on the intersection of selected ∩
  // filtered. Without this, the classic trap is: select 100, filter
  // down to 5, click Delete → all 100 get deleted including the 95
  // hidden ones. With the intersection, the user can't destructively
  // act on rows they can't see; if they want the full set deleted
  // they must clear the filter first.
  const filteredSelectedIds = useMemo(() => {
    const ids = new Set<string>();
    for (const row of filtered) {
      if (selectedSegments.has(row.segmentId)) ids.add(row.segmentId);
    }
    return ids;
  }, [filtered, selectedSegments]);
  const filteredSelectedCount = filteredSelectedIds.size;
  const hiddenSelectedCount = selectedSegments.size - filteredSelectedCount;

  // Build the segmentId -> [channel paths] map from in-memory trips so
  // the backend doesn't have to store channel lists in the DB. Only
  // includes the visible-and-selected segments, since those are the
  // only ones a bulk action can touch.
  const pathsBySegment = useMemo(() => {
    const map: Record<string, string[]> = {};
    for (const trip of trips) {
      for (const seg of trip.segments) {
        if (!filteredSelectedIds.has(seg.id)) continue;
        map[seg.id] = seg.channels.map((c) => c.filePath).filter(Boolean);
      }
    }
    return map;
  }, [trips, filteredSelectedIds]);

  async function onConfirmDelete() {
    if (filteredSelectedCount === 0) return;
    setBusy(true);
    setConfirmDelete(false);
    try {
      const report = await deleteSegmentsToTrash(
        Array.from(filteredSelectedIds),
        pathsBySegment,
      );
      setDeleteReport(report);
      await refetchTags();
      await refreshTripTagCounts();
      if (selectedTripId) await refreshTripTags(selectedTripId);
      // Drop only the segments we just deleted from the selection;
      // segments hidden by the filter (and thus skipped) stay
      // selected so the user can clear the filter and act on them.
      setSelectedSegments((prev) => {
        const next = new Set(prev);
        for (const id of filteredSelectedIds) next.delete(id);
        return next;
      });
    } catch (e) {
      console.error("deleteSegmentsToTrash failed", e);
    } finally {
      setBusy(false);
    }
  }

  async function onOpenSegment(row: Row) {
    selectTrip(row.tripId);
    // Next tick so trip selection settles before we seek.
    setTimeout(() => setActiveSegmentId(row.segmentId), 0);
    setMainView("player");
  }

  function fmtDate(iso: string): string {
    const d = new Date(iso);
    return (
      d.toLocaleDateString(undefined, {
        month: "short",
        day: "numeric",
      }) +
      " " +
      d.toLocaleTimeString(undefined, {
        hour: "numeric",
        minute: "2-digit",
      })
    );
  }

  function fmtDuration(s: number): string {
    const m = Math.floor(s / 60);
    const sec = Math.round(s % 60);
    return `${m}m ${sec}s`;
  }

  const selectedCount = selectedSegments.size;

  return (
    <div className="relative flex h-full flex-col overflow-hidden bg-neutral-950 text-neutral-100">
      <header className="flex items-center justify-between border-b border-neutral-800 px-4 py-3">
        <div>
          <h1 className="text-lg font-semibold">Review</h1>
          <p className="text-xs text-neutral-500">
            {filtered.length} of {rows.length} segments
            {selectedCount > 0 && (
              <span> · {selectedCount} selected</span>
            )}
            {hiddenSelectedCount > 0 && (
              <span
                className="text-amber-400"
                title="These segments are part of your selection but hidden by the current filter. Bulk actions (delete, tag) only affect visible-and-selected rows; clear the filter to act on hidden ones."
              >
                {" "}
                ({hiddenSelectedCount} hidden by filter)
              </span>
            )}
          </p>
          <p className="text-xs text-neutral-500">
            Click one or more pills below to filter the segments.
          </p>
        </div>
        <div className="flex items-center gap-4">
          <label
            className="flex items-center gap-1.5 whitespace-nowrap text-xs text-neutral-400"
            title="When checked, segments tagged 'keep' don't appear in the table below."
          >
            <input
              type="checkbox"
              checked={hideKept}
              onChange={(e) => setHideKept(e.target.checked)}
            />
            Hide &lsquo;keep&rsquo; tag
          </label>
          <button
            onClick={() => setMainView("player")}
            className="rounded-md border border-neutral-700 px-3 py-1 text-sm text-neutral-300 hover:bg-neutral-800"
          >
            Close
          </button>
        </div>
      </header>

      <div className="flex items-center gap-3 border-b border-neutral-800 px-4 py-2">
        <div className="flex items-center gap-1">
          {ALL_CATEGORIES.map((cat) => {
            const active = categoryFilter.has(cat);
            const colors = CATEGORY_COLORS[cat];
            return (
              <button
                key={cat}
                onClick={() => {
                  const next = new Set(categoryFilter);
                  if (active) next.delete(cat);
                  else next.add(cat);
                  setCategoryFilter(next);
                }}
                className={clsx(
                  "rounded-full px-2 py-0.5 text-[11px] font-medium uppercase tracking-wide",
                  active ? colors.bg : "bg-neutral-900",
                  active ? colors.text : "text-neutral-500",
                  "hover:brightness-125",
                )}
              >
                {cat}
              </button>
            );
          })}
        </div>

        {availableTagNames.length > 0 && (
          <div className="flex flex-wrap items-center gap-1">
            {availableTagNames.map((name) => {
              const active = tagFilter.has(name);
              const colors = CATEGORY_COLORS[categoryForTag(name)];
              return (
                <button
                  key={name}
                  onClick={() => {
                    const next = new Set(tagFilter);
                    if (active) next.delete(name);
                    else next.add(name);
                    setTagFilter(next);
                  }}
                  className={clsx(
                    "rounded px-1.5 py-0.5 text-[11px]",
                    active
                      ? clsx(colors.bg, colors.text, "ring-1 ring-inset ring-white/10")
                      : "bg-neutral-900 text-neutral-500",
                    "hover:brightness-125",
                  )}
                >
                  {displayNameForTag(name, placesById)}
                </button>
              );
            })}
          </div>
        )}

        <div className="relative ml-auto flex gap-2" ref={menuRootRef}>
          <div className="relative">
            <button
              onClick={() =>
                setOpenMenu(openMenu === "add" ? null : "add")
              }
              disabled={filteredSelectedCount === 0 || busy || userApplicable.length === 0}
              title={
                filteredSelectedCount === 0 && hiddenSelectedCount > 0
                  ? "All selected segments are hidden by your current filter — clear the filter to act on them."
                  : undefined
              }
              className={clsx(
                "rounded-md px-3 py-1 text-sm",
                filteredSelectedCount === 0 || busy
                  ? "cursor-not-allowed bg-neutral-800 text-neutral-500"
                  : "bg-emerald-700 text-white hover:bg-emerald-600",
              )}
            >
              Add tag ▾
            </button>
            {openMenu === "add" && (
              <TagMenu
                tags={userApplicable}
                onPick={(name) => {
                  setOpenMenu(null);
                  void onBulkTagChange(name, "add");
                }}
              />
            )}
          </div>
          <div className="relative">
            <button
              onClick={() =>
                setOpenMenu(openMenu === "remove" ? null : "remove")
              }
              disabled={filteredSelectedCount === 0 || busy || userApplicable.length === 0}
              title={
                filteredSelectedCount === 0 && hiddenSelectedCount > 0
                  ? "All selected segments are hidden by your current filter — clear the filter to act on them."
                  : undefined
              }
              className={clsx(
                "rounded-md border px-3 py-1 text-sm",
                filteredSelectedCount === 0 || busy
                  ? "cursor-not-allowed border-neutral-800 text-neutral-500"
                  : "border-neutral-700 text-neutral-300 hover:bg-neutral-800",
              )}
            >
              Remove tag ▾
            </button>
            {openMenu === "remove" && (
              <TagMenu
                tags={userApplicable}
                onPick={(name) => {
                  setOpenMenu(null);
                  void onBulkTagChange(name, "remove");
                }}
              />
            )}
          </div>
          <button
            onClick={() => setConfirmDelete(true)}
            disabled={filteredSelectedCount === 0 || busy}
            title={
              filteredSelectedCount === 0 && hiddenSelectedCount > 0
                ? "All selected segments are hidden by your current filter — clear the filter to delete them."
                : undefined
            }
            className={clsx(
              "rounded-md px-3 py-1 text-sm",
              filteredSelectedCount === 0 || busy
                ? "cursor-not-allowed bg-neutral-800 text-neutral-500"
                : "bg-red-700 text-white hover:bg-red-600",
            )}
          >
            Delete to trash
          </button>
        </div>
      </div>

      <div className="flex-1 overflow-auto">
        <table className="w-full text-sm">
          <thead className="sticky top-0 bg-neutral-900 text-xs uppercase tracking-wide text-neutral-400">
            <tr>
              <th className="w-8 px-2 py-2 text-left">
                <input
                  type="checkbox"
                  checked={
                    filtered.length > 0 &&
                    selectedSegments.size === filtered.length
                  }
                  onChange={toggleAll}
                />
              </th>
              <th
                onClick={() => toggleSort("start")}
                className="cursor-pointer px-2 py-2 text-left"
              >
                Time {sortKey === "start" && (sortDir === "asc" ? "↑" : "↓")}
              </th>
              <th
                onClick={() => toggleSort("duration")}
                className="cursor-pointer px-2 py-2 text-left"
              >
                Dur {sortKey === "duration" && (sortDir === "asc" ? "↑" : "↓")}
              </th>
              <th
                onClick={() => toggleSort("tags")}
                className="cursor-pointer px-2 py-2 text-left"
              >
                Tags {sortKey === "tags" && (sortDir === "asc" ? "↑" : "↓")}
              </th>
              <th className="px-2 py-2 text-left">Path</th>
            </tr>
          </thead>
          <tbody>
            {filtered.map((row) => {
              const selected = selectedSegments.has(row.segmentId);
              return (
                <tr
                  key={row.segmentId}
                  className={clsx(
                    "border-t border-neutral-900",
                    selected ? "bg-sky-950/30" : "hover:bg-neutral-900",
                  )}
                >
                  <td className="px-2 py-1">
                    <input
                      type="checkbox"
                      checked={selected}
                      onChange={() => toggleRow(row.segmentId)}
                    />
                  </td>
                  <td className="px-2 py-1">
                    <button
                      onClick={() => void onOpenSegment(row)}
                      className="text-neutral-200 hover:text-sky-300"
                    >
                      {fmtDate(row.segmentStart)}
                    </button>
                  </td>
                  <td className="px-2 py-1 text-neutral-400">
                    {fmtDuration(row.durationS)}
                  </td>
                  <td className="px-2 py-1">
                    <div className="flex flex-wrap gap-1">
                      {row.tags.map((tag) => (
                        <TagBadge
                          key={tag.id ?? `${tag.name}-${tag.source}`}
                          tag={tag}
                          compact
                        />
                      ))}
                    </div>
                  </td>
                  <td className="truncate px-2 py-1 text-xs text-neutral-500">
                    {row.masterPath}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
        {filtered.length === 0 && (
          <div className="p-8 text-center text-sm text-neutral-500">
            {rows.length === 0
              ? "No segments loaded. Run a scan first."
              : "No segments match the current filters."}
          </div>
        )}
      </div>

      {confirmDelete && (
        <div className="absolute inset-0 z-30 flex items-center justify-center bg-black/60">
          <div className="w-96 rounded-md border border-neutral-700 bg-neutral-900 p-4">
            <h2 className="text-base font-semibold">Delete {filteredSelectedCount} {filteredSelectedCount === 1 ? "segment" : "segments"}?</h2>
            <p className="mt-2 text-sm text-neutral-400">
              Files move to the OS trash and can be recovered from there.
              Tags and scan history for these segments are removed from the
              library.
            </p>
            {hiddenSelectedCount > 0 && (
              <p className="mt-2 rounded-md bg-sky-950 px-2 py-1 text-xs text-sky-300">
                {hiddenSelectedCount} other selected{" "}
                {hiddenSelectedCount === 1 ? "segment is" : "segments are"}{" "}
                hidden by the current filter and won&apos;t be touched.
                Clear the filter first if you want them included.
              </p>
            )}
            {selectedKeptCount > 0 && (
              <p className="mt-2 rounded-md bg-amber-950 px-2 py-1 text-xs text-amber-300">
                {selectedKeptCount} of the selected{" "}
                {selectedKeptCount === 1 ? "segment is" : "segments are"}{" "}
                marked &lsquo;keep&rsquo;. Delete anyway?
              </p>
            )}
            <div className="mt-4 flex justify-end gap-2">
              <button
                onClick={() => setConfirmDelete(false)}
                className="rounded-md border border-neutral-700 px-3 py-1 text-sm text-neutral-300 hover:bg-neutral-800"
              >
                Cancel
              </button>
              <button
                onClick={() => void onConfirmDelete()}
                className="rounded-md bg-red-700 px-3 py-1 text-sm text-white hover:bg-red-600"
              >
                Move to trash
              </button>
            </div>
          </div>
        </div>
      )}

      {deleteReport && (
        <div className="absolute bottom-4 right-4 z-30 max-w-sm rounded-md border border-neutral-700 bg-neutral-900 p-3 text-sm shadow-lg">
          <div className="flex items-start justify-between gap-2">
            <div>
              <div className="font-medium">
                Removed {deleteReport.segmentsRemoved} segments,{" "}
                {deleteReport.filesTrashed} files trashed
              </div>
              {deleteReport.failures.length > 0 && (
                <div className="mt-1 text-xs text-red-300">
                  {deleteReport.failures.length} file
                  {deleteReport.failures.length === 1 ? "" : "s"} failed —
                  see console
                </div>
              )}
            </div>
            <button
              onClick={() => setDeleteReport(null)}
              className="shrink-0 text-neutral-400 hover:text-neutral-200"
            >
              ×
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
