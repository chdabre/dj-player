//! The renderer: what a [`Scene`] becomes on the GPU. One wgpu pipeline per kind of shape
//! (rounded rectangles, waveforms, pictures; `rects.rs`, `waves.rs`, `images.rs`) and
//! glyphon for text (`text.rs`).
//!
//! Layout happens in design pixels (1024×600 for the device screen); the view uniform scales
//! and centres that onto the target, so the same scene fits a Retina window on the Mac and the
//! panel on the Pi.
//!
//! **Surfaces.** The device screen is one surface. A debug shell (the hardware stand-in,
//! feature `standin`) is a second one with its own design size: when it is on, the device
//! screen is rendered on its own into an offscreen texture exactly the size of the shell's
//! display slot, the shell is rendered to the window, and the device texture is copied into
//! the slot (`offscreen.rs`). The device screen never knows about the shell, so a device
//! build simply drops it.

mod images;
mod offscreen;
mod rects;
pub mod scene;
mod text;
mod waves;

use std::borrow::Cow;
use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

use images::Images;
use offscreen::Offscreen;
use rects::{RectBatch, Rects};
use scene::{Rgba, Scene};
use text::{Fonts, LayerText};
use waves::Waves;

pub const DESIGN_W: f32 = 1024.0;
pub const DESIGN_H: f32 = 600.0;

/// A debug shell around the device screen: its scene, its design size, and the slot (in its
/// design pixels) where the device screen goes.
pub struct ShellFrame<'a> {
    pub scene: &'a Scene,
    pub size: (f32, f32),
    pub display: (f32, f32, f32, f32),
}

/// How long the last frame spent where.
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameTiming {
    /// Waiting for a surface texture: with FIFO this is where vsync blocks.
    pub acquire: Duration,
    /// Building GPU data (text shaping, uploads) and submitting.
    pub cpu: Duration,
}

/// Which surface a draw is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Which {
    Device,
    Shell,
}

/// What one surface's scene needs on the GPU: its view uniform, rectangle instances and text.
struct Surface {
    view_buffer: wgpu::Buffer,
    view_bind: wgpu::BindGroup,
    rects: RectBatch,
    text: LayerText,
}

impl Surface {
    fn new(device: &wgpu::Device, fonts: &mut Fonts, view_layout: &wgpu::BindGroupLayout) -> Self {
        let view_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("view"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let view_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("view"),
            layout: view_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: view_buffer.as_entire_binding(),
            }],
        });
        Self {
            view_buffer,
            view_bind,
            rects: RectBatch::new(device),
            text: LayerText::new(device, fonts),
        }
    }
}

/// An instanced quad pipeline: a triangle strip of four vertices per instance, drawing into
/// `format` with `blend`.
fn quad_pipeline(
    device: &wgpu::Device,
    label: &str,
    source: &str,
    groups: &[&wgpu::BindGroupLayout],
    instance: Option<wgpu::VertexBufferLayout>,
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(source)),
    });
    let groups: Vec<Option<&wgpu::BindGroupLayout>> = groups.iter().copied().map(Some).collect();
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        bind_group_layouts: &groups,
        ..Default::default()
    });
    let buffers: Vec<Option<wgpu::VertexBufferLayout>> = instance.into_iter().map(Some).collect();
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs"),
            buffers: &buffers,
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs"),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        cache: None,
        multiview_mask: None,
    })
}

/// A design of `design` pixels → physical pixels on a `w`×`h` target: (scale, offset x,
/// offset y). The design is scaled uniformly and centred.
fn transform(w: u32, h: u32, (dw, dh): (f32, f32)) -> (f32, f32, f32) {
    let (w, h) = (w as f32, h as f32);
    let s = (w / dw).min(h / dh);
    (s, (w - dw * s) * 0.5, (h - dh * s) * 0.5)
}

/// sRGB colour → what the shader writes (linear for an sRGB target).
fn color(srgb: bool, c: Rgba) -> [f32; 4] {
    let [r, g, b, a] = c.0;
    if !srgb {
        return [r, g, b, a];
    }
    let lin = |v: f32| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    [lin(r), lin(g), lin(b), a]
}

pub struct Gfx {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    srgb: bool,
    sampler: wgpu::Sampler,
    rects: Rects,
    waves: Waves,
    images: Images,
    fonts: Fonts,
    offscreen: Offscreen,
    device_surface: Surface,
    shell_surface: Surface,
    /// Where the last frame put the device screen and the shell, in window pixels: the device
    /// screen's (scale, x, y) and the shell's (scale, x, y), for mapping touches.
    device_xform: (f32, f32, f32),
    shell_xform: Option<(f32, f32, f32)>,

