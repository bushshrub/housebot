module Storage
  ( loadMemory, saveMemory
  , loadHistory, appendTurn, clearHistory
  , loadSkills
  , loadNotes, saveNote, deleteNote
  ) where

import Config
import Types

import Data.Aeson
import Data.Aeson.Types (parseMaybe)
import qualified Data.ByteString.Lazy as BL
import qualified Data.Map.Strict as Map
import Data.Text (Text)
import qualified Data.Text as T
import Network.HTTP.Client
import Network.HTTP.Client.TLS (newTlsManager)
import Network.HTTP.Types (methodGet, methodPut, methodPost, methodDelete, statusIsSuccessful)

-- ── helpers ───────────────────────────────────────────────────────────────────

getJSON :: Manager -> String -> IO (Maybe Value)
getJSON mgr url = do
  req  <- parseRequest url
  resp <- httpLbs req { method = methodGet } mgr
  pure $ if statusIsSuccessful (responseStatus resp)
    then decode (responseBody resp)
    else Nothing

postJSON :: Manager -> String -> Value -> IO (Maybe Value)
postJSON mgr url body = do
  req <- parseRequest url
  let req' = req
        { method      = methodPost
        , requestBody = RequestBodyLBS (encode body)
        , requestHeaders = [("Content-Type", "application/json")]
        }
  resp <- httpLbs req' mgr
  pure $ if statusIsSuccessful (responseStatus resp)
    then decode (responseBody resp)
    else Nothing

putJSON :: Manager -> String -> Value -> IO ()
putJSON mgr url body = do
  req <- parseRequest url
  let req' = req
        { method      = methodPut
        , requestBody = RequestBodyLBS (encode body)
        , requestHeaders = [("Content-Type", "application/json")]
        }
  _ <- httpLbs req' mgr
  pure ()

deleteReq :: Manager -> String -> IO ()
deleteReq mgr url = do
  req <- parseRequest url
  _ <- httpLbs req { method = methodDelete } mgr
  pure ()

-- ── memory ────────────────────────────────────────────────────────────────────

loadMemory :: Manager -> Config -> Text -> IO Text
loadMemory mgr cfg uid = do
  mv <- getJSON mgr (cfgStorageUrl cfg <> "/memory/" <> T.unpack uid)
  pure $ case mv >>= \v -> parseMaybe (.: "content") v of
    Just t  -> t
    Nothing -> ""

saveMemory :: Manager -> Config -> Text -> Text -> IO ()
saveMemory mgr cfg uid content =
  putJSON mgr (cfgStorageUrl cfg <> "/memory/" <> T.unpack uid) (object ["content" .= content])

-- ── history ───────────────────────────────────────────────────────────────────

loadHistory :: Manager -> Config -> Text -> IO [Value]
loadHistory mgr cfg uid = do
  mv <- getJSON mgr (cfgStorageUrl cfg <> "/history/" <> T.unpack uid)
  pure $ case mv >>= \v -> parseMaybe (.: "messages") v of
    Just ms -> ms
    Nothing -> []

appendTurn :: Manager -> Config -> Text -> Value -> [Value] -> IO [Value]
appendTurn mgr cfg uid userMsg assistantMsgs = do
  mv <- postJSON mgr
    (cfgStorageUrl cfg <> "/history/" <> T.unpack uid)
    (object [ "user_message"       .= userMsg
            , "assistant_messages" .= assistantMsgs
            ])
  pure $ case mv >>= \v -> parseMaybe (.: "messages") v of
    Just ms -> ms
    Nothing -> []

clearHistory :: Manager -> Config -> Text -> IO ()
clearHistory mgr cfg uid =
  deleteReq mgr (cfgStorageUrl cfg <> "/history/" <> T.unpack uid)

-- ── skills ────────────────────────────────────────────────────────────────────

loadSkills :: Manager -> Config -> IO (Map.Map Text SkillRecord)
loadSkills mgr cfg = do
  mv <- getJSON mgr (cfgStorageUrl cfg <> "/skills")
  pure $ case mv >>= \v -> parseMaybe (.: "skills") v of
    Just m  -> m
    Nothing -> Map.empty

-- ── notes ─────────────────────────────────────────────────────────────────────

loadNotes :: Manager -> Config -> Text -> IO (Map.Map Text Text)
loadNotes mgr cfg uid = do
  mv <- getJSON mgr (cfgStorageUrl cfg <> "/notes/" <> T.unpack uid)
  pure $ case mv >>= \v -> parseMaybe (.: "notes") v of
    Just m  -> m
    Nothing -> Map.empty

saveNote :: Manager -> Config -> Text -> Text -> Text -> IO ()
saveNote mgr cfg uid name content =
  void $ postJSON mgr
    (cfgStorageUrl cfg <> "/notes/" <> T.unpack uid <> "/" <> T.unpack name)
    (object ["content" .= content])

deleteNote :: Manager -> Config -> Text -> Text -> IO ()
deleteNote mgr cfg uid name =
  deleteReq mgr (cfgStorageUrl cfg <> "/notes/" <> T.unpack uid <> "/" <> T.unpack name)

void :: IO (Maybe Value) -> IO ()
void m = m >> pure ()
