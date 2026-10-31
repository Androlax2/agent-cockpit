//! The names and ids the contract validates where they are made.

use cockpit_provider_api::{ConversationId, ProviderName};

#[test]
fn a_provider_name_is_lower_case_letters_digits_and_dashes() {
    for name in ["claude", "codex", "open-code2"] {
        assert_eq!(ProviderName::new(name).unwrap().as_str(), name);
    }
}

#[test]
fn a_provider_name_outside_its_rule_is_refused() {
    for name in ["", "Claude", "2claude", "-x", "a b", "a/b", &"a".repeat(33)] {
        assert!(ProviderName::new(name).is_err(), "{name:?} was admitted");
    }
}

#[test]
fn a_conversation_id_is_printable_ascii_without_spaces() {
    assert!(ConversationId::new("0b7f3c1e-1f6a-4c55-a2a1-9e3c7c0d1a2b").is_ok());
    for id in ["", "a b", "line\nbreak", "é", &"x".repeat(257)] {
        assert!(ConversationId::new(id).is_err(), "{id:?} was admitted");
    }
}
