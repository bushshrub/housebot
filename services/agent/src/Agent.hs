module Agent
  ( runAgent
  , resetSession
  , compactSession
  , modelInfo
  ) where

import Config
import Types
import Storage
import LlmClient

import Data.Aeson
import Data.Aeson.Types (parseMaybe)
import Data.List (intercalate, isPrefixOf)
import qualified Data.Map.Strict as Map
import Data.Maybe (fromMaybe, mapMaybe)
import Data.Text (Text)
import qualified Data.Text as T
import qualified Data.Text.Encoding as T
import qualified Data.Text.Lazy as TL
import Data.Time.LocalTime (getZonedTime)
import Data.Time.Format (formatTime, defaultTimeLocale)
import Network.HTTP.Client

-- ── system prompt ─────────────────────────────────────────────────────────────

buildSystemPrompt :: Text -> Text -> Text -> Map.Map Text SkillRecord -> Maybe Text -> IO Text
buildSystemPrompt username userId memory skills personality = do
  now <- getZonedTime
  let ts = T.pack $ formatTime defaultTimeLocale "%Y-%m-%d %H:%M" now
  let memSection = if T.null (T.strip memory) then ""
        else "\n\n## Your memory about " <> username <> "\n" <> memory
  let persSection = case personality of
        Just p | not (T.null (T.strip p)) ->
          "\n\n## Personality / tone for this user\n" <> T.strip p
        _ -> ""
  let skillsSection
        | Map.null skills =
            "\n- run_skill — Execute a custom skill by name. No skills are defined yet."
        | otherwise =
            "\n- run_skill — Execute a custom skill by name. Available skills:\n" <>
            T.intercalate "\n" (map renderSkill (Map.elems skills))
  pure $ "You are a helpful house assistant bot in a Discord server. You help with media, web search, and software development tasks.\n\n\
         \Current date/time: " <> ts <> "\nCurrent user: " <> username <> " (ID: " <> userId <> ")" <>
         memSection <> persSection <>
         "\n\n## Tools\n\
         \- ddg__* — Search the web via DuckDuckGo.\n\
         \- jellyfin__* — Query Jellyfin media server. READ ONLY.\n\
         \- run_opencode — Run a coding task. Delegate all coding here.\n\
         \- update_memory — Persist facts about the user for future conversations.\n\
         \- create_feature_request — File a GitHub issue for a feature request.\n\
         \- set_reminder — Set a timed reminder via DM.\n\
         \- summarize_url — Fetch and summarize a public URL.\n\
         \- translate — Translate text to another language." <>
         skillsSection <>
         "\n\n## Guidelines\n\
         \- Be conversational and friendly.\n\
         \- Delegate all coding to run_opencode. Never write code yourself.\n\
         \- Use DuckDuckGo for factual questions.\n\
         \- Update memory when you learn something worth remembering.\n\
         \- Keep responses concise.\n\
         \- When the user's message exceeds 500 characters, begin with a TL;DR: line.\n"
  where
    renderSkill s = "  - **" <> skillName s <> "**: " <>
      fromMaybe (skillName s) (skillDescription s)

-- ── user message builder ──────────────────────────────────────────────────────

buildUserMessage :: Text -> [ImageData] -> Value
buildUserMessage text [] = object ["role" .= ("user" :: Text), "content" .= text]
buildUserMessage text imgs =
  let imgParts = map (\img -> object
        [ "type" .= ("image_url" :: Text)
        , "image_url" .= object
            [ "url" .= ("data:" <> imageMediaType img <> ";base64," <> imageData img) ]
        ]) imgs
      parts = imgParts ++ [object ["type" .= ("text" :: Text), "text" .= text]]
  in object ["role" .= ("user" :: Text), "content" .= parts]

-- ── context token estimation ──────────────────────────────────────────────────

estimateTokens :: [Value] -> Int
estimateTokens msgs = sum $ map msgLen msgs
  where
    msgLen m = (T.length $ extractContent m) `div` 4
    extractContent m = case parseMaybe (.: "content") m of
      Just (String s) -> s
      Just other      -> T.pack (show other)
      Nothing         -> ""

-- ── tool definitions ─────────────────────────────────────────────────────────

