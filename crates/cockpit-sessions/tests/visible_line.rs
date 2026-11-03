//! #656: a stored delivery note is any error's text; a card shows it as one honest line.

#[test]
fn a_note_keeps_one_line_and_drops_formatting_that_could_disguise_it() {
    for (note, shown) in [
        ("tmux: no pane\nexit 1", "tmux: no pane exit 1"),
        ("tab\there", "tab here"),
        ("hidden\u{200B}mark", "hiddenmark"),
        ("over\u{202E}ride", "override"),
        ("plain", "plain"),
    ] {
        assert_eq!(cockpit_sessions::visible_line(note), shown, "{note:?}");
    }
}
