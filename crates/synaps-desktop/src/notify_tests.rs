//! D3's notification boundary (Shady #83 P2-5, P2-6, R2-3): the text the
//! OS is given, and the notifications as owned, bounded resources, with a
//! fake transport and with GPUI's (the window's teardown). Linux's bus
//! transport has its own: `notify_xdg_tests.rs`.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::TestAppContext;

use super::*;
// `super`'s `gpui_kit::*` brings the kit's `test` too: the std one here.
use core::prelude::v1::test;

fn key(n: usize) -> Key {
    Key { session: format!("s{}", n % 3), tool_id: format!("toolu_{n}"), card: cards_spec::CardId::parse(&format!("c{n:06}")).unwrap() }
}

/// The payload: the session's name and the question each one line, no
/// controls, no bidi or default-ignorable marks, bounded; an empty name
/// says "a session". Markup is text: kept as typed for macOS and Windows
/// (they draw text), escaped for a Linux server that reads markup, left
/// for one that doesn't.
#[test]
fn the_os_gets_one_bounded_line_of_plain_text() {
    let k = key(1);
    let n = note(&k, "token\u{202E}store\nIGNORE\u{200B}", "Ship <b>it</b> & \u{2067}now\u{2069}?\nsecond line");
    assert_eq!(n.title, "Needs you · tokenstore IGNORE");
    assert!(!n.title.contains('\n') && !n.body.contains('\n'));
    assert!(
        n.title.chars().chain(n.body.chars()).all(|c| !c.is_control()
            && !('\u{202A}'..='\u{202E}').contains(&c)
            && !('\u{2066}'..='\u{2069}').contains(&c)
            && c != '\u{200B}')
    );
    assert_eq!(n.body, "Ship <b>it</b> & now? second line", "literal markup is text");
    assert_eq!(n.tag, tag_of(&k));
    assert!(n.tag.len() <= 64);
    let long = note(&k, &"x".repeat(10_000), &"q".repeat(10_000));
    assert!(long.title.chars().count() <= "Needs you · ".chars().count() + LABEL_MAX, "{}", long.title.len());
    assert!(long.body.chars().count() <= cards_spec::QUESTION_MAX);
    assert_eq!(note(&k, " \u{202E}\n ", "q").title, "Needs you · a session");
    assert_eq!(label("\u{1F600} relay"), "\u{1F600} relay", "text, emoji and all, stays");
    // Linux's body.
    assert_eq!(xdg_body(&n.body, true), "Ship &lt;b&gt;it&lt;/b&gt; &amp; now? second line");
    assert_eq!(xdg_body("a & b", false), "a & b", "a server without body-markup shows text as sent");
    assert_eq!(xdg_body("&amp;", true), "&amp;amp;", "escaped once, as typed");
    // A binding's tag: the same for the same binding, another for another.
    assert_eq!(tag_of(&key(1)), tag_of(&key(1)));
    assert_ne!(tag_of(&key(1)), tag_of(&Key { card: cards_spec::CardId::parse("czzzzzz").unwrap(), ..key(1) }));
}

