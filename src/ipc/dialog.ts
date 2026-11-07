import { open } from "@tauri-apps/plugin-dialog";

type FolderKind = "archive" | "import-source";
const STORAGE_KEY = "tripviewer.folderPicker.v1";

function savedFolders(): Partial<Record<FolderKind, string>> {
  if (typeof window === "undefined") return {};
  try {
    const value = JSON.parse(window.localStorage.getItem(STORAGE_KEY) ?? "{}");
    return Object.fromEntries((["archive", "import-source"] as const)
      .filter(key => typeof value?.[key] === "string" && value[key].length > 0)
      .map(key => [key, value[key]]));
  } catch {
    return {};
  }
}

export async function pickFolder(
  title: string = "Select a folder of dashcam footage",
  defaultPath?: string,
  kind: FolderKind = "archive",
): Promise<string | null> {
  const initialPath = defaultPath ?? savedFolders()[kind];
  const selected = await open({
    directory: true,
    multiple: false,
    title,
    ...(initialPath ? { defaultPath: initialPath } : {}),
  });
  if (typeof selected === "string") {
    if (typeof window !== "undefined") {
      try {
        window.localStorage.setItem(STORAGE_KEY, JSON.stringify({ ...savedFolders(), [kind]: selected }));
      } catch (error) {
        console.warn("[folder-picker] could not remember the last folder", error);
      }
    }
    return selected;
  }
  return null;
}
