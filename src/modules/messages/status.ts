import type { RailStatus } from "../../shell/module";
import { startStore, subscribe, totalUnread } from "./store";

/**
 * The Messages button's badge on the rail: the unread total over all accounts that aren't muted.
 * Loaded with the shell, so it stays small (no UI).
 */
export function watchMessagesStatus(show: (status: RailStatus | null) => void): () => void {
  const update = () => {
    const total = totalUnread();
    show(total > 0 ? { tone: "on", count: total, title: `${total} unread ${total === 1 ? "message" : "messages"}` } : null);
  };
  const stop = subscribe(update);
  void startStore().then(update, () => undefined);
  return stop;
}
