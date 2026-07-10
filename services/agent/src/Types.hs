module Types where

import Data.Aeson
import Data.Text (Text)
import qualified Data.Text as T
import Data.Map.Strict (Map)
import GHC.Generics (Generic)

data ImageData = ImageData
  { imageMediaType :: Text
  , imageData      :: Text
  } deriving (Show, Generic)

instance FromJSON ImageData where
  parseJSON = withObject "ImageData" $ \o ->
    ImageData <$> o .: "media_type" <*> o .: "data"

instance ToJSON ImageData where
  toJSON img = object
    [ "media_type" .= imageMediaType img
    , "data"       .= imageData img
    ]

data RunRequest = RunRequest
  { runUserId      :: Text
  , runUsername    :: Text
  , runText        :: Text
  , runImages      :: [ImageData]
  , runPersonality :: Maybe Text
  } deriving (Show, Generic)

instance FromJSON RunRequest where
  parseJSON = withObject "RunRequest" $ \o ->
    RunRequest
      <$> o .:  "user_id"
      <*> o .:  "username"
      <*> o .:  "text"
      <*> o .:? "images"      .!= []
      <*> o .:? "personality"

data RunResponse = RunResponse
  { runText          :: Text
  , runSessionNotice :: Maybe Text
  } deriving (Show, Generic)

instance ToJSON RunResponse where
  toJSON r = object
    [ "text"           .= Types.runText r
    , "session_notice" .= runSessionNotice r
    ]

data SkillRecord = SkillRecord
  { skillName        :: Text
  , skillDescription :: Maybe Text
  , skillPrompt      :: Text
  } deriving (Show, Generic)

instance FromJSON SkillRecord where
  parseJSON = withObject "SkillRecord" $ \o ->
    SkillRecord
      <$> o .:  "name"
      <*> o .:? "description"
      <*> o .:  "prompt"

instance ToJSON SkillRecord where
  toJSON s = object
    [ "name"        .= skillName s
    , "description" .= skillDescription s
    , "prompt"      .= skillPrompt s
    ]

data ToolCall = ToolCall
  { tcId        :: Text
  , tcName      :: Text
  , tcArguments :: Text
  } deriving (Show, Generic)

data ChatCompletion = ChatCompletion
  { ccContent     :: Maybe Text
  , ccToolCalls   :: [ToolCall]
  , ccFinishReason :: Maybe Text
  , ccPromptTokens :: Int
  , ccCompletionTokens :: Int
  , ccCachedTokens :: Int
  } deriving (Show)

instance FromJSON ToolCall where
  parseJSON = withObject "ToolCall" $ \o ->
    ToolCall
      <$> o .: "id"
      <*> o .: "name"
      <*> o .: "arguments"

instance FromJSON ChatCompletion where
  parseJSON = withObject "ChatCompletion" $ \o ->
    ChatCompletion
      <$> o .:? "content"
      <*> o .:? "tool_calls"        .!= []
      <*> o .:? "finish_reason"
      <*> o .:? "prompt_tokens"     .!= 0
      <*> o .:? "completion_tokens" .!= 0
      <*> o .:? "cached_tokens"     .!= 0
