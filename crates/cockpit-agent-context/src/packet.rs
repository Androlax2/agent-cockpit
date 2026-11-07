//! The task packet put in front of an agent with its brief at every context start (#781).
//! A harness's summary of a compacted conversation kept about half of its issue numbers and
//! branch names (`docs/research/2026-11-04-686-compaction-research.md`); Torlek holds them
//! exactly, so it hands them back. The daemon gives the step's packet as `torlek workflow
//! packet` prints it, trimmed to the last handoff; this frames it and holds it to a fixed size.

/// The packet's fixed upper end in bytes, its frame included.
pub const TASK_PACKET_CAP_BYTES: usize = 2048;

/// `packet_text`, the trimmed packet of workflow `workflow`, framed with the command that
/// prints all of it and cut to [`TASK_PACKET_CAP_BYTES`] when longer.
pub fn task_packet(workflow: i64, packet_text: &str) -> String {
    let command = format!("`torlek workflow packet {workflow}`");
    let mut text = format!(
        "# Task packet\n\nYour step as {command} prints it, with only the last handoff and no \
         decisions; that command prints all of it.\n\n"
    );
    if text.len() + packet_text.len() <= TASK_PACKET_CAP_BYTES {
        text.push_str(packet_text);
        return text;
    }
    let cut = format!("\n[cut at {TASK_PACKET_CAP_BYTES} bytes: {command} prints the rest]\n");
    let room = TASK_PACKET_CAP_BYTES.saturating_sub(text.len() + cut.len());
    let end = (0..=room.min(packet_text.len()))
        .rev()
        .find(|&at| packet_text.is_char_boundary(at))
        .unwrap_or(0);
    text.push_str(&packet_text[..end]);
    text.push_str(&cut);
    text
}
