//! Finding other screenshot apps' settings on this computer, for Settings to offer to bring
//! them over. Reading what they mean is `prism_core::import`; this only finds the files, fills
//! in folder names, and checks whether the app is running.

use crate::util::{known_folder, wide};
use prism_core::import::{self, App, Found};
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::ProcessStatus::EnumProcesses;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegCloseKey, RegGetValueW, RegOpenKeyExW,
};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_LocalAppData, FOLDERID_Music, FOLDERID_Pictures, FOLDERID_Profile, FOLDERID_RoamingAppData,
    FOLDERID_Videos,
};
use windows::core::PCWSTR;

/// Another app with settings Prism can use.
#[derive(Clone, Debug)]
pub struct Other {
    pub app: App,
    pub found: Found,
    /// It's running now, so its shortcuts are still taken.
    pub running: bool,
}

/// The apps found on this computer, in `App::ALL`'s order. Ones whose settings hold nothing
/// Prism can use are left out.
pub fn find() -> Vec<Other> {
    let running = running_exes();
    App::ALL
        .iter()
        .filter_map(|&app| {
            let mut found = read(app)?;
            // A folder that isn't there any more isn't worth bringing over.
            if found.save_dir.as_deref().is_some_and(|d| !Path::new(d).is_dir()) {
                found.save_dir = None;
            }
            if found.is_empty() {
                return None;
            }
            let exe = match app {
                App::ShareX => "sharex.exe",
                App::Greenshot => "greenshot.exe",
                App::Flameshot => "flameshot.exe",
                App::Lightshot => "lightshot.exe",
            };
            Some(Other { app, found, running: running.iter().any(|r| r == exe) })
        })
        .collect()
}

fn read(app: App) -> Option<Found> {
    match app {
        App::ShareX => {
            let dir = sharex_folder()?;
            let config = std::fs::read_to_string(dir.join("ApplicationConfig.json")).ok()?;
            let keys_path = import::sharex_hotkeys_path(&config)
                .and_then(|p| expand_percent(&p))
                .map(|p| if Path::new(&p).is_dir() { Path::new(&p).join("HotkeysConfig.json") } else { PathBuf::from(p) })
                .unwrap_or_else(|| dir.join("HotkeysConfig.json"));
            let keys = std::fs::read_to_string(keys_path).ok();
            Some(import::sharex(&config, keys.as_deref(), &folder_var))
        }
        App::Greenshot => {
            // Greenshot prefers a Greenshot.ini beside itself, as the portable copy has.
            let beside = install_dir("Greenshot_is1").map(|d| d.join("Greenshot.ini"));
            let roaming = known_folder(&FOLDERID_RoamingAppData, false).map(|d| d.join("Greenshot").join("Greenshot.ini"));
            let text = [beside, roaming].into_iter().flatten().find_map(|p| std::fs::read_to_string(p).ok())?;
            Some(import::greenshot(&text, &folder_var))
        }
        App::Flameshot => {
            let path = known_folder(&FOLDERID_RoamingAppData, false)?.join("flameshot").join("flameshot.ini");
            Some(import::flameshot(&std::fs::read_to_string(path).ok()?))
        }
        App::Lightshot => lightshot_installed().then(import::lightshot),
    }
}

/// Whether Lightshot is here: its settings key, or the program where its installer puts it.
fn lightshot_installed() -> bool {
    let key = wide("Software\\SkillBrains\\Lightshot");
    let mut handle = HKEY::default();
    if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(key.as_ptr()), Some(0), KEY_READ, &mut handle) }.is_ok() {
        unsafe {
            let _ = RegCloseKey(handle);
        }
        return true;
    }
    let program_files = std::env::var("ProgramFiles(x86)").ok().map(PathBuf::from);
    let local = known_folder(&FOLDERID_LocalAppData, false);
    let roaming = known_folder(&FOLDERID_RoamingAppData, false);
    [program_files, local, roaming].into_iter().flatten().any(|d| d.join("Skillbrains").join("lightshot").join("Lightshot.exe").is_file())
}

