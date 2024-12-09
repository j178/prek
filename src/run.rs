use std::cmp::max;
use std::future::Future;
use std::sync::Arc;

use tokio::task::JoinSet;
use tracing::trace;

use crate::hook::Hook;

fn target_concurrency(serial: bool) -> usize {
    if serial || std::env::var_os("PRE_COMMIT_NO_CONCURRENCY").is_some() {
        1
    } else {
        std::thread::available_parallelism()
            .map(std::num::NonZero::get)
            .unwrap_or(1)
    }
}

// TODO: do a more accurate calculation
fn partitions<'a>(
    hook: &'a Hook,
    filenames: &'a [&String],
    concurrency: usize,
) -> Vec<Vec<&'a String>> {
    // If there are no filenames, we still want to run the hook once.
    if filenames.is_empty() {
        return vec![vec![]];
    }

    let max_per_batch = max(4, filenames.len().div_ceil(concurrency));
    // TODO: subtract the env size
    let max_cli_length = if cfg!(unix) {
        1 << 12
    } else {
        (1 << 15) - 2048 // UNICODE_STRING max - headroom
    };

    let command_length =
        hook.entry.len() + hook.args.iter().map(String::len).sum::<usize>() + hook.args.len();

    let mut partitions = Vec::new();
    let mut current = Vec::new();
    let mut current_length = command_length + 1;

    for &filename in filenames {
        let length = filename.len() + 1;
        if current_length + length > max_cli_length || current.len() >= max_per_batch {
            partitions.push(current);
            current = Vec::new();
            current_length = command_length + 1;
        }
        current.push(filename);
        current_length += length;
    }

    if !current.is_empty() {
        partitions.push(current);
    }

    partitions
}

pub async fn run_by_batch<T, F, Fut>(
    hook: &Hook,
    filenames: &[&String],
    run: F,
) -> anyhow::Result<Vec<T>>
where
    F: Fn(Vec<String>) -> Fut,
    F: Clone + Send + Sync + 'static,
    Fut: Future<Output = anyhow::Result<T>> + Send + 'static,
    T: Send + 'static,
{
    let mut concurrency = target_concurrency(hook.require_serial);

    // Split files into batches
    let partitions = partitions(hook, filenames, concurrency);
    concurrency = concurrency.min(partitions.len());
    let semaphore = Arc::new(tokio::sync::Semaphore::new(concurrency));
    trace!(
        total_files = filenames.len(),
        partitions = partitions.len(),
        concurrency = concurrency,
        "Running {}",
        hook.id,
    );

    let run = Arc::new(run);

    // Spawn tasks for each batch
    let mut tasks = JoinSet::new();

    for batch in partitions {
        let semaphore = semaphore.clone();
        let run = run.clone();

        let batch: Vec<_> = batch.into_iter().map(ToString::to_string).collect();

        tasks.spawn(async move {
            let _permit = semaphore
                .acquire()
                .await
                .map_err(|_| anyhow::anyhow!("Failed to acquire semaphore"))?;

            run(batch).await
        });
    }

    let mut results = Vec::new();
    while let Some(result) = tasks.join_next().await {
        results.push(result??);
    }

    Ok(results)
}
