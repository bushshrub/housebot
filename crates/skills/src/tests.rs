use super::*;

fn store() -> (tempfile::TempDir, Skills) {
    let tmp = tempfile::tempdir().unwrap();
    let skills = Skills::new(tmp.path());
    (tmp, skills)
}

fn skill(name: &str) -> Skill {
    Skill {
        name: name.to_string(),
        description: Some("Does a thing".to_string()),
        instructions: "Do the thing well.".to_string(),
        created_by: Some("1".to_string()),
        ..Skill::default()
    }
}

#[tokio::test]
async fn saved_skill_becomes_a_directory_with_skill_md() {
    let (tmp, skills) = store();
    skills.save(skill("greet")).await.unwrap();

    let path = tmp.path().join("greet").join("SKILL.md");
    let raw = tokio::fs::read_to_string(&path).await.unwrap();
    assert!(raw.starts_with("---\n"));
    assert!(raw.contains("name: greet"));
    assert!(raw.contains("Do the thing well."));
}

#[tokio::test]
async fn skills_round_trip_through_disk() {
    let (_t, skills) = store();
    let mut original = skill("greet");
    original.enabled_tools = vec!["web_search".to_string()];
    original.editors = vec!["7".to_string()];
    skills.save(original).await.unwrap();

    let loaded = skills.get("greet").await.unwrap();
    assert_eq!(loaded.description.as_deref(), Some("Does a thing"));
    assert_eq!(loaded.instructions.trim(), "Do the thing well.");
    assert_eq!(loaded.enabled_tools, vec!["web_search"]);
    assert_eq!(loaded.editors, vec!["7"]);
    assert_eq!(loaded.created_by.as_deref(), Some("1"));
}

#[tokio::test]
async fn a_description_containing_a_colon_survives() {
    let (_t, skills) = store();
    let mut awkward = skill("greet");
    awkward.description = Some("Note: does a thing".to_string());
    skills.save(awkward).await.unwrap();
    assert_eq!(
        skills.get("greet").await.unwrap().description.as_deref(),
        Some("Note: does a thing")
    );
}

#[tokio::test]
async fn bundled_files_are_discovered_but_not_read() {
    let (tmp, skills) = store();
    skills.save(skill("greet")).await.unwrap();
    let dir = tmp.path().join("greet");
    tokio::fs::create_dir_all(dir.join("references"))
        .await
        .unwrap();
    tokio::fs::create_dir_all(dir.join("scripts"))
        .await
        .unwrap();
    tokio::fs::write(dir.join("references").join("guide.md"), "deep detail")
        .await
        .unwrap();
    tokio::fs::write(dir.join("scripts").join("run.py"), "print(1)")
        .await
        .unwrap();

    let loaded = skills.get("greet").await.unwrap();
    assert_eq!(loaded.references, vec!["guide.md"]);
    assert_eq!(loaded.scripts, vec!["run.py"]);
    // Level 3 content stays on disk until explicitly requested.
    assert!(!loaded.instructions.contains("deep detail"));

    let body = skills
        .read_bundled("greet", BundleKind::References, "guide.md")
        .await
        .unwrap();
    assert_eq!(body, "deep detail");
}

#[tokio::test]
async fn bundled_reads_refuse_to_escape_the_skill() {
    let (_t, skills) = store();
    skills.save(skill("greet")).await.unwrap();
    for bad in ["../SKILL.md", "a/b", "..", ""] {
        assert!(skills
            .read_bundled("greet", BundleKind::References, bad)
            .await
            .is_err());
    }
}

#[tokio::test]
async fn skill_names_that_are_not_path_segments_are_refused() {
    let (_t, skills) = store();
    for bad in ["../escape", "a/b", "", "has space", "dot.dot"] {
        let mut invalid = skill("placeholder");
        invalid.name = bad.to_string();
        assert!(
            skills.save(invalid).await.is_err(),
            "{bad:?} should be refused"
        );
    }
}

#[tokio::test]
async fn delete_removes_the_whole_directory() {
    let (tmp, skills) = store();
    skills.save(skill("greet")).await.unwrap();
    tokio::fs::create_dir_all(tmp.path().join("greet").join("scripts"))
        .await
        .unwrap();

    assert!(skills.delete("greet").await.unwrap());
    assert!(!tokio::fs::try_exists(tmp.path().join("greet"))
        .await
        .unwrap());
    assert!(!skills.delete("greet").await.unwrap());
}

#[tokio::test]
async fn skill_creator_is_builtin_and_protected() {
    let (tmp, skills) = store();
    let creator = skills.get(SKILL_CREATOR_NAME).await.unwrap();
    assert!(creator.created_by.is_none());
    assert!(creator.enabled_tools.contains(&"create_skill".to_string()));

    let mut clash = skill(SKILL_CREATOR_NAME);
    clash.name = SKILL_CREATOR_NAME.to_string();
    assert_eq!(
        skills.save(clash).await.unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert_eq!(
        skills.delete(SKILL_CREATOR_NAME).await.unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert!(!tokio::fs::try_exists(tmp.path().join(SKILL_CREATOR_NAME))
        .await
        .unwrap());
}

#[tokio::test]
async fn missing_root_yields_only_the_builtin() {
    let skills = Skills::new("/nonexistent/housebot-skills");
    let all = skills.load_all().await;
    assert_eq!(all.len(), 1);
    assert!(all.contains_key(SKILL_CREATOR_NAME));
}

#[tokio::test]
async fn one_broken_skill_does_not_hide_the_others() {
    let (tmp, skills) = store();
    skills.save(skill("good")).await.unwrap();
    let broken = tmp.path().join("broken");
    tokio::fs::create_dir_all(&broken).await.unwrap();
    tokio::fs::write(broken.join("SKILL.md"), "no frontmatter here")
        .await
        .unwrap();

    let all = skills.load_all().await;
    assert!(all.contains_key("good"));
    assert!(!all.contains_key("broken"));
}

#[tokio::test]
async fn a_directory_without_skill_md_is_skipped() {
    let (tmp, skills) = store();
    tokio::fs::create_dir_all(tmp.path().join("empty"))
        .await
        .unwrap();
    assert!(!skills.load_all().await.contains_key("empty"));
}

#[tokio::test]
async fn editing_permissions_follow_author_and_editors() {
    let mut s = skill("greet");
    assert!(s.can_edit("1"));
    assert!(!s.can_edit("2"));
    assert!(s.add_editor("2"));
    assert!(!s.add_editor("2"));
    assert!(s.can_edit("2"));
    assert!(s.remove_editor("2"));
    assert!(!s.can_edit("2"));
}

#[test]
fn frontmatter_name_wins_over_the_directory_name() {
    let parsed = parse_skill_md("---\nname: real\n---\nbody\n", "dirname").unwrap();
    assert_eq!(parsed.name, "real");
}

#[test]
fn a_missing_name_falls_back_to_the_directory() {
    let parsed = parse_skill_md("---\ndescription: x\n---\nbody\n", "dirname").unwrap();
    assert_eq!(parsed.name, "dirname");
}

#[test]
fn skill_md_without_frontmatter_is_an_error() {
    assert!(parse_skill_md("just a body", "dirname").is_err());
}
