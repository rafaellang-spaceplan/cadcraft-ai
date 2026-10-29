//! Drawing the layout options: each option on its own layers in model space (overlaid on the
//! floor), a layout tab per option whose viewport freezes the other options' layers, a layer
//! state per option and a comparison tab with a table of the options' figures.

use std::collections::BTreeMap;
use std::sync::Arc;

use cadcraft_color::Color;
use cadcraft_doc::{Arc as ArcEnt, Block, Common, Drawing, Entity, EntityKind, Insert, Layer, Lineweight, MText, Space};
use cadcraft_geom::{Bounds2, PolyVertex, Vec2, Vec3, ccw_sweep};
use cadcraft_spaceplan::{Lang, LayoutOption, Piece, PieceKind, Plan, Room};
use serde_json::{Value, json};

use super::super::helpers::{line, lwpoly, rect_vertices};
use super::super::{bad, find_command};
use super::ID;
use crate::{Result, Session};

/// The four layers of one option.
pub struct OptionLayers {
    pub walls: String,
    pub furniture: String,
    pub text: String,
    pub zones: String,
}

impl OptionLayers {
    fn all(&self) -> [&str; 4] {
        [&self.walls, &self.furniture, &self.text, &self.zones]
    }
}

/// What to draw and how.
pub struct Style {
    pub lang: Lang,
    /// Layer name prefix, e.g. `OPC` → `OPC01-PAREDES`.
    pub prefix: String,
    pub layouts: bool,
    /// Existing blocks to use for furniture kinds (base point at the piece's centre, front
    /// towards −y), instead of CADCraft's own simple blocks.
    pub blocks: BTreeMap<PieceKind, String>,
    /// Drawing units per metre.
    pub unit: f64,
}

fn names(lang: Lang) -> (&'static str, [&'static str; 4], &'static str) {
    match lang {
        Lang::En => ("Option", ["WALLS", "FURNITURE", "TEXT", "ZONES"], "Comparison"),
        Lang::PtBr => ("Opção", ["PAREDES", "MOBILIARIO", "TEXTO", "ZONAS"], "Comparativo"),
    }
}

fn number(v: f64, decimals: usize, lang: Lang) -> String {
    let s = format!("{v:.decimals$}");
    if lang == Lang::PtBr { s.replace('.', ",") } else { s }
}

/// MTEXT contents with the characters MTEXT treats as codes escaped.
fn mtext_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('{', "\\{").replace('}', "\\}")
}

/// A free suffix ("", "B", "C"…) so a second run doesn't mix with the first one's layers and tabs.
fn free_suffix(d: &Drawing, prefix: &str, title: &str) -> Result<String> {
    for suffix in std::iter::once(String::new()).chain(('B'..='Z').map(String::from)) {
        let p = format!("{prefix}{suffix}01-");
        let taken = d.layers.iter().any(|l| l.name.to_ascii_uppercase().starts_with(&p.to_ascii_uppercase()))
            || d.layout(&layout_name(title, 1, &suffix)).is_some();
        if !taken {
            return Ok(suffix);
        }
    }
    Err(bad(ID, format!("layers {prefix}A01- to {prefix}Z01- are all taken: give another `prefix`")))
}

fn layout_name(title: &str, i: usize, suffix: &str) -> String {
    if suffix.is_empty() { format!("{title} {i:02}") } else { format!("{title} {i:02} {suffix}") }
}

fn add_layer(d: &mut Drawing, name: &str, color: u8, lw: i16) {
    if d.layer(name).is_none() {
        d.layers.push(Layer { name: name.into(), color: Color::Index(color), lineweight: Lineweight::from_dxf(lw), ..Layer::default() });
    }
}

fn mtext(at: Vec2, height: f64, width: f64, attach: u8, contents: String) -> MText {
    MText {
        insert: at.to3(0.0),
        height,
        width,
        attach,
        rotation: 0.0,
        style: "Standard".into(),
        contents,
        line_spacing: 1.0,
        line_spacing_exact: false,
        columns: None,
    }
}

fn common(layer: &str) -> Common {
    Common { layer: layer.into(), ..Common::default() }
}

