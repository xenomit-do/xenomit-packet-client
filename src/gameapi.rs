//! Adapter exposing the packet session through `xenomit_game_api`'s
//! synchronous `GameClient` trait.
//!
//! `call_function` accepts these function names (`call.target` is
//! ignored; `call.arguments` are JSON):
//!
//! | function          | arguments                                  |
//! |-------------------|--------------------------------------------|
//! | `move_to`         | `[x, y]`                                   |
//! | `attack`          | `[targetId]` or `[targetId, x, y]`         |
//! | `attack_rocket`   | `[targetId]` or `[targetId, x, y]`         |
//! | `stop_attack`     | `[]`                                       |
//! | `select`          | `[targetId]`                               |
//! | `collect`         | `["hash"]`                                 |
//! | `harvest`         | `["hash"]`                                 |
//! | `jump`            | `[]`                                       |
//! | `keep_alive`      | `[]`                                       |
//! | `legacy`          | `["text"]`                                 |
//! | `send_packet`     | `[{id, fields:{name:value}}]`              |
//! | `pump`            | `[timeout_ms]` — drain inbound frames      |
//!
//! The adapter also implements the shared optional surface:
//! `feature_state` (pet/gate/item/economy observations from the packet
//! stream), `poll_events` (bounded replay journal), and `game_command`
//! (validated packet requests — see `command_packet`).

use crate::model::{CollectableKind, EntityKind, WorldState, gate_total_parts, gate_total_waves};
use crate::modules;
use crate::packet::{Packet, Value};
use crate::session::{Event, LoginInfo, Session};
use serde_json::{Map, Value as Json, json};
use std::net::TcpStream;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use xenomit_game_api::model::{
    ActionResult, Bounds, ClientStatus, EntityCategory, FunctionCall, GameClient, GameEntity,
    GameMapInfo, GameSnapshot, Inspection, Point, StageInfo,
};
use xenomit_game_api::{
    ApiError, ClientEventBatch, ClientFeatureState, EconomyState, GalaxyGateInfo, GameCommand,
    GateFeatureState, PetFeatureState, Result as ApiResult,
};

/// `GameClient` backed by a packet session. Thread-safe via `Mutex`;
/// all methods are blocking and may be called from a blocking worker.
pub struct PacketGameClient {
    session: Mutex<Session<TcpStream>>,
}

impl PacketGameClient {
    pub fn new(session: Session<TcpStream>) -> Self {
        Self {
            session: Mutex::new(session),
        }
    }

    /// Borrow the world state immutably.
    pub fn with_world<R>(&self, f: impl FnOnce(&crate::model::WorldState) -> R) -> R {
        f(&self.session.lock().unwrap().world)
    }

    fn api_err(e: impl std::fmt::Display) -> ApiError {
        ApiError::Transport(e.to_string())
    }

    fn arg_i64(call: &FunctionCall, idx: usize) -> ApiResult<i64> {
        call.arguments
            .get(idx)
            .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
            .ok_or_else(|| ApiError::InvalidCall(format!("argument {idx} must be a number")))
    }

    fn arg_str(call: &FunctionCall, idx: usize) -> ApiResult<&str> {
        call.arguments
            .get(idx)
            .and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::InvalidCall(format!("argument {idx} must be a string")))
    }

    fn send(&self, p: &Packet) -> ApiResult<()> {
        self.session.lock().unwrap().send(p).map_err(Self::api_err)
    }

    /// Drain any inbound frames currently buffered on the socket.
    pub fn pump(&self, timeout: Duration) -> ApiResult<usize> {
        self.pump_events(timeout).map(|events| events.len())
    }

    /// Drain decoded events as well as updating the world, for consumers
    /// recording game logs. No second socket reader is needed.
    pub fn pump_events(&self, timeout: Duration) -> ApiResult<Vec<Event>> {
        let mut s = self.session.lock().unwrap();
        let deadline = Instant::now() + timeout;
        let mut events = Vec::new();
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match s.poll(remaining).map_err(Self::api_err)? {
                Some(event) => events.push(event),
                None => break,
            }
        }
        Ok(events)
    }
}

impl GameClient for PacketGameClient {
    fn status(&self) -> ApiResult<ClientStatus> {
        let s = self.session.lock().unwrap();
        let mut details = Map::new();
        details.insert("phase".into(), json!(format!("{:?}", s.phase)));
        if let Some(h) = &s.world.hero {
            details.insert(
                "hero".into(),
                json!({
                    "id": h.user_id, "name": h.user_name, "map": h.map_id,
                    "x": h.x, "y": h.y, "hp": h.hitpoints, "shield": h.shield,
                }),
            );
        }
        Ok(ClientStatus {
            backend: "packet".into(),
            connected: true,
            ready: s.world.logged_in,
            details: details.into_iter().collect(),
        })
    }

    fn ping(&self) -> ApiResult<()> {
        // Best available round-trip: flush a KeepAlive heartbeat.
        self.send(&modules::keep_alive(false))
    }

