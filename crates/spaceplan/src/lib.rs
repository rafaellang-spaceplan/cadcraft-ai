//! CADCraft space planning: office layout options from a floor plate and a space program.
//!
//! Given the floor (its outline, the cores, shafts and columns inside it and, optionally, the
//! entrance) and a program (workstations, meeting rooms, offices, booths, reception, pantry…),
//! [`generate`] builds many candidate layouts with different strategies — rooms at the core or on
//! the façade or grouped at one end, benches lengthwise or crosswise, islands of 4, 6 or 8, with
//! or without a central corridor — scores them against the program, and returns the best distinct
//! ones. It is deterministic: the same input and seed give the same options.
//!
//! The generator is geometry only. Reading a free-text briefing into a [`Program`] is the job of
//! the AI layer above; drawing the options is the engine's `spaceplan` command.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod furnish;
mod grid;
pub mod program;
mod rng;
mod solve;

use cadcraft_geom::Vec2;
use serde::{Deserialize, Serialize};

pub use furnish::PieceKind;
pub use program::{Lang, Program, SpaceKind, SpaceSpec, Workstations, Zone};
pub use solve::{BenchAxis, DAYLIGHT_DEPTH, MAX_FLOOR_SIDE, MAX_HOLES, MAX_VERTICES, RoomPlacement, Strategy};

use grid::Rect;
use solve::{Candidate, Context};

/// Most options returned, and most candidates tried.
pub const MAX_OPTIONS: usize = 50;
pub const MAX_CANDIDATES: usize = 200;

/// Why no layout could be made.
#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// The floor geometry is unusable.
    Floor(String),
    /// The program is invalid.
    Program(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Floor(m) => write!(f, "floor: {m}"),
            Error::Program(m) => write!(f, "program: {m}"),
        }
    }
}

impl std::error::Error for Error {}

/// The floor plate, in drawing coordinates.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Floor {
    /// The usable floor's outline (inside of the façade), closed.
    pub outline: Vec<Vec2>,
    /// Cores, shafts and columns inside it, each closed. Holes of 4 m² or more are cores:
    /// circulation runs around them.
    pub holes: Vec<Vec<Vec2>>,
    /// Where people come in (lift lobby, main door).
    pub entrance: Option<Vec2>,
    /// Drawing units per metre (1000 for millimetres).
    pub unit: f64,
}

/// How many options to make.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub options: usize,
    /// Candidates built before choosing (at least `options`).
    pub candidates: usize,
    pub seed: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { options: 10, candidates: 48, seed: 1 }
    }
}

/// A door in a room's wall: the opening from `hinge` to `end`, the leaf swinging into the room
/// to `swing`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Door {
    pub hinge: Vec2,
    pub end: Vec2,
    pub swing: Vec2,
}

/// A room or open zone of an option, in drawing coordinates.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Room {
    /// Index of its line in the program.
    pub spec: usize,
    pub kind: SpaceKind,
    pub name: String,
    /// The four corners, counter-clockwise.
    pub outline: [Vec2; 4],
    /// Walls: from one side of the door around to the other (open when there's a door, else
    /// the closed outline).
    pub walls: Vec<Vec2>,
    pub enclosed: bool,
    pub door: Option<Door>,
    /// m².
    pub area: f64,
    pub center: Vec2,
    pub seats: u32,
}

/// A piece of furniture in drawing coordinates: `width` along its rotated x axis, `depth` along
/// its y axis (drawing units). Users of desks, counters and chairs are on its −y side.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Piece {
    pub kind: PieceKind,
    pub center: Vec2,
    pub width: f64,
    pub depth: f64,
    pub angle: f64,
}

/// How one program line came out in an option.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceCount {
    pub spec: usize,
    pub kind: SpaceKind,
    pub name: String,
    pub placed: u32,
    pub target: u32,
}

/// What an option achieves. Areas are m².
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Metrics {
    pub workstations: u32,
    pub workstations_target: u32,
    pub rooms: u32,
    pub rooms_target: u32,
    pub spaces: Vec<SpaceCount>,
    pub usable_area: f64,
    pub room_area: f64,
    /// Benches with their chair zones and end aisles.
    pub workstation_area: f64,
    /// Everything else: corridors and unassigned floor.
    pub circulation_area: f64,
    /// Share of workstations within [`DAYLIGHT_DEPTH`] of the façade, 0..1.
    pub daylight: f64,
    /// Usable area per seat at a workstation or private office.
    pub area_per_person: f64,
}

