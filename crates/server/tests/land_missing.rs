//! The only test in its binary: it sets and reads the process's streaming limit
//! (`voxmar_engine::jobs`), which every server in a process says to, so no other server may
//! run beside it.

use std::time::{Duration, Instant};

use voxmar_engine::jobs;
use voxmar_protocol::{ClientMessage, PROTOCOL_VERSION, local_pair};
use voxmar_server::{ServerConfig, load_content};

/// A server stopped while its player's land is missing leaves the streaming limit as it
/// found it: a later world in the process, on a server elsewhere, which never says whether
/// land is missing, would otherwise stream unlimited throughout.
#[test]
fn a_stopped_server_leaves_no_land_missing() {
    let content = load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).expect("content loads");
    let hello = ClientMessage::Hello { name: "test".into(), protocol: PROTOCOL_VERSION, content: content.fingerprint(), admin_token: None };
    let server = voxmar_server::spawn(ServerConfig { view_distance: 2, ..Default::default() }, content).unwrap();
    let (connection, server_side) = local_pair();
    server.connections().send(server_side).unwrap();
    assert!(connection.send(hello));
    jobs::set_limit(4);
    // Far beyond the land generated yet: missing until it has come.
    assert!(connection.send(ClientMessage::Command { line: "/tp 40000 150 40000".into() }));
    // Only against a hang: the server says so on its next tick.
    let deadline = Instant::now() + Duration::from_secs(600);
    while jobs::limit() != 0 {
        assert!(Instant::now() < deadline, "the land never went missing");
        std::thread::sleep(Duration::from_millis(1));
    }
    drop(server);
    assert_eq!(jobs::limit(), 4, "the limit back once the server stopped");
}
