//! The alerts on screen: calls that ring until they end, and message toasts that fade. The queue is
//! plain data (no window, no clock of its own), so what stays, what expires and what folds into
//! "+N more" is tested without the app.

use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

/// Cards shown at once (calls first, then the newest toasts).
pub const MAX_VISIBLE: usize = 4;
/// A call from a web chat that nobody answered or declined goes away after this.
pub const WEB_CALL_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Call,
    Message,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Alert {
    pub id: u64,
    pub kind: Kind,
    /// Replaces the alert with the same key (one ringing call per source); empty never replaces.
    pub key: String,
    /// Which icon: "phone", "sms", or a Messages service id ("whatsapp", "teams", ...).
    pub service: String,
    pub title: String,
    pub body: String,
    pub video: bool,
    /// Unix ms.
    pub at: i64,
    pub account_id: Option<String>,
    pub account_name: Option<String>,
    pub phone_id: Option<String>,
    pub number: Option<String>,
    pub thread_id: Option<Value>,
    pub addresses: Vec<String>,
    /// How a call reaches the PC: "hfp" (Bluetooth), "app" (the Link app) or "web" (a Messages account).
    pub via: String,
    /// A line under the card (what to do next).
    pub note: Option<String>,
    /// A call that rings without the tone (Do not disturb, a muted account, the tone off).
    pub silent: bool,
    pub can_reply: bool,
    pub can_call: bool,
    pub can_read: bool,
    /// Made by the demo trigger: its buttons never reach a phone.
    pub demo: bool,
    #[serde(skip)]
    pub ttl: Duration,
    #[serde(skip)]
    pub expires: Option<Instant>,
}

impl Alert {
    pub fn new(kind: Kind, service: &str, title: &str, body: &str) -> Alert {
        Alert {
            id: 0,
            kind,
            key: String::new(),
            service: service.into(),
            title: title.into(),
            body: body.into(),
            video: false,
            at: 0,
            account_id: None,
            account_name: None,
            phone_id: None,
            number: None,
            thread_id: None,
            addresses: Vec::new(),
            via: String::new(),
            note: None,
            silent: false,
            can_reply: false,
            can_call: false,
            can_read: false,
            demo: false,
            ttl: Duration::ZERO,
            expires: None,
        }
    }

    /// The same card as `other`, apart from its id and clock (an update that needn't redraw).
    fn same_card(&self, other: &Alert) -> bool {
        let mut a = other.clone();
        a.id = self.id;
        a.at = self.at;
        a.expires = self.expires;
        serde_json::to_value(self).ok() == serde_json::to_value(&a).ok()
    }
}

#[derive(Default)]
pub struct Queue {
    items: Vec<Alert>,
    next: u64,
}

/// What the window draws: the cards in order (oldest first) and how many more are waiting.
pub struct View<'a> {
    pub cards: Vec<&'a Alert>,
    pub hidden: usize,
}

impl Queue {
    /// Adds an alert, or updates the one with its key. Returns its id and whether anything the
    /// window shows changed.
    pub fn push(&mut self, mut alert: Alert, now: Instant) -> (u64, bool) {
        alert.expires = match alert.kind {
            Kind::Message => Some(now + alert.ttl),
            Kind::Call => (alert.via == "web").then_some(now + WEB_CALL_TIMEOUT),
        };
        if !alert.key.is_empty() {
            if let Some(old) = self.items.iter_mut().find(|a| a.key == alert.key) {
                let changed = !old.same_card(&alert);
                alert.id = old.id;
                // A ringing call keeps ringing from when it began.
                alert.at = old.at;
                if !changed && alert.via != "web" {
                    alert.expires = old.expires;
                }
                *old = alert;
                return (old.id, changed);
            }
        }
        self.next += 1;
        alert.id = self.next;
        let id = alert.id;
        self.items.push(alert);
        (id, true)
    }

