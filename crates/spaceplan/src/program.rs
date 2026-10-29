//! The space program (briefing): what a layout must contain.
//!
//! Sizes are metres. Every number arrives from a person, a file or a language model, so
//! [`Program::validate`] clamps counts and refuses sizes outside what an office can hold.

use serde::{Deserialize, Serialize};

use crate::Error;

/// Most workstations one program may ask for.
pub const MAX_WORKSTATIONS: u32 = 5000;
/// Most rooms and zones (all counts added up) one program may ask for.
pub const MAX_SPACES: u32 = 500;
/// Smallest and largest room side, in metres.
pub const MIN_SIDE: f64 = 0.8;
pub const MAX_SIDE: f64 = 60.0;

/// The kinds of spaces an office program lists besides open-plan workstations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpaceKind {
    /// Meeting room with a table and chairs (`seats`).
    #[serde(alias = "reuniao", alias = "sala_reuniao", alias = "sala_de_reuniao", alias = "meeting_room", alias = "conference")]
    Meeting,
    /// Private (enclosed) office: desk, chair and guest chairs.
    #[serde(alias = "sala", alias = "sala_privativa", alias = "diretoria", alias = "private_office", alias = "gerencia")]
    Office,
    /// Phone booth / focus room for one or two people.
    #[serde(alias = "cabine", alias = "cabine_telefonica", alias = "phone_booth", alias = "booth", alias = "foco")]
    PhoneBooth,
    /// Reception desk and waiting area, near the entrance.
    #[serde(alias = "recepcao", alias = "lobby")]
    Reception,
    /// Pantry / kitchenette with a counter and tables.
    #[serde(alias = "copa", alias = "cafe", alias = "kitchen", alias = "cozinha")]
    Pantry,
    /// Open collaboration / lounge area.
    #[serde(alias = "colaborativo", alias = "lounge", alias = "descompressao", alias = "estar")]
    Collab,
    /// Storage, archive, IT or utility room.
    #[serde(alias = "deposito", alias = "arquivo", alias = "almoxarifado", alias = "ti", alias = "server", alias = "utility")]
    Storage,
    /// Anything else, sized by `width` × `depth` or `area`.
    #[serde(alias = "outro", alias = "other")]
    Custom,
}

impl SpaceKind {
    pub const ALL: [SpaceKind; 8] = [
        SpaceKind::Meeting,
        SpaceKind::Office,
        SpaceKind::PhoneBooth,
        SpaceKind::Reception,
        SpaceKind::Pantry,
        SpaceKind::Collab,
        SpaceKind::Storage,
        SpaceKind::Custom,
    ];

    /// The id used in JSON (`meeting`, `phone_booth`…).
    pub fn id(self) -> &'static str {
        match self {
            SpaceKind::Meeting => "meeting",
            SpaceKind::Office => "office",
            SpaceKind::PhoneBooth => "phone_booth",
            SpaceKind::Reception => "reception",
            SpaceKind::Pantry => "pantry",
            SpaceKind::Collab => "collab",
            SpaceKind::Storage => "storage",
            SpaceKind::Custom => "custom",
        }
    }

    /// The name a room of this kind gets when the program gives none.
    pub fn label(self, lang: Lang) -> &'static str {
        match (lang, self) {
            (Lang::En, SpaceKind::Meeting) => "Meeting room",
            (Lang::En, SpaceKind::Office) => "Office",
            (Lang::En, SpaceKind::PhoneBooth) => "Phone booth",
            (Lang::En, SpaceKind::Reception) => "Reception",
            (Lang::En, SpaceKind::Pantry) => "Pantry",
            (Lang::En, SpaceKind::Collab) => "Collaboration",
            (Lang::En, SpaceKind::Storage) => "Storage",
            (Lang::En, SpaceKind::Custom) => "Room",
            (Lang::PtBr, SpaceKind::Meeting) => "Sala de reunião",
            (Lang::PtBr, SpaceKind::Office) => "Sala privativa",
            (Lang::PtBr, SpaceKind::PhoneBooth) => "Cabine",
            (Lang::PtBr, SpaceKind::Reception) => "Recepção",
            (Lang::PtBr, SpaceKind::Pantry) => "Copa",
            (Lang::PtBr, SpaceKind::Collab) => "Área colaborativa",
            (Lang::PtBr, SpaceKind::Storage) => "Depósito",
            (Lang::PtBr, SpaceKind::Custom) => "Sala",
        }
    }

    /// Whether rooms of this kind get walls and a door unless the program says otherwise.
    pub fn enclosed_by_default(self) -> bool {
        !matches!(self, SpaceKind::Reception | SpaceKind::Collab | SpaceKind::Pantry)
    }

    /// Where rooms of this kind go unless the program or the option's strategy says otherwise.
    fn default_zone(self) -> Zone {
        match self {
            SpaceKind::Reception => Zone::Entrance,
            _ => Zone::Any,
        }
    }
}

