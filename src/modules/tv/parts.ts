import type { ShellContext } from "../../shell/module";
import { h } from "../../ui/dom";
import { icon } from "../../ui/icons";
import { isPhone } from "./phone";
import { picUrl, sizeText, tvApi, type Item, type List } from "./api";
import type { DownloadsStore } from "./downloads";
import type { Marks } from "./marks";

/** What every page of the section can ask for. */
export interface TvCtx {
  readonly shell: ShellContext;
  /** Plays a title in the Video player (looks inside its folder first when need be). */
  play(item: Item): void;
  /** Plays one file of a title (an episode); `siblings` is what next and previous walk through. */
  playUrl(item: Item, path: string, siblings?: string[]): void;
  /** `from` is the card (or hero) the page opens out of. */
  openDetail(item: Item, from?: HTMLElement): void;
  openCategory(url: string, label: string): void;
  readonly loader: PosterLoader;
  /** My List, Watch Later and Watched, as the page knows them. */
  readonly marks: Marks;
  readonly downloads: DownloadsStore;
  /** Puts a title on a list or takes it off (with a toast to undo it). */
  toggle(item: Item, list: List): void;
  /** Downloads a film; for a series opens its page (to pick an episode, a season or all). */
  download(item: Item): void;
  /** The actions menu of a title: right-click on the PC, long-press on the phone. */
  openMenu(item: Item, at: { x: number; y: number } | HTMLElement): void;
  /** Downloads some files of a series (episodes) and says so. */
  downloadFiles(item: Item, files: { url: string; name: string; size: number }[]): void;
  /** How far the player got in a file (0 to 1), or 0. */
  fractionOfUrl(url: string): number;
  /** When the player last had that file (ms), or 0. */
  resumeAt(url: string): number;
  /** VLC is installed on this PC (the PC only). */
  readonly canVlc: boolean;
  openInVlc(item: Item, url: string): void;
  /** The phone's share sheet (the PC copies the link). */
  share(item: Item, url: string): void;
  /** Shows an error as a toast. */
  fail(err: unknown): void;
  openLibrary(tab: "continue" | "fav" | "later" | "downloads" | "watched"): void;
}

const WEEK = 7 * 86400000;
/** A title the servers got in the last week. */
export const isNew = (item: Item) => item.added > 0 && Date.now() - item.added < WEEK;

const items = new WeakMap<Element, Item>();
/** The title a card (or something in it) stands for. */
export const itemOf = (el: Element | null): Item | undefined => {
  const card = el?.closest(".tv-item, .tv-card");
  const found = card && (items.get(card) ?? items.get(card.querySelector(".tv-card") ?? card));
  return found || undefined;
};

export const hueOf = (text: string) => {
  let n = 0;
  for (let i = 0; i < text.length; i++) n = (n * 31 + text.charCodeAt(i)) % 360;
  return n;
};

/** "2026 · Series · 1080p / 720p"; `size` adds the file size (known only once the folder was read). */
/** Sets the `--h` hue the poster-less gradients use (`h()` can't set custom properties through `style`). */
export const hued = <T extends HTMLElement>(el: T, text: string): T => {
  el.style.setProperty("--h", String(hueOf(text)));
  return el;
};

export const meta = (item: Item, size = false) => [item.year ? String(item.year) : "", item.kind === "series" ? "Series" : "", item.qualities?.length ? item.qualities.join(" / ") : item.quality, size ? sizeText(item.size) : ""].filter(Boolean).join(" · ");

/** Quality and the notable tags as small pills. */
export function badges(item: Item, max = 4): HTMLElement {
  const all = [item.quality, ...item.tags].filter(Boolean).slice(0, max);
  return h("span", { class: "tv-badges" }, ...all.map((t) => h("span", { class: "tv-badge" }, t)));
}

/**
 * Loads posters as cards come into view: a title whose folder hasn't been looked into yet is
 * resolved by the backend (a few at a time), then every card showing it is updated.
 */
export class PosterLoader {
  private io: IntersectionObserver;
  private views = new Map<string, Set<(item: Item) => void>>();
  private jobs = new WeakMap<Element, Item>();
  private queue: Item[] = [];
  private queued = new Set<string>();
  private active = 0;

