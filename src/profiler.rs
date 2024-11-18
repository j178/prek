use tracing::error;

/// Creates a profiler guard and returns it.
pub(crate) fn start_profiling() -> Option<pprof::ProfilerGuard<'static>> {
    match pprof::ProfilerGuardBuilder::default()
        .frequency(1000)
        .blocklist(&["libc", "libgcc", "pthread", "vdso"])
        .build()
    {
        Ok(guard) => Some(guard),
        Err(e) => {
            error!("Failed to build profiler guard: {e}");
            None
        }
    }
}

/// Reports the profiling results.
pub(crate) fn finish_profiling(profiler_guard: Option<pprof::ProfilerGuard>) {
    match profiler_guard
        .expect("Failed to retrieve profiler guard")
        .report()
        .build()
    {
        Ok(report) => {
            #[cfg(feature = "profiler-flamegraph")]
            {
                let random = rand::random::<u64>();
                let file = fs_err::File::create(format!(
                    "{}.{random}.flamegraph.svg",
                    env!("CARGO_PKG_NAME"),
                ))
                .expect("Failed to create flamegraph file");
                if let Err(e) = report.flamegraph(file) {
                    error!("failed to create flamegraph file: {e}");
                }
            }

            #[cfg(not(feature = "profiler-flamegraph"))]
            {
                info!("profiling report: {:?}", &report);
            }
        }
        Err(e) => {
            error!("Failed to build profiler report: {e}");
        }
    }
}