/// Draw `plan`; returns the JSON result of the command.
pub fn draw(s: &mut Session, plan: &Plan, style: &Style) -> Result<Value> {
    let (title, suffixes, compare_title) = names(style.lang);
    let suffix = free_suffix(s.doc()?, &style.prefix, title)?;
    let layers: Vec<OptionLayers> = (1..=plan.options.len())
        .map(|i| {
            let l = |k: usize| format!("{}{suffix}{i:02}-{}", style.prefix, suffixes.get(k).copied().unwrap_or("X"));
            OptionLayers { walls: l(0), furniture: l(1), text: l(2), zones: l(3) }
        })
        .collect();
    let mut floor_bounds = Bounds2::EMPTY;
    {
        let d = s.doc_mut()?;
        for (o, ls) in plan.options.iter().zip(&layers) {
            add_layer(d, &ls.walls, 7, 35);
            add_layer(d, &ls.furniture, 4, 18);
            add_layer(d, &ls.text, 2, 18);
            add_layer(d, &ls.zones, 3, 13);
            draw_option(d, o, ls, style, &mut floor_bounds)?;
        }
    }
    let mut out = Vec::new();
    for (i, (o, ls)) in plan.options.iter().zip(&layers).enumerate() {
        out.push(json!({
            "name": layout_name(title, i + 1, &suffix),
            "layers": ls.all(),
            "score": (o.score * 10.0).round() / 10.0,
            "description": o.description,
            "strategy": o.strategy,
            "metrics": o.metrics,
        }));
    }
    if style.layouts {
        layer_states(s, &layers, title, &suffix)?;
        for (i, (o, ls)) in plan.options.iter().zip(&layers).enumerate() {
            let name = layout_name(title, i + 1, &suffix);
            option_layout(s, &name, o, ls, &layers, floor_bounds, style.lang)?;
        }
        let name = if suffix.is_empty() { compare_title.to_string() } else { format!("{compare_title} {suffix}") };
        comparison_layout(s, &name, plan, title, &suffix, style.lang)?;
        // Show the first option.
        exec(s, "layout.set", json!({ "name": layout_name(title, 1, &suffix) }))?;
    }
    Ok(json!({ "options": out, "usableArea": (plan.usable_area * 10.0).round() / 10.0, "candidates": plan.candidates, "unitsPerMeter": style.unit }))
}

/// Run another command as part of this one (its undo step is folded into ours by the caller).
fn exec(s: &mut Session, id: &str, p: Value) -> Result<Value> {
    let spec = find_command(id).ok_or_else(|| bad(ID, format!("{id} is missing")))?;
    (spec.run)(s, &p)
}

fn draw_option(d: &mut Drawing, o: &LayoutOption, ls: &OptionLayers, style: &Style, bounds: &mut Bounds2) -> Result<()> {
    for r in &o.rooms {
        for c in r.outline {
            bounds.add(c);
        }
        draw_room(d, r, ls, style)?;
    }
    for p in &o.furniture {
        bounds.add(p.center);
        let block = furniture_block(d, p, style);
        let scale = Vec3::new(1.0, 1.0, 1.0);
        let kind = EntityKind::Insert(Insert {
            block,
            insert: p.center.to3(0.0),
            scale,
            rotation: p.angle,
            attribs: Vec::new(),
            cols: 1,
            rows: 1,
            col_spacing: 0.0,
            row_spacing: 0.0,
        });
        d.add(&Space::Model, common(&ls.furniture), kind)?;
    }
    Ok(())
}

