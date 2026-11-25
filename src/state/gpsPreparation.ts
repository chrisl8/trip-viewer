import { ensureTripGps } from "../ipc/gps";
import type { GpsPoint, Trip } from "../types/model";
import { useStore } from "./store";

type Request = { archive: ReturnType<typeof useStore.getState>["currentArchive"]; baseline: GpsPoint[] | undefined; sources: string; promise: Promise<void> };
const requests = new Map<string, Request>();

/** Show completed segments immediately, keeping persisted GPS atomic. */
export function prepareTripGps(
  trip: Trip,
  excluded: Record<string, string[]> = {},
  force = false,
): Promise<void> {
  const archive = useStore.getState().currentArchive;
  const pending = requests.get(trip.id);
  const previous = pending?.archive === archive ? pending : undefined;
  const sources = JSON.stringify(trip.segments);
  // Navigation rejoins the active job, including any forced health refresh.
  // It must not clear its preview or queue another extraction behind it.
  if (previous && previous.sources === sources && !force && Object.keys(excluded).length === 0) {
    return previous.promise;
  }
  const request: Request = {
    archive,
    baseline: previous ? previous.baseline : useStore.getState().tripGpsByTrip[trip.id],
    sources,
    promise: Promise.resolve(),
  };
  requests.set(trip.id, request);
  request.promise = runPreparation(trip, excluded, force, request);
  return request.promise;
}

async function runPreparation(
  trip: Trip,
  excluded: Record<string, string[]>,
  force: boolean,
  request: Request,
): Promise<void> {
  const active = () => requests.get(trip.id) === request && useStore.getState().currentArchive === request.archive;
  const resetPreview = () => useStore.setState((s) => {
    const gps = { ...s.tripGpsByTrip };
    if (request.baseline === undefined) delete gps[trip.id];
    else gps[trip.id] = request.baseline;
    return { tripGpsByTrip: gps };
  });
  resetPreview();
  useStore.setState((s) => {
    const progress = { ...s.gpsProgressByTrip };
    delete progress[trip.id];
    return {
      gpsPreparationByTrip: { ...s.gpsPreparationByTrip, [trip.id]: "loading" },
      gpsProgressByTrip: progress,
    };
  });
  let attempt = 0;
  const partial = new Map<number, GpsPoint[]>();
  const offsets: number[] = [];
  let cursor = 0;
  for (const segment of trip.segments) {
    offsets.push(cursor);
    cursor += segment.durationS;
  }
  try {
    const points = await ensureTripGps(trip, excluded, force, (progress) => {
      if (!active()) return;
      if (progress.kind === "started") {
        if (progress.attempt < attempt) return;
        attempt = progress.attempt;
        partial.clear();
        resetPreview();
      } else {
        if (progress.attempt !== attempt) return;
        for (const update of progress.updates) {
          if (update.index >= 0 && update.index < trip.segments.length) {
            partial.set(update.index, update.points);
          }
        }
        const preview = trip.segments.flatMap((segment, index) =>
          partial.get(index) ?? (request.baseline ?? []).filter((p) =>
            p.tOffsetS >= offsets[index] && p.tOffsetS < offsets[index] + segment.durationS));
        useStore.setState((s) => ({ tripGpsByTrip: { ...s.tripGpsByTrip, [trip.id]: preview } }));
      }
      useStore.setState((s) => ({ gpsProgressByTrip: {
        ...s.gpsProgressByTrip,
        [trip.id]: { completed: progress.kind === "started" ? 0 : progress.completed, total: progress.total },
      } }));
    });
    if (!active()) return;
    useStore.setState((s) => ({
      tripGpsByTrip: { ...s.tripGpsByTrip, [trip.id]: points },
      gpsPreparationByTrip: { ...s.gpsPreparationByTrip, [trip.id]: "ready" },
    }));
  } catch (error) {
    if (active()) {
      resetPreview();
      useStore.setState((s) => ({ gpsPreparationByTrip: { ...s.gpsPreparationByTrip, [trip.id]: "error" } }));
    }
    throw error;
  } finally {
    if (requests.get(trip.id) === request) requests.delete(trip.id);
  }
}
