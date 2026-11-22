import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/**
 * Messages' page side: typed wrappers for the `msg_*` commands and events of
 * docs/plans/messages.md (the Rust side is `src-tauri/src/messages`).
 */

export interface Account {
  id: string;
  service: string;
  url: string;
  name: string;
  order: number;
  muted: boolean;
  /** Minutes unused before it sleeps; null keeps it always on. */
  sleepAfter: number | null;
  zoom: number;
  /** "panel": inside Zazabor; "browser": its own browser app window (no unread counts or notifications from Zazabor). */
  mode: "panel" | "browser";
  /** The colour scheme the page is told the system has (light by default). */
  appearance: "light" | "dark" | "system";
  unread: number;
  /** Its panel is alive (not sleeping, and opened at least once since the app started). */
  awake: boolean;
}

export interface Settings {
  dnd: boolean;
  previews: boolean;
  defaultSleep: number;
  callsHintShown: boolean;
}

export interface State {
  accounts: Account[];
  settings: Settings;
  shown: string | null;
}

/** A rectangle in the window, in CSS pixels from its top-left corner. */
export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface AccountPatch {
  name?: string;
  muted?: boolean;
  sleepAfter?: number | null;
  zoom?: number;
  mode?: "panel" | "browser";
  appearance?: "light" | "dark" | "system";
}

export const msg = {
  state: () => invoke<State>("msg_state"),
  add: (service: string, name: string, url?: string, mode: "panel" | "browser" = "panel") => invoke<Account>("msg_add", { service, name, url: url ?? null, mode }),
  openBrowserWindow: (id: string) => invoke<void>("msg_open_browser_window", { id }),
  browserName: () => invoke<string | null>("msg_browser_name"),
  update: (id: string, patch: AccountPatch) => invoke<Account>("msg_update", { id, patch }),
  reorder: (ids: string[]) => invoke<void>("msg_reorder", { ids }),
  remove: (id: string, wipe: boolean) => invoke<void>("msg_remove", { id, wipe }),
  show: (id: string, rect: Rect) => invoke<void>("msg_show", { id, rect }),
  hide: () => invoke<void>("msg_hide"),
  rect: (rect: Rect) => invoke<void>("msg_rect", { rect }),
  reload: (id: string) => invoke<void>("msg_reload", { id }),
  hibernate: (id: string) => invoke<void>("msg_hibernate", { id }),
  openExternal: (id: string) => invoke<void>("msg_open_external", { id }),
  snapshot: (id: string) => invoke<string>("msg_snapshot", { id }),
  settings: (patch: Partial<Pick<Settings, "dnd" | "previews" | "defaultSleep">>) => invoke<Settings>("msg_settings", { patch }),
  dataSize: () => invoke<number>("msg_data_size"),
  clearData: () => invoke<void>("msg_clear_data"),
  start: () => invoke<void>("msg_start"),
};

export interface Events {
  "msg-changed": null;
  "msg-unread": { id: string; count: number };
  "msg-notified": { id: string };
  "msg-open": { id: string };
  "msg-key": { key: string };
  "msg-call-browser": { id: string };
  "msg-hint": { text: string };
  "msg-download": { ok: boolean; name: string; reason?: string };
}

export function onMsg<K extends keyof Events>(event: K, fn: (payload: Events[K]) => void): Promise<UnlistenFn> {
  return listen<Events[K]>(event, (e) => fn(e.payload));
}

export const errorText = (err: unknown): string => (err instanceof Error ? err.message : String(err));
