//! The memory of card choices without a window ([`super`]): what "the same
//! question" is, what a choice is kept as, the store's cap, the file (its
//! format, an unreadable one, atomic and ordered writes, this user's only,
//! a Forget no save undoes), off reading and writing nothing, and a card's
//! recall fixed at first sight, for as long as it is open.
//!
//! Shady #93 round 2: a Forget not yet done, or whose removal failed,
//! bars every load (no read: empty) until the file is confirmed gone, and
//! the next save retries it first (R2 P2-1).

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use cards_spec::{Answer, CardDoc, CardId, Choice, Reply};
use serde_json::{Value, json};

use super::{Asked, CAP, Recall, Sight, Store, accept_forget, enabled, remove, save, save_then};

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

/// Card `tool` of session `s` (its id made from the call's).
fn sight(s: &str, tool: &str) -> Sight {
    let n: u32 = tool.bytes().map(u32::from).sum();
    (s.into(), tool.into(), CardId::parse(&format!("c{n:06}")).unwrap())
}

/// Every card still open.
fn open(_: &Sight) -> bool {
    true
}

/// A save free to go.
fn go() -> AtomicBool {
    AtomicBool::new(false)
}

/// A snapshot's number, newer than every one before.
fn generation() -> u64 {
    super::next_generation()
}

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
/// left behind), this user's only, never an older snapshot over a newer
/// one; a removal removes it.
#[test]
fn writes_are_atomic_and_in_order() {
    let d = dir("writes");
    let path = d.join(super::FILE);
    let mut s = Store::default();
    s.put("3f9a1c2e4b5d6f70".into(), vec![1]);
    let newer = s.to_text();
    let older = generation();
    let g = generation();
    assert!(save(&path, g, &newer, &go()).unwrap());
    assert!(!save(&path, older, "{\"version\": 1, \"choices\": []}\n", &go()).unwrap());
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
    remove(&path, generation()).unwrap();
    assert!(!path.exists(), "removed");
    remove(&path, generation()).unwrap();
    let _ = std::fs::remove_dir_all(d);
}

/// Unix: the temporary file is created, never opened: a link left where it
/// goes is replaced, never followed, and what it points at is untouched.
#[cfg(unix)]
#[test]
fn a_link_in_the_temporary_files_place_is_never_followed() {
    use std::os::unix::fs::PermissionsExt as _;
    let d = dir("link");
    let path = d.join(super::FILE);
    let victim = d.join("victim");
    std::fs::write(&victim, "untouched").unwrap();
    std::fs::set_permissions(&victim, std::fs::Permissions::from_mode(0o644)).unwrap();
    let g = generation();
    let tmp = path.with_extension(format!("json.{}.{g}.tmp", std::process::id()));
    std::os::unix::fs::symlink(&victim, &tmp).unwrap();
    assert!(save(&path, g, "{\"version\": 1, \"choices\": []}\n", &go()).unwrap());
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "untouched");
    assert_eq!(std::fs::metadata(&victim).unwrap().permissions().mode() & 0o777, 0o644);
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    assert!(!std::fs::symlink_metadata(&path).unwrap().file_type().is_symlink());
    let _ = std::fs::remove_dir_all(d);
}

