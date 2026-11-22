import { titleKey, type Item, type List } from "./api";

/**
 * The user's lists (My List, Watch Later, Watched) as the page knows them. A title's marks belong
 * to the title (`titleKey`), so a card, the hero and the detail page of the same film agree: the
 * first sight of a title (from the backend) seeds it, and after that only `set` changes it.
 */
export class Marks {
  private flags = new Map<string, Set<List>>();
  private subs = new Set<() => void>();

  private of(item: Item): Set<List> {
    const key = titleKey(item);
    let set = this.flags.get(key);
    if (!set) {
      set = new Set<List>();
      if (item.fav) set.add("fav");
      if (item.later) set.add("later");
      if (item.watched) set.add("watched");
      this.flags.set(key, set);
    }
    return set;
  }

  has(item: Item, list: List): boolean {
    return this.of(item).has(list);
  }

  set(item: Item, list: List, on: boolean) {
    const set = this.of(item);
    if (on === set.has(list)) return;
    if (on) set.add(list);
    else set.delete(list);
    item[list] = on;
    this.subs.forEach((fn) => fn());
  }

  /** Calls `fn` after any mark changes; returns the way to stop. */
  subscribe(fn: () => void): () => void {
    this.subs.add(fn);
    return () => this.subs.delete(fn);
  }
}
