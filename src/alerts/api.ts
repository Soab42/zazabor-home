import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** Alerts' page side: the `alerts_*` commands and events of `src-tauri/src/alerts`. */

export type AlertKind = "call" | "message";

export interface Alert {
  id: number;
  kind: AlertKind;
  key: string;
  /** "phone", "sms", or a Messages service id. */
  service: string;
  title: string;
  body: string;
  video: boolean;
  at: number;
  accountId: string | null;
  accountName: string | null;
  phoneId: string | null;
  number: string | null;
  via: "hfp" | "app" | "web" | "";
  note: string | null;
  silent: boolean;
  canReply: boolean;
  canCall: boolean;
  canRead: boolean;
  demo: boolean;
}

export type Position = "bottom-right" | "bottom-left" | "top-right" | "top-left";

export interface AlertSettings {
  position: Position;
  toastSecs: number;
  ringtone: boolean;
  volume: number;
  dnd: boolean;
  previews: boolean;
  phoneCalls: boolean;
  sms: boolean;
  whatsapp: boolean;
  teams: boolean;
  messenger: boolean;
  otherMessages: boolean;
  callKeys: boolean;
  quickReplies: string[];
  callWindowHintShown: boolean;
}

export interface AlertsState {
  version: number;
  alerts: Alert[];
  hidden: number;
  settings: AlertSettings;
}

export const alertsApi = {
  state: () => invoke<AlertsState>("alerts_state"),
  ready: () => invoke<void>("alerts_ready"),
  resize: (height: number) => invoke<void>("alerts_resize", { height }),
  hover: (id: number, on: boolean) => invoke<void>("alerts_hover", { id, on }),
  dismiss: (id: number) => invoke<void>("alerts_dismiss", { id }),
  answer: (id: number) => invoke<void>("alerts_answer", { id }),
  decline: (id: number) => invoke<void>("alerts_decline", { id }),
  reply: (id: number, text: string) => invoke<void>("alerts_reply", { id, text }),
  callMessage: (id: number, text: string) => invoke<void>("alerts_call_message", { id, text }),
  call: (id: number) => invoke<void>("alerts_call", { id }),
  read: (id: number) => invoke<void>("alerts_read", { id }),
  open: (id: number) => invoke<void>("alerts_open", { id }),
  hotkey: (action: "answer" | "decline") => invoke<void>("alerts_hotkey", { action }),
  settings: () => invoke<AlertSettings>("alerts_settings"),
  setSettings: (settings: AlertSettings) => invoke<AlertSettings>("alerts_set_settings", { settings }),
};

export const onAlertsState = (fn: (state: AlertsState) => void): Promise<UnlistenFn> => listen<AlertsState>("alerts-state", (e) => fn(e.payload));
export const onAlertsSettings = (fn: (settings: AlertSettings) => void): Promise<UnlistenFn> => listen<AlertSettings>("alerts-settings", (e) => fn(e.payload));
export const errorText = (err: unknown) => (err instanceof Error ? err.message : String(err));
