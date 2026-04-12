import { useEffect, useState } from "react";

const STORE_LINK = "ms-windows-store://pdp/?productid=9N4WGH0Z6VHQ";

function checkHevcSupport(): boolean {
  const video = document.createElement("video");
  const result = video.canPlayType('video/mp4; codecs="hvc1.1.6.L150.B0"');
  return result !== "";
}

export function HevcSupportGate({ children }: { children: React.ReactNode }) {
  const [supported, setSupported] = useState<boolean | null>(null);
  const [dismissed, setDismissed] = useState(false);

  useEffect(() => {
    setSupported(checkHevcSupport());
  }, []);

  if (supported === null) return null;
  if (supported || dismissed) return <>{children}</>;

  return (
    <div className="flex h-full items-center justify-center bg-neutral-950 p-8">
      <div className="max-w-md rounded-lg border border-yellow-800 bg-yellow-950/50 p-6 text-center">
        <h2 className="text-lg font-semibold text-yellow-200">
          HEVC Video Extension Required
        </h2>
        <p className="mt-3 text-sm text-neutral-300">
          Your dashcam files use HEVC (H.265) encoding. Windows requires the
          Microsoft HEVC Video Extension to play these files.
        </p>
        <div className="mt-4 flex flex-col gap-2">
          <a
            href={STORE_LINK}
            className="rounded-md bg-blue-600 px-4 py-2 text-sm font-medium text-white transition-colors hover:bg-blue-500"
          >
            Open Microsoft Store
          </a>
          <button
            onClick={() => setDismissed(true)}
            className="rounded-md px-4 py-2 text-sm text-neutral-400 transition-colors hover:text-neutral-200"
          >
            Continue anyway
          </button>
        </div>
        <p className="mt-4 text-xs text-neutral-500">
          After installing the extension, restart Trip Viewer.
        </p>
      </div>
    </div>
  );
}
