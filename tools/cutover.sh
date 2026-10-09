#!/usr/bin/env bash
# Run from a separate terminal while Bus is running: tools/cutover.sh SESSION_ID.
# Builds first; stopping, installation and resume happen only after verification.
set -Eeuo pipefail

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
bus_cutover_step=initialize
bus_cutover_command=''
bus_cutover_exit_code=''
bus_cutover_stderr=''

# Capture each operation synchronously so recovery cannot race a tee process
# or overwrite the failing command's stderr. Helpers must propagate errors
# explicitly, since Bash disables errexit inside an if condition.
run_step() {
    bus_cutover_step=$1
    shift
    printf -v bus_cutover_command '%q ' "$@"
    local out="$bus_cutover_backup_dir/$bus_cutover_step.stdout.log"
    local err="$bus_cutover_backup_dir/$bus_cutover_step.stderr.log" status
    printf '\nstep=%s command=%s\n' "$bus_cutover_step" "$bus_cutover_command" >>"$bus_cutover_backup_dir/cutover.log"
    if "$@" >"$out" 2>"$err"; then status=0; else status=$?; fi
    cat "$out" "$err" >>"$bus_cutover_backup_dir/cutover.log"
    cat "$out"
    cat "$err" >&2
    if [[ "$status" != 0 ]]; then
        bus_cutover_exit_code=$status
        bus_cutover_stderr=$(cat "$err")
        fail "Step $bus_cutover_step failed (exit $status): $bus_cutover_command${bus_cutover_stderr:+: $bus_cutover_stderr}"
    fi
}

on_error() {
    bus_cutover_exit_code=$1
    bus_cutover_command=$2
    bus_cutover_stderr=$(cat "$bus_cutover_backup_dir/unhandled.stderr.log")
    bus_cutover_failure_reason="Step $bus_cutover_step failed (exit $1): $2${bus_cutover_stderr:+: $bus_cutover_stderr}"
    exit "$1"
}

fail() {
    bus_cutover_failure_reason=$*
    if [[ -n "$bus_cutover_backup_dir" ]]; then
        printf 'failure=%s\n' "$bus_cutover_failure_reason" >>"$bus_cutover_backup_dir/cutover.log"
    fi
    printf 'Cutover stopped: %s\n' "$bus_cutover_failure_reason" >&4
    exit 1
}

write_status() {
    [[ -n "$bus_cutover_backup_dir" ]] || return 0
    printf 'status=%s\nhead=%s\nsession=%s\nstep=%s\ncommand=%s\nexit_code=%s\nreason=%s\nstderr_begin\n%s\nstderr_end\n' "$1" "$bus_cutover_verified_head" "$bus_cutover_session_id" "$bus_cutover_step" "$bus_cutover_command" "$bus_cutover_exit_code" "${2:-}" "$bus_cutover_stderr" >"$bus_cutover_backup_dir/cutover-status.txt"
}

preflight() {
    local repo=$1 branch head remote_line remote_tip remote_ref dirty
    cd "$repo" || return $?
    branch=$(git symbolic-ref --quiet --short HEAD) || fail 'HEAD must be on a feature branch.'
    [[ "$branch" != master && "$branch" != main ]] || fail 'Cut over from a feature branch.'
    if [[ -n "$bus_cutover_branch" && "$branch" != "$bus_cutover_branch" ]]; then fail 'Branch changed during the build.'; fi
    bus_cutover_branch=$branch
    dirty=$(git status --porcelain --untracked-files=all) || return $?
    [[ -z "$dirty" ]] || fail 'The worktree or index is dirty; commit or remove those changes first.'
    head=$(git rev-parse HEAD) || return $?
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
    # Have a rollback binary even if the post-stop wait or session copy fails.
    cp -p "$binary_source" "$bus_cutover_backup_dir/bus-before-cutover" || return $?
    cmp -s "$binary_source" "$bus_cutover_backup_dir/bus-before-cutover" || fail 'Old binary backup verification failed.'
}

backup_session() {
    local session_source=$1
    python3 "$bus_cutover_repo/tools/cutover.py" backup --source "$session_source" --destination "$bus_cutover_backup_dir/session" || return $?
    printf 'Stopped session backup completed: %s/session\n' "$bus_cutover_backup_dir"
}

