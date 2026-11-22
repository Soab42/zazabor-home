import { save as saveDialog } from "../../shared/dialog";
import type { ShellContext } from "../../shell/module";
import { formatBytes, h } from "../../ui/dom";
import { icon } from "../../ui/icons";
import { notesApi } from "../../notes/api";
import { invoke } from "@tauri-apps/api/core";
import { showToast } from "../files/ops/toast";
import { videoApi, onState } from "../video/api";
import {
  dayText, clockText, durationText, errorText, meetApi, onMeet, timeText,
  type AiInfo, type AiModel, type Notes, type NotesDoc, type Recording, type TranscriptDoc, type TranscriptSegment,
} from "./api";

/**
 * The meeting page behind Meet → Recordings: the recording with its subtitle language, the
 * Transcript tab (who said what, when, in which language; search; click to jump there), the Notes
 * tab (summary, decisions, actions, questions and chapters, all editable; copy, Markdown file, a
 * note in Notes), and the state of the background job with its progress, cancel and re-run.
 * docs/plans/meet-notes.md.
 */

/**
 * Bangla transcripts, subtitles and notes on this PC. Off in 0.3.4 (stock models gave unusable
 * Bangla); matches `BANGLA_READY` in src-tauri/src/meet/ai.rs. Bangla chips stay hidden while it's
 * off, unless Cloud speech to text is on (`AiSettings.banglaOn`, which Zazabor works out).
 */
export const BANGLA_READY = false;

/** Whether Bangla is shown, given the settings (this PC's gate, or Cloud speech to text). */
export const banglaAvailable = (settings: { banglaOn: boolean } | null | undefined) => BANGLA_READY || !!settings?.banglaOn;

type Subs = "off" | "orig" | "en" | "bn";
type Shown = "orig" | "en" | "bn";

const MODEL_NAME: Record<string, string> = { small: "Whisper small", medium: "Whisper medium", qwen: "the notes model" };
const LANG_LABEL: Record<string, string> = { en: "EN", bn: "বাং" };
const CLOUD_NAME: Record<string, string> = { claude: "Claude", openai: "OpenAI", gemini: "Gemini", openrouter: "OpenRouter" };

/** `Name 2026-11-21 14.30.webm` → `Name 2026-11-21 14.30` plus a suffix. */
const sidecar = (path: string, suffix: string) => path.replace(/\.[^./\\]+$/, "") + suffix;

const speakerColor = (name: string) => {
  let n = 0;
  for (const c of name) n = (n * 31 + c.charCodeAt(0)) % 360;
  return `hsl(${n} 62% 62%)`;
};

/** The notes as Markdown (the same layout the backend's `.notes.md` has). */
export function notesMarkdown(title: string, notes: Notes, lang: "en" | "bn"): string {
  const head = lang === "bn" ? ["সারসংক্ষেপ", "সিদ্ধান্ত", "করণীয়", "অমীমাংসিত প্রশ্ন", "মূল মুহূর্ত"] : ["Summary", "Decisions", "Action items", "Open questions", "Key moments"];
  const list = (name: string, items: string[]) => (items.length ? `## ${name}\n\n${items.map((i) => `- ${i}`).join("\n")}\n\n` : "");
  return `# ${title}: meeting notes\n\n## ${head[0]}\n\n${notes.summary}\n\n`
    + list(head[1], notes.decisions)
    + list(head[2], notes.actions.map((a) => `**${a.owner}:** ${a.task}${a.due ? ` (${a.due})` : ""}`))
    + list(head[3], notes.questions)
    + list(head[4], notes.chapters.map((c) => `**${timeText(c.start)}** ${c.title} — ${c.summary}`));
}

export interface MeetingPage {
  el: HTMLElement;
  /** A job moved on (from the `meet-ai` event). */
  update(info: AiInfo): void;
  dispose(): void;
}

