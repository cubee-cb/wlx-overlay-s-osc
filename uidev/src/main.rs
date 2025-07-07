use glam::{Vec2, vec2};
use std::sync::Arc;
use testbed::{Testbed, testbed_any::TestbedAny};
use timestep::Timestep;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use vulkan::init_window;
use vulkano::{
    Validated, VulkanError,
    command_buffer::CommandBufferUsage,
    format::Format,
    image::{ImageUsage, view::ImageView},
    swapchain::{
        Surface, SurfaceInfo, Swapchain, SwapchainCreateInfo, SwapchainPresentInfo,
        acquire_next_image,
    },
    sync::GpuFuture,
};
use wgui::{
    event::{
        EventListenerCollection, MouseButtonIndex, MouseDownEvent, MouseMotionEvent, MouseUpEvent,
        MouseWheelEvent,
    },
    gfx::WGfx,
    renderer_vk::{self},
};
use winit::{
    event::{ElementState, Event, MouseScrollDelta, WindowEvent},
    event_loop::ControlFlow,
    keyboard::{KeyCode, PhysicalKey},
};

use crate::testbed::testbed_generic::TestbedGeneric;

mod assets;
mod profiler;
mod testbed;
mod timestep;
mod vulkan;

fn init_logging() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .pretty()
                .with_writer(std::io::stderr),
        )
        .with(
            /* read RUST_LOG env var */
            EnvFilter::builder()
                .with_default_directive(LevelFilter::DEBUG.into())
                .from_env_lossy(),
        )
        .init();
}

