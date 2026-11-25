#[cfg(target_os = "linux")]
fn main() -> std::io::Result<()> {
    // The trusted stage forks in a separate, single-threaded process.
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() == Some(std::ffi::OsStr::new("--stage")) {
        std::process::exit(atr_judger::stage_main(args));
    }
    atr_judger::serve()
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("atr-judger requires Linux with delegated cgroup v2 controllers.");
    std::process::exit(1);
}
