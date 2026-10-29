use std::path::{Path, PathBuf};

use cadcraft_engine::Session;
use serde_json::{Value, json};

use super::args::SpaceplanArgs;
use super::error::{CliError, Result};
use super::output::{Streams, export};
use super::{open, unicode};

/// Largest briefing or program file read.
const MAX_INPUT_BYTES: u64 = 32 << 20;

fn read_capped(path: &Path) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?.take(MAX_INPUT_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_INPUT_BYTES {
        return Err(CliError::Spaceplan(format!("{}: larger than {} MB", path.display(), MAX_INPUT_BYTES >> 20)));
    }
    Ok(bytes)
}

fn brief(a: &SpaceplanArgs) -> Result<cadcraft_ai::Brief> {
    let mut b = cadcraft_ai::Brief::text(a.brief_text.clone().unwrap_or_default());
    if let Some(path) = &a.brief {
        let bytes = read_capped(path)?;
        if bytes.starts_with(b"%PDF") {
            b.pdf = Some(bytes);
        } else {
            b.text = String::from_utf8_lossy(&bytes).into_owned();
        }
    }
    Ok(b)
}

/// The floor parameters shared by `spaceplan {analyze}` and `spaceplan`.
fn floor_params(a: &SpaceplanArgs) -> Value {
    let mut p = json!({});
    if let Some([x, y]) = a.point {
        p["point"] = json!([x, y]);
    }
    if let Some([x, y]) = a.entrance {
        p["entrance"] = json!([x, y]);
    }
    if !a.layers.is_empty() {
        p["layers"] = json!(a.layers);
    }
    if let Some(u) = &a.units {
        p["units"] = json!(u);
    }
    p
}

fn default_out(base: &Path) -> PathBuf {
    let stem = base.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "drawing".into());
    base.with_file_name(format!("{stem}-layouts.dxf"))
}

/// Open the base plan; a PDF's vector content is imported into a new metric drawing.
fn open_base(s: &mut Session, a: &SpaceplanArgs, io: &mut Streams) -> Result {
    let is_pdf = a.base.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
    if !is_pdf {
        return open(s, &a.base);
    }
    s.new_drawing(true);
    let r = s.execute("pdfimport", &json!({ "path": unicode(&a.base)?, "scale": a.pdf_scale, "page": a.pdf_page }))?;
    io.errln(format_args!("PDF imported at {}: {} objects on {}", a.pdf_scale, r["entities"], r["layers"]));
    Ok(())
}

pub(crate) fn spaceplan(a: &SpaceplanArgs, io: &mut Streams) -> Result {
    let mut s = Session::empty();
    open_base(&mut s, a, io)?;
    let floor = floor_params(a);
    let mut summary = json!({});
    let brief = if a.brief.is_some() || a.brief_text.is_some() { Some(brief(a)?) } else { None };
    let program: Value = match &a.program {
        Some(path) => serde_json::from_slice(&read_capped(path)?).map_err(|e| CliError::Spaceplan(format!("{}: {e}", path.display())))?,
        None => {
            let brief = brief.as_ref().ok_or_else(|| CliError::Spaceplan("give --brief, --brief-text or --program".into()))?;
            let cfg = cadcraft_ai::Config::from_env()?;
            // Measure the floor first, so the briefing is read against its area.
            let info = s.execute("spaceplan", &with(&floor, "analyze", json!(true)))?;
            let info: Option<cadcraft_ai::spaceplan::FloorInfo> = serde_json::from_value(info.clone()).ok();
            io.errln(format_args!("Reading the briefing with {}…", cfg.model));
            let read = cadcraft_ai::interpret(&cfg, brief, info.as_ref())?;
            summary["assumptions"] = json!(read.assumptions);
            summary["fits"] = json!(read.fits);
            let program = serde_json::to_value(&read.program).map_err(|e| CliError::Spaceplan(e.to_string()))?;
            if let Some(path) = &a.save_program {
                std::fs::write(path, serde_json::to_string_pretty(&program).unwrap_or_default())?;
            }
            program
        }
    };
    summary["program"] = program.clone();
    let mut p = with(&floor, "program", program);
    p["options"] = json!(a.options);
    p["seed"] = json!(a.seed);
    let result = s.execute("spaceplan", &p)?;
    for l in &s.log {
        io.errln(format_args!("{l}"));
    }
    if a.review
        && let Some(brief) = &brief
    {
        let cfg = cadcraft_ai::Config::from_env()?;
        io.errln(format_args!("Reviewing the options with {}…", cfg.model));
        summary["review"] = serde_json::to_value(cadcraft_ai::review(&cfg, brief, &result)?).unwrap_or(Value::Null);
    }
    let out = a.out.clone().unwrap_or_else(|| default_out(&a.base));
    unicode(&out)?;
    export(&s, &out, io)?;
    summary["result"] = result;
    summary["output"] = json!(out.to_string_lossy());
    io.json(&summary, a.compact);
    Ok(())
}

fn with(base: &Value, key: &str, v: Value) -> Value {
    let mut p = base.clone();
    p[key] = v;
    p
}
