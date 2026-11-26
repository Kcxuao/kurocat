use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::PathBuf,
    sync::{atomic::Ordering, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};

mod motion;
mod windows;

pub(crate) fn is_moving() -> bool {
    motion::active()
}
const EFFECT: &str = "mood-effect";
static EFFECT_LOCK: Mutex<()> = Mutex::new(());
const MINUTE: Duration = Duration::from_secs(60);
const BORED_AFTER: Duration = Duration::from_secs(20 * 60);
const COOLDOWN: Duration = Duration::from_secs(15 * 60);

#[derive(Clone, Deserialize, Serialize)]
#[serde(default)]
struct Saved {
    score: u8,
    quiet_until: u64,
    pranks_enabled: bool,
}

impl Default for Saved {
    fn default() -> Self {
        Self {
            score: 70,
            quiet_until: 0,
            pranks_enabled: false,
        }
    }
}

#[derive(Clone, Serialize)]
pub(crate) struct Effect {
    id: u64,
    kind: &'static str,
}

#[derive(Clone, Copy)]
pub(crate) enum Interaction {
    Pet,
    Feed,
    Chat,
}

struct Mood {
    saved: Saved,
    idle: Duration,
    cooldowns: [Option<Instant>; 3],
    chat_turns: u8,
    storage_error: Option<String>,
    generation: u64,
    next_prank: Instant,
    effect: Option<Effect>,
    effect_ready: bool,
    hover: Option<((i32, i32), Instant, Instant)>,
}

impl Mood {
    fn bored(&self) -> bool {
        self.idle >= BORED_AFTER
    }
    fn tier(&self) -> &'static str {
        match self.saved.score {
            70.. => "happy",
            35.. => "calm",
            _ => "low",
        }
    }
    fn decay_step(&self) -> u64 {
        self.idle.saturating_sub(BORED_AFTER).as_secs() / 600
    }
    fn tick(&mut self, elapsed: Duration) {
        let previous = self.decay_step();
        let was_bored = self.bored();
        self.idle += elapsed;
        if !was_bored && self.bored() {
            self.next_prank = self.next_prank.max(Instant::now() + MINUTE);
        }
        let steps = self.decay_step().saturating_sub(previous);
        self.saved.score = self
            .saved
            .score
            .saturating_sub((steps.saturating_mul(4)).min(100) as u8);
    }
    fn interact(&mut self, kind: Interaction, now: Instant) {
        self.idle = Duration::ZERO;
        self.generation += 1;
        let (index, points, gap) = match kind {
            Interaction::Pet => (0, 3, 10),
            Interaction::Feed => (1, 8, 60),
            Interaction::Chat => {
                self.chat_turns = (self.chat_turns + 1).min(3);
                if self.chat_turns < 3 {
                    return;
                }
                (2, 10, 600)
            }
        };
        if self.cooldowns[index]
            .is_some_and(|last| now.duration_since(last) < Duration::from_secs(gap))
        {
            return;
        }
        self.cooldowns[index] = Some(now);
        if index == 2 {
            self.chat_turns = 0;
        }
        self.saved.score = self.saved.score.saturating_add(points).min(100);
    }
}

pub(crate) struct State(Mutex<Mood>);

