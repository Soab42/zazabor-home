import { h } from "../../ui/dom";
import { icon } from "../../ui/icons";
import { tvApi, type Filter, type Item, type Page } from "./api";
import { card, notice, type TvCtx, sideScroll } from "./parts";

/** The quality chips say 4K where the folders say 2160p. */
export const qualityLabel = (q: string) => (q === "2160p" ? "4K" : q);

/** Audio and source tags offered as chips (a title carries them in its folder name). */
const TAGS = ["Dual Audio", "NF", "AMZN", "DSNP", "BluRay"];

const HIDE_KEY = "zazabor.tv.hideWatched";
export const hideWatchedSetting = {
  get: () => {
    try {
      return localStorage.getItem(HIDE_KEY) === "1";
    } catch {
      return false;
    }
  },
  set: (on: boolean) => {
    try {
      localStorage.setItem(HIDE_KEY, on ? "1" : "0");
    } catch {
      // not remembered
    }
  },
};

export interface BrowseOptions {
  title: string;
  subtitle?: string;
  /** What the page always filters by (a section, a category, a kind). */
  base: Filter;
  /** Show a search box and start with it focused. */
  search?: boolean;
  /** The category is a ranked list (IMDb Top 250): offer sorting by rank. */
  ranked?: boolean;
  /** Shown between the header and the filters (a row of the section's categories). */
  extra?: HTMLElement;
  /** The sort to start with, when not the newest first. */
  sortDefault?: NonNullable<Filter["sort"]>;
  /** What an empty page says (glyph, title, text), instead of the catalog-is-filling-up line. */
  emptyNotice?: [Parameters<typeof icon>[0], string, string];
  /** Inside the Library page: no big title of its own. */
  embedded?: boolean;
}

const PAGE = 60;

/** A grid of posters with year and quality chips and a sort switch; also the search page. */
export class BrowsePage {
  readonly el: HTMLElement;
  private grid = h("div", { class: "tv-grid", role: "list" });
  private chips = h("div", { class: "tv-chips" });
  private count = h("span", { class: "tv-count" });
  private input: HTMLInputElement | null = null;
  private filter: Filter;
  private items: Item[] = [];
  private total = 0;
  private busy = false;
  private serial = 0;
  private sentinel = h("div", { class: "tv-sentinel" });
  private io: IntersectionObserver;
  private years: [number, number][] = [];
  private qualities: [string, number][] = [];
  private sortBar = h("div", { class: "tv-seg", role: "radiogroup", "aria-label": "Sort" });
  private timer = 0;

  constructor(private ctx: TvCtx, private options: BrowseOptions) {
    this.filter = { ...options.base, sort: options.sortDefault ?? (options.ranked ? "rank" : "new") };
    if (options.base.list !== "watched" && hideWatchedSetting.get()) this.filter.hideWatched = true;
    const head = h("header", { class: `tv-page-head${options.embedded ? " embedded" : ""}` }, h("div", { class: "tv-page-title" }, options.embedded ? null : h("h1", null, options.title), options.subtitle ? h("p", null, options.subtitle) : null, this.count));
    if (options.search) {
      this.input = h("input", { class: "tv-search-input", type: "search", placeholder: "Search movies and series", "aria-label": "Search", spellcheck: false, autocomplete: "off" });
      this.input.addEventListener("input", () => {
        clearTimeout(this.timer);
        this.timer = window.setTimeout(() => {
          this.filter.q = this.input!.value.trim() || undefined;
          this.filter.year = undefined;
          this.filter.quality = undefined;
          this.filter.tag = undefined;
          void this.load();
        }, 120);
      });
      head.append(h("div", { class: "tv-page-tools" }, h("label", { class: "tv-search glass" }, icon("search", 18), this.input)));
    }
    this.drawSort();
    sideScroll(this.chips);
    this.el = h("div", { class: "tv-browse" }, head, options.extra, h("div", { class: "tv-filters" }, this.chips, this.sortBar), this.grid, this.sentinel);
    this.io = new IntersectionObserver((entries) => {
      if (entries.some((e) => e.isIntersecting)) void this.more();
    }, { rootMargin: "700px" });
    this.io.observe(this.sentinel);
  }

  focusSearch() {
    this.input?.focus();
  }

  dispose() {
    this.io.disconnect();
    clearTimeout(this.timer);
  }