fn load_testbed(
    listeners: &mut EventListenerCollection<(), ()>,
) -> anyhow::Result<Box<dyn Testbed>> {
    let name = std::env::var("TESTBED").unwrap_or_default();
    Ok(match name.as_str() {
        "" => Box::new(TestbedGeneric::new(listeners)?),
        _ => Box::new(TestbedAny::new(&name, listeners)?),
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_logging();

    let (gfx, event_loop, window, surface) = init_window()?;
    let inner_size = window.inner_size();
    let mut swapchain_size = [inner_size.width, inner_size.height];

    let mut swapchain_create_info =
        swapchain_create_info(&gfx, gfx.surface_format, surface.clone(), swapchain_size);

    let (mut swapchain, mut images) = {
        let (swapchain, images) = Swapchain::new(
            gfx.device.clone(),
            surface.clone(),
            swapchain_create_info.clone(),
        )?;

        let image_views = images
            .into_iter()
            .map(|image| ImageView::new_default(image).unwrap())
            .collect::<Vec<_>>();

        (swapchain, image_views)
    };

    let mut recreate = false;
    let mut last_draw = std::time::Instant::now();

    let mut scale = window.scale_factor() as f32;

    let mut listeners = EventListenerCollection::default();

    let mut testbed = load_testbed(&mut listeners)?;

    let mut mouse = Vec2::ZERO;

    let mut shared_context = renderer_vk::context::SharedContext::new(gfx.clone())?;
    let mut render_context = renderer_vk::context::Context::new(&mut shared_context, scale)?;

    render_context.update_viewport(&mut shared_context, swapchain_size, scale)?;
    println!("new swapchain_size: {swapchain_size:?}");

    let mut profiler = profiler::Profiler::new(100);
    let mut frame_index: u64 = 0;

    let mut timestep = Timestep::new();
    timestep.set_tps(60.0);

    #[allow(deprecated)]
    event_loop.run(move |event, elwt| {
        elwt.set_control_flow(ControlFlow::Poll);

        match event {
            Event::WindowEvent {
                event: WindowEvent::MouseWheel { delta, .. },
                ..
            } => match delta {
                MouseScrollDelta::LineDelta(x, y) => testbed
                    .layout()
                    .push_event(
                        &mut listeners,
                        &wgui::event::Event::MouseWheel(MouseWheelEvent {
                            shift: Vec2::new(x, y),
                            pos: mouse / scale,
                            device: 0,
                        }),
                        (&mut (), &mut ()),
                    )
                    .unwrap(),
                MouseScrollDelta::PixelDelta(pos) => testbed
                    .layout()
                    .push_event(
                        &mut listeners,
                        &wgui::event::Event::MouseWheel(MouseWheelEvent {
                            shift: Vec2::new(pos.x as f32 / 5.0, pos.y as f32 / 5.0),
                            pos: mouse / scale,
                            device: 0,
                        }),
                        (&mut (), &mut ()),
                    )
                    .unwrap(),
            },
            Event::WindowEvent {
                event: WindowEvent::MouseInput { state, button, .. },
                ..
            } => {
                if matches!(button, winit::event::MouseButton::Left) {
                    if matches!(state, winit::event::ElementState::Pressed) {
                        testbed
                            .layout()
                            .push_event(
                                &mut listeners,
                                &wgui::event::Event::MouseDown(MouseDownEvent {
                                    pos: mouse / scale,
                                    index: MouseButtonIndex::Left,
                                    device: 0,
                                }),
                                (&mut (), &mut ()),
                            )
                            .unwrap();
                    } else {
                        testbed
                            .layout()
                            .push_event(
                                &mut listeners,
                                &wgui::event::Event::MouseUp(MouseUpEvent {
                                    pos: mouse / scale,
                                    index: MouseButtonIndex::Left,
                                    device: 0,
                                }),
                                (&mut (), &mut ()),
                            )
                            .unwrap();
                    }
                }
            }
            Event::WindowEvent {
                event: WindowEvent::CursorMoved { position, .. },
                ..
            } => {
                mouse = vec2(position.x as _, position.y as _);
                testbed
                    .layout()
                    .push_event(
                        &mut listeners,
                        &wgui::event::Event::MouseMotion(MouseMotionEvent {
                            pos: mouse / scale,
                            device: 0,
                        }),
                        (&mut (), &mut ()),
                    )
                    .unwrap();
            }
            Event::WindowEvent {
                event: WindowEvent::KeyboardInput { event, .. },
                ..
            } => {
                if event.state == ElementState::Pressed {
                    if event.physical_key == PhysicalKey::Code(KeyCode::Equal) {
                        scale *= 1.25;
                        render_context
                            .update_viewport(&mut shared_context, swapchain_size, scale)
                            .unwrap();
                    }

                    if event.physical_key == PhysicalKey::Code(KeyCode::Minus) {
                        scale *= 0.75;
                        render_context
                            .update_viewport(&mut shared_context, swapchain_size, scale)
                            .unwrap();
                    }
                }
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                elwt.exit();
            }
            Event::WindowEvent {
                event: WindowEvent::Resized(_),
                ..
            } => {
                recreate = true;
            }
            Event::WindowEvent {
                event: WindowEvent::RedrawRequested,
                ..
            } => {
                if recreate {
                    let inner_size = window.inner_size();
                    swapchain_size = [inner_size.width, inner_size.height];

                    swapchain_create_info.image_extent = swapchain_size;

                    (swapchain, images) = {
                        let (swapchain, images) =
                            swapchain.recreate(swapchain_create_info.clone()).unwrap();

                        let image_views = images
                            .into_iter()
                            .map(|image| ImageView::new_default(image).unwrap())
                            .collect::<Vec<_>>();

                        (swapchain, image_views)
                    };

                    render_context
                        .update_viewport(&mut shared_context, swapchain_size, scale)
                        .unwrap();

                    println!("new swapchain_size: {swapchain_size:?}");
                    recreate = false;
                    window.request_redraw();
                }

                while timestep.on_tick() {
                    testbed.layout().tick().unwrap();
                }

                testbed
                    .update(
                        (swapchain_size[0] as f32 / scale) as _,
                        (swapchain_size[1] as f32 / scale) as _,
                        timestep.alpha,
                    )
                    .unwrap();

                if !render_context.dirty && !testbed.layout().check_toggle_needs_redraw() {
                    // no need to redraw
                    std::thread::sleep(std::time::Duration::from_millis(5)); // dirty fix to prevent cpu burning precious cycles doing a busy loop
                    return;
                }

                log::trace!("drawing frame {frame_index}");
                frame_index += 1;

                profiler.start();

                {
                    let (image_index, _, acquire_future) = match acquire_next_image(
                        swapchain.clone(),
                        None,
                    )
                    .map_err(Validated::unwrap)
                    {
                        Ok(r) => r,
                        Err(VulkanError::OutOfDate) => {
                            recreate = true;
                            return;
                        }
                        Err(e) => panic!("failed to acquire next image: {e}"),
                    };

                    let tgt = images[image_index as usize].clone();

                    last_draw = std::time::Instant::now();

                    let mut cmd_buf = gfx
                        .create_gfx_command_buffer(CommandBufferUsage::OneTimeSubmit)
                        .unwrap();
                    cmd_buf.begin_rendering(tgt).unwrap();

                    let primitives = wgui::drawing::draw(testbed.layout()).unwrap();
                    render_context
                        .draw(&mut shared_context, &mut cmd_buf, &primitives)
                        .unwrap();

                    cmd_buf.end_rendering().unwrap();

                    let cmd_buf = cmd_buf.build().unwrap();

                    acquire_future
                        .then_execute(gfx.queue_gfx.clone(), cmd_buf)
                        .unwrap()
                        .then_swapchain_present(
                            gfx.queue_gfx.clone(),
                            SwapchainPresentInfo::swapchain_image_index(
                                swapchain.clone(),
                                image_index,
                            ),
                        )
                        .then_signal_fence_and_flush()
                        .unwrap()
                        .wait(None)
                        .unwrap();
                }

                profiler.end();
            }
            Event::AboutToWait => {
                if last_draw.elapsed().as_millis() > 16 {
                    window.request_redraw();
                }
            }
            _ => (),
        }
    })?;

    Ok(())
}

fn swapchain_create_info(
    graphics: &WGfx,
    format: Format,
    surface: Arc<Surface>,
    extent: [u32; 2],
) -> SwapchainCreateInfo {
    let surface_capabilities = graphics
        .device
        .physical_device()
        .surface_capabilities(&surface, SurfaceInfo::default())
        .unwrap(); // want panic

    SwapchainCreateInfo {
        min_image_count: surface_capabilities.min_image_count.max(2),
        image_format: format,
        image_extent: extent,
        image_usage: ImageUsage::COLOR_ATTACHMENT,
        composite_alpha: surface_capabilities
            .supported_composite_alpha
            .into_iter()
            .next()
            .unwrap(), // want panic
        ..Default::default()
    }
}
