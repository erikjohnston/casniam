#![feature(await_macro, async_await, futures_api)]

#[macro_use]
extern crate serde_derive;

mod protocol;

use serde::de::IgnoredAny;

use crate::protocol::{DagChunkFragment, Event};

#[derive(Deserialize)]
struct V1Event {
    event_id: String,
    prev_events: Vec<(String, IgnoredAny)>,
}

impl Event for V1Event {
    fn get_prev_event_ids(&self) -> Vec<&str> {
        self.prev_events.iter().map(|(e, _)| &e as &str).collect()
    }

    fn get_event_id(&self) -> &str {
        &self.event_id
    }
}

fn main() {
    let evs = vec![
        r#"{"origin": "matrix.org", "signatures": {"matrix.org": {"ed25519:auto": "BJg0X+tjw0qYO13vHXx0ITskHuQ+m+mcXU+zVmOfnkT3IeDoRA6pN0Zju0uvH7CorYwU9Fv0ZAbx7UbrDrccBQ"}}, "origin_server_ts": 1536425000270, "sender": "@flux:matrix.org", "event_id": "$1536425000861889JUJJr:matrix.org", "prev_events": [["$1536424989861854NQRKF:matrix.org", {"sha256": "1ag6bYVMQlmYHULKLz5fzpkV2CK5dfZ/o1vOlPeVQpg"}]], "unsigned": {"age_ts": 1536425000270}, "content": {"body": "one just needs to write the code for it ;-)", "msgtype": "m.text"}, "depth": 152, "room_id": "!DYgXKezaHgMbiPMzjX:matrix.org", "auth_events": [["$153623842826mEsog:sw1v.org", {"sha256": "uFPD657qXDZ8CmYMVBgHK9RBLfrZhUENhd65biO+550"}], ["$1536241062285137nGFAZ:matrix.org", {"sha256": "B6QKV3pBu9G9+nZOhoCwi8Nn3HB5zOtmiIRJKsy772E"}], ["$153623687814254DeMde:matrix.org", {"sha256": "SRJ3nqJmt+4bwzFz8XGR08jweAd6slOvuSZMKBugK0c"}]], "hashes": {"sha256": "2isnXPIPGGoVLklfMb3ni5v+NuHNa2R/y+MiMwX8gtY"}, "type": "m.room.message"}"#,
        r#"{"origin": "matrix.org", "signatures": {"matrix.org": {"ed25519:auto": "Scqs18VgPqIFah99N8jLAq3qhTuLesHUTBW+LfDEXlpCqVK7HiGm/Y0whQF/F7TXAHzb8MgxaPfhQKZXl8J9BQ"}}, "origin_server_ts": 1536424989199, "sender": "@flux:matrix.org", "event_id": "$1536424989861854NQRKF:matrix.org", "prev_events": [["$153642497554hpAfe:morr.us", {"sha256": "6ea93XsOgM7/UHHFzebl1F+UHI8BCb/q19+1QXuruds"}]], "unsigned": {"age_ts": 1536424989199}, "content": {"body": "on the other hand it's feasible to introduce new permission system room-by-room in the future", "msgtype": "m.text"}, "depth": 151, "room_id": "!DYgXKezaHgMbiPMzjX:matrix.org", "auth_events": [["$153623842826mEsog:sw1v.org", {"sha256": "uFPD657qXDZ8CmYMVBgHK9RBLfrZhUENhd65biO+550"}], ["$1536241062285137nGFAZ:matrix.org", {"sha256": "B6QKV3pBu9G9+nZOhoCwi8Nn3HB5zOtmiIRJKsy772E"}], ["$153623687814254DeMde:matrix.org", {"sha256": "SRJ3nqJmt+4bwzFz8XGR08jweAd6slOvuSZMKBugK0c"}]], "hashes": {"sha256": "N18Q9+KJnAfrUEq/VcHuNL4Y2Ag3/dBxxnRlrkgd+SA"}, "type": "m.room.message"}"#,
        r#"{"origin": "morr.us", "signatures": {"morr.us": {"ed25519:a_BfsF": "CC4awWidyv/ueZP4Bk7gYsicpiXNIO2a/h4B83Agc0PyS0Y+iI6HY4z7ns66K0Sl2LT3PMhNEyq46aE3gH/UDQ"}}, "origin_server_ts": 1536424975964, "sender": "@morranr:morr.us", "event_id": "$153642497554hpAfe:morr.us", "prev_events": [["$153642496553qbGdb:morr.us", {"sha256": "jUMQDTjSnLhQ56AVPHWp4wjCL595LM4jpKn18E3Df8U"}]], "unsigned": {"age_ts": 1536424975964}, "content": {}, "redacts": "$153642496553qbGdb:morr.us", "depth": 150, "room_id": "!DYgXKezaHgMbiPMzjX:matrix.org", "auth_events": [["$153624472111YALFI:morr.us", {"sha256": "xUp1boB5AxAvuNoHN+pWrRDukPe0pOQTmMANRWiz6FQ"}], ["$153623842826mEsog:sw1v.org", {"sha256": "uFPD657qXDZ8CmYMVBgHK9RBLfrZhUENhd65biO+550"}], ["$153623687814254DeMde:matrix.org", {"sha256": "SRJ3nqJmt+4bwzFz8XGR08jweAd6slOvuSZMKBugK0c"}]], "hashes": {"sha256": "8SIpSsbaS/JWaiTNE7FGBSnu1LZ6RsyMOhMUwLWqh/A"}, "type": "m.room.redaction"}"#,
        r#"{"origin": "morr.us", "signatures": {"morr.us": {"ed25519:a_BfsF": "uB73mtRzCaBSwrgwyy9tZxa6YHpZUmM48swa3ceVrD9yZgGIVLHu9omWzhnyhPbb8P1eWEoruHRpb0iPUs/2AQ"}}, "origin_server_ts": 1536424965048, "sender": "@morranr:morr.us", "event_id": "$153642496553qbGdb:morr.us", "prev_events": [["$1536424957861751RGahD:matrix.org", {"sha256": "Dp72DOVZJUPmRqwDqnpUwdSKE1FOnPrXZYWYQnVtuyQ"}]], "unsigned": {"age_ts": 1536424965048}, "content": {"body": "1-4", "msgtype": "m.text"}, "depth": 149, "room_id": "!DYgXKezaHgMbiPMzjX:matrix.org", "auth_events": [["$153624472111YALFI:morr.us", {"sha256": "xUp1boB5AxAvuNoHN+pWrRDukPe0pOQTmMANRWiz6FQ"}], ["$153623842826mEsog:sw1v.org", {"sha256": "uFPD657qXDZ8CmYMVBgHK9RBLfrZhUENhd65biO+550"}], ["$153623687814254DeMde:matrix.org", {"sha256": "SRJ3nqJmt+4bwzFz8XGR08jweAd6slOvuSZMKBugK0c"}]], "hashes": {"sha256": "mH97pmvJxHSJ4S1T1PBQ0KTtQKnvqPwk6kSuEsTiQaw"}, "type": "m.room.message"}"#,
    ];

    let events: Vec<V1Event> = evs
        .into_iter()
        .map(|js| serde_json::from_str(js).unwrap())
        .collect();

    let chunks = DagChunkFragment::from_events(events);

    println!("Created {} chunk(s)", chunks.len());
}
