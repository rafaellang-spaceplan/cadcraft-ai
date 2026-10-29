//! PDFIMPORT: a PDF page's vector geometry as polylines in the drawing, at a scale.
//!
//! The PDF is read by `cadcraft-pdfin`. Paths land on `PDF_<layer>` layers when the PDF has
//! layers (optional-content groups, as CAD programs write them), else on `PDF_Geometry` (strokes)
//! and `PDF_Solid Fills` (fills); colours other than black and white are kept as true colours.

use cadcraft_color::{Color, Rgb};
use cadcraft_doc::Common;
use cadcraft_geom::{PolyVertex, Vec2};
use serde_json::{Value, json};

use super::helpers::{line, lwpoly};
use super::{CommandSpec, bad, bool_or, point_param, str_param};
use crate::{Result, Session};

const ID: &str = "pdfimport";
/// Largest PDF read.
#[cfg(not(target_arch = "wasm32"))]
const MAX_PDF_BYTES: usize = 256 << 20;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(ID, "PDF Import...", run).menu(&["Insert", "PDF Import..."]).params(
            "{path: .pdf, page? (1), scale?: \"1:100\" | 100 (the plot's scale: real size = paper size × 100; default 1:1), unitsPerPoint? (drawing units per PDF point, instead of scale), at?: [x,y] (where the page's lower-left corner goes, default 0,0), fills?: bool (solid fills as closed outlines, default true), layers?: bool (PDF layers become PDF_ layers, default true)} \
             → {entities, layers, page, pages, size: [w,h], unitsPerPoint, textSkipped, imagesSkipped, truncated}",
        ),
    ]
}

/// Drawing units per millimetre for the drawing's INSUNITS (unitless reads as millimetres).
fn units_per_mm(insunits: i64) -> f64 {
    match insunits {
        1 => 1.0 / 25.4,
        2 => 1.0 / 304.8,
        5 => 0.1,
        6 => 0.001,
        14 => 0.01,
        _ => 1.0,
    }
}

/// `1:100`, `1/100` or `100` → 100 (real size per paper size).
fn scale_ratio(v: &Value) -> Option<f64> {
    let r = match v {
        Value::Number(n) => n.as_f64()?,
        Value::String(t) => match t.trim().split_once([':', '/']) {
            Some((a, b)) => b.trim().parse::<f64>().ok()? / a.trim().parse::<f64>().ok()?,
            None => t.trim().parse().ok()?,
        },
        _ => return None,
    };
    (r.is_finite() && r > 0.0 && r < 1e7).then_some(r)
}

/// A layer name AutoCAD accepts.
fn layer_name(pdf_layer: &str) -> String {
    let clean: String = pdf_layer.trim().chars().map(|c| if "<>/\\\":;?*|,=`".contains(c) || c.is_control() { '_' } else { c }).take(240).collect();
    format!("PDF_{clean}")
}

#[cfg(not(target_arch = "wasm32"))]
fn read_file(path: &str) -> Result<Vec<u8>> {
    crate::capped_read::read_capped(std::path::Path::new(path), MAX_PDF_BYTES).map_err(|e| bad(ID, format!("{path}: {e}")))
}

#[cfg(target_arch = "wasm32")]
fn read_file(_path: &str) -> Result<Vec<u8>> {
    Err(bad(ID, "reading files is not available on the web yet"))
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_param(p, "path").ok_or_else(|| bad(ID, "`path` (a .pdf file) is required"))?;
    let page = match p.get("page") {
        None | Some(Value::Null) => 0,
        Some(v) => {
            v.as_u64().filter(|n| (1..=100_000).contains(n)).map(|n| n as usize - 1).ok_or_else(|| bad(ID, "`page` must be a page number from 1"))?
        }
    };
    let upp = match (p.get("unitsPerPoint"), p.get("scale")) {
        (Some(v), _) if !v.is_null() => {
            v.as_f64().filter(|u| u.is_finite() && *u > 0.0 && *u < 1e9).ok_or_else(|| bad(ID, "`unitsPerPoint` must be a positive number"))?
        }
        (_, Some(v)) if !v.is_null() => {
            let ratio = scale_ratio(v).ok_or_else(|| bad(ID, "`scale` must be like \"1:100\" or a positive number"))?;
            25.4 / 72.0 * ratio * units_per_mm(s.doc()?.header.i64("INSUNITS", 0))
        }
        _ => 25.4 / 72.0 * units_per_mm(s.doc()?.header.i64("INSUNITS", 0)),
    };
    let at = match p.get("at") {
        None | Some(Value::Null) => Vec2::ZERO,
        Some(_) => point_param(p, "at").ok_or_else(|| bad(ID, "`at` must be [x, y]"))?,
    };
    let use_layers = bool_or(p, "layers", true);
    let opts = cadcraft_pdfin::Options { page, fills: bool_or(p, "fills", true), ..cadcraft_pdfin::Options::default() };
    let bytes = read_file(path)?;
    let pdf = cadcraft_pdfin::read(&bytes, &opts).map_err(|e| bad(ID, format!("{path}: {e}")))?;
    let map = |q: Vec2| at + q * upp;
    let mut layers: Vec<String> = Vec::new();
    let mut count = 0usize;
    let space = s.space();
    let d = s.doc_mut()?;
    for path in &pdf.paths {
        let layer = match (&path.layer, use_layers, path.filled && !path.stroked) {
            (Some(l), true, _) => layer_name(l),
            (_, _, true) => "PDF_Solid Fills".to_string(),
            _ => "PDF_Geometry".to_string(),
        };
        if !layers.contains(&layer) {
            layers.push(layer.clone());
        }
        let [r, g, b] = path.color;
        let color = if (r, g, b) == (0, 0, 0) || (r, g, b) == (255, 255, 255) { Color::ByLayer } else { Color::True(Rgb(r, g, b)) };
        let pts: Vec<Vec2> = path.points.iter().map(|q| map(*q)).collect();
        let kind = match pts.as_slice() {
            [a, b] if !path.closed => line(*a, *b),
            _ => lwpoly(pts.iter().map(|q| PolyVertex::new(*q)).collect(), path.closed),
        };
        let common = Common { layer, color, ..Common::default() };
        d.add(&space, common, kind)?;
        count += 1;
    }
    let size = [pdf.width * upp, pdf.height * upp];
    let msg = format!(
        "{count} objects imported from page {} of {}{}{}",
        page + 1,
        pdf.pages,
        if pdf.text_skipped > 0 { format!("; {} text strings not imported", pdf.text_skipped) } else { String::new() },
        if pdf.truncated { "; the page was too large and was cut short" } else { "" }
    );
    s.echo(msg);
    Ok(json!({
        "entities": count,
        "layers": layers,
        "page": page + 1,
        "pages": pdf.pages,
        "size": size,
        "unitsPerPoint": upp,
        "textSkipped": pdf.text_skipped,
        "imagesSkipped": pdf.images_skipped,
        "truncated": pdf.truncated,
    }))
}

