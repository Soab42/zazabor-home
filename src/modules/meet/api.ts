import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { Account, Address, Overview } from "../mail/api";

/**
 * Meet's page side: typed wrappers for the `meet_*` commands and events of docs/plans/meet.md
 * (the Rust side is `src-tauri/src/meet`). Times are Unix ms.
 *
 * A dev build can run the whole section without a backend: open the page with `?meetmock` (or set
 * localStorage `zazabor.meetMock` to "1"; "bare" starts with nothing and the outside link off).
 * That mock (`mock.ts`) is never part of a release build.
 */

// ---- types --------------------------------------------------------------------------------------

export interface Invitee {
  email: string;
  name: string;
  sentAt: number | null;
  error: string | null;
}

export interface Meeting {
  id: string;
  /** 10 characters from [a-z2-9]; shown as abc-defg-hij. */
  code: string;
  title: string;
  /** A call starts with cameras off. */
  kind: "meeting" | "call";
  /** Unix ms; null for an instant meeting ("Meet now"). */
  startsAt: number | null;
  minutes: number;
  autoRecord: boolean;
  lobby: boolean;
  invitees: Invitee[];
  createdAt: number;
  live: boolean;
  hostIn: boolean;
  people: number;
  waiting: number;
  recording: boolean;
}

export interface Recording {
  id: string;
  meetingId: string;
  title: string;
  path: string;
  startedAt: number;
  endedAt: number | null;
  bytes: number;
}

export interface Tunnel {
  state: "off" | "starting" | "on" | "error";
  url: string | null;
  error: string | null;
}

export interface Turn {
  url: string;
  username: string;
  credential: string;
}

export interface MeetState {
  port: number;
  tunnel: Tunnel;
  /** The browser the host's window opens in, e.g. "Google Chrome"; null: the default browser. */
  browser: string | null;
  recordingsDir: string;
  stun: string[];
  turn: Turn | null;
  /** "quick": a random trycloudflare.com address; "domain": the user's own, through a named Cloudflare tunnel. */
  mode: "quick" | "domain";
  hostname: string;
  /** Whether a tunnel token is saved. The token itself is never sent to the page. */
  tokenSet: boolean;
}

/** What Settings' "Test" found at `https://<hostname>/api/health`. */
export interface DomainCheck {
  result: "this-pc" | "other" | "unreachable";
  message: string;
}

/** What the sheets send to `meet_create` and `meet_update` (the latter with the `id`). */
export interface MeetingInput {
  title: string;
  kind: "meeting" | "call";
  startsAt: number | null;
  minutes: number;
  autoRecord: boolean;
  lobby: boolean;
  invitees: { email: string; name: string }[];
}

/** Settings → Meet → Transcripts and notes. */
export interface AiSettings {
  /** Transcribe and take notes when a meeting ends. */
  auto: boolean;
  /** "auto": small, medium once Bangla is heard. */
  model: "auto" | "small" | "medium";
  notesLang: "en" | "bn" | "both";
  /** Translate English speech into Bangla subtitles (slow: the notes model does it). */
  bnSubs: boolean;
  /** "local": on this PC (the default). "cloud": the user's own API keys. */
  processing: "local" | "cloud";
  /** The user confirmed that audio and transcript text go to the chosen providers. */
  cloudConsent: boolean;
  /** If the cloud fails, do the job on this PC instead. */
  fallbackLocal: boolean;
  /** Speech to text in the cloud (Claude takes no audio). */
  sttProvider: CloudId;
  /** Translation and notes in the cloud. */
  llmProvider: CloudId;
  /** The model the user chose, by `<provider>.stt` or `<provider>.llm`; missing means the default. */
  cloudModels: Record<string, string>;
  /** Bangla is available: on this PC when its models can, else only with a cloud speech-to-text provider. Set by Zazabor, ignored when saving. */
  banglaOn: boolean;
}

export type CloudId = "claude" | "openai" | "gemini" | "openrouter";

/** A cloud provider and whether a key is saved for it. The key itself never reaches the page. */
export interface CloudProvider {
  id: CloudId;
  label: string;
  name: string;
  canTranscribe: boolean;
  keySet: boolean;
  keyPage: string;
  sttDefault: string | null;
  llmDefault: string;
}

/** What the card of a recording shows about its transcript job. */
export interface AiInfo {
  id: string;
  /** Per-speaker audio tracks saved (0: only the mixed sound is there). */
  tracks: number;
  state: "none" | "queued" | "running" | "done" | "error" | "cancelled" | "off" | "needs-models";
  stage: string;
  /** 0…1 */
  progress: number;
  message: string;
  hasTranscript: boolean;
  hasNotes: boolean;
  /** Model ids a "needs-models" job waits for ("small", "medium", "qwen"). */
  missing: string[];
  /** Minutes of audio a cloud run would send (0 while Cloud is off). */
  sendMinutes: number;
  /** "OpenAI", "Gemini"… when a run would send the sound to a cloud provider, else empty. */
  cloud: string;
}

