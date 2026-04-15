interface Props {
  onClose: () => void;
}

const shortcuts = [
  { keys: "Space", action: "Play / Pause" },
  { keys: "\u2190 / \u2192", action: "Seek 5 seconds" },
  { keys: "Shift + \u2190 / \u2192", action: "Seek 30 seconds" },
  { keys: "[ / ]", action: "Decrease / Increase speed" },
  { keys: "D", action: "Toggle drift HUD" },
  { keys: "M", action: "Toggle multi-channel view (Linux only)" },
];

const interactions = [
  { gesture: "Click side video", action: "Make it the main view" },
  { gesture: "Double-click main video", action: "Toggle fullscreen" },
  { gesture: "Escape", action: "Exit fullscreen" },
];

export function KeyboardShortcutsHelp({ onClose }: Props) {
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60" onClick={onClose}>
      <div
        className="w-full max-w-sm rounded-lg border border-neutral-700 bg-neutral-900 p-6"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="mb-4 flex items-center justify-between">
          <h2 className="text-sm font-semibold text-neutral-100">
            Keyboard Shortcuts
          </h2>
          <button
            onClick={onClose}
            className="text-neutral-500 hover:text-neutral-300"
          >
            &times;
          </button>
        </div>

        <table className="w-full text-xs">
          <tbody>
            {shortcuts.map((s) => (
              <tr key={s.keys} className="border-b border-neutral-800">
                <td className="py-1.5 pr-4">
                  <kbd className="rounded bg-neutral-800 px-1.5 py-0.5 font-mono text-neutral-300">
                    {s.keys}
                  </kbd>
                </td>
                <td className="py-1.5 text-neutral-400">{s.action}</td>
              </tr>
            ))}
          </tbody>
        </table>

        <h3 className="mb-2 mt-4 text-xs font-semibold text-neutral-400">
          Mouse
        </h3>
        <table className="w-full text-xs">
          <tbody>
            {interactions.map((s) => (
              <tr key={s.gesture} className="border-b border-neutral-800">
                <td className="py-1.5 pr-4 text-neutral-300">{s.gesture}</td>
                <td className="py-1.5 text-neutral-400">{s.action}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
