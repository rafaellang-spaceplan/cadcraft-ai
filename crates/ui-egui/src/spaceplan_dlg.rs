//! The Layout Options dialog (SPACEPLAN typed or from Tools > Space Planning): the floor plate
//! (the drawing open, or a PDF imported as the base), the briefing, which Claude reads into an
//! editable program in the background, then the options drawn by the `spaceplan` command and,
//! optionally, Claude's review of them. Everything it does goes through commands
//! (`pdfimport`, `spaceplan`); only the AI requests run here, off the UI thread.

use cadcraft_ai::spaceplan::{FloorInfo, Lang, Program, SpaceKind, SpaceSpec, Zone};
use egui::{RichText, vec2};
use serde_json::{Value, json};

use crate::CadApp;
use crate::theme::Tokens;

/// What a background AI request returns.
enum Answer {
    Program(Result<cadcraft_ai::Interpretation, cadcraft_ai::Error>),
    Review(Result<cadcraft_ai::Review, cadcraft_ai::Error>),
}

/// The dialog while open.
pub struct SpacePlanDlg {
    pub brief: String,
    /// A PDF briefing: its file name and bytes.
    pub brief_pdf: Option<(String, Vec<u8>)>,
    pub program: Program,
    pub assumptions: Vec<String>,
    pub fits: bool,
    pub options: u32,
    pub seed: u64,
    /// Only these layers make the floor (comma-separated; empty = all).
    pub layers: String,
    /// Drawing units when the drawing doesn't say ("" = from the drawing).
    pub units: String,
    pub pdf_scale: String,
    pub floor: Option<FloorInfo>,
    /// The last `spaceplan` result (for the review).
    pub result: Option<Value>,
    pub review: Option<cadcraft_ai::Review>,
    pub status: Option<(bool, String)>,
    pending: Option<std::sync::mpsc::Receiver<Answer>>,
}

impl Default for SpacePlanDlg {
    fn default() -> Self {
        let lang = if crate::i18n::current() == "pt-BR" { Lang::PtBr } else { Lang::En };
        SpacePlanDlg {
            brief: String::new(),
            brief_pdf: None,
            program: Program { lang, ..Program::default() },
            assumptions: Vec::new(),
            fits: true,
            options: 10,
            seed: 1,
            layers: String::new(),
            units: String::new(),
            pdf_scale: "1:100".into(),
            floor: None,
            result: None,
            review: None,
            status: None,
            pending: None,
        }
    }
}

impl SpacePlanDlg {
    fn busy(&self) -> bool {
        self.pending.is_some()
    }

    fn brief(&self) -> cadcraft_ai::Brief {
        cadcraft_ai::Brief { text: self.brief.clone(), pdf: self.brief_pdf.as_ref().map(|(_, b)| b.clone()) }
    }

    fn has_brief(&self) -> bool {
        !self.brief.trim().is_empty() || self.brief_pdf.is_some()
    }
}

/// Open the dialog (or bring it back as it was).
pub fn open(app: &mut CadApp) {
    if app.spaceplan_dlg.is_none() {
        app.spaceplan_dlg = Some(SpacePlanDlg::default());
    }
    app.ui.dialog = Some("spaceplan".into());
}

/// SPACEPLAN typed or from the Tools menu opens the dialog; JSON calls draw directly.
pub fn route(app: &mut CadApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    if matches!(id, "spaceplan" | "spl") && params.is_null() {
        open(app);
        return Some(Ok(Value::Null));
    }
    None
}

fn kind_label(k: SpaceKind) -> &'static str {
    match k {
        SpaceKind::Meeting => crate::tl!("Meeting room"),
        SpaceKind::Office => crate::tl!("Private office"),
        SpaceKind::PhoneBooth => crate::tl!("Phone booth"),
        SpaceKind::Reception => crate::tl!("Reception"),
        SpaceKind::Pantry => crate::tl!("Pantry"),
        SpaceKind::Collab => crate::tl!("Collaboration area"),
        SpaceKind::Storage => crate::tl!("Storage"),
        SpaceKind::Custom => crate::tl!("Other room"),
    }
}