export interface AiModel {
  id: string;
  label: string;
  note: string;
  bytes: number;
  installed: boolean;
  partial: number;
  downloading: boolean;
  error: string | null;
}

export interface TranscriptSegment {
  speaker: string;
  track: string;
  /** Seconds from the start of the recording. */
  start: number;
  end: number;
  /** What was spoken: "en" or "bn". */
  lang: string;
  text: string;
  en: string | null;
  bn: string | null;
}

export interface TranscriptDoc {
  version: number;
  model: string;
  /** "en", "bn" or "mixed". */
  language: string;
  duration: number;
  segments: TranscriptSegment[];
}

export interface NotesAction {
  owner: string;
  task: string;
  due: string | null;
}

export interface NotesChapter {
  start: number;
  end: number;
  title: string;
  summary: string;
}

/** The notes in one language. */
export interface Notes {
  summary: string;
  decisions: string[];
  actions: NotesAction[];
  questions: string[];
  chapters: NotesChapter[];
}

export interface NotesDoc {
  version: number;
  language: "en" | "bn" | "both";
  model: string;
  /** The cloud provider that wrote them ("claude", "openai", …); missing when the local model did. */
  provider?: CloudId;
  generatedAt: number;
  edited: boolean;
  en: Notes | null;
  bn: Notes | null;
}

export interface Events {
  "meet-changed": null;
  "meet-knock": { meetingId: string; title: string; name: string };
  /** A transcript job moved on (`id`, with state, stage, progress and message), or a model download ended (`models`). */
  "meet-ai": { id?: string; state?: AiInfo["state"]; stage?: string; progress?: number; message?: string; models?: boolean };
}

// ---- the dev mock -------------------------------------------------------------------------------

/** What the mock exposes; `mock.ts` implements it. */
export interface MeetMock {
  invoke(cmd: string, args: Record<string, unknown>): Promise<unknown>;
  listen<K extends keyof Events>(event: K, fn: (payload: Events[K]) => void): () => void;
}

function mockWanted(): string | null {
  try {
    const query = new URLSearchParams(location.search);
    if (query.has("meetmock")) return query.get("meetmock") || "1";
    return localStorage.getItem("zazabor.meetMock");
  } catch {
    return null;
  }
}

/** Only dev builds look for the mock; in a release `import.meta.env.DEV` is false and it's dropped. */
const mock: Promise<MeetMock> | null = import.meta.env.DEV && mockWanted()
  ? import("./mock").then((m) => m.createMock(mockWanted() ?? "1"))
  : null;

export const isMock = mock !== null;

function call<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> {
  if (mock) return mock.then((m) => m.invoke(cmd, args) as Promise<T>);
  return invoke<T>(cmd, args);
}

/** Listens to a Meet event; resolves to the function that stops listening. */
export function onMeet<K extends keyof Events>(event: K, fn: (payload: Events[K]) => void): Promise<UnlistenFn> {
  if (mock) return mock.then((m) => m.listen(event, fn));
  return listen<Events[K]>(event, (e) => fn(e.payload));
}

// ---- commands -----------------------------------------------------------------------------------

