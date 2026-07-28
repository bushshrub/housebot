//! Develop components.

use super::*;

// ── develop flow component builders ──────────────────────────────────────────

pub(crate) fn develop_approval_components(job_id: &str) -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new(format!("{DEVELOP_PREFIX}{job_id}:approve"))
            .label("Start work")
            .style(ButtonStyle::Success),
        CreateButton::new(format!("{DEVELOP_PREFIX}{job_id}:configure"))
            .label("Change configuration")
            .style(ButtonStyle::Secondary),
        CreateButton::new(format!("{DEVELOP_PREFIX}{job_id}:reject"))
            .label("Reject")
            .style(ButtonStyle::Danger),
    ])]
}

pub(crate) const AGENT_DISABLED_MESSAGE: &str =
    "Codex dispatch is temporarily disabled. Please choose another agent.";

/// Temporary Codex disable, checked on every dispatch path — not just the
/// interactive picker — so configured defaults and stored selections cannot
/// bypass it.
pub(crate) fn agent_dispatch_disabled(agent: CodingAgent) -> bool {
    agent == CodingAgent::Codex
}

pub(crate) fn develop_agent_components(job_id: &str) -> Vec<CreateActionRow> {
    // Discord cannot grey out a single select option, so the disabled state is
    // conveyed via the label/description and enforced in `develop_on_agent`.
    let options = vec![
        CreateSelectMenuOption::new("Claude Code", "claude"),
        CreateSelectMenuOption::new("OpenCode (NVIDIA)", "opencode"),
        CreateSelectMenuOption::new("🚫 Codex (disabled)", "codex")
            .description("Temporarily disabled — cannot be selected"),
    ];
    vec![
        CreateActionRow::SelectMenu(
            CreateSelectMenu::new(
                format!("{DEVELOP_PREFIX}{job_id}:agent"),
                CreateSelectMenuKind::String { options },
            )
            .placeholder("Select coding agent"),
        ),
        CreateActionRow::Buttons(vec![CreateButton::new(format!(
            "{DEVELOP_PREFIX}{job_id}:cancel"
        ))
        .label("Cancel")
        .style(ButtonStyle::Danger)]),
    ]
}

pub(crate) fn develop_model_components(
    job_id: &str,
    agent: CodingAgent,
    catalog: &AgentCatalog,
) -> Vec<CreateActionRow> {
    let models = catalog.models_for(agent);
    if models.is_empty() {
        return vec![CreateActionRow::Buttons(vec![CreateButton::new(format!(
            "{DEVELOP_PREFIX}{job_id}:cancel"
        ))
        .label("No models available — Cancel")
        .style(ButtonStyle::Danger)])];
    }
    let options: Vec<CreateSelectMenuOption> = models
        .iter()
        .map(|m| {
            let mut opt = CreateSelectMenuOption::new(&m.display_name, &m.id);
            if let Some(desc) = &m.description {
                opt = opt.description(desc.chars().take(100).collect::<String>());
            }
            opt
        })
        .collect();
    vec![
        CreateActionRow::SelectMenu(
            CreateSelectMenu::new(
                format!("{DEVELOP_PREFIX}{job_id}:model"),
                CreateSelectMenuKind::String { options },
            )
            .placeholder("Select model"),
        ),
        CreateActionRow::Buttons(vec![
            CreateButton::new(format!("{DEVELOP_PREFIX}{job_id}:back"))
                .label("← Back")
                .style(ButtonStyle::Secondary),
            CreateButton::new(format!("{DEVELOP_PREFIX}{job_id}:cancel"))
                .label("Cancel")
                .style(ButtonStyle::Danger),
        ]),
    ]
}

pub(crate) fn develop_effort_components(
    job_id: &str,
    agent: CodingAgent,
    model: &str,
    catalog: &AgentCatalog,
) -> Vec<CreateActionRow> {
    let efforts = catalog.efforts_for(agent, model).unwrap_or(&[]);
    if efforts.is_empty() {
        return vec![CreateActionRow::Buttons(vec![
            CreateButton::new(format!("{DEVELOP_PREFIX}{job_id}:confirm"))
                .label("Dispatch (no effort selection needed)")
                .style(ButtonStyle::Success),
            CreateButton::new(format!("{DEVELOP_PREFIX}{job_id}:back"))
                .label("← Back")
                .style(ButtonStyle::Secondary),
            CreateButton::new(format!("{DEVELOP_PREFIX}{job_id}:cancel"))
                .label("Cancel")
                .style(ButtonStyle::Danger),
        ])];
    }
    let options: Vec<CreateSelectMenuOption> = efforts
        .iter()
        .map(|e| {
            let mut opt = CreateSelectMenuOption::new(&e.display_name, &e.id);
            if let Some(desc) = &e.description {
                opt = opt.description(desc.chars().take(100).collect::<String>());
            }
            opt
        })
        .collect();
    vec![
        CreateActionRow::SelectMenu(
            CreateSelectMenu::new(
                format!("{DEVELOP_PREFIX}{job_id}:effort"),
                CreateSelectMenuKind::String { options },
            )
            .placeholder("Select effort level"),
        ),
        CreateActionRow::Buttons(vec![
            CreateButton::new(format!("{DEVELOP_PREFIX}{job_id}:back"))
                .label("← Back")
                .style(ButtonStyle::Secondary),
            CreateButton::new(format!("{DEVELOP_PREFIX}{job_id}:cancel"))
                .label("Cancel")
                .style(ButtonStyle::Danger),
        ]),
    ]
}

pub(crate) fn develop_confirm_components(job_id: &str) -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new(format!("{DEVELOP_PREFIX}{job_id}:confirm"))
            .label("Dispatch")
            .style(ButtonStyle::Success),
        CreateButton::new(format!("{DEVELOP_PREFIX}{job_id}:back"))
            .label("← Change Effort")
            .style(ButtonStyle::Secondary),
        CreateButton::new(format!("{DEVELOP_PREFIX}{job_id}:cancel"))
            .label("Cancel")
            .style(ButtonStyle::Danger),
    ])]
}
