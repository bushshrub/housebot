//! Skills stored as directories on the bot's persistent volume.
//!
//! ```text
//! <SKILLS_DIR>/<skill-name>/
//!   SKILL.md      # YAML frontmatter + instruction body
//!   references/   # read on demand
//!   scripts/      # executed in the sandbox on demand
//! ```
//!
//! Progressive disclosure has three levels: the system prompt carries skill
//! *names* only, `SKILL.md`'s body is loaded when a skill is invoked, and
//! `references/` and `scripts/` are opened only when the body calls for them.
//!
//! A skill's description is user-authored and never reaches the system prompt —
//! it is surfaced through the `list_skills` tool result instead, where it is
//! data the model read rather than instructions it believes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::Mutex;

use housebot_config as config;
use housebot_memory::ensure_dir;

pub mod frontmatter;

pub const SKILL_CREATOR_NAME: &str = "skill_creator";
const SKILL_FILE: &str = "SKILL.md";
const REFERENCES_DIR: &str = "references";
const SCRIPTS_DIR: &str = "scripts";

/// A packaged unit of capability. Globally visible and usable by anyone;
/// editing and deletion are restricted to the author and delegated editors.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Skill {
    pub name: String,
    pub description: Option<String>,
    /// The `SKILL.md` body — the instructions loaded at disclosure level 2.
    pub instructions: String,
    /// Tools the skill suggests. Advisory only: this never narrows the agent's
    /// tool surface, it is shown as a recommendation when the skill loads.
    pub enabled_tools: Vec<String>,
    /// Discord user ID of the skill's author.
    pub created_by: Option<String>,
    /// Discord user IDs of delegated editors.
    pub editors: Vec<String>,
    pub created_at: u64,
    pub updated_at: u64,
    /// Bundled file names discovered beside `SKILL.md`, for disclosure level 3.
    pub references: Vec<String>,
    pub scripts: Vec<String>,
}

impl Skill {
    /// Description, falling back to the skill name when absent.
    pub fn description_or_name(&self) -> &str {
        self.description.as_deref().unwrap_or(&self.name)
    }

    pub fn is_author(&self, user_id: &str) -> bool {
        self.created_by.as_deref() == Some(user_id)
    }

    /// Whether `user_id` may edit or delete this skill.
    pub fn can_edit(&self, user_id: &str) -> bool {
        self.is_author(user_id) || self.editors.iter().any(|e| e == user_id)
    }

    /// Add a delegated editor. Returns `false` if already present.
    pub fn add_editor(&mut self, editor_id: &str) -> bool {
        if self.editors.iter().any(|e| e == editor_id) {
            false
        } else {
            self.editors.push(editor_id.to_string());
            true
        }
    }

    /// Remove a delegated editor. Returns `false` if not found.
    pub fn remove_editor(&mut self, editor_id: &str) -> bool {
        let before = self.editors.len();
        self.editors.retain(|e| e != editor_id);
        self.editors.len() < before
    }

    pub fn effective_instructions(&self) -> &str {
        &self.instructions
    }

    /// A one-line summary of the skill's bundled files, empty when it has none.
    /// Names only — level 3 content stays on disk until it is asked for.
    pub fn bundled_summary(&self) -> String {
        let mut out = String::new();
        if !self.references.is_empty() {
            out.push_str(&format!("\n**References:** {}", self.references.join(", ")));
        }
        if !self.scripts.is_empty() {
            out.push_str(&format!("\n**Scripts:** {}", self.scripts.join(", ")));
        }
        out
    }

    /// Render the skill as the `SKILL.md` text that `parse_skill_md` reads back.
    pub fn to_skill_md(&self) -> String {
        let mut out = String::from("---\n");
        out.push_str(&format!("name: {}\n", frontmatter::emit_scalar(&self.name)));
        if let Some(description) = &self.description {
            out.push_str(&format!(
                "description: {}\n",
                frontmatter::emit_scalar(description)
            ));
        }
        if let Some(created_by) = &self.created_by {
            out.push_str(&format!(
                "created_by: {}\n",
                frontmatter::emit_scalar(created_by)
            ));
        }
        if !self.editors.is_empty() {
            out.push_str(&format!("editors: [{}]\n", join_list(&self.editors)));
        }
        if !self.enabled_tools.is_empty() {
            out.push_str(&format!(
                "enabled_tools: [{}]\n",
                join_list(&self.enabled_tools)
            ));
        }
        out.push_str(&format!("created_at: {}\n", self.created_at));
        out.push_str(&format!("updated_at: {}\n", self.updated_at));
        out.push_str("---\n");
        out.push_str(&self.instructions);
        if !self.instructions.ends_with('\n') {
            out.push('\n');
        }
        out
    }
}

