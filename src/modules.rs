//! Typed builders/parsers for the packets a bot actually uses.
//!
//! Field names and wire order come from the generated schema table; the
//! coordinate fixups replicate the anti-tamper checksums applied by
//! `EventManager.as` / `MapAssetAttackCommand.as` /
//! `ActivateRocketCommand.as` / `TryToSelectMapAssetCommand.as`:
//!
//! * `MoveRequest`: `(posX + targetY) % 3 == 0`, `(posY + targetX) % 3 != 0`
//! * `AttackLaserRequest`: `(targetX + targetY) % 3 != 0` (bumps X)
//! * `AttackRocketRequest`: `(targetX + targetY) % 3 != 0` (bumps Y)
//! * `CollectBoxRequest`: `(boxY + posX + posY) % 3 != 0` (bumps posY)
//! * `ShipSelectRequest`: `(targetX+targetY+posX+posY) % 8 == 0`
//!   (subtracts remainder from targetY)

use crate::packet::{Packet, Value};

pub mod id {
    pub const LEGACY: i16 = 1;
    pub const KEEP_ALIVE: i16 = 2;
    pub const ATTACK_LASER_RUN: i16 = 3;
    pub const ATTACK_LASER_REQUEST: i16 = 4;
    pub const LOGIN_REQUEST: i16 = 7;
    pub const ATTRIBUTE_HITPOINT_UPDATE: i16 = 8;
    pub const ATTRIBUTE_CREDITS_UPDATE: i16 = 9;
    pub const JUMP_REQUEST: i16 = 10;
    pub const READY_REQUEST: i16 = 14;
    pub const LOGOUT: i16 = 16;
    pub const JUMP_DENIED: i16 = 18;
    pub const JUMP_INITIATED: i16 = 19;
    pub const JUMP_TO_SERVER: i16 = 20;
    pub const ATTACK_ABORT_LASER_REQUEST: i16 = 25;
    pub const COLLECT_BOX_REQUEST: i16 = 26;
    pub const ATTACK_TYPE_MODULE: i16 = 27;
    pub const ATTACK_HIT: i16 = 28;
    pub const HERO_MOVE: i16 = 29;
    pub const SHIP_DESTROYED: i16 = 30;
    pub const SHIP_INITIALIZATION: i16 = 49;
    pub const SHIP_REMOVE: i16 = 52;
    pub const LOGIN_ERROR: i16 = 61;
    pub const MOVE_REQUEST: i16 = 82;
    pub const SHIP_CREATE: i16 = 83;
    pub const ATTACK_ROCKET_REQUEST: i16 = 89;
    pub const MOVE_COMMAND: i16 = 90;
    pub const HARVEST_REQUEST: i16 = 94;
    pub const ATTRIBUTE_LEVEL_UP: i16 = 95;
    pub const ATTRIBUTE_SHIP_SPEED_UPDATE: i16 = 96;
    pub const ATTRIBUTE_SPACE_UPDATE: i16 = 97;
    pub const ATTRIBUTE_CONFIG_COUNT: i16 = 98;
    pub const ATTRIBUTE_BOOSTER_UPDATE: i16 = 101;
    pub const ATTRIBUTE_SKILL_SHIELD_UPDATE: i16 = 103;
    pub const ORE_COUNT_MODULE: i16 = 105;
    pub const ATTRIBUTE_ORE_COUNT_UPDATE: i16 = 106;
    /// `TradeRequest` — opens/refreshes the ore trade window
    /// (`Refinery`/trade window maximize).
    pub const TRADE_REQUEST: i16 = 81;
    /// `TradeReplyCommand` — `priceInfos` answer to `TradeRequest`.
    pub const TRADE_REPLY: i16 = 130;
    pub const TRADE_SELL_ORE_REQUEST: i16 = 131;
    pub const LAB_REFINEMENT_REQUEST: i16 = 132;
    /// `LabUpdateItemRequest` — apply `updateWith` ore units to item
    /// `itemToUpdate` (LabItemModule.itemValue: 0 LASER / 1 ROCKET /
    /// 2 DRIVING / 3 SHIELD).
    pub const LAB_UPDATE_ITEM_REQUEST: i16 = 133;
    /// `LabItemModule` — the lab slot a `LabUpdateItemRequest` targets.
    pub const LAB_ITEM_MODULE: i16 = 134;
    /// `LabUpdateItemCommand` — server pushes `itemsUpdatedInfo`
    /// (applied ore type + amount per lab item).
    pub const LAB_UPDATE_ITEM_COMMAND: i16 = 135;
    /// `LabUpdateRequest` — asks for the lab item amounts (window open /
    /// periodic refresh while the refinery window is expanded).
    pub const LAB_UPDATE_REQUEST: i16 = 137;
    pub const MAP_CHANGED: i16 = 138;
    pub const ADD_ORE: i16 = 147;
    pub const PET_REQUEST: i16 = 153;
    pub const PET_GEAR_ACTIVATION_REQUEST: i16 = 154;
    pub const PET_HERO_ACTIVATION: i16 = 155;
    pub const PET_DEACTIVATION: i16 = 156;
    pub const PET_STATUS: i16 = 157;
    pub const PET_HITPOINTS_UPDATE: i16 = 158;
    pub const PET_EXPERIENCE_UPDATE: i16 = 159;
    pub const PET_LEVEL_UPDATE: i16 = 160;
    pub const PET_SHIELD_UPDATE: i16 = 161;
    pub const PET_FUEL_UPDATE: i16 = 162;
    pub const PET_GEAR_TYPE_MODULE: i16 = 164;
    pub const SHIP_SELECT_REQUEST: i16 = 165;
    pub const ORE_TYPE_MODULE: i16 = -24450;
    pub const PET_IS_DESTROYED: i16 = 227;
    pub const PET_INITIALIZATION: i16 = 229;
    pub const PET_ACTIVATION: i16 = 232;
    pub const PET_GEAR_REMOVE: i16 = 233;
    pub const PET_GEAR_ADD: i16 = 234;
    pub const PET_GEAR_RESET: i16 = 235;
    pub const PET_IDLE_MODE: i16 = 242;
    pub const PET_LASER_ATTACK_STOP: i16 = 243;
    /// `PetTradeWindowShowCommand` — server opens the PET trade window.
    pub const PET_TRADE_WINDOW_SHOW: i16 = 247;
    pub const ATTRIBUTE_SHIELD_UPDATE: i16 = 248;
    pub const KILL_SCREEN_POST: i16 = 252;
    pub const KILL_SCREEN_OPTION_MODULE: i16 = 253;
    pub const KILL_SCREEN_OPTION_TYPE_MODULE: i16 = 254;
    pub const KILL_SCREEN_REPAIR_REQUEST: i16 = 255;
    pub const CHANNEL_CLOSE_REQUEST: i16 = 260;
    pub const LOG_MESSAGE: i16 = 2590;
    pub const KILL_SCREEN_UPDATE: i16 = 6777;
    pub const INVITATION_ACCEPT_REQUEST: i16 = -2962;
    pub const JUMPGATE_CREATE: i16 = 3389;
    pub const JUMP_GATE_REMOVE: i16 = 4274;
    pub const MENU_INITIALIZATION: i16 = -8140;
    pub const SLOT_BAR_INIT: i16 = -620;
    pub const SLOT_INFO: i16 = -20408;
    pub const MENU_CATEGORY_INIT: i16 = -3461;
    pub const MENU_ITEM_INIT: i16 = 23633;
    pub const MENU_ACTION_REQUEST: i16 = 8766;
    pub const PET_GEAR_ACTIVATION_CHANGE: i16 = 4943;
    pub const GALAXY_GATE_INFO: i16 = -20312;
    pub const GALAXY_GATE_GATE_MODULE: i16 = 27276;
    pub const GALAXY_GATE_WAVE_UPDATE: i16 = 21168;
    pub const GALAXY_GATE_COMPLETE: i16 = -12924;
    pub const GALAXY_GATE_LIFE_LOST: i16 = -9602;
    pub const GALAXY_GATE_SETUP_GATE_COMMAND: i16 = -10753;
    pub const GALAXY_GATE_BUY_LIFE_COMMAND: i16 = 31119;
    pub const GALAXY_GATE_MULTI_ENERGY_REQUEST: i16 = 24604;
    pub const GALAXY_GATE_SETUP_GATE_REQUEST: i16 = 1978;
    pub const GALAXY_GATE_BUY_LIFE_REQUEST: i16 = 30847;
    pub const HELLSTORM_ATTACK: i16 = 287;
    pub const VERSION_REQUEST: i16 = 666;
    pub const VERSION_COMMAND: i16 = 667;
    pub const HITPOINT_INFO: i16 = 1300;
    pub const BEACON_COMMAND: i16 = 29630;
    pub const COMMAND_CENTER_NEWS_CLICKED: i16 = 30716;
    pub const ATTACK_ABORT_LASER: i16 = 10141;
    pub const ASSET_INFORMATION: i16 = 14266;
    pub const ADD_MINE: i16 = 14466;
    pub const ASSET_CREATE: i16 = -14859;
    pub const ADD_BOX: i16 = -4037;
    pub const ATTACK_HIT_NO_LOCK: i16 = -1145;
    pub const WINDOW_ITEM: i16 = -5398;
    pub const MAP_ADD_POI: i16 = -29100;
    pub const ADD_FIREWORK_BOX: i16 = -9316;
    pub const ADD_BEACON: i16 = 2850;
    pub const STAYIN_ALIVE: i16 = 8427;
    pub const LOGIN_RESPONSE: i16 = 13922;
    pub const REMOVE_MINE: i16 = 19063;
    /// `OreTradeInfoCommand` — trade-area type (`TRADETYPE_*`) plus ore
    /// prices; `type` 0 means trading is currently inactive.
    pub const ORE_TRADE_INFO: i16 = 1870;
    pub const MENU_ITEM_STATUS: i16 = 24049;
    pub const QUEST_CONDITION_UPDATE: i16 = 2014;
    pub const MAP_ASSET_ACTION_AVAILABLE: i16 = 6530;
    pub const ETERNAL_BLACKLIGHT_RANK_INFO: i16 = 10659;
    pub const PING: i16 = -7327;
    pub const PING_POSITION_REQUEST: i16 = 7747;
    pub const RELOGIN: i16 = -30607;
    /// `EmoteActivationCommand` — `{userId, lootId}`; plays the emote
    /// clip over the ship for `EmoteAttributes.duration` (5 s).
    pub const EMOTE_ACTIVATION: i16 = 4739;
    /// `AbilityEffectActivationCommand` — `{selectedAbilityId,
    /// activatorId, isPersistent, targetIds}`.
    pub const ABILITY_EFFECT_ACTIVATION: i16 = 275;
}

