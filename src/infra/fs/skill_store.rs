use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use walkdir::WalkDir;

use crate::domain::skills::models::{
    DeleteSkillInput, InstallHubSkillInput, InstallHubSkillResult, SaveSkillInput, SkillDocument,
    SkillHubInstallRecord, SkillScope, UninstallHubSkillResult,
};
use crate::domain::skills::parser::{parse_frontmatter, render_frontmatter};
use crate::infra::fs::user_memory_store::FileMemoryStore;
use crate::support::sanitize::sanitize_skill_folder;

#[derive(Clone)]
pub struct FileSkillStore {
    shared_root: PathBuf,
    memory_store: FileMemoryStore,
}

#[derive(Debug, Clone)]
struct HubSkillBundle {
    name: String,
    content: String,
    source: String,
    identifier: String,
    trust_level: String,
    metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
struct HubSkillScanResult {
    verdict: String,
    findings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SkillHubLockFile {
    version: u32,
    installed: BTreeMap<String, SkillHubInstallRecord>,
}

impl Default for SkillHubLockFile {
    fn default() -> Self {
        Self {
            version: 1,
            installed: BTreeMap::new(),
        }
    }
}

impl FileSkillStore {
    pub fn new(shared_root: PathBuf, memory_store: FileMemoryStore) -> Self {
        Self {
            shared_root,
            memory_store,
        }
    }

    pub fn list_items(
        &self,
        scope: SkillScope,
        user_id: Option<&str>,
    ) -> Result<Vec<SkillDocument>> {
        let mut items = match scope {
            SkillScope::Shared => self.load_from_dir(&self.shared_root, SkillScope::Shared)?,
            SkillScope::Private => {
                let user_id = require_user_id(user_id)?;
                self.load_from_dir(&self.private_root(user_id)?, SkillScope::Private)?
            }
            SkillScope::Effective => {
                let mut merged = self.load_from_dir(&self.shared_root, SkillScope::Shared)?;
                if let Some(user_id) = user_id {
                    for (name, skill) in
                        self.load_from_dir(&self.private_root(user_id)?, SkillScope::Private)?
                    {
                        merged.insert(name, skill);
                    }
                }
                merged
            }
        }
        .into_values()
        .collect::<Vec<_>>();

        items.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(items)
    }

    pub fn save_skill(&self, input: SaveSkillInput) -> Result<SkillDocument> {
        let scope_items = self.load_scope_map(input.scope, input.user_id.as_deref())?;
        let clean_name = input.name.trim();
        if clean_name.is_empty() {
            bail!("Skill 名称不能为空");
        }

        let normalized_body = if input.body.trim().is_empty() {
            format!("# {clean_name}\n")
        } else {
            input.body.trim().to_string()
        };

        let (target_dir, mut meta) = if let Some(current_name) = input.current_name.as_deref() {
            let current_skill = scope_items
                .get(current_name)
                .cloned()
                .with_context(|| format!("Skill 不存在: {current_name}"))?;
            if clean_name != current_name && scope_items.contains_key(clean_name) {
                bail!("Skill 名称已存在: {clean_name}");
            }
            let target_dir = PathBuf::from(&current_skill.path)
                .parent()
                .with_context(|| format!("invalid skill path: {}", current_skill.path))?
                .to_path_buf();
            (target_dir, current_skill.meta)
        } else {
            if scope_items.contains_key(clean_name) {
                bail!("Skill 名称已存在: {clean_name}");
            }
            let root_dir = self.scope_root(input.scope, input.user_id.as_deref())?;
            std::fs::create_dir_all(&root_dir)
                .with_context(|| format!("failed to create {}", root_dir.display()))?;
            let folder = sanitize_skill_folder(input.folder.as_deref().unwrap_or(clean_name))?;
            let target_dir = root_dir.join(folder);
            if target_dir.exists() && target_dir.read_dir()?.next().transpose()?.is_some() {
                bail!("Skill 目录已存在且非空: {}", target_dir.display());
            }
            (target_dir, BTreeMap::new())
        };

        upsert_meta(&mut meta, "name", clean_name);
        upsert_meta(&mut meta, "description", input.description.trim());
        upsert_meta(&mut meta, "tags", input.tags.trim());
        upsert_meta(&mut meta, "trigger", input.trigger.trim());

        std::fs::create_dir_all(&target_dir)
            .with_context(|| format!("failed to create {}", target_dir.display()))?;
        let skill_path = target_dir.join("SKILL.md");
        std::fs::write(&skill_path, render_frontmatter(&meta, &normalized_body))
            .with_context(|| format!("failed to write {}", skill_path.display()))?;

        self.get_item(clean_name, input.scope, input.user_id.as_deref())
    }