fn zone_label(z: Option<Zone>) -> &'static str {
    match z {
        None | Some(Zone::Any) => crate::tl!("Any"),
        Some(Zone::Core) => crate::tl!("Core"),
        Some(Zone::Facade) => crate::tl!("Façade"),
        Some(Zone::Entrance) => crate::tl!("Entrance"),
    }
}

/// The floor parameters of `spaceplan`: the selected closed outline when one object is
/// selected, the layers and units typed.
fn floor_params(app: &CadApp, st: &SpacePlanDlg) -> Value {
    let mut p = json!({});
    if let [h] = app.session.selection().as_slice() {
        p["boundary"] = json!(h.hex());
    }
    let layers: Vec<&str> = st.layers.split(',').map(str::trim).filter(|l| !l.is_empty()).collect();
    if !layers.is_empty() {
        p["layers"] = json!(layers);
    }
    if !st.units.is_empty() {
        p["units"] = json!(st.units);
    }
    p
}

fn measure(app: &mut CadApp) {
    let Some(st) = app.spaceplan_dlg.as_ref() else { return };
    let mut p = floor_params(app, st);
    p["analyze"] = json!(true);
    let r = app.session.execute("spaceplan", &p);
    let Some(st) = app.spaceplan_dlg.as_mut() else { return };
    match r {
        Ok(v) => {
            st.floor = serde_json::from_value(v).ok();
            st.status = None;
        }
        Err(e) => {
            st.floor = None;
            st.status = Some((false, e.to_string()));
        }
    }
}

fn import_pdf(app: &mut CadApp) {
    let Some(pick) = app.services.pick_file.as_ref() else { return };
    let Some(path) = pick(crate::tl!("PDF drawings (PDF)"), &["pdf"]) else { return };
    let scale = app.spaceplan_dlg.as_ref().map_or_else(|| "1:100".to_string(), |s| s.pdf_scale.clone());
    let r = app.run("pdfimport", json!({ "path": path, "scale": scale }));
    let _ = app.run("zoom", json!({ "mode": "extents" }));
    if let Some(st) = app.spaceplan_dlg.as_mut() {
        st.status = Some(match r {
            Ok(v) => (true, crate::tf!("PDF imported: {n} objects.", n = v["entities"])),
            Err(e) => (false, e),
        });
    }
    measure(app);
}

fn load_brief(app: &mut CadApp) {
    let Some(pick) = app.services.pick_file.as_ref() else { return };
    let Some(path) = pick(crate::tl!("Briefings (TXT, MD, PDF)"), &["txt", "md", "pdf"]) else { return };
    let Some(st) = app.spaceplan_dlg.as_mut() else { return };
    match std::fs::read(&path) {
        Ok(bytes) if bytes.starts_with(b"%PDF") => {
            let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(path);
            st.brief_pdf = Some((name, bytes));
        }
        Ok(bytes) => st.brief = String::from_utf8_lossy(&bytes).into_owned(),
        Err(e) => st.status = Some((false, format!("{path}: {e}"))),
    }
}

/// Start an AI request in the background (native builds).
#[cfg(not(target_arch = "wasm32"))]
fn start(st: &mut SpacePlanDlg, ctx: &egui::Context, job: impl FnOnce() -> Answer + Send + 'static) {
    let (tx, rx) = std::sync::mpsc::channel();
    let ctx = ctx.clone();
    let spawned = std::thread::Builder::new().name("cadcraft-ai".into()).spawn(move || {
        let _ = tx.send(job());
        ctx.request_repaint();
    });
    match spawned {
        Ok(_) => st.pending = Some(rx),
        Err(e) => st.status = Some((false, e.to_string())),
    }
}

#[cfg(target_arch = "wasm32")]
fn start(st: &mut SpacePlanDlg, _ctx: &egui::Context, _job: impl FnOnce() -> Answer + Send + 'static) {
    st.status = Some((false, cadcraft_ai::Error::Unavailable.to_string()));
}

