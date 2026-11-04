//! The beginning of the real scene model.
//!
//! A `FrameGraph` holds parent-linked frames whose origins resolve through
//! time-dependent providers. Moons are children of planet frames, exoplanets
//! children of host frames — positions compose hierarchically instead of
//! being special-cased. `atlas::obj_pos` now routes through this graph
//! (parity-tested against the legacy formulas below); camera-relative origin
//! rebasing builds on this.

use glam::DVec3;

use crate::data::Universe;
use crate::sim;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FrameId(pub u32);

/// How a frame's origin is expressed *in its parent frame* at a given epoch.
#[derive(Clone, Copy, Debug)]
pub enum Provider {
    /// the root: heliocentric ecliptic J2000
    Root,
    /// planet center from the analytic ephemeris (Earth = derived center)
    PlanetEphemeris(usize),
    /// moon on its parent-equator conic — parent must be the planet frame
    MoonOrbit(usize),
    /// spacecraft from the cached Horizons track (freezes at last sample,
    /// matching legacy craft semantics; coverage display is the caller's job)
    CraftTrack(usize),
    /// Gaia star, proper-motion propagated
    Star(usize),
    /// exoplanet host from the catalog position
    ExoHost(usize),
    /// exoplanet on its illustrative conic — parent must be the host frame
    ExoPlanetOrbit(usize, usize),
    /// fixed catalog point (galaxies, cosmic nodes, Sgr A*)
    Fixed(DVec3),
}

pub struct FrameNode {
    pub name: String,
    pub parent: Option<FrameId>,
    pub provider: Provider,
}

pub struct FrameGraph {
    pub nodes: Vec<FrameNode>,
    root: FrameId,
    sun: FrameId,
    planets: Vec<FrameId>,
    moons: Vec<FrameId>,
    craft: Vec<FrameId>,
    galactic_center: FrameId,
}

impl FrameGraph {
    pub fn build(u: &Universe) -> FrameGraph {
        let mut nodes = Vec::new();
        let mut push = |name: String, parent: Option<FrameId>, provider: Provider| -> FrameId {
            nodes.push(FrameNode {
                name,
                parent,
                provider,
            });
            FrameId(nodes.len() as u32 - 1)
        };
        let root = push("heliocentric ecliptic J2000".into(), None, Provider::Root);
        let sun = push("Sun".into(), Some(root), Provider::Fixed(DVec3::ZERO));
        let planets: Vec<FrameId> = sim::PLANETS
            .iter()
            .enumerate()
            .map(|(i, p)| push(p.name.into(), Some(root), Provider::PlanetEphemeris(i)))
            .collect();
        let moons: Vec<FrameId> = sim::MOONS
            .iter()
            .enumerate()
            .map(|(mi, m)| {
                push(
                    m.name.into(),
                    Some(planets[m.planet]),
                    Provider::MoonOrbit(mi),
                )
            })
            .collect();
        let craft: Vec<FrameId> = u
            .craft
            .iter()
            .enumerate()
            .map(|(ci, c)| push(c.name.into(), Some(root), Provider::CraftTrack(ci)))
            .collect();
        let galactic_center = push(
            "Sagittarius A*".into(),
            Some(root),
            Provider::Fixed(u.gal.center),
        );
        FrameGraph {
            nodes,
            root,
            sun,
            planets,
            moons,
            craft,
            galactic_center,
        }
    }

    pub fn root(&self) -> FrameId {
        self.root
    }
    pub fn sun(&self) -> FrameId {
        self.sun
    }
    pub fn planet(&self, i: usize) -> FrameId {
        self.planets[i]
    }
    pub fn moon(&self, mi: usize) -> FrameId {
        self.moons[mi]
    }
    pub fn craft_frame(&self, ci: usize) -> FrameId {
        self.craft[ci]
    }
    pub fn galactic_center(&self) -> FrameId {
        self.galactic_center
    }