/// One layout option.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayoutOption {
    pub strategy: Strategy,
    pub description: String,
    pub rooms: Vec<Room>,
    pub furniture: Vec<Piece>,
    pub metrics: Metrics,
    /// 0..100: how fully the program is met (workstations 55, rooms 35, daylight 10).
    pub score: f64,
}

/// The result of [`generate`]: the options, best first.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub options: Vec<LayoutOption>,
    pub candidates: usize,
    /// m².
    pub usable_area: f64,
    /// The rotation applied to align the floor's longest edge, radians.
    pub angle: f64,
}

/// Build layout options for `program` on `floor`.
pub fn generate(floor: &Floor, program: &Program, settings: &Settings) -> Result<Plan, Error> {
    let mut program = program.clone();
    program.validate()?;
    let options = settings.options.clamp(1, MAX_OPTIONS);
    let count = settings.candidates.clamp(options, MAX_CANDIDATES);
    let ctx = Context::new(floor, &program)?;
    let build = |st: &Strategy| {
        let c = ctx.run(st, settings.seed);
        let m = metrics(&ctx, &c);
        (score(&m), c, m)
    };
    let strategies = solve::strategies(&program, count, settings.seed);
    // Candidates are independent: build them on all cores (the order of the result doesn't
    // depend on the threads). The web build has one thread.
    #[cfg(not(target_arch = "wasm32"))]
    let mut scored: Vec<(f64, Candidate, Metrics)> = {
        use rayon::prelude::*;
        strategies.par_iter().map(build).collect()
    };
    #[cfg(target_arch = "wasm32")]
    let mut scored: Vec<(f64, Candidate, Metrics)> = strategies.iter().map(build).collect();
    // Best first, but spread over the strategies: each choice an option shares with one already
    // taken costs it a little, so ten options aren't ten near-copies of the best one. Ties keep
    // the seeded strategy order.
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut seen = std::collections::HashSet::new();
    scored.retain(|(_, c, _)| seen.insert(solve::fingerprint(c)));
    let mut picked: Vec<(f64, Candidate, Metrics)> = Vec::new();
    while picked.len() < options && !scored.is_empty() {
        let similarity = |c: &Candidate| {
            picked
                .iter()
                .map(|(_, p, _)| {
                    let (a, b) = (&c.strategy, &p.strategy);
                    f64::from(u8::from(a.rooms == b.rooms)) * 1.2
                        + f64::from(u8::from(a.benches == b.benches)) * 0.8
                        + f64::from(u8::from(a.per_side == b.per_side)) * 0.5
                        + f64::from(u8::from(a.spine == b.spine)) * 0.3
                })
                .sum::<f64>()
        };
        let best = scored
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| (a.0 - similarity(&a.1)).total_cmp(&(b.0 - similarity(&b.1))).then(std::cmp::Ordering::Greater));
        let Some(i) = best.map(|(i, _)| i) else { break };
        picked.push(scored.remove(i));
    }
    // Present them best first.
    picked.sort_by(|a, b| b.0.total_cmp(&a.0));
    let out: Vec<LayoutOption> = picked.into_iter().map(|(score, c, m)| to_world(&ctx, &c, m, score)).collect();
    Ok(Plan { options: out, candidates: count, usable_area: ctx.usable_area(), angle: ctx.frame.angle })
}

fn score(m: &Metrics) -> f64 {
    let ratio = |a: u32, t: u32| if t == 0 { 1.0 } else { (f64::from(a) / f64::from(t)).min(1.0) };
    55.0 * ratio(m.workstations, m.workstations_target) + 35.0 * ratio(m.rooms, m.rooms_target) + 10.0 * m.daylight
}