/// ShareX's own folder, where its settings are: where the registry or a PersonalPath.cfg points,
/// otherwise Documents\ShareX. The first that has its settings in it.
fn sharex_folder() -> Option<PathBuf> {
    let install = install_dir("82E6AC09-0FEF-4390-AD9F-0DD3F5561EFC_is1");
    let documents = known_folder(&FOLDERID_Documents, false);
    let mut places: Vec<PathBuf> = Vec::new();
    // The portable copy keeps them in a ShareX folder beside itself.
    if let Some(dir) = install.as_ref().filter(|d| d.join("Portable").exists()) {
        places.push(dir.join("ShareX"));
    }
    for hive in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        if let Some(p) = registry_string(hive, "Software\\ShareX", "PersonalPath") {
            places.push(PathBuf::from(p));
        }
    }
    let cfg_places = [install.clone(), documents.as_ref().map(|d| d.join("ShareX")), known_folder(&FOLDERID_LocalAppData, false).map(|d| d.join("ShareX"))];
    for cfg in cfg_places.into_iter().flatten().map(|d| d.join("PersonalPath.cfg")) {
        if let Ok(text) = std::fs::read_to_string(cfg) {
            let line = text.trim_start_matches('\u{feff}').lines().next().unwrap_or_default().trim().to_string();
            if !line.is_empty() {
                places.push(PathBuf::from(line));
            }
        }
    }
    places.extend(documents.map(|d| d.join("ShareX")));
    places
        .into_iter()
        .filter_map(|p| {
            let p = PathBuf::from(expand_percent(&p.to_string_lossy())?);
            // A relative path is from where ShareX is installed.
            if p.is_relative() { install.as_ref().map(|i| i.join(&p)) } else { Some(p) }
        })
        .find(|p| p.join("ApplicationConfig.json").is_file())
}

/// Where an installer put an app, from its uninstall entry.
fn install_dir(key: &str) -> Option<PathBuf> {
    let path = format!("Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{key}");
    [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE].into_iter().find_map(|hive| registry_string(hive, &path, "InstallLocation")).map(PathBuf::from).filter(|d| d.is_dir())
}

fn registry_string(hive: HKEY, path: &str, name: &str) -> Option<String> {
    let (p, n) = (wide(path), wide(name));
    let mut buf = vec![0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(hive, PCWSTR(p.as_ptr()), PCWSTR(n.as_ptr()), RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut size))
    };
    if status.is_err() {
        return None;
    }
    let text = String::from_utf16_lossy(&buf[..(size as usize / 2)]).trim_end_matches('\0').trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// A folder by the name .NET and Greenshot give it, such as `MyPictures`, or else an environment
/// variable.
fn folder_var(name: &str) -> Option<String> {
    let id = match name.to_ascii_lowercase().as_str() {
        "mypictures" => Some(FOLDERID_Pictures),
        "mydocuments" | "personal" => Some(FOLDERID_Documents),
        "desktop" | "desktopdirectory" => Some(FOLDERID_Desktop),
        "mymusic" => Some(FOLDERID_Music),
        "myvideos" => Some(FOLDERID_Videos),
        "applicationdata" => Some(FOLDERID_RoamingAppData),
        "localapplicationdata" => Some(FOLDERID_LocalAppData),
        "userprofile" => Some(FOLDERID_Profile),
        _ => None,
    };
    match id {
        Some(id) => known_folder(&id, false).map(|p| p.display().to_string()),
        None => std::env::var(name).ok().filter(|v| !v.is_empty()),
    }
}

/// `%Name%` filled in, as ShareX does with its own folder.
fn expand_percent(path: &str) -> Option<String> {
    let mut out = String::new();
    let mut rest = path;
    while let Some(start) = rest.find('%') {
        let after = &rest[start + 1..];
        let end = after.find('%')?;
        out.push_str(&rest[..start]);
        out.push_str(&folder_var(&after[..end])?);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

/// The file names of the programs running now, in lower case.
fn running_exes() -> Vec<String> {
    let mut ids = vec![0u32; 4096];
    let mut bytes = 0u32;
    if unsafe { EnumProcesses(ids.as_mut_ptr(), (ids.len() * 4) as u32, &mut bytes) }.is_err() {
        return Vec::new();
    }
    ids.truncate(bytes as usize / 4);
    ids.into_iter()
        .filter(|&id| id != 0)
        .filter_map(|id| unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, id).ok()?;
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
            let _ = CloseHandle(process);
            ok.then(|| String::from_utf16_lossy(&buf[..len as usize]))
        })
        .filter_map(|path| Path::new(&path).file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()))
        .collect()
}
