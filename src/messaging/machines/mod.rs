//! Saved SSH peers. A peer is another running Bus, never a managed installation.
mod config;
mod ssh;

pub(crate) use config::{load_targets, update_targets, MachineTarget};
pub(crate) use ssh::{connect_terminal, request, ControlConnection, TerminalConnection};

pub(crate) const HELP: &str = "Usage: bus machine list\n       bus machine add <label> <ssh-target>\n       bus machine remove <label>\n       bus machine enable <label>\n       bus machine disable <label>\n       bus --machine <label> <command> [arguments]\n\nLABEL names a saved target or a destination advertised by a connected viewer.\nA label missing locally uses that already-connected viewer's live route.\nNo live route means immediate failure; sends are never saved for later.\n\nSSH uses your existing keys, agent and config. Bus must already be running on the\nother machine and `bus` must be on its SSH command PATH. Nothing is installed or\ncopied. Labels identify rooms and panes; each machine keeps its own MASTER.\nAttachment paths are paths on the receiving machine. Cross-machine sends fail\nwithout queueing or retrying when the link is down. Use --room and --to for a\ncross-machine message; --reply-to settles a request on the responder's own Bus.\n";

pub(crate) fn bridge(kind: &str) -> std::io::Result<()> {
    ssh::bridge(kind)
}