fn metrics(ctx: &Context, c: &Candidate) -> Metrics {
    let p = ctx.program;
    let ws = &p.workstations;
    let desks: u32 = c.benches.iter().map(solve::Bench::desks).sum();
    let lit: usize = c
        .benches
        .iter()
        .flat_map(|b| furnish::bench(b, ws.desk_width, ws.desk_depth))
        .filter(|piece| piece.kind == PieceKind::Desk && ctx.facade_dist(piece.center) <= DAYLIGHT_DEPTH)
        .count();
    let spaces: Vec<SpaceCount> = p
        .spaces
        .iter()
        .enumerate()
        .map(|(i, s)| SpaceCount {
            spec: i,
            kind: s.kind,
            name: s.display_name(p.lang),
            placed: c.rooms.iter().filter(|r| r.spec == i).count() as u32,
            target: s.count,
        })
        .collect();
    let offices: u32 = spaces.iter().filter(|s| s.kind == SpaceKind::Office).map(|s| s.placed).sum();
    let room_area: f64 = c.rooms.iter().map(|r| r.rect.area()).sum();
    let workstation_area: f64 = c.benches.iter().map(|b| b.envelope.area()).sum();
    Metrics {
        workstations: desks,
        workstations_target: ws.count,
        rooms: c.rooms.len() as u32,
        rooms_target: p.room_count(),
        spaces,
        usable_area: c.usable,
        room_area,
        workstation_area,
        circulation_area: (c.usable - room_area - workstation_area).max(0.0),
        daylight: if desks == 0 { 0.0 } else { lit as f64 / f64::from(desks) },
        area_per_person: c.usable / f64::from((desks + offices).max(1)),
    }
}

fn to_world(ctx: &Context, c: &Candidate, metrics: Metrics, score: f64) -> LayoutOption {
    let p = ctx.program;
    let f = &ctx.frame;
    let world_piece = |lp: furnish::LocalPiece| Piece {
        kind: lp.kind,
        center: f.world(lp.center),
        width: lp.width * f.unit,
        depth: lp.depth * f.unit,
        angle: lp.angle + f.angle,
    };
    let mut furniture = Vec::new();
    let mut rooms = Vec::new();
    for r in &c.rooms {
        let Some(spec) = p.spaces.get(r.spec) else { continue };
        furniture.extend(furnish::room(r, spec).into_iter().map(world_piece));
        let door = r.door.map(|side| door(&r.rect, side, spec.kind));
        let corners = r.rect.corners();
        let walls = match (door, r.door) {
            (Some((hinge, end, _)), Some(side)) => {
                // From the far side of the opening around the room back to the hinge.
                let mut w = vec![end];
                for k in 1..=4 {
                    if let Some(c) = corners.get((side + k) % 4) {
                        w.push(*c);
                    }
                }
                w.push(hinge);
                w
            }
            _ => corners.to_vec(),
        };
        rooms.push(Room {
            spec: r.spec,
            kind: spec.kind,
            name: spec.display_name(p.lang),
            outline: corners.map(|v| f.world(v)),
            walls: walls.into_iter().map(|v| f.world(v)).collect(),
            enclosed: r.enclosed,
            door: door.map(|(h, e, s)| Door { hinge: f.world(h), end: f.world(e), swing: f.world(s) }),
            area: r.rect.area(),
            center: f.world(r.rect.center()),
            seats: spec.seat_count(),
        });
    }
    let ws = &p.workstations;
    for b in &c.benches {
        furniture.extend(furnish::bench(b, ws.desk_width, ws.desk_depth).into_iter().map(world_piece));
    }
    LayoutOption { strategy: c.strategy, description: c.strategy.describe(p.lang), rooms, furniture, metrics, score }
}

/// The door on side `side` of a room, local: (hinge, end of the opening, open leaf tip).
fn door(r: &Rect, side: usize, kind: SpaceKind) -> (Vec2, Vec2, Vec2) {
    let corners = r.corners();
    let a = corners.get(side).copied().unwrap_or(Vec2::ZERO);
    let b = corners.get((side + 1) % 4).copied().unwrap_or(Vec2::ZERO);
    let len = a.dist(b);
    let width: f64 = if kind == SpaceKind::PhoneBooth { 0.7 } else { 0.9 };
    let width = width.min((len - 0.3).max(0.3));
    let dir = (b - a).normalized();
    let hinge = a + dir * 0.15_f64.min((len - width) / 2.0);
    let end = hinge + dir * width;
    let swing = hinge - Rect::normal(side) * width;
    (hinge, end, swing)
}

#[cfg(test)]
mod tests;
