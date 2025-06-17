use std::any::Any;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;

use ashpd::desktop::{
    screencast::{CursorMode, Screencast, SourceType},
    PersistMode,
};

pub use ashpd::Error as AshpdError;

use pipewire as pw;
use pw::spa;

use pw::properties::properties;
use pw::stream::{Stream, StreamFlags};
use pw::{context::Context, main_loop::MainLoop, Error};
use spa::buffer::DataType;
use spa::buffer::MetaData;
use spa::buffer::MetaType;
use spa::param::video::VideoFormat;
use spa::param::video::VideoInfoRaw;
use spa::param::ParamType;
use spa::pod::serialize::GenError;
use spa::pod::ChoiceValue;
use spa::pod::Pod;
use spa::pod::{Object, Property, PropertyFlags, Value};
use spa::utils::Choice;
use spa::utils::ChoiceEnum;
use spa::utils::ChoiceFlags;

use crate::frame::DrmFormat;
use crate::frame::FourCC;
use crate::frame::FrameFormat;
use crate::frame::MouseMeta;
use crate::frame::Transform;
use crate::frame::WlxFrame;
use crate::frame::DRM_FORMAT_ABGR2101010;
use crate::frame::DRM_FORMAT_ABGR8888;
use crate::frame::DRM_FORMAT_ARGB8888;
use crate::frame::DRM_FORMAT_XBGR2101010;
use crate::frame::DRM_FORMAT_XBGR8888;
use crate::frame::DRM_FORMAT_XRGB8888;
use crate::frame::{DmabufFrame, FramePlane, MemFdFrame, MemPtrFrame};
use crate::WlxCapture;

pub struct PipewireStream {
    pub node_id: u32,
    pub position: Option<(i32, i32)>,
    pub size: Option<(i32, i32)>,
}

pub struct PipewireSelectScreenResult {
    pub streams: Vec<PipewireStream>,
    pub restore_token: Option<String>,
}

pub async fn pipewire_select_screen(
    token: Option<&str>,
    embed_mouse: bool,
    screens_only: bool,
    persist: bool,
    multiple: bool,
) -> Result<PipewireSelectScreenResult, AshpdError> {
    static CURSOR_MODES: AtomicU32 = AtomicU32::new(0);

    let proxy = Screencast::new().await?;
    let session = proxy.create_session().await?;

    let mut cursor_modes = CURSOR_MODES.load(Ordering::Relaxed);
    if cursor_modes == 0 {
        cursor_modes = proxy.get_property::<u32>("AvailableCursorModes").await?;

        log::debug!("Available cursor modes: {cursor_modes:#x}");

        // propery will be same system-wide, so race condition not a concern
        CURSOR_MODES.store(cursor_modes, Ordering::Relaxed);
    }

    let cursor_mode = match embed_mouse {
        true if cursor_modes & (CursorMode::Embedded as u32) != 0 => CursorMode::Embedded,
        _ if cursor_modes & (CursorMode::Metadata as u32) != 0 => CursorMode::Metadata,
        _ => CursorMode::Hidden,
    };

    log::debug!("Selected cursor mode: {cursor_mode:?}");

    let source_type = if screens_only {
        SourceType::Monitor.into()
    } else {
        SourceType::Monitor | SourceType::Window | SourceType::Virtual
    };

    let persist_mode = if persist {
        PersistMode::ExplicitlyRevoked
    } else {
        PersistMode::DoNot
    };

    proxy
        .select_sources(
            &session,
            cursor_mode,
            source_type,
            multiple,
            token,
            persist_mode,
        )
        .await?;

    let response = proxy.start(&session, None).await?.response()?;

    let streams: Vec<_> = response
        .streams()
        .iter()
        .map(|stream| PipewireStream {
            node_id: stream.pipe_wire_node_id(),
            position: stream.position(),
            size: stream.size(),
        })
        .collect();
    if !streams.is_empty() {
        return Ok(PipewireSelectScreenResult {
            streams,
            restore_token: response.restore_token().map(String::from),
        });
    }

    Err(ashpd::Error::NoResponse)
}

#[derive(Default)]
struct StreamData {
    format: Option<FrameFormat>,
    stream: Option<Stream>,
}

pub enum PwChangeRequest {
    Pause,
    Resume,
    Stop,
}

