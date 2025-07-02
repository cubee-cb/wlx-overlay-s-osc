use std::f32::consts::PI;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use ash::vk::SubmitInfo;
use glam::{Affine3A, Vec3, Vec3A, Vec4};
use idmap::IdMap;
use ovr_overlay::overlay::OverlayManager;
use ovr_overlay::sys::ETrackingUniverseOrigin;
use vulkano::{
    VulkanObject,
    command_buffer::{
        CommandBufferBeginInfo, CommandBufferLevel, CommandBufferUsage, RecordingCommandBuffer,
    },
    format::Format,
    image::view::ImageView,
    image::{Image, ImageLayout},
    sync::{
        AccessFlags, DependencyInfo, ImageMemoryBarrier, PipelineStages,
        fence::{Fence, FenceCreateInfo},
    },
};
use wgui::gfx::WGfx;

use crate::backend::input::{Haptics, PointerHit};
use crate::backend::overlay::{
    FrameMeta, OverlayBackend, OverlayData, OverlayState, ShouldRender, Z_ORDER_LINES,
};
use crate::graphics::CommandBuffers;
use crate::state::AppState;

use super::overlay::OpenVrOverlayData;

static LINE_AUTO_INCREMENT: AtomicUsize = AtomicUsize::new(1);

pub(super) struct LinePool {
    lines: IdMap<usize, OverlayData<OpenVrOverlayData>>,
    view: Arc<ImageView>,
    colors: [Vec4; 5],
}

impl LinePool {
    pub fn new(graphics: Arc<WGfx>) -> anyhow::Result<Self> {
        let mut command_buffer =
            graphics.create_xfer_command_buffer(CommandBufferUsage::OneTimeSubmit)?;

        let buf = vec![255; 16];

        let texture = command_buffer.upload_image(2, 2, Format::R8G8B8A8_UNORM, &buf)?;
        command_buffer.build_and_execute_now()?;

        transition_layout(
            &graphics,
            texture.clone(),
            ImageLayout::ShaderReadOnlyOptimal,
            ImageLayout::TransferSrcOptimal,
        )?
        .wait(None)?;

        let view = ImageView::new_default(texture)?;

        Ok(Self {
            lines: IdMap::new(),
            view,
            colors: [
                Vec4::new(1., 1., 1., 1.),
                Vec4::new(0., 0.375, 0.5, 1.),
                Vec4::new(0.69, 0.188, 0., 1.),
                Vec4::new(0.375, 0., 0.5, 1.),
                Vec4::new(1., 0., 0., 1.),
            ],
        })
    }

    pub fn allocate(&mut self) -> usize {
        let id = LINE_AUTO_INCREMENT.fetch_add(1, Ordering::Relaxed);

        let mut data = OverlayData::<OpenVrOverlayData> {
            state: OverlayState {
                name: Arc::from(format!("wlx-line{id}")),
                show_hide: true,
                ..Default::default()
            },
            data: OpenVrOverlayData {
                width: 0.002,
                override_width: true,
                image_view: Some(self.view.clone()),
                image_dirty: true,
                ..Default::default()
            },
            ..OverlayData::from_backend(Box::new(LineBackend {
                view: self.view.clone(),
            }))
        };
        data.state.z_order = Z_ORDER_LINES;
        data.state.dirty = true;

        self.lines.insert(id, data);
        id
    }

    pub fn draw_from(
        &mut self,
        id: usize,
        mut from: Affine3A,
        len: f32,
        color: usize,
        hmd: &Affine3A,
    ) {
        let rotation = Affine3A::from_axis_angle(Vec3::X, -PI * 0.5);

        from.translation += from.transform_vector3a(Vec3A::NEG_Z) * (len * 0.5);
        let mut transform = from * rotation * Affine3A::from_scale(Vec3::new(1., len / 0.002, 1.));

        let to_hmd = hmd.translation - from.translation;
        let sides = [Vec3A::Z, Vec3A::X, Vec3A::NEG_Z, Vec3A::NEG_X];
        let rotations = [
            Affine3A::IDENTITY,
            Affine3A::from_axis_angle(Vec3::Y, PI * 0.5),
            Affine3A::from_axis_angle(Vec3::Y, PI * 1.0),
            Affine3A::from_axis_angle(Vec3::Y, PI * 1.5),
        ];
        let mut closest = (0, 0.0);
        for (i, &side) in sides.iter().enumerate() {
            let dot = to_hmd.dot(transform.transform_vector3a(side));
            if i == 0 || dot > closest.1 {
                closest = (i, dot);
            }
        }

        transform *= rotations[closest.0];

        debug_assert!(color < self.colors.len());

        self.draw_transform(id, transform, self.colors[color]);
    }

