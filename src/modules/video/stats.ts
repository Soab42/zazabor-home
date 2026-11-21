/**
 * Playback statistics for tuning smoothness. Off unless the app runs with ZAZABOR_VIDEO_STATS=1;
 * then every two seconds the page logs delivered / drawn / dropped frames, the transport's per-frame
 * latency and the spread of the intervals between frames put on screen, to the app's stderr.
 */

const BASE = navigator.userAgent.includes("Windows") ? "http://cs.localhost/" : "cs://localhost/";

/** What the backend tells a page about frame delivery: its stats switch and the frame socket. */
export interface FrameInfo {
  stats?: boolean;
  port?: number;
  token?: string;
}

let info: Promise<FrameInfo> | null = null;

/** Asked once per page (the socket's port and token last as long as the app runs). */
export function frameInfo(): Promise<FrameInfo> {
  info ??= fetch(`${BASE}frame?info=1`, { cache: "no-store" })
    .then((r) => r.json() as Promise<FrameInfo>)
    .catch(() => {
      info = null;
      return {};
    });
  return info;
}

function percentile(sorted: number[], p: number): number {
  if (sorted.length === 0) return 0;
  return sorted[Math.min(sorted.length - 1, Math.floor(p * sorted.length))];
}

const round = (v: number) => Math.round(v * 10) / 10;

export class VideoStats {
  private enabled = false;
  private cost: number[] = [];
  private ticks: number[] = [];
  private lastTick = 0;
  get on() {
    return this.enabled;
  }
  private latency: number[] = [];
  private shown: number[] = [];
  private lastShown = 0;
  private delivered = 0;
  private dropped = 0;
  private duplicated = 0;
  private since = 0;
  private timer = 0;

  constructor() {
    frameInfo()
      .then((info) => {
        if (!info.stats) return;
        this.enabled = true;
        this.since = performance.now();
        this.timer = window.setInterval(() => this.report(), 2000);
      });
  }

  /** A frame reached the page `ms` after it was asked for / sent. */
  arrived(ms: number, skipped: number) {
    if (!this.enabled) return;
    this.latency.push(ms);
    this.delivered++;
    this.dropped += skipped;
  }

  /** Time one draw took, in ms (measured with the GPU finished). */
  drawCost(ms: number) {
    if (this.enabled) this.cost.push(ms);
  }

  /** A presenter callback ran at refresh time `now`. */
  tick(now: number) {
    if (!this.enabled) return;
    if (this.lastTick) this.ticks.push(now - this.lastTick);
    this.lastTick = now;
  }

  /** The presenter passed over `n` queued frames because a newer one was already due. */
  skipped(n: number) {
    if (this.enabled) this.dropped += n;
  }

  /** A frame went to the screen (`fresh` false: the previous picture was shown again). */
  shownFrame(fresh: boolean) {
    if (!this.enabled) return;
    const now = performance.now();
    if (fresh) {
      if (this.lastShown) this.shown.push(now - this.lastShown);
      this.lastShown = now;
    } else {
      this.duplicated++;
    }
  }

  private report() {
    const seconds = (performance.now() - this.since) / 1000;
    this.since = performance.now();
    const lat = [...this.latency].sort((a, b) => a - b);
    const gaps = [...this.shown].sort((a, b) => a - b);
    const line = {
      drawnFps: round(gaps.length / seconds),
      deliveredFps: round(this.delivered / seconds),
      dropped: this.dropped,
      duplicated: this.duplicated,
      latP50: round(percentile(lat, 0.5)),
      latP95: round(percentile(lat, 0.95)),
      gapP50: round(percentile(gaps, 0.5)),
      gapP95: round(percentile(gaps, 0.95)),
      gapMax: round(gaps[gaps.length - 1] ?? 0),
      drawP50: round(percentile([...this.cost].sort((a, b) => a - b), 0.5)),
      drawP95: round(percentile([...this.cost].sort((a, b) => a - b), 0.95)),
      rafP50: round(percentile([...this.ticks].sort((a, b) => a - b), 0.5)),
      rafP95: round(percentile([...this.ticks].sort((a, b) => a - b), 0.95)),
    };
    this.cost = [];
    this.ticks = [];
    this.latency = [];
    this.shown = [];
    this.delivered = this.dropped = this.duplicated = 0;
    void fetch(`${BASE}frame?stat=${encodeURIComponent(JSON.stringify(line))}`, { cache: "no-store" }).catch(() => {});
  }

  dispose() {
    clearInterval(this.timer);
  }
}
