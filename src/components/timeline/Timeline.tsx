import { useCallback, useMemo, useRef } from "react";
import { useStore } from "../../state/store";
import type { GpsPoint, Segment, Trip } from "../../types/model";

interface Props {
  onSeekTripTime: (tripTime: number) => void;
}

const HEIGHT = 56;
const SEG_BAR_H = 8;
const SPEED_AREA_H = HEIGHT - SEG_BAR_H - 4;

export function Timeline({ onSeekTripTime }: Props) {
  const svgRef = useRef<SVGSVGElement>(null);
  const trips = useStore((s) => s.trips);
  const loadedTripId = useStore((s) => s.loadedTripId);
  const activeSegmentId = useStore((s) => s.activeSegmentId);
  const currentTime = useStore((s) => s.currentTime);
  const gpsByFile = useStore((s) => s.gpsByFile);

  const trip: Trip | undefined = useMemo(
    () => trips.find((t) => t.id === loadedTripId),
    [trips, loadedTripId],
  );

  const totalDuration = useMemo(
    () => trip?.segments.reduce((sum, s) => sum + s.durationS, 0) ?? 0,
    [trip],
  );

  const tripTime = useMemo(() => {
    if (!trip) return 0;
    const activeId = activeSegmentId ?? trip.segments[0]?.id;
    let cumulative = 0;
    for (const seg of trip.segments) {
      if (seg.id === activeId) return cumulative + currentTime;
      cumulative += seg.durationS;
    }
    return cumulative + currentTime;
  }, [trip, activeSegmentId, currentTime]);

  const speedPoints: { x: number; speed: number }[] = useMemo(() => {
    if (!trip || totalDuration <= 0) return [];
    const pts: { x: number; speed: number }[] = [];
    let cumulative = 0;
    for (const seg of trip.segments) {
      const front = seg.channels.find((c) => c.kind === "front");
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
      e.currentTarget.setPointerCapture(e.pointerId);
      onSeekTripTime(xToTripTime(e.clientX));
    },
    [xToTripTime, onSeekTripTime],
  );

  const onPointerMove = useCallback(
    (e: React.PointerEvent) => {
      if (e.buttons === 0) return;
      onSeekTripTime(xToTripTime(e.clientX));
    },
    [xToTripTime, onSeekTripTime],
  );

  if (!trip || totalDuration <= 0) return null;

  const playheadX = (tripTime / totalDuration) * 100;

  let segCumulative = 0;
  const segRects = trip.segments.map((seg: Segment) => {
    const x = (segCumulative / totalDuration) * 100;
    const w = (seg.durationS / totalDuration) * 100;
    const active = seg.id === (activeSegmentId ?? trip.segments[0]?.id);
    segCumulative += seg.durationS;
    return (
      <rect
        key={seg.id}
        x={`${x}%`}
        y={SPEED_AREA_H + 2}
        width={`${w}%`}
        height={SEG_BAR_H}
        rx={2}
        fill={active ? "#3b82f6" : seg.isEvent ? "#f59e0b" : "#374151"}
      />
    );
  });

  return (
    <svg
      ref={svgRef}
      viewBox={`0 0 100 ${HEIGHT}`}
      preserveAspectRatio="none"
      className="h-14 w-full cursor-pointer select-none"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
    >
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
