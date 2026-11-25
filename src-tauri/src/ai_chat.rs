use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    io::ErrorKind,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{Emitter, Manager};

const REPLIES: [&str; 3] = ["能多说一点吗？", "哈哈，你还挺有趣", "我们换个话题吧"];

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub(crate) struct Settings {
    endpoint: String,
    model: String,
    proactive: bool,
    frequency: Frequency,
}

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Frequency {
    Quiet,
    #[default]
    Normal,
    Active,
}

#[derive(Clone, Deserialize, Serialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
struct Saved {
    settings: Settings,
    history: Vec<Message>,
    choices: Vec<String>,
    muted_day: Option<i64>,
}

#[derive(Clone, Serialize)]
struct Reply {
    text: String,
    choices: Vec<String>,
}

struct Bubble {
    reply: Reply,
    expires: Instant,
    hovered: bool,
}

struct Chat {
    saved: Saved,
    bubble: Option<Bubble>,
    due: Option<Instant>,
    ignored: u32,
    error: Option<String>,
    load_error: Option<String>,
}

pub struct State {
    chat: Mutex<Chat>,
    busy: AtomicBool,
    opening: AtomicBool,
}

impl Default for State {
    fn default() -> Self {
        let loaded = match fs::read(data_path()) {
            Ok(bytes) => serde_json::from_slice::<Saved>(&bytes).map_err(|error| error.to_string()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(Saved::default()),
            Err(error) => Err(error.to_string()),
        };
        let (saved, load_error) = match loaded {
            Ok(saved) => (saved, None),
            Err(_) => (
                Saved::default(),
                Some("无法读取 AI 聊天记录，原文件已保留。".into()),
            ),
        };
        Self {
            chat: Mutex::new(Chat {
                saved,
                bubble: None,
                due: None,
                ignored: 0,
                error: None,
                load_error,
            }),
            busy: AtomicBool::new(false),
            opening: AtomicBool::new(false),
        }
    }
}

struct Busy<'a>(&'a AtomicBool);
impl<'a> Busy<'a> {
    fn acquire(flag: &'a AtomicBool) -> Result<Self, String> {
        flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .map(|_| Self(flag))
            .map_err(|_| "请等当前操作完成。".into())
    }
}
impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn data_path() -> PathBuf {
    crate::config::app_data_dir().join("ai-chat.json")
}

fn persist(saved: &Saved) -> Result<(), String> {
    let path = data_path();
    fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let temp = path.with_extension("json.tmp");
    fs::write(
        &temp,
        serde_json::to_vec(&saved).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    fs::rename(temp, path).map_err(|e| e.to_string())
}

fn local_day() -> i64 {
    let mut t = unsafe { std::mem::zeroed::<windows_sys::Win32::Foundation::SYSTEMTIME>() };
    unsafe {
        windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t);
    }
    i64::from(t.wYear) * 10_000 + i64::from(t.wMonth) * 100 + i64::from(t.wDay)
}

fn interval(frequency: Frequency, ignored: u32) -> Duration {
    let (min, max) = match frequency {
        Frequency::Quiet => (60, 120),
        Frequency::Normal => (30, 60),
        Frequency::Active => (10, 25),
    };
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let seconds = min * 60 + u64::from(seed) % ((max - min) * 60 + 1);
    Duration::from_secs(seconds * if ignored >= 2 { 2 } else { 1 })
}

fn check_panel(window: &tauri::WebviewWindow) -> Result<(), String> {
    if window.label() == "ai-chat" {
        Ok(())
    } else {
        Err("请在 AI 菜单中操作。".into())
    }
}

fn valid_endpoint(endpoint: &str) -> Result<tauri::Url, String> {
    let url = tauri::Url::parse(endpoint).map_err(|_| "请输入完整的模型 API 地址。")?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1"));
    if url.host_str().is_none()
        || !(url.scheme() == "https" || url.scheme() == "http" && loopback)
        || url.fragment().is_some()
        || url.query().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || !url.path().ends_with("/chat/completions")
    {
        return Err("请填写以 /chat/completions 结尾的 HTTPS 地址。本机模型可以使用 HTTP。".into());
    }
    Ok(url)
}

