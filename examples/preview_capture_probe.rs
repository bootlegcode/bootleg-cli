//! Local-only acceptance probe; never contacts discovery or exposes file contents.
#[cfg(unix)]
fn main() {
    use bootleg_cli::{preview_application::SourceCapture, preview_capture::Capture};
    use std::time::{Duration, Instant};
    let args: Vec<_> = std::env::args().collect();
    assert_eq!(args.len(), 3, "usage: preview_capture_probe ROOT BASELINE");
    let baseline = args[2].parse().expect("full baseline");
    let result =
        Capture::new(args[1].clone().into(), "/usr/bin/git".into()).and_then(|mut capture| {
            capture.capture(&[baseline], Instant::now() + Duration::from_secs(120))
        });
    match result {
        Ok(_) => println!("captured"),
        Err(code) => {
            eprintln!("{code:?}");
            std::process::exit(2);
        }
    }
}
#[cfg(not(unix))]
fn main() {
    std::process::exit(2);
}
