import type { AppModule, ModuleFactory, ShellContext } from "../../shell/module";
import { h } from "../../ui/dom";
import { icon } from "../../ui/icons";
import "../files/ops/ops.css";
import { showToast } from "../files/ops/toast";
import { errorText, isMock, onTv, titleKey, tvApi, type CatRow, type Download, type DownloadRequest, type Home, type Item, type List, type Progress, type Section } from "./api";
import { BrowsePage } from "./browse";
import { Detail } from "./detail";
import { desktopBackend, DownloadsStore, phoneBackend } from "./downloads";
import { Hero, homeRows, homeSkeleton } from "./home";
import { LibraryPage, type ContinueEntry, type LibraryTab } from "./library";
import { LivePage } from "./live";
import { Marks } from "./marks";
import { closeMenu, menuIsOpen, showMenu, type MenuEntry } from "./menu";
import { itemOf, notice, PosterLoader, type TvCtx } from "./parts";
import { fractionOf, finished, inProgress, pickEpisode, type Positions } from "./progress";
import { isPhone, pullToRefresh } from "./phone";
import "./tv.css";

type Route =
  | { id: "home" }
  | { id: "section"; section: "movies" | "tv" | "animation" | "indian" }
  | { id: "category"; url: string; label: string }
  | { id: "search" }
  | { id: "library"; tab: LibraryTab }
  | { id: "live" };

const NAV: { id: string; label: string; route: Route }[] = [
  { id: "home", label: "Home", route: { id: "home" } },
  { id: "movies", label: "Movies", route: { id: "section", section: "movies" } },
  { id: "tv", label: "TV Shows", route: { id: "section", section: "tv" } },
  { id: "animation", label: "Animation", route: { id: "section", section: "animation" } },
  { id: "indian", label: "Bangla & Indian", route: { id: "section", section: "indian" } },
  { id: "live", label: "Live TV", route: { id: "live" } },
  { id: "library", label: "Library", route: { id: "library", tab: "continue" } },
  { id: "search", label: "Search", route: { id: "search" } },
];

/** The phone's bottom tab bar: five tabs (the other sections are reached from Home's rows and the Movies page; Search is in the top bar). */
const PHONE_NAV: { id: string; label: string; glyph: Parameters<typeof icon>[0]; route: Route }[] = [
  { id: "home", label: "Home", glyph: "home", route: { id: "home" } },
  { id: "movies", label: "Movies", glyph: "film", route: { id: "section", section: "movies" } },
  { id: "tv", label: "Series", glyph: "playlist", route: { id: "section", section: "tv" } },
  { id: "live", label: "Live TV", glyph: "tv", route: { id: "live" } },
  { id: "library", label: "Library", glyph: "bookmark", route: { id: "library", tab: "continue" } },
];

/** What a player that shows a name calls an episode: the title and the file's name, no extension. */
const episodeTitle = (title: string, url: string) => {
  const file = decodeURIComponent(url.split("/").pop() ?? "").replace(/\.[a-z0-9]{2,4}$/i, "");
  return file ? `${title} - ${file}` : title;
};

const SECTION_TITLES: Record<string, [string, string]> = {
  movies: ["Movies", "Every film on the servers"],
  tv: ["TV Shows", "Series, web series and shows"],
  animation: ["Animation", "Animated films, anime and cartoons"],
  indian: ["Bangla & Indian", "Kolkata Bangla, Hindi and South Indian films"],
};

const navId = (r: Route) => (r.id === "section" ? (r.section === "tv" ? "tv" : r.section) : r.id === "category" ? "" : r.id);
const routeKey = (r: Route) => JSON.stringify(r);

class TvModule implements AppModule {
  readonly id = "tv" as const;
  readonly title = "TV";
  readonly icon = "tv" as const;
  readonly el: HTMLElement;

