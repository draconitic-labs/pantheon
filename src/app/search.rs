//! Go-to-anything: planets, moons, spacecraft, named stars, exoplanets and their
//! hosts, galaxies, clusters, and places that are regions rather than objects.

use glam::DVec3;

use super::ui::fmt_words;
use crate::atlas::ObjectRef;
use crate::data::Universe;
use crate::frames::{KPC, MPC};
use crate::sim::{MOONS, PLANETS};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Dest {
    Obj(ObjectRef),
    Place {
        center: DVec3,
        width_m: f64,
        name: &'static str,
    },
}

#[derive(Clone)]
pub struct Entry {
    pub name: String,
    pub lower: String,
    pub detail: String,
    /// lower sorts first among equally good text matches
    pub rank: u8,
    pub dest: Dest,
}

pub struct SearchState {
    pub open: bool,
    pub query: String,
    pub selected: usize,
    entries: Vec<Entry>,
    last_query: Option<String>,
    results: Vec<usize>,
    suggestions: Vec<usize>,
}

fn entry(name: impl Into<String>, detail: impl Into<String>, rank: u8, dest: Dest) -> Entry {
    let name = name.into();
    Entry {
        lower: name.to_lowercase(),
        name,
        detail: detail.into(),
        rank,
        dest,
    }
}

/// Regions you can fly to that aren't a single object.
fn places(u: &Universe) -> Vec<Entry> {
    let gc = u.gal.center;
    let ga = u
        .nodes
        .iter()
        .find(|n| n.name.contains("Great Attractor"))
        .map(|n| n.pos)
        .unwrap_or(DVec3::ZERO);
    let p = |name: &'static str, detail: &str, center: DVec3, width_m: f64| {
        entry(
            name,
            detail,
            1,
            Dest::Place {
                center,
                width_m,
                name,
            },
        )
    };
    vec![
        p("Solar System", "the eight planets", DVec3::ZERO, 1.4e13),
        p("Inner planets", "Mercury to Mars", DVec3::ZERO, 8.0e11),
        p(
            "Asteroid belt",
            "between Mars and Jupiter",
            DVec3::ZERO,
            1.4e12,
        ),
        p("Kuiper belt", "beyond Neptune", DVec3::ZERO, 1.8e13),
        p(
            "Heliosphere",
            "where the solar wind ends",
            DVec3::ZERO,
            8.0e13,
        ),
        p(
            "Oort cloud",
            "a theoretical shell of comets",
            DVec3::ZERO,
            3.5e16,
        ),
        p(
            "Nearest stars",
            "the Sun's closest neighbours",
            DVec3::ZERO,
            6.0e17,
        ),
        p(
            "Solar neighbourhood",
            "the nearby stars of this atlas",
            DVec3::ZERO,
            8.0e18,
        ),
        p("Milky Way", "our galaxy", gc, 2.6e21),
        p("Galactic centre", "Sagittarius A*", gc, 3.0e20),
        p(
            "Local Group",
            "the Milky Way, Andromeda and their dwarfs",
            DVec3::ZERO,
            7.0e22,
        ),
        p(
            "Laniakea",
            "our supercluster, around the Great Attractor",
            ga,
            5.0e24,
        ),
        p(
            "Cosmic web",
            "the largest structure there is",
            DVec3::ZERO,
            8.0e24,
        ),
    ]
}

impl SearchState {
    pub fn new(u: &Universe) -> Self {
        let mut e: Vec<Entry> = Vec::new();
        e.push(entry("Sun", "our star", 0, Dest::Obj(ObjectRef::Sun)));
        for (i, p) in PLANETS.iter().enumerate() {
            e.push(entry(
                p.name,
                format!("planet, {:.2} AU from the Sun", p.el[0]),
                0,
                Dest::Obj(ObjectRef::Planet(i)),
            ));
        }
        for (i, m) in MOONS.iter().enumerate() {
            e.push(entry(
                m.name,
                format!("moon of {}", PLANETS[m.planet].name),
                1,
                Dest::Obj(ObjectRef::Moon(i)),
            ));
        }
        e.extend(places(u));
        for (i, c) in u.craft.iter().enumerate() {
            e.push(entry(
                c.name,
                "spacecraft",
                1,
                Dest::Obj(ObjectRef::Craft(i)),
            ));
        }
        for (i, g) in u.galaxies.iter().enumerate() {
            e.push(entry(
                g.name,
                format!("galaxy, {}", fmt_words(g.dist_kpc * KPC)),
                2,
                Dest::Obj(ObjectRef::Galaxy(i)),
            ));
        }
        for (i, n) in u.nodes.iter().enumerate() {
            e.push(entry(
                n.name,
                format!("galaxy cluster, {}", fmt_words(n.dist_mpc * MPC)),
                2,
                Dest::Obj(ObjectRef::Node(i)),
            ));
        }
        for (i, s) in u.stars.iter().enumerate() {
            if let Some(n) = s.name {
                e.push(entry(
                    n,
                    format!("star, {:.1} light-years", s.dist_pc * 3.2616),
                    2,
                    Dest::Obj(ObjectRef::Star(i)),
                ));
            }
        }
        for (i, s) in u.exos.iter().enumerate() {
            let n = s.planets.len();
            let plural = if n == 1 { "planet" } else { "planets" };
            // findable even with no distance, so picking it can say why it can't be placed
            let placed = if s.dist_pc.is_some() {
                ""
            } else {
                ", no measured distance"
            };
            e.push(entry(
                &s.host,
                format!("star with {n} {plural}{placed}"),
                3,
                Dest::Obj(ObjectRef::ExoHost(i)),
            ));
            for (k, p) in s.planets.iter().enumerate() {
                e.push(entry(
                    &p.name,
                    format!("exoplanet of {}", s.host),
                    4,
                    Dest::Obj(ObjectRef::ExoPlanet(i, k)),
                ));
            }
        }
        let suggestions = [
            "Earth",
            "Moon",
            "Mars",
            "Saturn",
            "Sun",
            "Proxima Centauri",
            "TRAPPIST-1",
            "Milky Way",
            "Andromeda (M31)",
            "Virgo Cluster",
            "Cosmic web",
        ]
        .iter()
        .filter_map(|want| e.iter().position(|x| x.name == *want))
        .collect();
        Self {
            open: false,
            query: String::new(),
            selected: 0,
            entries: e,
            last_query: None,
            results: Vec::new(),
            suggestions,
        }
    }

