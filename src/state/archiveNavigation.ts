import { currentArchive, openArchive, type CurrentArchive } from "../ipc/archive";
import { scanFolder } from "../ipc/scanner";
import { useStore } from "./store";

let selection = 0;
let opening = false;
let openQueue: Promise<unknown> = Promise.resolve();
const scans = new WeakMap<CurrentArchive, Promise<boolean>>();

/** Serialize native opens so the last choice also wins in settings.json. */
export async function selectArchive(path: string): Promise<CurrentArchive | null> {
  const request = ++selection;
  opening = true;
  useStore.setState({ error: null, status: "loading" });
  const task = openQueue.then(() => request === selection ? openArchive(path) : null);
  openQueue = task.catch(() => undefined);
  try {
    const info = await task;
    if (request !== selection) return null;
    opening = false;
    useStore.getState().setCurrentArchive(info);
    return info;
  } catch (error) {
    if (request !== selection) return null;
    opening = false;
    useStore.getState().setError(error instanceof Error ? error.message : String(error));
    throw error;
  }
}

/** Startup must not overwrite a folder already selected by the user. */
export async function restoreArchive(): Promise<void> {
  const request = selection;
  const initial = useStore.getState().currentArchive;
  try {
    const info = await currentArchive();
    if (request === selection && !opening && useStore.getState().currentArchive === initial) {
      useStore.getState().setCurrentArchive(info);
    }
  } catch (error) {
    if (request === selection && !opening && useStore.getState().currentArchive === initial) {
      useStore.getState().setError(error instanceof Error ? error.message : String(error));
    }
  }
}

/** One scan per active archive session; late results/errors are discarded. */
export function reloadArchive(options: { fresh?: boolean } = {}): Promise<boolean> {
  const archive = useStore.getState().currentArchive;
  if (!archive || opening) return Promise.resolve(false);
  const request = selection;
  const active = () => request === selection && useStore.getState().currentArchive === archive;
  const previous = scans.get(archive);
  if (previous) return options.fresh
    ? previous.then(() => active() ? reloadArchive() : false)
    : previous;
  useStore.setState({ error: null, status: "loading" });
  const task = (async () => {
    try {
      const result = await scanFolder(archive.root);
      if (!active()) return false;
      useStore.getState().setScanResult(result);
      return true;
    } catch (error) {
      if (active()) useStore.getState().setError(error instanceof Error ? error.message : String(error));
      return false;
    }
  })();
  scans.set(archive, task);
  void task.then(() => scans.delete(archive));
  return task;
}
