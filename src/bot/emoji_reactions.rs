//! Dynamic emoji reactions added to bot messages based on content analysis.

use serenity::all::ReactionType;

/// Pick contextually relevant emoji reactions for a given text.
/// Returns all matching reactions without limit.
pub fn select_reactions(text: &str) -> Vec<ReactionType> {
    let lower = text.to_ascii_lowercase();
    let mut selected = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for &(keywords, emoji) in RULES {
        if keywords.iter().any(|kw| lower.contains(kw)) && seen.insert(emoji) {
            selected.push(ReactionType::Unicode(emoji.to_string()));
        }
    }

    if selected.is_empty() {
        selected.push(ReactionType::Unicode("\u{1F44D}".to_string()));
    }

    selected
}

type Rule = (&'static [&'static str], &'static str);

/// Ordered keyword → emoji mappings.
const RULES: &[Rule] = &[
    (&["thank", "thanks", "thx"], "\u{1F64F}"),
    (&["good morning", "good evening", "gm"], "\u{1F305}"),
    (&["good night", "gn", "sweet dreams"], "\u{1F31C}"),
    (
        &["congrat", "🎉", "celebrate", "hooray", "hurray"],
        "\u{1F389}",
    ),
    (&["welcome", "hello", "hey", "hi ", "howdy"], "\u{1F44B}"),
    (&["sorry", "apologize", "apologies", "my bad"], "\u{1F614}"),
    (&["sad", "unfortunate", "alas", "depress"], "\u{1F622}"),
    (
        &[
            "lol",
            "lmao",
            "haha",
            "hehe",
            "funny",
            "😂",
            "joke",
            "hilarious",
        ],
        "\u{1F604}",
    ),
    (
        &["love", "❤️", "heart", "cute", "adorable", "precious"],
        "\u{2764}\u{FE0F}",
    ),
    (
        &[
            "great",
            "awesome",
            "amazing",
            "excellent",
            "fantastic",
            "wonderful",
            "incredible",
        ],
        "\u{1F389}",
    ),
    (
        &["nice", "good job", "well done", "proud of", "brilliant"],
        "\u{1F44F}",
    ),
    (
        &["idea", "suggest", "tip", "pro tip", "brainstorm"],
        "\u{1F4A1}",
    ),
    (&["reminder", "remember", "⏰"], "\u{23F0}"),
    (&["help", "need", "please assist"], "\u{1F64F}"),
    (
        &[
            "success",
            "complete",
            "done",
            "finished",
            "🏆",
            "victory",
            "win",
            "achievement",
        ],
        "\u{1F3C6}",
    ),
    (
        &[
            "yes",
            "correct",
            "exactly",
            "absolutely",
            "agree",
            "right",
            "indeed",
        ],
        "\u{1F44D}",
    ),
    (
        &["no", "incorrect", "disagree", "wrong", "nope", "nah"],
        "\u{274C}",
    ),
    (
        &["code", "programming", "software", "develop", "engineer"],
        "\u{1F4BB}",
    ),
    (&["bug", "debug", "error", "fix", "issue"], "\u{1F41B}"),
    (&["rust", "cargo", "🦀"], "\u{1F980}"),
    (&["deploy", "release", "launch", "ship", "🚀"], "\u{1F680}"),
    (
        &["search", "find", "look up", "🔍", "research"],
        "\u{1F50D}",
    ),
    (
        &["warning", "caution", "careful", "danger", "🚨"],
        "\u{26A0}\u{FE0F}",
    ),
    (
        &[
            "read",
            "book",
            "learn",
            "study",
            "documentation",
            "article",
            "reading",
        ],
        "\u{1F4D6}",
    ),
    (
        &[
            "music", "song", "playlist", "concert", "🎵", "melody", "tune",
        ],
        "\u{1F3B5}",
    ),
    (
        &[
            "food", "eat", "cook", "recipe", "hungry", "pizza", "coffee", "tea", "dinner", "lunch",
        ],
        "\u{1F37D}\u{FE0F}",
    ),
    (
        &[
            "travel",
            "trip",
            "vacation",
            "flight",
            "✈️",
            "journey",
            "adventure",
        ],
        "\u{2708}\u{FE0F}",
    ),
    (
        &[
            "money",
            "price",
            "cost",
            "💰",
            "budget",
            "expensive",
            "cheap",
            "pricey",
        ],
        "\u{1F4B0}",
    ),
    (
        &[
            "time", "schedule", "today", "tomorrow", "week", "deadline", "⏳", "minute", "hour",
        ],
        "\u{1F4C5}",
    ),
    (
        &["health", "exercise", "workout", "💪", "fitness", "gym"],
        "\u{1F4AA}",
    ),
    (
        &["good", "well", "better", "improve", "📈", "progress"],
        "\u{1F44D}",
    ),
    (
        &[
            "weather", "sunny", "rain", "snow", "storm", "🌤️", "☀️", "🌧️",
        ],
        "\u{1F326}\u{FE0F}",
    ),
    (
        &[
            "garden", "nature", "flower", "tree", "plant", "🌿", "🌻", "🌳",
        ],
        "\u{1F331}",
    ),
    (&["hot", "warm", "🔥", "fire", "lit"], "\u{1F525}"),
    (
        &["cold", "freeze", "❄️", "ice", "snow", "winter"],
        "\u{2744}\u{FE0F}",
    ),
    (&["question", "query", "?"], "\u{2753}"),
    (
        &["happy", "glad", "joy", "delight", "😊", "cheer"],
        "\u{1F60A}",
    ),
    (&["game", "gaming", "play", "🎮", "gamer"], "\u{1F3AE}"),
    (
        &["photo", "picture", "image", "camera", "📷", "photograph"],
        "\u{1F4F7}",
    ),
    (
        &["video", "movie", "film", "watch", "🎬", "cinema"],
        "\u{1F3AC}",
    ),
    (
        &["art", "draw", "paint", "creative", "design", "🎨"],
        "\u{1F3A8}",
    ),
    (
        &["science", "research", "experiment", "🔬", "lab", "discover"],
        "\u{1F52C}",
    ),
    (
        &["star", "🌟", "shine", "bright", "sparkle", "✨"],
        "\u{2728}",
    ),
    (&["party", "🎊", "celebration", "festival"], "\u{1F38A}"),
    (&["sleep", "tired", "😴", "rest", "nap", "bed"], "\u{1F634}"),
    (&["coffee", "tea", "caffeine", "☕"], "\u{2615}"),
    (
        &["beer", "wine", "drink", "🍺", "cheers", "toast"],
        "\u{1F37A}",
    ),
    (&["dog", "puppy", "🐶", "🐕"], "\u{1F436}"),
    (&["cat", "kitten", "🐱", "🐈", "meow"], "\u{1F431}"),
];