  constructor() {
    this.io = new IntersectionObserver((entries) => {
      for (const entry of entries) {
        const item = this.jobs.get(entry.target);
        if (!entry.isIntersecting || !item) continue;
        this.io.unobserve(entry.target);
        this.jobs.delete(entry.target);
        if (!item.poster && !item.noPoster && !this.queued.has(item.id)) {
          this.queued.add(item.id);
          this.queue.push(item);
          this.pump();
        }
      }
    }, { rootMargin: "400px 600px" });
  }

  /** Shows `item`'s poster in `apply`'s card once it is in view. */
  watch(el: Element, item: Item, apply: (item: Item) => void) {
    let set = this.views.get(item.id);
    if (!set) this.views.set(item.id, (set = new Set()));
    set.add(apply);
    this.jobs.set(el, item);
    this.io.observe(el);
  }

  /** Forgets every card (the page is drawn again). */
  reset() {
    this.io.disconnect();
    this.views.clear();
    this.queue = [];
    this.queued.clear();
  }

  /** Items changed behind our back (a new catalog): show what's there now. */
  refresh(items: Item[]) {
    for (const item of items) this.views.get(item.id)?.forEach((apply) => apply(item));
  }

  private pump() {
    while (this.active < 6 && this.queue.length) {
      const item = this.queue.shift()!;
      this.active++;
      tvApi.resolve(item.id).then((done) => this.refresh([done]), () => undefined).finally(() => {
        this.active--;
        this.queued.delete(item.id);
        this.pump();
      });
    }
  }
}

export interface CardOptions {
  /** A bar along the poster's foot: how far through it the viewer is (0 to 1). */
  progress?: number;
  /** A number shown big behind the poster (the Top 10 row). */
  rank?: number;
  /** A wide (landscape) card, for the Recently Played row. */
  wide?: boolean;
}

