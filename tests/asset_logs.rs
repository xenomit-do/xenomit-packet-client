use xenomit_packet_client::{
    model::{HeroState, WorldState, asset_log},
    modules::id,
    packet::{Packet, Value},
};

#[test]
fn legacy_rewards_and_spending_update_authoritative_assets() {
    let mut world = WorldState {
        hero: Some(HeroState::default()),
        ..Default::default()
    };
    for message in [
        "0|y|CRE|100|1000",
        "0|y|URI|-50|500",
        "0|LM|STD|HON|10|200",
        "0|y|EP|500|10000|5",
    ] {
        world.apply(&Packet {
            id: id::LEGACY,
            name: Some("LegacyModule"),
            fields: vec![(Some("message"), Value::Str(message.into()))],
        });
    }
    let hero = world.hero.unwrap();
    assert_eq!(
        [hero.credits, hero.uridium, hero.honour, hero.ep],
        [1000.0, 500.0, 200.0, 10000.0]
    );
    assert_eq!(asset_log("0|y|URI|-50|500"), Some((1, -50.0, 500.0)));
    for malformed in [
        "0|y|CRE|100",
        "0|y|CRE|NaN|1000",
        "0|y|URI|100|inf",
        "0|i|CRE|100|1000",
    ] {
        assert_eq!(asset_log(malformed), None);
    }
}
