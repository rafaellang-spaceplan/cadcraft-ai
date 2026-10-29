//! Furniture for benches and rooms, in local metres.
//!
//! A piece's own axes: width along +x, depth along +y, centred on its origin. The person using
//! a desk, counter or chair sits or stands on its −y side; a piece "against a wall" has its +y
//! towards that wall.

use std::f64::consts::{FRAC_PI_2, PI};

use cadcraft_geom::Vec2;
use serde::{Deserialize, Serialize};

use crate::grid::Rect;
use crate::program::{SpaceKind, SpaceSpec, meeting_table};
use crate::solve::{Bench, PlacedRoom};

/// The kinds of furniture drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PieceKind {
    Desk,
    Chair,
    /// Rectangular meeting table.
    Table,
    /// Round table (width = diameter).
    RoundTable,
    Sofa,
    Counter,
    Cabinet,
}

/// A piece of furniture in local metres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocalPiece {
    pub kind: PieceKind,
    pub center: Vec2,
    pub width: f64,
    pub depth: f64,
    /// Rotation of the piece's +x axis, radians.
    pub angle: f64,
}

const CHAIR: f64 = 0.5;

/// The angle that turns a piece's +y towards `dir`.
fn facing(dir: Vec2) -> f64 {
    (-dir.x).atan2(dir.y)
}

/// The angle that puts a piece's +y against wall `side` of a room (0 bottom, 1 right, 2 top, 3 left).
fn wall_angle(side: usize) -> f64 {
    facing(Rect::normal(side))
}

fn piece(kind: PieceKind, center: Vec2, width: f64, depth: f64, angle: f64) -> LocalPiece {
    LocalPiece { kind, center, width, depth, angle }
}

/// A chair at `at`, turned towards `towards`.
fn chair_facing(at: Vec2, towards: Vec2) -> LocalPiece {
    piece(PieceKind::Chair, at, CHAIR, CHAIR, facing(towards - at))
}

/// Desks and chairs of a bench.
pub fn bench(b: &Bench, desk_w: f64, desk_d: f64) -> Vec<LocalPiece> {
    let mut out = Vec::new();
    let r = b.rect;
    for row in 0..b.sides {
        for i in 0..b.per_side {
            let t = (f64::from(i) + 0.5) * desk_w;
            // Row 0 sits on the low side (person further low), row 1 on the high side.
            let (c, person) = match (b.along_x, row) {
                (true, 0) => (Vec2::new(r.x0 + t, r.y0 + desk_d / 2.0), Vec2::new(0.0, -1.0)),
                (true, _) => (Vec2::new(r.x0 + t, r.y1 - desk_d / 2.0), Vec2::new(0.0, 1.0)),
                (false, 0) => (Vec2::new(r.x0 + desk_d / 2.0, r.y0 + t), Vec2::new(-1.0, 0.0)),
                (false, _) => (Vec2::new(r.x1 - desk_d / 2.0, r.y0 + t), Vec2::new(1.0, 0.0)),
            };
            // Desk +y away from the person.
            out.push(piece(PieceKind::Desk, c, desk_w, desk_d, facing(-person)));
            let seat = c + person * (desk_d / 2.0 + 0.3);
            out.push(chair_facing(seat, c));
        }
    }
    out
}

/// Furniture of a room.
pub fn room(r: &PlacedRoom, spec: &SpaceSpec) -> Vec<LocalPiece> {
    let rect = r.rect;
    // The back wall faces the door; an open room's back wall is its first long side.
    let back = r.door.map_or(if rect.w() >= rect.h() { 2 } else { 1 }, |d| (d + 2) % 4);
    let seats = spec.seat_count();
    match spec.kind {
        SpaceKind::Meeting => meeting(rect, seats),
        SpaceKind::Office => office(rect, back, seats),
        SpaceKind::PhoneBooth => {
            let (w, d) = (0.9_f64.min(rect.side_len(back) - 0.2), 0.5);
            let desk = against(rect, back, PieceKind::Desk, (w, d), 0.0, 0.5);
            let seat = desk.center - Rect::normal(back) * (d / 2.0 + 0.3);
            vec![desk, chair_facing(seat, desk.center)]
        }
        SpaceKind::Reception => {
            let len = 1.8_f64.min(rect.side_len(back) - 0.6);
            let counter = against(rect, back, PieceKind::Counter, (len, 0.6), 1.0, 0.5);
            let mut out = vec![counter];
            // The receptionist stands between the counter and the back wall.
            out.push(chair_facing(counter.center + Rect::normal(back) * 0.6, counter.center));
            // A sofa along a side wall, towards the front.
            let side = (back + 1) % 4;
            if rect.side_len(side) > 3.0 {
                let sofa_len = 1.8_f64.min(rect.side_len(side) - 1.8);
                out.push(against(rect, side, PieceKind::Sofa, (sofa_len, 0.8), 0.1, 0.25));
            }
            out
        }
        SpaceKind::Pantry => {
            let len = (rect.side_len(back) - 0.4).max(0.6);
            let mut out = vec![against(rect, back, PieceKind::Counter, (len, 0.6), 0.0, 0.5)];
            let tables = seats.div_ceil(4).max(1);
            // Tables clear of the counter.
            out.extend(round_tables(rect, Some(back), 0.9, tables));
            out
        }
        SpaceKind::Collab => {
            let tables = seats.div_ceil(4).max(1);
            round_tables(rect, None, 1.0, tables)
        }
        SpaceKind::Storage => {
            let n = ((rect.side_len(back) - 0.2) / 0.9).floor().clamp(0.0, 40.0) as u32;
            (0..n).map(|i| against(rect, back, PieceKind::Cabinet, (0.9, 0.45), 0.0, (f64::from(i) + 0.5) / f64::from(n))).collect()
        }
        SpaceKind::Custom => Vec::new(),
    }
}

