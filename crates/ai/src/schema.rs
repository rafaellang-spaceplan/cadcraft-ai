//! The JSON schemas Claude's answers must follow (structured outputs) and the instructions that
//! go with them.
//!
//! Structured outputs need every object closed (`additionalProperties: false`) and every
//! property listed in `required`; optional values are spelled `anyOf [T, null]`.

use serde_json::{Value, json};

fn nullable(t: Value) -> Value {
    json!({ "anyOf": [t, { "type": "null" }] })
}

fn object(props: Value) -> Value {
    let required: Vec<String> = props.as_object().map(|m| m.keys().cloned().collect()).unwrap_or_default();
    json!({ "type": "object", "properties": props, "required": required, "additionalProperties": false })
}

/// A space program as `cadcraft_spaceplan::Program` reads it, with the reading's assumptions.
pub fn program() -> Value {
    let kinds = ["meeting", "office", "phone_booth", "reception", "pantry", "collab", "storage", "custom"];
    let space = object(json!({
        "kind": { "type": "string", "enum": kinds },
        "name": nullable(json!({ "type": "string", "description": "Room name as the briefing calls it, or null for the kind's default" })),
        "count": { "type": "integer", "description": "How many rooms of this line" },
        "seats": nullable(json!({ "type": "integer", "description": "People seated in one room" })),
        "width": nullable(json!({ "type": "number", "description": "Metres, only when the briefing gives the size" })),
        "depth": nullable(json!({ "type": "number", "description": "Metres, only with width" })),
        "area": nullable(json!({ "type": "number", "description": "m² of one room, when the briefing gives an area instead of sides" })),
        "enclosed": nullable(json!({ "type": "boolean", "description": "Walls and a door; null for the kind's default" })),
        "zone": nullable(json!({ "type": "string", "enum": ["core", "facade", "entrance"], "description": "Only when the briefing asks for a location" })),
    }));
    let workstations = object(json!({
        "count": { "type": "integer", "description": "Open-plan desks" },
        "deskWidth": { "type": "number", "description": "Metres (1.2 unless the briefing says otherwise)" },
        "deskDepth": { "type": "number", "description": "Metres (0.7 unless the briefing says otherwise)" },
        "perSide": nullable(json!({ "type": "integer", "description": "Desks per side of a bench, only when the briefing fixes the island size" })),
    }));
    object(json!({
        "program": object(json!({
            "workstations": workstations,
            "spaces": { "type": "array", "items": space },
            "corridor": { "type": "number", "description": "Secondary circulation width, metres (1.2 unless stated)" },
            "mainCorridor": { "type": "number", "description": "Main circulation width, metres (1.5 unless stated)" },
            "lang": { "type": "string", "enum": ["en", "pt-BR"] },
        })),
        "assumptions": { "type": "array", "items": { "type": "string" }, "description": "Each interpretation or default the briefing didn't settle, one short sentence each" },
        "fits": { "type": "boolean", "description": "Whether the program plausibly fits the usable area" },
    }))
}

/// A review of the generated options.
pub fn review() -> Value {
    object(json!({
        "recommended": { "type": "integer", "description": "Number of the recommended option (1-based)" },
        "summary": { "type": "string", "description": "Two or three sentences: which option and why" },
        "options": { "type": "array", "items": object(json!({
            "option": { "type": "integer" },
            "pros": { "type": "array", "items": { "type": "string" } },
            "cons": { "type": "array", "items": { "type": "string" } },
        })) },
    }))
}

pub const INTERPRET_SYSTEM: &str = "\
You turn an office fit-out briefing into the space program that CADCraft's layout generator plans with. \
The generator places open-plan workstation benches and these room kinds on a floor plate: \
meeting (table and chairs for `seats`), office (private office: desk, chair, up to two guests), \
phone_booth (one or two people), reception (counter and waiting sofa, near the entrance), \
pantry (kitchenette with tables for `seats`), collab (open collaboration/lounge area with tables for `seats`), \
storage (archive, IT, utility) and custom (anything else, sized by width/depth or area).

Map every need in the briefing to workstations or to a program line. Keep the briefing's own room names in `name`. \
Give sizes, zones and island sizes only when the briefing states them; otherwise leave them null so the generator \
uses its defaults and varies them between options. When the briefing gives a headcount rather than desks, derive \
the desk count (a stated sharing ratio applies; without one, one desk per person) and say so in `assumptions`. \
When it names a need without a number, choose a sensible count for the headcount and say so in `assumptions`. \
Don't add rooms the briefing doesn't ask for, except what is plainly implied (a reception when it mentions visitors). \
Write names and assumptions in the briefing's language and set `lang` to pt-BR for Portuguese, else en.

When the floor's usable area is given, estimate the program's area (about 7 m² per workstation with its share of \
circulation, rooms at their size plus 30%) and set `fits` accordingly; mention a shortfall in `assumptions`.";

pub const REVIEW_SYSTEM: &str = "\
You are a workplace strategist reviewing layout options that CADCraft generated for an office floor from a briefing. \
Each option lists its strategy (where rooms sit, bench direction, island size, central corridor) and its figures \
(workstations and rooms placed against the program, share of desks with daylight, usable area per person, score). \
Compare them against the briefing's priorities, recommend one, and give each option its concrete pros and cons in \
a few words each. Write in the briefing's language.";
