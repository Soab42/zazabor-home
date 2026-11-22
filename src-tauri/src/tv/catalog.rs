//! `tv.db`: every title the crawl has seen (name-parsed fields at once, the video link and poster
//! once a card is looked at), when each was last played, and the live channels (`live.rs`).

use std::path::Path;

use rusqlite::{params, params_from_iter, types::Value, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub type Result<T> = std::result::Result<T, String>;

pub fn db_err(e: rusqlite::Error) -> String {
    format!("The TV catalog: {e}")
}

/// What a folder's name says; the crawl's output.
#[derive(Debug, Clone, PartialEq)]
pub struct NewItem {
    pub title: String,
    pub year: Option<i32>,
    pub quality: String,
    pub tags: Vec<String>,
    pub kind: String,
    pub section: String,
    pub category: String,
    pub category_url: String,
    pub folder_url: String,
    pub added: i64,
    pub rank: Option<u32>,
}

/// What looking inside a title's folder found.
#[derive(Debug, Clone, Default)]
pub struct Resolved {
    pub video_url: String,
    pub size: i64,
    pub poster: String,
    pub tags: Vec<String>,
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    pub title: String,
    pub year: Option<i32>,
    pub quality: String,
    pub tags: Vec<String>,
    pub kind: String,
    pub section: String,
    pub category: String,
    pub category_url: String,
    pub folder_url: String,
    pub video_url: String,
    pub size: i64,
    /// The poster's file in the cache, when it is there.
    pub poster: Option<String>,
    /// The card-sized copy of the poster, when there is one (small posters have none).
    pub poster_thumb: Option<String>,
    /// The qualities this title comes in (720p and 1080p folders hold the same film once).
    pub qualities: Vec<String>,
    /// Looked at and the folder has no picture: don't ask again.
    pub no_poster: bool,
    pub added: i64,
    pub rank: Option<u32>,
    pub played: i64,
    /// My List, Watch Later and Watched: the user's marks on the title (kept across catalog refreshes).
    pub fav: bool,
    pub later: bool,
    pub watched: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Filter {
    pub section: Option<String>,
    pub kind: Option<String>,
    pub category_url: Option<String>,
    pub q: Option<String>,
    pub year: Option<i32>,
    pub quality: Option<String>,
    /// "new" (server time), "az", "year", "rank", "played" or "marked" (latest added to `list` first).
    pub sort: Option<String>,
    pub played: bool,
    /// Only titles on one of the user's lists: "fav", "later" or "watched".
    pub list: Option<String>,
    /// Leave out the titles marked watched.
    pub hide_watched: bool,
    /// Only titles carrying this tag ("Dual Audio", "NF", "BluRay"…).
    pub tag: Option<String>,
    /// Instead of a page: every folder of this title (its qualities), ungrouped.
    pub same_as: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

pub fn make_id(folder_url: &str) -> String {
    let digest = Sha256::digest(folder_url.as_bytes());
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS items (
  id TEXT PRIMARY KEY, title TEXT NOT NULL, year INTEGER, quality TEXT NOT NULL DEFAULT '',
  tags TEXT NOT NULL DEFAULT '', kind TEXT NOT NULL, section TEXT NOT NULL,
  category TEXT NOT NULL, category_url TEXT NOT NULL, folder_url TEXT NOT NULL,
  video_url TEXT NOT NULL DEFAULT '', size INTEGER NOT NULL DEFAULT 0, poster TEXT NOT NULL DEFAULT '',
  resolved INTEGER NOT NULL DEFAULT 0, added INTEGER NOT NULL DEFAULT 0, rank INTEGER,
  played INTEGER NOT NULL DEFAULT 0, seen INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS items_added ON items(added);
CREATE INDEX IF NOT EXISTS items_cat ON items(category_url);
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
-- the user's lists, keyed by the title (name, year, kind) rather than a folder, so they outlive a catalog refresh
CREATE TABLE IF NOT EXISTS marks (key TEXT NOT NULL, list TEXT NOT NULL, at INTEGER NOT NULL, PRIMARY KEY(key, list));
";

pub fn open(path: &Path) -> Result<Connection> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("The TV catalog can't make its folder: {e}"))?;
    }
    let conn = Connection::open(path).map_err(db_err)?;
    init(&conn)?;
    Ok(conn)
}

pub fn init(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA).map_err(db_err)?;
    #[cfg(not(target_os = "android"))]
    super::download::schema(conn)?;
    super::live::schema(conn)
}

pub fn meta_get(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0)).optional().ok().flatten()
}

