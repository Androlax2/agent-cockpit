//! A line typed at the terminal without echoing it, for a secret (#1251).

use std::io::{self, Stdin};
use std::os::fd::{AsFd, BorrowedFd};

use rustix::termios::{LocalModes, OptionalActions, Termios, tcgetattr, tcsetattr};

/// One line from `stdin`, a terminal, read with its echo off after `prompt` is shown on
/// stderr: the prompt comes once nothing typed would show.
pub(crate) fn read_hidden_line(stdin: &Stdin, prompt: &str) -> io::Result<String> {
    let echo_off = EchoOff::new(stdin.as_fd())?;
    eprint!("{prompt}");
    let mut line = String::new();
    let read = stdin.read_line(&mut line);
    // The Enter typed was not echoed either.
    eprintln!();
    echo_off.restore()?;
    read?;
    Ok(line)
}

/// The terminal with its echo turned off; dropping it puts the settings it found back, on an
/// early return or a panic too.
struct EchoOff<'fd> {
    terminal: BorrowedFd<'fd>,
    saved: Option<Termios>,
}

impl<'fd> EchoOff<'fd> {
    fn new(terminal: BorrowedFd<'fd>) -> io::Result<EchoOff<'fd>> {
        let saved = tcgetattr(terminal)?;
        let mut hidden = saved.clone();
        hidden.local_modes.remove(LocalModes::ECHO);
        tcsetattr(terminal, OptionalActions::Now, &hidden)?;
        Ok(EchoOff {
            terminal,
            saved: Some(saved),
        })
    }

    /// Puts the settings back now, reporting a failure the drop would have to ignore.
    fn restore(mut self) -> io::Result<()> {
        match self.saved.take() {
            Some(saved) => Ok(tcsetattr(self.terminal, OptionalActions::Now, &saved)?),
            None => Ok(()),
        }
    }
}

impl Drop for EchoOff<'_> {
    fn drop(&mut self) {
        if let Some(saved) = self.saved.take() {
            let _ = tcsetattr(self.terminal, OptionalActions::Now, &saved);
        }
    }
}