/** One title: a poster that lifts on hover or focus, with a caption below. */
export function card(item: Item, ctx: TvCtx, options: CardOptions = {}): HTMLElement {
  const img = h("img", { class: "tv-img", alt: "", draggable: false, decoding: "async", loading: "lazy" });
  img.sizes = "(min-width: 1700px) 240px, 190px";
  const fallback = hued(h("span", { class: "tv-fallback" }, h("span", { class: "tv-fallback-title" }, item.title), item.year ? h("span", { class: "tv-fallback-year" }, String(item.year)) : null), item.title);
  const poster = h("div", { class: "tv-poster" }, fallback, img);
  const apply = (next: Item) => {
    if (next.poster && img.dataset.src !== next.poster) {
      img.dataset.src = next.poster;
      img.onload = () => poster.classList.add("tv-has-img");
      // the 480 px copy is sharp enough for a card even on a HiDPI screen; the original backs it up for wide cards
      if (next.posterThumb) img.srcset = `${picUrl(next.posterThumb)} 480w, ${picUrl(next.poster)} 800w`;
      img.src = picUrl(next.posterThumb ?? next.poster);
    }
    item = Object.assign(item, next);
  };
  const playButton = h("button", { class: "tv-mini-play", type: "button", tabindex: -1, title: "Play", "aria-label": `Play ${item.title}` }, icon("play", 16));
  playButton.addEventListener("click", (e) => {
    e.stopPropagation();
    ctx.play(item);
  });
  const quickButton = (glyph: Parameters<typeof icon>[0], label: string, run: () => void) => {
    const b = h("button", { class: "tv-quick-btn", type: "button", tabindex: -1, title: label, "aria-label": label }, icon(glyph, 16));
    b.addEventListener("click", (e) => {
      e.stopPropagation();
      run();
    });
    return b;
  };
  const favButton = quickButton("heart", "My List (F)", () => ctx.toggle(item, "fav"));
  const laterButton = quickButton("clock", "Watch Later (L)", () => ctx.toggle(item, "later"));
  const quick = h("span", { class: "tv-quick" }, favButton, laterButton);
  if (ctx.downloads.canDownload) quick.append(quickButton("download", "Download (D)", () => ctx.download(item)));
  const flags = h("span", { class: "tv-flags" });
  poster.append(flags, quick);
  if (options.progress !== undefined) poster.append(h("span", { class: "tv-progress", role: "progressbar", "aria-valuenow": String(Math.round(options.progress * 100)) }, h("span", { style: { width: `${Math.round(options.progress * 100)}%` } })));
  const info = h("span", { class: "tv-hover" }, h("span", { class: "tv-hover-title" }, item.title), h("span", { class: "tv-hover-row" }, playButton, badges(item, 3)));
  const sheen = h("span", { class: "tv-sheen" });
  const face = h("div", { class: `tv-card${options.wide ? " wide" : ""}`, tabindex: -1, role: "button", "data-tv-focus": "", "aria-label": `${item.title}${item.year ? `, ${item.year}` : ""}` }, poster, sheen, info);
  face.addEventListener("click", () => ctx.openDetail(item, face));
  face.addEventListener("tv-play", () => ctx.play(item));
  face.addEventListener("pointermove", (e) => {
    const r = face.getBoundingClientRect();
    face.style.setProperty("--mx", `${((e.clientX - r.left) / r.width) * 100}%`);
    face.style.setProperty("--my", `${((e.clientY - r.top) / r.height) * 100}%`);
  });
  const caption = h("span", { class: "tv-caption", title: item.title }, h("span", { class: "tv-caption-title" }, item.title), h("span", { class: "tv-caption-meta" }, meta(item)));
  face.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    if (!suppressed) ctx.openMenu(item, { x: e.clientX, y: e.clientY });
  });
  // the phone: a long press opens the menu (and the tap that ends it must not open the page)
  let pressTimer = 0;
  let pressAt = { x: 0, y: 0 };
  let suppressed = false;
  if (isPhone) {
    face.addEventListener("pointerdown", (e) => {
      suppressed = false;
      pressAt = { x: e.clientX, y: e.clientY };
      clearTimeout(pressTimer);
      pressTimer = window.setTimeout(() => {
        suppressed = true;
        navigator.vibrate?.(12);
        ctx.openMenu(item, pressAt);
      }, 450);
    });
    const cancel = () => clearTimeout(pressTimer);
    face.addEventListener("pointermove", (e) => {
      if (Math.hypot(e.clientX - pressAt.x, e.clientY - pressAt.y) > 10) cancel();
    });
    face.addEventListener("pointerup", cancel);
    face.addEventListener("pointercancel", cancel);
    face.addEventListener("click", (e) => {
      if (suppressed) {
        e.stopImmediatePropagation();
        e.preventDefault();
        suppressed = false;
      }
    }, true);
  }
  const root = h("div", { class: `tv-item${options.rank ? " ranked" : ""}`, role: "listitem" }, options.rank ? h("span", { class: "tv-rank", "aria-hidden": "true" }, String(options.rank)) : null, h("div", { class: "tv-stack" }, face, caption));
  if (item.poster) apply(item);
  ctx.loader.watch(root, item, apply);
  items.set(root, item);
  items.set(face, item);
  // the badges and toggles follow the lists and downloads, wherever they change
  let attached = false;
  let drawn = "";
  const paint = () => {
    const fav = ctx.marks.has(item, "fav");
    const later = ctx.marks.has(item, "later");
    const watched = ctx.marks.has(item, "watched");
    const saved = ctx.downloads.hasAny(item);
    // downloads report every 400 ms: touch the page only when something on this card changed
    const now = `${fav}${later}${watched}${saved}`;
    if (now === drawn) return;
    drawn = now;
    root.classList.toggle("is-fav", fav);
    root.classList.toggle("is-later", later);
    root.classList.toggle("is-watched", watched);
    favButton.classList.toggle("on", fav);
    laterButton.classList.toggle("on", later);
    favButton.setAttribute("aria-pressed", String(fav));
    laterButton.setAttribute("aria-pressed", String(later));
    const parts: HTMLElement[] = [];
    if (isNew(item) && !watched) parts.push(h("span", { class: "tv-flag new" }, "NEW"));
    if (watched) parts.push(h("span", { class: "tv-flag watched", title: "Watched" }, icon("check", 12)));
    if (saved) parts.push(h("span", { class: "tv-flag saved", title: "Downloaded" }, icon("download", 12)));
    flags.replaceChildren(...parts);
  };
  paint();
  const stopMarks = ctx.marks.subscribe(() => {
    if (!root.isConnected) {
      if (attached) stopAll();
      return;
    }
    attached = true;
    paint();
  });
  const stopDownloads = ctx.downloads.subscribe(() => {
    if (root.isConnected) paint();
  });
  function stopAll() {
    stopMarks();
    stopDownloads();
  }
  return root;
}

/** Cards of a shelf while the catalog loads. */
export function skeletonShelf(title: string, count = 8): HTMLElement {
  return h("section", { class: "tv-shelf skeleton", "aria-busy": "true" }, h("header", { class: "tv-shelf-head" }, h("h2", null, title)), h("div", { class: "tv-row" }, ...Array.from({ length: count }, () => h("div", { class: "tv-item" }, h("div", { class: "tv-stack" }, h("div", { class: "tv-card tv-shimmer" }), h("span", { class: "tv-caption tv-shimmer-line" }))))));
}