pub fn meta_set(conn: &Connection, key: &str, value: &str) {
    let _ = conn.execute("INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=?2", params![key, value]);
}

/// Adds the titles a crawl found and refreshes what a name can change; poster, video link,
/// last-played time and a kind the folder's contents settled stay. Returns how many are new.
pub fn upsert(conn: &mut Connection, items: &[NewItem], stamp: i64) -> Result<usize> {
    let tx = conn.transaction().map_err(db_err)?;
    let mut fresh = 0;
    {
        let mut insert = tx
            .prepare(
                "INSERT INTO items(id,title,year,quality,tags,kind,section,category,category_url,folder_url,added,rank,seen)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)
                 ON CONFLICT(id) DO UPDATE SET title=?2, year=?3, quality=?4, tags=CASE WHEN resolved=1 THEN tags ELSE ?5 END,
                   section=?7, category=?8, category_url=?9, added=?11, rank=?12, seen=?13",
            )
            .map_err(db_err)?;
        for it in items {
            let id = make_id(&it.folder_url);
            let existed: bool = tx.query_row("SELECT 1 FROM items WHERE id=?1", [&id], |_| Ok(true)).optional().map_err(db_err)?.unwrap_or(false);
            insert
                .execute(params![id, it.title, it.year, it.quality, it.tags.join(","), it.kind, it.section, it.category, it.category_url, it.folder_url, it.added, it.rank, stamp])
                .map_err(db_err)?;
            if !existed {
                fresh += 1;
            }
        }
    }
    tx.commit().map_err(db_err)?;
    Ok(fresh)
}

/// Forgets what a finished crawl of a category no longer saw.
pub fn prune(conn: &Connection, category_url: &str, stamp: i64) -> usize {
    conn.execute("DELETE FROM items WHERE category_url=?1 AND seen<?2", params![category_url, stamp]).unwrap_or(0)
}

/// Drops categories that are no longer wanted.
pub fn keep_categories(conn: &Connection, urls: &[String]) {
    if urls.is_empty() {
        let _ = conn.execute("DELETE FROM items", []);
        return;
    }
    let marks = vec!["?"; urls.len()].join(",");
    let _ = conn.execute(&format!("DELETE FROM items WHERE category_url NOT IN ({marks})"), params_from_iter(urls.iter()));
}

/// Which folder of a title shows first: 1080p, then 4K, 720p, 480p, the rest.
const QUALITY_RANK: &str = "CASE quality WHEN '1080p' THEN 0 WHEN '2160p' THEN 1 WHEN '720p' THEN 2 WHEN '480p' THEN 3 ELSE 4 END";
/// What makes two folders one title: its name, year, kind and section.
const GROUP_KEY: &str = "lower(title), COALESCE(year,0), kind, section";

fn quality_rank(q: &str) -> u8 {
    match q {
        "1080p" => 0,
        "2160p" => 1,
        "720p" => 2,
        "480p" => 3,
        _ => 4,
    }
}

/// What identifies a title for the user's lists (SQL, so the same text is made everywhere).
const MARK_KEY: &str = "lower(title)||'|'||COALESCE(year,0)||'|'||kind";

pub const LISTS: [&str; 3] = ["fav", "later", "watched"];

const COLUMNS: &str = "id,title,year,quality,tags,kind,section,category,category_url,folder_url,video_url,size,poster,resolved,added,rank,played";

