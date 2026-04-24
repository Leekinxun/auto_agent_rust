use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use walkdir::WalkDir;

use crate::domain::skills::models::{DeleteSkillInput, SaveSkillInput, SkillDocument, SkillScope};
use crate::domain::skills::parser::{parse_frontmatter, render_frontmatter};
use crate::infra::fs::user_memory_store::FileMemoryStore;
use crate::support::sanitize::sanitize_skill_folder;

#[derive(Clone)]
pub struct FileSkillStore {
    shared_root: PathBuf,
    memory_store: FileMemoryStore,
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

        self.get_item(name, SkillScope::Private, Some(user_id))
    }

    pub fn delete_skill(&self, input: DeleteSkillInput) -> Result<SkillDocument> {
        let scope_items = self.load_scope_map(input.scope, input.user_id.as_deref())?;
        let skill = scope_items
            .get(input.name.trim())
            .cloned()
            .with_context(|| format!("Skill 不存在: {}", input.name.trim()))?;

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
