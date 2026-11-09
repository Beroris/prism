//! New versions of Prism, from its releases on GitHub, installed with one click.
//!
//! Prism looks shortly after it starts and then once a day (Settings can turn that off), or
//! when asked in Settings. A newer version downloads straight away, in the background, and is
//! checked against the size and SHA-256 GitHub lists for it. Then a popup offers to install it:
//!
//! - The installed copy runs the new Setup in its update mode, with no window. Setup closes
//!   Prism, replaces the exe and starts the new version.
//! - The portable copy swaps its own exe for the new one under the same name, so shortcuts to it
//!   keep working, and starts it.
//! - Any other copy, such as a build from source, links to the release page instead.
//!
//! The check asks GitHub for the latest release and nothing else. Downloads come only from
//! github.com. Network work runs on a worker thread, which posts what it found to the main
//! window, so nothing here can hold up a capture.

use crate::util::{SendHwnd, take_boxed, wide};
use prism_core::update::{self as feed, Download, Kind, Release};
use std::cell::RefCell;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows::Win32::Networking::WinHttp::{
    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE, WINHTTP_OPTION_ENABLE_HTTP_PROTOCOL, WINHTTP_PROTOCOL_FLAG_HTTP2,
    WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest,
    WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetOption, WinHttpSetTimeouts,
};
use windows::Win32::Security::Cryptography::{BCRYPT_SHA256_ALG_HANDLE, BCryptHash};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer, WM_APP};
use windows::core::PCWSTR;

/// The version this copy compares against. A test build can pretend to be older.
pub const CURRENT: &str = match option_env!("PRISM_TEST_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};
/// For testing the whole path without publishing: a local copy of GitHub's answer, whose files'
/// addresses start `https://github.com/local/` and are read from beside it. Never set for a
/// release.
const TEST_FEED: Option<&str> = option_env!("PRISM_TEST_FEED");

/// The worker's news, posted to the main window.
pub const WM_UPDATE: u32 = WM_APP + 5;
/// The first look, a little after starting, then a tick every hour that looks once a day.
pub const CHECK_TIMER: usize = 21;
/// A popup waiting for a capture to finish.
pub const PROMPT_TIMER: usize = 22;
const FIRST_CHECK_MS: u32 = 8_000;
const TICK_MS: u32 = 60 * 60 * 1000;
const PROMPT_RETRY_MS: u32 = 4_000;
const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// The installer's AppId, which Windows lists the installed app under.
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{6F1C2B7E-4D3A-4E8B-9C21-5A7D0F3B8E64}_is1";
/// The installed copy's download waits here, in Windows' temp folder.
const DOWNLOAD_FOLDER: &str = "Prism update";
/// Passed to the new portable copy with the old one's process id, so it waits for it to go.
pub const AFTER_UPDATE_ARG: &str = "--after-update";
/// Passed by Setup when it starts Prism again after an update.
pub const UPDATED_ARG: &str = "--updated";

/// Where an update has got to, for Settings, the tray and the popup.
#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    Idle,
    Checking,
    UpToDate,
    /// A newer version is coming down.
    Downloading(String),
    /// Downloaded and checked: one click installs it.
    Ready(String),
    /// Newer, but this copy can't install it: the release page has it.
    Available(String),
    Installing,
    Failed(String),
}

struct Updater {
    main: HWND,
    kind: Kind,
    phase: Phase,
    /// Asked for in Settings, so a failure is shown rather than kept quiet.
    manual: bool,
    last_check: Option<SystemTime>,
    file: Option<PathBuf>,
    page: String,
    /// The version the popup has offered, so it's offered once.
    offered: Option<String>,
    cleaned: bool,
}

thread_local! {
    static UPDATER: RefCell<Option<Updater>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Updater) -> R) -> Option<R> {
    UPDATER.with(|u| u.borrow_mut().as_mut().map(f))
}

/// What the worker found.
enum Event {
    Found(Release),
    UpToDate,
    Failed(String),
    Downloaded(PathBuf),
    DownloadFailed(String),
}

// --- On the main thread ---------------------------------------------------------------------

