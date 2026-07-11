//! Frame draw list and the four canvas pipelines (band, strokes, discs, points).
//! All geometry arrives in screen-space pixels — the f64 transform ladder lives
//! on the CPU (frames.rs); the GPU does anti-aliasing, glow, and surfaces.

use bytemuck::{Pod, Zeroable};

use super::gpu::{GpuContext, ViewTarget, FORMAT, FORMAT_HDR, SAMPLES};

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
    pub axis: [f32; 4], // spin axis view-space, rot phase
    pub ca: [f32; 4],
    pub cb: [f32; 4],
    pub rim: [f32; 4],
    pub depth: f32, // NDC z written to the depth buffer (spheres occlude)
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
    pub helio: [f32; 4],
    pub grav_a: [f32; 4],
    pub grav_b: [f32; 4],
    pub misc: [f32; 4],
}

impl Default for CanvasUniforms {
    fn default() -> Self {
        Self {
            size: [1.0, 1.0, 0.0, 0.0],
            fog_a: [0.0; 4],
            fog_b: [0.0; 4],
            cos_a: [0.0; 4],
            cos_b: [0.0; 4],
            helio: [0.0; 4],
            grav_a: [0.0; 4],
            grav_b: [0.0; 4],
            misc: [1.0, 0.0, 0.0, 0.0],
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
        Self { color, width, glow: 0.0, dash: 0.0, duty: 0.6 }
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
        self.over_mode = false;
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
    pub fn polyline_alpha(&mut self, pts: &[[f32; 2]], closed: bool, st: StrokeStyle, alphas: &[f32]) {
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
            if let Some(al) = alphas {
                col[3] *= al[i.min(al.len() - 1)];
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
                color: col,
                params,
            });
            verts.push(StrokeVertex {
                pos: [b[0] + ex - px, b[1] + ey - py],
                da: [-reach, arc + len],
                color: col,
                params,
            });
            idx.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 1, base + 3]);
            arc += len;
        }
    }

    pub fn circle(&mut self, c: [f32; 2], r: f32, st: StrokeStyle) {
        // ≤ ~4 px chords up to very large radii (AUDIT REND-006, partial)
        let n = ((r * 6.28 / 4.0) as usize).clamp(24, 1024);
        let pts: Vec<[f32; 2]> = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                [c[0] + r * a.cos(), c[1] + r * a.sin()]
            })
            .collect();
        self.polyline(&pts, true, st);
    }

    pub fn point(&mut self, center: [f32; 2], core_r: f32, glow_r: f32, spike: f32, color: [f32; 4]) {
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
        let quad = (core_r * 2.4).max(glow_r * 3.2).max(2.0).min(300.0);
        self.points.push(PointInst { center, shape: [core_r, glow_r, spike, quad], color, depth });
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
        Self { buf, cap, usage, label }
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
    tonemap: wgpu::RenderPipeline,
    bloom: wgpu::RenderPipeline,
    tm_bgl: wgpu::BindGroupLayout,
    tm_sampler: wgpu::Sampler,
    vb_strokes: GrowBuffer,
    ib_strokes: GrowBuffer,
    vb_over: GrowBuffer,
    ib_over: GrowBuffer,
    vb_points: GrowBuffer,
    vb_discs: GrowBuffer,
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

impl Pipelines {
    pub fn new(gpu: &GpuContext) -> Self {
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
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("canvas-pl"),
            bind_group_layouts: &[&view_bgl],
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

        // canvas pipelines render into the HDR intermediate (S2)
        let target = [Some(wgpu::ColorTargetState {
            format: FORMAT_HDR,
            blend: Some(blend_pma()),
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let prim = wgpu::PrimitiveState::default();
        let msaa = wgpu::MultisampleState { count: SAMPLES, ..Default::default() };

        // one canvas pass, per-pipeline depth policy: discs write real depth,
        // points test against it (a star can no longer shine through a planet),
        // background and engraving strokes pass through untested
        let pipeline = |sm: &wgpu::ShaderModule,
                        buffers: &[wgpu::VertexBufferLayout],
                        label: &str,
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
                    entry_point: Some("fs"),
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
            attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Float32],
        };

        // tonemap pass: HDR resolve → display sRGB (own layout, no depth/msaa)
        let tm_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tonemap-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let tm_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("tonemap-pl"),
            bind_group_layouts: &[&tm_bgl],
            push_constant_ranges: &[],
        });
        let sm_tm = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("tonemap"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/tonemap.wgsl").into()),
        });
        let tonemap = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("tonemap"),
            layout: Some(&tm_layout),
            vertex: wgpu::VertexState {
                module: &sm_tm,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &sm_tm,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let sm_bloom = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("bloom"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/bloom.wgsl").into()),
        });
        let bloom = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("bloom"),
            layout: Some(&tm_layout),
            vertex: wgpu::VertexState {
                module: &sm_bloom,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &sm_bloom,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: FORMAT_HDR,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let tm_sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("tonemap-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        Self {
            band: pipeline(&sm_band, &[], "band", false, false),
            strokes: pipeline(&sm_strokes, &[stroke_layout], "strokes", false, false),
            discs: pipeline(&sm_discs, &[disc_layout], "discs", true, true),
            points: pipeline(&sm_points, &[point_layout], "points", false, true),
            tonemap,
            bloom,
            tm_bgl,
            tm_sampler,
            vb_strokes: GrowBuffer::new(gpu, "vb-strokes", wgpu::BufferUsages::VERTEX),
            ib_strokes: GrowBuffer::new(gpu, "ib-strokes", wgpu::BufferUsages::INDEX),
            vb_over: GrowBuffer::new(gpu, "vb-over", wgpu::BufferUsages::VERTEX),
            ib_over: GrowBuffer::new(gpu, "ib-over", wgpu::BufferUsages::INDEX),
            vb_points: GrowBuffer::new(gpu, "vb-points", wgpu::BufferUsages::VERTEX),
            vb_discs: GrowBuffer::new(gpu, "vb-discs", wgpu::BufferUsages::VERTEX),
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
    ) {
        gpu.queue.write_buffer(&self.ubuf, 0, bytemuck::bytes_of(uni));
        self.vb_strokes.upload(gpu, bytemuck::cast_slice(&dl.stroke_verts));
        self.ib_strokes.upload(gpu, bytemuck::cast_slice(&dl.stroke_idx));
        self.vb_over.upload(gpu, bytemuck::cast_slice(&dl.over_verts));
        self.ib_over.upload(gpu, bytemuck::cast_slice(&dl.over_idx));
        self.vb_points.upload(gpu, bytemuck::cast_slice(&dl.points));
        self.vb_discs.upload(gpu, bytemuck::cast_slice(&dl.discs));

        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        {
            let mut pass = target.begin(&mut encoder, Some(wgpu::Color::BLACK));
            pass.set_bind_group(0, &self.bind, &[]);
            pass.set_pipeline(&self.band);
            pass.draw(0..3, 0..1);
            if !dl.stroke_idx.is_empty() {
                pass.set_pipeline(&self.strokes);
                pass.set_vertex_buffer(0, self.vb_strokes.buf.slice(..));
                pass.set_index_buffer(self.ib_strokes.buf.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..dl.stroke_idx.len() as u32, 0, 0..1);
            }
            if !dl.discs.is_empty() {
                pass.set_pipeline(&self.discs);
                pass.set_vertex_buffer(0, self.vb_discs.buf.slice(..));
                pass.draw(0..6, 0..dl.discs.len() as u32);
            }
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
        // HDR → bloom (half res) → ACES tonemap into the display texture
        if let (Some(hdr_view), Some(bloom_view)) =
            (&target.hdr_resolve_view, &target.bloom_view)
        {
            let make_bg = |b2: &wgpu::TextureView| {
                gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("post-bg"),
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
                            resource: wgpu::BindingResource::TextureView(b2),
                        },
                    ],
                })
            };
            let fullscreen = |encoder: &mut wgpu::CommandEncoder,
                              view: &wgpu::TextureView,
                              pipeline: &wgpu::RenderPipeline,
                              bg: &wgpu::BindGroup| {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("post-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
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
            let bg_bloom = make_bg(hdr_view); // binding 2 unused by bloom.wgsl
            fullscreen(&mut encoder, bloom_view, &self.bloom, &bg_bloom);
            let bg_tm = make_bg(bloom_view);
            fullscreen(&mut encoder, &target.resolve_view, &self.tonemap, &bg_tm);
        }
        gpu.queue.submit([encoder.finish()]);
    }
}