    pub fn resolve_effective(&self, name: &str, user_id: Option<&str>) -> Result<SkillDocument> {
        let scope = if user_id.is_some() {
            SkillScope::Effective
        } else {
            SkillScope::Shared
        };
        self.get_item(name, scope, user_id)
    }

    pub fn ensure_private_skill_copy(&self, user_id: &str, name: &str) -> Result<SkillDocument> {
        if let Ok(existing) = self.get_item(name, SkillScope::Private, Some(user_id)) {
            return Ok(existing);
        }

        let shared = self.get_item(name, SkillScope::Shared, None)?;
        let root_dir = self.private_root(user_id)?;
        std::fs::create_dir_all(&root_dir)
            .with_context(|| format!("failed to create {}", root_dir.display()))?;

        let preferred_folder = sanitize_skill_folder(&shared.folder)?;
        let target_dir = next_available_dir(&root_dir, &preferred_folder);
        let skill_path = target_dir.join("SKILL.md");
        std::fs::create_dir_all(&target_dir)
            .with_context(|| format!("failed to create {}", target_dir.display()))?;
        std::fs::write(&skill_path, render_frontmatter(&shared.meta, &shared.body))
            .with_context(|| format!("failed to write {}", skill_path.display()))?;
        tracing::info!(
            user_id,
            skill = shared.name.as_str(),
            source_scope = "shared",
            path = %skill_path.display(),
            "private skill created"
        );

        self.get_item(name, SkillScope::Private, Some(user_id))
    }

    pub fn get_private_skill(&self, user_id: &str, name: &str) -> Result<SkillDocument> {
        self.get_item(name, SkillScope::Private, Some(user_id))
    }

    pub fn rewrite_private_skill_body(
        &self,
        user_id: &str,
        name: &str,
        body: &str,
    ) -> Result<SkillDocument> {
        let normalized = body.trim();
        if normalized.is_empty() {
            bail!("Skill body cannot be empty");
        }

        let skill = self.get_item(name, SkillScope::Private, Some(user_id))?;
        let skill_path = PathBuf::from(&skill.path);
        std::fs::write(&skill_path, render_frontmatter(&skill.meta, normalized))
            .with_context(|| format!("failed to write {}", skill_path.display()))?;
        tracing::info!(
            user_id,
            skill = skill.name.as_str(),
            chars = normalized.len(),
            path = %skill_path.display(),
            "private skill updated"
        );

        self.get_item(name, SkillScope::Private, Some(user_id))
    }

