use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{
    AppHandle, DragDropEvent, Emitter, Manager, PhysicalPosition, WebviewWindow, WindowEvent,
};

mod desktop;
mod recycle;

const LABEL: &str = "feeding";
const LIMIT: usize = 12;
const WALK_MS: u64 = 1250;
const EAT_MS: u64 = 1200;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Paper,
    Picture,
    Archive,
    Other,
}

impl Kind {
    fn of(path: &Path) -> Self {
        match path
            .extension()
            .and_then(|p| p.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "doc" | "docx" | "pdf" | "txt" | "md" | "xlsx" | "pptx" => Self::Paper,
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" => Self::Picture,
            "zip" | "rar" | "7z" | "tar" | "gz" => Self::Archive,
            _ => Self::Other,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Snack {
    name: String,
    kind: Kind,
    icon: Vec<u8>,
    #[serde(default)]
    link: String,
}

struct Tray {
    snacks: VecDeque<Snack>,
    handfed: VecDeque<Kind>,
    hungry_at: Instant,
    request: bool,
    error: Option<String>,
}

pub struct State {
    tray: Mutex<Tray>,
    walking: AtomicBool,
    opening: AtomicBool,
    recycling: AtomicBool,
}

impl Default for State {
    fn default() -> Self {
        let loaded = match fs::read(queue_path()) {
            Ok(data) => serde_json::from_slice::<VecDeque<Snack>>(&data)
                .ok()
                .filter(|q| q.len() <= LIMIT)
                .ok_or("投喂记录无法读取，原记录已保留。"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(VecDeque::new()),
            Err(_) => Err("无法读取投喂记录。"),
        };
        let (snacks, error) = match loaded {
            Ok(mut q) => {
                // The previous prototype did not create desktop shortcuts.
                q.retain(|snack| !snack.link.is_empty());
                (q, None)
            }
            Err(e) => (VecDeque::new(), Some(e.into())),
        };
        Self {
            tray: Mutex::new(Tray {
                snacks,
                handfed: VecDeque::new(),
                hungry_at: Instant::now() + hunger(false),
                request: false,
                error,
            }),
            walking: AtomicBool::new(false),
            opening: AtomicBool::new(false),
            recycling: AtomicBool::new(false),
        }
    }
}

fn queue_path() -> PathBuf {
    crate::config::app_data_dir().join("feeding.json")
}

fn save_queue(path: &Path, queue: &VecDeque<Snack>) -> Result<(), String> {
    fs::create_dir_all(path.parent().ok_or("找不到投喂区数据目录。")?)
        .map_err(|e| e.to_string())?;
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    let data = serde_json::to_vec(queue).map_err(|e| e.to_string())?;
    let result = fs::write(&temp, data).and_then(|_| fs::rename(&temp, path));
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result.map_err(|e| format!("投喂记录保存失败：{e}"))
}

fn hunger(after_snack: bool) -> Duration {
    let noise = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as u64;
    Duration::from_secs(if after_snack {
        600 + noise % 361
    } else {
        120 + noise % 121
    })
}

fn changed(app: &AppHandle) {
    let _ = app.emit_to(LABEL, "kuro-feeding-changed", ());
}
fn notice(app: &AppHandle, message: String) {
    let _ = app.emit_to(LABEL, "kuro-feeding-notice", message);
}

fn drop_on_pet(app: AppHandle, files: Vec<PathBuf>) {
    if files.is_empty() {
        return;
    }
    let Some(owner) = app
        .get_webview_window("main")
        .and_then(|w| w.hwnd().ok())
        .map(|h| h.0 as isize)
    else {
        return;
    };
    let state = app.state::<State>();
    if state.recycling.swap(true, Ordering::AcqRel) {
        return;
    }
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<State>();
        let result = (|| -> Result<(), String> {
            if files.len() > LIMIT {
                return Err("一次最多回收 12 个文件。".into());
            }
            for path in &files {
                recycle::validate(path)?;
            }
            let mut kinds = Vec::new();
            let mut failures = Vec::new();
            for path in files {
                match recycle::recycle(owner, &path) {
                    Ok(()) => kinds.push(Kind::of(&path)),
                    Err(e) => failures.push(e),
                }
            }
            let count = kinds.len();
            let mut tray = state.tray.lock().unwrap();
            tray.handfed.extend(kinds);
            drop(tray);
            if failures.is_empty() {
                return Ok(());
            }
            Err(format!(
                "已回收 {count} 个文件；{} 个文件未能回收。{}",
                failures.len(),
                failures[0]
            ))
        })();
        if let Err(error) = result {
            let _ = app.emit_to("main", "kuro-feeding-drop-error", error);
        }
        state.recycling.store(false, Ordering::Release);
    });
}

fn drop_files(app: AppHandle, files: Vec<PathBuf>) {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<State>();
        let mut added = Vec::new();
        let mut failure = None;
        let slots = {
            let tray = state.tray.lock().unwrap();
            if let Some(error) = &tray.error {
                notice(&app, error.clone());
                return;
            }
            LIMIT.saturating_sub(tray.snacks.len())
        };
        if slots == 0 {
            notice(&app, "最多放 12 份桌面零食。".into());
            return;
        }
        for path in files.iter().filter(|p| p.is_file()).take(slots) {
            let Some(name) = path.file_name() else {
                continue;
            };
            match desktop::create(path) {
                Ok(link) => added.push(Snack {
                    name: name.to_string_lossy().chars().take(160).collect(),
                    kind: Kind::of(path),
                    icon: crate::shortcuts::native::icon(&path.to_string_lossy())
                        .unwrap_or_default(),
                    link,
                }),
                Err(error) => failure = Some(error),
            }
        }
        if added.is_empty() {
            notice(
                &app,
                failure.unwrap_or_else(|| "请从资源管理器拖入文件。".into()),
            );
            return;
        }
        let mut tray = state.tray.lock().unwrap();
        let mut next = tray.snacks.clone();
        let count = LIMIT.saturating_sub(next.len()).min(added.len());
        next.extend(added.iter().take(count).cloned());
        let result = save_queue(&queue_path(), &next);
        if result.is_ok() {
            tray.snacks = next;
        }
        drop(tray);
        let keep = if result.is_ok() { count } else { 0 };
        for snack in added.iter().skip(keep) {
            desktop::discard_created(&snack.link);
        }
        match result {
            Ok(()) => {
                changed(&app);
                notice(&app, format!("已在桌面放好 {count} 份文件零食。"));
            }
            Err(error) => notice(&app, error),
        }
    });
}

