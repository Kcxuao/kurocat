use super::{recycle, Point};
use std::{
    collections::hash_map::RandomState,
    ffi::{OsStr, OsString},
    fs,
    hash::BuildHasher,
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        fs::MetadataExt,
    },
    path::{Path, PathBuf},
    ptr::null_mut,
    sync::atomic::{AtomicU32, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
use windows::{
    core::{Interface, PCWSTR, VARIANT},
    Win32::{
        Foundation::{HANDLE, HWND},
        System::Com::{
            CoCreateInstance, CoTaskMemFree, IPersistFile, IServiceProvider, CLSCTX_INPROC_SERVER,
            STGM_READ,
        },
        UI::Shell::{
            FOLDERID_Desktop, IFolderView, IFolderView2, IShellBrowser, IShellFolder, IShellItem,
            IShellLinkW, IShellWindows, SHChangeNotify, SHCreateItemWithParent,
            SHGetKnownFolderPath, SID_STopLevelBrowser, ShellLink, ShellWindows, FWF_NOICONS,
            KF_FLAG_DEFAULT, SHCNE_CREATE, SHCNF_PATHW, SIGDN_FILESYSPATH, SWC_DESKTOP,
            SWFO_NEEDDISPATCH,
        },
    },
};

const PREFIX: &str = "KuroCat 零食 ";
const MARKER: &str = "KuroCat::DesktopFeeding::";

fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(Some(0)).collect()
}

fn desktop() -> windows::core::Result<PathBuf> {
    unsafe {
        let text = SHGetKnownFolderPath(&FOLDERID_Desktop, KF_FLAG_DEFAULT, HANDLE::default())?;
        let path = PathBuf::from(OsString::from_wide(text.as_wide()));
        CoTaskMemFree(Some(text.0.cast()));
        Ok(path)
    }
}

fn valid_name(name: &str) -> bool {
    name.starts_with(PREFIX)
        && name.ends_with(".lnk")
        && Path::new(name).file_name() == Some(OsStr::new(name))
}

pub(super) fn create(source: &Path) -> Result<String, String> {
    let _com = recycle::Com::new()?;
    let folder = desktop().map_err(|e| format!("无法找到 Windows 桌面：{e}"))?;
    if !folder.is_dir() {
        return Err("Windows 桌面文件夹不可用。".into());
    }
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let label: String = source
        .file_stem()
        .unwrap_or(OsStr::new("文件"))
        .to_string_lossy()
        .chars()
        .take(32)
        .collect();
    let name = format!(
        "{PREFIX}{label} [{stamp:x}-{:x}-{:x}].lnk",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let dest = folder.join(&name);
    // Reserve the new name before saving the shortcut to avoid overwriting a desktop item.
    let reservation = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&dest)
        .map_err(|e| e.to_string())?;
    drop(reservation);
    let result = unsafe {
        (|| -> windows::core::Result<()> {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            link.SetPath(PCWSTR(wide(source).as_ptr()))?;
            link.SetDescription(PCWSTR(wide(format!("{MARKER}{name}")).as_ptr()))?;
            let persist: IPersistFile = link.cast()?;
            let target = wide(&dest);
            persist.Save(PCWSTR(target.as_ptr()), true)?;
            SHChangeNotify(
                SHCNE_CREATE,
                SHCNF_PATHW,
                Some(target.as_ptr().cast()),
                None,
            );
            Ok(())
        })()
    };
    if let Err(error) = result {
        let _ = fs::remove_file(&dest);
        return Err(format!("创建桌面快捷方式失败：{error}"));
    }
    Ok(name)
}

// Read the marker from the .lnk itself so a replaced desktop file is never eaten.
fn tracked_path(name: &str) -> Result<Option<PathBuf>, String> {
    if !valid_name(name) {
        return Ok(None);
    }
    let path = desktop().map_err(|e| e.to_string())?.join(name);
    match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Ok(info) if info.is_file() && !info.file_type().is_symlink() => {}
        Ok(_) => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    unsafe {
        let link: IShellLinkW =
            CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(|e| e.to_string())?;
        let persist: IPersistFile = link.cast().map_err(|e| e.to_string())?;
        if persist
            .Load(PCWSTR(wide(&path).as_ptr()), STGM_READ)
            .is_err()
        {
            return Ok(None);
        }
        let mut description = [0u16; 1024];
        link.GetDescription(&mut description)
            .map_err(|e| e.to_string())?;
        let end = description
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(description.len());
        if String::from_utf16_lossy(&description[..end]) != format!("{MARKER}{name}") {
            return Ok(None);
        }
    }
    Ok(Some(path))
}

