use crate::{
    config::{self, Config},
    error::Result,
    store,
};
use serde_json::{json, Value};
use std::{
    io::Write,
    path::{Path, PathBuf},
};

pub fn plist(c: &Config) -> Result<PathBuf> {
    Ok(config::home()?
        .join("Library/LaunchAgents")
        .join(format!("local.feishu-bridge.{}.plist", c.app_id)))
}
fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
pub fn install(c: &Config, config: &Path) -> Result<Value> {
    c.listener()?;
    let p = plist(c)?;
    store::private_dir(p.parent().unwrap())?;
    store::private_dir(&c.state_dir)?;
    let exe = std::env::current_exe()?;
    let args = [
        exe.as_path(),
        Path::new("--config"),
        config,
        Path::new("listen"),
    ]
    .iter()
    .map(|p| format!("<string>{}</string>", xml(&p.to_string_lossy())))
    .collect::<String>();
    let body=format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>Label</key><string>local.feishu-bridge.{}</string><key>ProgramArguments</key><array>{args}</array><key>EnvironmentVariables</key><dict><key>PATH</key><string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string></dict><key>RunAtLoad</key><true/><key>KeepAlive</key><true/><key>ThrottleInterval</key><integer>10</integer><key>Umask</key><integer>63</integer><key>StandardOutPath</key><string>{}</string><key>StandardErrorPath</key><string>{}</string></dict></plist>\n",xml(&c.app_id),xml(&c.state_dir.join("stdout.log").to_string_lossy()),xml(&c.state_dir.join("stderr.log").to_string_lossy()));
    let tmp = p.with_extension("plist.tmp");
    let mut f = store::private_file(&tmp)?;
    f.set_len(0)?;
    f.write_all(body.as_bytes())?;
    f.sync_all()?;
    std::fs::rename(&tmp, &p)?;
    std::fs::File::open(p.parent().unwrap())?.sync_all()?;
    Ok(json!({"plist_path":p}))
}
pub fn uninstall(c: &Config) -> Result<Value> {
    let p = plist(c)?;
    match std::fs::remove_file(&p) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    Ok(json!({"plist_path":p}))
}
