use std::io::Write;

/// Initializes logging so that only the raw business message is printed,\n/// keeping the Rust logs visually close to the Java ones.
pub fn init_logging() {
    let env = env_logger::Env::default().default_filter_or("info");
    let mut builder = env_logger::Builder::from_env(env);
    builder.format(|buf, record| writeln!(buf, "{}", record.args()));
    let _ = builder.try_init();
}
