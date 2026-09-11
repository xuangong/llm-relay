use crate::cli_target::{
    CliBackend, CliTarget, InstalledTools, SnapshotMeta, TargetType, WindowsFsBackend, WslBackend,
};
use crate::AppError;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

const MANIFEST_VERSION: u32 = 1;
const ORIGIN_SUFFIX: &str = ".llm-relay.origin";
const BACKUP_SUFFIX: &str = ".llm-relay.bak";

mod migration;
pub use migration::{migrate_legacy_active, snapshot_for_apply};

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecyclePhase {
    Inactive,
    PreparingUse,
    Active,
    CapturingDisableBackup,
    RestoringOrigin,
    CleanupPending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Claude,
    Codex,
    Gemini,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredFileState {
    pub exists: bool,
    #[serde(default)]
    pub complete: bool,
}

impl Default for StoredFileState {
    fn default() -> Self {
        Self {
            exists: false,
            complete: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedFile {
    pub path: Vec<String>,
    pub provider: Provider,
    pub origin: StoredFileState,
    #[serde(default)]
    pub backup: StoredFileState,
    #[serde(default)]
    pub touched: bool,
    #[serde(default)]
    pub restored: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default = "default_true")]
    pub managed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedTarget {
    /// Missing lifecycle history: rebuild managed CLI files from .bak (or
    /// absence) on the next apply, including after an offline WSL reconnect.
    #[serde(default)]
    pub rebuild_from_backup: bool,
    pub target_type: String,
    #[serde(default)]
    pub distro_name: Option<String>,
    #[serde(default)]
    pub home: Option<String>,
    #[serde(default)]
    pub native_home: Option<String>,
    pub base_url: String,
    pub installed: StoredInstalledTools,
    pub label: String,
    pub files: Vec<ManagedFile>,
    #[serde(default)]
    pub extra_env_keys: HashSet<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_snapshot: Option<super::snapshot::TargetSnapshot>,
    #[serde(default)]
    pub pending: bool,
    #[serde(default)]
    pub pending_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct StoredInstalledTools {
    pub claude: bool,
    pub codex: bool,
    pub gemini: bool,
}

impl From<InstalledTools> for StoredInstalledTools {
    fn from(value: InstalledTools) -> Self {
        Self {
            claude: value.claude,
            codex: value.codex,
            gemini: value.gemini,
        }
    }
}

impl From<StoredInstalledTools> for InstalledTools {
    fn from(value: StoredInstalledTools) -> Self {
        Self {
            claude: value.claude,
            codex: value.codex,
            gemini: value.gemini,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LifecycleManifest {
    pub version: u32,
    pub phase: LifecyclePhase,
    pub updated_at: String,
    pub targets: Vec<ManagedTarget>,
    #[serde(default)]
    pub host_openai_api_key: Option<StoredHostValue>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StoredHostValue {
    pub exists: bool,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub relay_owned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleFileStatus {
    pub relative_path: String,
    pub provider: Provider,
    pub origin_exists: bool,
    pub backup_exists: Option<bool>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleTargetStatus {
    pub target_type: String,
    pub distro_name: Option<String>,
    pub label: String,
    pub phase: LifecyclePhase,
    pub files: Vec<LifecycleFileStatus>,
    pub pending: bool,
    pub pending_reason: Option<String>,
}

pub struct TargetHandle {
    pub target: CliTarget,
    pub manifest_index: usize,
}

pub fn manifest_exists() -> bool {
    crate::paths::cli_file_lifecycle_manifest().exists()
}

pub fn load() -> Result<Option<LifecycleManifest>, AppError> {
    let path = crate::paths::cli_file_lifecycle_manifest();
    if !path.exists() {
        return Ok(None);
    }
    let manifest: LifecycleManifest = serde_json::from_slice(&std::fs::read(&path)?)?;
    validate_manifest(&manifest)?;
    validate_target_roots(&manifest)?;
    Ok(Some(manifest))
}

pub fn load_or_quarantine() -> Result<Option<LifecycleManifest>, AppError> {
    match load() {
        Ok(manifest) => Ok(manifest),
        Err(error) => {
            let path = crate::paths::cli_file_lifecycle_manifest();
            if path.exists() {
                let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%f");
                let quarantined =
                    path.with_file_name(format!("cli-file-lifecycle.quarantine-{stamp}.json"));
                std::fs::rename(&path, &quarantined)?;
                crate::cli_target::atomic_write(
                    &crate::paths::cli_file_lifecycle_blocked(),
                    format!("unsafe lifecycle metadata quarantined: {error}").as_bytes(),
                )?;
                return Err(AppError::Config(format!(
                    "CLI lifecycle metadata was quarantined because it is unsafe: {error}"
                )));
            }
            Err(error)
        }
    }
}

pub fn save(manifest: &mut LifecycleManifest) -> Result<(), AppError> {
    manifest.updated_at = chrono::Utc::now().to_rfc3339();
    validate_manifest(manifest)?;
    crate::cli_target::atomic_write(
        &crate::paths::cli_file_lifecycle_manifest(),
        &serde_json::to_vec_pretty(manifest)?,
    )
}

pub fn status() -> Result<Vec<LifecycleTargetStatus>, AppError> {
    let Some(manifest) = load()? else {
        return Ok(Vec::new());
    };
    Ok(manifest
        .targets
        .into_iter()
        .map(|target| LifecycleTargetStatus {
            target_type: target.target_type,
            distro_name: target.distro_name,
            label: target.label,
            phase: manifest.phase,
            pending: target.pending,
            pending_reason: target.pending_reason,
            files: target
                .files
                .into_iter()
                .map(|file| LifecycleFileStatus {
                    relative_path: file.path.join("/"),
                    provider: file.provider,
                    origin_exists: file.origin.exists,
                    backup_exists: file.backup.complete.then_some(file.backup.exists),
                    error: file.error,
                })
                .collect(),
        })
        .collect())
}

pub fn prepare_use(
    targets: &[CliTarget],
    pending: &[crate::service::PendingWslTarget],
    shell_paths: &BTreeMap<String, Vec<String>>,
) -> Result<LifecycleManifest, AppError> {
    if crate::paths::cli_file_lifecycle_blocked().exists() {
        return Err(AppError::Config(
            "CLI lifecycle recovery is blocked; reset lifecycle ownership before Use".into(),
        ));
    }
    let previous = load()?;
    let mut manifest = LifecycleManifest {
        version: MANIFEST_VERSION,
        phase: LifecyclePhase::PreparingUse,
        updated_at: chrono::Utc::now().to_rfc3339(),
        targets: Vec::new(),
        host_openai_api_key: capture_host_openai_api_key(),
    };
    save(&mut manifest)?;

    for target in targets {
        let key = target_key(&target.snapshot_meta);
        let previous_target = previous.as_ref().and_then(|manifest| {
            manifest
                .targets
                .iter()
                .find(|stored| stored_key(stored) == key)
        });
        let mut files = descriptors(target.installed, shell_paths.get(&key).cloned());
        for file in &mut files {
            capture_origin(target, file, previous_target.is_none())?;
            let rel = refs(&file.path);

            if let Some(previous_file) = previous_target.and_then(|stored| {
                stored
                    .files
                    .iter()
                    .find(|candidate| candidate.path == file.path)
            }) {
                let backup_path = sidecar_path(&file.path, BACKUP_SUFFIX)?;
                let backup_refs = refs(&backup_path);
                if previous_file.backup.complete {
                    verify_sidecar(&*target.backend, &backup_refs, &previous_file.backup)?;
                    restore_state(&*target.backend, &rel, &backup_refs, &previous_file.backup)?;
                    file.backup = previous_file.backup.clone();
                }
            }
        }
        manifest.targets.push(stored_target(target, files));
        save(&mut manifest)?;
    }
    for pending in pending {
        manifest.targets.push(ManagedTarget {
            rebuild_from_backup: false,
            target_type: "wsl".into(),
            distro_name: Some(pending.name.clone()),
            home: pending.home.clone(),
            native_home: None,
            base_url: String::new(),
            installed: pending.installed.into(),
            label: format!("wsl:{}", pending.name),
            files: Vec::new(),
            extra_env_keys: HashSet::new(),
            legacy_snapshot: None,
            pending: true,
            pending_reason: Some(pending.reason.clone()),
        });
    }
    save(&mut manifest)?;
    Ok(manifest)
}

pub fn prepare_active_apply(
    targets: &[CliTarget],
    shell_paths: &BTreeMap<String, Vec<String>>,
) -> Result<Option<LifecycleManifest>, AppError> {
    let Some(mut manifest) = load_or_quarantine()? else {
        return Ok(None);
    };
    if manifest.phase == LifecyclePhase::PreparingUse {
        // The DB is already active, so this is an interrupted final phase save.
        // Resume using the local origins instead of recapturing working files.
        manifest.phase = LifecyclePhase::Active;
        save(&mut manifest)?;
    }
    if manifest.phase != LifecyclePhase::Active {
        return Err(AppError::Config(format!(
            "CLI lifecycle recovery is required before reapply ({:?})",
            manifest.phase
        )));
    }
    let mut changed = false;
    for target in targets {
        let result = (|| -> Result<(), AppError> {
            resume_missing_backups(&mut manifest, target)?;
            migration::resume(&mut manifest, target)?;
            let key = target_key(&target.snapshot_meta);
            let desired = descriptors(target.installed, shell_paths.get(&key).cloned());
            if let Some(index) = manifest
                .targets
                .iter()
                .position(|stored| stored_key(stored) == key)
            {
                for file in &mut manifest.targets[index].files {
                    if reconcile_deleted_origin(&*target.backend, file)? {
                        changed = true;
                    }
                }
                let desired_paths: HashSet<Vec<String>> =
                    desired.iter().map(|file| file.path.clone()).collect();
                let backend = &*target.backend;
                for existing in &mut manifest.targets[index].files {
                    if existing.managed && !desired_paths.contains(&existing.path) {
                        let origin = sidecar_path(&existing.path, ORIGIN_SUFFIX)?;
                        restore_state(
                            backend,
                            &refs(&existing.path),
                            &refs(&origin),
                            &existing.origin,
                        )?;
                        existing.managed = false;
                        existing.restored = true;
                        changed = true;
                    }
                }
                for mut file in desired {
                    if let Some(existing) = manifest.targets[index]
                        .files
                        .iter_mut()
                        .find(|existing| existing.path == file.path)
                    {
                        verify_origin(target, existing)?;
                        if !existing.managed {
                            existing.managed = true;
                            existing.restored = false;
                            changed = true;
                        }
                        continue;
                    }
                    capture_origin(target, &mut file, true)?;
                    file.touched = true;
                    manifest.targets[index].files.push(file);
                    save(&mut manifest)?;
                    changed = true;
                }
                let stored = &mut manifest.targets[index];
                if stored.base_url != target.base_url || stored.pending {
                    changed = true;
                }
                stored.base_url = target.base_url.clone();
                stored.home = target.snapshot_meta.home.clone();
                stored.installed = target.installed.into();
                stored.pending = true;
                stored.pending_reason = Some("Applying Relay configuration".into());
            } else {
                let files = desired;
                let mut stored = stored_target(target, Vec::new());
                stored.pending = true;
                stored.pending_reason = Some("Capturing original configuration".into());
                manifest.targets.push(stored);
                save(&mut manifest)?;
                let index = manifest.targets.len() - 1;
                for mut file in files {
                    capture_origin(target, &mut file, true)?;
                    file.touched = true;
                    manifest.targets[index].files.push(file);
                    save(&mut manifest)?;
                }
                manifest.targets[index].pending_reason =
                    Some("Applying Relay configuration".into());
                changed = true;
            }
            Ok(())
        })();
        if let Err(error) = result {
            if target.snapshot_meta.target_type != TargetType::Wsl {
                return Err(error);
            }
            let key = target_key(&target.snapshot_meta);
            if !manifest
                .targets
                .iter()
                .any(|stored| stored_key(stored) == key)
            {
                manifest.targets.push(stored_target(target, Vec::new()));
            }
            let stored = manifest
                .targets
                .iter_mut()
                .find(|stored| stored_key(stored) == key)
                .unwrap();
            stored.pending = true;
            stored.pending_reason = Some(format!("Waiting to sync WSL: {error}"));
            changed = true;
        }
    }
    if changed {
        save(&mut manifest)?;
    }
    Ok(Some(manifest))
}

pub fn mark_active(manifest: &mut LifecycleManifest) -> Result<(), AppError> {
    let keys: HashSet<String> = manifest
        .targets
        .iter()
        .filter(|target| !target.files.is_empty())
        .map(stored_key)
        .collect();
    mark_targets_active(manifest, &keys)
}

pub fn mark_targets_active(
    manifest: &mut LifecycleManifest,
    keys: &HashSet<String>,
) -> Result<(), AppError> {
    // Writers persist Extra-key ownership while applying; don't overwrite it
    // with the preparation-time manifest held by the service.
    if let Some(latest) = load()? {
        *manifest = latest;
    }
    manifest.phase = LifecyclePhase::Active;
    'targets: for target in &mut manifest.targets {
        if !keys.contains(&stored_key(target)) && !keys.contains(&report_key(target)) {
            continue;
        }
        let backend = backend_for(target)?;
        for file in &mut target.files {
            if !file.managed {
                continue;
            }
            file.touched = true;
            file.restored = false;
            file.error = None;
            if file.backup.complete {
                let backup_path = sidecar_path(&file.path, BACKUP_SUFFIX)?;
                if let Err(error) = backend.remove(&refs(&backup_path)) {
                    if target.target_type != "wsl" {
                        return Err(error);
                    }
                    target.pending = true;
                    target.pending_reason = Some(format!("Waiting to finish WSL sync: {error}"));
                    continue 'targets;
                }
                file.backup = StoredFileState::default();
            }
        }
        target.pending = false;
        target.pending_reason = None;
    }
    save(manifest)
}

pub fn recover(db_active: bool) -> Result<(), AppError> {
    let blocked = crate::paths::cli_file_lifecycle_blocked();
    if blocked.exists() {
        return Err(AppError::Config(
            "CLI lifecycle recovery is blocked; reset lifecycle ownership before applying changes"
                .into(),
        ));
    }
    let Some(mut manifest) = load_or_quarantine()? else {
        // A stale active DB row is not proof that backup files exist. The next
        // apply initializes missing history and generates from .bak or empty.
        return Ok(());
    };
    match manifest.phase {
        LifecyclePhase::Active | LifecyclePhase::Inactive => Ok(()),
        LifecyclePhase::PreparingUse if db_active => {
            for target in &manifest.targets {
                if target.pending && target.files.is_empty() {
                    continue;
                }
                let backend = backend_for(target)?;
                for file in &target.files {
                    verify_origin_for_backend(&*backend, file)?;
                }
            }
            manifest.phase = LifecyclePhase::Active;
            save(&mut manifest)
        }
        LifecyclePhase::PreparingUse | LifecyclePhase::CleanupPending if !db_active => {
            rollback_use(&mut manifest)
        }
        LifecyclePhase::CapturingDisableBackup | LifecyclePhase::RestoringOrigin => disable(),
        phase => Err(AppError::Config(format!(
            "CLI lifecycle recovery is blocked in phase {phase:?}"
        ))),
    }
}

pub fn prepare_missing_active(
    targets: &[CliTarget],
    pending: &[crate::service::PendingWslTarget],
) -> Result<(), AppError> {
    recover(true)?;
    if manifest_exists() || super::snapshot::has_legacy_snapshots()? {
        return Ok(());
    }
    let mut manifest = LifecycleManifest {
        version: MANIFEST_VERSION,
        phase: LifecyclePhase::Active,
        updated_at: String::new(),
        targets: targets
            .iter()
            .map(|target| {
                let mut stored = stored_target(target, missing_files(target.installed));
                stored.rebuild_from_backup = true;
                stored
            })
            .collect(),
        host_openai_api_key: capture_host_openai_api_key(),
    };
    for row in pending {
        manifest.targets.push(ManagedTarget {
            rebuild_from_backup: true,
            target_type: "wsl".into(),
            distro_name: Some(row.name.clone()),
            home: row.home.clone(),
            native_home: None,
            base_url: String::new(),
            installed: row.installed.into(),
            label: format!("wsl:{}", row.name),
            files: missing_files(row.installed),
            extra_env_keys: HashSet::new(),
            legacy_snapshot: None,
            pending: true,
            pending_reason: Some(row.reason.clone()),
        });
    }
    save(&mut manifest)
}

fn missing_files(installed: InstalledTools) -> Vec<ManagedFile> {
    descriptors(installed, None)
        .into_iter()
        .map(|mut file| {
            file.origin = state_from(None, true);
            file
        })
        .collect()
}

fn resume_missing_backups(
    manifest: &mut LifecycleManifest,
    target: &CliTarget,
) -> Result<(), AppError> {
    let Some(index) = manifest.targets.iter().position(|stored| {
        stored_key(stored) == target_key(&target.snapshot_meta) && stored.rebuild_from_backup
    }) else {
        return Ok(());
    };
    // Only CLI files are reset here. Shell profiles are captured normally when
    // prepare_active_apply adds their descriptors; never empty a user's shell.
    let mut files = manifest.targets[index].files.clone();
    for file in &mut files {
        let origin = target
            .backend
            .read_bytes(&refs(&sidecar_path(&file.path, ORIGIN_SUFFIX)?))?;
        let backup = target
            .backend
            .read_bytes(&refs(&sidecar_path(&file.path, BACKUP_SUFFIX)?))?;
        file.origin = state_from(origin.as_deref(), true);
        file.backup = state_from(backup.as_deref(), true);
        file.touched = true;
    }
    manifest.targets[index].files = files;
    // Persist origins before resetting any working file. A failed or interrupted
    // reset can retry from the same sidecars without capturing Relay as origin.
    save(manifest)?;
    for file in &manifest.targets[index].files {
        restore_state(
            &*target.backend,
            &refs(&file.path),
            &refs(&sidecar_path(&file.path, BACKUP_SUFFIX)?),
            &file.backup,
        )?;
    }
    manifest.targets[index].rebuild_from_backup = false;
    save(manifest)
}

pub fn has_pending_wsl() -> bool {
    load().ok().flatten().is_some_and(|manifest| {
        manifest
            .targets
            .iter()
            .any(|target| target.target_type == "wsl" && target.pending)
    })
}

pub fn record_pending_wsl(pending: &[crate::service::PendingWslTarget]) -> Result<(), AppError> {
    if pending.is_empty() {
        return Ok(());
    }
    let Some(mut manifest) = load()? else {
        return Ok(());
    };
    for row in pending {
        if let Some(stored) = manifest
            .targets
            .iter_mut()
            .find(|t| t.distro_name.as_deref() == Some(&row.name))
        {
            stored.pending = true;
            stored.pending_reason = Some(row.reason.clone());
        } else {
            manifest.targets.push(ManagedTarget {
                rebuild_from_backup: false,
                target_type: "wsl".into(),
                distro_name: Some(row.name.clone()),
                home: row.home.clone(),
                native_home: None,
                base_url: String::new(),
                installed: row.installed.into(),
                label: format!("wsl:{}", row.name),
                files: Vec::new(),
                extra_env_keys: HashSet::new(),
                legacy_snapshot: None,
                pending: true,
                pending_reason: Some(row.reason.clone()),
            });
        }
    }
    save(&mut manifest)
}

pub fn mark_target_failed(key: &str, error: &str) -> Result<(), AppError> {
    let Some(mut manifest) = load()? else {
        return Ok(());
    };
    if let Some(target) = manifest
        .targets
        .iter_mut()
        .find(|target| stored_key(target) == key || report_key(target) == key)
    {
        target.pending = true;
        target.pending_reason = Some(error.to_string());
        for file in &mut target.files {
            file.error = Some(error.to_string());
        }
    }
    manifest.phase = LifecyclePhase::Active;
    save(&mut manifest)
}

pub fn rollback_use(manifest: &mut LifecycleManifest) -> Result<(), AppError> {
    manifest.phase = LifecyclePhase::CleanupPending;
    save(manifest)?;
    let mut failures = Vec::new();
    for target in &mut manifest.targets {
        if target.pending && target.files.is_empty() {
            continue;
        }
        let backend = backend_for(target)?;
        for file in &mut target.files {
            let rel = refs(&file.path);
            let origin = sidecar_path(&file.path, ORIGIN_SUFFIX)?;
            if let Err(error) = restore_state(&*backend, &rel, &refs(&origin), &file.origin) {
                file.error = Some(error.to_string());
                failures.push(format!("{}:{}", target.label, file.path.join("/")));
            }
        }
    }
    if failures.is_empty() {
        manifest.phase = LifecyclePhase::Inactive;
        save(manifest)?;
        Ok(())
    } else {
        save(manifest)?;
        Err(AppError::Config(format!(
            "Use rollback failed for {}",
            failures.join(", ")
        )))
    }
}

pub fn disable() -> Result<(), AppError> {
    let Some(mut manifest) = load()? else {
        return Ok(());
    };
    for index in 0..manifest.targets.len() {
        if manifest.targets[index].legacy_snapshot.is_some()
            || manifest.targets[index].rebuild_from_backup
        {
            let stored = &manifest.targets[index];
            let target = CliTarget {
                backend: backend_for(stored)?,
                base_url: stored.base_url.clone(),
                installed: stored.installed.into(),
                label: stored.label.clone(),
                snapshot_meta: SnapshotMeta {
                    target_type: if stored.target_type == "wsl" {
                        TargetType::Wsl
                    } else {
                        TargetType::Windows
                    },
                    distro_name: stored.distro_name.clone(),
                    home: stored.home.clone(),
                },
            };
            resume_missing_backups(&mut manifest, &target)?;
            migration::resume(&mut manifest, &target)?;
        }
    }
    manifest.phase = LifecyclePhase::CapturingDisableBackup;
    save(&mut manifest)?;

    let mut capture_failures = Vec::new();
    for target in &mut manifest.targets {
        if target.pending && target.files.is_empty() {
            continue;
        }
        let backend = backend_for(target)?;
        for file in &mut target.files {
            if !file.touched {
                continue;
            }
            if let Err(error) = reconcile_deleted_origin(&*backend, file) {
                file.error = Some(error.to_string());
                capture_failures.push(format!("{}:{}", target.label, file.path.join("/")));
                continue;
            }
            let current = match backend.read_bytes(&refs(&file.path)) {
                Ok(current) => current,
                Err(error) => {
                    file.error = Some(error.to_string());
                    capture_failures.push(format!("{}:{}", target.label, file.path.join("/")));
                    continue;
                }
            };
            let backup = sidecar_path(&file.path, BACKUP_SUFFIX)?;
            let backup_refs = refs(&backup);
            if let Err(error) = write_state(&*backend, &backup_refs, current.as_deref()) {
                file.error = Some(error.to_string());
                capture_failures.push(format!("{}:{}", target.label, file.path.join("/")));
            } else {
                file.backup = state_from(current.as_deref(), true);
                file.error = None;
            }
        }
    }
    save(&mut manifest)?;
    if !capture_failures.is_empty() {
        manifest.phase = LifecyclePhase::CleanupPending;
        save(&mut manifest)?;
        return Err(AppError::Config(format!(
            "Disable backup failed for {}",
            capture_failures.join(", ")
        )));
    }

    manifest.phase = LifecyclePhase::RestoringOrigin;
    save(&mut manifest)?;
    let mut restore_failures = Vec::new();
    for target in &mut manifest.targets {
        if target.pending && target.files.is_empty() {
            continue;
        }
        let backend = backend_for(target)?;
        for file in &mut target.files {
            let origin = sidecar_path(&file.path, ORIGIN_SUFFIX)?;
            match restore_state(&*backend, &refs(&file.path), &refs(&origin), &file.origin) {
                Ok(()) => {
                    file.restored = true;
                    file.error = None;
                }
                Err(error) => {
                    file.error = Some(error.to_string());
                    restore_failures.push(format!("{}:{}", target.label, file.path.join("/")));
                }
            }
        }
    }
    save(&mut manifest)?;
    if !restore_failures.is_empty() {
        manifest.phase = LifecyclePhase::CleanupPending;
        save(&mut manifest)?;
        return Err(AppError::Config(format!(
            "Disable restore failed for {}",
            restore_failures.join(", ")
        )));
    }
    restore_host_openai_api_key(manifest.host_openai_api_key.as_ref())?;
    manifest.phase = LifecyclePhase::Inactive;
    save(&mut manifest)
}

pub fn build_targets(manifest: &LifecycleManifest) -> Result<Vec<TargetHandle>, AppError> {
    manifest
        .targets
        .iter()
        .enumerate()
        .map(|(index, stored)| {
            let backend = backend_for(stored)?;
            Ok(TargetHandle {
                target: CliTarget {
                    backend,
                    base_url: stored.base_url.clone(),
                    installed: stored.installed.into(),
                    label: stored.label.clone(),
                    snapshot_meta: SnapshotMeta {
                        target_type: if stored.target_type == "wsl" {
                            TargetType::Wsl
                        } else {
                            TargetType::Windows
                        },
                        distro_name: stored.distro_name.clone(),
                        home: stored.home.clone(),
                    },
                },
                manifest_index: index,
            })
        })
        .collect()
}

fn stored_target(target: &CliTarget, files: Vec<ManagedFile>) -> ManagedTarget {
    ManagedTarget {
        rebuild_from_backup: false,
        target_type: match target.snapshot_meta.target_type {
            TargetType::Windows => "native".into(),
            TargetType::Wsl => "wsl".into(),
        },
        distro_name: target.snapshot_meta.distro_name.clone(),
        home: target.snapshot_meta.home.clone(),
        native_home: target.backend.root_hint(),
        base_url: target.base_url.clone(),
        installed: target.installed.into(),
        label: target.label.clone(),
        files,
        extra_env_keys: HashSet::new(),
        legacy_snapshot: None,
        pending: false,
        pending_reason: None,
    }
}

fn verify_origin(target: &CliTarget, file: &ManagedFile) -> Result<(), AppError> {
    verify_origin_for_backend(&*target.backend, file)
}

/// If the user deletes both the working file and its original sidecar, absence
/// becomes the new baseline. Never recapture an existing Relay working file.
fn reconcile_deleted_origin(
    backend: &dyn CliBackend,
    file: &mut ManagedFile,
) -> Result<bool, AppError> {
    if !file.origin.exists {
        return Ok(false);
    }
    let origin = sidecar_path(&file.path, ORIGIN_SUFFIX)?;
    if backend.exists(&refs(&origin))? || backend.exists(&refs(&file.path))? {
        return Ok(false);
    }
    file.origin = state_from(None, true);
    Ok(true)
}

fn verify_origin_for_backend(backend: &dyn CliBackend, file: &ManagedFile) -> Result<(), AppError> {
    let origin = sidecar_path(&file.path, ORIGIN_SUFFIX)?;
    verify_sidecar(backend, &refs(&origin), &file.origin).map_err(|error| {
        AppError::Config(format!(
            "Cannot verify original backup {}: {error}",
            origin.join("/")
        ))
    })
}

fn capture_origin(
    target: &CliTarget,
    file: &mut ManagedFile,
    reject_existing_sidecar: bool,
) -> Result<(), AppError> {
    let origin = sidecar_path(&file.path, ORIGIN_SUFFIX)?;
    let origin_refs = refs(&origin);
    if reject_existing_sidecar && target.backend.exists(&origin_refs)? {
        return Err(AppError::Config(format!(
            "orphan origin sidecar exists for {}:{}",
            target.label,
            file.path.join("/")
        )));
    }
    let current = target.backend.read_bytes(&refs(&file.path))?;
    write_state(&*target.backend, &origin_refs, current.as_deref())?;
    file.origin = state_from(current.as_deref(), true);
    Ok(())
}

fn managed_file(path: &[&str], provider: Provider) -> ManagedFile {
    ManagedFile {
        path: path.iter().map(|part| part.to_string()).collect(),
        provider,
        origin: StoredFileState::default(),
        backup: StoredFileState::default(),
        touched: false,
        restored: false,
        error: None,
        managed: true,
    }
}

fn descriptors(installed: InstalledTools, shell_path: Option<Vec<String>>) -> Vec<ManagedFile> {
    let mut files = Vec::new();
    if installed.claude {
        files.push(managed_file(
            &[".claude", "settings.json"],
            Provider::Claude,
        ));
        files.push(managed_file(&[".claude.json"], Provider::Claude));
    }
    if installed.codex {
        files.push(managed_file(&[".codex", "auth.json"], Provider::Codex));
        files.push(managed_file(&[".codex", "config.toml"], Provider::Codex));
        if let Some(path) = shell_path {
            files.push(ManagedFile {
                path,
                provider: Provider::Codex,
                origin: StoredFileState::default(),
                backup: StoredFileState::default(),
                touched: false,
                restored: false,
                error: None,
                managed: true,
            });
        }
    }
    if installed.gemini {
        files.push(managed_file(&[".gemini", ".env"], Provider::Gemini));
        files.push(managed_file(
            &[".gemini", "settings.json"],
            Provider::Gemini,
        ));
    }
    files
}

fn backend_for(target: &ManagedTarget) -> Result<Box<dyn CliBackend>, AppError> {
    if target.target_type == "wsl" {
        let distro = target
            .distro_name
            .clone()
            .ok_or_else(|| AppError::Config("WSL lifecycle target has no distro".into()))?;
        let home = target
            .home
            .clone()
            .ok_or_else(|| AppError::Config("WSL lifecycle target has no home".into()))?;
        Ok(Box::new(WslBackend { distro, home }))
    } else if target.target_type == "native" {
        let home = target
            .native_home
            .as_ref()
            .ok_or_else(|| AppError::Config("native lifecycle target has no root".into()))?;
        let current = current_native_home()?;
        if normalize_native_path(std::path::Path::new(home)) != normalize_native_path(&current) {
            return Err(AppError::Config(
                "native lifecycle root does not match the current user home".into(),
            ));
        }
        Ok(Box::new(WindowsFsBackend {
            home: std::path::PathBuf::from(home),
        }))
    } else {
        Err(AppError::Config("invalid lifecycle target type".into()))
    }
}

fn validate_manifest(manifest: &LifecycleManifest) -> Result<(), AppError> {
    if manifest.version != MANIFEST_VERSION {
        return Err(AppError::Config(format!(
            "unsupported CLI lifecycle manifest version {}",
            manifest.version
        )));
    }
    let mut identities = HashSet::new();
    for target in &manifest.targets {
        if target.target_type != "native" && target.target_type != "wsl" {
            return Err(AppError::Config("invalid lifecycle target type".into()));
        }
        if !identities.insert(stored_key(target)) {
            return Err(AppError::Config("duplicate lifecycle target".into()));
        }
        for file in &target.files {
            validate_path(&file.path)?;
        }
    }
    Ok(())
}

fn validate_target_roots(manifest: &LifecycleManifest) -> Result<(), AppError> {
    let current_home = current_native_home()?;
    let normalized_current = normalize_native_path(&current_home);
    for target in &manifest.targets {
        if target.target_type == "native" {
            let stored = target
                .native_home
                .as_deref()
                .ok_or_else(|| AppError::Config("native lifecycle target has no root".into()))?;
            if normalize_native_path(std::path::Path::new(stored)) != normalized_current {
                return Err(AppError::Config(format!(
                    "native lifecycle root does not match the current user home"
                )));
            }
        } else if target.target_type == "wsl"
            && (target.distro_name.as_deref().unwrap_or_default().is_empty()
                || (!target.pending && target.home.as_deref().unwrap_or_default().is_empty()))
        {
            return Err(AppError::Config(
                "WSL lifecycle target identity is incomplete".into(),
            ));
        }
    }
    Ok(())
}

fn current_native_home() -> Result<std::path::PathBuf, AppError> {
    #[cfg(test)]
    if let Some(home) = std::env::var_os("LLM_RELAY_TEST_NATIVE_HOME") {
        return Ok(home.into());
    }
    dirs::home_dir().ok_or_else(|| AppError::Config("cannot resolve current native home".into()))
}

fn normalize_native_path(path: &std::path::Path) -> String {
    let value = path
        .canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .replace('\\', "/");
    if cfg!(windows) {
        value.to_ascii_lowercase()
    } else {
        value
    }
}

fn validate_path(path: &[String]) -> Result<(), AppError> {
    if path.is_empty()
        || path.iter().any(|segment| {
            segment.is_empty()
                || segment == "."
                || segment == ".."
                || segment.contains('/')
                || segment.contains('\\')
        })
    {
        return Err(AppError::Config("invalid managed lifecycle path".into()));
    }
    Ok(())
}

fn sidecar_path(path: &[String], suffix: &str) -> Result<Vec<String>, AppError> {
    validate_path(path)?;
    let mut sidecar = path.to_vec();
    let file = sidecar
        .last_mut()
        .ok_or_else(|| AppError::Config("empty lifecycle path".into()))?;
    file.push_str(suffix);
    Ok(sidecar)
}

fn refs(path: &[String]) -> Vec<&str> {
    path.iter().map(String::as_str).collect()
}

fn write_state(
    backend: &dyn CliBackend,
    sidecar: &[&str],
    content: Option<&[u8]>,
) -> Result<(), AppError> {
    backend.write_atomic(sidecar, content.unwrap_or_default())
}

fn restore_state(
    backend: &dyn CliBackend,
    working: &[&str],
    sidecar: &[&str],
    state: &StoredFileState,
) -> Result<(), AppError> {
    if !state.complete {
        return Err(AppError::Config(
            "incomplete lifecycle sidecar state".into(),
        ));
    }
    if state.exists {
        // A locally removed backup is not permission to delete the working
        // file or capture Relay's configuration as the original. Leave it as-is.
        let Some(content) = backend.read_bytes(sidecar)? else {
            return Ok(());
        };
        // Local users own backup contents; restore their current bytes verbatim.
        backend.write_atomic(working, &content)
    } else {
        backend.remove(working)
    }
}

fn verify_sidecar(
    backend: &dyn CliBackend,
    sidecar: &[&str],
    state: &StoredFileState,
) -> Result<(), AppError> {
    if !state.exists {
        return Ok(());
    }
    // Only actual read errors block apply; users may remove local backups.
    backend.read_bytes(sidecar)?;
    Ok(())
}

fn state_from(content: Option<&[u8]>, complete: bool) -> StoredFileState {
    StoredFileState {
        exists: content.is_some(),
        complete,
    }
}

fn capture_host_openai_api_key() -> Option<StoredHostValue> {
    if cfg!(test) {
        return None;
    }
    #[cfg(target_os = "windows")]
    {
        let output = std::process::Command::new("reg")
            .args(["query", "HKCU\\Environment", "/v", "OPENAI_API_KEY"])
            .output()
            .ok()?;
        if !output.status.success() {
            return Some(StoredHostValue::default());
        }
        let value = String::from_utf8_lossy(&output.stdout)
            .lines()
            .find(|line| line.contains("OPENAI_API_KEY"))
            .and_then(|line| line.split_whitespace().nth(2))
            .unwrap_or_default()
            .to_string();
        Some(StoredHostValue {
            exists: true,
            relay_owned: matches!(value.as_str(), "dummy" | "llm-relay-ignore"),
            value,
        })
    }
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("launchctl")
            .args(["getenv", "OPENAI_API_KEY"])
            .output()
            .ok()?;
        let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Some(StoredHostValue {
            exists: output.status.success() && !value.is_empty(),
            relay_owned: matches!(value.as_str(), "dummy" | "llm-relay-ignore"),
            value,
        })
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        None
    }
}

fn restore_host_openai_api_key(state: Option<&StoredHostValue>) -> Result<(), AppError> {
    let Some(state) = state else { return Ok(()) };
    if !state.relay_owned && state.exists {
        return Ok(());
    }
    #[cfg(target_os = "windows")]
    {
        let status = if state.exists {
            std::process::Command::new("setx")
                .args(["OPENAI_API_KEY", &state.value])
                .status()
        } else {
            std::process::Command::new("reg")
                .args(["delete", "HKCU\\Environment", "/v", "OPENAI_API_KEY", "/f"])
                .status()
        }
        .map_err(|error| AppError::Config(format!("restore OPENAI_API_KEY: {error}")))?;
        if !status.success() {
            return Err(AppError::Config("restore OPENAI_API_KEY failed".into()));
        }
    }
    #[cfg(target_os = "macos")]
    {
        let mut command = std::process::Command::new("launchctl");
        if state.exists {
            command.args(["setenv", "OPENAI_API_KEY", &state.value]);
        } else {
            command.args(["unsetenv", "OPENAI_API_KEY"]);
        }
        let status = command
            .status()
            .map_err(|error| AppError::Config(format!("restore launchctl env: {error}")))?;
        if !status.success() {
            return Err(AppError::Config("restore launchctl env failed".into()));
        }
    }
    if state.exists {
        std::env::set_var("OPENAI_API_KEY", &state.value);
    } else {
        std::env::remove_var("OPENAI_API_KEY");
    }
    Ok(())
}

fn target_key(meta: &SnapshotMeta) -> String {
    match meta.target_type {
        TargetType::Windows => "native".into(),
        TargetType::Wsl => format!("wsl:{}", meta.distro_name.as_deref().unwrap_or("")),
    }
}

fn stored_key(target: &ManagedTarget) -> String {
    if target.target_type == "wsl" {
        format!("wsl:{}", target.distro_name.as_deref().unwrap_or(""))
    } else {
        "native".into()
    }
}

fn report_key(target: &ManagedTarget) -> String {
    target
        .distro_name
        .clone()
        .unwrap_or_else(|| "windows".into())
}

pub fn restore_removed_targets(retained: &HashSet<String>) -> Result<(), AppError> {
    let Some(mut manifest) = load()? else {
        return Ok(());
    };
    for index in 0..manifest.targets.len() {
        if retained.contains(&report_key(&manifest.targets[index])) {
            continue;
        }
        let result = (|| -> Result<(), AppError> {
            let stored = &manifest.targets[index];
            let target = CliTarget {
                backend: backend_for(stored)?,
                base_url: stored.base_url.clone(),
                installed: stored.installed.into(),
                label: stored.label.clone(),
                snapshot_meta: SnapshotMeta {
                    target_type: if stored.target_type == "wsl" {
                        TargetType::Wsl
                    } else {
                        TargetType::Windows
                    },
                    distro_name: stored.distro_name.clone(),
                    home: stored.home.clone(),
                },
            };
            resume_missing_backups(&mut manifest, &target)?;
            migration::resume(&mut manifest, &target)?;
            let stored = &mut manifest.targets[index];
            for file in &mut stored.files {
                if !file.managed {
                    continue;
                }
                reconcile_deleted_origin(&*target.backend, file)?;
                let origin = sidecar_path(&file.path, ORIGIN_SUFFIX)?;
                restore_state(
                    &*target.backend,
                    &refs(&file.path),
                    &refs(&origin),
                    &file.origin,
                )?;
                file.managed = false;
                file.restored = true;
            }
            #[cfg(target_os = "windows")]
            if let Some(distro) = &stored.distro_name {
                crate::wsl::hosts::clear_hosts_entry(distro, &crate::wsl::hosts::relay_hostname())?;
            }
            stored.pending = false;
            stored.pending_reason = None;
            Ok(())
        })();
        if let Err(error) = result {
            if manifest.targets[index].target_type != "wsl" {
                return Err(error);
            }
            manifest.targets[index].pending = true;
            manifest.targets[index].pending_reason =
                Some(format!("Waiting to restore WSL configuration: {error}"));
        }
    }
    save(&mut manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli_target::WindowsFsBackend;
    use std::sync::{Mutex, MutexGuard, OnceLock};
    use tempfile::TempDir;

    fn env_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    fn target(home: &std::path::Path) -> CliTarget {
        CliTarget {
            backend: Box::new(WindowsFsBackend {
                home: home.to_path_buf(),
            }),
            base_url: "http://relay".into(),
            installed: InstalledTools {
                claude: true,
                codex: false,
                gemini: false,
            },
            label: "native".into(),
            snapshot_meta: SnapshotMeta {
                target_type: TargetType::Windows,
                distro_name: None,
                home: None,
            },
        }
    }

    #[test]
    fn appends_suffix_to_the_complete_filename() {
        assert_eq!(
            sidecar_path(&[".claude".into(), "settings.json".into()], ORIGIN_SUFFIX).unwrap(),
            vec![".claude", "settings.json.llm-relay.origin"]
        );
        assert_eq!(
            sidecar_path(&[".claude.json".into()], BACKUP_SUFFIX).unwrap(),
            vec![".claude.json.llm-relay.bak"]
        );
    }

    #[test]
    fn absent_and_empty_have_distinct_states() {
        let absent = state_from(None, true);
        let empty = state_from(Some(&[]), true);
        assert!(!absent.exists);
        assert!(empty.exists);
    }

    #[tokio::test]
    async fn disable_stops_proxy_even_when_restore_fails_and_stays_disabled() {
        let env = MigrationEnv::new();
        crate::keystore::init_test();
        let db = std::sync::Arc::new(crate::Database::open_in_memory().unwrap());
        let service = crate::Service::new(db.clone(), std::sync::Arc::new(crate::events::NullSink));
        let primary = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = primary.local_addr().unwrap();
        let proxy = crate::proxy_server::start_with_listeners(
            crate::proxy_server::ProxyState::new(
                db.clone(),
                service.switch_lock.clone(),
                service.sink.clone(),
            ),
            primary,
            None,
        )
        .await;
        let service = service.with_proxy(proxy.clone());
        // A stale active selection survives a restore error, but cannot restart
        // the listener or trigger an automatic gateway switch.
        let mut active = db.get_active_config().unwrap();
        active.gateway_id = Some(uuid::Uuid::new_v4().to_string());
        db.set_active_config(&active).unwrap();
        std::fs::write(
            env.config.path().join("cli-file-lifecycle.json"),
            b"invalid manifest",
        )
        .unwrap();
        assert!(service.clear_active().await.is_err());
        assert!(service.relay_disabled().unwrap());
        assert!(!proxy.is_running());
        let _other_owner = std::net::TcpListener::bind(addr).unwrap();
        service.start_proxy_if_enabled().await.unwrap();
        assert!(!proxy.is_running());
        let models = crate::ipc::protocol::ModelSelection {
            claude: None,
            claude_subagent: None,
            claude_small: None,
            codex: None,
            codex_subagent: None,
            gemini: None,
            claude_extra: crate::ipc::protocol::ClaudeExtraSelection::Inherit,
        };
        assert!(!service
            .auto_set_active(
                uuid::Uuid::new_v4(),
                uuid::Uuid::new_v4(),
                models.clone(),
                &active
            )
            .await
            .unwrap());
        std::fs::remove_file(env.config.path().join("cli-file-lifecycle.json")).unwrap();
        // Explicit Use must report port ownership before touching CLI files.
        assert!(service
            .set_active(uuid::Uuid::new_v4(), uuid::Uuid::new_v4(), models.clone())
            .await
            .unwrap_err()
            .to_string()
            .contains("Local proxy port"));
        assert!(!manifest_exists());
        service.clear_active().await.unwrap();
        assert!(db.get_active_config().unwrap().gateway_id.is_none());
        assert!(!proxy.is_running());
        drop(_other_owner);
        // If activation fails after binding, the newly acquired port is released.
        assert!(service
            .set_active(uuid::Uuid::new_v4(), uuid::Uuid::new_v4(), models)
            .await
            .is_err());
        assert!(!proxy.is_running());
        assert!(service.relay_disabled().unwrap());
        std::net::TcpListener::bind(addr).unwrap();
    }

    #[test]
    fn legacy_file_state_ignores_checksum() {
        let state: StoredFileState =
            serde_json::from_str(r#"{"exists":true,"sha256":"old-checksum","complete":true}"#)
                .unwrap();
        assert!(state.exists && state.complete);
        assert!(serde_json::to_value(state).unwrap().get("sha256").is_none());
    }

    #[test]
    fn locally_edited_sidecars_are_used_for_apply_disable_and_next_use() {
        let env = MigrationEnv::new();
        let native = target(env.home.path());
        let working = [".claude", "settings.json"];
        let origin = [".claude", "settings.json.llm-relay.origin"];
        let backup = [".claude", "settings.json.llm-relay.bak"];
        native.backend.write_atomic(&working, b"original").unwrap();
        let mut manifest = prepare_use(&[target(env.home.path())], &[], &BTreeMap::new()).unwrap();
        mark_active(&mut manifest).unwrap();
        native.backend.write_atomic(&working, b"relay").unwrap();
        native
            .backend
            .write_atomic(&origin, b"edited origin")
            .unwrap();

        prepare_active_apply(&[target(env.home.path())], &BTreeMap::new()).unwrap();
        assert_eq!(
            native.backend.read_bytes(&working).unwrap().unwrap(),
            b"relay"
        );
        disable().unwrap();
        assert_eq!(
            native.backend.read_bytes(&working).unwrap().unwrap(),
            b"edited origin"
        );

        native
            .backend
            .write_atomic(&backup, b"edited backup")
            .unwrap();
        prepare_use(&[target(env.home.path())], &[], &BTreeMap::new()).unwrap();
        assert_eq!(
            native.backend.read_bytes(&working).unwrap().unwrap(),
            b"edited backup"
        );
    }

    struct MigrationEnv {
        _lock: MutexGuard<'static, ()>,
        home: TempDir,
        config: TempDir,
        old_home: Option<std::ffi::OsString>,
        old_native: Option<std::ffi::OsString>,
    }

    impl MigrationEnv {
        fn new() -> Self {
            let guard = env_lock();
            let home = TempDir::new().unwrap();
            let config = TempDir::new().unwrap();
            let old_home = std::env::var_os("LLM_RELAY_HOME");
            let old_native = std::env::var_os("LLM_RELAY_TEST_NATIVE_HOME");
            std::env::set_var("LLM_RELAY_HOME", config.path());
            std::env::set_var("LLM_RELAY_TEST_NATIVE_HOME", home.path());
            Self {
                _lock: guard,
                home,
                config,
                old_home,
                old_native,
            }
        }
    }

    impl Drop for MigrationEnv {
        fn drop(&mut self) {
            for (key, value) in [
                ("LLM_RELAY_HOME", &self.old_home),
                ("LLM_RELAY_TEST_NATIVE_HOME", &self.old_native),
            ] {
                if let Some(value) = value {
                    std::env::set_var(key, value);
                } else {
                    std::env::remove_var(key);
                }
            }
        }
    }

    #[test]
    fn migration_then_toggle_uses_only_full_file_origins() {
        let env = MigrationEnv::new();
        let native = target(env.home.path());
        let settings = [".claude", "settings.json"];
        native.backend.write_atomic(&settings, br#"{"env":{"ANTHROPIC_BASE_URL":"https://original","KEEP":"original-extra"},"permissions":{"allow":[]}}"#).unwrap();
        super::super::snapshot::capture(&native).unwrap();
        native.backend.write_atomic(&settings, br#"{"env":{"ANTHROPIC_BASE_URL":"http://relay","ANTHROPIC_AUTH_TOKEN":"relay-token","KEEP":"original-extra"},"permissions":{"allow":["Read"]}}"#).unwrap();
        let working = native.backend.read_bytes(&settings).unwrap();
        migrate_legacy_active().unwrap();
        assert_eq!(native.backend.read_bytes(&settings).unwrap(), working);
        assert!(!super::super::snapshot::has_legacy_snapshots().unwrap());
        assert!(
            std::fs::read_dir(env.config.path().join("cli-config-backup"))
                .unwrap()
                .any(|entry| entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains("migrated-"))
        );
        let origin_path = [".claude", "settings.json.llm-relay.origin"];
        let origin = native.backend.read_bytes(&origin_path).unwrap();
        let value: serde_json::Value = serde_json::from_slice(origin.as_ref().unwrap()).unwrap();
        assert_eq!(value["env"]["ANTHROPIC_BASE_URL"], "https://original");
        assert_eq!(value["permissions"]["allow"][0], "Read");
        assert!(value["env"].get("ANTHROPIC_AUTH_TOKEN").is_none());

        let mut off = target(env.home.path());
        off.installed.claude = false;
        off.installed.codex = true;
        prepare_active_apply(&[off], &BTreeMap::new()).unwrap();
        assert_eq!(native.backend.read_bytes(&settings).unwrap(), origin);
        let mut prepared = prepare_active_apply(&[target(env.home.path())], &BTreeMap::new())
            .unwrap()
            .unwrap();
        let extra = BTreeMap::from([
            ("KEEP".into(), "override".into()),
            ("NEW_EXTRA".into(), "new".into()),
        ]);
        let snap = snapshot_for_apply(&native, Some(&extra)).unwrap();
        assert_eq!(
            snap.claude.extra_env_originals["KEEP"].as_deref(),
            Some("original-extra")
        );
        assert_eq!(snap.claude.extra_env_originals["NEW_EXTRA"], None);
        super::super::write_one_target(
            &native,
            "relay",
            Some("claude-test"),
            None,
            None,
            None,
            None,
            Some(&extra),
            Some(&snap),
        )
        .unwrap();
        mark_targets_active(&mut prepared, &HashSet::from(["windows".into()])).unwrap();
        assert!(!load().unwrap().unwrap().targets[0].pending);
        let snap = snapshot_for_apply(&native, None).unwrap();
        super::super::write_one_target(
            &native,
            "relay",
            Some("claude-test"),
            None,
            None,
            None,
            None,
            None,
            Some(&snap),
        )
        .unwrap();
        let value: serde_json::Value =
            serde_json::from_str(&native.backend.read(&settings).unwrap().unwrap()).unwrap();
        assert_eq!(value["env"]["KEEP"], "original-extra");
        assert!(value["env"].get("NEW_EXTRA").is_none());
        assert_eq!(native.backend.read_bytes(&origin_path).unwrap(), origin);
        assert!(!super::super::snapshot::snapshot_path(&native.snapshot_meta).exists());
        disable().unwrap();
        assert_eq!(native.backend.read_bytes(&settings).unwrap(), origin);
    }

    #[test]
    fn migration_rejects_conflicting_sidecar_without_changing_live_files() {
        let env = MigrationEnv::new();
        let native = target(env.home.path());
        native
            .backend
            .write_atomic(&[".claude", "settings.json"], b"{}")
            .unwrap();
        super::super::snapshot::capture(&native).unwrap();
        native
            .backend
            .write_atomic(
                &[".claude", "settings.json.llm-relay.origin"],
                b"unrelated backup",
            )
            .unwrap();
        assert!(migrate_legacy_active()
            .unwrap_err()
            .to_string()
            .contains("conflicting origin"));
        assert!(!manifest_exists());
        assert!(super::super::snapshot::has_legacy_snapshots().unwrap());
        assert_eq!(
            native
                .backend
                .read(&[".claude", "settings.json"])
                .unwrap()
                .as_deref(),
            Some("{}")
        );
    }

    #[test]
    fn migration_failure_can_retry_with_matching_sidecars() {
        let env = MigrationEnv::new();
        let native = target(env.home.path());
        super::super::snapshot::capture(&native).unwrap();
        // Simulate interruption after the first (absent-file) sidecar write.
        native
            .backend
            .write_atomic(&[".claude", "settings.json.llm-relay.origin"], b"")
            .unwrap();
        migrate_legacy_active().unwrap();
        migrate_legacy_active().unwrap();
        assert!(manifest_exists());
        assert!(!native
            .backend
            .exists(&[".claude", "settings.json"])
            .unwrap());
        assert!(!super::super::snapshot::has_legacy_snapshots().unwrap());
    }

    #[test]
    fn migration_does_not_silently_skip_an_invalid_target() {
        let env = MigrationEnv::new();
        let native = target(env.home.path());
        super::super::snapshot::capture(&native).unwrap();
        std::fs::write(
            crate::paths::cli_config_backup_dir().join("broken.json"),
            b"{",
        )
        .unwrap();
        assert!(migrate_legacy_active().is_err());
        assert!(!manifest_exists());
        assert!(super::super::snapshot::snapshot_path(&native.snapshot_meta).exists());
        assert!(!native
            .backend
            .exists(&[".claude", "settings.json.llm-relay.origin"])
            .unwrap());
    }

    struct SwitchableBackend {
        fs: WindowsFsBackend,
        online: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    #[test]
    fn deleted_config_and_origin_can_be_reenabled_then_restored_to_absence() {
        let env = MigrationEnv::new();
        let native = target(env.home.path());
        let settings = [".claude", "settings.json"];
        let origin = [".claude", "settings.json.llm-relay.origin"];
        native
            .backend
            .write_atomic(
                &settings,
                br#"{"env":{"ANTHROPIC_BASE_URL":"https://original"}}"#,
            )
            .unwrap();
        let mut manifest = prepare_use(&[target(env.home.path())], &[], &BTreeMap::new()).unwrap();
        mark_active(&mut manifest).unwrap();
        native.backend.remove(&settings).unwrap();
        native.backend.remove(&origin).unwrap();
        prepare_active_apply(&[target(env.home.path())], &BTreeMap::new()).unwrap();
        let stored = load().unwrap().unwrap();
        assert!(!stored.targets[0].files[0].origin.exists);
        let snapshot = snapshot_for_apply(&native, None).unwrap();
        super::super::write_one_target(
            &native,
            "relay",
            Some("claude-test"),
            None,
            None,
            None,
            None,
            None,
            Some(&snapshot),
        )
        .unwrap();
        assert!(native.backend.exists(&settings).unwrap());
        let mut off = target(env.home.path());
        off.installed.claude = false;
        off.installed.codex = true;
        prepare_active_apply(&[off], &BTreeMap::new()).unwrap();
        assert!(!native.backend.exists(&settings).unwrap());
    }

    #[test]
    fn disable_accepts_deleted_config_and_origin() {
        let env = MigrationEnv::new();
        let native = target(env.home.path());
        let settings = [".claude", "settings.json"];
        native.backend.write_atomic(&settings, b"{}").unwrap();
        let mut manifest = prepare_use(&[target(env.home.path())], &[], &BTreeMap::new()).unwrap();
        mark_active(&mut manifest).unwrap();
        native.backend.remove(&settings).unwrap();
        native
            .backend
            .remove(&[".claude", "settings.json.llm-relay.origin"])
            .unwrap();
        disable().unwrap();
        assert!(!native.backend.exists(&settings).unwrap());
        assert_eq!(load().unwrap().unwrap().phase, LifecyclePhase::Inactive);
    }

    #[test]
    fn missing_origin_allows_apply_and_disable_preserves_working_file() {
        let env = MigrationEnv::new();
        let native = target(env.home.path());
        native
            .backend
            .write_atomic(&[".claude", "settings.json"], b"{}")
            .unwrap();
        let mut manifest = prepare_use(&[target(env.home.path())], &[], &BTreeMap::new()).unwrap();
        mark_active(&mut manifest).unwrap();
        native
            .backend
            .remove(&[".claude", "settings.json.llm-relay.origin"])
            .unwrap();
        prepare_active_apply(&[target(env.home.path())], &BTreeMap::new()).unwrap();
        assert!(load().unwrap().unwrap().targets[0].files[0].origin.exists);
        disable().unwrap();
        assert_eq!(
            native
                .backend
                .read_bytes(&[".claude", "settings.json"])
                .unwrap()
                .unwrap(),
            b"{}"
        );
        assert!(!native
            .backend
            .exists(&[".claude", "settings.json.llm-relay.origin"])
            .unwrap());
        assert_eq!(load().unwrap().unwrap().phase, LifecyclePhase::Inactive);
    }

    #[test]
    fn missing_backup_allows_next_use_without_deleting_working_file() {
        let env = MigrationEnv::new();
        let native = target(env.home.path());
        let working = [".claude", "settings.json"];
        native.backend.write_atomic(&working, b"original").unwrap();
        let mut manifest = prepare_use(&[target(env.home.path())], &[], &BTreeMap::new()).unwrap();
        mark_active(&mut manifest).unwrap();
        native.backend.write_atomic(&working, b"relay").unwrap();
        disable().unwrap();
        native
            .backend
            .remove(&[".claude", "settings.json.llm-relay.bak"])
            .unwrap();
        prepare_use(&[target(env.home.path())], &[], &BTreeMap::new()).unwrap();
        assert_eq!(
            native.backend.read_bytes(&working).unwrap().unwrap(),
            b"original"
        );
    }

    impl SwitchableBackend {
        fn check(&self) -> Result<(), AppError> {
            if self.online.load(std::sync::atomic::Ordering::SeqCst) {
                Ok(())
            } else {
                Err(AppError::Config(
                    "wsl.exe exited with exit code: 0xffffffff".into(),
                ))
            }
        }
    }

    impl CliBackend for SwitchableBackend {
        fn read_bytes(&self, path: &[&str]) -> Result<Option<Vec<u8>>, AppError> {
            self.check()?;
            self.fs.read_bytes(path)
        }
        fn write_atomic(&self, path: &[&str], bytes: &[u8]) -> Result<(), AppError> {
            self.check()?;
            self.fs.write_atomic(path, bytes)
        }
        fn remove(&self, path: &[&str]) -> Result<(), AppError> {
            self.check()?;
            self.fs.remove(path)
        }
        fn exists(&self, path: &[&str]) -> Result<bool, AppError> {
            self.check()?;
            self.fs.exists(path)
        }
    }

    #[test]
    fn offline_wsl_migration_does_not_block_native_and_reconnect_restores_latest_selection() {
        let env = MigrationEnv::new();
        let native = target(env.home.path());
        super::super::snapshot::capture(&native).unwrap();
        let wsl_home = TempDir::new().unwrap();
        let online = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let wsl = CliTarget {
            backend: Box::new(SwitchableBackend {
                fs: WindowsFsBackend {
                    home: wsl_home.path().into(),
                },
                online: online.clone(),
            }),
            snapshot_meta: SnapshotMeta {
                target_type: TargetType::Wsl,
                distro_name: Some("Offline Test".into()),
                home: Some("/home/test".into()),
            },
            base_url: "http://relay".into(),
            label: "wsl:Offline Test".into(),
            installed: InstalledTools {
                claude: false,
                codex: false,
                gemini: false,
            },
        };
        wsl.backend
            .write_atomic(
                &[".claude", "settings.json"],
                br#"{"env":{"ANTHROPIC_BASE_URL":"https://original"}}"#,
            )
            .unwrap();
        super::super::snapshot::capture(&wsl).unwrap();
        wsl.backend
            .write_atomic(
                &[".claude", "settings.json"],
                br#"{"env":{"ANTHROPIC_BASE_URL":"http://relay"}}"#,
            )
            .unwrap();
        online.store(false, std::sync::atomic::Ordering::SeqCst);
        migrate_legacy_active().unwrap();
        assert!(has_pending_wsl());
        assert!(!super::super::snapshot::snapshot_path(&wsl.snapshot_meta).exists());
        // The serialized manifest is sufficient after a restart; no old JSON
        // file or access to the offline subsystem is needed to save settings.
        assert!(load()
            .unwrap()
            .unwrap()
            .targets
            .iter()
            .any(|t| t.legacy_snapshot.is_some()));
        let targets = [native, wsl];
        let mut manifest = prepare_active_apply(&targets, &BTreeMap::new())
            .unwrap()
            .unwrap();
        let retained = HashSet::from(["windows".into(), "Offline Test".into()]);
        let report = super::super::apply_to_targets(
            &targets,
            Some(&retained),
            "relay",
            Some("claude-test"),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(report.succeeded.contains("windows"));
        assert!(report.failed.contains_key("Offline Test"));
        assert!(report.failed["Offline Test"].contains("wsl.exe exited"));
        mark_targets_active(&mut manifest, &report.succeeded).unwrap();
        assert!(has_pending_wsl());
        online.store(true, std::sync::atomic::Ordering::SeqCst);
        let mut manifest = prepare_active_apply(&targets, &BTreeMap::new())
            .unwrap()
            .unwrap();
        let report = super::super::apply_to_targets(
            &targets,
            Some(&retained),
            "relay",
            Some("claude-test"),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        mark_targets_active(&mut manifest, &report.succeeded).unwrap();
        assert!(!has_pending_wsl());
        assert!(load()
            .unwrap()
            .unwrap()
            .targets
            .iter()
            .all(|t| t.legacy_snapshot.is_none()));
        let restored: serde_json::Value = serde_json::from_str(
            &targets[1]
                .backend
                .read(&[".claude", "settings.json"])
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(restored["env"]["ANTHROPIC_BASE_URL"], "https://original");
    }

    #[test]
    fn active_pre_manifest_recovery_allows_missing_backups() {
        let _env_guard = env_lock();
        let manifest_home = TempDir::new().unwrap();
        let previous_home = std::env::var_os("LLM_RELAY_HOME");
        std::env::set_var("LLM_RELAY_HOME", manifest_home.path());

        recover(true).unwrap();
        migrate_legacy_active().unwrap();
        assert!(!manifest_exists());

        if let Some(value) = previous_home {
            std::env::set_var("LLM_RELAY_HOME", value);
        } else {
            std::env::remove_var("LLM_RELAY_HOME");
        }
    }

    #[test]
    fn active_pre_manifest_recovery_accepts_a_legacy_snapshot() {
        let _env_guard = env_lock();
        let tmp = TempDir::new().unwrap();
        let manifest_home = TempDir::new().unwrap();
        let previous_home = std::env::var_os("LLM_RELAY_HOME");
        std::env::set_var("LLM_RELAY_HOME", manifest_home.path());

        super::super::snapshot::capture(&target(tmp.path())).unwrap();
        recover(true).unwrap();
        assert!(!manifest_exists());

        if let Some(value) = previous_home {
            std::env::set_var("LLM_RELAY_HOME", value);
        } else {
            std::env::remove_var("LLM_RELAY_HOME");
        }
    }

    #[test]
    fn missing_history_generates_from_empty_and_disable_restores_absence() {
        let env = MigrationEnv::new();
        let native = target(env.home.path());
        let settings = [".claude", "settings.json"];
        native
            .backend
            .write_atomic(&settings, br#"{"stale":true}"#)
            .unwrap();
        let targets = [target(env.home.path())];
        prepare_missing_active(&targets, &[]).unwrap();
        // Preparation is durable without changing working files yet.
        assert!(load().unwrap().unwrap().targets[0].rebuild_from_backup);
        let mut manifest = prepare_active_apply(&targets, &BTreeMap::new())
            .unwrap()
            .unwrap();
        assert!(!native.backend.exists(&settings).unwrap());
        let report = super::super::apply_to_targets(
            &targets,
            None,
            "relay",
            Some("claude-test"),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        mark_targets_active(&mut manifest, &report.succeeded).unwrap();
        let value: serde_json::Value =
            serde_json::from_str(&native.backend.read(&settings).unwrap().unwrap()).unwrap();
        assert!(value.get("stale").is_none());
        assert_eq!(value["env"]["ANTHROPIC_MODEL"], "claude-test[1m]");
        assert!(!manifest.targets[0].files[0].origin.exists);
        // Reapply must not reset newly generated files a second time.
        native
            .backend
            .write_atomic(&settings, br#"{"keep":true}"#)
            .unwrap();
        prepare_missing_active(&targets, &[]).unwrap();
        prepare_active_apply(&targets, &BTreeMap::new()).unwrap();
        assert!(native
            .backend
            .read(&settings)
            .unwrap()
            .unwrap()
            .contains("keep"));
        disable().unwrap();
        assert!(!native.backend.exists(&settings).unwrap());
        assert!(!native.backend.exists(&[".claude.json"]).unwrap());
    }

    #[test]
    fn missing_history_uses_available_backup_and_preserves_origin() {
        let env = MigrationEnv::new();
        let native = target(env.home.path());
        let settings = [".claude", "settings.json"];
        let origin = br#"{"original":true}"#;
        let backup = br#"{"fromBackup":true}"#;
        native
            .backend
            .write_atomic(&settings, br#"{"stale":true}"#)
            .unwrap();
        native
            .backend
            .write_atomic(&[".claude", "settings.json.llm-relay.origin"], origin)
            .unwrap();
        native
            .backend
            .write_atomic(&[".claude", "settings.json.llm-relay.bak"], backup)
            .unwrap();
        let targets = [target(env.home.path())];
        prepare_missing_active(&targets, &[]).unwrap();
        prepare_active_apply(&targets, &BTreeMap::new()).unwrap();
        assert_eq!(
            native.backend.read_bytes(&settings).unwrap().unwrap(),
            backup
        );
        disable().unwrap();
        assert_eq!(
            native.backend.read_bytes(&settings).unwrap().unwrap(),
            origin
        );
    }

    #[test]
    fn missing_history_retains_offline_targets_for_empty_rebuild() {
        let env = MigrationEnv::new();
        let wsl_home = TempDir::new().unwrap();
        let online = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let wsl = CliTarget {
            backend: Box::new(SwitchableBackend {
                fs: WindowsFsBackend {
                    home: wsl_home.path().into(),
                },
                online: online.clone(),
            }),
            base_url: "http://relay".into(),
            installed: InstalledTools::ALL,
            label: "wsl:Offline".into(),
            snapshot_meta: SnapshotMeta {
                target_type: TargetType::Wsl,
                distro_name: Some("Offline".into()),
                home: Some("/home/test".into()),
            },
        };
        let pending = crate::service::PendingWslTarget {
            name: "Offline".into(),
            home: Some("/home/test".into()),
            installed: InstalledTools::ALL,
            reason: "offline".into(),
        };
        prepare_missing_active(&[target(env.home.path())], &[pending]).unwrap();
        let manifest = load().unwrap().unwrap();
        assert!(manifest.targets[1].rebuild_from_backup);
        assert!(manifest.targets[1].pending);
        assert_eq!(manifest.targets[1].files.len(), 6);
        assert!(manifest.targets[1].files.iter().all(|f| !f.origin.exists));
        let targets = [wsl];
        prepare_active_apply(&targets, &BTreeMap::new()).unwrap();
        assert!(load().unwrap().unwrap().targets[1].rebuild_from_backup);
        online.store(true, std::sync::atomic::Ordering::SeqCst);
        targets[0]
            .backend
            .write_atomic(&[".claude", "settings.json"], br#"{"stale":true}"#)
            .unwrap();
        prepare_active_apply(&targets, &BTreeMap::new()).unwrap();
        assert!(!targets[0]
            .backend
            .exists(&[".claude", "settings.json"])
            .unwrap());
        assert!(!load().unwrap().unwrap().targets[1].rebuild_from_backup);
    }

    #[test]
    fn pending_wsl_is_recorded_without_touching_its_files() {
        let _env_guard = env_lock();
        let tmp = TempDir::new().unwrap();
        let manifest_home = TempDir::new().unwrap();
        let previous_home = std::env::var_os("LLM_RELAY_HOME");
        std::env::set_var("LLM_RELAY_HOME", manifest_home.path());
        std::env::set_var("LLM_RELAY_TEST_NATIVE_HOME", tmp.path());
        let native = target(tmp.path());
        let pending = crate::service::PendingWslTarget {
            name: "Offline Distro".into(),
            home: None,
            installed: InstalledTools::ALL,
            reason: "WSL home has not been probed".into(),
        };
        let manifest = prepare_use(&[native], &[pending], &BTreeMap::new()).unwrap();
        let stored = manifest
            .targets
            .iter()
            .find(|target| target.distro_name.as_deref() == Some("Offline Distro"))
            .unwrap();
        assert!(stored.pending);
        assert!(stored.files.is_empty());
        assert_eq!(
            stored.pending_reason.as_deref(),
            Some("WSL home has not been probed")
        );

        if let Some(value) = previous_home {
            std::env::set_var("LLM_RELAY_HOME", value);
        } else {
            std::env::remove_var("LLM_RELAY_HOME");
        }
        std::env::remove_var("LLM_RELAY_TEST_NATIVE_HOME");
    }

    #[test]
    fn stale_native_root_is_rejected() {
        let current = TempDir::new().unwrap();
        let stale = TempDir::new().unwrap();
        let previous = std::env::var_os("LLM_RELAY_TEST_NATIVE_HOME");
        std::env::set_var("LLM_RELAY_TEST_NATIVE_HOME", current.path());
        let manifest = LifecycleManifest {
            version: MANIFEST_VERSION,
            phase: LifecyclePhase::PreparingUse,
            updated_at: "now".into(),
            targets: vec![ManagedTarget {
                rebuild_from_backup: false,
                target_type: "native".into(),
                distro_name: None,
                home: None,
                native_home: Some(stale.path().to_string_lossy().into_owned()),
                base_url: "http://relay".into(),
                installed: InstalledTools::ALL.into(),
                label: "native".into(),
                files: Vec::new(),
                extra_env_keys: HashSet::new(),
                legacy_snapshot: None,
                pending: false,
                pending_reason: None,
            }],
            host_openai_api_key: None,
        };
        assert!(validate_target_roots(&manifest).is_err());
        if let Some(previous) = previous {
            std::env::set_var("LLM_RELAY_TEST_NATIVE_HOME", previous);
        } else {
            std::env::remove_var("LLM_RELAY_TEST_NATIVE_HOME");
        }
    }

    #[test]
    fn rejects_escaping_manifest_paths() {
        assert!(validate_path(&["..".into(), "settings.json".into()]).is_err());
        assert!(validate_path(&[".claude/settings.json".into()]).is_err());
    }

    #[test]
    fn lifecycle_round_trip_copies_whole_files_and_distinguishes_absent() {
        let _env_guard = env_lock();
        let tmp = TempDir::new().unwrap();
        let manifest_home = TempDir::new().unwrap();
        let previous_home = std::env::var_os("LLM_RELAY_HOME");
        std::env::set_var("LLM_RELAY_HOME", manifest_home.path());
        std::env::set_var("LLM_RELAY_TEST_NATIVE_HOME", tmp.path());
        let initial_target = target(tmp.path());
        initial_target
            .backend
            .write_atomic(&[".claude", "settings.json"], b"original\n")
            .unwrap();

        let mut manifest = prepare_use(&[initial_target], &[], &BTreeMap::new()).unwrap();
        let handles = build_targets(&manifest).unwrap();
        let managed_target = &handles[0].target;
        assert_eq!(
            managed_target
                .backend
                .read_bytes(&[".claude", "settings.json.llm-relay.origin"])
                .unwrap()
                .unwrap(),
            b"original\n"
        );
        assert!(managed_target
            .backend
            .read_bytes(&[".claude.json.llm-relay.origin"])
            .unwrap()
            .is_some());
        assert!(!manifest.targets[0].files[1].origin.exists);

        managed_target
            .backend
            .write_atomic(&[".claude", "settings.json"], b"relay\n")
            .unwrap();
        managed_target
            .backend
            .write_atomic(&[".claude.json"], b"relay-state")
            .unwrap();
        mark_active(&mut manifest).unwrap();
        disable().unwrap();

        assert_eq!(
            managed_target
                .backend
                .read_bytes(&[".claude", "settings.json"])
                .unwrap()
                .unwrap(),
            b"original\n"
        );
        assert!(managed_target
            .backend
            .read_bytes(&[".claude.json"])
            .unwrap()
            .is_none());
        assert_eq!(
            managed_target
                .backend
                .read_bytes(&[".claude", "settings.json.llm-relay.bak"])
                .unwrap()
                .unwrap(),
            b"relay\n"
        );

        managed_target
            .backend
            .write_atomic(&[".claude", "settings.json"], b"user-edited-origin")
            .unwrap();
        let next_target = target(tmp.path());
        let next_manifest = prepare_use(&[next_target], &[], &BTreeMap::new()).unwrap();
        let next_handles = build_targets(&next_manifest).unwrap();
        assert_eq!(
            next_handles[0]
                .target
                .backend
                .read_bytes(&[".claude", "settings.json"])
                .unwrap()
                .unwrap(),
            b"relay\n"
        );
        assert_eq!(
            next_handles[0]
                .target
                .backend
                .read_bytes(&[".claude", "settings.json.llm-relay.origin"])
                .unwrap()
                .unwrap(),
            b"user-edited-origin"
        );

        if let Some(value) = previous_home {
            std::env::set_var("LLM_RELAY_HOME", value);
        } else {
            std::env::remove_var("LLM_RELAY_HOME");
        }
        std::env::remove_var("LLM_RELAY_TEST_NATIVE_HOME");
    }
}
