//! The floor plate a layout is planned on, read from the drawing: its outline, the cores, shafts
//! and columns inside it, the entrance and the drawing units.

use cadcraft_doc::{Drawing, Entity, Handle, Space, entity_bounds};
use cadcraft_geom::{Bounds2, Polyline, Vec2, point_in_polygon, shoelace};
use cadcraft_spaceplan::{Floor, MAX_HOLES, MAX_VERTICES};
use serde_json::Value;

use super::super::curves::handle_value;
use super::super::hatch::{closed_loop, loops_at};
use super::super::{bad, point_param, points_param, str_param};
use super::ID;
use crate::{Result, Session};

/// Holes smaller than this (m²) are drawing noise (a hatch seed, a tag), not columns.
const MIN_HOLE_AREA: f64 = 0.04;

/// Drawing units per metre for an INSUNITS value (0 = unitless).
fn units_per_metre(insunits: i64) -> Option<f64> {
    Some(match insunits {
        1 => 39.370_078_740_157_48,
        2 => 3.280_839_895_013_123,
        4 => 1000.0,
        5 => 100.0,
        6 => 1.0,
        14 => 10.0,
        _ => return None,
    })
}

/// Drawing units per metre for a name (`mm`, `cm`, `m`, `in`, `ft`).
fn named_units(name: &str) -> Option<f64> {
    Some(match name.trim().to_ascii_lowercase().as_str() {
        "mm" | "millimeters" | "millimetres" | "milimetros" => 1000.0,
        "cm" | "centimeters" | "centimetres" | "centimetros" => 100.0,
        "m" | "meters" | "metres" | "metros" => 1.0,
        "in" | "inches" | "polegadas" => 39.370_078_740_157_48,
        "ft" | "feet" | "pes" => 3.280_839_895_013_123,
        _ => return None,
    })
}

/// A unitless drawing's units, guessed from the size of its floor: office floors are 8 to
/// 200 m across.
fn guess_units(b: &Bounds2) -> f64 {
    let side = b.width().max(b.height());
    if side >= 8_000.0 {
        1000.0
    } else if side >= 400.0 {
        100.0
    } else {
        1.0
    }
}

/// Tessellate a closed object (closed polyline, circle, ellipse, closed spline) at `tol`.
fn loop_points(d: &Drawing, e: &Entity) -> Option<Vec<Vec2>> {
    let vs = closed_loop(d, e)?;
    let b = entity_bounds(d, e, 0);
    let tol = ((b.width() + b.height()) * 1e-3).max(1e-9);
    let mut pts = Polyline { vertices: vs, closed: true }.tessellate(tol);
    // Polylines come back with the first point repeated at the end.
    if pts.len() > 1 && pts.first().zip(pts.last()).is_some_and(|(a, b)| a.near(*b, 1e-12)) {
        pts.pop();
    }
    (pts.len() >= 3 && pts.len() <= MAX_VERTICES && pts.iter().all(|p| p.is_finite())).then_some(pts)
}

fn layer_filter(p: &Value) -> Result<Option<Vec<String>>> {
    match p.get("layers") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(a)) => {
            let names: Option<Vec<String>> = a.iter().map(|v| v.as_str().map(str::to_string)).collect();
            names.map(Some).ok_or_else(|| bad(ID, "`layers` must be an array of layer names"))
        }
        Some(_) => Err(bad(ID, "`layers` must be an array of layer names")),
    }
}

fn on_layers(e: &Entity, layers: &Option<Vec<String>>) -> bool {
    layers.as_ref().is_none_or(|ls| ls.iter().any(|l| l.eq_ignore_ascii_case(&e.common.layer)))
}

