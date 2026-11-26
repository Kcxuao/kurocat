use std::{
    mem::{self, size_of},
    ptr::null_mut,
    time::Duration,
};
use tauri::WebviewWindow;
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, HWND, RECT, STILL_ACTIVE},
    Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST},
    System::{
        RemoteDesktop::{
            WTSActive, WTSFreeMemory, WTSQuerySessionInformationW, WTSSessionInfoEx, WTSINFOEXW,
            WTS_CURRENT_SESSION, WTS_SESSIONSTATE_UNLOCK,
        },
        Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    },
    UI::WindowsAndMessaging::{
        GetClassNameW, GetGUIThreadInfo, GetTopWindow, GetWindow, GetWindowLongW, GetWindowRect,
        GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, SetWindowPos, GUITHREADINFO,
        GUI_INMENUMODE, GUI_INMOVESIZE, GUI_POPUPMENUMODE, GUI_SYSTEMMENUMODE, GWL_EXSTYLE,
        GW_HWNDNEXT, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
        WS_EX_TOOLWINDOW,
    },
};

pub(super) fn awake_time() -> Option<Duration> {
    use windows_sys::Win32::System::WindowsProgramming::QueryUnbiasedInterruptTime;

    let mut ticks = 0;
    (unsafe { QueryUnbiasedInterruptTime(&mut ticks) } != 0)
        .then(|| Duration::from_nanos(ticks.saturating_mul(100)))
}

