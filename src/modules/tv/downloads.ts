import { h } from "../../ui/dom";
import { icon } from "../../ui/icons";
import { isPhone } from "./phone";
import { onTv, sizeText, titleKey, tvApi, type Download, type DownloadRequest, type Item } from "./api";
import { eta } from "./progress";

/**
 * Downloads, on both apps. On the PC the backend is the Rust downloader (`src-tauri/src/tv/download.rs`:
 * resumable, two at a time, events as it goes); on the phone it is Android's DownloadManager through
 * the `ZazaborDownloads` bridge (`MainActivity.kt`), which has no pause. The page only sees `Download`s.
 */
export interface Backend {
  readonly canPause: boolean;
  list(): Promise<Download[]>;
  add(requests: DownloadRequest[]): Promise<void>;
  pause(id: string): Promise<void>;
  resume(id: string): Promise<void>;
  remove(id: string, deleteFile: boolean): Promise<void>;
  /** Plays a finished download (the PC's player, or VLC on the phone). */
  play(d: Download): void;
  /** Reveals it in the file manager (the PC only). */
  show?(d: Download): void;
  /** Calls back with the whole list whenever it changes; returns the way to stop. */
  subscribe(fn: (all: Download[]) => void): () => void;
}

interface AndroidDownloads {
  list(): string;
  add(json: string): string;
  remove(id: string): void;
  play(id: string): string;
}

/** The Android bridge, when this is the phone app. */
const bridge = (): AndroidDownloads | undefined => (window as unknown as { ZazaborDownloads?: AndroidDownloads }).ZazaborDownloads;

export function desktopBackend(opts: { dir: () => string; play: (d: Download) => void; show: (d: Download) => void }): Backend {
  return {
    canPause: true,
    list: () => tvApi.dlList(),
    add: async (requests) => void (await tvApi.dlAdd(requests, opts.dir())),
    pause: (id) => tvApi.dlPause(id),
    resume: (id) => tvApi.dlResume(id),
    remove: (id, deleteFile) => tvApi.dlRemove(id, deleteFile),
    play: opts.play,
    show: opts.show,
    subscribe(fn) {
      const stop = onTv("tv-downloads", (all) => fn(all));
      return () => void stop.then((u) => u());
    },
  };
}

export function phoneBackend(opts: { wifiOnly: () => boolean }): Backend | null {
  const b = bridge();
  if (!b) return null;
  const read = (): Download[] => {
    try {
      return JSON.parse(b.list()) as Download[];
    } catch {
      return [];
    }
  };
  return {
    canPause: false,
    list: async () => read(),
    add: async (requests) => {
      const answer = b.add(JSON.stringify({ wifiOnly: opts.wifiOnly(), items: requests }));
      if (answer.startsWith("error:")) throw answer.slice(6).trim();
    },
    pause: async () => undefined,
    resume: async () => undefined,
    remove: async (id) => b.remove(id),
    play: (d) => {
      const answer = b.play(d.id);
      if (answer === "none") throw "No video player is installed. Install VLC to play.";
    },
    subscribe(fn) {
      // DownloadManager has no events for the page: ask now and then (often while something runs)
      let timer = 0;
      let stopped = false;
      const tick = () => {
        if (stopped) return;
        const all = read();
        fn(all);
        timer = window.setTimeout(tick, all.some((d) => d.state === "queued" || d.state === "running") ? 1200 : 6000);
      };
      tick();
      return () => {
        stopped = true;
        clearTimeout(timer);
      };
    },
  };
}

/** The list of downloads with their speed, shared by the whole section. */
export class DownloadsStore {
  list: Download[] = [];
  private subs = new Set<() => void>();
  private last = new Map<string, { got: number; at: number; bps: number }>();
  private stop: (() => void) | null = null;

  constructor(readonly backend: Backend | null) {}

  get canDownload() {
    return this.backend !== null;
  }

  async start() {
    if (!this.backend || this.stop) return;
    this.stop = this.backend.subscribe((all) => this.update(all));
    try {
      this.update(await this.backend.list());
    } catch {
      // the first event brings it
    }
  }

