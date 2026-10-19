//! claude-fade: runs a command (the cockpit's session pane: its nested tmux client) on a terminal of
//! its own and relays both ways. While the pane is out of focus (tmux's focus-out, ESC [ O, on the
//! way in) every colour the command draws gets dim added on its way out, so the whole session fades,
//! colours included, which tmux's window-style cannot do. Keystrokes pass through untouched.
//! A focus change has tmux redraw the client (refresh-client on the relay's terminal, through the
//! server in $TMUX), so what is already on screen fades or comes back too.

use std::ffi::CString;
use std::sync::atomic::{AtomicI32, Ordering};

const STDIN: i32 = 0;
const STDOUT: i32 = 1;
const DIM: &[u8] = b"\x1b[2m";
/// Normal intensity: the redraw that follows does not start with a reset.
const UNDIM: &[u8] = b"\x1b[22m";
const FOCUS_IN: &[u8] = b"\x1b[I";
const FOCUS_OUT: &[u8] = b"\x1b[O";
/// Where the SIGWINCH handler writes, to wake the poll loop (a signal handler may only write).
static RESIZE_PIPE: AtomicI32 = AtomicI32::new(-1);

#[derive(Clone, Copy, PartialEq)]
enum Escape {
    None,
    Started,
    /// A control sequence (ESC [): `is_private` for ESC [ < = > ?, whose final m is not a colour.
    Control { is_first_byte: bool, is_private: bool },
}

/// Adds dim after every colour sequence (CSI … m) while faded. The bytes go out as they come; an
/// escape sequence cut between two reads is followed across them.
struct Fader {
    is_faded: bool,
    escape: Escape,
}

impl Fader {
    fn new() -> Self {
        Fader { is_faded: false, escape: Escape::None }
    }

    fn filter(&mut self, chunk: &[u8]) -> Vec<u8> {
        let mut output = Vec::with_capacity(chunk.len() + 16);
        for &byte in chunk {
            output.push(byte);
            self.escape = match (self.escape, byte) {
                (_, 0x1b) => Escape::Started,
                (Escape::Started, b'[') => Escape::Control { is_first_byte: true, is_private: false },
                (Escape::Control { is_private, .. }, 0x40..=0x7e) => {
                    if byte == b'm' && !is_private && self.is_faded {
                        output.extend_from_slice(DIM);
                    }
                    Escape::None
                }
                (Escape::Control { is_first_byte: true, .. }, b'<'..=b'?') => Escape::Control { is_first_byte: false, is_private: true },
                (Escape::Control { is_private, .. }, _) => Escape::Control { is_first_byte: false, is_private },
                _ => Escape::None,
            };
        }
        output
    }
}

/// Finds tmux's focus reports in the keystrokes, also when one is cut between two reads.
#[derive(Default)]
struct FocusWatcher {
    tail: Vec<u8>,
}

impl FocusWatcher {
    /// The focus after this chunk: Some(true) in, Some(false) out, None when it holds no report.
    fn feed(&mut self, chunk: &[u8]) -> Option<bool> {
        let mut seen: Vec<u8> = std::mem::take(&mut self.tail);
        seen.extend_from_slice(chunk);
        let last_in = seen.windows(3).rposition(|window| window == FOCUS_IN);
        let last_out = seen.windows(3).rposition(|window| window == FOCUS_OUT);
        self.tail = seen[seen.len().saturating_sub(2)..].to_vec();
        match (last_in, last_out) {
            (Some(at_in), Some(at_out)) => Some(at_in > at_out),
            (Some(_), None) => Some(true),
            (None, Some(_)) => Some(false),
            (None, None) => None,
        }
    }
}

extern "C" fn on_resize(_: i32) {
    let fd = RESIZE_PIPE.load(Ordering::Relaxed);
    unsafe { libc::write(fd, b"r".as_ptr().cast(), 1) };
}

fn write_all(fd: i32, mut bytes: &[u8]) -> std::io::Result<()> {
    while !bytes.is_empty() {
        let written = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        if written < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        bytes = &bytes[written as usize..];
    }
    Ok(())
}

