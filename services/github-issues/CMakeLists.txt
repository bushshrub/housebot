cmake_minimum_required(VERSION 3.16)
project(github-issues CXX)
set(CMAKE_CXX_STANDARD 17)

find_package(CURL REQUIRED)
find_package(OpenSSL REQUIRED)

include(FetchContent)
FetchContent_Declare(
  nlohmann_json
  URL https://github.com/nlohmann/json/releases/download/v3.11.3/json.tar.xz
)
FetchContent_MakeAvailable(nlohmann_json)

add_executable(github-issues main.cpp)
target_link_libraries(github-issues
  PRIVATE
  CURL::libcurl
  OpenSSL::SSL
  OpenSSL::Crypto
  nlohmann_json::nlohmann_json
)