/// Closed objects inside `outline` (not the outline itself): the cores, shafts and columns.
fn holes_inside(d: &Drawing, outline: &[Vec2], skip: Option<Handle>, layers: &Option<Vec<String>>, unit: f64) -> Vec<Vec<Vec2>> {
    let ob = Bounds2::from_points(outline.iter().copied());
    let floor_area = shoelace(outline).abs();
    let Some(store) = d.space(&Space::Model) else { return Vec::new() };
    let mut out = Vec::new();
    for e in store.iter() {
        if out.len() >= MAX_HOLES {
            break;
        }
        if Some(e.handle) == skip || !d.is_visible(e) || !on_layers(e, layers) || !ob.contains_box(&entity_bounds(d, e, 0)) {
            continue;
        }
        let Some(pts) = loop_points(d, e) else { continue };
        let area = shoelace(&pts).abs();
        if area / (unit * unit) < MIN_HOLE_AREA || area >= floor_area * 0.6 {
            continue;
        }
        if pts.iter().all(|q| point_in_polygon(outline, *q)) {
            out.push(pts);
        }
    }
    out
}

/// The floor from the command's parameters:
/// - `points`: the outline's vertices;
/// - `boundary`: a closed object (default: the one selected closed object, else the largest
///   closed object in model space);
/// - `point`: a point inside the floor, whose boundary is traced as HATCH does (walls drawn as
///   lines work too);
///
/// plus `holes` (closed objects) or, by default, the closed objects found inside the outline,
/// `layers` (only these layers' objects count), `entrance` and `units` / `unitsPerMeter`.
pub fn from_params(s: &mut Session, p: &Value) -> Result<Floor> {
    let layers = layer_filter(p)?;
    let (outline, boundary, traced_holes) = outline(s, p, &layers)?;
    let d = s.doc()?;
    let b = Bounds2::from_points(outline.iter().copied());
    let unit = match (p.get("unitsPerMeter").and_then(Value::as_f64), str_param(p, "units")) {
        (Some(u), _) if u.is_finite() && u > 0.0 => u,
        (Some(_), _) => return Err(bad(ID, "`unitsPerMeter` must be a positive number")),
        (None, Some(name)) => named_units(name).ok_or_else(|| bad(ID, format!("unknown units \"{name}\" (mm, cm, m, in, ft)")))?,
        (None, None) => units_per_metre(d.header.i64("INSUNITS", 0)).unwrap_or_else(|| guess_units(&b)),
    };
    let holes = match p.get("holes") {
        None | Some(Value::Null) => match traced_holes {
            Some(h) => h,
            None => holes_inside(d, &outline, boundary, &layers, unit),
        },
        Some(Value::Array(a)) => {
            if a.len() > MAX_HOLES {
                return Err(bad(ID, format!("at most {MAX_HOLES} holes")));
            }
            let mut out = Vec::new();
            for v in a {
                let h = handle_value(v).ok_or_else(|| bad(ID, format!("`holes`: {v} is not an object handle")))?;
                let e = d.entity(h).ok_or_else(|| bad(ID, format!("`holes`: no object {}", h.hex())))?;
                out.push(loop_points(d, e).ok_or_else(|| bad(ID, format!("`holes`: {} is not a closed object", h.hex())))?);
            }
            out
        }
        Some(_) => return Err(bad(ID, "`holes` must be an array of object handles")),
    };
    let entrance = match p.get("entrance") {
        None | Some(Value::Null) => None,
        Some(_) => Some(point_param(p, "entrance").ok_or_else(|| bad(ID, "`entrance` must be [x, y]"))?),
    };
    Ok(Floor { outline, holes, entrance, unit })
}

type Outline = (Vec<Vec2>, Option<Handle>, Option<Vec<Vec<Vec2>>>);

