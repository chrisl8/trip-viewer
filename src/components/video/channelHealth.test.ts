import { describe, expect, it } from "vitest";
import { healthyLineup } from "./channelHealth";

describe("channel-local failover", () => {
  const labels = ["Front", "Interior", "Rear"];
  const orders = labels.flatMap((first) => labels.filter((second) => second !== first)
    .map((second) => [first, second, labels.find((last) => last !== first && last !== second)!]));
  for (const order of orders) {
    it(`isolates independent failures and preserves the promoted master: ${order}`, () => {
      const failed = new Set<string>();
      let master: string | null = labels[0];
      for (const label of order) {
        failed.add(label);
        const lineup = healthyLineup(labels, failed, master);
        expect(lineup).toHaveLength(labels.length - failed.size);
        expect(lineup.every((candidate) => !failed.has(candidate))).toBe(true);
        master = lineup[0] ?? null;
      }
      expect(master).toBeNull();
      // First recovered camera becomes the clock; later cameras rejoin as slaves.
      failed.delete(order[1]);
      master = healthyLineup(labels, failed, master)[0];
      failed.clear();
      expect(healthyLineup(labels, failed, master)[0]).toBe(order[1]);
      // Only the next source boundary restores canonical ordering.
      expect(healthyLineup(labels, new Set(), null)).toEqual(labels);
    });
  }
  it("does not switch back when the canonical master recovers", () => {
    expect(healthyLineup(labels, new Set(), "Interior")).toEqual(["Interior", "Front", "Rear"]);
  });
});
