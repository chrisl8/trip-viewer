import { open } from "@tauri-apps/plugin-dialog";

export async function pickFolder(): Promise<string | null> {
  const selected = await open({
    directory: true,
    multiple: false,
    title: "Select a folder of dashcam footage",
  });
  if (typeof selected === "string") return selected;
  return null;
}
