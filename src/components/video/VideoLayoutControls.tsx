import { overlayBackground, overlayPanelFor } from "../../settings/overlayLayout";
import type { OverlayCrop } from "../../settings/overlayLayout";
import type { CameraLayoutController } from "./useCameraLayout";

interface Props { layout: CameraLayoutController; labels: string[]; hidden?: boolean }
const control = "rounded border border-neutral-600 bg-neutral-900 px-2 py-1 text-xs text-neutral-100 disabled:opacity-40";

export function VideoLayoutControls({ layout, labels, hidden }: Props) {
  const background = overlayBackground(labels);
  const overlays = labels.filter(label => label !== background);
  const selected = overlays.find(label => label === layout.selected) ?? overlays[0];
  const panel = selected ? overlayPanelFor(layout.panels, selected, overlays.indexOf(selected)) : null;
  function updateCrop(change: Partial<OverlayCrop>) {
    if (!selected || !panel) return;
    // Crop controls also enable direct panning, so adjusting zoom doesn't leave
    // the mouse in Move frame mode. The frame and the background stay put.
    layout.setOperation("crop");
    layout.update(selected, { ...panel, crop: { ...panel.crop, ...change } });
  }
  return (
    <div hidden={hidden} onKeyDown={event => event.stopPropagation()}
      className="shrink-0 border-b border-neutral-800 bg-neutral-950 px-3 py-2 text-xs text-neutral-300">
      <div className="flex flex-wrap items-center gap-2">
        <label className="flex items-center gap-2">Camera layout
          <select aria-label="Camera layout" className={control} value={layout.mode}
            disabled={layout.editing} onChange={event => layout.setMode(event.target.value === "overlay" ? "overlay" : "grid")}>
            <option value="grid">Classic</option><option value="overlay">Overlay</option>
          </select>
        </label>
        {layout.mode === "overlay" && !layout.editing && (
          <button type="button" className={control} disabled={overlays.length === 0} onClick={layout.edit}>Edit overlay</button>
        )}
        {layout.mode === "overlay" && <span className="text-neutral-500">{background ? `${background} background` : "No visible cameras"}</span>}
        {layout.editing && <>
          <button type="button" className="rounded bg-blue-600 px-3 py-1 text-xs text-white" onClick={layout.save}>Save layout</button>
          <button type="button" className={control} onClick={layout.cancel}>Cancel</button>
          <button type="button" className={control} onClick={layout.reset}>Reset overlay</button>
        </>}
      </div>
      {layout.editing && panel && selected && (
        <div className="mt-2 flex flex-wrap items-center gap-2" aria-label="Overlay editor">
          <label className="flex items-center gap-2">Camera
            <select aria-label="Overlay camera" className={control} value={selected} onChange={event => layout.setSelected(event.target.value)}>
              {overlays.map(label => <option key={label}>{label}</option>)}
            </select>
          </label>
          <label className="flex items-center gap-2">Drag to
            <select aria-label="Overlay edit action" className={control} value={layout.operation}
              onChange={event => layout.setOperation(event.target.value === "crop" ? "crop" : "position")}>
              <option value="position">Move frame</option><option value="crop">Adjust video crop</option>
            </select>
          </label>
          <label className="flex items-center gap-2">Shape
            <select aria-label="Overlay shape" className={control} value={panel.frame}
              onChange={event => layout.update(selected, { ...panel, frame: event.target.value === "mirror" ? "mirror" : "rounded" })}>
              <option value="rounded">Rounded rectangle</option><option value="mirror">Rear-view mirror</option>
            </select>
          </label>
          <label className="flex items-center gap-2">Crop zoom
            <input aria-label="Overlay crop zoom" type="range" min="1" max="8" step="0.05" value={panel.crop.zoom}
              onChange={event => updateCrop({ zoom: Number(event.target.value) })} />
            <span className="w-10 tabular-nums">{Math.round(panel.crop.zoom * 100)}%</span>
          </label>
          <button type="button" className={control}
            title="Reset zoom to 100% and center the horizontal and vertical crop; keep the frame position and size"
            onClick={() => updateCrop({ zoom: 1, x: 0, y: 0 })}>Reset crop</button>
          <button type="button" className={control} onClick={() => layout.bringToFront(overlays, selected)}>Bring to front</button>
          <div className="flex w-full flex-wrap items-center gap-x-5 gap-y-2" aria-label="Visible video area">
            <label className="flex items-center gap-2">Horizontal crop
              <span className="text-neutral-500">Left</span>
              <input aria-label="Overlay horizontal crop position" type="range" min="0" max="100" step="1"
                value={Math.round((1 - panel.crop.x) * 50)}
                aria-valuetext={panel.crop.x === 0 ? "Center" : `${Math.round((1 - panel.crop.x) * 50)}% from left`}
                onChange={event => updateCrop({ x: 1 - Number(event.target.value) / 50 })} />
              <span className="text-neutral-500">Right</span>
              {panel.crop.x === 0 && <span className="text-neutral-400">Center</span>}
            </label>
            <label className="flex items-center gap-2">Vertical crop
              <span className="text-neutral-500">Top / sky</span>
              <input aria-label="Overlay vertical crop position" type="range" min="0" max="100" step="1"
                value={Math.round((1 - panel.crop.y) * 50)}
                aria-valuetext={panel.crop.y === 0 ? "Center" : `${Math.round((1 - panel.crop.y) * 50)}% from top`}
                onChange={event => updateCrop({ y: 1 - Number(event.target.value) / 50 })} />
              <span className="text-neutral-500">Bottom / road</span>
              {panel.crop.y === 0 && <span className="text-neutral-400">Center</span>}
            </label>
          </div>
          <p className="w-full text-neutral-500">Crop position selects the visible part of the recording. Move Vertical crop toward Bottom / road to show less sky. Zoom and crop-position controls enable Adjust video crop, so dragging the video then pans inside the fixed frame. In Move frame mode, drag the lower-right handle to resize. Focus a handle and use arrow keys for fine adjustments (Shift for larger steps). Save keeps the layout after restarting.</p>
        </div>
      )}
    </div>
  );
}
