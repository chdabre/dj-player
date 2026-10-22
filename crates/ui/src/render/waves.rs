//! Waveform columns, drawn from per-column segment data (`crate::waves`). Each instance is
//! one waveform area; its columns are rows `row..row + 6` of the column texture: three rows of
//! (from, to, alpha) and three of the segment colours (sRGB). A fragment box-filters the
//! columns under its pixel, so fractional column positions scroll smoothly, and blends a
//! column's segments in sRGB values, as the CPU reference does.

use super::quad_pipeline;
use super::scene::Scene;

const SHADER: &str = r#"
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
const COLUMNS: u32 = 1088;
const ROWS: u32 = 6;
const ATTRS: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4];
const INSTANCE_BYTES: u64 = 12 * 4;

pub(super) struct Waves {
    pipeline: wgpu::RenderPipeline,
    texture: wgpu::Texture,
    bind: wgpu::BindGroup,
    instances: wgpu::Buffer,
    bytes: Vec<u8>,
    texels: Vec<u8>,
}

impl Waves {
    pub(super) fn new(
        device: &wgpu::Device,
        view_layout: &wgpu::BindGroupLayout,
        format: wgpu::TextureFormat,
    ) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("waveform columns"),
            size: wgpu::Extent3d {
                width: COLUMNS,
                height: ROWS * MAX_WAVES as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("waveform columns"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(
                    &texture.create_view(&wgpu::TextureViewDescriptor::default()),
                ),
            }],
        });
        let pipeline = quad_pipeline(
            device,
            "waveform",
            SHADER,
            &[view_layout, &layout],
            Some(wgpu::VertexBufferLayout {
                array_stride: INSTANCE_BYTES,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &ATTRS,
            }),
            format,
            Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
        );
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("waveform instances"),
            size: MAX_WAVES as u64 * INSTANCE_BYTES,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            pipeline,
            texture,
            bind,
            instances,
            bytes: Vec::new(),
            texels: Vec::new(),
        }
    }

    /// Write the scene's waveform columns into the column texture and their instances;
    /// returns how many waveforms to draw.
    pub(super) fn upload(&mut self, queue: &wgpu::Queue, scene: &Scene) -> usize {
        if scene.waves.len() > MAX_WAVES {
            eprintln!("only {MAX_WAVES} waveforms per frame; dropping the rest");
        }
        let waves = scene.waves.len().min(MAX_WAVES);
        self.bytes.clear();
        for (k, (_, wave)) in scene.waves.iter().take(waves).enumerate() {
            let n = wave.columns.len().min(COLUMNS as usize);
            let row = k as u32 * ROWS;
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
                self.bytes.extend_from_slice(&v.to_ne_bytes());
            }
            if n == 0 {
                continue;
            }
            // Six rows: (from, to, alpha, 0) for each segment, then each segment's colour.
            self.texels.clear();
            for r in 0..ROWS as usize {
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
                        self.texels.extend_from_slice(&x.to_ne_bytes());
                    }
                }
            }
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: 0, y: row, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                &self.texels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(n as u32 * 16),
                    rows_per_image: Some(ROWS),
                },
                wgpu::Extent3d {
                    width: n as u32,
                    height: ROWS,
                    depth_or_array_layers: 1,
                },
            );
        }
        if waves > 0 {
            queue.write_buffer(&self.instances, 0, &self.bytes);
        }
        waves
    }

    /// Draw uploaded waveform `k`.
    pub(super) fn draw(&self, pass: &mut wgpu::RenderPass, view_bind: &wgpu::BindGroup, k: u32) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, view_bind, &[]);
        pass.set_bind_group(1, &self.bind, &[]);
        pass.set_vertex_buffer(0, self.instances.slice(..));
        pass.draw(0..4, k..k + 1);
    }
}