    pub async fn install_hub_skill(
        &self,
        input: InstallHubSkillInput,
    ) -> Result<InstallHubSkillResult> {
        let source = input.source.as_deref().unwrap_or("url").trim();
        if !source.is_empty() && !source.eq_ignore_ascii_case("url") {
            bail!("当前版本仅支持从 URL 安装 skill");
        }

        self.ensure_hub_dirs()?;
        let bundle = fetch_url_skill_bundle(
            input.identifier.trim(),
            input.name_override.as_deref().map(str::trim),
        )
        .await?;

        let existing_installation = self.get_hub_install_record(&bundle.name)?;
        if existing_installation.is_some() && !input.force {
            bail!("Skill 已通过 hub 安装: {}", bundle.name);
        }
        if existing_installation.is_none()
            && self
                .load_scope_map(SkillScope::Shared, None)?
                .contains_key(&bundle.name)
        {
            bail!("Skill 名称已存在且不是 hub 安装: {}", bundle.name);
        }

        let category_parts = normalize_install_category(input.category.as_deref())?;
        let install_path = build_install_path(&category_parts, &bundle.name);
        let target_dir = self.resolve_hub_install_dir(&install_path, &bundle.name)?;
        let existing_dir = existing_installation
            .as_ref()
            .map(|record| self.resolve_hub_install_dir(&record.install_path, &record.name))
            .transpose()?;
        let target_matches_existing = existing_dir
            .as_ref()
            .is_some_and(|path| path == &target_dir);
        if target_dir.exists() && !target_matches_existing {
            if target_dir.read_dir()?.next().transpose()?.is_some() {
                bail!("Skill 目录已存在且非空: {}", target_dir.display());
            }
            std::fs::remove_dir_all(&target_dir)
                .with_context(|| format!("failed to clean {}", target_dir.display()))?;
        }

        let quarantine_dir = self.quarantine_bundle(&bundle)?;
        let scan = scan_skill_dir(&quarantine_dir)?;
        if scan.verdict == "dangerous" || (scan.verdict == "caution" && !input.force) {
            let _ = std::fs::remove_dir_all(&quarantine_dir);
            bail!(
                "安装被 Skills Guard 阻止: verdict={} findings={}",
                scan.verdict,
                scan.findings.join("; ")
            );
        }

        if let Some(record) = existing_installation.as_ref() {
            self.remove_recorded_hub_skill(record)?;
            self.record_hub_uninstall(&bundle.name)?;
        }
        if target_dir.exists() {
            if target_dir.read_dir()?.next().transpose()?.is_some() {
                bail!("Skill 目录已存在且非空: {}", target_dir.display());
            }
            std::fs::remove_dir_all(&target_dir)
                .with_context(|| format!("failed to clean {}", target_dir.display()))?;
        }

        target_dir
            .parent()
            .map(std::fs::create_dir_all)
            .transpose()
            .with_context(|| format!("failed to create parent for {}", target_dir.display()))?;
        std::fs::rename(&quarantine_dir, &target_dir).with_context(|| {
            format!(
                "failed to install {} from {} to {}",
                bundle.name,
                quarantine_dir.display(),
                target_dir.display()
            )
        })?;

        let skill_path = target_dir.join("SKILL.md");
        let skill = load_skill_document_from_file(&skill_path, SkillScope::Shared)?;
        let now = timestamp_string();
        let installed_at = existing_installation
            .as_ref()
            .map(|record| record.installed_at.clone())
            .unwrap_or_else(|| now.clone());
        let record = SkillHubInstallRecord {
            name: bundle.name.clone(),
            source: bundle.source,
            identifier: bundle.identifier,
            trust_level: bundle.trust_level,
            scan_verdict: scan.verdict,
            content_hash: content_hash_dir(&target_dir)?,
            install_path,
            files: vec!["SKILL.md".to_string()],
            metadata: bundle.metadata,
            installed_at,
            updated_at: now,
        };
        self.record_hub_install(record.clone())?;
        self.append_hub_audit_log(
            "INSTALL",
            &record.name,
            &record.source,
            &record.trust_level,
            &record.scan_verdict,
            &record.content_hash,
        )?;

        Ok(InstallHubSkillResult {
            skill,
            installation: record,
        })
    }

    pub fn list_hub_installations(&self) -> Result<Vec<SkillHubInstallRecord>> {
        let mut installations = self
            .read_hub_lock()?
            .installed
            .into_values()
            .collect::<Vec<_>>();
        installations.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(installations)
    }

    pub fn uninstall_hub_skill(&self, name: &str) -> Result<UninstallHubSkillResult> {
        self.ensure_hub_dirs()?;
        let clean_name = sanitize_skill_folder(name)?;
        let record = self
            .get_hub_install_record(&clean_name)?
            .with_context(|| format!("Skill 不是 hub 安装或已卸载: {clean_name}"))?;
        let install_dir = self.resolve_hub_install_dir(&record.install_path, &clean_name)?;
        let skill_path = install_dir.join("SKILL.md");
        let deleted = if skill_path.exists() {
            load_skill_document_from_file(&skill_path, SkillScope::Shared)?
        } else {
            fallback_deleted_skill(&record, &skill_path)
        };

        if install_dir.exists() {
            std::fs::remove_dir_all(&install_dir)
                .with_context(|| format!("failed to delete {}", install_dir.display()))?;
        }
        self.record_hub_uninstall(&clean_name)?;
        self.append_hub_audit_log(
            "UNINSTALL",
            &record.name,
            &record.source,
            &record.trust_level,
            "n/a",
            "user_request",
        )?;

        Ok(UninstallHubSkillResult {
            deleted,
            installation: record,
        })
    }

    pub fn delete_skill(&self, input: DeleteSkillInput) -> Result<SkillDocument> {
        let scope_items = self.load_scope_map(input.scope, input.user_id.as_deref())?;
        let requested_name = input.name.trim();
        if input.scope == SkillScope::Shared
            && self.get_hub_install_record(requested_name)?.is_some()
        {
            bail!("Hub 安装的 skill 必须通过 hub 卸载: {requested_name}");
        }
        let skill = scope_items
            .get(requested_name)
            .cloned()
            .with_context(|| format!("Skill 不存在: {requested_name}"))?;

        let target_dir = PathBuf::from(&skill.path)
            .parent()
            .with_context(|| format!("invalid skill path: {}", skill.path))?
            .to_path_buf();
        let root_dir = self.scope_root(input.scope, input.user_id.as_deref())?;
        if !target_dir.starts_with(&root_dir) {
            bail!("Skill 目录越界");
        }

        std::fs::remove_dir_all(&target_dir)
            .with_context(|| format!("failed to delete {}", target_dir.display()))?;
        Ok(skill)
    }