    fn snapshot(&self, _target: Option<&str>) -> ApiResult<GameSnapshot> {
        let mut s = self.session.lock().unwrap();
        // Absorb whatever has arrived so the snapshot is current.
        let _ = s.pump();
        let w = &s.world;
        let observed_at_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let mut entities = Vec::with_capacity(w.entities.len() + w.collectables.len());
        for e in w.entities.values() {
            let mut fields = Map::new();
            fields.insert("npc".into(), json!(e.npc));
            fields.insert("factionId".into(), json!(e.faction_id));
            fields.insert("clanTag".into(), json!(e.clan_tag));
            fields.insert("typeId".into(), json!(e.type_id));
            fields.insert("destroyed".into(), json!(e.destroyed));
            if let Some(t) = e.move_target {
                fields.insert("moveTarget".into(), json!({"x": t.0, "y": t.1}));
            }
            entities.push(GameEntity {
                path: format!("entity/{}", e.id),
                class_name: match e.kind {
                    EntityKind::Ship => "Ship",
                    EntityKind::Asset => "Asset",
                    EntityKind::Other => "Entity",
                }
                .into(),
                name: e.name.clone(),
                category: if e.npc {
                    EntityCategory::Npc
                } else {
                    match e.kind {
                        EntityKind::Ship => EntityCategory::Player,
                        EntityKind::Asset => EntityCategory::Portal,
                        EntityKind::Other => EntityCategory::Unknown,
                    }
                },
                position: Point {
                    x: e.x as f64,
                    y: e.y as f64,
                },
                bounds: Bounds::default(),
                id: Some(json!(e.id)),
                health: e.hitpoints,
                max_health: e.hitpoints_max,
                shield: e.shield.map(|v| v as f64),
                max_shield: e.shield_max.map(|v| v as f64),
                fields,
            });
        }
        for c in w.collectables.values() {
            let mut fields = Map::new();
            fields.insert("kind".into(), json!(format!("{:?}", c.kind)));
            fields.insert("typeId".into(), json!(c.type_id));
            entities.push(GameEntity {
                path: format!("collectable/{}", c.hash),
                class_name: "Collectable".into(),
                name: c.hash.clone(),
                category: EntityCategory::Unknown,
                position: Point {
                    x: c.x as f64,
                    y: c.y as f64,
                },
                bounds: Bounds::default(),
                id: Some(json!(c.hash)),
                health: None,
                max_health: None,
                shield: None,
                max_shield: None,
                fields,
            });
        }

        let visited = entities.len();
        let map = w.hero.as_ref().map(|h| GameMapInfo {
            id: Some(h.map_id),
            name: Some(
                xenomit_game_api::star_map::map_name(h.map_id)
                    .unwrap_or_else(|| "Unknown map")
                    .to_owned(),
            ),
            width: None,
            height: None,
            player_position: Some(Point {
                x: h.x as f64,
                y: h.y as f64,
            }),
            safe_places: w
                .pois
                .values()
                .filter(|p| p.active)
                .filter_map(|p| {
                    let (x, y) = match p.coordinates.as_slice() {
                        [x, y, ..] => (*x as f64, *y as f64),
                        _ => return None,
                    };
                    Some(xenomit_game_api::model::SafePlace {
                        kind: "poi".into(),
                        name: p.poi_id.clone(),
                        x,
                        y,
                        radius: 400.0,
                    })
                })
                .collect(),
            in_safe_place: None,
        });

        Ok(GameSnapshot {
            navigation: Default::default(),
            observed_at_unix_ms,
            stage: StageInfo::default(),
            map,
            entities,
            visited,
            truncated: false,
        })
    }

    /// Packet-backed supplementary state. There is no display list, so
    /// `gui`/`event` stay unobserved; pet, gate, item, kill-screen and
    /// economy data come straight from the decoded packet stream.
    fn feature_state(&self) -> ApiResult<ClientFeatureState> {
        let mut s = self.session.lock().unwrap();
        let _ = s.pump();
        let w = &s.world;
        let observed_at_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        // The capabilities the packet backend actually implements —
        // GUI/overlay features have no packet-side equivalent and are
        // deliberately absent.
        let capabilities = [
            "portal.jump",
            "ore.refine",
            "ore.sell",
            "ore.observe",
            "pet.observe",
            "pet.activate",
            "pet.gear",
            "item.use",
            "config.select",
            "revive.free",
            "gate.observe",
            "gate.spin",
            "gate.place",
            "gate.life",
            "group.accept",
            "event.poll",
            "economy.observe",
            "booster.observe",
        ]
        .iter()
        .map(|c| c.to_string())
        .collect();

        let pet = w.pet.observed.then(|| {
            let vitals = w.pet.vitals_seen;
            PetFeatureState {
                id: w.pet.id,
                owned: Some(w.pet.owned),
                active: Some(w.pet.id.is_some()),
                destroyed: Some(w.pet.destroyed),
                position: w.pet.id.is_some().then(|| Point {
                    x: w.pet.x as f64,
                    y: w.pet.y as f64,
                }),
                health: vitals.then_some(w.pet.hitpoints),
                max_health: vitals.then_some(w.pet.hitpoints_max),
                shield: vitals.then_some(w.pet.shield),
                max_shield: vitals.then_some(w.pet.shield_max),
                fuel: vitals.then_some(w.pet.fuel),
                max_fuel: vitals.then_some(w.pet.fuel_max),
                level: (vitals || w.pet.id.is_some()).then_some(w.pet.level),
                gear_type: w.pet.gear_type,
                gears: Some(w.pet.gears.clone()),
                idle: w.pet.id.is_some().then_some(w.pet.idle),
                repair_available: None,
                data: json!({
                    "alive": w.pet.alive,
                    "hasFuel": w.pet.has_fuel,
                }),
            }
        });

        let gate = (w.gate_builder.is_some() || !w.gates.is_empty()).then(|| {
            let current = w.last_gate.and_then(|id| w.gates.get(&id));
            GateFeatureState {
                gate_id: w.last_gate,
                wave: current.map(|g| g.current_wave),
                total_waves: w.last_gate.map(gate_total_waves).filter(|t| *t > 0),
                lives: current.map(|g| g.lives),
                difficulty: None,
                completed: current.map(|g| g.finished),
                builder: w
                    .gates
                    .values()
                    .map(|g| GalaxyGateInfo {
                        id: g.id,
                        parts: g.parts,
                        total_parts: gate_total_parts(g.id),
                        lives: g.lives,
                        life_price: g.life_price,
                        current_wave: g.current_wave,
                        total_waves: gate_total_waves(g.id),
                        finished: g.finished,
                        multiplier: g.multiplier,
                    })
                    .collect(),
                data: w.gate_builder.as_ref().map_or_else(
                    || json!({}),
                    |b| {
                        json!({
                            "energy": b.energy,
                            "energyCost": b.energy_cost,
                            "spinAmountSelected": b.spin_amount,
                            "spinOnSale": b.spin_on_sale,
                            "galaxyGateDay": b.galaxy_gate_day,
                            "bonusRewardsDay": b.bonus_rewards_day,
                        })
                    },
                ),
            }
        });

        let economy = (w.hero.is_some() || !w.ores.is_empty() || w.trade.is_some()).then(|| {
            let h = w.hero.as_ref();
            EconomyState {
                ores: w.ores.clone(),
                cargo: h.map(|h| h.cargo_space as f64),
                max_cargo: h.map(|h| h.cargo_space_max as f64),
                credits: h.map(|h| h.credits),
                uridium: h.map(|h| h.uridium),
                trade_available: w.trade.as_ref().map(|t| t.area_type != 0),
                data: json!({
                    "configuration": w.configuration,
                    "tradeAreaType": w.trade.as_ref().map(|t| t.area_type),
                    "tradeCharges": w.trade.as_ref().map(|t| t.remaining_charges),
                }),
            }
        });

        Ok(ClientFeatureState {
            observed_at_unix_ms,
            capabilities,
            gui: None,
            pet,
            gate,
            event: None,
            economy,
            // Percentages are exact; the timer is the soonest-expiring
            // booster overall because `lootId → category` needs the item
            // database the packet client does not have.
            boosters: w.boosters.as_ref().map(|model| {
                let seconds_left = model.soonest_expiry();
                model
                    .categories
                    .iter()
                    .map(|(category, percent)| xenomit_game_api::BoosterState {
                        category: category.clone(),
                        percent: *percent,
                        seconds_left,
                    })
                    .collect()
            }),
            // Packet captures expose menu inventory and kill-screen offers,
            // but do not include client-side keyboard bindings or abilities.
            slot_bar: None,
            revive_options: w.kill_screen.as_ref().map(|screen| {
                screen
                    .options
                    .iter()
                    .map(|option| xenomit_game_api::ReviveOption {
                        repair_type: option.repair_type,
                        // Packet captures expose repair types and costs, but
                        // not the destination semantics for non-basic options.
                        // Only the protocol's known free basic repair is a base
                        // repair; other entries must not be guessed into one.
                        location: if option.repair_type == REPAIR_TYPE_BASIC {
                            xenomit_game_api::ReviveLocation::Base
                        } else {
                            xenomit_game_api::ReviveLocation::Unknown
                        },
                        visible: true,
                        free: option.cost == 0,
                        affordable: option.affordable,
                        cooldown_ms: Some(option.cooldown_ms.max(0) as u64),
                    })
                    .collect()
            }),
            repair_inventory: None,
            abilities: None,
            drones: None,
            group: None,
        })
    }