    /// A frame's origin expressed in its *parent* frame at jd (TDB).
    fn local_origin(&self, u: &Universe, provider: Provider, jd: f64) -> DVec3 {
        match provider {
            Provider::Root => DVec3::ZERO,
            Provider::Fixed(p) => p,
            Provider::PlanetEphemeris(i) => sim::body_pos(i, jd),
            Provider::MoonOrbit(mi) => sim::moon_rel(mi, jd),
            Provider::CraftTrack(ci) => u.craft[ci].track.pos_at(jd).unwrap_or_else(|| {
                u.craft[ci]
                    .track
                    .samples
                    .last()
                    .map(|s| s.1)
                    .unwrap_or(DVec3::ZERO)
            }),
            Provider::Star(si) => u.stars[si].pos_at(jd),
            Provider::ExoHost(ei) => u.exos[ei].pos,
            Provider::ExoPlanetOrbit(ei, k) => {
                crate::atlas::exo_planet_rel(u, ei, k, jd).unwrap_or(DVec3::ZERO)
            }
        }
    }

    /// Compose the transform chain down to the root.
    pub fn origin_in_root(&self, u: &Universe, id: FrameId, jd: f64) -> DVec3 {
        let mut acc = DVec3::ZERO;
        let mut cur = Some(id);
        while let Some(FrameId(i)) = cur {
            let node = &self.nodes[i as usize];
            acc += self.local_origin(u, node.provider, jd);
            cur = node.parent;
        }
        acc
    }

    /// Resolve a leaf provider that has an implicit parent chain (stars,
    /// exo hosts/planets, catalog points) without materializing a node.
    pub fn leaf_in_root(&self, u: &Universe, provider: Provider, jd: f64) -> DVec3 {
        match provider {
            Provider::ExoPlanetOrbit(ei, _) => {
                self.local_origin(u, Provider::ExoHost(ei), jd) + self.local_origin(u, provider, jd)
            }
            _ => self.local_origin(u, provider, jd),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::J2000;

    fn close(a: DVec3, b: DVec3) -> bool {
        (a - b).length() <= 1e-6 * b.length().max(1.0)
    }

    /// Migration parity (never delete the old path before the
    /// replacement proves equivalence): graph composition must reproduce the
    /// legacy special-case formulas exactly.
    #[test]
    fn graph_matches_legacy_positions() {
        let u = Universe::load();
        let g = FrameGraph::build(&u);
        let jd = J2000 + 9_688.0;
        for i in 0..sim::PLANETS.len() {
            assert!(
                close(g.origin_in_root(&u, g.planet(i), jd), sim::body_pos(i, jd)),
                "planet {i}"
            );
        }
        for mi in 0..sim::MOONS.len() {
            let legacy = sim::body_pos(sim::MOONS[mi].planet, jd) + sim::moon_rel(mi, jd);
            assert!(
                close(g.origin_in_root(&u, g.moon(mi), jd), legacy),
                "moon {mi}"
            );
        }
        for ci in 0..u.craft.len() {
            let legacy = u.craft[ci].track.pos_at(jd).unwrap_or_else(|| {
                u.craft[ci]
                    .track
                    .samples
                    .last()
                    .map(|s| s.1)
                    .unwrap_or(DVec3::ZERO)
            });
            assert!(
                close(g.origin_in_root(&u, g.craft_frame(ci), jd), legacy),
                "craft {ci}"
            );
        }
        // hierarchical leaves: an exoplanet composes host + conic
        if let Some(ei) = u.find_system("TRAPPIST-1") {
            let p = g.leaf_in_root(&u, Provider::ExoPlanetOrbit(ei, 3), jd);
            let legacy = u.exos[ei].pos + crate::atlas::exo_planet_rel(&u, ei, 3, jd).unwrap();
            assert!(close(p, legacy), "exo planet");
        }
        // frames really are parent-linked, not flat
        let moon_node = &g.nodes[g.moon(0).0 as usize];
        assert_eq!(moon_node.parent, Some(g.planet(2)));
    }
}
