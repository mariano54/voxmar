//! The CPU half of the frame-rate benchmark (`tools/bench cpu`): what streaming the world
//! costs without a GPU, so it runs on any machine, the Linux benchmark box included. A
//! player flies at four times flight speed over seed 1 at a view distance of 32 for
//! `FLIGHT` seconds, as Mariano flies, and the run reports how long the server's ticks
//! took (median, p99, worst), how fast columns arrived, and what lighting and meshing
//! cost a column on the client (as `ClientWorld` does them, on one thread).
//!
//! Ignored by default: `cargo test --release -p voxmar_server --test cpu_bench -- --ignored
//! --nocapture`. It prints one line starting `cpu:`, which `tools/bench` reads.

use std::sync::Arc;
use std::time::{Duration, Instant};

use voxmar_engine::coords::{COLUMN_CHUNKS, ColumnPos};
use voxmar_engine::glam::Vec3;
use voxmar_engine::light::LitRegion;
use voxmar_engine::mesh::mesh_chunk;
use voxmar_engine::world::{Column, VoxelWorld};
use voxmar_protocol::{ClientMessage, PROTOCOL_VERSION, PlayerInput, ServerMessage, local_pair};
use voxmar_server::{ServerConfig, load_content, spawn};

/// Seconds of flight, and columns meshed for the meshing cost.
const FLIGHT: f32 = 30.0;
const MESHED: usize = 200;
/// Height to fly at, clear of all but the highest peaks.
const ALTITUDE: f32 = 240.0;

#[test]
#[ignore]
fn streaming_at_four_times_flight_speed() {
    let content = load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).expect("content loads");
    let server = spawn(ServerConfig { seed: 1, view_distance: 32, ..Default::default() }, content.clone()).expect("server starts");
    let (connection, server_side) = local_pair();
    server.connections().send(server_side).expect("server takes the connection");
    connection.send(ClientMessage::Hello { name: "bench".into(), protocol: PROTOCOL_VERSION, content: content.fingerprint(), admin_token: None });
    connection.send(ClientMessage::Settings { view_distance: 32, fly_speed: 4.0 });

    let mut world = VoxelWorld::default();
    let mut position = None;
    let mut columns = 0usize;
    let speed = content.movement.fly_sprint_speed * 4.0 * 0.9;
    let (mut started, mut last_input, mut teleports) = (None::<Instant>, Instant::now(), 0);
    loop {
        while let Ok(Some(message)) = connection.try_recv() {
            match message {
                ServerMessage::Welcome { position: at, .. } => position = Some(at.with_y(ALTITUDE)),
                ServerMessage::Column { pos, column } => {
                    columns += started.is_some() as usize;
                    world.insert(pos, column);
                }
                ServerMessage::Unload { pos } => {
                    world.remove(pos);
                }
                // Flying on regardless, as before, but echoing it so the inputs count.
                ServerMessage::Teleport { seq, .. } => teleports = seq,
                _ => {}
            }
        }
        let Some(at) = position.as_mut() else { continue };
        let started = *started.get_or_insert_with(Instant::now);
        if started.elapsed().as_secs_f32() > FLIGHT {
            break;
        }
        // Inputs at the server's 20 a second, flying along +x.
        let dt = last_input.elapsed().as_secs_f32();
        if dt >= 0.05 {
            *at += Vec3::X * speed * dt;
            last_input = Instant::now();
            connection.send(ClientMessage::Input(PlayerInput { position: *at, yaw: -std::f32::consts::FRAC_PI_2, pitch: 0.0, selected_slot: 0, flying: true, teleport: teleports }));
        }
        std::thread::sleep(Duration::from_millis(1));
    }

    let mut ticks: Vec<f32> = server.tick_times().iter().map(|t| t.as_secs_f32() * 1000.0).collect();
    ticks.sort_by(f32::total_cmp);
    let at = |q: f32| ticks[((ticks.len() - 1) as f32 * q) as usize];

    // Lighting and meshing, as the client does them for every column that arrives.
    let registry = content.blocks.clone();
    let neighbourhoods: Vec<[Arc<Column>; 9]> = world
        .columns()
        .filter_map(|(&pos, _)| {
            let around: Option<Vec<Arc<Column>>> = (-1..=1).flat_map(|dz| (-1..=1).map(move |dx| pos.offset(dx, dz))).map(|p: ColumnPos| world.column(p).cloned()).collect();
            around?.try_into().ok()
        })
        .take(MESHED)
        .collect();
    let meshing = Instant::now();
    for columns in &neighbourhoods {
        let region = LitRegion::compute(&registry, columns);
        for cy in 0..COLUMN_CHUNKS {
            std::hint::black_box(mesh_chunk(&registry, &region, &columns[4], cy, 1));
        }
    }
    let mesh_ms = meshing.elapsed().as_secs_f32() * 1000.0 / neighbourhoods.len().max(1) as f32;

    println!(
        "cpu: {} ticks, median {:.2} ms, p99 {:.2} ms, max {:.2} ms; {:.0} columns/s streamed; {mesh_ms:.2} ms to light and mesh a column ({} columns)",
        ticks.len(),
        at(0.5),
        at(0.99),
        at(1.0),
        columns as f32 / FLIGHT,
        neighbourhoods.len(),
    );
    assert!(columns > 0, "no columns arrived");
}
