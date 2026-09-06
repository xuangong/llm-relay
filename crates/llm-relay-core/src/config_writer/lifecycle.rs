use crate::cli_target::{
    CliBackend, CliTarget, InstalledTools, SnapshotMeta, TargetType, WindowsFsBackend, WslBackend,
};
use crate::AppError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};

const MANIFEST_VERSION: u32 = 1;
const ORIGIN_SUFFIX: &str = ".llm-relay.origin";
const BACKUP_SUFFIX: &str = ".llm-relay.bak";

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
    pub sha256: String,
    #[serde(default)]
    pub complete: bool,
}

impl Default for StoredFileState {
    fn default() -> Self {
        Self {
            exists: false,
            sha256: sha256(&[]),
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
            target_type: "wsl".into(),
            distro_name: Some(pending.name.clone()),
            home: pending.home.clone(),
            native_home: None,
            base_url: String::new(),
            installed: pending.installed.into(),
            label: format!("wsl:{}", pending.name),
            files: Vec::new(),
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
        // Origins are immutable and verified below; resume instead of recapturing.
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
        let key = target_key(&target.snapshot_meta);
        let desired = descriptors(target.installed, shell_paths.get(&key).cloned());
        if let Some(index) = manifest
            .targets
            .iter()
            .position(|stored| stored_key(stored) == key)
        {
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
            manifest.targets[index].pending_reason = Some("Applying Relay configuration".into());
            changed = true;
        }
    }
    if changed {
        save(&mut manifest)?;
    }
    Ok(changed.then_some(manifest))
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
    manifest.phase = LifecyclePhase::Active;
    for target in &mut manifest.targets {
        if !keys.contains(&stored_key(target)) {
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
                backend.remove(&refs(&backup_path))?;
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
        if db_active {
            return Err(AppError::Config(
                "Active Relay configuration has no trusted full-file origin manifest".into(),
            ));
        }
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

pub fn has_pending_wsl() -> bool {
    load().ok().flatten().is_some_and(|manifest| {
        manifest
            .targets
            .iter()
            .any(|target| target.target_type == "wsl" && target.pending)
    })
}

pub fn mark_target_failed(key: &str, error: &str) -> Result<(), AppError> {
    let Some(mut manifest) = load()? else {
        return Ok(());
    };
    if let Some(target) = manifest
        .targets
        .iter_mut()
        .find(|target| stored_key(target) == key)
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
        pending: false,
        pending_reason: None,
    }
}

fn verify_origin(target: &CliTarget, file: &ManagedFile) -> Result<(), AppError> {
    verify_origin_for_backend(&*target.backend, file)
}

fn verify_origin_for_backend(backend: &dyn CliBackend, file: &ManagedFile) -> Result<(), AppError> {
    let origin = sidecar_path(&file.path, ORIGIN_SUFFIX)?;
    verify_sidecar(backend, &refs(&origin), &file.origin)
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
        let content = backend
            .read_bytes(sidecar)?
            .ok_or_else(|| AppError::Config("lifecycle sidecar is missing".into()))?;
        if sha256(&content) != state.sha256 {
            return Err(AppError::Config(
                "lifecycle sidecar checksum mismatch".into(),
            ));
        }
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
    let content = backend
        .read_bytes(sidecar)?
        .ok_or_else(|| AppError::Config("lifecycle sidecar is missing".into()))?;
    if sha256(&content) != state.sha256 {
        return Err(AppError::Config(
            "lifecycle sidecar checksum mismatch".into(),
        ));
    }
    Ok(())
}

fn state_from(content: Option<&[u8]>, complete: bool) -> StoredFileState {
    StoredFileState {
        exists: content.is_some(),
        sha256: sha256(content.unwrap_or_default()),
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

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
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
        assert_eq!(absent.sha256, empty.sha256);
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
                target_type: "native".into(),
                distro_name: None,
                home: None,
                native_home: Some(stale.path().to_string_lossy().into_owned()),
                base_url: "http://relay".into(),
                installed: InstalledTools::ALL.into(),
                label: "native".into(),
                files: Vec::new(),
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