  private nav = h("nav", { class: "tv-nav glass", "aria-label": "TV" });
  private glider = h("span", { class: "tv-nav-glider", "aria-hidden": "true" });
  private pills = new Map<string, HTMLElement>();
  private back = h("button", { class: "tv-nav-back", type: "button", title: "Back (Esc)", "aria-label": "Back", hidden: true, "data-tv-focus": "" }, icon("chevronLeft", 18));
  private status = h("button", { class: "tv-status", type: "button", title: "Refresh the catalog", "data-tv-focus": "" });
  private scroll = h("div", { class: "tv-scroll" });
  /** Phone only: the bar on top (back, title, refresh status, search, settings) and the pull-to-refresh spinner. */
  private top = h("header", { class: "tv-top" });
  private pull = h("div", { class: "tv-ptr", "aria-hidden": "true" }, icon("refresh", 20));
  private pageEl = h("div", { class: "tv-page" });
  private layer = h("div", { class: "tv-layer" });
  private loader = new PosterLoader();
  private route: Route = { id: "home" };
  private history: Route[] = [];
  private hero: Hero | null = null;
  private page: { dispose?(): void } | null = null;
  private home: Home | null = null;
  private progress: Progress | null = null;
  private detail: Detail | null = null;
  private detailFocus: HTMLElement | null = null;
  private drawSerial = 0;
  private reloadTimer = 0;
  private refreshTimer = 0;
  private active = false;
  private started = false;
  private unlisten: Promise<() => void>[] = [];
  private ctx: TvCtx & { canVlc: boolean };
  private marks = new Marks();
  private downloads: DownloadsStore;
  /** The downloads capsule in the PC's nav, and the dot on the phone's Library tab. */
  private dlNav = h("button", { class: "tv-nav-dl", type: "button", hidden: true, title: "Downloads", "data-tv-focus": "" });

  constructor(private shell: ShellContext) {
    const backend = isPhone
      ? phoneBackend({ wifiOnly: () => this.shell.config.tv.wifiOnly })
      : desktopBackend({
        dir: () => this.shell.config.tv.downloadDir,
        play: (d) => this.playDownload(d),
        show: (d) => this.shell.open({ kind: "folder", path: d.path.replace(/[\\/][^\\/]*$/, "") }),
      });
    this.downloads = new DownloadsStore(backend);
    this.ctx = {
      shell,
      loader: this.loader,
      marks: this.marks,
      downloads: this.downloads,
      toggle: (item, list) => this.toggle(item, list),
      download: (item) => void this.download(item),
      downloadFiles: (item, files) => void this.downloadFiles(item, files),
      openMenu: (item, at) => this.openMenu(item, at),
      fractionOfUrl: (url) => this.fractionOfUrl(url),
      resumeAt: (url) => this.localPositions(url).reduce((at, p) => Math.max(at, p.at), 0),
      canVlc: false,
      openInVlc: (item, url) => this.openInVlc(item, url),
      share: (item, url) => this.share(item, url),
      fail: (err) => this.fail(err),
      openLibrary: (tab) => this.go({ id: "library", tab }),
      play: (item) => this.play(item),
      playUrl: (item, path, siblings) => this.playUrl(item, path, siblings),
      openDetail: (item, from) => this.openDetail(item, from),
      openCategory: (url, label) => this.go({ id: "category", url, label }),
    };
    const tabs: { id: string; label: string; glyph?: Parameters<typeof icon>[0]; route: Route }[] = isPhone ? PHONE_NAV : NAV;
    for (const n of tabs) {
      const glyph = n.glyph ?? (n.id === "search" ? "search" : null);
      const pill = h("button", { class: "tv-nav-pill", type: "button", "data-tv-focus": "" }, glyph ? icon(glyph, isPhone ? 22 : 15) : null, isPhone ? h("span", null, n.label) : n.label);
      pill.addEventListener("click", () => this.go(n.route, true));
      this.pills.set(n.id, pill);
    }
    this.dlNav.addEventListener("click", () => this.go({ id: "library", tab: "downloads" }));
    this.downloads.subscribe(() => this.paintDownloads());
    this.back.addEventListener("click", () => this.goBack());
    this.status.addEventListener("click", () => void this.refresh(true));
    if (isPhone) {
      const search = h("button", { class: "tv-top-btn", type: "button", "aria-label": "Search", onclick: () => this.go({ id: "search" }, true) }, icon("search", 22));
      const settings = h("button", { class: "tv-top-btn", type: "button", "aria-label": "Settings", onclick: () => this.shell.openSettings("tv") }, icon("settings", 22));
      this.top.append(this.back, h("h1", { class: "tv-top-title" }, "Zazabor TV"), this.status, search, settings);
      this.nav.append(...this.pills.values());
      pullToRefresh(this.scroll, this.pull, () => this.route.id === "home" && !this.detail, () => void this.refresh(true));
    } else {
      this.nav.append(this.glider, this.back, ...this.pills.values(), h("span", { class: "tv-nav-sep" }), this.dlNav, this.status);
    }
    this.scroll.append(this.pageEl);
    this.el = h("div", { class: "tvx" }, this.scroll, ...(isPhone ? [this.top, this.pull] : []), this.nav, this.layer);
    this.el.addEventListener("focusin", (e) => this.rove(e.target as HTMLElement));
    let frame = 0;
    this.scroll.addEventListener("scroll", () => {
      if (frame || !this.hero) return;
      frame = requestAnimationFrame(() => {
        frame = 0;
        this.hero?.parallax(this.scroll.scrollTop);
      });
    }, { passive: true });
    // phone: the top bar is see-through over Home's picture and gets a body once the page scrolls under it
    if (isPhone) this.scroll.addEventListener("scroll", () => this.el.classList.toggle("scrolled", this.scroll.scrollTop > 60), { passive: true });
    new ResizeObserver(() => this.moveGlider()).observe(this.nav);
    this.paintNav();
    this.paintStatus();
  }

