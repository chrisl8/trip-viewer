import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useStore } from "../../state/store";
import { usePreferences } from "../../settings/preferences";
import type { GpsPoint, Segment, TagCategory, Trip } from "../../types/model";
import { CATEGORY_COLORS } from "../../utils/tagColors";
import { computeTripTime, tripTotalDuration } from "../../utils/tripTime";

import { adjacentBookmark, bookmarkTimeLabel } from "./bookmarks";
import { useBookmarks } from "./useBookmarks";

interface Props {
  onSeekTripTime: (tripTime: number) => void;
}

const HEIGHT = 62;
const SEG_BAR_H = 8;
/** One band per unique tag category present on the segment, stacked. */
const TAG_BAND_H = 1.6;
const MAX_BANDS = 3;
const TAG_BANDS_AREA_H = TAG_BAND_H * MAX_BANDS;
const SPEED_AREA_H = HEIGHT - SEG_BAR_H - TAG_BANDS_AREA_H - 2;

interface SpeedPoint {
  x: number;
  speed: number;
}

interface HoverSpeed {
  x: number;
  speed: number | null;
}

function niceSpeedScale(maxValue: number): { max: number; step: number } {
  // Aim for roughly four horizontal bands while keeping labels on familiar
  // 1 / 2 / 2.5 / 5 × powers-of-ten values.
  const safeMax = Math.max(1, maxValue);
  const roughStep = safeMax / 4;
  const magnitude = 10 ** Math.floor(Math.log10(roughStep));
  const normalized = roughStep / magnitude;
  const nice =
    normalized <= 1
      ? 1
      : normalized <= 2
        ? 2
        : normalized <= 2.5
          ? 2.5
          : normalized <= 5
            ? 5
            : 10;
  const step = nice * magnitude;
  return {
    step,
    max: Math.max(step, Math.ceil(safeMax / step) * step),
  };
}

function speedAtFraction(points: SpeedPoint[], fraction: number): number | null {
  if (points.length === 0) return null;
  if (points.length === 1) return points[0].speed;
  if (fraction <= points[0].x) return points[0].speed;
  if (fraction >= points[points.length - 1].x) return points[points.length - 1].speed;

  let low = 0;
  let high = points.length - 1;
  while (high - low > 1) {
    const mid = Math.floor((low + high) / 2);
    if (points[mid].x <= fraction) low = mid;
    else high = mid;
  }

  const a = points[low];
  const b = points[high];
  const span = b.x - a.x;
  if (span <= 0) return b.speed;
  const t = (fraction - a.x) / span;
  return a.speed + (b.speed - a.speed) * t;
}

// Visual stacking order for tag bands (highest priority on top).
// Event is loudest so it renders closest to the segment bar.
const CATEGORY_PRIORITY: TagCategory[] = [
  "event",
  "quality",
  "motion",
  "audio",
  "user",
];