/// Sets up the updater for the main window, and the first look a little later.
pub fn start(main: HWND) {
    let kind = this_copy();
    UPDATER.with(|u| {
        *u.borrow_mut() = Some(Updater {
            main,
            kind,
            phase: Phase::Idle,
            manual: false,
            last_check: None,
            file: None,
            page: feed::RELEASES_PAGE.to_string(),
            offered: None,
            cleaned: false,
        })
    });
    unsafe {
        SetTimer(Some(main), CHECK_TIMER, FIRST_CHECK_MS, None);
    }
}

/// Where the update has got to.
pub fn phase() -> Phase {
    with(|u| u.phase.clone()).unwrap_or(Phase::Idle)
}

/// The main window's timers that are the updater's.
pub fn on_timer(id: usize) {
    let Some(main) = with(|u| u.main) else { return };
    match id {
        CHECK_TIMER => unsafe {
            let _ = KillTimer(Some(main), CHECK_TIMER);
            SetTimer(Some(main), CHECK_TIMER, TICK_MS, None);
            let wanted = crate::app::settings_snapshot().is_none_or(|s| s.check_updates);
            // By the clock on the wall, so a day asleep counts.
            let due = with(|u| u.last_check.is_none_or(|at| at.elapsed().map_or(true, |since| since >= DAY))).unwrap_or(false);
            if wanted && due {
                begin(false);
            }
        },
        PROMPT_TIMER => unsafe {
            let _ = KillTimer(Some(main), PROMPT_TIMER);
            offer();
        },
        _ => {}
    }
}

/// Look now, as asked in Settings. With a version already downloaded, offer it again.
pub fn check_now() {
    if matches!(phase(), Phase::Ready(_)) {
        with(|u| u.offered = None);
        offer();
        return;
    }
    begin(true);
}

fn begin(manual: bool) {
    let Some((main, kind, clean)) = with(|u| {
        if matches!(u.phase, Phase::Checking | Phase::Downloading(_) | Phase::Ready(_) | Phase::Installing) {
            return None;
        }
        u.phase = Phase::Checking;
        u.manual = manual;
        u.last_check = Some(SystemTime::now());
        // By the first look, a Setup that started this copy has long finished: what it left
        // can go.
        let clean = !std::mem::replace(&mut u.cleaned, true);
        Some((SendHwnd::new(u.main), u.kind, clean))
    })
    .flatten() else {
        return;
    };
    changed();
    let _ = std::thread::Builder::new().name("update".into()).spawn(move || {
        if clean {
            clean_up();
        }
        match check(kind) {
            Err(e) => main.post_boxed(WM_UPDATE, 0, Event::Failed(e)),
            Ok(None) => main.post_boxed(WM_UPDATE, 0, Event::UpToDate),
            Ok(Some(release)) => {
                let download = release.download.clone().filter(|_| kind.installs_itself());
                main.post_boxed(WM_UPDATE, 0, Event::Found(release));
                if let Some(download) = download {
                    let event = match fetch(kind, &download) {
                        Ok(file) => Event::Downloaded(file),
                        Err(e) => Event::DownloadFailed(e),
                    };
                    main.post_boxed(WM_UPDATE, 0, event);
                }
            }
        }
    });
}

/// The worker's news arrived.
pub fn on_message(lp: LPARAM) {
    let event: Event = unsafe { take_boxed(lp) };
    let offer_it = with(|u| {
        match event {
            Event::Found(release) => {
                crate::util::log(&format!("update: found {}", release.version));
                u.page = release.page.clone();
                u.phase = if u.kind.installs_itself() && release.download.is_some() {
                    Phase::Downloading(release.version)
                } else {
                    Phase::Available(release.version)
                };
            }
            Event::UpToDate => {
                crate::util::log("update: up to date");
                u.phase = Phase::UpToDate;
            }
            Event::Downloaded(file) => {
                crate::util::log(&format!("update: downloaded {}", file.display()));
                if let Phase::Downloading(v) = &u.phase {
                    u.phase = Phase::Ready(v.clone());
                    u.file = Some(file);
                    return true;
                }
            }
            Event::Failed(e) | Event::DownloadFailed(e) => {
                crate::util::log(&format!("update: {e}"));
                if u.manual {
                    u.phase = Phase::Failed(e);
                } else {
                    // No connection or GitHub being busy will pass: stay quiet and look again
                    // in an hour.
                    u.phase = Phase::Idle;
                    u.last_check = None;
                }
            }
        }
        false
    })
    .unwrap_or(false);
    changed();
    if offer_it {
        offer();
    }
}

