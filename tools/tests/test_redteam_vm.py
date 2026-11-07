import os
import shutil
import signal
import subprocess
import tempfile
import unittest
from hashlib import sha256
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
SCRIPT = REPO / "tools/redteam-vm.sh"
FAKE_IMAGE_BYTES = b"not a real qcow2"

FAKE_CURL = """#!/usr/bin/env bash
while [[ $# -gt 0 ]]; do
    [[ $1 == --output ]] && output=$2
    shift
done
printf '%s' "$FAKE_IMAGE_BYTES" >"$output"
"""

FAKE_QEMU = """#!/usr/bin/env bash
printf '%s\\n' "$@" >"$FAKE_LOG_DIR/qemu-arguments"
while [[ $# -gt 0 ]]; do
    [[ $1 == -pidfile ]] && pid_file=$2
    shift
done
(exec -a "qemu-system-x86_64 -name torlek-redteam " sleep 60 >/dev/null 2>&1) &
echo $! >"$pid_file"
"""

FAKE_QEMU_IMG = """#!/usr/bin/env bash
printf '%s\\n' "$*" >>"$FAKE_LOG_DIR/qemu-img-calls"
[[ $1 == create ]] && touch "${@: -2:1}"
exit 0
"""

FAKE_SSH = """#!/usr/bin/env bash
printf '%s\\n' "$*" >>"$FAKE_LOG_DIR/ssh-calls"
printf '%s\\n' "${SSH_AUTH_SOCK-unset}" >>"$FAKE_LOG_DIR/ssh-agent-socket"
if [[ "$*" == *poweroff* && -f "$TORLEK_REDTEAM_DIR/qemu.pid" ]]; then
    kill "$(<"$TORLEK_REDTEAM_DIR/qemu.pid")"
fi
exit 0
"""


def write_executable(path, content):
    path.write_text(content)
    path.chmod(0o755)


