import { h } from "../../ui/dom";

/**
 * The services an account can be made for: their names, start addresses and bundled icons (small
 * drawings of our own, so nothing is fetched at run time). The addresses must match the Rust
 * catalogue (`src-tauri/src/messages/services.rs`), which checks them again.
 */

export interface ServiceChoice {
  label: string;
  hint: string;
  url: string;
}

export interface ServiceInfo {
  id: string;
  name: string;
  blurb: string;
  /** More than one start address: the sheet asks which. */
  choices?: ServiceChoice[];
  /** Known to refuse sign-in inside an embedded web view (Google accounts): the sheet suggests the browser. */
  browserRecommended?: boolean;
  /** Where an account starts (empty for a custom one). */
  url: string;
  /** A drawing on a 24-unit square, as SVG markup. */
  art: string;
}

const bg = (fill: string, inner: string, defs = "") => `${defs}<rect width="24" height="24" rx="6" fill="${fill}"/>${inner}`;
const gradient = (id: string, a: string, b: string, c?: string) =>
  `<defs><linearGradient id="${id}" x1="0" y1="1" x2="1" y2="0"><stop offset="0" stop-color="${a}"/>${c ? `<stop offset="0.5" stop-color="${b}"/><stop offset="1" stop-color="${c}"/>` : `<stop offset="1" stop-color="${b}"/>`}</linearGradient></defs>`;
const white = `fill="#fff"`;
const line = `fill="none" stroke="#fff" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"`;