fn draw_room(d: &mut Drawing, r: &Room, ls: &OptionLayers, style: &Style) -> Result<()> {
    let u = style.unit;
    let pts = |v: &[Vec2]| v.iter().map(|p| PolyVertex::new(*p)).collect::<Vec<_>>();
    if r.enclosed {
        // Partitions 10 cm thick, open at the door.
        let closed = r.door.is_none();
        let mut walls = lwpoly(pts(&r.walls), closed);
        if let EntityKind::LwPolyline(p) = &mut walls {
            p.const_width = 0.1 * u;
        }
        d.add(&Space::Model, common(&ls.walls), walls)?;
        if let Some(door) = r.door {
            d.add(&Space::Model, common(&ls.walls), line(door.hinge, door.swing))?;
            let (a1, a2) = ((door.end - door.hinge).angle(), (door.swing - door.hinge).angle());
            let (start, end) = if ccw_sweep(a1, a2) <= std::f64::consts::PI { (a1, a2) } else { (a2, a1) };
            let arc = ArcEnt { center: door.hinge.to3(0.0), radius: door.hinge.dist(door.end), start, end };
            d.add(&Space::Model, common(&ls.walls), EntityKind::Arc(arc))?;
        }
    } else {
        d.add(&Space::Model, common(&ls.zones), lwpoly(pts(&r.outline), true))?;
    }
    let height = if r.area < 4.0 { 0.12 } else { 0.2 } * u;
    let contents = format!("{}\\P{} m²", mtext_escape(&r.name), number(r.area, 1, style.lang));
    let text = mtext(r.center, height, 0.0, 5, contents);
    d.add(&Space::Model, common(&ls.text), EntityKind::MText(text))?;
    Ok(())
}

/// The block drawn for a piece: the user's, when they mapped one that exists, else one of ours
/// of the piece's size (made on first use).
fn furniture_block(d: &mut Drawing, p: &Piece, style: &Style) -> String {
    if let Some(name) = style.blocks.get(&p.kind)
        && let Some(b) = d.block(name)
    {
        return b.name.clone();
    }
    let cm = |v: f64| (v / style.unit * 100.0).round() as i64;
    let tag = match p.kind {
        PieceKind::Desk => "DESK",
        PieceKind::Chair => "CHAIR",
        PieceKind::Table => "TABLE",
        PieceKind::RoundTable => "ROUNDTABLE",
        PieceKind::Sofa => "SOFA",
        PieceKind::Counter => "COUNTER",
        PieceKind::Cabinet => "CABINET",
    };
    let name = format!("SP_{tag}_{}x{}", cm(p.width), cm(p.depth));
    if d.block(&name).is_none() {
        let mut b = Block::new(&name);
        b.description = "CADCraft space planning furniture".into();
        for kind in block_geometry(p.kind, p.width, p.depth, style.unit) {
            let h = d.new_handle();
            b.entities.push(Entity::new(h, kind));
        }
        d.blocks.insert(name.clone(), Arc::new(b));
    }
    name
}

/// A piece's outline, centred on the origin, width along x, front (the user's side) towards −y.
fn block_geometry(kind: PieceKind, w: f64, dep: f64, u: f64) -> Vec<EntityKind> {
    let (hw, hd) = (w / 2.0, dep / 2.0);
    let rect = |x0: f64, y0: f64, x1: f64, y1: f64| lwpoly(rect_vertices(Vec2::new(x0, y0), Vec2::new(x1, y1)), true);
    match kind {
        PieceKind::Desk => {
            // The worktop and a screen at the back.
            let sw = (0.25 * u).min(hw * 0.8);
            vec![rect(-hw, -hd, hw, hd), rect(-sw, hd - 0.12 * u, sw, hd - 0.08 * u)]
        }
        PieceKind::Chair => {
            let k = w / (0.5 * u);
            let s = |v: f64| v * u * k;
            vec![rect(s(-0.22), s(-0.17), s(0.22), s(0.23)), rect(s(-0.24), s(-0.25), s(0.24), s(-0.17))]
        }
        PieceKind::Table | PieceKind::Counter => {
            let mut v = vec![rect(-hw, -hd, hw, hd)];
            if kind == PieceKind::Counter {
                v.push(line(Vec2::new(-hw, -hd + 0.05 * u), Vec2::new(hw, -hd + 0.05 * u)));
            }
            v
        }
        PieceKind::RoundTable => vec![super::super::helpers::circle(Vec2::ZERO, hw)],
        PieceKind::Sofa => {
            let (back, arm) = ((0.2 * u).min(dep / 3.0), (0.15 * u).min(w / 6.0));
            vec![rect(-hw, -hd, hw, hd), rect(-hw, hd - back, hw, hd), rect(-hw, -hd, -hw + arm, hd - back), rect(hw - arm, -hd, hw, hd - back)]
        }
        PieceKind::Cabinet => vec![rect(-hw, -hd, hw, hd), line(Vec2::new(-hw, -hd), Vec2::new(hw, hd))],
    }
}

