module LlmClient
  ( chatStream
  , chatOnce
  , contextWindowTokens
  ) where

import Config
import Types

import Data.Aeson
import Data.Aeson.Types (parseMaybe)
import Data.Text (Text)
import qualified Data.Text as T
import Network.HTTP.Client
import Network.HTTP.Client.TLS (newTlsManager)
import Network.HTTP.Types (methodPost, methodGet, statusIsSuccessful)
import qualified Data.ByteString.Lazy as BL

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

chatStream :: Manager -> Config -> [Value] -> [Value] -> IO ChatCompletion
chatStream mgr cfg messages tools = do
  let body = object
        [ "model"      .= cfgModel cfg
        , "messages"   .= messages
        , "tools"      .= tools
        , "max_tokens" .= (4096 :: Int)
        ]
  mv <- postJSON mgr (cfgLlmUrl cfg <> "/chat/stream") body
  pure $ case mv >>= \v -> fromJSON v of
    Success c -> c
    _         -> ChatCompletion Nothing [] Nothing 0 0 0

chatOnce :: Manager -> Config -> [Value] -> IO Text
chatOnce mgr cfg messages = do
  let body = object
        [ "model"      .= cfgModel cfg
        , "messages"   .= messages
        , "max_tokens" .= (512 :: Int)
        ]
  mv <- postJSON mgr (cfgLlmUrl cfg <> "/chat/once") body
  pure $ case mv >>= \v -> parseMaybe (.: "content") v of
    Just t  -> t
    Nothing -> ""

contextWindowTokens :: Manager -> Config -> IO (Maybe Int)
contextWindowTokens mgr cfg = do
  req  <- parseRequest (cfgLlmUrl cfg <> "/context_window")
  resp <- httpLbs req { method = methodGet } mgr
  let mv = if statusIsSuccessful (responseStatus resp)
              then decode (responseBody resp) :: Maybe Value
              else Nothing
  pure $ mv >>= \v -> parseMaybe (.: "tokens") v
