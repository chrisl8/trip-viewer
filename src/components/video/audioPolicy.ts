import type { PlaybackSlice } from "../../state/store";

/**
 * WebKitGTK/GStreamer time-stretches multi-channel dashcam audio poorly at
 * browser playback rates other than 1x, and pre-rendered timelapse sources
 * already have a different time axis. For deterministic A/V sync, only the
 * canonical master is audible in Original mode at 1x.
 */
export function shouldMuteChannelAudio(
  canonicalAudioChannel: boolean,
  speed: PlaybackSlice["speed"],
  sourceMode: PlaybackSlice["sourceMode"],
): boolean {
  return !canonicalAudioChannel || speed !== 1 || sourceMode !== "original";
}
