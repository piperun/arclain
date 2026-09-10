//! A test-only executable also acts as the deliberately broken worker. It is
//! never a normal Cargo binary or a bundled production helper.
use arclain_archive::{ArchiveClient, Cancellation, ImportError, WorkerOptions};
use std::{
    io::{Read, Write},
    time::{Duration, Instant},
};
use unboxer::{Container, EntryName, ObjectKind, PackedBlock, PhysicalEntry, SequentialArchive};
fn frame(out: &mut dyn Write, tag: u8, data: &[u8]) {
    out.write_all(&[tag]).unwrap();
    out.write_all(&(data.len() as u32).to_le_bytes()).unwrap();
    out.write_all(data).unwrap();
    out.flush().unwrap();
}
fn json(out: &mut dyn Write, tag: u8, value: &impl serde::Serialize) {
    frame(out, tag, &serde_json::to_vec(value).unwrap());
}
fn receive(input: &mut dyn Read) -> (u8, Vec<u8>) {
    let mut h = [0; 5];
    input.read_exact(&mut h).unwrap();
    let mut b = vec![0; u32::from_le_bytes(h[1..].try_into().unwrap()) as usize];
    input.read_exact(&mut b).unwrap();
    (h[0], b)
}
fn broken_worker() {
    let mut input = std::io::stdin().lock();
    let mut out = std::io::stdout().lock();
    frame(&mut out, 1, b"arclain-import-v1");
    let (_, request) = receive(&mut input);
    let request: serde_json::Value = serde_json::from_slice(&request).unwrap();
    let scenario = request["source"].as_str().unwrap();
    if scenario == "hang" {
        std::thread::sleep(Duration::from_secs(60));
        return;
    }
    json(&mut out, 3, &Container::Zip);
    json(
        &mut out,
        4,
        &PackedBlock {
            packed_bytes: 3,
            unpacked_bytes: 3,
        },
    );
    json(
        &mut out,
        5,
        &PhysicalEntry {
            index: 0,
            name: EntryName::Unicode("file".into()),
            kind: ObjectKind::File,
            size: 3,
            block: Some(0),
            encrypted: false,
            method: "copy".into(),
            method_supported: true,
            unix_mode: None,
        },
    );
    frame(&mut out, 6, &1u64.to_le_bytes());
    receive(&mut input);
    if scenario == "bad-index" {
        frame(&mut out, 8, &9u64.to_le_bytes());
        return;
    }
    frame(&mut out, 8, &0u64.to_le_bytes());
    if scenario == "oversized" {
        out.write_all(&[9, 1, 0, 1, 0]).unwrap();
        out.flush().unwrap();
        return;
    }
    if scenario == "truncated" {
        out.write_all(&[9, 3, 0, 0, 0, b'a']).unwrap();
        out.flush().unwrap();
        return;
    }
    if scenario == "sink-drop" {
        loop {
            frame(&mut out, 9, &[1; 64 << 10]);
        }
    }
    frame(&mut out, 9, b"abc");
    frame(&mut out, 10, &0u64.to_le_bytes());
    if scenario != "missing-complete" {
        frame(
            &mut out,
            11,
            if scenario == "bad-complete" {
                b"unexpected"
            } else {
                b""
            },
        );
    }
}
fn options() -> WorkerOptions {
    WorkerOptions::new(std::env::current_exe().unwrap())
}
fn limits() -> unboxer::Limits {
    unboxer::Limits::new(10, 100, 100, 100)
}
fn main() {
    if std::env::args().nth(1).as_deref() == Some("--memory-limit") {
        broken_worker();
        return;
    }
    for scenario in [
        "truncated",
        "oversized",
        "missing-complete",
        "bad-complete",
        "bad-index",
    ] {
        let mut session = ArchiveClient::new(options())
            .open(
                std::path::Path::new(scenario),
                limits(),
                Cancellation::default(),
            )
            .unwrap();
        let result = (|| {
            while let Some(mut e) = session.next_entry()? {
                std::io::copy(&mut e.reader, &mut std::io::sink())?;
            }
            Ok::<(), unboxer::UnboxerError>(())
        })();
        assert!(result.is_err(), "accepted {scenario}");
        assert!(session.take_error().is_some(), "lost {scenario} cause");
    }
    let mut config = options();
    config.timeout = Duration::from_millis(200);
    assert!(matches!(
        ArchiveClient::new(config).open(
            std::path::Path::new("hang"),
            limits(),
            Cancellation::default()
        ),
        Err(ImportError::TimedOut)
    ));
    let started = Instant::now();
    let mut session = ArchiveClient::new(options())
        .open(
            std::path::Path::new("sink-drop"),
            limits(),
            Cancellation::default(),
        )
        .unwrap();
    let mut entry = session.next_entry().unwrap().unwrap();
    entry.reader.read_exact(&mut [0; 1]).unwrap();
    drop(entry);
    drop(session);
    assert!(started.elapsed() < Duration::from_secs(2));
    println!("7 client protocol/deadline/sink-abandonment cases passed");
}
