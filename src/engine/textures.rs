//! Surface imagery bank. Textures decode on worker threads and upload in slices
//! (a few MB per frame) so a planet's 8K map never stalls the frame. A body with
//! no file on disk simply stays procedural: a fresh clone has no imagery, and
//! that is a supported state, not an error.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};

use super::gpu::GpuContext;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TexId {
    Sun,
    Mercury,
    Venus,
    EarthDay,
    EarthNight,
    EarthClouds,
    Moon,
    Mars,
    Jupiter,
    Saturn,
    Uranus,
    Neptune,
    SaturnRing,
    Sky,
}

pub const ALL_TEX: [TexId; 14] = [
    TexId::Sun,
    TexId::Mercury,
    TexId::Venus,
    TexId::EarthDay,
    TexId::EarthNight,
    TexId::EarthClouds,
    TexId::Moon,
    TexId::Mars,
    TexId::Jupiter,
    TexId::Saturn,
    TexId::Uranus,
    TexId::Neptune,
    TexId::SaturnRing,
    TexId::Sky,
];

impl TexId {
    pub fn file(self) -> &'static str {
        match self {
            TexId::Sun => "8k_sun.jpg",
            TexId::Mercury => "8k_mercury.jpg",
            TexId::Venus => "4k_venus_atmosphere.jpg",
            TexId::EarthDay => "8k_earth_daymap.jpg",
            TexId::EarthNight => "8k_earth_nightmap.jpg",
            TexId::EarthClouds => "2k_earth_clouds.jpg",
            TexId::Moon => "8k_moon.jpg",
            TexId::Mars => "8k_mars.jpg",
            TexId::Jupiter => "8k_jupiter.jpg",
            TexId::Saturn => "8k_saturn.jpg",
            TexId::Uranus => "2k_uranus.jpg",
            TexId::Neptune => "2k_neptune.jpg",
            TexId::SaturnRing => "8k_saturn_ring_alpha.png",
            TexId::Sky => "eso_milkyway.jpg",
        }
    }
    fn index(self) -> usize {
        ALL_TEX.iter().position(|t| *t == self).unwrap()
    }
}

/// What the frame builder needs to know about imagery, by value: whether a map
/// exists on disk, and how far its fade-in has progressed (0..=1).
#[derive(Clone)]
pub struct TexStatus {
    pub present: [bool; 14],
    pub fade: [f32; 14],
}

impl TexStatus {
    pub fn on_disk(&self, t: TexId) -> bool {
        self.present[t.index()]
    }
    pub fn fade(&self, t: TexId) -> f32 {
        self.fade[t.index()]
    }
    /// True when any planet map is present (the data-status readout).
    pub fn any_planet_imagery(&self) -> bool {
        self.on_disk(TexId::EarthDay) || self.on_disk(TexId::Mars) || self.on_disk(TexId::Moon)
    }
}

impl Default for TexStatus {
    fn default() -> Self {
        Self {
            present: [false; 14],
            fade: [0.0; 14],
        }
    }
}

struct Decoded {
    id: TexId,
    width: u32,
    height: u32,
    /// RGBA8 mip chain, largest first
    mips: Vec<Vec<u8>>,
}

struct Upload {
    id: TexId,
    tex: wgpu::Texture,
    mips: Vec<Vec<u8>>,
    size: (u32, u32),
    level: usize,
    row: u32,
    bytes: u64,
}

enum Slot {
    Empty,
    Loading,
    Ready {
        view: wgpu::TextureView,
        bytes: u64,
        born: f64,
        last_used: f64,
    },
    Failed,
}

const SLICE_BYTES: usize = 24 * 1024 * 1024;
const BUDGET_BYTES: u64 = 1_500_000_000;
const FADE_S: f64 = 0.45;

/// Which map each of a body's three texture slots binds.
pub type TexSet = [Option<TexId>; 3];

pub struct TextureBank {
    root: PathBuf,
    slots: Vec<Slot>,
    pub status: TexStatus,
    tx: Sender<Result<Decoded, (TexId, String)>>,
    rx: Receiver<Result<Decoded, (TexId, String)>>,
    uploads: Vec<Upload>,
    pub bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    fallback_view: wgpu::TextureView,
    groups: HashMap<TexSet, wgpu::BindGroup>,
    zone_rings: Vec<Option<wgpu::BindGroup>>,
    in_flight: usize,
    max_dim: u32,
    now: f64,
}