fn read_brief(app: &mut CadApp, ctx: &egui::Context) {
    let cfg = match cadcraft_ai::Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            if let Some(st) = app.spaceplan_dlg.as_mut() {
                st.status = Some((false, e.to_string()));
            }
            return;
        }
    };
    measure(app);
    let Some(st) = app.spaceplan_dlg.as_mut() else { return };
    let (brief, floor) = (st.brief(), st.floor.clone());
    st.status = Some((true, crate::tf!("Reading the briefing with {model}…", model = cfg.model)));
    start(st, ctx, move || Answer::Program(cadcraft_ai::interpret(&cfg, &brief, floor.as_ref())));
}

fn review(app: &mut CadApp, ctx: &egui::Context) {
    let Some(st) = app.spaceplan_dlg.as_mut() else { return };
    let cfg = match cadcraft_ai::Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            st.status = Some((false, e.to_string()));
            return;
        }
    };
    let (brief, result) = (st.brief(), st.result.clone().unwrap_or(Value::Null));
    st.status = Some((true, crate::tf!("Reviewing the options with {model}…", model = cfg.model)));
    start(st, ctx, move || Answer::Review(cadcraft_ai::review(&cfg, &brief, &result)));
}

fn generate(app: &mut CadApp) {
    let Some(st) = app.spaceplan_dlg.as_ref() else { return };
    let mut p = floor_params(app, st);
    p["program"] = serde_json::to_value(&st.program).unwrap_or(Value::Null);
    p["options"] = json!(st.options);
    p["seed"] = json!(st.seed);
    let r = app.run("spaceplan", p);
    let Some(st) = app.spaceplan_dlg.as_mut() else { return };
    match r {
        Ok(v) => {
            let n = v["options"].as_array().map_or(0, Vec::len);
            st.status = Some((true, crate::tf!("{n} layout options drawn: one tab each, and a comparison tab.", n = n)));
            st.result = Some(v);
            st.review = None;
        }
        Err(e) => st.status = Some((false, e)),
    }
}

/// Take a finished background answer.
fn poll(st: &mut SpacePlanDlg) {
    let Some(rx) = &st.pending else { return };
    let Ok(answer) = rx.try_recv() else {
        if matches!(rx.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected)) {
            st.pending = None;
        }
        return;
    };
    st.pending = None;
    match answer {
        Answer::Program(Ok(i)) => {
            st.program = i.program;
            st.assumptions = i.assumptions;
            st.fits = i.fits;
            st.status = Some((true, crate::tl!("Briefing read. Check the program, then generate the options.").into()));
        }
        Answer::Review(Ok(r)) => {
            st.review = Some(r);
            st.status = None;
        }
        Answer::Program(Err(e)) | Answer::Review(Err(e)) => st.status = Some((false, e.to_string())),
    }
}