builtinTools :: Map.Map Text SkillRecord -> [Value]
builtinTools _skills =
  [ mkTool "run_opencode" "Run a coding task in a sandbox."
      (object ["type" .= ("object" :: Text), "properties" .= object
        [ "task" .= object ["type" .= ("string" :: Text)]
        , "model" .= object ["type" .= ("string" :: Text)]
        , "repo_url" .= object ["type" .= ("string" :: Text)]
        ], "required" .= (["task"] :: [Text])])
  , mkTool "update_memory" "Update persistent memory about the user."
      (object ["type" .= ("object" :: Text), "properties" .= object
        [ "memory_content" .= object ["type" .= ("string" :: Text)]
        ], "required" .= (["memory_content"] :: [Text])])
  , mkTool "run_skill" "Execute a custom skill by name."
      (object ["type" .= ("object" :: Text), "properties" .= object
        [ "name"  .= object ["type" .= ("string" :: Text)]
        , "input" .= object ["type" .= ("string" :: Text)]
        ], "required" .= (["name", "input"] :: [Text])])
  , mkTool "create_feature_request" "File a feature request GitHub issue."
      (object ["type" .= ("object" :: Text), "properties" .= object
        [ "title"       .= object ["type" .= ("string" :: Text)]
        , "description" .= object ["type" .= ("string" :: Text)]
        ], "required" .= (["title", "description"] :: [Text])])
  , mkTool "set_reminder" "Set a timed reminder."
      (object ["type" .= ("object" :: Text), "properties" .= object
        [ "message"       .= object ["type" .= ("string" :: Text)]
        , "delay_minutes" .= object ["type" .= ("number" :: Text)]
        ], "required" .= (["message", "delay_minutes"] :: [Text])])
  , mkTool "summarize_url" "Fetch and summarize a URL."
      (object ["type" .= ("object" :: Text), "properties" .= object
        [ "url" .= object ["type" .= ("string" :: Text)]
        ], "required" .= (["url"] :: [Text])])
  , mkTool "translate" "Translate text to a target language."
      (object ["type" .= ("object" :: Text), "properties" .= object
        [ "text"            .= object ["type" .= ("string" :: Text)]
        , "target_language" .= object ["type" .= ("string" :: Text)]
        ], "required" .= (["text", "target_language"] :: [Text])])
  ]
  where
    mkTool name desc params = object
      [ "type" .= ("function" :: Text)
      , "function" .= object
          [ "name"        .= (name :: Text)
          , "description" .= (desc :: Text)
          , "parameters"  .= params
          ]
      ]

-- ── tool dispatch ─────────────────────────────────────────────────────────────

getStr :: Value -> Text -> Text
getStr v k = case parseMaybe (.: k) v of
  Just s  -> s
  Nothing -> ""

dispatchTool :: Manager -> Config -> Text -> Text -> Value -> IO Text
dispatchTool mgr cfg userId toolName args =
  case toolName of
    "update_memory" -> do
      let content = getStr args "memory_content"
      saveMemory mgr cfg userId content
      pure "Memory updated."
    "run_skill" -> do
      let name = getStr args "name"
          input = getStr args "input"
      skills <- loadSkills mgr cfg
      case Map.lookup name skills of
        Nothing -> pure $ "Error: Skill '" <> name <> "' not found."
        Just s  ->
          chatOnce mgr cfg
            [ object ["role" .= ("system" :: Text), "content" .= skillPrompt s]
            , object ["role" .= ("user"   :: Text), "content" .= input]
            ]
    "translate" -> do
      let text   = getStr args "text"
          target = getStr args "target_language"
          prompt = "Translate the following text to " <> target <>
                   ". Return only the translation, no explanation.\n\n" <> text
      chatOnce mgr cfg
        [ object ["role" .= ("user" :: Text), "content" .= prompt] ]
    "summarize_url" -> do
      let url = getStr args "url"
          prompt = "Please summarize the content at this URL: " <> url
      chatOnce mgr cfg
        [ object ["role" .= ("user" :: Text), "content" .= prompt] ]
    _ | "create_feature_request" `T.isPrefixOf` toolName -> do
      let title = getStr args "title"
          desc  = getStr args "description"
      -- Call github-issues service
      pure $ "Feature request noted: " <> title
    _ -> pure $ "Error: Unknown tool " <> toolName

-- ── main agent loop ───────────────────────────────────────────────────────────

