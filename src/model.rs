//! Live world model maintained by applying decoded packets.
//!
//! The state is intentionally lossy: it tracks what a bot needs (hero,
//! entities, collectables, combat links, vitals) and ignores purely
//! cosmetic traffic. Unknown or unhandled modules are still delivered
//! through the event stream for callers that care.

use crate::modules::id;
use crate::packet::{Packet, Value};
use std::collections::{BTreeMap, HashMap};

/// Legacy reward logs carry both the signed change and authoritative total.
/// `0|y|CRE|delta|total` and `0|LM|style|CRE|delta|total`
/// are normalized by the stock BoxCollectResponseAssembly.
pub fn asset_log(message: &str) -> Option<(usize, f64, f64)> {
    let fields: Vec<_> = message.split('|').collect();
    let offset = match fields.get(1).copied()? {
        "y" => 2,
        "LM" => 3,
        _ => return None,
    };
    let index = match *fields.get(offset)? {
        "CRE" => 0,
        "URI" => 1,
        "HON" => 2,
        "EP" => 3,
        _ => return None,
    };
    let delta: f64 = fields.get(offset + 1)?.parse().ok()?;
    let total: f64 = fields.get(offset + 2)?.parse().ok()?;
    (delta.is_finite() && total.is_finite()).then_some((index, delta, total))
}

/// Hero (own player) state, populated from `ShipInitializationCommand`
/// and kept current by attribute-update commands.
#[derive(Clone, Debug, Default)]
pub struct HeroState {
    pub user_id: i32,
    pub user_name: String,
    pub type_id: String,
    pub clan_tag: String,
    pub x: i32,
    pub y: i32,
    pub map_id: i32,
    pub faction_id: i32,
    pub clan_id: i32,
    pub level: i32,
    pub speed: i32,
    pub hitpoints: f64,
    pub hitpoints_max: f64,
    pub nano_hull: i32,
    pub nano_hull_max: i32,
    pub shield: i32,
    pub shield_max: i32,
    pub cargo_space: i32,
    pub cargo_space_max: i32,
    pub credits: f64,
    pub uridium: f64,
    pub jackpot: f32,
    pub ep: f64,
    pub honour: f64,
    pub premium: bool,
    pub cloaked: bool,
    pub target_id: Option<i32>,
}