build_candidate() {
    local target_dir=$1 log=$2 status
    if env -u BUS_UPDATE_API_SCHEMA -u CARGO_BUILD_TARGET CARGO_TARGET_DIR="$target_dir" BUS_BUILD_CHANNEL=dev BUS_BUILD_ID="$bus_cutover_verified_head" LIBGHOSTTY_VT_SIMD=true cargo build --locked --bin bus >"$log" 2>&1; then
        return 0
    else
        status=$?
        cat "$log" >&2
        return "$status"
    fi
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
    local executable=${1:-$bus_cutover_binary} status
    if env -u BUS_DATA_DIR -u BUS_CALLBACK_DIR -u BUS_LAUNCH_ID "$executable" --dev resume "$bus_cutover_session_id" 2>"$bus_cutover_backup_dir/resume.stderr.log"; then
        return 0
    else
        status=$?
        bus_cutover_step=resume_session
        bus_cutover_command="$executable --dev resume $bus_cutover_session_id"
        bus_cutover_exit_code=$status
        bus_cutover_stderr=$(cat "$bus_cutover_backup_dir/resume.stderr.log")
        cat "$bus_cutover_backup_dir/resume.stderr.log" >>"$bus_cutover_backup_dir/cutover.log"
        if [[ -n "$bus_cutover_failure_reason" ]]; then
            # Retain the original failed deployment step, even when recovery
            # itself fails. Both errors must survive in the status receipt.
            printf '\nrecovery_step=resume_session\nrecovery_exit_code=%s\nrecovery_stderr_begin\n%s\nrecovery_stderr_end\n' "$status" "$bus_cutover_stderr" >>"$bus_cutover_backup_dir/cutover-status.txt"
        else
            write_status resume_failed "Session resume failed (exit $status): $bus_cutover_stderr"
        fi
        cat "$bus_cutover_backup_dir/resume.stderr.log" >&4
        return "$status"
    fi
}

recover_after_failure() {
    local reason=$1 old_copy=$2 destination=$3 recovery_binary=$3
    printf 'Cutover failed after the stop: %s\n' "$reason" >&4
    printf 'NEW BUILD WAS NOT DEPLOYED. Keeping or restoring the OLD binary, then resuming session %s.\n' "$bus_cutover_session_id" >&4
    if ! cmp -s "$old_copy" "$destination"; then
        if ! atomic_install "$old_copy" "$destination" 2>"$bus_cutover_backup_dir/recovery-install.stderr.log"; then
            # Still attempt to resume if the target directory cannot be written.
            printf 'Could not restore %s; resuming directly from saved old binary %s.\n' "$destination" "$old_copy" >&4
            recovery_binary=$old_copy
        fi
    fi
    write_status rolled_back "$reason"
    if [[ -f "$bus_cutover_backup_dir/recovery-install.stderr.log" ]]; then
        cat "$bus_cutover_backup_dir/recovery-install.stderr.log" >>"$bus_cutover_backup_dir/cutover.log"
        printf '\nrecovery_install_stderr_begin\n' >>"$bus_cutover_backup_dir/cutover-status.txt"
        cat "$bus_cutover_backup_dir/recovery-install.stderr.log" >>"$bus_cutover_backup_dir/cutover-status.txt"
        printf '\nrecovery_install_stderr_end\n' >>"$bus_cutover_backup_dir/cutover-status.txt"
    fi
    resume_session "$recovery_binary"
}

cleanup_transients() {
    if [[ -n "$bus_cutover_install_temp" ]]; then rm -f "$bus_cutover_install_temp"; fi
}

on_exit() {
    local status=$1 reason
    trap - EXIT INT TERM ERR
    set +e
    cleanup_transients
    if [[ -n "$bus_cutover_backup_dir" ]]; then
        if [[ -z "$bus_cutover_exit_code" ]]; then
            bus_cutover_exit_code=$status
            if [[ -f "$bus_cutover_backup_dir/$bus_cutover_step.stderr.log" ]]; then
                bus_cutover_stderr=$(cat "$bus_cutover_backup_dir/$bus_cutover_step.stderr.log")
                cat "$bus_cutover_backup_dir/$bus_cutover_step.stderr.log" >>"$bus_cutover_backup_dir/cutover.log"
            fi
        fi
        cat "$bus_cutover_backup_dir/unhandled.stderr.log" >>"$bus_cutover_backup_dir/cutover.log"
        cat "$bus_cutover_backup_dir/unhandled.stderr.log" >&4
        write_status not_installed "${bus_cutover_failure_reason:-exit $status}"
    fi
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
    python3 "$bus_cutover_repo/tools/cutover.py" "$1" --source "$bus_cutover_session_dir" --manifest "$bus_cutover_watch_file"
}

stop_session() {
    local output="$bus_cutover_backup_dir/stop-response.json"
    env BUS_DATA_DIR="$bus_cutover_session_dir" "$bus_cutover_binary" stop >"$output" || return $?
    cat "$output" || return $?
    python3 -c 'import json, sys; data=json.load(open(sys.argv[1])); assert data.get("stopped") is True, "The old binary did not confirm stopped:true"' "$output"
}