/// The language of generated labels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Lang {
    #[default]
    #[serde(rename = "en", alias = "en-US", alias = "en-GB")]
    En,
    #[serde(rename = "pt-BR", alias = "pt", alias = "pt-br", alias = "pt_BR")]
    PtBr,
}

/// A placement preference for a room.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Zone {
    /// Wherever the option's strategy puts enclosed rooms.
    #[default]
    #[serde(alias = "qualquer")]
    Any,
    /// Next to the building core (lifts, stairs), away from the windows.
    #[serde(alias = "nucleo", alias = "interior")]
    Core,
    /// Along the façade (daylight).
    #[serde(alias = "fachada", alias = "perimeter", alias = "window", alias = "janela")]
    Facade,
    /// Near the entrance.
    #[serde(alias = "entrada", alias = "acesso")]
    Entrance,
}

/// One line of the program: `count` rooms of one kind.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceSpec {
    #[serde(alias = "tipo", alias = "type")]
    pub kind: SpaceKind,
    #[serde(default, alias = "nome", alias = "label")]
    pub name: Option<String>,
    #[serde(default = "one", alias = "quantidade", alias = "qty")]
    pub count: u32,
    /// People seated (meeting rooms, pantry, collaboration areas, offices with guests).
    #[serde(default, alias = "lugares", alias = "pessoas", alias = "capacity")]
    pub seats: Option<u32>,
    /// Room size in metres; both or neither.
    #[serde(default, alias = "largura")]
    pub width: Option<f64>,
    #[serde(default, alias = "profundidade", alias = "length", alias = "comprimento")]
    pub depth: Option<f64>,
    /// Room area in m² (used when `width`/`depth` aren't given).
    #[serde(default)]
    pub area: Option<f64>,
    /// Walls and a door (default: by kind; reception, pantry and collaboration areas are open).
    #[serde(default, alias = "fechada", alias = "enclosed_room")]
    pub enclosed: Option<bool>,
    #[serde(default, alias = "zona", alias = "localizacao")]
    pub zone: Option<Zone>,
}

fn one() -> u32 {
    1
}

impl SpaceSpec {
    pub fn new(kind: SpaceKind, count: u32) -> Self {
        SpaceSpec { kind, name: None, count, seats: None, width: None, depth: None, area: None, enclosed: None, zone: None }
    }

    pub fn display_name(&self, lang: Lang) -> String {
        self.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).map_or_else(|| self.kind.label(lang).to_string(), str::to_string)
    }

    pub fn is_enclosed(&self) -> bool {
        self.enclosed.unwrap_or_else(|| self.kind.enclosed_by_default())
    }

    pub fn zone(&self) -> Zone {
        self.zone.unwrap_or_else(|| self.kind.default_zone())
    }

    /// Seats, with the default for the kind.
    pub fn seat_count(&self) -> u32 {
        let d = match self.kind {
            SpaceKind::Meeting => 6,
            SpaceKind::Office => 3,
            SpaceKind::PhoneBooth => 1,
            SpaceKind::Reception => 3,
            SpaceKind::Pantry => 8,
            SpaceKind::Collab => 8,
            SpaceKind::Storage | SpaceKind::Custom => 0,
        };
        self.seats.unwrap_or(d).min(60)
    }

    /// The room's size in metres, long side first: the program's, else derived from the kind
    /// and the seats.
    pub fn size(&self) -> (f64, f64) {
        let (w, d) = match (self.width, self.depth, self.area) {
            (Some(w), Some(d), _) => (w, d),
            (_, _, Some(a)) if a > 0.0 => {
                // A 4:3 room of that area.
                let w = (a * 4.0 / 3.0).sqrt();
                (w, a / w)
            }
            _ => self.default_size(),
        };
        let (w, d) = (w.clamp(MIN_SIDE, MAX_SIDE), d.clamp(MIN_SIDE, MAX_SIDE));
        if w >= d { (w, d) } else { (d, w) }
    }

    fn default_size(&self) -> (f64, f64) {
        let n = f64::from(self.seat_count());
        match self.kind {
            SpaceKind::Meeting => {
                let (tl, tw) = meeting_table(self.seat_count());
                (tl + 2.0, tw + 2.2)
            }
            SpaceKind::Office => {
                if n > 4.0 {
                    (4.8, 3.6)
                } else {
                    (3.6, 3.0)
                }
            }
            SpaceKind::PhoneBooth => {
                if n > 1.0 {
                    (2.0, 1.6)
                } else {
                    (1.4, 1.2)
                }
            }
            SpaceKind::Reception => (4.5, 3.5),
            SpaceKind::Pantry => {
                // A counter plus a four-seat table per four people.
                let tables = (n / 4.0).ceil().max(1.0);
                (2.4 * tables + 1.2, 4.0)
            }
            SpaceKind::Collab => {
                let tables = (n / 4.0).ceil().max(1.0);
                let cols = tables.sqrt().ceil();
                let rows = (tables / cols).ceil();
                ((cols * 2.4 + 0.6).max(4.0), (rows * 2.4 + 0.6).max(3.0))
            }
            SpaceKind::Storage => (2.4, 2.0),
            SpaceKind::Custom => (3.0, 3.0),
        }
    }
}

