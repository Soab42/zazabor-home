//! The service catalogue: where each chat service lives, which addresses stay inside its panel,
//! which are calls (they go to the system browser) and how its unread count shows in the tab title.
//! The icons are bundled with the page (src/modules/messages/icons.ts); nothing is fetched.

pub struct Service {
    pub id: &'static str,
    /// Addresses an account of this service may start at (Teams has two).
    pub urls: &'static [&'static str],
    /// Sites that belong to the service: a link to one stays in the panel, any other opens in the browser.
    pub domains: &'static [&'static str],
    /// Notification-hungry services start "always on", the rest sleep when unused.
    pub always_on: bool,
    /// Call and meeting pages: (host suffix, path prefix). They open in the system browser.
    pub calls: &'static [(&'static str, &'static str)],
}

pub const CUSTOM: &str = "custom";

pub const SERVICES: &[Service] = &[
    Service { id: "whatsapp", urls: &["https://web.whatsapp.com/"], domains: &["whatsapp.com", "whatsapp.net"], always_on: true, calls: &[("call.whatsapp.com", "/")] },
    Service { id: "messenger", urls: &["https://www.messenger.com/"], domains: &["messenger.com", "facebook.com", "fbcdn.net"], always_on: false, calls: &[("messenger.com", "/videochat"), ("messenger.com", "/groupcall"), ("facebook.com", "/groupcall"), ("facebook.com", "/videocall")] },
    Service {
        id: "teams",
        urls: &["https://teams.microsoft.com/", "https://teams.live.com/"],
        domains: &["microsoft.com", "microsoftonline.com", "live.com", "office.com", "office365.com", "microsoft365.com", "skype.com", "sharepoint.com", "msauth.net", "msftauth.net", "microsoftonline-p.com"],
        always_on: true,
        calls: &[("teams.microsoft.com", "/l/meetup-join"), ("teams.microsoft.com", "/meet/"), ("teams.live.com", "/meet/"), ("teams.microsoft.com", "/l/call"), ("teams.microsoft.com", "/v2/meetingjoin")],
    },
    Service { id: "telegram", urls: &["https://web.telegram.org/a/"], domains: &["telegram.org", "t.me", "telegram-cdn.org"], always_on: false, calls: &[] },
    Service { id: "instagram", urls: &["https://www.instagram.com/direct/inbox/"], domains: &["instagram.com", "cdninstagram.com", "facebook.com"], always_on: false, calls: &[] },
    Service { id: "slack", urls: &["https://app.slack.com/client"], domains: &["slack.com", "slack-edge.com"], always_on: false, calls: &[("app.slack.com", "/huddle"), ("slack.com", "/huddle")] },
    Service { id: "discord", urls: &["https://discord.com/app"], domains: &["discord.com", "discordapp.com", "discord.gg"], always_on: false, calls: &[] },
    Service { id: "google-chat", urls: &["https://chat.google.com/"], domains: &["google.com", "gstatic.com", "googleusercontent.com", "youtube.com"], always_on: false, calls: &[("meet.google.com", "/")] },
    Service { id: "google-messages", urls: &["https://messages.google.com/web"], domains: &["google.com", "gstatic.com", "googleusercontent.com"], always_on: false, calls: &[] },
    Service { id: "linkedin", urls: &["https://www.linkedin.com/messaging/"], domains: &["linkedin.com", "licdn.com"], always_on: false, calls: &[] },
];

pub fn find(id: &str) -> Option<&'static Service> {
    SERVICES.iter().find(|s| s.id == id)
}

/// Whether `service` is one we know, or the custom address.
pub fn known(id: &str) -> bool {
    id == CUSTOM || find(id).is_some()
}

pub fn host_of(url: &str) -> Option<String> {
    tauri::Url::parse(url).ok().and_then(|u| u.host_str().map(|h| h.to_ascii_lowercase()))
}

fn loopback(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "[::1]" | "::1")
}

/// A custom address: https, with a host. (A loopback address may be http: a page you run yourself.)
pub fn custom_url_ok(url: &str) -> bool {
    let Ok(parsed) = tauri::Url::parse(url) else { return false };
    let Some(host) = parsed.host_str() else { return false };
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return false;
    }
    match parsed.scheme() {
        "https" => true,
        "http" => loopback(&host.to_ascii_lowercase()),
        _ => false,
    }
}

/// The address an account may use: a catalogue address as it is, or a valid custom one.
pub fn url_ok(service: &str, url: &str) -> bool {
    match find(service) {
        Some(s) => s.urls.contains(&url),
        None => service == CUSTOM && custom_url_ok(url),
    }
}

pub fn host_in(host: &str, domain: &str) -> bool {
    host == domain || host.strip_suffix(domain).is_some_and(|rest| rest.ends_with('.'))
}

/// Whether a link to `url` from an account of `service` (starting at `home`) stays in the panel.
pub fn stays_inside(service: &str, home: &str, url: &str) -> bool {
    let Some(host) = host_of(url) else { return false };
    if let Some(s) = find(service) {
        return s.domains.iter().any(|d| host_in(&host, d));
    }
    host_of(home).is_some_and(|h| host_in(&host, &h))
}

/// Where the account's browser window goes: the address the panel shows now when it is a secure
/// page of the service itself (the open chat or channel), otherwise the account's own address.
pub fn browser_target(service: &str, home: &str, current: Option<&str>) -> String {
    match current {
        Some(url) if url.starts_with("https://") && stays_inside(service, home, url) => url.to_string(),
        _ => home.to_string(),
    }
}