#[cfg(test)]
mod tests {
    use cadcraft_doc::EntityKind;
    use serde_json::json;

    use super::*;

    #[test]
    fn scales_and_names() {
        assert_eq!(scale_ratio(&json!("1:100")), Some(100.0));
        assert_eq!(scale_ratio(&json!("1/50")), Some(50.0));
        assert_eq!(scale_ratio(&json!(20)), Some(20.0));
        for bad in [json!("0:1"), json!("x"), json!(-1), json!(null), json!("1:0")] {
            assert_eq!(scale_ratio(&bad), None, "{bad}");
        }
        assert_eq!(layer_name(" A-WALL/EXT "), "PDF_A-WALL_EXT");
    }

    /// A drawing plotted to PDF by CADCraft comes back at the same size and place.
    #[test]
    fn a_plot_round_trips_at_scale() {
        let mut s = Session::new();
        s.new_drawing(true);
        s.execute("rectang", &json!({ "p1": [0, 0], "p2": [20000, 10000] })).unwrap();
        s.execute("circle", &json!({ "center": [10000, 5000], "radius": 2000 })).unwrap();
        // Plot model space at 1:100 on A3 landscape, the lower-left corner at the paper origin.
        let d = s.doc().unwrap().clone();
        let opts = cadcraft_io::PdfOptions {
            compress: true,
            paper: Some("A3".into()),
            landscape: Some(true),
            fit: Some(false),
            scale: Some(0.01),
            center: Some(false),
            offset: Some([0.0, 0.0]),
            ..cadcraft_io::PdfOptions::default()
        };
        let bytes = cadcraft_io::pdf(&d, &cadcraft_doc::Space::Model, &opts).unwrap();
        let dir = std::env::temp_dir().join(format!("cadcraft-pdfimport-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("plot.pdf");
        std::fs::write(&path, &bytes).unwrap();
        let mut t = Session::new();
        t.new_drawing(true);
        let r = t.execute(ID, &json!({ "path": path.to_str().unwrap(), "scale": "1:100" })).unwrap();
        assert!(r["entities"].as_u64().unwrap() >= 2, "{r}");
        // CADCraft's plots keep the drawing's layers as PDF layers.
        assert_eq!(r["layers"], json!(["PDF_0"]));
        // The rectangle comes back as a closed polyline, 20 × 10 m again at 1:100.
        let d = t.doc().unwrap();
        let rects: Vec<cadcraft_geom::Bounds2> = d
            .space(&cadcraft_doc::Space::Model)
            .unwrap()
            .iter()
            .filter_map(|e| match &e.kind {
                EntityKind::LwPolyline(p) if p.closed && p.vertices.len() == 4 => {
                    Some(cadcraft_geom::Bounds2::from_points(p.vertices.iter().map(|v| v.p)))
                }
                _ => None,
            })
            .collect();
        let biggest = rects.iter().max_by(|a, b| (a.width() * a.height()).total_cmp(&(b.width() * b.height()))).unwrap();
        assert!((biggest.width() - 20000.0).abs() < 20.0 && (biggest.height() - 10000.0).abs() < 20.0, "{biggest:?}");
        // Imported again at 1:200: twice the size.
        let r2 = t.execute(ID, &json!({ "path": path.to_str().unwrap(), "scale": "1:200", "at": [100000, 0] })).unwrap();
        assert!((r2["size"][0].as_f64().unwrap() / r["size"][0].as_f64().unwrap() - 2.0).abs() < 1e-9);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bad_input_is_refused() {
        let mut s = Session::new();
        for p in [
            json!({}),
            json!({ "path": "/nonexistent/x.pdf" }),
            json!({ "path": "/", "page": 0 }),
            json!({ "path": "/", "scale": "abc" }),
            json!({ "path": "/", "unitsPerPoint": -1 }),
            json!({ "path": "/", "at": "x" }),
            json!({ "path": "/" }),
        ] {
            assert!(s.execute(ID, &p).is_err(), "{p}");
        }
        let dir = std::env::temp_dir().join(format!("cadcraft-pdfimport-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("not.pdf");
        std::fs::write(&path, b"PK\x03\x04 not a pdf").unwrap();
        assert!(s.execute(ID, &json!({ "path": path.to_str().unwrap() })).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