  // ---- life

  activate() {
    this.active = true;
    if (!this.started) {
      this.started = true;
      this.unlisten.push(
        onTv("tv-progress", (p) => this.onProgress(p)),
        onTv("tv-changed", () => this.onChanged()),
      );
      void tvApi.state().then((p) => this.onProgress(p), () => undefined);
      void this.downloads.start();
      if (!isPhone) void tvApi.vlcAvailable().then((ok) => (this.ctx.canVlc = ok), () => undefined);
      void this.refresh(false);
      this.refreshTimer = window.setInterval(() => void this.refresh(false), 30 * 60 * 1000);
      void this.show(false);
    } else if (this.route.id === "home") {
      this.scheduleReload(0);
    }
    this.hero?.start();
    if (!this.detail && !isPhone) window.setTimeout(() => this.focusFirst(), 60);
  }

  deactivate() {
    this.active = false;
    this.hero?.stop();
  }

  dispose() {
    clearInterval(this.refreshTimer);
    clearTimeout(this.reloadTimer);
    this.hero?.stop();
    this.page?.dispose?.();
    this.downloads.dispose();
    for (const u of this.unlisten) void u.then((fn) => fn());
  }

  configChanged() {
    void this.refresh(false);
    this.home = null;
    void this.show(true);
  }

  get modal() {
    return this.detail !== null;
  }

  // ---- the catalog

  private async refresh(force: boolean) {
    try {
      this.onProgress(await tvApi.start(this.shell.config.tv, force));
    } catch (err) {
      this.onProgress({ running: false, done: 0, total: 0, items: 0, reachable: false, error: errorText(err), lastCrawl: 0 });
    }
  }

  private onProgress(p: Progress) {
    const wasRunning = this.progress?.running === true;
    this.progress = p;
    this.paintStatus();
    if (wasRunning && !p.running) this.scheduleReload(300);
    else if (this.route.id === "home" && (!this.home || this.home.total === 0) && p.items > 0) this.scheduleReload(1200);
  }

  private onChanged() {
    if (this.route.id === "home" && (!this.home || this.home.total === 0)) this.scheduleReload(1200);
  }

  private scheduleReload(ms: number) {
    clearTimeout(this.reloadTimer);
    this.reloadTimer = window.setTimeout(() => {
      if (this.route.id === "home" || this.route.id === "section" || this.route.id === "category") void this.show(true);
    }, ms);
  }

  private paintStatus() {
    const p = this.progress;
    let text = "";
    if (p?.running) text = p.total ? `Updating ${p.done}/${p.total}` : "Looking…";
    else if (p?.reachable === false) text = "Offline";
    else if (p && p.items) text = `${p.items.toLocaleString()} titles`;
    this.status.classList.toggle("spin", p?.running === true);
    this.status.classList.toggle("bad", !p?.running && p?.reachable === false);
    this.status.replaceChildren(icon(p?.reachable === false && !p.running ? "bolt" : "refresh", 15), h("span", null, text || "Refresh"));
    this.status.title = p?.reachable === false ? p.error : "Refresh the catalog";
  }

  // ---- routes

  private go(route: Route, reset = false) {
    if (this.detail) this.closeDetail(false);
    if (routeKey(route) === routeKey(this.route)) {
      this.scroll.scrollTo({ top: 0, behavior: "smooth" });
      return;
    }
    if (reset) this.history = [];
    else this.history.push(this.route);
    this.route = route;
    void this.show(false);
  }

  /** Library's tabs replace each other (Back leaves the Library, not the last tab). */
  private selectTab(tab: LibraryTab) {
    this.route = { id: "library", tab };
    void this.show(false);
  }

  private goBack() {
    const prev = this.history.pop();
    if (!prev) return;
    this.route = prev;
    void this.show(false);
  }

