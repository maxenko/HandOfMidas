//! Integration test: record → anonymize → replay a tiny captured session.
//!
//! Exercises the full Stage-07 library surface: [`Recorder`], [`Anonymizer`],
//! [`Replayer`], and the [`TwsPcap*`] format.

use midas_ib_sim::session::{
    AnonymizeConfig, Anonymizer, Direction, ReplayMode, Replayer, TwsPcapReader,
};
use midas_ib_sim::{Recorder, TwsPcapHeader, TwsPcapWriter};
use tempfile::TempDir;

#[test]
fn full_cycle_record_anonymize_replay_roundtrips() {
    // 1. Write a tiny raw pcap "by hand" so we can test anonymize + replay
    //    without needing a real TWS.
    let dir = TempDir::new().unwrap();
    let raw = dir.path().join("session.tws.pcap");
    let hdr = TwsPcapHeader::new(210, 1_700_000_000_000_000_000);
    {
        let mut w = TwsPcapWriter::create(&raw, hdr).unwrap();
        w.write_record(&midas_ib_sim::TwsPcapRecord::new(
            1_000,
            Direction::ClientToSim,
            b"API\x0076.."
                .iter()
                .chain(b"DU1234567 hi".iter())
                .copied()
                .collect(),
        ))
        .unwrap();
        w.write_record(&midas_ib_sim::TwsPcapRecord::new(
            2_000,
            Direction::SimToClient,
            b"account=DU1234567;exec=0000e1a7.00218745.01".to_vec(),
        ))
        .unwrap();
        w.write_record(&midas_ib_sim::TwsPcapRecord::new(
            3_000,
            Direction::ClientToSim,
            b"bye".to_vec(),
        ))
        .unwrap();
    }

    // 2. Anonymize into a new file using the default (public) salt.
    let anon = dir.path().join("anon.tws.pcap");
    let mut ax = Anonymizer::new(AnonymizeConfig::default());
    let n = ax.process_files(&raw, &anon).unwrap();
    assert_eq!(n, 3);
    // No raw account code survives.
    let recs = TwsPcapReader::open(&anon).unwrap().read_all().unwrap();
    for r in &recs {
        assert!(!r.payload.windows(9).any(|w| w == b"DU1234567"));
    }
    // The two occurrences map to the SAME synthetic code.
    let first_synth = find_du_code(&recs[0].payload).unwrap();
    let second_synth = find_du_code(&recs[1].payload).unwrap();
    assert_eq!(first_synth, second_synth);
    assert!(first_synth.starts_with("DU0000"));

    // 3. Replay the anonymized file in best-effort mode — validates that
    //    the record count, direction order, and header timestamps are intact.
    let file = std::fs::File::open(&anon).unwrap();
    let mut replayer = Replayer::with_reader(file, ReplayMode::BestEffort).unwrap();
    assert_eq!(replayer.header().server_version_neg, 210);

    use midas_ib_sim::session::replayer::ReplayEmission;
    let e1 = replayer.step().unwrap();
    assert!(matches!(
        e1,
        ReplayEmission::ExpectClient {
            ts_nanos: 1_000,
            ..
        }
    ));
    replayer.submit_client_bytes(b"anything").unwrap();

    let e2 = replayer.step().unwrap();
    assert!(matches!(
        e2,
        ReplayEmission::ServerBytes {
            ts_nanos: 2_000,
            ..
        }
    ));

    let e3 = replayer.step().unwrap();
    assert!(matches!(
        e3,
        ReplayEmission::ExpectClient {
            ts_nanos: 3_000,
            ..
        }
    ));
    replayer.submit_client_bytes(b"bye").unwrap();

    assert!(matches!(replayer.step().unwrap(), ReplayEmission::Done));
}

#[test]
fn recorder_and_replayer_agree_on_timestamps_and_order() {
    let dir = TempDir::new().unwrap();
    let stem = dir.path().join("live");
    {
        let mut rec = Recorder::start(&stem, 200, false, None).unwrap();
        rec.record_client_to_sim(b"first").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        rec.record_sim_to_client(b"second").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        rec.record_client_to_sim(b"third").unwrap();
    }
    let mut p = stem.clone();
    p.set_extension("tws.pcap");
    let file = std::fs::File::open(&p).unwrap();

    // IgnoreClient mode — the replayer should surface only the server record
    // and preserve its timestamp relative to start.
    let mut replayer = Replayer::with_reader(file, ReplayMode::IgnoreClient).unwrap();
    use midas_ib_sim::session::replayer::ReplayEmission;
    let e = replayer.step().unwrap();
    match e {
        ReplayEmission::ServerBytes { ts_nanos, bytes } => {
            assert_eq!(bytes, b"second");
            assert!(ts_nanos >= 1_000_000); // at least 1ms since the first write
        }
        other => panic!("expected ServerBytes, got {other:?}"),
    }
    assert!(matches!(replayer.step().unwrap(), ReplayEmission::Done));
}

/// Extract the first `DU\d{7}` substring from a byte slice, as a `String`.
fn find_du_code(bytes: &[u8]) -> Option<String> {
    let s = std::str::from_utf8(bytes).ok()?;
    let i = s.find("DU")?;
    let candidate = &s.as_bytes()[i..];
    if candidate.len() < 9 {
        return None;
    }
    let tail = &candidate[2..9];
    if tail.iter().all(|b| b.is_ascii_digit()) {
        Some(String::from_utf8(candidate[..9].to_vec()).unwrap())
    } else {
        None
    }
}