main() {
    local tool built_head
    umask 077
    bus_cutover_backup_dir=$(mktemp -d /private/tmp/bus-cutover.XXXXXX)
    printf 'Backup and build-log directory: %s\n' "$bus_cutover_backup_dir"
    : >"$bus_cutover_backup_dir/cutover.log"
    : >"$bus_cutover_backup_dir/unhandled.stderr.log"
    # This fallback also captures an unexpected, unwrapped shell failure.
    exec 4>&2
    exec 2>>"$bus_cutover_backup_dir/unhandled.stderr.log"
    trap 'on_exit "$?"' EXIT
    trap 'on_error "$?" "$BASH_COMMAND"' ERR
    trap 'bus_cutover_failure_reason="Interrupted by Ctrl+C."; exit 130' INT
    trap 'bus_cutover_failure_reason="Interrupted by a termination signal."; exit 143' TERM
    printf 'Step 1: Check the branch, pushed commit, and clean worktree.\n'
    [[ "$bus_cutover_session_id" =~ ^[0-9a-f]{16}$ ]] || fail 'Usage: tools/cutover.sh SESSION_ID (16 lowercase hex digits).'
    cd "$bus_cutover_repo"
    run_step preflight preflight "$bus_cutover_repo"
    for tool in cargo python3 lsof ps; do
        command -v "$tool" >/dev/null || fail "Required command is missing: $tool. Nothing has been stopped."
    done
    [[ -x "$bus_cutover_binary" ]] || fail 'The old target/debug/bus executable is missing.'
    [[ -d "$bus_cutover_session_dir" ]] || fail "Session $bus_cutover_session_id is missing."
    umask 077
    run_step backup_binary prepare_backup "$bus_cutover_binary"
    built_head=$bus_cutover_verified_head

    printf 'Step 1 continued: Prebuild the new binary in target/cutover-candidate while the session stays running.\n'
    run_step build_candidate build_candidate "$bus_cutover_candidate_target" "$bus_cutover_backup_dir/build.log"
    cat "$bus_cutover_backup_dir/build.log"
    [[ -x "$bus_cutover_candidate" ]] || fail 'Prebuild did not produce an executable candidate. Nothing has been stopped.'
    run_step verify_candidate python3 tools/cutover.py record --binary "$bus_cutover_candidate" --head "$built_head" --manifest "$bus_cutover_backup_dir/candidate.json"
    write_status candidate_verified
    printf 'Recheck the pushed clean commit after the prebuild.\n'
    run_step recheck_preflight preflight "$bus_cutover_repo"
    [[ "$built_head" == "$bus_cutover_verified_head" ]] || fail 'HEAD changed during the build. Nothing has been stopped.'
    cmp -s "$bus_cutover_binary" "$bus_cutover_backup_dir/bus-before-cutover" || fail 'The old binary changed during the build. Nothing has been stopped.'
    bus_cutover_watch_file="$bus_cutover_backup_dir/processes.json"
    run_step snapshot_processes watch_processes snapshot

    printf 'Step 2: Stop session %s with the old binary, then wait for only its processes to exit.\n' "$bus_cutover_session_id"
    # Arm recovery before requesting the stop: even a partially completed stop
    # or an unparseable response must attempt to resume the old session.
    bus_cutover_resume_on_failure=true
    write_status stop_requested
    run_step stop_session stop_session
    write_status waiting_for_exit
    run_step wait_for_exit watch_processes wait

    printf 'Step 3: Back up the stopped session in the printed private backup directory.\n'
    run_step backup_session backup_session "$bus_cutover_session_dir"
    printf 'Step 4: Atomically install the prebuilt binary into target/debug/bus and verify it.\n'
    run_step install_candidate atomic_install "$bus_cutover_candidate" "$bus_cutover_binary"
    cleanup_transients
    run_step verify_installed python3 tools/cutover.py verify --binary "$bus_cutover_binary" --manifest "$bus_cutover_backup_dir/candidate.json"
    write_status installed_verified
    printf 'Step 5: Resume the same saved session with the verified new binary.\n'
    bus_cutover_step=resume_session
    # The interactive handoff can return when the user quits. It must never
    # trigger the cutover's deployment rollback at that point.
    trap - EXIT ERR
    bus_cutover_resume_on_failure=false
    resume_session
}

# Sourcing exposes helpers for safe fixture checks; it never starts the cutover.
if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
    main "$@"
fi
