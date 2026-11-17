export type FullscreenView =
  | { kind: "dashboard" }
  | { kind: "video"; label: string };

export type FullscreenStack = FullscreenView[];

export function sameFullscreenView(
  a: FullscreenView | undefined,
  b: FullscreenView,
): boolean {
  if (!a || a.kind !== b.kind) return false;
  if (a.kind === "dashboard") return true;
  return a.label === (b as { kind: "video"; label: string }).label;
}

/**
 * Push a logical fullscreen view. If the requested view is the immediately
 * previous one, treat that as back-navigation and pop instead of nesting an
 * identical layer. This gives consistent transitions without relying on
 * browser-nested Fullscreen API elements.
 */
export function openFullscreenView(
  stack: FullscreenStack,
  target: FullscreenView,
): FullscreenStack {
  const current = stack[stack.length - 1];
  if (sameFullscreenView(current, target)) return stack;

  const previous = stack[stack.length - 2];
  if (sameFullscreenView(previous, target)) return stack.slice(0, -1);

  return [...stack, target];
}

export function exitFullscreenView(stack: FullscreenStack): FullscreenStack {
  return stack.length === 0 ? stack : stack.slice(0, -1);
}

export function currentFullscreenView(
  stack: FullscreenStack,
): FullscreenView | null {
  return stack[stack.length - 1] ?? null;
}