pub(super) fn discard_created(name: &str) {
    if let Ok(_com) = recycle::Com::new() {
        if let Ok(Some(path)) = tracked_path(name) {
            let _ = fs::remove_file(path);
        }
    }
}

pub(super) fn eat(name: &str) -> Result<bool, String> {
    let _com = recycle::Com::new()?;
    let Some(path) = tracked_path(name)? else {
        return Ok(false);
    };
    recycle::recycle(0, &path)?;
    Ok(true)
}

fn desktop_meal(path: &Path) -> Option<fs::Metadata> {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_SYSTEM,
    };

    let shortcut = path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("lnk") || ext.eq_ignore_ascii_case("url"));
    if shortcut {
        return None;
    }
    fs::symlink_metadata(path).ok().filter(|meta| {
        meta.is_file()
            && meta.file_attributes()
                & (FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM | FILE_ATTRIBUTE_REPARSE_POINT)
                == 0
    })
}

pub(super) struct DesktopFile {
    pub(super) path: PathBuf,
    metadata: fs::Metadata,
}

impl DesktopFile {
    fn still_matches(&self) -> bool {
        desktop_meal(&self.path).is_some_and(|current| {
            current.creation_time() == self.metadata.creation_time()
                && current.last_write_time() == self.metadata.last_write_time()
                && current.file_size() == self.metadata.file_size()
        })
    }

    pub(super) fn position(&self) -> Result<Location, String> {
        let _com = recycle::Com::new()?;
        match self.path.file_name() {
            Some(name) if self.still_matches() => icon_position(name, Some(&self.path)),
            _ => Ok(Location::Gone),
        }
    }

    pub(super) fn eat(&self, expected: Point) -> Result<bool, String> {
        if !matches!(self.position()?, Location::Found(point) if point.near(expected))
            || !self.still_matches()
        {
            return Ok(false);
        }
        recycle::recycle_desktop_file(&self.path)?;
        Ok(true)
    }
}

pub(super) fn random_files() -> Result<Vec<DesktopFile>, String> {
    let folder = desktop().map_err(|e| format!("无法找到 Windows 桌面：{e}"))?;
    let mut files: Vec<_> = fs::read_dir(folder)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let metadata = desktop_meal(&path)?;
            Some(DesktopFile { path, metadata })
        })
        .collect();
    let random = RandomState::new();
    files.sort_by_cached_key(|file| random.hash_one(&file.path));
    Ok(files)
}

pub(super) enum Location {
    Found(Point),
    Gone,
    NotVisible,
}
pub(super) fn position(name: &str) -> Result<Location, String> {
    let _com = recycle::Com::new()?;
    if tracked_path(name)?.is_none() {
        return Ok(Location::Gone);
    }
    icon_position(OsStr::new(name), None)
}

