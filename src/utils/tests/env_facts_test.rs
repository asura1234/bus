use super::*;
use std::ffi::OsString;

struct RestoreRoot(Option<OsString>);

impl Drop for RestoreRoot {
    fn drop(&mut self) {
        if let Some(value) = &self.0 {
            std::env::set_var("BUS_DATA_DIR", value);
        } else {
            std::env::remove_var("BUS_DATA_DIR");
        }
    }
}

#[test]
fn bus_data_dir_keeps_absence_empty_and_inherited_path_bytes() {
    let _guard = crate::config::test_config_env_lock().lock().unwrap();
    let _restore = RestoreRoot(std::env::var_os("BUS_DATA_DIR"));

    std::env::remove_var("BUS_DATA_DIR");
    assert_eq!(bus_data_dir(), None);
    std::env::set_var("BUS_DATA_DIR", "");
    assert_eq!(bus_data_dir(), Some(PathBuf::new()));
    std::env::set_var("BUS_DATA_DIR", "relative/root");
    assert_eq!(bus_data_dir(), Some(PathBuf::from("relative/root")));

    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let path = OsString::from_vec(b"/tmp/bus-\xff".to_vec());
        std::env::set_var("BUS_DATA_DIR", &path);
        assert_eq!(bus_data_dir(), Some(PathBuf::from(path)));
    }
}

#[test]
fn operational_environment_keys_keep_the_legacy_spelling() {
    assert_eq!(SOCKET_PATH_ENV_VAR, "HERDR_SOCKET_PATH");
    assert_eq!(STARTUP_CWD_ENV_VAR, "HERDR_STARTUP_CWD");
}
