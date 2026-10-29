//! The floor as an occupancy grid in local metres, with prefix sums for constant-time
//! "is this rectangle free?" queries, and coarse distance fields to the façade and the core.

use cadcraft_geom::Vec2;

/// An axis-aligned rectangle in local metres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl Rect {
    pub fn new(x0: f64, y0: f64, x1: f64, y1: f64) -> Self {
        Rect { x0: x0.min(x1), y0: y0.min(y1), x1: x0.max(x1), y1: y0.max(y1) }
    }
    pub fn at(x: f64, y: f64, w: f64, h: f64) -> Self {
        Rect::new(x, y, x + w, y + h)
    }
    pub fn w(&self) -> f64 {
        self.x1 - self.x0
    }
    pub fn h(&self) -> f64 {
        self.y1 - self.y0
    }
    pub fn area(&self) -> f64 {
        self.w() * self.h()
    }
    pub fn center(&self) -> Vec2 {
        Vec2::new((self.x0 + self.x1) / 2.0, (self.y0 + self.y1) / 2.0)
    }
    pub fn expand(&self, dx: f64, dy: f64) -> Rect {
        Rect::new(self.x0 - dx, self.y0 - dy, self.x1 + dx, self.y1 + dy)
    }
    pub fn translate(&self, dx: f64, dy: f64) -> Rect {
        Rect::new(self.x0 + dx, self.y0 + dy, self.x1 + dx, self.y1 + dy)
    }
    pub fn corners(&self) -> [Vec2; 4] {
        [Vec2::new(self.x0, self.y0), Vec2::new(self.x1, self.y0), Vec2::new(self.x1, self.y1), Vec2::new(self.x0, self.y1)]
    }
    /// The strip of depth `d` just outside one side: 0 bottom, 1 right, 2 top, 3 left.
    pub fn outside(&self, side: usize, d: f64) -> Rect {
        match side {
            0 => Rect::new(self.x0, self.y0 - d, self.x1, self.y0),
            1 => Rect::new(self.x1, self.y0, self.x1 + d, self.y1),
            2 => Rect::new(self.x0, self.y1, self.x1, self.y1 + d),
            _ => Rect::new(self.x0 - d, self.y0, self.x0, self.y1),
        }
    }
    /// Length of a side.
    pub fn side_len(&self, side: usize) -> f64 {
        if side.is_multiple_of(2) { self.w() } else { self.h() }
    }
    /// Outward unit normal of a side.
    pub fn normal(side: usize) -> Vec2 {
        match side {
            0 => Vec2::new(0.0, -1.0),
            1 => Vec2::new(1.0, 0.0),
            2 => Vec2::new(0.0, 1.0),
            _ => Vec2::new(-1.0, 0.0),
        }
    }
}