    fn draw_transform(&mut self, id: usize, transform: Affine3A, color: Vec4) {
        if let Some(data) = self.lines.get_mut(id) {
            data.state.want_visible = true;
            data.state.transform = transform;
            data.data.color = color;
        } else {
            log::warn!("Line {id} does not exist");
        }
    }

    pub fn update(
        &mut self,
        universe: ETrackingUniverseOrigin,
        overlay: &mut OverlayManager,
        app: &mut AppState,
    ) -> anyhow::Result<()> {
        for data in self.lines.values_mut() {
            data.after_input(overlay, app)?;
            if data.state.want_visible {
                if data.state.dirty {
                    data.upload_texture(overlay, &app.gfx);
                    data.state.dirty = false;
                }

                data.upload_transform(universe.clone(), overlay);
                data.upload_color(overlay);
            }
        }
        Ok(())
    }

    pub fn mark_dirty(&mut self) {
        for data in self.lines.values_mut() {
            data.state.dirty = true;
        }
    }
}

struct LineBackend {
    view: Arc<ImageView>,
}

impl OverlayBackend for LineBackend {
    fn init(&mut self, _: &mut AppState) -> anyhow::Result<()> {
        Ok(())
    }
    fn pause(&mut self, _: &mut AppState) -> anyhow::Result<()> {
        Ok(())
    }
    fn resume(&mut self, _: &mut AppState) -> anyhow::Result<()> {
        Ok(())
    }
    fn should_render(&mut self, _: &mut AppState) -> anyhow::Result<ShouldRender> {
        Ok(ShouldRender::Unable)
    }
    fn render(
        &mut self,
        _: &mut AppState,
        _: Arc<ImageView>,
        _: &mut CommandBuffers,
        _: f32,
    ) -> anyhow::Result<bool> {
        Ok(false)
    }
    fn frame_meta(&mut self) -> Option<FrameMeta> {
        Some(FrameMeta {
            extent: self.view.image().extent(),
            ..Default::default()
        })
    }

    fn on_hover(&mut self, _: &mut AppState, _: &PointerHit) -> Option<Haptics> {
        None
    }
    fn on_left(&mut self, _: &mut AppState, _: usize) {}
    fn on_pointer(&mut self, _: &mut AppState, _: &PointerHit, _: bool) {}
    fn on_scroll(&mut self, _: &mut AppState, _: &PointerHit, _: f32, _: f32) {}
    fn get_interaction_transform(&mut self) -> Option<glam::Affine2> {
        None
    }
}

pub fn transition_layout(
    gfx: &WGfx,
    image: Arc<Image>,
    old_layout: ImageLayout,
    new_layout: ImageLayout,
) -> anyhow::Result<Fence> {
    let barrier = ImageMemoryBarrier {
        src_stages: PipelineStages::ALL_TRANSFER,
        src_access: AccessFlags::TRANSFER_WRITE,
        dst_stages: PipelineStages::ALL_TRANSFER,
        dst_access: AccessFlags::TRANSFER_READ,
        old_layout,
        new_layout,
        subresource_range: image.subresource_range(),
        ..ImageMemoryBarrier::image(image)
    };

    let command_buffer = unsafe {
        let mut builder = RecordingCommandBuffer::new(
            gfx.command_buffer_allocator.clone(),
            gfx.queue_gfx.queue_family_index(),
            CommandBufferLevel::Primary,
            CommandBufferBeginInfo {
                usage: CommandBufferUsage::OneTimeSubmit,
                inheritance_info: None,
                ..Default::default()
            },
        )?;

        builder.pipeline_barrier(&DependencyInfo {
            image_memory_barriers: smallvec::smallvec![barrier],
            ..Default::default()
        })?;
        builder.end()?
    };

    let fence = Fence::new(gfx.device.clone(), FenceCreateInfo::default())?;

    let fns = gfx.device.fns();
    unsafe {
        (fns.v1_0.queue_submit)(
            gfx.queue_gfx.handle(),
            1,
            [SubmitInfo::default().command_buffers(&[command_buffer.handle()])].as_ptr(),
            fence.handle(),
        )
    }
    .result()?;

    Ok(fence)
}
