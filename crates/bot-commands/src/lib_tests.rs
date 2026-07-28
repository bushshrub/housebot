//! Unit tests for `lib` (split out to keep the module under 400 lines).

use super::*;
use housebot_bot_config::UserConfigStore;
use housebot_channel_log::ChannelLog;
use housebot_grocery::GroceryList;
use housebot_history::History;
use housebot_memory::Memory;
use housebot_message_log::MessageLog;
use housebot_notes::Notes;
use housebot_profile::ProfileStore;
use housebot_reminders::Reminders;
use tempfile::TempDir;

fn stores() -> (
    TempDir,
    MessageLog,
    History,
    Memory,
    Notes,
    ProfileStore,
    UserConfigStore,
    Reminders,
    ChannelLog,
    GroceryList,
) {
    let tmp = TempDir::new().unwrap();
    let msg_log = MessageLog::new(tmp.path().join("message_log"));
    let history = History::new(tmp.path().join("history"), 30);
    let memory = Memory::new(tmp.path().join("memories"));
    let notes = Notes::new(tmp.path().join("notes"));
    let profile = ProfileStore::new(tmp.path().join("profiles"));
    let user_config = UserConfigStore::new(tmp.path().join("user_config"));
    let reminders = Reminders::new(tmp.path().join("reminders.json"));
    let channel_log = ChannelLog::new(tmp.path().join("channel_log"));
    let grocery = GroceryList::new(tmp.path().join("grocery"));
    (
        tmp,
        msg_log,
        history,
        memory,
        notes,
        profile,
        user_config,
        reminders,
        channel_log,
        grocery,
    )
}

#[tokio::test]
async fn erase_data_clears_all_stores() {
    let (
        _tmp,
        msg_log,
        history,
        memory,
        notes,
        profile,
        user_config,
        reminders,
        channel_log,
        grocery,
    ) = stores();
    let user_id = 123u64;

    // Populate all stores
    msg_log.append(user_id.to_string(), "test").await;
    history
        .save(
            user_id.to_string(),
            &[serde_json::json!({"role":"user","content":"hi"})],
        )
        .await
        .unwrap();
    memory
        .save(user_id.to_string(), "some memory")
        .await
        .unwrap();
    notes.save(user_id, "test", "content").await.unwrap();
    profile
        .save(
            user_id.to_string(),
            &housebot_profile::UserProfile {
                username: "alice".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    user_config
        .save(
            user_id,
            &housebot_bot_config::UserConfig {
                deep_memory_enabled: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    reminders
        .add(
            &user_id.to_string(),
            "reminder",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs_f64()
                + 60.0,
        )
        .await
        .unwrap();
    channel_log
        .append(1, user_id, "Alice", None, "channel msg")
        .await;
    grocery.add(user_id, "milk").await.unwrap();

    let reply = erase_data_command(
        &msg_log,
        &history,
        &memory,
        &notes,
        &profile,
        &user_config,
        &reminders,
        &channel_log,
        &grocery,
        user_id,
    )
    .await;

    assert!(reply.contains("erased"));
    assert!(reply.contains("message log"));
    assert!(reply.contains("conversation history"));
    assert!(reply.contains("memory"));
    assert!(reply.contains("notes"));
    assert!(reply.contains("profile"));
    assert!(reply.contains("reminders"));

    // Verify stores are cleared
    assert!(history.load(user_id.to_string()).await.is_empty());
    assert_eq!(memory.load(user_id.to_string()).await, "");
    assert!(notes.load_all(user_id).await.is_empty());
    assert_eq!(profile.load(user_id.to_string()).await.username, "");
    assert!(user_config.load(user_id).await.deep_memory_enabled);
    assert!(reminders.load().await.is_empty());
    assert!(grocery.load(user_id).await.is_empty());
}

#[tokio::test]
async fn erase_data_preserves_other_users() {
    let (
        _tmp,
        msg_log,
        history,
        memory,
        notes,
        profile,
        user_config,
        reminders,
        channel_log,
        grocery,
    ) = stores();
    let user_a = 100u64;
    let user_b = 200u64;

    // Populate stores with both users
    msg_log.append(user_a.to_string(), "a").await;
    msg_log.append(user_b.to_string(), "b").await;
    history
        .save(
            user_a.to_string(),
            &[serde_json::json!({"role":"user","content":"a"})],
        )
        .await
        .unwrap();
    history
        .save(
            user_b.to_string(),
            &[serde_json::json!({"role":"user","content":"b"})],
        )
        .await
        .unwrap();
    reminders
        .add(
            &user_a.to_string(),
            "reminder_a",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs_f64()
                + 60.0,
        )
        .await
        .unwrap();
    reminders
        .add(
            &user_b.to_string(),
            "reminder_b",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs_f64()
                + 60.0,
        )
        .await
        .unwrap();
    channel_log.append(1, user_a, "Alice", None, "msg a").await;
    channel_log.append(1, user_b, "Bob", None, "msg b").await;

    // Erase user A
    erase_data_command(
        &msg_log,
        &history,
        &memory,
        &notes,
        &profile,
        &user_config,
        &reminders,
        &channel_log,
        &grocery,
        user_a,
    )
    .await;

    // Verify user B is preserved
    assert_eq!(history.load(user_b.to_string()).await.len(), 1);
    let remaining_reminders = reminders.load().await;
    assert_eq!(remaining_reminders.len(), 1);
    assert_eq!(remaining_reminders[0].user_id, user_b.to_string());
}
