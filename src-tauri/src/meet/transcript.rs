//! A meeting's transcript on a timeline: segments (who, when, in which language, what), merged
//! from the per-speaker tracks, written as `.transcript.json` and WebVTT subtitles (original,
//! English, Bangla), cut into chapters for the notes, and the strict JSON the notes model must
//! answer in (validated, with the error text a repair prompt can use). No file is touched here.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One thing said, by one speaker.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Seg {
    pub speaker: String,
    /// The track file's stem (`1-Ayesha`).
    pub track: String,
    /// Seconds from the start of the recording.
    pub start: f64,
    pub end: f64,
    /// Whisper's code for what was spoken: "en", "bn".
    pub lang: String,
    /// As spoken.
    pub text: String,
    pub en: Option<String>,
    pub bn: Option<String>,
}

impl Seg {
    /// The English version: the original when it was English.
    pub fn english(&self) -> Option<&str> {
        self.en.as_deref().filter(|t| !t.is_empty()).or((self.lang == "en").then_some(self.text.as_str()))
    }
    pub fn bangla(&self) -> Option<&str> {
        self.bn.as_deref().filter(|t| !t.is_empty()).or((self.lang == "bn").then_some(self.text.as_str()))
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Doc {
    pub version: u32,
    /// The Whisper model(s) that heard it.
    pub model: String,
    /// "en", "bn" or "mixed".
    pub language: String,
    pub duration: f64,
    pub segments: Vec<Seg>,
}

/// What Whisper heard on one track, before merging.
pub struct Heard {
    pub speaker: String,
    pub track: String,
    /// Seconds into the meeting where this track starts.
    pub offset: f64,
    pub segments: Vec<RawSeg>,
}

#[derive(Clone, Debug)]
pub struct RawSeg {
    pub start: f64,
    pub end: f64,
    pub lang: String,
    pub text: String,
    pub en: Option<String>,
}

/// Puts every track's segments on the meeting's clock, in order of when they start. Empty text
/// is dropped; overlapping speakers are kept.
pub fn merge(tracks: Vec<Heard>) -> Vec<Seg> {
    let mut out: Vec<Seg> = tracks
        .into_iter()
        .flat_map(|t| {
            t.segments.into_iter().filter(|s| !s.text.trim().is_empty()).map(move |s| Seg {
                speaker: t.speaker.clone(),
                track: t.track.clone(),
                start: s.start + t.offset,
                end: s.end + t.offset,
                lang: s.lang,
                text: s.text.trim().to_string(),
                en: s.en.map(|e| e.trim().to_string()).filter(|e| !e.is_empty()),
                bn: None,
            })
        })
        .collect();
    out.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.end.total_cmp(&b.end)).then_with(|| a.speaker.cmp(&b.speaker)));
    out
}

