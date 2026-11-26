use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};

use super::windows::Target;
use crate::feeding::{walk, walk_duration, Point};

static MOVING: AtomicBool = AtomicBool::new(false);
const FRAME_MS: u64 = 25;
pub(super) const TUG: &[(i32, i32)] = &[
    (-14, 0),
    (-36, -3),
    (-65, -8),
    (-96, -13),
    (-128, -15),
    (-128, -15),
    (-103, -10),
    (-64, -4),
    (-22, 2),
    (26, 7),
    (-12, -4),
    (7, 2),
    (0, 0),
];
pub(super) const LEFT_LANDING: &[(i32, i32)] = &[
    (-12, 12),
    (-29, 32),
    (-43, 54),
    (-30, 37),
    (-12, 13),
    (9, -12),
    (-4, 5),
    (0, 0),
];
pub(super) const RIGHT_LANDING: &[(i32, i32)] = &[
    (14, 14),
    (34, 41),
    (50, 66),
    (35, 45),
    (16, 16),
    (-11, -15),
    (5, 6),
    (0, 0),
];

pub(super) fn active() -> bool {
    MOVING.load(Ordering::Acquire)
}

struct Pet<'a> {
    app: &'a AppHandle,
    window: WebviewWindow,
    origin: Point,
    last: Point,
    scale: f64,
}

impl<'a> Pet<'a> {
    fn begin(app: &'a AppHandle) -> Option<Self> {
        let window = app.get_webview_window("main")?;
        if !window.is_visible().ok()? {
            return None;
        }
        let origin = Point::of(window.outer_position().ok()?);
        let scale = window.scale_factor().ok()?;
        MOVING.store(true, Ordering::Release);
        Some(Self {
            app,
            window,
            origin,
            last: origin,
            scale,
        })
    }

    fn phase(&self, phase: &str) {
        let _ = self.app.emit_to("main", "kuro-mood-motion", phase);
    }

    fn owns_position(&self) -> bool {
        !self
            .app
            .state::<crate::AppState>()
            .dragging
            .load(Ordering::Relaxed)
            && self
                .window
                .outer_position()
                .ok()
                .is_some_and(|p| Point::of(p).near(self.last))
    }

    fn travel(&mut self, goal: Point, hop: bool, valid: &impl Fn() -> bool) -> bool {
        let from = self.last;
        if from == goal {
            return valid() && self.owns_position();
        }
        let duration = if hop {
            (walk_duration(from, goal, self.scale) / 2).clamp(550, 1550)
        } else {
            walk_duration(from, goal, self.scale).min(4600)
        };
        if hop {
            self.phase("crouch");
            std::thread::sleep(Duration::from_millis(200));
        }
        self.phase(if hop { "hop" } else { "walk" });
        let frames = duration.div_ceil(FRAME_MS);
        for frame in 1..=frames {
            if !valid() || !self.owns_position() {
                return false;
            }
            let elapsed = (frame * FRAME_MS).min(duration);
            let mut next = walk(from, goal, elapsed, duration);
            if hop {
                let t = elapsed as f64 / duration as f64;
                next.y -= (4.0 * t * (1.0 - t) * (72.0 * self.scale)) as i32;
            }
            if next != self.last && self.window.set_position(next.position()).is_err() {
                return false;
            }
            self.last = next;
            if frame < frames {
                std::thread::sleep(Duration::from_millis(FRAME_MS));
            }
        }
        true
    }
}

impl Drop for Pet<'_> {
    fn drop(&mut self) {
        self.phase("stop");
        MOVING.store(false, Ordering::Release);
    }
}

pub(super) fn perform(
    app: &AppHandle,
    target: &Target,
    tug: bool,
    valid: impl Fn() -> bool,
) -> bool {
    let Some(mut pet) = Pet::begin(app) else {
        return false;
    };
    let Ok(size) = pet.window.outer_size() else {
        return false;
    };
    let [left, right] = target.pet_landings(size.width as i32, size.height as i32, pet.scale, tug);
    let first = if tug { right } else { left };
    if !pet.travel(first, false, &valid) {
        return false;
    }

    let effect = |pet: &Pet<'_>, phase, offsets| {
        if !valid() || !pet.owns_position() {
            return false;
        }
        pet.phase(phase);
        std::thread::sleep(Duration::from_millis(105));
        target.nudge(offsets, Duration::from_millis(55), || {
            valid() && pet.owns_position()
        })
    };
    let completed = if tug {
        pet.phase("grip");
        std::thread::sleep(Duration::from_millis(260));
        let moved = effect(&pet, "tug", TUG);
        if moved && valid() && pet.owns_position() {
            pet.phase("stumble");
            std::thread::sleep(Duration::from_millis(610));
        }
        moved
    } else {
        let moved = effect(&pet, "land", LEFT_LANDING)
            && pet.travel(right, true, &valid)
            && effect(&pet, "land", RIGHT_LANDING);
        if moved && valid() && pet.owns_position() {
            pet.phase("rebound");
            std::thread::sleep(Duration::from_millis(530));
        }
        moved
    };
    if valid() && pet.owns_position() {
        pet.travel(pet.origin, false, &valid);
    }
    completed
}