/// Windows (Shady #93 P2-3): in a folder open to Everyone, inherited by
/// what is created there, the temporary file and the file a save leaves
/// both carry a protected DACL of one entry, the current user's SID, full
/// access; nothing inherited. A file made there as std makes one (as the
/// save did before) inherits Everyone: the check tells them apart.
#[cfg(windows)]
#[test]
fn on_windows_the_file_is_this_users_only() {
    use super::win::{Descriptor, acl, private_sddl, user_sid};
    let d = dir("acl");
    let sid = user_sid().expect("this user's SID");
    acl::set_dacl(&d, &format!("D:P(A;OICI;FA;;;WD)(A;OICI;FA;;;{sid})")).expect("an open folder");
    // A DACL as SDDL: its flags (`P`: protected) and its entries.
    let parts = |s: &str| -> (String, String) {
        let s = s.strip_prefix("D:").unwrap_or_else(|| panic!("a DACL: {s}"));
        let at = s.find('(').unwrap_or(s.len());
        (s[..at].to_string(), s[at..].to_string())
    };
    // The entries wanted, as Windows writes them back: one, allowing full
    // access to this user's SID, written as the SID or as its alias (the
    // built-in Administrator's is `LA`).
    let want = parts(&acl::sddl_of(Descriptor::of(&private_sddl().unwrap()).unwrap().0).unwrap()).1;
    assert_eq!(want.matches('(').count(), 1, "{want}");
    assert!(want.starts_with("(A;;FA;;;") && !want.contains(";WD)"), "{want}");
    let private = |p: &std::path::Path| {
        let got = acl::dacl(p).unwrap();
        let (flags, aces) = parts(&got);
        flags.contains('P') && aces == want
    };
    let plain = d.join("plain.json");
    std::fs::write(&plain, "{}").unwrap();
    let inherited = acl::dacl(&plain).unwrap();
    assert!(inherited.contains(";WD)"), "std's file inherits the folder's Everyone: {inherited}");
    assert!(!private(&plain), "the check tells it apart: {inherited}");
    let tmp = d.join("t.json.tmp");
    drop(super::create_private(&tmp).expect("created"));
    assert!(private(&tmp), "the temporary file, as created: {}", acl::dacl(&tmp).unwrap());
    assert!(super::create_private(&tmp).is_err(), "created new, never an existing file opened");
    let path = d.join(super::FILE);
    std::fs::write(&path, "an older file, inherited").unwrap();
    assert!(save(&path, generation(), "{\"version\": 1, \"choices\": []}\n", &go()).unwrap());
    assert!(private(&path), "the file, renamed over the older one: {}", acl::dacl(&path).unwrap());
    let _ = std::fs::remove_dir_all(d);
}

/// A Forget accepted while a save is mid-write (Shady #93 P2-1): its
/// removal waits for that write, then removes the file; a save numbered
/// before the Forget that comes later never writes, even when the removal
/// failed.
#[test]
fn a_forget_waits_for_a_save_mid_write_and_no_older_save_lands_after() {
    use std::sync::mpsc::channel;
    let d = dir("forget-race");
    let path = d.join(super::FILE);
    let text = "{\"version\": 1, \"choices\": []}\n";
    let (waiting, late) = (generation(), generation());
    let mid = generation();
    let (inside, at) = (channel(), channel::<()>());
    let p = path.clone();
    let writer = std::thread::spawn(move || {
        save_then(&p, mid, text, &go(), || {
            inside.0.send(()).unwrap();
            at.1.recv().unwrap();
        })
    });
    inside.1.recv().unwrap();
    // The save holds its write; the Forget is accepted all the same.
    let forget = accept_forget(&path);
    let p = path.clone();
    let removal = std::thread::spawn(move || remove(&p, forget));
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert!(!removal.is_finished(), "the removal waits for the write");
    at.0.send(()).unwrap();
    assert!(writer.join().unwrap().unwrap(), "the save mid-write finished");
    removal.join().unwrap().unwrap();
    assert!(!path.exists(), "then the removal: gone");
    for g in [waiting, late] {
        assert!(!save(&path, g, text, &go()).unwrap(), "a save from before the Forget never writes");
    }
    assert!(!path.exists());
    // A removal that fails still bars every older save.
    std::fs::create_dir_all(path.join("in-the-way")).unwrap();
    let before = generation();
    let forget = accept_forget(&path);
    assert!(remove(&path, forget).is_err(), "not removable");
    std::fs::remove_dir_all(&path).unwrap();
    assert!(!save(&path, before, text, &go()).unwrap(), "older than the Forget: never written");
    assert!(!path.exists());
    assert!(save(&path, generation(), text, &go()).unwrap(), "a save after it writes");
    let _ = std::fs::remove_dir_all(d);
}

