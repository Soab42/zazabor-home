import type { ShellContext } from "../../shell/module";
import { isMainWindow } from "../../shell/windows";
import { mountAlertKeys } from "../../alerts/keys";
import { showToast } from "../files/ops/toast";
import { msg, onMsg } from "./api";
import { startStore } from "./store";

/**
 * Messages in the main window, whatever section is open: starts the "always on" accounts so their
 * notifications arrive, opens an account when its desktop notification is clicked, and tells about
 * downloads and calls. Started by the shell.
 */

let started = false;

/** The account a notification asked for, until the section takes it. */
export let pendingOpen: string | null = null;
export const takePendingOpen = (): string | null => {
  const id = pendingOpen;
  pendingOpen = null;
  return id;
};
const openers = new Set<(id: string) => void>();
export const onOpenRequest = (fn: (id: string) => void): (() => void) => {
  openers.add(fn);
  return () => openers.delete(fn);
};

export function startMessagesService(shell: ShellContext) {
  if (started || !isMainWindow) return;
  started = true;
  mountAlertKeys();
  void startStore().then(() => msg.start()).catch((err) => console.warn("Messages:", err));
  void onMsg("msg-open", ({ id }) => {
    if (!shell.config.features.messages) return;
    pendingOpen = id;
    shell.showModule("messages");
    for (const fn of openers) fn(id);
  });
  void onMsg("msg-hint", ({ text }) => showToast(text, undefined, 9000));
  void onMsg("msg-download", ({ ok, name, reason }) => {
    if (ok) showToast(`Downloaded ${name} to your Downloads folder`, undefined, 6000);
    else if (reason === "program") showToast(`Blocked a program file (${name}). Open the page in your browser to get it.`, undefined, 8000);
    else showToast("The download didn't finish");
  });
}