/// "en", "bn" or "mixed" by how much of the speaking time each language had.
pub fn dominant_language(segs: &[Seg]) -> String {
    let (mut en, mut bn) = (0f64, 0f64);
    for s in segs {
        match s.lang.as_str() {
            "bn" => bn += s.end - s.start,
            _ => en += s.end - s.start,
        }
    }
    let total = en + bn;
    if total <= 0.0 || en / total >= 0.85 {
        "en".into()
    } else if bn / total >= 0.85 {
        "bn".into()
    } else {
        "mixed".into()
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Track {
    Original,
    English,
    Bangla,
}

/// `HH:MM:SS.mmm`.
pub fn vtt_time(seconds: f64) -> String {
    let ms = (seconds.max(0.0) * 1000.0).round() as u64;
    format!("{:02}:{:02}:{:02}.{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

/// `m:ss` or `h:mm:ss`, for notes.
pub fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// A WebVTT file: one cue per segment, "Speaker: text". Segments with no text in this language
/// (an English cue for Bangla speech that wasn't translated) are left out.
pub fn vtt(segs: &[Seg], which: Track) -> String {
    let mut out = String::from("WEBVTT\n\n");
    let mut n = 0;
    for s in segs {
        let text = match which {
            Track::Original => Some(s.text.as_str()),
            Track::English => s.english(),
            Track::Bangla => s.bangla(),
        };
        let Some(text) = text.map(str::trim).filter(|t| !t.is_empty()) else { continue };
        n += 1;
        let end = if s.end > s.start { s.end } else { s.start + 0.5 };
        // A blank line would end the cue.
        let body = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n");
        out.push_str(&format!("{n}\n{} --> {}\n{}: {}\n\n", vtt_time(s.start), vtt_time(end), escape(&s.speaker), escape(&body)));
    }
    out
}

// ---- chapters

/// How the transcript is cut for the notes.
#[derive(Clone, Copy, Debug)]
pub struct Cut {
    /// A chapter may end once it's this long and there's a pause.
    pub target: f64,
    /// A chapter never goes past this.
    pub max: f64,
    /// Nor past this much text (what the model can take in one go).
    pub max_chars: usize,
}

pub const CUT: Cut = Cut { target: 240.0, max: 330.0, max_chars: 6000 };

/// Index ranges of `segs` (end exclusive) for chapters of about 3–5 minutes, cut at pauses.
/// `len` is how much text a segment adds.
pub fn chapters(segs: &[Seg], cut: Cut, len: impl Fn(&Seg) -> usize) -> Vec<std::ops::Range<usize>> {
    let mut out: Vec<std::ops::Range<usize>> = Vec::new();
    let (mut from, mut chars) = (0usize, 0usize);
    for i in 0..segs.len() {
        chars += len(&segs[i]);
        let span = segs[i].end - segs[from].start;
        let pause = segs.get(i + 1).map_or(f64::INFINITY, |n| n.start - segs[i].end);
        let close = i + 1 == segs.len() || span >= cut.max || chars >= cut.max_chars || (span >= cut.target && pause >= 1.5);
        if close {
            out.push(from..i + 1);
            from = i + 1;
            chars = 0;
        }
    }
    // A short tail joins the chapter before it, if that stays within reason.
    if out.len() >= 2 {
        let last = out[out.len() - 1].clone();
        let prev = out[out.len() - 2].clone();
        let tail = segs[last.end - 1].end - segs[last.start].start;
        let joined: usize = segs[prev.start..last.end].iter().map(&len).sum();
        if tail < cut.target * 0.4 && joined <= cut.max_chars {
            out.pop();
            let n = out.len();
            out[n - 1] = prev.start..last.end;
        }
    }
    out
}

// ---- the notes' strict JSON

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Action {
    pub owner: String,
    pub task: String,
    pub due: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Chapter {
    pub start: f64,
    pub end: f64,
    pub title: String,
    pub summary: String,
}

/// The notes in one language.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Notes {
    pub summary: String,
    pub decisions: Vec<String>,
    pub actions: Vec<Action>,
    pub questions: Vec<String>,
    pub chapters: Vec<Chapter>,
}

/// The JSON object in a model's answer: it may come in a code fence or with a sentence around it.
pub fn extract_json(text: &str) -> Result<Value, String> {
    let (Some(a), Some(b)) = (text.find('{'), text.rfind('}')) else { return Err("there is no JSON object in the answer".into()) };
    if b < a {
        return Err("there is no JSON object in the answer".into());
    }
    serde_json::from_str(&text[a..=b]).map_err(|e| format!("the JSON is not valid ({e})"))
}

fn string_field(v: &Value, key: &str, required: bool) -> Result<String, String> {
    match v.get(key) {
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(s.trim().to_string()),
        Some(Value::String(_)) | None if !required => Ok(String::new()),
        Some(Value::String(_)) => Err(format!("\"{key}\" is empty")),
        Some(_) => Err(format!("\"{key}\" must be a string")),
        None => Err(format!("\"{key}\" is missing")),
    }
}

fn list_field(v: &Value, key: &str) -> Result<Vec<String>, String> {
    let Some(arr) = v.get(key) else { return Err(format!("\"{key}\" is missing")) };
    let Some(arr) = arr.as_array() else { return Err(format!("\"{key}\" must be a list of strings")) };
    let mut out = Vec::new();
    for item in arr {
        let Some(s) = item.as_str() else { return Err(format!("\"{key}\" must be a list of strings")) };
        if !s.trim().is_empty() {
            out.push(s.trim().to_string());
        }
    }
    out.truncate(20);
    Ok(out)
}

fn actions_field(v: &Value) -> Result<Vec<Action>, String> {
    let Some(items) = v.get("actions") else { return Err("\"actions\" is missing".into()) };
    let Some(items) = items.as_array() else { return Err("\"actions\" must be a list of objects with owner, task and due".into()) };
    let mut actions = Vec::new();
    for item in items.iter().take(20) {
        if !item.is_object() {
            return Err("\"actions\" must be a list of objects with owner, task and due".into());
        }
        let task = string_field(item, "task", true).map_err(|e| format!("an action: {e}"))?;
        let owner = string_field(item, "owner", false)?;
        let due = match item.get("due") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) if s.trim().is_empty() || ["null", "none", "n/a", "unknown"].contains(&s.trim().to_lowercase().as_str()) => None,
            Some(Value::String(s)) => Some(s.trim().to_string()),
            Some(_) => return Err("an action's \"due\" must be a string or null".into()),
        };
        actions.push(Action { owner: if owner.is_empty() { "Unassigned".into() } else { owner }, task, due });
    }
    Ok(actions)
}

/// The answer for one chapter: a title, a summary, and what was decided, asked and assigned in it
/// (the lists may be left out, but if they're there they must be right).
pub fn parse_chapter(text: &str) -> Result<(String, Notes), String> {
    let v = extract_json(text)?;
    let title = string_field(&v, "title", true)?;
    let summary = string_field(&v, "summary", true)?;
    let decisions = if v.get("decisions").is_some() { list_field(&v, "decisions")? } else { Vec::new() };
    let questions = if v.get("questions").is_some() { list_field(&v, "questions")? } else { Vec::new() };
    let actions = if v.get("actions").is_some() { actions_field(&v)? } else { Vec::new() };
    Ok((title, Notes { summary, decisions, actions, questions, chapters: Vec::new() }))
}

/// The answer for the whole meeting: summary, decisions, actions, questions (all four required).
pub fn parse_overall(text: &str) -> Result<Notes, String> {
    let v = extract_json(text)?;
    let summary = string_field(&v, "summary", true)?;
    let decisions = list_field(&v, "decisions")?;
    let questions = list_field(&v, "questions")?;
    let actions = actions_field(&v)?;
    Ok(Notes { summary, decisions, actions, questions, chapters: Vec::new() })
}

/// The answer for a batch of translated chapters: `{"chapters": [{"title", "summary"}, …]}` with `n` of them.
pub fn parse_chapter_texts(text: &str, n: usize) -> Result<Vec<(String, String)>, String> {
    let v = extract_json(text)?;
    let Some(arr) = v.get("chapters").and_then(Value::as_array) else { return Err("\"chapters\" must be a list".into()) };
    if arr.len() != n {
        return Err(format!("\"chapters\" must have exactly {n} items, not {}", arr.len()));
    }
    arr.iter().map(|c| Ok((string_field(c, "title", true)?, string_field(c, "summary", true)?))).collect()
}

fn heading(lang: &str) -> [&'static str; 5] {
    if lang == "bn" {
        ["সারসংক্ষেপ", "সিদ্ধান্ত", "করণীয়", "অমীমাংসিত প্রশ্ন", "মূল মুহূর্ত"]
    } else {
        ["Summary", "Decisions", "Action items", "Open questions", "Key moments"]
    }
}

/// The notes as Markdown (`lang` "en" or "bn" picks the headings).
pub fn notes_markdown(notes: &Notes, lang: &str) -> String {
    let h = heading(lang);
    let mut out = format!("## {}\n\n{}\n\n", h[0], notes.summary);
    let mut list = |title: &str, items: Vec<String>| {
        if !items.is_empty() {
            out.push_str(&format!("## {title}\n\n"));
            for i in items {
                out.push_str(&format!("- {i}\n"));
            }
            out.push('\n');
        }
    };
    list(h[1], notes.decisions.clone());
    list(h[2], notes.actions.iter().map(|a| format!("**{}:** {}{}", a.owner, a.task, a.due.as_ref().map(|d| format!(" ({d})")).unwrap_or_default())).collect());
    list(h[3], notes.questions.clone());
    list(h[4], notes.chapters.iter().map(|c| format!("**{}** {} — {}", clock(c.start), c.title, c.summary)).collect());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(start: f64, end: f64, lang: &str, text: &str) -> RawSeg {
        RawSeg { start, end, lang: lang.into(), text: text.into(), en: None }
    }

    fn seg(speaker: &str, start: f64, end: f64, lang: &str, text: &str) -> Seg {
        Seg { speaker: speaker.into(), track: "t".into(), start, end, lang: lang.into(), text: text.into(), en: None, bn: None }
    }

    #[test]
    fn tracks_merge_onto_one_timeline_with_their_offsets() {
        let a = Heard { speaker: "Ayesha".into(), track: "1-Ayesha".into(), offset: 0.0, segments: vec![raw(1.0, 3.0, "en", "Hello everyone."), raw(10.0, 12.0, "en", "  ")] };
        // Tanvir joined 5 s in and speaks over Ayesha's second line.
        let t = Heard { speaker: "Tanvir".into(), track: "2-Tanvir".into(), offset: 5.0, segments: vec![raw(0.5, 2.0, "bn", "সবাইকে স্বাগতম"), raw(6.0, 8.0, "en", "Right.")] };
        let m = merge(vec![t, a]);
        let got: Vec<(&str, f64)> = m.iter().map(|s| (s.speaker.as_str(), s.start)).collect();
        assert_eq!(got, [("Ayesha", 1.0), ("Tanvir", 5.5), ("Tanvir", 11.0)]);
        assert_eq!(m.len(), 3, "the blank segment was dropped");
        assert_eq!(m[1].end, 7.0);
    }

    #[test]
    fn dominant_language_follows_speaking_time() {
        assert_eq!(dominant_language(&[seg("a", 0.0, 10.0, "en", "x")]), "en");
        assert_eq!(dominant_language(&[seg("a", 0.0, 10.0, "bn", "x")]), "bn");
        assert_eq!(dominant_language(&[seg("a", 0.0, 10.0, "bn", "x"), seg("b", 10.0, 18.0, "en", "y")]), "mixed");
        assert_eq!(dominant_language(&[]), "en");
    }

    #[test]
    fn vtt_times_and_cues() {
        assert_eq!(vtt_time(0.0), "00:00:00.000");
        assert_eq!(vtt_time(3725.5), "01:02:05.500");
        assert_eq!(vtt_time(59.9996), "00:01:00.000");
        assert_eq!(clock(75.0), "1:15");
        assert_eq!(clock(3725.0), "1:02:05");
        let mut s = seg("Ayesha", 1.2, 4.5, "en", "Tom & Jerry <3\n\nare here");
        s.bn = Some("টম ও জেরি এখানে".into());
        let v = vtt(&[s], Track::Original);
        assert_eq!(v, "WEBVTT\n\n1\n00:00:01.200 --> 00:00:04.500\nAyesha: Tom &amp; Jerry &lt;3\nare here\n\n");
    }

    #[test]
    fn the_bangla_and_english_files_hold_each_languages_text_and_timing() {
        let mut a = seg("Ayesha", 1.0, 3.0, "en", "We ship on Friday.");
        a.bn = Some("আমরা শুক্রবার পাঠাব।".into());
        let mut b = seg("Tanvir", 3.5, 6.25, "bn", "আমি ডিজাইন শেষ করব।");
        b.en = Some("I will finish the design.".into());
        let c = seg("Nadia", 7.0, 8.0, "bn", "ঠিক আছে।");
        let segs = [a, b, c];
        let bn = vtt(&segs, Track::Bangla);
        assert_eq!(bn, "WEBVTT\n\n1\n00:00:01.000 --> 00:00:03.000\nAyesha: আমরা শুক্রবার পাঠাব।\n\n2\n00:00:03.500 --> 00:00:06.250\nTanvir: আমি ডিজাইন শেষ করব।\n\n3\n00:00:07.000 --> 00:00:08.000\nNadia: ঠিক আছে।\n\n");
        let en = vtt(&segs, Track::English);
        assert!(en.contains("Ayesha: We ship on Friday.") && en.contains("Tanvir: I will finish the design."));
        assert!(!en.contains("Nadia"), "an untranslated line has no English cue");
        let all = vtt(&segs, Track::Original);
        assert!(all.contains("Nadia: ঠিক আছে।") && all.contains("Ayesha: We ship on Friday."));
    }

    #[test]
    fn a_zero_length_segment_still_shows() {
        let v = vtt(&[seg("A", 2.0, 2.0, "en", "Yes.")], Track::Original);
        assert!(v.contains("00:00:02.000 --> 00:00:02.500"));
    }

    fn talk(minutes: f64) -> Vec<Seg> {
        // A sentence every 10 s with a pause after every 20th (a minute or so apart in practice).
        (0..(minutes * 6.0) as usize).map(|i| seg("A", i as f64 * 10.0, i as f64 * 10.0 + 8.0, "en", &"word ".repeat(20))).collect()
    }

    #[test]
    fn chapters_are_three_to_five_minutes_and_cover_everything() {
        let segs = talk(20.0);
        let ch = chapters(&segs, CUT, |s| s.text.len());
        assert!(ch.len() >= 4 && ch.len() <= 7, "{} chapters", ch.len());
        assert_eq!(ch[0].start, 0);
        assert_eq!(ch.last().unwrap().end, segs.len());
        for w in ch.windows(2) {
            assert_eq!(w[0].end, w[1].start);
        }
        for r in &ch[..ch.len() - 1] {
            let span = segs[r.end - 1].end - segs[r.start].start;
            assert!((150.0..=CUT.max + 10.0).contains(&span), "{span}");
        }
    }

    #[test]
    fn chapters_end_at_a_pause_once_long_enough() {
        let mut segs = talk(8.0);
        // A 5 s pause at the 250 s mark: the first chapter should end there, not at 330 s.
        for s in segs.iter_mut().skip(25) {
            s.start += 5.0;
            s.end += 5.0;
        }
        let ch = chapters(&segs, CUT, |s| s.text.len());
        assert_eq!(ch[0], 0..25);
    }

    #[test]
    fn a_short_tail_joins_the_chapter_before_and_short_meetings_are_one_chapter() {
        let segs = talk(4.4);
        assert_eq!(chapters(&segs, CUT, |s| s.text.len()).len(), 1);
        let segs = talk(5.5);
        let ch = chapters(&segs, CUT, |s| s.text.len());
        assert_eq!(ch.len(), 1, "{ch:?}");
        assert!(chapters(&[], CUT, |s| s.text.len()).is_empty());
    }

    #[test]
    fn text_volume_also_ends_a_chapter() {
        let segs: Vec<Seg> = (0..10).map(|i| seg("A", i as f64 * 5.0, i as f64 * 5.0 + 4.0, "en", &"x".repeat(2000))).collect();
        let ch = chapters(&segs, CUT, |s| s.text.len());
        assert!(ch.iter().all(|r| r.len() <= 3), "{ch:?}");
    }

    #[test]
    fn json_comes_out_of_fences_and_chatter() {
        let v = extract_json("Sure! Here you go:\n```json\n{\"title\": \"Plan\", \"summary\": \"We planned.\"}\n```\nAnything else?").unwrap();
        assert_eq!(v["title"], "Plan");
        assert!(extract_json("no json here").unwrap_err().contains("no JSON"));
        assert!(extract_json("{\"a\": ").unwrap_err().contains("no JSON") || extract_json("{\"a\": ").is_err());
        assert!(extract_json("{\"a\": 1,}").unwrap_err().contains("not valid"));
    }

    #[test]
    fn a_chapter_answer_needs_title_and_summary() {
        let (title, n) = parse_chapter("{\"title\":\" Budget \",\"summary\":\"They set it.\"}").unwrap();
        assert_eq!((title.as_str(), n.summary.as_str(), n.actions.len()), ("Budget", "They set it.", 0));
        let (_, n) = parse_chapter("{\"title\":\"T\",\"summary\":\"S\",\"decisions\":[\"d\"],\"actions\":[{\"owner\":\"Ana\",\"task\":\"x\"}]}").unwrap();
        assert_eq!((n.decisions.len(), n.actions[0].owner.as_str()), (1, "Ana"));
        assert!(parse_chapter("{\"title\":\"T\",\"summary\":\"S\",\"actions\":\"none\"}").unwrap_err().contains("actions"));
        assert!(parse_chapter("{\"title\":\"x\"}").unwrap_err().contains("\"summary\" is missing"));
        assert!(parse_chapter("{\"title\":3,\"summary\":\"s\"}").unwrap_err().contains("must be a string"));
        assert!(parse_chapter("{\"title\":\"  \",\"summary\":\"s\"}").unwrap_err().contains("empty"));
    }

    #[test]
    fn the_overall_answer_is_checked_and_tidied() {
        let ok = r#"{"summary":"We agreed.","decisions":["Ship Friday"," "],"actions":[{"owner":"Tanvir","task":"Finish the design","due":"Thursday"},{"owner":"","task":"Book a room","due":"null"},{"task":"Email Sam","due":null}],"questions":[]}"#;
        let n = parse_overall(ok).unwrap();
        assert_eq!(n.decisions, ["Ship Friday"]);
        assert_eq!(n.actions[0], Action { owner: "Tanvir".into(), task: "Finish the design".into(), due: Some("Thursday".into()) });
        assert_eq!(n.actions[1], Action { owner: "Unassigned".into(), task: "Book a room".into(), due: None });
        assert_eq!(n.actions[2].owner, "Unassigned");
        assert!(parse_overall(r#"{"summary":"s","decisions":"none","actions":[],"questions":[]}"#).unwrap_err().contains("\"decisions\" must be a list"));
        assert!(parse_overall(r#"{"summary":"s","decisions":[],"actions":["do it"],"questions":[]}"#).unwrap_err().contains("list of objects"));
        assert!(parse_overall(r#"{"summary":"s","decisions":[],"actions":[{"owner":"a"}],"questions":[]}"#).unwrap_err().contains("\"task\" is missing"));
        assert!(parse_overall(r#"{"summary":"s","decisions":[],"actions":[]}"#).unwrap_err().contains("\"questions\" is missing"));
        assert!(parse_overall(r#"{"summary":"s","decisions":[],"actions":[{"task":"t","due":5}],"questions":[]}"#).unwrap_err().contains("due"));
    }

    #[test]
    fn translated_chapters_keep_their_count() {
        let ok = "{\"chapters\":[{\"title\":\"শুরু\",\"summary\":\"আলোচনা\"},{\"title\":\"শেষ\",\"summary\":\"সমাপ্তি\"}]}";
        assert_eq!(parse_chapter_texts(ok, 2).unwrap()[1].0, "শেষ");
        assert!(parse_chapter_texts(ok, 3).unwrap_err().contains("exactly 3"));
        assert!(parse_chapter_texts("{\"chapters\":{}}", 1).is_err());
    }

    #[test]
    fn notes_read_as_markdown_with_timestamps() {
        let n = Notes {
            summary: "We agreed.".into(),
            decisions: vec!["Ship Friday".into()],
            actions: vec![Action { owner: "Tanvir".into(), task: "Design".into(), due: Some("Thu".into()) }],
            questions: vec![],
            chapters: vec![Chapter { start: 75.0, end: 200.0, title: "Plan".into(), summary: "Dates.".into() }],
        };
        let md = notes_markdown(&n, "en");
        assert!(md.starts_with("## Summary\n\nWe agreed.\n\n## Decisions\n\n- Ship Friday\n"));
        assert!(md.contains("- **Tanvir:** Design (Thu)") && md.contains("- **1:15** Plan — Dates."));
        assert!(!md.contains("Open questions"), "an empty list has no heading");
        assert!(notes_markdown(&n, "bn").contains("## সারসংক্ষেপ"));
    }
}
