module Config where

import System.Environment (lookupEnv)
import Data.Maybe (fromMaybe)

data Config = Config
  { cfgLlmUrl     :: String
  , cfgModel      :: String
  , cfgStorageUrl :: String
  , cfgMaxCtx     :: Int
  , cfgPort       :: Int
  }

fromEnv :: IO Config
fromEnv = do
  llm     <- fromMaybe "http://llm-client:3002"  <$> lookupEnv "LLM_CLIENT_URL"
  model   <- fromMaybe "gemma-4-12b-qat-q4kxl"  <$> lookupEnv "LLM_MODEL"
  storage <- fromMaybe "http://storage:3001"     <$> lookupEnv "STORAGE_URL"
  maxCtx  <- maybe 10000 read                    <$> lookupEnv "MAX_CONTEXT_TOKENS"
  port    <- maybe 3003  read                    <$> lookupEnv "PORT"
  pure Config
    { cfgLlmUrl     = llm
    , cfgModel      = model
    , cfgStorageUrl = storage
    , cfgMaxCtx     = maxCtx
    , cfgPort       = port
    }
