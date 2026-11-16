import { expect, test } from "vitest";
import { encoderLabel, verifiedEncoderLabel } from "./encoderLabel";

test("verified encoder display agrees with native preference and old capability payloads", () => {
  const base = { version: "ffmpeg", nvencHevc: false };
  expect(verifiedEncoderLabel(base)).toBe("CPU / HEVC");
  expect(verifiedEncoderLabel({ ...base, amfHevc: true })).toBe("AMD / AMF");
  expect(verifiedEncoderLabel({ ...base, vaapiDevice: "/dev/dri/renderD129" })).toBe("GPU / VA-API");
  expect(verifiedEncoderLabel({ ...base, nvencHevc: true, amfHevc: true })).toBe("NVIDIA / NVENC");
  expect(encoderLabel("hevc_amf")).toBe("AMD / AMF");
  expect(encoderLabel("other_encoder")).toBe("other_encoder");
});
