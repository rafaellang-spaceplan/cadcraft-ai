use cadcraft_geom::{Vec2, point_in_polygon};
use serde_json::json;

use super::*;

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Vec2> {
    vec![Vec2::new(x0, y0), Vec2::new(x1, y0), Vec2::new(x1, y1), Vec2::new(x0, y1)]
}

/// A 40 × 20 m floor with a 8 × 6 m core in the middle, in metres.
fn tower() -> Floor {
    Floor { outline: rect(0.0, 0.0, 40.0, 20.0), holes: vec![rect(16.0, 7.0, 24.0, 13.0)], entrance: Some(Vec2::new(20.0, 13.0)), unit: 1.0 }
}

fn office_program() -> Program {
    serde_json::from_value(json!({
        "workstations": {"count": 60},
        "spaces": [
            {"kind": "meeting", "count": 3, "seats": 6},
            {"kind": "meeting", "count": 1, "seats": 12, "name": "Board room"},
            {"kind": "office", "count": 2},
            {"kind": "phone_booth", "count": 3},
            {"kind": "reception"},
            {"kind": "pantry", "seats": 8}
        ]
    }))
    .unwrap()
}

fn inside_rect(r: &[Vec2], p: Vec2, tol: f64) -> bool {
    let (lo, hi) = r
        .iter()
        .fold((Vec2::new(f64::INFINITY, f64::INFINITY), Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY)), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
    p.x >= lo.x - tol && p.y >= lo.y - tol && p.x <= hi.x + tol && p.y <= hi.y + tol
}

fn bounds(r: &[Vec2]) -> (Vec2, Vec2) {
    r.iter().fold((Vec2::new(f64::INFINITY, f64::INFINITY), Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY)), |(lo, hi), v| (lo.min(*v), hi.max(*v)))
}

fn overlap(a: &[Vec2], b: &[Vec2]) -> f64 {
    let (alo, ahi) = bounds(a);
    let (blo, bhi) = bounds(b);
    let w = (ahi.x.min(bhi.x) - alo.x.max(blo.x)).max(0.0);
    let h = (ahi.y.min(bhi.y) - alo.y.max(blo.y)).max(0.0);
    w * h
}

#[test]
fn ten_distinct_options_that_meet_the_program() {
    let plan = generate(&tower(), &office_program(), &Settings::default()).unwrap();
    assert_eq!(plan.options.len(), 10);
    let best = &plan.options[0];
    assert!(best.score > 85.0, "best option scores {}: {:?}", best.score, best.metrics);
    assert_eq!(best.metrics.workstations, 60, "{:?}", best.metrics);
    assert_eq!(best.metrics.rooms, best.metrics.rooms_target, "{:?}", best.metrics);
    let mut descriptions = std::collections::HashSet::new();
    for (i, o) in plan.options.iter().enumerate() {
        assert!(descriptions.insert((o.description.clone(), o.strategy.variant)), "option {i} repeats a strategy");
        if i > 0 {
            assert!(o.score <= plan.options[i - 1].score + 1e-9, "options are sorted best first");
        }
    }
}