    /// Replay events recorded by the session pump. The journal is
    /// bounded; a cursor predating the retained tail reports
    /// `dropped`, and a cursor ahead of the session is an error.
    fn poll_events(&self, after_sequence: u64) -> ApiResult<ClientEventBatch> {
        let mut s = self.session.lock().unwrap();
        let _ = s.pump();
        s.journal.poll(after_sequence)
    }

    /// Typed manager commands over the packet channel. Every variant is
    /// validated against observed world state — GUI-only operations have
    /// no packet equivalent and fail closed with `Unavailable`.
    fn game_command(&self, command: &GameCommand) -> ApiResult<ActionResult> {
        let mut s = self.session.lock().unwrap();
        // Absorb pending frames so validation uses the freshest state.
        let _ = s.pump();
        let login = s.login.clone();
        let Some(packet) = command_packet(&s.world, login.as_ref(), command)? else {
            // Already satisfied (e.g. the pet is already active).
            return Ok(ActionResult::default());
        };
        s.send(&packet).map_err(Self::api_err)?;
        Ok(ActionResult::default())
    }

    fn inspect(&self, target: &str) -> ApiResult<Inspection> {
        // No reflection in a packet client; surface the entity's fields.
        let s = self.session.lock().unwrap();
        let mut insp = Inspection {
            name: target.into(),
            is_class: false,
            qualified: format!("packet::{target}"),
            ..Default::default()
        };
        if let Some(id_str) = target.strip_prefix("entity/") {
            if let Ok(eid) = id_str.parse::<i32>() {
                if let Some(e) = s.world.entities.get(&eid) {
                    insp.variables = [
                        ("name", e.name.clone()),
                        ("x", e.x.to_string()),
                        ("y", e.y.to_string()),
                        ("npc", e.npc.to_string()),
                    ]
                    .into_iter()
                    .map(|(n, _)| xenomit_game_api::model::MemberVariable {
                        name: n.into(),
                        owner: "Entity".into(),
                    })
                    .collect();
                }
            }
        }
        Ok(insp)
    }