  private paintNav() {
    const current = navId(this.route);
    for (const [id, pill] of this.pills) {
      pill.classList.toggle("on", id === current);
      if (id === current) pill.setAttribute("aria-current", "page");
      else pill.removeAttribute("aria-current");
    }
    this.back.hidden = this.history.length === 0;
    if (isPhone) this.el.dataset.route = this.route.id;
    requestAnimationFrame(() => this.moveGlider());
  }

  /** The glass highlight slides to the active pill. */
  private moveGlider() {
    const pill = this.pills.get(navId(this.route));
    this.glider.classList.toggle("none", !pill || !pill.offsetWidth);
    if (!pill || !pill.offsetWidth) return;
    this.glider.style.width = `${pill.offsetWidth}px`;
    this.glider.style.transform = `translate3d(${pill.offsetLeft}px, 0, 0)`;
  }

  /** Draws the current route; `keep` keeps the scroll position (a quiet redraw as the catalog grows). */
  private async show(keep: boolean) {
    const serial = ++this.drawSerial;
    const top = keep ? this.scroll.scrollTop : 0;
    this.hero?.stop();
    this.hero = null;
    this.page?.dispose?.();
    this.page = null;
    this.loader.reset();
    this.paintNav();
    const route = this.route;
    const target = this.pageEl;
    let entered = keep;
    const put = (...children: Node[]) => {
      target.replaceChildren(...children);
      this.scroll.scrollTop = top;
      if (!entered) {
        // a page comes in with a short fade and rise
        entered = true;
        target.classList.remove("enter");
        void target.offsetWidth;
        target.classList.add("enter");
      }
    };
    if (route.id === "home") {
      if (!this.home) put(homeSkeleton());
      let home: Home;
      try {
        home = await tvApi.home();
      } catch (err) {
        if (serial === this.drawSerial) put(notice("tv", "TV isn't available", errorText(err)));
        return;
      }
      if (serial !== this.drawSerial) return;
      this.home = home;
      this.loader.reset();
      if (!home.total) return put(this.emptyHome());
      const resume = await this.resumeEntries().catch(() => []);
      if (serial !== this.drawSerial) return;
      this.loader.reset();
      const { hero, rows } = homeRows(home, this.ctx, resume);
      this.hero = hero;
      const banner = this.banner();
      put(...[hero?.el ?? h("div", { class: "tv-hero-gap" }), banner, ...rows, h("div", { class: "tv-end" })].filter((n): n is HTMLElement => !!n));
      if (this.active) hero?.start();
    } else if (route.id === "library") {
      const library = new LibraryPage(this.ctx, route.tab, (tab) => this.selectTab(tab), () => this.resumeEntries(), () => (isPhone ? Promise.resolve("") : tvApi.dlDir(this.shell.config.tv.downloadDir)));
      this.page = library;
      put(library.el);
      await library.load();
    } else if (route.id === "live") {
      const live = new LivePage(this.ctx);
      this.page = live;
      put(live.el);
      await live.load();
    } else {
      const page = await this.browsePage(route);
      if (serial !== this.drawSerial) return;
      this.page = page;
      put(page.el);
      await page.load();
      if (route.id === "search" && !keep) page.focusSearch();
    }
    if (serial === this.drawSerial && !keep && this.active && route.id !== "search") window.setTimeout(() => this.focusFirst(), 60);
  }

  private async browsePage(route: Route): Promise<BrowsePage> {
    if (route.id === "search") return new BrowsePage(this.ctx, { title: "Search", subtitle: "Titles across every server", base: {}, search: true });
    if (route.id === "category") {
      return new BrowsePage(this.ctx, { title: route.label, base: { categoryUrl: route.url }, ranked: /imdb/i.test(route.label) });
    }
    const section = (route as { section: Section }).section;
    const [title, subtitle] = SECTION_TITLES[section];
    let cats: CatRow[] = this.home?.categories ?? [];
    if (!cats.length) cats = (await tvApi.home().catch(() => null))?.categories ?? [];
    const mine = cats.filter((c) => (section === "tv" ? c.kind === "series" : section === "movies" ? c.kind === "movie" : c.section === section));
    const extra = mine.length > 1
      ? h("div", { class: "tv-cats" }, ...mine.map((c) => {
        const b = h("button", { class: "tv-cat glass", type: "button", "data-tv-focus": "" }, c.label, h("span", null, c.count.toLocaleString()));
        b.addEventListener("click", () => this.go({ id: "category", url: c.url, label: c.label }));
        return b;
      }))
      : undefined;
    const base = section === "movies" ? { kind: "movie" as const } : section === "tv" ? { kind: "series" as const } : { section };
    return new BrowsePage(this.ctx, { title, subtitle, base, extra });
  }

