import { useEffect, useState } from "react";
import { getLayoutPreferences, saveLayoutPreferences, subscribeLayoutPreferences } from "../../settings/layout";
import { bringOverlayToFront, normalizeOverlayPanel, type CameraLayout, type OverlayPanel, type OverlayPanels } from "../../settings/overlayLayout";

export function useCameraLayout() {
  const [saved, setSaved] = useState(getLayoutPreferences);
  const [draft, setDraft] = useState<OverlayPanels | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [operation, setOperation] = useState<"position" | "crop">("position");
  useEffect(() => subscribeLayoutPreferences(() => {
    const preferences = getLayoutPreferences();
    setSaved(preferences);
    if (preferences.cameraLayout !== "overlay") setDraft(null);
  }), []);
  return {
    mode: saved.cameraLayout,
    panels: draft ?? saved.overlayPanels,
    editing: draft !== null,
    selected, setSelected, operation, setOperation,
    setMode(mode: CameraLayout) {
      setDraft(null);
      saveLayoutPreferences({ cameraLayout: mode });
    },
    edit() { setDraft(saved.overlayPanels); setOperation("position"); },
    cancel() { setDraft(null); },
    save() {
      if (draft) saveLayoutPreferences({ overlayPanels: draft });
      setDraft(null);
    },
    reset() { setDraft({}); },
    update(label: string, panel: OverlayPanel) {
      setDraft(current => current === null ? null : { ...current, [label]: normalizeOverlayPanel(panel, label) });
    },
    bringToFront(labels: string[], label: string) {
      setDraft(current => current === null ? null : bringOverlayToFront(current, labels, label));
    },
  };
}

export type CameraLayoutController = ReturnType<typeof useCameraLayout>;