  dispose() {
    this.stop?.();
    this.stop = null;
  }

  private update(all: Download[]) {
    const now = performance.now();
    for (const d of all) {
      const before = this.last.get(d.id);
      if (d.state !== "running") {
        this.last.set(d.id, { got: d.got, at: now, bps: 0 });
      } else if (!before || d.got < before.got) {
        this.last.set(d.id, { got: d.got, at: now, bps: 0 });
      } else if (now - before.at >= 300) {
        const inst = ((d.got - before.got) / (now - before.at)) * 1000;
        this.last.set(d.id, { got: d.got, at: now, bps: before.bps ? before.bps * 0.6 + inst * 0.4 : inst });
      }
    }
    for (const id of this.last.keys()) if (!all.some((d) => d.id === id)) this.last.delete(id);
    this.list = all;
    this.subs.forEach((fn) => fn());
  }

  subscribe(fn: () => void): () => void {
    this.subs.add(fn);
    return () => this.subs.delete(fn);
  }

  /** Bytes per second (0 when not running or not known yet). */
  speed(d: Download): number {
    return this.last.get(d.id)?.bps ?? 0;
  }

  /** Whole downloads that are queued or running. */
  get active(): Download[] {
    return this.list.filter((d) => d.state === "queued" || d.state === "running");
  }

  /** The finished, still-present file of a title (a film, or the episode named by `url`). */
  doneFor(item: Item, url?: string): Download | undefined {
    const key = titleKey(item);
    return this.list.find((d) => d.state === "done" && d.exists && d.key === key && (url === undefined ? !d.label : d.url === url));
  }

  /** Any finished file of the title (the ✓ on its card). */
  hasAny(item: Item): boolean {
    const key = titleKey(item);
    return this.list.some((d) => d.state === "done" && d.exists && d.key === key);
  }

  /** The download (any state) of one file, by its server address. */
  byUrl(url: string): Download | undefined {
    return this.list.find((d) => d.url === url);
  }

  progressOf(item: Item, url?: string): number | null {
    const key = titleKey(item);
    const d = this.list.find((x) => x.key === key && (url === undefined ? !x.label : x.url === url) && x.state !== "done");
    return d && d.size > 0 ? d.got / d.size : d ? 0 : null;
  }

  async add(requests: DownloadRequest[]) {
    if (!this.backend) throw "Downloads work in the Zazabor Home app on a PC and in Zazabor TV on a phone.";
    await this.backend.add(requests);
    if (!isPhone) return;
    // DownloadManager has no events: look at once
    this.update(await this.backend.list());
  }

  async pause(d: Download) {
    await this.backend?.pause(d.id);
  }

  async resume(d: Download) {
    await this.backend?.resume(d.id);
  }

  async remove(d: Download, deleteFile = true) {
    await this.backend?.remove(d.id, deleteFile);
    if (isPhone && this.backend) this.update(await this.backend.list());
  }

  play(d: Download) {
    this.backend?.play(d);
  }
}

/** "12 MB of 1.2 GB · 4.5 MB/s · 3 min left" */
export function statusText(d: Download, speed: number): string {
  const sizes = d.size > 0 ? `${sizeText(d.got) || "0 MB"} of ${sizeText(d.size)}` : sizeText(d.got) || "Starting…";
  switch (d.state) {
    case "queued": return "Waiting for a free slot";
    case "paused": return `Paused · ${sizes}`;
    case "failed": return d.error || "Failed";
    case "done": return `${sizeText(d.size)}${d.exists ? "" : " · the file is gone"}`;
    default: return [sizes, speed > 0 ? `${sizeText(speed)}/s` : "", speed > 0 && d.size > d.got ? eta((d.size - d.got) / speed) : ""].filter(Boolean).join(" · ");
  }
}

const rowName = (d: Download) => (d.label ? d.label.replace(/\.[a-z0-9]{2,4}$/i, "") : "");

