use tracing_subscriber::EnvFilter;

/// Initialize tracing to stderr. --trace implies --debug (spec 3.2).
pub fn init(debug: bool, trace: bool) {
    let level = if trace {
        "trace"
    } else if debug {
        "debug"
    } else {
        "warn"
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(level))
        .with_writer(std::io::stderr)
        .try_init();
}
