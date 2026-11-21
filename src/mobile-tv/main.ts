import "../styles.css";
import "../modules/tv/settings.css";
import "./mobile-tv.css";
import { applyAppearance } from "../appearance";
import { parseConfig, type Config } from "../config";
import type { OpenRequest, ShellContext } from "../shell/module";
import { createTvModule } from "../modules/tv";
import { tvCategory } from "../modules/tv/settings";
import { h } from "../ui/dom";
import { icon } from "../ui/icons";
import { showToast } from "../modules/files/ops/toast";

/**
 * Zazabor TV on Android: the desktop's TV section (src/modules/tv) with a phone shell around it.
 * Settings live in the WebView's storage since there is no desktop config here. Playing is VLC's
 * job: a video link is handed to it through the app's `ZazaborPlayer` bridge (MainActivity.kt).
 */

const CONFIG_KEY = "zazabor-tv-config";
const HINT_KEY = "zazabor-tv-vlc-hint";

function loadConfig(): Config {
  try {
    const raw = localStorage.getItem(CONFIG_KEY);
    if (raw) return parseConfig(raw);
  } catch (err) {
    console.warn("TV settings unreadable, using defaults:", err);
  }
  return parseConfig("{}");
}

const config = loadConfig();
// TV is a dark app: its hero and detail pages are dark whatever the theme.
config.appearance.mode = "dark";
applyAppearance(config.appearance);

interface Android {
  ZazaborPlayer?: { open(url: string, title: string, listJson: string): string; hasVlc(): boolean; getVlc(): void };
  ZazaborSystem?: { bottomInset(): number };
  ZazaborUpdates?: { check(): void; version(): string };
}
const android = window as unknown as Android;

// ---- playing: VLC

/** Longest list handed to VLC (a Live TV list can be hundreds of channels). */
const MAX_LIST = 100;

function playExternal(request: Extract<OpenRequest, { kind: "video" }>) {
  const player = android.ZazaborPlayer;
  if (!player) return showToast("Playing works in the Zazabor TV app on a phone.");
  const all = request.siblings ?? [request.path];
  const at = Math.max(0, all.indexOf(request.path));
  const list = all.slice(at, at + MAX_LIST).map((url, n) => ({ url, title: request.titles?.[at + n] ?? "" }));
  const answer = player.open(request.path, request.title ?? "", JSON.stringify(list));
  if (answer === "vlc") return;
  if (answer === "none") showToast("No video player is installed. Install VLC to play.", undefined, 4500);
  if (!localStorage.getItem(HINT_KEY) || answer === "none") vlcHint(player);
}

/** Said once (and whenever nothing can play at all): VLC plays these best. */
function vlcHint(player: NonNullable<Android["ZazaborPlayer"]>) {
  localStorage.setItem(HINT_KEY, "1");
  const close = () => sheet.remove();
  const get = h("button", { class: "tv-pill primary", type: "button", onclick: () => (player.getVlc(), close()) }, icon("external", 16), "Get VLC");
  const later = h("button", { class: "tv-pill glass", type: "button", onclick: close }, "Not now");
  const sheet = h("div", { class: "tvm-hint-wrap", onclick: (e: Event) => e.target === sheet && close() },
    h("div", { class: "tvm-hint", role: "dialog", "aria-label": "Play with VLC" },
      h("h2", null, "Play with VLC"),
      h("p", null, "Zazabor TV hands videos to a player app, and VLC plays these files best (audio tracks, subtitles, episodes one after another). Without it, any video player on this phone is offered."),
      h("div", { class: "tvm-hint-actions" }, get, later)));
  document.body.append(sheet);
}

// ---- the shell around the module

let saveTimer: number | undefined;
const shell: ShellContext = {
  config,
  saveConfig() {
    clearTimeout(saveTimer);
    saveTimer = window.setTimeout(() => {
      try {
        localStorage.setItem(CONFIG_KEY, JSON.stringify(config));
      } catch (err) {
        console.error("Saving TV settings failed:", err);
      }
    }, 300);
  },
  open: (request) => {
    if (request.kind === "video") playExternal(request);
  },
  openExternally: () => {},
  showModule: () => {},
  openSettings: () => openSettings(),
  lock: { enabled: false, lockNow: () => {}, authorize: async () => true, openSettings: () => {} },
};

const tvModule = await createTvModule(shell);
const app = document.getElementById("app")!;
app.append(tvModule.el);
tvModule.activate();

// The gesture or button bar's height, which the WebView doesn't report as a safe area.
if (android.ZazaborSystem) {
  const system = android.ZazaborSystem;
  const inset = () => document.documentElement.style.setProperty("--nav-bottom", `${system.bottomInset()}px`);
  inset();
  window.addEventListener("resize", inset);
}

// ---- settings: the desktop's Settings → TV page, in a full-screen sheet

const category = tvCategory({
  config,
  configChanged() {
    shell.saveConfig();
    tvModule.configChanged?.();
  },
});

let sheet: HTMLElement | null = null;

function openSettings() {
  if (sheet) return;
  category.refresh?.();
  const body = h("div", { class: "tvs-body" },
    ...category.groups.map((g) => h("section", { class: "tvs-group" }, h("h2", null, g.title), h("div", { class: "tvs-card" }, ...g.rows.map((r) => r.el)))));
  const updates = android.ZazaborUpdates;
  if (updates) {
    body.append(h("section", { class: "tvs-group" }, h("h2", null, "About"), h("div", { class: "tvs-card" },
      h("div", { class: "sf-row plain tv-set-row" }, h("div", { class: "st-text" }, h("span", { class: "sf-label" }, `Zazabor TV ${updates.version()}`), h("p", { class: "sf-hint" }, "The movie servers answer only on the ISP's network (BDIX). Videos open in VLC.")),
        h("div", { class: "tv-set-buttons" }, h("button", { class: "btn small", type: "button", onclick: () => updates.check() }, "Check for Updates"))))));
  }
  const back = h("button", { class: "tv-top-btn", type: "button", "aria-label": "Back", onclick: closeSettings }, icon("chevronLeft", 22));
  sheet = h("div", { class: "tvs", role: "dialog", "aria-label": "Settings" }, h("header", { class: "tvs-bar" }, back, h("h1", null, "Settings")), body);
  document.body.append(sheet);
  requestAnimationFrame(() => sheet?.classList.add("in"));
}

function closeSettings() {
  const closing = sheet;
  if (!closing) return;
  sheet = null;
  closing.classList.remove("in");
  window.setTimeout(() => closing.remove(), 220);
}

// Android's back button: closes settings or the detail page, or steps back through the pages; only
// from Home does it leave the app.
history.pushState({ tv: true }, "");
addEventListener("popstate", () => {
  const handled = sheet ? (closeSettings(), true) : tvModule.handleKey(new KeyboardEvent("keydown", { key: "Escape" }));
  if (handled) history.pushState({ tv: true }, "");
});
