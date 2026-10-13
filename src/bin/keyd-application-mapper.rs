use keyd::{appmapper, runtime};
use std::io::{self, Write};
use std::os::fd::AsFd;

fn main() {
    let result = runtime::install_signals().and_then(|()| appmapper::run());
    let status = if runtime::shutdown_signal() != 0 {
        runtime::shutdown_signal()
    } else {
        match result {
            Ok(status) => status,
            Err(error) => {
                let stderr = io::stderr();
                if let Ok(mut stderr) = runtime::FdIo::new(stderr.as_fd()) {
                    let _ = writeln!(stderr, "ERROR: {error}");
                }
                if error.kind() == io::ErrorKind::InvalidInput {
                    2
                } else {
                    1
                }
            }
        }
    };
    std::process::exit(if runtime::shutdown_signal() != 0 {
        runtime::shutdown_signal()
    } else {
        status
    });
}
