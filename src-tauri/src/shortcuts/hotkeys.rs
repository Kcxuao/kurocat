use super::{launch_by_id, State};
use std::collections::HashMap;
use std::ptr::null_mut;
use std::sync::atomic::Ordering;
use tauri::{Emitter, Manager};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT,
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct Chord {
    modifiers: u32,
    key: u32,
}

impl Chord {
    pub(super) fn parse(text: &str) -> Option<Self> {
        let mut parts = text.split('+');
        let key = parts.next_back()?;
        let key = match key.as_bytes() {
            [key @ (b'0'..=b'9' | b'A'..=b'Z')] => u32::from(*key),
            _ => {
                let function = key.strip_prefix('F')?.parse::<u32>().ok()?;
                if !(1..=11).contains(&function) {
                    return None;
                }
                0x6f + function
            }
        };
        let mut modifiers = 0;
        for part in parts {
            let flag = match part {
                "Alt" => MOD_ALT,
                "Ctrl" => MOD_CONTROL,
                "Shift" => MOD_SHIFT,
                _ => return None,
            };
            if modifiers & flag != 0 {
                return None;
            }
            modifiers |= flag;
        }
        (modifiers & (MOD_ALT | MOD_CONTROL) != 0).then_some(Self { modifiers, key })
    }
}

fn effective(setting: Option<&str>, index: usize) -> Option<Chord> {
    match setting {
        None => Some(Chord {
            modifiers: MOD_ALT,
            key: u32::from(b'1') + index as u32,
        }),
        Some("") => None,
        Some(text) => Chord::parse(text),
    }
}

#[derive(Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Status {
    Active,
    Conflict,
    Duplicate,
    Paused,
}

// These registrations belong to the existing Windows hook message thread.
pub(crate) struct Registry {
    registrations: HashMap<Chord, i32>,
    next_id: i32,
}

impl Registry {
    pub(crate) fn new() -> Self {
        Self {
            registrations: HashMap::new(),
            next_id: 0x6000,
        }
    }

    pub(crate) fn sync(&mut self, app: &tauri::AppHandle, paused: bool) {
        if crate::win_needs_win7_ulw() {
            return;
        }
        let state = app.state::<State>();
        let mut status = HashMap::new();
        let mut desired = HashMap::new();
        {
            let store = state.store.lock().unwrap();
            if store.config.hotkeys_enabled {
                // Explicit custom bindings take priority when reordering creates a collision.
                for custom in [true, false] {
                    for (index, item) in store.config.items.iter().enumerate() {
                        if item.hotkey.is_some() != custom {
                            continue;
                        }
                        let Some(chord) = effective(item.hotkey.as_deref(), index) else {
                            continue;
                        };
                        if paused {
                            status.insert(item.id.clone(), Status::Paused);
                        } else if let std::collections::hash_map::Entry::Vacant(slot) =
                            desired.entry(chord)
                        {
                            slot.insert(item.id.clone());
                        } else {
                            status.insert(item.id.clone(), Status::Duplicate);
                        }
                    }
                }
            }
        }
        self.registrations.retain(|chord, id| {
            if desired.contains_key(chord) {
                true
            } else {
                unsafe {
                    UnregisterHotKey(null_mut(), *id);
                }
                false
            }
        });
        for (chord, item) in desired {
            let registered = if self.registrations.contains_key(&chord) {
                true
            } else {
                let id = self.next_id;
                self.next_id = if id == 0xbfff { 0x6000 } else { id + 1 };
                let ok = unsafe {
                    RegisterHotKey(null_mut(), id, chord.modifiers | MOD_NOREPEAT, chord.key)
                } != 0;
                if ok {
                    self.registrations.insert(chord, id);
                }
                ok
            };
            status.insert(
                item,
                if registered {
                    Status::Active
                } else {
                    Status::Conflict
                },
            );
        }
        let mut old = state.hotkey_status.lock().unwrap();
        if *old != status {
            *old = status.clone();
            drop(old);
            let _ = app.emit_to("menu", "kuro-shortcut-hotkeys", status);
        }
    }

    pub(crate) fn dispatch(&self, app: &tauri::AppHandle, id: i32) {
        let Some((chord, _)) = self
            .registrations
            .iter()
            .find(|(_, registered)| **registered == id)
        else {
            return;
        };
        let state = app.state::<State>();
        if state.dialog.load(Ordering::Relaxed) {
            return;
        }
        let item = {
            let store = state.store.lock().unwrap();
            if !store.config.hotkeys_enabled {
                return;
            }
            [true, false].into_iter().find_map(|custom| {
                store
                    .config
                    .items
                    .iter()
                    .enumerate()
                    .find(|(index, item)| {
                        item.hotkey.is_some() == custom
                            && effective(item.hotkey.as_deref(), *index) == Some(*chord)
                    })
                    .map(|(_, item)| item.id.clone())
            })
        };
        if let Some(item) = item {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let _ = launch_by_id(app, item).await;
            });
        }
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        for id in self.registrations.values() {
            unsafe {
                UnregisterHotKey(null_mut(), *id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_follow_order_and_custom_keys_do_not() {
        assert_eq!(effective(None, 0), Chord::parse("Alt+1"));
        assert_eq!(effective(None, 7), Chord::parse("Alt+8"));
        assert_eq!(
            effective(Some("Shift+Ctrl+J"), 4),
            Chord::parse("Ctrl+Shift+J")
        );
        assert_eq!(effective(Some(""), 1), None);
        assert!(Chord::parse("F12").is_none());
        assert!(Chord::parse("Alt+F12").is_none());
        assert!(Chord::parse("Ctrl+Ctrl+J").is_none());
    }
}
