//! Rounded rectangles: an instanced quad with a signed-distance fragment shader, for fills,
//! borders and vertical gradients.

use super::scene::{Gradient, Scene};
use super::{color, quad_pipeline};

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

const ATTRS: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
    0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4
];
const INSTANCE_BYTES: u64 = 20 * 4;

/// The rectangle pipeline, shared by every layer.
pub(super) struct Rects {
    pipeline: wgpu::RenderPipeline,
}

impl Rects {
    pub(super) fn new(
        device: &wgpu::Device,
        view_layout: &wgpu::BindGroupLayout,
        format: wgpu::TextureFormat,
    ) -> Self {
        let pipeline = quad_pipeline(
            device,
            "rects",
            SHADER,
            &[view_layout],
            Some(wgpu::VertexBufferLayout {
                array_stride: INSTANCE_BYTES,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &ATTRS,
            }),
            format,
            Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
        );
        Self { pipeline }
    }

    /// Draw instances `from..to` of `batch`.
    pub(super) fn draw(
        &self,
        pass: &mut wgpu::RenderPass,
        view_bind: &wgpu::BindGroup,
        batch: &RectBatch,
        from: u32,
        to: u32,
    ) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, view_bind, &[]);
        pass.set_vertex_buffer(0, batch.buffer.slice(..batch.count * INSTANCE_BYTES));
        pass.draw(0..4, from..to);
    }
}

/// One layer's rectangle instances on the GPU, grown as needed.
pub(super) struct RectBatch {
    buffer: wgpu::Buffer,
    capacity: u64,
    bytes: Vec<u8>,
    /// Instances written by the last [`RectBatch::upload`].
    pub(super) count: u64,
}

impl RectBatch {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let capacity = 1024;
        Self {
            buffer: buffer(device, capacity),
            capacity,
            bytes: Vec::new(),
            count: 0,
        }
    }

    /// Write `scene`'s rectangles (colours converted for an sRGB target if `srgb`).
    pub(super) fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &Scene,
        srgb: bool,
    ) {
        self.bytes.clear();
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
                self.bytes.extend_from_slice(&v.to_ne_bytes());
            }
        }
        self.count = scene.rects.len() as u64;
        if self.count > self.capacity {
            self.capacity = self.count.next_power_of_two();
            self.buffer = buffer(device, self.capacity);
        }
        if self.count > 0 {
            queue.write_buffer(&self.buffer, 0, &self.bytes);
        }
    }
}

fn buffer(device: &wgpu::Device, capacity: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("rect instances"),
        size: capacity * INSTANCE_BYTES,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}