fn item_from_row(r: &Row) -> rusqlite::Result<Item> {
    let tags: String = r.get(4)?;
    let poster: String = r.get(12)?;
    let resolved: i64 = r.get(13)?;
    let there = !poster.is_empty() && Path::new(&poster).exists();
    let thumb = there.then(|| super::crawl::thumb_path(Path::new(&poster))).filter(|t| t.exists()).map(|t| t.to_string_lossy().into_owned());
    let mut qualities: Vec<String> = Vec::new();
    for q in r.get::<_, Option<String>>(17).ok().flatten().unwrap_or_default().split(',').filter(|q| !q.is_empty()) {
        if !qualities.iter().any(|x| x == q) {
            qualities.push(q.to_string());
        }
    }
    qualities.sort_by_key(|q| quality_rank(q));
    Ok(Item {
        id: r.get(0)?,
        title: r.get(1)?,
        year: r.get(2)?,
        quality: r.get(3)?,
        tags: tags.split(',').filter(|t| !t.is_empty()).map(String::from).collect(),
        kind: r.get(5)?,
        section: r.get(6)?,
        category: super::names::clean_label(&r.get::<_, String>(7)?),
        category_url: r.get(8)?,
        folder_url: r.get(9)?,
        video_url: r.get(10)?,
        size: r.get(11)?,
        no_poster: poster.is_empty() && resolved == 1,
        poster: there.then_some(poster),
        poster_thumb: thumb,
        qualities,
        added: r.get(14)?,
        rank: r.get(15)?,
        played: r.get(16)?,
        fav: false,
        later: false,
        watched: false,
    })
}

/// Sets the list flags of `items` from the user's marks.
fn fill_marks(conn: &Connection, items: &mut [Item]) {
    if items.is_empty() {
        return;
    }
    let sql = format!("SELECT id, EXISTS(SELECT 1 FROM marks m WHERE m.list='fav' AND m.key={MARK_KEY}), EXISTS(SELECT 1 FROM marks m WHERE m.list='later' AND m.key={MARK_KEY}), EXISTS(SELECT 1 FROM marks m WHERE m.list='watched' AND m.key={MARK_KEY}) FROM items WHERE id IN ({})", vec!["?"; items.len()].join(","));
    let Ok(mut stmt) = conn.prepare(&sql) else { return };
    let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
    let Ok(rows) = stmt.query_map(params_from_iter(ids.iter()), |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?, r.get::<_, bool>(2)?, r.get::<_, bool>(3)?))) else { return };
    let flags: std::collections::HashMap<String, (bool, bool, bool)> = rows.filter_map(|r| r.ok()).map(|(id, a, b, c)| (id, (a, b, c))).collect();
    for item in items.iter_mut() {
        if let Some(&(fav, later, watched)) = flags.get(&item.id) {
            (item.fav, item.later, item.watched) = (fav, later, watched);
        }
    }
}

pub fn get(conn: &Connection, id: &str) -> Option<Item> {
    let mut item = conn.query_row(&format!("SELECT {COLUMNS} FROM items WHERE id=?1"), [id], item_from_row).optional().ok().flatten()?;
    fill_marks(conn, std::slice::from_mut(&mut item));
    Some(item)
}

/// Puts the title `id` on (or takes it off) one of the user's lists.
pub fn set_mark(conn: &Connection, id: &str, list: &str, on: bool, now: i64) -> Result<()> {
    if !LISTS.contains(&list) {
        return Err(format!("There is no list called {list}"));
    }
    if on {
        conn.execute(&format!("INSERT INTO marks(key,list,at) SELECT {MARK_KEY},?2,?3 FROM items WHERE id=?1 ON CONFLICT(key,list) DO UPDATE SET at=?3"), params![id, list, now]).map_err(db_err)?;
    } else {
        conn.execute(&format!("DELETE FROM marks WHERE list=?2 AND key=(SELECT {MARK_KEY} FROM items WHERE id=?1)"), params![id, list]).map_err(db_err)?;
    }
    Ok(())
}