  private drawSort() {
    const modes: [NonNullable<Filter["sort"]>, string][] = [["new", "Newest added"], ["year", "Year"], ["az", "A–Z"]];
    if (this.options.ranked) modes.unshift(["rank", "Rank"]);
    if (this.options.base.list) modes.unshift(["marked", "Date added to list"]);
    this.sortBar.replaceChildren(...modes.map(([id, label]) => {
      const b = h("button", { class: `tv-seg-btn${this.filter.sort === id ? " on" : ""}`, type: "button", role: "radio", "aria-checked": String(this.filter.sort === id) }, label);
      b.addEventListener("click", () => {
        this.filter.sort = id;
        this.drawSort();
        void this.load(false);
      });
      return b;
    }));
  }

  private drawChips() {
    const chip = (label: string, on: boolean, pick: () => void) => {
      const b = h("button", { class: `tv-chip${on ? " on" : ""}`, type: "button", "aria-pressed": String(on) }, label);
      b.addEventListener("click", pick);
      return b;
    };
    const out: HTMLElement[] = [chip("All years", !this.filter.year, () => this.set({ year: undefined }))];
    for (const [year] of this.years) out.push(chip(String(year), this.filter.year === year, () => this.set({ year })));
    if (this.qualities.length > 1) {
      out.push(h("span", { class: "tv-chip-gap" }));
      for (const [quality] of this.qualities.slice(0, 4)) out.push(chip(qualityLabel(quality), this.filter.quality === quality, () => this.set({ quality: this.filter.quality === quality ? undefined : quality })));
    }
    out.push(h("span", { class: "tv-chip-gap" }));
    for (const tag of TAGS) out.push(chip(tag, this.filter.tag === tag, () => this.set({ tag: this.filter.tag === tag ? undefined : tag })));
    if (this.options.base.list !== "watched") {
      out.push(h("span", { class: "tv-chip-gap" }));
      out.push(chip("Hide watched", this.filter.hideWatched === true, () => {
        const on = !this.filter.hideWatched;
        hideWatchedSetting.set(on);
        this.set({ hideWatched: on || undefined });
      }));
    }
    this.chips.replaceChildren(...out);
  }

  private set(change: Partial<Filter>) {
    Object.assign(this.filter, change);
    void this.load(false);
  }

  /** (Re)reads the first page; `facets` also refreshes the year chips. */
  async load(facets = true) {
    const mine = ++this.serial;
    this.busy = true;
    let page: Page;
    try {
      page = await tvApi.items({ ...this.filter, limit: PAGE, offset: 0 }, facets);
    } catch {
      this.busy = false;
      return;
    }
    if (mine !== this.serial) return;
    this.busy = false;
    if (facets) {
      this.years = page.years;
      this.qualities = page.qualities;
    }
    this.drawChips();
    this.ctx.loader.reset();
    this.grid.replaceChildren();
    this.items = [];
    this.total = page.total;
    this.append(page.items);
    const narrowed = !!(this.filter.year || this.filter.quality || this.filter.tag || this.filter.hideWatched || this.filter.q);
    this.count.textContent = `${page.total.toLocaleString()} ${page.total === 1 ? "title" : "titles"}${narrowed ? " match" : ""}`;
    if (!page.items.length) {
      const [glyph, title, text] = this.options.emptyNotice && !narrowed ? this.options.emptyNotice : ["search" as const, this.filter.q ? "No matches" : narrowed ? "No titles match these filters" : "Nothing here yet", this.filter.q ? `Nothing in the catalog is called “${this.filter.q}”.` : narrowed ? "Try removing a filter." : "The catalog is still filling up, or this category has no titles."];
      this.grid.append(notice(glyph, title, text));
    }
  }

  private append(items: Item[]) {
    this.items.push(...items);
    this.grid.append(...items.map((item) => card(item, this.ctx)));
    // the sentinel only reports a change: look again, in case it is still in view
    this.io.unobserve(this.sentinel);
    this.io.observe(this.sentinel);
  }

  private async more() {
    if (this.busy || this.items.length >= this.total || !this.items.length) return;
    this.busy = true;
    const mine = this.serial;
    try {
      const page = await tvApi.items({ ...this.filter, limit: PAGE, offset: this.items.length });
      if (mine === this.serial) this.append(page.items);
    } catch {
      // the next scroll tries again
    } finally {
      this.busy = false;
    }
  }
}
