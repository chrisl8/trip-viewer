/** Shared-player fullscreen adapted from the full fork's VideoGrid path.
 * The browser owns one viewer root; cameras and transport keep their identity.
 * Native video fullscreen is intentionally not used: its controls bypass the
 * shared SyncEngine and can pause just one camera.
 */
export async function toggleViewerFullscreen(
  root: HTMLElement | null,
  label: string,
  currentLabel: string | null,
  commit: (label: string | null) => void,
  resync: () => void,
  host: Pick<Document, "fullscreenElement" | "exitFullscreen"> = document,
): Promise<void> {
  if (!root) return;
  const leaving = host.fullscreenElement === root && currentLabel === label;
  // Commit before requesting the browser transition, as in the full fork.
  // Resetting before exit lets Escape/fullscreenchange share the same path
  // without scheduling a duplicate re-anchor for this explicit exit.
  commit(leaving ? null : label);
  try {
    if (leaving) await host.exitFullscreen();
    else if (host.fullscreenElement !== root) await root.requestFullscreen();
    resync();
  } catch (error) {
    commit(currentLabel);
    throw error;
  }
}