    fn get_item(
        &self,
        name: &str,
        scope: SkillScope,
        user_id: Option<&str>,
    ) -> Result<SkillDocument> {
        let scope_items = self.load_scope_map(scope, user_id)?;
        scope_items
            .get(name)
            .cloned()
            .with_context(|| format!("Skill 不存在: {name}"))
    }

    fn load_scope_map(
        &self,
        scope: SkillScope,
        user_id: Option<&str>,
    ) -> Result<BTreeMap<String, SkillDocument>> {
        match scope {
            SkillScope::Shared => self.load_from_dir(&self.shared_root, SkillScope::Shared),
            SkillScope::Private => {
                let user_id = require_user_id(user_id)?;
                self.load_from_dir(&self.private_root(user_id)?, SkillScope::Private)
            }
            SkillScope::Effective => {
                let mut merged = self.load_from_dir(&self.shared_root, SkillScope::Shared)?;
                if let Some(user_id) = user_id {
                    for (name, skill) in
                        self.load_from_dir(&self.private_root(user_id)?, SkillScope::Private)?
                    {
                        merged.insert(name, skill);
                    }
                }
                Ok(merged)
            }
        }
    }

    fn scope_root(&self, scope: SkillScope, user_id: Option<&str>) -> Result<PathBuf> {
        match scope {
            SkillScope::Shared => Ok(self.shared_root.clone()),
            SkillScope::Private => {
                let user_id = require_user_id(user_id)?;
                self.private_root(user_id)
            }
            SkillScope::Effective => bail!("effective scope 不支持直接写入"),
        }
    }

    fn private_root(&self, user_id: &str) -> Result<PathBuf> {
        Ok(self.memory_store.ensure_workspace(user_id)?.skills_dir)
    }

    fn hub_root(&self) -> PathBuf {
        self.shared_root.join(".hub")
    }

    fn hub_quarantine_root(&self) -> PathBuf {
        self.hub_root().join("quarantine")
    }

    fn hub_lock_path(&self) -> PathBuf {
        self.hub_root().join("lock.json")
    }

    fn hub_audit_log_path(&self) -> PathBuf {
        self.hub_root().join("audit.log")
    }

    fn ensure_hub_dirs(&self) -> Result<()> {
        std::fs::create_dir_all(self.hub_quarantine_root()).with_context(|| {
            format!("failed to create {}", self.hub_quarantine_root().display())
        })?;
        std::fs::create_dir_all(self.hub_root().join("index-cache")).with_context(|| {
            format!(
                "failed to create {}",
                self.hub_root().join("index-cache").display()
            )
        })?;
        if !self.hub_lock_path().exists() {
            self.write_hub_lock(&SkillHubLockFile::default())?;
        }
        if !self.hub_audit_log_path().exists() {
            std::fs::File::create(self.hub_audit_log_path()).with_context(|| {
                format!("failed to create {}", self.hub_audit_log_path().display())
            })?;
        }
        Ok(())
    }

    fn read_hub_lock(&self) -> Result<SkillHubLockFile> {
        let path = self.hub_lock_path();
        if !path.exists() {
            return Ok(SkillHubLockFile::default());
        }
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        if raw.trim().is_empty() {
            return Ok(SkillHubLockFile::default());
        }
        serde_json::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
    }

    fn write_hub_lock(&self, lock: &SkillHubLockFile) -> Result<()> {
        std::fs::create_dir_all(self.hub_root())
            .with_context(|| format!("failed to create {}", self.hub_root().display()))?;
        let path = self.hub_lock_path();
        let tmp_path = path.with_extension("json.tmp");
        let content = serde_json::to_string_pretty(lock)? + "\n";
        std::fs::write(&tmp_path, content)
            .with_context(|| format!("failed to write {}", tmp_path.display()))?;
        std::fs::rename(&tmp_path, &path)
            .with_context(|| format!("failed to replace {}", path.display()))?;
        Ok(())
    }

    fn get_hub_install_record(&self, name: &str) -> Result<Option<SkillHubInstallRecord>> {
        Ok(self.read_hub_lock()?.installed.get(name).cloned())
    }

    fn record_hub_install(&self, record: SkillHubInstallRecord) -> Result<()> {
        let mut lock = self.read_hub_lock()?;
        lock.installed.insert(record.name.clone(), record);
        self.write_hub_lock(&lock)
    }

    fn record_hub_uninstall(&self, name: &str) -> Result<()> {
        let mut lock = self.read_hub_lock()?;
        lock.installed.remove(name);
        self.write_hub_lock(&lock)
    }

