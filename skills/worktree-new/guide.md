# Worktree provisioning principles

A worktree is an isolated checkout with its own branch and outputs. Keep it as
long as useful. Reuse an authorized lane for a replacement direction rather than
multiplying checkouts without a disk budget. Existing branches are preserved,
never reset to an integration base. Origin's symbolic HEAD determines that base;
an explicit base handles origins without one. New feature branches must not
track the integration branch, because a bare push could target it.

## Discovery and uncertainty

The scanner is repo-agnostic and uses only Git and Python 3.11+. It reads tracked
and visible untracked manifests, including initialized submodules, without
walking dependency caches or symlinks. Its canonical output and override contract
are defined in [repo-profile-format.md](references/repo-profile-format.md).
There is one shared cache in the main checkout. A caller refreshes it for the
checkout it is about to use; it must not assume another checkout's last scan
matches its own locks. Source contents and gitlink identities invalidate the
cache, including toolchain pins inside initialized submodules. Local hazards
such as rerere or a dependency symlink also refresh the output.

An uninitialized submodule has unknown pins. Initialization checks out the
superproject's gitlinks, never an upstream branch tip, and triggers another scan.
A disabled submodule update is a repository policy decision; it does not make
missing native dependencies available. Never seed a new checkout with ignored
files from an older one. Those files can hide a real broken gitlink.

Detection is evidence, not a claim that every project has a safe universal build
command. Cargo uses locked fetch, build and tests. Node uses its lockfile's
manager and a build plus a non-GUI test script. Root CMake uses configure, build,
and ctest with empty test collections treated as an error. Run wrappers, Make,
just, Xcode, Gradle, workspaces and native pins are inventoried. Complex native or
mobile builds need an explicit headless recipe; launching an app or simulator
cannot substitute for it. Inspect any explicit recipe semantically: it must
build and execute a non-GUI test or run, with no interactive login dependency.
The script can prove command exit status, not that a user-supplied `true` is a
meaningful test. Configure finite test commands; CI=true avoids common watch defaults.

## Dependency ownership

Default installs are locked and per checkout: npm ci, pnpm/bun frozen installs,
and the appropriate Yarn lock mode. An ambiguous manager or missing lock blocks
provisioning rather than silently changing dependencies. For Bus, the same
inferred Cargo path keeps cargo fetch --locked and runs the headless Rust tests;
there is no repository-name special case.

Sharing node_modules is an explicit optimization for matching lock-owning package
roots. Links that escape into another checkout's workspace/source require a private
install; a passing smoke must exercise the feature checkout. Install aliases are
recognized, while ambiguous/overlapping preparation is refused before it can write
through a shared link. The runner never replaces an existing directory or an
unexpected symlink. Inspect smoke scripts before sharing if they mutate dependencies.

Declare mutable state using `worktree.private_paths`, and custom dependency locks
using `extra_manifests`. A private path may not resolve outside the checkout, even
through an ancestor link. The runner checks before and after each recipe command
and immediately before recording readiness, so preparation cannot expose shared
state to a later step. Keep mutable activation receipts private while sharing
only immutable cache children. Generic visible-link warnings carry actual paths;
project-specific signatures belong in configuration and fixtures.

## Toolchains, disk and evidence

The governing Node pin must match each command's environment, or native-module
ABI errors can masquerade as source regressions. `--tool-bin` selects an existing
installation for the command's nearest pin. Numeric major/minor/full pins match
by prefix; unused nested pins are inventoried without blocking unrelated commands.
Rust's own per-directory toolchain selection respects nested rust-toolchain
files. Never remove Rust or another SDK just because a root-only search found
no pin; initialized submodule pins are part of the inventory.

The recipe carries a planning estimate, including nested Rust targets. Compare
it with free space and existing worktree count before creating more lanes.
The runner refuses provisioning below 25 GiB and rechecks before every command;
it holds a caller-authorized mkdir slot and releases only its own slot. All
children drop live BUS_/HERDR_ variables. Cargo gets a single target per checkout
(or the caller's authorized target), no incremental compilation and fewer than
half the host cores. Commands run at nice 19 on Unix. These precautions do not
reserve filesystem space: concurrent workers still need a shared disk budget.
No cleanup is performed. Pass nested target paths to worktree-close and verify
that no session owns them before reclaiming anything.

Ready means every prepare and smoke command passed and the original HEAD,
manifest hash and tracked source contents remained unchanged. A new attempt
invalidates prior readiness before doing work. A failure leaves the checkout in
place, writes failure evidence and names the failed command. Logs belong under
the worktree's temp directory. Full gates, GUI acceptance and provider sessions
are separate evidence. No provider memory or real user configuration is touched.

The profile warns when shared rerere is enabled. Rebase belongs to the rebase
workflow: disable rerere for the entire operation, audit original versus rebased
commit file lists, and investigate unexpected content drops. Provisioning does
not alter common Git configuration or repair rebases.
