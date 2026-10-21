//! Private disk snapshots and a bounded binary stream over our SSH channel.
use anyhow::{bail, Context, Result};
use remoteapp_protocol::{clipboard::*, identifier, read_line_limit};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

pub struct ClipboardStore {
    pub root: PathBuf,
}
impl ClipboardStore {
    pub fn open() -> Result<Self> {
        let root = PathBuf::from(std::env::var("HOME")?).join(".cache/lan-remoteapp/clipboard");
        fs::create_dir_all(&root)?;
        let meta = fs::symlink_metadata(&root)?;
        if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } {
            bail!("invalid clipboard cache")
        }
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        Ok(Self { root })
    }
    fn prune(&self, incoming: u64) -> Result<()> {
        let mut total = 0u64;
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if !identifier(name) || !entry.file_type()?.is_dir() {
                continue;
            }
            if entry
                .metadata()?
                .modified()?
                .elapsed()
                .unwrap_or_default()
                .as_secs()
                > 7 * 86400
            {
                fs::remove_dir_all(entry.path())?;
                continue;
            }
            if let Ok(manifest) = self.manifest(name) {
                total = total.saturating_add(manifest.entries.iter().map(|e| e.size).sum::<u64>());
            }
        }
        if total.saturating_add(incoming) > 4 * MAX_CLIPBOARD_BYTES {
            bail!("clipboard cache capacity exceeded")
        }
        Ok(())
    }
    pub fn create(&self, incoming: u64) -> Result<(String, PathBuf)> {
        self.prune(incoming)?;
        let mut random = [0u8; 16];
        File::open("/dev/urandom")?.read_exact(&mut random)?;
        let id: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let path = self.root.join(&id);
        fs::create_dir(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        fs::create_dir(path.join("data"))?;
        Ok((id, path))
    }
    fn path(&self, id: &str) -> Result<PathBuf> {
        if !identifier(id) {
            bail!("invalid clipboard transfer id")
        }
        let path = self.root.join(id);
        let meta = fs::symlink_metadata(&path)?;
        if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } {
            bail!("invalid clipboard snapshot")
        }
        Ok(path)
    }
    pub fn manifest(&self, id: &str) -> Result<ClipboardManifest> {
        let bytes = fs::read(self.path(id)?.join("manifest.json"))?;
        if bytes.len() > MAX_MANIFEST {
            bail!("clipboard manifest too large")
        }
        let m: ClipboardManifest = serde_json::from_slice(&bytes)?;
        m.validate()?;
        Ok(m)
    }
    fn finish(&self, path: &Path, manifest: &ClipboardManifest) -> Result<()> {
        manifest.validate()?;
        fs::write(path.join("manifest.json"), serde_json::to_vec(manifest)?)?;
        Ok(())
    }
    pub fn receive(&self, input: &mut impl BufRead) -> Result<String> {
        let line = read_line_limit(input, MAX_MANIFEST)?.context("missing clipboard manifest")?;
        let manifest: ClipboardManifest = serde_json::from_slice(&line)?;
        manifest.validate()?;
        let (id, path) = self.create(manifest.entries.iter().map(|e| e.size).sum())?;
        let result = (|| {
            for item in &manifest.entries {
                let dest = path.join("data").join(&item.path);
                if item.directory {
                    fs::create_dir_all(&dest)?;
                    continue;
                }
                fs::create_dir_all(dest.parent().context("invalid clipboard destination")?)?;
                let mut out = OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .mode(0o600)
                    .open(&dest)?;
                if std::io::copy(&mut input.take(item.size), &mut out)? != item.size {
                    bail!("truncated clipboard file")
                }
            }
            if manifest.kind == "image" {
                validate_png(&fs::read(
                    path.join("data").join(&manifest.entries[0].path),
                )?)?;
            }
            self.finish(&path, &manifest)?;
            Ok(())
        })();
        if let Err(e) = result {
            let _ = fs::remove_dir_all(path);
            return Err(e);
        }
        Ok(id)
    }
    pub fn send(&self, id: &str, output: &mut impl Write) -> Result<()> {
        let manifest = self.manifest(id)?;
        let path = self.path(id)?.join("data");
        serde_json::to_writer(&mut *output, &manifest)?;
        output.write_all(b"\n")?;
        for item in &manifest.entries {
            if item.directory {
                continue;
            }
            let mut file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(path.join(&item.path))?;
            if !file.metadata()?.is_file() || file.metadata()?.len() != item.size {
                bail!("clipboard snapshot changed")
            }
            if std::io::copy(&mut file, output)? != item.size {
                bail!("clipboard snapshot changed")
            }
        }
        Ok(())
    }
    pub fn image(&self, png: &[u8]) -> Result<String> {
        validate_png(png)?;
        let (id, path) = self.create(png.len() as u64)?;
        fs::write(path.join("data/image.png"), png)?;
        self.finish(
            &path,
            &ClipboardManifest {
                kind: "image".into(),
                entries: vec![ClipboardEntry {
                    path: "image.png".into(),
                    size: png.len() as u64,
                    directory: false,
                }],
            },
        )?;
        Ok(id)
    }
    pub fn files(&self, uris: &[u8]) -> Result<String> {
        let text = std::str::from_utf8(uris)?;
        let mut sources = Vec::new();
        let mut entries = Vec::new();
        fn visit(
            source: &Path,
            relative: String,
            entries: &mut Vec<ClipboardEntry>,
            sources: &mut Vec<PathBuf>,
        ) -> Result<()> {
            if entries.len() >= MAX_CLIPBOARD_ENTRIES {
                bail!("too many clipboard files")
            }
            let meta = fs::symlink_metadata(source)?;
            if !meta.is_file() && !meta.is_dir() {
                bail!("clipboard supports regular files and folders")
            }
            entries.push(ClipboardEntry {
                path: relative.clone(),
                size: if meta.is_file() { meta.len() } else { 0 },
                directory: meta.is_dir(),
            });
            sources.push(source.into());
            if meta.is_dir() {
                let mut children = fs::read_dir(source)?.collect::<std::io::Result<Vec<_>>>()?;
                children.sort_by_key(|e| e.file_name());
                for child in children {
                    visit(
                        &child.path(),
                        format!(
                            "{relative}/{}",
                            child.file_name().to_str().context("non UTF-8 filename")?
                        ),
                        entries,
                        sources,
                    )?;
                }
            }
            Ok(())
        }
        for line in text
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#') && *l != "copy" && *l != "cut")
        {
            let source = decode_file_uri(line)?;
            let name = source
                .file_name()
                .and_then(|n| n.to_str())
                .context("invalid copied filename")?;
            visit(&source, name.into(), &mut entries, &mut sources)?;
        }
        let manifest = ClipboardManifest {
            kind: "files".into(),
            entries,
        };
        manifest.validate()?;
        let (id, path) = self.create(manifest.entries.iter().map(|e| e.size).sum())?;
        let result = (|| {
            for (item, source) in manifest.entries.iter().zip(sources) {
                let dest = path.join("data").join(&item.path);
                if item.directory {
                    fs::create_dir_all(dest)?;
                } else {
                    let input = OpenOptions::new()
                        .read(true)
                        .custom_flags(libc::O_NOFOLLOW)
                        .open(source)?;
                    if !input.metadata()?.is_file() || input.metadata()?.len() != item.size {
                        bail!("copied file changed")
                    }
                    let mut output = OpenOptions::new()
                        .create_new(true)
                        .write(true)
                        .mode(0o600)
                        .open(dest)?;
                    if std::io::copy(&mut input.take(item.size), &mut output)? != item.size {
                        bail!("copied file changed")
                    }
                }
            }
            self.finish(&path, &manifest)
        })();
        if let Err(e) = result {
            let _ = fs::remove_dir_all(path);
            return Err(e);
        }
        Ok(id)
    }
    pub fn offers(&self, id: &str) -> Result<Vec<(String, Vec<u8>)>> {
        let m = self.manifest(id)?;
        let path = self.path(id)?.join("data");
        if m.kind == "image" {
            let bytes = fs::read(path.join(&m.entries[0].path))?;
            validate_png(&bytes)?;
            return Ok(vec![("image/png".into(), bytes)]);
        }
        let mut roots = Vec::new();
        for entry in &m.entries {
            let root = entry.path.split('/').next().unwrap();
            if !roots.contains(&root) {
                roots.push(root)
            }
        }
        let list = roots
            .into_iter()
            .map(|name| encode_file_uri(&path.join(name)))
            .collect::<Result<Vec<_>>>()?
            .join("\r\n")
            + "\r\n";
        Ok(vec![
            ("text/uri-list".into(), list.as_bytes().to_vec()),
            (
                "x-special/gnome-copied-files".into(),
                format!("copy\n{}", list.replace("\r\n", "\n")).into_bytes(),
            ),
            ("application/x-kde-cutselection".into(), b"0".to_vec()),
        ])
    }
}
pub fn validate_png(bytes: &[u8]) -> Result<()> {
    if bytes.len() > MAX_IMAGE_BYTES
        || bytes.len() < 24
        || &bytes[..8] != b"\x89PNG\r\n\x1a\n"
        || &bytes[12..16] != b"IHDR"
    {
        bail!("invalid clipboard PNG")
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into()?);
    let h = u32::from_be_bytes(bytes[20..24].try_into()?);
    if w == 0 || h == 0 || u64::from(w) * u64::from(h) > 64_000_000 {
        bail!("clipboard image dimensions exceeded")
    }
    Ok(())
}
pub fn encode_file_uri(path: &Path) -> Result<String> {
    let s = path.to_str().context("non UTF-8 clipboard path")?;
    if !path.is_absolute() {
        bail!("absolute clipboard path required")
    }
    Ok("file://".to_string()
        + &s.bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
                    (b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect::<String>())
}
pub fn decode_file_uri(s: &str) -> Result<PathBuf> {
    let raw = s
        .strip_prefix("file://localhost/")
        .map(|p| format!("/{p}"))
        .or_else(|| s.strip_prefix("file:///").map(|p| format!("/{p}")))
        .context("only local file URIs are supported")?;
    let mut out = Vec::new();
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                bail!("invalid file URI")
            };
            out.push(u8::from_str_radix(
                std::str::from_utf8(&bytes[i + 1..i + 3])?,
                16,
            )?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    if out.contains(&0) {
        bail!("invalid file URI")
    }
    Ok(PathBuf::from(String::from_utf8(out)?))
}