#[tauri::command]
pub async fn open_feeding(window: WebviewWindow, app: AppHandle) -> Result<(), String> {
    if window.label() != "menu" || crate::win_needs_win7_ulw() {
        return Err("当前桌宠窗口不支持投喂区。".into());
    }
    let state = app.state::<State>();
    if state.opening.swap(true, Ordering::AcqRel) {
        return Ok(());
    }
    let result = (|| {
        let tray = if let Some(tray) = app.get_webview_window(LABEL) {
            tray
        } else {
            let tray = tauri::WebviewWindowBuilder::new(
                &app,
                LABEL,
                tauri::WebviewUrl::App("feeding.html".into()),
            )
            .title("KuroCat · 文件投喂区")
            .inner_size(310., 330.)
            .resizable(false)
            .maximizable(false)
            .minimizable(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .visible(false)
            .additional_browser_args("--autoplay-policy=no-user-gesture-required")
            .build()
            .map_err(|e| e.to_string())?;
            let handle = app.clone();
            tray.on_window_event(move |event| {
                if let WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) = event {
                    drop_files(handle.clone(), paths.clone());
                }
            });
            if let Some((pet, work, dpi)) = crate::current_pet_rect_and_monitor(&app) {
                let size = tray
                    .outer_size()
                    .map(|s| (s.width as i32, s.height as i32))
                    .unwrap_or(((310. * dpi).round() as i32, (360. * dpi).round() as i32));
                let (x, y) = crate::compute_side_menu_pos(pet, work, size, (160. * dpi) as i32, 12);
                let _ = tray.set_position(PhysicalPosition::new(x, y));
            }
            tray
        };
        crate::close_context_menu(app.clone());
        tray.show().map_err(|e| e.to_string())?;
        tray.set_focus().map_err(|e| e.to_string())
    })();
    state.opening.store(false, Ordering::Release);
    result
}