export const SERVICES: ServiceInfo[] = [
  {
    id: "whatsapp", name: "WhatsApp", blurb: "Scan a QR code with your phone",
    art: bg("#25d366", `<path d="M12 4.6a7.4 7.4 0 0 0-6.3 11.2L4.6 19.4l3.7-1A7.4 7.4 0 1 0 12 4.6z" ${line}/><path d="M9.4 8.8c-.3.4-.3 1 .1 1.7.6 1.2 1.5 2.1 2.8 2.7.8.3 1.3.3 1.7-.1l.4-.6-1.4-.9-.6.4c-.8-.4-1.4-1-1.8-1.8l.4-.6-.9-1.4z" ${white}/>`),
    url: "https://web.whatsapp.com/",
  },
  {
    id: "messenger", name: "Messenger", blurb: "Facebook Messenger",
    art: bg("url(#g-msgr)", `<path d="M12 4.4c-4.2 0-7.6 3.1-7.6 7.1 0 2.2 1 4.1 2.6 5.4V20l2.6-1.4c.7.2 1.5.3 2.4.3 4.2 0 7.6-3.1 7.6-7.1S16.200 4.400 12 4.400z" ${white}/><path d="M7.600 13.600l3.200-3.400 2 1.900 3.100-1.900-3.200 3.400-2-1.900z" fill="#a033ff"/>`, gradient("g-msgr", "#0099ff", "#a033ff", "#ff5280")),
    url: "https://www.messenger.com/",
  },
  {
    id: "teams", name: "Microsoft Teams", blurb: "Work or school, or personal",
    art: bg("#5059c9", `<path d="M7.200 8.600h7.200M10.800 8.600V17" ${line} stroke-width="2.200"/><circle cx="17.200" cy="7.600" r="1.700" ${white}/><path d="M15.500 11h3.800c.6 0 1 .4 1 1v2.600c0 1.200-1 2-2.200 2-.6 0-1.100-.2-1.500-.6" ${line}/>`),
    url: "https://teams.microsoft.com/",
    choices: [
      { label: "Work or school", hint: "teams.microsoft.com", url: "https://teams.microsoft.com/" },
      { label: "Personal", hint: "teams.live.com", url: "https://teams.live.com/" },
    ],
  },
  {
    id: "telegram", name: "Telegram", blurb: "Scan a QR code or use your number",
    art: bg("#229ed9", `<path d="M5 11.700l13.200-5.100c.7-.2 1.200.2 1 1.100l-2.200 10.500c-.2.800-.7 1-1.300.6l-3.300-2.400-1.600 1.500c-.2.200-.4.300-.7.300l.2-3.400 6.200-5.600c.3-.2-.1-.4-.4-.2l-7.700 4.800-3.200-1c-.7-.2-.7-.7.200-1.100z" ${white}/>`),
    url: "https://web.telegram.org/a/",
  },
  {
    id: "instagram", name: "Instagram", blurb: "Direct messages",
    art: bg("url(#g-ig)", `<rect x="6" y="6" width="12" height="12" rx="3.600" ${line}/><circle cx="12" cy="12" r="2.900" ${line}/><circle cx="15.700" cy="8.300" r="0.900" ${white}/>`, gradient("g-ig", "#feda75", "#d62976", "#4f5bd5")),
    url: "https://www.instagram.com/direct/inbox/",
  },
  {
    id: "slack", name: "Slack", blurb: "Your workspace",
    art: bg("#4a154b", `<rect x="8.200" y="4.800" width="2.200" height="14.400" rx="1.100" fill="#36c5f0"/><rect x="13.600" y="4.800" width="2.200" height="14.400" rx="1.100" fill="#2eb67d"/><rect x="4.800" y="8.200" width="14.400" height="2.200" rx="1.100" fill="#ecb22e"/><rect x="4.800" y="13.600" width="14.400" height="2.200" rx="1.100" fill="#e01e5a"/>`),
    url: "https://app.slack.com/client",
  },
  {
    id: "discord", name: "Discord", blurb: "Servers and direct messages",
    art: bg("#5865f2", `<path d="M8 8.100c1.300-.6 2.600-.9 4-.9s2.700.3 4 .9c1.200 1.900 1.800 3.900 1.900 6.100-1 .8-2 1.200-3 1.500l-.7-1.100c.4-.1.800-.3 1.100-.5-2.200.9-4.400.9-6.600 0 .3.200.7.400 1.100.5l-.7 1.100c-1-.3-2-.7-3-1.500.1-2.200.7-4.200 1.900-6.100z" ${white}/><circle cx="9.900" cy="12.400" r="1.100" fill="#5865f2"/><circle cx="14.100" cy="12.400" r="1.100" fill="#5865f2"/>`),
    url: "https://discord.com/app",
  },
  {
    id: "google-chat", name: "Google Chat", blurb: "Chat and spaces",
    art: bg("#00ac47", `<path d="M6 7.200c0-.8.600-1.500 1.500-1.500h9c.8 0 1.500.7 1.500 1.500v7c0 .8-.7 1.500-1.500 1.500H11l-3.500 3v-3h0C6.600 15.700 6 15 6 14.200z" ${white}/>`),
    url: "https://chat.google.com/", browserRecommended: true,
  },
  {
    id: "google-messages", name: "Google Messages", blurb: "Your phone's texts, paired by QR",
    art: bg("#1a73e8", `<path d="M6 7.200c0-.8.600-1.500 1.500-1.500h9c.8 0 1.500.7 1.500 1.500v7c0 .8-.7 1.500-1.500 1.500H11l-3.500 3v-3h0C6.600 15.700 6 15 6 14.200z" ${white}/><circle cx="9.200" cy="10.700" r="1" fill="#1a73e8"/><circle cx="12" cy="10.700" r="1" fill="#1a73e8"/><circle cx="14.800" cy="10.700" r="1" fill="#1a73e8"/>`),
    url: "https://messages.google.com/web", browserRecommended: true,
  },
  {
    id: "linkedin", name: "LinkedIn", blurb: "Messaging",
    art: bg("#0a66c2", `<rect x="6.500" y="10" width="2.400" height="7.500" ${white}/><circle cx="7.700" cy="7.400" r="1.400" ${white}/><path d="M11 10h2.300v1c.5-.8 1.300-1.200 2.400-1.200 2 0 2.800 1.200 2.800 3.300v4.400h-2.400v-4c0-1-.3-1.600-1.200-1.600s-1.500.6-1.500 1.700v3.900H11z" ${white}/>`),
    url: "https://www.linkedin.com/messaging/",
  },
  {
    id: "custom", name: "Custom web app", blurb: "Any website that needs a login (https)",
    art: bg("#6b7385", `<circle cx="12" cy="12" r="6.600" ${line}/><ellipse cx="12" cy="12" rx="2.800" ry="6.600" ${line}/><path d="M5.400 12h13.200" ${line}/>`),
    url: "",
  },
];

export const serviceInfo = (id: string): ServiceInfo => SERVICES.find((s) => s.id === id) ?? SERVICES[SERVICES.length - 1];

/** A service's icon: its drawing on a rounded square. */
export function serviceIcon(id: string, size = 28): HTMLElement {
  const holder = h("span", { class: "msg-svc", "aria-hidden": "true" });
  holder.style.width = holder.style.height = `${size}px`;
  holder.innerHTML = `<svg viewBox="0 0 24 24" width="${size}" height="${size}">${serviceInfo(id).art}</svg>`;
  return holder;
}
