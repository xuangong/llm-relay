//! One-way import of field snapshots. Reconstruction happens in memory; live
//! files are never temporarily restored during migration.
use super::*;
use crate::config_writer::snapshot;
use std::sync::Mutex;

#[derive(Default)]
struct MemoryFiles(Mutex<BTreeMap<Vec<String>, Vec<u8>>>);

impl CliBackend for MemoryFiles {
    fn read_bytes(&self, path: &[&str]) -> Result<Option<Vec<u8>>, AppError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .get(&path.iter().map(|s| s.to_string()).collect::<Vec<_>>())
            .cloned())
    }
    fn write_atomic(&self, path: &[&str], bytes: &[u8]) -> Result<(), AppError> {
        self.0
            .lock()
            .unwrap()
            .insert(path.iter().map(|s| s.to_string()).collect(), bytes.to_vec());
        Ok(())
    }
    fn remove(&self, path: &[&str]) -> Result<(), AppError> {
        self.0
            .lock()
            .unwrap()
            .remove(&path.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        Ok(())
    }
    fn exists(&self, path: &[&str]) -> Result<bool, AppError> {
        Ok(self.read_bytes(path)?.is_some())
    }
}

fn archive(meta: &SnapshotMeta) -> Result<(), AppError> {
    let path = snapshot::snapshot_path(meta);
    if path.exists() {
        let archive = path.with_extension(format!(
            "migrated-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::rename(path, archive)?;
    }
    Ok(())
}

/// Commit native origins and durable WSL import payloads together. WSL imports
/// finish on reconnect, before any configuration is written to that target.
pub fn migrate_legacy_active() -> Result<(), AppError> {
    if manifest_exists() {
        return Ok(());
    }
    recover(true)?;
    // Unlike the UI's best-effort index, migration must not silently omit a
    // malformed target and then publish an incomplete manifest.
    let mut index = BTreeMap::new();
    for entry in std::fs::read_dir(crate::paths::cli_config_backup_dir())? {
        let path = entry?.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let snap: snapshot::TargetSnapshot = serde_json::from_slice(&std::fs::read(&path)?)?;
        let meta = SnapshotMeta {
            target_type: match snap.target_type.as_str() {
                "windows" => TargetType::Windows,
                "wsl" => TargetType::Wsl,
                _ => {
                    return Err(AppError::Config(
                        "invalid legacy snapshot target type".into(),
                    ))
                }
            },
            distro_name: snap.distro_name,
            home: snap.home,
        };
        if snapshot::snapshot_path(&meta) != path || index.insert(target_key(&meta), meta).is_some()
        {
            return Err(AppError::Config("legacy snapshot identity mismatch".into()));
        }
    }
    let mut manifest = LifecycleManifest {
        version: MANIFEST_VERSION,
        phase: LifecyclePhase::Active,
        updated_at: String::new(),
        targets: Vec::new(),
        host_openai_api_key: capture_host_openai_api_key(),
    };
    let mut staged = Vec::new();
    for meta in index.values() {
        let snap = snapshot::read(meta)?.ok_or_else(|| {
            AppError::Config("legacy snapshot disappeared during migration".into())
        })?;
        let backend: Box<dyn CliBackend> = match meta.target_type {
            TargetType::Windows => Box::new(WindowsFsBackend {
                home: current_native_home()?,
            }),
            TargetType::Wsl => Box::new(WslBackend {
                distro: meta
                    .distro_name
                    .clone()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| AppError::Config("legacy WSL snapshot has no distro".into()))?,
                home: meta.home.clone().unwrap_or_default(),
            }),
        };
        let target = CliTarget {
            backend,
            base_url: String::new(),
            installed: InstalledTools::ALL,
            label: meta.distro_name.clone().unwrap_or_else(|| "native".into()),
            snapshot_meta: meta.clone(),
        };
        if meta.target_type == TargetType::Wsl {
            let mut stored = stored_target(&target, Vec::new());
            stored
                .extra_env_keys
                .extend(snap.claude.extra_env_originals.keys().cloned());
            stored.legacy_snapshot = Some(snap);
            stored.pending = true;
            stored.pending_reason = Some(
                "Waiting for WSL to connect before importing its original configuration".into(),
            );
            manifest.targets.push(stored);
            continue;
        }
        let (files, contents) = reconstruct(&target, &snap)?;
        let mut stored = stored_target(&target, files);
        stored
            .extra_env_keys
            .extend(snap.claude.extra_env_originals.keys().cloned());
        manifest.targets.push(stored);
        staged.push((target, contents, manifest.targets.len() - 1));
    }
    // Write all sidecars before the atomic manifest commit. Matching sidecars
    // permit retries after interruption; unrelated orphan files are rejected.
    for (target, contents, index) in &staged {
        write_origins(target, &manifest.targets[*index].files, contents)?;
    }
    save(&mut manifest)?;
    for meta in index.values() {
        archive(meta)?;
    }
    Ok(())
}

/// Derive writer defaults from immutable full-file origins. No second on-disk
/// field snapshot is created. Only the names of managed Extra keys are tracked.
pub fn snapshot_for_apply(
    target: &CliTarget,
    extra: Option<&BTreeMap<String, String>>,
) -> Result<snapshot::TargetSnapshot, AppError> {
    let mut manifest = load()?
        .ok_or_else(|| AppError::Config("full-file lifecycle is required before apply".into()))?;
    let stored = manifest
        .targets
        .iter_mut()
        .find(|stored| stored_key(stored) == target_key(&target.snapshot_meta))
        .ok_or_else(|| AppError::Config("target has no full-file origins".into()))?;
    if stored.legacy_snapshot.is_some()
        || (stored.pending
            && stored.pending_reason.as_deref() != Some("Applying Relay configuration"))
    {
        return Err(AppError::Config(
            stored
                .pending_reason
                .clone()
                .unwrap_or_else(|| "WSL configuration is waiting to sync".into()),
        ));
    }
    for expected in descriptors(target.installed, None) {
        if !stored
            .files
            .iter()
            .any(|file| file.path == expected.path && file.origin.complete)
        {
            return Err(AppError::Config(
                "target origin capture is incomplete".into(),
            ));
        }
    }
    // Import auxiliary metadata from earlier full-file releases once.
    if let Some(old) = snapshot::read(&target.snapshot_meta)? {
        stored
            .extra_env_keys
            .extend(old.claude.extra_env_originals.keys().cloned());
    }
    if let Some(extra) = extra {
        stored.extra_env_keys.extend(extra.keys().cloned());
    }
    let memory = MemoryFiles::default();
    for file in &stored.files {
        verify_origin(target, file)?;
        if file.origin.exists {
            let path = sidecar_path(&file.path, ORIGIN_SUFFIX)?;
            let bytes = target
                .backend
                .read_bytes(&refs(&path))?
                .ok_or_else(|| AppError::Config("origin disappeared".into()))?;
            memory.write_atomic(&refs(&file.path), &bytes)?;
        }
    }
    let mut snap = snapshot::capture_from_backend(&target.snapshot_meta, &memory)?;
    let env = memory
        .read(&[".claude", "settings.json"])?
        .map(|s| serde_json::from_str::<serde_json::Value>(&s))
        .transpose()?
        .and_then(|v| v.get("env").and_then(serde_json::Value::as_object).cloned())
        .unwrap_or_default();
    snap.claude.extra_env_captured = true;
    for key in &stored.extra_env_keys {
        snap.claude.extra_env_originals.insert(
            key.clone(),
            env.get(key)
                .and_then(serde_json::Value::as_str)
                .map(String::from),
        );
    }
    save(&mut manifest)?;
    archive(&target.snapshot_meta)?;
    Ok(snap)
}

fn reconstruct(
    target: &CliTarget,
    snap: &snapshot::TargetSnapshot,
) -> Result<(Vec<ManagedFile>, Vec<Option<Vec<u8>>>), AppError> {
    let meta = &target.snapshot_meta;
    // Legacy snapshots cover these six files. Shell/onboarding history that
    // was never captured cannot be reconstructed; retain its current state.
    let mut files = descriptors(InstalledTools::ALL, None);
    let memory = MemoryFiles::default();
    for file in &files {
        if let Some(bytes) = target.backend.read_bytes(&refs(&file.path))? {
            memory.write_atomic(&refs(&file.path), &bytes)?;
        }
    }
    // Validate before invoking permissive legacy restore helpers.
    snapshot::capture_from_backend(meta, &memory)?;
    for path in [
        [".claude", "settings.json"],
        [".codex", "auth.json"],
        [".gemini", "settings.json"],
    ] {
        if let Some(content) = memory.read(&path)? {
            let value: serde_json::Value = serde_json::from_str(&content)?;
            if !value.is_object() {
                return Err(AppError::Config(format!(
                    "cannot migrate non-object {}",
                    path.join("/")
                )));
            }
        }
    }
    snapshot::restore(&snap, &memory)?;
    let mut contents = Vec::new();
    for file in &mut files {
        let bytes = memory.read_bytes(&refs(&file.path))?;
        file.origin = state_from(bytes.as_deref(), true);
        file.touched = true;
        contents.push(bytes);
    }
    Ok((files, contents))
}

pub(super) fn resume(manifest: &mut LifecycleManifest, target: &CliTarget) -> Result<(), AppError> {
    let Some(index) = manifest
        .targets
        .iter()
        .position(|stored| stored_key(stored) == target_key(&target.snapshot_meta))
    else {
        return Ok(());
    };
    let Some(snap) = manifest.targets[index].legacy_snapshot.clone() else {
        return Ok(());
    };
    let (files, contents) = reconstruct(target, &snap)?;
    write_origins(target, &files, &contents)?;
    manifest.targets[index].files = files;
    manifest.targets[index].legacy_snapshot = None;
    save(manifest)
}

fn write_origins(
    target: &CliTarget,
    files: &[ManagedFile],
    contents: &[Option<Vec<u8>>],
) -> Result<(), AppError> {
    for (file, bytes) in files.iter().zip(contents) {
        let origin = sidecar_path(&file.path, ORIGIN_SUFFIX)?;
        if let Some(existing) = target.backend.read_bytes(&refs(&origin))? {
            if existing != bytes.as_deref().unwrap_or_default() {
                return Err(AppError::Config(format!(
                    "conflicting origin sidecar during migration: {}:{}",
                    target.label,
                    file.path.join("/")
                )));
            }
        } else {
            write_state(&*target.backend, &refs(&origin), bytes.as_deref())?;
        }
    }
    Ok(())
}
