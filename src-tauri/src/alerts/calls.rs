//! Telling an incoming call from an ordinary message in what a chat web app reports: its
//! notification text. The phrases are one table per service (inject.js has the matching table for
//! the pages' own call dialogs). Conservative on purpose: a chat line that merely mentions a call
//! ("shall we call tomorrow?") is a message, so only whole call phrases count.

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Detected {
    Ringing { name: String, video: bool },
    Missed { name: String, video: bool },
    Ended,
}

/// What each service's notifications say while a call rings (matched anywhere in the body), and
/// whether that means video.
const RING: &[(&str, &[(&str, bool)])] = &[
    ("whatsapp", &[("incoming voice call", false), ("incoming video call", true), ("incoming call", false)]),
    ("teams", &[("is calling you", false), ("calling you", false), ("incoming call", false), ("incoming video call", true)]),
    ("messenger", &[("is calling you", false), ("incoming video chat", true), ("incoming video call", true), ("incoming call", false), ("incoming voice call", false), ("wants to video chat", true)]),
];

/// Phrases any service may use, matched only when they are the whole body (or the whole title).
const GENERIC: &[(&str, bool)] = &[
    ("incoming call", false),
    ("incoming voice call", false),
    ("incoming audio call", false),
    ("incoming video call", true),
    ("incoming group call", false),
    ("incoming group video call", true),
    ("incoming video chat", true),
    ("is calling you", false),
    ("is calling", false),
    ("calling you", false),
    ("is video calling you", true),
];

fn normalise(text: &str) -> String {
    text.trim().trim_end_matches(['.', '!', '…']).trim().to_lowercase()
}

/// "Ann is calling you" -> Ann.
fn name_before(body: &str, phrase: &str) -> Option<String> {
    let lower = body.to_lowercase();
    let at = lower.find(phrase)?;
    let name = body.get(..at)?.trim();
    (!name.is_empty() && name.chars().count() <= 60).then(|| name.to_string())
}

/// The caller named in a title like "Incoming call from Ann".
fn name_after_from(title: &str) -> Option<String> {
    let lower = title.to_lowercase();
    let at = lower.find(" from ")?;
    let name = title.get(at + 6..)?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

pub fn detect(service: &str, title: &str, body: &str, _tag: &str) -> Option<Detected> {
    let (t, b) = (normalise(title), normalise(body));
    let video = t.contains("video") || b.contains("video");
    // The call is over without being answered.
    if b.starts_with("missed ") && b.contains("call") || t.starts_with("missed ") && t.contains("call") {
        return Some(Detected::Missed { name: if t.starts_with("missed") { name_after_from(title).unwrap_or_default() } else { title.trim().to_string() }, video });
    }
    if matches!(b.as_str(), "call ended" | "the call has ended" | "call declined" | "call cancelled" | "call canceled") {
        return Some(Detected::Ended);
    }
    // The title says it ("Incoming call from Ann", "Incoming video call").
    if t.starts_with("incoming ") && (t.contains("call") || t.contains("chat")) && t.split_whitespace().count() <= 6 {
        let name = name_after_from(title).or_else(|| (!body.trim().is_empty()).then(|| body.trim().to_string())).unwrap_or_default();
        return Some(Detected::Ringing { name, video });
    }
    // The service's own phrases anywhere in a short body.
    if b.chars().count() <= 80 {
        let own = RING.iter().find(|(s, _)| *s == service).map(|(_, p)| *p).unwrap_or(&[]);
        let fits = |p: &str| b == p || (p.starts_with("incoming") && b.contains(p)) || (p.starts_with("is ") && b.ends_with(p) && b.starts_with(&t));
        if let Some((phrase, v)) = own.iter().find(|(p, _)| fits(p)) {
            let name = name_before(body, phrase).unwrap_or_else(|| title.trim().to_string());
            return Some(Detected::Ringing { name, video: *v || video });
        }
    }
    // Any service: the whole body is a call phrase.
    if let Some((_, v)) = GENERIC.iter().find(|(p, _)| b == *p) {
        return Some(Detected::Ringing { name: title.trim().to_string(), video: *v || video });
    }
    // "Ann is calling you": the phrase ends a short body.
    for (phrase, v) in GENERIC.iter().filter(|(p, _)| p.starts_with("is ") && p.ends_with("calling you") || *p == "calling you") {
        if b.ends_with(phrase) && b.starts_with(&t) && b.split_whitespace().count() <= 8 {
            return Some(Detected::Ringing { name: name_before(body, phrase).unwrap_or_else(|| title.trim().to_string()), video: *v || video });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring(name: &str, video: bool) -> Option<Detected> {
        Some(Detected::Ringing { name: name.into(), video })
    }

    #[test]
    fn whatsapp_calls() {
        assert_eq!(detect("whatsapp", "Ann", "Incoming voice call", ""), ring("Ann", false));
        assert_eq!(detect("whatsapp", "Ann", "Incoming video call", ""), ring("Ann", true));
        assert_eq!(detect("whatsapp", "Ann", "Missed voice call", ""), Some(Detected::Missed { name: "Ann".into(), video: false }));
    }

    #[test]
    fn teams_calls() {
        assert_eq!(detect("teams", "Ann Lee", "Ann Lee is calling you", ""), ring("Ann Lee", false));
        assert_eq!(detect("teams", "Incoming call from Bob", "", ""), ring("Bob", false));
        assert_eq!(detect("teams", "Bob", "Calling you…", ""), ring("Bob", false));
    }

    #[test]
    fn messenger_calls() {
        assert_eq!(detect("messenger", "Cy", "is calling you.", ""), ring("Cy", false));
        assert_eq!(detect("messenger", "Cy", "Incoming video chat", ""), ring("Cy", true));
    }

    #[test]
    fn other_services_need_the_whole_phrase() {
        assert_eq!(detect("custom", "Ann", "Incoming call", ""), ring("Ann", false));
        assert_eq!(detect("slack", "Dee", "Dee is calling you", ""), ring("Dee", false));
    }

    #[test]
    fn chat_lines_are_not_calls() {
        assert_eq!(detect("whatsapp", "Ann", "Shall we call tomorrow?", ""), None);
        assert_eq!(detect("teams", "Ann", "I was calling you all day but you did not pick up, where are you now", ""), None);
        assert_eq!(detect("custom", "Ann", "Hello", ""), None);
        assert_eq!(detect("telegram", "Ann", "the incoming call was loud", ""), None);
    }

    #[test]
    fn calls_that_ended() {
        assert_eq!(detect("teams", "Ann", "Call ended", ""), Some(Detected::Ended));
    }
}
