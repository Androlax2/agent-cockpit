//! Stage 2: inside bubblewrap's namespaces, the last step before the program. Checks the
//! mounts the launcher planned landed on the files it saw, applies Landlock at exactly the
//! ABI the daemon probed and the seccomp filter, then runs the program in its place. Any
//! failure stops the launch: the program never runs with less than the spec says.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::ffi::OsString;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};

use landlock::{
    ABI, Access, AccessFs, BitFlags, CompatLevel, Compatible, LandlockStatus, PathBeneath, PathFd,
    Ruleset, RulesetAttr, RulesetCreated, RulesetCreatedAttr, RulesetStatus, Scope,
};
use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
    SeccompRule, TargetArch,
};

use crate::plan::Policy;

/// The verb of the probe launch: applies the same restrictions to nothing and reports the
/// Landlock ABI the kernel gave.
pub const PROBE_VERB: &str = "stage2-probe";

/// The Landlock ABI that introduced scopes: abstract Unix sockets and signals stay inside
/// the sandbox.
const SCOPES_ABI: u8 = 6;

/// The Landlock ABI from which connecting to a named Unix socket is a file right.
const RESOLVE_UNIX_ABI: u8 = 9;

pub fn run(policy_argument: &OsString, arguments: &[OsString]) -> Result<Infallible, String> {
    let policy_text = policy_argument
        .to_str()
        .ok_or("stage 2's policy is not UTF-8")?;
    let policy: Policy = serde_json::from_str(policy_text)
        .map_err(|error| format!("stage 2's policy is malformed: {error}"))?;
    verify_mounts(&policy)?;
    crate::handoff::hand_over(&policy.proxy_handoff, &policy.forward_ports)?;
    restrict_files(&policy)?;
    restrict_system_calls()?;
    let error = std::process::Command::new(&policy.program)
        .arg0(&policy.program_name)
        .args(arguments)
        .exec();
    Err(format!("cannot run {}: {error}", policy.program.display()))
}

/// The probe launch's stage 2: Landlock with every right this launcher knows, best effort,
/// and the seccomp filter, then prints the ABI Landlock enforced, 0 when it enforced none.
pub fn probe() -> Result<(), String> {
    let newest = ABI::V9;
    let status = Ruleset::default()
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(AccessFs::from_all(newest))
        .and_then(|ruleset| ruleset.scope(Scope::from_all(newest)))
        .and_then(Ruleset::create)
        .and_then(RulesetCreated::restrict_self)
        .map_err(|error| format!("cannot apply Landlock: {error}"))?;
    let enforced = match (status.ruleset, status.landlock) {
        (RulesetStatus::NotEnforced, _) => 0,
        (_, LandlockStatus::Available { effective_abi, .. }) => abi_number(effective_abi)?,
        (_, LandlockStatus::NotEnabled | LandlockStatus::NotImplemented) => 0,
    };
    restrict_system_calls()?;
    println!("{enforced}");
    Ok(())
}

fn abi_number(abi: ABI) -> Result<u8, String> {
    abi.to_string()
        .parse()
        .map_err(|_| format!("Landlock reports no numbered ABI ({abi})"))
}

/// Each folder or file the launcher pinned or masked inside a writable folder must still be
/// the one it saw: a link swapped in before bubblewrap mounted it would have moved the mount.
fn verify_mounts(policy: &Policy) -> Result<(), String> {
    for expected in &policy.verify {
        let metadata = std::fs::symlink_metadata(&expected.path)
            .map_err(|error| format!("cannot check {}: {error}", expected.path.display()))?;
        if metadata.dev() != expected.device || metadata.ino() != expected.inode {
            return Err(format!(
                "{} changed between planning and mounting; the launch is refused",
                expected.path.display()
            ));
        }
    }
    Ok(())
}