/// Settings and the tray show the news.
fn changed() {
    crate::settings_win::refresh();
}

/// The popup that offers to install the downloaded version, once. It waits while a capture
/// is on the screen, so it's never in one.
fn offer() {
    let Some((main, version)) = with(|u| match &u.phase {
        Phase::Ready(v) if u.offered.as_ref() != Some(v) => Some((u.main, v.clone())),
        _ => None,
    })
    .flatten() else {
        return;
    };
    if crate::overlay::current().is_some() {
        unsafe {
            SetTimer(Some(main), PROMPT_TIMER, PROMPT_RETRY_MS, None);
        }
        return;
    }
    with(|u| u.offered = Some(version.clone()));
    crate::toast::show_update(&version);
}

/// Installs the downloaded version, and quits so it can take over.
pub fn install() {
    let Some((kind, file)) = with(|u| match (&u.phase, &u.file) {
        (Phase::Ready(_), Some(file)) => Some((u.kind, file.clone())),
        _ => None,
    })
    .flatten() else {
        // Nothing downloaded for this copy: the release page has it.
        if let Phase::Available(_) = phase() {
            open_page();
        }
        return;
    };
    crate::util::log(&format!("update: installing {}", file.display()));
    match start_install(kind, &file) {
        Ok(()) => {
            with(|u| u.phase = Phase::Installing);
            changed();
            crate::app::run(crate::app::Command::Quit);
        }
        Err(e) => {
            with(|u| {
                u.phase = Phase::Failed(e.clone());
                u.file = None;
            });
            changed();
            crate::toast::show("Couldn't update Prism", &e, None, None, crate::toast::Click::None);
        }
    }
}

/// The new version's release page, with its notes.
pub fn open_page() {
    let page = with(|u| u.page.clone()).unwrap_or_else(|| feed::RELEASES_PAGE.to_string());
    if page.starts_with(feed::DOWNLOADS_FROM) {
        crate::util::shell_open(Path::new(&page));
    }
}

/// The notes for the version now running.
pub fn this_version_page() -> PathBuf {
    PathBuf::from(format!("{}Beroris/prism/releases/tag/v{}", feed::DOWNLOADS_FROM, env!("CARGO_PKG_VERSION")))
}

// --- On the worker ---------------------------------------------------------------------------

fn check(kind: Kind) -> Result<Option<Release>, String> {
    let body = get(feed::LATEST_RELEASE, true, 1 << 20)?;
    feed::parse_release(&body, kind, CURRENT)
}

/// Downloads `d`, checks it, and puts it where this kind of copy installs it from.
fn fetch(kind: Kind, d: &Download) -> Result<PathBuf, String> {
    let destination = download_path(kind, d)?;
    let limit = if d.size > 0 { d.size as usize } else { 64 << 20 };
    let body = get(&d.url, false, limit).map_err(|e| {
        if e.starts_with("Couldn't reach") { "Couldn't download the update. Check your connection.".to_string() } else { e }
    })?;
    if d.size > 0 && body.len() as u64 != d.size {
        return Err("The download was cut short. Try again.".into());
    }
    if let Some(sha) = &d.sha256 {
        if sha256_hex(&body).as_deref() != Some(sha.as_str()) {
            return Err("The download didn't match the release. Try again.".into());
        }
    }
    if !body.starts_with(b"MZ") {
        return Err("The download wasn't a program. Try again.".into());
    }
    let cant_save = |folder: &Path| format!("Couldn't save the update in {}.", folder.display());
    let folder = destination.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(folder).map_err(|_| cant_save(folder))?;
    let partial = sibling(&destination, ".part");
    std::fs::write(&partial, &body).map_err(|_| cant_save(folder))?;
    let _ = std::fs::remove_file(&destination);
    if std::fs::rename(&partial, &destination).is_err() {
        let _ = std::fs::remove_file(&partial);
        return Err(cant_save(folder));
    }
    Ok(destination)
}

fn sha256_hex(data: &[u8]) -> Option<String> {
    let mut out = [0u8; 32];
    let status = unsafe { BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, data, &mut out) };
    status.is_ok().then(|| out.iter().map(|b| format!("{b:02x}")).collect())
}

/// Closes a WinHTTP handle when it goes.
struct Net(*mut c_void);

