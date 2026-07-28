//! Unit tests for `lib` (split out to keep the module under 400 lines).

use super::*;
use tempfile::TempDir;

fn store() -> (TempDir, ChannelLog) {
    let tmp = TempDir::new().unwrap();
    let log = ChannelLog::new(tmp.path().join("channel_log"));
    (tmp, log)
}

#[tokio::test]
async fn append_and_search_basic() {
    let (_t, log) = store();
    log.append(1, 10, "Alice", None, "hello world").await;
    log.append(1, 11, "Bob", None, "goodbye moon").await;
    let results = log.search(1, "hello", 10).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].username, "Alice");
    assert_eq!(results[0].content, "hello world");
}

#[tokio::test]
async fn find_authors_matches_username_nickname_and_id() {
    let (_t, log) = store();
    log.append(1, 10, "alice_dev", Some("Alice"), "hello").await;
    log.append(1, 11, "bob", Some("Builder"), "hi").await;
    log.append(2, 12, "outside", None, "hidden").await;

    assert_eq!(
        log.find_authors(1, "ALICE", 10).await.unwrap()[0].user_id,
        "10"
    );
    assert_eq!(
        log.find_authors(1, "build", 10).await.unwrap()[0].user_id,
        "11"
    );
    assert_eq!(
        log.find_authors(1, "10", 10).await.unwrap()[0].username,
        "alice_dev"
    );
    assert!(log.find_authors(1, "outside", 10).await.unwrap().is_empty());
}

#[tokio::test]
async fn find_authors_fuzzy_matches_any_word() {
    let (_t, log) = store();
    log.append(1, 10, "rice_grower", Some("Grower"), "hello")
        .await;
    log.append(1, 11, "wheat_grower", Some("Wheat Farmer"), "hi")
        .await;
    log.append(1, 12, "corn_king", Some("Corn"), "hey").await;

    // "rice farmer" should match both users 10 (username has "rice") and 11 (nick has "farmer")
    let results = log.find_authors(1, "rice farmer", 10).await.unwrap();
    assert_eq!(results.len(), 2);
    assert!(results.iter().any(|a| a.user_id == "10"));
    assert!(results.iter().any(|a| a.user_id == "11"));
}

#[tokio::test]
async fn find_authors_deduplicates_and_keeps_latest_names() {
    let (_t, log) = store();
    log.append(1, 10, "alice", None, "hello").await;
    log.append(1, 10, "alice_new", Some("Ali"), "again").await;
    let authors = log.find_authors(1, "", 10).await.unwrap();
    assert_eq!(authors.len(), 1);
    assert_eq!(authors[0].username, "alice_new");
    assert_eq!(authors[0].nick.as_deref(), Some("Ali"));
}

#[tokio::test]
async fn find_authors_ignores_punctuation_in_query() {
    let (_t, log) = store();
    log.append(1, 10, "alice_dev", Some("Alice"), "hello").await;
    log.append(1, 11, "bob_builder", Some("Bob"), "hi").await;

    // Query with punctuation should still match
    let results = log.find_authors(1, "alice!", 10).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].user_id, "10");

    let results = log.find_authors(1, "bob?", 10).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].user_id, "11");
}

#[tokio::test]
async fn find_authors_handles_typos_via_levenshtein() {
    let (_t, log) = store();
    log.append(1, 10, "alice_dev", Some("Alice"), "hello").await;
    log.append(1, 11, "jonathan", Some("Jon"), "hi").await;
    log.append(1, 12, "katherine", Some("Kat"), "hey").await;

    // "jonathin" is 1 edit from "jonathan" (username)
    let results = log.find_authors(1, "jonathin", 10).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].user_id, "11");

    // "katherina" is 1 edit from "katherine" (username)
    let results = log.find_authors(1, "katherina", 10).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].user_id, "12");
}

#[tokio::test]
async fn find_authors_fuzzy_matches_same_length_substring() {
    let (_t, log) = store();
    log.append(1, 10, "alice_dev", None, "hello").await;

    let results = log.find_authors(1, "alicf", 10).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].user_id, "10");
}

#[tokio::test]
async fn find_authors_fuzzy_matches_individual_target_word() {
    let (_t, log) = store();
    log.append(1, 11, "wheat_grower", Some("Wheat Farmer"), "hi")
        .await;

    let results = log.find_authors(1, "farme", 10).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].user_id, "11");
}

#[tokio::test]
async fn find_authors_returns_empty_for_nonempty_punctuation_only_query() {
    let (_t, log) = store();
    log.append(1, 10, "alice_dev", None, "hello").await;

    let results = log.find_authors(1, "!!!", 10).await.unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn find_authors_ignores_punctuation_in_stored_names() {
    let (_t, log) = store();
    log.append(1, 10, "alice.dev", Some("Alice"), "hello").await;
    log.append(1, 11, "bob_smith", Some("Bob-Smith"), "hi")
        .await;

    // Query without punctuation should match stored names that have punctuation
    let results = log.find_authors(1, "alicedev", 10).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].user_id, "10");

    let results = log.find_authors(1, "bobsmith", 10).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].user_id, "11");
}

