use std::{cell::RefCell, rc::Rc};

use super::hid::{self, HidProvider, VirtualKey};
use crate::overlays::wayvr::WayVRData;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum KeyboardFocus {
    PhysicalScreen,

    #[allow(dead_code)] // Not available if "wayvr" feature is disabled
    WayVR, // (for now without wayland window id data, it's handled internally),
}

pub struct HidWrapper {
    pub keyboard_focus: KeyboardFocus,
    pub inner: Box<dyn HidProvider>,
    pub wayvr: Option<Rc<RefCell<WayVRData>>>, // Dynamically created if requested
}

impl HidWrapper {
    pub fn new() -> Self {
        Self {
            keyboard_focus: KeyboardFocus::PhysicalScreen,
            inner: hid::initialize(),
            wayvr: None,
        }
    }

    pub fn send_key_routed(&self, key: VirtualKey, down: bool) {
        match self.keyboard_focus {
            KeyboardFocus::PhysicalScreen => self.inner.send_key(key, down),
            KeyboardFocus::WayVR =>
            {
                #[cfg(feature = "wayvr")]
                if let Some(wayvr) = &self.wayvr {
                    wayvr.borrow_mut().data.state.send_key(key as u32, down);
                }
            }
        }
    }

    pub fn set_modifiers_routed(&mut self, mods: u8) {
        match self.keyboard_focus {
            KeyboardFocus::PhysicalScreen => self.inner.set_modifiers(mods),
            KeyboardFocus::WayVR =>
            {
                #[cfg(feature = "wayvr")]
                if let Some(wayvr) = &self.wayvr {
                    wayvr.borrow_mut().data.state.set_modifiers(mods);
                }
            }
        }
    }
}
