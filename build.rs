// Build-time metadata only. No C compilation, linking, or runtime Git dependency.
use std::env;
use std::path::Path;
use std::process::Command;

fn git(arguments: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new("git")
        .args(arguments)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

fn setting(key: &str, default: &str) -> String {
    println!("cargo:rerun-if-env-changed={key}");
    let value = match env::var(key) {
        Ok(value) => value,
        Err(env::VarError::NotPresent) => default.to_owned(),
        Err(error) => panic!("invalid {key}: {error}"),
    };
    assert!(!value.contains(['\n', '\r', '\0']), "invalid {key}");
    println!("cargo:rustc-env={key}={value}");
    value
}

fn main() {
    env::set_current_dir(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"))
        .expect("enter package directory");
    for path in ["build.rs", "Cargo.toml", "src", "t"] {
        println!("cargo:rerun-if-changed={path}");
    }
    if Path::new(".git").exists() {
        println!("cargo:rerun-if-changed=.git");
    }
    // --git-path resolves both normal repositories and linked worktrees.
    for name in ["HEAD", "index", "packed-refs", "refs"] {
        if let Some(bytes) = git(&["rev-parse", "--git-path", name]) {
            let path = String::from_utf8(bytes).expect("Git metadata path must be UTF-8");
            let path = path.trim_end_matches('\n');
            assert!(!path.contains(['\n', '\r']), "invalid Git metadata path");
            if Path::new(path).exists() {
                println!("cargo:rerun-if-changed={path}");
            }
        }
    }
    if let Some(bytes) = git(&["rev-parse", "--git-common-dir"]) {
        let path = String::from_utf8(bytes).expect("Git common directory must be UTF-8");
        let path = path.trim_end_matches('\n');
        assert!(!path.contains(['\n', '\r']), "invalid Git common directory");
        println!("cargo:rerun-if-changed={path}");
    }
    if let Some(bytes) = git(&["ls-files", "-z"]) {
        for path in bytes
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
        {
            let path = std::str::from_utf8(path).expect("tracked path must be UTF-8");
            assert!(!path.contains(['\n', '\r']), "invalid tracked path");
            println!("cargo:rerun-if-changed={path}");
        }
    }
    let commit = git(&[
        "describe",
        "--no-match",
        "--always",
        "--abbrev=7",
        "--dirty",
    ])
    .and_then(|bytes| String::from_utf8(bytes).ok())
    .map(|value| value.trim_end_matches('\n').to_owned())
    .filter(|value| !value.is_empty() && !value.contains(['\n', '\r', '\0']))
    .unwrap_or_else(|| "unknown".to_owned());
    setting(
        "KEYD_BUILD_VERSION",
        &format!(
            "v{} ({commit})",
            env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION")
        ),
    );
    let prefix = setting("KEYD_PREFIX", "/usr/local");
    setting("KEYD_CONFIG_DIR", "/etc/keyd");
    setting("KEYD_SOCKET_PATH", "/var/run/keyd.socket");
    setting("KEYD_DATA_DIR", &format!("{prefix}/share/keyd"));
    let backend = setting("KEYD_VKBD", "uinput");
    assert!(
        matches!(backend.as_str(), "uinput" | "usb-gadget" | "stdout"),
        "invalid KEYD_VKBD: {backend}"
    );
    println!(
        "cargo:rustc-check-cfg=cfg(keyd_backend, values(\"uinput\", \"usb-gadget\", \"stdout\"))"
    );
    println!("cargo:rustc-cfg=keyd_backend=\"{backend}\"");
}
