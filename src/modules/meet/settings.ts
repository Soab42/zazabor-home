import "../files/ops/ops.css";
import "./meet.css";
import { invoke } from "@tauri-apps/api/core";
import type { SettingsRow } from "../../appearance";
import type { Config } from "../../config";
import type { Category } from "../../shell/settings";
import { formatBytes, h } from "../../ui/dom";
import { errorText, meetApi, onMeet, type AiModel, type AiSettings, type MeetState } from "./api";
import { BANGLA_READY } from "./meeting";

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
  let ai: AiSettings = { auto: true, model: "auto", notesLang: "en", bnSubs: BANGLA_READY };
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
  const autoRow: SettingsRow = { label: "Transcribe automatically", el: h("label", { class: "sf-row sf-toggle" }, h("div", { class: "st-text" }, h("span", { class: "sf-label" }, "Transcribe and take notes automatically"), h("p", { class: "sf-hint" }, BANGLA_READY ? "When a recorded meeting ends, this PC writes who said what (Bangla or English), subtitles in both languages and notes. Nothing leaves the computer. You can turn it off for one meeting, or run it later from the meeting." : "When a recorded meeting ends, this PC writes who said what, English subtitles and notes. Nothing leaves the computer. Bangla is coming in a later update. You can turn it off for one meeting, or run it later from the meeting.")), autoInput) };
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
      { title: "Transcripts and notes", rows: BANGLA_READY ? [autoRow, modelChoiceRow, langRow, bnRow, modelsRow] : [autoRow, modelChoiceRow, modelsRow] },
      { title: "Connection", rows: [stunRow, turnRow] },
    ],
    refresh() {
      problem.hidden = true;
      void meetApi.state().then(draw, () => draw(null));
      void meetApi.aiSettings().then((s) => {
        ai = s;
        drawAi();
      }, () => undefined);
      void refreshModels();
    },
  };
}