/// The title each URL belongs to: a film's own video link, or a folder the URL is inside of (an
/// episode). Used to tie the player's resume points back to the catalog.
pub fn by_urls(conn: &Connection, urls: &[String]) -> Vec<(String, Item)> {
    let mut out = Vec::new();
    for url in urls {
        let found = conn
            .query_row(&format!("SELECT {COLUMNS} FROM items WHERE video_url=?1 OR (length(folder_url)>0 AND substr(?1,1,length(folder_url))=folder_url) ORDER BY length(folder_url) DESC LIMIT 1"), [url], item_from_row)
            .optional()
            .ok()
            .flatten()
            // a downloaded copy (the PC's `downloads` table; the phone has none, which just finds nothing)
            .or_else(|| conn.query_row(&format!("SELECT {COLUMNS} FROM items WHERE {MARK_KEY}=(SELECT key FROM downloads WHERE path=?1) LIMIT 1"), [url], item_from_row).optional().ok().flatten());
        if let Some(mut item) = found {
            fill_marks(conn, std::slice::from_mut(&mut item));
            out.push((url.clone(), item));
        }
    }
    out
}

/// The stored poster path (even when its file has gone) and whether the folder was looked at.
pub fn poster_state(conn: &Connection, id: &str) -> Option<(String, bool)> {
    conn.query_row("SELECT poster,resolved FROM items WHERE id=?1", [id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? == 1))).optional().ok().flatten()
}

