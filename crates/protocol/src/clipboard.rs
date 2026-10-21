use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const MAX_CLIPBOARD_BYTES: u64 = 1024 * 1024 * 1024;
pub const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_CLIPBOARD_ENTRIES: usize = 4096;
pub const MAX_MANIFEST: usize = 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipboardEntry {
    pub path: String,
    pub size: u64,
    #[serde(default)]
    pub directory: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipboardManifest {
    pub kind: String,
    pub entries: Vec<ClipboardEntry>,
}
impl ClipboardManifest {
    pub fn validate(&self) -> Result<()> {
        if !["image", "files"].contains(&self.kind.as_str())
            || self.entries.is_empty()
            || self.entries.len() > MAX_CLIPBOARD_ENTRIES
        {
            bail!("invalid clipboard manifest")
        }
        let mut names = HashSet::new();
        let mut total = 0u64;
        for item in &self.entries {
            if item.path.len() > 4096
                || item.path.contains(['\0', '\\', '\r', '\n'])
                || item
                    .path
                    .split('/')
                    .any(|c| c.is_empty() || c == "." || c == "..")
                || !names.insert(&item.path)
                || (item.directory && item.size != 0)
            {
                bail!("invalid clipboard entry")
            }
            total = total
                .checked_add(item.size)
                .ok_or_else(|| anyhow::anyhow!("clipboard size overflow"))?;
        }
        let directories: HashSet<&str> = self
            .entries
            .iter()
            .filter(|e| e.directory)
            .map(|e| e.path.as_str())
            .collect();
        for item in &self.entries {
            let parts: Vec<_> = item.path.split('/').collect();
            for i in 1..parts.len() {
                let parent = parts[..i].join("/");
                if !directories.contains(parent.as_str()) {
                    bail!("clipboard parent directory missing")
                }
            }
        }
        if total > MAX_CLIPBOARD_BYTES {
            bail!("clipboard transfer too large")
        }
        if self.kind == "image"
            && (self.entries.len() != 1
                || self.entries[0].directory
                || total == 0
                || total > MAX_IMAGE_BYTES as u64)
        {
            bail!("invalid clipboard image")
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_traversal_duplicates_conflicting_parents_and_oversize() {
        for path in [
            "../file",
            "/file",
            "a/../file",
            "a\\file",
            "a\nfile",
            "a//file",
        ] {
            assert!(ClipboardManifest {
                kind: "files".into(),
                entries: vec![ClipboardEntry {
                    path: path.into(),
                    size: 1,
                    directory: false
                }]
            }
            .validate()
            .is_err());
        }
        let mut m = ClipboardManifest {
            kind: "files".into(),
            entries: vec![ClipboardEntry {
                path: "file".into(),
                size: 0,
                directory: false,
            }],
        };
        assert!(m.validate().is_ok());
        m.entries.push(m.entries[0].clone());
        assert!(m.validate().is_err());
        m.entries[1].path = "file/child".into();
        assert!(m.validate().is_err());
        m.entries.pop();
        m.entries[0].size = MAX_CLIPBOARD_BYTES + 1;
        assert!(m.validate().is_err());
    }
}