fn mip_chain(width: u32, height: u32, rgba: Vec<u8>) -> Vec<Vec<u8>> {
    let mut mips = vec![rgba];
    let (mut w, mut h) = (width as usize, height as usize);
    while w > 1 || h > 1 {
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let src = mips.last().unwrap();
        let mut dst = vec![0u8; nw * nh * 4];
        for y in 0..nh {
            let (y0, y1) = ((y * h / nh).min(h - 1), (((y * h / nh) + 1).min(h - 1)));
            for x in 0..nw {
                let (x0, x1) = ((x * w / nw).min(w - 1), (((x * w / nw) + 1).min(w - 1)));
                let d = (y * nw + x) * 4;
                for c in 0..4 {
                    let s = src[(y0 * w + x0) * 4 + c] as u32
                        + src[(y0 * w + x1) * 4 + c] as u32
                        + src[(y1 * w + x0) * 4 + c] as u32
                        + src[(y1 * w + x1) * 4 + c] as u32;
                    dst[d + c] = ((s + 2) / 4) as u8;
                }
            }
        }
        mips.push(dst);
        w = nw;
        h = nh;
    }
    mips
}

fn decode(id: TexId, path: PathBuf, max_dim: u32) -> Result<Decoded, (TexId, String)> {
    let reader = image::ImageReader::open(&path)
        .map_err(|e| (id, format!("{}: {e}", path.display())))?
        .with_guessed_format()
        .map_err(|e| (id, format!("{}: {e}", path.display())))?;
    let mut reader = reader;
    reader.no_limits();
    let img = reader
        .decode()
        .map_err(|e| (id, format!("{}: {e}", path.display())))?;
    let mut rgba = img.to_rgba8();
    drop(img);
    // respect the device limit: halve (cheap box filter) until it fits
    let (mut w, mut h) = rgba.dimensions();
    while w > max_dim || h > max_dim {
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        rgba = image::imageops::thumbnail(&rgba, nw, nh);
        w = nw;
        h = nh;
    }
    let mips = mip_chain(w, h, rgba.into_raw());
    Ok(Decoded {
        id,
        width: w,
        height: h,
        mips,
    })
}

