//! #781: the task packet shown with the brief at every context start, framed with the command
//! that prints all of it and held to a fixed size.

use cockpit_agent_context::{TASK_PACKET_CAP_BYTES, task_packet, unreadable_task_packet};

const PACKET: &str = "workflow 7 (code-change v1, running)\nproject: app\n\
goal: item 42 Fix the login form (feature, doing)\n\
step 2 check (active, s0123456789ab) [--step 2 --attempt 1]: tests pass, review next\n";

#[test]
fn a_packet_that_fits_is_given_whole_after_the_command_for_the_rest() {
    let text = task_packet(7, PACKET);

    let (frame, packet) = text
        .split_once("\n\n")
        .unwrap()
        .1
        .split_once("\n\n")
        .unwrap();
    assert!(frame.contains("`torlek workflow packet 7`"), "{frame}");
    assert_eq!(packet, PACKET);
}

#[test]
fn a_long_packet_is_cut_at_the_cap_and_says_so() {
    let notes = "Résumé of the issue body. ".repeat(400);

    let text = task_packet(i64::MAX, &format!("{PACKET}notes:\n{notes}\n"));

    assert!(text.len() <= TASK_PACKET_CAP_BYTES, "{} bytes", text.len());
    assert!(text.contains(PACKET), "{text}");
    assert!(
        text.ends_with(&format!(
            "[cut at {TASK_PACKET_CAP_BYTES} bytes: `torlek workflow packet {}` prints the rest]\n",
            i64::MAX
        )),
        "{text}"
    );
}

/// The cut never splits a character, whichever byte the cap falls on.
#[test]
fn the_cut_lands_on_a_character_boundary_at_any_length() {
    for prefix in 0..4 {
        let text = task_packet(7, &format!("{}{}", "x".repeat(prefix), "é€😀".repeat(400)));

        assert!(text.len() <= TASK_PACKET_CAP_BYTES, "{} bytes", text.len());
        assert!(text.contains("prints the rest]"), "{text}");
    }
}

#[test]
fn an_unreadable_packet_names_the_command_that_says_why() {
    let text = unreadable_task_packet(7);

    assert!(text.starts_with("# Task packet\n"), "{text}");
    assert!(text.contains("`torlek workflow packet 7`"), "{text}");
}