/// Puts the terminal back as it was, also when the relay ends on an error.
struct RawMode(Option<libc::termios>);

impl Drop for RawMode {
    fn drop(&mut self) {
        if let Some(saved) = self.0 {
            unsafe { libc::tcsetattr(STDIN, libc::TCSAFLUSH, &saved) };
        }
    }
}

fn enter_raw_mode() -> RawMode {
    if unsafe { libc::isatty(STDIN) } != 1 {
        return RawMode(None);
    }
    let mut saved: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(STDIN, &mut saved) } != 0 {
        return RawMode(None);
    }
    let mut raw = saved;
    unsafe {
        libc::cfmakeraw(&mut raw);
        libc::tcsetattr(STDIN, libc::TCSAFLUSH, &raw);
    }
    RawMode(Some(saved))
}

fn window_size() -> Option<libc::winsize> {
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    (unsafe { libc::ioctl(STDIN, libc::TIOCGWINSZ, &mut size) } == 0).then_some(size)
}

/// Starts the command on a new terminal; returns its pid and the terminal's master side.
fn spawn(command: &[String]) -> std::io::Result<(i32, i32)> {
    let arguments: Vec<CString> = command.iter().map(|argument| CString::new(argument.as_str())).collect::<Result<_, _>>()
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "an argument holds a NUL byte"))?;
    let mut pointers: Vec<*const libc::c_char> = arguments.iter().map(|argument| argument.as_ptr()).collect();
    pointers.push(std::ptr::null());
    let size = window_size();
    let size_pointer = size.as_ref().map_or(std::ptr::null(), |size| size as *const libc::winsize);
    let mut master = -1;
    let pid = unsafe { libc::forkpty(&mut master, std::ptr::null_mut(), std::ptr::null(), size_pointer) };
    if pid < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if pid == 0 {
        unsafe {
            libc::execvp(pointers[0], pointers.as_ptr());
            let message = b"claude-fade: the command could not start\n";
            libc::write(2, message.as_ptr().cast(), message.len());
            libc::_exit(127);
        }
    }
    Ok((pid, master))
}

fn terminal_name(master: i32) -> std::io::Result<String> {
    let mut name = [0 as libc::c_char; 128];
    if unsafe { libc::ptsname_r(master, name.as_mut_ptr(), name.len()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) }.to_string_lossy().into_owned())
}

/// In the background: the relay keeps relaying while tmux redraws through it.
fn redraw(terminal: &str) {
    let terminal = terminal.to_string();
    std::thread::spawn(move || {
        let _ = std::process::Command::new("tmux").args(["refresh-client", "-t", &terminal])
            .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
    });
}

fn watch_resizes() -> std::io::Result<i32> {
    let mut fds = [0; 2];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    RESIZE_PIPE.store(fds[1], Ordering::Relaxed);
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_resize as *const () as usize;
        action.sa_flags = libc::SA_RESTART;
        libc::sigaction(libc::SIGWINCH, &action, std::ptr::null_mut());
    }
    Ok(fds[0])
}

