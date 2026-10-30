//! Terminal report loop; usage data remains independent of terminal probing.

use super::report_lines;

/// cockpit usage: redraws the report until the pane is replaced.
pub fn report_forever() -> ! {
    loop {
        let columns = crossterm::terminal::size()
            .map(|(columns, _)| columns as usize)
            .unwrap_or(80);
        println!("\x1b[2J\x1b[H{}", report_lines(columns).join("\n"));
        use std::io::Write;
        let _ = std::io::stdout().flush();
        std::thread::sleep(std::time::Duration::from_secs(30));
    }
}
