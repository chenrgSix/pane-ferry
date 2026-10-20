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
        };
        assert!(validate(&[a.clone()]).is_err());
        a.executable = "/usr/bin/xterm".into();
        assert!(validate(&[a.clone()]).is_ok());
        assert!(validate(&[a.clone(), a]).is_err());
    }
}