/// A piece against wall `side`, `gap` metres off it, at fraction `t` along the wall (seen from
/// inside the room, left to right).
fn against(rect: Rect, side: usize, kind: PieceKind, (width, depth): (f64, f64), gap: f64, t: f64) -> LocalPiece {
    let [a, b, c, d] = rect.corners();
    // Each side's corners counter-clockwise; seen from inside, that runs right to left, so flip.
    let (start, end) = match side {
        0 => (b, a),
        1 => (c, b),
        2 => (d, c),
        _ => (a, d),
    };
    let along = start.lerp(end, 1.0 - t);
    let center = along - Rect::normal(side) * (gap + depth / 2.0);
    piece(kind, center, width, depth, wall_angle(side))
}

fn meeting(rect: Rect, seats: u32) -> Vec<LocalPiece> {
    let horizontal = rect.w() >= rect.h();
    let (long, short) = if horizontal { (rect.w(), rect.h()) } else { (rect.h(), rect.w()) };
    let (tl, tw) = meeting_table(seats);
    let (tl, tw) = (tl.min(long - 1.6).max(0.8), tw.min(short - 1.6).max(0.6));
    let c = rect.center();
    let angle = if horizontal { 0.0 } else { FRAC_PI_2 };
    let mut out = vec![piece(PieceKind::Table, c, tl, tw, angle)];
    let per_side = seats.div_ceil(2).max(1);
    let axis = if horizontal { Vec2::new(1.0, 0.0) } else { Vec2::new(0.0, 1.0) };
    let across = axis.perp();
    for (side, n) in [(1.0, per_side), (-1.0, seats.saturating_sub(per_side))] {
        for i in 0..n {
            let t = (f64::from(i) + 0.5) / f64::from(n) - 0.5;
            let at = c + axis * (t * tl) + across * (side * (tw / 2.0 + 0.35));
            out.push(chair_facing(at, at - across * side));
        }
    }
    out
}

fn office(rect: Rect, back: usize, seats: u32) -> Vec<LocalPiece> {
    let w = if rect.side_len(back) >= 3.2 { 1.6 } else { 1.2 };
    // The person sits between the desk and the back wall, facing the door.
    let mut desk = against(rect, back, PieceKind::Desk, (w, 0.7), 1.0, 0.5);
    desk.angle += PI;
    let n = Rect::normal(back);
    let mut out = vec![desk, chair_facing(desk.center + n * 0.65, desk.center)];
    let guests = seats.saturating_sub(1).min(2);
    for i in 0..guests {
        let off = (f64::from(i) - (f64::from(guests) - 1.0) / 2.0) * 0.7;
        let at = desk.center - n * 0.75 + n.perp() * off;
        if inside(rect, at, 0.25) {
            // Square to the desk, not turned towards its centre.
            out.push(piece(PieceKind::Chair, at, CHAIR, CHAIR, facing(n)));
        }
    }
    out
}

fn inside(rect: Rect, p: Vec2, margin: f64) -> bool {
    p.x >= rect.x0 + margin && p.x <= rect.x1 - margin && p.y >= rect.y0 + margin && p.y <= rect.y1 - margin
}

