use std::{
    f32::consts::PI,
    ops::Add,
    sync::{Arc, LazyLock},
    time::Instant,
};

use glam::{vec3a, Quat};
use idmap_derive::IntegerId;
use serde::{Deserialize, Serialize};

use crate::{
    backend::{
        common::OverlaySelector,
        overlay::{OverlayBackend, OverlayState, Positioning, Z_ORDER_TOAST},
        task::TaskType,
    },
    gui::{canvas::builder::CanvasBuilder, color_parse},
    state::{AppState, LeftRight},
};

const FONT_SIZE: isize = 16;
const PADDING: (f32, f32) = (25., 7.);
const PIXELS_TO_METERS: f32 = 1. / 2000.;
static TOAST_NAME: LazyLock<Arc<str>> = LazyLock::new(|| "toast".into());

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum DisplayMethod {
    Hide,
    Center,
    Watch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, IntegerId, Serialize, Deserialize)]
pub enum ToastTopic {
    System,
    DesktopNotification,
    XSNotification,
    IpdChange,
}

pub struct Toast {
    pub title: Arc<str>,
    pub body: Arc<str>,
    pub opacity: f32,
    pub timeout: f32,
    pub sound: bool,
    pub topic: ToastTopic,
}

#[allow(dead_code)]
impl Toast {
    pub const fn new(topic: ToastTopic, title: Arc<str>, body: Arc<str>) -> Self {
        Self {
            title,
            body,
            opacity: 1.0,
            timeout: 3.0,
            sound: false,
            topic,
        }
    }
    pub const fn with_timeout(mut self, timeout: f32) -> Self {
        self.timeout = timeout;
        self
    }
    pub const fn with_opacity(mut self, opacity: f32) -> Self {
        self.opacity = opacity;
        self
    }
    pub const fn with_sound(mut self, sound: bool) -> Self {
        self.sound = sound;
        self
    }
    pub fn submit(self, app: &mut AppState) {
        self.submit_at(app, Instant::now());
    }
    pub fn submit_at(self, app: &mut AppState, instant: Instant) {
        let selector = OverlaySelector::Name(TOAST_NAME.clone());

        let destroy_at = instant.add(std::time::Duration::from_secs_f32(self.timeout));

        let has_sound = self.sound && app.session.config.notifications_sound_enabled;
        if has_sound {
            app.audio.play(app.toast_sound);
        }

        // drop any toast that was created before us.
        // (DropOverlay only drops overlays that were
        // created before current frame)
        app.tasks
            .enqueue_at(TaskType::DropOverlay(selector.clone()), instant);

        // CreateOverlay only creates the overlay if
        // the selector doesn't exist yet, so in case
        // multiple toasts are submitted for the same
        // frame, only the first one gets created
        app.tasks.enqueue_at(
            TaskType::CreateOverlay(
                selector,
                Box::new(move |app| {
                    let mut maybe_toast = new_toast(self, app);
                    if let Some((state, _)) = maybe_toast.as_mut() {
                        state.auto_movement(app);
                        app.tasks.enqueue_at(
                            // at timeout, drop the overlay by ID instead
                            // in order to avoid dropping any newer toasts
                            TaskType::DropOverlay(OverlaySelector::Id(state.id)),
                            destroy_at,
                        );
                    }
                    maybe_toast
                }),
            ),
            instant,
        );
    }
}

fn new_toast(toast: Toast, app: &mut AppState) -> Option<(OverlayState, Box<dyn OverlayBackend>)> {
    let current_method = app
        .session
        .toast_topics
        .get(toast.topic)
        .copied()
        .unwrap_or(DisplayMethod::Hide);

    let (spawn_point, spawn_rotation, positioning) = match current_method {
        DisplayMethod::Hide => return None,
        DisplayMethod::Center => (
            vec3a(0., -0.2, -0.5),
            Quat::IDENTITY,
            Positioning::FollowHead { lerp: 0.1 },
        ),
        DisplayMethod::Watch => {
            let mut watch_pos = app.session.config.watch_pos + vec3a(-0.005, -0.05, 0.02);
            let mut watch_rot = app.session.config.watch_rot;
            let relative_to = match app.session.config.watch_hand {
                LeftRight::Left => Positioning::FollowHand { hand: 0, lerp: 1.0 },
                LeftRight::Right => {
                    watch_pos.x = -watch_pos.x;
                    watch_rot = watch_rot * Quat::from_rotation_x(PI) * Quat::from_rotation_z(PI);
                    Positioning::FollowHand { hand: 1, lerp: 1.0 }
                }
            };
            (watch_pos, watch_rot, relative_to)
        }
    };

    let title = if toast.title.is_empty() {
        "Notification".into()
    } else {
        toast.title
    };

    let mut size = if toast.body.is_empty() {
        let (w, h) = app
            .fc
            .get_text_size(&title, FONT_SIZE, app.graphics.clone())
            .ok()?;
        (w, h + 20.)
    } else {
        let (w0, _) = app
            .fc
            .get_text_size(&title, FONT_SIZE, app.graphics.clone())
            .ok()?;
        let (w1, h1) = app
            .fc
            .get_text_size(&toast.body, FONT_SIZE, app.graphics.clone())
            .ok()?;
        (w0.max(w1), h1 + 50.)
    };

    let og_width = size.0;
    size.0 += PADDING.0 * 2.;

    let mut canvas = CanvasBuilder::<(), ()>::new(
        size.0 as _,
        size.1 as _,
        app.graphics.clone(),
        app.graphics.native_format,
        (),
    )
    .ok()?;

    canvas.font_size = FONT_SIZE;
    canvas.fg_color = color_parse("#cad3f5").unwrap(); // want panic
    canvas.bg_color = color_parse("#1e2030").unwrap(); // want panic
    canvas.panel(0., 0., size.0, size.1, 16.);

    if toast.body.is_empty() {
        canvas.label_centered(PADDING.0, 0., og_width, size.1, 16., title);
    } else {
        canvas.label(PADDING.0, 54., og_width, size.1 - 54., 3., toast.body);

        canvas.fg_color = color_parse("#b8c0e0").unwrap(); // want panic
        canvas.bg_color = color_parse("#24273a").unwrap(); // want panic
        canvas.panel(0., 0., size.0, 30., 16.);
        canvas.label_centered(PADDING.0, 16., og_width, FONT_SIZE as f32 + 2., 16., title);
    }

    let state = OverlayState {
        name: TOAST_NAME.clone(),
        want_visible: true,
        spawn_scale: size.0 * PIXELS_TO_METERS,
        spawn_rotation,
        spawn_point,
        z_order: Z_ORDER_TOAST,
        positioning,
        ..Default::default()
    };
    let backend = Box::new(canvas.build());

    Some((state, backend))
}

fn msg_err(app: &mut AppState, message: &str) {
    Toast::new(ToastTopic::System, "Error".into(), message.into())
        .with_timeout(3.)
        .submit(app);
}

// Display the same error in the terminal and as a toast in VR.
// Formatted as "Failed to XYZ: Object is not defined"
pub fn error_toast<ErrorType>(app: &mut AppState, title: &str, err: ErrorType)
where
    ErrorType: std::fmt::Display + std::fmt::Debug,
{
    log::error!("{title}: {err:?}"); // More detailed version (use Debug)

    // Brief version (use Display)
    msg_err(app, &format!("{title}: {err}"));
}

pub fn error_toast_str(app: &mut AppState, message: &str) {
    log::error!("{message}");
    msg_err(app, message);
}