    fn call_function(&self, call: &FunctionCall) -> ApiResult<ActionResult> {
        let fname = call.function.as_str();
        match fname {
            "move_to" => {
                let (x, y) = (
                    Self::arg_i64(call, 0)? as i32,
                    Self::arg_i64(call, 1)? as i32,
                );
                let pos = {
                    let s = self.session.lock().unwrap();
                    s.world.hero_pos().unwrap_or((0, 0))
                };
                self.send(&modules::move_request(pos, (x, y)))?;
                Ok(ActionResult {
                    data: Some(json!({"target": [x, y]})),
                    note: None,
                })
            }
            "attack" => {
                let tid = Self::arg_i64(call, 0)? as i32;
                let pos = {
                    let s = self.session.lock().unwrap();
                    s.world
                        .entities
                        .get(&tid)
                        .map(|e| (e.x, e.y))
                        .or(s.world.hero_pos())
                        .unwrap_or((0, 0))
                };
                self.send(&modules::attack_laser_request(tid, pos))?;
                Ok(ActionResult::default())
            }
            "attack_rocket" => {
                let tid = Self::arg_i64(call, 0)? as i32;
                let pos = {
                    let s = self.session.lock().unwrap();
                    s.world
                        .entities
                        .get(&tid)
                        .map(|e| (e.x, e.y))
                        .unwrap_or((0, 0))
                };
                self.send(&modules::attack_rocket_request(tid, pos))?;
                Ok(ActionResult::default())
            }
            "stop_attack" => {
                self.send(&modules::attack_abort_laser_request())?;
                Ok(ActionResult::default())
            }
            "select" => {
                let tid = Self::arg_i64(call, 0)? as i32;
                let (tpos, pos) = {
                    let s = self.session.lock().unwrap();
                    let t = s
                        .world
                        .entities
                        .get(&tid)
                        .map(|e| (e.x, e.y))
                        .unwrap_or((0, 0));
                    (t, s.world.hero_pos().unwrap_or((0, 0)))
                };
                self.send(&modules::ship_select_request(tid, tpos, pos, (0, 0), 0))?;
                Ok(ActionResult::default())
            }
            "collect" => {
                let hash = Self::arg_str(call, 0)?.to_owned();
                let (cpos, pos) = {
                    let s = self.session.lock().unwrap();
                    let c = s
                        .world
                        .collectables
                        .get(&hash)
                        .map(|c| (c.x, c.y))
                        .ok_or_else(|| ApiError::InvalidCall("unknown hash".into()))?;
                    (c, s.world.hero_pos().unwrap_or((0, 0)))
                };
                let kind = self.session.lock().unwrap().world.collectables[&hash].kind;
                match kind {
                    CollectableKind::Ore => self.send(&modules::harvest_request(&hash))?,
                    _ => self.send(&modules::collect_box_request(&hash, cpos, pos))?,
                }
                Ok(ActionResult::default())
            }
            "harvest" => {
                let hash = Self::arg_str(call, 0)?.to_owned();
                self.send(&modules::harvest_request(&hash))?;
                Ok(ActionResult::default())
            }
            "jump" => {
                self.send(&modules::jump_request())?;
                Ok(ActionResult::default())
            }
            "keep_alive" => {
                self.send(&modules::keep_alive(false))?;
                Ok(ActionResult::default())
            }
            "legacy" => {
                let text = Self::arg_str(call, 0)?.to_owned();
                self.send(&modules::legacy(&text))?;
                Ok(ActionResult::default())
            }
            "pump" => {
                let ms = call
                    .arguments
                    .first()
                    .and_then(|v| v.as_u64())
                    .unwrap_or(50);
                let n = self.pump(Duration::from_millis(ms))?;
                Ok(ActionResult {
                    data: Some(json!({"frames": n})),
                    note: None,
                })
            }
            "send_packet" => {
                // Escape hatch: `{id, fields:{name: value}}`. Field names
                // are bound from the module's write schema so the wire
                // layout is always schema-correct.
                let spec = call
                    .arguments
                    .first()
                    .ok_or_else(|| ApiError::InvalidCall("missing packet spec".into()))?;
                let id = spec["id"]
                    .as_i64()
                    .ok_or_else(|| ApiError::InvalidCall("packet.id required".into()))?
                    as i16;
                let schema = crate::registry::schema(id)
                    .ok_or_else(|| ApiError::InvalidCall(format!("unregistered module id {id}")))?;
                let given = spec["fields"].as_object();
                let mut fields = Vec::new();
                for op in schema.write {
                    let Some(name) = op.name() else { continue };
                    let Some(v) = given.and_then(|o| o.get(name)) else {
                        continue;
                    };
                    let val = match v {
                        Json::Number(n) => n
                            .as_i64()
                            .map(Value::Int)
                            .or_else(|| n.as_f64().map(Value::F64))
                            .unwrap_or(Value::Int(0)),
                        Json::String(s) => Value::Str(s.clone()),
                        Json::Bool(b) => Value::Bool(*b),
                        _ => continue,
                    };
                    fields.push((Some(name), val));
                }
                self.send(&Packet {
                    id,
                    name: Some(schema.name),
                    fields,
                })?;
                Ok(ActionResult::default())
            }
            _ => Err(ApiError::InvalidCall(format!("unknown function '{fname}'"))),
        }
    }
}

/// Ore ids the trade window sells — `GuiManager.updateTradeWindow`'s
/// list and the native backend agree on it (no XENOMIT).
fn sellable_ore(ore_type: i32) -> bool {
    matches!(ore_type, 0..=2 | 4..=8 | 28)
}

/// Gate ids the galaxy-gate builder offers (`validate_gate` parity).
fn valid_gate(gate_id: i32) -> bool {
    matches!(gate_id, 1..=8 | 12 | 19)
}

/// `KillScreenOptionTypeModule` value of the free basic repair.
const REPAIR_TYPE_BASIC: i32 = 1;

/// game.xml `<ore refiner="…">` recipes: producing one unit of the key
/// consumes each `(ore type, count)` ingredient pair — the same data
/// the refinery window's capacity combobox computes. Static
/// configuration, not per-session state.
const REFINE_RECIPES: &[(i32, &[(i32, f64)])] = &[
    (4, &[(0, 20.0), (1, 10.0)]),           // prometid
    (5, &[(1, 10.0), (2, 20.0)]),           // duranium
    (6, &[(4, 10.0), (5, 10.0), (3, 1.0)]), // promerium
];

fn unavailable(msg: impl Into<String>) -> ApiError {
    ApiError::Unavailable(msg.into())
}

fn invalid(msg: impl Into<String>) -> ApiError {
    ApiError::InvalidCall(msg.into())
}

