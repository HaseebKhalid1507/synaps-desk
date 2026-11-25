//! The memory of card choices without a window ([`super`]): what "the same
//! question" is, what a choice is kept as, the store's cap, the file (its
//! format, an unreadable one, atomic and ordered writes), off reading and
//! writing nothing, and a card's recall fixed at first sight.

use std::path::PathBuf;

use cards_spec::{Answer, CardDoc, Choice, Reply};
use serde_json::{Value, json};

use super::{Asked, CAP, Recall, Store, enabled, write};

fn doc(v: Value) -> CardDoc {
    match cards_spec::parse_input(&v).expect("a card") {
        cards_spec::Input::Create { doc, .. } => doc,
        cards_spec::Input::Close(_) => unreachable!(),
    }
}

fn ship() -> Value {
    json!({"question": "Run the migration on staging now?", "wait": true,
        "detail": "It locks the tokens table for about 40 seconds.",
        "options": [{"label": "Run it now"}, {"label": "Wait for the low-traffic window", "hint": "after 22:00"}, "Don't run it"],
        "default": "Wait for the low-traffic window"})
}

fn key(cwd: &str, v: Value) -> Option<String> {
    Asked::of(cwd, &doc(v)).map(|a| a.key)
}

fn one(label: &str) -> Reply {
    Reply::Answer(Answer { choice: Some(Choice::One(label.into())), note: None, fields: None })
}

fn many(labels: &[&str]) -> Reply {
    Reply::Answer(Answer { choice: Some(Choice::Many(labels.iter().map(|l| l.to_string()).collect())), note: None, fields: None })
}

/// A folder of this test's own.
fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("synaps-desktop-recall-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("a test folder");
    d
}

const CWD: &str = "/home/you/work/relay-gateway";

