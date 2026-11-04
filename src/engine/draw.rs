//! Frame draw list and the four canvas pipelines (band, strokes, discs, points).
//! All geometry arrives in screen-space pixels — the f64 transform ladder lives
//! on the CPU (frames.rs); the GPU does anti-aliasing, glow, and surfaces.

use bytemuck::{Pod, Zeroable};

use super::gpu::{GpuContext, ViewTarget, FORMAT, FORMAT_HDR, SAMPLES};
use super::textures::{TexId, TexSet, TextureBank};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct StrokeVertex {
    pub pos: [f32; 2],
    pub da: [f32; 2],
    pub color: [f32; 4],
    pub params: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct PointInst {
    pub center: [f32; 2],
    pub shape: [f32; 4], // core_r, glow_r, spike, quad_half
    pub color: [f32; 4],
    pub depth: f32, // NDC z: small = near; default far so discs occlude glows
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct DiscInst {
    pub center: [f32; 2],
    pub pr: [f32; 4],   // radius_px, flatten, kind, seed
    pub sun: [f32; 4],  // sun dir view-space, ambient
    pub axis: [f32; 4], // body pole view-space, spin phase
    pub bx: [f32; 4],   // body x view-space, imagery fade 0..1
    pub by: [f32; 4],   // body y view-space, cloud longitude offset
    pub ca: [f32; 4],
    pub cb: [f32; 4],
    pub rim: [f32; 4],   // atmosphere rgb, strength
    pub fx: [f32; 4],    // atmosphere height (radii), ring inner, ring outer, relief
    pub flags: [f32; 4], // has night map, has clouds, has ring shadow, emission
    pub depth: f32,      // NDC z written to the depth buffer (spheres occlude)
}

impl DiscInst {
    /// A body with no imagery: unit axes, no atmosphere, procedural surface.
    pub fn plain(center: [f32; 2], pr: [f32; 4], depth: f32) -> Self {
        Self {
            center,
            pr,
            sun: [0.0, 0.0, 1.0, 0.05],
            axis: [0.0, 0.0, 1.0, 0.0],
            bx: [1.0, 0.0, 0.0, 0.0],
            by: [0.0, 1.0, 0.0, 0.0],
            ca: [1.0; 4],
            cb: [1.0; 4],
            rim: [0.0; 4],
            fx: [0.0; 4],
            flags: [0.0; 4],
            depth,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct RingInst {
    pub center: [f32; 2],
    pub r: [f32; 4],    // radius_px, inner, outer (planet radii), near flag
    pub pole: [f32; 4], // pole view-space, depth
    pub sun: [f32; 4],  // sun view-space, brightness
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct GalaxyInst {
    pub center: [f32; 2],
    pub m: [f32; 4],     // px -> galaxy plane
    pub shape: [f32; 4], // radius_px, kind, seed, quad_half
    pub tint: [f32; 4],  // arm rgb, brightness
}

/// Where a ring's radial profile comes from.
#[derive(Clone, Copy)]
pub enum RingMap {
    /// a file in the texture bank
    File(TexId),
    /// generated from the embedded ring zones, by index into `sim::RINGS`
    Zones(usize),
}

pub const KIND_EMBER: f32 = 0.0;
pub const KIND_ROCKY: f32 = 1.0;
pub const KIND_BANDED: f32 = 2.0;
pub const KIND_STAR: f32 = 3.0;
pub const KIND_OCEAN: f32 = 4.0;
pub const KIND_ICY: f32 = 5.0;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct CanvasUniforms {
    pub size: [f32; 4],
    pub fog_a: [f32; 4],
    pub fog_b: [f32; 4],
    pub cos_a: [f32; 4],
    pub cos_b: [f32; 4],
    pub deep_a: [f32; 4],
    pub deep_b: [f32; 4],
    pub helio: [f32; 4],
    pub helio_b: [f32; 4],
    pub grav_a: [f32; 4],
    pub grav_b: [f32; 4],
    pub sky_r: [f32; 4],
    pub sky_u: [f32; 4],
    pub sky_f: [f32; 4],
    pub sun_g: [f32; 4],
    pub nodes: [[f32; 4]; 16],
}

impl Default for CanvasUniforms {
    fn default() -> Self {
        Self {
            size: [1.0, 1.0, 0.0, 1.0],
            fog_a: [0.0; 4],
            fog_b: [0.0; 4],
            cos_a: [0.0; 4],
            cos_b: [0.0; 4],
            deep_a: [0.0; 4],
            deep_b: [0.0; 4],
            helio: [0.0; 4],
            helio_b: [0.0; 4],
            grav_a: [0.0; 4],
            grav_b: [0.0; 4],
            sky_r: [0.0; 4],
            sky_u: [0.0; 4],
            sky_f: [0.0; 4],
            sun_g: [0.0; 4],
            nodes: [[0.0; 4]; 16],
        }
    }
}

/// Style for a stroked path.
#[derive(Clone, Copy)]
pub struct StrokeStyle {
    pub color: [f32; 4],
    pub width: f32,
    pub glow: f32,
    pub dash: f32, // 0 = solid, else dash period px
    pub duty: f32, // dash duty cycle 0..1
}

impl StrokeStyle {
    pub fn new(color: [f32; 4], width: f32) -> Self {
        Self {
            color,
            width,
            glow: 0.0,
            dash: 0.0,
            duty: 0.6,
        }
    }
    pub fn glow(mut self, g: f32) -> Self {
        self.glow = g;
        self
    }
    pub fn dashed(mut self, period: f32, duty: f32) -> Self {
        self.dash = period;
        self.duty = duty;
        self
    }
}

#[derive(Default)]
pub struct DrawList {
    pub stroke_verts: Vec<StrokeVertex>,
    pub stroke_idx: Vec<u32>,
    // strokes drawn after discs (coastlines, surface engravings)
    pub over_verts: Vec<StrokeVertex>,
    pub over_idx: Vec<u32>,
    pub points: Vec<PointInst>,
    pub discs: Vec<DiscInst>,
    /// per-disc view depth (parallel to `discs`), used for back-to-front sort
    pub disc_z: Vec<f32>,
    /// per-disc imagery (parallel to `discs`)
    pub disc_tex: Vec<TexSet>,
    /// rings under the discs, rings over them, and their maps
    pub rings_far: Vec<RingInst>,
    pub rings_near: Vec<RingInst>,
    pub ring_tex: Vec<RingMap>,
    pub galaxies: Vec<GalaxyInst>,
    /// imagery this frame would like loaded
    pub want_tex: Vec<TexId>,
    // screen size for culling, set by the frame builder
    pub screen_w: f32,
    pub screen_h: f32,
    over_mode: bool,
}

impl DrawList {
    pub fn clear(&mut self) {
        self.stroke_verts.clear();
        self.stroke_idx.clear();
        self.over_verts.clear();
        self.over_idx.clear();
        self.points.clear();
        self.discs.clear();
        self.disc_z.clear();
        self.disc_tex.clear();
        self.rings_far.clear();
        self.rings_near.clear();
        self.ring_tex.clear();
        self.galaxies.clear();
        self.want_tex.clear();
        self.over_mode = false;
    }

    /// Add a body with its imagery set.
    pub fn disc(&mut self, inst: DiscInst, z: f32, tex: TexSet) {
        self.discs.push(inst);
        self.disc_z.push(z);
        self.disc_tex.push(tex);
    }

    /// Note a map this frame could use; returns its fade (0 until loaded).
    pub fn want(&mut self, t: TexId) {
        if !self.want_tex.contains(&t) {
            self.want_tex.push(t);
        }
    }

    /// Route subsequent strokes above (true) or below (false) the disc layer.
    pub fn set_over(&mut self, over: bool) {
        self.over_mode = over;
    }

    /// Tessellate a polyline (screen px). Each segment becomes a quad expanded
    /// by half-width + glow reach; fragment AA uses the signed perp distance.
    pub fn polyline(&mut self, pts: &[[f32; 2]], closed: bool, st: StrokeStyle) {
        self.polyline_impl(pts, closed, st, None);
    }

    /// Polyline with per-point alpha multipliers (orbit motion grading).
    pub fn polyline_alpha(
        &mut self,
        pts: &[[f32; 2]],
        closed: bool,
        st: StrokeStyle,
        alphas: &[f32],
    ) {
        self.polyline_impl(pts, closed, st, Some(alphas));
    }

    fn polyline_impl(
        &mut self,
        pts: &[[f32; 2]],
        closed: bool,
        st: StrokeStyle,
        alphas: Option<&[f32]>,
    ) {
        let n = pts.len();
        if n < 2 {
            return;
        }
        let hw = st.width * 0.5;
        let reach = hw + st.glow * 3.0 + 1.0;
        let params = [hw, st.glow, st.dash, st.duty];
        let mut arc = 0.0f32;
        let count = if closed { n } else { n - 1 };
        let (w, h) = (self.screen_w, self.screen_h);
        let (verts, idx) = if self.over_mode {
            (&mut self.over_verts, &mut self.over_idx)
        } else {
            (&mut self.stroke_verts, &mut self.stroke_idx)
        };
        for i in 0..count {
            let a = pts[i];
            let b = pts[(i + 1) % n];
            let dx = b[0] - a[0];
            let dy = b[1] - a[1];
            let len = (dx * dx + dy * dy).sqrt();
            if len < 1e-4 {
                continue;
            }
            // cull segments fully outside a generous screen margin
            let m = 64.0 + reach;
            if (a[0] < -m && b[0] < -m)
                || (a[1] < -m && b[1] < -m)
                || (a[0] > w + m && b[0] > w + m)
                || (a[1] > h + m && b[1] > h + m)
            {
                arc += len;
                continue;
            }
            let mut col = st.color;
            let mut col_b = st.color;
            if let Some(al) = alphas {
                col[3] *= al[i.min(al.len() - 1)];
                col_b[3] *= al[((i + 1) % n).min(al.len() - 1)];
            }
            let ux = dx / len;
            let uy = dy / len;
            let px = -uy * reach;
            let py = ux * reach;
            // extend caps by half-width so joins overlap seamlessly
            let ex = ux * hw.min(len * 0.5);
            let ey = uy * hw.min(len * 0.5);
            let base = verts.len() as u32;
            verts.push(StrokeVertex {
                pos: [a[0] - ex + px, a[1] - ey + py],
                da: [reach, arc],
                color: col,
                params,
            });
            verts.push(StrokeVertex {
                pos: [a[0] - ex - px, a[1] - ey - py],
                da: [-reach, arc],
                color: col,
                params,
            });
            verts.push(StrokeVertex {
                pos: [b[0] + ex + px, b[1] + ey + py],
                da: [reach, arc + len],
                color: col_b,
                params,
            });
            verts.push(StrokeVertex {
                pos: [b[0] + ex - px, b[1] + ey - py],
                da: [-reach, arc + len],
                color: col_b,
                params,
            });
            idx.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 1, base + 3]);
            arc += len;
        }
    }

    pub fn circle(&mut self, c: [f32; 2], r: f32, st: StrokeStyle) {
        // ≤ ~4 px chords up to very large radii (AUDIT REND-006, partial)
        let n = ((r * std::f32::consts::TAU / 4.0) as usize).clamp(24, 1024);
        let pts: Vec<[f32; 2]> = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                [c[0] + r * a.cos(), c[1] + r * a.sin()]
            })
            .collect();
        self.polyline(&pts, true, st);
    }

    pub fn point(
        &mut self,
        center: [f32; 2],
        core_r: f32,
        glow_r: f32,
        spike: f32,
        color: [f32; 4],
    ) {
        self.point_z(center, core_r, glow_r, spike, color, 0.9995);
    }

    /// Point sprite at an explicit NDC depth (pair glows with their discs).
    pub fn point_z(
        &mut self,
        center: [f32; 2],
        core_r: f32,
        glow_r: f32,
        spike: f32,
        color: [f32; 4],
        depth: f32,
    ) {
        let quad = (core_r * 2.6).max(glow_r * 4.2).clamp(2.0, 420.0);
        self.points.push(PointInst {
            center,
            shape: [core_r, glow_r, spike, quad],
            color,
            depth,
        });
    }
}

struct GrowBuffer {
    buf: wgpu::Buffer,
    cap: u64,
    usage: wgpu::BufferUsages,
    label: &'static str,
}

impl GrowBuffer {
    fn new(gpu: &GpuContext, label: &'static str, usage: wgpu::BufferUsages) -> Self {
        let cap = 64 * 1024;
        let buf = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: cap,
            usage: usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            buf,
            cap,
            usage,
            label,
        }
    }
    fn upload(&mut self, gpu: &GpuContext, data: &[u8]) {
        let need = data.len() as u64;
        if need > self.cap {
            self.cap = need.next_power_of_two();
            self.buf = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(self.label),
                size: self.cap,
                usage: self.usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !data.is_empty() {
            gpu.queue.write_buffer(&self.buf, 0, data);
        }
    }
}

pub struct Pipelines {
    pub view_bgl: wgpu::BindGroupLayout,
    ubuf: wgpu::Buffer,
    bind: wgpu::BindGroup,
    band: wgpu::RenderPipeline,
    strokes: wgpu::RenderPipeline,
    discs: wgpu::RenderPipeline,
    points: wgpu::RenderPipeline,
    rings_far: wgpu::RenderPipeline,
    rings_near: wgpu::RenderPipeline,
    galaxies: wgpu::RenderPipeline,
    tonemap: wgpu::RenderPipeline,
    bloom_pre: wgpu::RenderPipeline,
    bloom_down: wgpu::RenderPipeline,
    bloom_up: wgpu::RenderPipeline,
    tm_bgl: wgpu::BindGroupLayout,
    bloom_bgl: wgpu::BindGroupLayout,
    tm_sampler: wgpu::Sampler,
    vb_strokes: GrowBuffer,
    ib_strokes: GrowBuffer,
    vb_over: GrowBuffer,
    ib_over: GrowBuffer,
    vb_points: GrowBuffer,
    vb_discs: GrowBuffer,
    vb_rings_far: GrowBuffer,
    vb_rings_near: GrowBuffer,
    vb_galaxies: GrowBuffer,
}

const PRELUDE: &str = include_str!("../../shaders/prelude.wgsl");

fn blend_pma() -> wgpu::BlendState {
    wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        },
    }
}