/// Map one `GameCommand` to the wire packet it sends, validating
/// prerequisites against observed `WorldState` the way the stock
/// client gates its UI. `Ok(None)` means the request is already
/// satisfied (e.g. the pet is already in the wanted state) and nothing
/// needs sending. Pure and socket-free so it stays unit-testable.
fn command_packet(
    world: &WorldState,
    login: Option<&LoginInfo>,
    command: &GameCommand,
) -> ApiResult<Option<Packet>> {
    let packet = match command {
        // Stage/GUI interactions have no packet-side equivalent.
        GameCommand::SetOverlay { .. }
        | GameCommand::RemoveOverlay { .. }
        | GameCommand::ClickGui { .. }
        | GameCommand::CloseGui { .. }
        | GameCommand::SelectGateDifficulty { .. } => {
            return Err(unavailable(
                "GUI operations are unsupported by the packet backend",
            ));
        }
        GameCommand::JumpPortal { portal_id } => {
            let portal = world
                .portals
                .get(portal_id)
                .ok_or_else(|| unavailable("portal has not been observed on this map"))?;
            let (hx, hy) = world
                .hero_pos()
                .ok_or_else(|| unavailable("hero position is not observed"))?;
            let distance = ((portal.x - hx) as f64).hypot((portal.y - hy) as f64);
            if distance > 600.0 {
                return Err(invalid(format!(
                    "hero is too far from the portal ({distance:.0} > 600)"
                )));
            }
            modules::jump_request()
        }
        GameCommand::SetPet { enabled } => {
            // `Hero.ownsPet`/`isPetDestroyed` equivalents — only after
            // pet packets have been observed.
            let pet = &world.pet;
            if !pet.observed || !pet.owned || !pet.alive || pet.destroyed {
                return Err(unavailable("pet unavailable or destroyed"));
            }
            if pet.id.is_some() == *enabled {
                return Ok(None);
            }
            modules::set_pet_request(*enabled)
        }
        GameCommand::SelectPetGear { gear_type, target } => {
            let pet = &world.pet;
            if pet.id.is_none() {
                return Err(unavailable("pet is not active"));
            }
            if !pet.gears.contains(gear_type) {
                return Err(unavailable("pet gear is not equipped"));
            }
            modules::select_pet_gear_request(*gear_type, *target)
        }
        GameCommand::SelectConfiguration { configuration } => {
            if !(1..=2).contains(configuration) {
                return Err(invalid("ship configuration must be 1 or 2"));
            }
            if world.configuration == Some(i32::from(*configuration)) {
                return Ok(None);
            }
            // `ChangeConfingCommand` uses the legacy command channel —
            // `sendCommand("S", ["CFG", cfg, userID, sessionID])`, not
            // the registered `ConfigChangeRequest` module.
            let login = login.ok_or_else(|| unavailable("login credentials unavailable"))?;
            modules::legacy(&format!(
                "S|CFG|{configuration}|{}|{}",
                login.user_id, login.session_id
            ))
        }
        GameCommand::UseItem { item_id, activate } => {
            let item = world
                .menu_items
                .get(item_id.as_str())
                .ok_or_else(|| unavailable("item is not part of the observed menu"))?;
            if !item.visible || !item.available || item.blocked || (*activate && !item.activatable)
            {
                return Err(unavailable("item is not visible, blocked, or on cooldown"));
            }
            if !activate && item.selected {
                return Ok(None);
            }
            // `ItemsControlMenuProxy.sendActionRequest`, item-bar source.
            modules::menu_action_request(item_id, *activate)
        }
        GameCommand::Revive { repair_type } => {
            let kill_screen = world
                .kill_screen
                .as_ref()
                .ok_or_else(|| unavailable("hero is not destroyed"))?;
            if kill_screen.options.is_empty() {
                // No option list observed (post-login death): only the
                // implied free basic repair is honest.
                if *repair_type != REPAIR_TYPE_BASIC {
                    return Err(invalid(
                        "only the free basic repair is available without observed options",
                    ));
                }
            } else {
                let option = kill_screen
                    .options
                    .iter()
                    .find(|o| o.repair_type == *repair_type)
                    .ok_or_else(|| unavailable("repair option is not offered on this map"))?;
                // Automatic safety recovery only ever picks a free base
                // repair — never a paid or cooling option.
                if !matches!(option.repair_type, 1 | 6)
                    || option.cost != 0
                    || option.cooldown_ms > 0
                    || !option.affordable
                {
                    return Err(unavailable("revive requires an available free base repair"));
                }
            }
            let login = login.ok_or_else(|| unavailable("login credentials unavailable"))?;
            modules::repair_request(login, *repair_type)
        }
        GameCommand::SpinGalaxyGate {
            gate_id,
            spin_amount,
            use_multiplier,
        } => {
            if !valid_gate(*gate_id) {
                return Err(invalid("unknown galaxy gate id"));
            }
            if ![1, 5, 10, 100].contains(spin_amount) {
                return Err(invalid("galaxy spin amount must be 1, 5, 10, or 100"));
            }
            modules::spin_galaxy_gate(*gate_id, *spin_amount, *use_multiplier)
        }
        GameCommand::PlaceGalaxyGate { gate_id } => {
            if !valid_gate(*gate_id) {
                return Err(invalid("unknown galaxy gate id"));
            }
            modules::place_galaxy_gate(*gate_id)
        }
        GameCommand::BuyGalaxyLife { gate_id } => {
            if !valid_gate(*gate_id) {
                return Err(invalid("unknown galaxy gate id"));
            }
            modules::buy_galaxy_life(*gate_id)
        }
        GameCommand::AcceptGroupInvite { inviter_id } => {
            if *inviter_id <= 0 {
                return Err(invalid("group inviter id is missing"));
            }
            modules::invitation_accept_request(*inviter_id)
        }
        GameCommand::ReviveAt { repair_type, location } => {
            if *location != xenomit_game_api::ReviveLocation::Base {
                return Err(unavailable(
                    "packet backend only supports an observed free base repair",
                ));
            }
            // Reuse the established observed-option validation, including
            // affordability and cooldown checks, through the legacy variant.
            return command_packet(
                world,
                login,
                &GameCommand::Revive {
                    repair_type: *repair_type,
                },
            );
        }
        GameCommand::RepairDrones
        | GameCommand::InviteGroupMember { .. }
        | GameCommand::LeaveGroup
        | GameCommand::KickGroupMember { .. }
        | GameCommand::SetGroupInvites { .. } => {
            return Err(unavailable(
                "this manager operation is not implemented by the packet backend",
            ));
        }
        GameCommand::SellOre { ore_type, amount } => {
            if !sellable_ore(*ore_type) {
                return Err(invalid("ore type is not sellable through the trade window"));
            }
            if *amount == 0 {
                return Err(invalid("ore amount must be positive"));
            }
            let available = world.trade.as_ref().is_some_and(|t| t.area_type != 0);
            if !available {
                return Err(unavailable("ore trading is not available here"));
            }
            let inventory = *world.ores.get(ore_type).unwrap_or(&0.0);
            if (*amount as f64) > inventory {
                return Err(invalid(format!(
                    "requested sale exceeds observed ore inventory ({inventory})"
                )));
            }
            modules::sell_ore_request(*ore_type, *amount)
        }
        GameCommand::RefineOre { ore_type, amount } => {
            let Some((_, recipe)) = REFINE_RECIPES.iter().find(|(id, _)| id == ore_type) else {
                return Err(invalid("no refinement recipe for this ore type"));
            };
            if *amount == 0 {
                return Err(invalid("ore amount must be positive"));
            }
            if world.ores.is_empty() {
                return Err(unavailable("ore inventory has not been observed"));
            }
            // `fillComboBox` capacity: min over ingredients of
            // `have / required`.
            let capacity = recipe
                .iter()
                .map(|(ore, required)| {
                    let have = world.ores.get(ore).copied().unwrap_or(0.0);
                    (have / required).floor()
                })
                .fold(f64::INFINITY, f64::min);
            if (*amount as f64) > capacity {
                return Err(invalid(format!(
                    "requested refinement exceeds observed recipe capacity ({capacity})"
                )));
            }
            modules::refine_ore_request(*ore_type, *amount)
        }
    };
    Ok(Some(packet))
}