/// A meeting table for `seats` people: (length, width) in metres.
pub fn meeting_table(seats: u32) -> (f64, f64) {
    let per_side = f64::from(seats.div_ceil(2).max(1));
    let width = if seats > 10 { 1.2 } else { 1.0 };
    ((per_side * 0.7).max(1.2), width)
}

/// The open-plan workstations.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Workstations {
    #[serde(alias = "quantidade", alias = "estacoes", alias = "qty")]
    pub count: u32,
    /// Desk width and depth in metres.
    #[serde(alias = "larguraMesa", alias = "desk_width")]
    pub desk_width: f64,
    #[serde(alias = "profundidadeMesa", alias = "desk_depth")]
    pub desk_depth: f64,
    /// Desks per side of a bench (the cluster is two rows back to back). `None`: varies between
    /// options (2, 3 or 4).
    #[serde(alias = "porLado", alias = "per_side")]
    pub per_side: Option<u32>,
}

impl Default for Workstations {
    fn default() -> Self {
        Workstations { count: 0, desk_width: 1.2, desk_depth: 0.7, per_side: None }
    }
}

/// The whole program: open-plan workstations, rooms and circulation widths.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Program {
    #[serde(alias = "estacoes", alias = "desks")]
    pub workstations: Workstations,
    #[serde(alias = "ambientes", alias = "rooms")]
    pub spaces: Vec<SpaceSpec>,
    /// Width of secondary circulation (in front of doors, between benches), metres.
    #[serde(alias = "circulacao")]
    pub corridor: f64,
    /// Width of the main circulation (around the core, along the spine), metres.
    #[serde(alias = "circulacaoPrincipal", alias = "main_corridor")]
    pub main_corridor: f64,
    /// Language of generated room names.
    #[serde(alias = "idioma")]
    pub lang: Lang,
}

impl Default for Program {
    fn default() -> Self {
        // 1.20 m is the accessible route width of ABNT NBR 9050; 1.50 m lets two wheelchairs pass.
        Program { workstations: Workstations::default(), spaces: Vec::new(), corridor: 1.2, main_corridor: 1.5, lang: Lang::En }
    }
}

impl Program {
    /// Check and clamp everything that came from outside.
    pub fn validate(&mut self) -> Result<(), Error> {
        let ws = &mut self.workstations;
        if ws.count > MAX_WORKSTATIONS {
            return Err(Error::Program(format!("at most {MAX_WORKSTATIONS} workstations")));
        }
        for (name, v) in [("desk width", ws.desk_width), ("desk depth", ws.desk_depth)] {
            if !v.is_finite() || !(0.4..=3.0).contains(&v) {
                return Err(Error::Program(format!("{name} must be between 0.4 and 3 m")));
            }
        }
        ws.per_side = ws.per_side.map(|n| n.clamp(1, 8));
        for (name, v) in [("corridor", self.corridor), ("main corridor", self.main_corridor)] {
            if !v.is_finite() || !(0.6..=6.0).contains(&v) {
                return Err(Error::Program(format!("{name} width must be between 0.6 and 6 m")));
            }
        }
        let mut total = 0u32;
        for s in &mut self.spaces {
            total = total.saturating_add(s.count);
            for v in [s.width, s.depth, s.area].into_iter().flatten() {
                if !v.is_finite() || v <= 0.0 {
                    return Err(Error::Program(format!("{}: sizes must be positive numbers", s.kind.id())));
                }
            }
            if s.width.is_some() != s.depth.is_some() {
                return Err(Error::Program(format!("{}: give both width and depth, or area", s.kind.id())));
            }
            if s.area.is_some_and(|a| a > MAX_SIDE * MAX_SIDE) {
                return Err(Error::Program(format!("{}: area is larger than any floor", s.kind.id())));
            }
            if let Some(n) = &mut s.name {
                // Names are drawn as text: keep them short and single-line.
                *n = n.chars().filter(|c| !c.is_control()).take(80).collect();
            }
        }
        if total > MAX_SPACES {
            return Err(Error::Program(format!("at most {MAX_SPACES} rooms")));
        }
        if self.workstations.count == 0 && total == 0 {
            return Err(Error::Program("the program is empty: give workstations or spaces".into()));
        }
        Ok(())
    }

    /// Rooms asked for, all counts added up.
    pub fn room_count(&self) -> u32 {
        self.spaces.iter().fold(0u32, |a, s| a.saturating_add(s.count))
    }
}