/// Distance from `p` to the segment `a`-`b`.
pub fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let ab = b - a;
    let l2 = ab.len2();
    let t = if l2 > 0.0 { ((p - a).dot(ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
    p.dist(a + ab * t)
}

/// Distance from `p` to the edges of a closed polygon.
pub fn poly_dist(poly: &[Vec2], p: Vec2) -> f64 {
    let n = poly.len();
    let mut best = f64::INFINITY;
    for i in 0..n {
        if let (Some(a), Some(b)) = (poly.get(i), poly.get((i + 1) % n)) {
            best = best.min(seg_dist(p, *a, *b));
        }
    }
    best
}

/// Cell states.
pub const BAD: u8 = 0;
pub const FREE: u8 = 1;
/// Reserved circulation: rooms and benches stay off it, door access may cross it.
pub const CORRIDOR: u8 = 2;
pub const TAKEN: u8 = 3;

/// A coarse scalar field over the floor's bounds (bilinear lookups are not needed: the step is
/// small against room sizes).
#[derive(Clone, Debug)]
pub struct Field {
    origin: Vec2,
    step: f64,
    nx: usize,
    ny: usize,
    v: Vec<f64>,
}

impl Field {
    /// Sample `f` every `step` metres over `[min, max]`.
    pub fn sample(min: Vec2, max: Vec2, step: f64, f: impl Fn(Vec2) -> f64) -> Field {
        let nx = (((max.x - min.x) / step).ceil() as usize).saturating_add(1).max(1);
        let ny = (((max.y - min.y) / step).ceil() as usize).saturating_add(1).max(1);
        let mut v = Vec::with_capacity(nx.saturating_mul(ny));
        for j in 0..ny {
            for i in 0..nx {
                v.push(f(Vec2::new(min.x + i as f64 * step, min.y + j as f64 * step)));
            }
        }
        Field { origin: min, step, nx, ny, v }
    }
    pub fn at(&self, p: Vec2) -> f64 {
        let i = ((p.x - self.origin.x) / self.step).round().clamp(0.0, (self.nx - 1) as f64) as usize;
        let j = ((p.y - self.origin.y) / self.step).round().clamp(0.0, (self.ny - 1) as f64) as usize;
        self.v.get(j * self.nx + i).copied().unwrap_or(f64::INFINITY)
    }
}

/// The occupancy grid of one option being built. Cloning it is cheap enough to start every
/// candidate from the same empty floor.
#[derive(Clone, Debug)]
pub struct Grid {
    pub origin: Vec2,
    pub cell: f64,
    pub nx: usize,
    pub ny: usize,
    state: Vec<u8>,
    /// Prefix counts of cells that are neither free nor corridor (outside, blocked, taken).
    hard: Vec<u32>,
    /// Prefix counts of cells that aren't free.
    soft: Vec<u32>,
}

impl Grid {
    /// Rasterize the floor: cells whose centre is inside `outline` and outside every hole are free.
    pub fn new(outline: &[Vec2], holes: &[Vec<Vec2>], min: Vec2, max: Vec2, cell: f64) -> Grid {
        let nx = ((max.x - min.x) / cell).ceil().max(1.0) as usize;
        let ny = ((max.y - min.y) / cell).ceil().max(1.0) as usize;
        let mut state = vec![BAD; nx.saturating_mul(ny)];
        for j in 0..ny {
            let y = min.y + (j as f64 + 0.5) * cell;
            let inside = spans(outline, y);
            let holes_here: Vec<Vec<(f64, f64)>> = holes.iter().map(|h| spans(h, y)).collect();
            for i in 0..nx {
                let x = min.x + (i as f64 + 0.5) * cell;
                let free = in_spans(&inside, x) && !holes_here.iter().any(|h| in_spans(h, x));
                if free && let Some(c) = state.get_mut(j * nx + i) {
                    *c = FREE;
                }
            }
        }
        let mut g = Grid { origin: min, cell, nx, ny, state, hard: Vec::new(), soft: Vec::new() };
        g.rebuild();
        g
    }

    /// Recompute the prefix sums after marking.
    pub fn rebuild(&mut self) {
        let (w, h) = (self.nx + 1, self.ny + 1);
        self.hard.clear();
        self.hard.resize(w * h, 0);
        self.soft.clear();
        self.soft.resize(w * h, 0);
        for j in 0..self.ny {
            let mut row_hard = 0u32;
            let mut row_soft = 0u32;
            for i in 0..self.nx {
                let s = self.state.get(j * self.nx + i).copied().unwrap_or(BAD);
                row_hard += u32::from(s == BAD || s == TAKEN);
                row_soft += u32::from(s != FREE);
                let below = j * w + i + 1;
                let here = (j + 1) * w + i + 1;
                let (bh, bs) = (self.hard.get(below).copied().unwrap_or(0), self.soft.get(below).copied().unwrap_or(0));
                if let Some(c) = self.hard.get_mut(here) {
                    *c = bh + row_hard;
                }
                if let Some(c) = self.soft.get_mut(here) {
                    *c = bs + row_soft;
                }
            }
        }
    }

    /// The cells `r` overlaps (touching edges don't count): `(i0, j0, i1, j1)` half-open, or
    /// `None` when `r` leaves the grid.
    fn cells(&self, r: &Rect) -> Option<(usize, usize, usize, usize)> {
        const EPS: f64 = 1e-6;
        let fx0 = ((r.x0 - self.origin.x) / self.cell + EPS).floor();
        let fy0 = ((r.y0 - self.origin.y) / self.cell + EPS).floor();
        let fx1 = ((r.x1 - self.origin.x) / self.cell - EPS).ceil();
        let fy1 = ((r.y1 - self.origin.y) / self.cell - EPS).ceil();
        if !(fx0.is_finite() && fy0.is_finite() && fx1.is_finite() && fy1.is_finite()) {
            return None;
        }
        if fx0 < 0.0 || fy0 < 0.0 || fx1 > self.nx as f64 || fy1 > self.ny as f64 {
            return None;
        }
        Some((fx0 as usize, fy0 as usize, fx1.max(fx0) as usize, fy1.max(fy0) as usize))
    }

    fn count(table: &[u32], w: usize, (i0, j0, i1, j1): (usize, usize, usize, usize)) -> u32 {
        let g = |i: usize, j: usize| table.get(j * w + i).copied().unwrap_or(0);
        (g(i1, j1) + g(i0, j0)).saturating_sub(g(i0, j1) + g(i1, j0))
    }

    /// Every cell in `r` is free (rooms, benches).
    pub fn is_free(&self, r: &Rect) -> bool {
        self.cells(r).is_some_and(|c| Self::count(&self.soft, self.nx + 1, c) == 0)
    }

    /// Every cell in `r` is free or circulation (door access).
    pub fn is_passable(&self, r: &Rect) -> bool {
        self.cells(r).is_some_and(|c| Self::count(&self.hard, self.nx + 1, c) == 0)
    }

    /// Fraction of `r` that is wall, outside, blocked or taken (1 outside the grid).
    pub fn solid_fraction(&self, r: &Rect) -> f64 {
        let Some(c) = self.cells(r) else { return 1.0 };
        let n = (c.2 - c.0) * (c.3 - c.1);
        if n == 0 {
            return 0.0;
        }
        f64::from(Self::count(&self.hard, self.nx + 1, c)) / n as f64
    }

    /// Set the cells in `r` that are currently `from` (any non-bad state when `None`) to `to`.
    /// Call [`Grid::rebuild`] afterwards.
    pub fn mark(&mut self, r: &Rect, from: Option<u8>, to: u8) {
        let Some((i0, j0, i1, j1)) = self.cells(&self.clip(r)) else { return };
        for j in j0..j1 {
            for i in i0..i1 {
                if let Some(c) = self.state.get_mut(j * self.nx + i)
                    && *c != BAD
                    && from.is_none_or(|f| *c == f)
                {
                    *c = to;
                }
            }
        }
    }

    /// `r` clipped to the grid's bounds.
    pub fn clip(&self, r: &Rect) -> Rect {
        let max = Vec2::new(self.origin.x + self.nx as f64 * self.cell, self.origin.y + self.ny as f64 * self.cell);
        Rect::new(
            r.x0.clamp(self.origin.x, max.x),
            r.y0.clamp(self.origin.y, max.y),
            r.x1.clamp(self.origin.x, max.x),
            r.y1.clamp(self.origin.y, max.y),
        )
    }

    /// Area of free plus circulation cells, m².
    pub fn usable_area(&self) -> f64 {
        self.state.iter().filter(|s| **s != BAD).count() as f64 * self.cell * self.cell
    }
}

/// The x-intervals where the horizontal line at `y` is inside a closed polygon (even-odd).
fn spans(poly: &[Vec2], y: f64) -> Vec<(f64, f64)> {
    let n = poly.len();
    let mut xs = Vec::new();
    for i in 0..n {
        if let (Some(a), Some(b)) = (poly.get(i), poly.get((i + 1) % n))
            && (a.y > y) != (b.y > y)
        {
            xs.push(a.x + (y - a.y) * (b.x - a.x) / (b.y - a.y));
        }
    }
    xs.sort_by(f64::total_cmp);
    xs.chunks_exact(2).filter_map(|c| Some((*c.first()?, *c.get(1)?))).collect()
}

fn in_spans(spans: &[(f64, f64)], x: f64) -> bool {
    spans.iter().any(|(a, b)| x >= *a && x <= *b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(s: f64) -> Vec<Vec2> {
        vec![Vec2::new(0.0, 0.0), Vec2::new(s, 0.0), Vec2::new(s, s), Vec2::new(0.0, s)]
    }

    #[test]
    fn free_queries_follow_the_outline_and_holes() {
        let hole = vec![Vec2::new(4.0, 4.0), Vec2::new(6.0, 4.0), Vec2::new(6.0, 6.0), Vec2::new(4.0, 6.0)];
        let mut g = Grid::new(&square(10.0), &[hole], Vec2::ZERO, Vec2::new(10.0, 10.0), 0.1);
        assert!(g.is_free(&Rect::at(0.0, 0.0, 3.0, 3.0)));
        assert!(!g.is_free(&Rect::at(3.0, 3.0, 2.0, 2.0)), "overlaps the hole");
        assert!(!g.is_free(&Rect::at(9.0, 9.0, 2.0, 2.0)), "leaves the floor");
        assert!((g.usable_area() - 96.0).abs() < 0.5);
        g.mark(&Rect::at(0.0, 0.0, 2.0, 2.0), None, CORRIDOR);
        g.rebuild();
        assert!(!g.is_free(&Rect::at(1.0, 1.0, 2.0, 2.0)));
        assert!(g.is_passable(&Rect::at(1.0, 1.0, 2.0, 2.0)));
        g.mark(&Rect::at(0.0, 0.0, 1.0, 1.0), None, TAKEN);
        g.rebuild();
        assert!(!g.is_passable(&Rect::at(0.5, 0.5, 1.0, 1.0)));
        assert!(g.solid_fraction(&Rect::at(0.0, 0.0, 1.0, 1.0)) > 0.99);
        assert!(g.solid_fraction(&Rect::at(-1.0, 0.0, 1.0, 1.0)) > 0.99, "outside the grid is solid");
    }

    #[test]
    fn distances() {
        assert!((poly_dist(&square(10.0), Vec2::new(5.0, 2.0)) - 2.0).abs() < 1e-9);
        let f = Field::sample(Vec2::ZERO, Vec2::new(10.0, 10.0), 0.5, |p| poly_dist(&square(10.0), p));
        assert!((f.at(Vec2::new(5.0, 5.0)) - 5.0).abs() < 1e-9);
        assert!(f.at(Vec2::new(-50.0, 5.0)).is_finite(), "lookups outside clamp to the edge");
    }
}