  private banner(): HTMLElement | null {
    const p = this.progress;
    if (p?.reachable !== false || p.running) return null;
    return h("div", { class: "tv-banner glass" }, icon("bolt", 16), h("span", null, `${p.error || "Can't reach DhakaFlix."} Showing the saved catalog; playing needs the network.`));
  }

  private emptyHome(): HTMLElement {
    const p = this.progress;
    if (p?.running || !p) {
      return h("div", { class: "tv-empty-wrap" }, notice("refresh", "Reading DhakaFlix", p?.total ? `${p.done} of ${p.total} categories read, ${p.items.toLocaleString()} titles found so far.` : "Finding the movie and series categories…"));
    }
    const retry = h("button", { class: "tv-pill primary", type: "button", "data-tv-focus": "" }, icon("refresh", 16), "Try again");
    retry.addEventListener("click", () => void this.refresh(true));
    const settings = h("button", { class: "tv-pill glass", type: "button", "data-tv-focus": "" }, icon("settings", 16), "Server settings");
    settings.addEventListener("click", () => this.shell.openSettings("tv"));
    const off = p.reachable === false;
    return h("div", { class: "tv-empty-wrap" }, notice(off ? "bolt" : "tv", off ? "Can't reach the movie server" : "Nothing found yet", off ? p.error || "Can't reach DhakaFlix — are you on the ISP's network?" : "The servers answered but no movie folders turned up. Check the addresses and categories in Settings.", retry, settings));
  }

  // ---- playing

  private fail(err: unknown) {
    showToast(errorText(err));
  }

  private mark(item: Item) {
    item.played = Date.now();
    void tvApi.played(item.id).catch(() => undefined);
  }

  // ---- the user's lists

  private positions(): Positions {
    return this.shell.config.video?.positions ?? {};
  }

  /** The player's resume points for a file: under its address, and under its downloaded copy's path. */
  private localPositions(url: string) {
    const out = [this.positions()[url]];
    const d = this.downloads.byUrl(url);
    if (d?.state === "done") out.push(this.positions()[d.path]);
    return out.filter((p): p is NonNullable<typeof p> => !!p);
  }

  private fractionOfUrl(url: string): number {
    return Math.max(0, ...this.localPositions(url).map((p) => fractionOf(p)));
  }

  /** Sets a list's mark on the title (the page and the backend), without a message. */
  private applyMark(item: Item, list: List, on: boolean) {
    const before = this.marks.has(item, list);
    this.marks.set(item, list, on);
    void tvApi.mark(item.id, list, on).catch((err) => {
      this.marks.set(item, list, before);
      this.fail(err);
    });
    if (this.active && (this.route.id === "home" || this.route.id === "library") && list !== "watched") this.scheduleReload(500);
    else if (this.route.id === "library" && !this.detail) this.scheduleReload(500);
  }

  private toggle(item: Item, list: List) {
    const on = !this.marks.has(item, list);
    this.applyMark(item, list, on);
    const names: Record<List, [string, string]> = {
      fav: ["Added to My List", "Removed from My List"],
      later: ["Saved to Watch Later", "Removed from Watch Later"],
      watched: ["Marked as watched", "Marked as not watched"],
    };
    showToast(`${names[list][on ? 0 : 1]}: ${item.title}`, { label: "Undo", run: () => this.applyMark(item, list, !on) }, 4500);
  }

  /** What the player remembers, as titles: in progress with how far, and films that were finished get marked watched. */
  private async resumeEntries(): Promise<ContinueEntry[]> {
    const positions = this.positions();
    const progress = inProgress(positions);
    const done = finished(positions);
    const urls = [...progress.map((p) => p.url), ...done.map((p) => p.url)];
    if (!urls.length) return [];
    const byUrl = new Map((await tvApi.byUrls(urls)).map((m) => [m.url, m.item]));
    for (const f of done) {
      const item = byUrl.get(f.url);
      if (item && item.kind === "movie" && !this.marks.has(item, "watched")) this.applyMark(item, "watched", true);
    }
    const seen = new Set<string>();
    const out: ContinueEntry[] = [];
    for (const p of progress) {
      const item = byUrl.get(p.url);
      if (!item || seen.has(titleKey(item))) continue;
      seen.add(titleKey(item));
      if (item.kind === "movie" && this.marks.has(item, "watched")) continue;
      out.push({ item, fraction: p.fraction });
    }
    return out.slice(0, 30);
  }

