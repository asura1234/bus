"""Exercise shared HOME preparation before native PTY startup, without rebuilding Bus."""
import json
import os
import select
import shutil
import signal
import socket
import struct
import subprocess
import tempfile
import time
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
fcntl = pytest.importorskip("fcntl")
pty = pytest.importorskip("pty")
termios = pytest.importorskip("termios")


@pytest.fixture(scope="module")
def home_preparer(tmp_path_factory):
    directory = tmp_path_factory.mktemp("fixture-home-helper")
    source = directory / "prepare.rs"
    helper = ROOT / "src/utils/launch_guard/paths.rs"
    source.write_text(
        f'#[path={json.dumps(str(helper))}] mod paths;\n'
        'fn main() { let home=std::env::args_os().nth(1).expect("HOME argument"); '
        'if let Err(error)=paths::prepare_fixture_home(std::path::Path::new(&home)) '
        '{ eprintln!("{error}"); std::process::exit(1); } }\n'
    )
    binary = directory / "prepare"
    rustc = shutil.which("rustc")
    assert rustc, "rustc is required for the small shared-helper probe"
    subprocess.run(["nice", "-n", "19", rustc, "--edition=2021", "--cfg", "test",
                    "--cap-lints", "allow", "-C", "codegen-units=1", str(source), "-o", str(binary)],
                   check=True, capture_output=True, text=True)
    return binary


def test_preparation_refuses_a_protected_home_before_writes(home_preparer, tmp_path):
    protected = tmp_path / "protected"
    protected.mkdir()
    env = dict(os.environ, TEST_BUS_PROTECTED_HOME=str(protected))
    result = subprocess.run([home_preparer, protected], env=env, check=False,
                            capture_output=True, text=True)
    assert result.returncode != 0
    assert "test isolation refused" in result.stderr
    assert list(protected.iterdir()) == []


def test_native_server_and_client_boot_from_prepared_home(home_preparer):
    binary = os.environ.get("BUS_TEST_BINARY")
    if not binary:
        pytest.skip("requires the gate's built Bus binary or a preserved binary")
    with tempfile.TemporaryDirectory(prefix="bn-home-", dir="/tmp") as directory:
        root = Path(directory)
        runtime = root / "runtime"
        runtime.mkdir()
        home = runtime / "home"
        config = root / "config/bus"
        config.mkdir(parents=True)
        (config / "config.toml").write_text("onboarding = false\n")
        api = runtime / "api.sock"
        shell = runtime / "api-client.sock"
        env = {"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "HOME": str(home), "USERPROFILE": str(home),
               "TEST_BUS_PROTECTED_HOME": os.environ.get("TEST_BUS_PROTECTED_HOME", str(Path.home())),
               "XDG_CONFIG_HOME": str(config.parent), "XDG_RUNTIME_DIR": str(runtime),
               "XDG_STATE_HOME": str(root / "state"), "HERDR_SOCKET_PATH": str(api),
               "SHELL": "/bin/sh", "TERM": "xterm-256color"}
        assert not home.exists()
        subprocess.run([home_preparer, home], env=env, check=True, capture_output=True)
        processes, masters = [], []

        def start(role):
            master, slave = pty.openpty()
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
            try:
                # portable-pty selects this same HOME cwd before the child starts.
                process = subprocess.Popen([binary, role], cwd=home, env=env, stdin=slave,
                                           stdout=slave, stderr=slave, start_new_session=True)
            finally:
                os.close(slave)
            processes.append(process)
            masters.append(master)
            return process, master

        def ping():
            with socket.socket(socket.AF_UNIX) as connection:
                connection.settimeout(1)
                connection.connect(str(api))
                connection.sendall(b'{"id":"fixture","method":"ping","params":{}}\n')
                return json.loads(connection.makefile().readline())

        try:
            server, server_pty = start("server")
            deadline = time.monotonic() + 5
            while not (api.exists() and shell.exists()):
                assert server.poll() is None, os.read(server_pty, 8192)
                assert time.monotonic() < deadline, "native sockets did not appear"
                select.select([], [], [], 0.02)
            assert ping()["result"]["type"] == "pong"
            client, client_pty = start("client")
            output = b""
            deadline = time.monotonic() + 5
            while b"\x1b[?2026l" not in output:
                assert client.poll() is None, output.decode(errors="replace")
                assert time.monotonic() < deadline, output.decode(errors="replace")
                if select.select([client_pty], [], [], 0.05)[0]:
                    output += os.read(client_pty, 65536)
            assert ping()["result"]["type"] == "pong"
            assert not (home / ".local/share/bus").exists(), "native fixtures never open the Bus offer"
        finally:
            for process in reversed(processes):
                if process.poll() is None:
                    os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
            for master in masters:
                os.close(master)