/// Whether `url` is a call or meeting page of the service (opened in the system browser).
pub fn is_call(service: &str, url: &str) -> bool {
    let Ok(parsed) = tauri::Url::parse(url) else { return false };
    let Some(host) = parsed.host_str().map(str::to_ascii_lowercase) else { return false };
    let path = parsed.path();
    let generic = [("meet.google.com", "/"), ("zoom.us", "/j/"), ("whereby.com", "/")];
    let own = find(service).map(|s| s.calls).unwrap_or(&[]);
    own.iter().chain(generic.iter()).any(|(h, p)| host_in(&host, h) && path.starts_with(p))
}

/// The unread count a tab title carries: a leading "(3)" or "[3]", for some services also a
/// "(3)" elsewhere in the title, and Slack's "*" and "!" markers (unread, mention: shown as 1).
pub fn unread_from_title(service: &str, title: &str) -> u32 {
    let lead = leading_count(title);
    if lead > 0 {
        return lead;
    }
    match service {
        "teams" | "discord" | "linkedin" | "google-chat" | "instagram" | "messenger" => inner_count(title),
        "slack" => u32::from(title.starts_with("* ") || title.starts_with("! ")),
        _ => 0,
    }
}

fn leading_count(title: &str) -> u32 {
    let t = title.trim_start();
    let Some(rest) = t.strip_prefix('(').or_else(|| t.strip_prefix('[')) else { return 0 };
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() || digits.len() > 5 {
        return 0;
    }
    match rest[digits.len()..].chars().next() {
        Some(')' | ']') => digits.parse().unwrap_or(0),
        _ => 0,
    }
}

/// "Chat | (2) | Microsoft Teams": the first "(n)" after a separator or space.
fn inner_count(title: &str) -> u32 {
    title.char_indices().filter(|(_, c)| *c == '(').map(|(i, _)| leading_count(&title[i..])).find(|n| *n > 0).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_browser_window_opens_the_current_chat_on_the_service() {
        let home = "https://web.whatsapp.com";
        assert_eq!(browser_target("whatsapp", home, Some("https://web.whatsapp.com/send?phone=1")), "https://web.whatsapp.com/send?phone=1");
        assert_eq!(browser_target("whatsapp", home, Some("https://evil.example/x")), home);
        assert_eq!(browser_target("whatsapp", home, Some("http://web.whatsapp.com/")), home);
        assert_eq!(browser_target("whatsapp", home, Some("file:///etc/passwd")), home);
        assert_eq!(browser_target("whatsapp", home, None), home);
        assert_eq!(browser_target("custom", "https://chat.example.com/", Some("https://chat.example.com/room/4")), "https://chat.example.com/room/4");
    }

    #[test]
    fn titles_per_service() {
        assert_eq!(unread_from_title("whatsapp", "(3) WhatsApp"), 3);
        assert_eq!(unread_from_title("telegram", "[12] Telegram"), 12);
        assert_eq!(unread_from_title("whatsapp", "WhatsApp"), 0);
        assert_eq!(unread_from_title("teams", "Chat | (2) | Microsoft Teams"), 2);
        assert_eq!(unread_from_title("whatsapp", "Chat | (2) | x"), 0);
        assert_eq!(unread_from_title("slack", "* general - Work - Slack"), 1);
        assert_eq!(unread_from_title("slack", "general - Work - Slack"), 0);
        assert_eq!(unread_from_title("custom", "(3) Test"), 3);
        assert_eq!(unread_from_title("custom", "(x) Hi"), 0);
        assert_eq!(unread_from_title("custom", "(123456789) Hi"), 0);
    }

    #[test]
    fn urls_allowed() {
        assert!(url_ok("whatsapp", "https://web.whatsapp.com/"));
        assert!(!url_ok("whatsapp", "https://evil.example/"));
        assert!(url_ok("teams", "https://teams.live.com/"));
        assert!(url_ok("teams", "https://teams.microsoft.com/"));
        assert!(url_ok("custom", "https://chat.example.org/app"));
        assert!(!url_ok("custom", "http://chat.example.org/"));
        assert!(!url_ok("custom", "file:///etc/passwd"));
        assert!(!url_ok("custom", "javascript:alert(1)"));
        assert!(!url_ok("custom", "https://user:pw@example.org/"));
        assert!(url_ok("custom", "http://127.0.0.1:8080/"));
        assert!(!url_ok("nonsense", "https://example.org/"));
    }

    #[test]
    fn links_inside_or_out() {
        assert!(stays_inside("teams", "https://teams.microsoft.com/", "https://login.microsoftonline.com/x"));
        assert!(!stays_inside("teams", "https://teams.microsoft.com/", "https://evilmicrosoft.com/"));
        assert!(!stays_inside("whatsapp", "https://web.whatsapp.com/", "https://example.org/"));
        assert!(stays_inside("custom", "https://chat.example.org/", "https://a.chat.example.org/x"));
        assert!(!stays_inside("custom", "https://chat.example.org/", "https://example.org/"));
    }

    #[test]
    fn calls_are_found() {
        assert!(is_call("teams", "https://teams.microsoft.com/l/meetup-join/abc"));
        assert!(is_call("whatsapp", "https://call.whatsapp.com/video/xyz"));
        assert!(is_call("messenger", "https://www.messenger.com/videochat/1"));
        assert!(is_call("slack", "https://meet.google.com/abc-defg-hij"));
        assert!(!is_call("whatsapp", "https://web.whatsapp.com/"));
    }
}