impl Drop for Net {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

/// GETs `url` over HTTPS, following redirects, with no more than `limit` bytes back.
fn get(url: &str, api: bool, limit: usize) -> Result<Vec<u8>, String> {
    if let Some(feed_file) = TEST_FEED {
        return test_get(feed_file, url);
    }
    let offline = || "Couldn't reach GitHub. Check your connection.".to_string();
    let rest = url.strip_prefix("https://").ok_or_else(offline)?;
    let (host, path) = match rest.split_once('/') {
        Some((host, path)) => (host, format!("/{path}")),
        None => (rest, "/".to_string()),
    };
    unsafe {
        let agent = wide(&format!("Prism/{}", env!("CARGO_PKG_VERSION")));
        let session = Net(WinHttpOpen(PCWSTR(agent.as_ptr()), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0));
        if session.0.is_null() {
            return Err(offline());
        }
        // Each step gives up rather than wait for ever: resolving, connecting, sending, and
        // each read.
        let _ = WinHttpSetTimeouts(session.0, 10_000, 10_000, 15_000, 30_000);
        let _ = WinHttpSetOption(Some(session.0), WINHTTP_OPTION_ENABLE_HTTP_PROTOCOL, Some(&WINHTTP_PROTOCOL_FLAG_HTTP2.to_le_bytes()));
        let host = wide(host);
        let connection = Net(WinHttpConnect(session.0, PCWSTR(host.as_ptr()), 443, 0));
        if connection.0.is_null() {
            return Err(offline());
        }
        let (verb, path) = (wide("GET"), wide(&path));
        let request = Net(WinHttpOpenRequest(connection.0, PCWSTR(verb.as_ptr()), PCWSTR(path.as_ptr()), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), WINHTTP_FLAG_SECURE));
        if request.0.is_null() {
            return Err(offline());
        }
        let headers: Vec<u16> =
            if api { "Accept: application/vnd.github+json\r\nX-GitHub-Api-Version: 2022-11-28\r\n" } else { "" }.encode_utf16().collect();
        WinHttpSendRequest(request.0, (!headers.is_empty()).then_some(&headers[..]), None, 0, 0, 0).map_err(|_| offline())?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut()).map_err(|_| offline())?;
        let (mut status, mut size) = (0u32, 4u32);
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut c_void),
            &mut size,
            std::ptr::null_mut(),
        )
        .map_err(|_| offline())?;
        match status {
            200 => {}
            404 => return Err("Couldn't find Prism's releases on GitHub.".into()),
            403 | 429 => return Err("GitHub is busy. Try again later.".into()),
            other => return Err(format!("GitHub answered {other}. Try again later.")),
        }
        let mut body = Vec::new();
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            let mut read = 0u32;
            WinHttpReadData(request.0, chunk.as_mut_ptr() as *mut c_void, chunk.len() as u32, &mut read)
                .map_err(|_| "The download stopped. Check your connection and try again.".to_string())?;
            if read == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..read as usize]);
            if body.len() > limit {
                return Err("GitHub sent more than the release lists. Try again later.".into());
            }
        }
        Ok(body)
    }
}

/// [`get`] for a test build: GitHub's answer and the files come from a folder.
fn test_get(feed_file: &str, url: &str) -> Result<Vec<u8>, String> {
    std::thread::sleep(Duration::from_millis(300));
    let path = if url == feed::LATEST_RELEASE {
        PathBuf::from(feed_file)
    } else {
        let name = url.strip_prefix("https://github.com/local/").filter(|n| feed::is_plain_file_name(n)).ok_or("Not a test file.")?;
        Path::new(feed_file).with_file_name(name)
    };
    std::fs::read(path).map_err(|_| "Couldn't reach GitHub. Check your connection.".to_string())
}

// --- Installing -------------------------------------------------------------------------------

/// Where this kind of copy keeps the new version until it's installed.
fn download_path(kind: Kind, d: &Download) -> Result<PathBuf, String> {
    match kind {
        Kind::Installed => Ok(std::env::temp_dir().join(DOWNLOAD_FOLDER).join(&d.name)),
        // Beside the exe, so moving it into place can't fail for being on another drive.
        Kind::Portable => std::env::current_exe().map(|exe| sibling(&exe, ".new")).map_err(|_| "Couldn't find where Prism is.".to_string()),
        Kind::Other => Err("This copy of Prism can't update itself.".into()),
    }
}

/// `path` with `suffix` added to its whole file name.
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

