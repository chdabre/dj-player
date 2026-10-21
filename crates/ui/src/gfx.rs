//! The renderer: one wgpu pipeline for rounded rectangles (an instanced quad with a
//! signed-distance fragment shader), one for waveforms, and glyphon for text.
//!
//! Layout happens in design pixels (1024×600 for the device screen); the view uniform scales
//! and centres that onto the target, so the same scene fits a Retina window on the Mac and the
//! panel on the Pi.
//!
//! **Layers.** The device screen is one layer. A debug shell (the hardware stand-in, feature
//! `standin`) is a second layer with its own design size: when it is on, the device layer is
//! rendered on its own into an offscreen texture exactly the size of the shell's display slot,
//! the shell is rendered to the window, and the device texture is copied into the slot. The
//! device layer never knows about the shell, so a device build simply drops it.

use std::borrow::Cow;
use std::sync::Arc;
use std::time::{Duration, Instant};

use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache,
    TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

use crate::scene::{Align, Gradient, Rgba, Scene, Weight};

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
    @location(1) params: vec4<f32>,   // radius, border, gradient (0 none, 1 down, 2 mirrored)
    @location(2) fill: vec4<f32>,
    @location(3) stroke: vec4<f32>,
    @location(4) fill2: vec4<f32>,    // the gradient's other colour
};
struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) half: vec2<f32>,
    @location(2) params: vec3<f32>,
    @location(3) fill: vec4<f32>,
    @location(4) stroke: vec4<f32>,
    @location(5) fill2: vec4<f32>,
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
    o.params = vec3(i.params.xy * s, i.params.z);
    o.fill = i.fill;
    o.stroke = i.stroke;
    o.fill2 = i.fill2;
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
    // Vertical gradient: `fill` at the top (down) or at the edges (mirrored), `fill2` at
    // the bottom or the centre line.
    var f = o.fill;
    let v = clamp(o.local.y / o.half.y, -1.0, 1.0);
    if (o.params.z > 1.5) {
        f = mix(o.fill, o.fill2, 1.0 - abs(v));
    } else if (o.params.z > 0.5) {
        f = mix(o.fill, o.fill2, v * 0.5 + 0.5);
    }
    let fill = vec4(f.rgb * f.a, f.a);
    var c = fill;
    if (o.params.y > 0.0) {
        let inside = clamp(0.5 - (d + o.params.y), 0.0, 1.0);
        c = mix(vec4(o.stroke.rgb * o.stroke.a, o.stroke.a), fill, inside);
    }
    return c * coverage;
}
"#;

/// Waveform columns, drawn from per-column segment data (`waves.rs`). Each instance is one
/// waveform area; its columns are rows `row..row + 6` of the column texture: three rows of
/// (from, to, alpha) and three of the segment colours (sRGB). A fragment box-filters the
/// columns under its pixel, so fractional column positions scroll smoothly, and blends a
/// column's segments in sRGB values, as the CPU reference does.
const WAVE_SHADER: &str = r#"
struct View { xform: vec4<f32>, size: vec4<f32> };  // size.z: 1 = sRGB surface
@group(0) @binding(0) var<uniform> view: View;
@group(1) @binding(0) var cols: texture_2d<f32>;

struct Inst {
    @location(0) rect: vec4<f32>,
    @location(1) params: vec4<f32>,  // first x, column width, column count, first row
    @location(2) extra: vec4<f32>,   // align (0 mirrored, 1 bottom), opacity
};
struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) design: vec2<f32>,
    @location(1) rect: vec4<f32>,
    @location(2) params: vec4<f32>,
    @location(3) extra: vec4<f32>,
};

@vertex
fn vs(@builtin(vertex_index) vi: u32, i: Inst) -> Out {
    let corner = vec2<f32>(f32(vi & 1u), f32(vi >> 1u));
    let d = i.rect.xy + corner * i.rect.zw;
    let p = d * view.xform.x + view.xform.yz;
    var o: Out;
    o.pos = vec4(p.x / view.size.x * 2.0 - 1.0, 1.0 - p.y / view.size.y * 2.0, 0.0, 1.0);
    o.design = d;
    o.rect = i.rect;
    o.params = i.params;
    o.extra = i.extra;
    return o;
}

