import type { FfmpegCapabilities } from "../../ipc/timelapse";

export function encoderLabel(encoder: string): string {
  return ({
    hevc_nvenc: "NVIDIA / NVENC", hevc_amf: "AMD / AMF",
    hevc_vaapi: "GPU / VA-API", libx265: "CPU / HEVC",
  } as Record<string, string>)[encoder] ?? encoder;
}

export function verifiedEncoderLabel(caps: FfmpegCapabilities): string {
  return encoderLabel(caps.nvencHevc ? "hevc_nvenc" : caps.amfHevc ? "hevc_amf"
    : caps.vaapiDevice ? "hevc_vaapi" : "libx265");
}
