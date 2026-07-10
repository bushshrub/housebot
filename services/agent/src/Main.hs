module Main where

import Agent
import Config
import Types

import Data.Aeson
import Data.Text (Text)
import qualified Data.Text as T
import qualified Data.Text.Lazy as TL
import Network.HTTP.Client
import Network.HTTP.Client.TLS (newTlsManager)
import Web.Scotty

main :: IO ()
main = do
  cfg <- fromEnv
  mgr <- newTlsManager
  putStrLn $ "agent listening on port " <> show (cfgPort cfg)
  scotty (cfgPort cfg) $ do

    get "/health" $ text "ok"

    get "/model" $ do
      json $ object ["info" .= modelInfo cfg]

    post "/run" $ do
      req <- jsonData :: ActionM RunRequest
      resp <- liftIO $ runAgent mgr cfg req
      json resp

    post "/session/:user_id/reset" $ do
      uid <- pathParam "user_id" :: ActionM Text
      liftIO $ resetSession mgr cfg uid
      json $ object ["ok" .= True]

    post "/session/:user_id/compact" $ do
      uid <- pathParam "user_id" :: ActionM Text
      liftIO $ compactSession mgr cfg uid
      json $ object ["ok" .= True]
