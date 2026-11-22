import type { Account, Address, Overview } from "../mail/api";
import type { AiInfo, AiModel, AiSettings, CloudProvider, DomainCheck, Events, MeetMock, MeetState, Meeting, MeetingInput, NotesDoc, Recording, TranscriptDoc } from "./api";

/**
 * A pretend backend for working on the Meet section without one (dev builds only; see api.ts).
 * Modes: "1" the tunnel on and some meetings, "bare" nothing yet and the outside link off.
 * `window.__meetMock.emit(event, payload)` fires an event, e.g. a knock.
 */
export function createMock(mode: string): MeetMock {
  const listeners = new Map<string, Set<(payload: never) => void>>();
  const emit = <K extends keyof Events>(event: K, payload: Events[K]) => {
    for (const fn of listeners.get(event) ?? []) (fn as (p: Events[K]) => void)(structuredClone(payload));
  };
  (window as unknown as { __meetMock: unknown }).__meetMock = { emit };
  const now = Date.now();
  const min = 60_000;
  const hour = 60 * min;
  const day = 24 * hour;
  const at = (daysFromNow: number, h: number, m = 0) => {
    const d = new Date(now + daysFromNow * day);
    d.setHours(h, m, 0, 0);
    return d.getTime();
  };
  const person = (name: string, sent: number | null = null, error: string | null = null) => ({ email: `${name.split(" ")[0].toLowerCase()}@example.com`, name, sentAt: sent, error });

  const state: MeetState = {
    port: 47230,
    tunnel: mode === "bare" ? { state: "off", url: null, error: null } : { state: "on", url: "https://quiet-harbor-glass-7f3.trycloudflare.com", error: null },
    browser: "Google Chrome", recordingsDir: "/home/soab/Videos/Zazabor Meetings",
    stun: ["stun:stun.l.google.com:19302", "stun:stun.cloudflare.com:3478"], turn: null,
    mode: "quick", hostname: "", tokenSet: false,
  };
  const quickUrl = "https://quiet-harbor-glass-7f3.trycloudflare.com";
  // The address guests get: the domain when it's on, else the quick tunnel's random one.
  const address = () => (state.mode === "domain" ? `https://${state.hostname}` : quickUrl);

  const base = { kind: "meeting" as const, autoRecord: true, lobby: true, hostIn: false, people: 0, waiting: 0, recording: false, live: false };
  let meetings: Meeting[] = mode === "bare" ? [] : [
    { ...base, id: "a1b2c3d4e5f60001", code: "kqmnwjt7xa", title: "Design review", startsAt: now - 12 * min, minutes: 45, createdAt: now - 2 * day, live: true, hostIn: true, people: 4, waiting: 2, recording: true, invitees: [person("Ayesha Rahman", now - 2 * day), person("Tanvir Hasan", now - 2 * day), person("Nadia Islam", now - 2 * day)] },
    { ...base, id: "a1b2c3d4e5f60002", code: "p4rd8cwh3m", title: "Weekly sync with the team", startsAt: now + 3 * hour, minutes: 30, createdAt: now - 5 * day, invitees: [person("Sadia Karim", now - 5 * day), person("Imran Chowdhury", null, "Mailbox full (552)"), person("Rafiq Ahmed", now - 5 * day)] },
    { ...base, id: "a1b2c3d4e5f60003", code: "z9h2vbnq5c", title: "Call with Mom", kind: "call", startsAt: at(1, 20, 0), minutes: 0, autoRecord: false, lobby: false, createdAt: now - day, invitees: [] },
    { ...base, id: "a1b2c3d4e5f60004", code: "d7fxe3gr2k", title: "Quarterly planning", startsAt: at(-2, 11, 0), minutes: 60, createdAt: now - 6 * day, invitees: [person("Ayesha Rahman", now - 6 * day), person("Sadia Karim", now - 6 * day)] },
    { ...base, id: "a1b2c3d4e5f60005", code: "m3tq6yaw8j", title: "Meeting", startsAt: null, minutes: 0, createdAt: now - 4 * day, invitees: [] },
    { ...base, id: "a1b2c3d4e5f60006", code: "w5nc4kpb9s", title: "Interview: front-end role", startsAt: at(-9, 15, 0), minutes: 45, createdAt: now - 11 * day, invitees: [person("Nadia Islam", now - 11 * day)] },
  ];
  const recordings: Recording[] = mode === "bare" ? [] : [
    { id: "r1", meetingId: "a1b2c3d4e5f60001", title: "Design review", path: "/home/soab/Videos/Zazabor Meetings/Design review 2026-11-21 10.00.webm", startedAt: now - 11 * min, endedAt: null, bytes: 38_400_000 },
    { id: "r2", meetingId: "a1b2c3d4e5f60004", title: "Quarterly planning", path: "/home/soab/Videos/Zazabor Meetings/Quarterly planning 2026-11-19 11.00.webm", startedAt: at(-2, 11, 0), endedAt: at(-2, 11, 0) + 58 * min, bytes: 612_000_000 },
    { id: "r3", meetingId: "a1b2c3d4e5f60006", title: "Interview: front-end role", path: "/home/soab/Videos/Zazabor Meetings/Interview front-end role 2026-11-12 15.00.webm", startedAt: at(-9, 15, 0), endedAt: at(-9, 15, 0) + 41 * min, bytes: 402_000_000 },
    { id: "r4", meetingId: "a1b2c3d4e5f60005", title: "Meeting", path: "/home/soab/Videos/Zazabor Meetings/Meeting 2026-11-17 09.20.webm", startedAt: at(-4, 9, 20), endedAt: at(-4, 9, 20) + 6 * min, bytes: 41_500_000 },
  ];

  // Transcripts and notes: r2 is done, r3 is being worked on, r4 waits for the models.
  // `?processing=cloud` in the mock mode ("1-cloud") starts with Cloud on, for looking at the settings.
  const cloudOn = mode.endsWith("cloud");
  const aiSettings: AiSettings = {
    auto: true, model: "auto", notesLang: "both", bnSubs: true,
    processing: cloudOn ? "cloud" : "local", cloudConsent: cloudOn, fallbackLocal: false, sttProvider: "openai", llmProvider: "claude", cloudModels: {}, banglaOn: cloudOn,
  };
  const providers: CloudProvider[] = [
    { id: "claude", label: "Claude (Anthropic)", name: "Claude", canTranscribe: false, keySet: cloudOn, keyPage: "https://platform.claude.com/settings/keys", sttDefault: null, llmDefault: "claude-opus-5-5" },
    { id: "openai", label: "OpenAI", name: "OpenAI", canTranscribe: true, keySet: cloudOn, keyPage: "https://platform.openai.com/api-keys", sttDefault: "gpt-transcribe", llmDefault: "gpt-6.2-sol" },
    { id: "gemini", label: "Gemini (Google)", name: "Gemini", canTranscribe: true, keySet: false, keyPage: "https://aistudio.google.com/apikey", sttDefault: "gemini-3.5-transcribe", llmDefault: "gemini-4-hydrogen" },
    { id: "openrouter", label: "OpenRouter", name: "OpenRouter", canTranscribe: true, keySet: false, keyPage: "https://openrouter.ai/settings/keys", sttDefault: "google/gemini-3.8-flash", llmDefault: "google/gemini-3.8-flash" },
  ];
  const aiInfos: AiInfo[] = [
    { id: "r1", tracks: 0, state: "none", stage: "", progress: 0, message: "", hasTranscript: false, hasNotes: false, missing: [], sendMinutes: 0, cloud: "" },
    { id: "r2", tracks: 3, state: "done", stage: "", progress: 1, message: "14 lines from 4 min of talk.", hasTranscript: true, hasNotes: true, missing: [], sendMinutes: 0, cloud: "" },
    { id: "r3", tracks: 2, state: "running", stage: "transcribe", progress: 0.42, message: "Transcribing 17 of 41 min…", hasTranscript: false, hasNotes: false, missing: [], sendMinutes: 0, cloud: "" },
    { id: "r4", tracks: 0, state: "needs-models", stage: "", progress: 0, message: "Download the speech and notes models first.", hasTranscript: false, hasNotes: false, missing: ["small", "qwen"], sendMinutes: 0, cloud: "" },
  ];
  // Cloud on: a run would send the sound (about as long as the recording, per speaker).
  const syncCloud = () => {
    const mins = (r: Recording) => Math.max(1, Math.ceil(((r.endedAt ?? r.startedAt) - r.startedAt) / min)) * Math.max(1, aiInfos.find((i) => i.id === r.id)?.tracks ?? 1);
    for (const i of aiInfos) {
      const r = recordings.find((x) => x.id === i.id);
      const on = aiSettings.processing === "cloud" && aiSettings.cloudConsent && r;
      i.sendMinutes = on ? mins(r) : 0;
      i.cloud = on ? providers.find((p) => p.id === aiSettings.sttProvider)?.name ?? "" : "";
    }
  };
  const aiModels: AiModel[] = [
    { id: "small", label: "Whisper small", note: "Speech to text; good for English.", bytes: 487_601_967, installed: false, partial: 0, downloading: false, error: null },
    { id: "medium", label: "Whisper medium", note: "Speech to text; clearly better for Bangla.", bytes: 1_533_763_059, installed: false, partial: 0, downloading: false, error: null },
    { id: "qwen", label: "Qwen2.5 3B Instruct", note: "Writes the notes and translates between English and Bangla.", bytes: 2_104_932_768, installed: false, partial: 0, downloading: false, error: null },
  ];
  const line = (speaker: string, start: number, lang: "en" | "bn", text: string, en: string | null, bn: string | null) => ({ speaker, track: speaker, start, end: start + 6, lang, text, en, bn });
  const transcript: TranscriptDoc = {
    version: 1, model: "small+medium", language: "mixed", duration: 3480,
    segments: [
      line("Ayesha Rahman", 4, "en", "Good morning everyone, let's start with the quarter's targets.", null, "সবাইকে শুভ সকাল, চলুন এই প্রান্তিকের লক্ষ্য দিয়ে শুরু করি।"),
      line("Tanvir Hasan", 12, "bn", "আমি মনে করি আমাদের মোবাইল অ্যাপের কাজ আগে শেষ করা উচিত।", "I think we should finish the mobile app work first.", null),
      line("Nadia Islam", 21, "en", "Agreed. The design for the checkout screen is ready for review.", null, "একমত। চেকআউট স্ক্রিনের ডিজাইন রিভিউয়ের জন্য প্রস্তুত।"),
      line("Ayesha Rahman", 33, "en", "Great. Tanvir, can you review it by Thursday?", null, "দারুণ। তানভীর, তুমি কি বৃহস্পতিবারের মধ্যে এটা দেখে দিতে পারবে?"),
      line("Tanvir Hasan", 41, "bn", "হ্যাঁ, বৃহস্পতিবারের মধ্যে দেখে দেব।", "Yes, I'll look at it by Thursday.", null),
      line("Nadia Islam", 305, "en", "On pricing, should we keep the free plan for another quarter?", null, "মূল্য নিয়ে, আমরা কি আরও এক প্রান্তিক ফ্রি প্ল্যান রাখব?"),
      line("Ayesha Rahman", 316, "en", "Let's keep it until March, then review the numbers.", null, "চলো মার্চ পর্যন্ত রাখি, তারপর সংখ্যাগুলো দেখব।"),
      line("Tanvir Hasan", 330, "bn", "ঠিক আছে, তাহলে আমি মার্চের আগে একটা রিপোর্ট তৈরি করব।", "Okay, then I'll prepare a report before March.", null),
    ],
  };
  const aiNotes: NotesDoc = {
    version: 1, language: "both", model: "qwen2.5-3b-instruct-q4_k_m", generatedAt: now, edited: false,
    en: {
      summary: "The team agreed to finish the mobile app first. The checkout design is ready for review, and the free plan stays until March.",
      decisions: ["Finish the mobile app work first", "Keep the free plan until March, then review the numbers"],
      actions: [{ owner: "Tanvir Hasan", task: "Review the checkout design", due: "Thursday" }, { owner: "Tanvir Hasan", task: "Prepare a usage report", due: "before March" }],
      questions: ["Should the free plan continue after March?"],
      chapters: [{ start: 4, end: 120, title: "Quarter targets", summary: "Priorities for the quarter and the checkout design review." }, { start: 305, end: 420, title: "Pricing", summary: "The free plan is kept until March." }],
    },
    bn: {
      summary: "দল প্রথমে মোবাইল অ্যাপের কাজ শেষ করতে একমত হয়েছে। চেকআউট ডিজাইন রিভিউয়ের জন্য প্রস্তুত এবং মার্চ পর্যন্ত ফ্রি প্ল্যান থাকবে।",
      decisions: ["প্রথমে মোবাইল অ্যাপের কাজ শেষ করা", "মার্চ পর্যন্ত ফ্রি প্ল্যান রাখা, তারপর সংখ্যা দেখা"],
      actions: [{ owner: "Tanvir Hasan", task: "চেকআউট ডিজাইন দেখা", due: "বৃহস্পতিবার" }, { owner: "Tanvir Hasan", task: "ব্যবহারের রিপোর্ট তৈরি", due: "মার্চের আগে" }],
      questions: ["মার্চের পরেও কি ফ্রি প্ল্যান চলবে?"],
      chapters: [{ start: 4, end: 120, title: "প্রান্তিকের লক্ষ্য", summary: "প্রান্তিকের অগ্রাধিকার এবং চেকআউট ডিজাইনের রিভিউ।" }, { start: 305, end: 420, title: "মূল্য নির্ধারণ", summary: "মার্চ পর্যন্ত ফ্রি প্ল্যান থাকছে।" }],
    },
  };
  const aiFind = (id: unknown) => aiInfos.find((i) => i.id === id) ?? (() => { throw new Error("That recording isn't there any more."); })();

  const account = (id: string, email: string, name: string): Account => ({ id, email, name, color: "#0a84ff", username: email, imap: { host: "imap.example.com", port: 993, security: "ssl" }, smtp: { host: "smtp.example.com", port: 465, security: "ssl" } } as unknown as Account);
  const accounts = mode === "nomail" ? [] : [account("acc1", "me@example.com", "Soab"), account("acc2", "work@example.org", "Soab (work)")];
  const contacts: Address[] = [
    ["Ayesha Rahman", "ayesha@example.com"], ["Tanvir Hasan", "tanvir@example.com"], ["Nadia Islam", "nadia@example.com"],
    ["Sadia Karim", "sadia@example.com"], ["Imran Chowdhury", "imran@example.com"], ["Rafiq Ahmed", "rafiq@example.com"],
  ].map(([name, addr]) => ({ name, addr }));

  let counter = 10;
  const make = (input: MeetingInput, id?: string): Meeting => {
    const old = meetings.find((m) => m.id === id);
    return {
      ...base, ...old, ...input, id: id ?? `b1b2c3d4e5f6${String(++counter).padStart(4, "0")}`, code: old?.code ?? "x7k2m9qbn4", createdAt: old?.createdAt ?? Date.now(),
      invitees: input.invitees.map((i) => old?.invitees.find((o) => o.email === i.email) ?? { ...i, sentAt: null, error: null }),
    };
  };
  const find = (id: unknown) => {
    const m = meetings.find((x) => x.id === id);
    if (!m) throw new Error("No such meeting");
    return m;
  };
  const changed = () => setTimeout(() => emit("meet-changed", null), 0);
  const wait = <T>(value: T, ms = 120) => new Promise<T>((resolve) => setTimeout(() => resolve(structuredClone(value)), ms));

  return {
    listen(event, fn) {
      const set = listeners.get(event) ?? new Set();
      set.add(fn as (p: never) => void);
      listeners.set(event, set);
      return () => set.delete(fn as (p: never) => void);
    },
    async invoke(cmd, a) {
      switch (cmd) {
        case "meet_state": return wait(state);
        case "meet_list": return wait(meetings);
        case "meet_create": {
          const m = make(a as unknown as MeetingInput);
          meetings = [m, ...meetings];
          changed();
          return wait(m);
        }
        case "meet_update": {
          const m = make(a as unknown as MeetingInput, a.id as string);
          meetings = meetings.map((x) => (x.id === m.id ? m : x));
          changed();
          return wait(m);
        }
        case "meet_delete": {
          if (find(a.id).live) throw new Error("It's live right now. End it first.");
          meetings = meetings.filter((m) => m.id !== a.id);
          changed();
          return wait(undefined);
        }
        case "meet_join_url": {
          if (state.tunnel.state !== "on") {
            state.tunnel = { state: "on", url: address(), error: null };
            changed();
          }
          return wait(`${state.tunnel.url}/m/${find(a.id).code}`, 400);
        }
        case "meet_open": {
          const m = find(a.id);
          m.live = m.hostIn = true;
          m.people = Math.max(m.people, 1);
          m.recording = m.autoRecord;
          changed();
          return wait(undefined);
        }
        case "meet_invite": {
          const m = find(a.id);
          const sent = Date.now();
          m.invitees = m.invitees.map((i) => (i.email.startsWith("imran") ? { ...i, error: "Mailbox full (552)" } : { ...i, sentAt: sent, error: null }));
          changed();
          return wait(m.invitees, 500);
        }
        case "meet_end": {
          const m = find(a.id);
          Object.assign(m, { live: false, hostIn: false, people: 0, waiting: 0, recording: false });
          changed();
          return wait(undefined);
        }
        case "meet_recordings": return wait(recordings);
        case "meet_recording_delete": {
          recordings.splice(recordings.findIndex((r) => r.id === a.id), 1);
          changed();
          return wait(undefined);
        }
        case "meet_tunnel":
          state.tunnel = a.on ? { state: "on", url: address(), error: null } : { state: "off", url: null, error: null };
          changed();
          return wait(state, 500);
        case "meet_settings":
          state.stun = a.stun as string[];
          state.turn = a.turn as MeetState["turn"];
          return wait(state);
        case "meet_domain": {
          const mode = a.mode === "domain" ? "domain" : "quick";
          const hostname = typeof a.hostname === "string" && a.hostname.trim() ? a.hostname.trim().toLowerCase() : state.hostname;
          if (mode === "domain" && !/^([a-z0-9-]+\.)+[a-z]{2,}$/.test(hostname)) throw new Error(`“${hostname}” isn't a hostname. Use just the name, like meet.example.com.`);
          const token = typeof a.token === "string" && a.token.trim() ? a.token.trim() : null;
          if (token && token.length < 20) throw new Error("That doesn't look like a tunnel token. Copy the long string after --token in Cloudflare's install command.");
          if (a.clearToken) state.tokenSet = false;
          if (token) state.tokenSet = true;
          if (mode === "domain" && !state.tokenSet) throw new Error("Paste the tunnel token first.");
          state.mode = mode;
          state.hostname = hostname;
          if (state.tunnel.state === "on") state.tunnel.url = address();
          changed();
          return wait(state, 300);
        }
        case "meet_domain_test": {
          // The hostname decides what the mock finds: "other.*" another server, "down.*" a tunnel that isn't connected.
          const result: DomainCheck = state.hostname.startsWith("other.") ? { result: "other", message: `${state.hostname} answers (status 200), but it isn't this PC's meeting server. Check the tunnel's public hostname points at this tunnel.` }
            : state.hostname.startsWith("down.") ? { result: "unreachable", message: `Cloudflare answers for ${state.hostname} (error 502), but nothing is behind it: the tunnel isn't connected, or its service URL isn't localhost:47230. Wait a few seconds and try again.` }
            : { result: "this-pc", message: `It works: guests who open ${state.hostname} reach this PC.` };
          return wait(result, 700);
        }
        case "meet_ai_settings": return wait(aiSettings);
        case "meet_ai_set_settings": {
          const next = a.settings as AiSettings;
          if (next.processing === "cloud" && !next.cloudConsent) throw new Error("Confirm what is sent to the cloud before turning it on.");
          Object.assign(aiSettings, next);
          aiSettings.banglaOn = aiSettings.processing === "cloud" && aiSettings.cloudConsent;
          syncCloud();
          return wait(aiSettings);
        }
        case "meet_ai_cloud_state": return wait(providers);
        case "meet_ai_cloud_key_set": {
          if (!String(a.key ?? "").trim()) throw new Error("That doesn't look like an API key: it has no spaces and is one line.");
          providers.find((p) => p.id === a.provider)!.keySet = true;
          return wait(providers);
        }
        case "meet_ai_cloud_key_clear": { providers.find((p) => p.id === a.provider)!.keySet = false; return wait(providers); }
        case "meet_ai_cloud_test": {
          const p = providers.find((x) => x.id === a.provider)!;
          if (!p.keySet) throw new Error(`Add your ${p.name} API key first.`);
          return wait(`The key works with ${aiSettings.cloudModels[`${p.id}.llm`] || p.llmDefault}.`, 700);
        }
        case "meet_ai_status": syncCloud(); return wait(aiInfos);
        case "meet_ai_models": return wait(aiModels);
        case "meet_ai_model_download": {
          const m = aiModels.find((x) => x.id === a.id)!;
          m.downloading = true;
          const timer = window.setInterval(() => {
            m.partial = Math.min(m.bytes, m.partial + m.bytes / 8);
            if (m.partial >= m.bytes) {
              window.clearInterval(timer);
              Object.assign(m, { installed: true, downloading: false });
            }
            emit("meet-ai", { models: true });
          }, 600);
          return wait(undefined);
        }
        case "meet_ai_model_cancel": { aiModels.find((x) => x.id === a.id)!.downloading = false; return wait(undefined); }
        case "meet_ai_model_remove": { Object.assign(aiModels.find((x) => x.id === a.id)!, { installed: false, partial: 0 }); return wait(undefined); }
        case "meet_ai_run": {
          const i = aiFind(a.id);
          Object.assign(i, { state: "running", stage: "transcribe", progress: 0.05, message: "Transcribing 2 of 41 min…" });
          emit("meet-ai", { id: i.id, state: "running", stage: i.stage, progress: i.progress, message: i.message });
          return wait(undefined);
        }
        case "meet_ai_cancel": { Object.assign(aiFind(a.id), { state: "cancelled", message: "Stopped." }); changed(); return wait(undefined); }
        case "meet_ai_off": { Object.assign(aiFind(a.id), { state: a.off ? "off" : "none" }); changed(); return wait(undefined); }
        case "meet_transcript": return wait(transcript);
        case "meet_notes": return wait(aiNotes);
        case "meet_notes_save": return wait(undefined);
        case "mail_overview": return wait({ accounts, drafts: 0, colors: [] } satisfies Overview);
        case "mail_contacts": {
          const p = String(a.prefix ?? "").toLowerCase();
          return wait(contacts.filter((c) => c.name.toLowerCase().includes(p) || c.addr.includes(p)), 60);
        }
        default: throw new Error(`The Meet mock has no ${cmd}`);
      }
    },
  };
}
