use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, path::Path};
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct App {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub executable: String,
    #[serde(default)]
    pub argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub existing: Option<ExistingWindow>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExistingWindow {
    pub display: u16,
    pub xauthority: String,
    pub process_id: u32,
}
pub fn load_catalog(path: &Path) -> Result<Vec<App>> {
    let data = std::fs::read(path).context("read approved app catalog")?;
    if data.len() > 1024 * 1024 {
        bail!("catalog too large")
    }
    let apps: Vec<App> = serde_json::from_slice(&data)?;
    validate(&apps)?;
    Ok(apps)
}
pub fn validate(apps: &[App]) -> Result<()> {
    let mut ids = HashSet::new();
    if apps.len() > 256 {
        bail!("too many apps")
    }
    for a in apps {
        if a.name.is_empty()
            || a.name.len() > 512
            || a.description.len() > 4096
            || a.name.chars().any(char::is_control)
            || a.description.contains('\0')
        {
            bail!("invalid catalog metadata")
        }
        if !remoteapp_protocol::identifier(&a.id) || !ids.insert(&a.id) {
            bail!("invalid/duplicate app id")
        }
        if !Path::new(&a.executable).is_absolute()
            || a.executable.contains('\0')
            || a.argv.len() > 64
            || a.argv.iter().any(|s| s.len() > 4096 || s.contains('\0'))
        {
            bail!("invalid executable/argv")
        }
        if let Some(existing) = &a.existing {
            if existing.process_id <= 1
                || !Path::new(&existing.xauthority).is_absolute()
                || existing.xauthority.contains('\0')
                || !a.argv.is_empty()
            {
                bail!("invalid existing-window approval")
            }
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_client_shell() {
        let mut a = App {
            id: "a".into(),
            name: "A".into(),
            description: String::new(),
            executable: "relative".into(),
            argv: vec![],
            existing: None,
        };
        assert!(validate(&[a.clone()]).is_err());
        a.executable = "/usr/bin/xterm".into();
        assert!(validate(&[a.clone()]).is_ok());
        assert!(validate(&[a.clone(), a]).is_err());
    }
    #[test]
    fn existing_window_requires_explicit_local_approval() {
        let mut app: App = serde_json::from_value(serde_json::json!({
            "id":"existing", "name":"Existing", "executable":"/usr/bin/xterm",
            "existing":{"display":0,"xauthority":"/run/user/1000/Xauthority","process_id":1234}
        }))
        .unwrap();
        assert!(validate(&[app.clone()]).is_ok());
        app.existing.as_mut().unwrap().process_id = 1;
        assert!(validate(&[app.clone()]).is_err());
        app.existing.as_mut().unwrap().process_id = 1234;
        app.existing.as_mut().unwrap().xauthority = "relative".into();
        assert!(validate(&[app.clone()]).is_err());
        app.existing.as_mut().unwrap().xauthority = "/run/user/1000/Xauthority".into();
        app.argv.push("--launch-something".into());
        assert!(validate(&[app]).is_err());
    }
}
