//! The only test in its binary: it sets and reads the process's streaming limit
//! (`voxmar_engine::jobs`), which every server in a process says to, so no other server may
//! run beside it.

use std::time::{Duration, Instant};

use voxmar_engine::jobs;
use voxmar_protocol::{ClientConnection, ClientMessage, PROTOCOL_VERSION, ServerMessage, local_pair};
use voxmar_server::{ServerConfig, load_content};

/// Only against a server that hangs: every wait below ends on what the server says.
const SERVER_WAIT: Duration = Duration::from_secs(600);

/// Takes the server's messages until one `is` what is waited for.
fn wait_for(connection: &ClientConnection, watchdog: Instant, what: &str, is: impl Fn(&ServerMessage) -> bool) {
    loop {
        match connection.try_recv().expect("the server keeps the connection") {
            Some(message) if is(&message) => return,
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(1)),
        }
        assert!(Instant::now() < watchdog, "the server never sent {what}");
    }
}

/// A server stopped while its player's land is missing leaves the streaming limit as it
/// found it: a later world in the process, on a server elsewhere, which never says whether
/// land is missing, would otherwise stream unlimited throughout.
#[test]
fn a_stopped_server_leaves_no_land_missing() {
    let content = load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).expect("content loads");
    let hello = ClientMessage::Hello { name: "test".into(), protocol: PROTOCOL_VERSION, content: content.fingerprint(), admin_token: None };
    let server = voxmar_server::spawn(ServerConfig { view_distance: 2, ..Default::default() }, content).unwrap();
    let watchdog = Instant::now() + SERVER_WAIT;
    let (connection, server_side) = local_pair();
    server.connections().send(server_side).unwrap();
    assert!(connection.send(hello));
    wait_for(&connection, watchdog, "a welcome", |m| matches!(m, ServerMessage::Welcome { .. }));
    jobs::set_limit(4);
    // Far beyond the land generated yet (the local player is an admin): the teleport is
    // made before the next tick, which finds the land round it missing.
    assert!(connection.send(ClientMessage::Command { line: "/tp 40000 150 40000".into() }));
    wait_for(&connection, watchdog, "the teleport", |m| matches!(m, ServerMessage::Teleport { .. }));
    let teleported = server.ticks();
    while server.ticks() <= teleported {
        assert!(Instant::now() < watchdog, "the server stopped ticking");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(jobs::limit(), 0, "no limit while the land round the player is missing");
    drop(server);
    assert_eq!(jobs::limit(), 4, "the limit back once the server stopped");
}