fn program_editor(ui: &mut egui::Ui, p: &mut Program) {
    ui.horizontal(|ui| {
        let l = ui.label(crate::tl!("Workstations"));
        ui.add(egui::DragValue::new(&mut p.workstations.count).range(0..=cadcraft_ai::spaceplan::program::MAX_WORKSTATIONS)).labelled_by(l.id);
        let l = ui.label(crate::tl!("Desk (m)"));
        ui.add(egui::DragValue::new(&mut p.workstations.desk_width).speed(0.05).range(0.6..=3.0).max_decimals(2)).labelled_by(l.id);
        ui.label("×");
        ui.add(egui::DragValue::new(&mut p.workstations.desk_depth).speed(0.05).range(0.4..=3.0).max_decimals(2)).labelled_by(l.id);
    });
    let mut remove = None;
    egui::Grid::new("spaceplan_rooms").striped(true).num_columns(6).show(ui, |ui| {
        // Each cell is named by its column's heading.
        let room = ui.strong(crate::tl!("Room")).id;
        let name_h = ui.strong(crate::tl!("Name")).id;
        let count = ui.strong(crate::tl!("Count")).id;
        let seats_h = ui.strong(crate::tl!("Seats")).id;
        let location = ui.strong(crate::tl!("Location")).id;
        ui.label("");
        ui.end_row();
        for (i, s) in p.spaces.iter_mut().enumerate() {
            let kind = craft_ui::ComboBox::from_id_salt(("spaceplan_kind", i)).selected_text(kind_label(s.kind)).width(150.0).show_ui(ui, |ui| {
                for k in SpaceKind::ALL {
                    ui.selectable_value(&mut s.kind, k, kind_label(k));
                }
            });
            kind.response.labelled_by(room);
            let mut name = s.name.clone().unwrap_or_default();
            let field = egui::TextEdit::singleline(&mut name).hint_text(kind_label(s.kind));
            if ui.add_sized(vec2(150.0, 20.0), field).labelled_by(name_h).changed() {
                s.name = (!name.trim().is_empty()).then_some(name);
            }
            ui.add(egui::DragValue::new(&mut s.count).range(1..=200)).labelled_by(count);
            let mut seats = s.seats.unwrap_or(0);
            let auto = |v: f64, _| if v == 0.0 { crate::tl!("auto").to_string() } else { format!("{v}") };
            if ui.add(egui::DragValue::new(&mut seats).range(0..=60).custom_formatter(auto)).labelled_by(seats_h).changed() {
                s.seats = (seats > 0).then_some(seats);
            }
            let zone = craft_ui::ComboBox::from_id_salt(("spaceplan_zone", i)).selected_text(zone_label(s.zone)).width(100.0).show_ui(ui, |ui| {
                for z in [None, Some(Zone::Core), Some(Zone::Facade), Some(Zone::Entrance)] {
                    ui.selectable_value(&mut s.zone, z, zone_label(z));
                }
            });
            zone.response.labelled_by(location);
            if ui.small_button("×").on_hover_text(crate::tl!("Remove")).clicked() {
                remove = Some(i);
            }
            ui.end_row();
        }
    });
    if let Some(i) = remove {
        p.spaces.remove(i);
    }
    if ui.button(crate::tl!("Add room")).clicked() {
        p.spaces.push(SpaceSpec::new(SpaceKind::Meeting, 1));
    }
}

