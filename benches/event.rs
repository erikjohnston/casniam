#![feature(test)]

extern crate test;

use casniam::protocol::events::v3::SignedEventV3;

use test::Bencher;

#[bench]
fn deserialize(b: &mut Bencher) {
    let event = r#"{"auth_events":["$NgXjYGcKkvTYxqDOTUo13Ff9DVlBwDN0SEtNUGVEUIA","$JH7fJVdg9YOUCpXd52aS5KJYo4fno8tHP873phgtPwQ","$UzdqUOWoPZgDHNhy00L31XEgkdBX1qobzhSun3kwzSI"],"content":{"body":"[matrix-org/synapse] Half-Shot synchronize pull request #7785: Add /user/{user_id}/shared_rooms/ api [open] to Half-Shot - https://github.com/matrix-org/synapse/pull/7785","format":"org.matrix.custom.html","formatted_body":"[<u>matrix-org/synapse</u>] Half-Shot synchronize <b>pull request #7785</b>: Add /user/{user_id}/shared_rooms/ api [open] to Half-Shot - https://github.com/matrix-org/synapse/pull/7785","msgtype":"m.notice"},"depth":26010,"hashes":{"sha256":"Pi9VQxo1Dbxnx603ME8+C+F8gm9cwJhXOovujLSF6is"},"origin":"matrix.org","origin_server_ts":1598691790166,"prev_events":["$2MXPH0RwSPoXZJMVikhvVuPc1wOO74-Gn5_7xJWM6mM","$czUJ0gaAstVHkfEC7htLZW0gzIve0-2nMhVdkNEzhUU"],"prev_state":[],"room_id":"!XaqDhxuTIlvldquJaV:matrix.org","sender":"@_neb_github_=40travis=3at2l.io:matrix.org","type":"m.room.message","signatures":{"matrix.org":{"ed25519:a_RXGa":"pb4h1GfeZrsvYPeX+sfLORhWuoVYL0MV6NLJNv4g8ld8VgQEo8nBTjmVT8MlWN92Fw08KNXryZijHd9Z3yHFBw"}},"unsigned":{"age_ts":1598691790166}}"#;

    b.iter(|| serde_json::from_str::<SignedEventV3>(event).unwrap())
}