/// The same question: the folder, the question (its spacing and case
/// aside), one pick or several, and the option set (in any order, any
/// case). The detail, hints, notes, default, fields and `wait` are not
/// part of it.
#[test]
fn the_same_question_is_its_folder_question_and_option_set() {
    let k = key(CWD, ship()).expect("a key");
    assert_eq!(k.len(), 16);
    assert!(k.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
    let same = [
        json!({"question": "  run the MIGRATION on staging   now?", "options": ["Don't run it", "run it now", "Wait for the low-traffic window"]}),
        json!({"question": "Run the migration on staging now?", "detail": "Other words.", "default": "Run it now",
            "options": [{"label": "Run it now", "hint": "now"}, "Wait for the low-traffic window", "Don't run it"],
            "fields": [{"id": "why", "label": "Why"}]}),
    ];
    for v in same {
        assert_eq!(key(CWD, v.clone()).as_deref(), Some(k.as_str()), "the same question: {v}");
    }
    let other = [
        (CWD, json!({"question": "Run the migration on staging now?", "options": ["Run it now", "Wait for the low-traffic window"]})),
        (
            CWD,
            json!({"question": "Run the migration on staging now?", "options": ["Run it now", "Wait for the low-traffic window", "Don't run it", "Ask Ana"]}),
        ),
        (
            CWD,
            json!({"question": "Run the migration on staging now?", "options": ["Run it today", "Wait for the low-traffic window", "Don't run it"]}),
        ),
        (
            CWD,
            json!({"question": "Run the migration on staging now?", "multi": true, "options": ["Run it now", "Wait for the low-traffic window", "Don't run it"]}),
        ),
        (
            CWD,
            json!({"question": "Run the migration on prod now?", "options": ["Run it now", "Wait for the low-traffic window", "Don't run it"]}),
        ),
        ("/home/you/work/synaps-desktop", ship()),
        ("/home/you/work/relay-gateway/", ship()),
    ];
    for (cwd, v) in other {
        assert_ne!(key(cwd, v.clone()).as_deref(), Some(k.as_str()), "another question: {cwd} {v}");
    }
    assert_eq!(key(CWD, json!({"question": "Who reviews?", "fields": [{"id": "who", "label": "Reviewer"}]})), None, "no options");
    assert_eq!(key("", ship()), None, "no folder");
}

/// A choice is kept as its options' places in the sorted label set (a
/// multi pick's as the set), and read back as the doc's options in its
/// order, whatever order it lists them in now. Nothing for a skip, no
/// choice, a label not the doc's, ids out of range, or several ids on a
/// one-pick card.
#[test]
fn a_choice_is_kept_by_option_ids_and_read_back_as_options() {
    let d = doc(ship());
    let a = Asked::of(CWD, &d).unwrap();
    // Sorted, folded: "don't run it", "run it now", "wait for …".
    assert_eq!(a.pick(&one("Run it now")), Some(vec![1]));
    assert_eq!(
        a.pick(&Reply::Change(Answer { choice: Some(Choice::One("Don't run it".into())), note: None, fields: None })),
        Some(vec![0])
    );
    assert_eq!(a.options(&[1], &d), vec![0], "Run it now is the doc's first");
    let reordered = doc(json!({"question": "Run the migration on staging now?",
        "options": ["Wait for the low-traffic window", "Don't run it", "RUN IT NOW"]}));
    let b = Asked::of(CWD, &reordered).unwrap();
    assert_eq!(b.key, a.key);
    assert_eq!(b.options(&[1], &reordered), vec![2], "the same option, wherever it is now");
    assert_eq!(a.pick(&Reply::Skip), None, "a skip keeps nothing");
    assert_eq!(a.pick(&Reply::Answer(Answer::default())), None, "no choice");
    assert_eq!(a.pick(&one("Something else")), None, "not the doc's");
    assert_eq!(a.options(&[3], &d), Vec::<usize>::new(), "out of range");
    assert_eq!(a.options(&[0, 1], &d), Vec::<usize>::new(), "several on a one-pick card");
    let suites = doc(json!({"question": "Which suites?", "multi": true, "options": ["Unit", "Integration", "Load test", "Chaos"],
        "default": ["Unit", "Integration"]}));
    let s = Asked::of(CWD, &suites).unwrap();
    // Sorted: chaos, integration, load test, unit.
    assert_eq!(s.pick(&many(&["Load test", "Chaos", "Chaos"])), Some(vec![0, 2]), "a set: sorted, once each");
    assert_eq!(s.options(&[0, 2], &suites), vec![2, 3], "in the doc's order");
    assert_eq!(s.pick(&many(&[])), None, "an empty set keeps nothing");
}

/// At most [`CAP`] questions; past it the least recently answered goes,
/// and answering one again makes it the most recent.
#[test]
fn the_store_keeps_at_most_cap_the_least_recent_dropped() {
    let k = |i: usize| format!("{i:016x}");
    let mut s = Store::default();
    for i in 0..CAP {
        s.put(k(i), vec![(i % 9) as u8]);
    }
    assert_eq!(s.len(), CAP);
    // 0 answered again: the most recent now.
    s.put(k(0), vec![4]);
    assert_eq!(s.len(), CAP);
    s.put(k(CAP), vec![1]);
    s.put(k(CAP + 1), vec![2]);
    assert_eq!(s.len(), CAP, "capped");
    assert_eq!(s.get(&k(0)), Some(&[4u8][..]), "answered again, kept, its new choice");
    assert_eq!(s.get(&k(1)), None, "the least recent went first");
    assert_eq!(s.get(&k(2)), None, "then the next");
    assert_eq!(s.get(&k(3)), Some(&[3u8][..]));
    assert_eq!(s.get(&k(CAP + 1)), Some(&[2u8][..]));
    // The file holds the same, in the same order.
    let (back, bad) = Store::parse(&s.to_text()).unwrap();
    assert_eq!((back, bad), (s.clone(), 0));
    // A file longer than the cap (written by hand) keeps its newest.
    let rows: Vec<Value> = (0..CAP + 10).map(|i| json!({"key": k(i), "pick": [1]})).collect();
    let (long, _) = Store::parse(&json!({"version": 1, "choices": rows}).to_string()).unwrap();
    assert_eq!(long.len(), CAP);
    assert!(long.get(&k(9)).is_none() && long.get(&k(10)).is_some());
}

/// The file is the format and nothing else: a version, then each key
/// (16 hex digits) with its ids.
#[test]
fn the_file_holds_keys_and_option_ids_only() {
    let mut s = Store::default();
    s.put("3f9a1c2e4b5d6f70".into(), vec![1]);
    s.put("00000000000000ff".into(), vec![0, 2, 3]);
    let text = s.to_text();
    let v: Value = serde_json::from_str(&text).expect("JSON");
    assert_eq!(
        v,
        json!({"version": 1, "choices": [{"key": "3f9a1c2e4b5d6f70", "pick": [1]}, {"key": "00000000000000ff", "pick": [0, 2, 3]}]})
    );
}

/// An unreadable file never stops the desk: it starts empty and says why
/// (the log, and [`Recall::said`]); a readable file's bad entries are left
/// out, the rest kept.
#[test]
fn an_unreadable_file_starts_empty_and_says_why() {
    let d = dir("unreadable");
    let path = d.join(super::FILE);
    let bad: [&[u8]; 7] = [
        b"{\"version\": 1, \"choices\": [",
        b"not json at all",
        b"{\"version\": 2, \"choices\": []}",
        b"{\"version\": 1, \"choices\": \"none\"}",
        b"[1, 2, 3]",
        b"\xff\xfe\x00garbage",
        b"",
    ];
    for bytes in bad {
        std::fs::write(&path, bytes).unwrap();
        let r = Recall::new(true, Some(path.clone()));
        assert_eq!(r.len(), 0, "{:?}", String::from_utf8_lossy(bytes));
        assert!(r.said.is_some(), "it says why: {:?}", String::from_utf8_lossy(bytes));
        assert_eq!(r.reads, 1);
    }
    let mixed = json!({"version": 1, "choices": [
        {"key": "3f9a1c2e4b5d6f70", "pick": [1]},
        {"key": "NOT-HEX", "pick": [1]},
        {"key": "00000000000000ff", "pick": [9]},
        {"key": "00000000000000aa", "pick": []},
        {"key": "00000000000000bb"},
        {"key": "00000000000000cc", "pick": [2, 2, 0]}]});
    std::fs::write(&path, mixed.to_string()).unwrap();
    let r = Recall::new(true, Some(path.clone()));
    assert_eq!(r.said, None);
    assert_eq!(r.len(), 2, "the two good entries");
    assert_eq!(r.store.get("00000000000000cc"), Some(&[0u8, 2][..]));
    std::fs::remove_file(&path).unwrap();
    let r = Recall::new(true, Some(path));
    assert_eq!((r.len(), r.said.as_deref()), (0, None), "no file: empty, nothing to say");
    let _ = std::fs::remove_dir_all(d);
}

/// A write is whole or nothing (a temporary file renamed over it, none
/// left behind), never an older snapshot over a newer one, and `None`
/// removes the file.
#[test]
fn writes_are_atomic_and_in_order() {
    let d = dir("writes");
    let path = d.join(super::FILE);
    let mut s = Store::default();
    s.put("3f9a1c2e4b5d6f70".into(), vec![1]);
    let newer = s.to_text();
    let g = super::GENERATION.fetch_add(10, std::sync::atomic::Ordering::SeqCst) + 10;
    write(&path, g, Some(&newer)).unwrap();
    write(&path, g - 1, Some("{\"version\": 1, \"choices\": []}\n")).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), newer, "the older snapshot never lands over the newer");
    let left: Vec<String> = std::fs::read_dir(&d)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != super::FILE)
        .collect();
    assert!(left.is_empty(), "no temporary file left: {left:?}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600, "this user's only");
    }
    write(&path, g + 1, None).unwrap();
    assert!(!path.exists(), "removed");
    write(&path, g + 2, None).unwrap();
    let _ = std::fs::remove_dir_all(d);
}