export function Timeline({ onSeekTripTime }: Props) {
  const svgRef = useRef<SVGSVGElement>(null);
  const [hoverSpeed, setHoverSpeed] = useState<HoverSpeed | null>(null);
  const [dragTripTime, setDragTripTime] = useState<number | null>(null);
  const { speedUnit } = usePreferences();
  const trips = useStore((s) => s.trips);
  const loadedTripId = useStore((s) => s.loadedTripId);
  const archiveRoot = useStore(s => s.currentArchive?.root);
  const bookmarks = useBookmarks(archiveRoot, loadedTripId ?? undefined);
  useEffect(() => { setDragTripTime(null); }, [loadedTripId, archiveRoot]);
  const activeSegmentId = useStore((s) => s.activeSegmentId);
  const currentTime = useStore((s) => s.currentTime);
  const sourceMode = useStore((s) => s.sourceMode);
  const activeSpeedCurve = useStore((s) => s.activeSpeedCurve);
  const gpsByFile = useStore((s) => s.gpsByFile);
  const tripGpsByTrip = useStore((s) => s.tripGpsByTrip);
  const tagsBySegmentId = useStore((s) => s.tagsBySegmentId);
  const selectionMode = useStore((s) => s.selectionMode);
  const selectedSegmentIds = useStore((s) => s.selectedSegmentIds);
  const toggleSegmentSelection = useStore((s) => s.toggleSegmentSelection);

  const trip: Trip | undefined = useMemo(
    () => trips.find((t) => t.id === loadedTripId),
    [trips, loadedTripId],
  );

  const totalDuration = useMemo(
    () => tripTotalDuration(trip, activeSpeedCurve),
    [trip, activeSpeedCurve],
  );

  const tripTime = useMemo(
    () =>
      computeTripTime(
        trip,
        activeSegmentId,
        currentTime,
        sourceMode,
        activeSpeedCurve,
      ),
    [trip, activeSegmentId, currentTime, sourceMode, activeSpeedCurve],
  );

  const speedPoints: SpeedPoint[] = useMemo(() => {
    if (!trip || totalDuration <= 0) return [];
    // Archived path: trip_gps rows store trip-stitched t_offset_s, so
    // no per-segment cumulative math is needed and the curve renders
    // even when segments are tombstoned/absent (archive-only).
    const archived = tripGpsByTrip[trip.id];
    if (archived && archived.length > 0) {
      return archived.map((p) => ({
        x: p.tOffsetS / totalDuration,
        speed: p.speedMps,
      }));
    }
    // Fallback: per-file GPS, stitched on the fly. Used for trips that
    // have originals but no timelapse encode yet.
    const pts: SpeedPoint[] = [];
    let cumulative = 0;
    for (const seg of trip.segments) {
      // Master channel carries GPS; use channels[0] (Front or otherwise).
      const front = seg.channels[0];
      if (front) {
        const gps: GpsPoint[] = gpsByFile[front.filePath] ?? [];
        for (const p of gps) {
          pts.push({
            x: (cumulative + p.tOffsetS) / totalDuration,
            speed: p.speedMps,
          });
        }
      }
      cumulative += seg.durationS;
    }
    return pts;
  }, [trip, totalDuration, gpsByFile, tripGpsByTrip]);

  const speedFactor = speedUnit === "mph" ? 2.23694 : 3.6;
  const unitLabel = speedUnit === "mph" ? "mph" : "km/h";

  const maxDisplaySpeed = useMemo(
    () => Math.max(1, ...speedPoints.map((p) => p.speed * speedFactor)),
    [speedPoints, speedFactor],
  );

  const speedScale = useMemo(
    () => niceSpeedScale(maxDisplaySpeed),
    [maxDisplaySpeed],
  );

  const speedTicks = useMemo(() => {
    const ticks: number[] = [];
    for (let value = 0; value <= speedScale.max + speedScale.step / 2; value += speedScale.step) {
      ticks.push(value);
    }
    return ticks;
  }, [speedScale]);

  const speedPath = useMemo(() => {
    if (speedPoints.length < 2) return "";
    return speedPoints
      .map((p, i) => {
        const x = p.x * 100;
        const displaySpeed = p.speed * speedFactor;
        const y = SPEED_AREA_H * (1 - displaySpeed / speedScale.max);
        return `${i === 0 ? "M" : "L"}${x},${y}`;
      })
      .join(" ");
  }, [speedPoints, speedFactor, speedScale.max]);

  const xFraction = useCallback((clientX: number) => {
    const svg = svgRef.current;
    if (!svg) return 0;
    const rect = svg.getBoundingClientRect();
    if (rect.width <= 0) return 0;
    return Math.max(0, Math.min(1, (clientX - rect.left) / rect.width));
  }, []);

  const xToTripTime = useCallback(
    (clientX: number) => xFraction(clientX) * totalDuration,
    [totalDuration, xFraction],
  );

  const updateHover = useCallback(
    (clientX: number) => {
      const fraction = xFraction(clientX);
      const mps = speedAtFraction(speedPoints, fraction);
      setHoverSpeed({
        x: fraction,
        speed: mps === null ? null : mps * speedFactor,
      });
    },
    [speedPoints, speedFactor, xFraction],
  );

  const onPointerDown = useCallback(
    (e: React.PointerEvent) => {
      updateHover(e.clientX);
      // In selection mode the timeline is a multi-select widget, not a
      // seek widget. Clicks are handled per-segment-rect below.
      if (selectionMode || e.button !== 0) return;

      // WebKitGTK/GStreamer turns currentTime assignments into full pipeline
      // flushes. Preview the drag locally and perform exactly one real seek
      // when the pointer is released instead of seeking on every pointermove.
      e.currentTarget.setPointerCapture(e.pointerId);
      setDragTripTime(xToTripTime(e.clientX));
    },
    [xToTripTime, selectionMode, updateHover],
  );

  const onPointerMove = useCallback(
    (e: React.PointerEvent) => {
      updateHover(e.clientX);
      if (selectionMode || dragTripTime === null || (e.buttons & 1) === 0) return;
      setDragTripTime(xToTripTime(e.clientX));
    },
    [xToTripTime, selectionMode, updateHover, dragTripTime],
  );

  const finishTimelineSeek = useCallback(
    (e: React.PointerEvent) => {
      if (selectionMode || dragTripTime === null) return;
      const target = xToTripTime(e.clientX);
      setDragTripTime(null);
      onSeekTripTime(target);
    },
    [dragTripTime, onSeekTripTime, selectionMode, xToTripTime],
  );

  const cancelTimelineSeek = useCallback(() => {
    setDragTripTime(null);
  }, []);

  if (!trip || totalDuration <= 0) return null;

  const bookmarkTimes = bookmarks.times.filter(ms => ms <= Math.round(totalDuration * 1000));
  const previousBookmark = adjacentBookmark(bookmarkTimes, tripTime, -1);
  const nextBookmark = adjacentBookmark(bookmarkTimes, tripTime, 1);
  const setAtTime = (time: number) => void bookmarks.setBookmark(Math.round(Math.max(0, Math.min(totalDuration, time)) * 1000), true);
  const displayedTripTime = dragTripTime ?? tripTime;
  const playheadX = (displayedTripTime / totalDuration) * 100;
  // Archive-only trips have empty segments — render a single full-width
  // tombstone-hatched bar so the playhead has visual context.
  const archiveOnly = trip.segments.length === 0;

  let segCumulative = 0;
  const segRects: React.ReactNode[] = [];
  const selectionMarks: React.ReactNode[] = [];
  const tagBands: React.ReactNode[] = [];
  if (archiveOnly) {
    segRects.push(
      <rect
        key="__archive__"
        x={0}
        y={SPEED_AREA_H + 2}
        width="100%"
        height={SEG_BAR_H}
        rx={2}
        fill="url(#tombstone-hatch)"
      >
        <title>Originals deleted — covered by timelapse</title>
      </rect>,
    );
  }
  for (const seg of trip.segments as Segment[]) {
    const x = (segCumulative / totalDuration) * 100;
    const w = (seg.durationS / totalDuration) * 100;
    const active = seg.id === (activeSegmentId ?? trip.segments[0]?.id);
    const selected = selectedSegmentIds.has(seg.id);
    const tombstone = seg.isTombstone === true;
    segCumulative += seg.durationS;
    const segId = seg.id;
    // Tombstones: render with the diagonal-hatch pattern instead of a
    // solid fill, so the user can see at a glance that originals are
    // gone for this range. Selection / active highlighting still applies
    // — we paint the hatch under whatever colored frame the state would
    // normally use, and use a translucent overlay to keep the "active"
    // / "selected" cues legible.
    const baseFill = selected
      ? "#f43f5e"
      : active
        ? "#3b82f6"
        : seg.isEvent
          ? "#f59e0b"
          : "#374151";
    segRects.push(
      <rect
        key={seg.id}
        x={`${x}%`}
        y={SPEED_AREA_H + 2}
        width={`${w}%`}
        height={SEG_BAR_H}
        rx={2}
        fill={tombstone ? "url(#tombstone-hatch)" : baseFill}
        onClick={
          selectionMode
            ? (e) => {
                // Stop the click bubbling to the SVG pointerdown handler
                // (which is a no-op in selection mode anyway, but be
                // explicit to avoid future regressions).
                e.stopPropagation();
                toggleSegmentSelection(segId, { range: e.shiftKey });
              }
            : undefined
        }
        style={selectionMode ? { cursor: "pointer" } : undefined}
      >
        {tombstone && (
          <title>Originals deleted — covered by timelapse</title>
        )}
      </rect>,
    );
    if (tombstone && (selected || active)) {
      // Translucent state overlay on top of the hatch so highlight cues
      // still register without obscuring the hatch.
      segRects.push(
        <rect
          key={`state-${seg.id}`}
          x={`${x}%`}
          y={SPEED_AREA_H + 2}
          width={`${w}%`}
          height={SEG_BAR_H}
          rx={2}
          fill={baseFill}
          fillOpacity={0.35}
          pointerEvents="none"
        />,
      );
    }
    if (selected) {
      // Thin rose outline above the segment bar so selection reads at a
      // glance even on a narrow timeline. SVG `stroke` on the rect
      // itself would be clipped by adjacent rects; use a separate rect
      // with no fill.
      selectionMarks.push(
        <rect
          key={`sel-${seg.id}`}
          x={`${x}%`}
          y={SPEED_AREA_H + 1}
          width={`${w}%`}
          height={SEG_BAR_H + 2}
          rx={2}
          fill="none"
          stroke="#fda4af"
          strokeWidth={0.5}
          vectorEffect="non-scaling-stroke"
          pointerEvents="none"
        />,
      );
    }
    // Collect unique categories from this segment's tags. The
    // EE/is_event fallback is already covered by the `event` tag
    // emitted by ee_normalize, so we don't double-render here.
    const tags = tagsBySegmentId[seg.id] ?? [];
    if (tags.length === 0) continue;
    const categories = new Set<TagCategory>();
    for (const tag of tags) categories.add(tag.category);
    const ordered = CATEGORY_PRIORITY.filter((c) => categories.has(c)).slice(
      0,
      MAX_BANDS,
    );
    ordered.forEach((category, i) => {
      tagBands.push(
        <rect
          key={`${seg.id}-${category}`}
          x={`${x}%`}
          y={SPEED_AREA_H + 2 + SEG_BAR_H + i * TAG_BAND_H}
          width={`${w}%`}
          height={TAG_BAND_H}
          fill={CATEGORY_COLORS[category].hex}
        />,
      );
    });
  }

  const hoverY =
    hoverSpeed?.speed == null
      ? null
      : SPEED_AREA_H * (1 - hoverSpeed.speed / speedScale.max);

  return (<>
    <div className="flex flex-wrap items-center gap-2 border-t border-neutral-800 px-3 py-1 text-xs text-neutral-300">
      <button type="button" title="Previous bookmark" aria-label="Previous bookmark" disabled={selectionMode || previousBookmark === null}
        className="rounded px-2 py-1 hover:bg-neutral-700 disabled:opacity-30" onClick={() => previousBookmark !== null && onSeekTripTime(previousBookmark)}>
        <svg aria-hidden width="18" height="18" viewBox="0 0 18 18" fill="none" stroke="currentColor" strokeWidth="1.7"><path d="M4 3v12M14 4l-7 5 7 5Z" /></svg>
      </button>
      <button type="button" title="Next bookmark" aria-label="Next bookmark" disabled={selectionMode || nextBookmark === null}
        className="rounded px-2 py-1 hover:bg-neutral-700 disabled:opacity-30" onClick={() => nextBookmark !== null && onSeekTripTime(nextBookmark)}>
        <svg aria-hidden width="18" height="18" viewBox="0 0 18 18" fill="none" stroke="currentColor" strokeWidth="1.7"><path d="M14 3v12M4 4l7 5-7 5Z" /></svg>
      </button>
      <button type="button" disabled={!bookmarks.ready || selectionMode} onClick={() => setAtTime(tripTime)} className="rounded px-2 py-1 hover:bg-neutral-700 disabled:opacity-30">+ Bookmark</button>
      <span className="text-neutral-500">Right-click graph to add · right-click marker to remove</span>
      {bookmarks.error && <span role="alert" className="text-amber-300">{bookmarks.error}</span>}
    </div>
    <div
      data-tripviewer-timeline
      style={{ height: "var(--tripviewer-timeline-height, 3.5rem)" }}
      className="relative min-h-14 w-full select-none"
    >
      <svg
        ref={svgRef}
        viewBox={`0 0 100 ${HEIGHT}`}
        preserveAspectRatio="none"
        className={
          selectionMode
            ? "absolute inset-0 h-full w-full cursor-default"
            : "absolute inset-0 h-full w-full cursor-pointer"
        }
        onContextMenu={e => { e.preventDefault(); if (!selectionMode) setAtTime(xToTripTime(e.clientX)); }}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={finishTimelineSeek}
        onPointerCancel={cancelTimelineSeek}
        onPointerLeave={() => setHoverSpeed(null)}
      >
        {/* Diagonal-hatch fill for tombstone segments (originals deleted,
            covered by the trip's timelapse archive). preserveAspectRatio
            is "none" so the timeline stretches arbitrarily; we use
            patternUnits=userSpaceOnUse and a small stride so the hatch
            density stays readable at any width. */}
        <defs>
          <pattern
            id="tombstone-hatch"
            patternUnits="userSpaceOnUse"
            width={2}
            height={4}
            patternTransform="rotate(45)"
          >
            <rect width={2} height={4} fill="#4b5563" />
            <rect width={1} height={4} fill="#1f2937" />
          </pattern>
        </defs>

        {/* Speed grid. Labels are HTML overlays below so they do not stretch
            with preserveAspectRatio="none". */}
        {speedTicks.map((value) => {
          const y = SPEED_AREA_H * (1 - value / speedScale.max);
          return (
            <line
              key={`speed-grid-${value}`}
              x1={0}
              y1={y}
              x2={100}
              y2={y}
              stroke="#6b7280"
              strokeWidth={0.35}
              strokeOpacity={value === 0 ? 0.35 : 0.2}
              vectorEffect="non-scaling-stroke"
              pointerEvents="none"
            />
          );
        })}

        {/* Speed curve */}
      {speedPath && (
          <path
            d={speedPath}
            fill="none"
            stroke="#3b82f6"
            strokeWidth={0.7}
            strokeOpacity={0.8}
            vectorEffect="non-scaling-stroke"
            pointerEvents="none"
          />
        )}

        {/* Segment bars */}
        {segRects}

        {/* Selection outlines (above segment bars, below playhead) */}
        {selectionMarks}

        {/* Category-colored tag bands stacked below the segment bars */}
        {tagBands}

        {/* Hover position over the speed graph. */}
        {hoverSpeed && (
          <line
            x1={`${hoverSpeed.x * 100}%`}
            y1={0}
            x2={`${hoverSpeed.x * 100}%`}
            y2={SPEED_AREA_H}
            stroke="#d1d5db"
            strokeWidth={0.5}
            strokeOpacity={0.55}
            vectorEffect="non-scaling-stroke"
            pointerEvents="none"
          />
        )}

        {bookmarkTimes.map(ms => <line key={`bookmark-line-${ms}`} x1={`${ms / (totalDuration * 10)}%`} x2={`${ms / (totalDuration * 10)}%`}
          y1={0} y2={HEIGHT} stroke="#fbbf24" strokeWidth={1} vectorEffect="non-scaling-stroke" pointerEvents="none" />)}

        {/* Playhead */}
        <line
          x1={`${playheadX}%`}
          y1={0}
          x2={`${playheadX}%`}
          y2={HEIGHT}
          stroke="#ef4444"
          strokeWidth={0.5}
          vectorEffect="non-scaling-stroke"
          pointerEvents="none"
        />
      </svg>

        {bookmarkTimes.map((ms, index) => <button type="button" key={ms}
        aria-label={`Bookmark ${index + 1} at ${bookmarkTimeLabel(ms)}`}
        title={`Bookmark ${index + 1} · ${bookmarkTimeLabel(ms)} · Click to seek, right-click to remove`}
        disabled={selectionMode}
        className="absolute top-0 z-20 h-5 w-4 rounded-b bg-amber-400 text-[10px] font-bold text-neutral-950 disabled:opacity-40"
        style={{ left: `${ms / (totalDuration * 10)}%`, transform: ms === 0 ? undefined : ms >= totalDuration * 1000 - 1 ? "translateX(-100%)" : "translateX(-50%)" }}
        onClick={() => onSeekTripTime(ms / 1000)}
        onContextMenu={e => { e.preventDefault(); e.stopPropagation(); if (!selectionMode) void bookmarks.setBookmark(ms, false); }}>
        {index + 1}
      </button>)}

      {speedPath && (
        <div className="pointer-events-none absolute inset-0 text-[9px] text-neutral-500">
          <div className="absolute left-1 top-0 rounded bg-neutral-950/70 px-1 py-0.5 font-medium text-neutral-400">
            {unitLabel}
          </div>
          {speedTicks
            .filter((value) => value > 0)
            .map((value) => {
              const y = SPEED_AREA_H * (1 - value / speedScale.max);
              return (
                <div
                  key={`speed-label-${value}`}
                  className="absolute left-1 rounded bg-neutral-950/55 px-1 tabular-nums"
                  style={{
                    top: `${(y / HEIGHT) * 100}%`,
                    transform: "translateY(-50%)",
                  }}
                >
                  {Math.round(value)}
                </div>
              );
            })}
        </div>
      )}

      {hoverSpeed && hoverSpeed.speed !== null && hoverY !== null && (
        <div
          className="pointer-events-none absolute z-10 rounded bg-neutral-950/90 px-2 py-1 text-[11px] font-medium tabular-nums text-neutral-100 shadow"
          style={{
            left: `${Math.min(94, Math.max(6, hoverSpeed.x * 100))}%`,
            top: `${Math.min(70, Math.max(5, (hoverY / HEIGHT) * 100))}%`,
            transform: "translate(-50%, -110%)",
          }}
        >
          {Math.round(hoverSpeed.speed)} {unitLabel}
        </div>
      )}
    </div>
  </>);
}