#[test]
fn rooms_and_furniture_stay_on_the_floor_and_off_the_core() {
    let floor = tower();
    let plan = generate(&floor, &office_program(), &Settings::default()).unwrap();
    let core = &floor.holes[0];
    for o in &plan.options {
        for (i, r) in o.rooms.iter().enumerate() {
            for c in r.outline {
                assert!(inside_rect(&floor.outline, c, 1e-6), "{} outside the floor: {c:?}", r.name);
            }
            assert!(overlap(&r.outline, core) < 1e-6, "{} overlaps the core", r.name);
            for other in &o.rooms[i + 1..] {
                assert!(overlap(&r.outline, &other.outline) < 1e-6, "{} overlaps {}", r.name, other.name);
            }
            assert!((r.area - overlap(&r.outline, &r.outline)).abs() < 1e-6);
            if r.enclosed {
                let d = r.door.expect("enclosed rooms get a door");
                assert!(inside_rect(&r.outline, d.swing, 1e-6), "the door swings into the room");
                assert_eq!(r.walls.len(), 6, "walls run from the door around the room");
            }
        }
        for p in &o.furniture {
            assert!(point_in_polygon(&floor.outline, p.center), "{p:?} outside the floor");
            assert!(!point_in_polygon(core, p.center), "{p:?} inside the core");
        }
        let desks = o.furniture.iter().filter(|p| p.kind == PieceKind::Desk).count() as u32;
        let offices = o.metrics.spaces.iter().filter(|s| s.kind == SpaceKind::Office).map(|s| s.placed).sum::<u32>();
        let booths = o.metrics.spaces.iter().filter(|s| s.kind == SpaceKind::PhoneBooth).map(|s| s.placed).sum::<u32>();
        assert_eq!(desks, o.metrics.workstations + offices + booths);
        let m = &o.metrics;
        assert!((m.room_area + m.workstation_area + m.circulation_area - m.usable_area).abs() < 1.0 || m.circulation_area == 0.0);
    }
}

#[test]
fn deterministic_for_a_seed_and_different_across_seeds() {
    let a = generate(&tower(), &office_program(), &Settings::default()).unwrap();
    let b = generate(&tower(), &office_program(), &Settings::default()).unwrap();
    assert_eq!(a, b);
    let c = generate(&tower(), &office_program(), &Settings { seed: 99, ..Settings::default() }).unwrap();
    assert_ne!(a.options.iter().map(|o| o.strategy).collect::<Vec<_>>(), c.options.iter().map(|o| o.strategy).collect::<Vec<_>>());
}

#[test]
fn rotated_floor_in_millimetres() {
    // The same tower turned 30° and drawn in millimetres far from the origin.
    let (a, o) = (30f64.to_radians(), Vec2::new(250_000.0, -80_000.0));
    let tf = |p: &Vec2| o + (*p * 1000.0).rotate(a);
    let t = tower();
    let floor = Floor {
        outline: t.outline.iter().map(tf).collect(),
        holes: t.holes.iter().map(|h| h.iter().map(tf).collect()).collect(),
        entrance: t.entrance.as_ref().map(tf),
        unit: 1000.0,
    };
    let plan = generate(&floor, &office_program(), &Settings { options: 3, ..Settings::default() }).unwrap();
    assert!((plan.usable_area - 752.0).abs() < 8.0, "usable area {} m²", plan.usable_area);
    let best = &plan.options[0];
    assert_eq!(best.metrics.workstations, 60);
    for r in &best.rooms {
        // Room edges follow the floor's rotation.
        let e = r.outline[1] - r.outline[0];
        let ang = e.angle().rem_euclid(std::f64::consts::FRAC_PI_2);
        assert!((ang - a).abs() < 1e-6 || (ang - (a + std::f64::consts::FRAC_PI_2)).abs() < 1e-6, "{ang}");
        assert!(r.area > 1.0 && r.area < 60.0, "areas are m²: {}", r.area);
    }
    for p in &best.furniture {
        assert!(point_in_polygon(&floor.outline, p.center));
        assert!(p.width > 100.0, "furniture sizes are drawing units");
    }
}

#[test]
fn floor_without_core_and_more_than_fits() {
    let floor = Floor { outline: rect(0.0, 0.0, 15.0, 10.0), holes: vec![rect(5.0, 5.0, 5.4, 5.4)], entrance: None, unit: 1.0 };
    let mut p = office_program();
    p.workstations.count = 400;
    let plan = generate(&floor, &p, &Settings { options: 4, candidates: 12, seed: 3 }).unwrap();
    assert!(!plan.options.is_empty());
    for o in &plan.options {
        assert!(o.metrics.workstations < 400 && o.score < 80.0, "{:?}", o.metrics);
    }
}