/// Off stops a save not begun; never a removal.
#[test]
fn off_stops_a_save_not_begun_never_a_removal() {
    let d = dir("stop");
    let path = d.join(super::FILE);
    let text = "{\"version\": 1, \"choices\": []}\n";
    assert!(!save(&path, generation(), text, &AtomicBool::new(true)).unwrap(), "stopped: nothing written");
    assert!(!path.exists());
    assert!(save(&path, generation(), text, &go()).unwrap());
    let mut r = Recall::new(true, Some(path.clone()));
    let (_, _, _, stop) = r.snapshot().unwrap();
    let forget = r.forget().map(|(_, g)| g).unwrap();
    r.set_on(false);
    assert!(stop.load(std::sync::atomic::Ordering::SeqCst), "off: the save not begun is stopped");
    remove(&path, forget).unwrap();
    assert!(!path.exists(), "the removal, never");
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
    assert_eq!(r.at(&sight("s", "toolu_1"), Some(CWD), &ship, open), Vec::<usize>::new());
    assert!(!r.remember(CWD, &ship, &one("Don't run it")), "nothing kept");
    r.set_on(true);
    assert_eq!((r.reads, r.len()), (1, 1), "on: read now");
    assert_eq!(r.at(&sight("s", "toolu_1"), Some(CWD), &ship, open), Vec::<usize>::new(), "first seen off: nothing, kept");
    assert_eq!(r.at(&sight("s", "toolu_2"), Some(CWD), &ship, open), vec![0], "Run it now");
    r.set_on(false);
    assert_eq!((r.reads, r.len()), (1, 0), "off again: let go, not read");
    assert_eq!(r.at(&sight("s", "toolu_3"), Some(CWD), &ship, open), Vec::<usize>::new());
    assert_eq!(r.at(&sight("s", "toolu_2"), Some(CWD), &ship, open), vec![0], "off: an open card keeps what it showed");
    let _ = std::fs::remove_dir_all(d);
}

/// What an open card recalls is read at its first sight and kept while
/// it is open: a choice remembered meanwhile shows on the next card, not
/// on this one. Forget empties it for every card, and keeps that.
#[test]
fn what_a_card_recalls_is_fixed_at_first_sight() {
    let ship = doc(ship());
    let mut r = Recall::new(true, None);
    let at = |r: &Recall, tool: &str, cwd: Option<&str>| r.at(&sight("s", tool), cwd, &ship, open);
    assert_eq!(at(&r, "toolu_1", Some(CWD)), Vec::<usize>::new(), "nothing yet");
    assert!(r.remember(CWD, &ship, &one("Don't run it")));
    assert_eq!(at(&r, "toolu_1", Some(CWD)), Vec::<usize>::new(), "the card seen before: unchanged");
    assert_eq!(at(&r, "toolu_2", Some(CWD)), vec![2], "a new card: Don't run it");
    assert!(r.remember(CWD, &ship, &one("Run it now")));
    assert_eq!(at(&r, "toolu_2", Some(CWD)), vec![2], "still what it first recalled");
    assert_eq!(at(&r, "toolu_3", Some(CWD)), vec![0], "the latest choice");
    assert_eq!(at(&r, "toolu_4", Some("/elsewhere")), Vec::<usize>::new(), "another folder");
    assert_eq!(at(&r, "toolu_5", None), Vec::<usize>::new(), "no folder");
    assert!(r.forget().is_none(), "no file to remove");
    assert_eq!(r.len(), 0);
    assert_eq!(at(&r, "toolu_3", Some(CWD)), Vec::<usize>::new(), "forgotten, on every card");
    assert!(r.remember(CWD, &ship, &one("Run it now")));
    assert_eq!(at(&r, "toolu_3", Some(CWD)), Vec::<usize>::new(), "and kept so");
    assert_eq!(at(&r, "toolu_6", Some(CWD)), vec![0], "a new card: what was chosen since");
}