/// Helpers for the event side-channel: callers needing a live event
/// stream should use [`Session::poll`]/`pump` on their own handle.
impl From<Event> for Json {
    fn from(e: Event) -> Json {
        match e {
            Event::Packet(p) => json!({
                "kind": "packet",
                "id": p.id,
                "name": p.name,
            }),
            Event::Raw(r) => json!({"kind": "raw", "id": r.id, "len": r.body.len()}),
            Event::Malformed { error, .. } => {
                json!({"kind": "malformed", "error": error.to_string()})
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{HeroState, KillScreen, KillScreenOption, MenuItem, Portal};
    use crate::modules::id;
    use crate::packet::encode_body;

    fn login() -> LoginInfo {
        LoginInfo::new(42, "sid-1")
    }

    fn world_with_hero() -> WorldState {
        let mut w = WorldState::default();
        w.hero = Some(HeroState {
            user_id: 42,
            x: 1000,
            y: 1000,
            ..Default::default()
        });
        w
    }

    fn encode_ok(p: &Packet) {
        encode_body(p).expect("packet must encode against its schema");
    }

    #[test]
    fn jump_portal_requires_observed_portal_and_proximity() {
        let mut w = world_with_hero();
        let cmd = GameCommand::JumpPortal { portal_id: 7 };
        assert!(matches!(
            command_packet(&w, None, &cmd),
            Err(ApiError::Unavailable(_))
        ));
        w.portals.insert(
            7,
            Portal {
                id: 7,
                type_id: 1,
                x: 10_000,
                y: 10_000,
                faction_id: 0,
                on_minimap: true,
            },
        );
        assert!(matches!(
            command_packet(&w, None, &cmd),
            Err(ApiError::InvalidCall(_))
        ));
        w.portals.get_mut(&7).unwrap().x = 1200;
        w.portals.get_mut(&7).unwrap().y = 1000;
        let p = command_packet(&w, None, &cmd).unwrap().unwrap();
        assert_eq!(p.id, id::JUMP_REQUEST);
        encode_ok(&p);
    }

    #[test]
    fn pet_commands_validate_observed_state() {
        let mut w = WorldState::default();
        let on = GameCommand::SetPet { enabled: true };
        // Nothing observed → fail closed.
        assert!(matches!(
            command_packet(&w, None, &on),
            Err(ApiError::Unavailable(_))
        ));
        w.pet.observed = true;
        w.pet.owned = true;
        w.pet.alive = true;
        let p = command_packet(&w, None, &on).unwrap().unwrap();
        assert_eq!(
            (p.id, p.int("petRequestType").unwrap()),
            (id::PET_REQUEST, 0)
        );
        encode_ok(&p);
        // Already active → no packet needed.
        w.pet.id = Some(9);
        assert!(command_packet(&w, None, &on).unwrap().is_none());
        let off = GameCommand::SetPet { enabled: false };
        let p = command_packet(&w, None, &off).unwrap().unwrap();
        assert_eq!(p.int("petRequestType").unwrap(), 1);
        // Destroyed pets cannot be toggled.
        w.pet.id = None;
        w.pet.destroyed = true;
        assert!(matches!(
            command_packet(&w, None, &on),
            Err(ApiError::Unavailable(_))
        ));
    }

    #[test]
    fn pet_gear_requires_active_pet_and_equipped_gear() {
        let mut w = WorldState::default();
        let cmd = GameCommand::SelectPetGear {
            gear_type: 4,
            target: 99,
        };
        assert!(matches!(
            command_packet(&w, None, &cmd),
            Err(ApiError::Unavailable(_))
        ));
        w.pet.id = Some(9);
        assert!(matches!(
            command_packet(&w, None, &cmd),
            Err(ApiError::Unavailable(_))
        ));
        w.pet.gears = vec![4, 5];
        let p = command_packet(&w, None, &cmd).unwrap().unwrap();
        assert_eq!(p.id, id::PET_GEAR_ACTIVATION_REQUEST);
        assert_eq!(p.int("optParam").unwrap(), 99);
        assert_eq!(
            p.module("gearTypeToActivate")
                .unwrap()
                .int("typeValue")
                .unwrap(),
            4
        );
        encode_ok(&p);
    }

    #[test]
    fn configuration_uses_legacy_command_with_credentials() {
        let mut w = world_with_hero();
        let bad = GameCommand::SelectConfiguration { configuration: 3 };
        assert!(matches!(
            command_packet(&w, None, &bad),
            Err(ApiError::InvalidCall(_))
        ));
        let cmd = GameCommand::SelectConfiguration { configuration: 2 };
        // Without credentials the legacy command cannot be built.
        assert!(matches!(
            command_packet(&w, None, &cmd),
            Err(ApiError::Unavailable(_))
        ));
        let p = command_packet(&w, Some(&login()), &cmd).unwrap().unwrap();
        assert_eq!(p.id, id::LEGACY);
        assert_eq!(p.string("message").unwrap(), "S|CFG|2|42|sid-1");
        encode_ok(&p);
        // Already selected → nothing to send.
        w.configuration = Some(2);
        assert!(command_packet(&w, Some(&login()), &cmd).unwrap().is_none());
    }

    #[test]
    fn use_item_requires_observed_activatable_item() {
        let mut w = WorldState::default();
        let cmd = GameCommand::UseItem {
            item_id: "ammunition_laser_ucb-100".into(),
            activate: true,
        };
        assert!(matches!(
            command_packet(&w, None, &cmd),
            Err(ApiError::Unavailable(_))
        ));
        w.menu_items.insert(
            "ammunition_laser_ucb-100".into(),
            MenuItem {
                id: "ammunition_laser_ucb-100".into(),
                visible: true,
                available: true,
                activatable: true,
                ..Default::default()
            },
        );
        let p = command_packet(&w, None, &cmd).unwrap().unwrap();
        assert_eq!(p.id, id::MENU_ACTION_REQUEST);
        assert_eq!(p.int("actionType").unwrap(), 1);
        assert_eq!(p.int("sourceType").unwrap(), 0);
        assert_eq!(p.string("menuItemId").unwrap(), "ammunition_laser_ucb-100");
        encode_ok(&p);
        // Blocked items fail closed.
        w.menu_items
            .get_mut("ammunition_laser_ucb-100")
            .unwrap()
            .blocked = true;
        assert!(matches!(
            command_packet(&w, None, &cmd),
            Err(ApiError::Unavailable(_))
        ));
    }

    #[test]
    fn revive_is_free_only_and_needs_kill_screen() {
        let mut w = world_with_hero();
        let cmd = GameCommand::Revive { repair_type: 1 };
        assert!(matches!(
            command_packet(&w, None, &cmd),
            Err(ApiError::Unavailable(_))
        ));
        w.kill_screen = Some(KillScreen {
            killer_name: "npc".into(),
            options: Vec::new(),
        });
        // Empty option list: only the basic free repair is allowed, and
        // it needs the login credentials for the embedded LoginRequest.
        assert!(matches!(
            command_packet(&w, None, &cmd),
            Err(ApiError::Unavailable(_))
        ));
        let paid = GameCommand::Revive { repair_type: 3 };
        assert!(matches!(
            command_packet(&w, Some(&login()), &paid),
            Err(ApiError::InvalidCall(_))
        ));
        let p = command_packet(&w, Some(&login()), &cmd).unwrap().unwrap();
        assert_eq!(p.id, id::KILL_SCREEN_REPAIR_REQUEST);
        let selection = p.module("selection").unwrap();
        assert_eq!(selection.id, id::KILL_SCREEN_OPTION_TYPE_MODULE);
        assert_eq!(selection.int("repairTypeValue").unwrap(), 1);
        let request = p.module("requestModule").unwrap();
        assert_eq!(request.id, id::LOGIN_REQUEST);
        assert_eq!(request.int("userID").unwrap(), 42);
        assert_eq!(request.string("sessionID").unwrap(), "sid-1");
        encode_ok(&p);
        // An offered paid option is never picked automatically.
        w.kill_screen = Some(KillScreen {
            killer_name: String::new(),
            options: vec![KillScreenOption {
                repair_type: 1,
                cost: 100,
                currency: 0,
                affordable: true,
                cooldown_ms: 0,
            }],
        });
        assert!(matches!(
            command_packet(&w, Some(&login()), &cmd),
            Err(ApiError::Unavailable(_))
        ));
        // Unoffered option → unavailable too.
        w.kill_screen = Some(KillScreen {
            killer_name: String::new(),
            options: vec![KillScreenOption {
                repair_type: 2,
                cost: 0,
                currency: 0,
                affordable: true,
                cooldown_ms: 0,
            }],
        });
        assert!(matches!(
            command_packet(&w, Some(&login()), &cmd),
            Err(ApiError::Unavailable(_))
        ));
    }

    #[test]
    fn sell_ore_validates_type_trade_and_inventory() {
        let mut w = WorldState::default();
        let bad = GameCommand::SellOre {
            ore_type: 3, // xenomit is never sold
            amount: 10,
        };
        assert!(matches!(
            command_packet(&w, None, &bad),
            Err(ApiError::InvalidCall(_))
        ));
        let cmd = GameCommand::SellOre {
            ore_type: 0,
            amount: 10,
        };
        // No trade observation → fail closed even with inventory.
        w.ores.insert(0, 100.0);
        assert!(matches!(
            command_packet(&w, None, &cmd),
            Err(ApiError::Unavailable(_))
        ));
        w.trade = Some(crate::model::TradeInfo {
            area_type: 0,
            remaining_charges: 0,
            prices: Default::default(),
        });
        // TRADETYPE_INACTIVE still means no trading.
        assert!(matches!(
            command_packet(&w, None, &cmd),
            Err(ApiError::Unavailable(_))
        ));
        w.trade.as_mut().unwrap().area_type = 1;
        let p = command_packet(&w, None, &cmd).unwrap().unwrap();
        assert_eq!(p.id, id::TRADE_SELL_ORE_REQUEST);
        let count = p.module("toSell").unwrap();
        assert_eq!(
            count.module("oreType").unwrap().int("typeValue").unwrap(),
            0
        );
        assert_eq!(count.f64("count").unwrap(), 10.0);
        encode_ok(&p);
        // Over inventory → invalid.
        let too_much = GameCommand::SellOre {
            ore_type: 0,
            amount: 10_000,
        };
        assert!(matches!(
            command_packet(&w, None, &too_much),
            Err(ApiError::InvalidCall(_))
        ));
    }

    #[test]
    fn refine_ore_uses_recipe_capacity() {
        let mut w = WorldState::default();
        let bad = GameCommand::RefineOre {
            ore_type: 7, // seprom has no lab recipe
            amount: 1,
        };
        assert!(matches!(
            command_packet(&w, None, &bad),
            Err(ApiError::InvalidCall(_))
        ));
        let cmd = GameCommand::RefineOre {
            ore_type: 4,
            amount: 2,
        };
        assert!(matches!(
            command_packet(&w, None, &cmd),
            Err(ApiError::Unavailable(_))
        ));
        // 2 prometid needs 40 prometium + 20 endurium.
        w.ores.insert(0, 40.0);
        w.ores.insert(1, 20.0);
        let p = command_packet(&w, None, &cmd).unwrap().unwrap();
        assert_eq!(p.id, id::LAB_REFINEMENT_REQUEST);
        assert_eq!(
            p.module("toProduce")
                .unwrap()
                .module("oreType")
                .unwrap()
                .int("typeValue")
                .unwrap(),
            4
        );
        encode_ok(&p);
        // One ingredient short → capacity error.
        w.ores.insert(1, 19.0);
        assert!(matches!(
            command_packet(&w, None, &cmd),
            Err(ApiError::InvalidCall(_))
        ));
    }

    #[test]
    fn galaxy_gate_commands_validate_ids() {
        let w = WorldState::default();
        let spin = GameCommand::SpinGalaxyGate {
            gate_id: 1,
            spin_amount: 3,
            use_multiplier: false,
        };
        assert!(matches!(
            command_packet(&w, None, &spin),
            Err(ApiError::InvalidCall(_))
        ));
        let spin = GameCommand::SpinGalaxyGate {
            gate_id: 17,
            spin_amount: 5,
            use_multiplier: false,
        };
        assert!(matches!(
            command_packet(&w, None, &spin),
            Err(ApiError::InvalidCall(_))
        ));
        let spin = GameCommand::SpinGalaxyGate {
            gate_id: 19,
            spin_amount: 5,
            use_multiplier: true,
        };
        let p = command_packet(&w, None, &spin).unwrap().unwrap();
        assert_eq!(p.id, id::GALAXY_GATE_MULTI_ENERGY_REQUEST);
        assert_eq!(p.int("spinAmount").unwrap(), 5);
        assert_eq!(p.int("gateId").unwrap(), 19);
        assert_eq!(p.boolean("useMultiplier").unwrap(), true);
        encode_ok(&p);

        let place = GameCommand::PlaceGalaxyGate { gate_id: 2 };
        let p = command_packet(&w, None, &place).unwrap().unwrap();
        assert_eq!(p.id, id::GALAXY_GATE_SETUP_GATE_REQUEST);
        encode_ok(&p);

        let life = GameCommand::BuyGalaxyLife { gate_id: 8 };
        let p = command_packet(&w, None, &life).unwrap().unwrap();
        assert_eq!(p.id, id::GALAXY_GATE_BUY_LIFE_REQUEST);
        encode_ok(&p);
    }

    #[test]
    fn group_invite_requires_positive_inviter() {
        let w = WorldState::default();
        let bad = GameCommand::AcceptGroupInvite { inviter_id: 0 };
        assert!(matches!(
            command_packet(&w, None, &bad),
            Err(ApiError::InvalidCall(_))
        ));
        let cmd = GameCommand::AcceptGroupInvite { inviter_id: 77 };
        let p = command_packet(&w, None, &cmd).unwrap().unwrap();
        assert_eq!(p.id, id::INVITATION_ACCEPT_REQUEST);
        assert_eq!(p.int("inviterId").unwrap(), 77);
        encode_ok(&p);
    }

    #[test]
    fn gui_commands_are_unavailable() {
        let w = WorldState::default();
        for cmd in [
            GameCommand::SetOverlay {
                id: "o".into(),
                text: "t".into(),
                x: 0.0,
                y: 0.0,
                color: 0,
            },
            GameCommand::RemoveOverlay { id: "o".into() },
            GameCommand::ClickGui { name: "g".into() },
            GameCommand::CloseGui { name: "g".into() },
            GameCommand::SelectGateDifficulty {
                gui: "g".into(),
                difficulty: 1,
            },
        ] {
            assert!(matches!(
                command_packet(&w, None, &cmd),
                Err(ApiError::Unavailable(_))
            ));
        }
    }
}