/// Up to `count` round tables of `diameter` with four chairs each, in a grid filling the room
/// (away from the first metre along wall `clear`, where a counter stands).
fn round_tables(rect: Rect, clear: Option<usize>, diameter: f64, count: u32) -> Vec<LocalPiece> {
    let pitch = diameter + 1.2;
    let area = match clear {
        None => rect,
        Some(0) => Rect::new(rect.x0, rect.y0 + 1.0, rect.x1, rect.y1),
        Some(1) => Rect::new(rect.x0, rect.y0, rect.x1 - 1.0, rect.y1),
        Some(2) => Rect::new(rect.x0, rect.y0, rect.x1, rect.y1 - 1.0),
        Some(_) => Rect::new(rect.x0 + 1.0, rect.y0, rect.x1, rect.y1),
    };
    let cols = (area.w() / pitch).floor().clamp(0.0, 50.0) as u32;
    let rows = (area.h() / pitch).floor().clamp(0.0, 50.0) as u32;
    let mut out = Vec::new();
    let mut placed = 0;
    for j in 0..rows {
        for i in 0..cols {
            if placed >= count {
                return out;
            }
            let c = Vec2::new(
                area.x0 + (area.w() - f64::from(cols) * pitch) / 2.0 + (f64::from(i) + 0.5) * pitch,
                area.y0 + (area.h() - f64::from(rows) * pitch) / 2.0 + (f64::from(j) + 0.5) * pitch,
            );
            out.push(piece(PieceKind::RoundTable, c, diameter, diameter, 0.0));
            for k in 0..4 {
                let at = Vec2::polar(c, diameter / 2.0 + 0.3, f64::from(k) * FRAC_PI_2 + PI / 4.0);
                out.push(chair_facing(at, c));
            }
            placed += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facing_turns_plus_y() {
        for dir in [Vec2::new(1.0, 0.0), Vec2::new(0.0, -1.0), Vec2::new(-0.6, 0.8)] {
            let y = Vec2::new(0.0, 1.0).rotate(facing(dir));
            assert!(y.near(dir.normalized(), 1e-9), "{dir:?} → {y:?}");
        }
    }

    #[test]
    fn bench_desks_and_chairs() {
        let b = Bench { rect: Rect::at(0.0, 0.0, 3.6, 1.4), envelope: Rect::at(-0.6, -0.9, 4.8, 3.2), per_side: 3, sides: 2, along_x: true };
        let p = bench(&b, 1.2, 0.7);
        assert_eq!(p.iter().filter(|p| p.kind == PieceKind::Desk).count(), 6);
        assert_eq!(p.iter().filter(|p| p.kind == PieceKind::Chair).count(), 6);
        // Chairs sit outside the desk footprint, on the person's side.
        for c in p.iter().filter(|p| p.kind == PieceKind::Chair) {
            assert!(c.center.y < 0.0 || c.center.y > 1.4, "{c:?}");
        }
    }

    #[test]
    fn meeting_room_seats_everyone_inside() {
        let spec = SpaceSpec { seats: Some(8), ..SpaceSpec::new(SpaceKind::Meeting, 1) };
        let (w, d) = spec.size();
        let r = PlacedRoom { spec: 0, rect: Rect::at(10.0, 5.0, w, d), door: Some(0), enclosed: true };
        let p = room(&r, &spec);
        assert_eq!(p.iter().filter(|p| p.kind == PieceKind::Chair).count(), 8);
        assert!(p.iter().all(|p| inside(r.rect, p.center, 0.2)), "{p:?}");
    }

    #[test]
    fn default_rooms_get_their_furniture() {
        for kind in [SpaceKind::Collab, SpaceKind::Pantry, SpaceKind::Meeting, SpaceKind::Office, SpaceKind::Reception] {
            let spec = SpaceSpec::new(kind, 1);
            let (w, d) = spec.size();
            for door in [Some(0), Some(1)] {
                let r = PlacedRoom { spec: 0, rect: Rect::at(0.0, 0.0, w, d), door, enclosed: true };
                let pieces = room(&r, &spec);
                let chairs = pieces.iter().filter(|p| p.kind == PieceKind::Chair).count();
                let want = if kind == SpaceKind::Reception { 1 } else { 2 };
                assert!(chairs >= want, "{kind:?} door {door:?}: {chairs} chairs");
                if kind == SpaceKind::Reception {
                    assert!(pieces.iter().any(|p| p.kind == PieceKind::Sofa));
                }
            }
        }
    }

    #[test]
    fn every_kind_furnishes_inside_its_room() {
        for kind in SpaceKind::ALL {
            let spec = SpaceSpec::new(kind, 1);
            let (w, d) = spec.size();
            for door in [None, Some(0), Some(1), Some(2), Some(3)] {
                let r = PlacedRoom { spec: 0, rect: Rect::at(0.0, 0.0, w, d), door, enclosed: true };
                for p in room(&r, &spec) {
                    assert!(inside(r.rect, p.center, 0.0), "{kind:?} door {door:?}: {p:?} outside {w}×{d}");
                }
            }
        }
    }
}
