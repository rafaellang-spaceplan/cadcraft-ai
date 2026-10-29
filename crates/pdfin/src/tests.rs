use cadcraft_geom::Vec2;

use super::*;

/// A one-page PDF (595 × 842 pt) whose page draws `content`, with `resources` as the page's
/// resource dictionary and `extra` objects numbered from 5 (object 4 is the content stream).
fn pdf(content: &str, resources: &str, extra: &[&str]) -> Vec<u8> {
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R /Resources {resources} >>"),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len() + 1),
    ];
    objs.extend(extra.iter().map(|s| s.to_string()));
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for off in offsets {
        out.extend(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn near(a: Vec2, x: f64, y: f64) -> bool {
    a.near(Vec2::new(x, y), 1e-6)
}

#[test]
fn lines_rectangles_and_transforms() {
    let content = "0.5 w 1 0 0 RG\n10 10 m 110 10 l 110 60 l S\n2 0 0 2 100 200 cm\n0 0 0 rg 0 0 10 5 re f\n";
    let page = read(&pdf(content, "<< >>", &[]), &Options::default()).unwrap();
    assert_eq!((page.width, page.height, page.pages), (595.0, 842.0, 1));
    assert_eq!(page.paths.len(), 2);
    let a = &page.paths[0];
    assert!(a.stroked && !a.filled && !a.closed);
    assert_eq!(a.color, [255, 0, 0]);
    assert_eq!(a.width, 0.5);
    assert!(near(a.points[0], 10.0, 10.0) && near(a.points[2], 110.0, 60.0));
    let r = &page.paths[1];
    assert!(r.filled && r.closed && r.points.len() == 4);
    assert!(near(r.points[0], 100.0, 200.0) && near(r.points[2], 120.0, 210.0), "{:?}", r.points);
    assert_eq!(r.color, [0, 0, 0]);
    let no_fills = read(&pdf(content, "<< >>", &[]), &Options { fills: false, ..Options::default() }).unwrap();
    assert_eq!(no_fills.paths.len(), 1);
}

#[test]
fn a_path_back_at_its_start_is_closed() {
    let page = read(&pdf("0 0 m 10 0 l 10 10 l 0 10 l 0 0 l S 0 0 m 5 5 l 0 0 l S", "<< >>", &[]), &Options::default()).unwrap();
    assert!(page.paths[0].closed && page.paths[0].points.len() == 4, "{:?}", page.paths[0]);
    assert!(!page.paths[1].closed, "a line drawn there and back is not an area");
}

#[test]
fn curves_are_flattened_onto_the_curve() {
    // A quarter circle of radius 100 around the origin, as CAD programs write it.
    let k = 0.552_284_75 * 100.0;
    let content = format!("100 0 m 100 {k} {k} 100 0 100 c S");
    let page = read(&pdf(&content, "<< >>", &[]), &Options::default()).unwrap();
    let pts = &page.paths[0].points;
    assert!(pts.len() > 10);
    for p in pts {
        assert!((p.len() - 100.0).abs() < 0.1, "{p:?}");
    }
    assert!(near(*pts.last().unwrap(), 0.0, 100.0));
}

#[test]
fn state_stack_colours_and_layers() {
    let content = "q 0.5 0 0 0.5 0 0 cm /OC /L1 BDC 1 0 0 0 K 0 0 m 10 0 l S EMC Q\n\
                   /OC << /Type /OCG /Name (Inline) >> BDC 0.5 G 0 0 m 0 10 l S EMC\n\
                   /Artifact BMC 0 0 m 5 5 l S EMC\n\
                   BT /F1 12 Tf (hello) Tj ET";
    let res = "<< /Properties << /L1 5 0 R >> >>";
    let page = read(&pdf(content, res, &["<< /Type /OCG /Name <FEFF00500041005200450044004500530020> >>"]), &Options::default()).unwrap();
    assert_eq!(page.paths.len(), 3);
    assert_eq!(page.paths[0].layer.as_deref(), Some("PAREDES "));
    assert_eq!(page.paths[0].color, [0, 255, 255], "CMYK cyan");
    assert!(near(page.paths[0].points[1], 5.0, 0.0), "the q/Q-scoped transform");
    assert_eq!(page.paths[1].layer.as_deref(), Some("Inline"));
    assert_eq!(page.paths[1].color, [128, 128, 128]);
    assert!(near(page.paths[1].points[1], 0.0, 10.0), "Q restored the transform");
    assert_eq!(page.paths[2].layer, None);
    assert_eq!(page.layers, vec!["PAREDES ".to_string(), "Inline".to_string()]);
    assert_eq!(page.text_skipped, 1);
}

#[test]
fn form_xobjects_are_drawn_with_their_matrix() {
    let form = "<< /Type /XObject /Subtype /Form /BBox [0 0 100 100] /Matrix [1 0 0 1 50 0] /Length 13 >>\nstream\n0 0 m 10 0 l S\nendstream";
    let page = read(&pdf("1 0 0 1 0 100 cm /Fm1 Do /Fm1 Do", "<< /XObject << /Fm1 5 0 R >> >>", &[form]), &Options::default()).unwrap();
    assert_eq!(page.paths.len(), 2);
    assert!(near(page.paths[0].points[0], 50.0, 100.0) && near(page.paths[0].points[1], 60.0, 100.0), "{:?}", page.paths[0].points);
}

#[test]
fn hostile_pdfs_do_not_panic() {
    assert!(matches!(read(b"PK\x03\x04", &Options::default()), Err(Error::NotPdf(_))));
    assert!(
        read(b"%PDF-1.7\ngarbage", &Options::default()).is_err() || read(b"%PDF-1.7\ngarbage", &Options::default()).is_ok_and(|p| p.paths.is_empty())
    );
    assert!(matches!(read(&pdf("", "<< >>", &[]), &Options { page: 3, ..Options::default() }), Err(Error::NoPage { page: 3, pages: 1 })));
    // A form that draws itself: followed to the depth cap, then stopped.
    let form = "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Resources << /XObject << /Me 5 0 R >> >> /Length 21 >>\nstream\n0 0 m 1 1 l S /Me Do\nendstream";
    let page = read(&pdf("/Me Do", "<< /XObject << /Me 5 0 R >> >>", &[form]), &Options::default()).unwrap();
    assert!(page.truncated && page.paths.len() <= MAX_FORM_DEPTH + 1);
    // Unbalanced state and marked content, junk numbers, huge transforms.
    let junk = "Q Q Q EMC EMC q q q 1e308 0 0 1e308 0 0 cm 0 0 m 1e308 1e308 l S Q l c re h f S n 1 2 3 4 5 6 cm abc def BT ET";
    let page = read(&pdf(junk, "<< >>", &[]), &Options::default()).unwrap();
    assert!(page.paths.iter().all(|p| p.points.iter().all(|q| q.is_finite())));
    // The caps stop a page with too many paths.
    let many: String = (0..100).map(|i| format!("{i} 0 m {i} 1 l S\n")).collect();
    let page = read(&pdf(&many, "<< >>", &[]), &Options { max_paths: 10, ..Options::default() }).unwrap();
    assert!(page.truncated && page.paths.len() == 10);
}
