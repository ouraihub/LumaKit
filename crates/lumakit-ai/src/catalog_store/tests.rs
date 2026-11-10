use super::*;
use crate::{Protocol, catalog::EndpointEntry};
use lumakit_support::secrets::SavedKey;

mod labels;

use std::time::{SystemTime, UNIX_EPOCH};

fn root(name: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let path = std::env::temp_dir().join(format!("lumakit-catalog-{name}-{stamp}"));
    std::fs::create_dir_all(&path)?;
    Ok(path)
}

fn provider(id: &str, key: Option<SavedKey>) -> ProviderEntry {
    let selected_key = key.as_ref().map(|k| k.id.clone());
    ProviderEntry {
        id: id.into(),
        name: "Provider".into(),
        enabled: true,
        endpoints: vec![EndpointEntry {
            id: format!("{id}-endpoint"),
            name: "Endpoint".into(),
            base_url: "https://example.com/v1".into(),
            protocol: Protocol::OpenAiChat,
            keys: key.into_iter().collect(),
            models: Vec::new(),
            selected_key,
            selected_model: None,
        }],
    }
}

#[test]
fn empty_database_does_not_resurrect_json_after_marker() -> Result<(), Box<dyn std::error::Error>> {
    let path = root("empty")?;
    let json =
        serde_json::json!({"providers": [{"id":"old","name":"Old","enabled":true,"endpoints":[]}]});
    lumakit_support::save_versioned(&path.join(LEGACY_FILE), 1, &json)?;
    let store = CatalogStore::open(&path)?;
    assert_eq!(store.load()?.0.len(), 1);
    drop(store);
    std::fs::remove_file(path.join(LEGACY_FILE))?;
    let mut store = CatalogStore::open(&path)?;
    store.save(&[], false)?;
    drop(store);
    let json = serde_json::json!({"providers": [{"id":"new-old","name":"Old","enabled":true,"endpoints":[]}]});
    lumakit_support::save_versioned(&path.join(LEGACY_FILE), 1, &json)?;
    let store = CatalogStore::open(&path)?;
    assert!(store.load()?.0.is_empty());
    let _ = std::fs::remove_dir_all(path);
    Ok(())
}

#[test]
fn import_failure_does_not_write_marker_or_partial_rows() -> Result<(), Box<dyn std::error::Error>>
{
    let path = root("bad-import")?;
    let json = serde_json::json!({"providers": [{"id":"bad","name":"Bad","enabled":true,"endpoints":[{"id":"e","name":"E","base_url":"http://not-loopback","protocol":"OpenAiChat","keys":[],"models":[],"selected_key":null,"selected_model":null}]}]});
    lumakit_support::save_versioned(&path.join(LEGACY_FILE), 1, &json)?;
    assert!(CatalogStore::open(&path).is_err());
    let conn = Connection::open(path.join(DB_FILE))?;
    assert_eq!(
        conn.query_row("SELECT count(*) FROM providers", [], |r| r.get::<_, i64>(0))?,
        0
    );
    assert_eq!(
        conn.query_row(
            "SELECT value FROM store_meta WHERE key='legacy_imported'",
            [],
            |r| r.get::<_, String>(0)
        )
        .optional()?
        .as_deref(),
        Some("0")
    );
    let _ = std::fs::remove_dir_all(path);
    Ok(())
}

#[test]
fn future_version_and_malformed_json_are_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let future = root("future")?;
    lumakit_support::save_versioned(
        &future.join(LEGACY_FILE),
        2,
        &serde_json::json!({"providers":[]}),
    )?;
    assert!(CatalogStore::open(&future).is_err());
    let malformed = root("malformed")?;
    std::fs::write(malformed.join(LEGACY_FILE), b"not-json")?;
    assert!(CatalogStore::open(&malformed).is_err());
    let _ = std::fs::remove_dir_all(future);
    let _ = std::fs::remove_dir_all(malformed);
    Ok(())
}

#[test]
fn ciphertext_round_trip_and_save_rollback() -> Result<(), Box<dyn std::error::Error>> {
    let path = root("cipher")?;
    let key = SavedKey {
        id: "key-1".into(),
        masked: "••••••••".into(),
        label: None,
        enabled: true,
        ciphertext: vec![0, 1, 2, 255],
    };
    let mut store = CatalogStore::open(&path)?;
    let before = provider("p", Some(key.clone()));
    store.save(std::slice::from_ref(&before), false)?;
    assert_eq!(
        store.load()?.0[0].endpoints[0].keys[0].ciphertext,
        key.ciphertext
    );
    let mut invalid = before.clone();
    invalid.endpoints[0].base_url = "http://not-loopback".into();
    assert!(store.save(&[invalid], false).is_err());
    assert_eq!(
        store.load()?.0[0].endpoints[0].keys[0].ciphertext,
        key.ciphertext
    );
    let _ = std::fs::remove_dir_all(path);
    Ok(())
}
