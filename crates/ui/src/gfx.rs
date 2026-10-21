//! The renderer: one wgpu pipeline for rounded rectangles (an instanced quad with a
//! signed-distance fragment shader) and glyphon for text.
//!
//! Layout happens in design pixels (1024×600); the view uniform scales and centres that onto
//! the surface, so the same scene fits a Retina window on the Mac and the panel on the Pi.

use std::borrow::Cow;
use std::sync::Arc;
use std::time::{Duration, Instant};

use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache,
    TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

use crate::scene::{Align, Rgba, Scene, Weight};

pub const DESIGN_W: f32 = 1024.0;
pub const DESIGN_H: f32 = 600.0;

const FONT_REGULAR: &[u8] = include_bytes!("../assets/fonts/BarlowCondensed-Regular.ttf");
const FONT_SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/BarlowCondensed-SemiBold.ttf");
const FAMILY: &str = "Barlow Condensed";

const SHADER: &str = r#"
struct View { xform: vec4<f32>, size: vec4<f32> };  // scale, offset x, offset y; surface w, h
@group(0) @binding(0) var<uniform> view: View;

struct Inst {
    @location(0) rect: vec4<f32>,
    @location(1) params: vec4<f32>,   // radius, border
    @location(2) fill: vec4<f32>,
    @location(3) stroke: vec4<f32>,
};
struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) half: vec2<f32>,
    @location(2) params: vec2<f32>,
    @location(3) fill: vec4<f32>,
    @location(4) stroke: vec4<f32>,
};

@vertex
fn vs(@builtin(vertex_index) vi: u32, i: Inst) -> Out {
    let corner = vec2<f32>(f32(vi & 1u), f32(vi >> 1u));
    let s = view.xform.x;
    let p0 = i.rect.xy * s + view.xform.yz;
    let size = i.rect.zw * s;
    // One physical pixel of margin for the anti-aliased edge.
    let p = p0 - vec2(1.0) + corner * (size + vec2(2.0));
    var o: Out;
    o.pos = vec4(p.x / view.size.x * 2.0 - 1.0, 1.0 - p.y / view.size.y * 2.0, 0.0, 1.0);
    o.local = p - (p0 + size * 0.5);
    o.half = size * 0.5;
    o.params = i.params.xy * s;
    o.fill = i.fill;
    o.stroke = i.stroke;
    return o;
}

fn sd_round_box(p: vec2<f32>, half: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - half + vec2(r);
    return length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

@fragment
fn fs(o: Out) -> @location(0) vec4<f32> {
    let r = min(o.params.x, min(o.half.x, o.half.y));
    let d = sd_round_box(o.local, o.half, r);
    let coverage = clamp(0.5 - d, 0.0, 1.0);
    let fill = vec4(o.fill.rgb * o.fill.a, o.fill.a);
    var c = fill;
    if (o.params.y > 0.0) {
        let inside = clamp(0.5 - (d + o.params.y), 0.0, 1.0);
        c = mix(vec4(o.stroke.rgb * o.stroke.a, o.stroke.a), fill, inside);
    }
    return c * coverage;
}
"#;

const INSTANCE_ATTRS: [wgpu::VertexAttribute; 4] =
    wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4];
const INSTANCE_BYTES: u64 = 16 * 4;

/// Design pixels → physical pixels on a `w`×`h` target: (scale, offset x, offset y). The
/// design is scaled uniformly and centred.
fn transform(w: u32, h: u32) -> (f32, f32, f32) {
    let (w, h) = (w as f32, h as f32);
    let s = (w / DESIGN_W).min(h / DESIGN_H);
    (s, (w - DESIGN_W * s) * 0.5, (h - DESIGN_H * s) * 0.5)
}

/// How long the last frame spent where.
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameTiming {
    /// Waiting for a surface texture: with FIFO this is where vsync blocks.
    pub acquire: Duration,
    /// Building GPU data (text shaping, uploads) and submitting.
    pub cpu: Duration,
}

/// Cached shaped text, reused while its content is unchanged.
struct TextSlot {
    key: (String, u32, Weight, u32),
    buffer: Buffer,
    width: f32,
}

