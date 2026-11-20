//! Real file output, including a separately compiled release panic=abort child.
#![allow(clippy::expect_used, reason = "regression assertions fail loudly")]

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{SystemTime, UNIX_EPOCH},
};

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("lumakit-panic-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&path).expect("create scratch directory");
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if std::thread::panicking() {
            use std::io::Write as _;
            let _ = writeln!(
                std::io::stderr(),
                "panic regression artifacts retained: {}",
                self.0.display()
            );
            return;
        }
        fs::remove_dir_all(&self.0).expect("remove scratch directory");
    }
}

fn build_child(target: &Path, abort: bool) -> PathBuf {
    let mut command = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    command
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args([
            "rustc",
            "--locked",
            "-p",
            "lumakit-observability",
            "--example",
            "panic_child",
            "--target-dir",
        ])
        .arg(target);
    if abort {
        command.arg("--release");
    }
    command
        .arg("--")
        .args(["-C", if abort { "panic=abort" } else { "panic=unwind" }]);
    let output = command.output().expect("build isolated panic child");
    assert!(
        output.status.success(),
        "child build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    target
        .join(if abort { "release" } else { "debug" })
        .join("examples")
        .join(format!("panic_child{}", std::env::consts::EXE_SUFFIX))
}

fn assert_panic_output(dir: &Path, output: &Output, abort: bool, custom: bool) {
    assert!(
        !output.status.success(),
        "panic child must terminate unsuccessfully"
    );
    let text = fs::read_to_string(dir.join("app.log")).expect("read actual panic log");
    let trace = fs::read_to_string(dir.join("trace.txt")).expect("read expected trace");
    let panic_lines: Vec<_> = text
        .lines()
        .filter(|line| line.contains("code:RUST_PANIC"))
        .collect();
    assert_eq!(panic_lines.len(), 1, "{text}");
    let line = panic_lines[0];
    let parsed = lumakit_observability::parse_line(line);
    assert_eq!(parsed.level, "ERROR");
    assert_eq!(parsed.trace_id.as_deref(), Some(trace.as_str()));
    assert_eq!(parsed.operation.as_deref(), Some("panic.fixture.child"));
    assert!(
        line.contains("ParentSpanId:") && !line.contains("ParentSpanId:-"),
        "{line}"
    );
    let summary = if custom {
        "summary:non_string_payload"
    } else {
        "summary:redacted_string_payload"
    };
    assert!(line.contains(summary), "{line}");
    assert!(line.contains("thread_id:ThreadId("), "{line}");
    let panic_file = line
        .split('|')
        .find_map(|segment| segment.strip_prefix("panic_file:"))
        .expect("panic source file field");
    assert!(!panic_file.is_empty(), "{line}");
    assert!(
        !panic_file.starts_with(['/', '\\'])
            && !panic_file.contains(':')
            && panic_file
                .split(['/', '\\'])
                .all(|part| !matches!(part, "" | "." | "..")),
        "panic source file must be repository-relative or a filename: {line}"
    );
    assert!(
        line.contains("panic_line:") && line.contains("panic_column:"),
        "{line}"
    );
    assert!(
        line.contains("backtrace:") && line.contains("panic_child"),
        "{line}"
    );
    assert!(
        !text.contains("panic.fixture.filtered"),
        "Error filter did not apply"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    for secret in [
        "SYNTHETIC_PANIC_SECRET",
        "SYNTHETIC_NON_STRING_SECRET",
        "PRIVATE_BODY",
        "PRIVATE_USER",
        "PAYLOAD_PATH",
        env!("CARGO_MANIFEST_DIR"),
    ] {
        assert!(!text.contains(secret), "panic file leaked {secret}");
        assert!(!stderr.contains(secret), "stderr leaked {secret}");
    }
    if !custom {
        assert!(
            line.contains(&format!(
                "payload_hash:{}",
                lumakit_observability::diagnostic_hash(
                    "SYNTHETIC_PANIC_SECRET body=PRIVATE_BODY C:/PRIVATE_USER/PAYLOAD_PATH"
                )
            )),
            "{line}"
        );
    }
    let normalized = text.replace('\\', "/");
    assert!(
        !normalized.contains(&env!("CARGO_MANIFEST_DIR").replace('\\', "/")),
        "absolute source path leaked"
    );
    assert_eq!(
        dir.join("unwound.txt").exists(),
        !abort,
        "unwind/abort semantics changed"
    );
}

#[test]
fn panic_is_private_correlated_and_synchronous_for_unwind_and_release_abort() {
    let scratch = Scratch::new();
    // A separate target directory avoids inheriting the running test harness's panic strategy.
    let target = scratch.0.join("target");
    for abort in [false, true] {
        let child = build_child(&target, abort);
        for custom in [false, true] {
            let dir = scratch.0.join(format!("abort-{abort}-custom-{custom}"));
            let mut command = Command::new(&child);
            command.arg(&dir);
            if custom {
                command.arg("custom");
            }
            let output = command.output().expect("run isolated panic child");
            assert_panic_output(&dir, &output, abort, custom);
        }
    }
}
