#[macro_use]
extern crate serde_json;

use casniam::protocol::events::EventBuilder;

use serde_json::Value;

#[test]
fn test_create_room() {
    let room_id = "!room_id:example.com";
    let creator = "@alice:example.com";

    let mut builder =
        EventBuilder::new(room_id, creator, "m.room.create", Some(""));

    let content = if let Value::Object(o) = json!({
        "creator": creator.to_string(),
    }) {
        o
    } else {
        panic!();
    };

    builder = builder.with_content(content);
    builder = builder.origin("example.com");

    // TODO: Actually create the event.
}