fn credential_target(endpoint: &str) -> Vec<u16> {
    format!("KuroCat AI: {endpoint}")
        .encode_utf16()
        .chain(Some(0))
        .collect()
}

fn key_for(endpoint: &str) -> Option<String> {
    use windows_sys::Win32::Security::Credentials::*;
    let target = credential_target(endpoint);
    let mut ptr: *mut CREDENTIALW = std::ptr::null_mut();
    unsafe {
        if CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut ptr) == 0 {
            return None;
        }
        let entry = &*ptr;
        let key = if entry.CredentialBlobSize == 0 {
            None
        } else {
            String::from_utf8(
                std::slice::from_raw_parts(entry.CredentialBlob, entry.CredentialBlobSize as usize)
                    .to_vec(),
            )
            .ok()
        };
        CredFree(ptr.cast());
        key
    }
}

fn change_key(endpoint: &str, secret: &str) -> Result<(), String> {
    use windows_sys::Win32::Security::Credentials::*;
    let mut target = credential_target(endpoint);
    let ok = unsafe {
        if secret.is_empty() {
            if key_for(endpoint).is_none() {
                return Ok(());
            }
            CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0)
        } else {
            let mut entry: CREDENTIALW = std::mem::zeroed();
            entry.Type = CRED_TYPE_GENERIC;
            entry.TargetName = target.as_mut_ptr();
            entry.CredentialBlob = secret.as_ptr() as *mut u8;
            entry.CredentialBlobSize = secret.len() as u32;
            entry.Persist = CRED_PERSIST_LOCAL_MACHINE;
            CredWriteW(&entry, 0)
        }
    };
    if ok == 0 {
        Err("Windows 凭据管理器无法更新 API 密钥。".into())
    } else {
        Ok(())
    }
}

fn normalized_reply(content: &str) -> Result<Reply, String> {
    let content = content.trim();
    let candidate = content
        .strip_prefix("```json")
        .or_else(|| content.strip_prefix("```"))
        .and_then(|s| s.trim_end().strip_suffix("```"))
        .map(str::trim)
        .unwrap_or(content);
    let decoded = serde_json::from_str::<Value>(candidate).ok();
    let text = decoded
        .as_ref()
        .and_then(|v| v.get("text"))
        .and_then(Value::as_str)
        .unwrap_or(content)
        .trim()
        .chars()
        .take(1000)
        .collect::<String>();
    if text.is_empty() {
        return Err("模型返回了空消息。".into());
    }
    let mut choices = Vec::new();
    if let Some(raw) = decoded
        .as_ref()
        .and_then(|v| v.get("choices"))
        .and_then(Value::as_array)
    {
        for choice in raw.iter().filter_map(Value::as_str) {
            let choice = choice.trim().chars().take(22).collect::<String>();
            if !choice.is_empty() && !choices.contains(&choice) {
                choices.push(choice);
            }
            if choices.len() == 3 {
                break;
            }
        }
    }
    for fallback in REPLIES {
        if choices.len() == 3 {
            break;
        }
        if !choices.iter().any(|s| s == fallback) {
            choices.push(fallback.into());
        }
    }
    Ok(Reply { text, choices })
}

