use cadcraft_doc::{EntityKind, Space};
use serde_json::{Value, json};

use crate::Session;

/// A 40 × 20 m floor drawn in millimetres, with an 8 × 6 m core and two columns.
fn tower() -> Session {
    let mut s = Session::new();
    s.doc_mut().unwrap().header.set_i64("INSUNITS", 4);
    s.execute("pline", &json!({ "points": [[0, 0], [40000, 0], [40000, 20000], [0, 20000]], "closed": true })).unwrap();
    s.execute("pline", &json!({ "points": [[16000, 7000], [24000, 7000], [24000, 13000], [16000, 13000]], "closed": true })).unwrap();
    s.execute("circle", &json!({ "center": [8000, 10000], "radius": 300 })).unwrap();
    s.execute("circle", &json!({ "center": [32000, 10000], "radius": 300 })).unwrap();
    s.set_selection(Vec::new());
    s
}

fn program() -> Value {
    json!({
        "workstations": {"count": 48},
        "spaces": [
            {"kind": "meeting", "count": 3, "seats": 6},
            {"kind": "office", "count": 2},
            {"kind": "phone_booth", "count": 2},
            {"kind": "reception"},
            {"kind": "pantry"}
        ],
        "lang": "pt-BR"
    })
}

fn layer_entities(s: &Session, layer: &str) -> Vec<EntityKind> {
    let d = s.doc().unwrap();
    d.space(&Space::Model).unwrap().iter().filter(|e| e.common.layer == layer).map(|e| e.kind.clone()).collect()
}

#[test]
fn options_get_layers_tabs_layer_states_and_a_comparison() {
    let mut s = tower();
    let before = s.doc().unwrap().entity_count();
    let r = s.execute("spaceplan", &json!({ "program": program(), "options": 4 })).unwrap();
    let opts = r["options"].as_array().unwrap();
    assert_eq!(opts.len(), 4);
    assert_eq!(r["unitsPerMeter"], 1000.0, "INSUNITS = millimetres");
    assert!((r["usableArea"].as_f64().unwrap() - 751.4).abs() < 5.0, "{}", r["usableArea"]);
    assert_eq!(opts[0]["name"], "Opção 01");
    assert_eq!(opts[0]["metrics"]["workstations"], 48);
    let d = s.doc().unwrap();
    for i in 1..=4 {
        for suffix in ["PAREDES", "MOBILIARIO", "TEXTO", "ZONAS"] {
            assert!(d.layer(&format!("OPC{i:02}-{suffix}")).is_some());
        }
        let name = format!("Opção {i:02}");
        let layout = d.layout(&name).unwrap_or_else(|| panic!("{name} tab"));
        let vp = layout.entities.iter().find_map(|e| if let EntityKind::Viewport(v) = &e.kind { Some(v.clone()) } else { None }).unwrap();
        // The viewport shows only its option.
        assert_eq!(vp.frozen_layers.len(), 3 * 4);
        assert!(!vp.frozen_layers.iter().any(|l| l.starts_with(&format!("OPC{i:02}-"))));
        assert!(vp.view_center.near(cadcraft_geom::Vec2::new(20000.0, 10000.0), 1.0));
        // A title band with the figures under the viewport.
        assert!(layout.entities.iter().any(|e| matches!(&e.kind, EntityKind::MText(t) if t.contents.contains("Estações 48/48"))));
        assert!(d.layer_states.iter().any(|st| st.name == name));
    }
    let cmp = d.layout("Comparativo").unwrap();
    assert!(cmp.entities.iter().any(|e| matches!(e.kind, EntityKind::Table(_))));
    // Furniture is inserted as blocks; rooms have walls, doors and labels.
    let furniture = layer_entities(&s, "OPC01-MOBILIARIO");
    assert!(furniture.iter().all(|k| matches!(k, EntityKind::Insert(_))) && furniture.len() > 96);
    assert!(s.doc().unwrap().block("SP_DESK_120x70").is_some());
    let walls = layer_entities(&s, "OPC01-PAREDES");
    assert!(walls.iter().any(|k| matches!(k, EntityKind::LwPolyline(p) if p.const_width == 100.0)));
    assert!(walls.iter().any(|k| matches!(k, EntityKind::Arc(_))), "door swings");
    let text = layer_entities(&s, "OPC01-TEXTO");
    assert!(text.iter().any(|k| matches!(k, EntityKind::MText(t) if t.contents.starts_with("Sala de reunião\\P"))));
    // The first option's tab is shown; one undo removes everything.
    assert_eq!(s.space(), Space::Paper("Opção 01".into()));
    s.undo().unwrap();
    let d = s.doc().unwrap();
    assert_eq!(d.entity_count(), before);
    assert!(d.layout("Opção 01").is_none() && d.layer("OPC01-PAREDES").is_none() && d.layer_states.is_empty());
}

#[test]
fn a_second_run_does_not_mix_with_the_first() {
    let mut s = tower();
    let p = json!({ "program": program(), "options": 2, "layouts": false });
    s.execute("spaceplan", &p).unwrap();
    let r = s.execute("spaceplan", &p).unwrap();
    assert_eq!(r["options"][0]["layers"][0], "OPCB01-PAREDES");
    assert!(s.doc().unwrap().layout("Opção 01").is_none(), "layouts: false");
    assert_eq!(s.space(), Space::Model);
}

