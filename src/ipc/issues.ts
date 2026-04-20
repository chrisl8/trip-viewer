import { invoke } from "@tauri-apps/api/core";

/** Move a single file to the OS recycle bin / Trash. */
export function deleteToTrash(path: string): Promise<void> {
  return invoke("issues_delete_to_trash", { path });
}