fn ask(settings: &Settings, history: &[Message], user: &str) -> Result<Reply, String> {
    let endpoint = valid_endpoint(&settings.endpoint)?;
    if settings.model.trim().is_empty() {
        return Err("请先填写模型名称。".into());
    }
    let mut messages = vec![json!({
        "role":"system", "content":"你是住在电脑上的桌宠，友好、活泼，使用简体中文。发言简短自然，像朋友说话。只返回 JSON 对象，格式为 {\"text\":\"回复正文\",\"choices\":[\"用户能发出的追问\",\"用户能发出的接梗\",\"用户能发出的换话题\"]}。不要使用 Markdown，不要暗示能看到用户屏幕或操作。"
    })];
    messages.extend(
        history
            .iter()
            .skip(history.len().saturating_sub(10))
            .map(|m| json!(m)),
    );
    messages.push(json!({"role":"user","content":user}));
    let body =
        serde_json::to_vec(&json!({"model":settings.model,"messages":messages,"stream":false}))
            .map_err(|_| "无法准备模型请求。")?;
    let key = key_for(&settings.endpoint);
    let mut headers = "Content-Type: application/json\r\nAccept: application/json\r\n".to_owned();
    if let Some(key) = key {
        headers.push_str(&format!("Authorization: Bearer {key}\r\n"));
    }
    let mut response = Vec::new();
    crate::updater::winhttp_client::post_stream(endpoint.as_str(), &headers, &body, |chunk| {
        if response.len() + chunk.len() > 256 * 1024 {
            return Err("模型响应过大。".into());
        }
        response.extend_from_slice(chunk);
        Ok(())
    })?;
    let decoded: Value =
        serde_json::from_slice(&response).map_err(|_| "模型接口没有返回有效 JSON。")?;
    let content = decoded
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .ok_or("模型接口返回的格式不兼容，请检查服务配置。")?;
    normalized_reply(content)
}

fn trim_history(saved: &mut Saved) {
    let excess = saved.history.len().saturating_sub(40);
    saved.history.drain(..excess);
}

fn append_reply(saved: &mut Saved, reply: Reply) {
    saved.history.push(Message {
        role: "assistant".into(),
        content: reply.text,
    });
    saved.choices = reply.choices;
    trim_history(saved);
}

fn bubble_event(app: &tauri::AppHandle, bubble: Option<&Reply>) {
    let _ = app.emit_to("main", "kuro-ai-bubble", bubble);
}

pub fn bubble_visible(app: &tauri::AppHandle) -> bool {
    app.state::<State>().chat.lock().unwrap().bubble.is_some()
}

#[tauri::command]
pub async fn open_ai_menu(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
) -> Result<(), String> {
    if !matches!(window.label(), "main" | "menu") || crate::win_needs_win7_ulw() {
        return Err("此窗口无法打开 AI 菜单。".into());
    }
    let state = app.state::<State>();
    let _opening = Busy::acquire(&state.opening)?;
    let panel = match app.get_webview_window("ai-chat") {
        Some(existing) => existing,
        None => tauri::WebviewWindowBuilder::new(
            &app,
            "ai-chat",
            tauri::WebviewUrl::App("ai-chat.html".into()),
        )
        .title("KuroCat · AI 聊天")
        .inner_size(388., 560.)
        .min_inner_size(330., 420.)
        .visible(false)
        .additional_browser_args("--autoplay-policy=no-user-gesture-required")
        .build()
        .map_err(|e| e.to_string())?,
    };
    if let Some((pet, work, dpi)) = crate::current_pet_rect_and_monitor(&app) {
        let (width, height) = panel
            .outer_size()
            .ok()
            .map(|s| (s.width as i32, s.height as i32))
            .unwrap_or(((388.0 * dpi).round() as i32, (560.0 * dpi).round() as i32));
        let gap = (10.0 * dpi).round() as i32;
        let (x, y) = crate::compute_side_menu_pos(pet, work, (width, height), gap, gap);
        let _ = panel.set_position(tauri::PhysicalPosition::new(x, y));
    }
    {
        let mut chat = state.chat.lock().unwrap();
        if let Some(bubble) = &chat.bubble {
            let mut next = chat.saved.clone();
            append_reply(&mut next, bubble.reply.clone());
            persist(&next)?;
            chat.saved = next;
            chat.bubble = None;
        }
        chat.ignored = 0;
        chat.due = None;
    }
    bubble_event(&app, None);
    crate::close_context_menu(app.clone());
    panel.show().map_err(|e| e.to_string())?;
    panel.set_focus().map_err(|e| e.to_string())?;
    let _ = app.emit_to("ai-chat", "kuro-ai-open", ());
    Ok(())
}

