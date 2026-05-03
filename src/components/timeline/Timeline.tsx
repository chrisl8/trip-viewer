import { useCallback, useMemo, useRef } from "react";
import { useStore } from "../../state/store";
import type { GpsPoint, Segment, TagCategory, Trip } from "../../types/model";
import { CATEGORY_COLORS } from "../../utils/tagColors";
import { computeTripTime, tripTotalDuration } from "../../utils/tripTime";

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
  const trips = useStore((s) => s.trips);
  const loadedTripId = useStore((s) => s.loadedTripId);
  const activeSegmentId = useStore((s) => s.activeSegmentId);
  const currentTime = useStore((s) => s.currentTime);
  const sourceMode = useStore((s) => s.sourceMode);
  const activeSpeedCurve = useStore((s) => s.activeSpeedCurve);
  const gpsByFile = useStore((s) => s.gpsByFile);
  const tagsBySegmentId = useStore((s) => s.tagsBySegmentId);
  const selectionMode = useStore((s) => s.selectionMode);
  const selectedSegmentIds = useStore((s) => s.selectedSegmentIds);
  const toggleSegmentSelection = useStore((s) => s.toggleSegmentSelection);

  const trip: Trip | undefined = useMemo(
    () => trips.find((t) => t.id === loadedTripId),
    [trips, loadedTripId],
  );

  const totalDuration = useMemo(() => tripTotalDuration(trip), [trip]);

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

  const speedPoints: { x: number; speed: number }[] = useMemo(() => {
    if (!trip || totalDuration <= 0) return [];
    const pts: { x: number; speed: number }[] = [];
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
  }, [trip, totalDuration, gpsByFile]);

  const maxSpeed = useMemo(
    () => Math.max(1, ...speedPoints.map((p) => p.speed)),
    [speedPoints],
  );

  const speedPath = useMemo(() => {
    if (speedPoints.length < 2) return "";
    return speedPoints
      .map((p, i) => {
        const x = p.x * 100;
        const y = SPEED_AREA_H * (1 - p.speed / maxSpeed);
        return `${i === 0 ? "M" : "L"}${x},${y}`;
      })
      .join(" ");
  }, [speedPoints, maxSpeed]);

  const xToTripTime = useCallback(
    (clientX: number) => {
      const svg = svgRef.current;
      if (!svg || totalDuration <= 0) return 0;
      const rect = svg.getBoundingClientRect();
      const frac = Math.max(0, Math.min(1, (clientX - rect.left) / rect.width));
      return frac * totalDuration;
    },
    [totalDuration],
  );

  const onPointerDown = useCallback(
    (e: React.PointerEvent) => {
      // In selection mode the timeline is a multi-select widget, not a
      // seek widget. Clicks are handled per-segment-rect below; the
      // SVG-level pointerdown is suppressed entirely so a click on
      // empty timeline space (between segments, on the speed curve)
      // doesn't accidentally seek and break the user's selection flow.
      if (selectionMode) return;
      e.currentTarget.setPointerCapture(e.pointerId);
      onSeekTripTime(xToTripTime(e.clientX));
    },
    [xToTripTime, onSeekTripTime, selectionMode],
  );

  const onPointerMove = useCallback(
    (e: React.PointerEvent) => {
      if (selectionMode) return;
      if (e.buttons === 0) return;
      onSeekTripTime(xToTripTime(e.clientX));
    },
    [xToTripTime, onSeekTripTime, selectionMode],
  );

  if (!trip || totalDuration <= 0) return null;

  const playheadX = (tripTime / totalDuration) * 100;

  let segCumulative = 0;
  const segRects: React.ReactNode[] = [];
  const selectionMarks: React.ReactNode[] = [];
  const tagBands: React.ReactNode[] = [];
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

  return (
    <svg
      ref={svgRef}
      viewBox={`0 0 100 ${HEIGHT}`}
      preserveAspectRatio="none"
      className={
        selectionMode
          ? "h-14 w-full cursor-default select-none"
          : "h-14 w-full cursor-pointer select-none"
      }
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
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

      {/* Speed curve */}
      {speedPath && (
        <path
          d={speedPath}
          fill="none"
          stroke="#3b82f6"
          strokeWidth={0.4}
          strokeOpacity={0.6}
          vectorEffect="non-scaling-stroke"
        />
      )}

      {/* Segment bars */}
      {segRects}

      {/* Selection outlines (above segment bars, below playhead) */}
      {selectionMarks}

      {/* Category-colored tag bands stacked below the segment bars */}
      {tagBands}

      {/* Playhead */}
      <line
        x1={`${playheadX}%`}
        y1={0}
        x2={`${playheadX}%`}
        y2={HEIGHT}
        stroke="#ef4444"
        strokeWidth={0.5}
        vectorEffect="non-scaling-stroke"
      />
    </svg>
  );
}
