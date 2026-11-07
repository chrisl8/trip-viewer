/** Find a strictly adjacent bookmark; round floating-point clocks to the persisted millisecond. */
export function adjacentBookmark(timesMs: number[], timeS: number, direction: -1 | 1): number | null {
  const timeMs = Math.round(timeS * 1000);
  const candidates = timesMs.filter(t => direction < 0 ? t < timeMs : t > timeMs);
  return candidates.length ? (direction < 0 ? Math.max(...candidates) : Math.min(...candidates)) / 1000 : null;
}

export function bookmarkTimeLabel(ms: number): string {
  const seconds = Math.floor(ms / 1000);
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}.${String(ms % 1000).padStart(3, "0")}`;
}
