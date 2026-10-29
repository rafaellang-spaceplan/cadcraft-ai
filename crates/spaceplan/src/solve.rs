//! Building one layout option: reserve circulation, place the rooms one by one where the
//! option's strategy wants them, then fill what is left with workstation benches.
//!
//! Everything here works in local metres: the floor rotated so its longest edge is horizontal
//! ([`Frame`]), so rooms and benches are axis-aligned rectangles.

use cadcraft_geom::{Vec2, shoelace};
use serde::{Deserialize, Serialize};

use crate::grid::{CORRIDOR, FREE, Field, Grid, Rect, TAKEN, poly_dist};
use crate::program::{Lang, Program, SpaceKind, Zone};
use crate::rng::Rng;
use crate::{Error, Floor};

/// Most vertices in the floor outline and in each hole.
pub const MAX_VERTICES: usize = 20_000;
/// Most holes (cores, shafts, columns).
pub const MAX_HOLES: usize = 5_000;
/// Largest floor side in metres (anything larger is almost certainly a units mistake).
pub const MAX_FLOOR_SIDE: f64 = 1_000.0;
/// Most grid cells: the cell grows on large floors to stay under it.
const MAX_CELLS: f64 = 1_200_000.0;
/// Holes at least this large (m²) are a core (lifts, stairs, toilets): circulation runs around
/// them and "core" rooms gather next to them. Smaller ones are columns.
const CORE_AREA: f64 = 4.0;
/// Chair zone behind a desk and clear aisle at the end of a bench, metres.
const CHAIR_ZONE: f64 = 0.9;
const BENCH_END: f64 = 0.6;
/// Desks this close to the façade (metres) count as having daylight.
pub const DAYLIGHT_DEPTH: f64 = 6.0;

/// Local metres ↔ drawing coordinates.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub origin: Vec2,
    pub angle: f64,
    pub unit: f64,
}

impl Frame {
    pub fn local(&self, p: Vec2) -> Vec2 {
        (p - self.origin).rotate(-self.angle) / self.unit
    }
    pub fn world(&self, p: Vec2) -> Vec2 {
        self.origin + (p * self.unit).rotate(self.angle)
    }
}

/// Where an option puts the rooms whose program line has no zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RoomPlacement {
    /// Around the core, leaving the façade to the workstations.
    Core,
    /// Along the façade.
    Facade,
    /// Grouped at the start of the floor's long axis.
    StartEnd,
    /// Grouped at the far end of the floor's long axis.
    FarEnd,
}

/// Which way benches run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BenchAxis {
    /// Parallel to the floor's long axis.
    Along,
    /// Across it.
    Across,
}

/// The choices that make one option differ from another.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Strategy {
    pub rooms: RoomPlacement,
    pub benches: BenchAxis,
    /// Desks per side of a bench.
    pub per_side: u32,
    /// A main corridor along the long axis.
    pub spine: bool,
    /// Variation number: different random tie-breaks for the same choices.
    pub variant: u32,
}

impl Strategy {
    /// A one-line description for the option's title.
    pub fn describe(&self, lang: Lang) -> String {
        let rooms = match (lang, self.rooms) {
            (Lang::En, RoomPlacement::Core) => "rooms at the core",
            (Lang::En, RoomPlacement::Facade) => "rooms on the façade",
            (Lang::En, RoomPlacement::StartEnd) => "rooms grouped at one end",
            (Lang::En, RoomPlacement::FarEnd) => "rooms grouped at the other end",
            (Lang::PtBr, RoomPlacement::Core) => "salas junto ao núcleo",
            (Lang::PtBr, RoomPlacement::Facade) => "salas na fachada",
            (Lang::PtBr, RoomPlacement::StartEnd) => "salas agrupadas numa extremidade",
            (Lang::PtBr, RoomPlacement::FarEnd) => "salas agrupadas na outra extremidade",
        };
        let axis = match (lang, self.benches) {
            (Lang::En, BenchAxis::Along) => "lengthwise benches",
            (Lang::En, BenchAxis::Across) => "crosswise benches",
            (Lang::PtBr, BenchAxis::Along) => "estações no sentido longitudinal",
            (Lang::PtBr, BenchAxis::Across) => "estações no sentido transversal",
        };
        let n = self.per_side * 2;
        let bench = match lang {
            Lang::En => format!("islands of {n}"),
            Lang::PtBr => format!("ilhas de {n}"),
        };
        let spine = match (lang, self.spine) {
            (_, false) => "",
            (Lang::En, true) => " · central corridor",
            (Lang::PtBr, true) => " · circulação central",
        };
        format!("{rooms} · {axis} · {bench}{spine}")
    }
}