#[tauri::command]
pub fn ai_bubble(window: tauri::WebviewWindow, app: tauri::AppHandle, hovered: bool) {
    if window.label() != "main" {
        return;
    }
    if let Some(bubble) = app.state::<State>().chat.lock().unwrap().bubble.as_mut() {
        bubble.hovered = hovered;
        bubble.expires = Instant::now() + Duration::from_secs(12);
    }
}

#[tauri::command]
pub fn get_ai_chat(window: tauri::WebviewWindow, app: tauri::AppHandle) -> Result<Value, String> {
    check_panel(&window)?;
    let state = app.state::<State>();
    let chat = state.chat.lock().unwrap();
    Ok(
        json!({"settings":chat.saved.settings,"history":chat.saved.history,"choices":chat.saved.choices,
        "hasKey":key_for(&chat.saved.settings.endpoint).is_some(),"muted":chat.saved.muted_day==Some(local_day()),
        "error":chat.error.as_ref().or(chat.load_error.as_ref())}),
    )
}

#[tauri::command]
pub fn save_ai_settings(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    settings: Settings,
    api_key: Option<String>,
) -> Result<(), String> {
    check_panel(&window)?;
    let state = app.state::<State>();
    let _busy = Busy::acquire(&state.busy)?;
    let mut settings = settings;
    settings.endpoint = valid_endpoint(settings.endpoint.trim())?.to_string();
    settings.model = settings.model.trim().to_owned();
    if settings.endpoint.len() > 800 || settings.model.is_empty() || settings.model.len() > 150 {
        return Err("请填写有效的模型地址和模型名。".into());
    }
    if api_key
        .as_ref()
        .is_some_and(|key| key.len() > 2000 || key.chars().any(|c| c.is_control()))
    {
        return Err("API 密钥格式无效。".into());
    }
    let mut chat = state.chat.lock().unwrap();
    if let Some(error) = &chat.load_error {
        return Err(error.clone());
    }
    let mut next = chat.saved.clone();
    next.settings = settings;
    if let Some(key) = api_key {
        let previous = key_for(&next.settings.endpoint);
        change_key(&next.settings.endpoint, key.trim())?;
        if let Err(error) = persist(&next) {
            let _ = change_key(&next.settings.endpoint, previous.as_deref().unwrap_or(""));
            return Err(error);
        }
    } else {
        persist(&next)?;
    }
    chat.saved = next;
    chat.due = None;
    chat.error = None;
    let had_bubble = chat.bubble.take().is_some();
    drop(chat);
    if had_bubble {
        bubble_event(&app, None);
    }
    Ok(())
}

#[tauri::command]
pub fn mute_ai_today(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    mute: bool,
) -> Result<(), String> {
    check_panel(&window)?;
    let state = app.state::<State>();
    let _busy = Busy::acquire(&state.busy)?;
    let mut chat = state.chat.lock().unwrap();
    if let Some(error) = &chat.load_error {
        return Err(error.clone());
    }
    let mut next = chat.saved.clone();
    next.muted_day = mute.then(local_day);
    persist(&next)?;
    chat.saved = next;
    chat.due = None;
    let had_bubble = mute && chat.bubble.take().is_some();
    drop(chat);
    if had_bubble {
        bubble_event(&app, None);
    }
    Ok(())
}