    fn quarantine_bundle(&self, bundle: &HubSkillBundle) -> Result<PathBuf> {
        self.ensure_hub_dirs()?;
        let target = self.hub_quarantine_root().join(&bundle.name);
        if target.exists() {
            std::fs::remove_dir_all(&target)
                .with_context(|| format!("failed to clean {}", target.display()))?;
        }
        std::fs::create_dir_all(&target)
            .with_context(|| format!("failed to create {}", target.display()))?;
        std::fs::write(target.join("SKILL.md"), &bundle.content)
            .with_context(|| format!("failed to write {}", target.join("SKILL.md").display()))?;
        Ok(target)
    }

    fn resolve_hub_install_dir(&self, install_path: &str, skill_name: &str) -> Result<PathBuf> {
        std::fs::create_dir_all(&self.shared_root)
            .with_context(|| format!("failed to create {}", self.shared_root.display()))?;
        let normalized = normalize_lock_install_path(install_path, skill_name)?;
        let shared_root = self.shared_root.canonicalize().with_context(|| {
            format!(
                "failed to resolve skills root {}",
                self.shared_root.display()
            )
        })?;
        let mut target = self.shared_root.clone();
        for part in normalized.split('/') {
            target.push(part);
            if is_path_redirect(&target) {
                bail!("Unsafe install path contains symlink: {install_path}");
            }
        }
        if target.exists() {
            let resolved = target
                .canonicalize()
                .with_context(|| format!("failed to resolve {}", target.display()))?;
            if resolved == shared_root || !resolved.starts_with(&shared_root) {
                bail!("Unsafe install path: {install_path}");
            }
        } else if let Some(parent) = target.parent() {
            let mut current = self.shared_root.clone();
            for component in parent
                .strip_prefix(&self.shared_root)
                .unwrap_or(parent)
                .components()
            {
                current.push(component);
                if is_path_redirect(&current) {
                    bail!("Unsafe install path contains symlink: {install_path}");
                }
            }
        }
        Ok(target)
    }

    fn remove_recorded_hub_skill(&self, record: &SkillHubInstallRecord) -> Result<()> {
        let install_dir = self.resolve_hub_install_dir(&record.install_path, &record.name)?;
        if install_dir.exists() {
            std::fs::remove_dir_all(&install_dir)
                .with_context(|| format!("failed to delete {}", install_dir.display()))?;
        }
        Ok(())
    }

    fn append_hub_audit_log(
        &self,
        action: &str,
        skill_name: &str,
        source: &str,
        trust_level: &str,
        verdict: &str,
        extra: &str,
    ) -> Result<()> {
        self.ensure_hub_dirs()?;
        let line = format!(
            "{} {} {} {}:{} {} {}\n",
            timestamp_string(),
            action,
            skill_name,
            source,
            trust_level,
            verdict,
            extra
        );
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.hub_audit_log_path())
            .with_context(|| format!("failed to open {}", self.hub_audit_log_path().display()))?;
        file.write_all(line.as_bytes())
            .with_context(|| format!("failed to write {}", self.hub_audit_log_path().display()))?;
        Ok(())
    }

    fn load_from_dir(
        &self,
        root_dir: &Path,
        scope: SkillScope,
    ) -> Result<BTreeMap<String, SkillDocument>> {
        let mut items = BTreeMap::new();
        if !root_dir.exists() {
            return Ok(items);
        }

        let mut files = WalkDir::new(root_dir)
            .into_iter()
            .filter_entry(|entry| !is_excluded_skill_path(entry.path()))
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_file() && entry.file_name() == "SKILL.md")
            .map(|entry| entry.into_path())
            .collect::<Vec<_>>();
        files.sort();

        for skill_file in files {
            let raw = std::fs::read_to_string(&skill_file)
                .with_context(|| format!("failed to read {}", skill_file.display()))?;
            let (meta, body) = parse_frontmatter(&raw);
            let folder = skill_file
                .parent()
                .and_then(|value| value.file_name())
                .and_then(|value| value.to_str())
                .unwrap_or("skill")
                .to_string();
            let name = meta.get("name").cloned().unwrap_or_else(|| folder.clone());
            let document = SkillDocument {
                description: meta.get("description").cloned().unwrap_or_default(),
                tags: meta.get("tags").cloned().unwrap_or_default(),
                trigger: meta.get("trigger").cloned().unwrap_or_default(),
                folder,
                path: skill_file.display().to_string(),
                body,
                meta,
                scope,
                name: name.clone(),
            };
            items.insert(name, document);
        }

