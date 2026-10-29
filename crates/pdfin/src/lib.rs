//! CADCraft PDF import: the vector content of a PDF page as polylines.
//!
//! A drawing plotted to PDF (by CADCraft, AutoCAD or any CAD program) keeps its geometry as
//! paths: lines, Bézier curves and rectangles, stroked or filled, in a colour, often grouped in
//! optional-content groups named after the drawing's layers. [`read`] walks a page's content
//! stream (and the form XObjects it draws) with the graphics state's transformation, flattens
//! curves, and returns each painted subpath in PDF points with its colour, width and layer.
//! Text and images are not imported (they are counted).
//!
//! PDF files are hostile input: nesting, path counts and point counts are capped, numbers that
//! aren't finite are dropped, and nothing panics.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

use cadcraft_geom::Vec2;
use hayro_syntax::Pdf;
use hayro_syntax::content::TypedIter;
use hayro_syntax::content::ops::TypedInstruction as Op;
use hayro_syntax::object::{Array, Dict, Name, Number, Object, Stream};
use hayro_syntax::page::Resources;

/// Deepest nesting of form XObjects followed.
pub const MAX_FORM_DEPTH: usize = 12;
/// Deepest graphics state stack (`q` without `Q`).
const MAX_STATE_DEPTH: usize = 256;

/// What to import.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    /// Page index, from 0.
    pub page: usize,
    /// Import filled areas (solid hatches, poché) as closed outlines.
    pub fills: bool,
    /// Curve flattening tolerance, points.
    pub tolerance: f64,
    /// Caps: once reached, the rest of the page is skipped and [`Page::truncated`] is set.
    pub max_paths: usize,
    pub max_points: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options { page: 0, fills: true, tolerance: 0.05, max_paths: 400_000, max_points: 8_000_000 }
    }
}

/// One painted subpath, in PDF points (y up, origin at the page's lower-left corner).
#[derive(Clone, Debug, PartialEq)]
pub struct Path {
    pub points: Vec<Vec2>,
    pub closed: bool,
    pub stroked: bool,
    pub filled: bool,
    /// The paint colour (stroke colour when stroked, else fill colour), 0..=255.
    pub color: [u8; 3],
    /// Line width in points (after the transformation's scale).
    pub width: f64,
    /// The optional-content group (PDF layer) it belongs to.
    pub layer: Option<String>,
}

/// A page's vector content.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page {
    /// The page's size in points (media box).
    pub width: f64,
    pub height: f64,
    pub paths: Vec<Path>,
    /// PDF layer names, in the order first seen.
    pub layers: Vec<String>,
    /// Pages in the document.
    pub pages: usize,
    /// Text-showing operations skipped.
    pub text_skipped: usize,
    /// Images skipped.
    pub images_skipped: usize,
    /// A cap was reached.
    pub truncated: bool,
}

/// Why nothing was read.
#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    NotPdf(String),
    NoPage { page: usize, pages: usize },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotPdf(m) => write!(f, "not a readable PDF: {m}"),
            Error::NoPage { page, pages } => write!(f, "the PDF has {pages} page(s); there is no page {}", page + 1),
        }
    }
}

impl std::error::Error for Error {}

/// An affine transform `[a b c d e f]` as PDF writes it: `x' = a x + c y + e`, `y' = b x + d y + f`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Matrix([f64; 6]);

impl Matrix {
    const IDENTITY: Matrix = Matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    /// `self` applied after `inner` (PDF's `cm` premultiplies: CTM' = M × CTM).
    fn then(&self, outer: &Matrix) -> Matrix {
        let [a, b, c, d, e, f] = self.0;
        let [a2, b2, c2, d2, e2, f2] = outer.0;
        Matrix([a * a2 + b * c2, a * b2 + b * d2, c * a2 + d * c2, c * b2 + d * d2, e * a2 + f * c2 + e2, e * b2 + f * d2 + f2])
    }

    fn apply(&self, p: Vec2) -> Vec2 {
        let [a, b, c, d, e, f] = self.0;
        Vec2::new(a * p.x + c * p.y + e, b * p.x + d * p.y + f)
    }

    /// The scale a line width is multiplied by (geometric mean of the axes).
    fn scale(&self) -> f64 {
        let [a, b, c, d, ..] = self.0;
        (a * d - b * c).abs().sqrt()
    }

    fn is_finite(&self) -> bool {
        self.0.iter().all(|v| v.is_finite())
    }
}

#[derive(Clone, Debug)]
struct GState {
    ctm: Matrix,
    stroke: [u8; 3],
    fill: [u8; 3],
    width: f64,
}

struct Subpath {
    points: Vec<Vec2>,
    closed: bool,
}