fn only_tray(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == LABEL {
        Ok(())
    } else {
        Err("请从桌面投喂区操作。".into())
    }
}

#[tauri::command]
pub fn get_feeding(window: WebviewWindow, app: AppHandle) -> Result<serde_json::Value, String> {
    only_tray(&window)?;
    let state = app.state::<State>();
    let tray = state.tray.lock().unwrap();
    Ok(
        serde_json::json!({"snacks":tray.snacks, "busy":state.walking.load(Ordering::Relaxed),
        "waiting":tray.request, "hungry":Instant::now() >= tray.hungry_at, "error":tray.error}),
    )
}

#[tauri::command]
pub fn make_pet_hungry(window: WebviewWindow, app: AppHandle) -> Result<(), String> {
    only_tray(&window)?;
    let state = app.state::<State>();
    let mut tray = state.tray.lock().unwrap();
    if let Some(error) = &tray.error {
        return Err(error.clone());
    }
    if state.walking.load(Ordering::Acquire) {
        return Err("等桌宠吃完这份零食再试。".into());
    }
    tray.hungry_at = Instant::now();
    drop(tray);
    changed(&app);
    Ok(())
}

#[tauri::command]
pub fn try_feeding(window: WebviewWindow, app: AppHandle) -> Result<(), String> {
    only_tray(&window)?;
    let state = app.state::<State>();
    let mut tray = state.tray.lock().unwrap();
    if tray.snacks.is_empty() {
        return Err("先拖一份文件进来。".into());
    }
    if state.walking.load(Ordering::Acquire) {
        return Ok(());
    }
    tray.request = true;
    drop(tray);
    changed(&app);
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Point {
    x: i32,
    y: i32,
}
impl Point {
    fn of(p: PhysicalPosition<i32>) -> Self {
        Self { x: p.x, y: p.y }
    }
    fn position(self) -> PhysicalPosition<i32> {
        PhysicalPosition::new(self.x, self.y)
    }
    fn near(self, other: Self) -> bool {
        self.x.abs_diff(other.x) <= 4 && self.y.abs_diff(other.y) <= 4
    }
}

fn approach(
    pet: Point,
    size: (i32, i32),
    tray: Point,
    tray_size: (i32, i32),
    work: (i32, i32, i32, i32),
    gap: i32,
) -> Point {
    let (x, y, w, h) = work;
    let bounds_x = (x + w - size.0).max(x);
    let bounds_y = (y + h - size.1).max(y);
    let left = tray.x - size.0 - gap;
    let right = tray.x + tray_size.0 + gap;
    let overlaps_vertically = tray.y + tray_size.1 > y;
    let dest_x = match (left >= x, right <= bounds_x) {
        (true, true) if pet.x.abs_diff(left) <= pet.x.abs_diff(right) => left,
        (_, true) => right,
        (true, false) => left,
        _ => (tray.x + tray_size.0 / 2 - size.0 / 2).clamp(x, bounds_x),
    };
    let above = tray.y - size.1 - gap;
    let below = tray.y + tray_size.1 + gap;
    let dest_y = if left < x && right > bounds_x && overlaps_vertically {
        if above >= y {
            above
        } else {
            below
        }
    } else {
        tray.y + (tray_size.1 - size.1) / 2
    };
    Point {
        x: dest_x,
        y: dest_y.clamp(y, bounds_y),
    }
}

fn walk(from: Point, to: Point, elapsed: u64) -> Point {
    let t = (elapsed as f64 / WALK_MS as f64).clamp(0., 1.);
    let t = t * t * (3. - 2. * t);
    let axis = |a: i32, b: i32| (f64::from(a) + (f64::from(b) - f64::from(a)) * t).round() as i32;
    Point {
        x: axis(from.x, to.x),
        y: axis(from.y, to.y),
    }
}

struct Trip {
    origin: Point,
    target: Point,
    last: Point,
    desktop_item: Option<(String, Point)>,
    start: Instant,
    eating: bool,
    returning: bool,
    consumed: bool,
    kind: Kind,
}

fn pet_effect(app: &AppHandle, phase: &str, kind: Kind) {
    let _ = app.emit_to(
        "main",
        "kuro-feeding-effect",
        serde_json::json!({ "phase": phase, "kind": kind }),
    );
}

fn eligible(app: &AppHandle, main: &WebviewWindow) -> bool {
    let state = app.state::<crate::AppState>();
    let locked = {
        let settings = state.settings.lock().unwrap();
        settings.click_through_locked || (settings.obs_enabled && settings.obs_hide_pet)
    };
    !locked
        && !app.state::<State>().recycling.load(Ordering::Acquire)
        && !state.dragging.load(Ordering::Relaxed)
        && !state.paused_guard.load(Ordering::Relaxed)
        && !state.menu_open.load(Ordering::Relaxed)
        && !state.update_banner_open.load(Ordering::Relaxed)
        && !crate::shortcuts::is_showing(app)
        && !crate::ai_chat::bubble_visible(app)
        && !app
            .get_webview_window("ai-chat")
            .is_some_and(|win| win.is_visible().unwrap_or(false))
        && !crate::game_detect::is_game_active()
        && !crate::ai_chat::fullscreen_foreground()
        && main.is_visible().unwrap_or(false)
}

fn begin_trip(main: &WebviewWindow, icon: Point, snack: &Snack) -> Option<Trip> {
    let origin = Point::of(main.outer_position().ok()?);
    let pet_size = main.outer_size().ok()?;
    let monitor = main.current_monitor().ok()??;
    let area = monitor.work_area();
    if icon.x < area.position.x
        || icon.y < area.position.y
        || icon.x >= area.position.x + area.size.width as i32
        || icon.y >= area.position.y + area.size.height as i32
    {
        return None;
    }
    let icon_size = (48. * monitor.scale_factor()).round() as i32;
    let target = approach(
        origin,
        (pet_size.width as i32, pet_size.height as i32),
        icon,
        (icon_size, icon_size),
        (
            area.position.x,
            area.position.y,
            area.size.width as i32,
            area.size.height as i32,
        ),
        (8. * monitor.scale_factor()).round() as i32,
    );
    Some(Trip {
        origin,
        target,
        last: origin,
        desktop_item: Some((snack.link.clone(), icon)),
        start: Instant::now(),
        eating: false,
        returning: false,
        consumed: false,
        kind: snack.kind,
    })
}

fn finish(app: &AppHandle, main: &WebviewWindow, trip: &Trip, restore: bool) {
    if restore {
        let _ = main.set_position(trip.origin.position());
    }
    let state = app.state::<State>();
    state.walking.store(false, Ordering::Release);
    let mut tray = state.tray.lock().unwrap();
    if !trip.consumed && trip.desktop_item.is_some() {
        tray.hungry_at = Instant::now() + Duration::from_secs(15);
    }
    drop(tray);
    pet_effect(app, "stop", trip.kind);
    changed(app);
}

pub(crate) fn is_walking(app: &AppHandle) -> bool {
    app.try_state::<State>()
        .is_some_and(|state| state.walking.load(Ordering::Acquire))
}

fn forget_link(app: &AppHandle, name: &str) {
    let state = app.state::<State>();
    let mut tray = state.tray.lock().unwrap();
    let mut next = tray.snacks.clone();
    next.retain(|snack| snack.link != name);
    if next.len() == tray.snacks.len() {
        return;
    }
    let result = save_queue(&queue_path(), &next);
    tray.snacks = next;
    if tray.snacks.is_empty() {
        tray.request = false;
    }
    drop(tray);
    changed(app);
    if let Err(error) = result {
        notice(app, error);
    }
}

fn begin_hand_feed(main: &WebviewWindow, kind: Kind) -> Option<Trip> {
    let origin = Point::of(main.outer_position().ok()?);
    Some(Trip {
        origin,
        target: origin,
        last: origin,
        desktop_item: None,
        start: Instant::now() - Duration::from_millis(WALK_MS),
        eating: true,
        returning: false,
        consumed: false,
        kind,
    })
}

pub fn start(app: AppHandle) {
    if crate::win_needs_win7_ulw() {
        return;
    }
    if let Some(main) = app.get_webview_window("main") {
        let handle = app.clone();
        main.on_window_event(move |event| match event {
            WindowEvent::DragDrop(DragDropEvent::Enter { paths, .. }) if !paths.is_empty() => {
                let _ = handle.emit_to("main", "kuro-feeding-drop-hover", true);
            }
            WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) => {
                let _ = handle.emit_to("main", "kuro-feeding-drop-hover", false);
                drop_on_pet(handle.clone(), paths.clone());
            }
            WindowEvent::DragDrop(DragDropEvent::Leave) => {
                let _ = handle.emit_to("main", "kuro-feeding-drop-hover", false);
            }
            _ => {}
        });
    }
    std::thread::spawn(move || {
        let mut trip: Option<Trip> = None;
        let mut retry_after = Instant::now();
        loop {
            std::thread::sleep(Duration::from_millis(if trip.is_some() { 25 } else { 250 }));
            let state = app.state::<State>();
            let Some(main) = app.get_webview_window("main") else {
                continue;
            };

            if let Some(active) = trip.as_mut() {
                let dragging = app
                    .state::<crate::AppState>()
                    .dragging
                    .load(Ordering::Relaxed);
                let owns_position = main
                    .outer_position()
                    .ok()
                    .is_some_and(|p| Point::of(p).near(active.last));
                let can_continue = !dragging
                    && owns_position
                    && if active.desktop_item.is_some() {
                        eligible(&app, &main)
                    } else {
                        main.is_visible().unwrap_or(false)
                    };
                if !can_continue {
                    finish(&app, &main, active, owns_position && !dragging);
                    trip = None;
                    continue;
                }
                let elapsed = active.start.elapsed().as_millis() as u64;
                if elapsed >= WALK_MS && !active.eating {
                    if let Some((link, expected)) = &active.desktop_item {
                        if !matches!(desktop::position(link), Ok(desktop::Location::Found(position)) if position.near(*expected))
                        {
                            finish(&app, &main, active, true);
                            trip = None;
                            continue;
                        }
                    }
                    active.eating = true;
                    pet_effect(&app, "eat", active.kind);
                }
                if elapsed >= WALK_MS + EAT_MS / 2 && !active.consumed {
                    if let Some((name, _)) = &active.desktop_item {
                        if let Err(error) = desktop::eat(name) {
                            notice(&app, error);
                            finish(&app, &main, active, true);
                            trip = None;
                            continue;
                        }
                        forget_link(&app, name);
                    }
                    let mut tray = state.tray.lock().unwrap();
                    if active.desktop_item.is_none() {
                        tray.handfed.pop_front();
                    }
                    tray.hungry_at = Instant::now() + hunger(true);
                    active.consumed = true;
                    drop(tray);
                    changed(&app);
                }
                if active.desktop_item.is_none() {
                    if elapsed >= WALK_MS + EAT_MS {
                        finish(&app, &main, active, false);
                        trip = None;
                    }
                    continue;
                }
                if elapsed >= WALK_MS + EAT_MS && !active.returning {
                    active.returning = true;
                    pet_effect(&app, "walk", active.kind);
                }
                if elapsed >= 2 * WALK_MS + EAT_MS {
                    finish(&app, &main, active, true);
                    trip = None;
                    continue;
                }
                let next = if elapsed <= WALK_MS {
                    walk(active.origin, active.target, elapsed)
                } else if active.returning {
                    walk(active.target, active.origin, elapsed - WALK_MS - EAT_MS)
                } else {
                    active.target
                };
                if next != active.last && main.set_position(next.position()).is_ok() {
                    active.last = next;
                }
                continue;
            }
            let handfed = state.tray.lock().unwrap().handfed.front().copied();
            if let Some(kind) = handfed {
                let dragging = app
                    .state::<crate::AppState>()
                    .dragging
                    .load(Ordering::Relaxed);
                if !dragging && main.is_visible().unwrap_or(false) {
                    if let Some(next) = begin_hand_feed(&main, kind) {
                        state.walking.store(true, Ordering::Release);
                        pet_effect(&app, "eat", kind);
                        changed(&app);
                        trip = Some(next);
                    }
                }
                continue;
            }
            if Instant::now() < retry_after {
                continue;
            }
            let snack = {
                let tray = state.tray.lock().unwrap();
                if tray.error.is_some() || !tray.request && Instant::now() < tray.hungry_at {
                    continue;
                }
                tray.snacks.front().cloned()
            };
            let Some(snack) = snack else {
                continue;
            };
            if !eligible(&app, &main) {
                continue;
            }
            let icon = match desktop::position(&snack.link) {
                Ok(desktop::Location::Found(icon)) => icon,
                Ok(desktop::Location::Gone) => {
                    forget_link(&app, &snack.link);
                    continue;
                }
                Ok(desktop::Location::NotVisible) => {
                    notice(&app, "暂时看不到桌面图标。请确认桌面图标已显示。".into());
                    retry_after = Instant::now() + Duration::from_secs(3);
                    continue;
                }
                Err(error) => {
                    notice(&app, error);
                    retry_after = Instant::now() + Duration::from_secs(5);
                    continue;
                }
            };
            if let Some(next) = begin_trip(&main, icon, &snack) {
                state.walking.store(true, Ordering::Release);
                state.tray.lock().unwrap().request = false;
                pet_effect(&app, "walk", snack.kind);
                changed(&app);
                trip = Some(next);
            } else {
                notice(&app, "请把桌宠和桌面零食放在同一块屏幕。".into());
                retry_after = Instant::now() + Duration::from_secs(4);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_stops_beside_tray_and_returns_exactly() {
        let pet = Point { x: -350, y: 710 };
        let target = approach(
            pet,
            (220, 230),
            Point { x: -850, y: 630 },
            (310, 330),
            (-1920, 0, 1920, 1040),
            10,
        );
        assert_eq!(target.x, -530);
        assert_eq!(walk(pet, target, 0), pet);
        assert_ne!(walk(pet, target, 500), target);
        assert_eq!(walk(pet, target, WALK_MS), target);
        assert_eq!(walk(target, pet, WALK_MS), pet);
    }

    #[test]
    fn snack_removal_only_updates_queue() {
        let base =
            std::env::temp_dir().join(format!("kurocat-feeding-test-{}", std::process::id()));
        fs::create_dir_all(&base).unwrap();
        let original = base.join("snack.PNG");
        fs::write(&original, b"original sample").unwrap();
        let path = base.join("feeding.json");
        let mut queue = VecDeque::from([Snack {
            name: "snack.PNG".into(),
            kind: Kind::of(&original),
            icon: vec![],
            link: "test.lnk".into(),
        }]);
        assert_eq!(queue[0].kind, Kind::Picture);
        save_queue(&path, &queue).unwrap();
        queue.pop_front();
        save_queue(&path, &queue).unwrap();
        assert_eq!(fs::read(&original).unwrap(), b"original sample");
        assert!(
            serde_json::from_slice::<VecDeque<Snack>>(&fs::read(&path).unwrap())
                .unwrap()
                .is_empty()
        );
        fs::remove_file(original).unwrap();
        fs::remove_file(path).unwrap();
        fs::remove_dir(base).unwrap();
    }
}