export const meetApi = {
  state: () => call<MeetState>("meet_state"),
  list: () => call<Meeting[]>("meet_list"),
  create: (input: MeetingInput) => call<Meeting>("meet_create", { ...input }),
  update: (id: string, input: MeetingInput) => call<Meeting>("meet_update", { id, ...input }),
  remove: (id: string) => call<void>("meet_delete", { id }),
  /** The link to share; starts the outside link first when it's off (can take up to 20 s). */
  joinUrl: (id: string) => call<string>("meet_join_url", { id }),
  open: (id: string) => call<void>("meet_open", { id }),
  invite: (id: string, accountId: string, message: string) => call<Invitee[]>("meet_invite", { id, accountId, message }),
  end: (id: string) => call<void>("meet_end", { id }),
  recordings: () => call<Recording[]>("meet_recordings"),
  recordingDelete: (id: string) => call<void>("meet_recording_delete", { id }),
  // Transcripts, subtitles and notes (docs/plans/meet-notes.md).
  aiSettings: () => call<AiSettings>("meet_ai_settings"),
  setAiSettings: (settings: AiSettings) => call<AiSettings>("meet_ai_set_settings", { settings }),
  aiStatus: () => call<AiInfo[]>("meet_ai_status"),
  aiModels: () => call<AiModel[]>("meet_ai_models"),
  aiModelDownload: (id: string) => call<void>("meet_ai_model_download", { id }),
  aiModelCancel: (id: string) => call<void>("meet_ai_model_cancel", { id }),
  aiModelRemove: (id: string) => call<void>("meet_ai_model_remove", { id }),
  /** Transcribes (and writes the notes); `redo: "notes"` keeps the transcript. */
  aiRun: (id: string, options?: { redo?: "all" | "notes"; model?: AiSettings["model"]; processing?: "local" }) => call<void>("meet_ai_run", { id, options: options ?? null }),
  aiCancel: (id: string) => call<void>("meet_ai_cancel", { id }),
  // Cloud AI (Claude, OpenAI, Gemini, OpenRouter): keys live in the system keyring; the page only learns `keySet`.
  aiCloudState: () => call<CloudProvider[]>("meet_ai_cloud_state"),
  aiCloudKeySet: (provider: CloudId, key: string) => call<CloudProvider[]>("meet_ai_cloud_key_set", { provider, key }),
  aiCloudKeyClear: (provider: CloudId) => call<CloudProvider[]>("meet_ai_cloud_key_clear", { provider }),
  /** A tiny request with the saved key: resolves with a sentence, rejects with why it failed (invalid key, no credit, no connection). */
  aiCloudTest: (provider: CloudId) => call<string>("meet_ai_cloud_test", { provider }),
  /** Switches the automatic transcript off (or on again) for one meeting. */
  aiOff: (id: string, off: boolean) => call<void>("meet_ai_off", { id, off }),
  transcript: (id: string) => call<TranscriptDoc>("meet_transcript", { id }),
  notes: (id: string) => call<NotesDoc | null>("meet_notes", { id }),
  notesSave: (id: string, notes: { en?: Notes | null; bn?: Notes | null }) => call<void>("meet_notes_save", { id, notes }),
  tunnel: (on: boolean) => call<MeetState>("meet_tunnel", { on }),
  settings: (stun: string[], turn: Turn | null) => call<MeetState>("meet_settings", { stun, turn }),
  /** Quick link or own domain. Omit `hostname`/`token` to keep what's saved; `clearToken` forgets the token. */
  domain: (input: { mode: "quick" | "domain"; hostname?: string; token?: string; clearToken?: boolean }) => call<MeetState>("meet_domain", { ...input }),
  /** Fetches the domain's /api/health from this PC (starts the tunnel first when it's off; up to 30 s). */
  domainTest: () => call<DomainCheck>("meet_domain_test"),
  // Mail's accounts and contacts, through the mock in a dev build.
  mailAccounts: () => call<Overview>("mail_overview").then((o): Account[] => o.accounts),
  mailContacts: (prefix: string) => call<Address[]>("mail_contacts", { prefix }),
};

// ---- helpers ------------------------------------------------------------------------------------

export function errorText(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (typeof err === "string") return err;
  if (err && typeof err === "object" && "message" in err) return String((err as { message: unknown }).message);
  return String(err);
}

/** abc-defg-hij */
export function codeText(code: string): string {
  return code.length === 10 ? `${code.slice(0, 3)}-${code.slice(3, 7)}-${code.slice(7)}` : code;
}

const timeFormat = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" });
const dayFormat = new Intl.DateTimeFormat(undefined, { weekday: "short", month: "short", day: "numeric" });
const yearDayFormat = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", year: "numeric" });

const startOfDay = (ms: number) => new Date(ms).setHours(0, 0, 0, 0);

/** "Today", "Tomorrow", "Yesterday", "Fri, Nov 21" (with the year when it isn't this one). */
export function dayText(ms: number, now = Date.now()): string {
  const days = Math.round((startOfDay(ms) - startOfDay(now)) / 86_400_000);
  if (days === 0) return "Today";
  if (days === 1) return "Tomorrow";
  if (days === -1) return "Yesterday";
  return (new Date(ms).getFullYear() === new Date(now).getFullYear() ? dayFormat : yearDayFormat).format(ms);
}

/** "Today 3:00 PM · 30 min"; an instant meeting shows when it was made. */
export function whenText(m: Meeting): string {
  const at = m.startsAt ?? m.createdAt;
  const length = m.startsAt !== null && m.minutes > 0 ? ` · ${durationText(m.minutes * 60)}` : "";
  return `${dayText(at)} ${timeFormat.format(at)}${length}`;
}

export function clockText(ms: number): string {
  return timeFormat.format(ms);
}

/** "1:15" or "1:02:05". */
export function timeText(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  const h = Math.floor(s / 3600);
  const mm = Math.floor(s / 60) % 60;
  const ss = String(s % 60).padStart(2, "0");
  return h ? `${h}:${String(mm).padStart(2, "0")}:${ss}` : `${mm}:${ss}`;
}

/** "30 min", "1 h 30 min". */
export function durationText(seconds: number): string {
  const minutes = Math.max(1, Math.round(seconds / 60));
  if (minutes < 60) return `${minutes} min`;
  const rest = minutes % 60;
  return rest ? `${Math.floor(minutes / 60)} h ${rest} min` : `${Math.floor(minutes / 60)} h`;
}

/** Past once it is over: its planned end (or an hour after an instant start) has gone by and nobody is in it. */
export function isPast(m: Meeting, now = Date.now()): boolean {
  if (m.live) return false;
  const start = m.startsAt ?? m.createdAt;
  const end = start + Math.max(m.minutes, 30) * 60_000;
  return m.startsAt === null ? now > start + 60 * 60_000 : now > end;
}