#[tokio::test]
async fn search_returns_no_match() {
    let (_t, log) = store();
    log.append(1, 10, "Alice", None, "hello world").await;
    let results = log.search(1, "notfound", 10).await.unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn search_missing_channel_is_empty() {
    let (_t, log) = store();
    let results = log.search(999, "anything", 10).await.unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn search_respects_max_results() {
    let (_t, log) = store();
    for i in 0..10u64 {
        log.append(1, i, "User", None, "match").await;
    }
    let results = log.search(1, "match", 3).await.unwrap();
    assert_eq!(results.len(), 3);
}

#[tokio::test]
async fn search_returns_most_recent_when_capped() {
    let (_t, log) = store();
    log.append(1, 1, "First", None, "match").await;
    log.append(1, 2, "Second", None, "match").await;
    log.append(1, 3, "Third", None, "match").await;
    let results = log.search(1, "match", 2).await.unwrap();
    assert_eq!(results[0].username, "Second");
    assert_eq!(results[1].username, "Third");
}

#[tokio::test]
async fn search_invalid_regex_returns_error() {
    let (_t, log) = store();
    assert!(log.search(1, "[invalid", 10).await.is_err());
}

#[tokio::test]
async fn channels_are_isolated() {
    let (_t, log) = store();
    log.append(1, 10, "Alice", None, "channel one").await;
    log.append(2, 11, "Bob", None, "channel two").await;
    assert_eq!(log.search(1, "channel", 10).await.unwrap().len(), 1);
    assert_eq!(log.search(2, "channel", 10).await.unwrap().len(), 1);
    assert!(log.search(1, "two", 10).await.unwrap().is_empty());
}

#[tokio::test]
async fn search_matches_username() {
    let (_t, log) = store();
    log.append(1, 10, "AliceWonder", None, "some message").await;
    log.append(1, 11, "BobSmith", None, "another message").await;
    let results = log.search(1, "Alice", 10).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].username, "AliceWonder");
}

#[tokio::test]
async fn entries_have_valid_timestamp_and_ids() {
    let (_t, log) = store();
    log.append(1, 42, "TestUser", None, "content").await;
    let results = log.search(1, "content", 10).await.unwrap();
    assert_eq!(results[0].user_id, "42");
    assert!(!results[0].ts.is_empty());
}

#[tokio::test]
async fn remove_user_entries_removes_matching_user() {
    let (_t, log) = store();
    log.append(1, 10, "Alice", None, "hello").await;
    log.append(1, 20, "Bob", None, "world").await;
    log.append(1, 10, "Alice", None, "foo").await;
    log.remove_user_entries("10".to_string()).await.unwrap();
    let results = log.search(1, ".*", 10).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].user_id, "20");
}

#[tokio::test]
async fn remove_user_entries_preserves_other_users() {
    let (_t, log) = store();
    log.append(1, 10, "Alice", None, "hello").await;
    log.append(1, 20, "Bob", None, "world").await;
    log.append(1, 30, "Charlie", None, "bar").await;
    log.remove_user_entries("10".to_string()).await.unwrap();
    let results = log.search(1, ".*", 10).await.unwrap();
    assert_eq!(results.len(), 2);
    assert!(results.iter().any(|r| r.user_id == "20"));
    assert!(results.iter().any(|r| r.user_id == "30"));
}

#[tokio::test]
async fn remove_user_entries_noop_when_user_not_found() {
    let (_t, log) = store();
    log.append(1, 10, "Alice", None, "hello").await;
    log.remove_user_entries("999".to_string()).await.unwrap();
    let results = log.search(1, ".*", 10).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].user_id, "10");
}

#[tokio::test]
async fn append_after_remove_user_entries_does_not_corrupt_the_log() {
    let (_t, log) = store();
    log.append(1, 10, "Alice", None, "hello").await;
    log.append(1, 20, "Bob", None, "world").await;
    log.remove_user_entries("10".to_string()).await.unwrap();
    log.append(1, 30, "Charlie", None, "after removal").await;
    let results = log.search(1, ".*", 10).await.unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].user_id, "20");
    assert_eq!(results[1].user_id, "30");
    assert_eq!(results[1].content, "after removal");
}

#[tokio::test]
async fn remove_user_entries_noop_when_directory_is_missing() {
    let (_t, log) = store();
    log.remove_user_entries("10".to_string()).await.unwrap();
}

#[tokio::test]
async fn remove_user_entries_removes_from_all_channels() {
    let (_t, log) = store();
    log.append(1, 10, "Alice", None, "channel one").await;
    log.append(2, 10, "Alice", None, "channel two").await;
    log.append(1, 20, "Bob", None, "channel one bob").await;
    log.remove_user_entries("10".to_string()).await.unwrap();
    let results1 = log.search(1, ".*", 10).await.unwrap();
    let results2 = log.search(2, ".*", 10).await.unwrap();
    assert_eq!(results1.len(), 1);
    assert_eq!(results1[0].user_id, "20");
    assert!(results2.is_empty());
}

#[tokio::test]
async fn search_matches_nick() {
    let (_t, log) = store();
    log.append(1, 10, "username1", Some("Teddio"), "some message")
        .await;
    log.append(1, 11, "username2", None, "another message")
        .await;
    let results = log.search(1, "(?i)teddio", 10).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].username, "username1");
    assert_eq!(results[0].nick, Some("Teddio".to_string()));
}

#[tokio::test]
async fn get_recent_returns_messages_within_window() {
    let (_t, log) = store();
    log.append(1, 10, "Alice", None, "recent message").await;
    let results = log.get_recent(1, 5).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].content, "recent message");
}

#[tokio::test]
async fn get_recent_empty_for_missing_channel() {
    let (_t, log) = store();
    let results = log.get_recent(999, 30).await.unwrap();
    assert!(results.is_empty());
}