/// The option strategies to try, in a seeded order: every combination of room placement, bench
/// axis, bench size and spine, then repeats with other tie-breaks until `count`.
pub fn strategies(program: &Program, count: usize, seed: u64) -> Vec<Strategy> {
    let sizes: Vec<u32> = match program.workstations.per_side {
        Some(n) => vec![n],
        None => vec![3, 2, 4],
    };
    let mut base = Vec::new();
    for rooms in [RoomPlacement::Core, RoomPlacement::Facade, RoomPlacement::StartEnd, RoomPlacement::FarEnd] {
        for benches in [BenchAxis::Along, BenchAxis::Across] {
            for &per_side in &sizes {
                for spine in [false, true] {
                    base.push(Strategy { rooms, benches, per_side, spine, variant: 0 });
                }
            }
        }
    }
    // Fisher–Yates with the seed, so different seeds explore different options first.
    let mut rng = Rng::new(seed ^ 0x5eed);
    for i in (1..base.len()).rev() {
        let j = (rng.next_u64() % (i as u64 + 1)) as usize;
        base.swap(i, j);
    }
    let mut out = Vec::new();
    let mut variant = 0u32;
    while out.len() < count && variant < 64 {
        out.extend(base.iter().map(|s| Strategy { variant, ..*s }).take(count - out.len()));
        variant += 1;
    }
    out
}

/// A room placed in an option.
#[derive(Clone, Debug)]
pub struct PlacedRoom {
    pub spec: usize,
    pub rect: Rect,
    pub door: Option<usize>,
    pub enclosed: bool,
}

/// A workstation bench: `per_side` desks on `sides` (1 or 2) rows back to back.
#[derive(Clone, Copy, Debug)]
pub struct Bench {
    /// The desks' footprint.
    pub rect: Rect,
    /// The bench with its chair zones and end aisles.
    pub envelope: Rect,
    pub per_side: u32,
    pub sides: u32,
    pub along_x: bool,
}

impl Bench {
    pub fn desks(&self) -> u32 {
        self.per_side * self.sides
    }
}

/// One option, in local metres.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub strategy: Strategy,
    pub rooms: Vec<PlacedRoom>,
    pub benches: Vec<Bench>,
    pub usable: f64,
}

/// The floor prepared once for all options.
pub struct Context<'a> {
    pub program: &'a Program,
    pub frame: Frame,
    pub min: Vec2,
    pub max: Vec2,
    cores: Vec<Rect>,
    columns: usize,
    grid: Grid,
    facade: Field,
    core: Option<Field>,
    entrance: Option<Vec2>,
    /// The floor is longer along local x than along y.
    pub long_x: bool,
    stride: f64,
}

fn finite_poly(p: &[Vec2]) -> bool {
    p.iter().all(|v| v.is_finite())
}

