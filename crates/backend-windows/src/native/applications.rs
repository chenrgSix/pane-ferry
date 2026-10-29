use crate::Application;
use anyhow::{ensure, Context, Result};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};
use windows::{
    core::{Interface, PCWSTR, PWSTR},
    Win32::{
        Foundation::CloseHandle,
        Storage::EnhancedStorage::{PKEY_AppUserModel_ID, PKEY_Link_TargetParsingPath},
        System::Com::{CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED},
        UI::{Shell::*, WindowsAndMessaging::SW_SHOWNORMAL},
    },
};

struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}
#[derive(Clone)]
struct Entry {
    app: Application,
    shell_id: String,
}

pub fn packaged_key(id: &str) -> String {
    format!("appx-{:x}", Sha256::digest(id.to_lowercase().as_bytes()))
}

pub unsafe fn window_application_key(window: windows::Win32::Foundation::HWND) -> Option<String> {
    use windows::Win32::{System::Com::StructuredStorage::*, UI::Shell::PropertiesSystem::*};
    let store: IPropertyStore = SHGetPropertyStoreForWindow(window).ok()?;
    let mut value = store.GetValue(&PKEY_AppUserModel_ID).ok()?;
    let text = owned_string(PropVariantToStringAlloc(&value));
    let _ = PropVariantClear(&mut value);
    let id = text.ok()?;
    (id.contains('!') && !id.contains(['\\', '/', ':'])).then(|| packaged_key(&id))
}

pub unsafe fn packaged_window_key(
    window: windows::Win32::Foundation::HWND,
    process: windows::Win32::Foundation::HANDLE,
) -> Option<String> {
    window_application_key(window).or_else(|| {
        let mut id = [0u16; 1024];
        let mut length = id.len() as u32;
        if windows::Win32::Storage::Packaging::Appx::GetApplicationUserModelId(
            process,
            &mut length,
            Some(PWSTR(id.as_mut_ptr())),
        ) != windows::Win32::Foundation::ERROR_SUCCESS
        {
            return None;
        }
        let end = id.iter().position(|c| *c == 0).unwrap_or(id.len());
        Some(packaged_key(&String::from_utf16_lossy(&id[..end])))
    })
}

unsafe fn owned_string(value: windows::core::Result<PWSTR>) -> Result<String> {
    let value = value?;
    let result = value.to_string();
    CoTaskMemFree(Some(value.0.cast()));
    Ok(result?)
}

fn entry(name: String, shell_id: String, executable: String) -> Option<Entry> {
    if name.trim().is_empty()
        || name.len() > 1024
        || shell_id.is_empty()
        || shell_id.len() > 4096
        || shell_id.contains('\0')
    {
        return None;
    }
    let id = if !executable.is_empty() {
        let path = Path::new(&executable);
        // AppsFolder also contains documents, help files and internet shortcuts.
        // Publish executable applications only, never network locations.
        if !path.is_absolute()
            || executable.starts_with("\\\\")
            || !path
                .extension()
                .is_some_and(|s| s.eq_ignore_ascii_case("exe"))
            || !path.is_file()
        {
            return None;
        }
        let filename = path.file_name()?.to_string_lossy().to_lowercase();
        if filename.starts_with("paneferry") || filename == "lan-remoteapp-launcher.exe" {
            return None;
        }
        crate::application_key(&executable)
    } else if shell_id.contains('!') && !shell_id.contains(['\\', '/', ':']) {
        packaged_key(&shell_id)
    } else {
        return None;
    };
    Some(Entry {
        app: Application {
            id,
            name,
            description: "Windows 应用".into(),
            unavailable_reason: None,
        },
        shell_id,
    })
}

unsafe fn enumerate() -> Result<Vec<Entry>> {
    let folder: IShellItem = SHGetKnownFolderItem(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None)?;
    let iterator: IEnumShellItems = folder.BindToHandler(None, &BHID_EnumItems)?;
    let mut entries = BTreeMap::new();
    for _ in 0..2048 {
        let mut items = [None];
        let mut fetched = 0;
        iterator.Next(&mut items, Some(&mut fetched))?;
        if fetched == 0 {
            break;
        }
        let Some(item) = items[0].take() else {
            break;
        };
        let Ok(item) = item.cast::<IShellItem2>() else {
            continue;
        };
        let Ok(name) = owned_string(item.GetDisplayName(SIGDN_NORMALDISPLAY)) else {
            continue;
        };
        let Ok(shell_id) = owned_string(item.GetString(&PKEY_AppUserModel_ID)) else {
            continue;
        };
        let executable =
            owned_string(item.GetString(&PKEY_Link_TargetParsingPath)).unwrap_or_default();
        if let Some(entry) = entry(name, shell_id, executable) {
            entries.entry(entry.app.id.clone()).or_insert(entry);
        }
    }
    let mut entries: Vec<_> = entries.into_values().collect();
    entries.sort_by(|a, b| {
        a.app
            .name
            .to_lowercase()
            .cmp(&b.app.name.to_lowercase())
            .then(a.app.id.cmp(&b.app.id))
    });
    entries.truncate(512);
    Ok(entries)
}

fn shell_thread<T: Send + 'static>(work: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    std::thread::spawn(move || {
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        }
        let _apartment = Apartment;
        work()
    })
    .join()
    .map_err(|_| anyhow::anyhow!("application discovery worker failed"))?
}

pub fn list() -> Result<Vec<Application>> {
    shell_thread(|| unsafe { Ok(enumerate()?.into_iter().map(|e| e.app).collect()) })
}

pub fn launch(id: &str) -> Result<()> {
    let id = id.to_owned();
    shell_thread(move || unsafe {
        // Only an exact entry re-enumerated from the local Windows application
        // catalog may launch. The network never supplies paths, verbs or argv.
        let entry = enumerate()?
            .into_iter()
            .find(|e| e.app.id == id)
            .context("app_not_approved")?;
        ensure!(entry.app.unavailable_reason.is_none(), "app_not_approved");
        let file: Vec<_> = format!("shell:AppsFolder\\{}", entry.shell_id)
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOASYNC | SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI,
            lpVerb: windows::core::w!("open"),
            lpFile: PCWSTR(file.as_ptr()),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        ShellExecuteExW(&mut info).map_err(|_| anyhow::anyhow!("application_start_failed"))?;
        if !info.hProcess.is_invalid() {
            let _ = CloseHandle(info.hProcess);
        }
        Ok(())
    })
}
