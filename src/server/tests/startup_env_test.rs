use super::take_startup_cwd;
use crate::utils::env::STARTUP_CWD_ENV_VAR;
use std::ffi::OsString;
use std::path::PathBuf;

struct RestoreCwd(Option<OsString>);

impl Drop for RestoreCwd {
    fn drop(&mut self) {
        if let Some(value) = &self.0 {
            std::env::set_var(STARTUP_CWD_ENV_VAR, value);
        } else {
            std::env::remove_var(STARTUP_CWD_ENV_VAR);
        }
    }
}

#[test]
fn startup_cwd_is_taken_once_and_empty_values_are_removed() {
    let _guard = crate::config::test_config_env_lock().lock().unwrap();
    let _restore = RestoreCwd(std::env::var_os(STARTUP_CWD_ENV_VAR));

    std::env::remove_var(STARTUP_CWD_ENV_VAR);
    assert_eq!(take_startup_cwd(), None);
    std::env::set_var(STARTUP_CWD_ENV_VAR, "");
    assert_eq!(take_startup_cwd(), None);
    assert!(std::env::var_os(STARTUP_CWD_ENV_VAR).is_none());
    std::env::set_var(STARTUP_CWD_ENV_VAR, "relative/workspace");
    assert_eq!(
        take_startup_cwd(),
        Some(PathBuf::from("relative/workspace"))
    );
    assert!(std::env::var_os(STARTUP_CWD_ENV_VAR).is_none());
    assert_eq!(take_startup_cwd(), None);
}

#[cfg(unix)]
#[test]
fn startup_cwd_preserves_non_utf8_path_bytes_before_removing_the_key() {
    use std::os::unix::ffi::OsStringExt;
    let _guard = crate::config::test_config_env_lock().lock().unwrap();
    let _restore = RestoreCwd(std::env::var_os(STARTUP_CWD_ENV_VAR));
    let path = OsString::from_vec(b"/tmp/bus-\xff".to_vec());

    std::env::set_var(STARTUP_CWD_ENV_VAR, &path);
    assert_eq!(take_startup_cwd(), Some(PathBuf::from(path)));
    assert!(std::env::var_os(STARTUP_CWD_ENV_VAR).is_none());
}