fn blend_add() -> wgpu::BlendState {
    let c = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    };
    wgpu::BlendState { color: c, alpha: c }
}

impl Pipelines {
    pub fn new(gpu: &GpuContext, surface_bgl: &wgpu::BindGroupLayout) -> Self {
        let device = &gpu.device;
        let view_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("canvas-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let ubuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("canvas-uniforms"),
            size: std::mem::size_of::<CanvasUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("canvas-bg"),
            layout: &view_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: ubuf.as_entire_binding(),
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("canvas-pl"),
            bind_group_layouts: &[&view_bgl, surface_bgl],
            push_constant_ranges: &[],
        });

        let make = |label: &str, src: &str| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(format!("{PRELUDE}\n{src}").into()),
            })
        };
        let sm_band = make("band", include_str!("../../shaders/band.wgsl"));
        let sm_strokes = make("strokes", include_str!("../../shaders/strokes.wgsl"));
        let sm_discs = make("discs", include_str!("../../shaders/discs.wgsl"));
        let sm_points = make("points", include_str!("../../shaders/points.wgsl"));
        let sm_rings = make("rings", include_str!("../../shaders/rings.wgsl"));
        let sm_galaxies = make("galaxies", include_str!("../../shaders/galaxies.wgsl"));

        // canvas pipelines render into the HDR intermediate
        let target = [Some(wgpu::ColorTargetState {
            format: FORMAT_HDR,
            blend: Some(blend_pma()),
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let prim = wgpu::PrimitiveState::default();
        let msaa = wgpu::MultisampleState {
            count: SAMPLES,
            ..Default::default()
        };

        // one canvas pass, per-pipeline depth policy: discs write real depth,
        // points and near rings test against it, background and engraving
        // strokes pass through untested
        let pipeline = |sm: &wgpu::ShaderModule,
                        buffers: &[wgpu::VertexBufferLayout],
                        label: &str,
                        fs: &str,
                        depth_write: bool,
                        depth_test: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: sm,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers,
                },
                fragment: Some(wgpu::FragmentState {
                    module: sm,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &target,
                }),
                primitive: prim,
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: crate::engine::gpu::DEPTH_FORMAT,
                    depth_write_enabled: depth_write,
                    depth_compare: if depth_test {
                        wgpu::CompareFunction::LessEqual
                    } else {
                        wgpu::CompareFunction::Always
                    },
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: msaa,
                multiview: None,
                cache: None,
            })
        };

        let stroke_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<StrokeVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Float32x4],
        };
        let point_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<PointInst>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4, 2 => Float32x4, 3 => Float32],
        };
        let disc_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<DiscInst>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Float32x4, 8 => Float32x4, 9 => Float32x4, 10 => Float32x4, 11 => Float32],
        };
        let ring_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<RingInst>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4],
        };
        let galaxy_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<GalaxyInst>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4],
        };

        // tonemap pass: HDR resolve + bloom -> display sRGB (own layout)
        let tex2d = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let smp1 = wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let tm_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tonemap-bgl"),
            entries: &[tex2d(0), smp1, tex2d(2)],
        });
        let bloom_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bloom-bgl"),
            entries: &[tex2d(0), smp1],
        });
        let tm_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("tonemap-pl"),
            bind_group_layouts: &[&tm_bgl],
            push_constant_ranges: &[],
        });
        let bloom_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("bloom-pl"),
            bind_group_layouts: &[&bloom_bgl],
            push_constant_ranges: &[],
        });
        let post = |label: &str,
                    layout: &wgpu::PipelineLayout,
                    sm: &wgpu::ShaderModule,
                    fs: &str,
                    format: wgpu::TextureFormat,
                    blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: sm,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: sm,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            })
        };
        let sm_tm = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("tonemap"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/tonemap.wgsl").into()),
        });
        let sm_bloom = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("bloom"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/bloom.wgsl").into()),
        });
        let tonemap = post("tonemap", &tm_layout, &sm_tm, "fs", FORMAT, None);
        let bloom_pre = post(
            "bloom-pre",
            &bloom_layout,
            &sm_bloom,
            "fs_prefilter",
            FORMAT_HDR,
            None,
        );
        let bloom_down = post(
            "bloom-down",
            &bloom_layout,
            &sm_bloom,
            "fs_down",
            FORMAT_HDR,
            None,
        );
        let bloom_up = post(
            "bloom-up",
            &bloom_layout,
            &sm_bloom,
            "fs_up",
            FORMAT_HDR,
            Some(blend_add()),
        );
        let tm_sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("tonemap-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        Self {
            band: pipeline(&sm_band, &[], "band", "fs", false, false),
            strokes: pipeline(&sm_strokes, &[stroke_layout], "strokes", "fs", false, false),
            discs: pipeline(&sm_discs, &[disc_layout], "discs", "fs", true, true),
            points: pipeline(&sm_points, &[point_layout], "points", "fs", false, true),
            rings_far: pipeline(
                &sm_rings,
                std::slice::from_ref(&ring_layout),
                "rings-far",
                "fs",
                false,
                false,
            ),
            rings_near: pipeline(
                &sm_rings,
                std::slice::from_ref(&ring_layout),
                "rings-near",
                "fs",
                false,
                true,
            ),
            galaxies: pipeline(
                &sm_galaxies,
                &[galaxy_layout],
                "galaxies",
                "fs",
                false,
                false,
            ),
            tonemap,
            bloom_pre,
            bloom_down,
            bloom_up,
            tm_bgl,
            bloom_bgl,
            tm_sampler,
            vb_strokes: GrowBuffer::new(gpu, "vb-strokes", wgpu::BufferUsages::VERTEX),
            ib_strokes: GrowBuffer::new(gpu, "ib-strokes", wgpu::BufferUsages::INDEX),
            vb_over: GrowBuffer::new(gpu, "vb-over", wgpu::BufferUsages::VERTEX),
            ib_over: GrowBuffer::new(gpu, "ib-over", wgpu::BufferUsages::INDEX),
            vb_points: GrowBuffer::new(gpu, "vb-points", wgpu::BufferUsages::VERTEX),
            vb_discs: GrowBuffer::new(gpu, "vb-discs", wgpu::BufferUsages::VERTEX),
            vb_rings_far: GrowBuffer::new(gpu, "vb-rings-far", wgpu::BufferUsages::VERTEX),
            vb_rings_near: GrowBuffer::new(gpu, "vb-rings-near", wgpu::BufferUsages::VERTEX),
            vb_galaxies: GrowBuffer::new(gpu, "vb-galaxies", wgpu::BufferUsages::VERTEX),
            view_bgl,
            ubuf,
            bind,
        }
    }

    /// Render one canvas frame into `target` (clears to absolute black).
    pub fn render(
        &mut self,
        gpu: &GpuContext,
        target: &ViewTarget,
        dl: &DrawList,
        uni: &CanvasUniforms,
        bank: &mut TextureBank,
    ) {
        gpu.queue
            .write_buffer(&self.ubuf, 0, bytemuck::bytes_of(uni));
        self.vb_strokes
            .upload(gpu, bytemuck::cast_slice(&dl.stroke_verts));
        self.ib_strokes
            .upload(gpu, bytemuck::cast_slice(&dl.stroke_idx));
        self.vb_over
            .upload(gpu, bytemuck::cast_slice(&dl.over_verts));
        self.ib_over.upload(gpu, bytemuck::cast_slice(&dl.over_idx));
        self.vb_points.upload(gpu, bytemuck::cast_slice(&dl.points));
        self.vb_discs.upload(gpu, bytemuck::cast_slice(&dl.discs));
        self.vb_rings_far
            .upload(gpu, bytemuck::cast_slice(&dl.rings_far));
        self.vb_rings_near
            .upload(gpu, bytemuck::cast_slice(&dl.rings_near));
        self.vb_galaxies
            .upload(gpu, bytemuck::cast_slice(&dl.galaxies));

        // resolve bind groups up front: the pass borrows them
        let sky_bg = bank.group(gpu, [Some(TexId::Sky), None, None]);
        let none_bg = bank.group(gpu, [None, None, None]);
        let disc_bgs: Vec<wgpu::BindGroup> =
            dl.disc_tex.iter().map(|t| bank.group(gpu, *t)).collect();
        let ring_bgs: Vec<wgpu::BindGroup> = dl
            .ring_tex
            .iter()
            .map(|t| match t {
                RingMap::File(id) => bank.group(gpu, [Some(*id), None, None]),
                RingMap::Zones(i) => bank.zone_ring_group(gpu, *i),
            })
            .collect();

        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        {
            let mut pass = target.begin(&mut encoder, Some(wgpu::Color::BLACK));
            pass.set_bind_group(0, &self.bind, &[]);
            pass.set_bind_group(1, &sky_bg, &[]);
            pass.set_pipeline(&self.band);
            pass.draw(0..3, 0..1);
            pass.set_bind_group(1, &none_bg, &[]);
            if !dl.galaxies.is_empty() {
                pass.set_pipeline(&self.galaxies);
                pass.set_vertex_buffer(0, self.vb_galaxies.buf.slice(..));
                pass.draw(0..6, 0..dl.galaxies.len() as u32);
            }
            if !dl.stroke_idx.is_empty() {
                pass.set_pipeline(&self.strokes);
                pass.set_vertex_buffer(0, self.vb_strokes.buf.slice(..));
                pass.set_index_buffer(self.ib_strokes.buf.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..dl.stroke_idx.len() as u32, 0, 0..1);
            }
            if !dl.rings_far.is_empty() {
                pass.set_pipeline(&self.rings_far);
                pass.set_vertex_buffer(0, self.vb_rings_far.buf.slice(..));
                for (i, bg) in ring_bgs.iter().enumerate() {
                    pass.set_bind_group(1, bg, &[]);
                    pass.draw(0..6, i as u32..i as u32 + 1);
                }
            }
            if !dl.discs.is_empty() {
                pass.set_pipeline(&self.discs);
                pass.set_vertex_buffer(0, self.vb_discs.buf.slice(..));
                for (i, bg) in disc_bgs.iter().enumerate() {
                    pass.set_bind_group(1, bg, &[]);
                    pass.draw(0..6, i as u32..i as u32 + 1);
                }
            }
            if !dl.rings_near.is_empty() {
                pass.set_pipeline(&self.rings_near);
                pass.set_vertex_buffer(0, self.vb_rings_near.buf.slice(..));
                for (i, bg) in ring_bgs.iter().enumerate() {
                    pass.set_bind_group(1, bg, &[]);
                    pass.draw(0..6, i as u32..i as u32 + 1);
                }
            }
            pass.set_bind_group(1, &none_bg, &[]);
            if !dl.over_idx.is_empty() {
                pass.set_pipeline(&self.strokes);
                pass.set_vertex_buffer(0, self.vb_over.buf.slice(..));
                pass.set_index_buffer(self.ib_over.buf.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..dl.over_idx.len() as u32, 0, 0..1);
            }
            if !dl.points.is_empty() {
                pass.set_pipeline(&self.points);
                pass.set_vertex_buffer(0, self.vb_points.buf.slice(..));
                pass.draw(0..6, 0..dl.points.len() as u32);
            }
        }
        // HDR -> bloom mip chain -> AgX tonemap into the display texture
        if let Some(hdr_view) = &target.hdr_resolve_view {
            let bloom_bg = |src: &wgpu::TextureView| {
                gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("bloom-bg"),
                    layout: &self.bloom_bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(src),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.tm_sampler),
                        },
                    ],
                })
            };
            let fullscreen = |encoder: &mut wgpu::CommandEncoder,
                              view: &wgpu::TextureView,
                              pipeline: &wgpu::RenderPipeline,
                              bg: &wgpu::BindGroup,
                              load: wgpu::LoadOp<wgpu::Color>| {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("post-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, bg, &[]);
                pass.draw(0..3, 0..1);
            };
            let clear = wgpu::LoadOp::Clear(wgpu::Color::BLACK);
            let mips = &target.bloom_mips;
            fullscreen(
                &mut encoder,
                &mips[0],
                &self.bloom_pre,
                &bloom_bg(hdr_view),
                clear,
            );
            for m in 1..mips.len() {
                fullscreen(
                    &mut encoder,
                    &mips[m],
                    &self.bloom_down,
                    &bloom_bg(&mips[m - 1]),
                    clear,
                );
            }
            for m in (1..mips.len()).rev() {
                fullscreen(
                    &mut encoder,
                    &mips[m - 1],
                    &self.bloom_up,
                    &bloom_bg(&mips[m]),
                    wgpu::LoadOp::Load,
                );
            }
            let bg_tm = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("tonemap-bg"),
                layout: &self.tm_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(hdr_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.tm_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&mips[0]),
                    },
                ],
            });
            fullscreen(
                &mut encoder,
                &target.resolve_view,
                &self.tonemap,
                &bg_tm,
                clear,
            );
        }
        gpu.queue.submit([encoder.finish()]);
    }
}
