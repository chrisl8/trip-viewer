import { RefObject } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import type { Channel, ChannelKind, Segment } from "../../types/model";
import { ChannelPanel } from "./ChannelPanel";

interface Props {
  frontRef: RefObject<HTMLVideoElement | null>;
  interiorRef: RefObject<HTMLVideoElement | null>;
  rearRef: RefObject<HTMLVideoElement | null>;
  activeSegment: Segment | null;
}

function channelByKind(segment: Segment, kind: ChannelKind): Channel | undefined {
  return segment.channels.find((c) => c.kind === kind);
}

export function VideoGrid({
  frontRef,
  interiorRef,
  rearRef,
  activeSegment,
}: Props) {
  if (!activeSegment) {
    return (
      <div className="col-span-2 flex items-center justify-center text-sm text-neutral-500">
        Select a trip from the list to begin playback.
      </div>
    );
  }

  const front = channelByKind(activeSegment, "front");
  const interior = channelByKind(activeSegment, "interior");
  const rear = channelByKind(activeSegment, "rear");

  return (
    <>
      {front && (
        <ChannelPanel
          ref={frontRef}
          kind="front"
          src={convertFileSrc(front.filePath)}
          isMaster={true}
        />
      )}
      <div className="grid grid-rows-2 gap-2">
        {interior && (
          <ChannelPanel
            ref={interiorRef}
            kind="interior"
            src={convertFileSrc(interior.filePath)}
            isMaster={false}
          />
        )}
        {rear && (
          <ChannelPanel
            ref={rearRef}
            kind="rear"
            src={convertFileSrc(rear.filePath)}
            isMaster={false}
          />
        )}
      </div>
    </>
  );
}
