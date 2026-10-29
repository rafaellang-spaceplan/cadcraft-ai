//! SPACEPLAN: office layout options for a floor plate from a space program.
//!
//! The floor comes from the drawing (`floor.rs`), the options from `cadcraft-spaceplan`, and each
//! option is drawn on its own layers with a layout tab, a layer state and a line in a comparison
//! table (`draw.rs`). Reading a free-text briefing into the program is the AI layer's job; this
//! command takes the structured program.

mod draw;
mod floor;

use std::collections::BTreeMap;

use cadcraft_spaceplan::{Lang, PieceKind, Program, Settings};
use serde_json::Value;

use super::{CommandSpec, bad, bool_or, str_param};
use crate::{Result, Session};

const ID: &str = "spaceplan";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(ID, "Generate Layout Options...", run)
            .menu(&["Tools", "Space Planning", "Generate Layout Options..."])
            .alias(&["spl"])
            .params(
                "{program: {workstations?: {count, deskWidth? (1.2 m), deskDepth? (0.7 m), perSide?}, spaces?: [{kind: meeting|office|phone_booth|reception|pantry|collab|storage|custom, count?, seats?, width?, depth? | area? (m, m²), name?, enclosed?, zone?: core|facade|entrance}], corridor? (1.2 m), mainCorridor? (1.5 m), lang?: en|pt-BR}, \
                 floor: points?: [[x,y]…] | boundary?: closed object (default: the selected one, else the largest) | point?: [x,y] inside the floor (traced like HATCH), holes?: [closed objects] (default: closed objects inside), layers?: [layer] (only these count), entrance?: [x,y], units?: mm|cm|m|in|ft | unitsPerMeter? (default: INSUNITS), \
                 analyze?: bool (measure the floor only → {usableArea, length, width, cores, columns, hasEntrance, unitsPerMeter}), options? (10), candidates? (48), seed? (1), prefix? (OPT / OPC), layouts?: bool (default true), blocks?: {desk|chair|table|round_table|sofa|counter|cabinet: block name}} \
                 → {options: [{name, layers, score, description, strategy, metrics}], usableArea, candidates, unitsPerMeter}",
            ),
    ]
}

fn usize_or(p: &Value, key: &str, default: usize, max: usize) -> Result<usize> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => {
            v.as_u64().filter(|n| (1..=max as u64).contains(n)).map(|n| n as usize).ok_or_else(|| bad(ID, format!("`{key}` must be 1 to {max}")))
        }
    }
}

fn blocks_param(p: &Value) -> Result<BTreeMap<PieceKind, String>> {
    match p.get("blocks") {
        None | Some(Value::Null) => Ok(BTreeMap::new()),
        Some(Value::Object(m)) => {
            let mut out = BTreeMap::new();
            for (k, v) in m {
                let kind: PieceKind = serde_json::from_value(Value::String(k.clone()))
                    .map_err(|_| bad(ID, format!("`blocks`: unknown furniture \"{k}\" (desk, chair, table, round_table, sofa, counter, cabinet)")))?;
                let name = v.as_str().ok_or_else(|| bad(ID, format!("`blocks.{k}` must be a block name")))?;
                out.insert(kind, name.to_string());
            }
            Ok(out)
        }
        Some(_) => Err(bad(ID, "`blocks` must map furniture kinds to block names")),
    }
}

fn snapshot(s: &Session) -> Result<Snapshot> {
    let st = s.state()?;
    Ok((st.doc.clone(), st.space.clone(), st.mspace, st.selection.clone()))
}

fn restore(s: &mut Session, before: Snapshot) -> Result<()> {
    let st = s.state_mut()?;
    st.doc = before.0;
    st.space = before.1;
    st.mspace = before.2;
    st.selection = before.3;
    Ok(())
}

type Snapshot = (std::sync::Arc<cadcraft_doc::Drawing>, cadcraft_doc::Space, Option<cadcraft_doc::Handle>, Vec<cadcraft_doc::Handle>);

fn to_model(s: &mut Session) -> Result<()> {
    if s.space() != cadcraft_doc::Space::Model {
        super::find_command("layout.set").map(|c| (c.run)(s, &serde_json::json!({ "name": "Model" }))).transpose()?;
    }
    Ok(())
}

/// `analyze: true`: measure the floor, change nothing.
fn analyze(s: &mut Session, p: &Value) -> Result<Value> {
    let before = snapshot(s)?;
    let result = (|| {
        to_model(s)?;
        let floor = floor::from_params(s, p)?;
        let info = cadcraft_spaceplan::floor_info(&floor).map_err(|e| bad(ID, e.to_string()))?;
        let mut v = serde_json::to_value(info).map_err(|e| bad(ID, e.to_string()))?;
        if let Some(o) = v.as_object_mut() {
            o.insert("unitsPerMeter".into(), floor.unit.into());
        }
        Ok(v)
    })();
    // Tracing may have touched layer visibility: put the very same drawing back.
    restore(s, before)?;
    result
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    if bool_or(p, "analyze", false) {
        return analyze(s, p);
    }
    let raw = p.get("program").filter(|v| v.is_object()).ok_or_else(|| bad(ID, "`program` (object) is required"))?;
    let program: Program = serde_json::from_value(raw.clone()).map_err(|e| bad(ID, format!("program: {e}")))?;
    let settings = Settings {
        options: usize_or(p, "options", 10, cadcraft_spaceplan::MAX_OPTIONS)?,
        candidates: usize_or(p, "candidates", 48, cadcraft_spaceplan::MAX_CANDIDATES)?,
        seed: match p.get("seed") {
            None | Some(Value::Null) => 1,
            Some(v) => v.as_u64().ok_or_else(|| bad(ID, "`seed` must be a whole number"))?,
        },
    };
    let blocks = blocks_param(p)?;
    let prefix = match str_param(p, "prefix") {
        Some(x) if !x.trim().is_empty() && x.len() <= 16 && x.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') => x.to_string(),
        Some(_) => return Err(bad(ID, "`prefix` must be 1 to 16 letters, digits, - or _")),
        None => if program.lang == Lang::PtBr { "OPC" } else { "OPT" }.to_string(),
    };
    // The floor lives in model space; tracing and drawing work there.
    let before = snapshot(s)?;
    let result = (|| {
        to_model(s)?;
        let floor = floor::from_params(s, p)?;
        let plan = cadcraft_spaceplan::generate(&floor, &program, &settings).map_err(|e| bad(ID, e.to_string()))?;
        if plan.options.is_empty() {
            return Err(bad(ID, "no layout could be made on this floor"));
        }
        let style = draw::Style { lang: program.lang, prefix, layouts: bool_or(p, "layouts", true), blocks, unit: floor.unit };
        let out = draw::draw(s, &plan, &style)?;
        let n = plan.options.len();
        let best = plan.options.first().map_or(0.0, |o| o.score);
        s.echo(match program.lang {
            Lang::En => format!("{n} layout options created (best scores {best:.0}/100)."),
            Lang::PtBr => format!("{n} opções de layout criadas (a melhor tem nota {best:.0}/100)."),
        });
        Ok(out)
    })();
    if result.is_err() {
        // Nothing half-made stays behind.
        restore(s, before)?;
    }
    result
}

#[cfg(test)]
mod tests;