fn like(q: &str) -> String {
    format!("%{}%", q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"))
}

fn where_clause(f: &Filter, with_year: bool, with_quality: bool) -> (String, Vec<Value>) {
    let mut parts: Vec<String> = Vec::new();
    let mut args: Vec<Value> = Vec::new();
    fn add(parts: &mut Vec<String>, args: &mut Vec<Value>, sql: &str, v: Value) {
        parts.push(sql.to_string());
        args.push(v);
    }
    if let Some(s) = &f.section {
        add(&mut parts, &mut args, "section=?", Value::Text(s.clone()));
    }
    if let Some(k) = &f.kind {
        add(&mut parts, &mut args, "kind=?", Value::Text(k.clone()));
    }
    if let Some(c) = &f.category_url {
        add(&mut parts, &mut args, "category_url=?", Value::Text(c.clone()));
    }
    if let Some(q) = f.q.as_deref().map(str::trim).filter(|q| !q.is_empty()) {
        for word in q.split_whitespace() {
            add(&mut parts, &mut args, "(title LIKE ? ESCAPE '\\' OR category LIKE ? ESCAPE '\\')", Value::Text(like(word)));
            args.push(Value::Text(like(word)));
        }
    }
    if with_year {
        if let Some(y) = f.year {
            add(&mut parts, &mut args, "year=?", Value::Integer(y as i64));
        }
    }
    if with_quality {
        if let Some(q) = f.quality.as_deref().filter(|q| !q.is_empty()) {
            add(&mut parts, &mut args, "quality=?", Value::Text(q.to_string()));
        }
    }
    if f.played {
        parts.push("played>0".to_string());
    }
    if let Some(list) = f.list.as_deref().filter(|l| LISTS.contains(l)) {
        parts.push(format!("EXISTS (SELECT 1 FROM marks m WHERE m.list='{list}' AND m.key={MARK_KEY})"));
    }
    if f.hide_watched {
        parts.push(format!("NOT EXISTS (SELECT 1 FROM marks m WHERE m.list='watched' AND m.key={MARK_KEY})"));
    }
    if let Some(tag) = f.tag.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        add(&mut parts, &mut args, "(',' || tags || ',') LIKE ? ESCAPE '\\'", Value::Text(format!("%,{},%", tag.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"))));
    }
    (if parts.is_empty() { String::new() } else { format!(" WHERE {}", parts.join(" AND ")) }, args)
}

/// A page of titles: the folders of one film in several qualities come out as one title (the
/// best quality first), with `qualities` listing them all.
pub fn query(conn: &Connection, f: &Filter) -> Result<(Vec<Item>, i64)> {
    let (clause, args) = where_clause(f, true, true);
    let total: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM (SELECT 1 FROM items{clause} GROUP BY {GROUP_KEY})"), params_from_iter(args.iter()), |r| r.get(0)).map_err(db_err)?;
    let order = match f.sort.as_deref() {
        Some("az") => "title COLLATE NOCASE ASC, year DESC",
        Some("year") => "COALESCE(year,0) DESC, added DESC",
        Some("rank") => "COALESCE(rank,100000) ASC, added DESC",
        Some("played") => "played DESC",
        Some("marked") => match f.list.as_deref().filter(|l| LISTS.contains(l)) {
            Some("fav") => "(SELECT at FROM marks m WHERE m.list='fav' AND m.key=lower(title)||'|'||COALESCE(year,0)||'|'||kind) DESC",
            Some("later") => "(SELECT at FROM marks m WHERE m.list='later' AND m.key=lower(title)||'|'||COALESCE(year,0)||'|'||kind) DESC",
            _ => "(SELECT at FROM marks m WHERE m.list='watched' AND m.key=lower(title)||'|'||COALESCE(year,0)||'|'||kind) DESC",
        },
        _ => "added DESC, title COLLATE NOCASE ASC",
    };
    let sql = format!(
        "WITH g AS (SELECT {COLUMNS}, ROW_NUMBER() OVER (PARTITION BY {GROUP_KEY} ORDER BY {QUALITY_RANK}, added DESC) AS rn, group_concat(quality, ',') OVER (PARTITION BY {GROUP_KEY}) AS qs FROM items{clause})
         SELECT {COLUMNS}, qs FROM g WHERE rn=1 ORDER BY {order} LIMIT {} OFFSET {}",
        f.limit.unwrap_or(60).clamp(1, 2000),
        f.offset.unwrap_or(0).max(0)
    );
    let mut stmt = conn.prepare(&sql).map_err(db_err)?;
    let rows = stmt.query_map(params_from_iter(args.iter()), item_from_row).map_err(db_err)?;
    let mut items: Vec<Item> = rows.filter_map(|r| r.ok()).collect();
    fill_marks(conn, &mut items);
    Ok((items, total))
}

/// Every folder of the same title as `id` (its 720p, 1080p, ... versions), the best quality first.
pub fn variants(conn: &Connection, id: &str) -> Result<Vec<Item>> {
    let Some(item) = get(conn, id) else { return Ok(Vec::new()) };
    let sql = format!("SELECT {COLUMNS} FROM items WHERE lower(title)=lower(?1) AND COALESCE(year,0)=COALESCE(?2,0) AND kind=?3 AND section=?4 ORDER BY {QUALITY_RANK}, added DESC");
    let mut stmt = conn.prepare(&sql).map_err(db_err)?;
    let rows = stmt.query_map(params![item.title, item.year, item.kind, item.section], item_from_row).map_err(db_err)?;
    let mut items: Vec<Item> = rows.filter_map(|r| r.ok()).collect();
    fill_marks(conn, &mut items);
    Ok(items)
}

/// The years and qualities a filter's titles have (ignoring the filter's own year and quality).
pub fn facets(conn: &Connection, f: &Filter) -> Result<(Vec<(i32, i64)>, Vec<(String, i64)>)> {
    let (clause, args) = where_clause(f, false, false);
    let mut years = Vec::new();
    let mut stmt = conn.prepare(&format!("SELECT year,COUNT(*) FROM items{clause}{} year IS NOT NULL GROUP BY year ORDER BY year DESC", if clause.is_empty() { " WHERE" } else { " AND" })).map_err(db_err)?;
    for row in stmt.query_map(params_from_iter(args.iter()), |r| Ok((r.get::<_, i32>(0)?, r.get::<_, i64>(1)?))).map_err(db_err)? {
        years.push(row.map_err(db_err)?);
    }
    let mut qualities = Vec::new();
    let mut stmt = conn.prepare(&format!("SELECT quality,COUNT(*) FROM items{clause}{} quality<>'' GROUP BY quality ORDER BY COUNT(*) DESC", if clause.is_empty() { " WHERE" } else { " AND" })).map_err(db_err)?;
    for row in stmt.query_map(params_from_iter(args.iter()), |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))).map_err(db_err)? {
        qualities.push(row.map_err(db_err)?);
    }
    Ok((years, qualities))
}

/// (url, label, section, kind, count) of the categories that have titles.
pub fn categories(conn: &Connection) -> Result<Vec<(String, String, String, String, i64)>> {
    let mut stmt = conn
        .prepare("SELECT category_url,category,section,MIN(kind),COUNT(DISTINCT lower(title)||COALESCE(year,0)) FROM items GROUP BY category_url ORDER BY MAX(added) DESC")
        .map_err(db_err)?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).map_err(db_err)?;
    Ok(rows.filter_map(|r| r.ok()).map(|(u, l, s, k, n): (String, String, String, String, i64)| (u, super::names::clean_label(&l), s, k, n)).collect())
}

