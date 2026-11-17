/** Preserve the current clock whenever it is healthy, including after rejoin. */
export function healthyLineup(labels: string[], failed: ReadonlySet<string>, master: string | null): string[] {
  const healthy = labels.filter((label) => !failed.has(label));
  return master && healthy.includes(master)
    ? [master, ...healthy.filter((label) => label !== master)]
    : healthy;
}
