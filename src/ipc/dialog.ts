import { open } from "@tauri-apps/plugin-dialog";

export async function pickFolder(
  title: string = "Select a folder of dashcam footage",
): Promise<string | null> {
  const selected = await open({
    directory: true,
    multiple: false,
    title,
  });
  if (typeof selected === "string") return selected;
  return null;
}
