//! A successful official login relinquishes only this target's environment.
//! No credential contents or checksums are recorded in lifecycle metadata.
use super::*;
use serde_json::Value;

const APPLIED_SUFFIX: &str = ".llm-relay.applied";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LoginState {
    #[serde(default)]
    pub environment_disabled: bool,
    #[serde(default)]
    pub codex_armed: bool,
    #[serde(default)]
    pub claude: Option<ClaudeLogin>,
    #[serde(default)]
    pub released: Vec<ReleasedClient>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ClaudeLogin {
    account: Option<String>,
    organization: Option<String>,
    credentials_present: bool,
    // Unlike access-token expiry, this is not advanced by ordinary refresh.
    refresh_token_expires_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleasedClient {
    #[serde(default = "default_true")]
    pub official_login: bool,
    pub provider: Provider,
    pub cleanup_pending: bool,
    pub error: Option<String>,
    // Durable cleanup journal. Removed from ordinary restoration immediately,
    // so a crash cannot cause Disable to restore pre-login credentials.
    #[serde(default)]
    pub files: Vec<ManagedFile>,
}

impl LoginState {
    pub fn released(&self, provider: Provider) -> bool {
        self.released.iter().any(|entry| entry.provider == provider)
    }

    pub fn filter(&self, mut tools: InstalledTools) -> InstalledTools {
        if self.environment_disabled {
            return InstalledTools {
                claude: false,
                codex: false,
                gemini: false,
            };
        }
        tools.claude &= !self.released(Provider::Claude);
        tools.codex &= !self.released(Provider::Codex);
        tools.gemini &= !self.released(Provider::Gemini);
        tools
    }
}

fn read_json(backend: &dyn CliBackend, path: &[&str]) -> Result<Option<Value>, AppError> {
    backend
        .read(path)?
        .map(|s| serde_json::from_str(&s).map_err(AppError::from))
        .transpose()
}

fn nonempty(value: Option<&Value>) -> bool {
    value.and_then(Value::as_str).is_some_and(|s| !s.is_empty())
}

fn codex_logged_in(backend: &dyn CliBackend) -> Result<bool, AppError> {
    let Some(auth) = read_json(backend, &[".codex", "auth.json"])? else {
        return Ok(false);
    };
    Ok(auth
        .get("auth_mode")
        .and_then(Value::as_str)
        .is_none_or(|mode| mode == "chatgpt")
        && nonempty(auth.pointer("/tokens/access_token"))
        && nonempty(auth.pointer("/tokens/refresh_token"))
        && !nonempty(auth.get("OPENAI_API_KEY")))
}

fn claude_login(backend: &dyn CliBackend) -> Result<ClaudeLogin, AppError> {
    let account = read_json(backend, &[".claude.json"])?;
    let credentials = read_json(backend, &[".claude", ".credentials.json"])?;
    let oauth = credentials.as_ref().and_then(|v| v.get("claudeAiOauth"));
    Ok(ClaudeLogin {
        account: account
            .as_ref()
            .and_then(|v| v.pointer("/oauthAccount/accountUuid"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        organization: account
            .as_ref()
            .and_then(|v| v.pointer("/oauthAccount/organizationUuid"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        credentials_present: oauth
            .is_some_and(|v| nonempty(v.get("accessToken")) && nonempty(v.get("refreshToken"))),
        refresh_token_expires_at: oauth
            .and_then(|v| v.get("refreshTokenExpiresAt"))
            .and_then(Value::as_u64),
    })
}

fn new_claude_login(before: &ClaudeLogin, now: &ClaudeLogin) -> bool {
    // Do not compare access/refresh token strings: routine renewal rotates them.
    now.account.is_some()
        && (before.account != now.account
            || before.organization != now.organization
            || (now.credentials_present && !before.credentials_present)
            || (now.credentials_present
                && now.refresh_token_expires_at.is_some()
                && before.refresh_token_expires_at != now.refresh_token_expires_at))
}

/// Called just before writing a client, including first Use. Existing official
/// credentials are the baseline, not evidence of a new login.
pub fn before_apply(target: &CliTarget) -> Result<(), AppError> {
    if target.snapshot_meta.target_type != TargetType::Wsl && !cfg!(target_os = "windows") {
        return Ok(());
    }
    let Some(mut manifest) = load()? else {
        return Ok(());
    };
    let Some(stored) = manifest
        .targets
        .iter_mut()
        .find(|s| stored_key(s) == target_key(&target.snapshot_meta))
    else {
        return Ok(());
    };
    if target.installed.codex && stored.login.codex_armed && codex_logged_in(&*target.backend)? {
        return Err(AppError::Config(
            "Official Codex login detected; waiting for client cleanup".into(),
        ));
    }
    if target.installed.claude
        && stored.login.claude.as_ref().is_some_and(|old| {
            claude_login(&*target.backend).is_ok_and(|now| new_claude_login(old, &now))
        })
    {
        return Err(AppError::Config(
            "Official Claude login detected; waiting for client cleanup".into(),
        ));
    }
    if target.installed.claude && stored.login.claude.is_none() {
        stored.login.claude = Some(claude_login(&*target.backend)?);
        save(&mut manifest)?;
    }
    Ok(())
}

/// Keep the exact writer output used for field-level cleanup after login.
pub fn after_apply(target: &CliTarget) -> Result<(), AppError> {
    if target.snapshot_meta.target_type != TargetType::Wsl && !cfg!(target_os = "windows") {
        return Ok(());
    }
    for (enabled, path) in [
        (target.installed.claude, vec![".claude", "settings.json"]),
        (target.installed.codex, vec![".codex", "config.toml"]),
    ] {
        if enabled {
            let sidecar = sidecar_path(
                &path.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                APPLIED_SUFFIX,
            )?;
            if let Some(bytes) = target.backend.read_bytes(&path)? {
                target.backend.write_atomic(&refs(&sidecar), &bytes)?;
            }
        }
    }
    if target.installed.codex {
        if let Some(mut manifest) = load()? {
            if let Some(stored) = manifest
                .targets
                .iter_mut()
                .find(|s| stored_key(s) == target_key(&target.snapshot_meta))
            {
                stored.login.codex_armed = true;
                save(&mut manifest)?;
            }
        }
    }
    Ok(())
}

/// Scan saved targets, not just currently reachable proxy URLs. Background
/// scans skip stopped WSL distros rather than starting them for credential reads.
pub fn scan(background: bool) -> Result<bool, AppError> {
    scan_with(background, true, backend_for)
}

pub fn detect(background: bool) -> Result<bool, AppError> {
    scan_with(background, false, backend_for)
}

fn scan_with(
    background: bool,
    clean: bool,
    make_backend: impl Fn(&ManagedTarget) -> Result<Box<dyn CliBackend>, AppError>,
) -> Result<bool, AppError> {
    let Some(mut manifest) = load()? else {
        return Ok(false);
    };
    if !manifest.targets.iter().any(|t| {
        (t.target_type == "wsl" || cfg!(target_os = "windows"))
            && (manifest.phase == LifecyclePhase::Active
                || t.login.released.iter().any(|entry| entry.cleanup_pending))
    }) {
        return Ok(false);
    }
    let running = if background && manifest.targets.iter().any(|t| t.target_type == "wsl") {
        Some(
            crate::wsl::distro::discover_distros()?
                .into_iter()
                .filter(|d| d.running)
                .map(|d| d.name)
                .collect::<HashSet<_>>(),
        )
    } else {
        None
    };
    let mut changed = false;
    let mut observations_changed = false;
    for index in 0..manifest.targets.len() {
        // Each native host or WSL distro has its own persisted ownership.
        if manifest.targets[index].target_type != "wsl" && !cfg!(target_os = "windows") {
            continue;
        }
        if running.as_ref().is_some_and(|names| {
            manifest.targets[index]
                .distro_name
                .as_ref()
                .is_some_and(|name| !names.contains(name))
        }) {
            continue;
        }
        if manifest.targets[index].home.is_none() && manifest.targets[index].target_type == "wsl" {
            continue;
        }
        let backend = make_backend(&manifest.targets[index])?;
        if manifest.phase == LifecyclePhase::Active
            && !manifest.targets[index].login.environment_disabled
        {
            let mut detected_providers = Vec::new();
            for provider in [Provider::Codex, Provider::Claude] {
                let stored = &manifest.targets[index];
                if stored.login.released(provider)
                    || !stored
                        .files
                        .iter()
                        .any(|f| f.provider == provider && f.managed && f.touched)
                {
                    continue;
                }
                let detected = match provider {
                    Provider::Codex => codex_logged_in(&*backend),
                    Provider::Claude => claude_login(&*backend).map(|now| {
                        let old = manifest.targets[index].login.claude.as_ref();
                        let detected = old.is_some_and(|old| new_claude_login(old, &now));
                        observations_changed |= old != Some(&now);
                        // Keep observing logouts, but never token contents.
                        manifest.targets[index].login.claude = Some(now);
                        detected
                    }),
                    Provider::Gemini => unreachable!(),
                };
                match detected {
                    Ok(true) => detected_providers.push(provider),
                    Ok(false) => {}
                    // Half-written login files are not proof of login/logout.
                    Err(_) => continue,
                }
            }
            if !detected_providers.is_empty() {
                journal_environment(&mut manifest.targets[index], &detected_providers);
                save(&mut manifest)?;
                changed = true;
            }
        }
        if !clean {
            continue;
        }
        for entry_index in 0..manifest.targets[index].login.released.len() {
            if !manifest.targets[index].login.released[entry_index].cleanup_pending {
                continue;
            }
            let result = cleanup(
                &*backend,
                &manifest.targets[index],
                &manifest.targets[index].login.released[entry_index],
            );
            let entry = &mut manifest.targets[index].login.released[entry_index];
            match result {
                Ok(()) => {
                    entry.cleanup_pending = false;
                    entry.error = None;
                    entry.files.clear();
                    changed = true;
                }
                Err(error) => {
                    entry.error = Some(error.to_string());
                }
            }
            save(&mut manifest)?;
        }
        if manifest.targets[index].target_type == "native"
            && manifest.targets[index].login.environment_disabled
            && manifest.targets[index]
                .login
                .released
                .iter()
                .all(|entry| !entry.cleanup_pending)
        {
            manifest.host_openai_api_key = None;
            save(&mut manifest)?;
        }
    }
    if observations_changed {
        save(&mut manifest)?;
    }
    Ok(changed)
}

fn journal_environment(target: &mut ManagedTarget, official: &[Provider]) {
    target.login.environment_disabled = true;
    for provider in [Provider::Codex, Provider::Claude, Provider::Gemini] {
        let files: Vec<_> = target
            .files
            .iter()
            .filter(|file| file.provider == provider)
            .cloned()
            .collect();
        if !files.is_empty() {
            target.login.released.push(ReleasedClient {
                official_login: official.contains(&provider),
                provider,
                cleanup_pending: true,
                error: None,
                files,
            });
        }
    }
    target.files.clear();
    target.pending = false;
    target.pending_reason = None;
}

pub fn disable_environment(distro: Option<&str>) -> Result<(), AppError> {
    let Some(mut manifest) = load()? else {
        return Ok(());
    };
    if let Some(target) = manifest
        .targets
        .iter_mut()
        .find(|target| target.distro_name.as_deref() == distro)
    {
        if !target.login.environment_disabled {
            journal_environment(target, &[]);
        }
    }
    save(&mut manifest)
}

/// All old file records have been retired. The next apply captures a fresh
/// origin for the entire environment from its current official configuration.
pub fn enable_environment(distro: Option<&str>) -> Result<(), AppError> {
    let Some(mut manifest) = load()? else {
        return Ok(());
    };
    let Some(target) = manifest
        .targets
        .iter_mut()
        .find(|target| target.distro_name.as_deref() == distro)
    else {
        return Ok(());
    };
    if target
        .login
        .released
        .iter()
        .any(|entry| entry.cleanup_pending)
    {
        return Err(AppError::Config(
            "Environment cleanup has not finished; resolve the displayed error first".into(),
        ));
    }
    target.login = LoginState::default();
    target.pending = true;
    target.pending_reason = Some("Applying Relay configuration".into());
    save(&mut manifest)
}

fn write_if_unchanged(
    backend: &dyn CliBackend,
    path: &[&str],
    before: &str,
    after: &str,
) -> Result<(), AppError> {
    if before == after {
        return Ok(());
    }
    if backend.read(path)?.as_deref() != Some(before) {
        return Err(AppError::Config(
            "CLI changed configuration during login cleanup; retrying".into(),
        ));
    }
    backend.write_atomic(path, after.as_bytes())
}

fn cleanup(
    backend: &dyn CliBackend,
    target: &ManagedTarget,
    entry: &ReleasedClient,
) -> Result<(), AppError> {
    if !entry.official_login {
        let mut missing_origin = false;
        for file in &entry.files {
            if !file.managed || !file.touched || file.restored {
                continue;
            }
            let origin = sidecar_path(&file.path, ORIGIN_SUFFIX)?;
            missing_origin |= file.origin.exists && !backend.exists(&refs(&origin))?;
            restore_state(backend, &refs(&file.path), &refs(&origin), &file.origin)?;
        }
        // Other environments still need the listener. A missing backup must
        // not leave this disabled environment pointing at that live listener.
        if missing_origin {
            match entry.provider {
                Provider::Codex => cleanup_codex(backend, &target.base_url)?,
                Provider::Claude => cleanup_claude(backend, &target.base_url)?,
                Provider::Gemini => super::super::clear_gemini_config_with(backend)?,
            }
        }
    } else {
        match entry.provider {
            Provider::Codex => cleanup_codex(backend, &target.base_url)?,
            Provider::Claude => cleanup_claude(backend, &target.base_url)?,
            Provider::Gemini => return Ok(()),
        }
    }
    for file in &entry.files {
        // Shell files can contain unrelated user edits. Remove only our key line.
        if entry.provider == Provider::Codex && file.path.first().is_some_and(|s| s != ".codex") {
            if let Some(before) = backend.read(&refs(&file.path))? {
                let after = before
                    .split_inclusive('\n')
                    .filter(|line| {
                        let trimmed = line.trim();
                        !(trimmed.contains("OPENAI_API_KEY")
                            && ["llm-relay-ignore", "dummy"].iter().any(|key| {
                                [
                                    format!("={key}"),
                                    format!("=\"{key}\""),
                                    format!("='{key}'"),
                                    format!(" {key}"),
                                ]
                                .iter()
                                .any(|ending| trimmed.ends_with(ending))
                            })
                            && !trimmed.starts_with('#'))
                    })
                    .collect::<String>();
                write_if_unchanged(backend, &refs(&file.path), &before, &after)?;
            }
        }
    }
    // Only after cleanup succeeds may obsolete backups be removed.
    if target.target_type == "native" && entry.provider == Provider::Codex && !cfg!(test) {
        if capture_host_openai_api_key().is_some_and(|state| state.relay_owned) {
            restore_host_openai_api_key(Some(&StoredHostValue::default()))?;
        }
    }
    for file in &entry.files {
        for suffix in [ORIGIN_SUFFIX, BACKUP_SUFFIX, APPLIED_SUFFIX] {
            backend.remove(&refs(&sidecar_path(&file.path, suffix)?))?;
        }
    }
    Ok(())
}

fn cleanup_claude(backend: &dyn CliBackend, base_url: &str) -> Result<(), AppError> {
    let path = [".claude", "settings.json"];
    let Some(before) = backend.read(&path)? else {
        return Ok(());
    };
    let mut current: Value = serde_json::from_str(&before)?;
    // An absent original is represented by an empty sidecar. Locally edited
    // backups that cannot be parsed provide no defaults, not a cleanup block.
    let applied = backend
        .read(&[".claude", "settings.json.llm-relay.applied"])?
        .and_then(|s| serde_json::from_str::<Value>(&s).ok());
    let original = backend
        .read(&[".claude", "settings.json.llm-relay.origin"])?
        .and_then(|s| serde_json::from_str::<Value>(&s).ok());
    if let Some(env) = current.get_mut("env").and_then(Value::as_object_mut) {
        if let Some(written) = applied
            .as_ref()
            .and_then(|v| v.get("env"))
            .and_then(Value::as_object)
        {
            let old = original
                .as_ref()
                .and_then(|v| v.get("env"))
                .and_then(Value::as_object);
            for (key, value) in written {
                if key == "ANTHROPIC_BASE_URL" || key == "ANTHROPIC_AUTH_TOKEN" {
                    continue;
                }
                if env.get(key) == Some(value) && old.and_then(|o| o.get(key)) != Some(value) {
                    match old.and_then(|o| o.get(key)) {
                        Some(v) => {
                            env.insert(key.clone(), v.clone());
                        }
                        None => {
                            env.remove(key);
                        }
                    }
                }
            }
        }
        if env.get("ANTHROPIC_BASE_URL").and_then(Value::as_str) == Some(base_url) {
            env.remove("ANTHROPIC_BASE_URL");
        }
        for key in ["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"] {
            if env.get(key).and_then(Value::as_str).is_some_and(|s| {
                s == crate::proxy_server::PLACEHOLDER_KEY || s == "llm-relay-ignore"
            }) {
                env.remove(key);
            }
        }
    }
    // ~/.claude.json and .credentials.json are deliberately never restored.
    write_if_unchanged(
        backend,
        &path,
        &before,
        &serde_json::to_string_pretty(&current)?,
    )
}

fn cleanup_codex(backend: &dyn CliBackend, base_url: &str) -> Result<(), AppError> {
    let auth_path = [".codex", "auth.json"];
    if let Some(before) = backend.read(&auth_path)? {
        let mut auth: Value = serde_json::from_str(&before)?;
        if auth.get("OPENAI_API_KEY").and_then(Value::as_str)
            == Some(crate::proxy_server::PLACEHOLDER_KEY)
        {
            if let Some(object) = auth.as_object_mut() {
                object.remove("OPENAI_API_KEY");
            }
            write_if_unchanged(
                backend,
                &auth_path,
                &before,
                &serde_json::to_string_pretty(&auth)?,
            )?;
        }
    }
    let path = [".codex", "config.toml"];
    if let Some(before) = backend.read(&path)? {
        let mut doc = before
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| AppError::Config(e.to_string()))?;
        let written = backend
            .read(&[".codex", "config.toml.llm-relay.applied"])?
            .and_then(|s| s.parse::<toml_edit::DocumentMut>().ok());
        let original = backend
            .read(&[".codex", "config.toml.llm-relay.origin"])?
            .and_then(|s| s.parse::<toml_edit::DocumentMut>().ok());
        for key in [
            "model",
            "model_reasoning_effort",
            "default_subagent_model",
            "default_subagent_reasoning_effort",
        ] {
            if let Some(written) = &written {
                if doc.get(key).map(ToString::to_string)
                    == written.get(key).map(ToString::to_string)
                {
                    match original.as_ref().and_then(|o| o.get(key)) {
                        Some(v) => {
                            doc[key] = v.clone();
                        }
                        None => {
                            doc.as_table_mut().remove(key);
                        }
                    }
                }
            }
        }
        if doc.get("model_provider").and_then(toml_edit::Item::as_str) == Some("copilot_gateway") {
            doc.as_table_mut().remove("model_provider");
        }
        if let Some(providers) = doc
            .get_mut("model_providers")
            .and_then(toml_edit::Item::as_table_like_mut)
        {
            if let Some(gateway) = providers
                .get_mut("copilot_gateway")
                .and_then(toml_edit::Item::as_table_like_mut)
            {
                for (key, expected) in [
                    ("name", "Copilot Gateway"),
                    ("env_key", "OPENAI_API_KEY"),
                    ("wire_api", "responses"),
                ] {
                    if gateway.get(key).and_then(toml_edit::Item::as_str) == Some(expected) {
                        gateway.remove(key);
                    }
                }
                if gateway
                    .get("base_url")
                    .and_then(toml_edit::Item::as_str)
                    .is_some_and(|url| url.trim_end_matches('/') == base_url.trim_end_matches('/'))
                {
                    gateway.remove("base_url");
                }
                if gateway.is_empty() {
                    providers.remove("copilot_gateway");
                }
            }
        }
        write_if_unchanged(backend, &path, &before, &doc.to_string())?;
    }
    // Keep OAuth tokens exactly as written by login. No auth.json rewrite.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::tests::MigrationEnv;
    use super::*;

    fn target(home: &std::path::Path, name: &str) -> CliTarget {
        CliTarget {
            backend: Box::new(WindowsFsBackend { home: home.into() }),
            base_url: "http://relay:18080".into(),
            installed: InstalledTools {
                claude: true,
                codex: true,
                gemini: false,
            },
            label: format!("wsl:{name}"),
            snapshot_meta: SnapshotMeta {
                target_type: TargetType::Wsl,
                distro_name: Some(name.into()),
                home: Some(home.to_string_lossy().into()),
            },
        }
    }

    fn scan_test() -> bool {
        scan_with(false, true, |stored| {
            Ok(Box::new(WindowsFsBackend {
                home: stored.home.as_ref().unwrap().into(),
            }))
        })
        .unwrap()
    }

    fn apply(targets: &[CliTarget]) {
        prepare_use(targets, &[], &BTreeMap::new()).unwrap();
        for target in targets {
            before_apply(target).unwrap();
            super::super::super::write_codex_config_with(
                &*target.backend,
                &target.base_url,
                crate::proxy_server::PLACEHOLDER_KEY,
                Some("relay-model"),
                None,
            )
            .unwrap();
            super::super::super::write_claude_config_with(
                &*target.backend,
                &target.base_url,
                crate::proxy_server::PLACEHOLDER_KEY,
                Some("claude-relay"),
                None,
                None,
            )
            .unwrap();
            after_apply(target).unwrap();
        }
        let mut manifest = load().unwrap().unwrap();
        manifest.phase = LifecyclePhase::Active;
        for stored in &mut manifest.targets {
            for file in &mut stored.files {
                file.touched = true;
            }
        }
        save(&mut manifest).unwrap();
    }

    const AUTH: &[u8] = br#"{"auth_mode":"chatgpt","tokens":{"access_token":"new-test-access","refresh_token":"new-test-refresh","id_token":"new-test-id"},"OPENAI_API_KEY":null}"#;

    #[test]
    fn codex_login_releases_only_one_distro_and_preserves_new_ground_truth() {
        let env = MigrationEnv::new();
        let other = tempfile::TempDir::new().unwrap();
        let targets = [
            target(env.home.path(), "Ubuntu"),
            target(other.path(), "Debian"),
        ];
        targets[0]
            .backend
            .write_atomic(
                &[".codex", "config.toml"],
                b"model = \"official-old\"\n[projects.keep]\ntrust_level = \"trusted\"\n",
            )
            .unwrap();
        apply(&targets);
        let b = &targets[0].backend;
        b.write_atomic(&[".codex", "auth.json"], AUTH).unwrap();
        // A concurrent user edit must survive cleanup of the Relay fields.
        let cfg = b
            .read(&[".codex", "config.toml"])
            .unwrap()
            .unwrap()
            .replace("relay-model", "official-new");
        b.write_atomic(&[".codex", "config.toml"], cfg.as_bytes())
            .unwrap();
        assert!(scan_test());
        let manifest = load().unwrap().unwrap();
        assert!(manifest.targets[0].login.released(Provider::Codex));
        assert!(manifest.targets[0].login.environment_disabled);
        assert!(
            !manifest.targets[0]
                .login
                .released
                .iter()
                .find(|e| e.provider == Provider::Claude)
                .unwrap()
                .official_login
        );
        assert!(manifest.targets[1].login.released.is_empty());
        assert_eq!(manifest.phase, LifecyclePhase::Active);
        assert!(manifest.targets[0]
            .files
            .iter()
            .all(|f| f.provider != Provider::Codex));
        assert_eq!(
            b.read_bytes(&[".codex", "auth.json"]).unwrap().unwrap(),
            AUTH
        );
        let cfg = b.read(&[".codex", "config.toml"]).unwrap().unwrap();
        assert!(cfg.contains("official-new") && cfg.contains("projects.keep"));
        assert!(!cfg.contains("copilot_gateway"));
        for file in ["auth.json", "config.toml"] {
            assert!(!b
                .exists(&[".codex", &format!("{file}{ORIGIN_SUFFIX}")])
                .unwrap());
            assert!(!b
                .exists(&[".codex", &format!("{file}{BACKUP_SUFFIX}")])
                .unwrap());
        }
        assert!(!scan_test());
        // Global Use still respects the scoped disable.
        let disabled = manifest.targets[0].login.filter(InstalledTools::ALL);
        assert!(!disabled.codex && !disabled.claude && !disabled.gemini);
        enable_environment(Some("Ubuntu")).unwrap();
        let targets = [target(env.home.path(), "Ubuntu")];
        prepare_active_apply(&targets, &BTreeMap::new()).unwrap();
        assert_eq!(
            targets[0]
                .backend
                .read_bytes(&[".codex", "auth.json.llm-relay.origin"])
                .unwrap()
                .unwrap(),
            AUTH
        );
    }

    #[test]
    fn failed_cleanup_is_journaled_before_old_origins_can_be_restored() {
        let env = MigrationEnv::new();
        let targets = [target(env.home.path(), "Ubuntu")];
        apply(&targets);
        let b = &targets[0].backend;
        b.write_atomic(&[".codex", "auth.json"], AUTH).unwrap();
        b.write_atomic(&[".codex", "config.toml"], b"broken = [")
            .unwrap();
        assert!(scan_test());
        let manifest = load().unwrap().unwrap();
        let entry = &manifest.targets[0].login.released[0];
        assert!(entry.cleanup_pending && entry.error.is_some());
        assert!(manifest.targets[0]
            .files
            .iter()
            .all(|f| f.provider != Provider::Codex));
        assert!(b.exists(&[".codex", "auth.json.llm-relay.origin"]).unwrap());
        assert!(enable_environment(Some("Ubuntu")).is_err());
        b.write_atomic(
            &[".codex", "config.toml"],
            b"model_provider = \"copilot_gateway\"\n",
        )
        .unwrap();
        assert!(scan_test());
        assert!(!load().unwrap().unwrap().targets[0].login.released[0].cleanup_pending);
        assert_eq!(
            b.read_bytes(&[".codex", "auth.json"]).unwrap().unwrap(),
            AUTH
        );
    }

    #[test]
    fn claude_login_preserves_account_credentials_and_unrelated_settings() {
        let env = MigrationEnv::new();
        let targets = [target(env.home.path(), "Ubuntu")];
        apply(&targets);
        let b = &targets[0].backend;
        let account = br#"{"oauthAccount":{"accountUuid":"official-account","organizationUuid":"org"},"projects":{"keep":true}}"#;
        let credentials = br#"{"claudeAiOauth":{"accessToken":"new","refreshToken":"new","refreshTokenExpiresAt":1000}}"#;
        b.write_atomic(&[".claude.json"], account).unwrap();
        b.write_atomic(&[".claude", ".credentials.json"], credentials)
            .unwrap();
        let mut settings = read_json(&**b, &[".claude", "settings.json"])
            .unwrap()
            .unwrap();
        settings["permissions"] = serde_json::json!({"allow":["Read"]});
        settings["env"]["ANTHROPIC_MODEL"] = Value::String("user-model".into());
        b.write_atomic(
            &[".claude", "settings.json"],
            &serde_json::to_vec(&settings).unwrap(),
        )
        .unwrap();
        assert!(scan_test());
        let settings = read_json(&**b, &[".claude", "settings.json"])
            .unwrap()
            .unwrap();
        assert!(
            settings["env"].get("ANTHROPIC_BASE_URL").is_none(),
            "{:?}",
            load().unwrap().unwrap().targets[0].login.released
        );
        assert!(settings["env"].get("ANTHROPIC_AUTH_TOKEN").is_none());
        assert_eq!(settings["env"]["ANTHROPIC_MODEL"], "user-model");
        assert_eq!(settings["permissions"]["allow"][0], "Read");
        assert_eq!(b.read_bytes(&[".claude.json"]).unwrap().unwrap(), account);
        assert_eq!(
            b.read_bytes(&[".claude", ".credentials.json"])
                .unwrap()
                .unwrap(),
            credentials
        );
        assert!(!b.exists(&[".claude.json.llm-relay.origin"]).unwrap());
        assert!(
            !load().unwrap().unwrap().targets[0]
                .login
                .released
                .iter()
                .find(|e| e.provider == Provider::Codex)
                .unwrap()
                .official_login
        );
    }

    #[test]
    fn token_refresh_is_not_a_new_claude_login() {
        let env = MigrationEnv::new();
        let targets = [target(env.home.path(), "Ubuntu")];
        let b = &targets[0].backend;
        b.write_atomic(
            &[".claude.json"],
            br#"{"oauthAccount":{"accountUuid":"same","organizationUuid":"org"}}"#,
        )
        .unwrap();
        b.write_atomic(&[".claude", ".credentials.json"], br#"{"claudeAiOauth":{"accessToken":"old","refreshToken":"old","expiresAt":100,"refreshTokenExpiresAt":1000}}"#).unwrap();
        apply(&targets);
        b.write_atomic(&[".claude", ".credentials.json"], br#"{"claudeAiOauth":{"accessToken":"renewed","refreshToken":"rotated","expiresAt":200,"refreshTokenExpiresAt":1000}}"#).unwrap();
        assert!(!scan_test());
        assert!(load().unwrap().unwrap().targets[0]
            .login
            .released
            .is_empty());
        // A new authorization's refresh-token lifetime is distinguishable.
        b.write_atomic(&[".claude", ".credentials.json"], br#"{"claudeAiOauth":{"accessToken":"login","refreshToken":"login","refreshTokenExpiresAt":2000}}"#).unwrap();
        assert!(scan_test());
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn native_login_preserves_credentials_and_releases_only_native_environment() {
        let env = MigrationEnv::new();
        let mut native = target(env.home.path(), "unused");
        native.snapshot_meta = SnapshotMeta {
            target_type: TargetType::Windows,
            distro_name: None,
            home: None,
        };
        apply(&[native]);
        let b = WindowsFsBackend {
            home: env.home.path().into(),
        };
        b.write_atomic(&[".codex", "auth.json"], AUTH).unwrap();
        assert!(scan(false).unwrap());
        let manifest = load().unwrap().unwrap();
        assert!(manifest.targets[0].login.released(Provider::Codex));
        assert!(manifest.targets[0].login.environment_disabled);
        assert!(
            !manifest.targets[0]
                .login
                .released
                .iter()
                .find(|e| e.provider == Provider::Claude)
                .unwrap()
                .official_login
        );
        assert_eq!(manifest.phase, LifecyclePhase::Active);
        assert_eq!(
            b.read_bytes(&[".codex", "auth.json"]).unwrap().unwrap(),
            AUTH
        );
        assert!(!b
            .exists(&[".codex", "config.toml.llm-relay.origin"])
            .unwrap());
        assert!(!b
            .read(&[".codex", "config.toml"])
            .unwrap()
            .unwrap()
            .contains("copilot_gateway"));
    }

    #[tokio::test]
    async fn last_environment_off_stops_all_listeners_but_one_off_keeps_other_environment() {
        crate::keystore::init_test();
        let env = MigrationEnv::new();
        let mut native = target(env.home.path(), "unused");
        native.snapshot_meta = SnapshotMeta {
            target_type: TargetType::Windows,
            distro_name: None,
            home: None,
        };
        apply(&[native]);
        let db = std::sync::Arc::new(crate::Database::open_in_memory().unwrap());
        let managed = db.get_managed_clients().unwrap();
        let mut active = db.get_active_config().unwrap();
        active.gateway_id = Some("selected-gateway".into());
        db.set_active_config(&active).unwrap();
        db.upsert_wsl_distro(&crate::wsl::distro::DistroRow {
            name: "Other environment".into(),
            is_default: false,
            selected: true,
            home: None,
            user: None,
            has_claude: true,
            has_codex: true,
            has_gemini: false,
            resolved_url: None,
            probed_at: None,
        })
        .unwrap();
        let service = crate::Service::new(db.clone(), std::sync::Arc::new(crate::events::NullSink));
        let primary = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let secondary = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let primary_addr = primary.local_addr().unwrap();
        let secondary_addr = secondary.local_addr().unwrap();
        let proxy = crate::proxy_server::start_with_listeners(
            crate::proxy_server::ProxyState::new(
                db.clone(),
                service.switch_lock.clone(),
                service.sink.clone(),
            ),
            primary,
            Some((secondary_addr.ip(), secondary)),
        )
        .await;
        let service = service.with_proxy(proxy.clone());
        service.set_environment_enabled(None, false).await.unwrap();
        assert!(proxy.is_running());
        assert_eq!(
            db.get_active_config().unwrap().gateway_id,
            active.gateway_id
        );
        assert_eq!(db.get_managed_clients().unwrap(), managed);
        assert!(!service.windows_host_enabled().unwrap());
        assert_eq!(service.build_apply_plan().ready.len(), 0);
        assert!(db.list_wsl_distros().unwrap()[0].selected);

        service
            .toggle_wsl_distro("Other environment".into(), false)
            .await
            .unwrap();
        assert!(!proxy.is_running());
        assert!(service.relay_disabled().unwrap());
        assert!(db.get_active_config().unwrap().gateway_id.is_none());
        assert_eq!(db.get_managed_clients().unwrap(), managed);
        let _primary = std::net::TcpListener::bind(primary_addr).unwrap();
        let _secondary = std::net::TcpListener::bind(secondary_addr).unwrap();
    }
    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn official_login_in_last_environment_stops_all_listeners() {
        crate::keystore::init_test();
        let env = MigrationEnv::new();
        let mut native = target(env.home.path(), "unused");
        native.snapshot_meta = SnapshotMeta {
            target_type: TargetType::Windows,
            distro_name: None,
            home: None,
        };
        apply(&[native]);
        let db = std::sync::Arc::new(crate::Database::open_in_memory().unwrap());
        let managed = db.get_managed_clients().unwrap();
        let mut active = db.get_active_config().unwrap();
        active.gateway_id = Some("selected-gateway".into());
        db.set_active_config(&active).unwrap();
        let service = crate::Service::new(db.clone(), std::sync::Arc::new(crate::events::NullSink));
        let primary = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let secondary = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let primary_addr = primary.local_addr().unwrap();
        let secondary_addr = secondary.local_addr().unwrap();
        let proxy = crate::proxy_server::start_with_listeners(
            crate::proxy_server::ProxyState::new(
                db.clone(),
                service.switch_lock.clone(),
                service.sink.clone(),
            ),
            primary,
            Some((secondary_addr.ip(), secondary)),
        )
        .await;
        let service = service.with_proxy(proxy.clone());
        let backend = WindowsFsBackend {
            home: env.home.path().into(),
        };
        backend
            .write_atomic(&[".codex", "auth.json"], AUTH)
            .unwrap();
        service.check_official_logins().await.unwrap();
        assert!(!service.windows_host_enabled().unwrap());
        assert_eq!(
            backend
                .read_bytes(&[".codex", "auth.json"])
                .unwrap()
                .unwrap(),
            AUTH
        );
        assert!(!backend
            .exists(&[".codex", "auth.json.llm-relay.origin"])
            .unwrap());
        assert!(!proxy.is_running());
        assert!(service.relay_disabled().unwrap());
        assert!(db.get_active_config().unwrap().gateway_id.is_none());
        assert_eq!(db.get_managed_clients().unwrap(), managed);
        let _primary = std::net::TcpListener::bind(primary_addr).unwrap();
        let _secondary = std::net::TcpListener::bind(secondary_addr).unwrap();
    }
}