runAgent :: Manager -> Config -> RunRequest -> IO RunResponse
runAgent mgr cfg req = do
  memory  <- loadMemory mgr cfg (runUserId req)
  history <- loadHistory mgr cfg (runUserId req)
  skills  <- loadSkills mgr cfg
  system  <- buildSystemPrompt
               (runUsername req) (runUserId req)
               memory skills (runPersonality req)

  -- Context overflow guard
  let userMsg    = buildUserMessage (runText req) (runImages req)
      projected  = estimateTokens history + estimateTokens [userMsg]
      usage      = fromIntegral projected / fromIntegral (cfgMaxCtx cfg) :: Double
  (sessionNotice, history', memory') <-
    if not (null history) && usage >= 0.8
      then do
        compactSession mgr cfg (runUserId req)
        h <- loadHistory mgr cfg (runUserId req)
        m <- loadMemory  mgr cfg (runUserId req)
        pure ( Just "⚠️ Context window reached 80%, compacted and started new session."
             , h, m )
      else pure (Nothing, history, memory)

  system' <- buildSystemPrompt
               (runUsername req) (runUserId req)
               memory' skills (runPersonality req)

  let tools = builtinTools skills
  let messages0 =
        [object ["role" .= ("system" :: Text), "content" .= system']]
        ++ history' ++ [userMsg]

  (finalText, turnMsgs) <- loop messages0 tools [] 0
  _ <- appendTurn mgr cfg (runUserId req) userMsg turnMsgs
  pure RunResponse
    { Types.runText    = if T.null finalText then "(no response)" else finalText
    , runSessionNotice = sessionNotice
    }
  where
    loop messages tools accTurn depth
      | depth > 20 = pure ("(tool call limit reached)", accTurn)
      | otherwise  = do
          completion <- chatStream mgr cfg messages tools
          let assistantMsg = buildAssistantMsg completion
          let newMsgs      = messages ++ [assistantMsg]
          let newTurn      = accTurn ++ [assistantMsg]
          case (ccFinishReason completion, ccToolCalls completion) of
            (Just "tool_calls", tcs) | not (null tcs) -> do
              toolResults <- mapM (runTool newMsgs newTurn) tcs
              let (msgsList, turnList) = unzip toolResults
              loop (concat msgsList) tools (concat turnList) (depth + 1)
            _ ->
              pure (fromMaybe "" (ccContent completion), newTurn)

    runTool messages turn tc = do
      let args = case decode (encodeUtf8 (tcArguments tc)) of
                   Just v  -> v
                   Nothing -> object []
      result <- dispatchTool mgr cfg (runUserId req) (tcName tc) args
      let toolMsg = object
            [ "role"         .= ("tool" :: Text)
            , "tool_call_id" .= tcId tc
            , "content"      .= result
            ]
      pure (messages ++ [toolMsg], turn ++ [toolMsg])

    buildAssistantMsg completion =
      let base = [ "role"    .= ("assistant" :: Text)
                 , "content" .= ccContent completion
                 ]
          withTcs = if null (ccToolCalls completion) then base
            else base ++
              [ "tool_calls" .= map tcToJson (ccToolCalls completion) ]
      in object withTcs

    tcToJson tc = object
      [ "id"   .= tcId tc
      , "type" .= ("function" :: Text)
      , "function" .= object
          [ "name"      .= tcName tc
          , "arguments" .= tcArguments tc
          ]
      ]

encodeUtf8 :: Text -> BL.ByteString
encodeUtf8 = BL.fromStrict . T.encodeUtf8

-- ── session management ────────────────────────────────────────────────────────

resetSession :: Manager -> Config -> Text -> IO ()
resetSession mgr cfg userId = clearHistory mgr cfg userId

compactSession :: Manager -> Config -> Text -> IO ()
compactSession mgr cfg userId = do
  history <- loadHistory mgr cfg userId
  if null history
    then pure ()
    else do
      let convo = T.intercalate "\n" $ mapMaybe extractLine history
          truncated = T.take 6000 convo
          prompt = "The following is a conversation that has ended. Write a concise bullet-point summary \
                   \of the key facts, preferences, and decisions discussed. Be brief — 3-8 bullets max.\n\n\
                   \CONVERSATION:\n" <> truncated
      summary <- chatOnce mgr cfg
        [object ["role" .= ("user" :: Text), "content" .= prompt]]
      unless (T.null (T.strip summary)) $ do
        mem <- loadMemory mgr cfg userId
        now <- getZonedTime
        let ts      = T.pack $ formatTime defaultTimeLocale "%Y-%m-%d %H:%M" now
            newMem  = (if T.null (T.strip mem) then "" else T.strip mem <> "\n\n")
                   <> "## Conversation summary (" <> ts <> ")\n" <> summary
        saveMemory mgr cfg userId newMem
      clearHistory mgr cfg userId
  where
    extractLine m = case (parseMaybe (.: "role") m, parseMaybe (.: "content") m) of
      (Just r, Just c) -> Just (T.toUpper r <> ": " <> c)
      _                -> Nothing

modelInfo :: Config -> Text
modelInfo cfg = "**Model**\nName: `" <> T.pack (cfgModel cfg) <>
                "`\nMax context: ~" <> T.pack (show (cfgMaxCtx cfg)) <> " tokens"

unless :: Bool -> IO () -> IO ()
unless True  _ = pure ()
unless False m = m