/// Shady #89 N3. A Linux summary holds nothing a server takes for a link
/// (dunst offers to open a summary's `<a href>`, `<img src>`, and any
/// `http(s)://`, `ftp(s)://`, `news://`, `file://`, `mailto:` or `www.`
/// text): a word joiner after every `:`, `.` and `<`, the text otherwise
/// as it was.
#[test]
fn a_linux_summary_offers_no_link() {
    let n = note(&key(1), r#"<a href="https://example.com">release</a>"#, "q");
    assert_eq!(n.title, r#"Needs you · <a href="https://example.com">release</a>"#, "a session's name as typed");
    for t in [n.title.as_str(), "WWW.Example.org <IMG src=x> MAILTO:a@b file:///etc news://n ftps://f"] {
        let s = xdg_summary(t);
        assert_eq!(s.replace('\u{2060}', ""), t, "the same text");
        let low = s.to_lowercase();
        for pat in ["<a", "<img", "://", "www.e", "mailto:a"] {
            assert!(!low.contains(pat), "{pat} in {s:?}");
        }
        assert!(s.char_indices().all(|(i, c)| !matches!(c, ':' | '.' | '<') || s[i + c.len_utf8()..].starts_with('\u{2060}')), "{s:?}");
    }
}

/// A transport that remembers.
#[derive(Default)]
struct Fake {
    log: Rc<RefCell<Vec<String>>>,
    up: Rc<RefCell<Vec<String>>>,
}

impl Transport for Fake {
    fn show(&mut self, n: Note, _: &mut App) {
        self.log.borrow_mut().push(format!("show {}", n.tag));
        self.up.borrow_mut().push(n.tag);
    }

    fn close(&mut self, tag: &str, _: &mut App) {
        self.log.borrow_mut().push(format!("close {tag}"));
        self.up.borrow_mut().retain(|t| t != tag);
    }

    fn retire(&mut self, _: &mut App) -> Gone {
        for t in self.up.borrow_mut().drain(..) {
            self.log.borrow_mut().push(format!("close {t}"));
        }
        Gone(None)
    }
}

/// Notifications are owned: each card's taken down when it resolves
/// (reconcile), a stale one retired once; a card told once; never more
/// than `NOTES_MAX` up, the oldest taken down first. A thousand
/// post-and-resolve cycles leave nothing up and nothing remembered.
#[gpui_kit::test]
fn notifications_are_owned_and_bounded(cx: &mut TestAppContext) {
    let (log, up) = (Rc::new(RefCell::new(Vec::new())), Rc::new(RefCell::new(Vec::new())));
    let mut notes = Notes::new(Box::new(Fake { log: log.clone(), up: up.clone() }), None);
    cx.update(|cx| {
        let k = key(1);
        notes.post(k.clone(), note(&k, "s", "q"), cx);
        notes.post(k.clone(), note(&k, "s", "q"), cx);
        assert_eq!(*log.borrow(), [format!("show {}", tag_of(&k))], "once");
        assert_eq!(notes.card(&tag_of(&k)), Some(&k));
        notes.reconcile(|_| true, cx);
        assert_eq!(notes.len(), 1, "still open: still up");
        notes.reconcile(|_| false, cx);
        assert_eq!((notes.len(), up.borrow().len()), (0, 0), "resolved: taken down");
        assert!(!notes.retire(&tag_of(&k), cx), "taken down once");
        // Past the cap: the oldest goes first.
        for n in 0..NOTES_MAX + 5 {
            notes.post(key(n), note(&key(n), "s", "q"), cx);
        }
        assert_eq!((notes.len(), up.borrow().len()), (NOTES_MAX, NOTES_MAX));
        assert!(notes.card(&tag_of(&key(4))).is_none() && notes.card(&tag_of(&key(5))).is_some(), "the 5 oldest taken down");
        // Cycles: post, resolve.
        for n in 100..1100 {
            notes.post(key(n), note(&key(n), "s", "q"), cx);
            let keep = key(n + 1);
            notes.reconcile(|k| *k == keep, cx);
        }
        assert_eq!((notes.len(), up.borrow().len()), (0, 0), "nothing left up");
        let (shows, closes) = log.borrow().iter().fold((0, 0), |(s, c), l| if l.starts_with("show") { (s + 1, c) } else { (s, c + 1) });
        assert_eq!(shows, closes, "every notification shown was taken down");
    });
}

/// Shady #83 R2-3, GPUI's transport: a note is handed to the platform
/// from a task of its own (never in place); one taken down before that is
/// never shown; one shown is dismissed when its card resolves. The window
/// gone: every one up dismissed there and then, one still queued never
/// shown, nothing posted after, and nothing left registered.
#[gpui_kit::test]
fn gpui_notes_go_with_their_window(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_app_identity("synaps-desktop", "Synaps"));
    let mut notes = Notes::new(Box::new(Gpui::default()), None);
    let tag = |n: usize| tag_of(&key(n));
    let shown = |cx: &mut TestAppContext| cx.shown_system_notifications().iter().map(|n| n.tag.to_string()).collect::<Vec<_>>();
    cx.update(|cx| notes.post(key(1), note(&key(1), "s", "q"), cx));
    assert!(shown(cx).is_empty(), "queued, not shown in place");
    cx.run_until_parked();
    assert_eq!(shown(cx), [tag(1)]);
    cx.update(|cx| {
        notes.post(key(2), note(&key(2), "s", "q"), cx);
        notes.post(key(3), note(&key(3), "s", "q"), cx);
        notes.retire(&tag(2), cx);
    });
    cx.run_until_parked();
    assert_eq!(shown(cx), [tag(1), tag(3)], "2 resolved while queued: never shown");
    assert!(cx.dismissed_system_notifications().is_empty());
    // One platform call per turn of the UI thread (R2-8).
    cx.update(|cx| {
        for n in 6..9 {
            notes.post(key(n), note(&key(n), "s", "q"), cx);
        }
    });
    assert!(cx.executor().tick(), "the task's first turn");
    assert_eq!(shown(cx).len(), 3, "one shown per turn, not three");
    cx.run_until_parked();
    assert_eq!(shown(cx).len(), 5);
    cx.update(|cx| (6..9).for_each(|n| _ = notes.retire(&tag(n), cx)));
    cx.run_until_parked();
    assert_eq!(cx.dismissed_system_notifications().len(), 3, "dismissed with their cards");
    cx.update(|cx| {
        notes.post(key(4), note(&key(4), "s", "q"), cx);
        let _ = notes.teardown(cx);
        notes.post(key(5), note(&key(5), "s", "q"), cx);
    });
    let mut dismissed: Vec<String> = cx.dismissed_system_notifications().iter().skip(3).map(|t| t.to_string()).collect();
    dismissed.sort();
    let mut up = vec![tag(1), tag(3)];
    up.sort();
    assert_eq!(dismissed, up, "every one up dismissed at the teardown");
    cx.run_until_parked();
    let shown_now = shown(cx);
    assert!(!shown_now.contains(&tag(4)) && !shown_now.contains(&tag(5)), "4 (queued) and 5 (after) never shown");
    assert!(cx.delivered_system_notifications().is_empty(), "nothing left registered");
    assert_eq!(notes.len(), 0);
}
