//! D6b wired ([`super`]): the chip's words. The window's tests are
//! `rail_wishes_ui_tests.rs`.

use cards_spec::block::BlockId;
use cards_spec::block::message::Visibility as Notice;

use super::tells;

fn id(s: &str) -> BlockId {
    BlockId::parse(s).expect("an id")
}

#[test]
fn the_chip_says_what_rides_by_kind() {
    assert_eq!(tells(&[]), None);
    let riding = [(id("plan"), Notice::Dismissed, 1), (id("tests"), Notice::Dismissed, 2), (id("notes"), Notice::Shown, 3)];
    assert_eq!(tells(&riding).as_deref(), Some("+ tells the agent: plan, tests dismissed · notes shown ×"));
    assert_eq!(tells(&riding[2..]).as_deref(), Some("+ tells the agent: notes shown ×"));
}
