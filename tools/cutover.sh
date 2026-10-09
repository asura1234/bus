#!/usr/bin/env bash
# Run from a separate terminal while Bus is running: tools/cutover.sh SESSION_ID.
# Builds first; stopping, installation and resume happen only after verification.
set -euo pipefail

readonly bus_cutover_repo="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
readonly bus_cutover_session_id=${1:-}
readonly bus_cutover_session_dir="${BUS_CUTOVER_SESSION_DIR:-$HOME/.local/share/bus/sessions/$bus_cutover_session_id}"
readonly bus_cutover_binary="$bus_cutover_repo/target/debug/bus"
readonly bus_cutover_candidate_target="$bus_cutover_repo/target/cutover-candidate"
readonly bus_cutover_candidate="$bus_cutover_candidate_target/debug/bus"
bus_cutover_branch=''
bus_cutover_backup_dir=''
bus_cutover_watch_file=''
bus_cutover_install_temp=''
bus_cutover_verified_head=''
bus_cutover_resume_on_failure=false
bus_cutover_failure_reason=''

fail() {
    bus_cutover_failure_reason=$*
    printf 'Cutover stopped: %s\n' "$bus_cutover_failure_reason" >&2
    exit 1
}

write_status() {
    [[ -n "$bus_cutover_backup_dir" ]] || return 0
    printf 'status=%s\nhead=%s\nsession=%s\nreason=%s\n' "$1" "$bus_cutover_verified_head" "$bus_cutover_session_id" "${2:-}" >"$bus_cutover_backup_dir/cutover-status.txt"
}

preflight() {
    local repo=$1 branch head remote_line remote_tip remote_ref dirty
    cd "$repo"
    branch=$(git symbolic-ref --quiet --short HEAD) || fail 'HEAD must be on a feature branch.'
    [[ "$branch" != master && "$branch" != main ]] || fail 'Cut over from a feature branch.'
    if [[ -n "$bus_cutover_branch" && "$branch" != "$bus_cutover_branch" ]]; then fail 'Branch changed during the build.'; fi
    bus_cutover_branch=$branch
    dirty=$(git status --porcelain --untracked-files=all)
    [[ -z "$dirty" ]] || fail 'The worktree or index is dirty; commit or remove those changes first.'
    head=$(git rev-parse HEAD)
    remote_line=$(git ls-remote --exit-code origin "refs/heads/$branch") ||
        fail 'Cannot verify the pushed branch tip on origin. Nothing has been stopped.'
    read -r remote_tip remote_ref <<< "$remote_line"
    [[ "$remote_ref" == "refs/heads/$branch" && "$head" == "$remote_tip" ]] ||
        fail "HEAD ($head) is not the pushed $branch tip ($remote_tip). Nothing has been stopped."
    bus_cutover_verified_head=$head
    printf 'Verified clean %s at pushed commit %s.\n' "$branch" "$head"
}

prepare_backup() {
    local binary_source=$1
    umask 077
    bus_cutover_backup_dir=$(mktemp -d /private/tmp/bus-cutover.XXXXXX)
    printf 'Backup and build-log directory: %s\n' "$bus_cutover_backup_dir"
    # Have a rollback binary even if the post-stop wait or session copy fails.
    cp -p "$binary_source" "$bus_cutover_backup_dir/bus-before-cutover"
    cmp -s "$binary_source" "$bus_cutover_backup_dir/bus-before-cutover" || fail 'Old binary backup verification failed.'
}

backup_session() {
    local session_source=$1
    cp -R "$session_source" "$bus_cutover_backup_dir/session"
    printf 'Stopped session backup completed: %s/session\n' "$bus_cutover_backup_dir"
}

build_candidate() {
    local target_dir=$1 log=$2
    env -u BUS_UPDATE_API_SCHEMA -u CARGO_BUILD_TARGET CARGO_TARGET_DIR="$target_dir" BUS_BUILD_CHANNEL=dev BUS_BUILD_ID="$bus_cutover_verified_head" LIBGHOSTTY_VT_SIMD=true cargo build --locked --bin bus >"$log" 2>&1
}

atomic_install() {
    local source=$1 destination=$2
    bus_cutover_install_temp=$(mktemp "${destination}.cutover.XXXXXX") || return 1
    cp -p "$source" "$bus_cutover_install_temp" || return 1
    cmp -s "$source" "$bus_cutover_install_temp" || return 1
    mv -f "$bus_cutover_install_temp" "$destination" || return 1
    bus_cutover_install_temp=''
    cmp -s "$source" "$destination" || return 1
}

resume_session() {
    local executable=${1:-$bus_cutover_binary}
    exec env -u BUS_DATA_DIR -u BUS_CALLBACK_DIR -u BUS_LAUNCH_ID "$executable" --dev resume "$bus_cutover_session_id"
}

