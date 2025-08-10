use std::rc::Rc;

use crate::{any::AnyTrait, event::EventAlterables, layout::LayoutState};

pub mod button;
pub mod slider;

pub struct InitData<'a> {
	pub state: &'a LayoutState,
	pub alterables: &'a mut EventAlterables,
}

pub trait ComponentTrait: AnyTrait {
	fn init(&self, data: &mut InitData);
}

#[derive(Clone)]
pub struct Component(pub Rc<dyn ComponentTrait>);

pub type ComponentWeak = std::rc::Weak<dyn ComponentTrait>;

impl Component {
	pub fn weak(&self) -> ComponentWeak {
		Rc::downgrade(&self.0)
	}

	pub fn try_cast<T: 'static>(&self) -> anyhow::Result<Rc<T>> {
		if !(*self.0).as_any().is::<T>() {
			anyhow::bail!("try_cast: type not matching");
		}

		// safety: we already checked it above, should be safe to directly cast it
		unsafe { Ok(Rc::from_raw(Rc::into_raw(self.0.clone()) as _)) }
	}
}
