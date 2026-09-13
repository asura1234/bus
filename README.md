# Bus

> **Upstream and license:** Bus is an independently maintained fork of
> [Herdr](https://github.com/herdrdev/herdr), whose original work is credited to
> the Herdr contributors. Bus retains the inherited [Apache License 2.0](LICENSE).
> This fork changes Herdr to add room-based coordination of selected coding
> agents, shared conversation history, and Bus-specific controls and storage.
> This README has been updated for Bus.

Bus is a local terminal app for a human coordinating Codex, Claude Code, and
Cursor agents. Choose which agents receive each message, keep their final
responses in a room's history, and open an agent's real terminal when it needs
direct interaction. Rooms have their own notes, drafts, recipients, and files;
each agent has its own working directory.

## Quick start

Install Rust/Cargo, Zig **0.15.2**, and your platform's native build tools
(Xcode Command Line Tools on macOS). Install and sign in to the provider CLIs
you want to use.

```sh
git clone --branch main https://github.com/asura1234/bus.git
cd bus
cargo build --locked --bin herdr
./bus
```

The launcher uses `target/debug/herdr` from this checkout. If Zig is outside
your `PATH`, set `ZIG=/absolute/path/to/zig` for the build. The
[Bus guide](docs/next/bus.md#run-from-this-checkout) includes the verified local
macOS build setup.

Create a room with **Ctrl+R** and add agents with **Ctrl+N**. Choose recipients
with **@**, then press **Enter** to send. Open an agent to review its CLI and
complete the provider hook setup; use **Confirm setup** after reviewing it to
allow queued delivery. Click the room or press **F6** to return to the
conversation. Enter `/help` in the composer for the shortcut guide.

```sh
./bus --help       # Shortcuts and control commands
./bus --paths      # Data and log locations
./bus --dev        # Developer logging
```

Bus stores rooms and its runtime configuration under `~/.local/share/bus`.
Set an absolute `BUS_DATA_DIR` to use a separate instance. Closing the client
leaves the native server and agent terminals available; reopen with the same
data directory to reconnect.

## Documentation

- [Bus guide](docs/next/bus.md): room workflow, files, agent setup, storage, and diagnostics.
- [Herdr documentation](https://herdr.dev/docs/): the inherited terminal runtime.
- [Agent instructions](AGENTS.md) and [contribution guidance](CONTRIBUTING.md).

## Credits and third-party notices

Bus builds on Herdr and its contributors' terminal runtime. Herdr's upstream
acknowledgements remain in [SPONSORS.md](SPONSORS.md).

The full Apache 2.0 terms remain in [LICENSE](LICENSE). Bundled components
retain their own license and notice files, including
[Ghostty](vendor/libghostty-vt/LICENSE),
[portable-pty](vendor/portable-pty/LICENSE.md), and
[Microsoft ConPTY's license](packaging/windows/licenses/Microsoft.Windows.Console.ConPTY-LICENSE.txt)
and [third-party notices](packaging/windows/licenses/Microsoft.Windows.Console.ConPTY-NOTICE.md).