fn relay(command: &[String]) -> std::io::Result<i32> {
    let resizes = watch_resizes()?;
    let (child, master) = spawn(command)?;
    let terminal = terminal_name(master)?;
    let _raw_mode = enter_raw_mode();
    let mut fader = Fader::new();
    let mut focus = FocusWatcher::default();
    let mut buffer = [0u8; 65536];
    let mut is_stdin_open = true;
    loop {
        let mut polled = [
            libc::pollfd { fd: if is_stdin_open { STDIN } else { -1 }, events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: master, events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: resizes, events: libc::POLLIN, revents: 0 },
        ];
        if unsafe { libc::poll(polled.as_mut_ptr(), 3, -1) } < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if polled[0].revents != 0 {
            let count = unsafe { libc::read(STDIN, buffer.as_mut_ptr().cast(), buffer.len()) };
            if count <= 0 {
                is_stdin_open = false;
            } else {
                let keys = &buffer[..count as usize];
                write_all(master, keys)?;
                if let Some(is_focused) = focus.feed(keys) {
                    if fader.is_faded == is_focused {
                        fader.is_faded = !is_focused;
                        write_all(STDOUT, if fader.is_faded { DIM } else { UNDIM })?;
                        redraw(&terminal);
                    }
                }
            }
        }
        if polled[1].revents != 0 {
            let count = unsafe { libc::read(master, buffer.as_mut_ptr().cast(), buffer.len()) };
            if count <= 0 {
                break; // the command ended: its terminal reads EIO
            }
            write_all(STDOUT, &fader.filter(&buffer[..count as usize]))?;
        }
        if polled[2].revents != 0 {
            while unsafe { libc::read(resizes, buffer.as_mut_ptr().cast(), buffer.len()) } > 0 {}
            if let Some(size) = window_size() {
                unsafe { libc::ioctl(master, libc::TIOCSWINSZ, &size) };
            }
        }
    }
    let mut status = 0;
    unsafe { libc::waitpid(child, &mut status, 0) };
    Ok(if libc::WIFEXITED(status) { libc::WEXITSTATUS(status) } else { 1 })
}

fn main() {
    let command: Vec<String> = std::env::args().skip(1).collect();
    if command.is_empty() {
        eprintln!("usage: claude-fade <command> [argument…]");
        std::process::exit(2);
    }
    match relay(&command) {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("claude-fade: {error}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn faded() -> Fader {
        Fader { is_faded: true, escape: Escape::None }
    }

    #[test]
    fn colours_pass_unchanged_while_focused() {
        assert_eq!(Fader::new().filter(b"\x1b[31mred\x1b[0m"), b"\x1b[31mred\x1b[0m");
    }

    #[test]
    fn every_colour_sequence_gets_dim_while_faded() {
        for (drawn, shown) in [
            (&b"\x1b[31mred"[..], &b"\x1b[31m\x1b[2mred"[..]),
            (b"\x1b[0m", b"\x1b[0m\x1b[2m"),
            (b"\x1b[38;2;158;206;106mgreen", b"\x1b[38;2;158;206;106m\x1b[2mgreen"),
            (b"\x1b[m", b"\x1b[m\x1b[2m"),
            (b"plain", b"plain"),
        ] {
            assert_eq!(faded().filter(drawn), shown, "{:?}", String::from_utf8_lossy(drawn));
        }
    }

    #[test]
    fn a_sequence_cut_between_reads_still_gets_dim() {
        let mut fader = faded();
        let mut shown = fader.filter(b"a\x1b[3");
        shown.extend(fader.filter(b"8;5;1mb"));
        assert_eq!(shown, b"a\x1b[38;5;1m\x1b[2mb");
    }

    #[test]
    fn other_sequences_ending_in_m_are_left_alone() {
        for drawn in [&b"\x1b[>4;2m"[..], b"\x1b[?1004h", b"\x1b[2J", b"\x1b]0;title m\x07", b"\x1bm"] {
            assert_eq!(faded().filter(drawn), drawn, "{:?}", String::from_utf8_lossy(drawn));
        }
    }

    #[test]
    fn the_last_focus_report_in_the_keys_wins() {
        for (keys, focus) in [(&b"\x1b[O"[..], Some(false)), (b"\x1b[I", Some(true)), (b"\x1b[O\x1b[I", Some(true)), (b"abc", None)] {
            assert_eq!(FocusWatcher::default().feed(keys), focus, "{:?}", String::from_utf8_lossy(keys));
        }
    }

    #[test]
    fn a_focus_report_cut_between_reads_is_seen() {
        let mut watcher = FocusWatcher::default();
        assert_eq!(watcher.feed(b"x\x1b["), None);
        assert_eq!(watcher.feed(b"O"), Some(false));
        assert_eq!(watcher.feed(b"y"), None);
    }
}
