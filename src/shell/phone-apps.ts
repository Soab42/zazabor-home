import { invoke } from "@tauri-apps/api/core";
import { formatBytes, h } from "../ui/dom";
import { icon, type IconName } from "../ui/icons";
import { showToast } from "../modules/files/ops/toast";
import type { Category } from "./settings";
import "./phone-apps.css";

/**
 * Settings → Phone apps: a QR code per Android app, to scan with the phone's camera and install.
 * The links point at the latest release's fixed asset names (scripts/release-phone-apps.sh), so a
 * code never goes stale; versions come from that release's phone-apps.json when online.
 */

const BASE = "https://github.com/Soab42/zazabor-home/releases/latest/download/";

export interface PhoneApp {
  key: "link" | "wallet" | "tv";
  name: string;
  file: string;
  glyph: IconName;
  blurb: string;
}

export const PHONE_APPS: PhoneApp[] = [
  { key: "link", name: "Zazabor Link", file: "zazabor-link.apk", glyph: "phone", blurb: "Texts, calls, photos, files and notifications on this PC; mirror the phone's screen." },
  { key: "wallet", name: "Zazabor Wallet", file: "zazabor-wallet.apk", glyph: "wallet", blurb: "Your wallet on the phone, kept in sync with this PC." },
  { key: "tv", name: "Zazabor TV", file: "zazabor-tv.apk", glyph: "tv", blurb: "DhakaFlix movies, series and Live TV on the phone; plays in VLC." },
];

export const phoneAppUrl = (app: PhoneApp) => BASE + app.file;

type Versions = Record<string, { version?: string; bytes?: number }> | null;

export function phoneAppsCategory(): Category {
  const cards = h("div", { class: "pa-grid" });
  let versions: Versions = null;
  let asked = false;

  function card(app: PhoneApp): HTMLElement {
    return phoneAppCard(app, versions?.[app.key]);
  }

  function draw() {
    cards.replaceChildren(...PHONE_APPS.map(card));
  }

  const intro = h("div", { class: "pa-intro" },
    h("p", { class: "sf-hint" }, "Scan a code with your Android phone's camera to download the app, then open the file to install it. The apps update themselves after that."),
    cards,
    h("p", { class: "sf-hint pa-note" }, "Android may ask you to allow installing apps from your browser or files app: allow it for this install."));
  draw();

  return {
    id: "phone-apps",
    title: "Phone apps",
    icon: "phone",
    section: "App",
    intro,
    groups: [],
    refresh() {
      if (asked) return;
      asked = true;
      void invoke<Versions>("phone_apps_info").then((v) => {
        versions = v;
        if (v) draw();
      }, () => undefined);
    },
  };
}

/** One app's card: name, version, QR code, what it does, Copy link. Also shown in its section's settings. */
export function phoneAppCard(app: PhoneApp, info?: { version?: string; bytes?: number }): HTMLElement {
  const url = phoneAppUrl(app);
  const qr = h("div", { class: "pa-qr", role: "img", "aria-label": `QR code to download ${app.name}` });
  void invoke<string>("qr_svg", { text: url }).then(
    (svg) => {
      // Rust drew it (the qrcode crate). As an image it can't run anything.
      qr.replaceChildren(h("img", { src: `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`, alt: "", draggable: false }));
    },
    () => qr.replaceChildren(h("span", { class: "pa-qr-fail" }, "Couldn't draw the code")),
  );
  const meta = [info?.version ? `Version ${info.version}` : "", info?.bytes ? formatBytes(info.bytes) : ""].filter(Boolean).join(" · ");
  const copy = h("button", { class: "btn small", type: "button" }, icon("link", 14), "Copy link");
  copy.addEventListener("click", () => {
    void navigator.clipboard.writeText(url).then(() => showToast(`${app.name} link copied`), () => showToast(url, undefined, 8000));
  });
  return h("section", { class: "pa-card", "data-app": app.key },
    h("div", { class: "pa-head" }, h("span", { class: "pa-icon" }, icon(app.glyph, 18)), h("div", { class: "pa-titles" }, h("strong", null, app.name), meta ? h("span", null, meta) : null)),
    qr,
    h("p", { class: "pa-blurb" }, app.blurb),
    copy);
}
