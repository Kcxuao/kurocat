//! Windows hover launcher. Geometry and timers stay independent of the WebView.
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::Instant,
};
use tauri::{Emitter, Manager};
mod native;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Shortcut {
    pub id: String,
    pub name: String,
    pub path: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Config {
    version: u8,
    items: Vec<Shortcut>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    items: Vec<Shortcut>,
    icons: HashMap<String, Vec<u8>>,
    error: Option<String>,
}
pub struct Store {
    config: Config,
    error: Option<String>,
    icons: HashMap<String, (std::time::SystemTime, Vec<u8>)>,
}
pub struct State {
    store: Mutex<Store>,
    runtime: Mutex<Runtime>,
    pub dialog: AtomicBool,
    busy: AtomicBool,
}
impl Default for State {
    fn default() -> Self {
        let loaded = std::fs::read(path())
            .map_err(|e| e.to_string())
            .and_then(|s| serde_json::from_slice::<Config>(&s).map_err(|e| e.to_string()));
        let (config, error) = match loaded {
            Ok(c) if c.version == 1 && validate(&c.items, &c.items).is_ok() => (c, None),
            _ if !path().exists() => (
                Config {
                    version: 1,
                    items: vec![],
                },
                None,
            ),
            _ => (
                Config {
                    version: 1,
                    items: vec![],
                },
                Some("快捷配置无法读取，请修复 shortcuts.json 后重启；原文件未覆盖".into()),
            ),
        };
        Self {
            store: Mutex::new(Store {
                config,
                error,
                icons: HashMap::new(),
            }),
            runtime: Mutex::new(Runtime::default()),
            dialog: AtomicBool::new(false),
            busy: AtomicBool::new(false),
        }
    }
}
fn path() -> PathBuf {
    crate::config::app_data_dir().join("shortcuts.json")
}
fn validate(items: &[Shortcut], old: &[Shortcut]) -> Result<(), String> {
    if items.len() > 8 {
        return Err("最多添加 8 个快捷入口".into());
    }
    let mut ids = HashSet::new();
    for i in items {
        if i.id.is_empty() || i.id.len() > 80 || !ids.insert(&i.id) {
            return Err("快捷入口 ID 无效或重复".into());
        }
        if i.name.trim().is_empty() || i.name.chars().count() > 40 {
            return Err("名称需要 1～40 个字符".into());
        }
        let p = Path::new(&i.path);
        if !p.is_absolute() || i.path.contains('\0') {
            return Err("请选择绝对路径".into());
        }
        let unchanged = old.iter().any(|o| o.id == i.id && o.path == i.path);
        if !unchanged && !supported(p) {
            return Err("请选择有效文件夹、.exe 或 .lnk 文件".into());
        }
    }
    Ok(())
}
fn supported(p: &Path) -> bool {
    p.is_dir()
        || (p.is_file()
            && p.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("exe") || e.eq_ignore_ascii_case("lnk")))
}
fn write_config(p: &Path, c: &Config) -> Result<(), String> {
    std::fs::create_dir_all(p.parent().ok_or("配置目录不存在")?).map_err(|e| e.to_string())?;
    let tmp = p.with_extension(format!("{}.tmp", std::process::id()));
    let result = std::fs::write(
        &tmp,
        serde_json::to_vec_pretty(c).map_err(|e| e.to_string())?,
    )
    .and_then(|_| std::fs::rename(&tmp, p));
    if result.is_err() {
        let _ = std::fs::remove_file(tmp);
    }
    result.map_err(|e| format!("保存失败：{e}"))
}
fn authorize(window: &tauri::WebviewWindow, edit: bool) -> Result<(), String> {
    if crate::win_needs_win7_ulw() {
        return Err("快捷菜单仅支持 Windows 10/11".into());
    }
    if window.label() == "menu" || (!edit && window.label() == "shortcuts") {
        Ok(())
    } else {
        Err("此窗口无权操作快捷入口".into())
    }
}
#[tauri::command]
pub async fn get_shortcuts(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
) -> Result<View, String> {
    authorize(&window, false)?;
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<State>();
        let (items, error) = {
            let s = state.store.lock().unwrap();
            (s.config.items.clone(), s.error.clone())
        };
        let mut icons = HashMap::new();
        for i in &items {
            let stamp = std::fs::metadata(&i.path)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            let cached = state
                .store
                .lock()
                .unwrap()
                .icons
                .get(&i.path)
                .filter(|(t, _)| *t == stamp)
                .map(|(_, v)| v.clone());
            let rgba = cached.unwrap_or_else(|| {
                let rgba = native::icon(&i.path).unwrap_or_default();
                state
                    .store
                    .lock()
                    .unwrap()
                    .icons
                    .insert(i.path.clone(), (stamp, rgba.clone()));
                rgba
            });
            if !rgba.is_empty() {
                icons.insert(i.id.clone(), rgba);
            }
        }
        View {
            items,
            icons,
            error,
        }
    })
    .await
    .map_err(|e| e.to_string())
}
#[tauri::command]
pub fn save_shortcuts(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    mut items: Vec<Shortcut>,
) -> Result<(), String> {
    authorize(&window, true)?;
    let state = app.state::<State>();
    let mut s = state.store.lock().unwrap();
    if let Some(e) = &s.error {
        return Err(e.clone());
    }
    for i in &mut items {
        i.name = i.name.trim().into();
    }
    validate(&items, &s.config.items)?;
    let next = Config { version: 1, items };
    write_config(&path(), &next)?;
    s.config = next;
    s.icons.retain(|p, _| Path::new(p).exists());
    drop(s);
    close(&app);
    let _ = app.emit("kuro-shortcuts-changed", ());
    Ok(())
}
#[tauri::command]
pub async fn pick_shortcut_target(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    folder: bool,
) -> Result<Option<String>, String> {
    authorize(&window, true)?;
    if app.state::<State>().dialog.swap(true, Ordering::SeqCst) {
        return Err("文件选择器已打开".into());
    }
    close(&app);
    let owner = window
        .hwnd()
        .map(|h| h.0 as isize)
        .map_err(|e| e.to_string());
    let result = match owner {
        Ok(hwnd) => tauri::async_runtime::spawn_blocking(move || native::pick(hwnd, folder))
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r),
        Err(e) => Err(e),
    };
    app.state::<State>().dialog.store(false, Ordering::SeqCst);
    let _ = window.set_focus();
    result
}
#[tauri::command]
pub async fn launch_shortcut(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    id: String,
) -> Result<(), String> {
    authorize(&window, false)?;
    let state = app.state::<State>();
    if state.busy.swap(true, Ordering::SeqCst) {
        return Err("正在打开，请稍候".into());
    }
    let item = state
        .store
        .lock()
        .unwrap()
        .config
        .items
        .iter()
        .find(|i| i.id == id)
        .cloned();
    close(&app);
    let result = match item {
        Some(item) => tauri::async_runtime::spawn_blocking(move || native::launch(&item.path))
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r),
        None => Err("快捷入口不存在".into()),
    };
    state.busy.store(false, Ordering::SeqCst);
    if let Err(e) = &result {
        let _ = crate::open_context_menu(app.clone()).await;
        let _ = app.emit_to(
            "menu",
            "kuro-shortcut-error",
            serde_json::json!({"id":id,"message":e}),
        );
    }
    result
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq)]
pub struct Point {
    x: f64,
    y: f64,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}