/// The page being read.
struct Reader<'p> {
    opts: &'p Options,
    out: Page,
    points: usize,
    path: Vec<Subpath>,
    /// Current point (user space before the CTM: kept as transformed points in `path`).
    current: Option<Vec2>,
    start: Option<Vec2>,
    /// Marked-content stack: the PDF layer each level is in.
    marked: Vec<Option<String>>,
}

fn num(n: &Number) -> f64 {
    n.as_f64()
}

fn gray(g: f64) -> [u8; 3] {
    let v = (g.clamp(0.0, 1.0) * 255.0).round() as u8;
    [v, v, v]
}

fn rgb(r: f64, g: f64, b: f64) -> [u8; 3] {
    let c = |v: f64| (if v.is_finite() { v } else { 0.0 }.clamp(0.0, 1.0) * 255.0).round() as u8;
    [c(r), c(g), c(b)]
}

fn cmyk(c: f64, m: f64, y: f64, k: f64) -> [u8; 3] {
    rgb((1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k))
}

/// A colour from `sc`/`scn` operands: 1 gray, 3 RGB, 4 CMYK; anything else (patterns, other
/// colour spaces) keeps `prev`.
fn color_of(nums: &[Number], prev: [u8; 3]) -> [u8; 3] {
    match nums {
        [g] => gray(num(g)),
        [r, g, b] => rgb(num(r), num(g), num(b)),
        [c, m, y, k] => cmyk(num(c), num(m), num(y), num(k)),
        _ => prev,
    }
}

