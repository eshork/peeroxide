//! Dead Drop v3 (ships under wire-byte 0x02).
//!
//! Tree-indexed storage protocol: the index layer is a tree of mutable
//! signed records (instead of v2-original's linked list); the data layer
//! is a flat collection of immutable, content-addressed records, each
//! carrying a per-deaddrop salt for DHT address-space isolation.
//!
//! See `peeroxide-cli/DEADDROP_V3.md` (or `DEADDROP_V2.md` once landed)
//! for the wire-format specification.

#![allow(dead_code)]

pub mod build;
pub mod fetch;
pub mod keys;
pub mod need;
pub mod publish;
pub mod stream;
pub mod tree;
pub mod wire;

use super::{GetArgs, PutArgs};
use crate::cmd::deaddrop::progress::reporter::ProgressReporter;
use crate::cmd::deaddrop::progress::state::ProgressState;
use crate::config::ResolvedConfig;
use peeroxide_dht::hyperdht::HyperDhtHandle;
use std::sync::Arc;

#[allow(unused_imports)]
pub use wire::VERSION;

/// Concurrency cap shared between fetch and put pipelines.
pub const PARALLEL_FETCH_CAP: usize = 64;

/// PUT entry point: dispatched from `cmd::deaddrop::run_put` when the
/// user's command is `dd put` and `--v1` is not set.
pub async fn run_put(args: &PutArgs, cfg: &ResolvedConfig) -> i32 {
    publish::run_put(args, cfg).await
}

/// GET entry point: dispatched from `cmd::deaddrop::run_get` when the
/// fetched root chunk's first byte is `0x02`.
#[allow(clippy::too_many_arguments)]
pub async fn get_from_root(
    root_data: Vec<u8>,
    root_pk: [u8; 32],
    handle: HyperDhtHandle,
    task_handle: tokio::task::JoinHandle<
        Result<(), peeroxide_dht::hyperdht::HyperDhtError>,
    >,
    args: &GetArgs,
    progress: Arc<ProgressState>,
    reporter: ProgressReporter,
) -> i32 {
    fetch::get_from_root(
        root_data,
        root_pk,
        handle,
        task_handle,
        args,
        progress,
        reporter,
    )
    .await
}