fn join_list(items: &[String]) -> String {
    items
        .iter()
        .map(|item| frontmatter::emit_scalar(item))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Parse a `SKILL.md`. `fallback_name` is used when the frontmatter omits
/// `name`, so a directory rename cannot orphan its skill.
pub fn parse_skill_md(source: &str, fallback_name: &str) -> Result<Skill, String> {
    let (block, body) = frontmatter::split(source)
        .ok_or_else(|| "SKILL.md must begin with a '---' frontmatter block".to_string())?;
    let front = frontmatter::parse(block)?;
    let name = front
        .scalar("name")
        .filter(|name| !name.is_empty())
        .unwrap_or(fallback_name)
        .to_string();
    Ok(Skill {
        name,
        description: front
            .scalar("description")
            .filter(|d| !d.is_empty())
            .map(str::to_string),
        instructions: body.to_string(),
        enabled_tools: front.list("enabled_tools"),
        created_by: front
            .scalar("created_by")
            .filter(|c| !c.is_empty())
            .map(str::to_string),
        editors: front.list("editors"),
        created_at: front.number("created_at"),
        updated_at: front.number("updated_at"),
        references: Vec::new(),
        scripts: Vec::new(),
    })
}

/// Reject names that are not a single safe path segment. Skill names come from
/// users and become directory names, so traversal and separators are refused
/// outright rather than sanitised into something the caller did not ask for.
pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("skill name cannot be empty".into());
    }
    if name.len() > 64 {
        return Err("skill name cannot exceed 64 characters".into());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(
            "skill name may only contain lowercase letters, digits, and underscores".into(),
        );
    }
    Ok(())
}