export function createMeetingPage(shell: ShellContext, rec: Recording, info: AiInfo, back: () => void): MeetingPage {
  let current = info;
  let doc: TranscriptDoc | null = null;
  let notes: NotesDoc | null = null;
  let models: AiModel[] = [];
  let tab: "transcript" | "notes" = "transcript";
  let subs: Subs = "en";
  let shown: Shown = "orig";
  let notesLang: "en" | "bn" = "en";
  let query = "";
  let loadedFor = "";
  let saveTimer = 0;
  // Bangla shows on this PC when its models can, or while Cloud speech to text is on.
  let banglaOn = BANGLA_READY;

  const toast = showToast;
  const fail = (what: string) => (err: unknown) => toast(`${what}: ${errorText(err)}`);

  // ---- playing

  function subtitleFile(which: Subs): string | null {
    if (which === "off") return null;
    return sidecar(rec.path, which === "orig" ? ".vtt" : `.${which}.vtt`);
  }

  /** Opens the recording in the player, with the chosen subtitles, at `at` seconds. */
  async function play(at = 0) {
    const store = (shell.config.video.ext["subtitles"] ??= {}) as { assign?: Record<string, string> };
    const assign = (store.assign ??= {});
    const file = current.hasTranscript ? subtitleFile(subs) : null;
    if (file) assign[rec.path] = file;
    else delete assign[rec.path];
    shell.saveConfig();
    shell.open({ kind: "video", path: rec.path });
    if (at > 0) {
      const stop = await onState((s) => {
        if (s.path !== rec.path || !s.playing) return;
        stop();
        void videoApi.seek(at * 1000).catch(() => undefined);
      });
      window.setTimeout(stop, 15_000);
    }
  }

  // ---- the job

  const jobBox = h("div", { class: "mtg-job" });

  const modelsFor = (ids: string[]) => ids.map((id) => models.find((m) => m.id === id)).filter((m): m is AiModel => !!m);

  async function refreshModels() {
    try {
      models = await meetApi.aiModels();
    } catch {
      models = [];
    }
  }

  function drawJob() {
    const i = current;
    jobBox.replaceChildren();
    jobBox.hidden = false;
    const row = (...children: (Node | string | null)[]) => h("div", { class: "mtg-job-row" }, ...children);
    const act = (label: string, glyph: Parameters<typeof icon>[0], kind: string, run: () => void) => h("button", { class: `btn small${kind ? ` ${kind}` : ""}`, type: "button", onclick: run }, icon(glyph, 14), label);
    const run = (options?: Parameters<typeof meetApi.aiRun>[1]) => void meetApi.aiRun(rec.id, options).catch(fail("Couldn't start"));
    // With Cloud on, what a run sends, and the way out of it for this meeting.
    const cloudNote = (): Node[] => (i.cloud ? [h("p", { class: "sf-hint mtg-cloud" }, icon("upload", 13), `Cloud AI is on: about ${i.sendMinutes} minute${i.sendMinutes === 1 ? "" : "s"} of audio will be sent to ${i.cloud}, then the transcript text to your notes provider (Settings → Meet).`)] : []);
    const thisPc = (redo?: "notes") => (i.cloud ? act("Use this PC instead", "desktop", "", () => run({ processing: "local", ...(redo ? { redo } : {}) })) : null);
    if (i.state === "running" || i.state === "queued") {
      const bar = h("div", { class: "mtg-bar" }, h("div", { class: "mtg-bar-fill", style: { width: `${Math.round(i.progress * 100)}%` } }));
      jobBox.append(row(h("strong", null, i.state === "queued" ? "Waiting for its turn" : "Working on it"), h("span", { class: "mtg-job-msg" }, i.message), act("Stop", "stop", "", () => void meetApi.aiCancel(rec.id).catch(fail("Couldn't stop"))), ), bar,
        h("p", { class: "sf-hint" }, i.cloud ? "The sound and the transcript text go to your cloud providers (Settings → Meet). You can leave this page." : "This runs on this PC, at low priority, and waits while the PC is busy. You can leave this page."));
    } else if (i.state === "needs-models") {
      const needed = modelsFor(i.missing);
      const total = needed.filter((m) => !m.installed).reduce((n, m) => n + m.bytes, 0);
      jobBox.append(row(h("strong", null, "Download the models first"), h("span", { class: "mtg-job-msg" }, `${needed.map((m) => `${MODEL_NAME[m.id] ?? m.label} (${formatBytes(m.bytes)})`).join(", ")}. One time only, from Hugging Face, checked against a fixed checksum; nothing from your meetings is sent anywhere.`)));
      const buttons = h("div", { class: "mtg-job-buttons" });
      for (const m of needed) {
        if (m.installed) continue;
        buttons.append(m.downloading
          ? h("button", { class: "btn small", type: "button", onclick: () => void meetApi.aiModelCancel(m.id).then(refreshAndDraw) }, icon("stop", 14), `Pause ${MODEL_NAME[m.id] ?? m.label}… ${Math.round((m.partial / m.bytes) * 100)}%`)
          : act(`Download ${MODEL_NAME[m.id] ?? m.label} (${formatBytes(m.bytes)})`, "download", "primary", () => void meetApi.aiModelDownload(m.id).then(refreshAndDraw, fail("Couldn't download"))));
        if (m.error) jobBox.append(h("p", { class: "sf-hint bad" }, m.error));
      }
      if (needed.length && needed.every((m) => m.installed)) buttons.append(act(`Transcribe (${formatBytes(total)} ready)`, "play", "primary", () => run()));
      jobBox.append(buttons);
    } else if (i.state === "error") {
      jobBox.append(row(h("strong", { class: "bad" }, "It didn't work"), h("span", { class: "mtg-job-msg" }, i.message), act("Try again", "refresh", "", () => run()), thisPc(i.hasTranscript ? "notes" : undefined)));
    } else if (i.state === "cancelled") {
      jobBox.append(row(h("strong", null, "Stopped"), h("span", { class: "mtg-job-msg" }, i.hasTranscript ? "The transcript is there; the notes weren't finished." : ""), act(i.hasTranscript ? "Write the notes" : "Transcribe", "play", "primary", () => run(i.hasTranscript ? { redo: "notes" } : undefined)), thisPc(i.hasTranscript ? "notes" : undefined)));
    } else if (i.state === "off") {
      jobBox.append(row(h("strong", null, "Transcripts are off for this meeting"), act("Transcribe now", "play", "primary", () => void meetApi.aiOff(rec.id, false).then(() => run())), thisPc()), ...cloudNote());
    } else if (i.state === "none" && !i.hasTranscript) {
      jobBox.append(row(h("strong", null, "No transcript yet"), h("span", { class: "mtg-job-msg" }, i.tracks ? `${i.tracks} speaker track${i.tracks === 1 ? "" : "s"} recorded: each person is labelled by name.` : "This recording has only the mixed sound, so everyone is labelled “Meeting”."), act("Transcribe", "play", "primary", () => run()), thisPc()), ...cloudNote());
    } else {
      jobBox.hidden = !i.message && i.state === "done" ? true : false;
      if (!jobBox.hidden) jobBox.append(row(h("span", { class: "mtg-job-msg" }, i.message)));
    }
  }

  const refreshAndDraw = async () => {
    await refreshModels();
    drawJob();
  };

  // ---- parts of the page

  const title = h("h2", null, rec.title);
  const meta = h("p", { class: "mtg-page-meta" }, `${dayText(rec.startedAt)} ${clockText(rec.startedAt)}${rec.endedAt ? ` · ${durationText((rec.endedAt - rec.startedAt) / 1000)}` : ""} · ${formatBytes(rec.bytes)}`);

  const subChips = h("div", { class: "mtg-chips", role: "radiogroup", "aria-label": "Subtitles" });
  function drawSubChips() {
    const has = current.hasTranscript;
    const bn = !!doc?.segments.some((s) => s.bn || s.lang === "bn");
    const en = !!doc?.segments.some((s) => s.en || s.lang === "en");
    const options: [Subs, string, boolean][] = [["off", "Off", true], ["orig", "Original", has], ["en", "English", has && en], ["bn", "Bangla", banglaOn && has && bn]];
    subChips.replaceChildren(h("span", { class: "mtg-chips-label" }, icon("subtitles", 14), "Subtitles"),
      ...options.filter(([id]) => banglaOn || id !== "bn").map(([id, label, enabled]) => h("button", { class: `mtg-chip${subs === id ? " active" : ""}`, type: "button", role: "radio", "aria-checked": String(subs === id), disabled: !enabled, onclick: () => { subs = id; drawSubChips(); } }, label)));
  }

  const playButton = h("button", { class: "btn primary", type: "button", onclick: () => void play() }, icon("play", 15), "Play");
  const moreButton = h("button", { class: "btn small", type: "button" }, icon("more", 14), "More");
  moreButton.addEventListener("click", () => {
    const box = moreButton.getBoundingClientRect();
    void import("../../ui/menu").then(({ openMenu }) => openMenu(box.left, box.bottom + 4, [
      { label: "Transcribe again", icon: "refresh", action: () => void meetApi.aiRun(rec.id).catch(fail("Couldn't start")) },
      { label: "Transcribe again with the medium model", icon: "refresh", action: () => void meetApi.aiRun(rec.id, { model: "medium", processing: "local" }).catch(fail("Couldn't start")) },
      ...(current.cloud ? [{ label: "Transcribe again on this PC (nothing is sent)", icon: "desktop" as const, action: () => void meetApi.aiRun(rec.id, { processing: "local" }).catch(fail("Couldn't start")) }] : []),
      { label: "Write the notes again", icon: "pencil", disabled: !current.hasTranscript, action: () => void meetApi.aiRun(rec.id, { redo: "notes" }).catch(fail("Couldn't start")) },
      { label: "Turn transcripts off for this meeting", icon: "stop", action: () => void meetApi.aiOff(rec.id, true).catch(fail("Couldn't change it")) },
    ]));
  });

  const tabs = h("div", { class: "mtg-tabs", role: "tablist" });
  const pane = h("div", { class: "mtg-pane" });
  function drawTabs() {
    tabs.replaceChildren(...(["transcript", "notes"] as const).map((t) => h("button", { class: `mtg-tab${tab === t ? " active" : ""}`, type: "button", role: "tab", "aria-selected": String(tab === t), onclick: () => { tab = t; drawTabs(); drawPane(); } }, t === "transcript" ? "Transcript" : "Notes")));
  }

  // ---- transcript tab

  const line = (s: TranscriptSegment): string => (shown === "en" ? s.en ?? (s.lang === "en" ? s.text : "") : shown === "bn" ? s.bn ?? (s.lang === "bn" ? s.text : "") : s.text);

  function transcriptPane(): HTMLElement {
    if (!doc) return h("p", { class: "mtg-loading" }, current.state === "running" || current.state === "queued" ? "The transcript appears when it's done." : "No transcript yet.");
    const segments = doc.segments;
    const search = h("input", { class: "ops-input mtg-search", type: "search", placeholder: "Search the transcript", "aria-label": "Search the transcript", value: query });
    const switcher = h("div", { class: "mtg-chips" }, ...(([["orig", "As spoken"], ["en", "English"], ["bn", "Bangla"]] as [Shown, string][]).filter(([id]) => banglaOn || id !== "bn")).map(([id, label]) => h("button", { class: `mtg-chip${shown === id ? " active" : ""}`, type: "button", onclick: () => { shown = id; drawPane(); } }, label)));
    const list = h("div", { class: "mtg-lines" });
    const count = h("span", { class: "mtg-count" });
    const paint = () => {
      const q = query.trim().toLowerCase();
      const rows = segments.filter((s) => !q || [s.text, s.en ?? "", s.bn ?? "", s.speaker].some((t) => t.toLowerCase().includes(q)));
      count.textContent = q ? `${rows.length} of ${segments.length}` : `${segments.length} lines`;
      list.replaceChildren(...rows.map((s) => {
        const text = line(s) || (shown === "orig" ? "" : s.text);
        const missing = shown !== "orig" && !line(s);
        const body = h("span", { class: `mtg-text${missing ? " dim" : ""}`, lang: shown === "bn" || (shown === "orig" && s.lang === "bn") ? "bn" : "en" });
        if (q && text.toLowerCase().includes(q)) {
          const at = text.toLowerCase().indexOf(q);
          body.append(text.slice(0, at), h("mark", null, text.slice(at, at + q.length)), text.slice(at + q.length));
        } else body.textContent = text;
        return h("button", { class: "mtg-line", type: "button", title: "Play from here", onclick: () => void play(s.start) },
          h("span", { class: "mtg-time" }, timeText(s.start)),
          h("span", { class: "mtg-who", style: { color: speakerColor(s.speaker) } }, s.speaker),
          h("span", { class: `mtg-lang ${s.lang}` }, LANG_LABEL[s.lang] ?? s.lang.toUpperCase()),
          body);
      }));
      if (!rows.length) list.append(h("p", { class: "mtg-loading" }, segments.length ? "Nothing matches." : "No speech was found in this meeting."));
    };
    search.addEventListener("input", () => { query = search.value; paint(); });
    paint();
    return h("div", { class: "mtg-transcript" }, h("div", { class: "mtg-toolbar" }, search, switcher, count), list);
  }

  // ---- notes tab

  function scheduleSave() {
    window.clearTimeout(saveTimer);
    saveTimer = window.setTimeout(() => {
      if (!notes) return;
      void meetApi.notesSave(rec.id, { en: notes.en, bn: notes.bn }).catch(fail("Couldn't save the notes"));
    }, 600);
  }

  function notesPane(): HTMLElement {
    if (!notes) {
      const waiting = current.state === "running" || current.state === "queued";
      return h("p", { class: "mtg-loading" }, waiting ? "The notes appear when they're written." : current.hasTranscript ? "There are no notes yet." : "Transcribe the meeting first.",
        ...(!waiting && current.hasTranscript ? [h("button", { class: "btn small primary mtg-inline", type: "button", onclick: () => void meetApi.aiRun(rec.id, { redo: "notes" }).catch(fail("Couldn't start")) }, "Write the notes")] : []));
    }
    const available = (["en", "bn"] as const).filter((l) => notes?.[l]);
    if (!available.includes(notesLang)) notesLang = available[0] ?? "en";
    const n = notes[notesLang];
    if (!n) return h("p", { class: "mtg-loading" }, "These notes aren't in this language.");
    const lang = notesLang;
    const text = (value: string, onInput: (v: string) => void, rows = 1) => {
      const el = h("textarea", { class: "ops-input mtg-edit", rows, lang, spellcheck: false });
      el.value = value;
      const fit = () => { el.style.height = "auto"; el.style.height = `${el.scrollHeight}px`; };
      el.addEventListener("input", () => { onInput(el.value); fit(); scheduleSave(); });
      queueMicrotask(fit);
      return el;
    };
    const section = (name: string, ...body: (Node | null)[]) => h("section", { class: "mtg-section" }, h("h3", null, name), ...body);
    const head = lang === "bn" ? ["সারসংক্ষেপ", "সিদ্ধান্ত", "করণীয়", "অমীমাংসিত প্রশ্ন", "মূল মুহূর্ত"] : ["Summary", "Decisions", "Action items", "Open questions", "Key moments"];
    const stringList = (items: string[], placeholder: string) => {
      const box = h("div", { class: "mtg-items" });
      const paint = () => {
        box.replaceChildren(...items.map((value, i) => h("div", { class: "mtg-item" },
          text(value, (v) => { items[i] = v; }),
          h("button", { class: "icon-btn", type: "button", title: "Remove", "aria-label": "Remove", onclick: () => { items.splice(i, 1); paint(); scheduleSave(); } }, icon("close", 14)))),
          h("button", { class: "btn small ghost", type: "button", onclick: () => { items.push(""); paint(); box.querySelectorAll("textarea")[items.length - 1]?.focus(); } }, icon("plus", 13), placeholder));
      };
      paint();
      return box;
    };
    const actions = h("div", { class: "mtg-items" });
    const paintActions = () => {
      actions.replaceChildren(...n.actions.map((a, i) => h("div", { class: "mtg-item mtg-action" },
        h("input", { class: "ops-input mtg-owner", value: a.owner, lang, "aria-label": "Owner", oninput: (e: Event) => { a.owner = (e.target as HTMLInputElement).value; scheduleSave(); } }),
        text(a.task, (v) => { a.task = v; }),
        h("input", { class: "ops-input mtg-due", value: a.due ?? "", placeholder: "When", lang, "aria-label": "Due", oninput: (e: Event) => { a.due = (e.target as HTMLInputElement).value.trim() || null; scheduleSave(); } }),
        h("button", { class: "icon-btn", type: "button", title: "Remove", "aria-label": "Remove", onclick: () => { n.actions.splice(i, 1); paintActions(); scheduleSave(); } }, icon("close", 14)))),
        h("button", { class: "btn small ghost", type: "button", onclick: () => { n.actions.push({ owner: "", task: "", due: null }); paintActions(); } }, icon("plus", 13), "Add an action"));
    };
    paintActions();
    const chapters = h("div", { class: "mtg-chapters" }, ...n.chapters.map((c) => h("div", { class: "mtg-chapter" },
      h("button", { class: "mtg-time", type: "button", title: "Play from here", onclick: () => void play(c.start) }, timeText(c.start)),
      h("div", { class: "mtg-chapter-text" },
        h("input", { class: "ops-input mtg-chapter-title", value: c.title, lang, "aria-label": "Chapter title", oninput: (e: Event) => { c.title = (e.target as HTMLInputElement).value; scheduleSave(); } }),
        text(c.summary, (v) => { c.summary = v; }, 2)))));
    const markdown = () => notesMarkdown(rec.title, n, lang);
    const toolbar = h("div", { class: "mtg-toolbar" },
      available.length > 1 ? h("div", { class: "mtg-chips" }, ...available.map((l) => h("button", { class: `mtg-chip${notesLang === l ? " active" : ""}`, type: "button", onclick: () => { notesLang = l; drawPane(); } }, l === "en" ? "English" : "বাংলা"))) : null,
      h("span", { class: "spacer" }),
      h("button", { class: "btn small", type: "button", onclick: () => void navigator.clipboard.writeText(markdown()).then(() => toast("Notes copied"), () => toast("The system wouldn't copy them.")) }, icon("copy", 14), "Copy"),
      h("button", { class: "btn small", type: "button", onclick: () => void exportFile(markdown()) }, icon("save", 14), "Save as Markdown…"),
      h("button", { class: "btn small", type: "button", onclick: () => void exportNote(markdown()) }, icon("note", 14), "Add to Notes"));
    return h("div", { class: "mtg-notes" }, toolbar,
      section(head[0], text(n.summary, (v) => { n.summary = v; }, 3)),
      section(head[1], stringList(n.decisions, "Add a decision")),
      section(head[2], actions),
      section(head[3], stringList(n.questions, "Add a question")),
      n.chapters.length ? section(head[4], chapters) : null,
      h("p", { class: "sf-hint" }, `Written by ${notes.model.replace(/-q\d.*$/i, "")} ${notes.provider ? `through ${CLOUD_NAME[notes.provider] ?? notes.provider}` : "on this PC"}${notes.edited ? "; edited by you" : ""}. Changes save by themselves.`));
  }

  async function exportFile(markdown: string) {
    const path = await saveDialog({ title: "Save the notes", defaultPath: `${rec.title} notes.md`, filters: [{ name: "Markdown", extensions: ["md"] }] });
    if (!path) return;
    try {
      await invoke("fs_write_text", { path, text: markdown });
      toast("Notes saved");
    } catch (err) {
      fail("Couldn't save the notes")(err);
    }
  }

  async function exportNote(markdown: string) {
    try {
      const note = await notesApi.create({ body: markdown });
      await notesApi.update(note.id, { title: `${rec.title}: meeting notes` });
      toast("Added to Notes");
    } catch (err) {
      fail("Couldn't add it to Notes")(err);
    }
  }

  function drawPane() {
    pane.replaceChildren(tab === "transcript" ? transcriptPane() : notesPane());
  }

  // ---- loading

  async function load() {
    const key = `${current.hasTranscript}:${current.hasNotes}:${current.state === "running"}`;
    if (key === loadedFor) return;
    loadedFor = key;
    if (current.hasTranscript) doc = await meetApi.transcript(rec.id).catch(() => null);
    else doc = null;
    notes = current.hasNotes ? await meetApi.notes(rec.id).catch(() => null) : null;
    drawSubChips();
    drawPane();
  }

  const unlisten = onMeet("meet-ai", (e) => {
    if (e.models) void refreshAndDraw();
  });

  const backButton = h("button", { class: "btn small ghost mtg-back", type: "button", onclick: back }, icon("back", 14), "Recordings");
  const el = h("div", { class: "mtg-page" },
    h("div", { class: "mtg-page-head" }, backButton),
    h("div", { class: "mtg-page-title" }, h("div", null, title, meta), h("div", { class: "mtg-card-actions" }, playButton, moreButton)),
    subChips, jobBox, tabs, pane);

  drawSubChips();
  drawTabs();
  drawJob();
  drawPane();
  void refreshModels().then(drawJob);
  void meetApi.aiSettings().then((settings) => {
    banglaOn = banglaAvailable(settings);
    drawSubChips();
    drawPane();
  }, () => undefined);
  void load();

  return {
    el,
    update(next) {
      const before = current.state;
      current = next;
      drawJob();
      if (before !== next.state || next.state === "done") void load();
      else if (next.state === "running" || next.state === "queued") drawJob();
      drawSubChips();
    },
    dispose() {
      window.clearTimeout(saveTimer);
      void unlisten.then((stop) => stop());
    },
  };
}
