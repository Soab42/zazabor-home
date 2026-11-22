import { invoke } from "@tauri-apps/api/core";
import type { Config } from "../config";
import { iconMarkup } from "../ui/icons";
import { MODULE_IDS, type ModuleId } from "./module";
import { MODULES } from "./registry";

/** How each section looks as an app of its own: freedesktop categories, and its icon tile's gradient. */
const LOOK: Record<ModuleId, { categories: string[]; from: string; to: string }> = {
  files: { categories: ["Utility", "FileManager", "Core"], from: "#7ab4ff", to: "#2d63e6" },
  gallery: { categories: ["Graphics", "Viewer"], from: "#6fe2a4", to: "#1f9d61" },
  video: { categories: ["AudioVideo", "Video", "Player"], from: "#ff8a7a", to: "#d6334a" },
  terminal: { categories: ["System", "TerminalEmulator"], from: "#4b5365", to: "#14171d" },
  notes: { categories: ["Utility", "TextTools"], from: "#ffe27a", to: "#f0a91c" },
  time: { categories: ["Office", "ProjectManagement"], from: "#b69cff", to: "#6a45e0" },
  wallet: { categories: ["Office", "Finance"], from: "#ffc46b", to: "#ff7417" },
  office: { categories: ["Office"], from: "#7ab4ff", to: "#3b7be8" },
  studio: { categories: ["AudioVideo", "Graphics", "AudioVideoEditing"], from: "#ff9ad0", to: "#c43fa4" },
  clipboard: { categories: ["Utility"], from: "#8fe0d4", to: "#1b9a8e" },
  mail: { categories: ["Network", "Email"], from: "#7cc4ff", to: "#2a7fe0" },
  phone: { categories: ["Network", "Telephony"], from: "#7be0a0", to: "#18a05a" },
  apps: { categories: ["System", "Settings"], from: "#c3a27a", to: "#8a5a2c" },
  meet: { categories: ["Network", "VideoConference", "Chat"], from: "#8a9bff", to: "#4a47d6" },
  messages: { categories: ["Network", "InstantMessaging", "Chat"], from: "#6fe0c8", to: "#11a37f" },
  tv: { categories: ["AudioVideo", "Video", "TV"], from: "#ff7ab0", to: "#b3246a" },
};

/** The app-menu entries of the enabled sections (none when the setting is off), for the backend to write. */
export function sectionApps(config: Config) {
  if (!config.sectionApps) return [];
  return MODULE_IDS.filter((id) => config.features[id]).map((id) => ({
    id,
    title: MODULES[id].title,
    comment: MODULES[id].blurb,
    glyph: iconMarkup(MODULES[id].icon),
    ...LOOK[id],
  }));
}

/** Brings the desktop's app menu in line with the sections that are on; safe to call as often as needed. */
export function syncSectionApps(config: Config) {
  invoke("sections_sync", { sections: sectionApps(config) }).catch((err) => console.warn("Could not update the section apps:", err));
}
