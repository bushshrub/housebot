//! Docker command construction and execution for deployments.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeploymentStage {
    PullHousebotImage,
    RunDatabaseMigrations,
    RemovePreviousContainer,
    StartRequestedImage,
    CheckContainerState,
}

impl DeploymentStage {
    pub fn progress_message(self) -> &'static str {
        match self {
            Self::PullHousebotImage => "⬇️ Pulling housebot image…",
            Self::RunDatabaseMigrations => "🗄️ Applying database migrations…",
            Self::RemovePreviousContainer => "🛑 Removing the previous housebot container…",
            Self::StartRequestedImage => "🚀 Starting the requested housebot image…",
            Self::CheckContainerState => "🩺 Checking container state…",
        }
    }

    pub(crate) fn is_start(self) -> bool {
        self == Self::StartRequestedImage
    }

    pub(crate) fn is_health_check(self) -> bool {
        self == Self::CheckContainerState
    }
}

impl fmt::Display for DeploymentStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::PullHousebotImage => "pull_housebot_image",
            Self::RunDatabaseMigrations => "run_database_migrations",
            Self::RemovePreviousContainer => "remove_previous_container",
            Self::StartRequestedImage => "start_requested_image",
            Self::CheckContainerState => "check_container_state",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeploymentCommand {
    pub stage: DeploymentStage,
    pub args: Vec<String>,
}

impl DeploymentCommand {
    pub(crate) fn new(stage: DeploymentStage, args: Vec<String>) -> Self {
        Self { stage, args }
    }

    pub(crate) fn args(&self) -> Vec<&str> {
        self.args.iter().map(String::as_str).collect()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DeploymentRunSummary {
    pub(crate) container_name: String,
    pub(crate) container_id: Option<String>,
}

impl DeploymentRunSummary {
    pub(crate) fn completed_message(&self, sha: &str) -> String {
        match &self.container_id {
            Some(container_id) => format!(
                "✅ Automatic deployment of `{}` completed. Container `{}` is running as `{}`.",
                short_sha(sha),
                self.container_name,
                container_id
            ),
            None => format!(
                "✅ Automatic deployment of `{}` completed. Container `{}` is running.",
                short_sha(sha),
                self.container_name
            ),
        }
    }

    pub(crate) fn manual_completed_message(&self, sha: &str) -> String {
        match &self.container_id {
            Some(container_id) => format!(
                "✅ Deployment of `{}` completed. Container `{}` is running as `{}`.",
                short_sha(sha),
                self.container_name,
                container_id
            ),
            None => format!(
                "✅ Deployment of `{}` completed. Container `{}` is running.",
                short_sha(sha),
                self.container_name
            ),
        }
    }
}

pub fn valid_sha(sha: &str) -> bool {
    (7..=40).contains(&sha.len()) && sha.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub fn deploy_commands(
    sha: Option<&str>,
    docker_network: &str,
) -> anyhow::Result<Vec<DeploymentCommand>> {
    let main = match sha {
        Some(sha) if valid_sha(sha) => format!("ghcr.io/bushshrub/housebot:sha-{sha}"),
        Some(_) => anyhow::bail!("SHA must contain 7 to 40 hexadecimal characters"),
        None => "ghcr.io/bushshrub/housebot:latest".into(),
    };
    container_commands_with_env(&main, docker_network, housebot_env(), true)
}

pub(crate) fn valid_housebot_image(image: &str) -> bool {
    image.starts_with("ghcr.io/bushshrub/housebot:sha-")
        || image.starts_with("ghcr.io/bushshrub/housebot@sha256:")
}

pub fn container_commands(
    image: &str,
    docker_network: &str,
) -> anyhow::Result<Vec<DeploymentCommand>> {
    container_commands_with_env(image, docker_network, Vec::new(), false)
}

pub(crate) fn container_commands_with_env(
    image: &str,
    docker_network: &str,
    environment: Vec<(String, String)>,
    allow_latest: bool,
) -> anyhow::Result<Vec<DeploymentCommand>> {
    if !(valid_housebot_image(image)
        || (allow_latest && image == "ghcr.io/bushshrub/housebot:latest"))
    {
        anyhow::bail!("invalid housebot deployment image");
    }
    let mut run = vec![
        "run".into(),
        "--detach".into(),
        "--name".into(),
        HOUSE_CHATBOT_CONTAINER.into(),
        "--restart".into(),
        "unless-stopped".into(),
        "--network".into(),
        docker_network.into(),
    ];
    for (name, value) in &environment {
        run.push("--env".into());
        run.push(format!("{name}={value}"));
    }
    let mut migrate = vec![
        "run".into(),
        "--rm".into(),
        "--network".into(),
        docker_network.into(),
    ];
    for (name, value) in &environment {
        migrate.push("--env".into());
        migrate.push(format!("{name}={value}"));
    }
    migrate.extend([image.into(), "housebot".into(), "migrate".into()]);
    run.extend(["--env".into(), "DATA_DIR=/app/data".into(), image.into()]);
    Ok(vec![
        DeploymentCommand::new(
            DeploymentStage::PullHousebotImage,
            vec!["pull".into(), image.into()],
        ),
        DeploymentCommand::new(DeploymentStage::RunDatabaseMigrations, migrate),
        DeploymentCommand::new(
            DeploymentStage::RemovePreviousContainer,
            vec![
                "rm".into(),
                "--force".into(),
                HOUSE_CHATBOT_CONTAINER.into(),
            ],
        ),
        DeploymentCommand::new(DeploymentStage::StartRequestedImage, run),
        DeploymentCommand::new(
            DeploymentStage::CheckContainerState,
            vec![
                "inspect".into(),
                "--format={{.State.Running}}".into(),
                HOUSE_CHATBOT_CONTAINER.into(),
            ],
        ),
    ])
}

pub fn deploy_progress(stage: DeploymentStage) -> &'static str {
    stage.progress_message()
}

pub(crate) fn short_sha(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

pub(crate) fn docker_object_missing(error: &anyhow::Error) -> bool {
    let message = error.to_string();
    message.contains("No such object") || message.contains("No such container")
}

pub(crate) async fn run_docker(args: &[&str]) -> anyhow::Result<String> {
    let output = ProcessCommand::new("docker")
        .args(args)
        .current_dir("/")
        .output()
        .await?;
    let missing_container = args.first() == Some(&"rm")
        && (String::from_utf8_lossy(&output.stderr).contains("No such container")
            || String::from_utf8_lossy(&output.stderr).contains("No such object"));
    if !output.status.success() && !missing_container {
        anyhow::bail!(
            "docker command failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub(crate) async fn cleanup_old_housebot_images(keep: &[&str]) -> anyhow::Result<()> {
    let images = run_docker(&[
        "images",
        "--format={{.Repository}}:{{.Tag}}",
        "ghcr.io/bushshrub/housebot*",
    ])
    .await?;
    for image in images.lines().filter(|image| {
        (*image == "ghcr.io/bushshrub/housebot:latest"
            || image.starts_with("ghcr.io/bushshrub/housebot:sha-"))
            && !keep.contains(image)
    }) {
        run_docker(&["image", "rm", image]).await?;
    }
    Ok(())
}