#[tauri::command]
pub async fn send_ai_message(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    text: String,
    test: bool,
) -> Result<(), String> {
    check_panel(&window)?;
    let text = text.trim().to_owned();
    if !test && (text.is_empty() || text.chars().count() > 1000) {
        return Err("请输入 1–1000 字。".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<State>();
        let _busy = Busy::acquire(&state.busy)?;
        let saved = {
            let chat = state.chat.lock().unwrap();
            if let Some(error) = &chat.load_error {
                return Err(error.clone());
            }
            chat.saved.clone()
        };
        if test {
            ask(&saved.settings, &[], "这是连接测试，请回复一句简短问候。")?;
            return Ok(());
        }
        let reply = ask(&saved.settings, &saved.history, &text)?;
        let mut chat = state.chat.lock().unwrap();
        let mut next = chat.saved.clone();
        next.history.push(Message {
            role: "user".into(),
            content: text,
        });
        append_reply(&mut next, reply);
        persist(&next)?;
        chat.saved = next;
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

fn fullscreen_foreground() -> bool {
    use windows_sys::Win32::{
        Foundation::RECT,
        Graphics::Gdi::{
            GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
        },
        UI::WindowsAndMessaging::{GetClassNameW, GetForegroundWindow, GetWindowRect},
    };
    unsafe {
        let window = GetForegroundWindow();
        if window.is_null() {
            return false;
        }
        let mut class = [0u16; 32];
        let n = GetClassNameW(window, class.as_mut_ptr(), class.len() as i32);
        if matches!(
            String::from_utf16_lossy(&class[..n.max(0) as usize]).as_str(),
            "Progman" | "WorkerW"
        ) {
            return false;
        }
        let mut rect: RECT = std::mem::zeroed();
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        let monitor = MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST);
        GetWindowRect(window, &mut rect) != 0
            && GetMonitorInfoW(monitor, &mut info) != 0
            && rect.left <= info.rcMonitor.left
            && rect.top <= info.rcMonitor.top
            && rect.right >= info.rcMonitor.right
            && rect.bottom >= info.rcMonitor.bottom
    }
}

fn can_interrupt(app: &tauri::AppHandle, from_panel: bool) -> bool {
    let state = app.state::<crate::AppState>();
    let settings = state.settings.lock().unwrap();
    let blocked = settings.click_through_locked || settings.obs_enabled && settings.obs_hide_pet;
    drop(settings);
    !blocked
        && !state.paused_guard.load(Ordering::Relaxed)
        && !state.menu_open.load(Ordering::Relaxed)
        && !state.update_banner_open.load(Ordering::Relaxed)
        && !crate::shortcuts::is_showing(app)
        && app
            .get_webview_window("main")
            .is_some_and(|w| w.is_visible().unwrap_or(false))
        && (from_panel
            || !app
                .get_webview_window("ai-chat")
                .is_some_and(|w| w.is_visible().unwrap_or(false)))
        && !crate::game_detect::is_game_active()
        && !fullscreen_foreground()
}

fn ask_opener(saved: &Saved) -> Result<Reply, String> {
    ask(
        &saved.settings,
        &saved.history,
        "请主动用一句简短自然的话给我开个话题，避免重复上次的话。",
    )
}

fn show_bubble(app: &tauri::AppHandle, chat: &mut Chat, reply: Reply) {
    chat.error = None;
    chat.due = None;
    chat.bubble = Some(Bubble {
        reply,
        expires: Instant::now() + Duration::from_secs(12),
        hovered: false,
    });
    bubble_event(app, chat.bubble.as_ref().map(|b| &b.reply));
}

#[tauri::command]
pub async fn preview_ai_opener(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
) -> Result<(), String> {
    check_panel(&window)?;
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<State>();
        let _busy = Busy::acquire(&state.busy)?;
        if !can_interrupt(&app, true) {
            return Err("请先显示桌宠、退出全屏并解锁鼠标穿透，再试一次。".into());
        }
        let saved = {
            let chat = state.chat.lock().unwrap();
            if let Some(error) = &chat.load_error {
                return Err(error.clone());
            }
            chat.saved.clone()
        };
        let reply = ask_opener(&saved)?;
        if !can_interrupt(&app, true) {
            return Err("桌宠当前无法显示气泡，请恢复桌宠后再试。".into());
        }
        window.hide().map_err(|error| error.to_string())?;
        let mut chat = state.chat.lock().unwrap();
        show_bubble(&app, &mut chat, reply);
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

pub fn start(app: tauri::AppHandle) {
    if crate::win_needs_win7_ulw() {
        return;
    }
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(1));
        let state = app.state::<State>();
        let chat = state.chat.lock().unwrap();
        let scheduled = chat.saved.settings.proactive
            && !chat.saved.settings.endpoint.is_empty()
            && chat.saved.muted_day != Some(local_day())
            && chat.load_error.is_none();
        if !scheduled && chat.bubble.is_none() {
            continue;
        }
        drop(chat);
        let eligible = can_interrupt(&app, false);
        let mut chat = state.chat.lock().unwrap();
        let now = Instant::now();
        if !eligible {
            chat.due = None;
            if chat.bubble.take().is_some() {
                bubble_event(&app, None);
            }
            continue;
        }
        if let Some(bubble) = &chat.bubble {
            if !bubble.hovered && now >= bubble.expires {
                chat.bubble = None;
                chat.ignored = chat.ignored.saturating_add(1);
                chat.due = None;
                bubble_event(&app, None);
            }
            continue;
        }
        if !scheduled {
            continue;
        }
        let wait = interval(chat.saved.settings.frequency, chat.ignored);
        if now < *chat.due.get_or_insert(now + wait) {
            continue;
        }
        let Ok(_busy) = Busy::acquire(&state.busy) else {
            continue;
        };
        chat.due = None;
        let saved = chat.saved.clone();
        drop(chat);
        let result = ask_opener(&saved);
        let eligible = can_interrupt(&app, false);
        let mut chat = state.chat.lock().unwrap();
        match result {
            Ok(reply) if eligible => show_bubble(&app, &mut chat, reply),
            Ok(_) => {}
            Err(error) => chat.error = Some(error),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn always_returns_three_choices() {
        let reply = normalized_reply(r#"{"text":"你好","choices":["继续","继续",""]}"#).unwrap();
        assert_eq!(reply.text, "你好");
        assert_eq!(reply.choices.len(), 3);
        assert_eq!(normalized_reply("普通文本").unwrap().choices, REPLIES);
    }

    #[test]
    fn mock_provider_receives_context_and_returns_chat_replies() {
        use std::io::{BufRead, BufReader, Read, Write};
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let settings = Settings {
            endpoint: format!(
                "http://{}/v1/chat/completions",
                listener.local_addr().unwrap()
            ),
            model: "mock-pet".into(),
            ..Settings::default()
        };
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(4)))
                .unwrap();
            let mut reader = BufReader::new(&mut socket);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert!(line.starts_with("POST /v1/chat/completions "));
            let mut length = 0;
            loop {
                line.clear();
                assert_ne!(reader.read_line(&mut line).unwrap(), 0);
                if line == "\r\n" {
                    break;
                }
                if let Some((name, value)) = line.split_once(':') {
                    if name.eq_ignore_ascii_case("content-length") {
                        length = value.trim().parse().unwrap();
                    }
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let request: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(request["model"], "mock-pet");
            assert_eq!(request["messages"][1]["content"], "上句");
            assert_eq!(request["messages"][2]["content"], "下句");
            let answer = json!({"text":"模拟回复","choices":["追问","接梗","换话题"]}).to_string();
            let body = json!({"choices":[{"message":{"content":answer}}]}).to_string();
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
        });
        let reply = ask(
            &settings,
            &[Message {
                role: "assistant".into(),
                content: "上句".into(),
            }],
            "下句",
        )
        .unwrap();
        assert_eq!(reply.text, "模拟回复");
        assert_eq!(reply.choices, ["追问", "接梗", "换话题"]);
        server.join().unwrap();
    }

    #[test]
    fn api_address_rejects_insecure_remote_and_embedded_credentials() {
        assert!(valid_endpoint("https://example.com/v1/chat/completions").is_ok());
        assert!(valid_endpoint("http://127.0.0.1:8080/v1/chat/completions").is_ok());
        assert!(valid_endpoint("http://example.com/v1/chat/completions").is_err());
        assert!(valid_endpoint("https://user:pass@example.com/chat/completions").is_err());
    }
}
