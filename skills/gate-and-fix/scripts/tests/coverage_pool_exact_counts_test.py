"""Pooled profiles keep exact execution counts under heavy concurrency."""
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import bus_quality as quality  # noqa: E402

TESTS = 160
FAILING = 10


@unittest.skipUnless(
    shutil.which("cargo") and shutil.which("cargo-llvm-cov")
    and shutil.which("cargo-nextest"), "requires the Rust coverage tools and nextest",
)
class CoveragePoolExactCountsTest(unittest.TestCase):
    def test_every_process_including_failing_tests_is_counted_once(self):
        with tempfile.TemporaryDirectory(prefix="bus-cov-exact-") as folder:
            root = Path(folder)
            (root / "src").mkdir()
            (root / "Cargo.toml").write_text(
                '[package]\nname = "bus"\nversion = "0.0.0"\n'
                'edition = "2021"\n[workspace]\n'
            )
            cases = "".join(
                f"#[test] fn case_{n}() {{ super::tick(); "
                + ("panic!(\"expected failure\"); " if n < FAILING else "")
                + "}\n"
                for n in range(TESTS)
            )
            (root / "src/main.rs").write_text(
                "#[inline(never)] fn tick() { std::hint::black_box(1); }\n"
                "fn main() {}\n#[cfg(test)] mod tests {\n" + cases + "}\n"
            )
            shutil.copyfile(quality.ROOT / "rust-toolchain.toml", root / "rust-toolchain.toml")
            env = {
                key: value for key, value in os.environ.items()
                if not key.startswith(("BUS_", "HERDR_", "CARGO_LLVM_COV", "__CARGO_LLVM_COV"))
                and key not in {"RUSTFLAGS", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER",
                                "LLVM_PROFILE_FILE", "LLVM_PROFILE_FILE_NAME"}
            }
            env.update(
                CARGO_TARGET_DIR=str(root / "target"), CARGO_INCREMENTAL="0",
                CARGO_BUILD_JOBS="4", NEXTEST_TEST_THREADS="5",
                LLVM_PROFILE_FILE_NAME=quality.profile_file_name("unit"),
            )
            subprocess.run(["cargo", "generate-lockfile", "--offline"], cwd=root,
                           env=env, check=True, capture_output=True, timeout=60)
            subprocess.run(
                ["cargo", "llvm-cov", "nextest", "--no-report", "--no-fail-fast",
                 "--bin", "bus"],
                cwd=root, env=env, capture_output=True, timeout=600,
            )
            profiles = list((root / "target/llvm-cov-target").glob("*.profraw"))
            self.assertLessEqual(len(profiles), 4)
            report = root / "coverage.json"
            subprocess.run(
                ["cargo", "llvm-cov", "report", "--json", "--output-path", str(report)],
                cwd=root, env=env, check=True, capture_output=True, timeout=120,
            )
            functions = json.loads(report.read_text())["data"][0]["functions"]
            ticks = [entry["count"] for entry in functions
                     if "4tick" in entry["name"] and "case_" not in entry["name"]]
            self.assertEqual(ticks, [TESTS])


if __name__ == "__main__":
    unittest.main()
