//! Relaunch only the Bus this host owns, keeping its scratch data and trace.
use super::{failure, start, Host, HostOptions};
use serde_json::{json, Value};

impl Host {
    pub(super) fn restart(&mut self) -> Value {
        let owner = match self._owner.try_clone() {
            Ok(owner) => owner,
            Err(error) => return failure("restart_failed", error.to_string()),
        };
        let options = HostOptions {
            name: self.info.name.clone(),
            cols: self.info.size.0,
            rows: self.info.size.1,
            run_dir: self.info.run_dir.clone(),
            token: self.token.clone(),
            graphics: self.info.graphics,
            yolo: self.info.yolo,
        };
        let old_pid = self.info.bus_pid;
        let stopped = self.teardown();
        if stopped["ok"] != true {
            return stopped;
        }
        // Confirm the server is gone before launching; never attach to a
        // surviving server and call that a restart.
        let stop = self.bus(&["stop"]);
        if !stop.ok() {
            return failure("restart_failed", stop.stderr);
        }
        let restarted = match start(&self.paths, &options, owner, Some(self.trace.clone())) {
            Ok(host) => host,
            Err(error) => return failure("restart_failed", error),
        };
        // Keep screenshot names unique within the continuous run folder.
        restarted.shots.set(self.shots.get());
        let ready = restarted.ready();
        *self = restarted;
        match ready {
            Ok(()) => json!({"ok": true, "old_bus_pid": old_pid,
                "bus_pid": self.info.bus_pid, "data_dir": self.info.data_dir,
                "run_dir": self.info.run_dir}),
            Err(error) => {
                self.teardown();
                failure("restart_failed", error)
            }
        }
    }
}