/// A layer state per option: its layers on, the other options' off (LAYERSTATE restores them).
fn layer_states(s: &mut Session, layers: &[OptionLayers], title: &str, suffix: &str) -> Result<()> {
    let saved: Vec<(String, bool)> = s.doc()?.layers.iter().map(|l| (l.name.clone(), l.on)).collect();
    for i in 0..layers.len() {
        for l in &mut s.doc_mut()?.layers {
            if let Some(owner) = layers.iter().position(|ls| ls.all().contains(&l.name.as_str())) {
                l.on = owner == i;
            }
        }
        exec(s, "layerstate.save", json!({ "name": layout_name(title, i + 1, suffix) }))?;
    }
    for l in &mut s.doc_mut()?.layers {
        if let Some((_, on)) = saved.iter().find(|(n, _)| *n == l.name) {
            l.on = *on;
        }
    }
    Ok(())
}

fn paper_text(s: &mut Session, layout: &str, at: Vec2, height: f64, width: f64, contents: String) -> Result<()> {
    let t = mtext(at, height, width, 1, contents);
    s.doc_mut()?.add(&Space::Paper(layout.into()), Common::default(), EntityKind::MText(t))?;
    Ok(())
}

fn option_layout(s: &mut Session, name: &str, o: &LayoutOption, mine: &OptionLayers, all: &[OptionLayers], floor: Bounds2, lang: Lang) -> Result<()> {
    let r = exec(s, "layout.new", json!({ "name": name }))?;
    let vp_handle = r.get("viewport").and_then(Value::as_str).and_then(cadcraft_doc::Handle::parse_hex);
    let frozen: Vec<String> = all.iter().filter(|ls| ls.walls != mine.walls).flat_map(|ls| ls.all().map(str::to_string)).collect();
    let mut band = None;
    if let Some(h) = vp_handle {
        let d = s.doc_mut()?;
        d.modify_entity(h, |e| {
            if let EntityKind::Viewport(vp) = &mut e.kind {
                // Leave a band at the bottom of the sheet for the option's figures.
                let (left, bottom, top) = (vp.center.x - vp.width / 2.0, vp.center.y - vp.height / 2.0, vp.center.y + vp.height / 2.0);
                let band_h = vp.height * 0.16;
                vp.height -= band_h;
                vp.center.y = (top + bottom + band_h) / 2.0;
                band = Some((Vec2::new(left, bottom + band_h * 0.85), vp.width));
                if !floor.is_empty() {
                    vp.view_center = floor.center();
                    let aspect = vp.height / vp.width.max(1e-9);
                    vp.view_height = (floor.height() * 1.08).max(floor.width() * 1.08 * aspect);
                }
                vp.frozen_layers = frozen.clone();
            }
        })?;
    }
    let Some((at, width)) = band else { return Ok(()) };
    let m = &o.metrics;
    let pct = |v: f64| format!("{}%", number(v * 100.0, 0, lang));
    let (ws, rooms, light, per, score) = match lang {
        Lang::En => ("Workstations", "Rooms", "Daylight", "m²/person", "Score"),
        Lang::PtBr => ("Estações", "Salas", "Luz natural", "m²/pessoa", "Nota"),
    };
    let header = format!("{{\\fArial|b1;{}}} — {}", mtext_escape(name), mtext_escape(&o.description));
    let figures = format!(
        "{ws} {}/{} · {rooms} {}/{} · {light} {} · {} {per} · {score} {}",
        m.workstations,
        m.workstations_target,
        m.rooms,
        m.rooms_target,
        pct(m.daylight),
        number(m.area_per_person, 1, lang),
        number(o.score, 0, lang)
    );
    let missing: Vec<String> =
        m.spaces.iter().filter(|c| c.placed < c.target).map(|c| format!("{} {}/{}", mtext_escape(&c.name), c.placed, c.target)).collect();
    let mut contents = format!("{header}\\P{figures}");
    if !missing.is_empty() {
        let label = if lang == Lang::PtBr { "Não coube" } else { "Did not fit" };
        contents.push_str(&format!("\\P{label}: {}", missing.join(", ")));
    }
    let mm = cadcraft_render::sheet(s.doc()?, name).map_or(1.0, |sh| sh.unit_mm);
    paper_text(s, name, at, 3.5 / mm, width, contents)
}

