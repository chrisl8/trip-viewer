import { CSSProperties, RefObject } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import type { Channel, ChannelKind, Segment } from "../../types/model";
import { ChannelPanel } from "./ChannelPanel";
import { useStore } from "../../state/store";

interface Props {
  frontRef: RefObject<HTMLVideoElement | null>;
  interiorRef: RefObject<HTMLVideoElement | null>;
  rearRef: RefObject<HTMLVideoElement | null>;
  activeSegment: Segment | null;
}

function channelByKind(segment: Segment, kind: ChannelKind): Channel | undefined {
  return segment.channels.find((c) => c.kind === kind);
}

function refForKind(
  kind: ChannelKind,
  frontRef: RefObject<HTMLVideoElement | null>,
  interiorRef: RefObject<HTMLVideoElement | null>,
  rearRef: RefObject<HTMLVideoElement | null>,
): RefObject<HTMLVideoElement | null> {
  switch (kind) {
    case "front":
      return frontRef;
    case "interior":
      return interiorRef;
    case "rear":
      return rearRef;
  }
}

/**
 * Compute CSS grid placement for a channel.
 * The primary channel spans the left column across both rows.
 * The two secondaries stack in the right column.
 */
function gridStyle(
  kind: ChannelKind,
  primaryChannel: ChannelKind,
  secondaryIndex: number,
): CSSProperties {
  if (kind === primaryChannel) {
    return { gridColumn: 1, gridRow: "1 / 3" };
  }
  return { gridColumn: 2, gridRow: secondaryIndex + 1 };
}

export function VideoGrid({
  frontRef,
  interiorRef,
  rearRef,
  activeSegment,
}: Props) {
  const primaryChannel = useStore((s) => s.primaryChannel);
  const setPrimaryChannel = useStore((s) => s.setPrimaryChannel);

  if (!activeSegment) {
    return (
      <div className="col-span-2 flex items-center justify-center text-sm text-neutral-500">
        Select a trip from the list to begin playback.
      </div>
    );
  }

  const allKinds: ChannelKind[] = ["front", "interior", "rear"];
  // Track secondary index for grid row assignment
  let secondaryIdx = 0;

  function handleMainDoubleClick() {
    if (document.fullscreenElement) {
      document.exitFullscreen();
    } else {
      const ref = refForKind(primaryChannel, frontRef, interiorRef, rearRef);
      ref.current?.requestFullscreen();
    }
  }

  return (
    <div className="col-span-2 grid grid-cols-[2fr_1fr] grid-rows-2 gap-2">
      {allKinds.map((kind) => {
        const channel = channelByKind(activeSegment, kind);
        if (!channel) return null;

        const isPrimary = kind === primaryChannel;
        const idx = isPrimary ? 0 : secondaryIdx++;
        const ref = refForKind(kind, frontRef, interiorRef, rearRef);

        return (
          <div key={kind} style={gridStyle(kind, primaryChannel, idx)}>
            <ChannelPanel
              ref={ref}
              kind={kind}
              src={convertFileSrc(channel.filePath)}
              isMaster={isPrimary}
              onClick={isPrimary ? undefined : () => setPrimaryChannel(kind)}
              onDoubleClick={isPrimary ? handleMainDoubleClick : undefined}
            />
          </div>
        );
      })}
    </div>
  );
}
