use std::{ffi::OsStr, os::windows::ffi::OsStrExt, path::Path};
use windows::{
    core::{Interface, PCWSTR},
    Win32::{
        Foundation::HWND,
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, IPersistFile,
            CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, STGM_READ,
        },
        UI::Shell::{
            Common::COMDLG_FILTERSPEC, FileOpenDialog, IFileOpenDialog, IShellLinkW, ShellLink,
            FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM, FOS_NOCHANGEDIR, FOS_NODEREFERENCELINKS,
            FOS_PICKFOLDERS, SIGDN_FILESYSPATH, SLR_NOSEARCH, SLR_NOUPDATE, SLR_NO_UI,
        },
    },
};
fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(Some(0)).collect()
}
struct Com;
impl Com {
    fn new() -> Result<Self, String> {
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED)
                .ok()
                .map_err(|e| e.to_string())?;
        }
        Ok(Self)
    }
}
impl Drop for Com {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}
pub fn pick(owner: isize, folder: bool) -> Result<Option<String>, String> {
    let _com = Com::new()?;
    unsafe {
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| e.to_string())?;
        let mut flags = dialog.GetOptions().map_err(|e| e.to_string())?
            | FOS_FORCEFILESYSTEM
            | FOS_NODEREFERENCELINKS
            | FOS_NOCHANGEDIR;
        if folder {
            flags |= FOS_PICKFOLDERS;
        } else {
            flags |= FOS_FILEMUSTEXIST;
        }
        dialog.SetOptions(flags).map_err(|e| e.to_string())?;
        let label = wide("应用与 Windows 快捷方式");
        let filter = wide("*.exe;*.lnk");
        if !folder {
            dialog
                .SetFileTypes(&[COMDLG_FILTERSPEC {
                    pszName: PCWSTR(label.as_ptr()),
                    pszSpec: PCWSTR(filter.as_ptr()),
                }])
                .map_err(|e| e.to_string())?;
        }
        if let Err(e) = dialog.Show(HWND(owner as *mut _)) {
            if e.code().0 as u32 == 0x800704c7 {
                return Ok(None);
            }
            return Err(format!("无法打开选择器：{e}"));
        }
        let item = dialog.GetResult().map_err(|e| e.to_string())?;
        let ptr = item
            .GetDisplayName(SIGDN_FILESYSPATH)
            .map_err(|e| e.to_string())?;
        let result = ptr.to_string().map_err(|e| e.to_string());
        CoTaskMemFree(Some(ptr.0 as *const _));
        result.map(Some)
    }
}
fn check_link(path: &str) -> Result<(), String> {
    let _com = Com::new()?;
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| format!("无法读取快捷方式：{e}"))?;
        let file: IPersistFile = link.cast().map_err(|e| e.to_string())?;
        let p = wide(path);
        file.Load(PCWSTR(p.as_ptr()), STGM_READ)
            .map_err(|e| format!("快捷方式无法读取：{e}"))?;
        // Never show the Shell's own repair UI or rewrite the user's .lnk file.
        link.Resolve(
            HWND::default(),
            (SLR_NO_UI.0 | SLR_NOSEARCH.0 | SLR_NOUPDATE.0) as u32 | (500u32 << 16),
        )
        .map_err(|_| "INVALID_PATH:快捷方式目标已失效，请重新选择".to_string())?;
        let mut target = [0u16; 32768];
        link.GetPath(&mut target, std::ptr::null_mut(), 0)
            .map_err(|e| e.to_string())?;
        let len = target.iter().position(|v| *v == 0).unwrap_or(target.len());
        // Empty paths can refer to virtual Shell objects; let Windows launch those.
        if len > 0 {
            let target = String::from_utf16_lossy(&target[..len]);
            if !Path::new(&target).exists() {
                return Err("INVALID_PATH:快捷方式目标已失效，请重新选择".into());
            }
        }
        Ok(())
    }
}
pub fn launch(path: &str) -> Result<(), String> {
    use windows_sys::Win32::{
        Foundation::GetLastError,
        UI::{
            Shell::{ShellExecuteExW, SEE_MASK_FLAG_NO_UI, SHELLEXECUTEINFOW},
            WindowsAndMessaging::SW_SHOWNORMAL,
        },
    };
    if !Path::new(path).exists() {
        return Err("INVALID_PATH:路径已失效，请重新选择".into());
    }
    if !super::supported(Path::new(path)) {
        return Err("不支持此文件类型".into());
    }
    let lnk = Path::new(path)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("lnk"));
    if lnk {
        check_link(path)?;
    }
    let _com = Com::new()?;
    let p = wide(path);
    let verb = wide("open");
    let dir = wide(Path::new(path).parent().unwrap_or(Path::new("")));
    unsafe {
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = std::mem::size_of_val(&info) as u32;
        info.fMask = SEE_MASK_FLAG_NO_UI;
        info.lpVerb = verb.as_ptr();
        info.lpFile = p.as_ptr();
        info.nShow = SW_SHOWNORMAL;
        if !lnk && Path::new(path).is_file() {
            info.lpDirectory = dir.as_ptr();
        }
        if ShellExecuteExW(&mut info) == 0 {
            return Err(match GetLastError() {
                2 | 3 => "INVALID_PATH:路径或快捷方式目标已失效，请重新选择".into(),
                5 => "权限不足，无法打开此入口".into(),
                1223 => "已取消权限请求".into(),
                code => format!("启动失败，Windows 错误码：{code}"),
            });
        }
    }
    Ok(())
}
pub fn icon(path: &str) -> Result<Vec<u8>, String> {
    use windows_sys::Win32::{
        Graphics::Gdi::*,
        UI::{
            Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON},
            WindowsAndMessaging::{DestroyIcon, DrawIconEx, DI_NORMAL},
        },
    };
    let _com = Com::new()?;
    let p = wide(path);
    unsafe {
        let mut info: SHFILEINFOW = std::mem::zeroed();
        if SHGetFileInfoW(
            p.as_ptr(),
            0,
            &mut info,
            std::mem::size_of_val(&info) as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        ) == 0
            || info.hIcon.is_null()
        {
            return Err("图标不可用".into());
        }
        let dc = CreateCompatibleDC(std::ptr::null_mut());
        if dc.is_null() {
            DestroyIcon(info.hIcon);
            return Err("无法创建图标画布".into());
        }
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = 32;
        bmi.bmiHeader.biHeight = -32;
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB;
        let mut bits = std::ptr::null_mut();
        let bitmap = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        if bitmap.is_null() {
            DeleteDC(dc);
            DestroyIcon(info.hIcon);
            return Err("无法读取图标".into());
        }
        let old = SelectObject(dc, bitmap);
        let pixels = std::slice::from_raw_parts_mut(bits as *mut u8, 32 * 32 * 4);
        pixels.fill(0);
        DrawIconEx(
            dc,
            0,
            0,
            info.hIcon,
            32,
            32,
            0,
            std::ptr::null_mut(),
            DI_NORMAL,
        );
        let black = pixels.to_vec();
        pixels.fill(255);
        DrawIconEx(
            dc,
            0,
            0,
            info.hIcon,
            32,
            32,
            0,
            std::ptr::null_mut(),
            DI_NORMAL,
        );
        let white = pixels.to_vec();
        let mut rgba = vec![0; 32 * 32 * 4];
        for i in (0..rgba.len()).step_by(4) {
            let alpha = 255u8.saturating_sub(white[i].saturating_sub(black[i]));
            rgba[i + 3] = alpha;
            for (j, k) in [(0, 2), (1, 1), (2, 0)] {
                rgba[i + j] = if alpha == 0 {
                    0
                } else {
                    ((black[i + k] as u32 * 255) / alpha as u32).min(255) as u8
                };
            }
        }
        SelectObject(dc, old);
        DeleteObject(bitmap);
        DeleteDC(dc);
        DestroyIcon(info.hIcon);
        Ok(rgba)
    }
}
