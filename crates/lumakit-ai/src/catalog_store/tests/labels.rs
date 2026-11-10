use super::*;
use rusqlite::params;

fn key(id: &str, label: Option<&str>) -> SavedKey {
    SavedKey {
        id: id.into(),
        label: label.map(str::to_owned),
        enabled: true,
        masked: "••••••••".into(),
        // Intentionally not decryptable: migrations and label-only edits must treat it as opaque.
        ciphertext: vec![0, 1, 2, 255],
    }
}

#[test]
fn old_database_migrates_unlabeled_without_touching_ciphertext()
-> Result<(), Box<dyn std::error::Error>> {
    let path = root("old-schema-label")?;
    let conn = open_and_migrate(path.join(DB_FILE), &MIGRATIONS[..2])?;
    conn.execute(
        "INSERT INTO providers(id,name,enabled,position) VALUES('p','Provider',1,0)",
        [],
    )?;
    conn.execute(
        "INSERT INTO endpoints(id,provider_id,name,base_url,protocol,position,selected_key_id) \
         VALUES('p-endpoint','p','Endpoint','https://example.com/v1','openai_chat',0,'old-key')",
        [],
    )?;
    let original = key("old-key", None);
    conn.execute(
        "INSERT INTO endpoint_keys(id,endpoint_id,masked,ciphertext,position) VALUES(?1,'p-endpoint',?2,?3,0)",
        params![original.id, original.masked, original.ciphertext],
    )?;
    drop(conn);

    let mut store = CatalogStore::open(&path)?;
    let (mut providers, initialized) = store.load()?;
    let saved = &providers[0].endpoints[0].keys[0];
    assert!(saved.label.is_none());
    assert!(saved.enabled);
    assert_eq!(saved.id, original.id);
    assert_eq!(saved.ciphertext, original.ciphertext);
    assert_eq!(
        providers[0].endpoints[0].selected_key.as_deref(),
        Some("old-key")
    );
    providers[0].endpoints[0].keys[0].label = Some("  工作账号  ".into());
    store.save(&providers, initialized)?;
    drop(store);

    let store = CatalogStore::open(&path)?;
    let (providers, _) = store.load()?;
    let saved = &providers[0].endpoints[0].keys[0];
    assert_eq!(saved.label.as_deref(), Some("工作账号"));
    assert_eq!(saved.id, original.id);
    assert_eq!(saved.masked, original.masked);
    assert_eq!(saved.ciphertext, original.ciphertext);
    drop(store);
    std::fs::remove_dir_all(path)?;
    Ok(())
}

#[test]
fn legacy_json_import_preserves_labels_and_unlabeled_ciphertext()
-> Result<(), Box<dyn std::error::Error>> {
    let path = root("legacy-labels")?;
    let first = key("legacy-old", None);
    let second = key("legacy-labeled", Some("  备用账号  "));
    let mut p = provider("p", Some(first.clone()));
    p.endpoints[0].keys.push(second.clone());
    let mut json = serde_json::to_value(LegacyFile {
        providers: vec![p],
        presets_initialized: true,
    })?;
    json["providers"][0]["endpoints"][0]["keys"][0]
        .as_object_mut()
        .ok_or("missing key object")?
        .remove("label");
    lumakit_support::save_versioned(&path.join(LEGACY_FILE), 1, &json)?;
    let store = CatalogStore::open(&path)?;
    drop(store);

    let store = CatalogStore::open(&path)?;
    let (providers, initialized) = store.load()?;
    assert!(initialized);
    let keys = &providers[0].endpoints[0].keys;
    assert!(keys[0].label.is_none());
    assert_eq!(keys[0].id, first.id);
    assert_eq!(keys[0].ciphertext, first.ciphertext);
    assert_eq!(keys[1].label.as_deref(), Some("备用账号"));
    assert_eq!(keys[1].id, second.id);
    assert_eq!(keys[1].ciphertext, second.ciphertext);
    drop(store);
    std::fs::remove_dir_all(path)?;
    Ok(())
}