/// What cards recalled is kept while they are open, past any count; the
/// sweep, once enough are kept, lets go only of cards no longer open, so
/// what is kept stays within twice the open ones (Shady #93 P2-2).
#[test]
fn what_cards_recalled_lives_as_long_as_they_are_open() {
    let ship = doc(ship());
    let mut r = Recall::new(true, None);
    assert!(r.remember(CWD, &ship, &one("Run it now")));
    let a = sight("s", "toolu_a");
    assert_eq!(r.at(&a, Some(CWD), &ship, open), vec![0]);
    assert!(r.remember(CWD, &ship, &one("Don't run it")));
    let kept = |k: &Sight| *k == a || k.1.starts_with("toolu_open");
    for i in 0..5000 {
        let tool = if i % 100 == 0 { format!("toolu_open{i}") } else { format!("toolu_gone{i}") };
        let card = ("s".to_string(), tool, CardId::parse(&format!("c{i:06}")).unwrap());
        assert_eq!(r.at(&card, Some(CWD), &ship, kept), vec![2]);
        assert!(r.seen_len() <= 2 * 51 + super::SWEEP, "{i}: {} kept", r.seen_len());
    }
    assert_eq!(r.at(&a, Some(CWD), &ship, kept), vec![0], "a, open all along: what it first recalled");
    assert_eq!(r.at(&sight("s", "toolu_open4900"), Some(CWD), &ship, kept), vec![2]);
    for i in 0..200 {
        let card = ("s".to_string(), format!("toolu_after{i}"), CardId::parse(&format!("c{i:06}")).unwrap());
        r.at(&card, Some(CWD), &ship, |_| false);
    }
    assert!(r.seen_len() <= super::SWEEP, "every one closed: let go at the next sweep");
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

/// What holds `path`'s removal back while it is kept, the file left there
/// and readable: on Unix its folder made read-only, on Windows the file
/// held open, shared for reading only.
pub(crate) struct Held {
    #[cfg(unix)]
    dir: PathBuf,
    #[cfg(windows)]
    _file: std::fs::File,
}

pub(crate) fn held(path: &Path) -> Held {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = path.parent().expect("a folder").to_path_buf();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let held = Held { dir };
        assert!(std::fs::File::create(held.dir.join(".probe")).is_err(), "a read-only folder holds nothing back (root?)");
        held
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ)
            .open(path)
            .expect("the file, held");
        Held { _file: file }
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&self.dir, std::fs::Permissions::from_mode(0o755));
        }
    }
}

fn tag() -> Value {
    json!({"question": "Tag the release?", "wait": true, "options": ["Tag it", "Not yet"]})
}

fn deploy() -> Value {
    json!({"question": "Deploy to prod?", "wait": true, "options": ["Deploy", "Hold"]})
}

/// The file's choices, by key.
fn on_file(path: &Path) -> Store {
    Store::parse(&std::fs::read_to_string(path).expect("the file")).expect("readable").0
}