class DownloadRow {
  readonly el: HTMLElement;
  private fill = h("span", { class: "tv-dl-fill" });
  private status = h("span", { class: "tv-dl-status" });
  private actions = h("div", { class: "tv-dl-actions" });
  private lastState = "";

  constructor(private d: Download, private store: DownloadsStore, private onError: (e: unknown) => void) {
    const text = h("div", { class: "tv-dl-text" },
      h("span", { class: "tv-dl-title" }, d.title, d.year ? ` (${d.year})` : ""),
      rowName(d) ? h("span", { class: "tv-dl-file" }, rowName(d)) : null,
      h("span", { class: "tv-dl-bar", role: "progressbar" }, this.fill),
      this.status);
    this.el = h("div", { class: "tv-dl", role: "listitem" }, text, this.actions);
    this.update(d);
  }

  update(d: Download) {
    this.d = d;
    const pct = d.state === "done" ? 100 : d.size > 0 ? Math.min(100, (d.got / d.size) * 100) : 0;
    this.fill.style.width = `${pct}%`;
    this.el.dataset.state = d.state;
    this.el.querySelector(".tv-dl-bar")?.setAttribute("aria-valuenow", String(Math.round(pct)));
    this.status.textContent = statusText(d, this.store.speed(d));
    if (d.state !== this.lastState) {
      this.lastState = d.state;
      this.drawActions();
    }
  }

  private button(glyph: Parameters<typeof icon>[0], label: string, run: () => void | Promise<void>, danger = false) {
    const b = h("button", { class: `tv-dl-btn${danger ? " danger" : ""}`, type: "button", title: label, "aria-label": label, "data-tv-focus": "" }, icon(glyph, 17));
    b.addEventListener("click", () => void Promise.resolve(run()).catch((e) => this.onError(e)));
    return b;
  }

  private drawActions() {
    const { d, store } = this;
    const out: HTMLElement[] = [];
    if (d.state === "done") {
      if (d.exists) out.push(this.button("play", "Play", () => store.play(d)));
      if (d.exists && store.backend?.show) out.push(this.button("folder", "Show in Files", () => store.backend!.show!(d)));
      out.push(this.button("trash", "Delete the download", () => store.remove(d, true), true));
    } else {
      if (store.backend?.canPause) {
        if (d.state === "running" || d.state === "queued") out.push(this.button("pause", "Pause", () => store.pause(d)));
        else out.push(this.button("play", d.state === "failed" ? "Try again" : "Resume", () => store.resume(d)));
      }
      out.push(this.button("close", "Cancel and delete what was downloaded", () => store.remove(d, true), true));
    }
    this.actions.replaceChildren(...out);
  }
}

/** The Downloads page: progress bars, speed and time left, pause, resume, cancel, play, show in Files. */
export class DownloadsPanel {
  readonly el: HTMLElement;
  private list = h("div", { class: "tv-dl-list", role: "list" });
  private rows = new Map<string, DownloadRow>();
  private unsub: () => void;

  constructor(private store: DownloadsStore, folder: string, private onError: (e: unknown) => void) {
    this.el = h("div", { class: "tv-dl-panel" }, this.list, folder ? h("p", { class: "tv-dl-folder" }, icon("folder", 14), `Saved to ${folder}`) : null);
    this.paint();
    this.unsub = store.subscribe(() => this.paint());
  }

  dispose() {
    this.unsub();
  }

  private paint() {
    const all = this.store.list;
    const order = all.map((d) => d.id).join(",");
    const same = order === [...this.rows.keys()].join(",");
    if (!same) {
      const next = new Map<string, DownloadRow>();
      for (const d of all) next.set(d.id, this.rows.get(d.id) ?? new DownloadRow(d, this.store, this.onError));
      this.rows = next;
      this.list.replaceChildren(...(all.length ? [...next.values()].map((r) => r.el) : [h("div", { class: "tv-dl-empty" }, icon("download", 28), h("strong", null, "No downloads yet"), h("span", null, "Press Download on a title to keep it on this device."))]));
    }
    for (const d of all) this.rows.get(d.id)?.update(d);
  }
}