        Ok(items)
    }
}

fn require_user_id(user_id: Option<&str>) -> Result<&str> {
    match user_id.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => Ok(value),
        None => bail!("私有 skill 需要提供 user_id"),
    }
}

fn upsert_meta(meta: &mut BTreeMap<String, String>, key: &str, value: &str) {
    if value.is_empty() {
        meta.remove(key);
    } else {
        meta.insert(key.to_string(), value.to_string());
    }
}

fn next_available_dir(root_dir: &Path, folder_name: &str) -> PathBuf {
    let initial = root_dir.join(folder_name);
    if !initial.exists()
        || initial
            .read_dir()
            .ok()
            .and_then(|mut items| items.next())
            .is_none()
    {
        return initial;
    }

    let mut index = 2usize;
    loop {
        let candidate = root_dir.join(format!("{folder_name}-{index}"));
        if !candidate.exists()
            || candidate
                .read_dir()
                .ok()
                .and_then(|mut items| items.next())
                .is_none()
        {
            return candidate;
        }
        index += 1;
    }
}

async fn fetch_url_skill_bundle(
    identifier: &str,
    name_override: Option<&str>,
) -> Result<HubSkillBundle> {
    let url = reqwest::Url::parse(identifier).context("Skill URL 无效")?;
    if !matches!(url.scheme(), "http" | "https") {
        bail!("Skill URL 仅支持 http/https");
    }
    if !url.username().is_empty() || url.password().is_some() {
        bail!("Skill URL 不允许包含用户名或密码");
    }

    let response = reqwest::Client::new()
        .get(url.clone())
        .send()
        .await
        .with_context(|| format!("failed to fetch skill URL: {identifier}"))?;
    if !response.status().is_success() {
        bail!("Skill URL 请求失败: HTTP {}", response.status());
    }
    let bytes = response
        .bytes()
        .await
        .context("failed to read skill URL response body")?;
    if bytes.len() > 1_048_576 {
        bail!("SKILL.md 超过 1 MiB，拒绝安装");
    }
    let raw = String::from_utf8(bytes.to_vec()).context("SKILL.md 必须是 UTF-8 文本")?;
    let (mut meta, body) = parse_frontmatter(&raw);
    if body.trim().is_empty() {
        bail!("SKILL.md 正文不能为空");
    }

    let raw_name = name_override
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| meta.get("name").map(|value| value.trim().to_string()))
        .filter(|value| !value.is_empty())
        .or_else(|| derive_name_from_url(&url))
        .context("无法从 URL 或 frontmatter 推断 skill 名称，请提供 name_override")?;
    let name = sanitize_skill_folder(&raw_name)?;
    upsert_meta(&mut meta, "name", &name);
    let content = render_frontmatter(&meta, &body);

    let mut metadata = BTreeMap::new();
    metadata.insert("url".to_string(), identifier.to_string());
    metadata.insert("fetched_at".to_string(), timestamp_string());

    Ok(HubSkillBundle {
        name,
        content,
        source: "url".to_string(),
        identifier: identifier.to_string(),
        trust_level: "community".to_string(),
        metadata,
    })
}

fn derive_name_from_url(url: &reqwest::Url) -> Option<String> {
    let segments = url
        .path_segments()
        .map(|segments| segments.collect::<Vec<_>>())
        .unwrap_or_default();
    for segment in segments.into_iter().rev() {
        let clean = segment.trim();
        if clean.is_empty() {
            continue;
        }
        if clean.eq_ignore_ascii_case("skill.md") {
            continue;
        }
        let stem = clean.strip_suffix(".md").unwrap_or(clean);
        if !stem.is_empty() {
            return Some(stem.to_string());
        }
    }
    None
}

fn normalize_install_category(category: Option<&str>) -> Result<Vec<String>> {
    let Some(raw) = category.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(Vec::new());
    };
    let mut parts = Vec::new();
    for part in raw.replace('\\', "/").split('/') {
        let trimmed = part.trim();
        if trimmed.is_empty() || matches!(trimmed, "." | "..") {
            bail!("Skill 分类路径无效: {raw}");
        }
        if is_excluded_skill_component(trimmed) || trimmed.starts_with('.') {
            bail!("Skill 分类路径包含保留目录: {trimmed}");
        }
        let clean = sanitize_skill_folder(trimmed)?;
        if is_excluded_skill_component(&clean) || clean.starts_with('.') {
            bail!("Skill 分类路径包含保留目录: {clean}");
        }
        parts.push(clean);
    }
    Ok(parts)
}

