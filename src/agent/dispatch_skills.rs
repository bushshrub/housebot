//! Memory and skill tools.

use super::*;

impl Agent {
    pub(super) async fn dispatch_skills(
        &self,
        name: &str,
        args: &Value,
        user_id: &str,
    ) -> Option<ToolOutcome> {
        let outcome = match name {
            "update_memory" => {
                let new_content = str_arg(args, "memory_content");
                let _ = self.memory.save(user_id, new_content).await;
                ToolOutcome::Text("Memory updated.".to_string())
            }
            "search_memory" => {
                let query = str_arg(args, "query");
                let query = query.trim();
                if query.is_empty() {
                    return Some(ToolOutcome::Text(
                        "Error: search query cannot be blank.".to_string(),
                    ));
                }
                let content = self.memory.load(user_id).await;
                if content.trim().is_empty() {
                    ToolOutcome::Text("No memory stored for this user.".to_string())
                } else {
                    let query_lower = query.to_lowercase();
                    let matching: Vec<&str> = content
                        .lines()
                        .filter(|line| line.to_lowercase().contains(&query_lower))
                        .collect();
                    if matching.is_empty() {
                        ToolOutcome::Text(format!("No memory entries matching '{query}'."))
                    } else {
                        ToolOutcome::Text(matching.join("\n"))
                    }
                }
            }
            "use_skill" => {
                let skill_name = str_arg(args, "name");
                match self.skills.get(skill_name).await {
                    None => ToolOutcome::Text(format!("Error: Skill '{skill_name}' not found.")),
                    Some(skill) => {
                        if self.skill_enabled_for(user_id, skill_name).await {
                            let instructions = skill.effective_instructions();
                            ToolOutcome::Text(build_loaded_skill_content(&skill, instructions))
                        } else {
                            ToolOutcome::Text(format!(
                            "Error: Skill '{skill_name}' is not enabled for this user. Enable \
                             it first with the enable_skill tool (or `!skill enable {skill_name}`)."
                        ))
                        }
                    }
                }
            }
            "create_skill" => {
                let skill_name = args
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_lowercase();
                let existed = self.skills.get(&skill_name).await.is_some();
                let result =
                    tools::create_skill::dispatch_create_skill(&self.skills, user_id, args).await;
                if !existed && result.starts_with('✅') {
                    self.enable_skill_for_user(user_id, &skill_name).await;
                }
                ToolOutcome::Text(result)
            }
            "list_skills" => {
                let enabled = self.enabled_skills_for(user_id).await;
                ToolOutcome::Text(
                    tools::manage_skills::dispatch_list_skills(&self.skills, &enabled).await,
                )
            }
            "skill_info" => ToolOutcome::Text(
                tools::manage_skills::dispatch_skill_info(&self.skills, args).await,
            ),
            "delete_skill" => ToolOutcome::Text(
                tools::manage_skills::dispatch_delete_skill(&self.skills, user_id, args).await,
            ),
            "edit_skill" => ToolOutcome::Text(
                tools::manage_skills::dispatch_edit_skill(&self.skills, user_id, args).await,
            ),
            "enable_skill" => {
                let name = str_arg(args, "name").to_lowercase();
                if self.skills.get(&name).await.is_none() {
                    ToolOutcome::Text(format!(
                        "Error: Skill '{name}' not found in the marketplace."
                    ))
                } else if name == housebot_skills::SKILL_CREATOR_NAME {
                    ToolOutcome::Text(format!("Skill '{name}' is built in and always enabled."))
                } else if self.enable_skill_for_user(user_id, &name).await {
                    ToolOutcome::Text(format!(
                        "✅ Skill '{name}' enabled. You can now load it with use_skill."
                    ))
                } else {
                    ToolOutcome::Text(format!("Skill '{name}' is already enabled."))
                }
            }
            "disable_skill" => {
                let name = str_arg(args, "name").to_lowercase();
                if name == housebot_skills::SKILL_CREATOR_NAME {
                    ToolOutcome::Text(format!("Skill '{name}' is built in and always enabled."))
                } else if self.disable_skill_for_user(user_id, &name).await {
                    ToolOutcome::Text(format!("✅ Skill '{name}' disabled."))
                } else {
                    ToolOutcome::Text(format!("Skill '{name}' was not enabled."))
                }
            }
            _ => return None,
        };
        Some(outcome)
    }

    pub(crate) async fn enabled_skills_for(&self, user_id: &str) -> Vec<String> {
        let mut enabled = self
            .user_config
            .load(user_id.parse().unwrap_or(0))
            .await
            .enabled_skills;
        if !enabled
            .iter()
            .any(|name| name == housebot_skills::SKILL_CREATOR_NAME)
        {
            enabled.push(housebot_skills::SKILL_CREATOR_NAME.to_string());
        }
        enabled
    }

    /// Whether `user_id` has enabled the skill named `name`.
    async fn skill_enabled_for(&self, user_id: &str, name: &str) -> bool {
        self.enabled_skills_for(user_id)
            .await
            .iter()
            .any(|n| n == name)
    }

    /// Enable `name` for `user_id`. Returns `false` if it was already enabled.
    pub(crate) async fn enable_skill_for_user(&self, user_id: &str, name: &str) -> bool {
        let uid = user_id.parse().unwrap_or(0);
        let mut cfg = self.user_config.load(uid).await;
        if cfg.enabled_skills.iter().any(|n| n == name) {
            return false;
        }
        cfg.enabled_skills.push(name.to_string());
        if let Err(error) = self.user_config.save(uid, &cfg).await {
            tracing::error!(%error, %uid, %name, "failed to enable skill for user");
        }
        true
    }

    /// Disable `name` for `user_id`. Returns `false` if it was not enabled.
    async fn disable_skill_for_user(&self, user_id: &str, name: &str) -> bool {
        let uid = user_id.parse().unwrap_or(0);
        let mut cfg = self.user_config.load(uid).await;
        let before = cfg.enabled_skills.len();
        cfg.enabled_skills.retain(|n| n != name);
        if cfg.enabled_skills.len() == before {
            return false;
        }
        if let Err(error) = self.user_config.save(uid, &cfg).await {
            tracing::error!(%error, %uid, %name, "failed to disable skill for user");
        }
        true
    }
}