/// The Layout Options window.
pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    if app.spaceplan_dlg.is_none() {
        app.spaceplan_dlg = Some(SpacePlanDlg::default());
    }
    let t = Tokens::get();
    let has_picker = app.services.pick_file.is_some();
    let one_selected = app.session.selection().len() == 1;
    let (mut do_measure, mut do_import, mut do_load, mut do_read, mut do_generate, mut do_review) = (false, false, false, false, false, false);
    let Some(st) = app.spaceplan_dlg.as_mut() else { return };
    poll(st);
    if st.busy() {
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }
    egui::Window::new(crate::tl!("Layout Options"))
        .id(egui::Id::unique("spaceplan_dialog"))
        .open(open)
        .default_size(vec2(620.0, 640.0))
        .vscroll(true)
        .show(ctx, |ui| {
            ui.heading(crate::tl!("1. Floor plate"));
            ui.label(
                RichText::new(if one_selected {
                    crate::tl!("The selected closed outline is the floor; closed shapes inside it are cores and columns.")
                } else {
                    crate::tl!("The largest closed outline is the floor (select one to choose it); closed shapes inside it are cores and columns.")
                })
                .small(),
            );
            ui.horizontal(|ui| {
                let l = ui.label(crate::tl!("Layers"));
                ui.add(egui::TextEdit::singleline(&mut st.layers).desired_width(180.0).hint_text(crate::tl!("all"))).labelled_by(l.id);
                let units = ui.label(crate::tl!("Units"));
                let shown = if st.units.is_empty() { crate::tl!("from the drawing") } else { st.units.as_str() };
                let combo = craft_ui::ComboBox::from_id_salt("spaceplan_units").selected_text(shown).width(130.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut st.units, String::new(), crate::tl!("from the drawing"));
                    for u in ["mm", "cm", "m"] {
                        ui.selectable_value(&mut st.units, u.to_string(), u);
                    }
                });
                combo.response.labelled_by(units.id);
            });
            ui.horizontal(|ui| {
                do_import = ui.add_enabled(has_picker, egui::Button::new(crate::tl!("Import PDF base…"))).clicked();
                let l = ui.label(crate::tl!("at scale"));
                ui.add(egui::TextEdit::singleline(&mut st.pdf_scale).desired_width(60.0)).labelled_by(l.id);
                do_measure = ui.button(crate::tl!("Measure floor")).clicked();
            });
            if let Some(f) = &st.floor {
                ui.label(crate::tf!(
                    "Usable area {area} m² · {l} × {w} m · {cores} core(s) · {cols} column(s)",
                    area = format!("{:.0}", f.usable_area),
                    l = format!("{:.1}", f.length),
                    w = format!("{:.1}", f.width),
                    cores = f.cores,
                    cols = f.columns
                ));
            }
            ui.separator();
            ui.heading(crate::tl!("2. Briefing"));
            let brief_label = ui.label(RichText::new(crate::tl!("Briefing text")).small());
            ui.add(egui::TextEdit::multiline(&mut st.brief).desired_rows(5).desired_width(f32::INFINITY).hint_text(crate::tl!(
                "What the layout must hold: headcount, workstations, meeting rooms and their sizes, offices, booths, reception, pantry…"
            )))
            .labelled_by(brief_label.id);
            ui.horizontal(|ui| {
                do_load = ui.add_enabled(has_picker, egui::Button::new(crate::tl!("Load briefing file…"))).clicked();
                if let Some((name, _)) = &st.brief_pdf {
                    ui.label(RichText::new(name.as_str()).small());
                    if ui.small_button("×").on_hover_text(crate::tl!("Remove")).clicked() {
                        st.brief_pdf = None;
                    }
                }
                let can_read = st.has_brief() && !st.busy();
                do_read = ui.add_enabled(can_read, egui::Button::new(crate::tl!("Read briefing with AI"))).clicked();
                if st.busy() {
                    ui.add(egui::Spinner::new().color(t.accent));
                }
            });
            ui.separator();
            ui.heading(crate::tl!("3. Program"));
            program_editor(ui, &mut st.program);
            if !st.fits {
                ui.colored_label(t.warn, crate::tl!("The program may not fit the usable area."));
            }
            for a in &st.assumptions {
                ui.label(RichText::new(format!("• {a}")).small().weak());
            }
            ui.separator();
            ui.heading(crate::tl!("4. Options"));
            ui.horizontal(|ui| {
                let l = ui.label(crate::tl!("Options"));
                ui.add(egui::DragValue::new(&mut st.options).range(1..=cadcraft_ai::spaceplan::MAX_OPTIONS as u32)).labelled_by(l.id);
                let l = ui.label(crate::tl!("Variation"));
                ui.add(egui::DragValue::new(&mut st.seed).range(1..=9999)).labelled_by(l.id);
                let empty = st.program.workstations.count == 0 && st.program.spaces.is_empty();
                do_generate =
                    ui.add_enabled(!empty && !st.busy(), egui::Button::new(RichText::new(crate::tl!("Generate options")).strong())).clicked();
                do_review =
                    ui.add_enabled(st.result.is_some() && st.has_brief() && !st.busy(), egui::Button::new(crate::tl!("Review with AI"))).clicked();
            });
            if let Some(r) = &st.review {
                ui.label(RichText::new(crate::tf!("Recommended: option {n}", n = r.recommended)).strong());
                ui.label(r.summary.as_str());
                for o in &r.options {
                    ui.label(RichText::new(crate::tf!("Option {n}", n = o.option)).strong().small());
                    for p in &o.pros {
                        ui.label(RichText::new(format!("+ {p}")).small());
                    }
                    for c in &o.cons {
                        ui.label(RichText::new(format!("- {c}")).small().weak());
                    }
                }
            }
            if let Some((ok, msg)) = &st.status {
                ui.separator();
                if *ok {
                    ui.label(msg.as_str());
                } else {
                    ui.colored_label(t.warn, msg.as_str());
                }
            }
        });
    if do_import {
        import_pdf(app);
    }
    if do_measure {
        measure(app);
    }
    if do_load {
        load_brief(app);
    }
    if do_read {
        read_brief(app, ctx);
    }
    if do_generate {
        generate(app);
    }
    if do_review {
        review(app, ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadcraft_engine::Session;

    fn app() -> CadApp {
        let mut s = Session::new();
        s.execute("pline", &json!({ "points": [[0, 0], [30, 0], [30, 15], [0, 15]], "closed": true })).unwrap();
        s.doc_mut().unwrap().header.set_i64("INSUNITS", 6);
        CadApp::new(s, crate::Services::default())
    }

    #[test]
    fn typed_spaceplan_opens_the_dialog_and_json_calls_draw() {
        let mut app = app();
        app.run("spaceplan", Value::Null).unwrap();
        assert_eq!(app.ui.dialog.as_deref(), Some("spaceplan"));
        assert!(app.spaceplan_dlg.is_some());
        // A JSON call never opens it.
        app.ui.dialog = None;
        let r = app.run("spaceplan", json!({ "analyze": true })).unwrap();
        assert!(r["usableArea"].as_f64().unwrap() > 400.0);
        assert!(app.ui.dialog.is_none());
    }

    #[test]
    fn measure_and_generate_from_the_edited_program() {
        let mut app = app();
        open(&mut app);
        measure(&mut app);
        assert!(app.spaceplan_dlg.as_ref().unwrap().floor.as_ref().is_some_and(|f| (f.usable_area - 450.0).abs() < 2.0));
        {
            let st = app.spaceplan_dlg.as_mut().unwrap();
            st.program.workstations.count = 20;
            st.program.spaces.push(SpaceSpec::new(SpaceKind::Meeting, 2));
            st.options = 2;
        }
        generate(&mut app);
        let st = app.spaceplan_dlg.as_ref().unwrap();
        assert!(st.status.as_ref().is_some_and(|(ok, _)| *ok), "{:?}", st.status);
        assert_eq!(st.result.as_ref().unwrap()["options"].as_array().unwrap().len(), 2);
        assert!(app.session.doc().unwrap().layout("Option 01").is_some() || app.session.doc().unwrap().layout("Opção 01").is_some());
    }

    #[test]
    fn an_empty_program_reports_the_error() {
        let mut app = app();
        open(&mut app);
        generate(&mut app);
        assert!(app.spaceplan_dlg.as_ref().unwrap().status.as_ref().is_some_and(|(ok, _)| !ok));
    }

    #[test]
    fn answers_from_the_background_fill_the_program() {
        let mut st = SpacePlanDlg::default();
        let (tx, rx) = std::sync::mpsc::channel();
        st.pending = Some(rx);
        let program = Program { workstations: cadcraft_ai::spaceplan::Workstations { count: 33, ..Default::default() }, ..Program::default() };
        tx.send(Answer::Program(Ok(cadcraft_ai::Interpretation { program, assumptions: vec!["x".into()], fits: false }))).unwrap();
        poll(&mut st);
        assert!(!st.busy());
        assert_eq!(st.program.workstations.count, 33);
        assert!(!st.fits);
        let (tx, rx) = std::sync::mpsc::channel();
        st.pending = Some(rx);
        tx.send(Answer::Review(Err(cadcraft_ai::Error::NoCredentials))).unwrap();
        poll(&mut st);
        assert!(st.status.as_ref().is_some_and(|(ok, m)| !ok && m.contains("ANTHROPIC_API_KEY")));
        // A worker that died without answering doesn't leave the dialog busy.
        let (tx, rx) = std::sync::mpsc::channel::<Answer>();
        st.pending = Some(rx);
        drop(tx);
        poll(&mut st);
        assert!(!st.busy());
    }
}
