//! D3's notification boundary (Shady #83 P2-5, P2-6): the text the OS is
//! given, and the notifications as owned, bounded resources, with a fake
//! transport and the XDG task's memory without a bus.

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

/// The XDG task's memory (Linux): each tag's server id, at most
/// `NOTES_MAX`; a tag shown again replaces its own id; a server that
/// closes one (expired, dismissed, clicked) is heard; one that dies
/// without a word (nothing closes) still leaves at most `NOTES_MAX`
/// remembered, the oldest closed first; the window gone, every one is
/// closed.
#[test]
fn the_xdg_memory_is_bounded_without_a_server() {
    let mut s = Shown::default();
    assert!(s.shown("a".into(), 1).is_empty());
    assert_eq!((s.id("a"), s.tag(1)), (Some(1), Some("a")));
    assert!(s.shown("a".into(), 2).is_empty(), "replaced");
    assert_eq!((s.id("a"), s.tag(1), s.len()), (Some(2), None, 1));
    s.closed(2);
    assert_eq!((s.id("a"), s.len()), (None, 0), "the server closed it");
    assert_eq!(s.forget("a"), None);
    // A server gone silent: thousands shown, none closed by it.
    let mut closed = Vec::new();
    for n in 0..5000u32 {
        closed.extend(s.shown(format!("t{n}"), n + 10));
        assert!(s.len() <= NOTES_MAX);
    }
    assert_eq!(closed.len(), 5000 - NOTES_MAX, "each pushed out is closed");
    assert_eq!(closed[0], 10, "the oldest first");
    assert_eq!(s.forget(&format!("t{}", 4999)), Some(5009), "a card resolved: its id, to close");
    assert_eq!(s.drain().len(), NOTES_MAX - 1, "the window gone: every one closed");
    assert_eq!(s.len(), 0);
}