    pub fn get(&self, id: u64) -> Option<&Alert> {
        self.items.iter().find(|a| a.id == id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Alert> {
        self.items.iter_mut().find(|a| a.id == id)
    }

    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.items.len();
        self.items.retain(|a| a.id != id);
        self.items.len() != before
    }

    pub fn remove_key(&mut self, key: &str) -> bool {
        let before = self.items.len();
        self.items.retain(|a| a.key != key);
        self.items.len() != before
    }

    /// Removes the call with `key` only when it came `via` that way.
    pub fn remove_key_via(&mut self, key: &str, via: &str) -> bool {
        let before = self.items.len();
        self.items.retain(|a| !(a.key == key && a.via == via));
        self.items.len() != before
    }

    pub fn find_key(&self, key: &str) -> Option<&Alert> {
        self.items.iter().find(|a| a.key == key)
    }

    /// The pointer is on a toast (it stays) or left it (it goes soon after).
    pub fn hover(&mut self, id: u64, on: bool, now: Instant) -> bool {
        let Some(alert) = self.get_mut(id) else { return false };
        if alert.kind != Kind::Message {
            return false;
        }
        alert.expires = if on { None } else { Some(now + (alert.ttl / 2).max(Duration::from_secs(2))) };
        true
    }

    /// Drops what has run out; the ids dropped.
    pub fn sweep(&mut self, now: Instant) -> Vec<u64> {
        let gone: Vec<u64> = self.items.iter().filter(|a| a.expires.is_some_and(|t| t <= now)).map(|a| a.id).collect();
        self.items.retain(|a| !gone.contains(&a.id));
        gone
    }

    pub fn calls(&self) -> impl Iterator<Item = &Alert> {
        self.items.iter().filter(|a| a.kind == Kind::Call)
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Calls first (they never fold away), the newest toasts in what room is left; shown oldest
    /// first, so the newest card sits at the screen edge the stack grows from.
    pub fn view(&self, max: usize) -> View<'_> {
        let max = max.max(1);
        let mut chosen: Vec<&Alert> = self.items.iter().filter(|a| a.kind == Kind::Call).take(max).collect();
        let room = max - chosen.len();
        let toasts: Vec<&Alert> = self.items.iter().filter(|a| a.kind == Kind::Message).collect();
        chosen.extend(toasts.iter().skip(toasts.len().saturating_sub(room)).take(room));
        chosen.sort_by_key(|a| a.id);
        let hidden = self.items.len() - chosen.len();
        View { cards: chosen, hidden }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toast(title: &str) -> Alert {
        let mut a = Alert::new(Kind::Message, "sms", title, "hi");
        a.ttl = Duration::from_secs(8);
        a
    }

    fn call(key: &str, via: &str) -> Alert {
        let mut a = Alert::new(Kind::Call, "phone", "Ann", "Incoming call");
        a.key = key.into();
        a.via = via.into();
        a
    }

    #[test]
    fn five_rapid_messages_show_four_and_fold_one() {
        let mut q = Queue::default();
        let now = Instant::now();
        for n in 1..=5 {
            q.push(toast(&format!("m{n}")), now);
        }
        let view = q.view(MAX_VISIBLE);
        assert_eq!(view.cards.iter().map(|a| a.title.as_str()).collect::<Vec<_>>(), ["m2", "m3", "m4", "m5"]);
        assert_eq!(view.hidden, 1);
    }

    #[test]
    fn calls_keep_their_place_among_toasts() {
        let mut q = Queue::default();
        let now = Instant::now();
        q.push(call("call:phone", "hfp"), now);
        for n in 1..=5 {
            q.push(toast(&format!("m{n}")), now);
        }
        let view = q.view(MAX_VISIBLE);
        assert_eq!(view.cards.len(), 4);
        assert_eq!(view.cards[0].kind, Kind::Call);
        assert_eq!(view.hidden, 2);
    }

    #[test]
    fn a_call_with_the_same_key_updates_in_place() {
        let mut q = Queue::default();
        let now = Instant::now();
        let (id, changed) = q.push(call("call:phone", "hfp"), now);
        assert!(changed);
        let (again, changed) = q.push(call("call:phone", "hfp"), now + Duration::from_secs(1));
        assert_eq!((id, changed, q.len()), (again, false, 1));
        let mut renamed = call("call:phone", "hfp");
        renamed.title = "Bob".into();
        assert!(q.push(renamed, now).1);
        assert_eq!(q.get(id).unwrap().title, "Bob");
    }

    #[test]
    fn toasts_expire_unless_hovered() {
        let mut q = Queue::default();
        let now = Instant::now();
        let (a, _) = q.push(toast("a"), now);
        let (b, _) = q.push(toast("b"), now);
        assert!(q.hover(b, true, now));
        assert!(q.sweep(now + Duration::from_secs(7)).is_empty());
        assert_eq!(q.sweep(now + Duration::from_secs(9)), vec![a]);
        // Leaving a toast gives it a few more seconds.
        q.hover(b, false, now + Duration::from_secs(20));
        assert!(q.sweep(now + Duration::from_secs(23)).is_empty());
        assert_eq!(q.sweep(now + Duration::from_secs(25)), vec![b]);
    }

    #[test]
    fn phone_calls_ring_on_and_web_calls_time_out() {
        let mut q = Queue::default();
        let now = Instant::now();
        let (phone, _) = q.push(call("call:phone", "hfp"), now);
        let (web, _) = q.push(call("call:whatsapp-1", "web"), now);
        assert_eq!(q.sweep(now + Duration::from_secs(61)), vec![web]);
        assert!(q.get(phone).is_some());
    }

    #[test]
    fn a_call_is_removed_only_by_the_way_it_came() {
        let mut q = Queue::default();
        let now = Instant::now();
        q.push(call("call:phone", "app"), now);
        assert!(!q.remove_key_via("call:phone", "hfp"));
        assert!(q.remove_key_via("call:phone", "app"));
        assert!(q.is_empty());
    }
}