pub(super) fn active_session() -> bool {
    unsafe {
        let mut buffer = null_mut();
        let mut size = 0;
        if WTSQuerySessionInformationW(
            null_mut(),
            WTS_CURRENT_SESSION,
            WTSSessionInfoEx,
            &mut buffer,
            &mut size,
        ) == 0
            || buffer.is_null()
        {
            return false;
        }
        let active = if size as usize >= size_of::<WTSINFOEXW>() {
            let info = &*buffer.cast::<WTSINFOEXW>();
            info.Level == 1 && {
                let details = info.Data.WTSInfoExLevel1;
                details.SessionState == WTSActive
                    && details.SessionFlags as u32 == WTS_SESSIONSTATE_UNLOCK
            }
        } else {
            false
        };
        WTSFreeMemory(buffer.cast());
        active
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct TargetKey {
    hwnd: isize,
    pid: u32,
    tid: u32,
}

impl TargetKey {
    fn from_window(hwnd: HWND) -> Option<Self> {
        let mut pid = 0;
        let tid = unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        (tid != 0).then_some(Self {
            hwnd: hwnd as isize,
            pid,
            tid,
        })
    }
}

fn topmost_application_window(own_pid: u32) -> Option<TargetKey> {
    unsafe {
        let mut hwnd = GetTopWindow(null_mut());
        for _ in 0..4096 {
            if hwnd.is_null() {
                break;
            }
            if IsWindowVisible(hwnd) != 0
                && IsIconic(hwnd) == 0
                && GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW == 0
            {
                if let Some(key) = TargetKey::from_window(hwnd).filter(|key| key.pid != own_pid) {
                    let mut class = [0u16; 80];
                    let n = GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32);
                    if n > 0
                        && !matches!(
                            String::from_utf16_lossy(&class[..n as usize]).as_str(),
                            "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
                        )
                        && rect(hwnd).is_some_and(|r| r.right > r.left && r.bottom > r.top)
                    {
                        return Some(key);
                    }
                }
            }
            hwnd = GetWindow(hwnd, GW_HWNDNEXT);
        }
        None
    }
}

struct Process(HANDLE);
impl Drop for Process {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

pub(super) struct Target {
    hwnd: HWND,
    pid: u32,
    tid: u32,
    process: Process,
    origin: RECT,
    work: RECT,
}

fn rect(hwnd: HWND) -> Option<RECT> {
    unsafe {
        let mut rect = mem::zeroed();
        (GetWindowRect(hwnd, &mut rect) != 0).then_some(rect)
    }
}

fn same(a: RECT, b: RECT) -> bool {
    (a.left, a.top, a.right, a.bottom) == (b.left, b.top, b.right, b.bottom)
}

impl Target {
    pub(super) fn topmost_application() -> Option<Self> {
        Self::from_key(topmost_application_window(std::process::id())?)
    }

    fn from_key(key: TargetKey) -> Option<Self> {
        unsafe {
            let hwnd = key.hwnd as HWND;
            if TargetKey::from_window(hwnd)? != key {
                return None;
            }
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, key.pid);
            if handle.is_null() {
                return None;
            }
            let process = Process(handle);
            let origin = rect(hwnd)?;
            let mut monitor: MONITORINFO = mem::zeroed();
            monitor.cbSize = size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(
                MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST),
                &mut monitor,
            ) == 0
            {
                return None;
            }
            Some(Self {
                hwnd,
                pid: key.pid,
                tid: key.tid,
                process,
                origin,
                work: monitor.rcWork,
            })
        }
    }

    fn intact(&self) -> bool {
        unsafe {
            let mut pid = 0;
            let mut code = 0;
            IsWindow(self.hwnd) != 0
                && GetWindowThreadProcessId(self.hwnd, &mut pid) == self.tid
                && pid == self.pid
                && GetExitCodeProcess(self.process.0, &mut code) != 0
                && code == STILL_ACTIVE as u32
        }
    }

    fn free_to_move(&self) -> bool {
        unsafe {
            let mut info: GUITHREADINFO = mem::zeroed();
            info.cbSize = size_of::<GUITHREADINFO>() as u32;
            GetGUIThreadInfo(self.tid, &mut info) != 0
                && info.hwndCapture.is_null()
                && info.flags
                    & (GUI_INMOVESIZE | GUI_INMENUMODE | GUI_POPUPMENUMODE | GUI_SYSTEMMENUMODE)
                    == 0
        }
    }

    pub(super) fn overlay_position(&self, window: &WebviewWindow) -> (i32, i32) {
        let size = window.outer_size().ok();
        let width = size.map_or(92, |s| s.width as i32);
        let height = size.map_or(92, |s| s.height as i32);
        let x = (self.origin.right - width / 2).clamp(
            self.work.left,
            (self.work.right - width).max(self.work.left),
        );
        let y = (self.origin.top + 22).clamp(
            self.work.top,
            (self.work.bottom - height).max(self.work.top),
        );
        (x, y)
    }

    pub(super) fn pet_landings(
        &self,
        width: i32,
        height: i32,
        scale: f64,
        tug: bool,
    ) -> [crate::feeding::Point; 2] {
        let visible_left = self.origin.left.max(self.work.left);
        let visible_right = self.origin.right.min(self.work.right).max(visible_left);
        let span = visible_right - visible_left;
        let margin = (50.0 * scale) as i32;
        let anchors = if tug {
            [visible_right - margin.min(span / 2); 2]
        } else {
            [visible_left + span / 4, visible_right - span / 4]
        };
        let top = self.origin.top.clamp(self.work.top, self.work.bottom);
        let y = (top - height / 2 + (52.0 * scale) as i32).clamp(
            self.work.top,
            (self.work.bottom - height).max(self.work.top),
        );
        anchors.map(|x| crate::feeding::Point {
            x: (x - width * 2 / 3).clamp(
                self.work.left,
                (self.work.right - width).max(self.work.left),
            ),
            y,
        })
    }

    fn move_to(&self, x: i32, y: i32) -> bool {
        unsafe {
            SetWindowPos(
                self.hwnd,
                null_mut(),
                x,
                y,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOSIZE | SWP_NOZORDER | SWP_ASYNCWINDOWPOS,
            ) != 0
        }
    }

    pub(super) fn shake(&self, valid: impl Fn() -> bool) -> bool {
        self.nudge(
            &[
                (-44, 0),
                (44, 0),
                (-35, 0),
                (35, 0),
                (-21, 0),
                (21, 0),
                (-9, 0),
                (9, 0),
            ],
            Duration::from_millis(72),
            valid,
        )
    }

    pub(super) fn nudge(
        &self,
        offsets: &[(i32, i32)],
        interval: Duration,
        valid: impl Fn() -> bool,
    ) -> bool {
        let mut expected = self.origin;
        let mut moved = false;
        for &(dx, dy) in offsets {
            if !valid()
                || !self.intact()
                || !rect(self.hwnd).is_some_and(|current| same(current, expected))
            {
                break;
            }
            let next = RECT {
                left: self.origin.left + dx,
                right: self.origin.right + dx,
                top: self.origin.top + dy,
                bottom: self.origin.bottom + dy,
            };
            if !self.move_to(next.left, next.top) {
                break;
            }
            std::thread::sleep(interval);
            let mut actual = rect(self.hwnd);
            for _ in 0..5 {
                if !actual.is_some_and(|current| same(current, expected) && !same(current, next)) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(15));
                actual = rect(self.hwnd);
            }
            if !actual.is_some_and(|current| same(current, next)) {
                // A slow target may still have the asynchronous move queued.
                if actual.is_some_and(|current| same(current, expected))
                    && self.intact()
                    && self.free_to_move()
                {
                    self.move_to(self.origin.left, self.origin.top);
                }
                break;
            }
            expected = next;
            moved |= !same(next, self.origin);
        }
        if !same(expected, self.origin)
            && self.intact()
            && self.free_to_move()
            && rect(self.hwnd).is_some_and(|current| same(current, expected))
        {
            self.move_to(self.origin.left, self.origin.top);
        }
        moved
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, IsZoomed, ShowWindow, HWND_TOPMOST, SW_MAXIMIZE,
        WS_EX_NOACTIVATE, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
    };

    #[test]
    #[ignore = "creates a temporary window on an interactive Windows desktop"]
    fn shake_moves_and_restores_a_maximized_window() {
        let class: Vec<_> = "STATIC".encode_utf16().chain([0]).collect();
        let title: Vec<_> = "KuroCat 测试窗口".encode_utf16().chain([0]).collect();
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE,
                class.as_ptr(),
                title.as_ptr(),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                120,
                140,
                460,
                300,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
            )
        };
        assert!(!hwnd.is_null());
        struct Window(HWND);
        impl Drop for Window {
            fn drop(&mut self) {
                unsafe {
                    DestroyWindow(self.0);
                }
            }
        }
        let _window = Window(hwnd);
        unsafe {
            ShowWindow(hwnd, SW_MAXIMIZE);
        }
        assert_ne!(unsafe { IsZoomed(hwnd) }, 0);
        assert_ne!(
            unsafe {
                SetWindowPos(
                    hwnd,
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOACTIVATE
                        | SWP_NOSIZE
                        | windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOMOVE,
                )
            },
            0
        );
        let key = topmost_application_window(0).expect("a visible application window");
        assert_eq!(key.hwnd, hwnd as isize);
        let target = Target::from_key(key).unwrap();
        assert!(target.shake(|| true));
        assert!(same(rect(hwnd).unwrap(), target.origin));
        for pattern in [
            super::super::motion::TUG,
            super::super::motion::LEFT_LANDING,
            super::super::motion::RIGHT_LANDING,
        ] {
            assert!(target.nudge(pattern, Duration::from_millis(18), || true));
            assert!(same(rect(hwnd).unwrap(), target.origin));
        }
        assert_ne!(unsafe { IsZoomed(hwnd) }, 0);
    }
}