// Premultiplied sRGB colour of column `j` at height `d` (0 = baseline, 1 = edge).
fn column_color(j: i32, d: f32, count: i32, row: i32) -> vec4<f32> {
    if (j < 0 || j >= count) {
        return vec4(0.0);
    }
    var acc = vec4(0.0);
    for (var s = 0; s < 3; s = s + 1) {
        let seg = textureLoad(cols, vec2<i32>(j, row + s), 0);
        if (seg.y > seg.x && d >= seg.x && d < seg.y) {
            let c = textureLoad(cols, vec2<i32>(j, row + 3 + s), 0).rgb;
            let a = seg.z;
            acc = vec4(c * a, a) + acc * (1.0 - a);
        }
    }
    return acc;
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3(2.4));
    return select(hi, lo, c <= vec3(0.04045));
}

@fragment
fn fs(o: Out) -> @location(0) vec4<f32> {
    let count = i32(o.params.z);
    let row = i32(o.params.w);
    var d: f32;
    if (o.extra.x < 0.5) {
        let half = o.rect.w * 0.5;
        d = abs(o.design.y - (o.rect.y + half)) / half;
    } else {
        d = (o.rect.y + o.rect.w - o.design.y) / o.rect.w;
    }
    // This pixel's footprint in columns, and the two columns it can straddle.
    let fw = 1.0 / (view.xform.x * o.params.y);
    let u = (o.design.x - o.params.x) / o.params.y;
    let a = u - fw * 0.5;
    let j0 = floor(a);
    let w1 = clamp((a + fw - (j0 + 1.0)) / fw, 0.0, 1.0);
    let c = mix(
        column_color(i32(j0), d, count, row),
        column_color(i32(j0) + 1, d, count, row),
        w1,
    ) * o.extra.y;
    if (view.size.z < 0.5 || c.a <= 0.0) {
        return c;
    }
    return vec4(to_linear(c.rgb / c.a) * c.a, c.a);
}
"#;

/// Waveform areas per frame (zoom and overview, with room for two decks).
const MAX_WAVES: usize = 4;
/// Columns per waveform area: the panel is 1024 design pixels wide.
const WAVE_COLUMNS: u32 = 1088;
const WAVE_ROWS: u32 = 6;
const WAVE_ATTRS: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4];
const WAVE_INSTANCE_BYTES: u64 = 12 * 4;

const INSTANCE_ATTRS: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
    0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4
];
const INSTANCE_BYTES: u64 = 20 * 4;

/// A design of `design` pixels → physical pixels on a `w`×`h` target: (scale, offset x,
/// offset y). The design is scaled uniformly and centred.
fn transform(w: u32, h: u32, (dw, dh): (f32, f32)) -> (f32, f32, f32) {
    let (w, h) = (w as f32, h as f32);
    let s = (w / dw).min(h / dh);
    (s, (w - dw * s) * 0.5, (h - dh * s) * 0.5)
}

/// Copies the device layer's offscreen texture into the shell's display slot.
const BLIT_SHADER: &str = r#"
struct Rect { clip: vec4<f32> };  // left, top, right, bottom in clip space
@group(0) @binding(0) var<uniform> rect: Rect;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var smp: sampler;
struct Out { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };

@vertex
fn vs(@builtin(vertex_index) vi: u32) -> Out {
    let c = vec2<f32>(f32(vi & 1u), f32(vi >> 1u));
    var o: Out;
    o.pos = vec4(mix(rect.clip.x, rect.clip.z, c.x), mix(rect.clip.y, rect.clip.w, c.y), 0.0, 1.0);
    o.uv = c;
    return o;
}

@fragment
fn fs(o: Out) -> @location(0) vec4<f32> {
    return textureSample(tex, smp, o.uv);
}
"#;

/// Which layer a draw is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Which {
    Device,
    Shell,
}

/// A debug shell around the device screen: its scene, its design size, and the slot (in its
/// design pixels) where the device screen goes.
pub struct ShellFrame<'a> {
    pub scene: &'a Scene,
    pub size: (f32, f32),
    pub display: (f32, f32, f32, f32),
}

/// What one scene needs on the GPU: its view uniform, rectangle instances and text.
struct Layer {
    view_buffer: wgpu::Buffer,
    view_bind: wgpu::BindGroup,
    instances: wgpu::Buffer,
    instance_capacity: u64,
    instance_bytes: Vec<u8>,
    viewport: Viewport,
    text: TextRenderer,
    slots: Vec<TextSlot>,
}

impl Layer {
    fn new(
        device: &wgpu::Device,
        cache: &Cache,
        atlas: &mut TextAtlas,
        view_layout: &wgpu::BindGroupLayout,
    ) -> Self {
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
        let instance_capacity = 1024;
        Self {
            view_buffer,
            view_bind,
            instances: instance_buffer(device, instance_capacity),
            instance_capacity,
            instance_bytes: Vec::new(),
            viewport: Viewport::new(device, cache),
            text: TextRenderer::new(atlas, device, wgpu::MultisampleState::default(), None),
            slots: Vec::new(),
        }
    }
}

