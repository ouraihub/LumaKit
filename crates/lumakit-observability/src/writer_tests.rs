#![allow(clippy::unwrap_used, reason = "regression assertions fail loudly")]

use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("lumakit-writer-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn rotation_failure_is_retained_by_flush_without_logging_recursively() {
    let scratch = Scratch::new();
    let path = scratch.0.join("app.log");
    let mut writer = SharedWriter::open(path.clone(), 4).unwrap();
    writer.write_all(b"old\n").unwrap();
    fs::create_dir(path.with_extension("log.1")).unwrap();
    let error = writer.write_all(b"new\n").unwrap_err();
    let kind = error.kind();
    assert_eq!(writer.clone().flush().unwrap_err().kind(), kind);
    assert_eq!(fs::read(&path).unwrap(), b"old\n");
    fs::remove_dir(path.with_extension("log.1")).unwrap();
    // No automatic retry after a failed write; the original failure remains observable.
    assert_eq!(writer.flush().unwrap_err().kind(), kind);
    drop(writer);
}

#[test]
fn rotation_keeps_latest_complete_records_and_flushes_real_output() {
    let scratch = Scratch::new();
    let path = scratch.0.join("app.log");
    let mut writer = SharedWriter::open(path.clone(), 4).unwrap();
    writer.write_all(b"one\n").unwrap();
    writer.write_all(b"two\n").unwrap();
    writer.write_all(b"end\n").unwrap();
    writer.flush().unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"end\n");
    assert_eq!(fs::read(path.with_extension("log.1")).unwrap(), b"two\n");
    drop(writer);
}

#[test]
fn invalid_or_unreadable_level_configuration_is_not_defaulted() {
    let scratch = Scratch::new();
    let level = scratch.0.join("level.json");
    fs::write(&level, br#"{"level":"broken"}"#).unwrap();
    assert!(matches!(
        crate::LogHandle::init(&scratch.0, crate::LogLevel::Info, 1024),
        Err(crate::LogError::Json(_))
    ));
    fs::remove_file(&level).unwrap();
    fs::create_dir(&level).unwrap();
    assert!(matches!(
        crate::LogHandle::init(&scratch.0, crate::LogLevel::Info, 1024),
        Err(crate::LogError::Io(_))
    ));
}