recover_after_failure() {
    local reason=$1 old_copy=$2 destination=$3 recovery_binary=$3
    printf 'Cutover failed after the stop: %s\n' "$reason" >&2
    printf 'NEW BUILD WAS NOT DEPLOYED. Keeping or restoring the OLD binary, then resuming session %s.\n' "$bus_cutover_session_id" >&2
    if ! cmp -s "$old_copy" "$destination"; then
        if ! atomic_install "$old_copy" "$destination"; then
            # Still attempt to resume if the target directory cannot be written.
            printf 'Could not restore %s; resuming directly from saved old binary %s.\n' "$destination" "$old_copy" >&2
            recovery_binary=$old_copy
        fi
    fi
    write_status rolled_back "$reason"
    resume_session "$recovery_binary"
}

cleanup_transients() {
    if [[ -n "$bus_cutover_watch_file" ]]; then rm -f "$bus_cutover_watch_file"; fi
    if [[ -n "$bus_cutover_install_temp" ]]; then rm -f "$bus_cutover_install_temp"; fi
}

on_exit() {
    local status=$1 reason
    trap - EXIT INT TERM
    set +e
    cleanup_transients
    if [[ -n "$bus_cutover_backup_dir" ]]; then write_status not_installed "${bus_cutover_failure_reason:-exit $status}"; fi
    if [[ "$bus_cutover_resume_on_failure" == true ]]; then
        reason=${bus_cutover_failure_reason:-"A command failed (exit $status); see the error above."}
        recover_after_failure "$reason" "$bus_cutover_backup_dir/bus-before-cutover" "$bus_cutover_binary"
    fi
    exit "$status"
}

# Root membership comes from THIS session's current socket/lock inodes, never
# from users of target/debug/bus. Unrelated old server processes are ignored.
# Descendants stay tracked after reparenting; PID start times prevent PID reuse.
# This helper only observes processes; it never sends a signal.
watch_processes() {
    python3 - "$1" "$bus_cutover_session_dir" "$bus_cutover_watch_file" <<'PY'
import json
import os
from pathlib import Path
import subprocess
import sys
import time

mode, session_dir, manifest = sys.argv[1:]

def processes():
    output = subprocess.check_output(
        ["ps", "-axo", "pid=,ppid=,pgid=,stat=,lstart="], text=True, timeout=10
    )
    result = {}
    for line in output.splitlines():
        pid, parent, group, status, born = line.split(maxsplit=4)
        if not status.startswith("Z"):
            result[int(pid)] = (int(parent), int(group), born)
    return result

def session_roots():
    root = Path(session_dir)
    paths = [root / "coordinator.lock", root / "dev-control.lock"]
    paths.extend(root.rglob("*.sock"))
    paths = sorted({str(path) for path in paths if path.exists()})
    if not paths:
        return set()
    result = subprocess.run(
        ["lsof", "-nP", "-t", *paths], capture_output=True, text=True, timeout=10
    )
    if result.returncode not in (0, 1) or result.stderr.strip():
        raise RuntimeError(f"Cannot inspect this session's socket/lock owners: {result.stderr.strip()}")
    return {int(pid) for pid in result.stdout.split()}

def extend(rows, watched, groups):
    alive = {pid for pid, born in watched.items() if pid in rows and rows[pid][2] == born}
    roots = session_roots()
    reused = {pid for pid, born in watched.items() if pid in rows and rows[pid][2] != born} - roots
    owned_groups = groups - reused
    candidates = roots | alive
    while True:
        extra = {pid for pid, (parent, group, _) in rows.items()
                 if pid not in reused and (parent in candidates or group in owned_groups)}
        expanded = candidates | extra
        if expanded == candidates:
            break
        candidates = expanded
    for pid in candidates & rows.keys():
        watched[pid] = rows[pid][2]
        if rows[pid][1] != os.getpgrp():
            groups.add(rows[pid][1])
    return {pid for pid, born in watched.items() if pid in rows and rows[pid][2] == born}

try:
    if mode == "snapshot":
        watched, groups = {}, set()
        for _ in range(2):
            extend(processes(), watched, groups)
        if not watched:
            raise RuntimeError("Cannot identify this session's processes from its socket/lock owners. Nothing has been stopped.")
        with open(manifest, "w", encoding="utf-8") as output:
            json.dump({"pids": watched, "groups": sorted(groups)}, output)
        print(f"Watching {len(watched)} processes belonging to this session; unrelated old Bus servers are excluded.")
    elif mode == "wait":
        with open(manifest, encoding="utf-8") as source:
            saved = json.load(source)
        watched = {int(pid): born for pid, born in saved["pids"].items()}
        groups = set(saved["groups"])
        deadline = time.monotonic() + 60
        while True:
            alive = extend(processes(), watched, groups)
            if not alive:
                print("This session's server, client, and tracked pane processes have exited.")
                break
            if time.monotonic() >= deadline:
                raise RuntimeError(
                    "Timed out after 60 seconds waiting for this session's server/client/agent "
                    f"processes to exit. Remaining PIDs: {', '.join(map(str, sorted(alive)))}. "
                    "The old binary will be kept and the saved session resumed."
                )
            time.sleep(0.25)
    else:
        raise RuntimeError("Unknown process-watch operation.")
except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
    sys.exit(f"Cutover stopped: {error}")
PY
}