  // ---- downloads

  private request(item: Item, url: string, label: string, size: number): DownloadRequest {
    return { key: titleKey(item), itemId: item.id, title: item.title, year: item.year, label, url, size };
  }

  private async download(item: Item) {
    if (!this.downloads.canDownload) return this.fail("Downloads work in the Zazabor Home app on a PC and in Zazabor TV on a phone.");
    if (item.kind === "series") return this.openDetail(item);
    try {
      const full = item.videoUrl ? item : Object.assign(item, await tvApi.resolve(item.id));
      if (!full.videoUrl) throw "No video file was found in that folder.";
      if (this.downloads.byUrl(full.videoUrl)) return showToast(this.downloads.doneFor(full) ? `${item.title} is already downloaded` : `${item.title} is already in your downloads`, { label: "Downloads", run: () => this.go({ id: "library", tab: "downloads" }) });
      await this.downloads.add([this.request(full, full.videoUrl, "", full.size)]);
      showToast(`Downloading ${item.title}`, { label: "Downloads", run: () => this.go({ id: "library", tab: "downloads" }) }, 4500);
    } catch (err) {
      this.fail(err);
    }
  }

  private async downloadFiles(item: Item, files: { url: string; name: string; size: number }[]) {
    const fresh = files.filter((f) => !this.downloads.byUrl(f.url));
    if (!fresh.length) return showToast(files.length === 1 ? "That episode is already in your downloads" : "Those episodes are already in your downloads");
    try {
      await this.downloads.add(fresh.map((f) => this.request(item, f.url, f.name, f.size)));
      showToast(fresh.length === 1 ? `Downloading ${item.title}: 1 episode` : `Downloading ${item.title}: ${fresh.length} episodes`, { label: "Downloads", run: () => this.go({ id: "library", tab: "downloads" }) }, 4500);
    } catch (err) {
      this.fail(err);
    }
  }

  private playDownload(d: Download) {
    const title = d.label ? `${d.title} - ${d.label.replace(/\.[a-z0-9]{2,4}$/i, "")}` : d.title;
    if (d.itemId) void tvApi.played(d.itemId).catch(() => undefined);
    this.shell.open({ kind: "video", path: d.path, title });
  }

  private paintDownloads() {
    const active = this.downloads.active;
    const got = active.reduce((n, d) => n + d.got, 0);
    const size = active.reduce((n, d) => n + d.size, 0);
    const pct = size > 0 ? ` · ${Math.floor((got / size) * 100)}%` : "";
    this.dlNav.hidden = active.length === 0;
    this.dlNav.replaceChildren(icon("download", 14), h("span", null, `${active.length}${pct}`));
    this.dlNav.title = `${active.length} download${active.length === 1 ? "" : "s"} running`;
    const dot = this.pills.get("library");
    dot?.classList.toggle("busy", active.length > 0);
  }

  // ---- the actions menu, VLC and sharing

  private openMenu(item: Item, at: { x: number; y: number } | HTMLElement) {
    const entries: MenuEntry[] = [
      { label: item.kind === "series" ? "Watch" : "Play", glyph: "play", run: () => this.play(item) },
      { label: "Details", glyph: "info", run: () => this.openDetail(item) },
      { label: "My List", glyph: "heart", on: this.marks.has(item, "fav"), run: () => this.toggle(item, "fav") },
      { label: "Watch Later", glyph: "clock", on: this.marks.has(item, "later"), run: () => this.toggle(item, "later") },
      { label: "Watched", glyph: "check", on: this.marks.has(item, "watched"), run: () => this.toggle(item, "watched") },
    ];
    if (this.downloads.canDownload) {
      const saved = item.kind === "movie" ? this.downloads.doneFor(item) : undefined;
      entries.push(saved
        ? { label: "Downloaded: play offline copy", glyph: "download", run: () => this.downloads.play(saved) }
        : { label: item.kind === "series" ? "Download…" : "Download", glyph: "download", run: () => void this.download(item) });
    }
    if (item.kind === "movie") {
      if (isPhone) entries.push({ label: "Share link", glyph: "send", run: () => this.share(item, item.videoUrl || item.folderUrl) });
      else {
        entries.push({ label: "Copy link", glyph: "link", run: () => void navigator.clipboard.writeText(item.videoUrl || item.folderUrl).then(() => showToast("Link copied"), () => showToast("Couldn't copy the link")) });
        if (this.ctx.canVlc && item.videoUrl) entries.push({ label: "Open in VLC", glyph: "external", run: () => this.openInVlc(item, item.videoUrl) });
      }
    }
    showMenu(item.title, entries, at);
  }

