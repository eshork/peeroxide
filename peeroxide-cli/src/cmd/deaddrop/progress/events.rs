#![allow(dead_code)]

use std::sync::atomic::Ordering;

use serde::Serialize;

use super::state::{Phase, ProgressState};

fn now_rfc3339() -> String {
    use chrono::Utc;

    Utc::now().to_rfc3339()
}

#[derive(Serialize, Debug)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ProgressEvent<'a> {
    Start {
        phase: Phase,
        version: u8,
        filename: &'a str,
        bytes_total: u64,
        indexes_total: u32,
        indexes_done: u32,
        data_total: u32,
        data_done: u32,
        ts: String,
    },
    Progress {
        phase: Phase,
        version: u8,
        filename: &'a str,
        bytes_done: u64,
        bytes_total: u64,
        indexes_done: u32,
        indexes_total: u32,
        data_done: u32,
        data_total: u32,
        rate_bytes_per_sec: f64,
        #[serde(skip_serializing_if = "Option::is_none")]
        eta_seconds: Option<f64>,
        elapsed_seconds: f64,
        ts: String,
    },
    Done {
        phase: Phase,
        version: u8,
        filename: &'a str,
        bytes_done: u64,
        bytes_total: u64,
        indexes_done: u32,
        indexes_total: u32,
        data_done: u32,
        data_total: u32,
        elapsed_seconds: f64,
        ts: String,
    },
    #[serde(rename = "result")]
    PutResult {
        phase: Phase,
        version: u8,
        pickup_key: String,
        bytes: u64,
        chunks: u32,
        ts: String,
    },
    #[serde(rename = "result")]
    GetResult {
        phase: Phase,
        version: u8,
        bytes: u64,
        crc: String,
        output: String,
        ts: String,
    },
    Ack {
        pickup_number: u64,
        peer: String,
        ts: String,
    },
}

pub fn snapshot_start<'a>(state: &'a ProgressState) -> ProgressEvent<'a> {
    ProgressEvent::Start {
        phase: state.phase,
        version: state.version,
        filename: &state.filename,
        bytes_total: state.bytes_total.load(Ordering::Relaxed),
        indexes_total: state.indexes_total.load(Ordering::Relaxed),
        indexes_done: state.indexes_done.load(Ordering::Relaxed),
        data_total: state.data_total.load(Ordering::Relaxed),
        data_done: state.data_done.load(Ordering::Relaxed),
        ts: now_rfc3339(),
    }
}

pub fn snapshot_progress<'a>(state: &'a ProgressState, rate: f64, eta: Option<f64>) -> ProgressEvent<'a> {
    ProgressEvent::Progress {
        phase: state.phase,
        version: state.version,
        filename: &state.filename,
        bytes_done: state.bytes_done.load(Ordering::Relaxed),
        bytes_total: state.bytes_total.load(Ordering::Relaxed),
        indexes_done: state.indexes_done.load(Ordering::Relaxed),
        indexes_total: state.indexes_total.load(Ordering::Relaxed),
        data_done: state.data_done.load(Ordering::Relaxed),
        data_total: state.data_total.load(Ordering::Relaxed),
        rate_bytes_per_sec: rate,
        eta_seconds: eta,
        elapsed_seconds: state.start_instant.elapsed().as_secs_f64(),
        ts: now_rfc3339(),
    }
}

pub fn snapshot_done<'a>(state: &'a ProgressState) -> ProgressEvent<'a> {
    ProgressEvent::Done {
        phase: state.phase,
        version: state.version,
        filename: &state.filename,
        bytes_done: state.bytes_done.load(Ordering::Relaxed),
        bytes_total: state.bytes_total.load(Ordering::Relaxed),
        indexes_done: state.indexes_done.load(Ordering::Relaxed),
        indexes_total: state.indexes_total.load(Ordering::Relaxed),
        data_done: state.data_done.load(Ordering::Relaxed),
        data_total: state.data_total.load(Ordering::Relaxed),
        elapsed_seconds: state.start_instant.elapsed().as_secs_f64(),
        ts: now_rfc3339(),
    }
}

pub fn put_result(
    phase: Phase,
    version: u8,
    pickup_key: String,
    bytes: u64,
    chunks: u32,
) -> ProgressEvent<'static> {
    ProgressEvent::PutResult {
        phase,
        version,
        pickup_key,
        bytes,
        chunks,
        ts: now_rfc3339(),
    }
}