/// Off, the file is not read, nothing is recalled, and nothing is kept
/// to write; turned on, it is read then.
#[test]
fn off_reads_nothing_and_keeps_nothing() {
    let d = dir("off");
    let path = d.join(super::FILE);
    let ship = doc(ship());
    let k = Asked::of(CWD, &ship).unwrap().key;
    std::fs::write(&path, json!({"version": 1, "choices": [{"key": k, "pick": [1]}]}).to_string()).unwrap();
    let mut r = Recall::new(false, Some(path.clone()));
    assert_eq!((r.reads, r.len()), (0, 0), "not read");
    assert_eq!(r.at("s", "toolu_1", Some(CWD), &ship), Vec::<usize>::new());
    assert!(!r.remember(CWD, &ship, &one("Don't run it")), "nothing kept");
    assert!(r.snapshot().is_some_and(|(_, _, text)| text.is_none()), "and nothing to write but a removal (Forget's)");
    r.set_on(true);
    assert_eq!((r.reads, r.len()), (1, 1), "on: read now");
    assert_eq!(r.at("s", "toolu_2", Some(CWD), &ship), vec![0], "Run it now");
    r.set_on(false);
    assert_eq!((r.reads, r.len()), (1, 0), "off again: let go, not read");
    assert_eq!(r.at("s", "toolu_3", Some(CWD), &ship), Vec::<usize>::new());
    let _ = std::fs::remove_dir_all(d);
}

/// What an open card recalls is read at its first sight and kept while
/// it is open: a choice remembered meanwhile shows on the next card, not
/// on this one. Forget empties it for every card after.
#[test]
fn what_a_card_recalls_is_fixed_at_first_sight() {
    let ship = doc(ship());
    let mut r = Recall::new(true, None);
    assert_eq!(r.at("s", "toolu_1", Some(CWD), &ship), Vec::<usize>::new(), "nothing yet");
    assert!(r.remember(CWD, &ship, &one("Don't run it")));
    assert_eq!(r.at("s", "toolu_1", Some(CWD), &ship), Vec::<usize>::new(), "the card seen before: unchanged");
    assert_eq!(r.at("s", "toolu_2", Some(CWD), &ship), vec![2], "a new card: Don't run it");
    assert!(r.remember(CWD, &ship, &one("Run it now")));
    assert_eq!(r.at("s", "toolu_2", Some(CWD), &ship), vec![2], "still what it first recalled");
    assert_eq!(r.at("s", "toolu_3", Some(CWD), &ship), vec![0], "the latest choice");
    assert_eq!(r.at("s", "toolu_4", Some("/elsewhere"), &ship), Vec::<usize>::new(), "another folder");
    assert_eq!(r.at("s", "toolu_5", None, &ship), Vec::<usize>::new(), "no folder");
    r.forget();
    assert_eq!(r.len(), 0);
    assert_eq!(r.at("s", "toolu_3", Some(CWD), &ship), Vec::<usize>::new(), "forgotten, on every card");
}

/// `cards.remember`: on unless `false` or `"off"`.
#[test]
fn the_setting_is_on_unless_false_or_off() {
    assert!(enabled(None));
    assert!(enabled(Some(&json!(true))));
    assert!(enabled(Some(&json!("on"))));
    assert!(!enabled(Some(&json!(false))));
    assert!(!enabled(Some(&json!("off"))));
}
