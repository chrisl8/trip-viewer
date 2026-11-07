import type { CSSProperties } from "react";

/**
 * Compute CSS grid placement for a channel panel.
 *
 * Layout philosophy: primary takes col 1 full height; secondaries stack
 * in col 2. Row count adapts to secondary count so each secondary gets
 * the full column width. Works for 1, 2, 3, 4+ channels.
 */
export function channelGridStyle(
  isPrimary: boolean,
  secondaryIndex: number,
  secondaryCount: number,
  rowCount: number,
): CSSProperties {
  if (secondaryCount === 0) {
    // Single channel: fill the whole area.
    return { gridColumn: "1 / 3", gridRow: "1 / 3" };
  }
  if (isPrimary) {
    // Primary occupies col 1, spanning all rows. Using rowCount (not
    // secondaryCount) ensures the primary fills the full grid height even
    // when rowCount > secondaryCount (the 2-channel case, where rowCount=2
    // but secondaryCount=1). Without this, an empty grid row captures
    // pointer events from the transport controls below via a Chromium
    // compositor hit-testing edge case.
    return { gridColumn: 1, gridRow: `1 / ${rowCount + 1}` };
  }
  // Secondary cell: col 2, one row per secondary.
  return { gridColumn: 2, gridRow: secondaryCount === 1 ? `1 / ${rowCount + 1}` : secondaryIndex + 1 };
}
