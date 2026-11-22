/**
 * The order of the rail's sections. Pure functions on module ids (no imports), so they can be
 * tested with `node --test scripts/order.test.ts`.
 */

/** A saved order read from the config: only known ids, each once, in the saved order. */
export function normalizeOrder(raw: unknown, all: readonly string[]): string[] {
  if (!Array.isArray(raw)) return [];
  const seen = new Set<string>();
  for (const id of raw) if (typeof id === "string" && all.includes(id)) seen.add(id);
  return [...seen];
}

/** The full order: the saved ids first, then every section the saved order doesn't know, in the built-in order. */
export function fullOrder(saved: readonly string[], all: readonly string[]): string[] {
  const known = normalizeOrder(saved, all);
  return [...known, ...all.filter((id) => !known.includes(id))];
}

/** The enabled sections, in the order the user chose (hidden ones stay out; new ones come last). */
export function orderEnabled<T extends string>(enabled: readonly T[], saved: readonly string[], all: readonly string[]): T[] {
  const rank = new Map(fullOrder(saved, all).map((id, i) => [id, i]));
  return [...enabled].sort((a, b) => (rank.get(a) ?? all.length) - (rank.get(b) ?? all.length));
}

/**
 * A new saved order with `id` moved to sit at position `to` among the `visible` ones (the
 * hidden sections keep their places). `to` is clamped to the visible range.
 */
export function moveTo(visible: readonly string[], id: string, to: number, saved: readonly string[], all: readonly string[]): string[] {
  const order = fullOrder(saved, all);
  if (!visible.includes(id)) return order;
  // `visible` in the saved order, then the moved one placed; those take the slots the visible ones held.
  const ranked = order.filter((v) => visible.includes(v));
  const next = ranked.filter((v) => v !== id);
  next.splice(Math.max(0, Math.min(to, next.length)), 0, id);
  const result = [...order];
  let i = 0;
  order.forEach((v, slot) => {
    if (ranked.includes(v)) result[slot] = next[i++];
  });
  return result;
}