  private openInVlc(item: Item, url: string) {
    this.mark(item);
    void tvApi.openVlc(url).catch((err) => this.fail(err));
  }

  private share(item: Item, url: string) {
    const text = `${item.title}${item.year ? ` (${item.year})` : ""}\n${url}`;
    const android = (window as unknown as { ZazaborPlayer?: { share?(text: string): void } }).ZazaborPlayer;
    if (android?.share) return android.share(text);
    void navigator.clipboard.writeText(url).then(() => showToast("Link copied"), () => showToast("Couldn't copy the link"));
  }

  private play(item: Item) {
    if (item.kind === "series") {
      void tvApi.episodes(item.id).then((seasons) => {
        const pick = pickEpisode(seasons, (u) => this.fractionOfUrl(u), (u) => this.ctx.resumeAt(u));
        if (!pick) return this.openDetail(item);
        this.playUrl(item, pick.url, seasons.flatMap((s) => s.episodes.map((e) => e.url)));
      }, (err) => this.fail(err));
      return;
    }
    const saved = this.downloads.doneFor(item);
    if (saved) return this.downloads.play(saved);
    if (item.videoUrl) return this.playUrl(item, item.videoUrl);
    tvApi.resolve(item.id).then((done) => {
      Object.assign(item, done);
      if (done.videoUrl) this.playUrl(item, done.videoUrl);
      else this.fail("No video file was found in that folder.");
    }, (err) => this.fail(err));
  }

  private playUrl(item: Item, path: string, siblings?: string[]) {
    this.mark(item);
    // a downloaded episode plays from its file
    const own = (url: string) => {
      const d = this.downloads.byUrl(url);
      return d?.state === "done" && d.exists ? d : undefined;
    };
    const saved = own(path);
    if (saved && isPhone) return this.downloads.play(saved);
    const local = (url: string) => (isPhone ? url : own(url)?.path ?? url);
    const files = siblings && siblings.length > 1 ? siblings.map(local) : undefined;
    const names = siblings && siblings.length > 1 ? siblings.map((url) => episodeTitle(item.title, url)) : undefined;
    this.shell.open({ kind: "video", path: local(path), siblings: files, title: names ? episodeTitle(item.title, path) : item.title, titles: names });
  }

  // ---- the detail page

  private openDetail(item: Item, from?: HTMLElement) {
    if (this.detail) this.closeDetail(false);
    this.detailFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const detail = new Detail(item, this.ctx, () => this.closeDetail(true));
    this.detail = detail;
    this.layer.replaceChildren(detail.el);
    this.layer.classList.add("open");
    this.scroll.inert = true;
    this.nav.inert = true;
    this.top.inert = true;
    // on the phone the page slides up as a sheet (CSS) instead of zooming out of the card
    detail.open(isPhone ? null : from?.getBoundingClientRect() ?? null, this.layer.getBoundingClientRect());
    if (!isPhone) window.setTimeout(() => detail.focusPrimary(), 40);
  }

  private closeDetail(restore: boolean) {
    if (!this.detail) return;
    this.detail.dispose();
    this.detail = null;
    this.layer.classList.remove("open");
    this.layer.replaceChildren();
    this.scroll.inert = false;
    this.nav.inert = false;
    this.top.inert = false;
    if (restore && this.detailFocus?.isConnected) this.detailFocus.focus({ preventScroll: true });
    this.detailFocus = null;
  }

  // ---- keys and focus

  /** One element of the page is the tab stop; the arrows move between the rest. */
  private rove(target: HTMLElement) {
    const el = target.closest<HTMLElement>("[data-tv-focus]");
    if (!el || el.tagName === "BUTTON" || el.tagName === "INPUT") return;
    this.el.querySelectorAll<HTMLElement>('[data-tv-focus][tabindex="0"]').forEach((n) => (n.tabIndex = -1));
    el.tabIndex = 0;
    el.scrollIntoView({ block: "nearest", inline: "nearest", behavior: "smooth" });
  }

  private focusables(root: HTMLElement): HTMLElement[] {
    return [...root.querySelectorAll<HTMLElement>("[data-tv-focus]")].filter((n) => n.offsetParent !== null && !n.hasAttribute("disabled") && !n.closest("[inert]"));
  }

