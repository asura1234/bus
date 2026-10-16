//! Process invocation grammar and local session selection.
use crate::messaging::storage::sessions::ResumeTarget;

pub(super) const USAGE: &str = "Usage: bus [--dev] [--yolo] [--paths | --help]\n       bus sessions\n       bus [--dev] resume <session-id> [--yolo]\n       bus [--dev] resume --last [--yolo]\n       bus stop\n       bus perf [--since <minutes>m|<hours>h | --all]";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Action {
    Run,
    Sessions,
    Resume(ResumeTarget),
    Stop,
    Paths,
    Help,
    Control(Vec<String>),
    /// `bus --dev tui ...`: the TUI driver, in the dev variant only.
    Tui(Vec<String>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Invocation {
    pub(super) dev: bool,
    pub(super) yolo: bool,
    pub(super) action: Action,
}

pub(super) fn parse_invocation(args: &[String]) -> Result<Invocation, String> {
    let mut dev = false;
    let mut yolo = false;
    let mut action = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--dev" if !dev => {
                dev = true;
                index += 1;
            }
            "--yolo" if !yolo => {
                yolo = true;
                index += 1;
            }
            "--paths" if action.is_none() => {
                action = Some(Action::Paths);
                index += 1;
            }
            "--help" | "-h" if action.is_none() => {
                action = Some(Action::Help);
                index += 1;
            }
            "sessions" if action.is_none() => {
                action = Some(Action::Sessions);
                index += 1;
            }
            "stop" if action.is_none() => {
                action = Some(Action::Stop);
                index += 1;
            }
            "resume" if action.is_none() => {
                index += 1;
                let mut target = None;
                while index < args.len() {
                    match args[index].as_str() {
                        "--dev" if !dev => dev = true,
                        "--yolo" if !yolo => yolo = true,
                        "--last" if target.is_none() => target = Some(ResumeTarget::Last),
                        value if target.is_none() && !value.starts_with('-') => {
                            target = Some(ResumeTarget::Id(value.to_owned()));
                        }
                        _ => return Err(USAGE.to_owned()),
                    }
                    index += 1;
                }
                action = Some(Action::Resume(target.ok_or_else(|| USAGE.to_owned())?));
            }
            "tui" if action.is_none() => {
                action = Some(Action::Tui(args[index + 1..].to_vec()));
                break;
            }
            value if action.is_none() && !value.starts_with('-') => {
                action = Some(Action::Control(args[index..].to_vec()));
                break;
            }
            _ => return Err(USAGE.to_owned()),
        }
    }
    Ok(Invocation {
        dev,
        yolo,
        action: action.unwrap_or(Action::Run),
    })
}
