use glam::Vec3A;
use std::sync::{Arc, LazyLock};

use crate::backend::overlay::{OverlayData, OverlayState, Positioning, Z_ORDER_ANCHOR};
use crate::config::{load_known_yaml, ConfigType};
use crate::gui::modular::{modular_canvas, ModularUiConfig};
use crate::state::AppState;

pub static ANCHOR_NAME: LazyLock<Arc<str>> = LazyLock::new(|| Arc::from("anchor"));

pub fn create_anchor<O>(state: &mut AppState) -> anyhow::Result<OverlayData<O>>
where
    O: Default,
{
    let config = load_known_yaml::<ModularUiConfig>(ConfigType::Anchor);

    Ok(OverlayData {
        state: OverlayState {
            name: ANCHOR_NAME.clone(),
            want_visible: false,
            interactable: false,
            grabbable: false,
            z_order: Z_ORDER_ANCHOR,
            spawn_scale: config.width,
            spawn_point: Vec3A::NEG_Z * 0.5,
            positioning: Positioning::Static,
            ..Default::default()
        },
        backend: Box::new(modular_canvas(config.size, &config.elements, state)?),
        ..Default::default()
    })
}