/// A Forget not yet done (Shady #93 R2 P2-1): Off then On, or a new
/// memory of the same file, reads nothing back and starts empty; a choice
/// made then, saved before or after the removal ran, is all the file and
/// the memory hold. Once the file is confirmed gone, On reads it again.
#[test]
fn a_forget_not_yet_done_bars_every_load_whichever_lands_first() {
    let (ship, tag, deploy) = (doc(ship()), doc(tag()), doc(deploy()));
    for removal_first in [true, false] {
        let d = dir(&format!("barred-{removal_first}"));
        let path = d.join(super::FILE);
        let mut r = Recall::new(true, Some(path.clone()));
        assert!(r.remember(CWD, &ship, &one("Run it now")) && r.remember(CWD, &tag, &one("Tag it")));
        let (p, g, t, s) = r.snapshot().unwrap();
        assert!(save(&p, g, &t, &s).unwrap());
        assert_eq!(on_file(&path).len(), 2);
        // Accepted; its removal not run yet.
        let (_, forget) = r.forget().unwrap();
        r.set_on(false);
        r.set_on(true);
        assert_eq!((r.reads, r.len()), (1, 0), "on again before the removal ran: not read, empty");
        let again = Recall::new(true, Some(path.clone()));
        assert_eq!((again.reads, again.len()), (0, 0), "nor by a new memory of the file");
        assert!(r.at(&sight("s", "toolu_f1"), Some(CWD), &ship, open).is_empty(), "a fresh card recalls nothing");
        assert!(r.remember(CWD, &deploy, &one("Deploy")));
        let (p, g, t, s) = r.snapshot().unwrap();
        if removal_first {
            remove(&path, forget).unwrap();
            assert!(save(&p, g, &t, &s).unwrap());
        } else {
            assert!(save(&p, g, &t, &s).unwrap());
            remove(&path, forget).unwrap();
        }
        let kept = on_file(&path);
        assert_eq!(kept.len(), 1, "removal first: {removal_first}");
        assert!(kept.get(&Asked::of(CWD, &deploy).unwrap().key).is_some(), "only what came after the Forget");
        assert_eq!(r.len(), 1);
        r.set_on(false);
        r.set_on(true);
        assert_eq!((r.reads, r.len()), (2, 1), "confirmed gone: read again, what came after only");
        assert!(r.at(&sight("s", "toolu_f2"), Some(CWD), &ship, open).is_empty());
        assert!(r.at(&sight("s", "toolu_f3"), Some(CWD), &tag, open).is_empty());
        let _ = std::fs::remove_dir_all(d);
    }
}

/// A Forget whose removal failed (Shady #93 R2 P2-1): the file, still
/// there with what was forgotten, is never read back (On, a new memory of
/// it); the next save retries the removal first and writes nothing while
/// it fails; once it succeeds, the file holds only what came after, and
/// On reads again.
#[test]
fn a_failed_removal_bars_every_load_and_the_next_save_retries_it() {
    let (ship, tag, deploy) = (doc(ship()), doc(tag()), doc(deploy()));
    let d = dir("barred-failed");
    let path = d.join(super::FILE);
    let mut r = Recall::new(true, Some(path.clone()));
    assert!(r.remember(CWD, &ship, &one("Run it now")) && r.remember(CWD, &tag, &one("Tag it")));
    let (p, g, t, s) = r.snapshot().unwrap();
    assert!(save(&p, g, &t, &s).unwrap());
    let hold = held(&path);
    let (_, forget) = r.forget().unwrap();
    assert!(remove(&path, forget).is_err(), "held back: not removed");
    r.set_on(false);
    r.set_on(true);
    assert_eq!((r.reads, r.len()), (1, 0), "failed: not read back, empty");
    assert_eq!(Recall::new(true, Some(path.clone())).reads, 0, "nor by a new memory of the file");
    assert!(r.remember(CWD, &deploy, &one("Deploy")));
    let (p, g, t, s) = r.snapshot().unwrap();
    assert!(save(&p, g, &t, &s).is_err(), "the save retries the removal first: still held back, nothing written");
    assert_eq!(on_file(&path).len(), 2, "the forgotten file, untouched and unread");
    drop(hold);
    let (p, g, t, s) = r.snapshot().unwrap();
    assert!(save(&p, g, &t, &s).unwrap(), "retried: removed, then written");
    let kept = on_file(&path);
    assert_eq!(kept.len(), 1);
    assert!(kept.get(&Asked::of(CWD, &deploy).unwrap().key).is_some());
    r.set_on(false);
    r.set_on(true);
    assert_eq!((r.reads, r.len()), (2, 1), "confirmed gone: read again");
    let _ = std::fs::remove_dir_all(d);
}