pub fn get_result(
    phase: Phase,
    version: u8,
    bytes: u64,
    crc: String,
    output: String,
) -> ProgressEvent<'static> {
    ProgressEvent::GetResult {
        phase,
        version,
        bytes,
        crc,
        output,
        ts: now_rfc3339(),
    }
}

pub fn ack(pickup_number: u64, peer: String) -> ProgressEvent<'static> {
    ProgressEvent::Ack {
        pickup_number,
        peer,
        ts: now_rfc3339(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use serde_json::Value;

    use super::*;

    fn assert_has_type(json: &str) -> Value {
        let value: Value = serde_json::from_str(json).unwrap();
        assert!(value.get("type").is_some());
        value
    }

    #[test]
    fn serialize_all() {
        let state = ProgressState::new(Phase::Put, 2, Arc::<str>::from("file.txt"));
        state.set_length(4500, 5, 5);
        state.inc_data(900);
        state.inc_index();

        let events = [
            serde_json::to_string(&snapshot_start(&state)).unwrap(),
            serde_json::to_string(&snapshot_progress(&state, 12.5, Some(7.5))).unwrap(),
            serde_json::to_string(&snapshot_done(&state)).unwrap(),
            serde_json::to_string(&put_result(Phase::Put, 2, "pickup".into(), 4500, 5)).unwrap(),
            serde_json::to_string(&get_result(Phase::Get, 2, 4500, "abcd".into(), "stdout".into())).unwrap(),
            serde_json::to_string(&ack(1, "abc".into())).unwrap(),
        ];

        for json in events {
            assert_has_type(&json);
        }
    }

    #[test]
    fn result_variants() {
        let put = serde_json::to_string(&put_result(Phase::Put, 1, "k".into(), 10, 2)).unwrap();
        let get = serde_json::to_string(&get_result(Phase::Get, 1, 10, "crc".into(), "stdout".into())).unwrap();

        let put_v = assert_has_type(&put);
        let get_v = assert_has_type(&get);

        assert_eq!(put_v["type"], "result");
        assert_eq!(get_v["type"], "result");
        assert!(put_v.get("pickup_key").is_some());
        assert!(get_v.get("output").is_some());
    }

    #[test]
    fn omits_none_eta() {
        let state = ProgressState::new(Phase::Get, 2, Arc::<str>::from("file.txt"));
        state.set_length(100, 1, 1);
        state.inc_data(20);
        let json = serde_json::to_string(&ProgressEvent::Progress {
            phase: state.phase,
            version: state.version,
            filename: &state.filename,
            bytes_done: 20,
            bytes_total: 100,
            indexes_done: 0,
            indexes_total: 1,
            data_done: 1,
            data_total: 1,
            rate_bytes_per_sec: 1.0,
            eta_seconds: None,
            elapsed_seconds: 0.0,
            ts: "2026-01-01T00:00:00Z".into(),
        })
        .unwrap();
        let value = assert_has_type(&json);
        assert!(value.get("eta_seconds").is_none());
    }

    #[test]
    fn v1_done_includes_indexes() {
        let state = ProgressState::new(Phase::Put, 1, Arc::<str>::from("file.txt"));
        state.set_length(123, 0, 0);
        state.inc_data(123);
        std::thread::sleep(Duration::from_millis(1));
        let json = serde_json::to_string(&snapshot_done(&state)).unwrap();
        let value = assert_has_type(&json);
        assert_eq!(value["indexes_total"], 0);
    }

    #[test]
    fn ack_natural_fields() {
        let json = serde_json::to_string(&ProgressEvent::Ack {
            pickup_number: 1,
            peer: "abc".into(),
            ts: "2026-01-01T00:00:00Z".into(),
        })
        .unwrap();
        let value = assert_has_type(&json);
        assert_eq!(value["type"], "ack");
        assert!(value.get("pickup_number").is_some());
        assert!(value.get("peer").is_some());
        assert!(value.get("ts").is_some());
        assert!(value.get("phase").is_none());
        assert!(value.get("version").is_none());
        assert!(value.get("indexes_total").is_none());
        assert!(value.get("data_total").is_none());
    }
}