fn instance_buffer(device: &wgpu::Device, capacity: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("rect instances"),
        size: capacity * INSTANCE_BYTES,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// The device layer's offscreen target while a shell is on, and the bind group that copies it.
struct Offscreen {
    size: (u32, u32),
    view: wgpu::TextureView,
    bind: wgpu::BindGroup,
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
    device_layer: Layer,
    shell_layer: Layer,
    blit_pipeline: wgpu::RenderPipeline,
    blit_layout: wgpu::BindGroupLayout,
    blit_rect: wgpu::Buffer,
    sampler: wgpu::Sampler,
    offscreen: Option<Offscreen>,
    /// Where the last frame put the device screen and the shell, in window pixels: the device
    /// screen's (scale, x, y) and the shell's (scale, x, y), for mapping touches.
    device_xform: (f32, f32, f32),
    shell_xform: Option<(f32, f32, f32)>,
    wave_pipeline: wgpu::RenderPipeline,
    wave_texture: wgpu::Texture,
    wave_bind: wgpu::BindGroup,
    wave_instances: wgpu::Buffer,
    wave_bytes: Vec<u8>,

    fonts: FontSystem,
    swash: SwashCache,
    atlas: TextAtlas,

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
        let wave_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("waveform columns"),
            size: wgpu::Extent3d {
                width: WAVE_COLUMNS,
                height: WAVE_ROWS * MAX_WAVES as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let wave_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("waveform columns"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let wave_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("waveform columns"),
            layout: &wave_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(
                    &wave_texture.create_view(&wgpu::TextureViewDescriptor::default()),
                ),
            }],
        });
        let wave_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("waveform"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(WAVE_SHADER)),
        });
        let wave_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            bind_group_layouts: &[Some(&view_layout), Some(&wave_layout)],
            ..Default::default()
        });
        let wave_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("waveform"),
            layout: Some(&wave_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &wave_shader,
                entry_point: Some("vs"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: WAVE_INSTANCE_BYTES,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &WAVE_ATTRS,
                })],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &wave_shader,
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
        let wave_instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("waveform instances"),
            size: MAX_WAVES as u64 * WAVE_INSTANCE_BYTES,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blit"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let blit_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blit"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(BLIT_SHADER)),
        });
        let blit_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            bind_group_layouts: &[Some(&blit_layout)],
            ..Default::default()
        });
        let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("blit"),
            layout: Some(&blit_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &blit_shader,
                entry_point: Some("vs"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &blit_shader,
                entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
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
        let blit_rect = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("blit rect"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("blit"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let mut fonts = FontSystem::new();
        fonts.db_mut().load_font_data(FONT_REGULAR.to_vec());
        fonts.db_mut().load_font_data(FONT_SEMIBOLD.to_vec());
        let cache = Cache::new(&device);
        let mut atlas = TextAtlas::new(&device, &queue, &cache, format);
        let device_layer = Layer::new(&device, &cache, &mut atlas, &view_layout);
        let shell_layer = Layer::new(&device, &cache, &mut atlas, &view_layout);

        Ok(Self {
            device,
            queue,
            surface,
            config,
            srgb: format.is_srgb(),
            pipeline,
            device_layer,
            shell_layer,
            blit_pipeline,
            blit_layout,
            blit_rect,
            sampler,
            offscreen: None,
            device_xform: (1.0, 0.0, 0.0),
            shell_xform: None,
            wave_pipeline,
            wave_texture,
            wave_bind,
            wave_instances,
            wave_bytes: Vec::new(),
            fonts,
            swash: SwashCache::new(),
            atlas,
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
        self.atlas.trim();
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
        // Rows padded to the 256-byte copy alignment.
        let row = (w * 4).div_ceil(256) * 256;
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
        let mut commands = draws;
        commands.push(encoder.finish());
        self.queue.submit(commands);
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, |r| r.expect("map screenshot"));
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        let padded = readback
            .slice(..)
            .get_mapped_range()
            .expect("screenshot mapping")
            .to_vec();
        let mut pixels: Vec<u8> = padded
            .chunks_exact(row as usize)
            .flat_map(|r| r[..(w * 4) as usize].iter().copied())
            .collect();
        if matches!(
            self.config.format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        ) {
            for px in pixels.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
        }
        (w, h, pixels)
    }

    /// Encode a whole frame into `target`: the device layer alone, or the shell with the
    /// device layer rendered offscreen and copied into its display slot.
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
        self.ensure_offscreen(size);
        let dev_xform = transform(size.0, size.1, (DESIGN_W, DESIGN_H));
        self.device_xform = (dev_xform.0, px.0 + dev_xform.1, px.1 + dev_xform.2);
        self.shell_xform = Some(sx);
        let off = self.offscreen.as_ref().expect("just made").view.clone();
        let device = self.draw(Which::Device, scene, &off, size.0, size.1, dev_xform);
        let frame = self.draw(Which::Shell, shell.scene, target, w, h, sx);
        // The device screen into its slot.
        let (wf, hf) = (w as f32, h as f32);
        let clip = [
            px.0 / wf * 2.0 - 1.0,
            1.0 - px.1 / hf * 2.0,
            (px.0 + size.0 as f32) / wf * 2.0 - 1.0,
            1.0 - (px.1 + size.1 as f32) / hf * 2.0,
        ];
        let bytes: Vec<u8> = clip.iter().flat_map(|v| v.to_ne_bytes()).collect();
        self.queue.write_buffer(&self.blit_rect, 0, &bytes);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("device screen into the shell"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.blit_pipeline);
            pass.set_bind_group(0, &self.offscreen.as_ref().expect("just made").bind, &[]);
            pass.draw(0..4, 0..1);
        }
        vec![device, frame, encoder.finish()]
    }

    /// Make sure the device layer's offscreen target is `size` physical pixels.
    fn ensure_offscreen(&mut self, size: (u32, u32)) {
        if self.offscreen.as_ref().is_some_and(|o| o.size == size) {
            return;
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("device screen"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("blit"),
            layout: &self.blit_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.blit_rect.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.offscreen = Some(Offscreen { size, view, bind });
    }

    /// Encode `scene` on layer `which` into `target` (`w`×`h` physical pixels), placed by
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
        // Waveforms belong to the device screen.
        let waves = if which == Which::Device {
            self.upload_waves(scene)
        } else {
            0
        };
        let srgb = self.srgb;
        let layer = match which {
            Which::Device => &mut self.device_layer,
            Which::Shell => &mut self.shell_layer,
        };
        let (s, ox, oy) = xform;
        let mut view = Vec::with_capacity(32);
        let srgb_flag = if srgb { 1.0 } else { 0.0 };
        for v in [s, ox, oy, 0.0, w as f32, h as f32, srgb_flag, 0.0] {
            view.extend_from_slice(&v.to_ne_bytes());
        }
        self.queue.write_buffer(&layer.view_buffer, 0, &view);

        layer.instance_bytes.clear();
        for r in &scene.rects {
            let fill = color(srgb, r.fill);
            let stroke = color(srgb, r.stroke);
            let (kind, fill2) = match r.gradient {
                None => (0.0, fill),
                Some((to, Gradient::Down)) => (1.0, color(srgb, to)),
                Some((to, Gradient::Mirrored)) => (2.0, color(srgb, to)),
            };
            for v in [r.x, r.y, r.w, r.h, r.radius, r.border, kind, 0.0]
                .into_iter()
                .chain(fill)
                .chain(stroke)
                .chain(fill2)
            {
                layer.instance_bytes.extend_from_slice(&v.to_ne_bytes());
            }
        }
        let count = scene.rects.len() as u64;
        if count > layer.instance_capacity {
            layer.instance_capacity = count.next_power_of_two();
            layer.instances = instance_buffer(&self.device, layer.instance_capacity);
        }
        if count > 0 {
            self.queue
                .write_buffer(&layer.instances, 0, &layer.instance_bytes);
        }
        prepare_text(
            layer,
            &self.device,
            &self.queue,
            &mut self.fonts,
            &mut self.atlas,
            &mut self.swash,
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
            let mut drawn = 0u32;
            for k in 0..=waves {
                let until = scene
                    .waves
                    .get(k)
                    .filter(|_| k < waves)
                    .map_or(count as u32, |(at, _)| *at as u32);
                if until > drawn {
                    pass.set_pipeline(&self.pipeline);
                    pass.set_bind_group(0, &layer.view_bind, &[]);
                    pass.set_vertex_buffer(0, layer.instances.slice(..count * INSTANCE_BYTES));
                    pass.draw(0..4, drawn..until);
                    drawn = until;
                }
                if k < waves {
                    pass.set_pipeline(&self.wave_pipeline);
                    pass.set_bind_group(0, &layer.view_bind, &[]);
                    pass.set_bind_group(1, &self.wave_bind, &[]);
                    pass.set_vertex_buffer(0, self.wave_instances.slice(..));
                    pass.draw(0..4, k as u32..k as u32 + 1);
                }
            }
            if let Err(e) = layer.text.render(&self.atlas, &layer.viewport, &mut pass) {
                eprintln!("text render: {e}");
            }
        }
        encoder.finish()
    }

    /// Write the scene's waveform columns into the column texture and their instances;
    /// returns how many waveforms to draw.
    fn upload_waves(&mut self, scene: &Scene) -> usize {
        if scene.waves.len() > MAX_WAVES {
            eprintln!("only {MAX_WAVES} waveforms per frame; dropping the rest");
        }
        let waves = scene.waves.len().min(MAX_WAVES);
        self.wave_bytes.clear();
        let mut texels: Vec<u8> = Vec::new();
        for (k, (_, wave)) in scene.waves.iter().take(waves).enumerate() {
            let n = wave.columns.len().min(WAVE_COLUMNS as usize);
            let row = k as u32 * WAVE_ROWS;
            let align = match wave.align {
                crate::waves::Align::Mirrored => 0.0,
                crate::waves::Align::Bottom => 1.0,
            };
            for v in [
                wave.x,
                wave.y,
                wave.w,
                wave.h,
                wave.first_x,
                wave.column_width,
                n as f32,
                row as f32,
                align,
                wave.opacity.clamp(0.0, 1.0),
                0.0,
                0.0,
            ] {
                self.wave_bytes.extend_from_slice(&v.to_ne_bytes());
            }
            if n == 0 {
                continue;
            }
            // Six rows: (from, to, alpha, 0) for each segment, then each segment's colour.
            texels.clear();
            for r in 0..WAVE_ROWS as usize {
                for col in &wave.columns[..n] {
                    let segs = col.segments();
                    let v = match segs.get(r % 3) {
                        None => [0.0; 4],
                        Some(seg) if r < 3 => [seg.from, seg.to, seg.alpha, 0.0],
                        Some(seg) => {
                            let c = seg.color;
                            let f = |x: u8| f32::from(x) / 255.0;
                            [f(c.0), f(c.1), f(c.2), 1.0]
                        }
                    };
                    for x in v {
                        texels.extend_from_slice(&x.to_ne_bytes());
                    }
                }
            }
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.wave_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: 0, y: row, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                &texels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(n as u32 * 16),
                    rows_per_image: Some(WAVE_ROWS),
                },
                wgpu::Extent3d {
                    width: n as u32,
                    height: WAVE_ROWS,
                    depth_or_array_layers: 1,
                },
            );
        }
        if waves > 0 {
            self.queue
                .write_buffer(&self.wave_instances, 0, &self.wave_bytes);
        }
        waves
    }
}