    pub adapter_info: wgpu::AdapterInfo,
    pub present_mode: wgpu::PresentMode,
    // Dropped last: the surface refers to it.
    window: Arc<Window>,
}

impl Gfx {
    pub fn new(window: Arc<Window>, event_loop: &ActiveEventLoop) -> Result<Self, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(
            Box::new(event_loop.owned_display_handle()),
        ));
        let surface = instance
            .create_surface(Arc::clone(&window))
            .map_err(|e| format!("surface: {e}"))?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .map_err(|e| format!("adapter: {e}"))?;
        let adapter_info = adapter.get_info();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .map_err(|e| format!("device: {e}"))?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .or(caps.formats.first().copied())
            .ok_or("surface has no formats")?;
        let size = window.inner_size();
        // FIFO is vsync and always supported; one frame in flight keeps input-to-photon short.
        let present_mode = wgpu::PresentMode::Fifo;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            view_formats: vec![],
            desired_maximum_frame_latency: 1,
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(&device, &config);

        let view_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("view"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                // The waveform fragment shader reads the scale and the sRGB flag.
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("blit"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let mut fonts = Fonts::new(&device, &queue, format);
        let device_surface = Surface::new(&device, &mut fonts, &view_layout);
        let shell_surface = Surface::new(&device, &mut fonts, &view_layout);

        Ok(Self {
            rects: Rects::new(&device, &view_layout, format),
            waves: Waves::new(&device, &view_layout, format),
            images: Images::new(&device, &view_layout, format),
            offscreen: Offscreen::new(&device, format),
            fonts,
            device_surface,
            shell_surface,
            sampler,
            device,
            queue,
            surface,
            config,
            srgb: format.is_srgb(),
            device_xform: (1.0, 0.0, 0.0),
            shell_xform: None,
            adapter_info,
            present_mode,
            window,
        })
    }

    pub fn window(&self) -> &Window {
        &self.window
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
    }

    /// Physical pixel position in the window → the device screen's design pixels, as the
    /// last frame placed it. Outside the device screen (on a shell) it is out of range.
    pub fn to_design(&self, x: f64, y: f64) -> (f32, f32) {
        let (s, ox, oy) = self.device_xform;
        ((x as f32 - ox) / s, (y as f32 - oy) / s)
    }

    /// True if a window pixel falls on the device screen.
    #[cfg_attr(not(feature = "standin"), allow(dead_code))]
    pub fn on_device(&self, x: f64, y: f64) -> bool {
        let (dx, dy) = self.to_design(x, y);
        (0.0..DESIGN_W).contains(&dx) && (0.0..DESIGN_H).contains(&dy)
    }

    /// Window pixel → the shell's design pixels, if a shell was drawn.
    #[cfg_attr(not(feature = "standin"), allow(dead_code))]
    pub fn to_shell(&self, x: f64, y: f64) -> Option<(f32, f32)> {
        self.shell_xform
            .map(|(s, ox, oy)| ((x as f32 - ox) / s, (y as f32 - oy) / s))
    }

    /// Draw the device screen `scene`, inside `shell` if there is one, and present it.
    /// Returns `None` if the frame was skipped (surface not ready); the caller requests
    /// another redraw either way.
    pub fn render(&mut self, scene: &Scene, shell: Option<ShellFrame>) -> Option<FrameTiming> {
        let t0 = Instant::now();
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) => f,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return None;
            }
            wgpu::CurrentSurfaceTexture::Outdated
            | wgpu::CurrentSurfaceTexture::Suboptimal(_)
            | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return None;
            }
            wgpu::CurrentSurfaceTexture::Validation => panic!("surface validation error"),
        };
        let t1 = Instant::now();
        let target = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let (w, h) = (self.config.width, self.config.height);
        let commands = self.compose(scene, shell.as_ref(), &target, w, h);
        self.queue.submit(commands);
        self.window.pre_present_notify();
        self.queue.present(frame);
        self.fonts.atlas.trim();
        Some(FrameTiming {
            acquire: t1 - t0,
            cpu: t1.elapsed(),
        })
    }

    /// Render offscreen at 1:1 design pixels and return (width, height, RGBA8 pixels): the
    /// device screen at the panel's native 1024×600, or with `shell` the whole shell.
    pub fn screenshot(&mut self, scene: &Scene, shell: Option<ShellFrame>) -> (u32, u32, Vec<u8>) {
        let (w, h) = match &shell {
            Some(f) => (f.size.0 as u32, f.size.1 as u32),
            None => (DESIGN_W as u32, DESIGN_H as u32),
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("screenshot"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let draws = self.compose(scene, shell.as_ref(), &view, w, h);
        let pixels = offscreen::read_back(
            &self.device,
            &self.queue,
            &texture,
            self.config.format,
            (w, h),
            draws,
        );
        (w, h, pixels)
    }

    /// Encode a whole frame into `target`: the device screen alone, or the shell with the
    /// device screen rendered offscreen and copied into its display slot.
    fn compose(
        &mut self,
        scene: &Scene,
        shell: Option<&ShellFrame>,
        target: &wgpu::TextureView,
        w: u32,
        h: u32,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(shell) = shell else {
            let xform = transform(w, h, (DESIGN_W, DESIGN_H));
            self.device_xform = xform;
            self.shell_xform = None;
            return vec![self.draw(Which::Device, scene, target, w, h, xform)];
        };
        let sx = transform(w, h, shell.size);
        let (s, ox, oy) = sx;
        let (dx, dy, dw, dh) = shell.display;
        let px = ((ox + dx * s).round(), (oy + dy * s).round());
        let size = (
            ((dw * s).round() as u32).max(1),
            ((dh * s).round() as u32).max(1),
        );
        let off = self
            .offscreen
            .target(&self.device, self.config.format, &self.sampler, size);
        let dev_xform = transform(size.0, size.1, (DESIGN_W, DESIGN_H));
        self.device_xform = (dev_xform.0, px.0 + dev_xform.1, px.1 + dev_xform.2);
        self.shell_xform = Some(sx);
        let device = self.draw(Which::Device, scene, &off, size.0, size.1, dev_xform);
        let frame = self.draw(Which::Shell, shell.scene, target, w, h, sx);
        let blit = self
            .offscreen
            .blit(&self.device, &self.queue, target, (w, h), px);
        vec![device, frame, blit]
    }

    /// Encode `scene` on surface `which` into `target` (`w`×`h` physical pixels), placed by
    /// `xform` (scale, offset x, offset y).
    fn draw(
        &mut self,
        which: Which,
        scene: &Scene,
        target: &wgpu::TextureView,
        w: u32,
        h: u32,
        xform: (f32, f32, f32),
    ) -> wgpu::CommandBuffer {
        // Waveforms and pictures belong to the device screen.
        let (waves, images) = if which == Which::Device {
            (
                self.waves.upload(&self.queue, scene),
                self.images
                    .upload(&self.device, &self.queue, &self.sampler, self.srgb, scene),
            )
        } else {
            (0, 0)
        };
        let srgb = self.srgb;
        let surface = match which {
            Which::Device => &mut self.device_surface,
            Which::Shell => &mut self.shell_surface,
        };
        let (s, ox, oy) = xform;
        let mut view = Vec::with_capacity(32);
        let srgb_flag = if srgb { 1.0 } else { 0.0 };
        for v in [s, ox, oy, 0.0, w as f32, h as f32, srgb_flag, 0.0] {
            view.extend_from_slice(&v.to_ne_bytes());
        }
        self.queue.write_buffer(&surface.view_buffer, 0, &view);
        surface.rects.upload(&self.device, &self.queue, scene, srgb);
        surface.text.prepare(
            &self.device,
            &self.queue,
            &mut self.fonts,
            scene,
            (w, h),
            xform,
        );
        let [r, g, b, a] = color(srgb, scene.background).map(f64::from);
        let clear = wgpu::Color { r, g, b, a };

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            // Rectangles in batches, with each waveform after the rectangles below it.
            let count = surface.rects.count as u32;
            let mut drawn = 0u32;
            for k in 0..=waves {
                let until = scene
                    .waves
                    .get(k)
                    .filter(|_| k < waves)
                    .map_or(count, |(at, _)| *at as u32);
                if until > drawn {
                    self.rects
                        .draw(&mut pass, &surface.view_bind, &surface.rects, drawn, until);
                    drawn = until;
                }
                if k < waves {
                    self.waves.draw(&mut pass, &surface.view_bind, k as u32);
                }
            }
            if images > 0 {
                self.images
                    .draw(&mut pass, &surface.view_bind, scene, images);
            }
            surface.text.render(&self.fonts.atlas, &mut pass);
        }
        encoder.finish()
    }
}
