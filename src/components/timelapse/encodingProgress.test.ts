import { describe, expect, it } from "vitest";
import { estimateEncodingStep } from "./encodingProgress";

const progress = {
  tripId: "trip", tier: "8x", channel: "F", phase: "encoding" as const,
  encoder: "libx265", processedSeconds: 20, totalSeconds: 100,
  elapsedSeconds: 10, receivedAtMs: 1_000,
};

describe("encoding step ETA", () => {
  it("estimates from actual video progress before any whole job finishes", () => {
    expect(estimateEncodingStep(progress, 2_000)).toMatchObject({ percent: 20, remainingSeconds: 40 });
  });
  it("does not estimate from zero progress or encoder warmup", () => {
    expect(estimateEncodingStep({ ...progress, processedSeconds: 0 }, 2_000)?.remainingSeconds).toBeNull();
    expect(estimateEncodingStep({ ...progress, elapsedSeconds: 1 }, 2_000)?.remainingSeconds).toBeNull();
  });
  it("withholds a stale ETA rather than counting down while progress stalls", () => {
    expect(estimateEncodingStep(progress, 20_000)).toMatchObject({ stale: true, remainingSeconds: null });
  });
  it("bounds timestamps and treats completion as finalization, not a new estimate", () => {
    expect(estimateEncodingStep({ ...progress, processedSeconds: -5 }, 2_000)?.percent).toBe(0);
    expect(estimateEncodingStep({ ...progress, processedSeconds: 110 }, 2_000)).toMatchObject({ percent: 100, remainingSeconds: null });
    expect(estimateEncodingStep({ ...progress, totalSeconds: 0 }, 2_000)).toBeNull();
    expect(estimateEncodingStep({ ...progress, elapsedSeconds: NaN }, 2_000)).toBeNull();
  });
});