/// Landlock at exactly the probed ABI, as a hard requirement: reads and runs where the
/// policy reads, every right where it writes, nothing elsewhere.
fn restrict_files(policy: &Policy) -> Result<(), String> {
    let abi = ABI::from(i32::from(policy.landlock_abi));
    let mut ruleset = Ruleset::default()
        .set_compatibility(CompatLevel::HardRequirement)
        .handle_access(AccessFs::from_all(abi))
        .map_err(landlock_error)?;
    if policy.landlock_abi >= SCOPES_ABI {
        ruleset = ruleset
            .scope(Scope::from_all(abi))
            .map_err(landlock_error)?;
    }
    let mut socket_access = AccessFs::from_read(abi);
    if policy.landlock_abi >= RESOLVE_UNIX_ABI {
        socket_access |= AccessFs::ResolveUnix;
    }
    // Listing the root shows only what the view mounted; it reads no file.
    let mut created = ruleset
        .create()
        .map_err(landlock_error)?
        .add_rule(rule(Path::new("/"), AccessFs::ReadDir.into())?)
        .map_err(landlock_error)?;
    let grants = [
        (&policy.read, AccessFs::from_read(abi)),
        (&policy.sockets, socket_access),
        (&policy.write, AccessFs::from_all(abi)),
    ];
    for (paths, access) in grants {
        for path in paths {
            created = created
                .add_rule(rule(path, access)?)
                .map_err(landlock_error)?;
        }
    }
    for stream in inherited_streams() {
        created = created
            .add_rule(rule(
                &stream,
                AccessFs::from_read(abi) | AccessFs::from_write(abi),
            )?)
            .map_err(landlock_error)?;
    }
    let status = created.restrict_self().map_err(landlock_error)?;
    if status.ruleset != RulesetStatus::FullyEnforced {
        return Err(format!(
            "Landlock ABI {} is not fully enforced ({:?})",
            policy.landlock_abi, status.ruleset
        ));
    }
    Ok(())
}

/// The terminal or file each standard stream already is: tools reopen it by name through
/// `/dev/stdout`, and the pane's terminal is on the host's devpts, outside the
/// view's `/dev`. The program holds each open already, so reopening it gives nothing more.
fn inherited_streams() -> Vec<PathBuf> {
    (0..=2)
        .map(|descriptor| PathBuf::from(format!("/proc/self/fd/{descriptor}")))
        .filter(|stream| {
            std::fs::metadata(stream)
                .is_ok_and(|metadata| metadata.file_type().is_char_device() || metadata.is_file())
        })
        .collect()
}