#[test]
fn floor_traced_from_walls_drawn_as_lines() {
    let mut s = Session::new();
    s.doc_mut().unwrap().header.set_i64("INSUNITS", 6);
    // Four separate walls and a core, in metres, on a WALLS layer; a stray rectangle elsewhere.
    s.execute("layer.new", &json!({ "name": "WALLS", "current": true })).unwrap();
    for (a, b) in [([0, 0], [30, 0]), ([30, 0], [30, 15]), ([30, 15], [0, 15]), ([0, 15], [0, 0])] {
        s.execute("line", &json!({ "points": [a, b] })).unwrap();
    }
    s.execute("rectang", &json!({ "p1": [12, 5], "p2": [18, 10] })).unwrap();
    s.execute("layer.new", &json!({ "name": "NOTES", "current": true })).unwrap();
    s.execute("rectang", &json!({ "p1": [2, 2], "p2": [3, 3] })).unwrap();
    let prog = json!({ "workstations": {"count": 24}, "spaces": [{"kind": "meeting", "count": 2}] });
    let r = s.execute("spaceplan", &json!({ "program": prog, "point": [5, 5], "layers": ["WALLS"], "options": 2, "layouts": false })).unwrap();
    // 450 m² less the 30 m² core: the NOTES rectangle didn't count as a column.
    assert!((r["usableArea"].as_f64().unwrap() - 420.0).abs() < 3.0, "{}", r["usableArea"]);
    assert_eq!(r["unitsPerMeter"], 1.0);
    assert_eq!(r["options"][0]["metrics"]["workstations"], 24);
    assert!(s.doc().unwrap().layer("NOTES").unwrap().on, "layers hidden for the trace are back on");
    assert!(s.doc().unwrap().layer("OPT01-WALLS").is_some(), "English layer names by default");
}

#[test]
fn user_blocks_replace_the_built_in_furniture() {
    let mut s = tower();
    let r = s.execute("circle", &json!({ "center": [-5000, 0], "radius": 300 })).unwrap();
    s.execute("-block", &json!({ "name": "MY_CHAIR", "base": [-5000, 0], "handles": [r["handle"]] })).unwrap();
    s.execute("spaceplan", &json!({ "program": program(), "options": 1, "layouts": false, "blocks": {"chair": "MY_CHAIR", "sofa": "NOPE"} }))
        .unwrap();
    let names: Vec<String> =
        layer_entities(&s, "OPC01-MOBILIARIO").into_iter().filter_map(|k| if let EntityKind::Insert(i) = k { Some(i.block) } else { None }).collect();
    assert!(names.iter().any(|n| n == "MY_CHAIR"));
    assert!(!names.iter().any(|n| n.starts_with("SP_CHAIR")));
}

#[test]
fn bad_input_changes_nothing() {
    let mut s = tower();
    let before = s.doc().unwrap().clone();
    let undo = s.state().unwrap().undo.len();
    for p in [
        json!({}),
        json!({ "program": 5 }),
        json!({ "program": {} }),
        json!({ "program": {"workstations": {"count": -1}} }),
        json!({ "program": program(), "options": 0 }),
        json!({ "program": program(), "options": 1000 }),
        json!({ "program": program(), "seed": -1 }),
        json!({ "program": program(), "units": "furlongs" }),
        json!({ "program": program(), "unitsPerMeter": -1 }),
        json!({ "program": program(), "unitsPerMeter": 1 }),
        json!({ "program": program(), "points": [[0, 0], [1, 1]] }),
        json!({ "program": program(), "points": "x" }),
        json!({ "program": program(), "point": [-99999, -99999] }),
        json!({ "program": program(), "boundary": "FFFFFF" }),
        json!({ "program": program(), "holes": ["zz"] }),
        json!({ "program": program(), "holes": 3 }),
        json!({ "program": program(), "entrance": "here" }),
        json!({ "program": program(), "prefix": "a b" }),
        json!({ "program": program(), "blocks": {"throne": "X"} }),
        json!({ "program": program(), "layers": "WALLS" }),
    ] {
        assert!(s.execute("spaceplan", &p).is_err(), "{p}");
        assert_eq!(*s.doc().unwrap(), before, "{p}");
        assert_eq!(s.state().unwrap().undo.len(), undo, "{p}");
    }
}

#[test]
fn analyze_measures_and_changes_nothing() {
    let mut s = tower();
    let doc = s.state().unwrap().doc.clone();
    let undo = s.state().unwrap().undo.len();
    let r = s.execute("spaceplan", &json!({ "analyze": true })).unwrap();
    assert!((r["usableArea"].as_f64().unwrap() - 751.4).abs() < 5.0, "{r}");
    assert_eq!(r["length"], 40.0);
    assert_eq!(r["cores"], 1);
    assert_eq!(r["columns"], 2);
    assert_eq!(r["unitsPerMeter"], 1000.0);
    assert!(std::sync::Arc::ptr_eq(&doc, &s.state().unwrap().doc));
    assert_eq!(s.state().unwrap().undo.len(), undo);
}

#[test]
fn a_sheet_frame_around_the_plan_is_not_the_floor() {
    let mut s = tower();
    // A frame and a title block around the plan, as a plotted sheet (or a PDF of one) has.
    s.execute("pline", &json!({ "points": [[-5000, -8000], [45000, -8000], [45000, 25000], [-5000, 25000]], "closed": true })).unwrap();
    s.execute("pline", &json!({ "points": [[-6000, -9000], [46000, -9000], [46000, 26000], [-6000, 26000]], "closed": true })).unwrap();
    s.set_selection(Vec::new());
    let r = s.execute("spaceplan", &json!({ "analyze": true })).unwrap();
    assert_eq!(r["length"], 40.0, "{r}");
}
