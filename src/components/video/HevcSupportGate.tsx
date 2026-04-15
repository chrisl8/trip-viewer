import { useEffect, useState } from "react";

const STORE_LINK = "ms-windows-store://pdp/?productid=9N4WGH0Z6VHQ";

function checkHevcSupport(): boolean {
  const video = document.createElement("video");
  const result = video.canPlayType('video/mp4; codecs="hvc1.1.6.L150.B0"');
  return result !== "";
}

type Platform = "windows" | "linux" | "mac" | "unknown";

function detectPlatform(): Platform {
  const ua = navigator.userAgent;
  if (ua.includes("Windows")) return "windows";
  if (ua.includes("Mac OS X")) return "mac";
  if (ua.includes("Linux") && !ua.includes("Android")) return "linux";
  return "unknown";
}

export function HevcSupportGate({ children }: { children: React.ReactNode }) {
  const [supported, setSupported] = useState<boolean | null>(null);
  const [dismissed, setDismissed] = useState(false);

  useEffect(() => {
    setSupported(checkHevcSupport());
  }, []);

  if (supported === null) return null;
  if (supported || dismissed) return <>{children}</>;

  const platform = detectPlatform();

  return (
    <div className="flex h-full items-center justify-center bg-neutral-950 p-8">
      <div className="max-w-md rounded-lg border border-yellow-800 bg-yellow-950/50 p-6 text-center">
        <h2 className="text-lg font-semibold text-yellow-200">
          HEVC Video Support Required
        </h2>
        {platform === "windows" ? (
          <>
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
          </>
        ) : platform === "linux" ? (
          <>
            <p className="mt-3 text-sm text-neutral-300">
              Your dashcam files use HEVC (H.265) encoding. Linux needs
              GStreamer's libav plugin to decode H.265 through WebKitGTK.
            </p>
            <p className="mt-3 overflow-x-auto rounded-md bg-neutral-900 p-2 text-left font-mono text-xs text-neutral-300">
              sudo apt install gstreamer1.0-libav gstreamer1.0-plugins-bad
            </p>
            <button
              onClick={() => setDismissed(true)}
              className="mt-4 rounded-md px-4 py-2 text-sm text-neutral-400 transition-colors hover:text-neutral-200"
            >
              Continue anyway
            </button>
            <p className="mt-4 text-xs text-neutral-500">
              After installing the packages, restart Trip Viewer. If you're
              running the Flatpak build, HEVC should already work — report this
              as a bug.
            </p>
          </>
        ) : (
          <>
            <p className="mt-3 text-sm text-neutral-300">
              Your dashcam files use HEVC (H.265) encoding, which this system
              cannot currently decode.
            </p>
            <button
              onClick={() => setDismissed(true)}
              className="mt-4 rounded-md px-4 py-2 text-sm text-neutral-400 transition-colors hover:text-neutral-200"
            >
              Continue anyway
            </button>
          </>
        )}
      </div>
    </div>
  );
}