fn rule(path: &Path, access: BitFlags<AccessFs>) -> Result<PathBeneath<PathFd>, String> {
    let file = PathFd::new(path)
        .map_err(|error| format!("cannot open {} for Landlock: {error}", path.display()))?;
    let is_directory = std::fs::metadata(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?
        .is_dir();
    // A file takes only file rights; folder rights on it would make the rule invalid.
    let access = if is_directory {
        access
    } else {
        access & AccessFs::from_file(ABI::V9)
    };
    Ok(PathBeneath::new(file, access))
}

fn landlock_error(error: landlock::RulesetError) -> String {
    format!("cannot apply Landlock: {error}")
}

/// The seccomp filter: system calls an agent never needs, which reach the kernel's own
/// state or other processes, fail with `EPERM`; `clone3`, whose flags a filter cannot read,
/// fails with `ENOSYS` so programs fall back to `clone`, whose flags it can.
fn restrict_system_calls() -> Result<(), String> {
    let arch = target_arch()?;
    let denied = denied_calls();
    let filters = [
        SeccompFilter::new(
            unreadable_clones(),
            SeccompAction::Allow,
            SeccompAction::Errno(libc::ENOSYS.cast_unsigned()),
            arch,
        ),
        SeccompFilter::new(
            denied,
            SeccompAction::Allow,
            SeccompAction::Errno(libc::EPERM.cast_unsigned()),
            arch,
        ),
    ];
    for filter in filters {
        let program = filter
            .and_then(BpfProgram::try_from)
            .map_err(|error| format!("cannot build the seccomp filter: {error}"))?;
        seccompiler::apply_filter(&program)
            .map_err(|error| format!("cannot apply the seccomp filter: {error}"))?;
    }
    Ok(())
}

fn target_arch() -> Result<TargetArch, String> {
    TargetArch::try_from(std::env::consts::ARCH)
        .map_err(|_| format!("no seccomp filter for {}", std::env::consts::ARCH))
}

/// Denied outright: tracing and reading other processes, mounts and namespaces, kernel
/// modules, keys, BPF, perf, `io_uring` (whose operations no filter sees), the clock and
/// other machine-wide state.
const DENIED_ALWAYS: &[i64] = &[
    libc::SYS_ptrace,
    libc::SYS_process_vm_readv,
    libc::SYS_process_vm_writev,
    libc::SYS_mount,
    libc::SYS_umount2,
    libc::SYS_pivot_root,
    libc::SYS_chroot,
    libc::SYS_swapon,
    libc::SYS_swapoff,
    libc::SYS_reboot,
    libc::SYS_kexec_load,
    libc::SYS_kexec_file_load,
    libc::SYS_init_module,
    libc::SYS_finit_module,
    libc::SYS_delete_module,
    libc::SYS_acct,
    libc::SYS_settimeofday,
    libc::SYS_clock_settime,
    libc::SYS_clock_adjtime,
    libc::SYS_adjtimex,
    libc::SYS_quotactl,
    libc::SYS_quotactl_fd,
    libc::SYS_keyctl,
    libc::SYS_add_key,
    libc::SYS_request_key,
    libc::SYS_bpf,
    libc::SYS_perf_event_open,
    libc::SYS_userfaultfd,
    libc::SYS_open_by_handle_at,
    libc::SYS_name_to_handle_at,
    libc::SYS_syslog,
    libc::SYS_vhangup,
    libc::SYS_unshare,
    libc::SYS_setns,
    libc::SYS_fsopen,
    libc::SYS_fsconfig,
    libc::SYS_fsmount,
    libc::SYS_fspick,
    libc::SYS_move_mount,
    libc::SYS_open_tree,
    libc::SYS_mount_setattr,
    libc::SYS_io_uring_setup,
    libc::SYS_io_uring_enter,
    libc::SYS_io_uring_register,
    #[cfg(target_arch = "x86_64")]
    libc::SYS_iopl,
    #[cfg(target_arch = "x86_64")]
    libc::SYS_ioperm,
];

/// `clone` flags that make a namespace.
const NAMESPACE_FLAGS: [libc::c_int; 7] = [
    libc::CLONE_NEWNS,
    libc::CLONE_NEWCGROUP,
    libc::CLONE_NEWUTS,
    libc::CLONE_NEWIPC,
    libc::CLONE_NEWUSER,
    libc::CLONE_NEWPID,
    libc::CLONE_NEWNET,
];

/// Socket families an agent uses: local sockets, the network (loopback only in its
/// namespace) and routing netlink, which `getifaddrs` reads.
const SOCKET_FAMILIES: [libc::c_int; 4] = [
    libc::AF_UNIX,
    libc::AF_INET,
    libc::AF_INET6,
    libc::AF_NETLINK,
];

/// On x86_64 the x32 ABI reaches the same calls with this bit set, and a few under numbers
/// of their own; the filter's architecture check does not tell x32 apart, so every denied
/// call is denied under both.
#[cfg(target_arch = "x86_64")]
const X32_BIT: i64 = 0x4000_0000;

/// The x32 numbers of denied calls that have their own (arch/x86/entry/syscalls/
/// syscall_64.tbl): `ioctl`, `ptrace`, `kexec_load`, `process_vm_readv`, `process_vm_writev`.
#[cfg(target_arch = "x86_64")]
const X32_OWN_NUMBERS: [(i64, i64); 5] = [
    (libc::SYS_ioctl, 514),
    (libc::SYS_ptrace, 521),
    (libc::SYS_kexec_load, 528),
    (libc::SYS_process_vm_readv, 539),
    (libc::SYS_process_vm_writev, 540),
];

/// `clone3`, under x32's number too.
fn unreadable_clones() -> BTreeMap<i64, Vec<SeccompRule>> {
    let mut calls = BTreeMap::from([(libc::SYS_clone3, Vec::new())]);
    #[cfg(target_arch = "x86_64")]
    calls.insert(libc::SYS_clone3 | X32_BIT, Vec::new());
    calls
}

fn denied_calls() -> BTreeMap<i64, Vec<SeccompRule>> {
    let mut calls: BTreeMap<i64, Vec<SeccompRule>> = DENIED_ALWAYS
        .iter()
        .map(|&call| (call, Vec::new()))
        .collect();
    calls.insert(libc::SYS_clone, namespace_clones());
    calls.insert(libc::SYS_ioctl, terminal_injection());
    calls.insert(libc::SYS_socket, unusual_sockets());
    calls.insert(libc::SYS_socketpair, unusual_sockets());
    #[cfg(target_arch = "x86_64")]
    {
        let aliases: Vec<(i64, Vec<SeccompRule>)> = calls
            .iter()
            .map(|(&call, rules)| (call | X32_BIT, rules.clone()))
            .collect();
        calls.extend(aliases);
        for (call, x32_number) in X32_OWN_NUMBERS {
            let rules = calls.get(&call).cloned().unwrap_or_default();
            calls.insert(x32_number | X32_BIT, rules);
        }
    }
    calls
}

fn condition(argument: u8, operation: SeccompCmpOp, value: libc::c_int) -> SeccompCondition {
    // Every compared argument is an `int` or `unsigned int` the kernel reads as 32 bits:
    // comparing 64 would let set upper bits slip past.
    SeccompCondition::new(
        argument,
        SeccompCmpArgLen::Dword,
        operation,
        u64::from(value.cast_unsigned()),
    )
    .expect("the argument index is below 6")
}

fn single(conditions: Vec<SeccompCondition>) -> SeccompRule {
    SeccompRule::new(conditions).expect("a rule has at least one condition")
}

/// `clone` with any namespace flag, one rule per flag.
fn namespace_clones() -> Vec<SeccompRule> {
    NAMESPACE_FLAGS
        .iter()
        .map(|&flag| {
            let mask = u64::from(flag.cast_unsigned());
            single(vec![condition(0, SeccompCmpOp::MaskedEq(mask), flag)])
        })
        .collect()
}

/// `ioctl` that types into the terminal or drives the console.
fn terminal_injection() -> Vec<SeccompRule> {
    [libc::TIOCSTI, libc::TIOCLINUX]
        .into_iter()
        .map(|request| {
            let request = libc::c_int::try_from(request).expect("terminal requests fit 32 bits");
            single(vec![condition(1, SeccompCmpOp::Eq, request)])
        })
        .collect()
}

/// A socket of another family, or a netlink socket other than routing.
fn unusual_sockets() -> Vec<SeccompRule> {
    let other_family = SOCKET_FAMILIES
        .iter()
        .map(|&family| condition(0, SeccompCmpOp::Ne, family))
        .collect();
    let other_netlink = vec![
        condition(0, SeccompCmpOp::Eq, libc::AF_NETLINK),
        condition(2, SeccompCmpOp::Ne, libc::NETLINK_ROUTE),
    ];
    vec![single(other_family), single(other_netlink)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_denied_call_builds_into_a_filter_for_this_machine() {
        let filter = SeccompFilter::new(
            denied_calls(),
            SeccompAction::Allow,
            SeccompAction::Errno(libc::EPERM.cast_unsigned()),
            target_arch().unwrap(),
        )
        .unwrap();
        assert!(!BpfProgram::try_from(filter).unwrap().is_empty());
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn x32_numbers_are_denied_like_their_x86_64_calls() {
        let calls = denied_calls();
        assert!(calls[&(libc::SYS_ptrace | X32_BIT)].is_empty());
        assert!(calls[&(521 | X32_BIT)].is_empty());
        assert_eq!(calls[&(514 | X32_BIT)].len(), 2);
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn x32_clone3_fails_like_clone3() {
        assert!(unreadable_clones().contains_key(&(libc::SYS_clone3 | X32_BIT)));
    }
}