impl Reader<'_> {
    fn full(&self) -> bool {
        self.out.paths.len() >= self.opts.max_paths || self.points >= self.opts.max_points
    }

    fn layer(&self) -> Option<String> {
        self.marked.iter().rev().find_map(Clone::clone)
    }

    fn move_to(&mut self, p: Vec2) {
        self.path.push(Subpath { points: vec![p], closed: false });
        self.current = Some(p);
        self.start = Some(p);
    }

    fn line_to(&mut self, p: Vec2) {
        match self.path.last_mut() {
            Some(sp) if !sp.closed => sp.points.push(p),
            // A line without a move starts at the current point (or here).
            _ => {
                let from = self.current.unwrap_or(p);
                self.path.push(Subpath { points: vec![from, p], closed: false });
            }
        }
        self.current = Some(p);
    }

    fn curve_to(&mut self, c1: Vec2, c2: Vec2, p: Vec2) {
        let p0 = self.current.unwrap_or(c1);
        let len = p0.dist(c1) + c1.dist(c2) + c2.dist(p);
        let n = ((len / self.opts.tolerance.max(1e-6)).sqrt().ceil()).clamp(1.0, 64.0) as usize;
        for i in 1..=n {
            let t = i as f64 / n as f64;
            let u = 1.0 - t;
            let q = p0 * (u * u * u) + c1 * (3.0 * u * u * t) + c2 * (3.0 * u * t * t) + p * (t * t * t);
            self.line_to(q);
        }
    }

    fn close(&mut self) {
        if let Some(sp) = self.path.last_mut() {
            sp.closed = true;
        }
        self.current = self.start;
    }

    /// Paint the current path and start a new one.
    fn paint(&mut self, gs: &GState, stroked: bool, filled: bool, close: bool) {
        if close {
            self.close();
        }
        let path = std::mem::take(&mut self.path);
        self.current = None;
        if !stroked && (!filled || !self.opts.fills) {
            return;
        }
        let layer = self.layer();
        if let Some(l) = &layer
            && !self.out.layers.contains(l)
        {
            self.out.layers.push(l.clone());
        }
        for sp in path {
            if self.full() {
                self.out.truncated = true;
                return;
            }
            let mut pts: Vec<Vec2> = Vec::with_capacity(sp.points.len());
            for p in sp.points {
                if p.is_finite() && !pts.last().is_some_and(|l: &Vec2| l.near(p, 1e-9)) {
                    pts.push(p);
                }
            }
            // A subpath that comes back to its start is closed (CAD programs often write the
            // first point again instead of `h`); it keeps one copy of the start.
            let returns = pts.len() > 3 && pts.first().zip(pts.last()).is_some_and(|(a, b)| a.near(*b, 1e-6));
            if returns {
                pts.pop();
            }
            let closed = sp.closed || returns;
            if pts.len() < 2 {
                continue;
            }
            self.points += pts.len();
            self.out.paths.push(Path {
                closed: closed && pts.len() > 2,
                points: pts,
                stroked,
                filled,
                color: if stroked { gs.stroke } else { gs.fill },
                width: gs.width * gs.ctm.scale(),
                layer: layer.clone(),
            });
        }
    }

    /// The PDF layer name of an optional-content group dictionary.
    fn ocg_name(d: &Dict<'_>) -> Option<String> {
        let s = d.get::<hayro_syntax::object::String<'_>>(b"Name".as_slice())?;
        let bytes = s.as_bytes();
        // UTF-16BE with a byte order mark, else PDFDocEncoding (read as Latin-1).
        let name = if bytes.starts_with(&[0xFE, 0xFF]) {
            let units: Vec<u16> = bytes
                .get(2..)
                .unwrap_or(&[])
                .chunks_exact(2)
                .map(|c| u16::from_be_bytes([c.first().copied().unwrap_or(0), c.get(1).copied().unwrap_or(0)]))
                .collect();
            String::from_utf16_lossy(&units)
        } else {
            bytes.iter().map(|&b| b as char).collect()
        };
        let name: String = name.chars().filter(|c| !c.is_control()).take(200).collect();
        (!name.trim().is_empty()).then_some(name)
    }

    fn run(&mut self, content: &[u8], resources: &Resources<'_>, ctm: Matrix, depth: usize) {
        let mut stack = vec![GState { ctm, stroke: [0, 0, 0], fill: [0, 0, 0], width: 1.0 }];
        let mut iter = TypedIter::new(content);
        while let Some(op) = iter.next() {
            if self.full() {
                self.out.truncated = true;
                return;
            }
            match op {
                Op::SaveState(_) => {
                    if stack.len() < MAX_STATE_DEPTH
                        && let Some(top) = stack.last().cloned()
                    {
                        stack.push(top);
                    }
                    continue;
                }
                Op::RestoreState(_) => {
                    if stack.len() > 1 {
                        stack.pop();
                    }
                    continue;
                }
                _ => {}
            }
            let Some(gs) = stack.last_mut() else { return };
            let t = |gs: &GState, x: &Number, y: &Number| gs.ctm.apply(Vec2::new(num(x), num(y)));
            match op {
                Op::Transform(m) => {
                    let mat = Matrix([num(&m.0), num(&m.1), num(&m.2), num(&m.3), num(&m.4), num(&m.5)]);
                    if mat.is_finite() {
                        gs.ctm = mat.then(&gs.ctm);
                    }
                }
                Op::LineWidth(w) => {
                    let w = num(&w.0);
                    gs.width = if w.is_finite() { w.abs() } else { 1.0 };
                }
                Op::StrokeColorDeviceGray(g) => gs.stroke = gray(num(&g.0)),
                Op::NonStrokeColorDeviceGray(g) => gs.fill = gray(num(&g.0)),
                Op::StrokeColorDeviceRgb(c) => gs.stroke = rgb(num(&c.0), num(&c.1), num(&c.2)),
                Op::NonStrokeColorDeviceRgb(c) => gs.fill = rgb(num(&c.0), num(&c.1), num(&c.2)),
                Op::StrokeColorCmyk(c) => gs.stroke = cmyk(num(&c.0), num(&c.1), num(&c.2), num(&c.3)),
                Op::NonStrokeColorCmyk(c) => gs.fill = cmyk(num(&c.0), num(&c.1), num(&c.2), num(&c.3)),
                Op::StrokeColor(c) => gs.stroke = color_of(&c.0, gs.stroke),
                Op::NonStrokeColor(c) => gs.fill = color_of(&c.0, gs.fill),
                Op::StrokeColorNamed(c) => gs.stroke = color_of(&c.0, gs.stroke),
                Op::NonStrokeColorNamed(c) => gs.fill = color_of(&c.0, gs.fill),
                Op::MoveTo(m) => {
                    let p = t(gs, &m.0, &m.1);
                    self.move_to(p);
                }
                Op::LineTo(l) => {
                    let p = t(gs, &l.0, &l.1);
                    self.line_to(p);
                }
                Op::CubicTo(c) => {
                    let (c1, c2, p) = (t(gs, &c.0, &c.1), t(gs, &c.2, &c.3), t(gs, &c.4, &c.5));
                    self.curve_to(c1, c2, p);
                }
                Op::CubicStartTo(c) => {
                    // The first control point is the current point.
                    let (c2, p) = (t(gs, &c.0, &c.1), t(gs, &c.2, &c.3));
                    let c1 = self.current.unwrap_or(c2);
                    self.curve_to(c1, c2, p);
                }
                Op::CubicEndTo(c) => {
                    // The second control point is the end point.
                    let (c1, p) = (t(gs, &c.0, &c.1), t(gs, &c.2, &c.3));
                    self.curve_to(c1, p, p);
                }
                Op::ClosePath(_) => self.close(),
                Op::RectPath(r) => {
                    let (x, y, w, h) = (num(&r.0), num(&r.1), num(&r.2), num(&r.3));
                    let corners = [Vec2::new(x, y), Vec2::new(x + w, y), Vec2::new(x + w, y + h), Vec2::new(x, y + h)];
                    let pts: Vec<Vec2> = corners.iter().map(|p| gs.ctm.apply(*p)).collect();
                    self.path.push(Subpath { points: pts, closed: true });
                    self.current = Some(gs.ctm.apply(Vec2::new(x, y)));
                    self.start = self.current;
                }
                Op::StrokePath(_) => self.paint(&gs.clone(), true, false, false),
                Op::CloseAndStrokePath(_) => self.paint(&gs.clone(), true, false, true),
                Op::FillPathNonZero(_) | Op::FillPathNonZeroCompatibility(_) | Op::FillPathEvenOdd(_) => self.paint(&gs.clone(), false, true, true),
                Op::FillAndStrokeNonZero(_) | Op::FillAndStrokeEvenOdd(_) => self.paint(&gs.clone(), true, true, false),
                Op::CloseFillAndStrokeNonZero(_) | Op::CloseFillAndStrokeEvenOdd(_) => self.paint(&gs.clone(), true, true, true),
                Op::EndPath(_) => {
                    self.path.clear();
                    self.current = None;
                }
                Op::BeginMarkedContent(_) => self.marked.push(None),
                Op::BeginMarkedContentWithProperties(m) => {
                    let layer = if m.0.as_ref() == b"OC" {
                        match m.1 {
                            Object::Name(n) => resources.properties.get::<Dict<'_>>(n.as_ref()).and_then(|d| Self::ocg_name(&d)),
                            Object::Dict(d) => Self::ocg_name(d),
                            _ => None,
                        }
                    } else {
                        None
                    };
                    self.marked.push(layer);
                }
                Op::EndMarkedContent(_) => {
                    self.marked.pop();
                }
                Op::ShowText(_) | Op::ShowTexts(_) | Op::NextLineAndShowText(_) | Op::ShowTextWithParameters(_) => self.out.text_skipped += 1,
                Op::InlineImage(_) => self.out.images_skipped += 1,
                Op::XObject(x) => {
                    let ctm = gs.ctm;
                    self.xobject(x.0, resources, ctm, depth);
                }
                _ => {}
            }
        }
    }

    fn xobject(&mut self, name: &Name<'_>, resources: &Resources<'_>, ctm: Matrix, depth: usize) {
        let Some(stream) = resources.get_x_object(name) else { return };
        let dict = stream.dict();
        if dict.get::<Name<'_>>(b"Subtype".as_slice()).is_some_and(|s| s.as_ref() == b"Image") {
            self.out.images_skipped += 1;
            return;
        }
        if depth >= MAX_FORM_DEPTH {
            self.out.truncated = true;
            return;
        }
        self.form(&stream, resources, ctm, depth + 1);
    }

    fn form(&mut self, stream: &Stream<'_>, resources: &Resources<'_>, ctm: Matrix, depth: usize) {
        let dict = stream.dict();
        let matrix = dict
            .get::<Array<'_>>(b"Matrix".as_slice())
            .map(|a| a.iter::<Number>().map(|n| num(&n)).collect::<Vec<f64>>())
            .and_then(|v| <[f64; 6]>::try_from(v).ok())
            .map(Matrix)
            .filter(Matrix::is_finite)
            .unwrap_or(Matrix::IDENTITY);
        let Ok(content) = stream.decoded() else { return };
        let res = match dict.get::<Dict<'_>>(b"Resources".as_slice()) {
            Some(d) => Resources::from_parent(d, resources.clone()),
            None => resources.clone(),
        };
        self.run(&content, &res, matrix.then(&ctm), depth);
    }
}

