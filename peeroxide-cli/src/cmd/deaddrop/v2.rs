use super::*;

pub async fn run_put(args: &PutArgs, cfg: &ResolvedConfig) -> i32 {
    super::v1::run_put(args, cfg).await
}

pub async fn get_from_root(
    _root_data: Vec<u8>,
    _root_pk: [u8; 32],
    _handle: HyperDhtHandle,
    _task_handle: tokio::task::JoinHandle<Result<(), peeroxide_dht::hyperdht::HyperDhtError>>,
    _args: &GetArgs,
) -> i32 {
    eprintln!("error: v2 dead drop format not yet implemented");
    1
}
