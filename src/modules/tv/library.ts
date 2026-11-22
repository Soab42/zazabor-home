import { h } from "../../ui/dom";
import { icon } from "../../ui/icons";
import { tvApi, type Item } from "./api";
import { BrowsePage } from "./browse";
import { DownloadsPanel } from "./downloads";
import { card, notice, type TvCtx } from "./parts";
import { isPhone } from "./phone";

export type LibraryTab = "continue" | "fav" | "later" | "downloads" | "watched";

export const TABS: { id: LibraryTab; label: string; glyph: Parameters<typeof icon>[0] }[] = [
  // On the phone VLC plays and keeps its own place, so this lists what was played lately.
  { id: "continue", label: isPhone ? "Recently Played" : "Continue Watching", glyph: "play" },
  { id: "fav", label: "My List", glyph: "heart" },
  { id: "later", label: "Watch Later", glyph: "clock" },
  { id: "downloads", label: "Downloads", glyph: "download" },
  { id: "watched", label: "Watched", glyph: "check" },
];

/** What Library shows for "Continue Watching": titles in progress with how far, the latest first. */
export interface ContinueEntry {
  item: Item;
  fraction: number;
}

/**
 * One page for everything that is the user's own: what they are part-way through, their list,
 * what they saved for later, their downloads and what they have watched.
 */
export class LibraryPage {
  readonly el: HTMLElement;
  private tabs = h("div", { class: "tv-seasons tv-lib-tabs", role: "tablist" });
  private body = h("div", { class: "tv-lib-body" });
  private inner: { dispose?(): void } | null = null;
  private counts: Partial<Record<LibraryTab, number>> = {};
  private unsub: () => void;

  constructor(private ctx: TvCtx, private tab: LibraryTab, private select: (tab: LibraryTab) => void, private resume: () => Promise<ContinueEntry[]>, private folder: () => Promise<string>) {
    this.el = h("div", { class: "tv-library" }, h("header", { class: "tv-page-head" }, h("div", { class: "tv-page-title" }, h("h1", null, "Library"), h("p", null, "Yours: where you left off, your lists and your downloads"))), this.tabs, this.body);
    this.drawTabs();
    // the Downloads count follows the list as downloads start, finish and are cancelled
    this.unsub = ctx.downloads.subscribe(() => {
      if (this.counts.downloads === ctx.downloads.list.length) return;
      this.counts.downloads = ctx.downloads.list.length;
      this.drawTabs();
    });
  }

  dispose() {
    this.unsub();
    this.inner?.dispose?.();
  }

  private drawTabs() {
    this.tabs.replaceChildren(...TABS.map((t) => {
      const n = this.counts[t.id];
      const b = h("button", { class: `tv-season${t.id === this.tab ? " on" : ""}`, type: "button", role: "tab", "aria-selected": String(t.id === this.tab), "data-tv-focus": "" }, icon(t.glyph, 15), t.label, n ? h("span", { class: "tv-season-count" }, String(n)) : null);
      b.addEventListener("click", () => this.select(t.id));
      return b;
    }));
  }

  async load() {
    void this.loadCounts();
    this.inner?.dispose?.();
    this.inner = null;
    const tab = this.tab;
    if (tab === "downloads") {
      const panel = new DownloadsPanel(this.ctx.downloads, await this.folder().catch(() => ""), (err) => this.ctx.fail(err));
      this.inner = panel;
      this.body.replaceChildren(panel.el);
      return;
    }
    if (tab === "continue") {
      const entries = await this.resume().catch(() => []);
      if (tab !== this.tab) return;
      if (!entries.length) {
        this.body.replaceChildren((isPhone
          ? notice("play", "Nothing played yet", "Titles you play in VLC show up here, newest first. VLC remembers where you stopped.")
          : notice("play", "Nothing in progress", "Titles you start watching in Zazabor wait here, with a bar showing how far you got.")));
        return;
      }
      this.ctx.loader.reset();
      this.body.replaceChildren(h("div", { class: "tv-grid", role: "list" }, ...entries.map((e) => card(e.item, this.ctx, { progress: e.fraction }))));
      return;
    }
    const titles = { fav: "My List", later: "Watch Later", watched: "Watched" } as const;
    const empty: Record<"fav" | "later" | "watched", [Parameters<typeof icon>[0], string, string]> = {
      fav: ["heart", "Your list is empty", "Press the heart on a title (or F) to keep it here."],
      later: ["clock", "Nothing saved for later", "Press the clock on a title (or L) to save it for another evening."],
      watched: ["check", "Nothing marked watched yet", "Films you finish are marked automatically; you can mark anything from its page."],
    };
    const page = new BrowsePage(this.ctx, { title: titles[tab], base: { list: tab }, sortDefault: "marked", emptyNotice: empty[tab], embedded: true });
    this.inner = page;
    this.body.replaceChildren(page.el);
    await page.load();
  }

  private async loadCounts() {
    const lists = ["fav", "later", "watched"] as const;
    const totals = await Promise.all(lists.map((list) => tvApi.items({ list, limit: 1 }).then((p) => p.total, () => 0)));
    lists.forEach((l, i) => (this.counts[l] = totals[i]));
    this.counts.downloads = this.ctx.downloads.list.length;
    this.counts.continue = (await this.resume().catch(() => [])).length;
    this.drawTabs();
  }
}