#[test]
fn program_reads_portuguese_briefings() {
    let p: Program = serde_json::from_value(json!({
        "estacoes": {"quantidade": 40, "larguraMesa": 1.4},
        "ambientes": [
            {"tipo": "reuniao", "quantidade": 2, "lugares": 8},
            {"tipo": "diretoria", "nome": "Diretoria"},
            {"tipo": "cabine", "quantidade": 4},
            {"tipo": "copa", "zona": "nucleo"},
            {"tipo": "recepcao"}
        ],
        "idioma": "pt-BR"
    }))
    .unwrap();
    assert_eq!(p.workstations.count, 40);
    assert_eq!(p.workstations.desk_width, 1.4);
    assert_eq!(p.spaces.len(), 5);
    assert_eq!(p.spaces[0].kind, SpaceKind::Meeting);
    assert_eq!(p.spaces[0].seat_count(), 8);
    assert_eq!(p.spaces[3].zone(), Zone::Core);
    assert_eq!(p.spaces[4].zone(), Zone::Entrance);
    assert_eq!(p.lang, Lang::PtBr);
    assert_eq!(p.spaces[2].display_name(p.lang), "Cabine");
    let plan = generate(&tower(), &p, &Settings { options: 2, ..Settings::default() }).unwrap();
    assert!(plan.options[0].description.contains("ilhas de"), "{}", plan.options[0].description);
}

#[test]
fn hostile_input_is_refused_without_panicking() {
    let ok = office_program();
    let bad_floors = [
        Floor { outline: vec![], ..tower() },
        Floor { outline: rect(0.0, 0.0, f64::NAN, 10.0), ..tower() },
        Floor { outline: rect(0.0, 0.0, 1e300, 1e300), ..tower() },
        Floor { outline: rect(0.0, 0.0, 1.0, 1.0), ..tower() },
        Floor { unit: 0.0, ..tower() },
        Floor { unit: f64::INFINITY, ..tower() },
        Floor { outline: vec![Vec2::ZERO; MAX_VERTICES + 1], ..tower() },
        Floor { outline: vec![Vec2::ZERO, Vec2::new(10.0, 0.0), Vec2::new(20.0, 0.0)], ..tower() },
    ];
    for f in &bad_floors {
        assert!(generate(f, &ok, &Settings::default()).is_err(), "{:?}", f.outline.get(..3));
    }
    // Holes that are garbage are ignored, not fatal.
    let f = Floor { holes: vec![vec![Vec2::new(f64::NAN, 0.0); 3], vec![Vec2::ZERO], rect(-1e9, -1e9, 1e9, 1e9)], ..tower() };
    let plan = generate(&f, &ok, &Settings { options: 1, candidates: 1, seed: 0 }).unwrap();
    assert_eq!(plan.options.len(), 1);
    let bad_programs = [
        json!({}),
        json!({"workstations": {"count": 100000}}),
        json!({"workstations": {"count": 4, "deskWidth": 0}}),
        json!({"workstations": {"count": 4, "deskDepth": 1e300}}),
        json!({"workstations": {"count": 4}, "corridor": 0.01}),
        json!({"spaces": [{"kind": "meeting", "count": 100000}]}),
        json!({"spaces": [{"kind": "custom", "width": 3}]}),
        json!({"spaces": [{"kind": "custom", "width": -3, "depth": 2}]}),
        json!({"spaces": [{"kind": "custom", "area": 1e12}]}),
    ];
    for p in bad_programs {
        let program: Program = serde_json::from_value(p.clone()).unwrap();
        assert!(generate(&tower(), &program, &Settings::default()).is_err(), "{p}");
    }
    // Extreme settings are clamped.
    let plan = generate(&tower(), &ok, &Settings { options: usize::MAX, candidates: usize::MAX, seed: u64::MAX }).unwrap();
    assert!(plan.options.len() <= MAX_OPTIONS && plan.candidates <= MAX_CANDIDATES);
    // Rooms bigger than the floor are clamped and simply not placed.
    let huge: Program = serde_json::from_value(json!({"spaces": [{"kind": "custom", "width": 1e9, "depth": 1e9}]})).unwrap();
    let plan = generate(&tower(), &huge, &Settings { options: 1, candidates: 1, seed: 0 }).unwrap();
    assert_eq!(plan.options[0].metrics.rooms, 0);
}