fn builtin_skill_creator() -> Skill {
    Skill {
        name: SKILL_CREATOR_NAME.to_string(),
        description: Some(
            "Design clear, reusable Housebot skills through a review-first workflow.".to_string(),
        ),
        instructions: "Help the user design or improve a Housebot skill. First clarify the \
            desired behaviour, its boundaries, and the tools it genuinely needs. Prefer focused \
            instructions over broad personality prompts, and recommend only tools that actually \
            exist. A skill is a directory: SKILL.md holds the instructions, references/ holds \
            material to read on demand, and scripts/ holds code run in the sandbox. Keep SKILL.md \
            short and move detail into references/ so it is loaded only when needed. Sandbox \
            scripts have no network access — have the agent gather data and pass it in. Check \
            list_skills before choosing a name so you do not duplicate an existing skill. Present \
            a concise final draft of the name, description, instructions, and recommended tools, \
            and obtain explicit user approval before calling create_skill or edit_skill."
            .to_string(),
        enabled_tools: vec![
            "list_skills".to_string(),
            "skill_info".to_string(),
            "create_skill".to_string(),
            "edit_skill".to_string(),
        ],
        created_by: None,
        editors: Vec::new(),
        created_at: 0,
        updated_at: 0,
        references: Vec::new(),
        scripts: Vec::new(),
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Handle to the on-disk skills directory.
#[derive(Clone)]
pub struct Skills {
    root: PathBuf,
    cache: Arc<Mutex<Option<BTreeMap<String, Skill>>>>,
}

impl Default for Skills {
    fn default() -> Self {
        Self::new(config::skills_dir())
    }
}

impl Skills {
    /// Create a store rooted at `root`, one directory per skill.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            cache: Arc::new(Mutex::new(None)),
        }
    }

    fn skill_dir(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    async fn scan(&self) -> std::io::Result<BTreeMap<String, Skill>> {
        {
            if let Some(skills) = &*self.cache.lock().await {
                return Ok(skills.clone());
            }
        }
        let mut skills = BTreeMap::new();
        match tokio::fs::read_dir(&self.root).await {
            Ok(mut entries) => {
                while let Some(entry) = entries.next_entry().await? {
                    if !entry.file_type().await?.is_dir() {
                        continue;
                    }
                    let Some(dir_name) = entry.file_name().to_str().map(str::to_string) else {
                        continue;
                    };
                    if validate_name(&dir_name).is_err() {
                        continue;
                    }
                    match load_skill_dir(&entry.path(), &dir_name).await {
                        Ok(skill) => {
                            skills.insert(skill.name.clone(), skill);
                        }
                        // One unreadable skill must not take the whole store
                        // down: skip it loudly and keep the rest usable.
                        Err(error) => tracing::error!(
                            target: "housebot::skills",
                            %error,
                            skill = %dir_name,
                            "Skipping unreadable skill"
                        ),
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        skills.insert(SKILL_CREATOR_NAME.to_string(), builtin_skill_creator());
        *self.cache.lock().await = Some(skills.clone());
        Ok(skills)
    }

    /// Load every skill, keyed by name (cached after the first scan).
    pub async fn load_all(&self) -> BTreeMap<String, Skill> {
        match self.scan().await {
            Ok(skills) => skills,
            Err(error) => {
                tracing::error!(
                    target: "housebot::skills",
                    %error,
                    root = %self.root.display(),
                    "Failed to scan skills directory — returning built-ins without caching"
                );
                BTreeMap::from([(SKILL_CREATOR_NAME.to_string(), builtin_skill_creator())])
            }
        }
    }

    /// Fetch a single skill by name.
    pub async fn get(&self, name: &str) -> Option<Skill> {
        self.load_all().await.remove(name)
    }

    /// Write a skill's `SKILL.md`, creating its directory if needed.
    pub async fn save(&self, mut skill: Skill) -> std::io::Result<()> {
        if skill.name == SKILL_CREATOR_NAME {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "built-in skills cannot be overwritten",
            ));
        }
        validate_name(&skill.name)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
        if skill.created_at == 0 {
            skill.created_at = now_secs();
        }
        skill.updated_at = now_secs();

        let dir = self.skill_dir(&skill.name);
        ensure_dir(&dir).await?;
        let mut file = tokio::fs::File::create(dir.join(SKILL_FILE)).await?;
        {
            use tokio::io::AsyncWriteExt;
            file.write_all(skill.to_skill_md().as_bytes()).await?;
            file.flush().await?;
        }
        self.cache.lock().await.take();
        Ok(())
    }

    /// Delete a skill's whole directory, returning whether it existed.
    pub async fn delete(&self, name: &str) -> std::io::Result<bool> {
        if name == SKILL_CREATOR_NAME {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "built-in skills cannot be deleted",
            ));
        }
        validate_name(name)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
        let dir = self.skill_dir(name);
        if !tokio::fs::try_exists(&dir).await.unwrap_or(false) {
            return Ok(false);
        }
        tokio::fs::remove_dir_all(&dir).await?;
        self.cache.lock().await.take();
        Ok(true)
    }

    /// Read one bundled file (disclosure level 3).
    ///
    /// `kind` selects `references/` or `scripts/`; `file` must be a plain file
    /// name, since it originates with the model.
    pub async fn read_bundled(
        &self,
        name: &str,
        kind: BundleKind,
        file: &str,
    ) -> Result<String, String> {
        validate_name(name)?;
        if file.is_empty() || file.contains('/') || file.contains('\\') || file.contains("..") {
            return Err(format!("Error: '{file}' is not a valid file name."));
        }
        let path = self.skill_dir(name).join(kind.dir_name()).join(file);
        tokio::fs::read_to_string(&path)
            .await
            .map_err(|error| format!("Error: could not read {}/{file}: {error}", kind.dir_name()))
    }
}

/// Which bundled directory of a skill to address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleKind {
    References,
    Scripts,
}

impl BundleKind {
    pub fn dir_name(self) -> &'static str {
        match self {
            BundleKind::References => REFERENCES_DIR,
            BundleKind::Scripts => SCRIPTS_DIR,
        }
    }
}

async fn load_skill_dir(dir: &Path, fallback_name: &str) -> Result<Skill, String> {
    let source = tokio::fs::read_to_string(dir.join(SKILL_FILE))
        .await
        .map_err(|error| format!("{SKILL_FILE}: {error}"))?;
    let mut skill = parse_skill_md(&source, fallback_name)?;
    skill.references = list_dir(&dir.join(REFERENCES_DIR)).await;
    skill.scripts = list_dir(&dir.join(SCRIPTS_DIR)).await;
    Ok(skill)
}

async fn list_dir(dir: &Path) -> Vec<String> {
    let mut names = Vec::new();
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return names;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if entry
            .file_type()
            .await
            .map(|t| t.is_file())
            .unwrap_or(false)
        {
            if let Some(name) = entry.file_name().to_str() {
                names.push(name.to_string());
            }
        }
    }
    names.sort();
    names
}

#[cfg(test)]
mod tests;
