use std::{cell::RefCell, rc::Rc, sync::Arc};

use cosmic_text::Buffer;
use glam::{Mat4, Vec2, Vec3};
use slotmap::{SlotMap, new_key_type};

use crate::{
	drawing,
	gfx::{WGfx, cmd::GfxCommandBuffer},
};

use super::{
	rect::{RectPipeline, RectRenderer},
	text::{
		DEFAULT_METRICS, FONT_SYSTEM, SWASH_CACHE, TextArea, TextBounds,
		text_atlas::{TextAtlas, TextPipeline},
		text_renderer::TextRenderer,
	},
	viewport::Viewport,
};

struct RendererPass<'a> {
	submitted: bool,
	text_areas: Vec<TextArea<'a>>,
	text_renderer: TextRenderer,
	rect_renderer: RectRenderer,
}

impl RendererPass<'_> {
	fn new(text_atlas: &mut TextAtlas, rect_pipeline: RectPipeline) -> anyhow::Result<Self> {
		let text_renderer = TextRenderer::new(text_atlas)?;
		let rect_renderer = RectRenderer::new(rect_pipeline)?;

		Ok(Self {
			submitted: false,
			text_renderer,
			rect_renderer,
			text_areas: Vec::new(),
		})
	}

	fn submit(
		&mut self,
		gfx: &Arc<WGfx>,
		viewport: &mut Viewport,
		cmd_buf: &mut GfxCommandBuffer,
		text_atlas: &mut TextAtlas,
	) -> anyhow::Result<()> {
		if self.submitted {
			return Ok(());
		}
		self.submitted = true;
		self.rect_renderer.render(gfx, viewport, cmd_buf)?;

		{
			let mut font_system = FONT_SYSTEM.lock();
			let mut swash_cache = SWASH_CACHE.lock();

			self.text_renderer.prepare(
				&mut font_system,
				text_atlas,
				viewport,
				std::mem::take(&mut self.text_areas),
				&mut swash_cache,
			)?;
		}

		self.text_renderer.render(text_atlas, viewport, cmd_buf)?;

		Ok(())
	}
}

new_key_type! {
	struct SharedContextKey;
}

pub struct SharedContext {
	gfx: Arc<WGfx>,
	atlas_map: SlotMap<SharedContextKey, SharedAtlas>,
	rect_pipeline: RectPipeline,
	text_pipeline: TextPipeline,
}

impl SharedContext {
	pub fn new(gfx: Arc<WGfx>) -> anyhow::Result<Self> {
		let rect_pipeline = RectPipeline::new(gfx.clone(), gfx.surface_format)?;
		let text_pipeline = TextPipeline::new(gfx.clone(), gfx.surface_format)?;

		Ok(Self {
			gfx,
			atlas_map: SlotMap::with_key(),
			rect_pipeline,
			text_pipeline,
		})
	}

	fn atlas_for_pixel_scale(&mut self, pixel_scale: f32) -> anyhow::Result<SharedContextKey> {
		for (key, atlas) in &self.atlas_map {
			if (atlas.pixel_scale - pixel_scale).abs() < f32::EPSILON {
				return Ok(key);
			}
		}
		log::debug!("Initializing SharedAtlas for pixel scale {pixel_scale:.2}");
		let text_atlas = TextAtlas::new(self.text_pipeline.clone())?;
		Ok(self.atlas_map.insert(SharedAtlas {
			text_atlas,
			pixel_scale,
		}))
	}
}

struct SharedAtlas {
	text_atlas: TextAtlas,
	pixel_scale: f32,
}

pub struct Context {
	viewport: Viewport,
	shared_ctx_key: SharedContextKey,
	pub dirty: bool,
	pixel_scale: f32,
	empty_text: Rc<RefCell<Buffer>>,
}

