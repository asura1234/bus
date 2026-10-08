use std::{ffi::OsString, path::Path, sync::MutexGuard};

pub(crate) struct ProcessEnvironment {
    _guard: MutexGuard<'static, ()>,
    previous: Vec<(&'static str, Option<OsString>)>,
}

impl ProcessEnvironment {
    pub(crate) fn enter(root: Option<&Path>, session: Option<&str>) -> Self {
        let guard = crate::config::test_config_env_lock().lock().unwrap();
        let previous = ["BUS_DATA_DIR", "HERDR_SESSION"]
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect();
        for (key, value) in [
            ("BUS_DATA_DIR", root.map(|root| root.as_os_str())),
            ("HERDR_SESSION", session.map(std::ffi::OsStr::new)),
        ] {
            if let Some(value) = value {
                std::env::set_var(key, value);
            } else {
                std::env::remove_var(key);
            }
        }
        Self {
            _guard: guard,
            previous,
        }
    }
}

impl Drop for ProcessEnvironment {
    fn drop(&mut self) {
        for (key, value) in &self.previous {
            if let Some(value) = value {
                std::env::set_var(key, value);
            } else {
                std::env::remove_var(key);
            }
        }
    }
}
