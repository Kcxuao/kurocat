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
mod hotkeys;
pub(crate) mod native;
pub(crate) use hotkeys::Registry as HotkeyRegistry;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Shortcut {
    pub id: String,
    pub name: String,
    pub path: String,
    /// Normalized 32x32 RGBA, stored inline so source files can be moved safely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<Vec<u8>>,
    // Missing follows the slot default. An empty string disables this item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hotkey: Option<String>,
}
#[derive(Clone, Copy, Default, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effect {
    #[default]
    Energy,
    Magic,
    Comic,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Config {
    version: u8,
    #[serde(default)]
    effect: Effect,
    items: Vec<Shortcut>,
    #[serde(default = "default_hotkeys_enabled")]
    hotkeys_enabled: bool,
}
fn default_hotkeys_enabled() -> bool {
    true
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    effect: Effect,
    items: Vec<Shortcut>,
    icons: HashMap<String, Vec<u8>>,
    hotkeys_enabled: bool,
    hotkey_status: HashMap<String, hotkeys::Status>,
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
    preview_until: Mutex<Option<Instant>>,
    showing: AtomicBool,
    hotkey_status: Mutex<HashMap<String, hotkeys::Status>>,
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
                    effect: Effect::default(),
                    items: vec![],
                    hotkeys_enabled: true,
                },
                None,
            ),
            _ => (
                Config {
                    version: 1,
                    effect: Effect::default(),
                    items: vec![],
                    hotkeys_enabled: true,
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
            preview_until: Mutex::new(None),
            showing: AtomicBool::new(false),
            hotkey_status: Mutex::new(HashMap::new()),
        }
    }
}
fn path() -> PathBuf {
    crate::config::app_data_dir().join("shortcuts.json")
}
fn validate(items: &[Shortcut], old: &[Shortcut]) -> Result<(), String> {
    if items.len() > MAX_SHORTCUTS {
        return Err("最多添加 24 个快捷入口".into());
    }
    let mut ids = HashSet::new();
    for i in items {
        if i.id.is_empty() || i.id.len() > 80 || !ids.insert(&i.id) {
            return Err("快捷入口 ID 无效或重复".into());
        }
        if i.name.trim().is_empty() || i.name.chars().count() > 40 {
            return Err("名称需要 1～40 个字符".into());
        }
        if i.icon
            .as_ref()
            .is_some_and(|bytes| bytes.len() != 32 * 32 * 4)
        {
            return Err("自定义图标数据无效，请重新选择图片".into());
        }
        if i.hotkey
            .as_deref()
            .is_some_and(|s| !s.is_empty() && hotkeys::Chord::parse(s).is_none())
        {
            return Err("快捷键需要 Ctrl 或 Alt，并搭配字母、数字或 F1–F11。".into());
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
        let (items, error, effect, hotkeys_enabled) = {
            let s = state.store.lock().unwrap();
            (
                s.config.items.clone(),
                s.error.clone(),
                s.config.effect,
                s.config.hotkeys_enabled,
            )
        };
        let mut icons = HashMap::new();
        for i in &items {
            if let Some(rgba) = &i.icon {
                icons.insert(i.id.clone(), rgba.clone());
                continue;
            }
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
        let hotkey_status = state.hotkey_status.lock().unwrap().clone();
        View {
            effect,
            items,
            icons,
            hotkeys_enabled,
            hotkey_status,
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
    hotkeys_enabled: Option<bool>,
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
    let next = Config {
        version: 1,
        items,
        effect: s.config.effect,
        hotkeys_enabled: hotkeys_enabled.unwrap_or(s.config.hotkeys_enabled),
    };
    write_config(&path(), &next)?;
    s.config = next;
    s.icons.retain(|p, _| Path::new(p).exists());
    drop(s);
    crate::input_hook::refresh_shortcut_hotkeys();
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
    pick_target(
        window,
        app,
        if folder {
            native::Picker::Folder
        } else {
            native::Picker::Target
        },
    )
    .await
}

#[tauri::command]
pub async fn pick_shortcut_icon(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
) -> Result<Option<Vec<u8>>, String> {
    let path = pick_target(window, app, native::Picker::Icon).await?;
    match path {
        Some(path) => {
            tauri::async_runtime::spawn_blocking(move || native::custom_icon(Path::new(&path)))
                .await
                .map_err(|e| e.to_string())?
                .map(Some)
        }
        None => Ok(None),
    }
}

async fn pick_target(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    picker: native::Picker,
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
        Ok(hwnd) => tauri::async_runtime::spawn_blocking(move || native::pick(hwnd, picker))
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
    launch_by_id(app, id).await
}

async fn launch_by_id(app: tauri::AppHandle, id: String) -> Result<(), String> {
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
    rotates: bool,
    first: usize,
    page: usize,
    pages: usize,
    pager: Option<Point>,
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
const SLOTS_PER_RING: usize = 8;
const MAX_SHORTCUTS: usize = 24;
const RING_PERIODS: [f64; 3] = [48_000., -72_000., 96_000.];

// A partial ring is always innermost. Ranking and default hotkeys still use the original list order.
fn inner_ring_padding(count: usize) -> usize {
    count.next_multiple_of(SLOTS_PER_RING) - count
}

impl Layout {
    fn positions(&self, motion_ms: u64) -> Vec<Point> {
        if !self.rotates || motion_ms == 0 {
            return self.buttons.clone();
        }
        self.buttons
            .iter()
            .enumerate()
            .map(|(index, point)| {
                let ring = (inner_ring_padding(self.buttons.len()) + index) / SLOTS_PER_RING;
                let period = RING_PERIODS[ring];
                let angle = std::f64::consts::TAU * (motion_ms as f64 % period.abs()) / period;
                let (sin, cos) = angle.sin_cos();
                let x = point.x - self.anchor.x;
                let y = point.y - self.anchor.y;
                Point {
                    x: self.anchor.x + x * cos - y * sin,
                    y: self.anchor.y + x * sin + y * cos,
                }
            })
            .collect()
    }
}

fn layout(pet: Rect, work: Rect, sf: f64, count: usize, requested_page: usize) -> Layout {
    use std::f64::consts::{FRAC_PI_2, PI, TAU};
    let anchor = Point {
        x: pet.x + pet.w * 0.5,
        y: pet.y + pet.h * 0.5,
    };
    let r = 24. * sf;
    let pad = 8. * sf;
    let gap = 60. * sf;
    let inset = r + pad;
    let clear = (pet.w * 0.5).hypot(pet.h * 0.5) + r + 12. * sf;
    let fits = |p: Point| {
        p.x >= work.x + inset
            && p.y >= work.y + inset
            && p.x <= work.x + work.w - inset
            && p.y <= work.y + work.h - inset
    };
    let sides = [
        anchor.x - work.x,
        work.x + work.w - anchor.x,
        anchor.y - work.y,
        work.y + work.h - anchor.y,
    ];
    let near = (0..4)
        .min_by(|i, j| sides[*i].total_cmp(&sides[*j]))
        .unwrap();
    let inward = [0., PI, FRAC_PI_2, -FRAC_PI_2][near];
    let dx: f64 = if sides[0] < sides[1] { 1. } else { -1. };
    let dy: f64 = if sides[2] < sides[3] { 1. } else { -1. };
    let corner = dy.atan2(dx);
    let rings = count.div_ceil(SLOTS_PER_RING);
    let mut buttons = Vec::new();
    let mut rotates = false;
    let mut outer = 0.;
    for (span, center) in [(TAU, -FRAC_PI_2), (PI, inward), (FRAC_PI_2 * 0.84, corner)] {
        buttons.clear();
        let full = span == TAU;
        let mut radius = clear - gap;
        for ring in 0..rings {
            let slots = if ring == 0 {
                SLOTS_PER_RING - inner_ring_padding(count)
            } else {
                SLOTS_PER_RING
            };
            let step = if full {
                TAU / slots as f64
            } else {
                span / slots.saturating_sub(1).max(1) as f64
            };
            let spacing = if slots > 1 {
                gap / (2. * (step / 2.).sin())
            } else {
                clear
            };
            radius = (radius + gap).max(spacing).max(clear);
            for i in 0..slots {
                let angle = if full {
                    center
                        + i as f64 * step
                        + if ring % 2 == 1 {
                            PI / SLOTS_PER_RING as f64
                        } else {
                            0.
                        }
                } else if slots == 1 {
                    center
                } else {
                    center - span / 2. + i as f64 * step
                };
                buttons.push(Point {
                    x: anchor.x + radius * angle.cos(),
                    y: anchor.y + radius * angle.sin(),
                });
            }
        }
        let fits_orbit = radius + inset <= sides.into_iter().fold(f64::INFINITY, f64::min);
        if (!full || fits_orbit) && buttons.iter().copied().all(fits) {
            rotates = full;
            outer = radius;
            break;
        }
        buttons.clear();
    }
    let mut first = 0;
    let mut page = 0;
    let mut pages = 1;
    let mut pager = None;
    if buttons.is_empty() {
        // When the concentric arcs won't fit, use a compact paged panel.
        let columns = (((work.w - 2. * inset) / gap).floor() as usize + 1).clamp(1, 4);
        let rows = (((work.h - 2. * inset) / gap).floor() as usize + 1).clamp(1, 3);
        let slots = columns * rows;
        let per_page = if count > slots {
            slots.saturating_sub(1).max(1).min(8)
        } else {
            slots.min(8)
        };
        pages = count.div_ceil(per_page);
        page = requested_page.min(pages - 1);
        first = page * per_page;
        let length = (count - first).min(per_page);
        let rows_used = (length + usize::from(pages > 1)).div_ceil(columns);
        let w = (columns - 1) as f64 * gap;
        let h = (rows_used - 1) as f64 * gap;
        let min_x = work.x + inset;
        let min_y = work.y + inset;
        let max_x = (work.x + work.w - inset - w).max(min_x);
        let max_y = (work.y + work.h - inset - h).max(min_y);
        let x = (if dx > 0. {
            anchor.x + clear * 0.70
        } else {
            anchor.x - clear * 0.70 - w
        })
        .clamp(min_x, max_x);
        let y = (anchor.y - h / 2.).clamp(min_y, max_y);
        let cell = |i| Point {
            x: x + (i % columns) as f64 * gap,
            y: y + (i / columns) as f64 * gap,
        };
        buttons.extend((0..length).map(cell));
        if pages > 1 {
            pager = Some(cell(length));
        }
    }
    let margin = 64. * sf;
    let mut left = anchor.x - 90. * sf;
    let mut right = anchor.x + 90. * sf;
    let mut top = anchor.y - 90. * sf;
    let mut bottom = anchor.y + 90. * sf;
    if rotates {
        let extent = outer + inset;
        left = left.min(anchor.x - extent);
        right = right.max(anchor.x + extent);
        top = top.min(anchor.y - extent);
        bottom = bottom.max(anchor.y + extent);
    }
    for point in buttons.iter().copied().chain(pager) {
        left = left.min(point.x - r - margin);
        right = right.max(point.x + r + margin);
        top = top.min(point.y - r - margin);
        bottom = bottom.max(point.y + r + margin);
    }
    let x = left.max(work.x);
    let y = top.max(work.y);
    Layout {
        buttons,
        x,
        y,
        w: (right.min(work.x + work.w) - x).max(1.),
        h: (bottom.min(work.y + work.h) - y).max(1.),
        sf,
        anchor,
        rotates,
        first,
        page,
        pages,
        pager,
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
    hide_at: Option<Instant>,
    page: usize,
    orbit_ready: bool,
    orbit_ms: u64,
    last_orbit_tick: u64,
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
            hide_at: None,
            page: 0,
            orbit_ready: false,
            orbit_ms: 0,
            last_orbit_tick: 0,
        }
    }
}
pub fn is_showing(app: &tauri::AppHandle) -> bool {
    app.try_state::<State>()
        .is_some_and(|state| state.showing.load(Ordering::Relaxed))
}
pub fn close(app: &tauri::AppHandle) {
    if let Some(s) = app.try_state::<State>() {
        s.showing.store(false, Ordering::Relaxed);
        let mut rt = s.runtime.lock().unwrap();
        rt.timer = Timer::default();
        rt.layout = None;
        rt.key = None;
        rt.hide_at = None;
        rt.page = 0;
        rt.orbit_ready = false;
        rt.orbit_ms = 0;
        *s.preview_until.lock().unwrap() = None;
    }
    let _ = app.emit_to("shortcuts", "kuro-shortcuts-visibility", false);
    if let Some(w) = app.get_webview_window("shortcuts") {
        let _ = w.set_ignore_cursor_events(true);
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
    let preview = state
        .preview_until
        .lock()
        .unwrap()
        .is_some_and(|deadline| Instant::now() < deadline);
    let over_pet = over;
    let over = over || preview;
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
    let now = rt.epoch.elapsed().as_millis() as u64;
    let was = rt.timer.open;
    if !was && rt.page != 0 {
        rt.page = 0;
        rt.key = None;
    }
    let layout_changed = rt.key != Some(key);
    if layout_changed && count > 0 {
        let next = layout(pet, work, sf, count, rt.page);
        rt.page = next.page;
        rt.layout = Some(next);
        rt.orbit_ready = false;
        rt.orbit_ms = 0;
        rt.key = Some(key);
    }
    let pager = rt.layout.as_ref().and_then(|l| l.pager);
    let positions = rt
        .layout
        .as_ref()
        .map(|l| l.positions(if was { rt.orbit_ms } else { 0 }))
        .unwrap_or_default();
    let pointer_in_menu = pet.contains(pos)
        || rt.layout.as_ref().is_some_and(|l| {
            positions
                .iter()
                .copied()
                .chain(pager)
                .any(|b| segment_distance(pos, l.anchor, b) <= 36. * sf)
        });
    if preview && !rt.timer.open {
        rt.timer.armed = true;
    }
    let open = rt.timer.tick(now, eligible, over, over || pointer_in_menu);
    if !open {
        rt.orbit_ready = false;
        rt.orbit_ms = 0;
        rt.last_orbit_tick = now;
        if was {
            let _ = app.emit_to("shortcuts", "kuro-shortcuts-visibility", false);
            if let Some(w) = app.get_webview_window("shortcuts") {
                let _ = w.set_ignore_cursor_events(true);
                if eligible {
                    rt.hide_at = Some(
                        Instant::now() + std::time::Duration::from_millis(510 + count as u64 * 26),
                    );
                } else {
                    let _ = w.hide();
                    state.showing.store(false, Ordering::Relaxed);
                    rt.hide_at = None;
                }
            }
        }
        if rt
            .hide_at
            .is_some_and(|deadline| !eligible || Instant::now() >= deadline)
        {
            if let Some(w) = app.get_webview_window("shortcuts") {
                let _ = w.hide();
            }
            rt.hide_at = None;
            state.showing.store(false, Ordering::Relaxed);
        }
        return;
    }
    rt.hide_at = None;
    let Some(l) = rt.layout.clone() else {
        return;
    };
    let delta = now.saturating_sub(rt.last_orbit_tick).min(55);
    rt.last_orbit_tick = now;
    if !was {
        rt.orbit_ready = false;
        rt.orbit_ms = 0;
    }
    let movement_allowed = over_pet || (preview && !pointer_in_menu);
    let moving = was && movement_allowed && rt.orbit_ready && l.rotates && delta > 0;
    if moving {
        rt.orbit_ms += delta;
    }
    let positions = if moving {
        l.positions(rt.orbit_ms)
    } else {
        positions
    };
    if let Some(w) = app.get_webview_window("shortcuts") {
        // The same positions feed the WebView and the native mouse hit testing.
        let hit = positions
            .iter()
            .copied()
            .chain(pager)
            .any(|p| distance(pos, p) <= 24. * sf);
        let _ = w.set_ignore_cursor_events(!hit);
        if moving {
            let _ = app.emit_to("shortcuts", "kuro-shortcuts-orbit", &positions);
        }
        // DPI rounding can differ from the requested size by one pixel. Reapplying
        // geometry every hit-test tick rebuilds the WebView buttons and cancels animation.
        if !was || layout_changed {
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
            state.showing.store(true, Ordering::Relaxed);
            let _ = w.show();
            let _ = app.emit_to("shortcuts", "kuro-shortcuts-visibility", true);
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
    let state = app.state::<State>();
    let mut rt = state.runtime.lock().unwrap();
    if let Some(mut l) = rt.layout.clone() {
        l.buttons = l.positions(rt.orbit_ms);
        rt.orbit_ready = false;
        let _ = app.emit_to(window.label(), "kuro-shortcuts-layout", l);
    }
    let _ = app.emit_to(window.label(), "kuro-shortcuts-visibility", rt.timer.open);
    Ok(())
}

#[tauri::command]
pub fn shortcut_orbit_ready(window: tauri::WebviewWindow, app: tauri::AppHandle) {
    if window.label() != "shortcuts" {
        return;
    }
    let state = app.state::<State>();
    let mut rt = state.runtime.lock().unwrap();
    rt.orbit_ready = rt.timer.open;
}

#[tauri::command]
pub fn shortcut_next_page(window: tauri::WebviewWindow, app: tauri::AppHandle) {
    if window.label() != "shortcuts" {
        return;
    }
    let state = app.state::<State>();
    let mut rt = state.runtime.lock().unwrap();
    if !rt.timer.open {
        return;
    }
    if let Some(layout) = rt.layout.as_ref().filter(|l| l.pages > 1) {
        rt.page = (layout.page + 1) % layout.pages;
        rt.key = None;
    }
}

#[tauri::command]
pub fn preview_shortcut_effect(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
) -> Result<(), String> {
    authorize(&window, true)?;
    if app
        .state::<State>()
        .store
        .lock()
        .unwrap()
        .config
        .items
        .is_empty()
    {
        return Err("先添加一个快捷入口，再在桌面预览".into());
    }
    crate::close_context_menu(app.clone());
    close(&app);
    *app.state::<State>().preview_until.lock().unwrap() =
        Some(Instant::now() + std::time::Duration::from_secs(6));
    Ok(())
}

#[tauri::command]
pub fn set_shortcut_effect(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    effect: Effect,
) -> Result<(), String> {
    authorize(&window, true)?;
    let state = app.state::<State>();
    let mut store = state.store.lock().unwrap();
    if let Some(error) = &store.error {
        return Err(error.clone());
    }
    let mut next = store.config.clone();
    next.effect = effect;
    write_config(&path(), &next)?;
    store.config = next;
    drop(store);
    close(&app);
    let _ = app.emit("kuro-shortcuts-changed", ());
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
            icon: None,
            hotkey: None,
        }
    }
    #[test]
    fn shortcuts_validation() {
        let i = item("a");
        assert!(validate(&[i.clone()], &[i.clone()]).is_ok());
        let mut full: Vec<_> = (0..MAX_SHORTCUTS)
            .map(|idx| item(&idx.to_string()))
            .collect();
        assert!(validate(&full, &full).is_ok());
        full.push(item("overflow"));
        assert!(validate(&full, &full).is_err());
        assert!(validate(&[i.clone(), i.clone()], &[i.clone()]).is_err());
        let mut bad = i.clone();
        bad.name = " ".into();
        assert!(validate(&[bad], &[i]).is_err());
    }
    #[test]
    fn custom_icon_config_roundtrip() {
        let legacy: Shortcut =
            serde_json::from_str(r#"{"id":"old","name":"旧入口","path":"D:\\missing.exe"}"#)
                .unwrap();
        assert!(legacy.icon.is_none());
        let mut custom = legacy.clone();
        custom.icon = Some(vec![128; 4096]);
        let json = serde_json::to_vec(&custom).unwrap();
        assert_eq!(serde_json::from_slice::<Shortcut>(&json).unwrap(), custom);
        assert!(validate(&[custom.clone()], &[legacy.clone()]).is_ok());
        custom.icon = Some(vec![1; 5]);
        assert!(validate(&[custom], &[legacy]).is_err());
    }
    #[test]
    fn shortcuts_effect_compatibility() {
        let legacy: Config = serde_json::from_str(r#"{"version":1,"items":[]}"#).unwrap();
        assert_eq!(legacy.effect, Effect::Energy);
        let magic: Config =
            serde_json::from_str(r#"{"version":1,"items":[],"effect":"magic"}"#).unwrap();
        assert_eq!(magic.effect, Effect::Magic);
        assert_eq!(
            serde_json::from_slice::<Config>(&serde_json::to_vec(&magic).unwrap())
                .unwrap()
                .effect,
            Effect::Magic
        );
        assert!(serde_json::from_str::<Effect>(r#""unknown""#).is_err());
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
            for n in 1..=MAX_SHORTCUTS {
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
                    let l = layout(pet, work, sf, n, 0);
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
    fn multi_ring_orbit_spacing_and_direction() {
        let pet = Rect {
            x: 930.,
            y: 510.,
            w: 140.,
            h: 180.,
        };
        let work = Rect {
            x: 0.,
            y: 0.,
            w: 2000.,
            h: 1200.,
        };
        let l = layout(pet, work, 1., MAX_SHORTCUTS, 0);
        assert!(l.rotates);
        assert_eq!(l.buttons.len(), MAX_SHORTCUTS);
        let rotated = l.positions(2000);
        for (ring, direction) in [1., -1., 1.].into_iter().enumerate() {
            let a = l.buttons[ring * SLOTS_PER_RING];
            let b = rotated[ring * SLOTS_PER_RING];
            let cross =
                (a.x - l.anchor.x) * (b.y - l.anchor.y) - (a.y - l.anchor.y) * (b.x - l.anchor.x);
            assert!(cross * direction > 0.);
        }
        for (i, a) in rotated.iter().enumerate() {
            assert!(a.x >= l.x + 24. && a.x <= l.x + l.w - 24.);
            assert!(a.y >= l.y + 24. && a.y <= l.y + l.h - 24.);
            for b in rotated.iter().skip(i + 1) {
                assert!(distance(*a, *b) >= 59.9);
            }
        }
    }

    #[test]
    fn partial_ring_is_inside_complete_outer_rings() {
        let pet = Rect {
            x: 930.,
            y: 510.,
            w: 140.,
            h: 180.,
        };
        let work = Rect {
            x: 0.,
            y: 0.,
            w: 2000.,
            h: 1200.,
        };
        for (count, groups) in [(12, vec![4, 8]), (17, vec![1, 8, 8]), (23, vec![7, 8, 8])] {
            let l = layout(pet, work, 1., count, 0);
            assert!(l.rotates && l.buttons.len() == count);
            let rotated = l.positions(1200);
            let mut first = 0;
            let mut previous_radius = 0.;
            for (ring, slots) in groups.into_iter().enumerate() {
                let radius = distance(l.anchor, l.buttons[first]);
                assert!(radius > previous_radius + 59.9);
                for i in first..first + slots {
                    assert!((distance(l.anchor, l.buttons[i]) - radius).abs() < 0.001);
                    let a = l.buttons[i];
                    let b = rotated[i];
                    let cross = (a.x - l.anchor.x) * (b.y - l.anchor.y)
                        - (a.y - l.anchor.y) * (b.x - l.anchor.x);
                    assert_eq!(cross.is_sign_positive(), ring % 2 == 0);
                }
                first += slots;
                previous_radius = radius;
            }
            assert_eq!(first, count);
        }
    }

    #[test]
    fn small_work_area_pages_cover_every_target() {
        let pet = Rect {
            x: 112.,
            y: 83.,
            w: 120.,
            h: 150.,
        };
        let work = Rect {
            x: 0.,
            y: 0.,
            w: 345.,
            h: 310.,
        };
        let first = layout(pet, work, 1., MAX_SHORTCUTS, 0);
        assert!(first.pages > 1 && first.pager.is_some());
        let mut seen = 0;
        for page in 0..first.pages {
            let l = layout(pet, work, 1., MAX_SHORTCUTS, page);
            assert_eq!(l.first, seen);
            let pager = l.pager.unwrap();
            assert!(
                pager.x >= 24.
                    && pager.x <= work.w - 24.
                    && pager.y >= 24.
                    && pager.y <= work.h - 24.
            );
            assert!(l
                .buttons
                .iter()
                .all(|button| distance(pager, *button) >= 59.9));
            assert_eq!(l.positions(2000), l.buttons);
            seen += l.buttons.len();
        }
        assert_eq!(seen, MAX_SHORTCUTS);
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
            effect: Effect::Magic,
            hotkeys_enabled: true,
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
