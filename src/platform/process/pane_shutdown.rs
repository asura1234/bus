use crate::platform::{ProcessInstance, Signal};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io;

#[cfg(target_os = "linux")]
use super::linux::{pane_process_info, pane_processes};
#[cfg(target_os = "macos")]
use super::macos::{pane_process_info, pane_processes};

pub(super) struct PaneProcessInfo {
    pub(super) instance: ProcessInstance,
    pub(super) parent_pid: u32,
    pub(super) session_id: Option<u32>,
    pub(super) state: &'static str,
}

pub(crate) struct PaneProcessTree {
    session_id: u32,
    leader: Option<ProcessInstance>,
    owned: BTreeMap<u32, ProcessInstance>,
    kill_results: HashMap<ProcessInstance, io::Result<()>>,
}

impl PaneProcessTree {
    pub(crate) fn new(session_id: u32, leader: Option<ProcessInstance>) -> Self {
        Self {
            session_id,
            leader,
            owned: leader
                .into_iter()
                .map(|leader| (leader.pid, leader))
                .collect(),
            kill_results: HashMap::new(),
        }
    }

    pub(crate) fn refresh(&mut self, child_reaped: bool) {
        let processes: Vec<_> = pane_processes().collect();
        let session_reused = processes.iter().any(|process| {
            process.instance.pid == self.session_id
                && process.state != "zombie"
                && (child_reaped
                    || self.leader.is_some_and(|leader| leader != process.instance)
                    || self
                        .owned
                        .get(&self.session_id)
                        .is_some_and(|original| *original != process.instance))
        });
        let mut descendants = HashMap::<u32, Vec<&PaneProcessInfo>>::new();
        let mut pending = VecDeque::new();
        for process in &processes {
            let pid = process.instance.pid;
            if (!session_reused && process.session_id == Some(self.session_id))
                || self.owned.get(&pid) == Some(&process.instance)
            {
                pending.push_back(process);
            } else {
                descendants
                    .entry(process.parent_pid)
                    .or_default()
                    .push(process);
            }
        }
        while let Some(process) = pending.pop_front() {
            let instance = process.instance;
            self.owned.insert(instance.pid, instance);
            if let Some(children) = descendants.remove(&instance.pid) {
                pending.extend(
                    children
                        .into_iter()
                        .filter(|child| child.instance.birth >= instance.birth),
                );
            }
        }
    }

    pub(crate) fn begin_attempt(&mut self) {
        self.kill_results.clear();
    }

    pub(crate) fn pids(&self) -> Vec<u32> {
        self.owned.keys().copied().collect()
    }

    fn live(&self, child_reaped: bool) -> impl Iterator<Item = (ProcessInstance, String)> + '_ {
        self.owned.values().filter_map(move |&instance| {
            if instance.pid == self.session_id && child_reaped {
                return None;
            }
            let state = match pane_process_info(instance.pid) {
                Ok(info) if info.instance == instance && info.state != "zombie" => {
                    info.state.to_owned()
                }
                Ok(_) => return None,
                Err(error) if crate::platform::process_exists(instance.pid) => {
                    format!("cannot verify process identity or state ({error})")
                }
                Err(_) => return None,
            };
            Some((instance, state))
        })
    }

    pub(crate) fn is_empty(&self, child_reaped: bool) -> bool {
        self.live(child_reaped).next().is_none()
    }

    pub(crate) fn signal(&mut self, signal: Signal, child_reaped: bool) {
        for &instance in self.owned.values() {
            if instance.pid == self.session_id && child_reaped {
                continue;
            }
            if signal == Signal::Kill && self.kill_results.contains_key(&instance) {
                continue;
            }
            // Recheck ownership immediately before kill, including after reparenting.
            let result = match pane_process_info(instance.pid) {
                Ok(info) if info.instance == instance && info.state != "zombie" => {
                    signal_pid(instance.pid, signal)
                }
                Ok(_) => continue,
                Err(error) => Err(error),
            };
            if signal == Signal::Kill {
                self.kill_results.insert(instance, result);
            }
        }
    }

    pub(crate) fn survivors(&self, child_reaped: bool) -> Vec<String> {
        self.live(child_reaped)
            .map(|(instance, state)| {
                let reason = match self.kill_results.get(&instance) {
                    Some(Ok(())) => "SIGKILL accepted but exit not observed before deadline".into(),
                    Some(Err(error)) => format!("SIGKILL could not be sent or verified ({error})"),
                    None => "discovered at the shutdown deadline".into(),
                };
                format!(
                    "pid={} birth={} state={state}: {reason}",
                    instance.pid, instance.birth
                )
            })
            .collect()
    }
}

fn signal_pid(pid: u32, signal: Signal) -> io::Result<()> {
    let signal = match signal {
        Signal::Hangup => libc::SIGHUP,
        Signal::Terminate => libc::SIGTERM,
        Signal::Kill => libc::SIGKILL,
    };
    if unsafe { libc::kill(pid as libc::pid_t, signal) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(test)]
pub(crate) fn signal_processes(pids: &[u32], signal: Signal) {
    for &pid in pids {
        if pid > 0 {
            let _ = signal_pid(pid, signal);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn exited_process_does_not_block_shutdown_while_its_parent_has_not_waited() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = child.id();
        let instance = pane_process_info(pid).unwrap().instance;
        let tree = PaneProcessTree::new(pid, Some(instance));
        assert!(!tree.is_empty(false));

        child.kill().unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let exited = loop {
            if tree.is_empty(false) || Instant::now() >= deadline {
                break tree.is_empty(false);
            }
            std::thread::sleep(Duration::from_millis(10));
        };

        let still_in_process_table = crate::platform::process_exists(pid);
        child.wait().unwrap();
        assert!(
            still_in_process_table,
            "the test parent still held the zombie"
        );
        assert!(exited, "the zombie should not block shutdown");
    }
}
