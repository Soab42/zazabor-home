import type { VideoPosition } from "../../config";
import type { Season } from "./api";

/** At this fraction a film counts as watched. */
export const WATCHED_AT = 0.9;
/** Less than this and it isn't worth a "continue" row. */
export const MIN_MS = 15000;

export type Positions = Record<string, VideoPosition>;

export const fractionOf = (pos: VideoPosition | undefined): number => (pos && pos.length > 0 ? Math.min(1, pos.time / pos.length) : 0);

/** The resume points that are in progress (started, not finished), the latest first. */
export function inProgress(positions: Positions, max = 60): { url: string; pos: VideoPosition; fraction: number }[] {
  return Object.entries(positions)
    .map(([url, pos]) => ({ url, pos, fraction: fractionOf(pos) }))
    .filter((e) => e.pos.time >= MIN_MS && e.fraction < WATCHED_AT)
    .sort((a, b) => b.pos.at - a.pos.at)
    .slice(0, max);
}

/** Resume points that reached the end. */
export function finished(positions: Positions, max = 80): { url: string; pos: VideoPosition }[] {
  return Object.entries(positions)
    .map(([url, pos]) => ({ url, pos }))
    .filter((e) => e.pos.length > 0 && fractionOf(e.pos) >= WATCHED_AT)
    .sort((a, b) => b.pos.at - a.pos.at)
    .slice(0, max);
}

export interface Pick {
  url: string;
  /** "Continue" (part-way through it), "Next" (the one after a finished one) or "Start". */
  how: "continue" | "next" | "start";
  /** Season and episode numbers for the label. */
  season: number | null;
  episode: number | null;
}

/**
 * Which episode to play for a series: the one being watched, else the one after the latest
 * finished, else the first. `fraction(url)` is what the player remembers of that file.
 */
export function pickEpisode(seasons: Season[], fraction: (url: string) => number, at: (url: string) => number): Pick | null {
  const all = seasons.flatMap((s) => s.episodes);
  if (!all.length) return null;
  let last = -1;
  let lastAt = 0;
  all.forEach((e, i) => {
    const f = fraction(e.url);
    if (f > 0 && at(e.url) >= lastAt) {
      last = i;
      lastAt = at(e.url);
    }
  });
  const pick = (i: number, how: Pick["how"]): Pick => ({ url: all[i].url, how, season: all[i].season, episode: all[i].episode });
  if (last < 0) return pick(0, "start");
  if (fraction(all[last].url) < WATCHED_AT) return pick(last, "continue");
  return last + 1 < all.length ? pick(last + 1, "next") : pick(0, "start");
}

export const eta = (seconds: number): string => {
  if (!isFinite(seconds) || seconds <= 0) return "";
  if (seconds < 60) return `${Math.ceil(seconds)} s left`;
  if (seconds < 3600) return `${Math.ceil(seconds / 60)} min left`;
  return `${Math.floor(seconds / 3600)} h ${Math.ceil((seconds % 3600) / 60)} min left`;
};
