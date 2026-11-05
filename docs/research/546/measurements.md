# Measurements for the own-sandbox research

Appendix to [the report](../2026-11-05-546-own-sandbox-and-traffic-control.md) for issue
[#546](https://github.com/Androlax2/agent-cockpit/issues/546). Everything ran on 2026-11-05 on
one host, as the owner's OS user, from the research clone's ignored `target/` folder. Nothing
left the host: every server listened on loopback, and no live Torlek service, Greyproxy or
provider was contacted.

## Host

| Fact | Value | How it was read |
| --- | --- | --- |
| CPU | AMD Ryzen 7 7800X3D, 16 threads | `/proc/cpuinfo`, `nproc` |
| Kernel | 7.2.14-arch1-1 | `uname -r` |
| Landlock ABI | 10 | `landlock_create_ruleset(NULL, 0, LANDLOCK_CREATE_RULESET_VERSION)` through Python `ctypes` |
| Active LSMs | `capability,landlock,lockdown,yama,bpf` | `/sys/kernel/security/lsm` |
| Unprivileged user namespaces | allowed (`max_user_namespaces` 126678) | `/proc/sys/user/max_user_namespaces` |
| Kernel headers | `linux-api-headers` 7.3 (document Landlock ABI 9 `RESOLVE_UNIX` and ABI 10 UDP) | `/usr/include/linux/landlock.h` |
| Tools | bubblewrap 0.13.1, util-linux `unshare`, slirp4netns, socat, ApacheBench, curl, GCC 16.2.1, Python 3.14.9 | `--version` |
| Session | Torlek profile with `agents.sandbox = "off"`: host namespaces, no seccomp filter | `/proc/self/ns/*`, `/proc/self/status` |
| Managed Greyproxy | PID 3155, 24,636 kB resident, 17 threads, up 2 h 12 min, no sandboxed session using it | `/proc/3155/status`, `ps` |

## Launch latency

`spawnbench` starts a program 10 times to warm up, then 300 times, with `posix_spawn` and
`waitpid`, and refuses a non-zero exit. Two rounds:

```text
round 1
true:      n=300 median=272 p90=307 p99=351 mean=279 us
unshare:   n=300 median=1021 p90=1115 p99=5193 mean=1123 us
bwrap:     n=300 median=2037 p90=2244 p99=4298 mean=2123 us
native:    n=300 median=1020 p90=1193 p99=5378 mean=1142 us
round 2
true:      n=300 median=254 p90=330 p99=388 mean=274 us
unshare:   n=300 median=969 p90=1193 p99=4882 mean=1093 us
bwrap:     n=300 median=2035 p90=2340 p99=4511 mean=2144 us
native:    n=300 median=1031 p90=1223 p99=4401 mean=1149 us
```

Commands, with `W` the prototype's writable folder:

| Label | Command |
| --- | --- |
| `true` | `/usr/bin/true` |
| `unshare` | `unshare --user --map-current-user --mount --net --ipc --uts /usr/bin/true` |
| `bwrap` | `bwrap --unshare-all --die-with-parent --ro-bind /usr /usr --symlink usr/bin /bin --symlink usr/lib /lib --symlink usr/lib /lib64 --proc /proc --dev /dev --tmpfs /tmp /usr/bin/true` |
| `native` | `native "$W" -- /usr/bin/true` |

`native` is a measurement aid, not a design. It unshares user, mount, network, IPC and UTS
namespaces, maps the caller's uid and gid, builds a Landlock ruleset handling every filesystem
right up to ABI 5 plus TCP bind and connect and both ABI 6 scopes, allows `/usr` and `/etc`
read and execute, `/dev/null` read and write and `W` read and write, sets `no_new_privs`,
installs a 15-syscall seccomp deny list and `exec`s. A check run under it wrote and read a
file in `W` and was refused reading `~/.ssh/known_hosts`, listing `/`, reading
`/proc/self/status` and connecting to `1.1.1.1:80`.

```c
// Minimal Torlek-native sandbox prototype for #546 measurements only (never shipped).
// Usage: native WORKDIR -- PROGRAM [ARGS...]
// unshare(user, mount, net, ipc, uts) + Landlock (fs, tcp, scopes) + seccomp deny list + exec.
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <linux/filter.h>
#include <linux/landlock.h>
#include <linux/seccomp.h>
#include <sched.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/prctl.h>
#include <sys/syscall.h>
#include <unistd.h>

static void die(const char *what) { perror(what); _exit(111); }

static void write_file(const char *path, const char *text) {
    int fd = open(path, O_WRONLY);
    if (fd < 0 || write(fd, text, strlen(text)) < 0) die(path);
    close(fd);
}

static void allow_path(int ruleset, const char *path, __u64 access) {
    struct landlock_path_beneath_attr rule = {.allowed_access = access};
    rule.parent_fd = open(path, O_PATH | O_CLOEXEC);
    if (rule.parent_fd < 0) die(path);
    if (syscall(SYS_landlock_add_rule, ruleset, LANDLOCK_RULE_PATH_BENEATH, &rule, 0)) die("add_rule");
    close(rule.parent_fd);
}

#define DENY(nr) BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, nr, 0, 1), BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM)

int main(int argc, char **argv) {
    if (argc < 4 || strcmp(argv[2], "--")) { fprintf(stderr, "usage: native WORKDIR -- PROGRAM...\n"); return 2; }
    uid_t uid = getuid(); gid_t gid = getgid();
    if (unshare(CLONE_NEWUSER | CLONE_NEWNS | CLONE_NEWNET | CLONE_NEWIPC | CLONE_NEWUTS)) die("unshare");
    char map[64];
    write_file("/proc/self/setgroups", "deny");
    snprintf(map, sizeof map, "%u %u 1", uid, uid); write_file("/proc/self/uid_map", map);
    snprintf(map, sizeof map, "%u %u 1", gid, gid); write_file("/proc/self/gid_map", map);

    int abi = syscall(SYS_landlock_create_ruleset, NULL, 0, LANDLOCK_CREATE_RULESET_VERSION);
    if (abi < 6) { fprintf(stderr, "landlock abi %d < 6: refuse\n", abi); return 112; }
    __u64 read_x = LANDLOCK_ACCESS_FS_EXECUTE | LANDLOCK_ACCESS_FS_READ_FILE | LANDLOCK_ACCESS_FS_READ_DIR;
    __u64 all_fs = (LANDLOCK_ACCESS_FS_IOCTL_DEV << 1) - 1;
    struct landlock_ruleset_attr attr = {
        .handled_access_fs = all_fs,
        .handled_access_net = LANDLOCK_ACCESS_NET_BIND_TCP | LANDLOCK_ACCESS_NET_CONNECT_TCP,
        .scoped = LANDLOCK_SCOPE_ABSTRACT_UNIX_SOCKET | LANDLOCK_SCOPE_SIGNAL,
    };
    int ruleset = syscall(SYS_landlock_create_ruleset, &attr, sizeof attr, 0);
    if (ruleset < 0) die("create_ruleset");
    allow_path(ruleset, "/usr", read_x);
    allow_path(ruleset, "/etc", read_x);
    allow_path(ruleset, "/dev/null", LANDLOCK_ACCESS_FS_READ_FILE | LANDLOCK_ACCESS_FS_WRITE_FILE);
    allow_path(ruleset, argv[1], all_fs & ~LANDLOCK_ACCESS_FS_IOCTL_DEV);
    if (prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0)) die("no_new_privs");
    if (syscall(SYS_landlock_restrict_self, ruleset, 0)) die("restrict_self");
    close(ruleset);

    struct sock_filter filter[] = {
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(struct seccomp_data, nr)),
        DENY(__NR_ptrace), DENY(__NR_process_vm_readv), DENY(__NR_process_vm_writev),
        DENY(__NR_keyctl), DENY(__NR_add_key), DENY(__NR_request_key), DENY(__NR_bpf),
        DENY(__NR_perf_event_open), DENY(__NR_userfaultfd), DENY(__NR_mount), DENY(__NR_umount2),
        DENY(__NR_pivot_root), DENY(__NR_unshare), DENY(__NR_setns), DENY(__NR_io_uring_setup),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
    };
    struct sock_fprog program = {.len = sizeof filter / sizeof filter[0], .filter = filter};
    if (syscall(SYS_seccomp, SECCOMP_SET_MODE_FILTER, 0, &program)) die("seccomp");
    execv(argv[3], argv + 3);
    die("execv");
}
```

```c
// Spawn latency for #546: runs PROGRAM N times (after 10 warm-ups), prints
// median, p90, p99 and mean wall time in microseconds, refusing any non-zero exit.
// Usage: spawnbench N -- PROGRAM [ARGS...]
#define _GNU_SOURCE
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <time.h>

extern char **environ;

static double now_us(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec * 1e6 + ts.tv_nsec / 1e3;
}

static int compare(const void *a, const void *b) {
    double x = *(const double *)a, y = *(const double *)b;
    return (x > y) - (x < y);
}

static double run_once(char **command) {
    pid_t child;
    double start = now_us();
    if (posix_spawn(&child, command[0], NULL, NULL, command, environ)) { perror("spawn"); exit(1); }
    int status;
    waitpid(child, &status, 0);
    double elapsed = now_us() - start;
    if (!WIFEXITED(status) || WEXITSTATUS(status)) { fprintf(stderr, "%s exited %d\n", command[0], status); exit(1); }
    return elapsed;
}

int main(int argc, char **argv) {
    if (argc < 4 || strcmp(argv[2], "--")) { fprintf(stderr, "usage: spawnbench N -- PROGRAM...\n"); return 2; }
    int runs = atoi(argv[1]);
    char **command = argv + 3;
    for (int i = 0; i < 10; i++) run_once(command);
    double *samples = malloc(sizeof(double) * runs), total = 0;
    for (int i = 0; i < runs; i++) total += samples[i] = run_once(command);
    qsort(samples, runs, sizeof(double), compare);
    printf("n=%d median=%.0f p90=%.0f p99=%.0f mean=%.0f us\n", runs, samples[runs / 2],
           samples[runs * 9 / 10], samples[runs * 99 / 100], total / runs);
    return 0;
}
```

Build: `gcc -O2 -Wall -o native native.c && gcc -O2 -Wall -o spawnbench spawnbench.c`.

## Network paths

`netbench.sh` serves a 6-byte file and a 256 MiB file from Python's `http.server` on
`127.0.0.1`, then measures from three places: the host itself, an empty network namespace whose
`socat` forwards to a Unix socket where a host `socat` connects to the server (the bridge model
of sandbox-runtime and Greywall's outer hop), and a namespace routed by slirp4netns to the host
loopback (`10.0.2.2`). Latency is ApacheBench's mean time per request over 2000 sequential
requests without keep-alive; throughput is curl's download speed of the large file. Three runs:

```text
direct         latency_ms=0.252 throughput_MiB_s=3476
unix-bridge    latency_ms=0.669 throughput_MiB_s=1580
slirp4netns    latency_ms=0.297 throughput_MiB_s=2598
direct         latency_ms=0.252 throughput_MiB_s=3659
unix-bridge    latency_ms=0.662 throughput_MiB_s=1704
slirp4netns    latency_ms=0.296 throughput_MiB_s=2450
direct         latency_ms=0.254 throughput_MiB_s=3433
unix-bridge    latency_ms=0.669 throughput_MiB_s=1698
slirp4netns    latency_ms=0.296 throughput_MiB_s=2547
```

`fdpass.py` tests the design's listener: a child enters new user and network namespaces,
brings up `lo`, listens on `127.0.0.1:3128` and hands the socket to its parent over a
socketpair (`SCM_RIGHTS`) before `exec`ing the client. The parent stays in the host namespace,
accepts on that socket and relays each connection to the server with two threads. No process
inside the namespace forwards anything. Three runs, each beside a direct measurement:

```text
fd-passed(python relay) latency_ms=0.390 throughput_MiB_s=960  direct latency_ms=0.296
fd-passed(python relay) latency_ms=0.410 throughput_MiB_s=4073  direct latency_ms=0.253
fd-passed(python relay) latency_ms=0.379 throughput_MiB_s=4438  direct latency_ms=0.265
```

The Python relay is an upper bound for a Rust one. Bulk throughput through it varied from 960
to 4,438 MiB/s against 3,433 to 3,659 direct: the single-threaded test server and the first
run's cold cache dominate, not the path. The `socat` bridge forks a process per connection on
each side, which is most of its 0.4 ms.

```bash
#!/usr/bin/env bash
# Network path overhead for #546: one loopback HTTP server on the host, reached
# (1) directly, (2) from an empty network namespace through a Unix-socket bridge
# (the sandbox-runtime model: socat in the namespace, socat on the host), and
# (3) from a namespace whose traffic slirp4netns forwards to the host loopback.
# Measures small-request latency (ab, 2000 requests, no keep-alive) and bulk
# throughput (curl, 256 MiB). Everything stays on loopback; nothing leaves the host.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
run="$here/run"
rm -rf "$run" && mkdir -p "$run/www"
head -c 268435456 /dev/zero >"$run/www/big"
printf 'hello\n' >"$run/www/small"
port=47913
sockets="$(mktemp -d "$XDG_RUNTIME_DIR/nb.XXXX")"

python3 -I -m http.server "$port" --bind 127.0.0.1 --directory "$run/www" >/dev/null 2>&1 &
server=$!
socat "UNIX-LISTEN:$sockets/b.sock,fork" "TCP:127.0.0.1:$port" &
bridge=$!
trap 'kill $server $bridge 2>/dev/null || true; rm -rf "$sockets"' EXIT
sleep 1

measure() {
    local label="$1" url_base="$2"; shift 2
    local latency throughput
    latency=$("$@" ab -q -n 2000 -c 1 "$url_base/small" 2>/dev/null | awk '/Time per request/ {print $4; exit}')
    throughput=$("$@" curl -s -o /dev/null -w '%{speed_download}' "$url_base/big")
    printf '%-14s latency_ms=%s throughput_MiB_s=%.0f\n' "$label" "$latency" "$(awk -v b="$throughput" 'BEGIN{print b/1048576}')"
}

measure direct "http://127.0.0.1:$port"

in_bridge_ns() {
    unshare --user --map-root-user --net --fork -- bash -c '
        ip link set lo up
        socat TCP-LISTEN:8080,bind=127.0.0.1,fork,reuseaddr "UNIX-CONNECT:'"$sockets"'/b.sock" &
        sleep 0.3
        "$@"
        kill %1' _ "$@"
}
measure unix-bridge "http://127.0.0.1:8080" in_bridge_ns

in_slirp_ns() {
    unshare --user --map-current-user --net --fork -- bash -c '
        echo $$ >"'"$run"'/ns.pid"
        while [ ! -e "'"$run"'/slirp.ready" ]; do sleep 0.05; done
        "$@"' _ "$@" &
    local inner=$!
    while [ ! -s "$run/ns.pid" ]; do sleep 0.05; done
    slirp4netns --configure --mtu=65520 "$(cat "$run/ns.pid")" tap0 >/dev/null 2>&1 &
    local slirp=$!
    sleep 0.5 && touch "$run/slirp.ready"
    wait "$inner"
    kill "$slirp" 2>/dev/null || true
    rm -f "$run/ns.pid" "$run/slirp.ready"
}
measure slirp4netns "http://10.0.2.2:$port" in_slirp_ns
```

```python
"""#546 prototype: an outside process serves a listener that lives inside the agent's
network namespace. The child enters a new user + network namespace, brings up loopback,
listens on 127.0.0.1:3128 and hands that socket to the parent over a socketpair
(SCM_RIGHTS). The parent, still in the host namespace, accepts on it and relays each
connection to a host loopback server. No forwarder process runs inside the namespace.

Usage: python3 -I fdpass.py SERVER_PORT -- CLIENT [ARGS...]   (run from a non-/tmp dir)
"""

import os
import socket
import subprocess
import sys
import threading

server_port = int(sys.argv[1])
client = sys.argv[3:]
parent_end, child_end = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)
uid, gid = os.getuid(), os.getgid()

child = os.fork()
if child == 0:
    parent_end.close()
    os.unshare(os.CLONE_NEWUSER | os.CLONE_NEWNET)
    for path, text in (("setgroups", "deny"), ("uid_map", f"0 {uid} 1"), ("gid_map", f"0 {gid} 1")):
        with open(f"/proc/self/{path}", "w") as mapping:
            mapping.write(text)
    subprocess.run(["ip", "link", "set", "lo", "up"], check=True)
    listener = socket.socket()
    listener.bind(("127.0.0.1", 3128))
    listener.listen(64)
    socket.send_fds(child_end, [b"L"], [listener.fileno()])
    listener.close()
    child_end.recv(1)  # the parent is ready to accept
    os.execvp(client[0], client)

child_end.close()
_, fds, _, _ = socket.recv_fds(parent_end, 1, 1)
listener = socket.socket(fileno=fds[0])
parent_end.send(b"R")


def pump(source, sink):
    try:
        while data := source.recv(262144):
            sink.sendall(data)
    except OSError:
        pass
    finally:
        try:
            sink.shutdown(socket.SHUT_WR)
        except OSError:
            pass


def relay(inside):
    outside = socket.create_connection(("127.0.0.1", server_port))
    back = threading.Thread(target=pump, args=(outside, inside), daemon=True)
    back.start()
    pump(inside, outside)
    back.join()
    inside.close()
    outside.close()


def serve():
    while True:
        try:
            inside, _ = listener.accept()
        except OSError:
            return
        threading.Thread(target=relay, args=(inside,), daemon=True).start()


threading.Thread(target=serve, daemon=True).start()
_, status = os.waitpid(child, 0)
sys.exit(os.waitstatus_to_exitcode(status))
```

Run from a folder outside `/tmp`, with `XDG_RUNTIME_DIR` set (Unix socket paths are limited to
108 bytes): `./netbench.sh`; for the fd-passing test, start
`python3 -I -m http.server 47914 --bind 127.0.0.1 --directory run/www` and run
`python3 -I fdpass.py 47914 -- ab -q -n 2000 -c 1 http://127.0.0.1:3128/small` and
`python3 -I fdpass.py 47914 -- curl -s -o /dev/null -w '%{speed_download}' http://127.0.0.1:3128/big`.

## Not measured, and why

- Greywall's launch and its TUN path: the pinned Greywall registers with Greyproxy's fixed
  management port, which the profile's live service holds, and the live service is never a
  validation target (decision record of #495). A private Greyproxy in its own network
  namespace would make it measurable; that belongs to issue 7's disposable CI.
- Ubuntu's user-namespace restriction, WSL2 and macOS: no such host was available.
- Memory of a per-session Rust proxy: no implementation exists.

## Prior art and platform sources

Checked 2026-11-05 from primary sources where they exist. Items marked *unverified* were not
confirmed on a running system or in a primary source.

### Landlock

| ABI | Kernel | Adds |
| --- | --- | --- |
| 1 | 5.13 | Filesystem rights; renames and links across folders always denied |
| 2 | 5.19 | `FS_REFER` |
| 3 | 6.2 | `FS_TRUNCATE` |
| 4 | 6.7 | `NET_BIND_TCP`, `NET_CONNECT_TCP` |
| 5 | 6.10 | `FS_IOCTL_DEV` |
| 6 | 6.12 | Scopes: abstract Unix sockets, signals |
| 7 | 6.15 | Audit logging flags |
| 8 | 7.0 | `RESTRICT_SELF_TSYNC` |
| 9 | 7.1 | `FS_RESOLVE_UNIX` (named Unix sockets) |
| 10 | 7.2 | `NET_BIND_UDP`, `NET_CONNECT_SEND_UDP` |
| 11 | 7.3 | `RESTRICT_SELF_NO_NEW_PRIVS` |

- ABI 1 to 9: [landlock(7)](https://man7.org/linux/man-pages/man7/landlock.7.html); ABI 10 and 11:
  the [7.2](https://docs.kernel.org/7.2/userspace-api/landlock.html) and
  [7.3](https://docs.kernel.org/7.3/userspace-api/landlock.html) kernel documentation. Network
  rules match ports only ([documentation](https://docs.kernel.org/userspace-api/landlock.html)).
- A `MSG_FASTOPEN` bypass of `CONNECT_TCP` was
  [reported](https://lkml.rescloud.iu.edu/2606.2/01737.html) on 7.0.11 and
  [fixed](https://kernsec.org/pipermail/linux-security-module-archive/2026-July/058374.html) in
  July 2026; stable backports *unverified*. TCP rules do not cover MPTCP
  ([landrun](https://github.com/Zouuup/landrun)). Neither matters when the network namespace,
  not Landlock, contains the network.
- The [`landlock` crate](https://crates.io/crates/landlock) 0.4.7 (2026-07-27) stops at
  [ABI 9](https://docs.rs/landlock/latest/landlock/enum.ABI.html); its
  [`CompatLevel`](https://docs.rs/landlock/latest/landlock/enum.CompatLevel.html) can drop
  missing rights silently (`BestEffort`), report `NotEnforced` (`SoftRequirement`) or fail
  (`HardRequirement`).
- WSL2: stable [WSL 3.0.3](https://github.com/microsoft/WSL/releases/tag/3.0.3) ships kernel
  6.18.54.1; its
  [configuration](https://github.com/microsoft/WSL2-Linux-Kernel/blob/linux-msft-wsl-6.18.58.1/arch/x86/configs/config-wsl)
  enables Landlock, user namespaces and seccomp filters, which gives ABI 7. That no `lsm=`
  override is passed at boot is *unverified*.

### User namespaces

- Ubuntu 24.04 sets `kernel.apparmor_restrict_unprivileged_userns=1`
  ([LP#2046477](https://bugs.launchpad.net/bugs/2046477),
  [background](https://discourse.ubuntu.com/t/understanding-apparmor-user-namespace-restriction/58007)):
  an unconfined program's new user namespace has no capabilities, which breaks bubblewrap unless
  it has a profile with a `userns` rule or the sysctl is 0
  ([sandbox-runtime](https://github.com/anthropics/sandbox-runtime)). A `bwrap` profile shipped in
  a 24.04 update broke Flatpak and was withdrawn to `apparmor-profiles`
  ([LP#2072811](https://bugs.launchpad.net/ubuntu/noble/+source/apparmor/+bug/2072811)).
- bubblewrap 0.12.0 (2026-08-26) removed setuid mode; 0.13.1 (2026-10-14) is current
  ([releases](https://github.com/containers/bubblewrap/releases)).
- Debian allows user namespaces since bullseye
  ([release notes](https://www.debian.org/releases/bullseye/amd64/release-notes/ch-information));
  Arch's `linux` allows them and `linux-hardened` does not
  ([FS#63295](https://bugs.archlinux.org/task/63295), a weak source). WSL2's kernel lacks
  Ubuntu's sysctl ([lsm.c](https://github.com/microsoft/WSL2-Linux-Kernel/blob/linux-msft-wsl-6.18.58.1/security/apparmor/lsm.c)).

### seccomp

- [seccompiler](https://github.com/rust-vmm/seccompiler): pure Rust, x86_64, aarch64 and
  riscv64; the standalone repository is archived and development continues in the
  [rust-vmm monorepo](https://github.com/rust-vmm/rust-vmm/tree/main/seccompiler); last release
  0.5.0 (2025-03-07).
- [libseccomp](https://docs.rs/libseccomp/latest/libseccomp/) 0.4.0 binds the C library and has
  a user-notification API.
- [seccomp_unotify(2)](https://man7.org/linux/man-pages/man2/seccomp_unotify.2.html): user
  notification "must not be used to make security policy decisions" because pointer arguments
  can change between check and use.

### Peers

| Tool | Linux isolation | Network | TLS |
| --- | --- | --- | --- |
| Claude Code sandbox-runtime ([README](https://github.com/anthropics/sandbox-runtime), [docs](https://code.claude.com/docs/en/sandboxing)) | bubblewrap, no network namespace interfaces, seccomp against Unix sockets and nested namespaces | HTTP and SOCKS proxies on the host reached through `socat` over Unix sockets; domain lists; loopback, link-local and metadata refused | None unless `network.tlsTerminate` is set |
| Codex ([linux-sandbox](https://github.com/openai/codex/blob/main/codex-rs/linux-sandbox/README.md), [network-proxy](https://github.com/openai/codex/blob/main/codex-rs/network-proxy/README.md)) | bubblewrap plus seccomp in process since about March 2026; Landlock is legacy | `--unshare-net` and a TCP to Unix to TCP bridge to its proxy; domain lists | Intercepts in limited mode or with hooks; CA key in memory; trust variables per runtime ([certs.rs](https://github.com/openai/codex/blob/main/codex-rs/network-proxy/src/certs.rs)) |
| Gemini CLI ([sandbox.md](https://github.com/google-gemini/gemini-cli/blob/main/docs/cli/sandbox.md)) | Containers (Docker, Podman, gVisor, LXC); per-tool bubblewrap and seccomp | On or off | None found |
| Docker Sandboxes ([architecture](https://docs.docker.com/ai/sandboxes/architecture)) | A microVM per sandbox | Host proxy with policy, other TCP forwarded | Terminates TLS where it injects credentials |

Other launchers: [nsjail](https://github.com/google/nsjail) (namespaces, Kafel policies, cgroups,
pasta networking), [firejail](https://github.com/netblue30/firejail) (setuid root),
[minijail](https://github.com/google/minijail) (ChromeOS, Android) and
[landrun](https://github.com/Zouuup/landrun) (Landlock only, ABI 10).

### Network without root

- An empty namespace with a proxy only serves clients that honor the proxy variables; other TCP,
  UDP and DNS fail. Node's built-in clients honor them only with `NODE_USE_ENV_PROXY=1`
  ([Node](https://nodejs.org/api/cli.html#node_use_env_proxy1), 24.0 and 22.21).
- slirp4netns and [pasta](https://passt.top/passt/about/) give full L4 NAT without an allowlist;
  pasta is Podman 5's rootless default
  ([Podman](https://blog.podman.io/2024/03/podman-5-0-breaking-changes-in-detail/)). RootlessKit's
  iperf3 figures (MTU 1500 / 65520): slirp4netns 1.69 / 8.11 Gbps, pasta 0.24 / 31.9 Gbps,
  rootful veth 49.3 / 50.8 Gbps
  ([network.md](https://github.com/rootless-containers/rootlesskit/blob/master/docs/network.md));
  the pasta MTU 1500 figure looks anomalous.
- TUN with a userspace stack: [tun2proxy](https://github.com/tun2proxy/tun2proxy) (over
  [ipstack](https://github.com/narrowlink/ipstack)) can create the TUN in an unprivileged
  namespace and maps fake IPs back to names; no published overhead figures found.
- `systemd-run --user` cannot filter by address: `IPAddressDeny` needs privileged cgroup eBPF
  ([systemd-devel](https://lists.freedesktop.org/archives/systemd-devel/2022-December/048664.html)).

### TLS

- Trust variables differ per runtime: Node reads `NODE_EXTRA_CA_CERTS` only at start and
  ignores it when a client passes its own `ca`
  ([Node](https://nodejs.org/api/cli.html#node_extra_ca_certsfile)); Go reads `SSL_CERT_FILE`
  and `SSL_CERT_DIR` on Unix ([crypto/x509](https://pkg.go.dev/crypto/x509)).
- SNI-only filtering is evaded by Encrypted Client Hello ([RFC 9849](https://www.rfc-editor.org/rfc/rfc9849)),
  a missing SNI, SNI and Host mismatches over HTTP/2, and QUIC
  ([Compass Security](https://blog.compass-security.com/2025/03/bypassing-web-filters-part-4-host-header-spoofing-domain-fronting-detection-bypasses/)).

### macOS

- `sandbox-exec` is deprecated with no replacement for command-line programs
  ([apple/containerization#737](https://github.com/apple/containerization/issues/737)); Codex
  ([seatbelt.rs](https://github.com/openai/codex/blob/main/codex-rs/sandboxing/src/seatbelt.rs))
  and Claude Code still use it. A macOS 26 test was not found (*unverified*).
- Network Extension providers outside the App Store ship as system extensions, with
  notarization and user or MDM approval
  ([TN3134](https://developer.apple.com/documentation/technotes/tn3134-network-extension-provider-deployment)).
- Endpoint Security needs an Apple-granted
  [entitlement](https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.developer.endpoint-security.client),
  Full Disk Access and root
  ([result codes](https://developer.apple.com/documentation/endpointsecurity/es_new_client_result_t)).

### Resource limits

- The upstream [`user@.service`](https://github.com/systemd/systemd/blob/main/units/user@.service.in)
  delegates `pids memory cpu`; `io` and `cpuset` need a drop-in installed as root
  ([rootlesscontaine.rs](https://rootlesscontaine.rs/getting-started/common/cgroup2/)).
- WSL 2.5.1 removed cgroup v1 ([release](https://github.com/microsoft/WSL/releases/tag/2.5.1)).
