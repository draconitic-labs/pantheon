//! GPU context and offscreen render targets. Interactive canvas and headless
//! export share this path (adapted from the proven revelatio harness).

use anyhow::{anyhow, Context, Result};
use std::path::Path;

pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
pub const FORMAT_HDR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
pub const SAMPLES: u32 = 4;

#[derive(Clone)]
pub struct GpuContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

impl GpuContext {
    pub fn headless() -> Result<Self> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .map_err(|e| anyhow!("no GPU adapter: {e}"))?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("pantheon-device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::default(),
                trace: wgpu::Trace::Off,
            }))?;
        Ok(Self { device, queue })
    }

    pub fn from_eframe(rs: &eframe::egui_wgpu::RenderState) -> Self {
        Self { device: rs.device.clone(), queue: rs.queue.clone() }
    }
}

/// Adapter description for diagnostics (`pantheon --doctor`).
pub fn adapter_info() -> Result<String> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .map_err(|e| anyhow!("no GPU adapter: {e}"))?;
    let i = adapter.get_info();
    Ok(format!("{} ({:?}, {:?})", i.name, i.backend, i.device_type))
}

/// Offscreen MSAA color target with resolve texture and readback.
pub struct ViewTarget {
    pub width: u32,
    pub height: u32,
    /// canvas pass color attachment (HDR when `hdr`, else sRGB)
    pub msaa_view: wgpu::TextureView,
    /// what consumers see: ALWAYS the display sRGB texture (tonemapped when hdr)
    pub resolve: wgpu::Texture,
    pub resolve_view: wgpu::TextureView,
    pub depth_view: wgpu::TextureView,
    /// HDR resolve, input to the bloom/tonemap passes (hdr targets only)
    pub hdr_resolve_view: Option<wgpu::TextureView>,
    /// bloom mip chain, half resolution downward (hdr targets only)
    pub bloom_mips: Vec<wgpu::TextureView>,
}

impl ViewTarget {
    pub fn new(gpu: &GpuContext, width: u32, height: u32) -> Self {
        Self::new_with(gpu, width, height, false)
    }

    pub fn new_hdr(gpu: &GpuContext, width: u32, height: u32) -> Self {
        Self::new_with(gpu, width, height, true)
    }

    fn new_with(gpu: &GpuContext, width: u32, height: u32, hdr: bool) -> Self {
        let width = width.max(8);
        let height = height.max(8);
        let size = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
        let color_fmt = if hdr { FORMAT_HDR } else { FORMAT };
        let msaa = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("view-msaa"),
            size,
            mip_level_count: 1,
            sample_count: SAMPLES,
            dimension: wgpu::TextureDimension::D2,
            format: color_fmt,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let hdr_resolve_view = if hdr {
            let t = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("view-hdr-resolve"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT_HDR,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            Some(t.create_view(&Default::default()))
        } else {
            None
        };
        let bloom_mips = if hdr {
            let (bw, bh) = ((width / 2).max(4), (height / 2).max(4));
            let levels = (32 - bw.min(bh).leading_zeros()).clamp(1, 7);
            let t = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("view-bloom"),
                size: wgpu::Extent3d { width: bw, height: bh, depth_or_array_layers: 1 },
                mip_level_count: levels,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT_HDR,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            (0..levels)
                .map(|m| {
                    t.create_view(&wgpu::TextureViewDescriptor {
                        base_mip_level: m,
                        mip_level_count: Some(1),
                        ..Default::default()
                    })
                })
                .collect()
        } else {
            Vec::new()
        };
        let resolve = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("view-resolve"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let depth = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("view-depth"),
            size,
            mip_level_count: 1,
            sample_count: SAMPLES,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        Self {
            width,
            height,
            msaa_view: msaa.create_view(&Default::default()),
            resolve_view: resolve.create_view(&Default::default()),
            resolve,
            depth_view: depth.create_view(&Default::default()),
            hdr_resolve_view,
            bloom_mips,
        }
    }

    /// Begin the canvas render pass (with depth). `clear: None` loads content.
    pub fn begin<'e>(
        &self,
        encoder: &'e mut wgpu::CommandEncoder,
        clear: Option<wgpu::Color>,
    ) -> wgpu::RenderPass<'e> {
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("view-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.msaa_view,
                resolve_target: Some(self.hdr_resolve_view.as_ref().unwrap_or(&self.resolve_view)),
                ops: wgpu::Operations {
                    load: match clear {
                        Some(c) => wgpu::LoadOp::Clear(c),
                        None => wgpu::LoadOp::Load,
                    },
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        })
    }

    /// Depth-less pass for pipelines that declare no depth state (egui).
    pub fn begin_no_depth<'e>(
        &self,
        encoder: &'e mut wgpu::CommandEncoder,
        clear: Option<wgpu::Color>,
    ) -> wgpu::RenderPass<'e> {
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("view-pass-nodepth"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.msaa_view,
                resolve_target: Some(&self.resolve_view),
                ops: wgpu::Operations {
                    load: match clear {
                        Some(c) => wgpu::LoadOp::Clear(c),
                        None => wgpu::LoadOp::Load,
                    },
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        })
    }

    pub fn read_pixels(&self, gpu: &GpuContext) -> Result<Vec<u8>> {
        let bpr = (self.width * 4 + 255) / 256 * 256;
        let buf = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (bpr * self.height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.resolve,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bpr),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 },
        );
        gpu.queue.submit([encoder.finish()]);
        let slice = buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = gpu.device.poll(wgpu::PollType::Wait);
        rx.recv()??;
        let data = slice.get_mapped_range();
        let mut out = Vec::with_capacity((self.width * self.height * 4) as usize);
        for row in 0..self.height {
            let start = (row * bpr) as usize;
            out.extend_from_slice(&data[start..start + (self.width * 4) as usize]);
        }
        drop(data);
        buf.unmap();
        Ok(out)
    }
}

pub fn save_png(path: &Path, w: u32, h: u32, rgba: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let img =
        image::RgbaImage::from_raw(w, h, rgba.to_vec()).context("bad pixel buffer size")?;
    img.save(path)?;
    Ok(())
}