struct CaptureData<R>
where
    R: Any + Send,
{
    tx_ctrl: pw::channel::Sender<PwChangeRequest>,
    rx_frame: mpsc::Receiver<R>,
}

pub struct PipewireCapture<R>
where
    R: Any + Send,
{
    name: Arc<str>,
    data: Option<CaptureData<R>>,
    node_id: u32,
    handle: Option<JoinHandle<Result<(), Error>>>,
}

impl<R> PipewireCapture<R>
where
    R: Any + Send,
{
    pub fn new(name: Arc<str>, node_id: u32) -> Self {
        PipewireCapture {
            name,
            data: None,
            node_id,
            handle: None,
        }
    }
}

impl<R> Drop for PipewireCapture<R>
where
    R: Any + Send,
{
    fn drop(&mut self) {
        if let Some(data) = &self.data {
            let _ = data.tx_ctrl.send(PwChangeRequest::Stop);
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl<U, R> WlxCapture<U, R> for PipewireCapture<R>
where
    U: Any + Send,
    R: Any + Send,
{
    fn init(
        &mut self,
        dmabuf_formats: &[DrmFormat],
        user_data: U,
        receive_callback: fn(&U, WlxFrame) -> Option<R>,
    ) {
        let (tx_frame, rx_frame) = mpsc::sync_channel(2);
        let (tx_ctrl, rx_ctrl) = pw::channel::channel();

        self.data = Some(CaptureData { tx_ctrl, rx_frame });

        self.handle = Some(std::thread::spawn({
            let name = self.name.clone();
            let node_id = self.node_id;
            let formats = dmabuf_formats.to_vec();

            move || {
                main_loop::<U, R>(
                    name,
                    node_id,
                    formats,
                    tx_frame,
                    rx_ctrl,
                    user_data,
                    receive_callback,
                )
            }
        }));
    }
    fn is_ready(&self) -> bool {
        self.data.is_some()
    }
    fn supports_dmbuf(&self) -> bool {
        true
    }
    fn receive(&mut self) -> Option<R> {
        if let Some(data) = self.data.as_ref() {
            return data.rx_frame.try_iter().last();
        }
        None
    }
    fn pause(&mut self) {
        if let Some(data) = &self.data {
            match data.tx_ctrl.send(PwChangeRequest::Pause) {
                Ok(_) => (),
                Err(_) => {
                    log::warn!("{}: disconnected, stopping stream", &self.name);
                }
            }
        }
    }
    fn resume(&mut self) {
        if let Some(data) = &self.data {
            match data.tx_ctrl.send(PwChangeRequest::Resume) {
                Ok(_) => {
                    log::debug!(
                        "{}: dropped {} old frames before resuming",
                        &self.name,
                        data.rx_frame.try_iter().count()
                    );
                }
                Err(_) => {
                    log::warn!("{}: disconnected, stopping stream", &self.name);
                }
            }
        }
    }
    fn request_new_frame(&mut self) {}
}

fn main_loop<U, R>(
    name: Arc<str>,
    node_id: u32,
    dmabuf_formats: Vec<DrmFormat>,
    sender: mpsc::SyncSender<R>,
    receiver: pw::channel::Receiver<PwChangeRequest>,
    user_data: U,
    receive_callback: fn(&U, WlxFrame) -> Option<R>,
) -> Result<(), Error>
where
    U: Any,
    R: Any,
{
    let main_loop = MainLoop::new(None)?;
    let context = Context::new(&main_loop)?;
    let core = context.connect(None)?;

    let stream = Stream::new(
        &core,
        &name,
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )?;

    let _listener = stream
        .add_local_listener_with_user_data(FrameFormat::default())
        .state_changed({
            let name = name.clone();
            move |_, _, old, new| {
                log::info!("{}: stream state changed: {:?} -> {:?}", &name, old, new);
            }
        })
        .param_changed({
            let name = name.clone();
            move |stream, format, id, param| {
                let Some(param) = param else {
                    return;
                };
                if id != ParamType::Format.as_raw() {
                    return;
                }

                let mut info = VideoInfoRaw::default();
                info.parse(param)
                    .expect("Failed to parse param changed to VideoInfoRaw");

                format.width = info.size().width;
                format.height = info.size().height;
                format.fourcc = spa_to_fourcc(info.format());
                format.modifier = info.modifier();

                let kind = if format.modifier != 0 {
                    "DMA-buf"
                } else {
                    "SHM"
                };

                log::info!("{}: got {} video format:", &name, &kind);
                log::info!("  format: {} ({:?})", info.format().as_raw(), info.format());
                log::info!("  size: {}x{}", info.size().width, info.size().height);
                log::info!("  modifier: {}", info.modifier());
                let Ok(params_bytes) = obj_to_bytes(get_buffer_params()) else {
                    log::warn!("{}: failed to serialize buffer params", &name);
                    return;
                };
                let Some(params_pod) = Pod::from_bytes(&params_bytes) else {
                    log::warn!("{}: failed to deserialize buffer params", &name);
                    return;
                };

                let header_bytes = obj_to_bytes(get_meta_object(
                    spa::sys::SPA_META_Header,
                    std::mem::size_of::<spa::sys::spa_meta_header>(),
                ))
                .unwrap(); // want panic
                let header_pod = Pod::from_bytes(&header_bytes).unwrap(); // want panic

                let xform_bytes = obj_to_bytes(get_meta_object(
                    spa::sys::SPA_META_VideoTransform,
                    std::mem::size_of::<spa::sys::spa_meta_videotransform>(),
                ))
                .unwrap(); // want panic
                let xform_pod = Pod::from_bytes(&xform_bytes).unwrap(); // want panic

                let mut pods = [params_pod, header_pod, xform_pod];
                if let Err(e) = stream.update_params(&mut pods) {
                    log::error!("{}: failed to update params: {}", &name, e);
                }
            }
        })
        .process({
            let name = name.clone();
            let u = user_data;
            move |stream, format| {
                let mut maybe_buffer = None;
                // discard all but the newest frame
                while let Some(buffer) = stream.dequeue_buffer() {
                    maybe_buffer = Some(buffer);
                }

                if let Some(mut buffer) = maybe_buffer {
                    if let MetaData::Header(header) = buffer.find_meta_data(MetaType::Header) {
                        if header.flags & spa::sys::SPA_META_HEADER_FLAG_CORRUPTED != 0 {
                            log::warn!("{}: PipeWire buffer is corrupt.", &name);
                            return;
                        }
                    }

                    if let MetaData::VideoTransform(transform) =
                        buffer.find_meta_data(MetaType::VideoTransform)
                    {
                        format.transform = match transform.transform {
                            spa::sys::SPA_META_TRANSFORMATION_None => Transform::Normal,
                            spa::sys::SPA_META_TRANSFORMATION_90 => Transform::Rotated90,
                            spa::sys::SPA_META_TRANSFORMATION_180 => Transform::Rotated180,
                            spa::sys::SPA_META_TRANSFORMATION_270 => Transform::Rotated270,
                            spa::sys::SPA_META_TRANSFORMATION_Flipped => Transform::Flipped,
                            spa::sys::SPA_META_TRANSFORMATION_Flipped90 => Transform::Flipped90,
                            spa::sys::SPA_META_TRANSFORMATION_Flipped180 => Transform::Flipped180,
                            spa::sys::SPA_META_TRANSFORMATION_Flipped270 => Transform::Flipped270,
                            _ => Transform::Undefined,
                        };
                        log::debug!("{}: Transform: {:?}", &name, &format.transform);
                    }

                    let mouse_meta = match buffer.find_meta_data(MetaType::Cursor) {
                        MetaData::Cursor(cursor) if cursor.id != 0 => Some(MouseMeta {
                            x: cursor.position.x as f32 / format.width as f32,
                            y: cursor.position.y as f32 / format.height as f32,
                        }),
                        _ => None,
                    };

                    let datas = buffer.datas_mut();
                    if datas.is_empty() {
                        log::debug!("{}: no data", &name);
                        return;
                    }

                    let planes: Vec<FramePlane> = datas
                        .iter()
                        .map(|p| FramePlane {
                            fd: Some(p.as_raw().fd as _),
                            offset: p.chunk().offset(),
                            stride: p.chunk().stride(),
                        })
                        .collect();

                    match datas[0].type_() {
                        DataType::DmaBuf => {
                            let mut dmabuf = DmabufFrame {
                                format: *format,
                                num_planes: planes.len(),
                                mouse: mouse_meta,
                                ..Default::default()
                            };
                            dmabuf.planes[..planes.len()].copy_from_slice(&planes[..planes.len()]);

                            let frame = WlxFrame::Dmabuf(dmabuf);

                            if let Some(r) = receive_callback(&u, frame) {
                                match sender.try_send(r) {
                                    Ok(_) => (),
                                    Err(mpsc::TrySendError::Full(_)) => (),
                                    Err(mpsc::TrySendError::Disconnected(_)) => {
                                        log::warn!("{}: disconnected, stopping stream", &name);
                                        let _ = stream.disconnect();
                                    }
                                }
                            }
                        }
                        DataType::MemFd => {
                            let memfd = MemFdFrame {
                                format: *format,
                                plane: FramePlane {
                                    fd: Some(datas[0].as_raw().fd as _),
                                    offset: datas[0].chunk().offset(),
                                    stride: datas[0].chunk().stride(),
                                },
                                mouse: mouse_meta,
                            };

                            let frame = WlxFrame::MemFd(memfd);
                            if let Some(r) = receive_callback(&u, frame) {
                                match sender.try_send(r) {
                                    Ok(_) => (),
                                    Err(mpsc::TrySendError::Full(_)) => (),
                                    Err(mpsc::TrySendError::Disconnected(_)) => {
                                        log::warn!("{}: disconnected, stopping stream", &name);
                                        let _ = stream.disconnect();
                                    }
                                }
                            }
                        }
                        DataType::MemPtr => {
                            let memptr = MemPtrFrame {
                                format: *format,
                                ptr: datas[0].as_raw().data as _,
                                size: datas[0].chunk().size() as _,
                                mouse: mouse_meta,
                            };

                            let frame = WlxFrame::MemPtr(memptr);
                            if let Some(r) = receive_callback(&u, frame) {
                                match sender.try_send(r) {
                                    Ok(_) => (),
                                    Err(mpsc::TrySendError::Full(_)) => (),
                                    Err(mpsc::TrySendError::Disconnected(_)) => {
                                        log::warn!("{}: disconnected, stopping stream", &name);
                                        let _ = stream.disconnect();
                                    }
                                }
                            }
                        }
                        _ => {
                            log::error!("Received invalid frame data type ({:?})", datas[0].type_())
                        }
                    }
                }
            }
        })
        .register()?;

    let mut format_params: Vec<Vec<u8>> = dmabuf_formats
        .iter()
        .filter_map(|f| obj_to_bytes(get_format_params(Some(f))).ok())
        .collect();

    format_params.push(obj_to_bytes(get_format_params(None)).unwrap()); // safe unwrap: known
                                                                        // good values

    let mut params: Vec<&Pod> = format_params
        .iter()
        .filter_map(|bytes| Pod::from_bytes(bytes))
        .collect();

    stream.connect(
        spa::utils::Direction::Input,
        Some(node_id),
        StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
        params.as_mut_slice(),
    )?;

    let _receiver = receiver.attach(main_loop.loop_(), {
        let name = name.clone();
        let main_loop = main_loop.clone();
        move |req| match req {
            PwChangeRequest::Pause => {
                let _ = stream.set_active(false);
            }
            PwChangeRequest::Resume => {
                let _ = stream.set_active(true);
            }
            PwChangeRequest::Stop => {
                main_loop.quit();
                log::info!("{}: stopping pipewire loop", &name);
            }
        }
    });

    main_loop.run();
    log::info!("{}: pipewire loop exited", &name);
    Ok::<(), Error>(())
}

fn obj_to_bytes(obj: spa::pod::Object) -> Result<Vec<u8>, GenError> {
    Ok(spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(obj),
    )?
    .0
    .into_inner())
}

fn get_buffer_params() -> Object {
    let data_types = (1 << DataType::MemFd.as_raw())
        | (1 << DataType::MemPtr.as_raw())
        | (1 << DataType::DmaBuf.as_raw());

    let property = Property {
        key: spa::sys::SPA_PARAM_BUFFERS_dataType,
        flags: PropertyFlags::empty(),
        value: Value::Int(data_types),
    };

    spa::pod::object!(
        spa::utils::SpaTypes::ObjectParamBuffers,
        spa::param::ParamType::Buffers,
        property,
    )
}

fn get_meta_object(key: u32, size: usize) -> Object {
    let meta_type_property = Property {
        key: spa::sys::SPA_PARAM_META_type,
        flags: PropertyFlags::empty(),
        value: Value::Id(spa::utils::Id(key)),
    };

    let meta_size_property = Property {
        key: spa::sys::SPA_PARAM_META_size,
        flags: PropertyFlags::empty(),
        value: Value::Int(size as i32),
    };

    spa::pod::object!(
        spa::utils::SpaTypes::ObjectParamMeta,
        spa::param::ParamType::Meta,
        meta_type_property,
        meta_size_property,
    )
}

fn get_format_params(fmt: Option<&DrmFormat>) -> Object {
    let mut obj = spa::pod::object!(
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaType,
            Id,
            spa::param::format::MediaType::Video
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaSubtype,
            Id,
            spa::param::format::MediaSubtype::Raw
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            spa::utils::Rectangle {
                width: 256,
                height: 256,
            },
            spa::utils::Rectangle {
                width: 1,
                height: 1,
            },
            spa::utils::Rectangle {
                width: 8192,
                height: 8192,
            }
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            spa::utils::Fraction { num: 0, denom: 1 },
            spa::utils::Fraction { num: 0, denom: 1 },
            spa::utils::Fraction {
                num: 1000,
                denom: 1
            }
        ),
    );

    if let Some(fmt) = fmt {
        let spa_fmt = fourcc_to_spa(fmt.fourcc);

        let prop = spa::pod::property!(
            spa::param::format::FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            spa_fmt,
            spa_fmt,
        );
        obj.properties.push(prop);

        // TODO rewrite when property macro supports Long
        let prop = Property {
            key: spa::param::format::FormatProperties::VideoModifier.as_raw(),
            flags: PropertyFlags::MANDATORY | PropertyFlags::DONT_FIXATE,
            value: Value::Choice(ChoiceValue::Long(Choice(
                ChoiceFlags::empty(),
                ChoiceEnum::Enum {
                    default: fmt.modifiers[0] as _,
                    alternatives: fmt.modifiers.iter().map(|m| *m as _).collect(),
                },
            ))),
        };
        obj.properties.push(prop);
    } else {
        let prop = spa::pod::property!(
            spa::param::format::FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            spa::param::video::VideoFormat::RGBA,
            spa::param::video::VideoFormat::RGBA,
            spa::param::video::VideoFormat::BGRA,
            spa::param::video::VideoFormat::RGBx,
            spa::param::video::VideoFormat::BGRx,
            spa::param::video::VideoFormat::ABGR_210LE,
            spa::param::video::VideoFormat::xBGR_210LE,
        );
        obj.properties.push(prop);
    }

    obj
}

fn fourcc_to_spa(fourcc: FourCC) -> VideoFormat {
    match fourcc.value {
        DRM_FORMAT_ARGB8888 => VideoFormat::BGRA,
        DRM_FORMAT_ABGR8888 => VideoFormat::RGBA,
        DRM_FORMAT_XRGB8888 => VideoFormat::BGRx,
        DRM_FORMAT_XBGR8888 => VideoFormat::RGBx,
        DRM_FORMAT_ABGR2101010 => VideoFormat::ABGR_210LE,
        DRM_FORMAT_XBGR2101010 => VideoFormat::xBGR_210LE,
        _ => panic!("Unsupported format"),
    }
}

#[allow(non_upper_case_globals)]
fn spa_to_fourcc(spa: VideoFormat) -> FourCC {
    match spa {
        VideoFormat::BGRA => DRM_FORMAT_ARGB8888.into(),
        VideoFormat::RGBA => DRM_FORMAT_ABGR8888.into(),
        VideoFormat::BGRx => DRM_FORMAT_XRGB8888.into(),
        VideoFormat::RGBx => DRM_FORMAT_XBGR8888.into(),
        VideoFormat::ABGR_210LE => DRM_FORMAT_ABGR2101010.into(),
        VideoFormat::xBGR_210LE => DRM_FORMAT_XBGR2101010.into(),
        _ => panic!("Unsupported format"),
    }
}