impl<'a> Context<'a> {
    pub fn new(floor: &Floor, program: &'a Program) -> Result<Self, Error> {
        if !floor.unit.is_finite() || !(1e-4..=1e7).contains(&floor.unit) {
            return Err(Error::Floor("drawing units per metre must be a positive number".into()));
        }
        if floor.outline.len() < 3 || floor.outline.len() > MAX_VERTICES || !finite_poly(&floor.outline) {
            return Err(Error::Floor(format!("the floor outline needs 3 to {MAX_VERTICES} finite vertices")));
        }
        if floor.holes.len() > MAX_HOLES {
            return Err(Error::Floor(format!("at most {MAX_HOLES} cores, shafts and columns")));
        }
        // Rotate so the longest outline edge is horizontal: most floors are rectangles or close to.
        let n = floor.outline.len();
        let mut longest = (0.0, 0.0);
        for i in 0..n {
            if let (Some(a), Some(b)) = (floor.outline.get(i), floor.outline.get((i + 1) % n)) {
                let l = a.dist(*b);
                if l > longest.0 {
                    longest = (l, a.angle_to(*b));
                }
            }
        }
        let quarter = std::f64::consts::FRAC_PI_2;
        let angle = longest.1 - ((longest.1 + quarter / 2.0) / quarter).floor() * quarter;
        let sum = floor.outline.iter().fold(Vec2::ZERO, |a, p| a + *p);
        let frame = Frame { origin: sum / n as f64, angle, unit: floor.unit };
        let outline: Vec<Vec2> = floor.outline.iter().map(|p| frame.local(*p)).collect();
        let holes: Vec<Vec<Vec2>> = floor
            .holes
            .iter()
            .filter(|h| h.len() >= 3 && h.len() <= MAX_VERTICES && finite_poly(h))
            .map(|h| h.iter().map(|p| frame.local(*p)).collect())
            .collect();
        let (min, max) = outline
            .iter()
            .fold((Vec2::new(f64::INFINITY, f64::INFINITY), Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY)), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
        let (w, h) = (max.x - min.x, max.y - min.y);
        if !(w.is_finite() && h.is_finite()) || w > MAX_FLOOR_SIDE || h > MAX_FLOOR_SIDE {
            return Err(Error::Floor(format!("the floor is {w:.0} × {h:.0} m: larger than {MAX_FLOOR_SIDE:.0} m (check the drawing units)")));
        }
        if w < 2.0 || h < 2.0 || shoelace(&outline).abs() < 10.0 {
            return Err(Error::Floor(format!("the floor is {w:.2} × {h:.2} m: too small for a layout (check the drawing units)")));
        }
        let cell = (w * h / MAX_CELLS).sqrt().max(0.1);
        let grid = Grid::new(&outline, &holes, min, max, cell);
        let cores: Vec<Vec<Vec2>> = holes.iter().filter(|h| shoelace(h).abs() >= CORE_AREA).cloned().collect();
        let step = (w * h / 40_000.0).sqrt().max(0.5);
        let facade = Field::sample(min, max, step, |p| poly_dist(&outline, p));
        let core =
            (!cores.is_empty()).then(|| Field::sample(min, max, step, |p| cores.iter().map(|c| poly_dist(c, p)).fold(f64::INFINITY, f64::min)));
        let core_rects: Vec<Rect> = cores
            .iter()
            .map(|c| {
                let (lo, hi) =
                    c.iter().fold((Vec2::new(f64::INFINITY, f64::INFINITY), Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY)), |(lo, hi), p| {
                        (lo.min(*p), hi.max(*p))
                    });
                Rect::new(lo.x, lo.y, hi.x, hi.y)
            })
            .collect();
        let entrance = floor.entrance.filter(|p| p.is_finite()).map(|p| frame.local(p));
        let core_rects_len = core_rects.len();
        Ok(Context {
            program,
            frame,
            min,
            max,
            columns: holes.len().saturating_sub(core_rects_len),
            cores: core_rects,
            grid,
            facade,
            core,
            entrance,
            long_x: w >= h,
            // A whole number of cells, so rooms line up with the grid.
            stride: ((w * h / 12_000.0).sqrt().max(0.3) / cell).round().max(1.0) * cell,
        })
    }

    /// (cores, columns).
    pub fn hole_counts(&self) -> (usize, usize) {
        (self.cores.len(), self.columns)
    }

    pub fn usable_area(&self) -> f64 {
        self.grid.usable_area()
    }

    /// Distance to the façade (outline), metres.
    pub fn facade_dist(&self, p: Vec2) -> f64 {
        self.facade.at(p)
    }

    /// Build the option for one strategy.
    pub fn run(&self, st: &Strategy, seed: u64) -> Candidate {
        let mut g = self.grid.clone();
        self.reserve_circulation(&mut g, st);
        let mut rng = Rng::new(seed ^ hash_strategy(st));
        let mut order: Vec<usize> = Vec::new();
        for (i, s) in self.program.spaces.iter().enumerate() {
            order.extend(std::iter::repeat_n(i, s.count as usize));
        }
        let area = |i: &usize| self.program.spaces.get(*i).map_or(0.0, |s| -(s.size().0 * s.size().1));
        // The reception anchors the entrance; then the largest rooms first, while there's room.
        order.sort_by(|a, b| {
            let ra = self.program.spaces.get(*a).is_some_and(|s| s.kind == SpaceKind::Reception);
            let rb = self.program.spaces.get(*b).is_some_and(|s| s.kind == SpaceKind::Reception);
            rb.cmp(&ra).then(area(a).total_cmp(&area(b)))
        });
        let mut rooms = Vec::new();
        for i in order {
            if let Some(r) = self.place_room(&mut g, i, st, &mut rng, &rooms) {
                rooms.push(r);
            }
        }
        let benches = self.fill_benches(&mut g, st);
        Candidate { strategy: *st, rooms, benches, usable: self.grid.usable_area() }
    }

    fn reserve_circulation(&self, g: &mut Grid, st: &Strategy) {
        let mc = self.program.main_corridor;
        for c in &self.cores {
            g.mark(&c.expand(mc, mc), Some(FREE), CORRIDOR);
        }
        if st.spine {
            let c = self.cores.first().map_or_else(|| Rect::new(self.min.x, self.min.y, self.max.x, self.max.y).center(), Rect::center);
            let strip = if self.long_x {
                Rect::new(self.min.x, c.y - mc / 2.0, self.max.x, c.y + mc / 2.0)
            } else {
                Rect::new(c.x - mc / 2.0, self.min.y, c.x + mc / 2.0, self.max.y)
            };
            g.mark(&strip, Some(FREE), CORRIDOR);
        }
        if let Some(e) = self.entrance {
            g.mark(&Rect::new(e.x - mc, e.y - mc, e.x + mc, e.y + mc), Some(FREE), CORRIDOR);
        }
        g.rebuild();
    }

    /// Lower is better: how well a room centred at `p` sits in `zone`.
    fn zone_cost(&self, zone: Target, p: Vec2) -> f64 {
        let along = if self.long_x { p.x - self.min.x } else { p.y - self.min.y };
        let length = if self.long_x { self.max.x - self.min.x } else { self.max.y - self.min.y };
        match zone {
            Target::Core => match &self.core {
                Some(f) => f.at(p),
                // No core: the interior, away from the windows.
                None => -self.facade.at(p),
            },
            Target::Facade => self.facade.at(p),
            Target::Start => along,
            Target::End => length - along,
            Target::Entrance => match (self.entrance, &self.core) {
                (Some(e), _) => p.dist(e),
                (None, Some(f)) => f.at(p),
                (None, None) => along,
            },
        }
    }

    fn target(&self, zone: Zone, st: &Strategy) -> Target {
        match zone {
            Zone::Core => Target::Core,
            Zone::Facade => Target::Facade,
            Zone::Entrance => Target::Entrance,
            Zone::Any => match st.rooms {
                RoomPlacement::Core => Target::Core,
                RoomPlacement::Facade => Target::Facade,
                RoomPlacement::StartEnd => Target::Start,
                RoomPlacement::FarEnd => Target::End,
            },
        }
    }

    fn place_room(&self, g: &mut Grid, idx: usize, st: &Strategy, rng: &mut Rng, placed: &[PlacedRoom]) -> Option<PlacedRoom> {
        let spec = self.program.spaces.get(idx)?;
        let (w, d) = spec.size();
        let enclosed = spec.is_enclosed();
        let target = self.target(spec.zone(), st);
        let corridor = self.program.corridor;
        let jitter = if st.variant == 0 { 1.0 } else { 2.5 };
        let attach_depth = self.stride + g.cell;
        let center = Rect::new(self.min.x, self.min.y, self.max.x, self.max.y).center();
        let same: Vec<Vec2> = placed.iter().filter(|r| r.spec == idx).map(|r| r.rect.center()).collect();
        let dims: &[(f64, f64)] = if (w - d).abs() < 1e-6 { &[(w, d)] } else { &[(w, d), (d, w)] };
        let mut best: Option<(f64, Rect, Option<usize>, [bool; 4])> = None;
        for &(rw, rh) in dims {
            let mut y = self.min.y;
            while y + rh <= self.max.y + 1e-9 {
                let mut x = self.min.x;
                while x + rw <= self.max.x + 1e-9 {
                    let r = Rect::at(x, y, rw, rh);
                    x += self.stride;
                    if !g.is_free(&r) {
                        continue;
                    }
                    let c = r.center();
                    let mut cost = self.zone_cost(target, c);
                    if let Some(near) = same.iter().map(|p| p.dist(c)).min_by(f64::total_cmp) {
                        cost += 0.15 * near;
                    }
                    // Even leaning on four walls with no jitter, this spot can't win: skip the
                    // costly neighbourhood checks.
                    if best.as_ref().is_some_and(|b| cost - 2.4 >= b.0) {
                        continue;
                    }
                    let attached: [bool; 4] = std::array::from_fn(|s| g.solid_fraction(&r.outside(s, attach_depth)) >= 0.6);
                    let door =
                        (0..4).filter(|&s| !attached.get(s).copied().unwrap_or(true) && g.is_passable(&r.outside(s, corridor))).max_by(|a, b| {
                            let da = Rect::normal(*a).dot(center - r.center());
                            let db = Rect::normal(*b).dot(center - r.center());
                            da.total_cmp(&db)
                        });
                    if door.is_none() || (enclosed && !attached.iter().any(|a| *a)) {
                        continue;
                    }
                    cost += jitter * rng.next_f64() - 0.6 * attached.iter().filter(|a| **a).count() as f64;
                    if best.as_ref().is_none_or(|b| cost < b.0) {
                        best = Some((cost, r, door, attached));
                    }
                }
                y += self.stride;
            }
        }
        let (_, mut rect, door, attached) = best?;
        // Slide against the walls it leans on: the scan steps by the stride.
        for s in (0..4).filter(|s| attached.get(*s).copied().unwrap_or(false)) {
            let step = Rect::normal(s) * g.cell;
            for _ in 0..((self.stride / g.cell).ceil() as usize + 1) {
                let next = rect.translate(step.x, step.y);
                if !g.is_free(&next) {
                    break;
                }
                rect = next;
            }
        }
        g.mark(&rect, Some(FREE), TAKEN);
        if let Some(s) = door {
            g.mark(&rect.outside(s, corridor), Some(FREE), CORRIDOR);
        }
        g.rebuild();
        Some(PlacedRoom { spec: idx, rect, door: if enclosed { door } else { None }, enclosed })
    }

    fn fill_benches(&self, g: &mut Grid, st: &Strategy) -> Vec<Bench> {
        let ws = &self.program.workstations;
        let mut remaining = ws.count;
        let mut out = Vec::new();
        if remaining == 0 {
            return out;
        }
        let along_x = (st.benches == BenchAxis::Along) == self.long_x;
        let n = st.per_side.max(1);
        // Full benches everywhere first, then smaller ones in the gaps.
        let mut configs = vec![(n, 2)];
        configs.extend((1..n).rev().map(|k| (k, 2)));
        configs.extend((1..=n).rev().map(|k| (k, 1)));
        for (k, sides) in configs {
            if remaining == 0 {
                break;
            }
            let (along, across) = (f64::from(k) * ws.desk_width, f64::from(sides) * ws.desk_depth);
            let (fw, fh, ex, ey) = if along_x { (along, across, BENCH_END, CHAIR_ZONE) } else { (across, along, CHAIR_ZONE, BENCH_END) };
            let (ew, eh) = (fw + 2.0 * ex, fh + 2.0 * ey);
            let step = g.cell;
            let (outer_n, inner_n) = if along_x {
                (((self.max.y - self.min.y - eh) / step).floor(), ((self.max.x - self.min.x - ew) / step).floor())
            } else {
                (((self.max.x - self.min.x - ew) / step).floor(), ((self.max.y - self.min.y - eh) / step).floor())
            };
            if outer_n < 0.0 || inner_n < 0.0 {
                continue;
            }
            let (outer_n, inner_n) = (outer_n as usize, inner_n as usize);
            for o in 0..=outer_n {
                let mut i = 0usize;
                while i <= inner_n {
                    if remaining < k * sides {
                        break;
                    }
                    let (x, y) = if along_x {
                        (self.min.x + i as f64 * step, self.min.y + o as f64 * step)
                    } else {
                        (self.min.x + o as f64 * step, self.min.y + i as f64 * step)
                    };
                    let env = Rect::at(x, y, ew, eh);
                    if g.is_free(&env) {
                        g.mark(&env, Some(FREE), TAKEN);
                        g.rebuild();
                        out.push(Bench { rect: env.expand(-ex, -ey), envelope: env, per_side: k, sides, along_x });
                        remaining -= k * sides;
                        i += ((if along_x { ew } else { eh }) / step).floor().max(1.0) as usize;
                    } else {
                        i += 1;
                    }
                }
            }
        }
        out
    }
}

#[derive(Clone, Copy, Debug)]
enum Target {
    Core,
    Facade,
    Start,
    End,
    Entrance,
}

/// FNV-1a over the strategy, for its random stream.
fn hash_strategy(st: &Strategy) -> u64 {
    let parts = [st.rooms as u64, st.benches as u64, u64::from(st.per_side), u64::from(st.spine), u64::from(st.variant)];
    parts.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, p| (h ^ p).wrapping_mul(0x0100_0000_01b3))
}

/// A fingerprint of the geometry, to drop options that came out identical.
pub fn fingerprint(c: &Candidate) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut eat = |v: f64| {
        h = (h ^ ((v * 10.0).round() as i64 as u64)).wrapping_mul(0x0100_0000_01b3);
    };
    for r in &c.rooms {
        eat(r.spec as f64);
        for v in [r.rect.x0, r.rect.y0, r.rect.x1, r.rect.y1] {
            eat(v);
        }
    }
    for b in &c.benches {
        for v in [b.rect.x0, b.rect.y0, b.rect.x1, b.rect.y1] {
            eat(v);
        }
    }
    h
}