pub struct Gfx {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    srgb: bool,
    pipeline: wgpu::RenderPipeline,
    view_buffer: wgpu::Buffer,
    view_bind: wgpu::BindGroup,
    instances: wgpu::Buffer,
    instance_capacity: u64,
    instance_bytes: Vec<u8>,

    fonts: FontSystem,
    swash: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    text: TextRenderer,
    slots: Vec<TextSlot>,

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

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rects"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(SHADER)),
        });
        let view_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("view"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let view_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("view"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let view_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("view"),
            layout: &view_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: view_buffer.as_entire_binding(),
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            bind_group_layouts: &[Some(&view_layout)],
            ..Default::default()
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("rects"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: INSTANCE_BYTES,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &INSTANCE_ATTRS,
                })],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
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
        });
        let instance_capacity = 1024;
        let instances = Self::instance_buffer(&device, instance_capacity);

        let mut fonts = FontSystem::new();
        fonts.db_mut().load_font_data(FONT_REGULAR.to_vec());
        fonts.db_mut().load_font_data(FONT_SEMIBOLD.to_vec());
        let cache = Cache::new(&device);
        let viewport = Viewport::new(&device, &cache);
        let mut atlas = TextAtlas::new(&device, &queue, &cache, format);
        let text = TextRenderer::new(&mut atlas, &device, wgpu::MultisampleState::default(), None);

        Ok(Self {
            device,
            queue,
            surface,
            config,
            srgb: format.is_srgb(),
            pipeline,
            view_buffer,
            view_bind,
            instances,
            instance_capacity,
            instance_bytes: Vec::new(),
            fonts,
            swash: SwashCache::new(),
            viewport,
            atlas,
            text,
            slots: Vec::new(),
            adapter_info,
            present_mode,
            window,
        })
    }

    fn instance_buffer(device: &wgpu::Device, capacity: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rect instances"),
            size: capacity * INSTANCE_BYTES,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
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

    /// Physical pixel position in the window → design pixels.
    pub fn to_design(&self, x: f64, y: f64) -> (f32, f32) {
        let (s, ox, oy) = transform(self.config.width, self.config.height);
        ((x as f32 - ox) / s, (y as f32 - oy) / s)
    }

    fn color(&self, c: Rgba) -> [f32; 4] {
        let [r, g, b, a] = c.0;
        if !self.srgb {
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

    /// Draw `scene` and present it. Returns `None` if the frame was skipped (surface not
    /// ready); the caller requests another redraw either way.
    pub fn render(&mut self, scene: &Scene) -> Option<FrameTiming> {
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
        let commands = self.draw(scene, &target, w, h);
        self.queue.submit(Some(commands));
        self.window.pre_present_notify();
        self.queue.present(frame);
        self.atlas.trim();
        Some(FrameTiming {
            acquire: t1 - t0,
            cpu: t1.elapsed(),
        })
    }

    /// Render `scene` offscreen at the panel's native 1024×600 and return RGBA8 pixels.
    pub fn screenshot(&mut self, scene: &Scene) -> Vec<u8> {
        let (w, h) = (DESIGN_W as u32, DESIGN_H as u32);
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
        let draw = self.draw(scene, &view, w, h);
        let row = w * 4; // 4096: already a multiple of the 256-byte copy alignment
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("screenshot readback"),
            size: u64::from(row * h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([draw, encoder.finish()]);
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, |r| r.expect("map screenshot"));
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        let mut pixels = readback
            .slice(..)
            .get_mapped_range()
            .expect("screenshot mapping")
            .to_vec();
        if matches!(
            self.config.format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        ) {
            for px in pixels.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
        }
        // The window's next frame re-prepares text for its own size.
        self.slots.clear();
        pixels
    }

    /// Encode `scene` into `target` (`w`×`h` physical pixels).
    fn draw(
        &mut self,
        scene: &Scene,
        target: &wgpu::TextureView,
        w: u32,
        h: u32,
    ) -> wgpu::CommandBuffer {
        let (s, ox, oy) = transform(w, h);
        let mut view = Vec::with_capacity(32);
        for v in [s, ox, oy, 0.0, w as f32, h as f32, 0.0, 0.0] {
            view.extend_from_slice(&v.to_ne_bytes());
        }
        self.queue.write_buffer(&self.view_buffer, 0, &view);

        self.instance_bytes.clear();
        for r in &scene.rects {
            let fill = self.color(r.fill);
            let stroke = self.color(r.stroke);
            for v in [r.x, r.y, r.w, r.h, r.radius, r.border, 0.0, 0.0]
                .into_iter()
                .chain(fill)
                .chain(stroke)
            {
                self.instance_bytes.extend_from_slice(&v.to_ne_bytes());
            }
        }
        let count = scene.rects.len() as u64;
        if count > self.instance_capacity {
            self.instance_capacity = count.next_power_of_two();
            self.instances = Self::instance_buffer(&self.device, self.instance_capacity);
        }
        if count > 0 {
            self.queue
                .write_buffer(&self.instances, 0, &self.instance_bytes);
        }
        self.prepare_text(scene, w, h);
        let [r, g, b, a] = self.color(scene.background).map(f64::from);
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
            if count > 0 {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.view_bind, &[]);
                pass.set_vertex_buffer(0, self.instances.slice(..count * INSTANCE_BYTES));
                pass.draw(0..4, 0..count as u32);
            }
            if let Err(e) = self.text.render(&self.atlas, &self.viewport, &mut pass) {
                eprintln!("text render: {e}");
            }
        }
        encoder.finish()
    }

    fn prepare_text(&mut self, scene: &Scene, w: u32, h: u32) {
        let (s, ox, oy) = transform(w, h);
        self.viewport.update(
            &self.queue,
            Resolution {
                width: w,
                height: h,
            },
        );
        for (i, t) in scene.texts.iter().enumerate() {
            let key = (
                t.text.clone(),
                t.size.to_bits(),
                t.weight,
                t.line_height.to_bits(),
            );
            if self.slots.get(i).is_some_and(|slot| slot.key == key) {
                continue;
            }
            let metrics = Metrics::new(t.size, t.size * t.line_height);
            let weight = match t.weight {
                Weight::Regular => glyphon::Weight::NORMAL,
                Weight::SemiBold => glyphon::Weight::SEMIBOLD,
            };
            let attrs = Attrs::new().family(Family::Name(FAMILY)).weight(weight);
            if i == self.slots.len() {
                self.slots.push(TextSlot {
                    key: key.clone(),
                    buffer: Buffer::new(&mut self.fonts, metrics),
                    width: 0.0,
                });
            }
            let slot = &mut self.slots[i];
            slot.buffer.set_metrics(metrics);
            slot.buffer.set_size(None, None);
            slot.buffer
                .set_text(&t.text, &attrs, Shaping::Advanced, None);
            slot.buffer.shape_until_scroll(&mut self.fonts, false);
            slot.width = slot
                .buffer
                .layout_runs()
                .map(|r| r.line_w)
                .fold(0.0, f32::max);
            slot.key = key;
        }
        self.slots.truncate(scene.texts.len());
        let bounds = TextBounds {
            left: 0,
            top: 0,
            right: w as i32,
            bottom: h as i32,
        };
        let areas = scene.texts.iter().zip(&self.slots).map(|(t, slot)| {
            let left = match t.align {
                Align::Left => t.x,
                Align::Center => t.x - slot.width * 0.5,
                Align::Right => t.x - slot.width,
            };
            let [r, g, b, a] = t.color.0;
            let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            TextArea {
                buffer: &slot.buffer,
                left: ox + left * s,
                top: oy + t.y * s,
                scale: s,
                bounds,
                default_color: Color::rgba(byte(r), byte(g), byte(b), byte(a)),
                custom_glyphs: &[],
            }
        });
        if let Err(e) = self.text.prepare(
            &self.device,
            &self.queue,
            &mut self.fonts,
            &mut self.atlas,
            &self.viewport,
            areas,
            &mut self.swash,
        ) {
            eprintln!("text prepare: {e}");
        }
    }
}
