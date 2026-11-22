import { h } from "../../ui/dom";
import { icon } from "../../ui/icons";
import { showToast } from "../files/ops/toast";
import { errorText, picUrl, sizeText, tvApi, type Episode, type Item, type List, type Season } from "./api";
import { badges, hued, meta, shelf, type TvCtx } from "./parts";
import { showMenu, type MenuEntry } from "./menu";
import { pickEpisode, WATCHED_AT, type Pick } from "./progress";
import { isPhone } from "./phone";

const decodeSafe = (text: string) => {
  try {
    return decodeURIComponent(text);
  } catch {
    return text;
  }
};

/** Where a title sits on the server, readable: "English Movies (1080p) › (2025) 1080p". */
const folderPath = (url: string) => decodeSafe(url.replace(/^https?:\/\/[^/]+\/[^/]+\//, "")).replace(/\/$/, "").split("/").slice(0, -1).join(" › ");

const episodeName = (name: string) => name.replace(/\.[a-z0-9]{2,4}$/i, "");

/** A title's page over the catalog: backdrop from its poster, the facts, Play, and for a series its seasons. */
export class Detail {
  readonly el: HTMLElement;
  private item: Item;
  private body = h("div", { class: "tv-detail-body" });
  private seasons: Season[] = [];
  private season = 0;
  private episodes = h("div", { class: "tv-episodes" });
  private art: HTMLElement;
  private posterBox: HTMLElement;
  private more = h("div", { class: "tv-detail-more" });
  /** The title's other qualities (its 720p and 1080p folders), read once. */
  private variants: Item[] = [];
  private apply: (next: Item) => void = () => undefined;
  /** Where to carry on in a series: the episode being watched, the next one, or the first. */
  private pick: Pick | null = null;
  private stops: (() => void)[] = [];
  /** The buttons whose look follows the lists and downloads. */
  private live: (() => void)[] = [];

  /** The title (in the quality shown) this page is about. */
  get current(): Item {
    return this.item;
  }

  dispose() {
    this.stops.forEach((stop) => stop());
    this.stops = [];
  }

  constructor(item: Item, private ctx: TvCtx, private close: () => void) {
    this.item = item;
    const bg = h("img", { class: "tv-detail-bg", alt: "", draggable: false });
    const poster = h("img", { class: "tv-detail-poster", alt: "", draggable: false });
    const art = hued(h("div", { class: "tv-detail-art" }, bg, h("div", { class: "tv-detail-shade" })), item.title);
    const posterBox = hued(h("div", { class: "tv-detail-card" }, h("span", { class: "tv-fallback-title" }, item.title), poster), item.title);
    const apply = (next: Item) => {
      Object.assign(this.item, next);
      if (next.poster && bg.dataset.src !== next.poster) {
        bg.dataset.src = next.poster;
        bg.src = poster.src = picUrl(next.poster);
        art.classList.add("tv-has-img");
        posterBox.classList.add("tv-has-img");
      }
      this.drawFacts();
    };
    this.apply = apply;
    this.art = art;
    this.posterBox = posterBox;
    const x = h("button", { class: "tv-close glass", type: "button", "aria-label": "Close", title: "Close (Esc)", "data-tv-focus": "" }, icon("close", 18));
    x.addEventListener("click", () => this.close());
    this.el = h("div", { class: "tv-detail", role: "dialog", "aria-modal": "true", "aria-label": item.title }, art, x, h("div", { class: "tv-detail-scroll" }, h("div", { class: "tv-detail-main" }, posterBox, this.body), this.more));
    this.el.addEventListener("click", (e) => {
      if (e.target === this.el || (e.target as HTMLElement).classList.contains("tv-detail-scroll")) this.close();
    });
    this.drawFacts();
    const repaint = () => this.live.forEach((paint) => paint());
    let seen = "";
    const sync = () => {
      repaint();
      // episode rows show their download's state: draw them again when that changes (not on every byte)
      const now = ctx.downloads.list.filter((d) => d.key === this.keyOf()).map((d) => `${d.id}${d.state}`).join();
      if (now !== seen && this.seasons.length) {
        seen = now;
        this.drawSeasons();
      }
    };
    this.stops.push(ctx.marks.subscribe(repaint), ctx.downloads.subscribe(sync));
    if (item.poster) apply(item);
    else if (!item.noPoster) void tvApi.resolve(item.id).then(apply, () => undefined);
    void this.loadMore();
    if ((item.qualities?.length ?? 0) > 1) void tvApi.variants(item.id).then((all) => ((this.variants = all), this.drawFacts(), this.sizeVariants()), () => undefined);
    if (item.kind === "series") void this.loadSeasons();
    else if (!item.videoUrl) void tvApi.resolve(item.id).then(apply, () => undefined);
  }

  /** More from the same category, below the facts. */
  private async loadMore() {
    try {
      const page = await tvApi.items({ categoryUrl: this.item.categoryUrl, sort: "new", limit: 18 });
      const others = page.items.filter((i) => i.id !== this.item.id).slice(0, 16);
      if (others.length >= 3) this.more.replaceChildren(shelf(`More in ${this.item.category}`, others, this.ctx));
    } catch {
      // the page is complete without it
    }
  }

  /**
   * Zooms out of the card that was clicked: the backdrop grows from the card's rectangle (it is
   * blurred, so stretching it doesn't show) and the poster flies from the card to its place.
   * Transform and opacity only.
   */
  open(from: DOMRect | null, area: DOMRect) {
    if (!from || matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    const ease = "cubic-bezier(0.2, 0.8, 0.3, 1)";
    const sx = from.width / area.width;
    const sy = from.height / area.height;
    this.art.animate([
      { transformOrigin: "0 0", transform: `translate3d(${from.left - area.left}px, ${from.top - area.top}px, 0) scale(${sx}, ${sy})`, opacity: 0.2 },
      { transformOrigin: "0 0", transform: "none", opacity: 1 },
    ], { duration: 460, easing: ease });
    const to = this.posterBox.getBoundingClientRect();
    if (to.width) {
      const k = from.width / to.width;
      this.posterBox.style.animation = "none";
      this.posterBox.animate([
        { transformOrigin: "0 0", transform: `translate3d(${from.left - to.left}px, ${from.top - to.top}px, 0) scale(${k})` },
        { transformOrigin: "0 0", transform: "none" },
      ], { duration: 460, easing: ease });
    }
    this.el.querySelector(".tv-detail-scroll")?.animate([{ opacity: 0 }, { opacity: 1 }], { duration: 380, easing: "ease-out" });
  }

  focusPrimary() {
    (this.el.querySelector(".tv-pill.primary") as HTMLElement | null)?.focus({ preventScroll: true });
  }

  /** Each quality's file size comes from looking in its folder; done in the background. */
  private sizeVariants() {
    for (const v of this.variants) {
      if (v.kind !== "movie" || v.size) continue;
      void tvApi.resolve(v.id).then((done) => {
        Object.assign(v, done);
        if (v.id === this.item.id) Object.assign(this.item, done);
        this.drawFacts();
      }, () => undefined);
    }
  }

  private toggle(glyph: Parameters<typeof icon>[0], label: string, list: List, key: string): HTMLButtonElement {
    const b = h("button", { class: "tv-pill glass tv-toggle", type: "button", "data-tv-focus": "", title: `${label} (${key})` }, icon(glyph, 17), h("span", { class: "tv-pill-label" }, label));
    b.addEventListener("click", () => this.ctx.toggle(this.item, list));
    this.live.push(() => {
      const on = this.ctx.marks.has(this.item, list);
      b.classList.toggle("on", on);
      b.setAttribute("aria-pressed", String(on));
    });
    return b;
  }

  private iconButton(glyph: Parameters<typeof icon>[0], label: string, run: () => void): HTMLButtonElement {
    const b = h("button", { class: "tv-pill glass icon-only", type: "button", title: label, "aria-label": label, "data-tv-focus": "" }, icon(glyph, 17));
    b.addEventListener("click", run);
    return b;
  }

  /** What there is to play for a copy link, VLC or share: the film's file, or the episode being picked. */
  private link(): string {
    if (this.item.kind === "series") return this.pick?.url ?? this.seasons[0]?.episodes[0]?.url ?? this.item.folderUrl;
    return this.item.videoUrl || this.item.folderUrl;
  }

  private downloadButton(): HTMLButtonElement {
    const b = h("button", { class: "tv-pill glass tv-dl-btn-main", type: "button", "data-tv-focus": "", title: "Download (D)" });
    const series = this.item.kind === "series";
    const paint = () => {
      const store = this.ctx.downloads;
      const d = series ? undefined : store.byUrl(this.item.videoUrl) ?? store.list.find((x) => x.key === this.keyOf() && !x.label);
      let glyph: Parameters<typeof icon>[0] = "download";
      let label = "Download";
      if (series) {
        const mine = store.list.filter((x) => x.key === this.keyOf());
        const running = mine.filter((x) => x.state === "running" || x.state === "queued").length;
        if (running) label = `Downloading ${running}`;
        else if (mine.some((x) => x.state === "done")) (glyph = "check", label = `${mine.filter((x) => x.state === "done").length} downloaded`);
        label += " ▾";
      } else if (d?.state === "done" && d.exists) {
        glyph = "check";
        label = "Downloaded";
      } else if (d && d.state !== "done" && d.state !== "failed") {
        label = `${d.state === "paused" ? "Paused" : "Downloading"} ${d.size > 0 ? Math.floor((d.got / d.size) * 100) : 0}%`;
      } else if (d?.state === "failed") {
        label = "Retry download";
      } else if (this.item.size) {
        label = `Download · ${sizeText(this.item.size)}`;
      }
      b.replaceChildren(icon(glyph, 17), label);
      b.classList.toggle("on", glyph === "check");
    };
    paint();
    this.live.push(paint);
    b.addEventListener("click", () => this.downloadMenu(b));
    return b;
  }

  private keyOf() {
    return `${this.item.title.toLowerCase()}|${this.item.year ?? 0}|${this.item.kind}`;
  }

  private downloadMenu(anchor: HTMLElement) {
    const store = this.ctx.downloads;
    if (this.item.kind === "series") {
      const entries: MenuEntry[] = [];
      const files = (eps: Episode[]) => eps.map((e) => ({ url: e.url, name: e.name, size: e.size }));
      const total = (eps: Episode[]) => eps.reduce((n, e) => n + e.size, 0);
      const describe = (eps: Episode[]) => `${eps.length} episode${eps.length === 1 ? "" : "s"}${total(eps) ? ` · ${sizeText(total(eps))}` : ""}`;
      if (this.pick) {
        const ep = this.seasons.flatMap((s) => s.episodes).find((e) => e.url === this.pick!.url);
        if (ep) entries.push({ label: `${this.pick.how === "next" ? "Next episode" : this.pick.how === "continue" ? "This episode" : "First episode"}${this.pick.episode ? ` (${this.pick.episode})` : ""}`, glyph: "download", hint: sizeText(ep.size), run: () => this.ctx.downloadFiles(this.item, files([ep])) });
      }
      const season = this.seasons[this.season];
      if (season) entries.push({ label: season.name, glyph: "download", hint: describe(season.episodes), run: () => this.ctx.downloadFiles(this.item, files(season.episodes)) });
      const all = this.seasons.flatMap((s) => s.episodes);
      if (this.seasons.length > 1) entries.push({ label: "All seasons", glyph: "download", hint: describe(all), run: () => this.ctx.downloadFiles(this.item, files(all)) });
      if (!entries.length) entries.push({ label: "Episodes are still loading…", run: () => undefined });
      if (store.list.some((d) => d.key === this.keyOf())) entries.push({ label: "See downloads", glyph: "folder", run: () => this.ctx.openLibrary("downloads") });
      return showMenu("Download", entries, anchor);
    }
    const d = store.byUrl(this.item.videoUrl) ?? store.list.find((x) => x.key === this.keyOf() && !x.label);
    if (!d || d.state === "failed") return this.ctx.download(this.item);
    const entries: MenuEntry[] = [];
    if (d.state === "done" && d.exists) {
      entries.push({ label: "Play the downloaded copy", glyph: "play", run: () => store.play(d) });
      if (store.backend?.show) entries.push({ label: "Show in Files", glyph: "folder", run: () => store.backend!.show!(d) });
    }
    entries.push({ label: "See downloads", glyph: "download", run: () => this.ctx.openLibrary("downloads") });
    entries.push({ label: d.state === "done" ? "Delete the download" : "Cancel the download", glyph: "trash", danger: true, run: () => void store.remove(d, true).catch((e) => this.ctx.fail(e)) });
    showMenu(this.item.title, entries, anchor);
  }

  private drawFacts() {
    const item = this.item;
    this.live = [];
    const play = h("button", { class: "tv-pill primary", type: "button", "data-tv-focus": "" }, icon("play", 18), item.kind === "series" ? this.playLabel() : "Play");
    play.addEventListener("click", () => this.playMain());
    const folder = this.iconButton("external", "Open folder in browser", () => this.ctx.shell.openExternally(item.folderUrl));
    const copy = this.iconButton("link", "Copy link", () => {
      void navigator.clipboard.writeText(this.link()).then(() => showToast("Link copied"), () => showToast("Couldn't copy the link"));
    });
    const watched = this.iconButton("check", "Mark as watched", () => this.ctx.toggle(item, "watched"));
    this.live.push(() => {
      const on = this.ctx.marks.has(this.item, "watched");
      watched.classList.toggle("on", on);
      watched.title = watched.ariaLabel = on ? "Watched: mark as not watched" : "Mark as watched";
    });
    const extra: HTMLElement[] = [];
    if (isPhone) extra.push(this.iconButton("send", "Share", () => this.ctx.share(item, this.link())));
    else if (this.ctx.canVlc) extra.push(this.iconButton("tv", "Open in VLC", () => this.ctx.openInVlc(item, this.link())));
    const actions = h("div", { class: "tv-detail-actions" }, play, this.toggle("heart", "My List", "fav", "F"), this.toggle("clock", "Watch Later", "later", "L"), ...(this.ctx.downloads.canDownload ? [this.downloadButton()] : []), watched, copy, ...extra, folder);
    const facts = h("dl", { class: "tv-facts" },
      fact("Category", item.category),
      fact("Year", item.year ? String(item.year) : ""),
      fact("Size", sizeText(item.size)),
      fact("Server folder", folderPath(item.folderUrl)),
      fact("Added", item.added ? new Date(item.added).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" }) : ""));
    const keep = this.body.querySelector(".tv-episodes-wrap");
    this.body.replaceChildren(
      h("span", { class: "tv-kicker" }, item.category),
      h("h1", { class: "tv-detail-title" }, item.title),
      h("p", { class: "tv-slide-meta" }, meta({ ...item, qualities: [item.quality] }, true)),
      ...(this.variants.length > 1 ? [this.qualitySwitch()!] : []),
      badges(item, 7),
      actions,
      facts);
    this.live.forEach((paint) => paint());
    if (keep) this.body.append(keep);
  }

  /** 1080p / 720p chips (with each file's size) when the title has several folders; picking one swaps the whole page to it. */
  private qualitySwitch(): HTMLElement | null {
    if (this.variants.length < 2) return null;
    const row = h("div", { class: "tv-quality", role: "radiogroup", "aria-label": "Quality" }, h("span", { class: "tv-quality-label" }, "Quality"));
    const seg = h("div", { class: "tv-seg" });
    for (const v of this.variants) {
      const on = v.id === this.item.id;
      const b = h("button", { class: `tv-seg-btn${on ? " on" : ""}`, type: "button", role: "radio", "aria-checked": String(on), "data-tv-focus": "" }, v.quality === "2160p" ? "4K" : v.quality || "Original", v.size ? h("small", { class: "tv-seg-size" }, sizeText(v.size)) : null);
      b.addEventListener("click", () => this.pickQuality(v));
      seg.append(b);
    }
    row.append(seg);
    return row;
  }

  private pickQuality(v: Item) {
    if (v.id === this.item.id) return;
    this.item = v;
    this.seasons = [];
    this.season = 0;
    this.pick = null;
    this.body.querySelector(".tv-episodes-wrap")?.remove();
    this.drawFacts();
    if (v.poster) this.apply(v);
    else if (!v.noPoster) void tvApi.resolve(v.id).then(this.apply, () => undefined);
    if (v.kind === "series") void this.loadSeasons();
    else if (!v.videoUrl) void tvApi.resolve(v.id).then(this.apply, () => undefined);
  }

  private playLabel() {
    const p = this.pick;
    if (!p) return "Watch";
    const code = p.season && p.episode ? `S${String(p.season).padStart(2, "0")}E${String(p.episode).padStart(2, "0")}` : p.episode ? `Episode ${p.episode}` : "";
    return p.how === "continue" ? `Continue ${code}`.trim() : p.how === "next" ? `Next: ${code}`.trim() : "Play first episode";
  }

  private playMain() {
    if (this.item.kind === "series") {
      const url = this.pick?.url ?? this.seasons[0]?.episodes[0]?.url;
      if (url) this.playEpisode(url);
      return;
    }
    this.ctx.play(this.item);
  }

  private async loadSeasons() {
    const wrap = h("section", { class: "tv-episodes-wrap" }, h("div", { class: "tv-episodes-loading tv-shimmer-line" }), h("div", { class: "tv-episodes-loading tv-shimmer-line" }), h("div", { class: "tv-episodes-loading tv-shimmer-line" }));
    this.body.append(wrap);
    try {
      this.seasons = await tvApi.episodes(this.item.id);
    } catch (err) {
      wrap.replaceChildren(h("p", { class: "tv-error" }, `Couldn't read the episodes: ${errorText(err)}`));
      return;
    }
    if (!this.seasons.length) {
      wrap.replaceChildren(h("p", { class: "tv-error" }, "No episodes were found in this folder."));
      return;
    }
    this.pick = pickEpisode(this.seasons, (u) => this.ctx.fractionOfUrl(u), (u) => this.ctx.resumeAt(u));
    this.drawFacts();
    this.drawSeasons();
  }

  private drawSeasons() {
    const tabs = h("div", { class: "tv-seasons", role: "tablist" }, ...this.seasons.map((s, i) => {
      const tab = h("button", { class: `tv-season${i === this.season ? " on" : ""}`, type: "button", role: "tab", "aria-selected": String(i === this.season), "data-tv-focus": "" }, s.name, h("span", { class: "tv-season-count" }, String(s.episodes.length)));
      tab.addEventListener("click", () => {
        this.season = i;
        this.drawSeasons();
      });
      return tab;
    }));
    const season = this.seasons[this.season];
    const bytes = season.episodes.reduce((n, e) => n + e.size, 0);
    const summary = h("div", { class: "tv-season-summary" },
      h("span", null, `${season.episodes.length} episode${season.episodes.length === 1 ? "" : "s"}${bytes ? ` · ${sizeText(bytes)} in all` : ""}`));
    if (this.ctx.downloads.canDownload) {
      const all = h("button", { class: "tv-link", type: "button", "data-tv-focus": "" }, icon("download", 14), "Download season");
      all.addEventListener("click", () => this.ctx.downloadFiles(this.item, season.episodes.map((e) => ({ url: e.url, name: e.name, size: e.size }))));
      summary.append(all);
    }
    this.episodes = h("div", { class: "tv-episodes", role: "list" }, ...season.episodes.map((ep, i) => {
      const play = h("button", { class: "tv-ep-play", type: "button", tabindex: -1, "aria-label": `Play episode ${ep.episode ?? i + 1}` }, icon("play", 15));
      const fraction = this.ctx.fractionOfUrl(ep.url);
      const done = fraction >= WATCHED_AT;
      const have = this.ctx.downloads.byUrl(ep.url);
      const save = this.ctx.downloads.canDownload ? h("button", { class: `tv-ep-save${have ? " on" : ""}`, type: "button", tabindex: -1, title: have?.state === "done" ? "Downloaded" : have ? "Downloading" : "Download this episode", "aria-label": "Download this episode" }, icon(have?.state === "done" ? "check" : "download", 15)) : null;
      save?.addEventListener("click", (e) => {
        e.stopPropagation();
        if (!have) this.ctx.downloadFiles(this.item, [{ url: ep.url, name: ep.name, size: ep.size }]);
        else this.ctx.openLibrary("downloads");
      });
      const row = h("div", { class: `tv-episode${done ? " watched" : ""}${this.pick?.url === ep.url ? " up" : ""}`, role: "listitem", tabindex: -1, "data-tv-focus": "" },
        h("span", { class: "tv-ep-num" }, done ? icon("check", 14) : String(ep.episode ?? i + 1)),
        h("span", { class: "tv-ep-text" }, h("span", { class: "tv-ep-title" }, `Episode ${ep.episode ?? i + 1}`), h("span", { class: "tv-ep-file" }, episodeName(ep.name)),
          fraction > 0 && !done ? h("span", { class: "tv-progress inline", role: "progressbar", "aria-valuenow": String(Math.round(fraction * 100)) }, h("span", { style: { width: `${Math.round(fraction * 100)}%` } })) : null),
        h("span", { class: "tv-ep-size" }, sizeText(ep.size)), ...(save ? [save] : []), play);
      row.addEventListener("click", () => this.playEpisode(ep.url));
      row.addEventListener("keydown", (e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          e.stopPropagation();
          this.playEpisode(ep.url);
        }
      });
      return row;
    }));
    const wrap = this.body.querySelector(".tv-episodes-wrap") ?? h("section", { class: "tv-episodes-wrap" });
    wrap.replaceChildren(tabs, summary, this.episodes);
    if (!wrap.isConnected) this.body.append(wrap);
  }

  /** Plays an episode; next and previous walk through every episode in order. */
  private playEpisode(url: string) {
    const all = this.seasons.flatMap((s) => s.episodes.map((e) => e.url));
    this.ctx.playUrl(this.item, url, all);
  }
}

function fact(label: string, value: string): HTMLElement {
  return h("div", { class: "tv-fact", hidden: !value }, h("dt", null, label), h("dd", null, value));
}