main() {
    local tool stop_output build_status built_head
    trap 'on_exit "$?"' EXIT
    trap 'bus_cutover_failure_reason="Interrupted by Ctrl+C."; exit 130' INT
    trap 'bus_cutover_failure_reason="Interrupted by a termination signal."; exit 143' TERM
    printf 'Step 1: Check the branch, pushed commit, and clean worktree.\n'
    [[ "$bus_cutover_session_id" =~ ^[0-9a-f]{16}$ ]] || fail 'Usage: tools/cutover.sh SESSION_ID (16 lowercase hex digits).'
    cd "$bus_cutover_repo"
    preflight "$bus_cutover_repo"
    for tool in cargo python3 lsof ps; do
        command -v "$tool" >/dev/null || fail "Required command is missing: $tool. Nothing has been stopped."
    done
    [[ -x "$bus_cutover_binary" ]] || fail 'The old target/debug/bus executable is missing.'
    [[ -d "$bus_cutover_session_dir" ]] || fail "Session $bus_cutover_session_id is missing."
    umask 077
    prepare_backup "$bus_cutover_binary"
    built_head=$bus_cutover_verified_head

    printf 'Step 1 continued: Prebuild the new binary in target/cutover-candidate while the session stays running.\n'
    if build_candidate "$bus_cutover_candidate_target" "$bus_cutover_backup_dir/build.log"; then
        cat "$bus_cutover_backup_dir/build.log"
    else
        build_status=$?
        cat "$bus_cutover_backup_dir/build.log" >&2
        fail "Prebuild failed (exit $build_status). Nothing has been stopped; the old binary is unchanged."
    fi
    [[ -x "$bus_cutover_candidate" ]] || fail 'Prebuild did not produce an executable candidate. Nothing has been stopped.'
    python3 tools/cutover.py record --binary "$bus_cutover_candidate" --head "$built_head" --manifest "$bus_cutover_backup_dir/candidate.json" || fail 'Candidate revision verification failed. Nothing has been stopped.'
    write_status candidate_verified
    printf 'Recheck the pushed clean commit after the prebuild.\n'
    preflight "$bus_cutover_repo"
    [[ "$built_head" == "$bus_cutover_verified_head" ]] || fail 'HEAD changed during the build. Nothing has been stopped.'
    cmp -s "$bus_cutover_binary" "$bus_cutover_backup_dir/bus-before-cutover" || fail 'The old binary changed during the build. Nothing has been stopped.'
    bus_cutover_watch_file=$(mktemp /private/tmp/bus-cutover-processes.XXXXXX)
    watch_processes snapshot

    printf 'Step 2: Stop session %s with the old binary, then wait for only its processes to exit.\n' "$bus_cutover_session_id"
    # Arm recovery before requesting the stop: even a partially completed stop
    # or an unparseable response must attempt to resume the old session.
    bus_cutover_resume_on_failure=true
    write_status stop_requested
    if ! stop_output=$(env BUS_DATA_DIR="$bus_cutover_session_dir" "$bus_cutover_binary" stop); then
        printf '%s\n' "$stop_output" >&2
        fail 'The old stop command failed or timed out.'
    fi
    printf '%s\n' "$stop_output"
    if ! printf '%s\n' "$stop_output" | python3 -c 'import json, sys; sys.exit(0 if json.load(sys.stdin).get("stopped") is True else 1)'; then
        fail 'The old binary did not confirm stopped:true.'
    fi
    write_status waiting_for_exit
    watch_processes wait

    printf 'Step 3: Back up the stopped session in the printed private backup directory.\n'
    backup_session "$bus_cutover_session_dir"
    printf 'Step 4: Atomically install the prebuilt binary into target/debug/bus and verify it.\n'
    atomic_install "$bus_cutover_candidate" "$bus_cutover_binary" || fail 'Candidate installation or byte verification failed.'
    cleanup_transients
    python3 tools/cutover.py verify --binary "$bus_cutover_binary" --manifest "$bus_cutover_backup_dir/candidate.json" || fail 'Installed revision or bytes differ from the candidate.'
    write_status installed_verified
    printf 'Step 5: Resume the same saved session with the verified new binary.\n'
    resume_session
}

# Sourcing exposes helpers for safe fixture checks; it never starts the cutover.
if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
    main "$@"
fi
