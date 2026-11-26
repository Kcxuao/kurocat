//! The Shell owns recycling and its undo record. Never fall back to std::fs::remove_file.
use std::{
    ffi::OsStr,
    os::windows::ffi::OsStrExt,
    path::{Component, Path, Prefix},
};
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::HWND,
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
            COINIT_APARTMENTTHREADED,
        },
        UI::Shell::{
            FileOperation, IFileOperation, IFileOperationProgressSink, IShellItem,
            SHCreateItemFromParsingName, SHQueryRecycleBinW, FILEOPERATION_FLAGS,
            FOFX_ADDUNDORECORD, FOFX_EARLYFAILURE, FOFX_RECYCLEONDELETE, FOF_NOCONFIRMATION,
            FOF_NOERRORUI, FOF_NO_CONNECTED_ELEMENTS, FOF_SILENT, SHQUERYRBINFO,
        },
    },
};

fn wide(text: impl AsRef<OsStr>) -> Vec<u16> {
    text.as_ref().encode_wide().chain(Some(0)).collect()
}

pub(super) fn validate(path: &Path) -> Result<(), String> {
    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
    let drive_letter = match path.components().next() {
        Some(Component::Prefix(prefix)) if path.is_absolute() => match prefix.kind() {
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => Some(letter),
            _ => None,
        },
        _ => None,
    };
    let letter = drive_letter.ok_or("目前只支持回收本地磁盘上的文件。")?;
    if !path.is_file() {
        return Err("只能投喂文件，文件夹会留在原处。".into());
    }
    let drive = wide(format!("{}:\\", char::from(letter)));
    // Removable and network volumes can fall back to permanent deletion in Explorer.
    const DRIVE_FIXED: u32 = 3;
    if unsafe { GetDriveTypeW(drive.as_ptr()) } != DRIVE_FIXED {
        return Err("这个磁盘不支持桌宠回收，请用资源管理器处理。".into());
    }
    let mut info: SHQUERYRBINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of_val(&info) as u32;
    unsafe { SHQueryRecycleBinW(PCWSTR(drive.as_ptr()), &mut info) }
        .map_err(|_| "该磁盘的回收站不可用，文件未移动。".into())
}

pub(super) struct Com;
impl Com {
    pub(super) fn new() -> Result<Self, String> {
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

pub(super) fn recycle(owner: isize, path: &Path) -> Result<(), String> {
    recycle_with_flags(owner, path, FILEOPERATION_FLAGS(0))
}

pub(super) fn recycle_desktop_file(path: &Path) -> Result<(), String> {
    recycle_with_flags(0, path, FOF_NO_CONNECTED_ELEMENTS)
}

fn recycle_with_flags(
    owner: isize,
    path: &Path,
    extra_flags: FILEOPERATION_FLAGS,
) -> Result<(), String> {
    validate(path)?;
    unsafe {
        let _com = Com::new()?;
        let operation: IFileOperation =
            CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| e.to_string())?;
        operation
            .SetOwnerWindow(HWND(owner as _))
            .map_err(|e| e.to_string())?;
        operation
            .SetOperationFlags(
                FOFX_RECYCLEONDELETE
                    | FOFX_ADDUNDORECORD
                    | FOFX_EARLYFAILURE
                    | extra_flags
                    | FOF_NOCONFIRMATION
                    | FOF_SILENT
                    | FOF_NOERRORUI,
            )
            .map_err(|e| e.to_string())?;
        let source = wide(path);
        let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(source.as_ptr()), None)
            .map_err(|e| e.to_string())?;
        operation
            .DeleteItem(&item, None::<&IFileOperationProgressSink>)
            .map_err(|e| e.to_string())?;
        operation.PerformOperations().map_err(|e| e.to_string())?;
        if operation
            .GetAnyOperationsAborted()
            .map_err(|e| e.to_string())?
            .as_bool()
            || path.exists()
        {
            return Err("Windows 未将文件移入回收站。".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_virtual_network_and_directory_sources() {
        assert!(validate(Path::new(r"\\server\share\file.txt")).is_err());
        assert!(validate(Path::new("relative.txt")).is_err());
        assert!(validate(&std::env::temp_dir()).is_err());
    }
}