fn icon_position(name: &OsStr, expected_path: Option<&Path>) -> Result<Location, String> {
    let result = unsafe {
        (|| -> windows::core::Result<Option<Point>> {
            let shell: IShellWindows = CoCreateInstance(
                &ShellWindows,
                None,
                windows::Win32::System::Com::CLSCTX_LOCAL_SERVER,
            )?;
            let mut hwnd_number = 0;
            let desktop = shell.FindWindowSW(
                &VARIANT::from(0i32),
                &VARIANT::default(),
                SWC_DESKTOP,
                &mut hwnd_number,
                SWFO_NEEDDISPATCH,
            )?;
            let services: IServiceProvider = desktop.cast()?;
            let browser: IShellBrowser = services.QueryService(&SID_STopLevelBrowser)?;
            let view = browser.QueryActiveShellView()?;
            let hwnd = view.GetWindow()?;
            if windows_sys::Win32::UI::WindowsAndMessaging::IsWindowVisible(hwnd.0 as _) == 0 {
                return Ok(None);
            }
            if let Ok(options) = view.cast::<IFolderView2>() {
                if options
                    .GetCurrentFolderFlags()
                    .is_ok_and(|flags| flags & FWF_NOICONS.0 as u32 != 0)
                {
                    return Ok(None);
                }
            }
            let folder_view: IFolderView = view.cast()?;
            let folder: IShellFolder = folder_view.GetFolder()?;
            let mut pidl = null_mut();
            let mut attrs = 0;
            let name = wide(name);
            folder.ParseDisplayName(
                HWND::default(),
                None,
                PCWSTR(name.as_ptr()),
                None,
                &mut pidl,
                &mut attrs,
            )?;
            let point = (|| -> windows::core::Result<Option<_>> {
                if let Some(expected) = expected_path {
                    let item: IShellItem = SHCreateItemWithParent(None, &folder, pidl)?;
                    let display = item.GetDisplayName(SIGDN_FILESYSPATH)?;
                    let actual = PathBuf::from(OsString::from_wide(display.as_wide()));
                    CoTaskMemFree(Some(display.0.cast()));
                    if actual != expected {
                        return Ok(None);
                    }
                }
                folder_view.GetItemPosition(pidl).map(Some)
            })();
            CoTaskMemFree(Some(pidl.cast()));
            let Some(point) = point? else {
                return Ok(None);
            };
            let mut screen = windows_sys::Win32::Foundation::POINT {
                x: point.x,
                y: point.y,
            };
            if windows_sys::Win32::Graphics::Gdi::ClientToScreen(hwnd.0 as _, &mut screen) == 0 {
                return Ok(None);
            }
            Ok(Some(Point {
                x: screen.x,
                y: screen.y,
            }))
        })()
    };
    Ok(match result {
        Ok(Some(point)) => Location::Found(point),
        Ok(None) => Location::NotVisible,
        Err(error) => return Err(format!("无法读取桌面图标位置：{error}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_meals_exclude_shortcuts_and_directories() {
        let folder =
            std::env::temp_dir().join(format!("kurocat-glutton-test-{}", std::process::id()));
        fs::create_dir(&folder).unwrap();
        let file = folder.join("meal.txt");
        let link = folder.join("shortcut.LNK");
        let url = folder.join("website.url");
        for path in [&file, &link, &url] {
            fs::write(path, b"sample").unwrap();
        }
        let metadata = desktop_meal(&file).unwrap();
        assert!(desktop_meal(&link).is_none());
        assert!(desktop_meal(&url).is_none());
        assert!(desktop_meal(&folder).is_none());
        let meal = DesktopFile {
            path: file.clone(),
            metadata,
        };
        assert!(meal.still_matches());
        fs::write(&file, b"replaced contents").unwrap();
        assert!(!meal.still_matches());
        for path in [&file, &link, &url] {
            fs::remove_file(path).unwrap();
        }
        fs::remove_dir(folder).unwrap();
    }

    #[test]
    #[ignore = "requires an interactive Windows desktop and a disposable test file"]
    fn finds_regular_file_icon_on_windows_desktop() {
        let path = desktop()
            .unwrap()
            .join(format!("KuroCat glutton test {}.txt", std::process::id()));
        drop(
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .unwrap(),
        );
        let file = DesktopFile {
            metadata: desktop_meal(&path).unwrap(),
            path,
        };
        struct Cleanup<'a>(&'a DesktopFile);
        impl Drop for Cleanup<'_> {
            fn drop(&mut self) {
                if self.0.still_matches() {
                    let _ = fs::remove_file(&self.0.path);
                }
            }
        }
        let _cleanup = Cleanup(&file);
        let full_path = wide(&file.path);
        unsafe {
            SHChangeNotify(
                SHCNE_CREATE,
                SHCNF_PATHW,
                Some(full_path.as_ptr().cast()),
                None,
            )
        };
        for _ in 0..25 {
            match file.position() {
                Ok(Location::Found(_)) => return,
                Ok(_) => {}
                Err(error) => eprintln!("{error}"),
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        panic!("Explorer did not report the disposable desktop file's icon position");
    }

    #[test]
    #[ignore = "requires an interactive Windows desktop and temporarily creates its own test shortcut"]
    fn shell_desktop_icon_and_recycle_round_trip() {
        let folder =
            std::env::temp_dir().join(format!("kurocat-shell-feeding-{}", std::process::id()));
        fs::create_dir(&folder).unwrap();
        let sample = folder.join("feeding test.png");
        fs::write(&sample, b"original remains").unwrap();
        let name = create(&sample).unwrap();
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                discard_created(&self.0);
            }
        }
        let cleanup = Cleanup(name);
        let icon = (0..30)
            .find_map(|_| {
                let result = position(&cleanup.0);
                if let Err(e) = &result {
                    eprintln!("{e}");
                }
                if let Ok(Location::Found(point)) = result {
                    Some(point)
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    None
                }
            })
            .expect("Explorer must report the generated shortcut position");
        assert!(eat(&cleanup.0).unwrap());
        assert!(matches!(position(&cleanup.0), Ok(Location::Gone)));
        assert_eq!(fs::read(&sample).unwrap(), b"original remains");
        let direct = folder.join("direct feeding test.txt");
        fs::write(&direct, b"disposable direct-feed sample").unwrap();
        recycle::recycle(0, &direct).unwrap();
        assert!(!direct.exists());
        fs::remove_file(sample).unwrap();
        fs::remove_dir(folder).unwrap();
        println!(
            "Checked Explorer icon at ({}, {}), shortcut recycling and direct recycling",
            icon.x, icon.y
        );
    }
}