@unittest.skipUnless(shutil.which("bsdtar"), "bsdtar builds the cloud-init seed")
@unittest.skipUnless(shutil.which("ssh-keygen"), "ssh-keygen makes the throwaway key")
class RedTeamVmTests(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix="redteam-vm-test-"))
        self.addCleanup(shutil.rmtree, self.root, ignore_errors=True)
        self.state_dir = self.root / "state"
        self.log_dir = self.root / "logs"
        self.log_dir.mkdir()
        fakes = self.root / "fakes"
        fakes.mkdir()
        write_executable(fakes / "curl", FAKE_CURL)
        write_executable(fakes / "qemu-system-x86_64", FAKE_QEMU)
        write_executable(fakes / "qemu-img", FAKE_QEMU_IMG)
        write_executable(fakes / "ssh", FAKE_SSH)
        self.kvm_device = self.root / "kvm"
        self.kvm_device.touch()
        self.environment = {
            **os.environ,
            "PATH": f"{fakes}:{os.environ['PATH']}",
            "HOME": str(self.root / "home"),
            "TORLEK_REDTEAM_DIR": str(self.state_dir),
            "TORLEK_REDTEAM_KVM_DEVICE": str(self.kvm_device),
            "TORLEK_REDTEAM_IMAGE_URL": "https://example.invalid/fake.qcow2",
            "TORLEK_REDTEAM_IMAGE_SHA256": sha256(FAKE_IMAGE_BYTES).hexdigest(),
            "TORLEK_REDTEAM_BOOT_TIMEOUT": "20",
            "FAKE_IMAGE_BYTES": FAKE_IMAGE_BYTES.decode(),
            "FAKE_LOG_DIR": str(self.log_dir),
            "SSH_AUTH_SOCK": "/run/user/1000/the-owners-agent.sock",
        }
        self.addCleanup(self.kill_fake_guest)

    def kill_fake_guest(self):
        pid_file = self.state_dir / "qemu.pid"
        if pid_file.exists():
            try:
                os.kill(int(pid_file.read_text()), signal.SIGKILL)
            except ProcessLookupError:
                pass

    def run_script(self, *arguments, **environment_overrides):
        return subprocess.run(
            [str(SCRIPT), *arguments],
            capture_output=True,
            text=True,
            env={**self.environment, **environment_overrides},
        )

    def create_guest(self):
        self.assertEqual(self.run_script("fetch").returncode, 0)
        result = self.run_script("create")
        self.assertEqual(result.returncode, 0, result.stderr)

    def qemu_arguments(self):
        return (self.log_dir / "qemu-arguments").read_text().splitlines()

    def test_fetch_keeps_an_image_whose_checksum_matches(self):
        result = self.run_script("fetch")
        self.assertEqual(result.returncode, 0, result.stderr)
        image = self.state_dir / "Arch-Linux-x86_64-cloudimg-20261101.620452.qcow2"
        self.assertEqual(image.read_bytes(), FAKE_IMAGE_BYTES)
        self.assertEqual(image.stat().st_mode & 0o777, 0o400)

    def test_fetch_keeps_nothing_when_the_checksum_differs(self):
        result = self.run_script("fetch", TORLEK_REDTEAM_IMAGE_SHA256="0" * 64)
        self.assertEqual(result.returncode, 1)
        self.assertIn("image checksum mismatch", result.stderr)
        self.assertEqual(list(self.state_dir.iterdir()), [])

    def test_create_refuses_without_a_verified_image(self):
        result = self.run_script("create")
        self.assertEqual(result.returncode, 1)
        self.assertIn("no verified image", result.stderr)

    def test_create_refuses_when_the_guest_exists(self):
        self.create_guest()
        result = self.run_script("create")
        self.assertEqual(result.returncode, 1)
        self.assertIn("a guest already exists", result.stderr)

    def test_create_takes_the_clean_snapshot_after_provisioning(self):
        self.create_guest()
        qemu_img_calls = (self.log_dir / "qemu-img-calls").read_text().splitlines()
        self.assertTrue(qemu_img_calls[-1].startswith("snapshot -c clean "))
        ssh_calls = (self.log_dir / "ssh-calls").read_text()
        self.assertIn("pacman -Syu", ssh_calls)
        self.assertIn("releases/download", ssh_calls)

    def test_the_seed_holds_only_the_throwaway_public_key(self):
        self.create_guest()
        user_data = subprocess.run(
            ["bsdtar", "-xOf", str(self.state_dir / "seed.iso"), "user-data"],
            capture_output=True,
            text=True,
        ).stdout
        public_key = (self.state_dir / "id_ed25519.pub").read_text().strip()
        self.assertIn(public_key, user_data)
        self.assertIn("torlek-redteam-throwaway", user_data)
        self.assertNotIn("PRIVATE KEY", user_data)
        self.assertNotIn(str(self.root / "home"), user_data)
        self.assertEqual((self.state_dir / "id_ed25519").stat().st_mode & 0o777, 0o600)

    def test_start_shares_nothing_with_the_host(self):
        self.create_guest()
        self.assertEqual(self.run_script("start").returncode, 0)
        arguments = self.qemu_arguments()
        for forbidden in ("-virtfs", "-fsdev", "-usb", "-snapshot", "-nic", "-chardev"):
            self.assertNotIn(forbidden, arguments)
        self.assertNotIn("virtio-9p", " ".join(arguments))
        self.assertNotIn("vhost-user-fs", " ".join(arguments))
        drive_paths = [
            argument.split("file=")[1].split(",")[0]
            for argument in arguments
            if "file=" in argument
        ]
        self.assertEqual(len(drive_paths), 2)
        for drive_path in drive_paths:
            self.assertTrue(drive_path.startswith(str(self.state_dir) + "/"), drive_path)

    def test_start_forwards_ssh_on_the_host_loopback_only(self):
        self.create_guest()
        self.run_script("start")
        network = next(
            argument for argument in self.qemu_arguments() if argument.startswith("user,")
        )
        self.assertIn("hostfwd=tcp:127.0.0.1:2222-:22", network)

    def test_start_says_what_the_guest_network_reaches(self):
        self.create_guest()
        result = self.run_script("start")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("host loopback services", result.stderr)

    def test_start_refuses_an_unknown_option(self):
        self.create_guest()
        result = self.run_script("start", "--online")
        self.assertEqual(result.returncode, 1)
        self.assertIn("unknown start option", result.stderr)

    def test_start_refuses_without_kvm(self):
        self.create_guest()
        result = self.run_script("start", TORLEK_REDTEAM_KVM_DEVICE=str(self.root / "missing"))
        self.assertEqual(result.returncode, 1)
        self.assertIn("is not readable and writable", result.stderr)

    def test_ssh_uses_no_host_configuration_agent_or_keys(self):
        self.create_guest()
        self.run_script("start")
        self.run_script("ssh", "true")
        last_call = (self.log_dir / "ssh-calls").read_text().splitlines()[-1]
        self.assertIn("-F /dev/null", last_call)
        self.assertIn("IdentityAgent=none", last_call)
        self.assertIn("IdentitiesOnly=yes", last_call)
        self.assertIn("ForwardAgent=no", last_call)
        self.assertIn(f"UserKnownHostsFile={self.state_dir}/known_hosts", last_call)
        self.assertIn(f"-i {self.state_dir}/id_ed25519", last_call)
        self.assertEqual(set((self.log_dir / "ssh-agent-socket").read_text().split()), {"unset"})

    def test_check_refuses_when_the_guest_is_not_running(self):
        self.create_guest()
        result = self.run_script("check")
        self.assertEqual(result.returncode, 1)
        self.assertIn("the guest is not running", result.stderr)

    def test_a_stale_pid_file_is_not_a_running_guest(self):
        self.create_guest()
        (self.state_dir / "qemu.pid").write_text(str(os.getpid()))
        result = self.run_script("start")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_check_tells_the_guest_the_forward_port(self):
        self.create_guest()
        self.run_script("start", TORLEK_REDTEAM_SSH_PORT="2300")
        self.run_script("check", TORLEK_REDTEAM_SSH_PORT="2300")
        last_call = (self.log_dir / "ssh-calls").read_text().splitlines()[-1]
        self.assertIn("TORLEK_REDTEAM_SSH_PORT=2300", last_call)

    def test_check_runs_the_isolation_script_as_the_throwaway_user(self):
        self.create_guest()
        self.run_script("start")
        self.assertEqual(self.run_script("check").returncode, 0)
        last_call = (self.log_dir / "ssh-calls").read_text().splitlines()[-1]
        self.assertIn("redteam@127.0.0.1", last_call)
        self.assertIn("marker-after-clean", last_call)

    def test_reset_restores_the_clean_snapshot(self):
        self.create_guest()
        self.run_script("start")
        result = self.run_script("reset")
        self.assertEqual(result.returncode, 0, result.stderr)
        qemu_img_calls = (self.log_dir / "qemu-img-calls").read_text().splitlines()
        self.assertTrue(qemu_img_calls[-1].startswith("snapshot -a clean "))

    def test_destroy_removes_the_guest_and_keeps_the_image_and_unrelated_files(self):
        self.create_guest()
        unrelated_file = self.state_dir / "owners-notes.txt"
        unrelated_file.write_text("keep")
        self.assertEqual(self.run_script("destroy").returncode, 0)
        remaining = sorted(path.name for path in self.state_dir.iterdir())
        self.assertEqual(
            remaining, ["Arch-Linux-x86_64-cloudimg-20261101.620452.qcow2", "owners-notes.txt"]
        )

    def test_destroy_all_removes_the_image_and_the_empty_folder(self):
        self.create_guest()
        self.assertEqual(self.run_script("destroy", "--all").returncode, 0)
        self.assertFalse(self.state_dir.exists())

    def test_the_state_folder_defaults_to_the_torlek_redteam_folder_in_the_data_dir(self):
        environment = {
            key: value for key, value in self.environment.items() if key != "TORLEK_REDTEAM_DIR"
        }
        result = subprocess.run(
            [str(SCRIPT), "status"], capture_output=True, text=True, env=environment
        )
        self.assertIn(f"state dir: {self.root}/home/.local/share/torlek-redteam", result.stdout)

    def test_an_unknown_command_prints_the_usage(self):
        result = self.run_script("share-home")
        self.assertEqual(result.returncode, 1)
        self.assertIn("unknown command", result.stderr)


if __name__ == "__main__":
    unittest.main()