pub fn set_resolved(conn: &Connection, id: &str, r: &Resolved) -> Result<()> {
    let tags = {
        let current: String = conn.query_row("SELECT tags FROM items WHERE id=?1", [id], |row| row.get(0)).optional().map_err(db_err)?.unwrap_or_default();
        let mut all: Vec<String> = current.split(',').filter(|t| !t.is_empty()).map(String::from).collect();
        for t in &r.tags {
            if !all.contains(t) {
                all.push(t.clone());
            }
        }
        all.join(",")
    };
    conn.execute(
        "UPDATE items SET video_url=?2,size=?3,poster=?4,tags=?5,kind=COALESCE(?6,kind),resolved=1 WHERE id=?1",
        params![id, r.video_url, r.size, r.poster, tags, r.kind],
    )
    .map_err(db_err)?;
    Ok(())
}

pub fn touch_played(conn: &Connection, id: &str, now: i64) {
    let _ = conn.execute("UPDATE items SET played=?2 WHERE id=?1", params![id, now]);
}

/// Forgets every poster (the cache was cleared): cards fetch theirs again when seen.
pub fn forget_posters(conn: &Connection) {
    let _ = conn.execute("UPDATE items SET poster='', resolved=0", []);
}

pub fn count(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0)).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new(title: &str, year: i32, added: i64, category: &str) -> NewItem {
        NewItem {
            title: title.into(),
            year: Some(year),
            quality: "1080p".into(),
            tags: vec!["Dual Audio".into()],
            kind: "movie".into(),
            section: "movies".into(),
            category: category.into(),
            category_url: format!("http://h/{category}/"),
            folder_url: format!("http://h/{category}/{title}/"),
            added,
            rank: None,
        }
    }

    fn mem() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        init(&c).unwrap();
        c
    }

    #[test]
    fn merge_keeps_what_the_user_and_the_folder_gave() {
        let mut c = mem();
        let a = new("Anaconda", 2025, 100, "English");
        assert_eq!(upsert(&mut c, &[a.clone(), new("Afterburn", 2025, 300, "English")], 1).unwrap(), 2);
        let id = make_id(&a.folder_url);
        set_resolved(&c, &id, &Resolved { video_url: "http://h/a.mkv".into(), size: 5, poster: "/nope.jpg".into(), tags: vec!["HEVC".into()], kind: None }).unwrap();
        touch_played(&c, &id, 777);
        // the next crawl sees it again with a newer time, and one more title
        let mut again = a.clone();
        again.added = 900;
        assert_eq!(upsert(&mut c, &[again, new("Zodiac", 2007, 50, "English")], 2).unwrap(), 1);
        let item = get(&c, &id).unwrap();
        assert_eq!((item.added, item.played, item.video_url.as_str(), item.size), (900, 777, "http://h/a.mkv", 5));
        assert_eq!(item.tags, vec!["Dual Audio", "HEVC"]);
        assert_eq!(item.poster, None, "the poster file isn't there");
        assert!(!item.no_poster);
        assert_eq!(count(&c), 3);
        // Afterburn wasn't seen in the second crawl: pruned once it completes
        assert_eq!(prune(&c, &a.category_url, 2), 1);
        assert_eq!(count(&c), 2);
    }

    #[test]
    fn queries_sort_filter_and_search() {
        let mut c = mem();
        let mut imdb = new("The Godfather", 1972, 10, "IMDb");
        imdb.rank = Some(2);
        let mut shawshank = new("The Shawshank Redemption", 1994, 20, "IMDb");
        shawshank.rank = Some(1);
        let mut show = new("18 Again", 2020, 40, "Korean");
        show.kind = "series".into();
        show.quality = "720p".into();
        upsert(&mut c, &[imdb, shawshank, show, new("Anaconda", 2025, 30, "English")], 1).unwrap();
        let titles = |f: Filter| query(&c, &f).unwrap().0.into_iter().map(|i| i.title).collect::<Vec<_>>();
        assert_eq!(titles(Filter::default()), vec!["18 Again", "Anaconda", "The Shawshank Redemption", "The Godfather"]);
        assert_eq!(titles(Filter { sort: Some("rank".into()), category_url: Some("http://h/IMDb/".into()), ..Default::default() }), vec!["The Shawshank Redemption", "The Godfather"]);
        assert_eq!(titles(Filter { kind: Some("series".into()), ..Default::default() }), vec!["18 Again"]);
        assert_eq!(titles(Filter { q: Some("the god".into()), ..Default::default() }), vec!["The Godfather"]);
        assert_eq!(titles(Filter { quality: Some("720p".into()), ..Default::default() }), vec!["18 Again"]);
        assert_eq!(titles(Filter { year: Some(1994), ..Default::default() }), vec!["The Shawshank Redemption"]);
        assert_eq!(titles(Filter { sort: Some("az".into()), limit: Some(2), ..Default::default() }), vec!["18 Again", "Anaconda"]);
        assert_eq!(titles(Filter { q: Some("100%".into()), ..Default::default() }), Vec::<String>::new());
        let (years, qualities) = facets(&c, &Filter::default()).unwrap();
        assert_eq!(years.first(), Some(&(2025, 1)));
        assert_eq!(qualities[0], ("1080p".to_string(), 3));
        assert_eq!(categories(&c).unwrap().len(), 3);
        keep_categories(&c, &["http://h/IMDb/".to_string()]);
        assert_eq!(count(&c), 2);
    }

    #[test]
    fn qualities_of_one_title_group() {
        let mut c = mem();
        let mut low = new("Anaconda", 2025, 20, "English 720p");
        low.quality = "720p".into();
        upsert(&mut c, &[low, new("Anaconda", 2025, 10, "English 1080p"), new("Zodiac", 2007, 5, "English 1080p")], 1).unwrap();
        let (items, total) = query(&c, &Filter::default()).unwrap();
        assert_eq!(total, 2);
        assert_eq!(items.len(), 2);
        assert_eq!((items[0].title.as_str(), items[0].quality.as_str(), items[0].qualities.clone()), ("Anaconda", "1080p", vec!["1080p".to_string(), "720p".to_string()]));
        let (only, n) = query(&c, &Filter { quality: Some("720p".into()), ..Default::default() }).unwrap();
        assert_eq!((n, only[0].quality.as_str()), (1, "720p"));
        let all = variants(&c, &items[0].id).unwrap();
        assert_eq!(all.iter().map(|i| i.quality.as_str()).collect::<Vec<_>>(), vec!["1080p", "720p"]);
    }

    #[test]
    fn marks_outlive_a_catalog_refresh_and_filter() {
        let mut c = mem();
        let mut nf = new("Anaconda", 2025, 30, "English");
        nf.tags = vec!["Dual Audio".into(), "NF".into()];
        let mut other = new("Zodiac", 2007, 5, "English");
        other.tags = vec!["BluRay".into()];
        upsert(&mut c, &[nf.clone(), other.clone(), new("Afterburn", 2025, 300, "English")], 1).unwrap();
        let id = make_id(&nf.folder_url);
        set_mark(&c, &id, "fav", true, 10).unwrap();
        set_mark(&c, &make_id(&other.folder_url), "fav", true, 20).unwrap();
        set_mark(&c, &id, "later", true, 11).unwrap();
        set_mark(&c, &make_id(&other.folder_url), "watched", true, 12).unwrap();
        assert!(set_mark(&c, &id, "bogus", true, 1).is_err());
        let item = get(&c, &id).unwrap();
        assert_eq!((item.fav, item.later, item.watched), (true, true, false));
        let titles = |c: &Connection, f: Filter| query(c, &f).unwrap().0.into_iter().map(|i| i.title).collect::<Vec<_>>();
        assert_eq!(titles(&c, Filter { list: Some("fav".into()), sort: Some("marked".into()), ..Default::default() }), vec!["Zodiac", "Anaconda"]);
        assert_eq!(titles(&c, Filter { list: Some("later".into()), ..Default::default() }), vec!["Anaconda"]);
        assert_eq!(titles(&c, Filter { hide_watched: true, sort: Some("az".into()), ..Default::default() }), vec!["Afterburn", "Anaconda"]);
        assert_eq!(titles(&c, Filter { tag: Some("NF".into()), ..Default::default() }), vec!["Anaconda"]);
        assert_eq!(titles(&c, Filter { tag: Some("Dual Audio".into()), ..Default::default() }), vec!["Afterburn", "Anaconda"], "Zodiac is BluRay only");
        assert_eq!(titles(&c, Filter { tag: Some("N".into()), ..Default::default() }), Vec::<String>::new());
        // the catalog is forgotten and read again, the title now in another category's folder
        c.execute("DELETE FROM items", []).unwrap();
        keep_categories(&c, &["http://h/Elsewhere/".to_string()]);
        let mut moved = new("Anaconda", 2025, 400, "Elsewhere");
        moved.quality = "720p".into();
        upsert(&mut c, &[moved.clone()], 2).unwrap();
        let again = get(&c, &make_id(&moved.folder_url)).unwrap();
        assert_eq!((again.fav, again.later), (true, true), "the lists are about the title, not the folder");
        // toggling off
        set_mark(&c, &again.id, "later", false, 3).unwrap();
        assert!(!get(&c, &again.id).unwrap().later);
        assert!(get(&c, &again.id).unwrap().fav);
    }

    #[test]
    fn resume_points_find_their_title() {
        let mut c = mem();
        let mut show = new("18 Again", 2020, 40, "Korean");
        show.kind = "series".into();
        let film = new("Anaconda", 2025, 30, "English");
        upsert(&mut c, &[show.clone(), film.clone()], 1).unwrap();
        set_resolved(&c, &make_id(&film.folder_url), &Resolved { video_url: "http://h/English/Anaconda/a.mkv".into(), ..Default::default() }).unwrap();
        let found = by_urls(&c, &["http://h/English/Anaconda/a.mkv".to_string(), format!("{}S01/e1.mkv", show.folder_url), "http://elsewhere/x.mkv".to_string()]);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].1.title, "Anaconda");
        assert_eq!(found[1].1.title, "18 Again");
    }

    #[test]
    fn ids_are_stable() {
        assert_eq!(make_id("http://h/a/"), make_id("http://h/a/"));
        assert_ne!(make_id("http://h/a/"), make_id("http://h/b/"));
        assert_eq!(make_id("x").len(), 16);
    }
}
