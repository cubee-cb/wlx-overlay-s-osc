use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    time::Instant,
};

use anyhow::bail;
use rosc::{OscMessage, OscPacket, OscType};

use crate::overlays::{keyboard::KEYBOARD_NAME, watch::WATCH_NAME};

use super::common::OverlayContainer;

pub struct OscSender {
    last_sent: Instant,
    upstream: UdpSocket,
}

impl OscSender {
    pub fn new(send_port: u16) -> anyhow::Result<Self> {
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);

        let Ok(upstream) = UdpSocket::bind("0.0.0.0:0") else {
            bail!("Failed to bind UDP socket - OSC will not function.");
        };

        let Ok(_) = upstream.connect(SocketAddr::new(ip, send_port)) else {
            bail!("Failed to connect UDP socket - OSC will not function.");
        };

        Ok(Self {
            upstream,
            last_sent: Instant::now(),
        })
    }

    pub fn send_message(&self, addr: String, args: Vec<OscType>) -> anyhow::Result<()> {
        let packet = OscPacket::Message(OscMessage { addr, args });
        let Ok(bytes) = rosc::encoder::encode(&packet) else {
            bail!("Could not encode OSC packet.");
        };

        let Ok(_) = self.upstream.send(&bytes) else {
            bail!("Could not send OSC packet.");
        };

        Ok(())
    }

    pub fn send_params<D>(&mut self, overlays: &OverlayContainer<D>, batteries: &[f32; 9]) -> anyhow::Result<()>
    where
        D: Default,
    {
        if self.last_sent.elapsed().as_millis() < 100 {
            return Ok(());
        }

        let mut num_overlays = 0;
        let mut has_keyboard = false;
        let mut has_wrist = false;

        for o in overlays.iter() {
            if !o.state.want_visible {
                continue;
            }
            match o.state.name.as_ref() {
                WATCH_NAME => has_wrist = true,
                KEYBOARD_NAME => has_keyboard = true,
                _ => {
                    if o.state.interactable {
                        num_overlays += 1
                    }
                }
            }
        }

        self.send_message(
            "/avatar/parameters/isOverlayOpen".into(),
            vec![OscType::Bool(num_overlays > 0)],
        )?;
        self.send_message(
            "/avatar/parameters/isKeyboardOpen".into(),
            vec![OscType::Bool(has_keyboard)],
        )?;
        self.send_message(
            "/avatar/parameters/isWristVisible".into(),
            vec![OscType::Bool(has_wrist)],
        )?;
        self.send_message(
            "/avatar/parameters/openOverlayCount".into(),
            vec![OscType::Int(num_overlays)],
        )?;

        // send battery levels, as 0-1
        // assume 0,1,2 are hmd,left,right controller, anything else is a tracker
        let hmd_battery = batteries[0];
        let left_controller_battery = batteries[1];
        let right_controller_battery = batteries[2];

        // XSOverlay style
        /*/
        self.send_message(
            "/avatar/parameters/headsetBattery".into(), // this one doesn't exist, but it's a stepping stone for 0-1 values while keeping the OVR Toolkit style parameter on 0-100
                          vec![OscType::Float(hmd_battery)],
        )?;
        // */

        self.send_message(
            "/avatar/parameters/leftControllerBattery".into(),
                          vec![OscType::Float(right_controller_battery)],
        )?;
        self.send_message(
            "/avatar/parameters/rightControllerBattery".into(),
                          vec![OscType::Float(left_controller_battery)],
        )?;
        self.send_message(
            "/avatar/parameters/averageControllerBattery".into(),
                          vec![OscType::Float((right_controller_battery + left_controller_battery) / 2.0)],
        )?;

        /*/ TODO: uncomment once we have the trackers' battery info
        for i in 3..9 {
            self.send_message(
                format!("/avatar/parameters/tracker{}Battery", i).into(),
                                vec![OscType::Float(batteries[i])],
            )?;
        }
        // TODO: get average tracker battery, excluding ones with -1
        self.send_message(
            "/avatar/parameters/averageTrackerBattery".into(),
                            vec![OscType::Float(1.0)],
        )?;
        // */

        // OVR Toolkit style
        // according to their docs, OVR Toolkit is now supposed to use 0-1.
        // currently they still use 0-100, but this may change in a future update.
        self.send_message(
            "/avatar/parameters/hmdBattery".into(),
                          vec![OscType::Float(hmd_battery)],
        )?;

        Ok(())
    }
}