/// Short labels of an option's strategy: rooms, bench axis, island size, central corridor.
fn strategy_cells(o: &LayoutOption, lang: Lang) -> [String; 4] {
    use cadcraft_spaceplan::{BenchAxis, RoomPlacement};
    let st = &o.strategy;
    let pt = lang == Lang::PtBr;
    let rooms = match (st.rooms, pt) {
        (RoomPlacement::Core, true) => "núcleo",
        (RoomPlacement::Core, false) => "core",
        (RoomPlacement::Facade, true) => "fachada",
        (RoomPlacement::Facade, false) => "façade",
        (RoomPlacement::StartEnd, true) => "lado A",
        (RoomPlacement::StartEnd, false) => "side A",
        (RoomPlacement::FarEnd, true) => "lado B",
        (RoomPlacement::FarEnd, false) => "side B",
    };
    let axis = match (st.benches, pt) {
        (BenchAxis::Along, true) => "longitudinal",
        (BenchAxis::Along, false) => "lengthwise",
        (BenchAxis::Across, true) => "transversal",
        (BenchAxis::Across, false) => "crosswise",
    };
    let spine = match (st.spine, pt) {
        (true, true) => "sim",
        (false, true) => "não",
        (true, false) => "yes",
        (false, false) => "no",
    };
    [rooms.into(), axis.into(), (st.per_side * 2).to_string(), spine.into()]
}

fn comparison_layout(s: &mut Session, name: &str, plan: &Plan, title: &str, suffix: &str, lang: Lang) -> Result<()> {
    exec(s, "layout.new", json!({ "name": name, "viewport": false }))?;
    exec(s, "layout.set", json!({ "name": name }))?;
    let header: Vec<&str> = match lang {
        Lang::En => vec!["Option", "Rooms at", "Benches", "Island", "Spine", "Workstations", "Rooms", "Daylight", "m²/person", "Score"],
        Lang::PtBr => vec!["Opção", "Salas em", "Sentido", "Ilha", "Circ. central", "Estações", "Salas", "Luz natural", "m²/pessoa", "Nota"],
    };
    let cells: Vec<Vec<String>> = plan
        .options
        .iter()
        .enumerate()
        .map(|(i, o)| {
            let m = &o.metrics;
            let mut row = vec![layout_name(title, i + 1, suffix)];
            row.extend(strategy_cells(o, lang));
            row.extend([
                format!("{}/{}", m.workstations, m.workstations_target),
                format!("{}/{}", m.rooms, m.rooms_target),
                format!("{}%", number(m.daylight * 100.0, 0, lang)),
                number(m.area_per_person, 1, lang),
                number(o.score, 0, lang),
            ]);
            row
        })
        .collect();
    let table_title = match lang {
        Lang::En => format!("Layout options — usable area {} m²", number(plan.usable_area, 1, lang)),
        Lang::PtBr => format!("Opções de layout — área útil {} m²", number(plan.usable_area, 1, lang)),
    };
    // Fill the printable area's width, from its top-left corner.
    let sheet = cadcraft_render::sheet(s.doc()?, name);
    let (at, width, mm) = sheet.map_or((Vec2::new(10.0, 200.0), 277.0, 1.0), |sh| {
        let m = 5.0 / sh.unit_mm;
        (Vec2::new(sh.printable.min.x + m, sh.printable.max.y - m), sh.printable.width() - 2.0 * m, sh.unit_mm)
    });
    let col = width / header.len() as f64;
    exec(s, "table", json!({ "at": [at.x, at.y], "cells": cells, "header": header, "title": table_title, "rowHeight": 9.0 / mm, "colWidth": col }))?;
    Ok(())
}
