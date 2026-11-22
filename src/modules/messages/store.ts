import { msg, onMsg, type Account, type Settings, type State } from "./api";

/**
 * What the Messages section, the rail badge and the settings page share: the accounts with their
 * unread counts, kept up to date by the backend's events. Loaded once per window.
 */

let state: State | null = null;
let started: Promise<void> | null = null;
const subscribers = new Set<() => void>();

const emit = () => {
  for (const fn of subscribers) {
    try {
      fn();
    } catch (err) {
      console.warn("Messages:", err);
    }
  }
};

export const subscribe = (fn: () => void): (() => void) => {
  subscribers.add(fn);
  return () => subscribers.delete(fn);
};

export const getState = (): State | null => state;
export const accounts = (): Account[] => state?.accounts ?? [];
export const settings = (): Settings | null => state?.settings ?? null;
export const account = (id: string | null): Account | undefined => (id ? accounts().find((a) => a.id === id) : undefined);

export async function refresh(): Promise<void> {
  try {
    state = await msg.state();
  } catch (err) {
    console.warn("Messages:", err);
    return;
  }
  emit();
}

/** The unread messages in all accounts that aren't muted (what the rail badge counts). */
export const totalUnread = (): number => accounts().reduce((sum, a) => sum + (a.muted ? 0 : a.unread), 0);

/** Loads the accounts and starts listening; safe to call many times. */
export function startStore(): Promise<void> {
  started ??= (async () => {
    await onMsg("msg-changed", () => void refresh());
    await onMsg("msg-unread", ({ id, count }) => {
      const a = account(id);
      if (!a || a.unread === count) return;
      a.unread = count;
      emit();
    });
    await refresh();
  })();
  return started;
}