impl Default for State {
    fn default() -> Self {
        let result = match fs::read(path()) {
            Ok(data) => serde_json::from_slice::<Saved>(&data).map_err(|e| e.to_string()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Saved::default()),
            Err(e) => Err(e.to_string()),
        };
        let (mut saved, storage_error) = match result {
            Ok(saved) => (saved, None),
            Err(e) => (
                Saved::default(),
                Some(format!("无法读取心情数据，已保留原文件：{e}")),
            ),
        };
        saved.score = saved.score.min(100);
        let now = Instant::now();
        let quiet = Duration::from_secs(saved.quiet_until.saturating_sub(unix_now()).min(3600));
        Self(Mutex::new(Mood {
            saved,
            storage_error,
            idle: Duration::ZERO,
            cooldowns: [None; 3],
            chat_turns: 0,
            generation: 0,
            next_prank: now + quiet + MINUTE,
            effect: None,
            effect_ready: false,
            hover: None,
        }))
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn path() -> PathBuf {
    crate::config::app_data_dir().join("mood.json")
}

fn persist(mood: &Mood) -> Result<(), String> {
    if let Some(error) = &mood.storage_error {
        return Err(error.clone());
    }
    let path = path();
    fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    let data = serde_json::to_vec(&mood.saved).map_err(|e| e.to_string())?;
    let result = fs::write(&temp, data).and_then(|()| fs::rename(&temp, path));
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result.map_err(|e| format!("无法保存心情数据：{e}"))
}

fn snapshot(mood: &Mood) -> serde_json::Value {
    serde_json::json!({
        "score": mood.saved.score, "tier": mood.tier(), "bored": mood.bored(),
        "quietUntil": mood.saved.quiet_until, "pranksEnabled": mood.saved.pranks_enabled,
        "error": mood.storage_error,
    })
}

fn publish(app: &AppHandle, snapshot: serde_json::Value, expression: Option<&str>) {
    if let Some(bored) = snapshot.get("bored").and_then(serde_json::Value::as_bool) {
        let _ = app.emit_to("main", "kuro-mood-bored", bored);
    }
    let _ = app.emit_to("menu", "kuro-mood-changed", snapshot);
    if let Some(expression) = expression {
        let _ = app.emit_to("main", "kuro-mood-expression", expression);
    }
}

pub(crate) fn interact(app: &AppHandle, kind: Interaction) {
    if crate::win_needs_win7_ulw() {
        return;
    }
    let state = app.state::<State>();
    let mut mood = state.0.lock().unwrap();
    let score = mood.saved.score;
    let visible_change = mood.bored();
    mood.interact(kind, Instant::now());
    let changed = score != mood.saved.score;
    if changed {
        if let Err(error) = persist(&mood) {
            mood.storage_error = Some(error);
        }
    }
    if changed || visible_change {
        let data = snapshot(&mood);
        drop(mood);
        publish(app, data, None);
    }
}

pub(crate) fn pet_hover(app: &AppHandle, position: (i32, i32), over: bool) {
    if crate::win_needs_win7_ulw() {
        return;
    }
    let state = app.state::<State>();
    let mut mood = state.0.lock().unwrap();
    if !over || is_moving() {
        mood.hover = None;
        return;
    }
    let now = Instant::now();
    let (previous, first, last) = mood.hover.get_or_insert((position, now, now));
    if now.duration_since(*last) > Duration::from_millis(300) {
        *first = now;
    }
    if previous.0.abs_diff(position.0) + previous.1.abs_diff(position.1) < 2 {
        return;
    }
    *previous = position;
    *last = now;
    if now.duration_since(*first) >= Duration::from_millis(400) {
        *first = now;
        drop(mood);
        interact(app, Interaction::Pet);
    }
}

fn menu(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == "menu" && !crate::win_needs_win7_ulw() {
        Ok(())
    } else {
        Err("当前窗口不支持心情系统。".into())
    }
}

#[tauri::command]
pub fn get_mood(window: WebviewWindow, app: AppHandle) -> Result<serde_json::Value, String> {
    if window.label() != "main" || crate::win_needs_win7_ulw() {
        menu(&window)?;
    }
    Ok(snapshot(&app.state::<State>().0.lock().unwrap()))
}

#[tauri::command]
pub fn set_mood_quiet(window: WebviewWindow, app: AppHandle, quiet: bool) -> Result<(), String> {
    menu(&window)?;
    update(&app, |mood| {
        mood.saved.quiet_until = if quiet { unix_now() + 3600 } else { 0 };
        mood.next_prank = mood.next_prank.max(Instant::now() + MINUTE);
    })
}

#[tauri::command]
pub fn set_mood_pranks(window: WebviewWindow, app: AppHandle, enabled: bool) -> Result<(), String> {
    menu(&window)?;
    update(&app, |mood| {
        mood.saved.pranks_enabled = enabled;
        if enabled {
            mood.next_prank = Instant::now() + MINUTE;
        }
    })
}

fn update(app: &AppHandle, change: impl FnOnce(&mut Mood)) -> Result<(), String> {
    let state = app.state::<State>();
    let mut mood = state.0.lock().unwrap();
    let previous = mood.saved.clone();
    change(&mut mood);
    if let Err(error) = persist(&mood) {
        mood.saved = previous;
        return Err(error);
    }
    mood.generation += 1;
    let data = snapshot(&mood);
    drop(mood);
    publish(app, data, None);
    Ok(())
}

#[tauri::command]
pub fn get_mood_effect(window: WebviewWindow, app: AppHandle) -> Option<Effect> {
    if window.label() != EFFECT {
        return None;
    }
    app.state::<State>().0.lock().unwrap().effect.clone()
}

#[tauri::command]
pub fn mood_effect_ready(window: WebviewWindow, app: AppHandle, id: u64) {
    if window.label() != EFFECT {
        return;
    }
    let state = app.state::<State>();
    let mut mood = state.0.lock().unwrap();
    if mood.generation == id && mood.effect.as_ref().is_some_and(|item| item.id == id) {
        mood.effect_ready = true;
    }
}

fn eligible(app: &AppHandle) -> bool {
    !app.state::<crate::AppState>()
        .dragging
        .load(Ordering::Relaxed)
        && !crate::ai_chat::bubble_visible(app)
        && crate::ai_chat::can_interrupt(app, false)
}

fn overlay(
    app: &AppHandle,
    target: &windows::Target,
    effect: Effect,
) -> Result<WebviewWindow, String> {
    let window = match app.get_webview_window(EFFECT) {
        Some(window) => window,
        None => tauri::WebviewWindowBuilder::new(
            app,
            EFFECT,
            tauri::WebviewUrl::App("mood-effect.html".into()),
        )
        .inner_size(92., 92.)
        .decorations(false)
        .transparent(true)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .additional_browser_args("--autoplay-policy=no-user-gesture-required")
        .focusable(false)
        .focused(false)
        .visible(false)
        .build()
        .map_err(|e| e.to_string())?,
    };
    window
        .set_ignore_cursor_events(true)
        .map_err(|e| e.to_string())?;
    let state = app.state::<State>();
    let mut mood = state.0.lock().unwrap();
    mood.effect_ready = false;
    mood.effect = Some(effect);
    drop(mood);
    let (x, y) = target.overlay_position(&window);
    window
        .set_position(tauri::PhysicalPosition::new(x, y))
        .map_err(|e| e.to_string())?;
    let _ = app.emit_to(EFFECT, "kuro-mood-effect", ());
    window.show().map_err(|e| e.to_string())?;
    Ok(window)
}

#[tauri::command]
pub async fn preview_mood_prank(
    window: WebviewWindow,
    app: AppHandle,
    kind: String,
) -> Result<(), String> {
    menu(&window)?;
    if !matches!(kind.as_str(), "shake" | "tug" | "seesaw") {
        return Err("未知的恶作剧。".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = EFFECT_LOCK
            .try_lock()
            .map_err(|_| "桌宠正在捣乱，请稍后再试。")?;
        if !app.state::<State>().0.lock().unwrap().saved.pranks_enabled {
            return Err("请先开启「允许恶作剧」。".into());
        }
        let target = windows::Target::topmost_application().ok_or("没有打开的应用窗口。")?;
        crate::close_context_menu(app.clone());
        let valid = || {
            !crate::feeding::is_walking(&app)
                && app.state::<State>().0.lock().unwrap().saved.pranks_enabled
        };
        let moved = if kind == "shake" {
            target.shake(valid)
        } else if valid() {
            motion::perform(&app, &target, kind == "tug", valid)
        } else {
            return Err("等桌宠吃完再试。".into());
        };
        if !moved {
            return Err("Windows 未移动这个应用窗口。".into());
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

fn run_prank(app: &AppHandle, target: windows::Target, kind: &'static str, generation: u64) {
    let Ok(_guard) = EFFECT_LOCK.try_lock() else {
        return;
    };
    let state = app.state::<State>();
    let valid = || {
        windows::active_session()
            && eligible(app)
            && state.0.lock().unwrap().generation == generation
    };
    if matches!(kind, "tug" | "seesaw") {
        if valid() {
            motion::perform(app, &target, kind == "tug", valid);
        }
        return;
    }
    if !valid() {
        return;
    }
    if let Ok(window) = overlay(
        app,
        &target,
        Effect {
            id: generation,
            kind,
        },
    ) {
        let ready_by = Instant::now() + Duration::from_secs(2);
        while valid() && Instant::now() < ready_by && !state.0.lock().unwrap().effect_ready {
            std::thread::sleep(Duration::from_millis(40));
        }
        let stop = Instant::now() + Duration::from_millis(1200);
        while valid() && Instant::now() < stop {
            std::thread::sleep(Duration::from_millis(40));
        }
        let _ = window.hide();
    }
    state.0.lock().unwrap().effect = None;
}

pub(crate) fn start(app: AppHandle) {
    if crate::win_needs_win7_ulw() {
        return;
    }
    std::thread::spawn(move || {
        let state = app.state::<State>();
        let mut last = Instant::now();
        let mut last_awake = windows::awake_time();
        let mut was_active = false;
        loop {
            std::thread::sleep(Duration::from_millis(1000));
            let elapsed = last.elapsed();
            last = Instant::now();
            let awake = windows::awake_time();
            let awake_elapsed = awake
                .zip(last_awake)
                .map(|(now, before)| now.saturating_sub(before));
            last_awake = awake;
            let active = windows::active_session();
            let resumed = !was_active;
            was_active = active;
            let Some(awake_elapsed) = awake_elapsed else {
                continue;
            };
            if !active
                || resumed
                || elapsed.saturating_sub(awake_elapsed) > Duration::from_millis(300)
            {
                continue;
            }
            let mut mood = state.0.lock().unwrap();
            let (score, tier, bored) = (mood.saved.score, mood.tier(), mood.bored());
            mood.tick(awake_elapsed);
            if mood.saved.score != score {
                if let Err(error) = persist(&mood) {
                    mood.storage_error = Some(error);
                }
            }
            let changed = mood.saved.score != score || mood.bored() != bored;
            let expression = if tier != "low" && mood.tier() == "low" {
                Some("low")
            } else if !bored && mood.bored() {
                Some("bored")
            } else {
                None
            };
            let data = changed.then(|| snapshot(&mood));
            let due = mood.saved.pranks_enabled
                && mood.storage_error.is_none()
                && Instant::now() >= mood.next_prank
                && mood.saved.quiet_until <= unix_now()
                && (mood.bored() || mood.tier() == "low");
            drop(mood);
            if let Some(data) = data {
                publish(&app, data, expression);
            }
            if !due || !eligible(&app) {
                continue;
            }
            let Some(target) = windows::Target::topmost_application() else {
                continue;
            };
            let mut mood = state.0.lock().unwrap();
            if !mood.saved.pranks_enabled
                || mood.saved.quiet_until > unix_now()
                || (!mood.bored() && mood.tier() != "low")
            {
                continue;
            }
            let kind = if mood.tier() == "low" {
                "tug"
            } else {
                match unix_now() % 4 {
                    0 => "peek",
                    1 => "knock",
                    _ => "seesaw",
                }
            };
            mood.generation += 1;
            let generation = mood.generation;
            mood.next_prank = Instant::now() + COOLDOWN;
            drop(mood);
            run_prank(&app, target, kind, generation);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires an unlocked interactive Windows session"]
    fn detects_unlocked_desktop() {
        assert!(windows::active_session());
        assert!(windows::awake_time().is_some());
    }

    #[test]
    fn boredom_decay_and_interaction_boundaries() {
        let state = State::default();
        let mut mood = state.0.lock().unwrap();
        mood.saved = Saved::default();
        mood.tick(BORED_AFTER);
        assert!(mood.bored());
        assert_eq!(mood.saved.score, 70);
        mood.tick(MINUTE * 10);
        assert_eq!(mood.saved.score, 66);
        let now = Instant::now();
        mood.interact(Interaction::Pet, now);
        mood.interact(Interaction::Pet, now);
        assert_eq!(mood.saved.score, 69);
        assert!(!mood.bored());
        for _ in 0..3 {
            mood.interact(Interaction::Chat, now);
        }
        assert_eq!(mood.saved.score, 79);
        mood.tick(MINUTE * 300);
        assert_eq!(mood.saved.score, 0);
    }
}