/// Another entity on the map (player, NPC, station asset).
#[derive(Clone, Debug, Default)]
pub struct Entity {
    pub id: i32,
    pub kind: EntityKind,
    pub name: String,
    pub type_id: String,
    pub clan_tag: String,
    pub x: i32,
    pub y: i32,
    pub faction_id: i32,
    pub clan_id: i32,
    pub npc: bool,
    pub cloaked: bool,
    pub expansion_stage: i32,
    /// Active movement: destination and travel time in ms.
    pub move_target: Option<(i32, i32)>,
    pub move_time_ms: i32,
    pub hitpoints: Option<f64>,
    pub hitpoints_max: Option<f64>,
    pub shield: Option<i32>,
    pub shield_max: Option<i32>,
    pub destroyed: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EntityKind {
    #[default]
    Ship,
    Asset,
    Other,
}

/// A collectable map object keyed by its server hash.
#[derive(Clone, Debug)]
pub struct Collectable {
    pub hash: String,
    pub kind: CollectableKind,
    pub x: i32,
    pub y: i32,
    pub type_id: i32,
    /// `AddBoxCommand.boxType` (e.g. `BONUS_BOX`, `FROM_SHIP`) — the
    /// collectable asset id is `"box_" + type_str`. Empty for non-boxes.
    pub type_str: String,
    /// `AddMineCommand.maxSkill1` — `AddMineHandler` maps it to
    /// `Mine.pulseColorId` (`maxSkill1 ? 1 : 0`); 0 = no pulse overlay.
    pub pulse_color_id: i32,
    /// `AddFireworkBoxCommand.colourCode` — `FireworkBox` colour index.
    pub colour_code: i32,
    /// `AddFireworkBoxCommand.eventModifier` — `fireworkPatterns` class key
    /// (`"SolarClash"`; `""` resolves to `"standart"`).
    pub event_modifier: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CollectableKind {
    Box,
    Ore,
    Beacon,
    Mine,
    FireworkBox,
}

/// A point of interest from `MapAddPOICommand` (portals, bases).
#[derive(Clone, Debug)]
pub struct Poi {
    pub poi_id: String,
    pub poi_type: Option<i16>,
    pub active: bool,
    pub coordinates: Vec<i32>,
}

/// Map-context flags from `BeaconCommand`.
#[derive(Clone, Copy, Debug, Default)]
pub struct MapFlags {
    pub in_radiation_zone: bool,
    pub in_station: bool,
    pub near_jump_gate: bool,
    pub repair_robot_active: bool,
}

/// A jumpgate on the current map (`JumpgateCreateCommand`). `type_id`
/// is the `designId` — the bare numeric `LIB_PORTALS` asset key.
#[derive(Clone, Debug)]
pub struct Portal {
    pub id: i32,
    pub type_id: i32,
    pub x: i32,
    pub y: i32,
    pub faction_id: i32,
    pub on_minimap: bool,
}

/// Session-scope state of the hero's own P.E.T., populated from the
/// `Pet*` command stream. Kept minimal: enough for pet feature
/// reporting and `SetPet`/`SelectPetGear` validation.
#[derive(Clone, Debug, Default)]
pub struct PetState {
    /// Any pet-related module has been observed at all.
    pub observed: bool,
    /// `PetInitializationCommand.hasPet`.
    pub owned: bool,
    /// `PetInitializationCommand.petIsAlive`.
    pub alive: bool,
    /// `PetInitializationCommand.hasFuel`.
    pub has_fuel: bool,
    /// Summoned pet's entity id — `Some` while the pet is on the map.
    pub id: Option<i32>,
    /// Last observed map position (from the activation commands).
    pub x: i32,
    pub y: i32,
    /// `petId` of the last `PetStatusCommand` — status can arrive
    /// before the matching activation; the vitals apply either way.
    pub status_id: Option<i32>,
    /// Any vitals-bearing packet (`PetStatusCommand` or one of the
    /// hitpoint/shield/fuel updates) has been observed — gates the
    /// reported vitals so unobserved values stay unknown.
    pub vitals_seen: bool,
    /// `PetIsDestroyedCommand` observed.
    pub destroyed: bool,
    /// `PetIdleModeCommand.isIdle` (pet parked).
    pub idle: bool,
    pub level: i32,
    pub hitpoints: f64,
    pub hitpoints_max: f64,
    pub shield: f64,
    pub shield_max: f64,
    pub fuel: f64,
    pub fuel_max: f64,
    /// Equipped gears (`PetGearAddCommand`/`PetGearRemoveCommand`), as
    /// `PetGearTypeModule.typeValue`s.
    pub gears: Vec<i32>,
    /// Currently activated gear (`PetGearActivationChangeCommand`).
    pub gear_type: Option<i32>,
}

/// One item of the session menu (`MenuItemInitializationCommand` plus
/// standalone `MenuItemStatusCommand` updates).
#[derive(Clone, Debug, Default)]
pub struct MenuItem {
    /// `menuItemId` (e.g. `"ammunition_laser_ucb-100"`).
    pub id: String,
    /// Owning `categoryId` ("" when only a status update was seen).
    pub category: String,
    /// `actionStyle`: 0 one-shot, 1 toggle, 2 selection, 3 timer.
    pub action_style: i32,
    pub selected: bool,
    pub available: bool,
    pub blocked: bool,
    pub activatable: bool,
    pub visible: bool,
    pub counter_value: f64,
    pub max_counter_value: f64,
}

/// One repair option offered by `KillScreenPostCommand`/
/// `KillScreenUpdateCommand` (`KillScreenOptionModule`).
#[derive(Clone, Debug)]
pub struct KillScreenOption {
    /// `repairType.repairTypeValue` (`OPTIONTYPE_*` — 1 = basic free
    /// repair, 6 = basic full repair; others carry a price/cooldown).
    pub repair_type: i32,
    /// `price.amount` (0 = free).
    pub cost: i64,
    /// `price.type` (`PriceModule.type`).
    pub currency: i32,
    /// `affordableForPlayer`.
    pub affordable: bool,
    /// `cooldownTime` (ms) — option disabled while it ticks down.
    pub cooldown_ms: i64,
}

/// Kill-screen state while the hero is dead: opened by
/// `ShipDestroyedCommand` for the hero or `KillScreenPostCommand`,
/// updated by `KillScreenUpdateCommand`, cleared on map change.
#[derive(Clone, Debug, Default)]
pub struct KillScreen {
    /// `killerName` from the post command.
    pub killer_name: String,
    /// Offered repair options — empty until the server sends the
    /// list; the free basic repair (type 1) is implied then (same as
    /// the post-login `status 7` flow, which sends no post command).
    pub options: Vec<KillScreenOption>,
}

/// One galaxy gate's builder/run state (`GalaxyGateBuilderGateModule`).
#[derive(Clone, Debug)]
pub struct GalaxyGateState {
    pub id: i32,
    /// Parts collected so far (`currPart`).
    pub parts: i32,
    /// `livesLeft` (-1 = not deployed yet).
    pub lives: i32,
    /// `lifePrice` (-1 = not deployed yet).
    pub life_price: i32,
    /// `currWave` — wave progress while the gate is being played.
    pub current_wave: i32,
    /// `state == GATESTATE_FINISHED` (1), or parts-complete per
    /// `GalaxyGateBuilderGateCompleteCommand`.
    pub finished: bool,
    pub multiplier: i32,
    /// `mode` string ("alpha", "kronos", …).
    pub mode: String,
}

/// Last `OreTradeInfoCommand` — the trade-area type plus the ore
/// prices the trade window shows (`OreTradeInfoHandler`).
#[derive(Clone, Debug)]
pub struct TradeInfo {
    /// `type` — `TRADETYPE_*` (0 inactive, 1 space station, 2 drone,
    /// 3 pet, 4 battle station, 5 pirate station, 6 group gate station).
    pub area_type: i32,
    /// `remainingCharges` — trade-drone charges left (-1 = none).
    pub remaining_charges: i64,
    /// `ore typeValue → price` for the nine trade-window ores.
    pub prices: BTreeMap<i32, i64>,
}

/// Account-level gate builder data (`GalaxyGateBuilderInfoCommand`
/// outside the per-gate list).
#[derive(Clone, Debug)]
pub struct GateBuilderInfo {
    /// `money` — the gate energy/balance shown by the builder window.
    pub energy: f64,
    /// `energy_cost` per spin.
    pub energy_cost: i64,
    /// `spinamount_selected`.
    pub spin_amount: i64,
    pub spin_on_sale: bool,
    pub galaxy_gate_day: bool,
    pub bonus_rewards_day: bool,
}

/// Total part count per gate id — `getGateMaxPartsCount` in
/// `GalaxyGateBuilderModel.as` (0 when the id is unknown).
pub fn gate_total_parts(gate_id: i32) -> i32 {
    match gate_id {
        1 => 34,
        2 => 48,
        3 => 82,
        4 => 128,
        5 => 99,
        6 => 111,
        7 => 120,
        8 => 45,
        12 => 21,
        13 => 45,
        19 => 100,
        _ => 0,
    }
}

/// Total wave count per gate id — `getGateTotalWaves` in
/// `GalaxyGateBuilderModel.as` (0 when the id is unknown).
pub fn gate_total_waves(gate_id: i32) -> i32 {
    match gate_id {
        1..=3 => 40,
        4 => 29,
        5 => 30,
        6 => 46,
        7 => 29,
        8 => 21,
        12 => 50,
        13 => 12,
        19 => 54,
        _ => 0,
    }
}

/// Everything the packet client knows about the world.
#[derive(Default)]
pub struct WorldState {
    pub hero: Option<HeroState>,
    pub entities: HashMap<i32, Entity>,
    pub collectables: HashMap<String, Collectable>,
    pub pois: HashMap<String, Poi>,
    /// Attacker → victim laser links currently running.
    pub laser_links: HashMap<i32, i32>,
    /// Uids that have an active laser (attackers).
    pub laser_active: std::collections::HashSet<i32>,
    pub map_flags: MapFlags,
    pub logged_in: bool,
    /// Last `LegacyModule` text messages (server console output).
    pub legacy_log: Vec<String>,
    /// Jumpgates on the current map (`JumpgateCreateCommand`), keyed by
    /// `gateId`.
    pub portals: HashMap<i32, Portal>,
    /// Hero's P.E.T. (`Pet*` commands).
    pub pet: PetState,
    /// Session item menu (`MenuInitializationCommand` + status
    /// updates), keyed by `menuItemId`. Session scope — kept across
    /// map changes like the stock `ItemsControlMenuProxy`.
    pub menu_items: HashMap<String, MenuItem>,
    /// Kill-screen repair options while the hero is dead.
    pub kill_screen: Option<KillScreen>,
    /// Galaxy gates from `GalaxyGateBuilderInfoCommand`, keyed by id.
    pub gates: BTreeMap<i32, GalaxyGateState>,
    /// Gate-builder globals (`GalaxyGateBuilderInfoCommand`).
    pub gate_builder: Option<GateBuilderInfo>,
    /// Gate receiving the last wave/life/setup update — the gate
    /// currently in play for feature reporting.
    pub last_gate: Option<i32>,
    /// Ore inventory by `OreTypeModule.typeValue`
    /// (`AttributeOreCountUpdateCommand`).
    pub ores: BTreeMap<i32, f64>,
    /// Selected ship configuration (`0|S|CFG|n` legacy echo).
    pub configuration: Option<i32>,
    /// Last `OreTradeInfoCommand` — trade availability (`area_type !=
    /// 0`) plus the observed ore prices. `None` until the server
    /// pushes it.
    pub trade: Option<TradeInfo>,
    /// Booster model rebuilt from `AttributeBoosterUpdateCommand`. `None`
    /// until the first update arrives.
    pub boosters: Option<BoosterModel>,
}

/// The packet-side view of the game's `BoosterModel`.
///
/// The server sends the summed percentage per attribute category, and the
/// remaining seconds of each timed booster keyed by `lootId`. The client
/// resolves a `lootId` to its categories through the item database, which
/// the packet client does not carry, so timers cannot be attributed to a
/// single category here.
#[derive(Clone, Debug, Default)]
pub struct BoosterModel {
    /// `attributeType → totalPercent`.
    pub categories: BTreeMap<String, f64>,
    /// `lootId → (secondsLeft when received, when it was received)`.
    pub timers: HashMap<String, (f64, std::time::Instant)>,
}

impl BoosterModel {
    /// Seconds left on the soonest-expiring timed booster, if any.
    pub fn soonest_expiry(&self) -> Option<f64> {
        self.timers
            .values()
            .map(|(seconds, at)| seconds - at.elapsed().as_secs_f64())
            .filter(|left| *left > 0.0)
            .min_by(f64::total_cmp)
    }
}

impl WorldState {
    /// Fold one decoded inbound packet into the model.
    pub fn apply(&mut self, p: &Packet) {
        match p.id {
            id::SHIP_INITIALIZATION => self.on_ship_init(p),
            id::SHIP_CREATE => self.on_ship_create(p),
            id::SHIP_REMOVE => {
                if let Ok(uid) = p.int("userId") {
                    self.entities.remove(&(uid as i32));
                    if self.pet.id == Some(uid as i32) {
                        self.pet.id = None;
                    }
                }
            }
            id::MOVE_COMMAND => {
                if let (Ok(uid), Ok(x), Ok(y), Ok(t)) = (
                    p.int("userId"),
                    p.int("x"),
                    p.int("y"),
                    p.int("timeToTarget"),
                ) {
                    let e = self.entities.entry(uid as i32).or_default();
                    e.id = uid as i32;
                    e.move_target = Some((x as i32, y as i32));
                    e.move_time_ms = t as i32;
                    // The pet is kept out of `entities`; its last
                    // observed destination doubles as its position.
                    if self.pet.id == Some(uid as i32) {
                        self.pet.x = x as i32;
                        self.pet.y = y as i32;
                    }
                }
            }
            id::HERO_MOVE => {
                if let (Ok(x), Ok(y)) = (p.int("x"), p.int("y")) {
                    if let Some(h) = &mut self.hero {
                        h.x = x as i32;
                        h.y = y as i32;
                    }
                }
            }
            id::SHIP_DESTROYED => {
                if let Ok(uid) = p.int("destroyedUserId") {
                    if let Some(e) = self.entities.get_mut(&(uid as i32)) {
                        e.destroyed = true;
                    }
                    if self
                        .hero
                        .as_ref()
                        .is_some_and(|h| h.user_id == uid as i32)
                        && self.kill_screen.is_none()
                    {
                        self.kill_screen = Some(KillScreen::default());
                    }
                }
            }
            id::ASSET_CREATE => self.on_asset_create(p),
            3245 /* AssetRemoveCommand */ => {
                if let Ok(uid) = p.int("uid") {
                    self.entities.remove(&(uid as i32));
                }
            }
            id::ADD_BOX => self.on_collectable(p, CollectableKind::Box),
            id::ADD_ORE => self.on_collectable(p, CollectableKind::Ore),
            id::ADD_BEACON => self.on_collectable(p, CollectableKind::Beacon),
            id::ADD_MINE => self.on_collectable(p, CollectableKind::Mine),
            id::ADD_FIREWORK_BOX => {
                self.on_collectable(p, CollectableKind::FireworkBox)
            }
            -6137 /* RemoveCollectableCommand */ | id::REMOVE_MINE => {
                if let Ok(h) = p.string("hash") {
                    self.collectables.remove(h);
                }
            }
            id::ATTACK_LASER_RUN => {
                if let (Ok(a), Ok(t)) = (p.int("attackerId"), p.int("targetId")) {
                    self.laser_links.insert(a as i32, t as i32);
                    self.laser_active.insert(a as i32);
                    if let Some(h) = &mut self.hero {
                        if h.user_id == a as i32 {
                            h.target_id = Some(t as i32);
                        }
                    }
                }
            }
            id::ATTACK_ABORT_LASER | id::PET_LASER_ATTACK_STOP => {
                let key = if p.id == id::ATTACK_ABORT_LASER {
                    "uid"
                } else {
                    "petId"
                };
                if let Ok(uid) = p.int(key) {
                    self.laser_active.remove(&(uid as i32));
                    self.laser_links.remove(&(uid as i32));
                    if let Some(h) = &mut self.hero {
                        if h.user_id == uid as i32 {
                            h.target_id = None;
                        }
                    }
                }
            }
            id::ATTACK_HIT | id::ATTACK_HIT_NO_LOCK => {
                if let Ok(victim) = p.int("victimId") {
                    if let Ok(hp) = p.f64("victimHitpoints") {
                        if let Some(h) = &mut self.hero {
                            if h.user_id == victim as i32 {
                                h.hitpoints = hp;
                            }
                        }
                        if let Some(e) = self.entities.get_mut(&(victim as i32)) {
                            e.hitpoints = Some(hp);
                        }
                    }
                    if let Ok(sh) = p.int("victimShield") {
                        if let Some(e) = self.entities.get_mut(&(victim as i32)) {
                            e.shield = Some(sh as i32);
                        }
                    }
                }
            }
            id::HITPOINT_INFO => {
                if let Some(h) = &mut self.hero {
                    if let Ok(v) = p.f64("hitpoints") {
                        h.hitpoints = v;
                    }
                    if let Ok(v) = p.f64("hitpointsMax") {
                        h.hitpoints_max = v;
                    }
                    if let Ok(v) = p.int("nanoHull") {
                        h.nano_hull = v as i32;
                    }
                    if let Ok(v) = p.int("nanoHullMax") {
                        h.nano_hull_max = v as i32;
                    }
                }
            }
            id::ATTRIBUTE_HITPOINT_UPDATE => {
                if let Some(h) = &mut self.hero {
                    if let Ok(v) = p.int("hitpointsNow") {
                        h.hitpoints = v as f64;
                    }
                    if let Ok(v) = p.int("hitpointsMax") {
                        h.hitpoints_max = v as f64;
                    }
                }
            }
            id::ATTRIBUTE_SHIELD_UPDATE => {
                if let Some(h) = &mut self.hero {
                    if let Ok(v) = p.int("shieldNow") {
                        h.shield = v as i32;
                    }
                    if let Ok(v) = p.int("shieldMax") {
                        h.shield_max = v as i32;
                    }
                }
            }
            id::ATTRIBUTE_SHIP_SPEED_UPDATE => {
                if let (Some(h), Ok(v)) = (&mut self.hero, p.int("newSpeed")) {
                    h.speed = v as i32;
                }
            }
            id::ATTRIBUTE_CREDITS_UPDATE => {
                if let Some(h) = &mut self.hero {
                    if let Ok(v) = p.int("credits") {
                        h.credits = v as f64;
                    }
                    if let Ok(v) = p.int("uridium") {
                        h.uridium = v as f64;
                    }
                    if let Ok(v) = p.f64("jackpot") {
                        h.jackpot = v as f32;
                    }
                }
            }
            id::ATTRIBUTE_LEVEL_UP => {
                if let (Some(h), Ok(v)) = (&mut self.hero, p.int("level")) {
                    h.level = v as i32;
                }
            }
            id::ATTRIBUTE_SPACE_UPDATE => {
                if let (Some(h), Ok(v)) = (&mut self.hero, p.int("spaceLeft")) {
                    h.cargo_space = v as i32;
                }
            }
            id::BEACON_COMMAND => {
                self.map_flags = MapFlags {
                    in_radiation_zone: p.boolean("inRadiationZone").unwrap_or(false),
                    in_station: p.boolean("inStation").unwrap_or(false),
                    near_jump_gate: p.boolean("nearJumpGate").unwrap_or(false),
                    repair_robot_active: p.boolean("repairRobotActive").unwrap_or(false),
                };
            }
            id::MAP_ADD_POI => {
                if let Ok(pid) = p.string("poiId") {
                    let coords = p
                        .list("shapeCoordinates")
                        .map(|l| {
                            l.iter()
                                .filter_map(|v| match v {
                                    Value::Int(n) => Some(*n as i32),
                                    _ => None,
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    self.pois.insert(
                        pid.to_owned(),
                        Poi {
                            poi_id: pid.to_owned(),
                            poi_type: None,
                            active: p.boolean("active").unwrap_or(false),
                            coordinates: coords,
                        },
                    );
                }
            }
            6676 /* MapRemovePOICommand */ => {
                if let Ok(pid) = p.string("poiId") {
                    self.pois.remove(pid);
                }
            }
            id::LEGACY => {
                if let Ok(m) = p.string("message") {
                    if let Some((index, _, total)) = asset_log(m) {
                        if let Some(hero) = &mut self.hero {
                            match index { 0 => hero.credits = total, 1 => hero.uridium = total, 2 => hero.honour = total, _ => hero.ep = total }
                        }
                    }
                    // `0|S|CFG|n` — the server's configuration echo
                    // (`ConnectionManager` dispatches on field 1/2).
                    let fields: Vec<&str> = m.split('|').collect();
                    if fields.get(1) == Some(&"S") && fields.get(2) == Some(&"CFG") {
                        if let Some(Ok(c)) = fields.get(3).map(|v| v.parse::<i32>()) {
                            self.configuration = Some(c);
                        }
                    }
                    self.legacy_log.push(m.to_owned());
                    if self.legacy_log.len() > 500 { self.legacy_log.remove(0); }
                }
            }
            id::MAP_CHANGED => {
                // The server re-sends all per-map state on the new map;
                // drop the old tables so stale portals/entities cannot
                // validate commands meant for this map.
                self.entities.clear();
                self.collectables.clear();
                self.pois.clear();
                self.laser_links.clear();
                self.laser_active.clear();
                self.map_flags = MapFlags::default();
                self.portals.clear();
                self.kill_screen = None;
                self.pet.id = None;
                if let Ok(m) = p.int("newMapId") {
                    if let Some(h) = &mut self.hero {
                        h.map_id = m as i32;
                    }
                }
                // The hero is re-seeded like `on_ship_init` does so
                // `hero_pos` keeps working until fresh init arrives.
                if let Some(h) = &self.hero {
                    let uid = h.user_id;
                    self.entities.entry(uid).or_default().id = uid;
                }
            }
            id::JUMPGATE_CREATE => {
                if let Ok(gate) = p.int("gateId") {
                    self.portals.insert(
                        gate as i32,
                        Portal {
                            id: gate as i32,
                            type_id: p.int("designId").map(|v| v as i32).unwrap_or_default(),
                            x: p.int("x").map(|v| v as i32).unwrap_or_default(),
                            y: p.int("y").map(|v| v as i32).unwrap_or_default(),
                            faction_id: p.int("factionId").map(|v| v as i32).unwrap_or_default(),
                            on_minimap: p.boolean("isVisibleOnMinimap").unwrap_or(true),
                        },
                    );
                }
            }
            id::JUMP_GATE_REMOVE => {
                if let Ok(gate) = p.int("gateId") {
                    self.portals.remove(&(gate as i32));
                }
            }
            id::ATTRIBUTE_BOOSTER_UPDATE => self.on_booster_update(p),
            id::ATTRIBUTE_ORE_COUNT_UPDATE => {
                if let Ok(list) = p.list("oreCountList") {
                    for value in list {
                        let Value::Module(m) = value else { continue };
                        let ore = m
                            .module("oreType")
                            .ok()
                            .and_then(|t| t.int("typeValue").ok())
                            .map(|v| v as i32);
                        if let (Some(ore), Ok(count)) = (ore, m.f64("count")) {
                            self.ores.insert(ore, count);
                        }
                    }
                }
            }
            id::ORE_TRADE_INFO => {
                // `OreTradeInfoHandler`: sets `Hero.tradeAreaType` and
                // the per-ore prices shown in the trade window.
                let mut prices = BTreeMap::new();
                for (ore, field) in [
                    (0, "prometiumPrice"),
                    (1, "enduriumPrice"),
                    (2, "terbiumPrice"),
                    (4, "prometidPrice"),
                    (5, "duraniumPrice"),
                    (6, "promeriumPrice"),
                    (7, "sepromPrice"),
                    (8, "palladiumExchangeRate"),
                    (28, "osmiumPrice"),
                ] {
                    if let Ok(v) = p.int(field) {
                        prices.insert(ore, v);
                    }
                }
                self.trade = Some(TradeInfo {
                    area_type: p.int("type").map(|v| v as i32).unwrap_or(0),
                    remaining_charges: p.int("remainingCharges").unwrap_or(-1),
                    prices,
                });
            }
            id::KILL_SCREEN_POST => {
                let mut ks = self.kill_screen.take().unwrap_or_default();
                ks.killer_name = p.string("killerName").unwrap_or_default().to_owned();
                ks.options = kill_screen_options(p);
                self.kill_screen = Some(ks);
            }
            id::KILL_SCREEN_UPDATE => {
                let options = kill_screen_options(p);
                self.kill_screen
                    .get_or_insert_with(KillScreen::default)
                    .options = options;
            }
            id::MENU_INITIALIZATION => self.on_menu_init(p),
            id::MENU_ITEM_STATUS => {
                if let Ok(item_id) = p.string("menuItemId") {
                    let item = self
                        .menu_items
                        .entry(item_id.to_owned())
                        .or_insert_with(|| MenuItem {
                            id: item_id.to_owned(),
                            ..Default::default()
                        });
                    apply_menu_status(item, p);
                }
            }
            id::GALAXY_GATE_INFO => self.on_gate_info(p),
            id::GALAXY_GATE_WAVE_UPDATE => {
                if let (Ok(gate), Ok(wave)) = (p.int("gateId"), p.int("currWave")) {
                    if let Some(g) = self.gates.get_mut(&(gate as i32)) {
                        g.current_wave = wave as i32;
                    }
                    self.last_gate = Some(gate as i32);
                }
            }
            id::GALAXY_GATE_LIFE_LOST => {
                if let (Ok(gate), Ok(lives)) = (p.int("gateId"), p.int("livesLeft")) {
                    if let Some(g) = self.gates.get_mut(&(gate as i32)) {
                        g.lives = lives as i32;
                    }
                    self.last_gate = Some(gate as i32);
                }
            }
            id::GALAXY_GATE_COMPLETE => {
                // `updateGateComplete`: parts set to `currPart`, lives and
                // life price reset; finished once all parts are in.
                if let (Ok(gate), Ok(parts)) = (p.int("gateId"), p.int("currPart")) {
                    let gate_id = gate as i32;
                    let g = self.gates.entry(gate_id).or_insert_with(|| GalaxyGateState {
                        id: gate_id,
                        parts: 0,
                        lives: -1,
                        life_price: -1,
                        current_wave: 0,
                        finished: false,
                        multiplier: 0,
                        mode: String::new(),
                    });
                    g.parts = parts as i32;
                    g.lives = -1;
                    g.life_price = -1;
                    let total = gate_total_parts(gate_id);
                    if total > 0 && g.parts >= total {
                        g.finished = true;
                    }
                    self.last_gate = Some(gate_id);
                }
            }
            id::GALAXY_GATE_SETUP_GATE_COMMAND => {
                // `handlePrepareJumpComplete`: a deployed gate restarts
                // with 0 parts and fresh lives/price.
                if let Ok(gate) = p.int("id") {
                    let gate_id = gate as i32;
                    let g = self.gates.entry(gate_id).or_insert_with(|| GalaxyGateState {
                        id: gate_id,
                        parts: 0,
                        lives: -1,
                        life_price: -1,
                        current_wave: 0,
                        finished: false,
                        multiplier: 0,
                        mode: String::new(),
                    });
                    if let Ok(v) = p.int("livesLeft") {
                        g.lives = v as i32;
                    }
                    if let Ok(v) = p.int("lifePrice") {
                        g.life_price = v as i32;
                    }
                    g.finished = false;
                    g.parts = 0;
                    self.last_gate = Some(gate_id);
                }
            }
            id::GALAXY_GATE_BUY_LIFE_COMMAND => {
                // `handleGateLifePurchaseComplete` — refreshed lives and
                // price after a bought extra life.
                if let Ok(gate) = p.int("gateId") {
                    let gate_id = gate as i32;
                    let g = self.gates.entry(gate_id).or_insert_with(|| GalaxyGateState {
                        id: gate_id,
                        parts: 0,
                        lives: -1,
                        life_price: -1,
                        current_wave: 0,
                        finished: false,
                        multiplier: 0,
                        mode: String::new(),
                    });
                    if let Ok(v) = p.int("livesLeft") {
                        g.lives = v as i32;
                    }
                    if let Ok(v) = p.int("lifePrice") {
                        g.life_price = v as i32;
                    }
                    if let Ok(v) = p.int("multiplier") {
                        g.multiplier = v as i32;
                    }
                    self.last_gate = Some(gate_id);
                }
            }
            id::PET_INITIALIZATION => {
                self.pet.observed = true;
                self.pet.owned = p.boolean("hasPet").unwrap_or(false);
                self.pet.has_fuel = p.boolean("hasFuel").unwrap_or(false);
                self.pet.alive = p.boolean("petIsAlive").unwrap_or(false);
            }
            id::PET_HERO_ACTIVATION => self.on_pet_hero_activation(p),
            id::PET_ACTIVATION => self.on_pet_activation(p),
            id::PET_DEACTIVATION => {
                self.pet.observed = true;
                // `assemblePetDeactivation` ignores the id: it is the
                // hero's pet that goes away.
                let pid = p
                    .int("petId")
                    .ok()
                    .map(|v| v as i32)
                    .filter(|v| self.pet.id == Some(*v))
                    .or(self.pet.id);
                if self.pet.id == pid {
                    self.pet.id = None;
                }
            }
            id::PET_STATUS => self.on_pet_status(p),
            id::PET_HITPOINTS_UPDATE => {
                self.pet.observed = true;
                self.pet.vitals_seen = true;
                if let Ok(v) = p.f64("hitpointsNow") {
                    self.pet.hitpoints = v;
                }
                if let Ok(v) = p.f64("hitpointsMax") {
                    self.pet.hitpoints_max = v;
                }
            }
            id::PET_SHIELD_UPDATE => {
                self.pet.observed = true;
                self.pet.vitals_seen = true;
                if let Ok(v) = p.f64("petShieldNow") {
                    self.pet.shield = v;
                }
                if let Ok(v) = p.f64("petShieldMax") {
                    self.pet.shield_max = v;
                }
            }
            id::PET_FUEL_UPDATE => {
                self.pet.observed = true;
                self.pet.vitals_seen = true;
                if let Ok(v) = p.f64("petFuelNow") {
                    self.pet.fuel = v;
                }
                if let Ok(v) = p.f64("petFuelMax") {
                    self.pet.fuel_max = v;
                }
            }
            id::PET_LEVEL_UPDATE => {
                self.pet.observed = true;
                self.pet.vitals_seen = true;
                if let Ok(v) = p.int("petLevel") {
                    self.pet.level = v as i32;
                }
                // `designId` may change on level-up; the pet is not an
                // `entities` row, so there is nothing else to update.
            }
            id::PET_IDLE_MODE => {
                self.pet.observed = true;
                if let Ok(pid) = p.int("petId") {
                    if self.pet.id == Some(pid as i32) {
                        self.pet.idle = p.boolean("isIdle").unwrap_or(false);
                    }
                }
            }
            id::PET_IS_DESTROYED => {
                self.pet.observed = true;
                self.pet.destroyed = true;
                self.pet.alive = false;
            }
            id::PET_GEAR_ADD => {
                self.pet.observed = true;
                if let Some(gear) = p
                    .module("gearType")
                    .ok()
                    .and_then(|t| t.int("typeValue").ok())
                    .map(|v| v as i32)
                {
                    if !self.pet.gears.contains(&gear) {
                        self.pet.gears.push(gear);
                    }
                }
            }
            id::PET_GEAR_REMOVE => {
                self.pet.observed = true;
                if let Some(gear) = p
                    .module("gearType")
                    .ok()
                    .and_then(|t| t.int("typeValue").ok())
                    .map(|v| v as i32)
                {
                    self.pet.gears.retain(|g| *g != gear);
                }
            }
            id::PET_GEAR_RESET => {
                self.pet.observed = true;
                self.pet.gears.clear();
                self.pet.gear_type = None;
            }
            id::PET_GEAR_ACTIVATION_CHANGE => {
                self.pet.observed = true;
                let gear = p
                    .module("gearType")
                    .ok()
                    .and_then(|t| t.int("typeValue").ok())
                    .map(|v| v as i32);
                let active = p.boolean("active").unwrap_or(false);
                if let Some(gear) = gear {
                    if active {
                        self.pet.gear_type = Some(gear);
                    } else if self.pet.gear_type == Some(gear) {
                        self.pet.gear_type = None;
                    }
                }
            }
            id::LOGIN_RESPONSE => {
                self.logged_in = p.int("status").map(|s| s == 0).unwrap_or(false);
            }
            _ => {}
        }
    }

    fn on_ship_init(&mut self, p: &Packet) {
        let mut h = HeroState::default();
        macro_rules! gi {
            ($field:ident, $name:literal) => {
                if let Ok(v) = p.int($name) {
                    h.$field = v as _;
                }
            };
        }
        macro_rules! gf {
            ($field:ident, $name:literal) => {
                if let Ok(v) = p.f64($name) {
                    h.$field = v as _;
                }
            };
        }
        macro_rules! gs {
            ($field:ident, $name:literal) => {
                if let Ok(v) = p.string($name) {
                    h.$field = v.to_owned();
                }
            };
        }
        macro_rules! gb {
            ($field:ident, $name:literal) => {
                if let Ok(v) = p.boolean($name) {
                    h.$field = v;
                }
            };
        }
        gi!(user_id, "userId");
        gs!(user_name, "userName");
        gs!(type_id, "typeId");
        gs!(clan_tag, "clanTag");
        gi!(x, "x");
        gi!(y, "y");
        gi!(map_id, "mapId");
        gi!(faction_id, "factionId");
        gi!(clan_id, "clanId");
        gi!(level, "level");
        gi!(speed, "speed");
        gf!(hitpoints, "hitPoints");
        gf!(hitpoints_max, "hitMax");
        gi!(nano_hull, "nanoHull");
        gi!(nano_hull_max, "maxNanoHull");
        gi!(shield, "shield");
        gi!(shield_max, "shieldMax");
        gi!(cargo_space, "cargoSpace");
        gi!(cargo_space_max, "cargoSpaceMax");
        gf!(credits, "credits");
        gf!(uridium, "uridium");
        if let Ok(v) = p.f64("jackpot") {
            h.jackpot = v as f32;
        }
        gf!(ep, "ep");
        gf!(honour, "honourPoints");
        gb!(premium, "premium");
        gb!(cloaked, "cloaked");
        let uid = h.user_id;
        self.hero = Some(h);
        // The hero is also a regular map entity.
        self.entities.entry(uid).or_default().id = uid;
    }

    fn on_ship_create(&mut self, p: &Packet) {
        let Ok(uid) = p.int("userId") else { return };
        let e = self.entities.entry(uid as i32).or_default();
        e.id = uid as i32;
        e.kind = EntityKind::Ship;
        macro_rules! gi {
            ($field:ident, $name:literal) => {
                if let Ok(v) = p.int($name) {
                    e.$field = v as _;
                }
            };
        }
        macro_rules! gs {
            ($field:ident, $name:literal) => {
                if let Ok(v) = p.string($name) {
                    e.$field = v.to_owned();
                }
            };
        }
        macro_rules! gb {
            ($field:ident, $name:literal) => {
                if let Ok(v) = p.boolean($name) {
                    e.$field = v;
                }
            };
        }
        gs!(name, "userName");
        gs!(type_id, "typeId");
        gs!(clan_tag, "clanTag");
        gi!(x, "x");
        gi!(y, "y");
        gi!(faction_id, "factionId");
        gi!(clan_id, "clanId");
        gi!(expansion_stage, "expansionStage");
        gb!(npc, "npc");
        gb!(cloaked, "cloaked");
    }

    fn on_asset_create(&mut self, p: &Packet) {
        let Ok(uid) = p.int("assetId") else { return };
        let e = self.entities.entry(uid as i32).or_default();
        e.id = uid as i32;
        e.kind = EntityKind::Asset;
        if let Ok(v) = p.string("userName") {
            e.name = v.to_owned();
        }
        if let Ok(v) = p.string("clanTag") {
            e.clan_tag = v.to_owned();
        }
        if let Ok(v) = p.int("posX") {
            e.x = v as i32;
        }
        if let Ok(v) = p.int("posY") {
            e.y = v as i32;
        }
        if let Ok(v) = p.int("factionId") {
            e.faction_id = v as i32;
        }
        if let Ok(v) = p.int("clanId") {
            e.clan_id = v as i32;
        }
        if let Ok(v) = p.int("expansionStage") {
            e.expansion_stage = v as i32;
        }
        if let Ok(v) = p.int("designId") {
            e.type_id = v.to_string();
        }
    }

    fn on_collectable(&mut self, p: &Packet, kind: CollectableKind) {
        let Ok(hash) = p.string("hash") else { return };
        let type_id = p
            .int("typeId")
            .or_else(|_| p.int("beaconId"))
            .map(|v| v as i32)
            .unwrap_or_else(|_| {
                p.module("oreType")
                    .and_then(|m| m.int("typeValue"))
                    .map(|v| v as i32)
                    .unwrap_or_default()
            });
        let c = Collectable {
            hash: hash.to_owned(),
            kind,
            x: p.int("x").map(|v| v as i32).unwrap_or_default(),
            y: p.int("y").map(|v| v as i32).unwrap_or_default(),
            type_id,
            type_str: p.string("boxType").unwrap_or_default().to_owned(),
            pulse_color_id: p.boolean("maxSkill1").map(i32::from).unwrap_or(0),
            colour_code: p.int("colourCode").map(|v| v as i32).unwrap_or(0),
            event_modifier: p.string("eventModifier").unwrap_or_default().to_owned(),
        };
        self.collectables.insert(hash.to_owned(), c);
    }

    /// Hero position helper used by request builders.
    pub fn hero_pos(&self) -> Option<(i32, i32)> {
        self.hero.as_ref().map(|h| (h.x, h.y))
    }

    /// Nearest collectable of the given kinds to the hero.
    pub fn nearest_collectable(&self, kinds: &[CollectableKind]) -> Option<&Collectable> {
        let (hx, hy) = self.hero_pos()?;
        self.collectables
            .values()
            .filter(|c| kinds.contains(&c.kind))
            .min_by_key(|c| {
                let dx = (c.x - hx) as i64;
                let dy = (c.y - hy) as i64;
                dx * dx + dy * dy
            })
    }

    /// `MenuInitializationCommand` — full session menu; replaces the
    /// item table (`ParseItemsControlMenuDataCommand` semantics).
    fn on_menu_init(&mut self, p: &Packet) {
        self.menu_items.clear();
        for v in p.list("categories").unwrap_or(&[]) {
            let Value::Module(cat) = v else { continue };
            if cat.id != id::MENU_CATEGORY_INIT {
                continue;
            }
            let category_id = cat.string("categoryId").unwrap_or("");
            for iv in cat.list("menuItems").unwrap_or(&[]) {
                let Value::Module(im) = iv else { continue };
                if im.id != id::MENU_ITEM_INIT {
                    continue;
                }
                let Ok(status) = im.module("status") else {
                    continue;
                };
                let Ok(item_id) = status.string("menuItemId") else {
                    continue;
                };
                let mut item = MenuItem {
                    id: item_id.to_owned(),
                    category: category_id.to_owned(),
                    action_style: im.int("actionStyle").unwrap_or(0) as i32,
                    ..Default::default()
                };
                apply_menu_status(&mut item, status);
                self.menu_items.insert(item_id.to_owned(), item);
            }
        }
    }

    /// `PetHeroActivationCommand` — the hero's own pet spawns on the map.
    fn on_pet_hero_activation(&mut self, p: &Packet) {
        self.pet.observed = true;
        let pid = p.int("petId").map(|v| v as i32).unwrap_or_default();
        // Vitals attributed to a different pet's earlier status are not
        // this pet's — reset them.
        if self.pet.status_id.is_some() && self.pet.status_id != Some(pid) {
            self.pet.level = 0;
            self.pet.hitpoints = 0.0;
            self.pet.hitpoints_max = 0.0;
            self.pet.shield = 0.0;
            self.pet.shield_max = 0.0;
            self.pet.fuel = 0.0;
            self.pet.fuel_max = 0.0;
        }
        self.pet.id = Some(pid);
        self.pet.destroyed = false;
        self.pet.alive = true;
        self.pet.idle = false;
        if let Ok(v) = p.int("petLevel") {
            self.pet.level = v as i32;
        }
        self.pet.x = p.int("x").map(|v| v as i32).unwrap_or_default();
        self.pet.y = p.int("y").map(|v| v as i32).unwrap_or_default();
    }

    /// `PetActivationCommand` — a pet joins the map. Only the hero's
    /// own pet is tracked; other players' pets carry no state the
    /// shared API needs.
    fn on_pet_activation(&mut self, p: &Packet) {
        self.pet.observed = true;
        let owner_id = p.int("ownerId").map(|v| v as i32).unwrap_or_default();
        let pid = p.int("petId").map(|v| v as i32).unwrap_or_default();
        let is_hero = self.hero.as_ref().is_some_and(|h| h.user_id == owner_id);
        if !is_hero {
            return;
        }
        if self.pet.status_id.is_some() && self.pet.status_id != Some(pid) {
            self.pet.level = 0;
            self.pet.hitpoints = 0.0;
            self.pet.hitpoints_max = 0.0;
            self.pet.shield = 0.0;
            self.pet.shield_max = 0.0;
            self.pet.fuel = 0.0;
            self.pet.fuel_max = 0.0;
        }
        self.pet.id = Some(pid);
        self.pet.destroyed = false;
        self.pet.alive = true;
        self.pet.idle = p.boolean("isInIdleMode").unwrap_or(false);
        if let Ok(v) = p.int("petLevel") {
            self.pet.level = v as i32;
        }
        self.pet.x = p.int("x").map(|v| v as i32).unwrap_or_default();
        self.pet.y = p.int("y").map(|v| v as i32).unwrap_or_default();
    }

    /// `PetStatusCommand` — full vitals of the hero's pet (may precede
    /// its activation; `status_id` keeps them attributable).
    /// `AttributeBoosterUpdateHandler` → `BoosterProxy.updateCategories`.
    fn on_booster_update(&mut self, p: &Packet) {
        let model = self.boosters.get_or_insert_with(Default::default);
        if let Ok(list) = p.list("boostedDetailsUpdate") {
            for value in list {
                let Value::Module(m) = value else { continue };
                let Ok(loot_id) = m.string("lootId") else { continue };
                match m.int("secondsLeft") {
                    Ok(seconds) => {
                        model.timers.insert(
                            loot_id.to_owned(),
                            (seconds as f64, std::time::Instant::now()),
                        );
                    }
                    // Shared/equipment/station boosters carry no player timer.
                    Err(_) => {
                        model.timers.remove(loot_id);
                    }
                }
            }
        }
        if let Ok(list) = p.list("boostedDetailsRemove") {
            for value in list {
                if let Value::Module(m) = value
                    && let Ok(loot_id) = m.string("lootId")
                {
                    model.timers.remove(loot_id);
                }
            }
        }
        if let Ok(list) = p.list("boostedAttributes") {
            for value in list {
                let Value::Module(m) = value else { continue };
                if let (Ok(category), Ok(percent)) = (m.string("attributeType"), m.f64("value")) {
                    if percent > 0.0 {
                        model.categories.insert(category.to_owned(), percent);
                    } else {
                        model.categories.remove(category);
                    }
                }
            }
        }
    }

    fn on_pet_status(&mut self, p: &Packet) {
        self.pet.observed = true;
        self.pet.vitals_seen = true;
        let Ok(pid) = p.int("petId") else { return };
        let pid = pid as i32;
        if self.pet.id.is_some() && self.pet.id != Some(pid) {
            return;
        }
        self.pet.status_id = Some(pid);
        if let Ok(v) = p.int("petLevel") {
            self.pet.level = v as i32;
        }
        if let Ok(v) = p.f64("petHitPoints") {
            self.pet.hitpoints = v;
        }
        if let Ok(v) = p.f64("petHitPointsMax") {
            self.pet.hitpoints_max = v;
        }
        if let Ok(v) = p.f64("petShieldEnergyNow") {
            self.pet.shield = v;
        }
        if let Ok(v) = p.f64("petShieldEnergyMax") {
            self.pet.shield_max = v;
        }
        if let Ok(v) = p.f64("petCurrentFuel") {
            self.pet.fuel = v;
        }
        if let Ok(v) = p.f64("petMaxFuel") {
            self.pet.fuel_max = v;
        }
    }

    /// `GalaxyGateBuilderInfoCommand` — the whole gate builder table
    /// plus account-level spin data.
    fn on_gate_info(&mut self, p: &Packet) {
        self.gate_builder = Some(GateBuilderInfo {
            energy: p.f64("money").unwrap_or_default(),
            energy_cost: p.int("energy_cost").unwrap_or_default(),
            spin_amount: p.int("spinamount_selected").unwrap_or_default(),
            spin_on_sale: p.boolean("spinOnSale").unwrap_or(false),
            galaxy_gate_day: p.boolean("galaxyGateDay").unwrap_or(false),
            bonus_rewards_day: p.boolean("bonusRewardsDay").unwrap_or(false),
        });
        for v in p.list("gates").unwrap_or(&[]) {
            let Value::Module(g) = v else { continue };
            if g.id != id::GALAXY_GATE_GATE_MODULE {
                continue;
            }
            let Ok(gate_id) = g.int("id") else { continue };
            let gate_id = gate_id as i32;
            let lives = g.int("livesLeft").unwrap_or(-1) as i32;
            let life_price = g.int("lifePrice").unwrap_or(-1) as i32;
            let parts = g.int("currPart").unwrap_or(0) as i32;
            let finished = g.int("state").unwrap_or(0) == 1
                || (gate_total_parts(gate_id) > 0
                    && (lives == -1 || life_price == -1)
                    && parts >= gate_total_parts(gate_id));
            self.gates.insert(
                gate_id,
                GalaxyGateState {
                    id: gate_id,
                    parts,
                    lives,
                    life_price,
                    current_wave: g.int("currWave").unwrap_or(0) as i32,
                    finished,
                    multiplier: g.int("multiplier").unwrap_or(0) as i32,
                    mode: g.string("mode").unwrap_or_default().to_owned(),
                },
            );
        }
    }
}

/// Copy `MenuItemStatusCommand` fields into `item` — shared by the
/// init path (nested `status` module) and standalone updates.
fn apply_menu_status(item: &mut MenuItem, status: &Packet) {
    item.counter_value = status.f64("counterValue").unwrap_or(0.0);
    item.max_counter_value = status.f64("maxCounterValue").unwrap_or(0.0);
    item.selected = status.boolean("selected").unwrap_or(false);
    item.available = status.boolean("available").unwrap_or(false);
    item.blocked = status.boolean("blocked").unwrap_or(false);
    item.activatable = status.boolean("activatable").unwrap_or(false);
    item.visible = status.boolean("visible").unwrap_or(false);
}

/// Parse the `options` vector of `KillScreenPostCommand`/
/// `KillScreenUpdateCommand` — each element is a
/// `KillScreenOptionModule`.
fn kill_screen_options(p: &Packet) -> Vec<KillScreenOption> {
    p.list("options")
        .unwrap_or(&[])
        .iter()
        .filter_map(|v| {
            let Value::Module(m) = v else {
                return None;
            };
            if m.id != id::KILL_SCREEN_OPTION_MODULE {
                return None;
            }
            let price = m.module("price").ok();
            Some(KillScreenOption {
                repair_type: m
                    .module("repairType")
                    .ok()
                    .and_then(|t| t.int("repairTypeValue").ok())
                    .unwrap_or(-1) as i32,
                cost: price.and_then(|p| p.int("amount").ok()).unwrap_or(0),
                currency: price.and_then(|p| p.int("type").ok()).unwrap_or(0) as i32,
                affordable: m.boolean("affordableForPlayer").unwrap_or(false),
                cooldown_ms: m.int("cooldownTime").unwrap_or(0),
            })
        })
        .collect()
}
