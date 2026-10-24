"""Export run-local counters against the persistent cargo-llvm-cov objects."""
from __future__ import annotations

import contextlib
import os
import subprocess
from pathlib import Path

from coverage_run import MERGED, PROFILE_DIR
from gate_process import cleanup_signals


@contextlib.contextmanager
def merged_profile(root: Path, target: Path, env: dict, run):
    if not env.get(PROFILE_DIR):
        yield  # Legacy direct callers still let cargo-llvm-cov merge flat profiles.
        return
    directory = Path(env[PROFILE_DIR])
    profiles = sorted(directory.glob("*.profraw"))
    if not profiles:
        raise ValueError("no coverage profiles in this run")
    # cargo-llvm-cov 0.9 uses the workspace directory name for this lookup.
    # Refuse flat legacy input instead of silently mixing another run's data.
    index = target / (root.name + ".profdata")
    if list(target.glob("*.profraw")) or index.exists() or index.is_symlink():
        raise ValueError("unexpected flat coverage data; remove old data before restarting unit")
    tool = env.get("LLVM_PROFDATA")
    if not tool:
        libdir = subprocess.check_output(
            ["rustc", "--print", "target-libdir"], cwd=root,
            env={k: v for k, v in env.items() if not k.startswith(("BUS_", "HERDR_"))},
            text=True,
        ).strip()
        tool = str(Path(libdir).parent / "bin" / ("llvm-profdata.exe" if os.name == "nt" else "llvm-profdata"))
    inputs = directory / "profraw-list"
    inputs.write_text("".join(str(path) + "\n" for path in profiles), encoding="utf-8")
    merged = directory / MERGED
    if run(tool, "merge", "-sparse", "-f", str(inputs), "-o", str(merged),
           *env.get("LLVM_PROFDATA_FLAGS", "").split()):
        raise ValueError("failed to merge run coverage data")
    # With no flat raws, cargo-llvm-cov report reads this existing index. A hard
    # link is only a lookup, not a second profile copy; it also works on Windows
    # without symlink privileges. Objects remain in the persistent target.
    try:
        os.link(merged, index)
        yield
    finally:
        with cleanup_signals():
            if index.exists() and index.samefile(merged):
                index.unlink()