fn build_install_path(category_parts: &[String], skill_name: &str) -> String {
    let mut parts = category_parts.to_vec();
    parts.push(skill_name.to_string());
    parts.join("/")
}

fn normalize_lock_install_path(install_path: &str, skill_name: &str) -> Result<String> {
    let normalized = install_path.replace('\\', "/");
    let parts = normalized
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect::<Vec<_>>();
    if parts.is_empty() || parts.iter().any(|part| *part == "..") {
        bail!("Unsafe install path: {install_path}");
    }
    if parts.last().copied() != Some(skill_name) {
        bail!("Unsafe install path: {install_path}");
    }
    if parts
        .iter()
        .any(|part| part.starts_with('.') || is_excluded_skill_component(part))
    {
        bail!("Unsafe install path contains reserved component: {install_path}");
    }
    Ok(parts.join("/"))
}

fn scan_skill_dir(skill_dir: &Path) -> Result<HubSkillScanResult> {
    let skill_md = skill_dir.join("SKILL.md");
    let mut findings = Vec::new();
    let mut dangerous = false;
    let mut total_size = 0u64;

    if !skill_md.is_file() {
        findings.push("missing SKILL.md".to_string());
        dangerous = true;
    }

    for entry in WalkDir::new(skill_dir)
        .into_iter()
        .filter_entry(|entry| !is_excluded_skill_path(entry.path()))
    {
        let entry = entry?;
        let path = entry.path();
        if is_path_redirect(path) {
            findings.push(format!("symlink is not allowed: {}", path.display()));
            dangerous = true;
            continue;
        }
        if !entry.file_type().is_file() {
            continue;
        }

        let metadata = entry
            .metadata()
            .with_context(|| format!("failed to stat {}", path.display()))?;
        total_size = total_size.saturating_add(metadata.len());
        if metadata.len() > 1_048_576 {
            findings.push(format!("file too large: {}", path.display()));
            dangerous = true;
            continue;
        }

        if let Ok(text) = std::fs::read_to_string(path) {
            for finding in scan_skill_text(&text, path.strip_prefix(skill_dir).unwrap_or(path)) {
                if finding.starts_with("dangerous:") {
                    dangerous = true;
                }
                findings.push(finding);
            }
        }
    }

    if total_size > 2_000_000 {
        findings.push("dangerous: total skill size exceeds 2 MiB".to_string());
        dangerous = true;
    }

    let verdict = if dangerous {
        "dangerous"
    } else if findings.is_empty() {
        "safe"
    } else {
        "caution"
    };

    Ok(HubSkillScanResult {
        verdict: verdict.to_string(),
        findings,
    })
}

fn scan_skill_text(text: &str, rel_path: &Path) -> Vec<String> {
    let mut findings = Vec::new();
    let rel = rel_path.display();
    let lower = text.to_ascii_lowercase();

    for phrase in [
        "ignore previous instructions",
        "ignore all previous",
        "disregard your",
        "forget your instructions",
        "system prompt:",
        "<system>",
    ] {
        if lower.contains(phrase) {
            findings.push(format!(
                "caution: prompt-injection phrase `{phrase}` in {rel}"
            ));
        }
    }

    for phrase in ["rm -rf", "mkfs.", "dd if=", ":(){ :|:& };:", "shutdown -h"] {
        if lower.contains(phrase) {
            findings.push(format!(
                "dangerous: destructive command `{phrase}` in {rel}"
            ));
        }
    }

    if lower.contains("~/.ssh") || lower.contains("$home/.ssh") {
        findings.push(format!("dangerous: SSH directory access in {rel}"));
    }

    let mentions_network = lower.contains("curl ") || lower.contains("wget ");
    let mentions_secret = ["token", "secret", "password", "credential", "api_key"]
        .iter()
        .any(|needle| lower.contains(needle));
    if mentions_network && mentions_secret {
        findings.push(format!(
            "dangerous: network command combined with secret-like token in {rel}"
        ));
    }

    findings
}

fn content_hash_dir(directory: &Path) -> Result<String> {
    let mut files = WalkDir::new(directory)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .collect::<Vec<_>>();
    files.sort();

    let mut hasher = Sha1::new();
    for file in files {
        let rel = file
            .strip_prefix(directory)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        hasher.update(rel.as_bytes());
        hasher.update([0]);
        hasher.update(std::fs::read(&file)?);
    }
    Ok(format!("sha1:{:x}", hasher.finalize()))
}

