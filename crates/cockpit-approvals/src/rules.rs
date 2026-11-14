//! The rules of the scan an `ask run` command gets before the owner reads it (#1388), in one
//! table. Each has an id, the one line the owner reads and a severity; its pattern is data
//! the scan (`rule_scan.rs`) reads. Adding a rule, a program or a path is a change here and a
//! case in `tests/rule_scan.rs`.

/// How much a finding weighs on the owner's card: high ones come first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    High,
    Warn,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::High => "high",
            Severity::Warn => "warn",
        }
    }

    pub(crate) fn parse(text: &str) -> Option<Severity> {
        match text {
            "high" => Some(Severity::High),
            "warn" => Some(Severity::Warn),
            _ => None,
        }
    }
}

pub(crate) struct Rule {
    pub(crate) id: &'static str,
    pub(crate) severity: Severity,
    pub(crate) reason: &'static str,
    pub(crate) patterns: &'static [Pattern],
}

pub(crate) enum Pattern {
    /// What one of these commands prints becomes code a shell or an interpreter runs: piped
    /// into it, as its `-c` string, script or input through a substitution.
    RunsOutputOf(&'static [Source]),
    /// A file a downloader saved (`curl -o`, `wget`, `curl … > file`) is run later on the
    /// same command line.
    RunsSavedDownload,
    /// A command names a path in one of these places.
    Reads(&'static [Place]),
    /// A command writes a path in one of these places: an output redirection, or a program
    /// of [`WRITERS`] given it.
    Writes(&'static [Place]),
    /// One of these programs runs as described.
    Runs(&'static [Invocation]),
    /// `chmod` sets the setuid or setgid bit.
    SetsSetuid,
    /// `chown` gives a file to root.
    GivesToRoot,
    /// `rm`, or `find -delete`, on home, a folder above it, `/`, a folder at the top of `/`,
    /// or everything directly inside one of them.
    DeletesAtOrAboveHome,
    /// sudo runs a file the asking session can change (#1383's `root_runs`).
    SudoRunsEditable,
    /// Part of what sudo runs cannot be resolved (#1383's `root_runs`).
    SudoCannotTell,
    /// The program, its code or a script cannot be resolved without running the command: an
    /// expansion where the program is, `eval`, code from a pipe or an expansion.
    CannotTell,
}

/// A program whose output counts as code once an interpreter runs it.
pub(crate) struct Source {
    pub(crate) programs: &'static [&'static str],
    /// Only with one of these options (`base64 -d`); any use when empty.
    pub(crate) with_option: &'static [&'static str],
}

/// A path the scan looks for.
pub(crate) enum Place {
    /// In the home folder (`~/.ssh`): the path, or anything inside it.
    Home(&'static str),
    /// An absolute path, or anything inside it.
    Absolute(&'static str),
    /// A file of this name, or of this name and a suffix (`.env.local`), outside the clone.
    NamedOutsideClone(&'static str),
    /// Anything inside a folder ending in these components (`.git/hooks`), outside the clone.
    InsideOutsideClone(&'static str),
}

/// A program the scan flags when it runs, unless given one of `unless`.
pub(crate) struct Invocation {
    pub(crate) program: &'static str,
    /// Its first operand is one of these (`systemctl enable`); any when `None`.
    pub(crate) operand: Option<&'static [&'static str]>,
    pub(crate) unless: &'static [&'static str],
}

/// A program that writes the paths it is given.
pub(crate) struct Writer {
    pub(crate) program: &'static str,
    pub(crate) writes: Written,
}

/// Which of a writer's words are the paths it writes.
pub(crate) enum Written {
    /// Each operand.
    Operands,
    /// Each operand, when given this option (`sed -i`).
    OperandsWith(&'static str),
    /// The last operand, or the folder `-t` names (`cp`, `mv`).
    Destination,
    /// The value of the word starting with this (`dd of=`).
    Value(&'static str),
}

pub(crate) const DOWNLOADERS: &[Source] = &[Source {
    programs: &["curl", "wget", "fetch", "http", "https", "xh"],
    with_option: &[],
}];

const DECODERS: &[Source] = &[
    Source {
        programs: &["base64", "base32", "basenc"],
        with_option: &["-d", "--decode", "-D"],
    },
    Source {
        programs: &["xxd"],
        with_option: &["-r", "-revert"],
    },
    Source {
        programs: &["openssl"],
        with_option: &["-d"],
    },
    Source {
        programs: &["uudecode"],
        with_option: &[],
    },
];

const SECRETS: &[Place] = &[
    Place::Home(".ssh"),
    Place::Home(".gnupg"),
    Place::Home(".config/gh"),
    Place::Home(".netrc"),
    Place::Home(".git-credentials"),
    Place::Home(".aws"),
    Place::Home(".docker/config.json"),
    Place::Home(".kube"),
    Place::Home(".password-store"),
    Place::Home(".local/share/keyrings"),
    Place::Home(".mozilla"),
    Place::Home(".config/google-chrome"),
    Place::Home(".config/chromium"),
    Place::Home(".config/BraveSoftware"),
    Place::Home(".config/vivaldi"),
    Place::Home("snap/firefox"),
    Place::NamedOutsideClone(".env"),
];

const PERSISTENCE: &[Place] = &[
    Place::Home(".bashrc"),
    Place::Home(".bash_profile"),
    Place::Home(".bash_login"),
    Place::Home(".bash_logout"),
    Place::Home(".profile"),
    Place::Home(".zshrc"),
    Place::Home(".zshenv"),
    Place::Home(".zprofile"),
    Place::Home(".zlogin"),
    Place::Home(".config/fish/config.fish"),
    Place::Home(".config/fish/conf.d"),
    Place::Home(".config/autostart"),
    Place::Home(".config/systemd"),
    Place::Home(".config/environment.d"),
    Place::Home(".local/share/systemd"),
    Place::Home(".ssh/authorized_keys"),
    Place::Home(".ssh/rc"),
    Place::Home(".config/git/hooks"),
    Place::Absolute("/etc/profile"),
    Place::Absolute("/etc/profile.d"),
    Place::Absolute("/etc/bash.bashrc"),
    Place::Absolute("/etc/zsh"),
    Place::Absolute("/etc/environment"),
    Place::Absolute("/etc/xdg/autostart"),
    Place::Absolute("/etc/systemd"),
    Place::Absolute("/usr/lib/systemd"),
    Place::Absolute("/lib/systemd"),
    Place::Absolute("/etc/crontab"),
    Place::Absolute("/etc/cron.d"),
    Place::Absolute("/etc/cron.hourly"),
    Place::Absolute("/etc/cron.daily"),
    Place::Absolute("/etc/cron.weekly"),
    Place::Absolute("/etc/cron.monthly"),
    Place::Absolute("/var/spool/cron"),
    Place::Absolute("/etc/sudoers"),
    Place::Absolute("/etc/sudoers.d"),
    Place::Absolute("/etc/ld.so.preload"),
    Place::Absolute("/etc/ld.so.conf"),
    Place::Absolute("/etc/ld.so.conf.d"),
    Place::Absolute("/etc/pam.d"),
    Place::Absolute("/etc/udev/rules.d"),
    Place::InsideOutsideClone(".git/hooks"),
];

const PERSISTENCE_PROGRAMS: &[Invocation] = &[
    Invocation {
        program: "crontab",
        operand: None,
        unless: &["-l", "-r", "-V", "--help"],
    },
    Invocation {
        program: "systemctl",
        operand: Some(&[
            "enable",
            "link",
            "edit",
            "add-wants",
            "add-requires",
            "set-default",
        ]),
        unless: &[],
    },
    Invocation {
        program: "visudo",
        operand: None,
        unless: &["-c", "--check"],
    },
];

pub(crate) const WRITERS: &[Writer] = &[
    Writer {
        program: "tee",
        writes: Written::Operands,
    },
    Writer {
        program: "touch",
        writes: Written::Operands,
    },
    Writer {
        program: "truncate",
        writes: Written::Operands,
    },
    Writer {
        program: "sed",
        writes: Written::OperandsWith("-i"),
    },
    Writer {
        program: "perl",
        writes: Written::OperandsWith("-i"),
    },
    Writer {
        program: "cp",
        writes: Written::Destination,
    },
    Writer {
        program: "mv",
        writes: Written::Destination,
    },
    Writer {
        program: "install",
        writes: Written::Destination,
    },
    Writer {
        program: "ln",
        writes: Written::Destination,
    },
    Writer {
        program: "rsync",
        writes: Written::Destination,
    },
    Writer {
        program: "dd",
        writes: Written::Value("of="),
    },
];

/// Every rule, in the order the card lists findings of one severity.
pub(crate) const RULES: &[Rule] = &[
    Rule {
        id: "download-runs",
        severity: Severity::High,
        reason: "runs code it downloads",
        patterns: &[
            Pattern::RunsOutputOf(DOWNLOADERS),
            Pattern::RunsSavedDownload,
        ],
    },
    Rule {
        id: "decode-runs",
        severity: Severity::High,
        reason: "runs code it decodes",
        patterns: &[Pattern::RunsOutputOf(DECODERS)],
    },
    Rule {
        id: "sudo-runs-editable",
        severity: Severity::High,
        reason: "sudo runs a file this session can edit",
        patterns: &[Pattern::SudoRunsEditable],
    },
    Rule {
        id: "reads-secrets",
        severity: Severity::High,
        reason: "reads secrets",
        patterns: &[Pattern::Reads(SECRETS)],
    },
    Rule {
        id: "writes-persistence",
        severity: Severity::High,
        reason: "writes a file that runs code later",
        patterns: &[
            Pattern::Writes(PERSISTENCE),
            Pattern::Runs(PERSISTENCE_PROGRAMS),
        ],
    },
    Rule {
        id: "setuid-or-root-owned",
        severity: Severity::High,
        reason: "makes a file setuid, setgid or root's",
        patterns: &[Pattern::SetsSetuid, Pattern::GivesToRoot],
    },
    Rule {
        id: "deletes-home-or-root",
        severity: Severity::High,
        reason: "deletes at or above home or /",
        patterns: &[Pattern::DeletesAtOrAboveHome],
    },
    Rule {
        id: "cannot-tell",
        severity: Severity::Warn,
        reason: "can't tell what runs",
        patterns: &[Pattern::CannotTell],
    },
    Rule {
        id: "sudo-cannot-tell",
        severity: Severity::Warn,
        reason: "can't tell what runs as root",
        patterns: &[Pattern::SudoCannotTell],
    },
];