fn outline(s: &mut Session, p: &Value, layers: &Option<Vec<String>>) -> Result<Outline> {
    if let Some(v) = p.get("points").filter(|v| !v.is_null()) {
        let pts = points_param(p, "points").ok_or_else(|| bad(ID, format!("`points` must be [[x, y]…], got {v}")))?;
        if pts.len() < 3 || pts.len() > MAX_VERTICES {
            return Err(bad(ID, format!("`points` needs 3 to {MAX_VERTICES} vertices")));
        }
        return Ok((pts, None, None));
    }
    if let Some(at) = p.get("point").filter(|v| !v.is_null()) {
        let q = point_param(p, "point").ok_or_else(|| bad(ID, format!("`point` must be [x, y], got {at}")))?;
        return traced(s, q, layers);
    }
    let d = s.doc()?;
    let h = match p.get("boundary") {
        Some(v) if !v.is_null() => Some(handle_value(v).ok_or_else(|| bad(ID, format!("`boundary`: {v} is not an object handle")))?),
        _ => {
            let sel = s.selection();
            match sel.as_slice() {
                [one] => Some(*one),
                _ => None,
            }
        }
    };
    if let Some(h) = h {
        let e = d.entity(h).ok_or_else(|| bad(ID, format!("no object {}", h.hex())))?;
        let pts = loop_points(d, e).ok_or_else(|| bad(ID, format!("{} is not a closed object (closed polyline, circle…)", h.hex())))?;
        return Ok((pts, Some(h), None));
    }
    // The largest closed object in model space.
    let store = d.space(&Space::Model).ok_or_else(|| bad(ID, "no model space"))?;
    let best = store
        .iter()
        .filter(|e| d.is_visible(e) && on_layers(e, layers))
        .filter_map(|e| loop_points(d, e).map(|pts| (shoelace(&pts).abs(), pts, e.handle)))
        .max_by(|a, b| a.0.total_cmp(&b.0));
    match best {
        Some((_, pts, h)) => Ok((pts, Some(h), None)),
        None => Err(bad(ID, "no floor outline: draw it as a closed polyline, select it, or give `point` inside the floor")),
    }
}

/// The boundary around `q` and its islands, traced over the given layers only.
fn traced(s: &mut Session, q: Vec2, layers: &Option<Vec<String>>) -> Result<Outline> {
    // Tracing reads what is visible: hide the other layers for the trace, then put them back.
    let saved: Vec<(String, bool)> = s.doc()?.layers.iter().map(|l| (l.name.clone(), l.on)).collect();
    if let Some(ls) = layers {
        for l in &mut s.doc_mut()?.layers {
            l.on = l.on && ls.iter().any(|n| n.eq_ignore_ascii_case(&l.name));
        }
    }
    let found = loops_at(s, q);
    if layers.is_some() {
        for l in &mut s.doc_mut()?.layers {
            if let Some((_, on)) = saved.iter().find(|(n, _)| *n == l.name) {
                l.on = *on;
            }
        }
    }
    let loops = found.map_err(|_| bad(ID, "no closed floor boundary around `point` (check for gaps in the walls)"))?;
    let d = s.doc()?;
    let unit_guess = {
        let b = Bounds2::from_points(loops.iter().filter(|l| l.outer).flat_map(|l| l.vertices.iter().map(|v| v.p)));
        units_per_metre(d.header.i64("INSUNITS", 0)).unwrap_or_else(|| guess_units(&b))
    };
    let mut outline = None;
    let mut holes = Vec::new();
    for l in loops {
        let b = Bounds2::from_points(l.vertices.iter().map(|v| v.p));
        let tol = ((b.width() + b.height()) * 1e-3).max(1e-9);
        let mut pts = Polyline { vertices: l.vertices, closed: true }.tessellate(tol);
        if pts.len() > 1 && pts.first().zip(pts.last()).is_some_and(|(a, b)| a.near(*b, 1e-12)) {
            pts.pop();
        }
        if pts.len() < 3 || pts.len() > MAX_VERTICES {
            continue;
        }
        if l.outer {
            outline = Some(pts);
        } else if shoelace(&pts).abs() / (unit_guess * unit_guess) >= MIN_HOLE_AREA && holes.len() < MAX_HOLES {
            holes.push(pts);
        }
    }
    let outline = outline.ok_or_else(|| bad(ID, "no closed floor boundary around `point`"))?;
    Ok((outline, None, Some(holes)))
}