#[test]
fn save_keys_changes_and_clears_labels_without_overwriting_endpoint_fields()
-> Result<(), Box<dyn std::error::Error>> {
    let path = root("save-key-labels")?;
    let original = key("stable-key", Some("原标签"));
    let mut store = CatalogStore::open(&path)?;
    let p = provider("p", Some(original.clone()));
    store.save(&[p], true)?;
    let mut edited = original.clone();
    edited.enabled = false;
    edited.label = Some("  新标签  ".into());
    store.save_keys("p-endpoint", &[edited.clone()], Some(&edited.id))?;
    drop(store);

    let mut store = CatalogStore::open(&path)?;
    let (providers, initialized) = store.load()?;
    assert!(initialized);
    let endpoint = &providers[0].endpoints[0];
    assert_eq!(endpoint.name, "Endpoint");
    assert_eq!(endpoint.base_url, "https://example.com/v1");
    assert_eq!(endpoint.keys[0].label.as_deref(), Some("新标签"));
    assert!(!endpoint.keys[0].enabled);
    assert_eq!(endpoint.keys[0].id, original.id);
    assert_eq!(endpoint.keys[0].masked, original.masked);
    assert_eq!(endpoint.keys[0].ciphertext, original.ciphertext);
    edited.label = Some("   ".into());
    store.save_keys("p-endpoint", &[edited], Some(&original.id))?;
    drop(store);

    let store = CatalogStore::open(&path)?;
    let (providers, _) = store.load()?;
    assert!(providers[0].endpoints[0].keys[0].label.is_none());
    assert_eq!(
        providers[0].endpoints[0].keys[0].ciphertext,
        original.ciphertext
    );
    drop(store);
    std::fs::remove_dir_all(path)?;
    Ok(())
}

#[test]
fn invalid_labels_leave_catalog_and_key_selection_unchanged()
-> Result<(), Box<dyn std::error::Error>> {
    let path = root("invalid-label")?;
    let mut store = CatalogStore::open(&path)?;
    let mut p = provider("p", Some(key("stable", Some("原标签"))));
    // Unicode scalar count, not UTF-8 byte length, defines the accepted boundary.
    p.endpoints[0].keys[0].label = Some("标".repeat(128));
    store.save(std::slice::from_ref(&p), true)?;
    let before = serde_json::to_value(store.load()?.0)?;
    for label in ["标".repeat(129), "a\0b".into(), "a\nb".into()] {
        let mut invalid = p.clone();
        invalid.endpoints[0].keys[0].label = Some(label);
        invalid.name = "不应保存".into();
        assert!(store.save(std::slice::from_ref(&invalid), false).is_err());
        assert!(
            store
                .save_keys("p-endpoint", &invalid.endpoints[0].keys, None)
                .is_err()
        );
        assert_eq!(serde_json::to_value(store.load()?.0)?, before);
        assert!(store.load()?.1);
    }
    drop(store);
    std::fs::remove_dir_all(path)?;
    Ok(())
}

#[test]
fn sql_failure_rolls_back_labels_ciphertext_and_catalog_changes()
-> Result<(), Box<dyn std::error::Error>> {
    let path = root("rollback-label")?;
    let mut store = CatalogStore::open(&path)?;
    let mut p = provider("p", Some(key("first", Some("原标签"))));
    p.endpoints[0].keys.push(key("second", Some("备用")));
    store.save(std::slice::from_ref(&p), true)?;
    let before = serde_json::to_value(store.load()?.0)?;
    store.conn.execute_batch(
        "CREATE TRIGGER reject_label BEFORE UPDATE ON endpoint_keys \
         WHEN NEW.label='拒绝' BEGIN SELECT RAISE(ABORT,'test label failure'); END;",
    )?;
    let mut changed = p;
    changed.name = "不应保存".into();
    changed.endpoints[0].keys[0].label = Some("先写入".into());
    changed.endpoints[0].keys[1].label = Some("拒绝".into());
    assert!(store.save(std::slice::from_ref(&changed), false).is_err());
    assert_eq!(serde_json::to_value(store.load()?.0)?, before);
    assert!(store.load()?.1);
    assert!(
        store
            .save_keys("p-endpoint", &changed.endpoints[0].keys, Some("second"))
            .is_err()
    );
    assert_eq!(serde_json::to_value(store.load()?.0)?, before);
    drop(store);

    let store = CatalogStore::open(&path)?;
    assert_eq!(serde_json::to_value(store.load()?.0)?, before);
    assert!(store.load()?.1);
    drop(store);
    std::fs::remove_dir_all(path)?;
    Ok(())
}
