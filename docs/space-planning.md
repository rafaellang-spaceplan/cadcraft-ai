# Space planning: layout options from a briefing

CADCraft can turn an office floor plate and a briefing into several layout options at once:
open-plan workstation benches, meeting rooms, private offices, phone booths, reception, pantry,
collaboration areas and storage, each option with its own layers, a layout tab ready to plot and
a comparison table. Claude can read the briefing (free text or a PDF) into the space program and
review the options; the geometry itself is computed by CADCraft, deterministically.

AutoCAD has no equivalent; this is one of CADCraft's own features.

## The pieces

| Piece | What it does |
|---|---|
| `spaceplan` command (Tools ▸ Space Planning ▸ Generate Layout Options..., alias `SPL`) | Typed or from the menu: opens the Layout Options dialog. With JSON parameters (MCP, control channel, CLI, scripts): reads the floor, builds the options and draws them |
| `pdfimport` command (Insert ▸ PDF Import...) | Imports a vector PDF page (plotted from CAD) as polylines at a scale, keeping PDF layers and colours |
| `cadcraft-spaceplan` crate | The layout generator: geometry only, no AI |
| `cadcraft-ai` crate | Reads a briefing into a program and reviews options with the Claude API |
| `cadcraft-cli spaceplan` | The whole flow headless: base plan + briefing → result drawing + JSON summary |

## The floor plate

The base plan is the drawing open (DWG or DXF) or a PDF imported with `pdfimport`. The floor is:

- the selected closed object (closed polyline, circle…), or
- `point`: a point inside the floor; its boundary is traced as HATCH traces one (walls drawn as
  separate lines work), or
- by default the largest closed object that doesn't hold another a quarter of its size (so a
  sheet frame or title block around the plan is skipped).

Closed objects inside the floor are **holes**: 4 m² or more is a **core** (lifts, stairs,
toilets; a main corridor runs around it and "core" rooms gather next to it), smaller ones are
**columns**. `layers` limits all of this to some layers (e.g. the walls and structure layers of a
busy base plan). Units come from the drawing (INSUNITS), from `units`, or, in a unitless
drawing, from the floor's size.

## The program

```json
{
  "workstations": { "count": 60, "deskWidth": 1.4, "deskDepth": 0.7 },
  "spaces": [
    { "kind": "meeting", "count": 4, "seats": 6 },
    { "kind": "meeting", "name": "Board room", "count": 1, "seats": 12, "zone": "facade" },
    { "kind": "office", "count": 2 },
    { "kind": "phone_booth", "count": 3 },
    { "kind": "reception" },
    { "kind": "pantry", "seats": 10 }
  ],
  "corridor": 1.2,
  "mainCorridor": 1.5,
  "lang": "pt-BR"
}
```

Kinds: `meeting`, `office`, `phone_booth`, `reception`, `pantry`, `collab`, `storage`, `custom`.
Sizes (`width` × `depth` or `area`, metres) are optional: without them a room is sized from its
kind and seats. `zone` (`core`, `facade`, `entrance`) pins a room; without it the option's
strategy decides. `enclosed` overrides whether the room gets walls and a door (reception, pantry
and collaboration areas are open by default). Portuguese keys work too (`estacoes`, `ambientes`,
`tipo`, `quantidade`, `lugares`, `zona`…), and `lang: "pt-BR"` names rooms and layers in
Portuguese. Circulation defaults follow ABNT NBR 9050's 1.20 m accessible route.

## The options

The generator builds 48 candidates (by default) from every combination of:

- where rooms go: around the core, along the façade, grouped at one end or the other;
- benches lengthwise or crosswise, islands of 4, 6 or 8 (2 × 2, 2 × 3, 2 × 4 desks);
- with or without a central corridor;

scores each against the program (workstations 55, rooms 35, share of desks within 6 m of the
façade 10) and keeps the best ones while favouring different strategies, so ten options are ten
real alternatives. The same input and `seed` give the same options; another seed gives others.

Each option is drawn on four layers (`OPT01-WALLS`, `-FURNITURE`, `-TEXT`, `-ZONES`; in
Portuguese `OPC01-PAREDES`, `-MOBILIARIO`, `-TEXTO`, `-ZONAS`): 10 cm partitions with door
swings, furniture as blocks (`SP_DESK_120x70`, `SP_CHAIR_50x50`…, or your own: `blocks:
{"desk": "MY_DESK", "chair": "MY_CHAIR"}`, base point at the centre, front towards −y), and
labels with each room's area. Then:

- a **layout tab per option** whose viewport freezes the other options' layers, with the
  option's strategy and figures under it;
- a **layer state per option** (LAYERSTATE restore shows one option in model space);
- a **comparison tab** with a table of every option's figures.

A second run gets new layer and tab names (`OPCB01-…`) instead of mixing with the first. The
whole run is one undo step, and nothing is left behind when it fails.

## With Claude

Set `ANTHROPIC_API_KEY` (or `ANTHROPIC_AUTH_TOKEN`). The briefing goes to the Claude API with
the floor's measured area; the answer is a program in the format above, the assumptions made
(headcount to desks, unstated counts) and whether it fits. Review sends the options' figures
(not the drawing) and returns a recommendation with pros and cons per option.

- Model: `claude-opus-5-5` (override with `CADCRAFT_AI_MODEL`), effort `medium`
  (`CADCRAFT_AI_EFFORT`), structured outputs, server-side fallback on safety declines.
- The briefing and the floor's figures are sent to Anthropic; the drawing is not.
- The web build has no AI requests yet.

In the app: Tools ▸ Space Planning ▸ Generate Layout Options... → write or load the briefing →
**Read briefing with AI** → check and edit the program in the table → **Generate options** →
optionally **Review with AI**.

From the command line:

```sh
cadcraft-cli spaceplan planta.dwg --brief briefing.pdf --review -o opcoes.dwg
cadcraft-cli spaceplan planta.pdf --pdf-scale 1:100 --brief-text "60 estações, 4 salas de reunião para 6" -o opcoes.dxf
cadcraft-cli spaceplan planta.dwg --program programa.json --options 10 --seed 2
```

An agent connected over MCP can skip `cadcraft-ai` entirely: it reads the briefing itself and
calls `execute` with `spaceplan` and a program.

## Limits (today)

- Rooms and benches are rectangles aligned with the floor's longest edge; curved or angled
  façades are followed by the outline but rooms stay orthogonal.
- Every enclosed room gets one door on a side with clear circulation; egress paths and
  fire-code distances are not checked.
- Benches pack from one side of the free area; when the program leaves floor to spare, the
  spare floor is in one place rather than spread out.
- PDF import brings vector geometry only: no text (it is counted), no images, no underlays.