impl TextureBank {
    pub fn new(gpu: &GpuContext) -> Self {
        let root = crate::data::asset_root().join("data/textures");
        let mut status = TexStatus::default();
        for t in ALL_TEX {
            status.present[t.index()] = root.join(t.file()).is_file();
        }
        let tex_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let bgl = gpu
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("surface-bgl"),
                entries: &[
                    tex_entry(0),
                    tex_entry(1),
                    tex_entry(2),
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });
        let sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("surface-sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            anisotropy_clamp: 8,
            ..Default::default()
        });
        let fallback = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("surface-fallback"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        gpu.queue.write_texture(
            fallback.as_image_copy(),
            &[0, 0, 0, 0],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let fallback_view = fallback.create_view(&Default::default());
        let (tx, rx) = channel();
        let max_dim = std::env::var("PANTHEON_TEX_MAX")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(8192)
            .min(gpu.device.limits().max_texture_dimension_2d);
        Self {
            root,
            slots: ALL_TEX.iter().map(|_| Slot::Empty).collect(),
            status,
            tx,
            rx,
            uploads: Vec::new(),
            bgl,
            sampler,
            fallback_view,
            groups: HashMap::new(),
            zone_rings: crate::sim::RINGS.iter().map(|_| None).collect(),
            in_flight: 0,
            max_dim,
            now: 0.0,
        }
    }

    /// Ask for a map; the first request starts a background decode.
    pub fn want(&mut self, t: TexId) {
        let i = t.index();
        if !self.status.present[i] {
            return;
        }
        match &mut self.slots[i] {
            Slot::Empty => {
                if self.in_flight >= 3 {
                    return; // next frame
                }
                self.slots[i] = Slot::Loading;
                self.in_flight += 1;
                let tx = self.tx.clone();
                let path = self.root.join(t.file());
                let max_dim = self.max_dim;
                std::thread::spawn(move || {
                    let _ = tx.send(decode(t, path, max_dim));
                });
            }
            Slot::Ready { last_used, .. } => *last_used = self.now,
            Slot::Loading | Slot::Failed => {}
        }
    }

    /// Receive finished decodes and upload the next slice. Call once per frame.
    pub fn pump(&mut self, gpu: &GpuContext, now: f64) {
        self.now = now;
        while let Ok(res) = self.rx.try_recv() {
            self.in_flight -= 1;
            match res {
                Ok(d) => self.begin_upload(gpu, d),
                Err((id, msg)) => {
                    log::warn!("texture {id:?} failed: {msg}");
                    self.slots[id.index()] = Slot::Failed;
                    self.status.present[id.index()] = false;
                }
            }
        }
        let mut budget = SLICE_BYTES;
        while budget > 0 && !self.uploads.is_empty() {
            budget = self.upload_slice(gpu, budget);
        }
        for t in ALL_TEX {
            let i = t.index();
            self.status.fade[i] = match &self.slots[i] {
                Slot::Ready { born, .. } => (((now - *born) / FADE_S) as f32).clamp(0.0, 1.0),
                _ => 0.0,
            };
        }
        self.evict(now);
    }

    fn begin_upload(&mut self, gpu: &GpuContext, d: Decoded) {
        let tex = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(d.id.file()),
            size: wgpu::Extent3d {
                width: d.width,
                height: d.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: d.mips.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let bytes = d.mips.iter().map(|m| m.len() as u64).sum();
        self.uploads.push(Upload {
            id: d.id,
            tex,
            mips: d.mips,
            size: (d.width, d.height),
            level: 0,
            row: 0,
            bytes,
        });
    }

    /// Upload up to `budget` bytes of the first queued texture; returns what's left.
    fn upload_slice(&mut self, gpu: &GpuContext, budget: usize) -> usize {
        let Some(up) = self.uploads.first_mut() else {
            return 0;
        };
        let w = (up.size.0 >> up.level).max(1);
        let h = (up.size.1 >> up.level).max(1);
        let row_bytes = (w * 4) as usize;
        let rows = ((budget / row_bytes).max(1) as u32).min(h - up.row);
        let start = up.row as usize * row_bytes;
        let end = start + rows as usize * row_bytes;
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &up.tex,
                mip_level: up.level as u32,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: up.row,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &up.mips[up.level][start..end],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(rows),
            },
            wgpu::Extent3d {
                width: w,
                height: rows,
                depth_or_array_layers: 1,
            },
        );
        up.row += rows;
        let spent = rows as usize * row_bytes;
        if up.row >= h {
            up.mips[up.level] = Vec::new();
            up.level += 1;
            up.row = 0;
        }
        if up.level >= up.mips.len() {
            let up = self.uploads.remove(0);
            let view = up.tex.create_view(&Default::default());
            self.slots[up.id.index()] = Slot::Ready {
                view,
                bytes: up.bytes,
                born: self.now,
                last_used: self.now,
            };
            self.groups.clear();
        }
        budget.saturating_sub(spent)
    }

    fn evict(&mut self, now: f64) {
        let total: u64 = self
            .slots
            .iter()
            .map(|s| {
                if let Slot::Ready { bytes, .. } = s {
                    *bytes
                } else {
                    0
                }
            })
            .sum();
        if total <= BUDGET_BYTES {
            return;
        }
        let oldest = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| match s {
                Slot::Ready { last_used, .. } if now - *last_used > 2.0 => Some((i, *last_used)),
                _ => None,
            })
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        if let Some((i, _)) = oldest {
            self.slots[i] = Slot::Empty;
            self.groups.clear();
        }
    }

    /// Finish every requested load right now. Headless exports use this so a
    /// shot never captures a half-loaded surface.
    pub fn finish_loading(&mut self, gpu: &GpuContext) {
        while self.in_flight > 0 {
            match self.rx.recv() {
                Ok(Ok(d)) => {
                    self.in_flight -= 1;
                    self.begin_upload(gpu, d);
                }
                Ok(Err((id, msg))) => {
                    self.in_flight -= 1;
                    log::warn!("texture {id:?} failed: {msg}");
                    self.slots[id.index()] = Slot::Failed;
                    self.status.present[id.index()] = false;
                }
                Err(_) => break,
            }
        }
        while !self.uploads.is_empty() {
            let _ = self.upload_slice(gpu, usize::MAX / 2);
        }
        for s in &mut self.slots {
            if let Slot::Ready { born, .. } = s {
                *born = f64::MIN / 2.0;
            }
        }
        for t in ALL_TEX {
            let i = t.index();
            self.status.fade[i] = if matches!(self.slots[i], Slot::Ready { .. }) {
                1.0
            } else {
                0.0
            };
        }
    }

    /// True while something is decoding or uploading.
    pub fn busy(&self) -> bool {
        self.in_flight > 0 || !self.uploads.is_empty()
    }

    fn view_of(&self, t: Option<TexId>) -> &wgpu::TextureView {
        if let Some(t) = t {
            if let Slot::Ready { view, .. } = &self.slots[t.index()] {
                return view;
            }
        }
        &self.fallback_view
    }

    /// A radial ring profile generated from the embedded zones, for rings with
    /// no map on disk.
    pub fn zone_ring_group(&mut self, gpu: &GpuContext, ring: usize) -> wgpu::BindGroup {
        if let Some(Some(g)) = self.zone_rings.get(ring) {
            return g.clone();
        }
        let def = &crate::sim::RINGS[ring];
        let inner = def.zones.iter().map(|z| z.0).fold(f64::MAX, f64::min);
        let outer = def.zones.iter().map(|z| z.1).fold(0.0f64, f64::max);
        const W: u32 = 1024;
        let mut row = vec![0u8; W as usize * 4];
        for x in 0..W as usize {
            let km = inner + (outer - inner) * (x as f64 + 0.5) / W as f64;
            let dens = def
                .zones
                .iter()
                .find(|z| km >= z.0 && km <= z.1)
                .map(|z| z.2)
                .unwrap_or(0.0);
            // a little radial texture so wide zones don't read as flat bands
            let grain = 0.82 + 0.18 * (km * 0.0007).sin() as f32 * (km * 0.0031).cos() as f32;
            let a = (dens * grain * 255.0) as u8;
            row[x * 4] = 214;
            row[x * 4 + 1] = 200;
            row[x * 4 + 2] = 172;
            row[x * 4 + 3] = a;
        }
        let mips = mip_chain(W, 1, row);
        let tex = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ring-zones"),
            size: wgpu::Extent3d {
                width: W,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: mips.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (level, m) in mips.iter().enumerate() {
            let w = (W >> level).max(1);
            gpu.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &tex,
                    mip_level: level as u32,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                m,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w * 4),
                    rows_per_image: Some(1),
                },
                wgpu::Extent3d {
                    width: w,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
        }
        let view = tex.create_view(&Default::default());
        let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ring-zones-bg"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.fallback_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&self.fallback_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.zone_rings[ring] = Some(group.clone());
        group
    }

    /// Bind group for a set of maps; unavailable maps bind a 1x1 transparent texel.
    pub fn group(&mut self, gpu: &GpuContext, set: TexSet) -> wgpu::BindGroup {
        if let Some(g) = self.groups.get(&set) {
            return g.clone();
        }
        let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("surface-bg"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(self.view_of(set[0])),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(self.view_of(set[1])),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(self.view_of(set[2])),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.groups.insert(set, group.clone());
        group
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mip_chain_halves_to_one_texel_and_keeps_length_consistent() {
        let (w, h) = (8u32, 2u32);
        let rgba = vec![200u8; (w * h * 4) as usize];
        let mips = mip_chain(w, h, rgba);
        // 8x2, 4x1, 2x1, 1x1
        assert_eq!(mips.len(), 4);
        assert_eq!(mips[1].len(), 4 * 4);
        assert_eq!(mips[3].len(), 4);
        // a flat image stays flat through the box filter
        assert!(mips.iter().flatten().all(|&b| b == 200));
    }
}