/** Rows fade and rise into place, card by card, the first time they scroll into view. */
const reveal = new IntersectionObserver((entries) => {
  for (const entry of entries) {
    if (!entry.isIntersecting) continue;
    entry.target.classList.add("in");
    reveal.unobserve(entry.target);
  }
}, { rootMargin: "0px 0px 25% 0px", threshold: 0 });

export interface ShelfOptions extends CardOptions {
  /** Per card: how far through the title the viewer is. */
  progressOf?: (item: Item) => number | undefined;
  numbered?: boolean;
  onMore?: () => void;
  moreLabel?: string;
  subtitle?: string;
}

/** A titled row of cards that scrolls sideways: with arrow buttons, the wheel with Shift, or a drag. */
export function shelf(title: string, items: Item[], ctx: TvCtx, options: ShelfOptions = {}): HTMLElement {
  const row = h("div", { class: "tv-row", role: "list" });
  items.forEach((item, i) => {
    const c = card(item, ctx, { ...options, rank: options.numbered ? i + 1 : undefined, progress: options.progressOf?.(item) ?? options.progress });
    c.style.setProperty("--i", String(Math.min(i, 9)));
    row.append(c);
  });
  const left = h("button", { class: "tv-arrow left", type: "button", tabindex: -1, "aria-label": "Scroll left" }, icon("chevronLeft", 20));
  const right = h("button", { class: "tv-arrow right", type: "button", tabindex: -1, "aria-label": "Scroll right" }, icon("chevronRight", 20));
  const wrap = h("div", { class: "tv-row-wrap" }, left, row, right);
  const sync = () => {
    left.classList.toggle("show", row.scrollLeft > 8);
    right.classList.toggle("show", row.scrollLeft + row.clientWidth < row.scrollWidth - 8);
  };
  const page = (dir: number) => row.scrollBy({ left: dir * row.clientWidth * 0.86, behavior: "smooth" });
  left.addEventListener("click", () => page(-1));
  right.addEventListener("click", () => page(1));
  row.addEventListener("scroll", sync, { passive: true });
  new ResizeObserver(sync).observe(row);
  // drag with the mouse
  let startX = 0, startLeft = 0, moved = false;
  const drag = (e: PointerEvent) => {
    if (Math.abs(e.clientX - startX) > 6 && !moved) {
      moved = true;
      // snapping and hover lifts fight the finger: off while dragging
      row.classList.add("dragging");
    }
    if (moved) row.scrollLeft = startLeft - (e.clientX - startX);
  };
  const drop = () => {
    row.classList.remove("dragging");
    window.removeEventListener("pointermove", drag);
    window.removeEventListener("pointerup", drop);
  };
  row.addEventListener("pointerdown", (e) => {
    if (e.pointerType !== "mouse" || e.button !== 0) return;
    moved = false;
    startX = e.clientX;
    startLeft = row.scrollLeft;
    window.addEventListener("pointermove", drag);
    window.addEventListener("pointerup", drop);
  });
  row.addEventListener("click", (e) => {
    if (moved) {
      e.stopPropagation();
      e.preventDefault();
      moved = false;
    }
  }, true);
  const head = h("header", { class: "tv-shelf-head" }, h("h2", null, title), options.subtitle ? h("span", { class: "tv-shelf-sub" }, options.subtitle) : null, options.onMore ? h("button", { class: "tv-more", type: "button", onclick: options.onMore }, options.moreLabel ?? "See all", icon("chevronRight", 14)) : null);
  const section = h("section", { class: `tv-shelf${options.numbered ? " numbered" : ""}` }, head, wrap);
  requestAnimationFrame(sync);
  section.classList.add("reveal");
  reveal.observe(section);
  return section;
}

/** A glass message in the middle of a page. */
export function notice(glyph: Parameters<typeof icon>[0], title: string, text: string, ...actions: HTMLElement[]): HTMLElement {
  return h("div", { class: "tv-notice glass-strong" }, h("span", { class: "tv-notice-icon" }, icon(glyph, 30)), h("h2", null, title), h("p", null, text), actions.length ? h("div", { class: "tv-notice-actions" }, ...actions) : null);
}