fn load_skill_document_from_file(skill_file: &Path, scope: SkillScope) -> Result<SkillDocument> {
    let raw = std::fs::read_to_string(skill_file)
        .with_context(|| format!("failed to read {}", skill_file.display()))?;
    let (meta, body) = parse_frontmatter(&raw);
    let folder = skill_file
        .parent()
        .and_then(|value| value.file_name())
        .and_then(|value| value.to_str())
        .unwrap_or("skill")
        .to_string();
    let name = meta.get("name").cloned().unwrap_or_else(|| folder.clone());
    Ok(SkillDocument {
        description: meta.get("description").cloned().unwrap_or_default(),
        tags: meta.get("tags").cloned().unwrap_or_default(),
        trigger: meta.get("trigger").cloned().unwrap_or_default(),
        folder,
        path: skill_file.display().to_string(),
        body,
        meta,
        scope,
        name,
    })
}

fn fallback_deleted_skill(record: &SkillHubInstallRecord, skill_path: &Path) -> SkillDocument {
    SkillDocument {
        name: record.name.clone(),
        description: String::new(),
        tags: String::new(),
        trigger: String::new(),
        folder: Path::new(&record.install_path)
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or(record.name.as_str())
            .to_string(),
        path: skill_path.display().to_string(),
        body: String::new(),
        meta: BTreeMap::new(),
        scope: SkillScope::Shared,
    }
}

fn timestamp_string() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or_default();
    format!("unix:{seconds}")
}

fn is_path_redirect(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
}

fn is_excluded_skill_component(component: &str) -> bool {
    matches!(
        component,
        ".git"
            | ".github"
            | ".hub"
            | ".archive"
            | ".venv"
            | "venv"
            | "node_modules"
            | "site-packages"
            | "__pycache__"
            | ".tox"
            | ".nox"
            | ".pytest_cache"
            | ".mypy_cache"
            | ".ruff_cache"
    )
}

fn is_excluded_skill_path(path: &Path) -> bool {
    path.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .is_some_and(is_excluded_skill_component)
    })
}

#[cfg(test)]
mod tests {
    use super::FileSkillStore;
    use crate::config::model::FileMemoryConfig;
    use crate::domain::skills::parser::render_frontmatter;
    use crate::infra::fs::user_memory_store::FileMemoryStore;
    use std::collections::BTreeMap;
    use std::fs;
    use std::io::{self, Write};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestRepo {
        root: PathBuf,
    }

    impl TestRepo {
        fn new() -> Self {
            let unique = format!(
                "auto-claude-skill-store-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("system time before unix epoch")
                    .as_nanos()
            );
            let root = std::env::temp_dir().join(unique);
            fs::create_dir_all(&root).expect("create temp repo");
            Self { root }
        }
    }

    impl Drop for TestRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[derive(Clone, Default)]
    struct SharedWriter {
        buffer: Arc<Mutex<Vec<u8>>>,
    }

    impl SharedWriter {
        fn contents(&self) -> String {
            String::from_utf8(self.buffer.lock().expect("writer lock poisoned").clone())
                .expect("writer output is valid utf-8")
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for SharedWriter {
        type Writer = SharedWriterGuard;

        fn make_writer(&'a self) -> Self::Writer {
            SharedWriterGuard {
                buffer: self.buffer.clone(),
            }
        }
    }

    struct SharedWriterGuard {
        buffer: Arc<Mutex<Vec<u8>>>,
    }

    impl Write for SharedWriterGuard {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.buffer
                .lock()
                .expect("writer lock poisoned")
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn logs_when_private_skill_is_created_and_updated() {
        let repo = TestRepo::new();
        let shared_root = repo.root.join("skills");
        let shared_skill_dir = shared_root.join("demo");
        fs::create_dir_all(&shared_skill_dir).expect("create shared skill dir");

        let mut meta = BTreeMap::new();
        meta.insert("name".to_string(), "demo".to_string());
        meta.insert("description".to_string(), "shared demo".to_string());
        fs::write(
            shared_skill_dir.join("SKILL.md"),
            render_frontmatter(&meta, "# Demo\nshared body"),
        )
        .expect("write shared skill");

        let memory_store = FileMemoryStore::new(repo.root.clone(), FileMemoryConfig::default());
        let store = FileSkillStore::new(shared_root, memory_store);
        let writer = SharedWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer.clone())
            .without_time()
            .with_ansi(false)
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            store
                .ensure_private_skill_copy("demo-user", "demo")
                .expect("create private skill");
            store
                .rewrite_private_skill_body("demo-user", "demo", "# Demo\nprivate body")
                .expect("rewrite private skill body");
        });

        let output = writer.contents();
        assert!(output.contains("private skill created"));
        assert!(output.contains("private skill updated"));
        assert!(output.contains("demo-user"));
        assert!(output.contains("demo"));
    }
}