impl Rect {
    fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.y >= self.y && p.x <= self.x + self.w && p.y <= self.y + self.h
    }
}
#[derive(Clone, Serialize)]
struct Layout {
    buttons: Vec<Point>,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    sf: f64,
    anchor: Point,
}
fn distance(a: Point, b: Point) -> f64 {
    (a.x - b.x).hypot(a.y - b.y)
}
fn segment_distance(p: Point, a: Point, b: Point) -> f64 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / (dx * dx + dy * dy).max(0.001)).clamp(0., 1.);
    distance(
        p,
        Point {
            x: a.x + t * dx,
            y: a.y + t * dy,
        },
    )
}
fn layout(pet: Rect, work: Rect, sf: f64, n: usize) -> Layout {
    let anchor = Point {
        x: pet.x + pet.w * 0.5,
        y: pet.y + pet.h * 0.5,
    };
    let r = 24. * sf;
    let pad = 8. * sf;
    let left = anchor.x - work.x;
    let right = work.x + work.w - anchor.x;
    let top = anchor.y - work.y;
    let bottom = work.y + work.h - anchor.y;
    let nearest = [left, right, top, bottom]
        .into_iter()
        .enumerate()
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap()
        .0;
    let inward = [
        0.,
        std::f64::consts::PI,
        std::f64::consts::FRAC_PI_2,
        -std::f64::consts::FRAC_PI_2,
    ][nearest];
    let quadrant = (
        if left < right { 1.0_f64 } else { -1.0_f64 },
        if top < bottom { 1.0_f64 } else { -1.0_f64 },
    );
    let mut selected = vec![];
    for (span, mid) in [
        (std::f64::consts::TAU, -std::f64::consts::FRAC_PI_2),
        (std::f64::consts::PI, inward),
        (std::f64::consts::FRAC_PI_2, quadrant.1.atan2(quadrant.0)),
    ] {
        let step = if span == std::f64::consts::TAU {
            span / n.max(1) as f64
        } else {
            span / (n.saturating_sub(1)).max(1) as f64
        };
        let spacing = if n > 1 {
            60. * sf / (2. * (step / 2.).sin())
        } else {
            0.
        };
        let radius = (pet.w * 0.5).hypot(pet.h * 0.5).max(spacing) + r + 12. * sf;
        selected = (0..n)
            .map(|i| {
                let a = if span == std::f64::consts::TAU {
                    mid + i as f64 * step
                } else if n == 1 {
                    mid
                } else {
                    mid - span / 2. + i as f64 * step
                };
                Point {
                    x: anchor.x + radius * a.cos(),
                    y: anchor.y + radius * a.sin(),
                }
            })
            .collect();
        if selected.iter().all(|p| {
            p.x - r >= work.x + pad
                && p.y - r >= work.y + pad
                && p.x + r <= work.x + work.w - pad
                && p.y + r <= work.y + work.h - pad
        }) {
            break;
        }
    }
    // Very small work areas use a compact grid only when no arc can fit after translation.
    let bounds = |v: &Vec<Point>| {
        let x = v
            .iter()
            .map(|p| p.x - r - pad)
            .fold(f64::INFINITY, f64::min);
        let y = v
            .iter()
            .map(|p| p.y - r - pad)
            .fold(f64::INFINITY, f64::min);
        let right = v
            .iter()
            .map(|p| p.x + r + pad)
            .fold(f64::NEG_INFINITY, f64::max);
        let bottom = v
            .iter()
            .map(|p| p.y + r + pad)
            .fold(f64::NEG_INFINITY, f64::max);
        (x, y, right - x, bottom - y)
    };
    let (mut x, mut y, mut w, mut h) = bounds(&selected);
    if w > work.w || h > work.h {
        let cols = ((work.w - 2. * pad) / (60. * sf)).floor().max(1.) as usize;
        selected = (0..n)
            .map(|i| Point {
                x: work.x + pad + r + (i % cols) as f64 * 60. * sf,
                y: work.y + pad + r + (i / cols) as f64 * 60. * sf,
            })
            .collect();
        (x, y, w, h) = bounds(&selected);
    }
    let nx = x.clamp(work.x, (work.x + work.w - w).max(work.x));
    let ny = y.clamp(work.y, (work.y + work.h - h).max(work.y));
    for p in &mut selected {
        p.x += nx - x;
        p.y += ny - y;
    }
    Layout {
        buttons: selected,
        x: nx,
        y: ny,
        w,
        h,
        sf,
        anchor,
    }
}
#[derive(Default)]
struct Timer {
    entered: Option<u64>,
    left: Option<u64>,
    open: bool,
    armed: bool,
}
impl Timer {
    fn tick(&mut self, now: u64, eligible: bool, over: bool, keep: bool) -> bool {
        if !eligible {
            self.entered = None;
            self.left = None;
            self.open = false;
            self.armed = false;
            return false;
        }
        if !over && !self.open {
            self.armed = true;
        }
        if !self.open {
            if over && self.armed {
                let t = *self.entered.get_or_insert(now);
                if now - t >= 400 {
                    self.open = true;
                    self.left = None;
                }
            } else {
                self.entered = None;
            }
        } else if keep {
            self.left = None;
        } else {
            let t = *self.left.get_or_insert(now);
            if now - t >= 300 {
                self.open = false;
                self.entered = None;
                self.armed = false;
            }
        }
        self.open
    }
}
struct Runtime {
    timer: Timer,
    layout: Option<Layout>,
    epoch: Instant,
    key: Option<(Rect, Rect, f64, usize)>,
    queued: bool,
}
impl Default for Runtime {
    fn default() -> Self {
        Self {
            timer: Timer {
                armed: true,
                ..Default::default()
            },
            layout: None,
            epoch: Instant::now(),
            key: None,
            queued: false,
        }
    }
}
pub fn close(app: &tauri::AppHandle) {
    if let Some(s) = app.try_state::<State>() {
        let mut rt = s.runtime.lock().unwrap();
        rt.timer = Timer::default();
        rt.layout = None;
        rt.key = None;
    }
    if let Some(w) = app.get_webview_window("shortcuts") {
        let _ = w.hide();
    }
}
pub fn tick(
    app: &tauri::AppHandle,
    cursor: Option<(i32, i32)>,
    pet: Option<Rect>,
    over: bool,
    eligible: bool,
) {
    let state = app.state::<State>();
    let count = state.store.lock().unwrap().config.items.len();
    let Some(main) = app.get_webview_window("main") else {
        return;
    };
    let eligible = eligible
        && count > 0
        && !state.dialog.load(Ordering::Relaxed)
        && !state.busy.load(Ordering::Relaxed)
        && !crate::win_needs_win7_ulw();
    let (Some((cx, cy)), Some(pet)) = (cursor, pet) else {
        close(app);
        return;
    };
    let Some(mon) = main.current_monitor().ok().flatten() else {
        close(app);
        return;
    };
    let area = mon.work_area();
    let work = Rect {
        x: area.position.x as f64,
        y: area.position.y as f64,
        w: area.size.width as f64,
        h: area.size.height as f64,
    };
    let sf = mon.scale_factor();
    let key = (pet, work, sf, count);
    let mut rt = state.runtime.lock().unwrap();
    let pos = Point {
        x: cx as f64,
        y: cy as f64,
    };
    if rt.key != Some(key) && count > 0 {
        rt.layout = Some(layout(pet, work, sf, count));
        rt.key = Some(key);
    }
    let keep = over
        || pet.contains(pos)
        || rt.layout.as_ref().is_some_and(|l| {
            l.buttons
                .iter()
                .any(|b| segment_distance(pos, l.anchor, *b) <= 36. * sf)
        });
    let was = rt.timer.open;
    let now = rt.epoch.elapsed().as_millis() as u64;
    let open = rt.timer.tick(now, eligible, over, keep);
    if !open {
        if was {
            if let Some(w) = app.get_webview_window("shortcuts") {
                let _ = w.hide();
            }
        }
        return;
    }
    let Some(l) = rt.layout.clone() else {
        return;
    };
    if let Some(w) = app.get_webview_window("shortcuts") {
        let _ =
            w.set_ignore_cursor_events(!l.buttons.iter().any(|p| distance(pos, *p) <= 24. * sf));
        if !was
            || w.outer_position()
                .is_ok_and(|p| p.x != l.x.round() as i32 || p.y != l.y.round() as i32)
            || w.outer_size()
                .is_ok_and(|s| s.width != l.w.ceil() as u32 || s.height != l.h.ceil() as u32)
        {
            let _ = w.set_position(tauri::PhysicalPosition::new(
                l.x.round() as i32,
                l.y.round() as i32,
            ));
            let _ = w.set_size(tauri::PhysicalSize::new(
                l.w.ceil() as u32,
                l.h.ceil() as u32,
            ));
            let _ = app.emit_to("shortcuts", "kuro-shortcuts-layout", &l);
        }
        if !was {
            let _ = w.show();
        }
    } else if !rt.queued {
        rt.queued = true;
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || {
            let result = tauri::WebviewWindowBuilder::new(
                &handle,
                "shortcuts",
                tauri::WebviewUrl::App("shortcuts.html".into()),
            )
            .title("KuroCat Shortcuts")
            .transparent(true)
            .decorations(false)
            .shadow(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(false)
            .focused(false)
            .focusable(false)
            .visible(false)
            .additional_browser_args("--autoplay-policy=no-user-gesture-required")
            .build();
            let s = handle.state::<State>();
            let mut rt = s.runtime.lock().unwrap();
            rt.queued = false;
            if let Ok(w) = result {
                let _ = w.set_ignore_cursor_events(true);
                let _ = w.set_position(tauri::PhysicalPosition::new(l.x as i32, l.y as i32));
                let _ = w.set_size(tauri::PhysicalSize::new(
                    l.w.ceil() as u32,
                    l.h.ceil() as u32,
                ));
                rt.timer.open = false;
                rt.timer.entered = None;
            } else {
                rt.timer.open = false;
                rt.timer.entered = None;
            }
        });
    }
}
#[tauri::command]
pub fn shortcut_layout(window: tauri::WebviewWindow, app: tauri::AppHandle) -> Result<(), String> {
    authorize(&window, false)?;
    if let Some(l) = &app.state::<State>().runtime.lock().unwrap().layout {
        let _ = app.emit_to(window.label(), "kuro-shortcuts-layout", l);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn item(id: &str) -> Shortcut {
        Shortcut {
            id: id.into(),
            name: "测试".into(),
            path: "D:\\missing.exe".into(),
        }
    }
    #[test]
    fn shortcuts_validation() {
        let i = item("a");
        assert!(validate(&[i.clone()], &[i.clone()]).is_ok());
        assert!(validate(&vec![i.clone(); 9], &[i.clone()]).is_err());
        assert!(validate(&[i.clone(), i.clone()], &[i.clone()]).is_err());
        let mut bad = i.clone();
        bad.name = " ".into();
        assert!(validate(&[bad], &[i]).is_err());
    }
    #[test]
    fn shortcuts_timer() {
        let mut t = Timer {
            armed: true,
            ..Default::default()
        };
        assert!(!t.tick(0, true, true, true));
        assert!(!t.tick(399, true, true, true));
        assert!(t.tick(400, true, true, true));
        assert!(t.tick(500, true, false, false));
        assert!(t.tick(750, true, false, true));
        assert!(t.tick(900, true, false, false));
        assert!(!t.tick(1200, true, false, false));
        assert!(!t.tick(1800, true, true, true));
        t.tick(1801, true, false, false);
        t.tick(1802, true, true, true);
        assert!(t.tick(2202, true, true, true));
        assert!(!t.tick(2203, false, true, true));
    }
    #[test]
    fn shortcuts_geometry() {
        for sf in [1., 1.5] {
            for n in 1..=8 {
                for (x, y) in [
                    (-1920., 0.),
                    (-100., 0.),
                    (-1920., 700.),
                    (-100., 700.),
                    (-900., 400.),
                ] {
                    let work = Rect {
                        x: -1920.,
                        y: 0.,
                        w: 1920.,
                        h: 1080.,
                    };
                    let pet = Rect {
                        x,
                        y,
                        w: 250. * sf,
                        h: 250. * sf,
                    };
                    let l = layout(pet, work, sf, n);
                    for (i, a) in l.buttons.iter().enumerate() {
                        assert!(a.x - 24. * sf >= work.x - 0.01 && a.x + 24. * sf <= 0.01);
                        assert!(a.y - 24. * sf >= -0.01 && a.y + 24. * sf <= 1080.01);
                        for b in l.buttons.iter().skip(i + 1) {
                            assert!(distance(*a, *b) >= 60. * sf - 0.01);
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn shortcuts_corridor() {
        assert!(
            segment_distance(
                Point { x: 50., y: 10. },
                Point { x: 0., y: 0. },
                Point { x: 100., y: 0. }
            ) < 36.
        );
        assert!(
            segment_distance(
                Point { x: 50., y: 50. },
                Point { x: 0., y: 0. },
                Point { x: 100., y: 0. }
            ) > 36.
        );
    }
    #[test]
    fn shortcuts_persistence() {
        let dir = std::env::temp_dir().join(format!("kuro-shortcuts-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("shortcuts.json");
        let c = Config {
            version: 1,
            items: vec![item("b"), item("a")],
        };
        write_config(&p, &c).unwrap();
        let read: Config = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        assert_eq!(read.items, c.items);
        assert!(write_config(&dir, &c).is_err());
        std::fs::remove_file(p).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}