impl Context {
	pub fn new(shared: &mut SharedContext, pixel_scale: f32) -> anyhow::Result<Self> {
		let viewport = Viewport::new(&shared.gfx)?;
		let shared_ctx_key = shared.atlas_for_pixel_scale(pixel_scale)?;

		Ok(Self {
			viewport,
			shared_ctx_key,
			pixel_scale,
			dirty: true,
			empty_text: Rc::new(RefCell::new(Buffer::new_empty(DEFAULT_METRICS))),
		})
	}

	pub fn update_viewport(
		&mut self,
		shared: &mut SharedContext,
		resolution: [u32; 2],
		pixel_scale: f32,
	) -> anyhow::Result<()> {
		if (self.pixel_scale - pixel_scale).abs() > f32::EPSILON {
			self.pixel_scale = pixel_scale;
			self.shared_ctx_key = shared.atlas_for_pixel_scale(pixel_scale)?;
		}

		if self.viewport.resolution() != resolution {
			self.dirty = true;
		}

		let size = Vec2::new(
			resolution[0] as f32 / pixel_scale,
			resolution[1] as f32 / pixel_scale,
		);

		let fov = 0.4;
		let aspect_ratio = size.x / size.y;
		let projection = Mat4::perspective_rh(fov, aspect_ratio, 1.0, 100_000.0);

		let b = size.y / 2.0;
		let angle_half = fov / 2.0;
		let distance = (std::f32::consts::PI / 2.0 - angle_half).tan() * b;

		let view = Mat4::look_at_rh(
			Vec3::new(size.x / 2.0, size.y / 2.0, distance),
			Vec3::new(size.x / 2.0, size.y / 2.0, 0.0),
			Vec3::new(0.0, 1.0, 0.0),
		);

		let fin = projection * view;

		self.viewport.update(resolution, &fin, pixel_scale)?;
		Ok(())
	}

	pub fn draw(
		&mut self,
		shared: &mut SharedContext,
		cmd_buf: &mut GfxCommandBuffer,
		primitives: &[drawing::RenderPrimitive],
	) -> anyhow::Result<()> {
		self.dirty = false;

		let atlas = shared.atlas_map.get_mut(self.shared_ctx_key).unwrap();

		let mut passes = vec![RendererPass::new(
			&mut atlas.text_atlas,
			shared.rect_pipeline.clone(),
		)?];

		for primitive in primitives {
			let pass = passes.last_mut().unwrap(); // always safe

			match &primitive.payload {
				drawing::PrimitivePayload::Rectangle(rectangle) => {
					pass.rect_renderer.add_rect(
						primitive.boundary,
						*rectangle,
						&primitive.transform,
						primitive.depth,
					);
				}
				drawing::PrimitivePayload::Text(text) => {
					pass.text_areas.push(TextArea {
						buffer: text.clone(),
						left: primitive.boundary.pos.x * self.pixel_scale,
						top: primitive.boundary.pos.y * self.pixel_scale,
						bounds: TextBounds::default(), //FIXME: just using boundary coords here doesn't work
						scale: self.pixel_scale,
						default_color: cosmic_text::Color::rgb(0, 0, 0),
						custom_glyphs: &[],
						depth: primitive.depth,
						transform: primitive.transform,
					});
				}
				drawing::PrimitivePayload::Sprite(sprites) => {
					pass.text_areas.push(TextArea {
						buffer: self.empty_text.clone(),
						left: primitive.boundary.pos.x * self.pixel_scale,
						top: primitive.boundary.pos.y * self.pixel_scale,
						bounds: TextBounds::default(),
						scale: self.pixel_scale,
						custom_glyphs: sprites.as_slice(),
						default_color: cosmic_text::Color::rgb(255, 0, 255),
						depth: primitive.depth,
						transform: primitive.transform,
					});
				}
			}
		}

		let pass = passes.last_mut().unwrap();
		pass.submit(
			&shared.gfx,
			&mut self.viewport,
			cmd_buf,
			&mut atlas.text_atlas,
		)?;

		Ok(())
	}
}