/// Shape (or reuse) `scene`'s texts on `layer` and prepare them for a `w`×`h` target placed by
/// `xform`.
#[allow(clippy::too_many_arguments)]
fn prepare_text(
    layer: &mut Layer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    fonts: &mut FontSystem,
    atlas: &mut TextAtlas,
    swash: &mut SwashCache,
    scene: &Scene,
    (w, h): (u32, u32),
    (s, ox, oy): (f32, f32, f32),
) {
    layer.viewport.update(
        queue,
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
        if layer.slots.get(i).is_some_and(|slot| slot.key == key) {
            continue;
        }
        let metrics = Metrics::new(t.size, t.size * t.line_height);
        let weight = match t.weight {
            Weight::Regular => glyphon::Weight::NORMAL,
            Weight::SemiBold => glyphon::Weight::SEMIBOLD,
        };
        let attrs = Attrs::new().family(Family::Name(FAMILY)).weight(weight);
        if i == layer.slots.len() {
            layer.slots.push(TextSlot {
                key: key.clone(),
                buffer: Buffer::new(fonts, metrics),
                width: 0.0,
            });
        }
        let slot = &mut layer.slots[i];
        slot.buffer.set_metrics(metrics);
        slot.buffer.set_size(None, None);
        slot.buffer
            .set_text(&t.text, &attrs, Shaping::Advanced, None);
        slot.buffer.shape_until_scroll(fonts, false);
        slot.width = slot
            .buffer
            .layout_runs()
            .map(|r| r.line_w)
            .fold(0.0, f32::max);
        slot.key = key;
    }
    layer.slots.truncate(scene.texts.len());
    let bounds = TextBounds {
        left: 0,
        top: 0,
        right: w as i32,
        bottom: h as i32,
    };
    let areas = scene.texts.iter().zip(&layer.slots).map(|(t, slot)| {
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
    if let Err(e) = layer
        .text
        .prepare(device, queue, fonts, atlas, &layer.viewport, areas, swash)
    {
        eprintln!("text prepare: {e}");
    }
}