/// Fallback `VersionRequest` hash when the launcher flashvars do not name
/// a deployed `main.swf`. Live sessions replace this with the
/// `VersionHash` constant inside that SWF; the server rejects a stale one.
pub const VERSION_HASH: &str = "43e9184871f4b91cc4158e5c5bef3d3f";

fn mk(id: i16, fields: Vec<(&'static str, Value)>) -> Packet {
    Packet {
        id,
        name: crate::registry::module_name(id),
        fields: fields.into_iter().map(|(n, v)| (Some(n), v)).collect(),
    }
}

fn i(name: &'static str, v: i64) -> (&'static str, Value) {
    (name, Value::Int(v))
}

fn s(name: &'static str, v: impl Into<String>) -> (&'static str, Value) {
    (name, Value::Str(v.into()))
}

fn b(name: &'static str, v: bool) -> (&'static str, Value) {
    (name, Value::Bool(v))
}

// --------------------------------------------------------------------
// Session

/// First message after the `0x00` greeting byte.
pub fn version_request(version: &str) -> Packet {
    mk(id::VERSION_REQUEST, vec![s("version", version)])
}

/// `LoginRequest`: user id, faction (i16), session token, version hash,
/// game instance id, mini-client flag.
pub fn login_request(
    user_id: i32,
    faction_id: i16,
    session_id: &str,
    version: &str,
    instance_id: i32,
    is_mini_client: bool,
) -> Packet {
    mk(
        id::LOGIN_REQUEST,
        vec![
            i("userID", user_id as i64),
            i("factionID", faction_id as i64),
            s("sessionID", session_id),
            s("version", version),
            i("instanceId", instance_id as i64),
            b("isMiniClient", is_mini_client),
        ],
    )
}

pub const READY_MAP_LOADED_2D: i16 = 0;
pub const READY_MAP_LOADED_3D: i16 = 1;
pub const READY_UI_READY: i16 = 2;

/// `ReadyRequest` sent twice post-login: once with `MAP_LOADED_*` after
/// hero init, once with `UI_READY`.
pub fn ready_request(ready_type_str: &str, ready_type: i16) -> Packet {
    mk(
        id::READY_REQUEST,
        vec![
            s("readyTypeStr", ready_type_str),
            i("readyType", ready_type as i64),
        ],
    )
}

/// `KeepAlive` (`MouseClick` bool mirrors the client heartbeat field).
pub fn keep_alive(mouse_click: bool) -> Packet {
    mk(id::KEEP_ALIVE, vec![b("MouseClick", mouse_click)])
}

/// `LegacyModule` — free-form text command channel (e.g. `"w"` login flag).
pub fn legacy(message: &str) -> Packet {
    mk(id::LEGACY, vec![s("message", message)])
}

/// `StayinAlive` marker module (no payload).
pub fn stayin_alive() -> Packet {
    mk(id::STAYIN_ALIVE, vec![])
}

/// `ChannelCloseRequest` — polite close flag.
pub fn channel_close(close: bool) -> Packet {
    mk(id::CHANNEL_CLOSE_REQUEST, vec![b("close", close)])
}

// --------------------------------------------------------------------
// Movement

/// `MoveRequest` with the full client-side coordinate transform:
/// target masked to 23 bits when positive, `posY` bumped until
/// `(posY + targetX) % 3 != 0`, `posX` reduced so
/// `(posX + targetY) % 3 == 0`.
///
/// `position` is the hero's current position, `target` the destination.
pub fn move_request(position: (i32, i32), target: (i32, i32)) -> Packet {
    const MASK: i32 = 8_388_607; // 0x7FFFFF
    let mut tx = target.0;
    let mut ty = target.1;
    if tx > 0 {
        tx &= MASK;
    }
    if ty > 0 {
        ty &= MASK;
    }
    let mut px = position.0;
    let mut py = position.1;
    if (py + tx) % 3 == 0 {
        py += 1;
    }
    if px > 0 {
        px &= MASK;
    }
    if (py + tx) % 3 == 0 {
        py += 1;
    }
    // AS3 `%` is a truncated remainder, identical to Rust's `%`.
    px -= (px + ty) % 3;
    mk(
        id::MOVE_REQUEST,
        vec![
            i("positionX", px as i64),
            i("targetY", ty as i64),
            i("targetX", tx as i64),
            i("positionY", py as i64),
        ],
    )
}

// --------------------------------------------------------------------
// Combat

/// `AttackLaserRequest` — lock+fire at `target`. `target` is the
/// victim's position; bumped +1 on X so `(tx + ty) % 3 != 0`.
pub fn attack_laser_request(target_id: i32, target: (i32, i32)) -> Packet {
    let mut tx = target.0;
    let ty = target.1;
    if (tx + ty) % 3 == 0 {
        tx += 1;
    }
    mk(
        id::ATTACK_LASER_REQUEST,
        vec![
            i("targetId", target_id as i64),
            i("targetX", tx as i64),
            i("targetY", ty as i64),
        ],
    )
}

/// `AttackRocketRequest` — rocket at `target`; bumps Y instead of X.
pub fn attack_rocket_request(target_id: i32, target: (i32, i32)) -> Packet {
    let tx = target.0;
    let mut ty = target.1;
    if (tx + ty) % 3 == 0 {
        ty += 1;
    }
    mk(
        id::ATTACK_ROCKET_REQUEST,
        vec![
            i("targetId", target_id as i64),
            i("targetX", tx as i64),
            i("targetY", ty as i64),
        ],
    )
}

/// `AttackAbortLaserRequest` — marker, stops the laser.
pub fn attack_abort_laser_request() -> Packet {
    mk(id::ATTACK_ABORT_LASER_REQUEST, vec![])
}

// --------------------------------------------------------------------
// Interaction

/// `ShipSelectRequest` — click-select an entity. `click`/`radius` are
/// screen-space pick data (0/0 acceptable for a headless client).
/// Adjusts `targetY` down by `(tx+ty+px+py) % 8` so the sum is a
/// multiple of 8.
#[allow(clippy::too_many_arguments)]
pub fn ship_select_request(
    target_id: i32,
    target: (i32, i32),
    position: (i32, i32),
    click: (i32, i32),
    radius: i32,
) -> Packet {
    let (tx, mut ty) = target;
    let (px, py) = position;
    ty -= (tx + ty + py + px) % 8;
    mk(
        id::SHIP_SELECT_REQUEST,
        vec![
            i("targetId", target_id as i64),
            i("targetX", tx as i64),
            i("targetY", ty as i64),
            i("posX", px as i64),
            i("posY", py as i64),
            i("clickX", click.0 as i64),
            i("clickY", click.1 as i64),
            i("radius", radius as i64),
        ],
    )
}

/// `CollectBoxRequest` — pick up a box/beacon once on top of it.
/// `posY` bumps +1 when `(boxY + posX + posY) % 3 == 0`.
pub fn collect_box_request(item_hash: &str, box_pos: (i32, i32), position: (i32, i32)) -> Packet {
    let (bx, by) = box_pos;
    let (px, mut py) = position;
    if (by + px + py) % 3 == 0 {
        py += 1;
    }
    mk(
        id::COLLECT_BOX_REQUEST,
        vec![
            s("itemHash", item_hash),
            i("boxX", bx as i64),
            i("boxY", by as i64),
            i("posX", px as i64),
            i("posY", py as i64),
        ],
    )
}

/// `HarvestRequest` — collect ore/cargo items by hash.
pub fn harvest_request(item_hash: &str) -> Packet {
    mk(id::HARVEST_REQUEST, vec![s("itemHash", item_hash)])
}

/// Ore count payload used by the game's trade and refinery requests.
/// The protocol models the ore type and amount as `OreCountModule`
/// nested inside the request (rather than scalar request fields).
fn ore_count(ore_type: i32, amount: u64) -> Packet {
    let ore_type = Packet {
        id: id::ORE_TYPE_MODULE,
        name: crate::registry::module_name(id::ORE_TYPE_MODULE),
        fields: vec![(Some("typeValue"), Value::Int(ore_type as i64))],
    };
    Packet {
        id: id::ORE_COUNT_MODULE,
        name: crate::registry::module_name(id::ORE_COUNT_MODULE),
        fields: vec![
            (Some("oreType"), Value::Module(Box::new(ore_type))),
            (Some("count"), Value::F64(amount as f64)),
        ],
    }
}

/// `TradeSellOreRequest` (module 131), with its verified OreCountModule payload.
pub fn sell_ore_request(ore_type: i32, amount: u64) -> Packet {
    mk(
        id::TRADE_SELL_ORE_REQUEST,
        vec![(
            "toSell",
            Value::Module(Box::new(ore_count(ore_type, amount))),
        )],
    )
}

/// `LabRefinementRequest` (module 132), with its verified OreCountModule payload.
pub fn refine_ore_request(ore_type: i32, amount: u64) -> Packet {
    mk(
        id::LAB_REFINEMENT_REQUEST,
        vec![(
            "toProduce",
            Value::Module(Box::new(ore_count(ore_type, amount))),
        )],
    )
}

/// `LabUpdateItemRequest` (module 133) — `itemToUpdate` is a
/// `LabItemModule` (`itemValue` = item uint 0..3), `updateWith` the
/// `OreCountModule` amount applied (`RefinementManager.startUpdateItem`).
pub fn lab_update_item_request(item_value: i16, ore_type: i32, amount: u64) -> Packet {
    let item = Packet {
        id: id::LAB_ITEM_MODULE,
        name: crate::registry::module_name(id::LAB_ITEM_MODULE),
        fields: vec![(Some("itemValue"), Value::Int(item_value as i64))],
    };
    mk(
        id::LAB_UPDATE_ITEM_REQUEST,
        vec![
            ("itemToUpdate", Value::Module(Box::new(item))),
            (
                "updateWith",
                Value::Module(Box::new(ore_count(ore_type, amount))),
            ),
        ],
    )
}

/// `LabUpdateRequest` (module 137) — marker, pulls the lab item amounts.
pub fn lab_update_request() -> Packet {
    mk(id::LAB_UPDATE_REQUEST, vec![])
}

/// `PetRequest` activation/deactivation. The verified model enum is 0/1.
pub fn set_pet_request(enabled: bool) -> Packet {
    mk(
        id::PET_REQUEST,
        vec![("petRequestType", Value::Int(if enabled { 0 } else { 1 }))],
    )
}

/// `PetGearActivationRequest` with a typed `PetGearTypeModule` and target id.
pub fn select_pet_gear_request(gear_type: i32, target: i32) -> Packet {
    let gear = Packet {
        id: id::PET_GEAR_TYPE_MODULE,
        name: crate::registry::module_name(id::PET_GEAR_TYPE_MODULE),
        fields: vec![(Some("typeValue"), Value::Int(gear_type as i64))],
    };
    mk(
        id::PET_GEAR_ACTIVATION_REQUEST,
        vec![
            ("gearTypeToActivate", Value::Module(Box::new(gear))),
            ("optParam", Value::Int(target as i64)),
        ],
    )
}

/// `GalaxyGateBuilderMultiEnergyRequest` — `spin_amount` is 1, 5, 10 or
/// 100 (validated by the caller).
pub fn spin_galaxy_gate(gate_id: i32, spin_amount: u8, use_multiplier: bool) -> Packet {
    mk(
        id::GALAXY_GATE_MULTI_ENERGY_REQUEST,
        vec![
            ("spinAmount", Value::Int(spin_amount.into())),
            ("useMultiplier", Value::Bool(use_multiplier)),
            ("gateId", Value::Int(gate_id.into())),
        ],
    )
}

/// `GalaxyGateBuilderSetupGateRequest` — deploy a fully built gate.
pub fn place_galaxy_gate(gate_id: i32) -> Packet {
    mk(
        id::GALAXY_GATE_SETUP_GATE_REQUEST,
        vec![("gateId", Value::Int(gate_id.into()))],
    )
}

/// `GalaxyGateBuilderBuyLifeRequest` — purchase one extra gate life.
pub fn buy_galaxy_life(gate_id: i32) -> Packet {
    mk(
        id::GALAXY_GATE_BUY_LIFE_REQUEST,
        vec![("gateId", Value::Int(gate_id.into()))],
    )
}

/// `InvitationAcceptRequest` — join the group of `inviter_id`
/// (`GroupProxy.acceptInvitation`).
pub fn invitation_accept_request(inviter_id: i32) -> Packet {
    mk(
        id::INVITATION_ACCEPT_REQUEST,
        vec![("inviterId", Value::Int(inviter_id as i64))],
    )
}

/// `MenuActionRequest` on the item bar, the packet
/// `ItemsControlMenuProxy.sendActionRequest` dispatches:
/// `actionType` is `ACTIONTYPE_ACTIVATE`(1)/`ACTIONTYPE_SELECT`(0) and
/// `sourceType` is `SOURCETYPE_ITEM_BAR`(0) (see `MenuActionRequest.as`).
pub fn menu_action_request(item_id: &str, activate: bool) -> Packet {
    mk(
        id::MENU_ACTION_REQUEST,
        vec![
            ("actionType", Value::Int(activate as i64)),
            ("sourceType", Value::Int(0)),
            ("menuItemId", Value::Str(item_id.to_owned())),
        ],
    )
}

/// `KillScreenRepairRequest` — `selection` picks the respawn option
/// (`KillScreenOptionTypeModule.repairTypeValue`, e.g. 1 = basic repair)
/// and `requestModule` re-authenticates with a fresh `LoginRequest`,
/// exactly what `Killscreen.repair` does client-side.
pub fn repair_request(login: &crate::session::LoginInfo, repair_type: i32) -> Packet {
    let selection = Packet {
        id: id::KILL_SCREEN_OPTION_TYPE_MODULE,
        name: crate::registry::module_name(id::KILL_SCREEN_OPTION_TYPE_MODULE),
        fields: vec![(Some("repairTypeValue"), Value::Int(repair_type as i64))],
    };
    let request = login_request(
        login.user_id,
        login.faction_id,
        &login.session_id,
        &login.version,
        login.instance_id,
        login.is_mini_client,
    );
    mk(
        id::KILL_SCREEN_REPAIR_REQUEST,
        vec![
            ("selection", Value::Module(Box::new(selection))),
            ("requestModule", Value::Module(Box::new(request))),
        ],
    )
}

/// `JumpRequest` — marker module sent when standing on a portal.
pub fn jump_request() -> Packet {
    mk(id::JUMP_REQUEST, vec![])
}

/// `PingPositionRequest` — client position report (unrotated pair).
pub fn ping_position_request(position: (i32, i32)) -> Packet {
    mk(
        id::PING_POSITION_REQUEST,
        vec![i("x", position.0 as i64), i("y", position.1 as i64)],
    )
}

// --------------------------------------------------------------------
// Inbound parsers (thin accessors over the generic Packet)

/// `VersionCommand` fields.
pub fn parse_version_command(p: &Packet) -> Option<(String, bool)> {
    Some((
        p.string("version").ok()?.to_owned(),
        p.boolean("equal").ok()?,
    ))
}

/// `LoginResponse` status code (`0` = success).
pub fn parse_login_response(p: &Packet) -> Option<i16> {
    p.int("status").ok().map(|v| v as i16)
}

/// `MoveCommand` — observed entity motion.
pub fn parse_move_command(p: &Packet) -> Option<(i32, (i32, i32), i32)> {
    Some((
        p.int("userId").ok()? as i32,
        (p.int("x").ok()? as i32, p.int("y").ok()? as i32),
        p.int("timeToTarget").ok()? as i32,
    ))
}

/// `HeroMoveCommand` — authoritative hero position.
pub fn parse_hero_move(p: &Packet) -> Option<(i32, i32)> {
    Some((p.int("x").ok()? as i32, p.int("y").ok()? as i32))
}

/// `ShipRemoveCommand`.
pub fn parse_ship_remove(p: &Packet) -> Option<i32> {
    p.int("userId").ok().map(|v| v as i32)
}

/// `AttackLaserRunCommand` — laser started running between two entities.
pub fn parse_laser_run(p: &Packet) -> Option<(i32, i32)> {
    Some((
        p.int("attackerId").ok()? as i32,
        p.int("targetId").ok()? as i32,
    ))
}

/// `AttackAbortLaserCommand` — laser stopped for uid.
pub fn parse_laser_abort(p: &Packet) -> Option<i32> {
    p.int("uid").ok().map(|v| v as i32)
}

/// `ShipDestroyedCommand`.
pub fn parse_ship_destroyed(p: &Packet) -> Option<(i32, i32)> {
    Some((
        p.int("destroyedUserId").ok()? as i32,
        p.int("explosionTypeId").ok()? as i32,
    ))
}

/// `LogMessageCommand` — console/log text (`message.baseKey` with
/// `wildCardReplacements` substituted), the same line the HUD log shows.
pub fn parse_log_message(p: &Packet) -> Option<String> {
    let message = p.module("message").ok()?;
    let mut text = message.string("baseKey").ok()?.to_owned();
    if let Ok(replacements) = message.list("wildCardReplacements") {
        for value in replacements {
            if let Value::Module(replacement) = value {
                if let (Ok(key), Ok(value)) = (
                    replacement.string("wildcard"),
                    replacement.string("replacement"),
                ) {
                    if text.contains(key) {
                        text = text.replace(key, value);
                    } else {
                        text.push_str(&format!(" {key}={value}"));
                    }
                }
            }
        }
    }
    Some(text)
}