    pub fn open_palette(&mut self) {
        self.open = true;
        self.query.clear();
        self.selected = 0;
        self.last_query = None;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.query.clear();
        self.selected = 0;
    }

    /// The current result list: suggestions when the box is empty.
    pub fn results(&mut self) -> Vec<&Entry> {
        let q = self.query.trim().to_lowercase();
        if q.is_empty() {
            return self.suggestions.iter().map(|&i| &self.entries[i]).collect();
        }
        if self.last_query.as_deref() != Some(q.as_str()) {
            self.results = rank(&self.entries, &q, 9);
            self.last_query = Some(q);
            self.selected = 0;
        }
        self.results.iter().map(|&i| &self.entries[i]).collect()
    }
}

/// Starts-with beats word-start beats anywhere; then by how prominent the thing is.
fn rank(entries: &[Entry], q: &str, limit: usize) -> Vec<usize> {
    let mut hits: Vec<(u32, u32, usize)> = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        let Some(pos) = e.lower.find(q) else { continue };
        let tier = if pos == 0 {
            if e.lower.len() == q.len() {
                0
            } else {
                1
            }
        } else {
            let before = e.lower[..pos].chars().last().unwrap_or(' ');
            if !before.is_alphanumeric() {
                2
            } else {
                3
            }
        };
        hits.push((tier * 10 + e.rank as u32, e.lower.len() as u32, i));
    }
    hits.sort_unstable();
    hits.truncate(limit);
    hits.into_iter().map(|h| h.2).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str, rank_: u8) -> Entry {
        entry(name, "", rank_, Dest::Obj(ObjectRef::Sun))
    }

    fn names(entries: &[Entry], q: &str) -> Vec<String> {
        rank(entries, q, 9)
            .into_iter()
            .map(|i| entries[i].name.clone())
            .collect()
    }

    #[test]
    fn exact_beats_prefix_beats_word_beats_substring() {
        let es = vec![
            e("Moonstone", 0),
            e("Moon", 1),
            e("Blue Moon", 0),
            e("Harmoon", 0),
        ];
        assert_eq!(
            names(&es, "moon"),
            ["Moon", "Moonstone", "Blue Moon", "Harmoon"]
        );
    }

    #[test]
    fn prominent_things_rank_above_obscure_ones() {
        let es = vec![e("Saturn-like exoplanet", 4), e("Saturn", 0)];
        assert_eq!(names(&es, "sat")[0], "Saturn");
    }

    #[test]
    fn no_match_is_empty_and_limit_holds() {
        let es: Vec<Entry> = (0..30).map(|k| e(&format!("Star {k}"), 2)).collect();
        assert!(names(&es, "galaxy").is_empty());
        assert_eq!(rank(&es, "star", 9).len(), 9);
    }

    #[test]
    fn trappist_is_found_and_suggested() {
        let u = crate::data::Universe::load();
        let s = SearchState::new(&u);
        // the search box lowercases what's typed before ranking
        assert_eq!(names(&s.entries, "trappist")[0], "TRAPPIST-1");
        assert!(s
            .suggestions
            .iter()
            .any(|i| s.entries[*i].name == "TRAPPIST-1"));
    }

    #[test]
    fn a_host_with_no_distance_is_found_and_says_so() {
        let u = crate::data::Universe::load();
        let s = SearchState::new(&u);
        let Some(host) = u.exos.iter().find(|x| x.dist_pc.is_none()) else {
            return;
        };
        let e = s.entries.iter().find(|e| e.name == host.host).unwrap();
        assert!(e.detail.contains("no measured distance"), "{}", e.detail);
    }

    #[test]
    fn queries_with_non_ascii_names_do_not_panic() {
        let es = vec![e("α Centauri", 2), e("ε Eridani", 2)];
        assert_eq!(names(&es, "centauri"), ["α Centauri"]);
        assert_eq!(names(&es, "ε"), ["ε Eridani"]);
    }
}