/// Starts installing the downloaded version. Prism quits straight after.
fn start_install(kind: Kind, file: &Path) -> Result<(), String> {
    match kind {
        Kind::Installed => {
            // Setup's update mode keeps the user's choices as they are, shows nothing, and
            // starts Prism again when it's done.
            std::process::Command::new(file)
                .args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/UPDATE"])
                .spawn()
                .map_err(|_| "Couldn't start the installer. Try again.".to_string())?;
        }
        Kind::Portable => {
            let exe = std::env::current_exe().map_err(|_| "Couldn't find where Prism is.".to_string())?;
            swap_in(&exe, file)?;
            if std::process::Command::new(&exe).args([AFTER_UPDATE_ARG, &std::process::id().to_string(), "--background"]).spawn().is_err() {
                // The new version is in place, so opening Prism again starts it.
                return Err("Updated. Open Prism again to start the new version.".into());
            }
        }
        Kind::Other => return Err("This copy of Prism can't update itself.".into()),
    }
    Ok(())
}

/// Puts `new` where `exe` is, keeping the old one as "….old" until it has quit. Windows lets a
/// running exe be renamed, though not deleted.
fn swap_in(exe: &Path, new: &Path) -> Result<(), String> {
    let old = sibling(exe, ".old");
    let _ = std::fs::remove_file(&old);
    std::fs::rename(exe, &old).map_err(|_| "Couldn't make room for the new version.".to_string())?;
    if std::fs::rename(new, exe).is_err() {
        let _ = std::fs::rename(&old, exe);
        return Err("Couldn't put the new version in place.".into());
    }
    Ok(())
}

/// Run first thing at startup, before Prism checks it's the only copy. After a portable update
/// it waits for the old version to quit, so the new one isn't turned away as a second copy, and
/// removes it. Says whether this start follows an update.
pub fn finish_update(args: &[String]) -> bool {
    if let Some(at) = args.iter().position(|a| a == AFTER_UPDATE_ARG) {
        if let Some(pid) = args.get(at + 1).and_then(|id| id.parse::<u32>().ok()) {
            wait_for_exit(pid, 10_000);
        }
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::fs::remove_file(sibling(&exe, ".old"));
        }
        return true;
    }
    args.iter().any(|a| a == UPDATED_ARG)
}

fn wait_for_exit(pid: u32, ms: u32) {
    unsafe {
        if let Ok(process) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            let _ = WaitForSingleObject(process, ms);
            let _ = CloseHandle(process);
        }
    }
}

/// Clears what an update left: the old portable exe, a download that never finished, and the
/// Setup that ran.
fn clean_up() {
    if let Ok(exe) = std::env::current_exe() {
        for suffix in [".old", ".new", ".new.part"] {
            let leftover = sibling(&exe, suffix);
            if leftover.exists() {
                let _ = std::fs::remove_file(leftover);
            }
        }
    }
    let folder = std::env::temp_dir().join(DOWNLOAD_FOLDER);
    if folder.exists() {
        let _ = std::fs::remove_dir_all(folder);
    }
}

/// How this copy updates: by the portable name, or by being where Setup installed Prism.
fn this_copy() -> Kind {
    if crate::util::is_portable() {
        return Kind::Portable;
    }
    let Some(installed) = crate::system::read_string(UNINSTALL_KEY, "InstallLocation") else { return Kind::Other };
    let same = |a: &Path, b: &Path| match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy()),
        _ => false,
    };
    match std::env::current_exe().ok().as_deref().and_then(Path::parent) {
        Some(folder) if same(folder, Path::new(installed.trim())) => Kind::Installed,
        _ => Kind::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_the_standard() {
        assert_eq!(sha256_hex(b"abc").unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(sha256_hex(b"").unwrap(), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }

    #[test]
    fn a_portable_swap_keeps_the_name() {
        let dir = std::env::temp_dir().join(format!("prism-swap-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("Prism-1.0.0-Portable.exe");
        let new = sibling(&exe, ".new");
        std::fs::write(&exe, b"old").unwrap();
        std::fs::write(&new, b"new").unwrap();
        swap_in(&exe, &new).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        assert_eq!(std::fs::read(sibling(&exe, ".old")).unwrap(), b"old");
        assert!(!new.exists());
        // Nothing to swap in: the old one stays where it was.
        assert!(swap_in(&exe, &new).is_err());
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
