import clsx from "clsx";
import { useStore } from "../state/store";
import { KIND_META, kindCounts } from "../utils/issueKinds";
import type { MainView } from "../state/store";

interface TabSpec {
  view: MainView;
  label: string;
  /** Active-tab accent color. Mirrors the sidebar button colors that
   *  used to live in App.tsx, so the per-view "tone" stays consistent. */
  accent: {
    border: string;
    text: string;
  };
}

// Main workflow tabs (left cluster). Player is the canvas; Scan,
// Review, and Timelapse are the analyze → triage → produce phases.
const MAIN_TABS: TabSpec[] = [
  {
    view: "player",
    label: "Player",
    accent: { border: "border-blue-500", text: "text-blue-300" },
  },
  {
    view: "scan",
    label: "Scan",
    accent: { border: "border-sky-500", text: "text-sky-300" },
  },
  {
    view: "review",
    label: "Review",
    accent: { border: "border-emerald-500", text: "text-emerald-300" },
  },
  {
    view: "timelapse",
    label: "Timelapse",
    accent: { border: "border-violet-500", text: "text-violet-300" },
  },
];

// Utility tabs (right cluster). Configuration / setup screens, not
// workflow phases — separated by a flex spacer so the tab bar reads
// "main views on the left, settings on the right." Currently just
// Places (POI setup for the gps_place scan); future Settings or Help
// tabs would join here.
const UTILITY_TABS: TabSpec[] = [
  {
    view: "places",
    label: "⚙ Places",
    accent: { border: "border-rose-500", text: "text-rose-300" },
  },
];

/**
 * Horizontal tab strip rendered at the top of `<main>`. Replaces the
 * sidebar-header Scan/Review/Timelapse button row and the per-view
 * "X Close" buttons. The active tab is the navigation indicator —
 * clicking another tab is the way to leave a view, no Close button
 * needed.
 *
 * Running-status indicators (Scanning… N/M, Encoding… N/M) are
 * inherited from the sidebar buttons that previously owned them so
 * progress stays visible no matter which tab is active.
 */
export function MainNavTabs() {
  const mainView = useStore((s) => s.mainView);
  const setMainView = useStore((s) => s.setMainView);
  const scanRunning = useStore((s) => s.scanRunning);
  const scanProgress = useStore((s) => s.scanProgress);
  const timelapseRunning = useStore((s) => s.timelapseRunning);
  const timelapseProgress = useStore((s) => s.timelapseProgress);
  const scanErrors = useStore((s) => s.scanErrors);

  const issueCount = scanErrors.length;
  const issueBreakdown = kindCounts(scanErrors);

  function tabLabel(view: MainView, baseLabel: string): string {
    if (view === "scan" && scanRunning) {
      return `Scanning… ${scanProgress?.done ?? 0}/${scanProgress?.total ?? "?"}`;
    }
    if (view === "timelapse" && timelapseRunning) {
      return `Encoding… ${timelapseProgress?.done ?? 0}/${timelapseProgress?.total ?? "?"}`;
    }
    return baseLabel;
  }

  function renderTab({ view, label, accent }: TabSpec) {
    const active = mainView === view;
    const isPulsing =
      (view === "scan" && scanRunning && !active) ||
      (view === "timelapse" && timelapseRunning && !active);
    return (
      <button
        key={view}
        role="tab"
        aria-selected={active}
        onClick={() => setMainView(view)}
        className={clsx(
          "border-b-2 px-3 py-1.5 text-sm font-medium transition-colors",
          active
            ? `${accent.border} ${accent.text}`
            : "border-transparent text-neutral-400 hover:text-neutral-200",
          // Match the existing pulse animations used on the sidebar
          // buttons so transitions stay coherent.
          isPulsing && view === "scan" && "animate-pulse-sky",
          isPulsing && view === "timelapse" && "animate-pulse-violet",
        )}
        title={
          view === "scan" && scanRunning
            ? "Scan running — click to view"
            : view === "timelapse" && timelapseRunning
              ? "Timelapse encoding running — click to view"
              : view === "places"
                ? "Define points of interest used by GPS-aware scans"
                : undefined
        }
      >
        {tabLabel(view, label)}
      </button>
    );
  }

  return (
    <nav
      role="tablist"
      className="flex shrink-0 items-end gap-1 border-b border-neutral-800 bg-neutral-950 px-3 pt-1"
    >
      {MAIN_TABS.map(renderTab)}
      {issueCount > 0 && (
        <button
          role="tab"
          aria-selected={mainView === "issues"}
          onClick={() => setMainView("issues")}
          className={clsx(
            "border-b-2 px-3 py-1.5 text-sm font-medium transition-colors",
            mainView === "issues"
              ? "border-yellow-500 text-yellow-300"
              : "border-transparent text-yellow-500 hover:text-yellow-300",
          )}
          title={
            issueBreakdown.length > 0
              ? issueBreakdown
                  .slice(0, 3)
                  .map(
                    (b) =>
                      `${b.count} ${KIND_META[b.kind].label.toLowerCase()}`,
                  )
                  .join(" · ")
              : undefined
          }
        >
          {issueCount} {issueCount === 1 ? "issue" : "issues"}
        </button>
      )}
      {/* Spacer pushes the utility cluster to the right edge. */}
      <div className="flex-1" aria-hidden="true" />
      {UTILITY_TABS.map(renderTab)}
    </nav>
  );
}