  private focusFirst() {
    if (this.detail || !this.active || isPhone) return;
    const cur = document.activeElement;
    if (cur instanceof HTMLElement && this.el.contains(cur) && cur !== document.body && cur.matches("[data-tv-focus],input")) return;
    const first = this.pageEl.querySelector<HTMLElement>(".tv-slide.on .tv-pill, .tv-card, .tv-chan");
    first?.focus({ preventScroll: true });
  }

  private move(dir: "left" | "right" | "up" | "down"): boolean {
    const root = this.detail ? this.detail.el : this.el;
    const nodes = this.focusables(root);
    const cur = document.activeElement instanceof HTMLElement ? document.activeElement.closest<HTMLElement>("[data-tv-focus]") : null;
    if (!cur || !nodes.includes(cur)) {
      nodes.find((n) => n.getBoundingClientRect().top > 60)?.focus();
      return true;
    }
    const a = cur.getBoundingClientRect();
    const ax = a.left + a.width / 2;
    const ay = a.top + a.height / 2;
    let best: HTMLElement | null = null;
    let bestScore = Infinity;
    for (const n of nodes) {
      if (n === cur) continue;
      const r = n.getBoundingClientRect();
      const dx = r.left + r.width / 2 - ax;
      const dy = r.top + r.height / 2 - ay;
      let primary: number;
      let ortho: number;
      if (dir === "left" || dir === "right") {
        primary = dir === "right" ? dx : -dx;
        ortho = Math.abs(dy);
        if (primary <= 2 || ortho > Math.max(a.height, r.height) * 0.6) continue;
      } else {
        primary = dir === "down" ? dy : -dy;
        ortho = Math.abs(dx);
        if (primary <= 2 || primary < Math.min(a.height, r.height) * 0.35) continue;
      }
      const score = primary + ortho * (dir === "left" || dir === "right" ? 4 : 2.2);
      if (score < bestScore) {
        bestScore = score;
        best = n;
      }
    }
    if (best) {
      best.focus({ preventScroll: true });
      best.scrollIntoView({ block: dir === "up" || dir === "down" ? "center" : "nearest", inline: "nearest", behavior: "smooth" });
      return true;
    }
    if ((dir === "left" || dir === "right") && this.hero && cur.closest(".tv-hero")) {
      this.hero.step(dir === "right" ? 1 : -1);
      return true;
    }
    return dir === "left" || dir === "right";
  }

  handleKey(e: KeyboardEvent): boolean {
    if (e.ctrlKey || e.metaKey || e.altKey) return false;
    const typing = e.target instanceof HTMLInputElement;
    if (e.key === "Escape") {
      if (menuIsOpen()) return closeMenu(), true;
      if (this.detail) return this.closeDetail(true), true;
      if (typing && (e.target as HTMLInputElement).value) {
        (e.target as HTMLInputElement).value = "";
        (e.target as HTMLInputElement).dispatchEvent(new Event("input"));
        return true;
      }
      if (this.history.length) return this.goBack(), true;
      return false;
    }
    const dirs: Record<string, "left" | "right" | "up" | "down"> = { ArrowLeft: "left", ArrowRight: "right", ArrowUp: "up", ArrowDown: "down" };
    const dir = dirs[e.key];
    if (dir) {
      if (typing && dir !== "down" && dir !== "up") return false;
      return this.move(dir);
    }
    const focused = document.activeElement as HTMLElement | null;
    if (e.key === "Enter" && focused?.matches("[data-tv-focus]") && focused.tagName !== "BUTTON") {
      focused.click();
      return true;
    }
    if ((e.key === "p" || e.key === "P") && !typing && focused?.classList.contains("tv-card")) {
      focused.dispatchEvent(new CustomEvent("tv-play"));
      return true;
    }
    if (!isPhone && !typing && !menuIsOpen()) {
      const key = e.key.toLowerCase();
      const target = key === "f" || key === "l" || key === "d" ? this.detail?.current ?? itemOf(focused) : undefined;
      if (target) {
        if (key === "f") this.toggle(target, "fav");
        else if (key === "l") this.toggle(target, "later");
        else void this.download(target);
        return true;
      }
    }
    if (e.key === "/" && !typing) {
      this.go({ id: "search" }, true);
      return true;
    }
    return false;
  }
}

export const createTvModule: ModuleFactory = (shell) => {
  if (isMock) document.documentElement.dataset.tvMock = "1";
  return new TvModule(shell);
};
