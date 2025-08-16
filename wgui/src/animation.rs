use glam::{FloatExt, Vec2};

use crate::{
	event::{CallbackDataCommon, EventAlterables},
	layout::{LayoutState, WidgetID},
	widget::{WidgetData, WidgetObj},
};

pub enum AnimationEasing {
	Linear,
	InQuad,
	OutQuad,
	OutBack,
	InBack,
}

impl AnimationEasing {
	fn interpolate(&self, x: f32) -> f32 {
		match self {
			Self::Linear => x,
			Self::InQuad => x * x,
			Self::OutQuad => 1.0 - (1.0 - x) * (1.0 - x),
			Self::OutBack => {
				let a = 1.7;
				let b = a + 1.0;
				1.0 + b * (x - 1.0).powi(3) + a * (x - 1.0).powi(2)
			}
			Self::InBack => {
				let a = 1.7;
				let b = a + 1.0;
				b * x.powi(3) - a * x.powi(2)
			}
		}
	}
}

pub struct CallbackData<'a> {
	pub obj: &'a mut dyn WidgetObj,
	pub data: &'a mut WidgetData,
	pub widget_id: WidgetID,
	pub widget_size: Vec2,
	pub pos: f32, // 0.0 (start of animation) - 1.0 (end of animation)
}

pub type AnimationCallback = Box<dyn Fn(&mut CallbackDataCommon, &mut CallbackData)>;
pub struct Animation {
	target_widget: WidgetID,

	id: u32,
	ticks_remaining: u32,
	ticks_duration: u32,

	easing: AnimationEasing,

	pos: f32,
	pos_prev: f32,
	last_tick: bool,

	callback: AnimationCallback,
}

impl Animation {
	pub fn new(
		target_widget: WidgetID,
		ticks: u32,
		easing: AnimationEasing,
		callback: AnimationCallback,
	) -> Self {
		Self::new_ex(target_widget, 0, ticks, easing, callback)
	}

	pub fn new_ex(
		target_widget: WidgetID,
		animation_id: u32,
		ticks: u32,
		easing: AnimationEasing,
		callback: AnimationCallback,
	) -> Self {
		Self {
			target_widget,
			id: animation_id,
			callback,
			easing,
			ticks_duration: ticks,
			ticks_remaining: ticks,
			last_tick: false,
			pos: 0.0,
			pos_prev: 0.0,
		}
	}

	fn call(&self, state: &LayoutState, alterables: &mut EventAlterables, pos: f32) {
		let Some(widget) = state.widgets.get(self.target_widget).cloned() else {
			return; // failed
		};

		let widget_node = *state.nodes.get(self.target_widget).unwrap();
		let layout = state.tree.layout(widget_node).unwrap(); // should always succeed

		let mut widget_state = widget.state();
		let (data, obj) = widget_state.get_data_obj_mut();

		let data = &mut CallbackData {
			widget_id: self.target_widget,
			widget_size: Vec2::new(layout.size.width, layout.size.height),
			obj,
			data,
			pos,
		};

		let common = &mut CallbackDataCommon { state, alterables };

		(self.callback)(common, data);
	}
}

#[derive(Default)]
pub struct Animations {
	running_animations: Vec<Animation>,
}

impl Animations {
	pub fn tick(&mut self, state: &LayoutState, alterables: &mut EventAlterables) {
		for anim in &mut self.running_animations {
			let x = 1.0 - (anim.ticks_remaining as f32 / anim.ticks_duration as f32);
			let pos = if anim.ticks_remaining > 0 {
				anim.easing.interpolate(x)
			} else {
				anim.last_tick = true;
				1.0
			};

			anim.pos_prev = anim.pos;
			anim.pos = pos;
			anim.call(state, alterables, 1.0);

			if anim.last_tick {
				alterables.needs_redraw = true;
			}

			anim.ticks_remaining -= 1;
		}

		self
			.running_animations
			.retain(|anim| anim.ticks_remaining > 0);
	}

	pub fn process(&mut self, state: &LayoutState, alterables: &mut EventAlterables, alpha: f32) {
		for anim in &mut self.running_animations {
			let pos = anim.pos_prev.lerp(anim.pos, alpha);
			anim.call(state, alterables, pos);
		}
	}

	pub fn add(&mut self, anim: Animation) {
		// prevent running two animations at once
		self.stop_by_widget(anim.target_widget, Some(anim.id));
		self.running_animations.push(anim);
	}

	pub fn stop_by_widget(&mut self, widget_id: WidgetID, opt_animation_id: Option<u32>) {
		self.running_animations.retain(|anim| {
			if let Some(animation_id) = &opt_animation_id {
				if anim.target_widget == widget_id {
					anim.id != *animation_id
				} else {
					true
				}
			} else {
				anim.target_widget != widget_id
			}
		});
	}
}
