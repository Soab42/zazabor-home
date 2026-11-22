import "../files/ops/ops.css";
import "./meet.css";
import { invoke } from "@tauri-apps/api/core";
import type { SettingsRow } from "../../appearance";
import type { Config } from "../../config";
import type { Category } from "../../shell/settings";
import { formatBytes, h } from "../../ui/dom";
import { confirmSheet } from "../files/ops/sheets";
import { icon } from "../../ui/icons";
import { errorText, meetApi, onMeet, type AiModel, type AiSettings, type CloudId, type CloudProvider, type MeetState } from "./api";
import { BANGLA_READY, banglaAvailable } from "./meeting";

/**
 * Settings → Meet: the outside link (a Cloudflare quick tunnel, or the user's own domain through a
 * named tunnel) on or off with its address, where recordings go, the STUN servers and an optional TURN server for guests behind strict networks.
 */
export function meetCategory(_host: { readonly config: Config }): Category {
  let state: MeetState | null = null;

  const problem = h("p", { class: "sf-hint pl-settings-error", hidden: true });
  const fail = (err: unknown) => {
    problem.hidden = false;
    problem.textContent = errorText(err);
  };

  // ---- the outside link
  const tunnelInput = h("input", { type: "checkbox", class: "switch", role: "switch" });
  const tunnelHint = h("p", { class: "sf-hint" });
  const tunnelRow: SettingsRow = { label: "Outside link", el: h("label", { class: "sf-row sf-toggle" }, h("div", { class: "st-text" }, h("span", { class: "sf-label" }, "Let people outside this Wi-Fi join"), tunnelHint), tunnelInput) };
  tunnelInput.addEventListener("change", () => {
    const on = tunnelInput.checked;
    tunnelInput.disabled = true;
    void meetApi.tunnel(on).then(draw, (err) => {
      tunnelInput.checked = !on;
      fail(err);
    }).finally(() => (tunnelInput.disabled = false));
  });
  const addressHint = h("p", { class: "sf-hint" });
  const addressText = h("span", { class: "st-value mtg-settings-address" }, "—");
  const copy = h("button", { class: "btn small ghost", type: "button" }, "Copy");
  copy.addEventListener("click", () => void navigator.clipboard?.writeText(addressText.textContent ?? "").catch(() => undefined));
  const addressRow: SettingsRow = { label: "Address", el: h("div", { class: "sf-row plain" }, h("div", { class: "st-text" }, h("span", { class: "sf-label" }, "Address"), addressHint), h("div", { class: "pl-settings-buttons" }, addressText, copy)) };

  // ---- quick link or own domain
  const radio = (value: "quick" | "domain", label: string) => {
    const input = h("input", { type: "radio", name: "meet-mode", value });
    input.addEventListener("change", () => choose(value));
    return { input, el: h("label", { class: "mtg-radio" }, input, h("span", null, label)) };
  };
  const quickRadio = radio("quick", "Quick link (changes when the tunnel restarts)");
  const domainRadio = radio("domain", "My own domain (permanent links)");
  const modeRow: SettingsRow = { label: "Kind of link", el: h("div", { class: "sf-row wide" }, h("span", { class: "sf-label" }, "Kind of link"), quickRadio.el, domainRadio.el) };

  // What the radios show before it's saved; null follows the saved mode.
  let draft: "quick" | "domain" | null = null;
  const shownMode = () => draft ?? state?.mode ?? "quick";
  function choose(mode: "quick" | "domain") {
    draft = mode;
    if (mode === "quick") {
      void meetApi.domain({ mode }).then((next) => {
        draft = null;
        problem.hidden = true;
        draw(next);
      }, (err) => {
        draft = null;
        fail(err);
        draw(null);
      });
    } else {
      draw(null);
    }
  }

  const hostInput = h("input", { class: "ops-input mtg-settings-input", type: "text", placeholder: "meet.example.com", spellcheck: false, autocomplete: "off", autocapitalize: "off", "aria-label": "Hostname" });
  const tokenInput = h("input", { class: "ops-input mtg-settings-input", type: "password", spellcheck: false, autocomplete: "off", "aria-label": "Tunnel token" });
  const tokenState = h("span", { class: "mtg-token-saved" });
  const tokenRemove = h("button", { class: "btn small ghost", type: "button" }, "Remove");
  const tokenLine = h("div", { class: "pl-settings-buttons mtg-token-line" }, tokenState, tokenRemove);
  const saveDomain = h("button", { class: "btn small primary", type: "button" }, "Save");
  const test = h("button", { class: "btn small", type: "button" }, "Test");
  const testResult = h("p", { class: "sf-hint mtg-test", hidden: true });
  const busy = (button: HTMLButtonElement, work: Promise<unknown>) => {
    button.disabled = true;
    void work.finally(() => (button.disabled = false));
  };
  saveDomain.addEventListener("click", () => {
    const token = tokenInput.value.trim();
    busy(saveDomain, meetApi.domain({ mode: "domain", hostname: hostInput.value, ...(token ? { token } : {}) }).then((next) => {
      draft = null;
      tokenInput.value = "";
      problem.hidden = true;
      draw(next);
    }, fail));
  });
  tokenRemove.addEventListener("click", () => {
    busy(tokenRemove, meetApi.domain({ mode: state?.mode ?? "quick", clearToken: true }).then((next) => {
      problem.hidden = true;
      draw(next);
    }, fail));
  });
  test.addEventListener("click", () => {
    testResult.hidden = false;
    testResult.dataset.result = "busy";
    testResult.textContent = "Testing… (the tunnel starts first if it's off)";
    busy(test, meetApi.domainTest().then((r) => {
      testResult.dataset.result = r.result;
      testResult.textContent = (r.result === "this-pc" ? "✓ " : "✗ ") + r.message;
    }, (err) => {
      testResult.dataset.result = "unreachable";
      testResult.textContent = `✗ ${errorText(err)}`;
    }));
  });
  const step = (...parts: (Node | string)[]) => h("li", null, ...parts);
  const code = (text: string) => h("code", null, text);
  const guide = h("details", { class: "mtg-guide" },
    h("summary", null, "How to set it up"),
    h("ol", null,
      step("You need a free Cloudflare account and a domain whose DNS is on Cloudflare."),
      step("In the Cloudflare dashboard go to Zero Trust → Networks → Tunnels → Create a tunnel → Cloudflared. Name it ", code("zazabor-meet"), ". Copy the long string after ", code("--token"), " in the install command and paste it above."),
      step("In the tunnel's Public Hostname tab add one: subdomain ", code("meet"), ", your domain, service type ", code("HTTP"), ", URL ", code("localhost:47230"), "."),
      step("Save it there, type the full hostname (for example ", code("meet.example.com"), ") above, press Save, then Test."),
    ),
  );
  const domainRow: SettingsRow = {
    label: "My own domain",
    el: h("div", { class: "sf-row wide" },
      h("span", { class: "sf-label" }, "Hostname"), hostInput,
      h("span", { class: "sf-label mtg-gap" }, "Tunnel token"), tokenInput, tokenLine,
      h("div", { class: "pl-settings-buttons mtg-gap" }, saveDomain, test), testResult,
      guide),
  };

  // ---- recordings
  const folder = h("p", { class: "sf-hint pl-settings-path" });
  const show = h("button", { class: "btn small", type: "button" }, "Open Folder");
  show.addEventListener("click", () => state && void invoke("fs_open_default", { path: state.recordingsDir }).catch(fail));
  const folderRow: SettingsRow = { label: "Recordings folder", el: h("div", { class: "sf-row plain" }, h("div", { class: "st-text" }, h("span", { class: "sf-label" }, "Recordings go to"), folder), show) };

  // ---- transcripts and notes (docs/plans/meet-notes.md)
  let ai: AiSettings = { auto: true, model: "auto", notesLang: "en", bnSubs: BANGLA_READY, processing: "local", cloudConsent: false, fallbackLocal: false, sttProvider: "openai", llmProvider: "claude", cloudModels: {}, banglaOn: BANGLA_READY };
  let providers: CloudProvider[] = [];
  const saveAi = (patch: Partial<AiSettings>) => {
    const next = { ...ai, ...patch };
    void meetApi.setAiSettings(next).then((saved) => {
      ai = saved;
      problem.hidden = true;
      drawAi();
    }, (err) => {
      fail(err);
      drawAi();
    });
  };
  const autoInput = h("input", { type: "checkbox", class: "switch", role: "switch" });
  autoInput.addEventListener("change", () => saveAi({ auto: autoInput.checked }));
  const autoHint = h("p", { class: "sf-hint" });
  const autoRow: SettingsRow = { label: "Transcribe automatically", el: h("label", { class: "sf-row sf-toggle" }, h("div", { class: "st-text" }, h("span", { class: "sf-label" }, "Transcribe and take notes automatically"), autoHint), autoInput) };
  function drawAutoHint() {
    const turnOff = "You can turn it off for one meeting, or run it later from the meeting.";
    autoHint.textContent = cloudNow()
      ? `When a recorded meeting ends, the sound and then the transcript text are sent to the providers you chose below, and they write who said what (Bangla or English), subtitles in both languages and notes. ${turnOff}`
      : BANGLA_READY ? `When a recorded meeting ends, this PC writes who said what (Bangla or English), subtitles in both languages and notes. Nothing leaves the computer. ${turnOff}`
      : `When a recorded meeting ends, this PC writes who said what, English subtitles and notes. Nothing leaves the computer. Bangla needs Cloud AI for now. ${turnOff}`;
  }
  const choice = <T extends string>(label: string, options: [T, string][], pick: (v: T) => void) => {
    const select = h("select", { class: "ops-input mtg-settings-select", "aria-label": label }, ...options.map(([value, name]) => h("option", { value }, name)));
    select.addEventListener("change", () => pick(select.value as T));
    return select;
  };
  const modelSelect = choice<AiSettings["model"]>("Speech model", BANGLA_READY ? [["auto", "Automatic: small, medium for Bangla"], ["small", "Small (faster, good for English)"], ["medium", "Medium (best for Bangla, slower)"]] : [["auto", "Automatic (small)"], ["small", "Small (faster)"], ["medium", "Medium (more accurate, slower)"]], (model) => saveAi({ model }));
  const modelChoiceRow: SettingsRow = { label: "Speech model", el: h("div", { class: "sf-row plain" }, h("div", { class: "st-text" }, h("span", { class: "sf-label" }, "Speech model"), h("p", { class: "sf-hint" }, "About one minute of talk takes a few seconds (small) to a minute (medium) per speaker on this CPU.")), modelSelect) };
  const langSelect = choice<AiSettings["notesLang"]>("Notes language", [["en", "English"], ["bn", "Bangla"], ["both", "English and Bangla"]], (notesLang) => saveAi({ notesLang }));
  const langRow: SettingsRow = { label: "Notes language", el: h("div", { class: "sf-row plain" }, h("div", { class: "st-text" }, h("span", { class: "sf-label" }, "Notes language"), h("p", { class: "sf-hint" }, "Subtitles are always saved in the spoken language and in English; Bangla subtitles for English speech are below.")), langSelect) };
  const bnInput = h("input", { type: "checkbox", class: "switch", role: "switch" });
  bnInput.addEventListener("change", () => saveAi({ bnSubs: bnInput.checked }));
  const bnRow: SettingsRow = { label: "Bangla subtitles", el: h("label", { class: "sf-row sf-toggle" }, h("div", { class: "st-text" }, h("span", { class: "sf-label" }, "Translate English speech into Bangla subtitles"), h("p", { class: "sf-hint" }, "Done by the notes model; it adds time to a long meeting.")), bnInput) };
  // ---- processing: on this PC (default) or Cloud AI with the user's own keys
  const cloudNow = () => ai.processing === "cloud" && ai.cloudConsent;
  const nameOf = (id: CloudId) => providers.find((p) => p.id === id)?.name ?? id;
  const labelOf = (id: CloudId) => providers.find((p) => p.id === id)?.label ?? id;
  const processingRadio = (value: "local" | "cloud", label: string, hint: string) => {
    const input = h("input", { type: "radio", name: "meet-processing", value });
    input.addEventListener("change", () => void chooseProcessing(value));
    return { input, el: h("label", { class: "mtg-radio mtg-radio-block" }, input, h("span", null, h("span", { class: "mtg-radio-title" }, label), h("span", { class: "sf-hint mtg-radio-hint" }, hint))) };
  };
  const localRadio = processingRadio("local", "On this PC (default)", "Whisper and a small notes model run here. Nothing leaves the computer.");
  const cloudRadio = processingRadio("cloud", "Cloud AI (your own API keys)", "Claude, OpenAI, Gemini or OpenRouter do the listening and writing, and understand Bangla well. Their prices and terms apply to your key.");
  const processingRow: SettingsRow = { label: "Processing", el: h("div", { class: "sf-row wide" }, h("span", { class: "sf-label" }, "Processing"), localRadio.el, cloudRadio.el) };
  async function chooseProcessing(value: "local" | "cloud") {
    if (value === "local") return saveAi({ processing: "local" });
    if (!ai.cloudConsent) {
      const ok = await confirmSheet({
        title: "Send meetings to the cloud?",
        message: `Meeting audio (speech to text) will be sent to ${nameOf(ai.sttProvider)}, and transcript text (notes and translation) to ${nameOf(ai.llmProvider)}, using your own API keys. Their terms apply and you pay them. Nothing is sent while Cloud AI is off, and you can use this PC instead for any meeting.`,
        confirmLabel: "Turn On Cloud AI",
        icon: "upload",
      });
      if (!ok) return drawAi();
      return saveAi({ processing: "cloud", cloudConsent: true });
    }
    saveAi({ processing: "cloud" });
  }
  const noticeText = h("p", { class: "sf-hint mtg-notice" });
  const noticeRow: SettingsRow = { label: "What is sent", el: h("div", { class: "sf-row plain mtg-notice-row" }, h("span", { class: "mtg-notice-icon" }, icon("upload", 16)), noticeText) };
  const providerSelect = (label: string, pick: (id: CloudId) => void) => {
    const select = h("select", { class: "ops-input mtg-settings-select", "aria-label": label });
    select.addEventListener("change", () => pick(select.value as CloudId));
    return select;
  };
  const modelField = (label: string, task: "stt" | "llm") => {
    const input = h("input", { class: "ops-input mtg-settings-input mtg-model-input", type: "text", spellcheck: false, autocomplete: "off", autocapitalize: "off", "aria-label": label });
    const commit = () => {
      const provider = task === "stt" ? ai.sttProvider : ai.llmProvider;
      const key = `${provider}.${task}`;
      const value = input.value.trim();
      if ((ai.cloudModels[key] ?? "") === value) return;
      const cloudModels = { ...ai.cloudModels };
      if (value) cloudModels[key] = value;
      else delete cloudModels[key];
      saveAi({ cloudModels });
    };
    input.addEventListener("change", commit);
    input.addEventListener("keydown", (e) => {
      e.stopPropagation();
      if (e.key === "Enter") commit();
    });
    return input;
  };
  const sttSelect = providerSelect("Speech-to-text provider", (sttProvider) => saveAi({ sttProvider }));
  const sttModel = modelField("Speech-to-text model", "stt");
  const sttRow: SettingsRow = { label: "Speech to text", el: h("div", { class: "sf-row wide" }, h("span", { class: "sf-label" }, "Speech to text"),
    h("p", { class: "sf-hint" }, "Who listens to the meeting. Claude can't take audio, so it isn't offered here."), sttSelect, sttModel) };
  const llmSelect = providerSelect("Translation and notes provider", (llmProvider) => saveAi({ llmProvider }));
  const llmModel = modelField("Translation and notes model", "llm");
  const llmRow: SettingsRow = { label: "Translation and notes", el: h("div", { class: "sf-row wide" }, h("span", { class: "sf-label" }, "Translation and notes"),
    h("p", { class: "sf-hint" }, "Who writes the subtitles in the other language and the notes. Only transcript text is sent to it, never audio."), llmSelect, llmModel) };
  const fallbackInput = h("input", { type: "checkbox", class: "switch", role: "switch" });
  fallbackInput.addEventListener("change", () => saveAi({ fallbackLocal: fallbackInput.checked }));
  const fallbackRow: SettingsRow = { label: "Fall back", el: h("label", { class: "sf-row sf-toggle" }, h("div", { class: "st-text" }, h("span", { class: "sf-label" }, "Fall back to this PC if the cloud fails"),
    h("p", { class: "sf-hint" }, "Off: a failed run says what went wrong and nothing else happens. On: this PC does the job instead (English only, and it needs the models below).")), fallbackInput) };

  // One panel per provider in use: its key (kept in the system keyring, never shown again), a test, and where to get one.
  const keysBox = h("div", { class: "sf-row wide" });
  const keysRow: SettingsRow = { label: "API keys", el: keysBox };
  const panels = new Map<CloudId, { el: HTMLElement; update: (p: CloudProvider) => void }>();
  function keyPanel(first: CloudProvider) {
    const input = h("input", { class: "ops-input mtg-settings-input", type: "password", spellcheck: false, autocomplete: "off", "aria-label": `${first.name} API key` });
    const saved = h("span", { class: "mtg-token-saved" }, "Saved ✓ — replace or remove it");
    const replace = h("button", { class: "btn small ghost", type: "button" }, "Replace");
    const remove = h("button", { class: "btn small ghost", type: "button" }, "Remove");
    const savedLine = h("div", { class: "pl-settings-buttons mtg-token-line" }, saved, replace, remove);
    const save = h("button", { class: "btn small primary", type: "button" }, "Save key");
    const test = h("button", { class: "btn small", type: "button" }, "Test key");
    const result = h("p", { class: "sf-hint mtg-test", hidden: true });
    const where = h("p", { class: "sf-hint" });
    let current = first;
    const work = (button: HTMLButtonElement, job: Promise<unknown>) => {
      button.disabled = true;
      void job.finally(() => (button.disabled = false));
    };
    const edit = h("div", { class: "pl-settings-buttons mtg-key-edit" }, input, save);
    const doSave = () => {
      const key = input.value.trim();
      if (!key) return;
      work(save, meetApi.aiCloudKeySet(current.id, key).then((list) => {
        input.value = "";
        result.hidden = true;
        problem.hidden = true;
        setProviders(list);
      }, fail));
    };
    save.addEventListener("click", doSave);
    input.addEventListener("keydown", (e) => {
      e.stopPropagation();
      if (e.key === "Enter") doSave();
    });
    replace.addEventListener("click", () => {
      edit.hidden = false;
      input.focus();
    });
    remove.addEventListener("click", () => work(remove, meetApi.aiCloudKeyClear(current.id).then((list) => {
      result.hidden = true;
      setProviders(list);
    }, fail)));
    test.addEventListener("click", () => {
      result.hidden = false;
      result.dataset.result = "busy";
      result.textContent = "Testing…";
      work(test, meetApi.aiCloudTest(current.id).then((message) => {
        result.dataset.result = "this-pc";
        result.textContent = `✓ ${message}`;
      }, (err) => {
        result.dataset.result = "unreachable";
        result.textContent = `✗ ${errorText(err)}`;
      }));
    });
    const el = h("div", { class: "mtg-key-panel", "data-provider": first.id }, h("span", { class: "sf-label" }, `${first.label} key`), savedLine, edit, h("div", { class: "pl-settings-buttons" }, test), result, where);
    return {
      el,
      update(p: CloudProvider) {
        current = p;
        savedLine.hidden = !p.keySet;
        edit.hidden = p.keySet;
        input.placeholder = `Paste your ${p.name} API key`;
        test.disabled = !p.keySet;
        test.title = p.keySet ? "Sends a tiny request with the saved key (a tiny charge)" : "Save a key first";
        where.textContent = `Get one at ${p.keyPage.replace(/^https:\/\//, "")}. It is kept in the system keyring (or, with none, in a file only you can read) and is never shown again.`;
      },
    };
  }
  function setProviders(list: CloudProvider[]) {
    providers = list;
    drawAi();
  }
  function drawKeys() {
    const used = [...new Set([ai.sttProvider, ai.llmProvider])];
    const nodes: HTMLElement[] = [];
    for (const id of used) {
      const p = providers.find((x) => x.id === id);
      if (!p) continue;
      let panel = panels.get(id);
      if (!panel) {
        panel = keyPanel(p);
        panels.set(id, panel);
      }
      panel.update(p);
      nodes.push(panel.el);
    }
    keysBox.replaceChildren(h("span", { class: "sf-label" }, "API keys"), ...nodes);
  }

  const modelsBox = h("div", { class: "sf-row wide" });
  const modelsRow: SettingsRow = { label: "Models", el: modelsBox };
  let models: AiModel[] = [];
  function drawModels() {
    modelsBox.replaceChildren(h("span", { class: "sf-label" }, "Models on this PC"), h("p", { class: "sf-hint" }, "Downloaded once, when the first meeting needs them, and checked against a fixed checksum."),
      ...models.map((m) => {
        const button = m.installed
          ? h("button", { class: "btn small", type: "button", onclick: () => void meetApi.aiModelRemove(m.id).then(refreshModels, fail) }, "Delete")
          : m.downloading
            ? h("button", { class: "btn small", type: "button", onclick: () => void meetApi.aiModelCancel(m.id).then(refreshModels, fail) }, `Pause (${Math.round((m.partial / m.bytes) * 100)}%)`)
            : h("button", { class: "btn small primary", type: "button", onclick: () => void meetApi.aiModelDownload(m.id).then(refreshModels, fail) }, `Download ${formatBytes(m.bytes)}`);
        return h("div", { class: "pl-settings-buttons mtg-model" }, h("div", { class: "st-text" }, h("span", { class: "sf-label" }, m.label), h("p", { class: "sf-hint" }, m.installed ? `Installed · ${m.note}` : m.error ?? m.note)), button);
      }));
  }
  const refreshModels = () => meetApi.aiModels().then((list) => {
    models = list;
    drawModels();
  }, () => undefined);
  function drawAi() {
    autoInput.checked = ai.auto;
    modelSelect.value = ai.model;
    langSelect.value = ai.notesLang;
    bnInput.checked = ai.bnSubs;
    const cloud = ai.processing === "cloud";
    localRadio.input.checked = !cloud;
    cloudRadio.input.checked = cloud;
    for (const row of [noticeRow, sttRow, llmRow, keysRow, fallbackRow]) row.el.hidden = !cloud;
    // Bangla: this PC's gate, or a cloud speech-to-text provider.
    const bangla = banglaAvailable(ai);
    langRow.el.hidden = !bangla;
    bnRow.el.hidden = !bangla;
    fallbackInput.checked = ai.fallbackLocal;
    sttSelect.replaceChildren(...providers.filter((p) => p.canTranscribe).map((p) => h("option", { value: p.id }, p.label)));
    llmSelect.replaceChildren(...providers.map((p) => h("option", { value: p.id }, p.label)));
    sttSelect.value = ai.sttProvider;
    llmSelect.value = ai.llmProvider;
    const defaults = (id: CloudId, task: "stt" | "llm") => {
      const p = providers.find((x) => x.id === id);
      return (task === "stt" ? p?.sttDefault : p?.llmDefault) ?? "";
    };
    for (const [input, id, task] of [[sttModel, ai.sttProvider, "stt"], [llmModel, ai.llmProvider, "llm"]] as const) {
      input.placeholder = defaults(id, task);
      if (document.activeElement !== input) input.value = ai.cloudModels[`${id}.${task}`] ?? "";
    }
    noticeText.textContent = `Meeting audio (speech to text) will be sent to ${nameOf(ai.sttProvider)}, and transcript text (notes and translation) to ${nameOf(ai.llmProvider)}, with your own key${ai.sttProvider === ai.llmProvider ? "" : "s"}. Nothing is sent unless Cloud AI is on. ${labelOf(ai.sttProvider)} hears each speaker's track on its own, so each person keeps their name.`;
    drawAutoHint();
    drawKeys();
  }

  // ---- connection
  const field = (placeholder: string, label: string, type = "text") => h("input", { class: "ops-input mtg-settings-input", type, placeholder, spellcheck: false, autocomplete: "off", "aria-label": label });
  const stun = h("textarea", { class: "ops-input mtg-settings-stun", rows: 3, spellcheck: false, placeholder: "stun:stun.example.com:3478", "aria-label": "STUN servers" });
  const turnUrl = field("turn:turn.example.com:3478", "TURN server address");
  const turnUser = field("Username", "TURN username");
  const turnPass = field("Credential", "TURN credential", "password");
  const save = h("button", { class: "btn small primary", type: "button" }, "Save");
  save.addEventListener("click", () => {
    const servers = stun.value.split(/[\s,]+/).filter(Boolean);
    const url = turnUrl.value.trim();
    save.disabled = true;
    void meetApi.settings(servers, url ? { url, username: turnUser.value.trim(), credential: turnPass.value } : null).then((next) => {
      problem.hidden = true;
      draw(next);
    }, fail).finally(() => (save.disabled = false));
  });
  const stunRow: SettingsRow = { label: "STUN servers", el: h("div", { class: "sf-row wide" }, h("span", { class: "sf-label" }, "STUN servers"), h("p", { class: "sf-hint" }, "One per line. They help two browsers find each other."), stun) };
  const turnRow: SettingsRow = { label: "TURN server", el: h("div", { class: "sf-row wide" }, h("span", { class: "sf-label" }, "TURN server (optional)"), h("p", { class: "sf-hint" }, "Relays sound and picture for guests behind a strict network. Without one, some of them can't connect."), turnUrl, turnUser, turnPass, h("div", { class: "pl-settings-buttons" }, save)) };

  function draw(next: MeetState | null) {
    if (next) state = next;
    const s = state;
    if (!s) {
      tunnelHint.textContent = "Meet isn't available in this version of Zazabor Home.";
      tunnelInput.disabled = true;
      return;
    }
    tunnelInput.disabled = false;
    const mode = shownMode();
    quickRadio.input.checked = mode === "quick";
    domainRadio.input.checked = mode === "domain";
    domainRow.el.hidden = mode !== "domain";
    if (document.activeElement !== hostInput) hostInput.value = s.hostname;
    tokenState.textContent = s.tokenSet ? "Saved ✓ — type a new token to replace it" : "";
    tokenLine.hidden = !s.tokenSet;
    tokenInput.placeholder = s.tokenSet ? "Replace the saved token…" : "Paste the token from Cloudflare";
    test.disabled = !(s.mode === "domain" && s.tokenSet && s.hostname);
    test.title = test.disabled ? "Save the domain first" : "";
    addressHint.textContent = s.mode === "domain" ? "Your own domain: the address stays the same, so invitations never change." : "Changes each time the link starts; invitations carry the current one.";
    tunnelInput.checked = s.tunnel.state === "on" || s.tunnel.state === "starting";
    tunnelHint.textContent = s.tunnel.state === "on" ? "On. Guests join through the address below."
      : s.tunnel.state === "starting" ? "Starting…"
      : s.tunnel.state === "error" ? `Couldn't start: ${s.tunnel.error ?? "unknown error"}`
      : "Off. It starts by itself when you share a link: guests need its https address, since browsers only allow the camera and mic on a secure page.";
    addressText.textContent = s.tunnel.url ?? "—";
    copy.hidden = !s.tunnel.url;
    folder.textContent = s.recordingsDir;
    if (document.activeElement !== stun) stun.value = s.stun.join("\n");
    turnUrl.value = s.turn?.url ?? "";
    turnUser.value = s.turn?.username ?? "";
    turnPass.value = s.turn?.credential ?? "";
  }
  void onMeet("meet-changed", () => void meetApi.state().then(draw, () => undefined)).catch(() => undefined);
  void onMeet("meet-ai", (e) => e.models && void refreshModels()).catch(() => undefined);

  return {
    id: "meet", title: "Meet", icon: "video", section: "Modules",
    intro: problem,
    groups: [
      { title: "Outside link", rows: [tunnelRow, modeRow, domainRow, addressRow] },
      { title: "Recordings", rows: [folderRow] },
      { title: "Transcripts and notes", rows: [autoRow, processingRow, noticeRow, sttRow, llmRow, keysRow, fallbackRow, modelChoiceRow, langRow, bnRow, modelsRow] },
      { title: "Connection", rows: [stunRow, turnRow] },
    ],
    refresh() {
      problem.hidden = true;
      void meetApi.state().then(draw, () => draw(null));
      void Promise.all([meetApi.aiSettings(), meetApi.aiCloudState().catch(() => [] as CloudProvider[])]).then(([s, list]) => {
        ai = s;
        providers = list;
        drawAi();
      }, () => undefined);
      void refreshModels();
    },
  };
}
