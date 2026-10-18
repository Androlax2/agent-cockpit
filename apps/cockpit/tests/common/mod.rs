//! A program on a pseudo-terminal, for the tests that drive a full-screen program as Theo would.
#![allow(dead_code)]

use std::fs;
use std::io::{Read, Write};
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub struct Terminal {
    pub master: fs::File,
    pub child: Child,
    pub screen: Vec<u8>,
}

pub fn set_size(fd: i32, rows: u16, columns: u16) {
    let size = libc::winsize { ws_row: rows, ws_col: columns, ws_xpixel: 0, ws_ypixel: 0 };
    unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &size) };
}

impl Terminal {
    /// The program on a new pseudo-terminal of 24 × 80, as the session leader so the terminal is its own.
    pub fn start(program: &Path, arguments: &[&str], environment: &[(&str, &Path)]) -> Terminal {
        let (mut master, mut slave) = (0, 0);
        unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null(), std::ptr::null()) };
        set_size(master, 24, 80);
        let slave_fd = unsafe { OwnedFd::from_raw_fd(slave) };
        let mut command = Command::new(program);
        command.args(arguments).env("TMUX", "1").env("TERM", "xterm-256color")
            .stdin(Stdio::from(slave_fd.try_clone().unwrap())).stdout(Stdio::from(slave_fd.try_clone().unwrap())).stderr(Stdio::from(slave_fd));
        for (name, value) in environment {
            command.env(name, value);
        }
        unsafe {
            command.pre_exec(|| {
                libc::setsid();
                libc::ioctl(0, libc::TIOCSCTTY, 0);
                Ok(())
            });
        }
        let child = command.spawn().unwrap();
        unsafe { libc::fcntl(master, libc::F_SETFL, libc::O_NONBLOCK) };
        Terminal { master: unsafe { fs::File::from_raw_fd(master) }, child, screen: Vec::new() }
    }

    /// Collects what is drawn for `seconds`, or until `until` shows up.
    pub fn pump(&mut self, seconds: f64, until: Option<&[u8]>) {
        let end = Instant::now() + Duration::from_secs_f64(seconds);
        let mut buffer = [0u8; 65536];
        while Instant::now() < end {
            if until.is_some_and(|wanted| contains(&self.screen, wanted)) {
                return;
            }
            match self.master.read(&mut buffer) {
                Ok(0) => return,
                Ok(count) => self.screen.extend_from_slice(&buffer[..count]),
                Err(_) => std::thread::sleep(Duration::from_millis(30)),
            }
        }
    }

    pub fn type_keys(&mut self, keys: &[u8]) {
        self.master.write_all(keys).unwrap();
    }
}

pub fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|window| window == needle)
}

pub fn last_index(haystack: &[u8], needle: &[u8]) -> usize {
    haystack.windows(needle.len()).rposition(|window| window == needle).expect("text on screen")
}