/// The vector content of a page of the PDF in `bytes`.
pub fn read(bytes: &[u8], opts: &Options) -> Result<Page, Error> {
    if !bytes.starts_with(b"%PDF") && !bytes.get(..1024).is_some_and(|h| h.windows(5).any(|w| w == b"%PDF-")) {
        return Err(Error::NotPdf("no %PDF header".into()));
    }
    let pdf = Pdf::new(bytes.to_vec()).map_err(|e| Error::NotPdf(format!("{e:?}")))?;
    let pages = pdf.pages();
    let page = pages.get(opts.page).ok_or(Error::NoPage { page: opts.page, pages: pages.len() })?;
    let mb = page.media_box();
    let (x0, y0, x1, y1) = (mb.x0, mb.y0, mb.x1, mb.y1);
    let mut r = Reader {
        opts,
        out: Page { pages: pages.len(), ..Page::default() },
        points: 0,
        path: Vec::new(),
        current: None,
        start: None,
        marked: Vec::new(),
    };
    // Points relative to the media box's lower-left corner, so the page starts at (0, 0).
    let ctm = Matrix([1.0, 0.0, 0.0, 1.0, -x0, -y0]);
    r.out.width = (x1 - x0).abs();
    r.out.height = (y1 - y0).abs();
    if let Some(content) = page.page_stream() {
        r.run(content, page.resources(), ctm, 0);
    }
    Ok(r.out)
}

#[cfg(test)]
mod tests;
