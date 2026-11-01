#!/usr/bin/env python3
"""Install and remove this checkout's skills without Bus.

Standalone add-on invoked by ``./run install-skills`` and ``./run remove-skills``.
It does not start a Bus server, read the Bus data directory, or require the
``bus`` binary. Ownership is recorded in ``$HOME/.config/bus-skills/manifest.json``.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import stat
import sys
import tempfile
from pathlib import Path

PROVIDERS = (
    ("claude", ".claude"),
    ("codex", ".agents"),
    ("cursor", ".cursor"),
    ("grok", ".grok"),
    ("kimi", ".kimi-code"),
)
IGNORED = {".DS_Store", "Thumbs.db", "__pycache__"}
SCHEMA = 1


def repo_root() -> Path:
    return Path(__file__).resolve().parents[1]


def home_dir() -> Path:
    raw = os.environ.get("HOME") or os.environ.get("USERPROFILE")
    if not raw:
        raise OSError("Home directory unavailable")
    return Path(raw)


def config_dir(home: Path) -> Path:
    return home / ".config" / "bus-skills"


def manifest_path(home: Path) -> Path:
    return config_dir(home) / "manifest.json"


def skills_source() -> Path:
    return repo_root() / "skills"


def provider_dirs() -> tuple[str, ...]:
    return tuple(directory for _, directory in PROVIDERS)


def ignored(name: str) -> bool:
    return name in IGNORED


def digest(files: list[tuple[str, bytes]]) -> str:
    hasher = hashlib.sha256()
    for path, data in files:
        encoded = path.encode()
        hasher.update(len(encoded).to_bytes(8, "little"))
        hasher.update(encoded)
        hasher.update(len(data).to_bytes(8, "little"))
        hasher.update(data)
    return hasher.hexdigest()


def skip_source(name: str) -> bool:
    return (
        name.startswith(".")
        or name.endswith(".pyc")
        or ignored(name)
        or name == "tests"
        or name.endswith("_test.py")
    )


def _walk_source(root: Path, directory: Path, grouped: dict[str, list[tuple[str, bytes, bool]]], seen: set[Path]) -> None:
    try:
        resolved = directory.resolve()
    except OSError:
        return
    if resolved in seen:
        return
    seen.add(resolved)
    for entry in sorted(directory.iterdir(), key=lambda item: item.name):
        name = entry.name
        if skip_source(name):
            continue
        if entry.is_dir():
            _walk_source(root, entry, grouped, seen)
            continue
        if not entry.is_file():
            continue
        relative = entry.relative_to(root).as_posix()
        parts = relative.split("/")
        if len(parts) < 2 or any(skip_source(part) for part in parts):
            continue
        skill_name, inner = parts[0], "/".join(parts[1:])
        mode = entry.stat().st_mode
        grouped.setdefault(skill_name, []).append((inner, entry.read_bytes(), bool(mode & 0o111)))


def shipped() -> list[dict]:
    root = skills_source()
    grouped: dict[str, list[tuple[str, bytes, bool]]] = {}
    _walk_source(root, root, grouped, set())
    skills = []
    for name in sorted(grouped):
        files = grouped[name]
        skills.append(
            {
                "name": name,
                "files": files,
                "hash": digest([(path, data) for path, data, _mode in files]),
            }
        )
    return skills


def bundle_hash(skills: list[dict]) -> str:
    return digest([(skill["name"], skill["hash"].encode()) for skill in skills])


def linked_ancestor(path: Path) -> Path | None:
    """Stop at a provider home even when it does not exist yet.

    A symlink of ``HOME`` or ``/var`` is not a skill-registry link. A symlink of
    ``.claude`` or ``skills`` is, and must not be written through.
    """
    for ancestor in (path, *path.parents):
        try:
            meta = ancestor.lstat()
        except FileNotFoundError:
            meta = None
        if meta is not None and stat.S_ISLNK(meta.st_mode):
            return ancestor
        if ancestor.name in provider_dirs():
            break
    return None


def private_dir(path: Path) -> None:
    if path.is_symlink():
        raise OSError(f"{path} is not a real directory")
    if not path.exists():
        if path.parent != path:
            private_dir(path.parent)
        try:
            path.mkdir(mode=0o700)
        except FileExistsError:
            if path.is_symlink() or not path.is_dir():
                raise OSError(f"{path} is not a real directory") from None
        return
    meta = path.lstat()
    if not stat.S_ISDIR(meta.st_mode) or stat.S_ISLNK(meta.st_mode):
        raise OSError(f"{path} is not a real directory")


def tree_files(root: Path, directory: Path, files: dict[str, bytes]) -> None:
    for entry in sorted(directory.iterdir(), key=lambda item: item.name):
        name = entry.name
        if ignored(name):
            continue
        meta = entry.lstat()
        if stat.S_ISLNK(meta.st_mode):
            raise OSError("Skill contains a symlink")
        if stat.S_ISDIR(meta.st_mode):
            tree_files(root, entry, files)
        elif stat.S_ISREG(meta.st_mode):
            relative = entry.relative_to(root).as_posix()
            files[relative] = entry.read_bytes()
        else:
            raise OSError("Skill contains a special file")


def tree_hash(root: Path) -> str:
    files: dict[str, bytes] = {}
    tree_files(root, root, files)
    return digest(list(files.items()))


def load_manifest(home: Path) -> dict:
    path = manifest_path(home)
    try:
        raw = path.read_bytes()
    except FileNotFoundError:
        return {"schema": SCHEMA, "bundle_hash": "", "installed": {}}
    try:
        manifest = json.loads(raw)
    except json.JSONDecodeError as error:
        raise OSError(f"Invalid skills manifest at {path}: {error}") from error
    if not isinstance(manifest, dict) or manifest.get("schema") != SCHEMA:
        raise OSError("Unsupported skills manifest schema")
    installed = manifest.get("installed")
    if not isinstance(installed, dict):
        raise OSError(f"Invalid skills manifest at {path}")
    return manifest


def save_manifest(home: Path, manifest: dict) -> None:
    path = manifest_path(home)
    private_dir(path.parent)
    if path.is_symlink():
        raise OSError(f"refuses to replace a symlink at {path}")
    payload = json.dumps(manifest, indent=2, sort_keys=True).encode()
    fd, temporary = tempfile.mkstemp(prefix=".manifest-", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as handle:
            handle.write(payload)
            handle.flush()
            os.fsync(handle.fileno())
        os.chmod(temporary, 0o600)
        os.replace(temporary, path)
    except Exception:
        Path(temporary).unlink(missing_ok=True)
        raise


class Lock:
    def __init__(self, home: Path) -> None:
        self.path = config_dir(home) / "install.lock"
        self.handle = None

    def __enter__(self) -> "Lock":
        import fcntl

        private_dir(self.path.parent)
        if self.path.is_symlink():
            raise OSError(f"refuses to lock through a symlink at {self.path}")
        self.handle = self.path.open("a+")
        fcntl.flock(self.handle.fileno(), fcntl.LOCK_EX)
        return self

    def __exit__(self, *_args) -> None:
        import fcntl

        if self.handle is not None:
            fcntl.flock(self.handle.fileno(), fcntl.LOCK_UN)
            self.handle.close()


def skill_state(path: Path, owned: dict | None, desired: str) -> str:
    if linked_ancestor(path) is not None:
        return "blocked-link"
    try:
        meta = path.lstat()
    except FileNotFoundError:
        return "missing"
    if not stat.S_ISDIR(meta.st_mode) or stat.S_ISLNK(meta.st_mode):
        return "conflict"
    try:
        current = tree_hash(path)
    except PermissionError:
        raise
    except OSError:
        return "conflict"
    if current == desired:
        return "installed"
    if owned is not None and current == owned.get("hash"):
        return "outdated"
    return "conflict"


def copy_files(skill: dict, stage: Path) -> None:
    stage.mkdir(mode=0o700)
    for relative, data, executable in skill["files"]:
        target = stage / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
        target.chmod(0o755 if executable else 0o644)


def leave_action(state: str) -> str:
    if state == "conflict":
        return "preserved-conflict"
    if state == "blocked-link":
        return "blocked-link"
    return "unchanged"


def install_one(home: Path, root: Path, provider: str, skill: dict, manifest: dict) -> str:
    path = root / skill["name"]
    key = f"{provider}/{skill['name']}"
    owned = manifest["installed"].get(key)
    observed = skill_state(path, owned, skill["hash"])
    if observed == "installed" and (owned is None or owned.get("hash") != skill["hash"]):
        manifest["installed"][key] = {"hash": skill["hash"], "version": manifest["bundle_hash"]}
        save_manifest(home, manifest)
    if observed in {"conflict", "installed", "blocked-link"}:
        return leave_action(observed)
    private_dir(root)
    token = f"{os.getpid()}-{os.urandom(4).hex()}"
    stage = root / f".bus-stage-{token}"
    backup = root / f".bus-backup-{token}"
    try:
        copy_files(skill, stage)
        now = skill_state(path, manifest["installed"].get(key), skill["hash"])
        if now != observed:
            return leave_action(now)
        if observed == "outdated":
            os.rename(path, backup)
        try:
            os.rename(stage, path)
        except OSError:
            if backup.exists():
                os.rename(backup, path)
            raise
        manifest["installed"][key] = {"hash": skill["hash"], "version": manifest["bundle_hash"]}
        save_manifest(home, manifest)
        if backup.exists():
            import shutil

            shutil.rmtree(backup)
        return "updated" if observed == "outdated" else "installed"
    finally:
        if stage.exists():
            import shutil

            shutil.rmtree(stage, ignore_errors=True)


def status_entries(home: Path, skills: list[dict], manifest: dict) -> list[dict]:
    entries = []
    for provider, directory in PROVIDERS:
        for skill in skills:
            path = home / directory / "skills" / skill["name"]
            key = f"{provider}/{skill['name']}"
            state = skill_state(path, manifest["installed"].get(key), skill["hash"])
            entries.append(
                {
                    "provider": provider,
                    "name": skill["name"],
                    "path": str(path),
                    "state": state,
                }
            )
    return entries


def install_all(home: Path) -> list[dict]:
    skills = shipped()
    entries: list[dict] = []
    with Lock(home):
        manifest = load_manifest(home)
        manifest["bundle_hash"] = bundle_hash(skills)
        for provider, directory in PROVIDERS:
            root = home / directory / "skills"
            try:
                for skill in skills:
                    action = install_one(home, root, provider, skill, manifest)
                    entries.append(_entry(provider, skill["name"], root / skill["name"], action))
            except OSError as error:
                entries.append(_entry(provider, "", root, "failed", str(error)))
        manifest["bundle_hash"] = bundle_hash(skills)
        save_manifest(home, manifest)
    return entries


def prune_empty(path: Path, stop: Path) -> None:
    current = path
    while current != stop and current != current.parent:
        if not current.exists():
            current = current.parent
            continue
        if any(current.iterdir()):
            break
        current.rmdir()
        current = current.parent


def remove_one(home: Path, provider: str, skill: dict, manifest: dict) -> str:
    import shutil

    key = f"{provider}/{skill['name']}"
    owned = manifest["installed"].get(key)
    if owned is None:
        return "absent"
    directory = dict(PROVIDERS)[provider]
    path = home / directory / "skills" / skill["name"]
    if linked_ancestor(path) is not None:
        return "blocked-link"
    try:
        path.lstat()
    except FileNotFoundError:
        del manifest["installed"][key]
        save_manifest(home, manifest)
        return "removed"
    try:
        current = tree_hash(path)
    except OSError:
        return "preserved-conflict"
    if current != owned.get("hash"):
        return "preserved-conflict"
    shutil.rmtree(path)
    del manifest["installed"][key]
    save_manifest(home, manifest)
    prune_empty(path.parent, home)
    return "removed"


def _entry(provider: str, name: str, path: Path, action: str, error: str = "") -> dict:
    return {
        "provider": provider,
        "name": name,
        "path": str(path),
        "action": action,
        "state": action,
        "error": error,
    }


def remove_all(home: Path) -> list[dict]:
    skills = shipped()
    known = {skill["name"]: skill for skill in skills}
    entries: list[dict] = []
    with Lock(home):
        manifest = load_manifest(home)
        for provider, directory in PROVIDERS:
            root = home / directory / "skills"
            names = {
                key.split("/", 1)[1]
                for key in list(manifest["installed"])
                if key.startswith(f"{provider}/") and "/" in key
            }
            try:
                for name in sorted(names):
                    skill = known.get(name) or {"name": name, "hash": "", "files": []}
                    action = remove_one(home, provider, skill, manifest)
                    if action == "absent":
                        continue
                    entries.append(_entry(provider, name, root / name, action))
            except OSError as error:
                entries.append(_entry(provider, "", root, "failed", str(error)))
    return entries


def summary_lines(home: Path, entries: list[dict], mode: str) -> list[tuple[str, bool]]:
    lines = []
    by_provider: dict[str, list[dict]] = {provider: [] for provider, _directory in PROVIDERS}
    for entry in entries:
        by_provider[entry["provider"]].append(entry)
    for provider, directory in PROVIDERS:
        group = by_provider[provider]
        path = str(home / directory / "skills")
        failed = [item for item in group if item.get("action") == "failed"]
        if failed:
            reason = " ".join(str(failed[0].get("error") or "install failed").split())
            lines.append((f"{provider}: failed: {reason}", True))
            continue
        if mode == "install":
            lines.append(install_line(provider, path, group))
        else:
            lines.append(remove_line(provider, path, group))
    return lines


def install_line(provider: str, path: str, group: list[dict]) -> tuple[str, bool]:
    actions = {item["action"] for item in group}
    owned = [item for item in group if item["action"] in {"installed", "updated", "unchanged"}]
    if actions and actions <= {"blocked-link"}:
        return (f"{provider}: skipped: skills path is a symlink", False)
    if not owned and "preserved-conflict" in actions:
        return (f"{provider}: skipped: user skills preserved", False)
    return (f"{provider}: installed {len(owned)} skills to {path}", False)


def remove_line(provider: str, path: str, group: list[dict]) -> tuple[str, bool]:
    removed = [item for item in group if item["action"] == "removed"]
    if removed:
        return (f"{provider}: removed {len(removed)} skills from {path}", False)
    if any(item["action"] in {"preserved-conflict", "blocked-link"} for item in group):
        return (f"{provider}: skipped: user skills preserved", False)
    return (f"{provider}: skipped: nothing installed", False)


def run_guarded(home: Path, mode: str) -> tuple[list[str], bool]:
    try:
        entries = install_all(home) if mode == "install" else remove_all(home)
    except OSError as error:
        reason = " ".join(str(error).split())
        lines = [f"{provider}: failed: {reason}" for provider, _directory in PROVIDERS]
        return lines, True
    summarized = summary_lines(home, entries, mode)
    return [line for line, _failed in summarized], any(failed for _line, failed in summarized)


def confirm(mode: str, yes: bool, skills: list[dict]) -> bool:
    if yes:
        return True
    names = [skill["name"] for skill in skills if skill["name"] != "_bus"]
    if mode == "install":
        print("Install the following set of default skills?", file=sys.stderr)
        for name in names:
            print(f"  {name}", file=sys.stderr)
        print(
            "Copies go to ~/.claude/skills, ~/.agents/skills, ~/.cursor/skills, "
            "~/.grok/skills, and ~/.kimi-code/skills; user skills are preserved.",
            file=sys.stderr,
        )
        print("Install? [y/N]", file=sys.stderr)
    else:
        print("Remove the skills this add-on installed from every provider? [y/N]", file=sys.stderr)
    answer = sys.stdin.readline().strip().lower()
    return answer in {"y", "yes"}


def print_status(home: Path) -> int:
    skills = shipped()
    try:
        manifest = load_manifest(home)
    except OSError as error:
        print(f"failed: {error}", file=sys.stderr)
        return 1
    print(json.dumps({"skills": status_entries(home, skills, manifest)}, indent=2))
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Install or remove checkout skills for every provider")
    parser.add_argument("command", choices=("install", "remove", "status"))
    parser.add_argument("--yes", action="store_true", help="Do not prompt")
    args = parser.parse_args(argv)
    try:
        home = home_dir()
    except OSError as error:
        for provider, _directory in PROVIDERS:
            print(f"{provider}: failed: {error}")
        return 1
    if args.command == "status":
        return print_status(home)
    if not confirm(args.command, args.yes, shipped()):
        print("declined", file=sys.stderr)
        return 1
    lines, failed = run_guarded(home, args.command)
    print("\n".join(lines))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
